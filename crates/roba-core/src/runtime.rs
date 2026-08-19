//! Provider registry and ergonomic library entry point.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use crate::lifecycle::{Run, RunControlError};
use crate::provider::{
    Provider, ProviderAmbientContextCapabilities, ProviderError, ProviderLaunchContext,
};
use crate::run::{Prompt, ProviderId, RunSpec, TurnRequest};

/// Process-local Roba runtime. It owns provider adapters but no daemon,
/// database, queue, or global session pool.
#[derive(Default)]
pub struct Roba {
    providers: BTreeMap<ProviderId, Arc<dyn Provider>>,
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

    /// True when this host can execute the provider id.
    pub fn contains(&self, id: &ProviderId) -> bool {
        self.providers.contains_key(id)
    }

    /// Provider ids in deterministic order.
    pub fn provider_ids(&self) -> impl Iterator<Item = &ProviderId> {
        self.providers.keys()
    }

    /// Inspect one registered provider's enforceable ambient-context profiles.
    pub fn ambient_context_capabilities(
        &self,
        id: &ProviderId,
    ) -> Option<ProviderAmbientContextCapabilities> {
        self.providers
            .get(id)
            .map(|provider| provider.ambient_context_capabilities())
    }

    /// Validate a suspended run specification against its selected provider
    /// without starting provider work.
    pub fn validate_spec(&self, spec: &RunSpec) -> Result<(), SpecValidationError> {
        let provider = self
            .providers
            .get(&spec.agent.provider)
            .ok_or_else(|| SpecValidationError::ProviderUnavailable(spec.agent.provider.clone()))?;
        let prompt = Prompt::new("provider-neutral configuration preflight")
            .expect("static validation prompt is nonempty");
        let request = TurnRequest {
            spec: spec.clone().with_prompt(prompt.clone()),
            prompt,
        };
        provider
            .validate(&request)
            .map_err(SpecValidationError::Provider)
    }

    /// Construct one bounded run without starting provider work.
    pub fn create_run(&self, spec: RunSpec) -> Result<Run, RuntimeError> {
        self.create_run_with_launch_context(spec, ProviderLaunchContext::default())
    }

    /// Construct one bounded run with transient provider launch material
    /// without starting provider work.
    pub fn create_run_with_launch_context(
        &self,
        spec: RunSpec,
        launch_context: ProviderLaunchContext,
    ) -> Result<Run, RuntimeError> {
        let provider = self
            .providers
            .get(&spec.agent.provider)
            .ok_or_else(|| RuntimeError::ProviderUnavailable(spec.agent.provider.clone()))?;
        Run::new_with_launch_context(spec, Arc::clone(provider), launch_context)
            .map_err(RuntimeError::Run)
    }
}

/// Runtime construction error.
#[derive(Debug, Clone, PartialEq)]
pub enum RuntimeError {
    DuplicateProvider(ProviderId),
    ProviderUnavailable(ProviderId),
    Run(RunControlError),
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateProvider(id) => write!(f, "provider {id} is already registered"),
            Self::ProviderUnavailable(id) => write!(f, "provider {id} is not registered"),
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

/// Provider validation failure found without starting provider work.
#[derive(Debug, Clone, PartialEq)]
pub enum SpecValidationError {
    ProviderUnavailable(ProviderId),
    Provider(ProviderError),
}

impl fmt::Display for SpecValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ProviderUnavailable(id) => write!(formatter, "provider {id} is not registered"),
            Self::Provider(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for SpecValidationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Provider(error) => Some(error),
            Self::ProviderUnavailable(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{EventSink, ProviderCapabilities, ProviderError, ProviderFuture};
    use crate::run::{AgentSpec, RunOutcome, TurnRequest};

    struct FakeProvider;

    impl Provider for FakeProvider {
        fn id(&self) -> ProviderId {
            ProviderId::new("fake").unwrap()
        }

        fn capabilities(&self) -> ProviderCapabilities {
            ProviderCapabilities::default()
        }

        fn validate(&self, request: &TurnRequest) -> Result<(), ProviderError> {
            if request.spec.agent.model.as_deref() == Some("unsupported") {
                return Err(ProviderError::unsupported("model is unsupported"));
            }
            Ok(())
        }

        fn execute<'a>(
            &'a self,
            _request: TurnRequest,
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
    fn validation_runs_without_starting_provider_work() {
        let mut roba = Roba::new();
        roba.register(FakeProvider).unwrap();
        let mut agent = AgentSpec::new(ProviderId::new("fake").unwrap());
        agent.model = Some("unsupported".to_string());
        let error = roba.validate_spec(&RunSpec::suspended(agent)).unwrap_err();
        assert_eq!(
            error,
            SpecValidationError::Provider(ProviderError::unsupported("model is unsupported"))
        );
    }
}
