//! Process-local lifecycle for one bounded Roba run.

use std::collections::VecDeque;
use std::fmt;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, broadcast, watch};

use crate::provider::{EventSink, Provider, ProviderError, execute_turn};
use crate::run::{
    FailureKind, Prompt, ProviderId, RunEvent, RunFailure, RunOutcome, RunSpec, RunState,
    SessionSpec,
};

/// Read-only view of a live or terminal run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunSnapshot {
    pub state: RunState,
    pub turns_completed: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_outcome: Option<RunOutcome>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<RunFailure>,
}

impl RunSnapshot {
    /// True after no more provider work can start.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self.state,
            RunState::Completed | RunState::Failed | RunState::Cancelled
        )
    }
}

/// Owning library value for one run. Cloned controls are obtained through
/// [`Run::handle`].
pub struct Run {
    handle: RunHandle,
}

impl Run {
    /// Create a run without starting provider work.
    pub fn new(spec: RunSpec, provider: Arc<dyn Provider>) -> Result<Self, RunControlError> {
        let provider_id = provider.id();
        if spec.agent.provider != provider_id {
            return Err(RunControlError::ProviderMismatch {
                selected: spec.agent.provider.clone(),
                adapter: provider_id,
            });
        }
        let state = spec.initial_state();
        let snapshot = RunSnapshot {
            state,
            turns_completed: 0,
            last_outcome: None,
            failure: None,
        };
        let (snapshot_tx, _) = watch::channel(snapshot.clone());
        let (cancel_tx, _) = watch::channel(false);
        let (events, _) = broadcast::channel(256);
        Ok(Self {
            handle: RunHandle {
                inner: Arc::new(Inner {
                    spec,
                    provider,
                    control: Mutex::new(Control {
                        snapshot,
                        started: false,
                        steering: VecDeque::new(),
                    }),
                    snapshot_tx,
                    cancel_tx,
                    events,
                }),
            },
        })
    }

    /// Clone a control and observation handle.
    pub fn handle(&self) -> RunHandle {
        self.handle.clone()
    }

    /// Inspect the immutable resolved specification captured at creation.
    pub fn spec(&self) -> &RunSpec {
        &self.handle.inner.spec
    }

    /// Start a spec that already contains an initial prompt.
    pub async fn begin(&self) -> Result<(), RunControlError> {
        self.handle.begin().await
    }
}

/// Cloneable control and observation handle for one run.
#[derive(Clone)]
pub struct RunHandle {
    inner: Arc<Inner>,
}

impl RunHandle {
    /// Start a suspended run with its first prompt.
    pub async fn start(&self, prompt: Prompt) -> Result<(), RunControlError> {
        let mut control = self.inner.control.lock().await;
        if control.snapshot.state != RunState::Suspended || control.started {
            return Err(RunControlError::AlreadyStarted);
        }
        control.started = true;
        set_state(&self.inner, &mut control, RunState::Running);
        drop(control);
        spawn_driver(self.inner.clone(), prompt);
        Ok(())
    }

    /// Start a ready run from its resolved initial prompt.
    pub async fn begin(&self) -> Result<(), RunControlError> {
        let prompt = self
            .inner
            .spec
            .initial_prompt
            .clone()
            .ok_or(RunControlError::Suspended)?;
        let mut control = self.inner.control.lock().await;
        if control.snapshot.state != RunState::Ready || control.started {
            return Err(RunControlError::AlreadyStarted);
        }
        control.started = true;
        set_state(&self.inner, &mut control, RunState::Running);
        drop(control);
        spawn_driver(self.inner.clone(), prompt);
        Ok(())
    }

    /// Queue guidance for the next safe provider turn boundary.
    pub async fn steer(&self, prompt: Prompt) -> Result<(), RunControlError> {
        if !self.inner.provider.capabilities().resume {
            return Err(RunControlError::SteeringUnsupported);
        }
        let mut control = self.inner.control.lock().await;
        if !matches!(
            control.snapshot.state,
            RunState::Running | RunState::Waiting
        ) {
            return Err(RunControlError::NotRunning);
        }
        control.steering.push_back(prompt);
        let _ = self.inner.events.send(RunEvent::SteeringQueued);
        Ok(())
    }

    /// Return the latest in-memory snapshot.
    pub async fn status(&self) -> RunSnapshot {
        self.inner.control.lock().await.snapshot.clone()
    }

    /// Subscribe to normalized lifecycle and provider events.
    pub fn subscribe(&self) -> broadcast::Receiver<RunEvent> {
        self.inner.events.subscribe()
    }

    /// Cancel a suspended, ready, or running run. Running provider futures are
    /// dropped at the cancellation boundary.
    pub async fn cancel(&self) -> Result<(), RunControlError> {
        let mut control = self.inner.control.lock().await;
        if control.snapshot.is_terminal() {
            return Err(RunControlError::Terminal);
        }
        if matches!(
            control.snapshot.state,
            RunState::Running | RunState::Waiting
        ) {
            self.inner.cancel_tx.send_replace(true);
            return Ok(());
        }
        set_state(&self.inner, &mut control, RunState::Cancelled);
        Ok(())
    }

    /// Wait for a terminal snapshot.
    pub async fn wait(&self) -> RunSnapshot {
        let mut receiver = self.inner.snapshot_tx.subscribe();
        loop {
            let snapshot = receiver.borrow().clone();
            if snapshot.is_terminal() {
                return snapshot;
            }
            if receiver.changed().await.is_err() {
                return receiver.borrow().clone();
            }
        }
    }
}

struct Inner {
    spec: RunSpec,
    provider: Arc<dyn Provider>,
    control: Mutex<Control>,
    snapshot_tx: watch::Sender<RunSnapshot>,
    cancel_tx: watch::Sender<bool>,
    events: broadcast::Sender<RunEvent>,
}

struct Control {
    snapshot: RunSnapshot,
    started: bool,
    steering: VecDeque<Prompt>,
}

struct BroadcastSink {
    events: broadcast::Sender<RunEvent>,
}

impl EventSink for BroadcastSink {
    fn emit(&self, event: RunEvent) {
        let _ = self.events.send(event);
    }
}

fn spawn_driver(inner: Arc<Inner>, prompt: Prompt) {
    tokio::spawn(async move {
        drive(inner, prompt).await;
    });
}

async fn drive(inner: Arc<Inner>, mut prompt: Prompt) {
    let mut session = inner.spec.execution.session.clone();
    let sink = BroadcastSink {
        events: inner.events.clone(),
    };

    loop {
        let mut spec = inner.spec.clone();
        spec.initial_prompt = Some(prompt.clone());
        spec.execution.session = session.clone();
        let request = match spec.into_turn() {
            Ok(request) => request,
            Err(error) => {
                fail(
                    &inner,
                    RunFailure {
                        kind: FailureKind::Provider,
                        message: error.to_string(),
                    },
                )
                .await;
                return;
            }
        };

        let mut cancelled = inner.cancel_tx.subscribe();
        let result = tokio::select! {
            result = execute_turn(inner.provider.as_ref(), request, &sink) => Some(result),
            () = wait_for_cancellation(&mut cancelled) => None,
        };

        let Some(result) = result else {
            let mut control = inner.control.lock().await;
            set_state(&inner, &mut control, RunState::Cancelled);
            return;
        };

        match result {
            Err(error) => {
                fail(&inner, error.into()).await;
                return;
            }
            Ok(outcome) => {
                let mut control = inner.control.lock().await;
                control.snapshot.turns_completed += 1;
                control.snapshot.last_outcome = Some(outcome.clone());
                if let Some(next) = control.steering.pop_front() {
                    let Some(next_session) = outcome.session else {
                        let failure = RunFailure {
                            kind: FailureKind::Provider,
                            message: "provider returned no session handle required for steering"
                                .to_string(),
                        };
                        control.snapshot.failure = Some(failure.clone());
                        set_state(&inner, &mut control, RunState::Failed);
                        let _ = inner.events.send(RunEvent::Failed { failure });
                        return;
                    };
                    session = SessionSpec::Resume {
                        session: next_session,
                    };
                    prompt = next;
                    set_state(&inner, &mut control, RunState::Waiting);
                    set_state(&inner, &mut control, RunState::Running);
                    drop(control);
                    continue;
                }
                set_state(&inner, &mut control, RunState::Completed);
                return;
            }
        }
    }
}

async fn wait_for_cancellation(receiver: &mut watch::Receiver<bool>) {
    loop {
        if *receiver.borrow() {
            return;
        }
        if receiver.changed().await.is_err() {
            return;
        }
    }
}

async fn fail(inner: &Inner, failure: RunFailure) {
    let mut control = inner.control.lock().await;
    control.snapshot.failure = Some(failure.clone());
    set_state(inner, &mut control, RunState::Failed);
    let _ = inner.events.send(RunEvent::Failed { failure });
}

fn set_state(inner: &Inner, control: &mut Control, state: RunState) {
    control.snapshot.state = state;
    inner.snapshot_tx.send_replace(control.snapshot.clone());
    let _ = inner.events.send(RunEvent::StateChanged { state });
}

impl From<ProviderError> for RunFailure {
    fn from(error: ProviderError) -> Self {
        Self {
            kind: error.kind,
            message: error.message,
        }
    }
}

/// Invalid lifecycle operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunControlError {
    ProviderMismatch {
        selected: ProviderId,
        adapter: ProviderId,
    },
    Suspended,
    AlreadyStarted,
    NotRunning,
    SteeringUnsupported,
    Terminal,
}

impl fmt::Display for RunControlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ProviderMismatch { selected, adapter } => write!(
                f,
                "run selects provider {selected}, but adapter is {adapter}"
            ),
            Self::Suspended => f.write_str("run is suspended; supply its initial prompt first"),
            Self::AlreadyStarted => f.write_str("run has already started"),
            Self::NotRunning => f.write_str("run is not accepting steering"),
            Self::SteeringUnsupported => {
                f.write_str("provider cannot resume and does not support steering")
            }
            Self::Terminal => f.write_str("run is already terminal"),
        }
    }
}

impl std::error::Error for RunControlError {}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use tokio::sync::Notify;

    use super::*;
    use crate::provider::{ProviderCapabilities, ProviderFuture};
    use crate::run::{AgentSpec, RunOutcome, SessionHandle, TurnRequest};

    struct RecordingProvider {
        calls: AtomicUsize,
        first_started: Notify,
        release_first: Notify,
        block: bool,
    }

    impl Provider for RecordingProvider {
        fn id(&self) -> ProviderId {
            ProviderId::claude()
        }

        fn capabilities(&self) -> ProviderCapabilities {
            ProviderCapabilities {
                resume: true,
                ..ProviderCapabilities::default()
            }
        }

        fn validate(&self, _request: &crate::TurnRequest) -> Result<(), ProviderError> {
            Ok(())
        }

        fn execute<'a>(
            &'a self,
            request: TurnRequest,
            _events: &'a dyn EventSink,
        ) -> ProviderFuture<'a> {
            Box::pin(async move {
                let call = self.calls.fetch_add(1, Ordering::SeqCst);
                if call == 0 {
                    self.first_started.notify_one();
                    if self.block {
                        self.release_first.notified().await;
                    }
                }
                Ok(RunOutcome {
                    output: request.prompt.into_inner(),
                    session: Some(SessionHandle {
                        provider: ProviderId::claude(),
                        id: "session-1".to_string(),
                    }),
                    usage: None,
                    cost: None,
                    duration_ms: None,
                    provider_turns: None,
                    structured_output: None,
                })
            })
        }
    }

    fn provider(block: bool) -> Arc<RecordingProvider> {
        Arc::new(RecordingProvider {
            calls: AtomicUsize::new(0),
            first_started: Notify::new(),
            release_first: Notify::new(),
            block,
        })
    }

    #[tokio::test]
    async fn suspended_run_does_no_work_until_started() {
        let provider = provider(false);
        let run = Run::new(
            RunSpec::suspended(AgentSpec::new(ProviderId::claude())),
            provider.clone(),
        )
        .unwrap();
        assert_eq!(run.handle().status().await.state, RunState::Suspended);
        assert_eq!(provider.calls.load(Ordering::SeqCst), 0);

        run.handle()
            .start(Prompt::new("hello").unwrap())
            .await
            .unwrap();
        let terminal = run.handle().wait().await;
        assert_eq!(terminal.state, RunState::Completed);
        assert_eq!(terminal.last_outcome.unwrap().output, "hello");
        assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn steering_runs_at_the_next_resumed_boundary() {
        let provider = provider(true);
        let spec = RunSpec::suspended(AgentSpec::new(ProviderId::claude()))
            .with_prompt(Prompt::new("first").unwrap());
        let run = Run::new(spec, provider.clone()).unwrap();
        run.begin().await.unwrap();
        provider.first_started.notified().await;
        run.handle()
            .steer(Prompt::new("second").unwrap())
            .await
            .unwrap();
        provider.release_first.notify_one();

        let terminal = run.handle().wait().await;
        assert_eq!(terminal.state, RunState::Completed);
        assert_eq!(terminal.turns_completed, 2);
        assert_eq!(terminal.last_outcome.unwrap().output, "second");
        assert_eq!(provider.calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn cancellation_drops_the_provider_turn() {
        let provider = provider(true);
        let spec = RunSpec::suspended(AgentSpec::new(ProviderId::claude()))
            .with_prompt(Prompt::new("first").unwrap());
        let run = Run::new(spec, provider.clone()).unwrap();
        run.begin().await.unwrap();
        provider.first_started.notified().await;
        run.handle().cancel().await.unwrap();

        let terminal = run.handle().wait().await;
        assert_eq!(terminal.state, RunState::Cancelled);
        assert_eq!(terminal.turns_completed, 0);
    }

    #[tokio::test]
    async fn first_start_wins() {
        let provider = provider(true);
        let run = Run::new(
            RunSpec::suspended(AgentSpec::new(ProviderId::claude())),
            provider,
        )
        .unwrap();
        let handle = run.handle();
        handle.start(Prompt::new("one").unwrap()).await.unwrap();
        assert_eq!(
            handle.start(Prompt::new("two").unwrap()).await.unwrap_err(),
            RunControlError::AlreadyStarted
        );
        handle.cancel().await.unwrap();
        assert_eq!(handle.wait().await.state, RunState::Cancelled);
    }
}
