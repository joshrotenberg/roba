use std::fmt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Schema version of the content-free catalog manifest.
pub const CATALOG_SCHEMA_VERSION: u32 = 1;
/// Maximum UTF-8 bytes retained for one artifact body.
pub const MAX_ARTIFACT_BYTES: usize = 64 * 1024;
/// Maximum UTF-8 bytes retained across one catalog.
pub const MAX_CATALOG_BYTES: usize = 512 * 1024;
/// Maximum UTF-8 bytes accepted for one rendered prompt argument.
pub const MAX_PROMPT_ARGUMENT_BYTES: usize = 16 * 1024;

/// Semantic kind of one managed-context artifact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CatalogArtifactKind {
    Agent,
    Skill,
    Prompt,
}

/// Configuration layer that supplied one definition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CatalogOriginKind {
    BuiltIn,
    User,
    Project,
    Explicit,
    Parent,
}

/// Content-free provenance for one catalog definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogOrigin {
    pub kind: CatalogOriginKind,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locator: Option<String>,
}

impl CatalogOrigin {
    pub fn new(kind: CatalogOriginKind, label: impl Into<String>) -> Self {
        Self {
            kind,
            label: label.into(),
            locator: None,
        }
    }

    pub fn with_locator(mut self, locator: impl Into<String>) -> Self {
        self.locator = Some(locator.into());
        self
    }
}

/// Configured source of one artifact body.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CatalogSource {
    Inline { content: String },
    MarkdownPath { path: PathBuf },
}

impl fmt::Debug for CatalogSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Inline { content } => formatter
                .debug_struct("Inline")
                .field("bytes", &content.len())
                .finish(),
            Self::MarkdownPath { path } => formatter
                .debug_struct("MarkdownPath")
                .field("path", path)
                .finish(),
        }
    }
}

/// Declared input accepted by one reusable prompt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PromptArgumentDefinition {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
}

/// Strict unresolved agent, skill, or prompt definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CatalogDefinition {
    Agent {
        id: String,
        description: String,
        source: CatalogSource,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        default_skills: Vec<String>,
    },
    Skill {
        id: String,
        description: String,
        source: CatalogSource,
    },
    Prompt {
        id: String,
        description: String,
        source: CatalogSource,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        requires: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        arguments: Vec<PromptArgumentDefinition>,
    },
}

impl CatalogDefinition {
    pub fn id(&self) -> &str {
        match self {
            Self::Agent { id, .. } | Self::Skill { id, .. } | Self::Prompt { id, .. } => id,
        }
    }

    pub fn kind(&self) -> CatalogArtifactKind {
        match self {
            Self::Agent { .. } => CatalogArtifactKind::Agent,
            Self::Skill { .. } => CatalogArtifactKind::Skill,
            Self::Prompt { .. } => CatalogArtifactKind::Prompt,
        }
    }

    pub(crate) fn description(&self) -> &str {
        match self {
            Self::Agent { description, .. }
            | Self::Skill { description, .. }
            | Self::Prompt { description, .. } => description,
        }
    }

    pub(crate) fn source(&self) -> &CatalogSource {
        match self {
            Self::Agent { source, .. }
            | Self::Skill { source, .. }
            | Self::Prompt { source, .. } => source,
        }
    }

    pub(crate) fn references(&self) -> &[String] {
        match self {
            Self::Agent { default_skills, .. } => default_skills,
            Self::Prompt { requires, .. } => requires,
            Self::Skill { .. } => &[],
        }
    }

    pub(crate) fn arguments(&self) -> &[PromptArgumentDefinition] {
        match self {
            Self::Prompt { arguments, .. } => arguments,
            Self::Agent { .. } | Self::Skill { .. } => &[],
        }
    }
}

/// Resolved source metadata safe to publish without its body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CatalogSourceMetadata {
    Inline,
    MarkdownPath { path: PathBuf },
}

/// Stable digest of validated metadata and material.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CatalogFingerprint(pub(crate) String);

impl CatalogFingerprint {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// One content-free resolved catalog entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CatalogEntry {
    Agent {
        id: String,
        description: String,
        origin: CatalogOrigin,
        source: CatalogSourceMetadata,
        fingerprint: CatalogFingerprint,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        default_skills: Vec<String>,
    },
    Skill {
        id: String,
        description: String,
        origin: CatalogOrigin,
        source: CatalogSourceMetadata,
        fingerprint: CatalogFingerprint,
    },
    Prompt {
        id: String,
        description: String,
        origin: CatalogOrigin,
        source: CatalogSourceMetadata,
        fingerprint: CatalogFingerprint,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        requires: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        arguments: Vec<PromptArgumentDefinition>,
    },
}

impl CatalogEntry {
    pub fn id(&self) -> &str {
        match self {
            Self::Agent { id, .. } | Self::Skill { id, .. } | Self::Prompt { id, .. } => id,
        }
    }

    pub fn kind(&self) -> CatalogArtifactKind {
        match self {
            Self::Agent { .. } => CatalogArtifactKind::Agent,
            Self::Skill { .. } => CatalogArtifactKind::Skill,
            Self::Prompt { .. } => CatalogArtifactKind::Prompt,
        }
    }

    pub fn fingerprint(&self) -> &CatalogFingerprint {
        match self {
            Self::Agent { fingerprint, .. }
            | Self::Skill { fingerprint, .. }
            | Self::Prompt { fingerprint, .. } => fingerprint,
        }
    }

    pub(crate) fn default_skills(&self) -> &[String] {
        match self {
            Self::Agent { default_skills, .. } => default_skills,
            Self::Skill { .. } | Self::Prompt { .. } => &[],
        }
    }

    pub(crate) fn required_skills(&self) -> &[String] {
        match self {
            Self::Prompt { requires, .. } => requires,
            Self::Agent { .. } | Self::Skill { .. } => &[],
        }
    }

    pub(crate) fn arguments(&self) -> &[PromptArgumentDefinition] {
        match self {
            Self::Prompt { arguments, .. } => arguments,
            Self::Agent { .. } | Self::Skill { .. } => &[],
        }
    }
}

/// Serializable content-free catalog inventory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogManifest {
    pub schema_version: u32,
    pub entries: Vec<CatalogEntry>,
    pub fingerprint: CatalogFingerprint,
}

/// Requested agent role, skills, and reusable prompts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogSelectionSpec {
    pub agent: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub prompts: Vec<String>,
}

/// Deterministic, content-free effective selection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogSelection {
    pub agent: CatalogEntry,
    pub skills: Vec<CatalogEntry>,
    pub prompts: Vec<CatalogEntry>,
    pub fingerprint: CatalogFingerprint,
}
