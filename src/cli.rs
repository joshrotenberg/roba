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
  roba init
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
    /// Applied before configuration discovery, provider launch when applicable,
    /// and relative command-specific paths.
    #[arg(short = 'C', long, value_name = "PATH", global = true)]
    pub cwd: Option<PathBuf>,
}

#[derive(Subcommand, Debug)]
pub enum SubCommand {
    /// Create a conservative provider-neutral `roba.toml`.
    Init(InitArgs),
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

/// Provider-native ambient-context posture.
#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AmbientContextMode {
    /// Preserve the provider's normal user and workspace discovery.
    Ambient,
    /// Apply the provider's tested reduction and report retained sources.
    Controlled,
    /// Permit only Roba-declared context; unsupported providers refuse.
    Hermetic,
}

/// Provider-neutral provider-session continuity policy.
#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionModeArg {
    /// Retain validated provider continuity until explicitly rotated.
    Sticky,
    /// Start every admitted operation in a fresh provider session.
    Fresh,
    /// Retain continuity under host policy; phase one rotates only explicitly.
    Managed,
}

/// Fixed configuration shared by finite and hot provider-neutral agents.
#[derive(ClapArgs, Debug, Default)]
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

    /// Provider-native ambient context posture.
    ///
    /// Narrower modes fail before launch when the selected adapter cannot
    /// enforce them. Inspect exact retained, suppressed, and unobservable
    /// source classes via `roba://context`.
    #[arg(long = "ambient-context", value_enum)]
    pub ambient_context: Option<AmbientContextMode>,

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

    /// Provider-session continuity policy for this logical agent.
    #[arg(long = "session-mode", value_enum)]
    pub session_mode: Option<SessionModeArg>,
}

/// Deterministically initialize the effective workspace.
#[derive(ClapArgs, Debug)]
pub struct InitArgs {
    /// Select one managed agent role from the shipped context catalog.
    #[arg(long, value_name = "ID")]
    pub agent_role: Option<String>,

    /// Select an additional managed skill. Repeat to compose.
    #[arg(long = "skill", value_name = "ID", requires = "agent_role")]
    pub skills: Vec<String>,

    /// Enable a reusable managed prompt. Repeat to compose.
    #[arg(long = "prompt", value_name = "ID", requires = "agent_role")]
    pub prompts: Vec<String>,

    /// Print the exact validated TOML without creating a file.
    #[arg(long)]
    pub dry_run: bool,
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
    /// Build a bounded, content-free project survey for configuration tuning.
    Survey(ConfigSurveyArgs),
}

#[derive(ClapArgs, Debug)]
pub struct ConfigEffectiveArgs {
    #[command(flatten)]
    pub agent: AgentArgs,

    /// Emit a versioned JSON envelope instead of TOML.
    #[arg(long)]
    pub json: bool,
}

#[derive(ClapArgs, Debug)]
pub struct ConfigSurveyArgs {
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
        for command in ["init", "run", "serve", "config", "completions"] {
            assert!(help.contains(command), "missing {command}: {help}");
        }
        for removed in ["history", "profile", "alias", "doctor", "worktree"] {
            assert!(!help.contains(removed), "stale {removed}: {help}");
        }
    }

    #[test]
    fn config_survey_reuses_startup_overrides_without_accepting_a_prompt() {
        let parsed = Cli::try_parse_from([
            "roba",
            "config",
            "survey",
            "--provider",
            "codex",
            "--ambient-context",
            "controlled",
            "--json",
        ])
        .unwrap();
        let Some(SubCommand::Config {
            cmd: ConfigCmd::Survey(args),
        }) = parsed.command
        else {
            panic!("expected config survey");
        };
        assert_eq!(args.agent.provider, Some(RunProvider::Codex));
        assert_eq!(
            args.agent.ambient_context,
            Some(AmbientContextMode::Controlled)
        );
        assert!(args.json);

        let error =
            Cli::try_parse_from(["roba", "config", "survey", "unexpected-prompt"]).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::UnknownArgument);
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
            "--session-mode",
            "managed",
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
            "--session-mode",
            "managed",
        ])
        .unwrap();
        assert!(matches!(serve.command, Some(SubCommand::Serve(_))));
    }

    #[test]
    fn init_is_deterministic_and_managed_selection_is_explicit() {
        let plain = Cli::try_parse_from(["roba", "init", "--dry-run"]).unwrap();
        let Some(SubCommand::Init(plain)) = plain.command else {
            panic!("expected init");
        };
        assert!(plain.dry_run);
        assert!(plain.agent_role.is_none());
        assert!(plain.skills.is_empty());
        assert!(plain.prompts.is_empty());

        let managed = Cli::try_parse_from([
            "roba",
            "init",
            "--agent-role",
            "roba.repo-worker",
            "--skill",
            "roba.repository-change",
            "--prompt",
            "roba.issue-worker",
        ])
        .unwrap();
        let Some(SubCommand::Init(managed)) = managed.command else {
            panic!("expected init");
        };
        assert_eq!(managed.agent_role.as_deref(), Some("roba.repo-worker"));
        assert_eq!(managed.skills, ["roba.repository-change"]);
        assert_eq!(managed.prompts, ["roba.issue-worker"]);
    }

    #[test]
    fn init_context_selection_requires_an_agent_role() {
        for args in [
            ["roba", "init", "--skill", "roba.repository-change"],
            ["roba", "init", "--prompt", "roba.issue-worker"],
        ] {
            let error = Cli::try_parse_from(args).unwrap_err();
            assert_eq!(error.kind(), ErrorKind::MissingRequiredArgument);
        }
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
