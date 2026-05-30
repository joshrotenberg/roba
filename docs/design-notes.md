# roba design notes (brainstorm)

> **Note on status:** this is a brainstorm / idea log, not an
> authoritative spec. The actual state of `roba` lives in the code,
> the per-crate CHANGELOG, and the crate README. Items here may be
> shipped, partly shipped, or still purely speculative -- treat
> them as design context, not a project board.

Working notes for a CLI that wraps `claude -p` with better defaults,
better ergonomics, and a friendlier surface for scripts and humans.

The name `roba` (claude wrapper runner) was a placeholder that stuck.

## Decisions log

Settled in conversation; pending execution.

- **Positioning + 0.1 surface freeze, working notes** (2026-05-29).
  See `docs/positioning.md` for the running document on what roba
  is for ("augments interactive, doesn't replace") and the
  candidate cuts / collapses before the 0.1 surface gets frozen.
  Parking-lot items tracked as individual GitHub issues.
- **Config system: roba.toml + layered resolution** (2026-05-27).
  Single `roba.toml` file at any tier (no `.roba/` directory).
  Resolution: CLI > env (`ROBA_<PARAM>`) > files walking up from cwd,
  closer-beats-farther. Profiles kept as opt-in `[profile.NAME]`
  overlays in the same file. Recency-windowed auto-continue parked;
  `continue` stays binary. roba is positioned as a pass-through
  convenience layer to `claude -p`, not a sticky-session product.
  Full design in "Config system" section below; implementation
  tracked in GitHub #1.
- **Name: `roba`** (2026-05-24). Venetian / Italian for "stuff,
  things." Four chars, no `claude` substring (avoids trademark
  drift), confirmed free on crates.io. Affects: crate name, bin
  name, repo URL, config dir (`~/.config/roba/`), project-local dir
  (`.roba/profiles.toml`). Rename mechanics tracked in
  `migration-plan.md`.
- **Repo: dedicated** (2026-05-24). Move out of the
  claude-wrapper workspace into its own GitHub repo. Switch
  claude-wrapper dep from path to crates.io version. Mechanics in
  `migration-plan.md`.

## Premise

`claude -p` is the obvious tool for one-shot prompts, but the experience
has rough edges. The wrapper crate already exposes ~49 typed options
plus retry, budget, history, slash helpers, worktree, agents, settings,
skills, commands, and duplex permission modes. A thin CLI on top with
good defaults could be meaningfully nicer than the raw binary without
becoming a parallel re-implementation.

This is a brainstorm, not a spec. Add ideas freely.

## Pain inventory (today's `claude -p`)

| | Pain |
|---|---|
| Output | Plain mode buries cost/duration/session-id; JSON mode is hard to read; "the answer" mixed with metadata makes piping awkward |
| Prompt composition | `-p "<giant string>"` only -- no first-class stdin/file/$EDITOR/template/attachments |
| Tool perms | `--allowed-tools 'Bash(git *),Edit'` is a shell-quoting hazard; no presets (readonly, code-edit) |
| Sessions | `-c` resumes "last" but which? no `--pick`; no history listing from the CLI |
| Cost | Discoverable only post-hoc, no pre-run estimate, no soft warnings before hard ceiling |
| Failures | Single non-zero exit code -- auth, budget, network, timeout all look the same to callers |
| Streaming | All-or-nothing: stream-json firehose or final blob; no "show tool calls only" |
| Retry / re-run | No "retry that last one with sonnet"; no replay |
| Attachments | No clean way to include files; you write paths into the prompt and hope |
| MCP / agents | Passing requires file paths and JSON; no inline JSON, no defaults from cwd |

## Idea menu

Ideas ordered by category. Loose, not prioritized.

### Output handling

**Guiding principle:** rich and interactive on a TTY (spinners, color,
markdown render, inline tool calls), but very scriptable on a pipe
(plain text, only the answer on stdout). Both should fall out
automatically from `isatty` + `NO_COLOR` detection, not need separate
flag combinations.

#### Foundational

- stdout = the answer. stderr = metadata (cost, duration, session id,
  echo, spinner, tool calls, errors). Becomes pipeable:
  `roba "summarize" | jq` works because the answer is alone on stdout
- TTY/pipe auto-detect: markdown-render on TTY, plain on pipe.
  Honor `NO_COLOR`
- `--save out.{md,json}` writes a copy to a file; extension drives
  format. stdout stays clean
- `--tee out.md` writes both stdout and file (like Unix tee)

#### Mode switches

- `--json` -- full record as JSON on stdout (parseable)
- `--quiet` -- just the answer, no metadata, no echo, no footer
- `--brief` -- answer + one-line cost/duration footer on stderr
- `--raw` -- exactly what claude returned, no processing (the original
  string)

#### During-run (TTY only)

- Spinner with elapsed time; optional token / cost counter ticking up
- Streaming: live tokens as they arrive (default on TTY, off on pipe?
  TBD)
- Inline tool calls: `> Read(src/foo.rs)` lines as tools fire
- `--show-thinking` -- reveal extended-thinking blocks. Needs the
  streaming pipeline: thinking content arrives as separate stream
  events, not as part of the final JSON `result.result`. Bundle
  with the broader streaming work rather than ship a flag that
  silently shows nothing on the non-streaming path.
- Verbosity stacks: `-v` adds cost footer, `-vv` adds tool calls,
  `-vvv` adds thinking blocks

#### After-run

- Cost footer (stderr, on TTY by default): 
  `tokens 1.2k/600 . cost $0.018 . 4.3s . sonnet . session abc123`
- Tool rollup: `Used: Read x3, Edit x1, Bash x2` summary
- Session footer: `Session: abc123 (continue with roba -c)`
- Citations list: if claude referenced files, list them at the end

#### Content shaping

- `--code` -- print only fenced code blocks
- `--code --lang rust` -- filter by language
- `--code N` -- only the Nth code block (great for "give me a function")
- `--head N` / `--tail N` -- truncate long answers cleanly
- `--format auto|markdown|plain|json` -- explicit format override

#### Errors and refusals

- Refusal styling: render "I can't help with that" visibly different
  (color, prefix) so it doesn't get lost in a scrollback
- Auth / network / budget errors get distinct exit codes (see Failure
  modes) so callers can branch programmatically

### Prompt composition

- `-p "literal"` | `-f file.md` | `-` for stdin | `-e` opens `$EDITOR`
- `--prepend file --append -` mixes sources (template + stdin diff is a
  common case)
- `--attach 'src/**/*.rs'` rolls globbed files in with proper framing
  (file path + fenced code block)
- `--git-diff` / `--git-log -n 5` / `--git-status` snap-in context blocks
- Templating: `--var TICKET=ABC-123` substitutes into the prompt body
- Heredoc-style: `roba <<EOF` for inline multi-line without quoting

### Tool / permission ergonomics

- Repeated short flags instead of comma-joined: `-t Read -t Bash`
  (already supported by the wrapper)
- Presets:
  - `--readonly` -- Read, Glob, Grep
  - `--code` -- adds Edit, Write
  - `--full-auto` -- everything (today's `--dangerously-skip-permissions`)
  - `--no-bash`, `--no-net` for quick subtractions
- `--show-permissions` previews what will actually be allowed before
  running
- `--review-mode` -- read-only + a code-review system prompt baked in

### Sessions

- `roba -c` prints which session it picked (title + last-mod) so you're
  not guessing
- `roba --pick` fzf-style chooser over recent sessions; pairs with `-c`
- `roba history` lists recent N with title/cost/age/cwd (already have
  the wrapper-side reader -- `claude_wrapper::history`)
- `roba last` reprints last run's answer/cost/session-id (cheap re-read
  of disk)
- `roba fork <id>` clones a session to experiment without polluting
  the original

### Cost / budget

- `--budget 5.00` hard ceiling (already in wrapper)
- `--warn 1.00` soft warning line
- `--estimate` does input-token count + model rate to predict a ceiling,
  no API call needed
- Per-run footer on stderr:
  `tokens 1.2k/600  cost $0.018  4.3s  sonnet`
- `roba cost --since 1d` rolls up history JSONL into a usage report
  (uses the new `SessionSummary.total_tokens` field)

### Failure modes

- Typed exit codes:
  - 0 ok
  - 1 generic error
  - 2 auth (re-login needed)
  - 3 budget exceeded
  - 4 timeout
  - 5 transient network
  - 6 tool rejected
- Auto-retry transient classes (5, maybe 4) with exponential backoff
  (wrapper has the retry primitives already)
- Single-line stderr summary on each failure so callers can grep
- `--no-retry` for scripts that want deterministic behavior

### Profiles / templates

- `~/.config/roba/profiles.toml` with named combos:
  ```toml
  [profile.review-rust]
  model = "opus"
  system-prompt-file = "~/.config/roba/prompts/review-rust.md"
  tools = ["Read", "Glob", "Grep"]
  budget = 0.50
  ```
- Invocation: `roba --profile review-rust @src/main.rs`
- Built-in profiles shipped with the binary:
  - `review` -- code review on changed files
  - `explain` -- explain code at a path/range
  - `fix-build` -- read build error from stdin, propose fix
  - `commit-msg` -- generate commit message from staged diff
  - `summarize` -- summarize a file or piped text

### Async / detached

- `roba -p "..." --async` prints session id and exits; useful for long
  ones the user wants to background
- `roba attach <id>` resumes streaming output for a running session
- `roba wait <id>` blocks until done, then dumps result
- (The `dispatch` crate already prototyped this; revisit whether to
  fold it in or keep separate)

### Streaming / observability

- `--stream` shows live tokens
- `--show-thinking` reveals extended-thinking blocks
- `--show-tools` displays each tool call as it happens, one per line
- `--silent` swallows everything except the answer
- `--trace out.jsonl` writes the full stream-json to a file
- `--inspect` (later) opens a TUI viewer for the last run's transcript

### MCP / agents / extras

- `--with-mcp ./local-mcp.json` quick-attach an MCP config
- `--temp-mcp '{"server-x":{...}}'` inline JSON, no file needed
- `--agent foo` (already in wrapper)
- Auto-discover `roba.toml` in cwd for project-level defaults
- `--workdir` (already supported) plus `--worktree` to drop into a
  fresh worktree first (wrapper has the typed builder)

### REPL mode (future, on top of duplex)

A `roba repl` subcommand for interactive multi-turn work without
opening a full `claude` session. Lighter than claude proper, but
heavier than one-shot `roba "..."`. The "not quite the full claude
but still interactive turns" sweet spot.

Implementation: reedline-repl on the front, `DuplexSession` on the
back. This is where duplex genuinely earns its keep -- one warm
claude child held open across turns saves the spawn cost per turn
and unlocks mid-turn interrupt + permission handlers cleanly.

Sketch:

- Multi-line input (reedline supports it out of the box)
- Slash commands: `/exit`, `/clear`, `/model opus`, `/tools list`,
  `/cost`, `/save out.md`, `/fork`, `/resume <id>`, `/help`
- History recall (reedline's persistent history)
- Per-turn footer (same shape as one-shot roba) plus a cumulative
  cost line at the prompt: `[$0.42 total] >`
- Auto-checkpoint: every N turns, write the session id so a crash
  doesn't lose context
- `/re` to resend the last prompt with edits ($EDITOR), useful
  when claude misread the question
- Ctrl-C cancels current turn (duplex interrupt), Ctrl-D exits
- Optional Vi keybindings for editing prompts in place

Background note: Josh had a partial start on a similar REPL using
`reedline-repl` (or `reedline-repl-rs`?) -- worth pulling that
shape forward rather than starting from scratch.



If we ship a minimal v0.1, the highest-impact subset is probably:

1. **stdout/stderr separation + smart TTY/pipe behavior** -- biggest
   day-to-day "feels better" win
2. **Prompt composition (`-f`, stdin, `-e`, `--attach`)** -- removes
   the "how do I get my prompt in" friction
3. **Permission presets (`--readonly`, `--code`, `--no-bash`)** --
   turns a quoting nightmare into one flag
4. **Cost footer on stderr + `--estimate` + `roba cost`** -- makes cost
   legible without forcing JSON mode
5. **Typed exit codes + auto-retry transients** -- turns the runner
   into a script-friendly building block

Almost all of these lean on primitives that already exist in
`claude-wrapper`. The CLI would be a thin layer of good defaults,
not a from-scratch project.

## Open questions

- Where does this live? New crate in this workspace? Separate repo?
- Binary name. `roba`? `cq` (claude-query)? Something else?
- Sync default or async default? Sync is simpler for single-prompt;
  async fits long-running ones.
- Color/markdown rendering -- pull in a TUI dep (`ratatui`, `crossterm`)
  or stay minimal with `termcolor`?
- Profiles: ship as TOML or use the existing `roba.toml` shape we'd
  invent here?
- Does it absorb the `dispatch` crate's async/detach work, or stay
  separate and compose?

## More ideas

(scratch space; add freely)

### Session export -- `roba export <id>` / `roba cat <id>`

Field-discovered use case: pull a full conversation out of roba's
history to use as context elsewhere. Today you can fake it via
`roba last --type all -n 9999 --project <slug>` but the name and
flag combo don't advertise the use case.

A proper surface would be:

```bash
roba export <session-id>                   # default: markdown to stdout
roba export <session-id> --format prompt-context
roba export <session-id> --format jsonl    # raw stream events
roba export <session-id> --save out.md
roba cat <session-id>                      # alias for export markdown to stdout
```

Format options worth thinking through:

- **markdown** (default): rendered transcript with `## user` /
  `## assistant` headings, code blocks preserved, tool calls
  shown inline as italic markers.
- **prompt-context**: formatted as a primer you can feed back
  into another LLM ("Here's a prior conversation. Continue from
  here:"). Strips tool noise; just the user/assistant text.
- **jsonl**: pass-through the raw session JSONL for tools that
  want structure.
- **plain**: just the text content, no headers/markers (clean
  for grep / wc -l / pipe-into-another-thing).

Resolution: most-recent in cwd by default if no `<id>` given.
`--all-projects` widens; `--project SLUG` filters; matches the
existing history / last semantics.

Companion thoughts:

- `roba export latest` as a shorthand for "the most recent session
  in this cwd" since that's probably the common case.
- A `--with-meta` flag to also dump cost / token totals / model
  used / timestamps, useful for audit or sharing.

### Transient status bar / "TUI while working, shell when done"

A mental model worth chasing: while roba is making a call, the
terminal briefly becomes a lightweight TUI -- a pinned status bar
at the bottom showing elapsed time, tool count, latest action --
while content (text, tool calls) scrolls above. When the call
completes, the status bar clears and we're back to a normal shell
prompt with the answer rendered above. No persistent UI, no
"launch a full TUI" mode -- just a transient panel that exists
exactly as long as roba is doing something.

Concretely:

```
   That makes sense -- let me check the diff first.
   ▸ Bash(git diff --stat)
   ▸ Read(src/info.rs)
   Looking at info.rs now...
   ▸ Edit(src/info.rs)

   [pinned status bar at bottom, redraws in place]
   ⠋ 12.4s · 4 tools · Edit(src/info.rs)
```

After completion, the status bar disappears, the answer's footer
prints, and you're back to your shell prompt. Nothing lingers.

Implementation: indicatif's `MultiProgress` -- it coordinates a
spinner with anything else printing to the terminal. Spinner stays
pinned; everything else scrolls above. We'd route tool-call lines
and streamed text through `multi.println(...)` instead of
`eprintln!` / `println!` directly. The spinner's `set_message`
gets updated on each event with elapsed time + tool count + latest
action.

Composes with:

- **Live-text stream mode** (today): status bar adds the "still
  working during quiet stretches" signal that pure live tokens
  miss.
- **Buffered stream mode** (future): the status bar IS the only
  live UI; final render happens once it disappears.
- **Non-stream mode** (today): we already have a spinner here,
  but it doesn't "pin" -- could use the same MultiProgress
  treatment for consistency.

Add as one focused commit when ready. Pairs naturally with the
buffered-stream work since both need the spinner-as-status-channel
infrastructure.

### Streamed text re-render

Today `--stream` shows raw text with a 3-space indent but no
markdown rendering -- termimad needs the whole text at once to
parse structure, and we don't have that until the result event
arrives.

Three approaches if we want formatting in stream:

1. **Render at end via cursor manipulation.** Stream live as today,
   then on the result event cursor-up over the streamed text region
   and redraw via termimad. One redraw, no flicker. Needs accurate
   "rendered line count" tracking (soft-wrap aware) and crossterm
   cursor manipulation. Add as `--final-render` opt-in flag.

2. **Re-render every chunk.** Each new text event triggers
   clear-to-end + redraw the accumulated buffer. Real flicker risk,
   ANSI scroll math gets hairy when tool calls (stderr) interleave
   with the text region (stdout). Probably annoying to read.

3. **Buffered + rich spinner (preferred).** Don't stream the text
   body visibly. Show a status line on stderr that updates with
   each event:

       ⠋ thinking... 4.3s
       ⠙ thinking... 6.1s · Bash(git status)
       ⠹ thinking... 8.2s · Read(src/foo.rs) · 4 tools

   When the result event arrives, clear the status line and render
   the full text via termimad with tool calls inline at their
   chronological positions. The user still sees "claude is working"
   (latest tool + count + elapsed), and the final output is fully
   formatted -- without any mid-stream redraw mess.

   Implementation: existing indicatif spinner gains a `set_message`
   call on each assistant/result event. The streaming closure
   accumulates a `Vec<Item>` (text vs tool) instead of printing
   live. Final render walks the buffer and dispatches to
   `print_body` / `print_tool_call`.

Option 3 is the cleanest and avoids the line-counting pain of
option 1. The tradeoff is "you don't see the text appear live" --
but for prose answers, you couldn't read the wrapping/markdown
until the end anyway. For "show me what claude is doing" the
tool-call counter on the spinner covers it.

### `-C` / `--cwd PATH` -- run prompt in a different directory

```bash
roba -C ~/Code/foo "what does this crate do?"
roba --cwd ../bar -c "follow up about bar"
```

Matches `git -C` / `make -C` convention. Without it, roba inherits
the shell's cwd; with it, claude (and all cwd-derived behavior)
operates as if invoked from that path.

Knock-on effects to think through:

- **Claude's working dir.** Pass through to `QueryCommand` /
  process spawn so claude actually sees the right cwd.
- **Profile discovery.** The walk-up that finds
  `.roba/profiles.toml` (and the `default` auto-apply) walks from
  `--cwd`, not the shell's cwd. Probably what users want -- "run
  as if I were in that project."
- **History scope.** `roba history` / `roba last` infer the project
  slug from cwd; with `-C`, infer from `--cwd`. Same logic, just
  different starting point.
- **User-supplied paths** (`-f`, `--prepend`, `--append`,
  `--attach` globs). Open question: resolve relative to the
  shell's cwd (since the user typed them there) or to `--cwd`
  (consistent with everything else)? Lean toward shell's cwd:
  `roba -C ~/proj -f notes.md` should pick up `./notes.md` from
  where the user typed, not from `~/proj/notes.md`. Document
  clearly; absolute paths sidestep the ambiguity.
- **Composition with async.** `roba --async -C path "..."` lets
  you fire off a background job for another project without
  cd-ing. Common case.

Pairs naturally with the async story above -- the two flags
compose well.

### Async / detached execution

`roba --async "..."` returns immediately with a session id; the
prompt continues running in the background until claude completes.
Useful for long-running prompts ("review this whole module") where
you don't want to hold the terminal hostage.

```bash
$ roba --async "do a deep review of the auth module"
session 7c3f9a21 dispatched (pid 91234)
$ # ... go do other work ...
$ roba status 7c3f9a21
session 7c3f9a21 running (4m12s, 8 tool calls so far)
$ roba wait 7c3f9a21
... blocks until done, then prints the rendered output ...
$ roba attach 7c3f9a21
... attaches to the live stream from this point forward ...
```

Mechanics:

- **Double fork** to detach from the parent's terminal session.
  Standard Unix pattern: fork, parent exits, child setsid()s and
  forks again; grandchild is reparented to init and survives the
  terminal closing. Closes stdin/stdout/stderr or redirects to a
  log file in the roba state dir.
- **State directory**: `~/.local/state/roba/sessions/<id>/` holds:
  - `meta.json` (pid, started_at, prompt, profile applied, status)
  - `stream.jsonl` (events as they arrive from claude)
  - `out.txt` (final rendered text once complete)
- **Status / wait / attach** subcommands read from the state
  directory; `attach` tails `stream.jsonl` and renders new events
  as if it were a live `--stream`.
- **Lifecycle**: the background process updates `meta.json` status
  through `dispatched -> running -> completed | failed`. A periodic
  cleanup job (or manual `roba clean`) prunes old completed sessions.

Open questions:

1. **Queued messages.** If a session is mid-run and the user wants
   to send another prompt to the SAME session, do we queue it?
   Spawn a parallel? My instinct: queueing inside a session is
   hard (duplex needed), so each `roba` invocation makes its own
   background session. Use `--resume <id>` to chain.
2. **Output capture.** Background sessions can't render to the
   user's terminal directly. Do we keep the full streamed
   `stream.jsonl` for `roba attach` replay, or only the final text?
   Probably both -- jsonl is cheap.
3. **Notifications.** Should `roba` notify on completion (system
   notification, terminal bell, slack webhook)? Optional flag.
4. **Composition with `dispatch` crate.** The existing `dispatch`
   crate at the workspace level prototyped much of this. Decide:
   absorb its code into roba, depend on it, or keep separate and
   document the relationship.
5. **--resume with async**: `roba "first prompt" --async` then
   `roba "follow-up" --resume <id>` -- does the follow-up wait for
   the first to complete, or queue, or fork? Most consistent:
   error if the resumed session is still running; user can
   `roba wait` then resume.

This is where duplex genuinely earns its keep on the wrapper
side: a long-running background process IS a duplex session.
Inline `respond_to_permission` becomes valuable (background
claude can ask, we can answer via a side channel like a unix
socket). Worth revisiting the `DuplexSession` design once async
is in flight.

### Tool-call expansion levels

Today's tool-call rendering (both live `--stream` and `roba last
--type tools`) is one line per call: name + truncated primary arg.
Sometimes you want more, sometimes less. A verbosity knob would
help:

| Level | What | Approx use case |
|---|---|---|
| L0 | name only (`▸ Bash`) | "what tools is claude reaching for?" overview |
| L1 (today) | name + primary arg truncated to 60 chars | quick glance |
| L2 | name + full input JSON formatted | "what was the exact command?" |
| L3 | input + the tool_result content claude got back | full audit trail |

Surfaces:

- Live `--stream`: maybe `-v` / `-vv` count flags, or `--show-tools
  full`.
- `roba last`: same knob, applied to historical replay.

L3 is the interesting one. `tool_result` entries aren't in the
`tool_use` assistant block -- they live in subsequent `user`
entries with content blocks of type `tool_result`, paired by
`tool_use_id`. A small pairing pass over the entry stream would
build the (call, result) pairs. Cheap; not in flight yet.

### Config system: roba.toml + layered resolution

Settled 2026-05-27. Supersedes the earlier `~/.config/roba/profiles.toml`
+ `.roba/profiles.toml` scheme described in "Profiles / templates"
above. Implementation tracked in GitHub #1.

#### Positioning

roba is a **pass-through convenience layer to `claude -p`** -- not a
sticky-session product, not a parallel re-implementation. The config
system reflects that: it exists to let you stack defaults without
re-typing flags, never to inject hidden behavior. The mental slot it
fills is "something between `claude -p` and a full interactive
claude session."

#### Resolution model

For any single roba run, every setting comes from the highest layer
that defines it:

1. **CLI flag** -- explicit, per-call
2. **`ROBA_<PARAM>` env var** -- matches the CLI long name (lists
   comma-separated, e.g. `ROBA_ALLOW_TOOL="Edit,Write"`)
3. **`[profile.NAME]` overlay** in any `roba.toml` -- activated by
   `--profile NAME` or `ROBA_PROFILE=NAME`
4. **Top-level keys** in any `roba.toml`
5. **`~/.config/roba.toml`** -- your baseline overriding roba's
   built-in defaults
6. **roba's built-in defaults** (readonly by default, etc.)
7. **claude's defaults** -- the floor

Files (#3 + #4) walk up from cwd to the git root (or `~` if there
is none), with closer beating farther on the same key. Lists merge
across files; CLI and env replace.

#### Schema (v1)

One file, top-level keys = defaults, `[profile.NAME]` blocks use the
exact same field set as opt-in overlays.

```toml
model = "claude-sonnet-4-6"   # passthrough; omit for claude default

continue = false              # binary; no recency window

prepend = []
append = []
attach = []
git_diff = false
git_log = 0
git_status = false

[vars]

readonly = false
writable = false
full_auto = false
allow_tool = []
deny_tool = []

stream = false
echo = false
plain = false
quiet = false
json = false

[profile.NAME]
# same field set as overlays
```

#### Renames from the old schema

- `continue_session` -> `continue`
- `allow_tools` -> `allow_tool` (singular; matches CLI `--allow-tool`)
- `deny_tools` -> `deny_tool` (same)

`continue` is a Rust keyword; the struct uses
`#[serde(rename = "continue")]`.

#### File-layout changes

- Project: `.roba/profiles.toml` -> `roba.toml`. No directory.
- User: `~/.config/roba/profiles.toml` -> `~/.config/roba.toml`. No
  directory.

`ROBA_PROFILES_FILE` (today: points at an extra file at top priority)
is subsumed by the more general `ROBA_<PARAM>` override layer plus
the regular file discovery walk.

#### Conflict rules

- `readonly` / `writable` / `full_auto` remain mutually exclusive --
  error if more than one resolves to true after all layers compose.
- List vs scalar semantics: CLI and env **replace** the resolved
  value; multiple files **merge** (per-key for scalars, concat for
  lists, closer-wins on duplicate scalars).

#### Parked decisions (intentionally deferred)

- **Recency-windowed auto-continue.** Briefly explored; tabled. The
  framing landed on "do a piece of work with ability to follow up
  once or twice," but the cost of guessing wrong (wasted prompt,
  claude has no context) is asymmetric. `continue` stays binary;
  explicit `-c` / `--continue` is the surface.
- **Profile vs top-level precedence within the same file.** Current
  intent: an active `[profile.NAME]` block sits above same-file
  top-level keys but below env and CLI. Revisit when implementing --
  edge cases around explicit `false` values matter.
- **Keep profiles at all?** Kept for v1; reconsider if the layered
  defaults + env layer turn out to cover every real use case in
  practice.
- **`ROBA_SESSION` env var** for shell-scoped session pinning. Not
  in v1; comes back if the layered model proves insufficient.
- **`--fresh` flag** to force a new session despite resolved
  `continue = true`. Tracked separately as GitHub #2.

#### What this kills

- `ROBA_PROFILES_FILE` (subsumed by direct env-var overrides + file
  walk-up)
- `.roba/` directory (collapsed to a single `roba.toml`)
- Active investigation into recency / time-windowed stickiness
