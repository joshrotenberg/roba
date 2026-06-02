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
| [aliases.md](aliases.md) | `git`-style `[alias.NAME]` shortcuts: schema, lookup, substitution |
| [scripting.md](scripting.md) | Agent ABI: stdout/stderr split, versioned `--json` envelope, typed exit codes, `--no-retry` |
| [permissions.md](permissions.md) | Safe-by-default model, cross-layer precedence, `--show-permissions` |
| [vs-claude-p.md](vs-claude-p.md) | When to reach for `roba` vs plain `claude -p`, with side-by-side examples |
| [use-cases.md](use-cases.md) | Cookbook of patterns roba enables, seeded with multi-repo orchestration |
| [examples/github-actions/](examples/github-actions/) | Example workflow YAML for running roba in CI (PR auto-review) |

## Where things live

- **Code:** `src/` -- one module per concern (cli, prompt, output,
  render, session, stream, history, profile, cost)
- **Tests:** `src/*.rs` (unit), `tests/cli.rs` (mechanical),
  `tests/live.rs` (real claude, `#[ignore]`)
- **Changelog:** `CHANGELOG.md` -- conventional commits via release-plz
- **Crates.io page:** see [`README.md`](../README.md)
