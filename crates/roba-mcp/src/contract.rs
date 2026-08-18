//! Typed MCP values owned by the agent-host boundary.

use serde::{Deserialize, Serialize};
use tower_mcp::schemars::{self, JsonSchema};

/// Provider-neutral category for mechanically observed activity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ActivityKind {
    Command,
    FileChange,
    McpCall,
    WebSearch,
    PlanUpdate,
    StatusUpdate,
    ToolCall,
    Unknown,
}

impl From<roba_core::ProviderActivityKind> for ActivityKind {
    fn from(value: roba_core::ProviderActivityKind) -> Self {
        match value {
            roba_core::ProviderActivityKind::Command => Self::Command,
            roba_core::ProviderActivityKind::FileChange => Self::FileChange,
            roba_core::ProviderActivityKind::McpCall => Self::McpCall,
            roba_core::ProviderActivityKind::WebSearch => Self::WebSearch,
            roba_core::ProviderActivityKind::PlanUpdate => Self::PlanUpdate,
            roba_core::ProviderActivityKind::StatusUpdate => Self::StatusUpdate,
            roba_core::ProviderActivityKind::ToolCall => Self::ToolCall,
            roba_core::ProviderActivityKind::Unknown => Self::Unknown,
        }
    }
}

/// Provider-reported terminal disposition for one activity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ActivityStatus {
    Succeeded,
    Failed,
    Cancelled,
    Unknown,
}

impl From<roba_core::ProviderActivityStatus> for ActivityStatus {
    fn from(value: roba_core::ProviderActivityStatus) -> Self {
        match value {
            roba_core::ProviderActivityStatus::Succeeded => Self::Succeeded,
            roba_core::ProviderActivityStatus::Failed => Self::Failed,
            roba_core::ProviderActivityStatus::Cancelled => Self::Cancelled,
            roba_core::ProviderActivityStatus::Unknown => Self::Unknown,
        }
    }
}

/// One provider activity still active according to native stream evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ActiveActivity {
    pub id: String,
    pub activity: ActivityKind,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at_unix_ms: Option<u64>,
}

/// Coarse truthfulness state of provider-native observation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ObservationState {
    Unknown,
    Active,
    RecentlyActive,
    Terminal,
}

/// Whether Roba has a complete, mechanically sourced observation stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ObservationHealth {
    Unknown,
    Healthy,
    Degraded,
    Terminal,
}

/// Live provider-native evidence for the current or latest operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AgentObservation {
    pub state: ObservationState,
    pub health: ObservationHealth,
    pub active_activities: Vec<ActiveActivity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_provider_event_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_provider_event_at_unix_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_activity_at_unix_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub elapsed_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_remaining_ms: Option<u64>,
}

impl AgentObservation {
    pub(crate) fn unknown() -> Self {
        Self {
            state: ObservationState::Unknown,
            health: ObservationHealth::Unknown,
            active_activities: Vec::new(),
            last_provider_event_kind: None,
            last_provider_event_at_unix_ms: None,
            last_activity_at_unix_ms: None,
            elapsed_ms: None,
            timeout_remaining_ms: None,
        }
    }
}

/// Monotonic identity for one submitted agent turn.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(transparent)]
pub struct OperationId(u64);

impl OperationId {
    /// Construct an operation identity for a typed client request.
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Return the numeric operation identity.
    pub fn get(self) -> u64 {
        self.0
    }
}

/// Coarse lifetime of the hot logical agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AgentState {
    Idle,
    Running,
    Stopping,
    Stopped,
}

/// Safe, inspectable configuration published by `roba://agent`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AgentConfiguration {
    pub provider: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<Effort>,
    pub permissions: PermissionPolicy,
    pub tools: ToolPolicy,
    pub limits: LimitPolicy,
}

impl AgentConfiguration {
    pub(crate) fn from_run_spec(spec: &roba_core::RunSpec) -> Self {
        Self {
            provider: spec.agent.provider.to_string(),
            model: spec.agent.model.clone(),
            effort: spec.agent.effort.map(Into::into),
            permissions: spec.execution.permissions.into(),
            tools: spec.execution.tools.clone().into(),
            limits: spec.execution.limits.clone().into(),
        }
    }
}

/// Provider-neutral reasoning effort configured for the hosted agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Effort {
    Low,
    Medium,
    High,
    XHigh,
    Max,
}

impl From<roba_core::Effort> for Effort {
    fn from(value: roba_core::Effort) -> Self {
        match value {
            roba_core::Effort::Low => Self::Low,
            roba_core::Effort::Medium => Self::Medium,
            roba_core::Effort::High => Self::High,
            roba_core::Effort::XHigh => Self::XHigh,
            roba_core::Effort::Max => Self::Max,
        }
    }
}

impl From<Effort> for roba_core::Effort {
    fn from(value: Effort) -> Self {
        match value {
            Effort::Low => Self::Low,
            Effort::Medium => Self::Medium,
            Effort::High => Self::High,
            Effort::XHigh => Self::XHigh,
            Effort::Max => Self::Max,
        }
    }
}

/// Provider-independent authority requested for the hosted agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PermissionPolicy {
    ReadOnly,
    WorkspaceWrite,
    FullAuto,
}

impl From<roba_core::PermissionPolicy> for PermissionPolicy {
    fn from(value: roba_core::PermissionPolicy) -> Self {
        match value {
            roba_core::PermissionPolicy::ReadOnly => Self::ReadOnly,
            roba_core::PermissionPolicy::WorkspaceWrite => Self::WorkspaceWrite,
            roba_core::PermissionPolicy::FullAuto => Self::FullAuto,
        }
    }
}

/// Granular tool policy configured on the run template.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ToolPolicy {
    pub allow: Vec<String>,
    pub deny: Vec<String>,
}

impl From<roba_core::ToolPolicy> for ToolPolicy {
    fn from(value: roba_core::ToolPolicy) -> Self {
        Self {
            allow: value.allow,
            deny: value.deny,
        }
    }
}

/// Provider-independent ceilings configured on the run template.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct LimitPolicy {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_turns: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_cost_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
}

/// Provider settings and ceilings applied to one admitted operation only.
///
/// Follow-ups within that operation retain the same effective configuration.
/// Authority, provider identity, context, tools, and session continuity remain
/// fixed by the hosted agent.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TurnOverrides {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<Effort>,
    #[serde(default)]
    pub limits: LimitPolicy,
}

impl From<roba_core::LimitSpec> for LimitPolicy {
    fn from(value: roba_core::LimitSpec) -> Self {
        Self {
            max_turns: value.max_turns,
            max_cost_usd: value.max_cost_usd,
            timeout_secs: value.timeout_secs,
        }
    }
}

/// Public snapshot of the hot agent application state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AgentSnapshot {
    pub configuration: AgentConfiguration,
    /// Effective configuration for the active operation, including one-turn overrides.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_configuration: Option<AgentConfiguration>,
    pub observation: AgentObservation,
    pub state: AgentState,
    pub session_available: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_operation_id: Option<OperationId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_turn: Option<AgentTurnResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at_unix_ms: Option<u64>,
}

/// Least-authority identity exposed to the currently executing provider.
///
/// This projection deliberately omits agent configuration, retained provider
/// session evidence, prior turn results, event history, and control authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProviderSelfSnapshot {
    pub operation_id: OperationId,
    pub state: AgentState,
}

/// Why a turn was refused before provider work began.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AgentRefusalKind {
    InvalidPrompt,
    InvalidConfiguration,
    Busy,
    Stopped,
    Runtime,
}

/// Why an agent control operation could not be applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AgentControlRefusalKind {
    InvalidPrompt,
    Idle,
    Stopping,
    Stopped,
    OperationMismatch,
    OperationFinishing,
    OperationSettled,
    QueueFull,
    Unsupported,
    Runtime,
}

/// Typed refusal for follow-up or interruption.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AgentControlRefusal {
    pub kind: AgentControlRefusalKind,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_operation_id: Option<OperationId>,
}

/// Terminal disposition of one admitted operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AgentTerminalState {
    Completed,
    Failed,
    Cancelled,
}

/// Stable, compact evidence that an admitted operation has settled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct OperationSettlement {
    pub operation_id: OperationId,
    pub state: AgentTerminalState,
}

impl OperationSettlement {
    pub(crate) fn from_turn(result: &AgentTurnResult) -> Option<Self> {
        let (operation_id, state) = match result {
            AgentTurnResult::Completed { operation_id, .. } => {
                (*operation_id, AgentTerminalState::Completed)
            }
            AgentTurnResult::Failed { operation_id, .. } => {
                (*operation_id, AgentTerminalState::Failed)
            }
            AgentTurnResult::Cancelled { operation_id, .. } => {
                (*operation_id, AgentTerminalState::Cancelled)
            }
            AgentTurnResult::Refused { .. } => return None,
        };
        Some(Self {
            operation_id,
            state,
        })
    }
}

/// Result of queueing a follow-up for the active finite run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum AgentFollowUpResult {
    Queued { operation_id: OperationId },
    Refused { refusal: AgentControlRefusal },
}

impl AgentFollowUpResult {
    pub(crate) fn refused(
        kind: AgentControlRefusalKind,
        message: impl Into<String>,
        current_operation_id: Option<OperationId>,
    ) -> Self {
        Self::Refused {
            refusal: AgentControlRefusal {
                kind,
                message: message.into(),
                current_operation_id,
            },
        }
    }

    /// True when MCP should mark this as a tool execution error.
    pub fn is_error(&self) -> bool {
        matches!(self, Self::Refused { .. })
    }

    /// Human-facing text paired with the typed structured result.
    pub fn display_text(&self) -> String {
        match self {
            Self::Queued { operation_id } => {
                format!("follow-up queued for operation {}", operation_id.get())
            }
            Self::Refused { refusal } => refusal.message.clone(),
        }
    }
}

/// Source-compatible name for [`AgentFollowUpResult`].
pub type AgentSteerResult = AgentFollowUpResult;

/// Result of interrupting one exact admitted operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum AgentInterruptResult {
    Settled {
        settlement: OperationSettlement,
        cancellation_requested: bool,
    },
    Refused {
        refusal: AgentControlRefusal,
    },
}

impl AgentInterruptResult {
    pub(crate) fn refused(
        kind: AgentControlRefusalKind,
        message: impl Into<String>,
        current_operation_id: Option<OperationId>,
    ) -> Self {
        Self::Refused {
            refusal: AgentControlRefusal {
                kind,
                message: message.into(),
                current_operation_id,
            },
        }
    }

    /// True when MCP should mark this as a tool execution error.
    pub fn is_error(&self) -> bool {
        matches!(self, Self::Refused { .. })
    }

    /// Human-facing text paired with the typed structured result.
    pub fn display_text(&self) -> String {
        match self {
            Self::Settled {
                settlement,
                cancellation_requested,
            } => {
                let action = if *cancellation_requested {
                    "interrupted"
                } else {
                    "already settled"
                };
                format!(
                    "operation {} {action} as {:?}",
                    settlement.operation_id.get(),
                    settlement.state
                )
                .to_lowercase()
            }
            Self::Refused { refusal } => refusal.message.clone(),
        }
    }
}

/// Result of permanently shutting down the logical agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum AgentShutdownResult {
    Stopped {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        drained: Option<OperationSettlement>,
    },
}

impl AgentShutdownResult {
    /// Human-facing text paired with the typed structured result.
    pub fn display_text(&self) -> &'static str {
        "agent stopped"
    }
}

/// Typed application-level refusal returned as an MCP tool error result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AgentRefusal {
    pub kind: AgentRefusalKind,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_operation_id: Option<OperationId>,
}

/// Terminal or refused result of `agent.turn`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum AgentTurnResult {
    Completed {
        operation_id: OperationId,
        run: Box<CompletedTurn>,
    },
    Failed {
        operation_id: OperationId,
        run: Box<FailedTurn>,
    },
    Cancelled {
        operation_id: OperationId,
        run: Box<CancelledTurn>,
    },
    Refused {
        refusal: AgentRefusal,
    },
}

impl AgentTurnResult {
    pub(crate) fn terminal(
        operation_id: OperationId,
        snapshot: roba_core::RunSnapshot,
        configuration: AgentConfiguration,
    ) -> Self {
        let metadata = TurnMetadata::from_snapshot(&snapshot, configuration);
        match snapshot.state {
            roba_core::RunState::Completed => match snapshot.last_outcome {
                Some(outcome) => Self::Completed {
                    operation_id,
                    run: Box::new(CompletedTurn {
                        metadata,
                        outcome: outcome.into(),
                    }),
                },
                None => Self::internal_failure(
                    operation_id,
                    metadata,
                    None,
                    "completed finite run returned no outcome",
                ),
            },
            roba_core::RunState::Failed => match snapshot.failure {
                Some(failure) => Self::Failed {
                    operation_id,
                    run: Box::new(FailedTurn {
                        metadata,
                        last_outcome: snapshot.last_outcome.map(Into::into),
                        failure: failure.into(),
                    }),
                },
                None => Self::internal_failure(
                    operation_id,
                    metadata,
                    snapshot.last_outcome.map(Into::into),
                    "failed finite run returned no failure",
                ),
            },
            roba_core::RunState::Cancelled => Self::Cancelled {
                operation_id,
                run: Box::new(CancelledTurn {
                    metadata,
                    last_outcome: snapshot.last_outcome.map(Into::into),
                }),
            },
            _ => Self::internal_failure(
                operation_id,
                metadata,
                snapshot.last_outcome.map(Into::into),
                "finite run returned a non-terminal snapshot",
            ),
        }
    }

    fn internal_failure(
        operation_id: OperationId,
        metadata: TurnMetadata,
        last_outcome: Option<TurnOutcome>,
        message: impl Into<String>,
    ) -> Self {
        Self::Failed {
            operation_id,
            run: Box::new(FailedTurn {
                metadata,
                last_outcome,
                failure: TurnFailure {
                    kind: FailureKind::Provider,
                    message: message.into(),
                    details: FailureDetails::default(),
                },
            }),
        }
    }

    pub(crate) fn invalid_provider_result(
        operation_id: OperationId,
        mut snapshot: roba_core::RunSnapshot,
        message: String,
        configuration: AgentConfiguration,
    ) -> Self {
        let metadata = TurnMetadata::from_snapshot(&snapshot, configuration);
        if let Some(outcome) = &mut snapshot.last_outcome {
            outcome.session = None;
            outcome.cost = None;
        }
        let mut details: FailureDetails = snapshot
            .failure
            .take()
            .map(|failure| failure.details.into())
            .unwrap_or_default();
        details.session = None;
        details.cost = None;
        Self::Failed {
            operation_id,
            run: Box::new(FailedTurn {
                metadata,
                last_outcome: snapshot.last_outcome.map(Into::into),
                failure: TurnFailure {
                    kind: FailureKind::Provider,
                    message,
                    details,
                },
            }),
        }
    }

    pub(crate) fn refused(
        kind: AgentRefusalKind,
        message: impl Into<String>,
        active_operation_id: Option<OperationId>,
    ) -> Self {
        Self::Refused {
            refusal: AgentRefusal {
                kind,
                message: message.into(),
                active_operation_id,
            },
        }
    }

    /// True when MCP should mark this as a tool execution error.
    pub fn is_error(&self) -> bool {
        !matches!(self, Self::Completed { .. })
    }

    /// Human-facing text content paired with the typed structured result.
    pub fn display_text(&self) -> &str {
        match self {
            Self::Completed { run, .. } => &run.outcome.output,
            Self::Failed { run, .. } => &run.failure.message,
            Self::Cancelled { .. } => "agent turn was cancelled",
            Self::Refused { refusal } => &refusal.message,
        }
    }

    /// Operation identity, when provider work was admitted.
    pub fn operation_id(&self) -> Option<OperationId> {
        match self {
            Self::Completed { operation_id, .. }
            | Self::Failed { operation_id, .. }
            | Self::Cancelled { operation_id, .. } => Some(*operation_id),
            Self::Refused { .. } => None,
        }
    }

    pub(crate) fn without_session_evidence(&self) -> Self {
        let mut redacted = self.clone();
        match &mut redacted {
            Self::Completed { run, .. } => run.outcome.session = None,
            Self::Failed { run, .. } => {
                if let Some(outcome) = &mut run.last_outcome {
                    outcome.session = None;
                }
                run.failure.details.session = None;
            }
            Self::Cancelled { run, .. } => {
                if let Some(outcome) = &mut run.last_outcome {
                    outcome.session = None;
                }
            }
            Self::Refused { .. } => {}
        }
        redacted
    }
}

/// Provider conversation identity safe to return to local clients.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SessionHandle {
    pub provider: String,
    pub id: String,
}

impl From<roba_core::SessionHandle> for SessionHandle {
    fn from(value: roba_core::SessionHandle) -> Self {
        Self {
            provider: value.provider.to_string(),
            id: value.id,
        }
    }
}

/// Timing and accounting common to every admitted terminal turn.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TurnMetadata {
    /// Effective provider settings and limits used for this admitted operation.
    pub configuration: AgentConfiguration,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at_unix_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at_unix_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at_unix_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub elapsed_ms: Option<u64>,
    pub turns_completed: u32,
}

impl TurnMetadata {
    fn from_snapshot(value: &roba_core::RunSnapshot, configuration: AgentConfiguration) -> Self {
        Self {
            configuration,
            created_at_unix_ms: value.created_at_unix_ms,
            started_at_unix_ms: value.started_at_unix_ms,
            finished_at_unix_ms: value.finished_at_unix_ms,
            elapsed_ms: value.elapsed_ms,
            turns_completed: value.turns_completed,
        }
    }
}

/// A completed turn. Its variant and required outcome encode terminal state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CompletedTurn {
    #[serde(flatten)]
    pub metadata: TurnMetadata,
    pub outcome: TurnOutcome,
}

/// A failed operation. A prior successful follow-up turn may also be present.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct FailedTurn {
    #[serde(flatten)]
    pub metadata: TurnMetadata,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_outcome: Option<TurnOutcome>,
    pub failure: TurnFailure,
}

/// A cancelled operation. Earlier completed follow-up turns remain observable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CancelledTurn {
    #[serde(flatten)]
    pub metadata: TurnMetadata,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_outcome: Option<TurnOutcome>,
}

/// Successful provider outcome for one finite turn.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TurnOutcome {
    pub output: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<SessionHandle>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<TokenUsage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<Cost>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_turns: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured_output: Option<serde_json::Value>,
}

impl From<roba_core::RunOutcome> for TurnOutcome {
    fn from(value: roba_core::RunOutcome) -> Self {
        Self {
            output: value.output,
            session: value.session.map(Into::into),
            usage: value.usage.map(Into::into),
            cost: value.cost.map(Into::into),
            duration_ms: value.duration_ms,
            provider_turns: value.provider_turns,
            structured_output: value.structured_output,
        }
    }
}

/// Optional token buckets reported by a provider.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TokenUsage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cached_input: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_input: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_output: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total: Option<u64>,
}

impl From<roba_core::TokenUsage> for TokenUsage {
    fn from(value: roba_core::TokenUsage) -> Self {
        Self {
            input: value.input,
            cached_input: value.cached_input,
            cache_write_input: value.cache_write_input,
            output: value.output,
            reasoning_output: value.reasoning_output,
            total: value.total,
        }
    }
}

/// Provider-reported monetary cost.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Cost {
    pub currency: String,
    pub amount: f64,
}

impl From<roba_core::Cost> for Cost {
    fn from(value: roba_core::Cost) -> Self {
        Self {
            currency: value.currency,
            amount: value.amount,
        }
    }
}

/// Portable category for a provider or lifecycle failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FailureKind {
    Authentication,
    Timeout,
    Budget,
    MaxTurns,
    MaxCost,
    Limit,
    Cancelled,
    Unsupported,
    Provider,
}

impl From<roba_core::FailureKind> for FailureKind {
    fn from(value: roba_core::FailureKind) -> Self {
        match value {
            roba_core::FailureKind::Authentication => Self::Authentication,
            roba_core::FailureKind::Timeout => Self::Timeout,
            roba_core::FailureKind::Budget => Self::Budget,
            roba_core::FailureKind::MaxTurns => Self::MaxTurns,
            roba_core::FailureKind::MaxCost => Self::MaxCost,
            roba_core::FailureKind::Limit => Self::Limit,
            roba_core::FailureKind::Cancelled => Self::Cancelled,
            roba_core::FailureKind::Unsupported => Self::Unsupported,
            roba_core::FailureKind::Provider => Self::Provider,
        }
    }
}

/// Terminal failure returned by one finite run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TurnFailure {
    pub kind: FailureKind,
    pub message: String,
    pub details: FailureDetails,
}

impl From<roba_core::RunFailure> for TurnFailure {
    fn from(value: roba_core::RunFailure) -> Self {
        Self {
            kind: value.kind.into(),
            message: value.message,
            details: value.details.into(),
        }
    }
}

/// Provider evidence retained with a failed finite turn.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct FailureDetails {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<SessionHandle>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<TokenUsage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<Cost>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_turns: Option<u32>,
}

impl From<roba_core::RunFailureDetails> for FailureDetails {
    fn from(value: roba_core::RunFailureDetails) -> Self {
        Self {
            session: value.session.map(Into::into),
            usage: value.usage.map(Into::into),
            cost: value.cost.map(Into::into),
            duration_ms: value.duration_ms,
            provider_turns: value.provider_turns,
        }
    }
}
