use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

use assert_cmd::Command as AssertCommand;
use predicates::prelude::*;
use serde_json::{Value, json};

fn roba() -> AssertCommand {
    AssertCommand::new(assert_cmd::cargo::cargo_bin!("roba"))
}

#[test]
fn root_help_is_a_concise_command_index() {
    let output = roba().arg("--help").output().unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.lines().count() < 70,
        "root help is too long:\n{stdout}"
    );
    for command in ["run", "serve", "config", "completions"] {
        assert!(stdout.contains(command), "missing {command}:\n{stdout}");
    }
    for removed in [
        "bundle", "history", "last", "profile", "cost", "doctor", "alias", "persona", "jobs",
        "watch", "worktree", "show",
    ] {
        assert!(!stdout.contains(removed), "stale {removed}:\n{stdout}");
    }
    assert!(!stdout.contains("\u{1b}["), "piped help leaked ANSI");
}

#[test]
fn no_args_shows_help_and_version_is_current() {
    roba()
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("Usage:"));
    roba()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains(env!("CARGO_PKG_VERSION")));
}

#[test]
fn run_and_serve_help_own_the_agent_option_reference() {
    roba().args(["run", "--help"]).assert().success().stdout(
        predicate::str::contains("--provider")
            .and(predicate::str::contains("--instruction"))
            .and(predicate::str::contains("--context"))
            .and(predicate::str::contains("--writable"))
            .and(predicate::str::contains("--resume"))
            .and(predicate::str::contains("<PROMPT>")),
    );
    roba().args(["serve", "--help"]).assert().success().stdout(
        predicate::str::contains("--provider")
            .and(predicate::str::contains("--writable"))
            .and(predicate::str::contains("stdout is MCP wire data"))
            .and(predicate::str::contains("<PROMPT>").not())
            .and(predicate::str::contains("--json").not()),
    );
}

#[test]
fn removed_cli_surfaces_fail_at_parse_time() {
    for args in [
        vec!["legacy prompt"],
        vec!["history"],
        vec!["doctor"],
        vec!["config", "show"],
        vec!["--profile", "worker", "run", "work"],
    ] {
        roba().args(args).assert().failure().code(2);
    }
}

#[test]
fn run_requires_a_prompt_and_closes_provider_values() {
    roba().arg("run").assert().failure().code(2);
    roba()
        .args(["run", "--provider", "unknown", "work"])
        .assert()
        .failure()
        .code(2);
    roba()
        .args(["run", "--read-only", "--writable", "work"])
        .assert()
        .failure()
        .code(2);
}

#[test]
fn global_cwd_errors_cleanly() {
    roba()
        .args([
            "-C",
            "/definitely/not/a/roba/workspace",
            "config",
            "effective",
        ])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("--cwd"));
}

fn project_config(contents: &str) -> tempfile::TempDir {
    let project = tempfile::tempdir().unwrap();
    std::fs::create_dir(project.path().join(".git")).unwrap();
    std::fs::write(project.path().join("roba.toml"), contents).unwrap();
    project
}

#[test]
fn config_effective_reports_safe_values_sources_and_provenance() {
    let project = project_config(
        "version = 1\n\
         [agent]\nprovider = 'codex'\neffort = 'medium'\ninstructions = ['project']\n\
         [execution]\npermissions = 'read_only'\ntimeout_secs = 30\n\
         [context]\nproject = ['fixture']\n\
         [extensions.git]\nenabled = false\n",
    );

    let output = roba()
        .args([
            "-C",
            project.path().to_str().unwrap(),
            "config",
            "effective",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["version"], 1);
    assert_eq!(value["result"]["agent"]["provider"], "codex");
    assert_eq!(value["result"]["execution"]["permissions"], "read_only");
    assert_eq!(value["result"]["sources"][0]["kind"], "project");
    assert_eq!(
        value["result"]["provenance"]["agent.provider"][0],
        std::fs::canonicalize(project.path().join("roba.toml"))
            .unwrap()
            .display()
            .to_string()
    );
}

#[test]
fn config_errors_fail_closed_in_the_requested_envelope() {
    let project = project_config("version = 1\n[agent]\nproivder = 'codex'\n");
    let output = roba()
        .args([
            "-C",
            project.path().to_str().unwrap(),
            "config",
            "effective",
            "--json",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let error: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["version"], 1);
    assert_eq!(error["error"]["kind"], "other");
    assert_eq!(error["error"]["exit_code"], 1);
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap()
            .contains("parsing")
    );
}

#[test]
fn unsupported_provider_policy_fails_before_launch_with_typed_json() {
    let output = roba()
        .args([
            "run",
            "--no-config",
            "--provider",
            "codex",
            "--max-turns",
            "1",
            "work",
            "--json",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let error: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["error"]["kind"], "other");
    assert_eq!(error["error"]["exit_code"], 1);
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap()
            .contains("max-turn")
    );
}

#[test]
fn completions_are_generated_from_the_retained_surface() {
    let output = roba().args(["completions", "bash"]).output().unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("roba"));
    assert!(stdout.contains("serve"));
    assert!(!stdout.contains("history"));
}

#[test]
fn stdio_serve_is_idle_wire_clean_and_exits_via_agent_shutdown() {
    let project = tempfile::tempdir().unwrap();
    let mut child = Command::new(assert_cmd::cargo::cargo_bin!("roba"))
        .args([
            "-C",
            project.path().to_str().unwrap(),
            "serve",
            "--no-config",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());

    write_frame(
        &mut input,
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": { "name": "roba-cli-test", "version": "1" }
            }
        }),
    );
    let initialized = read_response(&mut output, 1);
    assert_eq!(initialized["jsonrpc"], "2.0");
    assert_eq!(initialized["result"]["serverInfo"]["name"], "roba-agent");

    write_frame(
        &mut input,
        json!({"jsonrpc":"2.0","method":"notifications/initialized","params":{}}),
    );
    write_frame(
        &mut input,
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": { "name": "agent.shutdown", "arguments": {} }
        }),
    );
    let shutdown = read_response(&mut output, 2);
    assert_eq!(shutdown["jsonrpc"], "2.0");
    assert!(shutdown["result"]["structuredContent"].is_object());

    drop(input);
    let status = child.wait().unwrap();
    assert!(status.success());
    let mut stderr = String::new();
    child
        .stderr
        .take()
        .unwrap()
        .read_to_string(&mut stderr)
        .unwrap();
    assert!(stderr.is_empty(), "serve diagnostics leaked: {stderr}");
}

fn write_frame(input: &mut impl Write, value: Value) {
    writeln!(input, "{}", serde_json::to_string(&value).unwrap()).unwrap();
    input.flush().unwrap();
}

fn read_response(output: &mut impl BufRead, id: i64) -> Value {
    loop {
        let mut line = String::new();
        let bytes = output.read_line(&mut line).unwrap();
        assert!(bytes > 0, "server closed before response {id}");
        let value: Value = serde_json::from_str(line.trim_end()).unwrap();
        assert_eq!(value["jsonrpc"], "2.0", "non-wire stdout: {value}");
        if value["id"] == id {
            return value;
        }
    }
}

use std::io::Read as _;
