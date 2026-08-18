//! Process-local lifecycle for one finite Roba run.

use std::collections::VecDeque;
use std::fmt;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex as AsyncMutex, watch};

use crate::provider::{
    EventSink, Provider, ProviderError, ProviderEvent, ProviderLaunchContext,
    execute_turn_with_launch_context,
};
use crate::run::{
    FailureKind, Prompt, ProviderId, RunEvent, RunFailure, RunOutcome, RunSpec, RunState,
    SessionSpec,
};

/// Maximum follow-ups retained for one active finite run.
pub const MAX_PENDING_FOLLOW_UPS: usize = 16;

/// Read-only view of a live or terminal run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunSnapshot {
    pub state: RunState,
    /// Wall-clock creation time when the host clock can represent Unix time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at_unix_ms: Option<u64>,
    /// Wall-clock time at which provider work became eligible to start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at_unix_ms: Option<u64>,
    /// Wall-clock time at which the run entered a terminal state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at_unix_ms: Option<u64>,
    /// Monotonic elapsed time from start to terminal settlement.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub elapsed_ms: Option<u64>,
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

/// One sequenced event retained for a run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunEventRecord {
    pub sequence: u64,
    /// Wall-clock occurrence time when the host clock can represent Unix time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub occurred_at_unix_ms: Option<u64>,
    pub event: RunEvent,
}

/// Bounded event page returned for one run cursor.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunEventPage {
    pub events: Vec<RunEventRecord>,
    /// Highest sequence inspected by this page. Supply this as the next cursor.
    pub next_sequence: u64,
    /// Oldest sequence still retained by the bounded journal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oldest_sequence: Option<u64>,
    /// True when the requested cursor predates retained history.
    pub truncated: bool,
    /// True when this run can emit no more events.
    pub terminal: bool,
}

/// Replayable subscription over retained and future run events.
pub struct RunEventSubscription {
    handle: RunHandle,
    cursor: u64,
    reported_truncation_at: Option<u64>,
}

/// One item returned by a replayable event subscription.
#[derive(Debug, Clone, PartialEq)]
pub enum RunEventSubscriptionItem {
    /// The cursor predates retained history. The next call begins replay at the
    /// oldest retained event, if one exists.
    HistoryTruncated { oldest_sequence: Option<u64> },
    /// One retained or newly emitted event.
    Event(Box<RunEventRecord>),
}

/// Maximum records retained for one run and returned in one event page.
pub const RUN_EVENT_CAPACITY: usize = 256;

/// Owning library value for one run. Cloned controls are obtained through
/// [`Run::handle`].
pub struct Run {
    handle: RunHandle,
}

impl Run {
    /// Create a run without starting provider work.
    pub fn new(spec: RunSpec, provider: Arc<dyn Provider>) -> Result<Self, RunControlError> {
        Self::new_with_launch_context(spec, provider, ProviderLaunchContext::default())
    }

    /// Create a run with transient provider launch material without starting
    /// provider work.
    pub fn new_with_launch_context(
        spec: RunSpec,
        provider: Arc<dyn Provider>,
        launch_context: ProviderLaunchContext,
    ) -> Result<Self, RunControlError> {
        let provider_id = provider.id();
        if spec.agent.provider != provider_id {
            return Err(RunControlError::ProviderMismatch {
                selected: spec.agent.provider.clone(),
                adapter: provider_id,
            });
        }

        let snapshot = RunSnapshot {
            state: spec.initial_state(),
            created_at_unix_ms: unix_time_ms(),
            started_at_unix_ms: None,
            finished_at_unix_ms: None,
            elapsed_ms: None,
            turns_completed: 0,
            last_outcome: None,
            failure: None,
        };
        let (snapshot_tx, _) = watch::channel(snapshot.clone());
        let (cancel_tx, _) = watch::channel(false);
        Ok(Self {
            handle: RunHandle {
                inner: Arc::new(Inner {
                    spec,
                    provider,
                    launch_context,
                    control: AsyncMutex::new(Control {
                        snapshot,
                        started: false,
                        started_at: None,
                        follow_ups: VecDeque::new(),
                    }),
                    snapshot_tx,
                    cancel_tx,
                    events: EventJournal::new(),
                }),
            },
        })
    }

    /// Clone a control and observation handle.
    pub fn handle(&self) -> RunHandle {
        self.handle.clone()
    }

    /// Inspect the immutable specification captured at creation.
    pub fn spec(&self) -> &RunSpec {
        &self.handle.inner.spec
    }

    /// Start a specification that already contains an initial prompt.
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
    /// Inspect the immutable specification captured at creation.
    pub fn spec(&self) -> &RunSpec {
        &self.inner.spec
    }

    /// Start a suspended run with its first prompt.
    pub async fn start(&self, prompt: Prompt) -> Result<(), RunControlError> {
        let mut control = self.inner.control.lock().await;
        if control.snapshot.state != RunState::Suspended || control.started {
            return Err(RunControlError::AlreadyStarted);
        }
        control.started = true;
        mark_started(&mut control);
        set_state(&self.inner, &mut control, RunState::Running);
        drop(control);
        spawn_driver(self.inner.clone(), prompt);
        Ok(())
    }

    /// Start a ready run from its captured initial prompt.
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
        mark_started(&mut control);
        set_state(&self.inner, &mut control, RunState::Running);
        drop(control);
        spawn_driver(self.inner.clone(), prompt);
        Ok(())
    }

    /// Queue a follow-up for the next provider-turn boundary.
    pub async fn follow_up(&self, prompt: Prompt) -> Result<(), RunControlError> {
        if !self.inner.provider.capabilities().resume {
            return Err(RunControlError::FollowUpUnsupported);
        }
        let mut control = self.inner.control.lock().await;
        if control.snapshot.state != RunState::Running {
            return Err(RunControlError::NotRunning);
        }
        if control.follow_ups.len() >= MAX_PENDING_FOLLOW_UPS {
            return Err(RunControlError::FollowUpQueueFull {
                maximum: MAX_PENDING_FOLLOW_UPS,
            });
        }
        control.follow_ups.push_back(prompt);
        self.inner.events.emit(RunEvent::FollowUpQueued);
        Ok(())
    }

    /// Compatibility alias for [`RunHandle::follow_up`].
    pub async fn steer(&self, prompt: Prompt) -> Result<(), RunControlError> {
        self.follow_up(prompt).await
    }

    /// Return the latest in-memory snapshot.
    pub async fn status(&self) -> RunSnapshot {
        self.inner.control.lock().await.snapshot.clone()
    }

    /// Subscribe to retained and future normalized events. The first call to
    /// [`RunEventSubscription::next`] replays the oldest retained record.
    pub fn subscribe(&self) -> RunEventSubscription {
        self.subscribe_after(0)
    }

    /// Subscribe after a sequence returned by [`RunHandle::event_page`].
    pub fn subscribe_after(&self, sequence: u64) -> RunEventSubscription {
        RunEventSubscription {
            handle: self.clone(),
            cursor: sequence,
            reported_truncation_at: None,
        }
    }

    /// Read retained events after `sequence` without waiting.
    pub async fn event_page(
        &self,
        sequence: u64,
        limit: usize,
    ) -> Result<RunEventPage, RunControlError> {
        validate_event_limit(limit)?;
        self.event_page_inner(sequence, limit).await
    }

    /// Wait until an event is available, history truncation is known, or the
    /// run is terminal.
    pub async fn wait_for_events(
        &self,
        sequence: u64,
        limit: usize,
    ) -> Result<RunEventPage, RunControlError> {
        validate_event_limit(limit)?;
        let mut changed = self.inner.events.subscribe();
        let mut cursor = sequence;
        loop {
            let page = self.event_page_inner(cursor, limit).await?;
            if !page.events.is_empty() || page.truncated || page.terminal {
                return Ok(page);
            }
            cursor = page.next_sequence;
            if changed.changed().await.is_err() {
                return self.event_page_inner(cursor, limit).await;
            }
        }
    }

    /// Cancel a suspended, ready, or running run. A running provider future is
    /// dropped before the run becomes terminal.
    pub async fn cancel(&self) -> Result<(), RunControlError> {
        let mut control = self.inner.control.lock().await;
        if control.snapshot.is_terminal() {
            return Err(RunControlError::Terminal);
        }
        match control.snapshot.state {
            RunState::Running => {
                set_state(&self.inner, &mut control, RunState::Finishing);
                self.inner.cancel_tx.send_replace(true);
            }
            RunState::Finishing => {}
            _ => set_state(&self.inner, &mut control, RunState::Cancelled),
        }
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

    async fn event_page_inner(
        &self,
        sequence: u64,
        limit: usize,
    ) -> Result<RunEventPage, RunControlError> {
        let retained = self.inner.events.page(sequence, limit)?;
        Ok(RunEventPage {
            events: retained.events,
            next_sequence: retained.next_sequence,
            oldest_sequence: retained.oldest_sequence,
            truncated: retained.truncated,
            terminal: self.status().await.is_terminal(),
        })
    }
}

impl RunEventSubscription {
    /// Last sequence consumed by this subscription.
    pub fn cursor(&self) -> u64 {
        self.cursor
    }

    /// Wait for and return the next retained or future event. Returns `None`
    /// only after the run is terminal and no further event is pending.
    pub async fn next(&mut self) -> Result<Option<RunEventSubscriptionItem>, RunControlError> {
        loop {
            let page = self.handle.wait_for_events(self.cursor, 1).await?;
            if page.truncated && self.reported_truncation_at != Some(self.cursor) {
                self.reported_truncation_at = Some(self.cursor);
                return Ok(Some(RunEventSubscriptionItem::HistoryTruncated {
                    oldest_sequence: page.oldest_sequence,
                }));
            }
            let previous = self.cursor;
            self.cursor = page.next_sequence;
            if self.cursor != previous {
                self.reported_truncation_at = None;
            }
            if let Some(record) = page.events.into_iter().next() {
                return Ok(Some(RunEventSubscriptionItem::Event(Box::new(record))));
            }
            if page.terminal {
                return Ok(None);
            }
        }
    }
}

struct Inner {
    spec: RunSpec,
    provider: Arc<dyn Provider>,
    launch_context: ProviderLaunchContext,
    control: AsyncMutex<Control>,
    snapshot_tx: watch::Sender<RunSnapshot>,
    cancel_tx: watch::Sender<bool>,
    events: EventJournal,
}

struct Control {
    snapshot: RunSnapshot,
    started: bool,
    started_at: Option<Instant>,
    follow_ups: VecDeque<Prompt>,
}

struct EventJournal {
    state: StdMutex<EventJournalState>,
    revision: watch::Sender<u64>,
}

struct EventJournalState {
    next_sequence: u64,
    records: VecDeque<RunEventRecord>,
    evicted_through: u64,
}

struct RetainedEventPage {
    events: Vec<RunEventRecord>,
    next_sequence: u64,
    oldest_sequence: Option<u64>,
    truncated: bool,
}

impl EventJournal {
    fn new() -> Self {
        let (revision, _) = watch::channel(0);
        Self {
            state: StdMutex::new(EventJournalState {
                next_sequence: 1,
                records: VecDeque::with_capacity(RUN_EVENT_CAPACITY),
                evicted_through: 0,
            }),
            revision,
        }
    }

    fn emit(&self, event: RunEvent) {
        let sequence = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let sequence = state.next_sequence;
            state.next_sequence = state
                .next_sequence
                .checked_add(1)
                .expect("run event sequence exhausted");
            if state.records.len() == RUN_EVENT_CAPACITY
                && let Some(evicted) = state.records.pop_front()
            {
                state.evicted_through = state.evicted_through.max(evicted.sequence);
            }
            state.records.push_back(RunEventRecord {
                sequence,
                occurred_at_unix_ms: unix_time_ms(),
                event,
            });
            sequence
        };
        self.revision.send_replace(sequence);
    }

    fn subscribe(&self) -> watch::Receiver<u64> {
        self.revision.subscribe()
    }

    fn page(&self, after: u64, limit: usize) -> Result<RetainedEventPage, RunControlError> {
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let newest = state.next_sequence.saturating_sub(1);
        if after > newest {
            return Err(RunControlError::EventCursorAhead {
                requested: after,
                newest,
            });
        }
        let events = state
            .records
            .iter()
            .filter(|record| record.sequence > after)
            .take(limit)
            .cloned()
            .collect::<Vec<_>>();
        let next_sequence = events
            .last()
            .map(|record| record.sequence)
            .unwrap_or(newest);
        Ok(RetainedEventPage {
            events,
            next_sequence,
            oldest_sequence: state.records.front().map(|record| record.sequence),
            truncated: after < state.evicted_through,
        })
    }
}

struct JournalSink<'a> {
    events: &'a EventJournal,
}

impl EventSink for JournalSink<'_> {
    fn emit(&self, event: ProviderEvent) {
        self.events.emit(match event {
            ProviderEvent::OutputDelta { text } => RunEvent::OutputDelta { text },
            ProviderEvent::Usage { usage } => RunEvent::Usage { usage },
            ProviderEvent::Warning { message } => RunEvent::Warning { message },
        });
    }
}

fn spawn_driver(inner: Arc<Inner>, prompt: Prompt) {
    tokio::spawn(async move {
        let driver_inner = Arc::clone(&inner);
        let result = tokio::spawn(async move {
            drive(driver_inner, prompt).await;
        })
        .await;
        if let Err(error) = result {
            let message = if error.is_panic() {
                "run driver panicked"
            } else {
                "run driver stopped unexpectedly"
            };
            fail(
                &inner,
                RunFailure {
                    kind: FailureKind::Provider,
                    message: message.to_string(),
                    details: Default::default(),
                },
            )
            .await;
        }
    });
}

async fn drive(inner: Arc<Inner>, mut prompt: Prompt) {
    let mut session = inner.spec.execution.session.clone();
    let sink = JournalSink {
        events: &inner.events,
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
                        details: Default::default(),
                    },
                )
                .await;
                return;
            }
        };

        let mut cancelled = inner.cancel_tx.subscribe();
        inner.events.emit(RunEvent::TurnStarted {
            provider: request.spec.agent.provider.clone(),
        });
        let result = tokio::select! {
            result = execute_turn_with_launch_context(
                inner.provider.as_ref(),
                request,
                inner.launch_context.clone(),
                &sink,
            ) => Some(result),
            () = wait_for_cancellation(&mut cancelled) => None,
        };

        let Some(result) = result else {
            let mut control = inner.control.lock().await;
            set_state(&inner, &mut control, RunState::Cancelled);
            return;
        };

        let mut control = inner.control.lock().await;
        if control.snapshot.state == RunState::Finishing || *inner.cancel_tx.borrow() {
            set_state(&inner, &mut control, RunState::Cancelled);
            return;
        }

        match result {
            Err(error) => {
                let failure = RunFailure::from(error);
                control.snapshot.failure = Some(failure.clone());
                inner.events.emit(RunEvent::Failed { failure });
                set_state(&inner, &mut control, RunState::Failed);
                return;
            }
            Ok(outcome) => {
                control.snapshot.turns_completed += 1;
                control.snapshot.last_outcome = Some(outcome.clone());
                inner.events.emit(RunEvent::TurnCompleted {
                    outcome: outcome.clone(),
                });
                if let Some(next) = control.follow_ups.pop_front() {
                    let next_session = outcome.session.or_else(|| match &session {
                        SessionSpec::Fresh => None,
                        SessionSpec::Resume { session } => Some(session.clone()),
                    });
                    let Some(next_session) = next_session else {
                        let failure = RunFailure {
                            kind: FailureKind::Provider,
                            message: "provider returned no session handle required for a follow-up"
                                .to_string(),
                            details: Default::default(),
                        };
                        drop(control);
                        fail(&inner, failure).await;
                        return;
                    };
                    session = SessionSpec::Resume {
                        session: next_session,
                    };
                    prompt = next;
                    inner.events.emit(RunEvent::FollowUpApplied);
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
    if control.snapshot.is_terminal() {
        return;
    }
    if control.snapshot.state == RunState::Finishing || *inner.cancel_tx.borrow() {
        set_state(inner, &mut control, RunState::Cancelled);
        return;
    }
    control.snapshot.failure = Some(failure.clone());
    inner.events.emit(RunEvent::Failed { failure });
    set_state(inner, &mut control, RunState::Failed);
}

fn set_state(inner: &Inner, control: &mut Control, state: RunState) {
    control.snapshot.state = state;
    if control.snapshot.is_terminal() && control.snapshot.finished_at_unix_ms.is_none() {
        control.snapshot.finished_at_unix_ms = unix_time_ms();
        control.snapshot.elapsed_ms = control
            .started_at
            .map(|started| u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX));
    }
    inner.events.emit(RunEvent::StateChanged { state });
    inner.snapshot_tx.send_replace(control.snapshot.clone());
}

fn mark_started(control: &mut Control) {
    control.snapshot.started_at_unix_ms = unix_time_ms();
    control.started_at = Some(Instant::now());
}

fn unix_time_ms() -> Option<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
}

fn validate_event_limit(limit: usize) -> Result<(), RunControlError> {
    if (1..=RUN_EVENT_CAPACITY).contains(&limit) {
        Ok(())
    } else {
        Err(RunControlError::InvalidEventLimit {
            maximum: RUN_EVENT_CAPACITY,
        })
    }
}

impl From<ProviderError> for RunFailure {
    fn from(error: ProviderError) -> Self {
        Self {
            kind: error.kind,
            message: error.message,
            details: error.details.map(|details| *details).unwrap_or_default(),
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
    FollowUpUnsupported,
    FollowUpQueueFull {
        maximum: usize,
    },
    Terminal,
    InvalidEventLimit {
        maximum: usize,
    },
    EventCursorAhead {
        requested: u64,
        newest: u64,
    },
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
            Self::NotRunning => f.write_str("run is not accepting follow-ups"),
            Self::FollowUpUnsupported => {
                f.write_str("provider cannot resume and does not support follow-ups")
            }
            Self::FollowUpQueueFull { maximum } => {
                write!(f, "follow-up queue is full (maximum {maximum})")
            }
            Self::Terminal => f.write_str("run is already terminal"),
            Self::InvalidEventLimit { maximum } => {
                write!(f, "event page limit must be between 1 and {maximum}")
            }
            Self::EventCursorAhead { requested, newest } => write!(
                f,
                "event cursor {requested} is ahead of newest sequence {newest}"
            ),
        }
    }
}

impl std::error::Error for RunControlError {}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use tokio::sync::Notify;

    use super::*;
    use crate::provider::{ProviderCapabilities, ProviderFuture};
    use crate::run::{AgentSpec, Cost, RunFailureDetails, SessionHandle, TokenUsage, TurnRequest};

    struct RecordingProvider {
        calls: AtomicUsize,
        first_started: Notify,
        release_first: Notify,
        block: bool,
        failure: Option<ProviderError>,
        launch_contexts: StdMutex<Vec<ProviderLaunchContext>>,
    }

    struct SessionlessProvider {
        calls: AtomicUsize,
        first_started: Notify,
        release_first: Notify,
        sessions: StdMutex<Vec<SessionSpec>>,
    }

    impl Provider for SessionlessProvider {
        fn id(&self) -> ProviderId {
            ProviderId::claude()
        }

        fn capabilities(&self) -> ProviderCapabilities {
            ProviderCapabilities {
                resume: true,
                ..ProviderCapabilities::default()
            }
        }

        fn validate(&self, _request: &TurnRequest) -> Result<(), ProviderError> {
            Ok(())
        }

        fn execute<'a>(
            &'a self,
            request: TurnRequest,
            _events: &'a dyn EventSink,
        ) -> ProviderFuture<'a> {
            Box::pin(async move {
                self.sessions
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(request.spec.execution.session.clone());
                let call = self.calls.fetch_add(1, Ordering::SeqCst);
                if call == 0 {
                    self.first_started.notify_one();
                    self.release_first.notified().await;
                }
                Ok(RunOutcome {
                    output: request.prompt.into_inner(),
                    session: None,
                    usage: None,
                    cost: None,
                    duration_ms: None,
                    provider_turns: None,
                    structured_output: None,
                })
            })
        }
    }

    struct PanickingProvider;

    impl Provider for PanickingProvider {
        fn id(&self) -> ProviderId {
            ProviderId::claude()
        }

        fn capabilities(&self) -> ProviderCapabilities {
            ProviderCapabilities::default()
        }

        fn validate(&self, _request: &TurnRequest) -> Result<(), ProviderError> {
            Ok(())
        }

        fn execute<'a>(
            &'a self,
            _request: TurnRequest,
            _events: &'a dyn EventSink,
        ) -> ProviderFuture<'a> {
            Box::pin(async move {
                panic!("provider future panicked");
            })
        }
    }

    struct EventfulProvider;

    impl Provider for EventfulProvider {
        fn id(&self) -> ProviderId {
            ProviderId::claude()
        }

        fn capabilities(&self) -> ProviderCapabilities {
            ProviderCapabilities::default()
        }

        fn validate(&self, _request: &TurnRequest) -> Result<(), ProviderError> {
            Ok(())
        }

        fn execute<'a>(
            &'a self,
            request: TurnRequest,
            events: &'a dyn EventSink,
        ) -> ProviderFuture<'a> {
            Box::pin(async move {
                events.emit(ProviderEvent::OutputDelta {
                    text: "chunk".to_string(),
                });
                events.emit(ProviderEvent::Warning {
                    message: "provider warning".to_string(),
                });
                Ok(RunOutcome {
                    output: request.prompt.into_inner(),
                    session: None,
                    usage: None,
                    cost: None,
                    duration_ms: None,
                    provider_turns: None,
                    structured_output: None,
                })
            })
        }
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

        fn validate(&self, _request: &TurnRequest) -> Result<(), ProviderError> {
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
                if let Some(error) = &self.failure {
                    return Err(error.clone());
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

        fn execute_with_launch_context<'a>(
            &'a self,
            request: TurnRequest,
            launch_context: ProviderLaunchContext,
            events: &'a dyn EventSink,
        ) -> ProviderFuture<'a> {
            self.launch_contexts
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(launch_context);
            self.execute(request, events)
        }
    }

    fn provider(block: bool) -> Arc<RecordingProvider> {
        Arc::new(RecordingProvider {
            calls: AtomicUsize::new(0),
            first_started: Notify::new(),
            release_first: Notify::new(),
            block,
            failure: None,
            launch_contexts: StdMutex::new(Vec::new()),
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
        assert!(terminal.started_at_unix_ms.is_some());
        assert!(terminal.finished_at_unix_ms.is_some());
        assert!(terminal.elapsed_ms.is_some());
        assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn follow_up_runs_at_the_next_resumed_boundary() {
        let provider = provider(true);
        let spec = RunSpec::suspended(AgentSpec::new(ProviderId::claude()))
            .with_prompt(Prompt::new("first").unwrap());
        let run = Run::new(spec, provider.clone()).unwrap();
        run.begin().await.unwrap();
        provider.first_started.notified().await;
        run.handle()
            .follow_up(Prompt::new("second").unwrap())
            .await
            .unwrap();
        provider.release_first.notify_one();

        let terminal = run.handle().wait().await;
        assert_eq!(terminal.state, RunState::Completed);
        assert_eq!(terminal.turns_completed, 2);
        assert_eq!(terminal.last_outcome.unwrap().output, "second");
        assert_eq!(provider.calls.load(Ordering::SeqCst), 2);

        let events = run
            .handle()
            .event_page(0, RUN_EVENT_CAPACITY)
            .await
            .unwrap()
            .events;
        let queued = events
            .iter()
            .position(|record| record.event == RunEvent::FollowUpQueued)
            .unwrap();
        let applied = events
            .iter()
            .position(|record| record.event == RunEvent::FollowUpApplied)
            .unwrap();
        let second_started = events
            .iter()
            .enumerate()
            .filter(|(_, record)| matches!(record.event, RunEvent::TurnStarted { .. }))
            .nth(1)
            .map(|(index, _)| index)
            .unwrap();
        assert!(queued < applied);
        assert!(applied < second_started);
    }

    #[tokio::test]
    async fn follow_up_queue_is_bounded_and_fifo() {
        let provider = provider(true);
        let run = Run::new(
            RunSpec::suspended(AgentSpec::new(ProviderId::claude()))
                .with_prompt(Prompt::new("first").unwrap()),
            provider.clone(),
        )
        .unwrap();
        run.begin().await.unwrap();
        provider.first_started.notified().await;
        for index in 0..MAX_PENDING_FOLLOW_UPS {
            run.handle()
                .follow_up(Prompt::new(format!("follow-up {index}")).unwrap())
                .await
                .unwrap();
        }
        assert_eq!(
            run.handle()
                .follow_up(Prompt::new("overflow").unwrap())
                .await
                .unwrap_err(),
            RunControlError::FollowUpQueueFull {
                maximum: MAX_PENDING_FOLLOW_UPS,
            }
        );
        provider.release_first.notify_one();

        let terminal = run.handle().wait().await;
        assert_eq!(terminal.state, RunState::Completed);
        assert_eq!(terminal.turns_completed, MAX_PENDING_FOLLOW_UPS as u32 + 1);
        assert_eq!(
            terminal.last_outcome.unwrap().output,
            format!("follow-up {}", MAX_PENDING_FOLLOW_UPS - 1)
        );
    }

    #[tokio::test]
    async fn launch_context_is_reused_across_resumed_turns_in_one_run() {
        let provider = provider(true);
        let context = ProviderLaunchContext::default()
            .try_with_mcp_endpoint(
                crate::provider::ProviderMcpEndpoint::new(
                    "roba",
                    "http://127.0.0.1:4123/mcp",
                    "run-scoped-token",
                )
                .unwrap(),
            )
            .unwrap();
        let spec = RunSpec::suspended(AgentSpec::new(ProviderId::claude()))
            .with_prompt(Prompt::new("first").unwrap());
        let run = Run::new_with_launch_context(spec, provider.clone(), context.clone()).unwrap();
        run.begin().await.unwrap();
        provider.first_started.notified().await;
        run.handle()
            .follow_up(Prompt::new("second").unwrap())
            .await
            .unwrap();
        provider.release_first.notify_one();

        let terminal = run.handle().wait().await;
        assert_eq!(terminal.state, RunState::Completed);
        assert_eq!(terminal.turns_completed, 2);
        assert_eq!(
            provider
                .launch_contexts
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_slice(),
            [context.clone(), context]
        );
    }

    #[tokio::test]
    async fn resumed_run_reuses_known_session_when_outcome_omits_it() {
        let provider = Arc::new(SessionlessProvider {
            calls: AtomicUsize::new(0),
            first_started: Notify::new(),
            release_first: Notify::new(),
            sessions: StdMutex::new(Vec::new()),
        });
        let known = SessionHandle {
            provider: ProviderId::claude(),
            id: "known-session".to_string(),
        };
        let mut spec = RunSpec::suspended(AgentSpec::new(ProviderId::claude()))
            .with_prompt(Prompt::new("first").unwrap());
        spec.execution.session = SessionSpec::Resume {
            session: known.clone(),
        };
        let run = Run::new(spec, provider.clone()).unwrap();
        run.begin().await.unwrap();
        provider.first_started.notified().await;
        run.handle()
            .follow_up(Prompt::new("second").unwrap())
            .await
            .unwrap();
        provider.release_first.notify_one();

        let terminal = run.handle().wait().await;
        assert_eq!(terminal.state, RunState::Completed);
        assert_eq!(terminal.turns_completed, 2);
        let sessions = provider
            .sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(
            sessions.as_slice(),
            [
                SessionSpec::Resume {
                    session: known.clone()
                },
                SessionSpec::Resume { session: known }
            ]
        );
    }

    #[tokio::test]
    async fn fresh_run_without_reported_session_cannot_steer() {
        let provider = Arc::new(SessionlessProvider {
            calls: AtomicUsize::new(0),
            first_started: Notify::new(),
            release_first: Notify::new(),
            sessions: StdMutex::new(Vec::new()),
        });
        let run = Run::new(
            RunSpec::suspended(AgentSpec::new(ProviderId::claude()))
                .with_prompt(Prompt::new("first").unwrap()),
            provider.clone(),
        )
        .unwrap();
        run.begin().await.unwrap();
        provider.first_started.notified().await;
        run.handle()
            .follow_up(Prompt::new("second").unwrap())
            .await
            .unwrap();
        provider.release_first.notify_one();

        let terminal = run.handle().wait().await;
        assert_eq!(terminal.state, RunState::Failed);
        assert_eq!(terminal.turns_completed, 1);
        assert!(
            terminal
                .failure
                .unwrap()
                .message
                .contains("no session handle")
        );
        assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn provider_panic_settles_failed_instead_of_stranding_waiters() {
        let run = Run::new(
            RunSpec::suspended(AgentSpec::new(ProviderId::claude()))
                .with_prompt(Prompt::new("hello").unwrap()),
            Arc::new(PanickingProvider),
        )
        .unwrap();
        run.begin().await.unwrap();

        let terminal = tokio::time::timeout(Duration::from_secs(1), run.handle().wait())
            .await
            .expect("panicked driver did not settle");
        assert_eq!(terminal.state, RunState::Failed);
        let failure = terminal.failure.unwrap();
        assert_eq!(failure.kind, FailureKind::Provider);
        assert_eq!(failure.message, "run driver panicked");
    }

    #[tokio::test]
    async fn lifecycle_wraps_provider_observations_in_authoritative_boundaries() {
        let run = Run::new(
            RunSpec::suspended(AgentSpec::new(ProviderId::claude()))
                .with_prompt(Prompt::new("hello").unwrap()),
            Arc::new(EventfulProvider),
        )
        .unwrap();
        run.begin().await.unwrap();
        assert_eq!(run.handle().wait().await.state, RunState::Completed);

        let page = run
            .handle()
            .event_page(0, RUN_EVENT_CAPACITY)
            .await
            .unwrap();
        let events = page
            .events
            .iter()
            .map(|record| &record.event)
            .collect::<Vec<_>>();
        assert!(matches!(
            events.as_slice(),
            [
                RunEvent::StateChanged {
                    state: RunState::Running
                },
                RunEvent::TurnStarted { .. },
                RunEvent::OutputDelta { text },
                RunEvent::Warning { message },
                RunEvent::TurnCompleted { .. },
                RunEvent::StateChanged {
                    state: RunState::Completed
                }
            ] if text == "chunk" && message == "provider warning"
        ));
    }

    #[tokio::test]
    async fn cancellation_drops_the_provider_turn_before_settling() {
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
        assert!(terminal.finished_at_unix_ms.is_some());
    }

    #[tokio::test]
    async fn cancellation_that_acquires_control_before_settlement_wins() {
        let provider = provider(true);
        let run = Run::new(
            RunSpec::suspended(AgentSpec::new(ProviderId::claude()))
                .with_prompt(Prompt::new("first").unwrap()),
            provider.clone(),
        )
        .unwrap();
        run.begin().await.unwrap();
        provider.first_started.notified().await;

        // Hold settlement at the control lock. Queue cancellation first, then
        // make the provider outcome ready. Once the lock opens, cancel must
        // linearize before the already-selected provider result.
        let guard = run.handle.inner.control.lock().await;
        let handle = run.handle();
        let cancellation = tokio::spawn(async move { handle.cancel().await });
        tokio::task::yield_now().await;
        provider.release_first.notify_one();
        tokio::task::yield_now().await;
        drop(guard);

        cancellation.await.unwrap().unwrap();
        assert_eq!(run.handle().wait().await.state, RunState::Cancelled);
        assert_eq!(run.handle().status().await.turns_completed, 0);
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

    #[tokio::test]
    async fn event_pages_are_bounded_replayable_and_reject_future_cursors() {
        let provider = provider(false);
        let run = Run::new(
            RunSpec::suspended(AgentSpec::new(ProviderId::claude()))
                .with_prompt(Prompt::new("hello").unwrap()),
            provider,
        )
        .unwrap();
        run.begin().await.unwrap();
        let terminal = run.handle().wait().await;
        assert!(terminal.is_terminal());

        let first = run.handle().event_page(0, 1).await.unwrap();
        assert_eq!(first.events.len(), 1);
        let rest = run
            .handle()
            .event_page(first.next_sequence, RUN_EVENT_CAPACITY)
            .await
            .unwrap();
        assert!(!rest.events.is_empty());
        assert!(rest.terminal);
        assert_eq!(
            run.handle()
                .event_page(rest.next_sequence + 1, 1)
                .await
                .unwrap_err(),
            RunControlError::EventCursorAhead {
                requested: rest.next_sequence + 1,
                newest: rest.next_sequence,
            }
        );

        let mut subscription = run.handle().subscribe();
        let mut replayed = Vec::new();
        while let Some(item) = subscription.next().await.unwrap() {
            match item {
                RunEventSubscriptionItem::Event(record) => replayed.push(*record),
                RunEventSubscriptionItem::HistoryTruncated { .. } => {
                    panic!("complete retained history must not report a gap")
                }
            }
        }
        assert_eq!(replayed.len(), first.events.len() + rest.events.len());
    }

    #[tokio::test]
    async fn event_history_reports_truncation_once_then_replays_retained_records() {
        let provider = provider(true);
        let run = Run::new(
            RunSpec::suspended(AgentSpec::new(ProviderId::claude()))
                .with_prompt(Prompt::new("hello").unwrap()),
            provider.clone(),
        )
        .unwrap();
        run.begin().await.unwrap();
        provider.first_started.notified().await;
        for index in 0..=RUN_EVENT_CAPACITY {
            run.handle().inner.events.emit(RunEvent::Warning {
                message: format!("warning {index}"),
            });
        }

        let page = run.handle().event_page(0, 1).await.unwrap();
        assert!(page.truncated);
        assert_eq!(page.events.len(), 1);

        let mut subscription = run.handle().subscribe();
        assert!(matches!(
            subscription.next().await.unwrap(),
            Some(RunEventSubscriptionItem::HistoryTruncated {
                oldest_sequence: Some(_)
            })
        ));
        assert!(matches!(
            subscription.next().await.unwrap(),
            Some(RunEventSubscriptionItem::Event(_))
        ));

        run.handle().cancel().await.unwrap();
        assert_eq!(run.handle().wait().await.state, RunState::Cancelled);
    }

    #[tokio::test]
    async fn provider_failure_details_survive_lifecycle_settlement() {
        let failure = ProviderError::new(FailureKind::MaxTurns, "turn limit reached").with_details(
            RunFailureDetails {
                session: Some(SessionHandle {
                    provider: ProviderId::claude(),
                    id: "resume-me".to_string(),
                }),
                usage: Some(TokenUsage {
                    output: Some(9),
                    ..TokenUsage::default()
                }),
                cost: Some(Cost::usd(1.25)),
                duration_ms: Some(12),
                provider_turns: Some(30),
            },
        );
        let provider = Arc::new(RecordingProvider {
            calls: AtomicUsize::new(0),
            first_started: Notify::new(),
            release_first: Notify::new(),
            block: false,
            failure: Some(failure),
            launch_contexts: StdMutex::new(Vec::new()),
        });
        let run = Run::new(
            RunSpec::suspended(AgentSpec::new(ProviderId::claude()))
                .with_prompt(Prompt::new("hello").unwrap()),
            provider,
        )
        .unwrap();
        run.begin().await.unwrap();
        let terminal = run.handle().wait().await;

        assert_eq!(terminal.state, RunState::Failed);
        let failure = terminal.failure.unwrap();
        assert_eq!(failure.kind, FailureKind::MaxTurns);
        assert_eq!(failure.details.session.unwrap().id, "resume-me");
        assert_eq!(failure.details.cost.unwrap().amount, 1.25);
        assert_eq!(failure.details.provider_turns, Some(30));
    }

    #[tokio::test]
    async fn failure_subscription_drains_terminal_events_before_ending() {
        let provider = Arc::new(RecordingProvider {
            calls: AtomicUsize::new(0),
            first_started: Notify::new(),
            release_first: Notify::new(),
            block: false,
            failure: Some(ProviderError::new(
                FailureKind::Provider,
                "provider stopped",
            )),
            launch_contexts: StdMutex::new(Vec::new()),
        });
        let run = Run::new(
            RunSpec::suspended(AgentSpec::new(ProviderId::claude()))
                .with_prompt(Prompt::new("hello").unwrap()),
            provider,
        )
        .unwrap();
        let mut subscription = run.handle().subscribe();
        run.begin().await.unwrap();

        let mut events = Vec::new();
        while let Some(item) = subscription.next().await.unwrap() {
            if let RunEventSubscriptionItem::Event(record) = item {
                events.push(record.event);
            }
        }

        assert!(matches!(
            events.as_slice(),
            [
                RunEvent::StateChanged {
                    state: RunState::Running
                },
                RunEvent::TurnStarted { .. },
                RunEvent::Failed { .. },
                RunEvent::StateChanged {
                    state: RunState::Failed
                }
            ]
        ));
    }
}
