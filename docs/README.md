# roba docs

Reference material for [`roba`](../), the single-prompt CLI runner
built on [`claude-wrapper`](https://crates.io/crates/claude-wrapper).

The repo-level [`README.md`](../README.md) is the marketing-shaped
intro -- start there if you're new. The docs here go deeper on
specific topics.

## Topics

| Doc | What |
|---|---|
| [profiles.md](profiles.md) | `roba.toml` config + profiles, schema, worked examples |
| [design-notes.md](design-notes.md) | Brainstorm / idea log -- design rationale, future directions |

## Where things live

- **Code:** `src/` -- one module per concern (cli, prompt, output,
  render, session, stream, history, profile, cost)
- **Tests:** `src/*.rs` (unit), `tests/cli.rs` (mechanical),
  `tests/live.rs` (real claude, `#[ignore]`)
- **Changelog:** `CHANGELOG.md` -- conventional commits via release-plz
- **Crates.io page:** see [`README.md`](../README.md)
