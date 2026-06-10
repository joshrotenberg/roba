# roba

[![Crates.io](https://img.shields.io/crates/v/roba.svg)](https://crates.io/crates/roba)
[![Documentation](https://docs.rs/roba/badge.svg)](https://docs.rs/roba)
[![CI](https://github.com/joshrotenberg/roba/actions/workflows/ci.yml/badge.svg)](https://github.com/joshrotenberg/roba/actions/workflows/ci.yml)
[![Downloads](https://img.shields.io/crates/d/roba.svg)](https://crates.io/crates/roba)
[![License](https://img.shields.io/crates/l/roba.svg)](#license)

A single-prompt CLI runner built on top of `claude -p`. One invocation,
one answer, done -- but with composable input, pipe-clean output,
sessions you can re-enter without living in them, and a stable scripting
ABI for agents and scripts.

- **For humans** -- what `claude -p` could be: build the prompt from
  files / stdin / git context, get rendered markdown on a TTY, save
  flag bundles as profiles, browse history, track cost.
- **For agents** in [Claude Code](https://github.com/anthropics/claude-code)
  -- a stable contract: stdout is the answer, stderr is metadata, a
  versioned `--json` envelope, typed exit codes, and `--trace` for
  observing a run in flight.

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
`roba --help`.** This README is the conceptual tour.

## Why not just `claude -p`?

`claude -p` is the one-shot primitive: one prompt in, the response to
stdout, exit. It's perfect for a quick "just answer this string" with no
piping, no file context, no continuity. roba doesn't replace it -- it
gives that same one-invocation-one-answer model a richer surface, for
when you want any of:

- **Composable input.** Build the prompt from more than a string: a
  file (`-f`), piped stdin, an editor buffer (`-e`), `--prepend` /
  `--append` files, glob-embedded files (`--attach`), git context
  (`--git-diff` / `--git-log` / `--git-status`), and `{{KEY}}` template
  vars (`--var`).
- **Pipe-clean output.** stdout is the answer and *only* the answer;
  every bit of metadata (cost footer, spinner, tool-call lines, refusal
  warnings) goes to stderr. `roba "..." | jq` always sees a clean pipe.
- **Rich on a TTY.** Rendered markdown, a spinner, dim metadata, colored
  markers -- a transient UI that exists while roba works and evaporates
  when the answer lands. `--plain` (or `NO_COLOR`) turns it all off.
- **Session re-entry without living in a session.** `-c` continues the
  most recent session in the directory, `-c ID` resumes a specific one,
  `--fork` branches it, `--pick` is a fuzzy chooser; `roba history` /
  `roba last` browse past runs. You dip back into a thread without
  opening the TUI. `--session-id <uuid>` assigns a caller-chosen id to a
  *new* session -- mint it once, then `-c=<uuid>` on later turns (the
  reliable scripted-multi-turn pattern).
- **Read-only inspection.** `roba worktree list` enumerates the git
  worktrees for the repo (all of them -- a superset of the ones claude's
  `--worktree` creates), with `--json` for scripts. Lists only; roba
  never creates, prunes, or removes worktrees.
- **A real ABI.** Typed exit codes, a versioned `--json` envelope, and a
  clean stream split -- so a script or agent can pin a contract instead
  of scraping prose. (See [For agents & scripts](#for-agents--scripts).)

Same one-shot model, a citizen of the pipe. For *interactive,
multi-turn* work, reach for `claude` itself; for *multiple providers*,
a tool like [`llm`](https://llm.datasette.com/). roba is Claude-only by
design -- the Claude-Code-native integration (sessions, permissions,
history) is the point.

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

`--add-dir` (repeatable) is a thin pass-through to claude's own
`--add-dir` -- claude's file tools are scoped to the cwd by default, and
each `--add-dir` grants access to one more directory.

This came from a real "oops" -- a streaming run quietly let claude
create a git branch when the user just wanted a chat. The default
doesn't regress. `--permission-mode` additionally sets claude's own
approval mode (`plan`, `acceptEdits`, ...), orthogonal to the
allow-list. Precedence across all layers: **CLI flag > `ROBA_*` env >
profile > built-in default**, and deny always wins over allow.

To give claude extra tools from an MCP server for one run, point it at
a server config file:

```bash
roba --mcp-config mcp.json "..."                  # add those servers' tools
roba --mcp-config mcp.json --strict-mcp-config .. # use ONLY those servers
```

`--mcp-config` (repeatable) is a thin pass-through to claude's own
`--mcp-config` -- roba forwards the path and claude reads the file.
This is *not* a roba MCP server; it just wires per-run MCP servers into
the `claude -p` call.

## Configuration: profiles & aliases

A `roba.toml` lets you stop retyping flags and define your own verbs.
Files are discovered by walking up from the cwd (plus
`~/.config/roba.toml`); closer-to-cwd wins per key.

- **Profiles** are named bundles of flag defaults: `--profile review`
  applies `[profile.review]`. A `default` profile auto-applies.
- **Aliases** are new verbs: `roba review 42` expands an
  `[alias.review]` prompt template (with `${1}` / `${pr}` / `$(...)`
  shell substitution) plus default flags, and dispatches like a normal
  call. This is how you keep roba's built-in surface generic -- your
  domain knowledge lives in *your* aliases, not the binary.

The fully-commented [`roba-config.sample.toml`](roba-config.sample.toml)
documents every key with worked examples; `roba profile init` drops it
in your project. Inspect with `roba profile {list,show,active}` and
`roba alias {list,show}`.

## For agents & scripts

When something other than a human is calling, roba is a much cleaner ABI
than `claude -p`:

- **stdout = the answer, stderr = everything else.** `roba "..." | jq`
  never sees decoration.
- **Versioned `--json` envelope.** Success:
  `{ "version": 1, "result": { ... }, "refusal": bool }`. Failure:
  `{ "version": 1, "error": { kind, message, exit_code, chain } }`.
  Pin `version` and you've pinned the shape.
- **Schema-validated output:** `--json-schema PATH` constrains the model's
  output to a JSON Schema (claude's own `--json-schema`). roba takes a
  path to a `.json` file, reads it, and inlines the contents (claude's
  flag wants inline JSON; a path is the ergonomic sugar) -- a missing or
  malformed file fails through the error envelope. The structured output
  surfaces under `.result.*` in the `--json` envelope. roba's default path
  already runs claude with JSON output, so just pair it with `--json`.
- **Typed exit codes:** `0` ok, `1` generic, `2` auth, `3` budget,
  `4` timeout.
- **`--no-retry`** surfaces transient failures immediately (the caller
  decides whether to retry), and **`--trace PATH`** writes the spawned
  session's events as JSONL so you can observe a run in flight.
- **Unattended dispatch** composes the primitives: `--full-auto` to fire
  a worker that edits files without supervision, plus `--worktree` when
  parallel same-repo workers must not share a branch.
- **Unattended guardrails:** `--max-turns N` caps the agentic turn count
  and `--max-budget-usd USD` caps total spend -- the rails an unbounded
  loop needs. Hitting either cap errors the run (generic exit `1`).
- **Resilience and statelessness:** `--fallback-model MODEL` retries on a
  second model when the primary is overloaded, and
  `--no-session-persistence` runs without writing a resumable session
  record (so the call leaves no trace in `roba history`). Both are thin
  pass-throughs to claude's own flags.

For an agent that *invokes* roba, [`skills/use-roba/SKILL.md`](skills/use-roba/SKILL.md)
documents this contract in agent-readable form -- copy it to
`~/.claude/skills/use-roba/SKILL.md`.

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
