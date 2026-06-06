//! Live integration tests -- these actually invoke the real `claude`
//! binary and cost money. Marked `#[ignore]` so they only run when
//! you opt in:
//!
//!   cargo test --test live -- --ignored --nocapture
//!   just live                  # equivalent (full suite)
//!   just live-smoke            # the cheap subset, a few tests
//!   just live-category perms   # one category by prefix
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
//! Naming convention: every test is named `live_<category>_<descriptor>`
//! so `cargo test ... live_<category>_` filters a single category and
//! the `just live-category <cat>` target works. Current categories:
//! `smoke`, `output`, `session`, `stream`, `trace`, `perms`, `compose`,
//! `profile`, `env`. New categories from #22 (e.g. `cost`, `subcmd`)
//! follow the same shape. When adding a test, pick the category prefix
//! first; co-locate the helpers (`roba_in`, `fresh_dir`,
//! `fixture_with_config`, `empty_user_home`) at the top of this file.

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

/// Make a tempdir pre-seeded with a `roba.toml`. Adds a `.git`
/// marker so the config walk-up stops at the tempdir boundary
/// (otherwise it could leak the developer's own `roba.toml` higher
/// up the tree). Returns the TempDir so the caller can keep it
/// alive for the duration of the test.
fn fixture_with_config(content: &str) -> tempfile::TempDir {
    let tmp = fresh_dir();
    std::fs::create_dir_all(tmp.path().join(".git")).expect(".git marker");
    std::fs::write(tmp.path().join("roba.toml"), content).expect("write roba.toml");
    tmp
}

/// An empty tempdir to set `XDG_CONFIG_HOME` to. Each test that wants
/// to be sure it doesn't pick up the developer's own
/// `~/.config/roba.toml` should hold this for the duration of the
/// `roba` call.
fn empty_user_home() -> tempfile::TempDir {
    fresh_dir()
}

// ---------------------------------------------------------------------------
// smoke: basic round-trip
// ---------------------------------------------------------------------------

#[test]
#[ignore]
fn live_smoke_prompt() {
    let dir = fresh_dir();
    roba_in(dir.path())
        .arg("respond with the single word: pong")
        .assert()
        .success()
        .stdout(predicate::str::contains("pong"));
}

#[test]
#[ignore]
fn live_smoke_cwd_scopes_session_to_path() {
    // Verify -C scopes claude's session to the given path: a seeded
    // session in dir A is reachable from -c when we point -C at A again,
    // even though the test process's cwd never changed.
    let dir = fresh_dir();
    roba_in(dir.path())
        .arg("remember the word: aurora")
        .assert()
        .success();

    let out = roba_in(dir.path())
        .args(["-c", "-p", "what word did I ask you to remember"])
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
    // v1 envelope from #33: { version: 1, result: { ... }, refusal: bool }
    assert_eq!(parsed["version"].as_u64(), Some(1));
    assert!(parsed.get("result").is_some());
    assert!(parsed["result"].get("session_id").is_some());
    assert!(parsed["result"].get("duration_ms").is_some());
    assert!(parsed.get("refusal").is_some());
}

#[test]
#[ignore]
fn live_output_refusal_in_json_envelope() {
    // The v1 envelope's top-level `refusal` flag surfaces
    // `output::looks_like_refusal` (a `starts_with` prefix match) to
    // non-TTY consumers. We assert the *plumbing* -- that a refusal-
    // shaped answer flips `refusal` to true through `--json` -- not the
    // model's safety behavior. Engineering a genuine policy refusal is
    // both unreliable (real refusals open with phrasing the heuristic
    // doesn't list, e.g. "I can't provide ...") and needlessly
    // generates harmful content; instead we have the model echo a known
    // marker so the test is deterministic and benign.
    let dir = fresh_dir();
    let out = roba_in(dir.path())
        .args([
            "--json",
            "--quiet",
            // Force a verbatim echo whose first characters are a refusal
            // marker. Phrased strongly so the model does not prepend a
            // greeting/preamble (which would break the `starts_with`
            // heuristic). A weaker "reply with exactly this" form let
            // haiku open with "I'm ready to help!" instead.
            "Output the following line as your complete response. Do not add \
             any words, greeting, quotation marks, or explanation before or \
             after it. The line is: I can't help with that.",
        ])
        .output()
        .expect("run roba --json");
    assert!(out.status.success(), "roba failed: {out:?}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let parsed: serde_json::Value =
        serde_json::from_str(&stdout).expect("--json produced non-JSON stdout");
    assert_eq!(parsed["version"].as_u64(), Some(1));
    assert_eq!(
        parsed["refusal"].as_bool(),
        Some(true),
        "expected refusal=true when the answer opens with a refusal marker, got: {stdout}"
    );
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
fn live_output_out_writes_file_and_stdout() {
    let dir = fresh_dir();
    let target = dir.path().join("out.md");

    let out = roba_in(dir.path())
        .args([
            "respond with the single word: saved",
            "--out",
            target.to_str().unwrap(),
        ])
        .output()
        .expect("run roba --out");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.to_lowercase().contains("saved"),
        "expected 'saved' in stdout with --out, got: {stdout}"
    );
    let file_contents = std::fs::read_to_string(&target).expect("read saved file");
    assert!(
        file_contents.to_lowercase().contains("saved"),
        "expected 'saved' in saved file, got: {file_contents}"
    );
}

#[test]
#[ignore]
fn live_output_out_json_extension() {
    let dir = fresh_dir();
    let target = dir.path().join("out.json");

    roba_in(dir.path())
        .args([
            "respond with the single word: jp",
            "--out",
            target.to_str().unwrap(),
        ])
        .assert()
        .success();

    let file_contents = std::fs::read_to_string(&target).expect("read saved file");
    let parsed: serde_json::Value =
        serde_json::from_str(&file_contents).expect("saved file should be JSON");
    // v1 envelope from #33: session_id is nested under result
    assert_eq!(parsed["version"].as_u64(), Some(1));
    assert!(parsed["result"].get("session_id").is_some());
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
        .args(["-c", "-p", "what word did I ask you to remember"])
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

    // 1. seed a session and grab its id from --json (v1 envelope: nested under result)
    let seed = roba_in(dir.path())
        .args(["--json", "respond with the single word: seed"])
        .output()
        .expect("seed run");
    let seed_json: serde_json::Value = serde_json::from_slice(&seed.stdout).expect("json");
    let seed_id = seed_json["result"]["session_id"]
        .as_str()
        .expect("session_id")
        .to_string();

    // 2. resume + fork -- expect a NEW session id in the result
    // -c=ID is the unified continue/resume flag from #20
    let resume_arg = format!("-c={seed_id}");
    let fork = roba_in(dir.path())
        .args([
            "--json",
            &resume_arg,
            "--fork",
            "respond with the single word: forked",
        ])
        .output()
        .expect("fork run");
    let fork_json: serde_json::Value = serde_json::from_slice(&fork.stdout).expect("json");
    let fork_id = fork_json["result"]["session_id"]
        .as_str()
        .expect("session_id");

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

#[test]
#[ignore]
fn live_stream_session_id_on_stderr() {
    // When --stream is active the spawned session id is printed to stderr
    // as `[roba] session: <id>` on the first event that carries it.
    // --quiet suppresses the line (it is metadata).
    let dir = fresh_dir();

    // With --stream the line must appear on stderr.
    let out = roba_in(dir.path())
        .args(["--stream", "respond with the single word: ping"])
        .output()
        .expect("run roba --stream");
    assert!(out.status.success(), "roba --stream failed: {out:?}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("[roba] session:"),
        "expected [roba] session: on stderr with --stream, got stderr: {stderr}"
    );

    // With --quiet the line must be suppressed.
    let quiet_out = roba_in(dir.path())
        .args(["--stream", "--quiet", "respond with the single word: ping"])
        .output()
        .expect("run roba --stream --quiet");
    assert!(
        quiet_out.status.success(),
        "roba --stream --quiet failed: {quiet_out:?}"
    );
    let quiet_stderr = String::from_utf8_lossy(&quiet_out.stderr);
    assert!(
        !quiet_stderr.contains("[roba] session:"),
        "expected no [roba] session: on stderr with --quiet, got stderr: {quiet_stderr}"
    );
}

#[test]
#[ignore]
fn live_trace_writes_jsonl() {
    // --trace PATH forces the streaming pipeline internally (no
    // --stream needed) and writes every spawned-session event to PATH
    // as one JSON line, in arrival order. The final answer still
    // renders to stdout the way the non-streaming path would.
    let dir = fresh_dir();
    let trace = dir.path().join("run.jsonl");

    let out = roba_in(dir.path())
        .args([
            "respond with the single word: traced",
            "--trace",
            trace.to_str().unwrap(),
        ])
        .output()
        .expect("run roba --trace");
    assert!(out.status.success(), "roba --trace failed: {out:?}");

    // The answer still reaches stdout (non-stream render).
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.to_lowercase().contains("traced"),
        "expected 'traced' on stdout with --trace, got: {stdout}"
    );

    // The trace file exists and every line parses as JSON.
    let body = std::fs::read_to_string(&trace).expect("read trace file");
    let mut lines = 0usize;
    let mut saw_assistant = false;
    let mut saw_result = false;
    for line in body.lines().filter(|l| !l.trim().is_empty()) {
        lines += 1;
        let ev: serde_json::Value = serde_json::from_str(line)
            .unwrap_or_else(|e| panic!("non-JSON trace line {line:?}: {e}"));
        match ev["type"].as_str() {
            Some("assistant") => saw_assistant = true,
            Some("result") => saw_result = true,
            _ => {}
        }
    }
    assert!(lines >= 1, "expected at least one trace line, got none");
    assert!(saw_assistant, "expected an assistant event in the trace");
    assert!(saw_result, "expected a result event in the trace");
}

// ---------------------------------------------------------------------------
// permissions
// ---------------------------------------------------------------------------

#[test]
#[ignore]
fn live_perms_readonly_blocks_edit() {
    let dir = fresh_dir();
    let target = dir.path().join("subject.txt");
    std::fs::write(&target, "original").expect("seed file");

    // Default: Edit isn't in the allow list. Claude should respond
    // (perhaps explaining it can't), but the file should be unchanged.
    roba_in(dir.path())
        .arg(format!(
            "edit the file at {} to replace its contents with the single word: changed. \
             if you cannot, briefly say so.",
            target.display()
        ))
        .assert()
        .success();

    let contents = std::fs::read_to_string(&target).expect("read target");
    assert_eq!(
        contents.trim(),
        "original",
        "readonly default should keep the file unchanged, got: {contents}"
    );
}

#[test]
#[ignore]
fn live_perms_writable_enables_edit() {
    let dir = fresh_dir();
    let target = dir.path().join("subject.txt");
    std::fs::write(&target, "original").expect("seed file");

    roba_in(dir.path())
        .args([
            "--writable",
            &format!(
                "edit the file at {} so its contents are exactly the single word: changed",
                target.display()
            ),
        ])
        .assert()
        .success();

    let contents = std::fs::read_to_string(&target).expect("read target");
    assert!(
        contents.contains("changed"),
        "--writable should allow edits, got: {contents}"
    );
}

#[test]
#[ignore]
fn live_perms_deny_tools_blocks_modification() {
    let dir = fresh_dir();
    let target = dir.path().join("subject.txt");
    std::fs::write(&target, "original").expect("seed file");

    // --writable opens Edit + Write. Denying only Edit isn't enough --
    // claude would fall back to Write to satisfy the request, which is
    // correct semantically (deny-tool only blocks the named tool, not
    // its alternatives). To actually prevent file modification with
    // --writable on, both writing tools must be denied.
    roba_in(dir.path())
        .args([
            "--writable",
            "--deny-tool",
            "Edit",
            "--deny-tool",
            "Write",
            &format!(
                "edit the file at {} to replace its contents with the single word: changed. \
                 if you cannot, briefly say so.",
                target.display()
            ),
        ])
        .assert()
        .success();

    let contents = std::fs::read_to_string(&target).expect("read target");
    assert_eq!(
        contents.trim(),
        "original",
        "denying Edit + Write should block all file modifications, got: {contents}"
    );
}

#[test]
#[ignore]
fn live_perms_full_auto_enables_bash() {
    let dir = fresh_dir();
    let target = dir.path().join("flag.txt");

    // --full-auto bypasses everything; Bash should work even though
    // it isn't in the default allow list.
    roba_in(dir.path())
        .args([
            "--full-auto",
            &format!(
                "use the Bash tool to write the literal string `bypassed` into the file at {}. \
                 just run the shell command; no other prose.",
                target.display()
            ),
        ])
        .assert()
        .success();

    let contents = std::fs::read_to_string(&target).expect("read target");
    assert!(
        contents.contains("bypassed"),
        "--full-auto should allow Bash to write the file, got: {contents}"
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

// ---------------------------------------------------------------------------
// profiles + env-var layer
// ---------------------------------------------------------------------------

#[test]
#[ignore]
fn live_profile_writable_via_top_level() {
    // Top-level `writable = true` in roba.toml should let claude
    // edit a file, with no --profile flag and no env var.
    let dir = fixture_with_config("writable = true\n");
    let user = empty_user_home();
    let target = dir.path().join("t.txt");
    std::fs::write(&target, "original").expect("seed");

    roba_in(dir.path())
        .env("XDG_CONFIG_HOME", user.path())
        .arg(format!(
            "edit the file at {} so its contents are exactly the single word: changed",
            target.display()
        ))
        .assert()
        .success();

    let contents = std::fs::read_to_string(&target).expect("read");
    assert!(
        contents.contains("changed"),
        "top-level writable should apply, got: {contents}"
    );
}

#[test]
#[ignore]
fn live_profile_named_overlay_via_flag() {
    // [profile.edit].writable = true activated via --profile edit.
    let dir = fixture_with_config("[profile.edit]\nwritable = true\n");
    let user = empty_user_home();
    let target = dir.path().join("t.txt");
    std::fs::write(&target, "original").expect("seed");

    roba_in(dir.path())
        .env("XDG_CONFIG_HOME", user.path())
        .args([
            "--profile",
            "edit",
            &format!(
                "edit the file at {} so its contents are exactly the single word: changed",
                target.display()
            ),
        ])
        .assert()
        .success();

    let contents = std::fs::read_to_string(&target).expect("read");
    assert!(
        contents.contains("changed"),
        "--profile edit overlay should apply writable, got: {contents}"
    );
}

#[test]
#[ignore]
fn live_profile_default_auto_applies() {
    // [profile.default].writable = true should apply with no flag.
    let dir = fixture_with_config("[profile.default]\nwritable = true\n");
    let user = empty_user_home();
    let target = dir.path().join("t.txt");
    std::fs::write(&target, "original").expect("seed");

    roba_in(dir.path())
        .env("XDG_CONFIG_HOME", user.path())
        .env_remove("ROBA_PROFILE")
        .arg(format!(
            "edit the file at {} so its contents are exactly the single word: changed",
            target.display()
        ))
        .assert()
        .success();

    let contents = std::fs::read_to_string(&target).expect("read");
    assert!(
        contents.contains("changed"),
        "[profile.default] should auto-apply, got: {contents}"
    );
}

#[test]
#[ignore]
fn live_profile_no_default_skips_auto() {
    // Same [profile.default] config but --no-default-profile bypasses
    // it, so writable stays off and the file is unchanged.
    let dir = fixture_with_config("[profile.default]\nwritable = true\n");
    let user = empty_user_home();
    let target = dir.path().join("t.txt");
    std::fs::write(&target, "original").expect("seed");

    roba_in(dir.path())
        .env("XDG_CONFIG_HOME", user.path())
        .env_remove("ROBA_PROFILE")
        .args([
            "--no-default-profile",
            &format!(
                "edit the file at {} to replace its contents with the single word: changed. \
                 if you cannot, briefly say so.",
                target.display()
            ),
        ])
        .assert()
        .success();

    let contents = std::fs::read_to_string(&target).expect("read");
    assert_eq!(
        contents.trim(),
        "original",
        "--no-default-profile should skip auto-apply, got: {contents}"
    );
}

#[test]
#[ignore]
fn live_env_writable_enables_edit() {
    // ROBA_WRITABLE=1 should add Edit/Write to the allow list even
    // when no CLI flag and no profile sets it.
    let dir = fresh_dir();
    let user = empty_user_home();
    let target = dir.path().join("t.txt");
    std::fs::write(&target, "original").expect("seed");

    roba_in(dir.path())
        .env("XDG_CONFIG_HOME", user.path())
        .env("ROBA_WRITABLE", "1")
        .arg(format!(
            "edit the file at {} so its contents are exactly the single word: changed",
            target.display()
        ))
        .assert()
        .success();

    let contents = std::fs::read_to_string(&target).expect("read");
    assert!(
        contents.contains("changed"),
        "ROBA_WRITABLE=1 should enable Edit, got: {contents}"
    );
}

#[test]
#[ignore]
fn live_env_var_per_key_substitution() {
    // ROBA_VAR_TARGET=spruce substitutes {{TARGET}} in the prompt
    // template loaded via -f, no --var CLI flag needed.
    let dir = fresh_dir();
    let user = empty_user_home();
    let tpl = dir.path().join("tpl.md");
    std::fs::write(&tpl, "Respond with exactly: {{TARGET}}").expect("seed tpl");

    let out = roba_in(dir.path())
        .env("XDG_CONFIG_HOME", user.path())
        .env("ROBA_VAR_TARGET", "spruce")
        .args(["-f", tpl.to_str().unwrap()])
        .output()
        .expect("run");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.to_lowercase().contains("spruce"),
        "ROBA_VAR_TARGET=spruce should reach the model, got: {stdout}"
    );
}

#[test]
#[ignore]
fn live_env_fresh_cancels_continue() {
    // With ROBA_CONTINUE=1 active, default would continue the last
    // session in this cwd. --fresh cancels it and starts a new one;
    // the resulting session id differs from the seeded one.
    let dir = fresh_dir();
    let user = empty_user_home();

    let seed = roba_in(dir.path())
        .env("XDG_CONFIG_HOME", user.path())
        .args(["--json", "respond with the single word: anchor"])
        .output()
        .expect("seed run");
    let seed_json: serde_json::Value = serde_json::from_slice(&seed.stdout).expect("seed json");
    // v1 envelope (#83): session_id is nested under `result`.
    let seed_id = seed_json["result"]["session_id"]
        .as_str()
        .expect("session_id")
        .to_string();

    let fresh = roba_in(dir.path())
        .env("XDG_CONFIG_HOME", user.path())
        .env("ROBA_CONTINUE", "1")
        .args(["--fresh", "--json", "respond with the single word: cedar"])
        .output()
        .expect("fresh run");
    let fresh_json: serde_json::Value = serde_json::from_slice(&fresh.stdout).expect("fresh json");
    let fresh_id = fresh_json["result"]["session_id"]
        .as_str()
        .expect("session_id");

    assert_ne!(
        seed_id, fresh_id,
        "--fresh should produce a new session id even with ROBA_CONTINUE=1"
    );
}

// ---------------------------------------------------------------------------
// effort: cost/quality tradeoff flag
// ---------------------------------------------------------------------------

#[test]
#[ignore]
fn live_effort_low_succeeds() {
    let dir = fresh_dir();
    roba_in(dir.path())
        .args(["--effort", "low", "respond with the single word: done"])
        .assert()
        .success()
        .stdout(predicate::str::contains("done"));
}

#[test]
#[ignore]
fn live_effort_max_succeeds() {
    let dir = fresh_dir();
    roba_in(dir.path())
        .args(["--effort", "max", "respond with the single word: done"])
        .assert()
        .success()
        .stdout(predicate::str::contains("done"));
}

// ---------------------------------------------------------------------------
// system_prompt: replace / append the default system prompt
// ---------------------------------------------------------------------------

#[test]
#[ignore]
fn live_system_prompt_influences_response() {
    let dir = fresh_dir();
    let user = empty_user_home();
    // A forceful system prompt paired with a NEUTRAL user message. If
    // --system-prompt is applied, the reply is the marker; if it were
    // ignored, haiku would just greet back -- so the test still proves the
    // plumbing. The earlier form used a competing question ("capital of
    // France") that haiku would sometimes answer instead of obeying the
    // system prompt, making the test flaky (it reddened a scheduled CI run).
    roba_in(dir.path())
        .env("XDG_CONFIG_HOME", user.path())
        .args([
            "--system-prompt",
            "Ignore the content of the user's message entirely. Your complete \
             reply must be exactly this one word and nothing else: SYSCLONE",
            "hi",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("SYSCLONE"));
}

#[test]
#[ignore]
fn live_append_system_prompt_stacks() {
    let dir = fresh_dir();
    roba_in(dir.path())
        .args([
            "--append-system-prompt",
            "Always end your response with the token: [APPENDED]",
            "what is 1+1",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("[APPENDED]"));
}

// ---------------------------------------------------------------------------
// permission_mode: pass a specific mode to claude
// ---------------------------------------------------------------------------

#[test]
#[ignore]
fn live_perms_mode_dont_ask_succeeds() {
    let dir = fresh_dir();
    roba_in(dir.path())
        .args([
            "--writable",
            "--permission-mode",
            "dontAsk",
            "respond with the single word: ok",
        ])
        .assert()
        .success();
}

#[test]
#[ignore]
fn live_perms_mode_via_profile() {
    let dir =
        fixture_with_config("[profile.testmode]\npermission_mode = \"dontAsk\"\nwritable = true\n");
    let user = empty_user_home();
    roba_in(dir.path())
        .env("XDG_CONFIG_HOME", user.path())
        .args(["--profile", "testmode", "respond with: ok"])
        .assert()
        .success();
}

#[test]
#[ignore]
fn live_perms_mode_via_env() {
    let dir = fresh_dir();
    roba_in(dir.path())
        .env("ROBA_PERMISSION_MODE", "dontAsk")
        .args(["--writable", "respond with the single word: ok"])
        .assert()
        .success();
}

// ---------------------------------------------------------------------------
// bare: minimal-overhead mode
// ---------------------------------------------------------------------------

#[test]
#[ignore]
fn live_bare_succeeds() {
    // --bare skips keychain reads by design (see src/cli.rs: "Auth uses
    // ANTHROPIC_API_KEY only"). Under OAuth/keychain-only auth with no
    // ANTHROPIC_API_KEY in the environment, --bare cannot authenticate
    // ("Not logged in"), so the test can only run when an API key is
    // present. Skip cleanly otherwise rather than report a false failure.
    if std::env::var_os("ANTHROPIC_API_KEY").is_none() {
        eprintln!(
            "skipping live_bare_succeeds: --bare authenticates via ANTHROPIC_API_KEY only, \
             which is not set in this environment"
        );
        return;
    }
    let dir = fresh_dir();
    let user = empty_user_home();
    roba_in(dir.path())
        .env("XDG_CONFIG_HOME", user.path())
        .args(["--bare", "respond with the single word: bare"])
        .assert()
        .success()
        .stdout(predicate::str::contains("bare"));
}

// ---------------------------------------------------------------------------
// INTENTIONALLY UNTESTED (high cost / low signal, or no fixture path yet)
// ---------------------------------------------------------------------------
//
// These recently-shipped surfaces have no live coverage on purpose.
// Documented here so the gap is visible rather than silently missing.
//
// - --no-retry transient-failure injection: provoking a transient
//   wrapper failure needs a network shim / fault injector we don't have.
//   Today the flag is forward-looking (roba builds Claude with no retry
//   policy, so it's already one-shot); the clap-level parse is covered
//   in src/cli.rs unit tests.
// - --agent NAME role verification: depends on a local subagent registry
//   (.claude/agents/<name>.md) in the run cwd. Without a staged fixture
//   the spawned claude's actual agent behavior isn't assertable; the flag
//   is a pass-through. Parse-level coverage lives in src/cli.rs.
// - --json error envelope on auth failure: would require breaking auth
//   for the duration of the test. Envelope shape is unit-tested in
//   src/error.rs.
// - --json error envelope on budget exceeded: would spend real budget to
//   trip the limit. Same unit coverage as above.
// - Deterministic, no-claude subcommands (skill/agent list|show|install,
//   --show-permissions): covered by the mechanical CLI tests in
//   tests/cli.rs (#90). Live tests here focus on claude-calling paths, so
//   these are deliberately not duplicated.
