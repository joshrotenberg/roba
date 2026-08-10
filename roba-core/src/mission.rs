//! Provider-neutral projection for one finite Roba mission.

use serde::{Deserialize, Serialize};

use crate::{
    LimitSpec, PermissionPolicy, RunId, RunSnapshot, ToolPolicy, WorkerPolicy, WorkerSnapshot,
};

/// State claimed for one mission work item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MissionWorkState {
    Planned,
    InProgress,
    Completed,
    Blocked,
}

/// One agent-reported work item. Lifecycle facts remain host-derived.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MissionWorkItem {
    pub id: String,
    pub title: String,
    pub state: MissionWorkState,
    pub reported_by: RunId,
}

/// One agent-reported blocker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MissionBlocker {
    pub id: String,
    pub message: String,
    pub reported_by: RunId,
}

/// One agent-reported artifact or external reference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MissionArtifact {
    pub id: String,
    pub kind: String,
    pub reference: String,
    pub reported_by: RunId,
}

/// Typed claim accepted from the root agent or one of its owned workers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MissionReport {
    WorkItem {
        id: String,
        title: String,
        state: MissionWorkState,
    },
    Blocker {
        id: String,
        message: String,
    },
    Artifact {
        id: String,
        artifact_kind: String,
        reference: String,
    },
}

impl MissionReport {
    pub(crate) fn validate(&self) -> Result<(), MissionReportError> {
        let fields: &[(&str, &str)] = match self {
            Self::WorkItem { id, title, .. } => &[("id", id), ("title", title)],
            Self::Blocker { id, message } => &[("id", id), ("message", message)],
            Self::Artifact {
                id,
                artifact_kind,
                reference,
            } => &[
                ("id", id),
                ("artifact_kind", artifact_kind),
                ("reference", reference),
            ],
        };
        for (name, value) in fields {
            if value.trim().is_empty() {
                return Err(MissionReportError::EmptyField(name));
            }
        }
        Ok(())
    }
}

/// Invalid agent-reported mission claim.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MissionReportError {
    EmptyField(&'static str),
    RunUnavailable,
    MissionClosed,
}

impl std::fmt::Display for MissionReportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyField(field) => write!(f, "mission report field {field} must not be empty"),
            Self::RunUnavailable => f.write_str("mission run is no longer available"),
            Self::MissionClosed => {
                f.write_str("mission run is terminal and no longer accepts reports")
            }
        }
    }
}

impl std::error::Error for MissionReportError {}

/// Agent-reported projection, kept visibly separate from runtime facts.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MissionClaims {
    pub work_items: Vec<MissionWorkItem>,
    pub blockers: Vec<MissionBlocker>,
    pub artifacts: Vec<MissionArtifact>,
}

/// Immutable authority and bounds captured from the root specification.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MissionAuthority {
    pub permissions: PermissionPolicy,
    pub tools: ToolPolicy,
    pub limits: LimitSpec,
    pub workers: WorkerPolicy,
}

/// Canonical process-local monitoring projection for one finite mission.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MissionSnapshot {
    pub root: RunSnapshot,
    pub workers: Vec<WorkerSnapshot>,
    pub authority: MissionAuthority,
    pub claims: MissionClaims,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_reject_empty_identity_and_payload_fields() {
        for report in [
            MissionReport::WorkItem {
                id: " ".to_string(),
                title: "work".to_string(),
                state: MissionWorkState::Planned,
            },
            MissionReport::Blocker {
                id: "blocker".to_string(),
                message: String::new(),
            },
            MissionReport::Artifact {
                id: "artifact".to_string(),
                artifact_kind: "pull_request".to_string(),
                reference: String::new(),
            },
        ] {
            assert!(matches!(
                report.validate(),
                Err(MissionReportError::EmptyField(_))
            ));
        }
    }
}
