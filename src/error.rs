//! JSON error envelope for `--json` mode.
//!
//! When `--json` is on and a runtime error occurs, roba emits a
//! structured envelope to stderr instead of the plain anyhow text.
//! Agents reading roba's stderr can parse this directly.
//!
//! Shape:
//!
//! ```text
//! {
//!   "version": 1,
//!   "error": {
//!     "kind": "auth" | "budget" | "timeout" | "history" | "other",
//!     "message": "human-readable summary",
//!     "exit_code": <int>,
//!     "chain": ["top context", "...", "root cause"]
//!   }
//! }
//! ```
//!
//! The `kind` mirrors the same dispatch [`crate::classify_exit_code`]
//! uses: it inspects the underlying `claude_wrapper::Error` variant
//! when present, otherwise falls back to `"other"`.
//!
//! ## Versioned ABI (v1)
//!
//! The top-level `version` field is the stability contract for
//! programmatic consumers. It is present on every `--json` output --
//! both this error envelope and the success envelope (`{ "version":
//! 1, "result": {...} }`, see [`crate::run_ask`]). Peel off `version`
//! before inspecting anything inside.
//!
//! Version 1 guarantees:
//!
//! - Top-level `version: 1` is present on every `--json` output.
//! - Success output carries a `result` field; error output carries an
//!   `error` field. The two are mutually exclusive.
//! - Inner fields documented at v1 are preserved. New fields may be
//!   added in a backward-compatible (additive) way without bumping the
//!   version.
//! - Breaking shape changes (renames, removals, type changes) require
//!   a version bump.

use serde::Serialize;

/// Outer envelope. Carries the top-level `version` ABI marker plus an
/// `"error"` key, which makes it structurally distinct from the
/// success envelope (which carries `result` instead). `version` is
/// listed first so it sorts to the top of pretty-printed JSON.
#[derive(Debug, Serialize)]
pub struct ErrorEnvelope {
    pub version: u32,
    pub error: ErrorBody,
}

/// Inner payload. `kind` is a small string union; `chain` lists the
/// anyhow context layers from top (most recent context call) to root
/// (the underlying error).
#[derive(Debug, Serialize)]
pub struct ErrorBody {
    pub kind: &'static str,
    pub message: String,
    pub exit_code: i32,
    pub chain: Vec<String>,
}

/// Classify an [`anyhow::Error`] into the envelope's `kind` string.
/// Matches the wrapper variant when downcastable; everything else is
/// `"other"`.
pub fn kind_of(err: &anyhow::Error) -> &'static str {
    if let Some(wrapper_err) = err.downcast_ref::<claude_wrapper::Error>() {
        match wrapper_err {
            claude_wrapper::Error::Auth { .. } => "auth",
            claude_wrapper::Error::BudgetExceeded { .. } => "budget",
            claude_wrapper::Error::Timeout { .. } => "timeout",
            claude_wrapper::Error::History { .. } => "history",
            _ => "other",
        }
    } else {
        "other"
    }
}

/// Build the envelope from an error + the already-computed exit code.
/// `message` is the top of the anyhow chain (the most recent context
/// call), matching what `{err}` would print without `:#`.
pub fn build_envelope(err: &anyhow::Error, exit_code: i32) -> ErrorEnvelope {
    let chain: Vec<String> = err.chain().map(|c| c.to_string()).collect();
    let message = chain.first().cloned().unwrap_or_else(|| err.to_string());
    ErrorEnvelope {
        version: 1,
        error: ErrorBody {
            kind: kind_of(err),
            message,
            exit_code,
            chain,
        },
    }
}

/// Render the envelope as pretty-printed JSON for stderr. Falls back
/// to a hand-rolled minimal envelope only if serde itself errors --
/// which it shouldn't for this shape.
pub fn render_json(err: &anyhow::Error, exit_code: i32) -> String {
    let env = build_envelope(err, exit_code);
    serde_json::to_string_pretty(&env).unwrap_or_else(|_| {
        format!(
            "{{\"version\":1,\"error\":{{\"kind\":\"other\",\"message\":\"serialization failed\",\"exit_code\":{exit_code},\"chain\":[]}}}}"
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use claude_wrapper::auth::AuthErrorKind;
    use serde_json::Value;

    fn envelope_value(err: &anyhow::Error, exit_code: i32) -> Value {
        let json = render_json(err, exit_code);
        serde_json::from_str(&json).expect("envelope must round-trip through serde_json")
    }

    #[test]
    fn auth_variant_serializes_as_auth_kind() {
        let err = anyhow::Error::new(claude_wrapper::Error::Auth {
            kind: AuthErrorKind::NotAuthenticated,
            command: "claude -p hi".to_string(),
            exit_code: 1,
            message: "not logged in".to_string(),
        });
        let value = envelope_value(&err, 2);
        assert_eq!(value["version"], 1, "top-level version must be 1");
        assert_eq!(value["error"]["kind"], "auth");
        assert_eq!(value["error"]["exit_code"], 2);
        assert!(value["error"]["chain"].is_array(), "chain must be an array");
        assert!(
            !value["error"]["message"].as_str().unwrap().is_empty(),
            "message must not be empty"
        );
    }

    #[test]
    fn budget_variant_serializes_as_budget_kind() {
        let err = anyhow::Error::new(claude_wrapper::Error::BudgetExceeded {
            total_usd: 5.0,
            max_usd: 4.0,
        });
        let value = envelope_value(&err, 3);
        assert_eq!(value["error"]["kind"], "budget");
        assert_eq!(value["error"]["exit_code"], 3);
    }

    #[test]
    fn timeout_variant_serializes_as_timeout_kind() {
        let err = anyhow::Error::new(claude_wrapper::Error::Timeout {
            timeout_seconds: 30,
        });
        let value = envelope_value(&err, 4);
        assert_eq!(value["error"]["kind"], "timeout");
        assert_eq!(value["error"]["exit_code"], 4);
    }

    #[test]
    fn history_variant_serializes_as_history_kind() {
        let err = anyhow::Error::new(claude_wrapper::Error::History {
            message: "no such project".to_string(),
        });
        let value = envelope_value(&err, 1);
        assert_eq!(value["error"]["kind"], "history");
        assert_eq!(value["error"]["exit_code"], 1);
    }

    #[test]
    fn non_wrapper_error_serializes_as_other_kind() {
        let err = anyhow::anyhow!("something else broke");
        let value = envelope_value(&err, 1);
        assert_eq!(value["version"], 1, "top-level version must be 1");
        assert_eq!(value["error"]["kind"], "other");
        assert_eq!(value["error"]["exit_code"], 1);
        assert_eq!(value["error"]["message"], "something else broke");
    }

    #[test]
    fn chain_preserves_top_to_root_order() {
        let root = anyhow::anyhow!("inner detail");
        let mid = root.context("middle layer");
        let top = mid.context("top context");
        let value = envelope_value(&top, 1);
        let chain: Vec<String> = value["error"]["chain"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        assert_eq!(
            chain,
            vec![
                "top context".to_string(),
                "middle layer".to_string(),
                "inner detail".to_string(),
            ]
        );
        // message is the top of the chain
        assert_eq!(value["error"]["message"], "top context");
    }

    #[test]
    fn rendered_json_is_parseable() {
        let err = anyhow::anyhow!("anything");
        let rendered = render_json(&err, 1);
        let parsed: Value = serde_json::from_str(&rendered).expect("parseable");
        assert_eq!(parsed["error"]["kind"], "other");
    }
}
