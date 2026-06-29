//! `roba` -- single-prompt CLI runner built on `claude-wrapper`.
//!
//! This lib hosts the module surface so integration tests can drive
//! the same code paths the binary uses. `main.rs` is just an entry
//! that hands a parsed [`cli::Cli`] to [`dispatch`].
//!
//! See the README for positioning and the agent ABI.

use anyhow::{Context, Result, bail};
use claude_wrapper::{Claude, QueryCommand};
use std::io::IsTerminal;

pub mod agent_check;
pub mod aliases;
pub mod cli;
pub mod config;
pub mod cost;
pub mod detach;
pub mod doctor;
pub mod draft;
pub mod env;
pub mod error;
pub mod history;
pub mod lint;
pub mod output;
pub mod profile;
pub mod prompt;
pub mod rates;
pub mod render;
pub mod session;
pub mod show;
pub mod stdin_probe;
pub mod stream;
pub mod worktree;

use crate::cli::{AskArgs, Cli, SubCommand};
use crate::history::{pick_session_interactive, run_history, run_last};
use crate::output::{
    default_body, extract_code_blocks, format_footer, looks_like_refusal, path_is_json,
    should_show_footer, surface_structured_output,
};
use crate::prompt::{
    apply_vars, collect_attachments, collect_git_context, compose_prompt, merge_optional,
    resolve_main_prompt,
};
use crate::session::apply_session;
use crate::stream::run_streaming;

/// Dispatch a parsed [`Cli`] to the matching runner. Subcommands
/// (history, last) run synchronously; the default action (`run_ask`)
/// is async.
pub async fn dispatch(cli: Cli) -> Result<()> {
    if let Some(path) = cli.cwd.as_deref() {
        std::env::set_current_dir(path)
            .with_context(|| format!("--cwd: cannot change directory to {}", path.display()))?;
    }
    match cli.command {
        Some(SubCommand::History(args)) => run_history(args),
        Some(SubCommand::Last(args)) => run_last(args),
        // Profile inspection is synchronous; `draft` makes one claude call
        // (the only async profile verb), so it routes through `run_draft`.
        Some(SubCommand::Profile { action }) => match action {
            crate::cli::ProfileAction::Draft(args) => profile::run_draft(args).await,
            other => profile::run(other),
        },
        Some(SubCommand::Cost(args)) => cost::run(args),
        // Health check: print one line per check (or a `--json`
        // envelope), exit 0/1. No claude prompt -- only `claude
        // --version`.
        Some(SubCommand::Doctor(args)) => {
            let code = doctor::run(args)?;
            std::process::exit(code);
        }
        // Alias inspection is synchronous; `draft` makes one claude call
        // (the only async alias verb), so it routes through `run_draft`.
        Some(SubCommand::Alias { action }) => match action {
            crate::cli::AliasAction::Draft(args) => aliases::run_draft(args).await,
            other => aliases::run(other),
        },
        // `config init` makes one claude call (with a read-only window
        // onto the project), so it is async like the draft verbs.
        Some(SubCommand::Config { cmd }) => match cmd {
            crate::cli::ConfigCmd::Init(args) => config::run_init(args).await,
            // Static config checks: print findings (or a `--json`
            // envelope), exit 0/1. Read-only, no claude call.
            crate::cli::ConfigCmd::Lint(args) => {
                let code = lint::run(args)?;
                std::process::exit(code);
            }
            // Merged-pool view: print the whole config pool merged into
            // one canonical roba.toml (or a `--json` envelope). Read-only,
            // no claude call.
            crate::cli::ConfigCmd::Show(args) => config::run_show(args),
        },
        // Read-only inspection of the repo's git worktrees. Shells to
        // `git worktree list` via claude-wrapper; no claude call.
        Some(SubCommand::Worktree { cmd }) => worktree::run(cmd),
        // Read-only result handle: reconstruct a stored session's result
        // from its on-disk JSONL. No claude call.
        Some(SubCommand::Show(args)) => show::run(&args),
        // Pure generator: print the completion script and exit. No
        // claude call, no prompt resolution.
        Some(SubCommand::Completions { shell }) => {
            use clap::CommandFactory;
            let mut cmd = Cli::command();
            clap_complete::generate(shell, &mut cmd, "roba", &mut std::io::stdout());
            Ok(())
        }
        // An unrecognized leading word: clap routes it here. The alias
        // name landed in `ask.prompt`; the remaining tokens are the
        // alias args (positional + any trailing flags).
        Some(SubCommand::External(rest)) => {
            let name = cli
                .ask
                .prompt
                .clone()
                .ok_or_else(|| anyhow::anyhow!("could not determine alias name"))?;
            aliases::dispatch_alias(&name, &rest).await
        }
        None => {
            // A bare single word may name a zero-arg alias
            // (`roba commit-msg`). Otherwise it's a normal prompt --
            // unless it's a close typo of a subcommand (`worktrees`),
            // in which case it's caught instead of silently prompting.
            if let Some(name) = aliases::bare_alias_candidate(&cli.ask)? {
                let trailing = aliases::trailing_args_from_env(&name);
                aliases::dispatch_alias(&name, &trailing).await
            } else if let Some(msg) = aliases::bare_subcommand_typo(&cli.ask) {
                anyhow::bail!(msg)
            } else {
                run_ask(cli.ask).await
            }
        }
    }
}

/// Versioned success envelope for `--json` mode. Top-level `version`
/// is the stable ABI marker (see the [`error`] module for the v1
/// contract); `result` holds the wrapper's `QueryResult` shape
/// verbatim. Mirrors the error envelope's `version` + `error` layout
/// so success and failure are structurally consistent.
///
/// `pub(crate)` so `roba show` (the reconstructed-envelope path in
/// [`crate::show`]) emits the byte-identical envelope shape instead of
/// re-deriving it.
#[derive(serde::Serialize)]
pub(crate) struct SuccessEnvelope<'a> {
    pub(crate) version: u32,
    pub(crate) result: &'a claude_wrapper::types::QueryResult,
    /// True when [`crate::output::looks_like_refusal`] matched the
    /// response body. Lets non-TTY consumers (the ones that never see
    /// the human-facing footer warning) branch on "got an answer" vs
    /// "got refused" without parsing the body text. Additive v1 field.
    pub(crate) refusal: bool,
}

/// Versioned success envelope for the read-only management commands'
/// `--json` mode. Mirrors [`SuccessEnvelope`]'s `version` + `result`
/// layout, minus the ask-only `refusal` flag, so the whole `--json`
/// surface is `{ version: 1, result: ... }` on success and
/// `{ version: 1, error: ... }` on failure (see the [`error`] module
/// for the v1 contract).
///
/// `cost`, `history`, `doctor`, and `worktree list` all wrap their
/// payload through this so a programmatic consumer peels off `version`
/// and `result` uniformly regardless of which command produced the
/// output. Generic over the payload `T` since each command has its own
/// result shape (a `Rollup`, a `Vec<SessionSummary>`, a doctor report,
/// a `Vec<Worktree>`).
#[derive(serde::Serialize)]
pub(crate) struct VersionedResult<'a, T: serde::Serialize> {
    pub(crate) version: u32,
    pub(crate) result: &'a T,
}

impl<'a, T: serde::Serialize> VersionedResult<'a, T> {
    /// Wrap a payload reference at the current ABI version (1).
    pub(crate) fn new(result: &'a T) -> Self {
        Self { version: 1, result }
    }
}

/// Default action: resolve a prompt, send it through claude, render
/// the result.
/// If an optional-value flag plausibly swallowed what the user meant as the
/// prompt, return a one-line note naming the flag and the consumed token.
///
/// Conservative by design: only fires when the consumed value contains
/// whitespace, since a real session id, worktree name, or language token
/// never does. So a legitimate `-c <uuid>`, `-w branch`, or `--code rust`
/// never triggers it -- only a quoted multi-word value that was almost
/// certainly meant as the prompt. `--git-log` is excluded: it parses as a
/// number, so a swallowed prompt already fails loud at parse time and never
/// reaches a no-prompt error.
fn swallow_note(args: &AskArgs) -> Option<String> {
    fn looks_like_prompt(v: &str) -> bool {
        v.chars().any(char::is_whitespace)
    }
    if let Some(Some(v)) = &args.continue_session
        && looks_like_prompt(v)
    {
        return Some(format!(
            "note: -c consumed \"{v}\" as its value; pass the prompt with -p, or -c alone to continue"
        ));
    }
    if let Some(Some(v)) = &args.worktree
        && looks_like_prompt(v)
    {
        return Some(format!(
            "note: -w consumed \"{v}\" as its value; pass the prompt with -p"
        ));
    }
    if let Some(v) = &args.code
        && looks_like_prompt(v)
    {
        return Some(format!(
            "note: --code consumed \"{v}\" as its value; pass the prompt with -p"
        ));
    }
    None
}

pub async fn run_ask(mut args: AskArgs) -> Result<()> {
    env::apply_env_overrides(&mut args)?;
    let pool = profile::load_pool()?;
    if let Some(chosen) = profile::resolve(&args, &pool)? {
        let source = profile::profile_source_label(&args, &pool);
        profile::merge_into_args(&mut args, chosen, &source);
    }
    // `--session NAME` resolves a configured `[session]` handle to its
    // bound uuid and feeds the existing `continue_session` path. Runs
    // after the profile merge so it overrides a profile-supplied
    // `continue`. clap already excludes `-c` / `--pick` / `--fresh`.
    if let Some(name) = args.session.clone() {
        let uuid = resolve_session(&name, &pool.sessions)?;
        args.continue_session = Some(Some(uuid));
    }
    // --show-permissions previews the resolved allow/deny set (with
    // provenance) using the exact same resolution flow a real run
    // uses, then exits without calling claude. Must come after the
    // full CLI > env > profile merge so the preview is faithful.
    if args.show_permissions {
        eprintln!("{}", output::format_permissions(&args));
        return Ok(());
    }
    // --fresh is the kill switch: it cancels any continuation
    // settings that arrived via env vars or profile defaults.
    if args.fresh {
        args.continue_session = None;
    }
    // --no-worktree (or ROBA_NO_WORKTREE) forces worktree off for this
    // run, overriding any config/env-set worktree. Runs after the env +
    // profile merge so the CLI/env override wins over a filled-in value,
    // and before `apply_session` reads `args.worktree`.
    apply_no_worktree(&mut args);
    // --fork branches a *specific* session, so it needs an explicit
    // id. clap's `requires = "continue_session"` only enforces that
    // `-c` was passed at all; it can't tell the bare form (`-c`,
    // "most recent") from the value form (`-c=ID`). Enforce the value
    // requirement here, after env + profile resolution has settled the
    // final shape of `continue_session`.
    if args.fork {
        match &args.continue_session {
            Some(Some(_)) => {} // fine: forking a specific session id
            Some(None) => bail!("--fork requires an explicit session id; use `-c=ID --fork`"),
            None => bail!("--fork requires `-c=ID`"),
        }
    }
    // --detach fires the run disowned and returns its session handle. Branch
    // here -- after the env/profile merge + session resolution (so the
    // rails-nudge predicate and the handle see resolved values) and before
    // prompt resolution (the detached child re-resolves the prompt itself).
    // The child re-execs the full argv minus `--detach`, so anything below
    // this seam runs in the child, not the parent.
    if args.detach {
        return detach::run_detached(&args);
    }
    // --json-schema names a PATH to a JSON Schema file. Resolve it here,
    // after the full CLI > env > profile merge, by reading the file and
    // replacing the value with its contents (claude's `--json-schema`
    // takes inline JSON; the path is roba's ergonomic sugar). Validate
    // the contents parse as JSON so a malformed schema fails via roba's
    // error envelope instead of surfacing as an opaque claude error. Both
    // the streaming and non-streaming paths read the resolved string from
    // `args.json_schema`, so this single seam covers both.
    if let Some(path) = args.json_schema.clone() {
        let contents = std::fs::read_to_string(&path)
            .with_context(|| format!("reading --json-schema file `{path}`"))?;
        serde_json::from_str::<serde_json::Value>(&contents)
            .with_context(|| format!("--json-schema file `{path}` is not valid JSON"))?;
        args.json_schema = Some(contents);
    }
    ensure_interactive_for_flags(&args)?;
    if args.pick {
        let id = pick_session_interactive()?;
        eprintln!("resuming session {}", id.get(..8).unwrap_or(&id));
        args.continue_session = Some(Some(id));
    }
    // Expand a git-style short session-id prefix for `-c <value>` against
    // this project's sessions, so the truncated id roba prints in its
    // footer is directly usable as a resume handle (#304). Runs after
    // every session selector has settled (`--session`, `--fresh`,
    // `--pick`) and uses the effective cwd (`dispatch` already applied
    // `-C/--cwd`), since session ids are per-project.
    expand_continue_prefix(&mut args)?;
    // Agent frontmatter permission check: warn if the agent declares
    // tools that are not in the resolved allowlist. Best-effort and
    // non-blocking -- dispatch still proceeds.
    let cwd = std::env::current_dir().unwrap_or_default();
    agent_check::maybe_warn(&args, &cwd);

    // `-p / --prompt` is an explicit alternative to the positional
    // prompt (clap enforces the mutual exclusion via conflicts_with_all,
    // also against `-f`/`-e`). Whichever was supplied is the explicit
    // prompt string; the resolve order is editor > file > explicit (piped
    // stdin becomes the prompt when none is given, otherwise context).
    let explicit_prompt = args.prompt_flag.as_deref().or(args.prompt.as_deref());
    let resolved = match resolve_main_prompt(
        explicit_prompt,
        args.file.as_deref(),
        args.editor,
        args.editor_history,
    ) {
        Ok(r) => r,
        Err(e) => {
            // An `empty stdin` (or similar) prompt-resolution failure can
            // really be an optional-value flag that swallowed the prompt --
            // surface that before the raw error.
            if let Some(note) = swallow_note(&args) {
                eprintln!("{note}");
            }
            return Err(e);
        }
    };
    let attachments = collect_attachments(&args.attach)?;
    let git_context = collect_git_context(&args)?;
    let context = merge_optional(attachments, git_context);
    // Promptless guard. The fully composed prompt (positional + prepend +
    // attach + git) is only known here, so the check belongs at this seam:
    // a user with no positional but a `--git-diff` is NOT promptless and
    // must not be intercepted. `compose_prompt` returns `None` only when
    // nothing composed to a non-empty body.
    let prompt = match compose_prompt(
        resolved.main,
        &args.prepend,
        resolved.piped_context,
        context,
        &args.append,
    )? {
        Some(p) => p,
        None => {
            // No resolvable prompt. On a TTY the user ran `roba` with no
            // input -- guide them with an abbreviated help blurb (exit 0).
            // Non-TTY (script/pipe) stays a hard error so callers still get
            // a non-zero exit.
            if std::io::stdin().is_terminal() {
                eprintln!("{}", crate::cli::no_prompt_blurb());
                return Ok(());
            }
            // If an optional-value flag swallowed the intended prompt, the
            // bare "no prompt" message hides the real cause -- name the
            // consumed token so the error explains itself.
            if let Some(note) = swallow_note(&args) {
                eprintln!("{note}");
            }
            anyhow::bail!(
                "no prompt: pass one as an argument, use -f / -e, --prepend / --append / --attach, pipe via stdin, or use `-` for stdin"
            );
        }
    };
    let prompt = apply_vars(prompt, &args.var);
    // Surface likely-typo'd `--var` keys (an unsubstituted `{{NAME}}` would
    // otherwise ship silently). Ungated like the `--attach matched no files`
    // warning -- it is a correctness signal, most valuable in a quiet
    // automated run, not decorative metadata.
    prompt::warn_unsubstituted_placeholders(&prompt);

    if args.echo && !args.quiet {
        eprintln!("{prompt}");
        eprintln!();
        eprintln!("---");
        eprintln!();
    }
    // --worktree needs a git repository (claude creates the worktree under
    // `.git`). Preflight here -- after the prompt resolves, before the
    // claude spawn -- so a non-git dir fails with a clean, actionable error
    // instead of claude's stderr buried behind the full argv echo (#327).
    // The effective working dir is already the process cwd: `dispatch`
    // applied any `-C/--cwd` before reaching here.
    if args.worktree.is_some() {
        let dir = std::env::current_dir().unwrap_or_default();
        if !is_in_git_repo(&dir) {
            bail!(
                "--worktree needs a git repository, but {} is not one; run `git init` or drop --worktree",
                dir.display()
            );
        }
    }
    // A run-level wall-clock deadline lives on the Claude client, so it is
    // enforced uniformly across both the streaming and non-streaming exec
    // paths below (the wrapper kills + reaps the child and returns
    // Error::Timeout, which classify_exit_code maps to exit 4). `0` disables.
    let mut builder = Claude::builder();
    if let Some(secs) = args.timeout
        && secs > 0
    {
        builder = builder.timeout_secs(secs);
    }
    let claude = builder.build()?;

    if args.stream {
        run_streaming(&claude, prompt, &args, stream::DisplayMode::Live).await?;
        return Ok(());
    }

    let pre_style = render::Style::detect(&args);
    let spinner = pre_style.spinner.then(render::spinner);
    let mut result = if args.trace.is_some() {
        // --trace without --stream: drive the streaming pipeline so the
        // event log can be captured, but suppress all live display and
        // render the final answer exactly as the non-streaming path
        // would (JSON envelope / --code / --out / footer below).
        match run_streaming(&claude, prompt, &args, stream::DisplayMode::Silent).await? {
            Some(r) => r,
            // Parity with the Live --stream path (src/stream.rs): no result
            // event is "no usable output", which is exit 6, not the generic
            // exit 1 a `bail!` would map to via classify_exit_code. Same
            // condition, same code on both paths.
            None => exit_unusable(
                EXIT_UNUSABLE_RESULT,
                "no result event: the streaming run produced no usable output",
            ),
        }
    } else {
        let name = session::derive_session_name(&prompt);
        apply_session(
            QueryCommand::new(prompt).name(name).prompt_via_stdin(true),
            &args,
        )
        .execute_json(&claude)
        .await?
    };
    if let Some(pb) = spinner {
        pb.finish_and_clear();
    }

    // #317: with --json-schema, claude returns the schema-constrained answer
    // as a fenced JSON block in `result.result` and leaves `structured_output`
    // unset. Surface it cleanly so `--json` consumers get a real
    // `.result.structured_output` and an unfenced `.result.result`. Gated on
    // `--json-schema` so a normal answer containing a code block is untouched.
    if args.json_schema.is_some() {
        surface_structured_output(&mut result);
    }

    let file_path = args.out.as_deref();
    let want_json = args.json || file_path.is_some_and(path_is_json);
    let body = if want_json {
        let envelope = SuccessEnvelope {
            version: 1,
            result: &result,
            refusal: looks_like_refusal(&result.result),
        };
        serde_json::to_string_pretty(&envelope)?
    } else if let Some(filter) = args.code.as_deref() {
        let lang = if filter.is_empty() {
            None
        } else {
            Some(filter)
        };
        extract_code_blocks(&result.result, lang)
    } else {
        default_body(&result, args.json_schema.is_some())
    };
    let style = render::Style::detect(&args);
    // --out always also writes stdout; redirect to /dev/null to suppress.
    render::print_body(&body, &style);
    if let Some(path) = file_path {
        std::fs::write(path, format!("{body}\n"))
            .with_context(|| format!("writing result to {}", path.display()))?;
    }
    if should_show_footer(&args) {
        render::print_meta_blank();
        if looks_like_refusal(&result.result) {
            render::print_warning("response looks like a refusal", &style);
        }
        // Load the rate table only when the footer might show dollars.
        // A bad --rates-file shouldn't sink an otherwise-good answer, so
        // fall back to no dollars on a load error rather than bailing.
        let rates = if args.no_dollars {
            None
        } else {
            rates::Rates::resolve(args.rates_file.as_deref()).ok()
        };
        render::print_meta(
            &format_footer(
                &result,
                rates.as_ref(),
                args.no_dollars,
                args.model.as_deref(),
                args.effort.map(|e| e.as_str()),
            ),
            &style,
        );
    }
    // A run that "succeeded" with no usable output (empty answer or
    // is_error) must not exit 0 -- that is a silent failure for any caller
    // that trusts `$?`. The body/envelope above already emitted (so
    // `--json` stays byte-clean); the exit code carries the signal.
    if let Some((code, note)) = classify_result(&result) {
        exit_unusable(code, note);
    }
    Ok(())
}

/// Walk UP from `dir` looking for a `.git` entry, returning true as
/// soon as one is found. The entry may be a directory (a normal repo)
/// OR a file (a linked worktree's `.git` is a file pointing at the
/// gitdir), so `exists()` is the right test. No subprocess.
fn is_in_git_repo(dir: &std::path::Path) -> bool {
    let mut cur = Some(dir);
    while let Some(d) = cur {
        if d.join(".git").exists() {
            return true;
        }
        cur = d.parent();
    }
    false
}

/// Fail fast when an interactive-only flag is set without a TTY on
/// stdin. `-e` / `--editor` and `--pick` both block on human input;
/// in a head-less context (script, CI step, orchestrator) the
/// process would hang waiting for keystrokes that can't arrive.
/// stderr-not-a-TTY is fine -- output redirection is normal.
fn ensure_interactive_for_flags(args: &AskArgs) -> Result<()> {
    if args.editor && !std::io::stdin().is_terminal() {
        bail!("--editor requires an interactive terminal (stdin not a TTY)");
    }
    if args.pick && !std::io::stdin().is_terminal() {
        bail!("--pick requires an interactive terminal (stdin not a TTY)");
    }
    Ok(())
}

/// Resolve a `--session NAME` handle to its bound session uuid using
/// the merged `[session]` map. Errors (listing the known names) when
/// the name is not configured -- the bind is the user's job, done in a
/// local roba.toml `[session]` table, so an unknown name is a config
/// mistake worth surfacing rather than silently starting fresh.
fn resolve_session(
    name: &str,
    sessions: &std::collections::HashMap<String, String>,
) -> Result<String> {
    match sessions.get(name) {
        Some(uuid) => Ok(uuid.clone()),
        None => {
            let known = if sessions.is_empty() {
                "(none configured)".to_string()
            } else {
                let mut names: Vec<&str> = sessions.keys().map(String::as_str).collect();
                names.sort_unstable();
                names.join(", ")
            };
            bail!("no session named '{name}' in config (known: {known})")
        }
    }
}

/// Force the worktree off when `--no-worktree` (or `ROBA_NO_WORKTREE`)
/// is set, nulling any worktree value that a profile/top-level config or
/// `ROBA_WORKTREE` filled in. `--no-worktree` is a per-run CLI/env
/// override, not a stored config key, so it always wins -- and clap
/// already excludes it from co-existing with `--worktree`/`-w`. Must run
/// after the env + profile merge and before `apply_session` reads
/// `args.worktree`.
fn apply_no_worktree(args: &mut AskArgs) {
    if args.no_worktree {
        args.worktree = None;
    }
}

/// Expand a `-c <value>` (`--continue=VALUE`) that carried an explicit
/// value, treating it as a git-style short session-id prefix and
/// resolving it to a full UUID against the current project's sessions
/// before it reaches [`apply_session`] (-> claude's `--resume`).
///
/// Three outcomes (see [`session::resolve_session_prefix`]):
/// - a unique prefix is expanded in place to the full id;
/// - an ambiguous prefix is a hard error listing the candidates -- never
///   silently pick one;
/// - no match is left UNCHANGED, so claude's own session-TITLE resume
///   (and any cross-project full id) still works.
///
/// A failure to enumerate the project's sessions (no project dir yet, an
/// unreadable history root) is swallowed: the value passes through
/// unchanged so a probe error never blocks an otherwise-valid resume.
///
/// Only the value form (`Some(Some(_))`) is touched; bare `-c`
/// ("most recent", `Some(None)`) and a fresh session (`None`) are
/// untouched. `--session-id` is deliberately NOT routed here -- it keeps
/// its strict-UUID parse-time validation (#284); this is the lenient
/// resume side.
fn expand_continue_prefix(args: &mut AskArgs) -> Result<()> {
    let Some(Some(value)) = &args.continue_session else {
        return Ok(());
    };
    let value = value.clone();
    let Ok(ids) = history::current_project_session_ids() else {
        return Ok(());
    };
    match session::resolve_session_prefix(&value, &ids) {
        session::Resolution::Unique(full) => {
            args.continue_session = Some(Some(full));
        }
        session::Resolution::Ambiguous(candidates) => {
            let listed = candidates.join("\n  ");
            bail!(
                "session id prefix `{value}` is ambiguous; it matches:\n  {listed}\npass more characters to disambiguate"
            );
        }
        session::Resolution::NoMatch => {} // leave unchanged: title resume / cross-project id
    }
    Ok(())
}

/// Exit code for a run that the wrapper returned as `Ok` but whose
/// [`QueryResult`](claude_wrapper::types::QueryResult) is unusable -- an
/// empty answer or `is_error: true`. Distinct from the `Err`-path codes
/// in [`classify_exit_code`] (1-5) so an orchestrator can branch on "the
/// call succeeded but produced nothing usable" without parsing output.
///
/// Structured-signal asymmetry (deliberate): code 6 exits from the
/// `Ok`-path, so the success-shaped `--json` envelope has already been
/// written to stdout. Unlike the `Err`-path codes -- which have no stdout
/// and so emit a structured error envelope on stderr (`error::render_json`
/// in `main`) -- code 6 writes only a plain `roba: <note> (exit 6)` line
/// to stderr. A `--json` consumer branches on the exit code and inspects
/// `.result` / `.is_error` on the stdout envelope; it does NOT scrape
/// stderr for an error envelope on code 6. Emitting one would contradict
/// "the success envelope is on stdout."
pub const EXIT_UNUSABLE_RESULT: i32 = 6;

/// Classify a successfully-returned [`QueryResult`] as usable or not.
///
/// The wrapper returns `Ok(QueryResult)` whenever claude exits 0, even
/// when the payload carries `is_error: true` or an empty answer (a
/// non-zero claude exit becomes an `Err` instead, handled by
/// [`classify_exit_code`]). Without this check those "successful"
/// non-answers exit 0, a silent failure for any scripted caller that
/// trusts `$?`.
///
/// Returns `Some((code, note))` when the result is unusable -- the code
/// to exit with and a clean one-line stderr note explaining why -- or
/// `None` when there is a usable answer.
///
/// "Usable" means a non-empty textual `result` (after trim) OR a
/// non-null `structured_output` (the `--json-schema` answer shape). A
/// refusal is text, so it stays usable and exits 0 (a refusal is a valid
/// answer; detect it via the envelope's `refusal` field, not the exit
/// code). `is_error: true` is always unusable regardless of body.
pub(crate) fn classify_result(
    result: &claude_wrapper::types::QueryResult,
) -> Option<(i32, &'static str)> {
    if result.is_error {
        return Some((
            EXIT_UNUSABLE_RESULT,
            "claude returned an error result (is_error: true)",
        ));
    }
    let has_text = !result.result.trim().is_empty();
    let has_structured = result
        .extra
        .get("structured_output")
        .is_some_and(|v| !v.is_null());
    if !has_text && !has_structured {
        return Some((
            EXIT_UNUSABLE_RESULT,
            "empty result: the run produced no usable output",
        ));
    }
    None
}

/// Print the clean stderr note for an unusable result and exit with
/// `code`. stdout is flushed first so the already-rendered answer body
/// or `--json` envelope is not lost behind the immediate exit -- the
/// envelope still emits; the exit code carries the failure signal.
pub(crate) fn exit_unusable(code: i32, note: &str) -> ! {
    use std::io::Write;
    let _ = std::io::stdout().flush();
    eprintln!("roba: {note} (exit {code})");
    std::process::exit(code);
}

/// Map an anyhow error chain to a stable exit code:
/// - 0: ok (handled by the caller's happy path)
/// - 1: generic failure
/// - 2: authentication required / token invalid
/// - 3: budget ceiling exceeded
/// - 4: request timed out
/// - 5: `--max-turns` cap hit (recoverable; finish the lifecycle)
///
/// Exit code 6 ([`EXIT_UNUSABLE_RESULT`]) is NOT produced here -- it is
/// the `Ok`-path "empty / is_error result" signal set by
/// `classify_result`, not an error-chain mapping.
pub fn classify_exit_code(err: &anyhow::Error) -> i32 {
    if let Some(wrapper_err) = err.downcast_ref::<claude_wrapper::Error>() {
        match wrapper_err {
            claude_wrapper::Error::Auth { .. } => 2,
            claude_wrapper::Error::BudgetExceeded { .. } => 3,
            claude_wrapper::Error::Timeout { .. } => 4,
            // A --max-turns cap-hit is recoverable, not a hard failure: the
            // tree is usually complete and just needs the lifecycle finished
            // (gates + commit). A distinct code lets an orchestrator tell that
            // apart from a generic failure without parsing the trace. (#309;
            // claude-wrapper 0.12.0 surfaces the typed variant.)
            claude_wrapper::Error::MaxTurnsExceeded { .. } => 5,
            _ => 1,
        }
    } else {
        1
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use claude_wrapper::auth::AuthErrorKind;

    #[test]
    fn resolve_session_known_name_returns_uuid() {
        let mut sessions = std::collections::HashMap::new();
        sessions.insert("meta".to_string(), "0199-uuid".to_string());
        let uuid = resolve_session("meta", &sessions).unwrap();
        assert_eq!(uuid, "0199-uuid");
    }

    #[test]
    fn resolve_session_unknown_name_errors_and_lists_known() {
        let mut sessions = std::collections::HashMap::new();
        sessions.insert("beta".to_string(), "b".to_string());
        sessions.insert("alpha".to_string(), "a".to_string());
        let err = resolve_session("nope", &sessions).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("no session named 'nope'"), "got: {msg}");
        // Known names are listed, sorted.
        assert!(msg.contains("alpha, beta"), "got: {msg}");
    }

    #[test]
    fn resolve_session_unknown_with_empty_map_says_none_configured() {
        let sessions = std::collections::HashMap::new();
        let err = resolve_session("meta", &sessions).unwrap_err();
        assert!(err.to_string().contains("(none configured)"), "got: {err}");
    }

    fn ask_args() -> AskArgs {
        use clap::Parser;
        Cli::try_parse_from(["roba", "placeholder"]).unwrap().ask
    }

    #[test]
    fn apply_no_worktree_nulls_config_set_worktree() {
        // Simulate a profile/top-level config (or ROBA_WORKTREE) having
        // filled in a worktree value; --no-worktree forces it off.
        let mut args = ask_args();
        args.worktree = Some(Some("mybranch".to_string()));
        args.no_worktree = true;
        apply_no_worktree(&mut args);
        assert!(args.worktree.is_none());

        let mut anon = ask_args();
        anon.worktree = Some(None); // anonymous worktree from config
        anon.no_worktree = true;
        apply_no_worktree(&mut anon);
        assert!(anon.worktree.is_none());
    }

    #[test]
    fn apply_no_worktree_leaves_worktree_when_flag_unset() {
        let mut args = ask_args();
        args.worktree = Some(Some("keep".to_string()));
        args.no_worktree = false;
        apply_no_worktree(&mut args);
        assert_eq!(args.worktree, Some(Some("keep".to_string())));
    }

    #[test]
    fn is_in_git_repo_false_without_dot_git() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!is_in_git_repo(dir.path()));
    }

    #[test]
    fn is_in_git_repo_true_with_dot_git_dir() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        assert!(is_in_git_repo(dir.path()));
    }

    #[test]
    fn is_in_git_repo_walks_up_to_parent() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        let nested = dir.path().join("a").join("b");
        std::fs::create_dir_all(&nested).unwrap();
        assert!(is_in_git_repo(&nested));
    }

    #[test]
    fn classify_auth_returns_2() {
        let err = claude_wrapper::Error::Auth {
            kind: AuthErrorKind::NotAuthenticated,
            command: "claude -p hi".to_string(),
            exit_code: 1,
            message: "not logged in".to_string(),
        };
        assert_eq!(classify_exit_code(&anyhow::Error::new(err)), 2);
    }

    #[test]
    fn classify_budget_returns_3() {
        let err = claude_wrapper::Error::BudgetExceeded {
            total_usd: 5.0,
            max_usd: 4.0,
        };
        assert_eq!(classify_exit_code(&anyhow::Error::new(err)), 3);
    }

    #[test]
    fn classify_timeout_returns_4() {
        let err = claude_wrapper::Error::Timeout {
            timeout_seconds: 30,
        };
        assert_eq!(classify_exit_code(&anyhow::Error::new(err)), 4);
    }

    #[test]
    fn classify_max_turns_returns_5() {
        let err = claude_wrapper::Error::MaxTurnsExceeded {
            command: "claude --print".to_string(),
            exit_code: 1,
            max_turns: Some(40),
        };
        assert_eq!(classify_exit_code(&anyhow::Error::new(err)), 5);
    }

    #[test]
    fn classify_other_wrapper_error_returns_1() {
        let err = claude_wrapper::Error::History {
            message: "no such project".to_string(),
        };
        assert_eq!(classify_exit_code(&anyhow::Error::new(err)), 1);
    }

    #[test]
    fn classify_non_wrapper_error_returns_1() {
        let err = anyhow::anyhow!("something else broke");
        assert_eq!(classify_exit_code(&err), 1);
    }

    #[test]
    fn success_envelope_has_version_and_result() {
        let result = claude_wrapper::types::QueryResult {
            result: "hello".to_string(),
            session_id: "abc123".to_string(),
            cost_usd: None,
            duration_ms: None,
            num_turns: None,
            is_error: false,
            extra: std::collections::HashMap::new(),
        };
        let envelope = SuccessEnvelope {
            version: 1,
            result: &result,
            refusal: looks_like_refusal(&result.result),
        };
        let json = serde_json::to_string_pretty(&envelope).expect("serializes");
        let value: serde_json::Value = serde_json::from_str(&json).expect("round-trips");
        assert_eq!(value["version"], 1, "top-level version must be 1");
        assert!(
            value.get("result").is_some(),
            "result field must be present"
        );
        assert!(value.get("error").is_none(), "error field must be absent");
        assert_eq!(value["result"]["result"], "hello");
        assert_eq!(value["result"]["session_id"], "abc123");
    }

    fn query_result_with_body(body: &str) -> claude_wrapper::types::QueryResult {
        claude_wrapper::types::QueryResult {
            result: body.to_string(),
            session_id: "abc123".to_string(),
            cost_usd: None,
            duration_ms: None,
            num_turns: None,
            is_error: false,
            extra: std::collections::HashMap::new(),
        }
    }

    #[test]
    fn classify_result_usable_body_is_none() {
        let r = query_result_with_body("Here is the answer.");
        assert!(classify_result(&r).is_none());
    }

    #[test]
    fn classify_result_refusal_stays_usable() {
        // A refusal is text, so it is a usable answer (exit 0); the
        // `refusal` field, not the exit code, is how callers detect it.
        let r = query_result_with_body("I can't help with that request.");
        assert!(
            looks_like_refusal(&r.result),
            "fixture must read as refusal"
        );
        assert!(classify_result(&r).is_none());
    }

    #[test]
    fn classify_result_empty_body_is_unusable() {
        let r = query_result_with_body("");
        let (code, note) = classify_result(&r).expect("empty result is unusable");
        assert_eq!(code, EXIT_UNUSABLE_RESULT);
        assert!(note.contains("empty"), "got: {note}");
    }

    #[test]
    fn classify_result_whitespace_only_body_is_unusable() {
        let r = query_result_with_body("   \n\t  ");
        assert_eq!(
            classify_result(&r).map(|(c, _)| c),
            Some(EXIT_UNUSABLE_RESULT)
        );
    }

    #[test]
    fn classify_result_is_error_is_unusable_even_with_body() {
        // is_error wins regardless of body content.
        let mut r = query_result_with_body("partial text before the error");
        r.is_error = true;
        let (code, note) = classify_result(&r).expect("is_error is unusable");
        assert_eq!(code, EXIT_UNUSABLE_RESULT);
        assert!(note.contains("is_error"), "got: {note}");
    }

    #[test]
    fn classify_result_empty_body_with_structured_output_is_usable() {
        // The --json-schema shape: empty textual result but a populated
        // structured_output is still a usable answer.
        let mut r = query_result_with_body("");
        r.extra.insert(
            "structured_output".to_string(),
            serde_json::json!({"answer": "Paris"}),
        );
        assert!(classify_result(&r).is_none());
    }

    #[test]
    fn classify_result_empty_body_with_null_structured_output_is_unusable() {
        let mut r = query_result_with_body("");
        r.extra
            .insert("structured_output".to_string(), serde_json::Value::Null);
        assert_eq!(
            classify_result(&r).map(|(c, _)| c),
            Some(EXIT_UNUSABLE_RESULT)
        );
    }

    #[test]
    fn success_envelope_includes_refusal_field() {
        // Refusal-shaped body -> refusal: true.
        let refused = query_result_with_body("I can't help with that request.");
        let envelope = SuccessEnvelope {
            version: 1,
            result: &refused,
            refusal: looks_like_refusal(&refused.result),
        };
        let value: serde_json::Value =
            serde_json::from_str(&serde_json::to_string_pretty(&envelope).expect("serializes"))
                .expect("round-trips");
        assert_eq!(value["refusal"], true, "refusal body must flag refusal");

        // Normal body -> refusal: false.
        let answered = query_result_with_body("Here is the answer you asked for.");
        let envelope = SuccessEnvelope {
            version: 1,
            result: &answered,
            refusal: looks_like_refusal(&answered.result),
        };
        let value: serde_json::Value =
            serde_json::from_str(&serde_json::to_string_pretty(&envelope).expect("serializes"))
                .expect("round-trips");
        assert_eq!(value["refusal"], false, "normal body must not flag refusal");
    }
}
