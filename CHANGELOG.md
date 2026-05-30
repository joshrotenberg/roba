# Changelog

All notable changes to `roba` are documented here. Format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versioning
follows [SemVer](https://semver.org/).

Per-crate releases are managed by
[release-plz](https://release-plz.dev/) using conventional commits;
when published the entries below become version sections.

## [Unreleased]

### Fixed

- Fail fast when an interactive-only flag is set without a TTY on
  stdin. `-e` / `--editor` and `--pick` both block on human input;
  in a head-less context (script, CI step, orchestrator) the
  process would hang waiting for keystrokes that can't arrive. Now
  both flags pre-check `stdin.is_terminal()` at the top of
  `run_ask` (right after env + profile resolution) and exit 1 with
  a canonical message: `--editor requires an interactive terminal
  (stdin not a TTY)` / `--pick requires an interactive terminal
  (stdin not a TTY)`. Closes #36.

### Added

- `--show-thinking` flag: render extended-thinking blocks live on
  stderr in the dim meta-channel style during `--stream`. Uses
  `StreamEvent::partial_message` from claude-wrapper 0.10.1 to
  decode `content_block_delta` events with a `thinking_delta`
  payload. Only takes effect alongside `--stream`; without it the
  flag is a silent no-op (profiles may legitimately set both). Also
  configurable via profile (`show_thinking = true`) and env
  (`ROBA_SHOW_THINKING=1`). Closes #10.
- `-w` / `--worktree[=NAME]` flag: passthrough to claude's
  `--worktree`, runs the session in a fresh git worktree. With no
  value, claude generates the name; with `=NAME` (e.g.
  `-w=feature-x`) the worktree directory/branch is pinned. The `=`
  is required for the named form to disambiguate from the positional
  prompt. The worktree persists after the session; clean up manually
  with `git worktree remove`. Pairs naturally with `--writable` or
  `--full-auto` -- the worktree is your sandbox. Also configurable
  via profile (`worktree = true` for presence, `worktree = "NAME"`
  for pinned name) and env (`ROBA_WORKTREE=1` for presence, any
  other non-truthy non-empty value treated as the name). Closes #28.
- `--editor-history N` (default 1): when composing with `-e`, the
  editor opens in `git commit`-style layout -- empty cursor area at
  the top for your prompt, then a `// ----- >8 -----` scissors
  line, then the last N responses below as a `//`-prefixed
  reference block. Strip removes everything from the scissors line
  down on save, so the response is for your eyes only (use `-c` to
  give claude the conversation context). `// ` prefix on reference
  lines avoids `#`-vs-markdown-header conflicts. `--editor-history
  0` reverts to the empty-editor behavior. Also configurable via
  profile (`editor_history = N`) and env (`ROBA_EDITOR_HISTORY=N`).
  Closes #5.
- Auto-named sessions: every roba call now passes `--name "roba: <preview>"`
  to claude so sessions surface in the `claude --resume` picker
  (which only lists named sessions). Prefix makes them
  distinguishable from interactive Claude Code sessions in the same
  project. Closes #3.
- `-C` / `--cwd PATH` global flag to run as if invoked from a
  different directory. Applies before any other resolution (session
  scoping, config walk-up, `--attach` globs, `--prepend` / `--append`
  relative paths, `--git-*` context). Pairs especially well with
  scripts and tests that want roba to operate in a tmp dir.
- `--fresh` CLI flag to force a fresh session. Cancels any
  profile- or env-supplied `continue = true`; the kill switch for
  accidental auto-continuation. Conflicts with `-c` / `--resume` /
  `--pick`.
- `--model MODEL` CLI flag to override the claude model per call
  (alias or full id).
- `ROBA_<PARAM>` env-var override layer. Every config knob is
  settable via an env var matching the CLI long-form (uppercased,
  `-` -> `_`, prefixed `ROBA_`). Lists comma-separated; vars via
  per-key `ROBA_VAR_<KEY>=value`. Sits between CLI and the file
  pool in precedence.
- Output-policy fields in `roba.toml` / profiles: `model`, `stream`,
  `echo`, `plain`, `quiet`, `json`.

### Changed

- **Config schema** (breaking; no published users yet):
  - File renamed: `~/.config/roba/profiles.toml` -> `~/.config/roba.toml`;
    `.roba/profiles.toml` -> `roba.toml` (no more `.roba/` directory).
  - Fields renamed: `continue_session` -> `continue`,
    `allow_tools` -> `allow_tool`, `deny_tools` -> `deny_tool`.
  - Top-level keys in `roba.toml` are now project-wide defaults
    that apply to every call (previously only `[profile.NAME]`
    tables were honored).
  - Project chain walks all the way up to the git root collecting
    every `roba.toml`; closer-to-cwd files override farther ones
    per-key, lists concat, vars merge per-key (previously only
    the closest file was loaded).

### Removed

- `ROBA_PROFILES_FILE` env var (point-at-an-extra-file). Subsumed
  by the per-knob `ROBA_<PARAM>` override layer.

### Added

- Initial cut of `roba`: single-prompt CLI runner over `claude-wrapper`.
- Input sources: positional, stdin (`-` or piped), `-f`, `-e`,
  `--prepend`, `--append`, `--attach` (glob), `--git-diff`,
  `--git-log`, `--git-status`, `--var K=V`.
- Output: plain default, `--json`, `--quiet`, `--code [LANG]`,
  `--head N` / `--tail N`, `--save`, `--tee`, `--stream`.
- Sessions: `-c` / `--resume ID`, `--fork`, `--pick` fuzzy chooser,
  `roba history`, `roba last`.
- Permissions: `--readonly`, `--full-auto` presets.
- Profiles: `--profile NAME` from `~/.config/roba.toml`,
  `roba profile {list,show,init,path}` subcommands.
- Cost: `roba cost` token rollup, `--by-project`, `--project`,
  `--json`.
- TTY UX: termimad markdown render, indicatif spinner, dim
  metadata, colored refusal/error markers, `--plain` master
  kill-switch, `NO_COLOR` env honored.
- Typed exit codes: 0 ok, 1 generic, 2 auth, 3 budget, 4 timeout.
- 86 unit tests, 22 mechanical CLI tests, 12 live-claude tests
  (`#[ignore]` by default).
