# roba aliases

Aliases are `git`-style shortcuts defined in the same `roba.toml`
files as [profiles](profiles.md) (same walk-up + merge discovery).
Where a profile is a *named bundle of flag defaults* you opt into with
`--profile`, an alias is a *new verb*: `roba NAME [args]` expands a
prompt template plus default flags and dispatches like a normal call.
This page covers the schema, lookup order, substitution, and caveats;
for the big-picture framing, see the repo [`README.md`](../README.md).

The point is to keep roba's built-in surface generic. roba doesn't
assume "github + writing software"; that domain knowledge lives in
your aliases (and in agents/skills), not in the binary.

## Schema

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

A second worked pair -- a PR review and a commit-message generator:

```toml
[alias.commit-msg]
description = "Conventional-commit message from the staged diff"
template = "Write a conventional-commit message for:\n\n$(git diff --staged)"
flags = ["--quiet"]
```

## Invoking

```bash
roba review 42        # expand [alias.review] with pr=42, dispatch
roba commit-msg       # zero-arg alias
roba review 42 --full-auto   # extra flags go after the positional args
```

Lookup order for `roba <word> [args]`:

1. **Built-in subcommand** (`cost`, `history`, `last`, `profile`,
   `alias`) -> dispatched as the built-in.
2. **Alias** in the merged pool -> expanded and dispatched.
3. **Otherwise** -> error listing the closest matches (built-ins +
   aliases) by edit distance.

A single bare word that matches an alias (`roba commit-msg`) is
treated as that alias; a multi-word quoted prompt (`roba "explain
this"`) is never an alias. If a single-word prompt collides with an
alias name, the alias wins -- rephrase or quote differently to force a
literal prompt.

## Variable substitution

In a `template`:

- `${1}`, `${2}`, ... -- positional args after the alias name (1-based).
- `${@}` -- all positional args joined with spaces.
- `${pr}`, `${num}`, ... -- named, resolved through the `args` schema
  (`args = ["pr"]` maps positional 1 to `${pr}`).
- `$$` -- a literal `$` (so dollar amounts and shell vars survive).
- Unknown or out-of-range variables expand to the empty string.

## Shell substitution

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
you wouldn't run yourself. Note this is orthogonal to roba's
[permission model](permissions.md), which governs what the *spawned
claude* may do -- `$(...)` runs in your own shell before claude is
even invoked.

## Flag merging

The alias's `flags` are placed *before* the flags you pass on the
command line, so roba's usual precedence applies: single-value flags
are last-wins (your CLI flag overrides the alias default), and the
pinned `agent` is overridden by an explicit `--agent`. Note that
mutually-exclusive flags still conflict -- e.g. an alias with
`flags = ["--readonly"]` plus a CLI `--full-auto` errors rather than
silently swapping. Put flags *after* the positional args.

## Inspecting

```bash
roba alias list               # NAME / DESCRIPTION / AGENT table
roba alias show review        # the TOML block + an expansion preview
roba alias path               # which roba.toml files contribute aliases
```

`roba alias show` previews the template with variables rendered as
`<placeholders>` and `$(...)` left unexpanded (it never runs your
shell just to show a definition).

## Shadowing a built-in

If you define `[alias.cost]` (or any built-in name), the built-in wins
the lookup and the alias is dead. roba warns loudly on stderr at
config-load time so you can rename it; it does not fail hard.

## v1 limitations

- **No recursion.** An alias cannot invoke another alias; a template
  expands to a prompt, and that's it.
- **Flags after positionals.** Extra CLI flags for an aliased call
  must follow the positional args (`roba review 42 --full-auto`).
- **No sandbox** for `$(...)` -- see the security note above.

A starter alias config (github verbs like `review` / `issue` /
`commit-msg`) makes a natural companion to a curated skill+agent
library like [joshrotenberg/agent-tools](https://github.com/joshrotenberg/agent-tools);
roba itself ships no opinionated alias defaults.
