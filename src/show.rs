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
use crate::receipt::{self, Receipt};
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
        match wait_for_complete(&root, args)? {
            Waited::Log(log) => log,
            // A detached run that died before claude persisted anything
            // (auth failure, a bad flag). There is no log to render and
            // never will be, so report the receipt and stop waiting.
            Waited::ReceiptOnly(rec) => report_receipt_only(&args.session_id, &rec),
        }
    } else {
        load_session(&root, &args.session_id)?
    };

    // Read the receipt AFTER the log is resolved so a terminal record
    // written during a `--wait` is picked up. Absent (every non-detached
    // run) leaves the render exactly as it was.
    let outcome = receipt::load(&args.session_id);
    render_session(&root, &log, args, outcome.as_ref())?;

    // Propagate the detached child's typed exit code. Without a receipt,
    // or on a clean exit 0, `show` exits 0 as it always has.
    if let Some(code) = outcome
        .as_ref()
        .filter(|r| r.failed())
        .and_then(|r| r.exit_code)
    {
        flush_and_exit(code);
    }
    Ok(())
}

/// Flush stdout (the answer may not end in a newline) and exit with a
/// specific code, bypassing `main`'s error mapping. Used only to mirror a
/// receipt's already-typed code.
fn flush_and_exit(code: i32) -> ! {
    use std::io::Write;
    let _ = std::io::stdout().flush();
    std::process::exit(code);
}

/// Report a detached run that exited without ever writing a session log,
/// then exit with its typed code. Nothing goes to stdout: there is no
/// answer, and stdout must stay the answer channel.
fn report_receipt_only(session_id: &str, rec: &Receipt) -> ! {
    let code = rec.exit_code.unwrap_or(roba_types::EXIT_FAILURE);
    eprintln!(
        "roba: detached run `{session_id}` exited {code} without writing a session log \
         (it failed before claude persisted anything)"
    );
    flush_and_exit(code);
}

/// What a `--wait` resolved to.
enum Waited {
    /// The session log, ready to render.
    Log(SessionLog),
    /// A terminal receipt but no session log -- the run failed early.
    ReceiptOnly(Receipt),
}

/// Load a session by id, distinguishing a genuine not-found from a real
/// I/O / lookup failure.
///
/// `read_session` collapses both into one `Error::History`, so we split
/// them via `find_session`: `Ok(None)` is the clean not-found case
/// (emit the plain "not found" message); an `Err` from the project walk
/// (e.g. a permissions error on `~/.claude/projects/`) propagates with
/// context; a found session that then fails to read (e.g. it vanished
/// mid-read) also propagates rather than masquerading as not-found.
fn load_session(root: &HistoryRoot, session_id: &str) -> Result<SessionLog> {
    match root.find_session(session_id) {
        Ok(Some(_)) => root
            .read_session(session_id)
            .with_context(|| format!("reading session `{session_id}`")),
        Ok(None) => bail!("session `{session_id}` not found"),
        Err(e) => Err(e).with_context(|| format!("looking up session `{session_id}`")),
    }
}

/// Poll the session's on-disk JSONL until it both exists AND looks
/// complete (see [`is_complete`]), then return the parsed log. Bounded
/// by `args.timeout` (default [`DEFAULT_WAIT_TIMEOUT_SECS`]; `0` waits
/// indefinitely) so it never hangs unbounded -- including the case where
/// the session hasn't written its JSONL yet.
/// A terminal receipt is AUTHORITATIVE and outranks the heuristic: the child
/// recorded its own exit, so there is nothing left to wait for -- including
/// the cases the heuristic cannot see (a crash, an auth failure, a cap hit)
/// and the case where the log never appears at all (#441).
fn wait_for_complete(root: &HistoryRoot, args: &crate::cli::ShowArgs) -> Result<Waited> {
    let timeout_secs = args.timeout.unwrap_or(DEFAULT_WAIT_TIMEOUT_SECS);
    let deadline = (timeout_secs != 0).then(|| Instant::now() + Duration::from_secs(timeout_secs));

    loop {
        let outcome = receipt::load(&args.session_id).filter(Receipt::is_terminal);

        // A missing session under `--wait` means "not started yet", not
        // an error: keep polling until it appears or the timeout fires.
        // A REAL lookup/read error (permissions, a vanished file) is NOT
        // "not started yet" -- fail fast rather than poll until timeout.
        match root.find_session(&args.session_id) {
            Ok(Some(_)) => {
                let log = root
                    .read_session(&args.session_id)
                    .with_context(|| format!("reading session `{}`", args.session_id))?;
                if outcome.is_some() || is_complete(&log) {
                    return Ok(Waited::Log(log));
                }
            }
            // Not started yet -- unless the run already recorded an exit,
            // in which case no log is ever coming and waiting out the
            // timeout would report a failure as a timeout.
            Ok(None) => {
                if let Some(rec) = outcome {
                    return Ok(Waited::ReceiptOnly(rec));
                }
            }
            Err(e) => {
                return Err(e).with_context(|| format!("looking up session `{}`", args.session_id));
            }
        }
        if let Some(deadline) = deadline
            && Instant::now() >= deadline
        {
            // Surface the wait-timeout as a `claude_wrapper::Error::Timeout`
            // so `classify_exit_code` downcasts it to the documented `4
            // timeout` exit (and `--json` reports `kind: "timeout"`),
            // rather than the generic `1` a plain `bail!` would yield.
            return Err(anyhow::Error::new(claude_wrapper::Error::Timeout {
                timeout_seconds: timeout_secs,
            }))
            .with_context(|| {
                format!(
                    "waited {timeout_secs}s for session `{}` to complete",
                    args.session_id
                )
            });
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
pub(crate) fn is_complete(log: &SessionLog) -> bool {
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
fn render_session(
    root: &HistoryRoot,
    log: &SessionLog,
    args: &crate::cli::ShowArgs,
    outcome: Option<&Receipt>,
) -> Result<()> {
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

    // The one thing the session log CANNOT tell us: how the run ended.
    // A terminal receipt carries the child's typed exit code, so prefer it
    // (#441); with no receipt, `is_error` stays false exactly as before and
    // `exit_code` is absent from the envelope rather than fabricated.
    let terminal = outcome.filter(|r| r.is_terminal());
    let mut extra = HashMap::new();
    if let Some(code) = terminal.and_then(|r| r.exit_code) {
        extra.insert("exit_code".to_string(), serde_json::json!(code));
    }

    let qr = QueryResult {
        result: result_text,
        session_id: log.session_id.clone(),
        cost_usd,
        // duration is not persisted per-run; the reconstructed envelope
        // is honest about that with a null.
        duration_ms: None,
        num_turns: Some(num_turns),
        usage: None,
        is_error: terminal.is_some_and(Receipt::failed),
        extra,
    };
    let refusal = looks_like_refusal(&qr.result);

    if args.json {
        // stdout stays a clean SuccessEnvelope (byte-identical shape to
        // the live `roba --json` path); metrics, if asked, go to stderr.
        let envelope = SuccessEnvelope {
            version: roba_types::VERSION,
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
    // A recorded exit code is the run's real outcome, so it leads the
    // provenance note. Absent for every run without a receipt.
    let exit = match qr.extra.get("exit_code").and_then(|v| v.as_i64()) {
        Some(code) => format!("exit {code} . "),
        None => String::new(),
    };
    render::print_meta(
        &format!(
            "session {id} . turns {turns} . {cost} . {exit}reconstructed envelope (duration unavailable)"
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
    fn load_session_missing_is_clean_not_found() {
        // An empty projects root: no `<id>.jsonl` anywhere -> the clean
        // not-found message, NOT a masked I/O error.
        let dir = tempfile::tempdir().unwrap();
        let root = HistoryRoot::at(dir.path());
        let err = load_session(&root, "missing-id").unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("not found"), "got: {msg}");
    }

    #[test]
    fn load_session_real_error_propagates_not_as_not_found() {
        // Point the root at a regular FILE: the project walk's read_dir
        // fails with a non-NotFound error, which must surface as a real
        // lookup error rather than collapse to "not found".
        let file = tempfile::NamedTempFile::new().unwrap();
        let root = HistoryRoot::at(file.path());
        let err = load_session(&root, "any-id").unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            !msg.contains("not found"),
            "a real I/O error must not masquerade as not-found: {msg}"
        );
        assert!(msg.contains("looking up session"), "got: {msg}");
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
