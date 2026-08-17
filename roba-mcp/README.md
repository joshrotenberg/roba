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
- Result variants encode their terminal invariants: a completed result always
  has an outcome and a failed result always has a failure.
- `roba://agent` dynamically reports configured policy, idle/running/stopped
  state, session availability, current operation identity, and the latest
  terminal result. Session identifiers are redacted from this resource; the
  originating `agent.turn` result retains valid session evidence.
- `connect_in_process` returns an initialized production `McpClient` over
  Tower MCP's concurrent `ChannelTransport`.
- `call_turn` is the typed client seam. It requires valid
  `structuredContent`, checks that MCP `isError` agrees with the typed status,
  and never treats display text as machine data.

Admission is single-flight. A second turn is refused as `busy`; it is never
queued or silently treated as steering. A detached coordinator owns internal
settlement, so dropping a Rust caller waiting on `AgentInstance::turn` cannot
leave the instance permanently running. MCP request-cancellation behavior is
not defined by this phase.

External transports, steering/interrupt/shutdown tools, agent-wide events,
MCP Tasks, extension fragments, and provider self-access remain later phases
in `../docs/design/mcp-native-agent-harness.md`. The root `roba run` command is
the first production client of this process-local contract.
