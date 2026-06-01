# roba vs `claude -p`

`roba` is built on top of `claude -p` -- it shells out to the same
`claude` binary under the hood (via
[`claude-wrapper`](https://crates.io/crates/claude-wrapper)). This
doc lays out when each one is the right reach.

Short version: `claude -p` is the one-shot primitive. `roba` is an
ergonomic, pipe-clean, orchestrator-friendly wrapper around it. They
coexist; `roba` doesn't replace `claude -p`, it gives the same
one-invocation-one-answer model a richer input/output surface.

## What `claude -p` is

`claude -p` (print mode) runs Claude Code non-interactively: one
prompt in, the response printed to stdout, then exit. No session to
live in, no TUI. It's the right tool for one-shot, "just answer the
string I gave you" cases.

```bash
$ claude -p "summarize the rust ownership model in 3 bullets"
Rust's ownership model rests on three rules:
- Each value has a single owner.
- When the owner goes out of scope, the value is dropped.
- Borrows are either many immutable or one mutable.
```

## Where `claude -p` is great (and roba doesn't try to replace)

- **Quick one-off questions** with no piping, no file context, no
  session continuity needed. Typing `roba` instead buys you nothing
  here.
- **Inside a long-running shell session** where you don't need any of
  roba's affordances and just want a fast answer.
- **The minimal-dependency invocation** from a parent agent or script
  that is happy to parse plain text and doesn't need structured
  output or typed exit codes.

If that's the shape of your need, `claude -p` is the simpler call.
Reach for `roba` when you want one or more of the things below.

## Where roba adds value

- **Composable input.** Build the prompt from more than a string
  literal: a file (`-f`), piped stdin, an editor buffer (`-e`),
  `--prepend` / `--append` files, glob-matched file embedding
  (`--attach GLOB`), git context (`--git-diff`, `--git-log`,
  `--git-status`), and `{{KEY}}` template substitution (`--var K=V`).
- **Pipe-clean output.** stdout is the answer and only the answer;
  all metadata (cost footer, spinner, tool-call lines, refusal
  warnings) goes to stderr. `roba "..." | jq` sees clean stdout
  whether or not the decoration is humming on stderr.
- **Rich on a TTY.** Markdown rendering, an indicatif spinner, dim
  metadata, and colored refusal/error markers when you're at a
  terminal. The UI is transient: it exists while roba is working and
  evaporates when the answer lands.
- **Profiles.** A `roba.toml` alias for flags you'd otherwise retype,
  with layered resolution (CLI > env > profile > built-in default).
  Codify a project's policy once. See
  [`profiles.md`](profiles.md).
- **Session continuity without living in a session.** `-c` continues
  the most recent session in the cwd, `-c=ID` resumes a specific one,
  `-c=ID --fork` branches it, `--pick` is an interactive fuzzy
  chooser, and `roba history` / `roba last` browse past runs. You dip
  back into a thread without opening the TUI.
- **Typed exit codes.** Distinct codes let a caller tell failure
  classes apart: `0` success, `2` auth, `3` budget, `4` timeout,
  `1` generic/history error.
- **Versioned JSON envelope.** `--json` returns a stable, versioned
  shape -- `{ "version": 1, "result": {...}, "refusal": bool }` on
  success, `{ "version": 1, "error": {...} }` on failure -- so an
  orchestrator can pin an ABI.
- **Permissions presets + provenance.** Safe by default (Read, Glob,
  Grep only). `--readonly`, `--writable`, `--full-auto` presets;
  `--allow-tool` / `--deny-tool` for specifics; `--show-permissions`
  previews the fully resolved allow/deny set with per-entry
  provenance before any call is made.
- **Cost rollup.** `roba cost` and `roba cost --by-project` total
  token usage across history.
- **Worktree sandbox.** `-w` (or `-w=NAME`) runs the call in a fresh
  git worktree, so a writable dispatch can't touch your working tree.
- **Fail-fast on interactive flags without a TTY.** `-e` and `--pick`
  error immediately when stdin isn't a terminal instead of hanging
  silently.

## Side-by-side examples

### Quick question on a TTY

```bash
claude -p "what's the difference between Arc and Rc?"
```

```bash
roba "what's the difference between Arc and Rc?"
```

Same answer; `roba` renders the markdown and prints a token/cost
footer on stderr, while `claude -p` emits plain text. On a pipe roba
drops the decoration automatically.

### Quick question, piped

```bash
claude -p "list 3 rust web frameworks as json" | jq
```

```bash
roba --json "list 3 rust web frameworks" | jq
```

`claude -p`'s stdout is the model's prose, so a downstream `jq`
depends on the model formatting clean JSON in the body. `roba --json`
wraps the run in a versioned envelope (`.result`, `.refusal`,
`.version`) that's stable regardless of how the body is phrased.

### Loaded with file context

```bash
claude -p "is the error handling consistent? $(cat src/foo.rs)"
```

```bash
roba --attach 'src/foo.rs' "is the error handling consistent?"
```

With `claude -p` you shell-substitute the file into the prompt string
yourself. `--attach` embeds each glob-matched file with a `File: PATH`
frame, and takes a glob so you can pull in many files at once.

### Continuing a previous chat

```bash
# claude -p: no built-in continuation; each call is independent.
claude -p "now show me how to test the unsafe variant"
```

```bash
roba -c "now show me how to test the unsafe variant"
roba -c=7c3f9a21 "and the safe variant?"   # resume a specific session
```

`claude -p` has no session re-entry. `roba -c` continues the most
recent session in the directory; `-c=ID` resumes a specific one (the
`=` is required).

### Code review on a diff

```bash
claude -p "review my recent changes: $(git diff)"
```

```bash
roba --git-diff "review my recent changes"
```

`--git-diff` embeds the working-tree diff for you (there are matching
`--git-log` and `--git-status` flags), instead of shell-substituting
`git diff` into the prompt string.

### Cost-conscious batch

```bash
# claude -p surfaces cost only inside its JSON output.
claude -p --output-format json "..." | jq .total_cost_usd
```

```bash
roba --quiet "..."            # answer only, metadata suppressed
roba cost --by-project        # roll up token usage across history
```

`roba cost` aggregates usage from history after the fact, so you can
batch quiet runs and total them later rather than scraping per-call
JSON.

## Where neither fits

- **Interactive, multi-turn work** -> use `claude` itself (the full
  Claude Code session). Both `claude -p` and `roba` are one-shot; if
  you want to *live in* a session, that's interactive's territory.
  roba's session flags let you re-enter a thread, but a single roba
  call is still one prompt and one answer.
- **Multiple providers** -> use a multi-provider CLI like
  [`llm`](https://llm.datasette.com/) or `aichat`. roba is
  Claude-only by design -- the wrapper underneath is
  `claude-wrapper`, and the Claude-Code-native integration (sessions,
  permissions, history) is the point.

## Note on agent / orchestrator use

When something other than a human is calling, roba's pipe-clean
output, typed exit codes, and versioned JSON envelope make it a much
cleaner ABI than `claude -p`. `claude -p` mixes any decoration into
stdout and leaves error classification to the caller; roba keeps
stdout to the answer, routes everything else to stderr, and reports
failure class through both the exit code and the structured error
envelope. That ABI work is tracked across the JSON-envelope and
fail-fast issues (#33, #34, #35, #36), and the multi-repo
orchestration use case it serves is captured in #38.
