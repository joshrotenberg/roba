# roba-types

Dependency-light machine contracts shared by the `roba` binary and subprocess
clients.

## What's here

- `EXIT_FAILURE` (1) through `EXIT_MAX_BUDGET` (7), the process exit-code map.
- `VersionedResult<T>`, the provider-neutral `{ version, result }` success
  envelope used by `roba run --json` and `roba config effective --json`.
- `ErrorEnvelope`, the `{ version, error }` failure envelope written to stderr.

The crate depends only on Serde. Provider-native result types, detached-run
receipts, queues, and process management are not part of this contract.

```rust
use roba_types::{EXIT_MAX_TURNS, VersionedResult};

let envelope: VersionedResult<serde_json::Value> = serde_json::from_str(stdout)?;
if exit_code == EXIT_MAX_TURNS {
    // The provider hit a recoverable turn guardrail.
}
# Ok::<(), serde_json::Error>(())
```

## License

MIT OR Apache-2.0.
