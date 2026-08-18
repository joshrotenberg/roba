//! Roba's provider-neutral machine contract, as a dependency-light library.
//!
//! Roba can be consumed as a subprocess whose `--json` output and exit code
//! form a machine ABI. This crate is that ABI, extracted so a
//! downstream harness can deserialize against it and branch on the exit code
//! without depending on the whole `roba` binary.
//!
//! Two pieces:
//!
//! - **Exit codes** ([`EXIT_FAILURE`] .. [`EXIT_MAX_BUDGET`]) -- the full map
//!   the binary returns. The `roba` binary references these same constants, so
//!   the crate and the binary cannot disagree.
//! - **JSON envelopes** ([`VersionedResult`] and [`ErrorEnvelope`]) -- the
//!   provider-neutral `{ version, result }` and `{ version, error }` shapes.
//!   They derive both `Serialize` and `Deserialize` so producer and consumer
//!   share one type per shape.

use serde::{Deserialize, Serialize};

/// The current `--json` ABI version. Every envelope carries it as the first
/// field a consumer should check before inspecting anything else.
pub const VERSION: u32 = 1;

// -- Exit codes -------------------------------------------------------------
//
// The full map the `roba` binary returns. `classify_exit_code` in the binary
// references these constants, so a change here changes both. `0` (success) is
// not a named constant -- it is the absence of a failure code.

/// Generic failure -- a provider run failed for a reason with no more specific
/// code, or Roba itself errored.
pub const EXIT_FAILURE: i32 = 1;
/// Authentication failure (the selected provider is not authenticated).
pub const EXIT_AUTH: i32 = 2;
/// The wrapper's own `BudgetTracker` ceiling was hit (distinct from claude's
/// `--max-budget-usd` CLI cap, which is [`EXIT_MAX_BUDGET`]).
pub const EXIT_BUDGET: i32 = 3;
/// The run exceeded its wall-clock `--timeout`.
pub const EXIT_TIMEOUT: i32 = 4;
/// The `--max-turns` cap was hit. Recoverable: the run is usually complete and
/// just needs its lifecycle finished (gates + commit), so a caller can tell
/// this apart from a hard failure and resume.
pub const EXIT_MAX_TURNS: i32 = 5;
/// A run produced no usable result (empty / `is_error`). Emitted by the binary
/// directly (never as an error envelope); the reliable machine signal is this
/// code, not stderr.
pub const EXIT_UNUSABLE_RESULT: i32 = 6;
/// The `--max-budget-usd` CLI spend cap was hit. Recoverable, like
/// [`EXIT_MAX_TURNS`]: a guardrail tripped mid-run, not a defect.
pub const EXIT_MAX_BUDGET: i32 = 7;

// -- Envelopes --------------------------------------------------------------

/// The `--json` success envelope: `{ version, result }`.
/// Generic over the provider-neutral payload `T`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionedResult<T> {
    /// ABI version ([`VERSION`]).
    pub version: u32,
    /// The command's result payload.
    pub result: T,
}

impl<T> VersionedResult<T> {
    /// Wrap a payload at the current ABI version.
    pub fn new(result: T) -> Self {
        Self {
            version: VERSION,
            result,
        }
    }
}

/// The `--json` failure envelope, emitted on stderr: `{ version, error }`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorEnvelope {
    /// ABI version ([`VERSION`]).
    pub version: u32,
    /// The failure detail.
    pub error: ErrorBody,
}

impl ErrorEnvelope {
    /// Wrap an error body at the current ABI version.
    pub fn new(error: ErrorBody) -> Self {
        Self {
            version: VERSION,
            error,
        }
    }
}

/// The failure detail inside an [`ErrorEnvelope`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorBody {
    /// A small string union such as `"auth"`, `"timeout"`, `"limit"`, or
    /// `"other"`. Mirrors the exit-code dispatch; a consumer can
    /// match on it or on the [`exit_code`](Self::exit_code).
    pub kind: String,
    /// A human-readable summary of the failure.
    pub message: String,
    /// The process exit code that accompanies this failure.
    pub exit_code: i32,
    /// The error-context chain, outermost first, root cause last.
    pub chain: Vec<String>,
    /// Optional doc URLs relevant to the failure. Omitted when empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub see_also: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versioned_result_round_trips() {
        let json = serde_json::to_string(&VersionedResult::new(&vec![1, 2, 3])).unwrap();
        assert_eq!(json, r#"{"version":1,"result":[1,2,3]}"#);
        let back: VersionedResult<Vec<i32>> = serde_json::from_str(&json).unwrap();
        assert_eq!(back.result, vec![1, 2, 3]);
    }

    #[test]
    fn error_envelope_shape_and_see_also_omitted_when_empty() {
        let env = ErrorEnvelope::new(ErrorBody {
            kind: "auth".to_string(),
            message: "not authenticated".to_string(),
            exit_code: EXIT_AUTH,
            chain: vec!["top".to_string(), "root".to_string()],
            see_also: Vec::new(),
        });
        let json = serde_json::to_string(&env).unwrap();
        assert!(json.contains(r#""version":1"#), "{json}");
        assert!(json.contains(r#""kind":"auth""#), "{json}");
        assert!(json.contains(r#""exit_code":2"#), "{json}");
        assert!(
            !json.contains("see_also"),
            "empty see_also must be omitted: {json}"
        );
        // Round-trips back to an owned body.
        let back: ErrorEnvelope = serde_json::from_str(&json).unwrap();
        assert_eq!(back.error.exit_code, 2);
        assert!(back.error.see_also.is_empty());
    }

    #[test]
    fn exit_codes_are_the_stable_map() {
        assert_eq!(
            (
                EXIT_FAILURE,
                EXIT_AUTH,
                EXIT_BUDGET,
                EXIT_TIMEOUT,
                EXIT_MAX_TURNS,
                EXIT_UNUSABLE_RESULT,
                EXIT_MAX_BUDGET
            ),
            (1, 2, 3, 4, 5, 6, 7)
        );
    }
}
