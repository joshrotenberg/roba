# roba

**For humans** -- a binary convenience wrapper around `claude -p`:
composable input, structured output, profiles, history, cost
tracking. One invocation, one answer, done.

**For agents** in [Claude Code](https://github.com/anthropics/claude-code)
-- an optional bundled library of skills and subagents encodes
patterns for multi-task, multi-repo orchestration. See
[Skill + agent library](#skill--agent-library) below.

Built on [`claude-wrapper`](https://crates.io/crates/claude-wrapper).

```bash
$ roba "summarize the rust ownership model in 3 bullets"
   Rust's ownership model rests on three rules:

     • Each value has a single owner.
     • When the owner goes out of scope, the value is dropped.
     • Borrows are either many immutable or one mutable.

tokens 1.2k/450 . 2.0s . session abc12345
```

## Install

```bash
cargo install roba
```

`roba` shells out to the `claude` binary, so you need
[claude-code](https://github.com/anthropics/claude-code) installed
and authenticated on your PATH.

**Docs:** full reference, topic guides, and the skill/agent library at
[joshrotenberg.github.io/roba](https://joshrotenberg.github.io/roba/).

## What it does that `claude -p` doesn't

| | |
|---|---|
| **Prompt sources** | positional, `-p/--prompt TEXT` (explicit prompt -- escapes ambiguity against `-c`/`-w`), stdin (`-` or piped), `-f FILE`, `-e` ($EDITOR), `--prepend`/`--append` files, `--attach GLOB`, `--git-diff`/`--git-log`/`--git-status`, `--var K=V` template substitution |
| **Output shaping** | `--json`, `--quiet`, `--code [LANG]`, `-o/--out PATH` (write to file and stdout), `--trace PATH` (stream events to a JSONL file -- a stable observability handle for in-flight runs) |
| **Sessions** | `-c` continue most recent, `-c ID` (or `-c=ID`) resume a specific session, `-c ID --fork` branch it, `--pick` (interactive fuzzy chooser), `--agent NAME` pin a subagent, `roba history`, `roba last` |
| **Permissions** | `--readonly`, `--writable`, `--allow-tool`/`--deny-tool`, `--full-auto`, `--show-permissions` |
| **Profiles** | `--profile NAME` from `~/.config/roba.toml`, `roba profile {list,show,init,path}` |
| **Aliases** | `git`-style `[alias.NAME]` shortcuts in `roba.toml` -> `roba NAME [args]` expands a prompt template (with variable + shell substitution) plus default flags, `roba alias {list,show,path}` |
| **Skill library** | `roba skill {install,list,show}` and `roba agent {install,list,show}` install the bundled skills + orchestrator subagents into `~/.claude/` |
| **TTY UX** | termimad markdown render, indicatif spinner, dim metadata, colored refusal/error markers, `--plain` master kill-switch, `NO_COLOR` honored |
| **Scripting** | typed exit codes (auth=2, budget=3, timeout=4), clean stdout/stderr split, structured `--json` output, `--no-retry` for deterministic-on-failure runs |
| **Usage tracking** | `roba cost`, `roba cost --by-project`; dollar amounts from a bundled per-model rate table (`--rates-file PATH` / `ROBA_RATES_FILE` to override, `--no-dollars` for tokens only) |

## Quick examples

```bash
# Just ask
roba "what's the difference between Arc and Rc?"

# Compose: system preamble + question + appendix
roba --prepend system.md "review this design" --append context.md

# Look at files
roba --attach 'src/**/*.rs' "is the error handling consistent?"

# Continue the most recent session in this directory (-c takes an
# optional id, so pass the prompt explicitly with -p)
roba -c -p "now show me how to test the unsafe variant"

# Read-only review against the current git diff
roba --readonly --git-diff "is this safe to merge?"

# Pipe-friendly (no decoration, just the answer)
roba "what's 2+2" -q          # prints "4"
echo "summarize this" | roba  # stdin works
```

> [!NOTE]
> `-c` (continue) and `-w` (worktree) both take an *optional* value, so
> a space-separated word right after them is consumed as that value:
> `roba -c "follow up"` treats `follow up` as the session id, and
> `roba -w "do it"` treats `do it` as the worktree name. Pass the prompt
> explicitly with `-p` / `--prompt` to disambiguate (e.g.
> `roba -c -p "follow up"`, `roba -w mybranch -p "do it"`).

## Topics

The deep references live in dedicated pages, also on the [docs site](https://joshrotenberg.github.io/roba/):

- **Profiles** -- TOML bundles of flags you'd otherwise type every
  time. See [`docs/profiles.md`](docs/profiles.md) for the schema and
  worked examples.
- **Aliases** -- `git`-style `[alias.NAME]` shortcuts in `roba.toml`
  that expand to a prompt template plus default flags. See
  [`docs/aliases.md`](docs/aliases.md).
- **Scripting / agent ABI** -- pipe-clean output (stdout = answer,
  stderr = metadata), versioned `--json` envelope (`{"version": 1,
  "result": {...}}`), typed exit codes (auth=2, budget=3, timeout=4),
  `--no-retry` for deterministic-on-failure runs. See
  [`docs/scripting.md`](docs/scripting.md) for the full contract.
- **Permissions** -- safe by default (Read, Glob, Grep only).
  `--writable` adds Edit + Write; `--allow-tool`/`--deny-tool` narrow
  scope; `--full-auto` bypasses (sandbox use only).
  `--show-permissions` previews the resolved set with per-entry
  provenance. See [`docs/permissions.md`](docs/permissions.md) for the
  precedence model + worked examples.
- **roba vs `claude -p`** -- when to reach for each, with side-by-side
  examples: [`docs/vs-claude-p.md`](docs/vs-claude-p.md).
- **Use cases** -- patterns roba enables, seeded with multi-repo
  orchestration: [`docs/use-cases.md`](docs/use-cases.md). CI
  auto-review: [`docs/examples/github-actions/`](docs/examples/github-actions/).

## Skill + agent library

roba bundles an optional library of operational skills and
orchestrator subagents that codify one curated convention for
driving multi-task, multi-repo work from inside Claude Code. The
binary works fine without it. Install via `roba skill install` /
`roba agent install` if the patterns match your work.

Documentation lives with the library:
[`skills/README.md`](skills/README.md) (Layer 1 operational skills)
and [`agents/README.md`](agents/README.md) (Layer 2 runner +
orchestrator subagents). Each bundled item is also addressable by
URL -- `--url` on `show` prints the canonical doc URLs (rendered site
+ raw markdown), and `--urls` on `list` swaps the description column
for them, handy when an agent wants to `WebFetch` the latest source.

For the multi-repo orchestration pattern the library is built for,
see [`docs/use-cases.md`](docs/use-cases.md). A shell script, a cron
job, or CI can drive roba just as well via its `--json` envelope,
typed exit codes, and `--trace` observability handle.

## Streaming mode

`--stream` emits tokens live with inline tool-call indicators and a
`used: Tool xN` rollup at the end. It's a TTY-only progress indicator
-- never load-bearing on a pipe; the [scripting
surface](docs/scripting.md) (`--json`, exit codes, structured errors)
is the contract for non-TTY consumers.

## Status

0.1.x. The CLI surface (flag names, exit codes, config schema,
`--json` envelope) is intended to be stable across 0.1.x. The
library API (everything under `roba::*` for integration testing)
may shift between minor versions.

## License

MIT OR Apache-2.0
