# roba quickstart

Get from install to your first answer in under two minutes.

## Prerequisites

`roba` shells out to the `claude` binary. You need
[claude-code](https://github.com/anthropics/claude-code) installed and
authenticated on your PATH before `roba` can do anything:

```bash
claude --version    # must exist
claude /login       # if not already authenticated
```

## Install

```bash
cargo install roba
```

That's it. No config required to start -- sensible defaults apply.

## First prompt

```bash
roba "what's the difference between Arc and Rc?"
```

Output goes to stdout. Metadata (tokens used, duration, session id)
goes to stderr so pipes stay clean.

## Examples

### Compose with files

Pass a system prompt as a preamble, then ask a question:

```bash
roba --prepend system.md "review this design"
```

Pipe stdin directly -- no flag needed when stdin is not a TTY:

```bash
cat error.log | roba "what's causing this and how do I fix it?"
```

Attach all Rust source files matching a glob:

```bash
roba --attach 'src/**/*.rs' "is the error handling consistent?"
```

### Extract code blocks

`--code` prints only fenced code blocks from the response. Useful when
you want the generated code without surrounding prose:

```bash
roba --code "write a Python function to flatten a nested list"
roba --code rust "show me a type-safe builder pattern"   # filter by language
```

### Capture output

`-o/--out PATH` writes the result to a file AND to stdout, so you can
pipe and save at the same time:

```bash
roba "summarize this RFC" -o summary.md          # file + stdout
roba "summarize this RFC" -o summary.json --json  # JSON envelope
```

### Continue and revisit sessions

Bare `-c` continues the most recent session in the current directory
(use `-p` to pass the prompt without ambiguity):

```bash
roba -c -p "now show me how to test that"
```

Resume a specific session by id:

```bash
roba -c abc12345 -p "follow up"
```

Open an interactive fuzzy chooser over recent sessions:

```bash
roba --pick
```

### Opt into write access

By default roba is read-only (Read, Glob, Grep only). Opt in explicitly
when you want claude to edit files:

```bash
roba --writable "rename all uses of foo to bar in src/"
roba --full-auto "implement the stub in src/lib.rs"   # sandbox only
```

## Next steps

**Profiles + aliases.** Save sets of flags you type every time into
`roba.toml` as named profiles, and define `[alias.NAME]` shortcuts that
expand a prompt template plus flags. See [`profiles.md`](profiles.md)
and [`aliases.md`](aliases.md).

**Skills and agents.** Drop skills into `~/.claude/skills/` and
agents into `~/.claude/agents/`; Claude Code auto-discovers them.
[joshrotenberg/agent-tools](https://github.com/joshrotenberg/agent-tools)
is one curated set.
