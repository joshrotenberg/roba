use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::builtins::builtin_definitions;
use crate::types::{
    CATALOG_SCHEMA_VERSION, CatalogArtifactKind, CatalogDefinition, CatalogEntry,
    CatalogFingerprint, CatalogManifest, CatalogOrigin, CatalogOriginKind, CatalogSelection,
    CatalogSelectionSpec, CatalogSource, CatalogSourceMetadata, MAX_ARTIFACT_BYTES,
    MAX_CATALOG_BYTES, MAX_PROMPT_ARGUMENT_BYTES, PromptArgumentDefinition,
};

const MAX_ID_BYTES: usize = 128;
const MAX_DESCRIPTION_BYTES: usize = 512;
const MAX_ORIGIN_LABEL_BYTES: usize = 512;

/// Immutable validated managed-context catalog.
#[derive(Clone)]
pub struct ContextCatalog {
    manifest: CatalogManifest,
    entries: Arc<BTreeMap<String, CatalogEntry>>,
    material: Arc<BTreeMap<String, Arc<str>>>,
}

impl ContextCatalog {
    pub fn builder() -> ContextCatalogBuilder {
        ContextCatalogBuilder::default()
    }

    /// Start a catalog builder with the reserved shipped definitions loaded.
    pub fn builder_with_builtins() -> ContextCatalogBuilder {
        let mut builder = Self::builder();
        let origin = CatalogOrigin::new(CatalogOriginKind::BuiltIn, "roba built-ins");
        for definition in builtin_definitions() {
            builder
                .add(origin.clone(), ".", definition)
                .expect("shipped catalog definitions must stay valid");
        }
        builder
    }

    /// Construct the intentionally small shipped catalog.
    pub fn builtins() -> Self {
        Self::builder_with_builtins()
            .build()
            .expect("shipped catalog relationships must stay valid")
    }

    pub fn manifest(&self) -> &CatalogManifest {
        &self.manifest
    }

    pub fn entry(&self, id: &str) -> Option<&CatalogEntry> {
        self.entries.get(id)
    }

    /// Return explicitly content-bearing material to trusted host code.
    pub fn material(&self, id: &str) -> Option<&str> {
        self.material.get(id).map(AsRef::as_ref)
    }

    /// Resolve one deterministic agent/skill/prompt selection.
    pub fn select(&self, spec: &CatalogSelectionSpec) -> Result<CatalogSelection, CatalogError> {
        reject_duplicate_ids("selected skill", &spec.skills)?;
        reject_duplicate_ids("selected prompt", &spec.prompts)?;

        let agent = self.require_kind(&spec.agent, CatalogArtifactKind::Agent)?;
        let prompts = spec
            .prompts
            .iter()
            .map(|id| self.require_kind(id, CatalogArtifactKind::Prompt).cloned())
            .collect::<Result<Vec<_>, _>>()?;

        let mut skill_ids = Vec::new();
        let mut seen = BTreeSet::new();
        for id in agent
            .default_skills()
            .iter()
            .chain(&spec.skills)
            .chain(prompts.iter().flat_map(CatalogEntry::required_skills))
        {
            self.require_kind(id, CatalogArtifactKind::Skill)?;
            if seen.insert(id.clone()) {
                skill_ids.push(id.clone());
            }
        }
        let skills = skill_ids
            .iter()
            .map(|id| {
                self.entries
                    .get(id)
                    .expect("validated skill identity must remain in the immutable catalog")
                    .clone()
            })
            .collect::<Vec<_>>();
        let agent = agent.clone();
        let encoded = serde_json::to_vec(&(&agent, &skills, &prompts))
            .expect("catalog selection metadata is serializable");
        Ok(CatalogSelection {
            agent,
            skills,
            prompts,
            fingerprint: fingerprint([encoded.as_slice()]),
        })
    }

    /// Render one reusable prompt with strict declared arguments.
    pub fn render_prompt(
        &self,
        id: &str,
        supplied: &BTreeMap<String, String>,
    ) -> Result<String, CatalogError> {
        let entry = self.require_kind(id, CatalogArtifactKind::Prompt)?;
        let material = self
            .material(id)
            .ok_or_else(|| CatalogError::MaterialUnavailable(id.to_owned()))?;
        let declarations = entry
            .arguments()
            .iter()
            .map(|argument| (argument.name.as_str(), argument))
            .collect::<BTreeMap<_, _>>();
        for name in supplied.keys() {
            if !declarations.contains_key(name.as_str()) {
                return Err(CatalogError::UnknownPromptArgument {
                    prompt: id.to_owned(),
                    argument: name.clone(),
                });
            }
        }

        let mut values = BTreeMap::new();
        for argument in entry.arguments() {
            let value = supplied
                .get(&argument.name)
                .or(argument.default.as_ref())
                .map(String::as_str);
            let value = match value {
                Some(value) => value,
                None if argument.required => {
                    return Err(CatalogError::MissingPromptArgument {
                        prompt: id.to_owned(),
                        argument: argument.name.clone(),
                    });
                }
                None => "",
            };
            if value.len() > MAX_PROMPT_ARGUMENT_BYTES {
                return Err(CatalogError::PromptArgumentTooLarge {
                    prompt: id.to_owned(),
                    argument: argument.name.clone(),
                    bytes: value.len(),
                    max: MAX_PROMPT_ARGUMENT_BYTES,
                });
            }
            values.insert(argument.name.as_str(), value);
        }
        render_template(id, material, &values)
    }

    fn require_kind(
        &self,
        id: &str,
        expected: CatalogArtifactKind,
    ) -> Result<&CatalogEntry, CatalogError> {
        let entry = self
            .entries
            .get(id)
            .ok_or_else(|| CatalogError::UnknownReference(id.to_owned()))?;
        if entry.kind() != expected {
            return Err(CatalogError::WrongArtifactKind {
                id: id.to_owned(),
                expected,
                actual: entry.kind(),
            });
        }
        Ok(entry)
    }
}

impl fmt::Debug for ContextCatalog {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ContextCatalog")
            .field("manifest", &self.manifest)
            .field("retained_material_entries", &self.material.len())
            .finish()
    }
}

/// Incremental fail-closed catalog construction.
#[derive(Debug, Default)]
pub struct ContextCatalogBuilder {
    entries: Vec<CatalogEntry>,
    material: BTreeMap<String, Arc<str>>,
    ids: BTreeSet<String>,
    total_material_bytes: usize,
}

impl ContextCatalogBuilder {
    /// Resolve and add one definition relative to its declaring directory.
    pub fn add(
        &mut self,
        origin: CatalogOrigin,
        base_directory: impl AsRef<Path>,
        definition: CatalogDefinition,
    ) -> Result<(), CatalogError> {
        validate_origin(&origin)?;
        validate_id(definition.id())?;
        if definition.id().starts_with("roba.") && origin.kind != CatalogOriginKind::BuiltIn {
            return Err(CatalogError::ReservedId(definition.id().to_owned()));
        }
        if self.ids.contains(definition.id()) {
            return Err(CatalogError::DuplicateId(definition.id().to_owned()));
        }
        validate_description(definition.id(), definition.description())?;
        validate_definition_lists(&definition)?;

        let (body, source) = resolve_source(
            definition.id(),
            definition.source(),
            base_directory.as_ref(),
        )?;
        let next_total = self.total_material_bytes.checked_add(body.len()).ok_or(
            CatalogError::CatalogTooLarge {
                bytes: usize::MAX,
                max: MAX_CATALOG_BYTES,
            },
        )?;
        if next_total > MAX_CATALOG_BYTES {
            return Err(CatalogError::CatalogTooLarge {
                bytes: next_total,
                max: MAX_CATALOG_BYTES,
            });
        }

        let metadata = serde_json::to_vec(&(&definition, &origin, &source))
            .expect("catalog definition metadata is serializable");
        let fingerprint = fingerprint([metadata.as_slice(), body.as_bytes()]);
        let id = definition.id().to_owned();
        let entry = resolve_entry(definition, origin, source, fingerprint);
        self.ids.insert(id.clone());
        self.entries.push(entry);
        self.material.insert(id, Arc::from(body));
        self.total_material_bytes = next_total;
        Ok(())
    }

    pub fn build(mut self) -> Result<ContextCatalog, CatalogError> {
        self.entries
            .sort_by(|left, right| left.id().cmp(right.id()));
        let entries = self
            .entries
            .iter()
            .map(|entry| (entry.id().to_owned(), entry.clone()))
            .collect::<BTreeMap<_, _>>();
        validate_relationships(&entries, &self.material)?;
        let encoded = serde_json::to_vec(&(CATALOG_SCHEMA_VERSION, &self.entries))
            .expect("catalog manifest fields are serializable");
        let manifest = CatalogManifest {
            schema_version: CATALOG_SCHEMA_VERSION,
            entries: self.entries,
            fingerprint: fingerprint([encoded.as_slice()]),
        };
        Ok(ContextCatalog {
            manifest,
            entries: Arc::new(entries),
            material: Arc::new(self.material),
        })
    }
}

fn resolve_entry(
    definition: CatalogDefinition,
    origin: CatalogOrigin,
    source: CatalogSourceMetadata,
    fingerprint: CatalogFingerprint,
) -> CatalogEntry {
    match definition {
        CatalogDefinition::Agent {
            id,
            description,
            default_skills,
            ..
        } => CatalogEntry::Agent {
            id,
            description,
            origin,
            source,
            fingerprint,
            default_skills,
        },
        CatalogDefinition::Skill {
            id, description, ..
        } => CatalogEntry::Skill {
            id,
            description,
            origin,
            source,
            fingerprint,
        },
        CatalogDefinition::Prompt {
            id,
            description,
            requires,
            arguments,
            ..
        } => CatalogEntry::Prompt {
            id,
            description,
            origin,
            source,
            fingerprint,
            requires,
            arguments,
        },
    }
}

fn validate_origin(origin: &CatalogOrigin) -> Result<(), CatalogError> {
    if origin.label.trim().is_empty() || origin.label.len() > MAX_ORIGIN_LABEL_BYTES {
        return Err(CatalogError::InvalidOriginLabel(origin.label.clone()));
    }
    if origin
        .locator
        .as_ref()
        .is_some_and(|locator| locator.trim().is_empty())
    {
        return Err(CatalogError::InvalidOriginLocator);
    }
    Ok(())
}

fn validate_id(id: &str) -> Result<(), CatalogError> {
    let mut characters = id.chars();
    if id.len() > MAX_ID_BYTES
        || !characters
            .next()
            .is_some_and(|character| character.is_ascii_alphanumeric())
        || !characters.all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-' | '/')
        })
    {
        return Err(CatalogError::InvalidId(id.to_owned()));
    }
    Ok(())
}

fn validate_description(id: &str, description: &str) -> Result<(), CatalogError> {
    if description.trim().is_empty() || description.len() > MAX_DESCRIPTION_BYTES {
        return Err(CatalogError::InvalidDescription(id.to_owned()));
    }
    Ok(())
}

fn validate_definition_lists(definition: &CatalogDefinition) -> Result<(), CatalogError> {
    for reference in definition.references() {
        validate_id(reference)?;
    }
    reject_duplicate_ids("artifact reference", definition.references())?;

    let mut names = BTreeSet::new();
    for argument in definition.arguments() {
        validate_argument(definition.id(), argument)?;
        if !names.insert(argument.name.clone()) {
            return Err(CatalogError::DuplicatePromptArgument {
                prompt: definition.id().to_owned(),
                argument: argument.name.clone(),
            });
        }
    }
    Ok(())
}

fn validate_argument(
    prompt: &str,
    argument: &PromptArgumentDefinition,
) -> Result<(), CatalogError> {
    let mut characters = argument.name.chars();
    if argument.name.len() > 64
        || !characters
            .next()
            .is_some_and(|character| character.is_ascii_alphabetic())
        || !characters
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'))
        || argument.description.trim().is_empty()
        || argument.description.len() > MAX_DESCRIPTION_BYTES
        || argument.required && argument.default.is_some()
    {
        return Err(CatalogError::InvalidPromptArgument {
            prompt: prompt.to_owned(),
            argument: argument.name.clone(),
        });
    }
    if argument
        .default
        .as_ref()
        .is_some_and(|value| value.len() > MAX_PROMPT_ARGUMENT_BYTES)
    {
        return Err(CatalogError::PromptArgumentTooLarge {
            prompt: prompt.to_owned(),
            argument: argument.name.clone(),
            bytes: argument.default.as_ref().map_or(0, String::len),
            max: MAX_PROMPT_ARGUMENT_BYTES,
        });
    }
    Ok(())
}

fn resolve_source(
    id: &str,
    source: &CatalogSource,
    base_directory: &Path,
) -> Result<(String, CatalogSourceMetadata), CatalogError> {
    match source {
        CatalogSource::Inline { content } => {
            validate_body(id, content)?;
            Ok((content.clone(), CatalogSourceMetadata::Inline))
        }
        CatalogSource::MarkdownPath { path } => {
            validate_relative_markdown_path(id, path)?;
            let base = std::fs::canonicalize(base_directory).map_err(|source| {
                CatalogError::ReadSource {
                    id: id.to_owned(),
                    path: base_directory.to_path_buf(),
                    source,
                }
            })?;
            let requested = base.join(path);
            let resolved =
                std::fs::canonicalize(&requested).map_err(|source| CatalogError::ReadSource {
                    id: id.to_owned(),
                    path: requested.clone(),
                    source,
                })?;
            if !resolved.starts_with(&base) {
                return Err(CatalogError::SourceEscapesBase {
                    id: id.to_owned(),
                    path: path.clone(),
                });
            }
            let metadata =
                std::fs::metadata(&resolved).map_err(|source| CatalogError::ReadSource {
                    id: id.to_owned(),
                    path: resolved.clone(),
                    source,
                })?;
            if !metadata.is_file() {
                return Err(CatalogError::SourceNotFile {
                    id: id.to_owned(),
                    path: path.clone(),
                });
            }
            if metadata.len() > MAX_ARTIFACT_BYTES as u64 {
                return Err(CatalogError::ArtifactTooLarge {
                    id: id.to_owned(),
                    bytes: usize::try_from(metadata.len()).unwrap_or(usize::MAX),
                    max: MAX_ARTIFACT_BYTES,
                });
            }
            let body =
                std::fs::read_to_string(&resolved).map_err(|source| CatalogError::ReadSource {
                    id: id.to_owned(),
                    path: resolved,
                    source,
                })?;
            validate_body(id, &body)?;
            Ok((
                body,
                CatalogSourceMetadata::MarkdownPath { path: path.clone() },
            ))
        }
    }
}

fn validate_relative_markdown_path(id: &str, path: &Path) -> Result<(), CatalogError> {
    let valid_components = path
        .components()
        .all(|component| matches!(component, Component::Normal(_) | Component::CurDir));
    let markdown_extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            matches!(extension.to_ascii_lowercase().as_str(), "md" | "markdown")
        });
    if path.as_os_str().is_empty()
        || path.to_str().is_none()
        || !valid_components
        || !markdown_extension
    {
        return Err(CatalogError::InvalidSourcePath {
            id: id.to_owned(),
            path: path.to_path_buf(),
        });
    }
    Ok(())
}

fn validate_body(id: &str, body: &str) -> Result<(), CatalogError> {
    if body.trim().is_empty() {
        return Err(CatalogError::EmptyArtifact(id.to_owned()));
    }
    if body.len() > MAX_ARTIFACT_BYTES {
        return Err(CatalogError::ArtifactTooLarge {
            id: id.to_owned(),
            bytes: body.len(),
            max: MAX_ARTIFACT_BYTES,
        });
    }
    Ok(())
}

fn validate_relationships(
    entries: &BTreeMap<String, CatalogEntry>,
    material: &BTreeMap<String, Arc<str>>,
) -> Result<(), CatalogError> {
    for entry in entries.values() {
        for skill in entry.default_skills().iter().chain(entry.required_skills()) {
            let target = entries
                .get(skill)
                .ok_or_else(|| CatalogError::UnknownReference(skill.clone()))?;
            if target.kind() != CatalogArtifactKind::Skill {
                return Err(CatalogError::WrongArtifactKind {
                    id: skill.clone(),
                    expected: CatalogArtifactKind::Skill,
                    actual: target.kind(),
                });
            }
        }
        if entry.kind() == CatalogArtifactKind::Prompt {
            let body = material
                .get(entry.id())
                .expect("every resolved catalog entry retains material");
            let placeholders = template_placeholders(entry.id(), body)?;
            let arguments = entry
                .arguments()
                .iter()
                .map(|argument| argument.name.as_str())
                .collect::<BTreeSet<_>>();
            for placeholder in &placeholders {
                if !arguments.contains(placeholder.as_str()) {
                    return Err(CatalogError::UndeclaredPromptPlaceholder {
                        prompt: entry.id().to_owned(),
                        argument: placeholder.clone(),
                    });
                }
            }
            for argument in arguments {
                if !placeholders.contains(argument) {
                    return Err(CatalogError::UnusedPromptArgument {
                        prompt: entry.id().to_owned(),
                        argument: argument.to_owned(),
                    });
                }
            }
        }
    }
    Ok(())
}

fn template_placeholders(prompt: &str, template: &str) -> Result<BTreeSet<String>, CatalogError> {
    let mut placeholders = BTreeSet::new();
    let mut remaining = template;
    while let Some(start) = remaining.find("{{") {
        let after_start = &remaining[start + 2..];
        let Some(end) = after_start.find("}}") else {
            return Err(CatalogError::MalformedPromptTemplate(prompt.to_owned()));
        };
        let name = &after_start[..end];
        if !valid_argument_name(name) {
            return Err(CatalogError::MalformedPromptTemplate(prompt.to_owned()));
        }
        placeholders.insert(name.to_owned());
        remaining = &after_start[end + 2..];
    }
    if remaining.contains("}}") {
        return Err(CatalogError::MalformedPromptTemplate(prompt.to_owned()));
    }
    Ok(placeholders)
}

fn valid_argument_name(name: &str) -> bool {
    let mut characters = name.chars();
    name.len() <= 64
        && characters
            .next()
            .is_some_and(|character| character.is_ascii_alphabetic())
        && characters
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'))
}

fn render_template(
    prompt: &str,
    template: &str,
    values: &BTreeMap<&str, &str>,
) -> Result<String, CatalogError> {
    let mut rendered = String::with_capacity(template.len());
    let mut remaining = template;
    while let Some(start) = remaining.find("{{") {
        rendered.push_str(&remaining[..start]);
        let after_start = &remaining[start + 2..];
        let end = after_start
            .find("}}")
            .ok_or_else(|| CatalogError::MalformedPromptTemplate(prompt.to_owned()))?;
        let name = &after_start[..end];
        rendered.push_str(values.get(name).copied().ok_or_else(|| {
            CatalogError::UndeclaredPromptPlaceholder {
                prompt: prompt.to_owned(),
                argument: name.to_owned(),
            }
        })?);
        remaining = &after_start[end + 2..];
        if rendered.len() > MAX_ARTIFACT_BYTES {
            return Err(CatalogError::RenderedPromptTooLarge {
                prompt: prompt.to_owned(),
                bytes: rendered.len(),
                max: MAX_ARTIFACT_BYTES,
            });
        }
    }
    rendered.push_str(remaining);
    if rendered.len() > MAX_ARTIFACT_BYTES {
        return Err(CatalogError::RenderedPromptTooLarge {
            prompt: prompt.to_owned(),
            bytes: rendered.len(),
            max: MAX_ARTIFACT_BYTES,
        });
    }
    Ok(rendered)
}

fn reject_duplicate_ids(label: &str, ids: &[String]) -> Result<(), CatalogError> {
    let mut seen = BTreeSet::new();
    for id in ids {
        if !seen.insert(id) {
            return Err(CatalogError::DuplicateReference {
                label: label.to_owned(),
                id: id.clone(),
            });
        }
    }
    Ok(())
}

fn fingerprint<'a>(parts: impl IntoIterator<Item = &'a [u8]>) -> CatalogFingerprint {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update((part.len() as u64).to_be_bytes());
        hasher.update(part);
    }
    let digest = hasher.finalize();
    let mut encoded = String::with_capacity(7 + digest.len() * 2);
    encoded.push_str("sha256:");
    for byte in digest {
        use fmt::Write as _;
        write!(encoded, "{byte:02x}").expect("writing to String cannot fail");
    }
    CatalogFingerprint(encoded)
}

/// Fail-closed catalog construction, selection, and rendering errors.
#[derive(Debug, Error)]
pub enum CatalogError {
    #[error("invalid catalog artifact id `{0}`")]
    InvalidId(String),
    #[error("catalog artifact id `{0}` is reserved for Roba built-ins")]
    ReservedId(String),
    #[error("duplicate catalog artifact id `{0}`")]
    DuplicateId(String),
    #[error("catalog artifact `{0}` has an empty or oversized description")]
    InvalidDescription(String),
    #[error("catalog origin has an empty or oversized label `{0}`")]
    InvalidOriginLabel(String),
    #[error("catalog origin locator must not be empty")]
    InvalidOriginLocator,
    #[error("catalog artifact `{id}` has invalid Markdown source path `{}`", path.display())]
    InvalidSourcePath { id: String, path: PathBuf },
    #[error("catalog artifact `{id}` source path `{}` escapes its declaring directory", path.display())]
    SourceEscapesBase { id: String, path: PathBuf },
    #[error("catalog artifact `{id}` source `{}` is not a file", path.display())]
    SourceNotFile { id: String, path: PathBuf },
    #[error("reading catalog artifact `{id}` source `{}`", path.display())]
    ReadSource {
        id: String,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("catalog artifact `{0}` must not be empty")]
    EmptyArtifact(String),
    #[error("catalog artifact `{id}` is {bytes} bytes; maximum is {max}")]
    ArtifactTooLarge {
        id: String,
        bytes: usize,
        max: usize,
    },
    #[error("catalog material is {bytes} bytes; maximum is {max}")]
    CatalogTooLarge { bytes: usize, max: usize },
    #[error("duplicate {label} `{id}`")]
    DuplicateReference { label: String, id: String },
    #[error("catalog reference `{0}` does not exist")]
    UnknownReference(String),
    #[error("catalog artifact `{id}` has kind {actual:?}; expected {expected:?}")]
    WrongArtifactKind {
        id: String,
        expected: CatalogArtifactKind,
        actual: CatalogArtifactKind,
    },
    #[error("prompt `{prompt}` has invalid argument `{argument}`")]
    InvalidPromptArgument { prompt: String, argument: String },
    #[error("prompt `{prompt}` declares duplicate argument `{argument}`")]
    DuplicatePromptArgument { prompt: String, argument: String },
    #[error("prompt `{0}` template is malformed")]
    MalformedPromptTemplate(String),
    #[error("prompt `{prompt}` uses undeclared placeholder `{argument}`")]
    UndeclaredPromptPlaceholder { prompt: String, argument: String },
    #[error("prompt `{prompt}` declares unused argument `{argument}`")]
    UnusedPromptArgument { prompt: String, argument: String },
    #[error("prompt `{prompt}` does not declare argument `{argument}`")]
    UnknownPromptArgument { prompt: String, argument: String },
    #[error("prompt `{prompt}` requires argument `{argument}`")]
    MissingPromptArgument { prompt: String, argument: String },
    #[error("prompt `{prompt}` argument `{argument}` is {bytes} bytes; maximum is {max}")]
    PromptArgumentTooLarge {
        prompt: String,
        argument: String,
        bytes: usize,
        max: usize,
    },
    #[error("rendered prompt `{prompt}` is {bytes} bytes; maximum is {max}")]
    RenderedPromptTooLarge {
        prompt: String,
        bytes: usize,
        max: usize,
    },
    #[error("catalog material for `{0}` is unavailable")]
    MaterialUnavailable(String),
}
