//! The MCP tool surface, wired to the session actor.
//!
//! Current tools: `prompt` (send the next turn) and `status` (a read-only
//! snapshot). Genuinely-new tools (`interrupt`, an async `submit`/Tasks queue,
//! resources, progress) are built on top of this seam incrementally.

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;
use tower_mcp::{CallToolResult, McpRouter, NoParams, Tool, ToolBuilder};

use crate::session::SessionHandle;

#[derive(Debug, Deserialize, JsonSchema)]
struct PromptInput {
    /// The next turn to send to the warm claude session.
    prompt: String,
}

/// Build the MCP router: server info + the tools, all sharing one session
/// handle. `structured` selects whether `prompt` returns `structuredContent`
/// (the session was launched with a schema) or plain text.
pub fn router(handle: SessionHandle, structured: bool) -> McpRouter {
    McpRouter::new()
        .server_info("roba-server", env!("CARGO_PKG_VERSION"))
        .tool(prompt_tool(handle.clone(), structured))
        .tool(status_tool(handle))
}

fn prompt_tool(handle: SessionHandle, structured: bool) -> Tool {
    ToolBuilder::new("prompt")
        .title("Prompt")
        .description(
            "Send the next turn to this process's single warm claude session. \
             Concurrent calls queue FIFO and run one at a time.",
        )
        .handler(move |input: PromptInput| {
            let handle = handle.clone();
            async move {
                match handle.prompt(input.prompt).await {
                    Err(e) => Ok(CallToolResult::error(e.to_string())),
                    Ok(out) if out.is_error => Ok(CallToolResult::error(out.text)),
                    Ok(out) if structured => match out.structured {
                        Some(value) => Ok(CallToolResult::json(value)),
                        // Schema set but the model returned prose: surface it plainly.
                        None => Ok(CallToolResult::text(out.text)),
                    },
                    Ok(out) => Ok(CallToolResult::text(out.text)),
                }
            }
        })
        .build()
}

fn status_tool(handle: SessionHandle) -> Tool {
    ToolBuilder::new("status")
        .title("Status")
        .description("Read-only snapshot of the session: id, turns completed, cumulative cost.")
        .handler(move |_input: NoParams| {
            let handle = handle.clone();
            async move {
                let snapshot = handle.status();
                let value = serde_json::to_value(snapshot).unwrap_or(Value::Null);
                Ok(CallToolResult::json(value))
            }
        })
        .build()
}
