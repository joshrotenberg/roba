//! Process-local lifecycle for one bounded Roba run.

use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex, Weak};

use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex as AsyncMutex, watch};

use crate::provider::{EventSink, Provider, ProviderContext, ProviderError, execute_turn};
use crate::run::{
    FailureKind, Prompt, ProviderId, RunEvent, RunFailure, RunId, RunOutcome, RunSpec, RunState,
    SessionSpec, WorkerPolicy, WorkerSpec,
};

/// Read-only view of a live or terminal run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunSnapshot {
    pub id: RunId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<RunId>,
    pub depth: u32,
    pub state: RunState,
    pub turns_completed: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_outcome: Option<RunOutcome>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<RunFailure>,
}

/// Read-only child-run view retained for the lifetime of the owning tree.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkerSnapshot {
    pub id: RunId,
    pub parent_id: RunId,
    pub depth: u32,
    pub provider: ProviderId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub run: RunSnapshot,
}

/// One sequenced event retained by the process-local run tree.
///
/// The root run observes events from every descendant. A child handle observes
/// only itself and its descendants.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunEventRecord {
    pub sequence: u64,
    pub run_id: RunId,
    pub event: RunEvent,
}

/// Bounded event page returned for one run-tree cursor.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunEventPage {
    pub events: Vec<RunEventRecord>,
    /// Highest tree-wide sequence inspected by this page. Supply this as the
    /// next `after` cursor, even when no scoped event was returned.
    pub next_sequence: u64,
    /// Oldest sequence still retained by the bounded journal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oldest_sequence: Option<u64>,
    /// True when the requested cursor predates retained history.
    pub truncated: bool,
    /// True when this run and all of its descendants can emit no more events.
    pub terminal: bool,
}

/// Replayable subscription over one run and its descendants.
pub struct RunEventSubscription {
    handle: RunHandle,
    cursor: u64,
}

/// Maximum records retained for one process-local run tree and returned in one
/// event page.
pub const RUN_EVENT_CAPACITY: usize = 256;

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
        let mut providers = BTreeMap::new();
        providers.insert(provider_id, provider);
        Self::with_providers(spec, providers)
    }

    pub(crate) fn with_providers(
        spec: RunSpec,
        providers: BTreeMap<ProviderId, Arc<dyn Provider>>,
    ) -> Result<Self, RunControlError> {
        spec.execution
            .workers
            .validate()
            .map_err(|_| RunControlError::InvalidWorkerPolicy)?;
        let provider = providers
            .get(&spec.agent.provider)
            .cloned()
            .ok_or_else(|| RunControlError::ProviderUnavailable(spec.agent.provider.clone()))?;
        let tree = Arc::new(RunTree::new(spec.execution.workers, providers));
        let inner = new_inner(spec, provider, tree.clone(), RunId::ROOT, None, 0);
        tree.register(&inner);
        Ok(Self {
            handle: RunHandle { inner },
        })
    }

    /// Identity of the root run within its process-local tree.
    pub fn id(&self) -> RunId {
        self.handle.inner.id
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

/// Least-authority child-run capability minted for one executing provider.
/// It cannot steer, cancel, or replace the parent's execution policy.
#[derive(Clone)]
pub struct WorkerControl {
    inner: Weak<Inner>,
}

impl fmt::Debug for WorkerControl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WorkerControl")
            .field("available", &self.inner.strong_count().gt(&0))
            .finish()
    }
}

impl WorkerControl {
    fn handle(&self) -> Result<RunHandle, RunControlError> {
        self.inner
            .upgrade()
            .map(|inner| RunHandle { inner })
            .ok_or(RunControlError::Terminal)
    }

    /// Spawn a child with the same agent, context, and inherited authority as
    /// the provider's exact parent run.
    pub async fn spawn(&self, prompt: Prompt) -> Result<RunHandle, RunControlError> {
        self.handle()?.spawn_inherited(prompt).await
    }

    /// Observe descendants of the provider's exact parent run.
    pub fn workers(&self) -> Result<Vec<WorkerSnapshot>, RunControlError> {
        Ok(self.handle()?.workers())
    }
}

impl RunHandle {
    /// Inspect the immutable resolved specification for this root or worker.
    pub fn spec(&self) -> &RunSpec {
        &self.inner.spec
    }

    /// Identity within this process-local run tree.
    pub fn id(&self) -> RunId {
        self.inner.id
    }

    /// Parent identity, absent only for the root.
    pub fn parent_id(&self) -> Option<RunId> {
        self.inner.parent_id
    }

    /// Distance from the root. The root has depth zero.
    pub fn depth(&self) -> u32 {
        self.inner.depth
    }

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
        emit(&self.inner, RunEvent::SteeringQueued);
        Ok(())
    }

    /// Return the latest in-memory snapshot.
    pub async fn status(&self) -> RunSnapshot {
        self.inner.control.lock().await.snapshot.clone()
    }

    /// Spawn a child with an explicitly selected agent while inheriting the
    /// parent's execution authority and a fresh provider session.
    pub async fn spawn_worker(&self, worker: WorkerSpec) -> Result<RunHandle, RunControlError> {
        let parent = self.inner.control.lock().await;
        if !matches!(parent.snapshot.state, RunState::Running | RunState::Waiting) {
            return Err(RunControlError::NotRunning);
        }

        let depth = self.inner.depth.saturating_add(1);
        let provider = self.inner.tree.provider(&worker.agent.provider)?;

        let mut execution = self.inner.spec.execution.clone();
        execution.session = SessionSpec::Fresh;
        let spec = RunSpec {
            agent: worker.agent,
            context: worker.context,
            execution,
            initial_prompt: Some(worker.prompt),
        };
        let request = spec
            .clone()
            .into_turn()
            .map_err(|_| RunControlError::WorkerPromptMissing)?;
        if let Err(error) = provider.validate(&request) {
            return Err(RunControlError::WorkerPreflight(error));
        }
        let id = self.inner.tree.reserve(depth)?;

        let inner = new_inner(
            spec,
            provider,
            self.inner.tree.clone(),
            id,
            Some(self.inner.id),
            depth,
        );
        self.inner.tree.register(&inner);
        let handle = RunHandle { inner };
        emit(
            &self.inner,
            RunEvent::WorkerSpawned {
                id,
                parent_id: self.inner.id,
                depth,
                provider: handle.inner.spec.agent.provider.clone(),
            },
        );
        drop(parent);
        handle.begin().await?;
        Ok(handle)
    }

    /// Spawn a child using the same agent and context as the parent.
    pub async fn spawn_inherited(&self, prompt: Prompt) -> Result<RunHandle, RunControlError> {
        self.spawn_worker(WorkerSpec {
            agent: self.inner.spec.agent.clone(),
            context: self.inner.spec.context.clone(),
            prompt,
        })
        .await
    }

    /// All descendants owned by this run, ordered by creation id. Terminal
    /// snapshots remain observable until the root tree is dropped.
    pub fn workers(&self) -> Vec<WorkerSnapshot> {
        self.inner.tree.descendants(self.inner.id)
    }

    /// Subscribe to retained and future normalized events for this run and its
    /// descendants. The first call to [`RunEventSubscription::next`] replays
    /// the oldest retained record.
    pub fn subscribe(&self) -> RunEventSubscription {
        self.subscribe_after(0)
    }

    /// Subscribe after an event sequence previously returned by
    /// [`RunHandle::event_page`].
    pub fn subscribe_after(&self, sequence: u64) -> RunEventSubscription {
        RunEventSubscription {
            handle: self.clone(),
            cursor: sequence,
        }
    }

    /// Read retained events after `sequence` without waiting.
    pub async fn event_page(
        &self,
        sequence: u64,
        limit: usize,
    ) -> Result<RunEventPage, RunControlError> {
        validate_event_limit(limit)?;
        Ok(self.scoped_event_page(sequence, limit).await)
    }

    /// Wait until at least one scoped event is available or this run becomes
    /// terminal. Unrelated sibling events are skipped while advancing the
    /// tree-wide cursor.
    pub async fn wait_for_events(
        &self,
        sequence: u64,
        limit: usize,
    ) -> Result<RunEventPage, RunControlError> {
        validate_event_limit(limit)?;
        let mut changed = self.inner.tree.events.subscribe();
        let mut cursor = sequence;
        loop {
            let page = self.scoped_event_page(cursor, limit).await;
            if !page.events.is_empty() || page.terminal {
                return Ok(page);
            }
            cursor = page.next_sequence;
            if changed.changed().await.is_err() {
                return Ok(self.scoped_event_page(cursor, limit).await);
            }
        }
    }

    /// Cancel a suspended, ready, or running run. Running provider futures are
    /// dropped at the cancellation boundary.
    pub async fn cancel(&self) -> Result<(), RunControlError> {
        self.cancel_self().await?;
        cancel_children(&self.inner).await;
        Ok(())
    }

    async fn cancel_self(&self) -> Result<(), RunControlError> {
        let mut control = self.inner.control.lock().await;
        if control.snapshot.is_terminal() {
            return Err(RunControlError::Terminal);
        }
        if matches!(
            control.snapshot.state,
            RunState::Running | RunState::Waiting
        ) {
            self.inner.cancel_tx.send_replace(true);
            set_state(&self.inner, &mut control, RunState::Finishing);
        } else {
            set_state(&self.inner, &mut control, RunState::Cancelled);
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

    async fn scoped_event_page(&self, sequence: u64, limit: usize) -> RunEventPage {
        let retained = self.inner.tree.events.page(sequence);
        let journal_sequence = retained.next_sequence;
        let mut next_sequence = journal_sequence;
        let mut events = Vec::with_capacity(limit);
        for record in retained.events {
            if self.inner.tree.includes(self.inner.id, record.run_id) {
                next_sequence = record.sequence;
                events.push(record);
                if events.len() == limit {
                    break;
                }
            }
        }
        if events.len() < limit {
            next_sequence = journal_sequence;
        }
        RunEventPage {
            events,
            next_sequence,
            oldest_sequence: retained.oldest_sequence,
            truncated: retained.truncated,
            terminal: self.status().await.is_terminal(),
        }
    }
}

impl RunEventSubscription {
    /// Last tree-wide sequence consumed by this subscription.
    pub fn cursor(&self) -> u64 {
        self.cursor
    }

    /// Wait for and return the next retained or future scoped event. Returns
    /// `None` only after the run is terminal and no further event is pending.
    pub async fn next(&mut self) -> Option<RunEventRecord> {
        loop {
            let page = self
                .handle
                .wait_for_events(self.cursor, 1)
                .await
                .expect("subscription uses a valid event page size");
            self.cursor = page.next_sequence;
            if let Some(record) = page.events.into_iter().next() {
                return Some(record);
            }
            if page.terminal {
                return None;
            }
        }
    }
}

struct Inner {
    id: RunId,
    parent_id: Option<RunId>,
    depth: u32,
    spec: RunSpec,
    provider: Arc<dyn Provider>,
    tree: Arc<RunTree>,
    control: AsyncMutex<Control>,
    snapshot_tx: watch::Sender<RunSnapshot>,
    cancel_tx: watch::Sender<bool>,
}

struct RunTree {
    policy: WorkerPolicy,
    next_id: AtomicU64,
    providers: BTreeMap<ProviderId, Arc<dyn Provider>>,
    records: StdMutex<BTreeMap<RunId, WorkerRecord>>,
    events: EventJournal,
}

struct EventJournal {
    state: StdMutex<EventJournalState>,
    revision: watch::Sender<u64>,
}

struct EventJournalState {
    next_sequence: u64,
    records: VecDeque<RunEventRecord>,
}

struct RetainedEventPage {
    events: Vec<RunEventRecord>,
    next_sequence: u64,
    oldest_sequence: Option<u64>,
    truncated: bool,
}

struct WorkerRecord {
    parent_id: Option<RunId>,
    depth: u32,
    provider: ProviderId,
    model: Option<String>,
    snapshot: watch::Receiver<RunSnapshot>,
    handle: Weak<Inner>,
}

struct Control {
    snapshot: RunSnapshot,
    started: bool,
    steering: VecDeque<Prompt>,
}

impl RunTree {
    fn new(policy: WorkerPolicy, providers: BTreeMap<ProviderId, Arc<dyn Provider>>) -> Self {
        Self {
            policy,
            next_id: AtomicU64::new(RunId::ROOT.get() + 1),
            providers,
            records: StdMutex::new(BTreeMap::new()),
            events: EventJournal::new(),
        }
    }

    fn provider(&self, provider_id: &ProviderId) -> Result<Arc<dyn Provider>, RunControlError> {
        self.providers
            .get(provider_id)
            .cloned()
            .ok_or_else(|| RunControlError::ProviderUnavailable(provider_id.clone()))
    }

    fn reserve(&self, depth: u32) -> Result<RunId, RunControlError> {
        if !self.policy.enabled() {
            return Err(RunControlError::WorkersDisabled);
        }
        if depth > self.policy.max_depth {
            return Err(RunControlError::WorkerDepthExceeded {
                requested: depth,
                maximum: self.policy.max_depth,
            });
        }
        let value = self
            .next_id
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |next| {
                let already_reserved = next.saturating_sub(RunId::ROOT.get() + 1);
                (already_reserved < u64::from(self.policy.max_workers)).then_some(next + 1)
            })
            .map_err(|_| RunControlError::WorkerLimitReached {
                maximum: self.policy.max_workers,
            })?;
        Ok(RunId::from_counter(value))
    }

    fn register(&self, inner: &Arc<Inner>) {
        lock_records(&self.records).insert(
            inner.id,
            WorkerRecord {
                parent_id: inner.parent_id,
                depth: inner.depth,
                provider: inner.spec.agent.provider.clone(),
                model: inner.spec.agent.model.clone(),
                snapshot: inner.snapshot_tx.subscribe(),
                handle: Arc::downgrade(inner),
            },
        );
    }

    fn descendants(&self, ancestor: RunId) -> Vec<WorkerSnapshot> {
        let records = lock_records(&self.records);
        records
            .iter()
            .filter(|(id, _)| **id != ancestor && is_descendant(&records, **id, ancestor))
            .filter_map(|(id, record)| {
                record.parent_id.map(|parent_id| WorkerSnapshot {
                    id: *id,
                    parent_id,
                    depth: record.depth,
                    provider: record.provider.clone(),
                    model: record.model.clone(),
                    run: record.snapshot.borrow().clone(),
                })
            })
            .collect()
    }

    fn descendant_handles(&self, ancestor: RunId) -> Vec<RunHandle> {
        let records = lock_records(&self.records);
        let mut descendants = records
            .iter()
            .filter(|(id, _)| **id != ancestor && is_descendant(&records, **id, ancestor))
            .filter_map(|(_, record)| record.handle.upgrade().map(|inner| (record.depth, inner)))
            .collect::<Vec<_>>();
        descendants.sort_by_key(|(depth, _)| std::cmp::Reverse(*depth));
        descendants
            .into_iter()
            .map(|(_, inner)| RunHandle { inner })
            .collect()
    }

    fn includes(&self, ancestor: RunId, candidate: RunId) -> bool {
        candidate == ancestor || is_descendant(&lock_records(&self.records), candidate, ancestor)
    }
}

impl EventJournal {
    fn new() -> Self {
        let (revision, _) = watch::channel(0);
        Self {
            state: StdMutex::new(EventJournalState {
                next_sequence: 1,
                records: VecDeque::with_capacity(RUN_EVENT_CAPACITY),
            }),
            revision,
        }
    }

    fn emit(&self, run_id: RunId, event: RunEvent) {
        let sequence = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let sequence = state.next_sequence;
            state.next_sequence = state.next_sequence.saturating_add(1);
            if state.records.len() == RUN_EVENT_CAPACITY {
                state.records.pop_front();
            }
            state.records.push_back(RunEventRecord {
                sequence,
                run_id,
                event,
            });
            sequence
        };
        self.revision.send_replace(sequence);
    }

    fn subscribe(&self) -> watch::Receiver<u64> {
        self.revision.subscribe()
    }

    fn page(&self, after: u64) -> RetainedEventPage {
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let next_sequence = after.max(state.next_sequence.saturating_sub(1));
        let oldest_sequence = state.records.front().map(|record| record.sequence);
        let truncated = oldest_sequence.is_some_and(|oldest| after.saturating_add(1) < oldest);
        RetainedEventPage {
            events: state
                .records
                .iter()
                .filter(|record| record.sequence > after)
                .cloned()
                .collect(),
            next_sequence,
            oldest_sequence,
            truncated,
        }
    }
}

fn lock_records(
    records: &StdMutex<BTreeMap<RunId, WorkerRecord>>,
) -> std::sync::MutexGuard<'_, BTreeMap<RunId, WorkerRecord>> {
    records
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn is_descendant(
    records: &BTreeMap<RunId, WorkerRecord>,
    mut candidate: RunId,
    ancestor: RunId,
) -> bool {
    while let Some(parent) = records.get(&candidate).and_then(|record| record.parent_id) {
        if parent == ancestor {
            return true;
        }
        candidate = parent;
    }
    false
}

fn new_inner(
    spec: RunSpec,
    provider: Arc<dyn Provider>,
    tree: Arc<RunTree>,
    id: RunId,
    parent_id: Option<RunId>,
    depth: u32,
) -> Arc<Inner> {
    let snapshot = RunSnapshot {
        id,
        parent_id,
        depth,
        state: spec.initial_state(),
        turns_completed: 0,
        last_outcome: None,
        failure: None,
    };
    let (snapshot_tx, _) = watch::channel(snapshot.clone());
    let (cancel_tx, _) = watch::channel(false);
    Arc::new(Inner {
        id,
        parent_id,
        depth,
        spec,
        provider,
        tree,
        control: AsyncMutex::new(Control {
            snapshot,
            started: false,
            steering: VecDeque::new(),
        }),
        snapshot_tx,
        cancel_tx,
    })
}

struct JournalSink {
    run_id: RunId,
    events: Arc<RunTree>,
}

impl EventSink for JournalSink {
    fn emit(&self, event: RunEvent) {
        self.events.events.emit(self.run_id, event);
    }
}

fn spawn_driver(inner: Arc<Inner>, prompt: Prompt) {
    tokio::spawn(async move {
        drive(inner, prompt).await;
    });
}

async fn drive(inner: Arc<Inner>, mut prompt: Prompt) {
    let mut session = inner.spec.execution.session.clone();
    let sink = JournalSink {
        run_id: inner.id,
        events: inner.tree.clone(),
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
        let context = if inner.spec.execution.workers.enabled() {
            ProviderContext::for_worker(WorkerControl {
                inner: Arc::downgrade(&inner),
            })
        } else {
            ProviderContext::default()
        };
        let result = tokio::select! {
            result = execute_turn(inner.provider.as_ref(), request, context, &sink) => Some(result),
            () = wait_for_cancellation(&mut cancelled) => None,
        };

        let Some(result) = result else {
            let mut control = inner.control.lock().await;
            set_state(&inner, &mut control, RunState::Finishing);
            drop(control);
            cancel_children(&inner).await;
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
                        drop(control);
                        fail(&inner, failure).await;
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
                set_state(&inner, &mut control, RunState::Finishing);
                drop(control);
                cancel_children(&inner).await;
                let mut control = inner.control.lock().await;
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
    set_state(inner, &mut control, RunState::Finishing);
    drop(control);
    cancel_children(inner).await;
    let mut control = inner.control.lock().await;
    set_state(inner, &mut control, RunState::Failed);
    emit(inner, RunEvent::Failed { failure });
}

async fn cancel_children(inner: &Inner) {
    let children = inner.tree.descendant_handles(inner.id);
    for child in &children {
        let _ = child.cancel_self().await;
    }
    for child in children {
        let _ = child.wait().await;
    }
}

fn set_state(inner: &Inner, control: &mut Control, state: RunState) {
    control.snapshot.state = state;
    inner.snapshot_tx.send_replace(control.snapshot.clone());
    emit(inner, RunEvent::StateChanged { state });
}

fn emit(inner: &Inner, event: RunEvent) {
    inner.tree.events.emit(inner.id, event);
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
    InvalidWorkerPolicy,
    ProviderUnavailable(ProviderId),
    WorkersDisabled,
    WorkerDepthExceeded {
        requested: u32,
        maximum: u32,
    },
    WorkerLimitReached {
        maximum: u32,
    },
    WorkerPromptMissing,
    WorkerPreflight(ProviderError),
    InvalidEventLimit {
        maximum: usize,
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
            Self::NotRunning => f.write_str("run is not accepting steering"),
            Self::SteeringUnsupported => {
                f.write_str("provider cannot resume and does not support steering")
            }
            Self::Terminal => f.write_str("run is already terminal"),
            Self::InvalidWorkerPolicy => f.write_str(
                "max_workers and max_depth must either both be zero or both be greater than zero",
            ),
            Self::ProviderUnavailable(provider) => {
                write!(f, "provider {provider} is not registered for this run tree")
            }
            Self::WorkersDisabled => f.write_str("this run does not permit child workers"),
            Self::WorkerDepthExceeded { requested, maximum } => write!(
                f,
                "worker depth {requested} exceeds this run's maximum depth {maximum}"
            ),
            Self::WorkerLimitReached { maximum } => {
                write!(f, "this run has reached its maximum of {maximum} workers")
            }
            Self::WorkerPromptMissing => f.write_str("worker prompt is missing"),
            Self::WorkerPreflight(error) => write!(f, "worker preflight refused: {error}"),
            Self::InvalidEventLimit { maximum } => {
                write!(f, "event page limit must be between 1 and {maximum}")
            }
        }
    }
}

impl std::error::Error for RunControlError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::WorkerPreflight(error) => Some(error),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use tokio::sync::Notify;

    use super::*;
    use crate::provider::{ProviderCapabilities, ProviderFuture};
    use crate::run::{
        AgentSpec, ContextSpec, PermissionPolicy, RunOutcome, SessionHandle, TurnRequest,
        WorkerPolicy, WorkerSpec,
    };

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
            _context: ProviderContext,
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

    #[tokio::test]
    async fn late_subscribers_replay_retained_events_without_cursor_loss() {
        let provider = provider(false);
        let spec = RunSpec::suspended(AgentSpec::new(ProviderId::claude()))
            .with_prompt(Prompt::new("hello").unwrap());
        let run = Run::new(spec, provider).unwrap();
        run.begin().await.unwrap();
        assert_eq!(run.handle().wait().await.state, RunState::Completed);

        let handle = run.handle();
        let first = handle.event_page(0, 1).await.unwrap();
        assert_eq!(first.events.len(), 1);
        assert_eq!(first.events[0].sequence, first.next_sequence);
        assert!(!first.truncated);

        let rest = handle
            .event_page(first.next_sequence, RUN_EVENT_CAPACITY)
            .await
            .unwrap();
        assert!(!rest.events.is_empty());
        assert!(
            rest.events
                .iter()
                .all(|record| record.sequence > first.next_sequence)
        );
        assert_eq!(
            rest.events.last().unwrap().event,
            RunEvent::StateChanged {
                state: RunState::Completed
            }
        );
        assert!(rest.terminal);

        let mut subscription = handle.subscribe();
        let mut replayed = Vec::new();
        while let Some(record) = subscription.next().await {
            replayed.push(record);
        }
        assert_eq!(replayed.len(), first.events.len() + rest.events.len());
        assert_eq!(subscription.cursor(), rest.next_sequence);
    }

    #[tokio::test]
    async fn waiting_event_cursor_wakes_when_a_suspended_run_starts() {
        let provider = provider(true);
        let run = Run::new(
            RunSpec::suspended(AgentSpec::new(ProviderId::claude())),
            provider.clone(),
        )
        .unwrap();
        let handle = run.handle();
        let waiter = tokio::spawn({
            let handle = handle.clone();
            async move { handle.wait_for_events(0, 1).await.unwrap() }
        });

        handle.start(Prompt::new("hello").unwrap()).await.unwrap();
        let page = waiter.await.unwrap();
        assert_eq!(page.events.len(), 1);
        assert_eq!(
            page.events[0].event,
            RunEvent::StateChanged {
                state: RunState::Running
            }
        );
        provider.first_started.notified().await;
        handle.cancel().await.unwrap();
        assert_eq!(handle.wait().await.state, RunState::Cancelled);
    }

    #[test]
    fn bounded_event_journal_reports_truncated_history() {
        let journal = EventJournal::new();
        for index in 0..=RUN_EVENT_CAPACITY {
            journal.emit(
                RunId::ROOT,
                RunEvent::Warning {
                    message: index.to_string(),
                },
            );
        }

        let page = journal.page(0);
        assert_eq!(page.events.len(), RUN_EVENT_CAPACITY);
        assert_eq!(page.oldest_sequence, Some(2));
        assert_eq!(page.next_sequence, RUN_EVENT_CAPACITY as u64 + 1);
        assert!(page.truncated);

        let future = journal.page(999);
        assert!(future.events.is_empty());
        assert_eq!(future.next_sequence, 999);
        assert!(!future.truncated);
    }

    struct WorkerProvider {
        calls: AtomicUsize,
        self_spawned: AtomicUsize,
        root_started: Notify,
        worker_started: Notify,
        release_root: Notify,
    }

    impl Provider for WorkerProvider {
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
            context: ProviderContext,
            _events: &'a dyn EventSink,
        ) -> ProviderFuture<'a> {
            Box::pin(async move {
                self.calls.fetch_add(1, Ordering::SeqCst);
                match request.prompt.as_str() {
                    "root" => {
                        self.root_started.notify_one();
                        self.release_root.notified().await;
                    }
                    "block worker" => {
                        self.worker_started.notify_one();
                        std::future::pending::<()>().await;
                    }
                    "self spawn" => {
                        let worker = context
                            .worker_control()
                            .expect("enabled root has worker control")
                            .spawn(Prompt::new("worker").unwrap())
                            .await
                            .unwrap();
                        self.self_spawned
                            .store(worker.id().get() as usize, Ordering::SeqCst);
                        assert_eq!(worker.wait().await.state, RunState::Completed);
                    }
                    _ => {}
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

    fn worker_root(policy: WorkerPolicy) -> (Run, Arc<WorkerProvider>) {
        let provider = Arc::new(WorkerProvider {
            calls: AtomicUsize::new(0),
            self_spawned: AtomicUsize::new(0),
            root_started: Notify::new(),
            worker_started: Notify::new(),
            release_root: Notify::new(),
        });
        let mut spec = RunSpec::suspended(AgentSpec::new(ProviderId::claude()))
            .with_prompt(Prompt::new("root").unwrap());
        spec.execution.workers = policy;
        (Run::new(spec, provider.clone()).unwrap(), provider)
    }

    #[tokio::test]
    async fn child_runs_are_owned_observable_and_counted_for_the_tree_lifetime() {
        let (run, provider) = worker_root(WorkerPolicy {
            max_workers: 2,
            max_depth: 1,
        });
        run.begin().await.unwrap();
        provider.root_started.notified().await;

        let worker = run
            .handle()
            .spawn_inherited(Prompt::new("worker").unwrap())
            .await
            .unwrap();
        assert_eq!(worker.parent_id(), Some(run.id()));
        assert_eq!(worker.depth(), 1);
        assert_eq!(worker.wait().await.state, RunState::Completed);

        let workers = run.handle().workers();
        assert_eq!(workers.len(), 1);
        assert_eq!(workers[0].id, worker.id());
        assert_eq!(workers[0].parent_id, run.id());
        assert_eq!(workers[0].run.state, RunState::Completed);
        assert_eq!(
            workers[0].run.last_outcome.as_ref().unwrap().output,
            "worker"
        );

        let root_events = run
            .handle()
            .event_page(0, RUN_EVENT_CAPACITY)
            .await
            .unwrap();
        assert!(
            root_events
                .events
                .iter()
                .any(|record| record.run_id == worker.id())
        );
        let worker_events = worker.event_page(0, RUN_EVENT_CAPACITY).await.unwrap();
        assert!(!worker_events.events.is_empty());
        assert!(
            worker_events
                .events
                .iter()
                .all(|record| record.run_id == worker.id())
        );

        provider.release_root.notify_one();
        assert_eq!(run.handle().wait().await.state, RunState::Completed);
        assert_eq!(provider.calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn disabled_depth_and_total_limits_fail_closed() {
        let (disabled, provider) = worker_root(WorkerPolicy::default());
        disabled.begin().await.unwrap();
        provider.root_started.notified().await;
        assert_eq!(
            disabled
                .handle()
                .spawn_inherited(Prompt::new("worker").unwrap())
                .await
                .err()
                .unwrap(),
            RunControlError::WorkersDisabled
        );
        provider.release_root.notify_one();
        disabled.handle().wait().await;

        let (limited, provider) = worker_root(WorkerPolicy {
            max_workers: 1,
            max_depth: 1,
        });
        limited.begin().await.unwrap();
        provider.root_started.notified().await;
        let first = limited
            .handle()
            .spawn_inherited(Prompt::new("block worker").unwrap())
            .await
            .unwrap();
        provider.worker_started.notified().await;
        assert_eq!(
            first
                .spawn_inherited(Prompt::new("grandchild").unwrap())
                .await
                .err()
                .unwrap(),
            RunControlError::WorkerDepthExceeded {
                requested: 2,
                maximum: 1,
            }
        );
        assert_eq!(
            limited
                .handle()
                .spawn_inherited(Prompt::new("two").unwrap())
                .await
                .err()
                .unwrap(),
            RunControlError::WorkerLimitReached { maximum: 1 }
        );
        provider.release_root.notify_one();
        limited.handle().wait().await;
    }

    #[tokio::test]
    async fn parent_completion_cancels_live_descendants_before_it_finishes() {
        let (run, provider) = worker_root(WorkerPolicy {
            max_workers: 2,
            max_depth: 2,
        });
        run.begin().await.unwrap();
        provider.root_started.notified().await;
        let worker = run
            .handle()
            .spawn_inherited(Prompt::new("block worker").unwrap())
            .await
            .unwrap();
        provider.worker_started.notified().await;

        provider.release_root.notify_one();
        let root_terminal = run.handle().wait().await;
        assert_eq!(root_terminal.state, RunState::Completed);
        assert_eq!(worker.wait().await.state, RunState::Cancelled);
        assert_eq!(run.handle().workers()[0].run.state, RunState::Cancelled);
    }

    #[tokio::test]
    async fn cancellation_closes_the_spawn_boundary_immediately() {
        let (run, provider) = worker_root(WorkerPolicy {
            max_workers: 1,
            max_depth: 1,
        });
        run.begin().await.unwrap();
        provider.root_started.notified().await;
        run.handle().cancel().await.unwrap();
        assert_eq!(
            run.handle()
                .spawn_inherited(Prompt::new("too late").unwrap())
                .await
                .err()
                .unwrap(),
            RunControlError::NotRunning
        );
        assert_eq!(run.handle().wait().await.state, RunState::Cancelled);
    }

    #[tokio::test]
    async fn executing_provider_gets_only_its_exact_worker_capability() {
        let provider = Arc::new(WorkerProvider {
            calls: AtomicUsize::new(0),
            self_spawned: AtomicUsize::new(0),
            root_started: Notify::new(),
            worker_started: Notify::new(),
            release_root: Notify::new(),
        });
        let mut spec = RunSpec::suspended(AgentSpec::new(ProviderId::claude()))
            .with_prompt(Prompt::new("self spawn").unwrap());
        spec.execution.workers = WorkerPolicy {
            max_workers: 1,
            max_depth: 1,
        };
        let run = Run::new(spec, provider.clone()).unwrap();

        run.begin().await.unwrap();
        assert_eq!(run.handle().wait().await.state, RunState::Completed);
        assert_eq!(provider.self_spawned.load(Ordering::SeqCst), 2);
        let workers = run.handle().workers();
        assert_eq!(workers.len(), 1);
        assert_eq!(workers[0].parent_id, run.id());
        assert_eq!(workers[0].run.state, RunState::Completed);
    }

    struct ImmediateCodexProvider;

    impl Provider for ImmediateCodexProvider {
        fn id(&self) -> ProviderId {
            ProviderId::codex()
        }

        fn capabilities(&self) -> ProviderCapabilities {
            ProviderCapabilities::default()
        }

        fn validate(&self, request: &TurnRequest) -> Result<(), ProviderError> {
            if request.spec.agent.provider == ProviderId::codex() {
                Ok(())
            } else {
                Err(ProviderError::unsupported("wrong provider"))
            }
        }

        fn execute<'a>(
            &'a self,
            request: TurnRequest,
            _context: ProviderContext,
            _events: &'a dyn EventSink,
        ) -> ProviderFuture<'a> {
            Box::pin(async move {
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

    #[tokio::test]
    async fn trusted_worker_agent_selection_can_change_provider_but_not_execution_authority() {
        let root_provider = Arc::new(WorkerProvider {
            calls: AtomicUsize::new(0),
            self_spawned: AtomicUsize::new(0),
            root_started: Notify::new(),
            worker_started: Notify::new(),
            release_root: Notify::new(),
        });
        let mut spec = RunSpec::suspended(AgentSpec::new(ProviderId::claude()))
            .with_prompt(Prompt::new("root").unwrap());
        spec.execution.permissions = PermissionPolicy::WorkspaceWrite;
        spec.execution.workers = WorkerPolicy {
            max_workers: 1,
            max_depth: 1,
        };
        let mut providers: BTreeMap<ProviderId, Arc<dyn Provider>> = BTreeMap::new();
        providers.insert(ProviderId::claude(), root_provider.clone());
        providers.insert(ProviderId::codex(), Arc::new(ImmediateCodexProvider));
        let run = Run::with_providers(spec, providers).unwrap();
        run.begin().await.unwrap();
        root_provider.root_started.notified().await;

        let worker = run
            .handle()
            .spawn_worker(WorkerSpec {
                agent: AgentSpec::new(ProviderId::codex()),
                context: ContextSpec::default(),
                prompt: Prompt::new("worker").unwrap(),
            })
            .await
            .unwrap();
        assert_eq!(
            worker.spec().execution.permissions,
            PermissionPolicy::WorkspaceWrite
        );
        assert_eq!(worker.spec().execution.session, SessionSpec::Fresh);
        assert_eq!(
            worker.spec().execution.workers,
            run.spec().execution.workers
        );
        assert_eq!(worker.wait().await.state, RunState::Completed);
        assert_eq!(worker.status().await.last_outcome.unwrap().output, "worker");

        root_provider.release_root.notify_one();
        run.handle().wait().await;
    }
}
