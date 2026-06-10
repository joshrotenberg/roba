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
fn help_long_trailer_is_byte_clean_off_tty() {
    // The `--help` long trailer is styled (green-bold headers, cyan command
    // columns), but the styling MUST route through clap's color pipeline so
    // it strips on a non-TTY. assert_cmd runs the binary off a TTY, so the
    // captured stdout must carry NO ANSI escape -- the agent ABI stays
    // byte-clean (the #181 discipline). A regression here would leak ANSI
    // into a pipe.
    let assert = roba().arg("--help").assert().success();
    let stdout =
        String::from_utf8(assert.get_output().stdout.clone()).expect("help output is valid UTF-8");
    assert!(
        !stdout.contains('\u{1b}'),
        "--help stdout leaked an ANSI escape off-TTY: {stdout:?}"
    );
    // Sanity: the styled sections are still present as plain text.
    assert!(stdout.contains("Examples -- for humans"));
    assert!(stdout.contains("Examples -- for agents & scripts"));
    assert!(stdout.contains("Unattended workers"));
    assert!(stdout.contains("Environment variables:"));
    assert!(stdout.contains("Configuration (roba.toml):"));
}

#[test]
fn version_prints_crate_version_and_exits_zero() {
    roba()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains(concat!(
            "roba ",
            env!("CARGO_PKG_VERSION")
        )));
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

#[test]
fn history_paths_flag_no_arg() {
    // --paths with no value should parse and run without panicking.
    // No real sessions may exist in CI; exit 0 is the contract.
    roba().args(["history", "--paths"]).assert().success();
}

#[test]
fn history_paths_flag_with_n() {
    // --paths 5 should parse correctly and run without panicking.
    roba().args(["history", "--paths", "5"]).assert().success();
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
fn missing_claude_prints_install_hint_on_stderr() {
    // Clear PATH so claude-wrapper can't find the `claude` binary,
    // driving the real Error::NotFound path. roba itself is invoked
    // by absolute path (cargo_bin), so it still launches.
    roba()
        .env("PATH", "")
        .arg("hi")
        .assert()
        .failure()
        .stderr(predicate::str::contains("not found on PATH"));
}

#[test]
fn alias_draft_reaches_claude_call() {
    // With PATH cleared, `roba alias draft` wires through to the claude
    // call and fails with the normal claude-missing error -- proving the
    // verb dispatches without needing the API.
    roba()
        .env("PATH", "")
        .args(["alias", "draft", "a verb that echoes its argument"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not found on PATH"));
}

#[test]
fn profile_draft_reaches_claude_call() {
    // With PATH cleared, `roba profile draft` wires through to the claude
    // call and fails with the normal claude-missing error -- proving the
    // verb dispatches without needing the API.
    roba()
        .env("PATH", "")
        .args(["profile", "draft", "a long-running worker with spend rails"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not found on PATH"));
}

#[test]
fn config_init_reaches_claude_call() {
    // With PATH cleared, `roba config init` wires through to the claude
    // call and fails with the normal claude-missing error -- proving the
    // verb dispatches without needing the API (no file is written, since
    // the failure happens before any output).
    roba()
        .env("PATH", "")
        .args(["config", "init"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not found on PATH"));
}

#[test]
fn config_init_write_refuses_existing_target_before_claude_call() {
    // The fail-fast clobber guard: `config init --write` into a dir that
    // already has a roba.toml refuses BEFORE any claude call. PATH is
    // cleared, so if the guard didn't fire first we'd see the
    // claude-missing error instead of the clobber message.
    let tmp = tempfile::tempdir().expect("tempdir");
    std::fs::write(tmp.path().join("roba.toml"), "readonly = true\n").expect("seed roba.toml");
    roba()
        .env("PATH", "")
        .args([
            "-C",
            tmp.path().to_str().unwrap(),
            "config",
            "init",
            "--write",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("already exists"))
        .stderr(predicate::str::contains("not found on PATH").not());
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

#[test]
fn no_args_empty_stdin_non_tty_still_errors() {
    // assert_cmd attaches a pipe (non-TTY) to stdin, so this exercises
    // the UNCHANGED path: no positional + non-TTY stdin routes through
    // `read_stdin`, which bails on empty input with a non-zero exit.
    //
    // The promptless-on-a-TTY guard in `run_ask` (the abbreviated help
    // blurb that returns exit 0) is TTY-only and gated on
    // `std::io::stdin().is_terminal()`. assert_cmd's stdin is never a
    // TTY, so that branch is not mechanically testable here -- the blurb
    // content is covered by the `no_prompt_blurb()` unit test in
    // `src/prompt.rs`.
    roba()
        .write_stdin("")
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("empty stdin"));
}

#[test]
fn piped_stdin_with_positional_composes_and_reaches_claude() {
    // Real piped content + a positional prompt: the stdin is merged as a
    // context block (it is no longer silently dropped), composition
    // succeeds, and the run proceeds to the claude call -- which fails
    // only because PATH is cleared. Reaching the claude-missing error
    // (not an earlier bail) proves the merge path composed cleanly.
    roba()
        .env("PATH", "")
        .arg("what's wrong here?")
        .write_stdin("ERROR: boom\n")
        .assert()
        .failure()
        .stderr(predicate::str::contains("not found on PATH"));
}

#[test]
fn empty_piped_stdin_with_positional_still_composes() {
    // Rule: empty piped stdin + a positional prompt is byte-identical to
    // no pipe -- no context part, composition is just the positional, and
    // the run reaches the claude call (failing only on the cleared PATH).
    roba()
        .env("PATH", "")
        .arg("hi")
        .write_stdin("")
        .assert()
        .failure()
        .stderr(predicate::str::contains("not found on PATH"));
}

// ---------------------------------------------------------------------------
// --detach guards (all fail before any spawn -- claude-free)
// ---------------------------------------------------------------------------
//
// The detach branch runs three guards in order: promptless -> stdin
// data-loss -> claude preflight. A promptless call hits the "needs a prompt"
// guard; a call with real piped DATA hits the "can't read piped stdin" guard;
// a benign non-TTY stdin (a closed/empty pipe, as an orchestrator supplies)
// passes the data-loss gate and reaches the claude preflight. Every failure
// here must leave stdout EMPTY -- that is the proof nothing was spawned.

#[test]
fn detach_promptless_errors_without_spawning() {
    // No prompt source at all: the detached child could never resolve a
    // prompt, so roba refuses up front. No handle on stdout.
    roba()
        .arg("--detach")
        .assert()
        .failure()
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains("explicit prompt"));
}

#[test]
#[cfg(unix)] // the piped-data stdin gate is unix-only by design;
// windows proceeds to the preflight (documented in src/detach.rs)
fn detach_piped_stdin_errors_without_spawning() {
    // A prompt is present, but stdin is a (non-TTY) pipe -- the detached
    // child's stdin is /dev/null, so the piped input would vanish. roba
    // rejects it and prints NO handle (proving no spawn happened).
    roba()
        .arg("--detach")
        .arg("say ok")
        .write_stdin("piped context that would be lost\n")
        .assert()
        .failure()
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains("piped stdin"));
}

#[test]
fn detach_benign_nontty_stdin_passes_gate() {
    // A benign non-TTY stdin (here an empty/closed pipe, the shape an
    // orchestrator firing `roba --detach -f task.md` supplies) carries no
    // data, so the data-loss gate lets it through. With PATH cleared, the run
    // proceeds PAST the stdin gate to the claude preflight and fails there --
    // proving the gate no longer blocks non-TTY callers. The failure is the
    // claude-missing error, NOT the stdin error, and stdout stays empty
    // (no handle, no spawn).
    //
    // (A true `< file` redirect is not cleanly expressible via assert_cmd's
    // pipe-based stdin; the regular-file empty/non-empty classification is
    // covered by the `data_loss` unit tests in src/detach.rs.)
    roba()
        .env("PATH", "")
        .arg("--detach")
        .arg("-f")
        .arg("Cargo.toml")
        .write_stdin("")
        .assert()
        .failure()
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains("not found on PATH"))
        .stderr(predicate::str::contains("piped stdin").not());
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
fn conflict_session_id_and_continue() {
    // --session-id assigns a NEW session's id; -c=ID resumes an
    // existing one. clap rejects the combination at parse time.
    assert_conflict(&["foo", "--session-id", "x", "-c=y"]);
}

#[test]
fn conflict_session_id_and_pick() {
    assert_conflict(&["foo", "--session-id", "x", "--pick"]);
}

#[test]
fn conflict_session_id_and_session() {
    assert_conflict(&["foo", "--session-id", "x", "--session", "meta"]);
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
fn json_schema_missing_file_errors_cleanly() {
    // --json-schema PATH that doesn't exist: parse succeeds (it's a plain
    // string flag), the failure is roba's runtime file-read error. Clean
    // non-zero exit, no panic.
    roba()
        .args(["--json-schema", "/no/such/schema-file.json", "hi"])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("reading --json-schema"));
}

#[test]
fn json_schema_malformed_json_errors_cleanly() {
    // A real file whose contents are not valid JSON fails through roba's
    // error path (not a panic, not an opaque claude error).
    let tmp = tempfile::tempdir().expect("tempdir");
    let schema = tmp.path().join("bad.json");
    std::fs::write(&schema, "{ this is not json ").expect("write schema");
    roba()
        .args(["--json-schema"])
        .arg(&schema)
        .arg("hi")
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("not valid JSON"));
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
// roba worktree list (shells to git, not claude)
// ---------------------------------------------------------------------------

#[test]
fn worktree_help_says_git_worktrees_not_claude() {
    // The help text must be accurate: the output is a superset of
    // claude's worktrees, so it describes "git worktrees", not
    // "claude's worktrees".
    roba()
        .args(["worktree", "list", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("git worktree"));
}

#[test]
fn worktree_list_json_lists_main_and_added() {
    use std::process::Command as Git;

    let tmp = tempfile::tempdir().expect("tempdir");
    let repo = tmp.path().join("repo");
    std::fs::create_dir(&repo).unwrap();

    let git = |args: &[&str]| {
        let ok = Git::new("git")
            .current_dir(&repo)
            .args(args)
            .status()
            .expect("run git")
            .success();
        assert!(ok, "git {args:?} failed");
    };
    git(&["init", "-q"]);
    git(&["config", "user.email", "test@example.com"]);
    git(&["config", "user.name", "Test"]);
    std::fs::write(repo.join("f.txt"), "hi").unwrap();
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "init"]);

    // Add a second worktree on a new branch.
    let wt2 = tmp.path().join("wt2");
    git(&[
        "worktree",
        "add",
        "-q",
        wt2.to_str().unwrap(),
        "-b",
        "feature",
    ]);

    // --json: the uniform { version: 1, result: [...] } envelope, where
    // result is the array of both worktrees.
    let assert = roba()
        .args(["worktree", "list", "--json", "-C", repo.to_str().unwrap()])
        .assert()
        .success();
    let out = &assert.get_output().stdout;
    let json: serde_json::Value = serde_json::from_slice(out).expect("valid JSON");
    assert_eq!(json["version"], 1, "top-level version must be 1");
    let arr = json["result"].as_array().expect("result is a JSON array");
    assert_eq!(arr.len(), 2, "expected main + added worktree, got: {json}");

    // The plain (human) form runs and exits 0.
    roba()
        .args(["worktree", "list", "-C", repo.to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("PATH"));
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

// --permission-mode conflict/parse tests
// ---------------------------------------------------------------------------

#[test]
fn permission_mode_plan_parses() {
    // --permission-mode plan must parse at the clap layer and fail later
    // (no claude call), not at the parse layer.
    roba()
        .args([
            "foo",
            "--permission-mode",
            "plan",
            "--prepend",
            "/no/such/file-pm",
        ])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("reading --prepend"));
}

#[test]
fn permission_mode_dont_ask_parses() {
    roba()
        .args([
            "foo",
            "--permission-mode",
            "dont-ask",
            "--prepend",
            "/no/such/file-pm",
        ])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("reading --prepend"));
}

#[test]
fn permission_mode_auto_parses() {
    roba()
        .args([
            "foo",
            "--permission-mode",
            "auto",
            "--prepend",
            "/no/such/file-pm",
        ])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("reading --prepend"));
}

#[test]
fn permission_mode_coexists_with_readonly() {
    // --permission-mode and --readonly operate at different levels:
    // --permission-mode passes --permission-mode to claude directly;
    // --readonly restricts --allowedTools. Both are valid together
    // (e.g. plan mode with the standard read-only allowlist).
    roba()
        .args([
            "foo",
            "--permission-mode",
            "plan",
            "--readonly",
            "--prepend",
            "/no/such/file-pm",
        ])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("reading --prepend"));
}

#[test]
fn permission_mode_coexists_with_writable() {
    // Both can be combined: e.g. write access with plan review step.
    roba()
        .args([
            "foo",
            "--permission-mode",
            "plan",
            "--writable",
            "--prepend",
            "/no/such/file-pm",
        ])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("reading --prepend"));
}

#[test]
fn permission_mode_coexists_with_full_auto() {
    // full-auto bypasses allowedTools; --permission-mode sets the mode.
    roba()
        .args([
            "foo",
            "--permission-mode",
            "dont-ask",
            "--full-auto",
            "--prepend",
            "/no/such/file-pm",
        ])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("reading --prepend"));
}

#[test]
fn permission_mode_invalid_value_errors() {
    // An unrecognized mode value should produce a clap error.
    roba()
        .args(["foo", "--permission-mode", "totally-wrong"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("totally-wrong").or(predicate::str::contains("invalid")));
}

#[test]
fn show_permissions_with_permission_mode_plan() {
    // --permission-mode plan + --show-permissions should show the mode
    // in the stderr output and exit 0 without calling claude.
    let project = empty_project();
    let user_home = tempfile::tempdir().expect("user home");
    let out = roba()
        .args([
            "-C",
            project.path().to_str().unwrap(),
            "--permission-mode",
            "plan",
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
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("plan"),
        "expected 'plan' in show-permissions output, got:\n{stderr}"
    );
    assert!(
        stderr.contains("[CLI]"),
        "expected '[CLI]' provenance tag, got:\n{stderr}"
    );
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
fn session_id_parses_with_show_permissions() {
    // --session-id parses cleanly alongside a real prompt. Pair with
    // --show-permissions so the run short-circuits before any claude
    // call: a clean exit proves the flag parses without conflict.
    roba()
        .args([
            "foo",
            "--session-id",
            "11111111-1111-4111-8111-111111111111",
            "--show-permissions",
        ])
        .assert()
        .success();
}

#[test]
fn limits_flags_parse_and_accept() {
    // --max-turns + --max-budget-usd parse cleanly alongside a real
    // prompt. Pair with --show-permissions so the run short-circuits
    // before any claude call: a clean exit proves both flags parse and
    // compose without conflict.
    roba()
        .args([
            "foo",
            "--max-turns",
            "5",
            "--max-budget-usd",
            "10.0",
            "--show-permissions",
        ])
        .assert()
        .success();
}

#[test]
fn mcp_config_flags_parse_and_accept() {
    // --mcp-config (repeatable) + --strict-mcp-config parse cleanly
    // alongside a real prompt. Pair with --show-permissions so the run
    // short-circuits before any claude call: a clean exit proves both
    // flags parse and compose without conflict. roba forwards the paths
    // verbatim and never reads them, so non-existent files are fine here.
    roba()
        .args([
            "foo",
            "--mcp-config",
            "a.json",
            "--mcp-config",
            "b.json",
            "--strict-mcp-config",
            "--show-permissions",
        ])
        .assert()
        .success();
}

#[test]
fn medtier_flags_parse_and_accept() {
    // --add-dir (repeatable) + --fallback-model + --no-session-persistence
    // parse cleanly alongside a real prompt. Pair with --show-permissions so
    // the run short-circuits before any claude call: a clean exit proves all
    // three flags parse and compose without conflict. roba forwards add_dir
    // paths verbatim and never reads them, so non-existent dirs are fine here.
    roba()
        .args([
            "foo",
            "--add-dir",
            "/extra/a",
            "--add-dir",
            "/extra/b",
            "--fallback-model",
            "haiku",
            "--no-session-persistence",
            "--show-permissions",
        ])
        .assert()
        .success();
}

#[test]
fn max_turns_rejects_non_numeric_value() {
    roba()
        .args(["foo", "--max-turns", "abc"])
        .assert()
        .failure();
}

#[test]
fn max_budget_usd_rejects_non_numeric_value() {
    roba()
        .args(["foo", "--max-budget-usd", "lots"])
        .assert()
        .failure();
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
fn session_unknown_name_errors_before_claude() {
    // `--session NAME` for an unconfigured NAME must fail loudly (and
    // before any claude call -- the unknown-name bail happens during
    // resolution, so this is safe to assert mechanically). A *known*
    // name would resume a session and invoke claude, so that path is
    // covered by unit tests, not here.
    let project = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let mut cmd = roba();
    cmd.arg("-C")
        .arg(project.path())
        .env("XDG_CONFIG_HOME", home.path())
        .env_remove("ROBA_SESSION")
        .env_remove("ROBA_PROFILE")
        .args(["--session", "nope", "hi"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("no session named"));
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

#[test]
fn cost_json_carries_version_and_result() {
    // The uniform { version: 1, result: <Rollup> } envelope: peel off
    // version + result and the rollup fields sit under result.
    let home = home_with_session("claude-sonnet-4-6", 1_000_000, 0);
    let out = roba()
        .args(["cost", "--json"])
        .env("HOME", home.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let v: serde_json::Value = serde_json::from_slice(&out).expect("stdout is JSON");
    assert_eq!(v["version"], 1, "top-level version must be 1");
    assert!(v.get("result").is_some(), "result must be present");
    assert!(v.get("error").is_none(), "error must be absent on success");
    // Rollup shape lives under result.
    assert_eq!(v["result"]["sessions"], 1);
    assert_eq!(v["result"]["total_tokens"], 1_000_000);
}

// ---------------------------------------------------------------------------
// roba show (read-only result handle, reconstructed envelope) -- refs #220
// ---------------------------------------------------------------------------

/// Seed a `$HOME/.claude/projects/<slug>/<id>.jsonl` with one user + one
/// assistant entry. The assistant carries a text content block (so the
/// answer reconstructs) plus `model` + `usage` (so the cost rollup can
/// compute a figure). Returns `(home, session_id)`.
fn home_with_text_session(answer: &str) -> (tempfile::TempDir, String) {
    let home = tempfile::tempdir().expect("home");
    let proj = home.path().join(".claude/projects/-tmp-proj");
    std::fs::create_dir_all(&proj).expect("mkdir projects");
    let session_id = "show-sess-1";
    let user =
        r#"{"type":"user","timestamp":"2026-06-01T10:00:00.000Z","message":{"content":"hi"}}"#;
    let assistant = format!(
        r#"{{"type":"assistant","timestamp":"2026-06-01T10:00:01.000Z","message":{{"model":"claude-sonnet-4-6","content":[{{"type":"text","text":"{answer}"}}],"usage":{{"input_tokens":1000000,"output_tokens":0,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}}}}"#
    );
    std::fs::write(
        proj.join(format!("{session_id}.jsonl")),
        format!("{user}\n{assistant}\n"),
    )
    .expect("write session");
    (home, session_id.to_string())
}

#[test]
fn show_json_reconstructs_envelope() {
    let (home, id) = home_with_text_session("the reconstructed answer");
    let out = roba()
        .args(["show", &id, "--json"])
        .env("HOME", home.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let v: serde_json::Value = serde_json::from_slice(&out).expect("stdout is JSON");
    assert_eq!(v["version"], 1);
    assert_eq!(v["result"]["result"], "the reconstructed answer");
    assert_eq!(v["result"]["session_id"], "show-sess-1");
    // num_turns is the DERIVED count of assistant entries (one here).
    assert_eq!(v["result"]["num_turns"], 1);
    // duration_ms is always null in the reconstructed envelope.
    assert!(v["result"]["duration_ms"].is_null());
    assert_eq!(v["refusal"], false);
}

#[test]
fn show_not_found_errors_cleanly() {
    let (home, _id) = home_with_text_session("ignored");
    roba()
        .args(["show", "does-not-exist"])
        .env("HOME", home.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("not found"));
}

/// Like `home_with_text_session` but the assistant entry carries a
/// terminal `stop_reason` so `roba show --wait` sees it as complete.
fn home_with_complete_session(answer: &str) -> (tempfile::TempDir, String) {
    let home = tempfile::tempdir().expect("home");
    let proj = home.path().join(".claude/projects/-tmp-proj");
    std::fs::create_dir_all(&proj).expect("mkdir projects");
    let session_id = "show-wait-sess-1";
    let user =
        r#"{"type":"user","timestamp":"2026-06-01T10:00:00.000Z","message":{"content":"hi"}}"#;
    let assistant = format!(
        r#"{{"type":"assistant","timestamp":"2026-06-01T10:00:01.000Z","message":{{"model":"claude-sonnet-4-6","stop_reason":"end_turn","content":[{{"type":"text","text":"{answer}"}}],"usage":{{"input_tokens":1000000,"output_tokens":0,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}}}}"#
    );
    std::fs::write(
        proj.join(format!("{session_id}.jsonl")),
        format!("{user}\n{assistant}\n"),
    )
    .expect("write session");
    (home, session_id.to_string())
}

#[test]
fn show_wait_timeout_errors_cleanly() {
    // A session that never appears must time out cleanly (not panic, not
    // hang). --timeout 1 bounds the wait to ~1-2s.
    let home = tempfile::tempdir().expect("home");
    let start = std::time::Instant::now();
    roba()
        .args(["show", "never-appears", "--wait", "--timeout", "1"])
        .env("HOME", home.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("waited 1s"))
        .stderr(predicate::str::contains("never-appears"));
    assert!(
        start.elapsed() < std::time::Duration::from_secs(5),
        "show --wait --timeout 1 should return within ~1-2s, took {:?}",
        start.elapsed()
    );
}

#[test]
fn show_wait_already_complete_returns_immediately() {
    // A session already complete on disk: --wait short-circuits and
    // renders the result without waiting out the timeout.
    let (home, id) = home_with_complete_session("the waited answer");
    let start = std::time::Instant::now();
    let out = roba()
        .args(["show", &id, "--wait", "--timeout", "5", "--json"])
        .env("HOME", home.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let v: serde_json::Value = serde_json::from_slice(&out).expect("stdout is JSON");
    assert_eq!(v["result"]["result"], "the waited answer");
    assert!(
        start.elapsed() < std::time::Duration::from_secs(3),
        "an already-complete session should not wait, took {:?}",
        start.elapsed()
    );
}

// ---------------------------------------------------------------------------
// roba history --worktree (read-only worktree filter) -- closes #218
// ---------------------------------------------------------------------------

/// Seed two sessions in distinct project dirs: one whose user-entry cwd
/// is inside `.claude/worktrees/foo`, and one in the base repo. The
/// `--worktree` filter discriminates purely on the cwd, regardless of
/// project slug. Returns the home tempdir.
fn home_with_worktree_sessions() -> tempfile::TempDir {
    let home = tempfile::tempdir().expect("home");

    // Session that ran inside .claude/worktrees/foo.
    let wt_proj = home
        .path()
        .join(".claude/projects/-repo--claude-worktrees-foo");
    std::fs::create_dir_all(&wt_proj).expect("mkdir worktree proj");
    let wt_user = r#"{"type":"user","timestamp":"2026-06-01T10:00:00.000Z","cwd":"/repo/.claude/worktrees/foo","message":{"content":"hi"}}"#;
    let wt_assistant = r#"{"type":"assistant","timestamp":"2026-06-01T10:00:01.000Z","message":{"content":[{"type":"text","text":"in worktree"}]}}"#;
    std::fs::write(
        wt_proj.join("wt-sess.jsonl"),
        format!("{wt_user}\n{wt_assistant}\n"),
    )
    .expect("write worktree session");

    // Session that ran in the base repo (no worktree).
    let base_proj = home.path().join(".claude/projects/-repo");
    std::fs::create_dir_all(&base_proj).expect("mkdir base proj");
    let base_user = r#"{"type":"user","timestamp":"2026-06-02T10:00:00.000Z","cwd":"/repo","message":{"content":"hi"}}"#;
    let base_assistant = r#"{"type":"assistant","timestamp":"2026-06-02T10:00:01.000Z","message":{"content":[{"type":"text","text":"in base"}]}}"#;
    std::fs::write(
        base_proj.join("base-sess.jsonl"),
        format!("{base_user}\n{base_assistant}\n"),
    )
    .expect("write base session");

    home
}

#[test]
fn history_worktree_filter_returns_only_matching() {
    let home = home_with_worktree_sessions();
    let output = roba()
        .args(["history", "--worktree", "foo", "--json"])
        .env("HOME", home.path())
        .assert()
        .success()
        // The slug pre-filter keeps the scanned set small, so the
        // "scan capped" note must not fire for a sparse match set.
        .stderr(predicate::str::contains("scanned only").not())
        .get_output()
        .clone();
    let v: serde_json::Value = serde_json::from_slice(&output.stdout).expect("stdout is JSON");
    assert_eq!(v["version"], 1, "top-level version must be 1");
    let arr = v["result"].as_array().expect("result is array of sessions");
    assert_eq!(arr.len(), 1, "only the worktree session should match");
    assert_eq!(arr[0]["session_id"], "wt-sess");
}

#[test]
fn history_worktree_filter_no_match_is_clean_empty() {
    let home = home_with_worktree_sessions();
    let out = roba()
        .args(["history", "--worktree", "nope", "--json"])
        .env("HOME", home.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let v: serde_json::Value = serde_json::from_slice(&out).expect("stdout is JSON");
    assert_eq!(v["version"], 1, "top-level version must be 1");
    assert_eq!(
        v["result"]
            .as_array()
            .expect("result is array of sessions")
            .len(),
        0,
        "no session matches an unknown worktree"
    );
}

#[test]
fn history_json_carries_version_and_result() {
    // The uniform { version: 1, result: [<SessionSummary>] } envelope:
    // the session list (formerly a bare top-level array) now sits under
    // result. --all-projects widens past the cwd-scoped default.
    let home = home_with_worktree_sessions();
    let out = roba()
        .args(["history", "--all-projects", "--json"])
        .env("HOME", home.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let v: serde_json::Value = serde_json::from_slice(&out).expect("stdout is JSON");
    assert_eq!(v["version"], 1, "top-level version must be 1");
    assert!(v.get("error").is_none(), "error must be absent on success");
    let arr = v["result"].as_array().expect("result is array of sessions");
    assert!(
        !arr.is_empty(),
        "seeded sessions must appear under result, got: {v}"
    );
}

// ---------------------------------------------------------------------------
// --no-agent-check / agent frontmatter permission check (#123)
// ---------------------------------------------------------------------------

/// A project dir whose `.claude/agents/test-agent/AGENT.md` declares
/// Bash (which is NOT in roba's default read-only allowlist).
fn project_with_bash_agent() -> tempfile::TempDir {
    make_dir_with_files(&[
        (".git/HEAD", ""),
        (
            ".claude/agents/test-agent/AGENT.md",
            "---\nname: Test Agent\ntools:\n  - Bash\n---\n# body\n",
        ),
    ])
}

#[test]
fn agent_check_warning_on_missing_tool() {
    // The agent declares Bash but the default allowlist only has
    // Read/Glob/Grep. roba should emit a warning to stderr before
    // failing on the (deliberately) missing prepend file.
    let project = project_with_bash_agent();
    let user_home = tempfile::tempdir().expect("user home");

    let out = roba()
        .args([
            "-C",
            project.path().to_str().unwrap(),
            "--agent",
            "test-agent",
            "--prepend",
            "/no/such/agent-check-warn-test",
            "some prompt",
        ])
        .env("HOME", user_home.path())
        .env_remove("ROBA_PROFILE")
        .output()
        .expect("run");

    // The run fails because the prepend file is missing, but the
    // warning must have already been emitted before that.
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("[roba] warning:"),
        "expected agent check warning, got:\n{stderr}"
    );
    assert!(
        stderr.contains("test-agent"),
        "expected agent name in warning, got:\n{stderr}"
    );
    assert!(
        stderr.contains("Bash"),
        "expected missing tool in warning, got:\n{stderr}"
    );
    assert!(
        stderr.contains("reading --prepend"),
        "expected prepend error after the warning, got:\n{stderr}"
    );
}

#[test]
fn agent_check_suppressed_by_no_agent_check_flag() {
    let project = project_with_bash_agent();
    let user_home = tempfile::tempdir().expect("user home");

    let out = roba()
        .args([
            "-C",
            project.path().to_str().unwrap(),
            "--agent",
            "test-agent",
            "--no-agent-check",
            "--prepend",
            "/no/such/agent-check-suppressed-test",
            "some prompt",
        ])
        .env("HOME", user_home.path())
        .env_remove("ROBA_PROFILE")
        .output()
        .expect("run");

    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("[roba] warning:"),
        "expected no warning with --no-agent-check, got:\n{stderr}"
    );
    assert!(
        stderr.contains("reading --prepend"),
        "expected prepend error, got:\n{stderr}"
    );
}

#[test]
fn agent_check_suppressed_by_quiet() {
    let project = project_with_bash_agent();
    let user_home = tempfile::tempdir().expect("user home");

    let out = roba()
        .args([
            "-C",
            project.path().to_str().unwrap(),
            "--agent",
            "test-agent",
            "--quiet",
            "--prepend",
            "/no/such/agent-check-quiet-test",
            "some prompt",
        ])
        .env("HOME", user_home.path())
        .env_remove("ROBA_PROFILE")
        .output()
        .expect("run");

    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("[roba] warning:"),
        "expected no warning with --quiet, got:\n{stderr}"
    );
    assert!(
        stderr.contains("reading --prepend"),
        "expected prepend error, got:\n{stderr}"
    );
}

#[test]
fn agent_check_suppressed_by_full_auto() {
    let project = project_with_bash_agent();
    let user_home = tempfile::tempdir().expect("user home");

    let out = roba()
        .args([
            "-C",
            project.path().to_str().unwrap(),
            "--agent",
            "test-agent",
            "--full-auto",
            "--prepend",
            "/no/such/agent-check-fullauto-test",
            "some prompt",
        ])
        .env("HOME", user_home.path())
        .env_remove("ROBA_PROFILE")
        .output()
        .expect("run");

    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("[roba] warning:"),
        "expected no warning with --full-auto, got:\n{stderr}"
    );
    assert!(
        stderr.contains("reading --prepend"),
        "expected prepend error, got:\n{stderr}"
    );
}

// ---------------------------------------------------------------------------
// --effort flag
// ---------------------------------------------------------------------------

#[test]
fn effort_parses_all_variants() {
    use clap::Parser;
    use roba::cli::{Cli, EffortLevel};
    for (s, expected) in [
        ("low", EffortLevel::Low),
        ("medium", EffortLevel::Medium),
        ("high", EffortLevel::High),
        ("xhigh", EffortLevel::Xhigh),
        ("max", EffortLevel::Max),
    ] {
        let cli = Cli::try_parse_from(["roba", "--effort", s, "prompt"]).unwrap();
        assert_eq!(cli.ask.effort, Some(expected), "variant {s:?}");
    }
}

#[test]
fn effort_invalid_value_errors() {
    use clap::Parser;
    use roba::cli::Cli;
    assert!(Cli::try_parse_from(["roba", "--effort", "ultra", "prompt"]).is_err());
}

#[test]
fn effort_unset_is_none() {
    use clap::Parser;
    use roba::cli::Cli;
    let cli = Cli::try_parse_from(["roba", "prompt"]).unwrap();
    assert!(cli.ask.effort.is_none());
}

// ---------------------------------------------------------------------------
// --system-prompt / --append-system-prompt (parse-level, no claude call)
// ---------------------------------------------------------------------------

#[test]
fn system_prompt_flag_parses() {
    // The flag must be recognized by clap; --show-permissions exits 0 without
    // calling claude so this never needs a real API key.
    let project = empty_project();
    let user_home = tempfile::tempdir().expect("user home");
    roba()
        .args([
            "-C",
            project.path().to_str().unwrap(),
            "--system-prompt",
            "You are a helpful assistant",
            "--show-permissions",
        ])
        .env("XDG_CONFIG_HOME", user_home.path())
        .env_remove("ROBA_PROFILE")
        .assert()
        .success();
}

#[test]
fn append_system_prompt_flag_parses() {
    let project = empty_project();
    let user_home = tempfile::tempdir().expect("user home");
    roba()
        .args([
            "-C",
            project.path().to_str().unwrap(),
            "--append-system-prompt",
            "Be concise.",
            "--show-permissions",
        ])
        .env("XDG_CONFIG_HOME", user_home.path())
        .env_remove("ROBA_PROFILE")
        .assert()
        .success();
}

#[test]
fn both_system_prompt_flags_coexist() {
    // Both flags set together must not cause a clap conflict error.
    let project = empty_project();
    let user_home = tempfile::tempdir().expect("user home");
    roba()
        .args([
            "-C",
            project.path().to_str().unwrap(),
            "--system-prompt",
            "Role: reviewer",
            "--append-system-prompt",
            "Be concise.",
            "--show-permissions",
        ])
        .env("XDG_CONFIG_HOME", user_home.path())
        .env_remove("ROBA_PROFILE")
        .assert()
        .success();
}

// ---------------------------------------------------------------------------
// completions
// ---------------------------------------------------------------------------

#[test]
fn completions_bash_prints_script_and_exits_zero() {
    // Pure generator: emits a completion script to stdout, no claude
    // call. The script references the binary name.
    roba()
        .args(["completions", "bash"])
        .assert()
        .success()
        .stdout(predicate::str::contains("roba"));
}

// ---------------------------------------------------------------------------
// doctor
// ---------------------------------------------------------------------------

#[test]
fn doctor_emits_all_check_lines() {
    // `roba doctor` runs the boundary checks and prints one line per
    // check to stdout. Assert structure (the check names are present),
    // not pass/fail -- the outcome depends on the test environment
    // (whether `claude` is installed, ANTHROPIC_API_KEY set, etc.).
    // No status assertion for the same reason: a missing `claude`
    // binary makes the command exit 1, which is correct behavior.
    roba()
        .arg("doctor")
        .assert()
        .stdout(predicate::str::contains("claude"))
        .stdout(predicate::str::contains("auth"))
        .stdout(predicate::str::contains("config"))
        .stdout(predicate::str::contains("rates"));
}

#[test]
fn doctor_json_carries_version_result_and_consistent_exit() {
    // The uniform { version: 1, result: { checks, overall } } envelope.
    // The exit code (0 unless any check fails) depends on the test
    // environment (whether `claude` is installed, etc.), so assert the
    // shape and the exit-code/overall *consistency* rather than a fixed
    // code: exit is 1 exactly when overall == "fail".
    let out = roba().args(["doctor", "--json"]).output().expect("run");
    let v: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("stdout is JSON on --json");
    assert_eq!(v["version"], 1, "top-level version must be 1");
    assert!(v.get("error").is_none(), "error must be absent on success");
    let checks = v["result"]["checks"].as_array().expect("checks array");
    let names: Vec<&str> = checks
        .iter()
        .map(|c| c["name"].as_str().expect("check name is a string"))
        .collect();
    assert_eq!(names, vec!["claude", "auth", "config", "rates"]);
    // Every check carries a status + message.
    for c in checks {
        assert!(c["status"].is_string(), "status must be a string");
        assert!(c["message"].is_string(), "message must be a string");
    }
    let overall = v["result"]["overall"]
        .as_str()
        .expect("overall is a string");
    let code = out.status.code().expect("exited normally");
    assert_eq!(
        code == 1,
        overall == "fail",
        "exit 1 iff overall is fail (overall={overall}, code={code})"
    );
    assert!(code == 0 || code == 1, "doctor exits 0 or 1, got {code}");
}
