//! The operator bridge: the running agent (south) asks the operator (north).
//!
//! The south `ask_operator` tool routes a question to the in-flight north
//! `prompt` handler, which raises an MCP `elicitation/create` to the operator
//! and returns the answer. This works because the agent only runs *during* a
//! north `prompt` call, so an elicitation-capable `Context` is available; the
//! prompt handler services these requests in a `select!` loop alongside the
//! turn. One session runs one turn at a time, so at most one channel is active.

use std::sync::{Arc, Mutex};

use tokio::sync::{mpsc, oneshot};
use tower_mcp::extract::Context;
use tower_mcp::{ElicitAction, ElicitFormParams, ElicitFormSchema, ElicitMode};

/// A question routed from `ask_operator` to the in-flight turn, with a channel
/// for the operator's answer.
pub struct ElicitRequest {
    pub question: String,
    pub reply: oneshot::Sender<ElicitOutcome>,
}

/// The operator's response to an `ask_operator`.
#[derive(Debug, Clone)]
pub enum ElicitOutcome {
    Answer(String),
    Declined,
    Cancelled,
    Unavailable,
}

/// Registration slot for the current turn's elicitation channel: the north
/// `prompt` handler sets it (Some) for the turn's duration, the south
/// `ask_operator` tool reads it.
pub type ElicitBridge = Arc<Mutex<Option<mpsc::Sender<ElicitRequest>>>>;

/// A fresh, empty bridge.
pub fn new_bridge() -> ElicitBridge {
    Arc::new(Mutex::new(None))
}

/// Raise an MCP elicitation to the operator for `question` and map the result.
pub async fn ask_via_context(ctx: &Context, question: &str) -> ElicitOutcome {
    if !ctx.can_elicit() {
        return ElicitOutcome::Unavailable;
    }
    let params = ElicitFormParams {
        mode: Some(ElicitMode::Form),
        message: question.to_string(),
        requested_schema: ElicitFormSchema::new().string_field(
            "answer",
            Some("Your answer to the agent"),
            true,
        ),
        meta: None,
    };
    match ctx.elicit_form(params).await {
        Ok(result) => match result.action {
            ElicitAction::Accept => {
                let content = serde_json::to_value(&result.content).unwrap_or_default();
                let answer = content
                    .get("answer")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                ElicitOutcome::Answer(answer)
            }
            ElicitAction::Decline => ElicitOutcome::Declined,
            ElicitAction::Cancel => ElicitOutcome::Cancelled,
            // ElicitAction is non-exhaustive; treat anything else as no answer.
            _ => ElicitOutcome::Cancelled,
        },
        Err(_) => ElicitOutcome::Unavailable,
    }
}
