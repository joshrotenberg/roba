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

    /// Run as if invoked from PATH (changes the working directory
    /// before any other resolution: session scoping, config walk-up,
    /// `--attach` globs, `--prepend` / `--append` relative paths,
    /// `--git-*` context).
    #[arg(short = 'C', long, value_name = "PATH", global = true)]
    pub cwd: Option<PathBuf>,
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
    /// Manage the bundled skill library.
    Skill {
        #[command(subcommand)]
        action: SkillAction,
    },
    /// Manage the bundled agent library.
    Agent {
        #[command(subcommand)]
        action: AgentAction,
    },
    /// Inspect user-defined aliases (`[alias.NAME]` in roba.toml).
    Alias {
        #[command(subcommand)]
        action: AliasAction,
    },
    /// Captures a user-defined alias invocation (`roba NAME [args]`).
    /// Not a real subcommand -- clap routes any unrecognized leading
    /// word here, and [`crate::dispatch`] expands it against the alias
    /// pool (or errors with close-match suggestions).
    #[command(external_subcommand)]
    External(Vec<String>),
}

#[derive(Subcommand, Debug)]
pub enum AliasAction {
    /// List aliases defined in the merged config pool.
    List,
    /// Print one alias's definition plus an expansion preview.
    Show {
        /// Alias name (as it appears under `[alias.NAME]`).
        name: String,
    },
    /// Print which files contribute aliases, in walk-up order.
    Path,
}

#[derive(Subcommand, Debug)]
pub enum SkillAction {
    /// Copy bundled skills to ~/.claude/skills/ (or a custom destination).
    Install(InstallArgs),
    /// List bundled skills with descriptions.
    List,
    /// Print the SKILL.md body for a named skill.
    Show {
        /// Skill name (the directory name, e.g. `draft-pr-first`).
        name: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum AgentAction {
    /// Copy bundled agents to ~/.claude/agents/ (or a custom destination).
    Install(InstallArgs),
    /// List bundled agents with descriptions.
    List,
    /// Print the AGENT.md body for a named agent.
    Show {
        /// Agent name (the directory name, e.g. `roba-runner`).
        name: String,
    },
}

#[derive(ClapArgs, Debug)]
pub struct InstallArgs {
    /// Custom destination directory. Default: ~/.claude/skills (or .../agents).
    #[arg(long, value_name = "PATH")]
    pub to: Option<PathBuf>,
    /// Preview without writing.
    #[arg(long)]
    pub dry_run: bool,
    /// Overwrite existing files without prompting.
    #[arg(long, conflicts_with = "skip")]
    pub force: bool,
    /// Leave existing files in place; install the rest.
    #[arg(long)]
    pub skip: bool,
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
    /// Write a starter `roba.toml` if none exists yet.
    Init {
        /// Overwrite an existing file instead of refusing.
        #[arg(long)]
        force: bool,
    },
    /// Print the resolved config path(s) and pool sources.
    Path,
    /// Show which profile would auto-apply right now (none, env, or default).
    Active,
}

#[derive(ClapArgs, Debug)]
pub struct LastArgs {
    /// How many items to show (default 1).
    #[arg(short = 'n', long = "number", value_name = "N")]
    pub number: Option<usize>,

    /// What kind of item to show.
    #[arg(long = "type", value_enum, default_value_t = LastKind::Text)]
    pub kind: LastKind,

    /// Filter to one project by slug. Overrides cwd inference.
    #[arg(long, value_name = "SLUG", allow_hyphen_values = true)]
    pub project: Option<String>,

    /// Look across all projects instead of just the current cwd's.
    #[arg(long, conflicts_with = "project")]
    pub all_projects: bool,
}

#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum LastKind {
    /// Assistant text answers (default).
    Text,
    /// Tool calls only.
    Tools,
    /// Everything in order -- text answers interleaved with tool calls.
    All,
}

impl LastKind {
    pub fn label(self) -> &'static str {
        match self {
            LastKind::Text => "text answers",
            LastKind::Tools => "tool calls",
            LastKind::All => "items",
        }
    }
}

#[derive(ClapArgs, Debug)]
pub struct HistoryArgs {
    /// Maximum number of sessions to show (default 10).
    #[arg(short = 'n', long, value_name = "N")]
    pub limit: Option<usize>,

    /// Show all sessions (no count limit). Overrides --limit.
    #[arg(long, conflicts_with = "limit")]
    pub all: bool,

    /// Filter to one project by slug. Overrides cwd inference.
    #[arg(long, value_name = "SLUG", allow_hyphen_values = true)]
    pub project: Option<String>,

    /// Look across all projects instead of just the current cwd's.
    #[arg(long, conflicts_with = "project")]
    pub all_projects: bool,

    /// Emit JSON instead of a human table.
    #[arg(long)]
    pub json: bool,
}

#[derive(ClapArgs, Debug)]
pub struct AskArgs {
    // ----- Prompt sources ---------------------------------------------------
    /// The prompt text (positional). Pass `-` for explicit stdin. For
    /// invocations where the positional form is ambiguous against
    /// optional-value flags (`-c`, `-w`), use `--prompt VALUE` /
    /// `-p VALUE` instead.
    #[arg(conflicts_with_all = ["file", "editor"])]
    pub prompt: Option<String>,

    /// Explicit prompt string. Use this when the positional form is
    /// ambiguous against an optional-value flag (e.g. `roba -c -p
    /// "..."` to continue most recent with a prompt that would
    /// otherwise be parsed as the continue ID). Mutually exclusive
    /// with the positional `[PROMPT]` argument.
    #[arg(
        short = 'p',
        long = "prompt",
        value_name = "TEXT",
        conflicts_with = "prompt",
        help_heading = "Prompt sources"
    )]
    pub prompt_flag: Option<String>,

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

    /// With `-e`, pre-fill the editor with the last N assistant
    /// responses from the most recent session in this dir, separated
    /// from your prompt by a scissors line. Default 1; pass 0 to
    /// disable. Only meaningful with `-e`.
    #[arg(long, value_name = "N", help_heading = "Prompt sources")]
    pub editor_history: Option<usize>,

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
    /// Suppress metadata: footer, spinner, tool-call markers. For rendering off, see --plain.
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

    /// Write the result to a file AND to stdout (format from
    /// extension, or --json wins).
    #[arg(short = 'o', long, value_name = "PATH", help_heading = "Output")]
    pub out: Option<PathBuf>,

    /// Write the spawned claude session's streaming events to PATH as
    /// they arrive (JSONL). Stable observability handle for in-flight
    /// runs; survives roba's exit. Forces the streaming pipeline
    /// internally even when --stream is not set.
    #[arg(long, value_name = "PATH", help_heading = "Output")]
    pub trace: Option<PathBuf>,

    /// TTY-only progress indicator: stream tokens + inline tool-call lines as they arrive. Never load-bearing on a pipe; conflicts with --json / --code / --out.
    #[arg(
        long,
        conflicts_with_all = ["json", "code", "out"],
        help_heading = "Output"
    )]
    pub stream: bool,

    /// Render extended-thinking blocks live on stderr. Only takes
    /// effect with `--stream`; ignored otherwise.
    #[arg(long, help_heading = "Output")]
    pub show_thinking: bool,

    /// Print the resolved prompt before the response.
    #[arg(long, help_heading = "Output")]
    pub echo: bool,

    /// Disable markdown rendering, color, and spinner. Footer still prints; for answer-only, see --quiet.
    #[arg(long, help_heading = "Output")]
    pub plain: bool,

    // ----- Failure modes ----------------------------------------------------
    /// Disable wrapper-level auto-retry on transient failures. The
    /// orchestrator gets the failure immediately and decides whether
    /// to retry, instead of roba quietly re-trying with exponential
    /// backoff. No effect on success or non-transient failures.
    #[arg(long, help_heading = "Failure modes")]
    pub no_retry: bool,

    // ----- Model ------------------------------------------------------------
    /// Override the claude model for this call.
    ///
    /// Accepts an alias (`sonnet`, `opus`, `haiku`) or a full model
    /// ID (`claude-sonnet-4-6`, `claude-opus-4-7`, etc.). Passed
    /// through to `claude -p --model`.
    #[arg(long, value_name = "MODEL", help_heading = "Model")]
    pub model: Option<String>,

    // ----- Sessions ---------------------------------------------------------
    /// Continue an existing session. Bare `-c` resumes the most
    /// recent session in this directory; `-c ID` (or `-c=ID`) resumes
    /// a specific session by id. Because the value is optional, a
    /// space-separated word after `-c` is consumed as the id: `roba -c
    /// "follow up"` treats "follow up" as the session id, not the
    /// prompt. To continue the most recent session with a prompt, use
    /// `roba -c -p "follow up"`.
    #[arg(
        short = 'c',
        long = "continue",
        num_args = 0..=1,
        value_name = "ID",
        help_heading = "Sessions"
    )]
    pub continue_session: Option<Option<String>>,

    /// Branch the resumed session instead of appending. Requires an
    /// explicit session id (`-c=ID --fork`); you can't fork "the most
    /// recent" without naming it.
    #[arg(long, requires = "continue_session", help_heading = "Sessions")]
    pub fork: bool,

    /// Interactive fuzzy chooser over recent sessions.
    #[arg(long, conflicts_with = "continue_session", help_heading = "Sessions")]
    pub pick: bool,

    /// Force a fresh session. Cancels a profile- or env-supplied
    /// `continue = true`. The kill switch for accidental
    /// auto-continuation.
    #[arg(
        long,
        conflicts_with_all = ["continue_session", "pick"],
        help_heading = "Sessions"
    )]
    pub fresh: bool,

    /// Run in a fresh git worktree. With no value, claude generates
    /// the name; with a value (`-w NAME` or `-w=NAME`), pin the
    /// worktree directory/branch (e.g. `-w feature-x`). Because the
    /// value is optional, a space-separated word after `-w` is
    /// consumed as the name: `roba -w "do it"` treats "do it" as the
    /// worktree name, not the prompt. To name a worktree and pass a
    /// prompt, use `roba -w NAME -p "..."`. The worktree persists
    /// after the session; clean up manually with `git worktree
    /// remove`. Pairs naturally with `--writable` or `--full-auto` --
    /// the worktree is your sandbox.
    #[arg(
        short = 'w',
        long,
        value_name = "NAME",
        num_args(0..=1),
        help_heading = "Sessions"
    )]
    pub worktree: Option<Option<String>>,

    /// Pin a specific claude-code subagent for this run. The named
    /// subagent must exist in `.claude/agents/NAME.md` within the cwd
    /// (or be auto-discovered per claude's standard lookup). Lets an
    /// orchestrator dispatch a run as a known agent instead of an
    /// unscoped default claude.
    #[arg(long, value_name = "NAME", help_heading = "Sessions")]
    pub agent: Option<String>,

    // ----- Permissions ------------------------------------------------------
    /// Explicit form of the default: Read, Glob, Grep only. No-op (the default).
    #[arg(long, conflicts_with = "full_auto", help_heading = "Permissions")]
    pub readonly: bool,

    /// Add Edit + Write to the allow list (preset for code edits).
    #[arg(long, conflicts_with = "full_auto", help_heading = "Permissions")]
    pub writable: bool,

    /// Bypass all tool permission checks (sandbox use only).
    #[arg(long, help_heading = "Permissions")]
    pub full_auto: bool,

    /// Allow a tool or tool pattern (repeatable). Adds to the default.
    #[arg(long = "allow-tool", value_name = "TOOL", help_heading = "Permissions")]
    pub allow_tool: Vec<String>,

    /// Deny a tool or tool pattern (repeatable).
    #[arg(long = "deny-tool", value_name = "TOOL", help_heading = "Permissions")]
    pub deny_tool: Vec<String>,

    /// Resolve permissions across all layers (CLI > env > profile >
    /// built-in default), print the effective allow/deny lists with
    /// per-entry provenance, and exit 0 without calling claude. Useful
    /// for verifying what a profile actually opens up before you rely
    /// on it.
    #[arg(long, help_heading = "Permissions")]
    pub show_permissions: bool,

    // ----- Permission provenance (internal; not a CLI surface) --------------
    // Populated as each layer contributes a value, so --show-permissions
    // can report where the effective permission set came from. The layer
    // label is one of "CLI", "env", "profile.<name>", or "config".
    /// Layer that set `readonly`.
    #[clap(skip)]
    pub readonly_source: Option<String>,
    /// Layer that set `writable`.
    #[clap(skip)]
    pub writable_source: Option<String>,
    /// Layer that set `full_auto`.
    #[clap(skip)]
    pub full_auto_source: Option<String>,
    /// Layer per `allow_tool` entry (parallel-indexed to `allow_tool`).
    #[clap(skip)]
    pub allow_tool_sources: Vec<String>,
    /// Layer per `deny_tool` entry (parallel-indexed to `deny_tool`).
    #[clap(skip)]
    pub deny_tool_sources: Vec<String>,

    // ----- Profiles ---------------------------------------------------------
    /// Apply a named profile (user, project, or env source).
    #[arg(long, value_name = "NAME", help_heading = "Profiles")]
    pub profile: Option<String>,

    /// Skip auto-applying `default` and `ROBA_PROFILE`.
    #[arg(long, help_heading = "Profiles")]
    pub no_default_profile: bool,
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

    #[test]
    fn out_and_json_compose() {
        // --out (destination) and --json (format) are orthogonal axes:
        // they must parse together without conflict. The run path then
        // lets --json force JSON regardless of the path extension.
        use clap::Parser;
        let cli =
            Cli::try_parse_from(["roba", "ask thing", "--out", "result.txt", "--json"]).unwrap();
        assert_eq!(
            cli.ask.out.as_deref(),
            Some(std::path::Path::new("result.txt"))
        );
        assert!(cli.ask.json);
    }

    #[test]
    fn worktree_missing_is_none() {
        use clap::Parser;
        let cli = Cli::try_parse_from(["roba", "do thing"]).unwrap();
        assert!(cli.ask.worktree.is_none());
    }

    #[test]
    fn worktree_short_alone_is_presence() {
        // Bare `-w` (claude generates the name). A following `-p` flag is
        // not consumed as the worktree value, so the prompt comes through.
        use clap::Parser;
        let cli = Cli::try_parse_from(["roba", "-w", "-p", "do thing"]).unwrap();
        assert_eq!(cli.ask.worktree, Some(None));
        assert_eq!(cli.ask.prompt_flag.as_deref(), Some("do thing"));
    }

    #[test]
    fn worktree_long_alone_is_presence() {
        use clap::Parser;
        let cli = Cli::try_parse_from(["roba", "--worktree", "-p", "do thing"]).unwrap();
        assert_eq!(cli.ask.worktree, Some(None));
        assert_eq!(cli.ask.prompt_flag.as_deref(), Some("do thing"));
    }

    #[test]
    fn worktree_short_equals_name() {
        use clap::Parser;
        let cli = Cli::try_parse_from(["roba", "-w=mybranch", "do thing"]).unwrap();
        assert_eq!(cli.ask.worktree, Some(Some("mybranch".to_string())));
        assert_eq!(cli.ask.prompt.as_deref(), Some("do thing"));
    }

    #[test]
    fn worktree_long_equals_name() {
        use clap::Parser;
        let cli = Cli::try_parse_from(["roba", "--worktree=mybranch", "do thing"]).unwrap();
        assert_eq!(cli.ask.worktree, Some(Some("mybranch".to_string())));
    }

    #[test]
    fn agent_parses_with_name() {
        use clap::Parser;
        let cli = Cli::try_parse_from(["roba", "--agent", "reviewer", "prompt"]).unwrap();
        assert_eq!(cli.ask.agent.as_deref(), Some("reviewer"));
        assert_eq!(cli.ask.prompt.as_deref(), Some("prompt"));
    }

    #[test]
    fn agent_omitted_is_none() {
        use clap::Parser;
        let cli = Cli::try_parse_from(["roba", "prompt"]).unwrap();
        assert!(cli.ask.agent.is_none());
    }

    #[test]
    fn no_retry_parses_alone() {
        use clap::Parser;
        let cli = Cli::try_parse_from(["roba", "--no-retry", "prompt"]).unwrap();
        assert!(cli.ask.no_retry);
        assert_eq!(cli.ask.prompt.as_deref(), Some("prompt"));
    }

    #[test]
    fn no_retry_conflicts_with_nothing() {
        // --no-retry is an orthogonal failure-mode knob; it must compose
        // freely with output flags like --quiet and --json.
        use clap::Parser;
        let cli = Cli::try_parse_from(["roba", "--no-retry", "--quiet", "prompt"]).unwrap();
        assert!(cli.ask.no_retry);
        assert!(cli.ask.quiet);

        let cli = Cli::try_parse_from(["roba", "--no-retry", "--json", "prompt"]).unwrap();
        assert!(cli.ask.no_retry);
        assert!(cli.ask.json);
    }

    #[test]
    fn trace_flag_parses() {
        use clap::Parser;
        let cli = Cli::try_parse_from(["roba", "--trace", "/tmp/x.jsonl", "prompt"]).unwrap();
        assert_eq!(
            cli.ask.trace.as_deref(),
            Some(std::path::Path::new("/tmp/x.jsonl"))
        );
        assert_eq!(cli.ask.prompt.as_deref(), Some("prompt"));
    }

    #[test]
    fn trace_omitted_is_none() {
        use clap::Parser;
        let cli = Cli::try_parse_from(["roba", "prompt"]).unwrap();
        assert!(cli.ask.trace.is_none());
    }

    #[test]
    fn trace_conflicts_with_nothing() {
        // --trace is observability-orthogonal: it must compose freely
        // with the output flags (--json, -q, --out) and with --stream.
        use clap::Parser;
        let cli = Cli::try_parse_from(["roba", "--trace", "/tmp/x", "--json", "prompt"]).unwrap();
        assert_eq!(
            cli.ask.trace.as_deref(),
            Some(std::path::Path::new("/tmp/x"))
        );
        assert!(cli.ask.json);

        let cli = Cli::try_parse_from(["roba", "--trace", "/tmp/x", "--quiet", "prompt"]).unwrap();
        assert!(cli.ask.trace.is_some());
        assert!(cli.ask.quiet);

        let cli =
            Cli::try_parse_from(["roba", "--trace", "/tmp/x", "--out", "r.txt", "prompt"]).unwrap();
        assert!(cli.ask.trace.is_some());
        assert_eq!(cli.ask.out.as_deref(), Some(std::path::Path::new("r.txt")));

        let cli = Cli::try_parse_from(["roba", "--trace", "/tmp/x", "--stream", "prompt"]).unwrap();
        assert!(cli.ask.trace.is_some());
        assert!(cli.ask.stream);
    }

    #[test]
    fn continue_parses_bare() {
        // Bare `-c` (continue most recent). A following `-p` flag is not
        // consumed as the session id, so the prompt comes through.
        use clap::Parser;
        let cli = Cli::try_parse_from(["roba", "-c", "-p", "prompt"]).unwrap();
        assert_eq!(cli.ask.continue_session, Some(None));
        assert_eq!(cli.ask.prompt_flag.as_deref(), Some("prompt"));
    }

    #[test]
    fn continue_parses_with_id() {
        use clap::Parser;
        let cli = Cli::try_parse_from(["roba", "-c=abc123", "prompt"]).unwrap();
        assert_eq!(cli.ask.continue_session, Some(Some("abc123".to_string())));
        assert_eq!(cli.ask.prompt.as_deref(), Some("prompt"));
    }

    #[test]
    fn continue_long_parses_with_id() {
        use clap::Parser;
        let cli = Cli::try_parse_from(["roba", "--continue=abc123", "prompt"]).unwrap();
        assert_eq!(cli.ask.continue_session, Some(Some("abc123".to_string())));
    }

    #[test]
    fn continue_without_equals_consumes_next_arg_as_id() {
        // BREAKING (pre-0.1.0): with require_equals dropped, `-c prompt`
        // now swallows "prompt" as the session id. The bare "most
        // recent" form is `-c` followed by a flag (or end of args); pass
        // a prompt via `-p` (see continue_bare_then_p_flag_for_prompt).
        use clap::Parser;
        let cli = Cli::try_parse_from(["roba", "-c", "prompt"]).unwrap();
        assert_eq!(cli.ask.continue_session, Some(Some("prompt".to_string())));
        assert!(cli.ask.prompt.is_none());
    }

    #[test]
    fn continue_missing_is_none() {
        use clap::Parser;
        let cli = Cli::try_parse_from(["roba", "prompt"]).unwrap();
        assert!(cli.ask.continue_session.is_none());
    }

    #[test]
    fn fork_requires_continue_at_parse_time() {
        // clap's `requires = "continue_session"` rejects --fork when -c
        // was never passed. (The bare-vs-id distinction is a runtime
        // check; this just enforces -c is present at all.)
        use clap::Parser;
        let err = Cli::try_parse_from(["roba", "--fork", "prompt"]).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("continue") || msg.contains("required"),
            "expected a requires error mentioning continue, got: {msg}"
        );
    }

    #[test]
    fn fork_with_specific_id_parses() {
        use clap::Parser;
        let cli = Cli::try_parse_from(["roba", "-c=abc123", "--fork", "prompt"]).unwrap();
        assert_eq!(cli.ask.continue_session, Some(Some("abc123".to_string())));
        assert!(cli.ask.fork);
    }

    #[test]
    fn pick_conflicts_with_continue() {
        use clap::Parser;
        assert!(Cli::try_parse_from(["roba", "--pick", "-c", "prompt"]).is_err());
    }

    #[test]
    fn fresh_conflicts_with_continue() {
        use clap::Parser;
        assert!(Cli::try_parse_from(["roba", "--fresh", "-c", "prompt"]).is_err());
    }

    #[test]
    fn worktree_long_space_name_attaches_value() {
        // BREAKING (pre-0.1.0): with require_equals dropped, `--worktree
        // NAME` (space form) now attaches NAME to the worktree flag
        // (Some(Some(NAME))) instead of leaving the flag bare and
        // letting NAME fall through to the positional prompt.
        use clap::Parser;
        let cli = Cli::try_parse_from(["roba", "--worktree", "mybranch"]).unwrap();
        assert_eq!(cli.ask.worktree, Some(Some("mybranch".to_string())));
        assert!(cli.ask.prompt.is_none());
    }

    #[test]
    fn external_subcommand_captures_unknown_leading_word() {
        // An unrecognized leading word with trailing args routes to the
        // External variant; the word itself lands in the prompt slot and
        // the rest become the alias args. dispatch() resolves it against
        // the alias pool.
        use clap::Parser;
        let cli = Cli::try_parse_from(["roba", "review", "42", "--readonly"]).unwrap();
        assert_eq!(cli.ask.prompt.as_deref(), Some("review"));
        match cli.command {
            Some(SubCommand::External(rest)) => {
                assert_eq!(rest, vec!["42".to_string(), "--readonly".to_string()]);
            }
            other => panic!("expected External, got {other:?}"),
        }
    }

    #[test]
    fn prompt_flag_parses() {
        use clap::Parser;
        let cli = Cli::try_parse_from(["roba", "-p", "hello"]).unwrap();
        assert_eq!(cli.ask.prompt_flag.as_deref(), Some("hello"));
        assert!(cli.ask.prompt.is_none());
    }

    #[test]
    fn prompt_flag_long_form_parses() {
        use clap::Parser;
        let cli = Cli::try_parse_from(["roba", "--prompt", "hello"]).unwrap();
        assert_eq!(cli.ask.prompt_flag.as_deref(), Some("hello"));
        assert!(cli.ask.prompt.is_none());
    }

    #[test]
    fn prompt_flag_conflicts_with_positional() {
        // clap's `conflicts_with = "prompt"` rejects supplying both the
        // explicit `-p` flag and the positional argument.
        use clap::Parser;
        assert!(Cli::try_parse_from(["roba", "-p", "x", "positional"]).is_err());
    }

    #[test]
    fn continue_with_space_value_consumes_id() {
        // require_equals dropped: a space-separated word after -c is now
        // consumed as the session id. The breaking semantic change.
        use clap::Parser;
        let cli = Cli::try_parse_from(["roba", "-c", "abc123"]).unwrap();
        assert_eq!(cli.ask.continue_session, Some(Some("abc123".to_string())));
        assert!(cli.ask.prompt.is_none());
    }

    #[test]
    fn worktree_with_space_value_and_prompt_flag() {
        // The escape hatch: name a worktree with a space value AND pass
        // a prompt via -p so the two don't collide.
        use clap::Parser;
        let cli = Cli::try_parse_from(["roba", "-w", "mybranch", "-p", "the prompt"]).unwrap();
        assert_eq!(cli.ask.worktree, Some(Some("mybranch".to_string())));
        assert_eq!(cli.ask.prompt_flag.as_deref(), Some("the prompt"));
    }

    #[test]
    fn continue_bare_then_p_flag_for_prompt() {
        // Continue most recent (bare -c) while still passing a prompt via
        // -p -- the documented replacement for the old `-c "prompt"`.
        use clap::Parser;
        let cli = Cli::try_parse_from(["roba", "-c", "-p", "follow up"]).unwrap();
        assert_eq!(cli.ask.continue_session, Some(None));
        assert_eq!(cli.ask.prompt_flag.as_deref(), Some("follow up"));
    }

    #[test]
    fn single_bare_word_is_prompt_not_external() {
        // A single leading word has no trailing positional, so clap
        // keeps it as the prompt (command None). dispatch() then decides
        // whether it names a zero-arg alias.
        use clap::Parser;
        let cli = Cli::try_parse_from(["roba", "commit-msg"]).unwrap();
        assert!(cli.command.is_none());
        assert_eq!(cli.ask.prompt.as_deref(), Some("commit-msg"));
    }
}
