# cwr design notes (brainstorm)

> **Note on status:** this is a brainstorm / idea log, not an
> authoritative spec. The actual state of `cwr` lives in the code,
> the per-crate CHANGELOG, and the crate README. Items here may be
> shipped, partly shipped, or still purely speculative -- treat
> them as design context, not a project board.

Working notes for a CLI that wraps `claude -p` with better defaults,
better ergonomics, and a friendlier surface for scripts and humans.

The name `cwr` (claude wrapper runner) was a placeholder that stuck.

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
  `cwr "summarize" | jq` works because the answer is alone on stdout
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
- Session footer: `Session: abc123 (continue with cwr -c)`
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
- Heredoc-style: `cwr <<EOF` for inline multi-line without quoting

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

- `cwr -c` prints which session it picked (title + last-mod) so you're
  not guessing
- `cwr --pick` fzf-style chooser over recent sessions; pairs with `-c`
- `cwr history` lists recent N with title/cost/age/cwd (already have
  the wrapper-side reader -- `claude_wrapper::history`)
- `cwr last` reprints last run's answer/cost/session-id (cheap re-read
  of disk)
- `cwr fork <id>` clones a session to experiment without polluting
  the original

### Cost / budget

- `--budget 5.00` hard ceiling (already in wrapper)
- `--warn 1.00` soft warning line
- `--estimate` does input-token count + model rate to predict a ceiling,
  no API call needed
- Per-run footer on stderr:
  `tokens 1.2k/600  cost $0.018  4.3s  sonnet`
- `cwr cost --since 1d` rolls up history JSONL into a usage report
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

- `~/.config/cwr/profiles.toml` with named combos:
  ```toml
  [profile.review-rust]
  model = "opus"
  system-prompt-file = "~/.config/cwr/prompts/review-rust.md"
  tools = ["Read", "Glob", "Grep"]
  budget = 0.50
  ```
- Invocation: `cwr --profile review-rust @src/main.rs`
- Built-in profiles shipped with the binary:
  - `review` -- code review on changed files
  - `explain` -- explain code at a path/range
  - `fix-build` -- read build error from stdin, propose fix
  - `commit-msg` -- generate commit message from staged diff
  - `summarize` -- summarize a file or piped text

### Async / detached

- `cwr -p "..." --async` prints session id and exits; useful for long
  ones the user wants to background
- `cwr attach <id>` resumes streaming output for a running session
- `cwr wait <id>` blocks until done, then dumps result
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
- Auto-discover `cwr.toml` in cwd for project-level defaults
- `--workdir` (already supported) plus `--worktree` to drop into a
  fresh worktree first (wrapper has the typed builder)

### REPL mode (future, on top of duplex)

A `cwr repl` subcommand for interactive multi-turn work without
opening a full `claude` session. Lighter than claude proper, but
heavier than one-shot `cwr "..."`. The "not quite the full claude
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
- Per-turn footer (same shape as one-shot cwr) plus a cumulative
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
4. **Cost footer on stderr + `--estimate` + `cwr cost`** -- makes cost
   legible without forcing JSON mode
5. **Typed exit codes + auto-retry transients** -- turns the runner
   into a script-friendly building block

Almost all of these lean on primitives that already exist in
`claude-wrapper`. The CLI would be a thin layer of good defaults,
not a from-scratch project.

## Open questions

- Where does this live? New crate in this workspace? Separate repo?
- Binary name. `cwr`? `cq` (claude-query)? Something else?
- Sync default or async default? Sync is simpler for single-prompt;
  async fits long-running ones.
- Color/markdown rendering -- pull in a TUI dep (`ratatui`, `crossterm`)
  or stay minimal with `termcolor`?
- Profiles: ship as TOML or use the existing `cwr.toml` shape we'd
  invent here?
- Does it absorb the `dispatch` crate's async/detach work, or stay
  separate and compose?

## More ideas

(scratch space; add freely)

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

### Tool-call expansion levels

Today's tool-call rendering (both live `--stream` and `cwr last
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
- `cwr last`: same knob, applied to historical replay.

L3 is the interesting one. `tool_result` entries aren't in the
`tool_use` assistant block -- they live in subsequent `user`
entries with content blocks of type `tool_result`, paired by
`tool_use_id`. A small pairing pass over the entry stream would
build the (call, result) pairs. Cheap; not in flight yet.
