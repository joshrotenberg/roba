# roba

Project context for Claude Code sessions. Read this before changing
anything substantial. Global conventions live in
`~/.claude/CLAUDE.md`; this file adds roba-specific context.

This file is the durable local design home: positioning, decisions,
brainstorm sketches, and the dogfood log. Anything actionable lives
in a GitHub issue. User-facing docs were consolidated 2026-06-07 (the
`docs/` tree + mdbook were retired) into three single-purpose homes
(use-roba skill removed 2026-06-09, #224 -- "docs are the skill"):
the **README** (concepts/positioning + the agent ABI), **`roba --help`**
(the flag/env/config reference, generated from `cli.rs`), and
**`roba-config.sample.toml`** (the annotated, parse-tested config
schema). This file is CLAUDE.local.md (renamed from the untracked
CLAUDE.md, 2026-06-10): the maintainer-private design home, gitignored,
auto-loaded locally. The TRACKED CLAUDE.md is a one-line @AGENTS.md
bridge so Claude Code contributors get the contributor baseline.

## What roba is

A single-prompt CLI runner built on top of
[`claude-wrapper`](https://crates.io/crates/claude-wrapper). The
charter: be what `claude -p` could be -- pipeable on a script,
rich on a TTY, with opinionated defaults that match an "ad-hoc
terminal sidekick" mental model.

The name `roba` is Venetian/Italian for "stuff, things." The
framing: roba returns the stuff you asked about. Not "claude
itself" -- roba is the bag, the answer the contents.

**Status:** v0.3.1 **PUBLISHED** to crates.io (2026-06-08): the
bare-roba-on-a-TTY help blurb (#196/#197, which fixed the dead #187
guard -- verified with a real pty) + four dependency bumps (#195:
dialoguer 0.12, indicatif 0.18, termimad 0.34, toml 1; net Cargo.lock
simplification). Changelog generated cleanly (the #184 cliff.toml fix
holds). Prior release **v0.3.0** shipped the wrapper-sharpening batch --
`completions` (#185), `roba doctor` (#186), no-args-on-TTY guard (#187),
friendlier claude-missing/auth errors (#188) -- all dogfooded through
roba via the full auto pipeline (release-plz -> publish + tag + GitHub
release -> cargo-dist 5-platform binaries + shell/PS/homebrew
installers, homebrew green). The 0.3.0 minor bump was driven by
cargo-semver-checks flagging the new public `SubCommand` variants; we
keep `semver_check = true` (see the 2026-06-08 decisions entry for the
token-403 + version-math story). `brew install joshrotenberg/brew/roba`
works. The
whole distribution path (crates.io + GitHub release artifacts +
homebrew) is validated end to end. Scheduled live-test CI runs daily
against the real API. roba is self-documenting (comprehensive colored
`--help`), and the docs are consolidated (README + `--help` + sample +
skill; the mdbook is gone). See "Where we left off" directly below.

**Important shift, 2026-06-02 evening:** roba is now a **pure
mechanical wrapper** around `claude -p`. The bundled skill+agent
library was unbundled (#130, BREAKING). Skills + agents live
separately at <https://github.com/joshrotenberg/agent-tools>
(private), driven by a dispatcher + runner model. The
"roba is the bag" framing still holds; the agent layer just lives
outside the bag now.

Future: roba may integrate with agent-tools as a dispatch option once agent-tools is stable, but this is TBD and not roba's current concern.

## Where we left off (2026-06-07) -- ACTIVE

### The whole release + docs + CI machine now works

A long arc since 0.2.0 settled the infrastructure:

- **Full release pipeline proven (0.2.1).** A `fix:` (#169) drove the
  release-plz `release-pr` half (#171) -- the previously-untested part --
  then publish + tag + GitHub release + cargo-dist 5-platform binaries +
  shell/PS/homebrew installers. The COMMITTER_TOKEN now has tap access;
  `brew install joshrotenberg/brew/roba` works. dist's failure-isolation
  (upsert + draft-on-failure) held under a real homebrew-token failure.
- **Scheduled live-test CI (#172, #176-#178).** Daily run against the
  real API via an `ANTHROPIC_API_KEY` secret; install/auth validated.
  Lesson banked: **live-API tests assert mechanics you control (flag
  plumbs through, envelope shape, exit codes), never model compliance** --
  the system-prompt/refusal tests were reshaped after model-behavior
  flakiness reddened CI.
- **roba is self-documenting (#180, #181).** `-h` lean, `--help` a
  comprehensive colored reference (env + config sections), generated from
  `cli.rs` so it can't drift.
- **Docs consolidated (#182).** `docs/` tree + mdbook retired; README
  (concepts) + `--help` (reference) + `roba-config.sample.toml`
  (annotated, parse-tested schema) + use-roba skill (agent ABI). The
  sample is embedded via `include_str!` and a unit test parses it.
- **MSRV dropped + fmt hook (#183).** Bleeding-edge binary (no
  rust-version, no MSRV CI job). `.githooks/pre-commit` runs `cargo fmt`
  (`just setup`) -- the real fix for the recurring fmt round-trips, which
  were a background-formatter race, NOT a toolchain mismatch (local and
  CI are both on the same stable; pinning a rust-toolchain.toml was
  rejected because it'd override the CI `beta` matrix row).
- **#173 fixed (#184, in flight).** 0.2.1 shipped an empty changelog:
  `cliff.toml` had `skip_tags = ""` / `ignore_tags = ""`, and an empty
  regex matches every tag, so release-plz's embedded git-cliff skipped
  every release. Removed both lines; backfilled the 0.2.1 section.

**Manual step DONE (2026-06-08):** GitHub Pages disabled via
`gh api -X DELETE repos/joshrotenberg/roba/pages` (GET now 404s). The old
`joshrotenberg.com/roba` mdbook site (orphaned after #182) is gone; the
repo tree was already clean (no book/, CNAME, docs workflow, or docs-site
URLs). No open loose ends from the docs consolidation.

### Backlog state (2026-06-07)

Open issues (all feature/design now -- the infra issues are closed):

- **Design / big bet**: #142 (headless Claude server -- the `roba serve`
  spike; collapses #9 async, #12 REPL, #37 named sessions, #66 --with-mcp)
- **Load-bearing plumbing**: #37 (named session handles -- addressing key for #142/#156)
- **Self-improvement loop**: #156 (`roba meta` -- downstream of #142 + #37)
- **Deferred**: #119 (`roba report`), #59 (cargo-feature gate, blocked on #39), #22 (live-test coverage expansion -- the "run often" half is done via #172)

Closed recently: #30 (cargo-dist), #173 (changelog, via #184), #163
(roba-runner -- closed as an agent-tools concern, not roba).

`git-spawn` (Josh's own crate) was evaluated for the `--git-*` flags and
**parked**: overkill for roba's 3 read-only commands (async + immature
0.1 + roba wants raw text). Revisit if `--worktree` management becomes a
real driver worth dogfooding it on.

### The 0.3.0 fork (still undecided)

The next deliberate `feat` is 0.3.0 (or a `fix` is 0.2.2). Two
directions, neither chosen:

- **A. `roba serve` (#142)** -- the big bet. Fixes the opaque
  `Bash(roba ...)` visibility gap that recurs in the dogfood log.
  High ceiling, real spike risk. See the brainstorm sketch below.
- **B. Sharpen the wrapper** -- quick wins not yet filed (shell
  completions, `roba doctor`, no-args UX).

### Open question worth surfacing

Project `settings.local.json` Bash entries are additive on top of
`--allowed-tools`, so `--readonly` doesn't fully suppress them. Potential
future `--strict` flag. Worth filing if it surfaces as a real need.

## Positioning (load-bearing)

**North Star (2026-06-09):** roba is **a sharp, focused sugaring of `claude -p`
-- that, and that alone.** Not a platform, not "the one thing," not an
unattended-agent-work *system* -- just sugar on `claude -p` that makes it
pipeable, composable, safe-by-default, and session-re-enterable. What you point
it at (CI, an unattended worker, an interactive one-shot) is your business; roba
just makes `claude -p` nicer to use.

**Discipline (Josh, 2026-06-09):** the recurring pull to make roba "the one
thing" is a known, already-tried, doesn't-work failure mode. The better goal is
**sharp, focused tools.** Two-question litmus for ANY proposal: **(1) does it
wrap/sugar running the `claude` binary (pass its flags, clean its I/O), OR
read-only *inspect* what claude produced/stored and report it?** -> IN, it
complements the sugar. **(2) does it MUTATE claude's private `.claude/` state,
run a daemon, or bake orchestration policy?** -> OUT, a DIFFERENT tool. So
read-only **managerial/inspection** commands (`history`, `cost`, `doctor`,
`show` #220, `worktree list` #217, history filters #218) are IN -- they make the
sugar more useful; the hard line is **mutation / daemon / orchestration**, not
"management." This is why #222 (claude's own worker flags) AND the read-only
managerial cluster are IN, while #224 ditches roba-shipped skills, #193 is
wontfix, and serve/#142 (a daemon) is a *separate tool*. (Two half-steps off,
both corrected here: the "unattended agent work" framing, and a brief
over-rotation that scrutinized read-only management as if it were creep -- it
isn't, as long as it stays read-only.)

**Claim:** roba sits between `claude` interactive and `claude -p`. It
augments the interactive workflow rather than replacing it.

This is the spec that decides the 0.1 surface. Every flag should be
justifiable by "a person reaching for roba instead of interactive, or
instead of bare `-p`, needs this." If a flag only makes sense in a
world where roba is your *primary* driver, it's probably scope creep
toward re-inventing interactive.

### The three-way split

- **`claude` (interactive):** open-ended, multi-turn, you're *in* it.
  Stateful by being a session you live in. Cost: context-switch out of
  your shell, no composition with other shell tools, output not
  capturable.
- **`claude -p`:** one shot, dumb. No composition, no session
  ergonomics, no output discipline, decoration leaks into stdout.
- **roba:** one invocation -> one answer, but with (a) composable
  input, (b) session continuity *without living in the session*, (c)
  clean stdout/stderr split so it's a citizen of the pipe. You stay in
  your shell.

### What "augments interactive" implies for the surface

- **Core -- things interactive can't do because you'd have to leave
  your shell:**
  - composition with files / git / globs as prompt input
  - pipe-clean output (stdout = answer)
  - scriptable exit codes
  - cheap session *re-entry* (`-c`, `--resume`, `--fork`, `--pick`) --
    you dip back into a thread without opening a TUI
- **Edge -- drifts toward "roba as primary UI":**
  - rich TTY markdown render (nice, but it's interactive's job to be
    pretty)
  - streaming with tool-call rollup (this is interactive's strength;
    why mirror it?)

roba flags split into agent-tier (stable ABI, typed exit codes, deterministic precedence) and human-tier (markdown render, spinner, color) concerns.

### The scope line: what roba is, and is not (load-bearing, 2026-06-09)

The orchestration research (#210-213) surfaced a gravitational pull toward
"roba as a Claude *management* tool" (manage sessions, prune worktrees,
orchestrate runs). The deliberate line, stated from the top:

**roba is primarily a `claude -p` wrapper. In some cases it adds
*read-only* management/inspection commands. It does NOT mutate claude's
private file state.**

- **Primary identity -- a `claude -p` wrapper.** Single-prompt runner,
  composable I/O, clean ABI, session re-entry. The mechanism.
- **Allowed secondary -- read-only management commands.** `roba history`,
  `roba cost`, `roba doctor`, `roba profile/alias` (inspect) -- all
  *read* claude's state or roba's own config and *report*. This is naming
  what roba already does, not a new capability. Reading claude's session
  JSONL (for `history`/`cost`) is the established, accepted coupling;
  extend it cautiously.
- **The hard boundary -- no mutation of claude's domain.** roba does NOT
  write/prune/manage `.claude/` (session JSONL, worktrees, credentials,
  settings). That's claude's private, undocumented, version-fragile
  domain. Mutating it makes roba brittle AND drifts it toward what
  Anthropic's hosted **Managed Agents** / `ant` CLI already own at the API
  layer. (Decisive example: `-w` worktrees are created by the *claude
  binary* under `.claude/worktrees/`; roba only passes `--worktree`. So a
  `roba worktree prune` would be roba mutating claude's files -- out of
  bounds. Worktree cleanup belongs upstream in claude-code, or to the
  orchestrator/skill.)
- **Litmus for any new flag/subcommand:** does it (a) wrap the `claude`
  binary, or (b) *read-only*-inspect claude/roba state and report? If yes
  -> in. If it bakes orchestration *policy* into the binary -> out, to a
  **skill** (#193). If it's a management *service* (a daemon) -> out, to a
  **deliberate, feature-gated (#59), separable** component (serve #142 /
  meta #156), never accidental creep in core `roba`.
- **Bonus framing (Josh):** a wrapper command that *calls* `claude` is
  good; a command that reaches into `.claude` to manage files is the line.

**Landscape note:** the hosted "managed Claude session" space is already
Anthropic's -- the `ant` CLI (REST-API wrapper) + **Managed Agents** (create
agent -> start session -> stream events; Anthropic runs the loop +
container). Those operate on the **API/platform** layer (API-key billed,
hosted). roba's distinct niche is the **local `claude` binary** layer
(subscription/OAuth auth, your machine, your files). serve (#142) would be
the *local-CLI counterpart* of Managed Agents -- a reason to keep it
deliberate and NOT rebuild the managed-agents control plane locally.

## Architecture

Standalone Rust crate, edition 2024, rust-version 1.90.0. Has both a
lib and bin target so integration tests can drive the same code paths.

```
src/
├── main.rs       entry point only; calls roba::dispatch
├── lib.rs        dispatch + run_ask + classify_exit_code
├── cli.rs        clap structs (Cli, SubCommand, AskArgs, *Args)
├── env.rs        ROBA_<PARAM> env-var override layer
├── error.rs      JSON error envelope shape
├── prompt.rs     input + composition + templating + git + attach
├── output.rs     formatters (footer, count, duration, refusal, code)
├── render.rs     presentation policy: termimad, color, spinner
├── session.rs    apply_session / apply_permissions
├── history.rs    history + last subcommands + --pick chooser
├── stream.rs     streaming pipeline + tool handling
├── profile.rs    profile schema + lookup pool + auto-apply
├── aliases.rs    user-defined `[alias.NAME]` shortcuts + expansion
└── cost.rs       cost / usage rollup across history

tests/
├── cli.rs        mechanical (assert_cmd, no claude calls)
└── live.rs       real claude calls, #[ignore] by default

docs/
├── README.md     index
└── profiles.md   user-facing reference for roba.toml + profiles
```

`docs/` is intentionally lean: only user-facing reference material.
Design / brainstorm / decisions log live in *this file*. Implementable
items live in GitHub issues.

## Design principles (the non-negotiable ones)

### 1. Safe by default

Default permissions are read-only: claude can use Read, Glob, Grep and
nothing else unless the user explicitly opts into more. Opt-ins:
`--writable` (adds Edit, Write), `--allow-tool TOOL` (adds one),
`--full-auto` (bypass all checks). This came from a real "ooops" --
streaming session quietly let claude run `git checkout` to a new
branch when the user just wanted a chat. Don't regress this default.

### 2. stdout = answer, stderr = metadata

A roba invocation pipes cleanly. Everything that ISN'T the answer goes
to stderr: cost footer, spinner, tool-call lines, refusal warnings,
error messages. So `roba "foo" | jq` always sees the answer and only
the answer.

### 3. Transient TUI, plain shell when done

On a TTY roba briefly becomes a lightweight UI (rendered markdown,
indicatif spinner, dim metadata, colored markers). When the call
completes, the UI evaporates and the shell prompt returns. No
persistent UI, no "launch a full TUI" mode -- just a transient panel
that exists exactly as long as roba is doing something.

### 4. Profiles are just defaults

A profile is a TOML alias for flags you'd otherwise type every time.
Nothing magical. CLI flags always override profile values. Config
resolution (highest layer wins on a given knob):

1. CLI flag
2. `ROBA_<PARAM>` env var (lists comma-separated, vars per-key via
   `ROBA_VAR_<KEY>`, truthy bools only enable)
3. Active `[profile.NAME]` overlay in the file pool
4. Top-level keys in the file pool
5. roba's built-in defaults
6. claude's defaults

Files compose: `~/.config/roba.toml`, then every `roba.toml` walking
up to the git root. Closer-to-cwd files override farther ones per-key;
lists concat across files; vars merge per-key.

Auto-apply order when no `--profile NAME` is passed:

1. `--no-default-profile` -> no profile
2. `ROBA_PROFILE=NAME` env -> apply NAME
3. `default` profile in pool -> apply silently
4. otherwise no profile

### 5. `--plain` is the master kill-switch

Disables markdown render, spinner, and color in one flag. Pairs with
`NO_COLOR=1` for environment-wide off.

## Current surface (v0.1.0)

Everything below is implemented and tested.

### Prompt sources

| Flag | Notes |
|---|---|
| positional `[PROMPT]` | Pass `-` for explicit stdin |
| `-f, --file PATH` | Read prompt from a file |
| `-e, --editor` | Compose in `$VISUAL` / `$EDITOR` (falls back to vi) |
| piped stdin | Detected automatically; no flag needed |

### Composition

| Flag | Notes |
|---|---|
| `--prepend PATH` (repeatable) | File contents prepended to prompt |
| `--append PATH` (repeatable) | File contents appended |
| `--attach GLOB` (repeatable) | Glob-matched files embedded with `File: PATH` framing |
| `--git-diff` | Embeds working-tree diff |
| `--git-log [N]` | Embeds last N commits (default 5) |
| `--git-status` | Embeds short status |
| `--var K=V` (repeatable) | Substitute `{{KEY}}` placeholders |

### Output

| Flag | Notes |
|---|---|
| `-q, --quiet` | Suppress metadata: footer, spinner, tool markers. For rendering off, see `--plain`. |
| `--json` | Full structured record as JSON on stdout (or as error envelope on failure) |
| `--code [LANG]` | Print only fenced code blocks |
| `-o, --out PATH` | Write the result to a file AND to stdout; extension drives format (`--json` forces JSON) |
| `--stream` | Live tokens as they arrive |
| `--show-thinking` | Render extended-thinking blocks live (requires streaming) |
| `--echo` | Print the resolved prompt before the response |
| `--plain` | Disable markdown rendering, color, and spinner. Footer still prints; for answer-only, see `--quiet`. |

### Sessions

| Flag | Notes |
|---|---|
| `-c, --continue` | Continue most recent session in this directory |
| `--resume ID` | Resume specific session |
| `--fork` (requires `--resume`) | Branch the resumed session |
| `--pick` | Interactive fuzzy chooser |
| `--fresh` | Force a new session despite resolved `continue = true` |
| `-w, --worktree [=NAME]` | Run in a fresh git worktree; pin the name with `-w=NAME` |

Subcommands: `roba history`, `roba last [-n N] [--type {text,tools,all}]`,
both scope to the current cwd's project by default (`--all-projects`
widens).

### Permissions

| Flag | Notes |
|---|---|
| (default) | Read, Glob, Grep only |
| `--readonly` | Explicit form of the default (no-op) |
| `--writable` | Adds Edit + Write |
| `--allow-tool TOOL` (repeatable) | Add a specific pattern |
| `--deny-tool TOOL` (repeatable) | Block a pattern |
| `--full-auto` | Bypass all checks (sandbox use only) |

### Profiles

| Flag | Notes |
|---|---|
| `--profile NAME` | Apply named profile from the merged pool |
| `--no-default-profile` | Skip auto-apply (env + `default`) |
| `roba profile {list,show,init,path,active}` | Inspect / manage the config |

### Aliases (post-0.1.0, #88)

| Invocation | Notes |
|---|---|
| `roba NAME [args]` | Expand `[alias.NAME]` (template + flags + agent) and dispatch |
| `roba alias {list,show,path}` | Inspect aliases in the merged pool |

`template` supports `${1}`/`${@}` (positional), `${pr}` (named via an
`args` schema), `$$` (literal `$`), and `$(...)` (shell substitution,
user's shell, not sandboxed). A template-less alias is a flag-shortcut
(args become the prompt). Built-ins win the lookup; a shadowing alias
warns at load.

### Cost (usage rollup, not dollars)

| Flag | Notes |
|---|---|
| `roba cost` | Total tokens across history |
| `roba cost --by-project` | Per-project breakdown |
| `roba cost --json` | Machine output |

Dollar amounts not implemented (claude-code persists tokens to JSONL
but not per-session cost). Would require a per-model rate table;
tracked as #11.

### Other

| Flag | Notes |
|---|---|
| `-C, --cwd PATH` | Run prompt as if invoked from a different directory (`git -C` style) |
| `--model NAME` | Override the model for this call |

## Decisions log

Chronological. Each entry: what was decided, the rationale, the PR or
issue that captures the work. Detailed reasoning sits in the commit
messages and PR bodies; this log is the index.

### Pre-2026-06-02 (archived)

Full decisions log for 2026-05-24 through 2026-06-01 lives in git history; see commit messages and PR bodies.

### 2026-06-10

- **AGENTS.md: skip (researched, 4-agent workflow).** Floated as the "instead of
  skills" answer + a larger doc file. Both fail: AGENTS.md serves CONTRIBUTOR-agents
  editing the repo that contains it (nearest-file-in-tree discovery) -- there is NO
  delivery path to CONSUMER-agents in other repos (the deleted use-roba skill's
  audience), and zero prior art for using it as consumer-CLI docs. Claude Code
  (roba's entire agent traffic) does NOT read AGENTS.md natively (official docs;
  claude-code#6235 open, 5,300+ reactions) -- it would need a symlink shim. And it
  would be a 4th hand-maintained doc home -- the drift surface #224 just removed.
  The README "For agents & scripts" section IS the fetchable agent-usage doc (GitHub
  renders it, crates.io carries it, agents WebFetch READMEs by habit). **Revisit
  triggers:** Claude Code ships native AGENTS.md support, OR a real non-Claude
  contributor appears -> then add a sub-50-line CONTRIBUTOR-facing AGENTS.md
  (build/test commands + a pointer to the README ABI), never a usage doc.

### 2026-06-09

- **Managerial-cluster placement eval: all three ship NOW, zero wrapper PRs
    (#217/#220/#218).** Ran the wrapper-vs-roba placement eval Josh asked for (4-agent
  workflow, grounded in claude-wrapper 0.11 source). **Decisive:** the placement
  principle's expensive case (push a MISSING primitive down into the wrapper, eat
  PR->release->dep-bump latency) DOES NOT ARISE -- the reusable claude-domain data
  primitives ALREADY live in claude-wrapper 0.11, correctly placed:
  `read_session`/`SessionLog`/`HistoryEntry` (show + #218); a complete dedicated
  `worktrees` module (`WorktreeRoot::for_repo().list()`, Serialize-derived, shells to
  `git worktree list --porcelain`) for #217. roba already exercises that coupling in
  `cost.rs`/`history.rs`. So all three are HYBRID where the wrapper half already
  exists -> roba-only thin-CLI work. **Build order:** #217 worktree list (cleanest)
  -> #220 show -> #218 history --worktree. **Caveats:** show returns a RECONSTRUCTED
  envelope (only result+session_id rebuild from JSONL; cost/duration/turns null);
  `--wait` has no explicit completion marker (roba-side heuristic); #218 `--agent` has
  NO clean datum (defer/alias -- agentId on zero entries; worktree dirs are literally
  `agent-<hash>` so agent==worktree handle today); worktree list is a git-porcelain
  SUPERSET of claude's worktrees (--help wording). ONE optional non-blocking wrapper PR
  (SessionSummary.cwd) as a later #218 optimization. **Confirms Josh's instinct** that
  the inspection logic belongs near the binary -- claude-wrapper already did exactly that.

- **#222 guardrails: exit-code finding -> a claude-wrapper enhancement candidate
    (PR #226).** `--max-turns` + `--max-budget-usd` landed as pass-throughs. The
  locked "`--max-turns` gets a new typed exit code" decision resolved to **NOT
  FEASIBLE**: `claude_wrapper::Error` has no max-turns/turn-limit variant, so hitting
  the cap surfaces as `Error::CommandFailed` -> generic exit 1 (the worker honestly
  did NOT fabricate a code). Bonus finding: `Error::BudgetExceeded` (exit 3) is
  constructed ONLY by the wrapper's own `BudgetTracker`, NOT from claude's
  `--max-budget-usd` flag -- so that flag's cap also exits 1, not 3. **Implication
  (the wrapper-placement principle in action):** typed exit codes for cap-hits would
  require claude-wrapper to parse claude's error output and surface typed variants
  (`MaxTurnsExceeded`, a claude-side budget variant), which roba would then map in
  `classify_exit_code`. That's an additive **claude-wrapper** enhancement, not a roba
  hack -- file upstream if/when it's worth it.

- **"Sharp, focused tools" over "the one thing" -- roba is `claude -p` sugar,
    that alone (Josh, 2026-06-09).** Resolves the skills tension. The pull toward
  "convenience layer for unattended agent work" (my phrasing earlier this day) was
  the recurring, already-tried, doesn't-work impulse to build one do-everything
  tool. Correction: build sharp, focused tools; roba is already a good `claude -p`
  sugaring -- let it be that and that alone. Litmus for any future proposal: is it
  sugar over the `claude` binary? If it's a NEW capability `claude -p` lacks
  (orchestration / daemon / skills library / session management), it's a DIFFERENT
  tool. Consequences: **#224 resolved -> ditch roba-shipped skills** (docs are the
  reference; orchestration procedure lives in agent-tools, a different tool);
  **#193 -> wontfix** (no starter skills set); **#222 stays IN** (passing through
  `claude -p`'s own flags is faithful sugaring, not creep); **serve/#142 reinforced
  as a *separate tool*, not a gated roba feature**; **#220/#217/#218 (read-only
  management) RESOLVED in-scope** (see the refinement entry below). North Star
  above rewritten to match.

- **Refinement: read-only managerial commands COMPLEMENT the sugar (Josh,
    2026-06-09).** The "sharp focus" pass briefly over-rotated into scrutinizing
  read-only management (I parked #217/#220 as "question"). Josh pulled it back:
  under the claude-p-sugar framing, read-only managerial/inspection commands
  (`show`, `history`/`cost`/`doctor`, `worktree list`, history filters) COMPLEMENT
  the goal -- they make using `claude -p` more convenient. The line that keeps roba
  sharp is **mutation / daemon / orchestration**, NOT "management": read-only
  inspect+report = IN; write/prune `.claude/`, run a persistent process, or bake
  policy = OUT (a different tool). Reconciles with the original scope-line section
  (which always allowed read-only management). Reclassified #217/#218/#220 ->
  in-scope (un-parked); the two-question litmus in Positioning is the durable form.
  serve/#142 stays OUT (it's a daemon -- fails Q2).

- **Placement + sequencing: inspection primitives -> claude-wrapper; roba stays
    the thin CLI; obvious flags first (Josh, 2026-06-09).** Two parts. (1)
  **Sequencing:** do the OBVIOUS pass-through flags (#222) FIRST -- they're
  unambiguous sugar -- then come back around to the managerial cluster
  (#220/#217/#218). (2) **Placement:** the #222 flags are already wrapper-side
  (claude-wrapper 0.11 exposes the builder methods; roba just wires the CLI). The
  managerial commands are net-new *inspection logic over claude's artifacts* (read
  session JSONL -> result, enumerate worktrees) -- per the existing "prefer additive
  changes to claude-wrapper" convention, evaluate per-command whether that primitive
  belongs as a reusable **claude-wrapper API** (close to the binary, where
  claude-domain knowledge should accrete, reusable by other consumers) vs directly
  in roba. **Default: push the primitive DOWN into claude-wrapper, keep roba a thin
  CLI sugar over it.** Keeps roba sharp and the claude-knowledge in one reusable home.

- **`claude -p` usage research -> sharpened positioning + a pass-through-flags
    roadmap (#222/#223/#224).** Ran a 6-agent multi-modal sweep (flag surface via
  live `claude --help` v2.1.167, in-the-wild `gh search code`, community recipes,
  our dogfood corpus, adjacent SDK/tools) -> coverage map + scope-line-filtered gap
  list. **Verdict:** roba covers/improves ~80-85% of real `claude -p` usage by
  frequency and beats raw `-p` on what matters (safe-by-default tools, stdout=answer/
  stderr=metadata, versioned `--json`, typed exit codes, composition, session
  re-entry). The **entire** gap list is one shape: a second tier of unattended-worker
  / agent-ABI flags that **claude-wrapper 0.11 already exposes as `QueryCommand`
  builder methods** but roba never wired -- each a pure pass-through (clap field + one
  `apply_session` line, the 12-step checklist), NOT a wrapper PR. **Adversarially
  verified** all 14 methods exist in `claude-wrapper-0.11.0/src/command/query.rs` with
  exact signatures (caught + corrected a getter/setter false alarm on `session_id`;
  confirmed `mcp_config`/`add_dir` push to Vecs = repeatable). High tier: `--session-id`
  (also closes #37 friction + the `--continue`-no-ops-in-print-mode multi-turn
  footgun), `--max-turns`, `--json-schema`, `--mcp-config`(+`--strict-mcp-config`).
  Med: `--add-dir`, `--max-budget-usd`, `--fallback-model`, `--no-session-persistence`.
  Low/deferred: `--agents` JSON + 6 settings flags (one "on demand" batch, no thin
  PRs). Anti-scope held clean (retry loop -> skill; duplex/serve -> #142; Unix
  `timeout`; SDK callbacks; `--debug` mirroring; interactive/IDE flags; claude mgmt
  subcommands; GH-native CI ergonomics; hosted sessions; `worktree prune`). Corpus
  bonus: roba's own `-w` gets ZERO real dispatch use (real isolation = orchestrator
  `git worktree add` + `-C`) -> a `--help` note, not a flag. Filed **#222** (umbrella
  flags), **#223** (docs align), **#224** (ditch skills). 3 open decisions carried in
  #222: `--session-id` mint+record in roba's own config (in-bounds, reopens the
  read-only #37 call); `--json-schema` auto-force `--output-format json` (footgun fix);
  `--max-turns` new typed exit code.
- **Positioning sharpened: "a convenience layer over `claude -p` for running
    unattended agent work" (Josh, 2026-06-09).** Deliberate reweight of the center of
  gravity from "augments interactive / sits between interactive and `-p`" to the
  **unattended-worker** identity -- which is exactly where 100% of the coverage gaps
  live. Attended human one-shots still work (a subset of "convenience over `claude
  -p`"), just no longer the headline. Drives the #223 docs-alignment pass. Possible
  downstream: a later pruning look at the human-tier surface (rich-TTY render,
  spinner, `--pick`) if the narrower reading is intended.
- **"Docs are the skill / less is more" -- ditch roba-shipped skills? (#224, open).**
  Josh floated (contrary to "roba + skill library is a winning combo"): roba's
  `--help`/README/parse-tested config sample ARE the reference, so a separate shipped
  `use-roba` skill is pure drift surface -- ship ZERO skills (#130 taken to its end).
  Reconciliation: REFERENCE (the ABI) = docs; PROCEDURE (dispatch-or-inline, PR
  lifecycle, prompt templates) = the separate agent-tools fleet, never roba-shipped.
  So this does NOT contradict "policy lives in skills" -- it relocates the skills out
  of roba entirely. **Gates #193** (the starter-kit broadening): resolve #224 first;
  if "ditch" wins, #193 is wontfix. Deferred -- "a different problem."

- **Native-subagent-detail gap: "won't fix -- wrong door" (two front doors).**
  Q (Josh): driving roba from a harness Workflow via `Bash(roba ...)` doesn't
  give the live detail/parallelism of native `agent()` subagents -- can roba
  integrate the Workflow-style detail and expose it over MCP/REST? Settled via
  a 5-lens adversarial panel (workflow `roba-native-detail-gap`, 6 agents,
  ~478K tok). **Decisive constraint:** native detail is the *harness
  instrumenting a child it spawned directly*; roba's `claude -p` is a
  **grandchild** one process-level below the harness's visibility boundary, so
  NO roba-side output (stdout/stderr/new fd/socket/NDJSON ABI) can inject nodes
  into the parent's `/workflows` render tree. Structural, not a missing
  feature. **Verdicts:** (do-now) **two front doors** -- native `agent()` when
  you want to watch/steer a live fan-out; `roba` via Bash when you want an
  isolated, costed, history-tracked, CI-graded worker; route by context, the
  dogfood log already does. (already-have-it) `--trace` + `--json`: full
  after-the-fact fidelity -- *verified* `--trace` drives
  `run_streaming(DisplayMode::Silent)` even under `--json` (src/lib.rs:217), so
  the per-tool event log is captured on every path today; zero live UI.
  (reject) a standalone NDJSON event-stream ABI -- duplicates `--trace`, heaviest
  stability promise in the codebase, still no native UI. (gated) caller-shaped
  structured `--json` return -- the one native feature a Bash child *can* match,
  but the near-term form is a **skill convention** (ask for JSON findings, parse
  `--json`), already works, not a binary feature until a schema-validation need
  survives dogfooding. (gated) **serve/#142 MCP worker** -- the only path that
  changes the front door, but MCP progress is `progressToken + number + one
  string` and the harness collapses every MCP tool to one row, so it buys
  native-*looking* + a structured return + a coarse, **version-fragile** spinner
  label (harness issues #4157 closed-not-planned, #51713 regression), NEVER the
  23-agent tree. So #142 is justified by addressable-worker + structured-return
  + ledger value, *never* by "MCP gives live detail." **Build nothing in core;**
  the move is two doc notes (the two-doors routing rule + documenting that
  `--trace` is claude-code's raw stream-json passthrough, currently undocumented/
  drift-prone). The parallelism itself is NOT lost (fan out N `Bash(roba --json)`,
  fan-in parsed envelopes) -- only the live *tree visualization* is, and that's
  the grandchild fact. Holds the scope line: zero binary surface, orchestrator =
  Workflow tool, worker = roba.

### 2026-06-08

- **Wrapper-sharpening series shipped via roba dogfood (#185-#188).**
  Four quick wrapper-quality wins, each dispatched through roba and merged
  on green: `completions` subcommand (#185/#189, clap_complete),
  `roba doctor` health check (#186/#190), no-args-on-TTY guide-don't-hang
  (#187/#191), and actionable claude-missing/unauthenticated hints
  (#188/#192). All four runs were clean (zero spirals). The series
  validated the dispatch loop end to end as a *durable-context* machine.

- **Dispatch loop: full prompt in the PR body + `--trace` visibility.**
  Two refinements to the orchestration pattern, both load-bearing for
  durability: (1) embed the *entire* dispatch prompt in the draft PR body
  (in a `<details>` block) rather than referencing a `/tmp/roba-task-N.md`
  path -- the path evaporates, the PR is forever, so the PR becomes a
  self-contained record of intent + diff + CI + merge. (2) Run roba with
  `--trace PATH` and a small `trace-digest.sh` jq filter to turn the
  opaque `Bash(roba ...)` subprocess into a live tool-by-tool window. The
  prescriptive `roba-dispatch` SKILL was deliberately NOT baked in -- the
  user's call was "encode it as an example, not a required skill."

- **0.3.0 shipped; release-plz token + pre-1.0 version math (#194).**
  Merging #189-#192 fired release-plz on each push, but it 403'd at PR
  creation four times -- leaving four orphan `release-plz-*` branches and
  no release PR. Root cause: the workflow passes `COMMITTER_TOKEN` (a
  fine-grained PAT) as release-plz's `GITHUB_TOKEN`, and that PAT had
  `Contents: write` (so branch pushes succeeded) but **not `Pull
  requests: write`** (so `POST /pulls` 403'd: "Resource not accessible by
  personal access token"). Latent since the token was re-scoped for the
  homebrew tap -- only surfaced now because this was the first
  release-PR-opening run since. Fix: add Pull requests: write to the PAT,
  then `gh workflow run release-plz.yml`. **Version math learned:** roba
  is pre-1.0, where release-plz maps *breaking -> minor (0.2->0.3),
  feat/fix -> patch (0.2.2->0.2.3)*. The 0.3.0 came from cargo-semver-
  checks flagging the new public `SubCommand` variants (Completions #189,
  Doctor #190) as breaking -- the `feat:` commits alone would have been
  0.2.3. `#[non_exhaustive]` would NOT have kept this batch on 0.2.x
  (adding the attribute is *itself* a breaking change -- `enum_marked_
  non_exhaustive`); only `semver_check = false` would (bump by commit
  type only). Decided to **keep `semver_check = true` and ship 0.3.0** --
  the API hygiene is worth more than avoiding a minor bump. Full pipeline
  green incl. `publish-homebrew-formula` (the job that 403'd pre-fix), so
  the homebrew path is re-validated. Cleaned up the four orphan branches.

- **Suggested-skills-set idea filed (#193).** roba ships one skill today
  (`skills/use-roba`, the agent ABI). #193 is a design issue to think
  about a small companion *suggested, opt-in* set ("how to drive roba
  well": roba-dispatch, prompt-template, trace-visibility,
  spiral-diagnosis) -- never bundled into the binary (don't regress
  #130), copy-to-`~/.claude/skills/` like use-roba. Key open question:
  the boundary vs the private agent-tools fleet (roba ships the generic
  public statement; agent-tools keeps the multi-agent orchestration
  layer). Not started -- a place to think.

- **roba serve (#142): don't kill, don't ship -- build toward it behind a
  gate.** Pro/con'd the headless MCP server. Verdict: the maintenance of a
  persistent daemon doesn't clear for a single-user internal loop *yet*,
  and this session proved the workarounds (`--trace` + `trace-digest.sh`,
  `run_in_background` + notification polling) are tolerable at zero
  maintenance. But the design has sharpened (transport + ledger, v1
  excludes duplex/REPL/scheduler) and engineered around what killed the
  prior claude-server (it's a near-*stateless broker*: state lives in
  claude's on-disk session records, jobs are orphan-able). Decision: serve
  is a bet on **#156 (meta-session/RSI)**, not on itself -- let #156 demand
  pull it in. Staged path: prove the #156 *grooming* half via `Bash(roba)`
  skills in agent-tools first (RSI half deferred -- its validation proxy is
  weak and it auto-promotes into `~/.claude/`, high blast radius); build
  serve behind a **cargo feature gate (#59)** when proven ("in-tree but
  unreleased" == default-off feature); spike `session_id` timing +
  tower-mcp first.

- **Stateless-substrate firewall (load-bearing principle).** roba owns
  **zero load-bearing runtime state** today -- it's a pure function of
  `(args, env, config, claude's on-disk session records)`. This is what
  makes serve's thin-broker design even possible. Firewall to hold:
  *disposable, self-healing caches are OK (there are none yet); roba owns
  no runtime state -- that's serve's job, gated/unreleased.* Two kinds of
  "stateful" are worlds apart: a reconstructible cache (mild) vs.
  load-bearing runtime state like live jobs/queues (the daemon, the scary
  kind). Don't let the first cache grease the slope -- name the line.

- **#37 reframed: stateful tag ledger -> declarative `[session.NAME]`
  config.** The named-session need (address a session by a stable handle,
  not a UUID) first looked like roba's *first owned mutable state* (a
  tag->uuid map) -- a Rubicon. Confirmed claude-wrapper has **no
  session-naming API** (sessions are UUID-only; `derive_session_name` is
  display-only), so *someone* must hold the map. Resolution (Josh's call):
  make it **config**, not state. A `[session]` table in the roba.toml pool
  (`NAME = "<uuid>"`), parsed exactly like `[alias]`/`[profile]`, read-only
  to roba -- design principle #4 ("profiles are just defaults") already
  covers it. `--session NAME` + `ROBA_SESSION=NAME` resolve NAME->uuid via
  the pool, then set `continue_session = Some(Some(uuid))` so
  `apply_session` is unchanged. The only friction -- you can't bind a name
  until the session exists (UUID is minted on first run) -- is a fine
  one-time cost for the actual use case (a handful of long-lived sessions:
  a `meta` driver, maybe one per active repo), and far cheaper than owning
  a self-healing cache or writing to config. UUIDs are machine-local, so
  the `[session]` block lives in untracked/local config. No `Profile`
  struct change -- it's a pool-level map, sibling to aliases. This is the
  version of #37 actually worth building; the stateful ledger is dropped.

- **`--dispatch` preset removed (#202/#203, BREAKING -> 0.4.0).** Driven by
  a roba *usage-research dispatch* over a real cross-project corpus (the
  mcp-proxy CI-fix + 4-issue effort: 4 task files + 2 traces, on disk). The
  research found `--dispatch` (= `--full-auto` + `--worktree` + `--fresh`)
  fit NEITHER real dispatch shape -- both the in-place (#173) and
  orchestrator-owned-named-branch (#177) cases used bare `--full-auto`,
  because the bundled `--worktree` is the wrong default (it isolates into an
  anonymous worktree you don't want for in-place work or a branch you'll PR
  from the main checkout). **Zero use** across ~15 real dispatches incl.
  every dogfood run this session. The tell that settled it: the docs PR
  (#201) had to say "here's `--dispatch`, but don't use it for the common
  case" -- when a flag's docs steer you away from it, the flag is the
  problem. Removed the full layered plumbing (flag + `Dispatch` heading +
  `ROBA_DISPATCH` + `Profile.dispatch` + tests; the inverse of the
  Adding-a-CLI-flag checklist; +30/-141). The primitives `--full-auto` /
  `--worktree` / `--fresh` all stay; the composition is now taught in a
  primitives-only `--help` "Dispatch modes" section + the use-roba skill.
  **Principle banked:** a preset that bundles orchestration opinion is the
  flag-level twin of a prompt-scaffolder -- composition knowledge belongs
  in the skill/`--help`, not a preset that picks a default. #201 was closed
  (superseded) and its keeper content (the `post_turn_summary` trace note)
  folded into #203 to avoid documenting a flag we were deleting.

### 2026-06-07

- **Docs consolidation -> README + `--help` + sample + skill (#180-#182).**
  The doc surface was four overlapping places (book + `docs/` + README +
  onboard help), guaranteed to drift -- and the mdbook shipped 100%
  broken (#166) before anyone noticed. Collapsed to four single-purpose,
  drift-resistant homes: `--help` owns the flag/env/config reference
  (generated from `cli.rs`); the README is the conceptual front door
  (absorbed `vs-claude-p` positioning); **`roba-config.sample.toml`** (at
  repo root, `include_str!`'d so `roba profile init` writes it, and a
  `sample_config_parses` unit test deserializes it -> config docs that
  can't go stale) owns the roba.toml schema; the use-roba skill owns the
  agent ABI. Deleted the entire `docs/` tree + `book/` + `docs.yml`. The
  `--help` got comprehensive first (#180 short/long doc split so `-h`
  stays lean; #181 color via a clap `Styles` palette -- auto-stripped on
  a non-TTY, so the agent ABI stays byte-clean -- plus env/config
  `after_long_help` sections). **Hot-take that paid off:** an annotated
  sample TOML beats a `config.md` -- it's valid TOML by construction, a
  copy-to-use artifact, and embeddable. **Pending manual step:** disable
  GitHub Pages.

- **Drop MSRV; fmt pre-commit hook (#183).** roba is a binary, so the
  self-imposed `rust-version = 1.90.0` + MSRV CI job were pure friction.
  Removed both. The recurring fmt CI round-trips (#179 + twice on #182)
  were diagnosed NOT as a version mismatch (local and CI are the same
  stable) but a **background formatter racing edits** -- reformatting
  after a manual `cargo fmt` but before commit. Fix: `.githooks/pre-commit`
  runs `cargo fmt --all -- --check` at commit time (`just setup` enables
  it). Deliberately did NOT add a `rust-toolchain.toml`: it'd override the
  CI `beta` matrix row, and there was no version drift to fix.

- **release-plz empty changelog -- root-caused + fixed (#184, closes #173).**
  0.2.1 published with a totally empty changelog. Reproduced locally with
  `RUST_LOG=debug release-plz update` (+ a throwaway `fix:` commit to make
  it release-triggering): release-plz collected the commits and handed
  them to git-cliff, which logged `Skipping release: X`. Root cause:
  `cliff.toml` set `skip_tags = ""` / `ignore_tags = ""`, and **an empty
  regex matches every tag**, so release-plz's *embedded* git-cliff skipped
  every release. (Standalone `git cliff` 2.13.1 treats empty as "no skip"
  -- a version-behavior difference that masked the bug and made `git
  cliff` output look correct.) Fix: remove both lines + a comment so they
  don't return. Backfilled the empty 0.2.1 section. **Lesson:** a confounded
  local repro (the macOS `/var`->`/private/var` symlink also breaks
  release-plz's per-commit file attribution) sent the first investigation
  down a wrong path; the debug trace (`Skipping release`) was the tell.

- **Live-test CI hardening; the model-compliance lesson (#172, #176-#178).**
  Stood up a daily scheduled live-test workflow against the real API
  (`ANTHROPIC_API_KEY` secret). First runs surfaced a `bash -e` install
  bug (#176) and -- more durably -- that two tests asserted *model
  compliance* (claude obeying a replacement `--system-prompt` / producing
  a refusal marker), which haiku does ~50% of the time. No wording fixes
  a coin flip. Reshaped both to deterministic assertions (smoke +
  `refusal: false` on a normal answer); the real coverage already lived
  in unit tests + the reliable additive-system-prompt live test.
  **Banked: for live-API CI, assert the mechanics you control, never that
  the model complies.** Also: local 5/5 != CI green for model-dependent
  tests -- CI (2.1.167, API-key auth) was stricter than local (2.1.162,
  OAuth).

- **git-spawn evaluated + parked.** See "Backlog state" above.

### 2026-06-05

- **0.2.0 published -- first real release (#164).** The secrets gate
  (`COMMITTER_TOKEN` + `CARGO_REGISTRY_TOKEN`) cleared. Before firing,
  found the changelog was inconsistent: `Cargo.toml` was hand-bumped to
  0.2.0 back on 2026-06-02 (#136), but ~7 flags merged *after* that prep
  (`--permission-mode`, `--effort`, `--bare`, `--system-prompt`,
  `--append-system-prompt`, `--dispatch`, `history --paths`) sat in
  `[Unreleased]` while shipping *inside* the 0.2.0 crate. Since 0.2.0 was
  never published, folded them into the `## [0.2.0]` section (dated to
  2026-06-05) for a coherent first changelog. **Load-bearing mechanism
  learned:** release-plz runs `release-pr` and `release` as *independent*
  jobs. `release` publishes whenever `Cargo.toml`'s version has no
  matching tag, regardless of whether a release PR ever existed -- which
  is why the manual hand-bump published *without* a release-PR merge. The
  normal flow (PR bumps version -> merge -> publish) only looks coupled
  because the bump usually comes from the PR. **Still untested:** the
  `release-pr` half (auto version-bump PR + git-cliff changelog regen),
  which needs `feat`/`fix` commits past `v0.2.0` -- the 0.3.0 trigger.

- **cargo-dist release artifacts, adrs pattern (#165, closes #30).**
  Adopted the proven adrs (0.7.3) release setup rather than stock dist.
  Config: `dist 0.31.0`, 5 targets (aarch64/x86_64 macOS+Linux, x86_64
  Windows), shell+powershell+homebrew installers, `allow-dirty=["ci"]`,
  `install-path=CARGO_HOME`, `install-updater=false`, tap
  `joshrotenberg/homebrew-brew`. **Release-ownership:** release-plz
  creates the tag AND the GitHub release (`git_release_enable = true`,
  git-cliff body); dist *uploads* artifacts to it via an **upsert** step
  (`gh release upload --clobber` if exists, else create). The tag push
  (via the `COMMITTER_TOKEN` PAT, not `GITHUB_TOKEN`) is what triggers
  `release.yml`. Ported adrs's three hand-customizations (release.yml is
  under `allow-dirty`): the upsert, a `cleanup-on-failure` job (drafts an
  incomplete release if a build fails after the release was created), and
  an `announce` no-op stub. **Two non-obvious findings:** (1) **dist
  0.31.0 does NOT emit the homebrew publish job** from `publish-jobs`/
  `tap` config the way adrs's 0.30.3 does -- it builds `roba.rb` but
  `dist plan` reports `publishers: null` and no push job is generated,
  even after `dist init`. Hand-ported adrs's manifest-driven
  `publish-homebrew-formula` job. Worth chasing during the standardize-CI
  pass -- either a config-format shift or a 0.31 regression. (2) The
  homebrew job auths to the tap with `COMMITTER_TOKEN`, but that PAT is
  scoped to **only the roba repo** -- it needs `homebrew-brew` (Contents:
  write) added before the first release, or the job 403s (non-blocking:
  binaries + shell/PS installers still publish). Committed `ci:` so the
  merge did NOT trigger a release; artifacts first materialize on the
  next version tag.

- **Book rendering fix (#166).** The deployed book served a blank stub
  for *every* chapter (confirmed live -- `quickstart.html` rendered just
  `# Quickstart`). Root cause: `book/build.sh`'s `.md`->`.html`
  post-process (added in #141, "mdbook Phase 3") ran over **all**
  generated files including `SUMMARY.md`, repointing the TOC at `.html`
  targets with no matching source. mdbook's `create-missing` (default on)
  then generated empty stub source pages that *shadowed* the real `.md`
  renders. Source markdown was always intact -- only the render wiring
  broke. Fix: exclude `SUMMARY.md` from the rewrite (its links must stay
  `.md`; mdbook converts them itself). Also refreshed two stale
  post-unbundle (#130) strings (book.toml description, build.sh header
  comment). Committed `docs:` (non-release-triggering); `docs.yml`
  republishes on merge. **Lesson:** #141 looked green in CI because
  `mdbook build` *succeeds* on a stubbed book -- the breakage was only
  visible in rendered output, not the exit code. A build-success gate is
  not a render-correctness gate.

### 2026-06-03

- **Bump claude-wrapper 0.10 -> 0.11 (#137).** The only breaking change in
  0.11.0 is the `decoded_path` fix for hyphenated project slugs (PR #625),
  which roba picks up automatically with the version bump. Evaluated two new
  API features: (1) `SessionSummary.total_cost_usd` -- exists since 0.10.0
  (#609), already used in tests; the JSONL pass in `enrich_costs` is still
  required because roba needs per-model and per-bucket (input/output/cache)
  breakdown for `Rollup.usage`, `unknown_models`, and per-project figures --
  the summary total alone does not replace it. Added a comment in
  `enrich_costs` and updated the module doc to reflect this accurately.
  (2) `prompt_via_stdin(true)` -- wired as a one-liner chain in the
  non-streaming path in `src/lib.rs` (`execute_json` supports it). Not wired
  for `src/stream.rs` because `stream_query` sets the child's stdin to null;
  a TODO comment was added there instead. All gates green (266 unit + 64 CLI
  tests). Closes #137.

### 2026-06-02

- **Prompt-input rework: `-p / --prompt` + drop `require_equals` on
  `-c` / `-w` (#99, #100). BREAKING (pre-0.1.0).** The work-machine
  orchestrator hit the same root twice: clap's `require_equals = true`
  on roba's two optional-value flags (`-c [ID]`, `-w [NAME]`) forced
  the clunky `-c=ID` / `-w=NAME` syntax. It was added originally to
  disambiguate the flag's optional value from the positional prompt.
  User decision (Option C): add an explicit prompt flag as the escape
  hatch AND drop `require_equals` so the natural space-separated form
  works in the common case. Changes: (1) new `-p, --prompt TEXT` on
  `AskArgs` with `conflicts_with = "prompt"` (clap *does* accept a
  conflict between a named arg and a positional by field-name id --
  verified, no runtime bail needed). (2) `require_equals` dropped from
  both `-c` and `-w`. (3) Resolution in `run_ask`:
  `args.prompt_flag.as_deref().or(args.prompt.as_deref())` feeds
  `resolve_main_prompt` -- `-p` slots in alongside the positional, the
  stdin > editor > explicit > file precedence is untouched. **The
  breaking semantic:** with `require_equals` gone, a space-separated
  word after `-c` / `-w` is now consumed as that flag's value, so
  `roba -c "follow up"` treats "follow up" as the session id (not the
  prompt) and `roba -w "do it"` treats "do it" as the worktree name.
  The documented replacement for the old behavior is `roba -c -p
  "follow up"` / `roba -w mybranch -p "do it"`. `bare_alias_candidate`
  reads only `ask.prompt`, so `-p "name"` is never treated as an alias
  invocation (explicit prompt = literal). Test churn: five existing
  cli.rs unit tests + one mechanical test pinned the old space-form
  behavior and were flipped to assert the new semantics (and the new
  bare forms now use `-c -p` / `-w -p` to express "presence + prompt").
  Docs: README surface table + Quick examples + a NOTE block;
  CHANGELOG Added (`-p`) + Changed (the breaking drop); corrected an
  actively-false "the `=` is required" line in `docs/vs-claude-p.md`.
  closes #100 (not #99).

- **Cost in dollars via bundled rate table (#11).** New `src/rates.rs`
  + `src/rates.toml` (baked in with `include_str!`): per-model
  USD-per-MTok prices (input / output / cache-read / cache-write) with
  an `as_of` date and `source` URL. `roba cost` now shows dollars
  alongside tokens (totals + a `COST` column under `--by-project` + a
  `cost_usd` field and `input/output/cache` `usage` breakdown in
  `--json`); the per-call footer shows `$X` too (relabeled from `cost
  $X` to bare `$X`). `--rates-file PATH` / `ROBA_RATES_FILE` /
  profile `rates_file` override the table; `--no-dollars` /
  `ROBA_NO_DOLLARS` / profile `no_dollars` suppress dollars. Full
  layer support on both `CostArgs` and `AskArgs`.
  **Non-obvious data finding (the load-bearing one):**
  `claude_wrapper::history::SessionSummary` only exposes a single
  combined `total_tokens` -- no input/output/cache split and no model.
  Accurate per-model, differentiated-rate costing is therefore
  *impossible from the summary alone*. The rollup does a second pass
  reading each session's full JSONL directly
  (`root.path().join(slug).join("<id>.jsonl")`, not the O(projects)
  `read_session` scan), extracting `message.model` + `message.usage`
  per assistant entry into a per-model `Usage` map, then costs each
  model separately. Verified live Anthropic pricing via WebFetch
  before committing (Opus 4.5-4.8 dropped to $5/$25; Haiku 4.5 is
  $1/$5 -- the issue's starting values were stale Haiku-3.5 numbers).
  Model lookup is exact-then-longest-prefix so a dated id
  (`claude-sonnet-4-6-20260101`) resolves to the undated table key.
  Unknown models are listed as "rates unknown", never costed at a
  misleading $0. Verified gates green; closes #11.

- **`sandbox-preflight` skill + runner integration (#112, #113).**
  Two work-machine validation reports surfaced the same root cause:
  sandbox-permission *silent degradation*. #112 -- the runner
  returned a "run this yourself" markdown artifact when `gh` wasn't
  allowlisted; output looked complete, no GitHub state changed, and
  the synchronous-lifecycle "complete" contract silently broke. #113
  -- a bare release-audit dispatch couldn't run the build gate
  (`cargo`/`maturin`) because those tools weren't allowlisted, so it
  asked a question and accumulated BLOCKED entries instead of acting.
  Fix is a new Layer 1 skill `sandbox-preflight` codifying one
  discipline: verify needed tools at step 0, **fail LOUD** on a block
  (never a "run this yourself" artifact -- that's the trap), auto-heal
  a small known-safe dev-tool allowlist (gh, git, cargo, npm/pnpm/
  yarn, pip/uv/maturin, go, make/bash/sh) by writing a single
  `Bash(<tool>:*)` entry to `.claude/settings.local.json` and
  surfacing it in the return summary, and ASK before adding anything
  security-sensitive (docker, kubectl, terraform, custom scripts).
  Wired into `roba-runner` (skills frontmatter + lifecycle Step 0 +
  a sandbox-block hand-back reason in Failure handling), referenced
  from `roba-orchestrator` (bare-dispatch note) and
  `roba-orchestration-prompt` (prompt-template note). Docs: README
  install-and-go required-allowlist note, `agents/README.md` +
  `skills/README.md`. Docs/skill content only, no src changes.
  Closes #112 and #113.

- **`release-audit-anchoring` skill (#114).** Work-machine validation
  report #114 surfaced a false-positive pattern in release-audit
  dispatches: a bare audit analyzed the working *branch tip*
  (`docs/refresh-readme-and-examples`, a stale post-merge branch that
  predated the release-plz v0.9.0 bump) rather than `origin/main`, and
  reported "version chaos" blocking findings when in reality
  `origin/main` was clean at 0.9.0. Fix is a new Layer 1 skill
  `release-audit-anchoring` codifying three disciplines: (1) anchor on
  `origin/main` (`git fetch` + `git show origin/main:<file>`, compute
  ahead/behind divergence), (2) surface branch divergence in the
  report's first paragraph so the user can redirect before reading
  findings, (3) cross-check published versions externally
  (`cargo search` / `gh release list` / tags) rather than trusting
  in-tree `Cargo.toml`/`CHANGELOG`/`README`, which drift with
  release-plz state. Referenced from `roba-orchestration-prompt`
  (pull into the discipline section for release-shaped tasks); runner
  body left slim (the prompt-author pulls it in). Docs/skill content
  only, no src changes. Closes #114.

- **mdbook docs site -- Phase 1 of #86 (refs #86).** A navigable docs
  site at [joshrotenberg.github.io/roba](https://joshrotenberg.github.io/roba/),
  aggregating the README (Introduction), `skills/*/SKILL.md`,
  `agents/*/AGENT.md`, and top-level `docs/*.md` (plus the
  github-actions example subtree) into one mdbook. Built by
  `book/build.sh` (a bash aggregator) + a `.github/workflows/docs.yml`
  GitHub Actions deploy to Pages on push to `main`. **Static-host
  only** (GitHub Pages), no serverless/Workers. **Decisions:** (1)
  only `book/book.toml` + `book/build.sh` are committed; both
  `book/src/` (script-generated source) and `book/book/` (mdbook
  output) are gitignored and regenerated by CI. (2) Frontmatter is
  preserved as a leading `` ```yaml `` fenced block rather than parsed
  -- visible without mdbook choking on it. (3) The flattened tree
  (each skill/agent becomes one file) needs cross-ref link rewriting;
  `build.sh` applies skill-page rules under `skills/` and agent-page
  rules under `agents/`, keying off the link *target* type
  (`SKILL.md` vs `AGENT.md`) so it's correct regardless of the
  original `../`/`../../` prefix. Source-path *link text* is left
  as-authored (only hrefs are rewritten) -- cosmetic, acceptable for
  Phase 1. (4) `multilingual = false` dropped from `book.toml` --
  mdbook 0.5.3 rejects the key. **Non-obvious finding:** the repo
  README's `docs/examples/github-actions/` bare-dir link and the
  section-intro READMEs' `../agents/` / `../skills/` bare-dir pointers
  don't resolve in mdbook, so the script rewrites them to the
  corresponding `README.md` page. **Manual step (surfaced in PR):**
  GitHub Pages must be set to deploy from "GitHub Actions"
  (Settings -> Pages -> Source) before the first workflow run can
  deploy. Phase 2 (URL addressability inside the binary) and Phase 3
  (`.md`-suffix link rewriting) are deferred to separate dispatches;
  this refs #86, does not close it.

- **Docs URL addressability -- Phase 2 of #86 (refs #86).** The roba
  binary is now URL-aware: two build-time const bases in
  `src/library.rs` (`DOCS_RENDERED_BASE` = the mdbook Pages site,
  `DOCS_RAW_BASE` = GitHub raw-content `main`) plus generic
  `rendered_url(kind, name)` / `raw_url(kind, name)` helpers and four
  named wrappers (`skill_rendered_url` etc.). Surfaced via `--url` on
  `skill show` / `agent show` (prints `rendered:` + `raw:` lines to
  stdout, **suppresses the body** -- the caller wanted URLs) and
  `--urls` on `skill list` / `agent list` (swaps the description
  column for the two URL columns). **Two-URL pattern, no plugin** --
  humans get the rendered `.html`, agents `WebFetch` the raw markdown.
  Also added an additive v1 `see_also: Vec<String>` field to
  `ErrorBody` in `src/error.rs` with
  `#[serde(skip_serializing_if = "Vec::is_empty")]` -- wired but
  nothing populates it yet (no error maps to a doc URL today); the
  mechanism exists so a future error can point at a doc without a
  version bump. **Non-obvious findings:** (1) the rendered URL uses a
  `.html` suffix -- verified against the live deploy; mdbook flattens
  `skills/<name>/SKILL.md` to a single `skills/<name>.md` source page
  served at `skills/<name>.html`. (2) The github.io base 301-redirects
  to a custom domain (joshrotenberg.com) but stays the canonical
  repo-pinned form. (3) The URL path key is the item's *directory*
  name (the URL component), not the frontmatter `name` or the matched
  query string. (4) The repo is currently **private**, so the raw URL
  needs auth today -- the pattern resolves once the repo is public
  (the Pages site is already public). Phase 3 (`.md`-suffix link
  rewriting) stays deferred; refs #86, does not close it.

- **Book restructured into four audience-driven sections (refs #86).**
  The mdbook layout was mechanism-driven (Skills / Agents / Docs,
  alphabetical within each) and didn't match the For-humans /
  For-agents framing the project uses. Replaced with four named
  sections via mdbook's `#` part-header syntax: "Getting started"
  (Quickstart), "Using roba" (vs-claude-p, use-cases, profiles,
  aliases, permissions, scripting, examples -- explicit order, not
  alphabetical), "Agents & orchestration" (overview, skills/agents with
  roba-orchestrator before roba-runner), "Reference" (one combined
  lookup page). Three new docs pages: `docs/quickstart.md`
  (install-to-first-answer in ~100 lines), `docs/agents-overview.md`
  (what the library is, install, orchestrator/runner pattern, URL
  addressability), `docs/reference.md` (flags table, env vars, exit
  codes, JSON envelope schema, roba.toml config schema -- sourced from
  `src/cli.rs`, `src/env.rs`, `src/lib.rs`, `src/error.rs`,
  `src/profile/types.rs`, `src/aliases.rs`). **Non-obvious finding:**
  `bash`/`sh` was sandbox-blocked, so `book/build.sh` couldn't be
  run directly -- the `book/src/` populated manually with the same
  render_page pattern (content + Source footer) and SUMMARY.md written
  directly. mdbook's duplicate-file detection caught README.md appearing
  in both the top-level `[Introduction]` and the "Getting started"
  section; removed the duplicate from the section. The build.sh rewrite
  uses explicit `case` dispatch for human-readable section titles
  (vs-claude-p -> "Why not just claude -p", etc.) and a preferred-agent-
  order loop with a fallback for any future additions. README Topics
  section updated with one-line pointers to Quickstart, Agents overview,
  Reference. Docs/CHANGELOG/build.sh only; no src/tests touched.

- **README slim -- Path A (refs #86).** The repo `README.md` was
  ~430 lines, bloated for both roles it serves (GitHub landing page +
  the book's Introduction, which renders the README verbatim). Path A
  (chosen 2026-06-03 discussion): keep one file serving both
  audiences but slim it to ~150 lines, pushing the four heaviest
  sections into dedicated `docs/` pages, each replaced in the README
  by a one-line pointer. New pages: `docs/permissions.md`
  (safe-by-default + cross-layer precedence + `--show-permissions`),
  `docs/scripting.md` (the agent ABI: stdout/stderr split, versioned
  `--json` envelope, typed exit codes, `--no-retry`), `docs/aliases.md`
  (the `[alias.NAME]` schema). **Non-obvious finding:** the Aliases
  content was *already duplicated* -- `docs/profiles.md` carried a
  full `## Aliases` section (fuller than the README's). Rather than
  create a third copy, `docs/aliases.md` became the canonical home
  (built from the profiles.md section, the more complete one) and the
  profiles.md section collapsed to a pointer. Cross-link convention
  (verified against existing docs + the book build): docs->docs use
  sibling-relative (`profiles.md`), docs->README use `../README.md`,
  README->docs use `docs/X.md` -- all three resolve both on GitHub
  and in the flattened mdbook tree. `book/build.sh` walks `docs/*.md`
  so the new pages auto-appear in SUMMARY (verified: rebuilt + `mdbook
  build` clean). No `build.sh` change needed. Fixed two stale anchors
  that pointed at moved README sections: `docs/use-cases.md`
  (`README.md#versioned-json-output` -> `scripting.md#versioned-json-envelope`)
  and the skills/agents READMEs (`profiles.md#aliases` ->
  `docs/aliases.md`). Docs-only; no src/tests touched. refs #86 (Phase
  3 + book-layout iteration remain).

- **Unbundle skill+agent library (#130). BREAKING (pre-0.1.0).**
  The skill+agent library moved to a separate repo
  (`joshrotenberg/agent-tools`). Roba is now a pure mechanical
  wrapper with no bundled content. Removed: `skills/`, `agents/`,
  `src/library.rs`, `src/skills.rs`, `src/agents.rs`, `build.rs`,
  `docs/agents-overview.md`, the `roba skill` and `roba agent`
  subcommands, and all CLI tests that exercised them. `--agent NAME`
  stays (it's a generic Claude Code passthrough, not bundle-specific).
  `book/build.sh` SUMMARY generator drops the "Agents &
  orchestration" section. README updated with a BYO paragraph pointing
  at `~/.claude/{skills,agents}/` and `joshrotenberg/agent-tools`.

The README froze a *wide* CLI surface. Stability promise is good; the
width was the risk. All three pruning candidates resolved:

| candidate | outcome | issue/PR |
|---|---|---|
| output-to-file trio (`--save`, `--tee`, `--json`) | collapse to `--json` + `-o/--out` | #41 / #56 |
| `--head` / `--tail` | cut both | #42 / #55 |
| `--quiet` vs `--plain` | keep both, sharpen help text | #43 / #54 |

Pruning principle for future surface decisions: real shell-history
frequency (the established method), not taste. With a single user,
that signal is sparse -- so for now we lean on the "augments
interactive" framing and the agent-vs-human split as the prune
criteria, and document why we kept or cut each thing.

### 2026-06-03

- **claude-wrapper bump 0.10 -> 0.11 (#137, #138).** `total_cost_usd` on `SessionSummary` cannot replace the JSONL pass (needs per-model/per-bucket breakdown); documented in `src/cost.rs`. `prompt_via_stdin(true)` wired on non-streaming path; streaming path has a TODO (stdin is null there, needs upstream library change).
- **skills/ directory + use-roba skill (#139, #140, closes #134).** `skills/use-roba/SKILL.md` documents roba's agent ABI (stdout/stderr contract, exit codes, --json envelope, key flags, common patterns). BYO install: copy to `~/.claude/skills/`. Wired into `book/build.sh` under a new "Skills" section.
- **mdbook Phase 3 (#141, refs #86).** `book/build.sh` post-processing pass rewrites local `.md` links to `.html` using `[^):]` to exclude `://` URLs. PR #141 open, all CI green.
- **CLAUDE.md slimmed.** Removed bundled-era agent/orchestration content (net -646 lines). Positioning trimmed to mechanical-wrapper framing; brainstorm sketches for closed features removed.

## Open architectural questions

- **roba serve / hot-process model (#142).** Design filed. Spike in progress -- see brainstorm sketch below. The two load-bearing unknowns to validate first: (1) stream-json session_id timing (must appear early enough to return in send_prompt response), (2) tower-mcp fit for the tool/resource surface.
- **`--stream` in or out of the 0.1 positioning (#39).** Streaming
  with live tool indicators is the most "I'm reimplementing
  interactive" feature in the surface. Leaning: keep stream but make
  it clearly a TTY-only nicety, never load-bearing. Resolve before
  cargo-feature gating (#59).
- **Cargo-feature gates per capability (#59).** Per-feature gates
  (`render`, `spinner`, `pick`, `repl`) would let agent operators
  build a stripped binary and humans skip features they don't want.
  Blocked on #39.
- **Skill library (#47).** Three-layer model settled (knowledge /
  procedure / content) in the comment thread. Format: adopt
  `.claude/agents/`-compatible markdown so a single file works as both
  a Claude Code subagent and a roba skill. Concrete next step:
  promote auto-memory entries to Layer 1 skill files.
- **`--readonly` should suppress lower-layer `writable = true` (#52).**
  Spec-vs-code gap surfaced by the #44 docs PR. Test currently pins
  the wrong behavior; flip when fixing.

## Brainstorm sketches (still-relevant)

### roba serve / hot-process / MCP server (#142)

Design issue: #142 ("headless Claude server -- MCP transport + ledger").
Spike in progress as of 2026-06-03.

**The model:**

roba optionally runs as a persistent process. Each call is still one
`claude -p` invocation -> one response, but the process is hot (pre-warmed
auth, session pool, work queue). The server is a transport + ledger, not
a session manager.

**Two execution modes behind one interface:**

| mode | under the hood | use case |
|---|---|---|
| one-shot | `claude -p` spawn, return, done | quick queries, stateless steps |
| named session | DuplexSession pool, routed by name | multi-step work, REPL, context continuity |

**Auto-routing (the key UX insight):**

`roba ask "..."` checks for a running server (Unix socket at
`~/.local/state/roba/server.sock` or `$ROBA_SERVER` env). If found: route
through server. If not: direct `claude -p` spawn (existing path, unchanged).
From the caller's perspective behavior is identical either way. The server
is an optimization and capability upgrade, not a hard dependency.

**Interface hierarchy (all backed by same dispatch core):**

```
roba serve                 # start server (foreground or background)
roba serve --repl          # start server + connect REPL client immediately
roba repl                  # connect to running server (or start one)

Interfaces:
  CLI    one-shot: same flags as today, routes through server if running
  REPL   reedline loop -- send_prompt per turn, one named session
  MCP    native Claude Code tool (solves Bash visibility gap in agent-tools)
  REST   optional, v2+
```

**The MCP angle (why it matters):**

When an orchestrator calls roba via `Bash(roba ...)`, Claude Code treats
it as an opaque shell command -- no session panel, no token count, no
"view" link. If roba exposes an MCP server, orchestrators call
`mcp_roba.send_prompt(...)` as a native tool call, which shows up in
Claude Code's interface. Solves the agent-tools visibility gap without
changing the one-call-per-turn semantics.

**MCP library: tower-mcp** (the obvious choice -- same Tower service
pattern used across the codebase; API known deeply; gaps found while
spiking feed back into tower-mcp directly).

**The roba-core workspace split (post-spike):**

Don't do this during the spike. After the spike proves the model works,
extract:

```
crates/
  roba-core/   lib -- dispatch, stream, session, profile, aliases, cost,
                      agent_check, prompt, error envelope, env overrides
  roba/        bin -- thin CLI (clap -> core types -> dispatch)
  roba-server/ bin -- MCP server (tool call -> core types -> dispatch)
```

`AskArgs` (clap-annotated) and the MCP tool params both map to a plain
`DispatchArgs` struct in core. Same permission model, same profile
resolution, no drift between interfaces.

**Connection to existing issues:**

- #9 (async) -- subsumed: orphan-able jobs in the server cover this
- #12 (REPL) -- subsumed: REPL is now a client interface to the server (~30 lines of reedline)
- #37 (named sessions) -- load-bearing: the session name is the addressing key for the server's DuplexSession pool
- #66 (--with-mcp) -- superseded: the MCP server IS this, inverted

**Spike goals (validate before committing to architecture):**

1. **stream-json session_id timing** -- fire `claude -p --output-format stream-json` against a fresh session, confirm `session_id` appears early enough in the event stream to return in the `send_prompt` response. Load-bearing for the `job_id -> session_id` ledger.
2. **tower-mcp fit** -- wire `send_prompt` as a single Tower service; see if the request/response shape maps cleanly onto `DispatchArgs -> JobHandle`.
3. **auto-routing feel** -- does silent socket-check + fallback feel right, or does it want explicit opt-in?

**What the spike defers:**

The `roba-core` workspace split, the full session pool + queue, the REPL client, REST. Start with `roba serve` as a subcommand in the current binary wired to tower-mcp directly in `src/serve.rs`. If the spike changes the design, no codebase reorganization was wasted.

**Why the previous claude-server didn't stick (hypothesis):**

It lived alongside claude-wrapper as a general session server with explicit lifecycle management (create/attach/send/receive/close). The caller had to manage the session. The roba version hides session lifecycle: caller says `-s project-foo`, server figures out if that session exists and is healthy, creates it if not. No explicit create/close API for the common case.

### meta-session / workspace grooming / local RSI loop (#156, agent-tools #215, #216)

**The model:** three pieces.

```
driver session  (named "meta", -c or --name meta to resume)
      |
      | MCP tool calls (roba server, once live -- see #142)
      | Bash(roba ...) (works today, just opaque subprocess)
      v
roba server (#142)
      |
      v
grooming skills (agent-tools #215, #216: groom-claude-mds, groom-sessions,
                  groom-branches, extract-learnings, promote-insights)
```

The driver session IS the memory. It accumulates context across every groom run;
`-c` or `--name meta` picks it back up. Named sessions give it stable identity
across cwds and days.

**Two loops:**

*Maintenance (grooming):* survey -> identify staleness/cruft -> propose -> apply.
- Stale CLAUDE.mds (done items still open, orphaned worktrees referenced, etc.)
- Session cleanup (old sessions with no open PR, simulate /compact via JSONL summarize)
- Branch/worktree cleanup (merged branches, orphaned worktrees)

*Improvement (RSI):* collect learnings -> synthesize -> propose -> validate -> apply.
- Extract from: decisions logs, dogfood logs, error patterns across projects
- Promote to: `~/.claude/CLAUDE.md`, `~/.claude/skills/`, `~/.claude/agents/`
- Validation proxy: run improved skill/agent on a known-good task from history
- Loop is recursive: better skills -> better runs -> richer learnings -> better proposals

**Scope gates (load-bearing for skill content):**

| operation | autonomy |
|---|---|
| read / survey / report | fully autonomous |
| propose CLAUDE.md updates | autonomous, human reviews output |
| delete worktrees / branches | needs confirmation |
| write to `~/.claude/skills/` or `agents/` | needs confirmation |

**Promotion target risk tiers:**

- `~/.claude/CLAUDE.md` -- global conventions, text-only, easy to diff/revert
- `~/.claude/skills/` -- procedure docs, no execution, affects all agents
- `~/.claude/agents/` -- executable prompts with tool access, always human gate

**What roba needs:**

- `--name NAME` flag (#37) -- stable session identity across cwds; the meta-session
  is just a session named "meta"
- roba serve (#142) -- closes the visibility gap (native tool call vs. opaque Bash)
- `roba meta` convenience alias -- optional, `roba -c --name meta "..."` is the full form

**Open questions:**

- Driver session home: one canonical `~/.claude-meta/` directory, or `--name meta`
  picks it up from any cwd?
- Scheduling: cron calling `roba -c --name meta "run weekly groom"`, or roba serve
  has a built-in scheduler?
- `/compact` simulation: read session JSONL, summarize with a fresh roba call, write
  sidecar. Close enough, or does this lose something important?
- agent-tools #122 (workspace-survey extension) and #127 (reconciliation agent) are
  related -- groom skills should build on them rather than re-derive.

## Conventions

Per Josh's global CLAUDE.md (`~/.claude/CLAUDE.md`):

- **No emojis** in code, commits, or docs
- **No em dashes** in docs / commits / comments. Use `--` or rephrase.
- **Prefer editing existing files** over creating new ones
- **Conventional commits**: `type: description`. `!` marks breaking.
  Don't include "Generated with Claude Code" or "Co-Authored-By"
  signatures.

Per Rust standards (`active/rust/CLAUDE.md` upstream):

- `thiserror` for libs, `anyhow` for apps (we're an app, use anyhow)
- Builder pattern returns `&mut Self` (matches claude-wrapper)
- `cargo fmt --all -- --check` before commit
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo test --lib --all-features`
- `cargo test --test '*' --all-features` (mechanical CLI tests)

Project-specific:

- Module structure mirrors the responsibility boundaries above;
  don't merge unless there's a real cohesion gain
- Pure functions get unit tests in the same module under
  `#[cfg(test)] mod tests`
- When passing `claude-wrapper` API additions through to roba, prefer
  additive changes to claude-wrapper itself; we control both repos but
  roba uses the published version
- Resolved bullets in the decisions log point at PR numbers; detailed
  reasoning sits in commit messages and PR bodies, not here

### Test placement (closes #62)

Two homes for tests, picked by what you're asserting:

- **`tests/cli.rs`** (mechanical, `assert_cmd`-based). End-to-end CLI
  behavior: argument parsing into actual exit codes, stdout/stderr
  separation, file outputs, presence/absence of errors. Use when you
  care about the *process boundary*.
- **Module unit tests** (`src/<module>.rs::tests`) using
  `Cli::try_parse_from`. Clap-level assertions that the binary
  wouldn't reliably surface: `conflicts_with` semantics,
  `conflicts_with_all` interactions, `required_unless_present`,
  value-parser results. Use when you care about the *parse tree*.

**Rule of thumb:** if a clap rule fires *before* `Cli::parse()`
returns, test it at the parse level. If you're asserting about exit
codes or output streams, test it at the binary level.

Surfaced by #41: a `--out` + `--json` interaction test would naturally
have gone in `tests/cli.rs`, but clap's `--help` handler short-circuits
*before* conflict validation. `roba foo --out file.json --json --help`
exits 0 even when there's a real conflict, which makes binary-level
"this doesn't conflict" assertions unreliable. The test landed in
`src/cli.rs` as a unit test using `Cli::try_parse_from`, which is the
right home for parse-level assertions.

### Live tests

`tests/live.rs` calls real claude. All tests are `#[ignore]` and only
run via `cargo test --test live -- --ignored`. Budget ~$1-2 to run the
full suite. CI doesn't run them.

Live tests follow a `live_<category>_<descriptor>` naming convention
(categories: `smoke`, `output`, `session`, `stream`, `trace`, `perms`,
`compose`, `profile`, `env`; #22 adds more like `cost`, `subcmd`). The
prefix lets `cargo test ... live_<category>_` and the `just
live-category <cat>` target filter a single category cleanly. Shared
setup lives in the helpers at the top of the file (`roba_in` is the
haiku-default base builder, plus `fresh_dir`, `fixture_with_config`,
`empty_user_home`); reuse them rather than re-deriving setup per test.
The prep slice (refs #22) is infrastructure only -- the full
~35-50-test category expansion stays deferred until a category is
load-bearing.

## Pre-PR checklist

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --lib --all-features
cargo test --test cli --all-features
# Optionally (costs money): cargo test --test live -- --ignored
cargo doc --no-deps --all-features    # for crates.io / docs.rs hygiene
```

### Before cutting a release

- **Re-verify `src/rates.toml`** -- check `[meta] as_of` + the per-model prices
  against the live pricing page (`source` URL) and bump if drifted. This replaces
  the auto-CI rates-drift idea (#208, closed 2026-06-10: a standing auto-issue CI
  is more noise than value for a single-user tool; verify manually at release time).
- Confirm the release-plz PR's version + changelog look right before merging it.

## Checklists

### Adding a CLI flag

Every new CLI flag needs updates in these locations, in order:

1. **`src/cli.rs`** -- add the clap field to `AskArgs` (or the relevant subcommand struct). Write a **terse first line + blank + detail** doc comment: `-h` shows the first line, `--help` shows all. This IS the reference now (no `docs/`), so make it good.
2. **`src/session.rs`** -- wire the field into `apply_session` or `apply_permissions`
3. **`src/env.rs`** -- add `ROBA_<PARAM>` env var override in `apply_env_overrides_from`
4. **`src/profile/types.rs`** -- add field to `Profile` struct; update `is_empty()` and the `merge_in()` destructure + body
5. **`src/profile/resolve.rs`** -- add merge block (CLI wins over profile); add unit test in `mod tests`
6. **`src/env.rs` tests** -- add unit tests for set / CLI-wins / ignores-invalid (3 tests minimum per flag)
7. **`src/cli.rs` tests** -- add parse-level unit tests for any `conflicts_with` / `required_unless_present` interactions
8. **`tests/cli.rs`** -- add mechanical CLI test if the flag affects exit code or stdout/stderr routing
9. **`tests/live.rs`** -- add `#[ignore]` live test covering the flag end-to-end (assert mechanics, not model compliance)
10. **`roba-config.sample.toml`** -- add a commented line documenting the new profile key (the `sample_config_parses` test enforces it stays valid)
11. **`README.md`** -- if the flag is part of the agent ABI (stdout/stderr, `--json` envelope, exit codes), reflect it in the "For agents & scripts" section. (The use-roba skill was removed 2026-06-09 (#224); the README is now the agent-ABI home alongside `--help`.)
12. **commit message** -- the changelog is generated by release-plz/git-cliff from conventional commits, so the commit subject IS the changelog entry. Do NOT hand-edit `CHANGELOG.md`'s `[Unreleased]` -- it gets regenerated.

**Signature-change note:** when modifying a shared output/render helper (e.g. `format_footer`), grep for all callers -- `lib.rs` (non-streaming) and `stream.rs` (streaming) are physically separated. The compiler catches it, but catching it pre-commit is cheaper.

**Rule of thumb:** if a flag touches permissions, check the README's "Safe by default" section. If it affects the JSON output envelope, check `src/error.rs` and the use-roba skill's ABI section.

**Validation:** after adding, run the pre-PR checklist above. For live tests, run `cargo test --test live -- --ignored live_<category>_` to filter.

## Test counts as of v0.1.0

- ~140-150 unit tests (per-module `mod tests`); grew with the
  2026-05-30 refactors
- ~38-40 mechanical CLI tests (`tests/cli.rs`)
- 12 live tests (`tests/live.rs`, `#[ignore]` by default)

## Dependency on claude-wrapper

roba depends on `claude-wrapper = "0.11"` from crates.io. The
wrapper's surface that we use:

- `claude_wrapper::{Claude, QueryCommand}` -- the core builder
- `claude_wrapper::streaming::{stream_query, StreamEvent}` --
  streaming pipeline
- `claude_wrapper::types::{OutputFormat, QueryResult}` -- typed
  response shape
- `claude_wrapper::history::{HistoryRoot, ListOptions, ListSort,
  HistoryEntry, SessionSummary}` -- session JSONL reader
- `claude_wrapper::auth::AuthErrorKind` (tests only)
- `claude_wrapper::Error` variants (Auth, BudgetExceeded, Timeout,
  History)

If we need new wrapper APIs, we PR claude-wrapper, wait for a release,
then bump our dep. The path-dep era is over (mid-2026).

## Relationship history

roba was developed inside `claude-wrapper`'s workspace on a long side
branch called `feat/cli-runner-brainstorm`, originally scaffolded as
`cwr` ("claude wrapper runner"). The session that built roba also
salvaged a bunch of wrapper-side modules (`history`, `skills`,
`settings`, `commands`, `duplex` permission_mode builders) that were
on a parallel parked branch and merged them to claude-wrapper main as
PRs #609-#614. Those features ship in claude-wrapper 0.10.

The full commit history is preserved here via `git filter-repo`
extraction; commits dated before 2026-05-27 in this repo lived in
claude-wrapper's tree. The `cwr -> roba` rename happened on 2026-05-27
just before extraction.

## Dogfood log

Running record of orchestrator-fired roba runs. Each entry: date /
target / model / wall-clock / spiral-y/n / lessons. Builds the corpus
that #47 Layer 1 skill content draws from.

Entries before 2026-06-01 archived; see git history.

| date | target | model | clock | spiral | lessons / PR |
|---|---|---|---|---|---|
| 2026-06-01 | roba#65 --trace PATH | sonnet | ~5m | no | #91 merged; `DisplayMode` enum (Live vs Silent) + `TraceWriter` flushes even on mid-flight error |
| 2026-06-01 | roba#88 user-defined aliases in roba.toml | sonnet | ~9m | no | #92 merged; 627-line aliases module; char-scan expander (avoids heredoc `$` mangling); synthetic argv gets clap precedence for free |
| 2026-06-01 | roba#89 gh docs salvage from #29 | sonnet | ~3m | no | #93 merged; agent caught two real errors in the issue body (`gh pr view:*` syntax, JSON envelope path) and corrected them |
| 2026-06-01 | live test review + additions | sonnet | ~4m | no | #94 merged; agent found TWO stale broken live tests (cut --head + nested session_id), fixed both + added refusal test + INTENTIONALLY-UNTESTED comment block |
| 2026-06-02 | book restructure + 3 new docs pages (#86) | sonnet | ~15m | no | PR #128; sandbox blocked bash/sh so book/src populated manually (same render_page pattern); duplicate README.md caught by mdbook validation; 4-section SUMMARY + quickstart + agents-overview + reference pages |
| 2026-06-02 | unbundle skill+agent library (BREAKING) | sonnet | ~12m | no | PR #130; net -3257 lines, 32 files; cleanly removed src/library.rs, src/skills.rs, src/agents.rs, build.rs, skills/, agents/, docs/agents-overview.md, roba skill + roba agent subcommands + their tests. Verified --agent NAME stays as generic passthrough. All gates green. |
| 2026-06-02 | stderr session id at dispatch start (#124) | sonnet | ~8m | no | PR #132; added DisplayMode-aware `session_id_printed` flag in stream.rs; surfaced honestly that streaming-pipeline-only is the right scope (non-streaming knows session id only at end, useless for mid-flight). |
| 2026-06-02 | agent frontmatter permission check (#123) | sonnet | ~14m | no | PR #133; 393-line new src/agent_check.rs with hand-rolled YAML frontmatter parser (no serde_yaml dep), agent file lookup (project + global, dir-style + flat), Bash coverage logic (granular `Bash(foo:*)` allows count as covering bare `Bash`), full layered resolution (--no-agent-check + ROBA_NO_AGENT_CHECK + profile), 4 mechanical CLI tests + 8 unit tests. |
| 2026-06-02 | post-unbundle docs cleanup | interactive | ~10m | n/a | PR #135 hand-edited; deleted docs/README.md (TOC redundant); removed obsolete `Installing the bundled skill + agent library` section from docs/profiles.md; removed `roba skill` + `roba agent` subcommand sections from docs/reference.md; reworked docs/use-cases.md (trim duplicated agent-tools content, remove "more coming" TODO, point at agent-tools for agent layer); net -108 lines. |
| 2026-06-08 | #187 no-args-on-TTY guard | sonnet | ~3m | no | PR #191; promptless-on-TTY guard at the `run_ask` seam (not `resolve_main_prompt` -- only the composed prompt is known there, so `--git-diff`-only isn't intercepted); confirmed `IsTerminal` already imported; mechanical test pins the unchanged non-TTY path. First run of the full-prompt-in-PR-body + `--trace` visibility loop. |
| 2026-06-08 | #188 friendlier claude-missing/auth errors | sonnet | ~5m | no | PR #192; verified `Error::NotFound` + `auth_kind()` against actual claude-wrapper 0.11 source before writing; factored shared `CLAUDE_CODE_URL` const so stderr hint + JSON `see_also` can't drift; additive hint via `print_meta` (primary error still prints first); mechanical test clears PATH to drive real NotFound end-to-end. |
| 2026-06-08 | #196 bare-roba help blurb (fixes dead #187 guard) | sonnet | ~4m | no | PR #197; `compose_prompt -> Option`, branch in `run_ask` (blurb+exit0 on TTY, error non-TTY); **verified with a real pty** (the proof #187 never got); single-sourced blurb from `AFTER_HELP`. |
| 2026-06-08 | #37 named sessions (`[session]` config) | sonnet | ~8m | no | PR #199; stateless declarative config layer (no owned state); pool map sibling to aliases; `--session`/`ROBA_SESSION` -> `continue_session`; pure `resolve_session` helper; caught a `Pool` literal in resolve.rs tests. 263 lines. |
| 2026-06-08 | rtk interop research (read-only) | sonnet | ~3m | no | memo only; rtk's global hook already covers roba workers for free; nothing to build; perm-matcher caveat banked. |
| 2026-06-08 | cross-project usage research (read-only) | sonnet | ~4m | no | memo only; mined mcp-proxy corpus (4 task files + 2 traces); found `--dispatch` fits neither shape -> drove #202. Honest, self-flagged thin spots. |
| 2026-06-08 | #202 remove `--dispatch` preset | sonnet | ~7m | no | PR #203 (BREAKING -> 0.4.0); +30/-141; removed full layered plumbing + corrected docs; caught an extra tests/cli.rs test the prompt missed; verified `--dispatch` rejected + new help renders. |
| 2026-06-09 | #222 `--session-id` pass-through (flag 1/N) | opus-4-8 | clean | no | PR #225 merged, all CI green; +227/-5 full-stack 12-step flag (cli/session/env/profile/sample/README + unit + live); honest gap-surfacing: caught roba has no `--resume` (conflicts `-c=ID`/`continue_session` instead), composes with `--fresh`. First of the #222 obvious-flags queue; pass-through only, mint+record deferred. |
| 2026-06-09 | #222 `--max-turns` + `--max-budget-usd` (flag 2/N) | opus-4-8 | clean | no | PR #226 merged; +281; honest exit-code investigation -- no `claude_wrapper::Error` variant for max-turns so NO fabricated code (cap -> exit 1); caught that `BudgetExceeded` (exit 3) is the wrapper's own `BudgetTracker`, not the `--max-budget-usd` flag (also exit 1); new `read_u32`/`read_f64` env helpers + `Limits` help_heading. Corrected a wrong assumption in the dispatch prompt. |
| 2026-06-09 | #222 `--json-schema` structured output (flag 3/N) | opus-4-8 | clean | no | PR #227 merged; +198; claude wants inline JSON so roba takes a PATH, reads+validates (`serde_json`), inlines -- errors via the envelope (verified live); confirmed `execute_json` already forces `--output-format json` (locked auto-force already satisfied, no redundant code); structured output surfaces under `.result.*` (`QueryResult.extra` flatten); read seam in `run_ask` covers streaming + non-streaming. |
| 2026-06-09 | #222 `--mcp-config` + `--strict-mcp-config` (flag 4/N, HIGH TIER DONE) | opus-4-8 | clean | no | PR #228 merged; +288; thin pass-through (roba forwards paths, claude reads them), new `MCP` help_heading, mirrored `--allow-tool`/`--attach` Vec conventions, serverless `{}` live test. Completes the 4 high-tier #222 flags (4 PRs, zero spirals). |
| 2026-06-09 | #222 med tier: `--add-dir` + `--fallback-model` + `--no-session-persistence` (3 flags, 1 PR) | opus-4-8 | clean | no | PR #229 merged; +395; all three trivial pass-throughs via established templates (`--mcp-config`/`--model`/`--bare`); 387 lib + 97 CLI tests. **High + med #222 tiers DONE -- 5 PRs (#225-#229), zero spirals**; low tier deferred on-demand. Sequential dogfood loop (orchestrator writes prompt + scouts wrapper facts -> roba implements -> CI -> merge) proven across a 5-PR run. |
| 2026-06-09 | #217 `roba worktree list` (managerial 1/3) | opus-4-8 | clean | no | PR #230 merged; +306; first managerial SUBCOMMAND -- thin CLI over claude-wrapper 0.11's existing `worktrees` module (placement principle validated: the wrapper already owned the primitive). New `src/worktree.rs`, table + `--json`, read-only (list only); caught `main.rs` `wants_json` for the new subcommand; smoke proved the superset (listed this repo's own `agent-*` worktree). Closes #217. |
| 2026-06-09 | `roba show` + `--metrics` (managerial 2/3) | opus-4-8 | clean | no | PR #231 merged; +441; RECONSTRUCTED envelope (result + session_id rebuild; num_turns/cost DERIVED; duration null) via `read_session` + `extract_message_text` + `usage_by_model`; `QueryResult` pub-constructed -> reused `SuccessEnvelope` (made `pub(crate)`) for byte-identical `--json`; last-assistant-with-text handles trailing tool-only turns; cost `None` never `$0`. `--wait` deferred -> #220 stays open. |
| 2026-06-09 | `roba history --worktree` + slug-prefilter refine (managerial 3/3) | opus-4-8 | clean | no | PR #232 (2 commits: feat + perf); +308; `session_worktree(cwd)` match + cross-project scan. Worker SURFACED the sparse-scan/noisy-cap-note wart -> on-branch refinement added a SOUND `slug_is_worktree` pre-filter (superset of matchable, no false negatives, verified vs 171 real dirs). Closes #218. **Managerial cluster COMPLETE (#217/#220/#218); --wait the only deferred piece (#220 open).** |
| 2026-06-09 | #221 uniform `{version:1}` `--json` envelope (cost/history/doctor + worktree) | opus-4-8 | clean | no | PR #233 merged (`feat!`, breaking); +351/-95; `VersionedResult<T>` wrapper; `doctor` gained `--json` (structured `Check` collection) + documented exit codes (0 ok / 1 fail, warnings ok); worker CAUGHT that `worktree list --json` was the last bare-array holdout and wrapped it too -> the `error.rs` "version on every --json" contract is now literally true. 416 lib + 106 CLI tests. |
| 2026-06-09 | docs pass: positioning + ditch skills + notes (#223/#224/#209) | opus-4-8 | clean | no | PR #234 merged; +52/-251; SURGICAL (README was already 0.5.0-current per the docs-audit workflow); folded 3 unique skill chunks (jq gotchas / refusal+exit-code map / post_turn_summary) into README THEN `git rm skills/`; sugar North Star in `--help` `about` + crates.io description; worker CAUGHT clap's `about` pulls from `CARGO_PKG_DESCRIPTION` not the doc comment -> updated `Cargo.toml` too (syncs crates.io). **Today: 11 PRs, zero spirals.** |
| 2026-06-10 | #220 `roba show --wait` (backlog-zero release prep) | opus-4-8 | clean | no | PR #236 merged; +330; completion signal = last Assistant entry's `message.stop_reason` terminal (NOT `tool_use`) -- worker EMPIRICALLY verified vs real JSONL (715 tool_use vs 24 end_turn) AND caught that completed sessions end on a trailing `agent-name`/Other entry, so `is_complete` scans BACKWARD for the last Assistant (else it never fires); mtime-quiescence rejected (false-fires during long tool calls); default `--timeout 600`, `0`=indefinite; not-found under `--wait` = "not started yet" (keeps polling). Closes #220 -> **issue backlog EMPTY** (also closed #222/#22/#208/#211/#213/#219/#59 in the triage). |

**Version note (2026-06-09):** today's whole batch releases as **0.5.0**, NOT 0.4.0
(0.4.0 already shipped earlier -- it carried the `--dispatch` removal #203). The
0.5.0 minor bump is driven by the breaking bits in today's work: the new
`Worktree`/`Show` `SubCommand` variants (cargo-semver-checks) + the `#221` `--json`
envelope reshape (`feat!`). release-plz opened the v0.5.0 release PR (#216).
Earlier same-day notes that say "-> 0.4.0" are mislabels; the truth is 0.5.0.

**Key lessons so far:**

1. **Spawned-jsonl observability.** When a roba run hangs, read the
   spawned claude session's jsonl directly at
   `~/.claude/projects/<dir>/<spawned-uuid>.jsonl`. Roba's
   stdout/stderr capture is unreliable; the jsonl is ground truth.
   Saved as memory `feedback-roba-agent-spiral-diagnosis`.
2. **Parallel-batch cancellation cascade.** When the spawned agent
   issues a parallel tool batch and one call errors (commonly a
   duplicate `git checkout -b`), siblings return
   `<tool_use_error>Cancelled: parallel tool call ...</tool_use_error>`.
   The agent often misreads this as "tool output missing" and starts
   flush-spamming. Prompt mitigations: sequential setup, verify-before-
   mutate, explicit "do not flush on cancellation."
3. **Prompt template.** The shape that consistently works: Setup
   (sequential, no batch), Decision, Task (specific files +
   functions), Tool-call discipline (mitigations above), numbered
   Steps with `fmt`/`clippy`/`test` verification, Constraints (no
   push, no amend, no main touch, no `gh pr create`), expected output
   (commit hash, branch, diff stat).
4. **Sync-watch-then-merge.** Don't rely on `gh pr merge --auto` --
   it silently no-ops when `allow_auto_merge: false`. Use
   `gh pr checks --watch` in background + `gh pr merge --squash
   --delete-branch` on exit code 0.
5. **Surface gaps, don't invent.** Telling the agent (in the prompt
   for a refactor) "if something fundamentally doesn't fit the
   boundaries above, surface it in the final summary -- don't
   invent a fifth file" worked. #68 returned two narrow refinements
   (`expand_path` and `home_dir` placement) with clear reasoning
   instead of forcing the literal split or overreaching into a new
   module.
6. **`--fresh` + hardened prompt template flat-out works.** All
   2026-05-31 dispatches were clean. The hardened template (sequential
   setup, verify-before-mutate, explicit cancellation handling) plus
   `--fresh` is the new baseline. Zero spirals across 4 runs (3 roba +
   1 docs cleanup).
7. **Honest gap-surfacing is the right behavior** (validated repeatedly
   2026-05-31 / 2026-06-01). The #64 run surfaced that claude-wrapper
   has no `disable_retry()` method; the #88 run flagged the doubled
   conflict-error message as expected v1 behavior; the #93 run caught
   two real errors in #89's issue body (`gh pr view:*` syntax, JSON
   envelope path) and corrected them; the #94 run found TWO stale
   broken live tests (cut `--head` and nested `session_id`) by doing
   real audit work. Tell the agent to surface findings; reward it
   for honest "this doesn't fit / this is broken" over silent
   over-claiming.
8. **Agent-led adversarial self-review pays off.** The #38 docs dispatch
   ran its own 3-lens review pass before committing and caught two
   real issues. For docs / marketing / framing-shaped work,
   instructing the agent to do a self-adversarial pass before commit
   is a cheap quality lever.
9. **Smart abstractions beat literal prompts.** The #85 dispatch noticed
   that the prompt's separate `skills.rs` + `agents.rs` modules could
   share a `library.rs` parameterized by `Kind` -- and did the
   abstraction without asking. The #58 dispatch did the same with
   `expand_path` / `home_dir` placement refinements (lesson #5).
   Trust the agent's judgment on local refinements when the prompt
   reserves the spec at the right level (boundary intent, not literal
   file structure).
10. **Zero spirals across the entire 2026-05-31 + 2026-06-01 arc**
    (~20 consecutive roba dispatches). The deterministic-ish work
    queue framing is now observable in practice. The orchestration
    prompt template + `--fresh` + draft-PR-first lifecycle produces
    predictable outcomes; the agent's honest gap-surfacing handles the
    edge cases. Pattern is stable.

11. **Write-permission wall: editing dispatches in `default` mode stall silently.**
    A worker launched without an explicit permission mode hits the Edit/Write approval
    gate on the first write and hangs. Best case: clean no-op. Worst case: ~40-turn
    mechanical bypass grind (touch + git apply heredocs, perl, tee) producing a
    corrupted file. The fix is one flag: launch editing dispatches with `--full-auto`
    or `--permission-mode acceptEdits`. `sandbox-preflight` currently only checks
    Bash-tool allowlisting -- it does not catch this gate. The right behavior (stop
    + report the exact gate message) and the wrong behavior (bypass grind) both appeared
    in the same week on the same root cause. Evidence: sessions f12e9f36 (good) and
    4cea4d02 (bad), Jun 2-3 2026.
12. **Concurrent auto-committer/linter races an in-flight dispatch.** A background
    lint/format/commit process can: (a) cause dozens of "file modified since read"
    retries, (b) delete a field from a file the worker is editing mid-run (breaking
    the build), (c) hijack the commit step so a single-commit contract is unsatisfiable.
    The worker may recover correctly, but the environment is actively fighting it.
    Either disable auto-commit during dispatches, or do not promise "single commit"
    in the dispatch prompt when a commit hook is active -- the contract cannot be
    honored and the worker looks non-compliant when it isn't. Evidence: session
    b5988f5a, Jun 3 2026.
13. **Mandate worktree isolation (`-w`) for dispatched workers that mutate the tree.**
    Two concurrent sessions sharing one checkout can have one's `git checkout` yank
    the other's branch mid-run -- observable as `gitBranch` flapping between unrelated
    branches within a single session transcript. roba's `-w` flag exists for this;
    make it the default for any dispatch that edits files, not an opt-in. Evidence:
    session 4cea4d02, Jun 3 2026.

## Working in this repo (read first, update last)

This file is the project-context durable home. Two disciplines around
it that keep it alive across sessions and runs:

- **Read first.** At the start of any non-trivial work (orchestrator
  session, roba dispatch, hand-edit), read this file. Claude Code
  auto-loads it when cwd is the project, so this is usually free; the
  discipline is "don't skip past it / don't override its content from
  prior-session memory of what it said."
- **Update last.** Before closing out work (final commit / merge /
  session end), ask: did this produce something CLAUDE.md should hold?
  Three categories worth capturing:
  - **Decisions log entry** -- a settled choice with a PR or issue
    reference. One terse line under the date.
  - **Dogfood log entry** -- if this was a roba dispatch, add a row to
    the dogfood table. New lessons bubble up to "Key lessons so far".
  - **Brainstorm sketch** -- a design idea worth capturing for later.
- **Don't update for nothing.** A small refactor that just executes
  the plan doesn't need a CLAUDE.md update. The bar is "would
  future-me want to find this when grepping the durable design home?"

The conversation is transient; this file (CLAUDE.local.md) is durable. State that should survive the
session ends up here.

## When in doubt

- Check this file: the decisions log + brainstorm sketches cover most
  "should we do X?" questions with rationale.
- Check open GitHub issues: anything implementable lives there.
- Check the commit log: messages are detailed and capture the "why"
  beyond the "what." `git log --oneline | head -30` to scan.
- The README is the marketing-shaped intro; this CLAUDE.md is the
  operational context.
