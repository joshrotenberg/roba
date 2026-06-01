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

use crate::cli::AskArgs;
use std::collections::HashMap;
use std::path::PathBuf;

/// Apply env-var overrides to fields the user didn't set on the
/// command line. Reads from the process environment.
pub fn apply_env_overrides(args: &mut AskArgs) {
    let env: HashMap<String, String> = std::env::vars().collect();
    apply_env_overrides_from(args, &env);
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

    // ----- Agent -----
    if args.agent.is_none()
        && let Some(s) = read_string(env, "ROBA_AGENT")
    {
        args.agent = Some(s);
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

    // ----- Permissions -----
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

    // ----- Failure modes -----
    if !args.no_retry && read_truthy(env, "ROBA_NO_RETRY") {
        args.no_retry = true;
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
}
