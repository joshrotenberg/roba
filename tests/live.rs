//! Live integration tests -- these actually invoke the real `claude`
//! binary and cost money. Marked `#[ignore]` so they only run when
//! you opt in:
//!
//!   cargo test --test live -- --ignored --nocapture
//!   just live              # equivalent
//!   just live-smoke        # ~3 tests, ~$0.05, ~10s
//!   just live-perms        # one category
//!
//! Each test runs in a fresh tempdir via `-C PATH` so sessions don't
//! bleed between tests (claude scopes sessions by cwd / project, and
//! each tempdir is its own project from claude's POV).
//!
//! All tests default to `--model haiku` for cost. A test that cares
//! about a specific model can append `--model <id>` -- clap's
//! last-wins semantics applies.
//!
//! Budget: at haiku rates the full suite is well under $1.
//! Keep prompts short and answers terse to minimize spend.
//!
//! Naming convention: `live_<category>_<descriptor>` so `cargo test`
//! filters work: `live_perms`, `live_output`, `live_session`, etc.

use assert_cmd::Command;
use predicates::prelude::*;
use std::path::Path;

/// Run `roba` against `dir` via `-C`, defaulting to the haiku model.
/// Tests that need a specific model can append `--model <id>` later;
/// clap's last-occurrence-wins semantics applies.
fn roba_in(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("roba").expect("cargo-built roba binary");
    cmd.args([
        "-C",
        dir.to_str().expect("utf-8 tempdir path"),
        "--model",
        "haiku",
    ]);
    cmd
}

fn fresh_dir() -> tempfile::TempDir {
    tempfile::tempdir().expect("create test tempdir")
}

/// Make a tempdir pre-seeded with a `roba.toml`. Returns the TempDir
/// so the caller can keep it alive for the duration of the test.
#[allow(dead_code)]
fn fixture_with_config(content: &str) -> tempfile::TempDir {
    let tmp = fresh_dir();
    std::fs::write(tmp.path().join("roba.toml"), content).expect("write roba.toml");
    tmp
}

// ---------------------------------------------------------------------------
// basic round-trip
// ---------------------------------------------------------------------------

#[test]
#[ignore]
fn live_basic_prompt() {
    let dir = fresh_dir();
    roba_in(dir.path())
        .arg("respond with the single word: pong")
        .assert()
        .success()
        .stdout(predicate::str::contains("pong"));
}

#[test]
#[ignore]
fn live_basic_cwd_scopes_session_to_path() {
    // Verify -C scopes claude's session to the given path: a seeded
    // session in dir A is reachable from -c when we point -C at A again,
    // even though the test process's cwd never changed.
    let dir = fresh_dir();
    roba_in(dir.path())
        .arg("remember the word: aurora")
        .assert()
        .success();

    let out = roba_in(dir.path())
        .args(["-c", "what word did I ask you to remember"])
        .output()
        .expect("run roba -c");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.to_lowercase().contains("aurora"),
        "expected -C to scope sessions to the tmp dir, got: {stdout}"
    );
}

// ---------------------------------------------------------------------------
// output shaping
// ---------------------------------------------------------------------------

#[test]
#[ignore]
fn live_output_quiet_no_metadata() {
    let dir = fresh_dir();
    let out = roba_in(dir.path())
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
fn live_output_json_valid() {
    let dir = fresh_dir();
    let out = roba_in(dir.path())
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

#[test]
#[ignore]
fn live_output_code_strips_fences() {
    let dir = fresh_dir();
    let out = roba_in(dir.path())
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
    assert!(
        stdout.contains("fn id"),
        "expected fn id in output, got: {stdout}"
    );
}

#[test]
#[ignore]
fn live_output_head_caps_lines() {
    let dir = fresh_dir();
    let out = roba_in(dir.path())
        .args([
            "list five fruits, one per line, nothing else",
            "--head",
            "3",
        ])
        .output()
        .expect("run roba");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let nonempty = stdout.lines().filter(|l| !l.trim().is_empty()).count();
    assert!(
        nonempty <= 3,
        "expected <=3 non-empty lines, got {nonempty} in: {stdout}"
    );
}

#[test]
#[ignore]
fn live_output_save_writes_file() {
    let dir = fresh_dir();
    let target = dir.path().join("out.md");

    let out = roba_in(dir.path())
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
fn live_output_save_json_extension() {
    let dir = fresh_dir();
    let target = dir.path().join("out.json");

    roba_in(dir.path())
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

// ---------------------------------------------------------------------------
// session continuation + fork
// ---------------------------------------------------------------------------

#[test]
#[ignore]
fn live_session_continue_carries_context() {
    let dir = fresh_dir();
    roba_in(dir.path())
        .arg("remember the word: zenith")
        .assert()
        .success();

    let out = roba_in(dir.path())
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
fn live_session_resume_fork_new_id() {
    let dir = fresh_dir();

    // 1. seed a session and grab its id from --json
    let seed = roba_in(dir.path())
        .args(["--json", "respond with the single word: seed"])
        .output()
        .expect("seed run");
    let seed_json: serde_json::Value = serde_json::from_slice(&seed.stdout).expect("json");
    let seed_id = seed_json["session_id"]
        .as_str()
        .expect("session_id")
        .to_string();

    // 2. resume + fork -- expect a NEW session id in the result
    let fork = roba_in(dir.path())
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
    let out = roba_in(dir.path())
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
// composition: attach / var
// ---------------------------------------------------------------------------

#[test]
#[ignore]
fn live_compose_attach_files_visible() {
    let dir = fresh_dir();
    let attach_path = dir.path().join("greeting.txt");
    std::fs::write(&attach_path, "secret word: kazoo").expect("write attach file");

    let out = roba_in(dir.path())
        .args([
            "--attach",
            attach_path.to_str().unwrap(),
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
fn live_compose_var_substitution() {
    let dir = fresh_dir();
    let tpl = dir.path().join("tpl.md");
    std::fs::write(&tpl, "Respond with exactly: {{TARGET}}").expect("write tpl");

    let out = roba_in(dir.path())
        .args(["-f", tpl.to_str().unwrap(), "--var", "TARGET=lighthouse"])
        .output()
        .expect("run roba -f --var");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.to_lowercase().contains("lighthouse"),
        "expected substituted value to reach the model, got: {stdout}"
    );
}
