use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use roba_context::{
    CatalogDefinition, CatalogError, CatalogOrigin, CatalogOriginKind, CatalogSelectionSpec,
    CatalogSource, ContextCatalog,
};
use serde_json::json;
use tempfile::TempDir;

fn project_origin() -> CatalogOrigin {
    CatalogOrigin::new(CatalogOriginKind::Project, "roba.toml").with_locator("/workspace/roba.toml")
}

fn inline(content: &str) -> CatalogSource {
    CatalogSource::Inline {
        content: content.to_owned(),
    }
}

fn inline_skill(id: &str, content: &str) -> CatalogDefinition {
    CatalogDefinition::Skill {
        id: id.to_owned(),
        description: format!("Skill {id}"),
        source: inline(content),
    }
}

#[test]
fn builtins_select_render_and_never_debug_material() {
    let catalog = ContextCatalog::builtins();
    let selection = catalog
        .select(&CatalogSelectionSpec {
            agent: "roba.repo-worker".to_owned(),
            skills: Vec::new(),
            prompts: vec!["roba.issue-worker".to_owned()],
        })
        .unwrap();

    assert_eq!(selection.skills.len(), 1);
    assert_eq!(selection.skills[0].id(), "roba.repository-change");
    let rendered = catalog
        .render_prompt(
            "roba.issue-worker",
            &BTreeMap::from([("issue".to_owned(), "#514".to_owned())]),
        )
        .unwrap();
    assert!(rendered.contains("issue #514"));
    assert!(!rendered.contains("{{issue}}"));

    let debug = format!("{catalog:?}");
    assert!(!debug.contains("Do not commit"));
    assert!(
        !serde_json::to_string(catalog.manifest())
            .unwrap()
            .contains("Do not commit")
    );
}

#[test]
fn builtins_are_an_additive_catalog_foundation() {
    let mut builder = ContextCatalog::builder_with_builtins();
    builder
        .add(
            project_origin(),
            ".",
            inline_skill("local.review", "Review this repository."),
        )
        .unwrap();
    let catalog = builder.build().unwrap();
    assert!(catalog.entry("roba.repo-worker").is_some());
    assert!(catalog.entry("local.review").is_some());
}

#[test]
fn path_sources_are_relative_bounded_and_fingerprinted() {
    let directory = TempDir::new().unwrap();
    fs::create_dir(directory.path().join("skills")).unwrap();
    fs::write(
        directory.path().join("skills/review.md"),
        "Review the exact diff.",
    )
    .unwrap();
    let definition = CatalogDefinition::Skill {
        id: "local.review".to_owned(),
        description: "Review the current change.".to_owned(),
        source: CatalogSource::MarkdownPath {
            path: PathBuf::from("skills/review.md"),
        },
    };
    let mut first = ContextCatalog::builder();
    first
        .add(project_origin(), directory.path(), definition.clone())
        .unwrap();
    let first = first.build().unwrap();
    assert_eq!(
        first.material("local.review"),
        Some("Review the exact diff.")
    );

    fs::write(
        directory.path().join("skills/review.md"),
        "Review the exact diff and tests.",
    )
    .unwrap();
    let mut second = ContextCatalog::builder();
    second
        .add(project_origin(), directory.path(), definition)
        .unwrap();
    let second = second.build().unwrap();
    assert_ne!(
        first.entry("local.review").unwrap().fingerprint(),
        second.entry("local.review").unwrap().fingerprint()
    );
    assert_ne!(first.manifest().fingerprint, second.manifest().fingerprint);
}

#[test]
fn path_sources_cannot_escape_or_use_non_markdown_files() {
    let directory = TempDir::new().unwrap();
    let mut builder = ContextCatalog::builder();
    let error = builder
        .add(
            project_origin(),
            directory.path(),
            CatalogDefinition::Skill {
                id: "local.escape".to_owned(),
                description: "Escape attempt.".to_owned(),
                source: CatalogSource::MarkdownPath {
                    path: PathBuf::from("../outside.md"),
                },
            },
        )
        .unwrap_err();
    assert!(matches!(error, CatalogError::InvalidSourcePath { .. }));

    fs::write(directory.path().join("not-markdown.txt"), "text").unwrap();
    let error = builder
        .add(
            project_origin(),
            directory.path(),
            CatalogDefinition::Skill {
                id: "local.text".to_owned(),
                description: "Wrong extension.".to_owned(),
                source: CatalogSource::MarkdownPath {
                    path: PathBuf::from("not-markdown.txt"),
                },
            },
        )
        .unwrap_err();
    assert!(matches!(error, CatalogError::InvalidSourcePath { .. }));
}

#[cfg(unix)]
#[test]
fn symlinked_path_cannot_escape_declaring_directory() {
    use std::os::unix::fs::symlink;

    let directory = TempDir::new().unwrap();
    let outside = TempDir::new().unwrap();
    fs::write(outside.path().join("outside.md"), "outside").unwrap();
    symlink(
        outside.path().join("outside.md"),
        directory.path().join("linked.md"),
    )
    .unwrap();
    let mut builder = ContextCatalog::builder();
    let error = builder
        .add(
            project_origin(),
            directory.path(),
            CatalogDefinition::Skill {
                id: "local.link".to_owned(),
                description: "Symlink escape.".to_owned(),
                source: CatalogSource::MarkdownPath {
                    path: PathBuf::from("linked.md"),
                },
            },
        )
        .unwrap_err();
    assert!(matches!(error, CatalogError::SourceEscapesBase { .. }));
}

#[cfg(unix)]
#[test]
fn non_utf8_source_paths_fail_before_manifest_serialization() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let directory = TempDir::new().unwrap();
    let mut builder = ContextCatalog::builder();
    let error = builder
        .add(
            project_origin(),
            directory.path(),
            CatalogDefinition::Skill {
                id: "local.non-utf8".to_owned(),
                description: "Invalid source path.".to_owned(),
                source: CatalogSource::MarkdownPath {
                    path: PathBuf::from(OsString::from_vec(vec![0xff, b'.', b'm', b'd'])),
                },
            },
        )
        .unwrap_err();
    assert!(matches!(error, CatalogError::InvalidSourcePath { .. }));
}

#[test]
fn duplicate_reserved_unknown_and_wrong_kind_references_fail_closed() {
    let mut builder = ContextCatalog::builder();
    let definition = inline_skill("local.review", "Review.");
    builder
        .add(project_origin(), ".", definition.clone())
        .unwrap();
    assert!(matches!(
        builder.add(project_origin(), ".", definition),
        Err(CatalogError::DuplicateId(id)) if id == "local.review"
    ));
    assert!(matches!(
        builder.add(
            project_origin(),
            ".",
            inline_skill("roba.reserved", "No."),
        ),
        Err(CatalogError::ReservedId(id)) if id == "roba.reserved"
    ));

    let mut missing = ContextCatalog::builder();
    missing
        .add(
            project_origin(),
            ".",
            CatalogDefinition::Agent {
                id: "local.agent".to_owned(),
                description: "Agent.".to_owned(),
                source: inline("Act."),
                default_skills: vec!["local.missing".to_owned()],
            },
        )
        .unwrap();
    assert!(matches!(
        missing.build(),
        Err(CatalogError::UnknownReference(id)) if id == "local.missing"
    ));

    let mut wrong = ContextCatalog::builder();
    wrong
        .add(
            project_origin(),
            ".",
            CatalogDefinition::Agent {
                id: "local.agent".to_owned(),
                description: "Agent.".to_owned(),
                source: inline("Act."),
                default_skills: Vec::new(),
            },
        )
        .unwrap();
    wrong
        .add(
            project_origin(),
            ".",
            CatalogDefinition::Prompt {
                id: "local.prompt".to_owned(),
                description: "Prompt.".to_owned(),
                source: inline("Use it."),
                requires: vec!["local.agent".to_owned()],
                arguments: Vec::new(),
            },
        )
        .unwrap();
    assert!(matches!(
        wrong.build(),
        Err(CatalogError::WrongArtifactKind { id, .. }) if id == "local.agent"
    ));
}

#[test]
fn prompt_arguments_are_strict_and_bounded() {
    let catalog = ContextCatalog::builtins();
    assert!(matches!(
        catalog.render_prompt("roba.issue-worker", &BTreeMap::new()),
        Err(CatalogError::MissingPromptArgument { argument, .. }) if argument == "issue"
    ));
    assert!(matches!(
        catalog.render_prompt(
            "roba.issue-worker",
            &BTreeMap::from([("extra".to_owned(), "value".to_owned())]),
        ),
        Err(CatalogError::UnknownPromptArgument { argument, .. }) if argument == "extra"
    ));

    let mut malformed = ContextCatalog::builder();
    malformed
        .add(
            project_origin(),
            ".",
            CatalogDefinition::Prompt {
                id: "local.prompt".to_owned(),
                description: "Malformed prompt.".to_owned(),
                source: inline("Do {{missing}}."),
                requires: Vec::new(),
                arguments: Vec::new(),
            },
        )
        .unwrap();
    assert!(matches!(
        malformed.build(),
        Err(CatalogError::UndeclaredPromptPlaceholder { argument, .. })
            if argument == "missing"
    ));
}

#[test]
fn serde_rejects_unknown_fields_and_debug_redacts_inline_source() {
    let value = json!({
        "kind": "skill",
        "id": "local.review",
        "description": "Review.",
        "source": {"kind": "inline", "content": "secret body"},
        "unknown": true,
    });
    assert!(serde_json::from_value::<CatalogDefinition>(value).is_err());

    let source = inline("secret body");
    let debug = format!("{source:?}");
    assert!(!debug.contains("secret body"));
    assert!(debug.contains("11"));
}

#[test]
fn selection_is_deterministic_and_rejects_duplicates() {
    let catalog = ContextCatalog::builtins();
    let spec = CatalogSelectionSpec {
        agent: "roba.repo-worker".to_owned(),
        skills: vec!["roba.repository-change".to_owned()],
        prompts: vec!["roba.issue-worker".to_owned()],
    };
    let first = catalog.select(&spec).unwrap();
    let second = catalog.select(&spec).unwrap();
    assert_eq!(first, second);
    assert_eq!(first.skills.len(), 1);

    let duplicate = CatalogSelectionSpec {
        skills: vec![
            "roba.repository-change".to_owned(),
            "roba.repository-change".to_owned(),
        ],
        ..spec
    };
    assert!(matches!(
        catalog.select(&duplicate),
        Err(CatalogError::DuplicateReference { id, .. })
            if id == "roba.repository-change"
    ));
}
