//! Agent-wide event projection and bounded replay journal.

use std::collections::VecDeque;
use std::fmt;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tower_mcp::schemars::{self, JsonSchema};

use crate::contract::{
    ActivityKind, ActivityStatus, AgentTerminalState, Cost, FailureKind, OperationId,
    OperationSettlement, TokenUsage,
};
use crate::extensions::AgentExtensionHookPhase;

/// Maximum number of agent-wide events retained in memory and returned in one page.
pub const AGENT_EVENT_CAPACITY: usize = 256;

/// One globally sequenced event belonging to an admitted agent operation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AgentEventRecord {
    /// Monotonic sequence within this `AgentInstance`'s lifetime.
    pub sequence: u64,
    /// Identity of the admitted operation that produced the event.
    pub operation_id: OperationId,
    /// Original sequence from the finite run journal, when this mirrors a core event.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_sequence: Option<u64>,
    /// Original wall-clock occurrence time, when the source supplied one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub occurred_at_unix_ms: Option<u64>,
    /// Provider-neutral event payload.
    pub event: AgentEvent,
}

/// One cursor page from the bounded agent-wide event journal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AgentEventPage {
    /// Retained records strictly after the requested cursor.
    pub events: Vec<AgentEventRecord>,
    /// Highest sequence inspected by this page. Supply it as the next cursor.
    pub next_sequence: u64,
    /// Oldest sequence still retained by the bounded journal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oldest_sequence: Option<u64>,
    /// True when the requested cursor predates retained agent history.
    pub truncated: bool,
    /// True when the logical agent is stopped and can emit no future events.
    ///
    /// The journal initializes this to false; `AgentInstance` supplies the
    /// lifecycle-aware value when it serves the page.
    pub closed: bool,
}

/// Provider-neutral event projected from a finite run or its agent settlement.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AgentEvent {
    /// The finite run changed lifecycle state.
    StateChanged { state: AgentRunState },
    /// A provider turn began.
    TurnStarted { provider: String },
    /// Incremental assistant output became available.
    OutputDelta { text: String },
    /// The provider reported token usage.
    Usage { usage: TokenUsage },
    /// A provider or lifecycle warning was emitted.
    Warning { message: String },
    /// A provider-native activity began.
    ActivityStarted {
        id: String,
        activity: ActivityKind,
        summary: String,
    },
    /// A provider-native activity completed.
    ActivityCompleted {
        id: String,
        activity: ActivityKind,
        status: ActivityStatus,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        duration_ms: Option<u64>,
        summary: String,
    },
    /// A follow-up was queued for the next provider-turn boundary.
    FollowUpQueued,
    /// The oldest queued follow-up was applied to a resumed provider turn.
    FollowUpApplied,
    /// One provider turn completed successfully.
    ///
    /// The public payload deliberately has no provider session field.
    TurnCompleted { outcome: EventTurnOutcome },
    /// The finite run failed.
    ///
    /// The public payload deliberately has no provider session field.
    Failed { failure: EventTurnFailure },
    /// The finite run journal evicted records before the fan-out could consume them.
    RunHistoryTruncated {
        /// Oldest source run sequence still available, when known.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        oldest_run_sequence: Option<u64>,
    },
    /// An extension observed a compact operation-scoped state change.
    ExtensionChanged {
        extension: String,
        phase: AgentExtensionHookPhase,
        fingerprint: String,
        summary: String,
    },
    /// An extension callback panicked or exceeded its host-enforced timeout.
    ExtensionFailed {
        extension: String,
        phase: AgentExtensionHookPhase,
    },
    /// Agent bookkeeping for this operation has committed after core termination.
    OperationSettled { state: AgentTerminalState },
}

impl From<roba_core::RunEvent> for AgentEvent {
    fn from(value: roba_core::RunEvent) -> Self {
        match value {
            roba_core::RunEvent::StateChanged { state } => Self::StateChanged {
                state: state.into(),
            },
            roba_core::RunEvent::TurnStarted { provider } => Self::TurnStarted {
                provider: provider.to_string(),
            },
            roba_core::RunEvent::OutputDelta { text } => Self::OutputDelta { text },
            roba_core::RunEvent::Usage { usage } => Self::Usage {
                usage: usage.into(),
            },
            roba_core::RunEvent::Warning { message } => Self::Warning { message },
            roba_core::RunEvent::ActivityStarted {
                id,
                activity,
                summary,
            } => Self::ActivityStarted {
                id,
                activity: activity.into(),
                summary,
            },
            roba_core::RunEvent::ActivityCompleted {
                id,
                activity,
                status,
                duration_ms,
                summary,
            } => Self::ActivityCompleted {
                id,
                activity: activity.into(),
                status: status.into(),
                duration_ms,
                summary,
            },
            roba_core::RunEvent::FollowUpQueued => Self::FollowUpQueued,
            roba_core::RunEvent::FollowUpApplied => Self::FollowUpApplied,
            roba_core::RunEvent::TurnCompleted { outcome } => Self::TurnCompleted {
                outcome: outcome.into(),
            },
            roba_core::RunEvent::Failed { failure } => Self::Failed {
                failure: failure.into(),
            },
        }
    }
}

/// Public lifecycle state mirrored from one finite core run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AgentRunState {
    Suspended,
    Ready,
    Running,
    Finishing,
    Completed,
    Failed,
    Cancelled,
}

impl From<roba_core::RunState> for AgentRunState {
    fn from(value: roba_core::RunState) -> Self {
        match value {
            roba_core::RunState::Suspended => Self::Suspended,
            roba_core::RunState::Ready => Self::Ready,
            roba_core::RunState::Running => Self::Running,
            roba_core::RunState::Finishing => Self::Finishing,
            roba_core::RunState::Completed => Self::Completed,
            roba_core::RunState::Failed => Self::Failed,
            roba_core::RunState::Cancelled => Self::Cancelled,
        }
    }
}

/// Successful provider outcome safe for agent-wide observation.
///
/// Unlike the initiating caller's turn result, this replay DTO cannot carry a
/// provider session handle.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct EventTurnOutcome {
    pub output: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<TokenUsage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<Cost>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_turns: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured_output: Option<serde_json::Value>,
}

impl From<roba_core::RunOutcome> for EventTurnOutcome {
    fn from(value: roba_core::RunOutcome) -> Self {
        Self {
            output: value.output,
            usage: value.usage.map(Into::into),
            cost: value
                .cost
                .filter(|cost| cost.amount.is_finite())
                .map(Into::into),
            duration_ms: value.duration_ms,
            provider_turns: value.provider_turns,
            structured_output: value.structured_output,
        }
    }
}

/// Terminal failure safe for agent-wide observation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct EventTurnFailure {
    pub kind: FailureKind,
    pub message: String,
    #[serde(default, skip_serializing_if = "EventFailureDetails::is_empty")]
    pub details: EventFailureDetails,
}

impl From<roba_core::RunFailure> for EventTurnFailure {
    fn from(value: roba_core::RunFailure) -> Self {
        Self {
            kind: value.kind.into(),
            message: value.message,
            details: value.details.into(),
        }
    }
}

/// Provider accounting evidence safe for agent-wide failure replay.
///
/// The provider session handle is intentionally absent.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct EventFailureDetails {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<TokenUsage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<Cost>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_turns: Option<u32>,
}

impl EventFailureDetails {
    fn is_empty(&self) -> bool {
        self == &Self::default()
    }
}

impl From<roba_core::RunFailureDetails> for EventFailureDetails {
    fn from(value: roba_core::RunFailureDetails) -> Self {
        Self {
            usage: value.usage.map(Into::into),
            cost: value
                .cost
                .filter(|cost| cost.amount.is_finite())
                .map(Into::into),
            duration_ms: value.duration_ms,
            provider_turns: value.provider_turns,
        }
    }
}

/// Invalid agent-wide event journal operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AgentEventError {
    /// A page limit was zero or exceeded this journal's capacity.
    InvalidEventLimit { requested: usize, maximum: usize },
    /// A cursor named an event that has never existed in this journal.
    EventCursorAhead { requested: u64, newest: u64 },
    /// No further global sequence can be allocated safely.
    SequenceExhausted,
}

impl fmt::Display for AgentEventError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidEventLimit { maximum, .. } => {
                write!(
                    formatter,
                    "event page limit must be between 1 and {maximum}"
                )
            }
            Self::EventCursorAhead { requested, newest } => write!(
                formatter,
                "event cursor {requested} is ahead of newest sequence {newest}"
            ),
            Self::SequenceExhausted => formatter.write_str("agent event sequence exhausted"),
        }
    }
}

impl std::error::Error for AgentEventError {}

/// Cloneable, process-local journal shared by the agent and its event fan-out.
#[derive(Clone)]
pub(crate) struct AgentEventJournal {
    inner: Arc<JournalInner>,
}

struct JournalInner {
    capacity: usize,
    state: Mutex<JournalState>,
}

struct JournalState {
    next_sequence: u64,
    records: VecDeque<AgentEventRecord>,
    evicted_through: u64,
}

impl AgentEventJournal {
    pub(crate) fn new() -> Self {
        Self::with_capacity(AGENT_EVENT_CAPACITY)
    }

    fn with_capacity(capacity: usize) -> Self {
        assert!(
            capacity > 0,
            "agent event journal capacity must be positive"
        );
        Self {
            inner: Arc::new(JournalInner {
                capacity,
                state: Mutex::new(JournalState {
                    next_sequence: 1,
                    records: VecDeque::with_capacity(capacity),
                    evicted_through: 0,
                }),
            }),
        }
    }

    fn append(
        &self,
        operation_id: OperationId,
        run_sequence: Option<u64>,
        occurred_at_unix_ms: Option<u64>,
        event: AgentEvent,
    ) -> Result<AgentEventRecord, AgentEventError> {
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let sequence = state.next_sequence;
        state.next_sequence = sequence
            .checked_add(1)
            .ok_or(AgentEventError::SequenceExhausted)?;

        if state.records.len() == self.inner.capacity
            && let Some(evicted) = state.records.pop_front()
        {
            state.evicted_through = state.evicted_through.max(evicted.sequence);
        }
        let record = AgentEventRecord {
            sequence,
            operation_id,
            run_sequence,
            occurred_at_unix_ms,
            event,
        };
        state.records.push_back(record.clone());
        Ok(record)
    }

    pub(crate) fn append_core(
        &self,
        operation_id: OperationId,
        record: roba_core::RunEventRecord,
    ) -> Result<AgentEventRecord, AgentEventError> {
        self.append(
            operation_id,
            Some(record.sequence),
            record.occurred_at_unix_ms,
            record.event.into(),
        )
    }

    pub(crate) fn append_history_gap(
        &self,
        operation_id: OperationId,
        oldest_run_sequence: Option<u64>,
    ) -> Result<AgentEventRecord, AgentEventError> {
        self.append(
            operation_id,
            None,
            None,
            AgentEvent::RunHistoryTruncated {
                oldest_run_sequence,
            },
        )
    }

    pub(crate) fn append_settled(
        &self,
        settlement: OperationSettlement,
    ) -> Result<AgentEventRecord, AgentEventError> {
        self.append(
            settlement.operation_id,
            None,
            None,
            AgentEvent::OperationSettled {
                state: settlement.state,
            },
        )
    }

    pub(crate) fn append_extension_changed(
        &self,
        operation_id: OperationId,
        extension: String,
        phase: AgentExtensionHookPhase,
        fingerprint: String,
        summary: String,
    ) -> Result<AgentEventRecord, AgentEventError> {
        self.append(
            operation_id,
            None,
            None,
            AgentEvent::ExtensionChanged {
                extension,
                phase,
                fingerprint,
                summary,
            },
        )
    }

    pub(crate) fn append_extension_failed(
        &self,
        operation_id: OperationId,
        extension: String,
        phase: AgentExtensionHookPhase,
    ) -> Result<AgentEventRecord, AgentEventError> {
        self.append(
            operation_id,
            None,
            None,
            AgentEvent::ExtensionFailed { extension, phase },
        )
    }

    pub(crate) fn page(&self, after: u64, limit: usize) -> Result<AgentEventPage, AgentEventError> {
        if !(1..=self.inner.capacity).contains(&limit) {
            return Err(AgentEventError::InvalidEventLimit {
                requested: limit,
                maximum: self.inner.capacity,
            });
        }

        let state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let newest = state.next_sequence.saturating_sub(1);
        if after > newest {
            return Err(AgentEventError::EventCursorAhead {
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
        Ok(AgentEventPage {
            events,
            next_sequence,
            oldest_sequence: state.records.front().map(|record| record.sequence),
            truncated: after < state.evicted_through,
            closed: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use roba_core::{
        ProviderId, RunEvent, RunEventRecord, RunFailure, RunFailureDetails, RunOutcome,
        SessionHandle,
    };

    fn operation(value: u64) -> OperationId {
        OperationId::new(value)
    }

    fn warning(message: &str) -> AgentEvent {
        AgentEvent::Warning {
            message: message.to_owned(),
        }
    }

    fn core_warning(sequence: u64, occurred_at_unix_ms: u64, message: &str) -> RunEventRecord {
        RunEventRecord {
            sequence,
            occurred_at_unix_ms: Some(occurred_at_unix_ms),
            event: RunEvent::Warning {
                message: message.to_owned(),
            },
        }
    }

    #[test]
    fn sequences_and_pages_span_operations_monotonically() {
        let journal = AgentEventJournal::with_capacity(4);
        let clone = journal.clone();
        let first = journal
            .append_core(operation(1), core_warning(1, 10, "one"))
            .unwrap();
        let second = clone
            .append_core(operation(1), core_warning(2, 20, "two"))
            .unwrap();
        let third = journal.append_history_gap(operation(2), Some(7)).unwrap();
        let fourth = journal
            .append_settled(OperationSettlement {
                operation_id: operation(2),
                state: AgentTerminalState::Cancelled,
            })
            .unwrap();

        assert_eq!(
            (
                first.sequence,
                second.sequence,
                third.sequence,
                fourth.sequence,
            ),
            (1, 2, 3, 4)
        );
        assert_eq!(
            (first.run_sequence, first.occurred_at_unix_ms),
            (Some(1), Some(10))
        );
        let first_page = journal.page(0, 2).unwrap();
        assert_eq!(
            first_page
                .events
                .iter()
                .map(|record| record.sequence)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert_eq!(first_page.next_sequence, 2);
        assert_eq!(first_page.oldest_sequence, Some(1));
        assert!(!first_page.truncated);

        let second_page = journal.page(first_page.next_sequence, 2).unwrap();
        assert_eq!(second_page.events, vec![third, fourth]);
        assert_eq!(second_page.next_sequence, 4);
    }

    #[test]
    fn eviction_reports_truncation_and_the_oldest_retained_sequence() {
        let journal = AgentEventJournal::with_capacity(2);
        for message in ["one", "two", "three"] {
            journal
                .append(operation(1), None, None, warning(message))
                .unwrap();
        }

        let truncated = journal.page(0, 2).unwrap();
        assert!(truncated.truncated);
        assert_eq!(truncated.oldest_sequence, Some(2));
        assert_eq!(
            truncated
                .events
                .iter()
                .map(|record| record.sequence)
                .collect::<Vec<_>>(),
            vec![2, 3]
        );

        let caught_up = journal.page(1, 2).unwrap();
        assert!(!caught_up.truncated);
        assert_eq!(caught_up.next_sequence, 3);
    }

    #[test]
    fn future_cursors_fail_with_the_requested_and_newest_sequences() {
        let journal = AgentEventJournal::with_capacity(2);
        journal
            .append(operation(1), None, None, warning("one"))
            .unwrap();

        assert_eq!(
            journal.page(2, 1).unwrap_err(),
            AgentEventError::EventCursorAhead {
                requested: 2,
                newest: 1,
            }
        );
    }

    #[test]
    fn zero_and_over_capacity_limits_fail_loudly() {
        let journal = AgentEventJournal::with_capacity(2);

        assert_eq!(
            journal.page(0, 0).unwrap_err(),
            AgentEventError::InvalidEventLimit {
                requested: 0,
                maximum: 2,
            }
        );
        assert_eq!(
            journal.page(0, 3).unwrap_err(),
            AgentEventError::InvalidEventLimit {
                requested: 3,
                maximum: 2,
            }
        );
    }

    #[test]
    fn completed_and_failed_events_redact_session_and_non_finite_cost() {
        let session = SessionHandle {
            provider: ProviderId::claude(),
            id: "secret-session".to_owned(),
        };
        let completed = AgentEvent::from(RunEvent::TurnCompleted {
            outcome: RunOutcome {
                output: "done".to_owned(),
                session: Some(session.clone()),
                usage: None,
                cost: Some(roba_core::Cost::usd(f64::NAN)),
                duration_ms: Some(1),
                provider_turns: Some(1),
                structured_output: None,
            },
        });
        let failed = AgentEvent::from(RunEvent::Failed {
            failure: RunFailure {
                kind: roba_core::FailureKind::Provider,
                message: "failed".to_owned(),
                details: RunFailureDetails {
                    session: Some(session),
                    usage: None,
                    cost: Some(roba_core::Cost::usd(f64::NAN)),
                    duration_ms: Some(2),
                    provider_turns: Some(1),
                },
            },
        });

        let completed_json = serde_json::to_value(completed).unwrap();
        let failed_json = serde_json::to_value(failed).unwrap();
        assert!(
            completed_json
                .get("outcome")
                .unwrap()
                .get("session")
                .is_none()
        );
        assert!(completed_json.get("outcome").unwrap().get("cost").is_none());
        assert!(
            failed_json
                .get("failure")
                .unwrap()
                .get("details")
                .unwrap()
                .get("session")
                .is_none()
        );
        assert!(
            failed_json
                .get("failure")
                .unwrap()
                .get("details")
                .unwrap()
                .get("cost")
                .is_none()
        );
        assert!(!completed_json.to_string().contains("secret-session"));
        assert!(!failed_json.to_string().contains("secret-session"));
    }
}
