# AGENTS.md

Instructions for coding agents working on this repo. For *driving* roba
(the agent ABI: envelope, exit codes, worker flags), see the README's
"For agents & scripts" section and `roba --help` -- this file is about
contributing.

## What this is

A library-first, provider-neutral runtime for one finite agent mission (Rust,
edition 2024). A mission may be a single prompt or a bounded multi-turn,
multi-worker directive. One Roba process owns the mission and exits when it is
complete, failed, or cancelled. Claude Code and Codex are provider adapters;
the CLI, REPL, and run-scoped MCP server are clients of the same public API.

The original single-prompt Claude CLI remains a compatibility surface while it
is incrementally moved onto the mission/run model.

## Scope line (read before adding any feature)

- IN: provider-neutral finite missions, bounded Roba-owned workers, explicit
  execution authority, lifecycle/event observation, steering, and thin
  library/CLI/REPL/MCP adapters.
- IN: the legacy Claude one-shot compatibility path and its read-only
  inspection commands while migration remains incomplete.
- OUT: a daemon, persistent session pool, hidden background work, or mutation
  of provider-private state.
- Keep provider mechanics in the wrapper crates where reusable. Keep workflow
  policy optional and typed rather than baking repository behavior into the
  core mission abstraction.

## Structure

- `roba-core/src/{run,lifecycle,mission,process,provider,runtime}.rs` -- public mission
  and run contracts, lifecycle, projection, provider boundary, and registry
- `roba-mcp` and `roba-repl` -- thin run-scoped adapters over `RunHandle`
- `src/main.rs` entry point; `src/lib.rs` dispatch, bounded run and legacy paths
- `src/cli.rs` clap surface -- doc comments here ARE the `--help` reference
- `src/session.rs` legacy flag -> `QueryCommand` wiring; `src/env.rs` legacy
  `ROBA_*` overrides
- `src/profile/` config layering; `src/show.rs`, `src/history.rs`,
  `src/cost.rs`, `src/worktree.rs`, `src/doctor.rs`, `src/jobs.rs`
  read-only subcommands; `src/receipt.rs` the run-receipt writer (the
  schema lives in `roba-types`)
- Doc homes: README (concepts + agent ABI),
  `docs/design/run-library-pivot.md` (current resume point), `--help`
  (reference generated from `cli.rs`), and parse-tested config examples

## Build and test (all must pass before a PR)

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --lib --all-features
cargo test --test cli --all-features
```

`--workspace` covers the member crates alongside the `roba` binary:
`roba-types` (the published, dependency-light machine contract: `--json`
envelopes, the exit-code map, run receipts) and `roba-core` (the
clap-free config-and-run engine). The `cli` integration tests are
roba-only.

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
