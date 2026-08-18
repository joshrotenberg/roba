//! MCP-native application host for one logical Roba agent.
//!
//! [`roba_core`] remains the finite, protocol-free execution engine. This
//! crate adds the longer-lived application state: a suspended run template,
//! one retained provider session, single-flight admission, and a typed MCP
//! contract shared by every interface.

mod agent;
mod context;
mod contract;
mod events;
mod extension_lifecycle;
mod extensions;
mod provider_endpoint;
mod router;
mod stdio;

pub use agent::{AgentBuildError, AgentInstance, AgentStopError};
pub use context::{
    AmbientContextPolicy, CONTEXT_MANIFEST_SCHEMA_VERSION, ContextAcquisition, ContextAudience,
    ContextBootstrap, ContextContent, ContextDelivery, ContextEntry, ContextEntryRead,
    ContextEntrySpec, ContextFingerprint, ContextFreshness, ContextGoalDelivery, ContextKind,
    ContextManifest, ContextOrigin, ContextOriginKind, ContextPhase, ContextPlan,
    ContextPlanBuilder, ContextPlanError, ContextPrecedence, ContextReadEvidence, ContextReadStats,
    ContextRequirement, ContextScope, ContextSensitivity, ContextSnapshot,
};
pub use contract::{
    ActiveActivity, ActivityKind, ActivityStatus, AgentConfiguration, AgentControlRefusal,
    AgentControlRefusalKind, AgentFollowUpResult, AgentInterruptResult, AgentObservation,
    AgentRefusal, AgentRefusalKind, AgentShutdownResult, AgentSnapshot, AgentState,
    AgentSteerResult, AgentTerminalState, AgentTurnResult, CancelledTurn, CompletedTurn, Cost,
    Effort, FailedTurn, FailureDetails, FailureKind, LimitPolicy, ObservationHealth,
    ObservationState, OperationId, OperationSettlement, PermissionPolicy, ProviderSelfSnapshot,
    SessionHandle, TokenUsage, ToolPolicy, TurnFailure, TurnMetadata, TurnOutcome, TurnOverrides,
};
pub use events::{
    AGENT_EVENT_CAPACITY, AgentEvent, AgentEventError, AgentEventPage, AgentEventRecord,
    AgentRunState, EventFailureDetails, EventTurnFailure, EventTurnOutcome,
};
pub use extensions::{
    AgentExtension, AgentExtensionChange, AgentExtensionError, AgentExtensionFuture,
    AgentExtensionHookPhase, AgentExtensionLifecycle, AgentExtensionManifestError,
    AgentExtensionOperation, AgentExtensionProjection, AgentExtensions, MAX_EXTENSION_HOOK_TIMEOUT,
};
pub use provider_endpoint::PROVIDER_MCP_SERVER_NAME;
pub use router::{
    AGENT_CONTEXT_ENTRY_TEMPLATE, AGENT_CONTEXT_URI, AGENT_EVENTS_TEMPLATE, AGENT_EVENTS_URI,
    AGENT_FOLLOW_UP_TOOL, AGENT_INTERRUPT_TOOL, AGENT_RESOURCE_URI, AGENT_SHUTDOWN_TOOL,
    AGENT_STEER_TOOL, AGENT_TASK_OPERATION_META_KEY, AGENT_TURN_TOOL, AgentClientError,
    ContextManifestInput, ContextReadInput, FollowUpInput, InterruptInput,
    ROBA_CONTEXT_MANIFEST_TOOL, ROBA_CONTEXT_READ_TOOL, ROBA_SELF_TOOL, SelfInput, ShutdownInput,
    SteerInput, TurnInput, agent_router, call_turn, connect_in_process, control_router, router,
};
pub use stdio::{StdioBinding, StdioBindingHandle};
