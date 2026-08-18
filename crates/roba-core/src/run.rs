//! Provider-neutral values for one bounded Roba run.
//!
//! These types describe intent and results. They contain no clap values,
//! terminal presentation, or Claude/Codex wrapper types. Provider adapters
//! must either honor the requested execution policy or refuse it before
//! spawning a child process.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::provider::{ProviderActivityKind, ProviderActivityStatus};

/// Stable provider identity used in resolved specifications and receipts.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProviderId(String);

impl ProviderId {
    /// Claude Code's built-in provider id.
    pub const CLAUDE: &'static str = "claude";
    /// OpenAI Codex's built-in provider id.
    pub const CODEX: &'static str = "codex";

    /// Construct a provider id.
    pub fn new(value: impl Into<String>) -> Result<Self, ProviderIdError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(ProviderIdError);
        }
        Ok(Self(value))
    }

    /// Construct the Claude provider id.
    pub fn claude() -> Self {
        Self(Self::CLAUDE.to_string())
    }

    /// Construct the Codex provider id.
    pub fn codex() -> Self {
        Self(Self::CODEX.to_string())
    }

    /// Return the provider id as text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ProviderId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A provider id was empty or whitespace-only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderIdError;

impl fmt::Display for ProviderIdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("provider id must not be empty")
    }
}

impl std::error::Error for ProviderIdError {}

/// A nonempty user message delivered to the root agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Prompt(String);

impl Prompt {
    /// Construct a prompt without changing its whitespace.
    pub fn new(value: impl Into<String>) -> Result<Self, PromptError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(PromptError);
        }
        Ok(Self(value))
    }

    /// Borrow the prompt text.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consume the prompt and return its text.
    pub fn into_inner(self) -> String {
        self.0
    }
}

/// A prompt was empty or whitespace-only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PromptError;

impl fmt::Display for PromptError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("prompt must not be empty")
    }
}

impl std::error::Error for PromptError {}

/// Provider-neutral reasoning effort.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Effort {
    Low,
    Medium,
    High,
    XHigh,
    Max,
}

/// Stable root-agent configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentSpec {
    /// Provider selected for the root agent.
    pub provider: ProviderId,
    /// Provider model id, or the provider default when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Provider-neutral effort request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<Effort>,
    /// Durable instructions for this named agent.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub instructions: Vec<String>,
}

impl AgentSpec {
    /// Construct an agent using a provider's defaults.
    pub fn new(provider: ProviderId) -> Self {
        Self {
            provider,
            model: None,
            effort: None,
            instructions: Vec::new(),
        }
    }
}

/// Ordered context that Roba composes before the current user message.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextSpec {
    /// Repository or project context shared by runs of the agent.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub project: Vec<String>,
    /// Context specific to this run.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub run: Vec<String>,
}

/// Portable permission posture. A provider must reject a posture it cannot
/// enforce rather than silently weakening it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionPolicy {
    /// Read and inspect only.
    #[default]
    ReadOnly,
    /// Permit workspace edits while retaining provider approval boundaries.
    WorkspaceWrite,
    /// Permit unattended provider operation. Use only in an external sandbox.
    FullAuto,
}

/// Granular tool policy applied after the coarse permission posture.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolPolicy {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allow: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deny: Vec<String>,
}

/// Provider-independent ceilings for a turn.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LimitSpec {
    /// Maximum provider turns.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_turns: Option<u32>,
    /// Maximum provider-reported spend in USD.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_cost_usd: Option<f64>,
    /// Wall-clock deadline in seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
}

/// Opaque provider conversation identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionHandle {
    pub provider: ProviderId,
    pub id: String,
}

/// Conversation continuity for one turn.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SessionSpec {
    #[default]
    Fresh,
    Resume {
        session: SessionHandle,
    },
}

/// Execution policy separate from agent identity and prompt context.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutionSpec {
    #[serde(default)]
    pub permissions: PermissionPolicy,
    #[serde(default)]
    pub tools: ToolPolicy,
    #[serde(default)]
    pub limits: LimitSpec,
    #[serde(default)]
    pub session: SessionSpec,
}

/// Fully inspectable intent for one bounded Roba run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunSpec {
    pub agent: AgentSpec,
    #[serde(default)]
    pub context: ContextSpec,
    #[serde(default)]
    pub execution: ExecutionSpec,
    /// Absence is intentional: the run remains suspended until `start` is
    /// called through a retained run handle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_prompt: Option<Prompt>,
}

impl RunSpec {
    /// Construct a suspended run specification.
    pub fn suspended(agent: AgentSpec) -> Self {
        Self {
            agent,
            context: ContextSpec::default(),
            execution: ExecutionSpec::default(),
            initial_prompt: None,
        }
    }

    /// Attach the first prompt to a suspended specification.
    pub fn with_prompt(mut self, prompt: Prompt) -> Self {
        self.initial_prompt = Some(prompt);
        self
    }

    /// The lifecycle implied before any provider process is started.
    pub fn initial_state(&self) -> RunState {
        if self.initial_prompt.is_some() {
            RunState::Ready
        } else {
            RunState::Suspended
        }
    }

    /// Convert an executable specification into a provider turn request.
    pub fn into_turn(self) -> Result<TurnRequest, RunSpecError> {
        let prompt = self.initial_prompt.clone().ok_or(RunSpecError::Suspended)?;
        Ok(TurnRequest { spec: self, prompt })
    }
}

/// A run specification cannot yet execute.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunSpecError {
    Suspended,
}

impl fmt::Display for RunSpecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Suspended => f.write_str("run is suspended and has no initial prompt"),
        }
    }
}

impl std::error::Error for RunSpecError {}

/// Immutable request handed to a provider adapter.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TurnRequest {
    pub spec: RunSpec,
    pub prompt: Prompt,
}

/// Coarse lifecycle state shared by library callers and thin adapters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    Suspended,
    Ready,
    Running,
    Finishing,
    Completed,
    Failed,
    Cancelled,
}

/// Token buckets reported by a provider. Every bucket is optional because
/// absence means unreported, never zero.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
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

impl TokenUsage {
    /// True when no usage bucket was reported.
    pub fn is_unreported(&self) -> bool {
        self == &Self::default()
    }
}

/// Provider-reported monetary cost.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Cost {
    pub currency: String,
    pub amount: f64,
}

impl Cost {
    /// Construct a USD cost reported by a provider.
    pub fn usd(amount: f64) -> Self {
        Self {
            currency: "USD".to_string(),
            amount,
        }
    }
}

/// Successful terminal outcome from a provider turn.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunOutcome {
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

/// Normalized run event.
///
/// The lifecycle owns turn boundaries, state changes, follow-ups, and terminal
/// failure. Provider adapters can contribute only output, usage, and warning
/// observations through [`crate::provider::ProviderEvent`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RunEvent {
    StateChanged {
        state: RunState,
    },
    TurnStarted {
        provider: ProviderId,
    },
    OutputDelta {
        text: String,
    },
    Usage {
        usage: TokenUsage,
    },
    Warning {
        message: String,
    },
    ActivityStarted {
        id: String,
        activity: ProviderActivityKind,
        summary: String,
    },
    ActivityCompleted {
        id: String,
        activity: ProviderActivityKind,
        status: ProviderActivityStatus,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        duration_ms: Option<u64>,
        summary: String,
    },
    FollowUpQueued,
    FollowUpApplied,
    TurnCompleted {
        outcome: RunOutcome,
    },
    Failed {
        failure: RunFailure,
    },
}

/// Portable failure category. Provider-native details remain in the message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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

/// Provider-reported terminal details retained when a failed turn supplied
/// them. Every field is optional because absence means unreported, never zero.
///
/// Limit terminals commonly carry enough information to resume the provider
/// session and account for the work completed before the boundary. Keeping
/// that evidence beside the failure lets every caller make the same recovery
/// decision without parsing a provider-specific message.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RunFailureDetails {
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

impl RunFailureDetails {
    /// True when the provider reported no terminal recovery or accounting
    /// field. Empty details are omitted from serialized failures.
    pub fn is_empty(&self) -> bool {
        self == &Self::default()
    }
}

/// Terminal failure retained by a run handle.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunFailure {
    pub kind: FailureKind,
    pub message: String,
    #[serde(default, skip_serializing_if = "RunFailureDetails::is_empty")]
    pub details: RunFailureDetails,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_and_provider_ids_reject_empty_values() {
        assert!(Prompt::new(" \n").is_err());
        assert!(ProviderId::new("").is_err());
    }

    #[test]
    fn a_promptless_spec_is_explicitly_suspended() {
        let spec = RunSpec::suspended(AgentSpec::new(ProviderId::claude()));
        assert_eq!(spec.initial_state(), RunState::Suspended);
        assert_eq!(spec.into_turn().unwrap_err(), RunSpecError::Suspended);
    }

    #[test]
    fn a_prompt_makes_the_spec_ready_and_round_trips() {
        let spec = RunSpec::suspended(AgentSpec::new(ProviderId::codex()))
            .with_prompt(Prompt::new("finish the task").unwrap());
        assert_eq!(spec.initial_state(), RunState::Ready);

        let json = serde_json::to_string(&spec).unwrap();
        let back: RunSpec = serde_json::from_str(&json).unwrap();
        let request = back.into_turn().unwrap();
        assert_eq!(request.prompt.as_str(), "finish the task");
        assert_eq!(request.spec.agent.provider.as_str(), ProviderId::CODEX);
    }

    #[test]
    fn removed_mission_and_worker_policy_fail_loudly() {
        let old = serde_json::json!({
            "agent": { "provider": "claude" },
            "mission": { "objective": "do the work" },
            "execution": {
                "workers": { "max_workers": 2 },
                "permissions": "read_only"
            }
        });
        let error = serde_json::from_value::<RunSpec>(old).unwrap_err();
        let message = error.to_string();
        assert!(
            message.contains("unknown field `mission`")
                || message.contains("unknown field `workers`"),
            "unexpected serde error: {message}"
        );
    }

    #[test]
    fn missing_usage_is_not_rendered_as_zero() {
        let usage = TokenUsage::default();
        assert!(usage.is_unreported());
        assert_eq!(serde_json::to_string(&usage).unwrap(), "{}");
    }

    #[test]
    fn failure_details_are_additive_and_omit_unreported_fields() {
        let legacy: RunFailure = serde_json::from_value(serde_json::json!({
            "kind": "provider",
            "message": "failed"
        }))
        .unwrap();
        assert!(legacy.details.is_empty());
        assert_eq!(
            serde_json::to_value(&legacy).unwrap(),
            serde_json::json!({"kind": "provider", "message": "failed"})
        );

        let failure = RunFailure {
            kind: FailureKind::MaxTurns,
            message: "limit reached".to_string(),
            details: RunFailureDetails {
                session: Some(SessionHandle {
                    provider: ProviderId::claude(),
                    id: "session-1".to_string(),
                }),
                usage: None,
                cost: Some(Cost::usd(1.25)),
                duration_ms: None,
                provider_turns: Some(30),
            },
        };
        let json = serde_json::to_value(&failure).unwrap();
        assert_eq!(json["details"]["session"]["id"], "session-1");
        assert_eq!(json["details"]["cost"]["amount"], 1.25);
        assert_eq!(json["details"]["provider_turns"], 30);
        assert!(json["details"].get("usage").is_none());
        assert_eq!(serde_json::from_value::<RunFailure>(json).unwrap(), failure);
    }
}
