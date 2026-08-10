//! roba-core: the clap-free, side-effect-free run engine.
//!
//! This crate is Roba's resolve-free core. It owns the provider-neutral
//! bounded-run contract plus the compatibility engine used by the current
//! CLI:
//!
//! - [`run`] and [`provider`] -- provider-neutral specifications, outcomes,
//!   events, and the one-turn provider boundary. A prompt-less [`RunSpec`]
//!   is explicitly suspended and causes no provider work.
//! - [`providers`] -- built-in Claude and Codex adapters behind the same run
//!   model. Claude also remains the compatibility provider for the legacy CLI.
//! - [`engine`] -- the pre-pivot config-and-run seam. [`engine::run`] remains
//!   available while the CLI migrates, and delegates execution through
//!   [`struct@ClaudeProvider`].
//! - [`session`] -- [`session::apply_session`], the single `Config ->
//!   QueryCommand` mapper the engine feeds, plus the permission/notice
//!   composition it consumes.
//!
//! The `roba` binary depends on this crate: its `run_ask` resolves the
//! provider-neutral subset of flags/profiles/prompt through [`RobaConfig`] and
//! [`RunSpec`], then adapts that result plus Claude-only compatibility controls
//! to [`engine::Config`]. Prompt composition, legacy profile/env layering,
//! live streaming display, output formatting, and exit-code classification
//! stay in the CLI crate.

pub mod engine;
pub mod lifecycle;
pub mod mission;
pub mod process;
pub mod provider;
pub mod providers;
pub mod resolve;
pub mod run;
pub mod runtime;
pub mod session;

pub use lifecycle::{
    RUN_EVENT_CAPACITY, Run, RunControlError, RunEventPage, RunEventRecord, RunEventSubscription,
    RunEventSubscriptionItem, RunHandle, RunSnapshot, WorkerControl, WorkerSnapshot,
};
pub use mission::{
    MissionArtifact, MissionAuthority, MissionBlocker, MissionClaims, MissionReport,
    MissionReportError, MissionSnapshot, MissionWorkItem, MissionWorkState,
};
pub use process::{
    AuthorityGrantId, AuthorityGrantIdError, CompletionPolicy, MissionPolicy, MissionPolicyError,
    ProcessActionId, ProcessActionIdError, ProcessActionRequest, ProcessActionScope,
    ProcessActionSpec, ProcessCapability, ProcessCapabilityDescriptor, ProcessCapabilityError,
    ProcessCapabilityId, ProcessCapabilityIdError, ProcessControl, ProcessControlError,
    ProcessFuture,
};
pub use provider::{
    EventSink, NoopEventSink, Provider, ProviderCapabilities, ProviderContext, ProviderError,
    ProviderFuture, ProviderMcpEndpoint, execute_turn,
};
pub use providers::claude::ClaudeProvider;
pub use providers::codex::CodexProvider;
pub use resolve::{ConfigLayer, ConfigParseError, ResolveError, RobaConfig, RunOverrides};
pub use run::{
    AgentSpec, ContextSpec, Cost, Effort, ExecutionSpec, FailureKind, LimitSpec, PermissionPolicy,
    Prompt, PromptError, ProviderId, ProviderIdError, RunEvent, RunFailure, RunFailureDetails,
    RunId, RunOutcome, RunSpec, RunSpecError, RunState, SessionHandle, SessionSpec, TokenUsage,
    ToolPolicy, TurnRequest, WorkerPolicy, WorkerPolicyError, WorkerSpec,
};
pub use runtime::{Roba, RuntimeError};
