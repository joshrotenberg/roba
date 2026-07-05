//! roba's stable machine contract, as a dependency-light library.
//!
//! roba is meant to be consumed as a subprocess whose `--json` output and
//! exit code are a stable ABI. This crate is that ABI, extracted so a
//! downstream harness can deserialize against it and branch on the exit code
//! without depending on the whole `roba` binary (tokio, clap, termimad, ...).
//!
//! Two pieces:
//!
//! - **Exit codes** ([`EXIT_FAILURE`] .. [`EXIT_MAX_BUDGET`]) -- the full map
//!   the binary returns. The `roba` binary references these same constants, so
//!   the crate and the binary cannot disagree.
//! - **`--json` envelopes** ([`SuccessEnvelope`], [`VersionedResult`],
//!   [`ErrorEnvelope`]) -- the uniform `{ version, result[, refusal] }` /
//!   `{ version, error }` shapes. Each is generic over the payload and derives
//!   both `Serialize` (roba serializes a borrow, no clone) and `Deserialize`
//!   (a consumer deserializes owned), so there is one type per shape and no
//!   drift between producer and consumer.
//!
//! The result payload is claude's own [`QueryResult`], re-exported here. This
//! crate pulls `claude-wrapper` with `default-features = false, features =
//! ["json"]`, so it carries serde but **no async runtime**.

use serde::{Deserialize, Serialize};

/// claude's result payload -- the shape inside a success envelope's `result`.
/// Re-exported (not mirrored) so it cannot drift from claude's contract.
pub use claude_wrapper::types::QueryResult;

/// The current `--json` ABI version. Every envelope carries it as the first
/// field a consumer should check before inspecting anything else.
pub const VERSION: u32 = 1;

// -- Exit codes -------------------------------------------------------------
//
// The full map the `roba` binary returns. `classify_exit_code` in the binary
// references these constants, so a change here changes both. `0` (success) is
// not a named constant -- it is the absence of a failure code.

/// Generic failure -- a `claude` run failed for a reason with no more specific
/// code, or roba itself errored.
pub const EXIT_FAILURE: i32 = 1;
/// Authentication failure (`claude` is not logged in / the key is bad).
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

/// The `--json` success envelope for a prompt run: `{ version, result,
/// refusal }`. `result` holds a [`QueryResult`]; `refusal` is true when the
/// response looked like a refusal, so a consumer can branch on
/// "got an answer" vs "got refused" without parsing the body.
///
/// Generic over the payload so the producer serializes with `T = &QueryResult`
/// (no clone) and a consumer deserializes `SuccessEnvelope<QueryResult>`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuccessEnvelope<T> {
    /// ABI version ([`VERSION`]).
    pub version: u32,
    /// The run's result payload.
    pub result: T,
    /// True when the response looked like a refusal.
    pub refusal: bool,
}

impl<T> SuccessEnvelope<T> {
    /// Wrap a payload at the current ABI version.
    pub fn new(result: T, refusal: bool) -> Self {
        Self {
            version: VERSION,
            result,
            refusal,
        }
    }
}

/// The `--json` success envelope for the read-only management commands
/// (`cost`, `history`, `last`, `doctor`, `worktree list`, ...): `{ version,
/// result }`, without the prompt-run-only `refusal` flag. Generic over the
/// payload `T`, which differs per command.
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
    /// A small string union: `"auth" | "budget" | "timeout" | "history" |
    /// "other"`. Mirrors the exit-code dispatch; a consumer can match on it or
    /// on the [`exit_code`](Self::exit_code).
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
    fn success_envelope_round_trips() {
        // Serialize as a borrow (the producer shape), deserialize as owned
        // (the consumer shape) -- the two directions agree on one struct.
        let payload = serde_json::json!({ "result": "hi", "session_id": "s1" });
        let json = serde_json::to_string(&SuccessEnvelope::new(&payload, false)).unwrap();
        let back: SuccessEnvelope<serde_json::Value> = serde_json::from_str(&json).unwrap();
        assert_eq!(back.version, 1);
        assert!(!back.refusal);
        assert_eq!(back.result["session_id"], "s1");
    }

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
