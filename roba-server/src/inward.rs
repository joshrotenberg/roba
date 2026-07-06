//! The inward (south) MCP surface.
//!
//! roba-server exposes a *second* MCP face, this one to its own running claude
//! session. The child is spawned with a generated `--mcp-config` pointing at an
//! in-process HTTP MCP server on an ephemeral localhost port; the running agent
//! then gets reflexive tools to introspect its own roba-server execution.
//!
//! Today that is one read-only tool, `context` (model, mode, session id, turns,
//! spend, remaining budget) -- self-awareness so the agent can pace itself. The
//! operator bridge (`ask_operator` via elicitation) builds on this same surface.
//!
//! The inward server shares the live [`SessionStatus`] with the actor (which
//! writes it), so `context` reports current figures.

use std::io::Write as _;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use schemars::JsonSchema;
use serde::Deserialize;
use tempfile::NamedTempFile;
use tokio::sync::oneshot;
use tower_mcp::{CallToolResult, HttpTransport, McpRouter, NoParams, Tool, ToolBuilder};

use crate::bridge::{ElicitBridge, ElicitOutcome, ElicitRequest};
use crate::config::ServerConfig;
use crate::session::SessionStatus;

/// What the inward tools read: the static launch config, the live session
/// status shared with the actor, and the operator bridge (for `ask_operator`).
#[derive(Clone)]
pub struct InwardContext {
    pub config: ServerConfig,
    pub status: Arc<Mutex<SessionStatus>>,
    pub bridge: ElicitBridge,
}

/// Bind an ephemeral localhost port, serve the inward MCP router in the
/// background, and return the base URL the child connects to.
pub async fn spawn_server(ctx: InwardContext) -> Result<String> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let url = format!("http://{}/", listener.local_addr()?);
    let router = HttpTransport::new(inward_router(ctx)).into_router();
    tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, router).await {
            tracing::error!(error = %e, "inward MCP server stopped");
        }
    });
    tracing::info!(url = %url, "inward (south) MCP surface up");
    Ok(url)
}

/// Write an `--mcp-config` file pointing the child at `inward_url`. The returned
/// handle must be kept alive for the session's lifetime (drop deletes the file).
pub fn write_mcp_config(inward_url: &str) -> Result<NamedTempFile> {
    let cfg = serde_json::json!({
        "mcpServers": { "roba": { "type": "http", "url": inward_url } }
    });
    let mut file = NamedTempFile::with_suffix(".json")?;
    file.write_all(serde_json::to_string_pretty(&cfg)?.as_bytes())?;
    file.flush()?;
    Ok(file)
}

/// The inward router. Server name `roba` => the child sees `mcp__roba__*` tools.
fn inward_router(ctx: InwardContext) -> McpRouter {
    let bridge = ctx.bridge.clone();
    McpRouter::new()
        .server_info("roba", env!("CARGO_PKG_VERSION"))
        .tool(context_tool(ctx))
        .tool(ask_operator_tool(bridge))
}

#[derive(Debug, Deserialize, JsonSchema)]
struct AskOperatorInput {
    /// The question to put to the human operator.
    question: String,
}

/// `ask_operator`: the agent asks the human operator a question mid-turn and
/// gets their answer. Routes through the bridge to the in-flight north `prompt`
/// handler, which raises the elicitation. Returns `{action, answer?}`.
fn ask_operator_tool(bridge: ElicitBridge) -> Tool {
    ToolBuilder::new("ask_operator")
        .title("Ask the operator")
        .description(
            "Ask the human operator a question and get their answer. Use when you \
             need a decision or clarification only they can give (which branch to \
             target, whether to proceed, a missing value).",
        )
        .handler(move |input: AskOperatorInput| {
            let bridge = bridge.clone();
            async move {
                let sender = bridge.lock().expect("bridge mutex poisoned").clone();
                let Some(tx) = sender else {
                    return Ok(CallToolResult::error(
                        "no operator is attached to this session right now",
                    ));
                };
                let (reply, rx) = oneshot::channel();
                if tx
                    .send(ElicitRequest {
                        question: input.question,
                        reply,
                    })
                    .await
                    .is_err()
                {
                    return Ok(CallToolResult::error("operator channel closed"));
                }
                let value = match rx.await {
                    Ok(ElicitOutcome::Answer(answer)) => {
                        serde_json::json!({"action": "accept", "answer": answer})
                    }
                    Ok(ElicitOutcome::Declined) => serde_json::json!({"action": "decline"}),
                    Ok(ElicitOutcome::Cancelled) => serde_json::json!({"action": "cancel"}),
                    Ok(ElicitOutcome::Unavailable) | Err(_) => {
                        return Ok(CallToolResult::error("operator unavailable"));
                    }
                };
                Ok(CallToolResult::json(value))
            }
        })
        .build()
}

fn context_tool(ctx: InwardContext) -> Tool {
    ToolBuilder::new("context")
        .title("roba context")
        .description(
            "Report your roba-server execution context: model, mode, session id, \
             turns used, spend so far, and remaining budget. Call this to learn how \
             you were run and to pace yourself against the budget.",
        )
        .handler(move |_: NoParams| {
            let ctx = ctx.clone();
            async move {
                let status = ctx.status.lock().expect("status mutex poisoned").clone();
                let remaining = ctx
                    .config
                    .max_usd
                    .map(|max| (max - status.cumulative_cost_usd).max(0.0));
                let value = serde_json::json!({
                    "model": ctx.config.model,
                    "structured": ctx.config.structured(),
                    "session_id": status.session_id,
                    "turns_completed": status.turns_completed,
                    "cumulative_cost_usd": status.cumulative_cost_usd,
                    "budget_max_usd": ctx.config.max_usd,
                    "budget_remaining_usd": remaining,
                });
                Ok(CallToolResult::json(value))
            }
        })
        .build()
}
