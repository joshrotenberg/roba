use std::ffi::OsStr;
use std::fs;
use std::future::Future;
use std::path::Path;
use std::process::{Command, Output};
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
    AgentSpec, EventSink, FailureKind, PermissionPolicy, Provider, ProviderCapabilities,
    ProviderError, ProviderFuture, ProviderId, ProviderLaunchContext, ProviderMcpEndpoint, Roba,
    RunOutcome, RunSpec, SessionHandle, SessionSpec, TurnRequest,
};
use roba_git::{
    GIT_SNAPSHOT_TOOL, GIT_STAGE_ALL_TOOL, GIT_WORKSPACE_RESOURCE_URI, GitAuthority, GitWorkspace,
};
use roba_mcp::{
    AGENT_TURN_TOOL, AgentExtensions, AgentInstance, OperationId, PROVIDER_MCP_SERVER_NAME,
    ROBA_SELF_TOOL, agent_router, connect_in_process,
};
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::sync::Semaphore;
use tower_mcp::{CallToolResult, ChannelTransport, McpClient};

const TEST_TIMEOUT: Duration = Duration::from_secs(10);
const SESSION_ID: &str = "phase-6-sticky-session";
const INSTRUCTION: &str = "  preserve instruction whitespace\nsecond line: αβ  ";
const PROJECT_CONTEXT: &str = "project context\n  stays byte-for-byte\t";
const RUN_CONTEXT: &str = "\trun context has leading and trailing space ";

#[derive(Clone)]
struct ProviderObservation {
    tools: Vec<String>,
    resources: Vec<String>,
    snapshot: Value,
}

struct FakeState {
    calls: AtomicUsize,
    requests: Mutex<Vec<TurnRequest>>,
    endpoints: Mutex<Vec<ProviderMcpEndpoint>>,
    observations: Mutex<Vec<ProviderObservation>>,
    callback_ready: Semaphore,
    release: Semaphore,
}

impl Default for FakeState {
    fn default() -> Self {
        Self {
            calls: AtomicUsize::new(0),
            requests: Mutex::new(Vec::new()),
            endpoints: Mutex::new(Vec::new()),
            observations: Mutex::new(Vec::new()),
            callback_ready: Semaphore::new(0),
            release: Semaphore::new(0),
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
                FailureKind::Provider,
                "harness provider requires transient launch context",
            ))
        })
    }

    fn execute_with_launch_context<'a>(
        &'a self,
        request: TurnRequest,
        launch_context: ProviderLaunchContext,
        _events: &'a dyn EventSink,
    ) -> ProviderFuture<'a> {
        let endpoint = match launch_context.mcp_endpoints() {
            [endpoint] => endpoint.clone(),
            endpoints => {
                let count = endpoints.len();
                return Box::pin(async move {
                    Err(ProviderError::new(
                        FailureKind::Provider,
                        format!("expected one private endpoint, received {count}"),
                    ))
                });
            }
        };
        self.state.calls.fetch_add(1, Ordering::SeqCst);
        lock(&self.state.requests).push(request.clone());
        lock(&self.state.endpoints).push(endpoint.clone());
        let state = Arc::clone(&self.state);

        Box::pin(async move {
            if endpoint.name() != PROVIDER_MCP_SERVER_NAME
                || endpoint.tool_names() != [GIT_SNAPSHOT_TOOL, ROBA_SELF_TOOL]
            {
                return Err(callback_error(
                    "validate launch manifest",
                    format!(
                        "server={}, tools={:?}",
                        endpoint.name(),
                        endpoint.tool_names()
                    ),
                ));
            }
            let mut client = initialized_http(&endpoint, Some(endpoint.bearer_token()))
                .await
                .map_err(|error| callback_error("authenticate", error))?;
            let mut tools = client
                .list_tools()
                .await
                .map_err(|error| callback_error("list tools", error))?;
            tools.sort_unstable();
            if tools != [GIT_SNAPSHOT_TOOL, ROBA_SELF_TOOL] {
                return Err(callback_error(
                    "validate provider discovery",
                    format!("unexpected tools: {tools:?}"),
                ));
            }
            let resources = client
                .list_resources()
                .await
                .map_err(|error| callback_error("list resources", error))?;
            if resources != [GIT_WORKSPACE_RESOURCE_URI] {
                return Err(callback_error(
                    "validate provider resources",
                    format!("unexpected resources: {resources:?}"),
                ));
            }
            let self_result = client
                .call_tool(ROBA_SELF_TOOL, json!({}))
                .await
                .map_err(|error| callback_error("call roba.self", error))?;
            if self_result.is_error {
                return Err(callback_error("call roba.self", "returned isError=true"));
            }
            let snapshot = client
                .call_tool(GIT_SNAPSHOT_TOOL, json!({}))
                .await
                .map_err(|error| callback_error("call git.snapshot", error))?;
            if snapshot.is_error {
                return Err(callback_error("call git.snapshot", "returned isError=true"));
            }
            let snapshot = snapshot
                .structured_content
                .ok_or_else(|| callback_error("call git.snapshot", "missing structured content"))?;
            if client
                .call_tool(AGENT_TURN_TOOL, json!({"text": "forbidden"}))
                .await
                .is_ok()
            {
                return Err(callback_error(
                    "enforce provider projection",
                    "agent.turn unexpectedly dispatched",
                ));
            }
            if client
                .call_tool(GIT_STAGE_ALL_TOOL, json!({}))
                .await
                .is_ok()
            {
                return Err(callback_error(
                    "enforce provider projection",
                    "git.stage_all unexpectedly dispatched",
                ));
            }

            lock(&state.observations).push(ProviderObservation {
                tools,
                resources,
                snapshot,
            });
            state.callback_ready.add_permits(1);
            state
                .release
                .acquire()
                .await
                .expect("release semaphore remains open")
                .forget();
            client
                .shutdown()
                .await
                .map_err(|error| callback_error("shut down provider client", error))?;

            let prompt = request.prompt.as_str().to_owned();
            let session = match request.spec.execution.session {
                SessionSpec::Fresh => SessionHandle {
                    provider: fake_provider_id(),
                    id: SESSION_ID.to_owned(),
                },
                SessionSpec::Resume { session } => session,
            };
            Ok(RunOutcome {
                output: format!("completed:{prompt}"),
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
        FailureKind::Provider,
        format!("provider failed to {action}: {error}"),
    )
}

fn fake_provider_id() -> ProviderId {
    ProviderId::new("roba-git-harness").expect("static provider id is valid")
}

fn test_agent(
    workspace: &GitWorkspace,
    authority: GitAuthority,
    permissions: PermissionPolicy,
) -> (AgentInstance, Arc<FakeState>) {
    let state = Arc::new(FakeState::default());
    let mut runtime = Roba::new();
    runtime
        .register(CallbackProvider {
            state: Arc::clone(&state),
        })
        .expect("fake provider registration succeeds");
    let mut agent = AgentSpec::new(fake_provider_id());
    agent.instructions = vec![INSTRUCTION.to_owned()];
    let mut template = RunSpec::suspended(agent);
    template.context.project = vec![PROJECT_CONTEXT.to_owned()];
    template.context.run = vec![RUN_CONTEXT.to_owned()];
    template.execution.permissions = permissions;
    let extensions = AgentExtensions::default()
        .try_with(workspace.extension(authority))
        .expect("Git extension installs without collisions");
    let agent = AgentInstance::new_with_extensions(runtime, template, extensions)
        .expect("extended agent template is valid");
    (agent, state)
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

async fn bounded<F>(future: F) -> F::Output
where
    F: Future,
{
    tokio::time::timeout(TEST_TIMEOUT, future)
        .await
        .expect("operation completed before the harness timeout")
}

async fn take(semaphore: &Semaphore) {
    bounded(semaphore.acquire())
        .await
        .expect("test semaphore remains open")
        .forget();
}

struct Fixture {
    temp: TempDir,
}

impl Fixture {
    fn with_initial_commit() -> Self {
        let temp = tempfile::tempdir().expect("fixture tempdir");
        let fixture = Self { temp };
        fixture.git_ok(["init", "--quiet"]);
        fixture.git_ok(["config", "user.name", "Roba Test"]);
        fixture.git_ok(["config", "user.email", "roba@example.invalid"]);
        fixture.git_ok(["config", "commit.gpgsign", "false"]);
        fixture.git_ok(["config", "core.autocrlf", "false"]);
        fixture.git_ok(["config", "core.filemode", "false"]);
        fixture.git_ok(["config", "core.fsmonitor", "false"]);
        fixture.git_ok(["config", "core.hooksPath", ".git/disabled-hooks"]);
        fixture.write("tracked.txt", "original\n");
        fixture.git_ok(["add", "--all"]);
        fixture.git_ok(["commit", "--quiet", "--no-gpg-sign", "-m", "initial"]);
        fixture
    }

    fn root(&self) -> &Path {
        self.temp.path()
    }

    fn write(&self, relative: &str, contents: &str) {
        fs::write(self.root().join(relative), contents).expect("write fixture file");
    }

    fn git_ok<I, S>(&self, args: I) -> Output
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let output = Command::new("git")
            .args(args)
            .current_dir(self.root())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .expect("fixture git executable");
        assert!(
            output.status.success(),
            "fixture git failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }
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
                    "clientInfo": {"name": "roba-git-harness", "version": "0"}
                }
            })),
        )
        .await?;
        require_success(&initialize)?;
        let initialized = response_json(&initialize.body)?;
        if initialized.get("result").is_none() || initialized.get("error").is_some() {
            return Err(format!("MCP initialize failed: {initialized}"));
        }
        let session_id = initialize
            .headers
            .get("mcp-session-id")
            .ok_or_else(|| "MCP initialize omitted mcp-session-id".to_owned())?
            .to_str()
            .map_err(|error| format!("invalid mcp-session-id: {error}"))?
            .to_owned();
        let notification = send_http(
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
        require_success(&notification)?;
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
            .ok_or_else(|| format!("MCP {method} omitted result: {envelope}"))
    }

    async fn list_tools(&mut self) -> Result<Vec<String>, String> {
        let result = self.request("tools/list", json!({})).await?;
        result["tools"]
            .as_array()
            .ok_or_else(|| format!("malformed tools/list result: {result}"))?
            .iter()
            .map(|tool| {
                tool["name"]
                    .as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| format!("unnamed tool: {tool}"))
            })
            .collect()
    }

    async fn list_resources(&mut self) -> Result<Vec<String>, String> {
        let result = self.request("resources/list", json!({})).await?;
        result["resources"]
            .as_array()
            .ok_or_else(|| format!("malformed resources/list result: {result}"))?
            .iter()
            .map(|resource| {
                resource["uri"]
                    .as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| format!("resource without URI: {resource}"))
            })
            .collect()
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
            .ok_or_else(|| format!("response was neither JSON nor MCP SSE: {text}"))?;
        serde_json::from_str(data).map_err(|error| format!("invalid MCP SSE data: {error}"))
    })
}

async fn initialized_http(
    endpoint: &ProviderMcpEndpoint,
    token: Option<&str>,
) -> Result<TestHttpMcpClient, String> {
    TestHttpMcpClient::connect(endpoint, token).await
}

async fn assert_expired(endpoint: &ProviderMcpEndpoint) {
    let result = bounded(initialized_http(endpoint, Some(endpoint.bearer_token()))).await;
    assert!(
        result.is_err(),
        "settled endpoint accepted its expired credential"
    );
}

async fn in_process_provider(agent: AgentInstance) -> McpClient {
    let client = McpClient::connect(ChannelTransport::new(agent_router(
        agent,
        OperationId::new(1),
    )))
    .await
    .expect("provider projection client connects");
    client
        .initialize("roba-git-role-test", "0")
        .await
        .expect("provider projection initializes");
    client
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn read_only_and_writable_agents_keep_stage_all_control_only() {
    let fixture = Fixture::with_initial_commit();
    fixture.write("untracked.txt", "not staged by discovery\n");
    let workspace = GitWorkspace::discover(fixture.root()).expect("Git workspace discovery");

    for (authority, permissions, control_has_stage) in [
        (GitAuthority::ReadOnly, PermissionPolicy::ReadOnly, false),
        (
            GitAuthority::WorkspaceWrite,
            PermissionPolicy::WorkspaceWrite,
            true,
        ),
    ] {
        let (agent, state) = test_agent(&workspace, authority, permissions);
        let control = connect_in_process(agent.clone())
            .await
            .expect("control ChannelTransport connects");
        let control_tools = control
            .list_tools()
            .await
            .expect("control discovery succeeds")
            .tools;
        assert!(
            control_tools
                .iter()
                .any(|tool| tool.name == GIT_SNAPSHOT_TOOL)
        );
        assert_eq!(
            control_tools
                .iter()
                .any(|tool| tool.name == GIT_STAGE_ALL_TOOL),
            control_has_stage
        );
        if !control_has_stage {
            assert!(
                control
                    .call_tool(GIT_STAGE_ALL_TOOL, json!({}))
                    .await
                    .is_err(),
                "read-only control projection dispatched git.stage_all"
            );
        }

        let provider = in_process_provider(agent).await;
        let mut provider_tools = provider
            .list_tools()
            .await
            .expect("provider discovery succeeds")
            .tools
            .into_iter()
            .map(|tool| tool.name)
            .collect::<Vec<_>>();
        provider_tools.sort_unstable();
        assert_eq!(provider_tools, [GIT_SNAPSHOT_TOOL, ROBA_SELF_TOOL]);
        assert!(
            provider
                .call_tool(GIT_STAGE_ALL_TOOL, json!({}))
                .await
                .is_err(),
            "provider projection dispatched git.stage_all"
        );
        assert!(
            provider
                .call_tool(AGENT_TURN_TOOL, json!({"text": "forbidden"}))
                .await
                .is_err(),
            "provider projection dispatched agent.turn"
        );
        let resources = provider
            .list_resources()
            .await
            .expect("provider resource discovery succeeds")
            .resources;
        assert_eq!(resources.len(), 1);
        assert_eq!(resources[0].uri, GIT_WORKSPACE_RESOURCE_URI);
        assert_eq!(state.calls.load(Ordering::SeqCst), 0);
        provider
            .shutdown()
            .await
            .expect("provider client shuts down");
        control.shutdown().await.expect("control client shuts down");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn active_provider_and_operator_share_git_state_without_changing_turn_context() {
    let original_cwd = std::env::current_dir().expect("current directory is readable");
    let fixture = Fixture::with_initial_commit();
    fixture.write("untracked.txt", "visible to both projections\n");
    let workspace = GitWorkspace::discover(fixture.root()).expect("Git workspace discovery");
    let (agent, state) = test_agent(
        &workspace,
        GitAuthority::WorkspaceWrite,
        PermissionPolicy::WorkspaceWrite,
    );
    let control = Arc::new(
        connect_in_process(agent)
            .await
            .expect("control ChannelTransport connects"),
    );
    let prompts = ["first turn", "second turn"];

    for (index, prompt) in prompts.iter().enumerate() {
        let turn_client = Arc::clone(&control);
        let prompt = (*prompt).to_owned();
        let turn = tokio::spawn(async move {
            turn_client
                .call_tool(AGENT_TURN_TOOL, json!({"text": prompt}))
                .await
        });
        take(&state.callback_ready).await;

        let endpoint = lock(&state.endpoints)
            .get(index)
            .expect("provider captured the active endpoint")
            .clone();
        assert_eq!(endpoint.name(), PROVIDER_MCP_SERVER_NAME);
        assert_eq!(endpoint.tool_names(), [GIT_SNAPSHOT_TOOL, ROBA_SELF_TOOL]);
        assert!(endpoint.url().starts_with("http://127.0.0.1:"));
        assert!(endpoint.url().ends_with("/mcp"));
        let provider = lock(&state.observations)
            .get(index)
            .expect("provider completed its active callback")
            .clone();
        assert_eq!(provider.tools, [GIT_SNAPSHOT_TOOL, ROBA_SELF_TOOL]);
        assert_eq!(provider.resources, [GIT_WORKSPACE_RESOURCE_URI]);

        let operator_tool = control
            .call_tool(GIT_SNAPSHOT_TOOL, json!({}))
            .await
            .expect("operator calls git.snapshot while provider is active");
        assert!(!operator_tool.is_error);
        let operator_snapshot = operator_tool
            .structured_content
            .expect("operator snapshot is typed");
        let operator_resource = control
            .read_resource(GIT_WORKSPACE_RESOURCE_URI)
            .await
            .expect("operator reads Git workspace resource while provider is active");
        let resource_snapshot: Value = serde_json::from_str(
            operator_resource
                .first_text()
                .expect("Git workspace resource is JSON text"),
        )
        .expect("Git workspace resource is valid JSON");
        assert_eq!(provider.snapshot, operator_snapshot);
        assert_eq!(provider.snapshot, resource_snapshot);

        state.release.add_permits(1);
        let result = bounded(turn)
            .await
            .expect("turn task joins")
            .expect("agent.turn MCP dispatch succeeds");
        assert!(!result.is_error, "turn failed: {result:?}");
        assert_expired(&endpoint).await;
    }

    let endpoints = lock(&state.endpoints).clone();
    assert_eq!(endpoints.len(), 2);
    assert_ne!(
        endpoints[0].bearer_token(),
        endpoints[1].bearer_token(),
        "each finite turn rotates its private credential"
    );
    let requests = lock(&state.requests).clone();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].spec.execution.session, SessionSpec::Fresh);
    assert_eq!(
        requests[1].spec.execution.session,
        SessionSpec::Resume {
            session: SessionHandle {
                provider: fake_provider_id(),
                id: SESSION_ID.to_owned(),
            }
        }
    );
    for (request, prompt) in requests.iter().zip(prompts) {
        assert_eq!(request.prompt.as_str(), prompt);
        assert_eq!(
            request
                .spec
                .initial_prompt
                .as_ref()
                .expect("started run records its prompt")
                .as_str(),
            prompt
        );
        assert_eq!(request.spec.agent.instructions, [INSTRUCTION]);
        assert_eq!(request.spec.context.project, [PROJECT_CONTEXT]);
        assert_eq!(request.spec.context.run, [RUN_CONTEXT]);
        let context = serde_json::to_string(&(
            &request.spec.agent.instructions,
            &request.spec.context.project,
            &request.spec.context.run,
        ))
        .expect("context serializes");
        assert!(!context.contains("git.snapshot"));
        assert!(!context.contains(GIT_WORKSPACE_RESOURCE_URI));
    }
    assert_eq!(state.calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        std::env::current_dir().expect("current directory remains readable"),
        original_cwd,
        "the harness and Git service must not mutate process cwd"
    );

    let control = Arc::into_inner(control).expect("turn client clones were dropped");
    control.shutdown().await.expect("control client shuts down");
}
