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
//!     "chain": ["top context", "...", "root cause"],
//!     "see_also": ["https://.../doc"]   // optional; omitted when empty
//!   }
//! }
//! ```
//!
//! `see_also` is an additive v1 field: a list of canonical doc URLs an
//! error wants to point at (e.g. the `--show-permissions` page for a
//! permission misconfig). It is omitted from the JSON entirely when
//! empty, so consumers parsing the v1 shape are unaffected.
//!
//! The `kind` mirrors the same dispatch [`crate::classify_exit_code`]
//! uses: it inspects the underlying `claude_wrapper::Error` variant
//! when present, otherwise falls back to `"other"`.
//!
//! ## Versioned ABI (v1)
//!
//! The top-level `version` field is the stability contract for
//! programmatic consumers. It is present on every `--json` output --
//! the error envelope and the success envelope alike. Peel off
//! `version` before inspecting anything inside.
//!
//! The success shape is `{ "version": 1, "result": {...} }`. Two
//! flavors share it:
//!
//! - The **ask** path ([`crate::run_ask`]) adds an `"refusal": <bool>`
//!   field alongside `result` -- the additive v1 flag that surfaces the
//!   refusal heuristic to non-TTY consumers.
//! - The **read-only management** commands -- `roba cost`, `roba
//!   history`, `roba doctor`, and `roba worktree list` -- emit the same
//!   `{ "version": 1, "result": {...} }` envelope *without* `refusal`
//!   (it's ask-specific). They wrap their payload through the crate's
//!   `VersionedResult` helper. `roba show` reuses the ask success
//!   envelope verbatim (its `result` is a reconstructed `QueryResult`).
//!
//! So the whole `--json` surface is `{ version, result, [refusal] }` on
//! success and `{ version, error }` on failure, uniformly.
//!
//! ## Exit 6 has no stderr error envelope (deliberate)
//!
//! The `{ version, error }` stderr envelope is emitted only on the
//! `Err`-path nonzero exits (codes 1-5, via [`render_json`]). Exit 6
//! ([`crate::EXIT_UNUSABLE_RESULT`]) never emits one -- the reliable
//! signal is the exit code itself. What is on stdout depends on the
//! subcase: for an empty / `is_error` `Ok`-path result the success-shaped
//! `{ version, result }` envelope is already written there; for a
//! streaming / `--trace` run that completed with no result event, exit 6
//! happens before the envelope is rendered, so stdout is empty. A `--json`
//! consumer branches on the exit code (and inspects `.result` /
//! `.is_error` on the stdout envelope when present); it does not scrape
//! stderr for an error envelope on code 6.
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

/// The `{ version, error }` failure envelope and its `{ kind, message,
/// exit_code, chain, see_also }` body live in the [`roba_types`] contract
/// crate (owned + `Deserialize` so a downstream harness shares the shape). The
/// binary re-exports and constructs them; `build_envelope` below is the one
/// producer.
pub use roba_types::{ErrorBody, ErrorEnvelope};

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

/// Canonical install URL for claude-code, pointed at by the
/// `NotFound` hint + `see_also`. Kept in one place so the plain-path
/// hint and the JSON `see_also` stay consistent.
const CLAUDE_CODE_URL: &str = "https://github.com/anthropics/claude-code";

/// Produce an actionable, human-facing hint for the two detectable
/// first-run failures: claude missing from PATH, and claude not
/// authenticated. Returns `None` for anything else (the primary error
/// text is enough). The hint is supplementary -- callers print it
/// *after* the underlying error, never instead of it.
///
/// `NotFound` is matched first so it wins over any auth classification.
pub fn hint_for_error(err: &anyhow::Error) -> Option<String> {
    let wrapper_err = err.downcast_ref::<claude_wrapper::Error>()?;
    if matches!(wrapper_err, claude_wrapper::Error::NotFound) {
        Some(format!(
            "claude binary not found on PATH. Install claude-code: {CLAUDE_CODE_URL}"
        ))
    } else if wrapper_err.auth_kind().is_some() {
        Some(
            "claude is not authenticated. Run `claude /login`, or set ANTHROPIC_API_KEY (and use --bare for API-key-only auth)."
                .to_string(),
        )
    } else {
        None
    }
}

/// Canonical doc URLs an error wants to point at, for the JSON
/// `see_also` field. Only `NotFound` has a canonical doc URL today;
/// the auth case has no doc page (the message-level hint covers it),
/// so it returns empty.
pub fn see_also_for(err: &anyhow::Error) -> Vec<String> {
    if let Some(claude_wrapper::Error::NotFound) = err.downcast_ref::<claude_wrapper::Error>() {
        vec![CLAUDE_CODE_URL.to_string()]
    } else {
        Vec::new()
    }
}

/// Human-facing render of the top-level error. Special-cases
/// `claude_wrapper::Error::CommandFailed` to LEAD with the cause
/// (claude's stderr, VERBATIM) + exit code + working dir, and DROP the
/// full `command` argv -- roba-constructed noise made huge by the
/// always-on #302 advisory. Never parses or rewords claude's stderr.
/// Every other error falls back to the anyhow chain (`{err:#}`).
pub fn render_human_error(err: &anyhow::Error) -> String {
    if let Some(claude_wrapper::Error::CommandFailed {
        exit_code,
        stdout,
        stderr,
        working_dir,
        ..
    }) = err.downcast_ref::<claude_wrapper::Error>()
    {
        let mut out = format!("claude failed (exit {exit_code})");
        if let Some(dir) = working_dir {
            out.push_str(&format!(" (in {})", dir.display()));
        }
        out.push_str(": ");
        // Lead with the cause, VERBATIM. Prefer stderr; fall back to
        // stdout; then to a placeholder. If both streams carry output
        // (rare), append stdout after the stderr cause, clearly labeled.
        if !stderr.is_empty() {
            out.push_str(stderr);
            if !stdout.is_empty() {
                out.push_str("\nstdout: ");
                out.push_str(stdout);
            }
        } else if !stdout.is_empty() {
            out.push_str(stdout);
        } else {
            out.push_str("(no output)");
        }
        out
    } else {
        format!("{err:#}")
    }
}

/// Build the envelope from an error + the already-computed exit code.
/// `message` is the top of the anyhow chain (the most recent context
/// call), matching what `{err}` would print without `:#`.
pub fn build_envelope(err: &anyhow::Error, exit_code: i32) -> ErrorEnvelope {
    let chain: Vec<String> = err.chain().map(|c| c.to_string()).collect();
    let message = chain.first().cloned().unwrap_or_else(|| err.to_string());
    ErrorEnvelope {
        version: roba_types::VERSION,
        error: ErrorBody {
            kind: kind_of(err).to_string(),
            message,
            exit_code,
            chain,
            // Populated only for errors with a canonical doc URL
            // (currently just NotFound); omitted-when-empty otherwise.
            see_also: see_also_for(err),
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
    fn empty_see_also_is_omitted_from_json() {
        // The default build_envelope path leaves see_also empty.
        let err = anyhow::anyhow!("plain error");
        let value = envelope_value(&err, 1);
        assert!(
            value["error"].get("see_also").is_none(),
            "empty see_also must not appear in the JSON"
        );
    }

    #[test]
    fn populated_see_also_appears_in_json() {
        let body = ErrorBody {
            kind: "other".to_string(),
            message: "with pointer".to_string(),
            exit_code: 1,
            chain: vec!["with pointer".to_string()],
            see_also: vec!["https://example.test/doc".to_string()],
        };
        let env = ErrorEnvelope {
            version: 1,
            error: body,
        };
        let json = serde_json::to_string(&env).expect("serializes");
        let value: Value = serde_json::from_str(&json).expect("round-trips");
        let see_also = value["error"]["see_also"]
            .as_array()
            .expect("see_also must be an array when populated");
        assert_eq!(see_also.len(), 1);
        assert_eq!(see_also[0], "https://example.test/doc");
    }

    #[test]
    fn hint_for_not_found_returns_install_hint() {
        let err = anyhow::Error::new(claude_wrapper::Error::NotFound);
        let hint = hint_for_error(&err).expect("NotFound must produce a hint");
        assert!(hint.contains("not found on PATH"), "hint was: {hint}");
        assert!(
            hint.contains("https://github.com/anthropics/claude-code"),
            "hint was: {hint}"
        );
    }

    #[test]
    fn hint_for_auth_returns_auth_hint() {
        let err = anyhow::Error::new(claude_wrapper::Error::Auth {
            kind: AuthErrorKind::NotAuthenticated,
            command: "claude -p hi".to_string(),
            exit_code: 1,
            message: "not logged in".to_string(),
        });
        let hint = hint_for_error(&err).expect("auth error must produce a hint");
        assert!(hint.contains("not authenticated"), "hint was: {hint}");
    }

    #[test]
    fn hint_for_non_wrapper_error_is_none() {
        let err = anyhow::anyhow!("boom");
        assert!(hint_for_error(&err).is_none());
    }

    #[test]
    fn not_found_populates_see_also_in_json() {
        let err = anyhow::Error::new(claude_wrapper::Error::NotFound);
        let value = envelope_value(&err, 1);
        let see_also = value["error"]["see_also"]
            .as_array()
            .expect("NotFound must populate see_also");
        assert_eq!(see_also.len(), 1);
        assert_eq!(see_also[0], "https://github.com/anthropics/claude-code");
    }

    #[test]
    fn render_human_command_failed_leads_with_cause_drops_argv() {
        let argv =
            "claude --print --append-system-prompt 'a very long #302 advisory ...' --worktree";
        let err = anyhow::Error::from(claude_wrapper::Error::CommandFailed {
            command: argv.to_string(),
            exit_code: 1,
            stdout: String::new(),
            stderr:
                "Error: Can only use --worktree in a git repository, but /x is not a git repository."
                    .to_string(),
            working_dir: Some("/x".into()),
        });
        let rendered = render_human_error(&err);
        assert!(
            rendered.starts_with("claude failed (exit 1)"),
            "got: {rendered}"
        );
        // The working dir is surfaced as a `(in DIR)` segment.
        assert!(
            rendered.contains("(in /x)"),
            "working dir must appear as a segment, got: {rendered}"
        );
        assert!(
            rendered.contains(
                "Error: Can only use --worktree in a git repository, but /x is not a git repository."
            ),
            "cause must appear verbatim, got: {rendered}"
        );
        assert!(
            !rendered.contains(argv),
            "the command argv must be dropped, got: {rendered}"
        );
    }

    /// Generalize the `CommandFailed` render across the branches the field
    /// hits: no working dir (no `(in ...)` segment), an empty-stderr
    /// fallback to stdout, and the both-empty `(no output)` placeholder.
    #[test]
    fn render_human_command_failed_variants() {
        // No working dir -> no `(in ...)` segment, exit code preserved.
        let no_dir = anyhow::Error::from(claude_wrapper::Error::CommandFailed {
            command: "claude --print".into(),
            exit_code: 2,
            stdout: String::new(),
            stderr: "boom from claude".into(),
            working_dir: None,
        });
        let r = render_human_error(&no_dir);
        assert_eq!(r, "claude failed (exit 2): boom from claude", "got: {r}");
        assert!(!r.contains("(in "), "no working dir means no segment: {r}");

        // Empty stderr -> lead with stdout instead.
        let stdout_only = anyhow::Error::from(claude_wrapper::Error::CommandFailed {
            command: "claude --print".into(),
            exit_code: 1,
            stdout: "only-on-stdout".into(),
            stderr: String::new(),
            working_dir: None,
        });
        let r = render_human_error(&stdout_only);
        assert_eq!(r, "claude failed (exit 1): only-on-stdout", "got: {r}");

        // Both empty -> the placeholder, never a bare trailing colon.
        let silent = anyhow::Error::from(claude_wrapper::Error::CommandFailed {
            command: "claude --print".into(),
            exit_code: 137,
            stdout: String::new(),
            stderr: String::new(),
            working_dir: None,
        });
        let r = render_human_error(&silent);
        assert_eq!(r, "claude failed (exit 137): (no output)", "got: {r}");
    }

    #[test]
    fn render_human_non_command_failed_falls_back_to_chain() {
        let err = anyhow::anyhow!("boom");
        assert_eq!(render_human_error(&err), "boom");
    }

    #[test]
    fn rendered_json_is_parseable() {
        let err = anyhow::anyhow!("anything");
        let rendered = render_json(&err, 1);
        let parsed: Value = serde_json::from_str(&rendered).expect("parseable");
        assert_eq!(parsed["error"]["kind"], "other");
    }
}
