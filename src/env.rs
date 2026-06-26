//! `ROBA_<PARAM>` env-var override layer.
//!
//! Sits between CLI flags (highest priority) and the file-based
//! profile pool. Lets the user override any single config knob for
//! one shell session without editing a file.
//!
//! # Naming
//!
//! Env vars match the CLI long-form, uppercased with `-` -> `_`, and
//! prefixed with `ROBA_`. So `--writable` -> `ROBA_WRITABLE`,
//! `--git-log` -> `ROBA_GIT_LOG`, `--allow-tool` -> `ROBA_ALLOW_TOOL`.
//!
//! # Value semantics
//!
//! - **String**: any non-empty value (e.g. `ROBA_MODEL=sonnet`).
//! - **Bool**: truthy (`1`, `true`, `yes`, `on`, case-insensitive)
//!   sets the flag. Other values are ignored -- this layer can only
//!   *enable*, never *disable*, a bool. To force a bool off when a
//!   file would set it on, use a profile selection.
//! - **Number** (e.g. `ROBA_GIT_LOG=5`): parsed; invalid values
//!   ignored.
//! - **List** (e.g. `ROBA_ALLOW_TOOL="Edit,Write"`): comma-separated,
//!   whitespace trimmed, empty entries dropped.
//! - **Map** (vars): one env var per key,
//!   `ROBA_VAR_<KEY>=<value>` (e.g. `ROBA_VAR_TICKET=ABC-123`).
//!
//! # Precedence rule
//!
//! The env layer fills only fields the CLI did NOT set. It does not
//! override an explicit CLI flag.

use crate::cli::{AskArgs, EffortLevel, PermMode};
use anyhow::{Result, bail};
use std::collections::HashMap;
use std::path::PathBuf;

/// Apply env-var overrides to fields the user didn't set on the
/// command line. Reads from the process environment.
///
/// Errors only if the environment carries a conflict the CLI layer would
/// have rejected (`ROBA_SESSION` together with `ROBA_SESSION_ID`); the
/// per-field fills themselves never error (an invalid value is silently
/// ignored).
pub fn apply_env_overrides(args: &mut AskArgs) -> Result<()> {
    let env: HashMap<String, String> = std::env::vars().collect();
    check_env_session_conflict(args, &env)?;
    apply_env_overrides_from(args, &env);
    Ok(())
}

/// Mirror the CLI-level `--session` / `--session-id` mutual exclusion at
/// the env layer.
///
/// clap rejects `--session` together with `--session-id`, but the env
/// layer applies `ROBA_SESSION` and `ROBA_SESSION_ID` independently, so
/// setting both (when the CLI set neither) would silently resolve to one
/// rather than surfacing the conflict. Detect that and bail with a message
/// naming both vars. If the CLI set either selector it wins, so the env
/// pair is not in play and no conflict is raised.
///
/// This is the env layer's only hard error -- every other env value is
/// best-effort (invalid values are ignored, never fatal).
fn check_env_session_conflict(args: &AskArgs, env: &HashMap<String, String>) -> Result<()> {
    if args.session.is_none()
        && args.session_id.is_none()
        && read_string(env, "ROBA_SESSION").is_some()
        && read_string(env, "ROBA_SESSION_ID").is_some()
    {
        bail!(
            "ROBA_SESSION and ROBA_SESSION_ID are mutually exclusive (same as --session / --session-id)"
        );
    }
    Ok(())
}

/// Same as [`apply_env_overrides`] but reads from a provided map.
/// Used by tests to avoid touching the real process environment.
pub fn apply_env_overrides_from(args: &mut AskArgs, env: &HashMap<String, String>) {
    // Tag anything the CLI already set as "CLI" provenance before the
    // env layer gets a chance to fill the gaps. The CLI is the highest
    // layer, so whatever is set here came from the command line.
    tag_cli_sources(args);

    // ----- Model -----
    if args.model.is_none()
        && let Some(s) = read_string(env, "ROBA_MODEL")
    {
        args.model = Some(s);
    }
    if args.fallback_model.is_none()
        && let Some(s) = read_string(env, "ROBA_FALLBACK_MODEL")
    {
        args.fallback_model = Some(s);
    }

    // ----- Effort -----
    if args.effort.is_none()
        && let Some(s) = read_string(env, "ROBA_EFFORT")
    {
        args.effort = match s.to_ascii_lowercase().as_str() {
            "low" => Some(EffortLevel::Low),
            "medium" => Some(EffortLevel::Medium),
            "high" => Some(EffortLevel::High),
            "xhigh" => Some(EffortLevel::Xhigh),
            "max" => Some(EffortLevel::Max),
            _ => None, // ignore unrecognized values
        };
    }

    // ----- Agent -----
    if args.agent.is_none()
        && let Some(s) = read_string(env, "ROBA_AGENT")
    {
        args.agent = Some(s);
    }

    // ----- System prompt -----
    if args.system_prompt.is_none()
        && let Some(s) = read_string(env, "ROBA_SYSTEM_PROMPT")
    {
        args.system_prompt = Some(s);
    }
    if args.append_system_prompt.is_none()
        && let Some(s) = read_string(env, "ROBA_APPEND_SYSTEM_PROMPT")
    {
        args.append_system_prompt = Some(s);
    }

    // ----- Composition -----
    if args.prepend.is_empty() {
        let paths = read_path_list(env, "ROBA_PREPEND");
        if !paths.is_empty() {
            args.prepend = paths;
        }
    }
    if args.append.is_empty() {
        let paths = read_path_list(env, "ROBA_APPEND");
        if !paths.is_empty() {
            args.append = paths;
        }
    }
    if args.attach.is_empty() {
        let attach = read_list(env, "ROBA_ATTACH");
        if !attach.is_empty() {
            args.attach = attach;
        }
    }
    if !args.git_diff && read_truthy(env, "ROBA_GIT_DIFF") {
        args.git_diff = true;
    }
    if args.git_log.is_none()
        && let Some(n) = read_usize(env, "ROBA_GIT_LOG")
    {
        args.git_log = Some(n);
    }
    if !args.git_status && read_truthy(env, "ROBA_GIT_STATUS") {
        args.git_status = true;
    }

    // ----- Sessions -----
    // ROBA_CONTINUE mirrors `-c` / `-c=ID`: a truthy value continues
    // the most recent session (`Some(None)`), any other non-empty,
    // non-falsy value is treated as a specific session id
    // (`Some(Some(id))`). Falsy / empty leaves it unset (fresh).
    if args.continue_session.is_none()
        && let Some(s) = env.get("ROBA_CONTINUE").filter(|s| !s.is_empty())
    {
        match s.to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => args.continue_session = Some(None),
            "0" | "false" | "no" | "off" => {} // explicit off -- stay fresh
            _ => args.continue_session = Some(Some(s.clone())),
        }
    }

    // ROBA_SESSION mirrors `--session NAME`: a configured `[session]`
    // handle resolved against the pool later in `run_ask`. String flag,
    // CLI wins.
    if args.session.is_none()
        && let Some(s) = read_string(env, "ROBA_SESSION")
    {
        args.session = Some(s);
    }

    // ROBA_SESSION_ID mirrors `--session-id UUID`: assign a
    // caller-chosen session id. String flag, CLI wins. claude validates
    // the UUID, so this layer just passes the value through.
    if args.session_id.is_none()
        && let Some(s) = read_string(env, "ROBA_SESSION_ID")
    {
        args.session_id = Some(s);
    }

    // ROBA_JSON_SCHEMA mirrors `--json-schema PATH`: the path to a JSON
    // Schema file. String flag, CLI wins. run_ask reads the path and
    // inlines + validates the contents, so this layer just carries the
    // path through.
    if args.json_schema.is_none()
        && let Some(s) = read_string(env, "ROBA_JSON_SCHEMA")
    {
        args.json_schema = Some(s);
    }

    // ROBA_NO_SESSION_PERSISTENCE mirrors --no-session-persistence:
    // truthy bool. Like every bool in this layer it can only enable.
    if !args.no_session_persistence && read_truthy(env, "ROBA_NO_SESSION_PERSISTENCE") {
        args.no_session_persistence = true;
    }

    // ----- Permissions -----
    if args.permission_mode.is_none()
        && let Some(s) = read_string(env, "ROBA_PERMISSION_MODE")
        && let Some(mode) = parse_permission_mode(&s)
    {
        args.permission_mode = Some(mode);
        args.permission_mode_source = Some("env".to_string());
    }
    if !args.readonly && read_truthy(env, "ROBA_READONLY") {
        args.readonly = true;
        args.readonly_source = Some("env".to_string());
    }
    if !args.writable && !args.readonly && read_truthy(env, "ROBA_WRITABLE") {
        args.writable = true;
        args.writable_source = Some("env".to_string());
    }
    if !args.full_auto && !args.writable && !args.readonly && read_truthy(env, "ROBA_FULL_AUTO") {
        args.full_auto = true;
        args.full_auto_source = Some("env".to_string());
    }
    if args.allow_tool.is_empty() {
        let tools = read_list(env, "ROBA_ALLOW_TOOL");
        if !tools.is_empty() {
            args.allow_tool_sources = vec!["env".to_string(); tools.len()];
            args.allow_tool = tools;
        }
    }
    if args.deny_tool.is_empty() {
        let tools = read_list(env, "ROBA_DENY_TOOL");
        if !tools.is_empty() {
            args.deny_tool_sources = vec!["env".to_string(); tools.len()];
            args.deny_tool = tools;
        }
    }
    // ROBA_ADD_DIR mirrors --add-dir: comma-separated list of extra
    // tool-access directories. List flag, CLI wins (only fills when empty).
    if args.add_dir.is_empty() {
        let dirs = read_list(env, "ROBA_ADD_DIR");
        if !dirs.is_empty() {
            args.add_dir_sources = vec!["env".to_string(); dirs.len()];
            args.add_dir = dirs;
        }
    }

    // ----- Output -----
    if !args.stream && read_truthy(env, "ROBA_STREAM") {
        args.stream = true;
    }
    if !args.show_thinking && read_truthy(env, "ROBA_SHOW_THINKING") {
        args.show_thinking = true;
    }
    if !args.echo && read_truthy(env, "ROBA_ECHO") {
        args.echo = true;
    }
    if !args.plain && read_truthy(env, "ROBA_PLAIN") {
        args.plain = true;
    }
    if !args.quiet && read_truthy(env, "ROBA_QUIET") {
        args.quiet = true;
    }
    if !args.json && read_truthy(env, "ROBA_JSON") {
        args.json = true;
    }
    if args.editor_history.is_none()
        && let Some(n) = read_usize(env, "ROBA_EDITOR_HISTORY")
    {
        args.editor_history = Some(n);
    }
    if args.trace.is_none()
        && let Some(s) = read_string(env, "ROBA_TRACE")
    {
        args.trace = Some(PathBuf::from(s));
    }
    if args.rates_file.is_none()
        && let Some(s) = read_string(env, "ROBA_RATES_FILE")
    {
        args.rates_file = Some(PathBuf::from(s));
    }
    if !args.no_dollars && read_truthy(env, "ROBA_NO_DOLLARS") {
        args.no_dollars = true;
    }
    // ROBA_NO_WORKTREE mirrors --no-worktree: a truthy bool that only
    // enables. It is the safe direction, so if both it and ROBA_WORKTREE
    // are set it wins -- the force-off in `run_ask` (apply_no_worktree)
    // is the backstop that nulls any worktree value regardless of order.
    if !args.no_worktree && read_truthy(env, "ROBA_NO_WORKTREE") {
        args.no_worktree = true;
    }
    if args.worktree.is_none()
        && let Some(s) = env.get("ROBA_WORKTREE").filter(|s| !s.is_empty())
    {
        let lower = s.to_ascii_lowercase();
        match lower.as_str() {
            "1" | "true" | "yes" | "on" => args.worktree = Some(None),
            "0" | "false" | "no" | "off" => {} // explicit off -- leave unset
            _ => args.worktree = Some(Some(s.clone())),
        }
    }

    if !args.no_agent_check && read_truthy(env, "ROBA_NO_AGENT_CHECK") {
        args.no_agent_check = true;
    }

    // ROBA_NO_AGENT_NOTICE mirrors --no-agent-notice: truthy bool that can
    // only enable. ROBA_AGENT_NOTICE mirrors --agent-notice: replacement
    // text, CLI wins (only fills when unset; empty values ignored).
    if !args.no_agent_notice && read_truthy(env, "ROBA_NO_AGENT_NOTICE") {
        args.no_agent_notice = true;
    }
    if args.agent_notice.is_none()
        && let Some(s) = read_string(env, "ROBA_AGENT_NOTICE")
    {
        args.agent_notice = Some(s);
    }

    // ----- Failure modes -----
    if !args.no_retry && read_truthy(env, "ROBA_NO_RETRY") {
        args.no_retry = true;
    }

    // ----- Limits -----
    if args.max_turns.is_none()
        && let Some(n) = read_u32(env, "ROBA_MAX_TURNS")
    {
        args.max_turns = Some(n);
    }
    if args.max_budget_usd.is_none()
        && let Some(v) = read_f64(env, "ROBA_MAX_BUDGET_USD")
    {
        args.max_budget_usd = Some(v);
    }
    if args.timeout.is_none()
        && let Some(n) = read_u64(env, "ROBA_TIMEOUT")
    {
        args.timeout = Some(n);
    }

    // ----- Mode -----
    if !args.bare && read_truthy(env, "ROBA_BARE") {
        args.bare = true;
    }

    // ----- MCP -----
    // ROBA_MCP_CONFIG mirrors --mcp-config: comma-separated list of config
    // file paths. List flag, CLI wins (only fills when CLI left it empty).
    if args.mcp_config.is_empty() {
        let configs = read_list(env, "ROBA_MCP_CONFIG");
        if !configs.is_empty() {
            args.mcp_config = configs;
        }
    }
    // ROBA_STRICT_MCP_CONFIG mirrors --strict-mcp-config: truthy bool.
    if !args.strict_mcp_config && read_truthy(env, "ROBA_STRICT_MCP_CONFIG") {
        args.strict_mcp_config = true;
    }

    // ----- Vars (ROBA_VAR_<KEY>=<value>) -----
    for (key, value) in env {
        if let Some(var_key) = key.strip_prefix("ROBA_VAR_")
            && !args.var.iter().any(|(k, _)| k == var_key)
        {
            args.var.push((var_key.to_string(), value.clone()));
        }
    }
}

/// Mark every permission-related field that is already set as
/// originating from the CLI. Called before the env layer fills gaps,
/// so any value present at this point must have come from clap (the
/// highest layer). Idempotent: only tags fields that are set and not
/// already tagged.
fn tag_cli_sources(args: &mut AskArgs) {
    if args.readonly && args.readonly_source.is_none() {
        args.readonly_source = Some("CLI".to_string());
    }
    if args.writable && args.writable_source.is_none() {
        args.writable_source = Some("CLI".to_string());
    }
    if args.full_auto && args.full_auto_source.is_none() {
        args.full_auto_source = Some("CLI".to_string());
    }
    if !args.allow_tool.is_empty() && args.allow_tool_sources.is_empty() {
        args.allow_tool_sources = vec!["CLI".to_string(); args.allow_tool.len()];
    }
    if !args.deny_tool.is_empty() && args.deny_tool_sources.is_empty() {
        args.deny_tool_sources = vec!["CLI".to_string(); args.deny_tool.len()];
    }
    if !args.add_dir.is_empty() && args.add_dir_sources.is_empty() {
        args.add_dir_sources = vec!["CLI".to_string(); args.add_dir.len()];
    }
    if args.permission_mode.is_some() && args.permission_mode_source.is_none() {
        args.permission_mode_source = Some("CLI".to_string());
    }
}

// ---------------------------------------------------------------------------
// Readers
// ---------------------------------------------------------------------------

fn read_string(env: &HashMap<String, String>, key: &str) -> Option<String> {
    env.get(key).filter(|s| !s.is_empty()).cloned()
}

/// Truthy means `1`/`true`/`yes`/`on` (case-insensitive). Everything
/// else -- including missing, empty, `0`/`false`/`no`/`off`, or
/// garbage -- is treated as "no override."
fn read_truthy(env: &HashMap<String, String>, key: &str) -> bool {
    match env.get(key) {
        Some(s) => matches!(s.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"),
        None => false,
    }
}

fn read_usize(env: &HashMap<String, String>, key: &str) -> Option<usize> {
    env.get(key)
        .filter(|s| !s.is_empty())
        .and_then(|s| s.parse().ok())
}

fn read_u32(env: &HashMap<String, String>, key: &str) -> Option<u32> {
    env.get(key)
        .filter(|s| !s.is_empty())
        .and_then(|s| s.parse().ok())
}

fn read_u64(env: &HashMap<String, String>, key: &str) -> Option<u64> {
    env.get(key)
        .filter(|s| !s.is_empty())
        .and_then(|s| s.parse().ok())
}

fn read_f64(env: &HashMap<String, String>, key: &str) -> Option<f64> {
    env.get(key)
        .filter(|s| !s.is_empty())
        .and_then(|s| s.parse().ok())
}

fn read_list(env: &HashMap<String, String>, key: &str) -> Vec<String> {
    env.get(key)
        .filter(|s| !s.is_empty())
        .map(|s| {
            s.split(',')
                .map(|p| p.trim().to_string())
                .filter(|p| !p.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

fn read_path_list(env: &HashMap<String, String>, key: &str) -> Vec<PathBuf> {
    read_list(env, key).into_iter().map(PathBuf::from).collect()
}

/// Parse `ROBA_PERMISSION_MODE` value (case-insensitive, camelCase or
/// snake_case) into a `PermMode`. Unknown values are silently
/// ignored so a mistyped env var never blocks a run.
fn parse_permission_mode(s: &str) -> Option<PermMode> {
    match s.to_ascii_lowercase().as_str() {
        "acceptedits" | "accept_edits" => Some(PermMode::AcceptEdits),
        "auto" => Some(PermMode::Auto),
        "bypasspermissions" | "bypass_permissions" => Some(PermMode::BypassPermissions),
        "default" => Some(PermMode::Default),
        "dontask" | "dont_ask" => Some(PermMode::DontAsk),
        "plan" => Some(PermMode::Plan),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::Cli;
    use clap::Parser;

    fn empty_args() -> AskArgs {
        Cli::try_parse_from(["roba", "placeholder"]).unwrap().ask
    }

    fn env_with(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    // -- model -------------------------------------------------------------

    #[test]
    fn model_fills_when_cli_unset() {
        let mut args = empty_args();
        apply_env_overrides_from(&mut args, &env_with(&[("ROBA_MODEL", "sonnet")]));
        assert_eq!(args.model.as_deref(), Some("sonnet"));
    }

    #[test]
    fn model_does_not_override_cli() {
        let mut args = empty_args();
        args.model = Some("opus".into());
        apply_env_overrides_from(&mut args, &env_with(&[("ROBA_MODEL", "sonnet")]));
        assert_eq!(args.model.as_deref(), Some("opus"));
    }

    // -- agent -------------------------------------------------------------

    #[test]
    fn env_agent_sets_from_roba_agent_var() {
        let mut args = empty_args();
        apply_env_overrides_from(&mut args, &env_with(&[("ROBA_AGENT", "reviewer")]));
        assert_eq!(args.agent.as_deref(), Some("reviewer"));
    }

    #[test]
    fn env_agent_does_not_override_cli() {
        let mut args = empty_args();
        args.agent = Some("planner".into());
        apply_env_overrides_from(&mut args, &env_with(&[("ROBA_AGENT", "reviewer")]));
        assert_eq!(args.agent.as_deref(), Some("planner"));
    }

    // -- bools -------------------------------------------------------------

    #[test]
    fn writable_accepts_common_truthy_values() {
        for val in ["1", "true", "yes", "on", "TRUE", "Yes"] {
            let mut args = empty_args();
            apply_env_overrides_from(&mut args, &env_with(&[("ROBA_WRITABLE", val)]));
            assert!(args.writable, "env value {val:?} should enable writable");
        }
    }

    #[test]
    fn writable_ignores_falsy_or_garbage() {
        for val in ["0", "false", "no", "off", "", "garbage"] {
            let mut args = empty_args();
            apply_env_overrides_from(&mut args, &env_with(&[("ROBA_WRITABLE", val)]));
            assert!(
                !args.writable,
                "env value {val:?} should leave writable off"
            );
        }
    }

    #[test]
    fn bool_does_not_override_cli_value() {
        let mut args = empty_args();
        args.writable = true;
        apply_env_overrides_from(&mut args, &env_with(&[("ROBA_WRITABLE", "0")]));
        assert!(args.writable, "CLI true should survive env false");
    }

    #[test]
    fn env_writable_suppressed_by_cli_readonly() {
        let mut args = empty_args();
        args.readonly = true;
        apply_env_overrides_from(&mut args, &env_with(&[("ROBA_WRITABLE", "1")]));
        assert!(
            !args.writable,
            "CLI --readonly should suppress lower-layer ROBA_WRITABLE"
        );
    }

    #[test]
    fn env_full_auto_suppressed_by_cli_readonly() {
        let mut args = empty_args();
        args.readonly = true;
        apply_env_overrides_from(&mut args, &env_with(&[("ROBA_FULL_AUTO", "1")]));
        assert!(
            !args.full_auto,
            "CLI --readonly should suppress lower-layer ROBA_FULL_AUTO"
        );
    }

    #[test]
    fn env_full_auto_suppressed_by_cli_writable() {
        let mut args = empty_args();
        args.writable = true;
        apply_env_overrides_from(&mut args, &env_with(&[("ROBA_FULL_AUTO", "1")]));
        assert!(
            !args.full_auto,
            "CLI --writable should suppress lower-layer ROBA_FULL_AUTO"
        );
    }

    #[test]
    fn env_no_retry_truthy_values_enable() {
        for val in ["1", "true", "yes", "on", "TRUE", "Yes"] {
            let mut args = empty_args();
            apply_env_overrides_from(&mut args, &env_with(&[("ROBA_NO_RETRY", val)]));
            assert!(args.no_retry, "env value {val:?} should enable no_retry");
        }
    }

    #[test]
    fn env_no_retry_ignores_falsy_or_garbage() {
        for val in ["0", "false", "no", "off", "", "garbage"] {
            let mut args = empty_args();
            apply_env_overrides_from(&mut args, &env_with(&[("ROBA_NO_RETRY", val)]));
            assert!(
                !args.no_retry,
                "env value {val:?} should leave no_retry off"
            );
        }
    }

    // -- sessions ----------------------------------------------------------

    #[test]
    fn continue_truthy_means_most_recent() {
        for val in ["1", "true", "yes", "on", "TRUE", "Yes"] {
            let mut args = empty_args();
            apply_env_overrides_from(&mut args, &env_with(&[("ROBA_CONTINUE", val)]));
            assert_eq!(
                args.continue_session,
                Some(None),
                "env value {val:?} should continue the most recent session"
            );
        }
    }

    #[test]
    fn continue_falsy_or_empty_stays_fresh() {
        for val in ["0", "false", "no", "off", ""] {
            let mut args = empty_args();
            apply_env_overrides_from(&mut args, &env_with(&[("ROBA_CONTINUE", val)]));
            assert_eq!(
                args.continue_session, None,
                "env value {val:?} should leave the session fresh"
            );
        }
    }

    #[test]
    fn continue_arbitrary_value_is_specific_id() {
        let mut args = empty_args();
        apply_env_overrides_from(&mut args, &env_with(&[("ROBA_CONTINUE", "abc12345")]));
        assert_eq!(args.continue_session, Some(Some("abc12345".to_string())));
    }

    #[test]
    fn continue_does_not_override_cli() {
        // CLI `-c=cli-id` must survive a truthy ROBA_CONTINUE.
        let mut args = empty_args();
        args.continue_session = Some(Some("cli-id".to_string()));
        apply_env_overrides_from(&mut args, &env_with(&[("ROBA_CONTINUE", "1")]));
        assert_eq!(args.continue_session, Some(Some("cli-id".to_string())));
    }

    // -- session -----------------------------------------------------------

    #[test]
    fn env_session_sets_from_roba_session_var() {
        let mut args = empty_args();
        apply_env_overrides_from(&mut args, &env_with(&[("ROBA_SESSION", "meta")]));
        assert_eq!(args.session.as_deref(), Some("meta"));
    }

    #[test]
    fn env_session_does_not_override_cli() {
        let mut args = empty_args();
        args.session = Some("cli-name".into());
        apply_env_overrides_from(&mut args, &env_with(&[("ROBA_SESSION", "env-name")]));
        assert_eq!(args.session.as_deref(), Some("cli-name"));
    }

    #[test]
    fn env_session_ignores_empty() {
        let mut args = empty_args();
        apply_env_overrides_from(&mut args, &env_with(&[("ROBA_SESSION", "")]));
        assert!(args.session.is_none());
    }

    // -- session_id --------------------------------------------------------

    #[test]
    fn env_session_id_sets_from_roba_session_id_var() {
        let mut args = empty_args();
        apply_env_overrides_from(
            &mut args,
            &env_with(&[("ROBA_SESSION_ID", "11111111-1111-4111-8111-111111111111")]),
        );
        assert_eq!(
            args.session_id.as_deref(),
            Some("11111111-1111-4111-8111-111111111111")
        );
    }

    #[test]
    fn env_session_id_does_not_override_cli() {
        let mut args = empty_args();
        args.session_id = Some("cli-uuid".into());
        apply_env_overrides_from(&mut args, &env_with(&[("ROBA_SESSION_ID", "env-uuid")]));
        assert_eq!(args.session_id.as_deref(), Some("cli-uuid"));
    }

    #[test]
    fn env_session_id_ignores_empty() {
        let mut args = empty_args();
        apply_env_overrides_from(&mut args, &env_with(&[("ROBA_SESSION_ID", "")]));
        assert!(args.session_id.is_none());
    }

    // -- session / session_id env conflict (#272) --------------------------

    #[test]
    fn env_both_session_vars_set_is_a_conflict() {
        let args = empty_args();
        let err = check_env_session_conflict(
            &args,
            &env_with(&[("ROBA_SESSION", "meta"), ("ROBA_SESSION_ID", "some-uuid")]),
        )
        .unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("ROBA_SESSION"), "got: {msg}");
        assert!(msg.contains("ROBA_SESSION_ID"), "got: {msg}");
    }

    #[test]
    fn env_session_alone_is_not_a_conflict() {
        let args = empty_args();
        assert!(check_env_session_conflict(&args, &env_with(&[("ROBA_SESSION", "meta")])).is_ok());
    }

    #[test]
    fn env_session_id_alone_is_not_a_conflict() {
        let args = empty_args();
        assert!(
            check_env_session_conflict(&args, &env_with(&[("ROBA_SESSION_ID", "some-uuid")]))
                .is_ok()
        );
    }

    #[test]
    fn env_session_conflict_suppressed_when_cli_set_session() {
        // CLI presence wins: a `--session` on the CLI means the env pair is
        // not in play, so both env vars set is not a conflict.
        let mut args = empty_args();
        args.session = Some("cli-name".into());
        assert!(
            check_env_session_conflict(
                &args,
                &env_with(&[("ROBA_SESSION", "meta"), ("ROBA_SESSION_ID", "some-uuid")]),
            )
            .is_ok()
        );
    }

    #[test]
    fn env_session_conflict_suppressed_when_cli_set_session_id() {
        let mut args = empty_args();
        args.session_id = Some("cli-uuid".into());
        assert!(
            check_env_session_conflict(
                &args,
                &env_with(&[("ROBA_SESSION", "meta"), ("ROBA_SESSION_ID", "some-uuid")]),
            )
            .is_ok()
        );
    }

    // -- json_schema -------------------------------------------------------

    #[test]
    fn env_json_schema_sets_from_roba_json_schema_var() {
        let mut args = empty_args();
        apply_env_overrides_from(
            &mut args,
            &env_with(&[("ROBA_JSON_SCHEMA", "/path/to/schema.json")]),
        );
        assert_eq!(args.json_schema.as_deref(), Some("/path/to/schema.json"));
    }

    #[test]
    fn env_json_schema_does_not_override_cli() {
        let mut args = empty_args();
        args.json_schema = Some("/cli/schema.json".into());
        apply_env_overrides_from(
            &mut args,
            &env_with(&[("ROBA_JSON_SCHEMA", "/env/schema.json")]),
        );
        assert_eq!(args.json_schema.as_deref(), Some("/cli/schema.json"));
    }

    #[test]
    fn env_json_schema_ignores_empty() {
        let mut args = empty_args();
        apply_env_overrides_from(&mut args, &env_with(&[("ROBA_JSON_SCHEMA", "")]));
        assert!(args.json_schema.is_none());
    }

    // -- usize -------------------------------------------------------------

    #[test]
    fn git_log_parses_number() {
        let mut args = empty_args();
        apply_env_overrides_from(&mut args, &env_with(&[("ROBA_GIT_LOG", "7")]));
        assert_eq!(args.git_log, Some(7));
    }

    #[test]
    fn git_log_ignores_invalid() {
        let mut args = empty_args();
        apply_env_overrides_from(&mut args, &env_with(&[("ROBA_GIT_LOG", "lots")]));
        assert_eq!(args.git_log, None);
    }

    // -- lists -------------------------------------------------------------

    #[test]
    fn allow_tool_comma_separated() {
        let mut args = empty_args();
        apply_env_overrides_from(
            &mut args,
            &env_with(&[("ROBA_ALLOW_TOOL", "Edit, Write , Bash(git status)")]),
        );
        assert_eq!(
            args.allow_tool,
            vec![
                "Edit".to_string(),
                "Write".to_string(),
                "Bash(git status)".to_string(),
            ]
        );
    }

    #[test]
    fn list_does_not_override_cli_list() {
        let mut args = empty_args();
        args.allow_tool = vec!["FromCli".into()];
        apply_env_overrides_from(&mut args, &env_with(&[("ROBA_ALLOW_TOOL", "FromEnv")]));
        assert_eq!(args.allow_tool, vec!["FromCli".to_string()]);
    }

    #[test]
    fn prepend_parses_paths() {
        let mut args = empty_args();
        apply_env_overrides_from(
            &mut args,
            &env_with(&[("ROBA_PREPEND", "/etc/preamble.md,~/style.md")]),
        );
        assert_eq!(
            args.prepend,
            vec![
                PathBuf::from("/etc/preamble.md"),
                PathBuf::from("~/style.md"),
            ]
        );
    }

    // -- vars --------------------------------------------------------------

    #[test]
    fn var_keys_picked_up_from_prefix() {
        let mut args = empty_args();
        apply_env_overrides_from(
            &mut args,
            &env_with(&[
                ("ROBA_VAR_TICKET", "ABC-123"),
                ("ROBA_VAR_NAME", "josh"),
                ("ROBA_PROFILE", "foo"), // not a var
            ]),
        );
        let map: HashMap<_, _> = args.var.iter().cloned().collect();
        assert_eq!(map.get("TICKET").map(String::as_str), Some("ABC-123"));
        assert_eq!(map.get("NAME").map(String::as_str), Some("josh"));
        assert!(!map.contains_key("PROFILE"));
    }

    #[test]
    fn var_does_not_override_cli_key() {
        let mut args = empty_args();
        args.var
            .push(("TICKET".to_string(), "from-cli".to_string()));
        apply_env_overrides_from(&mut args, &env_with(&[("ROBA_VAR_TICKET", "from-env")]));
        let map: HashMap<_, _> = args.var.iter().cloned().collect();
        assert_eq!(map.get("TICKET").map(String::as_str), Some("from-cli"));
    }

    // -- output policy flags -----------------------------------------------

    #[test]
    fn env_trace_sets_from_roba_trace_var() {
        let mut args = empty_args();
        apply_env_overrides_from(&mut args, &env_with(&[("ROBA_TRACE", "/tmp/run.jsonl")]));
        assert_eq!(
            args.trace.as_deref(),
            Some(std::path::Path::new("/tmp/run.jsonl"))
        );
    }

    #[test]
    fn env_trace_does_not_override_cli() {
        let mut args = empty_args();
        args.trace = Some(PathBuf::from("/cli/path.jsonl"));
        apply_env_overrides_from(&mut args, &env_with(&[("ROBA_TRACE", "/tmp/run.jsonl")]));
        assert_eq!(
            args.trace.as_deref(),
            Some(std::path::Path::new("/cli/path.jsonl"))
        );
    }

    #[test]
    fn env_trace_ignores_empty() {
        let mut args = empty_args();
        apply_env_overrides_from(&mut args, &env_with(&[("ROBA_TRACE", "")]));
        assert!(args.trace.is_none());
    }

    #[test]
    fn env_rates_file_sets_and_respects_cli() {
        let mut args = empty_args();
        apply_env_overrides_from(
            &mut args,
            &env_with(&[("ROBA_RATES_FILE", "/tmp/rates.toml")]),
        );
        assert_eq!(
            args.rates_file.as_deref(),
            Some(std::path::Path::new("/tmp/rates.toml"))
        );

        let mut args = empty_args();
        args.rates_file = Some(PathBuf::from("/cli/rates.toml"));
        apply_env_overrides_from(
            &mut args,
            &env_with(&[("ROBA_RATES_FILE", "/tmp/rates.toml")]),
        );
        assert_eq!(
            args.rates_file.as_deref(),
            Some(std::path::Path::new("/cli/rates.toml"))
        );
    }

    #[test]
    fn env_no_dollars_truthy_enables() {
        for val in ["1", "true", "yes", "on"] {
            let mut args = empty_args();
            apply_env_overrides_from(&mut args, &env_with(&[("ROBA_NO_DOLLARS", val)]));
            assert!(
                args.no_dollars,
                "env value {val:?} should enable no_dollars"
            );
        }
        let mut args = empty_args();
        apply_env_overrides_from(&mut args, &env_with(&[("ROBA_NO_DOLLARS", "0")]));
        assert!(!args.no_dollars);
    }

    #[test]
    fn output_flags_each_settable() {
        let mut args = empty_args();
        apply_env_overrides_from(
            &mut args,
            &env_with(&[
                ("ROBA_STREAM", "1"),
                ("ROBA_ECHO", "1"),
                ("ROBA_PLAIN", "1"),
                ("ROBA_QUIET", "1"),
                ("ROBA_JSON", "1"),
            ]),
        );
        assert!(args.stream);
        assert!(args.echo);
        assert!(args.plain);
        assert!(args.quiet);
        assert!(args.json);
    }

    // -- effort ---------------------------------------------------------------

    #[test]
    fn env_effort_sets_from_roba_effort_var() {
        let mut args = empty_args();
        apply_env_overrides_from(&mut args, &env_with(&[("ROBA_EFFORT", "high")]));
        assert_eq!(args.effort, Some(EffortLevel::High));
    }

    #[test]
    fn env_effort_does_not_override_cli() {
        let mut args = empty_args();
        args.effort = Some(EffortLevel::Max);
        apply_env_overrides_from(&mut args, &env_with(&[("ROBA_EFFORT", "low")]));
        assert_eq!(args.effort, Some(EffortLevel::Max));
    }

    #[test]
    fn env_effort_ignores_invalid_value() {
        let mut args = empty_args();
        apply_env_overrides_from(&mut args, &env_with(&[("ROBA_EFFORT", "ultra")]));
        assert!(args.effort.is_none());
    }

    #[test]
    fn env_effort_parses_all_variants() {
        for (s, expected) in [
            ("low", EffortLevel::Low),
            ("medium", EffortLevel::Medium),
            ("high", EffortLevel::High),
            ("xhigh", EffortLevel::Xhigh),
            ("max", EffortLevel::Max),
        ] {
            let mut args = empty_args();
            apply_env_overrides_from(&mut args, &env_with(&[("ROBA_EFFORT", s)]));
            assert_eq!(args.effort, Some(expected), "variant {s:?}");
        }
    }

    // -- permission_mode ---------------------------------------------------

    #[test]
    fn env_permission_mode_sets_from_roba_permission_mode_var() {
        let mut args = empty_args();
        apply_env_overrides_from(&mut args, &env_with(&[("ROBA_PERMISSION_MODE", "plan")]));
        assert!(args.permission_mode.is_some());
    }

    #[test]
    fn env_permission_mode_does_not_override_cli() {
        use crate::cli::PermMode;
        let mut args = empty_args();
        args.permission_mode = Some(PermMode::Auto);
        apply_env_overrides_from(&mut args, &env_with(&[("ROBA_PERMISSION_MODE", "plan")]));
        assert_eq!(args.permission_mode, Some(PermMode::Auto));
    }

    #[test]
    fn env_permission_mode_ignores_invalid_value() {
        let mut args = empty_args();
        apply_env_overrides_from(
            &mut args,
            &env_with(&[("ROBA_PERMISSION_MODE", "notamode")]),
        );
        assert!(args.permission_mode.is_none());
    }

    #[test]
    fn env_permission_mode_parses_known_variants() {
        use crate::cli::PermMode;
        for (s, expected) in [
            ("plan", PermMode::Plan),
            ("auto", PermMode::Auto),
            ("dontAsk", PermMode::DontAsk),
            ("acceptEdits", PermMode::AcceptEdits),
            ("default", PermMode::Default),
        ] {
            let mut args = empty_args();
            apply_env_overrides_from(&mut args, &env_with(&[("ROBA_PERMISSION_MODE", s)]));
            assert_eq!(args.permission_mode, Some(expected), "variant {s:?}");
        }
    }

    // -- bare --------------------------------------------------------------

    #[test]
    fn env_bare_sets_from_roba_bare_var() {
        let mut args = empty_args();
        apply_env_overrides_from(&mut args, &env_with(&[("ROBA_BARE", "1")]));
        assert!(args.bare);
    }

    #[test]
    fn env_bare_does_not_override_cli() {
        // `--bare` is a bool; once true, the env layer must not clear it
        let mut args = empty_args();
        args.bare = true;
        // env says "false-ish" -- truthy logic means the flag can only enable, not disable
        apply_env_overrides_from(&mut args, &env_with(&[("ROBA_BARE", "0")]));
        assert!(args.bare);
    }

    #[test]
    fn env_bare_ignores_false_value() {
        let mut args = empty_args();
        apply_env_overrides_from(&mut args, &env_with(&[("ROBA_BARE", "false")]));
        assert!(!args.bare);
    }

    // -- mcp ---------------------------------------------------------------

    #[test]
    fn env_mcp_config_comma_separated() {
        let mut args = empty_args();
        apply_env_overrides_from(
            &mut args,
            &env_with(&[("ROBA_MCP_CONFIG", "a.json, b.json , c.json")]),
        );
        assert_eq!(
            args.mcp_config,
            vec![
                "a.json".to_string(),
                "b.json".to_string(),
                "c.json".to_string(),
            ]
        );
    }

    #[test]
    fn env_mcp_config_does_not_override_cli() {
        let mut args = empty_args();
        args.mcp_config = vec!["cli.json".into()];
        apply_env_overrides_from(&mut args, &env_with(&[("ROBA_MCP_CONFIG", "env.json")]));
        assert_eq!(args.mcp_config, vec!["cli.json".to_string()]);
    }

    #[test]
    fn env_strict_mcp_config_truthy_enables() {
        for val in ["1", "true", "yes", "on", "TRUE", "Yes"] {
            let mut args = empty_args();
            apply_env_overrides_from(&mut args, &env_with(&[("ROBA_STRICT_MCP_CONFIG", val)]));
            assert!(
                args.strict_mcp_config,
                "env value {val:?} should enable strict_mcp_config"
            );
        }
    }

    #[test]
    fn env_strict_mcp_config_ignores_falsy_or_garbage() {
        for val in ["0", "false", "no", "off", "", "garbage"] {
            let mut args = empty_args();
            apply_env_overrides_from(&mut args, &env_with(&[("ROBA_STRICT_MCP_CONFIG", val)]));
            assert!(
                !args.strict_mcp_config,
                "env value {val:?} should leave strict_mcp_config off"
            );
        }
    }

    #[test]
    fn env_strict_mcp_config_does_not_clear_cli() {
        let mut args = empty_args();
        args.strict_mcp_config = true;
        apply_env_overrides_from(&mut args, &env_with(&[("ROBA_STRICT_MCP_CONFIG", "0")]));
        assert!(args.strict_mcp_config, "CLI true should survive env false");
    }

    // -- limits (max_turns / max_budget_usd) -------------------------------

    #[test]
    fn env_max_turns_sets_from_roba_var() {
        let mut args = empty_args();
        apply_env_overrides_from(&mut args, &env_with(&[("ROBA_MAX_TURNS", "7")]));
        assert_eq!(args.max_turns, Some(7));
    }

    #[test]
    fn env_max_turns_does_not_override_cli() {
        let mut args = empty_args();
        args.max_turns = Some(3);
        apply_env_overrides_from(&mut args, &env_with(&[("ROBA_MAX_TURNS", "7")]));
        assert_eq!(args.max_turns, Some(3));
    }

    #[test]
    fn env_max_turns_ignores_invalid() {
        let mut args = empty_args();
        apply_env_overrides_from(&mut args, &env_with(&[("ROBA_MAX_TURNS", "lots")]));
        assert_eq!(args.max_turns, None);
    }

    #[test]
    fn env_max_budget_usd_sets_from_roba_var() {
        let mut args = empty_args();
        apply_env_overrides_from(&mut args, &env_with(&[("ROBA_MAX_BUDGET_USD", "12.5")]));
        assert_eq!(args.max_budget_usd, Some(12.5));
    }

    #[test]
    fn env_max_budget_usd_does_not_override_cli() {
        let mut args = empty_args();
        args.max_budget_usd = Some(4.0);
        apply_env_overrides_from(&mut args, &env_with(&[("ROBA_MAX_BUDGET_USD", "12.5")]));
        assert_eq!(args.max_budget_usd, Some(4.0));
    }

    #[test]
    fn env_max_budget_usd_ignores_invalid() {
        let mut args = empty_args();
        apply_env_overrides_from(&mut args, &env_with(&[("ROBA_MAX_BUDGET_USD", "free")]));
        assert_eq!(args.max_budget_usd, None);
    }

    #[test]
    fn env_timeout_sets_from_roba_var() {
        let mut args = empty_args();
        apply_env_overrides_from(&mut args, &env_with(&[("ROBA_TIMEOUT", "300")]));
        assert_eq!(args.timeout, Some(300));
    }

    #[test]
    fn env_timeout_does_not_override_cli() {
        let mut args = empty_args();
        args.timeout = Some(30);
        apply_env_overrides_from(&mut args, &env_with(&[("ROBA_TIMEOUT", "300")]));
        assert_eq!(args.timeout, Some(30));
    }

    #[test]
    fn env_timeout_ignores_invalid() {
        let mut args = empty_args();
        apply_env_overrides_from(&mut args, &env_with(&[("ROBA_TIMEOUT", "soon")]));
        assert_eq!(args.timeout, None);
    }

    // -- system_prompt / append_system_prompt ------------------------------

    #[test]
    fn env_system_prompt_sets_from_roba_system_prompt_var() {
        let mut args = empty_args();
        apply_env_overrides_from(
            &mut args,
            &env_with(&[("ROBA_SYSTEM_PROMPT", "You are helpful.")]),
        );
        assert_eq!(args.system_prompt.as_deref(), Some("You are helpful."));
    }

    #[test]
    fn env_system_prompt_does_not_override_cli() {
        let mut args = empty_args();
        args.system_prompt = Some("cli-prompt".to_string());
        apply_env_overrides_from(
            &mut args,
            &env_with(&[("ROBA_SYSTEM_PROMPT", "env-prompt")]),
        );
        assert_eq!(args.system_prompt.as_deref(), Some("cli-prompt"));
    }

    #[test]
    fn env_append_system_prompt_sets_from_roba_var() {
        let mut args = empty_args();
        apply_env_overrides_from(
            &mut args,
            &env_with(&[("ROBA_APPEND_SYSTEM_PROMPT", "Be concise.")]),
        );
        assert_eq!(args.append_system_prompt.as_deref(), Some("Be concise."));
    }

    #[test]
    fn env_append_system_prompt_does_not_override_cli() {
        let mut args = empty_args();
        args.append_system_prompt = Some("cli-append".to_string());
        apply_env_overrides_from(
            &mut args,
            &env_with(&[("ROBA_APPEND_SYSTEM_PROMPT", "env-append")]),
        );
        assert_eq!(args.append_system_prompt.as_deref(), Some("cli-append"));
    }

    // -- med-tier pass-throughs (add_dir / fallback_model / no_session_persistence) --

    #[test]
    fn env_add_dir_comma_separated() {
        let mut args = empty_args();
        apply_env_overrides_from(
            &mut args,
            &env_with(&[("ROBA_ADD_DIR", "/extra/a, /extra/b , /extra/c")]),
        );
        assert_eq!(
            args.add_dir,
            vec![
                "/extra/a".to_string(),
                "/extra/b".to_string(),
                "/extra/c".to_string(),
            ]
        );
    }

    #[test]
    fn env_add_dir_does_not_override_cli() {
        let mut args = empty_args();
        args.add_dir = vec!["/cli/dir".into()];
        apply_env_overrides_from(&mut args, &env_with(&[("ROBA_ADD_DIR", "/env/dir")]));
        assert_eq!(args.add_dir, vec!["/cli/dir".to_string()]);
    }

    #[test]
    fn env_fallback_model_sets_from_roba_var() {
        let mut args = empty_args();
        apply_env_overrides_from(&mut args, &env_with(&[("ROBA_FALLBACK_MODEL", "haiku")]));
        assert_eq!(args.fallback_model.as_deref(), Some("haiku"));
    }

    #[test]
    fn env_fallback_model_does_not_override_cli() {
        let mut args = empty_args();
        args.fallback_model = Some("opus".into());
        apply_env_overrides_from(&mut args, &env_with(&[("ROBA_FALLBACK_MODEL", "haiku")]));
        assert_eq!(args.fallback_model.as_deref(), Some("opus"));
    }

    #[test]
    fn env_fallback_model_ignores_empty() {
        let mut args = empty_args();
        apply_env_overrides_from(&mut args, &env_with(&[("ROBA_FALLBACK_MODEL", "")]));
        assert!(args.fallback_model.is_none());
    }

    #[test]
    fn env_no_session_persistence_truthy_enables() {
        for val in ["1", "true", "yes", "on", "TRUE", "Yes"] {
            let mut args = empty_args();
            apply_env_overrides_from(
                &mut args,
                &env_with(&[("ROBA_NO_SESSION_PERSISTENCE", val)]),
            );
            assert!(
                args.no_session_persistence,
                "env value {val:?} should enable no_session_persistence"
            );
        }
    }

    #[test]
    fn env_no_session_persistence_ignores_falsy_or_garbage() {
        for val in ["0", "false", "no", "off", "", "garbage"] {
            let mut args = empty_args();
            apply_env_overrides_from(
                &mut args,
                &env_with(&[("ROBA_NO_SESSION_PERSISTENCE", val)]),
            );
            assert!(
                !args.no_session_persistence,
                "env value {val:?} should leave no_session_persistence off"
            );
        }
    }

    #[test]
    fn env_no_session_persistence_does_not_clear_cli() {
        let mut args = empty_args();
        args.no_session_persistence = true;
        apply_env_overrides_from(
            &mut args,
            &env_with(&[("ROBA_NO_SESSION_PERSISTENCE", "0")]),
        );
        assert!(
            args.no_session_persistence,
            "CLI true should survive env false"
        );
    }

    // -- no-worktree (ROBA_NO_WORKTREE) ------------------------------------

    #[test]
    fn env_no_worktree_truthy_enables() {
        for val in ["1", "true", "yes", "on", "TRUE", "Yes"] {
            let mut args = empty_args();
            apply_env_overrides_from(&mut args, &env_with(&[("ROBA_NO_WORKTREE", val)]));
            assert!(
                args.no_worktree,
                "env value {val:?} should enable no_worktree"
            );
        }
    }

    #[test]
    fn env_no_worktree_ignores_falsy_or_garbage() {
        for val in ["0", "false", "no", "off", "", "garbage"] {
            let mut args = empty_args();
            apply_env_overrides_from(&mut args, &env_with(&[("ROBA_NO_WORKTREE", val)]));
            assert!(
                !args.no_worktree,
                "env value {val:?} should leave no_worktree off"
            );
        }
    }

    #[test]
    fn env_no_worktree_does_not_clear_cli() {
        let mut args = empty_args();
        args.no_worktree = true;
        apply_env_overrides_from(&mut args, &env_with(&[("ROBA_NO_WORKTREE", "0")]));
        assert!(args.no_worktree, "CLI true should survive env false");
    }

    // -- agent notice (no_agent_notice / agent_notice) ---------------------

    #[test]
    fn env_no_agent_notice_truthy_enables() {
        for val in ["1", "true", "yes", "on", "TRUE", "Yes"] {
            let mut args = empty_args();
            apply_env_overrides_from(&mut args, &env_with(&[("ROBA_NO_AGENT_NOTICE", val)]));
            assert!(
                args.no_agent_notice,
                "env value {val:?} should enable no_agent_notice"
            );
        }
    }

    #[test]
    fn env_no_agent_notice_ignores_falsy_or_garbage() {
        for val in ["0", "false", "no", "off", "", "garbage"] {
            let mut args = empty_args();
            apply_env_overrides_from(&mut args, &env_with(&[("ROBA_NO_AGENT_NOTICE", val)]));
            assert!(
                !args.no_agent_notice,
                "env value {val:?} should leave no_agent_notice off"
            );
        }
    }

    #[test]
    fn env_no_agent_notice_does_not_clear_cli() {
        let mut args = empty_args();
        args.no_agent_notice = true;
        apply_env_overrides_from(&mut args, &env_with(&[("ROBA_NO_AGENT_NOTICE", "0")]));
        assert!(args.no_agent_notice, "CLI true should survive env false");
    }

    #[test]
    fn env_agent_notice_sets_from_roba_var() {
        let mut args = empty_args();
        apply_env_overrides_from(
            &mut args,
            &env_with(&[("ROBA_AGENT_NOTICE", "single-turn, careful")]),
        );
        assert_eq!(args.agent_notice.as_deref(), Some("single-turn, careful"));
    }

    #[test]
    fn env_agent_notice_does_not_override_cli() {
        let mut args = empty_args();
        args.agent_notice = Some("cli-notice".to_string());
        apply_env_overrides_from(&mut args, &env_with(&[("ROBA_AGENT_NOTICE", "env-notice")]));
        assert_eq!(args.agent_notice.as_deref(), Some("cli-notice"));
    }

    #[test]
    fn env_agent_notice_ignores_empty() {
        let mut args = empty_args();
        apply_env_overrides_from(&mut args, &env_with(&[("ROBA_AGENT_NOTICE", "")]));
        assert!(args.agent_notice.is_none());
    }
}
