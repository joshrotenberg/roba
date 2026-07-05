# roba-core

roba's clap-free, side-effect-free run engine: a `Config` in, a claude
`QueryResult` out.

[roba](https://github.com/joshrotenberg/roba) is a single-prompt CLI runner
over `claude -p`. This crate is the run engine underneath it, split out from
the CLI so a programmatic caller (and, later, `serve`) can drive a run without
depending on clap or the terminal.

## What's here

- **`engine`** -- the config-and-run seam. `engine::run(&Config)` builds the
  claude client, maps the `Config` onto a `QueryCommand`, executes, and returns
  the typed `QueryResult`. No clap, no stdout/stderr, no `process::exit`, no
  TTY. `Config` is a curated struct: a `Permissions` posture, a `Session`
  selector, and the run knobs (model, effort, caps, timeout, MCP, system-prompt
  overrides, ...).
- **`session`** -- `apply_session`, the single `Config -> QueryCommand` mapper
  the engine feeds, plus the permission and agent-notice composition it
  consumes.

The `roba` binary resolves flags, profiles, and prompt composition into a
`Config` and renders the result around this seam, so there is one
flag-to-command mapper and no second copy to drift.

## Stability

Internal to roba for now: the API may change without a major bump until the
`serve` surface (roba#142) settles. Depend on it directly at your own risk.

## License

MIT OR Apache-2.0.
