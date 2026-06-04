//! `roba` -- single-prompt CLI runner built on `claude-wrapper`.
//!
//! This lib hosts the module surface so integration tests can drive
//! the same code paths the binary uses. `main.rs` is just an entry
//! that hands a parsed [`cli::Cli`] to [`dispatch`].
//!
//! See `cli-runner.md` at the repo root for the design brainstorm.

use anyhow::{Context, Result, bail};
use claude_wrapper::{Claude, QueryCommand};
use std::io::IsTerminal;

pub mod agent_check;
pub mod aliases;
pub mod cli;
pub mod cost;
pub mod env;
pub mod error;
pub mod history;
pub mod output;
pub mod profile;
pub mod prompt;
pub mod rates;
pub mod render;
pub mod serve;
pub mod session;
pub mod stream;

use crate::cli::{AskArgs, Cli, SubCommand};
use crate::history::{pick_session_interactive, run_history, run_last};
use crate::output::{
    extract_code_blocks, format_footer, looks_like_refusal, path_is_json, should_show_footer,
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
        Some(SubCommand::Profile { action }) => profile::run(action),
        Some(SubCommand::Cost(args)) => cost::run(args),
        Some(SubCommand::Alias { action }) => aliases::run(action),
        Some(SubCommand::Serve(args)) => serve::run_serve(args).await,
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
            // (`roba commit-msg`). Otherwise it's a normal prompt.
            if let Some(name) = aliases::bare_alias_candidate(&cli.ask)? {
                let trailing = aliases::trailing_args_from_env(&name);
                aliases::dispatch_alias(&name, &trailing).await
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
#[derive(serde::Serialize)]
struct SuccessEnvelope<'a> {
    version: u32,
    result: &'a claude_wrapper::types::QueryResult,
    /// True when [`crate::output::looks_like_refusal`] matched the
    /// response body. Lets non-TTY consumers (the ones that never see
    /// the human-facing footer warning) branch on "got an answer" vs
    /// "got refused" without parsing the body text. Additive v1 field.
    refusal: bool,
}

/// Default action: resolve a prompt, send it through claude, render
/// the result.
pub async fn run_ask(mut args: AskArgs) -> Result<()> {
    env::apply_env_overrides(&mut args);
    let pool = profile::load_pool()?;
    if let Some(chosen) = profile::resolve(&args, &pool)? {
        let source = profile::profile_source_label(&args, &pool);
        profile::merge_into_args(&mut args, chosen, &source);
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
    ensure_interactive_for_flags(&args)?;
    if args.pick {
        let id = pick_session_interactive()?;
        eprintln!("resuming session {}", id.get(..8).unwrap_or(&id));
        args.continue_session = Some(Some(id));
    }
    // Agent frontmatter permission check: warn if the agent declares
    // tools that are not in the resolved allowlist. Best-effort and
    // non-blocking -- dispatch still proceeds.
    let cwd = std::env::current_dir().unwrap_or_default();
    agent_check::maybe_warn(&args, &cwd);

    // `-p / --prompt` is an explicit alternative to the positional
    // prompt (clap enforces the mutual exclusion via conflicts_with).
    // Whichever was supplied is the explicit prompt string; the rest of
    // the precedence (stdin > editor > explicit > file > none) is
    // unchanged.
    let explicit_prompt = args.prompt_flag.as_deref().or(args.prompt.as_deref());
    let main = resolve_main_prompt(
        explicit_prompt,
        args.file.as_deref(),
        args.editor,
        args.editor_history,
    )?;
    let attachments = collect_attachments(&args.attach)?;
    let git_context = collect_git_context(&args)?;
    let context = merge_optional(attachments, git_context);
    let prompt = compose_prompt(main, &args.prepend, context, &args.append)?;
    let prompt = apply_vars(prompt, &args.var);
    if args.echo && !args.quiet {
        eprintln!("{prompt}");
        eprintln!();
        eprintln!("---");
        eprintln!();
    }
    let claude = Claude::builder().build()?;

    if args.stream {
        run_streaming(&claude, prompt, &args, stream::DisplayMode::Live).await?;
        return Ok(());
    }

    let pre_style = render::Style::detect(&args);
    let spinner = pre_style.spinner.then(render::spinner);
    let result = if args.trace.is_some() {
        // --trace without --stream: drive the streaming pipeline so the
        // event log can be captured, but suppress all live display and
        // render the final answer exactly as the non-streaming path
        // would (JSON envelope / --code / --out / footer below).
        match run_streaming(&claude, prompt, &args, stream::DisplayMode::Silent).await? {
            Some(r) => r,
            None => bail!("streaming completed without a result event"),
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
        result.result.clone()
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
            &format_footer(&result, rates.as_ref(), args.no_dollars),
            &style,
        );
    }
    Ok(())
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

/// Map an anyhow error chain to a stable exit code:
/// - 0: ok (handled by the caller's happy path)
/// - 1: generic failure
/// - 2: authentication required / token invalid
/// - 3: budget ceiling exceeded
/// - 4: request timed out
pub fn classify_exit_code(err: &anyhow::Error) -> i32 {
    if let Some(wrapper_err) = err.downcast_ref::<claude_wrapper::Error>() {
        match wrapper_err {
            claude_wrapper::Error::Auth { .. } => 2,
            claude_wrapper::Error::BudgetExceeded { .. } => 3,
            claude_wrapper::Error::Timeout { .. } => 4,
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
