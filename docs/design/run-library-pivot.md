> Status: ACTIVE cleanup and hardening. The provider-neutral root-run core,
> Claude and Codex adapters, and blocking `roba run` path remain. The worker
> tree, mission/process layers, GitHub pack, and shipped MCP/REPL adapters are
> parked or removed. This document is the architecture decision and resume
> point for the next v0.12 work.

# Roba as a finite, provider-neutral root run

## Decision

Roba's library boundary is one finite, single-root agent run. One process owns
that run and exits when the run is complete, failed, or cancelled. The host
operating system remains the pool when several independent runs are needed.

Roba stays library-first. `roba-core` owns provider-neutral intent, execution,
lifecycle, outcomes, failures, and events. The `roba run` command is a thin
blocking adapter. A Rust host that needs live control retains the same public
`RunHandle` used by the runtime.

This is deliberately smaller than the earlier finite-mission experiment. Roba
does not currently own child workers, repository workflows, reported work-item
projections, or process capabilities. Those concerns can live in a separate
workflow layer until real use proves that the core must absorb any of them.

The established Claude one-shot command remains a compatibility product. It
keeps its prompt composition, profiles, aliases, personas, bundles, session
helpers, receipts, inspection commands, output split, JSON envelope, and typed
exit codes while the new path stabilizes.

## Current contract

### Specification and runtime

The retained public values are:

```text
RunSpec
  agent: AgentSpec
  context: ContextSpec
  execution: ExecutionSpec
  initial_prompt: Option<Prompt>

Roba
  register(provider)
  create_run(spec) -> Run

RunHandle
  spec()
  start(prompt) / begin()
  status()
  subscribe() / subscribe_after(sequence)
  event_page(sequence, limit)
  wait_for_events(sequence, limit)
  steer(message)
  cancel()
  wait()
```

`AgentSpec` selects a provider and optional model, effort, and ordered
instructions. `ContextSpec` separates project context from run-specific
context. `ExecutionSpec` carries permissions, tool policy, portable limits,
and a fresh or resumed provider session.

`Roba` is only a process-local provider registry. Hosts register exactly the
adapters they permit; duplicate and unavailable providers fail closed. It is
not a global singleton, daemon, scheduler, queue, or session database.

Creating a promptless run leaves it suspended and starts no provider process.
The library caller can later supply the first prompt through `RunHandle::start`.
A spec with an initial prompt is ready and begins through `Run::begin` or
`RunHandle::begin`. The blocking CLI always requires its initial prompt.

### Lifecycle and steering

The coarse state machine is:

```text
suspended | ready -> running -> completed | failed | cancelled
                                  |
                                  -> finishing -> cancelled
```

The exact intermediate path depends on whether another steering message is
queued. Steering is delivery at the next safe provider-turn boundary. It is
available only when the selected provider supports session resume. The API
does not claim portable mid-token injection.

Cancellation is process-local and terminal. For an active run, the lifecycle
enters `finishing`, drops the provider future, and only then publishes
`cancelled`. A suspended or ready run can be cancelled without launching a
provider.

### Observation and failure evidence

`RunHandle::status` returns one in-memory `RunSnapshot` with lifecycle timing,
completed-turn count, the latest outcome, and any terminal failure.

Normalized events are sequenced per run and stored in a bounded journal.
Callers may consume a replaying subscription, page by cursor, or wait for a
page. The journal reports when requested history has been truncated and
refuses cursors ahead of current state. It never presents a partial event
history as complete.

Provider failures use portable categories. When a provider reports terminal
recovery or accounting evidence, `RunFailureDetails` retains the session id,
usage, cost, duration, and provider turn count. Missing values stay absent.
The failure is visible in both the terminal snapshot and a normalized failed
event.

## Provider boundary

The provider contract is turn-based. An adapter must validate the complete
`TurnRequest` before spawning its child process, declare its supported
capabilities, emit normalized events, and return a provider-neutral outcome or
failure. A portable option that an adapter cannot enforce must be refused, not
silently ignored.

The built-in adapters are:

- Claude Code through `claude-wrapper`, including fresh/resumed execution,
  streaming, portable permissions, limits, timeout, and provider-reported
  accounting.
- Codex through `codex-wrapper`, including fresh/resumed execution, streaming,
  portable sandbox/approval posture, timeout, and supported effort settings.
  Codex refuses cost and turn caps it cannot authoritatively enforce. Roba
  preserves Codex's Git-repository check and does not request
  `danger-full-access`. Writable and full-auto Roba intent both map to
  non-interactive workspace-write plus `approval_policy=never`; attempts to
  leave the sandbox fail because no approval channel exists.

Wrapper-native result types do not cross the public `roba-core` boundary.

## CLI boundary

`roba run` accepts explicit flags and constructs one `RunSpec`. It selects
Claude by default, registers the built-in Claude and Codex adapters, begins the
run, waits for its terminal snapshot, and prints the answer or versioned JSON.

There is no separate hierarchical run-config format. In particular, the
removed `--config`, `--agent`, `--max-workers`, `--max-worker-depth`, `--repl`,
and `--mcp` flags are not part of `roba run`.

The legacy root command remains Claude-only and keeps the discovered
`roba.toml` pool. Its profile, alias, persona, and bundle configuration does
not leak into provider-neutral `RunSpec` resolution. The legacy
`--mcp-config` option remains a pass-through to Claude and is not a Roba
control server.

## Cleanup decisions

| Area | Current decision | Reason |
|---|---|---|
| Root `RunSpec`, lifecycle, events, `RunHandle` | Keep and harden | This is the reusable provider-neutral seam and the minimum useful live-control contract. |
| Claude and Codex adapters | Keep and harden | Codex support already exists through `codex-wrapper`; both adapters exercise the provider boundary. |
| Legacy Claude one-shot CLI | Keep compatible | It is the released, useful product surface and supplies the established scripting ABI. |
| Roba-owned worker tree | Park and remove from current API | It multiplied lifecycle, policy, cancellation, and transport complexity before a proven core need. |
| Mission projection and claimed work items | Park and remove | A workflow monitor can own this projection without changing root-run mechanics. |
| Process capability registry | Park and remove | Repository and board authority is workflow policy, not a prerequisite for executing one run. |
| GitHub workflow/process pack | Park outside Roba | GitHub-specific issue, branch, PR, review, and merge policy does not belong in the provider-neutral core. |
| `roba-mcp` and `roba-repl` crates | Remove from the shipped workspace | They were adapters over an unstable, overgrown surface. The underlying `RunHandle` seam remains. |
| Always-running server or session pool | Keep out | Finite process ownership is a useful constraint and makes completion and cancellation honest. |

"Parked" means prior work can be consulted later. It does not mean the types,
flags, crates, or behavior are current or compatibility promises.

## Possible run-scoped MCP control adapter

MCP remains worth revisiting for a genuinely long run. The design should stay
narrow:

- One adapter instance is bound to one live `RunHandle` and ends with that run.
- Tools mirror existing library operations such as `start`, `status`,
  `events`, `steer`, `cancel`, and `wait`.
- It adds no worker spawning, process capability, configuration, history,
  repository administration, or provider-private state access.
- Cursor and truncation semantics come directly from `event_page` and
  `wait_for_events`.
- Authentication and transport are adapter concerns and must not be persisted
  in `RunSpec` or snapshots.

`mcp-repl` already provides an interactive interface to an MCP server. If this
adapter returns, it can make a running Roba observable and steerable without a
custom `roba-repl` crate. No current CLI flag or workspace crate exposes this
surface.

## Next seams, not current claims

- **Per-run cwd:** capture and apply a working directory as part of one run
  rather than relying only on process-global cwd behavior.
- **Provider-neutral structured output:** add JSON Schema intent and normalized
  structured output only when both provider validation and result semantics
  can be represented honestly.
- **Context isolation:** define what project/run context is merely composed
  into a prompt and what, if anything, is isolated from ambient provider
  configuration. The current string lists are composition, not a sandbox.
- **Resume prompt stdin:** fresh Codex prompts are stdin-safe today, but
  `codex-wrapper` 0.3.1 places resumed prompts in argv. Move resume delivery to
  stdin when the wrapper supports it so resumed prompt text is not exposed in
  the child process arguments.

## Workflow-layer prior art

The separate Steward/`ok-v` prototype is workflow-layer prior art. Its visible
queue, bounded tick, single-writer lock,
session handoff, receipts, and doctor checks are useful ideas for an external
driver. They do not change Roba's root-run plan, and Steward is not imported as
a Roba subsystem.

The older deleted `roba-server` implementation also contains useful ideas such
as one actor, FIFO turns, `prompt`/`status`, inward context, and an operator
bridge. Its warm Claude-only backend and persistent-server positioning remain
out of scope.

## Explicit non-goals

- No always-running Roba daemon.
- No multi-run session pool, scheduler, webhook, board, or queue in the core.
- No hidden provider or child work after the owning run becomes terminal.
- No provider-common option that one adapter silently ignores.
- No repository, GitHub, or merge policy in `roba-core`.
- No Roba-owned worker tree without a new, evidence-backed design decision.
- No custom REPL while an MCP adapter plus `mcp-repl` can fill that role.
- No mutation of provider-private session state.

## Near-term work

The cleanup completed Codex error/resume/cancellation hardening, restored the
legacy persona surface, made lifecycle events authoritative, and added
fail-loud serialization for removed policy fields. Remaining work is:

1. Advance the next seams above without claiming unsupported portability or
   isolation.
2. Dogfood direct Rust and `roba run` paths with one root run at a time.
3. Revisit a run-scoped MCP adapter only after the core handle has proven
   stable and a long-running use case needs external observation or steering.

## Resume checklist

1. Read this document and `AGENTS.md`.
2. Inspect `git status` and work on a focused feature branch.
3. Confirm a proposed feature belongs to one finite root run. Put workflow
   policy in an external layer by default.
4. Preserve the legacy CLI unless an explicit compatibility decision says
   otherwise.
5. Run the repository's four required gates before publishing a review point.
6. Update this status note when the current architecture or parked boundary
   changes.
