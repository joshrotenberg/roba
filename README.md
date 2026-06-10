# roba

[![Crates.io](https://img.shields.io/crates/v/roba.svg)](https://crates.io/crates/roba)
[![Documentation](https://docs.rs/roba/badge.svg)](https://docs.rs/roba)
[![CI](https://github.com/joshrotenberg/roba/actions/workflows/ci.yml/badge.svg)](https://github.com/joshrotenberg/roba/actions/workflows/ci.yml)
[![Downloads](https://img.shields.io/crates/d/roba.svg)](https://crates.io/crates/roba)
[![License](https://img.shields.io/crates/l/roba.svg)](#license)

A single-prompt CLI runner on top of `claude -p`: one invocation, one
answer. Adds composable input, pipe-clean output, re-enterable sessions,
and a stable scripting ABI.

roba is sugar over the one binary -- not a platform, orchestrator,
daemon, or skills framework. Point it at a quick question, a CI step, or
an unattended worker.

- **Humans:** prompt from files / stdin / git context, rendered markdown
  on a TTY, flag bundles as profiles, history, cost.
- **Agents** ([Claude Code](https://github.com/anthropics/claude-code)):
  stdout is the answer, stderr is metadata, a versioned `--json`
  envelope, typed exit codes, `--trace` to watch a run.

Built on [`claude-wrapper`](https://crates.io/crates/claude-wrapper).

```console
$ roba "summarize the rust ownership model in 3 bullets"
   Rust's ownership model rests on three rules:

     • Each value has a single owner.
     • When the owner goes out of scope, the value is dropped.
     • Borrows are either many immutable or one mutable.

tokens 1.2k/450 · 2.0s · session abc12345
```

## Install

| Source | How |
|---|---|
| crates.io | `cargo install roba` |
| Homebrew | `brew install joshrotenberg/brew/roba` |
| Prebuilt binary | [latest release](https://github.com/joshrotenberg/roba/releases/latest) -- macOS (arm64 / x86_64), Linux (arm64 / x86_64), Windows; shell and PowerShell installers included |

`roba` shells out to the `claude` binary, so you need
[claude-code](https://github.com/anthropics/claude-code) installed and
authenticated (or `ANTHROPIC_API_KEY` set) on your `PATH`.

**The full flag, env-var, and config reference lives in the binary:
`roba --help`.** This README is the overview.

## vs. `claude -p` directly

`claude -p` is the one-shot primitive: prompt in, response to stdout,
exit. roba keeps that model and adds:

| Adds | How |
|---|---|
| **Composable input** | `-f` file, piped stdin, `-e` editor, `--prepend` / `--append`, `--attach` globs, `--git-diff` / `--git-log` / `--git-status`, `--var` template vars |
| **Pipe-clean output** | stdout is the answer; all metadata (footer, spinner, tool lines, warnings) goes to stderr -- piping to `jq` sees a clean stream |
| **TTY rendering** | markdown, spinner, color while it runs; `--plain` / `NO_COLOR` turns it off |
| **Session re-entry** | `-c` continue, `-c ID` resume, `--fork`, `--pick` chooser, `--session-id` mints a caller-chosen id; `roba history` / `roba last` browse past runs |
| **Read-only inspection** | `roba show <ID>` prints a stored run's result (`--metrics`, `--wait`); `roba worktree list`; `roba history --worktree NAME` finds a runner's session |
| **A stable scripting ABI** | typed exit codes, versioned `--json` envelope, clean stream split -- see [For agents & scripts](#for-agents--scripts) |

For interactive, multi-turn work, use `claude` itself; for multiple
providers, [`llm`](https://llm.datasette.com/). roba is Claude-only: the
Claude-Code-native integration (sessions, permissions, history) is the
point.

## Quick examples

```bash
# Just ask
roba "what's the difference between Arc and Rc?"

# Compose: preamble + question + appendix
roba --prepend system.md "review this design" --append context.md

# Pull in files by glob
roba --attach 'src/**/*.rs' "is the error handling consistent?"

# Read-only review against the working-tree diff
roba --readonly --git-diff "is this safe to merge?"

# Continue the most recent session here (pass the prompt with -p, since
# -c takes an optional session id)
roba -c -p "now show me how to test the unsafe variant"

# scripted multi-turn: mint an id once, reuse it on later turns
# (bare `claude -p --continue` no-ops in print mode; this is the fix)
uuid=$(uuidgen)
roba --session-id "$uuid" "start a refactor plan"
roba -c="$uuid" "now do step 1"

# Pipe-friendly: answer only, stdin in
roba "what's 2+2" -q            # prints "4"
echo "summarize this" | roba    # stdin works, no flag needed
```

> [!NOTE]
> `-c` (continue) and `-w` (worktree) take an *optional* value, so a
> space-separated word right after them is read as that value:
> `roba -c "follow up"` treats `follow up` as the session id. Pass the
> prompt explicitly with `-p` to disambiguate: `roba -c -p "follow up"`.

## Safe by default

roba starts read-only: claude may use `Read`, `Glob`, and `Grep` and
nothing else. You opt into more, explicitly:

```bash
roba "explain this"                      # read-only (default)
roba --writable "rename foo to bar"      # add Edit + Write
roba --allow-tool "Bash(git:*)" "..."    # add one specific pattern
roba --deny-tool WebFetch "..."          # block one (deny wins)
roba --add-dir ../shared "..."           # widen file-tool scope to another dir
roba --full-auto "..."                   # bypass every check (sandbox only)
roba --show-permissions --profile review # preview the resolved set, then exit
```

`--add-dir` (repeatable) passes through to claude's `--add-dir`: claude's
file tools are cwd-scoped by default; each `--add-dir` adds one directory.

The read-only start does not regress. `--permission-mode` additionally
sets claude's own approval mode (`plan`, `acceptEdits`, ...), orthogonal
to the allow-list. Precedence across all layers: **CLI flag > `ROBA_*`
env > profile > built-in default**, and deny always wins over allow.

To give claude extra tools from an MCP server for one run, point it at
a server config file:

```bash
roba --mcp-config mcp.json "..."                  # add those servers' tools
roba --mcp-config mcp.json --strict-mcp-config .. # use ONLY those servers
```

`--mcp-config` (repeatable) passes through to claude's `--mcp-config`:
roba forwards the path, claude reads it. Not a roba MCP server -- it
wires per-run MCP servers into the `claude -p` call.

## Configuration: profiles & aliases

A `roba.toml` lets you stop retyping flags and define your own verbs.
Files are discovered by walking up from the cwd (plus
`~/.config/roba.toml`); closer-to-cwd wins per key.

- **Profiles** are named bundles of flag defaults: `--profile review`
  applies `[profile.review]`. A `default` profile auto-applies.
- **Aliases** are new verbs: `roba review 42` expands an
  `[alias.review]` prompt template (`${1}` / `${pr}` / `$(...)` shell
  substitution) plus default flags and dispatches like a normal call.
  Your domain knowledge lives in your aliases, not the binary.

The fully-commented [`roba-config.sample.toml`](roba-config.sample.toml)
documents every key with worked examples; `roba profile init` drops it
in your project. Inspect with `roba profile {list,show,active}` and
`roba alias {list,show}`.

## For agents & scripts

The contract: **stdout is the answer, stderr is everything else**, and
`--json` wraps every output in a versioned envelope.

```text
success: { "version": 1, "result": { ... }, "refusal": bool }    (stdout)
failure: { "version": 1, "error": { kind, message, exit_code, chain } }    (stderr)
```

The read-only management commands (`cost`, `history`, `doctor`,
`worktree list`, `show`) emit the same `{ "version": 1, "result": ... }`
shape (minus the ask-only `refusal`), so one parser handles every
`--json` output. Pin `version` and you've pinned the shape.

Reading the envelope:

| Fact | Detail |
|---|---|
| The answer | `.result.result` -- not `.result` (that's the whole object) |
| Metrics | nest under `.result`: `.result.duration_ms`, `.result.num_turns`, `.result.total_cost_usd` (serde rename of `cost_usd`); the top-level paths return `null` |
| Top level | only `version`, `refusal`, and `result` |
| Refusal | still exits `0` (the call succeeded) -- detect via the `refusal` field, not the exit code |
| Exit codes | `0` ok, `1` generic (incl. `--max-turns` / `--max-budget-usd` cap hits), `2` auth, `3` budget, `4` timeout; the error `kind` maps the same way |
| `see_also` | omitted when empty -- don't assume the key exists |

```bash
out=$(roba --json "..."); echo "$out" | jq -r '.result.result'
```

Worker flags:

| Flag | Does |
|---|---|
| `--json-schema PATH` | schema-validated model output; roba reads the file and inlines it (claude's flag wants inline JSON); surfaces under `.result.*`, no extra output flag needed |
| `--max-turns N`, `--max-budget-usd USD` | rails for unattended loops; hitting a cap errors the run (exit `1`) |
| `--no-retry` | surface transient failures immediately; the caller owns retry |
| `--trace PATH` | the spawned session's events as JSONL -- watch a run in flight |
| `--fallback-model MODEL` | retry on a second model when the primary is overloaded |
| `--no-session-persistence` | run without writing a resumable session record |
| `--full-auto` | unsupervised editing worker; add `--worktree` for parallel same-repo workers (for orchestrator-owned branches use `git worktree add` + `-C <dir>` instead -- roba's `--worktree` is claude-managed, on a branch you won't PR from) |
| `roba doctor --json` | boundary checks as `{ checks: [{ name, status, message }], overall }`; exits `0` when no check fails, `1` otherwise |

Near the end of a turn the trace carries claude's `post_turn_summary`
system event (`status_category` + `status_detail`) -- a usable done /
what-happened signal, but it is claude's event passed through, not part
of roba's versioned ABI.

> [!NOTE]
> As of 2026-06-15 Anthropic meters programmatic usage (claude -p / Agent SDK) separately from interactive Claude. Every roba call is programmatic by construction, so all roba usage -- and the figures `roba cost` reports -- draws from that programmatic allotment, not your interactive limit.

## Bring your own skills and agents

roba is a pure mechanical wrapper -- no bundled skill or agent library.
Drop skills into `~/.claude/skills/` and agents into `~/.claude/agents/`;
Claude Code auto-discovers them. [joshrotenberg/agent-tools](https://github.com/joshrotenberg/agent-tools)
is one curated set if you want a starting point.

## Status

Published on crates.io. The CLI surface (flag names, exit codes, config
schema, `--json` envelope) is intended to be stable across `0.x`; the
library API (`roba::*`, for integration testing) may shift between minor
versions.

## License

MIT OR Apache-2.0
