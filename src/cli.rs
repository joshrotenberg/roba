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
  cat err.log | roba \"what's wrong here?\"              pipe a log + ask about it
  roba --attach 'src/**/*.rs' \"audit error handling\"   attach files
  roba -c -p \"now add a test for that\"                 continue the last session
  roba --writable \"rename foo to bar in src/\"          let claude edit files

Full flag detail, env vars, and roba.toml config: roba --help";

/// Shown under `--help`: examples plus a self-contained reference for the
/// env-var and roba.toml config layers, so the binary documents itself
/// for humans and agents without depending on an external docs site.
const AFTER_LONG_HELP: &str = "\
Examples -- for humans (interactive, rich TTY):
  roba \"explain the borrow checker in 3 bullets\"      one-shot question
  cat err.log | roba \"what's wrong here?\"             pipe a log + ask about it
  roba --attach 'src/**/*.rs' \"audit error handling\"  attach files as context
  roba -e                                             compose in $EDITOR
  roba -c -p \"now add a test for that\"                continue the last session

Examples -- for agents & scripts (deterministic, pipe-clean):
  roba --json \"list 3 risks\" | jq -r '.result.result'  structured output -> jq
  roba -q \"one-line summary\" > out.txt                 answer only, no metadata
  roba --no-retry \"...\"; echo \"exit=$?\"                typed exit codes
  roba --session ci-bot \"follow up\"                    resume a named session
  roba --full-auto -C repo -f task.md                  fire an unattended worker
  Exit codes: 0 ok (refusals included), 1 failure, 2 auth, 3 budget
  (wrapper tracker), 4 timeout, 5 max-turns (recoverable -- finish the
  lifecycle), 6 no usable output (empty/is_error result, or a streaming run
  with no result event), 7 max-budget cap (recoverable -- finish the
  lifecycle). Under --json, codes 1-5 and 7 emit a structured error
  envelope on stderr; code 6 never does -- the signal is
  the exit code (the success envelope is on stdout for empty/is_error;
  stdout is empty for the no-result-event case). Branch on $?.
  Unattended / CI recipe (--json + --bare + the cap trio + --trace, with
  exit-code branching): see the README \"For agents & scripts\" section.

Unattended workers (composing the primitives):
  --full-auto -C <dir> -f <file>   edit the current checkout in place; the
                                   orchestrator owns the branch and PR (-C
                                   chdirs first, so -f resolves inside <dir>)
  ...add --worktree                run in an isolated git worktree, for
                                   parallel same-repo workers that must not
                                   share a branch
  git worktree add; roba -C <dir>  own the branch you'll PR from, for the
                                   orchestrator-owns-the-branch case; roba's
                                   --worktree makes a claude-managed worktree
                                   instead
  --detach -C <dir> -f <file>      fire a run that survives the caller;
                                   prints the session handle (re-attach:
                                   roba show <id> --wait)

Environment variables:
  Most long flags have a ROBA_<FLAG> override (uppercased, '-' -> '_'):
  --model -> ROBA_MODEL; bool flags take a truthy value (--writable ->
  ROBA_WRITABLE=1; falsy values are ignored -- env can only enable, never
  disable). One-shot flags (-p, -f, -e, --code, --out, --fork, --pick,
  --fresh, --show-permissions, -C) have no env form. Special cases:
    ROBA_PROFILE=NAME      apply a profile (like --profile NAME)
    ROBA_VAR_<KEY>=VALUE   set a template var (like --var KEY=VALUE)
    ROBA_CONTINUE=1|ID     truthy = continue most recent; else a session id
    ROBA_WORKTREE=1|NAME   truthy = fresh anonymous worktree; else a name
    ROBA_RATES_FILE=PATH   override the rates table (footer + roba cost)
    ROBA_STATE_DIR=PATH    roba's disposable state (detached-run receipts
                           land in <PATH>/runs; default $XDG_STATE_HOME/roba
                           or ~/.local/state/roba)
    NO_COLOR=1             disable color (help and answer rendering)
  Precedence: CLI > ROBA_* env > profile > top-level keys > built-in default.

Configuration (roba.toml):
  Most flags are also keys under [profile.NAME] or at the top level of a
  roba.toml -- discovered by walking up from the cwd, plus
  ~/.config/roba.toml. Closer-to-cwd files win; a `default` profile
  auto-applies. [alias.NAME] defines shortcut verbs; [session] binds
  NAME = \"uuid\" handles for --session NAME. roba-config.sample.toml
  (written by `roba profile init`) lists every valid key. See the
  `roba profile` and `roba alias` subcommands.";

/// The blurb shown when `roba` is run with no resolvable prompt on a TTY.
///
/// The header is single-sourced from `CARGO_PKG_DESCRIPTION` so it stays
/// consistent with the `about` line shown by `-h`/`--help`, and the
/// examples from `AFTER_HELP` so they never drift. `No prompt given.` sits
/// on its own line so it reads as the error it is, not as part of the
/// tagline.
pub(crate) fn no_prompt_blurb() -> String {
    format!(
        "{}\n\nNo prompt given.\n\n{AFTER_HELP}",
        env!("CARGO_PKG_DESCRIPTION")
    )
}

/// Build the styled `--help` trailer from [`AFTER_LONG_HELP`].
///
/// Section header lines (column 0, ending in `:`) get the header style
/// (green-bold, matching clap's own `Options:` headers) and the command
/// column of two-column rows gets the literal style (cyan), so the trailer
/// matches the rest of `--help` instead of rendering flat.
///
/// The styling is pushed as ANSI spans into a [`clap::builder::StyledStr`],
/// which clap accumulates into the full help and strips alongside everything
/// else on a non-TTY / under `NO_COLOR` -- so the agent-ABI output stays
/// byte-clean (the #181 discipline). [`AFTER_LONG_HELP`] stays the single
/// plain source of the content; only the long `--help` is styled. The short
/// `-h` trailer ([`AFTER_HELP`]) and the no-prompt blurb keep the plain
/// const, since the blurb prints via `eprintln!` -- not clap's color
/// pipeline -- and would leak ANSI on a pipe.
fn after_long_help_styled() -> clap::builder::StyledStr {
    use std::fmt::Write as _;

    let header = STYLES.get_header();
    let literal = STYLES.get_literal();
    let mut out = clap::builder::StyledStr::new();

    for (i, line) in AFTER_LONG_HELP.lines().enumerate() {
        if i > 0 {
            out.push_str("\n");
        }
        if is_section_header(line) {
            let _ = write!(out, "{}{line}{}", header.render(), header.render_reset());
        } else if let Some((indent, command, rest)) = split_two_column(line) {
            let _ = write!(
                out,
                "{indent}{}{command}{}{rest}",
                literal.render(),
                literal.render_reset()
            );
        } else {
            out.push_str(line);
        }
    }
    out
}

/// A section header is a non-empty, column-0 line ending in `:`
/// (e.g. `Examples:`, `Configuration (roba.toml):`).
fn is_section_header(line: &str) -> bool {
    !line.is_empty() && !line.starts_with(char::is_whitespace) && line.ends_with(':')
}

/// Split a two-column row into `(indent, command, rest)`, where `command` is
/// the left token and `rest` is the gap-plus-description. A row qualifies
/// when, after its leading indent, a run of 2+ spaces separates a command
/// from a description. Prose and wrapped continuation lines (no interior 2+
/// space gap) return `None` and stay plain.
fn split_two_column(line: &str) -> Option<(&str, &str, &str)> {
    let indent_len = line.len() - line.trim_start().len();
    if indent_len == 0 {
        return None;
    }
    let (indent, body) = line.split_at(indent_len);
    let gap = body.find("  ")?;
    if gap == 0 {
        return None;
    }
    Some((indent, &body[..gap], &body[gap..]))
}

/// A sharp, focused sugaring of claude -p -- pipeable, composable, safe-by-default, session-re-enterable.
#[derive(Parser, Debug)]
#[command(
    version,
    about,
    long_about = None,
    after_help = AFTER_HELP,
    after_long_help = after_long_help_styled(),
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
    /// List recent sessions (current project by default).
    History(HistoryArgs),
    /// Reprint the most recent session's last answer.
    Last(LastArgs),
    /// Inspect or initialize the user profiles config.
    Profile {
        #[command(subcommand)]
        action: ProfileAction,
    },
    /// Roll up token usage across session history.
    ///
    /// As of 2026-06-15 Anthropic meters programmatic usage (claude -p /
    /// Agent SDK) separately from interactive Claude. Every roba call is
    /// programmatic by construction, so these figures draw from that
    /// programmatic allotment, not your interactive limit.
    Cost(CostArgs),
    /// Diagnose the claude boundary: binary, auth, config, rates.
    ///
    /// Runs a series of health checks and prints a pass/warn/fail line
    /// for each. Exit codes: 0 = no check failed (warnings are allowed
    /// and do not fail); 1 = at least one check failed. The same code is
    /// returned in both human and `--json` modes. Never calls claude
    /// with a prompt -- only `claude --version`.
    ///
    /// `--json` emits the uniform `{ version: 1, result: { checks,
    /// overall } }` envelope.
    Doctor(DoctorArgs),
    /// Inspect user-defined aliases (`[alias.NAME]` in roba.toml).
    Alias {
        #[command(subcommand)]
        action: AliasAction,
    },
    /// Inspect personas: role-bearing profiles (`[profile.NAME]` with `agent`).
    ///
    /// A persona is a profile that pins a claude agent (the role) plus its run
    /// envelope; `list` shows the role-bearing profiles, `show NAME` prints one
    /// and locates its agent file. Read-only. See also `roba profile` and
    /// `roba config explain`.
    Persona {
        #[command(subcommand)]
        action: PersonaAction,
    },
    /// Launch the roba-server MCP server, optionally as a persona.
    ///
    /// Resolves `--profile NAME` from the config pool (top-level defaults +
    /// `[profile.NAME]`) and maps the persona onto the `ROBA_*` env the server
    /// reads, then execs the `roba-server` binary (found next to `roba`).
    /// Existing `ROBA_*` env wins over the profile (env > profile). Without
    /// `--profile`, launches the server with the current environment.
    Serve(ServeArgs),
    /// Bootstrap a project roba.toml (claude-assisted).
    ///
    /// The per-project half of the config-draft verbs: `init` looks at
    /// the current project and drafts a whole starter roba.toml fitted
    /// to it. Sibling to the per-block `roba profile draft` / `roba alias
    /// draft`, which target your user config.
    Config {
        #[command(subcommand)]
        cmd: ConfigCmd,
    },
    /// Inspect the git worktrees for this repo (read-only).
    ///
    /// Read-only inspection only: `list` enumerates every git worktree
    /// the repo knows about. It does NOT create, prune, or remove
    /// worktrees -- that's git's (or claude's) job.
    Worktree {
        #[command(subcommand)]
        cmd: WorktreeCmd,
    },
    /// Show a stored session's result (read-only).
    ///
    /// Reconstructs a result from a session's on-disk JSONL: the answer
    /// is the last assistant message, found by id across all projects.
    /// Read-only -- it reads the session log and reports; it never writes
    /// under `.claude/`.
    ///
    /// The `--json` envelope is RECONSTRUCTED, not replayed: it is
    /// structurally identical to a live `roba --json` envelope but NOT
    /// byte-identical. `duration_ms` is always null (claude does not
    /// persist per-run wall time) and `cost_usd` / `num_turns` are
    /// DERIVED from the log (a token rollup and a count of assistant
    /// turns), not the original run's reported values.
    Show(ShowArgs),
    /// Generate a shell completion script (bash, zsh, fish, ...).
    ///
    /// Prints the script for SHELL to stdout; pipe or redirect it into
    /// the right place for your shell. Examples:
    ///   roba completions zsh  > ~/.zfunc/_roba
    ///   roba completions bash > /etc/bash_completion.d/roba
    #[command(verbatim_doc_comment)]
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
pub enum PersonaAction {
    /// List personas: profiles that pin an `agent` (the role).
    List,
    /// Show one persona's `[profile.NAME]` block and its resolved agent file.
    Show {
        /// Persona name (the `[profile.NAME]` whose `agent` is set).
        name: String,
    },
}

#[derive(ClapArgs, Debug)]
pub struct ServeArgs {
    /// Launch the server as this persona (a role-bearing `[profile.NAME]`).
    ///
    /// The profile is resolved from the config pool (top-level defaults +
    /// `[profile.NAME]`) and mapped onto the `ROBA_*` env the server reads.
    #[arg(long, visible_alias = "persona")]
    pub profile: Option<String>,
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
    /// Draft a new alias from a plain-language description.
    ///
    /// Sends DESCRIPTION to claude with the bundled alias schema, parses
    /// the result with roba's real deserializer (a hallucinated key is
    /// rejected exactly as a hand-written config would be), and prints
    /// the canonical `[alias.NAME]` block on stdout -- byte-clean, so it
    /// pipes straight to `>> roba.toml`. Everything else (collision
    /// warnings, where-it-wrote) goes to stderr. There is NO retry loop:
    /// an invalid draft fails loud with the deserializer error.
    Draft(AliasDraftArgs),
}

#[derive(ClapArgs, Debug)]
pub struct AliasDraftArgs {
    /// Plain-language description of the verb you want.
    pub description: String,

    /// Append the drafted block to a config file instead of only printing.
    ///
    /// Bare `--write` targets your user config (`~/.config/roba.toml`);
    /// `--write PATH` targets an explicit file (created if absent). A
    /// duplicate `[alias.NAME]` already in the target is a hard error --
    /// it would break the next config load. The block still prints on
    /// stdout in either case.
    #[arg(long, num_args = 0..=1, value_name = "PATH")]
    pub write: Option<Option<PathBuf>>,

    /// Model override for the generation call (alias or full id).
    #[arg(long, value_name = "NAME")]
    pub model: Option<String>,
}

#[derive(Subcommand, Debug)]
pub enum WorktreeCmd {
    /// List the git worktrees for this repo.
    ///
    /// Shells (via claude-wrapper) to `git worktree list --porcelain`
    /// and prints a row per worktree: path, branch (or `(detached)` /
    /// `(bare)`), short HEAD, and markers (`[main]`, `[locked]`,
    /// `[prunable]`). This is the full git view -- a SUPERSET of the
    /// worktrees claude's `--worktree` flag creates -- so it includes
    /// user-made worktrees too, not just claude's.
    List(WorktreeListArgs),
}

#[derive(ClapArgs, Debug)]
pub struct WorktreeListArgs {
    /// Emit JSON instead of a human table.
    ///
    /// Output is the uniform `{ version: 1, result: [worktrees] }` envelope.
    #[arg(long)]
    pub json: bool,
}

#[derive(ClapArgs, Debug)]
pub struct ShowArgs {
    /// Session id (UUID) to show. Located across all projects.
    ///
    /// `read_session` walks every project directory looking for
    /// `<id>.jsonl`, so no project-scoping flag is needed.
    ///
    /// EXIT CODE: for a run roba detached, `show` exits with that run's
    /// own typed exit code (it reads the run receipt the detached child
    /// wrote), so `roba show <ID> || handle_failure` works. This applies
    /// to plain `show`, not just `--wait`. Any run roba did not detach has
    /// no receipt and `show` exits 0 as it always has.
    pub session_id: String,

    /// Also print the per-model token + cost breakdown for the session.
    ///
    /// The breakdown goes to stderr (so a `--json` stdout stays a clean
    /// envelope). Costs come from the bundled rate table; an uncosted
    /// model shows `-`.
    #[arg(long)]
    pub metrics: bool,

    /// Emit the reconstructed success envelope as JSON.
    ///
    /// Structurally identical to a live `roba --json` envelope but NOT
    /// byte-identical: `duration_ms` is null and `cost_usd` / `num_turns`
    /// are derived from the log.
    #[arg(long)]
    pub json: bool,

    /// Poll until the session finishes, then render its result.
    ///
    /// Bare `show` errors immediately if the session isn't found yet.
    /// With `--wait`, roba instead polls the session's on-disk JSONL
    /// (~1s interval) and renders once the run completes -- and treats a
    /// not-yet-written session as "not started yet", waiting for it to
    /// appear rather than erroring. Composes with `--json` / `--metrics`
    /// (those render once complete).
    ///
    /// For a run roba detached, completion is EXACT: the detached child
    /// writes a run receipt carrying its typed exit code, and `show`
    /// prefers it. The receipt also makes `show` report the outcome --
    /// `is_error` and an `exit_code` field in the `--json` envelope -- and
    /// `show` exits with that same code, so a failed detached run no longer
    /// reconstructs as success. A run that died before claude persisted
    /// anything is reported from the receipt instead of waiting out the
    /// timeout.
    ///
    /// Without a receipt (any run roba did not detach) completion is a
    /// BEST-EFFORT heuristic over claude's session log (no explicit "done"
    /// marker is persisted): roba considers the run finished when the last
    /// assistant turn's `stop_reason` is terminal (`end_turn` /
    /// `stop_sequence` / `max_tokens`) rather than `tool_use`. It is not a
    /// guaranteed "done" event. Always bounded by `--timeout`.
    #[arg(long)]
    pub wait: bool,

    /// Cap on how long `--wait` polls, in seconds (default 600).
    ///
    /// On timeout, roba exits non-zero with a clean error (never a
    /// panic or unbounded hang). `0` waits indefinitely. Ignored without
    /// `--wait`.
    #[arg(long, value_name = "SECS")]
    pub timeout: Option<u64>,
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
    ///
    /// Output is the uniform `{ version: 1, result: <rollup> }` envelope.
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

#[derive(ClapArgs, Debug)]
pub struct DoctorArgs {
    /// Emit the check results as a `{ version, result }` JSON envelope.
    ///
    /// `result` is `{ checks: [{ name, status, message }], overall }`,
    /// where each status is `ok`/`warn`/`fail`. The exit code is the
    /// same as the human form (1 when any check fails).
    #[arg(long)]
    pub json: bool,

    /// Render without color (the same effect as `NO_COLOR` or a non-TTY
    /// stdout).
    ///
    /// The master color kill-switch for this view, matching `config
    /// explain`'s `--plain`. Has no effect under `--json` (that output is
    /// always byte-plain).
    #[arg(long)]
    pub plain: bool,
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
    /// Draft a new profile from a plain-language description.
    ///
    /// Sends DESCRIPTION to claude with the bundled profile schema, parses
    /// the result with roba's real deserializer (a hallucinated key is
    /// rejected exactly as a hand-written config would be), and prints
    /// the canonical `[profile.NAME]` block on stdout -- byte-clean, so it
    /// pipes straight to `>> roba.toml`. Everything else (collision
    /// warnings, where-it-wrote) goes to stderr. There is NO retry loop:
    /// an invalid draft fails loud with the deserializer error.
    Draft(ProfileDraftArgs),
}

#[derive(ClapArgs, Debug)]
pub struct ProfileDraftArgs {
    /// Plain-language description of the profile you want.
    pub description: String,

    /// Append the drafted block to a config file instead of only printing.
    ///
    /// Bare `--write` targets your user config (`~/.config/roba.toml`);
    /// `--write PATH` targets an explicit file (created if absent). A
    /// duplicate `[profile.NAME]` already in the target is a hard error --
    /// it would break the next config load. The block still prints on
    /// stdout in either case.
    #[arg(long, num_args = 0..=1, value_name = "PATH")]
    pub write: Option<Option<PathBuf>>,

    /// Model override for the generation call (alias or full id).
    #[arg(long, value_name = "NAME")]
    pub model: Option<String>,
}

#[derive(Subcommand, Debug)]
pub enum ConfigCmd {
    /// Draft a starter project roba.toml from the current project.
    ///
    /// Makes ONE claude call with a read-only (Read/Glob/Grep) view of
    /// the cwd, so it can skim the README / manifest / layout and fit a
    /// starter config to what it sees: conservative top-level defaults, a
    /// couple of profiles, maybe an alias or two -- every key commented.
    /// The whole drafted file is validated by roba's REAL config
    /// deserializer (a hallucinated key is rejected exactly as a
    /// hand-written config would be) before it is shown.
    ///
    /// stdout is the validated file content ONLY (pipe it to
    /// `> roba.toml`); collision/where-it-wrote notes go to stderr. There
    /// is NO retry loop: an invalid draft fails loud with the deserializer
    /// error and the raw output.
    Init(ConfigInitArgs),
    /// Static checks over roba config (read-only).
    ///
    /// Runs every STATICALLY-knowable check over the discovered config
    /// pool (default) or a single PATH, and reports findings with a typed
    /// exit: 0 when no ERROR findings (warnings are advisory and pass), 1
    /// on any error. For each file it parses via roba's real deserializer
    /// (a parse error is itself a finding), then flags built-in-shadowing
    /// aliases, pinned agents that don't resolve, and (best-effort) a
    /// pinned agent whose declared tools exceed the posture the entry's own
    /// flags would grant. As an advisory WARNING, it also flags a top-level
    /// `worktree`/`full_auto` -- a task-scoped unsafe setting that belongs in a
    /// named `[profile.NAME]`, not auto-applied to every run.
    ///
    /// Honest limits: lint-clean now does not guarantee warning-free at
    /// run time elsewhere -- agent files, the surrounding pool, and env
    /// differ by machine, and `$(...)` in aliases evaluates at expansion.
    /// The linter is a tripwire, not a proof.
    Lint(ConfigLintArgs),
    /// Show the merged config for the cwd (read-only).
    ///
    /// Default (the STRUCTURAL view): prints the whole config pool -- your
    /// user config plus every `roba.toml` walking up to the git root,
    /// closer-to-cwd winning -- merged into one canonical roba.toml: the
    /// top-level defaults first, then every `[profile.NAME]`,
    /// `[alias.NAME]`, and a `[session]` table when non-empty. The "what
    /// you'd have if the whole pool were one file" view, so you stop
    /// merging several files in your head.
    ///
    /// `--sources` (the EFFECTIVE view): instead prints the final
    /// top-level knob values a bare `roba "..."` in this cwd would start
    /// from, after collapsing every config layer (each file's top-level
    /// keys, then the auto-applied profile, then `ROBA_*` env), with each
    /// line annotated by the layer that won it. `--sources KEY` narrows it
    /// to a single key (the "why is worktree on?" answer). Only keys some
    /// layer actually set are shown.
    ///
    /// stdout is the body only (byte-clean, re-parseable); a short header
    /// naming the auto-applied profile and the source files goes to
    /// stderr. `--json` emits the uniform `{ version: 1, result }`
    /// envelope on stdout instead.
    Show(ConfigShowArgs),
    /// Explain the merged config in a human-readable layout (read-only).
    ///
    /// The human counterpart to `config show`. Where `show` prints raw,
    /// re-parseable TOML (the machine / redirect view), `explain` renders a
    /// grouped, annotated narrative: the auto-applied profile, the
    /// always-on top-level defaults (each with the one-line "what it does"
    /// drawn from `--help`), the opt-in `[profile.NAME]` overlays (with
    /// unsafe settings like `full_auto` / `worktree` flagged), the alias verbs
    /// with their invocation form, and the source files.
    ///
    /// Color-rendered on a TTY; `--plain` (or `NO_COLOR`, or a non-TTY
    /// stdout) renders it uncolored. This is a human view only -- it is
    /// never what a script parses; `show` and `--json` own that.
    Explain(ConfigExplainArgs),
}

#[derive(ClapArgs, Debug)]
pub struct ConfigLintArgs {
    /// Lint a single named config file instead of the discovered pool.
    ///
    /// With no PATH, every `roba.toml` in the walk (plus your user
    /// config) is checked. With a PATH, only that file is checked.
    pub path: Option<PathBuf>,

    /// Emit JSON instead of a human findings list.
    ///
    /// Output is the uniform `{ version: 1, result: { findings, ok } }`
    /// envelope; each finding carries a `severity` (`error`/`warning`).
    /// Exit is 0 when no error findings (warnings pass), 1 on any error --
    /// in both modes.
    #[arg(long)]
    pub json: bool,

    /// Render without color (the same effect as `NO_COLOR` or a non-TTY
    /// stdout).
    ///
    /// The master color kill-switch for this view, matching `config
    /// explain`'s `--plain`. Has no effect under `--json` (that output is
    /// always byte-plain).
    #[arg(long)]
    pub plain: bool,
}

#[derive(ClapArgs, Debug)]
pub struct ConfigShowArgs {
    /// Show the effective top-level config with per-key provenance.
    ///
    /// Bare `--sources` prints every set top-level knob as
    /// `key = value  # <source>`, where the source names the layer that
    /// won it (a specific `roba.toml`, the auto-applied `[profile.NAME]`,
    /// or `env (ROBA_X)`). `--sources KEY` prints just that one key (and
    /// reports to stderr when KEY is set by no layer). Without this flag,
    /// `config show` prints the structural merged-pool view instead.
    #[arg(long, num_args = 0..=1, value_name = "KEY")]
    pub sources: Option<Option<String>>,

    /// Emit the merged config as JSON instead of canonical TOML.
    ///
    /// Output is the uniform `{ version: 1, result }` envelope. For the
    /// default view `result` carries the active profile, the source files,
    /// the merged defaults, and the merged profile/alias/session maps. With
    /// `--sources` it carries `{ effective: { KEY: { value, source } } }`.
    /// stdout only.
    #[arg(long)]
    pub json: bool,
}

#[derive(ClapArgs, Debug)]
pub struct ConfigExplainArgs {
    /// Render without color (the same effect as `NO_COLOR` or a non-TTY
    /// stdout).
    ///
    /// `explain` is a human view; this only turns off ANSI color. It does
    /// NOT fall back to the raw TOML dump -- that is what `config show` is
    /// for.
    #[arg(long)]
    pub plain: bool,
}

#[derive(ClapArgs, Debug)]
pub struct ConfigInitArgs {
    /// Optional steer for the draft (e.g. "focus on PR review").
    ///
    /// Woven into the generation prompt on top of what the call observes
    /// in the project. Omit it to let the project itself drive the draft.
    pub description: Option<String>,

    /// Write the drafted file instead of only printing it.
    ///
    /// Bare `--write` targets `./roba.toml` (the PROJECT file -- this is
    /// the per-project verb); `--write PATH` targets an explicit file.
    /// REFUSES if the target already exists -- a whole-file verb must
    /// never clobber or append to an existing config; print to stdout and
    /// merge by hand instead. The file still prints on stdout either way.
    #[arg(long, num_args = 0..=1, value_name = "PATH")]
    pub write: Option<Option<PathBuf>>,

    /// Model override for the generation call (alias or full id).
    #[arg(long, value_name = "NAME")]
    pub model: Option<String>,
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

    /// Emit JSON instead of rendered items.
    ///
    /// Output is the uniform `{ version: 1, result: [items] }` envelope,
    /// matching `history --json`. Each item is content-block shaped:
    /// `{ "type": "text", "text": ... }` for an answer,
    /// `{ "type": "tool_use", "name": ..., "input": ... }` for a tool
    /// call. Byte-clean on stdout (no ANSI). An empty result (no
    /// matching session or no items of the requested `--type`) is a
    /// `{ version: 1, result: [] }` envelope and exit 0.
    #[arg(long)]
    pub json: bool,
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

/// Which axes `--hermetic` seals. `both` (the default) seals ambient roba config
/// AND ambient claude config; `roba` or `claude` seals just one.
#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum HermeticWhich {
    /// Seal both axes (default): roba's config pool and claude's ambient config.
    Both,
    /// Seal only roba's own config (skip the roba.toml pool + `~/.config`).
    Roba,
    /// Seal only claude's ambient config (`--setting-sources` + strict MCP).
    Claude,
}

impl EffortLevel {
    /// Lowercase label for display (footer suffix) and wiring.
    pub fn as_str(self) -> &'static str {
        match self {
            EffortLevel::Low => "low",
            EffortLevel::Medium => "medium",
            EffortLevel::High => "high",
            EffortLevel::Xhigh => "xhigh",
            EffortLevel::Max => "max",
        }
    }
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
    ///
    /// Output is the uniform `{ version: 1, result: [sessions] }` envelope.
    #[arg(long)]
    pub json: bool,

    /// Output JSONL file paths only (one per line), suitable for shell
    /// composition. Optional N limits to the N most recent sessions; without
    /// N every session's path prints (`-n`/`--all` are ignored). Nothing but
    /// paths is printed.
    #[arg(long, value_name = "N", num_args = 0..=1, require_equals = false)]
    pub paths: Option<Option<usize>>,

    /// Only show sessions that ran in a given git worktree.
    ///
    /// Matches sessions whose working directory is under
    /// `.claude/worktrees/<NAME>` -- the runner-worktree convention that
    /// `roba worktree list` enumerates. Use it to find a dispatched
    /// runner's session to resume with `-c ID`.
    ///
    /// Worktree sessions live under their own project slug (distinct from
    /// the base repo's), so this implies a cross-project scan and is
    /// orthogonal to `--project` (when both are given, `--worktree` wins).
    /// Only the most recent sessions are scanned; a note is printed if
    /// that scan cap is reached.
    #[arg(long, value_name = "NAME")]
    pub worktree: Option<String>,
}

#[derive(ClapArgs, Debug, Default)]
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
    /// recent session. Mutually exclusive with the positional `[PROMPT]`,
    /// `-f`, and `-e`.
    #[arg(
        short = 'p',
        long = "prompt",
        value_name = "TEXT",
        conflicts_with_all = ["prompt", "file", "editor"],
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
    ///
    /// The value is optional, so a space-separated word after `--git-log`
    /// is consumed as N -- a non-numeric next token (e.g. a prompt) fails
    /// loud as a usage error. To pass a prompt, put it before the flag or
    /// use `-p`.
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

    /// Constrain the model's output to a JSON Schema (path to a `.json` file).
    ///
    /// Passes claude's own `--json-schema` through for schema-validated
    /// structured output. The argument is a PATH to a file containing the
    /// JSON Schema -- roba reads it and inlines the contents (claude's flag
    /// takes inline JSON, which is hostile to type on a CLI, so the path is
    /// the ergonomic roba sugar). roba validates the file is parseable JSON
    /// and surfaces a clean error envelope if it is missing or malformed.
    ///
    /// The validated answer arrives as claude's `structured_output`. On the
    /// default path roba renders it as pretty-printed JSON to stdout so it
    /// stays pipeable -- `roba --json-schema s.json "..." | jq` sees the
    /// object -- even when claude also returns a prose `result` alongside
    /// the structured answer. With `--json` the structured output nests
    /// under `.result.structured_output` in the envelope instead. Composes
    /// with `--stream` (StreamJson is still a JSON format).
    #[arg(long, value_name = "PATH", help_heading = "Output")]
    pub json_schema: Option<String>,

    /// Print only fenced code blocks (optional language filter).
    ///
    /// The value is optional, so a space-separated word after `--code` is
    /// consumed as the LANG filter -- `roba --code "Write a fn..."` treats
    /// the prompt as the language and then has no prompt left. To filter by
    /// language and pass a prompt, use `roba --code rust -p "..."`.
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
    ///
    /// The lines are claude's own stream-json events written verbatim (one
    /// JSON object per line). The format is claude-code's, not part of
    /// roba's versioned `--json` ABI, and may change with claude versions;
    /// only the path is stable.
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

    /// Render extended-thinking blocks live on stderr. Only takes effect
    /// with `--stream` (rendered live) or `--trace` (events land in the
    /// trace); ignored otherwise.
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

    // ----- Limits -----------------------------------------------------------
    /// Cap the agentic turn count for this run (unattended guardrail).
    ///
    /// Passes claude's own `--max-turns N` through. When the cap is hit
    /// the run errors rather than continuing -- a rail for unattended /
    /// Ralph loops that would otherwise run unbounded. claude-wrapper
    /// detects the turn-cap result and surfaces it as the recoverable
    /// exit code 5 (distinct from the generic failure 1), so an unattended
    /// caller can resume the session and finish the lifecycle.
    #[arg(long, value_name = "N", help_heading = "Limits")]
    pub max_turns: Option<u32>,

    /// Cap total spend in USD for this run (unattended guardrail).
    ///
    /// Passes claude's own `--max-budget-usd USD` through. When the cap
    /// is hit the run errors. This is claude's CLI-side ceiling, a
    /// different mechanism from the wrapper's own budget tracker (exit 3).
    /// claude-wrapper detects the cap result and surfaces it as the
    /// recoverable exit code 7 (distinct from the generic failure 1), so an
    /// unattended caller can resume the session and finish the lifecycle,
    /// the same as 5 max-turns.
    #[arg(long, value_name = "USD", help_heading = "Limits")]
    pub max_budget_usd: Option<f64>,

    /// Wall-clock deadline for this run, in seconds.
    ///
    /// Bounds total elapsed time regardless of turns or spend: if the
    /// run is still going when the deadline passes, roba kills the
    /// claude child and exits the timeout code (4) with a clean stderr
    /// message. This is the rail for a headless `claude -p` that hangs
    /// making no progress, where `--max-turns` / `--max-budget-usd`
    /// (which bound work, not time) never trip. Composes with both as an
    /// independent cap. `0` disables (no deadline).
    #[arg(long, value_name = "SECS", help_heading = "Limits")]
    pub timeout: Option<u64>,

    // ----- Mode -------------------------------------------------------------
    /// Minimal-overhead mode (skip hooks, LSP, memory, keychain, etc.).
    ///
    /// Skips hooks, LSP, plugin sync, CLAUDE.md auto-discovery,
    /// auto-memory, and keychain reads; auth then uses ANTHROPIC_API_KEY
    /// only. For agent-tier calls where context is supplied explicitly.
    #[arg(long, help_heading = "Mode")]
    pub bare: bool,

    /// Start with all claude customizations disabled (security posture).
    ///
    /// Passes claude's own `--safe-mode` through (sets
    /// `CLAUDE_CODE_SAFE_MODE=1`): CLAUDE.md, skills, plugins, hooks, MCP
    /// servers, and custom commands and agents are all turned off; only
    /// claude's built-in behavior runs. This is a SECURITY posture, not a
    /// reproducibility one -- it shrinks the custom-code surface an
    /// injected or untrusted prompt (e.g. an issue body fed to a
    /// `--full-auto` worker) could exploit. Pairs naturally with
    /// unattended / `--full-auto` runs over untrusted input. Distinct from
    /// `--bare` (minimal-overhead reproducibility) and composable with it.
    #[arg(long, help_heading = "Mode")]
    pub safe_mode: bool,

    // ----- Model ------------------------------------------------------------
    /// Override the claude model for this call.
    ///
    /// Accepts an alias (`sonnet`, `opus`, `haiku`) or a full model
    /// ID (`claude-sonnet-4-6`, `claude-opus-4-7`, etc.). Passed
    /// through to `claude -p --model`.
    #[arg(long, value_name = "MODEL", help_heading = "Model")]
    pub model: Option<String>,

    /// Fall back to this model if the primary is overloaded.
    ///
    /// Passes claude's own `--fallback-model` through. Accepts an alias
    /// (`sonnet`, `opus`, `haiku`) or a full model ID. When the primary
    /// `--model` is overloaded, claude retries the call on this model
    /// instead of failing -- a resilience knob for unattended runs.
    #[arg(long, value_name = "MODEL", help_heading = "Model")]
    pub fallback_model: Option<String>,

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

    /// Don't inject the built-in single-turn advisory into the system prompt.
    ///
    /// By default roba appends a short note orienting the agent to its
    /// `claude -p` execution context: one non-interactive turn, with no
    /// cross-turn background-completion notifications. This suppresses it.
    /// To change the text instead, use `--agent-notice` or the
    /// `agent_notice` config key. The notice composes with (never clobbers)
    /// your own `--append-system-prompt`.
    #[arg(long, help_heading = "System prompt")]
    pub no_agent_notice: bool,

    /// Replace the built-in single-turn advisory text.
    ///
    /// roba appends a short advisory to the system prompt by default (see
    /// `--no-agent-notice`). Set this to substitute your own text; an empty
    /// string injects nothing (a config-level disable).
    #[arg(long, value_name = "TEXT", help_heading = "System prompt")]
    pub agent_notice: Option<String>,

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

    /// Resume a session by its configured name (see `[session]` in roba.toml).
    ///
    /// Looks NAME up in the merged config pool and resumes the bound
    /// session id. Bind names yourself in an (untracked, local) roba.toml
    /// `[session]` table -- UUIDs are machine-local. Conflicts with -c/--pick/--fresh.
    #[arg(
        long,
        value_name = "NAME",
        conflicts_with_all = ["continue_session", "pick", "fresh"],
        help_heading = "Sessions"
    )]
    pub session: Option<String>,

    /// Assign a caller-chosen UUID to this (new) session.
    ///
    /// Passes claude's own `--session-id <uuid>` through. The value must
    /// be a valid UUID (claude validates it). Use it to mint a session
    /// with an id you control on the FIRST turn, then `-c=ID`
    /// that id on later turns -- the reliable scripted-multi-turn pattern,
    /// since `claude -p --continue` no-ops in print mode. Conflicts with
    /// the session selectors (`-c`/`--continue`, `--fork`, `--pick`,
    /// `--session`), since those resume an existing session rather than
    /// name a new one. Composes with `--fresh`.
    #[arg(
        long,
        value_name = "UUID",
        value_parser = parse_session_id,
        conflicts_with_all = ["continue_session", "fork", "pick", "session"],
        help_heading = "Sessions"
    )]
    pub session_id: Option<String>,

    /// Fire the run detached: print the session handle and exit, leaving
    /// the run to outlive this process.
    ///
    /// roba mints a v4 session UUID (unless `--session-id` was given),
    /// re-execs itself with the same arguments minus `--detach`, in a new
    /// process group with all stdio detached, and exits 0. The minted (or
    /// given) UUID is the ONLY thing printed to stdout -- the handle IS the
    /// answer, so `id=$(roba --detach ...)` captures it. A re-attach hint
    /// goes to stderr. Re-attach from any later shell:
    ///   id=$(roba --detach -C repo -f task.md --trace /tmp/t.jsonl)
    ///   roba show "$id" --wait --timeout 600
    ///
    /// This is `nohup` baked in, not a daemon: roba owns NOTHING after the
    /// spawn (no supervisor, no socket, no resume machinery). Observe the
    /// run with `roba show --wait` / `--trace`; its state lives in claude's
    /// own session records.
    ///
    /// Requires an explicit prompt source (positional / `-p` / `-f`): the
    /// detached child's stdin is /dev/null, so stdin that carries data
    /// (a pipe with bytes, a non-empty `< file` redirect) is rejected rather
    /// than silently lost, while a benign non-TTY stdin (a closed/empty pipe
    /// or /dev/null from an orchestrator) passes through. On unix the check
    /// peeks for actual bytes; on Windows it is conservative -- any pipe on
    /// stdin is rejected (a console or `NUL` stdin passes). roba also
    /// verifies the claude binary resolves
    /// BEFORE detaching -- a dead-on-arrival child behind a printed handle is
    /// just silence.
    ///
    /// CLI-only by design: detaching is a deliberate per-invocation act, so
    /// there is no `ROBA_DETACH` env var and no profile key (same as `-e`
    /// and `--pick`). Conflicts with the output-consuming / interactive
    /// flags that imply attachment (`--json`, `--stream`, `--code`,
    /// `--show-thinking`, `-e`, `--pick`); `-o`/`--out` and `--trace` stay
    /// allowed (the detached child writes them).
    #[arg(
        long,
        conflicts_with_all = ["json", "stream", "code", "show_thinking", "editor", "pick"],
        help_heading = "Sessions"
    )]
    pub detach: bool,

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

    /// Force worktree OFF for this run, overriding a config-set worktree.
    ///
    /// A config (top-level or profile) `worktree = true` otherwise applies
    /// to every run with no per-run escape -- `--worktree`/`-w` and
    /// `ROBA_WORKTREE` only turn it ON. This is that escape: it nulls any
    /// config/env-set worktree so the run uses the main checkout. Conflicts
    /// with `--worktree`/`-w`. Also settable via `ROBA_NO_WORKTREE` (truthy).
    #[arg(
        long = "no-worktree",
        conflicts_with = "worktree",
        help_heading = "Sessions"
    )]
    pub no_worktree: bool,

    /// Pin a specific claude-code subagent for this run.
    ///
    /// The named subagent must exist in `.claude/agents/NAME.md` in the
    /// cwd (or be auto-discovered per claude's lookup). Lets an
    /// orchestrator dispatch as a known agent instead of default claude.
    #[arg(long, value_name = "NAME", help_heading = "Sessions")]
    pub agent: Option<String>,

    /// Run without writing a session record to disk.
    ///
    /// Passes claude's own `--no-session-persistence` through. The run
    /// executes normally but leaves no resumable session on disk -- so it
    /// won't appear in `roba history` and can't be continued with
    /// `-c`/`-c=ID` later.
    /// For one-off, stateless calls where a session record is just noise.
    #[arg(long, help_heading = "Sessions")]
    pub no_session_persistence: bool,

    // ----- Permissions ------------------------------------------------------
    /// Set claude's own `--permission-mode`.
    ///
    /// A separate axis from `--readonly` / `--writable` (which set the
    /// allowed-tools list); setting both is valid -- e.g.
    /// `--writable --permission-mode plan` grants write access but
    /// requires a plan review first. `--full-auto` is the exception: it
    /// bypasses permission checks entirely, so a `--permission-mode`
    /// passed with it is ignored.
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

    /// Grant tool access to an additional directory (repeatable).
    ///
    /// Passes claude's own `--add-dir <DIR>` through, once per path. By
    /// default claude's file tools are scoped to the cwd; each `--add-dir`
    /// widens that scope to another directory. roba forwards the path
    /// verbatim -- claude resolves and reads it.
    #[arg(long = "add-dir", value_name = "DIR", help_heading = "Permissions")]
    pub add_dir: Vec<String>,

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
    /// Layer per `add_dir` entry (parallel-indexed to `add_dir`).
    #[clap(skip)]
    pub add_dir_sources: Vec<String>,
    /// Layer that set `permission_mode`.
    #[clap(skip)]
    pub permission_mode_source: Option<String>,

    // ----- MCP --------------------------------------------------------------
    /// Load MCP servers from a JSON config file for this run (repeatable).
    ///
    /// Passes claude's own `--mcp-config <FILE>` through, once per path. The
    /// servers in the file provide additional tools for the call. This is the
    /// THIN pass-through -- it points the existing `claude -p` at MCP server
    /// JSON for one run. It is NOT the headless MCP *server* ("roba serve",
    /// closed as a separate tool); don't confuse the two. roba forwards the
    /// path verbatim and never reads the file -- claude reads it.
    #[arg(long, value_name = "FILE", help_heading = "MCP")]
    pub mcp_config: Vec<String>,

    /// Use only the `--mcp-config` servers, ignoring all other MCP config.
    ///
    /// Passes claude's own `--strict-mcp-config` through. Without it, the
    /// `--mcp-config` servers are added on top of any MCP servers claude
    /// already has configured; with it, only the `--mcp-config` ones are used.
    #[arg(long, help_heading = "MCP")]
    pub strict_mcp_config: bool,

    /// Which ambient claude setting sources to load (`user,project,local`).
    ///
    /// A comma-separated whitelist passed straight to claude's
    /// `--setting-sources`. Restrict it to seal ambient config: e.g.
    /// `--setting-sources user` drops the project and local layers (including a
    /// project `CLAUDE.md`) while auth stays on your keychain/subscription. This
    /// seals the promptspace, not tool permissions. The claude half of hermetic
    /// mode.
    #[arg(long, value_name = "LIST", help_heading = "Hermetic")]
    pub setting_sources: Option<String>,

    /// Seal the run to a known promptspace (hermetic mode).
    ///
    /// `--hermetic` seals BOTH axes; `--hermetic=roba` skips roba's own config
    /// pool (the up-tree roba.toml walk + `~/.config`); `--hermetic=claude`
    /// seals claude's ambient config (`--setting-sources user` + strict MCP),
    /// dropping the project's ambient CLAUDE.md/agents while your global
    /// `~/.claude` stays. For a full seal add `--setting-sources ''`. Seals the
    /// promptspace, not tool permissions.
    #[arg(
        long,
        value_name = "WHICH",
        require_equals = true,
        num_args = 0..=1,
        default_missing_value = "both",
        conflicts_with = "no_hermetic",
        help_heading = "Hermetic"
    )]
    pub hermetic: Option<HermeticWhich>,

    /// Cancel a hermetic setting arriving from env for this run.
    #[arg(long, help_heading = "Hermetic")]
    pub no_hermetic: bool,

    /// Load config from a `.roba/` bundle directory.
    ///
    /// The bundle's `roba.toml` becomes a config source: the closest
    /// (highest-precedence) layer, or the SOLE config under `--hermetic`.
    /// `--hermetic` with no explicit `--bundle` auto-discovers `./.roba`.
    #[arg(long, value_name = "DIR", help_heading = "Hermetic")]
    pub bundle: Option<std::path::PathBuf>,

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
/// Validate `--session-id` as a UUID locally so a malformed value fails
/// fast at parse time (usage error, exit 2) instead of spawning a doomed
/// claude that dies with "Invalid session ID" (exit 1). Accepts any case;
/// the value is returned unchanged. claude still does the authoritative
/// check, but the common typo is caught before the spawn.
pub fn parse_session_id(s: &str) -> std::result::Result<String, String> {
    uuid::Uuid::try_parse(s)
        .map(|_| s.to_string())
        .map_err(|_| format!("not a valid UUID: `{s}`"))
}

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
    fn session_id_valid_uuid_parses() {
        let uuid = "550e8400-e29b-41d4-a716-446655440000";
        let cli = Cli::try_parse_from(["roba", "--session-id", uuid, "hi"]).unwrap();
        assert_eq!(cli.ask.session_id.as_deref(), Some(uuid));
    }

    #[test]
    fn session_id_uppercase_uuid_parses() {
        let uuid = "550E8400-E29B-41D4-A716-446655440000";
        let cli = Cli::try_parse_from(["roba", "--session-id", uuid, "hi"]).unwrap();
        assert_eq!(cli.ask.session_id.as_deref(), Some(uuid));
    }

    #[test]
    fn session_id_rejects_non_uuid_at_parse_time() {
        let err = Cli::try_parse_from(["roba", "--session-id", "not-a-uuid", "hi"]).unwrap_err();
        // Usage error (exit 2), not a successful parse that spawns claude.
        assert_eq!(err.kind(), clap::error::ErrorKind::ValueValidation);
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
    fn doctor_parses_without_json() {
        use clap::Parser;
        let cli = Cli::try_parse_from(["roba", "doctor"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(SubCommand::Doctor(DoctorArgs {
                json: false,
                plain: false
            }))
        ));
    }

    #[test]
    fn doctor_json_flag_parses() {
        use clap::Parser;
        let cli = Cli::try_parse_from(["roba", "doctor", "--json"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(SubCommand::Doctor(DoctorArgs {
                json: true,
                plain: false
            }))
        ));
    }

    #[test]
    fn doctor_plain_flag_parses() {
        use clap::Parser;
        let cli = Cli::try_parse_from(["roba", "doctor", "--plain"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(SubCommand::Doctor(DoctorArgs {
                json: false,
                plain: true
            }))
        ));
    }

    #[test]
    fn config_init_parses_without_description() {
        use clap::Parser;
        let cli = Cli::try_parse_from(["roba", "config", "init"]).unwrap();
        let Some(SubCommand::Config {
            cmd: ConfigCmd::Init(args),
        }) = cli.command
        else {
            panic!("expected config init");
        };
        assert!(args.description.is_none());
        assert!(args.write.is_none());
    }

    #[test]
    fn config_init_parses_with_description() {
        use clap::Parser;
        let cli = Cli::try_parse_from(["roba", "config", "init", "focus on PR review"]).unwrap();
        let Some(SubCommand::Config {
            cmd: ConfigCmd::Init(args),
        }) = cli.command
        else {
            panic!("expected config init");
        };
        assert_eq!(args.description.as_deref(), Some("focus on PR review"));
    }

    #[test]
    fn config_init_write_bare_is_some_none() {
        use clap::Parser;
        let cli = Cli::try_parse_from(["roba", "config", "init", "--write"]).unwrap();
        let Some(SubCommand::Config {
            cmd: ConfigCmd::Init(args),
        }) = cli.command
        else {
            panic!("expected config init");
        };
        // Bare --write => Some(None) (default ./roba.toml target).
        assert_eq!(args.write, Some(None));
    }

    #[test]
    fn config_init_write_with_path() {
        use clap::Parser;
        let cli =
            Cli::try_parse_from(["roba", "config", "init", "--write", "custom.toml"]).unwrap();
        let Some(SubCommand::Config {
            cmd: ConfigCmd::Init(args),
        }) = cli.command
        else {
            panic!("expected config init");
        };
        assert_eq!(
            args.write,
            Some(Some(std::path::PathBuf::from("custom.toml")))
        );
    }

    #[test]
    fn config_lint_no_path_no_json() {
        use clap::Parser;
        let cli = Cli::try_parse_from(["roba", "config", "lint"]).unwrap();
        let Some(SubCommand::Config {
            cmd: ConfigCmd::Lint(args),
        }) = cli.command
        else {
            panic!("expected config lint");
        };
        assert!(args.path.is_none());
        assert!(!args.json);
    }

    #[test]
    fn config_lint_with_path() {
        use clap::Parser;
        let cli = Cli::try_parse_from(["roba", "config", "lint", "some/roba.toml"]).unwrap();
        let Some(SubCommand::Config {
            cmd: ConfigCmd::Lint(args),
        }) = cli.command
        else {
            panic!("expected config lint");
        };
        assert_eq!(args.path, Some(std::path::PathBuf::from("some/roba.toml")));
        assert!(!args.json);
    }

    #[test]
    fn config_lint_json_flag() {
        use clap::Parser;
        let cli = Cli::try_parse_from(["roba", "config", "lint", "--json"]).unwrap();
        let Some(SubCommand::Config {
            cmd: ConfigCmd::Lint(args),
        }) = cli.command
        else {
            panic!("expected config lint");
        };
        assert!(args.json);
        assert!(args.path.is_none());
    }

    #[test]
    fn worktree_missing_is_none() {
        use clap::Parser;
        let cli = Cli::try_parse_from(["roba", "do thing"]).unwrap();
        assert!(cli.ask.worktree.is_none());
    }

    #[test]
    fn no_worktree_alone_parses() {
        use clap::Parser;
        let cli = Cli::try_parse_from(["roba", "--no-worktree", "do thing"]).unwrap();
        assert!(cli.ask.no_worktree);
        assert!(cli.ask.worktree.is_none());
    }

    #[test]
    fn no_worktree_conflicts_with_worktree() {
        use clap::Parser;
        // --worktree and --no-worktree are mutually exclusive.
        assert!(Cli::try_parse_from(["roba", "--worktree", "--no-worktree", "do thing"]).is_err());
        assert!(Cli::try_parse_from(["roba", "-w", "--no-worktree", "do thing"]).is_err());
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
    fn session_parses_with_name() {
        use clap::Parser;
        let cli = Cli::try_parse_from(["roba", "--session", "meta", "-p", "hi"]).unwrap();
        assert_eq!(cli.ask.session.as_deref(), Some("meta"));
        assert_eq!(cli.ask.prompt_flag.as_deref(), Some("hi"));
    }

    #[test]
    fn session_conflicts_with_continue() {
        use clap::Parser;
        assert!(Cli::try_parse_from(["roba", "--session", "meta", "-c", "-p", "hi"]).is_err());
    }

    #[test]
    fn session_conflicts_with_pick_and_fresh() {
        use clap::Parser;
        assert!(Cli::try_parse_from(["roba", "--session", "meta", "--pick"]).is_err());
        assert!(Cli::try_parse_from(["roba", "--session", "meta", "--fresh", "-p", "hi"]).is_err());
    }

    #[test]
    fn fresh_conflicts_with_continue() {
        use clap::Parser;
        assert!(Cli::try_parse_from(["roba", "--fresh", "-c", "prompt"]).is_err());
    }

    #[test]
    fn session_id_parses_alone() {
        use clap::Parser;
        let cli = Cli::try_parse_from([
            "roba",
            "--session-id",
            "11111111-1111-4111-8111-111111111111",
            "-p",
            "hi",
        ])
        .unwrap();
        assert_eq!(
            cli.ask.session_id.as_deref(),
            Some("11111111-1111-4111-8111-111111111111")
        );
        assert_eq!(cli.ask.prompt_flag.as_deref(), Some("hi"));
    }

    #[test]
    fn session_id_conflicts_with_resume() {
        // `-c=ID` is roba's resume form; assigning a new id while
        // resuming an existing one is contradictory.
        use clap::Parser;
        assert!(Cli::try_parse_from(["roba", "--session-id", "x", "-c=y", "-p", "hi"]).is_err());
    }

    #[test]
    fn session_id_conflicts_with_pick_and_session() {
        use clap::Parser;
        assert!(Cli::try_parse_from(["roba", "--session-id", "x", "--pick"]).is_err());
        assert!(
            Cli::try_parse_from(["roba", "--session-id", "x", "--session", "meta", "-p", "hi"])
                .is_err()
        );
    }

    #[test]
    fn json_schema_parses() {
        use clap::Parser;
        let cli =
            Cli::try_parse_from(["roba", "--json-schema", "/tmp/schema.json", "-p", "hi"]).unwrap();
        assert_eq!(cli.ask.json_schema.as_deref(), Some("/tmp/schema.json"));
    }

    #[test]
    fn session_id_composes_with_fresh() {
        // --fresh (cancel auto-continue) and --session-id (name the new
        // session) are not contradictory: both want a new session.
        use clap::Parser;
        let uuid = "550e8400-e29b-41d4-a716-446655440000";
        let cli =
            Cli::try_parse_from(["roba", "--session-id", uuid, "--fresh", "-p", "hi"]).unwrap();
        assert_eq!(cli.ask.session_id.as_deref(), Some(uuid));
        assert!(cli.ask.fresh);
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
        // clap's `conflicts_with_all` rejects supplying both the explicit
        // `-p` flag and the positional argument.
        use clap::Parser;
        assert!(Cli::try_parse_from(["roba", "-p", "x", "positional"]).is_err());
    }

    #[test]
    fn prompt_flag_conflicts_with_file() {
        // `-p` and `-f` are both prompt sources; supplying both silently
        // dropped the `-p` text before the conflicts_with_all fix.
        use clap::Parser;
        assert!(Cli::try_parse_from(["roba", "-p", "x", "-f", "task.md"]).is_err());
    }

    #[test]
    fn prompt_flag_conflicts_with_editor() {
        // `-p` and `-e` are both prompt sources and are mutually exclusive.
        use clap::Parser;
        assert!(Cli::try_parse_from(["roba", "-p", "x", "-e"]).is_err());
    }

    #[test]
    fn prompt_flag_alone_still_parses() {
        // The escape hatch on its own is unaffected by the added conflicts.
        use clap::Parser;
        let cli = Cli::try_parse_from(["roba", "-p", "x"]).unwrap();
        assert_eq!(cli.ask.prompt_flag.as_deref(), Some("x"));
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
    fn max_turns_parses_valid_value() {
        use clap::Parser;
        let cli = Cli::try_parse_from(["roba", "--max-turns", "5", "prompt"]).unwrap();
        assert_eq!(cli.ask.max_turns, Some(5));
        assert_eq!(cli.ask.prompt.as_deref(), Some("prompt"));
    }

    #[test]
    fn max_turns_rejects_non_numeric() {
        use clap::Parser;
        assert!(Cli::try_parse_from(["roba", "--max-turns", "abc", "prompt"]).is_err());
    }

    #[test]
    fn max_budget_usd_parses_valid_value() {
        use clap::Parser;
        let cli = Cli::try_parse_from(["roba", "--max-budget-usd", "10.5", "prompt"]).unwrap();
        assert_eq!(cli.ask.max_budget_usd, Some(10.5));
    }

    #[test]
    fn max_budget_usd_rejects_non_numeric() {
        use clap::Parser;
        assert!(Cli::try_parse_from(["roba", "--max-budget-usd", "lots", "prompt"]).is_err());
    }

    #[test]
    fn limits_flags_compose() {
        // All three guardrails together, the unattended-loop case.
        use clap::Parser;
        let cli = Cli::try_parse_from([
            "roba",
            "--max-turns",
            "5",
            "--max-budget-usd",
            "10",
            "--timeout",
            "300",
            "prompt",
        ])
        .unwrap();
        assert_eq!(cli.ask.max_turns, Some(5));
        assert_eq!(cli.ask.max_budget_usd, Some(10.0));
        assert_eq!(cli.ask.timeout, Some(300));
    }

    #[test]
    fn timeout_parses_valid_value() {
        use clap::Parser;
        let cli = Cli::try_parse_from(["roba", "--timeout", "300", "prompt"]).unwrap();
        assert_eq!(cli.ask.timeout, Some(300));
    }

    #[test]
    fn timeout_zero_parses_as_disable() {
        // 0 is a valid value (disables the deadline); it must not error.
        use clap::Parser;
        let cli = Cli::try_parse_from(["roba", "--timeout", "0", "prompt"]).unwrap();
        assert_eq!(cli.ask.timeout, Some(0));
    }

    #[test]
    fn timeout_rejects_non_numeric() {
        use clap::Parser;
        assert!(Cli::try_parse_from(["roba", "--timeout", "soon", "prompt"]).is_err());
    }

    #[test]
    fn mcp_config_collects_repeated_values() {
        // Repeatable list flag: each --mcp-config pushes onto the Vec.
        use clap::Parser;
        let cli = Cli::try_parse_from([
            "roba",
            "--mcp-config",
            "a.json",
            "--mcp-config",
            "b.json",
            "prompt",
        ])
        .unwrap();
        assert_eq!(
            cli.ask.mcp_config,
            vec!["a.json".to_string(), "b.json".to_string()]
        );
        assert_eq!(cli.ask.prompt.as_deref(), Some("prompt"));
    }

    #[test]
    fn hermetic_bare_is_both() {
        use clap::Parser;
        let cli = Cli::try_parse_from(["roba", "--hermetic", "p"]).unwrap();
        assert_eq!(cli.ask.hermetic, Some(HermeticWhich::Both));
    }

    #[test]
    fn hermetic_value_parses_each_axis() {
        use clap::Parser;
        assert_eq!(
            Cli::try_parse_from(["roba", "--hermetic=roba", "p"])
                .unwrap()
                .ask
                .hermetic,
            Some(HermeticWhich::Roba)
        );
        assert_eq!(
            Cli::try_parse_from(["roba", "--hermetic=claude", "p"])
                .unwrap()
                .ask
                .hermetic,
            Some(HermeticWhich::Claude)
        );
    }

    #[test]
    fn hermetic_require_equals_does_not_swallow_prompt() {
        use clap::Parser;
        // A space-separated word after `--hermetic` is the PROMPT, not WHICH
        // (require_equals; the #100 footgun).
        let cli = Cli::try_parse_from(["roba", "--hermetic", "seal it"]).unwrap();
        assert_eq!(cli.ask.hermetic, Some(HermeticWhich::Both));
        assert_eq!(cli.ask.prompt.as_deref(), Some("seal it"));
    }

    #[test]
    fn hermetic_conflicts_with_no_hermetic() {
        use clap::Parser;
        assert!(Cli::try_parse_from(["roba", "--hermetic", "--no-hermetic", "p"]).is_err());
    }

    #[test]
    fn strict_mcp_config_parses() {
        use clap::Parser;
        let cli = Cli::try_parse_from([
            "roba",
            "--mcp-config",
            "a.json",
            "--strict-mcp-config",
            "prompt",
        ])
        .unwrap();
        assert_eq!(cli.ask.mcp_config, vec!["a.json".to_string()]);
        assert!(cli.ask.strict_mcp_config);
    }

    #[test]
    fn mcp_config_omitted_is_empty() {
        use clap::Parser;
        let cli = Cli::try_parse_from(["roba", "prompt"]).unwrap();
        assert!(cli.ask.mcp_config.is_empty());
        assert!(!cli.ask.strict_mcp_config);
    }

    #[test]
    fn add_dir_collects_repeated_values() {
        // Repeatable list flag: each --add-dir pushes onto the Vec.
        use clap::Parser;
        let cli = Cli::try_parse_from([
            "roba",
            "--add-dir",
            "/extra/a",
            "--add-dir",
            "/extra/b",
            "prompt",
        ])
        .unwrap();
        assert_eq!(
            cli.ask.add_dir,
            vec!["/extra/a".to_string(), "/extra/b".to_string()]
        );
        assert_eq!(cli.ask.prompt.as_deref(), Some("prompt"));
    }

    #[test]
    fn add_dir_omitted_is_empty() {
        use clap::Parser;
        let cli = Cli::try_parse_from(["roba", "prompt"]).unwrap();
        assert!(cli.ask.add_dir.is_empty());
    }

    #[test]
    fn fallback_model_parses() {
        use clap::Parser;
        let cli = Cli::try_parse_from(["roba", "--fallback-model", "haiku", "prompt"]).unwrap();
        assert_eq!(cli.ask.fallback_model.as_deref(), Some("haiku"));
        assert_eq!(cli.ask.prompt.as_deref(), Some("prompt"));
    }

    #[test]
    fn no_session_persistence_parses() {
        use clap::Parser;
        let cli = Cli::try_parse_from(["roba", "--no-session-persistence", "prompt"]).unwrap();
        assert!(cli.ask.no_session_persistence);
        assert_eq!(cli.ask.prompt.as_deref(), Some("prompt"));
    }

    #[test]
    fn medtier_flags_compose() {
        // All three med-tier pass-throughs together, the unattended case.
        use clap::Parser;
        let cli = Cli::try_parse_from([
            "roba",
            "--add-dir",
            "/extra",
            "--fallback-model",
            "sonnet",
            "--no-session-persistence",
            "prompt",
        ])
        .unwrap();
        assert_eq!(cli.ask.add_dir, vec!["/extra".to_string()]);
        assert_eq!(cli.ask.fallback_model.as_deref(), Some("sonnet"));
        assert!(cli.ask.no_session_persistence);
    }

    #[test]
    fn worktree_list_parses() {
        use clap::Parser;
        let cli = Cli::try_parse_from(["roba", "worktree", "list"]).unwrap();
        match cli.command {
            Some(SubCommand::Worktree {
                cmd: WorktreeCmd::List(args),
            }) => assert!(!args.json),
            other => panic!("expected worktree list, got {other:?}"),
        }
    }

    #[test]
    fn worktree_list_json_parses() {
        use clap::Parser;
        let cli = Cli::try_parse_from(["roba", "worktree", "list", "--json"]).unwrap();
        match cli.command {
            Some(SubCommand::Worktree {
                cmd: WorktreeCmd::List(args),
            }) => assert!(args.json),
            other => panic!("expected worktree list --json, got {other:?}"),
        }
    }

    #[test]
    fn worktree_list_honors_global_cwd() {
        // `-C/--cwd` is a global flag, so it attaches to the subcommand.
        use clap::Parser;
        let cli = Cli::try_parse_from(["roba", "worktree", "list", "-C", "/some/repo"]).unwrap();
        assert_eq!(cli.cwd.as_deref(), Some(std::path::Path::new("/some/repo")));
    }

    #[test]
    fn show_parses_session_id() {
        use clap::Parser;
        let cli = Cli::try_parse_from(["roba", "show", "abc-123"]).unwrap();
        match cli.command {
            Some(SubCommand::Show(args)) => {
                assert_eq!(args.session_id, "abc-123");
                assert!(!args.metrics);
                assert!(!args.json);
            }
            other => panic!("expected show, got {other:?}"),
        }
    }

    #[test]
    fn show_parses_metrics_flag() {
        use clap::Parser;
        let cli = Cli::try_parse_from(["roba", "show", "abc-123", "--metrics"]).unwrap();
        match cli.command {
            Some(SubCommand::Show(args)) => {
                assert_eq!(args.session_id, "abc-123");
                assert!(args.metrics);
            }
            other => panic!("expected show --metrics, got {other:?}"),
        }
    }

    #[test]
    fn show_parses_json_flag() {
        use clap::Parser;
        let cli = Cli::try_parse_from(["roba", "show", "abc-123", "--json", "--metrics"]).unwrap();
        match cli.command {
            Some(SubCommand::Show(args)) => {
                assert!(args.json);
                assert!(args.metrics);
            }
            other => panic!("expected show --json --metrics, got {other:?}"),
        }
    }

    #[test]
    fn show_parses_wait_flag() {
        use clap::Parser;
        let cli = Cli::try_parse_from(["roba", "show", "abc-123", "--wait"]).unwrap();
        match cli.command {
            Some(SubCommand::Show(args)) => {
                assert!(args.wait);
                assert_eq!(args.timeout, None);
            }
            other => panic!("expected show --wait, got {other:?}"),
        }
    }

    #[test]
    fn show_parses_wait_with_timeout() {
        use clap::Parser;
        let cli =
            Cli::try_parse_from(["roba", "show", "abc-123", "--wait", "--timeout", "30"]).unwrap();
        match cli.command {
            Some(SubCommand::Show(args)) => {
                assert!(args.wait);
                assert_eq!(args.timeout, Some(30));
            }
            other => panic!("expected show --wait --timeout 30, got {other:?}"),
        }
    }

    #[test]
    fn show_honors_global_cwd() {
        use clap::Parser;
        let cli = Cli::try_parse_from(["roba", "show", "abc-123", "-C", "/some/repo"]).unwrap();
        assert_eq!(cli.cwd.as_deref(), Some(std::path::Path::new("/some/repo")));
    }

    #[test]
    fn history_worktree_filter_parses() {
        use clap::Parser;
        let cli = Cli::try_parse_from(["roba", "history", "--worktree", "agent-abc123"]).unwrap();
        match cli.command {
            Some(SubCommand::History(args)) => {
                assert_eq!(args.worktree.as_deref(), Some("agent-abc123"));
            }
            other => panic!("expected history --worktree, got {other:?}"),
        }
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
    fn alias_draft_parses_description() {
        use clap::Parser;
        let cli =
            Cli::try_parse_from(["roba", "alias", "draft", "a read-only review verb"]).unwrap();
        match cli.command {
            Some(SubCommand::Alias {
                action: AliasAction::Draft(args),
            }) => {
                assert_eq!(args.description, "a read-only review verb");
                assert!(args.write.is_none());
                assert!(args.model.is_none());
            }
            other => panic!("expected alias draft, got {other:?}"),
        }
    }

    #[test]
    fn alias_draft_write_without_path() {
        use clap::Parser;
        let cli = Cli::try_parse_from(["roba", "alias", "draft", "desc", "--write"]).unwrap();
        match cli.command {
            Some(SubCommand::Alias {
                action: AliasAction::Draft(args),
            }) => assert!(matches!(args.write, Some(None))),
            other => panic!("expected alias draft, got {other:?}"),
        }
    }

    #[test]
    fn alias_draft_write_with_path() {
        use clap::Parser;
        let cli = Cli::try_parse_from(["roba", "alias", "draft", "desc", "--write", "/tmp/r.toml"])
            .unwrap();
        match cli.command {
            Some(SubCommand::Alias {
                action: AliasAction::Draft(args),
            }) => assert_eq!(args.write, Some(Some(PathBuf::from("/tmp/r.toml")))),
            other => panic!("expected alias draft, got {other:?}"),
        }
    }

    #[test]
    fn alias_draft_accepts_model() {
        use clap::Parser;
        let cli = Cli::try_parse_from([
            "roba",
            "alias",
            "draft",
            "desc",
            "--model",
            "claude-haiku-4-5",
        ])
        .unwrap();
        match cli.command {
            Some(SubCommand::Alias {
                action: AliasAction::Draft(args),
            }) => assert_eq!(args.model.as_deref(), Some("claude-haiku-4-5")),
            other => panic!("expected alias draft, got {other:?}"),
        }
    }

    #[test]
    fn profile_draft_parses_description() {
        use clap::Parser;
        let cli =
            Cli::try_parse_from(["roba", "profile", "draft", "a long-running worker"]).unwrap();
        match cli.command {
            Some(SubCommand::Profile {
                action: ProfileAction::Draft(args),
            }) => {
                assert_eq!(args.description, "a long-running worker");
                assert!(args.write.is_none());
                assert!(args.model.is_none());
            }
            other => panic!("expected profile draft, got {other:?}"),
        }
    }

    #[test]
    fn profile_draft_write_without_path() {
        use clap::Parser;
        let cli = Cli::try_parse_from(["roba", "profile", "draft", "desc", "--write"]).unwrap();
        match cli.command {
            Some(SubCommand::Profile {
                action: ProfileAction::Draft(args),
            }) => assert!(matches!(args.write, Some(None))),
            other => panic!("expected profile draft, got {other:?}"),
        }
    }

    #[test]
    fn profile_draft_write_with_path() {
        use clap::Parser;
        let cli =
            Cli::try_parse_from(["roba", "profile", "draft", "desc", "--write", "/tmp/p.toml"])
                .unwrap();
        match cli.command {
            Some(SubCommand::Profile {
                action: ProfileAction::Draft(args),
            }) => assert_eq!(args.write, Some(Some(PathBuf::from("/tmp/p.toml")))),
            other => panic!("expected profile draft, got {other:?}"),
        }
    }

    #[test]
    fn profile_draft_accepts_model() {
        use clap::Parser;
        let cli = Cli::try_parse_from([
            "roba",
            "profile",
            "draft",
            "desc",
            "--model",
            "claude-haiku-4-5",
        ])
        .unwrap();
        match cli.command {
            Some(SubCommand::Profile {
                action: ProfileAction::Draft(args),
            }) => assert_eq!(args.model.as_deref(), Some("claude-haiku-4-5")),
            other => panic!("expected profile draft, got {other:?}"),
        }
    }

    // -- --detach ----------------------------------------------------------

    #[test]
    fn detach_parses_alone() {
        use clap::Parser;
        let cli = Cli::try_parse_from(["roba", "--detach", "prompt"]).unwrap();
        assert!(cli.ask.detach);
        assert_eq!(cli.ask.prompt.as_deref(), Some("prompt"));
    }

    #[test]
    fn detach_omitted_is_false() {
        use clap::Parser;
        let cli = Cli::try_parse_from(["roba", "prompt"]).unwrap();
        assert!(!cli.ask.detach);
    }

    #[test]
    fn detach_composes_with_session_id_and_out_and_trace() {
        // --session-id, -o/--out, and --trace stay allowed under --detach
        // (the detached child uses/writes them).
        use clap::Parser;
        let cli = Cli::try_parse_from([
            "roba",
            "--detach",
            "--session-id",
            "11111111-1111-4111-8111-111111111111",
            "--out",
            "/tmp/a.txt",
            "--trace",
            "/tmp/t.jsonl",
            "prompt",
        ])
        .unwrap();
        assert!(cli.ask.detach);
        assert!(cli.ask.session_id.is_some());
        assert_eq!(
            cli.ask.out.as_deref(),
            Some(std::path::Path::new("/tmp/a.txt"))
        );
        assert!(cli.ask.trace.is_some());
    }

    #[test]
    fn no_agent_notice_parses_alone() {
        use clap::Parser;
        let cli = Cli::try_parse_from(["roba", "--no-agent-notice", "prompt"]).unwrap();
        assert!(cli.ask.no_agent_notice);
        assert!(cli.ask.agent_notice.is_none());
    }

    #[test]
    fn agent_notice_parses_with_text() {
        use clap::Parser;
        let cli = Cli::try_parse_from(["roba", "--agent-notice", "custom text", "prompt"]).unwrap();
        assert_eq!(cli.ask.agent_notice.as_deref(), Some("custom text"));
        assert!(!cli.ask.no_agent_notice);
    }

    #[test]
    fn detach_conflicts_with_attachment_flags() {
        // Each output-consuming / interactive flag must reject --detach at
        // the parse level.
        use clap::Parser;
        for conflicting in [
            vec!["--json"],
            vec!["--stream"],
            vec!["--code"],
            vec!["--show-thinking"],
            vec!["--editor"],
            vec!["--pick"],
        ] {
            let mut argv = vec!["roba", "--detach"];
            argv.extend(conflicting.iter().copied());
            argv.push("prompt");
            assert!(
                Cli::try_parse_from(&argv).is_err(),
                "expected --detach to conflict with {conflicting:?}"
            );
        }
    }
}
