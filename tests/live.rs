//! Live integration tests -- these actually invoke the real `claude`
//! binary and cost money. Marked `#[ignore]` so they only run when
//! you opt in:
//!
//!   cargo test -p roba --test live -- --ignored --nocapture
//!
//! Each test runs in a fresh tempdir so sessions don't bleed between
//! tests (claude scopes -c "most recent" by project, and each tempdir
//! is its own project from claude's POV).
//!
//! Budget: at sonnet/haiku rates the full suite is roughly $1-2.
//! Keep prompts short and answers terse to minimize spend.

use assert_cmd::Command;
use predicates::prelude::*;
use std::path::PathBuf;

fn roba_in(dir: &PathBuf) -> Command {
    let mut cmd = Command::cargo_bin("roba").expect("cargo-built roba binary");
    cmd.current_dir(dir);
    cmd
}

fn fresh_dir() -> tempfile::TempDir {
    tempfile::tempdir().expect("create test tempdir")
}

// ---------------------------------------------------------------------------
// basic round-trip
// ---------------------------------------------------------------------------

#[test]
#[ignore]
fn live_basic_prompt() {
    let dir = fresh_dir();
    roba_in(&dir.path().to_path_buf())
        .arg("respond with the single word: pong")
        .assert()
        .success()
        .stdout(predicate::str::contains("pong"));
}

#[test]
#[ignore]
fn live_quiet_suppresses_stderr_metadata() {
    let dir = fresh_dir();
    let out = roba_in(&dir.path().to_path_buf())
        .args(["-q", "respond with the single word: hush"])
        .output()
        .expect("run roba");
    assert!(out.status.success(), "roba failed: {out:?}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("cost"),
        "expected no cost footer with -q, got stderr: {stderr}"
    );
}

#[test]
#[ignore]
fn live_json_output_is_valid_json() {
    let dir = fresh_dir();
    let out = roba_in(&dir.path().to_path_buf())
        .args(["--json", "respond with the single word: jay"])
        .output()
        .expect("run roba");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let parsed: serde_json::Value =
        serde_json::from_str(&stdout).expect("--json produced non-JSON stdout");
    assert!(parsed.get("session_id").is_some());
    assert!(parsed.get("result").is_some());
    assert!(parsed.get("duration_ms").is_some());
}

// ---------------------------------------------------------------------------
// output shaping
// ---------------------------------------------------------------------------

#[test]
#[ignore]
fn live_code_extraction_strips_fences() {
    let dir = fresh_dir();
    let out = roba_in(&dir.path().to_path_buf())
        .args([
            "write exactly one rust function called id that takes i32 and returns it. fenced code block, no other prose.",
            "--code",
        ])
        .output()
        .expect("run roba");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.contains("```"),
        "--code did not strip fences: {stdout}"
    );
    assert!(stdout.contains("fn id"), "expected fn id in output, got: {stdout}");
}

#[test]
#[ignore]
fn live_head_caps_line_count() {
    let dir = fresh_dir();
    let out = roba_in(&dir.path().to_path_buf())
        .args(["list five fruits, one per line, nothing else", "--head", "3"])
        .output()
        .expect("run roba");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    // body has at most 3 non-empty lines (println adds a final newline)
    let nonempty = stdout.lines().filter(|l| !l.trim().is_empty()).count();
    assert!(nonempty <= 3, "expected <=3 non-empty lines, got {nonempty} in: {stdout}");
}

// ---------------------------------------------------------------------------
// session continuation + fork
// ---------------------------------------------------------------------------

#[test]
#[ignore]
fn live_continue_carries_context() {
    let dir = fresh_dir();
    let path = dir.path().to_path_buf();

    roba_in(&path)
        .arg("remember the word: zenith")
        .assert()
        .success();

    let out = roba_in(&path)
        .args(["-c", "what word did I ask you to remember"])
        .output()
        .expect("run roba -c");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.to_lowercase().contains("zenith"),
        "expected 'zenith' from continued session, got: {stdout}"
    );
}

#[test]
#[ignore]
fn live_resume_fork_creates_new_session_id() {
    let dir = fresh_dir();
    let path = dir.path().to_path_buf();

    // 1. seed a session and grab its id from --json
    let seed = roba_in(&path)
        .args(["--json", "respond with the single word: seed"])
        .output()
        .expect("seed run");
    let seed_json: serde_json::Value = serde_json::from_slice(&seed.stdout).expect("json");
    let seed_id = seed_json["session_id"].as_str().expect("session_id").to_string();

    // 2. resume + fork -- expect a NEW session id in the result
    let fork = roba_in(&path)
        .args([
            "--json",
            "--resume",
            &seed_id,
            "--fork",
            "respond with the single word: forked",
        ])
        .output()
        .expect("fork run");
    let fork_json: serde_json::Value = serde_json::from_slice(&fork.stdout).expect("json");
    let fork_id = fork_json["session_id"].as_str().expect("session_id");

    assert_ne!(
        seed_id, fork_id,
        "expected fork to produce a new session id"
    );
}

// ---------------------------------------------------------------------------
// streaming + tool use
// ---------------------------------------------------------------------------

#[test]
#[ignore]
fn live_stream_emits_to_stdout() {
    let dir = fresh_dir();
    let out = roba_in(&dir.path().to_path_buf())
        .args(["respond with the single word: streamed", "--stream"])
        .output()
        .expect("run roba --stream");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.to_lowercase().contains("streamed"),
        "expected 'streamed' on stdout, got: {stdout}"
    );
}

// ---------------------------------------------------------------------------
// composition: attach / git / var
// ---------------------------------------------------------------------------

#[test]
#[ignore]
fn live_attach_makes_files_visible_to_claude() {
    let dir = fresh_dir();
    let path = dir.path().to_path_buf();
    let attach_path = path.join("greeting.txt");
    std::fs::write(&attach_path, "secret word: kazoo").expect("write attach file");

    let glob = path.join("greeting.txt");
    let out = roba_in(&path)
        .args([
            "--attach",
            glob.to_str().unwrap(),
            "what is the secret word in the attached file? answer with just the word.",
        ])
        .output()
        .expect("run roba --attach");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.to_lowercase().contains("kazoo"),
        "expected 'kazoo' to be referenced from attached file, got: {stdout}"
    );
}

#[test]
#[ignore]
fn live_var_substitution_reaches_model() {
    let dir = fresh_dir();
    let path = dir.path().to_path_buf();
    let tpl = path.join("tpl.md");
    std::fs::write(&tpl, "Respond with exactly: {{TARGET}}").expect("write tpl");

    let out = roba_in(&path)
        .args([
            "-f",
            tpl.to_str().unwrap(),
            "--var",
            "TARGET=lighthouse",
        ])
        .output()
        .expect("run roba -f --var");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.to_lowercase().contains("lighthouse"),
        "expected substituted value to reach the model, got: {stdout}"
    );
}

// ---------------------------------------------------------------------------
// save / tee
// ---------------------------------------------------------------------------

#[test]
#[ignore]
fn live_save_writes_file_and_keeps_stdout_clean() {
    let dir = fresh_dir();
    let path = dir.path().to_path_buf();
    let target = path.join("out.md");

    let out = roba_in(&path)
        .args([
            "respond with the single word: saved",
            "--save",
            target.to_str().unwrap(),
        ])
        .output()
        .expect("run roba --save");
    assert!(out.status.success());
    assert!(
        out.stdout.is_empty(),
        "expected empty stdout with --save, got: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    let file_contents = std::fs::read_to_string(&target).expect("read saved file");
    assert!(
        file_contents.to_lowercase().contains("saved"),
        "expected 'saved' in saved file, got: {file_contents}"
    );
}

#[test]
#[ignore]
fn live_save_json_extension_promotes_to_json() {
    let dir = fresh_dir();
    let path = dir.path().to_path_buf();
    let target = path.join("out.json");

    roba_in(&path)
        .args([
            "respond with the single word: jp",
            "--save",
            target.to_str().unwrap(),
        ])
        .assert()
        .success();

    let file_contents = std::fs::read_to_string(&target).expect("read saved file");
    let parsed: serde_json::Value =
        serde_json::from_str(&file_contents).expect("saved file should be JSON");
    assert!(parsed.get("session_id").is_some());
}
