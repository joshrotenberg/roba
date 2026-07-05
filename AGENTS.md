# AGENTS.md

Instructions for coding agents working on this repo. For *driving* roba
(the agent ABI: envelope, exit codes, worker flags), see the README's
"For agents & scripts" section and `roba --help` -- this file is about
contributing.

## What this is

A single-prompt CLI runner (Rust, edition 2024) sugaring `claude -p`,
built on the published `claude-wrapper` crate. One invocation, one
answer. Lib + bin targets so integration tests drive the same code paths.

## Scope line (read before adding any feature)

- IN: wrapping/sugaring the `claude` binary (pass its flags through,
  clean its I/O), and read-only inspection of claude/roba state that
  reports (`history`, `cost`, `doctor`, `show`, `worktree list`).
- OUT: mutating claude's private `.claude/` state, daemons or persistent
  processes, orchestration policy. Those are different tools.
- Prefer pushing reusable claude-domain primitives down into
  claude-wrapper; keep roba the thin CLI over them.

## Structure

- `src/main.rs` entry point; `src/lib.rs` dispatch, `run_ask`, exit codes
- `src/cli.rs` clap surface -- doc comments here ARE the `--help` reference
- `src/session.rs` flag -> `QueryCommand` wiring; `src/env.rs` `ROBA_*` overrides
- `src/profile/` config layering; `src/show.rs`, `src/history.rs`,
  `src/cost.rs`, `src/worktree.rs`, `src/doctor.rs` read-only subcommands
- Doc homes: README (concepts + agent ABI), `--help` (reference,
  generated from `cli.rs`), `roba-config.sample.toml` (parse-tested schema)

## Build and test (all must pass before a PR)

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --lib --all-features
cargo test --test cli --all-features
```

`--workspace` covers the `roba-types` member crate (the published,
dependency-light `--json` envelope + exit-code contract) alongside the
`roba` binary. The `cli` integration tests are roba-only.

`tests/live.rs` calls real claude and costs money: `#[ignore]` by
default, run explicitly with `cargo test --test live -- --ignored`.
Live tests assert mechanics you control (a flag plumbs through, the
envelope shape, exit codes) -- never model compliance.

## Adding a CLI flag

Follow the full checklist, in order: clap field in `cli.rs` (terse first
doc line + detail -- this is the reference) -> `session.rs` wiring ->
`ROBA_<PARAM>` override in `env.rs` + tests -> a `config.rs` `ENV_MAP`
entry (so `config show --sources` reports the flag's provenance) ->
`Profile` field +
`is_empty()` + `merge_in` + resolve merge + tests -> parse-level tests for
conflicts -> mechanical test in `tests/cli.rs` if it touches exit codes or
stream routing -> one `#[ignore]` live test -> a commented line in
`roba-config.sample.toml` (a unit test parses it) -> README if it is part
of the agent ABI.

## Test placement

- clap rules that fire before `Cli::parse()` returns (conflicts,
  requires): unit tests in `src/cli.rs` via `Cli::try_parse_from`.
- Exit codes and stdout/stderr routing: `tests/cli.rs` (assert_cmd,
  never calls claude).

## Conventions

- Conventional commits (`type: description`; `!` marks breaking). The
  commit subject IS the changelog entry (release-plz + git-cliff) --
  never hand-edit `CHANGELOG.md`.
- No emojis. No em dashes -- use `--` or rephrase.
- Never commit to main: feature branch, then a PR. No "Generated with"
  or "Co-Authored-By" lines in commits or PRs.
- `anyhow` for errors (this is an app). Builder methods return `Self`.
  No unsafe code.
- stdout is the answer, stderr is metadata -- nothing decorative may
  leak to stdout; `--json` output must stay byte-clean.

## PRs

- Open the PR early with the plan as the body; link issues with
  `closes #N` (one keyword per issue number).
- CI runs macOS / Linux (stable + beta) / Windows plus fmt, clippy,
  docs, and a release-build check -- all must be green.
