use std::future::Future;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use roba_core::{
    AgentSpec, EventSink, FailureKind as CoreFailureKind, Provider, ProviderCapabilities,
    ProviderError, ProviderFuture, ProviderId, Roba, RunFailureDetails, RunOutcome, RunSpec,
    SessionHandle as CoreSessionHandle, SessionSpec, TurnRequest,
};
use roba_mcp::{
    AGENT_EVENT_CAPACITY, AGENT_TURN_TOOL, AgentEvent, AgentState, AgentTurnResult, OperationId,
    router,
};
use serde_json::{Value, json};
use tokio::sync::{Barrier, Semaphore};
use tower_mcp::client::TaskAwareCallToolOutcome;
use tower_mcp::{
    CallToolResult, ChannelTransport, McpClient, ProtocolSupport, TaskObject, TaskStatus,
    TaskSupportMode,
};

const FINAL_PROTOCOL: &str = "2026-07-28";
const LEGACY_PROTOCOL: &str = "2025-11-25";
const TEST_TIMEOUT: Duration = Duration::from_secs(5);
const TASK_OPERATION_META_KEY: &str = "com.github.joshrotenberg.roba/operation";
const FIVE_MINUTES_MS: u64 = 300_000;

struct CreatedAgentTask {
    task_id: String,
    operation_id: OperationId,
}

struct FakeState {
    calls: AtomicUsize,
    requests: Mutex<Vec<TurnRequest>>,
    started: Semaphore,
    release: Semaphore,
    cancelled_executions: AtomicUsize,
}

impl Default for FakeState {
    fn default() -> Self {
        Self {
            calls: AtomicUsize::new(0),
            requests: Mutex::new(Vec::new()),
            started: Semaphore::new(0),
            release: Semaphore::new(0),
            cancelled_executions: AtomicUsize::new(0),
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
            self.state
                .cancelled_executions
                .fetch_add(1, Ordering::SeqCst);
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
        _events: &'a dyn EventSink,
    ) -> ProviderFuture<'a> {
        let state = Arc::clone(&self.state);
        state.calls.fetch_add(1, Ordering::SeqCst);
        state
            .requests
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(request.clone());

        Box::pin(async move {
            let text = request.prompt.as_str().to_owned();
            if text == "panic" {
                panic!("intentional Task-backed fake provider panic");
            }
            if text.starts_with("hold") || text.starts_with("race") {
                let mut execution = HeldExecution {
                    state: Arc::clone(&state),
                    completed: false,
                };
                state.started.add_permits(1);
                state
                    .release
                    .acquire()
                    .await
                    .expect("test release semaphore remains open")
                    .forget();
                execution.completed = true;
            }

            if text == "fail" {
                return Err(ProviderError::new(
                    CoreFailureKind::MaxTurns,
                    "provider turn limit reached",
                )
                .with_details(RunFailureDetails {
                    session: Some(core_session("stable-session")),
                    provider_turns: Some(7),
                    ..Default::default()
                }));
            }

            let session = match request.spec.execution.session {
                SessionSpec::Fresh => core_session("stable-session"),
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
    ProviderId::new("phase3-task-fake").expect("static provider id is valid")
}

fn core_session(id: &str) -> CoreSessionHandle {
    CoreSessionHandle {
        provider: fake_provider_id(),
        id: id.to_owned(),
    }
}

fn test_agent() -> (roba_mcp::AgentInstance, Arc<FakeState>) {
    let state = Arc::new(FakeState::default());
    let mut runtime = Roba::new();
    runtime
        .register(FakeProvider {
            state: Arc::clone(&state),
        })
        .expect("fake provider registration succeeds");
    let agent = roba_mcp::AgentInstance::new(
        runtime,
        RunSpec::suspended(AgentSpec::new(fake_provider_id())),
    )
    .expect("test agent is valid");
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

async fn connect_final(agent: roba_mcp::AgentInstance, tasks: bool) -> Arc<McpClient> {
    let builder = McpClient::builder().protocol_support(
        ProtocolSupport::try_new([FINAL_PROTOCOL]).expect("final protocol is compiled"),
    );
    let builder = if tasks { builder.with_tasks() } else { builder };
    let client = bounded(builder.connect_simple(ChannelTransport::new(router(agent))))
        .await
        .expect("final-protocol ChannelTransport connects");
    bounded(client.discover("roba-task-test", "0"))
        .await
        .expect("final server discovery succeeds");
    assert_eq!(
        client.selected_protocol_version().await.as_deref(),
        Some(FINAL_PROTOCOL)
    );
    Arc::new(client)
}

async fn connect_legacy(agent: roba_mcp::AgentInstance) -> Arc<McpClient> {
    let client = bounded(McpClient::connect(ChannelTransport::new(router(agent))))
        .await
        .expect("legacy ChannelTransport connects");
    let initialized = bounded(client.initialize("roba-legacy-task-test", "0"))
        .await
        .expect("legacy initialize succeeds");
    assert_eq!(initialized.protocol_version, LEGACY_PROTOCOL);
    Arc::new(client)
}

async fn close(client: Arc<McpClient>) {
    let client = Arc::into_inner(client).expect("all test client clones were dropped");
    bounded(client.shutdown())
        .await
        .expect("MCP client shuts down cleanly");
}

async fn create_task(client: &McpClient, text: &str) -> CreatedAgentTask {
    let created = bounded(client.call_tool_as_task(AGENT_TURN_TOOL, json!({"text": text}), None))
        .await
        .expect("task-aware agent.turn returns a task");
    assert!(
        created.task.ttl.expect("active task publishes a TTL") > FIVE_MINUTES_MS,
        "an admitted active task outlives the settled-task retention window"
    );
    CreatedAgentTask {
        task_id: created.task.task_id,
        operation_id: operation_id_from_meta(
            created
                .meta
                .as_ref()
                .expect("admitted task publishes operation metadata"),
        ),
    }
}

async fn terminal_task(client: &McpClient, task_id: &str) -> TaskObject {
    bounded(async {
        loop {
            let task = client
                .task_get(task_id)
                .await
                .expect("tasks/get returns the task");
            if task.status.is_terminal() {
                return task;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
}

async fn take_started(state: &FakeState) {
    bounded(state.started.acquire())
        .await
        .expect("start semaphore remains open")
        .forget();
}

async fn wait_for_cancelled_executions(state: &FakeState, expected: usize) {
    bounded(async {
        while state.cancelled_executions.load(Ordering::SeqCst) < expected {
            tokio::task::yield_now().await;
        }
    })
    .await;
}

fn typed(result: &CallToolResult) -> AgentTurnResult {
    serde_json::from_value(
        result
            .structured_content
            .clone()
            .expect("agent result includes structured content"),
    )
    .expect("structured content matches AgentTurnResult")
}

fn operation_id_from_meta(meta: &Value) -> OperationId {
    serde_json::from_value(meta[TASK_OPERATION_META_KEY]["operationId"].clone())
        .expect("task metadata contains a typed Roba operation id")
}

fn task_operation_id(task: &TaskObject) -> OperationId {
    operation_id_from_meta(
        task.meta
            .as_ref()
            .expect("task view retains operation metadata"),
    )
}

fn normalized_structured(result: &CallToolResult) -> Value {
    let mut value = result
        .structured_content
        .clone()
        .expect("agent result includes structured content");
    let object = value
        .as_object_mut()
        .expect("AgentTurnResult serializes as an object");
    object.remove("operation_id");
    if let Some(run) = object.get_mut("run").and_then(Value::as_object_mut) {
        for field in [
            "created_at_unix_ms",
            "started_at_unix_ms",
            "finished_at_unix_ms",
            "elapsed_ms",
        ] {
            run.remove(field);
        }
    }
    value
}

fn assert_result_parity(synchronous: &CallToolResult, task: &CallToolResult) {
    assert_eq!(task.is_error, synchronous.is_error);
    assert_eq!(
        serde_json::to_value(&task.content).expect("task content serializes"),
        serde_json::to_value(&synchronous.content).expect("sync content serializes")
    );
    assert_eq!(
        normalized_structured(task),
        normalized_structured(synchronous)
    );
}

#[tokio::test]
async fn synchronous_and_task_paths_have_success_and_typed_failure_parity() {
    let (synchronous_agent, _) = test_agent();
    let (task_agent, _) = test_agent();
    let synchronous_client = connect_final(synchronous_agent, false).await;
    let task_client = connect_final(task_agent, true).await;

    for (text, is_error) in [("succeed", false), ("fail", true)] {
        let synchronous = match bounded(synchronous_client.call_tool_once_task_aware(
            AGENT_TURN_TOOL,
            json!({"text": text}),
            None,
            None,
        ))
        .await
        .expect("synchronous agent.turn returns")
        {
            TaskAwareCallToolOutcome::Complete(result) => result,
            other => panic!("non-Task client did not use the fallback: {other:?}"),
        };

        let created = create_task(&task_client, text).await;
        let task = terminal_task(&task_client, &created.task_id).await;
        assert_eq!(task.status, TaskStatus::Completed);
        assert_eq!(task_operation_id(&task), created.operation_id);
        assert!(task.error.is_none());
        let task_result = task.result.expect("completed task carries its tool result");
        assert_eq!(synchronous.is_error, is_error);
        assert_result_parity(&synchronous, &task_result);

        match (text, typed(&task_result)) {
            ("succeed", AgentTurnResult::Completed { .. })
            | ("fail", AgentTurnResult::Failed { .. }) => {}
            (_, other) => panic!("unexpected typed task result: {other:?}"),
        }
    }

    close(synchronous_client).await;
    close(task_client).await;
}

#[tokio::test]
async fn legacy_task_path_completes_cancels_drains_and_reuses_the_agent() {
    let (agent, state) = test_agent();
    let client = connect_legacy(agent.clone()).await;

    let tools = bounded(client.list_tools())
        .await
        .expect("legacy client lists tools");
    let turn = tools
        .tools
        .iter()
        .find(|tool| tool.name == AGENT_TURN_TOOL)
        .expect("legacy client discovers agent.turn");
    assert_eq!(
        turn.execution
            .as_ref()
            .and_then(|execution| execution.task_support),
        Some(TaskSupportMode::Optional)
    );

    let completed_created = create_task(&client, "legacy-complete").await;
    let completed = terminal_task(&client, &completed_created.task_id).await;
    assert_eq!(completed.status, TaskStatus::Completed);
    assert_eq!(
        task_operation_id(&completed),
        completed_created.operation_id
    );
    assert!(matches!(
        typed(
            completed
                .result
                .as_ref()
                .expect("legacy completed task carries a result")
        ),
        AgentTurnResult::Completed { operation_id, .. }
            if operation_id == completed_created.operation_id
    ));

    let cancelled_created = create_task(&client, "hold-legacy-cancel").await;
    take_started(&state).await;
    assert_eq!(
        agent.snapshot().await.current_operation_id,
        Some(cancelled_created.operation_id)
    );
    bounded(client.task_cancel(
        &cancelled_created.task_id,
        Some("legacy cancellation".to_owned()),
    ))
    .await
    .expect("legacy tasks/cancel is acknowledged");
    let cancelled = terminal_task(&client, &cancelled_created.task_id).await;
    assert_eq!(cancelled.status, TaskStatus::Cancelled);
    assert_eq!(
        task_operation_id(&cancelled),
        cancelled_created.operation_id
    );
    wait_for_cancelled_executions(&state, 1).await;
    assert_eq!(agent.snapshot().await.state, AgentState::Idle);

    let reused_created = create_task(&client, "legacy-after-cancel").await;
    let reused = terminal_task(&client, &reused_created.task_id).await;
    assert_eq!(reused.status, TaskStatus::Completed);
    assert!(matches!(
        typed(
            reused
                .result
                .as_ref()
                .expect("reused legacy task carries a result")
        ),
        AgentTurnResult::Completed { operation_id, .. }
            if operation_id == reused_created.operation_id
                && operation_id > cancelled_created.operation_id
    ));
    assert_eq!(state.calls.load(Ordering::SeqCst), 3);

    close(client).await;
}

#[tokio::test]
async fn task_cancellation_drains_the_exact_operation_and_leaves_the_agent_reusable() {
    let (agent, state) = test_agent();
    let client = connect_final(agent.clone(), true).await;

    let created = create_task(&client, "hold-cancel").await;
    take_started(&state).await;
    let operation_id = agent
        .snapshot()
        .await
        .current_operation_id
        .expect("the held task has an admitted operation");
    assert_eq!(created.operation_id, operation_id);

    bounded(client.task_cancel(&created.task_id, None))
        .await
        .expect("tasks/cancel is acknowledged");
    let cancelled = terminal_task(&client, &created.task_id).await;
    assert_eq!(cancelled.status, TaskStatus::Cancelled);
    assert_eq!(task_operation_id(&cancelled), operation_id);
    assert!(cancelled.result.is_none());
    assert!(cancelled.error.is_none());
    wait_for_cancelled_executions(&state, 1).await;

    let settled = agent.snapshot().await;
    assert_eq!(settled.state, AgentState::Idle);
    assert_eq!(settled.current_operation_id, None);
    match settled
        .latest_turn
        .expect("cancelled operation is retained")
    {
        AgentTurnResult::Cancelled {
            operation_id: actual,
            ..
        } => assert_eq!(actual, operation_id),
        other => panic!("unexpected cancelled settlement: {other:?}"),
    }
    let events = agent
        .event_page(0, AGENT_EVENT_CAPACITY)
        .await
        .expect("agent event history is readable");
    assert!(events.events.iter().any(|record| {
        record.operation_id == operation_id
            && matches!(&record.event, AgentEvent::OperationSettled { .. })
    }));

    let next_created = create_task(&client, "after-cancel").await;
    let next = terminal_task(&client, &next_created.task_id).await;
    assert_eq!(next.status, TaskStatus::Completed);
    assert_eq!(task_operation_id(&next), next_created.operation_id);
    assert!(matches!(
        typed(next.result.as_ref().expect("completed task has a result")),
        AgentTurnResult::Completed { operation_id: next, .. } if next > operation_id
    ));
    assert_eq!(state.calls.load(Ordering::SeqCst), 2);

    close(client).await;
}

#[tokio::test]
async fn task_backed_provider_panic_is_a_typed_completed_failure_and_agent_reuses() {
    let (agent, state) = test_agent();
    let client = connect_final(agent.clone(), true).await;

    let panicked_created = create_task(&client, "panic").await;
    let panicked = terminal_task(&client, &panicked_created.task_id).await;
    assert_eq!(panicked.status, TaskStatus::Completed);
    assert_eq!(task_operation_id(&panicked), panicked_created.operation_id);
    assert!(panicked.error.is_none());
    let failure = panicked
        .result
        .as_ref()
        .expect("provider panic produces a typed tool result");
    assert!(failure.is_error);
    assert!(matches!(
        typed(failure),
        AgentTurnResult::Failed { operation_id, .. }
            if operation_id == panicked_created.operation_id
    ));
    let snapshot = agent.snapshot().await;
    assert_eq!(snapshot.state, AgentState::Idle);
    assert!(matches!(
        snapshot.latest_turn,
        Some(AgentTurnResult::Failed { operation_id, .. })
            if operation_id == panicked_created.operation_id
    ));

    let reused_created = create_task(&client, "after-panic").await;
    let reused = terminal_task(&client, &reused_created.task_id).await;
    assert_eq!(reused.status, TaskStatus::Completed);
    assert!(matches!(
        typed(
            reused
                .result
                .as_ref()
                .expect("post-panic Task carries a result")
        ),
        AgentTurnResult::Completed { operation_id, .. }
            if operation_id == reused_created.operation_id
                && operation_id > panicked_created.operation_id
    ));
    assert_eq!(state.calls.load(Ordering::SeqCst), 2);

    close(client).await;
}

#[tokio::test]
async fn stale_task_cancellation_cannot_affect_a_later_operation() {
    let (agent, state) = test_agent();
    let client = connect_final(agent.clone(), true).await;

    let stale = create_task(&client, "hold-first").await;
    take_started(&state).await;
    bounded(client.task_cancel(&stale.task_id, None))
        .await
        .expect("first task cancellation is acknowledged");
    assert_eq!(
        terminal_task(&client, &stale.task_id).await.status,
        TaskStatus::Cancelled
    );
    wait_for_cancelled_executions(&state, 1).await;

    let later_created = create_task(&client, "hold-second").await;
    take_started(&state).await;
    let later_operation_id = agent
        .snapshot()
        .await
        .current_operation_id
        .expect("later operation is active");
    assert_eq!(later_created.operation_id, later_operation_id);

    bounded(client.task_cancel(&stale.task_id, None))
        .await
        .expect("final Tasks idempotently acknowledges terminal cancellation");
    let stale_view = bounded(client.task_get(&stale.task_id))
        .await
        .expect("stale task remains readable");
    assert_eq!(stale_view.status, TaskStatus::Cancelled);
    assert_eq!(task_operation_id(&stale_view), stale.operation_id);
    tokio::task::yield_now().await;
    let still_running = agent.snapshot().await;
    assert_eq!(still_running.state, AgentState::Running);
    assert_eq!(still_running.current_operation_id, Some(later_operation_id));
    assert_eq!(state.cancelled_executions.load(Ordering::SeqCst), 1);

    state.release.add_permits(1);
    let later = terminal_task(&client, &later_created.task_id).await;
    assert_eq!(later.status, TaskStatus::Completed);
    assert!(matches!(
        typed(later.result.as_ref().expect("later task has a result")),
        AgentTurnResult::Completed { operation_id, .. } if operation_id == later_operation_id
    ));
    assert_eq!(state.cancelled_executions.load(Ordering::SeqCst), 1);

    close(client).await;
}

#[tokio::test]
async fn task_completion_and_cancellation_races_converge_once() {
    let (agent, state) = test_agent();
    let client = connect_final(agent.clone(), true).await;

    for round in 0..8 {
        let created = create_task(&client, &format!("race-{round}")).await;
        take_started(&state).await;
        let operation_id = agent
            .snapshot()
            .await
            .current_operation_id
            .expect("racing task has an admitted operation");
        assert_eq!(created.operation_id, operation_id);

        let barrier = Arc::new(Barrier::new(3));
        let cancel_barrier = Arc::clone(&barrier);
        let cancel_client = Arc::clone(&client);
        let cancel_task_id = created.task_id.clone();
        let cancellation = tokio::spawn(async move {
            cancel_barrier.wait().await;
            cancel_client.task_cancel(&cancel_task_id, None).await
        });
        let release_barrier = Arc::clone(&barrier);
        let release_state = Arc::clone(&state);
        let completion = tokio::spawn(async move {
            release_barrier.wait().await;
            release_state.release.add_permits(1);
        });
        barrier.wait().await;
        let _ = bounded(cancellation)
            .await
            .expect("cancellation racer joins");
        bounded(completion).await.expect("completion racer joins");

        let task = terminal_task(&client, &created.task_id).await;
        assert_eq!(task_operation_id(&task), operation_id);
        assert!(matches!(
            task.status,
            TaskStatus::Completed | TaskStatus::Cancelled
        ));
        if task.status == TaskStatus::Completed {
            assert!(task.result.is_some());
        } else {
            assert!(task.result.is_none());
        }

        let snapshot = agent.snapshot().await;
        assert_eq!(snapshot.state, AgentState::Idle);
        assert_eq!(
            snapshot
                .latest_turn
                .as_ref()
                .and_then(AgentTurnResult::operation_id),
            Some(operation_id)
        );
        let page = agent
            .event_page(0, AGENT_EVENT_CAPACITY)
            .await
            .expect("agent event history is readable");
        assert_eq!(
            page.events
                .iter()
                .filter(|record| {
                    record.operation_id == operation_id
                        && matches!(&record.event, AgentEvent::OperationSettled { .. })
                })
                .count(),
            1,
            "round {round} published exactly one agent settlement"
        );
    }

    assert_eq!(state.calls.load(Ordering::SeqCst), 8);
    close(client).await;
}

#[tokio::test]
async fn final_non_task_client_discovers_and_calls_the_synchronous_fallback() {
    let (agent, state) = test_agent();
    let client = connect_final(agent, false).await;

    let tools = bounded(client.list_tools())
        .await
        .expect("non-Task final client lists tools");
    assert!(
        tools.tools.iter().any(|tool| tool.name == AGENT_TURN_TOOL),
        "optional agent.turn remains visible without Tasks negotiation"
    );

    let result = match bounded(client.call_tool_once_task_aware(
        AGENT_TURN_TOOL,
        json!({"text": "fallback"}),
        None,
        None,
    ))
    .await
    .expect("non-Task final client calls agent.turn")
    {
        TaskAwareCallToolOutcome::Complete(result) => result,
        other => panic!("non-Task client unexpectedly received {other:?}"),
    };
    assert!(!result.is_error);
    assert!(matches!(typed(&result), AgentTurnResult::Completed { .. }));
    assert_eq!(result.first_text(), Some("answer:fallback"));
    assert_eq!(state.calls.load(Ordering::SeqCst), 1);

    close(client).await;
}
