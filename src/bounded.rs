//! Thin CLI adapter for the provider-neutral finite-run API.

use std::fmt;

use anyhow::{Result, bail};
use roba_core::{
    AgentSpec, ClaudeProvider, CodexProvider, ContextSpec, Effort, ExecutionSpec, FailureKind,
    LimitSpec, PermissionPolicy, Prompt, ProviderId, Roba, RunFailure, RunFailureDetails,
    RunOutcome, RunSnapshot, RunSpec, RunState, SessionHandle, SessionSpec, TokenUsage, ToolPolicy,
};
use roba_mcp::{AgentInstance, AgentTurnResult, TurnInput, call_turn, connect_in_process};

use crate::VersionedResult;
use crate::cli::{AgentArgs, EffortLevel, RunArgs, RunProvider};

/// A terminal provider-neutral run failure retained for exit-code and JSON
/// classification by the binary boundary.
#[derive(Debug, Clone)]
pub struct BoundedRunError {
    failure: RunFailure,
}

impl BoundedRunError {
    pub(crate) fn new(failure: RunFailure) -> Self {
        Self { failure }
    }

    /// Inspect the normalized terminal failure.
    pub fn failure(&self) -> &RunFailure {
        &self.failure
    }
}

impl fmt::Display for BoundedRunError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.failure.message)
    }
}

impl std::error::Error for BoundedRunError {}

pub async fn run(args: RunArgs) -> Result<()> {
    let resolved = resolve_spec(&args)?;

    let agent = build_agent_from_template(resolved.template)?;
    let client = connect_in_process(agent).await?;
    let turn = call_turn(
        &client,
        TurnInput {
            text: resolved.prompt,
        },
    )
    .await;
    let shutdown = client.shutdown().await;
    let turn = turn?;
    shutdown?;
    let terminal = terminal_snapshot(turn)?;

    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&VersionedResult::new(&terminal))?
        );
    }

    match terminal.state {
        RunState::Completed => {
            let outcome = terminal
                .last_outcome
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("run completed without an outcome"))?;
            if !outcome_is_usable(outcome) {
                return Err(crate::unusable_result_error(
                    roba_types::EXIT_UNUSABLE_RESULT,
                    "provider returned an empty result",
                ));
            }
            if !args.json {
                println!("{}", outcome.output);
            }
            Ok(())
        }
        RunState::Failed => Err(anyhow::Error::new(BoundedRunError::new(
            terminal.failure.unwrap_or_else(|| RunFailure {
                kind: FailureKind::Provider,
                message: "run failed without a reported reason".to_string(),
                details: RunFailureDetails::default(),
            }),
        ))),
        RunState::Cancelled => Err(anyhow::Error::new(BoundedRunError::new(RunFailure {
            kind: FailureKind::Cancelled,
            message: "run was cancelled".to_string(),
            details: RunFailureDetails::default(),
        }))),
        state => bail!("run ended wait in unexpected state {state:?}"),
    }
}

fn outcome_is_usable(outcome: &RunOutcome) -> bool {
    !outcome.output.trim().is_empty()
        || outcome
            .structured_output
            .as_ref()
            .is_some_and(|value| !value.is_null())
}

fn terminal_snapshot(result: AgentTurnResult) -> Result<RunSnapshot> {
    match result {
        AgentTurnResult::Completed { run, .. } => {
            let run = *run;
            Ok(snapshot_from_parts(
                RunState::Completed,
                run.metadata,
                Some(map_outcome(run.outcome)?),
                None,
            ))
        }
        AgentTurnResult::Failed { run, .. } => {
            let run = *run;
            Ok(snapshot_from_parts(
                RunState::Failed,
                run.metadata,
                run.last_outcome.map(map_outcome).transpose()?,
                Some(map_failure(run.failure)?),
            ))
        }
        AgentTurnResult::Cancelled { run, .. } => {
            let run = *run;
            Ok(snapshot_from_parts(
                RunState::Cancelled,
                run.metadata,
                run.last_outcome.map(map_outcome).transpose()?,
                None,
            ))
        }
        AgentTurnResult::Refused { refusal } => bail!(refusal.message),
    }
}

fn snapshot_from_parts(
    state: RunState,
    metadata: roba_mcp::TurnMetadata,
    last_outcome: Option<RunOutcome>,
    failure: Option<RunFailure>,
) -> RunSnapshot {
    RunSnapshot {
        state,
        created_at_unix_ms: metadata.created_at_unix_ms,
        started_at_unix_ms: metadata.started_at_unix_ms,
        finished_at_unix_ms: metadata.finished_at_unix_ms,
        elapsed_ms: metadata.elapsed_ms,
        turns_completed: metadata.turns_completed,
        last_outcome,
        failure,
    }
}

fn map_outcome(outcome: roba_mcp::TurnOutcome) -> Result<RunOutcome> {
    Ok(RunOutcome {
        output: outcome.output,
        session: outcome.session.map(map_session).transpose()?,
        usage: outcome.usage.map(map_usage),
        cost: outcome.cost.map(map_cost),
        duration_ms: outcome.duration_ms,
        provider_turns: outcome.provider_turns,
        structured_output: outcome.structured_output,
    })
}

fn map_failure(failure: roba_mcp::TurnFailure) -> Result<RunFailure> {
    Ok(RunFailure {
        kind: map_failure_kind(failure.kind),
        message: failure.message,
        details: map_failure_details(failure.details)?,
    })
}

fn map_failure_details(details: roba_mcp::FailureDetails) -> Result<RunFailureDetails> {
    Ok(RunFailureDetails {
        session: details.session.map(map_session).transpose()?,
        usage: details.usage.map(map_usage),
        cost: details.cost.map(map_cost),
        duration_ms: details.duration_ms,
        provider_turns: details.provider_turns,
    })
}

fn map_session(session: roba_mcp::SessionHandle) -> Result<SessionHandle> {
    Ok(SessionHandle {
        provider: ProviderId::new(session.provider)?,
        id: session.id,
    })
}

fn map_usage(usage: roba_mcp::TokenUsage) -> TokenUsage {
    TokenUsage {
        input: usage.input,
        cached_input: usage.cached_input,
        cache_write_input: usage.cache_write_input,
        output: usage.output,
        reasoning_output: usage.reasoning_output,
        total: usage.total,
    }
}

fn map_cost(cost: roba_mcp::Cost) -> roba_core::Cost {
    roba_core::Cost {
        currency: cost.currency,
        amount: cost.amount,
    }
}

fn map_failure_kind(kind: roba_mcp::FailureKind) -> FailureKind {
    match kind {
        roba_mcp::FailureKind::Authentication => FailureKind::Authentication,
        roba_mcp::FailureKind::Timeout => FailureKind::Timeout,
        roba_mcp::FailureKind::Budget => FailureKind::Budget,
        roba_mcp::FailureKind::MaxTurns => FailureKind::MaxTurns,
        roba_mcp::FailureKind::MaxCost => FailureKind::MaxCost,
        roba_mcp::FailureKind::Limit => FailureKind::Limit,
        roba_mcp::FailureKind::Cancelled => FailureKind::Cancelled,
        roba_mcp::FailureKind::Unsupported => FailureKind::Unsupported,
        roba_mcp::FailureKind::Provider => FailureKind::Provider,
    }
}

#[cfg(test)]
fn terminal_json(snapshot: &roba_core::RunSnapshot) -> serde_json::Value {
    serde_json::to_value(VersionedResult::new(snapshot)).unwrap()
}

#[derive(Debug)]
struct ResolvedRun {
    template: RunSpec,
    prompt: String,
}

fn resolve_spec(args: &RunArgs) -> Result<ResolvedRun> {
    Ok(ResolvedRun {
        template: resolve_template(&args.agent)?,
        prompt: Prompt::new(args.prompt.clone())?.into_inner(),
    })
}

/// Resolve the fixed suspended template shared by one-shot and hot agents.
pub(crate) fn resolve_template(args: &AgentArgs) -> Result<RunSpec> {
    let provider = args
        .provider
        .map(map_provider)
        .unwrap_or_else(ProviderId::claude);
    let permissions = if args.full_auto {
        PermissionPolicy::FullAuto
    } else if args.writable {
        PermissionPolicy::WorkspaceWrite
    } else {
        PermissionPolicy::ReadOnly
    };
    let session = match &args.resume {
        Some(id) => SessionSpec::Resume {
            session: SessionHandle {
                provider: provider.clone(),
                id: id.clone(),
            },
        },
        None => SessionSpec::Fresh,
    };
    let mut agent = AgentSpec::new(provider);
    agent.model.clone_from(&args.model);
    agent.effort = args.effort.map(map_effort);
    agent.instructions.clone_from(&args.instructions);

    Ok(RunSpec {
        agent,
        context: ContextSpec {
            project: Vec::new(),
            run: args.context.clone(),
        },
        execution: ExecutionSpec {
            permissions,
            tools: ToolPolicy::default(),
            limits: LimitSpec {
                max_turns: args.max_turns,
                max_cost_usd: args.max_cost_usd,
                timeout_secs: args.timeout,
            },
            session,
        },
        initial_prompt: None,
    })
}

/// Construct one configured hot agent with both built-in providers available.
pub(crate) fn build_agent(args: &AgentArgs) -> Result<AgentInstance> {
    build_agent_from_template(resolve_template(args)?)
}

fn build_agent_from_template(template: RunSpec) -> Result<AgentInstance> {
    let mut roba = Roba::new();
    roba.register(ClaudeProvider)?;
    roba.register(CodexProvider::default())?;
    Ok(AgentInstance::new(roba, template)?)
}

fn map_provider(provider: RunProvider) -> ProviderId {
    match provider {
        RunProvider::Claude => ProviderId::claude(),
        RunProvider::Codex => ProviderId::codex(),
    }
}

fn map_effort(effort: EffortLevel) -> Effort {
    match effort {
        EffortLevel::Low => Effort::Low,
        EffortLevel::Medium => Effort::Medium,
        EffortLevel::High => Effort::High,
        EffortLevel::Xhigh => Effort::XHigh,
        EffortLevel::Max => Effort::Max,
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;
    use crate::cli::{Cli, SubCommand};

    fn parse_run_args(args: &[&str]) -> RunArgs {
        let cli = Cli::try_parse_from(
            std::iter::once("roba")
                .chain(std::iter::once("run"))
                .chain(args.iter().copied()),
        )
        .unwrap();
        match cli.command.unwrap() {
            SubCommand::Run(args) => args,
            other => panic!("expected run args, got {other:?}"),
        }
    }

    fn parse_serve_agent(args: &[&str]) -> AgentArgs {
        let cli = Cli::try_parse_from(
            std::iter::once("roba")
                .chain(std::iter::once("serve"))
                .chain(args.iter().copied()),
        )
        .unwrap();
        match cli.command.unwrap() {
            SubCommand::Serve(args) => args.agent,
            other => panic!("expected serve args, got {other:?}"),
        }
    }

    #[test]
    fn run_and_serve_resolve_the_same_suspended_agent_template() {
        let shared = [
            "--provider",
            "codex",
            "--model",
            "configured",
            "--effort",
            "xhigh",
            "--instruction",
            "be exact",
            "--context",
            "the tests are authoritative",
            "--writable",
            "--timeout",
            "30",
            "--max-turns",
            "12",
            "--max-cost-usd",
            "1.5",
            "--resume",
            "thread-1",
        ];
        let run = parse_run_args(&[&shared[..], &["hello"]].concat());
        let serve = parse_serve_agent(&shared);

        assert_eq!(
            resolve_spec(&run).unwrap().template,
            resolve_template(&serve).unwrap()
        );
        assert!(resolve_template(&serve).unwrap().initial_prompt.is_none());
    }

    #[test]
    fn cli_flags_resolve_directly_and_resume_is_fenced_to_the_provider() {
        let args = parse_run_args(&[
            "--provider",
            "codex",
            "--model",
            "configured",
            "--effort",
            "xhigh",
            "--instruction",
            "be exact",
            "--context",
            "the tests are authoritative",
            "--writable",
            "--timeout",
            "30",
            "--max-turns",
            "12",
            "--max-cost-usd",
            "1.5",
            "--resume",
            "thread-1",
            "hello",
        ]);
        let resolved = resolve_spec(&args).unwrap();
        assert_eq!(resolved.prompt, "hello");
        let spec = resolved.template;
        assert_eq!(spec.agent.provider, ProviderId::codex());
        assert_eq!(spec.agent.model.as_deref(), Some("configured"));
        assert_eq!(spec.agent.effort, Some(Effort::XHigh));
        assert_eq!(spec.agent.instructions, ["be exact"]);
        assert_eq!(spec.context.run, ["the tests are authoritative"]);
        assert_eq!(spec.execution.permissions, PermissionPolicy::WorkspaceWrite);
        assert_eq!(spec.execution.limits.timeout_secs, Some(30));
        assert_eq!(spec.execution.limits.max_turns, Some(12));
        assert_eq!(spec.execution.limits.max_cost_usd, Some(1.5));
        assert!(matches!(
            &spec.execution.session,
            SessionSpec::Resume { session }
                if session.provider == ProviderId::codex() && session.id == "thread-1"
        ));
        assert!(spec.initial_prompt.is_none());
    }

    #[test]
    fn defaults_are_read_only_fresh_claude() {
        let resolved = resolve_spec(&parse_run_args(&["hello"])).unwrap();
        assert_eq!(resolved.prompt, "hello");
        let spec = resolved.template;
        assert_eq!(spec.agent.provider, ProviderId::claude());
        assert_eq!(spec.execution.permissions, PermissionPolicy::ReadOnly);
        assert_eq!(spec.execution.session, SessionSpec::Fresh);
        assert!(spec.initial_prompt.is_none());
    }

    #[test]
    fn full_auto_is_retained_on_the_suspended_template() {
        let resolved = resolve_spec(&parse_run_args(&["--full-auto", "hello"])).unwrap();
        assert_eq!(
            resolved.template.execution.permissions,
            PermissionPolicy::FullAuto
        );
        assert!(resolved.template.initial_prompt.is_none());
    }

    #[test]
    fn prompt_validation_still_happens_before_host_construction() {
        let error = resolve_spec(&parse_run_args(&[" \n"])).unwrap_err();
        assert_eq!(error.to_string(), "prompt must not be empty");
    }

    #[test]
    fn mcp_result_projects_to_the_existing_terminal_json_without_reshaping() {
        let result: AgentTurnResult = serde_json::from_value(serde_json::json!({
            "status": "completed",
            "operation_id": 1,
            "run": {
                "created_at_unix_ms": 10,
                "started_at_unix_ms": 20,
                "finished_at_unix_ms": 30,
                "elapsed_ms": 10,
                "turns_completed": 1,
                "outcome": {
                    "output": "done",
                    "session": { "provider": "claude", "id": "session-1" },
                    "usage": { "input": 3, "output": 2 },
                    "cost": { "currency": "USD", "amount": 1.25 },
                    "duration_ms": 8,
                    "provider_turns": 1
                }
            }
        }))
        .unwrap();
        let completed = terminal_snapshot(result).unwrap();

        let json = terminal_json(&completed);
        assert_eq!(json["version"], roba_types::VERSION);
        assert_eq!(json["result"]["state"], "completed");
        assert_eq!(json["result"], serde_json::to_value(completed).unwrap());
        assert_eq!(json["result"]["last_outcome"]["session"]["id"], "session-1");
        assert_eq!(json["result"]["last_outcome"]["usage"]["input"], 3);
        assert_eq!(json["result"]["last_outcome"]["cost"]["amount"], 1.25);
    }

    #[test]
    fn every_mcp_failure_kind_preserves_the_existing_exit_contract() {
        let cases = [
            (
                "authentication",
                FailureKind::Authentication,
                roba_types::EXIT_AUTH,
            ),
            ("timeout", FailureKind::Timeout, roba_types::EXIT_TIMEOUT),
            ("budget", FailureKind::Budget, roba_types::EXIT_BUDGET),
            (
                "max_turns",
                FailureKind::MaxTurns,
                roba_types::EXIT_MAX_TURNS,
            ),
            (
                "max_cost",
                FailureKind::MaxCost,
                roba_types::EXIT_MAX_BUDGET,
            ),
            ("limit", FailureKind::Limit, roba_types::EXIT_FAILURE),
            (
                "cancelled",
                FailureKind::Cancelled,
                roba_types::EXIT_FAILURE,
            ),
            (
                "unsupported",
                FailureKind::Unsupported,
                roba_types::EXIT_FAILURE,
            ),
            ("provider", FailureKind::Provider, roba_types::EXIT_FAILURE),
        ];

        for (wire_kind, expected_kind, expected_exit) in cases {
            let result: AgentTurnResult = serde_json::from_value(serde_json::json!({
                "status": "failed",
                "operation_id": 1,
                "run": {
                    "turns_completed": 0,
                    "failure": {
                        "kind": wire_kind,
                        "message": "typed failure",
                        "details": {}
                    }
                }
            }))
            .unwrap();
            let snapshot = terminal_snapshot(result).unwrap();
            let failure = snapshot.failure.unwrap();
            assert_eq!(failure.kind, expected_kind);
            let error = anyhow::Error::new(BoundedRunError::new(failure));
            assert_eq!(crate::classify_exit_code(&error), expected_exit);
        }
    }

    #[test]
    fn mcp_cancelled_result_projects_to_a_cancelled_snapshot() {
        let result: AgentTurnResult = serde_json::from_value(serde_json::json!({
            "status": "cancelled",
            "operation_id": 7,
            "run": {
                "created_at_unix_ms": 10,
                "finished_at_unix_ms": 20,
                "elapsed_ms": 10,
                "turns_completed": 0
            }
        }))
        .unwrap();
        let snapshot = terminal_snapshot(result).unwrap();
        assert_eq!(snapshot.state, RunState::Cancelled);
        assert!(snapshot.failure.is_none());
        assert!(snapshot.last_outcome.is_none());
    }

    #[test]
    fn outer_mcp_structured_content_does_not_make_an_empty_outcome_usable() {
        let result: AgentTurnResult = serde_json::from_value(serde_json::json!({
            "status": "completed",
            "operation_id": 1,
            "run": {
                "turns_completed": 1,
                "outcome": { "output": " \n" }
            }
        }))
        .unwrap();
        let snapshot = terminal_snapshot(result).unwrap();
        assert!(!outcome_is_usable(snapshot.last_outcome.as_ref().unwrap()));
    }

    #[test]
    fn terminal_error_preserves_typed_failure() {
        let error = BoundedRunError::new(RunFailure {
            kind: FailureKind::Authentication,
            message: "log in".to_string(),
            details: RunFailureDetails::default(),
        });
        assert_eq!(error.failure().kind, FailureKind::Authentication);
        assert_eq!(error.to_string(), "log in");
    }

    #[test]
    fn empty_text_needs_non_null_structured_output_to_be_usable() {
        let mut outcome = RunOutcome {
            output: " \n".to_string(),
            session: None,
            usage: None,
            cost: None,
            duration_ms: None,
            provider_turns: None,
            structured_output: None,
        };
        assert!(!outcome_is_usable(&outcome));
        outcome.structured_output = Some(serde_json::json!({"answer": 42}));
        assert!(outcome_is_usable(&outcome));
        outcome.structured_output = None;
        outcome.output = "answer".to_string();
        assert!(outcome_is_usable(&outcome));
    }
}
