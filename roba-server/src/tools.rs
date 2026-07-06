//! The MCP tool surface, wired to the session actor.
//!
//! Current tools: `prompt` (send the next turn) and `status` (a read-only
//! snapshot). Genuinely-new tools (`interrupt`, an async `submit`/Tasks queue,
//! resources, progress) are built on top of this seam incrementally.

use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::mpsc;
use tower_mcp::extract::{Context, Json, State};
use tower_mcp::{CallToolResult, McpRouter, NoParams, Tool, ToolBuilder};

use crate::bridge::{ElicitBridge, ElicitRequest, ask_via_context};
use crate::session::SessionHandle;

#[derive(Debug, Deserialize, JsonSchema)]
struct PromptInput {
    /// The next turn to send to the warm claude session.
    prompt: String,
}

/// State shared into the `prompt` handler: the session handle, the operator
/// bridge (to service the agent's `ask_operator` during the turn), and the
/// output mode.
#[derive(Clone)]
struct PromptState {
    handle: SessionHandle,
    bridge: ElicitBridge,
    structured: bool,
}

/// Build the MCP router: server info + the tools. `bridge` lets the in-flight
/// `prompt` turn service the running agent's `ask_operator` requests.
pub fn router(handle: SessionHandle, structured: bool, bridge: ElicitBridge) -> McpRouter {
    let state = PromptState {
        handle: handle.clone(),
        bridge,
        structured,
    };
    McpRouter::new()
        .server_info("roba-server", env!("CARGO_PKG_VERSION"))
        .tool(prompt_tool(state))
        .tool(status_tool(handle))
}

fn prompt_tool(state: PromptState) -> Tool {
    ToolBuilder::new("prompt")
        .title("Prompt")
        .description(
            "Send the next turn to this process's single warm claude session. \
             Concurrent calls queue FIFO and run one at a time.",
        )
        .extractor_handler(
            state,
            |State(s): State<PromptState>,
             ctx: Context,
             Json(input): Json<PromptInput>| async move {
                let PromptState {
                    handle,
                    bridge,
                    structured,
                } = s;
                // Register a per-turn channel so the agent's `ask_operator`
                // requests reach this handler (which holds an elicitation-capable
                // Context) while the turn runs.
                let (tx, mut rx) = mpsc::channel::<ElicitRequest>(8);
                *bridge.lock().expect("bridge mutex poisoned") = Some(tx);

                let turn = handle.prompt(input.prompt);
                tokio::pin!(turn);
                let outcome = loop {
                    tokio::select! {
                        result = &mut turn => break result,
                        Some(req) = rx.recv() => {
                            let answer = ask_via_context(&ctx, &req.question).await;
                            let _ = req.reply.send(answer);
                        }
                    }
                };
                *bridge.lock().expect("bridge mutex poisoned") = None;

                match outcome {
                    Err(e) => Ok(CallToolResult::error(e.to_string())),
                    Ok(out) if out.is_error => Ok(CallToolResult::error(out.text)),
                    Ok(out) if structured => match out.structured {
                        Some(value) => Ok(CallToolResult::json(value)),
                        // Schema set but the model returned prose: surface it plainly.
                        None => Ok(CallToolResult::text(out.text)),
                    },
                    Ok(out) => Ok(CallToolResult::text(out.text)),
                }
            },
        )
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
