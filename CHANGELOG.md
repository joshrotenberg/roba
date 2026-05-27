# Changelog

All notable changes to `roba` are documented here. Format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versioning
follows [SemVer](https://semver.org/).

Per-crate releases are managed by
[release-plz](https://release-plz.dev/) using conventional commits;
when published the entries below become version sections.

## [Unreleased]

### Added

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
