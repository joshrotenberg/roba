//! Versioned provider-neutral startup configuration for `roba run` and
//! `roba serve`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use roba_core::{
    AgentSpec, ContextSpec, Effort, ExecutionSpec, LimitSpec, PermissionPolicy, ProviderId,
    RunSpec, SessionHandle, SessionSpec, ToolPolicy,
};
use serde::{Deserialize, Serialize};

use crate::VersionedResult;
use crate::cli::{AgentArgs, ConfigEffectiveArgs, EffortLevel, RunProvider};

const CONFIG_VERSION: u32 = 1;
const PROJECT_CANDIDATES: [&str; 3] = ["roba.toml", ".roba.toml", ".roba/roba.toml"];

/// Fully resolved host configuration used to construct one logical agent.
#[derive(Debug, Clone)]
pub(crate) struct ResolvedStartup {
    pub template: RunSpec,
    pub git_enabled: bool,
    pub git_progress_interval_secs: u64,
    pub effective: EffectiveStartupConfig,
}

/// Safe, provider-neutral effective configuration and field provenance.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffectiveStartupConfig {
    pub version: u32,
    pub sources: Vec<ConfigSource>,
    pub agent: EffectiveAgentConfig,
    pub execution: EffectiveExecutionConfig,
    pub context: EffectiveContextConfig,
    pub extensions: EffectiveExtensionsConfig,
    pub provenance: BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigSource {
    pub kind: ConfigSourceKind,
    pub path: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigSourceKind {
    User,
    Project,
    Explicit,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffectiveAgentConfig {
    pub provider: RunProvider,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<EffortLevel>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub instructions: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffectiveExecutionConfig {
    pub permissions: PermissionPolicy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_turns: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_cost_usd: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
    /// Whether the CLI supplied a provider-private resume seed. The id is
    /// deliberately absent from this inspectable configuration.
    pub resume_seeded: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffectiveContextConfig {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub project: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub run: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffectiveExtensionsConfig {
    pub git: EffectiveGitConfig,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffectiveGitConfig {
    pub enabled: bool,
    pub progress_interval_secs: u64,
}

impl Default for EffectiveGitConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            progress_interval_secs: 5,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct StartupFile {
    version: u32,
    #[serde(default)]
    agent: FileAgentConfig,
    #[serde(default)]
    execution: FileExecutionConfig,
    #[serde(default)]
    context: FileContextConfig,
    #[serde(default)]
    extensions: FileExtensionsConfig,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileAgentConfig {
    provider: Option<RunProvider>,
    model: Option<String>,
    effort: Option<EffortLevel>,
    #[serde(default)]
    instructions: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileExecutionConfig {
    permissions: Option<PermissionPolicy>,
    max_turns: Option<u32>,
    max_cost_usd: Option<f64>,
    timeout_secs: Option<u64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileContextConfig {
    #[serde(default)]
    project: Vec<String>,
    #[serde(default)]
    run: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileExtensionsConfig {
    #[serde(default)]
    git: FileGitConfig,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileGitConfig {
    enabled: Option<bool>,
    progress_interval_secs: Option<u64>,
}

#[derive(Debug)]
struct Layer {
    source: ConfigSource,
    config: StartupFile,
}

/// Resolve the effective cwd's startup stack and explicit CLI overrides.
pub(crate) fn resolve(args: &AgentArgs) -> Result<ResolvedStartup> {
    let cwd = std::env::current_dir().context("resolving provider-neutral config cwd")?;
    resolve_from(args, &cwd, user_config_path())
}

/// Print the provider-neutral effective config without starting a provider.
pub fn run_effective(args: ConfigEffectiveArgs) -> Result<()> {
    let resolved = resolve(&args.agent)?;
    let _validated_host = crate::bounded::build_agent_from_template(
        resolved.template.clone(),
        resolved.git_enabled,
        resolved.git_progress_interval_secs,
    )?;
    let effective = resolved.effective;
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&VersionedResult::new(effective))?
        );
    } else {
        print!("{}", toml::to_string_pretty(&effective)?);
    }
    Ok(())
}

fn resolve_from(
    args: &AgentArgs,
    cwd: &Path,
    user_config: Option<PathBuf>,
) -> Result<ResolvedStartup> {
    let layers = if args.no_config {
        Vec::new()
    } else if let Some(path) = &args.config {
        vec![load_required_layer(path, ConfigSourceKind::Explicit)?]
    } else {
        discover_layers(cwd, user_config)?
    };

    let mut effective = EffectiveStartupConfig {
        version: CONFIG_VERSION,
        sources: Vec::new(),
        agent: EffectiveAgentConfig {
            provider: RunProvider::Claude,
            model: None,
            effort: None,
            instructions: Vec::new(),
        },
        execution: EffectiveExecutionConfig {
            permissions: PermissionPolicy::ReadOnly,
            max_turns: None,
            max_cost_usd: None,
            timeout_secs: None,
            resume_seeded: false,
        },
        context: EffectiveContextConfig::default(),
        extensions: EffectiveExtensionsConfig::default(),
        provenance: BTreeMap::from([
            ("agent.provider".to_string(), vec!["default".to_string()]),
            (
                "execution.permissions".to_string(),
                vec!["default".to_string()],
            ),
            (
                "extensions.git.enabled".to_string(),
                vec!["default".to_string()],
            ),
            (
                "extensions.git.progress_interval_secs".to_string(),
                vec!["default".to_string()],
            ),
        ]),
    };
    for layer in layers {
        merge_layer(&mut effective, layer);
    }
    merge_cli(&mut effective, args);
    validate_effective(&effective, args.resume.as_deref())?;

    let provider = match effective.agent.provider {
        RunProvider::Claude => ProviderId::claude(),
        RunProvider::Codex => ProviderId::codex(),
    };
    let session = match &args.resume {
        Some(id) => SessionSpec::Resume {
            session: SessionHandle {
                provider: provider.clone(),
                id: id.clone(),
            },
        },
        None => SessionSpec::Fresh,
    };
    let mut agent = AgentSpec::new(provider);
    agent.model.clone_from(&effective.agent.model);
    agent.effort = effective.agent.effort.map(map_effort);
    agent.instructions.clone_from(&effective.agent.instructions);

    let template = RunSpec {
        agent,
        context: ContextSpec {
            project: effective.context.project.clone(),
            run: effective.context.run.clone(),
        },
        execution: ExecutionSpec {
            permissions: effective.execution.permissions,
            tools: ToolPolicy::default(),
            limits: LimitSpec {
                max_turns: effective.execution.max_turns,
                max_cost_usd: effective.execution.max_cost_usd,
                timeout_secs: effective.execution.timeout_secs,
            },
            session,
        },
        initial_prompt: None,
    };
    let git_enabled = effective.extensions.git.enabled;
    let git_progress_interval_secs = effective.extensions.git.progress_interval_secs;

    Ok(ResolvedStartup {
        template,
        git_enabled,
        git_progress_interval_secs,
        effective,
    })
}

fn discover_layers(cwd: &Path, user_config: Option<PathBuf>) -> Result<Vec<Layer>> {
    let mut layers = Vec::new();
    if let Some(path) = user_config
        && path.is_file()
        && let Some(layer) = load_discovered_layer(&path, ConfigSourceKind::User)?
    {
        layers.push(layer);
    }

    for directory in project_directories(cwd) {
        let mut matches = Vec::new();
        for relative in PROJECT_CANDIDATES {
            let path = directory.join(relative);
            if path.is_file()
                && let Some(layer) = load_discovered_layer(&path, ConfigSourceKind::Project)?
            {
                matches.push(layer);
            }
        }
        if matches.len() > 1 {
            let paths = matches
                .iter()
                .map(|layer| layer.source.path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ");
            bail!(
                "ambiguous provider-neutral config in {}: {paths}",
                directory.display()
            );
        }
        layers.extend(matches);
    }
    Ok(layers)
}

fn project_directories(cwd: &Path) -> Vec<PathBuf> {
    let mut directories = Vec::new();
    let mut current = cwd.to_path_buf();
    loop {
        directories.push(current.clone());
        if current.join(".git").exists() {
            break;
        }
        let Some(parent) = current.parent() else {
            break;
        };
        current = parent.to_path_buf();
    }
    directories.reverse();
    directories
}

fn user_config_path() -> Option<PathBuf> {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME")
        && !xdg.is_empty()
    {
        return Some(PathBuf::from(xdg).join("roba").join("roba.toml"));
    }
    home_dir().map(|home| home.join(".config/roba/roba.toml"))
}

fn home_dir() -> Option<PathBuf> {
    if let Ok(home) = std::env::var("HOME")
        && !home.is_empty()
    {
        return Some(PathBuf::from(home));
    }
    if let Ok(home) = std::env::var("USERPROFILE")
        && !home.is_empty()
    {
        return Some(PathBuf::from(home));
    }
    None
}

fn load_required_layer(path: &Path, kind: ConfigSourceKind) -> Result<Layer> {
    load_layer(path, kind)?.ok_or_else(|| {
        anyhow::anyhow!(
            "provider-neutral config {} must declare `version = 1`",
            path.display()
        )
    })
}

fn load_discovered_layer(path: &Path, kind: ConfigSourceKind) -> Result<Option<Layer>> {
    load_layer(path, kind)
}

fn load_layer(path: &Path, kind: ConfigSourceKind) -> Result<Option<Layer>> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("reading provider-neutral config at {}", path.display()))?;
    let value: toml::Value = toml::from_str(&content).map_err(|error| {
        anyhow::anyhow!(
            "parsing provider-neutral config at {}: {error}",
            path.display()
        )
    })?;
    if value.get("version").is_none() {
        return Ok(None);
    }
    let config: StartupFile = toml::from_str(&content).map_err(|error| {
        anyhow::anyhow!(
            "parsing provider-neutral config at {}: {error}",
            path.display()
        )
    })?;
    if config.version != CONFIG_VERSION {
        bail!(
            "unsupported provider-neutral config version {} at {}; expected {CONFIG_VERSION}",
            config.version,
            path.display()
        );
    }
    Ok(Some(Layer {
        source: ConfigSource {
            kind,
            path: path.to_path_buf(),
        },
        config,
    }))
}

fn merge_layer(effective: &mut EffectiveStartupConfig, layer: Layer) {
    let label = layer.source.path.display().to_string();
    let config = layer.config;
    effective.sources.push(layer.source);

    if let Some(value) = config.agent.provider {
        effective.agent.provider = value;
        replace_source(effective, "agent.provider", &label);
    }
    if let Some(value) = config.agent.model {
        effective.agent.model = Some(value);
        replace_source(effective, "agent.model", &label);
    }
    if let Some(value) = config.agent.effort {
        effective.agent.effort = Some(value);
        replace_source(effective, "agent.effort", &label);
    }
    if !config.agent.instructions.is_empty() {
        effective
            .agent
            .instructions
            .extend(config.agent.instructions);
        append_source(effective, "agent.instructions", &label);
    }
    if let Some(value) = config.execution.permissions {
        effective.execution.permissions = value;
        replace_source(effective, "execution.permissions", &label);
    }
    if let Some(value) = config.execution.max_turns {
        effective.execution.max_turns = Some(value);
        replace_source(effective, "execution.max_turns", &label);
    }
    if let Some(value) = config.execution.max_cost_usd {
        effective.execution.max_cost_usd = Some(value);
        replace_source(effective, "execution.max_cost_usd", &label);
    }
    if let Some(value) = config.execution.timeout_secs {
        effective.execution.timeout_secs = Some(value);
        replace_source(effective, "execution.timeout_secs", &label);
    }
    if !config.context.project.is_empty() {
        effective.context.project.extend(config.context.project);
        append_source(effective, "context.project", &label);
    }
    if !config.context.run.is_empty() {
        effective.context.run.extend(config.context.run);
        append_source(effective, "context.run", &label);
    }
    if let Some(value) = config.extensions.git.enabled {
        effective.extensions.git.enabled = value;
        replace_source(effective, "extensions.git.enabled", &label);
    }
    if let Some(value) = config.extensions.git.progress_interval_secs {
        effective.extensions.git.progress_interval_secs = value;
        replace_source(effective, "extensions.git.progress_interval_secs", &label);
    }
}

fn merge_cli(effective: &mut EffectiveStartupConfig, args: &AgentArgs) {
    if let Some(value) = args.provider {
        effective.agent.provider = value;
        replace_source(effective, "agent.provider", "cli");
    }
    if let Some(value) = &args.model {
        effective.agent.model = Some(value.clone());
        replace_source(effective, "agent.model", "cli");
    }
    if let Some(value) = args.effort {
        effective.agent.effort = Some(value);
        replace_source(effective, "agent.effort", "cli");
    }
    if !args.instructions.is_empty() {
        effective
            .agent
            .instructions
            .extend(args.instructions.iter().cloned());
        append_source(effective, "agent.instructions", "cli");
    }
    if !args.context.is_empty() {
        effective.context.run.extend(args.context.iter().cloned());
        append_source(effective, "context.run", "cli");
    }
    if args.read_only {
        effective.execution.permissions = PermissionPolicy::ReadOnly;
        replace_source(effective, "execution.permissions", "cli");
    } else if args.full_auto {
        effective.execution.permissions = PermissionPolicy::FullAuto;
        replace_source(effective, "execution.permissions", "cli");
    } else if args.writable {
        effective.execution.permissions = PermissionPolicy::WorkspaceWrite;
        replace_source(effective, "execution.permissions", "cli");
    }
    if args.git {
        effective.extensions.git.enabled = true;
        replace_source(effective, "extensions.git.enabled", "cli");
    } else if args.no_git {
        effective.extensions.git.enabled = false;
        replace_source(effective, "extensions.git.enabled", "cli");
    }
    if let Some(value) = args.max_turns {
        effective.execution.max_turns = Some(value);
        replace_source(effective, "execution.max_turns", "cli");
    }
    if let Some(value) = args.max_cost_usd {
        effective.execution.max_cost_usd = Some(value);
        replace_source(effective, "execution.max_cost_usd", "cli");
    }
    if let Some(value) = args.timeout {
        effective.execution.timeout_secs = Some(value);
        replace_source(effective, "execution.timeout_secs", "cli");
    }
    if args.resume.is_some() {
        effective.execution.resume_seeded = true;
        replace_source(effective, "execution.resume_seeded", "cli");
    }
}

fn replace_source(effective: &mut EffectiveStartupConfig, field: &str, source: &str) {
    effective
        .provenance
        .insert(field.to_string(), vec![source.to_string()]);
}

fn append_source(effective: &mut EffectiveStartupConfig, field: &str, source: &str) {
    let sources = effective.provenance.entry(field.to_string()).or_default();
    if !sources.iter().any(|existing| existing == source) {
        sources.push(source.to_string());
    }
}

fn validate_effective(effective: &EffectiveStartupConfig, resume: Option<&str>) -> Result<()> {
    if effective
        .agent
        .model
        .as_ref()
        .is_some_and(|model| model.trim().is_empty())
    {
        bail!("agent.model must not be empty");
    }
    if effective
        .agent
        .instructions
        .iter()
        .any(|value| value.trim().is_empty())
    {
        bail!("agent.instructions must not contain empty values");
    }
    if effective
        .context
        .project
        .iter()
        .chain(&effective.context.run)
        .any(|value| value.trim().is_empty())
    {
        bail!("context values must not be empty");
    }
    if effective.execution.max_turns == Some(0) {
        bail!("execution.max_turns must be greater than zero");
    }
    if effective
        .execution
        .max_cost_usd
        .is_some_and(|value| !value.is_finite() || value <= 0.0)
    {
        bail!("execution.max_cost_usd must be finite and greater than zero");
    }
    if effective.execution.timeout_secs == Some(0) {
        bail!("execution.timeout_secs must be greater than zero");
    }
    if resume.is_some_and(|value| value.trim().is_empty()) {
        bail!("--resume must not be empty");
    }
    Ok(())
}

fn map_effort(effort: EffortLevel) -> Effort {
    match effort {
        EffortLevel::Low => Effort::Low,
        EffortLevel::Medium => Effort::Medium,
        EffortLevel::High => Effort::High,
        EffortLevel::Xhigh => Effort::XHigh,
        EffortLevel::Max => Effort::Max,
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser;
    use tempfile::TempDir;

    use super::*;
    use crate::cli::{Cli, SubCommand};

    fn args(values: &[&str]) -> AgentArgs {
        let cli = Cli::try_parse_from(["roba", "serve"].into_iter().chain(values.iter().copied()))
            .unwrap();
        match cli.command.unwrap() {
            SubCommand::Serve(args) => args.agent,
            other => panic!("expected serve, got {other:?}"),
        }
    }

    fn write(path: &Path, content: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    #[test]
    fn versioned_layers_merge_farthest_first_and_cli_wins() {
        let temp = TempDir::new().unwrap();
        let repo = temp.path().join("repo");
        let nested = repo.join("nested");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::create_dir_all(&nested).unwrap();
        let user = temp.path().join("user/roba/roba.toml");
        write(
            &user,
            "version = 1\n[agent]\nprovider = 'codex'\ninstructions = ['user']\n",
        );
        write(
            &repo.join(".roba.toml"),
            "version = 1\n[agent]\nmodel = 'repo'\ninstructions = ['repo']\n[execution]\npermissions = 'workspace_write'\n[context]\nproject = ['project']\n[extensions.git]\nenabled = true\nprogress_interval_secs = 9\n",
        );
        write(
            &nested.join(".roba/roba.toml"),
            "version = 1\n[agent]\neffort = 'medium'\n[context]\nrun = ['nested']\n",
        );

        let resolved = resolve_from(
            &args(&["--model", "cli", "--read-only", "--no-git"]),
            &nested,
            Some(user),
        )
        .unwrap();
        assert_eq!(resolved.template.agent.provider, ProviderId::codex());
        assert_eq!(resolved.template.agent.model.as_deref(), Some("cli"));
        assert_eq!(
            resolved.template.agent.instructions,
            ["user".to_string(), "repo".to_string()]
        );
        assert_eq!(resolved.template.context.project, ["project"]);
        assert_eq!(resolved.template.context.run, ["nested"]);
        assert_eq!(
            resolved.template.execution.permissions,
            PermissionPolicy::ReadOnly
        );
        assert!(!resolved.git_enabled);
        assert_eq!(resolved.git_progress_interval_secs, 9);
        assert_eq!(resolved.effective.sources.len(), 3);
        assert_eq!(
            resolved.effective.provenance["agent.model"],
            ["cli".to_string()]
        );
    }

    #[test]
    fn unversioned_files_are_ignored_and_explicit_files_require_version() {
        let temp = TempDir::new().unwrap();
        write(&temp.path().join("roba.toml"), "model = 'unversioned'\n");
        let resolved = resolve_from(&args(&[]), temp.path(), None).unwrap();
        assert_eq!(resolved.template.agent.provider, ProviderId::claude());
        assert!(resolved.effective.sources.is_empty());

        let explicit = args(&["--config", temp.path().join("roba.toml").to_str().unwrap()]);
        let error = resolve_from(&explicit, temp.path(), None).unwrap_err();
        assert!(format!("{error:#}").contains("must declare `version = 1`"));
    }

    #[test]
    fn sibling_configs_unknown_fields_and_versions_fail_closed() {
        let temp = TempDir::new().unwrap();
        write(&temp.path().join("roba.toml"), "version = 1\n");
        write(&temp.path().join(".roba.toml"), "version = 1\n");
        let error = resolve_from(&args(&[]), temp.path(), None).unwrap_err();
        assert!(error.to_string().contains("ambiguous"));

        std::fs::remove_file(temp.path().join(".roba.toml")).unwrap();
        write(
            &temp.path().join("roba.toml"),
            "version = 1\n[agent]\nproivder = 'codex'\n",
        );
        let error = resolve_from(&args(&[]), temp.path(), None).unwrap_err();
        assert!(format!("{error:#}").contains("unknown field"));

        write(&temp.path().join("roba.toml"), "version = 2\n");
        let error = resolve_from(&args(&[]), temp.path(), None).unwrap_err();
        assert!(error.to_string().contains("unsupported"));
    }

    #[test]
    fn project_discovery_stops_at_the_git_root() {
        let temp = TempDir::new().unwrap();
        write(
            &temp.path().join("roba.toml"),
            "version = 1\n[agent]\nprovider = 'codex'\n",
        );
        let repo = temp.path().join("repo");
        let nested = repo.join("nested");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::create_dir_all(&nested).unwrap();

        let resolved = resolve_from(&args(&[]), &nested, None).unwrap();
        assert_eq!(resolved.template.agent.provider, ProviderId::claude());
        assert!(resolved.effective.sources.is_empty());
    }

    #[test]
    fn no_config_skips_discovery_and_inspection_redacts_resume_id() {
        let temp = TempDir::new().unwrap();
        write(
            &temp.path().join("roba.toml"),
            "version = 1\n[agent]\nprovider = 'codex'\n",
        );
        let resolved = resolve_from(
            &args(&["--no-config", "--resume", "secret-thread"]),
            temp.path(),
            None,
        )
        .unwrap();
        assert_eq!(resolved.template.agent.provider, ProviderId::claude());
        assert!(resolved.effective.execution.resume_seeded);
        let serialized = serde_json::to_string(&resolved.effective).unwrap();
        assert!(!serialized.contains("secret-thread"));
    }

    #[test]
    fn tracked_startup_sample_is_the_real_strict_schema() {
        let config: StartupFile = toml::from_str(include_str!("../roba-startup.sample.toml"))
            .expect("tracked provider-neutral startup sample must parse");
        assert_eq!(config.version, CONFIG_VERSION);
        assert_eq!(config.agent.provider, Some(RunProvider::Codex));
        assert_eq!(config.extensions.git.enabled, Some(true));
        assert_eq!(config.extensions.git.progress_interval_secs, Some(5));
    }

    #[test]
    fn repository_self_config_is_a_valid_conservative_startup_config() {
        let config: StartupFile = toml::from_str(include_str!("../roba.toml"))
            .expect("Roba's checked-in self configuration must parse");
        assert_eq!(config.version, CONFIG_VERSION);
        assert_eq!(config.agent.provider, Some(RunProvider::Codex));
        assert_eq!(
            config.execution.permissions,
            Some(PermissionPolicy::ReadOnly)
        );
        assert_eq!(config.extensions.git.enabled, Some(true));
        assert_eq!(config.extensions.git.progress_interval_secs, Some(5));
    }
}
