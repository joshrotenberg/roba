//! Provider execution boundary.

use std::fmt;
use std::future::Future;
use std::pin::Pin;

use serde::{Deserialize, Serialize};

use crate::run::{FailureKind, ProviderId, RunFailureDetails, RunOutcome, TokenUsage, TurnRequest};

/// One ephemeral MCP endpoint made available only to the provider run being
/// executed.
///
/// Credentials are deliberately omitted from [`fmt::Debug`] output and this
/// type does not implement serde. Launch material must never be folded into a
/// serializable [`TurnRequest`], run result, or receipt.
#[derive(Clone, PartialEq, Eq)]
pub struct ProviderMcpEndpoint {
    name: String,
    url: String,
    bearer_token: String,
    tool_names: Vec<String>,
}

/// Invalid provider-native MCP server or tool name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderMcpEndpointError {
    /// The server name cannot be represented safely in provider launch
    /// configuration.
    InvalidServerName(String),
    /// A tool name cannot be represented safely in provider launch
    /// configuration.
    InvalidToolName(String),
    /// Two endpoints would compete for the same provider-native server name.
    DuplicateServerName(String),
}

impl fmt::Display for ProviderMcpEndpointError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidServerName(name) => {
                write!(formatter, "invalid provider MCP server name: {name:?}")
            }
            Self::InvalidToolName(name) => {
                write!(formatter, "invalid provider MCP tool name: {name:?}")
            }
            Self::DuplicateServerName(name) => {
                write!(formatter, "duplicate provider MCP server name: {name:?}")
            }
        }
    }
}

impl std::error::Error for ProviderMcpEndpointError {}

/// Whether a name is safe for MCP discovery and provider-native allowlists.
///
/// MCP capability names are one to 128 ASCII letters, digits, underscores,
/// hyphens, or dots.
pub fn is_valid_provider_mcp_name(value: &str) -> bool {
    (1..=128).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
}

fn is_valid_provider_mcp_server_name(value: &str) -> bool {
    is_valid_provider_mcp_name(value) && !value.contains('.')
}

impl ProviderMcpEndpoint {
    /// Construct an endpoint after its listener and authentication boundary
    /// are ready.
    pub fn new(
        name: impl Into<String>,
        url: impl Into<String>,
        bearer_token: impl Into<String>,
    ) -> Result<Self, ProviderMcpEndpointError> {
        let name = name.into();
        // Provider CLIs address transient servers through dotted config paths.
        // Keep this segment representable as a bare key; tool names retain
        // the wider MCP grammar and may contain dots.
        if !is_valid_provider_mcp_server_name(&name) {
            return Err(ProviderMcpEndpointError::InvalidServerName(name));
        }
        Ok(Self {
            name,
            url: url.into(),
            bearer_token: bearer_token.into(),
            tool_names: Vec::new(),
        })
    }

    /// Advertise the exact tools the provider adapter may approve.
    ///
    /// Names are sorted and deduplicated so provider-native launch
    /// configuration is deterministic. An endpoint with no advertised tools
    /// remains attached but grants no tool approvals.
    pub fn try_with_tool_names<I, S>(
        mut self,
        tool_names: I,
    ) -> Result<Self, ProviderMcpEndpointError>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        for tool_name in tool_names {
            let tool_name = tool_name.into();
            if !is_valid_provider_mcp_name(&tool_name) {
                return Err(ProviderMcpEndpointError::InvalidToolName(tool_name));
            }
            self.tool_names.push(tool_name);
        }
        self.tool_names.sort_unstable();
        self.tool_names.dedup();
        Ok(self)
    }

    /// Stable server name exposed to the provider.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Ephemeral host-local endpoint URL.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Deliberate secret access for an adapter configuring its child process.
    /// Callers must not log or persist this value.
    pub fn bearer_token(&self) -> &str {
        &self.bearer_token
    }

    /// Exact endpoint-local tool names the provider adapter may approve.
    pub fn tool_names(&self) -> &[String] {
        &self.tool_names
    }
}

impl fmt::Debug for ProviderMcpEndpoint {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProviderMcpEndpoint")
            .field("name", &self.name)
            .field("url", &self.url)
            .field("bearer_token", &"[REDACTED]")
            .field("tool_names", &self.tool_names)
            .finish()
    }
}

/// Transient, non-serializable launch material for one finite provider run.
///
/// The same context is reused for resumed provider turns within that run. A
/// higher-level host may mint a fresh context for the next finite run.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct ProviderLaunchContext {
    mcp_endpoints: Vec<ProviderMcpEndpoint>,
    bootstrap_instruction: Option<String>,
}

impl ProviderLaunchContext {
    /// Ephemeral MCP endpoints an adapter must attach to this provider run.
    pub fn mcp_endpoints(&self) -> &[ProviderMcpEndpoint] {
        &self.mcp_endpoints
    }

    /// Minimal host instruction needed before the provider can use transient
    /// launch capabilities.
    ///
    /// This material is deliberately non-serializable and redacted from
    /// [`fmt::Debug`]. Higher layers should keep it small and put substantive
    /// context behind the attached MCP contract.
    pub fn bootstrap_instruction(&self) -> Option<&str> {
        self.bootstrap_instruction.as_deref()
    }

    /// Attach one minimal provider-launch bootstrap instruction.
    pub fn with_bootstrap_instruction(mut self, instruction: impl Into<String>) -> Self {
        self.bootstrap_instruction = Some(instruction.into());
        self
    }

    /// Add an already-running endpoint without changing serializable run
    /// intent or authority.
    pub fn try_with_mcp_endpoint(
        mut self,
        endpoint: ProviderMcpEndpoint,
    ) -> Result<Self, ProviderMcpEndpointError> {
        if self
            .mcp_endpoints
            .iter()
            .any(|existing| existing.name == endpoint.name)
        {
            return Err(ProviderMcpEndpointError::DuplicateServerName(endpoint.name));
        }
        self.mcp_endpoints.push(endpoint);
        Ok(self)
    }
}

impl fmt::Debug for ProviderLaunchContext {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProviderLaunchContext")
            .field("mcp_endpoints", &self.mcp_endpoints)
            .field(
                "bootstrap_instruction",
                &self.bootstrap_instruction.as_ref().map(|_| "[REDACTED]"),
            )
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

/// Provider-neutral category for mechanically observed activity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderActivityKind {
    Command,
    FileChange,
    McpCall,
    WebSearch,
    PlanUpdate,
    StatusUpdate,
    ToolCall,
    Unknown,
}

/// Provider-reported terminal disposition for one activity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderActivityStatus {
    Succeeded,
    Failed,
    Cancelled,
    Unknown,
}

/// Incremental observation owned by a provider adapter.
///
/// Lifecycle events such as turn boundaries, state changes, follow-ups, and
/// terminal failure are deliberately absent. The run driver emits those from
/// authoritative control state instead of trusting an adapter to do so.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProviderEvent {
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

    /// Execute with transient host-local launch material.
    ///
    /// Providers that do not need launch material remain source-compatible:
    /// the default delegates to [`Provider::execute`]. Context-aware adapters
    /// override this method while keeping ordinary direct execution as the
    /// empty-context path.
    fn execute_with_launch_context<'a>(
        &'a self,
        request: TurnRequest,
        _launch_context: ProviderLaunchContext,
        events: &'a dyn EventSink,
    ) -> ProviderFuture<'a> {
        self.execute(request, events)
    }
}

/// Validate and execute one request through a provider.
pub async fn execute_turn(
    provider: &dyn Provider,
    request: TurnRequest,
    events: &dyn EventSink,
) -> Result<RunOutcome, ProviderError> {
    execute_turn_with_launch_context(provider, request, ProviderLaunchContext::default(), events)
        .await
}

/// Validate and execute one request with transient provider launch material.
pub async fn execute_turn_with_launch_context(
    provider: &dyn Provider,
    request: TurnRequest,
    launch_context: ProviderLaunchContext,
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
    provider
        .execute_with_launch_context(request, launch_context, events)
        .await
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::run::{AgentSpec, Prompt, RunSpec};

    struct FakeProvider {
        calls: AtomicUsize,
        contexts: Mutex<Vec<ProviderLaunchContext>>,
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

        fn execute_with_launch_context<'a>(
            &'a self,
            request: TurnRequest,
            launch_context: ProviderLaunchContext,
            events: &'a dyn EventSink,
        ) -> ProviderFuture<'a> {
            self.contexts
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(launch_context);
            self.execute(request, events)
        }
    }

    #[tokio::test]
    async fn provider_mismatch_refuses_before_execute() {
        let provider = FakeProvider {
            calls: AtomicUsize::new(0),
            contexts: Mutex::new(Vec::new()),
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
            contexts: Mutex::new(Vec::new()),
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
        assert_eq!(
            provider
                .contexts
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_slice(),
            [ProviderLaunchContext::default()]
        );
    }

    #[tokio::test]
    async fn launch_context_reaches_context_aware_provider_without_secret_debug_output() {
        let provider = FakeProvider {
            calls: AtomicUsize::new(0),
            contexts: Mutex::new(Vec::new()),
        };
        let request = RunSpec::suspended(AgentSpec::new(ProviderId::claude()))
            .with_prompt(Prompt::new("hello").unwrap())
            .into_turn()
            .unwrap();
        let serialized_request = serde_json::to_string(&request).unwrap();
        let context = ProviderLaunchContext::default()
            .try_with_mcp_endpoint(
                ProviderMcpEndpoint::new(
                    "roba",
                    "http://127.0.0.1:4123/mcp",
                    "secret-provider-token",
                )
                .unwrap()
                .try_with_tool_names(["self", "git.snapshot", "self"])
                .unwrap(),
            )
            .unwrap()
            .with_bootstrap_instruction("private bootstrap contract");

        let outcome =
            execute_turn_with_launch_context(&provider, request, context.clone(), &NoopEventSink)
                .await
                .unwrap();

        assert_eq!(outcome.output, "hello");
        assert_eq!(
            provider
                .contexts
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_slice(),
            std::slice::from_ref(&context)
        );
        assert!(!format!("{context:?}").contains("secret-provider-token"));
        assert!(!format!("{context:?}").contains("private bootstrap contract"));
        assert_eq!(
            context.bootstrap_instruction(),
            Some("private bootstrap contract")
        );
        assert_eq!(context.mcp_endpoints()[0].name(), "roba");
        assert_eq!(
            context.mcp_endpoints()[0].url(),
            "http://127.0.0.1:4123/mcp"
        );
        assert_eq!(
            context.mcp_endpoints()[0].bearer_token(),
            "secret-provider-token"
        );
        assert_eq!(
            context.mcp_endpoints()[0].tool_names(),
            ["git.snapshot", "self"]
        );
        assert!(format!("{context:?}").contains("git.snapshot"));
        for private_value in [
            "http://127.0.0.1:4123/mcp",
            "secret-provider-token",
            "git.snapshot",
            "private bootstrap contract",
        ] {
            assert!(!serialized_request.contains(private_value));
        }
    }

    #[test]
    fn endpoint_without_advertised_tools_grants_no_tool_names() {
        let endpoint =
            ProviderMcpEndpoint::new("roba", "http://127.0.0.1:4123/mcp", "secret-provider-token")
                .unwrap();

        assert!(endpoint.tool_names().is_empty());
        assert!(!format!("{endpoint:?}").contains("secret-provider-token"));
    }

    #[test]
    fn endpoint_names_reject_provider_allowlist_injection() {
        assert!(matches!(
            ProviderMcpEndpoint::new(
                "roba.internal",
                "http://127.0.0.1:4123/mcp",
                "secret-provider-token"
            ),
            Err(ProviderMcpEndpointError::InvalidServerName(name)) if name == "roba.internal"
        ));
        assert!(matches!(
            ProviderMcpEndpoint::new(
                "roba,Bash",
                "http://127.0.0.1:4123/mcp",
                "secret-provider-token"
            ),
            Err(ProviderMcpEndpointError::InvalidServerName(name)) if name == "roba,Bash"
        ));
        let endpoint =
            ProviderMcpEndpoint::new("roba", "http://127.0.0.1:4123/mcp", "secret-provider-token")
                .unwrap();
        assert!(matches!(
            endpoint.try_with_tool_names(["git.snapshot", "x,Bash"]),
            Err(ProviderMcpEndpointError::InvalidToolName(name)) if name == "x,Bash"
        ));
        assert!(!is_valid_provider_mcp_name(""));
        assert!(!is_valid_provider_mcp_name(&"x".repeat(129)));

        let first =
            ProviderMcpEndpoint::new("roba", "http://127.0.0.1:4123/mcp", "first-token").unwrap();
        let second =
            ProviderMcpEndpoint::new("roba", "http://127.0.0.1:4124/mcp", "second-token").unwrap();
        let context = ProviderLaunchContext::default()
            .try_with_mcp_endpoint(first)
            .unwrap();
        assert!(matches!(
            context.try_with_mcp_endpoint(second),
            Err(ProviderMcpEndpointError::DuplicateServerName(name)) if name == "roba"
        ));
    }
}
