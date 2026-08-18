//! Supervision for operation-scoped extension lifecycle callbacks.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{broadcast, watch};
use tokio::task::JoinHandle;

use crate::contract::AgentTerminalState;
use crate::events::{AgentEventJournal, AgentEventRecord};
use crate::extensions::{
    AgentExtensionChange, AgentExtensionHookPhase, AgentExtensionLifecycleRegistration,
    AgentExtensionOperation, MAX_EXTENSION_HOOK_TIMEOUT,
};

const MIN_EXTENSION_HOOK_TIMEOUT: Duration = Duration::from_millis(1);
const EXTENSION_NAME_LIMIT: usize = 96;
const EXTENSION_FINGERPRINT_LIMIT: usize = 160;
const EXTENSION_SUMMARY_LIMIT: usize = 240;

pub(crate) struct ExtensionOperationSupervisor {
    registrations: Arc<[AgentExtensionLifecycleRegistration]>,
    operation: AgentExtensionOperation,
    events: AgentEventJournal,
    event_tx: broadcast::Sender<AgentEventRecord>,
    stop_tx: watch::Sender<bool>,
    pollers: Vec<JoinHandle<()>>,
}

impl ExtensionOperationSupervisor {
    pub(crate) async fn admitted(
        registrations: &[AgentExtensionLifecycleRegistration],
        operation: AgentExtensionOperation,
        events: AgentEventJournal,
        event_tx: broadcast::Sender<AgentEventRecord>,
    ) -> Self {
        let (stop_tx, _) = watch::channel(false);
        let mut supervisor = Self {
            registrations: Arc::from(registrations),
            operation,
            events,
            event_tx,
            stop_tx,
            pollers: Vec::new(),
        };
        supervisor.run_all(HookInvocation::Admitted).await;
        supervisor
    }

    pub(crate) async fn started(&mut self) {
        self.run_all(HookInvocation::Started).await;
        for registration in self.registrations.iter().cloned() {
            let Some(interval) = registration
                .lifecycle
                .poll_interval()
                .filter(|interval| !interval.is_zero())
            else {
                continue;
            };
            let mut stop = self.stop_tx.subscribe();
            let operation = self.operation.clone();
            let events = self.events.clone();
            let event_tx = self.event_tx.clone();
            self.pollers.push(tokio::spawn(async move {
                loop {
                    tokio::select! {
                        biased;
                        changed = stop.changed() => {
                            if changed.is_err() || *stop.borrow() {
                                break;
                            }
                        }
                        () = tokio::time::sleep(interval) => {
                            invoke_and_publish(
                                registration.clone(),
                                operation.clone(),
                                HookInvocation::Tick,
                                &events,
                                &event_tx,
                            ).await;
                        }
                    }
                }
            }));
        }
    }

    pub(crate) async fn settle(mut self, terminal: AgentTerminalState) {
        self.stop_tx.send_replace(true);
        for poller in self.pollers.drain(..) {
            let _ = poller.await;
        }
        self.run_all(HookInvocation::Settling(terminal)).await;
        self.run_all(HookInvocation::Settled(terminal)).await;
    }

    async fn run_all(&mut self, invocation: HookInvocation) {
        for registration in self.registrations.iter().cloned() {
            invoke_and_publish(
                registration,
                self.operation.clone(),
                invocation,
                &self.events,
                &self.event_tx,
            )
            .await;
        }
    }
}

pub(crate) async fn shutdown_extensions(registrations: &[AgentExtensionLifecycleRegistration]) {
    for registration in registrations.iter().cloned() {
        let timeout = bounded_timeout(registration.lifecycle.hook_timeout());
        let lifecycle = Arc::clone(&registration.lifecycle);
        let mut task = tokio::spawn(async move { lifecycle.host_shutdown().await });
        if tokio::time::timeout(timeout, &mut task).await.is_err() {
            task.abort();
            let _ = task.await;
        }
    }
}

#[derive(Clone, Copy)]
enum HookInvocation {
    Admitted,
    Started,
    Tick,
    Settling(AgentTerminalState),
    Settled(AgentTerminalState),
}

impl HookInvocation {
    fn phase(self) -> AgentExtensionHookPhase {
        match self {
            Self::Admitted => AgentExtensionHookPhase::Admitted,
            Self::Started => AgentExtensionHookPhase::Started,
            Self::Tick => AgentExtensionHookPhase::Tick,
            Self::Settling(_) => AgentExtensionHookPhase::Settling,
            Self::Settled(_) => AgentExtensionHookPhase::Settled,
        }
    }
}

async fn invoke_and_publish(
    registration: AgentExtensionLifecycleRegistration,
    operation: AgentExtensionOperation,
    invocation: HookInvocation,
    events: &AgentEventJournal,
    event_tx: &broadcast::Sender<AgentEventRecord>,
) {
    let phase = invocation.phase();
    let operation_id = operation.operation_id;
    let timeout = bounded_timeout(registration.lifecycle.hook_timeout());
    let lifecycle = Arc::clone(&registration.lifecycle);
    let mut task = tokio::spawn(async move {
        match invocation {
            HookInvocation::Admitted => lifecycle.operation_admitted(operation).await,
            HookInvocation::Started => lifecycle.operation_started(operation).await,
            HookInvocation::Tick => lifecycle.observation_tick(operation).await,
            HookInvocation::Settling(terminal) => {
                lifecycle.operation_settling(operation, terminal).await
            }
            HookInvocation::Settled(terminal) => {
                lifecycle.operation_settled(operation, terminal).await
            }
        }
    });

    let result = match tokio::time::timeout(timeout, &mut task).await {
        Ok(Ok(change)) => Ok(change),
        Ok(Err(_)) => Err(()),
        Err(_) => {
            task.abort();
            let _ = task.await;
            Err(())
        }
    };
    let extension = bounded_text(&registration.name, EXTENSION_NAME_LIMIT);
    let record = match result {
        Ok(Some(AgentExtensionChange {
            fingerprint,
            summary,
        })) => events.append_extension_changed(
            operation_id,
            extension,
            phase,
            bounded_token(&fingerprint, EXTENSION_FINGERPRINT_LIMIT),
            bounded_text(&summary, EXTENSION_SUMMARY_LIMIT),
        ),
        Ok(None) => return,
        Err(()) => events.append_extension_failed(operation_id, extension, phase),
    };
    if let Ok(record) = record {
        let _ = event_tx.send(record);
    }
}

fn bounded_timeout(requested: Duration) -> Duration {
    requested
        .max(MIN_EXTENSION_HOOK_TIMEOUT)
        .min(MAX_EXTENSION_HOOK_TIMEOUT)
}

fn bounded_token(value: &str, maximum: usize) -> String {
    value
        .chars()
        .filter(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
        .take(maximum)
        .collect()
}

fn bounded_text(value: &str, maximum: usize) -> String {
    let mut output = String::new();
    let mut previous_space = false;
    for character in value.chars() {
        let character = if character.is_control() {
            ' '
        } else {
            character
        };
        if character.is_whitespace() {
            if previous_space {
                continue;
            }
            previous_space = true;
            output.push(' ');
        } else {
            previous_space = false;
            output.push(character);
        }
        if output.chars().count() >= maximum {
            break;
        }
    }
    output.trim().to_owned()
}
