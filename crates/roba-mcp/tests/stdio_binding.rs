use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use roba_core::{
    AgentSpec, EventSink, Provider, ProviderCapabilities, ProviderError, ProviderFuture,
    ProviderId, Roba, RunOutcome, RunSpec, SessionHandle as CoreSessionHandle, SessionSpec,
    TurnRequest,
};
use roba_mcp::{
    AGENT_EVENTS_URI, AGENT_INTERRUPT_TOOL, AGENT_RESOURCE_URI, AGENT_SHUTDOWN_TOOL,
    AGENT_TURN_TOOL, AgentEventPage, AgentInterruptResult, AgentShutdownResult, AgentSnapshot,
    AgentState, AgentTerminalState, AgentTurnResult, StdioBinding,
};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream};
use tokio::sync::Semaphore;
use tower_mcp::{ChannelTransport, McpClient, ProtocolSupport};

const FINAL_PROTOCOL: &str = "2026-07-28";
const STABLE_PROTOCOL: &str = "2025-11-25";
const TASKS_EXTENSION: &str = "io.modelcontextprotocol/tasks";
const TEST_TIMEOUT: Duration = Duration::from_secs(5);
const STREAM_CAPACITY: usize = 64 * 1024;

#[derive(Clone, Copy, Debug)]
enum Protocol {
    Stable,
    Final,
}

struct FakeState {
    calls: AtomicUsize,
    started: Semaphore,
    dropped: Semaphore,
    release: Semaphore,
}

impl Default for FakeState {
    fn default() -> Self {
        Self {
            calls: AtomicUsize::new(0),
            started: Semaphore::new(0),
            dropped: Semaphore::new(0),
            release: Semaphore::new(0),
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
        _events: &'a dyn EventSink,
    ) -> ProviderFuture<'a> {
        let state = Arc::clone(&self.state);
        state.calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            let text = request.prompt.as_str().to_owned();
            if text.starts_with("hold") {
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

            let session = match request.spec.execution.session {
                SessionSpec::Fresh => core_session("stdio-session"),
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
    ProviderId::new("phase5-stdio-fake").expect("static provider id is valid")
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

async fn take(permits: &Semaphore, description: &str) {
    bounded(permits.acquire())
        .await
        .unwrap_or_else(|_| panic!("{description} semaphore remains open"))
        .forget();
}

struct WireClient {
    protocol: Protocol,
    tasks: bool,
    input: Option<DuplexStream>,
    output: BufReader<DuplexStream>,
    next_id: i64,
    pending: HashMap<i64, Value>,
}

impl WireClient {
    fn new(protocol: Protocol, tasks: bool, input: DuplexStream, output: DuplexStream) -> Self {
        Self {
            protocol,
            tasks,
            input: Some(input),
            output: BufReader::new(output),
            next_id: 1,
            pending: HashMap::new(),
        }
    }

    fn final_meta(&self) -> Value {
        let capabilities = if self.tasks {
            json!({"extensions": {TASKS_EXTENSION: {}}})
        } else {
            json!({})
        };
        json!({
            "io.modelcontextprotocol/protocolVersion": FINAL_PROTOCOL,
            "io.modelcontextprotocol/clientInfo": {
                "name": "roba-stdio-test",
                "version": "0"
            },
            "io.modelcontextprotocol/clientCapabilities": capabilities
        })
    }

    async fn send(&mut self, value: Value) {
        let input = self.input.as_mut().expect("client input remains open");
        let mut frame = serde_json::to_vec(&value).expect("request serializes");
        frame.push(b'\n');
        input.write_all(&frame).await.expect("request writes");
        input.flush().await.expect("request flushes");
    }

    async fn start_request(&mut self, method: &str, mut params: Value) -> i64 {
        if matches!(self.protocol, Protocol::Final) {
            params
                .as_object_mut()
                .expect("request params are an object")
                .insert("_meta".to_owned(), self.final_meta());
        }
        let id = self.next_id;
        self.next_id += 1;
        self.send(json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        }))
        .await;
        id
    }

    async fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.start_request(method, params).await;
        self.response(id).await
    }

    async fn response(&mut self, id: i64) -> Value {
        if let Some(frame) = self.pending.remove(&id) {
            return frame;
        }
        bounded(async {
            loop {
                let mut line = String::new();
                let read = self
                    .output
                    .read_line(&mut line)
                    .await
                    .expect("server frame reads");
                assert_ne!(read, 0, "server closed before response {id}");
                let frame: Value =
                    serde_json::from_str(line.trim_end()).expect("stdout frame is JSON");
                assert_eq!(frame["jsonrpc"], "2.0", "stdout is JSON-RPC: {frame}");
                let Some(actual) = frame.get("id").and_then(Value::as_i64) else {
                    continue;
                };
                if actual == id {
                    return frame;
                }
                self.pending.insert(actual, frame);
            }
        })
        .await
    }

    async fn handshake(&mut self) {
        match self.protocol {
            Protocol::Stable => {
                let response = self
                    .request(
                        "initialize",
                        json!({
                            "protocolVersion": STABLE_PROTOCOL,
                            "capabilities": {},
                            "clientInfo": {"name": "roba-stdio-test", "version": "0"}
                        }),
                    )
                    .await;
                let result = result(&response);
                assert_eq!(result["protocolVersion"], STABLE_PROTOCOL);
                assert_eq!(result["serverInfo"]["name"], "roba-agent");
                assert_control_instructions(result);
                self.send(json!({
                    "jsonrpc": "2.0",
                    "method": "notifications/initialized"
                }))
                .await;
            }
            Protocol::Final => {
                let response = self.request("server/discover", json!({})).await;
                let result = result(&response);
                assert_eq!(
                    result["_meta"]["io.modelcontextprotocol/serverInfo"]["name"],
                    "roba-agent"
                );
                assert!(
                    result["supportedVersions"]
                        .as_array()
                        .expect("discovery publishes protocol versions")
                        .contains(&json!(FINAL_PROTOCOL))
                );
                assert_control_instructions(result);
            }
        }
    }

    fn close_input(&mut self) {
        self.input.take();
    }
}

fn result(frame: &Value) -> &Value {
    assert!(frame.get("error").is_none(), "request failed: {frame}");
    frame.get("result").expect("response contains result")
}

fn assert_control_instructions(result: &Value) {
    let instructions = result["instructions"]
        .as_str()
        .expect("Roba publishes operator guidance during handshake");
    for expected in [
        "one persistent logical Roba agent",
        "agent.turn",
        "MCP Tasks",
        "roba://agent",
        "roba://context",
        "roba://events",
        "agent.follow_up",
        "agent.interrupt",
        "agent.shutdown",
        "inspect discovery",
    ] {
        assert!(
            instructions.contains(expected),
            "control instructions omitted {expected:?}: {instructions}"
        );
    }
}

fn structured<T: serde::de::DeserializeOwned>(frame: &Value) -> T {
    serde_json::from_value(result(frame)["structuredContent"].clone())
        .expect("structured content matches the published type")
}

fn resource<T: serde::de::DeserializeOwned>(frame: &Value) -> T {
    let text = result(frame)["contents"][0]["text"]
        .as_str()
        .expect("resource response contains text");
    serde_json::from_str(text).expect("resource text matches the published type")
}

fn spawn_binding(
    agent: roba_mcp::AgentInstance,
    protocol: Protocol,
    tasks: bool,
) -> (WireClient, tokio::task::JoinHandle<tower_mcp::Result<()>>) {
    let (client_input, server_input) = tokio::io::duplex(STREAM_CAPACITY);
    let (server_output, client_output) = tokio::io::duplex(STREAM_CAPACITY);
    let mut binding = StdioBinding::new(agent);
    let server =
        tokio::spawn(async move { binding.run_with_streams(server_input, server_output).await });
    (
        WireClient::new(protocol, tasks, client_input, client_output),
        server,
    )
}

async fn stop_binding(
    client: &mut WireClient,
    server: tokio::task::JoinHandle<tower_mcp::Result<()>>,
) {
    let response = client
        .request(
            "tools/call",
            json!({"name": AGENT_SHUTDOWN_TOOL, "arguments": {}}),
        )
        .await;
    assert!(matches!(
        structured::<AgentShutdownResult>(&response),
        AgentShutdownResult::Stopped { .. }
    ));
    bounded(server)
        .await
        .expect("stdio binding task joins")
        .expect("stdio binding exits cleanly");
}

#[derive(Debug, PartialEq)]
struct ContractObservation {
    tools: Value,
    resources: Value,
    resource_templates: Value,
    prompts: Value,
    turn: Value,
}

async fn channel_observation(protocol: Protocol) -> ContractObservation {
    let (agent, _) = test_agent();
    let client = match protocol {
        Protocol::Stable => {
            let client = McpClient::builder()
                .protocol_support(ProtocolSupport::stable())
                .connect_simple(ChannelTransport::new(roba_mcp::router(agent)))
                .await
                .expect("stable ChannelTransport connects");
            let initialized = client
                .initialize("roba-channel-test", "0")
                .await
                .expect("stable ChannelTransport initializes");
            assert_eq!(initialized.protocol_version, STABLE_PROTOCOL);
            assert_control_instructions(
                &serde_json::to_value(initialized).expect("initialize result serializes"),
            );
            client
        }
        Protocol::Final => {
            let client = McpClient::builder()
                .protocol_support(
                    ProtocolSupport::try_new([FINAL_PROTOCOL]).expect("final protocol is compiled"),
                )
                .connect_simple(ChannelTransport::new(roba_mcp::router(agent)))
                .await
                .expect("final ChannelTransport connects");
            let discovered = client
                .discover("roba-channel-test", "0")
                .await
                .expect("final ChannelTransport discovers");
            assert_control_instructions(
                &serde_json::to_value(discovered).expect("discover result serializes"),
            );
            client
        }
    };

    let tools = serde_json::to_value(client.list_tools().await.expect("tools/list succeeds"))
        .expect("tools serialize")["tools"]
        .clone();
    let resources = serde_json::to_value(
        client
            .list_resources()
            .await
            .expect("resources/list succeeds"),
    )
    .expect("resources serialize")["resources"]
        .clone();
    let resource_templates = serde_json::to_value(
        client
            .list_resource_templates()
            .await
            .expect("resources/templates/list succeeds"),
    )
    .expect("resource templates serialize")["resourceTemplates"]
        .clone();
    let prompts = serde_json::to_value(client.list_prompts().await.expect("prompts/list succeeds"))
        .expect("prompts serialize")["prompts"]
        .clone();
    let turn = serde_json::to_value(
        client
            .call_tool(AGENT_TURN_TOOL, json!({"text": "parity"}))
            .await
            .expect("agent.turn succeeds"),
    )
    .expect("turn result serializes");
    client.shutdown().await.expect("channel client shuts down");

    ContractObservation {
        tools,
        resources,
        resource_templates,
        prompts,
        turn: normalized_turn(turn),
    }
}

fn normalized_turn(value: Value) -> Value {
    let content = value["content"].clone();
    let is_error = value
        .get("isError")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let mut structured = value["structuredContent"].clone();
    let object = structured
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
    json!({
        "content": content,
        "isError": is_error,
        "structuredContent": structured
    })
}

#[tokio::test]
async fn stable_and_final_stdio_match_channel_discovery_and_structured_results() {
    for protocol in [Protocol::Stable, Protocol::Final] {
        let channel = bounded(channel_observation(protocol)).await;
        let (agent, _) = test_agent();
        let (mut client, server) = spawn_binding(agent, protocol, false);
        client.handshake().await;

        let tools = result(&client.request("tools/list", json!({})).await)["tools"].clone();
        let resources =
            result(&client.request("resources/list", json!({})).await)["resources"].clone();
        let resource_templates = result(
            &client.request("resources/templates/list", json!({})).await,
        )["resourceTemplates"]
            .clone();
        let prompts = result(&client.request("prompts/list", json!({})).await)["prompts"].clone();
        let turn = result(
            &client
                .request(
                    "tools/call",
                    json!({"name": AGENT_TURN_TOOL, "arguments": {"text": "parity"}}),
                )
                .await,
        )
        .clone();
        let stdio = ContractObservation {
            tools,
            resources,
            resource_templates,
            prompts,
            turn: normalized_turn(turn),
        };
        assert_eq!(stdio, channel, "{protocol:?} stdio contract drifted");

        stop_binding(&mut client, server).await;
    }
}

#[tokio::test]
async fn held_synchronous_turn_is_observable_and_interruptible_on_stable_and_final_stdio() {
    for protocol in [Protocol::Stable, Protocol::Final] {
        let (agent, state) = test_agent();
        let (mut client, server) = spawn_binding(agent.clone(), protocol, false);
        client.handshake().await;

        let turn_id = client
            .start_request(
                "tools/call",
                json!({
                    "name": AGENT_TURN_TOOL,
                    "arguments": {"text": format!("hold-{protocol:?}")}
                }),
            )
            .await;
        take(&state.started, "provider start").await;

        let snapshot: AgentSnapshot = resource(
            &client
                .request("resources/read", json!({"uri": AGENT_RESOURCE_URI}))
                .await,
        );
        assert_eq!(snapshot.state, AgentState::Running);
        let operation_id = snapshot
            .current_operation_id
            .expect("running resource publishes the operation id");
        let events: AgentEventPage = resource(
            &client
                .request("resources/read", json!({"uri": AGENT_EVENTS_URI}))
                .await,
        );
        assert!(
            events
                .events
                .iter()
                .any(|event| event.operation_id == operation_id),
            "active operation is observable in replay"
        );

        let interrupt = client
            .request(
                "tools/call",
                json!({
                    "name": AGENT_INTERRUPT_TOOL,
                    "arguments": {"operation_id": operation_id}
                }),
            )
            .await;
        match structured::<AgentInterruptResult>(&interrupt) {
            AgentInterruptResult::Settled {
                settlement,
                cancellation_requested,
            } => {
                assert_eq!(settlement.operation_id, operation_id);
                assert_eq!(settlement.state, AgentTerminalState::Cancelled);
                assert!(cancellation_requested);
            }
            other => panic!("unexpected interrupt result: {other:?}"),
        }
        assert!(matches!(
            structured::<AgentTurnResult>(&client.response(turn_id).await),
            AgentTurnResult::Cancelled {
                operation_id: actual,
                ..
            } if actual == operation_id
        ));
        take(&state.dropped, "provider drop").await;
        assert_eq!(agent.snapshot().await.state, AgentState::Idle);

        stop_binding(&mut client, server).await;
    }
}

#[tokio::test]
async fn final_task_and_eof_drain_the_provider_and_stop_the_agent_before_returning() {
    let (agent, state) = test_agent();
    let (mut client, server) = spawn_binding(agent.clone(), Protocol::Final, true);
    client.handshake().await;

    let created = client
        .request(
            "tools/call",
            json!({"name": AGENT_TURN_TOOL, "arguments": {"text": "hold-task-eof"}}),
        )
        .await;
    assert_eq!(result(&created)["resultType"], "task");
    assert!(result(&created)["taskId"].as_str().is_some());
    assert_eq!(result(&created)["status"], "working");
    take(&state.started, "task provider start").await;

    let snapshot: AgentSnapshot = resource(
        &client
            .request("resources/read", json!({"uri": AGENT_RESOURCE_URI}))
            .await,
    );
    assert_eq!(snapshot.state, AgentState::Running);
    let operation_id = snapshot.current_operation_id.unwrap();

    client.close_input();
    bounded(server)
        .await
        .expect("stdio binding task joins")
        .expect("EOF shuts the binding down cleanly");
    assert_eq!(state.dropped.available_permits(), 1);
    let stopped = agent.snapshot().await;
    assert_eq!(stopped.state, AgentState::Stopped);
    assert!(matches!(
        stopped.latest_turn,
        Some(AgentTurnResult::Cancelled {
            operation_id: actual,
            ..
        }) if actual == operation_id
    ));
}

#[tokio::test]
async fn agent_shutdown_drains_responds_and_exits_while_stdio_input_remains_open() {
    let (agent, state) = test_agent();
    let (mut client, server) = spawn_binding(agent.clone(), Protocol::Stable, false);
    client.handshake().await;

    let turn_id = client
        .start_request(
            "tools/call",
            json!({
                "name": AGENT_TURN_TOOL,
                "arguments": {"text": "hold-agent-shutdown"}
            }),
        )
        .await;
    take(&state.started, "shutdown provider start").await;
    let operation_id = agent.snapshot().await.current_operation_id.unwrap();

    let shutdown = client
        .request(
            "tools/call",
            json!({"name": AGENT_SHUTDOWN_TOOL, "arguments": {}}),
        )
        .await;
    assert_eq!(
        structured::<AgentShutdownResult>(&shutdown),
        AgentShutdownResult::Stopped {
            drained: Some(roba_mcp::OperationSettlement {
                operation_id,
                state: AgentTerminalState::Cancelled,
            })
        }
    );
    assert!(matches!(
        structured::<AgentTurnResult>(&client.response(turn_id).await),
        AgentTurnResult::Cancelled {
            operation_id: actual,
            ..
        } if actual == operation_id
    ));
    assert!(client.input.is_some(), "test keeps stdin open");
    bounded(server)
        .await
        .expect("stdio binding task joins")
        .expect("agent.shutdown exits the binding cleanly");
    assert_eq!(state.dropped.available_permits(), 1);
    assert_eq!(agent.snapshot().await.state, AgentState::Stopped);
}
