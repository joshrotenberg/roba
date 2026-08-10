> Status: ACTIVE mission hardening. The original five implementation phases
> are complete: provider-neutral execution, Claude and Codex, hierarchical
> config, process-local lifecycle and workers, and thin CLI/REPL/MCP adapters.
> Current work makes the finite-mission model explicit and gives monitors one
> canonical runtime projection. Legacy CLI cleanup and broader real-world
> dogfood remain before a release decision. This document is the resume point
> if the work moves elsewhere.

# Roba as a bounded, provider-neutral agent run

## Decision

Roba is the agent. A Roba run is one intentioned, bounded invocation of that
agent. The run may create worker runs, can be observed and steered while it is
alive, and exits when the intention is complete.

Roba stays library-first. The CLI, REPL, and run-scoped MCP server are adapters
over the same public run API. There is no required daemon and no persistent
session pool. A process exists for the lifetime of one run; the OS remains the
pool when several runs are needed.

This replaces the old positioning of Roba as only sugar around `claude -p`.
The useful constraint from that era remains: keep the execution model small,
make durable outputs explicit, and do not hide workflow policy in a resident
service.

## Product shape

### One finite mission

"Run" remains the execution substrate, but the product concept is a finite
mission. The same machinery should handle both `2+2` and "work five issues in
this backlog": the latter adds explicit worker bounds, authority grants,
process capabilities, and a completion policy rather than switching to a
resident orchestration service.

Roba mechanically owns lifecycle, timing, worker state, usage, limits, and
terminal settlement. Agents may publish typed work items, blockers, and
artifacts for monitors, but those remain visibly claim-backed and cannot
overwrite host-derived facts. Rust, CLI JSON, REPL, and MCP consume the same
`MissionSnapshot`; they do not maintain parallel dashboards.

Repository issue work is an optional process capability. The library now owns
stable capability, action, and grant identifiers; an immutable root
`MissionPolicy`; and a host-owned registry that freezes descriptors and
refuses unknown capabilities or missing grants before provider work. A private
run-bound MCP dispatcher exposes only declared actions. The first concrete
pack, `roba-process-github`, supplies repository-scoped issue/PR reads,
idempotent PR creation, and exact-head merging with separate read, PR-write,
and merge grants. Actions whose grants are absent are omitted from the private
surface. It is intentionally sequential in the current checkout: parallel
issue writes remain deferred until the host can issue worktree leases and
assign trusted worker working directories. A host such as Ciacola may later
consume Roba as its mechanical mission harness without making persistence a
Roba requirement.

The public library converges on these concepts:

```text
RunSpec
  agent: AgentSpec
  context: ContextSpec
  execution: ExecutionSpec
  mission: MissionPolicy
  initial_prompt: Option<Prompt>

Run
  start(prompt)
  steer(message)
  status()
  subscribe()
  cancel()
  wait()
```

Creating a run without an initial prompt leaves it suspended. The first prompt
may arrive from a library caller, the CLI, the REPL, or the run-scoped MCP
surface. Roba does not spawn a provider child while suspended.

The minimum lifecycle is:

```text
suspended -> running <-> waiting -> completed | failed | cancelled
```

For the first provider-neutral version, steering means delivery at the next
safe turn boundary. A caller may cancel the current provider turn and resume
with the steering message when immediate intervention is required. The API
must not imply portable mid-token injection that only one provider supports.

## Configuration model

The current surface mirrors almost every option across clap, `ROBA_*`,
top-level TOML, and profiles. That creates dozens of flat fields and multiple
large merge implementations. The replacement separates persistent run policy
from invocation presentation.

Execution settings resolve in one documented order:

```text
Roba defaults -> provider defaults -> named agent -> run overrides
```

Instructions compose in a different, fixed order:

```text
Roba system contract
  -> agent instructions
  -> project context
  -> run context
  -> initial user prompt
  -> steering messages
```

Persistent configuration should cover only stable execution policy:

- provider, model, and effort
- agent instructions
- context sources
- tool and permission policy
- limits

Invocation-only concerns stay out of persistent profiles:

- terminal rendering and JSON presentation
- trace destination
- detach/attach behavior
- whether to expose a REPL or MCP endpoint
- one-off prompt composition sugar

Named `AgentSpec` values replace profiles and the redundant persona label in
the public run model. Legacy profiles remain readable only for one-shot
compatibility, and bundles remain Claude-only context packaging. Aliases are
CLI sugar and do not shape the public run model.

## Provider boundary

`roba-core` currently exposes Claude types directly. Replace that with a small
provider contract around provider-neutral `RunSpec`, events, outcomes, session
handles, and typed failure categories.

The provider contract is turn-based rather than warm-process-based:

- Claude supports fresh and resumed turns, streaming, and additionally a warm
  duplex session.
- Codex supports fresh and resumed turns plus streaming, but not the same warm
  duplex primitive.
- The Roba run process supplies continuity; provider adapters may choose the
  best compatible child-process mechanism without changing run semantics.

The existing `codex-wrapper::QueryResult` intentionally mirrors the Claude
result shape, and both wrappers expose session helpers. Use those similarities
inside adapters, but do not leak either wrapper's public types through
`roba-core`.

## Run-scoped MCP and REPL

MCP and the REPL are clients of the same `RunHandle`. They must not gain
separate execution logic.

Initial external MCP surface:

- `start` -- supply the first prompt to a suspended run
- `status`
- `wait`
- `events` -- bounded cursor replay and long polling across the run tree
- `workers`
- `usage`
- `steer`
- `cancel`

The surface exists only for the run lifetime. It exposes run observability and
control, not arbitrary process, configuration, history, or host administration.

The REPL can create a suspended run, inspect its resolved specification, start
it, observe it, steer it, and wait for its terminal outcome. It calls the same
library methods as MCP.

## Child-run ownership

The process-local tree has one root identity and monotonically assigned child
identities. Its root specification captures `max_workers` and
`max_worker_depth`; zero/zero disables workers, and specifying only one bound
is invalid. The total limit counts terminal workers as well as live workers, so
sequential spawning cannot evade it.

A trusted library caller may select a registered worker agent/provider, but a
worker request carries no execution policy. Roba copies permissions, tools,
provider limits, and worker bounds from the parent and forces a fresh provider
session. Those provider limits apply independently to each child, so worker
bounds limit the tree but do not currently impose an aggregate spend ceiling.
The public MCP and REPL spawn commands are narrower still: they clone the
parent agent and context and accept only a prompt.

Parent completion, failure, or cancellation closes its spawn boundary and
cancels live descendants before publishing the parent's terminal state.
Terminal child snapshots remain queryable until the owning root tree is
dropped. Provider-facing self-spawn transport is deliberately not implied by
this layer; it requires an internal run-scoped MCP/broker configuration in a
later slice.

### Provider-facing worker transport

The next transport binds a narrow worker-control capability to the exact run
whose provider turn is executing. The capability can spawn an inherited child
and list that run's descendants; it cannot steer, cancel, replace execution
policy, or select a broader identity. It is carried beside the transient
provider request rather than serialized into `RunSpec`.

When workers are enabled, an opt-in provider middleware exposes that capability
through a short-lived MCP server on an ephemeral loopback listener. Every
server gets an unpredictable bearer credential, starts before the provider
process, and shuts down when that provider turn is dropped or completes.
Claude receives the endpoint through a temporary MCP configuration and Codex
through equivalent per-command configuration on both open and resume. The
credential is never part of a run snapshot or durable configuration.
Each adapter authorizes only the two private worker tools: Claude receives
exact allowed-tool patterns, while Codex receives an exact per-tool approval
for `spawn_worker` so non-interactive approval policy can remain fail-closed
for every other write-capable MCP tool.

The transport remains process-local and is not an operating-system isolation
claim. Its authority comes from the host-created capability and immutable run
tree policy, not from a caller-supplied run id or parent field.

Provider-native subagent facilities are disabled on the bounded-run path.
Claude's Agent tool and Codex's multi-agent feature would otherwise create
work outside Roba's worker count, depth, cancellation, and observation model.
Provider instructions name the exact private worker tools, require waiting for
owned workers, and forbid shell-launched Roba/provider processes as substitutes
when a spawn is refused.

## Implementation sequence

### Phase 1 -- provider-neutral contracts and Claude compatibility

- [x] Add provider-neutral `RunSpec`, `RunOutcome`, `RunEvent`, session handle,
      failure, usage, model, effort, permissions, and limit types to
      `roba-core`.
- [x] Add an object-safe `Provider` trait that executes one fresh or resumed
      turn and emits normalized boundary events. Output-delta streaming moves
      behind it with the CLI migration.
- [x] Move `engine::run`'s Claude client construction behind `ClaudeProvider`.
      The existing CLI's direct streaming seam remains a compatibility path.
- [x] Stream Claude fresh and resumed output plus usage through the normalized
      event sink, with fake open/resume/cancel/missing-terminal coverage and a
      paid MCP-observed smoke outside the normal gate.
- [x] Keep `engine::Config` and `engine::run` as a compatibility adapter so the
      current CLI and tests do not change behavior in this phase.
- [x] Add contract and adapter tests using fake provider boundaries; no paid
      tests in the normal gate.

Acceptance: the existing CLI test matrix is unchanged, `roba-core` no longer
exposes Claude result types from its new run API, and the Claude adapter remains
the only production provider without being the type system's default.

### Phase 2 -- Codex provider

- [x] Add `codex-wrapper` and `CodexProvider`.
- [x] Map fresh and resume, session/thread identity, supported limits,
      sandbox/approval posture, usage, and typed failures honestly.
- [x] Detect unsupported provider-specific settings before spawning a child.
- [x] Move Codex output streaming behind the provider-neutral event sink.
- [x] Add fake-binary open/resume/stream/cancel tests and one explicit paid
      smoke outside the normal gate.

Acceptance: the same public `RunSpec` starts and resumes both providers, and no
provider silently ignores a requested safety or limit setting.

### Phase 3 -- resolved hierarchical specification

- [x] Move prompt/context and configuration resolution into a library crate.
- [x] Introduce named `AgentSpec` values and the fixed instruction stack.
- [x] Make the resolved specification serializable and inspectable.
- [x] Snapshot the resolved specification before the first provider call.
- [x] Adapt `roba run` to the hierarchy; keep its flags as final run overrides.
- [x] Load the new hierarchy from a small, explicit public TOML format.
- [x] Route the overlapping policy of the legacy one-shot compatibility path
      through the same hierarchy before applying Claude-only controls.
- [x] Deprecate legacy profiles, remove the redundant persona inspection
      concept, and keep aliases/bundles confined to one-shot CLI sugar.

Acceptance: a caller can build, resolve, inspect, serialize, and execute a run
without clap or terminal code. Configuration precedence has one implementation.

### Phase 4 -- bounded run lifecycle

- [x] Add `Run`, `RunHandle`, state transitions, event subscription, and
      cancellation.
- [x] Support prompt-less suspended creation and exactly one initial start.
- [x] Define boundary-safe steering and provider resume behavior.
- [x] Promote useful receipt-like lifecycle timing into snapshots and
      replayable events without introducing persistence or a database.
- [x] Add child-run ownership and bounded worker-tree observability for Rust,
      external MCP, and REPL callers.
- [x] Give the root provider an internal, policy-bound worker-spawn transport.

Acceptance: a process-local run can be suspended, started, observed, steered,
cancelled, and awaited entirely through the library.

### Phase 5 -- adapters

- [x] Add the initial run-scoped MCP control adapter.
- [x] Add the initial REPL over the same `RunHandle`.
- [x] Make `roba run` the thin CLI constructor/attachment path.
- [x] Preserve pipe-clean one-off usage as a convenience over the same API.
- [x] Add worker spawn and observation to MCP and the REPL.
- [x] Add bounded cursor replay and long-poll event observation to MCP.
- [x] Add immutable process-capability declarations, explicit grants, and a
      private run-bound action dispatcher.

Acceptance: CLI, REPL, MCP, and direct Rust callers produce identical lifecycle
and outcome semantics.

### Phase 6 -- first repository process pack

- [x] Add action-level grants so one declared capability can expose reads,
      PR writes, and merges independently.
- [x] Add an optional library-first GitHub process pack with typed issue and
      PR actions, exact repository scoping, idempotent PR reconciliation, and
      exact reviewed-head merge fencing.
- [x] Add the smallest explicit `roba run` opt-in and deterministic fake-`gh`
      coverage without widening the public monitoring MCP surface.
- [ ] Dogfood sequential multi-issue missions across several repositories.
- [ ] Design host-issued workspace leases before allowing parallel workers to
      mutate repository worktrees.

Acceptance: a run can receive repository process knowledge and only the exact
grants selected by its host; no process action appears on the public monitor
surface, and retries do not duplicate pull requests or merge a moved head.

## Explicit non-goals

- No always-running Roba daemon.
- No multi-run session pool inside Roba.
- No schedule, webhook, queue, repository, or board subsystem in the core.
- No provider-common option that is silently ignored by one adapter.
- No requirement for MCP or a REPL when a caller wants one blocking run.
- No restoration of the deleted `roba-server` crate as-is.

## Useful prior art in this repository

The deleted `roba-server` implementation at commit `2c75429^` contains useful
bounded-process ideas: one actor, FIFO turns, `prompt`/`status`, inward
`context`, and an operator bridge. Reuse concepts selectively. Its warm
Claude-only backend, environment-only configuration, and persistent-server
positioning are not the new architecture.

Current assets to preserve:

- `roba-core` as the clap-free execution home
- `roba-types` receipts and stable machine envelope
- detached-run receipts plus `show --wait`, `jobs`, and `watch` as proven
  observability primitives
- the current CLI's prompt composition and pipe-clean output behavior

## Resume checklist

1. Read this document and `AGENTS.md`.
2. Start from clean `main`, inspect `git status`, and create a focused branch.
3. All planned pivot phases are complete. Choose the next work from direct
   library/CLI dogfood, API stabilization, or a new explicit issue; do not add
   always-running server responsibilities to Roba by default.
4. Preserve current CLI behavior until a phase's acceptance criteria say a
   compatibility surface may change.
5. Run the repository's four required gates before publishing a review point.
6. Update the checkboxes and the status note here whenever a phase lands.
