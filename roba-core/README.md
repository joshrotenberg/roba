# roba-core

Provider-neutral contracts and a process-local lifecycle for one finite,
single-root Roba run.

[roba](https://github.com/joshrotenberg/roba) is library-first. This crate lets
a Rust host construct, execute, observe, steer, cancel, and await one run
without depending on clap, terminal behavior, a daemon, or persistent storage.

## What's here

- **`run` / `provider`** -- provider-neutral specifications, events, outcomes,
  typed failures, sessions, capabilities, and the one-turn provider contract.
- **`lifecycle` / `runtime`** -- suspended creation, exact-once start,
  boundary-safe steering, bounded replayable events, cancellation, waiting,
  and an explicit provider registry.
- **`providers`** -- Claude and Codex adapters that normalize provider-native
  results without inventing missing usage or cost.
- **`engine`** -- the legacy Claude `Config -> QueryResult` seam retained while
  the established CLI remains compatible.
- **`session`** -- `apply_session`, the single legacy `Config -> QueryCommand`
  mapper consumed by that engine.

The provider-neutral API has no stdout/stderr, `process::exit`, TTY, clap,
database, queue, or global session pool. Provider adapters spawn the selected
provider CLI only after a run starts. A promptless suspended run spawns
nothing.

## Run control

`Roba` owns an explicit process-local provider registry. `create_run` captures
an immutable `RunSpec` and returns a `Run`; cloning `Run::handle()` produces a
`RunHandle` with the public control surface:

- `start` supplies the first prompt to a suspended run; `begin` starts a spec
  that already contains one.
- `status` returns the latest in-memory `RunSnapshot`.
- `subscribe` and `subscribe_after` replay retained events before waiting for
  new events.
- `event_page` and `wait_for_events` support explicit cursors and bounded
  long-lived observation.
- `steer` queues guidance for the next safe provider-turn boundary when the
  provider supports resume.
- `cancel` drops active provider work before publishing terminal cancellation.
- `wait` resolves to the terminal snapshot.

Each event record carries a monotonically increasing sequence. The in-memory
journal retains 256 records and reports truncation when a caller's cursor has
fallen behind. Cursors ahead of the journal are refused.

`RunFailure` carries a portable failure category and optional
`RunFailureDetails`. Providers preserve honestly reported terminal recovery
and accounting fields such as a resumable session, usage, cost, duration, and
provider turn count. The same failure is retained by the handle and emitted as
a `RunEvent::Failed`; missing telemetry remains absent.

## Deliberate boundary

The current crate owns one root run only. The prior worker tree, mission
projection, process-capability registry, and GitHub-specific workflow/process
pack are parked and are not part of this API. The former `roba-mcp` and
`roba-repl` crates are also not workspace members.

A future run-scoped MCP adapter may expose a narrow subset of `RunHandle` for
observability and steering. It should contain no execution logic of its own.
An external client such as `mcp-repl` can supply an interactive interface, so
the core does not require a custom REPL.

Steward in `ok-v` is separate workflow-layer prior art. It may consume or
drive Roba in another project, but it is not part of `roba-core`.

## Stability

The provider-neutral API is under active development and may change before the
next stable Roba release. The legacy engine remains available for
compatibility.

## License

MIT OR Apache-2.0.
