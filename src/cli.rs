//! Clap-derived CLI structs. `Cli` is the top-level entry; default
//! invocation (no subcommand) dispatches to [`crate::run_ask`] with
//! the flattened [`AskArgs`].

use clap::{Args as ClapArgs, Parser, Subcommand};
use std::path::PathBuf;

/// Single-prompt CLI runner built on claude-wrapper.
#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<SubCommand>,

    #[command(flatten)]
    pub ask: AskArgs,
}

#[derive(Subcommand, Debug)]
pub enum SubCommand {
    /// List recent sessions across all projects.
    History(HistoryArgs),
    /// Reprint the most recent session's last answer.
    Last(LastArgs),
    /// Inspect or initialize the user profiles config.
    Profile {
        #[command(subcommand)]
        action: ProfileAction,
    },
    /// Roll up token usage across session history.
    Cost(CostArgs),
}

#[derive(ClapArgs, Debug)]
pub struct CostArgs {
    /// Group totals by project slug.
    #[arg(long)]
    pub by_project: bool,

    /// Filter to one project's sessions by slug.
    #[arg(long, value_name = "SLUG", allow_hyphen_values = true)]
    pub project: Option<String>,

    /// Limit the projects table to the top N by token usage.
    /// Only meaningful with --by-project. Default 10.
    #[arg(short = 'n', long, value_name = "N")]
    pub limit: Option<usize>,

    /// Emit JSON instead of a human table.
    #[arg(long)]
    pub json: bool,
}

#[derive(Subcommand, Debug)]
pub enum ProfileAction {
    /// List profile names defined in the config.
    List,
    /// Print the TOML for one profile by name.
    Show {
        /// Profile name (as it appears under `[profile.NAME]`).
        name: String,
    },
    /// Write a starter `profiles.toml` if none exists yet.
    Init {
        /// Overwrite an existing file instead of refusing.
        #[arg(long)]
        force: bool,
    },
    /// Print the resolved config path.
    Path,
}

#[derive(ClapArgs, Debug)]
pub struct LastArgs {
    /// Filter to one project by slug. Project slugs start with `-`
    /// so this accepts hyphen-prefixed values without quoting.
    #[arg(long, value_name = "SLUG", allow_hyphen_values = true)]
    pub project: Option<String>,
}

#[derive(ClapArgs, Debug)]
pub struct HistoryArgs {
    /// Maximum number of sessions to show (default 10).
    #[arg(short = 'n', long, value_name = "N")]
    pub limit: Option<usize>,

    /// Show all sessions (no limit). Overrides --limit.
    #[arg(long, conflicts_with = "limit")]
    pub all: bool,

    /// Filter to one project by slug. Project slugs start with `-`
    /// so this accepts hyphen-prefixed values without quoting.
    #[arg(long, value_name = "SLUG", allow_hyphen_values = true)]
    pub project: Option<String>,

    /// Emit JSON instead of a human table.
    #[arg(long)]
    pub json: bool,
}

#[derive(ClapArgs, Debug)]
pub struct AskArgs {
    // ----- Prompt sources ---------------------------------------------------
    /// Prompt text. Use `-` for stdin, or omit when piping.
    #[arg(conflicts_with_all = ["file", "editor"])]
    pub prompt: Option<String>,

    /// Read the prompt from a file.
    #[arg(
        short,
        long,
        value_name = "PATH",
        conflicts_with = "editor",
        help_heading = "Prompt sources"
    )]
    pub file: Option<PathBuf>,

    /// Compose in $VISUAL / $EDITOR (falls back to vi).
    #[arg(short = 'e', long = "editor", help_heading = "Prompt sources")]
    pub editor: bool,

    // ----- Composition ------------------------------------------------------
    /// Prepend a file to the prompt (repeatable).
    #[arg(long, value_name = "PATH", help_heading = "Composition")]
    pub prepend: Vec<PathBuf>,

    /// Append a file to the prompt (repeatable).
    #[arg(long, value_name = "PATH", help_heading = "Composition")]
    pub append: Vec<PathBuf>,

    /// Embed glob-matched files with `File: PATH` framing (repeatable).
    #[arg(long, value_name = "GLOB", help_heading = "Composition")]
    pub attach: Vec<String>,

    /// Embed `git diff` (working tree) as a context block.
    #[arg(long, help_heading = "Composition")]
    pub git_diff: bool,

    /// Embed `git log --oneline -n N` (default 5).
    #[arg(
        long,
        value_name = "N",
        num_args(0..=1),
        default_missing_value = "5",
        help_heading = "Composition"
    )]
    pub git_log: Option<usize>,

    /// Embed `git status --short` as a context block.
    #[arg(long, help_heading = "Composition")]
    pub git_status: bool,

    /// Substitute `{{KEY}}` placeholders (repeatable).
    #[arg(
        long,
        value_name = "K=V",
        value_parser = parse_kv,
        help_heading = "Composition"
    )]
    pub var: Vec<(String, String)>,

    // ----- Output -----------------------------------------------------------
    /// Answer only -- no metadata, no decoration.
    #[arg(short = 'q', long, help_heading = "Output")]
    pub quiet: bool,

    /// Full structured result as JSON on stdout.
    #[arg(long, help_heading = "Output")]
    pub json: bool,

    /// Print only fenced code blocks (optional language filter).
    #[arg(
        long,
        value_name = "LANG",
        num_args(0..=1),
        default_missing_value = "",
        conflicts_with = "json",
        help_heading = "Output"
    )]
    pub code: Option<String>,

    /// First N lines only.
    #[arg(
        long,
        value_name = "N",
        conflicts_with_all = ["tail", "json"],
        help_heading = "Output"
    )]
    pub head: Option<usize>,

    /// Last N lines only.
    #[arg(
        long,
        value_name = "N",
        conflicts_with_all = ["head", "json"],
        help_heading = "Output"
    )]
    pub tail: Option<usize>,

    /// Write the result to PATH instead of stdout.
    #[arg(long, value_name = "PATH", help_heading = "Output")]
    pub save: Option<PathBuf>,

    /// Write the result to both stdout and PATH (like tee).
    #[arg(
        long,
        value_name = "PATH",
        conflicts_with = "save",
        help_heading = "Output"
    )]
    pub tee: Option<PathBuf>,

    /// Stream tokens as they arrive.
    #[arg(
        long,
        conflicts_with_all = ["json", "code", "head", "tail", "save", "tee"],
        help_heading = "Output"
    )]
    pub stream: bool,

    /// Print the resolved prompt before the response.
    #[arg(long, help_heading = "Output")]
    pub echo: bool,

    /// No rendering, color, or spinner.
    #[arg(long, help_heading = "Output")]
    pub plain: bool,

    // ----- Sessions ---------------------------------------------------------
    /// Continue the most recent session in this directory.
    #[arg(
        short = 'c',
        long = "continue",
        conflicts_with = "resume",
        help_heading = "Sessions"
    )]
    pub continue_session: bool,

    /// Resume a specific session by id.
    #[arg(long, value_name = "ID", help_heading = "Sessions")]
    pub resume: Option<String>,

    /// Branch the resumed session instead of appending (requires --resume).
    #[arg(long, requires = "resume", help_heading = "Sessions")]
    pub fork: bool,

    /// Interactive fuzzy chooser over recent sessions.
    #[arg(
        long,
        conflicts_with_all = ["continue_session", "resume"],
        help_heading = "Sessions"
    )]
    pub pick: bool,

    // ----- Permissions ------------------------------------------------------
    /// Read, Glob, Grep tools only.
    #[arg(long, conflicts_with = "full_auto", help_heading = "Permissions")]
    pub readonly: bool,

    /// Bypass all tool permission checks (sandbox use only).
    #[arg(long, help_heading = "Permissions")]
    pub full_auto: bool,

    // ----- Profiles ---------------------------------------------------------
    /// Apply a named profile from ~/.config/cwr/profiles.toml.
    #[arg(long, value_name = "NAME", help_heading = "Profiles")]
    pub profile: Option<String>,
}

/// Parser for `--var K=V`. Splits on the first `=` so values may
/// contain additional `=` characters. Rejects an empty key.
pub fn parse_kv(s: &str) -> std::result::Result<(String, String), String> {
    let (k, v) = s
        .split_once('=')
        .ok_or_else(|| format!("expected K=V, got `{s}`"))?;
    if k.is_empty() {
        return Err(format!("empty key in `{s}`"));
    }
    Ok((k.to_string(), v.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_kv_splits_on_first_equals() {
        assert_eq!(
            parse_kv("foo=bar"),
            Ok(("foo".to_string(), "bar".to_string()))
        );
    }

    #[test]
    fn parse_kv_keeps_equals_in_value() {
        assert_eq!(
            parse_kv("x=a=b=c"),
            Ok(("x".to_string(), "a=b=c".to_string()))
        );
    }

    #[test]
    fn parse_kv_rejects_no_equals() {
        assert!(parse_kv("foo").is_err());
    }

    #[test]
    fn parse_kv_rejects_empty_key() {
        assert!(parse_kv("=bar").is_err());
    }

    #[test]
    fn parse_kv_accepts_empty_value() {
        assert_eq!(parse_kv("k="), Ok(("k".to_string(), String::new())));
    }
}
