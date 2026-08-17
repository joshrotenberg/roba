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
5. Create a new finite `Run` and subscribe to its event journal.
6. Perform the bounded core start transition before releasing admission, then
   publish the already-running handle as active. This transition only spawns
   the supervised driver; it does not await provider work.
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
- Waits until the run has settled; completion or failure may win the race.
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

Tower allocates the Task before Roba admits work. A Task preparation step then
captures an opaque ticket for the exact finite run and publishes its operation
id as Task metadata before returning the Task handle. The live handler selects
over that ticket's settlement and `TaskContext::cancelled()`. It never resolves
whichever operation happens to be current later, so delayed cancellation
cannot affect a subsequent run.

`tasks/cancel` is an immediate, eventually consistent acknowledgment. The live
handler applies cancellation to the captured `RunHandle`, waits for agent
settlement, and only then returns. Clients use `tasks/get` or `task_wait` as
the settlement barrier. Completion wins when it is already observable; a
Task cancellation that wins and settles the core run cancelled becomes a
cancelled MCP Task. MCP's cancelled Task shape does not carry a
`CallToolResult`, so exact structured-result parity applies to completed Task
outcomes. That includes provider and domain failures: `isError: true` remains
a completed Task carrying Roba's typed failure.

The direct-call path deliberately separates request lifetime from admitted
agent work. Dropping a synchronous MCP waiter leaves the exact operation
running, supervised, and observable; the caller or another operator uses
`agent.interrupt` or `agent.shutdown` to end it. This is pinned independently
of transport cancellation because Tower MCP 0.22's `ChannelTransport` does
not forward `notifications/cancelled`. Task-backed cancellation is a separate
execution-cancellation contract and targets its captured operation id.

The process-scoped harness uses Tower's in-memory MCP task store. Roba extends
the active lease so a long live run cannot disappear under Tower's default
five-minute TTL, shortens it after settlement to retain the result for at
least five minutes, and reclaims expired records on the next Task admission.
Durable Tasks, restart reconciliation, and a background job database are
workflow-layer features and are not implied.

## Router composition

An extension exposes explicit control and provider `McpRouter` fragments.
Composition is static at harness construction. `AgentExtensions::try_with`
and `AgentInstance::new_with_extensions` use `McpRouter::try_merge` and fail
startup on exact capability collisions. Last-writer-wins merging is not
acceptable because it can silently replace a security or control capability.
Fragments are capability bags: router identity, session state, task stores,
auth, and middleware are owned by the fresh Roba projection roots rather than
imported from fragments.

Examples:

- aliases and reusable recipes become MCP Prompts;
- inspectable context becomes MCP Resources;
- explicit context operations become tools;
- receipts and telemetry become resources or event observers;
- Git and GitHub packages contribute typed tools and resources;
- project-specific packages capture a narrow `AgentHandle` when they need
  current state.

Router merging does not automatically intercept `agent.turn`. Phase 6 did not
add a pre-turn composition hook because the Git proof reads live state through
MCP. If a later service needs mandatory context injection, that is a separate
ordered, bounded contract with repeated-resume coverage rather than an
implicit side effect of router installation.

Dynamic installation by the model is out of scope. The host chooses modules
and authority before the turn.

### First adopted service: `roba-git`

The opt-in `roba-git` package captures one canonical repository at host
construction. It exposes the same deterministic `git.snapshot` tool and
`roba://git/workspace` resource state through control and provider fragments.
Reads are bounded, disable optional Git locks and configured filesystem
monitoring, and never accept a caller-selected cwd.

Writable control projections also expose `git.stage_all`, a typed workflow
that refuses conflicts and no-op requests and returns before/after snapshots
plus the exact resulting index tree. Provider projections remain read-only:
staging may execute repository-configured filters as host processes, which is
broader authority than provider workspace-write alone. Raw Git remains the
escape hatch.

### Remaining extension parking lot

These are candidate packages, not adopted deliverables. Each must justify its
workflow semantics beyond wrapping a command and must leave the lower-level
tool available as an escape hatch:

- `roba-gh` may use `octocrab` for typed GitHub operations and expose useful
  development-process steps, but a thin replacement for `gh` is not enough;
- `roba-tick` may expose CRUD scheduling through MCP and submit turns,
  steering, interruption, or shutdown as an ordinary client of a hot agent.
  Scheduling remains a supervisor above the finite core. Any agent-controlled
  cadence needs explicit bounds and an audit history;
- `roba-pattern` may package prompts, skills, and work patterns such as staged
  phases or provider-native delegation. Intentional self-calls require a
  separate bounded recursion decision. Periodic session summarization and
  rotation may reveal one small host primitive, while summarization policy
  stays in the extension.

The shared MCP contract makes these composable; it does not by itself make
them necessary or safe.

## Control and agent projections

The control projection is the northbound contract for operators and clients.
The agent projection is the work contract injected into the provider process.

Both are built from the same service modules and capture the same
`AgentInstance`, but discovery must show only capabilities the caller can use.
Runtime denial without discovery filtering is not sufficient because it gives
the model a misleading tool catalog and increases accidental calls.

The base agent projection is an explicit allowlist containing only the
read-only `self` tool. An installed fragment contributes only its explicit
provider surface; the control surface is never mirrored. `roba-git` adds
bounded `git.snapshot` observation while keeping staging operator-only. The
provider projection always excludes:

- `agent.turn`;
- `agent.steer`, `agent.interrupt`, and `agent.shutdown`;
- Tasks, agent state, and event history;
- authority and module configuration;
- credentials and provider-private session state.

Later reporting, request-input, or completion tools may be added as optional
modules rather than core mission machinery. Sharing another control resource
with the provider requires an explicit least-authority decision; it does not
happen automatically.

## Provider as a client of Roba

Before starting a provider turn, the harness binds the agent projection to a
private authenticated loopback endpoint. A non-serializable launch context
passes that endpoint to the provider adapter:

```text
ProviderLaunchContext
  mcp_endpoints: Vec<ProviderMcpEndpoint>

ProviderMcpEndpoint
  stable server name
  ephemeral URL
  redacted bearer credential
  exact provider-callable tool names
```

The endpoint and credential are host-local launch material. Roba does not
structurally place them in `RunSpec`, `TurnRequest`, JSON result schemas,
receipts, event projections, or persisted provider session metadata. Debug
formatting may include the loopback URL for diagnosis but redacts the bearer
credential. The executing provider necessarily receives the credential and
can repeat any observed text in its own output; launch-context redaction is
not a content-filtering security boundary.

Claude and Codex adapters translate the same provider-neutral endpoint into
their wrapper-native configuration. The endpoint name remains `roba` and the
same endpoint is reused across resumed provider turns in one finite run. A new
finite run binds a new listener and rotates the credential; the operating
system may reuse a numeric port after the prior listener closes.

The provider-facing credential is scoped to the active operation and revoked
when that operation settles. Run cancellation drops the provider future,
revokes the credential, stops accepting requests, and waits for the endpoint
server before publishing agent settlement. The base `self` callback is
immediate and `roba-git` bounds its read calls. A future extension with
arbitrary or long-running provider callbacks must add explicit request
tracking and cancellation before claiming that in-flight handlers are
forcibly drained.

The historical provider context and endpoint code is prior art for this seam,
but worker and process controls are not restored with it.

### Authority honesty

Providing Git or GitHub through MCP does not by itself make MCP the exclusive
authority boundary. If the provider retains ambient shell, network, or `gh`
access, the MCP tools are a consistent interface and audit path, not complete
enforcement. Provider-native approval contains the exact tool names actually
present in its fragment, but approval is not authorization; forbidden tools
must be absent from the provider router. Phase 6 keeps `git.stage_all`
operator-only instead of treating workspace-write as permission to launch
host-side Git filters.

## Bindings

All bindings serve the same typed contract:

- `ChannelTransport` is the production in-process path used by `roba run` and
  integration tests;
- foreground stdio is the hot external binding used by `roba serve`; it keeps
  stdout byte-clean MCP and supports the legacy and final lifecycle;
- Unix domain socket support is local-only, uses a private parent directory,
  controlled socket permissions, peer checks where available, and explicit
  cleanup;
- HTTP binds loopback by default and requires a bearer capability, OAuth, or
  equivalently strong authentication before remote exposure.

One binding per process is the default until a use case proves otherwise.
Several bindings may share one `AgentInstance`, but all authorized callers then
share authority over that same agent and active run.

Transport EOF or server shutdown has an explicit policy. Stdio EOF begins
logical agent shutdown before Tower drains in-flight requests, so a long
synchronous turn cannot deadlock transport exit. Logical `agent.shutdown`
stops the transport in the other direction only after closing admission and
draining provider work; its response is flushed before the binding returns.
Network bindings use graceful shutdown and do not leave hidden provider
children.

## Compatibility boundary

The first client migration was provider-neutral `roba run`. The first external
binding adds the unreleased `roba serve` subcommand without changing that run
projection.

Its flags, stdout answer, stderr metadata, versioned JSON, empty-result
classification, and typed exit codes remain compatible. Its implementation
changes from direct `Run::begin`/`RunHandle::wait` calls to an in-process MCP
client calling `agent.turn`.

Compatibility applies to admitted run requests. Inputs that cannot construct
a truthful agent host, currently a blank seeded resume id or a negative or
non-finite cost ceiling, fail during host preflight. In JSON mode they emit a
structured error on stderr without a terminal snapshot on stdout because no
finite run existed. The former direct path admitted those invalid values far
enough to manufacture a failed snapshot; Phase 2 intentionally tightens that
edge rather than representing preflight failure as provider work.

The released legacy Claude one-shot command remains untouched until a separate
compatibility decision. Profiles, aliases, bundles, receipts, history, and
inspection commands are not silently pulled into the new contract.

Adding `serve` reserves that bare subcommand spelling. The literal legacy
prompt remains available as `roba -p serve`, but a user-defined alias named
`serve` is shadowed by the built-in subcommand. This is the deliberate Phase 5
CLI compatibility exception; all released one-shot flags and explicit prompt
forms remain available.

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
`crates/roba-mcp/tests` cannot be skipped by a library-only workspace command.

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
> ABI regression for admitted runs; construction-invalid configuration fails
> earlier and never claims that a run existed.

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
- add one harmless callback tool as the deterministic re-entry proof;
- do not add Git, GitHub, workers, or process controls yet.

Required tests:

- provider launch configuration contains the endpoint without logging its
  credential;
- provider-native command/config construction is exact for Claude and Codex;
- an executing fake provider calls back into the harness without deadlock;
- agent discovery excludes turn, interrupt, shutdown, and authority controls;
- an invalid or expired credential is refused;
- run cancellation drops the provider-owned callback client and revokes the
  private endpoint before settlement is published;
- Claude's temporary MCP configuration is private and removed;
- Codex receives its credential through an environment variable, not argv;
- endpoint state and credentials are not structurally serialized into run or
  MCP results;
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

## Phase 1 checkpoint decisions

- `AgentInstance` owns terminal settlement independently of the Rust caller
  awaiting a turn. Dropping that caller cannot strand the logical agent in
  `Running`.
- This checkpoint did not define MCP request-cancellation behavior. Phase 3A
  later chose observable direct-call detachment plus explicit operation-scoped
  interruption; Task cancellation was deferred to and completed in Phase 3B.
- `roba://agent` publishes only whether a provider session is available and
  redacts session identifiers from its retained latest-turn projection. The
  direct `agent.turn` result keeps validated session evidence for the caller
  that initiated the operation.

## Phase 2 checkpoint decisions

- The CLI trusts only decoded `agent.turn` `structuredContent`. Display text
  is never parsed as machine data, and missing, malformed, or contradictory
  tool results fail closed.
- The MCP result is projected back into the established `RunSnapshot` before
  rendering. MCP operation ids and result tags do not leak into the v0.11 JSON
  ABI, and typed failure details still drive exit codes and receipt cost.
- A one-shot client shuts down its process-local MCP connection after the turn
  call on both success and error paths.
- Blank seeded session ids and negative or non-finite cost ceilings are now
  host-preflight errors. This accepted invalid-input tightening is covered by
  a CLI test and intentionally emits no synthetic terminal snapshot.

## Phase 3A checkpoint decisions

- Steering and interruption require the active operation id. The id is a
  generation fence: stale controls fail closed and cannot affect a later turn.
- Dropping a direct MCP `agent.turn` waiter detaches that waiter only. The
  admitted operation remains supervised and visible until it settles or an
  explicit interrupt or shutdown drains it. MCP Task cancellation will cancel
  its captured operation in Phase 3B.
- Agent-wide event replay has its own bounded global sequence while preserving
  the source finite-run sequence. Source and agent history loss are both
  explicit, and terminal provider events are copied before agent settlement is
  published.
- Shutdown first closes admission with `Stopping`, drains any active finite
  run, and only then publishes `Stopped`. Concurrent callers share the same
  settlement. Phase 3A shut down the logical agent only; binding EOF, signal,
  and graceful transport exit were deferred to and completed in Phase 5.
- Interruption retains a provider session already known before the cancelled
  turn. A fresh turn interrupted before terminal session evidence is exposed
  cannot claim the provider-private session; its next turn starts fresh.

## Phase 3B checkpoint decisions

- `agent.turn` is one optional dual-path tool. Tower's Task preparation admits
  and captures the exact finite run before publishing the Task handle; the
  synchronous fallback keeps the direct-call behavior unchanged.
- Roba publishes the captured operation id under
  `com.github.joshrotenberg.roba/operation` Task metadata. Task cancellation
  uses the opaque admission ticket itself rather than a task-id lookup or the
  later current operation.
- `tasks/cancel` acknowledges immediately. Polling `tasks/get` or awaiting the
  Task is the settlement barrier. An already-observable completion wins a
  cancellation race; otherwise Roba cancels and drains the finite run before
  reporting the Task cancelled.
- MCP cancelled Tasks cannot carry a tool result. Exact display,
  `structuredContent`, and `isError` parity applies to completed Task outcomes,
  including typed Roba provider failures. The agent snapshot and event replay
  retain the underlying Roba cancellation settlement.
- Tower's `MemoryTaskStore` remains the only task store. Active turns receive
  a process-lifetime practical lease, settled results remain visible for at
  least five minutes, and the next Task admission reclaims expired records.
  Persistence and restart reconciliation remain outside the base harness.
- The router opts into the final 2026-07-28 protocol and catches live-handler
  panics so one glue-layer fault cannot leave a Task working forever. Provider
  panics still settle through Roba's typed finite-run failure path.

## Phase 4 checkpoint decisions

- `ProviderLaunchContext` is a transient value beside `RunSpec` and
  `TurnRequest`, not a field inside either serializable contract. The provider
  trait keeps its original execution method and adds a defaulted
  context-aware method, so existing third-party providers remain source
  compatible. One context is reused across steered provider turns within a
  finite run.
- Every admitted `AgentInstance` operation binds one private IPv4 loopback MCP
  endpoint before provider work starts. Its provider-native server name is
  `roba`; a new finite operation binds a new endpoint and rotates its bearer
  credential.
- The provider router is an explicit allowlist with one immediate, read-only
  tool named `self`. It has no control tools, Tasks, resources, configuration,
  events, or retained session evidence. Recursive `agent.turn` access is
  absent from discovery and dispatch rather than rejected after admission.
- Claude receives the endpoint through an owner-private temporary MCP file and
  an exact `mcp__roba__self` allowlist entry. Codex receives wrapper-generated
  MCP overrides and the bearer only through a child environment variable,
  with approval narrowed to `roba.self`. The ordinary direct provider APIs
  continue to use an empty launch context.
- Operation settlement revokes the credential, closes the listener, and waits
  for endpoint shutdown before publishing the agent result. This is sufficient
  for the current immediate `self` handler. Long-running extension callbacks
  require explicit request tracking and cancellation before Roba can promise
  forced in-flight drain semantics.
- Binding an ephemeral IPv4 loopback listener is a deliberate turn-admission
  prerequisite. If the environment forbids it, the host returns a typed
  runtime refusal before provider work starts rather than silently removing
  the provider projection.
- Roba structurally omits launch URLs and credentials from run requests,
  snapshots, events, and MCP results. Launch-context Debug may show the
  loopback URL but redacts the credential. This does not stop an executing
  provider from repeating a credential it was intentionally given; provider
  output remains untrusted content.
- The private HTTP endpoint is provider launch plumbing, not the optional
  operator-facing HTTP binding parked in Phase 7. No paid real-provider
  dogfood was required for this mechanical checkpoint.

## Phase 5 checkpoint decisions

- `roba serve` owns one promptless `AgentInstance` and one foreground stdio
  control binding. It accepts the same suspended provider template as
  `roba run`, without a prompt, `--json`, or legacy profile/config layering.
  The provider session remains hot across turns; the provider process remains
  finite and is launched once per admitted operation.
- The binding uses Tower's compiled legacy and final MCP lifecycle support and
  concurrent request dispatch. It exposes the same control router as the
  in-process client, including optional Task-backed `agent.turn`; it does not
  advertise sampling, elicitation, or server notifications.
- Stdio EOF and binding shutdown begin `AgentInstance::shutdown` before Tower
  drains requests. Logical `agent.shutdown` propagates in the other direction
  to stop transport input, then Tower drains and flushes its response. Every
  transport return path awaits idempotent agent shutdown before surfacing a
  transport error or success.
- `StdioBinding::run` supplies Tower with a bounded async input bridge fed by one
  ordinary stdin reader thread. Tokio's own stdin uses an uncancellable
  blocking-pool read that can pin runtime shutdown when a client keeps its pipe
  open after `agent.shutdown`; the bridge detaches that OS read from the Tokio
  runtime while preserving backpressure, EOF, and read errors. Foreground
  `run` is single-use and process-scoped; an embedding that continues after
  the binding returns supplies its own controllable reader through
  `run_with_streams`.
- `agent.interrupt` drains only its named operation and leaves the hot server
  reusable. Provider failures are typed MCP results and do not decide the
  server process exit code. Agent-template limits and timeout apply to each
  finite operation rather than the idle host lifetime or an aggregate budget.
- Stdio owns both input and output from the first byte; the host prints no
  banner or success output. EOF, logical shutdown, and SIGTERM are graceful
  zero-exit requests. Startup/configuration and transport failures use the
  ordinary generic failure exit, while clap misuse remains exit 2.
- A piped stdio host consumes SIGINT without ending the agent so `mcp-repl`
  can use Ctrl-C to stop waiting on a local command while the child shares its
  foreground process group. A direct terminal host treats Ctrl-C as graceful
  shutdown. EOF and `agent.shutdown` are the cross-platform explicit stop
  paths for the piped server; SIGTERM is also graceful on Unix.
- `serve` is now a reserved subcommand spelling. The explicit legacy prompt
  remains available as `roba -p serve`, while an alias named `serve` is
  shadowed; the ambiguous bare `roba serve` form selects the hot host.
- `mcp-repl` remains an external client rather than a Roba dependency or a
  custom REPL implementation. Task state, events, and provider sessions are
  process-local and disappear when the foreground binding exits.

## Phase 6 checkpoint decisions

- `AgentExtension` carries independent control and provider `McpRouter`
  fragments. `AgentExtensions` is immutable for one `AgentInstance` and
  preflights exact capability collisions against both real base projections
  before provider work or a private listener can start. Fragment router
  identity, session state, task stores, auth, and middleware are not part of
  the composition contract.
- Provider-callable extension tools are an explicit trusted manifest.
  `ProviderMcpEndpoint` sorts and deduplicates the names, Claude receives exact
  `mcp__SERVER__TOOL` allow entries, and Codex receives exact always-quoted
  TOML tool approval keys. This makes dotted names such as `git.snapshot`
  unambiguous. Server and tool names are validated against MCP's bounded ASCII
  name grammar before configuration, preventing delimiter injection into a
  provider allowlist. The manifest does not grant authority to tools absent
  from the provider router.
- `--git` is host configuration shared by `roba run` and `roba serve`, not a
  serialized `RunSpec` field. Startup captures the nearest canonical Git
  repository containing the effective cwd. Calls cannot redirect the service
  to another path, and all projections share the same service state.
- `git.snapshot` and `roba://git/workspace` expose deterministic typed state to
  control and provider clients. Snapshot commands have finite timeouts and
  disable optional locks and configured filesystem monitors. Git state is
  read on demand; no prompt or durable context is injected, so resumed turns
  preserve their original context exactly once.
- `git.stage_all` is a writable-control workflow only. It stages current
  tracked, deleted, and untracked changes, refuses conflicts and no-op
  requests, and returns before/after snapshots plus the resulting index-tree
  object id. It is absent from every provider projection because host-side Git
  filters are broader authority than provider workspace-write alone.
- Exact provider and operator calls observe equal repository snapshots. The
  private provider credential still rotates per finite operation, expires
  before settlement, and cannot reach `agent.turn` or operator controls. Raw
  Git remains available when this narrow service is insufficient.

## Current phase ledger

| Phase | Status | Evidence |
|---|---|---|
| 0. Architecture record | Complete | Design review plus full common gate green, 2026-08-17 |
| 1. Minimal in-process agent | Complete | Two independent correctness reviews; 1 unit and 11 ChannelTransport integration tests; full common gate green, 2026-08-17 |
| 2. CLI over MCP | Complete | Two independent audits; 6 MCP unit plus 11 agent integration tests; 10 compatibility-projection unit and 188 CLI tests; full common gate green, 2026-08-17 |
| 3A. Controls and events | Complete | Independent concurrency review; 13 MCP unit, 11 base-agent integration, and 12 control/event ChannelTransport tests; 188 CLI compatibility tests; full common gate green, 2026-08-17 |
| 3B. MCP Tasks | Complete | Independent concurrency/API review; 13 MCP unit, 11 base-agent, 12 control/event, and 7 final-plus-legacy Task integration tests; 188 CLI compatibility tests; full common gate green, 2026-08-17 |
| 4. Provider self-client | Complete | Three independent blocker reviews; 88 core tests; 15 MCP unit, 11 base-agent, 12 control/event, 7 Task, and 3 authenticated-loopback provider integration tests; 188 CLI compatibility tests; full common gate green, 2026-08-17 |
| 5. Hot stdio | Complete | Independent lifecycle review; 15 MCP unit, 11 base-agent, 12 control/event, 7 Task, 3 provider-loopback, and 4 stable-plus-final stdio integration tests; 192 CLI tests including wire purity and real fake-provider child reaping; `mcp-repl` 0.3.0 final-protocol active fake-provider Task smoke; full common gate green, 2026-08-17 |
| 6. First extension | Complete | Independent security/correctness review; 93 core tests; 15 MCP unit, 11 base-agent, 12 control/event, 7 Task, 7 extension-composition, 3 provider-loopback, and 4 stdio integration tests; 11 `roba-git` fixture and authenticated-callback tests; 193 CLI tests including black-box `roba serve --git`; full common gate green, 2026-08-17 |
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
