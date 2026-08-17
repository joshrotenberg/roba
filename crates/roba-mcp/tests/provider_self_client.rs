use std::future::Future;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use hyper::{HeaderMap, Method, Request, StatusCode};
use hyper_util::client::legacy::Client as HyperClient;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;
use roba_core::{
    AgentSpec, EventSink, FailureKind as CoreFailureKind, Provider, ProviderCapabilities,
    ProviderError, ProviderFuture, ProviderId, ProviderLaunchContext, ProviderMcpEndpoint, Roba,
    RunOutcome, RunSpec, SessionHandle as CoreSessionHandle, SessionSpec, TurnRequest,
};
use roba_mcp::{
    AGENT_EVENTS_URI, AGENT_INTERRUPT_TOOL, AGENT_RESOURCE_URI, AGENT_SHUTDOWN_TOOL,
    AGENT_STEER_TOOL, AGENT_TURN_TOOL, AgentInstance, AgentInterruptResult, AgentState,
    AgentTerminalState, AgentTurnResult, OperationId, PROVIDER_MCP_SERVER_NAME,
    ProviderSelfSnapshot, ROBA_SELF_TOOL, agent_router, connect_in_process,
};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tokio::sync::Semaphore;
use tower_mcp::{CallToolResult, ChannelTransport, McpClient};

const TEST_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone)]
struct CallbackObservation {
    tools: Vec<String>,
    resource_count: usize,
    snapshot: ProviderSelfSnapshot,
    result_json: String,
}

struct FakeState {
    calls: AtomicUsize,
    endpoints: Mutex<Vec<ProviderMcpEndpoint>>,
    launch_debug: Mutex<Vec<String>>,
    request_json: Mutex<Vec<String>>,
    callbacks: Mutex<Vec<CallbackObservation>>,
    endpoint_ready: Semaphore,
    callback_ready: Semaphore,
    release: Semaphore,
    dropped: Semaphore,
}

impl Default for FakeState {
    fn default() -> Self {
        Self {
            calls: AtomicUsize::new(0),
            endpoints: Mutex::new(Vec::new()),
            launch_debug: Mutex::new(Vec::new()),
            request_json: Mutex::new(Vec::new()),
            callbacks: Mutex::new(Vec::new()),
            endpoint_ready: Semaphore::new(0),
            callback_ready: Semaphore::new(0),
            release: Semaphore::new(0),
            dropped: Semaphore::new(0),
        }
    }
}

struct ExecutionGuard {
    state: Arc<FakeState>,
    completed: bool,
}

impl Drop for ExecutionGuard {
    fn drop(&mut self) {
        if !self.completed {
            self.state.dropped.add_permits(1);
        }
    }
}

struct CallbackProvider {
    state: Arc<FakeState>,
}

impl Provider for CallbackProvider {
    fn id(&self) -> ProviderId {
        fake_provider_id()
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            resume: true,
            streaming: true,
            read_only: true,
            workspace_write: true,
            full_auto: true,
            timeout: true,
            ..Default::default()
        }
    }

    fn validate(&self, _request: &TurnRequest) -> Result<(), ProviderError> {
        Ok(())
    }

    fn execute<'a>(
        &'a self,
        _request: TurnRequest,
        _events: &'a dyn EventSink,
    ) -> ProviderFuture<'a> {
        Box::pin(async {
            Err(ProviderError::new(
                CoreFailureKind::Provider,
                "test provider requires its launch context",
            ))
        })
    }

    fn execute_with_launch_context<'a>(
        &'a self,
        request: TurnRequest,
        launch_context: ProviderLaunchContext,
        _events: &'a dyn EventSink,
    ) -> ProviderFuture<'a> {
        let state = Arc::clone(&self.state);
        let call = state.calls.fetch_add(1, Ordering::SeqCst) + 1;
        let endpoint = match launch_context.mcp_endpoints() {
            [endpoint] => endpoint.clone(),
            endpoints => {
                let count = endpoints.len();
                return Box::pin(async move {
                    Err(ProviderError::new(
                        CoreFailureKind::Provider,
                        format!("expected one provider MCP endpoint, got {count}"),
                    ))
                });
            }
        };

        state
            .launch_debug
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(format!("{launch_context:?}"));
        state
            .request_json
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(serde_json::to_string(&request).expect("turn request serializes"));
        state
            .endpoints
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(endpoint.clone());
        state.endpoint_ready.add_permits(1);

        Box::pin(async move {
            let mut guard = ExecutionGuard {
                state: Arc::clone(&state),
                completed: false,
            };
            let mut client = initialized_http(&endpoint, Some(endpoint.bearer_token()))
                .await
                .map_err(|error| callback_error("initialize", error))?;
            let tools = client
                .list_tools()
                .await
                .map_err(|error| callback_error("list tools", error))?;
            let resources = client
                .list_resources()
                .await
                .map_err(|error| callback_error("list resources", error))?;
            let result = client
                .call_tool(ROBA_SELF_TOOL, json!({}))
                .await
                .map_err(|error| callback_error("call roba.self", error))?;
            if result.is_error {
                return Err(ProviderError::new(
                    CoreFailureKind::Provider,
                    "roba.self unexpectedly returned an MCP tool error",
                ));
            }
            let snapshot = typed::<ProviderSelfSnapshot>(&result);
            state
                .callbacks
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(CallbackObservation {
                    tools,
                    resource_count: resources,
                    snapshot,
                    result_json: serde_json::to_string(&result)
                        .expect("callback result serializes"),
                });
            state.callback_ready.add_permits(1);

            let prompt = request.prompt.as_str().to_owned();
            if prompt == "hold" {
                state
                    .release
                    .acquire()
                    .await
                    .expect("test release semaphore remains open")
                    .forget();
            }

            let session = match request.spec.execution.session {
                SessionSpec::Fresh => CoreSessionHandle {
                    provider: fake_provider_id(),
                    id: format!("session-{call}"),
                },
                SessionSpec::Resume { session } => session,
            };
            client
                .shutdown()
                .await
                .map_err(|error| callback_error("shut down callback client", error))?;
            guard.completed = true;
            Ok(RunOutcome {
                output: format!("answer:{prompt}"),
                session: Some(session),
                usage: None,
                cost: None,
                duration_ms: Some(1),
                provider_turns: Some(1),
                structured_output: None,
            })
        })
    }
}

fn callback_error(action: &str, error: impl std::fmt::Display) -> ProviderError {
    ProviderError::new(
        CoreFailureKind::Provider,
        format!("provider failed to {action}: {error}"),
    )
}

fn fake_provider_id() -> ProviderId {
    ProviderId::new("provider-self-fake").expect("static provider id is valid")
}

fn test_agent() -> (AgentInstance, Arc<FakeState>) {
    let state = Arc::new(FakeState::default());
    let mut runtime = Roba::new();
    runtime
        .register(CallbackProvider {
            state: Arc::clone(&state),
        })
        .expect("fake provider registration succeeds");
    let template = RunSpec::suspended(AgentSpec::new(fake_provider_id()));
    let agent = AgentInstance::new(runtime, template).expect("test agent is valid");
    (agent, state)
}

async fn bounded<F>(future: F) -> F::Output
where
    F: Future,
{
    tokio::time::timeout(TEST_TIMEOUT, future)
        .await
        .expect("operation completed before the test timeout")
}

async fn take(semaphore: &Semaphore, label: &str) {
    bounded(semaphore.acquire())
        .await
        .unwrap_or_else(|_| panic!("{label} semaphore remains open"))
        .forget();
}

fn typed<T: DeserializeOwned>(result: &CallToolResult) -> T {
    serde_json::from_value(
        result
            .structured_content
            .clone()
            .expect("typed tool result contains structured content"),
    )
    .expect("structured content matches its published type")
}

type CleartextClient = HyperClient<HttpConnector, Full<Bytes>>;

struct HttpResponse {
    status: StatusCode,
    headers: HeaderMap,
    body: Vec<u8>,
}

struct TestHttpMcpClient {
    client: CleartextClient,
    url: String,
    authorization: Option<String>,
    session_id: String,
    next_id: u64,
}

impl TestHttpMcpClient {
    async fn connect(
        endpoint: &ProviderMcpEndpoint,
        bearer_token: Option<&str>,
    ) -> Result<Self, String> {
        let client = HyperClient::builder(TokioExecutor::new()).build(HttpConnector::new());
        let authorization = bearer_token.map(|token| format!("Bearer {token}"));
        let initialize = send_http(
            &client,
            endpoint.url(),
            &authorization,
            None,
            Method::POST,
            Some(json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2025-11-25",
                    "capabilities": {},
                    "clientInfo": {
                        "name": "roba-provider-self-test",
                        "version": env!("CARGO_PKG_VERSION")
                    }
                }
            })),
        )
        .await?;
        require_success(&initialize)?;
        let initialized = response_json(&initialize.body)?;
        if initialized.get("result").is_none() || initialized.get("error").is_some() {
            return Err(format!("MCP initialize failed: {initialized}"));
        }
        if initialized["result"]["serverInfo"]["name"] != PROVIDER_MCP_SERVER_NAME {
            return Err(format!(
                "provider MCP server name was not stable: {initialized}"
            ));
        }
        let session_id = initialize
            .headers
            .get("mcp-session-id")
            .ok_or_else(|| "MCP initialize response omitted mcp-session-id".to_owned())?
            .to_str()
            .map_err(|error| format!("invalid mcp-session-id: {error}"))?
            .to_owned();
        let initialized_notification = send_http(
            &client,
            endpoint.url(),
            &authorization,
            Some(&session_id),
            Method::POST,
            Some(json!({
                "jsonrpc": "2.0",
                "method": "notifications/initialized"
            })),
        )
        .await?;
        require_success(&initialized_notification)?;

        Ok(Self {
            client,
            url: endpoint.url().to_owned(),
            authorization,
            session_id,
            next_id: 2,
        })
    }

    async fn request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        let response = send_http(
            &self.client,
            &self.url,
            &self.authorization,
            Some(&self.session_id),
            Method::POST,
            Some(json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": method,
                "params": params
            })),
        )
        .await?;
        require_success(&response)?;
        let envelope = response_json(&response.body)?;
        if let Some(error) = envelope.get("error") {
            return Err(format!("MCP {method} failed: {error}"));
        }
        envelope
            .get("result")
            .cloned()
            .ok_or_else(|| format!("MCP {method} response omitted result: {envelope}"))
    }

    async fn list_tools(&mut self) -> Result<Vec<String>, String> {
        let result = self.request("tools/list", json!({})).await?;
        result["tools"]
            .as_array()
            .ok_or_else(|| format!("tools/list returned malformed result: {result}"))?
            .iter()
            .map(|tool| {
                tool["name"]
                    .as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| format!("tools/list returned unnamed tool: {tool}"))
            })
            .collect()
    }

    async fn list_resources(&mut self) -> Result<usize, String> {
        let result = self.request("resources/list", json!({})).await?;
        result["resources"]
            .as_array()
            .map(Vec::len)
            .ok_or_else(|| format!("resources/list returned malformed result: {result}"))
    }

    async fn call_tool(&mut self, name: &str, arguments: Value) -> Result<CallToolResult, String> {
        let result = self
            .request("tools/call", json!({"name": name, "arguments": arguments}))
            .await?;
        serde_json::from_value(result)
            .map_err(|error| format!("tools/call result did not match MCP: {error}"))
    }

    async fn shutdown(self) -> Result<(), String> {
        let response = send_http(
            &self.client,
            &self.url,
            &self.authorization,
            Some(&self.session_id),
            Method::DELETE,
            None,
        )
        .await?;
        require_success(&response)
    }
}

async fn send_http(
    client: &CleartextClient,
    url: &str,
    authorization: &Option<String>,
    session_id: Option<&str>,
    method: Method,
    body: Option<Value>,
) -> Result<HttpResponse, String> {
    let mut request = Request::builder()
        .method(method)
        .uri(url)
        .header(ACCEPT, "application/json, text/event-stream");
    if body.is_some() {
        request = request.header(CONTENT_TYPE, "application/json");
    }
    if let Some(authorization) = authorization {
        request = request.header(AUTHORIZATION, authorization);
    }
    if let Some(session_id) = session_id {
        request = request.header("mcp-session-id", session_id);
    }
    let body = body
        .map(|value| Full::new(Bytes::from(value.to_string())))
        .unwrap_or_else(|| Full::new(Bytes::new()));
    let response = client
        .request(
            request
                .body(body)
                .map_err(|error| format!("failed to build HTTP request: {error}"))?,
        )
        .await
        .map_err(|error| format!("HTTP request failed: {error}"))?;
    let status = response.status();
    let headers = response.headers().clone();
    let body = response
        .into_body()
        .collect()
        .await
        .map_err(|error| format!("failed to collect HTTP response: {error}"))?
        .to_bytes()
        .to_vec();
    Ok(HttpResponse {
        status,
        headers,
        body,
    })
}

fn require_success(response: &HttpResponse) -> Result<(), String> {
    if response.status.is_success() {
        Ok(())
    } else {
        Err(format!(
            "HTTP {}: {}",
            response.status,
            String::from_utf8_lossy(&response.body)
        ))
    }
}

fn response_json(body: &[u8]) -> Result<Value, String> {
    if body.is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_slice(body).or_else(|_| {
        let text = String::from_utf8_lossy(body);
        let data = text
            .lines()
            .find_map(|line| line.strip_prefix("data: "))
            .ok_or_else(|| format!("HTTP response was neither JSON nor MCP SSE: {text}"))?;
        serde_json::from_str(data).map_err(|error| format!("invalid MCP SSE data: {error}"))
    })
}

async fn initialized_http(
    endpoint: &ProviderMcpEndpoint,
    bearer_token: Option<&str>,
) -> Result<TestHttpMcpClient, String> {
    TestHttpMcpClient::connect(endpoint, bearer_token).await
}

async fn assert_auth_rejected(endpoint: &ProviderMcpEndpoint, token: Option<&str>) {
    let error = match bounded(initialized_http(endpoint, token)).await {
        Ok(_) => panic!("private endpoint unexpectedly accepted invalid authentication"),
        Err(error) => error,
    };
    assert!(
        error.contains("401"),
        "active endpoint rejected authentication for an unexpected reason: {error}"
    );
}

async fn assert_expired(endpoint: &ProviderMcpEndpoint) {
    let result = bounded(initialized_http(endpoint, Some(endpoint.bearer_token()))).await;
    assert!(
        result.is_err(),
        "settled endpoint unexpectedly accepted its former credential"
    );
}

async fn call_turn(client: Arc<McpClient>, text: &'static str) -> CallToolResult {
    bounded(client.call_tool(AGENT_TURN_TOOL, json!({"text": text})))
        .await
        .expect("agent.turn returns an MCP result")
}

fn snapshot_callbacks(state: &FakeState) -> Vec<CallbackObservation> {
    state
        .callbacks
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

fn snapshot_endpoints(state: &FakeState) -> Vec<ProviderMcpEndpoint> {
    state
        .endpoints
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

fn assert_launch_material_absent(serialized: &str, endpoints: &[ProviderMcpEndpoint]) {
    for endpoint in endpoints {
        assert!(
            !serialized.contains(endpoint.url()),
            "serialized public/core value leaked provider endpoint URL"
        );
        assert!(
            !serialized.contains(endpoint.bearer_token()),
            "serialized public/core value leaked provider bearer token"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn control_and_provider_discovery_are_exact_role_projections() {
    let (agent, state) = test_agent();

    let provider_client = McpClient::connect(ChannelTransport::new(agent_router(
        agent.clone(),
        OperationId::new(41),
    )))
    .await
    .expect("provider projection client connects");
    provider_client
        .initialize("provider-projection-test", env!("CARGO_PKG_VERSION"))
        .await
        .expect("provider projection initializes");
    let provider_tools = provider_client
        .list_tools()
        .await
        .expect("provider tools are discoverable")
        .tools
        .into_iter()
        .map(|tool| tool.name)
        .collect::<Vec<_>>();
    assert_eq!(provider_tools, [ROBA_SELF_TOOL]);
    assert!(
        provider_client
            .list_resources()
            .await
            .expect("provider resources are discoverable")
            .resources
            .is_empty()
    );
    assert!(
        provider_client
            .list_prompts()
            .await
            .expect("provider prompts are discoverable")
            .prompts
            .is_empty()
    );
    for forbidden in [
        AGENT_TURN_TOOL,
        AGENT_STEER_TOOL,
        AGENT_INTERRUPT_TOOL,
        AGENT_SHUTDOWN_TOOL,
    ] {
        assert!(
            provider_client
                .call_tool(forbidden, json!({}))
                .await
                .is_err(),
            "provider projection unexpectedly dispatched {forbidden}"
        );
    }
    provider_client
        .shutdown()
        .await
        .expect("provider projection client shuts down");

    let control_client = connect_in_process(agent)
        .await
        .expect("control projection client connects");
    let mut control_tools = control_client
        .list_tools()
        .await
        .expect("control tools are discoverable")
        .tools
        .into_iter()
        .map(|tool| tool.name)
        .collect::<Vec<_>>();
    control_tools.sort_unstable();
    let mut expected = vec![
        AGENT_INTERRUPT_TOOL.to_owned(),
        AGENT_SHUTDOWN_TOOL.to_owned(),
        AGENT_STEER_TOOL.to_owned(),
        AGENT_TURN_TOOL.to_owned(),
    ];
    expected.sort_unstable();
    assert_eq!(control_tools, expected);
    assert_eq!(state.calls.load(Ordering::SeqCst), 0);
    control_client
        .shutdown()
        .await
        .expect("control projection client shuts down");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn authenticated_callback_rotates_credentials_expires_and_stays_out_of_serialization() {
    let (agent, state) = test_agent();
    let client = Arc::new(
        connect_in_process(agent)
            .await
            .expect("control projection client connects"),
    );

    let first_turn = tokio::spawn(call_turn(Arc::clone(&client), "hold"));
    take(&state.endpoint_ready, "endpoint ready").await;
    take(&state.callback_ready, "callback ready").await;
    let first_endpoint = snapshot_endpoints(&state)
        .into_iter()
        .next()
        .expect("first endpoint was captured");
    assert_eq!(first_endpoint.name(), PROVIDER_MCP_SERVER_NAME);
    assert_auth_rejected(&first_endpoint, None).await;
    assert_auth_rejected(&first_endpoint, Some("definitely-the-wrong-token")).await;
    let mut current_client = bounded(initialized_http(
        &first_endpoint,
        Some(first_endpoint.bearer_token()),
    ))
    .await
    .expect("current bearer authenticates");
    let current = bounded(current_client.call_tool(ROBA_SELF_TOOL, json!({})))
        .await
        .expect("authenticated callback succeeds");
    assert!(!current.is_error);
    let current_snapshot = typed::<ProviderSelfSnapshot>(&current);
    assert_eq!(current_snapshot.state, AgentState::Running);
    current_client
        .shutdown()
        .await
        .expect("authenticated probe shuts down");
    state.release.add_permits(1);
    let first_result = bounded(first_turn).await.expect("first turn task joins");
    assert!(!first_result.is_error);
    assert_expired(&first_endpoint).await;

    let second_turn = tokio::spawn(call_turn(Arc::clone(&client), "hold"));
    take(&state.endpoint_ready, "endpoint ready").await;
    take(&state.callback_ready, "callback ready").await;
    let endpoints = snapshot_endpoints(&state);
    let second_endpoint = endpoints
        .get(1)
        .expect("second endpoint was captured")
        .clone();
    assert_eq!(second_endpoint.name(), PROVIDER_MCP_SERVER_NAME);
    assert_ne!(
        first_endpoint.bearer_token(),
        second_endpoint.bearer_token(),
        "a new finite run must rotate its provider credential"
    );
    assert_auth_rejected(&second_endpoint, Some(first_endpoint.bearer_token())).await;
    let second_current = bounded(initialized_http(
        &second_endpoint,
        Some(second_endpoint.bearer_token()),
    ))
    .await
    .expect("rotated bearer authenticates");
    second_current
        .shutdown()
        .await
        .expect("rotated authenticated probe shuts down");
    state.release.add_permits(1);
    let second_result = bounded(second_turn).await.expect("second turn task joins");
    assert!(!second_result.is_error);
    assert_expired(&second_endpoint).await;

    let callbacks = snapshot_callbacks(&state);
    assert_eq!(callbacks.len(), 2);
    for (index, callback) in callbacks.iter().enumerate() {
        assert_eq!(callback.tools, [ROBA_SELF_TOOL]);
        assert_eq!(callback.resource_count, 0);
        assert_eq!(callback.snapshot.state, AgentState::Running);
        assert_eq!(callback.snapshot.operation_id.get(), index as u64 + 1);
        assert_launch_material_absent(&callback.result_json, &endpoints);
    }
    let launch_debug = state
        .launch_debug
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    assert_eq!(launch_debug.len(), 2);
    for (debug, endpoint) in launch_debug.iter().zip(&endpoints) {
        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains(endpoint.bearer_token()));
    }
    let requests = state
        .request_json
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    for request in requests {
        assert_launch_material_absent(&request, &endpoints);
    }
    let public_snapshot = bounded(client.read_resource(AGENT_RESOURCE_URI))
        .await
        .expect("public agent resource remains readable");
    assert_launch_material_absent(
        public_snapshot
            .first_text()
            .expect("agent resource is JSON text"),
        &endpoints,
    );
    let public_events = bounded(client.read_resource(AGENT_EVENTS_URI))
        .await
        .expect("public events resource remains readable");
    assert_launch_material_absent(
        public_events
            .first_text()
            .expect("events resource is JSON text"),
        &endpoints,
    );
    assert_launch_material_absent(
        &serde_json::to_string(&first_result).expect("first MCP result serializes"),
        &endpoints,
    );
    assert_launch_material_absent(
        &serde_json::to_string(&second_result).expect("second MCP result serializes"),
        &endpoints,
    );

    let client = Arc::into_inner(client).expect("all control client clones were dropped");
    client
        .shutdown()
        .await
        .expect("control projection client shuts down");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn interrupt_drops_provider_and_revokes_endpoint_before_settlement_returns() {
    let (agent, state) = test_agent();
    let client = Arc::new(
        connect_in_process(agent)
            .await
            .expect("control projection client connects"),
    );

    let turn = tokio::spawn(call_turn(Arc::clone(&client), "hold"));
    take(&state.endpoint_ready, "endpoint ready").await;
    take(&state.callback_ready, "callback ready").await;
    let endpoint = snapshot_endpoints(&state)
        .into_iter()
        .next()
        .expect("provider endpoint was captured");
    let callback = snapshot_callbacks(&state)
        .into_iter()
        .next()
        .expect("provider callback completed");

    let interrupted = bounded(client.call_tool(
        AGENT_INTERRUPT_TOOL,
        json!({"operation_id": callback.snapshot.operation_id}),
    ))
    .await
    .expect("interrupt returns after settlement");
    assert!(!interrupted.is_error);
    let interrupt = typed::<AgentInterruptResult>(&interrupted);
    assert_eq!(
        interrupt,
        AgentInterruptResult::Settled {
            settlement: roba_mcp::OperationSettlement {
                operation_id: callback.snapshot.operation_id,
                state: AgentTerminalState::Cancelled,
            },
            cancellation_requested: true,
        }
    );
    state
        .dropped
        .try_acquire()
        .expect("provider future was dropped before interrupt settlement returned")
        .forget();
    assert_expired(&endpoint).await;

    let cancelled = bounded(turn).await.expect("cancelled turn task joins");
    assert!(cancelled.is_error);
    assert!(matches!(
        typed::<AgentTurnResult>(&cancelled),
        AgentTurnResult::Cancelled { .. }
    ));

    let next = call_turn(Arc::clone(&client), "next").await;
    assert!(!next.is_error, "agent remains reusable after cancellation");
    let endpoints = snapshot_endpoints(&state);
    assert_eq!(endpoints.len(), 2);
    assert_ne!(endpoints[0].bearer_token(), endpoints[1].bearer_token());
    assert_expired(&endpoints[0]).await;
    assert_expired(&endpoints[1]).await;

    let client = Arc::into_inner(client).expect("all control client clones were dropped");
    client
        .shutdown()
        .await
        .expect("control projection client shuts down");
}
