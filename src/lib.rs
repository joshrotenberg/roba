//! `cwr` -- single-prompt CLI runner built on `claude-wrapper`.
//!
//! This lib hosts the module surface so integration tests can drive
//! the same code paths the binary uses. `main.rs` is just an entry
//! that hands a parsed [`cli::Cli`] to [`dispatch`].
//!
//! See `cli-runner.md` at the repo root for the design brainstorm.

use anyhow::{Context, Result};
use claude_wrapper::{Claude, QueryCommand};

pub mod cli;
pub mod history;
pub mod output;
pub mod prompt;
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
    match cli.command {
        Some(SubCommand::History(args)) => run_history(args),
        Some(SubCommand::Last(args)) => run_last(args),
        None => run_ask(cli.ask).await,
    }
}

/// Default action: resolve a prompt, send it through claude, render
/// the result.
pub async fn run_ask(mut args: AskArgs) -> Result<()> {
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

    let result = apply_session(QueryCommand::new(prompt), &args)
        .execute_json(&claude)
        .await?;

    let file_path = args.tee.as_deref().or(args.save.as_deref());
    let want_json = args.json || file_path.is_some_and(path_is_json);
    let body = if want_json {
        serde_json::to_string_pretty(&result)?
    } else if let Some(filter) = args.code.as_deref() {
        let lang = if filter.is_empty() { None } else { Some(filter) };
        extract_code_blocks(&result.result, lang)
    } else {
        result.result.clone()
    };
    let body = truncate_lines(&body, args.head, args.tail);

    let write_stdout = args.save.is_none();
    if write_stdout {
        println!("{body}");
    }
    if let Some(path) = file_path {
        std::fs::write(path, format!("{body}\n"))
            .with_context(|| format!("writing result to {}", path.display()))?;
    }
    if should_show_footer(&args) {
        eprintln!();
        if looks_like_refusal(&result.result) {
            eprintln!("warning: response looks like a refusal");
        }
        eprintln!("{}", format_footer(&result));
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
