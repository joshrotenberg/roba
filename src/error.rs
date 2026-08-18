//! Versioned JSON and byte-clean human errors for the command boundary.

pub use roba_types::{ErrorBody, ErrorEnvelope};

/// Classify an error for the versioned machine envelope.
pub fn kind_of(error: &anyhow::Error) -> &'static str {
    let Some(run_error) = error.downcast_ref::<crate::bounded::BoundedRunError>() else {
        return "other";
    };
    match run_error.failure().kind {
        roba_core::FailureKind::Authentication => "auth",
        roba_core::FailureKind::Timeout => "timeout",
        roba_core::FailureKind::Budget | roba_core::FailureKind::MaxCost => "budget",
        roba_core::FailureKind::MaxTurns | roba_core::FailureKind::Limit => "limit",
        roba_core::FailureKind::Cancelled
        | roba_core::FailureKind::Unsupported
        | roba_core::FailureKind::Provider => "other",
    }
}

/// Render the complete anyhow chain for a human-facing error.
pub fn render_human_error(error: &anyhow::Error) -> String {
    format!("{error:#}")
}

/// Build the stable `{ version, error }` envelope.
pub fn build_envelope(error: &anyhow::Error, exit_code: i32) -> ErrorEnvelope {
    let chain: Vec<String> = error.chain().map(ToString::to_string).collect();
    let message = chain.first().cloned().unwrap_or_else(|| error.to_string());
    ErrorEnvelope {
        version: roba_types::VERSION,
        error: ErrorBody {
            kind: kind_of(error).to_string(),
            message,
            exit_code,
            chain,
            see_also: Vec::new(),
        },
    }
}

/// Pretty-print the machine envelope for stderr.
pub fn render_json(error: &anyhow::Error, exit_code: i32) -> String {
    serde_json::to_string_pretty(&build_envelope(error, exit_code)).unwrap_or_else(|_| {
        format!(
            "{{\"version\":1,\"error\":{{\"kind\":\"other\",\"message\":\"serialization failed\",\"exit_code\":{exit_code},\"chain\":[]}}}}"
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use roba_core::{FailureKind, RunFailure, RunFailureDetails};

    fn run_error(kind: FailureKind) -> anyhow::Error {
        anyhow::Error::new(crate::bounded::BoundedRunError::new(RunFailure {
            kind,
            message: "provider failed".to_string(),
            details: RunFailureDetails::default(),
        }))
    }

    #[test]
    fn typed_failures_keep_machine_kinds() {
        assert_eq!(kind_of(&run_error(FailureKind::Authentication)), "auth");
        assert_eq!(kind_of(&run_error(FailureKind::Timeout)), "timeout");
        assert_eq!(kind_of(&run_error(FailureKind::MaxCost)), "budget");
        assert_eq!(kind_of(&run_error(FailureKind::MaxTurns)), "limit");
        assert_eq!(kind_of(&run_error(FailureKind::Provider)), "other");
    }

    #[test]
    fn envelope_preserves_chain_order_and_omits_empty_pointers() {
        let error = anyhow::anyhow!("root").context("outer");
        let value: serde_json::Value =
            serde_json::from_str(&render_json(&error, 1)).expect("valid error envelope");
        assert_eq!(value["version"], 1);
        assert_eq!(value["error"]["message"], "outer");
        assert_eq!(value["error"]["chain"][0], "outer");
        assert_eq!(value["error"]["chain"][1], "root");
        assert!(value["error"].get("see_also").is_none());
    }
}
