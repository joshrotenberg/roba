//! `roba` -- single-prompt CLI runner built on `claude-wrapper`.
//!
//! This lib hosts the module surface so integration tests can drive
//! the same code paths the binary uses. `main.rs` is just an entry
//! that hands a parsed [`cli::Cli`] to [`dispatch`].
//!
//! See the README for positioning and the agent ABI.

use anyhow::{Context, Result, bail};
use claude_wrapper::Claude;
use std::io::IsTerminal;

pub mod agent_check;
pub mod aliases;
pub mod bounded;
pub mod cli;
pub mod config;
pub mod cost;
pub mod detach;
pub mod doctor;
pub mod draft;
pub mod env;
pub mod error;
pub mod history;
pub mod jobs;
pub mod lint;
pub mod output;
pub mod profile;
pub mod prompt;
pub mod rates;
pub mod receipt;
pub mod render;
pub mod show;
pub mod stdin_probe;
pub mod stream;
pub mod style;
pub mod worktree;

// The clap-free run engine lives in roba-core (#416). Re-export so the rest of
// the crate keeps addressing it as `crate::engine` / `crate::session`.
use roba_core::{
    ConfigLayer, PermissionPolicy, Prompt as RunPrompt, ProviderId, RobaConfig, RunOverrides,
    RunSpec, SessionHandle, SessionSpec, ToolPolicy,
};
pub use roba_core::{engine, session};

use crate::cli::{AskArgs, Cli, SubCommand};
use crate::history::{pick_session_interactive, run_history, run_last};
use crate::output::{
    default_body, extract_code_blocks, format_footer, looks_like_refusal, path_is_json,
    should_show_footer,
};
use crate::prompt::{
    apply_vars, collect_attachments, collect_git_context, compose_prompt, merge_optional,
    resolve_main_prompt,
};
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
        Some(SubCommand::Run(args)) => bounded::run(args).await,
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
        // Derived views over run receipts (#444): read-only, no claude call.
        Some(SubCommand::Jobs(args)) => jobs::run_jobs(&args),
        Some(SubCommand::Watch(args)) => jobs::run_watch(&args),
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
            // Human-readable narrated view of the merged pool. Read-only,
            // no claude call; stdout-only, never machine-parsed.
            crate::cli::ConfigCmd::Explain(args) => config::run_explain(args),
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

// The `--json` envelope shapes live in the `roba-types` contract crate (owned
// + `Deserialize` so a downstream harness shares the exact shape). The binary
// serializes them with a `&`-borrow payload (no clone), so the JSON is
// byte-identical to before the extraction:
//
// - `SuccessEnvelope<&QueryResult>` -- the prompt-run `{ version, result,
//   refusal }` (used here and by `roba show`).
// - `VersionedResult<&T>` -- the read-only commands' `{ version, result }`
//   (cost / history / last / doctor / worktree list).
pub(crate) use roba_types::{SuccessEnvelope, VersionedResult};

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

/// Map roba's clap `--effort` value into the provider-neutral run vocabulary.
fn effort_to_run(e: cli::EffortLevel) -> roba_core::Effort {
    use cli::EffortLevel;
    match e {
        EffortLevel::Low => roba_core::Effort::Low,
        EffortLevel::Medium => roba_core::Effort::Medium,
        EffortLevel::High => roba_core::Effort::High,
        EffortLevel::Xhigh => roba_core::Effort::XHigh,
        EffortLevel::Max => roba_core::Effort::Max,
    }
}

/// Map the resolved provider-neutral effort into Claude's compatibility type.
fn run_effort_to_cw(e: roba_core::Effort) -> claude_wrapper::Effort {
    match e {
        roba_core::Effort::Low => claude_wrapper::Effort::Low,
        roba_core::Effort::Medium => claude_wrapper::Effort::Medium,
        roba_core::Effort::High => claude_wrapper::Effort::High,
        roba_core::Effort::XHigh => claude_wrapper::Effort::Xhigh,
        roba_core::Effort::Max => claude_wrapper::Effort::Max,
    }
}

/// Map roba's clap `--permission-mode` value to claude-wrapper's
/// `PermissionMode` (kept out of the core mapper so `Config` is clap-free).
fn permission_mode_to_cw(mode: cli::PermMode) -> claude_wrapper::PermissionMode {
    use claude_wrapper::PermissionMode;
    use cli::PermMode;
    match mode {
        PermMode::AcceptEdits => PermissionMode::AcceptEdits,
        PermMode::Auto => PermissionMode::Auto,
        #[allow(deprecated)]
        PermMode::BypassPermissions => PermissionMode::BypassPermissions,
        PermMode::Default => PermissionMode::Default,
        PermMode::DontAsk => PermissionMode::DontAsk,
        PermMode::Plan => PermissionMode::Plan,
    }
}

/// The bundle directory for this run: an explicit `--bundle`, else `./.roba`
/// when the roba-hermetic axis is on and it exists.
fn resolve_bundle(args: &AskArgs, roba_hermetic: bool) -> Option<std::path::PathBuf> {
    if let Some(b) = &args.bundle {
        return Some(b.clone());
    }
    if roba_hermetic {
        let dot = std::path::PathBuf::from(".roba");
        if dot.is_dir() {
            return Some(dot);
        }
    }
    None
}

/// Provide a bundle's context files to claude: `system-prompt.md` composes into
/// `--append-system-prompt`, `mcp.json` adds to `--mcp-config`. Agent provision
/// under a strict seal (`--agents` JSON) is a separate concern (#422).
fn apply_bundle_context(args: &mut AskArgs, bundle: Option<&std::path::Path>) -> Result<()> {
    let Some(dir) = bundle else {
        return Ok(());
    };
    let sp = dir.join("system-prompt.md");
    if sp.is_file() {
        let content = std::fs::read_to_string(&sp)
            .with_context(|| format!("reading bundle system prompt {}", sp.display()))?
            .trim()
            .to_string();
        if !content.is_empty() {
            args.append_system_prompt = Some(match args.append_system_prompt.take() {
                Some(existing) => format!("{existing}\n\n{content}"),
                None => content,
            });
        }
    }
    let mcp = dir.join("mcp.json");
    if mcp.is_file() {
        args.mcp_config.push(mcp.to_string_lossy().into_owned());
    }
    Ok(())
}

/// The `(roba, claude)` axes sealed by hermetic mode, honoring `--no-hermetic`.
fn hermetic_axes(args: &AskArgs) -> (bool, bool) {
    use crate::cli::HermeticWhich;
    if args.no_hermetic {
        return (false, false);
    }
    match args.hermetic {
        None => (false, false),
        Some(HermeticWhich::Both) => (true, true),
        Some(HermeticWhich::Roba) => (true, false),
        Some(HermeticWhich::Claude) => (false, true),
    }
}

fn resolve_legacy_run_spec(args: &AskArgs, prompt: String) -> Result<RunSpec> {
    let permissions = if args.full_auto {
        PermissionPolicy::FullAuto
    } else if args.writable {
        PermissionPolicy::WorkspaceWrite
    } else {
        PermissionPolicy::ReadOnly
    };
    let session = match &args.continue_session {
        Some(Some(id)) => SessionSpec::Resume {
            session: SessionHandle {
                provider: ProviderId::claude(),
                id: id.clone(),
            },
        },
        _ => SessionSpec::Fresh,
    };
    RobaConfig {
        defaults: ConfigLayer {
            provider: Some(ProviderId::claude()),
            ..ConfigLayer::default()
        },
        ..RobaConfig::default()
    }
    .resolve(
        None,
        RunOverrides {
            policy: ConfigLayer {
                model: args.model.clone(),
                effort: args.effort.map(effort_to_run),
                permissions: Some(permissions),
                tools: Some(ToolPolicy {
                    allow: args.allow_tool.clone(),
                    deny: args.deny_tool.clone(),
                }),
                max_turns: args.max_turns,
                max_cost_usd: args.max_budget_usd,
                // Legacy `--timeout 0` means disabled; the hierarchy's
                // canonical representation for disabled is absence.
                timeout_secs: args.timeout.filter(|seconds| *seconds > 0),
                ..ConfigLayer::default()
            },
            session,
            initial_prompt: Some(RunPrompt::new(prompt)?),
            ..RunOverrides::default()
        },
    )
    .map_err(Into::into)
}

/// Adapt the resolved one-shot CLI into the same hierarchical [`RunSpec`] used
/// by `roba run`, then layer only Claude-specific compatibility controls onto
/// [`engine::Config`]. Profiles and env have already populated `AskArgs`; this
/// bridge centralizes the overlapping validation and resolved vocabulary while
/// the legacy-only session, worktree, MCP, and presentation flags remain.
fn build_config(args: &AskArgs, prompt: impl Into<String>) -> Result<engine::Config> {
    use engine::{Permissions, Session};
    let spec = resolve_legacy_run_spec(args, prompt.into())?;
    let execution = spec.execution;
    let permissions = match execution.permissions {
        PermissionPolicy::ReadOnly => Permissions::ReadOnly,
        PermissionPolicy::WorkspaceWrite => Permissions::Writable,
        PermissionPolicy::FullAuto => Permissions::FullAuto,
    };
    let session = if let Some(id) = &args.session_id {
        Session::WithId(id.clone())
    } else if args.continue_session == Some(None) {
        // The provider-neutral hierarchy intentionally has no ambient
        // "most recent" selector; keep that legacy lookup as an overlay.
        Session::Continue
    } else {
        match execution.session {
            SessionSpec::Fresh => Session::Fresh,
            SessionSpec::Resume { session } => Session::Resume(session.id),
        }
    };
    // Claude-hermetic axis: seal ambient claude config. `user` is the default
    // (seals project/local ambient, keeps your global ~/.claude); an explicit
    // --setting-sources wins (e.g. `''` for a full seal).
    let (_, claude_hermetic) = hermetic_axes(args);
    Ok(engine::Config {
        prompt: spec
            .initial_prompt
            .expect("legacy bridge always supplies a prompt")
            .into_inner(),
        model: spec.agent.model,
        fallback_model: args.fallback_model.clone(),
        effort: spec.agent.effort.map(run_effort_to_cw),
        agent: args.agent.clone(),
        permissions,
        permission_mode: args.permission_mode.map(permission_mode_to_cw),
        allow_tools: execution.tools.allow,
        deny_tools: execution.tools.deny,
        session,
        fork: args.fork,
        worktree: args.worktree.clone(),
        max_turns: execution.limits.max_turns,
        max_budget_usd: execution.limits.max_cost_usd,
        timeout_secs: execution.limits.timeout_secs,
        json_schema: args.json_schema.clone(),
        system_prompt: args.system_prompt.clone(),
        append_system_prompt: args.append_system_prompt.clone(),
        agent_notice: args.agent_notice.clone(),
        no_agent_notice: args.no_agent_notice,
        no_retry: args.no_retry,
        no_session_persistence: args.no_session_persistence,
        bare: args.bare,
        safe_mode: args.safe_mode,
        add_dir: args.add_dir.clone(),
        mcp_config: args.mcp_config.clone(),
        strict_mcp_config: args.strict_mcp_config || claude_hermetic,
        setting_sources: args
            .setting_sources
            .clone()
            .or_else(|| claude_hermetic.then(|| "user".to_string())),
        exclude_dynamic_system_prompt_sections: claude_hermetic,
    })
}

pub async fn run_ask(mut args: AskArgs) -> Result<()> {
    env::apply_env_overrides(&mut args)?;
    // roba-hermetic axis: ignore roba's own ambient config (the pool walk +
    // ~/.config). A bundle (explicit --bundle, or ./.roba under --hermetic)
    // provides config instead -- the sole config when sealed, else the closest
    // layer on top of the ambient pool.
    let (roba_hermetic, _) = hermetic_axes(&args);
    let bundle = resolve_bundle(&args, roba_hermetic);
    let cwd = std::env::current_dir().context("getting current dir")?;
    let pool = profile::load_pool_with_bundle(&cwd, bundle.as_deref(), roba_hermetic)?;
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
    // Provide a bundle's context files to claude (system-prompt.md ->
    // --append-system-prompt, mcp.json -> --mcp-config); after profile
    // resolution so a bundle system prompt composes on top.
    apply_bundle_context(&mut args, bundle.as_deref())?;
    // Collapse the resolved args + composed prompt into the engine Config. Both
    // exec paths below run through it, and the engine::run public entry uses the
    // same Config -> apply_session mapper, so nothing drifts.
    let config = build_config(&args, prompt)?;

    // The anonymous-worktree-defeats-continue advisory (#328): stderr only, so
    // stdout / --json stay byte-clean. Emitted here in the CLI layer -- the
    // shared apply_session mapper (reused by the side-effect-free engine::run)
    // no longer prints it. Fires on every exec path (stream / trace / plain).
    if session::continue_defeated_by_anon_worktree(&config) {
        eprintln!(
            "warning: -c/--resume with an anonymous --worktree starts a fresh worktree each run, so there is no prior session to continue. Use a named worktree (-w NAME) or drop --worktree."
        );
    }

    let mut builder = Claude::builder();
    if let Some(secs) = args.timeout
        && secs > 0
    {
        builder = builder.timeout_secs(secs);
    }
    let claude = builder.build()?;

    if args.stream {
        run_streaming(&claude, &config, &args, stream::DisplayMode::Live).await?;
        return Ok(());
    }

    let pre_style = render::Style::detect(&args);
    let spinner = pre_style.spinner.then(render::spinner);
    let mut result = if args.trace.is_some() {
        // --trace without --stream: drive the streaming pipeline so the
        // event log can be captured, but suppress all live display and
        // render the final answer exactly as the non-streaming path
        // would (JSON envelope / --code / --out / footer below).
        match run_streaming(&claude, &config, &args, stream::DisplayMode::Silent).await? {
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
        // The non-streaming run flows through the shared engine build+execute
        // (the same Config the engine::run public entry uses), so they can't drift.
        engine::execute(&config, &claude).await?
    };
    if let Some(pb) = spinner {
        pb.finish_and_clear();
    }

    // Note observed spend for the terminal receipt (#449) as soon as the
    // result lands -- before the unusable-output exit below, so even an
    // exit-6 receipt carries what the run cost.
    if let Some(cost) = result.cost_usd {
        receipt::note_cost(cost);
    }

    // #317: with --json-schema, claude returns the schema-constrained answer
    // as a fenced JSON block in `result.result` and leaves `structured_output`
    // unset. Surface it cleanly so `--json` consumers get a real
    // `.result.structured_output` and an unfenced `.result.result`. Gated on
    // `--json-schema` so a normal answer containing a code block is untouched.
    if args.json_schema.is_some() {
        engine::surface_structured_output(&mut result);
    }

    let file_path = args.out.as_deref();
    let want_json = args.json || file_path.is_some_and(path_is_json);
    let body = if want_json {
        let envelope = SuccessEnvelope {
            version: roba_types::VERSION,
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
/// before it reaches [`crate::session::apply_session`] (-> claude's `--resume`).
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

/// Exit code for a run that did not fail but produced no usable answer:
/// the wrapper returned `Ok` with an empty or `is_error: true`
/// [`QueryResult`](claude_wrapper::types::QueryResult), OR a streaming /
/// `--trace` run completed with no result event at all. Distinct from the
/// `Err`-path codes in [`classify_exit_code`] (1-5 and 7) so an orchestrator can
/// branch on "the call did not fail, but produced nothing usable" without
/// parsing output.
///
/// Structured-signal asymmetry (deliberate). Unlike the `Err`-path codes
/// -- which have no stdout and so emit a structured error envelope on
/// stderr (`error::render_json` in `main`) -- code 6 never emits an error
/// envelope on stderr, only a plain `roba: <note> (exit 6)` line. The
/// reliable signal is the exit code itself. What is on stdout depends on
/// the subcase: for an empty / `is_error` result the success-shaped
/// `--json` envelope is already written there (inspect `.result` /
/// `.is_error`); for the no-result-event subcase, which exits before the
/// envelope is rendered, stdout is empty. So a `--json` consumer branches
/// on the exit code (and reads the stdout envelope when present); it does
/// NOT scrape stderr for an error envelope on code 6.
///
/// Defined in [`roba_types`] (the shared contract) and re-exported here so the
/// binary and the published crate use the one constant.
pub use roba_types::EXIT_UNUSABLE_RESULT;

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
    // One of the three seams where a detached child's typed exit code is
    // known; record it so `roba show` reports the outcome instead of
    // reconstructing success (#441). A no-op for a foreground run.
    receipt::finish(code);
    std::process::exit(code);
}

/// Observed spend carried by a typed error, when the failure happened after
/// claude emitted a result event: the cap-hit variants parse
/// `total_cost_usd` out of it. `None` for every other error -- unknown,
/// never zero. Surfaced so the error exit seam can note it for the terminal
/// receipt (#449).
pub fn error_cost_usd(err: &anyhow::Error) -> Option<f64> {
    match err.downcast_ref::<claude_wrapper::Error>()? {
        claude_wrapper::Error::MaxTurnsExceeded { cost_usd, .. }
        | claude_wrapper::Error::MaxBudgetExceeded { cost_usd, .. } => *cost_usd,
        _ => None,
    }
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
    use roba_types::{
        EXIT_AUTH, EXIT_BUDGET, EXIT_FAILURE, EXIT_MAX_BUDGET, EXIT_MAX_TURNS, EXIT_TIMEOUT,
    };
    if let Some(wrapper_err) = err.downcast_ref::<claude_wrapper::Error>() {
        match wrapper_err {
            claude_wrapper::Error::Auth { .. } => EXIT_AUTH,
            claude_wrapper::Error::BudgetExceeded { .. } => EXIT_BUDGET,
            claude_wrapper::Error::Timeout { .. } => EXIT_TIMEOUT,
            // A --max-turns cap-hit is recoverable, not a hard failure: the
            // tree is usually complete and just needs the lifecycle finished
            // (gates + commit). A distinct code lets an orchestrator tell that
            // apart from a generic failure without parsing the trace. (#309;
            // claude-wrapper 0.12.0 surfaces the typed variant.)
            claude_wrapper::Error::MaxTurnsExceeded { .. } => EXIT_MAX_TURNS,
            // A --max-budget-usd cap-hit is the same shape as max-turns: a
            // guardrail tripped mid-run, not a defect. The tree is usually
            // intact (detection is post-hoc, so the run may have completed the
            // work before tripping). A distinct, recoverable code lets an
            // orchestrator resume the session and finish the lifecycle rather
            // than treating the spend ceiling as a hard failure. (#388;
            // claude-wrapper 0.12.3 surfaces the typed variant.)
            claude_wrapper::Error::MaxBudgetExceeded { .. } => EXIT_MAX_BUDGET,
            _ => EXIT_FAILURE,
        }
    } else {
        EXIT_FAILURE
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use claude_wrapper::auth::AuthErrorKind;

    fn ask(argv: &[&str]) -> AskArgs {
        use clap::Parser;
        cli::Cli::try_parse_from(argv).unwrap().ask
    }

    #[test]
    fn hermetic_axes_map_which() {
        assert_eq!(hermetic_axes(&ask(&["roba", "p"])), (false, false));
        assert_eq!(
            hermetic_axes(&ask(&["roba", "--hermetic", "p"])),
            (true, true)
        );
        assert_eq!(
            hermetic_axes(&ask(&["roba", "--hermetic=roba", "p"])),
            (true, false)
        );
        assert_eq!(
            hermetic_axes(&ask(&["roba", "--hermetic=claude", "p"])),
            (false, true)
        );
    }

    #[test]
    fn hermetic_no_hermetic_cancels() {
        let mut a = ask(&["roba", "--no-hermetic", "p"]);
        a.hermetic = Some(cli::HermeticWhich::Both);
        assert_eq!(hermetic_axes(&a), (false, false));
    }

    #[test]
    fn hermetic_claude_axis_sets_the_seal() {
        let c = build_config(&ask(&["roba", "--hermetic", "p"]), "p").unwrap();
        assert_eq!(c.setting_sources.as_deref(), Some("user"));
        assert!(c.strict_mcp_config);
        assert!(c.exclude_dynamic_system_prompt_sections);
    }

    #[test]
    fn hermetic_explicit_setting_sources_overrides_default() {
        let c = build_config(
            &ask(&["roba", "--hermetic", "--setting-sources", "", "p"]),
            "p",
        )
        .unwrap();
        assert_eq!(c.setting_sources.as_deref(), Some(""));
    }

    #[test]
    fn hermetic_roba_axis_only_leaves_claude_seal_off() {
        let c = build_config(&ask(&["roba", "--hermetic=roba", "p"]), "p").unwrap();
        assert_eq!(c.setting_sources, None);
        assert!(!c.strict_mcp_config);
        assert!(!c.exclude_dynamic_system_prompt_sections);
    }

    #[test]
    fn apply_bundle_context_composes_system_prompt_and_mcp() {
        let tmp = tempfile::tempdir().unwrap();
        let bundle = tmp.path();
        std::fs::write(bundle.join("system-prompt.md"), "  be terse  ").unwrap();
        std::fs::write(bundle.join("mcp.json"), "{}").unwrap();

        // No prior append: the (trimmed) bundle system prompt becomes it.
        let mut a = ask(&["roba", "p"]);
        apply_bundle_context(&mut a, Some(bundle)).unwrap();
        assert_eq!(a.append_system_prompt.as_deref(), Some("be terse"));
        assert_eq!(a.mcp_config.len(), 1);
        assert!(a.mcp_config[0].ends_with("mcp.json"));

        // Prior append (e.g. from a profile): the bundle composes on top.
        let mut a = ask(&["roba", "--append-system-prompt", "first", "p"]);
        apply_bundle_context(&mut a, Some(bundle)).unwrap();
        assert_eq!(a.append_system_prompt.as_deref(), Some("first\n\nbe terse"));
    }

    #[test]
    fn apply_bundle_context_noop_without_files() {
        let mut a = ask(&["roba", "p"]);
        apply_bundle_context(&mut a, None).unwrap();
        let empty = tempfile::tempdir().unwrap();
        apply_bundle_context(&mut a, Some(empty.path())).unwrap();
        assert!(a.append_system_prompt.is_none());
        assert!(a.mcp_config.is_empty());
    }

    #[test]
    fn build_config_collapses_permission_posture() {
        use engine::Permissions;
        // The flat --readonly/--writable/--full-auto flags collapse into the
        // curated posture: full_auto wins, then writable, else read-only.
        assert!(matches!(
            build_config(&ask(&["roba", "p"]), "p").unwrap().permissions,
            Permissions::ReadOnly
        ));
        assert!(matches!(
            build_config(&ask(&["roba", "--writable", "p"]), "p")
                .unwrap()
                .permissions,
            Permissions::Writable
        ));
        assert!(matches!(
            build_config(&ask(&["roba", "--full-auto", "p"]), "p")
                .unwrap()
                .permissions,
            Permissions::FullAuto
        ));
    }

    #[test]
    fn build_config_collapses_session_selector() {
        use engine::Session;
        assert!(matches!(
            build_config(&ask(&["roba", "p"]), "p").unwrap().session,
            Session::Fresh
        ));
        assert!(matches!(
            build_config(&ask(&["roba", "-c", "-p", "x"]), "p")
                .unwrap()
                .session,
            Session::Continue
        ));
        let uuid = "12345678-1234-4234-8234-123456789abc";
        assert!(matches!(
            build_config(&ask(&["roba", &format!("--continue={uuid}"), "p"]), "p")
                .unwrap()
                .session,
            Session::Resume(id) if id == uuid
        ));
        assert!(matches!(
            build_config(&ask(&["roba", "--session-id", uuid, "p"]), "p")
                .unwrap()
                .session,
            Session::WithId(id) if id == uuid
        ));
    }

    #[test]
    fn build_config_carries_the_prompt_and_pass_through_knobs() {
        let c = build_config(
            &ask(&["roba", "--max-turns", "5", "--add-dir", "/repo", "p"]),
            "composed prompt",
        )
        .unwrap();
        assert_eq!(c.prompt, "composed prompt");
        assert_eq!(c.max_turns, Some(5));
        assert_eq!(c.add_dir, vec!["/repo".to_string()]);
    }

    #[test]
    fn legacy_bridge_resolves_overlapping_policy_through_run_spec() {
        let c = build_config(
            &ask(&[
                "roba",
                "--model",
                "claude-test",
                "--effort",
                "xhigh",
                "--full-auto",
                "--allow-tool",
                "Read",
                "--deny-tool",
                "Bash",
                "--max-turns",
                "5",
                "--max-budget-usd",
                "1.25",
                "--timeout",
                "30",
                "p",
            ]),
            "composed prompt",
        )
        .unwrap();

        assert_eq!(c.prompt, "composed prompt");
        assert_eq!(c.model.as_deref(), Some("claude-test"));
        assert_eq!(c.effort, Some(claude_wrapper::Effort::Xhigh));
        assert!(matches!(c.permissions, engine::Permissions::FullAuto));
        assert_eq!(c.allow_tools, vec!["Read"]);
        assert_eq!(c.deny_tools, vec!["Bash"]);
        assert_eq!(c.max_turns, Some(5));
        assert_eq!(c.max_budget_usd, Some(1.25));
        assert_eq!(c.timeout_secs, Some(30));
    }

    #[test]
    fn legacy_bridge_preserves_native_claude_agent_as_an_overlay() {
        let c = build_config(&ask(&["roba", "--agent", "claude-native-agent", "p"]), "p").unwrap();
        assert_eq!(c.agent.as_deref(), Some("claude-native-agent"));
    }

    #[test]
    fn legacy_timeout_zero_normalizes_to_no_deadline() {
        let c = build_config(&ask(&["roba", "--timeout", "0", "p"]), "p").unwrap();
        assert_eq!(c.timeout_secs, None);
    }

    #[test]
    fn legacy_bridge_rejects_invalid_shared_policy() {
        let mut args = ask(&["roba", "p"]);
        args.max_turns = Some(0);
        let err = build_config(&args, "p").unwrap_err();
        assert!(
            err.to_string()
                .contains("max_turns must be greater than zero")
        );
    }

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
        // The rail-stop variants are #[non_exhaustive] (claude-wrapper 0.13,
        // #669), so build them via the public path: from_command_failure
        // detects the terminal result event's subtype on stdout.
        let stdout = r#"{"type":"result","subtype":"error_max_turns","is_error":true,"num_turns":40,"result":"Reached maximum number of turns (40)"}"#;
        let err = claude_wrapper::Error::from_command_failure(
            "claude --print".to_string(),
            1,
            stdout.to_string(),
            String::new(),
            None,
        );
        assert_eq!(classify_exit_code(&anyhow::Error::new(err)), 5);
    }

    #[test]
    fn classify_max_budget_returns_7() {
        let stdout = r#"{"type":"result","subtype":"error_max_budget_usd","is_error":true,"result":"Reached maximum budget ($0.01)"}"#;
        let err = claude_wrapper::Error::from_command_failure(
            "claude --print".to_string(),
            1,
            stdout.to_string(),
            String::new(),
            None,
        );
        assert_eq!(classify_exit_code(&anyhow::Error::new(err)), 7);
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
            usage: None,
            is_error: false,
            extra: std::collections::HashMap::new(),
        };
        let envelope = SuccessEnvelope {
            version: roba_types::VERSION,
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
            usage: None,
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
