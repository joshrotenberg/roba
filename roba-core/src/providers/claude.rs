//! Claude Code provider adapter.

use std::path::PathBuf;

use anyhow::Result;
use claude_wrapper::streaming::{BlockDelta, PartialMessageEvent, StreamEvent, stream_query};
use claude_wrapper::types::QueryResult;
use claude_wrapper::{
    Claude, Effort as ClaudeEffort, McpConfigBuilder, OutputFormat, QueryCommand, TempMcpConfig,
};

use crate::engine::{self, Config, Permissions, Session};
use crate::provider::{
    EventSink, Provider, ProviderCapabilities, ProviderContext, ProviderError, ProviderFuture,
};
use crate::run::{
    Cost, Effort, FailureKind, PermissionPolicy, ProviderId, RunEvent, RunOutcome, SessionHandle,
    SessionSpec, TokenUsage, TurnRequest,
};

/// Claude Code implementation of Roba's provider-neutral turn boundary.
#[derive(Debug, Clone, Default)]
pub struct ClaudeProvider {
    binary: Option<PathBuf>,
}

/// Backward-compatible default provider value for callers that constructed the
/// former unit struct as `ClaudeProvider`.
#[allow(non_upper_case_globals)]
pub const ClaudeProvider: ClaudeProvider = ClaudeProvider { binary: None };

const WORKER_GUIDANCE: &str = "Roba owns all child work for this run. When the task calls for workers, use only the `mcp__roba_workers__spawn_worker` tool, then use `mcp__roba_workers__workers` and wait for every spawned worker before answering. Never launch Roba or provider CLIs in the shell to simulate workers, and never substitute provider-native subagents. If Roba refuses a spawn, report that refusal instead of using another mechanism.";

impl ClaudeProvider {
    /// Use an explicit Claude executable instead of resolving `claude` from
    /// `PATH`. This is useful for embedded hosts and deterministic tests.
    pub fn with_binary(binary: impl Into<PathBuf>) -> Self {
        Self {
            binary: Some(binary.into()),
        }
    }

    fn client(&self, config: &Config) -> Result<Claude> {
        let mut builder = Claude::builder();
        if let Some(binary) = &self.binary {
            builder = builder.binary(binary);
        }
        if let Some(secs) = config.timeout_secs
            && secs > 0
        {
            builder = builder.timeout_secs(secs);
        }
        builder.build().map_err(Into::into)
    }

    /// Execute the pre-pivot Claude config without changing its behavior.
    /// This is the compatibility seam used by [`crate::engine::run`].
    pub async fn execute_legacy(&self, config: &Config) -> Result<QueryResult> {
        let claude = self.client(config)?;
        let mut result = engine::execute(config, &claude).await?;
        if config.json_schema.is_some() {
            engine::surface_structured_output(&mut result);
        }
        Ok(result)
    }

    fn bounded_command(config: &Config) -> QueryCommand {
        // Roba owns bounded child-run creation. Claude's native Agent tool is
        // not represented in the run tree and would bypass worker count,
        // depth, cancellation, and event observation.
        engine::query_command(config)
            // `stream_query` consumes NDJSON but does not override the command's
            // output format or forward a stdin prompt itself.
            .output_format(OutputFormat::StreamJson)
            .prompt_via_stdin(false)
            .disallowed_tool("Agent")
            .include_partial_messages()
    }

    fn config(request: &TurnRequest) -> Result<Config, ProviderError> {
        let mut config = Config::new(request.prompt.as_str());
        config.model.clone_from(&request.spec.agent.model);
        config.effort = request.spec.agent.effort.map(map_effort);
        config.permissions = match request.spec.execution.permissions {
            PermissionPolicy::ReadOnly => Permissions::ReadOnly,
            PermissionPolicy::WorkspaceWrite => Permissions::Writable,
            PermissionPolicy::FullAuto => Permissions::FullAuto,
        };
        config
            .allow_tools
            .clone_from(&request.spec.execution.tools.allow);
        config
            .deny_tools
            .clone_from(&request.spec.execution.tools.deny);
        config.max_turns = request.spec.execution.limits.max_turns;
        config.max_budget_usd = request.spec.execution.limits.max_cost_usd;
        config.timeout_secs = request.spec.execution.limits.timeout_secs;
        config.session = match &request.spec.execution.session {
            SessionSpec::Fresh => Session::Fresh,
            SessionSpec::Resume { session } => {
                if session.provider != ProviderId::claude() {
                    return Err(ProviderError::unsupported(format!(
                        "cannot resume {} session with Claude provider",
                        session.provider
                    )));
                }
                Session::Resume(session.id.clone())
            }
        };

        let mut instructions = request.spec.agent.instructions.clone();
        instructions.extend(request.spec.context.project.iter().cloned());
        instructions.extend(request.spec.context.run.iter().cloned());
        if !instructions.is_empty() {
            config.append_system_prompt = Some(instructions.join("\n\n"));
        }
        Ok(config)
    }

    fn mcp_config(context: &ProviderContext) -> Result<Option<TempMcpConfig>, ProviderError> {
        if context.mcp_endpoints().is_empty() {
            return Ok(None);
        }
        let mut builder = McpConfigBuilder::new();
        for endpoint in context.mcp_endpoints() {
            builder = builder.http_server_with_headers(
                endpoint.name(),
                endpoint.url(),
                [(
                    "Authorization",
                    format!("Bearer {}", endpoint.bearer_token()),
                )],
            );
        }
        builder.build_temp().map(Some).map_err(|error| {
            ProviderError::new(
                FailureKind::Provider,
                format!("failed to build Claude MCP configuration: {error}"),
            )
        })
    }

    fn configure_internal_worker_control(config: &mut Config, context: &ProviderContext) {
        if context
            .mcp_endpoints()
            .iter()
            .any(|endpoint| endpoint.name() == "roba_workers")
        {
            config
                .allow_tools
                .push("mcp__roba_workers__spawn_worker".to_string());
            config
                .allow_tools
                .push("mcp__roba_workers__workers".to_string());
            config.append_system_prompt = Some(match config.append_system_prompt.take() {
                Some(existing) => format!("{existing}\n\n{WORKER_GUIDANCE}"),
                None => WORKER_GUIDANCE.to_string(),
            });
        }
    }

    /// Normalize Claude's result without converting missing telemetry to zero.
    pub fn normalize(result: QueryResult) -> RunOutcome {
        let structured_output = result.extra.get("structured_output").cloned();
        let usage = result.usage.as_ref().map(normalize_usage);
        RunOutcome {
            output: result.result,
            session: (!result.session_id.is_empty()).then(|| SessionHandle {
                provider: ProviderId::claude(),
                id: result.session_id,
            }),
            usage,
            cost: result.cost_usd.map(Cost::usd),
            duration_ms: result.duration_ms,
            provider_turns: result.num_turns,
            structured_output,
        }
    }
}

fn normalize_usage(usage: &claude_wrapper::types::TokenUsage) -> TokenUsage {
    TokenUsage {
        input: usage.input_tokens,
        cached_input: usage.cached_input_tokens,
        cache_write_input: usage.cache_write_input_tokens,
        output: usage.output_tokens,
        reasoning_output: usage.reasoning_output_tokens,
        total: usage.total_tokens,
    }
}

impl Provider for ClaudeProvider {
    fn id(&self) -> ProviderId {
        ProviderId::claude()
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            resume: true,
            streaming: true,
            read_only: true,
            workspace_write: true,
            full_auto: true,
            max_turns: true,
            max_cost: true,
            timeout: true,
        }
    }

    fn validate(&self, request: &TurnRequest) -> Result<(), ProviderError> {
        if request.spec.agent.provider != ProviderId::claude() {
            return Err(ProviderError::unsupported(format!(
                "Claude adapter cannot execute provider {}",
                request.spec.agent.provider
            )));
        }
        if request.spec.execution.limits.max_turns == Some(0) {
            return Err(ProviderError::unsupported(
                "Claude max_turns must be greater than zero",
            ));
        }
        if request.spec.execution.limits.timeout_secs == Some(0) {
            return Err(ProviderError::unsupported(
                "Claude timeout_secs must be greater than zero",
            ));
        }
        if request
            .spec
            .execution
            .limits
            .max_cost_usd
            .is_some_and(|cost| !cost.is_finite() || cost <= 0.0)
        {
            return Err(ProviderError::unsupported(
                "Claude max_cost_usd must be finite and greater than zero",
            ));
        }
        Self::config(request).map(|_| ())
    }

    fn execute<'a>(
        &'a self,
        request: TurnRequest,
        context: ProviderContext,
        events: &'a dyn EventSink,
    ) -> ProviderFuture<'a> {
        Box::pin(async move {
            self.validate(&request)?;
            events.emit(RunEvent::TurnStarted {
                provider: ProviderId::claude(),
            });
            let mut config = Self::config(&request)?;
            let mcp_config = Self::mcp_config(&context)?;
            if let Some(mcp_config) = &mcp_config {
                config.mcp_config.push(mcp_config.path().to_string());
            }
            Self::configure_internal_worker_control(&mut config, &context);
            let claude = self.client(&config).map_err(|error| {
                ProviderError::new(FailureKind::Provider, format!("Claude failed: {error:#}"))
            })?;
            let mut terminal = None;
            let mut terminal_error = None;
            stream_query(&claude, &Self::bounded_command(&config), |event| {
                emit_stream_event(&event, events);
                if event.is_result() {
                    match serde_json::from_value::<QueryResult>(event.data) {
                        Ok(result) => {
                            if let Some(usage) = result.usage.as_ref() {
                                events.emit(RunEvent::Usage {
                                    usage: normalize_usage(usage),
                                });
                            }
                            terminal = Some(result);
                        }
                        Err(error) => terminal_error = Some(error),
                    }
                }
            })
            .await
            .map_err(map_error)?;
            if let Some(error) = terminal_error {
                return Err(ProviderError::new(
                    FailureKind::Provider,
                    format!("Claude result event was invalid: {error}"),
                ));
            }
            let mut result = terminal.ok_or_else(|| {
                ProviderError::new(
                    FailureKind::Provider,
                    "Claude stream ended without a result event",
                )
            })?;
            if config.json_schema.is_some() {
                engine::surface_structured_output(&mut result);
            }
            if result.is_error {
                return Err(ProviderError::new(
                    FailureKind::Provider,
                    if result.result.is_empty() {
                        "Claude returned an unusable error result".to_string()
                    } else {
                        result.result
                    },
                ));
            }
            let outcome = Self::normalize(result);
            events.emit(RunEvent::TurnCompleted {
                outcome: outcome.clone(),
            });
            Ok(outcome)
        })
    }
}

fn emit_stream_event(event: &StreamEvent, sink: &dyn EventSink) {
    if let Some(PartialMessageEvent::BlockDelta {
        delta: BlockDelta::Text(text),
        ..
    }) = event.partial_message()
    {
        sink.emit(RunEvent::OutputDelta { text });
    }
}

fn map_error(error: claude_wrapper::Error) -> ProviderError {
    let kind = match error {
        claude_wrapper::Error::Auth { .. } => FailureKind::Authentication,
        claude_wrapper::Error::Timeout { .. } => FailureKind::Timeout,
        claude_wrapper::Error::MaxTurnsExceeded { .. }
        | claude_wrapper::Error::MaxBudgetExceeded { .. }
        | claude_wrapper::Error::BudgetExceeded { .. } => FailureKind::Limit,
        claude_wrapper::Error::VersionMismatch { .. }
        | claude_wrapper::Error::UntestedCliVersion { .. }
        | claude_wrapper::Error::DangerousNotAllowed { .. } => FailureKind::Unsupported,
        _ => FailureKind::Provider,
    };
    ProviderError::new(kind, format!("Claude failed: {error}"))
}

fn map_effort(effort: Effort) -> ClaudeEffort {
    match effort {
        Effort::Low => ClaudeEffort::Low,
        Effort::Medium => ClaudeEffort::Medium,
        Effort::High => ClaudeEffort::High,
        Effort::XHigh => ClaudeEffort::Xhigh,
        Effort::Max => ClaudeEffort::Max,
    }
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use std::sync::Mutex;

    use claude_wrapper::ClaudeCommand;
    use serde_json::json;

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
    fn fake_claude(temp: &tempfile::TempDir) -> (ClaudeProvider, PathBuf) {
        use std::os::unix::fs::PermissionsExt;

        let marker = temp.path().join("blocked.pid");
        let binary = temp.path().join("claude");
        let script = format!(
            r#"#!/bin/sh
prompt=
resuming=false
for arg do
  prompt=$arg
  if [ "$arg" = --resume ]; then
    resuming=true
  fi
done
case "$prompt" in
  block)
    printf '%s' "$$" > '{}'
    exec sleep 30
    ;;
  unterminated)
    text=partial
    terminal=false
    ;;
  *)
    if [ "$resuming" = true ]; then
      text=resumed
    else
      text=opened
    fi
    terminal=true
    ;;
esac
printf '{{"type":"stream_event","session_id":"session-1","event":{{"type":"content_block_delta","index":0,"delta":{{"type":"text_delta","text":"%s"}}}}}}\n' "$text"
if [ "$terminal" = true ]; then
  printf '{{"type":"result","subtype":"success","result":"%s","session_id":"session-1","total_cost_usd":0.02,"duration_ms":10,"num_turns":1,"is_error":false,"usage":{{"input_tokens":3,"output_tokens":2}}}}\n' "$text"
fi
"#,
            marker.display()
        );
        std::fs::write(&binary, script).unwrap();
        let mut permissions = std::fs::metadata(&binary).unwrap().permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&binary, permissions).unwrap();
        (ClaudeProvider::with_binary(binary), marker)
    }

    fn request(provider: ProviderId) -> TurnRequest {
        RunSpec::suspended(AgentSpec::new(provider))
            .with_prompt(Prompt::new("hello").unwrap())
            .into_turn()
            .unwrap()
    }

    #[test]
    fn normalizes_only_reported_telemetry() {
        let result: QueryResult = serde_json::from_value(json!({
            "result": "done",
            "session_id": "session-1",
            "total_cost_usd": 0.25,
            "duration_ms": 123,
            "num_turns": 2,
            "usage": {
                "input_tokens": 10,
                "output_tokens": 4
            },
            "structured_output": {"ok": true},
            "is_error": false
        }))
        .unwrap();

        let outcome = ClaudeProvider::normalize(result);
        assert_eq!(outcome.output, "done");
        assert_eq!(outcome.session.unwrap().id, "session-1");
        assert_eq!(outcome.cost, Some(Cost::usd(0.25)));
        assert_eq!(outcome.usage.as_ref().unwrap().input, Some(10));
        assert_eq!(outcome.usage.as_ref().unwrap().cached_input, None);
        assert_eq!(outcome.structured_output, Some(json!({"ok": true})));
    }

    #[test]
    fn validates_before_provider_launch() {
        let provider = ClaudeProvider;
        let mut invalid = request(ProviderId::claude());
        invalid.spec.execution.limits.max_cost_usd = Some(f64::NAN);
        let error = provider.validate(&invalid).unwrap_err();
        assert_eq!(error.kind, FailureKind::Unsupported);

        let mismatch = request(ProviderId::codex());
        assert_eq!(
            provider.validate(&mismatch).unwrap_err().kind,
            FailureKind::Unsupported
        );
    }

    #[test]
    fn rejects_cross_provider_session_resume() {
        let provider = ClaudeProvider::default();
        let mut request = request(ProviderId::claude());
        request.spec.execution.session = SessionSpec::Resume {
            session: SessionHandle {
                provider: ProviderId::codex(),
                id: "thread-1".to_string(),
            },
        };
        assert_eq!(
            provider.validate(&request).unwrap_err().kind,
            FailureKind::Unsupported
        );
    }

    #[test]
    fn worker_mcp_configuration_is_ephemeral_and_authenticated() {
        let context =
            ProviderContext::default().with_mcp_endpoint(crate::ProviderMcpEndpoint::new(
                "roba_workers",
                "http://127.0.0.1:4123/mcp",
                "secret-worker-token",
            ));
        let config = ClaudeProvider::mcp_config(&context).unwrap().unwrap();
        let json = std::fs::read_to_string(config.path()).unwrap();
        let mut request_config = Config::new("test");
        ClaudeProvider::configure_internal_worker_control(&mut request_config, &context);

        assert!(json.contains("roba_workers"));
        assert!(json.contains("http://127.0.0.1:4123/mcp"));
        assert!(json.contains("Bearer secret-worker-token"));
        assert_eq!(
            request_config.allow_tools,
            [
                "mcp__roba_workers__spawn_worker",
                "mcp__roba_workers__workers"
            ]
        );
        assert_eq!(
            request_config.append_system_prompt.as_deref(),
            Some(WORKER_GUIDANCE)
        );
        assert!(
            request_config
                .append_system_prompt
                .as_deref()
                .unwrap()
                .contains("Never launch Roba or provider CLIs in the shell")
        );
        let args = ClaudeProvider::bounded_command(&request_config).args();
        assert!(args.iter().any(|arg| arg == "--disallowed-tools"));
        assert!(args.iter().any(|arg| arg == "Agent"));
        assert!(args.iter().any(|arg| arg == "--include-partial-messages"));
        assert!(
            args.windows(2)
                .any(|args| args[0] == "--output-format" && args[1] == "stream-json")
        );
        assert!(!format!("{context:?}").contains("secret-worker-token"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn fake_binary_streams_open_and_resume_into_normalized_events() {
        let temp = tempfile::tempdir().unwrap();
        let (provider, _) = fake_claude(&temp);

        let fresh_events = RecordingSink::default();
        let fresh = provider
            .execute(
                request(ProviderId::claude()),
                ProviderContext::default(),
                &fresh_events,
            )
            .await
            .unwrap();
        assert_eq!(fresh.output, "opened");
        assert_eq!(fresh.session.as_ref().unwrap().id, "session-1");
        assert_eq!(fresh.usage.as_ref().unwrap().input, Some(3));
        assert_eq!(fresh.usage.as_ref().unwrap().output, Some(2));
        assert_eq!(fresh.cost, Some(Cost::usd(0.02)));
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

        let mut resumed = request(ProviderId::claude());
        resumed.spec.execution.session = SessionSpec::Resume {
            session: SessionHandle {
                provider: ProviderId::claude(),
                id: "session-1".to_string(),
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
        let (provider, marker) = fake_claude(&temp);
        let run = crate::Run::new(
            RunSpec::suspended(AgentSpec::new(ProviderId::claude()))
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
        .expect("fake Claude process did not start");
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
        .expect("cancelled Claude process remained alive");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn successful_process_without_result_event_fails_closed() {
        let temp = tempfile::tempdir().unwrap();
        let (provider, _) = fake_claude(&temp);
        let request = RunSpec::suspended(AgentSpec::new(ProviderId::claude()))
            .with_prompt(Prompt::new("unterminated").unwrap())
            .into_turn()
            .unwrap();
        let events = RecordingSink::default();

        let error = provider
            .execute(request, ProviderContext::default(), &events)
            .await
            .unwrap_err();

        assert_eq!(error.kind, FailureKind::Provider);
        assert!(error.message.contains("without a result event"));
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
