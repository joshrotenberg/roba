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
| `readonly` | `bool` | `--readonly` | Explicit form of the default; no-op (the default) |
| `writable` | `bool` | `--writable` | Adds Edit + Write to the allow list |
| `full_auto` | `bool` | `--full-auto` | Bypass all permission checks |
| `allow_tool` | `[string]` | `--allow-tool TOOL` (repeatable) | Adds to the allow list |
| `deny_tool` | `[string]` | `--deny-tool TOOL` (repeatable) | Deny patterns; useful with `full_auto` to keep some teeth |
| `continue` | `bool` | `-c` / `--continue` | Skipped if `--resume` is also passed |
| `vars` | `{ key = "value" }` | `--var KEY=VALUE` (repeatable) | CLI keys override profile keys |
| `model` | `string` | `--model MODEL` | Alias (`sonnet`/`opus`/`haiku`) or full id (`claude-sonnet-4-6`) |
| `stream` | `bool` | `--stream` | Stream tokens as they arrive |
| `echo` | `bool` | `--echo` | Print resolved prompt before the response |
| `plain` | `bool` | `--plain` | No rendering, color, or spinner |
| `quiet` | `bool` | `-q` / `--quiet` | Answer only, no metadata |
| `json` | `bool` | `--json` | Structured result as JSON on stdout |

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
