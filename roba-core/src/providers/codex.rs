//! OpenAI Codex provider adapter.

use std::path::PathBuf;

use codex_wrapper::types::{JsonLineEvent, QueryResult};
use codex_wrapper::{
    ApprovalPolicyConfig, Codex, ExecCommand, ExecResumeCommand, McpConfigBuilder, McpServerConfig,
    SandboxMode,
};

use crate::provider::{
    EventSink, Provider, ProviderCapabilities, ProviderError, ProviderEvent, ProviderFuture,
    ProviderLaunchContext,
};
use crate::run::{
    Effort, FailureKind, PermissionPolicy, ProviderId, RunFailureDetails, RunOutcome,
    SessionHandle, SessionSpec, TokenUsage, TurnRequest,
};

/// Codex CLI implementation of Roba's provider-neutral turn boundary.
#[derive(Debug, Clone, Default)]
pub struct CodexProvider {
    binary: Option<PathBuf>,
}

const ROBA_SELF_SERVER: &str = "roba";
const ROBA_SELF_TOOL_APPROVAL: &str = "mcp_servers.roba.tools.self.approval_mode=\"approve\"";

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
        launch_context: &ProviderLaunchContext,
    ) -> Result<Codex, ProviderError> {
        let mut builder = Codex::builder();
        if let Some(binary) = &self.binary {
            builder = builder.binary(binary);
        }
        if let Some(seconds) = request.spec.execution.limits.timeout_secs {
            builder = builder.timeout_secs(seconds);
        }
        for (name, value) in mcp_configuration(launch_context).environment {
            builder = builder.env(name, value);
        }
        builder.build().map_err(map_error)
    }

    fn fresh_command(request: &TurnRequest, launch_context: &ProviderLaunchContext) -> ExecCommand {
        let mut command = ExecCommand::new(render_prompt(request)).prompt_via_stdin();
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
            // `codex exec` is non-interactive, so an approval request has no
            // operator to answer. WorkspaceWrite and FullAuto therefore use
            // the same Codex mechanics; their distinct Roba meanings remain
            // useful to callers deciding where the run may be launched.
            PermissionPolicy::WorkspaceWrite => command
                .sandbox(SandboxMode::WorkspaceWrite)
                .approval_policy(ApprovalPolicyConfig::Never),
            PermissionPolicy::FullAuto => command
                .sandbox(SandboxMode::WorkspaceWrite)
                .approval_policy(ApprovalPolicyConfig::Never),
        };
        for value in mcp_configuration(launch_context).overrides {
            command = command.config(value);
        }
        command
    }

    fn resume_command(
        request: &TurnRequest,
        session_id: &str,
        launch_context: &ProviderLaunchContext,
    ) -> ExecResumeCommand {
        // codex-wrapper 0.3.1 exposes stdin prompts for ExecCommand only.
        // ExecResumeCommand therefore still places the resumed prompt in argv.
        let mut command = ExecResumeCommand::new()
            .session_id(session_id)
            .prompt(render_prompt(request));
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
            // Resume is non-interactive for the same reason as fresh exec.
            PermissionPolicy::WorkspaceWrite => command
                .config("sandbox_mode=\"workspace-write\"")
                .approval_policy(ApprovalPolicyConfig::Never),
            PermissionPolicy::FullAuto => command
                .config("sandbox_mode=\"workspace-write\"")
                .approval_policy(ApprovalPolicyConfig::Never),
        };
        for value in mcp_configuration(launch_context).overrides {
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
                "Codex does not support the provider-neutral max effort; use xhigh",
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
        events: &'a dyn EventSink,
    ) -> ProviderFuture<'a> {
        self.execute_with_launch_context(request, ProviderLaunchContext::default(), events)
    }

    fn execute_with_launch_context<'a>(
        &'a self,
        request: TurnRequest,
        launch_context: ProviderLaunchContext,
        events: &'a dyn EventSink,
    ) -> ProviderFuture<'a> {
        Box::pin(async move {
            self.validate(&request)?;
            let codex = self.client(&request, &launch_context)?;
            let mut captured = Vec::new();
            let stream_result = {
                let mut capture = |event| {
                    emit_stream_event(&event, events);
                    captured.push(event);
                };
                match &request.spec.execution.session {
                    SessionSpec::Fresh => {
                        codex_wrapper::streaming::stream_exec(
                            &codex,
                            &Self::fresh_command(&request, &launch_context),
                            &mut capture,
                        )
                        .await
                    }
                    SessionSpec::Resume { session } => {
                        codex_wrapper::streaming::stream_exec_resume(
                            &codex,
                            &Self::resume_command(&request, &session.id, &launch_context),
                            &mut capture,
                        )
                        .await
                    }
                }
            };
            let resume_session = match &request.spec.execution.session {
                SessionSpec::Fresh => None,
                SessionSpec::Resume { session } => Some(session.clone()),
            };
            let wrapper_failure = stream_result
                .err()
                .map(map_error)
                .map(|error| preserve_resume_session(error, resume_session.as_ref()));
            if let Some(failure) = terminal_stream_failure(
                &captured,
                resume_session.as_ref(),
                wrapper_failure.as_ref(),
            ) {
                return Err(failure);
            }
            if let Some(failure) = wrapper_failure {
                return Err(failure);
            }
            if !captured.iter().any(JsonLineEvent::is_turn_completed) {
                return Err(preserve_resume_session(
                    ProviderError::new(
                        FailureKind::Provider,
                        "Codex stream ended without a turn.completed event",
                    ),
                    resume_session.as_ref(),
                ));
            }
            let result = QueryResult::from_events(captured);
            let mut outcome = Self::normalize(result);
            if outcome.session.is_none() {
                outcome.session = resume_session;
            }
            Ok(outcome)
        })
    }
}

fn terminal_stream_failure(
    events: &[JsonLineEvent],
    resume_session: Option<&SessionHandle>,
    wrapper_failure: Option<&ProviderError>,
) -> Option<ProviderError> {
    let terminal = events.iter().rev().find(|event| {
        event.is_turn_completed() || event.is_turn_failed() || event.event_type == "error"
    })?;
    if terminal.is_turn_completed() {
        return None;
    }

    let label = if terminal.is_turn_failed() {
        "Codex turn failed"
    } else {
        "Codex reported an error"
    };
    let message = terminal_error_message(terminal)
        .map(|message| format!("{label}: {message}"))
        .or_else(|| wrapper_failure.map(|failure| failure.message.clone()))
        .unwrap_or_else(|| format!("{label} without an error message"));
    let kind = wrapper_failure
        .map(|failure| failure.kind)
        .unwrap_or(FailureKind::Provider);

    Some(
        ProviderError::new(kind, message).with_details(terminal_failure_details(
            events,
            resume_session,
            wrapper_failure,
        )),
    )
}

fn terminal_error_message(event: &JsonLineEvent) -> Option<String> {
    let nested_error = event.extra.get("error");
    nested_error
        .and_then(|error| error.get("message"))
        .or_else(|| event.extra.get("message"))
        .and_then(serde_json::Value::as_str)
        .or_else(|| nested_error.and_then(serde_json::Value::as_str))
        .map(str::trim)
        .filter(|message| !message.is_empty())
        .map(str::to_string)
        .or_else(|| {
            nested_error
                .filter(|error| !error.is_null())
                .map(serde_json::Value::to_string)
        })
}

fn terminal_failure_details(
    events: &[JsonLineEvent],
    resume_session: Option<&SessionHandle>,
    wrapper_failure: Option<&ProviderError>,
) -> RunFailureDetails {
    let session_id = events
        .iter()
        .find_map(JsonLineEvent::thread_id)
        .or_else(|| events.iter().find_map(JsonLineEvent::session_id));
    let usage = events
        .iter()
        .rev()
        .find_map(JsonLineEvent::usage)
        .map(normalize_token_usage);
    let mut details = wrapper_failure
        .and_then(|failure| failure.details.as_deref())
        .cloned()
        .unwrap_or_default();
    details.session = session_id
        .map(|id| SessionHandle {
            provider: ProviderId::codex(),
            id: id.to_string(),
        })
        .or_else(|| details.session.take())
        .or_else(|| resume_session.cloned());
    if usage.is_some() {
        details.usage = usage;
    }
    details
}

fn preserve_resume_session(
    error: ProviderError,
    resume_session: Option<&SessionHandle>,
) -> ProviderError {
    let Some(resume_session) = resume_session else {
        return error;
    };
    let mut details = error.details.as_deref().cloned().unwrap_or_default();
    if details.session.is_none() {
        details.session = Some(resume_session.clone());
    }
    error.with_details(details)
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
        sink.emit(ProviderEvent::OutputDelta { text });
    }
    if let Some(usage) = event.usage() {
        sink.emit(ProviderEvent::Usage {
            usage: normalize_token_usage(usage),
        });
    }
}

struct CodexMcpConfiguration {
    overrides: Vec<String>,
    environment: Vec<(String, String)>,
}

fn mcp_configuration(launch_context: &ProviderLaunchContext) -> CodexMcpConfiguration {
    let mut configuration = CodexMcpConfiguration {
        overrides: Vec::new(),
        environment: Vec::new(),
    };
    for (index, endpoint) in launch_context.mcp_endpoints().iter().enumerate() {
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
        if endpoint.name() == ROBA_SELF_SERVER {
            configuration
                .overrides
                .push(ROBA_SELF_TOOL_APPROVAL.to_string());
        }
    }
    configuration
}

fn render_prompt(request: &TurnRequest) -> String {
    let mut sections = Vec::new();
    sections.extend(request.spec.agent.instructions.iter().cloned());
    sections.extend(request.spec.context.project.iter().cloned());
    sections.extend(request.spec.context.run.iter().cloned());
    sections.push(request.prompt.as_str().to_string());
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
        events: Mutex<Vec<ProviderEvent>>,
    }

    #[cfg(unix)]
    impl EventSink for RecordingSink {
        fn emit(&self, event: ProviderEvent) {
            self.events.lock().unwrap().push(event);
        }
    }

    #[cfg(unix)]
    fn fake_codex(temp: &tempfile::TempDir) -> (CodexProvider, PathBuf) {
        use std::os::unix::fs::PermissionsExt;

        let marker = temp.path().join("blocked.pid");
        let args_marker = temp.path().join("codex.args");
        let token_marker = temp.path().join("codex.token");
        let binary = temp.path().join("codex");
        let script = format!(
            r#"#!/bin/sh
printf '%s\n' "$@" > '{}'
printf '%s' "${{ROBA_INTERNAL_MCP_TOKEN_0-}}" > '{}'
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
if [ "$prompt" = "turn-failed" ]; then
  printf '%s\n' '{{"type":"thread.started","thread_id":"failed-thread"}}'
  printf '%s\n' '{{"type":"turn.failed","error":{{"message":"usage limit reached"}}}}'
  exit 17
fi
if [ "$prompt" = "turn-failed-auth" ]; then
  printf '%s\n' '{{"type":"thread.started","thread_id":"auth-thread"}}'
  printf '%s\n' '{{"type":"turn.failed","error":{{"message":"credentials rejected"}}}}'
  printf '%s\n' '401 Unauthorized' >&2
  exit 1
fi
if [ "$prompt" = "error-event" ]; then
  printf '%s\n' '{{"type":"thread.started","session_id":"error-session"}}'
  printf '%s\n' '{{"type":"error","message":"transport stopped"}}'
  exit 0
fi
resume_prompt=
for arg in "$@"; do
  case "$arg" in
    resume-no-thread|resume-failed-no-thread) resume_prompt=$arg ;;
  esac
done
if [ "$resume_prompt" = "resume-no-thread" ]; then
  printf '%s\n' '{{"type":"item.completed","item":{{"type":"agent_message","text":"resumed without start"}}}}'
  printf '%s\n' '{{"type":"turn.completed","usage":{{"input_tokens":4,"output_tokens":3}}}}'
  exit 0
fi
if [ "$resume_prompt" = "resume-failed-no-thread" ]; then
  printf '%s\n' '{{"type":"turn.failed","error":{{"message":"resume failed"}}}}'
  exit 19
fi
case " $* " in
  *" resume "*) text=resumed ;;
  *) text=opened ;;
esac
printf '%s\n' '{{"type":"thread.started","thread_id":"thread-1"}}'
printf '{{"type":"item.completed","item":{{"type":"agent_message","text":"%s"}}}}\n' "$text"
printf '%s\n' '{{"type":"turn.completed","usage":{{"input_tokens":3,"output_tokens":2}}}}'
"#,
            args_marker.display(),
            token_marker.display(),
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

    fn launch_context() -> ProviderLaunchContext {
        ProviderLaunchContext::default().with_mcp_endpoint(
            crate::provider::ProviderMcpEndpoint::new(
                ROBA_SELF_SERVER,
                "http://127.0.0.1:4123/mcp",
                "secret-provider-token",
            ),
        )
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
        assert_eq!(render_prompt(&turn), "agent\n\nproject\n\nrun\n\nhello");
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
    fn non_interactive_commands_use_explicit_sandbox_and_never_approval() {
        for (policy, sandbox) in [
            (PermissionPolicy::ReadOnly, "read-only"),
            (PermissionPolicy::WorkspaceWrite, "workspace-write"),
            (PermissionPolicy::FullAuto, "workspace-write"),
        ] {
            let mut turn = request();
            turn.spec.execution.permissions = policy;

            let fresh =
                CodexProvider::fresh_command(&turn, &ProviderLaunchContext::default()).args();
            assert!(
                fresh
                    .windows(2)
                    .any(|args| { args == ["-c", "approval_policy=\"never\""] })
            );
            assert!(fresh.windows(2).any(|args| args == ["--sandbox", sandbox]));
            assert!(!fresh.iter().any(|arg| arg == "--skip-git-repo-check"));
            assert!(fresh.iter().any(|arg| arg == "-"));
            assert!(!fresh.iter().any(|arg| arg == "hello"));

            let resumed = CodexProvider::resume_command(
                &turn,
                "known-thread",
                &ProviderLaunchContext::default(),
            )
            .args();
            assert!(
                resumed
                    .windows(2)
                    .any(|args| { args == ["-c", "approval_policy=\"never\""] })
            );
            let sandbox_config = format!("sandbox_mode=\"{sandbox}\"");
            assert!(
                resumed
                    .windows(2)
                    .any(|args| args == ["-c", sandbox_config.as_str()])
            );
            assert!(!resumed.iter().any(|arg| arg == "--skip-git-repo-check"));
            // codex-wrapper 0.3.1 has no resume-stdin API.
            assert!(resumed.iter().any(|arg| arg == "hello"));
        }
    }

    #[test]
    fn launch_context_builds_exact_mcp_overrides_and_secret_environment() {
        let context = launch_context();
        let configuration = mcp_configuration(&context);
        assert_eq!(
            configuration.overrides,
            [
                "mcp_servers.roba.url=\"http://127.0.0.1:4123/mcp\"",
                "mcp_servers.roba.bearer_token_env_var=\"ROBA_INTERNAL_MCP_TOKEN_0\"",
                "mcp_servers.roba.required=true",
                ROBA_SELF_TOOL_APPROVAL,
            ]
        );
        assert_eq!(
            configuration.environment,
            [(
                "ROBA_INTERNAL_MCP_TOKEN_0".to_string(),
                "secret-provider-token".to_string()
            )]
        );

        for args in [
            CodexProvider::fresh_command(&request(), &context).args(),
            CodexProvider::resume_command(&request(), "thread-1", &context).args(),
        ] {
            for expected in &configuration.overrides {
                assert!(
                    args.windows(2)
                        .any(|pair| { pair[0] == "-c" && pair[1] == *expected })
                );
            }
            assert!(!args.iter().any(|arg| arg.contains("secret-provider-token")));
        }
        assert!(!format!("{context:?}").contains("secret-provider-token"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn fake_binary_receives_mcp_overrides_and_secret_only_in_environment() {
        let temp = tempfile::tempdir().unwrap();
        let (provider, _) = fake_codex(&temp);
        let events = RecordingSink::default();

        let outcome = provider
            .execute_with_launch_context(request(), launch_context(), &events)
            .await
            .unwrap();

        assert_eq!(outcome.output, "opened");
        let args = std::fs::read_to_string(temp.path().join("codex.args")).unwrap();
        for expected in [
            "mcp_servers.roba.url=\"http://127.0.0.1:4123/mcp\"",
            "mcp_servers.roba.bearer_token_env_var=\"ROBA_INTERNAL_MCP_TOKEN_0\"",
            "mcp_servers.roba.required=true",
            ROBA_SELF_TOOL_APPROVAL,
        ] {
            assert!(
                args.lines().any(|arg| arg == expected),
                "missing {expected}"
            );
        }
        assert!(!args.contains("secret-provider-token"));
        assert_eq!(
            std::fs::read_to_string(temp.path().join("codex.token")).unwrap(),
            "secret-provider-token"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn fake_binary_streams_open_and_resume_into_normalized_events() {
        let temp = tempfile::tempdir().unwrap();
        let (provider, _) = fake_codex(&temp);

        let fresh_events = RecordingSink::default();
        let fresh = provider.execute(request(), &fresh_events).await.unwrap();
        assert_eq!(fresh.output, "opened");
        assert_eq!(fresh.session.as_ref().unwrap().id, "thread-1");
        assert_eq!(fresh.usage.as_ref().unwrap().input, Some(3));
        assert_eq!(fresh.usage.as_ref().unwrap().output, Some(2));
        let fresh_events = fresh_events.events.into_inner().unwrap();
        assert!(fresh_events.contains(&ProviderEvent::OutputDelta {
            text: "opened".to_string(),
        }));
        assert!(fresh_events.iter().any(|event| matches!(
            event,
            ProviderEvent::Usage { usage }
                if usage.input == Some(3) && usage.output == Some(2)
        )));

        let mut resumed = request();
        resumed.spec.execution.session = SessionSpec::Resume {
            session: SessionHandle {
                provider: ProviderId::codex(),
                id: "thread-1".to_string(),
            },
        };
        let resume_events = RecordingSink::default();
        let resumed = provider.execute(resumed, &resume_events).await.unwrap();
        assert_eq!(resumed.output, "resumed");
        assert!(
            resume_events
                .events
                .into_inner()
                .unwrap()
                .contains(&ProviderEvent::OutputDelta {
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

        // Full-workspace test runs can briefly starve child startup on loaded
        // CI hosts; this deadline is only a harness guard and returns as soon
        // as the marker appears.
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            while !marker.exists() {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("fake Codex process did not start");
        let pid = std::fs::read_to_string(&marker).unwrap();

        run.handle().cancel().await.unwrap();
        assert_eq!(run.handle().wait().await.state, RunState::Cancelled);
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
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

        let error = provider.execute(request, &events).await.unwrap_err();

        assert_eq!(error.kind, FailureKind::Provider);
        assert!(error.message.contains("without a turn.completed event"));
        let events = events.events.into_inner().unwrap();
        assert!(events.contains(&ProviderEvent::OutputDelta {
            text: "partial".to_string(),
        }));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn turn_failed_event_preserves_reason_and_thread_without_inventing_a_kind() {
        let temp = tempfile::tempdir().unwrap();
        let (provider, _) = fake_codex(&temp);
        let request = RunSpec::suspended(AgentSpec::new(ProviderId::codex()))
            .with_prompt(Prompt::new("turn-failed").unwrap())
            .into_turn()
            .unwrap();

        let error = provider
            .execute(request, &RecordingSink::default())
            .await
            .unwrap_err();

        assert_eq!(error.kind, FailureKind::Provider);
        assert_eq!(error.message, "Codex turn failed: usage limit reached");
        assert_eq!(
            error
                .details
                .as_deref()
                .and_then(|details| details.session.as_ref())
                .map(|session| session.id.as_str()),
            Some("failed-thread")
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn terminal_event_keeps_the_wrappers_typed_failure_kind() {
        let temp = tempfile::tempdir().unwrap();
        let (provider, _) = fake_codex(&temp);
        let request = RunSpec::suspended(AgentSpec::new(ProviderId::codex()))
            .with_prompt(Prompt::new("turn-failed-auth").unwrap())
            .into_turn()
            .unwrap();

        let error = provider
            .execute(request, &RecordingSink::default())
            .await
            .unwrap_err();

        assert_eq!(error.kind, FailureKind::Authentication);
        assert_eq!(error.message, "Codex turn failed: credentials rejected");
        assert_eq!(
            error
                .details
                .as_deref()
                .and_then(|details| details.session.as_ref())
                .map(|session| session.id.as_str()),
            Some("auth-thread")
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn resumed_success_falls_back_to_the_requested_session() {
        let temp = tempfile::tempdir().unwrap();
        let (provider, _) = fake_codex(&temp);
        let mut request = RunSpec::suspended(AgentSpec::new(ProviderId::codex()))
            .with_prompt(Prompt::new("resume-no-thread").unwrap())
            .into_turn()
            .unwrap();
        request.spec.execution.session = SessionSpec::Resume {
            session: SessionHandle {
                provider: ProviderId::codex(),
                id: "known-thread".to_string(),
            },
        };

        let outcome = provider
            .execute(request, &RecordingSink::default())
            .await
            .unwrap();

        assert_eq!(outcome.output, "resumed without start");
        assert_eq!(outcome.session.unwrap().id, "known-thread");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn resumed_failure_falls_back_to_the_requested_session() {
        let temp = tempfile::tempdir().unwrap();
        let (provider, _) = fake_codex(&temp);
        let mut request = RunSpec::suspended(AgentSpec::new(ProviderId::codex()))
            .with_prompt(Prompt::new("resume-failed-no-thread").unwrap())
            .into_turn()
            .unwrap();
        request.spec.execution.session = SessionSpec::Resume {
            session: SessionHandle {
                provider: ProviderId::codex(),
                id: "known-thread".to_string(),
            },
        };

        let error = provider
            .execute(request, &RecordingSink::default())
            .await
            .unwrap_err();

        assert_eq!(error.kind, FailureKind::Provider);
        assert_eq!(error.message, "Codex turn failed: resume failed");
        assert_eq!(
            error
                .details
                .as_deref()
                .and_then(|details| details.session.as_ref())
                .map(|session| session.id.as_str()),
            Some("known-thread")
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn bare_error_event_preserves_reason_and_session_id() {
        let temp = tempfile::tempdir().unwrap();
        let (provider, _) = fake_codex(&temp);
        let request = RunSpec::suspended(AgentSpec::new(ProviderId::codex()))
            .with_prompt(Prompt::new("error-event").unwrap())
            .into_turn()
            .unwrap();

        let error = provider
            .execute(request, &RecordingSink::default())
            .await
            .unwrap_err();

        assert_eq!(error.kind, FailureKind::Provider);
        assert_eq!(error.message, "Codex reported an error: transport stopped");
        assert_eq!(
            error
                .details
                .as_deref()
                .and_then(|details| details.session.as_ref())
                .map(|session| session.id.as_str()),
            Some("error-session")
        );
    }
}
