# roba-mcp

`roba-mcp` hosts one hot logical Roba agent behind a typed Model Context
Protocol contract. Each `agent.turn` call creates one finite `roba-core` run;
the host retains the provider session between calls while remaining idle
between provider processes.

The first implementation is deliberately process-local:

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

External transports, extension fragments, and provider self-access remain
later phases in `../docs/design/mcp-native-agent-harness.md`. The root
`roba run` command is the first production client of this process-local
contract.
