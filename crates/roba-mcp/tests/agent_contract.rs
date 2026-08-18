use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use roba_core::{
    AgentSpec, ContextSpec, Cost as CoreCost, Effort as CoreEffort, EventSink,
    FailureKind as CoreFailureKind, PermissionPolicy as CorePermissionPolicy, Provider,
    ProviderCapabilities, ProviderError, ProviderFuture, ProviderId, Roba, RunFailureDetails,
    RunOutcome, RunSpec, SessionHandle as CoreSessionHandle, SessionSpec, TurnRequest,
};
use roba_mcp::{
    AGENT_CONTEXT_ENTRY_TEMPLATE, AGENT_CONTEXT_URI, AGENT_RESOURCE_URI, AGENT_TURN_TOOL,
    AgentBuildError, AgentInstance, AgentRefusalKind, AgentSnapshot, AgentState, AgentStopError,
    AgentTurnResult, AmbientContextPolicy, ContextAudience, ContextContent, ContextDelivery,
    ContextEntrySpec, ContextKind, ContextOrigin, ContextOriginKind, ContextPhase, ContextPlan,
    ContextPrecedence, ContextScope, ContextSnapshot, Effort, FailureKind, PermissionPolicy,
    connect_in_process,
};
use serde_json::json;
use tokio::sync::Semaphore;
use tower_mcp::{CallToolResult, McpClient};

struct FakeState {
    calls: AtomicUsize,
    requests: Mutex<Vec<TurnRequest>>,
    started: Semaphore,
    release: Semaphore,
}

impl Default for FakeState {
    fn default() -> Self {
        Self {
            calls: AtomicUsize::new(0),
            requests: Mutex::new(Vec::new()),
            started: Semaphore::new(0),
            release: Semaphore::new(0),
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
            let text = request.prompt.as_str().to_string();
            if text == "hold" {
                state.started.add_permits(1);
                state
                    .release
                    .acquire()
                    .await
                    .expect("test release semaphore remains open")
                    .forget();
            }
            if text == "wrong-session-success" {
                return Ok(RunOutcome {
                    output: "untrusted answer".to_string(),
                    session: Some(CoreSessionHandle {
                        provider: ProviderId::codex(),
                        id: "wrong-success".to_string(),
                    }),
                    usage: None,
                    cost: None,
                    duration_ms: None,
                    provider_turns: Some(1),
                    structured_output: None,
                });
            }
            if text == "wrong-session-failure" {
                return Err(
                    ProviderError::new(CoreFailureKind::MaxTurns, "untrusted failure")
                        .with_details(RunFailureDetails {
                            session: Some(CoreSessionHandle {
                                provider: ProviderId::codex(),
                                id: "wrong-failure".to_string(),
                            }),
                            provider_turns: Some(9),
                            ..Default::default()
                        }),
                );
            }
            if text == "fail" {
                return Err(ProviderError::new(
                    CoreFailureKind::MaxTurns,
                    "provider turn limit reached",
                )
                .with_details(RunFailureDetails {
                    session: Some(core_session("session-from-failure")),
                    provider_turns: Some(12),
                    ..Default::default()
                }));
            }
            if text == "nan-cost" {
                return Ok(RunOutcome {
                    output: "invalid accounting".to_string(),
                    session: Some(core_session("session-with-invalid-cost")),
                    usage: None,
                    cost: Some(CoreCost::usd(f64::NAN)),
                    duration_ms: None,
                    provider_turns: Some(1),
                    structured_output: None,
                });
            }

            let session = match request.spec.execution.session {
                SessionSpec::Fresh => core_session("session-1"),
                SessionSpec::Resume { session } => session,
            };
            Ok(RunOutcome {
                output: format!("answer:{text}"),
                session: (text != "no-session").then_some(session),
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
    ProviderId::new("fake").expect("static provider id is valid")
}

fn core_session(id: &str) -> CoreSessionHandle {
    CoreSessionHandle {
        provider: fake_provider_id(),
        id: id.to_string(),
    }
}

fn agent_with_session(seed: Option<&str>) -> (AgentInstance, Arc<FakeState>) {
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
    let agent = AgentInstance::new(runtime, template).expect("valid test agent");
    (agent, state)
}

fn typed(result: &CallToolResult) -> AgentTurnResult {
    serde_json::from_value(
        result
            .structured_content
            .clone()
            .expect("agent tool always returns structured content"),
    )
    .expect("structured content matches the published result type")
}

async fn read_agent(client: &McpClient) -> AgentSnapshot {
    let resource = client
        .read_resource(AGENT_RESOURCE_URI)
        .await
        .expect("agent resource is readable");
    assert_eq!(
        resource.contents[0].mime_type.as_deref(),
        Some("application/json")
    );
    serde_json::from_str(resource.first_text().expect("agent resource is text JSON"))
        .expect("agent resource matches the published snapshot type")
}

#[tokio::test]
async fn schema_idle_construction_and_sequential_resume_cross_the_real_mcp_client() {
    let (agent, state) = agent_with_session(None);
    let client = connect_in_process(agent)
        .await
        .expect("in-process client connects");

    let initial = read_agent(&client).await;
    assert_eq!(initial.state, AgentState::Idle);
    assert!(!initial.session_available);
    assert!(initial.current_operation_id.is_none());
    assert!(initial.latest_turn.is_none());
    assert_eq!(state.calls.load(Ordering::SeqCst), 0);

    let tools = client.list_tools().await.expect("tools/list succeeds");
    let turn = tools
        .tools
        .iter()
        .find(|tool| tool.name == AGENT_TURN_TOOL)
        .expect("agent.turn is advertised");
    assert_eq!(turn.input_schema["properties"]["text"]["type"], "string");
    assert_eq!(turn.input_schema["properties"]["text"]["pattern"], "\\S");
    assert_eq!(turn.input_schema["required"][0], "text");
    assert!(turn.input_schema["properties"]["overrides"].is_object());
    assert_eq!(turn.input_schema["additionalProperties"], false);
    let output_schema = turn
        .output_schema
        .as_ref()
        .expect("agent.turn publishes its structured output schema");
    let variants = output_schema["oneOf"]
        .as_array()
        .expect("tagged result schema has one branch per status");
    assert_eq!(variants.len(), 4);
    let mut statuses = variants
        .iter()
        .filter_map(|variant| variant["properties"]["status"]["const"].as_str())
        .collect::<Vec<_>>();
    statuses.sort_unstable();
    assert_eq!(statuses, ["cancelled", "completed", "failed", "refused"]);

    let resources = client
        .list_resources()
        .await
        .expect("resources/list succeeds");
    let agent_resource = resources
        .resources
        .iter()
        .find(|resource| resource.uri == AGENT_RESOURCE_URI)
        .expect("roba://agent is advertised");
    assert_eq!(agent_resource.name, "Roba agent");
    assert_eq!(
        agent_resource.mime_type.as_deref(),
        Some("application/json")
    );

    let first = client
        .call_tool(AGENT_TURN_TOOL, json!({"text": "one"}))
        .await
        .expect("first tool call completes");
    assert!(!first.is_error);
    assert_eq!(first.first_text(), Some("answer:one"));
    match typed(&first) {
        AgentTurnResult::Completed { operation_id, run } => {
            assert_eq!(operation_id.get(), 1);
            assert_eq!(run.outcome.session.unwrap().id, "session-1");
        }
        other => panic!("unexpected first result: {other:?}"),
    }

    let second = client
        .call_tool(AGENT_TURN_TOOL, json!({"text": "two"}))
        .await
        .expect("second tool call completes");
    assert!(!second.is_error);
    assert_eq!(second.first_text(), Some("answer:two"));
    assert_eq!(typed(&second).operation_id().map(|id| id.get()), Some(2));

    {
        let requests = state
            .requests
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].spec.execution.session, SessionSpec::Fresh);
        assert_eq!(
            requests[1].spec.execution.session,
            SessionSpec::Resume {
                session: core_session("session-1")
            }
        );
    }

    let settled = read_agent(&client).await;
    assert_eq!(settled.state, AgentState::Idle);
    assert!(settled.session_available);
    let latest = settled.latest_turn.expect("latest turn retained");
    assert_eq!(latest.operation_id().map(|id| id.get()), Some(2));
    match latest {
        AgentTurnResult::Completed { run, .. } => {
            assert!(
                run.outcome.session.is_none(),
                "resource redacts session ids"
            )
        }
        other => panic!("unexpected latest resource result: {other:?}"),
    }
    client.shutdown().await.expect("client shuts down cleanly");
}

#[tokio::test]
async fn seeded_resume_is_used_on_the_first_turn() {
    let (agent, state) = agent_with_session(Some("seed-session"));
    let client = connect_in_process(agent)
        .await
        .expect("in-process client connects");

    let result = client
        .call_tool(AGENT_TURN_TOOL, json!({"text": "first"}))
        .await
        .expect("seeded turn completes");
    assert!(!result.is_error);
    {
        let requests = state
            .requests
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(
            requests[0].spec.execution.session,
            SessionSpec::Resume {
                session: core_session("seed-session")
            }
        );
    }
    assert!(read_agent(&client).await.session_available);
    client.shutdown().await.expect("client shuts down cleanly");
}

#[tokio::test]
async fn agent_resource_publishes_the_complete_safe_template_policy() {
    let state = Arc::new(FakeState::default());
    let mut runtime = Roba::new();
    runtime
        .register(FakeProvider {
            state: Arc::clone(&state),
        })
        .expect("fake provider registration succeeds");
    let mut template = RunSpec::suspended(AgentSpec::new(fake_provider_id()));
    template.agent.model = Some("test-model".to_string());
    template.agent.effort = Some(CoreEffort::High);
    template.execution.permissions = CorePermissionPolicy::WorkspaceWrite;
    template.execution.tools.allow = vec!["Read".to_string(), "Edit".to_string()];
    template.execution.tools.deny = vec!["Network".to_string()];
    template.execution.limits.max_turns = Some(7);
    template.execution.limits.max_cost_usd = Some(1.5);
    template.execution.limits.timeout_secs = Some(30);
    let agent = AgentInstance::new(runtime, template).expect("valid configured agent");
    let client = connect_in_process(agent)
        .await
        .expect("in-process client connects");

    let snapshot = read_agent(&client).await;
    assert_eq!(snapshot.configuration.provider, "fake");
    assert_eq!(snapshot.configuration.model.as_deref(), Some("test-model"));
    assert_eq!(snapshot.configuration.effort, Some(Effort::High));
    assert_eq!(
        snapshot.configuration.permissions,
        PermissionPolicy::WorkspaceWrite
    );
    assert_eq!(snapshot.configuration.tools.allow, ["Read", "Edit"]);
    assert_eq!(snapshot.configuration.tools.deny, ["Network"]);
    assert_eq!(snapshot.configuration.limits.max_turns, Some(7));
    assert_eq!(snapshot.configuration.limits.max_cost_usd, Some(1.5));
    assert_eq!(snapshot.configuration.limits.timeout_secs, Some(30));
    assert_eq!(state.calls.load(Ordering::SeqCst), 0);
    client.shutdown().await.expect("client shuts down cleanly");
}

#[tokio::test]
async fn turn_overrides_are_operation_local_visible_and_evidenced() {
    let state = Arc::new(FakeState::default());
    let mut runtime = Roba::new();
    runtime
        .register(FakeProvider {
            state: Arc::clone(&state),
        })
        .expect("fake provider registration succeeds");
    let mut template = RunSpec::suspended(AgentSpec::new(fake_provider_id()));
    template.agent.model = Some("default-model".to_string());
    template.agent.effort = Some(CoreEffort::Low);
    template.execution.limits.timeout_secs = Some(30);
    let agent = AgentInstance::new(runtime, template).expect("valid configured agent");
    let client = Arc::new(
        connect_in_process(agent)
            .await
            .expect("in-process client connects"),
    );

    let turn_client = Arc::clone(&client);
    let turn = tokio::spawn(async move {
        turn_client
            .call_tool(
                AGENT_TURN_TOOL,
                json!({
                    "text": "hold",
                    "overrides": {
                        "model": "operation-model",
                        "effort": "high",
                        "limits": {
                            "max_turns": 4,
                            "max_cost_usd": 1.25,
                            "timeout_secs": 5
                        }
                    }
                }),
            )
            .await
            .expect("overridden turn completes")
    });
    tokio::time::timeout(Duration::from_secs(1), state.started.acquire())
        .await
        .expect("provider started before timeout")
        .expect("start semaphore remains open")
        .forget();

    let running = read_agent(&client).await;
    assert_eq!(
        running.configuration.model.as_deref(),
        Some("default-model")
    );
    let active = running
        .active_configuration
        .expect("active operation publishes its effective configuration");
    assert_eq!(active.model.as_deref(), Some("operation-model"));
    assert_eq!(active.effort, Some(Effort::High));
    assert_eq!(active.limits.max_turns, Some(4));
    assert_eq!(active.limits.max_cost_usd, Some(1.25));
    assert_eq!(active.limits.timeout_secs, Some(5));

    state.release.add_permits(1);
    let result = typed(&turn.await.expect("turn task joins"));
    match result {
        AgentTurnResult::Completed { run, .. } => {
            assert_eq!(run.metadata.configuration, active);
        }
        other => panic!("unexpected overridden result: {other:?}"),
    }
    assert!(read_agent(&client).await.active_configuration.is_none());

    client
        .call_tool(AGENT_TURN_TOOL, json!({"text": "default"}))
        .await
        .expect("default turn completes");
    {
        let requests = state
            .requests
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(
            requests[0].spec.agent.model.as_deref(),
            Some("operation-model")
        );
        assert_eq!(requests[0].spec.agent.effort, Some(CoreEffort::High));
        assert_eq!(requests[0].spec.execution.limits.max_turns, Some(4));
        assert_eq!(
            requests[1].spec.agent.model.as_deref(),
            Some("default-model")
        );
        assert_eq!(requests[1].spec.agent.effort, Some(CoreEffort::Low));
        assert_eq!(requests[1].spec.execution.limits.max_turns, None);
    }

    let client = Arc::into_inner(client).expect("all client task clones were dropped");
    client.shutdown().await.expect("client shuts down cleanly");
}

#[tokio::test]
async fn invalid_turn_overrides_refuse_before_provider_work() {
    let (agent, state) = agent_with_session(None);
    let client = connect_in_process(agent)
        .await
        .expect("in-process client connects");
    for overrides in [
        json!({"model": "  ", "limits": {}}),
        json!({"limits": {"max_turns": 0}}),
        json!({"limits": {"max_cost_usd": 0.0}}),
        json!({"limits": {"timeout_secs": 0}}),
    ] {
        let result = client
            .call_tool(
                AGENT_TURN_TOOL,
                json!({"text": "never starts", "overrides": overrides}),
            )
            .await
            .expect("invalid override is a typed result");
        assert!(result.is_error);
        match typed(&result) {
            AgentTurnResult::Refused { refusal } => {
                assert_eq!(refusal.kind, AgentRefusalKind::InvalidConfiguration)
            }
            other => panic!("unexpected invalid override result: {other:?}"),
        }
    }
    assert_eq!(state.calls.load(Ordering::SeqCst), 0);
    client.shutdown().await.expect("client shuts down cleanly");
}

#[tokio::test]
async fn provider_failure_is_typed_reusable_and_updates_session_evidence() {
    let (agent, state) = agent_with_session(None);
    let client = connect_in_process(agent)
        .await
        .expect("in-process client connects");

    client
        .call_tool(AGENT_TURN_TOOL, json!({"text": "establish"}))
        .await
        .expect("establishing turn completes");
    let failed = client
        .call_tool(AGENT_TURN_TOOL, json!({"text": "fail"}))
        .await
        .expect("provider failure remains an MCP tool result");
    assert!(failed.is_error);
    assert_eq!(failed.first_text(), Some("provider turn limit reached"));
    match typed(&failed) {
        AgentTurnResult::Failed { operation_id, run } => {
            assert_eq!(operation_id.get(), 2);
            let failure = run.failure;
            assert_eq!(failure.kind, FailureKind::MaxTurns);
            assert_eq!(failure.details.provider_turns, Some(12));
            assert_eq!(
                failure
                    .details
                    .session
                    .expect("failure session is retained")
                    .id,
                "session-from-failure"
            );
        }
        other => panic!("unexpected failure result: {other:?}"),
    }
    let after_failure = read_agent(&client).await;
    assert_eq!(after_failure.state, AgentState::Idle);
    assert!(after_failure.session_available);
    match after_failure.latest_turn.expect("failed turn retained") {
        AgentTurnResult::Failed { run, .. } => {
            assert!(
                run.failure.details.session.is_none(),
                "resource redacts failure session ids"
            )
        }
        other => panic!("unexpected retained failure: {other:?}"),
    }

    let recovered = client
        .call_tool(AGENT_TURN_TOOL, json!({"text": "recover"}))
        .await
        .expect("agent remains reusable after failure");
    assert!(!recovered.is_error);
    {
        let requests = state
            .requests
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(requests.len(), 3);
        assert_eq!(
            requests[2].spec.execution.session,
            SessionSpec::Resume {
                session: core_session("session-from-failure")
            }
        );
    }
    client.shutdown().await.expect("client shuts down cleanly");
}

#[tokio::test]
async fn invalid_returned_session_evidence_fails_loudly_and_preserves_known_state() {
    let (agent, state) = agent_with_session(None);
    let client = connect_in_process(agent)
        .await
        .expect("in-process client connects");

    let wrong_success = client
        .call_tool(AGENT_TURN_TOOL, json!({"text": "wrong-session-success"}))
        .await
        .expect("invalid provider result remains a typed tool result");
    assert!(wrong_success.is_error);
    match typed(&wrong_success) {
        AgentTurnResult::Failed { run, .. } => {
            assert_eq!(run.failure.kind, FailureKind::Provider);
            assert!(run.failure.message.contains("expected fake"));
            assert!(
                run.last_outcome
                    .expect("provider output remains observable")
                    .session
                    .is_none()
            );
        }
        other => panic!("unexpected invalid success result: {other:?}"),
    }
    assert!(!read_agent(&client).await.session_available);
    assert!(
        !wrong_success
            .structured_content
            .as_ref()
            .unwrap()
            .to_string()
            .contains("wrong-success")
    );

    client
        .call_tool(AGENT_TURN_TOOL, json!({"text": "establish"}))
        .await
        .expect("valid turn establishes a session");
    let wrong_failure = client
        .call_tool(AGENT_TURN_TOOL, json!({"text": "wrong-session-failure"}))
        .await
        .expect("invalid failure evidence remains a typed tool result");
    assert!(wrong_failure.is_error);
    match typed(&wrong_failure) {
        AgentTurnResult::Failed { run, .. } => {
            assert_eq!(run.failure.kind, FailureKind::Provider);
            assert!(run.failure.details.session.is_none());
            assert_eq!(run.failure.details.provider_turns, Some(9));
        }
        other => panic!("unexpected invalid failure result: {other:?}"),
    }
    assert!(read_agent(&client).await.session_available);
    assert!(
        !wrong_failure
            .structured_content
            .as_ref()
            .unwrap()
            .to_string()
            .contains("wrong-failure")
    );

    client
        .call_tool(AGENT_TURN_TOOL, json!({"text": "after"}))
        .await
        .expect("agent remains reusable after invalid evidence");
    {
        let requests = state
            .requests
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(requests.len(), 4);
        assert_eq!(requests[1].spec.execution.session, SessionSpec::Fresh);
        assert_eq!(
            requests[2].spec.execution.session,
            SessionSpec::Resume {
                session: core_session("session-1")
            }
        );
        assert_eq!(
            requests[3].spec.execution.session,
            SessionSpec::Resume {
                session: core_session("session-1")
            }
        );
    }
    client.shutdown().await.expect("client shuts down cleanly");
}

#[tokio::test]
async fn non_finite_provider_cost_cannot_violate_the_wire_schema() {
    let (agent, state) = agent_with_session(None);
    let client = connect_in_process(agent)
        .await
        .expect("in-process client connects");

    let invalid = client
        .call_tool(AGENT_TURN_TOOL, json!({"text": "nan-cost"}))
        .await
        .expect("invalid telemetry remains a structured tool result");
    assert!(invalid.is_error);
    match typed(&invalid) {
        AgentTurnResult::Failed { run, .. } => {
            assert_eq!(run.failure.kind, FailureKind::Provider);
            assert!(run.failure.message.contains("non-finite"));
            assert!(run.failure.details.cost.is_none());
            assert!(
                run.last_outcome
                    .expect("provider output remains observable")
                    .cost
                    .is_none()
            );
        }
        other => panic!("unexpected invalid telemetry result: {other:?}"),
    }
    assert!(!read_agent(&client).await.session_available);
    assert_eq!(state.calls.load(Ordering::SeqCst), 1);
    client.shutdown().await.expect("client shuts down cleanly");
}

#[tokio::test]
async fn resumed_success_without_new_session_evidence_preserves_continuity() {
    let (agent, state) = agent_with_session(None);
    let client = connect_in_process(agent)
        .await
        .expect("in-process client connects");

    client
        .call_tool(AGENT_TURN_TOOL, json!({"text": "establish"}))
        .await
        .expect("establishing turn completes");
    let sessionless = client
        .call_tool(AGENT_TURN_TOOL, json!({"text": "no-session"}))
        .await
        .expect("sessionless resumed turn completes");
    assert!(!sessionless.is_error);
    match typed(&sessionless) {
        AgentTurnResult::Completed { run, .. } => {
            assert!(run.outcome.session.is_none())
        }
        other => panic!("unexpected sessionless result: {other:?}"),
    }
    assert!(read_agent(&client).await.session_available);

    client
        .call_tool(AGENT_TURN_TOOL, json!({"text": "after"}))
        .await
        .expect("turn after sessionless result completes");
    {
        let requests = state
            .requests
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(
            requests[2].spec.execution.session,
            SessionSpec::Resume {
                session: core_session("session-1")
            }
        );
    }
    client.shutdown().await.expect("client shuts down cleanly");
}

#[tokio::test]
async fn concurrent_turn_is_typed_busy_and_only_one_provider_call_wins() {
    let (agent, state) = agent_with_session(None);
    let client = Arc::new(
        connect_in_process(agent.clone())
            .await
            .expect("in-process client connects"),
    );
    let first_client = Arc::clone(&client);
    let first = tokio::spawn(async move {
        first_client
            .call_tool(AGENT_TURN_TOOL, json!({"text": "hold"}))
            .await
            .expect("held turn eventually completes")
    });
    tokio::time::timeout(Duration::from_secs(1), state.started.acquire())
        .await
        .expect("provider started before timeout")
        .expect("start semaphore remains open")
        .forget();

    let running = read_agent(&client).await;
    assert_eq!(running.state, AgentState::Running);
    assert_eq!(running.current_operation_id.map(|id| id.get()), Some(1));
    assert!(matches!(agent.stop().await, Err(AgentStopError::Busy(_))));

    let busy = client
        .call_tool(AGENT_TURN_TOOL, json!({"text": "second"}))
        .await
        .expect("busy is an application result");
    assert!(busy.is_error);
    match typed(&busy) {
        AgentTurnResult::Refused { refusal } => {
            assert_eq!(refusal.kind, AgentRefusalKind::Busy);
            assert_eq!(refusal.active_operation_id.map(|id| id.get()), Some(1));
        }
        other => panic!("unexpected busy result: {other:?}"),
    }
    assert_eq!(state.calls.load(Ordering::SeqCst), 1);

    state.release.add_permits(1);
    let completed = first.await.expect("first client task joins");
    assert!(!completed.is_error);
    assert_eq!(read_agent(&client).await.state, AgentState::Idle);
    let client = Arc::into_inner(client).expect("all client task clones were dropped");
    client.shutdown().await.expect("client shuts down cleanly");
}

#[tokio::test]
async fn stopped_agent_and_invalid_prompt_are_typed_without_provider_work() {
    let (agent, state) = agent_with_session(None);
    let client = connect_in_process(agent.clone())
        .await
        .expect("in-process client connects");

    let invalid = client
        .call_tool(AGENT_TURN_TOOL, json!({"text": "  \n"}))
        .await
        .expect("invalid prompt is an application result");
    assert!(invalid.is_error);
    match typed(&invalid) {
        AgentTurnResult::Refused { refusal } => {
            assert_eq!(refusal.kind, AgentRefusalKind::InvalidPrompt)
        }
        other => panic!("unexpected invalid prompt result: {other:?}"),
    }
    assert_eq!(state.calls.load(Ordering::SeqCst), 0);

    agent.stop().await.expect("idle agent stops");
    agent.stop().await.expect("stopping is idempotent");
    assert_eq!(read_agent(&client).await.state, AgentState::Stopped);
    let stopped = client
        .call_tool(AGENT_TURN_TOOL, json!({"text": "work"}))
        .await
        .expect("stopped refusal is an application result");
    assert!(stopped.is_error);
    match typed(&stopped) {
        AgentTurnResult::Refused { refusal } => {
            assert_eq!(refusal.kind, AgentRefusalKind::Stopped)
        }
        other => panic!("unexpected stopped result: {other:?}"),
    }
    assert_eq!(state.calls.load(Ordering::SeqCst), 0);
    client.shutdown().await.expect("client shuts down cleanly");
}

#[tokio::test]
async fn aborted_agent_turn_waiter_does_not_own_agent_settlement() {
    let (agent, state) = agent_with_session(None);
    let waiting_agent = agent.clone();
    let waiting = tokio::spawn(async move { waiting_agent.turn("hold".to_string()).await });
    tokio::time::timeout(Duration::from_secs(1), state.started.acquire())
        .await
        .expect("provider started before timeout")
        .expect("start semaphore remains open")
        .forget();
    waiting.abort();
    let _ = waiting.await;
    state.release.add_permits(1);

    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if agent.snapshot().await.state == AgentState::Idle {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("detached coordinator settles without its original waiter");
    let next = agent.turn("next".to_string()).await;
    assert!(matches!(next, AgentTurnResult::Completed { .. }));
    assert_eq!(state.calls.load(Ordering::SeqCst), 2);
}

#[test]
fn construction_rejects_invalid_templates_without_provider_work() {
    let state = Arc::new(FakeState::default());
    let mut runtime = Roba::new();
    runtime
        .register(FakeProvider {
            state: Arc::clone(&state),
        })
        .expect("fake provider registration succeeds");
    let ready = RunSpec::suspended(AgentSpec::new(fake_provider_id()))
        .with_prompt(roba_core::Prompt::new("already ready").unwrap());
    assert_eq!(
        AgentInstance::new(runtime, ready).err().unwrap(),
        AgentBuildError::TemplateNotSuspended
    );
    assert_eq!(state.calls.load(Ordering::SeqCst), 0);

    let unavailable = RunSpec::suspended(AgentSpec::new(ProviderId::codex()));
    assert_eq!(
        AgentInstance::new(Roba::new(), unavailable).err().unwrap(),
        AgentBuildError::ProviderUnavailable(ProviderId::codex())
    );

    let mut runtime = Roba::new();
    runtime
        .register(FakeProvider {
            state: Arc::clone(&state),
        })
        .expect("fake provider registration succeeds");
    let mut mismatched = RunSpec::suspended(AgentSpec::new(fake_provider_id()));
    mismatched.execution.session = SessionSpec::Resume {
        session: CoreSessionHandle {
            provider: ProviderId::codex(),
            id: "wrong-provider".to_string(),
        },
    };
    assert_eq!(
        AgentInstance::new(runtime, mismatched).err().unwrap(),
        AgentBuildError::SessionProviderMismatch {
            selected: fake_provider_id(),
            session: ProviderId::codex(),
        }
    );

    let mut runtime = Roba::new();
    runtime
        .register(FakeProvider {
            state: Arc::clone(&state),
        })
        .expect("fake provider registration succeeds");
    let mut empty_session = RunSpec::suspended(AgentSpec::new(fake_provider_id()));
    empty_session.execution.session = SessionSpec::Resume {
        session: core_session("  "),
    };
    assert_eq!(
        AgentInstance::new(runtime, empty_session).err().unwrap(),
        AgentBuildError::EmptySessionId
    );

    let mut runtime = Roba::new();
    runtime
        .register(FakeProvider {
            state: Arc::clone(&state),
        })
        .expect("fake provider registration succeeds");
    let mut invalid_cost = RunSpec::suspended(AgentSpec::new(fake_provider_id()));
    invalid_cost.execution.limits.max_cost_usd = Some(f64::NAN);
    assert_eq!(
        AgentInstance::new(runtime, invalid_cost).err().unwrap(),
        AgentBuildError::InvalidMaxCost
    );
    assert_eq!(state.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn construction_retains_a_content_free_inventory_of_explicit_template_context() {
    let state = Arc::new(FakeState::default());
    let mut runtime = Roba::new();
    runtime
        .register(FakeProvider {
            state: Arc::clone(&state),
        })
        .expect("fake provider registration succeeds");
    let mut template = RunSpec::suspended(AgentSpec::new(fake_provider_id()));
    template.agent.instructions = vec!["private instruction".to_string()];
    template.context = ContextSpec {
        project: vec!["private project context".to_string()],
        run: vec!["private run context".to_string()],
    };

    let agent = AgentInstance::new(runtime, template).expect("valid test agent");
    let manifest = agent.context_plan().manifest();

    assert_eq!(manifest.entries.len(), 3);
    assert_eq!(manifest.entries[0].id, "agent.instruction.1");
    assert_eq!(manifest.entries[1].id, "project.context.1");
    assert_eq!(manifest.entries[2].id, "run.context.1");
    let serialized = serde_json::to_string(manifest).unwrap();
    assert!(!serialized.contains("private instruction"));
    assert!(!serialized.contains("private project context"));
    assert!(!serialized.contains("private run context"));
    assert_eq!(state.calls.load(Ordering::SeqCst), 0);

    let client = connect_in_process(agent)
        .await
        .expect("context control client connects");
    let resources = client.list_resources().await.unwrap().resources;
    assert!(
        resources
            .iter()
            .any(|resource| resource.uri == AGENT_CONTEXT_URI)
    );
    let templates = client
        .list_resource_templates()
        .await
        .unwrap()
        .resource_templates;
    assert!(
        templates
            .iter()
            .any(|template| template.uri_template == AGENT_CONTEXT_ENTRY_TEMPLATE)
    );
    let snapshot = client.read_resource(AGENT_CONTEXT_URI).await.unwrap();
    let snapshot_text = snapshot.first_text().unwrap();
    assert!(!snapshot_text.contains("private instruction"));
    let snapshot: ContextSnapshot = serde_json::from_str(snapshot_text).unwrap();
    assert_eq!(snapshot.operation_id, None);
    assert_eq!(snapshot.read_evidence, None);
    let uri = format!(
        "roba://context/entry?id=agent.instruction.1&generation={}",
        snapshot.manifest.generation
    );
    let content = client.read_resource(&uri).await.unwrap();
    let content: ContextContent = serde_json::from_str(content.first_text().unwrap()).unwrap();
    assert_eq!(content.operation_id, None);
    assert_eq!(content.content, "private instruction");
    assert!(
        client
            .read_resource("roba://context/entry?id=agent.instruction.1&generation=2")
            .await
            .is_err()
    );
    client.shutdown().await.unwrap();
}

#[tokio::test]
async fn explicit_host_context_is_validated_inspectable_and_not_prompt_injected() {
    let state = Arc::new(FakeState::default());
    let mut runtime = Roba::new();
    runtime
        .register(FakeProvider {
            state: Arc::clone(&state),
        })
        .expect("fake provider registration succeeds");
    let mut template = RunSpec::suspended(AgentSpec::new(fake_provider_id()));
    template.agent.instructions = vec!["base instruction".to_owned()];
    let mut builder = ContextPlan::builder_from_run_spec(&template, AmbientContextPolicy::Ambient);
    builder
        .add_inline(
            ContextEntrySpec::new(
                "issue.summary",
                ContextKind::Reference,
                ContextOrigin::new(ContextOriginKind::External, "issue tracker"),
                ContextPhase::Live,
                ContextScope::Operation,
                ContextDelivery::McpResource {
                    uri: "roba://context/entry?id=issue.summary&generation=1".to_owned(),
                },
            )
            .audience(ContextAudience::Provider)
            .precedence(ContextPrecedence::Operation)
            .required(true),
            "Fix issue 42 without expanding scope.",
        )
        .unwrap();
    builder
        .add_inline(
            ContextEntrySpec::new(
                "operator.notes",
                ContextKind::Reference,
                ContextOrigin::new(ContextOriginKind::Cli, "operator note"),
                ContextPhase::Live,
                ContextScope::Agent,
                ContextDelivery::McpResource {
                    uri: "roba://context/entry?id=operator.notes&generation=1".to_owned(),
                },
            )
            .audience(ContextAudience::Operator)
            .precedence(ContextPrecedence::Operation),
            "Never visible to the provider projection.",
        )
        .unwrap();
    let plan = builder.build();
    let agent = AgentInstance::new_with_context_plan(runtime, template, Default::default(), plan)
        .expect("explicit context plan matches the suspended template");

    assert_eq!(agent.context_plan().manifest().entries.len(), 3);
    assert_eq!(agent.context_plan().provider_manifest().entries.len(), 2);
    assert!(
        agent
            .context_plan()
            .provider_manifest()
            .entries
            .iter()
            .all(|entry| entry.id != "operator.notes")
    );

    let client = connect_in_process(agent)
        .await
        .expect("control projection connects");
    let manifest = client.read_resource(AGENT_CONTEXT_URI).await.unwrap();
    let snapshot: ContextSnapshot = serde_json::from_str(manifest.first_text().unwrap()).unwrap();
    assert_eq!(snapshot.manifest.entries.len(), 3);
    assert!(snapshot.bootstrap.is_none());
    let operator = client
        .read_resource("roba://context/entry?id=operator.notes&generation=1")
        .await
        .expect("administrative control projection can inspect operator context");
    let operator: ContextContent = serde_json::from_str(operator.first_text().unwrap()).unwrap();
    assert_eq!(
        operator.content,
        "Never visible to the provider projection."
    );

    let turn = client
        .call_tool(AGENT_TURN_TOOL, json!({"text": "run"}))
        .await
        .expect("turn completes");
    assert!(!turn.is_error);
    {
        let requests = state
            .requests
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].spec.agent.instructions, ["base instruction"]);
        assert!(requests[0].spec.context.project.is_empty());
        assert!(requests[0].spec.context.run.is_empty());
    }
    let settled = client.read_resource(AGENT_CONTEXT_URI).await.unwrap();
    let settled: ContextSnapshot = serde_json::from_str(settled.first_text().unwrap()).unwrap();
    let bootstrap = settled
        .bootstrap
        .expect("settled operation retains its inspectable bootstrap");
    assert_eq!(bootstrap.required_acquisitions.len(), 1);
    assert_eq!(bootstrap.required_acquisitions[0].id, "issue.summary");
    assert!(!bootstrap.render().contains("Fix issue 42"));
    client.shutdown().await.unwrap();
}

#[test]
fn explicit_context_plan_cannot_hide_or_replace_run_spec_context() {
    let state = Arc::new(FakeState::default());
    let mut runtime = Roba::new();
    runtime
        .register(FakeProvider {
            state: Arc::clone(&state),
        })
        .expect("fake provider registration succeeds");
    let mut template = RunSpec::suspended(AgentSpec::new(fake_provider_id()));
    template.agent.instructions = vec!["must remain represented".to_owned()];
    let incomplete = ContextPlan::builder(AmbientContextPolicy::Ambient).build();

    let error =
        AgentInstance::new_with_context_plan(runtime, template, Default::default(), incomplete)
            .err()
            .expect("context plan mismatch must fail before admission");
    assert!(matches!(error, AgentBuildError::ContextPlan(_)));
    assert!(error.to_string().contains("agent.instruction.1"));
    assert_eq!(state.calls.load(Ordering::SeqCst), 0);
}
