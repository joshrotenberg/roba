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

```bash
cargo install roba
# or: brew install joshrotenberg/brew/roba
```

`roba` shells out to the `claude` binary, so you need
[claude-code](https://github.com/anthropics/claude-code) installed and
authenticated (or `ANTHROPIC_API_KEY` set) on your `PATH`.

**The full flag, env-var, and config reference lives in the binary:
`roba --help`.** This README is the overview.

## Why not just `claude -p`?

`claude -p` is the one-shot primitive: one prompt in, response to stdout,
exit -- no piping, no file context, no continuity. roba keeps that model
and adds:

- **Composable input.** Files (`-f`), stdin, an editor (`-e`),
  `--prepend` / `--append`, globbed files (`--attach`), git context
  (`--git-diff` / `--git-log` / `--git-status`), `{{KEY}}` vars (`--var`).
- **Pipe-clean output.** stdout is the answer; all metadata (cost footer,
  spinner, tool-call lines, refusal warnings) goes to stderr.
  `roba "..." | jq` sees a clean pipe.
- **On a TTY.** Rendered markdown, spinner, dim metadata, colored markers
  -- gone when the answer lands. `--plain` / `NO_COLOR` turns it off.
- **Session re-entry, without living in one.** `-c` continues the latest
  session here, `-c ID` resumes a specific one, `--fork` branches,
  `--pick` is a fuzzy chooser. `roba history` / `roba last` browse past
  runs; `roba history --worktree <NAME>` finds a runner's session to
  resume. `--session-id <uuid>` names a new session -- mint once, reuse
  with `-c=<uuid>` (scripted multi-turn; bare `claude -p --continue`
  no-ops in print mode).
- **Read-only inspection.** `roba worktree list` lists the repo's git
  worktrees (`--json` for scripts; list-only, never mutates).
  `roba show <SESSION_ID>` prints a stored session's result from its
  JSONL (`--json` envelope, optional `--metrics`) -- reconstructed, so
  `duration_ms` is null and `cost_usd` / `num_turns` are derived.
  `--wait [--timeout SECS]` polls until the run finishes (best-effort
  completion heuristic over the session log), then renders.
- **A stable ABI.** Typed exit codes, a versioned `--json` envelope, a
  clean stream split. Pin a contract instead of scraping prose. (See
  [For agents & scripts](#for-agents--scripts).)

For interactive, multi-turn work, use `claude` itself. For multiple
providers, a tool like [`llm`](https://llm.datasette.com/). roba is
Claude-only by design -- the Claude-Code-native integration (sessions,
permissions, history) is the point.

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

When something other than a human is calling, roba is a stable ABI over
`claude -p`:

- **stdout = the answer, stderr = everything else.** `roba "..." | jq`
  never sees decoration.
- **Versioned `--json` envelope.** Success:
  `{ "version": 1, "result": { ... }, "refusal": bool }`. Failure:
  `{ "version": 1, "error": { kind, message, exit_code, chain } }`.
  Pin `version` and you've pinned the shape. The read-only management
  commands (`cost`, `history`, `doctor`, `worktree list`) emit the same
  `{ "version": 1, "result": ... }` envelope (without the ask-only
  `refusal` flag), so one parser handles every `--json` output.
  Schema gotchas before you write `jq`: the answer is at
  **`.result.result`** (the whole object is at `.result` -- don't grab
  that); metrics nest under `.result`, so top-level `.duration_ms` /
  `.num_turns` / `.cost_usd` all return `null` -- read
  `.result.duration_ms` / `.result.num_turns` / `.result.total_cost_usd`;
  cost is serialized as `total_cost_usd` (a serde rename of `cost_usd`);
  and `version` and `refusal` are the only top-level fields besides
  `result`. The reliable extraction one-liner:
  `out=$(roba --json "..."); echo "$out" | jq -r '.result.result'`.
- **`roba doctor --json`** reports the boundary checks as
  `{ checks: [{ name, status, message }], overall }` (status is
  `ok`/`warn`/`fail`); it exits `0` when no check fails and `1` when any
  does -- the same code in human and `--json` modes.
- **Schema-validated output:** `--json-schema PATH` constrains the model's
  output to a JSON Schema (claude's own `--json-schema`). roba takes a
  path to a `.json` file, reads it, and inlines the contents (claude's
  flag wants inline JSON; a path is the ergonomic sugar) -- a missing or
  malformed file fails through the error envelope. The structured output
  surfaces under `.result.*` in the `--json` envelope. roba's default path
  already runs claude with JSON output, so just pair it with `--json`.
- **Typed exit codes:** `0` ok, `1` generic, `2` auth, `3` budget,
  `4` timeout. A refusal still exits `0` (the call succeeded) -- detect it
  via the top-level `refusal` field, not the exit code. On a failure the
  error `kind` maps to the code: `auth` -> 2, `budget` -> 3, `timeout` ->
  4, `history` / `other` -> 1. The error envelope's `see_also` (doc URLs)
  is omitted entirely when empty, so don't assume the key is present.
- **`--no-retry`** surfaces transient failures immediately (the caller
  decides whether to retry), and **`--trace PATH`** writes the spawned
  session's events as JSONL so you can observe a run in flight. An
  orchestrator tailing that trace will see a `system` event with
  `"subtype": "post_turn_summary"` (carrying `status_category` +
  `status_detail`) near the end of a turn -- a convenient
  done / what-happened signal. Caveat: that event is claude's own,
  passed through unchanged; it is NOT part of roba's versioned ABI, so
  don't depend on it the way you depend on the `--json` envelope or the
  exit codes.
- **Unattended dispatch** composes the primitives: `--full-auto` to fire
  a worker that edits files without supervision, plus `--worktree` when
  parallel same-repo workers must not share a branch. For the
  orchestrator-owns-the-branch case, prefer a plain `git worktree add`
  plus `-C <dir>` instead -- roba's `--worktree` creates a
  claude-managed worktree on a branch you won't PR from.
- **Unattended guardrails:** `--max-turns N` caps the agentic turn count
  and `--max-budget-usd USD` caps total spend -- the rails an unbounded
  loop needs. Hitting either cap errors the run (generic exit `1`).

  > [!NOTE]
  > As of 2026-06-15 Anthropic meters programmatic usage (claude -p / Agent SDK) separately from interactive Claude. Every roba call is programmatic by construction, so all roba usage -- and the figures `roba cost` reports -- draws from that programmatic allotment, not your interactive limit.

- **Resilience and statelessness:** `--fallback-model MODEL` retries on a
  second model when the primary is overloaded, and
  `--no-session-persistence` runs without writing a resumable session
  record (so the call leaves no trace in `roba history`). Both are thin
  pass-throughs to claude's own flags.

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
