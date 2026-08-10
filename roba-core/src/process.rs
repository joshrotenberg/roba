//! Host-owned process capabilities for finite missions.
//!
//! A process capability describes a workflow the host knows how to perform.
//! Declaring one does not grant authority: every capability lists the exact
//! grants it requires, and [`Roba`](crate::Roba) validates the immutable
//! mission policy before constructing a run or starting provider work.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use crate::RunId;

macro_rules! stable_id {
    ($name:ident, $error:ident, $what:literal) => {
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self, $error> {
                let value = value.into();
                if valid_identifier(&value) {
                    Ok(Self(value))
                } else {
                    Err($error)
                }
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                let value = String::deserialize(deserializer)?;
                Self::new(value).map_err(serde::de::Error::custom)
            }
        }

        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub struct $error;

        impl fmt::Display for $error {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(
                    f,
                    concat!($what, " must be a lowercase slash-separated identifier")
                )
            }
        }

        impl std::error::Error for $error {}
    };
}

stable_id!(
    ProcessCapabilityId,
    ProcessCapabilityIdError,
    "process capability id"
);
stable_id!(
    AuthorityGrantId,
    AuthorityGrantIdError,
    "authority grant id"
);
stable_id!(ProcessActionId, ProcessActionIdError, "process action id");

fn valid_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.split('/').all(|segment| {
            !segment.is_empty()
                && segment.bytes().all(|byte| {
                    byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"._-".contains(&byte)
                })
        })
}

/// How Roba decides that the finite mission is over.
///
/// Only root-terminal completion is currently executable. New policies must
/// be backed by host verification before they are added here.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum CompletionPolicy {
    #[default]
    RootTerminal,
}

/// Immutable process declarations captured from the root run.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct MissionPolicy {
    capabilities: BTreeSet<ProcessCapabilityId>,
    grants: BTreeSet<AuthorityGrantId>,
    completion: CompletionPolicy,
}

impl MissionPolicy {
    pub fn new(
        capabilities: impl IntoIterator<Item = ProcessCapabilityId>,
        grants: impl IntoIterator<Item = AuthorityGrantId>,
        completion: CompletionPolicy,
    ) -> Result<Self, MissionPolicyError> {
        let capabilities = collect_unique(capabilities, "capability")?;
        let grants = collect_unique(grants, "grant")?;
        Ok(Self {
            capabilities,
            grants,
            completion,
        })
    }

    pub fn capabilities(&self) -> &BTreeSet<ProcessCapabilityId> {
        &self.capabilities
    }

    pub fn grants(&self) -> &BTreeSet<AuthorityGrantId> {
        &self.grants
    }

    pub fn completion(&self) -> CompletionPolicy {
        self.completion
    }

    pub fn is_empty(&self) -> bool {
        self.capabilities.is_empty()
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawMissionPolicy {
    #[serde(default)]
    capabilities: Vec<ProcessCapabilityId>,
    #[serde(default)]
    grants: Vec<AuthorityGrantId>,
    #[serde(default)]
    completion: CompletionPolicy,
}

impl<'de> Deserialize<'de> for MissionPolicy {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = RawMissionPolicy::deserialize(deserializer)?;
        Self::new(raw.capabilities, raw.grants, raw.completion).map_err(serde::de::Error::custom)
    }
}

fn collect_unique<T: Ord>(
    values: impl IntoIterator<Item = T>,
    kind: &'static str,
) -> Result<BTreeSet<T>, MissionPolicyError> {
    let mut result = BTreeSet::new();
    for value in values {
        if !result.insert(value) {
            return Err(MissionPolicyError::Duplicate(kind));
        }
    }
    Ok(result)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MissionPolicyError {
    Duplicate(&'static str),
}

impl fmt::Display for MissionPolicyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Duplicate(kind) => write!(f, "mission policy contains a duplicate {kind}"),
        }
    }
}

impl std::error::Error for MissionPolicyError {}

/// One action exposed by a process capability.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessActionSpec {
    pub id: ProcessActionId,
    pub description: String,
    /// JSON Schema for the action input. The generic private MCP dispatcher
    /// publishes it; the host implementation validates the supplied value
    /// before side effects.
    #[serde(default = "default_object_schema")]
    pub input_schema: Value,
    #[serde(default)]
    pub destructive: bool,
}

fn default_object_schema() -> Value {
    serde_json::json!({ "type": "object" })
}

/// Stable description projected to the executing agent and monitors.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessCapabilityDescriptor {
    pub id: ProcessCapabilityId,
    pub description: String,
    pub required_grants: BTreeSet<AuthorityGrantId>,
    pub actions: Vec<ProcessActionSpec>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub instructions: Vec<String>,
}

/// One run-bound action invocation. The caller cannot supply grants or alter
/// the mission policy; the host registry already validated both.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProcessActionRequest {
    pub run_id: RunId,
    pub action: ProcessActionId,
    #[serde(default)]
    pub input: Value,
}

pub type ProcessFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Value, ProcessCapabilityError>> + Send + 'a>>;

/// Host implementation of one process capability.
pub trait ProcessCapability: Send + Sync {
    fn descriptor(&self) -> ProcessCapabilityDescriptor;
    fn invoke<'a>(&'a self, request: ProcessActionRequest) -> ProcessFuture<'a>;
}

#[derive(Clone)]
pub(crate) struct RegisteredProcessCapability {
    pub descriptor: ProcessCapabilityDescriptor,
    pub implementation: Arc<dyn ProcessCapability>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessCapabilityError(pub String);

impl fmt::Display for ProcessCapabilityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ProcessCapabilityError {}

/// Least-authority process control minted for one exact run.
#[derive(Clone)]
pub struct ProcessControl {
    run_id: RunId,
    capabilities: Arc<BTreeMap<ProcessCapabilityId, RegisteredProcessCapability>>,
    descriptors: Arc<Vec<ProcessCapabilityDescriptor>>,
}

impl ProcessControl {
    pub(crate) fn new(
        run_id: RunId,
        capabilities: BTreeMap<ProcessCapabilityId, RegisteredProcessCapability>,
    ) -> Self {
        let descriptors = capabilities
            .values()
            .map(|item| item.descriptor.clone())
            .collect();
        Self {
            run_id,
            capabilities: Arc::new(capabilities),
            descriptors: Arc::new(descriptors),
        }
    }

    pub fn descriptors(&self) -> &[ProcessCapabilityDescriptor] {
        &self.descriptors
    }

    pub fn instructions(&self) -> impl Iterator<Item = &str> {
        self.descriptors
            .iter()
            .flat_map(|descriptor| descriptor.instructions.iter().map(String::as_str))
    }

    pub async fn invoke(
        &self,
        capability: &ProcessCapabilityId,
        action: ProcessActionId,
        input: Value,
    ) -> Result<Value, ProcessControlError> {
        let implementation = self
            .capabilities
            .get(capability)
            .ok_or_else(|| ProcessControlError::CapabilityUnavailable(capability.clone()))?;
        if !implementation
            .descriptor
            .actions
            .iter()
            .any(|candidate| candidate.id == action)
        {
            return Err(ProcessControlError::ActionUnavailable {
                capability: capability.clone(),
                action,
            });
        }
        implementation
            .implementation
            .invoke(ProcessActionRequest {
                run_id: self.run_id,
                action,
                input,
            })
            .await
            .map_err(ProcessControlError::Invocation)
    }

    #[cfg(test)]
    pub(crate) fn test_control(descriptor: ProcessCapabilityDescriptor) -> Self {
        struct TestCapability(ProcessCapabilityDescriptor);

        impl ProcessCapability for TestCapability {
            fn descriptor(&self) -> ProcessCapabilityDescriptor {
                self.0.clone()
            }

            fn invoke<'a>(&'a self, _request: ProcessActionRequest) -> ProcessFuture<'a> {
                Box::pin(async { Ok(Value::Null) })
            }
        }

        let id = descriptor.id.clone();
        let implementation: Arc<dyn ProcessCapability> =
            Arc::new(TestCapability(descriptor.clone()));
        Self::new(
            RunId::ROOT,
            BTreeMap::from([(
                id,
                RegisteredProcessCapability {
                    descriptor,
                    implementation,
                },
            )]),
        )
    }
}

impl fmt::Debug for ProcessControl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProcessControl")
            .field("run_id", &self.run_id)
            .field("descriptors", &self.descriptors)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProcessControlError {
    CapabilityUnavailable(ProcessCapabilityId),
    ActionUnavailable {
        capability: ProcessCapabilityId,
        action: ProcessActionId,
    },
    Invocation(ProcessCapabilityError),
}

impl fmt::Display for ProcessControlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CapabilityUnavailable(id) => write!(f, "process capability {id} is unavailable"),
            Self::ActionUnavailable { capability, action } => {
                write!(f, "process action {capability}/{action} is unavailable")
            }
            Self::Invocation(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for ProcessControlError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_and_policy_are_strict_and_deterministic() {
        assert!(ProcessCapabilityId::new("repo/issues").is_ok());
        for invalid in ["", "Repo/issues", "/repo", "repo//issues", "repo issues"] {
            assert!(ProcessCapabilityId::new(invalid).is_err(), "{invalid}");
        }

        let policy = MissionPolicy::new(
            [
                ProcessCapabilityId::new("repo/issues").unwrap(),
                ProcessCapabilityId::new("github/pull-request").unwrap(),
            ],
            [AuthorityGrantId::new("repo/write").unwrap()],
            CompletionPolicy::RootTerminal,
        )
        .unwrap();
        let json = serde_json::to_string(&policy).unwrap();
        assert_eq!(
            json,
            r#"{"capabilities":["github/pull-request","repo/issues"],"grants":["repo/write"],"completion":"root_terminal"}"#
        );
        assert!(
            serde_json::from_str::<MissionPolicy>(
                r#"{"capabilities":["repo/issues","repo/issues"]}"#
            )
            .is_err()
        );
    }
}
