//! Mechanical scenarios for detached-run receipts (#441).
//!
//! A detached run's typed exit code used to vanish: `show` reconstructed the
//! envelope with `is_error: false` hardcoded, so an orchestrator following
//! the documented `--detach` + `show --wait` recipe reported a capped,
//! auth-failed, or crashed run as success. These scenarios pin the fix at the
//! process boundary, with no claude call: the receipt is planted on disk and
//! `show` is asked what it reports.
//!
//! Isolation comes from the shared fixture builder (`HOME` pinned, so
//! `~/.claude/projects` is the fixture's) plus `ROBA_STATE_DIR` (so receipts
//! land in a temp dir, never the real one).

mod common;

use std::path::Path;

use common::{Project, project};

const SESSION: &str = "11111111-1111-4111-8111-111111111111";

/// Write a minimal session JSONL under the fixture's `~/.claude/projects`,
/// with one completed assistant turn. `show` locates a session by scanning
/// project dirs for `<id>.jsonl`, so the slug is arbitrary.
fn plant_session(proj: &Project, session_id: &str) {
    let dir = proj.config_home.join(".claude").join("projects").join("-p");
    std::fs::create_dir_all(&dir).expect("mkdir projects");
    let line = serde_json::json!({
        "type": "assistant",
        "message": {
            "content": [{"type": "text", "text": "the answer"}],
            "stop_reason": "end_turn"
        }
    });
    std::fs::write(dir.join(format!("{session_id}.jsonl")), format!("{line}\n"))
        .expect("write session jsonl");
}

/// Plant a receipt exactly where `runs_dir()` looks under `ROBA_STATE_DIR`.
fn plant_receipt(state_dir: &Path, session_id: &str, body: serde_json::Value) {
    let runs = state_dir.join("runs");
    std::fs::create_dir_all(&runs).expect("mkdir runs");
    std::fs::write(runs.join(format!("{session_id}.json")), body.to_string())
        .expect("write receipt");
}

fn terminal(session_id: &str, code: i32) -> serde_json::Value {
    serde_json::json!({
        "session_id": session_id,
        "pid": 4242,
        "started_at": 1_700_000_000u64,
        "state": "exited",
        "exit_code": code,
        "ended_at": 1_700_000_010u64,
    })
}

// -- the fix: a recorded failure is reported as one -----------------------

#[test]
fn terminal_failure_receipt_makes_show_report_the_error() {
    let proj = project().build();
    let state = common::fresh_dir();
    plant_session(&proj, SESSION);
    // exit 7 = the recoverable `--max-budget-usd` cap: the exact shape of
    // run that used to be reported as a clean success.
    plant_receipt(state.path(), SESSION, terminal(SESSION, 7));

    let out = proj
        .roba()
        .args(["show", SESSION, "--json"])
        .env("ROBA_STATE_DIR", state.path())
        .output()
        .expect("run show");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let json: serde_json::Value = serde_json::from_str(&stdout).expect("clean json envelope");
    assert_eq!(
        json["result"]["is_error"], true,
        "a recorded non-zero exit must not reconstruct as success: {stdout}"
    );
    assert_eq!(json["result"]["exit_code"], 7, "got: {stdout}");
    assert_eq!(
        out.status.code(),
        Some(7),
        "show must propagate the detached run's typed exit code"
    );
}

#[test]
fn terminal_success_receipt_reports_success_and_the_code() {
    let proj = project().build();
    let state = common::fresh_dir();
    plant_session(&proj, SESSION);
    plant_receipt(state.path(), SESSION, terminal(SESSION, 0));

    let out = proj
        .roba()
        .args(["show", SESSION, "--json"])
        .env("ROBA_STATE_DIR", state.path())
        .output()
        .expect("run show");

    let json: serde_json::Value = serde_json::from_slice(&out.stdout).expect("clean json envelope");
    assert_eq!(json["result"]["is_error"], false);
    assert_eq!(json["result"]["exit_code"], 0);
    assert_eq!(out.status.code(), Some(0));
}

#[test]
fn footer_carries_the_exit_code() {
    let proj = project().build();
    let state = common::fresh_dir();
    plant_session(&proj, SESSION);
    plant_receipt(state.path(), SESSION, terminal(SESSION, 2));

    let out = proj
        .roba()
        .args(["show", SESSION])
        .env("ROBA_STATE_DIR", state.path())
        .output()
        .expect("run show");

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("exit 2"),
        "footer missing the code: {stderr}"
    );
    // The answer channel stays the answer channel.
    assert!(String::from_utf8_lossy(&out.stdout).contains("the answer"));
}

// -- no receipt: byte-for-byte the old behavior ---------------------------

#[test]
fn no_receipt_falls_back_to_the_stop_reason_heuristic() {
    let proj = project().build();
    // An empty state dir: no receipt for this (or any) session.
    let state = common::fresh_dir();
    plant_session(&proj, SESSION);

    let out = proj
        .roba()
        .args(["show", SESSION, "--json"])
        .env("ROBA_STATE_DIR", state.path())
        .output()
        .expect("run show");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let json: serde_json::Value = serde_json::from_str(&stdout).expect("clean json envelope");
    assert_eq!(json["result"]["is_error"], false);
    assert!(
        json["result"].get("exit_code").is_none(),
        "an absent receipt must not fabricate an exit code: {stdout}"
    );
    assert_eq!(out.status.code(), Some(0));
}

#[test]
fn a_malformed_receipt_is_ignored_rather_than_fatal() {
    let proj = project().build();
    let state = common::fresh_dir();
    plant_session(&proj, SESSION);
    let runs = state.path().join("runs");
    std::fs::create_dir_all(&runs).unwrap();
    std::fs::write(runs.join(format!("{SESSION}.json")), "{ not json").unwrap();

    let out = proj
        .roba()
        .args(["show", SESSION, "--json"])
        .env("ROBA_STATE_DIR", state.path())
        .output()
        .expect("run show");

    assert_eq!(
        out.status.code(),
        Some(0),
        "a disposable artifact must never break the read path"
    );
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).expect("clean json envelope");
    assert_eq!(json["result"]["is_error"], false);
}

// -- --wait: the receipt is authoritative ---------------------------------

#[test]
fn wait_reports_a_run_that_died_before_writing_a_log() {
    // The auth-failure shape: the child exited 2 and claude never persisted
    // a session. Without a receipt this waits out the full timeout and
    // reports a timeout; with one it reports the real failure immediately.
    let proj = project().build();
    let state = common::fresh_dir();
    plant_receipt(state.path(), SESSION, terminal(SESSION, 2));

    let out = proj
        .roba()
        .args(["show", SESSION, "--wait", "--timeout", "30"])
        .env("ROBA_STATE_DIR", state.path())
        .output()
        .expect("run show --wait");

    assert_eq!(out.status.code(), Some(2), "must mirror the recorded code");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("without writing a session log"),
        "got: {stderr}"
    );
    assert!(
        out.stdout.is_empty(),
        "no answer exists, so stdout stays empty"
    );
}

#[test]
fn wait_keeps_polling_while_the_receipt_says_running() {
    // A start record is not an outcome: `--wait` must keep waiting (and here
    // time out) rather than treat "started" as "finished".
    let proj = project().build();
    let state = common::fresh_dir();
    plant_receipt(
        state.path(),
        SESSION,
        serde_json::json!({
            "session_id": SESSION,
            "pid": 4242,
            "started_at": 1_700_000_000u64,
            "state": "running",
        }),
    );

    let out = proj
        .roba()
        .args(["show", SESSION, "--wait", "--timeout", "1"])
        .env("ROBA_STATE_DIR", state.path())
        .output()
        .expect("run show --wait");

    assert_eq!(
        out.status.code(),
        Some(4),
        "documented timeout exit: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn wait_returns_on_a_terminal_receipt_even_if_the_log_looks_unfinished() {
    // The crash case the heuristic cannot see: the last assistant turn is
    // mid-`tool_use`, so `stop_reason` says "more is coming" -- forever.
    // The receipt says the process is gone.
    let proj = project().build();
    let state = common::fresh_dir();
    let dir = proj.config_home.join(".claude").join("projects").join("-p");
    std::fs::create_dir_all(&dir).unwrap();
    let line = serde_json::json!({
        "type": "assistant",
        "message": {
            "content": [{"type": "text", "text": "partial"}],
            "stop_reason": "tool_use"
        }
    });
    std::fs::write(dir.join(format!("{SESSION}.jsonl")), format!("{line}\n")).unwrap();
    plant_receipt(state.path(), SESSION, terminal(SESSION, 1));

    let out = proj
        .roba()
        .args(["show", SESSION, "--wait", "--timeout", "30", "--json"])
        .env("ROBA_STATE_DIR", state.path())
        .output()
        .expect("run show --wait");

    assert_eq!(
        out.status.code(),
        Some(1),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).expect("clean json envelope");
    assert_eq!(json["result"]["is_error"], true);
    assert_eq!(json["result"]["exit_code"], 1);
}

// -- ownership: an inherited ROBA_RECEIPT must not be claimed --------------
//
// `ROBA_RECEIPT` reaches the detached child through the ENVIRONMENT, and env
// is inherited by everything the child spawns: `claude`, and every process
// claude runs via Bash -- including a nested `roba`. Without an ownership
// check that nested roba stamps the outer run's receipt `exited/0` on its own
// exit, and `show --wait` returns that wrong success to the orchestrator
// while the real run is still working.

/// A pid that is certainly not the roba process under test.
fn foreign_pid() -> u32 {
    std::process::id().wrapping_add(1)
}

fn running(session_id: &str, pid: u32) -> serde_json::Value {
    serde_json::json!({
        "session_id": session_id,
        "pid": pid,
        "started_at": 1_700_000_000u64,
        "state": "running",
    })
}

/// Read back the planted receipt.
fn read_receipt(state_dir: &Path, session_id: &str) -> serde_json::Value {
    let text = std::fs::read_to_string(state_dir.join("runs").join(format!("{session_id}.json")))
        .expect("receipt still exists");
    serde_json::from_str(&text).expect("receipt is valid json")
}

#[test]
fn a_nested_roba_does_not_close_another_runs_receipt() {
    // The repro: any roba invocation that merely inherited ROBA_RECEIPT,
    // against a live run's record. Before the ownership check this rewrote
    // the record as `exited, exit_code: 0`.
    let proj = project().build();
    let state = common::fresh_dir();
    let path = state.path().join("runs").join(format!("{SESSION}.json"));
    plant_receipt(state.path(), SESSION, running(SESSION, foreign_pid()));

    let out = proj
        .roba()
        .args(["alias", "list"])
        .env("ROBA_STATE_DIR", state.path())
        .env("ROBA_RECEIPT", &path)
        .output()
        .expect("run a nested roba");
    assert!(
        out.status.success(),
        "the nested command itself still succeeds: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let rec = read_receipt(state.path(), SESSION);
    assert_eq!(rec["state"], "running", "another run's record was closed");
    assert!(rec.get("exit_code").is_none(), "got: {rec}");
    assert_eq!(rec["pid"], foreign_pid(), "the owner's pid was overwritten");
    assert_eq!(rec["started_at"], 1_700_000_000u64);
}

#[test]
fn a_nested_roba_does_not_close_another_runs_receipt_on_the_error_path() {
    // The terminal record is written at every exit seam, so the error seam
    // needs the same guard as the success seam.
    let proj = project().build();
    let state = common::fresh_dir();
    let path = state.path().join("runs").join(format!("{SESSION}.json"));
    plant_receipt(state.path(), SESSION, running(SESSION, foreign_pid()));

    let out = proj
        .roba()
        .args(["show", "22222222-2222-4222-8222-222222222222"])
        .env("ROBA_STATE_DIR", state.path())
        .env("ROBA_RECEIPT", &path)
        .output()
        .expect("run a nested roba that fails");
    assert!(!out.status.success(), "the nested command fails as usual");

    let rec = read_receipt(state.path(), SESSION);
    assert_eq!(rec["state"], "running", "another run's record was closed");
    assert!(rec.get("exit_code").is_none(), "got: {rec}");
}

#[test]
fn wait_is_not_short_circuited_by_a_nested_roba() {
    // The consequence the guard buys, at the recipe level: `show --wait`
    // against a live run keeps waiting (here, times out) instead of
    // returning the nested process's success.
    let proj = project().build();
    let state = common::fresh_dir();
    let path = state.path().join("runs").join(format!("{SESSION}.json"));
    plant_receipt(state.path(), SESSION, running(SESSION, foreign_pid()));

    let nested = proj
        .roba()
        .args(["alias", "list"])
        .env("ROBA_STATE_DIR", state.path())
        .env("ROBA_RECEIPT", &path)
        .output()
        .expect("run a nested roba");
    assert!(nested.status.success());

    let out = proj
        .roba()
        .args(["show", SESSION, "--wait", "--timeout", "1"])
        .env("ROBA_STATE_DIR", state.path())
        .output()
        .expect("run show --wait");

    assert_eq!(
        out.status.code(),
        Some(4),
        "documented timeout exit, not a reported success: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn a_receipt_this_process_owns_is_still_written() {
    // The guard must not break the real path: with no record on disk the
    // run claims the receipt and closes it with its own typed exit code.
    let proj = project().build();
    let state = common::fresh_dir();
    let path = state.path().join("runs").join(format!("{SESSION}.json"));
    std::fs::create_dir_all(state.path().join("runs")).unwrap();

    let out = proj
        .roba()
        .args(["show", "22222222-2222-4222-8222-222222222222"])
        .env("ROBA_STATE_DIR", state.path())
        .env("ROBA_RECEIPT", &path)
        .output()
        .expect("run roba with an unclaimed receipt");
    let code = out.status.code().expect("exited normally");
    assert_ne!(code, 0, "a missing session is an error");

    let rec = read_receipt(state.path(), SESSION);
    assert_eq!(rec["state"], "exited");
    assert_eq!(rec["exit_code"], code, "the run's own typed code");
}

#[test]
fn a_terminal_record_from_a_prior_run_is_reclaimed() {
    // A finished record is not a live owner: a new run reusing the id must
    // still get a receipt, otherwise a reused session id silently loses one.
    let proj = project().build();
    let state = common::fresh_dir();
    let path = state.path().join("runs").join(format!("{SESSION}.json"));
    plant_receipt(state.path(), SESSION, terminal(SESSION, 7));

    let out = proj
        .roba()
        .args(["show", "22222222-2222-4222-8222-222222222222"])
        .env("ROBA_STATE_DIR", state.path())
        .env("ROBA_RECEIPT", &path)
        .output()
        .expect("run roba over a finished record");
    let code = out.status.code().expect("exited normally");

    let rec = read_receipt(state.path(), SESSION);
    assert_eq!(rec["state"], "exited");
    assert_eq!(rec["exit_code"], code, "replaced by this run's outcome");
    assert_ne!(rec["pid"], 4242, "the new owner's pid");
}
