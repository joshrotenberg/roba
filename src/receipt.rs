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
use std::time::{SystemTime, UNIX_EPOCH};

// The consumer-facing contract (shape, paths, readers) lives in roba-types;
// re-export it so in-crate callers keep saying `crate::receipt::*`.
pub use roba_types::receipt::{
    RECEIPT_ENV, Receipt, STATE_DIR_ENV, State, load, path_for, read_at, runs_dir,
};

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
    };
    let _ = write_atomic(&path, &rec);
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
    let Some(mut rec) = read_at(&path) else {
        return;
    };
    if rec.pid != std::process::id() {
        return;
    }
    rec.state = State::Exited;
    rec.exit_code = Some(exit_code);
    rec.ended_at = Some(now_secs());
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
        }
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
