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
  boundary-safe steering, observation, cancellation, waiting, and an explicit
  provider registry. No daemon, database, queue, or global session pool.
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

## Stability

The bounded-run API is under active development and may change before the next
stable Roba release. The legacy engine remains available for compatibility.

## License

MIT OR Apache-2.0.
