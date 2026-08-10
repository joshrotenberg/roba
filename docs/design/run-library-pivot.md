> Status: ACTIVE vertical slice, started 2026-08-09 on
> `codex/run-library-pivot`. Provider-neutral execution, both providers, the
> process-local lifecycle, and thin CLI/REPL/MCP adapters are implemented.
> Codex streaming, bounded child runs, event observation, paid Codex smoke,
> and the explicit hierarchical run-config path are implemented. Legacy CLI
> config cleanup and Claude streaming remain. This document is the resume
> point if the work moves elsewhere.

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

The public library converges on these concepts:

```text
RunSpec
  agent: AgentSpec
  context: ContextSpec
  execution: ExecutionSpec
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

Profiles, personas, and bundles should converge on one named `AgentSpec`
concept. Aliases are CLI sugar and must not shape the public run model. Do not
delete legacy configuration until the replacement can run the existing CLI
through a compatibility adapter.

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
- [ ] Remove or deprecate redundant legacy one-shot config fields and concepts
      only after its compatibility adapter consumes the hierarchy.

Acceptance: a caller can build, resolve, inspect, serialize, and execute a run
without clap or terminal code. Configuration precedence has one implementation.

### Phase 4 -- bounded run lifecycle

- [x] Add `Run`, `RunHandle`, state transitions, event subscription, and
      cancellation.
- [x] Support prompt-less suspended creation and exactly one initial start.
- [x] Define boundary-safe steering and provider resume behavior.
- [ ] Promote receipts from detached-CLI artifacts into run outcomes/events
      where useful, without introducing a database requirement.
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

Acceptance: CLI, REPL, MCP, and direct Rust callers produce identical lifecycle
and outcome semantics.

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
3. Continue the first unchecked item that advances the current vertical slice.
   The recommended next provider slice is Claude event streaming, followed by
   adapting the legacy one-shot compatibility path onto the hierarchy without
   importing its aliases, profiles, or presentation options into `roba-core`.
4. Preserve current CLI behavior until a phase's acceptance criteria say a
   compatibility surface may change.
5. Run the repository's four required gates before publishing a review point.
6. Update the checkboxes and the status note here whenever a phase lands.
