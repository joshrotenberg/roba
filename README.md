# cwr

A single-prompt CLI runner for [Claude Code](https://github.com/anthropics/claude-code).
Built on [`claude-wrapper`](https://crates.io/crates/claude-wrapper).

`cwr` is what `claude -p` could be: opinionated defaults, composable
flags, rich on a TTY, scriptable on a pipe. One invocation, one
answer, done.

```bash
$ cwr "summarize the rust ownership model in 3 bullets"
   Rust's ownership model rests on three rules:

     • Each value has a single owner.
     • When the owner goes out of scope, the value is dropped.
     • Borrows are either many immutable or one mutable.

tokens 1.2k/450 . cost $0.0192 . 2.0s . session abc12345
```

## Install

```bash
cargo install --path crates/cwr   # from this repo, for now
```

`cwr` shells out to the `claude` binary, so you need
[claude-code](https://github.com/anthropics/claude-code) installed
and authenticated on your PATH.

## What it does that `claude -p` doesn't

| | |
|---|---|
| **Prompt sources** | positional, stdin (`-` or piped), `-f FILE`, `-e` ($EDITOR), `--prepend`/`--append` files, `--attach GLOB`, `--git-diff`/`--git-log`/`--git-status`, `--var K=V` template substitution |
| **Output shaping** | `--json`, `--quiet`, `--code [LANG]`, `--head N`/`--tail N`, `--save PATH`, `--tee PATH` |
| **Sessions** | `-c` continue most recent, `--resume ID`, `--fork`, `--pick` (interactive fuzzy chooser), `cwr history`, `cwr last` |
| **Permissions** | `--readonly`, `--full-auto` presets |
| **Profiles** | `--profile NAME` from `~/.config/cwr/profiles.toml`, `cwr profile {list,show,init,path}` |
| **TTY UX** | termimad markdown render, indicatif spinner, dim metadata, colored refusal/error markers, `--plain` master kill-switch, `NO_COLOR` honored |
| **Scripting** | typed exit codes (auth=2, budget=3, timeout=4), clean stdout/stderr split, structured `--json` output |
| **Usage tracking** | `cwr cost`, `cwr cost --by-project` |

Streaming mode (`--stream`) emits tokens live with inline tool-call
indicators and a `used: Tool xN` rollup at the end.

## Quick examples

```bash
# Just ask
cwr "what's the difference between Arc and Rc?"

# Compose: system preamble + question + appendix
cwr --prepend system.md "review this design" --append context.md

# Code extraction (handy in scripts)
cwr "write a python one-liner that reverses a string" --code python

# Look at files
cwr --attach 'src/**/*.rs' "is the error handling consistent?"

# Continue the previous session
cwr -c "now show me how to test the unsafe variant"

# Fuzzy-pick a recent session to resume
cwr --pick "follow up to whatever I select"

# Save the structured record
cwr "explain quicksort" --save out.json

# Read-only review against current git diff
cwr --readonly --git-diff "is this safe to merge?"

# Named profile (combo of the above)
cwr --profile review "look at this change"

# Pipe-friendly (no decoration, just the answer)
cwr "what's 2+2" -q          # prints "4"
echo "summarize this" | cwr  # stdin works
```

## Profiles

A profile is a TOML alias for a bunch of flags you'd otherwise type
every time. See [`docs/cwr/profiles.md`](../../docs/cwr/profiles.md)
for the schema and worked examples.

```bash
cwr profile init             # drops a starter profiles.toml
cwr profile list             # names defined
cwr profile show review      # the TOML for one profile
```

## Output discipline

- **stdout** = the answer. Pipe-safe.
- **stderr** = metadata (cost footer, tool calls, refusal warnings,
  spinner). Visible to humans, invisible to scripts that don't
  capture it.
- Auto-detect: rich on a TTY, plain on a pipe. `--plain` is the
  manual override. `NO_COLOR=1` honored.

So `cwr "foo" | jq` always sees clean stdout, even with the
spinner / footer / tool calls humming on stderr.

## Status

Early. The CLI surface (flag names, exit codes, config schema) is
the part we'd like to keep stable across 0.1.x. The library API
(everything under `cwr::*` for integration testing) may shift
freely.

## License

MIT OR Apache-2.0
