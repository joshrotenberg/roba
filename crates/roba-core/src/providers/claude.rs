//! Claude Code provider adapter.

use std::path::PathBuf;

use anyhow::Result;
use claude_wrapper::streaming::{BlockDelta, PartialMessageEvent, StreamEvent, stream_query};
use claude_wrapper::types::QueryResult;
use claude_wrapper::{
    Claude, Effort as ClaudeEffort, McpConfigBuilder, OutputFormat, QueryCommand, TempMcpConfig,
    ToolPattern,
};

use crate::engine::{self, Config, Permissions, Session};
use crate::provider::{
    EventSink, Provider, ProviderCapabilities, ProviderError, ProviderEvent, ProviderFuture,
    ProviderLaunchContext,
};
use crate::run::{
    Cost, Effort, FailureKind, PermissionPolicy, ProviderId, RunFailureDetails, RunOutcome,
    SessionHandle, SessionSpec, TokenUsage, TurnRequest,
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
        engine::query_command(config)
            // `stream_query` consumes NDJSON but does not override the command's
            // output format or forward a stdin prompt itself.
            .output_format(OutputFormat::StreamJson)
            .prompt_via_stdin(false)
            .include_partial_messages()
    }

    fn config(request: &TurnRequest) -> Result<Config, ProviderError> {
        let mut config = Config::new(request.prompt.as_str());
        // The legacy notice describes a one-shot process that will never be
        // resumed. A bounded RunHandle may steer through resumed turns, so
        // injecting that notice here would contradict the lifecycle contract.
        config.no_agent_notice = true;
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

    fn mcp_config(
        launch_context: &ProviderLaunchContext,
    ) -> Result<Option<TempMcpConfig>, ProviderError> {
        if launch_context.mcp_endpoints().is_empty() {
            return Ok(None);
        }
        let mut builder = McpConfigBuilder::new();
        for endpoint in launch_context.mcp_endpoints() {
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

    fn allow_mcp_tools(config: &mut Config, launch_context: &ProviderLaunchContext) {
        for endpoint in launch_context.mcp_endpoints() {
            for tool_name in endpoint.tool_names() {
                let qualified = ToolPattern::mcp(endpoint.name(), tool_name)
                    .as_str()
                    .to_owned();
                if !config.allow_tools.contains(&qualified) {
                    config.allow_tools.push(qualified);
                }
            }
        }
    }

    fn apply_bootstrap(config: &mut Config, launch_context: &ProviderLaunchContext) {
        let Some(bootstrap) = launch_context.bootstrap_instruction() else {
            return;
        };
        config.append_system_prompt = Some(match config.append_system_prompt.take() {
            Some(existing) => format!("{bootstrap}\n\n{existing}"),
            None => bootstrap.to_owned(),
        });
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
            let mut config = Self::config(&request)?;
            Self::apply_bootstrap(&mut config, &launch_context);
            let mcp_config = Self::mcp_config(&launch_context)?;
            if let Some(mcp_config) = &mcp_config {
                config.mcp_config.push(mcp_config.path().to_string());
            }
            Self::allow_mcp_tools(&mut config, &launch_context);
            let claude = self.client(&config).map_err(|error| {
                ProviderError::new(FailureKind::Provider, format!("Claude failed: {error:#}"))
            })?;
            let mut terminal = None;
            let mut terminal_error = None;
            let stream_result = stream_query(&claude, &Self::bounded_command(&config), |event| {
                emit_stream_event(&event, events);
                if event.is_result() {
                    match serde_json::from_value::<QueryResult>(event.data) {
                        Ok(result) => {
                            if let Some(usage) = result.usage.as_ref() {
                                events.emit(ProviderEvent::Usage {
                                    usage: normalize_usage(usage),
                                });
                            }
                            terminal = Some(result);
                        }
                        Err(error) => terminal_error = Some(error),
                    }
                }
            })
            .await;
            if let Some(error) = terminal_error {
                return Err(ProviderError::new(
                    FailureKind::Provider,
                    format!("Claude result event was invalid: {error}"),
                ));
            }
            if terminal.as_ref().is_some_and(|result| result.is_error) {
                return Err(terminal_failure(terminal.expect("checked above")));
            }
            stream_result.map_err(map_error)?;
            let mut result = terminal.ok_or_else(|| {
                ProviderError::new(
                    FailureKind::Provider,
                    "Claude stream ended without a result event",
                )
            })?;
            if config.json_schema.is_some() {
                engine::surface_structured_output(&mut result);
            }
            let outcome = Self::normalize(result);
            Ok(outcome)
        })
    }
}

fn terminal_failure(result: QueryResult) -> ProviderError {
    let subtype = result
        .extra
        .get("subtype")
        .and_then(serde_json::Value::as_str);
    let kind = match subtype {
        Some("error_max_turns") => FailureKind::MaxTurns,
        Some("error_max_budget_usd") => FailureKind::MaxCost,
        _ => FailureKind::Provider,
    };
    let reported_reason = result
        .extra
        .get("errors")
        .and_then(serde_json::Value::as_array)
        .and_then(|errors| errors.iter().find_map(serde_json::Value::as_str))
        .or_else(|| (!result.result.is_empty()).then_some(result.result.as_str()));
    let details = terminal_details(&result);
    let message = if matches!(kind, FailureKind::MaxTurns | FailureKind::MaxCost) {
        let turns = result
            .num_turns
            .map(|turns| format!(" after {turns} provider turns"))
            .unwrap_or_default();
        let label = match subtype {
            Some("error_max_turns") => "max-turns limit",
            Some("error_max_budget_usd") => "max-budget limit",
            _ => "provider limit",
        };
        let reason = reported_reason
            .map(|reason| format!(": {reason}"))
            .unwrap_or_default();
        format!("Claude hit {label}{turns}{reason}")
    } else {
        reported_reason.map(str::to_owned).unwrap_or_else(|| {
            subtype
                .map(|subtype| format!("Claude returned {subtype}"))
                .unwrap_or_else(|| "Claude returned an unusable error result".to_string())
        })
    };

    ProviderError::new(kind, message).with_details(details)
}

fn terminal_details(result: &QueryResult) -> RunFailureDetails {
    let usage = result
        .usage
        .as_ref()
        .map(normalize_usage)
        .filter(|usage| !usage.is_unreported());
    RunFailureDetails {
        session: (!result.session_id.is_empty()).then(|| SessionHandle {
            provider: ProviderId::claude(),
            id: result.session_id.clone(),
        }),
        usage,
        cost: result.cost_usd.map(Cost::usd),
        duration_ms: result.duration_ms,
        provider_turns: result.num_turns,
    }
}

fn emit_stream_event(event: &StreamEvent, sink: &dyn EventSink) {
    if let Some(PartialMessageEvent::BlockDelta {
        delta: BlockDelta::Text(text),
        ..
    }) = event.partial_message()
    {
        sink.emit(ProviderEvent::OutputDelta { text });
    }
}

fn map_error(error: claude_wrapper::Error) -> ProviderError {
    let kind = match error {
        claude_wrapper::Error::Auth { .. } => FailureKind::Authentication,
        claude_wrapper::Error::Timeout { .. } => FailureKind::Timeout,
        claude_wrapper::Error::BudgetExceeded { .. } => FailureKind::Budget,
        claude_wrapper::Error::MaxTurnsExceeded { .. } => FailureKind::MaxTurns,
        claude_wrapper::Error::MaxBudgetExceeded { .. } => FailureKind::MaxCost,
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

    const TEST_MCP_SERVER: &str = "roba";
    const TEST_GIT_TOOL: &str = "mcp__roba__git.snapshot";
    const TEST_SELF_TOOL: &str = "mcp__roba__self";
    const TEST_BOOTSTRAP: &str = "minimal Roba bootstrap";

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
    fn fake_claude(temp: &tempfile::TempDir) -> (ClaudeProvider, PathBuf) {
        use std::os::unix::fs::PermissionsExt;

        let marker = temp.path().join("blocked.pid");
        let args_marker = temp.path().join("claude.args");
        let mcp_config_marker = temp.path().join("claude.mcp.json");
        let mcp_path_marker = temp.path().join("claude.mcp.path");
        let binary = temp.path().join("claude");
        let script = format!(
            r#"#!/bin/sh
prompt=
resuming=false
previous=
: > '{}'
for arg do
  printf '%s\n' "$arg" >> '{}'
  if [ "$previous" = --mcp-config ]; then
    cp "$arg" '{}'
    printf '%s' "$arg" > '{}'
  fi
  prompt=$arg
  if [ "$arg" = --resume ]; then
    resuming=true
  fi
  previous=$arg
done
case "$prompt" in
  block)
    printf '%s' "$$" > '{}'
    exec sleep 30
    ;;
  limit-turns)
    printf '{{"type":"result","subtype":"error_max_turns","session_id":"limit-session","total_cost_usd":1.25,"duration_ms":321,"num_turns":30,"is_error":true,"usage":{{"input_tokens":100,"output_tokens":20}},"errors":["Reached maximum number of turns (30)"]}}\n'
    exit 1
    ;;
  limit-budget)
    printf '{{"type":"result","subtype":"error_max_budget_usd","session_id":"budget-session","total_cost_usd":2.75,"duration_ms":654,"num_turns":12,"is_error":true,"usage":{{"input_tokens":50,"output_tokens":10}},"errors":["Reached maximum budget ($2.00)"]}}\n'
    exit 1
    ;;
  failed)
    printf 'provider exploded\n' >&2
    exit 9
    ;;
  unterminated)
    text=partial
    terminal=false
    settle_delay=true
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
if [ "$settle_delay" = true ]; then
  sleep 0.1
fi
if [ "$terminal" = true ]; then
  printf '{{"type":"result","subtype":"success","result":"%s","session_id":"session-1","total_cost_usd":0.02,"duration_ms":10,"num_turns":1,"is_error":false,"usage":{{"input_tokens":3,"output_tokens":2}}}}\n' "$text"
fi
"#,
            args_marker.display(),
            args_marker.display(),
            mcp_config_marker.display(),
            mcp_path_marker.display(),
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

    fn launch_context() -> ProviderLaunchContext {
        ProviderLaunchContext::default()
            .try_with_mcp_endpoint(
                crate::provider::ProviderMcpEndpoint::new(
                    TEST_MCP_SERVER,
                    "http://127.0.0.1:4123/mcp",
                    "secret-provider-token",
                )
                .unwrap()
                .try_with_tool_names(["self", "git.snapshot", "self"])
                .unwrap(),
            )
            .unwrap()
            .with_bootstrap_instruction(TEST_BOOTSTRAP)
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
    fn bounded_turn_suppresses_the_legacy_single_turn_notice() {
        let request = request(ProviderId::claude());
        let config = ClaudeProvider::config(&request).unwrap();

        assert!(config.no_agent_notice);
        assert!(crate::session::compose_append_system_prompt(&config).is_none());
    }

    #[test]
    fn explicit_context_is_appended_again_for_fresh_and_resumed_turns() {
        let mut fresh = request(ProviderId::claude());
        fresh.spec.agent.instructions = vec!["agent".to_string()];
        fresh.spec.context.project = vec!["project".to_string()];
        fresh.spec.context.run = vec!["run".to_string()];
        let mut resumed = fresh.clone();
        resumed.spec.execution.session = SessionSpec::Resume {
            session: SessionHandle {
                provider: ProviderId::claude(),
                id: "session-1".to_string(),
            },
        };

        for turn in [fresh, resumed] {
            let config = ClaudeProvider::config(&turn).unwrap();
            assert_eq!(
                config.append_system_prompt.as_deref(),
                Some("agent\n\nproject\n\nrun")
            );
            // Provider-native user/project/local settings, CLAUDE.md, skills,
            // hooks, MCP, and memory remain ambient on the current bounded
            // path. A future controlled/hermetic mode must opt out explicitly.
            assert_eq!(config.setting_sources, None);
            assert!(!config.strict_mcp_config);
            assert!(!config.exclude_dynamic_system_prompt_sections);
        }
    }

    #[test]
    fn operation_configuration_is_reapplied_to_fresh_and_resumed_turns() {
        let mut fresh = request(ProviderId::claude());
        fresh.spec.agent.model = Some("claude-operation-model".to_string());
        fresh.spec.agent.effort = Some(Effort::High);
        fresh.spec.execution.limits.max_turns = Some(7);
        fresh.spec.execution.limits.max_cost_usd = Some(2.5);
        fresh.spec.execution.limits.timeout_secs = Some(45);
        let mut resumed = fresh.clone();
        resumed.spec.execution.session = SessionSpec::Resume {
            session: SessionHandle {
                provider: ProviderId::claude(),
                id: "session-1".to_string(),
            },
        };

        for turn in [&fresh, &resumed] {
            let config = ClaudeProvider::config(turn).unwrap();
            assert_eq!(config.model.as_deref(), Some("claude-operation-model"));
            assert_eq!(config.effort, Some(map_effort(Effort::High)));
            assert_eq!(config.max_turns, Some(7));
            assert_eq!(config.max_budget_usd, Some(2.5));
            assert_eq!(config.timeout_secs, Some(45));
        }
    }

    #[test]
    fn launch_bootstrap_precedes_explicit_context_for_fresh_and_resumed_turns() {
        let mut fresh = request(ProviderId::claude());
        fresh.spec.agent.instructions = vec!["agent instruction".to_owned()];
        let mut resumed = fresh.clone();
        resumed.spec.execution.session = SessionSpec::Resume {
            session: SessionHandle {
                provider: ProviderId::claude(),
                id: "session-1".to_owned(),
            },
        };
        let launch =
            ProviderLaunchContext::default().with_bootstrap_instruction("minimal Roba bootstrap");

        for turn in [fresh, resumed] {
            let mut config = ClaudeProvider::config(&turn).unwrap();
            ClaudeProvider::apply_bootstrap(&mut config, &launch);
            assert_eq!(
                config.append_system_prompt.as_deref(),
                Some("minimal Roba bootstrap\n\nagent instruction")
            );
        }
        assert!(!format!("{launch:?}").contains("minimal Roba bootstrap"));
    }

    #[test]
    fn launch_context_builds_exact_private_config_and_tool_allowlist() {
        let context = launch_context();
        let mcp_config = ClaudeProvider::mcp_config(&context).unwrap().unwrap();
        let path = PathBuf::from(mcp_config.path());
        let value: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            value,
            json!({
                "mcpServers": {
                    "roba": {
                        "type": "http",
                        "url": "http://127.0.0.1:4123/mcp",
                        "headers": {
                            "Authorization": "Bearer secret-provider-token"
                        }
                    }
                }
            })
        );

        let fresh = request(ProviderId::claude());
        let mut resumed = request(ProviderId::claude());
        resumed.spec.execution.session = SessionSpec::Resume {
            session: SessionHandle {
                provider: ProviderId::claude(),
                id: "session-1".to_string(),
            },
        };
        for turn in [fresh, resumed] {
            let mut config = ClaudeProvider::config(&turn).unwrap();
            config.mcp_config.push(mcp_config.path().to_string());
            ClaudeProvider::allow_mcp_tools(&mut config, &context);
            ClaudeProvider::allow_mcp_tools(&mut config, &context);
            assert_eq!(config.allow_tools, [TEST_GIT_TOOL, TEST_SELF_TOOL]);

            let args = ClaudeProvider::bounded_command(&config).args();
            assert!(
                args.windows(2)
                    .any(|pair| { pair == ["--mcp-config", mcp_config.path()] })
            );
            let expected_tools = format!("Read,Glob,Grep,{TEST_GIT_TOOL},{TEST_SELF_TOOL}");
            assert!(
                args.windows(2)
                    .any(|pair| { pair[0] == "--allowed-tools" && pair[1] == expected_tools })
            );
            assert!(!args.iter().any(|arg| arg.contains("secret-provider-token")));
        }
        assert!(!format!("{context:?}").contains("secret-provider-token"));

        drop(mcp_config);
        assert!(!path.exists());
    }

    #[test]
    fn attached_endpoint_without_advertised_tools_adds_no_allowlist_entries() {
        let context = ProviderLaunchContext::default()
            .try_with_mcp_endpoint(
                crate::provider::ProviderMcpEndpoint::new(
                    TEST_MCP_SERVER,
                    "http://127.0.0.1:4123/mcp",
                    "secret-provider-token",
                )
                .unwrap(),
            )
            .unwrap();
        let mut config = ClaudeProvider::config(&request(ProviderId::claude())).unwrap();

        ClaudeProvider::allow_mcp_tools(&mut config, &context);

        assert!(config.allow_tools.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn temporary_mcp_config_is_owner_private() {
        use std::os::unix::fs::PermissionsExt;

        let mcp_config = ClaudeProvider::mcp_config(&launch_context())
            .unwrap()
            .unwrap();
        let mode = std::fs::metadata(mcp_config.path())
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o077, 0);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn fake_binary_receives_exact_mcp_attachment_and_cleanup() {
        let temp = tempfile::tempdir().unwrap();
        let (provider, _) = fake_claude(&temp);
        let events = RecordingSink::default();

        let outcome = provider
            .execute_with_launch_context(request(ProviderId::claude()), launch_context(), &events)
            .await
            .unwrap();

        assert_eq!(outcome.output, "opened");
        let args = std::fs::read_to_string(temp.path().join("claude.args")).unwrap();
        assert!(args.lines().any(|arg| arg == "--mcp-config"));
        assert!(args.lines().any(|arg| arg == "--allowed-tools"));
        assert!(args.contains(TEST_GIT_TOOL));
        assert!(args.contains(TEST_SELF_TOOL));
        assert!(args.contains(TEST_BOOTSTRAP));
        assert!(!args.contains("secret-provider-token"));
        let value: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(temp.path().join("claude.mcp.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            value["mcpServers"]["roba"]["url"],
            "http://127.0.0.1:4123/mcp"
        );
        assert_eq!(
            value["mcpServers"]["roba"]["headers"]["Authorization"],
            "Bearer secret-provider-token"
        );
        let original_path = std::fs::read_to_string(temp.path().join("claude.mcp.path")).unwrap();
        assert!(!PathBuf::from(original_path).exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn fake_binary_streams_open_and_resume_into_normalized_events() {
        let temp = tempfile::tempdir().unwrap();
        let (provider, _) = fake_claude(&temp);

        let fresh_events = RecordingSink::default();
        let fresh = provider
            .execute(request(ProviderId::claude()), &fresh_events)
            .await
            .unwrap();
        assert_eq!(fresh.output, "opened");
        assert_eq!(fresh.session.as_ref().unwrap().id, "session-1");
        assert_eq!(fresh.usage.as_ref().unwrap().input, Some(3));
        assert_eq!(fresh.usage.as_ref().unwrap().output, Some(2));
        assert_eq!(fresh.cost, Some(Cost::usd(0.02)));
        let fresh_events = fresh_events.events.into_inner().unwrap();
        assert!(fresh_events.contains(&ProviderEvent::OutputDelta {
            text: "opened".to_string(),
        }));
        assert!(fresh_events.iter().any(|event| matches!(
            event,
            ProviderEvent::Usage { usage }
                if usage.input == Some(3) && usage.output == Some(2)
        )));

        let mut resumed = request(ProviderId::claude());
        resumed.spec.execution.session = SessionSpec::Resume {
            session: SessionHandle {
                provider: ProviderId::claude(),
                id: "session-1".to_string(),
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

        let error = provider.execute(request, &events).await.unwrap_err();

        assert_eq!(error.kind, FailureKind::Provider);
        assert!(
            error.message.contains("without a result event"),
            "unexpected provider error: {}",
            error.message
        );
        let events = events.events.into_inner().unwrap();
        assert!(events.contains(&ProviderEvent::OutputDelta {
            text: "partial".to_string(),
        }));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn streamed_limit_terminal_precedes_generic_process_failure() {
        let temp = tempfile::tempdir().unwrap();
        let (provider, _) = fake_claude(&temp);

        for (prompt, expected_kind, session, turns, cost, reason) in [
            (
                "limit-turns",
                FailureKind::MaxTurns,
                "limit-session",
                30,
                1.25,
                "max-turns limit after 30 provider turns",
            ),
            (
                "limit-budget",
                FailureKind::MaxCost,
                "budget-session",
                12,
                2.75,
                "max-budget limit after 12 provider turns",
            ),
        ] {
            let request = RunSpec::suspended(AgentSpec::new(ProviderId::claude()))
                .with_prompt(Prompt::new(prompt).unwrap())
                .into_turn()
                .unwrap();
            let events = RecordingSink::default();
            let error = provider.execute(request, &events).await.unwrap_err();

            assert_eq!(error.kind, expected_kind);
            assert!(error.message.contains(reason), "{}", error.message);
            assert!(!error.message.contains("--output-format"));
            let details = error.details.as_deref().unwrap();
            assert_eq!(details.session.as_ref().unwrap().id, session);
            assert_eq!(details.provider_turns, Some(turns));
            assert_eq!(details.cost, Some(Cost::usd(cost)));
            assert!(details.duration_ms.is_some());
            assert!(details.usage.is_some());
            assert!(events.events.into_inner().unwrap().iter().any(|event| {
                matches!(event, ProviderEvent::Usage { usage } if usage.input.is_some())
            }));
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn unrelated_nonzero_stream_exit_remains_a_provider_failure() {
        let temp = tempfile::tempdir().unwrap();
        let (provider, _) = fake_claude(&temp);
        let request = RunSpec::suspended(AgentSpec::new(ProviderId::claude()))
            .with_prompt(Prompt::new("failed").unwrap())
            .into_turn()
            .unwrap();

        let error = provider
            .execute(request, &RecordingSink::default())
            .await
            .unwrap_err();

        assert_eq!(error.kind, FailureKind::Provider);
        assert!(error.message.contains("provider exploded"));
        assert!(error.details.is_none());
    }
}
