//! Thin CLI adapter for the new bounded-run library API.

use anyhow::{Context, Result, bail};
use tokio::io::BufReader;

use roba_core::{
    ClaudeProvider, CodexProvider, CompletionPolicy, ConfigLayer, Effort, MissionPolicy,
    PermissionPolicy, Prompt, ProviderId, Roba, RobaConfig, RunOverrides, RunSpec, RunState,
    SessionHandle, SessionSpec,
};
use roba_process_github::{GitHubProcess, GitHubRepository};

use crate::VersionedResult;
use crate::cli::{EffortLevel, RunArgs, RunProvider};

pub async fn run(args: RunArgs) -> Result<()> {
    if args.prompt.is_none() && !args.repl && !args.mcp {
        bail!("a prompt-less run is suspended; add --repl or --mcp so it can be started");
    }

    let mut spec = resolve_spec(&args)?;

    let mut roba = Roba::new();
    roba.register(roba_mcp::WorkerMcpProvider::new(ClaudeProvider))?;
    roba.register(roba_mcp::WorkerMcpProvider::new(CodexProvider::default()))?;
    configure_github_process(&args, &mut spec, &mut roba)?;
    let run = roba.create_run(spec)?;
    if run.spec().initial_prompt.is_some() {
        run.begin().await?;
    }

    if args.mcp {
        return roba_mcp::serve_stdio(run.handle())
            .await
            .map_err(Into::into);
    }
    if args.repl {
        eprintln!("roba run REPL; /help lists commands");
        let reader = BufReader::new(tokio::io::stdin());
        return roba_repl::Repl::new(run.handle())
            .run(reader, tokio::io::stdout())
            .await
            .map_err(Into::into);
    }

    let terminal = run.handle().wait().await;
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&VersionedResult::new(&terminal))?
        );
    }
    match terminal.state {
        RunState::Completed => {
            if !args.json
                && let Some(outcome) = terminal.last_outcome
            {
                println!("{}", outcome.output);
            }
            Ok(())
        }
        RunState::Failed => bail!(
            "{}",
            terminal
                .failure
                .map(|failure| failure.message)
                .unwrap_or_else(|| "run failed without a reported reason".to_string())
        ),
        RunState::Cancelled => bail!("run was cancelled"),
        state => bail!("run ended wait in unexpected state {state:?}"),
    }
}

fn configure_github_process(args: &RunArgs, spec: &mut RunSpec, roba: &mut Roba) -> Result<()> {
    let Some(repository) = &args.github_repo else {
        return Ok(());
    };
    let process =
        GitHubProcess::new(GitHubRepository::new(repository)?).with_binary(&args.gh_binary);
    let mut grants = vec![GitHubProcess::read_grant()];
    if args.github_pr_write {
        grants.push(GitHubProcess::pull_request_write_grant());
    }
    if args.github_merge {
        grants.push(GitHubProcess::merge_grant());
    }
    spec.mission = MissionPolicy::new(
        [GitHubProcess::capability_id()],
        grants,
        CompletionPolicy::RootTerminal,
    )?;
    roba.register_capability(process)?;
    Ok(())
}

#[cfg(test)]
fn terminal_json(snapshot: &roba_core::RunSnapshot) -> serde_json::Value {
    serde_json::to_value(VersionedResult::new(snapshot)).unwrap()
}

fn resolve_spec(args: &RunArgs) -> Result<RunSpec> {
    let mut config = match &args.config {
        Some(path) => {
            let input = std::fs::read_to_string(path)
                .with_context(|| format!("reading run config {}", path.display()))?;
            RobaConfig::from_toml(&input)
                .with_context(|| format!("parsing run config {}", path.display()))?
        }
        None => RobaConfig::default(),
    };
    config
        .defaults
        .provider
        .get_or_insert_with(ProviderId::claude);

    let permissions = if args.full_auto {
        Some(PermissionPolicy::FullAuto)
    } else if args.writable {
        Some(PermissionPolicy::WorkspaceWrite)
    } else {
        None
    };
    let provider = args.provider.map(map_provider);
    let mut spec = config.resolve(
        args.agent.as_deref(),
        RunOverrides {
            policy: ConfigLayer {
                provider,
                model: args.model.clone(),
                effort: args.effort.map(map_effort),
                instructions: args.instructions.clone(),
                permissions,
                max_turns: args.max_turns,
                max_cost_usd: args.max_cost_usd,
                timeout_secs: args.timeout,
                max_workers: args.max_workers,
                max_worker_depth: args.max_worker_depth,
                ..ConfigLayer::default()
            },
            context: args.context.clone(),
            initial_prompt: args.prompt.clone().map(Prompt::new).transpose()?,
            ..RunOverrides::default()
        },
    )?;
    if let Some(id) = &args.resume {
        spec.execution.session = SessionSpec::Resume {
            session: SessionHandle {
                provider: spec.agent.provider.clone(),
                id: id.clone(),
            },
        };
    }
    Ok(spec)
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
            SubCommand::Run(args) => *args,
            other => panic!("expected run args, got {other:?}"),
        }
    }

    #[test]
    fn cli_resolution_preserves_config_policy_and_fences_resume_to_selected_provider() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("run.toml");
        std::fs::write(
            &path,
            r#"
[defaults]
provider = "claude"
permissions = "full_auto"

[agents.builder]
provider = "codex"
model = "configured"
"#,
        )
        .unwrap();
        let path = path.to_str().unwrap();

        let args = parse_run_args(&[
            "--config",
            path,
            "--agent",
            "builder",
            "--model",
            "overridden",
            "--resume",
            "thread-1",
            "hello",
        ]);
        let spec = resolve_spec(&args).unwrap();
        assert_eq!(spec.agent.provider, ProviderId::codex());
        assert_eq!(spec.agent.model.as_deref(), Some("overridden"));
        assert_eq!(spec.execution.permissions, PermissionPolicy::FullAuto);
        assert!(matches!(
            spec.execution.session,
            SessionSpec::Resume {
                session: SessionHandle { provider, ref id }
            } if provider == ProviderId::codex() && id == "thread-1"
        ));

        let args = parse_run_args(&[
            "--config",
            path,
            "--agent",
            "builder",
            "--writable",
            "hello",
        ]);
        let spec = resolve_spec(&args).unwrap();
        assert_eq!(spec.execution.permissions, PermissionPolicy::WorkspaceWrite);
    }

    #[test]
    fn terminal_json_preserves_each_terminal_snapshot_without_reshaping() {
        use roba_core::{
            Cost, FailureKind, ProviderId, RunFailure, RunFailureDetails, RunId, RunOutcome,
            SessionHandle,
        };

        let snapshot =
            |state: RunState, outcome: Option<RunOutcome>, failure: Option<RunFailure>| {
                roba_core::RunSnapshot {
                    id: serde_json::from_value::<RunId>(serde_json::json!(1)).unwrap(),
                    parent_id: None,
                    depth: 0,
                    state,
                    created_at_unix_ms: Some(10),
                    started_at_unix_ms: Some(20),
                    finished_at_unix_ms: Some(30),
                    elapsed_ms: Some(10),
                    turns_completed: u32::from(outcome.is_some()),
                    last_outcome: outcome,
                    failure,
                }
            };
        let completed = snapshot(
            RunState::Completed,
            Some(RunOutcome {
                output: "done".to_string(),
                session: None,
                usage: None,
                cost: None,
                duration_ms: Some(8),
                provider_turns: Some(1),
                structured_output: None,
            }),
            None,
        );
        let failed = snapshot(
            RunState::Failed,
            None,
            Some(RunFailure {
                kind: FailureKind::Limit,
                message: "turn limit reached".to_string(),
                details: RunFailureDetails {
                    session: Some(SessionHandle {
                        provider: ProviderId::claude(),
                        id: "limit-session".to_string(),
                    }),
                    usage: None,
                    cost: Some(Cost::usd(1.25)),
                    duration_ms: None,
                    provider_turns: Some(30),
                },
            }),
        );
        let cancelled = snapshot(RunState::Cancelled, None, None);

        for (snapshot, state) in [
            (completed, "completed"),
            (failed, "failed"),
            (cancelled, "cancelled"),
        ] {
            let json = terminal_json(&snapshot);
            assert_eq!(json["version"], roba_types::VERSION);
            assert_eq!(json["result"]["state"], state);
            assert_eq!(
                json["result"],
                serde_json::to_value(snapshot).unwrap(),
                "the adapter must wrap the public snapshot without reshaping it"
            );
        }
    }

    #[tokio::test]
    async fn github_process_flags_resolve_exact_capabilities_and_grants() {
        let args = parse_run_args(&[
            "--provider",
            "codex",
            "--github-repo",
            "Owner/Project",
            "--github-pr-write",
            "--gh-binary",
            "/test/gh",
            "work the backlog",
        ]);
        let mut spec = resolve_spec(&args).unwrap();
        let mut roba = Roba::new();
        roba.register(roba_mcp::WorkerMcpProvider::new(CodexProvider::default()))
            .unwrap();
        configure_github_process(&args, &mut spec, &mut roba).unwrap();

        assert_eq!(
            spec.mission.capabilities(),
            &std::collections::BTreeSet::from([GitHubProcess::capability_id()])
        );
        assert_eq!(
            spec.mission.grants(),
            &std::collections::BTreeSet::from([
                GitHubProcess::read_grant(),
                GitHubProcess::pull_request_write_grant(),
            ])
        );
        let run = roba.create_run(spec).unwrap();
        let mission = run.handle().mission().await;
        let actions = &mission.authority.process_capabilities[0].actions;
        assert!(
            actions
                .iter()
                .any(|action| action.id.as_str() == "pulls/create")
        );
        assert!(
            !actions
                .iter()
                .any(|action| action.id.as_str() == "pulls/merge")
        );
    }

    #[test]
    fn github_write_flags_require_an_explicit_repository() {
        for flag in ["--github-pr-write", "--github-merge", "--gh-binary"] {
            let mut args = vec![flag];
            if flag == "--gh-binary" {
                args.push("/test/gh");
            }
            args.push("work");
            assert!(
                Cli::try_parse_from(
                    std::iter::once("roba")
                        .chain(std::iter::once("run"))
                        .chain(args)
                )
                .is_err()
            );
        }
    }
}
