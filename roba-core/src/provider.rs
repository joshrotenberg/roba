//! Provider execution boundary.

use std::fmt;
use std::future::Future;
use std::pin::Pin;

use serde::{Deserialize, Serialize};

use crate::run::{FailureKind, ProviderId, RunFailureDetails, RunOutcome, TokenUsage, TurnRequest};

/// Capabilities whose absence must cause pre-spawn refusal when requested.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderCapabilities {
    pub resume: bool,
    pub streaming: bool,
    pub read_only: bool,
    pub workspace_write: bool,
    pub full_auto: bool,
    pub max_turns: bool,
    pub max_cost: bool,
    pub timeout: bool,
}

/// A normalized provider failure with a stable category.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderError {
    pub kind: FailureKind,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Box<RunFailureDetails>>,
}

impl ProviderError {
    /// Construct a provider error.
    pub fn new(kind: FailureKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            details: None,
        }
    }

    /// Attach provider-reported terminal details to this failure.
    pub fn with_details(mut self, details: RunFailureDetails) -> Self {
        self.details = (!details.is_empty()).then(|| Box::new(details));
        self
    }

    /// Construct a refusal for a setting the provider cannot honor.
    pub fn unsupported(message: impl Into<String>) -> Self {
        Self::new(FailureKind::Unsupported, message)
    }
}

impl fmt::Display for ProviderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ProviderError {}

/// Incremental observation owned by a provider adapter.
///
/// Lifecycle events such as turn boundaries, state changes, steering, and
/// terminal failure are deliberately absent. The run driver emits those from
/// authoritative control state instead of trusting an adapter to do so.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProviderEvent {
    OutputDelta { text: String },
    Usage { usage: TokenUsage },
    Warning { message: String },
}

/// Synchronous receiver for provider-owned events while a turn runs.
/// Implementations should return quickly and perform blocking work elsewhere.
pub trait EventSink: Send + Sync {
    fn emit(&self, event: ProviderEvent);
}

/// Event sink for callers that need only the terminal outcome.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoopEventSink;

impl EventSink for NoopEventSink {
    fn emit(&self, _event: ProviderEvent) {}
}

/// Boxed provider future keeps the trait object-safe without a macro runtime.
pub type ProviderFuture<'a> =
    Pin<Box<dyn Future<Output = Result<RunOutcome, ProviderError>> + Send + 'a>>;

/// One fresh or resumed provider turn.
pub trait Provider: Send + Sync {
    /// Stable provider id.
    fn id(&self) -> ProviderId;

    /// Declared capabilities used for inspection and preflight.
    fn capabilities(&self) -> ProviderCapabilities;

    /// Validate the complete request before any provider child is spawned.
    fn validate(&self, request: &TurnRequest) -> Result<(), ProviderError>;

    /// Execute a request that has passed [`Provider::validate`]. Adapters
    /// should still call their own validation when invoked directly.
    fn execute<'a>(&'a self, request: TurnRequest, events: &'a dyn EventSink)
    -> ProviderFuture<'a>;
}

/// Validate and execute one request through a provider.
pub async fn execute_turn(
    provider: &dyn Provider,
    request: TurnRequest,
    events: &dyn EventSink,
) -> Result<RunOutcome, ProviderError> {
    if request.spec.agent.provider != provider.id() {
        return Err(ProviderError::unsupported(format!(
            "run selects provider {}, but adapter is {}",
            request.spec.agent.provider,
            provider.id()
        )));
    }
    provider.validate(&request)?;
    provider.execute(request, events).await
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::run::{AgentSpec, Prompt, RunSpec};

    struct FakeProvider {
        calls: AtomicUsize,
    }

    impl Provider for FakeProvider {
        fn id(&self) -> ProviderId {
            ProviderId::claude()
        }

        fn capabilities(&self) -> ProviderCapabilities {
            ProviderCapabilities::default()
        }

        fn validate(&self, _request: &TurnRequest) -> Result<(), ProviderError> {
            Ok(())
        }

        fn execute<'a>(
            &'a self,
            request: TurnRequest,
            _events: &'a dyn EventSink,
        ) -> ProviderFuture<'a> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move {
                Ok(RunOutcome {
                    output: request.prompt.into_inner(),
                    session: None,
                    usage: None,
                    cost: None,
                    duration_ms: None,
                    provider_turns: None,
                    structured_output: None,
                })
            })
        }
    }

    #[tokio::test]
    async fn provider_mismatch_refuses_before_execute() {
        let provider = FakeProvider {
            calls: AtomicUsize::new(0),
        };
        let request = RunSpec::suspended(AgentSpec::new(ProviderId::codex()))
            .with_prompt(Prompt::new("hello").unwrap())
            .into_turn()
            .unwrap();

        let error = execute_turn(&provider, request, &NoopEventSink)
            .await
            .unwrap_err();
        assert_eq!(error.kind, FailureKind::Unsupported);
        assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn matching_provider_returns_normalized_outcome() {
        let provider = FakeProvider {
            calls: AtomicUsize::new(0),
        };
        let request = RunSpec::suspended(AgentSpec::new(ProviderId::claude()))
            .with_prompt(Prompt::new("hello").unwrap())
            .into_turn()
            .unwrap();

        let outcome = execute_turn(&provider, request, &NoopEventSink)
            .await
            .unwrap();
        assert_eq!(outcome.output, "hello");
        assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    }
}
