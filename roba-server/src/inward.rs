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
use tempfile::NamedTempFile;
use tower_mcp::{CallToolResult, HttpTransport, McpRouter, NoParams, Tool, ToolBuilder};

use crate::config::ServerConfig;
use crate::session::SessionStatus;

/// What the inward tools read: the static launch config plus the live session
/// status shared with the actor.
#[derive(Clone)]
pub struct InwardContext {
    pub config: ServerConfig,
    pub status: Arc<Mutex<SessionStatus>>,
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
    McpRouter::new()
        .server_info("roba", env!("CARGO_PKG_VERSION"))
        .tool(context_tool(ctx))
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
