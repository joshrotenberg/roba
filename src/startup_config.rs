//! Versioned provider-neutral startup configuration for `roba run` and
//! `roba serve`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use roba_context::{
    CatalogDefinition, CatalogEntry, CatalogFingerprint, CatalogManifest, CatalogOrigin,
    CatalogOriginKind, CatalogSelection, CatalogSelectionSpec, CatalogSource, ContextCatalog,
    PromptArgumentDefinition, builtin_definitions,
};
use roba_core::{
    AgentSpec, ContextSpec, Effort, ExecutionSpec, LimitSpec, PermissionPolicy, ProviderId,
    RunSpec, SessionHandle, SessionSpec, ToolPolicy,
};
use roba_mcp::{AmbientContextPolicy, ContextDiagnostic, SessionMode, SessionPolicy};
use serde::{Deserialize, Serialize};

use crate::VersionedResult;
use crate::cli::{
    AgentArgs, AmbientContextMode, ConfigEffectiveArgs, EffortLevel, RunProvider, SessionModeArg,
};

const CONFIG_VERSION: u32 = 1;
const PROJECT_CANDIDATES: [&str; 3] = ["roba.toml", ".roba.toml", ".roba/roba.toml"];

/// Fully resolved host configuration used to construct one logical agent.
#[derive(Debug, Clone)]
pub(crate) struct ResolvedStartup {
    pub template: RunSpec,
    pub catalog: ContextCatalog,
    pub catalog_selection: Option<CatalogSelection>,
    pub ambient_context_policy: AmbientContextPolicy,
    pub session_policy: SessionPolicy,
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
    pub session: EffectiveSessionConfig,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffectiveSessionConfig {
    pub mode: SessionModeArg,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffectiveContextConfig {
    pub ambient_policy: AmbientContextMode,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<ContextDiagnostic>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub project: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub run: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub prompts: Vec<String>,
    pub builtins: EffectiveCatalogBuiltinsConfig,
    pub catalog: CatalogManifest,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selection: Option<EffectiveCatalogSelection>,
}

impl Default for EffectiveContextConfig {
    fn default() -> Self {
        let catalog = ContextCatalog::builtins();
        Self {
            ambient_policy: AmbientContextMode::Ambient,
            diagnostics: Vec::new(),
            project: Vec::new(),
            run: Vec::new(),
            agent: None,
            skills: Vec::new(),
            prompts: Vec::new(),
            builtins: EffectiveCatalogBuiltinsConfig::default(),
            catalog: catalog.manifest().clone(),
            selection: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffectiveCatalogBuiltinsConfig {
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub disable: Vec<String>,
}

impl Default for EffectiveCatalogBuiltinsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            disable: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffectiveCatalogSelection {
    pub agent: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub prompts: Vec<String>,
    pub fingerprint: CatalogFingerprint,
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
    session: FileSessionConfig,
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
struct FileSessionConfig {
    mode: Option<SessionModeArg>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileContextConfig {
    ambient_policy: Option<AmbientContextMode>,
    #[serde(default)]
    project: Vec<String>,
    #[serde(default)]
    run: Vec<String>,
    agent: Option<String>,
    #[serde(default)]
    skills: Vec<String>,
    #[serde(default)]
    prompts: Vec<String>,
    #[serde(default)]
    builtins: FileCatalogBuiltinsConfig,
    #[serde(default)]
    definitions: Vec<FileCatalogDefinition>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileCatalogBuiltinsConfig {
    enabled: Option<bool>,
    #[serde(default)]
    disable: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum FileCatalogDefinition {
    Agent {
        id: String,
        description: String,
        inline: Option<String>,
        path: Option<PathBuf>,
        #[serde(default)]
        default_skills: Vec<String>,
    },
    Skill {
        id: String,
        description: String,
        inline: Option<String>,
        path: Option<PathBuf>,
    },
    Prompt {
        id: String,
        description: String,
        inline: Option<String>,
        path: Option<PathBuf>,
        #[serde(default)]
        requires: Vec<String>,
        #[serde(default)]
        arguments: Vec<PromptArgumentDefinition>,
    },
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
    definitions: Vec<CatalogDefinition>,
}

#[derive(Debug)]
struct CatalogDefinitionLayer {
    source: ConfigSource,
    definitions: Vec<CatalogDefinition>,
}

/// Resolve the effective cwd's startup stack and explicit CLI overrides.
pub(crate) fn resolve(args: &AgentArgs) -> Result<ResolvedStartup> {
    let cwd = std::env::current_dir().context("resolving provider-neutral config cwd")?;
    resolve_from(args, &cwd, user_config_path())
}

/// Print the provider-neutral effective config without starting a provider.
pub fn run_effective(args: ConfigEffectiveArgs) -> Result<()> {
    let resolved = resolve(&args.agent)?;
    let validated_host = crate::bounded::build_agent_from_template(
        resolved.template.clone(),
        resolved.catalog.clone(),
        resolved.catalog_selection.clone(),
        resolved.ambient_context_policy,
        resolved.session_policy,
        resolved.git_enabled,
        resolved.git_progress_interval_secs,
    )?;
    let mut effective = resolved.effective;
    effective.context.diagnostics = validated_host.context_diagnostics().to_vec();
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

    resolve_layers(args, layers)
}

fn resolve_layers(args: &AgentArgs, layers: Vec<Layer>) -> Result<ResolvedStartup> {
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
        session: EffectiveSessionConfig {
            mode: SessionModeArg::Sticky,
        },
        context: EffectiveContextConfig::default(),
        extensions: EffectiveExtensionsConfig::default(),
        provenance: BTreeMap::from([
            ("agent.provider".to_string(), vec!["default".to_string()]),
            (
                "execution.permissions".to_string(),
                vec!["default".to_string()],
            ),
            ("session.mode".to_string(), vec!["default".to_string()]),
            (
                "context.builtins.enabled".to_string(),
                vec!["default".to_string()],
            ),
            (
                "context.ambient_policy".to_string(),
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
    let mut catalog_layers = Vec::new();
    for layer in layers {
        merge_layer(&mut effective, &mut catalog_layers, layer);
    }
    merge_cli(&mut effective, args);
    validate_effective(&effective, args.resume.as_deref())?;
    let (catalog, catalog_selection) = resolve_catalog(&effective.context, catalog_layers)?;
    effective.context.catalog = catalog.manifest().clone();
    effective.context.selection = catalog_selection.as_ref().map(effective_catalog_selection);

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
    let ambient_context_policy = map_ambient_context(effective.context.ambient_policy);
    let session_policy = SessionPolicy {
        mode: map_session_mode(effective.session.mode),
    };

    Ok(ResolvedStartup {
        template,
        catalog,
        catalog_selection,
        ambient_context_policy,
        session_policy,
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
    parse_layer(path, kind, &content)
}

fn parse_layer(path: &Path, kind: ConfigSourceKind, content: &str) -> Result<Option<Layer>> {
    let value: toml::Value = toml::from_str(content).map_err(|error| {
        anyhow::anyhow!(
            "parsing provider-neutral config at {}: {error}",
            path.display()
        )
    })?;
    if value.get("version").is_none() {
        return Ok(None);
    }
    let mut config: StartupFile = toml::from_str(content).map_err(|error| {
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
    let definitions = std::mem::take(&mut config.context.definitions)
        .into_iter()
        .map(FileCatalogDefinition::resolve)
        .collect::<Result<Vec<_>>>()
        .with_context(|| format!("resolving managed context in {}", path.display()))?;
    Ok(Some(Layer {
        source: ConfigSource {
            kind,
            path: path.to_path_buf(),
        },
        config,
        definitions,
    }))
}

/// Validate a generated document through the same strict schema, catalog,
/// and effective-config resolver used by `run` and `serve` without reading or
/// installing the target path.
pub(crate) fn validate_generated_document(document: &str, target: &Path) -> Result<()> {
    let layer = parse_layer(target, ConfigSourceKind::Explicit, document)?.ok_or_else(|| {
        anyhow::anyhow!(
            "generated provider-neutral config must declare `version = {CONFIG_VERSION}`"
        )
    })?;
    resolve_layers(&AgentArgs::default(), vec![layer])?;
    Ok(())
}

/// Validate a standalone generated document through provider and extension
/// host construction without admitting work or starting a provider process.
pub(crate) fn validate_generated_host(document: &str, target: &Path) -> Result<()> {
    let layer = parse_layer(target, ConfigSourceKind::Explicit, document)?.ok_or_else(|| {
        anyhow::anyhow!(
            "generated provider-neutral config must declare `version = {CONFIG_VERSION}`"
        )
    })?;
    let resolved = resolve_layers(&AgentArgs::default(), vec![layer])?;
    crate::bounded::build_agent_from_template(
        resolved.template,
        resolved.catalog,
        resolved.catalog_selection,
        resolved.ambient_context_policy,
        resolved.session_policy,
        resolved.git_enabled,
        resolved.git_progress_interval_secs,
    )?;
    Ok(())
}

/// Validate a generated project layer against the startup stack it will join.
pub(crate) fn validate_generated_install(document: &str, target: &Path, cwd: &Path) -> Result<()> {
    let generated = parse_layer(target, ConfigSourceKind::Project, document)?.ok_or_else(|| {
        anyhow::anyhow!(
            "generated provider-neutral config must declare `version = {CONFIG_VERSION}`"
        )
    })?;
    let mut layers = discover_layers(cwd, user_config_path())?;
    layers.push(generated);
    resolve_layers(&AgentArgs::default(), layers)?;
    Ok(())
}

fn merge_layer(
    effective: &mut EffectiveStartupConfig,
    catalog_layers: &mut Vec<CatalogDefinitionLayer>,
    layer: Layer,
) {
    let label = layer.source.path.display().to_string();
    let source = layer.source.clone();
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
    if let Some(value) = config.session.mode {
        effective.session.mode = value;
        replace_source(effective, "session.mode", &label);
    }
    if !config.context.project.is_empty() {
        effective.context.project.extend(config.context.project);
        append_source(effective, "context.project", &label);
    }
    if let Some(value) = config.context.ambient_policy {
        effective.context.ambient_policy = value;
        replace_source(effective, "context.ambient_policy", &label);
    }
    if !config.context.run.is_empty() {
        effective.context.run.extend(config.context.run);
        append_source(effective, "context.run", &label);
    }
    if let Some(value) = config.context.agent {
        effective.context.agent = Some(value);
        replace_source(effective, "context.agent", &label);
    }
    if !config.context.skills.is_empty() {
        effective.context.skills.extend(config.context.skills);
        append_source(effective, "context.skills", &label);
    }
    if !config.context.prompts.is_empty() {
        effective.context.prompts.extend(config.context.prompts);
        append_source(effective, "context.prompts", &label);
    }
    if let Some(value) = config.context.builtins.enabled {
        effective.context.builtins.enabled = value;
        replace_source(effective, "context.builtins.enabled", &label);
    }
    if !config.context.builtins.disable.is_empty() {
        effective
            .context
            .builtins
            .disable
            .extend(config.context.builtins.disable);
        append_source(effective, "context.builtins.disable", &label);
    }
    if !layer.definitions.is_empty() {
        append_source(effective, "context.definitions", &label);
        catalog_layers.push(CatalogDefinitionLayer {
            source,
            definitions: layer.definitions,
        });
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
    if let Some(value) = args.ambient_context {
        effective.context.ambient_policy = value;
        replace_source(effective, "context.ambient_policy", "cli");
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
    if let Some(value) = args.session_mode {
        effective.session.mode = value;
        replace_source(effective, "session.mode", "cli");
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
    if effective.context.agent.is_none()
        && (!effective.context.skills.is_empty() || !effective.context.prompts.is_empty())
    {
        bail!("context.skills and context.prompts require context.agent");
    }
    reject_duplicate_values("context.skills", &effective.context.skills)?;
    reject_duplicate_values("context.prompts", &effective.context.prompts)?;
    reject_duplicate_values(
        "context.builtins.disable",
        &effective.context.builtins.disable,
    )?;
    if !effective.context.builtins.enabled && !effective.context.builtins.disable.is_empty() {
        bail!("context.builtins.disable cannot be used when built-ins are disabled");
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
    if resume.is_some() && effective.session.mode == SessionModeArg::Fresh {
        bail!("session.mode = fresh cannot be combined with --resume");
    }
    Ok(())
}

fn reject_duplicate_values(field: &str, values: &[String]) -> Result<()> {
    let mut seen = BTreeSet::new();
    for value in values {
        if !seen.insert(value) {
            bail!("{field} contains duplicate id {value}");
        }
    }
    Ok(())
}

fn resolve_catalog(
    config: &EffectiveContextConfig,
    layers: Vec<CatalogDefinitionLayer>,
) -> Result<(ContextCatalog, Option<CatalogSelection>)> {
    let mut builder = ContextCatalog::builder();
    if config.builtins.enabled {
        let disabled = config
            .builtins
            .disable
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        let definitions = builtin_definitions();
        let known = definitions
            .iter()
            .map(CatalogDefinition::id)
            .collect::<BTreeSet<_>>();
        if let Some(id) = disabled.iter().find(|id| !known.contains(**id)) {
            bail!("context.builtins.disable contains unknown built-in id {id}");
        }
        let origin = CatalogOrigin::new(CatalogOriginKind::BuiltIn, "roba built-ins");
        for definition in definitions {
            if !disabled.contains(definition.id()) {
                builder
                    .add(origin.clone(), ".", definition)
                    .context("loading shipped managed context")?;
            }
        }
    }

    for layer in layers {
        let label = layer.source.path.display().to_string();
        let origin =
            CatalogOrigin::new(catalog_origin_kind(layer.source.kind), &label).with_locator(&label);
        let base_directory = layer.source.path.parent().unwrap_or_else(|| Path::new("."));
        for definition in layer.definitions {
            builder
                .add(origin.clone(), base_directory, definition)
                .with_context(|| format!("loading managed context from {label}"))?;
        }
    }

    let catalog = builder
        .build()
        .context("validating managed context catalog")?;
    let selection = config
        .agent
        .as_ref()
        .map(|agent| {
            catalog.select(&CatalogSelectionSpec {
                agent: agent.clone(),
                skills: config.skills.clone(),
                prompts: config.prompts.clone(),
            })
        })
        .transpose()
        .context("resolving managed context selection")?;
    Ok((catalog, selection))
}

fn catalog_origin_kind(kind: ConfigSourceKind) -> CatalogOriginKind {
    match kind {
        ConfigSourceKind::User => CatalogOriginKind::User,
        ConfigSourceKind::Project => CatalogOriginKind::Project,
        ConfigSourceKind::Explicit => CatalogOriginKind::Explicit,
    }
}

fn effective_catalog_selection(selection: &CatalogSelection) -> EffectiveCatalogSelection {
    EffectiveCatalogSelection {
        agent: selection.agent.id().to_owned(),
        skills: selection
            .skills
            .iter()
            .map(CatalogEntry::id)
            .map(str::to_owned)
            .collect(),
        prompts: selection
            .prompts
            .iter()
            .map(CatalogEntry::id)
            .map(str::to_owned)
            .collect(),
        fingerprint: selection.fingerprint.clone(),
    }
}

fn map_ambient_context(mode: AmbientContextMode) -> AmbientContextPolicy {
    match mode {
        AmbientContextMode::Ambient => AmbientContextPolicy::Ambient,
        AmbientContextMode::Controlled => AmbientContextPolicy::Controlled,
        AmbientContextMode::Hermetic => AmbientContextPolicy::Hermetic,
    }
}

fn map_session_mode(mode: SessionModeArg) -> SessionMode {
    match mode {
        SessionModeArg::Sticky => SessionMode::Sticky,
        SessionModeArg::Fresh => SessionMode::Fresh,
        SessionModeArg::Managed => SessionMode::Managed,
    }
}

impl FileCatalogDefinition {
    fn resolve(self) -> Result<CatalogDefinition> {
        Ok(match self {
            Self::Agent {
                id,
                description,
                inline,
                path,
                default_skills,
            } => CatalogDefinition::Agent {
                source: configured_catalog_source(&id, inline, path)?,
                id,
                description,
                default_skills,
            },
            Self::Skill {
                id,
                description,
                inline,
                path,
            } => CatalogDefinition::Skill {
                source: configured_catalog_source(&id, inline, path)?,
                id,
                description,
            },
            Self::Prompt {
                id,
                description,
                inline,
                path,
                requires,
                arguments,
            } => CatalogDefinition::Prompt {
                source: configured_catalog_source(&id, inline, path)?,
                id,
                description,
                requires,
                arguments,
            },
        })
    }
}

fn configured_catalog_source(
    id: &str,
    inline: Option<String>,
    path: Option<PathBuf>,
) -> Result<CatalogSource> {
    match (inline, path) {
        (Some(content), None) => Ok(CatalogSource::Inline { content }),
        (None, Some(path)) => Ok(CatalogSource::MarkdownPath { path }),
        (None, None) => bail!("context definition {id} requires exactly one of inline or path"),
        (Some(_), Some(_)) => {
            bail!("context definition {id} cannot declare both inline and path")
        }
    }
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
    fn session_mode_is_strict_layered_provenanced_and_resume_safe() {
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("roba.toml");
        write(&config, "version = 1\n[session]\nmode = 'managed'\n");

        let resolved = resolve_from(&AgentArgs::default(), temp.path(), Some(config.clone()))
            .expect("managed session config resolves");
        assert_eq!(resolved.session_policy.mode, SessionMode::Managed);
        assert_eq!(resolved.effective.session.mode, SessionModeArg::Managed);
        assert_eq!(
            resolved.effective.provenance["session.mode"],
            [config.display().to_string()]
        );

        let cli = resolve_from(
            &args(&["--session-mode", "sticky"]),
            temp.path(),
            Some(config),
        )
        .expect("CLI session policy overrides the file");
        assert_eq!(cli.session_policy.mode, SessionMode::Sticky);
        assert_eq!(cli.effective.provenance["session.mode"], ["cli"]);

        let error = resolve_from(
            &args(&["--session-mode", "fresh", "--resume", "seed"]),
            temp.path(),
            None,
        )
        .expect_err("fresh policy and explicit resume conflict");
        assert!(error.to_string().contains("cannot be combined"));
    }

    #[test]
    fn ambient_context_policy_is_strict_layered_and_provenanced() {
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("roba.toml");
        write(
            &config,
            "version = 1\n[agent]\nprovider = 'codex'\n[context]\nambient_policy = 'controlled'\n",
        );

        let resolved = resolve_from(&args(&[]), temp.path(), Some(config.clone())).unwrap();
        assert_eq!(
            resolved.ambient_context_policy,
            AmbientContextPolicy::Controlled
        );
        assert_eq!(
            resolved.effective.context.ambient_policy,
            AmbientContextMode::Controlled
        );
        assert_eq!(
            resolved.effective.provenance["context.ambient_policy"],
            [config.display().to_string()]
        );

        let cli = resolve_from(
            &args(&["--ambient-context", "ambient"]),
            temp.path(),
            Some(config),
        )
        .unwrap();
        assert_eq!(cli.ambient_context_policy, AmbientContextPolicy::Ambient);
        assert_eq!(
            cli.effective.provenance["context.ambient_policy"],
            ["cli".to_owned()]
        );

        let invalid = temp.path().join("invalid.toml");
        write(
            &invalid,
            "version = 1\n[context]\nambient_policy = 'unknown'\n",
        );
        let error = resolve_from(&args(&[]), temp.path(), Some(invalid)).unwrap_err();
        assert!(format!("{error:#}").contains("unknown variant"));
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
    fn managed_catalog_layers_resolve_paths_selection_and_safe_provenance() {
        let temp = TempDir::new().unwrap();
        let user = temp.path().join("user/roba/roba.toml");
        write(
            &user,
            "version = 1\n\
             [context]\nskills = ['local.review']\n\
             [[context.definitions]]\nkind = 'skill'\nid = 'local.review'\ndescription = 'Review carefully.'\npath = 'skills/review.md'\n",
        );
        write(
            &user.parent().unwrap().join("skills/review.md"),
            "Private review material that must not be serialized.",
        );

        let repo = temp.path().join("repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        write(
            &repo.join("roba.toml"),
            "version = 1\n\
             [context]\nagent = 'local.worker'\nprompts = ['local.issue']\n\
             [[context.definitions]]\nkind = 'agent'\nid = 'local.worker'\ndescription = 'Local worker.'\ninline = 'Private agent material.'\ndefault_skills = ['local.review']\n\
             [[context.definitions]]\nkind = 'prompt'\nid = 'local.issue'\ndescription = 'Work one issue.'\ninline = 'Private prompt material for {{issue}}.'\nrequires = ['roba.repository-change']\narguments = [{ name = 'issue', description = 'Issue id.', required = true }]\n",
        );

        let resolved = resolve_from(&args(&[]), &repo, Some(user.clone())).unwrap();
        let context = &resolved.effective.context;
        assert_eq!(context.agent.as_deref(), Some("local.worker"));
        assert_eq!(context.skills, ["local.review"]);
        assert_eq!(context.prompts, ["local.issue"]);
        let selection = context.selection.as_ref().unwrap();
        assert_eq!(selection.agent, "local.worker");
        assert_eq!(selection.skills, ["local.review", "roba.repository-change"]);
        assert_eq!(selection.prompts, ["local.issue"]);
        assert!(selection.fingerprint.as_str().starts_with("sha256:"));

        let review = context
            .catalog
            .entries
            .iter()
            .find(|entry| entry.id() == "local.review")
            .unwrap();
        match review {
            CatalogEntry::Skill { origin, source, .. } => {
                assert_eq!(origin.kind, CatalogOriginKind::User);
                assert_eq!(origin.locator.as_deref(), Some(user.to_str().unwrap()));
                assert!(matches!(
                    source,
                    roba_context::CatalogSourceMetadata::MarkdownPath { path }
                        if path == Path::new("skills/review.md")
                ));
            }
            other => panic!("expected skill metadata, got {other:?}"),
        }

        let serialized = serde_json::to_string(&resolved.effective).unwrap();
        for body in [
            "Private review material",
            "Private agent material",
            "Private prompt material",
        ] {
            assert!(!serialized.contains(body));
        }
        assert_eq!(
            resolved.effective.provenance["context.definitions"],
            [
                user.display().to_string(),
                repo.join("roba.toml").display().to_string(),
            ]
        );
    }

    #[test]
    fn catalog_selection_is_opt_in_and_builtins_can_be_disabled_or_filtered() {
        let empty = resolve_from(&args(&["--no-config"]), Path::new("."), None).unwrap();
        assert!(empty.effective.context.agent.is_none());
        assert!(empty.effective.context.selection.is_none());
        assert_eq!(empty.effective.context.catalog.entries.len(), 3);
        assert!(empty.template.agent.instructions.is_empty());
        assert!(empty.template.context.project.is_empty());
        assert!(empty.template.context.run.is_empty());

        let temp = TempDir::new().unwrap();
        write(
            &temp.path().join("roba.toml"),
            "version = 1\n[context.builtins]\nenabled = false\n",
        );
        let disabled = resolve_from(&args(&[]), temp.path(), None).unwrap();
        assert!(disabled.effective.context.catalog.entries.is_empty());

        write(
            &temp.path().join("roba.toml"),
            "version = 1\n\
             [context]\nagent = 'roba.repo-worker'\n\
             [context.builtins]\ndisable = ['roba.issue-worker']\n",
        );
        let filtered = resolve_from(&args(&[]), temp.path(), None).unwrap();
        assert!(
            filtered
                .effective
                .context
                .catalog
                .entries
                .iter()
                .all(|entry| entry.id() != "roba.issue-worker")
        );
        assert_eq!(
            filtered
                .effective
                .context
                .selection
                .as_ref()
                .unwrap()
                .skills,
            ["roba.repository-change"]
        );
    }

    #[test]
    fn invalid_catalog_configuration_fails_before_provider_work() {
        let temp = TempDir::new().unwrap();
        let config = temp.path().join("roba.toml");

        write(
            &config,
            "version = 1\n[context]\nskills = ['roba.repository-change']\n",
        );
        assert!(
            resolve_from(&args(&[]), temp.path(), None)
                .unwrap_err()
                .to_string()
                .contains("require context.agent")
        );

        write(
            &config,
            "version = 1\n\
             [[context.definitions]]\nkind = 'skill'\nid = 'local.bad'\ndescription = 'Bad.'\ninline = 'body'\npath = 'bad.md'\n",
        );
        assert!(
            format!(
                "{:#}",
                resolve_from(&args(&[]), temp.path(), None).unwrap_err()
            )
            .contains("cannot declare both inline and path")
        );

        write(
            &config,
            "version = 1\n[context.builtins]\ndisable = ['roba.missing']\n",
        );
        assert!(
            resolve_from(&args(&[]), temp.path(), None)
                .unwrap_err()
                .to_string()
                .contains("unknown built-in id")
        );

        write(
            &config,
            "version = 1\n\
             [[context.definitions]]\nkind = 'skill'\nid = 'local.duplicate'\ndescription = 'One.'\ninline = 'one'\n\
             [[context.definitions]]\nkind = 'skill'\nid = 'local.duplicate'\ndescription = 'Two.'\ninline = 'two'\n",
        );
        assert!(
            format!(
                "{:#}",
                resolve_from(&args(&[]), temp.path(), None).unwrap_err()
            )
            .contains("duplicate catalog artifact id")
        );
    }

    #[test]
    fn tracked_startup_sample_is_the_real_strict_schema() {
        let config: StartupFile = toml::from_str(include_str!("../roba-startup.sample.toml"))
            .expect("tracked provider-neutral startup sample must parse");
        assert_eq!(config.version, CONFIG_VERSION);
        assert_eq!(config.agent.provider, Some(RunProvider::Codex));
        assert_eq!(config.context.agent.as_deref(), Some("roba.repo-worker"));
        assert_eq!(config.context.prompts, ["roba.issue-worker"]);
        assert_eq!(config.extensions.git.enabled, Some(true));
        assert_eq!(config.extensions.git.progress_interval_secs, Some(5));
    }

    #[test]
    fn repository_self_config_is_a_valid_conservative_startup_config() {
        let config: StartupFile = toml::from_str(include_str!("../roba.toml"))
            .expect("Roba's checked-in self configuration must parse");
        assert_eq!(config.version, CONFIG_VERSION);
        assert_eq!(config.agent.provider, Some(RunProvider::Codex));
        assert_eq!(config.context.agent.as_deref(), Some("roba.repo-worker"));
        assert_eq!(config.context.prompts, ["roba.issue-worker"]);
        assert_eq!(
            config.execution.permissions,
            Some(PermissionPolicy::ReadOnly)
        );
        assert_eq!(config.extensions.git.enabled, Some(true));
        assert_eq!(config.extensions.git.progress_interval_secs, Some(5));
    }
}
