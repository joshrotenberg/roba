//! Typed, bounded managed-context catalogs for Roba.
//!
//! This crate owns context *content and selection*, not delivery. It resolves
//! host-configured agent, skill, and prompt definitions into one immutable
//! catalog with content-free provenance and fingerprints. `roba-mcp` remains
//! responsible for role-scoped projection, operation generations, provider
//! acquisition, and read evidence.
//!
//! Definitions may carry bounded inline material or a repository-local
//! Markdown path. Construction resolves and validates all material before the
//! immutable catalog is exposed. Selection resolves exactly one agent plus
//! explicit and transitive skills, while prompt rendering accepts only
//! declared bounded arguments. Public manifests and debug output deliberately
//! omit retained bodies so hosts choose where material may be projected.

#![forbid(unsafe_code)]

mod builtins;
mod catalog;
mod types;

pub use builtins::builtin_definitions;
pub use catalog::{CatalogError, ContextCatalog, ContextCatalogBuilder};
pub use types::{
    CATALOG_SCHEMA_VERSION, CatalogArtifactKind, CatalogDefinition, CatalogEntry,
    CatalogFingerprint, CatalogManifest, CatalogOrigin, CatalogOriginKind, CatalogSelection,
    CatalogSelectionSpec, CatalogSource, CatalogSourceMetadata, MAX_ARTIFACT_BYTES,
    MAX_CATALOG_BYTES, MAX_PROMPT_ARGUMENT_BYTES, PromptArgumentDefinition,
};
