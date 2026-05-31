# roba — running design notes

Working document. Brainstorm log, not a backlog. Newest thinking lives near the
relevant section, not strictly at the bottom.

---

## Positioning (load-bearing)

**Claim:** roba sits between `claude` interactive and `claude -p`. It augments the
interactive workflow rather than replacing it.

This is the spec that decides the 0.1 surface. Every flag should be justifiable by
"a person reaching for roba instead of interactive, or instead of bare `-p`, needs
this." If a flag only makes sense in a world where roba is your *primary* driver,
it's probably scope creep toward re-inventing interactive.

### The three-way split, made concrete

- `claude` (interactive): open-ended, multi-turn, you're *in* it. Stateful by being
  a session you live in. Cost: context-switch out of your shell, no composition with
  other shell tools, output not capturable.
- `claude -p`: one shot, dumb. No composition, no session ergonomics, no output
  discipline, decoration leaks into stdout.
- **roba**: one invocation -> one answer, but with (a) composable input, (b) session
  continuity *without living in the session*, (c) clean stdout/stderr split so it's
  a citizen of the pipe. You stay in your shell.

### What "augments interactive" implies for the surface

The augment framing predicts which features are core vs. ornamental:

- **Core -- the things interactive can't do because you'd have to leave your shell:**
  - composition with files / git / globs as prompt input
  - pipe-clean output (stdout = answer)
  - scriptable exit codes
  - cheap session *re-entry* (`-c`, `--resume`, `--fork`, `--pick`) -- you dip back
    into a thread without opening a TUI
- **Edge -- things that drift toward "roba as primary UI":**
  - rich TTY markdown render (nice, but it's interactive's job to be pretty)
  - `--head`/`--tail` (you have a pipe; that's the whole point)
  - streaming with tool-call rollup (this is interactive's strength; why mirror it?)

Open question to resolve before freeze: **is `--stream` in or out of the positioning?**
Streaming-with-live-tool-indicators is the most "I'm reimplementing the interactive
view" feature in the whole surface. If roba's pitch is "you stay in your shell and get
one answer," a live streaming view fights that. Counter-argument: long single answers
benefit from progress signal, and it's still one-shot. Leaning: keep stream but make
it clearly a TTY-only nicety, never the thing the contract is built around.

---

## Surface pruning before 0.1 freeze

The README freezes a *wide* CLI surface (flag names, exit codes, config schema).
Stability promise is good; the width is the risk. Prune signal = real shell history
frequency (the established method), not taste.

### Candidate collapses / cuts (from first read)

- **Output-to-file trio:** `--save PATH`, `--tee PATH`, `--json`. Three routes to
  structured/file output. `--save` reads like `--tee` minus stdout. Consider one flag
  with a mode, or at least decide the orthogonal axes: (format: pretty|json) x
  (destination: stdout|file|both). Today those axes are tangled across three flags.
- **`--head N` / `--tail N`:** arguably reimplementing coreutils. Only justified in
  TTY mode (no pipe to pipe into). And then: do they truncate *rendered* lines or
  *source* lines? Ambiguous against the markdown renderer. Lean cut unless history
  shows real TTY use. _Resolved (#42): cut both flags._ Pipe mode already has
  `| head`/`| tail`, TTY scrollback covers the TTY case, and the source-vs-rendered
  semantics were never pinned down.
- **`--quiet`/`-q` vs `--plain`:** adjacent meanings ("answer only" vs "no
  decoration"). Users will grab the wrong one. Either rename for contrast or document
  the distinction sharply. _Resolved (#43): keep both, clarify the help text._ They are
  orthogonal axes -- `--quiet` is the metadata kill-switch (footer, spinner, tool
  markers), `--plain` the decoration kill-switch (markdown, color, spinner) -- and each
  flag's help now cross-references the other. Renaming would lose the `-q` Unix
  convention and the `--plain` / `NO_COLOR` ecosystem pairing.

### Permissions precedence (needs explicit model)

README shows `--readonly` as *both* the default *and* a preset flag. If it's the
default, the flag only matters as an override against a profile that set
`writable = true`. Define and document precedence for the surprising case:
profile says `writable`, CLI says `--readonly` -> CLI wins (state this). General rule
to pin: **CLI flag > profile field > built-in default**, and within that, does
`--deny-tool` always beat `--allow-tool`? (Probably yes; deny wins.)

---

## Parking lot / to-discuss

- (positioning) finalize stream in/out
- (freeze) run the shell-history frequency pass; list flags by use count
- (output) decide the format x destination axes; collapse the trio
- (perms) write the precedence table into README + profiles.md
