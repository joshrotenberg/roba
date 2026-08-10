//! OpenAI Codex provider adapter.

use std::path::PathBuf;

use codex_wrapper::types::{JsonLineEvent, QueryResult};
use codex_wrapper::{
    ApprovalPolicyConfig, Codex, ExecCommand, ExecResumeCommand, McpConfigBuilder, McpServerConfig,
    SandboxMode,
};

use crate::provider::{
    EventSink, Provider, ProviderCapabilities, ProviderContext, ProviderError, ProviderFuture,
};
use crate::run::{
    Effort, FailureKind, PermissionPolicy, ProviderId, RunEvent, RunOutcome, SessionHandle,
    SessionSpec, TokenUsage, TurnRequest,
};

/// Codex CLI implementation of Roba's provider-neutral turn boundary.
#[derive(Debug, Clone, Default)]
pub struct CodexProvider {
    binary: Option<PathBuf>,
}

const WORKER_GUIDANCE: &str = "Roba owns all child work for this run. When the task calls for workers, use only the `roba_workers.spawn_worker` MCP tool, then use `roba_workers.workers` and wait for every spawned worker before answering. Keep monitors current with the report_work_item, report_blocker, and report_artifact tools; those are claims and do not replace your final answer. Never launch Roba or provider CLIs in the shell to simulate workers, and never substitute provider-native subagents. If Roba refuses a spawn, report that refusal instead of using another mechanism.";

impl CodexProvider {
    /// Use an explicit Codex executable instead of resolving `codex` from
    /// `PATH`. This is useful for embedded hosts and deterministic tests.
    pub fn with_binary(binary: impl Into<PathBuf>) -> Self {
        Self {
            binary: Some(binary.into()),
        }
    }

    fn client(
        &self,
        request: &TurnRequest,
        context: &ProviderContext,
    ) -> Result<Codex, ProviderError> {
        let mut builder = Codex::builder();
        if let Some(binary) = &self.binary {
            builder = builder.binary(binary);
        }
        if let Some(seconds) = request.spec.execution.limits.timeout_secs {
            builder = builder.timeout_secs(seconds);
        }
        for (name, value) in mcp_configuration(context).environment {
            builder = builder.env(name, value);
        }
        builder.build().map_err(map_error)
    }

    fn fresh_command(request: &TurnRequest, context: &ProviderContext) -> ExecCommand {
        // Roba owns bounded child-run creation. Codex's native multi-agent
        // feature is not represented in the run tree and would bypass worker
        // count, depth, cancellation, and event observation.
        let mut command = ExecCommand::new(render_prompt(request, context))
            .prompt_via_stdin()
            .disable("multi_agent");
        if let Some(model) = &request.spec.agent.model {
            command = command.model(model.clone());
        }
        if let Some(effort) = request.spec.agent.effort {
            command = command.config(reasoning_effort(effort));
        }
        command = match request.spec.execution.permissions {
            PermissionPolicy::ReadOnly => command
                .sandbox(SandboxMode::ReadOnly)
                .approval_policy(ApprovalPolicyConfig::Never),
            PermissionPolicy::WorkspaceWrite => command
                .sandbox(SandboxMode::WorkspaceWrite)
                .approval_policy(ApprovalPolicyConfig::OnRequest),
            PermissionPolicy::FullAuto => command
                .sandbox(SandboxMode::WorkspaceWrite)
                .approval_policy(ApprovalPolicyConfig::Never),
        };
        for value in mcp_configuration(context).overrides {
            command = command.config(value);
        }
        command
    }

    fn resume_command(
        request: &TurnRequest,
        session_id: &str,
        context: &ProviderContext,
    ) -> ExecResumeCommand {
        let mut command = ExecResumeCommand::new()
            .session_id(session_id)
            .prompt(render_prompt(request, context))
            .disable("multi_agent");
        if let Some(model) = &request.spec.agent.model {
            command = command.model(model.clone());
        }
        if let Some(effort) = request.spec.agent.effort {
            command = command.config(reasoning_effort(effort));
        }
        command = match request.spec.execution.permissions {
            PermissionPolicy::ReadOnly => command
                .config("sandbox_mode=\"read-only\"")
                .approval_policy(ApprovalPolicyConfig::Never),
            PermissionPolicy::WorkspaceWrite => command
                .config("sandbox_mode=\"workspace-write\"")
                .approval_policy(ApprovalPolicyConfig::OnRequest),
            PermissionPolicy::FullAuto => command
                .config("sandbox_mode=\"workspace-write\"")
                .approval_policy(ApprovalPolicyConfig::Never),
        };
        for value in mcp_configuration(context).overrides {
            command = command.config(value);
        }
        command
    }

    /// Normalize Codex's result without estimating monetary cost or missing
    /// usage buckets.
    pub fn normalize(result: QueryResult) -> RunOutcome {
        let session_id = result.thread_id.or(result.session_id);
        RunOutcome {
            output: result.result,
            session: session_id.map(|id| SessionHandle {
                provider: ProviderId::codex(),
                id,
            }),
            usage: result.usage.map(normalize_token_usage),
            // Codex reports token usage but no authoritative price.
            cost: None,
            duration_ms: None,
            provider_turns: None,
            structured_output: None,
        }
    }
}

impl Provider for CodexProvider {
    fn id(&self) -> ProviderId {
        ProviderId::codex()
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            resume: true,
            // JSONL output and usage are normalized into the common event
            // sink before the same records assemble the terminal outcome.
            streaming: true,
            read_only: true,
            workspace_write: true,
            full_auto: true,
            max_turns: false,
            max_cost: false,
            timeout: true,
        }
    }

    fn validate(&self, request: &TurnRequest) -> Result<(), ProviderError> {
        if request.spec.agent.provider != ProviderId::codex() {
            return Err(ProviderError::unsupported(format!(
                "Codex adapter cannot execute provider {}",
                request.spec.agent.provider
            )));
        }
        if request
            .spec
            .agent
            .model
            .as_ref()
            .is_some_and(String::is_empty)
        {
            return Err(ProviderError::unsupported("Codex model must not be empty"));
        }
        if request.spec.agent.effort == Some(Effort::Max) {
            return Err(ProviderError::unsupported(
                "Codex does not support the provider-neutral max effort; use x_high",
            ));
        }
        if request.spec.execution.limits.max_turns.is_some() {
            return Err(ProviderError::unsupported(
                "Codex provider does not support a max-turn ceiling",
            ));
        }
        if !request.spec.execution.tools.allow.is_empty()
            || !request.spec.execution.tools.deny.is_empty()
        {
            return Err(ProviderError::unsupported(
                "Codex provider cannot enforce Roba's granular tool allow/deny policy",
            ));
        }
        if request.spec.execution.limits.max_cost_usd.is_some() {
            return Err(ProviderError::unsupported(
                "Codex reports no authoritative monetary cost and cannot enforce max_cost_usd",
            ));
        }
        if request.spec.execution.limits.timeout_secs == Some(0) {
            return Err(ProviderError::unsupported(
                "Codex timeout_secs must be greater than zero",
            ));
        }
        if let SessionSpec::Resume { session } = &request.spec.execution.session {
            if session.provider != ProviderId::codex() {
                return Err(ProviderError::unsupported(format!(
                    "cannot resume {} session with Codex provider",
                    session.provider
                )));
            }
            if session.id.trim().is_empty() {
                return Err(ProviderError::unsupported(
                    "Codex resume session id must not be empty",
                ));
            }
        }
        Ok(())
    }

    fn execute<'a>(
        &'a self,
        request: TurnRequest,
        context: ProviderContext,
        events: &'a dyn EventSink,
    ) -> ProviderFuture<'a> {
        Box::pin(async move {
            self.validate(&request)?;
            let codex = self.client(&request, &context)?;
            events.emit(RunEvent::TurnStarted {
                provider: ProviderId::codex(),
            });
            let mut captured = Vec::new();
            {
                let mut capture = |event| {
                    emit_stream_event(&event, events);
                    captured.push(event);
                };
                match &request.spec.execution.session {
                    SessionSpec::Fresh => codex_wrapper::streaming::stream_exec(
                        &codex,
                        &Self::fresh_command(&request, &context),
                        &mut capture,
                    )
                    .await
                    .map_err(map_error)?,
                    SessionSpec::Resume { session } => {
                        codex_wrapper::streaming::stream_exec_resume(
                            &codex,
                            &Self::resume_command(&request, &session.id, &context),
                            &mut capture,
                        )
                        .await
                        .map_err(map_error)?
                    }
                }
            }
            if !captured.iter().any(JsonLineEvent::is_turn_completed) {
                return Err(ProviderError::new(
                    FailureKind::Provider,
                    "Codex stream ended without a turn.completed event",
                ));
            }
            let result = QueryResult::from_events(captured);
            let outcome = Self::normalize(result);
            events.emit(RunEvent::TurnCompleted {
                outcome: outcome.clone(),
            });
            Ok(outcome)
        })
    }
}

fn normalize_token_usage(usage: codex_wrapper::types::TokenUsage) -> TokenUsage {
    TokenUsage {
        input: usage.input_tokens,
        cached_input: usage.cached_input_tokens,
        cache_write_input: usage.cache_write_input_tokens,
        output: usage.output_tokens,
        reasoning_output: usage.reasoning_output_tokens,
        total: usage.total_tokens,
    }
}

fn emit_stream_event(event: &codex_wrapper::types::JsonLineEvent, sink: &dyn EventSink) {
    if let Some(text) = event.agent_message_text() {
        sink.emit(RunEvent::OutputDelta { text });
    }
    if let Some(usage) = event.usage() {
        sink.emit(RunEvent::Usage {
            usage: normalize_token_usage(usage),
        });
    }
}

struct CodexMcpConfiguration {
    overrides: Vec<String>,
    environment: Vec<(String, String)>,
}

fn mcp_configuration(context: &ProviderContext) -> CodexMcpConfiguration {
    let mut configuration = CodexMcpConfiguration {
        overrides: Vec::new(),
        environment: Vec::new(),
    };
    for (index, endpoint) in context.mcp_endpoints().iter().enumerate() {
        let variable = format!("ROBA_INTERNAL_MCP_TOKEN_{index}");
        configuration
            .environment
            .push((variable.clone(), endpoint.bearer_token().to_string()));
        configuration.overrides.extend(
            McpConfigBuilder::new()
                .server(
                    endpoint.name(),
                    McpServerConfig::http(endpoint.url())
                        .bearer_token_env_var(variable)
                        .required(),
                )
                .config_overrides(),
        );
        if endpoint.name() == "roba_workers" {
            for tool in [
                "spawn_worker",
                "report_work_item",
                "report_blocker",
                "report_artifact",
            ] {
                configuration.overrides.push(format!(
                    "mcp_servers.roba_workers.tools.{tool}.approval_mode=\"approve\""
                ));
            }
        }
    }
    configuration
}

fn render_prompt(request: &TurnRequest, context: &ProviderContext) -> String {
    let mut sections = Vec::new();
    sections.extend(request.spec.agent.instructions.iter().cloned());
    sections.extend(request.spec.context.project.iter().cloned());
    sections.extend(request.spec.context.run.iter().cloned());
    sections.push(request.prompt.as_str().to_string());
    if context
        .mcp_endpoints()
        .iter()
        .any(|endpoint| endpoint.name() == "roba_workers")
    {
        sections.push(WORKER_GUIDANCE.to_string());
    }
    sections.join("\n\n")
}

fn reasoning_effort(effort: Effort) -> String {
    let value = match effort {
        Effort::Low => "low",
        Effort::Medium => "medium",
        Effort::High => "high",
        Effort::XHigh => "xhigh",
        Effort::Max => unreachable!("max effort is rejected during Codex validation"),
    };
    format!("model_reasoning_effort=\"{value}\"")
}

fn map_error(error: codex_wrapper::Error) -> ProviderError {
    let kind = match error {
        codex_wrapper::Error::Auth { .. } => FailureKind::Authentication,
        codex_wrapper::Error::Timeout { .. } => FailureKind::Timeout,
        codex_wrapper::Error::TokenBudgetExceeded { .. } => FailureKind::Limit,
        codex_wrapper::Error::Cancelled { .. } => FailureKind::Cancelled,
        codex_wrapper::Error::Config { .. }
        | codex_wrapper::Error::VersionMismatch { .. }
        | codex_wrapper::Error::UntestedCliVersion { .. }
        | codex_wrapper::Error::DangerousNotAllowed { .. } => FailureKind::Unsupported,
        _ => FailureKind::Provider,
    };
    ProviderError::new(kind, format!("Codex failed: {error}"))
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use std::sync::Mutex;

    use codex_wrapper::CodexCommand;
    use codex_wrapper::types::{JsonLineEvent, TokenUsage as CodexUsage};

    use super::*;
    #[cfg(unix)]
    use crate::run::RunState;
    use crate::run::{AgentSpec, Prompt, RunSpec};

    #[cfg(unix)]
    #[derive(Default)]
    struct RecordingSink {
        events: Mutex<Vec<RunEvent>>,
    }

    #[cfg(unix)]
    impl EventSink for RecordingSink {
        fn emit(&self, event: RunEvent) {
            self.events.lock().unwrap().push(event);
        }
    }

    #[cfg(unix)]
    fn fake_codex(temp: &tempfile::TempDir) -> (CodexProvider, PathBuf) {
        use std::os::unix::fs::PermissionsExt;

        let marker = temp.path().join("blocked.pid");
        let binary = temp.path().join("codex");
        let script = format!(
            r#"#!/bin/sh
prompt=$(cat)
if [ "$prompt" = "block" ]; then
  printf '%s' "$$" > '{}'
  exec sleep 30
fi
if [ "$prompt" = "unterminated" ]; then
  printf '%s\n' '{{"type":"thread.started","thread_id":"thread-1"}}'
  printf '%s\n' '{{"type":"item.completed","item":{{"type":"agent_message","text":"partial"}}}}'
  exit 0
fi
case " $* " in
  *" resume "*) text=resumed ;;
  *) text=opened ;;
esac
printf '%s\n' '{{"type":"thread.started","thread_id":"thread-1"}}'
printf '{{"type":"item.completed","item":{{"type":"agent_message","text":"%s"}}}}\n' "$text"
printf '%s\n' '{{"type":"turn.completed","usage":{{"input_tokens":3,"output_tokens":2}}}}'
"#,
            marker.display()
        );
        std::fs::write(&binary, script).unwrap();
        let mut permissions = std::fs::metadata(&binary).unwrap().permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&binary, permissions).unwrap();
        (CodexProvider::with_binary(binary), marker)
    }

    fn request() -> TurnRequest {
        RunSpec::suspended(AgentSpec::new(ProviderId::codex()))
            .with_prompt(Prompt::new("hello").unwrap())
            .into_turn()
            .unwrap()
    }

    #[test]
    fn normalizes_thread_and_only_reported_usage() {
        let result = QueryResult {
            result: "done".to_string(),
            session_id: None,
            thread_id: Some("thread-1".to_string()),
            usage: Some(CodexUsage {
                input_tokens: Some(10),
                output_tokens: Some(4),
                ..CodexUsage::default()
            }),
            events: Vec::<JsonLineEvent>::new(),
        };
        let outcome = CodexProvider::normalize(result);
        assert_eq!(outcome.output, "done");
        assert_eq!(outcome.session.unwrap().id, "thread-1");
        assert_eq!(outcome.usage.as_ref().unwrap().input, Some(10));
        assert_eq!(outcome.usage.as_ref().unwrap().cached_input, None);
        assert!(outcome.cost.is_none());
    }

    #[test]
    fn unsupported_limits_refuse_before_launch() {
        let provider = CodexProvider::default();
        let mut turn = request();
        turn.spec.execution.limits.max_turns = Some(2);
        let error = provider.validate(&turn).unwrap_err();
        assert_eq!(error.kind, FailureKind::Unsupported);

        turn.spec.execution.limits.max_turns = None;
        turn.spec.execution.limits.max_cost_usd = Some(1.0);
        assert_eq!(
            provider.validate(&turn).unwrap_err().kind,
            FailureKind::Unsupported
        );
    }

    #[test]
    fn prompt_hierarchy_is_deterministic() {
        let mut turn = request();
        turn.spec.agent.instructions = vec!["agent".to_string()];
        turn.spec.context.project = vec!["project".to_string()];
        turn.spec.context.run = vec!["run".to_string()];
        assert_eq!(
            render_prompt(&turn, &ProviderContext::default()),
            "agent\n\nproject\n\nrun\n\nhello"
        );
    }

    #[test]
    fn effort_mapping_is_explicit() {
        assert_eq!(
            reasoning_effort(Effort::Low),
            "model_reasoning_effort=\"low\""
        );
        assert_eq!(
            reasoning_effort(Effort::XHigh),
            "model_reasoning_effort=\"xhigh\""
        );
    }

    #[test]
    fn worker_mcp_configuration_matches_open_and_resume_without_secret_argv() {
        let context =
            ProviderContext::default().with_mcp_endpoint(crate::ProviderMcpEndpoint::new(
                "roba_workers",
                "http://127.0.0.1:4123/mcp",
                "secret-worker-token",
            ));
        let request = request();
        let open = CodexProvider::fresh_command(&request, &context).args();
        let resume = CodexProvider::resume_command(&request, "thread-1", &context).args();
        let configuration = mcp_configuration(&context);
        let prompt = render_prompt(&request, &context);

        for value in &configuration.overrides {
            assert!(
                open.contains(value),
                "open command lacks {value:?}: {open:?}"
            );
            assert!(
                resume.contains(value),
                "resume command lacks {value:?}: {resume:?}"
            );
        }
        for tool in [
            "spawn_worker",
            "report_work_item",
            "report_blocker",
            "report_artifact",
        ] {
            assert!(configuration.overrides.iter().any(|value| {
                value == &format!("mcp_servers.roba_workers.tools.{tool}.approval_mode=\"approve\"")
            }));
        }
        assert_eq!(
            configuration.environment,
            vec![(
                "ROBA_INTERNAL_MCP_TOKEN_0".to_string(),
                "secret-worker-token".to_string()
            )]
        );
        assert!(!open.iter().any(|arg| arg.contains("secret-worker-token")));
        assert!(!resume.iter().any(|arg| arg.contains("secret-worker-token")));
        assert!(
            open.windows(2)
                .any(|args| args == ["--disable", "multi_agent"])
        );
        assert!(
            resume
                .windows(2)
                .any(|args| args == ["--disable", "multi_agent"])
        );
        assert!(prompt.ends_with(WORKER_GUIDANCE));
        assert!(prompt.contains("roba_workers.spawn_worker"));
        assert!(prompt.contains("Never launch Roba or provider CLIs in the shell"));
        assert!(!format!("{context:?}").contains("secret-worker-token"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn fake_binary_streams_open_and_resume_into_normalized_events() {
        let temp = tempfile::tempdir().unwrap();
        let (provider, _) = fake_codex(&temp);

        let fresh_events = RecordingSink::default();
        let fresh = provider
            .execute(request(), ProviderContext::default(), &fresh_events)
            .await
            .unwrap();
        assert_eq!(fresh.output, "opened");
        assert_eq!(fresh.session.as_ref().unwrap().id, "thread-1");
        assert_eq!(fresh.usage.as_ref().unwrap().input, Some(3));
        assert_eq!(fresh.usage.as_ref().unwrap().output, Some(2));
        let fresh_events = fresh_events.events.into_inner().unwrap();
        assert!(fresh_events.contains(&RunEvent::OutputDelta {
            text: "opened".to_string(),
        }));
        assert!(fresh_events.iter().any(|event| matches!(
            event,
            RunEvent::Usage { usage }
                if usage.input == Some(3) && usage.output == Some(2)
        )));
        assert!(matches!(
            fresh_events.last(),
            Some(RunEvent::TurnCompleted { .. })
        ));

        let mut resumed = request();
        resumed.spec.execution.session = SessionSpec::Resume {
            session: SessionHandle {
                provider: ProviderId::codex(),
                id: "thread-1".to_string(),
            },
        };
        let resume_events = RecordingSink::default();
        let resumed = provider
            .execute(resumed, ProviderContext::default(), &resume_events)
            .await
            .unwrap();
        assert_eq!(resumed.output, "resumed");
        assert!(
            resume_events
                .events
                .into_inner()
                .unwrap()
                .contains(&RunEvent::OutputDelta {
                    text: "resumed".to_string(),
                })
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn cancelling_a_streamed_turn_kills_the_fake_process() {
        let temp = tempfile::tempdir().unwrap();
        let (provider, marker) = fake_codex(&temp);
        let run = crate::Run::new(
            RunSpec::suspended(AgentSpec::new(ProviderId::codex()))
                .with_prompt(Prompt::new("block").unwrap()),
            std::sync::Arc::new(provider),
        )
        .unwrap();
        run.begin().await.unwrap();

        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while !marker.exists() {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("fake Codex process did not start");
        let pid = std::fs::read_to_string(&marker).unwrap();

        run.handle().cancel().await.unwrap();
        assert_eq!(run.handle().wait().await.state, RunState::Cancelled);
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                let alive = std::process::Command::new("kill")
                    .args(["-0", pid.as_str()])
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status()
                    .is_ok_and(|status| status.success());
                if !alive {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("cancelled Codex process remained alive");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn successful_process_without_terminal_event_fails_closed() {
        let temp = tempfile::tempdir().unwrap();
        let (provider, _) = fake_codex(&temp);
        let request = RunSpec::suspended(AgentSpec::new(ProviderId::codex()))
            .with_prompt(Prompt::new("unterminated").unwrap())
            .into_turn()
            .unwrap();
        let events = RecordingSink::default();

        let error = provider
            .execute(request, ProviderContext::default(), &events)
            .await
            .unwrap_err();

        assert_eq!(error.kind, FailureKind::Provider);
        assert!(error.message.contains("without a turn.completed event"));
        let events = events.events.into_inner().unwrap();
        assert!(events.contains(&RunEvent::OutputDelta {
            text: "partial".to_string(),
        }));
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, RunEvent::TurnCompleted { .. }))
        );
    }
}
