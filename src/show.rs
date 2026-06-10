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

use claude_wrapper::history::{HistoryEntry, HistoryRoot, SessionLog};
use claude_wrapper::types::QueryResult;

use crate::SuccessEnvelope;
use crate::cost::{Usage, cost_breakdown, usage_by_model};
use crate::history::extract_message_text;
use crate::output::{format_count, looks_like_refusal, truncate_arg};
use crate::rates::Rates;
use crate::render;

/// Implementation of `roba show`.
pub fn run(args: &crate::cli::ShowArgs) -> Result<()> {
    let root = HistoryRoot::home().context("locating ~/.claude/projects")?;
    // `read_session` only fails when no `<id>.jsonl` exists anywhere, so
    // map any failure to a clean not-found error (never a panic).
    let log = match root.read_session(&args.session_id) {
        Ok(log) => log,
        Err(_) => bail!("session `{}` not found", args.session_id),
    };

    let (result_text, num_turns) = reconstruct_answer(&log);

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
}
