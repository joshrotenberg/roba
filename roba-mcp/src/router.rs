//! Base MCP router and process-local client binding.

use std::collections::HashMap;
use std::error::Error as StdError;
use std::fmt;

use serde::{Deserialize, Serialize};
use tower_mcp::schemars::{self, JsonSchema, schema_for};
use tower_mcp::{
    CallToolResult, ChannelTransport, Content, Error as McpError, McpClient, McpRouter,
    ReadResourceResult, ResourceBuilder, ResourceTemplateBuilder, ToolBuilder,
};

use crate::{
    AGENT_EVENT_CAPACITY, AgentInstance, AgentInterruptResult, AgentShutdownResult,
    AgentSteerResult, AgentTurnResult, OperationId,
};

/// Base tool for one finite turn through the logical agent.
pub const AGENT_TURN_TOOL: &str = "agent.turn";
/// Control tool for queueing guidance on one active operation.
pub const AGENT_STEER_TOOL: &str = "agent.steer";
/// Control tool for cancelling one active operation and awaiting settlement.
pub const AGENT_INTERRUPT_TOOL: &str = "agent.interrupt";
/// Control tool for permanently stopping the logical agent.
pub const AGENT_SHUTDOWN_TOOL: &str = "agent.shutdown";
/// Dynamic state resource for the logical agent.
pub const AGENT_RESOURCE_URI: &str = "roba://agent";
/// Default agent-wide event resource.
pub const AGENT_EVENTS_URI: &str = "roba://events";
/// Cursor-paged agent-wide event resource template.
pub const AGENT_EVENTS_TEMPLATE: &str = "roba://events{?after,limit}";

const DEFAULT_EVENT_LIMIT: usize = 100;

/// Input contract for [`AGENT_TURN_TOOL`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TurnInput {
    #[schemars(regex(pattern = r"\S"))]
    pub text: String,
}

/// Input contract for [`AGENT_STEER_TOOL`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SteerInput {
    pub operation_id: OperationId,
    #[schemars(regex(pattern = r"\S"))]
    pub text: String,
}

/// Input contract for [`AGENT_INTERRUPT_TOOL`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InterruptInput {
    pub operation_id: OperationId,
}

/// Input contract for [`AGENT_SHUTDOWN_TOOL`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShutdownInput {}

/// Failure to call or decode the typed [`AGENT_TURN_TOOL`] contract.
#[derive(Debug)]
pub enum AgentClientError {
    /// The input could not be encoded for the MCP call.
    EncodeInput(serde_json::Error),
    /// The MCP request failed before returning a tool result.
    Call(McpError),
    /// The tool result omitted its machine-readable result.
    MissingStructuredContent,
    /// The machine-readable result did not match [`AgentTurnResult`].
    DecodeStructuredContent(serde_json::Error),
    /// MCP's `isError` flag contradicted the typed application status.
    StatusMismatch {
        /// The flag carried by the MCP result.
        mcp_is_error: bool,
        /// The flag implied by the decoded [`AgentTurnResult`] variant.
        typed_is_error: bool,
    },
}

impl fmt::Display for AgentClientError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EncodeInput(error) => {
                write!(formatter, "failed to encode agent.turn input: {error}")
            }
            Self::Call(error) => write!(formatter, "agent.turn MCP call failed: {error}"),
            Self::MissingStructuredContent => {
                formatter.write_str("agent.turn returned no structuredContent")
            }
            Self::DecodeStructuredContent(error) => {
                write!(
                    formatter,
                    "agent.turn returned malformed structuredContent: {error}"
                )
            }
            Self::StatusMismatch {
                mcp_is_error,
                typed_is_error,
            } => write!(
                formatter,
                "agent.turn status mismatch: MCP isError={mcp_is_error}, typed result requires isError={typed_is_error}"
            ),
        }
    }
}

impl StdError for AgentClientError {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        match self {
            Self::EncodeInput(error) | Self::DecodeStructuredContent(error) => Some(error),
            Self::Call(error) => Some(error),
            Self::MissingStructuredContent | Self::StatusMismatch { .. } => None,
        }
    }
}

/// Build the base single-agent MCP router.
pub fn router(agent: AgentInstance) -> McpRouter {
    let turn_output_schema = serde_json::to_value(schema_for!(AgentTurnResult))
        .expect("static agent turn schema must serialize");
    let turn_agent = agent.clone();
    let turn = ToolBuilder::new(AGENT_TURN_TOOL)
        .description("Run one prompt through this logical Roba agent.")
        .output_schema(turn_output_schema)
        .handler(move |input: TurnInput| {
            let agent = turn_agent.clone();
            async move {
                let result = agent.turn(input.text).await;
                encode_turn(&result)
            }
        })
        .build();

    let steer_output_schema = serde_json::to_value(schema_for!(AgentSteerResult))
        .expect("static agent steer schema must serialize");
    let steer_agent = agent.clone();
    let steer = ToolBuilder::new(AGENT_STEER_TOOL)
        .description("Queue guidance for one exact active Roba operation.")
        .output_schema(steer_output_schema)
        .handler(move |input: SteerInput| {
            let agent = steer_agent.clone();
            async move {
                let result = agent.steer(input.operation_id, input.text).await;
                encode_steer(&result)
            }
        })
        .build();

    let interrupt_output_schema = serde_json::to_value(schema_for!(AgentInterruptResult))
        .expect("static agent interrupt schema must serialize");
    let interrupt_agent = agent.clone();
    let interrupt = ToolBuilder::new(AGENT_INTERRUPT_TOOL)
        .description("Cancel one exact active Roba operation and await settlement.")
        .output_schema(interrupt_output_schema)
        .handler(move |input: InterruptInput| {
            let agent = interrupt_agent.clone();
            async move {
                let result = agent.interrupt(input.operation_id).await;
                encode_interrupt(&result)
            }
        })
        .build();

    let shutdown_output_schema = serde_json::to_value(schema_for!(AgentShutdownResult))
        .expect("static agent shutdown schema must serialize");
    let shutdown_agent = agent.clone();
    let shutdown = ToolBuilder::new(AGENT_SHUTDOWN_TOOL)
        .description("Permanently stop this Roba agent after draining active work.")
        .output_schema(shutdown_output_schema)
        .handler(move |_input: ShutdownInput| {
            let agent = shutdown_agent.clone();
            async move {
                let result = agent.shutdown().await;
                encode_shutdown(&result)
            }
        })
        .build();

    let resource_agent = agent.clone();
    let state = ResourceBuilder::new(AGENT_RESOURCE_URI)
        .name("Roba agent")
        .description("Current state of this logical Roba agent.")
        .mime_type("application/json")
        .handler(move || {
            let agent = resource_agent.clone();
            async move {
                let snapshot = agent.snapshot().await;
                let json = serde_json::to_string_pretty(&snapshot).map_err(|error| {
                    McpError::internal(format!("failed to serialize agent snapshot: {error}"))
                })?;
                Ok(ReadResourceResult::text_with_mime(
                    AGENT_RESOURCE_URI,
                    json,
                    "application/json",
                ))
            }
        })
        .build();

    let default_events_agent = agent.clone();
    let events = ResourceBuilder::new(AGENT_EVENTS_URI)
        .name("Roba agent events")
        .description("Recent globally sequenced events for this logical Roba agent.")
        .mime_type("application/json")
        .handler(move || {
            let agent = default_events_agent.clone();
            async move {
                read_events_resource(
                    agent,
                    AGENT_EVENTS_URI.to_owned(),
                    0,
                    DEFAULT_EVENT_LIMIT.min(AGENT_EVENT_CAPACITY),
                )
                .await
            }
        })
        .build();

    let template_events_agent = agent;
    let events_template = ResourceTemplateBuilder::new(AGENT_EVENTS_TEMPLATE)
        .name("Roba agent event page")
        .description("Read agent-wide events after an optional sequence cursor.")
        .mime_type("application/json")
        .argument(
            "after",
            Some("Return records after this agent-wide sequence."),
            false,
        )
        .argument("limit", Some("Maximum records to return."), false)
        .handler(move |uri: String, variables: HashMap<String, String>| {
            let agent = template_events_agent.clone();
            async move {
                let (after, limit) = parse_event_query(&variables)?;
                read_events_resource(agent, uri, after, limit).await
            }
        });

    McpRouter::new()
        .server_info("roba-agent", env!("CARGO_PKG_VERSION"))
        .tool(turn)
        .tool(steer)
        .tool(interrupt)
        .tool(shutdown)
        .resource(state)
        .resource(events)
        .resource_template(events_template)
}

/// Connect and initialize a production in-process MCP client.
pub async fn connect_in_process(agent: AgentInstance) -> tower_mcp::Result<McpClient> {
    let client = McpClient::connect(ChannelTransport::new(router(agent))).await?;
    client
        .initialize("roba-in-process", env!("CARGO_PKG_VERSION"))
        .await?;
    Ok(client)
}

/// Call [`AGENT_TURN_TOOL`] and decode its authoritative typed result.
///
/// Display content is intentionally ignored. Missing or malformed
/// `structuredContent`, and any disagreement between MCP's `isError` flag and
/// the decoded application status, fail closed.
pub async fn call_turn(
    client: &McpClient,
    input: TurnInput,
) -> Result<AgentTurnResult, AgentClientError> {
    let arguments = serde_json::to_value(input).map_err(AgentClientError::EncodeInput)?;
    let result = client
        .call_tool(AGENT_TURN_TOOL, arguments)
        .await
        .map_err(AgentClientError::Call)?;
    decode_turn(result)
}

fn decode_turn(result: CallToolResult) -> Result<AgentTurnResult, AgentClientError> {
    let structured = result
        .structured_content
        .ok_or(AgentClientError::MissingStructuredContent)?;
    let typed: AgentTurnResult =
        serde_json::from_value(structured).map_err(AgentClientError::DecodeStructuredContent)?;
    let typed_is_error = typed.is_error();
    if result.is_error != typed_is_error {
        return Err(AgentClientError::StatusMismatch {
            mcp_is_error: result.is_error,
            typed_is_error,
        });
    }
    Ok(typed)
}

fn encode_turn(value: &AgentTurnResult) -> tower_mcp::Result<CallToolResult> {
    let mut result = CallToolResult::from_serialize(value)?;
    result.content = vec![Content::text(value.display_text())];
    result.is_error = value.is_error();
    Ok(result)
}

fn encode_steer(value: &AgentSteerResult) -> tower_mcp::Result<CallToolResult> {
    let mut result = CallToolResult::from_serialize(value)?;
    result.content = vec![Content::text(value.display_text())];
    result.is_error = value.is_error();
    Ok(result)
}

fn encode_interrupt(value: &AgentInterruptResult) -> tower_mcp::Result<CallToolResult> {
    let mut result = CallToolResult::from_serialize(value)?;
    result.content = vec![Content::text(value.display_text())];
    result.is_error = value.is_error();
    Ok(result)
}

fn encode_shutdown(value: &AgentShutdownResult) -> tower_mcp::Result<CallToolResult> {
    let mut result = CallToolResult::from_serialize(value)?;
    result.content = vec![Content::text(value.display_text())];
    result.is_error = false;
    Ok(result)
}

fn parse_event_query(variables: &HashMap<String, String>) -> tower_mcp::Result<(u64, usize)> {
    let after = parse_query_value::<u64>(variables, "after")?.unwrap_or(0);
    let limit = parse_query_value::<usize>(variables, "limit")?
        .unwrap_or_else(|| DEFAULT_EVENT_LIMIT.min(AGENT_EVENT_CAPACITY));
    Ok((after, limit))
}

fn parse_query_value<T>(
    variables: &HashMap<String, String>,
    name: &str,
) -> tower_mcp::Result<Option<T>>
where
    T: std::str::FromStr,
{
    variables
        .get(name)
        .map(|value| {
            value.parse().map_err(|_| {
                McpError::invalid_params(format!(
                    "event resource query parameter {name} must be an unsigned integer"
                ))
            })
        })
        .transpose()
}

async fn read_events_resource(
    agent: AgentInstance,
    uri: String,
    after: u64,
    limit: usize,
) -> tower_mcp::Result<ReadResourceResult> {
    let page = agent
        .event_page(after, limit)
        .await
        .map_err(|error| McpError::invalid_params(error.to_string()))?;
    let json = serde_json::to_string_pretty(&page).map_err(|error| {
        McpError::internal(format!("failed to serialize agent event page: {error}"))
    })?;
    Ok(ReadResourceResult::text_with_mime(
        uri,
        json,
        "application/json",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AgentRefusal, AgentRefusalKind};

    #[test]
    fn event_query_defaults_and_parses_unsigned_values() {
        assert_eq!(parse_event_query(&HashMap::new()).unwrap(), (0, 100));
        assert_eq!(
            parse_event_query(&HashMap::from([
                ("after".to_owned(), "42".to_owned()),
                ("limit".to_owned(), "7".to_owned()),
            ]))
            .unwrap(),
            (42, 7)
        );
    }

    #[test]
    fn event_query_rejects_malformed_and_overflowing_values() {
        for (name, value) in [
            ("after", "not-a-number"),
            ("after", "18446744073709551616"),
            ("limit", "not-a-number"),
            ("limit", "184467440737095516160"),
        ] {
            let error = parse_event_query(&HashMap::from([(name.to_owned(), value.to_owned())]))
                .expect_err("malformed query value must fail");
            assert!(error.to_string().contains(name), "{error}");
        }
    }

    fn refused(message: &str) -> AgentTurnResult {
        AgentTurnResult::Refused {
            refusal: AgentRefusal {
                kind: AgentRefusalKind::Runtime,
                message: message.to_owned(),
                active_operation_id: None,
            },
        }
    }

    fn encoded(value: &AgentTurnResult) -> CallToolResult {
        let mut result = CallToolResult::from_serialize(value).unwrap();
        result.is_error = value.is_error();
        result
    }

    async fn client_returning(result: CallToolResult) -> McpClient {
        let turn = ToolBuilder::new(AGENT_TURN_TOOL)
            .handler(move |_input: TurnInput| {
                let result = result.clone();
                async move { Ok::<_, McpError>(result) }
            })
            .build();
        let server = McpRouter::new().server_info("test-agent", "0").tool(turn);
        let client = McpClient::connect(ChannelTransport::new(server))
            .await
            .unwrap();
        client.initialize("test-client", "0").await.unwrap();
        client
    }

    async fn call_result(result: CallToolResult) -> Result<AgentTurnResult, AgentClientError> {
        let client = client_returning(result).await;
        let decoded = call_turn(
            &client,
            TurnInput {
                text: "do the thing".to_owned(),
            },
        )
        .await;
        client.shutdown().await.unwrap();
        decoded
    }

    #[tokio::test]
    async fn call_turn_crosses_the_mcp_client_and_returns_the_typed_result() {
        let expected = refused("typed refusal");
        let actual = call_result(encoded(&expected)).await.unwrap();

        assert_eq!(actual, expected);
    }

    #[test]
    fn structured_content_is_authoritative_over_display_text() {
        let expected = refused("typed refusal");
        let mut result = encoded(&expected);
        result.content = vec![Content::text("not the typed result")];

        assert_eq!(decode_turn(result).unwrap(), expected);
    }

    #[tokio::test]
    async fn display_text_is_never_a_fallback_for_missing_structured_content() {
        let expected = refused("typed refusal");
        let result = CallToolResult::error(serde_json::to_string(&expected).unwrap());

        assert!(matches!(
            call_result(result).await,
            Err(AgentClientError::MissingStructuredContent)
        ));
    }

    #[tokio::test]
    async fn display_text_is_never_a_fallback_for_malformed_structured_content() {
        let expected = refused("typed refusal");
        let mut result = CallToolResult::error(serde_json::to_string(&expected).unwrap());
        result.structured_content = Some(serde_json::json!({"status": "unknown"}));

        assert!(matches!(
            call_result(result).await,
            Err(AgentClientError::DecodeStructuredContent(_))
        ));
    }

    #[tokio::test]
    async fn contradictory_mcp_and_typed_status_fails_closed() {
        let mut result = encoded(&refused("typed refusal"));
        result.is_error = false;

        assert!(matches!(
            call_result(result).await,
            Err(AgentClientError::StatusMismatch {
                mcp_is_error: false,
                typed_is_error: true,
            })
        ));
    }
}
