# roba-core

Provider-neutral contracts and a process-local lifecycle for one finite,
single-root Roba run.

[Roba](https://github.com/joshrotenberg/roba) is library-first. This crate lets
a Rust host construct, execute, observe, follow up, cancel, and await one run
without depending on clap, terminal behavior, MCP, a daemon, or persistence.

## What's here

- `run` and `provider` define specifications, events, outcomes, typed failures,
  sessions, capabilities, transient launch context, and the provider boundary.
- `lifecycle` and `runtime` implement suspended creation, exact-once start,
  safe follow-up boundaries, bounded replay, cancellation, waiting, and an
  explicit provider registry.
- `providers` contains Claude and Codex adapters that normalize only telemetry
  the provider actually reports.

Provider-specific command construction is private adapter machinery. It is not
a second public configuration model.

## Run control

`Roba` owns a process-local provider registry. `create_run` captures an
immutable `RunSpec` and returns a `Run`; `Run::handle()` produces a cloneable
`RunHandle` with this control surface:

- `start` supplies the first prompt to a suspended run; `begin` starts a spec
  that already contains one.
- `status` returns the latest in-memory `RunSnapshot`.
- `subscribe`, `event_page`, and `wait_for_events` expose bounded replay.
- `follow_up` queues another prompt at a resumable provider-turn boundary.
- `cancel` drops active provider work before terminal cancellation is visible.
- `wait` resolves only after terminal settlement.

Every event has a monotonically increasing sequence. The bounded journal makes
history loss explicit and rejects future cursors.

`RunFailure` carries a portable category and optional `RunFailureDetails`.
Providers preserve honestly observed session, usage, cost, duration, and turn
evidence; missing telemetry stays absent.

## Boundary

The crate owns one finite root run. It does not own MCP schemas or transports,
hot-agent lifetime, queues, schedules, Git/GitHub policy, worker trees, or
multi-agent routing. `roba-mcp` composes those application concerns above the
core without making `roba-core` stateful or transport-aware.

## Stability

The provider-neutral API is under active development and may change between
minor releases while it is hardened.

## License

MIT OR Apache-2.0.
