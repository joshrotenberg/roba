//! MCP-native application host for one logical Roba agent.
//!
//! [`roba_core`] remains the finite, protocol-free execution engine. This
//! crate adds the longer-lived application state: a suspended run template,
//! one retained provider session, single-flight admission, and a typed MCP
//! contract shared by every interface.

mod agent;
mod contract;
mod router;

pub use agent::{AgentBuildError, AgentInstance, AgentStopError};
pub use contract::{
    AgentConfiguration, AgentRefusal, AgentRefusalKind, AgentSnapshot, AgentState, AgentTurnResult,
    CancelledTurn, CompletedTurn, Cost, Effort, FailedTurn, FailureDetails, FailureKind,
    LimitPolicy, OperationId, PermissionPolicy, SessionHandle, TokenUsage, ToolPolicy, TurnFailure,
    TurnMetadata, TurnOutcome,
};
pub use router::{AGENT_RESOURCE_URI, AGENT_TURN_TOOL, TurnInput, connect_in_process, router};
