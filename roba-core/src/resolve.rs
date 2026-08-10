//! Hierarchical, provider-neutral run specification resolution.
//!
//! This is deliberately independent of Roba's legacy profiles and clap
//! values. Persistent policy resolves in one order:
//!
//! `Roba defaults -> selected provider defaults -> named agent -> run overrides`.
//!
//! Scalar values use the last specified value. Instruction and project
//! context lists append in the same order so their provenance remains visible
//! in the resolved [`RunSpec`].

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::run::{
    AgentSpec, ContextSpec, Effort, ExecutionSpec, LimitSpec, PermissionPolicy, Prompt, ProviderId,
    RunSpec, SessionSpec, ToolPolicy,
};

/// A partial persistent policy layer.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigLayer {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<ProviderId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<Effort>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub instructions: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub project_context: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permissions: Option<PermissionPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<ToolPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_turns: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_cost_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_workers: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_worker_depth: Option<u32>,
}

/// Small persistent Roba configuration. Named agents replace profiles,
/// personas, and bundles as the one library concept.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RobaConfig {
    #[serde(default)]
    pub defaults: ConfigLayer,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub providers: BTreeMap<ProviderId, ConfigLayer>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub agents: BTreeMap<String, ConfigLayer>,
}

/// Invocation-only values layered over persistent configuration.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunOverrides {
    #[serde(default)]
    pub policy: ConfigLayer,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub context: Vec<String>,
    #[serde(default)]
    pub session: SessionSpec,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_prompt: Option<Prompt>,
}

impl RobaConfig {
    /// Parse the public hierarchical configuration format from TOML.
    ///
    /// The format is intentionally the serde shape of [`RobaConfig`]:
    /// `[defaults]`, `[providers.NAME]`, and `[agents.NAME]`. Unknown fields
    /// are rejected so a misspelled safety setting cannot be ignored.
    pub fn from_toml(input: &str) -> Result<Self, ConfigParseError> {
        toml::from_str(input).map_err(ConfigParseError)
    }

    /// Resolve a complete, inspectable specification.
    pub fn resolve(
        &self,
        agent_name: Option<&str>,
        overrides: RunOverrides,
    ) -> Result<RunSpec, ResolveError> {
        let agent = match agent_name {
            Some(name) => Some(
                self.agents
                    .get(name)
                    .ok_or_else(|| ResolveError::UnknownAgent(name.to_string()))?,
            ),
            None => None,
        };

        // Provider selection is resolved first so its defaults are applied
        // even when a named agent or run override changes the global default.
        let provider = overrides
            .policy
            .provider
            .clone()
            .or_else(|| agent.and_then(|layer| layer.provider.clone()))
            .or_else(|| self.defaults.provider.clone())
            .ok_or(ResolveError::MissingProvider)?;

        let mut resolved = Resolved::default();
        resolved.apply(&self.defaults);
        if let Some(provider_defaults) = self.providers.get(&provider) {
            resolved.apply(provider_defaults);
        }
        if let Some(agent) = agent {
            resolved.apply(agent);
        }
        resolved.apply(&overrides.policy);
        // The preselected provider is authoritative. A provider default is
        // not allowed to redirect resolution into another provider bucket.
        resolved.provider = Some(provider.clone());

        validate(&resolved)?;

        Ok(RunSpec {
            agent: AgentSpec {
                provider,
                model: resolved.model,
                effort: resolved.effort,
                instructions: resolved.instructions,
            },
            context: ContextSpec {
                project: resolved.project_context,
                run: overrides.context,
            },
            execution: ExecutionSpec {
                permissions: resolved.permissions.unwrap_or_default(),
                tools: resolved.tools.unwrap_or_default(),
                limits: LimitSpec {
                    max_turns: resolved.max_turns,
                    max_cost_usd: resolved.max_cost_usd,
                    timeout_secs: resolved.timeout_secs,
                },
                session: overrides.session,
                workers: crate::run::WorkerPolicy {
                    max_workers: resolved.max_workers.unwrap_or(0),
                    max_depth: resolved.max_worker_depth.unwrap_or(0),
                },
            },
            mission: crate::MissionPolicy::default(),
            initial_prompt: overrides.initial_prompt,
        })
    }
}

/// A public hierarchical Roba configuration could not be decoded.
#[derive(Debug)]
pub struct ConfigParseError(toml::de::Error);

impl fmt::Display for ConfigParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid Roba run config: {}", self.0)
    }
}

impl std::error::Error for ConfigParseError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.0)
    }
}

#[derive(Debug, Default)]
struct Resolved {
    provider: Option<ProviderId>,
    model: Option<String>,
    effort: Option<Effort>,
    instructions: Vec<String>,
    project_context: Vec<String>,
    permissions: Option<PermissionPolicy>,
    tools: Option<ToolPolicy>,
    max_turns: Option<u32>,
    max_cost_usd: Option<f64>,
    timeout_secs: Option<u64>,
    max_workers: Option<u32>,
    max_worker_depth: Option<u32>,
}

impl Resolved {
    fn apply(&mut self, layer: &ConfigLayer) {
        if let Some(provider) = &layer.provider {
            self.provider = Some(provider.clone());
        }
        if let Some(model) = &layer.model {
            self.model = Some(model.clone());
        }
        if let Some(effort) = layer.effort {
            self.effort = Some(effort);
        }
        self.instructions.extend(layer.instructions.iter().cloned());
        self.project_context
            .extend(layer.project_context.iter().cloned());
        if let Some(permissions) = layer.permissions {
            self.permissions = Some(permissions);
        }
        if let Some(tools) = &layer.tools {
            self.tools = Some(tools.clone());
        }
        if let Some(max_turns) = layer.max_turns {
            self.max_turns = Some(max_turns);
        }
        if let Some(max_cost_usd) = layer.max_cost_usd {
            self.max_cost_usd = Some(max_cost_usd);
        }
        if let Some(timeout_secs) = layer.timeout_secs {
            self.timeout_secs = Some(timeout_secs);
        }
        if let Some(max_workers) = layer.max_workers {
            self.max_workers = Some(max_workers);
        }
        if let Some(max_worker_depth) = layer.max_worker_depth {
            self.max_worker_depth = Some(max_worker_depth);
        }
    }
}

fn validate(resolved: &Resolved) -> Result<(), ResolveError> {
    if resolved
        .model
        .as_ref()
        .is_some_and(|model| model.trim().is_empty())
    {
        return Err(ResolveError::EmptyModel);
    }
    if resolved.max_turns == Some(0) {
        return Err(ResolveError::InvalidMaxTurns);
    }
    if resolved.timeout_secs == Some(0) {
        return Err(ResolveError::InvalidTimeout);
    }
    if resolved
        .max_cost_usd
        .is_some_and(|cost| !cost.is_finite() || cost <= 0.0)
    {
        return Err(ResolveError::InvalidMaxCost);
    }
    let worker_policy = crate::run::WorkerPolicy {
        max_workers: resolved.max_workers.unwrap_or(0),
        max_depth: resolved.max_worker_depth.unwrap_or(0),
    };
    if worker_policy.validate().is_err() {
        return Err(ResolveError::InvalidWorkerPolicy);
    }
    Ok(())
}

/// A configuration could not resolve to executable policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveError {
    MissingProvider,
    UnknownAgent(String),
    EmptyModel,
    InvalidMaxTurns,
    InvalidMaxCost,
    InvalidTimeout,
    InvalidWorkerPolicy,
}

impl fmt::Display for ResolveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingProvider => f.write_str("no provider is configured for this run"),
            Self::UnknownAgent(name) => write!(f, "unknown agent {name:?}"),
            Self::EmptyModel => f.write_str("model must not be empty"),
            Self::InvalidMaxTurns => f.write_str("max_turns must be greater than zero"),
            Self::InvalidMaxCost => {
                f.write_str("max_cost_usd must be finite and greater than zero")
            }
            Self::InvalidTimeout => f.write_str("timeout_secs must be greater than zero"),
            Self::InvalidWorkerPolicy => f.write_str(
                "max_workers and max_worker_depth must either both be zero or both be greater than zero",
            ),
        }
    }
}

impl std::error::Error for ResolveError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::run::RunState;

    fn layer(provider: Option<ProviderId>, model: &str, instruction: &str) -> ConfigLayer {
        ConfigLayer {
            provider,
            model: Some(model.to_string()),
            instructions: vec![instruction.to_string()],
            ..ConfigLayer::default()
        }
    }

    #[test]
    fn resolves_the_documented_hierarchy() {
        let mut config = RobaConfig {
            defaults: layer(Some(ProviderId::claude()), "default", "roba"),
            ..RobaConfig::default()
        };
        config.providers.insert(
            ProviderId::codex(),
            layer(None, "provider-model", "provider"),
        );
        config.agents.insert(
            "builder".to_string(),
            layer(Some(ProviderId::codex()), "agent-model", "agent"),
        );
        let overrides = RunOverrides {
            policy: layer(None, "run-model", "override"),
            context: vec!["run context".to_string()],
            initial_prompt: Some(Prompt::new("ship it").unwrap()),
            ..RunOverrides::default()
        };

        let spec = config.resolve(Some("builder"), overrides).unwrap();
        assert_eq!(spec.agent.provider, ProviderId::codex());
        assert_eq!(spec.agent.model.as_deref(), Some("run-model"));
        assert_eq!(
            spec.agent.instructions,
            ["roba", "provider", "agent", "override"]
        );
        assert_eq!(spec.context.run, ["run context"]);
        assert_eq!(spec.initial_state(), RunState::Ready);
    }

    #[test]
    fn run_provider_override_selects_that_providers_defaults() {
        let mut config = RobaConfig {
            defaults: layer(Some(ProviderId::claude()), "claude-default", "roba"),
            ..RobaConfig::default()
        };
        config.providers.insert(
            ProviderId::codex(),
            layer(None, "codex-default", "codex provider"),
        );
        let overrides = RunOverrides {
            policy: ConfigLayer {
                provider: Some(ProviderId::codex()),
                ..ConfigLayer::default()
            },
            ..RunOverrides::default()
        };

        let spec = config.resolve(None, overrides).unwrap();
        assert_eq!(spec.agent.provider, ProviderId::codex());
        assert_eq!(spec.agent.model.as_deref(), Some("codex-default"));
    }

    #[test]
    fn prompt_is_optional_and_unknown_agents_fail_loudly() {
        let config = RobaConfig {
            defaults: ConfigLayer {
                provider: Some(ProviderId::claude()),
                ..ConfigLayer::default()
            },
            ..RobaConfig::default()
        };
        let spec = config.resolve(None, RunOverrides::default()).unwrap();
        assert_eq!(spec.initial_state(), RunState::Suspended);
        assert_eq!(
            config
                .resolve(Some("missing"), RunOverrides::default())
                .unwrap_err(),
            ResolveError::UnknownAgent("missing".to_string())
        );
    }

    #[test]
    fn malformed_limits_fail_during_resolution() {
        let config = RobaConfig {
            defaults: ConfigLayer {
                provider: Some(ProviderId::claude()),
                max_cost_usd: Some(f64::NAN),
                ..ConfigLayer::default()
            },
            ..RobaConfig::default()
        };
        assert_eq!(
            config.resolve(None, RunOverrides::default()).unwrap_err(),
            ResolveError::InvalidMaxCost
        );
    }

    #[test]
    fn worker_bounds_resolve_together_or_fail_closed() {
        let mut config = RobaConfig {
            defaults: ConfigLayer {
                provider: Some(ProviderId::claude()),
                max_workers: Some(3),
                max_worker_depth: Some(2),
                ..ConfigLayer::default()
            },
            ..RobaConfig::default()
        };
        let spec = config.resolve(None, RunOverrides::default()).unwrap();
        assert_eq!(spec.execution.workers.max_workers, 3);
        assert_eq!(spec.execution.workers.max_depth, 2);

        config.defaults.max_worker_depth = None;
        assert_eq!(
            config.resolve(None, RunOverrides::default()).unwrap_err(),
            ResolveError::InvalidWorkerPolicy
        );
    }

    #[test]
    fn public_toml_shape_loads_hierarchy_and_rejects_unknown_fields() {
        let config = RobaConfig::from_toml(
            r#"
[defaults]
provider = "claude"
instructions = ["default"]

[providers.codex]
model = "gpt-5.6"
effort = "high"

[agents.builder]
provider = "codex"
instructions = ["build the requested change"]
permissions = "workspace_write"
max_workers = 3
max_worker_depth = 2
"#,
        )
        .unwrap();

        let spec = config
            .resolve(Some("builder"), RunOverrides::default())
            .unwrap();
        assert_eq!(spec.agent.provider, ProviderId::codex());
        assert_eq!(spec.agent.model.as_deref(), Some("gpt-5.6"));
        assert_eq!(spec.agent.effort, Some(Effort::High));
        assert_eq!(
            spec.agent.instructions,
            ["default", "build the requested change"]
        );
        assert_eq!(spec.execution.permissions, PermissionPolicy::WorkspaceWrite);
        assert_eq!(spec.execution.workers.max_workers, 3);
        assert_eq!(spec.execution.workers.max_depth, 2);

        let error = RobaConfig::from_toml(
            r#"
[defaults]
provider = "codex"
timeuot_secs = 30
"#,
        )
        .unwrap_err();
        assert!(error.to_string().contains("unknown field `timeuot_secs`"));
    }

    #[test]
    fn shipped_run_config_example_stays_on_the_public_parser() {
        let config =
            RobaConfig::from_toml(include_str!("../../examples/run-config/roba.toml")).unwrap();
        let spec = config
            .resolve(Some("builder"), RunOverrides::default())
            .unwrap();

        assert_eq!(spec.agent.provider, ProviderId::codex());
        assert_eq!(spec.execution.permissions, PermissionPolicy::WorkspaceWrite);
        assert_eq!(spec.execution.limits.timeout_secs, Some(600));
        assert_eq!(spec.execution.workers.max_workers, 4);
        assert_eq!(spec.execution.workers.max_depth, 2);
    }
}
