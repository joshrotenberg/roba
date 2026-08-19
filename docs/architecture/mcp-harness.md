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
- `agent.follow_up` -- queue another provider turn for one exact active operation;
- `agent.interrupt` -- cancel one exact operation and await settlement;
- `agent.shutdown` -- close admission permanently and drain active work;
- `roba://agent` -- redacted configuration and current agent state;
- `roba://events` and `roba://events{?after,limit}` -- bounded,
  cursor-addressed event history across finite runs.
- `roba://context` and `roba://context/entry{?id,generation}` -- a
  content-free context manifest, provider read evidence, and explicit
  generation-fenced content reads.

Admission is single-flight. A concurrent turn receives a typed `busy` refusal;
the base contract does not hide a turn-admission queue. Operation identifiers
fence delayed follow-up and interruption so stale controls cannot affect later
work. Follow-up prompts are a separate bounded FIFO owned by the active
operation and are applied only at provider-turn boundaries.

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

Provider adapters normalize mechanically observed command, file-change, MCP,
web-search, plan, status, and unknown activities into start/completion events.
The harness bounds identifiers and summaries before they enter the journal.
`roba://agent` derives active activity, last provider event/activity times,
elapsed and timeout-remaining duration, and explicit unknown/healthy/degraded/
terminal observation health from that same stream. It never estimates a
percentage or treats provider silence as progress.

Task-backed calls receive activity and warning records as best-effort MCP log
notifications before settlement. The replay journal remains authoritative
when a client does not negotiate logging, misses a notification, or falls
behind.

## Role-scoped provider projection

For each admitted operation the harness binds an ephemeral authenticated
loopback MCP endpoint for the provider process. Its base surface contains the
read-only `self`, `context.manifest`, and `context.read` tools plus role-scoped
context resources. The tools and resources expose the same generation-fenced
contract; tools are the portable provider path when native MCP resource access
is unavailable. The projection structurally excludes turn admission, follow-up,
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
fragments, an exact provider-tool manifest, and optional typed context entries.
`AgentExtensions` preflights both MCP projections with fail-closed `try_merge`
semantics, then compiles context entries into the existing immutable
`ContextPlan` before an agent starts. Extensions cannot replace base tools,
resources, templates, prompts, or context IDs. Retained extension context is
available through the role-scoped context plane and is not appended to
`RunSpec` or provider prompts.

An extension may also attach one operation lifecycle observer. The host calls
it at admission before provider work, after start, on a serialized periodic
tick, during settlement after the provider endpoint is drained, after final
state capture, and during host shutdown. Every callback is exact-operation
scoped, runs outside the agent control lock, has a host-enforced timeout, and
is drained before terminal settlement becomes visible. Panics, timeouts, and
typed hook failures produce compact extension failure events rather than
wedging the agent. Extension changes publish only bounded fingerprint and
summary evidence; full state remains in extension-owned MCP resources.

The first extension is `roba-git`. When enabled, both projections receive
bounded read-only Git snapshot access for one fixed workspace plus cached
`roba://git/progress`. The cache records an operation baseline, current state,
commits, diff/path summaries, timestamps, fingerprint, and sampler health.
Sampling occurs only while an operation is active and a final synchronous
refresh precedes settlement. Its mutating `git.stage_all` operation is
control-only and requires writable authority; provider workspace-write does
not imply authority to execute host-side Git filters.

Extensions are contributions, not prompt injection. A resumed provider
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

Explicit context management is an active architectural seam. The current MCP
surface publishes the typed manifest, capability-checked ambient-provider
policy and source matrix, and operation-scoped read evidence while keeping
bodies behind explicit generation-fenced reads. Acknowledgement and
capability gating remain incremental work. The adopted contract and current
inventory are recorded in
[`context.md`](context.md).

Scheduling, GitHub workflows, richer Git mutations, context rotation, and
Roba-to-Roba coordination should arrive as optional MCP services or external
clients only after their authority and lifecycle contracts are explicit.
