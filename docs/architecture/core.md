# Finite provider-neutral core

> Status: current architecture on `main`.

## Decision

`roba-core` owns one finite, single-root agent run. A run begins suspended,
may execute one or more provider turns when steered, and settles as completed,
failed, or cancelled. The host owns the lifetime of the `RunHandle`; the
operating system remains the pool when several independent agents are needed.

This boundary is intentionally smaller than the earlier mission and worker
experiments. Roba core does not own child-agent trees, workflow queues,
repository processes, scheduling, or multi-agent routing.

## Contract

`RunSpec` describes provider-neutral intent:

- `AgentSpec` selects a provider, model, effort, and provider session;
- `ContextSpec` carries explicit instructions and context intent;
- `ExecutionSpec` carries permissions, tool policy, portable limits, and
  timeout;
- an optional initial `Prompt` determines whether the run is ready or
  suspended.

`Roba` is a provider registry and run factory. `RunHandle` supplies the live
control surface:

- `start` performs the one-way suspended-to-running transition;
- `status` returns the authoritative snapshot;
- `event_page`, `wait_for_events`, and `subscribe` expose bounded replay;
- `steer` queues a provider-native resumed turn at a safe turn boundary;
- `cancel` requests process-group cancellation;
- `wait` resolves only after terminal settlement.

The lifecycle synthesizes state and turn-boundary events. Providers may emit
only provider-owned output, usage, and warning events, so an adapter cannot
forge authoritative lifecycle history. Panics and abnormal driver exits settle
as typed failures rather than leaving a run permanently active.

## Provider boundary

Claude Code and Codex are built-in adapters. Both normalize native output into
provider-neutral outcomes, usage, sessions, failures, and events. Unsupported
portable controls fail honestly instead of being approximated.

Transient launch material is separate from serializable run intent.
`ProviderLaunchContext` may carry operation-scoped MCP endpoints and exact tool
names, but endpoint credentials never appear in `RunSpec`, snapshots, or
events. One launch context remains stable across resumed provider turns
inside the same finite run.

The core does not depend on Tower MCP or any transport. It knows only the
provider endpoint values required to launch a provider process.

## Above and beside the core

`roba run` is a blocking adapter that resolves explicit provider-neutral flags,
creates one run, and projects the terminal snapshot back onto its established
stdout, JSON, stderr, and exit-code behavior.

`roba-mcp` is a separate application layer. It retains provider session
continuity across several finite core runs and supplies hot-agent lifetime,
MCP schemas, Tasks, bindings, and extensions. Those concerns are not core run
semantics.

## Boundaries

The following require a new evidence-backed decision before entering core:

- Roba-owned worker or agent trees;
- a mission, process-capability, or GitHub workflow abstraction;
- a durable queue, scheduler, daemon, or session pool;
- transport or Tower MCP types;
- mutation of provider-private state;
- controls a provider cannot enforce authoritatively.

Provider-neutral structured output, explicit working directory and launch
environment policy, stronger context isolation, and safer resumed-prompt
delivery remain valid narrow seams when supported honestly by the providers.
