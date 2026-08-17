> Status: ADOPTED implementation plan, 2026-08-17. This document governs the
> above-core MCP work on `codex/mcp-native-harness`. It describes intended
> behavior, not behavior shipped by v0.11.0. Each phase becomes a current claim
> only after its acceptance tests and repository gates pass.

# MCP-native single-agent harness

## Decision

`roba-core` remains the provider-neutral engine for one finite root run. A new
`roba-mcp` layer will own one hot logical agent, expose that agent through one
canonical MCP contract, and create a fresh finite core run for each submitted
turn.

The same composed service is available to two kinds of client:

- operators, CLIs, REPLs, and automation use a control projection;
- the Claude or Codex process executing the active run may use an
  agent-facing projection.

The projections share one `AgentInstance`, workspace, provider session,
extension state, and event history. They do not have to advertise identical
capabilities. In particular, the agent-facing projection must not expose
self-recursive control such as starting another turn or shutting down its own
harness.

This is an MCP-native harness, not merely an MCP facade over a provider. MCP is
the application contract for invoking the agent, observing it, and composing
services such as context, Git, and GitHub. The underlying domain core remains
free of transport and Tower MCP types.

## Architectural invariant

One constructed `AgentInstance` is one logical agent in one workspace with at
most one active finite run.

An `McpRouter` may be cloned by Tower, and several role-specific routers or
transport bindings may capture the same `Arc<AgentInstance>`. Those mechanical
router objects do not create more agents. Creating a new `AgentInstance` does.

The base layer contains no agent registry, multi-agent routing, hidden queue,
durable task store, scheduler, or persistent session pool.

## Layers

```text
layer 4  clients and bindings
         roba CLI | mcp-repl | in-process | stdio | Unix | HTTP

layer 3  composable service fragments
         context | aliases | receipts | Git | GitHub | project services

layer 2  MCP-native agent harness
         AgentInstance | control projection | agent projection

         provider-neutral domain kernel
         roba-core Run | RunHandle | outcomes | failures | events

layer 1  provider adapters and reusable wrappers
         ClaudeProvider/claude-wrapper | CodexProvider/codex-wrapper

layer 0  provider executables
         claude | codex
```

`roba-core` and `roba-mcp` are deliberately separate crates. Core owns domain
semantics. `roba-mcp` owns the long-lived agent application state, MCP schema,
router composition, and bindings.

## Ownership boundaries

### `roba-core`

Core continues to own:

- `RunSpec`, provider identity, context intent, permissions, and limits;
- fresh or resumed provider session identity;
- validation and execution of one finite run;
- normalized output, structured output, usage, cost, and typed failure;
- per-run status, replayable events, steering, cancellation, and waiting;
- the process-local provider registry.

Core does not learn about MCP requests, JSON-RPC, clients, router fragments,
network listeners, task stores, authentication principals, or service
discovery.

### `roba-mcp`

The harness layer owns:

- one suspended `RunSpec` template;
- the retained provider `SessionHandle`;
- `Idle`, `Running`, `Stopping`, and `Stopped` agent state;
- at most one current `RunHandle`;
- monotonically increasing operation identity;
- the latest terminal turn record;
- a bounded agent-wide event journal over multiple finite runs;
- MCP tools, resources, prompts, Tasks integration, and structured results;
- fail-closed composition of router fragments;
- role-specific control and agent projections;
- transport lifetime and private provider-facing endpoint lifetime.

### Host and workflow layers

The root binary or another Rust host owns:

- which providers and service fragments are installed;
- which transport is bound and how long it lives;
- workspace selection and transport security;
- rendering, stdout/stderr policy, and process exit codes;
- any queue, retry, scheduling, durable workflow, or federation policy.

Steward remains one example of a workflow layer above this contract. It may
drive one Roba agent and retain its own visible queue, lock, receipts, and
verification policy.

## Agent lifecycle

```text
                 submit turn
        +----------------------------+
        |                            v
      idle <--- completed/failed/cancelled --- running
        |                                      |
        | shutdown                             | shutdown
        v                                      v
      stopped <---------- stopping <-----------+
```

An ordinary turn performs the following steps:

1. Refuse an empty prompt, a stopped instance, or a concurrent submission.
2. Allocate a new operation id.
3. Clone the suspended `RunSpec` template.
4. Set `ExecutionSpec.session` from the retained provider session.
5. Create a new finite `Run` and publish its handle as active.
6. Start the run without holding the agent control lock.
7. Fan per-run events into the agent-wide journal with the operation id.
8. Await the terminal snapshot or explicit cancellation.
9. Retain session evidence from a successful outcome or terminal failure,
   falling back to an already-known resumed handle when appropriate.
10. Record the terminal result and return to `Idle` unless shutdown won the
    race.

A provider failure terminates one turn, not the hot agent. Only shutdown makes
the logical agent permanently refuse new work.

The harness stays hot; provider processes do not. Claude and Codex are spawned
per finite run and resume through their opaque provider session handles.

## Concurrency policy

The base harness is single-flight. A second turn submitted while one is active
returns a typed `busy` application result.

The base must not silently queue the turn or reinterpret it as steering. A
higher layer may implement a queue by waiting and submitting later. Guidance
for the currently active finite run is an explicit `agent.steer` operation and
retains `RunHandle::steer` semantics.

All router handlers must avoid holding the agent control mutex while awaiting
provider work or extension calls. The provider may call back into the same
harness during its turn; holding the lock across the provider future would
deadlock that re-entry.

## Base MCP contract

The conceptual prompt tool is named `agent.turn`. The distinct name avoids a
collision with the MCP Prompt primitive and the `prompt` command in
`mcp-repl`.

### Tools

`agent.turn`

- Input: a required non-empty text prompt.
- Ordinary call: waits and returns the terminal turn result.
- Task-aware call: returns an MCP Task and completes it when the finite run
  settles.
- Concurrent call: returns a typed `busy` application error.

`agent.steer`

- Queues guidance at the active run's next safe provider-turn boundary.
- Refuses when idle or when the provider cannot resume.
- Does not represent portable mid-token injection.

`agent.interrupt`

- Cancels the active finite run.
- Waits until the run has settled cancelled.
- Leaves the logical agent reusable and idle.

`agent.shutdown`

- Refuses future turns permanently.
- Cancels and drains active work before reporting completion.
- Signals the owning binding to stop accepting requests and exit cleanly.

### Resources

`roba://agent`

- Provider, configured policy, agent state, session availability, current
  operation identity, latest terminal operation, and timestamps.
- Credentials and provider-private configuration are never included.

`roba://events`

- Cursor-paged agent-wide events.
- Each record has an agent sequence and operation id around the normalized
  core event.
- Eviction and future-cursor behavior match the fail-loud core journal.

Additional current/latest-operation resources may be introduced only when a
concrete client needs them. The base does not duplicate every status value as
both a tool and resource without evidence.

### Result and error contract

Successful turns return:

- ordinary MCP text content containing the assistant answer when present;
- typed `structuredContent` containing operation identity, terminal state,
  normalized outcome, session evidence, usage, cost, and timing.

Provider and domain failures are tool execution results with `isError: true`
and typed structured failure content. JSON-RPC errors are reserved for malformed
protocol input, missing capabilities, authorization failures, and server bugs.

The root CLI must consume structured content. It must never parse human error
text to recover Roba exit codes.

## MCP Tasks and cancellation

`agent.turn` supports both normal and Task-aware callers through Tower MCP's
live task handler plus synchronous fallback.

The Task path must explicitly select over the finite run and
`TaskContext::cancelled()`. On task cancellation it calls the matching
`RunHandle::cancel`, waits for settlement, and only then returns a cancelled
Task outcome. A task id or operation generation guards this path so delayed
cancellation cannot affect a later run.

The direct-call path also defines its disconnect behavior explicitly. Dropping
an MCP handler future is not sufficient because the core run is supervised by
its own task. Bindings either keep the operation running and observable or
invoke `agent.interrupt`; they must not accidentally orphan provider work.

The process-scoped harness uses an in-memory MCP task store. Durable Tasks,
restart reconciliation, and a background job database are workflow-layer
features and are not implied.

## Router composition

An extension may expose Tower MCP `Tool`, `Resource`, `Prompt`, or
`McpRouter` values. Initial composition is static at harness construction.

The host uses `McpRouter::try_merge` or namespaced composition and fails startup
on collisions. Last-writer-wins merging is not acceptable for extension code
because it can silently replace a security or control capability.

Examples:

- aliases and reusable recipes become MCP Prompts;
- inspectable context becomes MCP Resources;
- explicit context operations become tools;
- receipts and telemetry become resources or event observers;
- Git and GitHub packages contribute typed tools and resources;
- project-specific packages capture a narrow `AgentHandle` when they need
  current state.

Router merging does not automatically intercept `agent.turn`. Mandatory
context injection therefore uses an explicit, ordered pre-turn composition
hook. That hook is part of the agent harness and must be deterministic,
bounded, and covered for repeated resumed turns.

Dynamic installation by the model is out of scope. The host chooses modules
and authority before the turn.

## Control and agent projections

The control projection is the northbound contract for operators and clients.
The agent projection is the work contract injected into the provider process.

Both are built from the same service modules and capture the same
`AgentInstance`, but discovery must show only capabilities the caller can use.
Runtime denial without discovery filtering is not sufficient because it gives
the model a misleading tool catalog and increases accidental calls.

At minimum, the agent projection excludes:

- `agent.turn`;
- `agent.interrupt` and `agent.shutdown`;
- authority and module configuration;
- credentials and provider-private session state.

Read-only status or event resources may be shared. Reporting, request-input,
or completion tools may be added as optional modules rather than core mission
machinery.

## Provider as a client of Roba

Before starting a provider turn, the harness may bind the agent projection to
a private authenticated endpoint. A non-serializable launch context passes
that endpoint to the provider adapter:

```text
ProviderLaunchContext
  mcp_endpoints: Vec<ProviderMcpEndpoint>

ProviderMcpEndpoint
  stable server name
  ephemeral URL
  redacted bearer credential
```

The endpoint and credential are host-local launch material. They never enter
`RunSpec`, JSON output, receipts, debug formatting, or persisted provider
session metadata.

Claude and Codex adapters translate the same provider-neutral endpoint into
their wrapper-native configuration. The endpoint name remains stable across
resumed turns; its credential may rotate for every finite run.

The provider-facing credential is scoped to the active operation and revoked
or made unreachable when that operation settles. In-flight service calls are
cancelled or drained as part of run cancellation.

The historical provider context and endpoint code is prior art for this seam,
but worker and process controls are not restored with it.

### Authority honesty

Providing Git or GitHub through MCP does not by itself make MCP the exclusive
authority boundary. If the provider retains ambient shell, network, or `gh`
access, the MCP tools are a consistent interface and audit path, not complete
enforcement. Roba must describe isolation honestly and align provider-native
tool restrictions with the selected permission policy before claiming that
all mutations pass through the harness.

## Bindings

All bindings serve the same typed contract:

- `ChannelTransport` is the production in-process path used by `roba run` and
  integration tests;
- stdio is the first hot external binding and keeps stdout byte-clean MCP;
- Unix domain socket support is local-only, uses a private parent directory,
  controlled socket permissions, peer checks where available, and explicit
  cleanup;
- HTTP binds loopback by default and requires a bearer capability, OAuth, or
  equivalently strong authentication before remote exposure.

One binding per process is the default until a use case proves otherwise.
Several bindings may share one `AgentInstance`, but all authorized callers then
share authority over that same agent and active run.

Transport EOF or server shutdown has an explicit policy. Stdio EOF shuts down
the agent and drains provider work. Network bindings use graceful shutdown and
do not leave hidden provider children.

## Compatibility boundary

The first client migration is only provider-neutral `roba run`.

Its flags, stdout answer, stderr metadata, versioned JSON, empty-result
classification, and typed exit codes remain compatible. Its implementation
changes from direct `Run::begin`/`RunHandle::wait` calls to an in-process MCP
client calling `agent.turn`.

The released legacy Claude one-shot command remains untouched until a separate
compatibility decision. Profiles, aliases, bundles, receipts, history, and
inspection commands are not silently pulled into the new contract.

The deleted custom `roba-repl` remains deleted. `mcp-repl` is the interactive
client and dogfood target.

## Roba driving Roba

Once one Roba agent has an authenticated control endpoint, another Roba can be
an ordinary MCP client of it. This enables observation, task-backed turn
submission, steering, and higher-level delegation without adding child-agent
semantics to `roba-core`.

This is a consequence of the contract, not part of the initial implementation.
Federation requires explicit answers for authority delegation, cycle
prevention, cancellation propagation, budgets, endpoint discovery, and
failure ownership. It remains parked until two independently useful Roba
instances and a concrete driver need it.

The one-agent invariant still applies to each instance. A higher layer may
drive several Robas; no individual router becomes a multi-agent server.

## Delivery protocol

Every phase is a discrete local commit. A phase is complete only when:

1. its stated behavior and refusal cases are covered by tests;
2. phase-specific integration tests pass;
3. all common repository gates pass;
4. documentation describes only behavior that now exists;
5. `git diff --check` is clean;
6. the phase evidence and any changed decisions are recorded here;
7. no work from the next phase is bundled into the commit.

If a gate fails, work remains in the current phase. Later phases do not begin
on top of a knowingly failing checkpoint.

The common gate is:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo build --workspace --all-features
cargo test --workspace --lib --all-features
cargo test --test cli --all-features
cargo test --workspace --doc --all-features
RUSTDOCFLAGS='-D warnings' cargo doc --workspace --all-features --no-deps
cargo build --release --all-features
git diff --check
```

Once `roba-mcp` exists, every phase also runs
`cargo test -p roba-mcp --all-features` so crate integration tests under
`roba-mcp/tests` cannot be skipped by a library-only workspace command.

Ignored live tests may spend money and are never folded silently into the
common gate. A relevant phase adds the ignored mechanical smoke test, compiles
it in the common gates, and records a separately authorized paid run when one
is warranted.

## Phases

### Phase 0 -- architecture record

Deliverables:

- adopt this design and update stale cleanup scope text;
- record the phase protocol, common gate, and parked federation consequence;
- make no production behavior change.

Acceptance:

- documentation distinguishes current behavior from adopted work;
- `roba-core` remains finite and protocol-free;
- hot agent lifetime, provider self-client, and no-queue policies are explicit;
- common gates pass on the documentation checkpoint.

### Phase 1 -- minimal in-process agent vertical

Deliverables:

- reintroduce `roba-mcp` as a workspace crate;
- implement `AgentInstance` with `Idle`, `Running`, and `Stopped` state;
- retain a suspended run template and provider session;
- implement typed `agent.turn` and `roba://agent`;
- expose an in-process `McpClient` over `ChannelTransport`;
- no root CLI migration and no external binding yet.

Required tests:

- construction remains idle and spawns no provider;
- first turn creates one finite run and retains its session;
- second turn creates a distinct finite run and resumes that session;
- an explicitly seeded resume handle is used on the first turn;
- success and failure session evidence carry forward correctly;
- exactly one concurrent submission wins and the other returns typed `busy`;
- failed turns return the agent to reusable idle state;
- stopped agents refuse work;
- failed tool calls return `isError: true` with typed structured failure
  content;
- tool schema and structured output are asserted through the MCP client, not
  by calling the handler directly.

Exit criterion:

> The same in-process router completes two sequential prompts through one
> provider conversation without changing core finite-run semantics.

### Phase 2 -- `roba run` becomes an MCP client

Deliverables:

- split `bounded::resolve_spec` into a suspended template plus prompt;
- call `agent.turn` through the in-process MCP client;
- decode typed structured results into existing CLI success and failure paths;
- keep legacy root-command dispatch unchanged.

Required tests:

- all existing `roba run` flag resolution tests remain;
- plain stdout is exactly the final answer;
- `--json` remains byte-clean and versioned;
- authentication, timeout, limit, provider, cancelled, and empty-result
  classifications retain their exit codes;
- malformed MCP structured content fails closed rather than being parsed from
  display text;
- direct core tests still pass independently of MCP.

Exit criterion:

> The shipped provider-neutral CLI crosses MCP in-process with no observable
> ABI regression.

### Phase 3A -- controls and agent-wide observation

Deliverables:

- add `agent.steer`, `agent.interrupt`, and `agent.shutdown`;
- add agent-wide event replay and `roba://events`;
- define direct-request cancellation behavior;
- preserve a reusable idle agent after interruption.

Required tests:

- direct interrupt cancels the matching core run;
- cancellation settles before the reported terminal state;
- interrupt/completion and shutdown/completion races settle once;
- steering reaches the active run and refuses invalid states;
- event sequence remains monotonic across several finite runs;
- truncation and future cursors fail loudly;
- shutdown permanently refuses turns and drains active work;
- provider or driver panic cannot leave an eternally running agent.

This phase must make session continuity after interruption explicit. A known
resumed handle can be retained. A fresh turn interrupted after the provider
has established a session may currently lose that new handle. Acceptance must
either add and test a narrow session-established provider event or document
and test that the next turn starts fresh. It must not imply continuity without
evidence.

Exit criterion:

> A long operation can be observed, steered, or interrupted without ending or
> corrupting the logical agent.

### Phase 3B -- MCP Tasks

Deliverables:

- add optional Tasks support for `agent.turn` with synchronous fallback;
- map `tasks/cancel` explicitly to the matching active finite run;
- preserve result and failure parity between synchronous and Task paths;
- use Tower MCP's task machinery rather than inventing a Roba task store.

Required tests:

- synchronous and Task paths return equivalent typed structured results;
- `tasks/cancel` cancels only its matching operation and awaits settlement;
- stale Task cancellation cannot affect a later operation;
- Task cancellation/completion races settle once;
- a cancelled Task leaves the logical agent reusable;
- non-Task clients continue to discover and call the same `agent.turn` tool.

Exit criterion:

> A task-aware client can background, poll, and cancel a turn without changing
> behavior for ordinary clients.

If Tower cannot provide this contract cleanly, park Phase 3B and continue only
with an explicit design update. Do not build a custom task subsystem to hold
the schedule.

### Phase 4 -- provider-facing self-client projection

Deliverables:

- restore a minimal non-serializable provider launch context containing only
  MCP endpoints;
- build separate control and agent projections over one `AgentInstance`;
- bind the agent projection privately with a run-scoped credential;
- attach that endpoint mechanically through Claude and Codex adapters;
- add one harmless callback tool/resource as the deterministic re-entry proof;
- do not add Git, GitHub, workers, or process controls yet.

Required tests:

- provider launch configuration contains the endpoint without logging its
  credential;
- provider-native command/config construction is exact for Claude and Codex;
- an executing fake provider calls back into the harness without deadlock;
- agent discovery excludes turn, interrupt, shutdown, and authority controls;
- an invalid or expired credential is refused;
- callback cancellation and run cancellation drain each other;
- Claude's temporary MCP configuration is private and removed;
- Codex receives its credential through an environment variable, not argv;
- endpoint state and credentials never serialize into run or MCP results;
- fake Claude and Codex binaries prove exact endpoint attachment mechanically.

A separately authorized real-provider callback may be recorded as manual
dogfood. It is not a pass/fail test because model compliance is not a stable
mechanical assertion.

Exit criterion:

> The active provider is an authenticated MCP client of its own harness while
> an operator turn is in progress.

### Phase 5 -- hot stdio and `mcp-repl` dogfood

Deliverables:

- add a foreground stdio serve command for one configured agent;
- keep stdout exclusively MCP wire data and diagnostics on stderr;
- bind transport EOF and signals to graceful agent shutdown;
- document and exercise the `mcp-repl` workflow.

Required tests:

- stdio serves the same schemas and structured results as ChannelTransport;
- requests dispatch concurrently so observation and interruption overtake a
  long turn;
- EOF and shutdown drain the active provider;
- no provider child remains after server exit;
- an `mcp-repl` smoke starts a Task-backed turn, reads status/events, and
  cancels or awaits it.

Exit criterion:

> A user can keep one Roba hot in `mcp-repl` without a custom Roba REPL.

### Phase 6 -- extension composition and first workspace service

Deliverables:

- publish a fail-closed router-fragment composition convention;
- add deterministic pre-turn context composition only if the proof needs it;
- implement one narrow Git fragment before a GitHub service;
- expose the fragment through appropriate control and agent projections.

Required tests:

- name and URI collisions fail at startup;
- discovery differs correctly by projection;
- read-only policy cannot reach mutating Git tools;
- resumed turns do not duplicate injected durable context;
- operator and provider clients observe the same repository state;
- Git behavior uses fixtures and does not depend on the developer repository.

Exit criterion:

> A separately testable workspace service composes into Roba and is usable by
> both an operator and the active provider under explicit authority.

### Phase 7 -- optional local and network bindings

Deliverables are demand-driven rather than automatic:

- Unix socket binding with private filesystem lifecycle;
- secure loopback HTTP with authentication and graceful shutdown;
- no remote exposure by default.

Each binding must pass the same contract suite as ChannelTransport and stdio,
plus binding-specific authentication, origin/host, concurrency, cleanup, and
shutdown tests.

### Parked -- Roba-to-Roba federation

Before implementation, write a separate decision using a concrete use case.
It must define capability delegation, cycle prevention, cancellation and Task
propagation, budgets, endpoint discovery, failure ownership, and audit
identity. Federation does not reopen `roba-core` as a multi-agent runtime.

## Current phase ledger

| Phase | Status | Evidence |
|---|---|---|
| 0. Architecture record | Complete | Design review plus full common gate green, 2026-08-17 |
| 1. Minimal in-process agent | Not started | -- |
| 2. CLI over MCP | Not started | -- |
| 3A. Controls and events | Not started | -- |
| 3B. MCP Tasks | Not started | -- |
| 4. Provider self-client | Not started | -- |
| 5. Hot stdio | Not started | -- |
| 6. First extension | Not started | -- |
| 7. Optional bindings | Parked pending demand | -- |
| Federation | Parked pending separate decision | -- |

## Resume checklist

1. Read this document and `AGENTS.md` completely.
2. Inspect the current phase ledger and latest phase commit.
3. Confirm the work belongs to the active phase.
4. Keep `roba-core` finite and protocol-free.
5. Preserve the legacy CLI unless the active phase explicitly changes it.
6. Add refusal and race tests with the positive path.
7. Run the phase-specific tests and full common gate.
8. Update the ledger with exact evidence before the phase commit.
9. Commit one green conventional checkpoint and start the next phase only from
   a clean worktree.
