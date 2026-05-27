# roba docs

Reference material for [`roba`](../../crates/roba/), the single-prompt
CLI runner built on [`claude-wrapper`](../../crates/claude-wrapper/).

The crate-level [`crates/roba/README.md`](../../crates/roba/README.md)
is the marketing-shaped intro -- start there if you're new. The
docs here go deeper on specific topics.

## Topics

| Doc | What |
|---|---|
| [profiles.md](profiles.md) | `~/.config/roba/profiles.toml`, schema, worked examples |
| [design-notes.md](design-notes.md) | Brainstorm / idea log -- design rationale, future directions |

## Where things live

- **Code:** `crates/roba/src/` -- one module per concern (cli, prompt,
  output, render, session, stream, history, profile, cost)
- **Tests:** `crates/roba/src/*.rs` (unit), `crates/roba/tests/cli.rs`
  (mechanical), `crates/roba/tests/live.rs` (real claude, `#[ignore]`)
- **Changelog:** `crates/roba/CHANGELOG.md` -- per-crate, conventional
  commits via release-plz
- **Crates.io page:** see `crates/roba/README.md`
