use std::future::pending;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use roba_core::{
    AgentSpec, EventSink, Provider, ProviderCapabilities, ProviderError, ProviderFuture,
    ProviderId, Roba, RunOutcome, RunSpec, TurnRequest,
};
use roba_mcp::{
    AgentControlRefusalKind, AgentEvent, AgentExtension, AgentExtensionChange,
    AgentExtensionFuture, AgentExtensionHookPhase, AgentExtensionLifecycle,
    AgentExtensionOperation, AgentExtensions, AgentFollowUpResult, AgentInstance,
    AgentInterruptResult, AgentTerminalState, AgentTurnResult,
};
use tokio::sync::Semaphore;
use tower_mcp::McpRouter;

const TEST_TIMEOUT: Duration = Duration::from_secs(5);

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

struct HeldProvider {
    state: Arc<ProviderState>,
}

impl Provider for HeldProvider {
    fn id(&self) -> ProviderId {
        provider_id()
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            resume: true,
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
        self.state.calls.fetch_add(1, Ordering::SeqCst);
        self.state.started.add_permits(1);
        let state = Arc::clone(&self.state);
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
    ProviderId::new("extension-lifecycle-test").expect("static provider id is valid")
}

fn agent(provider: Arc<ProviderState>, extensions: AgentExtensions) -> AgentInstance {
    let mut runtime = Roba::new();
    runtime
        .register(HeldProvider { state: provider })
        .expect("test provider registers");
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

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

struct BlockingAdmissionLifecycle {
    entered: Arc<Semaphore>,
    release: Arc<Semaphore>,
    phases: Arc<Mutex<Vec<(AgentExtensionHookPhase, AgentTerminalState)>>>,
    shutdowns: Arc<AtomicUsize>,
}

impl AgentExtensionLifecycle for BlockingAdmissionLifecycle {
    fn operation_admitted(
        &self,
        _operation: AgentExtensionOperation,
    ) -> AgentExtensionFuture<Option<AgentExtensionChange>> {
        let entered = Arc::clone(&self.entered);
        let release = Arc::clone(&self.release);
        Box::pin(async move {
            entered.add_permits(1);
            release
                .acquire()
                .await
                .expect("admission release remains open")
                .forget();
            None
        })
    }

    fn operation_settling(
        &self,
        _operation: AgentExtensionOperation,
        terminal: AgentTerminalState,
    ) -> AgentExtensionFuture<Option<AgentExtensionChange>> {
        let phases = Arc::clone(&self.phases);
        Box::pin(async move {
            lock(&phases).push((AgentExtensionHookPhase::Settling, terminal));
            None
        })
    }

    fn operation_settled(
        &self,
        _operation: AgentExtensionOperation,
        terminal: AgentTerminalState,
    ) -> AgentExtensionFuture<Option<AgentExtensionChange>> {
        let phases = Arc::clone(&self.phases);
        Box::pin(async move {
            lock(&phases).push((AgentExtensionHookPhase::Settled, terminal));
            None
        })
    }

    fn host_shutdown(&self) -> AgentExtensionFuture<()> {
        let shutdowns = Arc::clone(&self.shutdowns);
        Box::pin(async move {
            shutdowns.fetch_add(1, Ordering::SeqCst);
        })
    }
}

#[tokio::test]
async fn admission_precedes_provider_work_and_interrupt_drains_exact_hooks() {
    let provider = Arc::new(ProviderState::default());
    let entered = Arc::new(Semaphore::new(0));
    let release = Arc::new(Semaphore::new(0));
    let phases = Arc::new(Mutex::new(Vec::new()));
    let shutdowns = Arc::new(AtomicUsize::new(0));
    let lifecycle = Arc::new(BlockingAdmissionLifecycle {
        entered: Arc::clone(&entered),
        release: Arc::clone(&release),
        phases: Arc::clone(&phases),
        shutdowns: Arc::clone(&shutdowns),
    });
    let extensions = AgentExtensions::default()
        .try_with(
            AgentExtension::new("blocking", McpRouter::new(), McpRouter::new())
                .with_lifecycle(lifecycle),
        )
        .unwrap();
    let agent = agent(Arc::clone(&provider), extensions);

    let turn_agent = agent.clone();
    let turn = tokio::spawn(async move { turn_agent.turn("work".to_owned()).await });
    take(&entered).await;
    let snapshot = agent.snapshot().await;
    let operation_id = snapshot
        .current_operation_id
        .expect("operation is admitted");
    assert_eq!(provider.calls.load(Ordering::SeqCst), 0);

    let follow_up = agent.follow_up(operation_id, "later".to_owned()).await;
    match follow_up {
        AgentFollowUpResult::Refused { refusal } => {
            assert_eq!(refusal.kind, AgentControlRefusalKind::OperationStarting);
        }
        other => panic!("starting follow-up should be refused, got {other:?}"),
    }

    let interrupt_agent = agent.clone();
    let interrupt = tokio::spawn(async move { interrupt_agent.interrupt(operation_id).await });
    tokio::task::yield_now().await;
    assert_eq!(provider.calls.load(Ordering::SeqCst), 0);
    release.add_permits(1);

    let interrupted = tokio::time::timeout(TEST_TIMEOUT, interrupt)
        .await
        .expect("interrupt settles")
        .expect("interrupt task joins");
    match interrupted {
        AgentInterruptResult::Settled { settlement, .. } => {
            assert_eq!(settlement.state, AgentTerminalState::Cancelled);
        }
        other => panic!("interrupt should settle, got {other:?}"),
    }
    assert!(matches!(
        turn.await.expect("turn task joins"),
        AgentTurnResult::Cancelled { .. }
    ));
    assert_eq!(
        *lock(&phases),
        [
            (
                AgentExtensionHookPhase::Settling,
                AgentTerminalState::Cancelled
            ),
            (
                AgentExtensionHookPhase::Settled,
                AgentTerminalState::Cancelled
            ),
        ]
    );
    assert_eq!(provider.calls.load(Ordering::SeqCst), 0);

    agent.shutdown().await;
    assert_eq!(shutdowns.load(Ordering::SeqCst), 1);
}

struct TickLifecycle {
    started: Arc<Semaphore>,
    tick_entered: Arc<Semaphore>,
    tick_release: Arc<Semaphore>,
    tick_calls: Arc<AtomicUsize>,
    active_ticks: Arc<AtomicUsize>,
    maximum_active_ticks: Arc<AtomicUsize>,
}

impl AgentExtensionLifecycle for TickLifecycle {
    fn poll_interval(&self) -> Option<Duration> {
        Some(Duration::from_secs(1))
    }

    fn operation_started(
        &self,
        _operation: AgentExtensionOperation,
    ) -> AgentExtensionFuture<Option<AgentExtensionChange>> {
        let started = Arc::clone(&self.started);
        Box::pin(async move {
            started.add_permits(1);
            None
        })
    }

    fn observation_tick(
        &self,
        _operation: AgentExtensionOperation,
    ) -> AgentExtensionFuture<Option<AgentExtensionChange>> {
        let entered = Arc::clone(&self.tick_entered);
        let release = Arc::clone(&self.tick_release);
        let calls = Arc::clone(&self.tick_calls);
        let active = Arc::clone(&self.active_ticks);
        let maximum = Arc::clone(&self.maximum_active_ticks);
        Box::pin(async move {
            calls.fetch_add(1, Ordering::SeqCst);
            let current = active.fetch_add(1, Ordering::SeqCst) + 1;
            maximum.fetch_max(current, Ordering::SeqCst);
            entered.add_permits(1);
            release
                .acquire()
                .await
                .expect("tick release remains open")
                .forget();
            active.fetch_sub(1, Ordering::SeqCst);
            Some(AgentExtensionChange::new(
                "git:unsafe!fingerprint",
                "  one\n compact\tchange  ",
            ))
        })
    }
}

#[tokio::test(start_paused = true)]
async fn periodic_ticks_do_not_overlap_and_change_events_precede_settlement() {
    let provider = Arc::new(ProviderState::default());
    let started = Arc::new(Semaphore::new(0));
    let tick_entered = Arc::new(Semaphore::new(0));
    let tick_release = Arc::new(Semaphore::new(0));
    let tick_calls = Arc::new(AtomicUsize::new(0));
    let active_ticks = Arc::new(AtomicUsize::new(0));
    let maximum_active_ticks = Arc::new(AtomicUsize::new(0));
    let lifecycle = Arc::new(TickLifecycle {
        started: Arc::clone(&started),
        tick_entered: Arc::clone(&tick_entered),
        tick_release: Arc::clone(&tick_release),
        tick_calls: Arc::clone(&tick_calls),
        active_ticks,
        maximum_active_ticks: Arc::clone(&maximum_active_ticks),
    });
    let extensions = AgentExtensions::default()
        .try_with(
            AgentExtension::new("git progress", McpRouter::new(), McpRouter::new())
                .with_lifecycle(lifecycle),
        )
        .unwrap();
    let agent = agent(Arc::clone(&provider), extensions);

    let turn_agent = agent.clone();
    let turn = tokio::spawn(async move { turn_agent.turn("work".to_owned()).await });
    take(&started).await;
    take(&provider.started).await;
    tokio::time::advance(Duration::from_secs(1)).await;
    take(&tick_entered).await;
    tokio::time::advance(Duration::from_secs(5)).await;
    tokio::task::yield_now().await;
    assert_eq!(tick_calls.load(Ordering::SeqCst), 1);
    assert_eq!(maximum_active_ticks.load(Ordering::SeqCst), 1);

    tick_release.add_permits(1);
    provider.release.add_permits(1);
    let result = tokio::time::timeout(TEST_TIMEOUT, turn)
        .await
        .expect("turn settles")
        .expect("turn task joins");
    assert!(matches!(result, AgentTurnResult::Completed { .. }));

    let page = agent.event_page(0, 256).await.unwrap();
    let changed = page
        .events
        .iter()
        .find(|record| matches!(record.event, AgentEvent::ExtensionChanged { .. }))
        .expect("tick change is journaled");
    match &changed.event {
        AgentEvent::ExtensionChanged {
            extension,
            phase,
            fingerprint,
            summary,
        } => {
            assert_eq!(extension, "git progress");
            assert_eq!(*phase, AgentExtensionHookPhase::Tick);
            assert_eq!(fingerprint, "gitunsafefingerprint");
            assert_eq!(summary, "one compact change");
        }
        other => panic!("expected extension change, got {other:?}"),
    }
    let settled = page
        .events
        .iter()
        .find(|record| matches!(record.event, AgentEvent::OperationSettled { .. }))
        .expect("operation settlement is journaled");
    assert!(changed.sequence < settled.sequence);
}

enum BrokenKind {
    Panic,
    Timeout,
}

struct BrokenLifecycle(BrokenKind);

impl AgentExtensionLifecycle for BrokenLifecycle {
    fn hook_timeout(&self) -> Duration {
        Duration::from_millis(1)
    }

    fn operation_admitted(
        &self,
        _operation: AgentExtensionOperation,
    ) -> AgentExtensionFuture<Option<AgentExtensionChange>> {
        match self.0 {
            BrokenKind::Panic => Box::pin(async { panic!("extension panic") }),
            BrokenKind::Timeout => Box::pin(pending()),
        }
    }
}

#[tokio::test]
async fn panicked_and_timed_out_hooks_fail_loudly_without_wedging_the_agent() {
    let provider = Arc::new(ProviderState::default());
    let extensions = AgentExtensions::default()
        .try_with(
            AgentExtension::new("panic", McpRouter::new(), McpRouter::new())
                .with_lifecycle(Arc::new(BrokenLifecycle(BrokenKind::Panic))),
        )
        .unwrap()
        .try_with(
            AgentExtension::new("timeout", McpRouter::new(), McpRouter::new())
                .with_lifecycle(Arc::new(BrokenLifecycle(BrokenKind::Timeout))),
        )
        .unwrap();
    let agent = agent(Arc::clone(&provider), extensions);

    let turn_agent = agent.clone();
    let turn = tokio::spawn(async move { turn_agent.turn("work".to_owned()).await });
    take(&provider.started).await;
    provider.release.add_permits(1);
    let result = tokio::time::timeout(TEST_TIMEOUT, turn)
        .await
        .expect("turn settles despite broken hooks")
        .expect("turn task joins");
    assert!(matches!(result, AgentTurnResult::Completed { .. }));

    let page = agent.event_page(0, 256).await.unwrap();
    let failures = page
        .events
        .iter()
        .filter_map(|record| match &record.event {
            AgentEvent::ExtensionFailed {
                extension,
                phase: AgentExtensionHookPhase::Admitted,
            } => Some(extension.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(failures, ["panic", "timeout"]);
}
