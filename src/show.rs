//! `roba show <SESSION_ID>` -- read-only result handle for a stored
//! session.
//!
//! Reconstructs a result from a session's on-disk JSONL via
//! `claude_wrapper::history::HistoryRoot::read_session`, which finds
//! `<id>.jsonl` across every project directory. Read-only: it reads the
//! session log and reports; it never writes under `.claude/`.
//!
//! The envelope is RECONSTRUCTED, not replayed: it is structurally
//! identical to a live `roba --json` envelope but NOT byte-identical.
//! `duration_ms` is always null (claude does not persist per-run wall
//! time), and `cost_usd` / `num_turns` are DERIVED from the log (a token
//! rollup against the bundled rate table, and a count of assistant
//! turns), not the original run's reported values.

use anyhow::{Context, Result, bail};
use std::collections::HashMap;
use std::time::{Duration, Instant};

use claude_wrapper::history::{HistoryEntry, HistoryRoot, SessionLog};
use claude_wrapper::types::QueryResult;

use crate::SuccessEnvelope;
use crate::cost::{Usage, cost_breakdown, usage_by_model};
use crate::history::extract_message_text;
use crate::output::{format_count, looks_like_refusal, truncate_arg};
use crate::rates::Rates;
use crate::render;

/// Default `--wait` timeout, in seconds. Bounds every wait so `--wait`
/// can never hang indefinitely unless the user explicitly asks for it
/// with `--timeout 0`.
const DEFAULT_WAIT_TIMEOUT_SECS: u64 = 600;

/// How often `--wait` re-reads the session JSONL. A small constant;
/// not configurable for v1.
const POLL_INTERVAL: Duration = Duration::from_secs(1);

/// Implementation of `roba show`.
pub fn run(args: &crate::cli::ShowArgs) -> Result<()> {
    let root = HistoryRoot::home().context("locating ~/.claude/projects")?;

    let log = if args.wait {
        // `--wait` gates WHEN we render: poll until the run completes (or
        // the timeout fires), then fall through to the normal render
        // path below. A not-yet-written session is treated as "not
        // started yet" rather than an immediate not-found error.
        wait_for_complete(&root, args)?
    } else {
        // `read_session` only fails when no `<id>.jsonl` exists anywhere,
        // so map any failure to a clean not-found error (never a panic).
        match root.read_session(&args.session_id) {
            Ok(log) => log,
            Err(_) => bail!("session `{}` not found", args.session_id),
        }
    };

    render_session(&root, &log, args)
}

/// Poll the session's on-disk JSONL until it both exists AND looks
/// complete (see [`is_complete`]), then return the parsed log. Bounded
/// by `args.timeout` (default [`DEFAULT_WAIT_TIMEOUT_SECS`]; `0` waits
/// indefinitely) so it never hangs unbounded -- including the case where
/// the session hasn't written its JSONL yet.
fn wait_for_complete(root: &HistoryRoot, args: &crate::cli::ShowArgs) -> Result<SessionLog> {
    let timeout_secs = args.timeout.unwrap_or(DEFAULT_WAIT_TIMEOUT_SECS);
    let deadline = (timeout_secs != 0).then(|| Instant::now() + Duration::from_secs(timeout_secs));

    loop {
        // A missing session under `--wait` means "not started yet", not
        // an error: keep polling until it appears or the timeout fires.
        if let Ok(log) = root.read_session(&args.session_id)
            && is_complete(&log)
        {
            return Ok(log);
        }
        if let Some(deadline) = deadline
            && Instant::now() >= deadline
        {
            bail!(
                "waited {timeout_secs}s for session `{}` to complete",
                args.session_id
            );
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// Best-effort completion heuristic over claude's session log.
///
/// There is no explicit "done" marker persisted to the JSONL, so this
/// reads the most reliable in-band signal: the LAST `assistant` entry's
/// `message.stop_reason`. A terminal reason (`end_turn`, `stop_sequence`,
/// `max_tokens` -- anything that is not `tool_use`) means the turn
/// finished; `tool_use` means a tool call is pending and more entries are
/// coming (a tool_result, then another assistant turn). A null/missing
/// `stop_reason`, or no assistant entry at all, is treated as not-yet-
/// complete. Trailing non-assistant entries (`agent-name`, summaries)
/// land in `HistoryEntry::Other` and are skipped -- we look past them to
/// the last real assistant turn.
///
/// This is more reliable than file-mtime quiescence, which falsely fires
/// during a long tool call (a 30s build writes nothing to the JSONL, so
/// mtime goes quiet mid-run). It is still best-effort, not a guarantee --
/// consistent with how the reconstructed envelope is documented.
fn is_complete(log: &SessionLog) -> bool {
    let last_assistant = log.entries.iter().rev().find_map(|e| match e {
        HistoryEntry::Assistant { message, .. } => Some(message),
        _ => None,
    });
    match last_assistant
        .and_then(|m| m.get("stop_reason"))
        .and_then(|v| v.as_str())
    {
        Some("tool_use") => false,
        Some(_) => true,
        None => false,
    }
}

/// Reconstruct + render a parsed session log -- the existing `show`
/// output path, unchanged by `--wait` (which only gates when we get
/// here). Reused by both the immediate and the poll-until-complete entry
/// points so the rendering logic lives in exactly one place.
fn render_session(root: &HistoryRoot, log: &SessionLog, args: &crate::cli::ShowArgs) -> Result<()> {
    let (result_text, num_turns) = reconstruct_answer(log);

    // Per-model token rollup, read from the same JSONL the summary came
    // from. Best-effort: an unreadable file yields an empty rollup, which
    // leaves `cost_usd` None rather than fabricating a figure.
    let jsonl_path = root
        .path()
        .join(&log.project_slug)
        .join(format!("{}.jsonl", log.session_id));
    let by_model = std::fs::read_to_string(&jsonl_path)
        .map(|text| usage_by_model(&text))
        .unwrap_or_default();

    // Derive the dollar total from the rollup. `cost_breakdown` returns
    // `None` when no model in the log matched the table -- we propagate
    // that rather than costing unknown models at a misleading $0.
    let rates = Rates::resolve(None).ok();
    let cost_usd = rates.as_ref().and_then(|r| cost_breakdown(&by_model, r).1);

    let qr = QueryResult {
        result: result_text,
        session_id: log.session_id.clone(),
        cost_usd,
        // duration is not persisted per-run; the reconstructed envelope
        // is honest about that with a null.
        duration_ms: None,
        num_turns: Some(num_turns),
        is_error: false,
        extra: HashMap::new(),
    };
    let refusal = looks_like_refusal(&qr.result);

    if args.json {
        // stdout stays a clean SuccessEnvelope (byte-identical shape to
        // the live `roba --json` path); metrics, if asked, go to stderr.
        let envelope = SuccessEnvelope {
            version: 1,
            result: &qr,
            refusal,
        };
        println!("{}", serde_json::to_string_pretty(&envelope)?);
        if args.metrics {
            print_metrics(&by_model, rates.as_ref());
        }
        return Ok(());
    }

    // Non-json: the answer goes to stdout; metadata to stderr (roba's
    // stdout=answer / stderr=metadata discipline).
    let style = render::Style::detect_for_subcommand();
    render::print_body(&qr.result, &style);
    print_footer(&qr, refusal, &style);
    if args.metrics {
        print_metrics(&by_model, rates.as_ref());
    }
    Ok(())
}

/// Pull the reconstructed answer + derived turn count out of a parsed
/// session. The answer is the LAST assistant message that carried text
/// (a trailing tool-only turn doesn't blank it); the turn count is every
/// assistant entry. Pure over the log so it unit-tests without disk.
fn reconstruct_answer(log: &SessionLog) -> (String, u32) {
    let mut result_text = String::new();
    let mut num_turns: u32 = 0;
    for entry in &log.entries {
        if let HistoryEntry::Assistant { message, .. } = entry {
            num_turns += 1;
            if let Some(text) = extract_message_text(message) {
                result_text = text;
            }
        }
    }
    (result_text, num_turns)
}

/// Metadata footer to stderr: session id, derived turn count, derived
/// cost (or "cost unavailable"), and the reconstructed-envelope note.
fn print_footer(qr: &QueryResult, refusal: bool, style: &render::Style) {
    render::print_meta_blank();
    if refusal {
        render::print_warning("response looks like a refusal", style);
    }
    let id = qr.session_id.get(..8).unwrap_or(&qr.session_id);
    let turns = qr.num_turns.unwrap_or(0);
    let cost = match qr.cost_usd {
        Some(c) => format!("${c:.4}"),
        None => "cost unavailable".to_string(),
    };
    render::print_meta(
        &format!(
            "session {id} . turns {turns} . {cost} . reconstructed envelope (duration unavailable)"
        ),
        style,
    );
}

/// Per-model usage + cost breakdown to stderr (the `--metrics` block).
/// Sorted by total tokens descending, then model name. Uncosted models
/// (not in the rate table) show `-` in the COST column.
fn print_metrics(by_model: &HashMap<String, Usage>, rates: Option<&Rates>) {
    if by_model.is_empty() {
        eprintln!("no per-model usage recorded for this session");
        return;
    }
    let mut models: Vec<(&String, &Usage)> = by_model.iter().collect();
    models.sort_by(|a, b| b.1.total().cmp(&a.1.total()).then(a.0.cmp(b.0)));

    eprintln!();
    eprintln!(
        "{:<28} {:>9} {:>9} {:>9} {:>9} {:>10}",
        "MODEL", "IN", "OUT", "CACHE_R", "CACHE_W", "COST"
    );
    for (model, u) in models {
        let cost = rates
            .and_then(|r| r.cost_usd(model, u.input, u.output, u.cache_read, u.cache_write))
            .map(|c| format!("${c:.4}"))
            .unwrap_or_else(|| "-".to_string());
        eprintln!(
            "{:<28} {:>9} {:>9} {:>9} {:>9} {:>10}",
            truncate_arg(model, 28),
            format_count(u.input),
            format_count(u.output),
            format_count(u.cache_read),
            format_count(u.cache_write),
            cost,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Map, json};

    fn assistant(text: Option<&str>) -> HistoryEntry {
        let message = match text {
            Some(t) => json!({"content": [{"type": "text", "text": t}]}),
            None => json!({"content": [{"type": "tool_use", "name": "Read", "input": {}}]}),
        };
        HistoryEntry::Assistant {
            uuid: None,
            timestamp: None,
            message,
            rest: Map::new(),
        }
    }

    /// Assistant entry carrying an explicit `stop_reason` (the
    /// completion signal `is_complete` reads).
    fn assistant_stop(reason: Option<&str>) -> HistoryEntry {
        let message = match reason {
            Some(r) => json!({"content": [{"type": "text", "text": "hi"}], "stop_reason": r}),
            None => json!({"content": [{"type": "text", "text": "hi"}]}),
        };
        HistoryEntry::Assistant {
            uuid: None,
            timestamp: None,
            message,
            rest: Map::new(),
        }
    }

    /// A trailing non-assistant metadata entry (e.g. `agent-name`),
    /// which lands in `HistoryEntry::Other`.
    fn other(tag: &str) -> HistoryEntry {
        HistoryEntry::Other {
            type_tag: tag.to_string(),
            raw: json!({"type": tag}),
        }
    }

    fn user() -> HistoryEntry {
        HistoryEntry::User {
            uuid: None,
            timestamp: None,
            cwd: None,
            git_branch: None,
            message: json!({"content": "hi"}),
            rest: Map::new(),
        }
    }

    fn log(entries: Vec<HistoryEntry>) -> SessionLog {
        SessionLog {
            session_id: "sess-1".to_string(),
            project_slug: "-tmp-proj".to_string(),
            entries,
        }
    }

    #[test]
    fn reconstruct_takes_last_assistant_text_and_counts_turns() {
        let l = log(vec![
            user(),
            assistant(Some("first answer")),
            user(),
            assistant(Some("final answer")),
        ]);
        let (text, turns) = reconstruct_answer(&l);
        assert_eq!(text, "final answer");
        assert_eq!(turns, 2);
    }

    #[test]
    fn reconstruct_skips_trailing_tool_only_turn_for_text() {
        // A trailing tool-only assistant turn still counts toward the
        // turn count, but doesn't blank the last textual answer.
        let l = log(vec![user(), assistant(Some("the answer")), assistant(None)]);
        let (text, turns) = reconstruct_answer(&l);
        assert_eq!(text, "the answer");
        assert_eq!(turns, 2);
    }

    #[test]
    fn reconstruct_empty_log_is_empty_zero() {
        let (text, turns) = reconstruct_answer(&log(vec![]));
        assert_eq!(text, "");
        assert_eq!(turns, 0);
    }

    #[test]
    fn is_complete_terminal_stop_reason_is_true() {
        let l = log(vec![user(), assistant_stop(Some("end_turn"))]);
        assert!(is_complete(&l));
    }

    #[test]
    fn is_complete_tool_use_is_false() {
        // A pending tool call means more entries are coming.
        let l = log(vec![user(), assistant_stop(Some("tool_use"))]);
        assert!(!is_complete(&l));
    }

    #[test]
    fn is_complete_trailing_user_entry_is_false() {
        // A trailing user/tool_result with no following assistant turn
        // (e.g. interrupted mid-tool-call) is not complete.
        let l = log(vec![assistant_stop(Some("tool_use")), user()]);
        assert!(!is_complete(&l));
    }

    #[test]
    fn is_complete_empty_log_is_false() {
        assert!(!is_complete(&log(vec![])));
    }

    #[test]
    fn is_complete_missing_stop_reason_is_false() {
        let l = log(vec![user(), assistant_stop(None)]);
        assert!(!is_complete(&l));
    }

    #[test]
    fn is_complete_looks_past_trailing_other_metadata() {
        // Trailing `agent-name`/summary entries land in Other and must
        // not hide the terminal assistant turn behind them.
        let l = log(vec![
            user(),
            assistant_stop(Some("end_turn")),
            other("agent-name"),
        ]);
        assert!(is_complete(&l));
    }

    #[test]
    fn is_complete_uses_last_assistant_turn() {
        // Last assistant is mid-tool-call even though an earlier turn
        // ended -- not complete.
        let l = log(vec![
            user(),
            assistant_stop(Some("end_turn")),
            user(),
            assistant_stop(Some("tool_use")),
        ]);
        assert!(!is_complete(&l));
    }
}
