//! Run-scoped MCP adapter over [`roba_core::RunHandle`].
//!
//! The adapter owns no execution state. Its tools call the same handle a Rust
//! host or REPL uses, so transport choice cannot change lifecycle semantics.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

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
    FailureKind, MissionReport, MissionWorkState, ProcessActionId, ProcessCapabilityId,
    ProcessControl, Prompt, Provider, ProviderCapabilities, ProviderContext, ProviderError,
    ProviderFuture, ProviderId, ProviderMcpEndpoint, RUN_EVENT_CAPACITY, RunHandle, TurnRequest,
    WorkerControl,
};

const INTERNAL_SERVER_NAME: &str = "roba_workers";

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PromptArgs {
    /// Message for the root Roba agent.
    text: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct EventsArgs {
    /// Last sequence observed by the client. Omit or use zero to replay the
    /// oldest retained event.
    #[serde(default)]
    after: u64,
    /// Maximum records to return, from 1 through 256.
    #[serde(default = "default_event_limit")]
    limit: usize,
    /// Wait this many milliseconds for a new event when the cursor is caught
    /// up. The maximum is 30 seconds.
    #[serde(default)]
    wait_ms: u64,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum ReportWorkState {
    Planned,
    InProgress,
    Completed,
    Blocked,
}

impl From<ReportWorkState> for MissionWorkState {
    fn from(value: ReportWorkState) -> Self {
        match value {
            ReportWorkState::Planned => Self::Planned,
            ReportWorkState::InProgress => Self::InProgress,
            ReportWorkState::Completed => Self::Completed,
            ReportWorkState::Blocked => Self::Blocked,
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ReportWorkItemArgs {
    id: String,
    title: String,
    state: ReportWorkState,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ReportBlockerArgs {
    id: String,
    message: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ReportArtifactArgs {
    id: String,
    artifact_kind: String,
    reference: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ProcessActionArgs {
    capability: String,
    action: String,
    #[serde(default)]
    input: serde_json::Value,
}

const MAX_EVENT_WAIT_MS: u64 = 30_000;

fn default_event_limit() -> usize {
    100
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

    let mission = {
        let handle = handle.clone();
        ToolBuilder::new("mission")
            .description("Inspect the canonical mission projection, including runtime facts and agent-reported work state.")
            .read_only()
            .no_params_handler(move || {
                let handle = handle.clone();
                async move { json_result(handle.mission().await) }
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

    let events = {
        let handle = handle.clone();
        ToolBuilder::new("events")
            .description(
                "Read timestamped, sequenced lifecycle and provider events for this run tree. Use next_sequence as the next after cursor; wait_ms enables bounded long polling.",
            )
            .read_only()
            .handler(move |args: EventsArgs| {
                let handle = handle.clone();
                async move {
                    if !(1..=RUN_EVENT_CAPACITY).contains(&args.limit) {
                        return Ok(CallToolResult::error(format!(
                            "limit must be between 1 and {RUN_EVENT_CAPACITY}"
                        )));
                    }
                    if args.wait_ms > MAX_EVENT_WAIT_MS {
                        return Ok(CallToolResult::error(format!(
                            "wait_ms must be between 0 and {MAX_EVENT_WAIT_MS}"
                        )));
                    }
                    let page = match handle.event_page(args.after, args.limit).await {
                        Ok(page)
                            if page.events.is_empty()
                                && !page.truncated
                                && !page.terminal
                                && args.wait_ms > 0 =>
                        {
                            match tokio::time::timeout(
                                Duration::from_millis(args.wait_ms),
                                handle.wait_for_events(args.after, args.limit),
                            )
                            .await
                            {
                                Ok(result) => result,
                                Err(_) => handle.event_page(args.after, args.limit).await,
                            }
                        }
                        result => result,
                    };
                    match page {
                        Ok(page) => json_result(page),
                        Err(error) => Ok(CallToolResult::error(error.to_string())),
                    }
                }
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
            "This MCP server controls one finite Roba mission. Start supplies the first prompt; mission returns the canonical monitoring projection; status, wait, events, and workers expose its underlying run tree; steer queues root guidance; spawn_worker starts an inherited child within explicit bounds; cancel ends the tree.",
        )
        .tool(start)
        .tool(status)
        .tool(mission)
        .tool(wait)
        .tool(steer)
        .tool(spawn_worker)
        .tool(workers)
        .tool(events)
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

    fn supports_process_control(&self) -> bool {
        true
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
            let worker_control = context.worker_control().cloned();
            let process_control = context.process_control().cloned();
            if worker_control.is_none() && process_control.is_none() {
                return self.inner.execute(request, context, events).await;
            }
            let server = InternalWorkerServer::start(worker_control, process_control)
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
    async fn start(
        worker_control: Option<WorkerControl>,
        process_control: Option<ProcessControl>,
    ) -> std::io::Result<Self> {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
        let address = listener.local_addr()?;
        let token = Uuid::new_v4().simple().to_string();
        let authorization: Arc<str> = format!("Bearer {token}").into();
        let app = HttpTransport::new(internal_router(worker_control, process_control))
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

fn internal_router(
    worker_control: Option<WorkerControl>,
    process_control: Option<ProcessControl>,
) -> McpRouter {
    let mut router = McpRouter::new()
        .server_info("roba-worker-control", env!("CARGO_PKG_VERSION"))
        .instructions(
            "This private server exposes only capabilities captured for this exact run. It cannot widen run authority or overwrite host-derived runtime facts.",
        );

    if let Some(control) = worker_control {
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
        let workers = {
            let control = control.clone();
            ToolBuilder::new("workers")
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
                .build()
        };

        let report_work_item = {
            let control = control.clone();
            ToolBuilder::new("report_work_item")
                .description("Upsert this run's claimed mission work-item state for monitors.")
                .non_destructive()
                .handler(move |args: ReportWorkItemArgs| {
                    let control = control.clone();
                    async move {
                        match control
                            .report(MissionReport::WorkItem {
                                id: args.id,
                                title: args.title,
                                state: args.state.into(),
                            })
                            .await
                        {
                            Ok(()) => Ok(CallToolResult::text("mission work item recorded")),
                            Err(error) => Ok(CallToolResult::error(error.to_string())),
                        }
                    }
                })
                .build()
        };
        let report_blocker = {
            let control = control.clone();
            ToolBuilder::new("report_blocker")
                .description("Upsert this run's claimed mission blocker for monitors.")
                .non_destructive()
                .handler(move |args: ReportBlockerArgs| {
                    let control = control.clone();
                    async move {
                        match control
                            .report(MissionReport::Blocker {
                                id: args.id,
                                message: args.message,
                            })
                            .await
                        {
                            Ok(()) => Ok(CallToolResult::text("mission blocker recorded")),
                            Err(error) => Ok(CallToolResult::error(error.to_string())),
                        }
                    }
                })
                .build()
        };
        let report_artifact = ToolBuilder::new("report_artifact")
            .description(
                "Upsert this run's claimed mission artifact or external reference for monitors.",
            )
            .non_destructive()
            .handler(move |args: ReportArtifactArgs| {
                let control = control.clone();
                async move {
                    match control
                        .report(MissionReport::Artifact {
                            id: args.id,
                            artifact_kind: args.artifact_kind,
                            reference: args.reference,
                        })
                        .await
                    {
                        Ok(()) => Ok(CallToolResult::text("mission artifact recorded")),
                        Err(error) => Ok(CallToolResult::error(error.to_string())),
                    }
                }
            })
            .build();

        router = router
            .tool(spawn_worker)
            .tool(workers)
            .tool(report_work_item)
            .tool(report_blocker)
            .tool(report_artifact);
    }

    if let Some(control) = process_control {
        let list_control = control.clone();
        let process_capabilities =
            ToolBuilder::new("process_capabilities")
                .description("List process capabilities and actions declared for this exact run.")
                .read_only()
                .no_params_handler(move || {
                    let control = list_control.clone();
                    async move {
                        json_result(serde_json::json!({ "capabilities": control.descriptors() }))
                    }
                })
                .build();
        let invoke_process_action = ToolBuilder::new("invoke_process_action")
            .description(
                "Invoke one declared process action. The host enforces the run's immutable grants.",
            )
            .handler(move |args: ProcessActionArgs| {
                let control = control.clone();
                async move {
                    let capability = match ProcessCapabilityId::new(args.capability) {
                        Ok(value) => value,
                        Err(error) => return Ok(CallToolResult::error(error.to_string())),
                    };
                    let action = match ProcessActionId::new(args.action) {
                        Ok(value) => value,
                        Err(error) => return Ok(CallToolResult::error(error.to_string())),
                    };
                    match control.invoke(&capability, action, args.input).await {
                        Ok(value) => json_result(value),
                        Err(error) => Ok(CallToolResult::error(error.to_string())),
                    }
                }
            })
            .build();
        router = router
            .tool(process_capabilities)
            .tool(invoke_process_action);
    }

    router
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
    use std::collections::BTreeSet;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use serde_json::json;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::sync::Notify;
    use tower_mcp::TestClient;

    use super::*;
    use roba_core::{
        AgentSpec, AuthorityGrantId, CompletionPolicy, EventSink, MissionPolicy, ProcessActionId,
        ProcessActionRequest, ProcessActionSpec, ProcessCapability, ProcessCapabilityDescriptor,
        ProcessFuture, Provider, ProviderCapabilities, ProviderContext, ProviderError,
        ProviderFuture, ProviderId, Roba, Run, RunOutcome, RunSpec, SessionHandle, TurnRequest,
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

    struct RecordingProcess {
        calls: Arc<AtomicUsize>,
    }

    impl ProcessCapability for RecordingProcess {
        fn descriptor(&self) -> ProcessCapabilityDescriptor {
            ProcessCapabilityDescriptor {
                id: ProcessCapabilityId::new("test/recorder").unwrap(),
                description: "record deterministic values".to_string(),
                required_grants: BTreeSet::from([AuthorityGrantId::new("test/record").unwrap()]),
                actions: vec![ProcessActionSpec {
                    id: ProcessActionId::new("record").unwrap(),
                    description: "record one value".to_string(),
                    input_schema: json!({
                        "type": "object",
                        "required": ["turn"],
                        "properties": {"turn": {"type": "integer"}}
                    }),
                    destructive: false,
                }],
                instructions: vec!["Record each process phase through Roba.".to_string()],
            }
        }

        fn invoke<'a>(&'a self, request: ProcessActionRequest) -> ProcessFuture<'a> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move {
                Ok(json!({
                    "run_id": request.run_id,
                    "action": request.action,
                    "input": request.input,
                }))
            })
        }
    }

    #[derive(Debug, Clone)]
    struct ProviderThatCallsProcessMcp {
        started: Arc<Notify>,
        release: Arc<Notify>,
        turns: Arc<AtomicUsize>,
    }

    impl Provider for ProviderThatCallsProcessMcp {
        fn id(&self) -> ProviderId {
            ProviderId::new("process-fake").unwrap()
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
            context: ProviderContext,
            _events: &'a dyn EventSink,
        ) -> ProviderFuture<'a> {
            Box::pin(async move {
                let turn = self.turns.fetch_add(1, Ordering::SeqCst) + 1;
                let control = context
                    .process_control()
                    .cloned()
                    .expect("declared process control");
                assert_eq!(control.descriptors()[0].id.as_str(), "test/recorder");
                let mut client = TestClient::from_router(internal_router(None, Some(control)));
                client.initialize().await;
                let capabilities = client
                    .call_tool_json("process_capabilities", json!({}))
                    .await;
                assert_eq!(capabilities["capabilities"][0]["id"], "test/recorder");
                let result = client
                    .call_tool_json(
                        "invoke_process_action",
                        json!({
                            "capability": "test/recorder",
                            "action": "record",
                            "input": {"turn": turn}
                        }),
                    )
                    .await;
                assert_eq!(result["input"]["turn"], turn);
                if turn == 1 {
                    self.started.notify_one();
                    self.release.notified().await;
                }
                Ok(RunOutcome {
                    output: request.prompt.into_inner(),
                    session: Some(SessionHandle {
                        provider: ProviderId::new("process-fake").unwrap(),
                        id: "process-session".to_string(),
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
    async fn declared_process_capability_is_private_and_identical_on_open_and_resume() {
        let calls = Arc::new(AtomicUsize::new(0));
        let provider = ProviderThatCallsProcessMcp {
            started: Arc::new(Notify::new()),
            release: Arc::new(Notify::new()),
            turns: Arc::new(AtomicUsize::new(0)),
        };
        let mut roba = Roba::new();
        roba.register(WorkerMcpProvider::new(provider.clone()))
            .unwrap();
        roba.register_capability(RecordingProcess {
            calls: calls.clone(),
        })
        .unwrap();
        let mut spec = RunSpec::suspended(AgentSpec::new(ProviderId::new("process-fake").unwrap()))
            .with_prompt(Prompt::new("open").unwrap());
        spec.execution.workers = WorkerPolicy {
            max_workers: 1,
            max_depth: 1,
        };
        spec.mission = MissionPolicy::new(
            [ProcessCapabilityId::new("test/recorder").unwrap()],
            [AuthorityGrantId::new("test/record").unwrap()],
            CompletionPolicy::RootTerminal,
        )
        .unwrap();
        let run = roba.create_run(spec).unwrap();

        run.begin().await.unwrap();
        provider.started.notified().await;
        let worker = run
            .handle()
            .spawn_inherited(Prompt::new("child").unwrap())
            .await
            .unwrap();
        assert_eq!(worker.spec().mission, run.spec().mission);
        assert_eq!(worker.wait().await.state, roba_core::RunState::Completed);
        run.handle()
            .steer(Prompt::new("resume").unwrap())
            .await
            .unwrap();
        provider.release.notify_one();
        let terminal = run.handle().wait().await;
        assert_eq!(terminal.state, roba_core::RunState::Completed);
        assert_eq!(provider.turns.load(Ordering::SeqCst), 3);
        assert_eq!(calls.load(Ordering::SeqCst), 3);

        let mission = run.handle().mission().await;
        assert_eq!(
            mission
                .authority
                .process
                .capabilities()
                .iter()
                .next()
                .unwrap()
                .as_str(),
            "test/recorder"
        );
        assert_eq!(
            mission.authority.process_capabilities[0].actions[0].input_schema["properties"]["turn"]
                ["type"],
            "integer"
        );

        let mut public = TestClient::from_router(router(run.handle()));
        public.initialize().await;
        let error = public
            .call_tool_expect_error("invoke_process_action", json!({}))
            .await;
        assert!(!error.to_string().is_empty());
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
        assert!(before["created_at_unix_ms"].is_u64());
        assert!(before.get("started_at_unix_ms").is_none());
        let mission = client.call_tool_json("mission", json!({})).await;
        assert_eq!(mission["root"]["state"], "suspended");
        assert!(mission["workers"].as_array().unwrap().is_empty());
        assert!(
            mission["claims"]["work_items"]
                .as_array()
                .unwrap()
                .is_empty()
        );

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
        assert!(terminal["started_at_unix_ms"].is_u64());
        assert!(terminal["finished_at_unix_ms"].is_u64());
        assert!(terminal["elapsed_ms"].is_u64());

        let first = client
            .call_tool_json("events", json!({"after": 0, "limit": 1}))
            .await;
        assert_eq!(first["events"].as_array().unwrap().len(), 1);
        assert_eq!(first["events"][0]["run_id"], 1);
        assert!(first["events"][0]["occurred_at_unix_ms"].is_u64());
        assert!(!first["truncated"].as_bool().unwrap());
        let cursor = first["next_sequence"].as_u64().unwrap();

        let rest = client
            .call_tool_json("events", json!({"after": cursor, "limit": 256}))
            .await;
        assert!(!rest["events"].as_array().unwrap().is_empty());
        assert_eq!(
            rest["events"].as_array().unwrap().last().unwrap()["event"]["kind"],
            "state_changed"
        );
        assert_eq!(
            rest["events"].as_array().unwrap().last().unwrap()["event"]["state"],
            "completed"
        );
        assert!(rest["terminal"].as_bool().unwrap());

        let caught_up = client
            .call_tool_json(
                "events",
                json!({"after": rest["next_sequence"], "wait_ms": 1}),
            )
            .await;
        assert!(caught_up["events"].as_array().unwrap().is_empty());
        assert!(caught_up["terminal"].as_bool().unwrap());
    }

    #[tokio::test]
    async fn mcp_event_long_poll_is_bounded_and_validated() {
        let run = Run::new(
            RunSpec::suspended(AgentSpec::new(ProviderId::new("fake").unwrap())),
            Arc::new(FakeProvider),
        )
        .unwrap();
        let mut client = TestClient::from_router(router(run.handle()));
        client.initialize().await;

        let empty = client.call_tool_json("events", json!({"wait_ms": 1})).await;
        assert!(empty["events"].as_array().unwrap().is_empty());
        assert!(!empty["terminal"].as_bool().unwrap());

        let bad_limit = client
            .call_tool_expect_error("events", json!({"limit": 0}))
            .await;
        assert!(bad_limit.to_string().contains("between 1 and 256"));
        let bad_wait = client
            .call_tool_expect_error("events", json!({"wait_ms": 30_001}))
            .await;
        assert!(bad_wait.to_string().contains("between 0 and 30000"));

        let future_cursor = client
            .call_tool_expect_error("events", json!({"after": 1, "wait_ms": 1}))
            .await;
        assert!(
            future_cursor
                .to_string()
                .contains("event cursor 1 is ahead of newest sequence 0")
        );
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

        let events = client.call_tool_json("events", json!({"limit": 256})).await;
        assert!(
            events["events"]
                .as_array()
                .unwrap()
                .iter()
                .any(|record| record["run_id"] == 2)
        );

        let refusal = client
            .call_tool_expect_error("spawn_worker", json!({"text": "second"}))
            .await;
        assert!(refusal.to_string().contains("maximum of 1 workers"));

        provider.release_root.notify_one();
        let terminal = run.handle().wait().await;
        assert_eq!(
            terminal.state,
            roba_core::RunState::Completed,
            "{terminal:?}"
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
                    let mut client = TestClient::from_router(internal_router(Some(control), None));
                    client.initialize().await;
                    client
                        .call_tool(
                            "report_work_item",
                            json!({
                                "id": "issue-1",
                                "title": "Implement issue 1",
                                "state": "in_progress"
                            }),
                        )
                        .await;
                    client
                        .call_tool(
                            "report_artifact",
                            json!({
                                "id": "branch-1",
                                "artifact_kind": "branch",
                                "reference": "agent/issue-1"
                            }),
                        )
                        .await;
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
        let terminal = run.handle().wait().await;
        assert_eq!(
            terminal.state,
            roba_core::RunState::Completed,
            "{terminal:?}"
        );
        let workers = run.handle().workers();
        assert_eq!(workers.len(), 1);
        assert_eq!(workers[0].parent_id, run.id());
        assert!(workers[0].run.is_terminal());
        let mission = run.handle().mission().await;
        assert_eq!(mission.claims.work_items.len(), 1);
        assert_eq!(mission.claims.work_items[0].id, "issue-1");
        assert_eq!(mission.claims.artifacts.len(), 1);
    }
}
