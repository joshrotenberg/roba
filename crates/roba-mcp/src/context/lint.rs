//! Deterministic semantic linting for Roba-declared context.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use roba_core::{PermissionPolicy, RunSpec, SessionSpec, is_valid_provider_mcp_name};
use serde::{Deserialize, Serialize};
use tower_mcp::schemars::{self, JsonSchema};

use super::{
    ContextDelivery, ContextEntry, ContextFingerprint, ContextFreshness, ContextKind,
    ContextOriginKind, ContextPlan, ContextSensitivity,
};

/// Aggregate eager material size that produces a warning.
pub const EAGER_CONTEXT_WARNING_BYTES: u64 = 32 * 1024;

/// Stable machine-readable context diagnostic code.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ContextDiagnosticCode {
    DuplicateMaterial,
    DeclaredPrecedenceConflict,
    AuthorityPolicyConflict,
    StableContextReinjected,
    UnsafeSourceLocator,
    ExcessiveEagerContext,
    RequiredDeliveryUnavailable,
}

impl fmt::Display for ContextDiagnosticCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::DuplicateMaterial => "duplicate_material",
            Self::DeclaredPrecedenceConflict => "declared_precedence_conflict",
            Self::AuthorityPolicyConflict => "authority_policy_conflict",
            Self::StableContextReinjected => "stable_context_reinjected",
            Self::UnsafeSourceLocator => "unsafe_source_locator",
            Self::ExcessiveEagerContext => "excessive_eager_context",
            Self::RequiredDeliveryUnavailable => "required_delivery_unavailable",
        })
    }
}

/// Enforcement severity for one context diagnostic.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ContextDiagnosticSeverity {
    Error,
    Warning,
}

/// Content-free provenance for one entry involved in a diagnostic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ContextDiagnosticProvenance {
    pub entry_id: String,
    pub origin_kind: ContextOriginKind,
    pub origin_label: String,
}

/// One deterministic, content-free finding about declared context.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ContextDiagnostic {
    pub code: ContextDiagnosticCode,
    pub severity: ContextDiagnosticSeverity,
    pub entry_ids: Vec<String>,
    pub provenance: Vec<ContextDiagnosticProvenance>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<ContextFingerprint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_bytes: Option<u64>,
    pub message: String,
    pub remediation: String,
}

impl ContextDiagnostic {
    pub fn is_error(&self) -> bool {
        self.severity == ContextDiagnosticSeverity::Error
    }
}

pub(super) fn evaluate(plan: &ContextPlan, spec: &RunSpec) -> Vec<ContextDiagnostic> {
    let entries = &plan.manifest.entries;
    let mut diagnostics = Vec::new();
    duplicate_material(entries, &mut diagnostics);
    declared_precedence_conflicts(plan, &mut diagnostics);
    authority_policy_conflicts(plan, spec.execution.permissions, &mut diagnostics);
    stable_reinjection(entries, &mut diagnostics);
    unsafe_locators(entries, &mut diagnostics);
    excessive_eager_context(plan, &mut diagnostics);
    unavailable_required_delivery(entries, &spec.execution.session, &mut diagnostics);
    diagnostics.sort_by(|left, right| {
        left.code
            .cmp(&right.code)
            .then_with(|| left.entry_ids.cmp(&right.entry_ids))
    });
    diagnostics
}

fn duplicate_material(entries: &[ContextEntry], diagnostics: &mut Vec<ContextDiagnostic>) {
    let mut groups: BTreeMap<&ContextFingerprint, Vec<&ContextEntry>> = BTreeMap::new();
    for entry in entries {
        if let Some(fingerprint) = &entry.fingerprint {
            groups.entry(fingerprint).or_default().push(entry);
        }
    }
    for (fingerprint, group) in groups {
        if group.len() < 2 {
            continue;
        }
        diagnostics.push(diagnostic(
            ContextDiagnosticCode::DuplicateMaterial,
            ContextDiagnosticSeverity::Warning,
            &group,
            Some(fingerprint.clone()),
            None,
            "multiple declared entries have the same safe content fingerprint",
            "remove the duplicate or give the entries one shared stable identity",
        ));
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum DirectiveFamily {
    Modify,
    Commit,
    Push,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum DirectivePolarity {
    Permit,
    Deny,
}

fn declared_precedence_conflicts(plan: &ContextPlan, diagnostics: &mut Vec<ContextDiagnostic>) {
    let mut directives: BTreeMap<DirectiveFamily, (Vec<&ContextEntry>, Vec<&ContextEntry>)> =
        BTreeMap::new();
    for entry in &plan.manifest.entries {
        if !matches!(
            entry.kind,
            ContextKind::Instruction | ContextKind::Authority
        ) {
            continue;
        }
        if entry.sensitivity == ContextSensitivity::Secret {
            continue;
        }
        let Some(material) = plan.material.get(&entry.id) else {
            continue;
        };
        for (family, polarity) in classify_directives(material) {
            let (permitted, denied) = directives.entry(family).or_default();
            match polarity {
                DirectivePolarity::Permit => permitted.push(entry),
                DirectivePolarity::Deny => denied.push(entry),
            }
        }
    }
    for (_family, (permitted, denied)) in directives {
        if permitted.is_empty() || denied.is_empty() {
            continue;
        }
        let mut involved = permitted;
        involved.extend(denied);
        involved.sort_by(|left, right| left.id.cmp(&right.id));
        involved.dedup_by(|left, right| left.id == right.id);
        diagnostics.push(diagnostic(
            ContextDiagnosticCode::DeclaredPrecedenceConflict,
            ContextDiagnosticSeverity::Warning,
            &involved,
            None,
            None,
            "declared instructions contain opposing directives at Roba precedence layers",
            "remove the contradiction or replace it with one explicit higher-precedence instruction",
        ));
    }
}

fn classify_directives(material: &str) -> BTreeSet<(DirectiveFamily, DirectivePolarity)> {
    let normalized = material.to_ascii_lowercase();
    let mut findings = BTreeSet::new();
    for phrase in [
        "you may modify files",
        "you may edit files",
        "edit files as needed",
        "make changes as needed",
    ] {
        if normalized.contains(phrase) {
            findings.insert((DirectiveFamily::Modify, DirectivePolarity::Permit));
        }
    }
    for phrase in [
        "do not modify files",
        "must not modify files",
        "do not edit files",
        "must not edit files",
    ] {
        if normalized.contains(phrase) {
            findings.insert((DirectiveFamily::Modify, DirectivePolarity::Deny));
        }
    }
    for phrase in ["commit changes", "create a commit", "you may commit"] {
        if normalized.contains(phrase) {
            findings.insert((DirectiveFamily::Commit, DirectivePolarity::Permit));
        }
    }
    for phrase in ["do not commit", "must not commit"] {
        if normalized.contains(phrase) {
            findings.insert((DirectiveFamily::Commit, DirectivePolarity::Deny));
        }
    }
    for phrase in ["push changes", "you may push"] {
        if normalized.contains(phrase) {
            findings.insert((DirectiveFamily::Push, DirectivePolarity::Permit));
        }
    }
    for phrase in ["do not push", "must not push"] {
        if normalized.contains(phrase) {
            findings.insert((DirectiveFamily::Push, DirectivePolarity::Deny));
        }
    }
    findings
}

fn authority_policy_conflicts(
    plan: &ContextPlan,
    permissions: PermissionPolicy,
    diagnostics: &mut Vec<ContextDiagnostic>,
) {
    if permissions != PermissionPolicy::ReadOnly {
        return;
    }
    let mut involved = Vec::new();
    for entry in &plan.manifest.entries {
        if !matches!(
            entry.kind,
            ContextKind::Instruction | ContextKind::Authority
        ) {
            continue;
        }
        if entry.sensitivity == ContextSensitivity::Secret {
            continue;
        }
        let Some(material) = plan.material.get(&entry.id) else {
            continue;
        };
        if classify_directives(material)
            .iter()
            .any(|(_, polarity)| *polarity == DirectivePolarity::Permit)
        {
            involved.push(entry);
        }
    }
    if involved.is_empty() {
        return;
    }
    diagnostics.push(diagnostic(
        ContextDiagnosticCode::AuthorityPolicyConflict,
        ContextDiagnosticSeverity::Warning,
        &involved,
        None,
        None,
        "declared prose appears to grant writes while typed execution authority is read-only",
        "remove the prose grant or select an explicit writable execution policy; prose never grants authority",
    ));
}

fn stable_reinjection(entries: &[ContextEntry], diagnostics: &mut Vec<ContextDiagnostic>) {
    for entry in entries {
        if entry.delivery == ContextDelivery::ProviderAdapter
            && !matches!(
                entry.freshness,
                ContextFreshness::EveryTurn | ContextFreshness::Dynamic
            )
        {
            diagnostics.push(diagnostic(
                ContextDiagnosticCode::StableContextReinjected,
                ContextDiagnosticSeverity::Warning,
                &[entry],
                entry.fingerprint.clone(),
                None,
                "generation-stable context is wired through an every-turn provider adapter path",
                "declare every-turn freshness or move stable material behind a generation-fenced context read",
            ));
        }
    }
}

fn unsafe_locators(entries: &[ContextEntry], diagnostics: &mut Vec<ContextDiagnostic>) {
    for entry in entries {
        let origin_unsafe = entry
            .origin
            .locator
            .as_deref()
            .is_some_and(locator_is_unsafe);
        let delivery_unsafe = match &entry.delivery {
            ContextDelivery::McpResource { uri } => !valid_uri(uri),
            ContextDelivery::McpTool { name } => !is_valid_provider_mcp_name(name),
            _ => false,
        };
        if origin_unsafe || delivery_unsafe {
            diagnostics.push(diagnostic(
                ContextDiagnosticCode::UnsafeSourceLocator,
                ContextDiagnosticSeverity::Error,
                &[entry],
                None,
                None,
                "a declared context locator is empty, malformed, contains control data, or resembles embedded credentials",
                "replace it with a bounded body-free path, URI, or exact MCP capability name",
            ));
        }
    }
}

fn locator_is_unsafe(locator: &str) -> bool {
    if locator.is_empty()
        || locator.trim() != locator
        || locator.len() > 2048
        || locator.chars().any(char::is_control)
    {
        return true;
    }
    let lowered = locator.to_ascii_lowercase();
    if [
        "token=",
        "password=",
        "secret=",
        "api_key=",
        "apikey=",
        "authorization=",
    ]
    .iter()
    .any(|marker| lowered.contains(marker))
    {
        return true;
    }
    if let Some((_, remainder)) = locator.split_once("://") {
        let authority = remainder.split('/').next().unwrap_or(remainder);
        if authority.contains('@') {
            return true;
        }
    }
    false
}

fn valid_uri(uri: &str) -> bool {
    if uri.len() > 2048 || uri.chars().any(char::is_whitespace) {
        return false;
    }
    let Some((scheme, remainder)) = uri.split_once(':') else {
        return false;
    };
    !remainder.is_empty()
        && scheme
            .chars()
            .next()
            .is_some_and(|character| character.is_ascii_alphabetic())
        && scheme.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '+' | '-' | '.')
        })
        && !locator_is_unsafe(uri)
}

fn excessive_eager_context(plan: &ContextPlan, diagnostics: &mut Vec<ContextDiagnostic>) {
    let eager = plan
        .manifest
        .entries
        .iter()
        .filter(|entry| {
            matches!(
                entry.delivery,
                ContextDelivery::ProviderAdapter | ContextDelivery::Bootstrap
            )
        })
        .filter_map(|entry| {
            plan.material
                .get(&entry.id)
                .map(|material| (entry, material))
        });
    let mut involved = Vec::new();
    let mut bytes = 0_u64;
    for (entry, material) in eager {
        involved.push(entry);
        bytes = bytes.saturating_add(u64::try_from(material.len()).unwrap_or(u64::MAX));
    }
    if bytes <= EAGER_CONTEXT_WARNING_BYTES {
        return;
    }
    diagnostics.push(diagnostic(
        ContextDiagnosticCode::ExcessiveEagerContext,
        ContextDiagnosticSeverity::Warning,
        &involved,
        None,
        Some(bytes),
        "eagerly delivered declared context exceeds the bounded byte warning threshold",
        "keep launch context concise and move large reference material behind MCP context reads",
    ));
}

fn unavailable_required_delivery(
    entries: &[ContextEntry],
    session: &SessionSpec,
    diagnostics: &mut Vec<ContextDiagnostic>,
) {
    for entry in entries.iter().filter(|entry| entry.required) {
        let unavailable = match &entry.delivery {
            ContextDelivery::ProviderAdapter | ContextDelivery::Bootstrap => {
                !entry.material_available
            }
            ContextDelivery::Session => matches!(session, SessionSpec::Fresh),
            ContextDelivery::McpResource { uri } => !valid_uri(uri),
            ContextDelivery::McpTool { name } => !is_valid_provider_mcp_name(name),
            ContextDelivery::ProviderAmbient => false,
        };
        if unavailable {
            diagnostics.push(diagnostic(
                ContextDiagnosticCode::RequiredDeliveryUnavailable,
                ContextDiagnosticSeverity::Error,
                &[entry],
                None,
                None,
                "a required context entry cannot be acquired through its declared delivery path",
                "provide retained material, a valid MCP capability, or a resumed provider session",
            ));
        }
    }
}

fn diagnostic(
    code: ContextDiagnosticCode,
    severity: ContextDiagnosticSeverity,
    entries: &[&ContextEntry],
    fingerprint: Option<ContextFingerprint>,
    observed_bytes: Option<u64>,
    message: &str,
    remediation: &str,
) -> ContextDiagnostic {
    let mut ordered = entries.to_vec();
    ordered.sort_by(|left, right| left.id.cmp(&right.id));
    ordered.dedup_by(|left, right| left.id == right.id);
    ContextDiagnostic {
        code,
        severity,
        entry_ids: ordered.iter().map(|entry| entry.id.clone()).collect(),
        provenance: ordered
            .iter()
            .map(|entry| ContextDiagnosticProvenance {
                entry_id: entry.id.clone(),
                origin_kind: entry.origin.kind,
                origin_label: entry.origin.label.clone(),
            })
            .collect(),
        fingerprint,
        observed_bytes,
        message: message.to_owned(),
        remediation: remediation.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use roba_core::{AgentSpec, ProviderId};

    use super::*;
    use crate::context::{
        AmbientContextPolicy, ContextAudience, ContextEntrySpec, ContextOrigin, ContextPhase,
        ContextPrecedence, ContextScope,
    };

    fn spec() -> RunSpec {
        RunSpec::suspended(AgentSpec::new(ProviderId::new("fake").unwrap()))
    }

    fn entry(id: &str, kind: ContextKind, delivery: ContextDelivery) -> ContextEntrySpec {
        ContextEntrySpec::new(
            id,
            kind,
            ContextOrigin::new(ContextOriginKind::Workspace, "lint fixture"),
            ContextPhase::Bootstrap,
            ContextScope::Agent,
            delivery,
        )
        .audience(ContextAudience::Both)
        .freshness(ContextFreshness::EveryTurn)
        .sensitivity(ContextSensitivity::Redacted)
    }

    #[test]
    fn duplicate_safe_fingerprints_name_every_entry_without_bodies_or_secrets() {
        let mut builder = ContextPlan::builder(AmbientContextPolicy::Ambient);
        builder
            .add_inline(
                entry(
                    "duplicate.alpha",
                    ContextKind::Reference,
                    ContextDelivery::McpResource {
                        uri: "roba://fixture/alpha".to_owned(),
                    },
                ),
                "PRIVATE DUPLICATE BODY",
            )
            .unwrap();
        builder
            .add_inline(
                entry(
                    "duplicate.beta",
                    ContextKind::Reference,
                    ContextDelivery::McpResource {
                        uri: "roba://fixture/beta".to_owned(),
                    },
                )
                .sensitivity(ContextSensitivity::Public),
                "PRIVATE DUPLICATE BODY",
            )
            .unwrap();
        builder
            .add_inline(
                entry(
                    "secret.gamma",
                    ContextKind::Instruction,
                    ContextDelivery::McpResource {
                        uri: "roba://fixture/gamma".to_owned(),
                    },
                )
                .sensitivity(ContextSensitivity::Secret),
                "PRIVATE DUPLICATE BODY",
            )
            .unwrap();
        let diagnostics = builder.build().lint(&spec());

        assert_eq!(diagnostics.len(), 1);
        let duplicate = &diagnostics[0];
        assert_eq!(duplicate.code, ContextDiagnosticCode::DuplicateMaterial);
        assert_eq!(duplicate.entry_ids, ["duplicate.alpha", "duplicate.beta"]);
        assert!(duplicate.fingerprint.is_some());
        let encoded = serde_json::to_string(&diagnostics).unwrap();
        assert!(!encoded.contains("PRIVATE DUPLICATE BODY"));
        assert!(!format!("{diagnostics:?}").contains("PRIVATE DUPLICATE BODY"));
    }

    #[test]
    fn provider_diagnostics_do_not_reveal_operator_only_entries() {
        let mut builder = ContextPlan::builder(AmbientContextPolicy::Ambient);
        builder
            .add_inline(
                entry(
                    "provider.reference",
                    ContextKind::Reference,
                    ContextDelivery::McpResource {
                        uri: "roba://fixture/provider".to_owned(),
                    },
                )
                .audience(ContextAudience::Provider),
                "PRIVATE SHARED BODY",
            )
            .unwrap();
        builder
            .add_inline(
                entry(
                    "operator.reference",
                    ContextKind::Reference,
                    ContextDelivery::McpResource {
                        uri: "roba://fixture/operator".to_owned(),
                    },
                )
                .audience(ContextAudience::Operator),
                "PRIVATE SHARED BODY",
            )
            .unwrap();
        let plan = builder.build();

        assert_eq!(plan.lint(&spec()).len(), 1);
        assert!(plan.provider_lint(&spec()).is_empty());
    }

    #[test]
    fn bounded_directive_checks_are_deterministic_and_respect_false_positive_boundaries() {
        let mut builder = ContextPlan::builder(AmbientContextPolicy::Ambient);
        builder
            .add_inline(
                entry(
                    "policy.allow",
                    ContextKind::Authority,
                    ContextDelivery::McpResource {
                        uri: "roba://fixture/allow".to_owned(),
                    },
                ),
                "You may modify files after validation.",
            )
            .unwrap();
        builder
            .add_inline(
                entry(
                    "policy.deny",
                    ContextKind::Instruction,
                    ContextDelivery::McpResource {
                        uri: "roba://fixture/deny".to_owned(),
                    },
                )
                .precedence(ContextPrecedence::Operation),
                "Do not modify files in this operation.",
            )
            .unwrap();
        builder
            .add_inline(
                entry(
                    "reference.words",
                    ContextKind::Reference,
                    ContextDelivery::McpResource {
                        uri: "roba://fixture/reference".to_owned(),
                    },
                ),
                "A document asking whether to modify files.",
            )
            .unwrap();
        builder
            .add_inline(
                entry(
                    "secret.words",
                    ContextKind::Instruction,
                    ContextDelivery::McpResource {
                        uri: "roba://fixture/secret".to_owned(),
                    },
                )
                .sensitivity(ContextSensitivity::Secret),
                "You may push changes.",
            )
            .unwrap();

        let diagnostics = builder.build().lint(&spec());
        assert_eq!(
            diagnostics
                .iter()
                .map(|diagnostic| diagnostic.code)
                .collect::<Vec<_>>(),
            [
                ContextDiagnosticCode::DeclaredPrecedenceConflict,
                ContextDiagnosticCode::AuthorityPolicyConflict,
            ]
        );
        assert_eq!(diagnostics[0].entry_ids, ["policy.allow", "policy.deny"]);
        assert_eq!(diagnostics[1].entry_ids, ["policy.allow"]);
        assert!(diagnostics.iter().all(|diagnostic| !diagnostic.is_error()));
    }

    #[test]
    fn stable_reinjection_and_eager_weight_are_mechanical_warnings() {
        let mut builder = ContextPlan::builder(AmbientContextPolicy::Ambient);
        builder
            .add_inline(
                entry(
                    "stable.adapter",
                    ContextKind::Instruction,
                    ContextDelivery::ProviderAdapter,
                )
                .freshness(ContextFreshness::Generation),
                "stable",
            )
            .unwrap();
        builder
            .add_inline(
                entry(
                    "large.adapter",
                    ContextKind::Reference,
                    ContextDelivery::ProviderAdapter,
                ),
                "x".repeat(usize::try_from(EAGER_CONTEXT_WARNING_BYTES).unwrap()),
            )
            .unwrap();

        let diagnostics = builder.build().lint(&spec());
        assert_eq!(
            diagnostics
                .iter()
                .map(|diagnostic| diagnostic.code)
                .collect::<Vec<_>>(),
            [
                ContextDiagnosticCode::StableContextReinjected,
                ContextDiagnosticCode::ExcessiveEagerContext,
            ]
        );
        assert_eq!(
            diagnostics[1].observed_bytes,
            Some(EAGER_CONTEXT_WARNING_BYTES + 6)
        );
    }

    #[test]
    fn unsafe_locators_and_impossible_required_delivery_are_hard_findings() {
        let mut builder = ContextPlan::builder(AmbientContextPolicy::Ambient);
        builder
            .add_available(
                entry(
                    "unsafe.source",
                    ContextKind::Reference,
                    ContextDelivery::McpResource { uri: String::new() },
                )
                .required(true),
            )
            .unwrap();
        builder
            .add_available(
                entry(
                    "session.required",
                    ContextKind::Session,
                    ContextDelivery::Session,
                )
                .required(true),
            )
            .unwrap();
        builder
            .add_available(ContextEntrySpec {
                origin: ContextOrigin::new(ContextOriginKind::External, "tracker")
                    .with_locator("https://example.test/item?token=PRIVATE_TOKEN"),
                ..entry(
                    "credential.locator",
                    ContextKind::Reference,
                    ContextDelivery::McpResource {
                        uri: "roba://fixture/safe".to_owned(),
                    },
                )
            })
            .unwrap();

        let diagnostics = builder.build().lint(&spec());
        assert_eq!(
            diagnostics
                .iter()
                .map(|diagnostic| diagnostic.code)
                .collect::<Vec<_>>(),
            [
                ContextDiagnosticCode::UnsafeSourceLocator,
                ContextDiagnosticCode::UnsafeSourceLocator,
                ContextDiagnosticCode::RequiredDeliveryUnavailable,
                ContextDiagnosticCode::RequiredDeliveryUnavailable,
            ]
        );
        assert!(diagnostics.iter().all(ContextDiagnostic::is_error));
        let encoded = serde_json::to_string(&diagnostics).unwrap();
        assert!(!encoded.contains("PRIVATE_TOKEN"));
    }

    #[test]
    fn normal_paths_uris_and_non_directive_prose_do_not_warn() {
        let mut builder = ContextPlan::builder(AmbientContextPolicy::Ambient);
        builder
            .add_inline(
                ContextEntrySpec {
                    origin: ContextOrigin::new(ContextOriginKind::Workspace, "config")
                        .with_locator("/workspace/roba.toml"),
                    ..entry(
                        "safe.reference",
                        ContextKind::Reference,
                        ContextDelivery::McpResource {
                            uri: "roba://context/entry?id=safe.reference&generation=1".to_owned(),
                        },
                    )
                },
                "Discuss whether to modify files; this is not an authority grant.",
            )
            .unwrap();

        assert!(builder.build().lint(&spec()).is_empty());
    }
}
