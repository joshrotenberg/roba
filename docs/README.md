# cwr docs

Reference material for [`cwr`](../../crates/cwr/), the single-prompt
CLI runner built on [`claude-wrapper`](../../crates/claude-wrapper/).

The crate-level [`crates/cwr/README.md`](../../crates/cwr/README.md)
is the marketing-shaped intro -- start there if you're new. The
docs here go deeper on specific topics.

## Topics

| Doc | What |
|---|---|
| [profiles.md](profiles.md) | `~/.config/cwr/profiles.toml`, schema, worked examples |
| [design-notes.md](design-notes.md) | Brainstorm / idea log -- design rationale, future directions |

## Where things live

- **Code:** `crates/cwr/src/` -- one module per concern (cli, prompt,
  output, render, session, stream, history, profile, cost)
- **Tests:** `crates/cwr/src/*.rs` (unit), `crates/cwr/tests/cli.rs`
  (mechanical), `crates/cwr/tests/live.rs` (real claude, `#[ignore]`)
- **Changelog:** `crates/cwr/CHANGELOG.md` -- per-crate, conventional
  commits via release-plz
- **Crates.io page:** see `crates/cwr/README.md`
