//! Clap-derived CLI structs. `Cli` is the top-level entry; default
//! invocation (no subcommand) dispatches to [`crate::run_ask`] with
//! the flattened [`AskArgs`].

use clap::builder::styling::{AnsiColor, Styles};
use clap::{Args as ClapArgs, Parser, Subcommand};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Help color palette: green-bold section headers + usage, cyan flag
/// names, dim value placeholders. clap auto-disables color on a non-TTY
/// and honors `NO_COLOR` / `CLICOLOR`, so piped or scripted output stays
/// byte-clean -- color is a pure TTY nicety, never on the agent-ABI path.
const STYLES: Styles = Styles::styled()
    .header(AnsiColor::Green.on_default().bold())
    .usage(AnsiColor::Green.on_default().bold())
    .literal(AnsiColor::Cyan.on_default())
    .placeholder(AnsiColor::BrightBlack.on_default());

/// Shown under `-h`: a few worked examples plus a pointer to `--help`
/// for the full reference. Kept short so `-h` stays scannable.
const AFTER_HELP: &str = "\
Examples:
  roba \"explain the borrow checker in 3 bullets\"       one-shot question
  cat err.log | roba \"what's wrong here?\"              pipe stdin
  roba --attach 'src/**/*.rs' \"audit error handling\"   attach files
  roba -c -p \"now add a test for that\"                 continue the last session
  roba --writable \"rename foo to bar in src/\"          let claude edit files

Full flag detail, env vars, and roba.toml config: roba --help";

/// Shown under `--help`: examples plus a self-contained reference for the
/// env-var and roba.toml config layers, so the binary documents itself
/// for humans and agents without depending on an external docs site.
const AFTER_LONG_HELP: &str = "\
Examples:
  roba \"explain the borrow checker in 3 bullets\"       one-shot question
  cat err.log | roba \"what's wrong here?\"              pipe stdin
  roba --attach 'src/**/*.rs' \"audit error handling\"   attach files
  roba -c -p \"now add a test for that\"                 continue the last session
  roba --writable \"rename foo to bar in src/\"          let claude edit files

Environment variables:
  Every long flag has a ROBA_<FLAG> override (uppercased, '-' -> '_'):
  --model -> ROBA_MODEL; bool flags take a truthy value (--writable ->
  ROBA_WRITABLE=1). Special cases:
    ROBA_PROFILE=NAME      apply a profile (like --profile NAME)
    ROBA_VAR_<KEY>=VALUE   set a template var (like --var KEY=VALUE)
    ROBA_RATES_FILE=PATH   override the footer rates table
    NO_COLOR=1             disable color (help and answer rendering)
  Precedence: CLI flag > ROBA_* env > active profile > built-in default.

Configuration (roba.toml):
  Every flag is also a key under [profile.NAME] or at the top level of a
  roba.toml -- discovered by walking up from the cwd, plus
  ~/.config/roba.toml. Closer-to-cwd files win; a `default` profile
  auto-applies. Define [alias.NAME] shortcuts too. See the `roba profile`
  and `roba alias` subcommands.";

/// Single-prompt CLI runner built on claude-wrapper.
#[derive(Parser, Debug)]
#[command(
    version,
    about,
    long_about = None,
    after_help = AFTER_HELP,
    after_long_help = AFTER_LONG_HELP,
    styles = STYLES,
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<SubCommand>,

    #[command(flatten)]
    pub ask: AskArgs,

    /// Run as if invoked from PATH (`git -C` style).
    ///
    /// Changes the working directory before any other resolution:
    /// session scoping, config walk-up, `--attach` globs, `--prepend` /
    /// `--append` relative paths, `--git-*` context.
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
    /// Inspect user-defined aliases (`[alias.NAME]` in roba.toml).
    Alias {
        #[command(subcommand)]
        action: AliasAction,
    },
    /// Generate a shell completion script (bash, zsh, fish, ...).
    ///
    /// Prints the script for SHELL to stdout; pipe or redirect it into
    /// the right place for your shell. Examples:
    ///   roba completions zsh  > ~/.zfunc/_roba
    ///   roba completions bash > /etc/bash_completion.d/roba
    Completions {
        /// Target shell (bash, zsh, fish, powershell, elvish).
        shell: clap_complete::Shell,
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

    /// Override the bundled rates table with a user-supplied TOML file
    /// (same schema). Also honored via `ROBA_RATES_FILE`.
    #[arg(long, value_name = "PATH")]
    pub rates_file: Option<PathBuf>,

    /// Suppress dollar amounts (tokens only). Use when the bundled
    /// rates are stale and you don't want misleading numbers.
    #[arg(long)]
    pub no_dollars: bool,
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

/// Effort level for `--effort`. Controls the cost/quality tradeoff for
/// a call. Maps directly to `claude_wrapper::Effort`.
#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EffortLevel {
    /// Low effort: fast, cheap.
    Low,
    /// Medium effort.
    Medium,
    /// High effort (default when unset).
    High,
    /// Extra-high effort.
    Xhigh,
    /// Maximum effort: most thorough.
    Max,
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

/// `--permission-mode` choices for the CLI flag. Mirrors the modes
/// accepted by `claude -p --permission-mode`. Full layer support:
/// CLI > `ROBA_PERMISSION_MODE` env > profile `permission_mode` key.
///
/// Coexists with `--readonly` / `--writable` / `--full-auto`: those
/// flags control the `--allowedTools` list; `--permission-mode` is a
/// separate, additional claude mechanism.
#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum PermMode {
    /// Auto-accept file edits (`acceptEdits`).
    #[value(name = "acceptEdits", alias = "accept-edits")]
    AcceptEdits,
    /// Model-driven permission decisions (`auto`).
    Auto,
    /// Bypass all permission checks (`bypassPermissions`). Deprecated
    /// upstream; prefer `--full-auto` for this effect.
    #[value(name = "bypassPermissions", alias = "bypass-permissions")]
    BypassPermissions,
    /// Default interactive permissions (`default`).
    Default,
    /// Accept all allowed tools without prompting (`dontAsk`). Useful
    /// in non-interactive pipelines where tools are pre-approved via
    /// `--allow-tool` or a profile.
    #[value(name = "dontAsk", alias = "dont-ask")]
    DontAsk,
    /// Read-only plan mode: show what claude intends before executing
    /// (`plan`). Useful with `--writable` for a review step before
    /// write access is exercised.
    Plan,
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

    /// Output JSONL file paths only (one per line), suitable for shell composition.
    /// Optional N limits to the N most recent sessions. Implies --quiet.
    #[arg(long, value_name = "N", num_args = 0..=1, require_equals = false)]
    pub paths: Option<Option<usize>>,
}

#[derive(ClapArgs, Debug)]
pub struct AskArgs {
    // ----- Prompt sources ---------------------------------------------------
    /// Prompt text (positional). Pass `-` for explicit stdin.
    ///
    /// When the positional form is ambiguous against an optional-value
    /// flag (`-c`, `-w`), use `--prompt` / `-p` instead.
    #[arg(conflicts_with_all = ["file", "editor"])]
    pub prompt: Option<String>,

    /// Explicit prompt string (escape hatch for the ambiguous positional).
    ///
    /// Use when the positional form would be swallowed by an
    /// optional-value flag, e.g. `roba -c -p "..."` to continue the most
    /// recent session. Mutually exclusive with the positional `[PROMPT]`.
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

    /// Pre-fill the editor with the last N responses (with `-e`).
    ///
    /// Pulls the last N assistant responses from the most recent session
    /// in this dir, separated from your prompt by a scissors line.
    /// Default 1; pass 0 to disable. Only meaningful with `-e`.
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

    /// Write the spawned claude session's events to PATH as JSONL.
    ///
    /// A stable observability handle for in-flight runs that survives
    /// roba's exit. Forces the streaming pipeline internally even when
    /// `--stream` is not set.
    #[arg(long, value_name = "PATH", help_heading = "Output")]
    pub trace: Option<PathBuf>,

    /// Live TTY progress: stream tokens + inline tool-call lines.
    ///
    /// TTY-only; never load-bearing on a pipe. Conflicts with `--json`,
    /// `--code`, and `--out`.
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

    /// Override the bundled per-model rates table for the footer.
    ///
    /// Same TOML schema as `roba cost --rates-file`. Affects the per-call
    /// footer's dollar figure.
    #[arg(long, value_name = "PATH", help_heading = "Output")]
    pub rates_file: Option<PathBuf>,

    /// Omit the dollar figure from the per-call footer (tokens only).
    /// Use when the bundled rates are stale.
    #[arg(long, help_heading = "Output")]
    pub no_dollars: bool,

    // ----- Failure modes ----------------------------------------------------
    /// Disable wrapper-level auto-retry on transient failures.
    ///
    /// The caller gets the failure immediately and decides whether to
    /// retry, instead of roba re-trying with exponential backoff. No
    /// effect on success or non-transient failures.
    #[arg(long, help_heading = "Failure modes")]
    pub no_retry: bool,

    // ----- Mode -------------------------------------------------------------
    /// Minimal-overhead mode (skip hooks, LSP, memory, keychain, etc.).
    ///
    /// Skips hooks, LSP, plugin sync, CLAUDE.md auto-discovery,
    /// auto-memory, and keychain reads; auth then uses ANTHROPIC_API_KEY
    /// only. For agent-tier calls where context is supplied explicitly.
    #[arg(long, help_heading = "Mode")]
    pub bare: bool,

    // ----- Model ------------------------------------------------------------
    /// Override the claude model for this call.
    ///
    /// Accepts an alias (`sonnet`, `opus`, `haiku`) or a full model
    /// ID (`claude-sonnet-4-6`, `claude-opus-4-7`, etc.). Passed
    /// through to `claude -p --model`.
    #[arg(long, value_name = "MODEL", help_heading = "Model")]
    pub model: Option<String>,

    /// Effort level: the cost/quality tradeoff for this call.
    ///
    /// `low` is fast and cheap; `max` is most thorough.
    #[arg(long, value_name = "LEVEL", value_enum, help_heading = "Model")]
    pub effort: Option<EffortLevel>,

    // ----- System prompt ----------------------------------------------------
    /// Replace the default system prompt for this call.
    ///
    /// When combined with `--append-system-prompt`, the replace runs
    /// first and the append adds on top.
    #[arg(long, value_name = "TEXT", help_heading = "System prompt")]
    pub system_prompt: Option<String>,

    /// Append TEXT to the default system prompt for this call.
    ///
    /// When combined with `--system-prompt`, the replace runs first and
    /// this appends on top.
    #[arg(long, value_name = "TEXT", help_heading = "System prompt")]
    pub append_system_prompt: Option<String>,

    // ----- Sessions ---------------------------------------------------------
    /// Continue a session: bare `-c` = most recent here, `-c ID` = specific.
    ///
    /// The value is optional, so a space-separated word after `-c` is
    /// consumed as the id -- `roba -c "follow up"` treats "follow up" as
    /// the session id, not the prompt. To continue the most recent
    /// session with a prompt, use `roba -c -p "follow up"`.
    #[arg(
        short = 'c',
        long = "continue",
        num_args = 0..=1,
        value_name = "ID",
        help_heading = "Sessions"
    )]
    pub continue_session: Option<Option<String>>,

    /// Branch the resumed session instead of appending to it.
    ///
    /// Requires an explicit session id (`-c=ID --fork`); you can't fork
    /// "the most recent" without naming it.
    #[arg(long, requires = "continue_session", help_heading = "Sessions")]
    pub fork: bool,

    /// Interactive fuzzy chooser over recent sessions.
    #[arg(long, conflicts_with = "continue_session", help_heading = "Sessions")]
    pub pick: bool,

    /// Force a fresh session (cancel any auto-continue).
    ///
    /// Cancels a profile- or env-supplied `continue = true` -- the kill
    /// switch for accidental auto-continuation.
    #[arg(
        long,
        conflicts_with_all = ["continue_session", "pick"],
        help_heading = "Sessions"
    )]
    pub fresh: bool,

    /// Run in a fresh git worktree (optionally named: `-w NAME`).
    ///
    /// With no value claude generates the name; `-w NAME` (or `-w=NAME`)
    /// pins it. The value is optional, so a space-separated word after
    /// `-w` is consumed as the name -- to name a worktree and pass a
    /// prompt, use `roba -w NAME -p "..."`. The worktree persists after
    /// the session (clean up with `git worktree remove`); it pairs
    /// naturally with `--writable` / `--full-auto` as a sandbox.
    #[arg(
        short = 'w',
        long,
        value_name = "NAME",
        num_args(0..=1),
        help_heading = "Sessions"
    )]
    pub worktree: Option<Option<String>>,

    /// Pin a specific claude-code subagent for this run.
    ///
    /// The named subagent must exist in `.claude/agents/NAME.md` in the
    /// cwd (or be auto-discovered per claude's lookup). Lets an
    /// orchestrator dispatch as a known agent instead of default claude.
    #[arg(long, value_name = "NAME", help_heading = "Sessions")]
    pub agent: Option<String>,

    // ----- Permissions ------------------------------------------------------
    /// Set claude's own `--permission-mode`.
    ///
    /// A separate axis from `--readonly` / `--writable` / `--full-auto`
    /// (which set the allowed-tools list); setting both is valid -- e.g.
    /// `--writable --permission-mode plan` grants write access but
    /// requires a plan review first.
    #[arg(long, value_name = "MODE", value_enum, help_heading = "Permissions")]
    pub permission_mode: Option<PermMode>,

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

    /// Preview the resolved allow/deny set and exit (no claude call).
    ///
    /// Resolves permissions across all layers (CLI > env > profile >
    /// default) and prints the effective lists with per-entry
    /// provenance. Useful for verifying what a profile actually opens up.
    #[arg(long, help_heading = "Permissions")]
    pub show_permissions: bool,

    /// Skip the agent frontmatter permission check.
    ///
    /// With `--agent NAME`, roba parses the agent's `tools:` field and
    /// warns if any declared tool isn't in the resolved allowlist. This
    /// flag (or `--quiet` / `--full-auto`) suppresses that warning.
    #[arg(long, help_heading = "Permissions")]
    pub no_agent_check: bool,

    // ----- Dispatch ---------------------------------------------------------
    /// Preset for unattended file-mutating dispatch.
    ///
    /// Implies `--full-auto`, `--worktree`, and `--fresh` (individual
    /// flags override). For firing a worker agent that edits files
    /// without supervision. Warns to stderr if `--agent` is unset.
    #[arg(long, help_heading = "Dispatch")]
    pub dispatch: bool,

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
    /// Layer that set `permission_mode`.
    #[clap(skip)]
    pub permission_mode_source: Option<String>,

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
    fn permission_mode_parses_all_variants() {
        // camelCase is the canonical/documented form (mirrors claude's
        // native modes and the env + output layers); the kebab forms are
        // kept as back-compat aliases. Both must parse.
        use clap::Parser;
        for mode in &[
            // canonical camelCase
            "acceptEdits",
            "auto",
            "bypassPermissions",
            "default",
            "dontAsk",
            "plan",
            // kebab aliases
            "accept-edits",
            "bypass-permissions",
            "dont-ask",
        ] {
            let cli = Cli::try_parse_from(["roba", "--permission-mode", mode, "prompt"])
                .unwrap_or_else(|e| panic!("--permission-mode {mode} should parse: {e}"));
            assert!(
                cli.ask.permission_mode.is_some(),
                "--permission-mode {mode} should be Some"
            );
        }
    }

    #[test]
    fn permission_mode_coexists_with_writable() {
        // --permission-mode and --writable operate at different levels;
        // they must compose without a clap conflict error.
        use clap::Parser;
        let cli =
            Cli::try_parse_from(["roba", "--writable", "--permission-mode", "plan", "prompt"])
                .unwrap();
        assert!(cli.ask.writable);
        assert!(cli.ask.permission_mode.is_some());
    }

    #[test]
    fn permission_mode_coexists_with_readonly() {
        use clap::Parser;
        let cli = Cli::try_parse_from([
            "roba",
            "--readonly",
            "--permission-mode",
            "dont-ask",
            "prompt",
        ])
        .unwrap();
        assert!(cli.ask.readonly);
        assert!(cli.ask.permission_mode.is_some());
    }

    #[test]
    fn permission_mode_coexists_with_full_auto() {
        use clap::Parser;
        let cli =
            Cli::try_parse_from(["roba", "--full-auto", "--permission-mode", "plan", "prompt"])
                .unwrap();
        assert!(cli.ask.full_auto);
        assert!(cli.ask.permission_mode.is_some());
    }

    #[test]
    fn bare_parses_alone() {
        use clap::Parser;
        let cli = Cli::try_parse_from(["roba", "--bare", "prompt"]).unwrap();
        assert!(cli.ask.bare);
    }

    #[test]
    fn bare_is_orthogonal() {
        // --bare must compose freely with output flags
        use clap::Parser;
        let cli = Cli::try_parse_from(["roba", "--bare", "--quiet", "prompt"]).unwrap();
        assert!(cli.ask.bare);
        assert!(cli.ask.quiet);

        let cli = Cli::try_parse_from(["roba", "--bare", "--json", "prompt"]).unwrap();
        assert!(cli.ask.bare);
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

    #[test]
    fn dispatch_flag_parsed() {
        use clap::Parser;
        let cli = Cli::try_parse_from(["roba", "--dispatch", "do thing"]).unwrap();
        assert!(cli.ask.dispatch);
        assert_eq!(cli.ask.prompt.as_deref(), Some("do thing"));
    }

    #[test]
    fn dispatch_no_agent_parses() {
        // --dispatch without --agent parses cleanly at the clap layer;
        // the missing-agent warning is a runtime concern, not a parse error.
        use clap::Parser;
        let cli = Cli::try_parse_from(["roba", "--dispatch", "task"]).unwrap();
        assert!(cli.ask.dispatch);
        assert!(cli.ask.agent.is_none());
    }

    #[test]
    fn dispatch_with_agent_parses() {
        use clap::Parser;
        let cli =
            Cli::try_parse_from(["roba", "--dispatch", "--agent", "worker.md", "task"]).unwrap();
        assert!(cli.ask.dispatch);
        assert_eq!(cli.ask.agent.as_deref(), Some("worker.md"));
    }
}
