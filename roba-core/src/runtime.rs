//! Provider registry and ergonomic library entry point.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use crate::lifecycle::{Run, RunControlError};
use crate::process::{
    AuthorityGrantId, ProcessCapability, ProcessCapabilityDescriptor, ProcessCapabilityId,
    RegisteredProcessCapability,
};
use crate::provider::Provider;
use crate::run::{ProviderId, RunSpec};

/// Process-local Roba runtime. It owns provider adapters but no daemon,
/// database, queue, or global session pool.
#[derive(Default)]
pub struct Roba {
    providers: BTreeMap<ProviderId, Arc<dyn Provider>>,
    capabilities: BTreeMap<ProcessCapabilityId, RegisteredProcessCapability>,
}

impl Roba {
    /// Empty runtime. Register only the providers the host intends to allow.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a provider. Duplicate ids are refused rather than silently
    /// replacing a host's security or credential configuration.
    pub fn register<P>(&mut self, provider: P) -> Result<(), RuntimeError>
    where
        P: Provider + 'static,
    {
        self.register_shared(Arc::new(provider))
    }

    /// Register an already shared provider.
    pub fn register_shared(&mut self, provider: Arc<dyn Provider>) -> Result<(), RuntimeError> {
        let id = provider.id();
        if self.providers.contains_key(&id) {
            return Err(RuntimeError::DuplicateProvider(id));
        }
        self.providers.insert(id, provider);
        Ok(())
    }

    /// Register one host-owned process capability. Registration describes
    /// knowledge only; a run must still declare it and carry every required
    /// authority grant.
    pub fn register_capability<C>(&mut self, capability: C) -> Result<(), RuntimeError>
    where
        C: ProcessCapability + 'static,
    {
        self.register_capability_shared(Arc::new(capability))
    }

    /// Register an already shared process capability.
    pub fn register_capability_shared(
        &mut self,
        capability: Arc<dyn ProcessCapability>,
    ) -> Result<(), RuntimeError> {
        let descriptor = capability.descriptor();
        validate_descriptor(&descriptor)?;
        if self.capabilities.contains_key(&descriptor.id) {
            return Err(RuntimeError::DuplicateCapability(descriptor.id));
        }
        self.capabilities.insert(
            descriptor.id.clone(),
            RegisteredProcessCapability {
                descriptor,
                implementation: capability,
            },
        );
        Ok(())
    }

    /// True when this host can execute the provider id.
    pub fn contains(&self, id: &ProviderId) -> bool {
        self.providers.contains_key(id)
    }

    /// Provider ids in deterministic order.
    pub fn provider_ids(&self) -> impl Iterator<Item = &ProviderId> {
        self.providers.keys()
    }

    /// Construct one bounded run without starting provider work.
    pub fn create_run(&self, spec: RunSpec) -> Result<Run, RuntimeError> {
        let provider = self
            .providers
            .get(&spec.agent.provider)
            .ok_or_else(|| RuntimeError::ProviderUnavailable(spec.agent.provider.clone()))?;
        if !spec.mission.is_empty() && !provider.supports_process_control() {
            return Err(RuntimeError::ProviderProcessControlUnavailable(
                spec.agent.provider.clone(),
            ));
        }
        let capabilities = self.resolve_capabilities(&spec)?;
        Run::with_components(spec, self.providers.clone(), capabilities).map_err(RuntimeError::Run)
    }

    fn resolve_capabilities(
        &self,
        spec: &RunSpec,
    ) -> Result<BTreeMap<ProcessCapabilityId, RegisteredProcessCapability>, RuntimeError> {
        let mut resolved = BTreeMap::new();
        let mut consumed_grants = std::collections::BTreeSet::new();
        for id in spec.mission.capabilities() {
            let capability = self
                .capabilities
                .get(id)
                .ok_or_else(|| RuntimeError::CapabilityUnavailable(id.clone()))?;
            for grant in &capability.descriptor.required_grants {
                if !spec.mission.grants().contains(grant) {
                    return Err(RuntimeError::MissingCapabilityGrant {
                        capability: id.clone(),
                        grant: grant.clone(),
                    });
                }
                consumed_grants.insert(grant.clone());
            }
            let mut capability = capability.clone();
            for action in &capability.descriptor.actions {
                consumed_grants.extend(action.required_grants.iter().cloned());
            }
            capability.descriptor.actions.retain(|action| {
                action
                    .required_grants
                    .iter()
                    .all(|grant| spec.mission.grants().contains(grant))
            });
            resolved.insert(id.clone(), capability);
        }
        if let Some(grant) = spec
            .mission
            .grants()
            .iter()
            .find(|grant| !consumed_grants.contains(*grant))
        {
            return Err(RuntimeError::UnusedMissionGrant(grant.clone()));
        }
        Ok(resolved)
    }
}

fn validate_descriptor(descriptor: &ProcessCapabilityDescriptor) -> Result<(), RuntimeError> {
    if descriptor.description.trim().is_empty() {
        return Err(RuntimeError::InvalidCapability {
            capability: descriptor.id.clone(),
            reason: "description must not be empty".to_string(),
        });
    }
    let mut actions = std::collections::BTreeSet::new();
    for action in &descriptor.actions {
        if action.description.trim().is_empty() {
            return Err(RuntimeError::InvalidCapability {
                capability: descriptor.id.clone(),
                reason: format!("action {} has an empty description", action.id),
            });
        }
        if !actions.insert(action.id.clone()) {
            return Err(RuntimeError::InvalidCapability {
                capability: descriptor.id.clone(),
                reason: format!("action {} is declared more than once", action.id),
            });
        }
    }
    Ok(())
}

/// Runtime construction error.
#[derive(Debug, Clone, PartialEq)]
pub enum RuntimeError {
    DuplicateProvider(ProviderId),
    ProviderUnavailable(ProviderId),
    ProviderProcessControlUnavailable(ProviderId),
    DuplicateCapability(ProcessCapabilityId),
    CapabilityUnavailable(ProcessCapabilityId),
    MissingCapabilityGrant {
        capability: ProcessCapabilityId,
        grant: AuthorityGrantId,
    },
    UnusedMissionGrant(AuthorityGrantId),
    InvalidCapability {
        capability: ProcessCapabilityId,
        reason: String,
    },
    Run(RunControlError),
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateProvider(id) => write!(f, "provider {id} is already registered"),
            Self::ProviderUnavailable(id) => write!(f, "provider {id} is not registered"),
            Self::ProviderProcessControlUnavailable(id) => {
                write!(f, "provider {id} has no private process-control transport")
            }
            Self::DuplicateCapability(id) => {
                write!(f, "process capability {id} is already registered")
            }
            Self::CapabilityUnavailable(id) => {
                write!(f, "process capability {id} is not registered")
            }
            Self::MissingCapabilityGrant { capability, grant } => write!(
                f,
                "process capability {capability} requires undeclared grant {grant}"
            ),
            Self::UnusedMissionGrant(grant) => {
                write!(
                    f,
                    "mission grant {grant} is not required by a declared capability"
                )
            }
            Self::InvalidCapability { capability, reason } => {
                write!(f, "process capability {capability} is invalid: {reason}")
            }
            Self::Run(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for RuntimeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Run(error) => Some(error),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::{
        CompletionPolicy, MissionPolicy, ProcessActionId, ProcessActionRequest, ProcessActionSpec,
        ProcessCapabilityError, ProcessFuture,
    };
    use crate::provider::{
        EventSink, ProviderCapabilities, ProviderContext, ProviderError, ProviderFuture,
    };
    use crate::run::{AgentSpec, RunOutcome, TurnRequest};

    struct FakeProvider;
    struct RawProvider;

    struct FakeProcess;

    impl ProcessCapability for FakeProcess {
        fn descriptor(&self) -> ProcessCapabilityDescriptor {
            ProcessCapabilityDescriptor {
                id: ProcessCapabilityId::new("test/process").unwrap(),
                description: "deterministic test process".to_string(),
                required_grants: [AuthorityGrantId::new("test/write").unwrap()]
                    .into_iter()
                    .collect(),
                actions: vec![
                    ProcessActionSpec {
                        id: ProcessActionId::new("record").unwrap(),
                        description: "record a value".to_string(),
                        input_schema: serde_json::json!({"type": "object"}),
                        required_grants: Default::default(),
                        scope: crate::ProcessActionScope::RunTree,
                        destructive: false,
                    },
                    ProcessActionSpec {
                        id: ProcessActionId::new("admin").unwrap(),
                        description: "perform an elevated action".to_string(),
                        input_schema: serde_json::json!({"type": "object"}),
                        required_grants: [AuthorityGrantId::new("test/admin").unwrap()]
                            .into_iter()
                            .collect(),
                        scope: crate::ProcessActionScope::RootOnly,
                        destructive: true,
                    },
                ],
                instructions: vec!["Follow the deterministic test process.".to_string()],
            }
        }

        fn invoke<'a>(&'a self, request: ProcessActionRequest) -> ProcessFuture<'a> {
            Box::pin(async move {
                if request.action.as_str() != "record" {
                    return Err(ProcessCapabilityError("unexpected action".to_string()));
                }
                Ok(serde_json::json!({
                    "run_id": request.run_id,
                    "recorded": request.input,
                }))
            })
        }
    }

    impl Provider for FakeProvider {
        fn id(&self) -> ProviderId {
            ProviderId::new("fake").unwrap()
        }

        fn capabilities(&self) -> ProviderCapabilities {
            ProviderCapabilities::default()
        }

        fn supports_process_control(&self) -> bool {
            true
        }

        fn validate(&self, _request: &TurnRequest) -> Result<(), ProviderError> {
            Ok(())
        }

        fn execute<'a>(
            &'a self,
            _request: TurnRequest,
            _context: ProviderContext,
            _events: &'a dyn EventSink,
        ) -> ProviderFuture<'a> {
            Box::pin(async {
                Ok(RunOutcome {
                    output: String::new(),
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

    impl Provider for RawProvider {
        fn id(&self) -> ProviderId {
            ProviderId::new("raw").unwrap()
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
            _context: ProviderContext,
            _events: &'a dyn EventSink,
        ) -> ProviderFuture<'a> {
            panic!("process-control preflight must refuse before execution")
        }
    }

    #[test]
    fn host_registers_explicit_providers_and_creates_suspended_runs() {
        let mut roba = Roba::new();
        roba.register(FakeProvider).unwrap();
        assert_eq!(
            roba.provider_ids()
                .map(ProviderId::as_str)
                .collect::<Vec<_>>(),
            ["fake"]
        );
        let run = roba
            .create_run(RunSpec::suspended(AgentSpec::new(
                ProviderId::new("fake").unwrap(),
            )))
            .unwrap();
        assert_eq!(run.spec().agent.provider.as_str(), "fake");
    }

    #[test]
    fn duplicates_and_unregistered_providers_fail_closed() {
        let mut roba = Roba::new();
        roba.register(FakeProvider).unwrap();
        assert_eq!(
            roba.register(FakeProvider).unwrap_err(),
            RuntimeError::DuplicateProvider(ProviderId::new("fake").unwrap())
        );
        assert_eq!(
            roba.create_run(RunSpec::suspended(AgentSpec::new(ProviderId::codex())))
                .err()
                .unwrap(),
            RuntimeError::ProviderUnavailable(ProviderId::codex())
        );
    }

    #[test]
    fn process_capabilities_require_registration_and_every_grant() {
        let mut roba = Roba::new();
        roba.register(FakeProvider).unwrap();
        let mut spec = RunSpec::suspended(AgentSpec::new(ProviderId::new("fake").unwrap()));
        spec.mission = MissionPolicy::new(
            [ProcessCapabilityId::new("test/process").unwrap()],
            [],
            CompletionPolicy::RootTerminal,
        )
        .unwrap();

        assert_eq!(
            roba.create_run(spec.clone()).err().unwrap(),
            RuntimeError::CapabilityUnavailable(ProcessCapabilityId::new("test/process").unwrap())
        );
        roba.register_capability(FakeProcess).unwrap();
        assert_eq!(
            roba.create_run(spec.clone()).err().unwrap(),
            RuntimeError::MissingCapabilityGrant {
                capability: ProcessCapabilityId::new("test/process").unwrap(),
                grant: AuthorityGrantId::new("test/write").unwrap(),
            }
        );

        spec.mission = MissionPolicy::new(
            [ProcessCapabilityId::new("test/process").unwrap()],
            [
                AuthorityGrantId::new("test/write").unwrap(),
                AuthorityGrantId::new("test/unused").unwrap(),
            ],
            CompletionPolicy::RootTerminal,
        )
        .unwrap();
        assert_eq!(
            roba.create_run(spec.clone()).err().unwrap(),
            RuntimeError::UnusedMissionGrant(AuthorityGrantId::new("test/unused").unwrap())
        );

        spec.mission = MissionPolicy::new(
            [ProcessCapabilityId::new("test/process").unwrap()],
            [AuthorityGrantId::new("test/write").unwrap()],
            CompletionPolicy::RootTerminal,
        )
        .unwrap();
        let resolved = roba.resolve_capabilities(&spec).unwrap();
        assert_eq!(
            resolved.values().next().unwrap().descriptor.actions.len(),
            1
        );
        let mut elevated = spec.clone();
        elevated.mission = MissionPolicy::new(
            [ProcessCapabilityId::new("test/process").unwrap()],
            [
                AuthorityGrantId::new("test/write").unwrap(),
                AuthorityGrantId::new("test/admin").unwrap(),
            ],
            CompletionPolicy::RootTerminal,
        )
        .unwrap();
        assert_eq!(
            roba.resolve_capabilities(&elevated)
                .unwrap()
                .values()
                .next()
                .unwrap()
                .descriptor
                .actions
                .len(),
            2
        );
        let run = roba.create_run(spec).unwrap();
        assert_eq!(
            run.spec()
                .mission
                .capabilities()
                .iter()
                .next()
                .unwrap()
                .as_str(),
            "test/process"
        );
    }

    #[test]
    fn empty_process_policy_preserves_the_minimal_run_path() {
        let mut roba = Roba::new();
        roba.register(FakeProvider).unwrap();
        roba.register_capability(FakeProcess).unwrap();
        let run = roba
            .create_run(RunSpec::suspended(AgentSpec::new(
                ProviderId::new("fake").unwrap(),
            )))
            .unwrap();
        assert!(run.spec().mission.is_empty());
    }

    #[test]
    fn provider_without_private_process_transport_fails_before_run_creation() {
        let mut roba = Roba::new();
        roba.register(RawProvider).unwrap();
        roba.register_capability(FakeProcess).unwrap();
        let mut spec = RunSpec::suspended(AgentSpec::new(ProviderId::new("raw").unwrap()));
        spec.mission = MissionPolicy::new(
            [ProcessCapabilityId::new("test/process").unwrap()],
            [AuthorityGrantId::new("test/write").unwrap()],
            CompletionPolicy::RootTerminal,
        )
        .unwrap();
        assert_eq!(
            roba.create_run(spec).err().unwrap(),
            RuntimeError::ProviderProcessControlUnavailable(ProviderId::new("raw").unwrap())
        );
    }
}
