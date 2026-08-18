# Roba documentation

This directory contains current architectural decisions. It is deliberately
small: historical proposals and implementation journals remain available in
Git history, but they are not kept beside the active contract where they can be
mistaken for shipped behavior.

Documentation has five authoritative homes:

- the root `README.md` explains the current product and its main workflows;
- `roba --help` and subcommand help are the CLI reference generated from
  `src/cli.rs`;
- crate READMEs explain the public Rust surfaces owned by each workspace crate;
- `docs/architecture/` records the durable decisions and boundaries behind the
  current implementation;
- `legacy-cli.md` records the supported Claude compatibility workflows and
  stable scripting contract.

The practical guide [`running-roba.md`](running-roba.md) builds from one finite
CLI run to a hot, observable MCP agent using only shipped behavior. It also
marks future compositions explicitly instead of presenting them as available.

The annotated `roba-config.sample.toml` is the reference for the legacy
one-shot configuration format. It and every tracked example configuration are
parsed and semantically linted by the test suite.

Plans belong in GitHub issues until adopted. Once implemented, retain the
decision and its rationale here, not a phase ledger or working-session log.

## Current architecture

- [`running-roba.md`](running-roba.md) -- progressively richer CLI, MCP, and
  combined examples.

- [`architecture/core.md`](architecture/core.md) -- the finite,
  provider-neutral execution core.
- [`architecture/mcp-harness.md`](architecture/mcp-harness.md) -- the hot,
  single-agent MCP application layer above the core.
- [`architecture/agent-control.md`](architecture/agent-control.md) -- exact
  turn, follow-up, override, interruption, and configuration lifetimes.
- [`architecture/context.md`](architecture/context.md) -- the typed context
  plan, current provider inventory, and isolation boundaries.
- [`legacy-cli.md`](legacy-cli.md) -- the Claude-only compatibility command,
  configuration model, and scripting ABI.
