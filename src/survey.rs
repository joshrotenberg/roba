//! Bounded, content-free project evidence for future configuration tuning.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use roba_context::CatalogManifest;
use roba_mcp::{AgentConfiguration, AmbientContextStatus, ContextDiagnostic, ContextManifest};
use serde::{Deserialize, Serialize};

use crate::VersionedResult;
use crate::cli::ConfigSurveyArgs;
use crate::startup_config::{
    ConfigSource, EffectiveCatalogSelection, EffectiveExtensionsConfig, EffectiveStartupConfig,
};

/// Schema version of the deterministic project-survey packet.
pub const PROJECT_SURVEY_SCHEMA_VERSION: u32 = 1;
/// Maximum serialized bytes accepted for the safe startup portion.
pub const PROJECT_SURVEY_MAX_STARTUP_BYTES: u64 = 1024 * 1024;

const MARKER_CANDIDATES: [MarkerCandidate; 18] = [
    MarkerCandidate::file("AGENTS.md", SurveyMarkerKind::Guidance, None),
    MarkerCandidate::file("ARCHITECTURE.md", SurveyMarkerKind::Guidance, None),
    MarkerCandidate::file("README.md", SurveyMarkerKind::Documentation, None),
    MarkerCandidate::file("CONTRIBUTING.md", SurveyMarkerKind::Documentation, None),
    MarkerCandidate::file(
        "Cargo.toml",
        SurveyMarkerKind::PackageManifest,
        Some("rust"),
    ),
    MarkerCandidate::file(
        "package.json",
        SurveyMarkerKind::PackageManifest,
        Some("javascript"),
    ),
    MarkerCandidate::file(
        "pyproject.toml",
        SurveyMarkerKind::PackageManifest,
        Some("python"),
    ),
    MarkerCandidate::file("go.mod", SurveyMarkerKind::PackageManifest, Some("go")),
    MarkerCandidate::file("mix.exs", SurveyMarkerKind::PackageManifest, Some("elixir")),
    MarkerCandidate::file("pom.xml", SurveyMarkerKind::PackageManifest, Some("java")),
    MarkerCandidate::file(
        "build.gradle",
        SurveyMarkerKind::PackageManifest,
        Some("java"),
    ),
    MarkerCandidate::file("Makefile", SurveyMarkerKind::Automation, None),
    MarkerCandidate::file("justfile", SurveyMarkerKind::Automation, None),
    MarkerCandidate::directory("src", SurveyMarkerKind::SourceDirectory),
    MarkerCandidate::directory("tests", SurveyMarkerKind::TestDirectory),
    MarkerCandidate::directory("docs", SurveyMarkerKind::DocumentationDirectory),
    MarkerCandidate::directory(".github/workflows", SurveyMarkerKind::WorkflowDirectory),
    MarkerCandidate::directory(".roba", SurveyMarkerKind::ContextDirectory),
];

/// Safe startup and workspace evidence supplied by one survey.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectSurvey {
    pub schema_version: u32,
    pub limits: ProjectSurveyLimits,
    pub startup: SurveyStartup,
    pub workspace: SurveyWorkspace,
}

/// Mechanical bounds and disclosure policy applied to the survey.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectSurveyLimits {
    pub recursive: bool,
    pub file_contents_included: bool,
    pub marker_candidates: u32,
    pub max_startup_bytes: u64,
    pub observed_startup_bytes: u64,
}

/// Body-free effective startup state selected for a future tuning proposal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SurveyStartup {
    pub sources: Vec<ConfigSource>,
    pub configuration: AgentConfiguration,
    pub ambient_context: AmbientContextStatus,
    pub context_manifest: ContextManifest,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub context_diagnostics: Vec<ContextDiagnostic>,
    pub catalog: CatalogManifest,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selection: Option<EffectiveCatalogSelection>,
    pub extensions: EffectiveExtensionsConfig,
    pub provenance: BTreeMap<String, Vec<String>>,
}

/// Fixed-scope workspace facts collected without reading file contents.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SurveyWorkspace {
    pub canonical_cwd: String,
    pub marker_root: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository: Option<SurveyRepository>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ecosystems: Vec<String>,
    pub markers: Vec<SurveyMarker>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub omissions: Vec<SurveyMarkerOmission>,
}

/// Nearest repository boundary discovered without executing Git.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SurveyRepository {
    pub root: String,
    pub relative_cwd: String,
}

/// One recognized top-level project marker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SurveyMarker {
    pub path: String,
    pub kind: SurveyMarkerKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes: Option<u64>,
}

/// Stable semantic category for a recognized marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SurveyMarkerKind {
    Guidance,
    Documentation,
    PackageManifest,
    Automation,
    SourceDirectory,
    TestDirectory,
    DocumentationDirectory,
    WorkflowDirectory,
    ContextDirectory,
}

/// Why a present recognized marker was excluded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SurveyMarkerOmission {
    pub path: String,
    pub reason: SurveyMarkerOmissionReason,
}

/// Stable exclusion reason for a recognized path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SurveyMarkerOmissionReason {
    Symlink,
    UnexpectedType,
}

#[derive(Debug, Clone, Copy)]
struct MarkerCandidate {
    path: &'static str,
    kind: SurveyMarkerKind,
    expected: ExpectedPathKind,
    ecosystem: Option<&'static str>,
}

impl MarkerCandidate {
    const fn file(
        path: &'static str,
        kind: SurveyMarkerKind,
        ecosystem: Option<&'static str>,
    ) -> Self {
        Self {
            path,
            kind,
            expected: ExpectedPathKind::File,
            ecosystem,
        }
    }

    const fn directory(path: &'static str, kind: SurveyMarkerKind) -> Self {
        Self {
            path,
            kind,
            expected: ExpectedPathKind::Directory,
            ecosystem: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExpectedPathKind {
    File,
    Directory,
}

/// Build and print a bounded survey without starting provider work.
pub async fn run(args: ConfigSurveyArgs) -> Result<()> {
    let resolved = crate::startup_config::resolve(&args.agent)?;
    let host = crate::bounded::build_agent_from_template(
        resolved.template,
        resolved.catalog.clone(),
        resolved.catalog_selection,
        resolved.ambient_context_policy,
        resolved.git_enabled,
        resolved.git_progress_interval_secs,
    )?;
    let agent = host.snapshot().await;
    let context = host.context_snapshot().await;
    let effective = resolved.effective;
    let cwd = std::env::current_dir().context("resolving project survey cwd")?;
    let startup = survey_startup(effective, agent.configuration, context);
    let startup_bytes = validate_startup_size(serde_json::to_vec(&startup)?.len())?;
    let survey = ProjectSurvey {
        schema_version: PROJECT_SURVEY_SCHEMA_VERSION,
        limits: ProjectSurveyLimits {
            recursive: false,
            file_contents_included: false,
            marker_candidates: u32::try_from(MARKER_CANDIDATES.len())
                .expect("static survey candidate count fits u32"),
            max_startup_bytes: PROJECT_SURVEY_MAX_STARTUP_BYTES,
            observed_startup_bytes: startup_bytes,
        },
        startup,
        workspace: survey_workspace(&cwd)?,
    };
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&VersionedResult::new(survey))?
        );
    } else {
        print!("{}", toml::to_string_pretty(&survey)?);
    }
    Ok(())
}

fn validate_startup_size(bytes: usize) -> Result<u64> {
    let bytes = u64::try_from(bytes).unwrap_or(u64::MAX);
    if bytes > PROJECT_SURVEY_MAX_STARTUP_BYTES {
        anyhow::bail!(
            "project survey startup evidence is {bytes} bytes, exceeding the {PROJECT_SURVEY_MAX_STARTUP_BYTES}-byte limit"
        );
    }
    Ok(bytes)
}

fn survey_startup(
    effective: EffectiveStartupConfig,
    configuration: AgentConfiguration,
    context: roba_mcp::ContextSnapshot,
) -> SurveyStartup {
    SurveyStartup {
        sources: effective.sources,
        configuration,
        ambient_context: context.ambient_context,
        context_manifest: context.manifest,
        context_diagnostics: context.diagnostics,
        catalog: effective.context.catalog,
        selection: effective.context.selection,
        extensions: effective.extensions,
        provenance: effective.provenance,
    }
}

fn survey_workspace(cwd: &Path) -> Result<SurveyWorkspace> {
    let canonical_cwd = std::fs::canonicalize(cwd)
        .with_context(|| format!("canonicalizing project survey cwd {}", cwd.display()))?;
    let repository = nearest_repository(&canonical_cwd)?;
    let marker_root = repository
        .as_ref()
        .map_or(canonical_cwd.as_path(), |(root, _)| root.as_path());
    let mut ecosystems = BTreeSet::new();
    let mut markers = Vec::new();
    let mut omissions = Vec::new();

    for candidate in MARKER_CANDIDATES {
        let path = marker_root.join(candidate.path);
        let metadata = match std::fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("inspecting survey marker {}", path.display()));
            }
        };
        if metadata.file_type().is_symlink() {
            omissions.push(SurveyMarkerOmission {
                path: candidate.path.to_owned(),
                reason: SurveyMarkerOmissionReason::Symlink,
            });
            continue;
        }
        let expected_type = match candidate.expected {
            ExpectedPathKind::File => metadata.is_file(),
            ExpectedPathKind::Directory => metadata.is_dir(),
        };
        if !expected_type {
            omissions.push(SurveyMarkerOmission {
                path: candidate.path.to_owned(),
                reason: SurveyMarkerOmissionReason::UnexpectedType,
            });
            continue;
        }
        if let Some(ecosystem) = candidate.ecosystem {
            ecosystems.insert(ecosystem.to_owned());
        }
        markers.push(SurveyMarker {
            path: candidate.path.to_owned(),
            kind: candidate.kind,
            bytes: metadata.is_file().then_some(metadata.len()),
        });
    }
    markers.sort_by(|left, right| left.path.cmp(&right.path));
    omissions.sort_by(|left, right| left.path.cmp(&right.path));

    Ok(SurveyWorkspace {
        canonical_cwd: display_path(&canonical_cwd),
        marker_root: display_path(marker_root),
        repository: repository.map(|(_, evidence)| evidence),
        ecosystems: ecosystems.into_iter().collect(),
        markers,
        omissions,
    })
}

fn nearest_repository(cwd: &Path) -> Result<Option<(PathBuf, SurveyRepository)>> {
    let Some(root) = cwd
        .ancestors()
        .find(|candidate| candidate.join(".git").exists())
    else {
        return Ok(None);
    };
    let root = std::fs::canonicalize(root)
        .with_context(|| format!("canonicalizing survey repository root {}", root.display()))?;
    let relative = cwd.strip_prefix(&root).unwrap_or(Path::new(""));
    Ok(Some((
        root.clone(),
        SurveyRepository {
            root: display_path(&root),
            relative_cwd: if relative.as_os_str().is_empty() {
                ".".to_owned()
            } else {
                display_path(relative)
            },
        },
    )))
}

fn display_path(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_survey_is_fixed_content_free_and_repository_scoped() {
        let project = tempfile::tempdir().unwrap();
        std::fs::create_dir(project.path().join(".git")).unwrap();
        std::fs::create_dir(project.path().join("src")).unwrap();
        std::fs::write(project.path().join("README.md"), "PRIVATE README BODY").unwrap();
        std::fs::write(project.path().join("Cargo.toml"), "PRIVATE MANIFEST BODY").unwrap();
        std::fs::write(project.path().join("secrets.env"), "PRIVATE SECRET").unwrap();
        let nested = project.path().join("nested");
        std::fs::create_dir(&nested).unwrap();

        let survey = survey_workspace(&nested).unwrap();
        let repository = survey.repository.unwrap();
        assert_eq!(
            survey.marker_root,
            display_path(&std::fs::canonicalize(project.path()).unwrap())
        );
        assert_eq!(
            repository.root,
            display_path(&std::fs::canonicalize(project.path()).unwrap())
        );
        assert_eq!(repository.relative_cwd, "nested");
        assert_eq!(
            survey
                .markers
                .iter()
                .map(|marker| marker.path.as_str())
                .collect::<Vec<_>>(),
            ["Cargo.toml", "README.md", "src"]
        );

        let survey = survey_workspace(project.path()).unwrap();
        assert_eq!(survey.ecosystems, ["rust"]);
        assert_eq!(
            survey
                .markers
                .iter()
                .map(|marker| marker.path.as_str())
                .collect::<Vec<_>>(),
            ["Cargo.toml", "README.md", "src"]
        );
        let encoded = serde_json::to_string(&survey).unwrap();
        for private in [
            "PRIVATE README BODY",
            "PRIVATE MANIFEST BODY",
            "PRIVATE SECRET",
            "secrets.env",
        ] {
            assert!(!encoded.contains(private));
        }
    }

    #[test]
    fn workspace_survey_reports_present_wrong_types_without_following_them() {
        let project = tempfile::tempdir().unwrap();
        std::fs::create_dir(project.path().join("README.md")).unwrap();
        std::fs::write(project.path().join("src"), "not a directory").unwrap();

        let survey = survey_workspace(project.path()).unwrap();
        assert!(survey.markers.is_empty());
        assert_eq!(
            survey.omissions,
            [
                SurveyMarkerOmission {
                    path: "README.md".to_owned(),
                    reason: SurveyMarkerOmissionReason::UnexpectedType,
                },
                SurveyMarkerOmission {
                    path: "src".to_owned(),
                    reason: SurveyMarkerOmissionReason::UnexpectedType,
                },
            ]
        );
    }

    #[test]
    fn startup_evidence_has_an_exact_serialized_ceiling() {
        assert_eq!(
            validate_startup_size(usize::try_from(PROJECT_SURVEY_MAX_STARTUP_BYTES).unwrap())
                .unwrap(),
            PROJECT_SURVEY_MAX_STARTUP_BYTES
        );
        let error =
            validate_startup_size(usize::try_from(PROJECT_SURVEY_MAX_STARTUP_BYTES).unwrap() + 1)
                .unwrap_err();
        assert!(error.to_string().contains("exceeding"));
    }
}
