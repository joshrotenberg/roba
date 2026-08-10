//! Provider execution boundary.

use std::fmt;
use std::future::Future;
use std::pin::Pin;

use serde::{Deserialize, Serialize};

use crate::lifecycle::WorkerControl;
use crate::run::{FailureKind, ProviderId, RunEvent, RunFailureDetails, RunOutcome, TurnRequest};

/// One ephemeral MCP endpoint made available only to the provider turn being
/// executed. Credentials are deliberately omitted from `Debug` output and are
/// never part of a serializable run specification.
#[derive(Clone, PartialEq, Eq)]
pub struct ProviderMcpEndpoint {
    name: String,
    url: String,
    bearer_token: String,
}

impl ProviderMcpEndpoint {
    /// Construct an endpoint after its listener and authentication boundary
    /// are ready.
    pub fn new(
        name: impl Into<String>,
        url: impl Into<String>,
        bearer_token: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            url: url.into(),
            bearer_token: bearer_token.into(),
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    /// Deliberate secret access for provider adapters configuring the child
    /// process. Callers must not log or persist this value.
    pub fn bearer_token(&self) -> &str {
        &self.bearer_token
    }
}

impl fmt::Debug for ProviderMcpEndpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProviderMcpEndpoint")
            .field("name", &self.name)
            .field("url", &self.url)
            .field("bearer_token", &"[REDACTED]")
            .finish()
    }
}

/// Transient host capabilities for one provider turn.
///
/// The worker control is minted by the lifecycle for the exact run being
/// executed. Middleware may add ephemeral MCP endpoints, but no field is
/// serialized into [`TurnRequest`].
#[derive(Clone, Default)]
pub struct ProviderContext {
    worker_control: Option<WorkerControl>,
    mcp_endpoints: Vec<ProviderMcpEndpoint>,
}

impl ProviderContext {
    pub(crate) fn for_worker(control: WorkerControl) -> Self {
        Self {
            worker_control: Some(control),
            mcp_endpoints: Vec::new(),
        }
    }

    /// Narrow worker capability bound to this provider's current run.
    pub fn worker_control(&self) -> Option<&WorkerControl> {
        self.worker_control.as_ref()
    }

    /// Ephemeral MCP endpoints a provider adapter must attach to this turn.
    pub fn mcp_endpoints(&self) -> &[ProviderMcpEndpoint] {
        &self.mcp_endpoints
    }

    /// Add an already-running endpoint. This is intended for provider
    /// middleware; it does not change the underlying run authority.
    pub fn with_mcp_endpoint(mut self, endpoint: ProviderMcpEndpoint) -> Self {
        self.mcp_endpoints.push(endpoint);
        self
    }
}

impl fmt::Debug for ProviderContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProviderContext")
            .field("worker_control", &self.worker_control.is_some())
            .field("mcp_endpoints", &self.mcp_endpoints)
            .finish()
    }
}

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

/// Synchronous event receiver used by provider adapters while a turn runs.
/// Implementations should return quickly and perform blocking work elsewhere.
pub trait EventSink: Send + Sync {
    fn emit(&self, event: RunEvent);
}

/// Event sink for callers that need only the terminal outcome.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoopEventSink;

impl EventSink for NoopEventSink {
    fn emit(&self, _event: RunEvent) {}
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
    fn execute<'a>(
        &'a self,
        request: TurnRequest,
        context: ProviderContext,
        events: &'a dyn EventSink,
    ) -> ProviderFuture<'a>;
}

/// Validate and execute one request through a provider.
pub async fn execute_turn(
    provider: &dyn Provider,
    request: TurnRequest,
    context: ProviderContext,
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
    provider.execute(request, context, events).await
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
            _context: ProviderContext,
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

        let error = execute_turn(
            &provider,
            request,
            ProviderContext::default(),
            &NoopEventSink,
        )
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

        let outcome = execute_turn(
            &provider,
            request,
            ProviderContext::default(),
            &NoopEventSink,
        )
        .await
        .unwrap();
        assert_eq!(outcome.output, "hello");
        assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    }
}
