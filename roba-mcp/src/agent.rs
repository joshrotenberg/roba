//! Hot single-agent application state above finite core runs.

use std::fmt;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use roba_core::{
    Prompt, ProviderId, Roba, RunHandle, RunSnapshot, RunSpec, SessionHandle as CoreSessionHandle,
    SessionSpec,
};
use tokio::sync::{Mutex, oneshot};

use crate::contract::{
    AgentConfiguration, AgentRefusalKind, AgentSnapshot, AgentState, AgentTurnResult, OperationId,
};

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
    control: Mutex<Control>,
}

struct Control {
    stopped: bool,
    next_operation_id: u64,
    session: Option<CoreSessionHandle>,
    active: Option<ActiveOperation>,
    latest_turn: Option<AgentTurnResult>,
}

struct ActiveOperation {
    id: OperationId,
    handle: RunHandle,
}

impl AgentInstance {
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

        Ok(Self {
            inner: Arc::new(Inner {
                runtime,
                template,
                configuration,
                created_at_unix_ms: unix_time_ms(),
                control: Mutex::new(Control {
                    stopped: false,
                    next_operation_id: 1,
                    session,
                    active: None,
                    latest_turn: None,
                }),
            }),
        })
    }

    /// Submit one prompt and await its terminal finite-run result.
    ///
    /// Settlement belongs to a detached coordinator rather than the calling
    /// waiter. Dropping a caller waiting on this method therefore cannot wedge
    /// the agent in `running` after its provider turn finishes. MCP request
    /// cancellation semantics remain a separate control-layer decision.
    pub async fn turn(&self, text: String) -> AgentTurnResult {
        let reservation = {
            let mut control = self.inner.control.lock().await;
            if control.stopped {
                return AgentTurnResult::refused(
                    AgentRefusalKind::Stopped,
                    "agent is stopped",
                    None,
                );
            }
            if let Some(active) = &control.active {
                return AgentTurnResult::refused(
                    AgentRefusalKind::Busy,
                    format!("agent is already running operation {}", active.id.get()),
                    Some(active.id),
                );
            }
            let prompt = match Prompt::new(text) {
                Ok(prompt) => prompt,
                Err(error) => {
                    return AgentTurnResult::refused(
                        AgentRefusalKind::InvalidPrompt,
                        error.to_string(),
                        None,
                    );
                }
            };
            let Some(next_operation_id) = control.next_operation_id.checked_add(1) else {
                return AgentTurnResult::refused(
                    AgentRefusalKind::Runtime,
                    "agent operation identity exhausted",
                    None,
                );
            };
            let operation_id = OperationId::new(control.next_operation_id);

            let mut spec = self.inner.template.clone();
            spec.execution.session = match &control.session {
                Some(session) => SessionSpec::Resume {
                    session: session.clone(),
                },
                None => SessionSpec::Fresh,
            };
            let run = match self.inner.runtime.create_run(spec) {
                Ok(run) => run,
                Err(error) => {
                    return AgentTurnResult::refused(
                        AgentRefusalKind::Runtime,
                        error.to_string(),
                        None,
                    );
                }
            };
            let handle = run.handle();
            control.next_operation_id = next_operation_id;
            control.active = Some(ActiveOperation {
                id: operation_id,
                handle: handle.clone(),
            });
            (operation_id, prompt, handle)
        };

        let (operation_id, prompt, handle) = reservation;
        let (completion_tx, completion_rx) = oneshot::channel();
        let coordinator = self.clone();
        tokio::spawn(async move {
            let worker = tokio::spawn(async move {
                handle
                    .start(prompt)
                    .await
                    .map_err(|error| error.to_string())?;
                Ok::<RunSnapshot, String>(handle.wait().await)
            });
            let result = match worker.await {
                Ok(Ok(snapshot)) => coordinator.settle(operation_id, snapshot).await,
                Ok(Err(message)) => coordinator.abort(operation_id, message).await,
                Err(error) => {
                    coordinator
                        .abort(operation_id, format!("agent turn worker stopped: {error}"))
                        .await
                }
            };
            let _ = completion_tx.send(result);
        });

        match completion_rx.await {
            Ok(result) => result,
            Err(_) => {
                self.abort(operation_id, "agent turn coordinator stopped unexpectedly")
                    .await
            }
        }
    }

    /// Return the latest inspectable agent state without starting work.
    pub async fn snapshot(&self) -> AgentSnapshot {
        let control = self.inner.control.lock().await;
        AgentSnapshot {
            configuration: self.inner.configuration.clone(),
            state: if control.stopped {
                AgentState::Stopped
            } else if control.active.is_some() {
                AgentState::Running
            } else {
                AgentState::Idle
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

    /// Permanently stop an idle agent.
    ///
    /// Active-run shutdown and cancellation become MCP controls in Phase 3;
    /// this narrow method exists so the initial contract can prove stopped
    /// refusal without implying those later semantics.
    pub async fn stop(&self) -> Result<(), AgentStopError> {
        let mut control = self.inner.control.lock().await;
        if let Some(active) = &control.active {
            return Err(AgentStopError::Busy(active.id));
        }
        control.stopped = true;
        Ok(())
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
        result
    }

    async fn abort(
        &self,
        operation_id: OperationId,
        message: impl Into<String>,
    ) -> AgentTurnResult {
        let message = message.into();
        let handle = {
            let control = self.inner.control.lock().await;
            match &control.active {
                Some(active) if active.id == operation_id => active.handle.clone(),
                _ => {
                    return control
                        .latest_turn
                        .as_ref()
                        .filter(|latest| latest.operation_id() == Some(operation_id))
                        .cloned()
                        .unwrap_or_else(|| {
                            AgentTurnResult::refused(AgentRefusalKind::Runtime, message, None)
                        });
                }
            }
        };

        if let Err(error) = handle.cancel().await {
            let snapshot = handle.status().await;
            if snapshot.is_terminal() {
                return self.settle(operation_id, snapshot).await;
            }
            let recovery = self.clone();
            tokio::spawn(async move {
                recovery.settle(operation_id, handle.wait().await).await;
            });
            return AgentTurnResult::refused(
                AgentRefusalKind::Runtime,
                format!("{message}: cancellation also failed: {error}"),
                Some(operation_id),
            );
        }
        self.settle(operation_id, handle.wait().await).await
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
