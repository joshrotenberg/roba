//! Hot single-agent application state above finite core runs.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Arc, Mutex as StdMutex, Weak};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use roba_core::{
    Prompt, ProviderId, Roba, RunControlError, RunEventSubscription, RunEventSubscriptionItem,
    RunHandle, RunSnapshot, RunSpec, SessionHandle as CoreSessionHandle, SessionSpec,
};
use tokio::sync::{Mutex, broadcast, watch};

use crate::context::{
    ContextContent, ContextPlan, ContextPlanError, ContextReadError, ContextReadEvidence,
    ContextSnapshot, OperationContext,
};
use crate::contract::{
    ActiveActivity, AgentConfiguration, AgentControlRefusalKind, AgentFollowUpResult,
    AgentInterruptResult, AgentObservation, AgentRefusalKind, AgentShutdownResult, AgentSnapshot,
    AgentState, AgentTerminalState, AgentTurnResult, ObservationHealth, ObservationState,
    OperationId, OperationSettlement, ProviderSelfSnapshot, TurnOverrides,
};
use crate::events::{
    AGENT_EVENT_CAPACITY, AgentEvent, AgentEventError, AgentEventJournal, AgentEventPage,
    AgentEventRecord,
};
use crate::extension_lifecycle::{ExtensionOperationSupervisor, shutdown_extensions};
use crate::extensions::{AgentExtensionError, AgentExtensions};
use crate::provider_endpoint::ProviderEndpoint;

/// One hot logical agent with at most one active finite core run.
#[derive(Clone)]
pub struct AgentInstance {
    inner: Arc<Inner>,
}

struct Inner {
    runtime: Roba,
    template: RunSpec,
    context_plan: ContextPlan,
    configuration: AgentConfiguration,
    extensions: AgentExtensions,
    created_at_unix_ms: Option<u64>,
    events: AgentEventJournal,
    event_tx: broadcast::Sender<AgentEventRecord>,
    control: Mutex<Control>,
    shutdown_tx: watch::Sender<Option<AgentShutdownResult>>,
}

struct Control {
    lifetime: AgentLifetime,
    next_operation_id: u64,
    session: Option<CoreSessionHandle>,
    active: Option<Arc<ActiveOperation>>,
    latest_turn: Option<AgentTurnResult>,
    latest_context_evidence: Option<ContextReadEvidence>,
    latest_observation: Option<AgentObservation>,
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
    context: OperationContext,
    configuration: AgentConfiguration,
    observation: StdMutex<OperationObservation>,
    started_at: Instant,
    timeout: Option<Duration>,
    admitted_at_unix_ms: Option<u64>,
}

#[derive(Default)]
struct OperationObservation {
    active: BTreeMap<String, ActiveActivity>,
    last_provider_event_kind: Option<String>,
    last_provider_event_at_unix_ms: Option<u64>,
    last_activity_at_unix_ms: Option<u64>,
    degraded: bool,
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

    pub(crate) async fn provider_context_manifest(
        &self,
        operation_id: OperationId,
    ) -> Option<ContextSnapshot> {
        let agent = AgentInstance {
            inner: self.inner.upgrade()?,
        };
        agent.provider_context_manifest(operation_id).await
    }

    pub(crate) async fn provider_context_content(
        &self,
        operation_id: OperationId,
        id: &str,
        generation: u64,
    ) -> Result<ContextContent, ContextReadError> {
        let agent = AgentInstance {
            inner: self
                .inner
                .upgrade()
                .ok_or(ContextReadError::OperationUnavailable(operation_id))?,
        };
        agent
            .provider_context_content(operation_id, id, generation)
            .await
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

    pub(crate) fn extensions(&self) -> &AgentExtensions {
        &self.inner.extensions
    }

    /// Content-free provenance and delivery inventory for the fixed template.
    ///
    /// This foundation describes context already compiled into [`RunSpec`].
    /// It does not claim that provider-native ambient sources are isolated or
    /// that the provider has read MCP-available context.
    pub fn context_plan(&self) -> &ContextPlan {
        &self.inner.context_plan
    }

    /// Construct an idle host from a suspended run template.
    ///
    /// Construction never starts provider work. A seeded resume handle is
    /// extracted into agent state and applied to the first submitted turn.
    pub fn new(runtime: Roba, template: RunSpec) -> Result<Self, AgentBuildError> {
        Self::new_with_extensions(runtime, template, AgentExtensions::default())
    }

    /// Construct an idle host with one immutable set of MCP extensions.
    ///
    /// Both role-specific projections are composed and checked against the
    /// built-in contract before construction succeeds. No provider work or
    /// private listener starts during this preflight.
    pub fn new_with_extensions(
        runtime: Roba,
        template: RunSpec,
        extensions: AgentExtensions,
    ) -> Result<Self, AgentBuildError> {
        let context_plan = ContextPlan::from_run_spec(&template);
        Self::new_with_context_plan(runtime, template, extensions, context_plan)
    }

    /// Construct an idle host with an explicit immutable context plan.
    ///
    /// The plan may add host-owned MCP-native entries, but it must retain the
    /// exact provider-adapter entries already present in the executable
    /// [`RunSpec`]. Both MCP projections and extension collisions are
    /// preflighted before provider work or private listener creation.
    pub fn new_with_context_plan(
        runtime: Roba,
        mut template: RunSpec,
        extensions: AgentExtensions,
        context_plan: ContextPlan,
    ) -> Result<Self, AgentBuildError> {
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

        context_plan
            .validate_run_spec(&template)
            .map_err(AgentBuildError::ContextPlan)?;

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
        let configuration = AgentConfiguration::from_run_spec(&template);

        let (shutdown_tx, _) = watch::channel(None);
        let (event_tx, _) = broadcast::channel(AGENT_EVENT_CAPACITY);
        let agent = Self {
            inner: Arc::new(Inner {
                runtime,
                template,
                context_plan,
                configuration,
                extensions,
                created_at_unix_ms: unix_time_ms(),
                events: AgentEventJournal::new(),
                event_tx,
                control: Mutex::new(Control {
                    lifetime: AgentLifetime::Open,
                    next_operation_id: 1,
                    session,
                    active: None,
                    latest_turn: None,
                    latest_context_evidence: None,
                    latest_observation: None,
                }),
                shutdown_tx,
            }),
        };
        agent
            .extensions()
            .preflight(
                crate::router::base_control_router(agent.clone()),
                crate::router::base_agent_router(agent.clone(), OperationId::new(1)),
            )
            .map_err(AgentBuildError::Extension)?;
        Ok(agent)
    }

    /// Submit one prompt and await its terminal finite-run result.
    ///
    /// Settlement belongs to a detached coordinator rather than the calling
    /// waiter. Dropping a caller waiting on this method therefore cannot wedge
    /// the agent in `running` after its provider turn finishes.
    pub async fn turn(&self, text: String) -> AgentTurnResult {
        self.turn_with_overrides(text, TurnOverrides::default())
            .await
    }

    /// Submit one prompt with operation-local provider settings and ceilings.
    pub async fn turn_with_overrides(
        &self,
        text: String,
        overrides: TurnOverrides,
    ) -> AgentTurnResult {
        match self.admit_turn_with_overrides(text, overrides).await {
            TurnAdmission::Admitted(turn) => self.wait_admitted(&turn).await,
            TurnAdmission::Refused(result) => result,
        }
    }

    /// Admit one turn without tying its settlement to the calling future.
    ///
    /// The returned capability is generation-fenced to this exact finite run.
    /// It lets protocol adapters wait or cancel without resolving whichever
    /// operation happens to be current later.
    pub(crate) async fn admit_turn_with_overrides(
        &self,
        text: String,
        overrides: TurnOverrides,
    ) -> TurnAdmission {
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
        if let Err(message) = apply_turn_overrides(&mut spec, overrides) {
            return TurnAdmission::Refused(AgentTurnResult::refused(
                AgentRefusalKind::InvalidConfiguration,
                message,
                None,
            ));
        }
        spec.execution.session = match &control.session {
            Some(session) => SessionSpec::Resume {
                session: session.clone(),
            },
            None => SessionSpec::Fresh,
        };
        let bootstrap = self.inner.context_plan.provider_bootstrap(
            operation_id,
            &spec.agent.provider,
            spec.execution.permissions,
        );
        let operation_context = OperationContext::new(self.inner.context_plan.clone(), bootstrap);
        let configuration = AgentConfiguration::from_run_spec(&spec);
        let (provider_endpoint, launch_context) = match ProviderEndpoint::start(
            self.clone(),
            operation_id,
            operation_context.bootstrap().render(),
        )
        .await
        {
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
        let timeout = configuration.limits.timeout_secs.map(Duration::from_secs);
        let active = Arc::new(ActiveOperation {
            id: operation_id,
            handle,
            settlement,
            provider_endpoint,
            context: operation_context,
            configuration,
            observation: StdMutex::new(OperationObservation::default()),
            started_at: Instant::now(),
            timeout,
            admitted_at_unix_ms: unix_time_ms(),
        });
        active
            .provider_endpoint
            .close_when_run_settles(active.handle.clone());
        control.next_operation_id = next_operation_id;
        control.active = Some(active.clone());
        drop(control);

        self.spawn_operation(active.clone(), prompt, subscription, settlement_tx);
        TurnAdmission::Admitted(AdmittedTurn { active })
    }

    fn spawn_operation(
        &self,
        active: Arc<ActiveOperation>,
        prompt: Prompt,
        subscription: RunEventSubscription,
        settlement_tx: watch::Sender<Option<AgentTurnResult>>,
    ) {
        let coordinator = self.clone();
        tokio::spawn(async move {
            let event_journal = coordinator.inner.events.clone();
            let event_tx = coordinator.inner.event_tx.clone();
            let operation_id = active.id;
            let observed = active.clone();
            let event_pump =
                tokio::spawn(pump_events(subscription, event_journal, event_tx, observed));

            let operation = crate::extensions::AgentExtensionOperation {
                operation_id,
                configuration: active.configuration.clone(),
                admitted_at_unix_ms: active.admitted_at_unix_ms,
            };
            let mut extensions = ExtensionOperationSupervisor::admitted(
                coordinator.inner.extensions.lifecycle_registrations(),
                operation,
                coordinator.inner.events.clone(),
                coordinator.inner.event_tx.clone(),
            )
            .await;

            let handle = active.handle.clone();
            if !handle.status().await.is_terminal() {
                match handle.start(prompt).await {
                    Ok(()) => extensions.started().await,
                    Err(_) => {
                        let _ = handle.cancel().await;
                    }
                }
            }
            let worker_handle = handle.clone();
            let worker = tokio::spawn(async move { worker_handle.wait().await });
            let snapshot = match worker.await {
                Ok(snapshot) => snapshot,
                Err(_) => recover_run(&handle).await,
            };
            if event_pump.await.is_err() {
                active.mark_degraded();
                if let Ok(record) = coordinator
                    .inner
                    .events
                    .append_history_gap(operation_id, None)
                {
                    let _ = coordinator.inner.event_tx.send(record);
                }
            }
            active.provider_endpoint.shutdown().await;
            extensions.settle(terminal_state(&snapshot)).await;
            let result = coordinator
                .settle(operation_id, snapshot, active.configuration.clone())
                .await;
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
                return self
                    .settle(active.id, snapshot, active.configuration.clone())
                    .await;
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
        let observation = control
            .active
            .as_ref()
            .map(|active| active.observation(false))
            .or_else(|| control.latest_observation.clone())
            .unwrap_or_else(AgentObservation::unknown);
        AgentSnapshot {
            configuration: self.inner.configuration.clone(),
            active_configuration: control
                .active
                .as_ref()
                .map(|active| active.configuration.clone()),
            observation,
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

    /// Content-free context and provider read evidence for the active or most
    /// recently settled operation. Reading this control view does not count as
    /// provider acquisition.
    pub async fn context_snapshot(&self) -> ContextSnapshot {
        let control = self.inner.control.lock().await;
        let (operation_id, bootstrap, evidence) = match &control.active {
            Some(active) => (
                Some(active.id),
                Some(active.context.bootstrap().clone()),
                Some(active.context.evidence()),
            ),
            None => {
                let operation_id = control
                    .latest_context_evidence
                    .as_ref()
                    .map(|evidence| evidence.operation_id);
                let bootstrap = operation_id.map(|operation_id| {
                    self.inner.context_plan.provider_bootstrap(
                        operation_id,
                        &self.inner.template.agent.provider,
                        self.inner.template.execution.permissions,
                    )
                });
                (
                    operation_id,
                    bootstrap,
                    control.latest_context_evidence.clone(),
                )
            }
        };
        ContextSnapshot {
            operation_id,
            manifest: self.inner.context_plan.manifest().clone(),
            bootstrap,
            read_evidence: evidence,
        }
    }

    pub(crate) async fn context_content(
        &self,
        id: &str,
        generation: u64,
    ) -> Result<ContextContent, ContextReadError> {
        let operation_id = {
            let control = self.inner.control.lock().await;
            control.active.as_ref().map(|active| active.id).or_else(|| {
                control
                    .latest_context_evidence
                    .as_ref()
                    .map(|evidence| evidence.operation_id)
            })
        };
        self.inner
            .context_plan
            .content(operation_id, id, generation)
    }

    async fn provider_context_manifest(
        &self,
        operation_id: OperationId,
    ) -> Option<ContextSnapshot> {
        let context = {
            let control = self.inner.control.lock().await;
            (control.lifetime == AgentLifetime::Open)
                .then(|| control.active.as_ref())
                .flatten()
                .filter(|active| active.id == operation_id)
                .cloned()
        }?;
        Some(context.context.manifest_read())
    }

    async fn provider_context_content(
        &self,
        operation_id: OperationId,
        id: &str,
        generation: u64,
    ) -> Result<ContextContent, ContextReadError> {
        let context = {
            let control = self.inner.control.lock().await;
            (control.lifetime == AgentLifetime::Open)
                .then(|| control.active.as_ref())
                .flatten()
                .filter(|active| active.id == operation_id)
                .cloned()
                .ok_or(ContextReadError::OperationUnavailable(operation_id))?
        };
        context.context.content_read(id, generation)
    }

    /// Permanently stop an idle agent.
    ///
    /// This compatibility helper refuses active work. Use [`Self::shutdown`]
    /// when the caller must cancel and drain an active operation.
    pub async fn stop(&self) -> Result<(), AgentStopError> {
        let mut control = self.inner.control.lock().await;
        match control.lifetime {
            AgentLifetime::Stopped => return Ok(()),
            AgentLifetime::Stopping => {
                drop(control);
                let _ = self.wait_stopped().await;
                return Ok(());
            }
            AgentLifetime::Open => {}
        }
        if let Some(active) = control.active.as_ref() {
            return Err(AgentStopError::Busy(active.id));
        }
        control.lifetime = AgentLifetime::Stopping;
        drop(control);

        shutdown_extensions(self.inner.extensions.lifecycle_registrations()).await;
        let result = AgentShutdownResult::Stopped { drained: None };
        let mut control = self.inner.control.lock().await;
        control.lifetime = AgentLifetime::Stopped;
        drop(control);
        self.inner.shutdown_tx.send_replace(Some(result));
        Ok(())
    }

    /// Queue a follow-up for one exact active operation.
    pub async fn follow_up(&self, operation_id: OperationId, text: String) -> AgentFollowUpResult {
        let prompt = match Prompt::new(text) {
            Ok(prompt) => prompt,
            Err(error) => {
                return AgentFollowUpResult::refused(
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
                    return AgentFollowUpResult::refused(
                        AgentControlRefusalKind::Stopping,
                        "agent is stopping",
                        control.active.as_ref().map(|active| active.id),
                    );
                }
                AgentLifetime::Stopped => {
                    return AgentFollowUpResult::refused(
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
                    return AgentFollowUpResult::refused(
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
                    return AgentFollowUpResult::refused(
                        AgentControlRefusalKind::OperationSettled,
                        format!("operation {} has already settled", operation_id.get()),
                        None,
                    );
                }
                None => {
                    return AgentFollowUpResult::refused(
                        AgentControlRefusalKind::Idle,
                        "agent has no active operation",
                        None,
                    );
                }
            }
        };
        match handle.follow_up(prompt).await {
            Ok(()) => AgentFollowUpResult::Queued { operation_id },
            Err(RunControlError::FollowUpUnsupported) => AgentFollowUpResult::refused(
                AgentControlRefusalKind::Unsupported,
                "provider cannot resume and does not support follow-ups",
                Some(operation_id),
            ),
            Err(RunControlError::FollowUpQueueFull { .. }) => AgentFollowUpResult::refused(
                AgentControlRefusalKind::QueueFull,
                "follow-up queue is full",
                Some(operation_id),
            ),
            Err(RunControlError::NotRunning | RunControlError::Terminal) => {
                let current = self.current_operation_id().await;
                if current == Some(operation_id) {
                    if handle.status().await.state == roba_core::RunState::Suspended {
                        AgentFollowUpResult::refused(
                            AgentControlRefusalKind::OperationStarting,
                            format!("operation {} is starting", operation_id.get()),
                            Some(operation_id),
                        )
                    } else {
                        AgentFollowUpResult::refused(
                            AgentControlRefusalKind::OperationFinishing,
                            format!("operation {} is finishing", operation_id.get()),
                            Some(operation_id),
                        )
                    }
                } else {
                    AgentFollowUpResult::refused(
                        AgentControlRefusalKind::OperationSettled,
                        format!("operation {} has settled", operation_id.get()),
                        current,
                    )
                }
            }
            Err(error) => AgentFollowUpResult::refused(
                AgentControlRefusalKind::Runtime,
                error.to_string(),
                self.current_operation_id().await,
            ),
        }
    }

    /// Source-compatible alias for [`AgentInstance::follow_up`].
    pub async fn steer(&self, operation_id: OperationId, text: String) -> AgentFollowUpResult {
        self.follow_up(operation_id, text).await
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
                shutdown_extensions(coordinator.inner.extensions.lifecycle_registrations()).await;
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

    /// Wait for another caller to finish permanently stopping this agent.
    ///
    /// This is a passive lifecycle observation used by bindings. It never
    /// initiates shutdown itself.
    pub(crate) async fn wait_stopped(&self) -> AgentShutdownResult {
        let mut receiver = self.inner.shutdown_tx.subscribe();
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

    async fn settle(
        &self,
        operation_id: OperationId,
        snapshot: RunSnapshot,
        configuration: AgentConfiguration,
    ) -> AgentTurnResult {
        let reported_session = reported_session(&snapshot);
        let invalid_evidence_message =
            invalid_terminal_evidence(&snapshot, &self.inner.template.agent.provider);
        let invalid_evidence = invalid_evidence_message.is_some();
        let result = match invalid_evidence_message {
            Some(message) => AgentTurnResult::invalid_provider_result(
                operation_id,
                snapshot,
                message,
                configuration,
            ),
            None => AgentTurnResult::terminal(operation_id, snapshot, configuration),
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
        let context_evidence = control
            .active
            .as_ref()
            .map(|active| active.context.evidence());
        control.latest_turn = Some(result.clone());
        control.latest_context_evidence = context_evidence;
        control.latest_observation = control
            .active
            .as_ref()
            .map(|active| active.observation(true));
        control.active = None;
        if let Some(settlement) = OperationSettlement::from_turn(&result)
            && let Ok(record) = self.inner.events.append_settled(settlement)
        {
            let _ = self.inner.event_tx.send(record);
        }
        result
    }

    pub(crate) fn subscribe_live_events(&self) -> broadcast::Receiver<AgentEventRecord> {
        self.inner.event_tx.subscribe()
    }
}

impl ActiveOperation {
    fn observe(&self, record: &AgentEventRecord) {
        let mut observation = self
            .observation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let occurred_at = record.occurred_at_unix_ms.or_else(unix_time_ms);
        let provider_kind = match &record.event {
            AgentEvent::ActivityStarted {
                id,
                activity,
                summary,
            } => {
                observation.active.insert(
                    id.clone(),
                    ActiveActivity {
                        id: id.clone(),
                        activity: *activity,
                        summary: summary.clone(),
                        started_at_unix_ms: occurred_at,
                    },
                );
                observation.last_activity_at_unix_ms = occurred_at;
                Some("activity_started")
            }
            AgentEvent::ActivityCompleted { id, .. } => {
                observation.active.remove(id);
                observation.last_activity_at_unix_ms = occurred_at;
                Some("activity_completed")
            }
            AgentEvent::OutputDelta { .. } => Some("output_delta"),
            AgentEvent::Usage { .. } => Some("usage"),
            AgentEvent::Warning { .. } => Some("warning"),
            AgentEvent::RunHistoryTruncated { .. } => {
                observation.degraded = true;
                None
            }
            AgentEvent::StateChanged { .. }
            | AgentEvent::TurnStarted { .. }
            | AgentEvent::FollowUpQueued
            | AgentEvent::FollowUpApplied
            | AgentEvent::TurnCompleted { .. }
            | AgentEvent::Failed { .. }
            | AgentEvent::ExtensionChanged { .. }
            | AgentEvent::ExtensionFailed { .. }
            | AgentEvent::OperationSettled { .. } => None,
        };
        if let Some(kind) = provider_kind {
            observation.last_provider_event_kind = Some(kind.to_owned());
            observation.last_provider_event_at_unix_ms = occurred_at;
        }
    }

    fn mark_degraded(&self) {
        self.observation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .degraded = true;
    }

    fn observation(&self, terminal: bool) -> AgentObservation {
        let observation = self
            .observation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let elapsed = self.started_at.elapsed();
        let elapsed_ms = u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX);
        let active_activities = if terminal {
            Vec::new()
        } else {
            observation.active.values().cloned().collect()
        };
        let state = if terminal {
            ObservationState::Terminal
        } else if !active_activities.is_empty() {
            ObservationState::Active
        } else if observation.last_activity_at_unix_ms.is_some() {
            ObservationState::RecentlyActive
        } else {
            ObservationState::Unknown
        };
        let health = if terminal {
            ObservationHealth::Terminal
        } else if observation.degraded {
            ObservationHealth::Degraded
        } else if observation.last_provider_event_kind.is_some() {
            ObservationHealth::Healthy
        } else {
            ObservationHealth::Unknown
        };
        AgentObservation {
            state,
            health,
            active_activities,
            last_provider_event_kind: observation.last_provider_event_kind.clone(),
            last_provider_event_at_unix_ms: observation.last_provider_event_at_unix_ms,
            last_activity_at_unix_ms: observation.last_activity_at_unix_ms,
            elapsed_ms: Some(elapsed_ms),
            timeout_remaining_ms: self.timeout.map(|timeout| {
                u64::try_from(timeout.saturating_sub(elapsed).as_millis()).unwrap_or(u64::MAX)
            }),
        }
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
    event_tx: broadcast::Sender<AgentEventRecord>,
    active: Arc<ActiveOperation>,
) {
    let operation_id = active.id;
    loop {
        match subscription.next().await {
            Ok(Some(RunEventSubscriptionItem::Event(record))) => {
                if let Ok(record) = events.append_core(operation_id, *record) {
                    active.observe(&record);
                    let _ = event_tx.send(record);
                }
            }
            Ok(Some(RunEventSubscriptionItem::HistoryTruncated { oldest_sequence })) => {
                active.mark_degraded();
                if let Ok(record) = events.append_history_gap(operation_id, oldest_sequence) {
                    active.observe(&record);
                    let _ = event_tx.send(record);
                }
            }
            Ok(None) => return,
            Err(_) => {
                active.mark_degraded();
                if let Ok(record) = events.append_history_gap(operation_id, None) {
                    active.observe(&record);
                    let _ = event_tx.send(record);
                }
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

fn apply_turn_overrides(spec: &mut RunSpec, overrides: TurnOverrides) -> Result<(), String> {
    if let Some(model) = overrides.model {
        if model.trim().is_empty() {
            return Err("turn override model must not be empty".to_string());
        }
        spec.agent.model = Some(model);
    }
    if let Some(effort) = overrides.effort {
        spec.agent.effort = Some(effort.into());
    }
    if let Some(max_turns) = overrides.limits.max_turns {
        if max_turns == 0 {
            return Err("turn override max_turns must be greater than zero".to_string());
        }
        spec.execution.limits.max_turns = Some(max_turns);
    }
    if let Some(max_cost_usd) = overrides.limits.max_cost_usd {
        if !max_cost_usd.is_finite() || max_cost_usd <= 0.0 {
            return Err(
                "turn override max_cost_usd must be finite and greater than zero".to_string(),
            );
        }
        spec.execution.limits.max_cost_usd = Some(max_cost_usd);
    }
    if let Some(timeout_secs) = overrides.limits.timeout_secs {
        if timeout_secs == 0 {
            return Err("turn override timeout_secs must be greater than zero".to_string());
        }
        spec.execution.limits.timeout_secs = Some(timeout_secs);
    }
    Ok(())
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

fn terminal_state(snapshot: &RunSnapshot) -> AgentTerminalState {
    match snapshot.state {
        roba_core::RunState::Completed => AgentTerminalState::Completed,
        roba_core::RunState::Cancelled => AgentTerminalState::Cancelled,
        roba_core::RunState::Failed
        | roba_core::RunState::Suspended
        | roba_core::RunState::Ready
        | roba_core::RunState::Running
        | roba_core::RunState::Finishing => AgentTerminalState::Failed,
    }
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
    ContextPlan(ContextPlanError),
    Extension(AgentExtensionError),
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
            Self::ContextPlan(error) => write!(f, "invalid context plan: {error}"),
            Self::Extension(error) => write!(f, "invalid agent extension: {error}"),
        }
    }
}

impl std::error::Error for AgentBuildError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::ContextPlan(error) => Some(error),
            Self::Extension(error) => Some(error),
            Self::TemplateNotSuspended
            | Self::ProviderUnavailable(_)
            | Self::SessionProviderMismatch { .. }
            | Self::EmptySessionId
            | Self::InvalidMaxCost => None,
        }
    }
}

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
