# roba-mcp

`roba-mcp` hosts one hot logical Roba agent behind a typed Model Context
Protocol contract. Each `agent.turn` call creates one finite `roba-core` run;
the host retains the provider session between calls while remaining idle
between provider processes.

The operator contract has process-local and foreground stdio bindings:

- `AgentInstance` owns a suspended `RunSpec`, one optional provider session,
  and at most one active `RunHandle`.
- `agent.turn { "text": ... }` waits for one finite run. Success returns the
  assistant answer as text plus typed `structuredContent`; provider failures,
  cancellation, blank prompts, `busy`, and `stopped` are typed tool results
  with `isError: true`.
- `agent.steer` queues guidance for one named active operation.
  `agent.interrupt` cancels that exact operation and waits for settlement.
  Both require an operation id so a delayed control cannot affect a later
  turn.
- `agent.shutdown` permanently closes admission and drains active work before
  reporting that the logical agent is stopped.
- Result variants encode their terminal invariants: a completed result always
  has an outcome and a failed result always has a failure.
- `roba://agent` dynamically reports configured policy,
  idle/running/stopping/stopped state, session availability, current operation
  identity, and the latest terminal result. Session identifiers are redacted
  from this resource; the originating `agent.turn` result retains valid
  session evidence.
- `roba://events` and `roba://events{?after,limit}` expose bounded,
  agent-wide replay. Global sequences continue across finite runs, source-run
  sequences remain visible, lost history is explicit, and event projections
  redact provider session ids.
- `connect_in_process` returns an initialized production `McpClient` over
  Tower MCP's concurrent `ChannelTransport`.
- `call_turn` is the typed client seam. It requires valid
  `structuredContent`, checks that MCP `isError` agrees with the typed status,
  and never treats display text as machine data.
- `StdioBinding` serves the same control router over stdin/stdout with legacy
  and final MCP lifecycle support. Requests dispatch concurrently, so status,
  event reads, steering, and interruption can overtake a long synchronous
  turn.

The same `AgentInstance` also has a separate provider-facing projection. For
each admitted finite operation, the host binds that projection to an ephemeral
IPv4 loopback port, mints a high-entropy bearer credential, and passes the
endpoint to `roba-core` as non-serializable launch context. Claude receives an
owner-private temporary MCP configuration; Codex receives wrapper-native
configuration plus the bearer through a child environment variable. The
credential rotates for every finite run and is revoked when that run settles.

The provider router is an explicit allowlist, not the control router with
runtime denials. Its base publishes one immediate, read-only tool named
`self` (qualified as `roba.self` by provider clients). Installed extensions
may contribute separate provider tools or resources, but nothing from their
control fragment is mirrored automatically. The base publishes no turns,
steering, interruption, shutdown, Tasks, configuration, event history, or
retained session evidence. This proves provider re-entry without opening
recursion or operator authority.

## Extension composition

`AgentExtension` contains named control and provider `McpRouter` fragments
plus an explicit manifest of provider-callable tools. `AgentExtensions` is an
immutable aggregate installed through `AgentInstance::new_with_extensions`.
Construction preflights each projection against Roba's real base router and
fails closed on exact tool, resource, resource-template, or prompt conflicts.
Actual control and per-operation provider routers repeat that validated merge;
last-writer-wins replacement is never used.

Fragments are capability bags. Their router identity, session state, task
store, auth, and middleware are not imported when Tower merges them into a
fresh Roba projection. Extension authors must namespace capabilities and treat
their declared provider-tool manifest as trusted launch configuration, because
Tower MCP does not expose complete synchronous router introspection. Approval
is not authorization: a capability forbidden to the provider must be absent
from the provider fragment, not merely omitted from the manifest.

The first consumer is `roba-git`. It shares one fixed repository service
between projections, contributes bounded `git.snapshot` observation to both,
and keeps `git.stage_all` in writable control projections only. It adds no
pre-turn prompt context. The provider sees exact native approvals for only
the tools present in its fragment.

Launch URLs and credentials are absent from `RunSpec`, `TurnRequest`, run and
MCP result schemas, and event projections. Launch-context diagnostics may show
the loopback URL but redact the bearer credential. A provider process
necessarily receives the credential and could repeat it in its own output;
Roba's guarantee is that the host does not structurally copy that launch
material into public values or log the credential.

Loopback binding is a deliberate admission prerequisite. A host that forbids
an ephemeral IPv4 loopback listener receives a typed runtime refusal before
provider work starts; the agent does not silently downgrade to a run without
its provider projection.

`agent.turn` is one optional dual-path MCP tool. A caller that negotiates
Tasks receives a process-local live Task; a caller without Tasks uses the
same synchronous fallback that powers `roba run`. Tower allocates the Task,
then Roba's preparation step admits exactly one finite run and attaches its
operation id under the `com.github.joshrotenberg.roba/operation` metadata key.
No task-id registry is needed.

`tasks/cancel` acknowledges the cancellation signal immediately, as required
by MCP. `tasks/get` or `task_wait` is the settlement barrier: the Task remains
nonterminal while Roba cancels and drains its captured run. If completion wins
the race, the Task retains the completed typed tool result. If Task
cancellation wins and the run settles cancelled, the Task becomes
`cancelled`; MCP's cancelled Task shape carries no tool result, while
`roba://agent` and `roba://events` retain the Roba settlement.
Completed Task outcomes, including typed provider failures with
`isError: true`, carry the same display and structured content as the
synchronous path.

The Tower in-memory task store is process-scoped. Active Tasks receive a
practically process-lifetime lease so long turns do not disappear after
Tower's five-minute default. On settlement the lease is shortened to retain
the result for at least five minutes, and expired records are reclaimed at the
next Task admission. Task durability and restart reconciliation remain
workflow-layer concerns.

Admission is single-flight. A second turn is refused as `busy`; it is never
queued or silently treated as steering. A detached coordinator owns internal
settlement, so dropping a Rust caller waiting on `AgentInstance::turn` cannot
leave the instance permanently running. With Tower MCP 0.22's current
`ChannelTransport`, dropping a direct MCP call detaches only its waiter. The
admitted operation stays visible and supervised until it finishes or an
explicit `agent.interrupt` or `agent.shutdown` drains it. This is an
intentional direct-call contract, not an implicit background queue.

`StdioBinding` cross-couples the transport and logical agent lifetimes. Stdio
EOF begins agent shutdown before Tower waits for in-flight requests, preventing
a long synchronous turn from deadlocking the transport drain. Conversely,
`agent.shutdown` stops the transport only after closing admission and draining
the active run; Tower flushes that tool response before the binding returns.
Every transport exit path awaits idempotent agent shutdown, including malformed
or broken streams. A binding shutdown handle starts the same graceful drain;
it never abandons the binding future.

Foreground `StdioBinding::run` is single-use and reads stdin through a bounded
async bridge backed by one ordinary detached reader thread. This avoids
Tokio's uncancellable stdin blocking-pool read pinning runtime shutdown when a
client keeps its pipe open after `agent.shutdown`. An embedding that continues
after the binding returns should use `run_with_streams` with a reader it owns
and can close.

The root `roba run` command is the production in-process client, while
`roba serve` owns one foreground stdio binding for clients such as `mcp-repl`.
The latter starts idle and does not launch a provider until `agent.turn` is
admitted. Provider failures remain typed results and leave the binding hot;
only logical shutdown, stdio EOF, or the host's shutdown policy ends it.

Unix/HTTP operator bindings remain future work described in
[`docs/architecture/mcp-harness.md`](../../docs/architecture/mcp-harness.md).
The private HTTP listener is operation-scoped provider plumbing, not a general
HTTP binding. The base `self` handler is immediate and `roba-git` bounds its
read calls. Before an extension adds arbitrary or long-running provider
callbacks, the endpoint host must add explicit request tracking and
cancellation rather than generalizing the current teardown claim.
