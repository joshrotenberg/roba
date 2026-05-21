# Changelog

All notable changes to `cwr` are documented here. Format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versioning
follows [SemVer](https://semver.org/).

Per-crate releases are managed by
[release-plz](https://release-plz.dev/) using conventional commits;
when published the entries below become version sections.

## [Unreleased]

### Added

- Initial cut of `cwr`: single-prompt CLI runner over `claude-wrapper`.
- Input sources: positional, stdin (`-` or piped), `-f`, `-e`,
  `--prepend`, `--append`, `--attach` (glob), `--git-diff`,
  `--git-log`, `--git-status`, `--var K=V`.
- Output: plain default, `--json`, `--quiet`, `--code [LANG]`,
  `--head N` / `--tail N`, `--save`, `--tee`, `--stream`.
- Sessions: `-c` / `--resume ID`, `--fork`, `--pick` fuzzy chooser,
  `cwr history`, `cwr last`.
- Permissions: `--readonly`, `--full-auto` presets.
- Profiles: `--profile NAME` from `~/.config/cwr/profiles.toml`,
  `cwr profile {list,show,init,path}` subcommands.
- Cost: `cwr cost` token rollup, `--by-project`, `--project`,
  `--json`.
- TTY UX: termimad markdown render, indicatif spinner, dim
  metadata, colored refusal/error markers, `--plain` master
  kill-switch, `NO_COLOR` env honored.
- Typed exit codes: 0 ok, 1 generic, 2 auth, 3 budget, 4 timeout.
- 86 unit tests, 22 mechanical CLI tests, 12 live-claude tests
  (`#[ignore]` by default).
