use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use roba_core::{
    AgentSpec, EventSink, Provider, ProviderCapabilities, ProviderError, ProviderFuture,
    ProviderId, Roba, RunOutcome, RunSpec, TurnRequest,
};
use roba_mcp::{
    AGENT_CONTEXT_ENTRY_TEMPLATE, AGENT_CONTEXT_URI, AGENT_EVENTS_TEMPLATE, AGENT_EVENTS_URI,
    AGENT_INTERRUPT_TOOL, AGENT_RESOURCE_URI, AGENT_SHUTDOWN_TOOL, AGENT_STEER_TOOL,
    AGENT_TURN_TOOL, AgentBuildError, AgentExtension, AgentExtensionProjection, AgentExtensions,
    AgentInstance, ContextAudience, ContextDelivery, ContextEntrySpec, ContextFreshness,
    ContextKind, ContextOrigin, ContextOriginKind, ContextPhase, ContextPlanError,
    ContextPrecedence, ContextScope, ContextSensitivity, OperationId, ROBA_CONTEXT_MANIFEST_TOOL,
    ROBA_CONTEXT_READ_TOOL, ROBA_SELF_TOOL, ShutdownInput, agent_router, connect_in_process,
};
use tower_mcp::{
    CallToolResult, ChannelTransport, McpClient, McpRouter, MergeConflictKind, ResourceBuilder,
    ToolBuilder,
};

#[derive(Clone)]
struct CountingProvider {
    calls: Arc<AtomicUsize>,
}

impl Provider for CountingProvider {
    fn id(&self) -> ProviderId {
        fake_provider_id()
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            resume: true,
            streaming: true,
            read_only: true,
            workspace_write: true,
            full_auto: true,
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
        self.calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Ok(test_outcome()) })
    }
}

fn test_outcome() -> RunOutcome {
    RunOutcome {
        output: "unused".to_owned(),
        session: None,
        usage: None,
        cost: None,
        duration_ms: None,
        provider_turns: Some(1),
        structured_output: None,
    }
}

fn fake_provider_id() -> ProviderId {
    ProviderId::new("extension-test").expect("static provider id is valid")
}

fn runtime(calls: Arc<AtomicUsize>) -> Roba {
    let mut runtime = Roba::new();
    runtime
        .register(CountingProvider { calls })
        .expect("test provider registration succeeds");
    runtime
}

fn template() -> RunSpec {
    RunSpec::suspended(AgentSpec::new(fake_provider_id()))
}

fn tool_router(name: &str) -> McpRouter {
    let tool = ToolBuilder::new(name)
        .description("extension test tool")
        .handler(|_input: ShutdownInput| async move { Ok(CallToolResult::text("ok")) })
        .build();
    McpRouter::new().tool(tool)
}

fn resource_router(uri: &str) -> McpRouter {
    McpRouter::new().resource(
        ResourceBuilder::new(uri)
            .name("extension test resource")
            .text("ok"),
    )
}

fn tool_and_resource_router(tool: &str, uri: &str) -> McpRouter {
    tool_router(tool).resource(
        ResourceBuilder::new(uri)
            .name("extension test resource")
            .text("ok"),
    )
}

fn build_with(
    extensions: AgentExtensions,
    calls: Arc<AtomicUsize>,
) -> Result<AgentInstance, AgentBuildError> {
    AgentInstance::new_with_extensions(runtime(calls), template(), extensions)
}

fn extension_error(error: AgentBuildError) -> roba_mcp::AgentExtensionError {
    match error {
        AgentBuildError::Extension(error) => error,
        other => panic!("expected extension error, got {other:?}"),
    }
}

#[test]
fn built_in_control_collision_fails_before_provider_work() {
    let calls = Arc::new(AtomicUsize::new(0));
    let extensions = AgentExtensions::default()
        .try_with(AgentExtension::new(
            "replace-turn",
            tool_router(AGENT_TURN_TOOL),
            McpRouter::new(),
        ))
        .expect("the fragment does not collide with another extension");

    let error = extension_error(
        build_with(extensions, Arc::clone(&calls))
            .err()
            .expect("built-in collision must fail construction"),
    );
    assert_eq!(error.projection(), AgentExtensionProjection::Control);
    assert_eq!(error.extension(), "replace-turn");
    assert_eq!(error.conflicts().conflicts().len(), 1);
    assert_eq!(
        error.conflicts().conflicts()[0].kind,
        MergeConflictKind::Tool
    );
    assert_eq!(error.conflicts().conflicts()[0].name, AGENT_TURN_TOOL);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[test]
fn built_in_provider_self_collision_fails_before_provider_work() {
    let calls = Arc::new(AtomicUsize::new(0));
    let extensions = AgentExtensions::default()
        .try_with(
            AgentExtension::new(
                "replace-self",
                McpRouter::new(),
                tool_router(ROBA_SELF_TOOL),
            )
            .try_provider_tool(ROBA_SELF_TOOL)
            .unwrap(),
        )
        .expect("the fragment does not collide with another extension");

    let error = extension_error(
        build_with(extensions, Arc::clone(&calls))
            .err()
            .expect("built-in collision must fail construction"),
    );
    assert_eq!(error.projection(), AgentExtensionProjection::Provider);
    assert_eq!(error.extension(), "replace-self");
    assert_eq!(
        error.conflicts().conflicts()[0].kind,
        MergeConflictKind::Tool
    );
    assert_eq!(error.conflicts().conflicts()[0].name, ROBA_SELF_TOOL);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[test]
fn built_in_control_context_collision_fails_before_provider_work() {
    let calls = Arc::new(AtomicUsize::new(0));
    let extensions = AgentExtensions::default()
        .try_with(AgentExtension::new(
            "replace-control-context",
            resource_router(AGENT_CONTEXT_URI),
            McpRouter::new(),
        ))
        .expect("the fragment does not collide with another extension");

    let error = extension_error(
        build_with(extensions, Arc::clone(&calls))
            .err()
            .expect("built-in context collision must fail construction"),
    );
    assert_eq!(error.projection(), AgentExtensionProjection::Control);
    assert_eq!(error.extension(), "replace-control-context");
    assert_eq!(
        error.conflicts().conflicts()[0].kind,
        MergeConflictKind::Resource
    );
    assert_eq!(error.conflicts().conflicts()[0].name, AGENT_CONTEXT_URI);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[test]
fn built_in_provider_context_collision_fails_before_provider_work() {
    let calls = Arc::new(AtomicUsize::new(0));
    let extensions = AgentExtensions::default()
        .try_with(AgentExtension::new(
            "replace-provider-context",
            McpRouter::new(),
            resource_router(AGENT_CONTEXT_URI),
        ))
        .expect("the fragment does not collide with another extension");

    let error = extension_error(
        build_with(extensions, Arc::clone(&calls))
            .err()
            .expect("built-in context collision must fail construction"),
    );
    assert_eq!(error.projection(), AgentExtensionProjection::Provider);
    assert_eq!(error.extension(), "replace-provider-context");
    assert_eq!(
        error.conflicts().conflicts()[0].kind,
        MergeConflictKind::Resource
    );
    assert_eq!(error.conflicts().conflicts()[0].name, AGENT_CONTEXT_URI);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[test]
fn extension_tool_and_static_resource_collisions_fail_closed() {
    let first = AgentExtension::new(
        "first",
        tool_and_resource_router("git.snapshot", "git://snapshot"),
        McpRouter::new(),
    );
    let second = AgentExtension::new(
        "second",
        tool_and_resource_router("git.snapshot", "git://snapshot"),
        McpRouter::new(),
    );
    let installed = AgentExtensions::default()
        .try_with(first)
        .expect("first extension installs");
    let error = installed
        .try_with(second)
        .expect_err("incoming collisions must not replace existing handlers");

    assert_eq!(error.projection(), AgentExtensionProjection::Control);
    assert_eq!(error.extension(), "second");
    let conflicts = error.conflicts().conflicts();
    assert_eq!(conflicts.len(), 2);
    assert!(conflicts.iter().any(|conflict| {
        conflict.kind == MergeConflictKind::Tool && conflict.name == "git.snapshot"
    }));
    assert!(conflicts.iter().any(|conflict| {
        conflict.kind == MergeConflictKind::Resource && conflict.name == "git://snapshot"
    }));
}

#[tokio::test]
async fn the_same_capability_name_is_independent_across_projections() {
    let calls = Arc::new(AtomicUsize::new(0));
    let extensions = AgentExtensions::default()
        .try_with(
            AgentExtension::new(
                "shared-name",
                tool_router("git.snapshot"),
                tool_router("git.snapshot"),
            )
            .try_provider_tool("git.snapshot")
            .unwrap()
            .try_provider_tool("git.snapshot")
            .unwrap(),
        )
        .expect("separate projections do not collide");
    let agent = build_with(extensions, Arc::clone(&calls)).expect("agent preflight succeeds");

    let control = connect_in_process(agent.clone())
        .await
        .expect("control client connects");
    let control_tools = control
        .list_tools()
        .await
        .expect("control discovery succeeds")
        .tools
        .into_iter()
        .map(|tool| tool.name)
        .collect::<Vec<_>>();
    assert!(control_tools.iter().any(|name| name == "git.snapshot"));
    control.shutdown().await.expect("control client shuts down");

    let provider = McpClient::connect(ChannelTransport::new(agent_router(
        agent,
        OperationId::new(1),
    )))
    .await
    .expect("provider client connects");
    provider
        .initialize("extension-provider-test", env!("CARGO_PKG_VERSION"))
        .await
        .expect("provider client initializes");
    let mut provider_tools = provider
        .list_tools()
        .await
        .expect("provider discovery succeeds")
        .tools
        .into_iter()
        .map(|tool| tool.name)
        .collect::<Vec<_>>();
    provider_tools.sort_unstable();
    assert_eq!(
        provider_tools,
        [
            ROBA_CONTEXT_MANIFEST_TOOL,
            ROBA_CONTEXT_READ_TOOL,
            "git.snapshot",
            ROBA_SELF_TOOL,
        ]
    );
    provider
        .shutdown()
        .await
        .expect("provider client shuts down");
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[test]
fn provider_manifest_rejects_allowlist_injection() {
    let error = AgentExtension::new("malicious", McpRouter::new(), McpRouter::new())
        .try_provider_tool("git.snapshot,Bash")
        .expect_err("comma-delimited tool injection must fail before composition");

    assert_eq!(error.name(), "git.snapshot,Bash");
    assert!(error.to_string().contains("invalid provider MCP tool name"));
}

fn extension_context(id: &str, audience: ContextAudience) -> ContextEntrySpec {
    ContextEntrySpec::new(
        id,
        ContextKind::Reference,
        ContextOrigin::new(ContextOriginKind::Extension, "extension-test"),
        ContextPhase::Bootstrap,
        ContextScope::Agent,
        ContextDelivery::McpResource {
            uri: format!("roba://context/entry?id={id}"),
        },
    )
    .audience(audience)
    .precedence(ContextPrecedence::Host)
    .freshness(ContextFreshness::Generation)
    .sensitivity(ContextSensitivity::Public)
}

#[test]
fn extension_context_compiles_into_the_existing_plan_without_prompt_injection() {
    let calls = Arc::new(AtomicUsize::new(0));
    let extension = AgentExtension::new("context", McpRouter::new(), McpRouter::new())
        .with_inline_context(
            extension_context("extension.shared", ContextAudience::Both),
            "shared lazy context",
        )
        .with_inline_context(
            extension_context("extension.operator", ContextAudience::Operator),
            "operator-only context",
        );
    let debug = format!("{extension:?}");
    assert!(debug.contains("extension.shared"));
    assert!(debug.contains("extension.operator"));
    assert!(!debug.contains("shared lazy context"));
    assert!(!debug.contains("operator-only context"));

    let extensions = AgentExtensions::default()
        .try_with(extension)
        .expect("extension installs");
    let agent = build_with(extensions, Arc::clone(&calls)).expect("agent construction succeeds");

    let manifest = agent.context_plan().manifest();
    assert_eq!(
        manifest
            .entries
            .iter()
            .map(|entry| entry.id.as_str())
            .collect::<Vec<_>>(),
        ["extension.shared", "extension.operator"]
    );
    assert_eq!(
        agent.context_plan().material("extension.shared"),
        Some("shared lazy context")
    );
    assert!(template().agent.instructions.is_empty());
    assert!(template().context.project.is_empty());
    assert!(template().context.run.is_empty());

    let provider = agent.context_plan().provider_manifest();
    assert_eq!(provider.entries.len(), 1);
    assert_eq!(provider.entries[0].id, "extension.shared");
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[test]
fn duplicate_extension_context_fails_before_provider_work() {
    let calls = Arc::new(AtomicUsize::new(0));
    let first = AgentExtension::new("first", McpRouter::new(), McpRouter::new())
        .with_inline_context(
            extension_context("extension.duplicate", ContextAudience::Both),
            "first",
        );
    let second =
        AgentExtension::new("second", McpRouter::new(), McpRouter::new()).with_available_context(
            extension_context("extension.duplicate", ContextAudience::Both),
        );
    let extensions = AgentExtensions::default()
        .try_with(first)
        .expect("first extension installs")
        .try_with(second)
        .expect("MCP capabilities do not collide");

    let error = build_with(extensions, Arc::clone(&calls))
        .err()
        .expect("duplicate context must fail construction");
    assert!(matches!(
        error,
        AgentBuildError::ContextPlan(ContextPlanError::DuplicateId(id))
            if id == "extension.duplicate"
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn default_extensions_preserve_exact_base_discovery() {
    let calls = Arc::new(AtomicUsize::new(0));
    let agent = build_with(AgentExtensions::default(), Arc::clone(&calls))
        .expect("base agent construction succeeds");

    let control = connect_in_process(agent.clone())
        .await
        .expect("control client connects");
    let mut control_tools = control
        .list_tools()
        .await
        .expect("control tools list succeeds")
        .tools
        .into_iter()
        .map(|tool| tool.name)
        .collect::<Vec<_>>();
    control_tools.sort_unstable();
    let mut expected_tools = vec![
        AGENT_INTERRUPT_TOOL.to_owned(),
        AGENT_SHUTDOWN_TOOL.to_owned(),
        AGENT_STEER_TOOL.to_owned(),
        AGENT_TURN_TOOL.to_owned(),
    ];
    expected_tools.sort_unstable();
    assert_eq!(control_tools, expected_tools);

    let mut resources = control
        .list_resources()
        .await
        .expect("control resources list succeeds")
        .resources
        .into_iter()
        .map(|resource| resource.uri)
        .collect::<Vec<_>>();
    resources.sort_unstable();
    let mut expected_resources = vec![
        AGENT_CONTEXT_URI.to_owned(),
        AGENT_EVENTS_URI.to_owned(),
        AGENT_RESOURCE_URI.to_owned(),
    ];
    expected_resources.sort_unstable();
    assert_eq!(resources, expected_resources);
    let templates = control
        .list_resource_templates()
        .await
        .expect("control resource templates list succeeds")
        .resource_templates;
    let mut templates = templates
        .into_iter()
        .map(|template| template.uri_template)
        .collect::<Vec<_>>();
    templates.sort_unstable();
    let mut expected_templates = vec![
        AGENT_CONTEXT_ENTRY_TEMPLATE.to_owned(),
        AGENT_EVENTS_TEMPLATE.to_owned(),
    ];
    expected_templates.sort_unstable();
    assert_eq!(templates, expected_templates);
    assert!(
        control
            .list_prompts()
            .await
            .expect("control prompts list succeeds")
            .prompts
            .is_empty()
    );
    control.shutdown().await.expect("control client shuts down");

    let provider = McpClient::connect(ChannelTransport::new(agent_router(
        agent,
        OperationId::new(1),
    )))
    .await
    .expect("provider client connects");
    provider
        .initialize("base-provider-test", env!("CARGO_PKG_VERSION"))
        .await
        .expect("provider client initializes");
    let mut provider_tools = provider
        .list_tools()
        .await
        .expect("provider tools list succeeds")
        .tools
        .into_iter()
        .map(|tool| tool.name)
        .collect::<Vec<_>>();
    provider_tools.sort_unstable();
    assert_eq!(
        provider_tools,
        [
            ROBA_CONTEXT_MANIFEST_TOOL,
            ROBA_CONTEXT_READ_TOOL,
            ROBA_SELF_TOOL,
        ]
    );
    let provider_resources = provider
        .list_resources()
        .await
        .expect("provider resources list succeeds")
        .resources;
    assert_eq!(provider_resources.len(), 1);
    assert_eq!(provider_resources[0].uri, AGENT_CONTEXT_URI);
    let provider_templates = provider
        .list_resource_templates()
        .await
        .expect("provider templates list succeeds")
        .resource_templates;
    assert_eq!(provider_templates.len(), 1);
    assert_eq!(
        provider_templates[0].uri_template,
        AGENT_CONTEXT_ENTRY_TEMPLATE
    );
    provider
        .shutdown()
        .await
        .expect("provider client shuts down");
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[test]
fn a_static_resource_collision_is_attributed_to_the_incoming_extension() {
    let installed = AgentExtensions::default()
        .try_with(AgentExtension::new(
            "first-resource",
            resource_router("git://head"),
            McpRouter::new(),
        ))
        .expect("first extension installs");
    let error = installed
        .try_with(AgentExtension::new(
            "second-resource",
            resource_router("git://head"),
            McpRouter::new(),
        ))
        .expect_err("duplicate static URI must fail");
    assert_eq!(error.extension(), "second-resource");
    assert_eq!(
        error.conflicts().conflicts()[0].kind,
        MergeConflictKind::Resource
    );
    assert_eq!(error.conflicts().conflicts()[0].name, "git://head");
}
