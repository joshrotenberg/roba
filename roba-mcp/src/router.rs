//! Base MCP router and process-local client binding.

use serde::{Deserialize, Serialize};
use tower_mcp::schemars::{self, JsonSchema, schema_for};
use tower_mcp::{
    CallToolResult, ChannelTransport, Content, Error, McpClient, McpRouter, ReadResourceResult,
    ResourceBuilder, ToolBuilder,
};

use crate::{AgentInstance, AgentTurnResult};

/// Base tool for one finite turn through the logical agent.
pub const AGENT_TURN_TOOL: &str = "agent.turn";
/// Dynamic state resource for the logical agent.
pub const AGENT_RESOURCE_URI: &str = "roba://agent";

/// Input contract for [`AGENT_TURN_TOOL`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TurnInput {
    #[schemars(regex(pattern = r"\S"))]
    pub text: String,
}

/// Build the base single-agent MCP router.
pub fn router(agent: AgentInstance) -> McpRouter {
    let output_schema = serde_json::to_value(schema_for!(AgentTurnResult))
        .expect("static agent turn schema must serialize");
    let turn_agent = agent.clone();
    let turn = ToolBuilder::new(AGENT_TURN_TOOL)
        .description("Run one prompt through this logical Roba agent.")
        .output_schema(output_schema)
        .handler(move |input: TurnInput| {
            let agent = turn_agent.clone();
            async move {
                let result = agent.turn(input.text).await;
                encode_turn(&result)
            }
        })
        .build();

    let resource_agent = agent;
    let state = ResourceBuilder::new(AGENT_RESOURCE_URI)
        .name("Roba agent")
        .description("Current state of this logical Roba agent.")
        .mime_type("application/json")
        .handler(move || {
            let agent = resource_agent.clone();
            async move {
                let snapshot = agent.snapshot().await;
                let json = serde_json::to_string_pretty(&snapshot).map_err(|error| {
                    Error::internal(format!("failed to serialize agent snapshot: {error}"))
                })?;
                Ok(ReadResourceResult::text_with_mime(
                    AGENT_RESOURCE_URI,
                    json,
                    "application/json",
                ))
            }
        })
        .build();

    McpRouter::new()
        .server_info("roba-agent", env!("CARGO_PKG_VERSION"))
        .tool(turn)
        .resource(state)
}

/// Connect and initialize a production in-process MCP client.
pub async fn connect_in_process(agent: AgentInstance) -> tower_mcp::Result<McpClient> {
    let client = McpClient::connect(ChannelTransport::new(router(agent))).await?;
    client
        .initialize("roba-in-process", env!("CARGO_PKG_VERSION"))
        .await?;
    Ok(client)
}

fn encode_turn(value: &AgentTurnResult) -> tower_mcp::Result<CallToolResult> {
    let mut result = CallToolResult::from_serialize(value)?;
    result.content = vec![Content::text(value.display_text())];
    result.is_error = value.is_error();
    Ok(result)
}
