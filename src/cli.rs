//! Command-first CLI for the provider-neutral Roba harness.

use clap::builder::styling::{AnsiColor, Styles};
use clap::{Args as ClapArgs, Parser, Subcommand};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

const STYLES: Styles = Styles::styled()
    .header(AnsiColor::Green.on_default().bold())
    .usage(AnsiColor::Green.on_default().bold())
    .literal(AnsiColor::Cyan.on_default())
    .placeholder(AnsiColor::BrightBlack.on_default());

const AFTER_HELP: &str = "\
Examples:
  roba run \"inspect this repository\"
  roba run --provider codex --writable --git \"fix the failing tests\"
  mcp-repl -- roba serve --provider codex --git
  roba config effective

Use `roba help <COMMAND>` for the complete command reference.";

const RUN_AFTER_HELP: &str = "\
Examples:
  roba run --provider codex \"inspect this repository\"
  roba run --writable --git \"fix the failing tests\"
  roba run --no-config \"ignore discovered startup config\"
  roba run --instruction \"work methodically\" --context \"issue #489\" \"propose a plan\"
  roba run --json \"summarize current risks\" | jq '.result'

Versioned startup config is discovered from the cwd to the Git root and from
~/.config/roba/roba.toml. CLI values override file values. Inspect the exact
effective values and provenance with `roba config effective`.

Each invocation admits one finite operation and waits for terminal settlement.";

const SERVE_AFTER_HELP: &str = "\
Examples:
  mcp-repl -- roba serve --provider codex
  mcp-repl --protocol final -- roba serve --provider claude --git --writable

The same versioned startup config and override rules as `roba run` apply.
The resolved config is pinned for the lifetime of this process.

The process hosts one persistent logical agent with at most one active operation.
Each agent.turn is finite; agent.interrupt keeps the host available, while
agent.shutdown drains active work and terminates it. stdout is MCP wire data.";

/// One MCP-native harness for a finite or hot Claude/Codex agent.
#[derive(Parser, Debug)]
#[command(
    version,
    about,
    long_about = None,
    styles = STYLES,
    after_help = AFTER_HELP,
    arg_required_else_help = true,
    subcommand_required = true
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<SubCommand>,

    /// Use PATH as the effective workspace (`git -C` style).
    ///
    /// Applied before configuration discovery, provider launch, and relative
    /// command-specific paths.
    #[arg(short = 'C', long, value_name = "PATH", global = true)]
    pub cwd: Option<PathBuf>,
}

#[derive(Subcommand, Debug)]
pub enum SubCommand {
    /// Run one finite provider-neutral Roba agent.
    Run(RunArgs),
    /// Serve one hot provider-neutral Roba agent over stdio MCP.
    Serve(ServeArgs),
    /// Inspect provider-neutral startup configuration.
    Config {
        #[command(subcommand)]
        cmd: ConfigCmd,
    },
    /// Generate a shell completion script.
    Completions {
        /// Target shell (bash, zsh, fish, powershell, or elvish).
        shell: clap_complete::Shell,
    },
}

/// Provider selected for one logical agent.
#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunProvider {
    Claude,
    Codex,
}

/// Provider-neutral reasoning effort.
#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EffortLevel {
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

/// Fixed configuration shared by finite and hot provider-neutral agents.
#[derive(ClapArgs, Debug)]
pub struct AgentArgs {
    /// Use only this provider-neutral versioned config file.
    #[arg(long, value_name = "PATH", conflicts_with = "no_config")]
    pub config: Option<PathBuf>,

    /// Ignore all provider-neutral startup config files.
    #[arg(long, conflicts_with = "config")]
    pub no_config: bool,

    /// Provider for this agent. Defaults to Claude when unset by config.
    #[arg(long, value_enum)]
    pub provider: Option<RunProvider>,

    /// Provider model id.
    #[arg(long)]
    pub model: Option<String>,

    /// Provider-neutral reasoning effort.
    #[arg(long, value_enum)]
    pub effort: Option<EffortLevel>,

    /// Provider instruction delivered on every finite turn. Repeat to compose.
    #[arg(long = "instruction")]
    pub instructions: Vec<String>,

    /// Context delivered to the provider and recorded in the context manifest.
    #[arg(long = "context")]
    pub context: Vec<String>,

    /// Add the repository-scoped Git MCP service.
    #[arg(long)]
    pub git: bool,

    /// Disable Git MCP even when startup config enables it.
    #[arg(long, conflicts_with = "git")]
    pub no_git: bool,

    /// Force read-only authority even when startup config grants writes.
    #[arg(long, conflicts_with_all = ["writable", "full_auto"])]
    pub read_only: bool,

    /// Permit edits in the current workspace.
    #[arg(long, conflicts_with_all = ["read_only", "full_auto"])]
    pub writable: bool,

    /// Run unattended inside a workspace-write sandbox.
    #[arg(long, conflicts_with_all = ["read_only", "writable"])]
    pub full_auto: bool,

    /// Per-run provider turn ceiling. Unsupported providers refuse before launch.
    #[arg(long)]
    pub max_turns: Option<u32>,

    /// Per-run provider-reported dollar ceiling.
    #[arg(long)]
    pub max_cost_usd: Option<f64>,

    /// Per-run wall-clock provider deadline in seconds.
    #[arg(long)]
    pub timeout: Option<u64>,

    /// Seed this agent from a provider session or thread id.
    #[arg(long)]
    pub resume: Option<String>,
}

#[derive(ClapArgs, Debug)]
#[command(after_help = RUN_AFTER_HELP)]
pub struct RunArgs {
    #[command(flatten)]
    pub agent: AgentArgs,

    /// Initial intention for this finite run.
    pub prompt: String,

    /// Emit the terminal run snapshot as a versioned JSON envelope.
    #[arg(long)]
    pub json: bool,
}

#[derive(ClapArgs, Debug)]
#[command(after_help = SERVE_AFTER_HELP)]
pub struct ServeArgs {
    #[command(flatten)]
    pub agent: AgentArgs,
}

#[derive(Subcommand, Debug)]
pub enum ConfigCmd {
    /// Show effective startup configuration and field provenance.
    Effective(ConfigEffectiveArgs),
}

#[derive(ClapArgs, Debug)]
pub struct ConfigEffectiveArgs {
    #[command(flatten)]
    pub agent: AgentArgs,

    /// Emit a versioned JSON envelope instead of TOML.
    #[arg(long)]
    pub json: bool,
}

#[cfg(test)]
mod tests {
    use clap::{CommandFactory, error::ErrorKind};

    use super::*;

    #[test]
    fn root_requires_a_subcommand_and_keeps_only_the_current_surface() {
        let error = Cli::try_parse_from(["roba"]).unwrap_err();
        assert_eq!(
            error.kind(),
            ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
        );

        let help = Cli::command().render_long_help().to_string();
        for command in ["run", "serve", "config", "completions"] {
            assert!(help.contains(command), "missing {command}: {help}");
        }
        for removed in ["history", "profile", "alias", "doctor", "worktree"] {
            assert!(!help.contains(removed), "stale {removed}: {help}");
        }
    }

    #[test]
    fn finite_and_hot_commands_share_agent_configuration() {
        let run = Cli::try_parse_from([
            "roba",
            "run",
            "--provider",
            "codex",
            "--writable",
            "--git",
            "work",
        ])
        .unwrap();
        assert!(matches!(run.command, Some(SubCommand::Run(_))));

        let serve = Cli::try_parse_from([
            "roba",
            "serve",
            "--provider",
            "codex",
            "--writable",
            "--git",
        ])
        .unwrap();
        assert!(matches!(serve.command, Some(SubCommand::Serve(_))));
    }

    #[test]
    fn permission_postures_are_mutually_exclusive() {
        let error =
            Cli::try_parse_from(["roba", "run", "--read-only", "--writable", "work"]).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::ArgumentConflict);
    }

    #[test]
    fn removed_root_and_config_surfaces_fail_at_parse_time() {
        for args in [
            vec!["roba", "legacy prompt"],
            vec!["roba", "history"],
            vec!["roba", "--profile", "worker", "run", "work"],
            vec!["roba", "config", "show"],
        ] {
            assert!(Cli::try_parse_from(args).is_err());
        }
    }
}
