//! Private, operation-scoped provider callback binding.

use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use roba_core::{ProviderLaunchContext, ProviderMcpEndpoint, ProviderMcpEndpointError, RunHandle};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tower_mcp::auth::{AuthError, AuthInfo, AuthLayer, AuthResult, Validate};
use tower_mcp::transport::HttpTransport;
use uuid::Uuid;

use crate::{AgentInstance, OperationId};

/// Stable provider-native name for the private Roba MCP server.
pub const PROVIDER_MCP_SERVER_NAME: &str = "roba";

const MCP_PATH: &str = "/mcp";
const MAX_PROVIDER_REQUEST_BYTES: usize = 64 * 1024;
const ENDPOINT_DRAIN_TIMEOUT: Duration = Duration::from_secs(5);

/// A private endpoint and its operation-scoped shutdown capability.
pub(crate) struct ProviderEndpoint {
    state: Arc<EndpointState>,
    task: Mutex<Option<JoinHandle<std::io::Result<()>>>>,
}

impl ProviderEndpoint {
    /// Bind an authenticated loopback endpoint before provider work starts.
    pub(crate) async fn start(
        agent: AgentInstance,
        operation_id: OperationId,
    ) -> Result<(Self, ProviderLaunchContext), ProviderEndpointError> {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .map_err(ProviderEndpointError::Bind)?;
        let address = listener
            .local_addr()
            .map_err(ProviderEndpointError::LocalAddress)?;
        // Two independent v4 UUIDs provide roughly 244 random bits while
        // keeping the token header-safe without another encoding dependency.
        let token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
        let credential = OperationCredential::new(token.clone(), operation_id);
        let (shutdown, receiver) = oneshot::channel();
        let state = Arc::new(EndpointState {
            credential: credential.clone(),
            shutdown: Mutex::new(Some(shutdown)),
        });

        let mut tool_names = agent.extensions().provider_tool_names().to_vec();
        tool_names.push(crate::ROBA_SELF_TOOL.to_owned());
        tool_names.push(crate::ROBA_CONTEXT_MANIFEST_TOOL.to_owned());
        tool_names.push(crate::ROBA_CONTEXT_READ_TOOL.to_owned());
        tool_names.sort_unstable();
        tool_names.dedup();
        let app = HttpTransport::new(crate::router::agent_router(agent, operation_id))
            .max_body_size(MAX_PROVIDER_REQUEST_BYTES)
            .into_router_at(MCP_PATH)
            .layer(AuthLayer::new(credential));
        let task = tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    let _ = receiver.await;
                })
                .await
        });
        let url = format!("http://{address}{MCP_PATH}");
        let endpoint = ProviderMcpEndpoint::new(PROVIDER_MCP_SERVER_NAME, url, token)
            .and_then(|endpoint| endpoint.try_with_tool_names(tool_names))
            .map_err(ProviderEndpointError::Configuration)?;
        let launch_context = ProviderLaunchContext::default()
            .try_with_mcp_endpoint(endpoint)
            .map_err(ProviderEndpointError::Configuration)?;

        Ok((
            Self {
                state,
                task: Mutex::new(Some(task)),
            },
            launch_context,
        ))
    }

    /// Ensure transport teardown is independent of the agent settlement task.
    ///
    /// The watcher owns only the run handle and endpoint shutdown state. The
    /// HTTP router holds a weak agent reference, so this cannot create an
    /// `AgentInstance` -> endpoint -> server task -> `AgentInstance` cycle.
    pub(crate) fn close_when_run_settles(&self, handle: RunHandle) {
        let state = Arc::clone(&self.state);
        tokio::spawn(async move {
            let _ = handle.wait().await;
            state.close();
        });
    }

    /// Revoke the credential, stop accepting requests, and drain the server.
    pub(crate) async fn shutdown(&self) {
        self.state.close();
        let task = self
            .task
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        let Some(mut task) = task else {
            return;
        };
        if tokio::time::timeout(ENDPOINT_DRAIN_TIMEOUT, &mut task)
            .await
            .is_err()
        {
            task.abort();
            let _ = task.await;
        }
    }
}

impl Drop for ProviderEndpoint {
    fn drop(&mut self) {
        self.state.close();
    }
}

struct EndpointState {
    credential: OperationCredential,
    shutdown: Mutex<Option<oneshot::Sender<()>>>,
}

impl EndpointState {
    fn close(&self) {
        self.credential.expire();
        if let Some(shutdown) = self
            .shutdown
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        {
            let _ = shutdown.send(());
        }
    }
}

/// Header validator whose diagnostics never reveal credential material.
#[derive(Clone)]
struct OperationCredential {
    expected: Arc<str>,
    operation_id: OperationId,
    live: Arc<AtomicBool>,
}

impl OperationCredential {
    fn new(expected: String, operation_id: OperationId) -> Self {
        Self {
            expected: expected.into(),
            operation_id,
            live: Arc::new(AtomicBool::new(true)),
        }
    }

    fn expire(&self) {
        self.live.store(false, Ordering::Release);
    }
}

impl fmt::Debug for OperationCredential {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OperationCredential")
            .field("expected", &"[REDACTED]")
            .field("operation_id", &self.operation_id)
            .field("live", &self.live.load(Ordering::Acquire))
            .finish()
    }
}

impl Validate for OperationCredential {
    async fn validate(&self, supplied: &str) -> AuthResult {
        if self.live.load(Ordering::Acquire)
            && constant_time_equal(self.expected.as_bytes(), supplied.as_bytes())
        {
            AuthResult::Authenticated(Some(AuthInfo {
                client_id: format!("provider-operation-{}", self.operation_id.get()),
                claims: None,
            }))
        } else {
            AuthResult::Failed(AuthError {
                code: "invalid_or_expired_provider_credential".to_owned(),
                message: "The provider credential is invalid or expired".to_owned(),
            })
        }
    }
}

fn constant_time_equal(expected: &[u8], supplied: &[u8]) -> bool {
    let mut difference = expected.len() ^ supplied.len();
    for (index, expected_byte) in expected.iter().enumerate() {
        let supplied_byte = supplied.get(index).copied().unwrap_or_default();
        difference |= usize::from(expected_byte ^ supplied_byte);
    }
    difference == 0
}

/// Failure to create the private provider callback endpoint.
#[derive(Debug)]
pub(crate) enum ProviderEndpointError {
    Bind(std::io::Error),
    LocalAddress(std::io::Error),
    Configuration(ProviderMcpEndpointError),
}

impl fmt::Display for ProviderEndpointError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Bind(error) => write!(formatter, "failed to bind provider MCP endpoint: {error}"),
            Self::LocalAddress(error) => {
                write!(
                    formatter,
                    "failed to inspect provider MCP endpoint: {error}"
                )
            }
            Self::Configuration(error) => {
                write!(formatter, "invalid provider MCP endpoint: {error}")
            }
        }
    }
}

impl std::error::Error for ProviderEndpointError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Bind(error) | Self::LocalAddress(error) => Some(error),
            Self::Configuration(error) => Some(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credential_comparison_requires_an_exact_value() {
        assert!(constant_time_equal(b"secret", b"secret"));
        assert!(!constant_time_equal(b"secret", b"secreu"));
        assert!(!constant_time_equal(b"secret", b"secret-extra"));
        assert!(!constant_time_equal(b"secret", b"secre"));
    }

    #[tokio::test]
    async fn validator_debug_is_redacted_and_expiry_fails_closed() {
        let validator = OperationCredential::new("secret-value".to_owned(), OperationId::new(7));
        assert!(matches!(
            validator.validate("secret-value").await,
            AuthResult::Authenticated(_)
        ));
        assert!(!format!("{validator:?}").contains("secret-value"));

        validator.expire();
        assert!(matches!(
            validator.validate("secret-value").await,
            AuthResult::Failed(_)
        ));
    }
}
