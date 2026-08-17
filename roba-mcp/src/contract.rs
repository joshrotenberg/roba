//! Typed MCP values owned by the agent-host boundary.

use serde::{Deserialize, Serialize};
use tower_mcp::schemars::{self, JsonSchema};

/// Monotonic identity for one submitted agent turn.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(transparent)]
pub struct OperationId(u64);

impl OperationId {
    pub(crate) fn new(value: u64) -> Self {
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
    pub state: AgentState,
    pub session_available: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_operation_id: Option<OperationId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_turn: Option<AgentTurnResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at_unix_ms: Option<u64>,
}

/// Why a turn was refused before provider work began.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AgentRefusalKind {
    InvalidPrompt,
    Busy,
    Stopped,
    Runtime,
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
    pub(crate) fn terminal(operation_id: OperationId, snapshot: roba_core::RunSnapshot) -> Self {
        let metadata = TurnMetadata::from(&snapshot);
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
    ) -> Self {
        let metadata = TurnMetadata::from(&snapshot);
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

impl From<&roba_core::RunSnapshot> for TurnMetadata {
    fn from(value: &roba_core::RunSnapshot) -> Self {
        Self {
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

/// A failed turn. A prior successful steering turn may also be present.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct FailedTurn {
    #[serde(flatten)]
    pub metadata: TurnMetadata,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_outcome: Option<TurnOutcome>,
    pub failure: TurnFailure,
}

/// A cancelled turn. Earlier completed steering turns remain observable.
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
