# roba-mcp

`roba-mcp` hosts one hot logical Roba agent behind a typed Model Context
Protocol contract. Each `agent.turn` call creates one finite `roba-core` run;
the host retains the provider session between calls while remaining idle
between provider processes.

The operator contract has process-local and foreground stdio bindings:

- Stable `initialize` and final `server/discover` publish concise operator
  instructions describing the single-flight lifecycle and pointing clients at
  the authoritative state, context, and event resources. Capability discovery
  remains the canonical API reference; clients decide how to render the
  guidance.
- `AgentInstance` owns a suspended `RunSpec`, one immutable session policy,
  one monotonic session generation, one optional provider session, and at most
  one active `RunHandle`.
- `agent.turn { "text": ..., "overrides": ... }` waits for one finite run.
  Optional model, effort, and limit overrides apply only to that operation.
  Success returns the
  assistant answer as text plus typed `structuredContent`; provider failures,
  cancellation, blank prompts, `busy`, and `stopped` are typed tool results
  with `isError: true`.
- `agent.follow_up` queues another prompt for one named active operation at
  the next provider-turn boundary.
  `agent.interrupt` cancels that exact operation and waits for settlement.
  Both require an operation id so a delayed control cannot affect a later
  turn.
- `agent.shutdown` permanently closes admission and drains active work before
  reporting that the logical agent is stopped.
- `agent.session.rotate` performs an idle, generation-fenced clean rotation.
  It drops retained provider continuity without a model call, advances the
  generation once, and resets its observed provider-turn count.
- Result variants encode their terminal invariants: a completed result always
  has an outcome and a failed result always has a failure.
- `roba://agent` dynamically reports configured policy,
  idle/running/stopping/stopped state, session policy, generation,
  availability, observed provider-turn count, current operation identity,
  provider-native observation health, active activity, elapsed and
  timeout-remaining time, and the latest terminal result. Session identifiers
  are redacted from this resource; the originating `agent.turn` result retains
  valid session evidence.
- `roba://events` and `roba://events{?after,limit}` expose bounded,
  agent-wide replay. Global sequences continue across finite runs, source-run
  sequences remain visible, lost history is explicit, and event projections
  redact provider session ids.
- `roba://context` exposes the content-free effective context manifest and
  current or latest provider read evidence. Individual bodies are available
  only through the generation-fenced
  `roba://context/entry{?id,generation}` template.
- `roba://context/catalog` exposes the content-free managed agent, skill, and
  prompt inventory plus the effective selection. Explicit operator reads use
  `roba://context/catalog/artifact{?id}`. Enabled reusable prompts appear in
  `prompts/list` and render through the same catalog source of truth.
- `connect_in_process` returns an initialized production `McpClient` over
  Tower MCP's concurrent `ChannelTransport`.
- `call_turn` is the typed client seam. It requires valid
  `structuredContent`, checks that MCP `isError` agrees with the typed status,
  and never treats display text as machine data.
- `StdioBinding` serves the same control router over stdin/stdout with stable
  and final MCP lifecycle support. Requests dispatch concurrently, so status,
  event reads, follow-up, and interruption can overtake a long synchronous
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
`self` (qualified as `roba.self` by provider clients), the content-free
`roba://context` manifest, and its generation-fenced entry resource template.
It also publishes equivalent read-only `context.manifest` and `context.read`
tools so provider adapters do not depend on native resource support. Every
base tool is included in the exact provider launch allowlist. Installed
extensions may contribute separate provider tools or resources, but nothing
from their control fragment is mirrored automatically. The base publishes no
turns, follow-up, interruption, shutdown, Tasks, configuration, event history,
or retained session evidence. This proves provider re-entry without opening
recursion or operator authority.

## Extension composition

`AgentExtension` contains named control and provider `McpRouter` fragments,
an explicit manifest of provider-callable tools, and optional retained or
externally available context entries. `AgentExtensions` is an immutable
aggregate installed through `AgentInstance::new_with_extensions`. Construction
preflights each projection against Roba's real base router and fails closed on
exact tool, resource, resource-template, prompt, or context-ID conflicts.
Actual control and per-operation provider routers repeat that validated merge;
last-writer-wins replacement is never used.

Extension context compiles into the host's existing immutable `ContextPlan` at
that same construction boundary. Audience restrictions are structural, and
retained bodies remain outside `RunSpec`, provider prompt text, snapshots, and
extension debug output.

The root host installs managed context as an ordinary extension contribution.
Its control fragment owns catalog resources and selected MCP prompts; its
provider fragment is empty. The selected agent role and transitive skills join
the context plan instead. The agent is a mandatory generation-fenced read,
skills are lazy, and reusable prompts never become standing provider context.

Fragments are capability bags. Their router identity, session state, task
store, auth, and middleware are not imported when Tower merges them into a
fresh Roba projection. Extension authors must namespace capabilities and treat
their declared provider-tool manifest as trusted launch configuration, because
Tower MCP does not expose complete synchronous router introspection. Approval
is not authorization: a capability forbidden to the provider must be absent
from the provider fragment, not merely omitted from the manifest.

The first consumer is `roba-git`. It shares one fixed repository service
between projections, contributes bounded `git.snapshot` observation to both,
and keeps `git.stage_all` in writable control projections only. Its small
activation entry is lazy context-plane material, not pre-turn prompt context.
The provider sees exact native approvals for only
the tools present in its fragment.

Extensions may attach an operation lifecycle observer for baseline, started,
periodic, settling, settled, and host-shutdown work. Hooks are
exact-operation-scoped, serialized per extension, bounded by a host timeout,
run outside the agent control lock, and fully drained before settlement.
Panics, timeouts, and typed failures become compact globally sequenced events;
full extension state belongs in extension-owned MCP resources.

Launch URLs and credentials are absent from `RunSpec`, `TurnRequest`, run and
MCP result schemas, and event projections. Launch-context diagnostics may show
the loopback URL but redact the bearer credential. A provider process
necessarily receives the credential and could repeat it in its own output;
Roba's guarantee is that the host does not structurally copy that launch
material into public values or log the credential.

`AgentInstance::context_plan` inventories the instructions and project/run
context already compiled into the fixed `RunSpec`, including current
every-turn freshness, stable IDs, origins, delivery intent, and redacted
fingerprints. Prompt material is retained only in the host-owned
`ContextPlan`; `ContextManifest`, `ContextSnapshot`, and `Debug` output contain
no bodies. Public and redacted bodies may be requested through the explicit
content resource; secret entries are structurally unavailable there.

Hosts that need additional MCP-native context start with
`ContextPlan::builder_from_run_spec`, add entries with explicit audience and
precedence, and construct the agent through
`AgentInstance::new_with_context_plan`. Construction fails if the supplied plan
omits or changes context already present in the executable `RunSpec`. The
operator can inspect the complete plan; the provider manifest and content
surface structurally exclude operator-only entries. Declared precedence orders
the plan but does not claim to override hidden provider-managed policy.

For each admitted operation, the host compiles an inspectable
`ContextBootstrap` containing provider identity, exact operation, authority,
current-goal delivery, provider-manifest fingerprint, and mandatory MCP
acquisitions. The rendered bootstrap is passed through the non-serializable
provider launch context; it contains no entry bodies and is redacted from
launch `Debug` output. `ContextSnapshot.bootstrap` exposes the typed contract
while active and for the latest settled operation. Asking for acquisition is
not evidence that it happened; provider-side reads remain the mechanical
evidence boundary.

Each provider-side manifest or entry read through either the resource or tool
form is recorded against the exact operation id and context generation, with
first/last timestamps and a saturating count. The private endpoint is drained
before settlement, so the control projection can retain the final evidence
without a late-read race. This proves only that the provider MCP client made
the request. Explicit acknowledgement, provider-native ambient inventory,
generation updates, and capability gating remain follow-on work.

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

While a Task is active, normalized activity and warning records are also sent
as `notifications/message` entries from the `roba.activity` logger. Final
protocol callers opt in with the per-request MCP log level. Notifications are
best-effort; `roba://events` remains the bounded replay source and reports
truncation explicitly. Activity summaries are provider-neutral and redacted:
raw commands, paths, tool inputs/results, and search queries are not published.
Silence is represented as unknown observation, never as fabricated progress.

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
queued or silently treated as a follow-up. A detached coordinator owns internal
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
