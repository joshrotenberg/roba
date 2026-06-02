# roba reference

One-stop lookup for flags, environment variables, exit codes, the JSON
envelope schema, and the `roba.toml` config schema. For narrative
context, see the linked topic pages.

Source of truth: `src/cli.rs` (flags), `src/env.rs` (env vars),
`src/lib.rs` (`classify_exit_code`), `src/error.rs` and `src/lib.rs`
(JSON envelopes), `src/profile/types.rs` and `src/aliases.rs` (config
schema). Run `roba SUBCOMMAND --help` for the definitive per-flag docs.

---

## Subcommands

### `roba [PROMPT]` -- ask

The default action. Sends a prompt to claude and renders the answer.

| Flag | Short | Notes |
|---|---|---|
| `[PROMPT]` | | Positional prompt. Pass `-` for explicit stdin |
| `--prompt TEXT` | `-p` | Explicit prompt; escapes ambiguity against `-c`/`-w` |
| `--file PATH` | `-f` | Read the prompt from a file |
| `--editor` | `-e` | Compose in `$VISUAL` / `$EDITOR` (falls back to vi) |
| `--editor-history N` | | With `-e`, pre-fill last N responses. Default 1; 0 disables |
| `--prepend PATH` | | Prepend file contents to prompt (repeatable) |
| `--append PATH` | | Append file contents to prompt (repeatable) |
| `--attach GLOB` | | Embed glob-matched files with `File: PATH` framing (repeatable) |
| `--git-diff` | | Embed `git diff` (working tree) |
| `--git-log [N]` | | Embed `git log --oneline -n N` (default 5) |
| `--git-status` | | Embed `git status --short` |
| `--var K=V` | | Substitute `{{KEY}}` placeholders (repeatable) |
| `--quiet` | `-q` | Suppress metadata (footer, spinner, tool markers) |
| `--json` | | Emit the versioned JSON envelope on stdout |
| `--code [LANG]` | | Print only fenced code blocks; optional language filter |
| `--out PATH` | `-o` | Write result to file AND stdout |
| `--trace PATH` | | Write streaming events to JSONL file as they arrive |
| `--stream` | | Live tokens + tool-call indicators (TTY only) |
| `--show-thinking` | | Render extended-thinking blocks live (with `--stream`) |
| `--echo` | | Print resolved prompt before the response |
| `--plain` | | Disable markdown render, color, spinner |
| `--rates-file PATH` | | Override bundled rates table for footer dollar figure |
| `--no-dollars` | | Omit dollar figure from footer (tokens only) |
| `--no-retry` | | Disable wrapper-level auto-retry; surface failures immediately |
| `--model MODEL` | | Override model (alias or full id) |
| `--continue [ID]` | `-c` | Continue most recent session (bare `-c`) or a specific id |
| `--fork` | | Branch the resumed session (requires `-c ID`) |
| `--pick` | | Interactive fuzzy session chooser |
| `--fresh` | | Force new session; cancels profile/env `continue = true` |
| `--worktree [NAME]` | `-w` | Run in a fresh git worktree; optional name pins the branch |
| `--agent NAME` | | Pin a Claude Code subagent for this run |
| `--readonly` | | Explicit default: Read, Glob, Grep only. Active suppressor |
| `--writable` | | Add Edit + Write |
| `--full-auto` | | Bypass all permission checks (sandbox only) |
| `--allow-tool TOOL` | | Add a tool/pattern to the allow list (repeatable) |
| `--deny-tool TOOL` | | Deny a tool/pattern (repeatable; deny beats allow) |
| `--show-permissions` | | Preview resolved allow/deny set with provenance; exit 0 |
| `--profile NAME` | | Apply a named profile |
| `--no-default-profile` | | Skip auto-applying `default` and `ROBA_PROFILE` |
| `-C PATH` | | Change working directory before all resolution |

### `roba history`

List recent sessions.

| Flag | Notes |
|---|---|
| `-n N` / `--limit N` | Max sessions to show (default 10) |
| `--all` | Show all (no limit) |
| `--project SLUG` | Filter to one project |
| `--all-projects` | Widen across all projects |
| `--json` | Emit JSON |

### `roba last`

Reprint the most recent session's last answer.

| Flag | Notes |
|---|---|
| `-n N` | How many items (default 1) |
| `--type text\|tools\|all` | Filter type (default `text`) |
| `--project SLUG` | Filter to one project |
| `--all-projects` | Widen across all projects |

### `roba cost`

Roll up token usage (and dollar cost) across session history.

| Flag | Notes |
|---|---|
| `--by-project` | Group totals by project slug |
| `--project SLUG` | Filter to one project |
| `-n N` / `--limit N` | Top N projects (with `--by-project`; default 10) |
| `--json` | Emit JSON |
| `--rates-file PATH` | Override bundled rates table |
| `--no-dollars` | Tokens only |

### `roba profile`

Inspect and manage `roba.toml` profiles.

| Verb | Notes |
|---|---|
| `list` | Profile names in the merged pool |
| `show NAME` | TOML for one profile |
| `init [--force]` | Write a starter `roba.toml` |
| `path` | Resolved config file chain |
| `active` | Which profile would auto-apply now |

### `roba alias`

Inspect user-defined aliases.

| Verb | Notes |
|---|---|
| `list` | All aliases with descriptions |
| `show NAME` | One alias's definition + expansion preview |
| `path` | Which files contribute aliases |

### `roba skill`

Manage the bundled skill library.

| Verb | Notes |
|---|---|
| `install [--to PATH] [--dry-run] [--force] [--skip]` | Copy to `~/.claude/skills/` |
| `list [--urls]` | Bundled skills; `--urls` swaps description for URL columns |
| `show NAME [--url]` | Print SKILL.md body; `--url` prints URLs and suppresses body |

### `roba agent`

Manage the bundled agent library.

| Verb | Notes |
|---|---|
| `install [--to PATH] [--dry-run] [--force] [--skip]` | Copy to `~/.claude/agents/` |
| `list [--urls]` | Bundled agents; `--urls` swaps description for URL columns |
| `show NAME [--url]` | Print AGENT.md body; `--url` prints URLs and suppresses body |

---

## Environment variables

Every `ROBA_<PARAM>` var matches the CLI long-form, uppercased with
`-` -> `_`. Sits between CLI (highest) and the file pool. Bool vars
accept truthy `1`/`true`/`yes`/`on` (case-insensitive); other values
are ignored -- the env layer can only enable a bool, never disable one
set by a file. List vars are comma-separated; whitespace trimmed.

Source: `src/env.rs`.

| Variable | Type | Matches |
|---|---|---|
| `ROBA_MODEL` | string | `--model` |
| `ROBA_AGENT` | string | `--agent` |
| `ROBA_PREPEND` | path list | `--prepend` |
| `ROBA_APPEND` | path list | `--append` |
| `ROBA_ATTACH` | string list | `--attach` |
| `ROBA_GIT_DIFF` | bool | `--git-diff` |
| `ROBA_GIT_LOG` | number | `--git-log N` |
| `ROBA_GIT_STATUS` | bool | `--git-status` |
| `ROBA_CONTINUE` | bool or string | `-c` (truthy = most recent; any other non-falsy string = specific id) |
| `ROBA_READONLY` | bool | `--readonly` |
| `ROBA_WRITABLE` | bool | `--writable` |
| `ROBA_FULL_AUTO` | bool | `--full-auto` |
| `ROBA_ALLOW_TOOL` | string list | `--allow-tool` |
| `ROBA_DENY_TOOL` | string list | `--deny-tool` |
| `ROBA_STREAM` | bool | `--stream` |
| `ROBA_SHOW_THINKING` | bool | `--show-thinking` |
| `ROBA_ECHO` | bool | `--echo` |
| `ROBA_PLAIN` | bool | `--plain` |
| `ROBA_QUIET` | bool | `--quiet` |
| `ROBA_JSON` | bool | `--json` |
| `ROBA_EDITOR_HISTORY` | number | `--editor-history` |
| `ROBA_TRACE` | string (path) | `--trace` |
| `ROBA_RATES_FILE` | string (path) | `--rates-file` |
| `ROBA_NO_DOLLARS` | bool | `--no-dollars` |
| `ROBA_WORKTREE` | bool or string | `-w` (truthy = unnamed; any other non-falsy string = name) |
| `ROBA_NO_RETRY` | bool | `--no-retry` |
| `ROBA_PROFILE` | string | `--profile` |
| `ROBA_VAR_<KEY>` | string | `--var KEY=VALUE` (one env var per key) |

---

## Exit codes

Source: `src/lib.rs`, `classify_exit_code`.

| Code | Meaning |
|---|---|
| `0` | Success |
| `1` | Generic failure (also: `history` wrapper error) |
| `2` | Authentication required / token invalid |
| `3` | Budget ceiling exceeded |
| `4` | Request timed out |

Clap parse errors (mistyped flags, missing required values) exit with
clap's own codes (usually 2) regardless of the above -- they fire before
roba's dispatch reaches the classifier.

---

## JSON envelope

`--json` wraps every output (success and error) in a versioned envelope.
Peel off `version` before inspecting inside. The version field is stable;
breaking shape changes bump it. Additive fields may appear in existing
versions without a bump.

Source: `src/lib.rs` (success), `src/error.rs` (error). Narrative:
[scripting.md](scripting.md).

### Success

Emitted on **stdout**. Exit 0.

```json
{
  "version": 1,
  "result": {
    "result": "the answer text",
    "session_id": "abc12345",
    "is_error": false
  },
  "refusal": false
}
```

`refusal` is `true` when the response heuristic (`looks_like_refusal`)
matched the body -- useful for orchestrators that need to distinguish
"got refused" from "got an answer" without parsing text. A refusal still
exits 0.

### Error

Emitted on **stderr**. Exits with the typed code.

```json
{
  "version": 1,
  "error": {
    "kind": "auth",
    "message": "top-level error description",
    "exit_code": 2,
    "chain": ["top context", "...", "root cause"],
    "see_also": ["https://.../doc"]
  }
}
```

`kind` is one of `"auth"` (2), `"budget"` (3), `"timeout"` (4),
`"history"` (1), `"other"` (1). `chain` is the anyhow error chain from
top to root. `see_also` is additive and omitted when empty.

---

## roba.toml config schema

Config lives in `~/.config/roba.toml` (user-level) and any `roba.toml`
walking up from cwd to the git root (project chain). Closer files
override farther ones per scalar key; lists concat; `vars` merge per-key.
Top-level keys are unnamed defaults; `[profile.NAME]` tables are opt-in
overlays; `[alias.NAME]` tables define shortcuts.

Source: `src/profile/types.rs`, `src/aliases.rs`. Narrative:
[profiles.md](profiles.md), [aliases.md](aliases.md).

### Top-level defaults and `[profile.NAME]`

Both use the same field set. Every field is optional.

| Field | Type | Maps to |
|---|---|---|
| `prepend` | `[path]` | `--prepend` (repeatable) |
| `append` | `[path]` | `--append` (repeatable) |
| `attach` | `[glob]` | `--attach` (repeatable) |
| `git_diff` | bool | `--git-diff` |
| `git_log` | int | `--git-log N` |
| `git_status` | bool | `--git-status` |
| `readonly` | bool | `--readonly` |
| `writable` | bool | `--writable` |
| `full_auto` | bool | `--full-auto` |
| `continue` | bool or string | `-c` (`true` = most recent; string = specific id) |
| `allow_tool` | `[string]` | `--allow-tool` (repeatable) |
| `deny_tool` | `[string]` | `--deny-tool` (repeatable) |
| `vars` | `{ key = "value" }` | `--var KEY=VALUE` |
| `model` | string | `--model` |
| `agent` | string | `--agent` |
| `stream` | bool | `--stream` |
| `show_thinking` | bool | `--show-thinking` |
| `echo` | bool | `--echo` |
| `plain` | bool | `--plain` |
| `quiet` | bool | `--quiet` |
| `json` | bool | `--json` |
| `editor_history` | int | `--editor-history N` |
| `worktree` | bool or string | `-w` (`true` = unnamed; string = pinned name) |
| `no_retry` | bool | `--no-retry` |
| `trace` | string (path) | `--trace` |
| `rates_file` | string (path) | `--rates-file` |
| `no_dollars` | bool | `--no-dollars` |

Unknown keys are rejected at parse time.

### `[alias.NAME]`

| Field | Type | Notes |
|---|---|---|
| `description` | string | Shown by `roba alias list` |
| `agent` | string | Pin `--agent NAME` for this alias |
| `template` | string | Prompt body with substitution. Omit for a flag-shortcut alias |
| `flags` | `[string]` | Default CLI flags added to every dispatch |
| `args` | `[string]` | Positional schema: position 1 -> `${args[0]}`, etc. |

Substitution in `template`: `${1}` / `${@}` (positional), `${NAME}`
(named via `args`), `$$` (literal `$`), `$(...)` (shell substitution
via `sh -c`). See [aliases.md](aliases.md) for full details and
caveats.
