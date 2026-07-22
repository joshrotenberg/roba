//! Derived views over run receipts: `roba jobs` (a ps-like table) and
//! `roba watch` (`show --wait` pluralized). Slices 1-2 of #444.
//!
//! Both are READ-ONLY projections of durable state -- the receipts a
//! detached run writes (#441) plus, for `watch`, the session JSONL claude
//! itself persists. Nothing here owns runtime state: kill the process at
//! any point and nothing is lost, re-run it and the view reconstructs.
//! That is the doctrine line these verbs live inside ("hot running and
//! notifications WITHOUT a resident process" -- the shell is the REPL).
//!
//! The honest "stale?" column: a `running` receipt whose recorded pid is
//! gone means the run was killed hard enough to skip its exit seam
//! (SIGKILL, power loss). On unix that is a `kill(pid, 0)` probe; on
//! Windows liveness is not probed, so a running record reads `running?`
//! (unknown), never a false claim either way.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use claude_wrapper::history::HistoryRoot;
use serde::Serialize;

use crate::cli::{JobsArgs, WatchArgs};
use crate::receipt::{self, Receipt};

/// Same cadence as `show --wait` (src/show.rs): cheap file reads, and a
/// detached run is minutes-long, so finer polling buys nothing.
const POLL_INTERVAL: Duration = Duration::from_secs(1);

/// Default watch bound, matching `show --wait`'s documented default.
const DEFAULT_WATCH_TIMEOUT_SECS: u64 = 600;

// ---------------------------------------------------------------------------
// jobs
// ---------------------------------------------------------------------------

/// Liveness of a receipt's recorded state, derived rather than trusted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum JobState {
    /// Terminal receipt, exit 0.
    Ok,
    /// Terminal receipt, non-zero exit (the code is in the row).
    Failed,
    /// Running record, pid alive.
    Running,
    /// Running record, pid gone: died without reaching its exit seam.
    Stale,
    /// Running record, liveness unknowable on this platform.
    Unknown,
}

/// One row of the jobs table, also the `--json` payload shape.
#[derive(Debug, Serialize)]
struct JobRow {
    session_id: String,
    state: JobState,
    #[serde(skip_serializing_if = "Option::is_none")]
    exit_code: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cost_usd: Option<f64>,
    started_at: u64,
    /// Best-effort project path (decoded from the session's history
    /// location); absent when the session wrote no JSONL (died early) or
    /// history is unavailable.
    #[serde(skip_serializing_if = "Option::is_none")]
    project: Option<String>,
}

/// `kill(pid, 0)` probe: `Some(true)` alive, `Some(false)` gone, `None`
/// unknowable (non-unix). EPERM means "exists but not ours" -- alive.
#[cfg(unix)]
fn pid_alive(pid: u32) -> Option<bool> {
    let r = unsafe { libc::kill(pid as libc::pid_t, 0) };
    if r == 0 {
        return Some(true);
    }
    Some(std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM))
}

#[cfg(not(unix))]
fn pid_alive(_pid: u32) -> Option<bool> {
    None
}

/// Derive the honest state for a receipt given a liveness probe result.
fn derive_state(rec: &Receipt, alive: Option<bool>) -> JobState {
    if rec.is_terminal() {
        return if rec.failed() {
            JobState::Failed
        } else {
            JobState::Ok
        };
    }
    match alive {
        Some(true) => JobState::Running,
        Some(false) => JobState::Stale,
        None => JobState::Unknown,
    }
}

/// The STATE cell: `ok`, `exit N`, `running`, `stale?`, `running?`.
fn state_cell(row: &JobRow) -> String {
    match row.state {
        JobState::Ok => "ok".to_string(),
        JobState::Failed => format!("exit {}", row.exit_code.unwrap_or(1)),
        JobState::Running => "running".to_string(),
        JobState::Stale => "stale?".to_string(),
        JobState::Unknown => "running?".to_string(),
    }
}

/// Humanize an age in seconds: `42s`, `13m`, `7h`, `3d`.
fn humanize_age(secs: u64) -> String {
    match secs {
        0..=59 => format!("{secs}s"),
        60..=3599 => format!("{}m", secs / 60),
        3600..=86399 => format!("{}h", secs / 3600),
        _ => format!("{}d", secs / 86400),
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// All receipts in the runs directory, newest first. Best-effort: an
/// unreadable entry is skipped, a missing directory is an empty list.
fn all_receipts() -> Vec<Receipt> {
    let Some(dir) = receipt::runs_dir() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut recs: Vec<Receipt> = entries
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| receipt::read_at(&e.path()))
        .collect();
    recs.sort_by_key(|r| std::cmp::Reverse(r.started_at));
    recs
}

/// Best-effort project column: where claude filed the session's JSONL,
/// decoded back to a path. `None` when the session never wrote one or
/// history is unreachable -- the row still renders.
fn project_for(root: Option<&HistoryRoot>, session_id: &str) -> Option<String> {
    let root = root?;
    let (_, slug) = root.find_session(session_id).ok().flatten()?;
    let project = root
        .list_projects()
        .ok()?
        .into_iter()
        .find(|p| p.slug == slug)?;
    Some(project.decoded_path.display().to_string())
}

/// `roba jobs`: the ps-like table over run receipts.
pub fn run_jobs(args: &JobsArgs) -> Result<()> {
    let recs = all_receipts();
    if recs.is_empty() {
        eprintln!("no detached runs recorded");
        if let Some(dir) = receipt::runs_dir() {
            eprintln!("(receipts live in {}; --detach writes them)", dir.display());
        }
        return Ok(());
    }
    // History is optional garnish for the PROJECT column; jobs must render
    // even with no ~/.claude at all.
    let root = HistoryRoot::home().ok();
    let now = now_secs();
    let rows: Vec<JobRow> = recs
        .into_iter()
        .map(|rec| {
            let alive = if rec.is_terminal() {
                None
            } else {
                pid_alive(rec.pid)
            };
            JobRow {
                state: derive_state(&rec, alive),
                exit_code: rec.exit_code,
                cost_usd: rec.cost_usd,
                started_at: rec.started_at,
                project: project_for(root.as_ref(), &rec.session_id),
                session_id: rec.session_id,
            }
        })
        .collect();

    if args.json {
        let envelope = roba_types::VersionedResult::new(&rows);
        println!("{}", serde_json::to_string_pretty(&envelope)?);
        return Ok(());
    }

    println!(
        "{:<10}  {:<9}  {:<8}  {:<5}  PROJECT",
        "SESSION", "STATE", "COST", "AGE"
    );
    for row in &rows {
        let short: String = row.session_id.chars().take(8).collect();
        let cost = row
            .cost_usd
            .map(|c| format!("${c:.4}"))
            .unwrap_or_else(|| "-".to_string());
        let age = humanize_age(now.saturating_sub(row.started_at));
        let project = row.project.as_deref().unwrap_or("-");
        println!(
            "{short:<10}  {:<9}  {cost:<8}  {age:<5}  {project}",
            state_cell(row)
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// watch
// ---------------------------------------------------------------------------

/// Resolve one watch target against the receipts on disk: exact id, else a
/// unique prefix. Ambiguity and no-match are loud errors -- watching the
/// wrong run is worse than asking again.
fn resolve_target(id: &str, recs: &[Receipt]) -> Result<String> {
    if recs.iter().any(|r| r.session_id == id) {
        return Ok(id.to_string());
    }
    let matches: Vec<&Receipt> = recs
        .iter()
        .filter(|r| r.session_id.starts_with(id))
        .collect();
    match matches.len() {
        1 => Ok(matches[0].session_id.clone()),
        0 => bail!("no receipt matches `{id}` (see `roba jobs` for recorded runs)"),
        n => bail!("`{id}` is ambiguous: {n} receipts match (see `roba jobs`)"),
    }
}

/// How one watched run finished.
struct Done {
    session_id: String,
    /// The typed exit code, when a terminal receipt recorded one; `None`
    /// when completion came from the session-log heuristic instead.
    exit_code: Option<i32>,
    cost_usd: Option<f64>,
}

/// Check one session for completion: a terminal receipt is authoritative;
/// otherwise the session-log heuristic (`show`'s `is_complete`) decides.
fn check_done(root: Option<&HistoryRoot>, session_id: &str) -> Option<Done> {
    if let Some(rec) = receipt::load(session_id).filter(Receipt::is_terminal) {
        return Some(Done {
            session_id: session_id.to_string(),
            exit_code: rec.exit_code,
            cost_usd: rec.cost_usd,
        });
    }
    let root = root?;
    let log = root.read_session(session_id).ok()?;
    crate::show::is_complete(&log).then(|| Done {
        session_id: session_id.to_string(),
        exit_code: None,
        cost_usd: None,
    })
}

/// One completion line: `<id8>  exit N  [$cost]` or `<id8>  done (log)`.
fn done_line(done: &Done) -> String {
    let short: String = done.session_id.chars().take(8).collect();
    let mut line = match done.exit_code {
        Some(code) => format!("{short:<10}  exit {code}"),
        None => format!("{short:<10}  done (log)"),
    };
    if let Some(cost) = done.cost_usd {
        line.push_str(&format!("  ${cost:.4}"));
    }
    line
}

/// Ring an OSC-9 terminal notification on stderr when it is a TTY. The
/// escape is metadata, so it rides stderr like every other non-answer byte;
/// terminals without OSC-9 support ignore it silently.
fn notify(text: &str) {
    use std::io::{IsTerminal, Write};
    let mut err = std::io::stderr();
    if err.is_terminal() {
        let _ = write!(err, "\x1b]9;{text}\x07");
        let _ = err.flush();
    }
}

/// `roba watch`: poll a set of sessions, report each completion, exit when
/// all are done (0 all ok / 1 any failed) or on timeout (the typed exit 4).
pub fn run_watch(args: &WatchArgs) -> Result<()> {
    let recs = all_receipts();

    // Explicit ids resolve against the receipts; no ids means "everything
    // currently running" -- the tab you glance at after firing a batch.
    let mut pending: Vec<String> = if args.session_ids.is_empty() {
        recs.iter()
            .filter(|r| !r.is_terminal())
            .map(|r| r.session_id.clone())
            .collect()
    } else {
        args.session_ids
            .iter()
            .map(|id| resolve_target(id, &recs))
            .collect::<Result<Vec<_>>>()?
    };
    if pending.is_empty() {
        eprintln!("nothing to watch: no running detached runs recorded");
        return Ok(());
    }
    eprintln!("watching {} run(s)", pending.len());

    let timeout_secs = args.timeout.unwrap_or(DEFAULT_WATCH_TIMEOUT_SECS);
    let deadline = (timeout_secs != 0).then(|| Instant::now() + Duration::from_secs(timeout_secs));
    let root = HistoryRoot::home().ok();
    let mut any_failed = false;

    loop {
        let mut still = Vec::with_capacity(pending.len());
        for id in pending.drain(..) {
            match check_done(root.as_ref(), &id) {
                Some(done) => {
                    if done.exit_code.is_some_and(|c| c != 0) {
                        any_failed = true;
                    }
                    // The completion line IS the answer: stdout.
                    println!("{}", done_line(&done));
                    notify(&format!("roba: {}", done_line(&done)));
                }
                None => still.push(id),
            }
        }
        pending = still;
        if pending.is_empty() {
            break;
        }
        if let Some(deadline) = deadline
            && Instant::now() >= deadline
        {
            // Mirror `show --wait`: the typed Timeout maps to exit 4 via
            // classify_exit_code, with the pending set named for context.
            return Err(anyhow::Error::new(claude_wrapper::Error::Timeout {
                timeout_seconds: timeout_secs,
            }))
            .with_context(|| {
                format!(
                    "still waiting on {} run(s): {}",
                    pending.len(),
                    pending.join(", ")
                )
            });
        }
        std::thread::sleep(POLL_INTERVAL);
    }

    if any_failed {
        // Failure of a WATCHED RUN, not of watch itself: signal via the
        // process exit (generic 1), with the lines above as the detail.
        std::process::exit(1);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::receipt::State;

    fn rec(id: &str, state: State, code: Option<i32>) -> Receipt {
        Receipt {
            session_id: id.to_string(),
            pid: 42,
            started_at: 100,
            state,
            exit_code: code,
            ended_at: code.map(|_| 200),
            cost_usd: None,
        }
    }

    // -- state derivation --------------------------------------------------

    #[test]
    fn terminal_receipts_derive_ok_and_failed() {
        assert_eq!(
            derive_state(&rec("a", State::Exited, Some(0)), None),
            JobState::Ok
        );
        assert_eq!(
            derive_state(&rec("a", State::Exited, Some(7)), None),
            JobState::Failed
        );
    }

    #[test]
    fn running_receipt_state_follows_liveness() {
        let r = rec("a", State::Running, None);
        assert_eq!(derive_state(&r, Some(true)), JobState::Running);
        assert_eq!(derive_state(&r, Some(false)), JobState::Stale);
        assert_eq!(derive_state(&r, None), JobState::Unknown);
    }

    #[test]
    fn state_cells_render_honestly() {
        let row = |state, code| JobRow {
            session_id: "s".into(),
            state,
            exit_code: code,
            cost_usd: None,
            started_at: 0,
            project: None,
        };
        assert_eq!(state_cell(&row(JobState::Ok, Some(0))), "ok");
        assert_eq!(state_cell(&row(JobState::Failed, Some(7))), "exit 7");
        assert_eq!(state_cell(&row(JobState::Running, None)), "running");
        assert_eq!(state_cell(&row(JobState::Stale, None)), "stale?");
        assert_eq!(state_cell(&row(JobState::Unknown, None)), "running?");
    }

    #[cfg(unix)]
    #[test]
    fn our_own_pid_is_alive_and_a_wild_pid_is_not() {
        assert_eq!(pid_alive(std::process::id()), Some(true));
        // Beyond any real pid space on macOS/Linux defaults.
        assert_eq!(pid_alive(99_999_999), Some(false));
    }

    // -- rendering helpers -------------------------------------------------

    #[test]
    fn ages_humanize_by_magnitude() {
        assert_eq!(humanize_age(42), "42s");
        assert_eq!(humanize_age(60), "1m");
        assert_eq!(humanize_age(7200), "2h");
        assert_eq!(humanize_age(200_000), "2d");
    }

    #[test]
    fn done_lines_show_code_and_cost() {
        let mut d = Done {
            session_id: "abcdefgh-rest".into(),
            exit_code: Some(7),
            cost_usd: Some(0.13),
        };
        assert_eq!(done_line(&d), "abcdefgh    exit 7  $0.1300");
        d.exit_code = None;
        d.cost_usd = None;
        assert_eq!(done_line(&d), "abcdefgh    done (log)");
    }

    // -- target resolution -------------------------------------------------

    #[test]
    fn targets_resolve_exact_then_unique_prefix() {
        let recs = vec![
            rec("aaaa-1111", State::Running, None),
            rec("aabb-2222", State::Running, None),
        ];
        assert_eq!(resolve_target("aaaa-1111", &recs).unwrap(), "aaaa-1111");
        assert_eq!(resolve_target("aab", &recs).unwrap(), "aabb-2222");
        assert!(
            resolve_target("aa", &recs)
                .unwrap_err()
                .to_string()
                .contains("ambiguous")
        );
        assert!(
            resolve_target("zz", &recs)
                .unwrap_err()
                .to_string()
                .contains("no receipt")
        );
    }
}
