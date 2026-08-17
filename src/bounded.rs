//! Thin CLI adapter for the provider-neutral finite-run API.

use std::fmt;

use anyhow::{Result, bail};
use roba_core::{
    AgentSpec, ClaudeProvider, CodexProvider, ContextSpec, Effort, ExecutionSpec, FailureKind,
    LimitSpec, PermissionPolicy, Prompt, ProviderId, Roba, RunFailure, RunFailureDetails,
    RunOutcome, RunSpec, RunState, SessionHandle, SessionSpec, ToolPolicy,
};

use crate::VersionedResult;
use crate::cli::{EffortLevel, RunArgs, RunProvider};

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
    let spec = resolve_spec(&args)?;

    let mut roba = Roba::new();
    roba.register(ClaudeProvider)?;
    roba.register(CodexProvider::default())?;
    let run = roba.create_run(spec)?;
    run.begin().await?;

    let terminal = run.handle().wait().await;
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

#[cfg(test)]
fn terminal_json(snapshot: &roba_core::RunSnapshot) -> serde_json::Value {
    serde_json::to_value(VersionedResult::new(snapshot)).unwrap()
}

fn resolve_spec(args: &RunArgs) -> Result<RunSpec> {
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
        initial_prompt: Some(Prompt::new(args.prompt.clone())?),
    })
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
            "--resume",
            "thread-1",
            "hello",
        ]);
        let spec = resolve_spec(&args).unwrap();
        assert_eq!(spec.agent.provider, ProviderId::codex());
        assert_eq!(spec.agent.model.as_deref(), Some("configured"));
        assert_eq!(spec.agent.effort, Some(Effort::XHigh));
        assert_eq!(spec.agent.instructions, ["be exact"]);
        assert_eq!(spec.context.run, ["the tests are authoritative"]);
        assert_eq!(spec.execution.permissions, PermissionPolicy::WorkspaceWrite);
        assert_eq!(spec.execution.limits.timeout_secs, Some(30));
        assert!(matches!(
            spec.execution.session,
            SessionSpec::Resume {
                session: SessionHandle { provider, ref id }
            } if provider == ProviderId::codex() && id == "thread-1"
        ));
        assert_eq!(spec.initial_prompt.unwrap().as_str(), "hello");
    }

    #[test]
    fn defaults_are_read_only_fresh_claude() {
        let spec = resolve_spec(&parse_run_args(&["hello"])).unwrap();
        assert_eq!(spec.agent.provider, ProviderId::claude());
        assert_eq!(spec.execution.permissions, PermissionPolicy::ReadOnly);
        assert_eq!(spec.execution.session, SessionSpec::Fresh);
    }

    #[test]
    fn terminal_json_wraps_the_public_snapshot_without_reshaping() {
        use roba_core::{Cost, RunOutcome};

        let completed = roba_core::RunSnapshot {
            state: RunState::Completed,
            created_at_unix_ms: Some(10),
            started_at_unix_ms: Some(20),
            finished_at_unix_ms: Some(30),
            elapsed_ms: Some(10),
            turns_completed: 1,
            last_outcome: Some(RunOutcome {
                output: "done".to_string(),
                session: None,
                usage: None,
                cost: Some(Cost::usd(1.25)),
                duration_ms: Some(8),
                provider_turns: Some(1),
                structured_output: None,
            }),
            failure: None,
        };

        let json = terminal_json(&completed);
        assert_eq!(json["version"], roba_types::VERSION);
        assert_eq!(json["result"]["state"], "completed");
        assert_eq!(
            json["result"],
            serde_json::to_value(completed).unwrap(),
            "the adapter must wrap the public snapshot without reshaping it"
        );
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
