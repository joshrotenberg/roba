use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

use assert_cmd::Command as AssertCommand;
use predicates::prelude::*;
use serde_json::{Value, json};

fn roba() -> AssertCommand {
    AssertCommand::new(assert_cmd::cargo::cargo_bin!("roba"))
}

fn isolated_roba(project: &tempfile::TempDir) -> AssertCommand {
    let mut command = roba();
    command.env("XDG_CONFIG_HOME", project.path().join("xdg"));
    command
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
    for command in ["init", "run", "serve", "config", "completions"] {
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
    roba().args(["init", "--help"]).assert().success().stdout(
        predicate::str::contains("--agent-role")
            .and(predicate::str::contains("--skill"))
            .and(predicate::str::contains("--prompt"))
            .and(predicate::str::contains("--dry-run"))
            .and(predicate::str::contains("--survey").not())
            .and(predicate::str::contains("--preset").not()),
    );
    roba().args(["run", "--help"]).assert().success().stdout(
        predicate::str::contains("--provider")
            .and(predicate::str::contains("--instruction"))
            .and(predicate::str::contains("--context"))
            .and(predicate::str::contains("--ambient-context"))
            .and(predicate::str::contains("--writable"))
            .and(predicate::str::contains("--resume"))
            .and(predicate::str::contains("<PROMPT>")),
    );
    roba().args(["serve", "--help"]).assert().success().stdout(
        predicate::str::contains("--provider")
            .and(predicate::str::contains("--ambient-context"))
            .and(predicate::str::contains("--writable"))
            .and(predicate::str::contains("stdout is MCP wire data"))
            .and(predicate::str::contains("<PROMPT>").not())
            .and(predicate::str::contains("--json").not()),
    );
    roba().args(["config", "--help"]).assert().success().stdout(
        predicate::str::contains("effective")
            .and(predicate::str::contains("survey"))
            .and(predicate::str::contains("propose")),
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

#[test]
fn init_dry_run_and_install_use_the_same_validated_document() {
    let project = tempfile::tempdir().unwrap();
    let cwd = project.path().to_str().unwrap();
    let preview = isolated_roba(&project)
        .args(["-C", cwd, "init", "--dry-run"])
        .output()
        .unwrap();
    assert!(preview.status.success());
    assert!(preview.stderr.is_empty());
    assert!(!project.path().join("roba.toml").exists());
    let preview_text = std::str::from_utf8(&preview.stdout).unwrap();
    let preview_value: toml::Value = toml::from_str(preview_text).unwrap();
    assert_eq!(preview_value["version"].as_integer(), Some(1));
    assert_eq!(
        preview_value["execution"]["permissions"].as_str(),
        Some("read_only")
    );
    assert!(preview_value.get("agent").is_none());
    assert!(preview_value.get("context").is_none());

    let installed = isolated_roba(&project)
        .args(["-C", cwd, "init"])
        .output()
        .unwrap();
    assert!(installed.status.success());
    assert!(installed.stderr.is_empty());
    let installed_stdout = String::from_utf8(installed.stdout).unwrap();
    assert!(installed_stdout.contains("Created"));
    assert!(installed_stdout.contains("roba config effective"));
    assert!(installed_stdout.contains("roba run"));
    assert!(installed_stdout.contains("roba serve"));
    assert_eq!(
        std::fs::read(project.path().join("roba.toml")).unwrap(),
        preview.stdout
    );

    let effective = isolated_roba(&project)
        .args(["-C", cwd, "config", "effective", "--json"])
        .output()
        .unwrap();
    assert!(effective.status.success());
    let effective: Value = serde_json::from_slice(&effective.stdout).unwrap();
    assert_eq!(effective["result"]["execution"]["permissions"], "read_only");
    assert!(effective["result"]["context"]["selection"].is_null());
}

#[test]
fn init_can_select_existing_managed_catalog_ids() {
    let project = tempfile::tempdir().unwrap();
    let cwd = project.path().to_str().unwrap();
    let output = isolated_roba(&project)
        .args([
            "-C",
            cwd,
            "init",
            "--agent-role",
            "roba.repo-worker",
            "--prompt",
            "roba.issue-worker",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(output.stderr.is_empty());

    let document = std::fs::read_to_string(project.path().join("roba.toml")).unwrap();
    let value: toml::Value = toml::from_str(&document).unwrap();
    assert_eq!(value["context"]["agent"].as_str(), Some("roba.repo-worker"));
    assert_eq!(
        value["context"]["prompts"][0].as_str(),
        Some("roba.issue-worker")
    );
    assert!(!document.contains("bounded repository worker"));

    let effective = isolated_roba(&project)
        .args(["-C", cwd, "config", "effective", "--json"])
        .output()
        .unwrap();
    assert!(effective.status.success());
    let effective: Value = serde_json::from_slice(&effective.stdout).unwrap();
    assert_eq!(
        effective["result"]["context"]["selection"]["agent"],
        "roba.repo-worker"
    );
    assert_eq!(
        effective["result"]["context"]["selection"]["skills"][0],
        "roba.repository-change"
    );
}

#[test]
fn init_refuses_existing_or_invalid_configuration_without_mutation() {
    let project = tempfile::tempdir().unwrap();
    let cwd = project.path().to_str().unwrap();
    let existing = project.path().join(".roba.toml");
    std::fs::write(&existing, "version = 1\n# keep me\n").unwrap();
    isolated_roba(&project)
        .args(["-C", cwd, "init"])
        .assert()
        .failure()
        .code(1)
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains(
            "already contains a recognized Roba config",
        ));
    assert_eq!(
        std::fs::read_to_string(existing).unwrap(),
        "version = 1\n# keep me\n"
    );
    assert!(!project.path().join("roba.toml").exists());

    let fresh = tempfile::tempdir().unwrap();
    isolated_roba(&fresh)
        .args([
            "-C",
            fresh.path().to_str().unwrap(),
            "init",
            "--agent-role",
            "local.missing",
        ])
        .assert()
        .failure()
        .code(1)
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains(
            "catalog reference `local.missing` does not exist",
        ));
    assert!(!fresh.path().join("roba.toml").exists());

    let layered = tempfile::tempdir().unwrap();
    std::fs::create_dir(layered.path().join(".git")).unwrap();
    std::fs::write(
        layered.path().join("roba.toml"),
        "version = 1\n[context.builtins]\nenabled = false\n",
    )
    .unwrap();
    let nested = layered.path().join("nested");
    std::fs::create_dir(&nested).unwrap();
    isolated_roba(&layered)
        .args([
            "-C",
            nested.to_str().unwrap(),
            "init",
            "--agent-role",
            "roba.repo-worker",
        ])
        .assert()
        .failure()
        .code(1)
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains(
            "catalog reference `roba.repo-worker` does not exist",
        ));
    assert!(!nested.join("roba.toml").exists());
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
         [context]\nambient_policy = 'controlled'\nproject = ['fixture']\n\
         [extensions.git]\nenabled = false\nprogress_interval_secs = 0\n",
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
    assert_eq!(value["result"]["context"]["ambient_policy"], "controlled");
    assert_eq!(
        value["result"]["extensions"]["git"]["progress_interval_secs"],
        0
    );
    assert_eq!(value["result"]["sources"][0]["kind"], "project");
    let reported_path = value["result"]["sources"][0]["path"]
        .as_str()
        .expect("config source path is a string");
    assert_eq!(
        value["result"]["provenance"]["agent.provider"][0], reported_path,
        "source and field provenance should name the same config"
    );
    assert_eq!(
        value["result"]["provenance"]["extensions.git.progress_interval_secs"][0],
        reported_path
    );
    assert_eq!(
        std::fs::canonicalize(reported_path).unwrap(),
        std::fs::canonicalize(project.path().join("roba.toml")).unwrap(),
        "reported config path should resolve to the fixture config"
    );
}

#[test]
fn config_effective_reports_content_free_context_diagnostics() {
    let project = project_config(
        "version = 1\n\
         [agent]\nprovider = 'codex'\ninstructions = ['PRIVATE REPEATED', 'PRIVATE REPEATED']\n",
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
    let diagnostics = value["result"]["context"]["diagnostics"]
        .as_array()
        .expect("effective context exposes diagnostics");
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0]["code"], "duplicate_material");
    assert_eq!(diagnostics[0]["severity"], "warning");
    assert_eq!(
        diagnostics[0]["entry_ids"],
        serde_json::json!(["agent.instruction.1", "agent.instruction.2"])
    );
    assert!(
        diagnostics[0]["fingerprint"]
            .as_str()
            .is_some_and(|fingerprint| fingerprint.starts_with("sha256:"))
    );
    assert!(
        !serde_json::to_string(diagnostics)
            .unwrap()
            .contains("PRIVATE REPEATED")
    );
}

#[test]
fn config_survey_is_bounded_content_free_and_does_not_start_a_provider() {
    let project = project_config(
        "version = 1\n\
         [agent]\ninstructions = ['PRIVATE REPEATED', 'PRIVATE REPEATED']\n",
    );
    std::fs::write(project.path().join("README.md"), "PRIVATE README BODY").unwrap();
    std::fs::write(project.path().join("Cargo.toml"), "PRIVATE MANIFEST BODY").unwrap();
    std::fs::write(project.path().join("secrets.env"), "PRIVATE SECRET VALUE").unwrap();

    let output = roba()
        .args([
            "-C",
            project.path().to_str().unwrap(),
            "config",
            "survey",
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
    let survey = &value["result"];
    assert_eq!(value["version"], 1);
    assert_eq!(survey["schema_version"], 1);
    assert_eq!(survey["limits"]["recursive"], false);
    assert_eq!(survey["limits"]["file_contents_included"], false);
    assert_eq!(survey["limits"]["marker_candidates"], 18);
    assert_eq!(survey["limits"]["max_startup_bytes"], 1024 * 1024);
    assert!(
        survey["limits"]["observed_startup_bytes"]
            .as_u64()
            .is_some_and(|bytes| bytes > 0 && bytes < 1024 * 1024)
    );
    assert_eq!(survey["startup"]["configuration"]["provider"], "claude");
    assert_eq!(
        survey["startup"]["context_diagnostics"][0]["code"],
        "duplicate_material"
    );
    assert_eq!(
        survey["workspace"]["markers"]
            .as_array()
            .unwrap()
            .iter()
            .map(|marker| marker["path"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["Cargo.toml", "README.md"]
    );
    assert_eq!(
        survey["workspace"]["ecosystems"],
        serde_json::json!(["rust"])
    );
    assert_eq!(
        survey["workspace"]["marker_root"],
        std::fs::canonicalize(project.path())
            .unwrap()
            .to_string_lossy()
            .as_ref()
    );
    assert_eq!(survey["workspace"]["repository"]["relative_cwd"], ".");
    let stdout = String::from_utf8(output.stdout).unwrap();
    for private in [
        "PRIVATE REPEATED",
        "PRIVATE README BODY",
        "PRIVATE MANIFEST BODY",
        "PRIVATE SECRET VALUE",
        "secrets.env",
    ] {
        assert!(!stdout.contains(private));
    }
}

#[test]
fn config_effective_reports_managed_catalog_metadata_without_bodies() {
    let project = project_config(
        "version = 1\n\
         [context]\nagent = 'local.worker'\n\
         [context.builtins]\nenabled = false\n\
         [[context.definitions]]\nkind = 'agent'\nid = 'local.worker'\ndescription = 'Local worker.'\ninline = 'PRIVATE AGENT BODY'\ndefault_skills = ['local.review']\n\
         [[context.definitions]]\nkind = 'skill'\nid = 'local.review'\ndescription = 'Local review skill.'\npath = '.roba/review.md'\n",
    );
    std::fs::create_dir_all(project.path().join(".roba")).unwrap();
    std::fs::write(project.path().join(".roba/review.md"), "PRIVATE SKILL BODY").unwrap();

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
    let context = &value["result"]["context"];
    assert_eq!(context["agent"], "local.worker");
    assert_eq!(context["builtins"]["enabled"], false);
    assert_eq!(context["selection"]["agent"], "local.worker");
    assert_eq!(context["selection"]["skills"][0], "local.review");
    assert_eq!(context["catalog"]["entries"].as_array().unwrap().len(), 2);
    assert!(
        context["catalog"]["fingerprint"]
            .as_str()
            .unwrap()
            .starts_with("sha256:")
    );
    let review = context["catalog"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["id"] == "local.review")
        .unwrap();
    assert_eq!(review["origin"]["kind"], "project");
    assert_eq!(review["source"]["kind"], "markdown_path");
    assert_eq!(review["source"]["path"], ".roba/review.md");
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(!stdout.contains("PRIVATE AGENT BODY"));
    assert!(!stdout.contains("PRIVATE SKILL BODY"));
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
fn unsupported_hermetic_ambient_context_fails_before_launch() {
    let output = roba()
        .args([
            "run",
            "--no-config",
            "--provider",
            "codex",
            "--ambient-context",
            "hermetic",
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
            .contains("cannot enforce hermetic ambient context")
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

#[test]
fn stdio_serve_projects_configured_managed_prompts_and_catalog_resources() {
    let project = project_config(
        "version = 1\n\
         [context]\nagent = 'local.worker'\nprompts = ['local.task']\n\
         [context.builtins]\nenabled = false\n\
         [[context.definitions]]\nkind = 'agent'\nid = 'local.worker'\ndescription = 'Local worker.'\ninline = 'PRIVATE ROLE BODY'\n\
         [[context.definitions]]\nkind = 'prompt'\nid = 'local.task'\ndescription = 'Do one local task.'\ninline = 'Handle {{target}} carefully.'\narguments = [{ name = 'target', description = 'Task target.', required = true }]\n",
    );
    let mut child = Command::new(assert_cmd::cargo::cargo_bin!("roba"))
        .args(["-C", project.path().to_str().unwrap(), "serve"])
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
                "clientInfo": { "name": "roba-context-cli-test", "version": "1" }
            }
        }),
    );
    let initialized = read_response(&mut output, 1);
    assert_eq!(initialized["result"]["serverInfo"]["name"], "roba-agent");
    write_frame(
        &mut input,
        json!({"jsonrpc":"2.0","method":"notifications/initialized","params":{}}),
    );

    write_frame(
        &mut input,
        json!({"jsonrpc":"2.0","id":2,"method":"prompts/list","params":{}}),
    );
    let prompts = read_response(&mut output, 2);
    assert_eq!(prompts["result"]["prompts"].as_array().unwrap().len(), 1);
    assert_eq!(prompts["result"]["prompts"][0]["name"], "local.task");
    assert_eq!(
        prompts["result"]["prompts"][0]["arguments"][0]["required"],
        true
    );

    write_frame(
        &mut input,
        json!({
            "jsonrpc":"2.0",
            "id":3,
            "method":"prompts/get",
            "params":{"name":"local.task","arguments":{"target":"issue #514"}}
        }),
    );
    let prompt = read_response(&mut output, 3);
    assert_eq!(
        prompt["result"]["messages"][0]["content"]["text"],
        "Handle issue #514 carefully."
    );

    write_frame(
        &mut input,
        json!({
            "jsonrpc":"2.0",
            "id":4,
            "method":"resources/read",
            "params":{"uri":"roba://context/catalog"}
        }),
    );
    let catalog = read_response(&mut output, 4);
    let catalog_text = catalog["result"]["contents"][0]["text"]
        .as_str()
        .expect("catalog resource returns JSON text");
    let catalog_value: Value = serde_json::from_str(catalog_text).unwrap();
    assert_eq!(catalog_value["selection"]["agent"]["id"], "local.worker");
    assert!(!catalog_text.contains("PRIVATE ROLE BODY"));
    assert!(!catalog_text.contains("Handle {{target}} carefully."));

    write_frame(
        &mut input,
        json!({
            "jsonrpc":"2.0",
            "id":5,
            "method":"resources/read",
            "params":{"uri":"roba://context/catalog/artifact?id=local.worker"}
        }),
    );
    let artifact = read_response(&mut output, 5);
    let artifact_text = artifact["result"]["contents"][0]["text"]
        .as_str()
        .expect("artifact resource returns JSON text");
    assert!(artifact_text.contains("PRIVATE ROLE BODY"));

    write_frame(
        &mut input,
        json!({
            "jsonrpc": "2.0",
            "id": 6,
            "method": "tools/call",
            "params": { "name": "agent.shutdown", "arguments": {} }
        }),
    );
    assert!(read_response(&mut output, 6)["result"]["structuredContent"].is_object());
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
