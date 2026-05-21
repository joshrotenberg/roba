//! `cwr history`, `cwr last`, and the `--pick` interactive chooser.
//!
//! All read-only operations over `~/.claude/projects` via
//! `claude_wrapper::history`. No claude calls.

use anyhow::{Context, Result, bail};
use std::io::IsTerminal;

use crate::cli::{HistoryArgs, LastArgs};
use crate::output::{format_timestamp, truncate_arg};

/// Implementation of `cwr history`.
pub fn run_history(args: HistoryArgs) -> Result<()> {
    use claude_wrapper::history::{HistoryRoot, ListOptions, ListSort};

    let root = HistoryRoot::home().context("locating ~/.claude/projects")?;
    let limit = if args.all {
        None
    } else {
        Some(args.limit.unwrap_or(10))
    };
    let opts = ListOptions {
        limit,
        offset: 0,
        include_empty: false,
        sort: ListSort::RecencyDesc,
    };
    let sessions = root
        .list_sessions_with(args.project.as_deref(), &opts)
        .context("reading session history")?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&sessions)?);
        return Ok(());
    }

    if sessions.is_empty() {
        eprintln!("no sessions found");
        return Ok(());
    }

    println!("{:<10} {:<17} {:>5}  TITLE", "SESSION", "LAST", "MSGS");
    for s in &sessions {
        let short_id = s.session_id.get(..8).unwrap_or(&s.session_id);
        let last = s
            .last_timestamp
            .as_deref()
            .and_then(format_timestamp)
            .unwrap_or_else(|| "?".to_string());
        let title = s
            .title
            .as_deref()
            .or(s.first_user_preview.as_deref())
            .unwrap_or("(no title)");
        let title = truncate_arg(title, 60);
        println!(
            "{:<10} {:<17} {:>5}  {}",
            short_id, last, s.message_count, title
        );
    }
    Ok(())
}

/// Implementation of `cwr last`.
pub fn run_last(args: LastArgs) -> Result<()> {
    use claude_wrapper::history::{HistoryEntry, HistoryRoot, ListOptions, ListSort};

    let root = HistoryRoot::home().context("locating ~/.claude/projects")?;
    let opts = ListOptions {
        limit: Some(1),
        offset: 0,
        include_empty: false,
        sort: ListSort::RecencyDesc,
    };
    let sessions = root
        .list_sessions_with(args.project.as_deref(), &opts)
        .context("reading session history")?;
    let summary = sessions
        .first()
        .ok_or_else(|| anyhow::anyhow!("no sessions found"))?;
    let log = root
        .read_session(&summary.session_id)
        .context("reading most recent session")?;

    let last_assistant = log
        .entries
        .iter()
        .rev()
        .find_map(|entry| match entry {
            HistoryEntry::Assistant { message, .. } => Some(message),
            _ => None,
        })
        .ok_or_else(|| anyhow::anyhow!("session has no assistant entries"))?;

    if let Some(text) = extract_message_text(last_assistant) {
        println!("{text}");
    } else {
        eprintln!("(last assistant entry had no text content)");
    }

    if std::io::stderr().is_terminal() {
        let short = summary.session_id.get(..8).unwrap_or(&summary.session_id);
        let when = summary
            .last_timestamp
            .as_deref()
            .and_then(format_timestamp)
            .unwrap_or_else(|| "?".to_string());
        eprintln!();
        eprintln!(
            "session {short} . {} messages . {when}",
            summary.message_count
        );
    }
    Ok(())
}

/// Extract concatenated text content from an assistant message's
/// content blocks. Returns `None` if there are no text blocks (e.g.
/// the message was all tool_use).
pub fn extract_message_text(message: &serde_json::Value) -> Option<String> {
    let blocks = message.get("content")?.as_array()?;
    let mut out = String::new();
    for block in blocks {
        if block.get("type").and_then(|t| t.as_str()) == Some("text")
            && let Some(text) = block.get("text").and_then(|v| v.as_str())
        {
            out.push_str(text);
        }
    }
    if out.is_empty() { None } else { Some(out) }
}

/// `--pick`: open a fuzzy-filter picker over the 50 most recent
/// sessions and return the selected session id. Requires a TTY.
pub fn pick_session_interactive() -> Result<String> {
    use claude_wrapper::history::{HistoryRoot, ListOptions, ListSort};
    use dialoguer::{FuzzySelect, theme::ColorfulTheme};

    if !std::io::stdout().is_terminal() {
        bail!("--pick requires a TTY");
    }
    let root = HistoryRoot::home().context("locating ~/.claude/projects")?;
    let opts = ListOptions {
        limit: Some(50),
        offset: 0,
        include_empty: false,
        sort: ListSort::RecencyDesc,
    };
    let sessions = root
        .list_sessions_with(None, &opts)
        .context("reading session history")?;
    if sessions.is_empty() {
        bail!("no sessions to pick from");
    }
    let items: Vec<String> = sessions
        .iter()
        .map(|s| {
            let id = s.session_id.get(..8).unwrap_or(&s.session_id);
            let when = s
                .last_timestamp
                .as_deref()
                .and_then(format_timestamp)
                .unwrap_or_else(|| "?".to_string());
            let title = s
                .title
                .as_deref()
                .or(s.first_user_preview.as_deref())
                .unwrap_or("(no title)");
            format!("{id}  {when}  {}", truncate_arg(title, 60))
        })
        .collect();
    let selection = FuzzySelect::with_theme(&ColorfulTheme::default())
        .with_prompt("Pick a session to resume")
        .items(&items)
        .default(0)
        .interact()
        .context("session picker cancelled")?;
    Ok(sessions[selection].session_id.clone())
}
