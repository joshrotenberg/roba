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

mod common;
use common::{fresh_dir, roba_in};

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
    // `output::looks_like_refusal` to non-TTY consumers. We assert the wiring
    // on the RELIABLE side: a normal answer carries `refusal: false`.
    //
    // The `refusal: true` path is deliberately NOT live-tested. claude refuses
    // to roleplay a refusal -- asked to echo "I can't help with that" it
    // answers "I should respond authentically..." instead -- and engineering a
    // genuine policy refusal is both unreliable and generates harmful content.
    // The true-case detection (a refusal-marker prefix flips the flag) is
    // covered deterministically by the `looks_like_refusal` unit tests in
    // `src/output.rs` (refusal_detects_common_phrases, _is_case_insensitive,
    // _tolerates_leading_whitespace, _does_not_match_normal_answers).
    let dir = fresh_dir();
    let out = roba_in(dir.path())
        .args([
            "--json",
            "--quiet",
            "what is 2+2? answer with just the number.",
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
        Some(false),
        "a normal answer must not be flagged as a refusal, got: {stdout}"
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

#[test]
#[ignore]
fn live_session_id_assigns() {
    // --session-id assigns a caller-chosen UUID to the new session.
    // Assert the mechanics (the flag plumbs through and the returned
    // session id EQUALS what we supplied), not model behavior. A fixed
    // valid UUID keeps the run deterministic.
    let dir = fresh_dir();
    let chosen = "5f3c1a2b-4d6e-4f80-9a1b-2c3d4e5f6071";

    let out = roba_in(dir.path())
        .args([
            "--json",
            "--session-id",
            chosen,
            "respond with the single word: assigned",
        ])
        .output()
        .expect("session-id run");
    let parsed: serde_json::Value = serde_json::from_slice(&out.stdout).expect("json");
    let returned = parsed["result"]["session_id"].as_str().expect("session_id");

    assert_eq!(
        returned, chosen,
        "expected the returned session id to equal the supplied --session-id"
    );
}

#[test]
#[ignore]
fn live_show_roundtrips() {
    // Create a session, capture its id, then `roba show` it back and
    // assert the reconstructed envelope roundtrips: same session id, a
    // non-empty result. Mechanics, not model compliance.
    let dir = fresh_dir();
    let seed = roba_in(dir.path())
        .args(["--json", "respond with the single word: echo"])
        .output()
        .expect("run roba --json");
    assert!(seed.status.success(), "seed failed: {seed:?}");
    let seed_json: serde_json::Value =
        serde_json::from_slice(&seed.stdout).expect("seed stdout is JSON");
    let id = seed_json["result"]["session_id"]
        .as_str()
        .expect("session_id");

    // `show` finds the session by id across all projects, so it does not
    // need `-C`; use a plain binary invocation against the real $HOME.
    let out = Command::cargo_bin("roba")
        .expect("cargo-built roba binary")
        .args(["show", id, "--json"])
        .output()
        .expect("run roba show");
    assert!(out.status.success(), "show failed: {out:?}");
    let shown: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("show stdout is JSON");
    assert_eq!(
        shown["result"]["session_id"].as_str(),
        Some(id),
        "session id must roundtrip"
    );
    assert!(
        !shown["result"]["result"]
            .as_str()
            .unwrap_or_default()
            .is_empty(),
        "reconstructed result must be non-empty"
    );
}

#[test]
#[ignore]
fn live_show_wait_returns_completed_result() {
    // A completed session is already terminal on disk by the time the
    // seed call returns, so `show --wait` short-circuits its poll loop
    // and renders immediately. Mechanics, not model compliance: assert
    // the id roundtrips and the result is non-empty within the timeout.
    let dir = fresh_dir();
    let seed = roba_in(dir.path())
        .args(["--json", "respond with the single word: waited"])
        .output()
        .expect("run roba --json");
    assert!(seed.status.success(), "seed failed: {seed:?}");
    let seed_json: serde_json::Value =
        serde_json::from_slice(&seed.stdout).expect("seed stdout is JSON");
    let id = seed_json["result"]["session_id"]
        .as_str()
        .expect("session_id");

    let out = Command::cargo_bin("roba")
        .expect("cargo-built roba binary")
        .args(["show", id, "--wait", "--timeout", "60", "--json"])
        .output()
        .expect("run roba show --wait");
    assert!(out.status.success(), "show --wait failed: {out:?}");
    let shown: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("show stdout is JSON");
    assert_eq!(shown["result"]["session_id"].as_str(), Some(id));
    assert!(
        !shown["result"]["result"]
            .as_str()
            .unwrap_or_default()
            .is_empty(),
        "waited reconstructed result must be non-empty"
    );
}

#[test]
#[ignore]
fn live_json_schema_accepted() {
    // --json-schema constrains structured output. Assert MECHANICS only:
    // the flag is accepted, the run completes, and the --json envelope
    // parses. We do NOT assert the model's output is schema-valid -- that
    // is model compliance and flaky. roba reads the schema from a file
    // and inlines it; a tiny valid JSON Schema keeps the run cheap.
    let dir = fresh_dir();
    let schema_path = dir.path().join("schema.json");
    std::fs::write(
        &schema_path,
        r#"{"type":"object","properties":{"answer":{"type":"string"}},"required":["answer"]}"#,
    )
    .expect("write schema");

    let out = roba_in(dir.path())
        .args(["--json", "--json-schema"])
        .arg(&schema_path)
        .arg("respond with the single word: schema")
        .output()
        .expect("json-schema run");

    assert!(out.status.success(), "roba failed: {out:?}");
    let parsed: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("--json produced non-JSON stdout");
    assert_eq!(parsed["version"].as_u64(), Some(1));
    assert!(parsed.get("result").is_some());
}

#[test]
#[ignore]
fn live_json_schema_structured_output_clean() {
    // #317: with --json --json-schema, roba surfaces the structured answer
    // cleanly. Assert MECHANICS roba controls: `.result.result` is not a
    // raw fenced code block, and if `.result.structured_output` is present
    // it parses as a JSON value (object/array). We do NOT assert the model
    // obeyed the schema -- only that whatever it returned is delivered
    // un-fenced and addressable.
    let dir = fresh_dir();
    let schema_path = dir.path().join("schema.json");
    std::fs::write(
        &schema_path,
        r#"{"type":"object","properties":{"answer":{"type":"string"}},"required":["answer"]}"#,
    )
    .expect("write schema");

    let out = roba_in(dir.path())
        .args(["--json", "--json-schema"])
        .arg(&schema_path)
        .arg("capital of France?")
        .output()
        .expect("json-schema run");

    assert!(out.status.success(), "roba failed: {out:?}");
    let parsed: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("--json produced non-JSON stdout");
    let body = parsed["result"]["result"].as_str().unwrap_or_default();
    assert!(
        !body.trim_start().starts_with("```"),
        "result body should be unfenced, got: {body}"
    );
    if let Some(so) = parsed["result"].get("structured_output") {
        assert!(
            so.is_object() || so.is_array(),
            "structured_output should be a JSON value, got: {so}"
        );
    }
}

#[test]
#[ignore]
fn live_json_schema_default_render() {
    // --json-schema with NO --json: the validated answer lands in
    // structured_output, and roba's default path renders it as pretty JSON
    // on stdout so the `| jq` contract holds. claude may ALSO return a prose
    // `result` alongside the structured answer (it does as of 2.1.186);
    // schema mode prefers the structured object regardless.
    // Assert MECHANICS: stdout is non-empty and parses as a JSON object.
    let dir = fresh_dir();
    let schema_path = dir.path().join("schema.json");
    std::fs::write(
        &schema_path,
        r#"{"type":"object","properties":{"answer":{"type":"string"}},"required":["answer"]}"#,
    )
    .expect("write schema");

    let out = roba_in(dir.path())
        .arg("--json-schema")
        .arg(&schema_path)
        .arg("capital of France?")
        .output()
        .expect("json-schema default run");

    assert!(out.status.success(), "roba failed: {out:?}");
    assert!(
        !out.stdout.is_empty(),
        "default path printed nothing; structured_output was dropped"
    );
    let parsed: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("default path produced non-JSON stdout");
    assert!(
        parsed.get("answer").is_some(),
        "structured output missing `answer` key: {parsed}"
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

#[test]
#[ignore]
fn live_compose_stdin_with_prompt() {
    // Piped stdin + a positional prompt: the stdin merges in as a context
    // block instead of being dropped. Assert on roba's OWN echoed
    // resolved prompt (stderr, printed before the model call) -- this is
    // deterministic, not model compliance.
    //
    // Note: `--echo` is gated on `!quiet`, so `-q` would suppress it;
    // `--echo --plain` is the right combo for observing the resolved
    // prompt.
    let dir = fresh_dir();
    let out = roba_in(dir.path())
        .args(["--echo", "--plain", "what is the marker?"])
        .write_stdin("MARKER LINE 42")
        .output()
        .expect("run roba --echo with piped stdin");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("MARKER LINE 42"),
        "echoed prompt should contain the piped marker, got stderr: {stderr}"
    );
    assert!(
        stderr.contains("what is the marker?"),
        "echoed prompt should contain the positional question, got stderr: {stderr}"
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
fn live_system_prompt_succeeds() {
    let dir = fresh_dir();
    let user = empty_user_home();
    // SMOKE ONLY. We assert that a replacement `--system-prompt` reaches
    // claude and the call succeeds with non-empty output -- NOT that the
    // model obeys it. In `--print` mode haiku ignores a *replacement* system
    // prompt roughly half the time (it falls back to its default coding-agent
    // identity and just greets), so any marker-compliance assertion is ~50%
    // flaky regardless of wording. Earlier attempts to "fix" it by tweaking
    // the prompt only chased that coin-flip.
    //
    // The reliable end-to-end "system prompt influences output" signal is the
    // *additive* path, covered by `live_append_system_prompt_stacks` (haiku
    // reliably honors "always end with <token>"). Flag parsing is covered by
    // the mechanical tests in `tests/cli.rs`.
    roba_in(dir.path())
        .env("XDG_CONFIG_HOME", user.path())
        .args([
            "--system-prompt",
            "You are a helpful assistant.",
            "what is 1+1",
        ])
        .assert()
        .success()
        .stdout(predicate::str::is_empty().not());
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

#[test]
#[ignore]
fn live_safe_mode_succeeds() {
    // --safe-mode disables claude's customization surfaces (CLAUDE.md,
    // skills, plugins, hooks, MCP, custom commands/agents) but leaves auth
    // and permissions working normally -- so a plain prompt still answers.
    // Asserts the MECHANIC (the flag plumbs through and the run succeeds),
    // never model behavior. Unlike --bare, safe-mode does not change auth,
    // so no ANTHROPIC_API_KEY gate is needed.
    let dir = fresh_dir();
    let user = empty_user_home();
    roba_in(dir.path())
        .env("XDG_CONFIG_HOME", user.path())
        .args(["--safe-mode", "respond with the single word: safe"])
        .assert()
        .success()
        .stdout(predicate::str::contains("safe"));
}

// ---------------------------------------------------------------------------
// exit: typed exit codes for failure classes (claude-wrapper 0.11.1, #281)
// ---------------------------------------------------------------------------

#[test]
#[ignore]
fn live_exit_model_not_found_is_failure_not_auth() {
    // A bogus model 404s. claude-wrapper 0.11.1 (#633) no longer misclassifies
    // that as auth, so roba returns the generic failure code 1 -- NOT the
    // auth/usage code 2 (which an orchestrator reads as "halt the fleet").
    // Deterministic: an unknown model always 404s. Built without `roba_in`
    // because `--model` rejects multiple occurrences (its haiku default would
    // collide and produce a usage error, not the 404).
    let dir = fresh_dir();
    Command::cargo_bin("roba")
        .expect("cargo-built roba")
        .args([
            "-C",
            dir.path().to_str().expect("utf-8 path"),
            "--model",
            "totally-not-a-model-xyz",
            "hi",
        ])
        .assert()
        .code(1);
}

#[test]
#[ignore]
fn live_exit_max_turns_returns_5() {
    // A task that needs >=2 sequential turns (read a file, THEN report it)
    // cannot finish in one turn, so --max-turns 1 trips the cap. claude-wrapper
    // 0.12.0 surfaces that as MaxTurnsExceeded (detected via the --json result
    // event), which roba maps to the recoverable exit code 5 -- distinct from a
    // generic failure so an orchestrator can finish the lifecycle. (#309)
    let dir = fresh_dir();
    std::fs::write(dir.path().join("marker.txt"), "sentinel-42").expect("seed marker");
    roba_in(dir.path())
        .args([
            "--json",
            "--max-turns",
            "1",
            "Read the file marker.txt and report its exact contents.",
        ])
        .assert()
        .code(5);
}

#[test]
#[ignore]
fn live_exit_max_budget_returns_7() {
    // A near-zero --max-budget-usd cap (below even one turn's cost -- system
    // prompt cache creation alone exceeds a cent) trips claude's spend ceiling
    // after the first turn. claude-wrapper 0.12.3 detects that as
    // MaxBudgetExceeded (the error_max_budget_usd result subtype), which roba
    // maps to the recoverable exit code 7 -- distinct from a generic failure so
    // an orchestrator can finish the lifecycle, the same as 5 max-turns.
    // Detection is post-hoc, so the run overspends the cap before tripping; we
    // assert the mechanic roba owns (cap -> exit 7), never a dollar figure. (#388)
    let dir = fresh_dir();
    roba_in(dir.path())
        .args([
            "--max-budget-usd",
            "0.01",
            "Write a two-line poem about the sea.",
        ])
        .assert()
        .code(7);
}

#[test]
#[ignore]
fn live_exit_timeout_returns_4() {
    // A 1-second wall-clock deadline cannot survive process spawn + auth +
    // even one inference call, so the run is killed and roba exits the
    // timeout code (4) -- the rail for a headless claude that hangs. This
    // asserts the mechanism roba owns (kill-on-deadline -> exit 4), not any
    // model behavior. (#359)
    let dir = fresh_dir();
    roba_in(dir.path())
        .args(["--timeout", "1", "Write a 500-word essay about the ocean."])
        .assert()
        .code(4);
}

#[test]
#[ignore]
fn live_exit_bare_missing_key_is_auth() {
    // --bare authenticates via ANTHROPIC_API_KEY only. With the key removed,
    // claude-wrapper 0.11.1 (#633) surfaces the missing key as an auth failure,
    // so roba returns the auth/usage code 2 -- not the generic 1 it used to.
    let dir = fresh_dir();
    let user = empty_user_home();
    roba_in(dir.path())
        .env("XDG_CONFIG_HOME", user.path())
        .env_remove("ANTHROPIC_API_KEY")
        .args(["--bare", "hi"])
        .assert()
        .code(2);
}

// ---------------------------------------------------------------------------
// limits: unattended guardrails (--max-turns / --max-budget-usd)
// ---------------------------------------------------------------------------

#[test]
#[ignore]
fn live_limits_flags_accepted() {
    // Both guardrails plumb through and a normal short run completes.
    // We assert ONLY that the flags are accepted and the run succeeds --
    // not that a cap FIRES (turn-count / spend dependent = model-flaky).
    // The caps here (5 turns, $10) are generous enough that a trivial
    // one-shot answer never trips them.
    let dir = fresh_dir();
    roba_in(dir.path())
        .args([
            "--max-turns",
            "5",
            "--max-budget-usd",
            "10.0",
            "respond with the single word: bounded",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("bounded"));
}

// ---------------------------------------------------------------------------
// mcp: per-run MCP server config pass-through (--mcp-config)
// ---------------------------------------------------------------------------

#[test]
#[ignore]
fn live_mcp_config_accepted() {
    // --mcp-config plumbs through and a normal short run completes. Assert
    // MECHANICS only: the flag is accepted, no server starts, the run
    // succeeds. We use a MINIMAL config with NO servers ({"mcpServers":{}})
    // so nothing real is launched -- asserting actual MCP tools would need a
    // live server and would be flaky. roba forwards the path; claude reads it.
    let dir = fresh_dir();
    let cfg_path = dir.path().join("mcp.json");
    std::fs::write(&cfg_path, r#"{"mcpServers":{}}"#).expect("write mcp config");

    roba_in(dir.path())
        .arg("--mcp-config")
        .arg(&cfg_path)
        .arg("respond with the single word: mcp")
        .assert()
        .success()
        .stdout(predicate::str::contains("mcp"));
}

// ---------------------------------------------------------------------------
// med-tier pass-throughs (--add-dir / --fallback-model / --no-session-persistence)
// ---------------------------------------------------------------------------

#[test]
#[ignore]
fn live_medtier_flags_accepted() {
    // The three med-tier pass-throughs plumb through and a normal short run
    // completes. Assert MECHANICS only: all three flags are accepted and the
    // run succeeds. We do NOT assert that the fallback actually fires (needs
    // an overloaded primary) or that no JSONL was written (environment- and
    // timing-dependent) -- those are claude's behaviors, not roba's plumbing.
    // --add-dir points at a real temp dir; --fallback-model reuses the haiku
    // id the helpers default to; --no-session-persistence is a bare flag.
    let dir = fresh_dir();
    let extra = fresh_dir();

    roba_in(dir.path())
        .arg("--add-dir")
        .arg(extra.path())
        .args([
            "--fallback-model",
            "haiku",
            "--no-session-persistence",
            "respond with the single word: medtier",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("medtier"));
}

// ---------------------------------------------------------------------------
// alias draft (claude-assisted, parse-validated alias generation)
// ---------------------------------------------------------------------------

#[test]
#[ignore]
fn live_alias_draft() {
    // Assert MECHANICS: a draft exits 0 and its stdout parses through the
    // REAL `Alias` deserializer as exactly one `[alias.NAME]` block. We do
    // NOT assert anything about the generated NAME or wording -- that's
    // model behavior, not roba's plumbing. The draft call carries its own
    // `--model` (not roba_in's top-level one, which draft ignores).
    let dir = fresh_dir();
    let out = Command::cargo_bin("roba")
        .expect("cargo-built roba binary")
        .args([
            "-C",
            dir.path().to_str().expect("utf-8 tempdir path"),
            "alias",
            "draft",
            "a verb that asks for a one-line summary of a file given as the first argument",
            "--model",
            "claude-haiku-4-5",
        ])
        .output()
        .expect("run roba alias draft");
    assert!(
        out.status.success(),
        "draft should exit 0; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);

    #[derive(serde::Deserialize)]
    struct Wrapper {
        alias: std::collections::HashMap<String, roba::aliases::Alias>,
    }
    let parsed: Wrapper = toml::from_str(&stdout)
        .unwrap_or_else(|e| panic!("draft stdout did not parse as an alias: {e}\n{stdout}"));
    assert_eq!(
        parsed.alias.len(),
        1,
        "expected exactly one alias block on stdout, got:\n{stdout}"
    );
}

#[test]
#[ignore]
fn live_profile_draft() {
    // Profile twin of `live_alias_draft`. Assert MECHANICS: a draft exits 0
    // and its stdout parses through the REAL `Profile` deserializer
    // (`deny_unknown_fields`) as exactly one `[profile.NAME]` block. We do
    // NOT assert anything about the generated NAME or keys -- that's model
    // behavior, not roba's plumbing. The draft call carries its own
    // `--model` (not roba_in's top-level one, which draft ignores).
    let dir = fresh_dir();
    let out = Command::cargo_bin("roba")
        .expect("cargo-built roba binary")
        .args([
            "-C",
            dir.path().to_str().expect("utf-8 tempdir path"),
            "profile",
            "draft",
            "a cheap fast one-shot profile",
            "--model",
            "claude-haiku-4-5",
        ])
        .output()
        .expect("run roba profile draft");
    assert!(
        out.status.success(),
        "draft should exit 0; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);

    #[derive(serde::Deserialize)]
    struct Wrapper {
        profile: std::collections::HashMap<String, roba::profile::Profile>,
    }
    let parsed: Wrapper = toml::from_str(&stdout)
        .unwrap_or_else(|e| panic!("draft stdout did not parse as a profile: {e}\n{stdout}"));
    assert_eq!(
        parsed.profile.len(),
        1,
        "expected exactly one profile block on stdout, got:\n{stdout}"
    );
}

#[test]
#[ignore]
fn live_config_init() {
    // The per-project bootstrap. Assert the MECHANIC roba owns -- the
    // deterministic-bookend invariant -- WITHOUT asserting model
    // compliance: `config init` validates the drafted config through the
    // REAL per-file deserializer before exiting, so a clean exit implies a
    // parseable config and a non-clean exit is a graceful validator
    // rejection (not a crash). Either way roba's plumbing is sound; we do
    // NOT require the model to emit a valid config every run. The call
    // carries its own `--model` (config init ignores roba_in's top-level).
    let dir = fresh_dir();
    // A minimal but realistic project: a git marker so the config walk
    // stops here, a README, and a source file for the call to skim.
    std::fs::create_dir_all(dir.path().join(".git")).expect(".git marker");
    std::fs::write(
        dir.path().join("README.md"),
        "# widget\n\nA small Rust CLI for widgets.\n",
    )
    .expect("write README");
    std::fs::create_dir_all(dir.path().join("src")).expect("src dir");
    std::fs::write(
        dir.path().join("src/main.rs"),
        "fn main() { println!(\"widget\"); }\n",
    )
    .expect("write src");

    let out = Command::cargo_bin("roba")
        .expect("cargo-built roba binary")
        .args([
            "-C",
            dir.path().to_str().expect("utf-8 tempdir path"),
            "config",
            "init",
            "keep it minimal",
            "--model",
            "claude-haiku-4-5",
        ])
        .output()
        .expect("run roba config init");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    if out.status.success() {
        // Exit 0 must IMPLY a parseable config -- roba only succeeds after
        // its own deserializer accepted the draft. Validate through the
        // SAME deserializer the pool loader uses.
        roba::profile::pool::parse_config_str(&stdout).unwrap_or_else(|e| {
            panic!("config init exited 0 but stdout did not parse: {e:#}\n{stdout}")
        });
    } else {
        // A model slip (e.g. an alias-only key in a [profile.*] table) is
        // caught deterministically by roba's validator. That is roba
        // working CORRECTLY, not a roba bug -- assert the graceful
        // rejection rather than failing on model behavior.
        assert!(
            stderr.contains("drafted config did not parse"),
            "non-zero exit should be a clean validator rejection, not a crash; stderr:\n{stderr}"
        );
    }
}

// ---------------------------------------------------------------------------
// detach: fire-and-survive round-trip
// ---------------------------------------------------------------------------

#[test]
#[ignore]
fn live_detach_roundtrip() {
    // The #258 survivable-hand-off recipe, automated: `--detach` fires a
    // disowned run and prints the session handle; `show <id> --wait` then
    // re-attaches from a fresh process and renders the answer.
    //
    // `--detach` refuses a non-TTY stdin (piped input would be lost on the
    // detached child), and assert_cmd always pipes stdin -- so this test
    // drives the binary via std::process with stdin INHERITED. Run it from a
    // real terminal: `cargo test --test live -- --ignored live_detach_`.
    use std::process::{Command as StdCommand, Stdio};

    let dir = fresh_dir();
    let bin = assert_cmd::cargo::cargo_bin("roba");

    let out = StdCommand::new(&bin)
        .args([
            "-C",
            dir.path().to_str().expect("utf-8 tempdir path"),
            "--model",
            "haiku",
            "--detach",
            "respond with the single word: pong",
        ])
        .stdin(Stdio::inherit()) // a real TTY when run from a terminal
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("spawn roba --detach");

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "--detach should exit 0 after spawning (stdin must be a TTY); stderr:\n{stderr}"
    );
    let handle = String::from_utf8_lossy(&out.stdout).trim().to_string();
    // Handle is the ONLY thing on stdout, and is a v4-UUID-shaped line.
    assert_eq!(handle.len(), 36, "handle should be a UUID, got: {handle:?}");
    assert_eq!(
        handle.matches('-').count(),
        4,
        "handle should be a UUID, got: {handle:?}"
    );

    // Re-attach from a fresh process and wait for the detached run to finish.
    let show = Command::cargo_bin("roba")
        .expect("cargo-built roba binary")
        .args(["show", &handle, "--wait", "--timeout", "90"])
        .assert()
        .success();
    let rendered = String::from_utf8_lossy(&show.get_output().stdout);
    assert!(
        !rendered.trim().is_empty(),
        "show --wait should render a non-empty result for the detached run"
    );
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
