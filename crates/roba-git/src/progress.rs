//! Operation-scoped cached Git progress observation.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use git_spawn::GitCommand;
use git_spawn::parse::{StatusEntry, StatusKind, parse_diff_numstat, parse_full_status};
use roba_mcp::{
    AgentExtensionChange, AgentExtensionFuture, AgentExtensionHookError, AgentExtensionHookResult,
    AgentExtensionLifecycle, AgentExtensionOperation, AgentTerminalState, OperationId,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::RwLock;
use tower_mcp::schemars::{self, JsonSchema};
use tower_mcp::{Error as McpError, McpRouter, ReadResourceResult, ResourceBuilder};

use super::{
    GitOperationFailure, GitWorkspace, GitWorkspaceError, GitWorkspaceSnapshot, SNAPSHOT_TIMEOUT,
    configure_command,
};

const MAX_COMMIT_SUMMARIES: u32 = 64;
const MAX_COMMIT_SUMMARY_CHARS: usize = 160;

/// Cached operation progress resource URI.
pub const GIT_PROGRESS_RESOURCE_URI: &str = "roba://git/progress";

/// Sampling policy for operation-scoped Git progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GitProgressConfig {
    /// Period between active-operation samples. `None` disables periodic
    /// sampling while retaining admission and final synchronous refreshes.
    pub poll_interval: Option<Duration>,
}

impl GitProgressConfig {
    /// Build a policy from the provider-neutral config representation.
    pub fn from_interval_secs(seconds: u64) -> Self {
        Self {
            poll_interval: (seconds > 0).then(|| Duration::from_secs(seconds)),
        }
    }
}

impl Default for GitProgressConfig {
    fn default() -> Self {
        Self::from_interval_secs(5)
    }
}

/// Lifecycle state of the cached Git progress view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GitProgressState {
    /// No operation has populated the cache yet.
    Idle,
    /// One exact operation is active.
    Observing,
    /// The latest operation has fully settled.
    Settled,
}

/// Health of the most recent bounded Git observation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GitProgressHealth {
    Healthy,
    Degraded,
}

/// Aggregate line counts from a bounded Git diff.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GitDiffStatistics {
    pub files: u32,
    pub insertions: u32,
    pub deletions: u32,
    pub binary_files: u32,
}

/// One rename observed in either index or worktree state.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
pub struct GitRenameSummary {
    pub from: String,
    pub to: String,
}

/// Bounded path-level classification without diff bodies.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GitPathSummary {
    pub added: Vec<String>,
    pub modified: Vec<String>,
    pub deleted: Vec<String>,
    pub renamed: Vec<GitRenameSummary>,
    pub untracked: Vec<String>,
    pub conflicts: Vec<String>,
}

/// One coherent repository observation used for baseline/current comparison.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GitProgressPoint {
    pub workspace: GitWorkspaceSnapshot,
    pub staged: GitDiffStatistics,
    pub unstaged: GitDiffStatistics,
    pub paths: GitPathSummary,
}

/// Commit created after the operation baseline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GitCommitSummary {
    pub object_id: String,
    pub summary: String,
}

/// Cheap cached progress for the latest operation in this fixed repository.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GitProgressSnapshot {
    pub state: GitProgressState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<OperationId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal: Option<AgentTerminalState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub baseline: Option<GitProgressPoint>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current: Option<GitProgressPoint>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub commits_since_baseline: Vec<GitCommitSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_observed_at_unix_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_changed_at_unix_ms: Option<u64>,
    /// Number of bounded observations attempted for this exact operation.
    pub observations: u64,
    pub health: GitProgressHealth,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<GitOperationFailure>,
}

impl Default for GitProgressSnapshot {
    fn default() -> Self {
        Self {
            state: GitProgressState::Idle,
            operation_id: None,
            terminal: None,
            baseline: None,
            current: None,
            commits_since_baseline: Vec::new(),
            fingerprint: None,
            last_observed_at_unix_ms: None,
            last_changed_at_unix_ms: None,
            observations: 0,
            health: GitProgressHealth::Healthy,
            last_error: None,
        }
    }
}

pub(crate) struct GitProgressLifecycle {
    workspace: GitWorkspace,
    config: GitProgressConfig,
    cache: Arc<RwLock<GitProgressSnapshot>>,
}

impl GitProgressLifecycle {
    pub(crate) fn new(workspace: GitWorkspace, config: GitProgressConfig) -> Arc<Self> {
        Arc::new(Self {
            workspace,
            config,
            cache: Arc::new(RwLock::new(GitProgressSnapshot::default())),
        })
    }

    pub(crate) async fn snapshot(&self) -> GitProgressSnapshot {
        self.cache.read().await.clone()
    }

    pub(crate) fn router(self: &Arc<Self>) -> McpRouter {
        let lifecycle = Arc::clone(self);
        let resource = ResourceBuilder::new(GIT_PROGRESS_RESOURCE_URI)
            .name("Roba Git operation progress")
            .description("Cached Git progress for the active or latest Roba operation.")
            .mime_type("application/json")
            .handler(move || {
                let lifecycle = Arc::clone(&lifecycle);
                async move {
                    let snapshot = lifecycle.snapshot().await;
                    let json = serde_json::to_string(&snapshot).map_err(|error| {
                        McpError::internal(format!(
                            "failed to serialize Git progress snapshot: {error}"
                        ))
                    })?;
                    Ok(ReadResourceResult::text_with_mime(
                        GIT_PROGRESS_RESOURCE_URI,
                        json,
                        "application/json",
                    ))
                }
            })
            .build();
        McpRouter::new().resource(resource)
    }

    async fn observe(
        &self,
        operation: AgentExtensionOperation,
        baseline: bool,
    ) -> AgentExtensionHookResult {
        let observed_at = unix_time_ms();
        let (baseline_head, previous_head, previous_fingerprint) = {
            let cache = self.cache.read().await;
            if !baseline && cache.operation_id != Some(operation.operation_id) {
                return Ok(None);
            }
            if baseline {
                (None, None, None)
            } else {
                (
                    cache
                        .baseline
                        .as_ref()
                        .and_then(|point| point.workspace.head.clone()),
                    cache
                        .current
                        .as_ref()
                        .and_then(|point| point.workspace.head.clone()),
                    cache.fingerprint.clone(),
                )
            }
        };
        let (point, commits) = match self
            .workspace
            .progress_observation(
                baseline_head.as_deref(),
                previous_head.as_deref(),
                !baseline,
            )
            .await
        {
            Ok(observation) => observation,
            Err(error) => {
                let mut cache = self.cache.write().await;
                if cache.operation_id == Some(operation.operation_id) || baseline {
                    cache.state = GitProgressState::Observing;
                    cache.operation_id = Some(operation.operation_id);
                    cache.terminal = None;
                    if baseline {
                        cache.baseline = None;
                        cache.current = None;
                        cache.commits_since_baseline.clear();
                        cache.fingerprint = None;
                        cache.last_changed_at_unix_ms = None;
                    }
                    cache.last_observed_at_unix_ms = observed_at;
                    cache.observations = if baseline {
                        1
                    } else {
                        cache.observations.saturating_add(1)
                    };
                    cache.health = GitProgressHealth::Degraded;
                    cache.last_error = Some(GitOperationFailure::from_error(&error));
                }
                return Err(AgentExtensionHookError::new(error.to_string()));
            }
        };
        let fingerprint = fingerprint(&point);
        let head_changed = previous_head != point.workspace.head;

        let changed = baseline || previous_fingerprint.as_deref() != Some(&fingerprint);
        let summary = changed.then(|| progress_summary(&point, head_changed, commits.len()));
        let mut cache = self.cache.write().await;
        if !baseline && cache.operation_id != Some(operation.operation_id) {
            return Ok(None);
        }
        if baseline {
            cache.baseline = Some(point.clone());
            cache.commits_since_baseline.clear();
        } else if head_changed {
            cache.commits_since_baseline = commits;
        }
        cache.state = GitProgressState::Observing;
        cache.operation_id = Some(operation.operation_id);
        cache.terminal = None;
        cache.current = Some(point);
        cache.fingerprint = Some(fingerprint.clone());
        cache.last_observed_at_unix_ms = observed_at;
        cache.observations = if baseline {
            1
        } else {
            cache.observations.saturating_add(1)
        };
        if changed {
            cache.last_changed_at_unix_ms = observed_at;
        }
        cache.health = GitProgressHealth::Healthy;
        cache.last_error = None;
        Ok(summary.map(|summary| AgentExtensionChange::new(fingerprint, summary)))
    }
}

impl AgentExtensionLifecycle for GitProgressLifecycle {
    fn poll_interval(&self) -> Option<Duration> {
        self.config.poll_interval
    }

    fn operation_admitted(
        &self,
        operation: AgentExtensionOperation,
    ) -> AgentExtensionFuture<AgentExtensionHookResult> {
        let lifecycle = Arc::new(self.clone_inner());
        Box::pin(async move { lifecycle.observe(operation, true).await })
    }

    fn observation_tick(
        &self,
        operation: AgentExtensionOperation,
    ) -> AgentExtensionFuture<AgentExtensionHookResult> {
        let lifecycle = Arc::new(self.clone_inner());
        Box::pin(async move { lifecycle.observe(operation, false).await })
    }

    fn operation_settling(
        &self,
        operation: AgentExtensionOperation,
        _terminal: AgentTerminalState,
    ) -> AgentExtensionFuture<AgentExtensionHookResult> {
        let lifecycle = Arc::new(self.clone_inner());
        Box::pin(async move { lifecycle.observe(operation, false).await })
    }

    fn operation_settled(
        &self,
        operation: AgentExtensionOperation,
        terminal: AgentTerminalState,
    ) -> AgentExtensionFuture<AgentExtensionHookResult> {
        let cache = Arc::clone(&self.cache);
        Box::pin(async move {
            let mut cache = cache.write().await;
            if cache.operation_id == Some(operation.operation_id) {
                cache.state = GitProgressState::Settled;
                cache.terminal = Some(terminal);
            }
            Ok(None)
        })
    }
}

impl GitProgressLifecycle {
    fn clone_inner(&self) -> Self {
        Self {
            workspace: self.workspace.clone(),
            config: self.config,
            cache: Arc::clone(&self.cache),
        }
    }
}

impl GitWorkspace {
    async fn progress_observation(
        &self,
        baseline_head: Option<&str>,
        previous_head: Option<&str>,
        include_commits: bool,
    ) -> Result<(GitProgressPoint, Vec<GitCommitSummary>), GitWorkspaceError> {
        let _operation = self.inner.operation.lock().await;
        tokio::time::timeout(SNAPSHOT_TIMEOUT, async {
            let point = self.progress_point_inner().await?;
            let commits = if include_commits && previous_head != point.workspace.head.as_deref() {
                self.commits_since_inner(baseline_head, point.workspace.head.as_deref())
                    .await?
            } else {
                Vec::new()
            };
            Ok((point, commits))
        })
        .await
        .map_err(|_| GitWorkspaceError::Timeout {
            operation: "git.progress",
            duration: SNAPSHOT_TIMEOUT,
        })?
    }

    async fn progress_point_inner(&self) -> Result<GitProgressPoint, GitWorkspaceError> {
        let workspace = self.snapshot_inner().await?;
        let entries = self.status_entries().await?;
        let staged = self.diff_statistics(true).await?;
        let unstaged = self.diff_statistics(false).await?;
        Ok(GitProgressPoint {
            workspace,
            staged,
            unstaged,
            paths: summarize_paths(entries),
        })
    }

    async fn status_entries(&self) -> Result<Vec<StatusEntry>, GitWorkspaceError> {
        let mut status = self.inner.repository.status();
        status
            .format(git_spawn::command::status::StatusFormat::PorcelainV1)
            .branch()
            .null_terminate()
            .untracked_files("all")
            .global_args(["--no-optional-locks", "-c", "core.fsmonitor=false"]);
        configure_command(&mut status);
        Ok(parse_full_status(&status.execute().await?.stdout_str())?.entries)
    }

    async fn diff_statistics(&self, cached: bool) -> Result<GitDiffStatistics, GitWorkspaceError> {
        let mut command = self.inner.repository.diff();
        if cached {
            command.cached();
        }
        command.numstat().null_terminate().global_args([
            "--no-optional-locks",
            "-c",
            "core.fsmonitor=false",
        ]);
        configure_command(&mut command);
        let diff = parse_diff_numstat(&command.execute().await?.stdout_str())?;
        Ok(GitDiffStatistics {
            files: u32::try_from(diff.files.len()).unwrap_or(u32::MAX),
            insertions: diff.total_insertions,
            deletions: diff.total_deletions,
            binary_files: u32::try_from(diff.files.iter().filter(|file| file.binary).count())
                .unwrap_or(u32::MAX),
        })
    }

    async fn commits_since_inner(
        &self,
        baseline: Option<&str>,
        current: Option<&str>,
    ) -> Result<Vec<GitCommitSummary>, GitWorkspaceError> {
        let Some(current) = current else {
            return Ok(Vec::new());
        };
        let mut command = self.inner.repository.log();
        command
            .max_count(MAX_COMMIT_SUMMARIES)
            .reverse()
            .format("%H%x09%s")
            .revision(match baseline {
                Some(baseline) => format!("{baseline}..{current}"),
                None => current.to_owned(),
            })
            .global_args(["--no-optional-locks", "-c", "core.fsmonitor=false"]);
        configure_command(&mut command);
        let output = command.execute().await?;
        Ok(output
            .stdout_str()
            .lines()
            .filter_map(|line| line.split_once('\t'))
            .map(|(object_id, summary)| GitCommitSummary {
                object_id: object_id.to_owned(),
                summary: summary.chars().take(MAX_COMMIT_SUMMARY_CHARS).collect(),
            })
            .collect())
    }
}

fn summarize_paths(entries: Vec<StatusEntry>) -> GitPathSummary {
    let mut added = BTreeSet::new();
    let mut modified = BTreeSet::new();
    let mut deleted = BTreeSet::new();
    let mut renamed = BTreeSet::new();
    let mut untracked = BTreeSet::new();
    let mut conflicts = BTreeSet::new();

    for entry in entries {
        if super::is_conflict(entry.index, entry.worktree) {
            conflicts.insert(entry.path);
            continue;
        }
        if matches!(entry.index, StatusKind::Untracked)
            || matches!(entry.worktree, StatusKind::Untracked)
        {
            untracked.insert(entry.path);
            continue;
        }
        if matches!(entry.index, StatusKind::Renamed | StatusKind::Copied)
            || matches!(entry.worktree, StatusKind::Renamed | StatusKind::Copied)
        {
            renamed.insert(GitRenameSummary {
                from: entry.original_path.unwrap_or_default(),
                to: entry.path,
            });
            continue;
        }
        let kinds = [entry.index, entry.worktree];
        if kinds.iter().any(|kind| matches!(kind, StatusKind::Deleted)) {
            deleted.insert(entry.path);
        } else if kinds.iter().any(|kind| matches!(kind, StatusKind::Added)) {
            added.insert(entry.path);
        } else if kinds.iter().any(|kind| {
            matches!(
                kind,
                StatusKind::Modified | StatusKind::TypeChanged | StatusKind::Other(_)
            )
        }) {
            modified.insert(entry.path);
        }
    }

    GitPathSummary {
        added: added.into_iter().collect(),
        modified: modified.into_iter().collect(),
        deleted: deleted.into_iter().collect(),
        renamed: renamed.into_iter().collect(),
        untracked: untracked.into_iter().collect(),
        conflicts: conflicts.into_iter().collect(),
    }
}

fn fingerprint(point: &GitProgressPoint) -> String {
    let encoded = serde_json::to_vec(point).expect("Git progress point always serializes");
    let digest = Sha256::digest(encoded);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn progress_summary(point: &GitProgressPoint, head_changed: bool, commits: usize) -> String {
    let branch = point.workspace.branch.as_deref().unwrap_or("detached");
    format!(
        "Git {branch}: {} staged, {} unstaged, {} untracked, {} commits",
        point.workspace.staged.len(),
        point.workspace.unstaged.len(),
        point.workspace.untracked.len(),
        if head_changed { commits } else { 0 }
    )
}

fn unix_time_ms() -> Option<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| u64::try_from(duration.as_millis()).ok())
}
