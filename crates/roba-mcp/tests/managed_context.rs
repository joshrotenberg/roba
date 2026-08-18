use std::collections::HashMap;

use roba_context::{
    CatalogDefinition, CatalogOrigin, CatalogOriginKind, CatalogSelectionSpec, CatalogSource,
    ContextCatalog,
};
use roba_core::{
    AgentSpec, EventSink, FailureKind, Provider, ProviderCapabilities, ProviderError,
    ProviderFuture, ProviderId, Roba, RunSpec, TurnRequest,
};
use roba_mcp::{
    AGENT_CONTEXT_ENTRY_TEMPLATE, AGENT_CONTEXT_URI, AgentExtensions, AgentInstance,
    ContextFreshness, ContextKind, MANAGED_CONTEXT_ARTIFACT_TEMPLATE, MANAGED_CONTEXT_CATALOG_URI,
    ManagedContextArtifact, ManagedContextCatalogSnapshot, ManagedContextError, OperationId,
    agent_router, connect_in_process, managed_context_extension,
};
use tower_mcp::{ChannelTransport, McpClient};

struct InertProvider;

impl Provider for InertProvider {
    fn id(&self) -> ProviderId {
        provider_id()
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            resume: true,
            streaming: true,
            read_only: true,
            workspace_write: true,
            full_auto: true,
            timeout: true,
            ..Default::default()
        }
    }

    fn validate(&self, _request: &TurnRequest) -> Result<(), ProviderError> {
        Ok(())
    }

    fn execute<'a>(
        &'a self,
        _request: TurnRequest,
        _events: &'a dyn EventSink,
    ) -> ProviderFuture<'a> {
        Box::pin(async {
            Err(ProviderError::new(
                FailureKind::Provider,
                "test provider should not execute",
            ))
        })
    }
}

fn provider_id() -> ProviderId {
    ProviderId::new("managed-context-test").expect("static provider id is valid")
}

fn selected_catalog() -> (ContextCatalog, roba_context::CatalogSelection) {
    let mut builder = ContextCatalog::builder_with_builtins();
    builder
        .add(
            CatalogOrigin::new(CatalogOriginKind::Project, "fixture"),
            ".",
            CatalogDefinition::Prompt {
                id: "local.unselected".to_owned(),
                description: "An available but disabled prompt.".to_owned(),
                source: CatalogSource::Inline {
                    content: "UNSELECTED PRIVATE PROMPT BODY".to_owned(),
                },
                requires: Vec::new(),
                arguments: Vec::new(),
            },
        )
        .unwrap();
    let catalog = builder.build().unwrap();
    let selection = catalog
        .select(&CatalogSelectionSpec {
            agent: "roba.repo-worker".to_owned(),
            skills: Vec::new(),
            prompts: vec!["roba.issue-worker".to_owned()],
        })
        .expect("shipped selection is valid");
    (catalog, selection)
}

fn agent(
    catalog: ContextCatalog,
    selection: Option<roba_context::CatalogSelection>,
) -> AgentInstance {
    let mut runtime = Roba::new();
    runtime
        .register(InertProvider)
        .expect("test provider registration succeeds");
    let extension = managed_context_extension(catalog, selection)
        .expect("managed context contribution compiles");
    let extensions = AgentExtensions::default()
        .try_with(extension)
        .expect("managed context projection is collision free");
    AgentInstance::new_with_extensions(
        runtime,
        RunSpec::suspended(AgentSpec::new(provider_id())),
        extensions,
    )
    .expect("managed context agent builds")
}

#[tokio::test]
async fn selected_agent_skills_prompts_and_operator_resources_share_one_catalog() {
    let (catalog, selection) = selected_catalog();
    let agent = agent(catalog, Some(selection));
    let entries = &agent.context_plan().manifest().entries;
    assert_eq!(entries.len(), 2);
    let role = entries
        .iter()
        .find(|entry| entry.id == "roba.repo-worker")
        .expect("selected agent joins the context plan");
    assert_eq!(role.kind, ContextKind::Instruction);
    assert_eq!(role.freshness, ContextFreshness::FreshSession);
    assert!(role.required);
    let skill = entries
        .iter()
        .find(|entry| entry.id == "roba.repository-change")
        .expect("transitive selected skill joins the context plan");
    assert_eq!(skill.kind, ContextKind::Reference);
    assert!(!skill.required);
    assert!(
        entries.iter().all(|entry| entry.id != "roba.issue-worker"),
        "a reusable operation directive must not become standing context"
    );

    let client = connect_in_process(agent.clone())
        .await
        .expect("control client initializes");
    let prompts = client.list_prompts().await.expect("prompts/list succeeds");
    assert_eq!(prompts.prompts.len(), 1);
    assert_eq!(prompts.prompts[0].name, "roba.issue-worker");
    assert_eq!(prompts.prompts[0].arguments.len(), 1);
    assert!(prompts.prompts[0].arguments[0].required);
    assert!(
        client.get_prompt("local.unselected", None).await.is_err(),
        "available but unselected prompts must not be dispatched"
    );

    let prompt = client
        .get_prompt(
            "roba.issue-worker",
            Some(HashMap::from([("issue".to_owned(), "#514".to_owned())])),
        )
        .await
        .expect("prompts/get renders through the catalog");
    let rendered = prompt
        .first_message_text()
        .expect("managed prompt renders one text message");
    assert!(rendered.contains("issue #514"));
    assert!(!rendered.contains("{{issue}}"));
    assert!(
        client.get_prompt("roba.issue-worker", None).await.is_err(),
        "missing declared arguments must fail in the catalog renderer"
    );

    let inventory = client
        .read_resource(MANAGED_CONTEXT_CATALOG_URI)
        .await
        .expect("catalog resource is readable");
    let inventory: ManagedContextCatalogSnapshot = serde_json::from_str(
        inventory
            .first_text()
            .expect("catalog resource contains JSON text"),
    )
    .expect("catalog resource matches the public type");
    assert_eq!(inventory.catalog.entries.len(), 4);
    assert_eq!(
        inventory
            .selection
            .as_ref()
            .map(|selection| selection.agent.id()),
        Some("roba.repo-worker")
    );
    let inventory_json = serde_json::to_string(&inventory).unwrap();
    assert!(!inventory_json.contains("You are a Roba-managed repository worker"));
    assert!(!inventory_json.contains("UNSELECTED PRIVATE PROMPT BODY"));

    let artifact_uri = "roba://context/catalog/artifact?id=roba.repository-change";
    let artifact = client
        .read_resource(artifact_uri)
        .await
        .expect("explicit operator artifact read succeeds");
    let artifact: ManagedContextArtifact = serde_json::from_str(
        artifact
            .first_text()
            .expect("artifact resource contains JSON text"),
    )
    .expect("artifact resource matches the public type");
    assert_eq!(artifact.entry.id(), "roba.repository-change");
    assert!(artifact.content.contains("For repository changes"));
    assert!(!format!("{artifact:?}").contains("For repository changes"));

    let provider = McpClient::connect(ChannelTransport::new(agent_router(
        agent,
        OperationId::new(1),
    )))
    .await
    .expect("provider projection connects");
    provider
        .initialize("managed-context-provider", "1")
        .await
        .expect("provider projection initializes");
    assert!(provider.list_prompts().await.unwrap().prompts.is_empty());
    let resources = provider.list_resources().await.unwrap();
    assert_eq!(
        resources
            .resources
            .iter()
            .map(|resource| resource.uri.as_str())
            .collect::<Vec<_>>(),
        [AGENT_CONTEXT_URI]
    );
    let templates = provider.list_resource_templates().await.unwrap();
    assert_eq!(
        templates
            .resource_templates
            .iter()
            .map(|resource| resource.uri_template.as_str())
            .collect::<Vec<_>>(),
        [AGENT_CONTEXT_ENTRY_TEMPLATE]
    );
    assert_ne!(
        MANAGED_CONTEXT_ARTIFACT_TEMPLATE,
        AGENT_CONTEXT_ENTRY_TEMPLATE
    );
}

#[tokio::test]
async fn available_builtins_do_not_select_or_inject_managed_context() {
    let agent = agent(ContextCatalog::builtins(), None);
    assert!(agent.context_plan().manifest().entries.is_empty());
    let client = connect_in_process(agent)
        .await
        .expect("ambient-only control client initializes");
    assert!(client.list_prompts().await.unwrap().prompts.is_empty());
    let inventory = client
        .read_resource(MANAGED_CONTEXT_CATALOG_URI)
        .await
        .expect("available catalog remains inspectable");
    let inventory: ManagedContextCatalogSnapshot = serde_json::from_str(
        inventory
            .first_text()
            .expect("catalog resource contains JSON text"),
    )
    .unwrap();
    assert_eq!(inventory.catalog.entries.len(), 3);
    assert!(inventory.selection.is_none());
}

#[test]
fn a_selection_from_another_catalog_fails_before_agent_construction() {
    fn local_catalog(body: &str) -> ContextCatalog {
        let mut builder = ContextCatalog::builder();
        builder
            .add(
                CatalogOrigin::new(CatalogOriginKind::Project, "fixture"),
                ".",
                CatalogDefinition::Agent {
                    id: "local.worker".to_owned(),
                    description: "Local worker.".to_owned(),
                    source: CatalogSource::Inline {
                        content: body.to_owned(),
                    },
                    default_skills: Vec::new(),
                },
            )
            .unwrap();
        builder.build().unwrap()
    }

    let original = local_catalog("original body");
    let selection = original
        .select(&CatalogSelectionSpec {
            agent: "local.worker".to_owned(),
            skills: Vec::new(),
            prompts: Vec::new(),
        })
        .unwrap();
    let error = managed_context_extension(local_catalog("changed body"), Some(selection))
        .expect_err("catalog and selection must be one coherent startup result");
    assert!(matches!(error, ManagedContextError::SelectionMismatch));
}
