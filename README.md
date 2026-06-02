# roba

A single-prompt CLI runner for [Claude Code](https://github.com/anthropics/claude-code).
Built on [`claude-wrapper`](https://crates.io/crates/claude-wrapper).

A wrapper around `claude -p` with opinionated defaults, composable
input, structured output, a bundled skill / subagent library, and
user-defined aliases. One invocation, one answer, done.

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

To get the bundled skill + subagent library set up under
`~/.claude/`:

```bash
roba skill install
roba agent install
```

After that, an interactive Claude Code session can drive multi-task
work via the orchestrator agent:

```bash
claude --agent=roba-orchestrator
# "work the backlog in foo and bar"
```

Before invoking the orchestrator, the runner needs `gh` and `git` in
the Claude Code sandbox allowlist. The pre-flight skill will auto-heal
for known-safe dev tools (cargo, npm, pip, go, etc.) on first
encounter, but `gh` and `git` are universal and worth pre-configuring
(in `.claude/settings.local.json` under `permissions.allow`:
`"Bash(gh:*)"`, `"Bash(git:*)"`). See
[`skills/sandbox-preflight/SKILL.md`](skills/sandbox-preflight/SKILL.md)
for the policy.

See [Skill + agent library](#skill--agent-library) for details.

## What it does that `claude -p` doesn't

| | |
|---|---|
| **Prompt sources** | positional, `-p/--prompt TEXT` (explicit prompt -- escapes ambiguity against `-c`/`-w`), stdin (`-` or piped), `-f FILE`, `-e` ($EDITOR), `--prepend`/`--append` files, `--attach GLOB`, `--git-diff`/`--git-log`/`--git-status`, `--var K=V` template substitution |
| **Output shaping** | `--json`, `--quiet`, `--code [LANG]`, `-o/--out PATH` (write to file and stdout), `--trace PATH` (stream events to a JSONL file -- a stable observability handle for in-flight runs) |
| **Sessions** | `-c` continue most recent, `-c ID` (or `-c=ID`) resume a specific session, `-c ID --fork` branch it, `--pick` (interactive fuzzy chooser), `--agent NAME` pin a subagent, `roba history`, `roba last` |
| **Permissions** | `--readonly`, `--full-auto` presets |
| **Profiles** | `--profile NAME` from `~/.config/roba.toml`, `roba profile {list,show,init,path}` |
| **Aliases** | `git`-style `[alias.NAME]` shortcuts in `roba.toml` -> `roba NAME [args]` expands a prompt template (with variable + shell substitution) plus default flags, `roba alias {list,show,path}` |
| **Skill library** | `roba skill {install,list,show}` and `roba agent {install,list,show}` install the bundled skills + orchestrator subagents into `~/.claude/` |
| **TTY UX** | termimad markdown render, indicatif spinner, dim metadata, colored refusal/error markers, `--plain` master kill-switch, `NO_COLOR` honored |
| **Scripting** | typed exit codes (auth=2, budget=3, timeout=4), clean stdout/stderr split, structured `--json` output, `--no-retry` for deterministic-on-failure runs |
| **Usage tracking** | `roba cost`, `roba cost --by-project`; dollar amounts from a bundled per-model rate table (`--rates-file PATH` / `ROBA_RATES_FILE` to override, `--no-dollars` for tokens only) |

Streaming mode (`--stream`) emits tokens live with inline tool-call
indicators and a `used: Tool xN` rollup at the end. It's a TTY-only
progress indicator -- never load-bearing on a pipe, and the scripting
surface (`--json`, exit codes, structured errors) is the contract for
non-TTY consumers.

For a deeper side-by-side -- when to reach for `roba` vs plain
`claude -p`, with worked examples -- see
[`docs/vs-claude-p.md`](docs/vs-claude-p.md). For patterns like
multi-repo orchestration, see
[`docs/use-cases.md`](docs/use-cases.md). For CI auto-review
workflows, see
[`docs/examples/github-actions/`](docs/examples/github-actions/).

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

# Continue the most recent session in this directory.
# -c takes an optional id, so a bare word after it is read as the id;
# use -p to pass the prompt explicitly.
roba -c -p "now show me how to test the unsafe variant"

# Resume a specific session by id (space or `=` both work)
roba -c 7c3f9a21 "and the safe variant?"

# Fuzzy-pick a recent session to resume
roba --pick "follow up to whatever I select"

# Save the structured record (also echoes to stdout; redirect to suppress)
roba "explain quicksort" --out out.json

# Capture the spawned session's event stream for observability
# (tail it live, or read it after to diagnose a run)
roba --trace /tmp/run.jsonl "refactor the parser"

# Read-only review against current git diff
roba --readonly --git-diff "is this safe to merge?"

# Named profile (combo of the above)
roba --profile review "look at this change"

# Pipe-friendly (no decoration, just the answer)
roba "what's 2+2" -q          # prints "4"
echo "summarize this" | roba  # stdin works
```

> [!NOTE]
> `-c` (continue) and `-w` (worktree) both take an *optional* value, so
> a space-separated word right after them is consumed as that value:
> `roba -c "follow up"` treats `follow up` as the session id, and
> `roba -w "do it"` treats `do it` as the worktree name. For
> invocations where the positional prompt is ambiguous against one of
> these flags, pass the prompt explicitly with `-p` / `--prompt`
> (e.g. `roba -c -p "follow up"`, `roba -w mybranch -p "do it"`).

## Profiles

A profile is a TOML alias for a bunch of flags you'd otherwise type
every time. See [`docs/profiles.md`](docs/profiles.md) for the schema
and worked examples.

```bash
roba profile init             # drops a starter roba.toml
roba profile list             # names defined
roba profile show review      # the TOML for one profile
```

## Aliases

Aliases are `git`-style shortcuts defined in `roba.toml`. They keep
roba's built-in surface generic -- domain shortcuts (github, your
workflow) live in config, not in the binary. Each `[alias.NAME]`
expands `roba NAME [args]` into a prompt template plus default flags,
and can pin a subagent.

```toml
[alias.review]
description = "Review a PR by number"
agent = "reviewer"                 # optional: pins --agent
template = """
Review PR #${pr} in this repo.

$(gh pr diff ${pr})
"""
flags = ["--readonly"]
args = ["pr"]                       # positional 1 -> ${pr}

[alias.commit-msg]
description = "Conventional-commit message from the staged diff"
template = "Write a conventional-commit message for:\n\n$(git diff --staged)"
flags = ["--quiet"]
```

```bash
roba review 42                 # expand [alias.review] with pr=42, dispatch
roba commit-msg               # zero-arg alias
roba review 42 --full-auto    # CLI flags merge with (and override) the alias's

roba alias list               # aliases in the merged pool, with descriptions
roba alias show review        # the TOML + an expansion preview
roba alias path               # which files contribute aliases
```

Templates support positional (`${1}`, `${@}`) and named (`${pr}` via
the `args` schema) substitution, `$$` for a literal `$`, and
`$(command)` shell substitution. An alias with no `template` is a
flag-shortcut: the user's args become the prompt verbatim.

**Security:** `$(...)` runs in your shell with your permissions.
Aliases are your own config, so this is intentional -- it is not a
sandbox. See [`docs/profiles.md`](docs/profiles.md#aliases) for the
full schema, lookup order, and caveats.

## Skill + agent library

roba ships a starter set of operational skills and orchestrator
subagents. Install them into `~/.claude/` so any Claude Code session
auto-discovers them:

```bash
roba skill install            # -> ~/.claude/skills/
roba agent install            # -> ~/.claude/agents/
roba skill list               # what's bundled, with descriptions
roba skill show draft-pr-first  # print a skill's SKILL.md body
```

`install` flags: `--to PATH` (custom destination), `--dry-run`
(preview), `--force` (overwrite existing), `--skip` (leave existing in
place). The agent commands mirror the skill ones. The bundle is
embedded in the binary at build time -- no network fetch.

## Output discipline

- **stdout** = the answer. Pipe-safe.
- **stderr** = metadata (cost footer, tool calls, refusal warnings,
  spinner). Visible to humans, invisible to scripts that don't
  capture it.
- Auto-detect: rich on a TTY, plain on a pipe. `--plain` is the
  manual override. `NO_COLOR=1` honored.

So `roba "foo" | jq` always sees clean stdout, even with the
spinner / footer / tool calls humming on stderr.

### Versioned JSON output

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

### Failure modes

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

## Permissions

Safe by default. claude can use `Read`, `Glob`, and `Grep` but
nothing else unless you say so:

```bash
roba "explain this"              # readonly default
roba "..." --writable            # add Edit + Write
roba "..." --allow-tool "Bash(git status)"   # add one specific pattern
roba "..." --deny-tool WebFetch  # block a specific tool
roba "..." --full-auto           # bypass every check (sandbox only)
roba --profile review --show-permissions   # preview the resolved set, then exit
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

`--readonly`, `--writable`, and `--full-auto` are mutually
exclusive across layers (they're presence flags that flip a bool).
The highest layer that sets one wins, and a higher-priority flag
**suppresses** the lower-privilege ones from lower layers:

- `--readonly` suppresses lower-layer `writable = true` and
  `full_auto = true`.
- `--writable` suppresses lower-layer `full_auto = true`.

`--full-auto` beats `--writable` because `apply_permissions`
short-circuits on `full_auto` before inspecting `writable`.

`--readonly` is the explicit name for the built-in default, but it
is now an **active suppressor**: passing `--readonly` on the CLI
cancels a `writable = true` or `full_auto = true` coming from a
profile or env var, so the call stays read-only.

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

### Previewing the resolved set

Because a lower-layer profile can quietly add `writable = true` or
extra allow-list entries, it's easy to fire a prompt assuming a
permission set you didn't actually get. `--show-permissions`
resolves every layer (the same flow a real run uses), prints the
effective allow/deny lists with per-entry provenance, and exits 0
**without calling claude**:

```text
$ roba --profile review --show-permissions
allow:
  Read       [default]
  Glob       [default]
  Grep       [default]
  Edit       [profile.review]
  Write      [profile.review]
deny:
  Bash(rm *) [profile.review]
```

Each tag shows the winning layer: `[default]` for the built-in safe
trio, or `[CLI]` / `[env]` / `[profile.NAME]` / `[config]` for
anything a higher layer contributed. Under `--full-auto` the output
collapses to a single line (`all tools allowed (--full-auto from
...)`), since the resolution is "everything." The preview goes to
stderr, so stdout stays clean.

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
| `roba --readonly "..."` | read-only -- `--readonly` suppresses the profile's `writable = true` (and any lower-layer `full_auto`) |
| `roba --allow-tool Edit "..."` | read-only base + `Edit` only; profile's `allow_tool` list is replaced, but `writable = true` from the profile still applies (so Edit, Write are also in) |

## Status

0.1.x. The CLI surface (flag names, exit codes, config schema,
`--json` envelope) is intended to be stable across 0.1.x. The
library API (everything under `roba::*` for integration testing)
may shift between minor versions.

## License

MIT OR Apache-2.0
