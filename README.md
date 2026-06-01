# roba

A single-prompt CLI runner for [Claude Code](https://github.com/anthropics/claude-code).
Built on [`claude-wrapper`](https://crates.io/crates/claude-wrapper).

`roba` is what `claude -p` could be: opinionated defaults, composable
flags, rich on a TTY, scriptable on a pipe. One invocation, one
answer, done.

```bash
$ roba "summarize the rust ownership model in 3 bullets"
   Rust's ownership model rests on three rules:

     • Each value has a single owner.
     • When the owner goes out of scope, the value is dropped.
     • Borrows are either many immutable or one mutable.

tokens 1.2k/450 . cost $0.0192 . 2.0s . session abc12345
```

## Install

```bash
cargo install --path .   # from this repo, for now
```

`roba` shells out to the `claude` binary, so you need
[claude-code](https://github.com/anthropics/claude-code) installed
and authenticated on your PATH.

## What it does that `claude -p` doesn't

| | |
|---|---|
| **Prompt sources** | positional, stdin (`-` or piped), `-f FILE`, `-e` ($EDITOR), `--prepend`/`--append` files, `--attach GLOB`, `--git-diff`/`--git-log`/`--git-status`, `--var K=V` template substitution |
| **Output shaping** | `--json`, `--quiet`, `--code [LANG]`, `-o/--out PATH` (write to file and stdout) |
| **Sessions** | `-c` continue most recent, `--resume ID`, `--fork`, `--pick` (interactive fuzzy chooser), `roba history`, `roba last` |
| **Permissions** | `--readonly`, `--full-auto` presets |
| **Profiles** | `--profile NAME` from `~/.config/roba.toml`, `roba profile {list,show,init,path}` |
| **TTY UX** | termimad markdown render, indicatif spinner, dim metadata, colored refusal/error markers, `--plain` master kill-switch, `NO_COLOR` honored |
| **Scripting** | typed exit codes (auth=2, budget=3, timeout=4), clean stdout/stderr split, structured `--json` output |
| **Usage tracking** | `roba cost`, `roba cost --by-project` |

Streaming mode (`--stream`) emits tokens live with inline tool-call
indicators and a `used: Tool xN` rollup at the end. It's a TTY-only
progress indicator -- never load-bearing on a pipe, and the scripting
surface (`--json`, exit codes, structured errors) is the contract for
non-TTY consumers.

## Quick examples

```bash
# Just ask
roba "what's the difference between Arc and Rc?"

# Compose: system preamble + question + appendix
roba --prepend system.md "review this design" --append context.md

# Code extraction (handy in scripts)
roba "write a python one-liner that reverses a string" --code python

# Look at files
roba --attach 'src/**/*.rs' "is the error handling consistent?"

# Continue the previous session
roba -c "now show me how to test the unsafe variant"

# Fuzzy-pick a recent session to resume
roba --pick "follow up to whatever I select"

# Save the structured record (also echoes to stdout; redirect to suppress)
roba "explain quicksort" --out out.json

# Read-only review against current git diff
roba --readonly --git-diff "is this safe to merge?"

# Named profile (combo of the above)
roba --profile review "look at this change"

# Pipe-friendly (no decoration, just the answer)
roba "what's 2+2" -q          # prints "4"
echo "summarize this" | roba  # stdin works
```

## Profiles

A profile is a TOML alias for a bunch of flags you'd otherwise type
every time. See [`docs/profiles.md`](docs/profiles.md) for the schema
and worked examples.

```bash
roba profile init             # drops a starter roba.toml
roba profile list             # names defined
roba profile show review      # the TOML for one profile
```

## Output discipline

- **stdout** = the answer. Pipe-safe.
- **stderr** = metadata (cost footer, tool calls, refusal warnings,
  spinner). Visible to humans, invisible to scripts that don't
  capture it.
- Auto-detect: rich on a TTY, plain on a pipe. `--plain` is the
  manual override. `NO_COLOR=1` honored.

So `roba "foo" | jq` always sees clean stdout, even with the
spinner / footer / tool calls humming on stderr.

### JSON error envelope

When you pass `--json` and a runtime error happens, roba prints a
structured envelope to **stderr** (stdout stays empty) and exits
with the existing typed code. Pipe consumers parse it the same way
they parse the success record.

```json
{
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

Without `--json`, the error path is unchanged: a styled
`error: ...` line on stderr. Clap parse errors (mistyped flags,
missing values) are emitted by clap itself before roba sees them
and stay as plain stderr regardless of `--json`.

## Permissions

Safe by default. claude can use `Read`, `Glob`, and `Grep` but
nothing else unless you say so:

```bash
roba "explain this"              # readonly default
roba "..." --writable            # add Edit + Write
roba "..." --allow-tool "Bash(git status)"   # add one specific pattern
roba "..." --deny-tool WebFetch  # block a specific tool
roba "..." --full-auto           # bypass every check (sandbox only)
```

Same knobs work as profile fields (`writable = true`,
`allow_tool = [...]`, etc.) so you can codify a project's policy
in `roba.toml` once and not think about it again.

### Precedence

When the same permission knob is set in more than one place, the
highest layer wins:

| Layer | Example |
|---|---|
| CLI flag | `--writable`, `--allow-tool Edit` |
| Env var | `ROBA_WRITABLE=1`, `ROBA_ALLOW_TOOL=Edit,Write` |
| Active profile overlay | `[profile.NAME] writable = true` |
| Top-level `roba.toml` | `writable = true` at the file's top level |
| Built-in default | read-only: `Read`, `Glob`, `Grep` only |

`--writable` and `--full-auto` are mutually exclusive with the
default (they're presence flags that flip a bool). The highest
layer that sets one wins. `--full-auto` beats `--writable`
because `apply_permissions` short-circuits on `full_auto` before
inspecting `writable`.

`--readonly` is the explicit name for the built-in default. It is
a no-op marker, **not** an active suppressor: passing `--readonly`
on the CLI does not cancel a `writable = true` or
`full_auto = true` coming from a profile or env var. To enforce
read-only behavior when a profile turns on writable, pass
`--no-default-profile` (skips the auto-applied profile) or unset
`ROBA_WRITABLE` / `ROBA_FULL_AUTO` for the call. (Tracked as a
known gap; see #52.)

`allow_tool` and `deny_tool` lists **accumulate across layers**.
Across `roba.toml` files, closer-to-cwd entries concat on top of
farther-from-cwd entries, and the active profile's list concats
on top of the top-level list. The CLI (`--allow-tool` /
`--deny-tool`, repeatable) and env (`ROBA_ALLOW_TOOL` /
`ROBA_DENY_TOOL`, comma-separated) each **replace** the resolved
list when set, rather than concatenating with it.

When the same tool ends up in both the allow list and the deny
list, **deny wins**. roba passes both lists through to claude
unchanged; claude is the final arbiter.

#### Worked example

Profile in `roba.toml`:

```toml
[profile.default]
writable = true
allow_tool = ["Bash(git status)"]
```

| Invocation | Resolved permissions |
|---|---|
| `roba "..."` | writable (Edit, Write) + `Bash(git status)` (auto-applied profile) |
| `roba --full-auto "..."` | full-auto bypasses everything; profile's writable + allow_tool ignored |
| `roba --no-default-profile "..."` | read-only default (Read, Glob, Grep); profile skipped |
| `roba --readonly "..."` | **still writable** -- `--readonly` doesn't suppress profile writable; use `--no-default-profile` instead |
| `roba --allow-tool Edit "..."` | read-only base + `Edit` only; profile's `allow_tool` list is replaced, but `writable = true` from the profile still applies (so Edit, Write are also in) |

## Status

Early. The CLI surface (flag names, exit codes, config schema) is
the part we'd like to keep stable across 0.1.x. The library API
(everything under `roba::*` for integration testing) may shift
freely.

## License

MIT OR Apache-2.0
