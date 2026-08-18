# AGENTS.md

Instructions for coding agents contributing to Roba. For driving Roba, use
`roba --help`, `roba run --help`, `roba serve --help`, and
[`docs/running-roba.md`](docs/running-roba.md).

Read [`ARCHITECTURE.md`](ARCHITECTURE.md) before changing a top-level layer,
ownership boundary, authority path, lifecycle, or evidence contract.

## What this is

Roba is a library-first, MCP-native harness for one logical coding agent
(Rust, edition 2024). `roba-core` executes one finite, provider-neutral run.
`roba-mcp` retains one logical agent across finite runs and exposes a typed MCP
contract. Claude Code and Codex are built-in provider adapters.

The root binary has four command groups:

- `run` executes one finite operation through the process-local MCP contract;
- `serve` hosts one hot agent over stdio MCP;
- `config effective` explains the resolved versioned startup configuration;
- `completions` generates shell completions.

`roba-git` is the first optional service composed into the MCP router. It
captures one repository at startup, exposes read-only state to operator and
provider projections, and exposes staging only to writable operator views.

## Scope line

- IN: one logical agent, one active finite run, explicit execution authority,
  retained provider session continuity, lifecycle observation, replayable
  events, follow-up, interruption, and clean settlement.
- IN: typed operator and least-authority provider MCP projections, immutable
  fail-closed router fragments, and thin in-process/stdio interfaces.
- IN: strict, versioned startup configuration shared by `run` and `serve`.
- OUT of core: queues, schedules, GitHub workflow policy, repository managers,
  and Roba-to-Roba orchestration. Those may be optional MCP-native layers.
- OUT: hidden background work, provider-private state mutation, a built-in
  worker tree, or a multi-agent server disguised as one agent.
- Keep reusable provider mechanics in provider adapters. Keep workflow policy
  out of `roba-core`.

## Structure

- `crates/roba-core/src/{run,lifecycle,provider,runtime}.rs` -- finite run
  contracts, lifecycle, provider boundary, transient launch context, registry.
- `crates/roba-core/src/providers/{claude,codex}.rs` -- built-in adapters.
- `crates/roba-mcp/src/{agent,contract,events,extensions,extension_lifecycle,router,stdio,provider_endpoint}.rs`
  -- hot-agent state, MCP values, replay, composition, and bindings.
- `crates/roba-git` -- fixed-workspace Git MCP service and cached operation
  progress observer.
- `crates/roba-types` -- dependency-light JSON envelopes and exit-code map.
- `src/cli.rs` -- clap surface; its doc comments are the help reference.
- `src/startup_config.rs` -- discovery, layering, validation, and provenance.
- `src/bounded.rs` -- finite CLI resolution and process-local MCP call.
- `src/serve.rs` -- stdio host, signal policy, and graceful shutdown.
- `src/{main,lib,error}.rs` -- entry point, dispatch, and machine/human errors.
- `ARCHITECTURE.md` -- authoritative system map, ownership boundaries, and
  invariants.
- `docs/architecture/` -- implemented architectural contracts.
- `docs/running-roba.md` -- progressively richer usage examples.

## Build and test

All gates must pass before a PR:

```bash
cargo fmt --all -- --check
cargo install mdbook-lint --version 0.15.2 --locked
git ls-files -z '*.md' | xargs -0 mdbook-lint lint \
  --config .mdbook-lint.toml --fail-on-warnings
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo build --workspace --all-features
cargo test --workspace --lib --all-features
cargo test --test cli --all-features
cargo test -p roba-mcp --all-features
cargo test -p roba-git --all-features
cargo test --workspace --doc --all-features
RUSTDOCFLAGS='-D warnings' cargo doc --workspace --all-features --no-deps
cargo build --release --all-features
git diff --check
```

`tests/live.rs` calls real providers and can cost money. It is ignored by
default; run it explicitly with `cargo test --test live -- --ignored`.

## Changes by surface

For a `run` or `serve` setting, update the shared clap field, strict versioned
file schema, precedence rule, `RunSpec` mapping, safe effective/provenance
view, parse tests, black-box tests where observable, and documentation.

Host services such as `--git` stay outside serialized `RunSpec`. Compose their
role-specific routers before constructing `AgentInstance`, fail closed on
collisions, and pass only validated exact provider tool names through the
transient launch context.

MCP contract changes need schema/discovery tests plus behavior through a real
`McpClient` and `ChannelTransport`. Long operations also need cancellation,
settlement, replay, and stale-operation tests. Provider-facing capabilities
must prove they are absent from unauthorized projections, not merely hidden
from discovery.

## Test placement

- Clap conflicts and requirements: unit tests in `src/cli.rs` using
  `Cli::try_parse_from`.
- Exit codes, stdout/stderr routing, startup config, and binary stdio wiring:
  `tests/cli.rs` with deterministic fake providers when needed.
- Finite lifecycle/provider behavior: `roba-core` unit and integration tests.
- Hot-agent and MCP wire behavior: `roba-mcp/tests`.
- Git service behavior and cross-layer callback proof: `roba-git/tests`.

## Conventions

- Conventional commits (`type: description`; `!` marks breaking). The commit
  subject is the changelog entry. Never hand-edit `CHANGELOG.md`.
- No emojis. No em dashes; use `--` or rephrase.
- Never commit to `main`. Use a feature branch and PR. Do not add generated or
  co-author trailers.
- Use `anyhow` for application errors. Builder methods return `Self`. No
  unsafe code.
- Stdout is answer or wire data; stderr is metadata. JSON and MCP output must
  remain byte-clean.

## PRs

Open the PR early with the plan in its body. Link issues with `closes #N`, one
keyword per issue. CI runs macOS, Linux stable/beta, Windows, formatting,
clippy, docs, and release builds; all must pass.
