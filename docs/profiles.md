# roba profiles

A profile is a named bundle of `roba` flags you'd otherwise type every
time. Nothing magical -- the profile only fills in fields you didn't
pass on the command line. CLI flags always win.

```bash
roba --profile review "what changed and is it safe to merge?"
```

## Where profiles live

`roba` builds a merged pool from a chain of `roba.toml` files. Later
sources override earlier ones; when the same profile name appears in
more than one file, fields merge per-key (closer-to-cwd file wins on
scalars, lists concat, vars merge per-key).

1. **User-level:** `$XDG_CONFIG_HOME/roba.toml` or
   `~/.config/roba.toml`. Your global baseline.
2. **Project chain:** every `roba.toml` walking up from the current
   directory; stops at the git root if there is one, else the
   filesystem root. Closer-to-cwd files override farther ones on
   the same key.

Top-level keys in a `roba.toml` are project-wide defaults: every
roba call in the dir inherits them. `[profile.NAME]` tables sit
above those defaults and activate via `--profile NAME` (or
`ROBA_PROFILE`, or by being named `default`).

Missing files are fine. `roba profile path` prints what's currently
in the chain so you can see which file would supply which name.

## Env-var overrides

Every config knob is also settable via an env var matching the CLI
long-form, uppercased with `-` -> `_` and prefixed `ROBA_`. Sits
between CLI flags (top priority) and the file pool, so you can
override one knob for a single shell session without editing a file:

```bash
ROBA_WRITABLE=1 roba "rename the foo variable to bar"
ROBA_MODEL=opus roba "review this design"
ROBA_GIT_LOG=10 roba "summarize recent work"
ROBA_ALLOW_TOOL="Bash(git status),Bash(git diff)" roba "..."
ROBA_VAR_TICKET=ABC-123 roba --profile commit-msg "..."
```

Value rules:

- **String** (e.g. `ROBA_MODEL`): any non-empty value.
- **Bool** (e.g. `ROBA_WRITABLE`): truthy `1`/`true`/`yes`/`on`
  (case-insensitive) enables. Other values are ignored -- the env
  layer can only enable a bool, never disable a file-set one.
- **Number** (e.g. `ROBA_GIT_LOG=5`): parsed; invalid values
  silently ignored.
- **List** (e.g. `ROBA_ALLOW_TOOL`, `ROBA_PREPEND`): comma-separated,
  whitespace trimmed, empty entries dropped.
- **Vars**: one env var per key, `ROBA_VAR_<KEY>=<value>`.

Like files, env-var overrides only fill fields the user didn't set
on the CLI. `roba --writable=false ...` -- well, there's no such
flag, since `--writable` is presence-flagged -- but
`--allow-tool Edit` on the CLI fully replaces any `ROBA_ALLOW_TOOL`
list, same as it overrides a profile's `allow_tool` list.

The one setting that *does* have an explicit kill switch is
`continue`: pass `--fresh` to force a new session even when a profile
or env var sets `continue = true` (or pins a specific session id).
Pair it with `ROBA_CONTINUE=1` enabled in a project default to opt out
for one call without re-typing config.

## Auto-apply and explicit invocation

When `roba` runs the default ask path:

1. `roba --profile NAME ...` -> apply `NAME`. Error if it doesn't exist.
2. `roba --no-default-profile ...` -> skip auto-apply for this call.
3. `ROBA_PROFILE=NAME roba ...` (env) -> apply `NAME` as the default.
4. If a `default` profile exists in the pool -> apply it silently.
5. Otherwise no profile.

Explicit `--profile` always wins. `--no-default-profile` bypasses
both steps 3 and 4 in one go.

To see what would auto-apply without making a call:

```bash
roba profile active
```

## Schema

Every field is optional -- specify only what you want a profile to
override.

| Field | Type | Maps to | Notes |
|---|---|---|---|
| `prepend` | `[path]` | `--prepend PATH` (repeatable) | `~/` is expanded |
| `append` | `[path]` | `--append PATH` (repeatable) | `~/` is expanded |
| `attach` | `[glob]` | `--attach GLOB` (repeatable) | |
| `git_diff` | `bool` | `--git-diff` | |
| `git_log` | `int` | `--git-log N` | |
| `git_status` | `bool` | `--git-status` | |
| `readonly` | `bool` | `--readonly` | Explicit form of the default; suppresses lower-layer `writable` / `full_auto` |
| `writable` | `bool` | `--writable` | Adds Edit + Write to the allow list |
| `full_auto` | `bool` | `--full-auto` | Bypass all permission checks |
| `allow_tool` | `[string]` | `--allow-tool TOOL` (repeatable) | Adds to the allow list |
| `deny_tool` | `[string]` | `--deny-tool TOOL` (repeatable) | Deny patterns; useful with `full_auto` to keep some teeth |
| `continue` | `bool` or `string` | `-c` / `-c=ID` | `true` continues the most recent session in the directory; a string resumes that specific session id (e.g. `continue = "7c3f9a21"`); `false` stays fresh. CLI `-c` / `-c=ID` overrides it |
| `vars` | `{ key = "value" }` | `--var KEY=VALUE` (repeatable) | CLI keys override profile keys |
| `model` | `string` | `--model MODEL` | Alias (`sonnet`/`opus`/`haiku`) or full id (`claude-sonnet-4-6`) |
| `agent` | `string` | `--agent NAME` | Pin a claude-code subagent by name; must exist in `.claude/agents/NAME.md` (or be auto-discovered per claude's lookup) |
| `stream` | `bool` | `--stream` | Stream tokens as they arrive |
| `show_thinking` | `bool` | `--show-thinking` | Render extended-thinking blocks live on stderr. Only takes effect with `--stream`; ignored otherwise |
| `echo` | `bool` | `--echo` | Print resolved prompt before the response |
| `plain` | `bool` | `--plain` | No rendering, color, or spinner |
| `quiet` | `bool` | `-q` / `--quiet` | Answer only, no metadata |
| `json` | `bool` | `--json` | Structured result as JSON on stdout |
| `editor_history` | `int` | `--editor-history N` | With `-e`, pre-fill the editor with the last N assistant responses, separated by a scissors line. Default 1; 0 disables |
| `worktree` | `bool` or `string` | `-w` / `--worktree[=NAME]` | `true` runs every session in a fresh git worktree (claude generates the name); a string pins the worktree directory/branch (e.g. `worktree = "feature-x"`) |
| `no_retry` | `bool` | `--no-retry` | Disable wrapper-level auto-retry on transient failures; the failure surfaces immediately with its normal exit code |
| `trace` | `string` | `--trace PATH` | Write the spawned session's streaming events to PATH as JSONL (a stable observability handle); `~/` is expanded. Forces the streaming pipeline internally even without `--stream` |
| `rates_file` | `string` | `--rates-file PATH` | Override the bundled per-model rates table used for the footer dollar figure (same schema as `roba cost --rates-file`); `~/` is expanded. Also honored via `ROBA_RATES_FILE` |
| `no_dollars` | `bool` | `--no-dollars` | Omit the dollar figure from the per-call footer (tokens only); useful when the bundled rates are stale |

Unknown keys are rejected at parse time -- a typo errors fast instead
of being silently ignored.

## CLI overrides profile

Two rules:

1. If you pass a scalar flag on the CLI, it wins (e.g. `--git-log 7`
   beats a profile's `git_log = 3`).
2. If you pass a list/repeatable flag on the CLI, the CLI list
   *replaces* the profile list entirely -- they don't concatenate.

For `vars`, the same idea but per-key: CLI `--var NAME=foo` overrides
the profile's `NAME` and the rest of the profile's vars still apply.

## Precedence (full layer list)

When the same knob is set in more than one place, the highest
layer wins:

1. **CLI flag** (e.g. `--writable`, `--allow-tool Edit`)
2. **Env var** (e.g. `ROBA_WRITABLE=1`, `ROBA_ALLOW_TOOL=Edit,Write`)
3. **Active `[profile.NAME]` overlay** (selected explicitly,
   auto-applied via `ROBA_PROFILE`, or named `default`)
4. **Top-level keys** in any `roba.toml` (closer-to-cwd files
   win for scalars; lists and vars merge as described above)
5. **roba's built-in defaults** (read-only permissions, no
   composition, no streaming)

The env layer runs first, then the profile layer; both check
"did the CLI / a prior layer already set this?" before filling
in a value. So a CLI flag is never overridden by env or profile,
and an env var is never overridden by profile.

### Permissions precedence

For the `readonly` / `writable` / `full_auto` knobs, the highest
layer that sets one wins, and a higher-priority flag **suppresses**
the more-permissive ones from lower layers. `full_auto` also
short-circuits `writable` at apply time. So:

- profile `writable = true` + no CLI/env override -> writable
  (Edit, Write added)
- profile `writable = true` + CLI `--full-auto` -> full-auto
  (bypasses everything)
- profile `full_auto = true` + CLI `--writable` -> writable
  (CLI `--writable` suppresses the lower-layer `full_auto`)
- profile `writable = true` + CLI `--readonly` -> read-only
  (CLI `--readonly` suppresses the lower-layer `writable`)

`--readonly` is the explicit name for the default, and it is an
**active suppressor**: passing `--readonly` cancels a
`writable = true` or `full_auto = true` coming from a lower layer,
so the call stays read-only. It also pairs cleanly with permissive
list additions (`--readonly --allow-tool "Bash(git status)"`).
Symmetrically, `--writable` suppresses a lower-layer
`full_auto = true`. See #52.

For `allow_tool` and `deny_tool`, lists **accumulate across
layers**:

- Across `roba.toml` files, closer-to-cwd entries concat on top
  of farther-from-cwd entries.
- The active profile's list concats on top of the merged
  top-level list.
- The CLI (`--allow-tool` / `--deny-tool`, repeatable) and env
  (`ROBA_ALLOW_TOOL` / `ROBA_DENY_TOOL`, comma-separated)
  **replace** the resolved list when set -- they don't
  concatenate with the lower layer.

When the same tool ends up in both `allow_tool` and `deny_tool`
(across any combination of layers), **deny wins**. roba passes
both resolved lists through to claude unchanged; claude is the
final arbiter.

## Worked examples

Drop these into `~/.config/roba.toml` (or a project-local
`roba.toml`). They're starting points, not opinions -- adapt to
your habits.

### `default` -- per-project auto-apply

Drop this into your project's `roba.toml` and `roba "..."` in
that tree picks it up without `--profile`:

```toml
# /path/to/my-project/roba.toml
[profile.default]
readonly = true
continue = true
prepend = ["CLAUDE.md"]
```

What happens:

- `cd /path/to/my-project && roba "what does this do"` ->
  read-only tools, continues the most recent session, prepends
  your project's CLAUDE.md to every prompt.
- `cd ~ && roba "..."` -> no default applies (no `roba.toml`
  walking up).
- `roba --no-default-profile "..."` -> bypass even inside the project.

### `default` with project-aware permissions

Discussion + read-only git introspection. Lets claude run
`git status` / `git diff` to ground its answers without opening up
branch operations or edits:

```toml
[profile.default]
readonly = true
continue = true
allow_tool = [
    "Bash(git status)",
    "Bash(git diff)",
    "Bash(git diff --stat)",
    "Bash(git log)",
    "Bash(git log --oneline)",
]
```

`readonly` seeds the allow list with Read/Glob/Grep; `allow_tool`
adds these specific Bash patterns. Anything else (Edit, Write,
`Bash(git checkout)`, etc.) still gets blocked.

### `review` -- code review on current changes

```toml
[profile.review]
readonly = true
git_diff = true
```

Usage:

```bash
roba --profile review "is the auth change safe to merge?"
```

What it does: locks claude to read-only tools (no edits, no shell)
and embeds your working-tree diff into the prompt. Add a prepend
file with your own review style if you want stronger opinions:

```toml
[profile.review]
readonly = true
git_diff = true
prepend = ["~/.config/roba/prompts/review-style.md"]
```

### `explain` -- read-only walkthrough

```toml
[profile.explain]
readonly = true
```

Usage:

```bash
roba --profile explain --attach 'src/foo.rs' "what does this module do, and what assumptions does it make?"
```

Pairs well with `--attach`. The profile keeps claude from poking at
anything you didn't ask about.

### `commit-msg` -- generate a commit message from staged work

```toml
[profile.commit-msg]
readonly = true
git_diff = true

[profile.commit-msg.vars]
STYLE = "imperative, concise, no marketing"
```

Usage:

```bash
roba --profile commit-msg "write a commit message in the {{STYLE}} style"
```

The `STYLE` placeholder is substituted from the profile's vars. You
can override per-invocation:

```bash
roba --profile commit-msg --var STYLE="bullet points" "write a commit message in the {{STYLE}} style"
```

### `summarize` -- distill long content

```toml
[profile.summarize]
readonly = true

[profile.summarize.vars]
LENGTH = "one paragraph"
```

Usage with stdin:

```bash
cat long-doc.md | roba --profile summarize "summarize this in {{LENGTH}}, plain prose"
```

### `fix-build` -- diagnose a failed build from piped output

```toml
[profile.fix-build]
readonly = true
git_status = true
```

Usage:

```bash
cargo build 2>&1 | roba --profile fix-build "what's broken and how do I fix it?"
```

The `git_status` line gives claude context on which files you've been
editing -- often the actual culprit isn't obvious from the error
alone.

### `ticket` -- thread a project label through a template

```toml
[profile.ticket]
git_log = 3

[profile.ticket.vars]
PROJECT = "MYPROJ"
```

Usage with a template file:

```bash
# ~/.config/roba/prompts/standup.md
# Write today's standup in the {{PROJECT}} format. Recent commits:

roba --profile ticket -f ~/.config/roba/prompts/standup.md
```

### `gh-context` -- read-only PR / issue context via the gh CLI

A profile that lets claude pull GitHub context (PR / issue / diff /
list) through the `gh` CLI without opening the door to mutations:

```toml
[profile.gh-context]
description = "Read-only gh access for PR / issue context"
readonly = true
allow_tool = [
    "Bash(gh pr view:*)",
    "Bash(gh pr diff:*)",
    "Bash(gh pr list:*)",
    "Bash(gh issue view:*)",
    "Bash(gh issue list:*)",
    "Bash(gh repo view:*)",
]
```

Usage:

```bash
roba --profile gh-context "summarize the open PRs in this repo"
roba --profile gh-context "what does PR #42 do?"
```

`readonly = true` seeds the allow list with Read / Glob / Grep; the
`Bash(gh ...:*)` patterns add the read-only gh commands on top. The
`:*` suffix is Claude Code's prefix matcher -- it allows the command
plus any arguments (a PR number, `--json` flags), so `Bash(gh pr
view:*)` covers `gh pr view 42 --json title`. Claude can pull context
but can't `gh pr merge`, `gh pr close`, or run any verb you didn't
list.

For mutations, layer a more permissive profile or add the specific
patterns explicitly (`--allow-tool "Bash(gh pr comment:*)"`). Keep the
deny side in mind too: `deny_tool` wins over `allow_tool`, so a broad
allow plus a targeted deny is a valid shape if you'd rather subtract.

## Installing the bundled skill + agent library

Alongside config, roba bundles a starter set of operational *skills*
and orchestrator *subagents* (the same ones in this repo's `skills/`
and `agents/`). They're embedded in the binary at build time and
install into `~/.claude/` so any Claude Code session -- not just roba
-- auto-discovers them:

```bash
roba skill install            # copy bundled skills -> ~/.claude/skills/
roba agent install            # copy bundled agents -> ~/.claude/agents/

roba skill list               # bundled skills with descriptions
roba skill show draft-pr-first  # print one skill's SKILL.md body
```

`install` accepts `--to PATH` (custom destination), `--dry-run`
(preview without writing), `--force` (overwrite existing files), and
`--skip` (leave existing files in place, install the rest). With no
flag, an existing file prompts before overwriting (and is left alone
when there's no TTY). The `roba agent` subcommands mirror `roba skill`
exactly.

This is what makes the install-and-go path work: `cargo install roba`,
`roba skill install && roba agent install`, then
`claude --agent=roba-orchestrator` lands you in an orchestrator that
already knows the roba dispatch lifecycle.

## Aliases

Aliases are `git`-style shortcuts defined in the same `roba.toml`
files as profiles (same walk-up + merge discovery). Where a profile is
a *named bundle of flag defaults* you opt into with `--profile`, an
alias is a *new verb*: `roba NAME [args]` expands a prompt template
plus default flags and dispatches like a normal call.

The point is to keep roba's built-in surface generic. roba doesn't
assume "github + writing software"; that domain knowledge lives in
your aliases (and in agents/skills), not in the binary.

### Schema

Each alias is an `[alias.NAME]` table:

```toml
[alias.review]
description = "Review a PR by number"   # shown by `roba alias list`
agent = "reviewer"                       # optional: pins --agent NAME
template = """
Review PR #${pr} in this repo.

$(gh pr view ${pr} --json title -q '.title')

Diff:
$(gh pr diff ${pr})
"""
flags = ["--readonly"]                    # default CLI flags
args = ["pr"]                             # positional schema: pos 1 -> ${pr}
```

| field | meaning |
|---|---|
| `description` | optional summary for `roba alias list` |
| `agent` | optional subagent to pin (same as `flags = ["--agent", "NAME"]`; the field exists for `alias list` discoverability) |
| `template` | the prompt body, with variable + shell substitution (see below). Omit it for a *flag-shortcut* alias |
| `flags` | default CLI flags merged into the dispatch |
| `args` | optional positional schema mapping names to argument positions |

A **flag-shortcut alias** has no `template` -- the user's positional
args become the prompt verbatim. Useful for binding an agent + flags
to a short verb:

```toml
[alias.r]
description = "Quick review preset -- agent + flags, prompt is your args"
agent = "reviewer"
flags = ["--readonly"]
# roba r "look at the auth module"  ->  --readonly --agent reviewer "look at the auth module"
```

### Invoking

```bash
roba review 42        # expand [alias.review] with pr=42, dispatch
roba commit-msg       # zero-arg alias
roba review 42 --full-auto   # extra flags go after the positional args
```

Lookup order for `roba <word> [args]`:

1. **Built-in subcommand** (`cost`, `history`, `last`, `profile`,
   `skill`, `agent`, `alias`) -> dispatched as the built-in.
2. **Alias** in the merged pool -> expanded and dispatched.
3. **Otherwise** -> error listing the closest matches (built-ins +
   aliases) by edit distance.

A single bare word that matches an alias (`roba commit-msg`) is
treated as that alias; a multi-word quoted prompt (`roba "explain
this"`) is never an alias. If a single-word prompt collides with an
alias name, the alias wins -- rephrase or quote differently to force a
literal prompt.

### Variable substitution

In a `template`:

- `${1}`, `${2}`, ... -- positional args after the alias name (1-based).
- `${@}` -- all positional args joined with spaces.
- `${pr}`, `${num}`, ... -- named, resolved through the `args` schema
  (`args = ["pr"]` maps positional 1 to `${pr}`).
- `$$` -- a literal `$` (so dollar amounts and shell vars survive).
- Unknown or out-of-range variables expand to the empty string.

### Shell substitution

`$(command)` spans run in your shell (`sh -c`) and their stdout is
interpolated (one trailing newline trimmed). This is the load-bearing
power -- aliases compose with `gh`, `git`, `jq`, anything:

```toml
[alias.issue]
description = "Pull an issue into the prompt"
template = "$(gh issue view ${num} --json title,body)"
args = ["num"]
```

**Security:** shell substitution runs with *your* permissions.
Aliases are user-controlled config, so this is intentional -- it is
**not** a sandbox. Don't define an alias whose template runs a command
you wouldn't run yourself.

### Flag merging

The alias's `flags` are placed *before* the flags you pass on the
command line, so roba's usual precedence applies: single-value flags
are last-wins (your CLI flag overrides the alias default), and the
pinned `agent` is overridden by an explicit `--agent`. Note that
mutually-exclusive flags still conflict -- e.g. an alias with
`flags = ["--readonly"]` plus a CLI `--full-auto` errors rather than
silently swapping. Put flags *after* the positional args.

### Inspecting

```bash
roba alias list               # NAME / DESCRIPTION / AGENT table
roba alias show review        # the TOML block + an expansion preview
roba alias path               # which roba.toml files contribute aliases
```

`roba alias show` previews the template with variables rendered as
`<placeholders>` and `$(...)` left unexpanded (it never runs your
shell just to show a definition).

### Shadowing a built-in

If you define `[alias.cost]` (or any built-in name), the built-in wins
the lookup and the alias is dead. roba warns loudly on stderr at
config-load time so you can rename it; it does not fail hard.

### v1 limitations

- **No recursion.** An alias cannot invoke another alias; a template
  expands to a prompt, and that's it.
- **Flags after positionals.** Extra CLI flags for an aliased call
  must follow the positional args (`roba review 42 --full-auto`).
- **No sandbox** for `$(...)` -- see the security note above.

A starter alias config (github verbs like `review` / `issue` /
`commit-msg`) is a natural companion to `roba skill install`; shipping
one via the install command is tracked as a follow-up rather than
built into the binary.

## Tips

- **Keep prompts in files, not vars.** Profiles are best at flag
  defaults; for long prompt templates, prefer `prepend = ["..."]`
  pointing at a markdown file. Easier to edit, version-control, and
  share.
- **Layer a profile and ad-hoc flags freely.** `--profile review
  --full-auto` is fine if you want the review preset *and* tool
  bypass for that one call.
- **Inspect what a profile sets** by combining with `--echo`:

  ```bash
  echo "" | roba --profile review --echo -q
  ```

  prints the assembled prompt to stderr without making a real call.
- **Share profiles with your team** by checking
  `roba.toml` into a dotfiles repo or by dropping a copy in your
  project root and pointing `XDG_CONFIG_HOME` at it for that shell.

## Future

- Inline prompt text in profile schema (`prepend_inline = ["..."]`)
  so a profile can carry a prompt without a separate file
