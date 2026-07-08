//! The internal config-and-run seam (#407).
//!
//! [`run`] is roba's resolve-free, side-effect-free core: a [`Config`] in, a
//! [`claude_wrapper::types::QueryResult`] out. No clap, no stdout/stderr, no
//! `process::exit`, no TTY, no interactive prompts. The CLI's `run_ask`
//! resolves flags/profiles/prompt and renders the result around this; a
//! programmatic caller (or, later, `serve` -- see #142) builds a `Config`
//! directly.
//!
//! `Config` covers the full run-shaping surface that [`apply_session`]
//! consumes -- model/fallback/effort/agent, the permission posture +
//! `--permission-mode`, session continuity, worktree, caps, timeout, the
//! system-prompt overrides + the agent notice, retry/persistence/bare/safe
//! modes, `--add-dir`, and MCP config. It feeds the proven `apply_session`
//! mapper directly (`apply_session` reads `&Config`), so there is one
//! flag->command mapper and no second copy to drift; a bare `Config` maps to
//! roba's safe defaults (read-only, a fresh session, no caps, and the built-in
//! agent notice injected). The CLI's `run_ask` builds a `Config` from its
//! resolved `AskArgs` (`build_config`) and routes through this same seam.
//!
//! Out of scope for v1 (stays in the CLI layer): prompt composition
//! (attach/git/prepend/vars -- `Config` takes the already-composed prompt),
//! profile/env layering, live streaming display, output formatting, and
//! exit-code classification.

use anyhow::Result;
use claude_wrapper::types::QueryResult;
use claude_wrapper::{Claude, Effort, PermissionMode, QueryCommand};

use crate::session::{apply_session, derive_session_name};

/// What to do about session continuity, mirroring the CLI's `-c` / `--resume`
/// / `--session-id` / `--fresh` selectors as one closed choice.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Session {
    /// Start a new session (roba's default).
    #[default]
    Fresh,
    /// Continue the most recent session in the working directory.
    Continue,
    /// Resume a specific existing session by id.
    Resume(String),
    /// Start a new session with a caller-chosen id (for later re-attachment).
    WithId(String),
}

/// The permission posture for a run. Mirrors roba's safe-by-default model:
/// the default is read-only (Read/Glob/Grep), and the variants open it up.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Permissions {
    /// Read-only: Read, Glob, Grep only (roba's default).
    #[default]
    ReadOnly,
    /// Read-only plus Edit + Write.
    Writable,
    /// Bypass all permission checks (sandbox / unattended-worker use only).
    FullAuto,
}

/// A config-and-run request: everything [`run`] needs to send one prompt
/// through claude and hand back the typed result. The `prompt` is the final,
/// already-composed text (the CLI does attach/git/var composition before
/// building this).
#[derive(Debug, Clone, Default)]
pub struct Config {
    /// The fully-composed prompt to send.
    pub prompt: String,
    /// Override the model for this run (`None` = claude's default).
    pub model: Option<String>,
    /// Fallback model when the primary is overloaded (`--fallback-model`).
    pub fallback_model: Option<String>,
    /// Reasoning-effort level (`None` = claude's default). The CLI maps its
    /// `--effort` value to this claude-wrapper type in `build_config`.
    pub effort: Option<Effort>,
    /// Run under a named subagent definition (`--agent`).
    pub agent: Option<String>,
    /// Permission posture (default read-only).
    pub permissions: Permissions,
    /// A specific claude permission mode composed on top of the posture. `None`
    /// leaves claude's default. Ignored under [`Permissions::FullAuto`], which
    /// bypasses all checks before the mode is read. The CLI maps its
    /// `--permission-mode` value to this claude-wrapper type in `build_config`.
    pub permission_mode: Option<PermissionMode>,
    /// Extra allowed tool patterns layered on top of the posture.
    pub allow_tools: Vec<String>,
    /// Tool patterns to block (ignored under [`Permissions::FullAuto`]).
    pub deny_tools: Vec<String>,
    /// Session continuity (default a fresh session).
    pub session: Session,
    /// Branch the resumed session instead of continuing it in place. Only
    /// meaningful with [`Session::Resume`].
    pub fork: bool,
    /// Run in a git worktree: `None` = no worktree, `Some(None)` = a fresh
    /// anonymous worktree, `Some(Some(name))` = a named worktree.
    pub worktree: Option<Option<String>>,
    /// Cap the agentic turn count (`None` = uncapped).
    pub max_turns: Option<u32>,
    /// Cap total spend in USD (`None` = uncapped).
    pub max_budget_usd: Option<f64>,
    /// Wall-clock deadline in seconds (`None` or `0` = no deadline).
    pub timeout_secs: Option<u64>,
    /// Constrain output to a JSON Schema. The value is the inline schema
    /// JSON (not a path -- the CLI's path-reading sugar is its own concern).
    pub json_schema: Option<String>,
    /// Replace claude's system prompt (`--system-prompt`). `None` keeps it.
    pub system_prompt: Option<String>,
    /// Append to the system prompt (`--append-system-prompt`); composed with
    /// the agent notice below into one appended block.
    pub append_system_prompt: Option<String>,
    /// Override the built-in single-turn agent-notice text (`--agent-notice`).
    /// `Some("")` disables the notice (as does `no_agent_notice`); `None` uses
    /// the built-in text.
    pub agent_notice: Option<String>,
    /// Suppress the built-in single-turn agent notice (`--no-agent-notice`).
    /// Default `false` -- the notice is injected, faithful to the CLI default.
    pub no_agent_notice: bool,
    /// Fail fast on a transient error instead of retrying (`--no-retry`): a
    /// single attempt, no backoff.
    pub no_retry: bool,
    /// Do not persist the session to claude's history
    /// (`--no-session-persistence`).
    pub no_session_persistence: bool,
    /// Bare mode (`--bare`): the minimal claude invocation.
    pub bare: bool,
    /// Safe mode (`--safe-mode`): claude's extra guardrails.
    pub safe_mode: bool,
    /// Extra tool-access directories (`--add-dir`), forwarded verbatim.
    pub add_dir: Vec<String>,
    /// MCP server config files (`--mcp-config`), forwarded verbatim.
    pub mcp_config: Vec<String>,
    /// Use only the servers in `mcp_config`, ignoring other MCP sources
    /// (`--strict-mcp-config`).
    pub strict_mcp_config: bool,
    /// Comma-separated whitelist of ambient claude setting sources to load
    /// (`user,project,local`); `None` uses claude's default. The claude-hermetic
    /// lever (`--setting-sources`): restrict it to seal ambient config.
    pub setting_sources: Option<String>,
}

impl Config {
    /// Construct a `Config` for a plain prompt; chain the setters for the rest.
    pub fn new(prompt: impl Into<String>) -> Self {
        Self {
            prompt: prompt.into(),
            ..Self::default()
        }
    }
}

/// Run one prompt through claude under `config` and return the typed result.
///
/// Side-effect-free: no printing, no exit, no TTY. Builds the `Claude` client
/// (honouring `timeout_secs`), maps `config` onto a `QueryCommand` via the
/// shared [`apply_session`], executes, and surfaces schema-constrained output
/// the same way the CLI does. Errors propagate as `anyhow` (the CLI maps them
/// to typed exit codes; a programmatic caller inspects them directly).
pub async fn run(config: &Config) -> Result<QueryResult> {
    let mut builder = Claude::builder();
    if let Some(secs) = config.timeout_secs
        && secs > 0
    {
        builder = builder.timeout_secs(secs);
    }
    let claude = builder.build()?;

    let mut result = execute(config, &claude).await?;

    // Parity with run_ask: with a schema active, surface the structured answer
    // onto `structured_output` / an unfenced `result` (see #317).
    if config.json_schema.is_some() {
        surface_structured_output(&mut result);
    }
    Ok(result)
}

/// Build the `QueryCommand` for `config` -- through the one proven
/// [`apply_session`] mapper -- and run it non-streaming, returning the typed
/// result. The single build+execute path shared by [`run`] and the CLI's
/// non-streaming branch (`run_ask`), which both hand this the same [`Config`],
/// so there is no parallel copy to drift.
///
/// Schema surfacing is the caller's job -- the CLI shares it with its `--trace`
/// path, and [`run`] does it after this returns.
pub async fn execute(config: &Config, claude: &Claude) -> Result<QueryResult> {
    let name = derive_session_name(&config.prompt);
    let result = apply_session(
        QueryCommand::new(config.prompt.clone())
            .name(name)
            .prompt_via_stdin(true),
        config,
    )
    .execute_json(claude)
    .await?;
    Ok(result)
}

/// Surface `--json-schema` structured output cleanly (#317).
///
/// In practice claude delivers the schema-constrained answer as a fenced
/// ```` ```json ```` block inside `result.result` and does NOT populate a
/// `structured_output` field. This normalizes that: when the body is a
/// single fenced block whose contents parse as JSON, the parsed value is
/// inserted under `result.extra["structured_output"]` (so `--json`
/// consumers read `.result.structured_output` directly) and
/// `result.result` is replaced with the unfenced text (so
/// `.result.result` and the default text body are clean too).
///
/// No-ops when `structured_output` is already present and non-null (a
/// future wrapper/claude version may surface it natively), or when the
/// body is not a single parseable fenced JSON block. The caller gates
/// this on `--json-schema` so a normal answer containing a JSON code
/// block is never reinterpreted as structured output.
pub fn surface_structured_output(result: &mut QueryResult) {
    if result
        .extra
        .get("structured_output")
        .is_some_and(|v| !v.is_null())
    {
        return;
    }
    let Some(inner) = unfence_single_block(&result.result) else {
        return;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&inner) else {
        return;
    };
    result.extra.insert("structured_output".to_string(), value);
    result.result = inner;
}

/// If `text` is a single fenced code block -- it opens with a line
/// starting ```` ``` ```` (optionally a language tag) and closes with a
/// bare ```` ``` ```` line, with nothing outside the fence -- return the
/// inner content (trimmed). Returns `None` otherwise, so a normal answer
/// that merely contains a code block mid-paragraph is left untouched.
fn unfence_single_block(text: &str) -> Option<String> {
    let trimmed = text.trim();
    let mut lines = trimmed.lines();
    let first = lines.next()?;
    if !first.starts_with("```") {
        return None;
    }
    let rest: Vec<&str> = lines.collect();
    let (last, body) = rest.split_last()?;
    if last.trim() != "```" {
        return None;
    }
    Some(body.join("\n").trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_new_defaults_are_safe() {
        // A bare Config is roba's safe default: read-only, a fresh session, no
        // caps, the agent notice on. The Config -> QueryCommand mapping is
        // exercised via apply_session (session.rs); the AskArgs -> Config
        // mapping via build_config (lib.rs).
        let c = Config::new("hi");
        assert_eq!(c.prompt, "hi");
        assert!(matches!(c.permissions, Permissions::ReadOnly));
        assert!(matches!(c.session, Session::Fresh));
        assert!(!c.fork && c.worktree.is_none());
        assert!(c.max_turns.is_none() && c.max_budget_usd.is_none());
        assert!(!c.no_agent_notice && c.agent_notice.is_none());
    }

    // -- schema output surfacing (#317) ------------------------------------

    fn query_result_body(body: &str) -> QueryResult {
        serde_json::from_value(serde_json::json!({
            "result": body,
            "session_id": "s1",
            "is_error": false,
        }))
        .expect("build QueryResult fixture")
    }

    #[test]
    fn surface_structured_output_unfences_json_block() {
        // The real --json-schema shape (#317): a fenced ```json block in
        // `result.result`, no `structured_output` key.
        let mut result = query_result_body("```json\n{\"answer\": \"Paris\"}\n```");
        surface_structured_output(&mut result);
        assert_eq!(
            result.extra.get("structured_output").unwrap()["answer"],
            "Paris"
        );
        // result.result is unfenced so `.result.result` reads cleanly.
        assert_eq!(result.result, "{\"answer\": \"Paris\"}");
    }

    #[test]
    fn surface_structured_output_handles_untagged_fence() {
        let mut result = query_result_body("```\n[1, 2, 3]\n```");
        surface_structured_output(&mut result);
        assert_eq!(
            result.extra.get("structured_output").unwrap(),
            &serde_json::json!([1, 2, 3])
        );
    }

    #[test]
    fn surface_structured_output_noop_when_already_present() {
        // A native structured_output is never clobbered by the body.
        let mut result: QueryResult = serde_json::from_value(serde_json::json!({
            "result": "```json\n{\"from\": \"body\"}\n```",
            "session_id": "s1",
            "is_error": false,
            "structured_output": {"from": "native"},
        }))
        .expect("build QueryResult fixture");
        surface_structured_output(&mut result);
        assert_eq!(
            result.extra.get("structured_output").unwrap()["from"],
            "native"
        );
        // The native path leaves the body unchanged.
        assert!(result.result.contains("```"));
    }

    #[test]
    fn surface_structured_output_noop_on_non_fenced_text() {
        let mut result = query_result_body("The answer is 42.");
        surface_structured_output(&mut result);
        assert!(!result.extra.contains_key("structured_output"));
        assert_eq!(result.result, "The answer is 42.");
    }

    #[test]
    fn surface_structured_output_noop_when_fence_is_not_json() {
        // A fenced block that does not parse as JSON is left untouched.
        let mut result = query_result_body("```rust\nfn main() {}\n```");
        surface_structured_output(&mut result);
        assert!(!result.extra.contains_key("structured_output"));
        assert!(result.result.contains("fn main"));
    }

    #[test]
    fn surface_structured_output_noop_on_prose_around_block() {
        // A real answer that merely contains a JSON block mid-text is not a
        // single fenced block, so it is not reinterpreted as structured output.
        let mut result =
            query_result_body("Here you go:\n```json\n{\"a\": 1}\n```\nHope that helps.");
        surface_structured_output(&mut result);
        assert!(!result.extra.contains_key("structured_output"));
    }
}
