//! Static, fail-closed MCP extension composition.

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use roba_core::is_valid_provider_mcp_name;
use serde::{Deserialize, Serialize};
use tower_mcp::schemars::{self, JsonSchema};
use tower_mcp::{McpRouter, MergeConflicts};

use crate::contract::{AgentConfiguration, AgentTerminalState, OperationId};

/// Maximum time the host permits one extension lifecycle callback to run.
pub const MAX_EXTENSION_HOOK_TIMEOUT: Duration = Duration::from_secs(30);

/// Boxed asynchronous extension callback.
pub type AgentExtensionFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

/// Exact, immutable identity and policy of one admitted operation.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentExtensionOperation {
    pub operation_id: OperationId,
    pub configuration: AgentConfiguration,
    pub admitted_at_unix_ms: Option<u64>,
}

/// Compact, content-free evidence returned by an extension lifecycle hook.
///
/// The agent journal bounds and normalizes both strings before publication.
/// Full extension state belongs in an extension-owned MCP resource.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentExtensionChange {
    pub fingerprint: String,
    pub summary: String,
}

/// Lifecycle boundary associated with extension event evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AgentExtensionHookPhase {
    Admitted,
    Started,
    Tick,
    Settling,
    Settled,
}

impl AgentExtensionChange {
    /// Construct compact change evidence for the agent-wide event journal.
    pub fn new(fingerprint: impl Into<String>, summary: impl Into<String>) -> Self {
        Self {
            fingerprint: fingerprint.into(),
            summary: summary.into(),
        }
    }
}

/// Optional lifecycle observer attached to one static MCP extension.
///
/// Roba invokes callbacks outside the agent control lock, serializes ticks for
/// each extension, catches panics, applies a hard timeout, and drains every
/// callback before agent-level settlement. Returning `None` emits no event.
pub trait AgentExtensionLifecycle: Send + Sync + 'static {
    /// Polling interval while an operation is running. `None` disables ticks.
    fn poll_interval(&self) -> Option<Duration> {
        None
    }

    /// Requested callback timeout, capped by [`MAX_EXTENSION_HOOK_TIMEOUT`].
    fn hook_timeout(&self) -> Duration {
        MAX_EXTENSION_HOOK_TIMEOUT
    }

    /// Capture state after admission and before provider work starts.
    fn operation_admitted(
        &self,
        _operation: AgentExtensionOperation,
    ) -> AgentExtensionFuture<Option<AgentExtensionChange>> {
        Box::pin(async { None })
    }

    /// Observe that the finite provider run has started.
    fn operation_started(
        &self,
        _operation: AgentExtensionOperation,
    ) -> AgentExtensionFuture<Option<AgentExtensionChange>> {
        Box::pin(async { None })
    }

    /// Perform one non-overlapping periodic observation.
    fn observation_tick(
        &self,
        _operation: AgentExtensionOperation,
    ) -> AgentExtensionFuture<Option<AgentExtensionChange>> {
        Box::pin(async { None })
    }

    /// Perform final work after the core run and periodic observer have drained.
    fn operation_settling(
        &self,
        _operation: AgentExtensionOperation,
        _terminal: AgentTerminalState,
    ) -> AgentExtensionFuture<Option<AgentExtensionChange>> {
        Box::pin(async { None })
    }

    /// Record the terminal disposition before agent settlement is published.
    fn operation_settled(
        &self,
        _operation: AgentExtensionOperation,
        _terminal: AgentTerminalState,
    ) -> AgentExtensionFuture<Option<AgentExtensionChange>> {
        Box::pin(async { None })
    }

    /// Release host-lifetime state after active operation work has drained.
    fn host_shutdown(&self) -> AgentExtensionFuture<()> {
        Box::pin(async {})
    }
}

/// The role-specific MCP surface being composed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentExtensionProjection {
    /// Operator, CLI, REPL, and automation access.
    Control,
    /// Least-authority access injected into the active provider process.
    Provider,
}

impl fmt::Display for AgentExtensionProjection {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Control => formatter.write_str("control"),
            Self::Provider => formatter.write_str("provider"),
        }
    }
}

/// One named pair of role-specific Tower MCP capability fragments.
///
/// The two routers are independent capability bags. Nothing registered on the
/// control router is copied into the provider router. Router-level settings on
/// either fragment, including server identity and session state, are ignored
/// when Tower merges the fragment into Roba's fresh projection root.
#[derive(Clone)]
pub struct AgentExtension {
    name: Arc<str>,
    control: McpRouter,
    provider: McpRouter,
    provider_tools: Vec<String>,
    lifecycle: Option<Arc<dyn AgentExtensionLifecycle>>,
}

impl AgentExtension {
    /// Define one extension with explicit control and provider fragments.
    pub fn new(name: impl Into<String>, control: McpRouter, provider: McpRouter) -> Self {
        Self {
            name: Arc::from(name.into()),
            control,
            provider,
            provider_tools: Vec::new(),
            lifecycle: None,
        }
    }

    /// Attach one lifecycle observer to this extension.
    pub fn with_lifecycle(mut self, lifecycle: Arc<dyn AgentExtensionLifecycle>) -> Self {
        self.lifecycle = Some(lifecycle);
        self
    }

    /// Declare one provider-fragment tool for exact provider launch approval.
    ///
    /// This does not copy a control tool into the provider projection. The
    /// named tool must already be part of the explicit provider router.
    pub fn try_provider_tool(
        mut self,
        name: impl Into<String>,
    ) -> Result<Self, AgentExtensionManifestError> {
        let name = name.into();
        if !is_valid_provider_mcp_name(&name) {
            return Err(AgentExtensionManifestError { name });
        }
        self.provider_tools.push(name);
        self.provider_tools.sort_unstable();
        self.provider_tools.dedup();
        Ok(self)
    }

    /// Stable diagnostic name of this extension.
    pub fn name(&self) -> &str {
        &self.name
    }
}

impl fmt::Debug for AgentExtension {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentExtension")
            .field("name", &self.name)
            .field("control", &self.control)
            .field("provider", &self.provider)
            .field("provider_tools", &self.provider_tools)
            .field("has_lifecycle", &self.lifecycle.is_some())
            .finish()
    }
}

/// Immutable aggregate of installed agent extensions.
///
/// [`Self::try_with`] rejects collisions against every previously installed
/// fragment. [`crate::AgentInstance::new_with_extensions`] performs the final
/// preflight against Roba's built-in control and provider projections before
/// the instance can start work or bind its private provider endpoint.
#[derive(Clone)]
pub struct AgentExtensions {
    entries: Arc<[AgentExtension]>,
    control: McpRouter,
    provider: McpRouter,
    provider_tools: Arc<[String]>,
    lifecycles: Arc<[AgentExtensionLifecycleRegistration]>,
}

#[derive(Clone)]
pub(crate) struct AgentExtensionLifecycleRegistration {
    pub(crate) name: Arc<str>,
    pub(crate) lifecycle: Arc<dyn AgentExtensionLifecycle>,
}

impl AgentExtensions {
    /// Add one extension, failing rather than replacing an existing MCP
    /// capability with the same identity.
    pub fn try_with(self, extension: AgentExtension) -> Result<Self, AgentExtensionError> {
        let control = self
            .control
            .try_merge(extension.control.clone())
            .map_err(|conflicts| {
                AgentExtensionError::new(
                    AgentExtensionProjection::Control,
                    extension.name(),
                    conflicts,
                )
            })?;
        let provider = self
            .provider
            .try_merge(extension.provider.clone())
            .map_err(|conflicts| {
                AgentExtensionError::new(
                    AgentExtensionProjection::Provider,
                    extension.name(),
                    conflicts,
                )
            })?;

        let mut entries = self.entries.to_vec();
        let mut provider_tools = self.provider_tools.to_vec();
        let mut lifecycles = self.lifecycles.to_vec();
        provider_tools.extend(extension.provider_tools.iter().cloned());
        provider_tools.sort_unstable();
        provider_tools.dedup();
        if let Some(lifecycle) = &extension.lifecycle {
            lifecycles.push(AgentExtensionLifecycleRegistration {
                name: Arc::clone(&extension.name),
                lifecycle: Arc::clone(lifecycle),
            });
        }
        entries.push(extension);

        Ok(Self {
            entries: entries.into(),
            control,
            provider,
            provider_tools: provider_tools.into(),
            lifecycles: lifecycles.into(),
        })
    }

    pub(crate) fn preflight(
        &self,
        control: McpRouter,
        provider: McpRouter,
    ) -> Result<(), AgentExtensionError> {
        self.compose_entries(control, AgentExtensionProjection::Control)?;
        self.compose_entries(provider, AgentExtensionProjection::Provider)?;
        Ok(())
    }

    pub(crate) fn merge_control(&self, base: McpRouter) -> McpRouter {
        base.try_merge(self.control.clone())
            .unwrap_or_else(|error| {
                panic!("validated control extension composition changed: {error}")
            })
    }

    pub(crate) fn merge_provider(&self, base: McpRouter) -> McpRouter {
        base.try_merge(self.provider.clone())
            .unwrap_or_else(|error| {
                panic!("validated provider extension composition changed: {error}")
            })
    }

    pub(crate) fn provider_tool_names(&self) -> &[String] {
        &self.provider_tools
    }

    pub(crate) fn lifecycle_registrations(&self) -> &[AgentExtensionLifecycleRegistration] {
        &self.lifecycles
    }

    fn compose_entries(
        &self,
        mut base: McpRouter,
        projection: AgentExtensionProjection,
    ) -> Result<McpRouter, AgentExtensionError> {
        for extension in self.entries.iter() {
            let fragment = match projection {
                AgentExtensionProjection::Control => extension.control.clone(),
                AgentExtensionProjection::Provider => extension.provider.clone(),
            };
            base = base.try_merge(fragment).map_err(|conflicts| {
                AgentExtensionError::new(projection, extension.name(), conflicts)
            })?;
        }
        Ok(base)
    }
}

impl Default for AgentExtensions {
    fn default() -> Self {
        Self {
            entries: Arc::from([]),
            control: McpRouter::new(),
            provider: McpRouter::new(),
            provider_tools: Arc::from([]),
            lifecycles: Arc::from([]),
        }
    }
}

impl fmt::Debug for AgentExtensions {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AgentExtensions")
            .field("entries", &self.entries)
            .field("provider_tools", &self.provider_tools)
            .field("lifecycle_count", &self.lifecycles.len())
            .finish_non_exhaustive()
    }
}

/// A named extension collided with an earlier or built-in MCP capability.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentExtensionError {
    projection: AgentExtensionProjection,
    extension: String,
    conflicts: MergeConflicts,
}

impl AgentExtensionError {
    fn new(
        projection: AgentExtensionProjection,
        extension: impl Into<String>,
        conflicts: MergeConflicts,
    ) -> Self {
        Self {
            projection,
            extension: extension.into(),
            conflicts,
        }
    }

    /// Projection in which the collision occurred.
    pub fn projection(&self) -> AgentExtensionProjection {
        self.projection
    }

    /// Incoming extension whose capabilities collided.
    pub fn extension(&self) -> &str {
        &self.extension
    }

    /// Exact Tower MCP capability conflicts.
    pub fn conflicts(&self) -> &MergeConflicts {
        &self.conflicts
    }
}

impl fmt::Display for AgentExtensionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "cannot compose extension '{}' into the {} projection: {}",
            self.extension, self.projection, self.conflicts
        )
    }
}

impl std::error::Error for AgentExtensionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.conflicts)
    }
}

/// A provider tool manifest name cannot be represented safely by MCP clients
/// and provider-native allowlists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentExtensionManifestError {
    name: String,
}

impl AgentExtensionManifestError {
    /// Rejected manifest entry.
    pub fn name(&self) -> &str {
        &self.name
    }
}

impl fmt::Display for AgentExtensionManifestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "invalid provider MCP tool name: {:?}", self.name)
    }
}

impl std::error::Error for AgentExtensionManifestError {}
