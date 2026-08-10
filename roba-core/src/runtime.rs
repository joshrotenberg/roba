//! Provider registry and ergonomic library entry point.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use crate::lifecycle::{Run, RunControlError};
use crate::provider::Provider;
use crate::run::{ProviderId, RunSpec};

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

    /// Construct one bounded run without starting provider work.
    pub fn create_run(&self, spec: RunSpec) -> Result<Run, RuntimeError> {
        if !self.providers.contains_key(&spec.agent.provider) {
            return Err(RuntimeError::ProviderUnavailable(
                spec.agent.provider.clone(),
            ));
        }
        Run::with_providers(spec, self.providers.clone()).map_err(RuntimeError::Run)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{
        EventSink, ProviderCapabilities, ProviderContext, ProviderError, ProviderFuture,
    };
    use crate::run::{AgentSpec, RunOutcome, TurnRequest};

    struct FakeProvider;

    impl Provider for FakeProvider {
        fn id(&self) -> ProviderId {
            ProviderId::new("fake").unwrap()
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
}
