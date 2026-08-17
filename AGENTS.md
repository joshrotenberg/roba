# AGENTS.md

Instructions for coding agents working on this repo. For *driving* Roba (the
legacy one-shot agent ABI: envelope, exit codes, and unattended-run recipes),
see the README's "For agents & scripts" section and `roba --help`. This file is
about contributing.

## What this is

A library-first, provider-neutral runtime for one finite, single-root agent run
(Rust, edition 2024). Claude Code and Codex are provider adapters. Rust hosts
can retain a public `RunHandle` for status, replayable events, steering,
cancellation, and waiting; `roba run` is the current thin blocking CLI.

The v0.12 work adds an MCP-native layer above that finite core. The current
`roba-mcp` crate supplies one hot `AgentInstance`, typed `agent.turn`,
operation-scoped steering/interruption, logical shutdown, agent-wide replay,
`roba://agent`, optional live Tasks for `agent.turn`, and an in-process MCP
client. `roba run` crosses that contract and projects the typed result back
onto its compatibility ABI. Each finite operation also gets a private,
authenticated provider projection containing only the read-only `self` tool;
Claude and Codex receive that endpoint as transient launch context. External
operator transports and extensions remain later phases in
`docs/design/mcp-native-agent-harness.md`.

The original single-prompt Claude CLI remains a compatibility surface while
the provider-neutral API stabilizes.

## Scope line (read before adding any feature)

- IN: provider-neutral finite root runs, explicit execution authority,
  lifecycle and event observation, boundary-safe steering, cancellation, and
  thin library/CLI adapters.
- IN: the phase-gated, single-agent `roba-mcp` harness above core: one hot
  logical agent, one active finite run, one canonical MCP contract, and
  role-scoped operator/provider views.
- IN: the legacy Claude one-shot path, its config/profile/persona surface, and
  its read-only inspection commands.
- OUT: Roba-owned worker trees, a mission projection, a multi-agent server, a
  hidden daemon, a persistent session pool, a built-in scheduler or queue,
  hidden background work, or mutation of provider-private state.
- PARKED: Roba-to-Roba federation, operator-facing Unix/HTTP bindings without
  demonstrated demand, and broad GitHub workflow policy. These require
  separate evidence after the base harness. `mcp-repl` remains the interactive
  client, so a custom Roba REPL is not required.
- Keep reusable provider mechanics in the wrapper crates. Keep workflow policy
  outside the core run abstraction.
- Steward in `ok-v` is a separate workflow layer and useful prior art, not a
  Roba subsystem.

## Structure

- `roba-core/src/{run,lifecycle,provider,runtime}.rs` -- public run contracts,
  single-root lifecycle, provider boundary plus transient launch context, and
  provider registry
- `roba-core/src/providers/{claude,codex}.rs` -- built-in provider adapters
- `roba-mcp/src/{agent,contract,events,router,provider_endpoint}.rs` -- hot
  single-agent state, typed MCP values, bounded agent-wide replay, role-scoped
  routers, in-process control client, and private provider callback binding
- `src/main.rs` entry point; `src/lib.rs` dispatch plus bounded and legacy paths
- `src/bounded.rs` -- explicit `roba run` flags to a suspended `RunSpec`, the
  in-process MCP call, and compatibility result projection
- `src/cli.rs` clap surface -- doc comments here ARE the `--help` reference
- `src/session.rs` legacy flag -> `QueryCommand` wiring; `src/env.rs` legacy
  `ROBA_*` overrides
- `src/profile/` legacy config layering; `src/show.rs`, `src/history.rs`,
  `src/cost.rs`, `src/worktree.rs`, `src/doctor.rs`, `src/jobs.rs` read-only
  subcommands; `src/receipt.rs` the run-receipt writer (the schema lives in
  `roba-types`)
- Doc homes: README (current concepts + agent ABI),
  `docs/design/run-library-pivot.md` (finite-core decision),
  `docs/design/mcp-native-agent-harness.md` (adopted phased implementation
  plan), `--help` (reference generated from `cli.rs`), and parse-tested legacy
  config examples

## Build and test (all must pass before a PR)

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo build --workspace --all-features
cargo test --workspace --lib --all-features
cargo test --test cli --all-features
cargo test --workspace --doc --all-features
RUSTDOCFLAGS='-D warnings' cargo doc --workspace --all-features --no-deps
cargo build --release --all-features
git diff --check
```

Also run `cargo test -p roba-mcp --all-features` so its integration tests are
included.

`--workspace` covers the member crates alongside the `roba` binary:
`roba-types` (the published, dependency-light machine contract: `--json`
envelopes, the exit-code map, run receipts), `roba-core` (the clap-free
provider-and-run engine), and `roba-mcp` (the process-local single-agent MCP
host). The `cli` integration tests are Roba-only.

`tests/live.rs` calls real Claude or Codex and can cost money: `#[ignore]` by
default, run explicitly with `cargo test --test live -- --ignored`. Live tests
assert mechanics you control (a flag plumbs through, the envelope shape, exit
codes), never model compliance.

## Adding a CLI flag

For an explicit provider-neutral `roba run` flag: add the clap field in
`cli.rs` (terse first doc line plus detail), map it to `RunSpec` in
`bounded.rs`, add parse-level conflict tests, add a mechanical test in
`tests/cli.rs` when it touches exit codes or stream routing, and document it in
the README if it is part of the agent ABI. Do not add it to legacy environment,
profile, or config layering.

For a legacy one-shot flag, follow the full compatibility checklist in order:
clap field in `cli.rs` -> `session.rs` wiring -> `ROBA_<PARAM>` override in
`env.rs` + tests -> a `config.rs` `ENV_MAP` entry (so `config show --sources`
reports provenance) -> `Profile` field + `is_empty()` + `merge_in` + resolve
merge + tests -> parse-level conflict tests -> mechanical `tests/cli.rs`
coverage for exit codes or stream routing -> one `#[ignore]` live test -> a
commented line in `roba-config.sample.toml` -> README when it is part of the
agent ABI.

## Test placement

- clap rules that fire before `Cli::parse()` returns (conflicts, requires):
  unit tests in `src/cli.rs` via `Cli::try_parse_from`.
- Exit codes and stdout/stderr routing: `tests/cli.rs` (`assert_cmd`, never
  calls a provider).

## Conventions

- Conventional commits (`type: description`; `!` marks breaking). The commit
  subject IS the changelog entry (release-plz + git-cliff), so never hand-edit
  `CHANGELOG.md`.
- No emojis. No em dashes; use `--` or rephrase.
- Never commit to main: feature branch, then a PR. No "Generated with" or
  "Co-Authored-By" lines in commits or PRs.
- `anyhow` for errors (this is an app). Builder methods return `Self`. No unsafe
  code.
- stdout is the answer, stderr is metadata. Nothing decorative may leak to
  stdout; `--json` output must stay byte-clean.

## PRs

- Open the PR early with the plan as the body; link issues with `closes #N`
  (one keyword per issue number).
- CI runs macOS / Linux (stable + beta) / Windows plus fmt, clippy, docs, and a
  release-build check. All must be green.
