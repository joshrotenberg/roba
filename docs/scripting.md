# roba scripting / agent ABI

This page documents how roba's non-TTY consumption surface is shaped:
the stdout/stderr split that keeps the answer pipe-clean, the versioned
`--json` envelope that gives orchestrators a stable ABI, and the typed
exit codes plus `--no-retry` that make failures deterministic. It's for
anyone consuming roba from a script, an agent orchestrator, or CI --
the contract you can pin against. For the big-picture framing, see the
repo [`README.md`](../README.md). Permission control (also part of the
agent surface) lives in [`permissions.md`](permissions.md).

## Output discipline

- **stdout** = the answer. Pipe-safe.
- **stderr** = metadata (cost footer, tool calls, refusal warnings,
  spinner). Visible to humans, invisible to scripts that don't
  capture it.
- Auto-detect: rich on a TTY, plain on a pipe. `--plain` is the
  manual override. `NO_COLOR=1` honored.
- **Dispatch session id**: when the streaming pipeline is active
  (`--stream` or `--trace`), roba prints `[roba] session: <id>`
  to stderr on the first event that carries the id. Gives
  orchestrators a stable handle to `~/.claude/projects/<dir>/<id>.jsonl`
  without requiring `--trace` upfront. Suppressed by `--quiet`.

So `roba "foo" | jq` always sees clean stdout, even with the
spinner / footer / tool calls humming on stderr.

## Versioned JSON envelope

`--json` output is a versioned envelope so agent orchestrators can
pin a stable ABI. Every `--json` output (success or error) carries a
top-level `version` field; peel that off before inspecting anything
inside.

On success the record goes to **stdout** wrapped in `result`:

```json
{
  "version": 1,
  "result": {
    "result": "the answer text",
    "session_id": "abc123",
    "is_error": false
  },
  "refusal": false
}
```

`refusal` is true when the heuristic in `looks_like_refusal` matched
the response body; useful for orchestrators that need to branch on
"got an answer" vs "got refused" without parsing the body text. A
refusal still exits 0 -- the call succeeded, the heuristic just labels
the body.

On a runtime error roba prints the envelope to **stderr** (stdout
stays empty) and exits with the typed code:

```json
{
  "version": 1,
  "error": {
    "kind": "auth",
    "message": "claude -p exited with 1: not logged in",
    "exit_code": 2,
    "chain": ["top context", "...", "root cause"]
  }
}
```

`kind` is one of `"auth"` (exit 2), `"budget"` (3), `"timeout"`
(4), `"history"` (1), or `"other"` (1). The mapping mirrors the
typed exit codes. `chain` lists the anyhow context layers from
top (the most recent context call) down to the root cause.

**Version 1 contract:**

- Top-level `version: 1` is present on every `--json` output.
- Success carries a `result` field, error carries an `error` field;
  the two are mutually exclusive.
- Inner fields documented at v1 are preserved. New fields may be
  added in a backward-compatible (additive) way without bumping the
  version.
- Breaking shape changes (renames, removals, type changes) bump the
  version.

Without `--json`, the error path is unchanged: a styled
`error: ...` line on stderr. Clap parse errors (mistyped flags,
missing values) are emitted by clap itself before roba sees them
and stay as plain stderr regardless of `--json`.

## Failure modes

claude-wrapper can auto-retry some transient failure classes
(timeouts, certain exit codes) with backoff. That's a reasonable
default for a human on a TTY, but an orchestrator usually wants
deterministic semantics: see the failure now, decide whether to
retry itself. `--no-retry` turns wrapper-level auto-retry off for
the run so a transient failure surfaces immediately with its
normal typed exit code instead of being silently re-tried.

```bash
roba --no-retry "..."            # fail fast, no backoff
ROBA_NO_RETRY=1 roba "..."       # same, via env
```

It's also a profile field (`no_retry = true`). No effect on
success or on non-transient failures.
