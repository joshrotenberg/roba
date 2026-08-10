//! Thin CLI adapter for the new bounded-run library API.

use anyhow::{Result, bail};
use tokio::io::BufReader;

use roba_core::{
    ClaudeProvider, CodexProvider, ConfigLayer, Effort, PermissionPolicy, Prompt, ProviderId, Roba,
    RobaConfig, RunOverrides, RunState, SessionHandle, SessionSpec,
};

use crate::cli::{EffortLevel, RunArgs, RunProvider};

pub async fn run(args: RunArgs) -> Result<()> {
    if args.prompt.is_none() && !args.repl && !args.mcp {
        bail!("a prompt-less run is suspended; add --repl or --mcp so it can be started");
    }

    let provider = match args.provider {
        RunProvider::Claude => ProviderId::claude(),
        RunProvider::Codex => ProviderId::codex(),
    };
    let permissions = if args.full_auto {
        PermissionPolicy::FullAuto
    } else if args.writable {
        PermissionPolicy::WorkspaceWrite
    } else {
        PermissionPolicy::ReadOnly
    };
    let config = RobaConfig {
        defaults: ConfigLayer {
            provider: Some(provider.clone()),
            model: args.model,
            effort: args.effort.map(map_effort),
            instructions: args.instructions,
            permissions: Some(permissions),
            max_turns: args.max_turns,
            max_cost_usd: args.max_cost_usd,
            timeout_secs: args.timeout,
            max_workers: args.max_workers,
            max_worker_depth: args.max_worker_depth,
            ..ConfigLayer::default()
        },
        ..RobaConfig::default()
    };
    let initial_prompt = args.prompt.map(Prompt::new).transpose()?;
    let session = args
        .resume
        .map(|id| SessionSpec::Resume {
            session: SessionHandle {
                provider: provider.clone(),
                id,
            },
        })
        .unwrap_or_default();
    let spec = config.resolve(
        None,
        RunOverrides {
            context: args.context,
            session,
            initial_prompt,
            ..RunOverrides::default()
        },
    )?;

    let mut roba = Roba::new();
    roba.register(ClaudeProvider)?;
    roba.register(CodexProvider)?;
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
    match terminal.state {
        RunState::Completed => {
            if let Some(outcome) = terminal.last_outcome {
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

fn map_effort(effort: EffortLevel) -> Effort {
    match effort {
        EffortLevel::Low => Effort::Low,
        EffortLevel::Medium => Effort::Medium,
        EffortLevel::High => Effort::High,
        EffortLevel::Xhigh => Effort::XHigh,
        EffortLevel::Max => Effort::Max,
    }
}
