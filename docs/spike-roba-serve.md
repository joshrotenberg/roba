# Spike: roba serve / MCP server

**Branch:** `spike/roba-serve`
**Date:** 2026-06-04
**Status:** Parked -- revisit when core "claude -p but better" work is stable.

## What was validated

1. **tower-mcp 0.12 API fits cleanly.** `ToolBuilder` handler pattern (`|input: T|
   async move { ... }`) with a `JsonSchema`-derived input type is axum-shaped.
   Wire a tool in ~15 lines, a resource in ~5. No proc macros, no friction.

2. **Real dispatch works.** `dispatch_for_mcp()` in `src/serve.rs` calls
   `execute_json` directly, bypassing `run_ask`'s stdout-writing output path.
   Returns `(result_text, session_id, model)` as plain values. Zero impedance --
   tower-mcp handlers are `async` and tokio is already on the runtime.

3. **Session continuation threads cleanly.** `QueryResult.session_id` is a plain
   `String` (always populated). Pass it back in the next `send_prompt` call and
   `.resume(id)` picks up the thread. No null checks needed.

4. **Resources work.** `roba://status` (static) and `roba://sessions` (history
   snapshot at startup, top 10) both registered and served. `HistoryRoot::home()
   + list_sessions_with()` is the right seam for the sessions list.

5. **StdioTransport is the right transport for Claude Code.** Claude Code manages
   the process lifecycle; `.mcp.json` at the repo root registers `roba serve
   --stdio` and auto-discovers it. `mcp__roba__send_prompt` shows up as a
   first-class tool call in the UI with prompt + response inline.

## What the tool surface looks like

**`send_prompt` tool input:**
```json
{
  "prompt": "what changed in the last 5 commits?",
  "session_id": "optional-uuid-to-continue",
  "model": "claude-haiku-4-5",
  "system_prompt": "be concise"
}
```

**`send_prompt` tool output:**
```json
{
  "result": "...",
  "session_id": "uuid-claude-assigned",
  "model": "claude-sonnet-4-5"
}
```

**Resources:** `roba://status`, `roba://sessions`

## What remains for the real impl

- **Unix socket transport** -- `tower-mcp = { features = ["unix"] }` adds
  axum + hyper. Needed for the auto-routing story (check
  `~/.local/state/roba/server.sock`, fall back to direct spawn). For
  Claude Code integration, stdio is sufficient.

- **Dynamic resources** -- `roba://sessions` is snapshotted at startup today.
  tower-mcp 0.12 doesn't expose a dynamic resource callback in the current
  surface; a real impl either reloads at startup or adds a `refresh_sessions`
  tool.

- **State injection** -- `State<T>` extractor (same as axum) is the right seam
  for threading a shared config or pool into handlers. Not needed while
  `dispatch_for_mcp` is stateless (just builds a fresh `Claude` client per
  call).

- **Permissions / profile passthrough** -- `dispatch_for_mcp` uses hardcoded
  readonly defaults today. Wiring `system_prompt`, `--writable`, profile
  resolution through a `DispatchArgs`-like struct is the next step.

- **roba-core workspace split** -- post-spike architecture for when
  `roba-server` becomes its own binary. `DispatchArgs` lives in `roba-core`,
  both the CLI and MCP server map onto it. Don't do this until the server
  design is settled.

## Key context for the next session

The auto-routing design (check socket, fall back to direct spawn) is still an
open question -- see CLAUDE.md brainstorm sketch. The spike proves the MCP
server side works; the client-side routing logic in `run_ask` is unwritten.

Issues subsumed by this design: #9 (async), #12 (REPL), #37 (named sessions),
#66 (--with-mcp). None of those need separate implementation -- they're all
surfaces on top of the serve model.
