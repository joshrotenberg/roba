//! Run receipts -- the durable outcome record a detached roba run leaves
//! behind, as a consumer-facing contract.
//!
//! A detached run (`roba --detach`) writes a small JSON record keyed by
//! session id: a `running` record early in the run, then a terminal record at
//! the exit seam carrying the typed exit code. `roba show` prefers the
//! receipt over its `stop_reason` heuristic, and downstream harnesses (a
//! scheduler, a watcher) can read receipts directly through this module
//! without depending on the roba binary crate.
//!
//! ```json
//! {"session_id":"...","pid":41234,"started_at":1753,"state":"running"}
//! {"session_id":"...","pid":41234,"started_at":1753,"state":"exited",
//!  "exit_code":7,"ended_at":1789,"cost_usd":0.042}
//! ```
//!
//! # The contract, for consumers
//!
//! - **One writer.** Only the detached roba child writes a receipt; ownership
//!   is enforced by a pid check against the record on disk, so a nested roba
//!   that merely inherited the receipt env cannot clobber it. Consumers only
//!   read.
//! - **Best-effort, never load-bearing.** A missing, unreadable, or malformed
//!   receipt means "no information" -- it is NEVER a failure signal. Only a
//!   terminal record ([`Receipt::is_terminal`]) is authoritative about the
//!   outcome; a `running` record says nothing (the run may be live, or it may
//!   have been SIGKILLed before reaching its exit seam).
//! - **A receipt describes a finished run.** It is not a job table, a queue,
//!   or a supervisor.
//! - **Receipts expire.** The writer sweeps records older than ~30 days when
//!   a new detached run starts, so the directory self-heals. Consumers must
//!   not treat an absent old receipt as meaningful.
//!
//! # Location
//!
//! `$ROBA_STATE_DIR/runs/<id>.json`, else `$XDG_STATE_HOME/roba/runs/<id>.json`,
//! else `~/.local/state/roba/runs/<id>.json` -- see [`runs_dir`]. Deliberately
//! NOT `.roba/` (the hermetic config bundle directory, which is authored and
//! potentially committed); receipts are per-machine and disposable.

use std::path::{Path, PathBuf};

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
    /// Observed spend for the run, from claude's own result event
    /// (`total_cost_usd`), when the run produced one. Absent means unknown --
    /// a run that died before a result event, or a run whose auth reports no
    /// cost -- never zero.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
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

/// Resolve the user's home directory from `HOME` (Unix) or `USERPROFILE`
/// (Windows). Private: consumers resolve receipt locations via [`runs_dir`] /
/// [`path_for`], not raw home paths.
fn home_dir() -> Option<PathBuf> {
    if let Ok(h) = std::env::var("HOME")
        && !h.is_empty()
    {
        return Some(PathBuf::from(h));
    }
    if let Ok(h) = std::env::var("USERPROFILE")
        && !h.is_empty()
    {
        return Some(PathBuf::from(h));
    }
    None
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
    home_dir().map(|h| h.join(".local").join("state").join("roba").join("runs"))
}

/// The receipt path for a session id, or `None` when receipts are
/// unavailable (no resolvable directory) or the id is unsafe as a filename.
///
/// The id reaches roba from `--session-id` / `--session NAME`, i.e. user
/// input, so an id carrying a path separator or `..` is rejected rather than
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

/// Read the record at an exact path. Best-effort, like [`load`]. Exposed for
/// callers that hold a receipt path directly (roba's own writer, a consumer
/// scanning [`runs_dir`]).
pub fn read_at(path: &Path) -> Option<Receipt> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
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
            cost_usd: None,
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
    fn cost_serializes_only_when_present() {
        let mut r = rec(State::Exited, Some(0));
        assert!(!serde_json::to_string(&r).unwrap().contains("cost_usd"));
        r.cost_usd = Some(0.042);
        let text = serde_json::to_string(&r).unwrap();
        assert!(text.contains(r#""cost_usd":0.042"#), "got: {text}");
        let back: Receipt = serde_json::from_str(&text).unwrap();
        assert_eq!(back.cost_usd, Some(0.042));
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

    // -- best-effort reads -------------------------------------------------

    #[test]
    fn read_at_missing_or_malformed_is_none() {
        let dir = tempfile::tempdir().unwrap();
        assert!(read_at(&dir.path().join("absent.json")).is_none());
        let bad = dir.path().join("bad.json");
        std::fs::write(&bad, "{not json").unwrap();
        assert!(read_at(&bad).is_none());
    }
}
