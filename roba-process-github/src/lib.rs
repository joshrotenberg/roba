//! GitHub issue and pull-request process knowledge for one finite Roba mission.
//!
//! The pack is deliberately repository-scoped and sequential. It does not
//! create worktrees or claim safe parallel writes. Read, pull-request write,
//! and merge authority are separate mission grants on Roba's typed process
//! surface. Those grants are not an OS sandbox: a host must separately remove
//! ambient credentials or shell/network tools if it needs an adversarial
//! authority boundary.

use std::collections::BTreeSet;
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use roba_core::{
    AuthorityGrantId, ProcessActionId, ProcessActionRequest, ProcessActionScope, ProcessActionSpec,
    ProcessCapability, ProcessCapabilityDescriptor, ProcessCapabilityError, ProcessCapabilityId,
    ProcessFuture,
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

const CAPABILITY: &str = "github/issues";
const GRANT_READ: &str = "github/issues/read";
const GRANT_PR_WRITE: &str = "github/pulls/write";
const GRANT_MERGE: &str = "github/pulls/merge";

/// Exact GitHub repository identity used on every `gh` call.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct GitHubRepository(String);

impl GitHubRepository {
    pub fn new(value: impl Into<String>) -> Result<Self, GitHubRepositoryError> {
        let value = value.into();
        let Some((owner, repository)) = value.split_once('/') else {
            return Err(GitHubRepositoryError);
        };
        if repository.contains('/') || !valid_slug_part(owner) || !valid_slug_part(repository) {
            return Err(GitHubRepositoryError);
        }
        Ok(Self(format!(
            "{}/{}",
            owner.to_ascii_lowercase(),
            repository.to_ascii_lowercase()
        )))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for GitHubRepository {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(serde::de::Error::custom)
    }
}

impl fmt::Display for GitHubRepository {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

fn valid_slug_part(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 100
        && value
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GitHubRepositoryError;

impl fmt::Display for GitHubRepositoryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("GitHub repository must be an owner/name slug")
    }
}

impl std::error::Error for GitHubRepositoryError {}

/// Repository-scoped GitHub process pack.
#[derive(Clone)]
pub struct GitHubProcess {
    repository: GitHubRepository,
    gh_binary: PathBuf,
}

impl GitHubProcess {
    pub fn new(repository: GitHubRepository) -> Self {
        Self {
            repository,
            gh_binary: PathBuf::from("gh"),
        }
    }

    /// Select a `gh` binary. This is useful for hermetic hosts and tests.
    pub fn with_binary(mut self, binary: impl Into<PathBuf>) -> Self {
        self.gh_binary = binary.into();
        self
    }

    pub fn repository(&self) -> &GitHubRepository {
        &self.repository
    }

    fn gh_repository(&self) -> String {
        format!("github.com/{}", self.repository)
    }

    pub fn capability_id() -> ProcessCapabilityId {
        ProcessCapabilityId::new(CAPABILITY).expect("static capability id")
    }

    pub fn read_grant() -> AuthorityGrantId {
        AuthorityGrantId::new(GRANT_READ).expect("static read grant")
    }

    pub fn pull_request_write_grant() -> AuthorityGrantId {
        AuthorityGrantId::new(GRANT_PR_WRITE).expect("static PR-write grant")
    }

    pub fn merge_grant() -> AuthorityGrantId {
        AuthorityGrantId::new(GRANT_MERGE).expect("static merge grant")
    }

    async fn invoke_action(
        &self,
        action: &str,
        input: Value,
    ) -> Result<Value, ProcessCapabilityError> {
        match action {
            "issues/list" => self.list_issues(decode(input)?).await,
            "issues/get" => self.get_issue(decode(input)?).await,
            "pulls/get" => self.get_pull_request(decode(input)?).await,
            "pulls/create" => self.create_pull_request(decode(input)?).await,
            "pulls/merge" => self.merge_pull_request(decode(input)?).await,
            _ => Err(ProcessCapabilityError(format!(
                "unknown GitHub process action {action}"
            ))),
        }
    }

    async fn list_issues(&self, input: ListIssuesInput) -> ProcessResult {
        let limit = input.limit.unwrap_or(20);
        if !(1..=100).contains(&limit) {
            return Err(process_error("issue list limit must be between 1 and 100"));
        }
        let mut args = vec![
            "issue".to_string(),
            "list".to_string(),
            "--repo".to_string(),
            self.gh_repository(),
            "--state".to_string(),
            "open".to_string(),
            "--limit".to_string(),
            limit.to_string(),
            "--json".to_string(),
            "number,title,url,labels,author".to_string(),
        ];
        for label in input.labels {
            if label.trim().is_empty() {
                return Err(process_error("issue label must not be empty"));
            }
            args.extend(["--label".to_string(), label]);
        }
        let issues = self.run_json(&args).await?;
        require_array(&issues, "issue list")?;
        Ok(json!({"repository": self.repository, "issues": issues}))
    }

    async fn get_issue(&self, input: NumberInput) -> ProcessResult {
        validate_number(input.number)?;
        let issue = self
            .run_json(&[
                "issue".into(),
                "view".into(),
                input.number.to_string(),
                "--repo".into(),
                self.gh_repository(),
                "--json".into(),
                "number,title,body,url,state,labels,author,assignees".into(),
            ])
            .await?;
        require_object(&issue, "issue")?;
        Ok(json!({"repository": self.repository, "issue": issue}))
    }

    async fn get_pull_request(&self, input: NumberInput) -> ProcessResult {
        validate_number(input.number)?;
        let pull_request = self.pull_request(input.number).await?;
        Ok(json!({"repository": self.repository, "pull_request": pull_request}))
    }

    async fn create_pull_request(&self, input: CreatePullRequestInput) -> ProcessResult {
        validate_branch(&input.head)?;
        validate_branch(&input.base)?;
        if input.title.trim().is_empty() || input.body.trim().is_empty() {
            return Err(process_error(
                "pull request title and body must not be empty",
            ));
        }
        if let Some(existing) = self.find_pull_request(&input.head, &input.base).await? {
            return Ok(json!({
                "repository": self.repository,
                "created": false,
                "pull_request": existing,
            }));
        }

        let mut args = vec![
            "pr".into(),
            "create".into(),
            "--repo".into(),
            self.gh_repository(),
            "--head".into(),
            input.head.clone(),
            "--base".into(),
            input.base.clone(),
            "--title".into(),
            input.title,
            "--body".into(),
            input.body,
        ];
        if input.draft {
            args.push("--draft".into());
        }
        let create = self.run(&args).await;
        let observed = self.find_pull_request(&input.head, &input.base).await?;
        match (create, observed) {
            (_, Some(pull_request)) => Ok(json!({
                "repository": self.repository,
                "created": true,
                "pull_request": pull_request,
            })),
            (Err(error), None) => Err(error),
            (Ok(_), None) => Err(process_error(
                "gh reported pull request creation success but no matching pull request was found",
            )),
        }
    }

    async fn merge_pull_request(&self, input: MergePullRequestInput) -> ProcessResult {
        validate_number(input.number)?;
        validate_oid(&input.expected_head_oid)?;
        let before = self.pull_request(input.number).await?;
        let observed_head = before
            .get("headRefOid")
            .and_then(Value::as_str)
            .ok_or_else(|| process_error("pull request response omitted headRefOid"))?;
        if observed_head != input.expected_head_oid {
            return Err(process_error(format!(
                "pull request head changed: expected {}, observed {observed_head}",
                input.expected_head_oid
            )));
        }
        if before.get("state").and_then(Value::as_str) == Some("MERGED") {
            return Ok(json!({
                "repository": self.repository,
                "merged": true,
                "already_merged": true,
                "pull_request": before,
            }));
        }
        if before.get("state").and_then(Value::as_str) != Some("OPEN") {
            return Err(process_error("only an open pull request can be merged"));
        }
        if before.get("isDraft").and_then(Value::as_bool) != Some(false) {
            return Err(process_error("a draft pull request cannot be merged"));
        }
        self.run(&[
            "pr".into(),
            "merge".into(),
            input.number.to_string(),
            "--repo".into(),
            self.gh_repository(),
            format!("--{}", input.method.as_flag()),
            "--match-head-commit".into(),
            input.expected_head_oid,
        ])
        .await?;
        let after = self.pull_request(input.number).await?;
        if after.get("state").and_then(Value::as_str) != Some("MERGED") {
            return Err(process_error(
                "merge command completed but GitHub does not report the pull request merged",
            ));
        }
        Ok(json!({
            "repository": self.repository,
            "merged": true,
            "already_merged": false,
            "pull_request": after,
        }))
    }

    async fn find_pull_request(
        &self,
        head: &str,
        base: &str,
    ) -> Result<Option<Value>, ProcessCapabilityError> {
        let value = self
            .run_json(&[
                "pr".into(),
                "list".into(),
                "--repo".into(),
                self.gh_repository(),
                "--head".into(),
                head.to_string(),
                "--state".into(),
                "all".into(),
                "--limit".into(),
                "100".into(),
                "--json".into(),
                "number,title,url,state,isDraft,headRefName,headRefOid,baseRefName,mergedAt".into(),
            ])
            .await?;
        let values = require_array(&value, "pull request list")?;
        Ok(values
            .iter()
            .find(|pull_request| {
                pull_request.get("headRefName").and_then(Value::as_str) == Some(head)
                    && pull_request.get("baseRefName").and_then(Value::as_str) == Some(base)
            })
            .cloned())
    }

    async fn pull_request(&self, number: u64) -> Result<Value, ProcessCapabilityError> {
        let value = self
            .run_json(&[
                "pr".into(),
                "view".into(),
                number.to_string(),
                "--repo".into(),
                self.gh_repository(),
                "--json".into(),
                "number,title,url,state,isDraft,mergeStateStatus,headRefName,headRefOid,baseRefName,statusCheckRollup,mergedAt".into(),
            ])
            .await?;
        require_object(&value, "pull request")?;
        Ok(value)
    }

    async fn run_json(&self, args: &[String]) -> ProcessResult {
        let output = self.run(args).await?;
        serde_json::from_slice(&output)
            .map_err(|_| process_error("gh returned malformed JSON for a typed process action"))
    }

    async fn run(&self, args: &[String]) -> Result<Vec<u8>, ProcessCapabilityError> {
        let output = tokio::process::Command::new(&self.gh_binary)
            .args(args)
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .output()
            .await
            .map_err(|error| {
                process_error(format!(
                    "failed to launch gh binary {}: {error}",
                    display_path(&self.gh_binary)
                ))
            })?;
        if !output.status.success() {
            return Err(process_error(format!(
                "gh command failed with status {}",
                output
                    .status
                    .code()
                    .map_or_else(|| "unknown".to_string(), |code| code.to_string())
            )));
        }
        Ok(output.stdout)
    }
}

impl fmt::Debug for GitHubProcess {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GitHubProcess")
            .field("repository", &self.repository)
            .field("gh_binary", &self.gh_binary)
            .finish()
    }
}

impl ProcessCapability for GitHubProcess {
    fn descriptor(&self) -> ProcessCapabilityDescriptor {
        ProcessCapabilityDescriptor {
            id: Self::capability_id(),
            description: format!(
                "Sequential GitHub issue and pull-request workflow for {}",
                self.repository
            ),
            required_grants: BTreeSet::from([Self::read_grant()]),
            actions: vec![
                action(
                    "issues/list",
                    "List a bounded open-issue backlog",
                    list_schema(),
                    [],
                    false,
                ),
                action("issues/get", "Read one issue", number_schema(), [], false),
                action(
                    "pulls/get",
                    "Read one pull request and checks",
                    number_schema(),
                    [],
                    false,
                ),
                action(
                    "pulls/create",
                    "Create or reconcile a pull request for an existing branch",
                    create_pr_schema(),
                    [Self::pull_request_write_grant()],
                    true,
                ),
                action(
                    "pulls/merge",
                    "Merge an exact reviewed pull-request head",
                    merge_pr_schema(),
                    [Self::merge_grant()],
                    true,
                ),
            ],
            instructions: vec![format!(
                "This mission has a repository-scoped GitHub process for {}. Work sequentially in the current checkout. Use the typed GitHub actions for issue and PR facts, keep mission work items and artifacts current, and never infer PR-write or merge authority from prose. If an action is absent or refused, report the boundary instead of bypassing it.",
                self.repository
            )],
        }
    }

    fn invoke<'a>(&'a self, request: ProcessActionRequest) -> ProcessFuture<'a> {
        Box::pin(async move {
            self.invoke_action(request.action.as_str(), request.input)
                .await
        })
    }
}

type ProcessResult = Result<Value, ProcessCapabilityError>;

fn action(
    id: &str,
    description: &str,
    input_schema: Value,
    grants: impl IntoIterator<Item = AuthorityGrantId>,
    destructive: bool,
) -> ProcessActionSpec {
    ProcessActionSpec {
        id: ProcessActionId::new(id).expect("static action id"),
        description: description.to_string(),
        input_schema,
        required_grants: grants.into_iter().collect(),
        scope: if destructive {
            ProcessActionScope::RootOnly
        } else {
            ProcessActionScope::RunTree
        },
        destructive,
    }
}

fn decode<T: DeserializeOwned>(input: Value) -> Result<T, ProcessCapabilityError> {
    serde_json::from_value(input)
        .map_err(|error| process_error(format!("invalid action input: {error}")))
}

fn process_error(message: impl Into<String>) -> ProcessCapabilityError {
    ProcessCapabilityError(message.into())
}

fn validate_number(number: u64) -> Result<(), ProcessCapabilityError> {
    if number == 0 {
        Err(process_error("GitHub number must be greater than zero"))
    } else {
        Ok(())
    }
}

fn validate_branch(branch: &str) -> Result<(), ProcessCapabilityError> {
    let invalid = branch.is_empty()
        || branch.len() > 255
        || branch.starts_with('.')
        || branch.starts_with('/')
        || branch.ends_with('.')
        || branch.ends_with('/')
        || branch.ends_with(".lock")
        || branch.contains("..")
        || branch.contains("@{")
        || branch.chars().any(|character| {
            character.is_control() || character.is_whitespace() || "~^:?*[\\".contains(character)
        });
    if invalid {
        Err(process_error(
            "head and base must be safe local branch names",
        ))
    } else {
        Ok(())
    }
}

fn validate_oid(oid: &str) -> Result<(), ProcessCapabilityError> {
    if matches!(oid.len(), 40 | 64) && oid.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(process_error(
            "expected_head_oid must be a full 40- or 64-character hexadecimal object id",
        ))
    }
}

fn require_array<'a>(
    value: &'a Value,
    label: &str,
) -> Result<&'a Vec<Value>, ProcessCapabilityError> {
    value
        .as_array()
        .ok_or_else(|| process_error(format!("gh {label} response was not an array")))
}

fn require_object(value: &Value, label: &str) -> Result<(), ProcessCapabilityError> {
    value
        .as_object()
        .map(|_| ())
        .ok_or_else(|| process_error(format!("gh {label} response was not an object")))
}

fn display_path(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ListIssuesInput {
    #[serde(default)]
    limit: Option<u64>,
    #[serde(default)]
    labels: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct NumberInput {
    number: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CreatePullRequestInput {
    head: String,
    base: String,
    title: String,
    body: String,
    #[serde(default = "default_draft")]
    draft: bool,
}

fn default_draft() -> bool {
    true
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MergePullRequestInput {
    number: u64,
    expected_head_oid: String,
    #[serde(default)]
    method: MergeMethod,
}

#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
enum MergeMethod {
    Merge,
    Rebase,
    #[default]
    Squash,
}

impl MergeMethod {
    fn as_flag(self) -> &'static str {
        match self {
            Self::Merge => "merge",
            Self::Rebase => "rebase",
            Self::Squash => "squash",
        }
    }
}

fn list_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "limit": {"type": "integer", "minimum": 1, "maximum": 100},
            "labels": {"type": "array", "items": {"type": "string"}}
        }
    })
}

fn number_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["number"],
        "properties": {"number": {"type": "integer", "minimum": 1}}
    })
}

fn create_pr_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["head", "base", "title", "body"],
        "properties": {
            "head": {"type": "string"},
            "base": {"type": "string"},
            "title": {"type": "string"},
            "body": {"type": "string"},
            "draft": {"type": "boolean", "default": true}
        }
    })
}

fn merge_pr_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["number", "expected_head_oid"],
        "properties": {
            "number": {"type": "integer", "minimum": 1},
            "expected_head_oid": {"type": "string"},
            "method": {"type": "string", "enum": ["merge", "rebase", "squash"], "default": "squash"}
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn process() -> GitHubProcess {
        GitHubProcess::new(GitHubRepository::new("Owner/Project").unwrap())
    }

    #[test]
    fn repository_identity_is_exact_normalized_and_validated() {
        assert_eq!(
            GitHubRepository::new("Owner/Project").unwrap().as_str(),
            "owner/project"
        );
        for invalid in [
            "project",
            "owner/",
            "/project",
            "owner/project/extra",
            ".owner/project",
            "owner/.project",
            "owner/project name",
        ] {
            assert!(
                GitHubRepository::new(invalid).is_err(),
                "accepted {invalid}"
            );
        }
    }

    #[test]
    fn descriptor_separates_read_pr_write_and_merge_authority() {
        let descriptor = process().descriptor();
        assert_eq!(descriptor.id, GitHubProcess::capability_id());
        assert_eq!(
            descriptor.required_grants,
            BTreeSet::from([GitHubProcess::read_grant()])
        );
        let grants = |id: &str| {
            descriptor
                .actions
                .iter()
                .find(|action| action.id.as_str() == id)
                .unwrap()
                .required_grants
                .clone()
        };
        assert!(grants("issues/list").is_empty());
        assert!(grants("issues/get").is_empty());
        assert!(grants("pulls/get").is_empty());
        assert_eq!(
            grants("pulls/create"),
            BTreeSet::from([GitHubProcess::pull_request_write_grant()])
        );
        assert_eq!(
            grants("pulls/merge"),
            BTreeSet::from([GitHubProcess::merge_grant()])
        );
        for action in &descriptor.actions {
            let expected = if action.destructive {
                ProcessActionScope::RootOnly
            } else {
                ProcessActionScope::RunTree
            };
            assert_eq!(action.scope, expected, "scope for {}", action.id);
        }
    }

    #[test]
    fn typed_inputs_reject_unknown_fields_and_unsafe_refs() {
        assert!(decode::<NumberInput>(json!({"number": 1, "extra": true})).is_err());
        for branch in ["", "../main", "main.lock", "fork:branch", "bad branch"] {
            assert!(validate_branch(branch).is_err(), "accepted {branch}");
        }
        assert!(validate_branch("agent/issue-42").is_ok());
        assert!(validate_oid("abc").is_err());
        assert!(validate_oid(&"a".repeat(40)).is_ok());
    }

    #[cfg(unix)]
    mod unix {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;

        use tempfile::TempDir;

        use super::*;

        struct FakeGh {
            _temp: TempDir,
            process: GitHubProcess,
            log: PathBuf,
        }

        impl FakeGh {
            fn new(body: &str) -> Self {
                let temp = tempfile::tempdir().unwrap();
                let binary = temp.path().join("gh");
                let log = temp.path().join("commands.log");
                let state = temp.path().join("state");
                let script = format!(
                    "#!/bin/sh\nset -eu\nlog={}\nstate={}\nprintf '%s\\n' \"$*\" >> \"$log\"\n{}\n",
                    shell_quote(&log),
                    shell_quote(&state),
                    body
                );
                fs::write(&binary, script).unwrap();
                let mut permissions = fs::metadata(&binary).unwrap().permissions();
                permissions.set_mode(0o700);
                fs::set_permissions(&binary, permissions).unwrap();
                let process = process().with_binary(&binary);
                Self {
                    _temp: temp,
                    process,
                    log,
                }
            }

            fn log(&self) -> String {
                fs::read_to_string(&self.log).unwrap_or_default()
            }
        }

        fn shell_quote(path: &Path) -> String {
            format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"))
        }

        #[tokio::test]
        async fn issue_reads_are_typed_and_repository_scoped() {
            let fake = FakeGh::new(
                r#"
case "$1 $2" in
  "issue list") printf '%s\n' '[{"number":7,"title":"Fix it"}]' ;;
  "issue view") printf '%s\n' '{"number":7,"title":"Fix it","state":"OPEN"}' ;;
  *) exit 9 ;;
esac
"#,
            );

            let listed = fake
                .process
                .invoke_action("issues/list", json!({"limit": 5, "labels": ["bug"]}))
                .await
                .unwrap();
            assert_eq!(listed["repository"], "owner/project");
            assert_eq!(listed["issues"][0]["number"], 7);
            let issue = fake
                .process
                .invoke_action("issues/get", json!({"number": 7}))
                .await
                .unwrap();
            assert_eq!(issue["issue"]["state"], "OPEN");

            let log = fake.log();
            assert!(
                log.lines()
                    .all(|line| line.contains("--repo github.com/owner/project"))
            );
            assert!(log.contains("--label bug"));
        }

        #[tokio::test]
        async fn pull_request_creation_reconciles_response_loss_and_retries() {
            let fake = FakeGh::new(
                r#"
case "$1 $2" in
  "pr list")
    if test -f "$state"; then
      printf '%s\n' '[{"number":11,"state":"OPEN","isDraft":true,"headRefName":"agent/11","headRefOid":"abc","baseRefName":"main"}]'
    else
      printf '%s\n' '[]'
    fi
    ;;
  "pr create") touch "$state"; exit 17 ;;
  *) exit 9 ;;
esac
"#,
            );
            let input = json!({
                "head": "agent/11",
                "base": "main",
                "title": "Fix issue 11",
                "body": "Closes #11"
            });

            let created = fake
                .process
                .invoke_action("pulls/create", input.clone())
                .await
                .unwrap();
            assert_eq!(created["created"], true);
            assert_eq!(created["pull_request"]["number"], 11);
            let replay = fake
                .process
                .invoke_action("pulls/create", input)
                .await
                .unwrap();
            assert_eq!(replay["created"], false);
            assert_eq!(replay["pull_request"]["number"], 11);
            assert_eq!(fake.log().matches("pr create").count(), 1);
        }

        #[tokio::test]
        async fn merge_requires_the_exact_reviewed_head_and_is_idempotent() {
            let fake = FakeGh::new(
                r#"
case "$1 $2" in
  "pr view")
    if test -f "$state"; then state=MERGED; else state=OPEN; fi
    printf '{"number":11,"state":"%s","isDraft":false,"headRefOid":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","headRefName":"agent/11","baseRefName":"main"}\n' "$state"
    ;;
  "pr merge") touch "$state" ;;
  *) exit 9 ;;
esac
"#,
            );

            let error = fake
                .process
                .invoke_action(
                    "pulls/merge",
                    json!({"number": 11, "expected_head_oid": "b".repeat(40)}),
                )
                .await
                .unwrap_err();
            assert!(error.to_string().contains("head changed"));
            assert!(!fake.log().contains("pr merge"));

            let merged = fake
                .process
                .invoke_action(
                    "pulls/merge",
                    json!({
                        "number": 11,
                        "expected_head_oid": "a".repeat(40),
                        "method": "squash"
                    }),
                )
                .await
                .unwrap();
            assert_eq!(merged["merged"], true);
            assert_eq!(merged["already_merged"], false);
            assert!(
                fake.log()
                    .contains(&format!("--match-head-commit {}", "a".repeat(40)))
            );

            let replay = fake
                .process
                .invoke_action(
                    "pulls/merge",
                    json!({"number": 11, "expected_head_oid": "a".repeat(40)}),
                )
                .await
                .unwrap();
            assert_eq!(replay["already_merged"], true);
            assert_eq!(fake.log().matches("pr merge").count(), 1);

            let error = fake
                .process
                .invoke_action(
                    "pulls/merge",
                    json!({"number": 11, "expected_head_oid": "b".repeat(40)}),
                )
                .await
                .unwrap_err();
            assert!(error.to_string().contains("head changed"));
            assert_eq!(fake.log().matches("pr merge").count(), 1);
        }

        #[tokio::test]
        async fn command_failures_do_not_echo_gh_stderr() {
            let fake = FakeGh::new("printf '%s\\n' 'credential=secret-value' >&2\nexit 7");
            let error = fake
                .process
                .invoke_action("issues/get", json!({"number": 1}))
                .await
                .unwrap_err()
                .to_string();
            assert!(error.contains("status 7"));
            assert!(!error.contains("secret-value"));
        }

        #[tokio::test]
        async fn malformed_gh_json_is_refused() {
            let fake = FakeGh::new("printf '%s\\n' 'not-json'");
            let error = fake
                .process
                .invoke_action("issues/list", json!({}))
                .await
                .unwrap_err();
            assert!(error.to_string().contains("malformed JSON"));
        }
    }
}
