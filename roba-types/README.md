# roba-types

roba's stable machine contract, as a dependency-light library: the `--json`
envelope shapes and the exit-code map. No async runtime (no tokio).

[roba](https://github.com/joshrotenberg/roba) is meant to be driven as a
subprocess whose `--json` output and exit code are a stable ABI. This crate is
that ABI, so a downstream harness can deserialize against it and branch on the
exit code without depending on the whole `roba` binary.

## What's here

- **Exit codes** -- `EXIT_FAILURE` (1) through `EXIT_MAX_BUDGET` (7), the full
  map the binary returns. The `roba` binary references these same constants.
- **Envelopes** -- `SuccessEnvelope<T>` (`{ version, result, refusal }`),
  `VersionedResult<T>` (`{ version, result }`, read-only commands and bounded
  terminal run snapshots), and
  `ErrorEnvelope` (`{ version, error }`). Each derives `Serialize` and
  `Deserialize`, so producer and consumer share one type per shape.
- **`QueryResult`** -- claude's result payload, re-exported (not mirrored).

## Example

```rust
use roba_types::{SuccessEnvelope, QueryResult, EXIT_MAX_TURNS};

let env: SuccessEnvelope<QueryResult> = serde_json::from_str(stdout)?;
if exit_code == EXIT_MAX_TURNS {
    // recoverable cap-hit: resume the session and finish the lifecycle
}
```

## License

MIT OR Apache-2.0.
