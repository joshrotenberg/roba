//! Claude Code provider adapter.

use anyhow::Result;
use claude_wrapper::types::QueryResult;
use claude_wrapper::{
    Claude, Effort as ClaudeEffort, McpConfigBuilder, QueryCommand, TempMcpConfig,
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
#[derive(Debug, Clone, Copy, Default)]
pub struct ClaudeProvider;

impl ClaudeProvider {
    /// Execute the pre-pivot Claude config without changing its behavior.
    /// This is the compatibility seam used by [`crate::engine::run`].
    pub async fn execute_legacy(&self, config: &Config) -> Result<QueryResult> {
        let mut builder = Claude::builder();
        if let Some(secs) = config.timeout_secs
            && secs > 0
        {
            builder = builder.timeout_secs(secs);
        }
        let claude = builder.build()?;
        let mut result = engine::execute(config, &claude).await?;
        if config.json_schema.is_some() {
            engine::surface_structured_output(&mut result);
        }
        Ok(result)
    }

    async fn execute_bounded(&self, config: &Config) -> Result<QueryResult> {
        let mut builder = Claude::builder();
        if let Some(secs) = config.timeout_secs
            && secs > 0
        {
            builder = builder.timeout_secs(secs);
        }
        let claude = builder.build()?;
        let mut result = Self::bounded_command(config).execute_json(&claude).await?;
        if config.json_schema.is_some() {
            engine::surface_structured_output(&mut result);
        }
        Ok(result)
    }

    fn bounded_command(config: &Config) -> QueryCommand {
        // Roba owns bounded child-run creation. Claude's native Agent tool is
        // not represented in the run tree and would bypass worker count,
        // depth, cancellation, and event observation.
        engine::query_command(config).disallowed_tool("Agent")
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

    fn allow_internal_mcp_tools(config: &mut Config, context: &ProviderContext) {
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
        }
    }

    /// Normalize Claude's result without converting missing telemetry to zero.
    pub fn normalize(result: QueryResult) -> RunOutcome {
        let structured_output = result.extra.get("structured_output").cloned();
        // claude-wrapper 0.13.0 preserves newer usage payloads in `extra`.
        // Reading the observed value here keeps this adapter compatible with
        // that stable wrapper release without pretending absent fields are 0.
        let usage = result.extra.get("usage").map(normalize_usage);
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

fn normalize_usage(value: &serde_json::Value) -> TokenUsage {
    let field = |names: &[&str]| {
        names
            .iter()
            .find_map(|name| value.get(name).and_then(serde_json::Value::as_u64))
    };
    TokenUsage {
        input: field(&["input_tokens"]),
        cached_input: field(&["cached_input_tokens", "cache_read_input_tokens"]),
        cache_write_input: field(&["cache_write_input_tokens", "cache_creation_input_tokens"]),
        output: field(&["output_tokens"]),
        reasoning_output: field(&["reasoning_output_tokens"]),
        total: field(&["total_tokens"]),
    }
}

impl Provider for ClaudeProvider {
    fn id(&self) -> ProviderId {
        ProviderId::claude()
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            resume: true,
            // The compatibility adapter is non-streaming. The existing CLI
            // streaming path moves behind this boundary in a later slice.
            streaming: false,
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
            Self::allow_internal_mcp_tools(&mut config, &context);
            let result = self.execute_bounded(&config).await.map_err(|error| {
                ProviderError::new(FailureKind::Provider, format!("Claude failed: {error:#}"))
            })?;
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
    use claude_wrapper::ClaudeCommand;
    use serde_json::json;

    use super::*;
    use crate::run::{AgentSpec, Prompt, RunSpec};

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
        let provider = ClaudeProvider;
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
        ClaudeProvider::allow_internal_mcp_tools(&mut request_config, &context);

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
        let args = ClaudeProvider::bounded_command(&request_config).args();
        assert!(args.iter().any(|arg| arg == "--disallowed-tools"));
        assert!(args.iter().any(|arg| arg == "Agent"));
        assert!(!format!("{context:?}").contains("secret-worker-token"));
    }
}
