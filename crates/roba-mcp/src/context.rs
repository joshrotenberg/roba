//! Provider-neutral context planning above the finite run core.
//!
//! A context plan records provenance and delivery intent without serializing
//! prompt material. The finite [`roba_core::RunSpec`] remains the executable
//! provider contract; this module supplies the host-level evidence needed to
//! explain how that intent was assembled and, in later layers, expose it over
//! MCP without copying every context body into every turn.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use roba_core::RunSpec;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tower_mcp::schemars::{self, JsonSchema};

use crate::OperationId;

/// Schema version of the public context manifest.
pub const CONTEXT_MANIFEST_SCHEMA_VERSION: u32 = 1;

/// How much provider-native ambient discovery the host intends to retain.
///
/// This is intent, not a claim that an adapter can enforce the requested
/// isolation. Provider capability validation must fail closed before a
/// controlled or hermetic mode is advertised as effective.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AmbientContextPolicy {
    /// Preserve provider-native user and workspace discovery.
    Ambient,
    /// Inventory ambient sources and add an authoritative Roba bootstrap.
    Controlled,
    /// Permit only context declared by the plan.
    Hermetic,
}

/// Semantic role of one context entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ContextKind {
    Instruction,
    Reference,
    Authority,
    Session,
}

/// Lifecycle phase in which an entry exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ContextPhase {
    ProviderBaseline,
    ProviderAmbient,
    Bootstrap,
    Session,
    Turn,
    Live,
}

/// Logical reach of one context entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ContextScope {
    User,
    Workspace,
    Agent,
    Operation,
    Turn,
}

/// How an entry reaches, or becomes available to, the provider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ContextDelivery {
    /// Provider-native discovery outside Roba's direct control.
    ProviderAmbient,
    /// Existing adapter-specific prompt delivery.
    ProviderAdapter,
    /// A minimal launch bootstrap needed before MCP can be used.
    Bootstrap,
    /// Opaque knowledge retained by a provider session.
    Session,
    /// Context available as an MCP resource.
    McpResource { uri: String },
    /// Context computed or retrieved by an MCP tool.
    McpTool { name: String },
}

/// When an entry must be refreshed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ContextFreshness {
    FreshSession,
    EveryTurn,
    Generation,
    Dynamic,
}

/// Disclosure policy for context material and fingerprints.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ContextSensitivity {
    /// Material may be displayed by an explicitly content-bearing surface.
    Public,
    /// Material is hidden from manifests and diagnostics, but may be read
    /// through an explicitly content-bearing, role-scoped surface.
    Redacted,
    /// Neither material nor a content-derived fingerprint may be exposed by
    /// the generic context contract.
    Secret,
}

/// Broad provenance category for one context entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ContextOriginKind {
    Provider,
    Roba,
    Cli,
    RunSpec,
    Workspace,
    Extension,
    ParentAgent,
    External,
}

/// Human-inspectable provenance that does not contain context material.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ContextOrigin {
    pub kind: ContextOriginKind,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locator: Option<String>,
}

impl ContextOrigin {
    pub fn new(kind: ContextOriginKind, label: impl Into<String>) -> Self {
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

/// Stable digest safe to include in a context manifest.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct ContextFingerprint(String);

impl ContextFingerprint {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Metadata supplied before material is added to a plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextEntrySpec {
    pub id: String,
    pub kind: ContextKind,
    pub origin: ContextOrigin,
    pub phase: ContextPhase,
    pub scope: ContextScope,
    pub delivery: ContextDelivery,
    pub freshness: ContextFreshness,
    pub sensitivity: ContextSensitivity,
    pub required: bool,
}

impl ContextEntrySpec {
    pub fn new(
        id: impl Into<String>,
        kind: ContextKind,
        origin: ContextOrigin,
        phase: ContextPhase,
        scope: ContextScope,
        delivery: ContextDelivery,
    ) -> Self {
        Self {
            id: id.into(),
            kind,
            origin,
            phase,
            scope,
            delivery,
            freshness: ContextFreshness::Generation,
            sensitivity: ContextSensitivity::Redacted,
            required: false,
        }
    }

    pub fn freshness(mut self, freshness: ContextFreshness) -> Self {
        self.freshness = freshness;
        self
    }

    pub fn sensitivity(mut self, sensitivity: ContextSensitivity) -> Self {
        self.sensitivity = sensitivity;
        self
    }

    pub fn required(mut self, required: bool) -> Self {
        self.required = required;
        self
    }
}

/// One content-free entry in the public context manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ContextEntry {
    pub id: String,
    pub kind: ContextKind,
    pub origin: ContextOrigin,
    pub phase: ContextPhase,
    pub scope: ContextScope,
    pub delivery: ContextDelivery,
    pub freshness: ContextFreshness,
    pub sensitivity: ContextSensitivity,
    pub required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<ContextFingerprint>,
    pub material_available: bool,
}

/// Serializable, content-free description of the effective context plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ContextManifest {
    pub schema_version: u32,
    pub generation: u64,
    pub ambient_policy: AmbientContextPolicy,
    pub entries: Vec<ContextEntry>,
    pub fingerprint: ContextFingerprint,
}

/// Aggregate timestamps and count for one mechanically observed MCP read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ContextReadStats {
    pub first_read_at_unix_ms: Option<u64>,
    pub last_read_at_unix_ms: Option<u64>,
    pub read_count: u64,
}

impl ContextReadStats {
    fn record(&mut self, observed_at_unix_ms: Option<u64>) {
        if self.read_count == 0 {
            self.first_read_at_unix_ms = observed_at_unix_ms;
        }
        self.last_read_at_unix_ms = observed_at_unix_ms;
        self.read_count = self.read_count.saturating_add(1);
    }
}

/// Mechanical read evidence for one context entry in one operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ContextEntryRead {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<ContextFingerprint>,
    pub stats: ContextReadStats,
}

/// Generation-fenced context reads observed for one exact provider operation.
///
/// A read proves that the provider-side MCP client requested the resource. It
/// does not prove that the model understood, acknowledged, or followed it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ContextReadEvidence {
    pub operation_id: OperationId,
    pub generation: u64,
    pub manifest_fingerprint: ContextFingerprint,
    pub manifest: ContextReadStats,
    pub entries: Vec<ContextEntryRead>,
}

/// Content-free Roba-declared context plus current or latest read evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ContextSnapshot {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<OperationId>,
    pub manifest: ContextManifest,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_evidence: Option<ContextReadEvidence>,
}

/// Explicitly content-bearing response for one context entry.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ContextContent {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<OperationId>,
    pub generation: u64,
    pub entry: ContextEntry,
    pub content: String,
}

impl fmt::Debug for ContextContent {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ContextContent")
            .field("operation_id", &self.operation_id)
            .field("generation", &self.generation)
            .field("entry", &self.entry)
            .field("content", &"[REDACTED]")
            .finish()
    }
}

/// Host-owned plan retaining material separately from its public manifest.
#[derive(Clone)]
pub struct ContextPlan {
    manifest: ContextManifest,
    material: Arc<BTreeMap<String, Arc<str>>>,
}

impl ContextPlan {
    pub fn builder(ambient_policy: AmbientContextPolicy) -> ContextPlanBuilder {
        ContextPlanBuilder::new(ambient_policy)
    }

    /// Describe the explicit context already present in a finite run spec.
    ///
    /// This is a characterization of today's adapter contract: all three
    /// vectors are delivered again on every provider turn. It does not add or
    /// remove provider-native ambient context.
    pub fn from_run_spec(spec: &RunSpec) -> Self {
        let mut builder = Self::builder(AmbientContextPolicy::Ambient);
        for (index, text) in spec.agent.instructions.iter().enumerate() {
            builder
                .add_inline(
                    run_spec_entry(
                        format!("agent.instruction.{}", index + 1),
                        ContextKind::Instruction,
                        format!("agent.instructions[{index}]"),
                        ContextPhase::Bootstrap,
                        ContextScope::Agent,
                    ),
                    text,
                )
                .expect("generated RunSpec context ids are unique and valid");
        }
        for (index, text) in spec.context.project.iter().enumerate() {
            builder
                .add_inline(
                    run_spec_entry(
                        format!("project.context.{}", index + 1),
                        ContextKind::Reference,
                        format!("context.project[{index}]"),
                        ContextPhase::Bootstrap,
                        ContextScope::Workspace,
                    ),
                    text,
                )
                .expect("generated RunSpec context ids are unique and valid");
        }
        for (index, text) in spec.context.run.iter().enumerate() {
            builder
                .add_inline(
                    run_spec_entry(
                        format!("run.context.{}", index + 1),
                        ContextKind::Reference,
                        format!("context.run[{index}]"),
                        ContextPhase::Turn,
                        ContextScope::Operation,
                    ),
                    text,
                )
                .expect("generated RunSpec context ids are unique and valid");
        }
        builder.build()
    }

    pub fn manifest(&self) -> &ContextManifest {
        &self.manifest
    }

    /// Return retained material to trusted host code.
    pub fn material(&self, id: &str) -> Option<&str> {
        self.material.get(id).map(AsRef::as_ref)
    }

    pub(crate) fn content(
        &self,
        operation_id: Option<OperationId>,
        id: &str,
        generation: u64,
    ) -> Result<ContextContent, ContextReadError> {
        if generation != self.manifest.generation {
            return Err(ContextReadError::GenerationMismatch {
                requested: generation,
                current: self.manifest.generation,
            });
        }
        let entry = self
            .manifest
            .entries
            .iter()
            .find(|entry| entry.id == id)
            .cloned()
            .ok_or_else(|| ContextReadError::EntryNotFound(id.to_owned()))?;
        if entry.sensitivity == ContextSensitivity::Secret || !entry.material_available {
            return Err(ContextReadError::ContentUnavailable(id.to_owned()));
        }
        let content = self
            .material(id)
            .ok_or_else(|| ContextReadError::ContentUnavailable(id.to_owned()))?
            .to_owned();
        Ok(ContextContent {
            operation_id,
            generation,
            entry,
            content,
        })
    }
}

impl fmt::Debug for ContextPlan {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ContextPlan")
            .field("manifest", &self.manifest)
            .field("retained_material_entries", &self.material.len())
            .finish()
    }
}

fn run_spec_entry(
    id: String,
    kind: ContextKind,
    label: String,
    phase: ContextPhase,
    scope: ContextScope,
) -> ContextEntrySpec {
    ContextEntrySpec::new(
        id,
        kind,
        ContextOrigin::new(ContextOriginKind::RunSpec, label),
        phase,
        scope,
        ContextDelivery::ProviderAdapter,
    )
    .freshness(ContextFreshness::EveryTurn)
    .sensitivity(ContextSensitivity::Redacted)
    .required(true)
}

/// Incremental, fail-closed context-plan construction.
pub struct ContextPlanBuilder {
    generation: u64,
    ambient_policy: AmbientContextPolicy,
    entries: Vec<ContextEntry>,
    material: BTreeMap<String, Arc<str>>,
    ids: BTreeSet<String>,
}

impl ContextPlanBuilder {
    pub fn new(ambient_policy: AmbientContextPolicy) -> Self {
        Self {
            generation: 1,
            ambient_policy,
            entries: Vec::new(),
            material: BTreeMap::new(),
            ids: BTreeSet::new(),
        }
    }

    pub fn generation(mut self, generation: u64) -> Self {
        self.generation = generation;
        self
    }

    pub fn add_inline(
        &mut self,
        spec: ContextEntrySpec,
        material: impl AsRef<str>,
    ) -> Result<(), ContextPlanError> {
        let material = material.as_ref();
        let material_available = spec.sensitivity != ContextSensitivity::Secret;
        let fingerprint = match spec.sensitivity {
            ContextSensitivity::Public | ContextSensitivity::Redacted => {
                Some(fingerprint([material.as_bytes()]))
            }
            ContextSensitivity::Secret => None,
        };
        let id = self.add_entry(spec, fingerprint, material_available)?;
        self.material.insert(id, Arc::from(material));
        Ok(())
    }

    pub fn add_available(&mut self, spec: ContextEntrySpec) -> Result<(), ContextPlanError> {
        self.add_entry(spec, None, false).map(|_| ())
    }

    fn add_entry(
        &mut self,
        spec: ContextEntrySpec,
        fingerprint: Option<ContextFingerprint>,
        material_available: bool,
    ) -> Result<String, ContextPlanError> {
        if !valid_context_id(&spec.id) {
            return Err(ContextPlanError::InvalidId(spec.id));
        }
        if !self.ids.insert(spec.id.clone()) {
            return Err(ContextPlanError::DuplicateId(spec.id));
        }
        let id = spec.id.clone();
        self.entries.push(ContextEntry {
            id: spec.id,
            kind: spec.kind,
            origin: spec.origin,
            phase: spec.phase,
            scope: spec.scope,
            delivery: spec.delivery,
            freshness: spec.freshness,
            sensitivity: spec.sensitivity,
            required: spec.required,
            fingerprint,
            material_available,
        });
        Ok(id)
    }

    pub fn build(self) -> ContextPlan {
        let encoded = serde_json::to_vec(&(
            CONTEXT_MANIFEST_SCHEMA_VERSION,
            self.generation,
            self.ambient_policy,
            &self.entries,
        ))
        .expect("context manifest fields are serializable");
        let manifest = ContextManifest {
            schema_version: CONTEXT_MANIFEST_SCHEMA_VERSION,
            generation: self.generation,
            ambient_policy: self.ambient_policy,
            entries: self.entries,
            fingerprint: fingerprint([encoded.as_slice()]),
        };
        ContextPlan {
            manifest,
            material: Arc::new(self.material),
        }
    }
}

/// Invalid context-plan construction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContextPlanError {
    InvalidId(String),
    DuplicateId(String),
}

impl fmt::Display for ContextPlanError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidId(id) => write!(formatter, "invalid context entry id `{id}`"),
            Self::DuplicateId(id) => write!(formatter, "duplicate context entry id `{id}`"),
        }
    }
}

impl std::error::Error for ContextPlanError {}

/// Failure to resolve one generation-fenced context content resource.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ContextReadError {
    OperationUnavailable(OperationId),
    GenerationMismatch { requested: u64, current: u64 },
    EntryNotFound(String),
    ContentUnavailable(String),
}

impl fmt::Display for ContextReadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OperationUnavailable(operation_id) => write!(
                formatter,
                "context for operation {} is unavailable or has expired",
                operation_id.get()
            ),
            Self::GenerationMismatch { requested, current } => write!(
                formatter,
                "context generation {requested} is stale or unknown; current generation is {current}"
            ),
            Self::EntryNotFound(id) => write!(formatter, "context entry `{id}` does not exist"),
            Self::ContentUnavailable(id) => {
                write!(formatter, "context entry `{id}` has no readable content")
            }
        }
    }
}

/// Mutable read evidence for one exact provider operation.
pub(crate) struct OperationContext {
    plan: ContextPlan,
    evidence: Mutex<ContextReadEvidence>,
}

impl OperationContext {
    pub(crate) fn new(operation_id: OperationId, plan: ContextPlan) -> Self {
        Self {
            evidence: Mutex::new(ContextReadEvidence {
                operation_id,
                generation: plan.manifest.generation,
                manifest_fingerprint: plan.manifest.fingerprint.clone(),
                manifest: ContextReadStats {
                    first_read_at_unix_ms: None,
                    last_read_at_unix_ms: None,
                    read_count: 0,
                },
                entries: Vec::new(),
            }),
            plan,
        }
    }

    pub(crate) fn manifest_read(&self) -> ContextSnapshot {
        let mut evidence = self
            .evidence
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        evidence.manifest.record(unix_time_ms());
        ContextSnapshot {
            operation_id: Some(evidence.operation_id),
            manifest: self.plan.manifest.clone(),
            read_evidence: Some(evidence.clone()),
        }
    }

    pub(crate) fn content_read(
        &self,
        id: &str,
        generation: u64,
    ) -> Result<ContextContent, ContextReadError> {
        let operation_id = self.evidence().operation_id;
        let content = self.plan.content(Some(operation_id), id, generation)?;
        let mut evidence = self
            .evidence
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let observed_at = unix_time_ms();
        if let Some(existing) = evidence.entries.iter_mut().find(|entry| entry.id == id) {
            existing.stats.record(observed_at);
        } else {
            let mut stats = ContextReadStats {
                first_read_at_unix_ms: None,
                last_read_at_unix_ms: None,
                read_count: 0,
            };
            stats.record(observed_at);
            evidence.entries.push(ContextEntryRead {
                id: id.to_owned(),
                fingerprint: content.entry.fingerprint.clone(),
                stats,
            });
        }
        Ok(content)
    }

    pub(crate) fn evidence(&self) -> ContextReadEvidence {
        self.evidence
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

fn valid_context_id(value: &str) -> bool {
    let mut chars = value.chars();
    chars
        .next()
        .is_some_and(|character| character.is_ascii_alphanumeric())
        && chars.all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-' | '/')
        })
}

fn fingerprint<'a>(parts: impl IntoIterator<Item = &'a [u8]>) -> ContextFingerprint {
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
    ContextFingerprint(encoded)
}

fn unix_time_ms() -> Option<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
}

#[cfg(test)]
mod tests {
    use roba_core::{AgentSpec, ContextSpec, ProviderId, RunSpec};
    use serde_json::json;

    use super::*;

    fn entry(id: &str, sensitivity: ContextSensitivity) -> ContextEntrySpec {
        ContextEntrySpec::new(
            id,
            ContextKind::Instruction,
            ContextOrigin::new(ContextOriginKind::Cli, "--instruction"),
            ContextPhase::Bootstrap,
            ContextScope::Agent,
            ContextDelivery::ProviderAdapter,
        )
        .freshness(ContextFreshness::FreshSession)
        .sensitivity(sensitivity)
        .required(true)
    }

    #[test]
    fn manifest_and_debug_never_serialize_material() {
        let mut builder = ContextPlan::builder(AmbientContextPolicy::Controlled);
        builder
            .add_inline(
                entry("worker.instructions", ContextSensitivity::Redacted),
                "private worker instructions",
            )
            .unwrap();
        builder
            .add_inline(
                entry("worker.secret", ContextSensitivity::Secret),
                "secret-provider-token",
            )
            .unwrap();
        let plan = builder.build();

        let encoded = serde_json::to_string(plan.manifest()).unwrap();
        let debug = format!("{plan:?}");
        assert!(!encoded.contains("private worker instructions"));
        assert!(!encoded.contains("secret-provider-token"));
        assert!(!debug.contains("private worker instructions"));
        assert!(!debug.contains("secret-provider-token"));
        assert_eq!(
            plan.material("worker.instructions"),
            Some("private worker instructions")
        );
        assert!(plan.manifest().entries[0].fingerprint.is_some());
        assert!(plan.manifest().entries[1].fingerprint.is_none());
        assert!(plan.manifest().entries[0].material_available);
        assert!(!plan.manifest().entries[1].material_available);
    }

    #[test]
    fn manifest_schema_is_tagged_and_content_free() {
        let mut builder = ContextPlan::builder(AmbientContextPolicy::Ambient).generation(7);
        builder
            .add_available(
                ContextEntrySpec::new(
                    "issue.live",
                    ContextKind::Reference,
                    ContextOrigin::new(ContextOriginKind::Extension, "roba-gh"),
                    ContextPhase::Live,
                    ContextScope::Operation,
                    ContextDelivery::McpResource {
                        uri: "roba://context/issue".to_string(),
                    },
                )
                .freshness(ContextFreshness::Dynamic),
            )
            .unwrap();
        let value = serde_json::to_value(builder.build().manifest()).unwrap();

        assert_eq!(value["schema_version"], 1);
        assert_eq!(value["generation"], 7);
        assert_eq!(value["ambient_policy"], "ambient");
        assert_eq!(value["entries"][0]["delivery"]["kind"], "mcp_resource");
        assert_eq!(
            value["entries"][0]["delivery"]["uri"],
            "roba://context/issue"
        );
        assert_eq!(value["entries"][0]["material_available"], false);
        assert_eq!(value["entries"][0].get("fingerprint"), None);
    }

    #[test]
    fn construction_rejects_invalid_and_duplicate_ids() {
        let mut builder = ContextPlan::builder(AmbientContextPolicy::Ambient);
        assert_eq!(
            builder
                .add_available(entry("bad id", ContextSensitivity::Public))
                .unwrap_err(),
            ContextPlanError::InvalidId("bad id".to_string())
        );
        builder
            .add_available(entry("same.id", ContextSensitivity::Public))
            .unwrap();
        assert_eq!(
            builder
                .add_available(entry("same.id", ContextSensitivity::Public))
                .unwrap_err(),
            ContextPlanError::DuplicateId("same.id".to_string())
        );
    }

    #[test]
    fn fingerprints_are_stable_and_change_with_inspectable_material() {
        let build = |material: &str, sensitivity| {
            let mut builder = ContextPlan::builder(AmbientContextPolicy::Controlled);
            builder
                .add_inline(entry("one", sensitivity), material)
                .unwrap();
            builder.build()
        };
        let first = build("alpha", ContextSensitivity::Redacted);
        let again = build("alpha", ContextSensitivity::Redacted);
        let changed = build("beta", ContextSensitivity::Redacted);
        assert_eq!(first.manifest(), again.manifest());
        assert_ne!(first.manifest().fingerprint, changed.manifest().fingerprint);

        let secret_a = build("alpha", ContextSensitivity::Secret);
        let secret_b = build("beta", ContextSensitivity::Secret);
        assert_eq!(secret_a.manifest(), secret_b.manifest());
    }

    #[test]
    fn run_spec_inventory_preserves_order_and_current_every_turn_delivery() {
        let mut spec = RunSpec::suspended(AgentSpec::new(ProviderId::codex()));
        spec.agent.instructions = vec!["agent one".to_string(), "agent two".to_string()];
        spec.context = ContextSpec {
            project: vec!["project".to_string()],
            run: vec!["run".to_string()],
        };

        let plan = ContextPlan::from_run_spec(&spec);
        let ids = plan
            .manifest()
            .entries
            .iter()
            .map(|entry| entry.id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            ids,
            [
                "agent.instruction.1",
                "agent.instruction.2",
                "project.context.1",
                "run.context.1"
            ]
        );
        assert!(plan.manifest().entries.iter().all(|entry| {
            entry.delivery == ContextDelivery::ProviderAdapter
                && entry.freshness == ContextFreshness::EveryTurn
                && entry.required
                && entry.sensitivity == ContextSensitivity::Redacted
        }));
        assert_eq!(plan.material("agent.instruction.1"), Some("agent one"));
        assert_eq!(plan.material("run.context.1"), Some("run"));

        let value = serde_json::to_value(plan.manifest()).unwrap();
        assert_eq!(
            value["entries"][0]["origin"],
            json!({
                "kind": "run_spec",
                "label": "agent.instructions[0]"
            })
        );
    }

    #[test]
    fn operation_reads_are_generation_fenced_counted_and_content_safe_in_debug() {
        let mut builder = ContextPlan::builder(AmbientContextPolicy::Controlled).generation(4);
        builder
            .add_inline(
                entry("worker.instructions", ContextSensitivity::Redacted),
                "read this once",
            )
            .unwrap();
        let operation = OperationContext::new(OperationId::new(9), builder.build());

        let first_manifest = operation.manifest_read();
        assert_eq!(first_manifest.operation_id, Some(OperationId::new(9)));
        assert_eq!(
            first_manifest
                .read_evidence
                .as_ref()
                .unwrap()
                .manifest
                .read_count,
            1
        );
        let content = operation.content_read("worker.instructions", 4).unwrap();
        assert_eq!(content.content, "read this once");
        assert!(!format!("{content:?}").contains("read this once"));
        operation.content_read("worker.instructions", 4).unwrap();
        let evidence = operation.manifest_read().read_evidence.unwrap();
        assert_eq!(evidence.manifest.read_count, 2);
        assert_eq!(evidence.entries.len(), 1);
        assert_eq!(evidence.entries[0].stats.read_count, 2);

        assert_eq!(
            operation
                .content_read("worker.instructions", 3)
                .unwrap_err(),
            ContextReadError::GenerationMismatch {
                requested: 3,
                current: 4
            }
        );
        assert_eq!(operation.evidence().entries[0].stats.read_count, 2);
    }

    #[test]
    fn generic_context_resources_never_expose_secret_material() {
        let mut builder = ContextPlan::builder(AmbientContextPolicy::Hermetic);
        builder
            .add_inline(
                entry("provider.secret", ContextSensitivity::Secret),
                "do not expose",
            )
            .unwrap();
        let plan = builder.build();

        assert_eq!(
            plan.content(None, "provider.secret", 1).unwrap_err(),
            ContextReadError::ContentUnavailable("provider.secret".to_owned())
        );
        assert!(
            !serde_json::to_string(plan.manifest())
                .unwrap()
                .contains("do not expose")
        );
    }
}
