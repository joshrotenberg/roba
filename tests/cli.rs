//! Mechanical integration tests -- exercise the roba binary surface
//! without ever calling claude. Covers: clap dispatch, conflict
//! matrix, exit codes, file-side error paths.
//!
//! For tests that actually invoke claude, see `tests/live.rs`
//! (marked `#[ignore]`).

use assert_cmd::Command;
use predicates::prelude::*;

fn roba() -> Command {
    Command::cargo_bin("roba").expect("cargo-built roba binary")
}

// ---------------------------------------------------------------------------
// help / version
// ---------------------------------------------------------------------------

#[test]
fn help_prints_usage_and_exits_zero() {
    roba()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("Usage: roba"))
        .stdout(predicate::str::contains("history"))
        .stdout(predicate::str::contains("last"));
}

#[test]
fn version_prints_crate_version_and_exits_zero() {
    roba()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains("roba 0.1.0"));
}

#[test]
fn history_help_describes_subcommand() {
    roba()
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
    roba()
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
    roba()
        .arg("-f")
        .arg(f.path())
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("empty"));
}

#[test]
fn missing_prepend_errors_with_exit_1() {
    roba()
        .args(["foo", "--prepend", "/no/such/prepend-12345"])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("reading --prepend"));
}

#[test]
fn dash_with_empty_stdin_errors() {
    roba()
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
    roba()
        .args(args)
        .assert()
        .failure()
        .stderr(predicate::str::contains("cannot be used with"));
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
fn conflict_fresh_and_continue() {
    assert_conflict(&["foo", "--fresh", "-c"]);
}

#[test]
fn conflict_fresh_and_resume() {
    assert_conflict(&["foo", "--fresh", "--resume", "abc123"]);
}

#[test]
fn conflict_fresh_and_pick() {
    assert_conflict(&["foo", "--fresh", "--pick"]);
}

// ---------------------------------------------------------------------------
// -C / --cwd
// ---------------------------------------------------------------------------

#[test]
fn cwd_to_missing_dir_errors_cleanly() {
    roba()
        .args(["-C", "/no/such/dir/should/exist/xyz", "foo"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("cannot change directory"));
}

#[test]
fn help_mentions_cwd_flag() {
    roba()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("--cwd"));
}

#[test]
fn help_mentions_worktree_flag() {
    roba()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("--worktree"));
}

#[test]
fn worktree_alone_parses_and_fails_at_runtime_not_clap() {
    // -w by itself + missing prepend file: parse must succeed,
    // failure comes from the runtime (not clap), confirming the
    // presence form still works.
    roba()
        .args(["foo", "-w", "--prepend", "/no/such/worktree-test-noname"])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("reading --prepend"));
}

#[test]
fn worktree_named_with_equals_parses() {
    // -w=NAME + missing prepend file: parse must succeed (clap
    // accepts the `=` form), failure is the runtime read error.
    roba()
        .args([
            "foo",
            "-w=mybranch",
            "--prepend",
            "/no/such/worktree-test-named",
        ])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("reading --prepend"));
}

#[test]
fn worktree_long_space_name_is_rejected() {
    // require_equals = true: `--worktree NAME` (space form) is a
    // clap parse error, not silently consumed.
    roba()
        .args(["foo", "--worktree", "mybranch"])
        .assert()
        .failure()
        .stderr(
            predicate::str::contains("equal").or(predicate::str::contains("unexpected argument")),
        );
}

// ---------------------------------------------------------------------------
// --show-permissions (resolves layers, prints preview, no claude call)
// ---------------------------------------------------------------------------

/// A profile-free project dir (just a `.git` boundary) so config
/// walk-up finds nothing and the resolved permission set is purely
/// CLI + built-in defaults.
fn empty_project() -> tempfile::TempDir {
    make_dir_with_files(&[(".git/HEAD", "")])
}

#[test]
fn show_permissions_default_no_profile() {
    let project = empty_project();
    let user_home = tempfile::tempdir().expect("user home");
    let out = roba()
        .args([
            "-C",
            project.path().to_str().unwrap(),
            "--show-permissions",
            "ignored",
        ])
        .env("XDG_CONFIG_HOME", user_home.path())
        .env_remove("ROBA_PROFILE")
        .output()
        .expect("run");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    // The answer stream stays clean -- the preview is metadata.
    assert!(out.stdout.is_empty(), "stdout should be empty for preview");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("allow:"), "got:\n{stderr}");
    assert!(stderr.contains("Read"), "got:\n{stderr}");
    assert!(stderr.contains("Glob"), "got:\n{stderr}");
    assert!(stderr.contains("Grep"), "got:\n{stderr}");
    assert!(stderr.contains("[default]"), "got:\n{stderr}");
}

#[test]
fn show_permissions_with_writable_via_cli() {
    let project = empty_project();
    let user_home = tempfile::tempdir().expect("user home");
    let out = roba()
        .args([
            "-C",
            project.path().to_str().unwrap(),
            "--writable",
            "--show-permissions",
        ])
        .env("XDG_CONFIG_HOME", user_home.path())
        .env_remove("ROBA_PROFILE")
        .output()
        .expect("run");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("Edit"), "got:\n{stderr}");
    assert!(stderr.contains("Write"), "got:\n{stderr}");
    assert!(stderr.contains("[CLI]"), "got:\n{stderr}");
}

#[test]
fn show_permissions_with_full_auto_via_cli() {
    let project = empty_project();
    let user_home = tempfile::tempdir().expect("user home");
    let out = roba()
        .args([
            "-C",
            project.path().to_str().unwrap(),
            "--full-auto",
            "--show-permissions",
        ])
        .env("XDG_CONFIG_HOME", user_home.path())
        .env_remove("ROBA_PROFILE")
        .output()
        .expect("run");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("all tools allowed (--full-auto from CLI)"),
        "got:\n{stderr}"
    );
}

#[test]
fn show_permissions_does_not_spawn_claude() {
    // A trivially low budget would fail any real claude call. Exit 0
    // here proves --show-permissions returns before dispatch.
    let project = empty_project();
    let user_home = tempfile::tempdir().expect("user home");
    let out = roba()
        .args(["-C", project.path().to_str().unwrap(), "--show-permissions"])
        .env("XDG_CONFIG_HOME", user_home.path())
        .env_remove("ROBA_PROFILE")
        .env("ROBA_BUDGET", "0.00001")
        .output()
        .expect("run");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("allow:"), "got:\n{stderr}");
}

// ---------------------------------------------------------------------------
// profile subcommand: config layering (no claude calls)
// ---------------------------------------------------------------------------

/// Seed a tempdir with the given relative files. Convenient for
/// fixtures that need a `.git` boundary plus one or more
/// `roba.toml` files at different depths.
fn make_dir_with_files(files: &[(&str, &str)]) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().expect("tempdir");
    for (relpath, content) in files {
        let p = tmp.path().join(relpath);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).expect("mkdir");
        }
        std::fs::write(&p, content).expect("write file");
    }
    tmp
}

#[test]
fn cli_profile_path_lists_walkup_chain() {
    let project = make_dir_with_files(&[
        (".git/HEAD", ""),
        ("roba.toml", "[profile.outer]\n"),
        ("a/b/roba.toml", "[profile.inner]\n"),
    ]);
    let user_home = tempfile::tempdir().expect("user home");
    let nested = project.path().join("a/b");

    let out = roba()
        .args(["-C", nested.to_str().unwrap(), "profile", "path"])
        .env("XDG_CONFIG_HOME", user_home.path())
        .output()
        .expect("run");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    let pathlines: Vec<&str> = stdout.lines().filter(|l| l.contains("roba.toml")).collect();
    assert!(
        pathlines.len() >= 2,
        "expected >= 2 roba.toml entries (project root + nested), got:\n{stdout}"
    );
}

#[test]
fn cli_profile_active_default_auto_applies() {
    let project = make_dir_with_files(&[
        (".git/HEAD", ""),
        ("roba.toml", "[profile.default]\nreadonly = true\n"),
    ]);
    let user_home = tempfile::tempdir().expect("user home");

    let out = roba()
        .args(["-C", project.path().to_str().unwrap(), "profile", "active"])
        .env("XDG_CONFIG_HOME", user_home.path())
        .env_remove("ROBA_PROFILE")
        .output()
        .expect("run");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("active: default"),
        "expected active default, got:\n{stdout}"
    );
    assert!(
        stdout.contains("auto-applied"),
        "expected auto-applied note, got:\n{stdout}"
    );
}

#[test]
fn cli_profile_active_env_override() {
    let project = make_dir_with_files(&[
        (".git/HEAD", ""),
        ("roba.toml", "[profile.foo]\nwritable = true\n"),
    ]);
    let user_home = tempfile::tempdir().expect("user home");

    let out = roba()
        .args(["-C", project.path().to_str().unwrap(), "profile", "active"])
        .env("XDG_CONFIG_HOME", user_home.path())
        .env("ROBA_PROFILE", "foo")
        .output()
        .expect("run");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("active: foo"),
        "expected active foo, got:\n{stdout}"
    );
    assert!(
        stdout.contains("from ROBA_PROFILE env"),
        "expected env-source note, got:\n{stdout}"
    );
}

#[test]
fn cli_profile_show_merges_walkup() {
    let project = make_dir_with_files(&[
        (".git/HEAD", ""),
        (
            "roba.toml",
            "[profile.review]\nreadonly = true\nprepend = [\"/parent.md\"]\n",
        ),
        (
            "sub/roba.toml",
            "[profile.review]\ngit_diff = true\nprepend = [\"/child.md\"]\n",
        ),
    ]);
    let user_home = tempfile::tempdir().expect("user home");
    let sub = project.path().join("sub");

    let out = roba()
        .args(["-C", sub.to_str().unwrap(), "profile", "show", "review"])
        .env("XDG_CONFIG_HOME", user_home.path())
        .output()
        .expect("run");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("readonly = true"),
        "expected parent's readonly: {stdout}"
    );
    assert!(
        stdout.contains("git_diff = true"),
        "expected child's git_diff: {stdout}"
    );
    assert!(
        stdout.contains("/parent.md"),
        "expected parent's prepend: {stdout}"
    );
    assert!(
        stdout.contains("/child.md"),
        "expected child's prepend: {stdout}"
    );
}

#[test]
fn conflict_readonly_and_full_auto() {
    assert_conflict(&["foo", "--readonly", "--full-auto"]);
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
fn conflict_stream_and_out() {
    assert_conflict(&["foo", "--stream", "--out", "/tmp/a"]);
}

#[test]
fn help_mentions_show_thinking_flag() {
    roba()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("--show-thinking"));
}

#[test]
fn show_thinking_parses_with_stream() {
    // --show-thinking + --stream + missing prepend file: parse must
    // succeed, failure comes from the runtime not from clap.
    roba()
        .args([
            "foo",
            "--show-thinking",
            "--stream",
            "--prepend",
            "/no/such/show-thinking-test",
        ])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("reading --prepend"));
}

#[test]
fn fork_without_resume_errors() {
    roba()
        .args(["foo", "--fork"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("--resume"));
}

#[test]
fn editor_without_tty_fails_fast() {
    // assert_cmd's write_stdin attaches a pipe to stdin, so stdin
    // is not a TTY. -e must error early with the canonical message.
    roba()
        .arg("-e")
        .write_stdin("")
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains(
            "--editor requires an interactive terminal (stdin not a TTY)",
        ));
}

#[test]
fn pick_without_tty_fails_fast() {
    roba()
        .args(["foo", "--pick"])
        .write_stdin("")
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains(
            "--pick requires an interactive terminal (stdin not a TTY)",
        ));
}

#[test]
fn var_bad_syntax_errors() {
    roba()
        .args(["foo", "--var", "no-equals"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("expected K=V"));
}

// ---------------------------------------------------------------------------
// --json error envelope: structured stderr instead of plain anyhow text
// ---------------------------------------------------------------------------

#[test]
fn json_error_envelope_on_empty_stdin() {
    // Trigger a known exit-1, non-wrapper error path (empty stdin
    // via `-`) with --json and confirm the stderr parses as the
    // documented envelope shape.
    let out = roba()
        .args(["--json", "-"])
        .write_stdin("")
        .output()
        .expect("run");
    assert!(!out.status.success(), "expected failure");
    assert_eq!(out.status.code(), Some(1));

    let stderr = String::from_utf8_lossy(&out.stderr);
    let value: serde_json::Value = serde_json::from_str(&stderr).unwrap_or_else(|e| {
        panic!("--json error stderr must be parseable JSON; got:\n{stderr}\nerror: {e}")
    });
    assert_eq!(
        value["version"], 1,
        "versioned envelope must carry top-level version, got: {stderr}"
    );
    assert_eq!(value["error"]["kind"], "other");
    assert_eq!(value["error"]["exit_code"], 1);
    assert!(
        value["error"]["chain"].is_array(),
        "chain must be an array, got: {stderr}"
    );
    assert!(
        value["error"]["message"]
            .as_str()
            .is_some_and(|m| !m.is_empty()),
        "message must be non-empty, got: {stderr}"
    );
}

#[test]
fn plain_error_path_unchanged_without_json() {
    // Without --json, the existing styled "error: empty stdin..."
    // message must still be present and stderr must NOT be JSON.
    let out = roba().arg("-").write_stdin("").output().expect("run");
    assert!(!out.status.success(), "expected failure");
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("empty stdin"),
        "expected plain error message, got:\n{stderr}"
    );
    assert!(
        serde_json::from_str::<serde_json::Value>(&stderr).is_err(),
        "plain stderr should not be JSON, got:\n{stderr}"
    );
}
