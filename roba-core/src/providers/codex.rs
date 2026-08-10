//! OpenAI Codex provider adapter.

use codex_wrapper::types::QueryResult;
use codex_wrapper::{ApprovalPolicyConfig, Codex, ExecCommand, ExecResumeCommand, SandboxMode};

use crate::provider::{EventSink, Provider, ProviderCapabilities, ProviderError, ProviderFuture};
use crate::run::{
    Effort, FailureKind, PermissionPolicy, ProviderId, RunEvent, RunOutcome, SessionHandle,
    SessionSpec, TokenUsage, TurnRequest,
};

/// Codex CLI implementation of Roba's provider-neutral turn boundary.
#[derive(Debug, Clone, Copy, Default)]
pub struct CodexProvider;

impl CodexProvider {
    fn client(request: &TurnRequest) -> Result<Codex, ProviderError> {
        let mut builder = Codex::builder();
        if let Some(seconds) = request.spec.execution.limits.timeout_secs {
            builder = builder.timeout_secs(seconds);
        }
        builder.build().map_err(map_error)
    }

    fn fresh_command(request: &TurnRequest) -> ExecCommand {
        let mut command = ExecCommand::new(render_prompt(request)).prompt_via_stdin();
        if let Some(model) = &request.spec.agent.model {
            command = command.model(model.clone());
        }
        if let Some(effort) = request.spec.agent.effort {
            command = command.config(reasoning_effort(effort));
        }
        match request.spec.execution.permissions {
            PermissionPolicy::ReadOnly => command
                .sandbox(SandboxMode::ReadOnly)
                .approval_policy(ApprovalPolicyConfig::Never),
            PermissionPolicy::WorkspaceWrite => command
                .sandbox(SandboxMode::WorkspaceWrite)
                .approval_policy(ApprovalPolicyConfig::OnRequest),
            PermissionPolicy::FullAuto => command
                .sandbox(SandboxMode::WorkspaceWrite)
                .approval_policy(ApprovalPolicyConfig::Never),
        }
    }

    fn resume_command(request: &TurnRequest, session_id: &str) -> ExecResumeCommand {
        let mut command = ExecResumeCommand::new()
            .session_id(session_id)
            .prompt(render_prompt(request));
        if let Some(model) = &request.spec.agent.model {
            command = command.model(model.clone());
        }
        if let Some(effort) = request.spec.agent.effort {
            command = command.config(reasoning_effort(effort));
        }
        match request.spec.execution.permissions {
            PermissionPolicy::ReadOnly => command
                .config("sandbox_mode=\"read-only\"")
                .approval_policy(ApprovalPolicyConfig::Never),
            PermissionPolicy::WorkspaceWrite => command
                .config("sandbox_mode=\"workspace-write\"")
                .approval_policy(ApprovalPolicyConfig::OnRequest),
            PermissionPolicy::FullAuto => command
                .config("sandbox_mode=\"workspace-write\"")
                .approval_policy(ApprovalPolicyConfig::Never),
        }
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
            usage: result.usage.map(|usage| TokenUsage {
                input: usage.input_tokens,
                cached_input: usage.cached_input_tokens,
                cache_write_input: usage.cache_write_input_tokens,
                output: usage.output_tokens,
                reasoning_output: usage.reasoning_output_tokens,
                total: usage.total_tokens,
            }),
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
            // The wrapper supports streaming, but this first adapter slice
            // uses its typed terminal result. Streaming moves behind the
            // common event sink with the run lifecycle.
            streaming: false,
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
        events: &'a dyn EventSink,
    ) -> ProviderFuture<'a> {
        Box::pin(async move {
            self.validate(&request)?;
            let codex = Self::client(&request)?;
            events.emit(RunEvent::TurnStarted {
                provider: ProviderId::codex(),
            });
            let result = match &request.spec.execution.session {
                SessionSpec::Fresh => Self::fresh_command(&request)
                    .execute_json(&codex)
                    .await
                    .map_err(map_error)?,
                SessionSpec::Resume { session } => Self::resume_command(&request, &session.id)
                    .execute_json(&codex)
                    .await
                    .map_err(map_error)?,
            };
            let outcome = Self::normalize(result);
            events.emit(RunEvent::TurnCompleted {
                outcome: outcome.clone(),
            });
            Ok(outcome)
        })
    }
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
    use codex_wrapper::types::{JsonLineEvent, TokenUsage as CodexUsage};

    use super::*;
    use crate::run::{AgentSpec, Prompt, RunSpec};

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
        let provider = CodexProvider;
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
}
