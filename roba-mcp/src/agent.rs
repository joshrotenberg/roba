//! Hot single-agent application state above finite core runs.

use std::fmt;
use std::sync::{Arc, Weak};
use std::time::{SystemTime, UNIX_EPOCH};

use roba_core::{
    Prompt, ProviderId, Roba, RunControlError, RunEventSubscription, RunEventSubscriptionItem,
    RunHandle, RunSnapshot, RunSpec, SessionHandle as CoreSessionHandle, SessionSpec,
};
use tokio::sync::{Mutex, watch};

use crate::contract::{
    AgentConfiguration, AgentControlRefusalKind, AgentInterruptResult, AgentRefusalKind,
    AgentShutdownResult, AgentSnapshot, AgentState, AgentSteerResult, AgentTurnResult, OperationId,
    OperationSettlement, ProviderSelfSnapshot,
};
use crate::events::{AgentEventError, AgentEventJournal, AgentEventPage};
use crate::provider_endpoint::ProviderEndpoint;

/// One hot logical agent with at most one active finite core run.
#[derive(Clone)]
pub struct AgentInstance {
    inner: Arc<Inner>,
}

struct Inner {
    runtime: Roba,
    template: RunSpec,
    configuration: AgentConfiguration,
    created_at_unix_ms: Option<u64>,
    events: AgentEventJournal,
    control: Mutex<Control>,
    shutdown_tx: watch::Sender<Option<AgentShutdownResult>>,
}

struct Control {
    lifetime: AgentLifetime,
    next_operation_id: u64,
    session: Option<CoreSessionHandle>,
    active: Option<Arc<ActiveOperation>>,
    latest_turn: Option<AgentTurnResult>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AgentLifetime {
    Open,
    Stopping,
    Stopped,
}

struct ActiveOperation {
    id: OperationId,
    handle: RunHandle,
    settlement: watch::Receiver<Option<AgentTurnResult>>,
    provider_endpoint: ProviderEndpoint,
}

/// Weak reference used by provider callback routers to avoid retaining their
/// owning `AgentInstance` through the server task.
#[derive(Clone)]
pub(crate) struct WeakAgentInstance {
    inner: Weak<Inner>,
}

impl WeakAgentInstance {
    pub(crate) async fn provider_self(
        &self,
        operation_id: OperationId,
    ) -> Option<ProviderSelfSnapshot> {
        let agent = AgentInstance {
            inner: self.inner.upgrade()?,
        };
        agent.provider_self(operation_id).await
    }
}

/// Opaque capability for one exact admitted finite run.
#[derive(Clone)]
pub(crate) struct AdmittedTurn {
    active: Arc<ActiveOperation>,
}

impl AdmittedTurn {
    /// Identity of the exact finite run captured by this capability.
    pub(crate) fn operation_id(&self) -> OperationId {
        self.active.id
    }
}

/// Result of validating and attempting one turn admission.
#[derive(Clone)]
pub(crate) enum TurnAdmission {
    Admitted(AdmittedTurn),
    Refused(AgentTurnResult),
}

impl AgentInstance {
    pub(crate) fn downgrade(&self) -> WeakAgentInstance {
        WeakAgentInstance {
            inner: Arc::downgrade(&self.inner),
        }
    }

    /// Construct an idle host from a suspended run template.
    ///
    /// Construction never starts provider work. A seeded resume handle is
    /// extracted into agent state and applied to the first submitted turn.
    pub fn new(runtime: Roba, mut template: RunSpec) -> Result<Self, AgentBuildError> {
        if template.initial_prompt.is_some() {
            return Err(AgentBuildError::TemplateNotSuspended);
        }
        if !runtime.contains(&template.agent.provider) {
            return Err(AgentBuildError::ProviderUnavailable(
                template.agent.provider.clone(),
            ));
        }
        if template
            .execution
            .limits
            .max_cost_usd
            .is_some_and(|cost| !cost.is_finite() || cost < 0.0)
        {
            return Err(AgentBuildError::InvalidMaxCost);
        }

        let session = match std::mem::take(&mut template.execution.session) {
            SessionSpec::Fresh => None,
            SessionSpec::Resume { session } => {
                if session.provider != template.agent.provider {
                    return Err(AgentBuildError::SessionProviderMismatch {
                        selected: template.agent.provider.clone(),
                        session: session.provider,
                    });
                }
                if session.id.trim().is_empty() {
                    return Err(AgentBuildError::EmptySessionId);
                }
                Some(session)
            }
        };
        let configuration = AgentConfiguration {
            provider: template.agent.provider.to_string(),
            model: template.agent.model.clone(),
            effort: template.agent.effort.map(Into::into),
            permissions: template.execution.permissions.into(),
            tools: template.execution.tools.clone().into(),
            limits: template.execution.limits.clone().into(),
        };

        let (shutdown_tx, _) = watch::channel(None);
        Ok(Self {
            inner: Arc::new(Inner {
                runtime,
                template,
                configuration,
                created_at_unix_ms: unix_time_ms(),
                events: AgentEventJournal::new(),
                control: Mutex::new(Control {
                    lifetime: AgentLifetime::Open,
                    next_operation_id: 1,
                    session,
                    active: None,
                    latest_turn: None,
                }),
                shutdown_tx,
            }),
        })
    }

    /// Submit one prompt and await its terminal finite-run result.
    ///
    /// Settlement belongs to a detached coordinator rather than the calling
    /// waiter. Dropping a caller waiting on this method therefore cannot wedge
    /// the agent in `running` after its provider turn finishes.
    pub async fn turn(&self, text: String) -> AgentTurnResult {
        match self.admit_turn(text).await {
            TurnAdmission::Admitted(turn) => self.wait_admitted(&turn).await,
            TurnAdmission::Refused(result) => result,
        }
    }

    /// Admit one turn without tying its settlement to the calling future.
    ///
    /// The returned capability is generation-fenced to this exact finite run.
    /// It lets protocol adapters wait or cancel without resolving whichever
    /// operation happens to be current later.
    pub(crate) async fn admit_turn(&self, text: String) -> TurnAdmission {
        let mut control = self.inner.control.lock().await;
        match control.lifetime {
            AgentLifetime::Stopping | AgentLifetime::Stopped => {
                return TurnAdmission::Refused(AgentTurnResult::refused(
                    AgentRefusalKind::Stopped,
                    "agent is stopped",
                    None,
                ));
            }
            AgentLifetime::Open => {}
        }
        if let Some(active) = &control.active {
            return TurnAdmission::Refused(AgentTurnResult::refused(
                AgentRefusalKind::Busy,
                format!("agent is already running operation {}", active.id.get()),
                Some(active.id),
            ));
        }
        let prompt = match Prompt::new(text) {
            Ok(prompt) => prompt,
            Err(error) => {
                return TurnAdmission::Refused(AgentTurnResult::refused(
                    AgentRefusalKind::InvalidPrompt,
                    error.to_string(),
                    None,
                ));
            }
        };
        let Some(next_operation_id) = control.next_operation_id.checked_add(1) else {
            return TurnAdmission::Refused(AgentTurnResult::refused(
                AgentRefusalKind::Runtime,
                "agent operation identity exhausted",
                None,
            ));
        };
        let operation_id = OperationId::new(control.next_operation_id);

        let mut spec = self.inner.template.clone();
        spec.execution.session = match &control.session {
            Some(session) => SessionSpec::Resume {
                session: session.clone(),
            },
            None => SessionSpec::Fresh,
        };
        let (provider_endpoint, launch_context) =
            match ProviderEndpoint::start(self.clone(), operation_id).await {
                Ok(endpoint) => endpoint,
                Err(error) => {
                    return TurnAdmission::Refused(AgentTurnResult::refused(
                        AgentRefusalKind::Runtime,
                        error.to_string(),
                        None,
                    ));
                }
            };
        let run = match self
            .inner
            .runtime
            .create_run_with_launch_context(spec, launch_context)
        {
            Ok(run) => run,
            Err(error) => {
                return TurnAdmission::Refused(AgentTurnResult::refused(
                    AgentRefusalKind::Runtime,
                    error.to_string(),
                    None,
                ));
            }
        };
        let handle = run.handle();
        let subscription = handle.subscribe();
        let (settlement_tx, settlement) = watch::channel(None);
        let active = Arc::new(ActiveOperation {
            id: operation_id,
            handle,
            settlement,
            provider_endpoint,
        });
        if let Err(error) = active.handle.start(prompt).await {
            return TurnAdmission::Refused(AgentTurnResult::refused(
                AgentRefusalKind::Runtime,
                error.to_string(),
                None,
            ));
        }
        active
            .provider_endpoint
            .close_when_run_settles(active.handle.clone());
        control.next_operation_id = next_operation_id;
        control.active = Some(active.clone());
        drop(control);

        self.spawn_operation(active.clone(), subscription, settlement_tx);
        TurnAdmission::Admitted(AdmittedTurn { active })
    }

    fn spawn_operation(
        &self,
        active: Arc<ActiveOperation>,
        subscription: RunEventSubscription,
        settlement_tx: watch::Sender<Option<AgentTurnResult>>,
    ) {
        let coordinator = self.clone();
        tokio::spawn(async move {
            let event_journal = coordinator.inner.events.clone();
            let operation_id = active.id;
            let event_pump = tokio::spawn(pump_events(subscription, event_journal, operation_id));

            let handle = active.handle.clone();
            let worker_handle = handle.clone();
            let worker = tokio::spawn(async move { worker_handle.wait().await });
            let snapshot = match worker.await {
                Ok(snapshot) => snapshot,
                Err(_) => recover_run(&handle).await,
            };
            if event_pump.await.is_err() {
                let _ = coordinator
                    .inner
                    .events
                    .append_history_gap(operation_id, None);
            }
            active.provider_endpoint.shutdown().await;
            let result = coordinator.settle(operation_id, snapshot).await;
            settlement_tx.send_replace(Some(result));
        });
    }

    async fn wait_for_settlement(&self, active: Arc<ActiveOperation>) -> AgentTurnResult {
        let mut settlement = active.settlement.clone();
        loop {
            if let Some(result) = settlement.borrow().clone() {
                return result;
            }
            if settlement.changed().await.is_err() {
                let snapshot = recover_run(&active.handle).await;
                active.provider_endpoint.shutdown().await;
                return self.settle(active.id, snapshot).await;
            }
        }
    }

    /// Wait for one exact admitted turn to settle.
    pub(crate) async fn wait_admitted(&self, turn: &AdmittedTurn) -> AgentTurnResult {
        self.wait_for_settlement(turn.active.clone()).await
    }

    /// Cancel one exact admitted turn and wait until agent settlement.
    ///
    /// Cancellation is applied to the captured core handle, never to a
    /// newly-current operation. A completion race is therefore harmless: the
    /// already-terminal result wins and is returned unchanged.
    pub(crate) async fn cancel_admitted_and_wait(&self, turn: &AdmittedTurn) -> AgentTurnResult {
        let _ = turn.active.handle.cancel().await;
        self.wait_for_settlement(turn.active.clone()).await
    }

    /// Return the latest inspectable agent state without starting work.
    pub async fn snapshot(&self) -> AgentSnapshot {
        let control = self.inner.control.lock().await;
        AgentSnapshot {
            configuration: self.inner.configuration.clone(),
            state: match control.lifetime {
                AgentLifetime::Stopping => AgentState::Stopping,
                AgentLifetime::Stopped => AgentState::Stopped,
                AgentLifetime::Open if control.active.is_some() => AgentState::Running,
                AgentLifetime::Open => AgentState::Idle,
            },
            session_available: control.session.is_some(),
            current_operation_id: control.active.as_ref().map(|active| active.id),
            latest_turn: control
                .latest_turn
                .as_ref()
                .map(AgentTurnResult::without_session_evidence),
            created_at_unix_ms: self.inner.created_at_unix_ms,
        }
    }

    async fn provider_self(&self, operation_id: OperationId) -> Option<ProviderSelfSnapshot> {
        let control = self.inner.control.lock().await;
        (control.lifetime == AgentLifetime::Open
            && control
                .active
                .as_ref()
                .is_some_and(|active| active.id == operation_id))
        .then_some(ProviderSelfSnapshot {
            operation_id,
            state: AgentState::Running,
        })
    }

    /// Permanently stop an idle agent.
    ///
    /// This compatibility helper refuses active work. Use [`Self::shutdown`]
    /// when the caller must cancel and drain an active operation.
    pub async fn stop(&self) -> Result<(), AgentStopError> {
        let mut control = self.inner.control.lock().await;
        match control.lifetime {
            AgentLifetime::Stopping => {
                drop(control);
                let _ = self.shutdown().await;
                return Ok(());
            }
            AgentLifetime::Stopped => return Ok(()),
            AgentLifetime::Open => {}
        }
        if let Some(active) = control.active.as_ref() {
            return Err(AgentStopError::Busy(active.id));
        }
        control.lifetime = AgentLifetime::Stopped;
        let result = AgentShutdownResult::Stopped { drained: None };
        self.inner.shutdown_tx.send_replace(Some(result));
        Ok(())
    }

    /// Queue guidance for one exact active operation.
    pub async fn steer(&self, operation_id: OperationId, text: String) -> AgentSteerResult {
        let prompt = match Prompt::new(text) {
            Ok(prompt) => prompt,
            Err(error) => {
                return AgentSteerResult::refused(
                    AgentControlRefusalKind::InvalidPrompt,
                    error.to_string(),
                    self.current_operation_id().await,
                );
            }
        };
        let handle = {
            let control = self.inner.control.lock().await;
            match control.lifetime {
                AgentLifetime::Stopping => {
                    return AgentSteerResult::refused(
                        AgentControlRefusalKind::Stopping,
                        "agent is stopping",
                        control.active.as_ref().map(|active| active.id),
                    );
                }
                AgentLifetime::Stopped => {
                    return AgentSteerResult::refused(
                        AgentControlRefusalKind::Stopped,
                        "agent is stopped",
                        None,
                    );
                }
                AgentLifetime::Open => {}
            }
            match &control.active {
                Some(active) if active.id == operation_id => active.handle.clone(),
                Some(active) => {
                    return AgentSteerResult::refused(
                        AgentControlRefusalKind::OperationMismatch,
                        format!(
                            "operation {} is active, not {}",
                            active.id.get(),
                            operation_id.get()
                        ),
                        Some(active.id),
                    );
                }
                None if control
                    .latest_turn
                    .as_ref()
                    .and_then(AgentTurnResult::operation_id)
                    == Some(operation_id) =>
                {
                    return AgentSteerResult::refused(
                        AgentControlRefusalKind::OperationSettled,
                        format!("operation {} has already settled", operation_id.get()),
                        None,
                    );
                }
                None => {
                    return AgentSteerResult::refused(
                        AgentControlRefusalKind::Idle,
                        "agent has no active operation",
                        None,
                    );
                }
            }
        };
        match handle.steer(prompt).await {
            Ok(()) => AgentSteerResult::Queued { operation_id },
            Err(RunControlError::SteeringUnsupported) => AgentSteerResult::refused(
                AgentControlRefusalKind::Unsupported,
                "provider cannot resume and does not support steering",
                Some(operation_id),
            ),
            Err(RunControlError::NotRunning | RunControlError::Terminal) => {
                let current = self.current_operation_id().await;
                if current == Some(operation_id) {
                    AgentSteerResult::refused(
                        AgentControlRefusalKind::OperationFinishing,
                        format!("operation {} is finishing", operation_id.get()),
                        Some(operation_id),
                    )
                } else {
                    AgentSteerResult::refused(
                        AgentControlRefusalKind::OperationSettled,
                        format!("operation {} has settled", operation_id.get()),
                        current,
                    )
                }
            }
            Err(error) => AgentSteerResult::refused(
                AgentControlRefusalKind::Runtime,
                error.to_string(),
                self.current_operation_id().await,
            ),
        }
    }

    /// Cancel one exact active operation and wait for agent-level settlement.
    pub async fn interrupt(&self, operation_id: OperationId) -> AgentInterruptResult {
        let active = {
            let control = self.inner.control.lock().await;
            match control.lifetime {
                AgentLifetime::Stopping => {
                    return AgentInterruptResult::refused(
                        AgentControlRefusalKind::Stopping,
                        "agent is stopping",
                        control.active.as_ref().map(|active| active.id),
                    );
                }
                AgentLifetime::Stopped => {
                    return AgentInterruptResult::refused(
                        AgentControlRefusalKind::Stopped,
                        "agent is stopped",
                        None,
                    );
                }
                AgentLifetime::Open => {}
            }
            match &control.active {
                Some(active) if active.id == operation_id => active.clone(),
                Some(active) => {
                    return AgentInterruptResult::refused(
                        AgentControlRefusalKind::OperationMismatch,
                        format!(
                            "operation {} is active, not {}",
                            active.id.get(),
                            operation_id.get()
                        ),
                        Some(active.id),
                    );
                }
                None => {
                    if let Some(settlement) = control
                        .latest_turn
                        .as_ref()
                        .and_then(OperationSettlement::from_turn)
                        .filter(|settlement| settlement.operation_id == operation_id)
                    {
                        return AgentInterruptResult::Settled {
                            settlement,
                            cancellation_requested: false,
                        };
                    }
                    return AgentInterruptResult::refused(
                        AgentControlRefusalKind::Idle,
                        "agent has no matching active operation",
                        None,
                    );
                }
            }
        };

        let cancellation_requested = match active.handle.cancel().await {
            Ok(()) => true,
            Err(RunControlError::Terminal) => false,
            Err(error) => {
                return AgentInterruptResult::refused(
                    AgentControlRefusalKind::Runtime,
                    format!(
                        "failed to interrupt operation {}: {error}",
                        operation_id.get()
                    ),
                    self.current_operation_id().await,
                );
            }
        };
        let result = self.wait_for_settlement(active).await;
        match OperationSettlement::from_turn(&result) {
            Some(settlement) => AgentInterruptResult::Settled {
                settlement,
                cancellation_requested,
            },
            None => AgentInterruptResult::refused(
                AgentControlRefusalKind::Runtime,
                "operation settlement was not terminal",
                self.current_operation_id().await,
            ),
        }
    }

    /// Permanently refuse new work, cancelling and draining any active run.
    pub async fn shutdown(&self) -> AgentShutdownResult {
        let mut receiver = self.inner.shutdown_tx.subscribe();
        let active = {
            let mut control = self.inner.control.lock().await;
            match control.lifetime {
                AgentLifetime::Open => {
                    control.lifetime = AgentLifetime::Stopping;
                    Some(control.active.clone())
                }
                AgentLifetime::Stopping | AgentLifetime::Stopped => None,
            }
        };
        if let Some(active) = active {
            let coordinator = self.clone();
            tokio::spawn(async move {
                let drained = match active {
                    Some(active) => {
                        let _ = active.handle.cancel().await;
                        let result = coordinator.wait_for_settlement(active).await;
                        OperationSettlement::from_turn(&result)
                    }
                    None => None,
                };
                let result = AgentShutdownResult::Stopped { drained };
                let mut control = coordinator.inner.control.lock().await;
                control.lifetime = AgentLifetime::Stopped;
                drop(control);
                coordinator.inner.shutdown_tx.send_replace(Some(result));
            });
        }
        loop {
            if let Some(result) = receiver.borrow().clone() {
                return result;
            }
            if receiver.changed().await.is_err() {
                return AgentShutdownResult::Stopped { drained: None };
            }
        }
    }

    /// Read a bounded page from the agent-wide event journal.
    pub async fn event_page(
        &self,
        after: u64,
        limit: usize,
    ) -> Result<AgentEventPage, AgentEventError> {
        let control = self.inner.control.lock().await;
        let mut page = self.inner.events.page(after, limit)?;
        page.closed = control.lifetime == AgentLifetime::Stopped;
        Ok(page)
    }

    async fn current_operation_id(&self) -> Option<OperationId> {
        self.inner
            .control
            .lock()
            .await
            .active
            .as_ref()
            .map(|active| active.id)
    }

    async fn settle(&self, operation_id: OperationId, snapshot: RunSnapshot) -> AgentTurnResult {
        let reported_session = reported_session(&snapshot);
        let invalid_evidence_message =
            invalid_terminal_evidence(&snapshot, &self.inner.template.agent.provider);
        let invalid_evidence = invalid_evidence_message.is_some();
        let result = match invalid_evidence_message {
            Some(message) => {
                AgentTurnResult::invalid_provider_result(operation_id, snapshot, message)
            }
            None => AgentTurnResult::terminal(operation_id, snapshot),
        };
        let mut control = self.inner.control.lock().await;
        if !control
            .active
            .as_ref()
            .is_some_and(|active| active.id == operation_id)
        {
            return control
                .latest_turn
                .as_ref()
                .filter(|latest| latest.operation_id() == Some(operation_id))
                .cloned()
                .unwrap_or(result);
        }
        if !invalid_evidence && let Some(session) = reported_session {
            control.session = Some(session);
        }
        control.latest_turn = Some(result.clone());
        control.active = None;
        if let Some(settlement) = OperationSettlement::from_turn(&result) {
            let _ = self.inner.events.append_settled(settlement);
        }
        result
    }
}

async fn recover_run(handle: &RunHandle) -> RunSnapshot {
    let snapshot = handle.status().await;
    if snapshot.is_terminal() {
        return snapshot;
    }
    let _ = handle.cancel().await;
    handle.wait().await
}

async fn pump_events(
    mut subscription: RunEventSubscription,
    events: AgentEventJournal,
    operation_id: OperationId,
) {
    loop {
        match subscription.next().await {
            Ok(Some(RunEventSubscriptionItem::Event(record))) => {
                let _ = events.append_core(operation_id, *record);
            }
            Ok(Some(RunEventSubscriptionItem::HistoryTruncated { oldest_sequence })) => {
                let _ = events.append_history_gap(operation_id, oldest_sequence);
            }
            Ok(None) => return,
            Err(_) => {
                let _ = events.append_history_gap(operation_id, None);
                return;
            }
        }
    }
}

fn unix_time_ms() -> Option<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
}

fn reported_session(snapshot: &RunSnapshot) -> Option<CoreSessionHandle> {
    snapshot
        .failure
        .as_ref()
        .and_then(|failure| failure.details.session.clone())
        .or_else(|| {
            snapshot
                .last_outcome
                .as_ref()
                .and_then(|outcome| outcome.session.clone())
        })
}

fn invalid_terminal_evidence(snapshot: &RunSnapshot, expected: &ProviderId) -> Option<String> {
    let sessions = snapshot
        .failure
        .as_ref()
        .and_then(|failure| failure.details.session.as_ref())
        .into_iter()
        .chain(
            snapshot
                .last_outcome
                .as_ref()
                .and_then(|outcome| outcome.session.as_ref()),
        );
    for session in sessions {
        if &session.provider != expected {
            return Some(format!(
                "provider returned session evidence for {}, expected {expected}",
                session.provider
            ));
        }
        if session.id.trim().is_empty() {
            return Some("provider returned an empty session id".to_string());
        }
    }
    let costs = snapshot
        .failure
        .as_ref()
        .and_then(|failure| failure.details.cost.as_ref())
        .into_iter()
        .chain(
            snapshot
                .last_outcome
                .as_ref()
                .and_then(|outcome| outcome.cost.as_ref()),
        );
    for cost in costs {
        if !cost.amount.is_finite() {
            return Some("provider returned a non-finite monetary cost".to_string());
        }
    }
    None
}

/// Invalid construction of a logical agent host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentBuildError {
    TemplateNotSuspended,
    ProviderUnavailable(ProviderId),
    SessionProviderMismatch {
        selected: ProviderId,
        session: ProviderId,
    },
    EmptySessionId,
    InvalidMaxCost,
}

impl fmt::Display for AgentBuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TemplateNotSuspended => {
                f.write_str("agent template must be suspended and contain no prompt")
            }
            Self::ProviderUnavailable(provider) => {
                write!(f, "provider {provider} is not registered")
            }
            Self::SessionProviderMismatch { selected, session } => write!(
                f,
                "agent selects provider {selected}, but seeded session belongs to {session}"
            ),
            Self::EmptySessionId => f.write_str("seeded session id must not be empty"),
            Self::InvalidMaxCost => {
                f.write_str("maximum cost must be a finite non-negative number")
            }
        }
    }
}

impl std::error::Error for AgentBuildError {}

/// An idle-only stop was attempted during active work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentStopError {
    Busy(OperationId),
}

impl fmt::Display for AgentStopError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Busy(operation_id) => write!(
                f,
                "cannot stop agent while operation {} is running",
                operation_id.get()
            ),
        }
    }
}

impl std::error::Error for AgentStopError {}

#[cfg(test)]
mod tests {
    use roba_core::{
        FailureKind, RunFailure, RunFailureDetails, RunOutcome, RunState, SessionHandle,
    };

    use super::*;

    #[test]
    fn terminal_failure_session_outranks_an_earlier_turn_outcome() {
        let provider = ProviderId::new("fake").unwrap();
        let session = |id: &str| SessionHandle {
            provider: provider.clone(),
            id: id.to_string(),
        };
        let snapshot = RunSnapshot {
            state: RunState::Failed,
            created_at_unix_ms: None,
            started_at_unix_ms: None,
            finished_at_unix_ms: None,
            elapsed_ms: None,
            turns_completed: 1,
            last_outcome: Some(RunOutcome {
                output: "first turn".to_string(),
                session: Some(session("older-success")),
                usage: None,
                cost: None,
                duration_ms: None,
                provider_turns: None,
                structured_output: None,
            }),
            failure: Some(RunFailure {
                kind: FailureKind::MaxTurns,
                message: "later turn failed".to_string(),
                details: RunFailureDetails {
                    session: Some(session("newer-failure")),
                    ..Default::default()
                },
            }),
        };

        assert_eq!(reported_session(&snapshot).unwrap().id, "newer-failure");
    }
}
