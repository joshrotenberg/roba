use std::ffi::OsStr;
use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use roba_git::{
    GIT_SNAPSHOT_TOOL, GIT_STAGE_ALL_TOOL, GIT_WORKSPACE_RESOURCE_URI, GitAuthority,
    GitStageAllRefusalKind, GitStageAllResult, GitWorkspace, GitWorkspaceError,
    GitWorkspaceSnapshot,
};
use serde_json::json;
use tempfile::TempDir;
use tower_mcp::{ChannelTransport, McpClient, McpRouter};

struct Fixture {
    temp: TempDir,
}

impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().expect("fixture tempdir");
        let fixture = Self { temp };
        fixture.git_ok(["init", "--quiet"]);
        fixture.git_ok(["config", "user.name", "Roba Test"]);
        fixture.git_ok(["config", "user.email", "roba@example.invalid"]);
        fixture.git_ok(["config", "commit.gpgsign", "false"]);
        fixture.git_ok(["config", "core.autocrlf", "false"]);
        fixture.git_ok(["config", "core.filemode", "false"]);
        fixture.git_ok(["config", "core.fsmonitor", "false"]);
        fixture.git_ok(["config", "core.hooksPath", ".git/disabled-hooks"]);
        fixture
    }

    fn with_initial_commit() -> Self {
        let fixture = Self::new();
        fixture.write("modified.txt", "original\n");
        fixture.write("deleted.txt", "delete me\n");
        fixture.write("conflict.txt", "base\n");
        fixture.git_ok(["add", "--all"]);
        fixture.git_ok(["commit", "--quiet", "--no-gpg-sign", "-m", "initial"]);
        fixture
    }

    fn root(&self) -> &Path {
        self.temp.path()
    }

    fn write(&self, relative: &str, contents: &str) {
        let path = self.root().join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("fixture parent directory");
        }
        fs::write(path, contents).expect("write fixture file");
    }

    fn git_ok<I, S>(&self, args: I) -> Output
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let output = self.git(args);
        assert!(
            output.status.success(),
            "fixture git failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }

    fn git<I, S>(&self, args: I) -> Output
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        Command::new("git")
            .args(args)
            .current_dir(self.root())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .expect("fixture git executable")
    }

    fn git_text<I, S>(&self, args: I) -> String
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        String::from_utf8(self.git_ok(args).stdout)
            .expect("git fixture output is UTF-8")
            .trim()
            .to_owned()
    }
}

async fn client(router: McpRouter) -> McpClient {
    let client = McpClient::connect(ChannelTransport::new(router))
        .await
        .expect("MCP client connects");
    client
        .initialize("roba-git-test", "0")
        .await
        .expect("MCP client initializes");
    client
}

async fn read_workspace(client: &McpClient) -> GitWorkspaceSnapshot {
    let resource = client
        .read_resource(GIT_WORKSPACE_RESOURCE_URI)
        .await
        .expect("Git workspace resource is readable");
    assert_eq!(
        resource.contents[0].mime_type.as_deref(),
        Some("application/json")
    );
    serde_json::from_str(resource.first_text().expect("resource is JSON text"))
        .expect("resource has the public snapshot schema")
}

#[test]
fn discovery_captures_the_nearest_canonical_repository_and_nested_cwd() {
    let fixture = Fixture::new();
    let nested = fixture.root().join("one/two");
    fs::create_dir_all(&nested).expect("nested cwd");

    let workspace = GitWorkspace::discover(&nested).expect("nested repository discovery");
    assert_eq!(
        workspace.repository_root(),
        fs::canonicalize(fixture.root()).unwrap()
    );
    assert_eq!(
        workspace.working_directory(),
        fs::canonicalize(nested).unwrap()
    );

    let unrelated = tempfile::tempdir().expect("unrelated tempdir");
    assert!(matches!(
        GitWorkspace::discover(unrelated.path()),
        Err(GitWorkspaceError::NotRepository(_))
    ));

    let file = unrelated.path().join("file");
    fs::write(&file, "not a directory").unwrap();
    assert!(matches!(
        GitWorkspace::discover(file),
        Err(GitWorkspaceError::NotDirectory(_))
    ));
}

#[test]
fn discovery_accepts_a_worktree_style_git_file() {
    let temp = tempfile::tempdir().expect("worktree fixture");
    fs::write(temp.path().join(".git"), "gitdir: /not/used/by-discovery\n").unwrap();
    let nested = temp.path().join("nested");
    fs::create_dir(&nested).unwrap();

    let workspace = GitWorkspace::discover(nested).expect(".git file is a repository marker");
    assert_eq!(
        workspace.repository_root(),
        fs::canonicalize(temp.path()).unwrap()
    );
}

#[tokio::test]
async fn snapshot_is_typed_sorted_and_scoped_to_the_fixture() {
    let fixture = Fixture::with_initial_commit();
    fixture.write("modified.txt", "staged version\n");
    fixture.git_ok(["add", "--", "modified.txt"]);
    fixture.write("modified.txt", "unstaged version\n");
    fs::remove_file(fixture.root().join("deleted.txt")).unwrap();
    fixture.write("z-untracked.txt", "z\n");
    fixture.write("a-untracked.txt", "a\n");

    let nested = fixture.root().join("nested");
    fs::create_dir(&nested).unwrap();
    let workspace = GitWorkspace::discover(&nested).unwrap();
    let snapshot = workspace.snapshot().await.unwrap();

    let expected_head = fixture.git_text(["rev-parse", "HEAD"]);
    assert_eq!(snapshot.head.as_deref(), Some(expected_head.as_str()));
    assert_eq!(snapshot.staged, ["modified.txt"]);
    assert_eq!(snapshot.unstaged, ["deleted.txt", "modified.txt"]);
    assert_eq!(snapshot.untracked, ["a-untracked.txt", "z-untracked.txt"]);
    assert!(snapshot.conflicts.is_empty());
    assert_eq!(snapshot.repository_root, display(fixture.root()));
    assert_eq!(snapshot.working_directory, display(&nested));

    let other = Fixture::with_initial_commit();
    other.write("only-other.txt", "other\n");
    let again = workspace.snapshot().await.unwrap();
    assert!(!again.untracked.iter().any(|path| path == "only-other.txt"));
}

#[tokio::test]
async fn snapshot_tool_and_resource_publish_exactly_the_same_json_value() {
    let fixture = Fixture::with_initial_commit();
    fixture.write("new.txt", "new\n");
    let workspace = GitWorkspace::discover(fixture.root()).unwrap();
    let client = client(workspace.provider_router()).await;

    let tool = client
        .call_tool(GIT_SNAPSHOT_TOOL, json!({}))
        .await
        .expect("snapshot tool succeeds");
    assert!(!tool.is_error);
    let structured = tool
        .structured_content
        .expect("snapshot tool has structured content");
    let resource = serde_json::to_value(read_workspace(&client).await).unwrap();
    assert_eq!(structured, resource);
}

#[tokio::test]
async fn authority_changes_control_discovery_but_provider_stays_read_only() {
    let fixture = Fixture::with_initial_commit();
    let workspace = GitWorkspace::discover(fixture.root()).unwrap();

    let read_only = client(workspace.control_router(GitAuthority::ReadOnly)).await;
    let read_tools = read_only.list_tools().await.unwrap();
    assert!(
        read_tools
            .tools
            .iter()
            .any(|tool| tool.name == GIT_SNAPSHOT_TOOL)
    );
    assert!(
        !read_tools
            .tools
            .iter()
            .any(|tool| tool.name == GIT_STAGE_ALL_TOOL)
    );
    assert!(
        read_only
            .call_tool(GIT_STAGE_ALL_TOOL, json!({}))
            .await
            .is_err()
    );

    let writable = client(workspace.control_router(GitAuthority::WorkspaceWrite)).await;
    let write_tools = writable.list_tools().await.unwrap();
    assert!(
        write_tools
            .tools
            .iter()
            .any(|tool| tool.name == GIT_STAGE_ALL_TOOL)
    );

    let provider = client(workspace.provider_router()).await;
    let provider_tools = provider.list_tools().await.unwrap();
    assert!(
        provider_tools
            .tools
            .iter()
            .any(|tool| tool.name == GIT_SNAPSHOT_TOOL)
    );
    assert!(
        !provider_tools
            .tools
            .iter()
            .any(|tool| tool.name == GIT_STAGE_ALL_TOOL)
    );
    assert!(
        provider
            .call_tool(GIT_STAGE_ALL_TOOL, json!({}))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn stage_all_records_before_after_and_exact_tree_without_moving_head() {
    let fixture = Fixture::with_initial_commit();
    let head = fixture.git_text(["rev-parse", "HEAD"]);
    fixture.write("modified.txt", "modified\n");
    fs::remove_file(fixture.root().join("deleted.txt")).unwrap();
    fixture.write("untracked.txt", "untracked\n");
    let workspace = GitWorkspace::discover(fixture.root()).unwrap();
    let client = client(workspace.control_router(GitAuthority::WorkspaceWrite)).await;

    let raw = client
        .call_tool(GIT_STAGE_ALL_TOOL, json!({}))
        .await
        .expect("stage tool returns a typed result");
    assert!(!raw.is_error);
    let result: GitStageAllResult = serde_json::from_value(
        raw.structured_content
            .expect("stage tool has structured content"),
    )
    .unwrap();
    let GitStageAllResult::Staged {
        before,
        after,
        index_tree,
    } = result
    else {
        panic!("expected staged result")
    };

    assert_eq!(before.unstaged, ["deleted.txt", "modified.txt"]);
    assert_eq!(before.untracked, ["untracked.txt"]);
    assert_eq!(
        after.staged,
        ["deleted.txt", "modified.txt", "untracked.txt"]
    );
    assert!(after.unstaged.is_empty());
    assert!(after.untracked.is_empty());
    assert_eq!(index_tree, fixture.git_text(["write-tree"]));
    assert_eq!(head, fixture.git_text(["rev-parse", "HEAD"]));
    assert_eq!(before.head.as_deref(), Some(head.as_str()));
    assert_eq!(after.head.as_deref(), Some(head.as_str()));
}

#[tokio::test]
async fn stage_all_refuses_noop_and_unresolved_conflicts() {
    let clean = Fixture::with_initial_commit();
    let clean_workspace = GitWorkspace::discover(clean.root()).unwrap();
    let result = clean_workspace.stage_all().await;
    assert!(result.is_error());
    assert!(matches!(
        result,
        GitStageAllResult::Refused {
            refusal: roba_git::GitStageAllRefusal {
                kind: GitStageAllRefusalKind::NothingToStage,
                ..
            },
            ..
        }
    ));

    let conflict = Fixture::with_initial_commit();
    let base = conflict.git_text(["branch", "--show-current"]);
    conflict.git_ok(["checkout", "--quiet", "-b", "other"]);
    conflict.write("conflict.txt", "other\n");
    conflict.git_ok(["commit", "--quiet", "--no-gpg-sign", "-am", "other"]);
    conflict.git_ok(["checkout", "--quiet", base.as_str()]);
    conflict.write("conflict.txt", "ours\n");
    conflict.git_ok(["commit", "--quiet", "--no-gpg-sign", "-am", "base"]);
    let merge = conflict.git(["merge", "--no-edit", "other"]);
    assert!(!merge.status.success(), "fixture merge must conflict");

    let workspace = GitWorkspace::discover(conflict.root()).unwrap();
    let before = workspace.snapshot().await.unwrap();
    assert_eq!(before.conflicts, ["conflict.txt"]);
    let result = workspace.stage_all().await;
    assert!(matches!(
        result,
        GitStageAllResult::Refused {
            refusal: roba_git::GitStageAllRefusal {
                kind: GitStageAllRefusalKind::Conflicts,
                ..
            },
            ..
        }
    ));
}

#[tokio::test]
async fn inspection_failure_is_reported_after_the_captured_repo_disappears() {
    let fixture = Fixture::with_initial_commit();
    let workspace = GitWorkspace::discover(fixture.root()).unwrap();
    let dot_git = fixture.root().join(".git");
    let moved = fixture.root().join(".git-moved-for-test");
    fs::rename(&dot_git, &moved).unwrap();

    assert!(workspace.snapshot().await.is_err());
    let result = workspace.stage_all().await;
    assert!(matches!(result, GitStageAllResult::Failed { .. }));

    fs::rename(moved, dot_git).unwrap();
}

fn display(path: impl AsRef<Path>) -> String {
    fs::canonicalize(path)
        .unwrap()
        .to_string_lossy()
        .into_owned()
}

#[allow(dead_code)]
fn assert_send_sync<T: Send + Sync>() {}

#[test]
fn workspace_is_send_sync() {
    assert_send_sync::<GitWorkspace>();
}
