//! Managed agent, skill, and prompt projection through the MCP host.

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::sync::Arc;

use roba_context::{
    CatalogArtifactKind, CatalogEntry, CatalogManifest, CatalogOrigin, CatalogOriginKind,
    CatalogSelection, CatalogSelectionSpec, ContextCatalog,
};
use serde::{Deserialize, Serialize};
use tower_mcp::{
    Content, GetPromptResult, McpRouter, PromptArgument, PromptBuilder, PromptMessage, PromptRole,
    ReadResourceResult, ResourceBuilder, ResourceTemplateBuilder,
};

use crate::{
    AGENT_CONTEXT_ENTRY_TEMPLATE, AgentExtension, ContextAudience, ContextDelivery,
    ContextEntrySpec, ContextFreshness, ContextKind, ContextOrigin, ContextOriginKind,
    ContextPhase, ContextPrecedence, ContextScope, ContextSensitivity,
};

/// Stable extension identity for the host-managed context catalog.
pub const MANAGED_CONTEXT_EXTENSION_NAME: &str = "managed context";
/// Content-free inventory and effective managed-context selection.
pub const MANAGED_CONTEXT_CATALOG_URI: &str = "roba://context/catalog";
/// Explicitly content-bearing operator read for one catalog artifact.
pub const MANAGED_CONTEXT_ARTIFACT_TEMPLATE: &str = "roba://context/catalog/artifact{?id}";

/// Content-free catalog inventory and optional effective selection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManagedContextCatalogSnapshot {
    pub catalog: CatalogManifest,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selection: Option<CatalogSelection>,
}

/// Explicitly content-bearing operator view of one managed artifact.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManagedContextArtifact {
    pub entry: CatalogEntry,
    pub content: String,
}

impl fmt::Debug for ManagedContextArtifact {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ManagedContextArtifact")
            .field("entry", &self.entry)
            .field("content", &"[REDACTED]")
            .finish()
    }
}

/// Failure to compile one resolved catalog into an MCP contribution.
#[derive(Debug)]
pub enum ManagedContextError {
    Catalog(roba_context::CatalogError),
    SelectionMismatch,
}

impl fmt::Display for ManagedContextError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Catalog(error) => write!(formatter, "invalid managed context selection: {error}"),
            Self::SelectionMismatch => formatter
                .write_str("managed context selection does not belong to the supplied catalog"),
        }
    }
}

impl std::error::Error for ManagedContextError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Catalog(error) => Some(error),
            Self::SelectionMismatch => None,
        }
    }
}

impl From<roba_context::CatalogError> for ManagedContextError {
    fn from(error: roba_context::CatalogError) -> Self {
        Self::Catalog(error)
    }
}

/// Compile one resolved catalog and optional selection through the ordinary
/// extension path.
///
/// The control fragment publishes content-free inventory, explicit operator
/// artifact reads, and selected reusable prompts. The provider fragment is
/// empty. Selected agent and skill bodies instead join the immutable context
/// plan and are available only through its generation-fenced provider tools.
pub fn managed_context_extension(
    catalog: ContextCatalog,
    selection: Option<CatalogSelection>,
) -> Result<AgentExtension, ManagedContextError> {
    validate_selection(&catalog, selection.as_ref())?;
    let catalog = Arc::new(catalog);
    let snapshot = ManagedContextCatalogSnapshot {
        catalog: catalog.manifest().clone(),
        selection: selection.clone(),
    };
    let mut control = catalog_router(Arc::clone(&catalog), snapshot);
    if let Some(selection) = &selection {
        for prompt in &selection.prompts {
            control = control.prompt(catalog_prompt(Arc::clone(&catalog), prompt));
        }
    }

    let mut extension =
        AgentExtension::new(MANAGED_CONTEXT_EXTENSION_NAME, control, McpRouter::new());
    if let Some(selection) = selection {
        extension = extension.with_inline_context(
            selected_context_spec(&selection.agent, true),
            selected_material(&catalog, &selection.agent),
        );
        for skill in &selection.skills {
            extension = extension.with_inline_context(
                selected_context_spec(skill, false),
                selected_material(&catalog, skill),
            );
        }
    }
    Ok(extension)
}

fn validate_selection(
    catalog: &ContextCatalog,
    selection: Option<&CatalogSelection>,
) -> Result<(), ManagedContextError> {
    let Some(selection) = selection else {
        return Ok(());
    };
    let resolved = catalog.select(&CatalogSelectionSpec {
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
    })?;
    if &resolved != selection {
        return Err(ManagedContextError::SelectionMismatch);
    }
    Ok(())
}

fn catalog_router(
    catalog: Arc<ContextCatalog>,
    snapshot: ManagedContextCatalogSnapshot,
) -> McpRouter {
    let inventory = ResourceBuilder::new(MANAGED_CONTEXT_CATALOG_URI)
        .name("Roba managed context catalog")
        .description("Content-free managed agent, skill, and prompt inventory and selection.")
        .mime_type("application/json")
        .handler(move || {
            let snapshot = snapshot.clone();
            async move {
                serialize_resource(
                    MANAGED_CONTEXT_CATALOG_URI,
                    &snapshot,
                    "managed context catalog",
                )
            }
        })
        .build();

    let artifact = ResourceTemplateBuilder::new(MANAGED_CONTEXT_ARTIFACT_TEMPLATE)
        .name("Roba managed context artifact")
        .description("Read one explicit managed agent, skill, or prompt body as the operator.")
        .mime_type("application/json")
        .argument("id", Some("Exact catalog artifact ID."), true)
        .handler(move |uri: String, variables: HashMap<String, String>| {
            let catalog = Arc::clone(&catalog);
            async move {
                let id = variables
                    .get("id")
                    .filter(|id| !id.trim().is_empty())
                    .ok_or_else(|| tower_mcp::Error::invalid_params("missing artifact id"))?;
                let entry = catalog.entry(id).cloned().ok_or_else(|| {
                    tower_mcp::Error::invalid_params(format!(
                        "managed context artifact `{id}` does not exist"
                    ))
                })?;
                let content = catalog.material(id).ok_or_else(|| {
                    tower_mcp::Error::invalid_params(format!(
                        "managed context artifact `{id}` has no readable content"
                    ))
                })?;
                serialize_resource(
                    &uri,
                    &ManagedContextArtifact {
                        entry,
                        content: content.to_owned(),
                    },
                    "managed context artifact",
                )
            }
        });

    McpRouter::new()
        .resource(inventory)
        .resource_template(artifact)
}

fn catalog_prompt(catalog: Arc<ContextCatalog>, entry: &CatalogEntry) -> tower_mcp::Prompt {
    let CatalogEntry::Prompt {
        id,
        description,
        arguments,
        ..
    } = entry
    else {
        panic!("catalog selection prompt must retain its prompt kind")
    };
    let mut builder = PromptBuilder::new(id).description(description);
    for argument in arguments {
        builder = builder.argument(PromptArgument {
            name: argument.name.clone(),
            description: Some(argument.description.clone()),
            required: argument.required,
        });
    }
    let id = id.clone();
    let description = description.clone();
    builder
        .handler(move |arguments: HashMap<String, String>| {
            let catalog = Arc::clone(&catalog);
            let id = id.clone();
            let description = description.clone();
            async move {
                let rendered = catalog
                    .render_prompt(&id, &arguments.into_iter().collect::<BTreeMap<_, _>>())
                    .map_err(|error| tower_mcp::Error::invalid_params(error.to_string()))?;
                Ok(GetPromptResult {
                    description: Some(description),
                    messages: vec![PromptMessage {
                        role: PromptRole::User,
                        content: Content::text(rendered),
                        meta: None,
                    }],
                    meta: None,
                })
            }
        })
        .build()
}

fn selected_context_spec(entry: &CatalogEntry, required: bool) -> ContextEntrySpec {
    let (kind, origin) = match entry {
        CatalogEntry::Agent { origin, .. } => (ContextKind::Instruction, origin),
        CatalogEntry::Skill { origin, .. } => (ContextKind::Reference, origin),
        CatalogEntry::Prompt { .. } => {
            panic!("prompts are directives and must not become standing context entries")
        }
    };
    ContextEntrySpec::new(
        entry.id(),
        kind,
        context_origin(origin),
        ContextPhase::Bootstrap,
        ContextScope::Agent,
        ContextDelivery::McpResource {
            uri: AGENT_CONTEXT_ENTRY_TEMPLATE.to_owned(),
        },
    )
    .audience(ContextAudience::Provider)
    .precedence(ContextPrecedence::Host)
    .freshness(if entry.kind() == CatalogArtifactKind::Agent {
        ContextFreshness::FreshSession
    } else {
        ContextFreshness::Generation
    })
    .sensitivity(ContextSensitivity::Redacted)
    .required(required)
}

fn context_origin(origin: &CatalogOrigin) -> ContextOrigin {
    let kind = match origin.kind {
        CatalogOriginKind::BuiltIn => ContextOriginKind::Roba,
        CatalogOriginKind::Project => ContextOriginKind::Workspace,
        CatalogOriginKind::Parent => ContextOriginKind::ParentAgent,
        CatalogOriginKind::User | CatalogOriginKind::Explicit => ContextOriginKind::External,
    };
    let mut mapped = ContextOrigin::new(kind, origin.label.clone());
    if let Some(locator) = &origin.locator {
        mapped = mapped.with_locator(locator.clone());
    }
    mapped
}

fn selected_material(catalog: &ContextCatalog, entry: &CatalogEntry) -> String {
    catalog
        .material(entry.id())
        .expect("validated selected catalog entries retain material")
        .to_owned()
}

fn serialize_resource<T: Serialize>(
    uri: &str,
    value: &T,
    label: &str,
) -> tower_mcp::Result<ReadResourceResult> {
    let json = serde_json::to_string_pretty(value).map_err(|error| {
        tower_mcp::Error::internal(format!("failed to serialize {label}: {error}"))
    })?;
    Ok(ReadResourceResult::text_with_mime(
        uri,
        json,
        "application/json",
    ))
}
