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
    /// The prompt to send to Claude. Pass `-` to read from stdin
    /// explicitly. If omitted and stdin is piped, stdin is used.
    #[arg(conflicts_with_all = ["file", "editor"])]
    pub prompt: Option<String>,

    /// Read the prompt from a file.
    #[arg(short, long, value_name = "PATH", conflicts_with = "editor")]
    pub file: Option<PathBuf>,

    /// Open $VISUAL or $EDITOR (falling back to vi) to compose the
    /// prompt. The editor opens an empty markdown buffer; on save +
    /// exit the content becomes the prompt. Aborts on non-zero
    /// editor exit or empty buffer.
    #[arg(short = 'e', long = "editor")]
    pub editor: bool,

    /// Print the resolved prompt before the response, separated by
    /// a divider. Useful with -e / -f / stdin where the sent prompt
    /// isn't already on screen.
    #[arg(long)]
    pub echo: bool,

    /// Suppress everything except the answer on stdout. Overrides
    /// --echo and (later) the cost footer / any other stderr noise.
    /// Use when you want the cleanest possible output even on a TTY.
    #[arg(short = 'q', long)]
    pub quiet: bool,

    /// Emit the full structured result as JSON on stdout instead of
    /// the plain answer. Includes session_id, cost_usd, duration_ms,
    /// num_turns, is_error, and the answer text. Pretty-printed.
    #[arg(long)]
    pub json: bool,

    /// Write the result to PATH instead of stdout. If the path ends
    /// in .json, the full structured record is written; otherwise
    /// the plain answer. --json overrides the extension and forces
    /// JSON regardless.
    #[arg(long, value_name = "PATH")]
    pub save: Option<PathBuf>,

    /// Write the result to both stdout and PATH (like Unix tee).
    /// Same extension-driven format rules as --save.
    #[arg(long, value_name = "PATH", conflicts_with = "save")]
    pub tee: Option<PathBuf>,

    /// Print only fenced code blocks from the answer. Pass with no
    /// value to take every block; pass --code rust to filter by
    /// language. Multiple blocks are separated by a blank line.
    /// Mutually exclusive with --json.
    #[arg(
        long,
        value_name = "LANG",
        num_args(0..=1),
        default_missing_value = "",
        conflicts_with = "json",
    )]
    pub code: Option<String>,

    /// Truncate output to the first N lines. Applied after --code.
    /// Mutually exclusive with --tail and --json.
    #[arg(long, value_name = "N", conflicts_with_all = ["tail", "json"])]
    pub head: Option<usize>,

    /// Truncate output to the last N lines. Applied after --code.
    /// Mutually exclusive with --head and --json.
    #[arg(long, value_name = "N", conflicts_with_all = ["head", "json"])]
    pub tail: Option<usize>,

    /// Stream the response to stdout as it arrives instead of waiting
    /// for the full result. Mutually exclusive with the output-shaping
    /// flags (--json, --code, --head, --tail, --save, --tee) since
    /// they all need the final body assembled before they can act.
    #[arg(
        long,
        conflicts_with_all = ["json", "code", "head", "tail", "save", "tee"],
    )]
    pub stream: bool,

    /// Continue the most recent session in this working directory.
    /// Mutually exclusive with --resume.
    #[arg(short = 'c', long = "continue", conflicts_with = "resume")]
    pub continue_session: bool,

    /// Resume a specific session by id.
    #[arg(long, value_name = "ID")]
    pub resume: Option<String>,

    /// Branch the resumed session into a new one instead of appending
    /// to it. Requires --resume. Useful for "what if I asked it this
    /// instead" experiments without polluting the original transcript.
    #[arg(long, requires = "resume")]
    pub fork: bool,

    /// Open an interactive fuzzy-filter picker over recent sessions
    /// and resume the one you select. Requires a TTY. Mutually
    /// exclusive with -c / --resume.
    #[arg(long, conflicts_with_all = ["continue_session", "resume"])]
    pub pick: bool,

    /// Restrict claude to read-only tools (Read, Glob, Grep). Good
    /// default for "summarize / explain this code" prompts where you
    /// don't want any edits or shell access. Mutually exclusive with
    /// --full-auto.
    #[arg(long, conflicts_with = "full_auto")]
    pub readonly: bool,

    /// Bypass all tool permission checks. Equivalent to claude's
    /// --dangerously-skip-permissions. Only use in a sandbox you
    /// trust. Mutually exclusive with --readonly.
    #[arg(long)]
    pub full_auto: bool,

    /// Prepend the contents of a file to the prompt. Can be passed
    /// multiple times; files are joined in order with blank lines
    /// between them. Composes with positional / -f / -e / stdin.
    #[arg(long, value_name = "PATH")]
    pub prepend: Vec<PathBuf>,

    /// Append the contents of a file to the prompt. Same semantics
    /// as --prepend but joined after the main prompt.
    #[arg(long, value_name = "PATH")]
    pub append: Vec<PathBuf>,

    /// Attach files matching a glob pattern into the prompt. Each
    /// file is included with a `File: PATH` header and a fenced
    /// code block. Pass multiple times for several patterns. Useful
    /// for "look at these files and answer X" prompts without
    /// having to paste contents by hand.
    #[arg(long, value_name = "GLOB")]
    pub attach: Vec<String>,

    /// Embed `git diff` output (working-tree changes) as a context
    /// block before the prompt.
    #[arg(long)]
    pub git_diff: bool,

    /// Embed `git log --oneline -n N` as a context block. Bare
    /// --git-log defaults to 5 commits.
    #[arg(long, value_name = "N", num_args(0..=1), default_missing_value = "5")]
    pub git_log: Option<usize>,

    /// Embed `git status --short` as a context block.
    #[arg(long)]
    pub git_status: bool,

    /// Substitute `{{KEY}}` placeholders in the assembled prompt with
    /// the given value. Pass multiple --var K=V flags for several
    /// substitutions. Applied after all composition.
    #[arg(long, value_name = "K=V", value_parser = parse_kv)]
    pub var: Vec<(String, String)>,

    /// Apply a named profile from `~/.config/cwr/profiles.toml`. The
    /// profile fills in any flags you didn't pass on the command line.
    /// CLI flags always override profile values.
    #[arg(long, value_name = "NAME")]
    pub profile: Option<String>,

    /// Disable all visual decoration -- no markdown rendering, no
    /// spinner, no color. Useful when piping into a script or when
    /// the rendered form is getting in the way. NO_COLOR=1 in the
    /// environment achieves a partial version (color only).
    #[arg(long)]
    pub plain: bool,
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
