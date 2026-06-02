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
fn conflict_pick_and_continue() {
    assert_conflict(&["foo", "--pick", "-c"]);
}

#[test]
fn conflict_fresh_and_continue() {
    assert_conflict(&["foo", "--fresh", "-c"]);
}

#[test]
fn conflict_fresh_and_pick() {
    assert_conflict(&["foo", "--fresh", "--pick"]);
}

#[test]
fn conflict_prompt_flag_and_positional() {
    // -p and the positional prompt are mutually exclusive (clap-level
    // conflicts_with). Supplying both errors at parse time.
    assert_conflict(&["-p", "x", "positional"]);
}

#[test]
fn prompt_flag_parses_and_fails_at_runtime_not_clap() {
    // -p VALUE + missing prepend file: parse must succeed (the explicit
    // prompt flag is accepted), the failure is the runtime read error.
    roba()
        .args(["-p", "hello", "--prepend", "/no/such/prompt-flag-test"])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("reading --prepend"));
}

#[test]
fn continue_bare_parses() {
    // Bare `-c` followed by a flag (`--prepend`) stays the presence
    // form (Some(None)) -- clap does not consume a flag token as the
    // optional id value. `foo` is the positional prompt. It parses; the
    // failure here is the missing prepend file, proving parse succeeded.
    roba()
        .args(["foo", "-c", "--prepend", "/no/such/continue-bare-test"])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("reading --prepend"));
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

// ---------------------------------------------------------------------------
// --agent
// ---------------------------------------------------------------------------

#[test]
fn help_mentions_agent_flag() {
    roba()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("--agent"));
}

#[test]
fn agent_with_name_parses() {
    // --agent NAME + missing prepend file: parse must succeed, the
    // failure comes from the runtime read error (not clap).
    roba()
        .args([
            "foo",
            "--agent",
            "reviewer",
            "--prepend",
            "/no/such/agent-test-name",
        ])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("reading --prepend"));
}

#[test]
fn worktree_long_space_name_attaches_value() {
    // BREAKING (pre-0.1.0): with require_equals dropped, `--worktree
    // NAME` (space form) now attaches NAME to the flag. Here `foo` is
    // the positional prompt, `mybranch` is the worktree name, and the
    // missing prepend file proves the parse succeeded before the
    // runtime read error.
    roba()
        .args([
            "foo",
            "--worktree",
            "mybranch",
            "--prepend",
            "/no/such/worktree-space-name",
        ])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("reading --prepend"));
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
fn fork_alone_errors() {
    // --fork without any -c: clap's `requires = "continue_session"`
    // rejects it at parse time.
    roba()
        .args(["foo", "--fork"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("continue"));
}

#[test]
fn fork_with_bare_continue_errors_at_runtime() {
    // --fork with bare -c (no id): clap is satisfied (the flag is
    // present), but the runtime check rejects it -- you can't fork
    // "the most recent" without naming a specific session.
    roba()
        .args(["foo", "-c", "--fork"])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains(
            "--fork requires an explicit session id",
        ));
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

// ---------------------------------------------------------------------------
// skill / agent library subcommands (#85)
// ---------------------------------------------------------------------------

#[test]
fn skill_list_outputs_known_skills() {
    roba()
        .args(["skill", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("draft-pr-first"))
        .stdout(predicate::str::contains("roba-orchestration-prompt"))
        .stdout(predicate::str::contains("heredoc-backticks"));
}

#[test]
fn skill_list_excludes_top_level_readme() {
    // The repo-level skills/README.md is documentation, not a skill;
    // it must not appear as a list row.
    let out = roba().args(["skill", "list"]).output().expect("run");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.lines().any(|l| l.starts_with("README")),
        "README should not be a listed skill, got:\n{stdout}"
    );
}

#[test]
fn skill_install_dry_run_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("skills");
    roba()
        .args(["skill", "install", "--dry-run", "--to"])
        .arg(&dest)
        .assert()
        .success()
        .stderr(predicate::str::contains("would write"))
        .stderr(predicate::str::contains("dry run"));
    assert!(
        !dest.exists(),
        "dry-run must not create the destination tree"
    );
}

#[test]
fn skill_install_writes_expected_tree() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("skills");
    roba()
        .args(["skill", "install", "--to"])
        .arg(&dest)
        .assert()
        .success()
        .stderr(predicate::str::contains("installed"));
    assert!(
        dest.join("draft-pr-first/SKILL.md").is_file(),
        "expected draft-pr-first/SKILL.md installed"
    );
    // Top-level README is not installed.
    assert!(
        !dest.join("README.md").exists(),
        "repo README.md must not be installed"
    );
}

#[test]
fn skill_install_force_overwrites() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("skills");
    let target = dest.join("draft-pr-first/SKILL.md");
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
    std::fs::write(&target, "STALE").unwrap();
    roba()
        .args(["skill", "install", "--force", "--to"])
        .arg(&dest)
        .assert()
        .success();
    let body = std::fs::read_to_string(&target).unwrap();
    assert_ne!(body, "STALE", "--force should overwrite the seeded file");
    assert!(body.contains("draft-pr-first"));
}

#[test]
fn skill_install_skip_leaves_existing() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("skills");
    let target = dest.join("draft-pr-first/SKILL.md");
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
    std::fs::write(&target, "SEEDED").unwrap();
    roba()
        .args(["skill", "install", "--skip", "--to"])
        .arg(&dest)
        .assert()
        .success()
        .stderr(predicate::str::contains("skipped"));
    let body = std::fs::read_to_string(&target).unwrap();
    assert_eq!(body, "SEEDED", "--skip must leave the existing file intact");
}

#[test]
fn skill_show_prints_body() {
    roba()
        .args(["skill", "show", "draft-pr-first"])
        .assert()
        .success()
        .stdout(predicate::str::contains("# Draft PR first"))
        .stdout(predicate::str::contains("name: draft-pr-first"));
}

#[test]
fn skill_show_unknown_errors() {
    roba()
        .args(["skill", "show", "no-such-skill"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("no bundled skill named"));
}

#[test]
fn agent_list_outputs_known_agents() {
    roba()
        .args(["agent", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("roba-runner"))
        .stdout(predicate::str::contains("roba-orchestrator"));
}

#[test]
fn agent_install_writes_expected_tree() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("agents");
    roba()
        .args(["agent", "install", "--to"])
        .arg(&dest)
        .assert()
        .success()
        .stderr(predicate::str::contains("installed"));
    assert!(
        dest.join("roba-runner/AGENT.md").is_file(),
        "expected roba-runner/AGENT.md installed"
    );
    assert!(
        !dest.join("README.md").exists(),
        "repo README.md must not be installed"
    );
}

#[test]
fn agent_show_prints_body() {
    roba()
        .args(["agent", "show", "roba-runner"])
        .assert()
        .success()
        .stdout(predicate::str::contains("name: roba-runner"));
}

#[test]
fn agent_show_unknown_errors() {
    roba()
        .args(["agent", "show", "no-such-agent"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("no bundled agent named"));
}

// --url / --urls doc addressability (#86 Phase 2)

#[test]
fn skill_show_url_prints_urls() {
    roba()
        .args(["skill", "show", "draft-pr-first", "--url"])
        .assert()
        .success()
        // Rendered + raw URLs, body suppressed.
        .stdout(predicate::str::contains(
            "https://joshrotenberg.github.io/roba/skills/draft-pr-first.html",
        ))
        .stdout(predicate::str::contains(
            "https://raw.githubusercontent.com/joshrotenberg/roba/main/skills/draft-pr-first/SKILL.md",
        ))
        .stdout(predicate::str::contains("# Draft PR first").not());
}

#[test]
fn skill_list_urls_includes_url_columns() {
    roba()
        .args(["skill", "list", "--urls"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "https://joshrotenberg.github.io/roba/skills/draft-pr-first.html",
        ))
        .stdout(predicate::str::contains(
            "https://raw.githubusercontent.com/joshrotenberg/roba/main/skills/draft-pr-first/SKILL.md",
        ));
}

#[test]
fn agent_show_url_prints_urls() {
    roba()
        .args(["agent", "show", "roba-runner", "--url"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "https://joshrotenberg.github.io/roba/agents/roba-runner.html",
        ))
        .stdout(predicate::str::contains(
            "https://raw.githubusercontent.com/joshrotenberg/roba/main/agents/roba-runner/AGENT.md",
        ));
}

#[test]
fn agent_list_urls_includes_url_columns() {
    roba()
        .args(["agent", "list", "--urls"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "https://joshrotenberg.github.io/roba/agents/roba-runner.html",
        ))
        .stdout(predicate::str::contains(
            "https://raw.githubusercontent.com/joshrotenberg/roba/main/agents/roba-runner/AGENT.md",
        ));
}

// ---------------------------------------------------------------------------
// user-defined aliases (#88)
// ---------------------------------------------------------------------------

/// A project dir with a `.git` boundary and a `roba.toml` defining a
/// handful of aliases for the dispatch / management tests.
fn alias_project() -> tempfile::TempDir {
    make_dir_with_files(&[
        (".git/HEAD", ""),
        (
            "roba.toml",
            r#"
[alias.review]
description = "Review a PR by number"
agent = "reviewer"
template = "PR #${pr}"
flags = ["--prepend", "/no/such/alias-prepend-xyz"]
args = ["pr"]

[alias.perms]
description = "permission preset"
flags = ["--show-permissions"]

[alias.boom]
template = "$(exit 9)"
"#,
        ),
    ])
}

/// Run roba scoped to `project` with an isolated (empty) user config.
fn roba_in(project: &tempfile::TempDir, user_home: &tempfile::TempDir) -> Command {
    let mut cmd = roba();
    cmd.arg("-C")
        .arg(project.path())
        .env("XDG_CONFIG_HOME", user_home.path())
        .env_remove("ROBA_PROFILE");
    cmd
}

#[test]
fn alias_list_outputs_known_aliases() {
    let project = alias_project();
    let home = tempfile::tempdir().unwrap();
    roba_in(&project, &home)
        .args(["alias", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("review"))
        .stdout(predicate::str::contains("Review a PR by number"))
        .stdout(predicate::str::contains("reviewer"));
}

#[test]
fn alias_show_prints_definition_and_preview() {
    let project = alias_project();
    let home = tempfile::tempdir().unwrap();
    roba_in(&project, &home)
        .args(["alias", "show", "review"])
        .assert()
        .success()
        .stdout(predicate::str::contains("[alias.review]"))
        .stdout(predicate::str::contains("PR #${pr}"))
        .stdout(predicate::str::contains("--prepend"))
        .stdout(predicate::str::contains("expansion preview"))
        .stdout(predicate::str::contains("PR #<pr>"));
}

#[test]
fn alias_show_unknown_errors_with_suggestion() {
    let project = alias_project();
    let home = tempfile::tempdir().unwrap();
    roba_in(&project, &home)
        .args(["alias", "show", "reviewz"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "no built-in or alias named `reviewz`",
        ))
        .stderr(predicate::str::contains("review"));
}

#[test]
fn alias_path_lists_contributing_files() {
    let project = alias_project();
    let home = tempfile::tempdir().unwrap();
    roba_in(&project, &home)
        .args(["alias", "path"])
        .assert()
        .success()
        .stdout(predicate::str::contains("roba.toml"))
        .stdout(predicate::str::contains("alias(es) defined"));
}

#[test]
fn alias_dispatch_expands_and_reaches_run_ask() {
    // `roba review 42` resolves the alias, merges its flags (a bad
    // --prepend), and reaches run_ask -- which fails reading the
    // prepend file. The read error (not an unknown-alias error) proves
    // the alias expanded and dispatched.
    let project = alias_project();
    let home = tempfile::tempdir().unwrap();
    roba_in(&project, &home)
        .args(["review", "42"])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("reading --prepend"));
}

#[test]
fn alias_dispatch_merges_alias_and_cli_flags() {
    // The `perms` alias carries --show-permissions; the user adds
    // --writable. Both must land: exit 0 (show-permissions short-
    // circuits before any claude call) and the preview shows the
    // user's writable opt-in.
    let project = alias_project();
    let home = tempfile::tempdir().unwrap();
    let out = roba_in(&project, &home)
        .args(["perms", "--writable"])
        .output()
        .expect("run");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("allow:"), "got:\n{stderr}");
    assert!(stderr.contains("Edit"), "got:\n{stderr}");
    assert!(stderr.contains("Write"), "got:\n{stderr}");
}

#[test]
fn alias_dispatch_runs_shell_substitution() {
    // The `boom` alias template is `$(exit 9)`; the failing shell
    // substitution surfaces during expansion, before any claude call.
    let project = alias_project();
    let home = tempfile::tempdir().unwrap();
    roba_in(&project, &home)
        .args(["boom"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("shell substitution"));
}

#[test]
fn alias_dispatch_unknown_multiword_errors_clearly() {
    let project = alias_project();
    let home = tempfile::tempdir().unwrap();
    roba_in(&project, &home)
        .args(["reviewz", "42"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "no built-in or alias named `reviewz`",
        ));
}

#[test]
fn alias_shadowing_builtin_warns() {
    let project = make_dir_with_files(&[
        (".git/HEAD", ""),
        (
            "roba.toml",
            "[alias.cost]\ndescription = \"shadow\"\ntemplate = \"x\"\n",
        ),
    ]);
    let home = tempfile::tempdir().unwrap();
    roba_in(&project, &home)
        .args(["alias", "list"])
        .assert()
        .success()
        .stderr(predicate::str::contains(
            "alias `cost` is shadowed by the built-in",
        ));
}

#[test]
fn help_mentions_alias_subcommand() {
    roba()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("alias"));
}

// ---------------------------------------------------------------------------
// cost -- dollar reporting (no claude call; reads seeded session JSONL)
// ---------------------------------------------------------------------------

/// Seed a fake `$HOME/.claude/projects/<slug>/<id>.jsonl` with one
/// user + one assistant entry. The assistant carries `model` + `usage`
/// so the cost rollup can compute dollars. Returns the home tempdir.
fn home_with_session(model: &str, input: u64, output: u64) -> tempfile::TempDir {
    let home = tempfile::tempdir().expect("home");
    let proj = home.path().join(".claude/projects/-tmp-proj");
    std::fs::create_dir_all(&proj).expect("mkdir projects");
    let user =
        r#"{"type":"user","timestamp":"2026-06-01T10:00:00.000Z","message":{"content":"hi"}}"#;
    let assistant = format!(
        r#"{{"type":"assistant","timestamp":"2026-06-01T10:00:01.000Z","message":{{"model":"{model}","usage":{{"input_tokens":{input},"output_tokens":{output},"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}}}}"#
    );
    std::fs::write(proj.join("sess-1.jsonl"), format!("{user}\n{assistant}\n"))
        .expect("write session");
    home
}

#[test]
fn cost_dollars_default_shows_dollar_column() {
    // 1M sonnet-4-6 input @ $3/MTok = $3.00.
    let home = home_with_session("claude-sonnet-4-6", 1_000_000, 0);
    roba()
        .arg("cost")
        .env("HOME", home.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("cost:").and(predicate::str::contains("$3.00")));
}

#[test]
fn cost_no_dollars_omits_dollar_column() {
    let home = home_with_session("claude-sonnet-4-6", 1_000_000, 0);
    roba()
        .args(["cost", "--no-dollars"])
        .env("HOME", home.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("tokens").and(predicate::str::contains("$").not()));
}

#[test]
fn cost_rates_file_uses_override() {
    // Override sonnet input to $100/MTok -> 1M input = $100.00.
    let home = home_with_session("claude-sonnet-4-6", 1_000_000, 0);
    let rates = make_dir_with_files(&[(
        "rates.toml",
        "[meta]\nas_of = \"2026-01-01\"\nsource = \"test\"\n\n[models.\"claude-sonnet-4-6\"]\ninput = 100.0\noutput = 200.0\ncache_read = 1.0\ncache_write = 1.0\n",
    )]);
    let rates_path = rates.path().join("rates.toml");
    roba()
        .args(["cost", "--rates-file", rates_path.to_str().unwrap()])
        .env("HOME", home.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("$100.00"));
}

#[test]
fn cost_by_project_shows_cost_column() {
    let home = home_with_session("claude-sonnet-4-6", 1_000_000, 0);
    roba()
        .args(["cost", "--by-project"])
        .env("HOME", home.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("COST").and(predicate::str::contains("$3.00")));
}
