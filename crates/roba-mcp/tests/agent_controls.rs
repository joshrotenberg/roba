use std::future::Future;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use roba_core::{
    AgentSpec, EventSink, MAX_PENDING_FOLLOW_UPS, Provider, ProviderCapabilities, ProviderError,
    ProviderEvent, ProviderFuture, ProviderId, Roba, RunOutcome, RunSpec,
    SessionHandle as CoreSessionHandle, SessionSpec, TurnRequest,
};
use roba_mcp::{
    AGENT_CONTEXT_ENTRY_TEMPLATE, AGENT_CONTEXT_URI, AGENT_EVENT_CAPACITY, AGENT_EVENTS_TEMPLATE,
    AGENT_EVENTS_URI, AGENT_INTERRUPT_TOOL, AGENT_RESOURCE_URI, AGENT_SHUTDOWN_TOOL,
    AGENT_STEER_TOOL, AGENT_TURN_TOOL, AgentControlRefusalKind, AgentEvent, AgentEventPage,
    AgentInterruptResult, AgentShutdownResult, AgentSnapshot, AgentState, AgentSteerResult,
    AgentTerminalState, AgentTurnResult, OperationSettlement, connect_in_process,
};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tokio::sync::{Barrier, Semaphore};
use tower_mcp::{CallToolResult, McpClient};

const TEST_TIMEOUT: Duration = Duration::from_secs(5);

struct FakeState {
    calls: AtomicUsize,
    requests: Mutex<Vec<TurnRequest>>,
    started: Semaphore,
    release: Semaphore,
    dropped: Semaphore,
}

impl Default for FakeState {
    fn default() -> Self {
        Self {
            calls: AtomicUsize::new(0),
            requests: Mutex::new(Vec::new()),
            started: Semaphore::new(0),
            release: Semaphore::new(0),
            dropped: Semaphore::new(0),
        }
    }
}

struct HeldExecution {
    state: Arc<FakeState>,
    completed: bool,
}

impl Drop for HeldExecution {
    fn drop(&mut self) {
        if !self.completed {
            self.state.dropped.add_permits(1);
        }
    }
}

struct FakeProvider {
    state: Arc<FakeState>,
}

impl Provider for FakeProvider {
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
        request: TurnRequest,
        events: &'a dyn EventSink,
    ) -> ProviderFuture<'a> {
        let state = Arc::clone(&self.state);
        let call = state.calls.fetch_add(1, Ordering::SeqCst) + 1;
        state
            .requests
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(request.clone());

        Box::pin(async move {
            state.started.add_permits(1);
            let text = request.prompt.as_str().to_owned();
            events.emit(ProviderEvent::Warning {
                message: format!("warning:{call}:{text}"),
            });
            events.emit(ProviderEvent::OutputDelta {
                text: format!("delta:{call}:{text}"),
            });

            if text == "panic" {
                panic!("intentional fake provider panic");
            }
            if let Some(count) = text.strip_prefix("burst:") {
                let count: usize = count.parse().expect("test burst count is valid");
                for index in 0..count {
                    events.emit(ProviderEvent::Warning {
                        message: format!("burst:{index}"),
                    });
                }
            }
            if text.starts_with("hold") {
                let mut guard = HeldExecution {
                    state: Arc::clone(&state),
                    completed: false,
                };
                state
                    .release
                    .acquire()
                    .await
                    .expect("test release semaphore remains open")
                    .forget();
                guard.completed = true;
            }

            let session = match request.spec.execution.session {
                SessionSpec::Fresh => core_session(&format!("session-{call}")),
                SessionSpec::Resume { session } => session,
            };
            Ok(RunOutcome {
                output: format!("answer:{text}"),
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

fn fake_provider_id() -> ProviderId {
    ProviderId::new("phase3-fake").expect("static provider id is valid")
}

fn core_session(id: &str) -> CoreSessionHandle {
    CoreSessionHandle {
        provider: fake_provider_id(),
        id: id.to_owned(),
    }
}

fn test_agent(seed: Option<&str>) -> (roba_mcp::AgentInstance, Arc<FakeState>) {
    let state = Arc::new(FakeState::default());
    let mut runtime = Roba::new();
    runtime
        .register(FakeProvider {
            state: Arc::clone(&state),
        })
        .expect("fake provider registration succeeds");
    let mut template = RunSpec::suspended(AgentSpec::new(fake_provider_id()));
    if let Some(seed) = seed {
        template.execution.session = SessionSpec::Resume {
            session: core_session(seed),
        };
    }
    let agent = roba_mcp::AgentInstance::new(runtime, template).expect("test agent is valid");
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

async fn take_started(state: &FakeState) {
    bounded(state.started.acquire())
        .await
        .expect("start semaphore remains open")
        .forget();
}

async fn wait_for_calls(state: &FakeState, expected: usize) {
    bounded(async {
        while state.calls.load(Ordering::SeqCst) < expected {
            tokio::task::yield_now().await;
        }
    })
    .await;
}

async fn connect(agent: roba_mcp::AgentInstance) -> Arc<McpClient> {
    Arc::new(
        bounded(connect_in_process(agent))
            .await
            .expect("in-process MCP client connects"),
    )
}

async fn close(client: Arc<McpClient>) {
    let client = Arc::into_inner(client).expect("all test client clones were dropped");
    bounded(client.shutdown())
        .await
        .expect("MCP client shuts down cleanly");
}

async fn call(client: &McpClient, tool: &str, arguments: Value) -> CallToolResult {
    bounded(client.call_tool(tool, arguments))
        .await
        .expect("tool call returns an MCP result")
}

fn typed<T: DeserializeOwned>(result: &CallToolResult) -> T {
    serde_json::from_value(
        result
            .structured_content
            .clone()
            .expect("typed tool result includes structured content"),
    )
    .expect("structured content matches its published type")
}

async fn read_agent(client: &McpClient) -> AgentSnapshot {
    let resource = bounded(client.read_resource(AGENT_RESOURCE_URI))
        .await
        .expect("agent resource is readable");
    serde_json::from_str(resource.first_text().expect("agent resource is JSON text"))
        .expect("agent resource matches AgentSnapshot")
}

async fn read_events(client: &McpClient, uri: &str) -> AgentEventPage {
    let resource = bounded(client.read_resource(uri))
        .await
        .expect("events resource is readable");
    let content = resource
        .contents
        .first()
        .expect("events resource has content");
    assert_eq!(content.uri, uri);
    assert_eq!(content.mime_type.as_deref(), Some("application/json"));
    serde_json::from_str(resource.first_text().expect("events resource is JSON text"))
        .expect("events resource matches AgentEventPage")
}

fn terminal_state(result: &AgentTurnResult) -> AgentTerminalState {
    match result {
        AgentTurnResult::Completed { .. } => AgentTerminalState::Completed,
        AgentTurnResult::Failed { .. } => AgentTerminalState::Failed,
        AgentTurnResult::Cancelled { .. } => AgentTerminalState::Cancelled,
        AgentTurnResult::Refused { refusal } => panic!("expected terminal turn, got {refusal:?}"),
    }
}

fn operation_id(result: &AgentTurnResult) -> u64 {
    result
        .operation_id()
        .expect("terminal result has operation identity")
        .get()
}

fn settled_events(page: &AgentEventPage, operation_id: u64) -> Vec<AgentTerminalState> {
    page.events
        .iter()
        .filter(|record| record.operation_id.get() == operation_id)
        .filter_map(|record| match record.event {
            AgentEvent::OperationSettled { state } => Some(state),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn controls_and_event_resources_publish_complete_schemas() {
    let (agent, state) = test_agent(None);
    let client = connect(agent).await;

    let tools = bounded(client.list_tools())
        .await
        .expect("tools/list succeeds");
    let mut names = tools
        .tools
        .iter()
        .map(|tool| tool.name.as_str())
        .collect::<Vec<_>>();
    names.sort_unstable();
    assert_eq!(
        names,
        [
            AGENT_STEER_TOOL,
            AGENT_INTERRUPT_TOOL,
            AGENT_SHUTDOWN_TOOL,
            AGENT_TURN_TOOL,
        ]
    );

    let tool = |name: &str| {
        tools
            .tools
            .iter()
            .find(|tool| tool.name == name)
            .expect("base tool is advertised")
    };
    let steer = tool(AGENT_STEER_TOOL);
    assert_eq!(steer.input_schema["additionalProperties"], false);
    assert_eq!(
        steer.input_schema["properties"]["operation_id"]["$ref"],
        "#/$defs/OperationId"
    );
    assert_eq!(
        steer.input_schema["$defs"]["OperationId"]["type"],
        "integer"
    );
    assert_eq!(steer.input_schema["properties"]["text"]["pattern"], "\\S");
    let steer_required = steer.input_schema["required"].as_array().unwrap();
    assert!(steer_required.contains(&json!("operation_id")));
    assert!(steer_required.contains(&json!("text")));
    assert!(
        steer
            .output_schema
            .as_ref()
            .unwrap()
            .to_string()
            .contains("queued")
    );
    assert!(
        steer
            .output_schema
            .as_ref()
            .unwrap()
            .to_string()
            .contains("refused")
    );

    let interrupt = tool(AGENT_INTERRUPT_TOOL);
    assert_eq!(interrupt.input_schema["additionalProperties"], false);
    assert_eq!(interrupt.input_schema["required"], json!(["operation_id"]));
    assert!(
        interrupt
            .output_schema
            .as_ref()
            .unwrap()
            .to_string()
            .contains("settled")
    );
    assert!(
        interrupt
            .output_schema
            .as_ref()
            .unwrap()
            .to_string()
            .contains("refused")
    );

    let shutdown = tool(AGENT_SHUTDOWN_TOOL);
    assert_eq!(shutdown.input_schema["type"], "object");
    assert_eq!(shutdown.input_schema["additionalProperties"], false);
    assert!(
        shutdown
            .output_schema
            .as_ref()
            .unwrap()
            .to_string()
            .contains("stopped")
    );

    let resources = bounded(client.list_resources())
        .await
        .expect("resources/list succeeds");
    let events = resources
        .resources
        .iter()
        .find(|resource| resource.uri == AGENT_EVENTS_URI)
        .expect("exact events resource is advertised");
    assert_eq!(events.mime_type.as_deref(), Some("application/json"));
    let context = resources
        .resources
        .iter()
        .find(|resource| resource.uri == AGENT_CONTEXT_URI)
        .expect("context manifest resource is advertised");
    assert_eq!(context.mime_type.as_deref(), Some("application/json"));

    let templates = bounded(client.list_resource_templates())
        .await
        .expect("resources/templates/list succeeds");
    let events_template = templates
        .resource_templates
        .iter()
        .find(|template| template.uri_template == AGENT_EVENTS_TEMPLATE)
        .expect("cursor events template is advertised");
    assert_eq!(
        events_template.mime_type.as_deref(),
        Some("application/json")
    );
    assert_eq!(
        events_template
            .arguments
            .iter()
            .map(|argument| (argument.name.as_str(), argument.required))
            .collect::<Vec<_>>(),
        [("after", false), ("limit", false)]
    );
    let context_template = templates
        .resource_templates
        .iter()
        .find(|template| template.uri_template == AGENT_CONTEXT_ENTRY_TEMPLATE)
        .expect("generation-fenced context entry template is advertised");
    assert_eq!(
        context_template
            .arguments
            .iter()
            .map(|argument| (argument.name.as_str(), argument.required))
            .collect::<Vec<_>>(),
        [("id", true), ("generation", true)]
    );

    let initial = read_events(&client, AGENT_EVENTS_URI).await;
    assert!(initial.events.is_empty());
    assert_eq!(initial.next_sequence, 0);
    assert!(!initial.closed);
    assert_eq!(state.calls.load(Ordering::SeqCst), 0);
    close(client).await;
}

#[tokio::test]
async fn malformed_control_inputs_fail_without_structured_results_or_state_changes() {
    let (agent, state) = test_agent(None);
    let client = connect(agent).await;

    for (tool, arguments) in [
        (AGENT_STEER_TOOL, json!({"operation_id": 1})),
        (
            AGENT_STEER_TOOL,
            json!({"operation_id": "one", "text": "guide"}),
        ),
        (
            AGENT_STEER_TOOL,
            json!({"operation_id": 1, "text": "guide", "extra": true}),
        ),
        (AGENT_INTERRUPT_TOOL, json!({})),
        (
            AGENT_INTERRUPT_TOOL,
            json!({"operation_id": 1, "extra": true}),
        ),
        (AGENT_SHUTDOWN_TOOL, json!({"extra": true})),
    ] {
        let result = call(&client, tool, arguments).await;
        assert!(
            result.is_error,
            "malformed {tool} call must be a tool error"
        );
        assert!(
            result.structured_content.is_none(),
            "Tower rejects malformed {tool} input before the typed handler"
        );
    }

    let snapshot = read_agent(&client).await;
    assert_eq!(snapshot.state, AgentState::Idle);
    assert!(snapshot.current_operation_id.is_none());
    assert!(snapshot.latest_turn.is_none());
    assert_eq!(state.calls.load(Ordering::SeqCst), 0);
    close(client).await;
}

#[tokio::test]
async fn steer_is_targeted_validated_and_runs_a_resumed_second_provider_turn() {
    let (agent, state) = test_agent(None);
    let client = connect(agent.clone()).await;
    let turn_client = Arc::clone(&client);
    let turn = tokio::spawn(async move {
        call(
            &turn_client,
            AGENT_TURN_TOOL,
            json!({
                "text": "hold-steer",
                "overrides": {"model": "operation-model", "limits": {"timeout_secs": 9}}
            }),
        )
        .await
    });
    take_started(&state).await;
    let operation = read_agent(&client)
        .await
        .current_operation_id
        .expect("held operation is published");

    let stale = call(
        &client,
        AGENT_STEER_TOOL,
        json!({"operation_id": operation.get() + 1, "text": "wrong"}),
    )
    .await;
    assert!(stale.is_error);
    match typed::<AgentSteerResult>(&stale) {
        AgentSteerResult::Refused { refusal } => {
            assert_eq!(refusal.kind, AgentControlRefusalKind::OperationMismatch)
        }
        other => panic!("unexpected stale steer: {other:?}"),
    }

    let blank = call(
        &client,
        AGENT_STEER_TOOL,
        json!({"operation_id": operation.get(), "text": "  \n"}),
    )
    .await;
    assert!(blank.is_error);
    match typed::<AgentSteerResult>(&blank) {
        AgentSteerResult::Refused { refusal } => {
            assert_eq!(refusal.kind, AgentControlRefusalKind::InvalidPrompt)
        }
        other => panic!("unexpected blank steer: {other:?}"),
    }

    let queued = call(
        &client,
        AGENT_STEER_TOOL,
        json!({"operation_id": operation.get(), "text": "guidance"}),
    )
    .await;
    assert!(!queued.is_error);
    assert!(matches!(typed(&queued), AgentSteerResult::Queued { .. }));

    state.release.add_permits(1);
    wait_for_calls(&state, 2).await;
    let result = bounded(turn).await.expect("turn task joins");
    let result: AgentTurnResult = typed(&result);
    match result {
        AgentTurnResult::Completed { operation_id, run } => {
            assert_eq!(operation_id, operation);
            assert_eq!(run.metadata.turns_completed, 2);
            assert_eq!(run.outcome.output, "answer:guidance");
        }
        other => panic!("unexpected steered turn result: {other:?}"),
    }
    {
        let requests = state
            .requests
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(requests[1].prompt.as_str(), "guidance");
        assert_eq!(
            requests[0].spec.agent.model.as_deref(),
            Some("operation-model")
        );
        assert_eq!(
            requests[1].spec.agent.model.as_deref(),
            Some("operation-model")
        );
        assert_eq!(requests[1].spec.execution.limits.timeout_secs, Some(9));
        assert_eq!(
            requests[1].spec.execution.session,
            SessionSpec::Resume {
                session: core_session("session-1")
            }
        );
    }
    assert_eq!(read_agent(&client).await.state, AgentState::Idle);
    close(client).await;
}

#[tokio::test]
async fn follow_up_queue_refuses_overflow_without_losing_the_operation() {
    let (agent, state) = test_agent(None);
    let client = connect(agent).await;
    let turn_client = Arc::clone(&client);
    let turn = tokio::spawn(async move {
        call(
            &turn_client,
            AGENT_TURN_TOOL,
            json!({"text": "hold-follow-ups"}),
        )
        .await
    });
    take_started(&state).await;
    let operation = read_agent(&client)
        .await
        .current_operation_id
        .expect("held operation is published");

    for index in 0..MAX_PENDING_FOLLOW_UPS {
        let queued = call(
            &client,
            AGENT_STEER_TOOL,
            json!({"operation_id": operation.get(), "text": format!("next-{index}")}),
        )
        .await;
        assert!(matches!(typed(&queued), AgentSteerResult::Queued { .. }));
    }
    let overflow = call(
        &client,
        AGENT_STEER_TOOL,
        json!({"operation_id": operation.get(), "text": "overflow"}),
    )
    .await;
    assert!(overflow.is_error);
    match typed::<AgentSteerResult>(&overflow) {
        AgentSteerResult::Refused { refusal } => {
            assert_eq!(refusal.kind, AgentControlRefusalKind::QueueFull)
        }
        other => panic!("unexpected overflow result: {other:?}"),
    }

    state.release.add_permits(1);
    wait_for_calls(&state, MAX_PENDING_FOLLOW_UPS + 1).await;
    match typed::<AgentTurnResult>(&bounded(turn).await.expect("turn task joins")) {
        AgentTurnResult::Completed { run, .. } => {
            assert_eq!(
                run.metadata.turns_completed,
                MAX_PENDING_FOLLOW_UPS as u32 + 1
            );
            assert_eq!(
                run.outcome.output,
                format!("answer:next-{}", MAX_PENDING_FOLLOW_UPS - 1)
            );
        }
        other => panic!("unexpected bounded follow-up result: {other:?}"),
    }
    close(client).await;
}

#[tokio::test]
async fn interrupt_targets_drops_settles_before_reply_and_leaves_a_reusable_agent() {
    let (agent, state) = test_agent(None);
    let client = connect(agent).await;
    let turn_client = Arc::clone(&client);
    let turn = tokio::spawn(async move {
        call(
            &turn_client,
            AGENT_TURN_TOOL,
            json!({"text": "hold-interrupt"}),
        )
        .await
    });
    take_started(&state).await;
    let operation = read_agent(&client)
        .await
        .current_operation_id
        .expect("held operation is published");

    let stale = call(
        &client,
        AGENT_INTERRUPT_TOOL,
        json!({"operation_id": operation.get() + 1}),
    )
    .await;
    assert!(stale.is_error);
    assert_eq!(state.dropped.available_permits(), 0);

    let interrupted = call(
        &client,
        AGENT_INTERRUPT_TOOL,
        json!({"operation_id": operation.get()}),
    )
    .await;
    assert!(!interrupted.is_error);
    let interrupt: AgentInterruptResult = typed(&interrupted);
    assert_eq!(
        interrupt,
        AgentInterruptResult::Settled {
            settlement: OperationSettlement {
                operation_id: operation,
                state: AgentTerminalState::Cancelled,
            },
            cancellation_requested: true,
        }
    );
    assert_eq!(state.dropped.available_permits(), 1);
    let snapshot = read_agent(&client).await;
    assert_eq!(snapshot.state, AgentState::Idle);
    assert_eq!(
        snapshot.latest_turn.as_ref().map(operation_id),
        Some(operation.get())
    );

    let original: AgentTurnResult = typed(&bounded(turn).await.expect("turn task joins"));
    assert_eq!(terminal_state(&original), AgentTerminalState::Cancelled);
    assert_eq!(operation_id(&original), operation.get());
    let page = read_events(&client, AGENT_EVENTS_URI).await;
    assert_eq!(
        settled_events(&page, operation.get()),
        [AgentTerminalState::Cancelled]
    );

    let next = call(&client, AGENT_TURN_TOOL, json!({"text": "next"})).await;
    let next: AgentTurnResult = typed(&next);
    assert_eq!(operation_id(&next), operation.get() + 1);
    assert_eq!(terminal_state(&next), AgentTerminalState::Completed);
    close(client).await;
}

#[tokio::test]
async fn interrupted_session_continuity_uses_only_previously_known_evidence() {
    for (seed, expected) in [
        (
            Some("seed-session"),
            SessionSpec::Resume {
                session: core_session("seed-session"),
            },
        ),
        (None, SessionSpec::Fresh),
    ] {
        let (agent, state) = test_agent(seed);
        let client = connect(agent).await;
        let turn_client = Arc::clone(&client);
        let turn = tokio::spawn(async move {
            call(
                &turn_client,
                AGENT_TURN_TOOL,
                json!({"text": "hold-session"}),
            )
            .await
        });
        take_started(&state).await;
        let operation = read_agent(&client).await.current_operation_id.unwrap();
        call(
            &client,
            AGENT_INTERRUPT_TOOL,
            json!({"operation_id": operation.get()}),
        )
        .await;
        let _ = bounded(turn).await.expect("turn task joins");
        call(&client, AGENT_TURN_TOOL, json!({"text": "after"})).await;

        {
            let requests = state
                .requests
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            assert_eq!(requests.len(), 2);
            assert_eq!(requests[0].spec.execution.session, expected);
            assert_eq!(requests[1].spec.execution.session, expected);
        }
        close(client).await;
    }
}

#[tokio::test]
async fn dropping_a_real_mcp_turn_waiter_detaches_until_explicit_interrupt() {
    let (agent, state) = test_agent(None);
    let client = connect(agent).await;
    let turn_client = Arc::clone(&client);
    let waiter = tokio::spawn(async move {
        call(
            &turn_client,
            AGENT_TURN_TOOL,
            json!({"text": "hold-detach"}),
        )
        .await
    });
    take_started(&state).await;
    let operation = read_agent(&client).await.current_operation_id.unwrap();
    waiter.abort();
    assert!(bounded(waiter).await.is_err());
    tokio::task::yield_now().await;

    let running = read_agent(&client).await;
    assert_eq!(running.state, AgentState::Running);
    assert_eq!(running.current_operation_id, Some(operation));
    assert_eq!(state.dropped.available_permits(), 0);

    let interrupted = call(
        &client,
        AGENT_INTERRUPT_TOOL,
        json!({"operation_id": operation.get()}),
    )
    .await;
    assert!(!interrupted.is_error);
    assert_eq!(state.dropped.available_permits(), 1);
    assert_eq!(read_agent(&client).await.state, AgentState::Idle);
    close(client).await;
}

#[tokio::test]
async fn interrupt_completion_race_converges_on_one_terminal_settlement() {
    let (agent, state) = test_agent(None);
    let client = connect(agent).await;
    let turn_client = Arc::clone(&client);
    let turn = tokio::spawn(async move {
        call(&turn_client, AGENT_TURN_TOOL, json!({"text": "hold-race"})).await
    });
    take_started(&state).await;
    let operation = read_agent(&client).await.current_operation_id.unwrap();
    let barrier = Arc::new(Barrier::new(3));
    let release_barrier = Arc::clone(&barrier);
    let release_state = Arc::clone(&state);
    let release = tokio::spawn(async move {
        release_barrier.wait().await;
        release_state.release.add_permits(1);
    });
    let interrupt_barrier = Arc::clone(&barrier);
    let interrupt_client = Arc::clone(&client);
    let interrupt = tokio::spawn(async move {
        interrupt_barrier.wait().await;
        call(
            &interrupt_client,
            AGENT_INTERRUPT_TOOL,
            json!({"operation_id": operation.get()}),
        )
        .await
    });
    barrier.wait().await;
    bounded(release).await.expect("release task joins");
    let turn: AgentTurnResult = typed(&bounded(turn).await.expect("turn task joins"));
    let interrupt: AgentInterruptResult =
        typed(&bounded(interrupt).await.expect("interrupt task joins"));
    let AgentInterruptResult::Settled { settlement, .. } = interrupt else {
        panic!("matching race interrupt must converge on settlement")
    };
    assert_eq!(settlement.operation_id.get(), operation.get());
    assert_eq!(settlement.state, terminal_state(&turn));
    assert_eq!(read_agent(&client).await.state, AgentState::Idle);
    let events = read_events(&client, AGENT_EVENTS_URI).await;
    assert_eq!(settled_events(&events, operation.get()), [settlement.state]);
    close(client).await;
}

#[tokio::test]
async fn shutdown_drains_active_work_is_permanent_and_is_idempotent() {
    let (agent, state) = test_agent(None);
    let client = connect(agent).await;
    let turn_client = Arc::clone(&client);
    let turn = tokio::spawn(async move {
        call(
            &turn_client,
            AGENT_TURN_TOOL,
            json!({"text": "hold-shutdown"}),
        )
        .await
    });
    take_started(&state).await;
    let operation = read_agent(&client).await.current_operation_id.unwrap();

    let stopped = call(&client, AGENT_SHUTDOWN_TOOL, json!({})).await;
    assert!(!stopped.is_error);
    let stopped: AgentShutdownResult = typed(&stopped);
    assert_eq!(
        stopped,
        AgentShutdownResult::Stopped {
            drained: Some(OperationSettlement {
                operation_id: operation,
                state: AgentTerminalState::Cancelled,
            })
        }
    );
    assert_eq!(state.dropped.available_permits(), 1);
    let original: AgentTurnResult = typed(&bounded(turn).await.expect("turn task joins"));
    assert_eq!(terminal_state(&original), AgentTerminalState::Cancelled);
    assert_eq!(read_agent(&client).await.state, AgentState::Stopped);

    let refused = call(&client, AGENT_TURN_TOOL, json!({"text": "too late"})).await;
    assert!(refused.is_error);
    assert!(matches!(
        typed::<AgentTurnResult>(&refused),
        AgentTurnResult::Refused { .. }
    ));
    assert_eq!(state.calls.load(Ordering::SeqCst), 1);
    let again: AgentShutdownResult = typed(&call(&client, AGENT_SHUTDOWN_TOOL, json!({})).await);
    assert_eq!(again, stopped);
    assert!(read_events(&client, AGENT_EVENTS_URI).await.closed);
    close(client).await;
}

#[tokio::test]
async fn shutdown_completion_race_stops_after_exactly_one_operation_settlement() {
    let (agent, state) = test_agent(None);
    let client = connect(agent).await;
    let turn_client = Arc::clone(&client);
    let turn = tokio::spawn(async move {
        call(
            &turn_client,
            AGENT_TURN_TOOL,
            json!({"text": "hold-shutdown-race"}),
        )
        .await
    });
    take_started(&state).await;
    let operation = read_agent(&client).await.current_operation_id.unwrap();
    let barrier = Arc::new(Barrier::new(3));
    let release_barrier = Arc::clone(&barrier);
    let release_state = Arc::clone(&state);
    let release = tokio::spawn(async move {
        release_barrier.wait().await;
        release_state.release.add_permits(1);
    });
    let shutdown_barrier = Arc::clone(&barrier);
    let shutdown_client = Arc::clone(&client);
    let shutdown = tokio::spawn(async move {
        shutdown_barrier.wait().await;
        call(&shutdown_client, AGENT_SHUTDOWN_TOOL, json!({})).await
    });
    barrier.wait().await;
    bounded(release).await.expect("release task joins");
    let turn: AgentTurnResult = typed(&bounded(turn).await.expect("turn task joins"));
    let shutdown: AgentShutdownResult =
        typed(&bounded(shutdown).await.expect("shutdown task joins"));
    if let AgentShutdownResult::Stopped {
        drained: Some(drained),
    } = shutdown
    {
        assert_eq!(drained.operation_id.get(), operation.get());
        assert_eq!(drained.state, terminal_state(&turn));
    }
    assert_eq!(read_agent(&client).await.state, AgentState::Stopped);
    let events = read_events(&client, AGENT_EVENTS_URI).await;
    assert_eq!(
        settled_events(&events, operation.get()),
        [terminal_state(&turn)]
    );
    close(client).await;
}

#[tokio::test]
async fn event_pages_are_global_monotonic_settled_before_reply_and_fail_loudly() {
    let (agent, _state) = test_agent(None);
    let client = connect(agent).await;
    for text in ["one", "two"] {
        let result = call(&client, AGENT_TURN_TOOL, json!({"text": text})).await;
        let result: AgentTurnResult = typed(&result);
        let page = read_events(&client, AGENT_EVENTS_URI).await;
        assert_eq!(
            settled_events(&page, operation_id(&result)),
            [AgentTerminalState::Completed],
            "OperationSettled must be visible before agent.turn replies"
        );
    }

    let page = read_events(
        &client,
        &format!("{AGENT_EVENTS_URI}?after=0&limit={AGENT_EVENT_CAPACITY}"),
    )
    .await;
    assert!(
        page.events
            .windows(2)
            .all(|pair| pair[0].sequence < pair[1].sequence)
    );
    assert_eq!(page.next_sequence, page.events.last().unwrap().sequence);
    for operation in [1, 2] {
        let run_sequences = page
            .events
            .iter()
            .filter(|record| record.operation_id.get() == operation)
            .filter_map(|record| record.run_sequence)
            .collect::<Vec<_>>();
        assert_eq!(run_sequences.first(), Some(&1));
        assert!(run_sequences.windows(2).all(|pair| pair[0] < pair[1]));
        assert_eq!(settled_events(&page, operation).len(), 1);
    }

    let cursor_uri = format!("{AGENT_EVENTS_URI}?after={}&limit=1", page.next_sequence);
    let empty = read_events(&client, &cursor_uri).await;
    assert!(empty.events.is_empty());
    assert_eq!(empty.next_sequence, page.next_sequence);

    for uri in [
        format!("{AGENT_EVENTS_URI}?after={}", page.next_sequence + 1),
        format!("{AGENT_EVENTS_URI}?after=18446744073709551616"),
        format!("{AGENT_EVENTS_URI}?limit=0"),
        format!("{AGENT_EVENTS_URI}?limit={}", AGENT_EVENT_CAPACITY + 1),
        format!("{AGENT_EVENTS_URI}?limit=184467440737095516160"),
        format!("{AGENT_EVENTS_URI}?limit=bad"),
    ] {
        assert!(
            bounded(client.read_resource(&uri)).await.is_err(),
            "invalid event query must fail: {uri}"
        );
    }
    close(client).await;
}

#[tokio::test]
async fn event_eviction_reports_truncation() {
    let (agent, _state) = test_agent(None);
    let client = connect(agent).await;
    let result = call(
        &client,
        AGENT_TURN_TOOL,
        json!({"text": format!("burst:{}", AGENT_EVENT_CAPACITY + 16)}),
    )
    .await;
    assert!(!result.is_error);
    let page = read_events(
        &client,
        &format!("{AGENT_EVENTS_URI}?after=0&limit={AGENT_EVENT_CAPACITY}"),
    )
    .await;
    assert!(page.truncated);
    assert!(page.oldest_sequence.is_some_and(|sequence| sequence > 1));
    close(client).await;
}

#[tokio::test]
async fn provider_panic_settles_failed_and_the_next_turn_recovers() {
    let (agent, state) = test_agent(None);
    let client = connect(agent).await;
    let failed = call(&client, AGENT_TURN_TOOL, json!({"text": "panic"})).await;
    assert!(failed.is_error);
    let failed: AgentTurnResult = typed(&failed);
    match &failed {
        AgentTurnResult::Failed { run, .. } => {
            assert!(run.failure.message.contains("panicked"));
        }
        other => panic!("provider panic must become terminal failure: {other:?}"),
    }
    assert_eq!(read_agent(&client).await.state, AgentState::Idle);
    let events = read_events(&client, AGENT_EVENTS_URI).await;
    assert_eq!(
        settled_events(&events, operation_id(&failed)),
        [AgentTerminalState::Failed]
    );

    let next = call(&client, AGENT_TURN_TOOL, json!({"text": "after-panic"})).await;
    assert!(!next.is_error);
    assert_eq!(state.calls.load(Ordering::SeqCst), 2);
    close(client).await;
}
