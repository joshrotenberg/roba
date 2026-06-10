//! `roba history`, `roba last`, and the `--pick` interactive chooser.
//!
//! All read-only operations over `~/.claude/projects` via
//! `claude_wrapper::history`. No claude calls.

use anyhow::{Context, Result, bail};
use std::io::IsTerminal;

use crate::cli::{HistoryArgs, LastArgs};
use crate::output::{format_timestamp, truncate_arg};

/// Upper bound on worktree-slug sessions whose JSONL is read when
/// matching `--worktree`. After the cheap slug pre-filter narrows the
/// candidate set to worktree-run sessions (see [`slug_is_worktree`]),
/// each survivor's JSONL is read for its cwd; this caps that work. If
/// the worktree-slug candidate set itself exceeds the cap, a note is
/// printed (no silent truncation).
const WORKTREE_SCAN_CAP: usize = 1000;

/// Implementation of `roba history`.
pub fn run_history(args: HistoryArgs) -> Result<()> {
    use claude_wrapper::history::HistoryRoot;

    let root = HistoryRoot::home().context("locating ~/.claude/projects")?;

    // --paths: emit one absolute JSONL path per line, then return.
    if let Some(paths_n) = args.paths {
        let (sessions, _) = list_for_history(&root, &args, paths_n)?;
        for s in &sessions {
            let path = root
                .path()
                .join(&s.project_slug)
                .join(format!("{}.jsonl", s.session_id));
            println!("{}", path.display());
        }
        return Ok(());
    }

    let limit = if args.all {
        None
    } else {
        Some(args.limit.unwrap_or(10))
    };
    let (sessions, inferred_from_cwd) = list_for_history(&root, &args, limit)?;

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

/// List the sessions for `roba history`, applying the `--worktree`
/// filter when present.
///
/// Without `--worktree`: the normal scoped listing (cwd-inferred,
/// `--project`, or `--all-projects`) at `limit`.
///
/// With `--worktree NAME`: a cross-project scan (worktree sessions live
/// under their own project slugs, so `--project` is ignored), capped at
/// [`WORKTREE_SCAN_CAP`], retaining up to `limit` sessions that ran in
/// that worktree. Reads each candidate's JSONL read-only. Returns
/// `(sessions, inferred_from_cwd)`.
fn list_for_history(
    root: &claude_wrapper::history::HistoryRoot,
    args: &HistoryArgs,
    limit: Option<usize>,
) -> Result<(Vec<claude_wrapper::history::SessionSummary>, bool)> {
    use claude_wrapper::history::{ListOptions, ListSort};

    if let Some(name) = args.worktree.as_deref() {
        // Enumerate every session summary (cheap: metadata only), then
        // pre-narrow to worktree-run sessions by project slug before
        // reading any JSONL for its cwd.
        //
        // The slug pre-filter is SOUND: a worktree-run session's cwd is
        // `<repo>/.claude/worktrees/<name>`, which claude-wrapper encodes
        // into a project slug containing the substring `claude-worktrees`
        // (the `/` path separators become `-`). The cwd rule that decides
        // the actual match (`session_worktree`, via `session_in_worktree`)
        // ONLY matches sessions whose cwd is under `.claude/worktrees/` --
        // exactly the worktree-slug set. So the slug filter's domain is a
        // superset of the cwd rule's: it can only skip sessions that could
        // never match (no false negatives). The cwd check stays the source
        // of truth; the slug filter is a pre-narrowing only.
        let scan_opts = ListOptions {
            limit: None,
            offset: 0,
            include_empty: false,
            sort: ListSort::RecencyDesc,
        };
        let candidates: Vec<_> = root
            .list_sessions_with(None, &scan_opts)
            .context("reading session history")?
            .into_iter()
            .filter(|s| slug_is_worktree(&s.project_slug))
            .collect();
        // The cap now bounds the worktree-slug candidate set, so the
        // note fires only when there are genuinely more worktree
        // sessions than the cap -- not for ordinary sparse matches.
        let capped = candidates.len() > WORKTREE_SCAN_CAP;
        let mut matched = Vec::new();
        for s in candidates.into_iter().take(WORKTREE_SCAN_CAP) {
            if limit.is_some_and(|n| matched.len() >= n) {
                break;
            }
            if session_in_worktree(root, &s, name) {
                matched.push(s);
            }
        }
        if capped && limit.is_none_or(|n| matched.len() < n) {
            eprintln!(
                "note: scanned only the {WORKTREE_SCAN_CAP} most recent worktree sessions for worktree `{name}`; older sessions were not checked"
            );
        }
        return Ok((matched, false));
    }

    let (scope, inferred) = resolve_project_scope(args.project.clone(), args.all_projects);
    let opts = ListOptions {
        limit,
        offset: 0,
        include_empty: false,
        sort: ListSort::RecencyDesc,
    };
    let sessions = root
        .list_sessions_with(scope.as_deref(), &opts)
        .context("reading session history")?;
    Ok((sessions, inferred))
}

/// True if a session ran in the worktree named `name`. Reads the
/// session's JSONL (built from its project slug + id, as `--paths` and
/// `roba show` do) and checks the first `user` entry that carries a
/// `cwd`. Read-only and best-effort: an unreadable or worktree-less
/// session simply doesn't match (never panics).
fn session_in_worktree(
    root: &claude_wrapper::history::HistoryRoot,
    s: &claude_wrapper::history::SessionSummary,
    name: &str,
) -> bool {
    use std::io::BufRead;

    let path = root
        .path()
        .join(&s.project_slug)
        .join(format!("{}.jsonl", s.session_id));
    let Ok(file) = std::fs::File::open(&path) else {
        return false;
    };
    // The cwd is stable per session, so the first `user` entry that
    // carries one settles the match. Read line-by-line and stop there
    // rather than slurping the whole (often large) JSONL.
    for line in std::io::BufReader::new(file).lines().map_while(Result::ok) {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        if value.get("type").and_then(|t| t.as_str()) == Some("user")
            && let Some(cwd) = value.get("cwd").and_then(|c| c.as_str())
        {
            return session_worktree(cwd) == Some(name);
        }
    }
    false
}

/// Extract a worktree name from a session's working-directory path.
///
/// Runner worktrees live at `<repo>/.claude/worktrees/<name>` (the dirs
/// `roba worktree list` enumerates). Returns the `<name>` segment right
/// after `.claude/worktrees/`, ignoring any trailing subdirectories
/// (`.../worktrees/foo/sub` -> `foo`). Returns `None` for a cwd that
/// isn't inside a worktree.
fn session_worktree(cwd: &str) -> Option<&str> {
    let after = cwd.split(".claude/worktrees/").nth(1)?;
    let name = after.split('/').next()?;
    if name.is_empty() { None } else { Some(name) }
}

/// True if a project slug belongs to a worktree-run session.
///
/// claude-wrapper derives the project slug from the session's cwd by
/// replacing path separators with `-`, so a worktree cwd
/// `<repo>/.claude/worktrees/<name>` yields a slug containing the
/// substring `claude-worktrees` (e.g. `-repo--claude-worktrees-foo`).
/// This is the slug-level mirror of [`session_worktree`]: every cwd
/// `session_worktree` can match lives under `.claude/worktrees/`, so its
/// slug contains `claude-worktrees`. The set this returns `true` for is
/// therefore a superset of the matchable sessions, which is what makes
/// it a sound pre-filter (no false negatives).
fn slug_is_worktree(slug: &str) -> bool {
    slug.contains("claude-worktrees")
}

/// Implementation of `roba last`.
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
                        let input = block
                            .get("input")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null);
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
                let summary = crate::output::summarize_tool(name, input);
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
    Tool {
        name: String,
        input: serde_json::Value,
    },
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

/// Fetch up to `n` recent assistant text responses from the most
/// recent session in the current cwd's project. Returns oldest-first
/// (so direct concatenation reads chronologically). Returns
/// `Ok(vec![])` when no session exists, no text content was found,
/// or `n == 0`.
///
/// Used by the editor compose flow to pre-fill the buffer with prior
/// context (see `prompt::build_editor_preamble`).
pub fn last_n_assistant_texts_in_cwd(n: usize) -> Result<Vec<String>> {
    use claude_wrapper::history::{HistoryEntry, HistoryRoot, ListOptions, ListSort};

    if n == 0 {
        return Ok(Vec::new());
    }
    let Some(slug) = current_project_slug() else {
        return Ok(Vec::new());
    };
    let root = HistoryRoot::home().context("locating ~/.claude/projects")?;
    let opts = ListOptions {
        limit: Some(1),
        offset: 0,
        include_empty: false,
        sort: ListSort::RecencyDesc,
    };
    let sessions = root
        .list_sessions_with(Some(&slug), &opts)
        .context("reading session history")?;
    let Some(summary) = sessions.first() else {
        return Ok(Vec::new());
    };
    let log = root
        .read_session(&summary.session_id)
        .context("reading most recent session")?;

    let mut texts: Vec<String> = Vec::new();
    for entry in &log.entries {
        if let HistoryEntry::Assistant { message, .. } = entry
            && let Some(t) = extract_message_text(message)
            && !t.trim().is_empty()
        {
            texts.push(t);
        }
    }
    let start = texts.len().saturating_sub(n);
    Ok(texts[start..].to_vec())
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

    #[test]
    fn session_worktree_extracts_name() {
        assert_eq!(
            session_worktree(
                "/Users/jo/Code/active/agent-tools/.claude/worktrees/agent-a1ce841658fc4a90d"
            ),
            Some("agent-a1ce841658fc4a90d")
        );
    }

    #[test]
    fn session_worktree_ignores_trailing_subdirs() {
        assert_eq!(
            session_worktree("/repo/.claude/worktrees/foo/src/lib"),
            Some("foo")
        );
    }

    #[test]
    fn session_worktree_none_for_non_worktree_cwd() {
        assert_eq!(session_worktree("/Users/jo/Code/active/agent-tools"), None);
        assert_eq!(session_worktree("/repo/.claude/projects/whatever"), None);
    }

    #[test]
    fn session_worktree_none_for_empty_trailing_segment() {
        // A path ending exactly at the marker has no name segment.
        assert_eq!(session_worktree("/repo/.claude/worktrees/"), None);
    }

    #[test]
    fn slug_is_worktree_true_for_worktree_slug() {
        // Real-shape slugs (cwd separators collapsed to `-`).
        assert!(slug_is_worktree("-repo--claude-worktrees-foo"));
        assert!(slug_is_worktree(
            "-Users-jo-Code-active-agent-tools--claude-worktrees-agent-a1ce841658fc4a90d"
        ));
    }

    #[test]
    fn slug_is_worktree_false_for_base_slug() {
        // A base-repo session's slug has no worktree marker, so the
        // pre-filter skips it -- and the cwd rule could never match it.
        assert!(!slug_is_worktree("-repo"));
        assert!(!slug_is_worktree("-Users-jo-Code-active-agent-tools"));
    }

    #[test]
    fn paths_output_construction() {
        // Verify the path construction logic: root_path / project_slug / session_id.jsonl
        // Use a relative root to keep the assertion OS-agnostic (avoids path-separator
        // differences between Unix and Windows).
        let root_path = std::path::Path::new("projects");
        let project_slug = "-Users-alice-myproject";
        let session_id = "abc12345";
        let path = root_path
            .join(project_slug)
            .join(format!("{}.jsonl", session_id));
        // Verify structural components, not the full OS-specific string.
        assert_eq!(path.parent().unwrap().file_name().unwrap(), project_slug);
        assert_eq!(
            path.file_name().unwrap().to_str().unwrap(),
            "abc12345.jsonl"
        );
    }
}
