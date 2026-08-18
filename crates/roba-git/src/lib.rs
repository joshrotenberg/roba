//! Typed, repository-scoped Git fragments for the Roba MCP harness.
//!
//! [`GitWorkspace`] captures one canonical repository at construction. Every
//! control and provider projection created from it observes that same state;
//! MCP callers cannot redirect an operation to another path.

mod progress;

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use git_spawn::command::status::StatusFormat;
use git_spawn::parse::{StatusKind, parse_full_status};
use git_spawn::{GitCommand, Repository};
use roba_mcp::{
    AgentExtension, ContextAudience, ContextDelivery, ContextEntrySpec, ContextFreshness,
    ContextKind, ContextOrigin, ContextOriginKind, ContextPhase, ContextPrecedence, ContextScope,
    ContextSensitivity,
};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tower_mcp::schemars::{self, JsonSchema, schema_for};
use tower_mcp::{
    CallToolResult, Content, Error as McpError, McpRouter, ReadResourceResult, ResourceBuilder,
    ToolBuilder,
};

pub use progress::{
    GIT_PROGRESS_RESOURCE_URI, GitCommitSummary, GitDiffStatistics, GitPathSummary,
    GitProgressConfig, GitProgressHealth, GitProgressPoint, GitProgressSnapshot, GitProgressState,
    GitRenameSummary,
};

/// Read the current repository snapshot.
pub const GIT_SNAPSHOT_TOOL: &str = "git.snapshot";
/// Stage every tracked, deleted, and untracked change in the fixed workspace.
pub const GIT_STAGE_ALL_TOOL: &str = "git.stage_all";
/// Live JSON projection of the fixed repository.
pub const GIT_WORKSPACE_RESOURCE_URI: &str = "roba://git/workspace";
/// Lazy context entry describing how to discover the Git extension.
pub const GIT_CONTEXT_ENTRY_ID: &str = "roba.git.activation";

const EXTENSION_NAME: &str = "roba-git";
const GIT_CONTEXT_ENTRY_URI: &str = "roba://context/entry?id=roba.git.activation";
const GIT_CONTEXT: &str = "The Roba Git extension observes one fixed repository. Read current state with `git.snapshot` or `roba://git/workspace`, and read operation-scoped change evidence from `roba://git/progress`. `git.stage_all` is available only to an explicitly writable operator projection; it is never provider authority.";
const COMMAND_TIMEOUT: Duration = Duration::from_secs(3);
pub(crate) const SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(12);
const STAGE_MUTATION_TIMEOUT: Duration = Duration::from_secs(20);

/// Authority granted to the operator/control projection.
///
/// The provider projection remains read-only for both variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitAuthority {
    /// Advertise only repository observation.
    ReadOnly,
    /// Also advertise the bounded `git.stage_all` control workflow.
    WorkspaceWrite,
}

/// One fixed repository shared by every router projection of this service.
#[derive(Clone)]
pub struct GitWorkspace {
    inner: Arc<Inner>,
}

struct Inner {
    repository: Repository,
    repository_root: PathBuf,
    working_directory: PathBuf,
    operation: Mutex<()>,
}

impl fmt::Debug for GitWorkspace {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GitWorkspace")
            .field("repository_root", &self.inner.repository_root)
            .field("working_directory", &self.inner.working_directory)
            .finish_non_exhaustive()
    }
}

impl GitWorkspace {
    /// Discover the nearest repository containing `start` and capture it.
    ///
    /// Both `start` and the selected repository root are canonicalized once.
    /// A `.git` directory or file qualifies, so linked worktrees work without
    /// special casing. No later operation consults ambient process cwd.
    pub fn discover(start: impl AsRef<Path>) -> Result<Self, GitWorkspaceError> {
        let requested = start.as_ref();
        let working_directory =
            std::fs::canonicalize(requested).map_err(|source| GitWorkspaceError::Canonicalize {
                path: requested.to_path_buf(),
                source,
            })?;
        if !working_directory.is_dir() {
            return Err(GitWorkspaceError::NotDirectory(working_directory));
        }

        let repository_root = working_directory
            .ancestors()
            .find(|candidate| candidate.join(".git").exists())
            .map(Path::to_path_buf)
            .ok_or_else(|| GitWorkspaceError::NotRepository(working_directory.clone()))?;
        let repository_root = std::fs::canonicalize(&repository_root).map_err(|source| {
            GitWorkspaceError::Canonicalize {
                path: repository_root,
                source,
            }
        })?;
        let repository = Repository::open(repository_root.clone())?;

        Ok(Self {
            inner: Arc::new(Inner {
                repository,
                repository_root,
                working_directory,
                operation: Mutex::new(()),
            }),
        })
    }

    /// Canonical root of the captured repository.
    pub fn repository_root(&self) -> &Path {
        &self.inner.repository_root
    }

    /// Canonical directory from which repository discovery began.
    pub fn working_directory(&self) -> &Path {
        &self.inner.working_directory
    }

    /// Build the role-scoped Roba extension around this shared workspace.
    pub fn extension(&self, authority: GitAuthority) -> AgentExtension {
        self.extension_with_progress(authority, GitProgressConfig::default())
    }

    /// Build the role-scoped extension with cached operation progress.
    pub fn extension_with_progress(
        &self,
        authority: GitAuthority,
        config: GitProgressConfig,
    ) -> AgentExtension {
        let lifecycle = progress::GitProgressLifecycle::new(self.clone(), config);
        let control = self
            .control_router(authority)
            .try_merge(lifecycle.router())
            .expect("static Git control progress resource must not collide");
        let provider = self
            .provider_router()
            .try_merge(lifecycle.router())
            .expect("static Git provider progress resource must not collide");
        AgentExtension::new(EXTENSION_NAME, control, provider)
            .try_provider_tool(GIT_SNAPSHOT_TOOL)
            .expect("static Git provider tool name must be valid")
            .with_inline_context(
                ContextEntrySpec::new(
                    GIT_CONTEXT_ENTRY_ID,
                    ContextKind::Reference,
                    ContextOrigin::new(ContextOriginKind::Extension, EXTENSION_NAME),
                    ContextPhase::Bootstrap,
                    ContextScope::Agent,
                    ContextDelivery::McpResource {
                        uri: GIT_CONTEXT_ENTRY_URI.to_owned(),
                    },
                )
                .audience(ContextAudience::Both)
                .precedence(ContextPrecedence::Host)
                .freshness(ContextFreshness::Generation)
                .sensitivity(ContextSensitivity::Public),
                GIT_CONTEXT,
            )
            .with_lifecycle(lifecycle)
    }

    /// Build a fresh operator/control router fragment.
    pub fn control_router(&self, authority: GitAuthority) -> McpRouter {
        self.router(authority)
    }

    /// Build a fresh least-authority provider router fragment.
    pub fn provider_router(&self) -> McpRouter {
        self.router(GitAuthority::ReadOnly)
    }

    /// Take a coherent snapshot relative to other calls through this service.
    pub async fn snapshot(&self) -> Result<GitWorkspaceSnapshot, GitWorkspaceError> {
        let _operation = self.inner.operation.lock().await;
        self.snapshot_bounded().await
    }

    /// Stage all current changes and report the exact resulting index tree.
    pub async fn stage_all(&self) -> GitStageAllResult {
        let _operation = self.inner.operation.lock().await;
        let before = match self.snapshot_bounded().await {
            Ok(snapshot) => snapshot,
            Err(error) => {
                return GitStageAllResult::Failed {
                    failure: GitOperationFailure::from_error(&error),
                    before: None,
                    after: None,
                };
            }
        };

        if !before.conflicts.is_empty() {
            return GitStageAllResult::Refused {
                refusal: GitStageAllRefusal {
                    kind: GitStageAllRefusalKind::Conflicts,
                    message: "repository has unresolved conflicts".to_owned(),
                },
                snapshot: before,
            };
        }
        if before.unstaged.is_empty() && before.untracked.is_empty() {
            return GitStageAllResult::Refused {
                refusal: GitStageAllRefusal {
                    kind: GitStageAllRefusalKind::NothingToStage,
                    message: "repository has no unstaged or untracked changes".to_owned(),
                },
                snapshot: before,
            };
        }

        let mutation = tokio::time::timeout(STAGE_MUTATION_TIMEOUT, async {
            let mut add = self.inner.repository.add();
            add.all();
            configure_command(&mut add);
            add.execute().await?;

            let mut write_tree = self.inner.repository.write_tree();
            configure_command(&mut write_tree);
            write_tree.execute().await
        })
        .await;

        let index_tree = match mutation {
            Ok(Ok(index_tree)) => index_tree,
            Ok(Err(error)) => {
                return self
                    .failed_after(before, GitWorkspaceError::Git(error))
                    .await;
            }
            Err(_) => {
                return self
                    .failed_after(
                        before,
                        GitWorkspaceError::Timeout {
                            operation: "git.stage_all",
                            duration: STAGE_MUTATION_TIMEOUT,
                        },
                    )
                    .await;
            }
        };

        match self.snapshot_bounded().await {
            Ok(after) => GitStageAllResult::Staged {
                before,
                after,
                index_tree,
            },
            Err(error) => GitStageAllResult::Failed {
                failure: GitOperationFailure::from_error(&error),
                before: Some(before),
                after: None,
            },
        }
    }

    fn router(&self, authority: GitAuthority) -> McpRouter {
        let tool_workspace = self.clone();
        let output_schema = serde_json::to_value(schema_for!(GitWorkspaceSnapshot))
            .expect("static git snapshot schema must serialize");
        let snapshot = ToolBuilder::new(GIT_SNAPSHOT_TOOL)
            .description("Read the current state of this fixed Git workspace.")
            .output_schema(output_schema)
            .read_only_safe()
            .handler(move |_input: GitSnapshotInput| {
                let workspace = tool_workspace.clone();
                async move {
                    let snapshot = workspace.snapshot().await.map_err(|error| {
                        McpError::tool_with_name(GIT_SNAPSHOT_TOOL, error.to_string())
                    })?;
                    encode_snapshot(&snapshot)
                }
            })
            .build();

        let resource_workspace = self.clone();
        let resource = ResourceBuilder::new(GIT_WORKSPACE_RESOURCE_URI)
            .name("Roba Git workspace")
            .description("Current state of this fixed Git workspace.")
            .mime_type("application/json")
            .handler(move || {
                let workspace = resource_workspace.clone();
                async move {
                    let snapshot = workspace.snapshot().await.map_err(|error| {
                        McpError::internal(format!(
                            "failed to read {GIT_WORKSPACE_RESOURCE_URI}: {error}"
                        ))
                    })?;
                    let json = serde_json::to_string(&snapshot).map_err(|error| {
                        McpError::internal(format!(
                            "failed to serialize Git workspace snapshot: {error}"
                        ))
                    })?;
                    Ok(ReadResourceResult::text_with_mime(
                        GIT_WORKSPACE_RESOURCE_URI,
                        json,
                        "application/json",
                    ))
                }
            })
            .build();

        let router = McpRouter::new().tool(snapshot).resource(resource);
        if authority == GitAuthority::ReadOnly {
            return router;
        }

        let stage_workspace = self.clone();
        let output_schema = serde_json::to_value(schema_for!(GitStageAllResult))
            .expect("static git stage result schema must serialize");
        let stage = ToolBuilder::new(GIT_STAGE_ALL_TOOL)
            .description(
                "Stage every current change in this fixed workspace and return an index receipt.",
            )
            .output_schema(output_schema)
            .non_destructive()
            .handler(move |_input: GitStageAllInput| {
                let workspace = stage_workspace.clone();
                async move {
                    let result = workspace.stage_all().await;
                    encode_stage_result(&result)
                }
            })
            .build();
        router.tool(stage)
    }

    async fn snapshot_bounded(&self) -> Result<GitWorkspaceSnapshot, GitWorkspaceError> {
        tokio::time::timeout(SNAPSHOT_TIMEOUT, self.snapshot_inner())
            .await
            .map_err(|_| GitWorkspaceError::Timeout {
                operation: "git.snapshot",
                duration: SNAPSHOT_TIMEOUT,
            })?
    }

    async fn snapshot_inner(&self) -> Result<GitWorkspaceSnapshot, GitWorkspaceError> {
        let mut status = self.inner.repository.status();
        status
            .format(StatusFormat::PorcelainV1)
            .branch()
            .null_terminate()
            .untracked_files("all")
            .global_args(["--no-optional-locks", "-c", "core.fsmonitor=false"]);
        configure_command(&mut status);
        let status = status.execute().await?;
        let status = parse_full_status(&status.stdout_str())?;

        let mut staged = Vec::new();
        let mut unstaged = Vec::new();
        let mut untracked = Vec::new();
        let mut conflicts = Vec::new();
        for entry in status.entries {
            if is_conflict(entry.index, entry.worktree) {
                conflicts.push(entry.path);
            } else if matches!(entry.index, StatusKind::Untracked)
                || matches!(entry.worktree, StatusKind::Untracked)
            {
                untracked.push(entry.path);
            } else {
                if is_change(entry.index) {
                    staged.push(entry.path.clone());
                }
                if is_change(entry.worktree) {
                    unstaged.push(entry.path);
                }
            }
        }
        sort_paths(&mut staged);
        sort_paths(&mut unstaged);
        sort_paths(&mut untracked);
        sort_paths(&mut conflicts);

        Ok(GitWorkspaceSnapshot {
            repository_root: display_path(&self.inner.repository_root),
            working_directory: display_path(&self.inner.working_directory),
            head: self.read_head().await?,
            branch: status.branch,
            tracking: status.tracking,
            ahead: status.ahead,
            behind: status.behind,
            staged,
            unstaged,
            untracked,
            conflicts,
        })
    }

    async fn read_head(&self) -> Result<Option<String>, GitWorkspaceError> {
        let mut command = self.inner.repository.rev_parse();
        command
            .verify()
            .arg_str("--quiet")
            .arg_str("HEAD")
            .global_args(["--no-optional-locks", "-c", "core.fsmonitor=false"]);
        configure_command(&mut command);
        let output = command.execute_raw_unchecked().await?;
        match output.exit_code {
            0 => Ok(Some(output.stdout_trimmed())),
            1 => Ok(None),
            exit_code => {
                let stdout = output.stdout_str().into_owned();
                let stderr = output.stderr;
                Err(GitWorkspaceError::Git(git_spawn::Error::command_failed(
                    "git rev-parse --verify --quiet HEAD",
                    exit_code,
                    stdout,
                    stderr,
                )))
            }
        }
    }

    async fn failed_after(
        &self,
        before: GitWorkspaceSnapshot,
        error: GitWorkspaceError,
    ) -> GitStageAllResult {
        GitStageAllResult::Failed {
            failure: GitOperationFailure::from_error(&error),
            before: Some(before),
            after: self.snapshot_bounded().await.ok(),
        }
    }
}

fn configure_command(command: &mut impl GitCommand) {
    // Every command in this service is noninteractive. Close stdin explicitly
    // so Git never inherits a hot stdio host's MCP control pipe.
    command
        .stdin_bytes(Vec::new())
        .with_timeout(COMMAND_TIMEOUT);
}

fn encode_snapshot(snapshot: &GitWorkspaceSnapshot) -> Result<CallToolResult, McpError> {
    let mut result = CallToolResult::from_serialize(snapshot)?;
    let json = serde_json::to_string(snapshot).map_err(|error| {
        McpError::internal(format!(
            "failed to serialize Git workspace snapshot: {error}"
        ))
    })?;
    result.content = vec![Content::text(json)];
    Ok(result)
}

fn encode_stage_result(result: &GitStageAllResult) -> Result<CallToolResult, McpError> {
    let mut encoded = CallToolResult::from_serialize(result)?;
    encoded.content = vec![Content::text(result.display_text())];
    encoded.is_error = result.is_error();
    Ok(encoded)
}

fn is_change(kind: StatusKind) -> bool {
    !matches!(
        kind,
        StatusKind::Unmodified | StatusKind::Untracked | StatusKind::Ignored
    )
}

fn is_conflict(index: StatusKind, worktree: StatusKind) -> bool {
    matches!(
        (index, worktree),
        (StatusKind::Deleted, StatusKind::Deleted)
            | (StatusKind::Added, StatusKind::Unmerged)
            | (StatusKind::Unmerged, StatusKind::Deleted)
            | (StatusKind::Unmerged, StatusKind::Added)
            | (StatusKind::Deleted, StatusKind::Unmerged)
            | (StatusKind::Added, StatusKind::Added)
            | (StatusKind::Unmerged, StatusKind::Unmerged)
    )
}

fn sort_paths(paths: &mut Vec<String>) {
    paths.sort();
    paths.dedup();
}

fn display_path(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// Empty input for [`GIT_SNAPSHOT_TOOL`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GitSnapshotInput {}

/// Empty input for [`GIT_STAGE_ALL_TOOL`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GitStageAllInput {}

/// Deterministic typed state of one fixed Git workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GitWorkspaceSnapshot {
    /// Canonical repository root captured at service construction.
    pub repository_root: String,
    /// Canonical discovery start directory captured at construction.
    pub working_directory: String,
    /// Full current commit object id, or `None` for an unborn branch.
    pub head: Option<String>,
    /// Current branch name, or `None` for detached HEAD.
    pub branch: Option<String>,
    /// Configured upstream ref, if any.
    pub tracking: Option<String>,
    /// Commits ahead of `tracking`.
    pub ahead: u32,
    /// Commits behind `tracking`.
    pub behind: u32,
    /// Repository-relative paths changed in the index.
    pub staged: Vec<String>,
    /// Repository-relative tracked paths changed outside the index.
    pub unstaged: Vec<String>,
    /// Repository-relative untracked paths.
    pub untracked: Vec<String>,
    /// Repository-relative paths with unresolved merges.
    pub conflicts: Vec<String>,
}

/// Why `git.stage_all` declined to change the index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GitStageAllRefusalKind {
    /// The index contains unresolved merge stages.
    Conflicts,
    /// No unstaged or untracked path exists.
    NothingToStage,
}

/// Typed, stable refusal from `git.stage_all`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GitStageAllRefusal {
    pub kind: GitStageAllRefusalKind,
    pub message: String,
}

/// Coarse stable category for a Git operation failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GitOperationFailureKind {
    Prerequisite,
    Repository,
    Command,
    Parsing,
    Io,
    Timeout,
    Runtime,
}

/// Serializable failure evidence returned by a mutating tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GitOperationFailure {
    pub kind: GitOperationFailureKind,
    pub message: String,
}

impl GitOperationFailure {
    fn from_error(error: &GitWorkspaceError) -> Self {
        let kind = match error {
            GitWorkspaceError::NotRepository(_) => GitOperationFailureKind::Repository,
            GitWorkspaceError::NotDirectory(_) | GitWorkspaceError::Canonicalize { .. } => {
                GitOperationFailureKind::Io
            }
            GitWorkspaceError::Timeout { .. } => GitOperationFailureKind::Timeout,
            GitWorkspaceError::Git(error) => match error.category() {
                "prerequisites" => GitOperationFailureKind::Prerequisite,
                "repository" => GitOperationFailureKind::Repository,
                "command" => GitOperationFailureKind::Command,
                "parsing" => GitOperationFailureKind::Parsing,
                "io" => GitOperationFailureKind::Io,
                _ => GitOperationFailureKind::Runtime,
            },
        };
        Self {
            kind,
            message: error.to_string(),
        }
    }
}

/// Structured outcome of the bounded staging workflow.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum GitStageAllResult {
    /// All current changes were staged and the resulting index tree recorded.
    Staged {
        before: GitWorkspaceSnapshot,
        after: GitWorkspaceSnapshot,
        index_tree: String,
    },
    /// No mutation was attempted because a typed precondition failed.
    Refused {
        refusal: GitStageAllRefusal,
        snapshot: GitWorkspaceSnapshot,
    },
    /// Git failed or timed out; `after` is best-effort mutation evidence.
    Failed {
        failure: GitOperationFailure,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        before: Option<GitWorkspaceSnapshot>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        after: Option<GitWorkspaceSnapshot>,
    },
}

impl GitStageAllResult {
    /// True when MCP should mark the tool execution as an error.
    pub fn is_error(&self) -> bool {
        !matches!(self, Self::Staged { .. })
    }

    /// Compact human-facing content paired with the structured result.
    pub fn display_text(&self) -> String {
        match self {
            Self::Staged { index_tree, .. } => {
                format!("staged workspace as index tree {index_tree}")
            }
            Self::Refused { refusal, .. } => refusal.message.clone(),
            Self::Failed { failure, .. } => failure.message.clone(),
        }
    }
}

/// Failure to discover or inspect a fixed Git workspace.
#[derive(Debug, thiserror::Error)]
pub enum GitWorkspaceError {
    #[error("failed to canonicalize {path}: {source}")]
    Canonicalize {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("Git workspace start is not a directory: {0}")]
    NotDirectory(PathBuf),
    #[error("no Git repository contains {0}")]
    NotRepository(PathBuf),
    #[error(transparent)]
    Git(#[from] git_spawn::Error),
    #[error("{operation} timed out after {duration:?}")]
    Timeout {
        operation: &'static str,
        duration: Duration,
    },
}

#[cfg(test)]
mod tests {
    use git_spawn::command::status::StatusCommand;

    use super::*;

    #[test]
    fn command_policy_closes_stdin_and_bounds_execution() {
        let mut command = StatusCommand::new();
        configure_command(&mut command);

        let executor = command.get_executor();
        assert_eq!(executor.stdin.as_deref(), Some(&[][..]));
        assert_eq!(executor.timeout, Some(COMMAND_TIMEOUT));
    }
}
