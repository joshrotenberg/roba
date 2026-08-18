use std::ffi::OsStr;
use std::fs;
use std::path::Path;
use std::process::{Command, Output};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use roba_core::{
    AgentSpec, EventSink, Provider, ProviderCapabilities, ProviderError, ProviderFuture,
    ProviderId, Roba, RunOutcome, RunSpec, TurnRequest,
};
use roba_git::{
    GIT_PROGRESS_RESOURCE_URI, GitAuthority, GitProgressConfig, GitProgressSnapshot,
    GitProgressState, GitWorkspace,
};
use roba_mcp::{
    AgentEvent, AgentExtensionHookPhase, AgentExtensions, AgentInstance, AgentTurnResult,
    connect_in_process,
};
use tempfile::TempDir;
use tokio::sync::Semaphore;

const TEST_TIMEOUT: Duration = Duration::from_secs(10);

struct Fixture {
    temp: TempDir,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().expect("fixture tempdir");
        let fixture = Self { temp };
        fixture.git_ok(["init", "--quiet"]);
        fixture.git_ok(["config", "user.name", "Roba Test"]);
        fixture.git_ok(["config", "user.email", "roba@example.invalid"]);
        fixture.git_ok(["config", "commit.gpgsign", "false"]);
        fixture.git_ok(["config", "core.autocrlf", "false"]);
        fixture.git_ok(["config", "core.filemode", "false"]);
        fixture.git_ok(["config", "core.fsmonitor", "false"]);
        fixture.write("tracked.txt", "baseline\n");
        fixture.write("rename-source.txt", "rename baseline\n");
        fixture.write("delete-me.txt", "delete baseline\n");
        fixture.git_ok(["add", "--all"]);
        fixture.git_ok(["commit", "--quiet", "--no-gpg-sign", "-m", "baseline"]);
        fixture
    }

    fn root(&self) -> &Path {
        self.temp.path()
    }

    fn write(&self, relative: &str, contents: &str) {
        fs::write(self.root().join(relative), contents).expect("write fixture file");
    }

    fn git_ok<I, S>(&self, args: I) -> Output
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let output = Command::new("git")
            .args(args)
            .current_dir(self.root())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .expect("fixture git executable");
        assert!(
            output.status.success(),
            "fixture git failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }

    fn git_text<I, S>(&self, args: I) -> String
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        String::from_utf8(self.git_ok(args).stdout)
            .expect("Git output is UTF-8")
            .trim()
            .to_owned()
    }
}

struct ProviderState {
    calls: AtomicUsize,
    started: Semaphore,
    release: Semaphore,
}

impl Default for ProviderState {
    fn default() -> Self {
        Self {
            calls: AtomicUsize::new(0),
            started: Semaphore::new(0),
            release: Semaphore::new(0),
        }
    }
}

struct HeldProvider(Arc<ProviderState>);

impl Provider for HeldProvider {
    fn id(&self) -> ProviderId {
        provider_id()
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            read_only: true,
            ..Default::default()
        }
    }

    fn validate(&self, _request: &TurnRequest) -> Result<(), ProviderError> {
        Ok(())
    }

    fn execute<'a>(
        &'a self,
        _request: TurnRequest,
        _events: &'a dyn EventSink,
    ) -> ProviderFuture<'a> {
        self.0.calls.fetch_add(1, Ordering::SeqCst);
        self.0.started.add_permits(1);
        let state = Arc::clone(&self.0);
        Box::pin(async move {
            state
                .release
                .acquire()
                .await
                .expect("provider release remains open")
                .forget();
            Ok(RunOutcome {
                output: "done".to_owned(),
                session: None,
                usage: None,
                cost: None,
                duration_ms: Some(1),
                provider_turns: Some(1),
                structured_output: None,
            })
        })
    }
}

fn provider_id() -> ProviderId {
    ProviderId::new("git-progress-test").expect("static provider id")
}

fn agent(
    fixture: &Fixture,
    provider: Arc<ProviderState>,
    config: GitProgressConfig,
) -> AgentInstance {
    let mut runtime = Roba::new();
    runtime
        .register(HeldProvider(provider))
        .expect("test provider registers");
    let workspace = GitWorkspace::discover(fixture.root()).expect("Git workspace discovers");
    let extensions = AgentExtensions::default()
        .try_with(workspace.extension_with_progress(GitAuthority::ReadOnly, config))
        .expect("Git progress extension composes");
    AgentInstance::new_with_extensions(
        runtime,
        RunSpec::suspended(AgentSpec::new(provider_id())),
        extensions,
    )
    .expect("test agent builds")
}

async fn take(semaphore: &Semaphore) {
    tokio::time::timeout(TEST_TIMEOUT, semaphore.acquire())
        .await
        .expect("test signal arrived")
        .expect("test semaphore remains open")
        .forget();
}

async fn progress(client: &tower_mcp::McpClient) -> GitProgressSnapshot {
    let resource = client
        .read_resource(GIT_PROGRESS_RESOURCE_URI)
        .await
        .expect("cached Git progress resource reads");
    serde_json::from_str(
        resource
            .first_text()
            .expect("progress resource is JSON text"),
    )
    .expect("progress resource follows the public schema")
}

async fn wait_for_fingerprint_change(
    client: &tower_mcp::McpClient,
    previous: &str,
) -> GitProgressSnapshot {
    for _ in 0..1_000 {
        let snapshot = progress(client).await;
        if snapshot.fingerprint.as_deref() != Some(previous) {
            return snapshot;
        }
        tokio::task::yield_now().await;
    }
    panic!("Git progress fingerprint did not change")
}

async fn wait_for_observations(client: &tower_mcp::McpClient, minimum: u64) -> GitProgressSnapshot {
    for _ in 0..10_000 {
        let snapshot = progress(client).await;
        if snapshot.observations >= minimum {
            return snapshot;
        }
        tokio::task::yield_now().await;
    }
    panic!("Git progress did not reach {minimum} observations")
}

#[tokio::test]
async fn progress_tracks_baseline_ticks_and_final_state_without_idle_polling() {
    let fixture = Fixture::new();
    let baseline_head = fixture.git_text(["rev-parse", "HEAD"]);
    let provider = Arc::new(ProviderState::default());
    let agent = agent(
        &fixture,
        Arc::clone(&provider),
        GitProgressConfig {
            poll_interval: Some(Duration::from_millis(1)),
        },
    );
    let client = connect_in_process(agent.clone())
        .await
        .expect("control client connects");

    let turn_agent = agent.clone();
    let turn = tokio::spawn(async move { turn_agent.turn("work".to_owned()).await });
    take(&provider.started).await;

    let baseline = progress(&client).await;
    assert!(baseline.baseline.is_some(), "{baseline:#?}");
    assert_eq!(baseline.state, GitProgressState::Observing);
    assert_eq!(
        baseline
            .baseline
            .as_ref()
            .and_then(|point| point.workspace.head.as_deref()),
        Some(baseline_head.as_str())
    );
    assert_eq!(baseline.baseline, baseline.current);
    let baseline_fingerprint = baseline.fingerprint.clone().expect("baseline fingerprint");
    let baseline_changed_at = baseline.last_changed_at_unix_ms;

    let unchanged = wait_for_observations(&client, 2).await;
    assert_eq!(unchanged.fingerprint, baseline.fingerprint);
    assert_eq!(unchanged.last_changed_at_unix_ms, baseline_changed_at);

    fixture.git_ok(["switch", "-c", "work", "--quiet"]);
    fixture.git_ok(["mv", "rename-source.txt", "renamed.txt"]);
    fs::remove_file(fixture.root().join("delete-me.txt")).expect("delete tracked fixture");
    fixture.write("tracked.txt", "tick change\n");
    fixture.write("untracked.txt", "tick untracked\n");
    let changed = wait_for_fingerprint_change(&client, &baseline_fingerprint).await;
    let changed_point = changed.current.as_ref().expect("changed current point");
    assert_eq!(changed_point.workspace.branch.as_deref(), Some("work"));
    assert_eq!(changed_point.paths.modified, ["tracked.txt"]);
    assert_eq!(changed_point.paths.deleted, ["delete-me.txt"]);
    assert_eq!(changed_point.paths.renamed.len(), 1);
    assert_eq!(changed_point.paths.renamed[0].from, "rename-source.txt");
    assert_eq!(changed_point.paths.renamed[0].to, "renamed.txt");
    assert_eq!(changed_point.paths.untracked, ["untracked.txt"]);
    assert_eq!(changed.commits_since_baseline, []);

    fixture.git_ok(["add", "--all"]);
    fixture.git_ok([
        "commit",
        "--quiet",
        "--no-gpg-sign",
        "-m",
        "operation commit",
    ]);
    let operation_head = fixture.git_text(["rev-parse", "HEAD"]);
    fixture.write("staged.txt", "staged evidence\n");
    fixture.git_ok(["add", "staged.txt"]);
    fixture.write("tracked.txt", "final unstaged evidence\n");
    fixture.write("final-untracked.txt", "raw-secret-must-not-enter-events\n");
    provider.release.add_permits(1);

    let result = tokio::time::timeout(TEST_TIMEOUT, turn)
        .await
        .expect("turn settles")
        .expect("turn task joins");
    assert!(matches!(result, AgentTurnResult::Completed { .. }));

    let final_progress = progress(&client).await;
    assert_eq!(final_progress.state, GitProgressState::Settled);
    let final_point = final_progress
        .current
        .as_ref()
        .expect("final current point");
    assert_eq!(
        final_point.workspace.head.as_deref(),
        Some(operation_head.as_str())
    );
    assert_eq!(final_point.workspace.staged, ["staged.txt"]);
    assert_eq!(final_point.workspace.unstaged, ["tracked.txt"]);
    assert_eq!(final_point.workspace.untracked, ["final-untracked.txt"]);
    assert_eq!(final_progress.commits_since_baseline.len(), 1);
    assert_eq!(
        final_progress.commits_since_baseline[0].object_id,
        operation_head
    );
    assert_eq!(
        final_progress.commits_since_baseline[0].summary,
        "operation commit"
    );

    let page = agent.event_page(0, 256).await.expect("event page reads");
    let changes = page
        .events
        .iter()
        .filter(|record| matches!(record.event, AgentEvent::ExtensionChanged { .. }))
        .collect::<Vec<_>>();
    assert_eq!(changes.len(), 3, "baseline, changed tick, and final commit");
    assert!(matches!(
        changes[0].event,
        AgentEvent::ExtensionChanged {
            phase: AgentExtensionHookPhase::Admitted,
            ..
        }
    ));
    assert!(matches!(
        changes[1].event,
        AgentEvent::ExtensionChanged {
            phase: AgentExtensionHookPhase::Tick,
            ..
        }
    ));
    assert!(matches!(
        changes[2].event,
        AgentEvent::ExtensionChanged {
            phase: AgentExtensionHookPhase::Settling,
            ..
        }
    ));
    assert!(
        !serde_json::to_string(&page)
            .unwrap()
            .contains("raw-secret-must-not-enter-events")
    );

    fs::rename(
        fixture.root().join(".git"),
        fixture.root().join(".git-hidden"),
    )
    .expect("hide Git metadata after settlement");
    assert_eq!(progress(&client).await, final_progress);

    let settled_observations = final_progress.observations;
    for _ in 0..10_000 {
        tokio::task::yield_now().await;
    }
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    let idle = progress(&client).await;
    assert_eq!(idle, final_progress);
    assert_eq!(idle.observations, settled_observations);
}

#[tokio::test]
async fn zero_interval_disables_periodic_sampling_but_keeps_final_refresh() {
    let fixture = Fixture::new();
    let provider = Arc::new(ProviderState::default());
    let agent = agent(
        &fixture,
        Arc::clone(&provider),
        GitProgressConfig::from_interval_secs(0),
    );
    let client = connect_in_process(agent.clone()).await.unwrap();
    let turn_agent = agent.clone();
    let turn = tokio::spawn(async move { turn_agent.turn("work".to_owned()).await });
    take(&provider.started).await;
    let baseline = progress(&client).await;

    fixture.write("tracked.txt", "changed with polling disabled\n");
    for _ in 0..10_000 {
        tokio::task::yield_now().await;
    }
    assert_eq!(progress(&client).await, baseline);

    provider.release.add_permits(1);
    assert!(matches!(
        turn.await.expect("turn task joins"),
        AgentTurnResult::Completed { .. }
    ));
    let settled = progress(&client).await;
    assert_eq!(settled.state, GitProgressState::Settled);
    assert_ne!(settled.fingerprint, baseline.fingerprint);
}
