//! The internal config-and-run seam (#407).
//!
//! [`run`] is roba's resolve-free, side-effect-free core: a [`Config`] in, a
//! [`claude_wrapper::types::QueryResult`] out. No clap, no stdout/stderr, no
//! `process::exit`, no TTY, no interactive prompts. The CLI's `run_ask`
//! resolves flags/profiles/prompt and renders the result around this; a
//! programmatic caller (or, later, `serve` -- see #142) builds a `Config`
//! directly.
//!
//! v1 is deliberately small and faithful: `Config` exposes the common run
//! knobs and reuses the proven [`apply_session`] mapper (via
//! `Config::to_ask_args`), so there is no second flag->command mapper to
//! drift. Knobs not on `Config` take their safe `AskArgs` defaults (read-only
//! permissions, a fresh session, no caps). Growing `Config` toward full
//! `AskArgs` fidelity and routing `run_ask` through it are the follow-up
//! phases in #407.
//!
//! Out of scope for v1 (stays in the CLI layer): prompt composition
//! (attach/git/prepend/vars -- `Config` takes the already-composed prompt),
//! profile/env layering, live streaming display, output formatting, and
//! exit-code classification.

use anyhow::Result;
use claude_wrapper::types::QueryResult;
use claude_wrapper::{Claude, QueryCommand};

use crate::cli::AskArgs;
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
    /// Permission posture (default read-only).
    pub permissions: Permissions,
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
}

impl Config {
    /// Construct a `Config` for a plain prompt; chain the setters for the rest.
    pub fn new(prompt: impl Into<String>) -> Self {
        Self {
            prompt: prompt.into(),
            ..Self::default()
        }
    }

    /// Project this `Config` onto a defaulted [`AskArgs`] so the run reuses the
    /// single proven flag->`QueryCommand` mapper ([`apply_session`]). Only the
    /// knobs `Config` exposes are set; everything else takes its `AskArgs`
    /// default (read-only, fresh, no caps), which is exactly roba's safe
    /// default. `timeout_secs` lives on the `Claude` client, not here.
    fn to_ask_args(&self) -> AskArgs {
        let (continue_session, session_id) = match &self.session {
            Session::Fresh => (None, None),
            Session::Continue => (Some(None), None),
            Session::Resume(id) => (Some(Some(id.clone())), None),
            Session::WithId(id) => (None, Some(id.clone())),
        };
        let (writable, full_auto) = match self.permissions {
            Permissions::ReadOnly => (false, false),
            Permissions::Writable => (true, false),
            Permissions::FullAuto => (false, true),
        };
        AskArgs {
            model: self.model.clone(),
            writable,
            full_auto,
            allow_tool: self.allow_tools.clone(),
            deny_tool: self.deny_tools.clone(),
            continue_session,
            session_id,
            fork: self.fork,
            worktree: self.worktree.clone(),
            max_turns: self.max_turns,
            max_budget_usd: self.max_budget_usd,
            json_schema: self.json_schema.clone(),
            ..AskArgs::default()
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

    let args = config.to_ask_args();
    let name = derive_session_name(&config.prompt);
    let cmd = apply_session(
        QueryCommand::new(config.prompt.clone())
            .name(name)
            .prompt_via_stdin(true),
        &args,
    );
    let mut result = cmd.execute_json(&claude).await?;

    // Parity with run_ask: with a schema active, surface the structured answer
    // onto `structured_output` / an unfenced `result` (see #317).
    if config.json_schema.is_some() {
        crate::output::surface_structured_output(&mut result);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_ask_args_defaults_are_safe() {
        // A bare Config maps to the safe defaults: read-only (no writable/
        // full_auto), a fresh session, no caps.
        let args = Config::new("hi").to_ask_args();
        assert!(!args.writable && !args.full_auto, "default is read-only");
        assert!(
            args.continue_session.is_none() && args.session_id.is_none(),
            "default is a fresh session"
        );
        assert!(args.worktree.is_none() && !args.fork);
        assert!(args.max_turns.is_none() && args.max_budget_usd.is_none());
    }

    #[test]
    fn to_ask_args_maps_session_variants() {
        let resume = Config {
            session: Session::Resume("abc123".into()),
            fork: true,
            ..Config::new("p")
        }
        .to_ask_args();
        assert_eq!(resume.continue_session, Some(Some("abc123".to_string())));
        assert!(resume.session_id.is_none() && resume.fork);

        let cont = Config {
            session: Session::Continue,
            ..Config::new("p")
        }
        .to_ask_args();
        assert_eq!(cont.continue_session, Some(None));

        let with_id = Config {
            session: Session::WithId("mine".into()),
            ..Config::new("p")
        }
        .to_ask_args();
        assert_eq!(with_id.session_id, Some("mine".to_string()));
        assert!(with_id.continue_session.is_none());
    }

    #[test]
    fn to_ask_args_maps_permissions_and_tools() {
        let writable = Config {
            permissions: Permissions::Writable,
            ..Config::new("p")
        }
        .to_ask_args();
        assert!(writable.writable && !writable.full_auto);

        let auto = Config {
            permissions: Permissions::FullAuto,
            allow_tools: vec!["Bash(git:*)".into()],
            deny_tools: vec!["Write".into()],
            ..Config::new("p")
        }
        .to_ask_args();
        assert!(auto.full_auto);
        assert_eq!(auto.allow_tool, vec!["Bash(git:*)".to_string()]);
        assert_eq!(auto.deny_tool, vec!["Write".to_string()]);
    }
}
