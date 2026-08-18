//! Mechanical integration tests -- exercise the roba binary surface
//! without calling a real provider. Covers: clap dispatch, conflict
//! matrix, exit codes, file-side error paths.
//!
//! For tests that invoke real providers, see `tests/live.rs`
//! (marked `#[ignore]`).

use assert_cmd::Command;
use clap::Parser;
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
        .stdout(predicate::str::contains("last"))
        .stdout(predicate::str::contains("bundle"));
}

#[test]
fn help_leads_with_the_provider_neutral_harness() {
    let output = roba().arg("-h").output().expect("render short help");
    assert!(output.status.success());
    assert!(output.stderr.is_empty());

    let stdout = String::from_utf8(output.stdout).expect("short help is UTF-8");
    assert!(stdout.contains("An MCP-native harness for one logical Claude or Codex agent."));
    assert!(stdout.contains("roba run --provider codex"));
    assert!(stdout.contains("mcp-repl -- roba serve"));
    assert!(stdout.contains("Legacy flag detail: `roba --help`"));
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
    assert!(stdout.contains("Unattended runs"));
    assert!(stdout.contains("Environment variables:"));
    assert!(stdout.contains("Legacy one-shot configuration (roba.toml):"));
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
fn bounded_run_help_exposes_the_finite_provider_surface() {
    let output = roba()
        .args(["run", "--help"])
        .output()
        .expect("render run help");
    assert!(output.status.success());
    assert!(output.stderr.is_empty());

    let stdout = String::from_utf8(output.stdout).expect("run help is UTF-8");
    for expected in [
        "<PROMPT>",
        "--provider",
        "--model",
        "--effort",
        "--instruction",
        "--context",
        "--git",
        "--writable",
        "--full-auto",
        "--resume",
        "--json",
    ] {
        assert!(
            stdout.contains(expected),
            "run help omitted {expected:?}:\n{stdout}"
        );
    }
    for parked in [
        "--config",
        "--agent",
        "--repl",
        "--mcp",
        "--max-workers",
        "--max-worker-depth",
    ] {
        assert!(
            !stdout.contains(parked),
            "run help still advertises parked option {parked:?}:\n{stdout}"
        );
    }
    assert!(stdout.contains("Each invocation admits one finite operation"));
    assert!(stdout.contains("roba run --instruction"));
}

fn inspectable_bundle_fixture() -> tempfile::TempDir {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    std::fs::write(root.join("unknown.txt"), "ignored").unwrap();
    std::fs::write(root.join("system-prompt.md"), "SECRET PROMPT").unwrap();
    std::fs::write(
        root.join("mcp.json"),
        r#"{"mcpServers":{"zeta":{"command":"SECRET MCP"},"alpha":{"command":"safe"}}}"#,
    )
    .unwrap();
    std::fs::write(
        root.join("settings.json"),
        r#"{"permissions":{"allow":["Read"]},"hooks":{"PreToolUse":[{"command":"SECRET HOOK"}]}}"#,
    )
    .unwrap();
    let agents = root.join("agents");
    std::fs::create_dir_all(&agents).unwrap();
    std::fs::write(
        agents.join("zeta.md"),
        "---\ndescription: Zeta agent\n---\nSECRET ZETA BODY",
    )
    .unwrap();
    std::fs::write(
        agents.join("alpha.md"),
        "---\ndescription: Alpha agent\ntools: Read\n---\nSECRET ALPHA BODY",
    )
    .unwrap();
    temp
}

#[test]
fn bundle_inspect_is_zero_provider_sorted_and_redacted() {
    let bundle = inspectable_bundle_fixture();
    let output = roba()
        .env("PATH", "")
        .args(["bundle", "inspect", bundle.path().to_str().unwrap()])
        .output()
        .expect("inspect bundle without provider PATH");
    assert!(
        output.status.success(),
        "inspection failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("mcp servers (2)"));
    assert!(stdout.contains("permission allow: 1 rule(s)"));
    assert!(stdout.contains("hook event: PreToolUse"));
    assert!(stdout.contains("unknown top-level entry: unknown.txt"));
    assert!(
        stdout.find("alpha — Alpha agent").unwrap() < stdout.find("zeta — Zeta agent").unwrap()
    );
    assert!(stdout.find("  alpha\n").unwrap() < stdout.find("  zeta\n").unwrap());
    for secret in [
        "SECRET PROMPT",
        "SECRET MCP",
        "SECRET HOOK",
        "SECRET ALPHA BODY",
        "SECRET ZETA BODY",
    ] {
        assert!(!stdout.contains(secret), "inspection leaked {secret:?}");
    }
}

#[test]
fn bundle_inspect_json_uses_versioned_inventory_and_structured_errors() {
    let bundle = inspectable_bundle_fixture();
    let output = roba()
        .env("PATH", "")
        .args([
            "bundle",
            "inspect",
            bundle.path().to_str().unwrap(),
            "--json",
        ])
        .output()
        .expect("inspect bundle as JSON");
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let envelope: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(envelope["version"], roba_types::VERSION);
    assert_eq!(
        envelope["result"]["root"],
        bundle.path().display().to_string()
    );
    assert_eq!(envelope["result"]["agents"][0]["name"], "alpha");
    assert_eq!(
        envelope["result"]["mcp_servers"],
        serde_json::json!(["alpha", "zeta"])
    );
    assert_eq!(
        envelope["result"]["settings"]["permission_rule_counts"]["allow"],
        1
    );
    assert_eq!(
        envelope["result"]["settings"]["hook_events"],
        serde_json::json!(["PreToolUse"])
    );
    let serialized = String::from_utf8(output.stdout).unwrap();
    assert!(!serialized.contains("SECRET"));

    let missing = bundle.path().join("missing");
    let output = roba()
        .args(["bundle", "inspect", missing.to_str().unwrap(), "--json"])
        .output()
        .expect("inspect missing bundle as JSON");
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let error: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["version"], roba_types::VERSION);
    assert_eq!(error["error"]["exit_code"], 1);
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap()
            .contains("does not exist")
    );

    let malformed = tempfile::tempdir().unwrap();
    let secret = "SUPER_SECRET_BUNDLE_CONFIG";
    std::fs::write(
        malformed.path().join("roba.toml"),
        format!("unknown = \"{secret}\"\n"),
    )
    .unwrap();
    let output = roba()
        .args([
            "bundle",
            "inspect",
            malformed.path().to_str().unwrap(),
            "--json",
        ])
        .output()
        .expect("inspect malformed bundle as JSON");
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr).unwrap();
    let error: serde_json::Value = serde_json::from_str(&stderr).unwrap();
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap()
            .contains("source details redacted")
    );
    assert!(!stderr.contains(secret));
}

#[test]
fn explicit_missing_bundle_refuses_before_provider_resolution() {
    let temp = tempfile::tempdir().unwrap();
    let missing = temp.path().join("missing");
    roba()
        .env("PATH", "")
        .args(["--bundle", missing.to_str().unwrap(), "hello"])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("bundle").and(predicate::str::contains("does not exist")));
}

#[test]
fn one_shot_bundle_uses_the_same_mcp_validation_before_provider_resolution() {
    let bundle = tempfile::tempdir().unwrap();
    std::fs::write(bundle.path().join("mcp.json"), r#"{"mcpServers":[]}"#).unwrap();
    roba()
        .env("PATH", "")
        .args(["--bundle", bundle.path().to_str().unwrap(), "hello"])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains(
            "field `mcpServers` must be a JSON object",
        ));
}

#[test]
fn bounded_run_parses_provider_permissions_resume_and_prompt() {
    let parsed = roba::cli::Cli::try_parse_from([
        "roba",
        "run",
        "--provider",
        "codex",
        "--writable",
        "--resume",
        "thread-123",
        "finish the migration",
    ])
    .expect("the direct run surface should parse");

    let Some(roba::cli::SubCommand::Run(args)) = parsed.command else {
        panic!("expected the run subcommand");
    };
    assert_eq!(args.provider, Some(roba::cli::RunProvider::Codex));
    assert!(args.writable);
    assert!(!args.full_auto);
    assert_eq!(args.resume.as_deref(), Some("thread-123"));
    assert_eq!(args.prompt, "finish the migration");
}

#[test]
fn bounded_run_requires_a_prompt_at_parse_time() {
    roba()
        .args(["run", "--provider", "codex"])
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("<PROMPT>"))
        .stderr(predicate::str::contains("required"));
}

#[test]
fn bounded_run_provider_is_a_closed_value_enum() {
    roba()
        .args(["run", "--provider", "unknown", "hello"])
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("invalid value 'unknown'"))
        .stderr(predicate::str::contains("claude"))
        .stderr(predicate::str::contains("codex"));
}

#[test]
fn bounded_run_permission_modes_are_mutually_exclusive() {
    roba()
        .args(["run", "--writable", "--full-auto", "hello"])
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("cannot be used with"));
}

#[test]
fn bounded_run_rejects_legacy_options_before_the_subcommand() {
    roba()
        .args(["--model", "legacy-model", "run", "hello"])
        .env("PATH", "")
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains(
            "legacy one-shot options cannot be placed before `run`",
        ));

    let output = roba()
        .args(["--json", "run", "hello"])
        .env("PATH", "")
        .output()
        .expect("reject misplaced legacy JSON option");
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let error: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap()
            .contains("legacy one-shot options cannot be placed before `run`")
    );
}

#[test]
fn bounded_run_rejects_parked_and_removed_options() {
    for args in [
        &["run", "--config", "run.toml", "hello"][..],
        &["run", "--agent", "builder", "hello"][..],
        &["run", "--repl", "hello"][..],
        &["run", "--mcp", "hello"][..],
        &["run", "--max-workers", "2", "hello"][..],
        &["run", "--max-worker-depth", "1", "hello"][..],
    ] {
        roba()
            .args(args)
            .assert()
            .failure()
            .code(2)
            .stderr(predicate::str::contains("unexpected argument"));
    }
}

#[test]
fn codex_unsupported_limit_refuses_before_cli_launch() {
    roba()
        .args(["run", "--provider", "codex", "--max-turns", "1", "hello"])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains(
            "Codex provider does not support a max-turn ceiling",
        ));
}

#[test]
fn bounded_run_invalid_host_configuration_fails_before_a_turn() {
    for (args, expected) in [
        (
            vec!["run", "--max-cost-usd=-1", "--json", "hello"],
            "maximum cost must be a finite non-negative number",
        ),
        (
            vec!["run", "--resume", "", "--json", "hello"],
            "seeded session id must not be empty",
        ),
    ] {
        let output = roba()
            .args(args)
            .output()
            .expect("reject invalid host input");
        assert_eq!(output.status.code(), Some(roba_types::EXIT_FAILURE));
        assert!(output.stdout.is_empty());
        let error: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
        assert_eq!(error["error"]["exit_code"], roba_types::EXIT_FAILURE);
        assert!(
            error["error"]["message"]
                .as_str()
                .unwrap()
                .contains(expected)
        );
    }
}

#[test]
fn serve_help_exposes_only_the_promptless_provider_neutral_surface() {
    let output = roba()
        .args(["serve", "--help"])
        .output()
        .expect("render serve help");
    assert!(output.status.success());
    assert!(output.stderr.is_empty());

    let stdout = String::from_utf8(output.stdout).expect("serve help is UTF-8");
    for expected in [
        "--provider",
        "--model",
        "--effort",
        "--instruction",
        "--context",
        "--writable",
        "--full-auto",
        "--max-turns",
        "--max-cost-usd",
        "--timeout",
        "--resume",
    ] {
        assert!(
            stdout.contains(expected),
            "serve help omitted {expected:?}:\n{stdout}"
        );
    }
    for absent in [
        "<PROMPT>",
        "--json",
        "--config",
        "--agent",
        "--repl",
        "--mcp",
        "--max-workers",
        "--max-worker-depth",
    ] {
        assert!(
            !stdout.contains(absent),
            "serve help advertised out-of-scope option {absent:?}:\n{stdout}"
        );
    }
    assert!(stdout.contains("mcp-repl --protocol final -- roba serve"));
    assert!(stdout.contains("at most one active operation"));
    assert!(stdout.contains("agent.interrupt keeps the host available"));
    assert!(stdout.contains("stdout is MCP wire data"));
}

#[test]
fn serve_rejects_run_only_legacy_and_parked_options() {
    for args in [
        &["serve", "--json"][..],
        &["serve", "--config", "run.toml"][..],
        &["serve", "--agent", "builder"][..],
        &["serve", "--repl"][..],
        &["serve", "--mcp"][..],
        &["serve", "--max-workers", "2"][..],
        &["serve", "--max-worker-depth", "1"][..],
    ] {
        roba()
            .args(args)
            .assert()
            .failure()
            .code(2)
            .stderr(predicate::str::contains("unexpected argument"));
    }

    roba()
        .args(["--model", "legacy-model", "serve"])
        .assert()
        .failure()
        .code(1)
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains(
            "legacy one-shot options cannot be placed before `serve`",
        ));
}

const STABLE_MCP_PROTOCOL: &str = "2025-11-25";
const FINAL_MCP_PROTOCOL: &str = "2026-07-28";
const SERVE_PROCESS_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

#[derive(Clone, Copy, Debug)]
enum CliWireProtocol {
    Stable,
    Final,
}

struct ServeProcess {
    child: std::process::Child,
    stdin: Option<std::process::ChildStdin>,
    frames: std::sync::mpsc::Receiver<std::io::Result<String>>,
    reader: Option<std::thread::JoinHandle<()>>,
}

impl ServeProcess {
    fn spawn(mut command: std::process::Command) -> Self {
        use std::process::Stdio;

        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn stdio serve process");
        let stdin = child.stdin.take().expect("serve stdin is piped");
        let stdout = child.stdout.take().expect("serve stdout is piped");
        let (sender, frames) = std::sync::mpsc::channel();
        let reader = std::thread::spawn(move || {
            use std::io::BufRead;

            for line in std::io::BufReader::new(stdout).lines() {
                if sender.send(line).is_err() {
                    break;
                }
            }
        });
        Self {
            child,
            stdin: Some(stdin),
            frames,
            reader: Some(reader),
        }
    }

    #[cfg(unix)]
    fn id(&self) -> u32 {
        self.child.id()
    }

    fn send(&mut self, frame: serde_json::Value) {
        use std::io::Write;

        let stdin = self.stdin.as_mut().expect("serve stdin remains open");
        serde_json::to_writer(&mut *stdin, &frame).expect("MCP request serializes");
        stdin.write_all(b"\n").expect("MCP frame writes");
        stdin.flush().expect("MCP frame flushes");
    }

    fn receive(&self) -> serde_json::Value {
        let line = self
            .frames
            .recv_timeout(SERVE_PROCESS_TIMEOUT)
            .expect("serve produced a response before the timeout")
            .expect("serve stdout remained readable");
        parse_wire_frame(line)
    }

    fn assert_idle(&mut self) {
        assert!(
            matches!(
                self.frames
                    .recv_timeout(std::time::Duration::from_millis(200)),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout)
            ),
            "promptless serve emitted stdout before receiving an MCP request"
        );
        assert!(
            self.child.try_wait().expect("query serve child").is_none(),
            "promptless serve exited instead of remaining idle"
        );
    }

    #[cfg(unix)]
    fn close_input(&mut self) {
        self.stdin.take();
    }

    fn wait_and_collect(&mut self) -> Vec<serde_json::Value> {
        use std::io::Read;

        let deadline = std::time::Instant::now() + SERVE_PROCESS_TIMEOUT;
        let status = loop {
            if let Some(status) = self.child.try_wait().expect("query serve child") {
                break status;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "serve did not exit before the process timeout"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        };
        assert!(status.success(), "serve exited with {status}");

        let mut stderr = String::new();
        self.child
            .stderr
            .take()
            .expect("serve stderr is piped")
            .read_to_string(&mut stderr)
            .expect("read serve stderr");
        assert!(
            stderr.is_empty(),
            "serve leaked metadata to stderr: {stderr}"
        );

        self.reader
            .take()
            .expect("stdout reader exists")
            .join()
            .expect("stdout reader did not panic");
        self.frames
            .try_iter()
            .map(|line| {
                parse_wire_frame(line.expect("serve stdout remained readable until process exit"))
            })
            .collect()
    }
}

impl Drop for ServeProcess {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

fn serve_command() -> std::process::Command {
    let mut command = std::process::Command::new(assert_cmd::cargo::cargo_bin!("roba"));
    command.arg("serve");
    command
}

fn parse_wire_frame(line: String) -> serde_json::Value {
    let frame: serde_json::Value = serde_json::from_str(&line)
        .unwrap_or_else(|error| panic!("stdout was not JSON: {error}: {line:?}"));
    assert_eq!(frame["jsonrpc"], "2.0", "stdout was not JSON-RPC: {frame}");
    frame
}

fn final_client_meta() -> serde_json::Value {
    serde_json::json!({
        "io.modelcontextprotocol/protocolVersion": FINAL_MCP_PROTOCOL,
        "io.modelcontextprotocol/clientInfo": {
            "name": "roba-cli-test",
            "version": "0"
        },
        "io.modelcontextprotocol/clientCapabilities": {}
    })
}

fn handshake_serve(server: &mut ServeProcess, protocol: CliWireProtocol) {
    match protocol {
        CliWireProtocol::Stable => {
            server.send(serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": STABLE_MCP_PROTOCOL,
                    "capabilities": {},
                    "clientInfo": {"name": "roba-cli-test", "version": "0"}
                }
            }));
            let response = server.receive();
            assert_eq!(response["id"], 1);
            assert_eq!(response["result"]["protocolVersion"], STABLE_MCP_PROTOCOL);
            assert_eq!(response["result"]["serverInfo"]["name"], "roba-agent");
            server.send(serde_json::json!({
                "jsonrpc": "2.0",
                "method": "notifications/initialized"
            }));
        }
        CliWireProtocol::Final => {
            server.send(serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "server/discover",
                "params": {"_meta": final_client_meta()}
            }));
            let response = server.receive();
            assert_eq!(response["id"], 1);
            assert_eq!(
                response["result"]["_meta"]["io.modelcontextprotocol/serverInfo"]["name"],
                "roba-agent"
            );
            assert!(
                response["result"]["supportedVersions"]
                    .as_array()
                    .expect("discovery publishes versions")
                    .contains(&serde_json::json!(FINAL_MCP_PROTOCOL))
            );
        }
    }
}

fn send_shutdown(server: &mut ServeProcess, protocol: CliWireProtocol, id: u64) {
    let mut params = serde_json::json!({"name": "agent.shutdown", "arguments": {}});
    if matches!(protocol, CliWireProtocol::Final) {
        params
            .as_object_mut()
            .expect("tool params are an object")
            .insert("_meta".to_owned(), final_client_meta());
    }
    server.send(serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "tools/call",
        "params": params
    }));
}

#[test]
fn serve_is_idle_and_wire_only_for_stable_and_final_clients_until_logical_shutdown() {
    for protocol in [CliWireProtocol::Stable, CliWireProtocol::Final] {
        let mut server = ServeProcess::spawn(serve_command());
        server.assert_idle();
        handshake_serve(&mut server, protocol);

        #[cfg(unix)]
        {
            send_unix_signal(server.id(), "INT");
            std::thread::sleep(std::time::Duration::from_millis(100));
            assert!(
                server
                    .child
                    .try_wait()
                    .expect("query serve child")
                    .is_none(),
                "piped SIGINT stopped a {protocol:?} MCP server"
            );
        }

        send_shutdown(&mut server, protocol, 2);
        let response = server.receive();
        assert_eq!(response["id"], 2);
        assert_eq!(response["result"]["structuredContent"]["status"], "stopped");
        assert!(
            server.wait_and_collect().is_empty(),
            "logical shutdown emitted unexpected trailing stdout"
        );
    }
}

#[test]
fn serve_git_extension_is_opt_in_and_scoped_to_the_effective_cwd() {
    let repository = tempfile::tempdir().expect("Git fixture");
    let init = std::process::Command::new("git")
        .args(["init", "--quiet"])
        .current_dir(repository.path())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("initialize Git fixture");
    assert!(
        init.status.success(),
        "git init failed: {}",
        String::from_utf8_lossy(&init.stderr)
    );
    std::fs::write(repository.path().join("untracked.txt"), "fixture\n")
        .expect("write Git fixture");

    let mut command = serve_command();
    command.arg("--git").current_dir(repository.path());
    let mut server = ServeProcess::spawn(command);
    handshake_serve(&mut server, CliWireProtocol::Stable);

    server.send(serde_json::json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/list",
        "params": {}
    }));
    let tools = server.receive();
    let names = tools["result"]["tools"]
        .as_array()
        .expect("tools/list returned tools")
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect::<Vec<_>>();
    assert!(names.contains(&"git.snapshot"));
    assert!(
        !names.contains(&"git.stage_all"),
        "read-only serve exposed the mutating Git workflow"
    );

    server.send(serde_json::json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "resources/read",
        "params": {"uri": "roba://git/workspace"}
    }));
    let resource = server.receive();
    let text = resource["result"]["contents"][0]["text"]
        .as_str()
        .unwrap_or_else(|| panic!("Git workspace resource returned JSON text: {resource:#}"));
    let snapshot: serde_json::Value = serde_json::from_str(text).expect("typed Git snapshot");
    assert_eq!(
        std::path::Path::new(snapshot["repository_root"].as_str().unwrap()),
        std::fs::canonicalize(repository.path()).unwrap()
    );
    assert_eq!(snapshot["untracked"], serde_json::json!(["untracked.txt"]));

    send_shutdown(&mut server, CliWireProtocol::Stable, 4);
    let response = server.receive();
    assert_eq!(response["id"], 4);
    assert_eq!(response["result"]["structuredContent"]["status"], "stopped");
    assert!(server.wait_and_collect().is_empty());
}

#[cfg(unix)]
#[test]
fn serve_eof_and_sigterm_cancel_and_reap_a_held_provider_child() {
    for stop in ["EOF", "SIGTERM"] {
        let bin = fake_claude_streaming_hold();
        let home = tempfile::tempdir().expect("home");
        let cfg = tempfile::tempdir().expect("cfg");
        let provider_pid_path = home.path().join(format!("provider-{stop}.pid"));

        let mut command = serve_command();
        command
            .env("PATH", format!("{}:/usr/bin:/bin", bin.path().display()))
            .env("HOME", home.path())
            .env("XDG_CONFIG_HOME", cfg.path())
            .env("ROBA_PROVIDER_PID", &provider_pid_path)
            .current_dir(home.path());
        let mut server = ServeProcess::spawn(command);
        handshake_serve(&mut server, CliWireProtocol::Stable);
        server.send(serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {"name": "agent.turn", "arguments": {"text": "hold"}}
        }));

        let provider_pid = wait_for_pid_file(&provider_pid_path);
        match stop {
            "EOF" => server.close_input(),
            "SIGTERM" => send_unix_signal(server.id(), "TERM"),
            _ => unreachable!(),
        }

        let trailing = server.wait_and_collect();
        assert!(
            trailing.iter().any(|frame| {
                frame["id"] == 2 && frame["result"]["structuredContent"]["status"] == "cancelled"
            }),
            "{stop} did not settle the held turn on the MCP wire: {trailing:?}"
        );
        assert!(
            wait_for_process_exit(provider_pid),
            "{stop} left provider child {provider_pid} alive"
        );
    }
}

#[cfg(unix)]
fn send_unix_signal(pid: u32, signal: &str) {
    let status = std::process::Command::new("kill")
        .args([format!("-{signal}"), pid.to_string()])
        .status()
        .expect("invoke kill");
    assert!(status.success(), "failed to send SIG{signal} to {pid}");
}

#[cfg(unix)]
fn wait_for_pid_file(path: &std::path::Path) -> u32 {
    let deadline = std::time::Instant::now() + SERVE_PROCESS_TIMEOUT;
    loop {
        if let Ok(contents) = std::fs::read_to_string(path)
            && let Ok(pid) = contents.trim().parse()
        {
            return pid;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "provider did not publish its PID at {}",
            path.display()
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[cfg(unix)]
fn wait_for_process_exit(pid: u32) -> bool {
    use std::process::Stdio;

    let deadline = std::time::Instant::now() + SERVE_PROCESS_TIMEOUT;
    loop {
        let alive = std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success());
        if !alive {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
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
fn config_lint_shadowing_alias_exits_1_with_finding() {
    // A pool with a built-in-shadowing alias. lint reports it on stdout
    // (the verb's output IS the report) and exits 1. XDG_CONFIG_HOME is
    // isolated to an empty dir so the real user config can't leak in.
    let project = make_dir_with_files(&[
        (".git/HEAD", ""),
        ("roba.toml", "[alias.cost]\ntemplate = \"x ${@}\"\n"),
    ]);
    let user_home = tempfile::tempdir().expect("user home");
    roba()
        .args(["-C", project.path().to_str().unwrap(), "config", "lint"])
        .env("XDG_CONFIG_HOME", user_home.path())
        .assert()
        .code(1)
        .stdout(predicate::str::contains("builtin-shadow"))
        .stdout(predicate::str::contains("cost"));
}

#[test]
fn config_lint_clean_config_exits_0() {
    let project = make_dir_with_files(&[
        (".git/HEAD", ""),
        (
            "roba.toml",
            "readonly = true\n\n[profile.review]\ngit_diff = true\n",
        ),
    ]);
    let user_home = tempfile::tempdir().expect("user home");
    roba()
        .args(["-C", project.path().to_str().unwrap(), "config", "lint"])
        .env("XDG_CONFIG_HOME", user_home.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("no issues found"));
}

#[test]
fn bare_word_subcommand_typo_errors_without_prompting() {
    // `roba worktrees` (a one-edit typo of `worktree`) must bail with a
    // suggestion instead of silently becoming a prompt + calling claude
    // (#353). Isolated config (empty `.git` project + empty
    // XDG_CONFIG_HOME) means no alias named `worktrees` exists. PATH is
    // emptied as belt-and-suspenders: the guard bails before run_ask, so
    // claude is never looked up -- a regression that fell through to a
    // prompt would surface a different ("not found on PATH") error and
    // fail these assertions.
    let project = make_dir_with_files(&[(".git/HEAD", "")]);
    let user_home = tempfile::tempdir().expect("user home");
    roba()
        .args(["-C", project.path().to_str().unwrap(), "worktrees"])
        .env("XDG_CONFIG_HOME", user_home.path())
        .env("PATH", "")
        .assert()
        .failure()
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains("is not a roba command"))
        .stderr(predicate::str::contains("did you mean `worktree`"))
        .stderr(predicate::str::contains("roba -p"));
}

#[test]
fn config_lint_json_emits_versioned_envelope() {
    // --json: the uniform { version: 1, result: { findings, ok } } envelope
    // on stdout, even when findings exist (exit 1).
    let project = make_dir_with_files(&[
        (".git/HEAD", ""),
        ("roba.toml", "[alias.show]\ntemplate = \"x ${@}\"\n"),
    ]);
    let user_home = tempfile::tempdir().expect("user home");
    let assert = roba()
        .args([
            "-C",
            project.path().to_str().unwrap(),
            "config",
            "lint",
            "--json",
        ])
        .env("XDG_CONFIG_HOME", user_home.path())
        .assert()
        .code(1);
    let out = &assert.get_output().stdout;
    let json: serde_json::Value = serde_json::from_slice(out).expect("valid JSON");
    assert_eq!(json["version"], 1, "top-level version must be 1");
    assert_eq!(json["result"]["ok"], false, "got: {json}");
    let findings = json["result"]["findings"]
        .as_array()
        .expect("findings is an array");
    assert_eq!(findings.len(), 1, "got: {json}");
    assert_eq!(findings[0]["rule"], "builtin-shadow");
}

#[test]
fn config_lint_missing_path_errors() {
    // A single named PATH that doesn't exist is a clean error (exit 1),
    // not a panic.
    roba()
        .args(["config", "lint", "/no/such/roba.toml"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("no such config file"));
}

#[test]
fn config_show_prints_merged_pool_to_stdout() {
    // The merged top-level keys + every [profile.NAME] land on STDOUT
    // (byte-clean, pipeable), while the active-profile + sources header
    // is METADATA on STDERR. XDG_CONFIG_HOME is isolated so the real user
    // config can't leak in.
    let project = make_dir_with_files(&[
        (".git/HEAD", ""),
        (
            "roba.toml",
            "readonly = true\n\n\
             [profile.default]\ngit_diff = true\n\n\
             [profile.review]\nwritable = true\n",
        ),
    ]);
    let user_home = tempfile::tempdir().expect("user home");
    roba()
        .args(["-C", project.path().to_str().unwrap(), "config", "show"])
        .env("XDG_CONFIG_HOME", user_home.path())
        .assert()
        .success()
        // The merged body on stdout.
        .stdout(predicate::str::contains("readonly = true"))
        .stdout(predicate::str::contains("[profile.default]"))
        .stdout(predicate::str::contains("[profile.review]"))
        // The header must NOT leak into stdout.
        .stdout(predicate::str::contains("active profile:").not())
        // The header IS on stderr (a `default` profile auto-applies).
        .stderr(predicate::str::contains(
            "active profile: default (auto-applied)",
        ))
        .stderr(predicate::str::contains("sources:"));
}

#[test]
fn config_show_json_emits_versioned_envelope() {
    // --json: the uniform { version: 1, result } envelope on stdout, with
    // the merged profile names under result.profiles. stdout is byte-clean
    // JSON (no header leakage).
    let project = make_dir_with_files(&[
        (".git/HEAD", ""),
        (
            "roba.toml",
            "readonly = true\n\n[profile.review]\nwritable = true\n",
        ),
    ]);
    let user_home = tempfile::tempdir().expect("user home");
    let assert = roba()
        .args([
            "-C",
            project.path().to_str().unwrap(),
            "config",
            "show",
            "--json",
        ])
        .env("XDG_CONFIG_HOME", user_home.path())
        .assert()
        .success();
    let out = &assert.get_output().stdout;
    let json: serde_json::Value = serde_json::from_slice(out).expect("valid JSON");
    assert_eq!(json["version"], 1, "top-level version must be 1");
    assert_eq!(json["result"]["defaults"]["readonly"], true, "got: {json}");
    assert!(
        json["result"]["profiles"]["review"].is_object(),
        "got: {json}"
    );
}

#[test]
fn config_explain_renders_human_layout() {
    // The human view: grouped sections, the auto-applied profile named, a
    // top-level unsafe setting flagged, and an alias invocation form -- all on
    // stdout (explain is a stdout-only human view). --plain keeps it
    // byte-clean so the assertions are stable.
    let project = make_dir_with_files(&[
        (".git/HEAD", ""),
        (
            "roba.toml",
            "readonly = true\n\n\
             [profile.default]\nfull_auto = true\n\n\
             [alias.review]\ndescription = \"review a PR\"\nargs = [\"pr\"]\ntemplate = \"review ${pr}\"\n",
        ),
    ]);
    let user_home = tempfile::tempdir().expect("user home");
    roba()
        .args([
            "-C",
            project.path().to_str().unwrap(),
            "config",
            "explain",
            "--plain",
        ])
        .env("XDG_CONFIG_HOME", user_home.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("Active profile"))
        .stdout(predicate::str::contains("default (auto-applied)"))
        .stdout(predicate::str::contains("Top-level defaults (always on)"))
        .stdout(predicate::str::contains("readonly = true"))
        .stdout(predicate::str::contains(
            "Profiles (opt in with --profile NAME)",
        ))
        // The [profile.default] unsafe setting is flagged.
        .stdout(predicate::str::contains("[!] unsafe:"))
        // The alias header carries the usage hint; the per-line `roba ` prefix
        // is dropped (it lives in the header now).
        .stdout(predicate::str::contains("Aliases (verbs, run with roba"))
        .stdout(predicate::str::contains("review <pr>"))
        .stdout(predicate::str::contains("roba review <pr>").not())
        .stdout(predicate::str::contains("Sources (closest-to-cwd wins)"))
        // --plain leaks no ANSI escape.
        .stdout(predicate::str::contains("\x1b").not());
}

#[test]
fn config_show_sources_attributes_each_key_to_its_winning_layer() {
    // EFFECTIVE view: a user file sets full_auto, a project file overrides
    // model (closer wins over the farther user file), and the auto-applied
    // [profile.default] sets max_turns. Each line is attributed to the
    // layer that won it.
    let user_home = tempfile::tempdir().expect("user home");
    std::fs::write(
        user_home.path().join("roba.toml"),
        "full_auto = true\nmodel = \"sonnet\"\n",
    )
    .expect("write user config");
    let project = make_dir_with_files(&[
        (".git/HEAD", ""),
        (
            "roba.toml",
            "model = \"opus\"\n\n[profile.default]\nmax_turns = 80\n",
        ),
    ]);
    let assert = roba()
        .args([
            "-C",
            project.path().to_str().unwrap(),
            "config",
            "show",
            "--sources",
        ])
        .env("XDG_CONFIG_HOME", user_home.path())
        .env_remove("ROBA_PROFILE")
        .env_remove("ROBA_MODEL")
        .env_remove("ROBA_FULL_AUTO")
        .env_remove("ROBA_MAX_TURNS")
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout);

    // full_auto: only the farther user file set it.
    let full_auto = stdout
        .lines()
        .find(|l| l.starts_with("full_auto ="))
        .unwrap_or_else(|| panic!("no full_auto line in:\n{stdout}"));
    assert!(full_auto.contains("full_auto = true"), "{full_auto}");
    assert!(
        full_auto.contains(&user_home.path().display().to_string()),
        "full_auto should attribute to the user file: {full_auto}"
    );

    // model: the closer project file wins over the farther user file.
    let model = stdout
        .lines()
        .find(|l| l.starts_with("model ="))
        .unwrap_or_else(|| panic!("no model line in:\n{stdout}"));
    assert!(model.contains("model = \"opus\""), "{model}");
    assert!(
        model.contains(&project.path().display().to_string()),
        "model should attribute to the closer project file: {model}"
    );

    // max_turns: from the auto-applied profile.
    assert!(
        stdout.contains("max_turns = 80  # [profile.default]"),
        "got:\n{stdout}"
    );
}

#[test]
fn config_show_sources_single_key_prints_only_that_key() {
    let user_home = tempfile::tempdir().expect("user home");
    let project = make_dir_with_files(&[
        (".git/HEAD", ""),
        ("roba.toml", "readonly = true\nmodel = \"opus\"\n"),
    ]);
    roba()
        .args([
            "-C",
            project.path().to_str().unwrap(),
            "config",
            "show",
            "--sources",
            "model",
        ])
        .env("XDG_CONFIG_HOME", user_home.path())
        .env_remove("ROBA_PROFILE")
        .env_remove("ROBA_MODEL")
        .assert()
        .success()
        .stdout(predicate::str::contains("model = \"opus\""))
        // Only the requested key prints.
        .stdout(predicate::str::contains("readonly").not());
}

#[test]
fn config_show_sources_attributes_env_layer() {
    // A ROBA_* env override is the highest config layer and is attributed
    // to `env (ROBA_X)`.
    let user_home = tempfile::tempdir().expect("user home");
    let project = make_dir_with_files(&[(".git/HEAD", "")]);
    roba()
        .args([
            "-C",
            project.path().to_str().unwrap(),
            "config",
            "show",
            "--sources",
        ])
        .env("XDG_CONFIG_HOME", user_home.path())
        .env_remove("ROBA_PROFILE")
        .env("ROBA_WORKTREE", "1")
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "worktree = true  # env (ROBA_WORKTREE)",
        ));
}

#[test]
fn config_show_sources_unset_key_reports_to_stderr() {
    // A key set by no layer is genuinely unset (claude's own default
    // applies): a stderr note, byte-clean stdout, exit 0.
    let user_home = tempfile::tempdir().expect("user home");
    let project = make_dir_with_files(&[(".git/HEAD", ""), ("roba.toml", "readonly = true\n")]);
    roba()
        .args([
            "-C",
            project.path().to_str().unwrap(),
            "config",
            "show",
            "--sources",
            "max_budget_usd",
        ])
        .env("XDG_CONFIG_HOME", user_home.path())
        .env_remove("ROBA_PROFILE")
        .assert()
        .success()
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains(
            "max_budget_usd is not set by any config layer",
        ));
}

#[test]
fn config_show_sources_json_emits_versioned_envelope() {
    let user_home = tempfile::tempdir().expect("user home");
    let project = make_dir_with_files(&[
        (".git/HEAD", ""),
        ("roba.toml", "readonly = true\nmodel = \"opus\"\n"),
    ]);
    let assert = roba()
        .args([
            "-C",
            project.path().to_str().unwrap(),
            "config",
            "show",
            "--sources",
            "--json",
        ])
        .env("XDG_CONFIG_HOME", user_home.path())
        .env_remove("ROBA_PROFILE")
        .env_remove("ROBA_MODEL")
        .assert()
        .success();
    let out = &assert.get_output().stdout;
    let json: serde_json::Value = serde_json::from_slice(out).expect("valid JSON");
    assert_eq!(json["version"], 1);
    assert_eq!(json["result"]["effective"]["model"]["value"], "opus");
    assert!(
        json["result"]["effective"]["model"]["source"]
            .as_str()
            .unwrap()
            .contains("roba.toml"),
        "got: {json}"
    );
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
fn swallow_note_names_consumed_token_on_no_prompt() {
    // `-c "two words"` lets the optional value swallow what was meant as the
    // prompt, leaving nothing to run. The no-prompt error then names the
    // consumed token so the failure explains itself (#285). Empty stdin keeps
    // this off the TTY blurb path (assert_cmd stdin is a non-TTY pipe).
    roba()
        .args(["-c", "two words"])
        .write_stdin("")
        .assert()
        .failure()
        .stderr(predicate::str::contains("-c consumed \"two words\""));
}

#[test]
fn swallow_note_absent_for_bare_session_id_value() {
    // A whitespace-free `-c` value is a plausible real session id, not a
    // swallowed prompt -- the heuristic must NOT fire a note here.
    roba()
        .args(["-c", "abc123"])
        .write_stdin("")
        .assert()
        .failure()
        .stderr(predicate::str::contains("consumed").not());
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

// A well-formed UUID, so the conflict check (not the value_parser) is what
// rejects these invocations.
const VALID_UUID: &str = "550e8400-e29b-41d4-a716-446655440000";

#[test]
fn conflict_session_id_and_continue() {
    // --session-id assigns a NEW session's id; -c=ID resumes an
    // existing one. clap rejects the combination at parse time.
    assert_conflict(&["foo", "--session-id", VALID_UUID, "-c=y"]);
}

#[test]
fn conflict_session_id_and_pick() {
    assert_conflict(&["foo", "--session-id", VALID_UUID, "--pick"]);
}

#[test]
fn conflict_session_id_and_session() {
    assert_conflict(&["foo", "--session-id", VALID_UUID, "--session", "meta"]);
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
fn worktree_in_non_git_dir_preflight_fails_without_spawning_claude() {
    // -w in a directory that is not a git repo bails BEFORE the claude
    // spawn with a clean, actionable message on stderr (#327). PATH is
    // cleared so any spawn attempt would be a NotFound error -- the
    // preflight message proves we never got that far. XDG_CONFIG_HOME is
    // isolated so the real user config can't inject a default that changes
    // the outcome.
    let dir = tempfile::tempdir().expect("non-git tempdir");
    let user_home = tempfile::tempdir().expect("user home");
    roba()
        .args(["-C", dir.path().to_str().unwrap(), "-w", "-p", "hi"])
        .env("PATH", "")
        .env("XDG_CONFIG_HOME", user_home.path())
        .env_remove("ROBA_PROFILE")
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "--worktree needs a git repository",
        ))
        .stderr(predicate::str::contains("git init"));
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
    // The active-profile header is metadata and lands on stderr; stdout
    // carries only the re-parseable [profile.NAME] block (principle #2).
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("active: default"),
        "expected active default on stderr, got:\n{stderr}"
    );
    assert!(
        stderr.contains("auto-applied"),
        "expected auto-applied note on stderr, got:\n{stderr}"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("[profile.default]"),
        "expected profile block on stdout, got:\n{stdout}"
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
    // Header + reason are metadata on stderr; stdout is byte-clean TOML.
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("active: foo"),
        "expected active foo on stderr, got:\n{stderr}"
    );
    assert!(
        stderr.contains("from ROBA_PROFILE env"),
        "expected env-source note on stderr, got:\n{stderr}"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("[profile.foo]"),
        "expected profile block on stdout, got:\n{stdout}"
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

// ---------------------------------------------------------------------------
// persona list / show -- role-bearing profiles (#428)
// ---------------------------------------------------------------------------

/// A project with one persona (a profile with `agent` set) and one plain
/// profile, plus the persona's agent file so `persona show` can resolve it.
fn persona_project() -> tempfile::TempDir {
    make_dir_with_files(&[
        (".git/HEAD", ""),
        (
            ".claude/agents/reviewer.md",
            "---\nname: reviewer\ndescription: sample\n---\nYou review PRs.",
        ),
        (
            "roba.toml",
            r#"
[profile.reviewer]
description = "Read and comment PR reviewer"
agent = "reviewer"
allow_tool = ["Bash(gh pr view:*)"]

[profile.plainish]
model = "haiku"
"#,
        ),
    ])
}

#[test]
fn persona_list_shows_only_role_bearing_profiles() {
    let project = persona_project();
    let home = tempfile::tempdir().unwrap();
    roba_in(&project, &home)
        .args(["persona", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("reviewer"))
        .stdout(predicate::str::contains("Read and comment PR reviewer"))
        // The plain profile (no agent) is not a persona.
        .stdout(predicate::str::contains("plainish").not());
}

#[test]
fn persona_show_prints_block_and_locates_agent() {
    let project = persona_project();
    let home = tempfile::tempdir().unwrap();
    roba_in(&project, &home)
        .args(["persona", "show", "reviewer"])
        .assert()
        .success()
        // stdout: the re-parseable profile block.
        .stdout(predicate::str::contains("[profile.reviewer]"))
        .stdout(predicate::str::contains("agent = \"reviewer\""))
        // stderr: the resolved agent file (metadata).
        .stderr(predicate::str::contains("reviewer.md"));
}

#[test]
fn persona_show_non_persona_errors() {
    let project = persona_project();
    let home = tempfile::tempdir().unwrap();
    roba_in(&project, &home)
        .args(["persona", "show", "plainish"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not a persona"));
}

#[test]
fn persona_show_unknown_errors_with_known_list() {
    let project = persona_project();
    let home = tempfile::tempdir().unwrap();
    roba_in(&project, &home)
        .args(["persona", "show", "nope"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("no persona named `nope`"))
        .stderr(predicate::str::contains("reviewer"));
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
fn cost_suggestion_line_goes_to_stderr_not_stdout() {
    // The "--by-project / --json" guidance is metadata; stdout holds
    // only data so `roba cost | ...` stays clean (principle #2).
    let home = home_with_session("claude-sonnet-4-6", 1_000_000, 0);
    let out = roba()
        .arg("cost")
        .env("HOME", home.path())
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stdout.contains("run with --by-project"),
        "suggestion must not be on stdout, got:\n{stdout}"
    );
    assert!(
        stderr.contains("run with --by-project"),
        "suggestion must be on stderr, got:\n{stderr}"
    );
}

#[test]
fn cost_no_dollars_note_goes_to_stderr_not_stdout() {
    let home = home_with_session("claude-sonnet-4-6", 1_000_000, 0);
    let out = roba()
        .args(["cost", "--no-dollars"])
        .env("HOME", home.path())
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stdout.contains("dollars suppressed"),
        "note must not be on stdout, got:\n{stdout}"
    );
    assert!(
        stderr.contains("dollars suppressed"),
        "note must be on stderr, got:\n{stderr}"
    );
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
    // hang). --timeout 1 bounds the wait to ~1-2s. The wait-timeout maps
    // to exit 4 (the documented `4 timeout` code), not the generic 1.
    let home = tempfile::tempdir().expect("home");
    let start = std::time::Instant::now();
    roba()
        .args(["show", "never-appears", "--wait", "--timeout", "1"])
        .env("HOME", home.path())
        .assert()
        .code(4)
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
// roba last --json + empty-result exit-code alignment with history -- #396
// ---------------------------------------------------------------------------

/// Seed one session whose single assistant turn carries a tool_use block
/// followed by a text block, under a fixed project slug. Returns the home
/// tempdir; scope the `last` call with `--project -last-proj`.
fn home_with_last_session() -> tempfile::TempDir {
    let home = tempfile::tempdir().expect("home");
    let proj = home.path().join(".claude/projects/-last-proj");
    std::fs::create_dir_all(&proj).expect("mkdir projects");
    let user =
        r#"{"type":"user","timestamp":"2026-06-01T10:00:00.000Z","message":{"content":"hi"}}"#;
    let assistant = r#"{"type":"assistant","timestamp":"2026-06-01T10:00:01.000Z","message":{"content":[{"type":"tool_use","name":"Read","input":{"file_path":"x"}},{"type":"text","text":"the answer"}]}}"#;
    std::fs::write(
        proj.join("last-sess-1.jsonl"),
        format!("{user}\n{assistant}\n"),
    )
    .expect("write session");
    home
}

#[test]
fn last_json_carries_version_and_result() {
    // The uniform { version: 1, result: [items] } envelope, byte-clean
    // on stdout (parses as JSON, no ANSI). --type all surfaces both the
    // tool_use and the text block in content-block shape.
    let home = home_with_last_session();
    let out = roba()
        .args([
            "last",
            "--project",
            "-last-proj",
            "--type",
            "all",
            "-n",
            "2",
            "--json",
        ])
        .env("HOME", home.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let v: serde_json::Value = serde_json::from_slice(&out).expect("stdout is JSON");
    assert_eq!(v["version"], 1, "top-level version must be 1");
    let arr = v["result"].as_array().expect("result is array of items");
    assert_eq!(arr.len(), 2, "both blocks of the turn are items under all");
    assert_eq!(arr[0]["type"], "tool_use");
    assert_eq!(arr[0]["name"], "Read");
    assert_eq!(arr[0]["input"]["file_path"], "x");
    assert_eq!(arr[1]["type"], "text");
    assert_eq!(arr[1]["text"], "the answer");
}

#[test]
fn last_json_default_type_is_text_only() {
    // Without --type, `last` shows text answers; the JSON result must
    // mirror that (the tool_use block is filtered out).
    let home = home_with_last_session();
    let out = roba()
        .args(["last", "--project", "-last-proj", "--json"])
        .env("HOME", home.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let v: serde_json::Value = serde_json::from_slice(&out).expect("stdout is JSON");
    let arr = v["result"].as_array().expect("result is array of items");
    assert_eq!(arr.len(), 1, "only the text block under the default type");
    assert_eq!(arr[0]["type"], "text");
    assert_eq!(arr[0]["text"], "the answer");
}

#[test]
fn last_empty_project_exits_zero_matching_history() {
    // `last --project nonexistent` must align with `history`: exit 0,
    // advisory on stderr, nothing on stdout, and the SAME 'no sessions
    // found' wording (no 'error:' prefix).
    let home = home_with_last_session();
    roba()
        .args(["last", "--project", "-no-such-project"])
        .env("HOME", home.path())
        .assert()
        .success()
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains("no sessions found"))
        .stderr(predicate::str::contains("error:").not());

    // The sibling `history` command on the same empty filter: identical
    // exit code and wording.
    roba()
        .args(["history", "--project", "-no-such-project"])
        .env("HOME", home.path())
        .assert()
        .success()
        .stderr(predicate::str::contains("no sessions found"));
}

#[test]
fn last_json_empty_project_is_clean_empty_envelope() {
    // Under --json the empty case is the { version: 1, result: [] }
    // envelope on stdout (not an advisory), exit 0 -- mirroring
    // `history --json` on an empty filter.
    let home = home_with_last_session();
    let out = roba()
        .args(["last", "--project", "-no-such-project", "--json"])
        .env("HOME", home.path())
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let v: serde_json::Value = serde_json::from_slice(&out).expect("stdout is JSON");
    assert_eq!(v["version"], 1, "top-level version must be 1");
    assert_eq!(
        v["result"].as_array().expect("result is array").len(),
        0,
        "no session matches an unknown project"
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

#[test]
fn agent_check_missing_tool_hint_leads_with_no_agent_check() {
    // The missing-tools hint must lead with the acknowledge-the-constraint
    // option (running an agent below its declared tools is often deliberate).
    let project = project_with_bash_agent();
    let user_home = tempfile::tempdir().expect("user home");

    let out = roba()
        .args([
            "-C",
            project.path().to_str().unwrap(),
            "--agent",
            "test-agent",
            "--prepend",
            "/no/such/agent-check-hint-test",
            "some prompt",
        ])
        .env("HOME", user_home.path())
        .env_remove("ROBA_PROFILE")
        .output()
        .expect("run");

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("hint: intentional? --no-agent-check suppresses"),
        "expected the constraint-acknowledging hint, got:\n{stderr}"
    );
}

/// A project dir whose `.claude/agents/write-agent/AGENT.md` declares
/// Edit + Write (the canonical stall hazard).
fn project_with_write_agent() -> tempfile::TempDir {
    make_dir_with_files(&[
        (".git/HEAD", ""),
        (
            ".claude/agents/write-agent/AGENT.md",
            "---\nname: Write Agent\ntools:\n  - Edit\n  - Write\n---\n# body\n",
        ),
    ])
}

#[test]
fn agent_check_no_stall_warning_under_readonly() {
    // #264 false-positive repro: a write-declaring agent run read-only. The
    // write tools are unresolved, so the stall warning must NOT fire -- they
    // surface in the missing-tools warning instead.
    let project = project_with_write_agent();
    let user_home = tempfile::tempdir().expect("user home");

    let out = roba()
        .args([
            "-C",
            project.path().to_str().unwrap(),
            "--agent",
            "write-agent",
            "--prepend",
            "/no/such/agent-check-readonly-stall-test",
            "some prompt",
        ])
        .env("HOME", user_home.path())
        .env_remove("ROBA_PROFILE")
        .output()
        .expect("run");

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("not in the resolved allowlist"),
        "expected the missing-tools warning, got:\n{stderr}"
    );
    assert!(
        !stderr.contains("dispatch will stall"),
        "stall warning must NOT fire under read-only, got:\n{stderr}"
    );
}

#[test]
fn agent_check_stall_warning_with_writable() {
    // --writable resolves Edit/Write into the allowlist but sets no
    // permission mode, so the first write stalls: the stall warning fires
    // (and the missing-tools warning does not -- they are mutually exclusive).
    let project = project_with_write_agent();
    let user_home = tempfile::tempdir().expect("user home");

    let out = roba()
        .args([
            "-C",
            project.path().to_str().unwrap(),
            "--agent",
            "write-agent",
            "--writable",
            "--prepend",
            "/no/such/agent-check-writable-stall-test",
            "some prompt",
        ])
        .env("HOME", user_home.path())
        .env_remove("ROBA_PROFILE")
        .output()
        .expect("run");

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("dispatch will stall"),
        "expected the stall warning under --writable + default mode, got:\n{stderr}"
    );
    assert!(
        stderr.contains("--permission-mode acceptEdits"),
        "expected the escalation-shaped stall hint, got:\n{stderr}"
    );
    assert!(
        !stderr.contains("not in the resolved allowlist"),
        "missing-tools warning must NOT fire when writes are resolved, got:\n{stderr}"
    );
}

#[test]
fn agent_check_no_stall_warning_with_writable_accept_edits() {
    // --writable resolves the write tools and accept-edits auto-approves
    // them: no stall, no warning.
    let project = project_with_write_agent();
    let user_home = tempfile::tempdir().expect("user home");

    let out = roba()
        .args([
            "-C",
            project.path().to_str().unwrap(),
            "--agent",
            "write-agent",
            "--writable",
            "--permission-mode",
            "accept-edits",
            "--prepend",
            "/no/such/agent-check-acceptedits-test",
            "some prompt",
        ])
        .env("HOME", user_home.path())
        .env_remove("ROBA_PROFILE")
        .output()
        .expect("run");

    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains("dispatch will stall"),
        "stall warning must NOT fire under accept-edits, got:\n{stderr}"
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

#[test]
fn doctor_plain_emits_no_ansi() {
    // `--plain` is accepted and the human output stays byte-plain (no ESC).
    // Under assert_cmd stdout is already a pipe, so color is off regardless;
    // this pins that the flag parses and the off path leaks no ANSI. The
    // colored-vs-plain rendering itself is covered by doctor's unit tests.
    let out = roba().args(["doctor", "--plain"]).output().expect("run");
    assert!(
        !out.stdout.contains(&0x1b),
        "doctor --plain leaked an ESC byte"
    );
    let code = out.status.code().expect("exited normally");
    assert!(code == 0 || code == 1, "doctor exits 0 or 1, got {code}");
}

#[test]
fn doctor_plain_does_not_affect_json() {
    // `--plain` has no effect under `--json` (that output is already
    // byte-plain); the envelope shape is unchanged.
    let out = roba()
        .args(["doctor", "--plain", "--json"])
        .output()
        .expect("run");
    let v: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("stdout is JSON on --json");
    assert_eq!(v["version"], 1, "top-level version must be 1");
    assert!(v["result"]["checks"].is_array(), "checks array present");
}

#[test]
fn config_lint_plain_emits_no_ansi() {
    // `--plain` is accepted by `config lint` and the findings output stays
    // byte-plain. Mirrors `doctor --plain` -- the shared report-verb
    // off-switch.
    let project = make_dir_with_files(&[
        (".git/HEAD", ""),
        ("roba.toml", "[alias.cost]\ntemplate = \"x ${@}\"\n"),
    ]);
    let user_home = tempfile::tempdir().expect("user home");
    let out = roba()
        .args([
            "-C",
            project.path().to_str().unwrap(),
            "config",
            "lint",
            "--plain",
        ])
        .env("XDG_CONFIG_HOME", user_home.path())
        .output()
        .expect("run");
    assert!(
        !out.stdout.contains(&0x1b),
        "config lint --plain leaked an ESC byte"
    );
}

// ---------------------------------------------------------------------------
// -c <prefix> -- git-style short session-id resolution (#304)
// ---------------------------------------------------------------------------

/// An ambiguous short prefix for `-c` errors with the candidate ids and
/// never reaches claude (the bail happens during resolution, before any
/// claude call). Unix-only: the project-slug encoding replaces `/` with
/// `-`, which only matches the resolver's cwd encoding on Unix paths.
#[cfg(unix)]
#[test]
fn continue_ambiguous_prefix_lists_candidates_and_fails() {
    let home = tempfile::tempdir().expect("home");
    let project = tempfile::tempdir().expect("project");
    // The resolver scopes to the canonicalized-cwd project slug. Derive the
    // fixture dir via the SAME wrapper function the binary uses (canonicalize
    // + encode '/' AND '.'), so they can't drift -- tempdir names contain a
    // `.` (the `.tmpXXXX` prefix), which the old `/`-only encoding missed.
    let slug = claude_wrapper::history::HistoryRoot::project_slug(project.path());
    let proj = home.path().join(".claude/projects").join(&slug);
    std::fs::create_dir_all(&proj).expect("mkdir project slug");

    // Two sessions sharing the 8-char prefix `ef7de917` (the displayed
    // footer form). Each needs >=1 message so it survives the empty-session
    // filter the enumeration applies.
    let user =
        r#"{"type":"user","timestamp":"2026-06-01T10:00:00.000Z","message":{"content":"hi"}}"#;
    let assistant = r#"{"type":"assistant","timestamp":"2026-06-01T10:00:01.000Z","message":{"model":"claude-haiku-4-5","content":[{"type":"text","text":"ok"}]}}"#;
    let body = format!("{user}\n{assistant}\n");
    let id_a = "ef7de917-aaaa-4aaa-8aaa-000000000001";
    let id_b = "ef7de917-bbbb-4bbb-8bbb-000000000002";
    std::fs::write(proj.join(format!("{id_a}.jsonl")), &body).expect("write a");
    std::fs::write(proj.join(format!("{id_b}.jsonl")), &body).expect("write b");

    roba()
        .current_dir(project.path())
        .env("HOME", home.path())
        .args(["-c", "ef7de917", "hi"])
        .assert()
        .failure()
        .stderr(
            predicate::str::contains("ambiguous")
                .and(predicate::str::contains(id_a))
                .and(predicate::str::contains(id_b)),
        );
}

// ---------------------------------------------------------------------------
// #360: never exit 0 on an empty / is_error result.
//
// The wrapper returns Ok(QueryResult) whenever claude exits 0, even when the
// payload is an empty answer or carries is_error: true. roba must map those
// to a non-zero exit (6) so a non-answer never looks like success to a caller
// that trusts $?. These drive a fake `claude` on PATH that prints a canned
// JSON result and exits 0 -- no real claude call. Unix-only: the fake is a
// /bin/sh script.
// ---------------------------------------------------------------------------

/// Write an executable `claude` shim that ignores its args/stdin and prints
/// `json_stdout` followed by exit 0. Returns the dir to put on PATH.
#[cfg(unix)]
fn fake_claude(json_stdout: &str) -> tempfile::TempDir {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().expect("fake claude dir");
    let path = dir.path().join("claude");
    // A `--version` probe (should one ever fire) gets a version string; any
    // other invocation drains stdin, then prints the canned JSON (the heredoc
    // is quoted so the JSON is emitted verbatim).
    //
    // `cat >/dev/null` drains stdin BEFORE emitting output: roba writes the
    // prompt to claude's stdin, and if this shim exits without reading it, the
    // prompt write races the exit and fails with EPIPE -- which roba surfaces
    // as an `Err` (exit 1), not the empty/`is_error` `Ok` path (exit 6). That
    // race only loses under load, so it flaked Linux CI intermittently (#371).
    // Draining stdin makes the read deterministic and the exit code stable.
    let script = format!(
        "#!/bin/sh\ncase \"$*\" in\n  *--version*) echo '1.0.0 (fake)'; exit 0;;\nesac\ncat >/dev/null 2>&1\ncat <<'ROBA_FAKE_EOF'\n{json_stdout}\nROBA_FAKE_EOF\n"
    );
    std::fs::write(&path, script).expect("write fake claude");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
        .expect("chmod fake claude");
    dir
}

/// Write an executable `claude` shim that emits one streaming event for the
/// provider-neutral bounded-run adapter.
#[cfg(unix)]
fn fake_claude_streaming_result(result_event: &str) -> tempfile::TempDir {
    fake_claude_streaming_terminal(result_event, 0)
}

/// Write an executable `claude` shim that emits one streaming terminal event
/// and then exits with the requested status.
#[cfg(unix)]
fn fake_claude_streaming_terminal(result_event: &str, exit_code: i32) -> tempfile::TempDir {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().expect("fake claude dir");
    let path = dir.path().join("claude");
    let script = format!(
        r#"#!/bin/sh
case "$*" in
  *--version*) echo '1.0.0 (fake)'; exit 0;;
esac
cat >/dev/null 2>&1
printf '%s\n' '{result_event}'
exit {exit_code}
"#
    );
    std::fs::write(&path, script).expect("write fake claude");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
        .expect("chmod fake claude");
    dir
}

/// Write a `claude` shim that stays alive until the bounded adapter's timeout
/// cancels it. The provider should never wait for the fallback success event.
#[cfg(unix)]
fn fake_claude_streaming_timeout() -> tempfile::TempDir {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().expect("fake claude dir");
    let path = dir.path().join("claude");
    let script = r#"#!/bin/sh
case "$*" in
  *--version*) echo '1.0.0 (fake)'; exit 0;;
esac
cat >/dev/null 2>&1
sleep 5
printf '%s\n' '{"type":"result","subtype":"success","result":"too late","session_id":"late","is_error":false}'
"#;
    std::fs::write(&path, script).expect("write fake claude");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
        .expect("chmod fake claude");
    dir
}

/// Write a `claude` shim that publishes its PID and holds until cancelled.
#[cfg(unix)]
fn fake_claude_streaming_hold() -> tempfile::TempDir {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().expect("fake claude dir");
    let path = dir.path().join("claude");
    let script = r#"#!/bin/sh
case "$*" in
  *--version*) echo '1.0.0 (fake)'; exit 0;;
esac
printf '%s\n' "$$" > "$ROBA_PROVIDER_PID"
cat >/dev/null 2>&1
exec /bin/sleep 30
"#;
    std::fs::write(&path, script).expect("write fake held claude");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
        .expect("chmod fake held claude");
    dir
}

/// Write an executable `codex` shim that records argv and stdin before
/// emitting either a successful JSONL turn or a structured failed turn.
#[cfg(unix)]
fn fake_codex_streaming() -> tempfile::TempDir {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().expect("fake codex dir");
    let path = dir.path().join("codex");
    let script = r#"#!/bin/sh
printf '%s\n' "$@" > "$ROBA_CAPTURE_ARGS"
prompt=$(cat)
printf '%s' "$prompt" > "$ROBA_CAPTURE_STDIN"
printf '%s\n' '{"type":"thread.started","thread_id":"codex-thread-1"}'
if [ "${ROBA_CODEX_FAILURE:-}" = 1 ]; then
  printf '%s\n' '{"type":"turn.failed","error":{"message":"login required"}}'
  printf '%s\n' '401 Unauthorized' >&2
  exit 1
fi
if [ "${ROBA_CODEX_EMPTY:-}" = 1 ]; then
  printf '%s\n' '{"type":"turn.completed","usage":{"input_tokens":1,"output_tokens":0}}'
  exit 0
fi
printf '%s\n' '{"type":"item.completed","item":{"type":"agent_message","text":"codex answer"}}'
printf '%s\n' '{"type":"turn.completed","usage":{"input_tokens":7,"output_tokens":3}}'
"#;
    std::fs::write(&path, script).expect("write fake codex");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
        .expect("chmod fake codex");
    dir
}

/// Like [`fake_claude`], but records one argument per line at
/// `$ROBA_CAPTURE_ARGS` before returning a successful result.
#[cfg(unix)]
fn fake_claude_capturing_args() -> tempfile::TempDir {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().expect("fake claude dir");
    let path = dir.path().join("claude");
    let script = r#"#!/bin/sh
case "$*" in
  *--version*) echo '1.0.0 (fake)'; exit 0;;
esac
printf '%s\n' "$@" > "$ROBA_CAPTURE_ARGS"
if [ "${ROBA_VALIDATE_BUNDLE_PATHS:-}" = 1 ]; then
  previous=''
  for argument in "$@"; do
    case "$previous" in
      --settings|--mcp-config|--plugin-dir)
        [ -e "$argument" ] || exit 97
        ;;
    esac
    previous="$argument"
  done
fi
cat >/dev/null 2>&1
if [ "${ROBA_EMPTY_RESULT:-}" = 1 ]; then
  printf '%s\n' '{"result":"","session_id":"s1","is_error":false}'
else
  printf '%s\n' '{"result":"ok","session_id":"s1","is_error":false}'
fi
"#;
    std::fs::write(&path, script).expect("write fake claude");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
        .expect("chmod fake claude");
    dir
}

/// A roba command wired to a fake claude on PATH, isolated from any real
/// roba.toml (HOME + XDG_CONFIG_HOME point at empty temp dirs).
#[cfg(unix)]
fn roba_with_fake_claude(
    bin_dir: &std::path::Path,
    home: &std::path::Path,
    cfg: &std::path::Path,
) -> Command {
    // The fake dir leads so its `claude` wins the `which` lookup; the
    // system bins follow so the shim's `/bin/sh` + `cat` resolve.
    let path = format!("{}:/usr/bin:/bin", bin_dir.display());
    let mut cmd = roba();
    cmd.env("PATH", path)
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", cfg)
        .current_dir(home);
    cmd
}

#[cfg(unix)]
#[test]
fn hermetic_bundle_provisions_the_exact_claude_child_surface() {
    let bin = fake_claude_capturing_args();
    let home = tempfile::tempdir().expect("home");
    let cfg = tempfile::tempdir().expect("cfg");
    let bundle = home.path().join(".roba");
    let capture = home.path().join("args.txt");

    std::fs::create_dir_all(bundle.join("agents")).unwrap();
    std::fs::write(
        bundle.join("agents/reviewer.md"),
        "---\ndescription: Reviews code\ntools: Read, Grep\n---\nReview the change.",
    )
    .unwrap();
    std::fs::write(bundle.join("settings.json"), r#"{"hooks":{}}"#).unwrap();
    std::fs::write(bundle.join("mcp.json"), r#"{"mcpServers":{}}"#).unwrap();

    std::fs::create_dir_all(bundle.join("skills/review")).unwrap();
    std::fs::create_dir_all(bundle.join(".claude-plugin")).unwrap();
    std::fs::write(
        bundle.join(".claude-plugin/plugin.json"),
        r#"{"name":"bundle"}"#,
    )
    .unwrap();
    let plugin = bundle.join("plugins/lint/.claude-plugin");
    std::fs::create_dir_all(&plugin).unwrap();
    std::fs::write(plugin.join("plugin.json"), r#"{"name":"lint"}"#).unwrap();

    roba_with_fake_claude(bin.path(), home.path(), cfg.path())
        .env("ROBA_CAPTURE_ARGS", &capture)
        .env("ROBA_VALIDATE_BUNDLE_PATHS", "1")
        .args([
            "--hermetic",
            "--bundle",
            bundle.to_str().unwrap(),
            "--agent",
            "reviewer",
            "hello",
        ])
        .assert()
        .success()
        .stdout("ok\n");

    let captured = std::fs::read_to_string(&capture).unwrap();
    let argv: Vec<&str> = captured.lines().collect();
    let value_after = |flag: &str| {
        let index = argv.iter().position(|arg| *arg == flag).unwrap();
        argv[index + 1]
    };

    assert_eq!(value_after("--agent"), "reviewer");
    let agents: serde_json::Value = serde_json::from_str(value_after("--agents")).unwrap();
    assert_eq!(agents["reviewer"]["description"], "Reviews code");
    assert_eq!(agents["reviewer"]["prompt"], "Review the change.");
    let settings = value_after("--settings");
    assert_ne!(settings, bundle.join("settings.json").to_str().unwrap());
    assert!(settings.ends_with("settings.json"));
    let mcp = value_after("--mcp-config");
    assert_ne!(mcp, bundle.join("mcp.json").to_str().unwrap());
    assert!(mcp.ends_with("mcp.json"));
    assert_eq!(value_after("--setting-sources"), "");
    assert!(argv.contains(&"--strict-mcp-config"));

    let plugin_roots: Vec<&str> = argv
        .windows(2)
        .filter(|pair| pair[0] == "--plugin-dir")
        .map(|pair| pair[1])
        .collect();
    assert_eq!(plugin_roots.len(), 2);
    assert_ne!(plugin_roots[0], bundle.to_str().unwrap());
    assert_ne!(
        plugin_roots[1],
        bundle.join("plugins/lint").to_str().unwrap()
    );
    assert!(plugin_roots[1].ends_with("plugins/lint"));
    for snapshot_path in [settings, mcp, plugin_roots[0], plugin_roots[1]] {
        assert!(
            !std::path::Path::new(snapshot_path).exists(),
            "run-local snapshot survived provider completion: {snapshot_path}"
        );
    }

    let override_capture = home.path().join("override-args.txt");
    roba_with_fake_claude(bin.path(), home.path(), cfg.path())
        .env("ROBA_CAPTURE_ARGS", &override_capture)
        .args(["--hermetic=claude", "--setting-sources", "user", "hello"])
        .assert()
        .success()
        .stdout("ok\n");
    let override_argv = std::fs::read_to_string(override_capture).unwrap();
    let override_argv: Vec<&str> = override_argv.lines().collect();
    let index = override_argv
        .iter()
        .position(|arg| *arg == "--setting-sources")
        .unwrap();
    assert_eq!(override_argv[index + 1], "user");
}

#[cfg(unix)]
#[test]
fn unusable_bundle_result_still_cleans_the_run_local_snapshot() {
    let bin = fake_claude_capturing_args();
    let home = tempfile::tempdir().expect("home");
    let cfg = tempfile::tempdir().expect("cfg");
    let bundle = home.path().join("bundle");
    let capture = home.path().join("empty-result-args.txt");

    std::fs::create_dir_all(bundle.join("plugins/lint/.claude-plugin")).unwrap();
    std::fs::write(bundle.join("settings.json"), r#"{"hooks":{}}"#).unwrap();
    std::fs::write(bundle.join("mcp.json"), r#"{"mcpServers":{}}"#).unwrap();
    std::fs::write(
        bundle.join("plugins/lint/.claude-plugin/plugin.json"),
        r#"{"name":"lint"}"#,
    )
    .unwrap();

    roba_with_fake_claude(bin.path(), home.path(), cfg.path())
        .env("ROBA_CAPTURE_ARGS", &capture)
        .env("ROBA_VALIDATE_BUNDLE_PATHS", "1")
        .env("ROBA_EMPTY_RESULT", "1")
        .args(["--bundle", bundle.to_str().unwrap(), "hello"])
        .assert()
        .code(6)
        .stderr(predicate::str::contains("empty result"));

    let captured = std::fs::read_to_string(&capture).unwrap();
    let argv: Vec<&str> = captured.lines().collect();
    let mut snapshot_paths = argv.windows(2).filter_map(|pair| {
        matches!(pair[0], "--settings" | "--mcp-config" | "--plugin-dir").then_some(pair[1])
    });
    let first = snapshot_paths.next().expect("captured bundle path");
    assert!(!std::path::Path::new(first).exists());
    for snapshot_path in snapshot_paths {
        assert!(!std::path::Path::new(snapshot_path).exists());
    }
}

#[cfg(unix)]
#[test]
fn detached_bundle_uses_and_cleans_the_parents_snapshot() {
    let bin = fake_claude_capturing_args();
    let home = tempfile::tempdir().expect("home");
    let cfg = tempfile::tempdir().expect("cfg");
    let bundle = home.path().join("detached-bundle");
    let capture = home.path().join("detached-args.txt");
    let state = home.path().join("state");
    let session = "123e4567-e89b-42d3-a456-426614174000";

    std::fs::create_dir_all(bundle.join("plugins/lint/.claude-plugin")).unwrap();
    std::fs::write(bundle.join("settings.json"), r#"{"hooks":{}}"#).unwrap();
    std::fs::write(bundle.join("mcp.json"), r#"{"mcpServers":{}}"#).unwrap();
    std::fs::write(
        bundle.join("plugins/lint/.claude-plugin/plugin.json"),
        r#"{"name":"lint"}"#,
    )
    .unwrap();

    let output = roba_with_fake_claude(bin.path(), home.path(), cfg.path())
        .env("ROBA_CAPTURE_ARGS", &capture)
        .env("ROBA_VALIDATE_BUNDLE_PATHS", "1")
        .env("ROBA_STATE_DIR", &state)
        .args([
            "--detach",
            "--bundle",
            bundle.to_str().unwrap(),
            "--session-id",
            session,
            "--max-turns",
            "1",
            "hello",
        ])
        .output()
        .expect("launch detached bundle run");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), session);

    // The parent has already captured every provider input. Removing the
    // source before the detached provider reports proves the re-exec does not
    // reopen the caller's path.
    std::fs::remove_dir_all(&bundle).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !capture.exists() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let captured = std::fs::read_to_string(&capture).expect("detached provider captured argv");
    let argv: Vec<&str> = captured.lines().collect();
    let snapshot_paths: Vec<&str> = argv
        .windows(2)
        .filter_map(|pair| {
            matches!(pair[0], "--settings" | "--mcp-config" | "--plugin-dir").then_some(pair[1])
        })
        .collect();
    assert!(!snapshot_paths.is_empty());
    assert!(
        snapshot_paths
            .iter()
            .all(|path| !path.starts_with(bundle.to_str().unwrap()))
    );

    let cleanup_deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while snapshot_paths
        .iter()
        .any(|path| std::path::Path::new(path).exists())
        && std::time::Instant::now() < cleanup_deadline
    {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    for snapshot_path in snapshot_paths {
        assert!(
            !std::path::Path::new(snapshot_path).exists(),
            "detached run-local snapshot survived child completion: {snapshot_path}"
        );
    }
}

#[cfg(unix)]
#[test]
fn empty_result_exits_nonzero_with_clean_note() {
    let bin = fake_claude(r#"{"result":"","session_id":"s1","is_error":false}"#);
    let home = tempfile::tempdir().expect("home");
    let cfg = tempfile::tempdir().expect("cfg");
    roba_with_fake_claude(bin.path(), home.path(), cfg.path())
        .arg("hello")
        .assert()
        .code(6)
        .stderr(predicate::str::contains("empty result"));
}

#[cfg(unix)]
#[test]
fn is_error_result_exits_nonzero_with_clean_note() {
    let bin = fake_claude(r#"{"result":"partial","session_id":"s1","is_error":true}"#);
    let home = tempfile::tempdir().expect("home");
    let cfg = tempfile::tempdir().expect("cfg");
    roba_with_fake_claude(bin.path(), home.path(), cfg.path())
        .arg("hello")
        .assert()
        .code(6)
        .stderr(predicate::str::contains("is_error"));
}

#[cfg(unix)]
#[test]
fn empty_result_json_envelope_still_emits_and_exits_nonzero() {
    // --json stays byte-clean: the success envelope is emitted on stdout and
    // the exit code (6), not a missing envelope, carries the failure signal.
    let bin = fake_claude(r#"{"result":"","session_id":"s1","is_error":false}"#);
    let home = tempfile::tempdir().expect("home");
    let cfg = tempfile::tempdir().expect("cfg");
    let assert = roba_with_fake_claude(bin.path(), home.path(), cfg.path())
        .args(["--json", "hello"])
        .assert()
        .code(6);
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).expect("utf8 stdout");
    let value: serde_json::Value =
        serde_json::from_str(&stdout).expect("stdout is a valid JSON envelope");
    assert_eq!(value["version"], 1);
    assert!(value.get("result").is_some(), "envelope carries result");
}

#[cfg(unix)]
#[test]
fn nonempty_result_exits_zero() {
    // The happy path is untouched: a usable answer still exits 0.
    let bin = fake_claude(r#"{"result":"the answer is 42","session_id":"s1","is_error":false}"#);
    let home = tempfile::tempdir().expect("home");
    let cfg = tempfile::tempdir().expect("cfg");
    roba_with_fake_claude(bin.path(), home.path(), cfg.path())
        .arg("hello")
        .assert()
        .success()
        .stdout(predicate::str::contains("the answer is 42"));
}

#[cfg(unix)]
#[test]
fn bounded_run_json_emits_the_complete_terminal_snapshot() {
    let bin = fake_claude_streaming_result(
        r#"{"type":"result","subtype":"success","result":"bounded answer","session_id":"session-1","total_cost_usd":0.02,"duration_ms":10,"num_turns":1,"is_error":false,"usage":{"input_tokens":3,"output_tokens":2}}"#,
    );
    let home = tempfile::tempdir().expect("home");
    let cfg = tempfile::tempdir().expect("cfg");

    let output = roba_with_fake_claude(bin.path(), home.path(), cfg.path())
        .args(["run", "--provider", "claude", "--json", "hello"])
        .output()
        .expect("run bounded JSON adapter");
    assert!(
        output.status.success(),
        "bounded run failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());

    let envelope: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(envelope["version"], roba_types::VERSION);
    let snapshot = &envelope["result"];
    assert_eq!(snapshot["state"], "completed");
    assert_eq!(snapshot["turns_completed"], 1);
    assert_eq!(snapshot["last_outcome"]["output"], "bounded answer");
    assert_eq!(snapshot["last_outcome"]["session"]["id"], "session-1");
    assert_eq!(snapshot["last_outcome"]["usage"]["input"], 3);
    assert_eq!(snapshot["last_outcome"]["usage"]["output"], 2);
    assert_eq!(snapshot["last_outcome"]["cost"]["currency"], "USD");
    assert_eq!(snapshot["last_outcome"]["cost"]["amount"], 0.02);
    assert_eq!(snapshot["last_outcome"]["duration_ms"], 10);
    assert_eq!(snapshot["last_outcome"]["provider_turns"], 1);
    assert!(snapshot["created_at_unix_ms"].is_u64());
    assert!(snapshot["started_at_unix_ms"].is_u64());
    assert!(snapshot["finished_at_unix_ms"].is_u64());
    assert!(snapshot["elapsed_ms"].is_u64());

    roba_with_fake_claude(bin.path(), home.path(), cfg.path())
        .args(["run", "--provider", "claude", "hello"])
        .assert()
        .success()
        .stdout("bounded answer\n");
}

#[cfg(unix)]
#[test]
fn bounded_run_json_preserves_a_failed_snapshot_and_structured_error() {
    let bin = fake_claude_streaming_result(
        r#"{"type":"result","subtype":"error_during_execution","result":"bounded failure","session_id":"session-1","is_error":true}"#,
    );
    let home = tempfile::tempdir().expect("home");
    let cfg = tempfile::tempdir().expect("cfg");

    let output = roba_with_fake_claude(bin.path(), home.path(), cfg.path())
        .args(["run", "--provider", "claude", "--json", "hello"])
        .output()
        .expect("run failed bounded JSON adapter");
    assert_eq!(output.status.code(), Some(1));

    let envelope: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(envelope["version"], roba_types::VERSION);
    assert_eq!(envelope["result"]["state"], "failed");
    assert_eq!(envelope["result"]["failure"]["kind"], "provider");
    assert_eq!(envelope["result"]["failure"]["message"], "bounded failure");

    let error: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["version"], roba_types::VERSION);
    assert_eq!(error["error"]["exit_code"], 1);
    assert_eq!(error["error"]["message"], "bounded failure");
}

#[cfg(unix)]
#[test]
fn bounded_run_json_preserves_recoverable_limit_details_after_nonzero_exit() {
    let bin = fake_claude_streaming_terminal(
        r#"{"type":"result","subtype":"error_max_turns","session_id":"limit-session","total_cost_usd":1.25,"duration_ms":321,"num_turns":30,"is_error":true,"usage":{"input_tokens":100,"output_tokens":20},"errors":["Reached maximum number of turns (30)"]}"#,
        1,
    );
    let home = tempfile::tempdir().expect("home");
    let cfg = tempfile::tempdir().expect("cfg");

    let output = roba_with_fake_claude(bin.path(), home.path(), cfg.path())
        .args(["run", "--provider", "claude", "--json", "hello"])
        .output()
        .expect("run capped bounded JSON adapter");
    assert_eq!(output.status.code(), Some(5));

    let envelope: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let failure = &envelope["result"]["failure"];
    assert_eq!(failure["kind"], "max_turns");
    assert_eq!(failure["details"]["session"]["id"], "limit-session");
    assert_eq!(failure["details"]["usage"]["input"], 100);
    assert_eq!(failure["details"]["usage"]["output"], 20);
    assert_eq!(failure["details"]["cost"]["amount"], 1.25);
    assert_eq!(failure["details"]["duration_ms"], 321);
    assert_eq!(failure["details"]["provider_turns"], 30);

    let error: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["error"]["kind"], "limit");
    assert_eq!(error["error"]["exit_code"], 5);
    let message = error["error"]["message"].as_str().unwrap();
    assert!(message.contains("max-turns limit after 30 provider turns"));
    assert!(!message.contains("--output-format"));
}

#[cfg(unix)]
#[test]
fn bounded_run_timeout_crosses_mcp_and_keeps_exit_four() {
    let bin = fake_claude_streaming_timeout();
    let home = tempfile::tempdir().expect("home");
    let cfg = tempfile::tempdir().expect("cfg");
    let started = std::time::Instant::now();

    let output = roba_with_fake_claude(bin.path(), home.path(), cfg.path())
        .args([
            "run",
            "--provider",
            "claude",
            "--timeout",
            "1",
            "--json",
            "hello",
        ])
        .output()
        .expect("run timed bounded adapter through MCP");
    assert_eq!(output.status.code(), Some(roba_types::EXIT_TIMEOUT));
    assert!(
        started.elapsed() < std::time::Duration::from_secs(4),
        "provider timeout did not cancel the slow child promptly"
    );

    let terminal: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(terminal["result"]["state"], "failed");
    assert_eq!(terminal["result"]["failure"]["kind"], "timeout");

    let error: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["error"]["kind"], "timeout");
    assert_eq!(error["error"]["exit_code"], roba_types::EXIT_TIMEOUT);
}

#[cfg(unix)]
#[test]
fn codex_bounded_run_is_stdin_safe_and_normalized_at_the_cli_boundary() {
    let bin = fake_codex_streaming();
    let home = tempfile::tempdir().expect("home");
    let cfg = tempfile::tempdir().expect("cfg");
    let captured_args = home.path().join("codex-args.txt");
    let captured_stdin = home.path().join("codex-stdin.txt");
    let prompt = "sensitive fresh prompt";

    let output = roba_with_fake_claude(bin.path(), home.path(), cfg.path())
        .env("ROBA_CAPTURE_ARGS", &captured_args)
        .env("ROBA_CAPTURE_STDIN", &captured_stdin)
        .args([
            "run",
            "--provider",
            "codex",
            "--model",
            "gpt-test",
            "--effort",
            "high",
            "--writable",
            "--json",
            prompt,
        ])
        .output()
        .expect("run fake Codex through the CLI");
    assert!(
        output.status.success(),
        "Codex run failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());

    let envelope: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let outcome = &envelope["result"]["last_outcome"];
    assert_eq!(envelope["result"]["state"], "completed");
    assert_eq!(outcome["output"], "codex answer");
    assert_eq!(outcome["session"]["provider"], "codex");
    assert_eq!(outcome["session"]["id"], "codex-thread-1");
    assert_eq!(outcome["usage"]["input"], 7);
    assert_eq!(outcome["usage"]["output"], 3);

    let argv = std::fs::read_to_string(&captured_args).unwrap();
    assert!(argv.lines().any(|arg| arg == "exec"));
    assert!(argv.lines().any(|arg| arg == "--json"));
    assert!(argv.lines().any(|arg| arg == "gpt-test"));
    assert!(argv.contains("model_reasoning_effort=\"high\""));
    assert!(argv.lines().any(|arg| arg == "workspace-write"));
    assert!(!argv.contains(prompt), "fresh prompt leaked into argv");
    let stdin = std::fs::read_to_string(captured_stdin).unwrap();
    assert!(
        stdin.starts_with("You are operating as a Roba-managed agent using provider \"codex\"")
    );
    assert!(stdin.contains("execution authority is workspace_write"));
    assert!(stdin.contains("call `context.manifest`"));
    assert!(stdin.ends_with(&format!("\n\n{prompt}")));
}

#[cfg(unix)]
#[test]
fn codex_structured_failure_keeps_thread_and_auth_exit_code_end_to_end() {
    let bin = fake_codex_streaming();
    let home = tempfile::tempdir().expect("home");
    let cfg = tempfile::tempdir().expect("cfg");
    let captured_args = home.path().join("codex-failed-args.txt");
    let captured_stdin = home.path().join("codex-failed-stdin.txt");

    let output = roba_with_fake_claude(bin.path(), home.path(), cfg.path())
        .env("ROBA_CAPTURE_ARGS", &captured_args)
        .env("ROBA_CAPTURE_STDIN", &captured_stdin)
        .env("ROBA_CODEX_FAILURE", "1")
        .args(["run", "--provider", "codex", "--json", "hello"])
        .output()
        .expect("run failed fake Codex through the CLI");
    assert_eq!(output.status.code(), Some(roba_types::EXIT_AUTH));

    let terminal: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(terminal["result"]["state"], "failed");
    assert_eq!(terminal["result"]["failure"]["kind"], "authentication");
    assert_eq!(
        terminal["result"]["failure"]["details"]["session"]["id"],
        "codex-thread-1"
    );

    let error: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["error"]["kind"], "auth");
    assert_eq!(error["error"]["exit_code"], roba_types::EXIT_AUTH);
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap()
            .contains("login required")
    );
}

#[cfg(unix)]
#[test]
fn codex_empty_answer_uses_the_stable_unusable_result_exit() {
    let bin = fake_codex_streaming();
    let home = tempfile::tempdir().expect("home");
    let cfg = tempfile::tempdir().expect("cfg");
    let captured_args = home.path().join("codex-empty-args.txt");
    let captured_stdin = home.path().join("codex-empty-stdin.txt");

    let output = roba_with_fake_claude(bin.path(), home.path(), cfg.path())
        .env("ROBA_CAPTURE_ARGS", &captured_args)
        .env("ROBA_CAPTURE_STDIN", &captured_stdin)
        .env("ROBA_CODEX_EMPTY", "1")
        .args(["run", "--provider", "codex", "--json", "hello"])
        .output()
        .expect("run empty fake Codex through the CLI");
    assert_eq!(output.status.code(), Some(roba_types::EXIT_UNUSABLE_RESULT));

    let terminal: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(terminal["result"]["state"], "completed");
    assert_eq!(terminal["result"]["last_outcome"]["output"], "");
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("provider returned an empty result"));
    assert!(serde_json::from_str::<serde_json::Value>(&stderr).is_err());
}

#[cfg(unix)]
#[test]
fn trace_no_result_event_exits_6() {
    // #368 Note 1: the --trace (Silent, no --stream) path drives the
    // streaming pipeline; a run that emits NDJSON events but no `result`
    // event is "no usable output" -- it must exit 6, the same code the
    // Live --stream path uses, not the generic exit 1 a bail would give.
    // The fake shim emits two valid stream-json events (system + assistant)
    // and no result event.
    let bin = fake_claude(
        "{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"s1\"}\n\
         {\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"hi\"}]}}",
    );
    let home = tempfile::tempdir().expect("home");
    let cfg = tempfile::tempdir().expect("cfg");
    let trace = home.path().join("trace.jsonl");
    roba_with_fake_claude(bin.path(), home.path(), cfg.path())
        .args(["--trace", trace.to_str().unwrap(), "hello"])
        .assert()
        .code(6)
        .stderr(predicate::str::contains("no result event"));
}

// ---------------------------------------------------------------------------
// jobs + watch (#444 slices 1-2): derived views over run receipts.
// All receipts are planted under an isolated ROBA_STATE_DIR -- no claude.
// ---------------------------------------------------------------------------

/// Plant a receipt JSON under `<state>/runs/<id>.json`.
fn plant_receipt(state_dir: &std::path::Path, id: &str, body: &str) {
    let runs = state_dir.join("runs");
    std::fs::create_dir_all(&runs).unwrap();
    std::fs::write(runs.join(format!("{id}.json")), body).unwrap();
}

#[test]
fn jobs_empty_state_dir_is_a_note_and_exit_0() {
    let dir = tempfile::tempdir().unwrap();
    roba()
        .env("ROBA_STATE_DIR", dir.path())
        .arg("jobs")
        .assert()
        .success()
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains("no detached runs recorded"));
}

#[test]
fn jobs_lists_terminal_receipts_with_state_and_cost() {
    let dir = tempfile::tempdir().unwrap();
    plant_receipt(
        dir.path(),
        "aaaa1111-0000-4000-8000-000000000001",
        r#"{"session_id":"aaaa1111-0000-4000-8000-000000000001","pid":1,"started_at":100,"state":"exited","exit_code":7,"ended_at":200,"cost_usd":0.13}"#,
    );
    plant_receipt(
        dir.path(),
        "bbbb2222-0000-4000-8000-000000000002",
        r#"{"session_id":"bbbb2222-0000-4000-8000-000000000002","pid":1,"started_at":300,"state":"exited","exit_code":0,"ended_at":400}"#,
    );
    roba()
        .env("ROBA_STATE_DIR", dir.path())
        .arg("jobs")
        .assert()
        .success()
        .stdout(predicate::str::contains("aaaa1111"))
        .stdout(predicate::str::contains("exit 7"))
        .stdout(predicate::str::contains("$0.1300"))
        .stdout(predicate::str::contains("bbbb2222"))
        .stdout(predicate::str::contains("ok"));
}

#[test]
fn jobs_marks_a_dead_pid_running_record_stale() {
    // The SIGKILL case: a `running` record whose pid is gone. Pid
    // 99999999 is beyond real pid spaces on macOS/Linux (unix-only
    // assertion; Windows reports liveness as unknown).
    let dir = tempfile::tempdir().unwrap();
    plant_receipt(
        dir.path(),
        "cccc3333-0000-4000-8000-000000000003",
        r#"{"session_id":"cccc3333-0000-4000-8000-000000000003","pid":99999999,"started_at":100,"state":"running"}"#,
    );
    let assert = roba()
        .env("ROBA_STATE_DIR", dir.path())
        .arg("jobs")
        .assert()
        .success();
    #[cfg(unix)]
    assert.stdout(predicate::str::contains("stale?"));
    #[cfg(not(unix))]
    assert.stdout(predicate::str::contains("running?"));
}

#[test]
fn jobs_json_wears_the_versioned_envelope() {
    let dir = tempfile::tempdir().unwrap();
    plant_receipt(
        dir.path(),
        "dddd4444-0000-4000-8000-000000000004",
        r#"{"session_id":"dddd4444-0000-4000-8000-000000000004","pid":1,"started_at":100,"state":"exited","exit_code":0,"ended_at":150}"#,
    );
    let output = roba()
        .env("ROBA_STATE_DIR", dir.path())
        .args(["jobs", "--json"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let parsed: serde_json::Value = serde_json::from_slice(&output).expect("byte-clean JSON");
    assert_eq!(parsed["version"], 1);
    assert_eq!(parsed["result"][0]["state"], "ok");
    assert_eq!(
        parsed["result"][0]["session_id"],
        "dddd4444-0000-4000-8000-000000000004"
    );
}

#[test]
fn watch_terminal_receipt_completes_immediately_with_failure_exit() {
    // A watched run that already failed: watch prints its line and exits 1
    // (a watched RUN failed; watch itself worked).
    let dir = tempfile::tempdir().unwrap();
    plant_receipt(
        dir.path(),
        "eeee5555-0000-4000-8000-000000000005",
        r#"{"session_id":"eeee5555-0000-4000-8000-000000000005","pid":1,"started_at":100,"state":"exited","exit_code":7,"ended_at":200,"cost_usd":1.5}"#,
    );
    roba()
        .env("ROBA_STATE_DIR", dir.path())
        .args(["watch", "eeee5555"])
        .assert()
        .code(1)
        .stdout(predicate::str::contains("exit 7"))
        .stdout(predicate::str::contains("$1.5000"));
}

#[test]
fn watch_all_ok_exits_0() {
    let dir = tempfile::tempdir().unwrap();
    plant_receipt(
        dir.path(),
        "ffff6666-0000-4000-8000-000000000006",
        r#"{"session_id":"ffff6666-0000-4000-8000-000000000006","pid":1,"started_at":100,"state":"exited","exit_code":0,"ended_at":200}"#,
    );
    roba()
        .env("ROBA_STATE_DIR", dir.path())
        .args(["watch", "ffff6666"])
        .assert()
        .success()
        .stdout(predicate::str::contains("exit 0"));
}

#[test]
fn watch_timeout_on_a_running_receipt_exits_4() {
    // A foreign-pid running record that never finishes: --timeout 1 must
    // surface the typed timeout (exit 4), mirroring `show --wait`.
    let dir = tempfile::tempdir().unwrap();
    plant_receipt(
        dir.path(),
        "9999aaaa-0000-4000-8000-000000000007",
        r#"{"session_id":"9999aaaa-0000-4000-8000-000000000007","pid":1,"started_at":100,"state":"running"}"#,
    );
    roba()
        .env("ROBA_STATE_DIR", dir.path())
        .args(["watch", "9999aaaa", "--timeout", "1"])
        .assert()
        .code(4);
}

#[test]
fn watch_nothing_running_is_a_note_and_exit_0() {
    let dir = tempfile::tempdir().unwrap();
    roba()
        .env("ROBA_STATE_DIR", dir.path())
        .arg("watch")
        .assert()
        .success()
        .stderr(predicate::str::contains("nothing to watch"));
}

#[test]
fn watch_unknown_id_errors_loudly() {
    let dir = tempfile::tempdir().unwrap();
    roba()
        .env("ROBA_STATE_DIR", dir.path())
        .args(["watch", "zzzz"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("no receipt matches"));
}
