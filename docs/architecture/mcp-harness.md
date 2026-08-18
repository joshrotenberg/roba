# MCP-native single-agent harness

> Status: current architecture on `main`.

## Decision

`roba-mcp` owns one hot logical agent above the finite `roba-core` run model.
One constructed `AgentInstance` has one fixed provider and policy template, one
retained provider session, one bounded event history, and at most one active
finite run.

The harness may stay idle until cancelled or shut down. Provider processes do
not stay hot: each admitted turn creates a fresh finite core run and resumes the
retained provider session when valid terminal evidence is available.

An `McpRouter` is a view of an agent, not the identity of an agent. Multiple
role-specific routers may capture the same `AgentInstance` without creating a
pool or multi-agent server.

## Base control contract

The control projection exposes:

- `agent.turn` -- admit one prompt, returning a typed terminal result;
- `agent.steer` -- queue guidance for one exact active operation;
- `agent.interrupt` -- cancel one exact operation and await settlement;
- `agent.shutdown` -- close admission permanently and drain active work;
- `roba://agent` -- redacted configuration and current agent state;
- `roba://events` and `roba://events{?after,limit}` -- bounded,
  cursor-addressed event history across finite runs.

Admission is single-flight. A concurrent turn receives a typed `busy` refusal;
the base contract does not hide a queue. Operation identifiers fence delayed
steering and interruption so stale controls cannot affect later work.

Provider and domain failures are successful MCP exchanges with typed
`structuredContent` and `isError: true`. JSON-RPC errors are reserved for
malformed protocol input, missing capabilities, authorization failures, and
server faults. Clients decode the structured value and never infer semantics
from display text.

## Tasks, events, and cancellation

`agent.turn` supports a synchronous fallback and optional MCP Tasks. A normal
provider failure still completes the Task with a typed error result. Cancelling
a Task targets the exact admitted operation, drains it, and only then settles
the Task as cancelled.

Direct non-Task request lifetime does not own operation lifetime. Dropping a
client waiter leaves admitted work supervised and observable until explicit
interrupt, shutdown, or natural settlement.

The agent event journal assigns a monotonic sequence across all finite runs,
records operation identity, and reports truncation explicitly. Terminal agent
state is never published ahead of the operation's final event records.

## Role-scoped provider projection

For each admitted operation the harness binds an ephemeral authenticated
loopback MCP endpoint for the provider process. Its base surface contains only
the read-only `self` tool. It structurally excludes turn admission, steering,
interrupt, shutdown, prior results, configuration, and operator event history.

Each operation receives a new listener and credential. The credential is
passed through provider-native launch configuration, is absent from serialized
run values, and is revoked before the operation settles. Provider adapters
approve only the exact advertised MCP tool names; there is no wildcard grant.

The provider-facing projection is an explicit allowlist, not the control router
with tools removed after composition. This prevents accidental recursive
authority and makes role differences discoverable.

## Extensions

`AgentExtension` contributes independently scoped control and provider router
fragments plus an exact provider-tool manifest. `AgentExtensions` preflights
both projections with fail-closed `try_merge` semantics before an agent starts.
Extensions cannot replace base tools, resources, templates, or prompts.

The first extension is `roba-git`. When enabled, both projections receive
bounded read-only Git snapshot access for one fixed workspace. Its mutating
`git.stage_all` operation is control-only and requires writable authority;
provider workspace-write does not imply authority to execute host-side Git
filters.

Extensions are MCP capabilities, not prompt injection. A resumed provider
session discovers and calls current state on demand without accumulating a
duplicate context block on every turn.

## Bindings

`ChannelTransport` is the in-process client path used by `roba run` and tests.
`StdioBinding` serves the same control contract concurrently for `roba serve`
and clients such as `mcp-repl`. The stdio host begins promptless and launches no
provider until a turn is admitted.

Logical shutdown, stdio EOF, and host signals are bridged in both directions:
the agent is drained before the binding returns, while an `agent.shutdown`
response is allowed to flush before transport exit. Provider failures remain
typed tool results and do not terminate the hot server.

General operator HTTP and Unix-socket bindings remain unimplemented. The
private provider loopback endpoint is launch plumbing, not a public network
service.

## Boundaries and next seams

The base harness does not contain a durable queue, scheduler, task store,
session pool, agent registry, or multi-agent routing. Higher layers may call
one or more Roba agents through the same MCP contract, but federation policy
does not belong in this base instance.

Explicit context management is the next architectural seam. The intended
direction is MCP-native, inspectable context availability with a minimal launch
bootstrap, source and precedence metadata, freshness rules, and evidence of
what an agent read. It must distinguish Roba-controlled context from ambient
provider instructions instead of pretending the latter do not exist. This work
is tracked in GitHub issue #489; the adopted foundation and current inventory
are recorded in [`context.md`](context.md).

Scheduling, GitHub workflows, richer Git mutations, context rotation, and
Roba-to-Roba coordination should arrive as optional MCP services or external
clients only after their authority and lifecycle contracts are explicit.
