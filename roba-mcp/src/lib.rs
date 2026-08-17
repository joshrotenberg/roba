//! MCP-native application host for one logical Roba agent.
//!
//! [`roba_core`] remains the finite, protocol-free execution engine. This
//! crate adds the longer-lived application state: a suspended run template,
//! one retained provider session, single-flight admission, and a typed MCP
//! contract shared by every interface.

mod agent;
mod contract;
mod events;
mod provider_endpoint;
mod router;

pub use agent::{AgentBuildError, AgentInstance, AgentStopError};
pub use contract::{
    AgentConfiguration, AgentControlRefusal, AgentControlRefusalKind, AgentInterruptResult,
    AgentRefusal, AgentRefusalKind, AgentShutdownResult, AgentSnapshot, AgentState,
    AgentSteerResult, AgentTerminalState, AgentTurnResult, CancelledTurn, CompletedTurn, Cost,
    Effort, FailedTurn, FailureDetails, FailureKind, LimitPolicy, OperationId, OperationSettlement,
    PermissionPolicy, ProviderSelfSnapshot, SessionHandle, TokenUsage, ToolPolicy, TurnFailure,
    TurnMetadata, TurnOutcome,
};
pub use events::{
    AGENT_EVENT_CAPACITY, AgentEvent, AgentEventError, AgentEventPage, AgentEventRecord,
    AgentRunState, EventFailureDetails, EventTurnFailure, EventTurnOutcome,
};
pub use provider_endpoint::PROVIDER_MCP_SERVER_NAME;
pub use router::{
    AGENT_EVENTS_TEMPLATE, AGENT_EVENTS_URI, AGENT_INTERRUPT_TOOL, AGENT_RESOURCE_URI,
    AGENT_SHUTDOWN_TOOL, AGENT_STEER_TOOL, AGENT_TASK_OPERATION_META_KEY, AGENT_TURN_TOOL,
    AgentClientError, InterruptInput, ROBA_SELF_TOOL, SelfInput, ShutdownInput, SteerInput,
    TurnInput, agent_router, call_turn, connect_in_process, control_router, router,
};
