//! Scenario suite -- end-to-end "autonomous-work journey" tests.
//!
//! Codifies the manual shakedown as a repeatable PRE-RELEASE regression
//! gate. Each scenario drives roba the way a user or agent drives it for
//! unattended work (composed pipeline, detached hand-off, dispatch,
//! session re-entry, safe-by-default) and asserts the journey's MECHANICAL
//! invariants.
//!
//! Load-bearing rule (the same one live.rs follows, at scenario scale):
//! **assert mechanics you control, never model compliance.** We check
//! that the `--json` envelope is well-formed, stdout is byte-clean, the
//! detached handle is the only stdout, the short id resolves to the same
//! session, a write was denied, a commit was produced -- never that the
//! model's *answer* is correct.
//!
//! Two reliability tiers:
//!   - **Tier A (deterministic mechanics):** plumbing that holds
//!     regardless of what the model says -- envelope shape, pipe
//!     cleanliness, handle-is-stdout, short-id resolution, permission
//!     denial. These are as solid as the mechanical `tests/cli.rs`
//!     tests, just exercised end to end against real claude.
//!   - **Tier B (agentic):** the `--full-auto` dispatch fixes a failing
//!     test in a seeded crate and asserts the MECHANICS of a completed
//!     dispatch (`cargo test` green + a commit produced). Reliable but
//!     NOT deterministic -- the model must actually do the work -- so the
//!     fidelity-vs-reliability tension is resolved with the MODEL, not the
//!     task: this one scenario uses a capable model (sonnet), so a red
//!     means roba's dispatch regressed, not that a weak model flubbed.
//!     The single non-deterministic test in the suite.
//!
//! Local-only / opt-in (real claude, costs money). All tests are
//! `#[ignore]`:
//!
//!   cargo test --test scenarios -- --ignored --nocapture
//!   just scenario                      # equivalent
//!
//! Short prompts; the four Tier-A scenarios are haiku, the one Tier-B
//! dispatch is sonnet; budget ~$1-2 for the suite. Run before cutting a
//! release (see the "Before cutting a release" checklist).
//!
//! Naming: every test is `scenario_<name>` so `just scenario` and a
//! `cargo test ... scenario_` filter select the suite. Helpers live at
//! the top of this file (kept self-contained; a future `tests/common`
//! module could de-dup the small overlap with live.rs).

use assert_cmd::Command;
use predicates::prelude::*;
use std::path::Path;

/// Run `roba` against `dir` via `-C`, defaulting to the haiku model.
/// For the ask path (a prompt). Subcommands (`show`, etc.) reject
/// `--model`, so use [`roba_sub`] for those.
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

/// Run a roba *subcommand* against `dir` via `-C` (no `--model`, which
/// only the ask path accepts). Use for `show`, `history`, etc.
fn roba_sub(dir: &Path) -> Command {
    let mut cmd = Command::cargo_bin("roba").expect("cargo-built roba binary");
    cmd.args(["-C", dir.to_str().expect("utf-8 tempdir path")]);
    cmd
}

fn fresh_dir() -> tempfile::TempDir {
    tempfile::tempdir().expect("create test tempdir")
}

/// True if `s` looks like a v4-shaped UUID (`8-4-4-4-12` hex). Used to
/// assert the `--detach` handle-is-stdout contract without pulling in a
/// uuid dep.
fn is_uuid_shaped(s: &str) -> bool {
    let groups = [8usize, 4, 4, 4, 12];
    let parts: Vec<&str> = s.split('-').collect();
    parts.len() == groups.len()
        && parts
            .iter()
            .zip(groups)
            .all(|(p, n)| p.len() == n && p.chars().all(|c| c.is_ascii_hexdigit()))
}

/// Run a `git` subcommand in `dir`, panicking on failure. For fixture
/// setup and the commit-count oracle.
fn git(dir: &Path, args: &[&str]) {
    let status = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .status()
        .expect("git available");
    assert!(status.success(), "git {args:?} failed");
}

/// Number of commits reachable from HEAD (`0` if none). The mechanical
/// "a commit was produced" oracle: compare before/after a dispatch.
fn commit_count(dir: &Path) -> usize {
    let out = std::process::Command::new("git")
        .args(["rev-list", "--count", "HEAD"])
        .current_dir(dir)
        .output()
        .expect("git available");
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse()
        .unwrap_or(0)
}

/// Seed a tempdir with a minimal git repo + a tiny Rust crate whose test
/// FAILS until a stubbed function is implemented -- the canonical
/// "fix the failing test" dispatch fixture. Local git identity is set so
/// the worker's own `git commit` succeeds under `--full-auto`.
fn seed_failing_crate() -> tempfile::TempDir {
    let tmp = fresh_dir();
    let p = tmp.path();
    std::fs::write(
        p.join("Cargo.toml"),
        "[package]\nname = \"fixture\"\nversion = \"0.0.0\"\nedition = \"2021\"\n\n\
         [lib]\npath = \"src/lib.rs\"\n",
    )
    .expect("write Cargo.toml");
    std::fs::create_dir_all(p.join("src")).expect("src dir");
    std::fs::write(
        p.join("src/lib.rs"),
        "//! Fixture crate: the test below fails until `add` is implemented.\n\n\
         /// Return the sum of two integers.\n\
         pub fn add(a: i32, b: i32) -> i32 {\n    \
         todo!(\"implement: return a + b\")\n}\n\n\
         #[cfg(test)]\nmod tests {\n    use super::*;\n\n    \
         #[test]\n    fn adds() {\n        \
         assert_eq!(add(2, 2), 4);\n        \
         assert_eq!(add(10, 5), 15);\n    }\n}\n",
    )
    .expect("write src/lib.rs");
    git(p, &["init", "-q"]);
    git(p, &["config", "user.email", "scenario@roba.test"]);
    git(p, &["config", "user.name", "roba scenario"]);
    git(p, &["add", "-A"]);
    git(p, &["commit", "-qm", "seed: failing add() test"]);
    tmp
}

// ===========================================================================
// Tier A -- deterministic mechanics
// ===========================================================================

/// Composed-input pipeline: piped stdin as context + `--json`. The
/// journey a script uses (`cat file | roba --json | jq`). Mechanical:
/// stdout is a single well-formed `{version:1, result:{...}}` envelope a
/// downstream consumer can parse, and metadata stays off stdout.
#[test]
#[ignore]
fn scenario_pipeline_json_clean() {
    let dir = fresh_dir();
    let out = roba_in(dir.path())
        .args(["--json", "reply with the single token PONG"])
        .write_stdin("some piped context\n")
        .output()
        .expect("run roba --json with piped stdin");
    assert!(out.status.success(), "pipeline run exits 0");

    let stdout = String::from_utf8(out.stdout).expect("utf-8 stdout");
    // The whole point: stdout parses as ONE clean JSON object. A metadata
    // leak (footer/spinner/tool line) would make this fail.
    let v: serde_json::Value = serde_json::from_str(stdout.trim())
        .expect("stdout is a single clean JSON object (no metadata leak)");
    assert_eq!(v["version"], serde_json::json!(1), "versioned envelope");
    assert!(
        v["result"]["result"].is_string(),
        "result.result is present and a string"
    );
    assert!(
        v["result"]["session_id"].is_string(),
        "result.session_id is present"
    );
}

/// Detached hand-off roundtrip: `--detach` prints ONLY the session
/// handle, then a later `show <id> --wait` re-attaches and returns the
/// completed run. Mechanical: stdout-is-a-bare-uuid (handle-first
/// contract), then the handle resolves to a non-empty result. The
/// agentic part is a trivial "say a token", kept minimal.
#[test]
#[ignore]
fn scenario_detach_handoff() {
    let dir = fresh_dir();

    // 1. Fire detached; the only stdout is the handle.
    let out = roba_in(dir.path())
        .args([
            "--detach",
            "--max-turns",
            "3",
            "reply with the single token DETACHED",
        ])
        .output()
        .expect("run roba --detach");
    assert!(out.status.success(), "--detach exits 0 after spawning");
    let handle = String::from_utf8(out.stdout)
        .expect("utf-8 stdout")
        .trim()
        .to_string();
    assert!(
        is_uuid_shaped(&handle),
        "stdout is the bare session handle; got {handle:?}"
    );

    // 2. Re-attach from a fresh invocation and block until complete.
    roba_sub(dir.path())
        .args(["show", &handle, "--wait", "--timeout", "120"])
        .assert()
        .success()
        .stdout(predicate::str::is_empty().not());
}

// ===========================================================================
// Tier B -- trivial-agentic (reliable, not deterministic)
// ===========================================================================

/// The dispatch journey: fire a `--full-auto` worker at a seeded crate
/// with a failing test and assert the MECHANICS of a completed dispatch --
/// `cargo test` now passes and a new commit was produced.
///
/// **Why sonnet, not haiku:** this tests roba's DISPATCH PLUMBING end to
/// end (full-auto edits + the run drives a real fix + a commit lands), so
/// a red must mean roba's dispatch regressed, not that a weak model
/// flubbed a real task. A capable-enough model makes the agentic
/// completion reliable while the assertion stays purely mechanical -- the
/// test oracle and commit existence, never "is the code good". This is
/// the single non-deterministic scenario in the suite; the other four are
/// Tier A.
#[test]
#[ignore]
fn scenario_dispatch_fixes_failing_test() {
    let repo = seed_failing_crate();
    let before = commit_count(repo.path());

    // The canonical dispatch shape: full-auto, capable model, a bounded
    // budget, a scoped task that mirrors the real loop (implement + commit).
    Command::cargo_bin("roba")
        .expect("cargo-built roba binary")
        .args([
            "-C",
            repo.path().to_str().expect("utf-8 path"),
            "--full-auto",
            "--model",
            "sonnet",
            "--max-turns",
            "30",
            "Implement the `add` function in src/lib.rs so `cargo test` passes \
             (it currently has a todo!()), then commit the change.",
        ])
        .assert()
        .success();

    // Oracle 1: the test now passes. The test IS the correctness gate, so
    // this asserts a mechanic (green/red), not the model's answer.
    let test_status = std::process::Command::new("cargo")
        .args(["test", "--quiet"])
        .current_dir(repo.path())
        .status()
        .expect("cargo available");
    assert!(
        test_status.success(),
        "after the dispatch, `cargo test` passes in the fixture crate"
    );

    // Oracle 2: a new commit was produced (the dispatch mirrors the real
    // loop, where the worker commits its own change).
    assert!(
        commit_count(repo.path()) > before,
        "the dispatch produced a new commit (was {before})"
    );
}

// ===========================================================================
// Tier A -- session re-entry + safe-by-default (deterministic mechanics)
// ===========================================================================

/// Session re-entry by the SHORT id roba displays. Run once, take the first 8
/// chars of the session id (the footer/`show` form), and `-c <short>` it.
/// Mechanical: the short id RESOLVES (no "not a UUID" reject, #304) and the
/// run continues the SAME session -- the resumed envelope's session_id equals
/// the original full id. We assert continuity of the handle, never that the
/// model recalled anything.
///
/// This is the scenario whose first run found #310 (current-project enumeration
/// missed sessions in symlinked/dotted cwds, so the short id had nothing to
/// resolve against from a tempdir). Re-enabled now that #310 is fixed in
/// claude-wrapper 0.12.0 (slug derivation canonicalizes + encodes `.`).
#[test]
#[ignore]
fn scenario_session_reentry_shortid() {
    let dir = fresh_dir();

    // Run 1: establish a session; capture its full id from the envelope.
    let out1 = roba_in(dir.path())
        .args(["--json", "reply with the single token ONE"])
        .output()
        .expect("run 1");
    assert!(out1.status.success(), "first run exits 0");
    let v1: serde_json::Value =
        serde_json::from_str(String::from_utf8(out1.stdout).expect("utf8").trim())
            .expect("run 1 --json");
    let full_id = v1["result"]["session_id"]
        .as_str()
        .expect("session_id present")
        .to_string();
    assert!(full_id.len() >= 8, "session id long enough to shorten");
    let short = &full_id[..8];

    // Run 2: continue via the SHORT id. #304 resolves the prefix against the
    // project's sessions (which #310 made findable); no UUID reject.
    let out2 = roba_in(dir.path())
        .args(["--json", "-c", short, "reply with the single token TWO"])
        .output()
        .expect("run 2");
    let stderr2 = String::from_utf8_lossy(&out2.stderr);
    assert!(
        !stderr2.contains("not a UUID") && !stderr2.contains("Invalid session"),
        "short id `{short}` resolved (no UUID reject); stderr: {stderr2}"
    );
    assert!(out2.status.success(), "continue-by-short-id exits 0");
    let v2: serde_json::Value =
        serde_json::from_str(String::from_utf8(out2.stdout).expect("utf8").trim())
            .expect("run 2 --json");
    assert_eq!(
        v2["result"]["session_id"].as_str(),
        Some(full_id.as_str()),
        "the short id resumed the SAME session (handle continuity)"
    );
}

/// Safe-by-default: under the default read-only posture, a prompt that asks
/// to WRITE a file must not produce one. Mechanical safety guarantee: the
/// file does not exist afterward (the permission system never grants Write
/// without an opt-in). If the model attempted the write, the envelope's
/// `permission_denials` records the block -- asserted as a bonus when present,
/// but the load-bearing guarantee is the absent file.
#[test]
#[ignore]
fn scenario_readonly_denies_write() {
    let dir = fresh_dir();
    let target = dir.path().join("SHOULD_NOT_EXIST.txt");

    let out = roba_in(dir.path())
        .args([
            "--json",
            "Use the Write tool to create a file named SHOULD_NOT_EXIST.txt \
             containing the word oops in the current directory.",
        ])
        .output()
        .expect("run");
    assert!(out.status.success(), "run exits 0");

    // The hard guarantee: no write happened under the default posture.
    assert!(
        !target.exists(),
        "default read-only posture must not allow a Write"
    );

    // Bonus: if the model tried, the envelope records the denial. Only assert
    // when present -- the model may decline to attempt at all, which is still a
    // safe outcome (the file is absent either way).
    let v: serde_json::Value =
        serde_json::from_str(String::from_utf8(out.stdout).expect("utf8").trim()).expect("--json");
    if let Some(denials) = v["result"]["permission_denials"].as_array()
        && !denials.is_empty()
    {
        let mentions_write = denials.iter().any(|d| {
            d.as_str().map(|s| s.contains("Write")).unwrap_or(false)
                || d["tool_name"].as_str() == Some("Write")
                || d.to_string().contains("Write")
        });
        assert!(
            mentions_write,
            "a recorded permission denial should name Write; got {denials:?}"
        );
    }
}
