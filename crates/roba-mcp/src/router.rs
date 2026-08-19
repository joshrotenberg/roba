//! Base MCP router and process-local client binding.

use std::collections::HashMap;
use std::error::Error as StdError;
use std::fmt;
use std::sync::Arc;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use tower_mcp::async_task::{MemoryTaskStore, TaskStore};
use tower_mcp::schemars::{self, JsonSchema, schema_for};
use tower_mcp::{
    CallToolResult, ChannelTransport, Content, Error as McpError, LogLevel, LoggingMessageParams,
    McpClient, McpRouter, ReadResourceResult, RequestContext, ResourceBuilder,
    ResourceTemplateBuilder, TaskContext, TaskOutcome, TaskPreparation, ToolBuilder,
};

use crate::agent::TurnAdmission;
use crate::context::ContextReadError;
use crate::provider_endpoint::PROVIDER_MCP_SERVER_NAME;
use crate::{
    AGENT_EVENT_CAPACITY, AgentFollowUpResult, AgentInstance, AgentInterruptResult,
    AgentSessionRotateResult, AgentShutdownResult, AgentTurnResult, OperationId,
    ProviderSelfSnapshot, SessionRotationStrategy, TurnOverrides,
};

/// Base tool for one finite turn through the logical agent.
pub const AGENT_TURN_TOOL: &str = "agent.turn";
/// Control tool for queueing a prompt at the next provider-turn boundary.
pub const AGENT_FOLLOW_UP_TOOL: &str = "agent.follow_up";
/// Source-compatible constant for the renamed follow-up tool.
pub const AGENT_STEER_TOOL: &str = AGENT_FOLLOW_UP_TOOL;
/// Control tool for cancelling one active operation and awaiting settlement.
pub const AGENT_INTERRUPT_TOOL: &str = "agent.interrupt";
/// Control tool for cleanly advancing one idle provider-session generation.
pub const AGENT_SESSION_ROTATE_TOOL: &str = "agent.session.rotate";
/// Control tool for permanently stopping the logical agent.
pub const AGENT_SHUTDOWN_TOOL: &str = "agent.shutdown";
/// Harmless identity callback exposed only to the executing provider.
///
/// Provider clients see the fully qualified name `roba.self` because the
/// private MCP server itself is named `roba`.
pub const ROBA_SELF_TOOL: &str = "self";
/// Read the content-free context manifest and current operation evidence.
pub const ROBA_CONTEXT_MANIFEST_TOOL: &str = "context.manifest";
/// Read one generation-fenced context entry and record provider acquisition.
pub const ROBA_CONTEXT_READ_TOOL: &str = "context.read";
/// Dynamic state resource for the logical agent.
pub const AGENT_RESOURCE_URI: &str = "roba://agent";
/// Default agent-wide event resource.
pub const AGENT_EVENTS_URI: &str = "roba://events";
/// Cursor-paged agent-wide event resource template.
pub const AGENT_EVENTS_TEMPLATE: &str = "roba://events{?after,limit}";
/// Content-free effective context and provider read evidence.
pub const AGENT_CONTEXT_URI: &str = "roba://context";
/// Generation-fenced content for one context entry.
pub const AGENT_CONTEXT_ENTRY_TEMPLATE: &str = "roba://context/entry{?id,generation}";
/// Task metadata key carrying the exact admitted Roba operation identity.
pub const AGENT_TASK_OPERATION_META_KEY: &str = "com.github.joshrotenberg.roba/operation";

/// Stable operator guidance published during MCP initialization/discovery.
///
/// Keep this deliberately compact. Tool and resource discovery remain the
/// canonical API reference; these instructions explain the lifecycle and
/// point clients at the dynamic state they need to operate it correctly.
const CONTROL_INSTRUCTIONS: &str = "\
This endpoint controls one persistent logical Roba agent. Start work with \
agent.turn; use MCP Tasks for long-running turns. Only one operation may run \
at a time. Read roba://agent for current state and operation identity, \
roba://context for supplied context and provenance, and roba://events for \
replayable provider activity. Task-backed turns may also deliver live \
roba.activity log notifications. Use agent.follow_up to queue another provider turn after the \
current one, agent.interrupt to cancel \
work while keeping the agent available, agent.session.rotate to cleanly reset \
provider continuity while idle, and agent.shutdown only when the host \
should terminate. When present, managed prompts and the content-free catalog \
are discoverable through prompts/list and roba://context/catalog. Additional \
capabilities may be exposed as MCP extensions; \
inspect discovery rather than assuming they exist.";

const DEFAULT_EVENT_LIMIT: usize = 100;
// Keep an admitted live operation addressable for effectively the process
// lifetime. This is the largest integer exactly representable by common JSON
// clients. The handler shortens the lease after settlement.
const ACTIVE_TASK_TTL_MS: u64 = 9_007_199_254_740_991;
const SETTLED_TASK_RETENTION_MS: u64 = 300_000;
const TASK_CREATION_SLACK_MS: u64 = 60_000;

#[derive(Clone)]
struct PreparedTurn {
    admission: TurnAdmission,
    prepared_at: Instant,
}

/// Input contract for [`AGENT_TURN_TOOL`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TurnInput {
    #[schemars(regex(pattern = r"\S"))]
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overrides: Option<TurnOverrides>,
}

/// Input contract for [`AGENT_FOLLOW_UP_TOOL`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FollowUpInput {
    pub operation_id: OperationId,
    #[schemars(regex(pattern = r"\S"))]
    pub text: String,
}

/// Source-compatible name for [`FollowUpInput`].
pub type SteerInput = FollowUpInput;

/// Input contract for [`AGENT_INTERRUPT_TOOL`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InterruptInput {
    pub operation_id: OperationId,
}

/// Input contract for [`AGENT_SESSION_ROTATE_TOOL`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SessionRotateInput {
    pub expected_generation: u64,
    pub strategy: SessionRotationStrategy,
}

/// Input contract for [`AGENT_SHUTDOWN_TOOL`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShutdownInput {}

/// Empty input contract for [`ROBA_SELF_TOOL`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SelfInput {}

/// Empty input contract for [`ROBA_CONTEXT_MANIFEST_TOOL`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ContextManifestInput {}

/// Input contract for [`ROBA_CONTEXT_READ_TOOL`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ContextReadInput {
    #[schemars(regex(pattern = r"\S"))]
    pub id: String,
    pub generation: u64,
}

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

/// Build the operator/control projection for one logical agent.
pub fn control_router(agent: AgentInstance) -> McpRouter {
    let extensions = agent.extensions().clone();
    extensions.merge_control(base_control_router(agent))
}

pub(crate) fn base_control_router(agent: AgentInstance) -> McpRouter {
    let task_store = Arc::new(MemoryTaskStore::new());
    let turn_output_schema = serde_json::to_value(schema_for!(AgentTurnResult))
        .expect("static agent turn schema must serialize");
    let task_agent = agent.clone();
    let task_execution_store = task_store.clone();
    let fallback_agent = agent.clone();
    let preparation_agent = agent.clone();
    let preparation_store = task_store.clone();
    let turn = ToolBuilder::new(AGENT_TURN_TOOL)
        .description("Run one prompt through this logical Roba agent.")
        .output_schema(turn_output_schema)
        .live_task_handler_with_context(
            move |context: RequestContext, task: TaskContext, _input: TurnInput| {
                let agent = task_agent.clone();
                let store = task_execution_store.clone();
                async move {
                    let prepared =
                        context
                            .extension::<PreparedTurn>()
                            .cloned()
                            .ok_or_else(|| {
                                McpError::internal(
                                    "agent.turn task started without prepared turn admission",
                                )
                            })?;
                    execute_task_turn(agent, context, task, prepared, store).await
                }
            },
        )
        .fallback_handler(move |input: TurnInput| {
            let agent = fallback_agent.clone();
            async move {
                let result = agent
                    .turn_with_overrides(input.text, input.overrides.unwrap_or_default())
                    .await;
                encode_turn(&result)
            }
        })
        .build()
        .with_typed_task_preparation(move |task: TaskContext, input: TurnInput| {
            let agent = preparation_agent.clone();
            let store = preparation_store.clone();
            async move {
                let prepared_at = Instant::now();
                // MemoryTaskStore hides expired records but only reclaims
                // them when asked. Reclaim at the next task boundary so a hot
                // agent does not accumulate every completed turn forever.
                store.cleanup_expired();
                let retained = store
                    .set_ttl(task.task_id(), ACTIVE_TASK_TTL_MS)
                    .await
                    .map_err(|error| {
                        McpError::internal(format!(
                            "failed to retain live agent.turn task: {error}"
                        ))
                    })?;
                if !retained {
                    return Err(McpError::internal(
                        "live agent.turn task disappeared during preparation",
                    ));
                }
                let admission = agent
                    .admit_turn_with_overrides(input.text, input.overrides.unwrap_or_default())
                    .await;
                let mut preparation = TaskPreparation::new().with_extension(PreparedTurn {
                    admission: admission.clone(),
                    prepared_at,
                });
                if let TurnAdmission::Admitted(turn) = admission {
                    preparation = preparation.with_meta(serde_json::Map::from_iter([(
                        AGENT_TASK_OPERATION_META_KEY.to_owned(),
                        serde_json::json!({ "operationId": turn.operation_id() }),
                    )]));
                }
                Ok(preparation)
            }
        });

    let follow_up_output_schema = serde_json::to_value(schema_for!(AgentFollowUpResult))
        .expect("static agent follow-up schema must serialize");
    let follow_up_agent = agent.clone();
    let follow_up = ToolBuilder::new(AGENT_FOLLOW_UP_TOOL)
        .description("Queue a prompt for the next turn of one exact active operation.")
        .output_schema(follow_up_output_schema)
        .handler(move |input: FollowUpInput| {
            let agent = follow_up_agent.clone();
            async move {
                let result = agent.follow_up(input.operation_id, input.text).await;
                encode_follow_up(&result)
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

    let rotate_output_schema = serde_json::to_value(schema_for!(AgentSessionRotateResult))
        .expect("static agent session rotation schema must serialize");
    let rotate_agent = agent.clone();
    let rotate = ToolBuilder::new(AGENT_SESSION_ROTATE_TOOL)
        .description(
            "Cleanly rotate retained provider continuity at one exact idle session generation.",
        )
        .output_schema(rotate_output_schema)
        .handler(move |input: SessionRotateInput| {
            let agent = rotate_agent.clone();
            async move {
                let result = agent
                    .rotate_session(input.expected_generation, input.strategy)
                    .await;
                encode_session_rotate(&result)
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

    let template_events_agent = agent.clone();
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

    let context_manifest_agent = agent.clone();
    let context_manifest = ResourceBuilder::new(AGENT_CONTEXT_URI)
        .name("Roba context manifest")
        .description("Roba-declared context provenance and provider read evidence.")
        .mime_type("application/json")
        .handler(move || {
            let agent = context_manifest_agent.clone();
            async move {
                let snapshot = agent.context_snapshot().await;
                serialize_resource(AGENT_CONTEXT_URI, &snapshot, "context snapshot")
            }
        })
        .build();

    let context_entry_agent = agent;
    let context_entry = ResourceTemplateBuilder::new(AGENT_CONTEXT_ENTRY_TEMPLATE)
        .name("Roba context entry")
        .description("Read one explicit context entry from an exact context generation.")
        .mime_type("application/json")
        .argument("id", Some("Stable context entry ID."), true)
        .argument("generation", Some("Exact context generation."), true)
        .handler(move |uri: String, variables: HashMap<String, String>| {
            let agent = context_entry_agent.clone();
            async move {
                let (id, generation) = parse_context_query(&variables)?;
                let content = agent
                    .context_content(&id, generation)
                    .await
                    .map_err(context_read_error)?;
                serialize_resource(&uri, &content, "context entry")
            }
        });

    McpRouter::new()
        .server_info("roba-agent", env!("CARGO_PKG_VERSION"))
        .instructions(CONTROL_INSTRUCTIONS)
        .task_store(task_store)
        .tool(turn)
        .tool(follow_up)
        .tool(interrupt)
        .tool(rotate)
        .tool(shutdown)
        .resource(state)
        .resource(events)
        .resource(context_manifest)
        .resource_template(events_template)
        .resource_template(context_entry)
        .with_tasks()
        .catch_panics()
}

/// Build the least-authority projection injected into one provider operation.
///
/// This is an explicit allowlist, not a filtered control router. It contains
/// only operation identity, generation-fenced context resources, and explicit
/// provider extension capabilities. It contains no turn admission, follow-up,
/// interruption, shutdown, Tasks, event history, configuration, or retained
/// provider-session state.
pub fn agent_router(agent: AgentInstance, operation_id: OperationId) -> McpRouter {
    let extensions = agent.extensions().clone();
    extensions.merge_provider(base_agent_router(agent, operation_id))
}

pub(crate) fn base_agent_router(agent: AgentInstance, operation_id: OperationId) -> McpRouter {
    let weak_agent = agent.downgrade();
    let output_schema = serde_json::to_value(schema_for!(ProviderSelfSnapshot))
        .expect("static provider self schema must serialize");
    let self_tool = ToolBuilder::new(ROBA_SELF_TOOL)
        .description("Identify this exact active Roba provider operation.")
        .output_schema(output_schema)
        .read_only_safe()
        .handler(move |_input: SelfInput| {
            let agent = weak_agent.clone();
            async move {
                let snapshot = agent.provider_self(operation_id).await.ok_or_else(|| {
                    McpError::tool_with_name(
                        ROBA_SELF_TOOL,
                        "provider operation is unavailable or has expired",
                    )
                })?;
                let mut result = CallToolResult::from_serialize(&snapshot)?;
                result.content = vec![Content::text(format!(
                    "Roba operation {} is running",
                    operation_id.get()
                ))];
                Ok(result)
            }
        })
        .build();

    let manifest_agent = agent.downgrade();
    let context_manifest = ResourceBuilder::new(AGENT_CONTEXT_URI)
        .name("Roba context manifest")
        .description("Context available to this exact provider operation.")
        .mime_type("application/json")
        .handler(move || {
            let agent = manifest_agent.clone();
            async move {
                let snapshot = agent
                    .provider_context_manifest(operation_id)
                    .await
                    .ok_or_else(|| {
                        McpError::invalid_params(format!(
                            "context for operation {} is unavailable or has expired",
                            operation_id.get()
                        ))
                    })?;
                serialize_resource(AGENT_CONTEXT_URI, &snapshot, "provider context snapshot")
            }
        })
        .build();

    let manifest_tool_agent = agent.downgrade();
    let manifest_output_schema = serde_json::to_value(schema_for!(crate::ContextSnapshot))
        .expect("static context manifest schema must serialize");
    let context_manifest_tool = ToolBuilder::new(ROBA_CONTEXT_MANIFEST_TOOL)
        .description(
            "Inspect Roba-declared context and read evidence for this exact operation before deciding which context entries to read.",
        )
        .output_schema(manifest_output_schema)
        .read_only_safe()
        .handler(move |_input: ContextManifestInput| {
            let agent = manifest_tool_agent.clone();
            async move {
                let snapshot = agent
                    .provider_context_manifest(operation_id)
                    .await
                    .ok_or_else(|| {
                        McpError::tool_with_name(
                            ROBA_CONTEXT_MANIFEST_TOOL,
                            format!(
                                "context for operation {} is unavailable or has expired",
                                operation_id.get()
                            ),
                        )
                    })?;
                let mut result = CallToolResult::from_serialize(&snapshot)?;
                result.content = vec![Content::text(
                    serde_json::to_string_pretty(&snapshot).map_err(|error| {
                        McpError::internal(format!(
                            "failed to serialize provider context snapshot: {error}"
                        ))
                    })?,
                )];
                Ok(result)
            }
        })
        .build();

    let content_tool_agent = agent.downgrade();
    let content_output_schema = serde_json::to_value(schema_for!(crate::ContextContent))
        .expect("static context content schema must serialize");
    let context_read_tool = ToolBuilder::new(ROBA_CONTEXT_READ_TOOL)
        .description(
            "Read one Roba context entry using the exact id and generation from context.manifest.",
        )
        .output_schema(content_output_schema)
        .read_only_safe()
        .handler(move |input: ContextReadInput| {
            let agent = content_tool_agent.clone();
            async move {
                let content = agent
                    .provider_context_content(operation_id, &input.id, input.generation)
                    .await
                    .map_err(|error| {
                        McpError::tool_with_name(ROBA_CONTEXT_READ_TOOL, error.to_string())
                    })?;
                let mut result = CallToolResult::from_serialize(&content)?;
                result.content = vec![Content::text(content.content.clone())];
                Ok(result)
            }
        })
        .build();

    let content_agent = agent.downgrade();
    let context_entry = ResourceTemplateBuilder::new(AGENT_CONTEXT_ENTRY_TEMPLATE)
        .name("Roba context entry")
        .description("Read one context entry for this exact provider operation and generation.")
        .mime_type("application/json")
        .argument("id", Some("Stable context entry ID."), true)
        .argument("generation", Some("Exact context generation."), true)
        .handler(move |uri: String, variables: HashMap<String, String>| {
            let agent = content_agent.clone();
            async move {
                let (id, generation) = parse_context_query(&variables)?;
                let content = agent
                    .provider_context_content(operation_id, &id, generation)
                    .await
                    .map_err(context_read_error)?;
                serialize_resource(&uri, &content, "provider context entry")
            }
        });

    McpRouter::new()
        .server_info(PROVIDER_MCP_SERVER_NAME, env!("CARGO_PKG_VERSION"))
        .tool(self_tool)
        .tool(context_manifest_tool)
        .tool(context_read_tool)
        .resource(context_manifest)
        .resource_template(context_entry)
        .catch_panics()
}

/// Build the base operator projection.
///
/// Kept as the compatibility name used by existing clients and tests.
pub fn router(agent: AgentInstance) -> McpRouter {
    control_router(agent)
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

async fn execute_task_turn(
    agent: AgentInstance,
    context: RequestContext,
    task: TaskContext,
    prepared: PreparedTurn,
    store: Arc<MemoryTaskStore>,
) -> tower_mcp::Result<TaskOutcome> {
    let outcome = match prepared.admission {
        TurnAdmission::Refused(result) => TaskOutcome::Completed(encode_turn(&result)?),
        TurnAdmission::Admitted(turn) if task.is_cancelled() => {
            settle_cancelled_task(&agent, &turn).await?
        }
        TurnAdmission::Admitted(turn) => {
            let mut events = agent.subscribe_live_events();
            let replay = agent
                .event_page(0, AGENT_EVENT_CAPACITY)
                .await
                .map_err(|error| McpError::internal(error.to_string()))?;
            if replay.truncated {
                context.send_log(
                    LoggingMessageParams::new(
                        LogLevel::Warning,
                        serde_json::json!({
                            "kind": "live_activity_truncated",
                            "operation_id": turn.operation_id(),
                            "replay_resource": AGENT_EVENTS_URI,
                        }),
                    )
                    .with_logger("roba.activity"),
                );
            }
            let mut delivered_through = 0;
            for record in replay
                .events
                .iter()
                .filter(|record| record.operation_id == turn.operation_id())
            {
                send_live_event(&context, record)?;
                delivered_through = delivered_through.max(record.sequence);
            }
            let outcome = loop {
                tokio::select! {
                    biased;
                    result = agent.wait_admitted(&turn) => {
                        break TaskOutcome::Completed(encode_turn(&result)?);
                    }
                    () = task.cancelled() => {
                        break settle_cancelled_task(&agent, &turn).await?;
                    }
                    event = events.recv() => match event {
                        Ok(record)
                            if record.operation_id == turn.operation_id()
                                && record.sequence > delivered_through => {
                            send_live_event(&context, &record)?;
                            delivered_through = record.sequence;
                        }
                        Ok(_) => {}
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                            context.send_log(
                                LoggingMessageParams::new(
                                    LogLevel::Warning,
                                    serde_json::json!({
                                        "kind": "live_activity_truncated",
                                        "operation_id": turn.operation_id(),
                                        "skipped": skipped,
                                        "replay_resource": AGENT_EVENTS_URI,
                                    }),
                                )
                                .with_logger("roba.activity"),
                            );
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => {}
                    },
                }
            };
            drain_live_events(
                &context,
                &mut events,
                turn.operation_id(),
                &mut delivered_through,
            )?;
            outcome
        }
    };
    retain_settled_task(&store, &task, prepared.prepared_at).await?;
    Ok(outcome)
}

fn drain_live_events(
    context: &RequestContext,
    events: &mut tokio::sync::broadcast::Receiver<crate::AgentEventRecord>,
    operation_id: OperationId,
    delivered_through: &mut u64,
) -> tower_mcp::Result<()> {
    loop {
        match events.try_recv() {
            Ok(record)
                if record.operation_id == operation_id && record.sequence > *delivered_through =>
            {
                send_live_event(context, &record)?;
                *delivered_through = record.sequence;
            }
            Ok(_) => {}
            Err(tokio::sync::broadcast::error::TryRecvError::Lagged(skipped)) => {
                context.send_log(
                    LoggingMessageParams::new(
                        LogLevel::Warning,
                        serde_json::json!({
                            "kind": "live_activity_truncated",
                            "operation_id": operation_id,
                            "skipped": skipped,
                            "replay_resource": AGENT_EVENTS_URI,
                        }),
                    )
                    .with_logger("roba.activity"),
                );
            }
            Err(
                tokio::sync::broadcast::error::TryRecvError::Empty
                | tokio::sync::broadcast::error::TryRecvError::Closed,
            ) => return Ok(()),
        }
    }
}

fn send_live_event(
    context: &RequestContext,
    record: &crate::AgentEventRecord,
) -> tower_mcp::Result<()> {
    let level = match &record.event {
        crate::AgentEvent::Warning { .. } | crate::AgentEvent::RunHistoryTruncated { .. } => {
            Some(LogLevel::Warning)
        }
        crate::AgentEvent::ActivityStarted { .. } | crate::AgentEvent::ActivityCompleted { .. } => {
            Some(LogLevel::Info)
        }
        _ => None,
    };
    if let Some(level) = level {
        context.send_log(
            LoggingMessageParams::new(level, serde_json::to_value(record)?)
                .with_logger("roba.activity"),
        );
    }
    Ok(())
}

async fn settle_cancelled_task(
    agent: &AgentInstance,
    turn: &crate::agent::AdmittedTurn,
) -> tower_mcp::Result<TaskOutcome> {
    let result = agent.cancel_admitted_and_wait(turn).await;
    if matches!(result, AgentTurnResult::Cancelled { .. }) {
        Ok(TaskOutcome::Cancelled {
            message: Some("agent turn cancelled".to_owned()),
        })
    } else {
        Ok(TaskOutcome::Completed(encode_turn(&result)?))
    }
}

async fn retain_settled_task(
    store: &MemoryTaskStore,
    task: &TaskContext,
    prepared_at: Instant,
) -> tower_mcp::Result<()> {
    let elapsed_ms = u64::try_from(prepared_at.elapsed().as_millis()).unwrap_or(u64::MAX);
    // Tower measures TTL from task creation, which slightly precedes task
    // preparation. Include conservative slack so the visible terminal result
    // remains available for at least the intended retention in practice.
    let ttl_ms = elapsed_ms
        .saturating_add(SETTLED_TASK_RETENTION_MS)
        .saturating_add(TASK_CREATION_SLACK_MS);
    let retained = store
        .set_ttl(task.task_id(), ttl_ms)
        .await
        .map_err(|error| {
            McpError::internal(format!("failed to retain settled agent.turn task: {error}"))
        })?;
    if !retained {
        return Err(McpError::internal(
            "agent.turn task disappeared before settlement was recorded",
        ));
    }
    Ok(())
}

fn encode_follow_up(value: &AgentFollowUpResult) -> tower_mcp::Result<CallToolResult> {
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

fn encode_session_rotate(value: &AgentSessionRotateResult) -> tower_mcp::Result<CallToolResult> {
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

fn parse_context_query(variables: &HashMap<String, String>) -> tower_mcp::Result<(String, u64)> {
    let id = variables
        .get("id")
        .filter(|id| !id.trim().is_empty())
        .cloned()
        .ok_or_else(|| McpError::invalid_params("context entry id is required"))?;
    let generation = variables
        .get("generation")
        .ok_or_else(|| McpError::invalid_params("context generation is required"))?
        .parse::<u64>()
        .map_err(|_| McpError::invalid_params("context generation must be an unsigned integer"))?;
    Ok((id, generation))
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

fn context_read_error(error: ContextReadError) -> McpError {
    McpError::invalid_params(error.to_string())
}

fn serialize_resource<T: Serialize>(
    uri: &str,
    value: &T,
    label: &str,
) -> tower_mcp::Result<ReadResourceResult> {
    let json = serde_json::to_string_pretty(value)
        .map_err(|error| McpError::internal(format!("failed to serialize {label}: {error}")))?;
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

    #[test]
    fn context_query_requires_an_entry_and_exact_unsigned_generation() {
        assert_eq!(
            parse_context_query(&HashMap::from([
                ("id".to_owned(), "agent.instruction.1".to_owned()),
                ("generation".to_owned(), "7".to_owned()),
            ]))
            .unwrap(),
            ("agent.instruction.1".to_owned(), 7)
        );
        for variables in [
            HashMap::new(),
            HashMap::from([("id".to_owned(), "entry".to_owned())]),
            HashMap::from([
                ("id".to_owned(), " ".to_owned()),
                ("generation".to_owned(), "1".to_owned()),
            ]),
            HashMap::from([
                ("id".to_owned(), "entry".to_owned()),
                ("generation".to_owned(), "not-a-number".to_owned()),
            ]),
        ] {
            assert!(parse_context_query(&variables).is_err());
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
                overrides: None,
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
