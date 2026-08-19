//! Paid smoke tests for the real provider boundaries.
//!
//! These tests are ignored by default. Run them explicitly with:
//!
//! ```text
//! cargo test --test live -- --ignored --nocapture
//! ```

use assert_cmd::Command;

fn provider_workspace() -> tempfile::TempDir {
    let directory = tempfile::tempdir().expect("provider workspace");
    let status = std::process::Command::new("git")
        .args(["init", "--quiet"])
        .current_dir(directory.path())
        .status()
        .expect("start git init");
    assert!(status.success(), "git init failed");
    directory
}

fn run_provider(provider: &str, prompt: &str) -> serde_json::Value {
    let workspace = provider_workspace();
    let output = Command::cargo_bin("roba")
        .expect("cargo-built roba binary")
        .args([
            "-C",
            workspace.path().to_str().expect("UTF-8 workspace path"),
            "run",
            "--no-config",
            "--provider",
            provider,
            "--json",
            prompt,
        ])
        .output()
        .expect("run provider");
    assert!(
        output.status.success(),
        "{provider} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("provider stdout is a JSON envelope")
}

#[test]
#[ignore = "calls real Claude and may cost money"]
fn live_claude_run_returns_a_resumable_terminal_snapshot() {
    let envelope = run_provider("claude", "Reply with exactly: pong");
    assert_eq!(envelope["result"]["state"], "completed");
    assert_eq!(
        envelope["result"]["last_outcome"]["session"]["provider"],
        "claude"
    );
    assert!(
        envelope["result"]["last_outcome"]["session"]["id"].is_string(),
        "Claude did not return a resumable session id: {envelope}"
    );
}

#[test]
#[ignore = "calls real Codex and may cost money"]
fn live_codex_run_returns_a_resumable_terminal_snapshot() {
    let envelope = run_provider("codex", "Reply with exactly: pong");
    assert_eq!(envelope["result"]["state"], "completed");
    assert_eq!(
        envelope["result"]["last_outcome"]["session"]["provider"],
        "codex"
    );
    assert!(
        envelope["result"]["last_outcome"]["session"]["id"].is_string(),
        "Codex did not return a resumable thread id: {envelope}"
    );
}

#[test]
#[ignore = "calls real Codex and may cost money"]
fn live_codex_config_proposal_reads_the_survey_and_uses_the_typed_tool() {
    let workspace = provider_workspace();
    std::fs::write(
        workspace.path().join("Cargo.toml"),
        "[package]\nname='sample'\nversion='0.1.0'\n",
    )
    .unwrap();
    let output = Command::cargo_bin("roba")
        .expect("cargo-built roba binary")
        .args([
            "-C",
            workspace.path().to_str().expect("UTF-8 workspace path"),
            "config",
            "propose",
            "--no-config",
            "--provider",
            "codex",
            "--json",
        ])
        .output()
        .expect("run provider-assisted proposal");
    assert!(
        output.status.success(),
        "Codex proposal failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let envelope: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("proposal stdout is JSON");
    assert_eq!(envelope["result"]["application"], "preview_only");
    assert_eq!(
        envelope["result"]["survey_context"]["entry_id"],
        "roba.config.survey"
    );
    assert!(
        envelope["result"]["survey_context"]["read_count"]
            .as_u64()
            .is_some_and(|count| count > 0)
    );
    let document = envelope["result"]["document"]
        .as_str()
        .expect("validated proposal document");
    let parsed: toml::Value = toml::from_str(document).expect("proposal is strict TOML");
    assert_eq!(parsed["version"].as_integer(), Some(1));
    assert!(!workspace.path().join("roba.toml").exists());
}
