//! Provider-neutral CLI host for the Roba core and MCP harness.

use anyhow::{Context, Result};

pub mod bounded;
pub mod cli;
pub mod error;
pub mod init;
pub mod proposal;
pub mod serve;
pub mod startup_config;
pub mod survey;

pub(crate) use roba_types::VersionedResult;

use crate::cli::{Cli, ConfigCmd, SubCommand};

/// Dispatch one parsed command through the provider-neutral host.
pub async fn dispatch(cli: Cli) -> Result<()> {
    if let Some(path) = cli.cwd.as_deref() {
        std::env::set_current_dir(path)
            .with_context(|| format!("--cwd: cannot change directory to {}", path.display()))?;
    }

    match cli.command {
        Some(SubCommand::Init(args)) => init::run(args),
        Some(SubCommand::Run(args)) => bounded::run(args).await,
        Some(SubCommand::Serve(args)) => serve::run(args).await,
        Some(SubCommand::Config {
            cmd: ConfigCmd::Effective(args),
        }) => startup_config::run_effective(args),
        Some(SubCommand::Config {
            cmd: ConfigCmd::Survey(args),
        }) => survey::run(args).await,
        Some(SubCommand::Config {
            cmd: ConfigCmd::Propose(args),
        }) => proposal::run(args).await,
        Some(SubCommand::Completions { shell }) => {
            use clap::CommandFactory;
            let mut command = Cli::command();
            clap_complete::generate(shell, &mut command, "roba", &mut std::io::stdout());
            Ok(())
        }
        None => anyhow::bail!("a command is required; run `roba --help`"),
    }
}

pub use roba_types::EXIT_UNUSABLE_RESULT;

/// Typed signal for a terminal provider outcome with no usable answer.
#[derive(Debug)]
pub struct UnusableResultError {
    code: i32,
    note: &'static str,
}

impl UnusableResultError {
    pub fn code(&self) -> i32 {
        self.code
    }

    pub fn note(&self) -> &'static str {
        self.note
    }
}

impl std::fmt::Display for UnusableResultError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.note)
    }
}

impl std::error::Error for UnusableResultError {}

pub(crate) fn unusable_result_error(code: i32, note: &'static str) -> anyhow::Error {
    anyhow::Error::new(UnusableResultError { code, note })
}

/// Map a typed terminal failure to Roba's stable process exit contract.
pub fn classify_exit_code(error: &anyhow::Error) -> i32 {
    use roba_types::{
        EXIT_AUTH, EXIT_BUDGET, EXIT_FAILURE, EXIT_MAX_BUDGET, EXIT_MAX_TURNS, EXIT_TIMEOUT,
    };

    if let Some(unusable) = error.downcast_ref::<UnusableResultError>() {
        return unusable.code();
    }
    let Some(run_error) = error.downcast_ref::<bounded::BoundedRunError>() else {
        return EXIT_FAILURE;
    };
    match run_error.failure().kind {
        roba_core::FailureKind::Authentication => EXIT_AUTH,
        roba_core::FailureKind::Timeout => EXIT_TIMEOUT,
        roba_core::FailureKind::Budget => EXIT_BUDGET,
        roba_core::FailureKind::MaxTurns => EXIT_MAX_TURNS,
        roba_core::FailureKind::MaxCost => EXIT_MAX_BUDGET,
        roba_core::FailureKind::Limit
        | roba_core::FailureKind::Cancelled
        | roba_core::FailureKind::Unsupported
        | roba_core::FailureKind::Provider => EXIT_FAILURE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use roba_core::{FailureKind, RunFailure, RunFailureDetails};

    #[test]
    fn provider_neutral_failures_preserve_the_exit_contract() {
        let cases = [
            (FailureKind::Authentication, roba_types::EXIT_AUTH),
            (FailureKind::Timeout, roba_types::EXIT_TIMEOUT),
            (FailureKind::Budget, roba_types::EXIT_BUDGET),
            (FailureKind::MaxTurns, roba_types::EXIT_MAX_TURNS),
            (FailureKind::MaxCost, roba_types::EXIT_MAX_BUDGET),
            (FailureKind::Provider, roba_types::EXIT_FAILURE),
            (FailureKind::Cancelled, roba_types::EXIT_FAILURE),
            (FailureKind::Unsupported, roba_types::EXIT_FAILURE),
            (FailureKind::Limit, roba_types::EXIT_FAILURE),
        ];

        for (kind, expected) in cases {
            let error = anyhow::Error::new(bounded::BoundedRunError::new(RunFailure {
                kind,
                message: "failure".to_string(),
                details: RunFailureDetails::default(),
            }));
            assert_eq!(classify_exit_code(&error), expected, "{kind:?}");
        }
    }

    #[test]
    fn unknown_and_unusable_errors_keep_their_distinct_codes() {
        assert_eq!(classify_exit_code(&anyhow::anyhow!("boom")), 1);
        let unusable = unusable_result_error(EXIT_UNUSABLE_RESULT, "empty");
        assert_eq!(classify_exit_code(&unusable), EXIT_UNUSABLE_RESULT);
    }
}
