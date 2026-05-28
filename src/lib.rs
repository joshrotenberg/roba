//! `roba` -- single-prompt CLI runner built on `claude-wrapper`.
//!
//! This lib hosts the module surface so integration tests can drive
//! the same code paths the binary uses. `main.rs` is just an entry
//! that hands a parsed [`cli::Cli`] to [`dispatch`].
//!
//! See `cli-runner.md` at the repo root for the design brainstorm.

use anyhow::{Context, Result};
use claude_wrapper::{Claude, QueryCommand};

pub mod cli;
pub mod cost;
pub mod env;
pub mod history;
pub mod output;
pub mod profile;
pub mod prompt;
pub mod render;
pub mod session;
pub mod stream;

use crate::cli::{AskArgs, Cli, SubCommand};
use crate::history::{pick_session_interactive, run_history, run_last};
use crate::output::{
    extract_code_blocks, format_footer, looks_like_refusal, path_is_json, should_show_footer,
    truncate_lines,
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
        None => run_ask(cli.ask).await,
    }
}

/// Default action: resolve a prompt, send it through claude, render
/// the result.
pub async fn run_ask(mut args: AskArgs) -> Result<()> {
    env::apply_env_overrides(&mut args);
    let pool = profile::load_pool()?;
    if let Some(chosen) = profile::resolve(&args, &pool)? {
        profile::merge_into_args(&mut args, chosen);
    }
    // --fresh is the kill switch: it cancels any continuation
    // settings that arrived via env vars or profile defaults.
    if args.fresh {
        args.continue_session = false;
    }
    if args.pick {
        let id = pick_session_interactive()?;
        eprintln!("resuming session {}", id.get(..8).unwrap_or(&id));
        args.resume = Some(id);
    }
    let main = resolve_main_prompt(args.prompt.as_deref(), args.file.as_deref(), args.editor)?;
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
        return run_streaming(&claude, prompt, &args).await;
    }

    let pre_style = render::Style::detect(&args);
    let spinner = pre_style.spinner.then(render::spinner);
    let name = session::derive_session_name(&prompt);
    let result = apply_session(QueryCommand::new(prompt).name(name), &args)
        .execute_json(&claude)
        .await?;
    if let Some(pb) = spinner {
        pb.finish_and_clear();
    }

    let file_path = args.tee.as_deref().or(args.save.as_deref());
    let want_json = args.json || file_path.is_some_and(path_is_json);
    let body = if want_json {
        serde_json::to_string_pretty(&result)?
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
    let pre_truncate_lines = body.lines().count();
    let body = truncate_lines(&body, args.head, args.tail);
    let dropped_lines = pre_truncate_lines.saturating_sub(body.lines().count());

    let style = render::Style::detect(&args);
    let write_stdout = args.save.is_none();
    if write_stdout {
        render::print_body(&body, &style);
    }
    if let Some(path) = file_path {
        std::fs::write(path, format!("{body}\n"))
            .with_context(|| format!("writing result to {}", path.display()))?;
    }
    if should_show_footer(&args) {
        render::print_meta_blank();
        if looks_like_refusal(&result.result) {
            render::print_warning("response looks like a refusal", &style);
        }
        if dropped_lines > 0 {
            let (flag, n) = if let Some(n) = args.head {
                ("--head", n)
            } else if let Some(n) = args.tail {
                ("--tail", n)
            } else {
                ("", 0)
            };
            render::print_meta(
                &format!("… {dropped_lines} more lines truncated by {flag} {n}"),
                &style,
            );
        }
        render::print_meta(&format_footer(&result), &style);
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
}
