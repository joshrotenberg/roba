---
name: use-roba
description: How to invoke roba correctly from scripts and agent pipelines -- core contract, exit codes, JSON envelope, key flags, and common patterns.
---

# use-roba

A reference for scripts and agent pipelines that invoke roba. Covers
the output contract, exit codes, JSON envelope shape, key flags, and
common invocation patterns.

## Core contract

- **stdout** = the answer only. Pipe-safe. `roba "question" | jq` always
  sees the answer and only the answer.
- **stderr** = metadata: cost footer, spinner, tool-call markers, refusal
  warnings, error messages. Visible to humans on a TTY, invisible to
  scripts that don't capture it.
- Auto-detects TTY vs pipe: rich markdown render and spinner on a TTY,
  plain text on a pipe. `--plain` is the manual override; `NO_COLOR=1`
  is honored.
- `--quiet` / `-q` suppresses metadata (footer, spinner, tool markers)
  without affecting stdout content. Use in scripts when stderr noise
  is unwanted.

## Exit codes

| Code | Meaning |
|------|---------|
| 0 | Success (answer returned; `refusal` in `--json` may still be true) |
| 1 | Usage / argument error, or history error |
| 2 | Auth failure (not logged in, permission denied) |
| 3 | Budget exceeded |
| 4 | Timeout |

Refusals exit 0 -- the call succeeded; the heuristic labeled the body.
Detect refusals via the `refusal` field in `--json` output.

## `--json` envelope (v1)

On success, the envelope goes to **stdout**:

```json
{
  "version": 1,
  "result": {
    "result": "the answer text",
    "session_id": "abc123",
    "total_cost_usd": 0.0123,
    "duration_ms": 1234,
    "num_turns": 7,
    "is_error": false
  },
  "refusal": false
}
```

Field map (success):

| Path | Type | Notes |
|------|------|-------|
| `.version` | int | Always `1`. **Top-level.** |
| `.refusal` | bool | True when the answer matched the refusal heuristic. **Top-level.** Still exits 0. |
| `.result.result` | string | The answer text. **This is the answer**, not `.result`. |
| `.result.session_id` | string | Session id, for `-c`/`--resume` follow-ups. |
| `.result.total_cost_usd` | number \| null | Cost in USD. Serde renames `cost_usd` -> `total_cost_usd`. May be null. |
| `.result.duration_ms` | int \| null | Wall-clock ms. May be null. |
| `.result.num_turns` | int \| null | Turn count. May be null. |
| `.result.is_error` | bool | Wrapper-level error flag. |

Plus any extra CLI fields, flattened into `.result`.

**Schema gotchas (read this before writing `jq`):**

- The answer is at **`.result.result`**, *not* `.result`. `jq -r '.result'`
  prints the whole inner object, not the text.
- Metrics nest one level deep, under `.result`. `jq '.duration_ms'`,
  `jq '.num_turns'`, `jq '.cost_usd'` all return **`null`** at the top
  level -- use `.result.duration_ms`, `.result.num_turns`,
  `.result.total_cost_usd`.
- The cost field is serialized as **`total_cost_usd`** (serde rename of
  the internal `cost_usd`); `jq '.result.cost_usd'` is `null`.
- `total_cost_usd` / `duration_ms` / `num_turns` are `Option` -- expect
  `null` when the run or path didn't report them.
- `version` and `refusal` are the only **top-level** fields besides
  `result`.

Extraction example:

```sh
out=$(roba --json --quiet "what is the capital of France?")
echo "$out" | jq -r '.result.result'
echo "$out" | jq '{cost: .result.total_cost_usd, turns: .result.num_turns, refused: .refusal}'
```

On a runtime error, the envelope goes to **stderr** (stdout stays empty):

```json
{
  "version": 1,
  "error": {
    "kind": "auth",
    "message": "claude -p exited with 1: not logged in",
    "exit_code": 2,
    "chain": ["top context", "root cause"],
    "see_also": ["https://docs.example/page"]
  }
}
```

`kind` values: `"auth"` (exit 2), `"budget"` (3), `"timeout"` (4),
`"history"` (1), `"other"` (1). `exit_code` mirrors the process exit
code. `chain` lists the anyhow context layers, top (most recent) to root
(underlying cause). `see_also` is an optional list of doc URLs --
**omitted entirely when empty**, so don't assume the key is present.

Version 1 contract: `version` is always present; `result` and `error`
are mutually exclusive; new fields may be added additively without
bumping the version.

## Key flags for agent use

| Flag | Description |
|------|-------------|
| `--json` | Structured envelope on stdout (success) or stderr (error) |
| `--quiet` / `-q` | Suppress metadata: footer, spinner, tool markers |
| `-p TEXT` / `--prompt TEXT` | Explicit prompt flag; use when `-c` or `-w` are also present to avoid ambiguity (those flags consume a space-separated word as their value) |
| `-c` | Continue most recent session in this directory |
| `-c=ID` or `-c ID` | Resume a specific session by id |
| `--session NAME` / `ROBA_SESSION=NAME` | Resume a session by a stable name bound in a roba.toml `[session]` table (`NAME = "<uuid>"`); roba reads the map and resumes the bound id. Conflicts with `-c` / `--pick` / `--fresh`. Useful for long-lived per-repo or driver sessions you don't want to address by UUID. |
| `--fresh` | Force a new session even if config or env has `continue = true` |
| `-w` / `--worktree` | Run in a fresh git worktree (sandboxed checkout); use for parallel same-repo workers that must not share a branch |
| `-w=NAME` / `-w NAME` | Same, with a pinned worktree name (useful for branch naming) |
| `-C PATH` | Run as if invoked from a different directory (git -C style) |
| `--trace PATH` | Write raw streaming events to a JSONL file as they arrive; stable observability handle for in-flight runs |
| `--no-retry` | Fail fast on transient failures; no auto-retry; for orchestrators that want deterministic failure |
| `--show-permissions` | Preview the resolved permission stack with per-entry provenance; exits 0 without calling claude |
| `--writable` | Add Edit and Write to the default read-only permission set |
| `--full-auto` | Bypass all permission checks (sandbox use only) |
| `--allow-tool TOOL` | Add a specific tool pattern (repeatable) |
| `--deny-tool TOOL` | Block a specific tool pattern (repeatable) |
| `--permission-mode MODE` | Pass a permission mode directly to claude: `dontAsk` (auto-approve allowed tools), `acceptEdits` (auto-accept file edits), `plan` (show plan before executing), `auto`, `default`. Coexists with `--writable` / `--allow-tool`: those set the allowlist, this sets the interaction mode. |
| `--system-prompt TEXT` | Replace the default system prompt for this call. When combined with `--append-system-prompt`, replace runs first. |
| `--append-system-prompt TEXT` | Append TEXT to the default system prompt. Use to add agent-specific instructions without losing the default context. |
| `--agent NAME` | Pin a Claude Code subagent (e.g. a project-specific agent under `.claude/agents/`) |
| `--bare` | Minimal-overhead mode: skip hooks, LSP, plugin sync, CLAUDE.md auto-discovery, auto-memory, keychain reads. Use for non-interactive agent dispatches where those features add latency but no value. |
| `--effort LEVEL` | Cost/quality tradeoff: `low`, `medium`, `high`, `xhigh`, `max`. Default is haiku-level behavior; `max` gets the most thorough response at higher cost. |
| `--model MODEL` | Override the model for this call. Accepts aliases (`haiku`, `sonnet`, `opus`) or full ids. |

## Permission precedence

One rule: CLI > `ROBA_<PARAM>` env > active profile > config file > built-in default;
`--deny-tool` beats `--allow-tool` at any layer.

Default permission set is read-only: Read, Glob, Grep only. Opt in
explicitly with `--writable`, `--allow-tool`, or `--full-auto`.

`--permission-mode MODE` is orthogonal to the allowlist flags. The shortcut
flags (`--writable`, `--full-auto`) control *which tools* are allowed;
`--permission-mode` controls *how claude behaves* when using those tools (e.g.
`dontAsk` to skip interactive approval prompts, `plan` to show a plan first).
Both can be set in the same call.

## Common agent patterns

**Silent JSON call: extract the answer**

```bash
result=$(roba --json --quiet "what is the capital of France?")
echo "$result" | jq -r '.result.result'
```

**Capture session id and continue in a follow-up call**

```bash
first=$(roba --json --quiet "explain the ownership model")
session_id=$(echo "$first" | jq -r '.result.session_id')
roba --json --quiet -c "$session_id" -p "now give me a concrete example"
```

**Sandboxed writable worktree**

```bash
roba --writable -w=my-branch -p "implement the foo feature in src/foo.rs"
```

**Observability with trace for a long task**

```bash
roba --trace /tmp/run.jsonl --json -p "analyze the codebase for security issues"
# tail -f /tmp/run.jsonl to watch streaming events as they arrive
```

`--trace` passes through claude code's raw stream-json events unchanged.
Among them, an orchestrator tailing the file will see a `system` event with
`"subtype": "post_turn_summary"` near the end of a turn -- a convenient "is
it done / what happened" handle:

```json
{ "type": "system", "subtype": "post_turn_summary",
  "status_category": "review_ready",
  "status_detail": "issue #177 fixed: ... all checks green",
  "needs_action": "", "session_id": "...", "summarizes_uuid": "...", "uuid": "..." }
```

`status_category` (e.g. `review_ready`) plus `status_detail` (a
human-readable summary) are useful as a completion signal. Caveat: this is
claude code's event passed through the trace, not part of roba's versioned
ABI -- roba just forwards it and the schema is upstream and may change.
Don't depend on it the way you can depend on the `--json` envelope or the
exit codes.

**Run in a different project directory**

```bash
roba -C /path/to/project --json --quiet "summarize the public API"
```

**Branch on failure type via exit code**

```bash
roba --no-retry --json "summarize this file" || {
  case $? in
    2) echo "auth failure -- check claude login" ;;
    3) echo "budget exceeded" ;;
    4) echo "timeout -- retry or increase timeout" ;;
    *) echo "other failure" ;;
  esac
}
```

**Fast non-interactive dispatch (agent-tier)**

```bash
roba --bare --effort low --quiet --json \
  -p "summarize the last 5 commits" --git-log 5
```

`--bare` skips all hooks and auto-discovery overhead; `--effort low` uses the
cheapest compute level. Together they are the recommended flags for high-volume
or latency-sensitive agent pipelines.
