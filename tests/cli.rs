//! Mechanical integration tests -- exercise the cwr binary surface
//! without ever calling claude. Covers: clap dispatch, conflict
//! matrix, exit codes, file-side error paths.
//!
//! For tests that actually invoke claude, see `tests/live.rs`
//! (marked `#[ignore]`).

use assert_cmd::Command;
use predicates::prelude::*;

fn cwr() -> Command {
    Command::cargo_bin("cwr").expect("cargo-built cwr binary")
}

// ---------------------------------------------------------------------------
// help / version
// ---------------------------------------------------------------------------

#[test]
fn help_prints_usage_and_exits_zero() {
    cwr()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("Usage: cwr"))
        .stdout(predicate::str::contains("history"))
        .stdout(predicate::str::contains("last"));
}

#[test]
fn version_prints_crate_version_and_exits_zero() {
    cwr()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains("cwr 0.1.0"));
}

#[test]
fn history_help_describes_subcommand() {
    cwr()
        .args(["history", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("List recent sessions"))
        .stdout(predicate::str::contains("--limit"))
        .stdout(predicate::str::contains("--project"));
}

// ---------------------------------------------------------------------------
// no-prompt / missing-source error paths (exit 1, no claude call)
// ---------------------------------------------------------------------------

#[test]
fn missing_file_errors_with_exit_1() {
    cwr()
        .arg("-f")
        .arg("/no/such/path-12345.md")
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("reading prompt"));
}

#[test]
fn empty_file_errors_with_exit_1() {
    let f = tempfile::NamedTempFile::new().unwrap();
    cwr()
        .arg("-f")
        .arg(f.path())
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("empty"));
}

#[test]
fn missing_prepend_errors_with_exit_1() {
    cwr()
        .args(["foo", "--prepend", "/no/such/prepend-12345"])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("reading --prepend"));
}

#[test]
fn dash_with_empty_stdin_errors() {
    cwr()
        .arg("-")
        .write_stdin("")
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("empty stdin"));
}

// ---------------------------------------------------------------------------
// conflict matrix (clap rejects with exit 2 by convention)
// ---------------------------------------------------------------------------

fn assert_conflict(args: &[&str]) {
    cwr()
        .args(args)
        .assert()
        .failure()
        .stderr(predicate::str::contains("cannot be used with"));
}

#[test]
fn conflict_head_and_tail() {
    assert_conflict(&["foo", "--head", "1", "--tail", "1"]);
}

#[test]
fn conflict_head_and_json() {
    assert_conflict(&["foo", "--head", "1", "--json"]);
}

#[test]
fn conflict_code_and_json() {
    assert_conflict(&["foo", "--code", "--json"]);
}

#[test]
fn conflict_positional_and_file() {
    assert_conflict(&["foo", "-f", "Cargo.toml"]);
}

#[test]
fn conflict_positional_and_editor() {
    assert_conflict(&["foo", "-e"]);
}

#[test]
fn conflict_file_and_editor() {
    assert_conflict(&["-f", "Cargo.toml", "-e"]);
}

#[test]
fn conflict_continue_and_resume() {
    assert_conflict(&["foo", "-c", "--resume", "abc123"]);
}

#[test]
fn conflict_pick_and_continue() {
    assert_conflict(&["foo", "--pick", "-c"]);
}

#[test]
fn conflict_readonly_and_full_auto() {
    assert_conflict(&["foo", "--readonly", "--full-auto"]);
}

#[test]
fn conflict_save_and_tee() {
    assert_conflict(&["foo", "--save", "/tmp/a", "--tee", "/tmp/b"]);
}

#[test]
fn conflict_stream_and_json() {
    assert_conflict(&["foo", "--stream", "--json"]);
}

#[test]
fn conflict_stream_and_code() {
    assert_conflict(&["foo", "--stream", "--code"]);
}

#[test]
fn conflict_stream_and_save() {
    assert_conflict(&["foo", "--stream", "--save", "/tmp/a"]);
}

#[test]
fn fork_without_resume_errors() {
    cwr()
        .args(["foo", "--fork"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("--resume"));
}

#[test]
fn var_bad_syntax_errors() {
    cwr()
        .args(["foo", "--var", "no-equals"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("expected K=V"));
}
