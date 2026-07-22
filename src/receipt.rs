//! Run receipts -- the durable outcome record for a detached run.
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
//! heuristic. Shape:
//!
//! ```json
//! {"session_id":"...","pid":41234,"started_at":1753,"state":"running"}
//! {"session_id":"...","pid":41234,"started_at":1753,"state":"exited",
//!  "exit_code":7,"ended_at":1789}
//! ```
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
//!
//! Location: `$ROBA_STATE_DIR/runs/<id>.json`, else
//! `$XDG_STATE_HOME/roba/runs/<id>.json`, else
//! `~/.local/state/roba/runs/<id>.json`. Deliberately NOT `.roba/` -- that is
//! the hermetic config bundle directory, meant to be authored, audited, and
//! potentially committed; receipts are per-machine and disposable.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// Env var carrying the receipt path from the detaching parent to the
/// detached child. Env, not argv: the re-exec does raw argv surgery on the
/// user's own tokens, and a receipt path has no business on the clap
/// surface. Its presence is also the child's only signal that roba itself
/// detached it, so a receipt is never written for an ordinary foreground run.
pub const RECEIPT_ENV: &str = "ROBA_RECEIPT";

/// Env var overriding the state directory root (receipts land in
/// `<dir>/runs/`). Exists for test isolation and for callers that keep roba's
/// disposable state somewhere specific.
pub const STATE_DIR_ENV: &str = "ROBA_STATE_DIR";

/// Lifecycle state of a run receipt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    /// The child started and has not recorded an exit. Either still running
    /// or killed hard enough to skip its exit seam (SIGKILL, power loss).
    Running,
    /// The child reached an exit seam and recorded its typed code.
    Exited,
}

/// The on-disk record. Timestamps are Unix epoch SECONDS -- coarse on
/// purpose, and dependency-free (no chrono).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Receipt {
    /// Session handle this run was launched under (the `show <ID>` key).
    pub session_id: String,
    /// The detached child's pid. Recorded so a later refinement can tell
    /// "still running" from "died without recording an exit" via a liveness
    /// check; nothing reads it for that purpose yet.
    pub pid: u32,
    pub started_at: u64,
    pub state: State,
    /// The child's typed exit code. Present iff `state` is `exited`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<u64>,
}

impl Receipt {
    /// True once the child recorded an exit code. Only a terminal receipt is
    /// authoritative about the outcome; a `running` one says nothing.
    pub fn is_terminal(&self) -> bool {
        self.state == State::Exited && self.exit_code.is_some()
    }

    /// True when this run finished with a non-zero typed exit code. A
    /// non-terminal receipt is NOT a failure -- it is "no answer yet".
    pub fn failed(&self) -> bool {
        self.is_terminal() && self.exit_code != Some(0)
    }
}

/// Seconds since the Unix epoch; 0 if the clock is before it (never in
/// practice, and a bogus timestamp must not fail a run).
fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// The directory receipts live in, per the documented precedence. `None`
/// when no home can be resolved and nothing is set -- receipts are then
/// simply off.
pub fn runs_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var(STATE_DIR_ENV)
        && !dir.is_empty()
    {
        return Some(PathBuf::from(dir).join("runs"));
    }
    if let Ok(dir) = std::env::var("XDG_STATE_HOME")
        && !dir.is_empty()
    {
        return Some(PathBuf::from(dir).join("roba").join("runs"));
    }
    crate::profile::home_dir().map(|h| h.join(".local").join("state").join("roba").join("runs"))
}

/// The receipt path for a session id, or `None` when receipts are
/// unavailable (no resolvable directory) or the id is unsafe as a filename.
///
/// The id reaches us from `--session-id` / `--session NAME`, i.e. user input,
/// so an id carrying a path separator or `..` is rejected rather than
/// allowed to escape the runs directory.
pub fn path_for(session_id: &str) -> Option<PathBuf> {
    if !is_safe_id(session_id) {
        return None;
    }
    Some(runs_dir()?.join(format!("{session_id}.json")))
}

/// Reject ids that would escape the runs directory or name nothing.
fn is_safe_id(id: &str) -> bool {
    !id.is_empty()
        && id != "."
        && id != ".."
        && !id.contains('/')
        && !id.contains('\\')
        && !id.contains('\0')
}

/// Read the receipt for a session id. Best-effort: any missing file, unset
/// directory, or malformed JSON yields `None`, which callers treat as "no
/// receipt" and fall back to their prior behavior.
pub fn load(session_id: &str) -> Option<Receipt> {
    read_at(&path_for(session_id)?)
}

/// Read the record at an exact path. Best-effort, like [`load`].
fn read_at(path: &Path) -> Option<Receipt> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
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

    // -- outcome predicates ------------------------------------------------

    #[test]
    fn terminal_zero_is_not_a_failure() {
        let r = rec(State::Exited, Some(0));
        assert!(r.is_terminal());
        assert!(!r.failed());
    }

    #[test]
    fn terminal_non_zero_is_a_failure() {
        let r = rec(State::Exited, Some(7));
        assert!(r.is_terminal());
        assert!(r.failed());
    }

    #[test]
    fn running_receipt_is_neither_terminal_nor_failed() {
        // The kill -9 case reads as "running", never as success or failure.
        let r = rec(State::Running, None);
        assert!(!r.is_terminal());
        assert!(!r.failed(), "no answer yet must not report as failure");
    }

    #[test]
    fn exited_without_a_code_is_not_terminal() {
        // A truncated/hand-edited record must not be trusted as an outcome.
        assert!(!rec(State::Exited, None).is_terminal());
    }

    // -- serialization -----------------------------------------------------

    #[test]
    fn round_trips_through_json() {
        let r = rec(State::Exited, Some(2));
        let text = serde_json::to_string(&r).unwrap();
        let back: Receipt = serde_json::from_str(&text).unwrap();
        assert_eq!(back.session_id, "sess-1");
        assert_eq!(back.pid, 42);
        assert_eq!(back.state, State::Exited);
        assert_eq!(back.exit_code, Some(2));
        assert_eq!(back.ended_at, Some(200));
    }

    #[test]
    fn running_record_omits_terminal_fields() {
        let text = serde_json::to_string(&rec(State::Running, None)).unwrap();
        assert!(text.contains(r#""state":"running""#), "got: {text}");
        assert!(!text.contains("exit_code"), "got: {text}");
        assert!(!text.contains("ended_at"), "got: {text}");
    }

    // -- id safety ---------------------------------------------------------

    #[test]
    fn rejects_ids_that_would_escape_the_runs_dir() {
        for bad in ["", ".", "..", "../evil", "a/b", "a\\b"] {
            assert!(!is_safe_id(bad), "should reject {bad:?}");
        }
    }

    #[test]
    fn accepts_a_uuid_shaped_id() {
        assert!(is_safe_id("11111111-1111-4111-8111-111111111111"));
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
