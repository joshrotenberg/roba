//! roba-core: the clap-free engine for one finite agent run.
//!
//! This crate owns the provider-neutral run contract plus the compatibility
//! engine used by the current CLI:
//!
//! - [`run`] and [`provider`] -- provider-neutral specifications, outcomes,
//!   events, and the provider-turn boundary. A prompt-less [`RunSpec`] is
//!   explicitly suspended and causes no provider work.
//! - [`lifecycle`] -- one process-local root run with observation, bounded
//!   event replay, follow-ups at resumable turn boundaries, cancellation, and
//!   terminal settlement.
//! - [`providers`] -- built-in Claude and Codex adapters behind the same run
//!   model. Claude also remains the compatibility provider for the legacy CLI.
//! - [`engine`] -- the pre-pivot config-and-run seam. [`engine::run`] remains
//!   available while the CLI migrates, and delegates execution through
//!   [`struct@ClaudeProvider`].
//! - [`session`] -- [`session::apply_session`], the single `Config ->
//!   QueryCommand` mapper the engine feeds, plus the permission/notice
//!   composition it consumes.
//!
//! The `roba` binary depends on this crate. Prompt composition, legacy
//! profile/env layering, live streaming display, output formatting, and
//! exit-code classification stay in the CLI crate.

pub mod engine;
pub mod lifecycle;
pub mod provider;
pub mod providers;
pub mod run;
pub mod runtime;
pub mod session;

pub use lifecycle::{
    MAX_PENDING_FOLLOW_UPS, RUN_EVENT_CAPACITY, Run, RunControlError, RunEventPage, RunEventRecord,
    RunEventSubscription, RunEventSubscriptionItem, RunHandle, RunSnapshot,
};
pub use provider::{
    EventSink, NoopEventSink, Provider, ProviderCapabilities, ProviderError, ProviderEvent,
    ProviderFuture, ProviderLaunchContext, ProviderMcpEndpoint, ProviderMcpEndpointError,
    execute_turn, execute_turn_with_launch_context, is_valid_provider_mcp_name,
};
pub use providers::claude::ClaudeProvider;
pub use providers::codex::CodexProvider;
pub use run::{
    AgentSpec, ContextSpec, Cost, Effort, ExecutionSpec, FailureKind, LimitSpec, PermissionPolicy,
    Prompt, PromptError, ProviderId, ProviderIdError, RunEvent, RunFailure, RunFailureDetails,
    RunOutcome, RunSpec, RunSpecError, RunState, SessionHandle, SessionSpec, TokenUsage,
    ToolPolicy, TurnRequest,
};
pub use runtime::{Roba, RuntimeError};
