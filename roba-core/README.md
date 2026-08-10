# roba-core

Provider-neutral contracts and a process-local lifecycle for one bounded Roba
agent run.

[roba](https://github.com/joshrotenberg/roba) is library-first. This crate owns
the run model beneath its CLI, REPL, and run-scoped MCP adapters, so a Rust host
can create and control a run without depending on clap or terminal behavior.

## What's here

- **`run` / `provider`** -- provider-neutral specifications, events, outcomes,
  failures, sessions, and the one-turn provider contract.
- **`lifecycle` / `runtime`** -- suspended creation, exact-once start,
  boundary-safe steering, bounded replayable run-tree events, cancellation,
  waiting, and an explicit provider registry. No daemon, database, queue, or
  global session pool.
- **`resolve`** -- one serializable hierarchy: Roba defaults, selected provider
  defaults, named agent, then run overrides.
- **`providers`** -- Claude and Codex adapters that normalize provider-native
  results without inventing missing usage or cost.
- **`engine`** -- the legacy Claude `Config -> QueryResult` seam retained while
  the established CLI migrates onto the new run model.
- **`session`** -- `apply_session`, the single `Config -> QueryCommand` mapper
  the compatibility engine feeds.

The new API has no stdout/stderr, `process::exit`, TTY, clap, or persistent
server dependency. Provider adapters do spawn the selected provider CLI once a
run starts; a prompt-less suspended run spawns nothing.

`RunHandle::subscribe()` replays the oldest event still held by the run tree
and then waits for new events. Each record carries a tree-wide sequence and the
emitting run id. The root sees its entire worker tree; a child sees only itself
and its descendants. Hosts that keep their own cursor can use `event_page` and
`wait_for_events`. The in-memory journal retains 256 records and reports
subtree-scoped truncation instead of pretending older events still exist.
Subscriptions return `RunEventSubscriptionItem::HistoryTruncated` before
replaying retained records after a gap, and cursors ahead of the journal are
refused.

`RunFailure` carries a portable failure category and optional
`RunFailureDetails`. Providers use those details for honestly reported
terminal recovery and accounting fields such as a resumable session, usage,
cost, duration, and provider turn count. The same failure snapshot is retained
by the handle and emitted as a `RunEvent::Failed`; missing telemetry remains
absent.

## Stability

The bounded-run API is under active development and may change before the next
stable Roba release. The legacy engine remains available for compatibility.

## License

MIT OR Apache-2.0.
