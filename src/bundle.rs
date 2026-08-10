//! Shared validation, provisioning, and redacted inspection for legacy
//! Claude `.roba/` bundles.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use claude_wrapper::artifacts::AgentsRoot;
use serde::Serialize;
use serde_json::{Map as JsonMap, Value as JsonValue};

use crate::VersionedResult;
use crate::cli::{AskArgs, BundleCmd, BundleInspectArgs};
use crate::engine;

const RECOGNIZED_TOP_LEVEL: &[&str] = &[
    ".claude-plugin",
    "agents",
    "mcp.json",
    "plugins",
    "roba.toml",
    "settings.json",
    "skills",
    "system-prompt.md",
];

/// Provider-specific bundle material validated once before it is inspected or
/// passed into the legacy Claude one-shot path.
pub(crate) struct BundlePlan {
    system_prompt: Option<String>,
    mcp_config: Option<PathBuf>,
    settings: Option<PathBuf>,
    agents_json: Option<String>,
    plugin_roots: Vec<String>,
    config: crate::profile::Pool,
    inspection: BundleInspection,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct BundleInspection {
    root: String,
    artifacts: Vec<String>,
    agents: Vec<AgentInspection>,
    mcp_servers: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    settings: Option<SettingsInspection>,
    plugins: Vec<PluginInspection>,
    warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct AgentInspection {
    name: String,
    description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    model: Option<String>,
    tools: Vec<String>,
    skills: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct SettingsInspection {
    permission_rule_counts: BTreeMap<String, usize>,
    hook_events: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct PluginInspection {
    root: String,
    manifest: String,
    name: String,
}

impl BundlePlan {
    pub(crate) fn load(root: &Path) -> Result<Self> {
        if !root.exists() {
            bail!("bundle {} does not exist", root.display());
        }
        if !root.is_dir() {
            bail!("bundle {} is not a directory", root.display());
        }
        validate_recognized_entry_types(root)?;

        // Snapshot the parsed config once. Validation and execution must not
        // reopen potentially changed bytes, and parse diagnostics must not
        // echo a source line that may contain a secret.
        let config = crate::profile::pool::load_bundle_pool(root).map_err(|_| {
            anyhow::anyhow!(
                "bundle config {} could not be read or parsed; source details redacted",
                root.join("roba.toml").display()
            )
        })?;

        let mut artifacts = Vec::new();
        let mut warnings = unknown_top_level_entries(root)?;
        let roba_toml = root.join("roba.toml");
        push_artifact(&mut artifacts, "roba.toml", &roba_toml);

        let system_prompt_path = root.join("system-prompt.md");
        push_artifact(&mut artifacts, "system-prompt.md", &system_prompt_path);
        let system_prompt = if system_prompt_path.is_file() {
            let content = std::fs::read_to_string(&system_prompt_path)
                .with_context(|| {
                    format!(
                        "reading bundle system prompt {}",
                        system_prompt_path.display()
                    )
                })?
                .trim()
                .to_string();
            (!content.is_empty()).then_some(content)
        } else {
            None
        };

        let mcp_path = root.join("mcp.json");
        push_artifact(&mut artifacts, "mcp.json", &mcp_path);
        let (mcp_config, mcp_servers) = if mcp_path.is_file() {
            let value = read_json_object(&mcp_path, "bundle MCP config")?;
            let servers = inspect_mcp_servers(&value, &mcp_path)?;
            (Some(mcp_path), servers)
        } else {
            (None, Vec::new())
        };

        let settings_path = root.join("settings.json");
        push_artifact(&mut artifacts, "settings.json", &settings_path);
        let (settings, settings_inspection) = if settings_path.is_file() {
            let value = read_json_object(&settings_path, "bundle settings")?;
            let inspection = inspect_settings(&value, &settings_path)?;
            (Some(settings_path), Some(inspection))
        } else {
            (None, None)
        };

        let agents_dir = root.join("agents");
        push_artifact(&mut artifacts, "agents/", &agents_dir);
        let (agents_json, agents, agent_artifacts) = load_agents(&agents_dir)?;
        artifacts.extend(agent_artifacts);

        let skills_dir = root.join("skills");
        push_artifact(&mut artifacts, "skills/", &skills_dir);
        let plugins_dir = root.join("plugins");
        push_artifact(&mut artifacts, "plugins/", &plugins_dir);
        let (plugin_roots, plugins, plugin_artifacts) = load_plugins(root)?;
        artifacts.extend(plugin_artifacts);

        artifacts.sort();
        artifacts.dedup();
        warnings.sort();

        let inspection = BundleInspection {
            root: root.display().to_string(),
            artifacts,
            agents,
            mcp_servers,
            settings: settings_inspection,
            plugins,
            warnings,
        };

        Ok(Self {
            system_prompt,
            mcp_config,
            settings,
            agents_json,
            plugin_roots,
            config,
            inspection,
        })
    }

    pub(crate) fn resolve_pool(
        &self,
        cwd: &Path,
        bundle_only: bool,
    ) -> Result<crate::profile::Pool> {
        crate::profile::pool::load_pool_with_preparsed_bundle(
            cwd,
            Some(self.config.clone()),
            bundle_only,
        )
    }

    pub(crate) fn apply_context(&self, args: &mut AskArgs) {
        if let Some(content) = &self.system_prompt {
            args.append_system_prompt = Some(match args.append_system_prompt.take() {
                Some(existing) => format!("{existing}\n\n{content}"),
                None => content.clone(),
            });
        }
        if let Some(mcp) = &self.mcp_config {
            args.mcp_config.push(mcp.to_string_lossy().into_owned());
        }
    }

    pub(crate) fn apply_provisioning(&self, config: &mut engine::Config) {
        if let Some(settings) = &self.settings {
            config.settings = Some(settings.to_string_lossy().into_owned());
        }
        config.agents_json.clone_from(&self.agents_json);
        config.plugin_dir.extend(self.plugin_roots.iter().cloned());
    }
}

/// Dispatch the zero-provider `roba bundle` inspection surface.
pub fn run(cmd: BundleCmd) -> Result<()> {
    match cmd {
        BundleCmd::Inspect(args) => inspect(args),
    }
}

fn inspect(args: BundleInspectArgs) -> Result<()> {
    let plan = BundlePlan::load(&args.path)?;
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&VersionedResult::new(&plan.inspection))?
        );
    } else {
        print!("{}", render_human(&plan.inspection));
    }
    Ok(())
}

fn load_agents(agents_dir: &Path) -> Result<(Option<String>, Vec<AgentInspection>, Vec<String>)> {
    if !agents_dir.is_dir() {
        return Ok((None, Vec::new(), Vec::new()));
    }

    let root = AgentsRoot::at(agents_dir);
    let mut stems = Vec::new();
    for entry in std::fs::read_dir(agents_dir)
        .with_context(|| format!("reading bundle agents {}", agents_dir.display()))?
    {
        let path = entry
            .with_context(|| format!("reading bundle agents {}", agents_dir.display()))?
            .path();
        if !path.is_file() || path.extension().and_then(|value| value.to_str()) != Some("md") {
            continue;
        }
        let stem = path
            .file_stem()
            .and_then(|value| value.to_str())
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "bundle agent filename is not valid UTF-8: {}",
                    path.display()
                )
            })?;
        stems.push(stem.to_string());
    }
    stems.sort();

    let mut definitions = BTreeMap::new();
    let mut inspections = Vec::new();
    let mut artifacts = Vec::new();
    for stem in stems {
        let agent = root.get(&stem).map_err(|_| {
            anyhow::anyhow!(
                "bundle agent {} could not be parsed; source details redacted",
                agents_dir.join(format!("{stem}.md")).display()
            )
        })?;
        let description = agent
            .description
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "bundle agent {} needs a non-empty `description` in its frontmatter",
                    agent.file_path.display()
                )
            })?;
        if agent.body.trim().is_empty() {
            bail!(
                "bundle agent {} needs a non-empty prompt body",
                agent.file_path.display()
            );
        }

        let tools = agent.tools;
        let skills = agent.skills;
        let model = agent.model;
        let mut definition = JsonMap::new();
        definition.insert(
            "description".to_string(),
            JsonValue::String(description.clone()),
        );
        definition.insert("prompt".to_string(), JsonValue::String(agent.body));
        if !tools.is_empty() {
            definition.insert("tools".to_string(), serde_json::to_value(&tools)?);
        }
        if let Some(value) = &model {
            definition.insert("model".to_string(), JsonValue::String(value.clone()));
        }
        if !skills.is_empty() {
            definition.insert("skills".to_string(), serde_json::to_value(&skills)?);
        }
        definitions.insert(stem.clone(), JsonValue::Object(definition));
        artifacts.push(format!("agents/{stem}.md"));
        let mut inspected_tools = tools;
        inspected_tools.sort();
        inspected_tools.dedup();
        let mut inspected_skills = skills;
        inspected_skills.sort();
        inspected_skills.dedup();
        inspections.push(AgentInspection {
            name: stem,
            description,
            model,
            tools: inspected_tools,
            skills: inspected_skills,
        });
    }

    let json = (!definitions.is_empty())
        .then(|| serde_json::to_string(&definitions))
        .transpose()?;
    Ok((json, inspections, artifacts))
}

fn load_plugins(root: &Path) -> Result<(Vec<String>, Vec<PluginInspection>, Vec<String>)> {
    let mut plugin_paths = Vec::new();
    if root.join("skills").is_dir() {
        plugin_paths.push(root.to_path_buf());
    }

    let plugins = root.join("plugins");
    if plugins.is_dir() {
        if has_plugin_manifest(&plugins) {
            plugin_paths.push(plugins);
        } else {
            let mut children = Vec::new();
            for entry in std::fs::read_dir(&plugins)
                .with_context(|| format!("reading bundle plugins {}", plugins.display()))?
            {
                let path = entry
                    .with_context(|| format!("reading bundle plugins {}", plugins.display()))?
                    .path();
                if path.is_dir() {
                    children.push(path);
                }
            }
            let mut normalized = Vec::with_capacity(children.len());
            for child in children {
                normalized.push((relative_display(root, &child)?, child));
            }
            normalized.sort_by(|left, right| left.0.cmp(&right.0));
            plugin_paths.extend(normalized.into_iter().map(|(_, path)| path));
        }
    }

    let mut roots = Vec::new();
    let mut inspections = Vec::new();
    let mut artifacts = Vec::new();
    for path in plugin_paths {
        let manifest = path.join(".claude-plugin/plugin.json");
        if !manifest.is_file() {
            bail!(
                "bundle plugin {} needs .claude-plugin/plugin.json before it can be passed to Claude",
                path.display()
            );
        }
        let value = read_json_object(&manifest, "bundle plugin manifest")?;
        let name = value
            .get("name")
            .and_then(JsonValue::as_str)
            .filter(|name| !name.trim().is_empty())
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "bundle plugin manifest {} needs a non-empty string `name`",
                    manifest.display()
                )
            })?
            .to_string();
        let relative_root = relative_display(root, &path)?;
        let relative_manifest = relative_display(root, &manifest)?;
        roots.push(path.to_string_lossy().into_owned());
        inspections.push(PluginInspection {
            root: relative_root,
            manifest: relative_manifest.clone(),
            name,
        });
        artifacts.push(relative_manifest);
    }
    Ok((roots, inspections, artifacts))
}

fn has_plugin_manifest(root: &Path) -> bool {
    root.join(".claude-plugin/plugin.json").is_file()
}

fn validate_recognized_entry_types(root: &Path) -> Result<()> {
    for name in ["roba.toml", "system-prompt.md", "mcp.json", "settings.json"] {
        let path = root.join(name);
        if path.exists() && !path.is_file() {
            bail!("bundle artifact {} must be a file", path.display());
        }
    }
    for name in ["agents", "skills", "plugins", ".claude-plugin"] {
        let path = root.join(name);
        if path.exists() && !path.is_dir() {
            bail!("bundle artifact {} must be a directory", path.display());
        }
    }
    Ok(())
}

fn push_artifact(artifacts: &mut Vec<String>, relative: &str, path: &Path) {
    if path.exists() {
        artifacts.push(relative.to_string());
    }
}

fn read_json_object(path: &Path, label: &str) -> Result<JsonMap<String, JsonValue>> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("reading {label} {}", path.display()))?;
    let value: JsonValue = serde_json::from_str(&raw)
        .with_context(|| format!("parsing {label} {}", path.display()))?;
    value
        .as_object()
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("{label} {} must contain a JSON object", path.display()))
}

fn object_keys(
    object: &JsonMap<String, JsonValue>,
    key: &str,
    path: &Path,
    label: &str,
) -> Result<Vec<String>> {
    let Some(value) = object.get(key) else {
        return Ok(Vec::new());
    };
    let nested = value.as_object().ok_or_else(|| {
        anyhow::anyhow!(
            "{label} {} field `{key}` must be a JSON object",
            path.display()
        )
    })?;
    let mut keys: Vec<String> = nested.keys().cloned().collect();
    keys.sort();
    Ok(keys)
}

fn inspect_mcp_servers(object: &JsonMap<String, JsonValue>, path: &Path) -> Result<Vec<String>> {
    let Some(value) = object.get("mcpServers") else {
        return Ok(Vec::new());
    };
    let servers = value.as_object().ok_or_else(|| {
        anyhow::anyhow!(
            "bundle MCP config {} field `mcpServers` must be a JSON object",
            path.display()
        )
    })?;
    for (name, definition) in servers {
        if !definition.is_object() {
            bail!(
                "bundle MCP config {} server `{name}` must be a JSON object",
                path.display()
            );
        }
    }
    let mut names: Vec<String> = servers.keys().cloned().collect();
    names.sort();
    Ok(names)
}

fn inspect_settings(
    object: &JsonMap<String, JsonValue>,
    path: &Path,
) -> Result<SettingsInspection> {
    let mut permission_rule_counts = BTreeMap::new();
    if let Some(permissions) = object.get("permissions") {
        let permissions = permissions.as_object().ok_or_else(|| {
            anyhow::anyhow!(
                "bundle settings {} field `permissions` must be a JSON object",
                path.display()
            )
        })?;
        for kind in ["allow", "ask", "deny"] {
            let Some(rules) = permissions.get(kind) else {
                continue;
            };
            let rules = rules.as_array().ok_or_else(|| {
                anyhow::anyhow!(
                    "bundle settings {} permission `{kind}` must be a JSON array",
                    path.display()
                )
            })?;
            permission_rule_counts.insert(kind.to_string(), rules.len());
        }
    }
    let hook_events = object_keys(object, "hooks", path, "bundle settings")?;
    if let Some(hooks) = object.get("hooks").and_then(JsonValue::as_object) {
        for (event, definitions) in hooks {
            if !definitions.is_array() {
                bail!(
                    "bundle settings {} hook event `{event}` must be a JSON array",
                    path.display()
                );
            }
        }
    }
    Ok(SettingsInspection {
        permission_rule_counts,
        hook_events,
    })
}

fn unknown_top_level_entries(root: &Path) -> Result<Vec<String>> {
    let recognized: BTreeSet<&str> = RECOGNIZED_TOP_LEVEL.iter().copied().collect();
    let mut warnings = Vec::new();
    for entry in
        std::fs::read_dir(root).with_context(|| format!("reading bundle {}", root.display()))?
    {
        let entry = entry.with_context(|| format!("reading bundle {}", root.display()))?;
        let name = entry.file_name().into_string().map_err(|name| {
            anyhow::anyhow!(
                "bundle entry name is not valid UTF-8: {}",
                PathBuf::from(name).display()
            )
        })?;
        if !recognized.contains(name.as_str()) {
            warnings.push(format!("unknown top-level entry: {name}"));
        }
    }
    Ok(warnings)
}

fn relative_display(root: &Path, path: &Path) -> Result<String> {
    let relative = path.strip_prefix(root).with_context(|| {
        format!(
            "bundle artifact {} escaped root {}",
            path.display(),
            root.display()
        )
    })?;
    if relative.as_os_str().is_empty() {
        return Ok(".".to_string());
    }
    let parts: Result<Vec<_>> = relative
        .components()
        .map(|component| {
            component
                .as_os_str()
                .to_str()
                .map(str::to_string)
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "bundle artifact path is not valid UTF-8: {}",
                        path.display()
                    )
                })
        })
        .collect();
    Ok(parts?.join("/"))
}

fn render_human(inspection: &BundleInspection) -> String {
    let mut out = format!("bundle {}\n", inspection.root);
    section(&mut out, "artifacts", &inspection.artifacts);

    out.push_str(&format!("agents ({})\n", inspection.agents.len()));
    for agent in &inspection.agents {
        out.push_str(&format!("  {} — {}\n", agent.name, agent.description));
        if let Some(model) = &agent.model {
            out.push_str(&format!("    model: {model}\n"));
        }
        if !agent.tools.is_empty() {
            out.push_str(&format!("    tools: {}\n", agent.tools.join(", ")));
        }
        if !agent.skills.is_empty() {
            out.push_str(&format!("    skills: {}\n", agent.skills.join(", ")));
        }
    }
    section(&mut out, "mcp servers", &inspection.mcp_servers);

    if let Some(settings) = &inspection.settings {
        out.push_str("settings\n");
        for (kind, count) in &settings.permission_rule_counts {
            out.push_str(&format!("  permission {kind}: {count} rule(s)\n"));
        }
        for event in &settings.hook_events {
            out.push_str(&format!("  hook event: {event}\n"));
        }
    } else {
        out.push_str("settings (none)\n");
    }

    out.push_str(&format!("plugins ({})\n", inspection.plugins.len()));
    for plugin in &inspection.plugins {
        out.push_str(&format!("  {} — {}\n", plugin.root, plugin.name));
    }
    if !inspection.warnings.is_empty() {
        section(&mut out, "warnings", &inspection.warnings);
    }
    out
}

fn section(out: &mut String, label: &str, values: &[String]) {
    out.push_str(&format!("{label} ({})\n", values.len()));
    for value in values {
        out.push_str(&format!("  {value}\n"));
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::*;

    fn complete_bundle() -> tempfile::TempDir {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();

        // Deliberately create entries out of lexical order; inspection is a
        // stable machine contract, not filesystem enumeration order.
        std::fs::write(root.join("z-unknown.txt"), "ignored").unwrap();
        std::fs::write(root.join("system-prompt.md"), "TOP SECRET PROMPT").unwrap();
        std::fs::write(
            root.join("settings.json"),
            r#"{
                "hooks": {"PreToolUse": [{"hooks": [{"command": "SECRET COMMAND"}]}]},
                "permissions": {"deny": ["Bash(secret:*)"], "allow": ["Read", "Grep"]},
                "env": {"SECRET_ENV": "SECRET VALUE"}
            }"#,
        )
        .unwrap();
        std::fs::write(
            root.join("mcp.json"),
            r#"{"mcpServers":{"zeta":{"command":"SECRET MCP"},"alpha":{"command":"safe"}}}"#,
        )
        .unwrap();
        std::fs::write(root.join("roba.toml"), "model = \"sonnet\"\n").unwrap();

        let agents = root.join("agents");
        std::fs::create_dir_all(&agents).unwrap();
        std::fs::write(
            agents.join("zeta.md"),
            "---\ndescription: Zeta agent\ntools: Grep, Read\n---\nSECRET ZETA BODY",
        )
        .unwrap();
        std::fs::write(
            agents.join("alpha.md"),
            "---\ndescription: Alpha agent\nmodel: haiku\nskills: audit\n---\nSECRET ALPHA BODY",
        )
        .unwrap();

        std::fs::create_dir_all(root.join("skills/review")).unwrap();
        std::fs::create_dir_all(root.join(".claude-plugin")).unwrap();
        std::fs::write(
            root.join(".claude-plugin/plugin.json"),
            r#"{"name":"bundle"}"#,
        )
        .unwrap();
        for name in ["zeta", "alpha"] {
            let manifest = root.join("plugins").join(name).join(".claude-plugin");
            std::fs::create_dir_all(&manifest).unwrap();
            std::fs::write(
                manifest.join("plugin.json"),
                format!(r#"{{"name":"{name}"}}"#),
            )
            .unwrap();
        }
        temp
    }

    #[test]
    fn plan_is_sorted_redacted_and_shared_with_provisioning() {
        let temp = complete_bundle();
        let plan = BundlePlan::load(temp.path()).unwrap();
        let inspection = &plan.inspection;

        assert_eq!(
            inspection
                .agents
                .iter()
                .map(|agent| agent.name.as_str())
                .collect::<Vec<_>>(),
            ["alpha", "zeta"]
        );
        assert_eq!(inspection.mcp_servers, ["alpha", "zeta"]);
        assert_eq!(
            inspection
                .plugins
                .iter()
                .map(|plugin| plugin.name.as_str())
                .collect::<Vec<_>>(),
            ["bundle", "alpha", "zeta"]
        );
        assert_eq!(
            inspection.settings.as_ref().unwrap().permission_rule_counts,
            BTreeMap::from([("allow".to_string(), 2), ("deny".to_string(), 1)])
        );
        assert_eq!(
            inspection.settings.as_ref().unwrap().hook_events,
            ["PreToolUse"]
        );
        assert_eq!(
            inspection.warnings,
            ["unknown top-level entry: z-unknown.txt"]
        );

        let json = serde_json::to_string(inspection).unwrap();
        for secret in [
            "TOP SECRET PROMPT",
            "SECRET COMMAND",
            "SECRET VALUE",
            "SECRET MCP",
            "SECRET ALPHA BODY",
            "SECRET ZETA BODY",
        ] {
            assert!(
                !json.contains(secret),
                "inspection leaked {secret:?}: {json}"
            );
        }

        let mut args = crate::cli::Cli::try_parse_from(["roba", "prompt"])
            .unwrap()
            .ask;
        plan.apply_context(&mut args);
        assert_eq!(
            args.append_system_prompt.as_deref(),
            Some("TOP SECRET PROMPT")
        );
        assert_eq!(
            args.mcp_config,
            [temp.path().join("mcp.json").display().to_string()]
        );

        let mut config = engine::Config::new("prompt");
        plan.apply_provisioning(&mut config);
        assert_eq!(config.plugin_dir.len(), 3);
        assert!(
            config
                .agents_json
                .as_deref()
                .unwrap()
                .contains("SECRET ALPHA BODY")
        );
    }

    #[test]
    fn plan_refuses_missing_non_directory_and_malformed_structures() {
        let temp = tempfile::tempdir().unwrap();
        assert!(
            BundlePlan::load(&temp.path().join("missing"))
                .err()
                .unwrap()
                .to_string()
                .contains("does not exist")
        );

        let file = temp.path().join("file");
        std::fs::write(&file, "x").unwrap();
        assert!(
            BundlePlan::load(&file)
                .err()
                .unwrap()
                .to_string()
                .contains("not a directory")
        );

        let bundle = tempfile::tempdir().unwrap();
        std::fs::write(bundle.path().join("mcp.json"), "[]").unwrap();
        assert!(
            BundlePlan::load(bundle.path())
                .err()
                .unwrap()
                .to_string()
                .contains("bundle MCP config")
        );

        let bundle = tempfile::tempdir().unwrap();
        std::fs::write(bundle.path().join("mcp.json"), r#"{"mcpServers":[]}"#).unwrap();
        assert!(
            BundlePlan::load(bundle.path())
                .err()
                .unwrap()
                .to_string()
                .contains("field `mcpServers` must be a JSON object")
        );

        let bundle = tempfile::tempdir().unwrap();
        std::fs::write(bundle.path().join("settings.json"), r#"{"hooks":[]}"#).unwrap();
        assert!(
            BundlePlan::load(bundle.path())
                .err()
                .unwrap()
                .to_string()
                .contains("field `hooks` must be a JSON object")
        );

        let bundle = tempfile::tempdir().unwrap();
        std::fs::write(
            bundle.path().join("mcp.json"),
            r#"{"mcpServers":{"bad":[]}}"#,
        )
        .unwrap();
        assert!(
            BundlePlan::load(bundle.path())
                .err()
                .unwrap()
                .to_string()
                .contains("server `bad` must be a JSON object")
        );

        let bundle = tempfile::tempdir().unwrap();
        std::fs::write(
            bundle.path().join("settings.json"),
            r#"{"permissions":{"allow":"Read"}}"#,
        )
        .unwrap();
        assert!(
            BundlePlan::load(bundle.path())
                .err()
                .unwrap()
                .to_string()
                .contains("permission `allow` must be a JSON array")
        );

        let bundle = tempfile::tempdir().unwrap();
        std::fs::write(
            bundle.path().join("settings.json"),
            r#"{"hooks":{"PreToolUse":{}}}"#,
        )
        .unwrap();
        assert!(
            BundlePlan::load(bundle.path())
                .err()
                .unwrap()
                .to_string()
                .contains("hook event `PreToolUse` must be a JSON array")
        );

        let bundle = tempfile::tempdir().unwrap();
        std::fs::create_dir(bundle.path().join("mcp.json")).unwrap();
        assert!(
            BundlePlan::load(bundle.path())
                .err()
                .unwrap()
                .to_string()
                .contains("must be a file")
        );
    }

    #[test]
    fn absent_bundle_settings_do_not_clear_resolved_settings() {
        let bundle = tempfile::tempdir().unwrap();
        let plan = BundlePlan::load(bundle.path()).unwrap();
        let mut config = engine::Config::new("prompt");
        config.settings = Some("existing-settings.json".to_string());

        plan.apply_provisioning(&mut config);

        assert_eq!(config.settings.as_deref(), Some("existing-settings.json"));
    }

    #[test]
    fn parsed_bundle_config_is_redacted_and_snapshotted_for_execution() {
        let bundle = tempfile::tempdir().unwrap();
        let config = bundle.path().join("roba.toml");
        std::fs::write(&config, "model = \"sonnet\"\n").unwrap();
        let plan = BundlePlan::load(bundle.path()).unwrap();

        // Execution uses the already-inspected config even if the file moves.
        std::fs::write(&config, "model = \"opus\"\nSECRET = \"value\"\n").unwrap();
        let resolved = plan.resolve_pool(bundle.path(), true).unwrap();
        assert_eq!(resolved.defaults.model.as_deref(), Some("sonnet"));

        let malformed = tempfile::tempdir().unwrap();
        let secret = "SUPER_SECRET_CONFIG_VALUE";
        std::fs::write(
            malformed.path().join("roba.toml"),
            format!("unknown = \"{secret}\"\n"),
        )
        .unwrap();
        let error = BundlePlan::load(malformed.path())
            .err()
            .unwrap()
            .to_string();
        assert!(error.contains("source details redacted"));
        assert!(!error.contains(secret));
    }

    #[test]
    fn plugin_order_uses_normalized_unicode_paths_on_every_platform() {
        let bundle = tempfile::tempdir().unwrap();
        for name in ["\u{10000}", "\u{e000}"] {
            let manifest = bundle
                .path()
                .join("plugins")
                .join(name)
                .join(".claude-plugin");
            std::fs::create_dir_all(&manifest).unwrap();
            std::fs::write(
                manifest.join("plugin.json"),
                format!(r#"{{"name":"{name}"}}"#),
            )
            .unwrap();
        }

        let plan = BundlePlan::load(bundle.path()).unwrap();
        assert_eq!(
            plan.inspection
                .plugins
                .iter()
                .map(|plugin| plugin.root.as_str())
                .collect::<Vec<_>>(),
            ["plugins/\u{e000}", "plugins/\u{10000}"]
        );
    }
}
