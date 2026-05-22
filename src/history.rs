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
    let (scope, inferred_from_cwd) = resolve_project_scope(args.project.clone(), args.all_projects);
    let sessions = root
        .list_sessions_with(scope.as_deref(), &opts)
        .context("reading session history")?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&sessions)?);
        return Ok(());
    }

    if sessions.is_empty() {
        if inferred_from_cwd {
            eprintln!("no sessions in this project (use --all-projects to widen)");
        } else {
            eprintln!("no sessions found");
        }
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

    let n = args.number.unwrap_or(1);
    if n == 0 {
        return Ok(());
    }

    let root = HistoryRoot::home().context("locating ~/.claude/projects")?;
    let opts = ListOptions {
        limit: Some(1),
        offset: 0,
        include_empty: false,
        sort: ListSort::RecencyDesc,
    };
    let (scope, inferred_from_cwd) = resolve_project_scope(args.project.clone(), args.all_projects);
    let sessions = root
        .list_sessions_with(scope.as_deref(), &opts)
        .context("reading session history")?;
    let summary = sessions.first().ok_or_else(|| {
        if inferred_from_cwd {
            anyhow::anyhow!("no sessions in this project (use --all-projects to widen)")
        } else {
            anyhow::anyhow!("no sessions found")
        }
    })?;
    let log = root
        .read_session(&summary.session_id)
        .context("reading most recent session")?;

    // Expand assistant entries into per-block items so text and
    // tool_use can be filtered / counted independently.
    let mut items: Vec<Item> = Vec::new();
    for entry in &log.entries {
        if let HistoryEntry::Assistant { message, .. } = entry {
            let Some(blocks) = message.get("content").and_then(|c| c.as_array()) else {
                continue;
            };
            for block in blocks {
                match block.get("type").and_then(|t| t.as_str()) {
                    Some("text") => {
                        if let Some(t) = block.get("text").and_then(|v| v.as_str())
                            && !t.trim().is_empty()
                        {
                            items.push(Item::Text(t.to_string()));
                        }
                    }
                    Some("tool_use") => {
                        let name = block
                            .get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("?")
                            .to_string();
                        let input = block.get("input").cloned().unwrap_or(serde_json::Value::Null);
                        items.push(Item::Tool { name, input });
                    }
                    _ => {}
                }
            }
        }
    }

    let filtered: Vec<&Item> = items
        .iter()
        .filter(|it| match args.kind {
            crate::cli::LastKind::Text => matches!(it, Item::Text(_)),
            crate::cli::LastKind::Tools => matches!(it, Item::Tool { .. }),
            crate::cli::LastKind::All => true,
        })
        .collect();

    if filtered.is_empty() {
        bail!(
            "session has no {} ({} total items)",
            args.kind.label(),
            items.len()
        );
    }

    let start = filtered.len().saturating_sub(n);
    let recent = &filtered[start..];

    let style = crate::render::Style::detect_for_subcommand();
    let mut prev_text = false;
    for (i, item) in recent.iter().enumerate() {
        match item {
            Item::Text(text) => {
                // Divider between consecutive text answers so they
                // don't blur together.
                if i > 0 && prev_text {
                    crate::render::print_meta_blank();
                    crate::render::print_meta("---", &style);
                    crate::render::print_meta_blank();
                }
                crate::render::print_body(text, &style);
                prev_text = true;
            }
            Item::Tool { name, input } => {
                let summary = crate::stream::summarize_tool(name, input);
                crate::render::print_tool_call(&summary, &style);
                prev_text = false;
            }
        }
    }

    if std::io::stderr().is_terminal() {
        let short = summary.session_id.get(..8).unwrap_or(&summary.session_id);
        let when = summary
            .last_timestamp
            .as_deref()
            .and_then(format_timestamp)
            .unwrap_or_else(|| "?".to_string());
        crate::render::print_meta_blank();
        crate::render::print_meta(
            &format!(
                "session {short} . {} messages . {when} . showing {} of {} {}",
                summary.message_count,
                recent.len(),
                filtered.len(),
                args.kind.label(),
            ),
            &style,
        );
    }
    Ok(())
}

/// One renderable item from an assistant message's content blocks.
enum Item {
    Text(String),
    Tool { name: String, input: serde_json::Value },
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

/// Resolve which project slug to scope the listing to.
///
/// Returns `(scope, inferred_from_cwd)`:
/// - `(Some(slug), false)`: explicit `--project SLUG`
/// - `(None, false)`: `--all-projects` (no filter)
/// - `(Some(slug), true)`: cwd-inferred default
/// - `(None, false)`: cwd inference failed; fall back to all
pub fn resolve_project_scope(
    explicit: Option<String>,
    all_projects: bool,
) -> (Option<String>, bool) {
    if let Some(p) = explicit {
        return (Some(p), false);
    }
    if all_projects {
        return (None, false);
    }
    match current_project_slug() {
        Some(slug) => (Some(slug), true),
        None => (None, false),
    }
}

/// Encode the current cwd as a Claude Code project slug. The
/// convention is: canonicalize the cwd, then replace `/` with `-`.
/// Returns `None` if cwd can't be read or isn't valid UTF-8.
pub fn current_project_slug() -> Option<String> {
    let cwd = std::env::current_dir().ok()?;
    let canonical = cwd.canonicalize().unwrap_or(cwd);
    let s = canonical.to_str()?;
    Some(s.replace('/', "-"))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_message_text_concatenates_text_blocks() {
        let msg = serde_json::json!({
            "content": [
                {"type": "text", "text": "Hello "},
                {"type": "text", "text": "world."},
            ]
        });
        assert_eq!(extract_message_text(&msg), Some("Hello world.".to_string()));
    }

    #[test]
    fn extract_message_text_ignores_tool_use_blocks() {
        let msg = serde_json::json!({
            "content": [
                {"type": "tool_use", "name": "Read", "input": {"file_path": "x"}},
                {"type": "text", "text": "After tool"},
            ]
        });
        assert_eq!(extract_message_text(&msg), Some("After tool".to_string()));
    }

    #[test]
    fn extract_message_text_returns_none_when_only_tools() {
        let msg = serde_json::json!({
            "content": [
                {"type": "tool_use", "name": "Read", "input": {"file_path": "x"}}
            ]
        });
        assert_eq!(extract_message_text(&msg), None);
    }

    #[test]
    fn extract_message_text_returns_none_for_missing_content() {
        let msg = serde_json::json!({});
        assert_eq!(extract_message_text(&msg), None);
    }

    #[test]
    fn extract_message_text_returns_none_for_content_not_array() {
        let msg = serde_json::json!({"content": "should be an array"});
        assert_eq!(extract_message_text(&msg), None);
    }
}
