//! Run-scoped MCP adapter over [`roba_core::RunHandle`].
//!
//! The adapter owns no execution state. Its tools call the same handle a Rust
//! host or REPL uses, so transport choice cannot change lifecycle semantics.

use schemars::JsonSchema;
use serde::Deserialize;
use tower_mcp::transport::GenericStdioTransport;
use tower_mcp::{CallToolResult, McpRouter, ToolBuilder};

use roba_core::{Prompt, RunHandle};

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PromptArgs {
    /// Message for the root Roba agent.
    text: String,
}

/// Build the minimal control and observation surface for one run.
pub fn router(handle: RunHandle) -> McpRouter {
    let start = {
        let handle = handle.clone();
        ToolBuilder::new("start")
            .description("Supply the first prompt to a suspended Roba run.")
            .non_destructive()
            .handler(move |args: PromptArgs| {
                let handle = handle.clone();
                async move {
                    let prompt = match Prompt::new(args.text) {
                        Ok(prompt) => prompt,
                        Err(error) => return Ok(CallToolResult::error(error.to_string())),
                    };
                    match handle.start(prompt).await {
                        Ok(()) => snapshot_result(&handle).await,
                        Err(error) => Ok(CallToolResult::error(error.to_string())),
                    }
                }
            })
            .build()
    };

    let status = {
        let handle = handle.clone();
        ToolBuilder::new("status")
            .description("Inspect the current Roba run state and latest outcome.")
            .read_only()
            .no_params_handler(move || {
                let handle = handle.clone();
                async move { snapshot_result(&handle).await }
            })
            .build()
    };

    let wait = {
        let handle = handle.clone();
        ToolBuilder::new("wait")
            .description("Wait for this Roba run to complete, fail, or be cancelled.")
            .read_only()
            .no_params_handler(move || {
                let handle = handle.clone();
                async move { json_result(handle.wait().await) }
            })
            .build()
    };

    let steer = {
        let handle = handle.clone();
        ToolBuilder::new("steer")
            .description("Queue guidance for the root agent's next safe provider turn boundary.")
            .non_destructive()
            .handler(move |args: PromptArgs| {
                let handle = handle.clone();
                async move {
                    let prompt = match Prompt::new(args.text) {
                        Ok(prompt) => prompt,
                        Err(error) => return Ok(CallToolResult::error(error.to_string())),
                    };
                    match handle.steer(prompt).await {
                        Ok(()) => snapshot_result(&handle).await,
                        Err(error) => Ok(CallToolResult::error(error.to_string())),
                    }
                }
            })
            .build()
    };

    let cancel = ToolBuilder::new("cancel")
        .description("Cancel the active Roba provider turn and end this run.")
        .destructive()
        .no_params_handler(move || {
            let handle = handle.clone();
            async move {
                match handle.cancel().await {
                    Ok(()) => snapshot_result(&handle).await,
                    Err(error) => Ok(CallToolResult::error(error.to_string())),
                }
            }
        })
        .build();

    McpRouter::new()
        .server_info("roba-run", env!("CARGO_PKG_VERSION"))
        .instructions(
            "This MCP server controls one bounded Roba run. Start supplies the first prompt; status and wait observe it; steer queues guidance for the next turn boundary; cancel ends it.",
        )
        .tool(start)
        .tool(status)
        .tool(wait)
        .tool(steer)
        .tool(cancel)
}

/// Serve the run router over stdin/stdout until the client closes the stream.
/// The host remains responsible for the owning process lifetime.
pub async fn serve_stdio(handle: RunHandle) -> tower_mcp::Result<()> {
    GenericStdioTransport::new(router(handle)).run().await
}

async fn snapshot_result(handle: &RunHandle) -> tower_mcp::Result<CallToolResult> {
    json_result(handle.status().await)
}

fn json_result<T: serde::Serialize>(value: T) -> tower_mcp::Result<CallToolResult> {
    Ok(match serde_json::to_value(value) {
        Ok(value) => CallToolResult::json(value),
        Err(error) => CallToolResult::error(format!("failed to serialize run state: {error}")),
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::json;
    use tower_mcp::TestClient;

    use super::*;
    use roba_core::{
        AgentSpec, EventSink, Provider, ProviderCapabilities, ProviderError, ProviderFuture,
        ProviderId, Run, RunOutcome, RunSpec, SessionHandle, TurnRequest,
    };

    struct FakeProvider;

    impl Provider for FakeProvider {
        fn id(&self) -> ProviderId {
            ProviderId::new("fake").unwrap()
        }

        fn capabilities(&self) -> ProviderCapabilities {
            ProviderCapabilities {
                resume: true,
                ..ProviderCapabilities::default()
            }
        }

        fn validate(&self, _request: &TurnRequest) -> Result<(), ProviderError> {
            Ok(())
        }

        fn execute<'a>(
            &'a self,
            request: TurnRequest,
            _events: &'a dyn EventSink,
        ) -> ProviderFuture<'a> {
            Box::pin(async move {
                Ok(RunOutcome {
                    output: request.prompt.into_inner(),
                    session: Some(SessionHandle {
                        provider: ProviderId::new("fake").unwrap(),
                        id: "session-1".to_string(),
                    }),
                    usage: None,
                    cost: None,
                    duration_ms: None,
                    provider_turns: None,
                    structured_output: None,
                })
            })
        }
    }

    #[tokio::test]
    async fn mcp_starts_and_observes_the_same_suspended_run() {
        let run = Run::new(
            RunSpec::suspended(AgentSpec::new(ProviderId::new("fake").unwrap())),
            Arc::new(FakeProvider),
        )
        .unwrap();
        let mut client = TestClient::from_router(router(run.handle()));
        client.initialize().await;

        let before = client.call_tool_json("status", json!({})).await;
        assert_eq!(before["state"], "suspended");

        let started = client
            .call_tool_json("start", json!({"text": "hello"}))
            .await;
        assert!(matches!(
            started["state"].as_str(),
            Some("running" | "completed")
        ));

        let terminal = client.call_tool_json("wait", json!({})).await;
        assert_eq!(terminal["state"], "completed");
        assert_eq!(terminal["last_outcome"]["output"], "hello");
    }

    #[tokio::test]
    async fn mcp_refuses_a_second_start() {
        let run = Run::new(
            RunSpec::suspended(AgentSpec::new(ProviderId::new("fake").unwrap())),
            Arc::new(FakeProvider),
        )
        .unwrap();
        let mut client = TestClient::from_router(router(run.handle()));
        client.initialize().await;
        client.call_tool("start", json!({"text": "hello"})).await;
        let error = client
            .call_tool_expect_error("start", json!({"text": "again"}))
            .await;
        assert!(error.to_string().contains("already started"));
    }
}
