//! Run-scoped MCP adapter over [`roba_core::RunHandle`].
//!
//! The adapter owns no execution state. Its tools call the same handle a Rust
//! host or REPL uses, so transport choice cannot change lifecycle semantics.

use std::fmt;
use std::sync::Arc;

use axum::extract::Request;
use axum::http::{StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use schemars::JsonSchema;
use serde::Deserialize;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tower_mcp::transport::GenericStdioTransport;
use tower_mcp::{CallToolResult, HttpTransport, McpRouter, ToolBuilder};
use uuid::Uuid;

use roba_core::{
    FailureKind, Prompt, Provider, ProviderCapabilities, ProviderContext, ProviderError,
    ProviderFuture, ProviderId, ProviderMcpEndpoint, RunHandle, TurnRequest, WorkerControl,
};

const INTERNAL_SERVER_NAME: &str = "roba_workers";

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PromptArgs {
    /// Message for the root Roba agent.
    text: String,
}

/// Build the minimal control and observation surface for one run.
pub fn router(handle: RunHandle) -> McpRouter {
    let start = {
        let handle = handle.clone();
        ToolBuilder::new("start")
            .description("Supply the first prompt to a suspended Roba run.")
            .non_destructive()
            .handler(move |args: PromptArgs| {
                let handle = handle.clone();
                async move {
                    let prompt = match Prompt::new(args.text) {
                        Ok(prompt) => prompt,
                        Err(error) => return Ok(CallToolResult::error(error.to_string())),
                    };
                    match handle.start(prompt).await {
                        Ok(()) => snapshot_result(&handle).await,
                        Err(error) => Ok(CallToolResult::error(error.to_string())),
                    }
                }
            })
            .build()
    };

    let status = {
        let handle = handle.clone();
        ToolBuilder::new("status")
            .description("Inspect the current Roba run state and latest outcome.")
            .read_only()
            .no_params_handler(move || {
                let handle = handle.clone();
                async move { snapshot_result(&handle).await }
            })
            .build()
    };

    let wait = {
        let handle = handle.clone();
        ToolBuilder::new("wait")
            .description("Wait for this Roba run to complete, fail, or be cancelled.")
            .read_only()
            .no_params_handler(move || {
                let handle = handle.clone();
                async move { json_result(handle.wait().await) }
            })
            .build()
    };

    let steer = {
        let handle = handle.clone();
        ToolBuilder::new("steer")
            .description("Queue guidance for the root agent's next safe provider turn boundary.")
            .non_destructive()
            .handler(move |args: PromptArgs| {
                let handle = handle.clone();
                async move {
                    let prompt = match Prompt::new(args.text) {
                        Ok(prompt) => prompt,
                        Err(error) => return Ok(CallToolResult::error(error.to_string())),
                    };
                    match handle.steer(prompt).await {
                        Ok(()) => snapshot_result(&handle).await,
                        Err(error) => Ok(CallToolResult::error(error.to_string())),
                    }
                }
            })
            .build()
    };

    let spawn_worker = {
        let handle = handle.clone();
        ToolBuilder::new("spawn_worker")
            .description(
                "Start a child run with the root agent's provider, context, and execution policy. This may incur provider usage.",
            )
            .non_destructive()
            .handler(move |args: PromptArgs| {
                let handle = handle.clone();
                async move {
                    let prompt = match Prompt::new(args.text) {
                        Ok(prompt) => prompt,
                        Err(error) => return Ok(CallToolResult::error(error.to_string())),
                    };
                    match handle.spawn_inherited(prompt).await {
                        Ok(worker) => snapshot_result(&worker).await,
                        Err(error) => Ok(CallToolResult::error(error.to_string())),
                    }
                }
            })
            .build()
    };

    let workers = {
        let handle = handle.clone();
        ToolBuilder::new("workers")
            .description("List every child run owned by this run, including terminal workers.")
            .read_only()
            .no_params_handler(move || {
                let handle = handle.clone();
                async move { json_result(serde_json::json!({ "workers": handle.workers() })) }
            })
            .build()
    };

    let cancel = ToolBuilder::new("cancel")
        .description("Cancel the active Roba provider turn and end this run.")
        .destructive()
        .no_params_handler(move || {
            let handle = handle.clone();
            async move {
                match handle.cancel().await {
                    Ok(()) => snapshot_result(&handle).await,
                    Err(error) => Ok(CallToolResult::error(error.to_string())),
                }
            }
        })
        .build();

    McpRouter::new()
        .server_info("roba-run", env!("CARGO_PKG_VERSION"))
        .instructions(
            "This MCP server controls one bounded Roba run. Start supplies the first prompt; status, wait, and workers observe it; steer queues root guidance; spawn_worker starts an inherited child within explicit bounds; cancel ends the tree.",
        )
        .tool(start)
        .tool(status)
        .tool(wait)
        .tool(steer)
        .tool(spawn_worker)
        .tool(workers)
        .tool(cancel)
}

/// Serve the run router over stdin/stdout until the client closes the stream.
/// The host remains responsible for the owning process lifetime.
pub async fn serve_stdio(handle: RunHandle) -> tower_mcp::Result<()> {
    GenericStdioTransport::new(router(handle)).run().await
}

/// Provider middleware that gives an executing Claude or Codex turn only the
/// worker capability minted for that exact run.
///
/// When workers are disabled, this delegates without opening a listener.
#[derive(Clone)]
pub struct WorkerMcpProvider<P> {
    inner: P,
}

impl<P> WorkerMcpProvider<P> {
    pub fn new(inner: P) -> Self {
        Self { inner }
    }

    pub fn into_inner(self) -> P {
        self.inner
    }
}

impl<P: fmt::Debug> fmt::Debug for WorkerMcpProvider<P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("WorkerMcpProvider")
            .field(&self.inner)
            .finish()
    }
}

impl<P> Provider for WorkerMcpProvider<P>
where
    P: Provider,
{
    fn id(&self) -> ProviderId {
        self.inner.id()
    }

    fn capabilities(&self) -> ProviderCapabilities {
        self.inner.capabilities()
    }

    fn validate(&self, request: &TurnRequest) -> Result<(), ProviderError> {
        self.inner.validate(request)
    }

    fn execute<'a>(
        &'a self,
        request: TurnRequest,
        context: ProviderContext,
        events: &'a dyn roba_core::EventSink,
    ) -> ProviderFuture<'a> {
        Box::pin(async move {
            let Some(control) = context.worker_control().cloned() else {
                return self.inner.execute(request, context, events).await;
            };
            let server = InternalWorkerServer::start(control)
                .await
                .map_err(|error| {
                    ProviderError::new(
                        FailureKind::Provider,
                        format!("failed to start Roba worker MCP transport: {error}"),
                    )
                })?;
            let context = context.with_mcp_endpoint(server.endpoint.clone());
            let result = self.inner.execute(request, context, events).await;
            server.shutdown().await;
            result
        })
    }
}

struct InternalWorkerServer {
    endpoint: ProviderMcpEndpoint,
    shutdown: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<()>>,
}

impl InternalWorkerServer {
    async fn start(control: WorkerControl) -> std::io::Result<Self> {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
        let address = listener.local_addr()?;
        let token = Uuid::new_v4().simple().to_string();
        let authorization: Arc<str> = format!("Bearer {token}").into();
        let app = HttpTransport::new(worker_router(control))
            .into_router_at("/mcp")
            .layer(middleware::from_fn(move |request: Request, next: Next| {
                require_bearer(authorization.clone(), request, next)
            }));
        let (shutdown, receiver) = oneshot::channel();
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    let _ = receiver.await;
                })
                .await;
        });
        Ok(Self {
            endpoint: ProviderMcpEndpoint::new(
                INTERNAL_SERVER_NAME,
                format!("http://{address}/mcp"),
                token,
            ),
            shutdown: Some(shutdown),
            task: Some(task),
        })
    }

    async fn shutdown(mut self) {
        self.signal_shutdown();
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
    }

    fn signal_shutdown(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
    }
}

impl Drop for InternalWorkerServer {
    fn drop(&mut self) {
        self.signal_shutdown();
    }
}

async fn require_bearer(expected: Arc<str>, request: Request, next: Next) -> Response {
    let supplied = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok());
    if supplied != Some(expected.as_ref()) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    next.run(request).await
}

fn worker_router(control: WorkerControl) -> McpRouter {
    let spawn_worker = {
        let control = control.clone();
        ToolBuilder::new("spawn_worker")
            .description(
                "Start an inherited child run within this run's immutable worker bounds. This may incur provider usage.",
            )
            .non_destructive()
            .handler(move |args: PromptArgs| {
                let control = control.clone();
                async move {
                    let prompt = match Prompt::new(args.text) {
                        Ok(prompt) => prompt,
                        Err(error) => return Ok(CallToolResult::error(error.to_string())),
                    };
                    match control.spawn(prompt).await {
                        Ok(worker) => snapshot_result(&worker).await,
                        Err(error) => Ok(CallToolResult::error(error.to_string())),
                    }
                }
            })
            .build()
    };
    let workers = ToolBuilder::new("workers")
        .description("List descendants owned by this exact provider run.")
        .read_only()
        .no_params_handler(move || {
            let control = control.clone();
            async move {
                match control.workers() {
                    Ok(workers) => json_result(serde_json::json!({ "workers": workers })),
                    Err(error) => Ok(CallToolResult::error(error.to_string())),
                }
            }
        })
        .build();

    McpRouter::new()
        .server_info("roba-worker-control", env!("CARGO_PKG_VERSION"))
        .instructions(
            "This private server can spawn inherited workers and inspect their state. It cannot widen run authority or control unrelated runs.",
        )
        .tool(spawn_worker)
        .tool(workers)
}

async fn snapshot_result(handle: &RunHandle) -> tower_mcp::Result<CallToolResult> {
    json_result(handle.status().await)
}

fn json_result<T: serde::Serialize>(value: T) -> tower_mcp::Result<CallToolResult> {
    Ok(match serde_json::to_value(value) {
        Ok(value) => CallToolResult::json(value),
        Err(error) => CallToolResult::error(format!("failed to serialize run state: {error}")),
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::json;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::sync::Notify;
    use tower_mcp::TestClient;

    use super::*;
    use roba_core::{
        AgentSpec, EventSink, Provider, ProviderCapabilities, ProviderContext, ProviderError,
        ProviderFuture, ProviderId, Run, RunOutcome, RunSpec, SessionHandle, TurnRequest,
        WorkerPolicy,
    };

    struct FakeProvider;

    impl Provider for FakeProvider {
        fn id(&self) -> ProviderId {
            ProviderId::new("fake").unwrap()
        }

        fn capabilities(&self) -> ProviderCapabilities {
            ProviderCapabilities {
                resume: true,
                ..ProviderCapabilities::default()
            }
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
            Box::pin(async move {
                Ok(RunOutcome {
                    output: request.prompt.into_inner(),
                    session: Some(SessionHandle {
                        provider: ProviderId::new("fake").unwrap(),
                        id: "session-1".to_string(),
                    }),
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
    async fn mcp_starts_and_observes_the_same_suspended_run() {
        let run = Run::new(
            RunSpec::suspended(AgentSpec::new(ProviderId::new("fake").unwrap())),
            Arc::new(FakeProvider),
        )
        .unwrap();
        let mut client = TestClient::from_router(router(run.handle()));
        client.initialize().await;

        let before = client.call_tool_json("status", json!({})).await;
        assert_eq!(before["state"], "suspended");

        let started = client
            .call_tool_json("start", json!({"text": "hello"}))
            .await;
        assert!(matches!(
            started["state"].as_str(),
            Some("running" | "completed")
        ));

        let terminal = client.call_tool_json("wait", json!({})).await;
        assert_eq!(terminal["state"], "completed");
        assert_eq!(terminal["last_outcome"]["output"], "hello");
    }

    #[tokio::test]
    async fn mcp_refuses_a_second_start() {
        let run = Run::new(
            RunSpec::suspended(AgentSpec::new(ProviderId::new("fake").unwrap())),
            Arc::new(FakeProvider),
        )
        .unwrap();
        let mut client = TestClient::from_router(router(run.handle()));
        client.initialize().await;
        client.call_tool("start", json!({"text": "hello"})).await;
        let error = client
            .call_tool_expect_error("start", json!({"text": "again"}))
            .await;
        assert!(error.to_string().contains("already started"));
    }

    struct BlockingRootProvider {
        root_started: Notify,
        release_root: Notify,
    }

    impl Provider for BlockingRootProvider {
        fn id(&self) -> ProviderId {
            ProviderId::new("fake").unwrap()
        }

        fn capabilities(&self) -> ProviderCapabilities {
            ProviderCapabilities {
                resume: true,
                ..ProviderCapabilities::default()
            }
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
            Box::pin(async move {
                if request.prompt.as_str() == "root" {
                    self.root_started.notify_one();
                    self.release_root.notified().await;
                }
                Ok(RunOutcome {
                    output: request.prompt.into_inner(),
                    session: Some(SessionHandle {
                        provider: ProviderId::new("fake").unwrap(),
                        id: "session-1".to_string(),
                    }),
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
    async fn mcp_spawns_and_observes_a_bounded_inherited_worker() {
        let provider = Arc::new(BlockingRootProvider {
            root_started: Notify::new(),
            release_root: Notify::new(),
        });
        let mut spec = RunSpec::suspended(AgentSpec::new(ProviderId::new("fake").unwrap()))
            .with_prompt(Prompt::new("root").unwrap());
        spec.execution.workers = WorkerPolicy {
            max_workers: 1,
            max_depth: 1,
        };
        let run = Run::new(spec, provider.clone()).unwrap();
        run.begin().await.unwrap();
        provider.root_started.notified().await;

        let mut client = TestClient::from_router(router(run.handle()));
        client.initialize().await;
        let spawned = client
            .call_tool_json("spawn_worker", json!({"text": "worker"}))
            .await;
        assert_eq!(spawned["parent_id"], 1);
        assert_eq!(spawned["depth"], 1);

        let workers = client.call_tool_json("workers", json!({})).await;
        assert_eq!(workers["workers"].as_array().unwrap().len(), 1);
        assert_eq!(workers["workers"][0]["provider"], "fake");

        let refusal = client
            .call_tool_expect_error("spawn_worker", json!({"text": "second"}))
            .await;
        assert!(refusal.to_string().contains("maximum of 1 workers"));

        provider.release_root.notify_one();
        assert_eq!(
            run.handle().wait().await.state,
            roba_core::RunState::Completed
        );
    }

    #[derive(Debug)]
    struct ProviderThatCallsWorkerMcp;

    async fn health_status(endpoint: &ProviderMcpEndpoint, authenticated: bool) -> String {
        let address_and_path = endpoint.url().strip_prefix("http://").unwrap();
        let (address, path) = address_and_path.split_once('/').unwrap();
        let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
        let authorization = if authenticated {
            format!("Authorization: Bearer {}\r\n", endpoint.bearer_token())
        } else {
            String::new()
        };
        let request = format!(
            "GET /{path}/health HTTP/1.1\r\nHost: {address}\r\n{authorization}Connection: close\r\n\r\n"
        );
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).await.unwrap();
        response.lines().next().unwrap().to_string()
    }

    impl Provider for ProviderThatCallsWorkerMcp {
        fn id(&self) -> ProviderId {
            ProviderId::new("fake").unwrap()
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
            context: ProviderContext,
            _events: &'a dyn EventSink,
        ) -> ProviderFuture<'a> {
            Box::pin(async move {
                if request.prompt.as_str() == "root" {
                    let endpoint = context
                        .mcp_endpoints()
                        .first()
                        .expect("worker middleware attached one endpoint");
                    assert!(!format!("{context:?}").contains(endpoint.bearer_token()));

                    assert!(health_status(endpoint, false).await.contains("401"));
                    assert!(health_status(endpoint, true).await.contains("200"));

                    let control = context.worker_control().unwrap().clone();
                    let mut client = TestClient::from_router(worker_router(control));
                    client.initialize().await;
                    client
                        .call_tool_json("spawn_worker", json!({"text": "worker"}))
                        .await;
                    let workers = client.call_tool_json("workers", json!({})).await;
                    assert_eq!(workers["workers"].as_array().unwrap().len(), 1);
                }
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
    async fn provider_middleware_binds_authenticated_mcp_to_the_exact_run() {
        let mut spec = RunSpec::suspended(AgentSpec::new(ProviderId::new("fake").unwrap()))
            .with_prompt(Prompt::new("root").unwrap());
        spec.execution.workers = WorkerPolicy {
            max_workers: 1,
            max_depth: 1,
        };
        let provider = Arc::new(WorkerMcpProvider::new(ProviderThatCallsWorkerMcp));
        let run = Run::new(spec, provider).unwrap();

        run.begin().await.unwrap();
        assert_eq!(
            run.handle().wait().await.state,
            roba_core::RunState::Completed
        );
        let workers = run.handle().workers();
        assert_eq!(workers.len(), 1);
        assert_eq!(workers[0].parent_id, run.id());
        assert!(workers[0].run.is_terminal());
    }
}
