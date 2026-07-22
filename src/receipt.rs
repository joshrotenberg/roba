//! Run receipts -- the WRITE side of the durable outcome record for a
//! detached run.
//!
//! `--detach` re-execs roba disowned and drops the child handle, so the
//! child's typed exit code has nowhere to go. `roba show` then had to
//! reconstruct the envelope with `is_error: false` hardcoded, which made a
//! budget-capped, auth-failed, or crashed detached run report as success --
//! the typed exit-code ABI failing in exactly the unattended case it exists
//! for (#441).
//!
//! A receipt closes that gap: the detached child writes a small JSON record
//! keyed by session id, and `show` prefers it over the `stop_reason`
//! heuristic.
//!
//! The record SHAPE, the path-resolution contract, and the best-effort
//! readers live in [`roba_types::receipt`] (re-exported here), so downstream
//! consumers (a scheduler, a watcher) read receipts through the published
//! ABI crate without depending on this binary. This module keeps what only
//! the writer needs: the atomic write, the pid-ownership guard, and the
//! `start`/`finish` seams.
//!
//! Three properties keep this inside the "roba owns no runtime state" line:
//!
//! - **One writer.** The CHILD writes both records (the start record early in
//!   `main`, the terminal record at the exit seam). The parent only computes
//!   the path and hands it over in `ROBA_RECEIPT`. A single writer means the
//!   parent can never clobber a terminal record written by a fast child.
//!   `ROBA_RECEIPT` is inherited by every descendant, so that single writer is
//!   enforced by an explicit pid check (the `owns` helper) rather than
//!   assumed.
//! - **Best-effort, never load-bearing.** Every write and read here is
//!   fallible and swallowed. A missing or unreadable receipt degrades to
//!   exactly today's behavior, so this is a disposable, self-healing artifact
//!   rather than state roba depends on.
//! - **A receipt describes a FINISHED run.** It is not a job table, a queue,
//!   or a supervisor. Same species as claude's own session records.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

// The consumer-facing contract (shape, paths, readers) lives in roba-types;
// re-export it so in-crate callers keep saying `crate::receipt::*`.
pub use roba_types::receipt::{
    RECEIPT_ENV, Receipt, STATE_DIR_ENV, State, load, path_for, read_at, runs_dir,
};

/// The run's observed spend, noted once where the result is known (the
/// success seam's `total_cost_usd`, or a cap-hit error's parsed cost) and
/// attached to the terminal record by [`finish`]. Process-global because the
/// exit seams live in `main` while the result lives in `run_ask`; set-once
/// because one process is one run.
static OBSERVED_COST: OnceLock<f64> = OnceLock::new();

/// Note the run's observed cost for the terminal receipt. Safe to call on
/// any run (a no-op recording for a foreground run whose receipt never
/// gets written); later calls after the first are ignored.
pub fn note_cost(cost_usd: f64) {
    let _ = OBSERVED_COST.set(cost_usd);
}

/// Seconds since the Unix epoch; 0 if the clock is before it (never in
/// practice, and a bogus timestamp must not fail a run).
fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Write a record atomically: serialize to a sibling temp file, then rename
/// over the target. A concurrent reader therefore sees either the previous
/// record or the new one, never a half-written file.
fn write_atomic(path: &Path, rec: &Receipt) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string(rec)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    // The pid suffix keeps two processes writing the same id from colliding
    // on the temp file (they cannot both be the single writer, but a stale
    // temp file from a killed run must not block a later one).
    let tmp = path.with_extension(format!("json.tmp.{}", std::process::id()));
    std::fs::write(&tmp, json)?;
    // `fs::rename` replaces an existing destination on both platforms (on
    // Windows it uses MOVEFILE_REPLACE_EXISTING), so no unlink-first step is
    // needed -- and adding one would open the exact window (destination
    // briefly absent) that the temp-file-plus-rename exists to close.
    match std::fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

/// The path this process should write its receipt to, if any. Set by the
/// detaching parent via [`RECEIPT_ENV`]; absent for every ordinary run, which
/// is what keeps receipts scoped to runs roba itself detached.
fn active_path() -> Option<PathBuf> {
    let raw = std::env::var(RECEIPT_ENV).ok()?;
    (!raw.is_empty()).then(|| PathBuf::from(raw))
}

/// Whether this process may write the record at `path`.
///
/// [`RECEIPT_ENV`] travels in the ENVIRONMENT, and env is inherited by every
/// descendant: the detached child's `claude`, and in turn every process
/// claude spawns -- including a nested `roba` (the chained/recursive shape,
/// and roba verbs invoked inside a dispatch). Without this check a nested
/// roba stamps the outer run's receipt `exited, exit_code: 0` while the real
/// run is still working, and `show --wait` hands the orchestrator that wrong
/// success: the same class of failure #441 exists to fix, on the exact recipe
/// the README tells callers to depend on.
///
/// Ownership is decided by pid against the record already on disk:
///
/// - no record: unclaimed, this process may claim it.
/// - a TERMINAL record: a prior run of this id finished, so a genuinely new
///   run reusing the id may overwrite it.
/// - a `running` record carrying OUR pid: our own start record.
/// - a `running` record carrying a FOREIGN pid: someone else's live run --
///   decline, leaving their record untouched.
///
/// The one case this declines wrongly is a new run reusing an id whose
/// previous run was SIGKILLed, leaving a `running` record behind a dead pid.
/// That degrades to no receipt, i.e. exactly today's behavior, and is what
/// the deferred pid-liveness check resolves.
fn owns(path: &Path) -> bool {
    match read_at(path) {
        None => true,
        Some(existing) => existing.is_terminal() || existing.pid == std::process::id(),
    }
}

/// Session id for a receipt path -- the file stem, which [`path_for`] built
/// from the id. Used to fill the start record without plumbing the id
/// through the child's argv parsing.
fn id_from_path(path: &Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_string()
}

/// Receipts older than this are swept when a new detached run claims its
/// path: 30 days, comfortably past any plausible `show`/re-attach window
/// (claude's own session records are the durable history; a receipt is only
/// the run's outcome signal). The sweep is the directory's whole lifecycle
/// -- amortized on the runs that create the pressure, so the state dir
/// self-heals instead of accumulating one file per detached run forever
/// (#447).
const PRUNE_AFTER_SECS: u64 = 30 * 24 * 60 * 60;

/// Remove entries in `dir` whose mtime is before `cutoff` (seconds since
/// the Unix epoch), skipping `keep`. Sweeps expired records AND stale temp
/// files from killed runs. Best-effort: any unreadable entry or failed
/// remove is skipped, and a missing directory is a no-op.
fn prune_older_than(dir: &Path, cutoff: u64, keep: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path == keep {
            continue;
        }
        let Some(mtime) = entry
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
        else {
            continue;
        };
        if mtime < cutoff {
            let _ = std::fs::remove_file(&path);
        }
    }
}

/// Record that this (detached) run started. Called once, early in `main`;
/// a no-op for any process without [`RECEIPT_ENV`] set.
///
/// The child writes its own start record rather than the parent writing one
/// after `spawn()`: a child that exits fast would otherwise have its terminal
/// record clobbered by the parent's late start record. One writer, in order,
/// no race.
///
/// Declines when the record on disk belongs to another live run (the `owns`
/// helper), which is what keeps a nested roba that merely inherited
/// [`RECEIPT_ENV`] from claiming the outer run's receipt.
pub fn start() {
    let Some(path) = active_path() else { return };
    if !owns(&path) {
        return;
    }
    let rec = Receipt {
        session_id: id_from_path(&path),
        pid: std::process::id(),
        started_at: now_secs(),
        state: State::Running,
        exit_code: None,
        ended_at: None,
        cost_usd: None,
    };
    let _ = write_atomic(&path, &rec);
    // The directory's lifecycle: each new run sweeps expired receipts, so
    // the state dir stays disposable. After the write, so a slow sweep can
    // never delay the record an observer is about to poll for.
    if let Some(dir) = path.parent() {
        prune_older_than(dir, now_secs().saturating_sub(PRUNE_AFTER_SECS), &path);
    }
}

/// Close a record with its terminal outcome: the pure half of [`finish`],
/// split out so the attach logic is testable without the process-global
/// cost note or the env-carried path.
fn close(mut rec: Receipt, exit_code: i32, cost_usd: Option<f64>, now: u64) -> Receipt {
    rec.state = State::Exited;
    rec.exit_code = Some(exit_code);
    rec.ended_at = Some(now);
    // Attach observed spend when this run noted one; otherwise keep whatever
    // the record already carried (nothing today) rather than erasing it.
    if cost_usd.is_some() {
        rec.cost_usd = cost_usd;
    }
    rec
}

/// Record this run's terminal exit code. Called at every exit seam of a
/// detached child; a no-op without [`RECEIPT_ENV`].
///
/// Failure here is swallowed on purpose: a run must not change its exit code
/// because a disposable observability artifact could not be written.
///
/// Only the process that claimed the record in [`start`] may close it: the
/// on-disk `pid` must be ours. A nested roba that inherited [`RECEIPT_ENV`]
/// reads a foreign pid here and leaves the record alone, so it can never
/// report a still-working run as `exited, exit_code: 0`. `started_at` and
/// `pid` are carried over from the start record.
pub fn finish(exit_code: i32) {
    let Some(path) = active_path() else { return };
    let Some(rec) = read_at(&path) else {
        return;
    };
    if rec.pid != std::process::id() {
        return;
    }
    let rec = close(rec, exit_code, OBSERVED_COST.get().copied(), now_secs());
    let _ = write_atomic(&path, &rec);
}

#[cfg(test)]
mod tests {
    use super::*;

    // Schema, predicate, and id-safety tests live with the types in
    // roba-types::receipt; this module tests only what the writer owns.

    fn rec(state: State, code: Option<i32>) -> Receipt {
        Receipt {
            session_id: "sess-1".to_string(),
            pid: 42,
            started_at: 100,
            state,
            exit_code: code,
            ended_at: code.map(|_| 200),
            cost_usd: None,
        }
    }

    // -- close (terminal attach) -------------------------------------------

    #[test]
    fn close_attaches_code_time_and_cost() {
        let r = close(rec(State::Running, None), 7, Some(0.13), 300);
        assert_eq!(r.state, State::Exited);
        assert_eq!(r.exit_code, Some(7));
        assert_eq!(r.ended_at, Some(300));
        assert_eq!(r.cost_usd, Some(0.13));
    }

    #[test]
    fn close_without_cost_leaves_cost_absent() {
        // No result event (crash-shaped exits): unknown, never zero.
        let r = close(rec(State::Running, None), 1, None, 300);
        assert!(r.is_terminal());
        assert_eq!(r.cost_usd, None);
    }

    // -- prune (the lifecycle) ---------------------------------------------

    #[test]
    fn prune_removes_entries_older_than_cutoff_and_keeps_ours() {
        let dir = tempfile::tempdir().unwrap();
        let old_rec = dir.path().join("old.json");
        let stale_tmp = dir.path().join("dead.json.tmp.999");
        let ours = dir.path().join("ours.json");
        write_atomic(&old_rec, &rec(State::Exited, Some(0))).unwrap();
        std::fs::write(&stale_tmp, "half-written").unwrap();
        write_atomic(&ours, &rec(State::Running, None)).unwrap();

        // A cutoff in the future ages out everything except the kept path.
        let future = now_secs() + 10;
        prune_older_than(dir.path(), future, &ours);
        assert!(!old_rec.exists(), "expired record should be swept");
        assert!(!stale_tmp.exists(), "stale temp file should be swept");
        assert!(ours.exists(), "our own record must survive any cutoff");
    }

    #[test]
    fn prune_keeps_entries_newer_than_cutoff() {
        let dir = tempfile::tempdir().unwrap();
        let fresh = dir.path().join("fresh.json");
        write_atomic(&fresh, &rec(State::Exited, Some(0))).unwrap();
        // A cutoff in the past leaves fresh records alone.
        prune_older_than(
            dir.path(),
            now_secs().saturating_sub(60),
            &dir.path().join("other.json"),
        );
        assert!(fresh.exists());
    }

    #[test]
    fn prune_missing_dir_is_a_noop() {
        let dir = tempfile::tempdir().unwrap();
        prune_older_than(&dir.path().join("absent"), now_secs(), Path::new("x"));
    }

    // -- atomic write ------------------------------------------------------

    #[test]
    fn write_atomic_creates_parents_and_leaves_no_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("runs").join("sess-1.json");
        write_atomic(&path, &rec(State::Exited, Some(5))).unwrap();

        let back: Receipt = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(back.exit_code, Some(5));

        let leftovers: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.contains("tmp"))
            .collect();
        assert!(leftovers.is_empty(), "temp file left behind: {leftovers:?}");
    }

    #[test]
    fn write_atomic_overwrites_an_existing_record() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sess-1.json");
        write_atomic(&path, &rec(State::Running, None)).unwrap();
        write_atomic(&path, &rec(State::Exited, Some(0))).unwrap();
        let back: Receipt = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(back.state, State::Exited);
        assert_eq!(back.exit_code, Some(0));
    }

    // -- ownership ---------------------------------------------------------
    //
    // ROBA_RECEIPT is inherited by every descendant of the detached child,
    // so a nested roba must not claim or close the outer run's record.

    /// Write a record with a specific pid at `path`.
    fn plant(path: &Path, pid: u32, state: State, code: Option<i32>) {
        let mut r = rec(state, code);
        r.pid = pid;
        write_atomic(path, &r).unwrap();
    }

    /// A pid that is certainly not ours.
    fn foreign_pid() -> u32 {
        std::process::id().wrapping_add(1)
    }

    #[test]
    fn an_unclaimed_path_is_ownable() {
        let dir = tempfile::tempdir().unwrap();
        assert!(owns(&dir.path().join("sess-1.json")));
    }

    #[test]
    fn our_own_running_record_stays_ours() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sess-1.json");
        plant(&path, std::process::id(), State::Running, None);
        assert!(owns(&path));
    }

    #[test]
    fn a_foreign_running_record_is_not_ours() {
        // The nested-roba case: another live run holds this receipt.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sess-1.json");
        plant(&path, foreign_pid(), State::Running, None);
        assert!(!owns(&path), "must not claim another live run's receipt");
    }

    #[test]
    fn a_terminal_foreign_record_may_be_reclaimed() {
        // A prior run of this id finished, so a new run reusing the id owns
        // the path -- otherwise a reused session id could never get a receipt.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sess-1.json");
        plant(&path, foreign_pid(), State::Exited, Some(0));
        assert!(owns(&path));
    }

    #[test]
    fn a_malformed_record_does_not_block_ownership() {
        // Unreadable is indistinguishable from absent; degrade to claimable
        // rather than losing receipts to one corrupt file.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sess-1.json");
        std::fs::write(&path, "{not json").unwrap();
        assert!(owns(&path));
    }

    #[test]
    fn id_from_path_is_the_file_stem() {
        assert_eq!(id_from_path(Path::new("/s/runs/abc-123.json")), "abc-123");
    }
}
