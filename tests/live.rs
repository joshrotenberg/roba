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
