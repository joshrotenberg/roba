//! MCP-native application host for one logical Roba agent.
//!
//! [`roba_core`] remains the finite, protocol-free execution engine. This
//! crate adds the longer-lived application state: a suspended run template,
//! one retained provider session, single-flight admission, and a typed MCP
//! contract shared by every interface.

mod agent;
mod contract;
mod events;
mod router;

pub use agent::{AgentBuildError, AgentInstance, AgentStopError};
pub use contract::{
    AgentConfiguration, AgentControlRefusal, AgentControlRefusalKind, AgentInterruptResult,
    AgentRefusal, AgentRefusalKind, AgentShutdownResult, AgentSnapshot, AgentState,
    AgentSteerResult, AgentTerminalState, AgentTurnResult, CancelledTurn, CompletedTurn, Cost,
    Effort, FailedTurn, FailureDetails, FailureKind, LimitPolicy, OperationId, OperationSettlement,
    PermissionPolicy, SessionHandle, TokenUsage, ToolPolicy, TurnFailure, TurnMetadata,
    TurnOutcome,
};
pub use events::{
    AGENT_EVENT_CAPACITY, AgentEvent, AgentEventError, AgentEventPage, AgentEventRecord,
    AgentRunState, EventFailureDetails, EventTurnFailure, EventTurnOutcome,
};
pub use router::{
    AGENT_EVENTS_TEMPLATE, AGENT_EVENTS_URI, AGENT_INTERRUPT_TOOL, AGENT_RESOURCE_URI,
    AGENT_SHUTDOWN_TOOL, AGENT_STEER_TOOL, AGENT_TASK_OPERATION_META_KEY, AGENT_TURN_TOOL,
    AgentClientError, InterruptInput, ShutdownInput, SteerInput, TurnInput, call_turn,
    connect_in_process, router,
};
