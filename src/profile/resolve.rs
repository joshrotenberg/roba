//! Resolution and merging: pick the effective [`Profile`] for an
//! invocation, then layer it onto the user's [`AskArgs`] without
//! clobbering anything the CLI set explicitly.

use anyhow::Result;
use std::path::PathBuf;

use super::home_dir;
use super::types::{ContinueSetting, PermissionModeConfig, Pool, Profile, WorktreeSetting};
use crate::cli::{AskArgs, PermMode};

// ---------------------------------------------------------------------------
// Resolution: which profile should this invocation apply?
// ---------------------------------------------------------------------------

/// Pick the effective profile for this run: the merged top-level
/// defaults, optionally overlaid with a selected named profile.
///
/// Returns `None` only when there's nothing to apply (no defaults, no
/// selected profile). Otherwise the returned profile is what
/// [`merge_into_args`] should be called with.
///
/// Selection precedence:
///
/// 1. `--profile NAME` -> that profile (error if missing)
/// 2. `--no-default-profile` -> no overlay (defaults still apply)
/// 3. `ROBA_PROFILE=NAME` env -> that profile (error if missing)
/// 4. `default` profile in pool -> that profile
/// 5. otherwise -> no overlay (defaults still apply)
pub fn resolve(args: &AskArgs, pool: &Pool) -> Result<Option<Profile>> {
    let mut effective = pool.defaults.clone();

    if let Some(name) = select_profile_name(args, pool) {
        let overlay = pool
            .get(&name)
            .cloned()
            .ok_or_else(|| missing_profile_error(&name, pool))?;
        effective.merge_in(overlay);
    }

    if effective.is_empty() {
        Ok(None)
    } else {
        Ok(Some(effective))
    }
}

/// The named profile this invocation would activate, if any. Mirrors
/// the selection precedence in [`resolve`]; `None` means "no named
/// overlay" (top-level defaults still apply). Used to label the
/// provenance of profile-contributed values for `--show-permissions`.
pub fn select_profile_name(args: &AskArgs, pool: &Pool) -> Option<String> {
    if let Some(name) = &args.profile {
        Some(name.clone())
    } else if args.no_default_profile {
        None
    } else if let Ok(name) = std::env::var("ROBA_PROFILE")
        && !name.is_empty()
    {
        Some(name)
    } else if pool.profiles.contains_key("default") {
        Some("default".to_string())
    } else {
        None
    }
}

/// The provenance label for values the profile layer contributes:
/// `profile.<name>` when a named overlay is active, else `config`
/// (top-level `roba.toml` keys).
pub fn profile_source_label(args: &AskArgs, pool: &Pool) -> String {
    match select_profile_name(args, pool) {
        Some(name) => format!("profile.{name}"),
        None => "config".to_string(),
    }
}

pub(super) fn missing_profile_error(name: &str, pool: &Pool) -> anyhow::Error {
    let sources = if pool.sources.is_empty() {
        "(no config sources found)".to_string()
    } else {
        pool.sources
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    };
    anyhow::anyhow!("no profile named `{name}` in {sources}")
}

// ---------------------------------------------------------------------------
// Merging into AskArgs
// ---------------------------------------------------------------------------

/// Apply a profile's defaults to [`AskArgs`]. CLI values always
/// win; this only fills in fields the user didn't set. `source` is
/// the provenance label recorded for any permission value this layer
/// contributes (see [`profile_source_label`]).
pub fn merge_into_args(args: &mut AskArgs, mut profile: Profile, source: &str) {
    if args.prepend.is_empty() {
        args.prepend = std::mem::take(&mut profile.prepend)
            .into_iter()
            .map(expand_path)
            .collect();
    }
    if args.append.is_empty() {
        args.append = std::mem::take(&mut profile.append)
            .into_iter()
            .map(expand_path)
            .collect();
    }
    if args.attach.is_empty() {
        args.attach = std::mem::take(&mut profile.attach);
    }
    if let Some(v) = profile.git_diff
        && !args.git_diff
    {
        args.git_diff = v;
    }
    if args.git_log.is_none()
        && let Some(v) = profile.git_log
    {
        args.git_log = Some(v);
    }
    if let Some(v) = profile.git_status
        && !args.git_status
    {
        args.git_status = v;
    }
    if let Some(v) = profile.readonly
        && !args.readonly
    {
        args.readonly = v;
        if v {
            args.readonly_source = Some(source.to_string());
        }
    }
    if let Some(v) = profile.writable
        && !args.writable
        && !args.readonly
    {
        args.writable = v;
        if v {
            args.writable_source = Some(source.to_string());
        }
    }
    if let Some(v) = profile.full_auto
        && !args.full_auto
        && !args.writable
        && !args.readonly
    {
        args.full_auto = v;
        if v {
            args.full_auto_source = Some(source.to_string());
        }
    }
    // continue_session is filled only when the CLI / env layer left
    // it unset. A profile `continue = true` continues the most recent
    // session; `continue = "id"` resumes that specific session.
    if args.continue_session.is_none() {
        args.continue_session = match profile.continue_session {
            Some(ContinueSetting::MostRecent(true)) => Some(None),
            Some(ContinueSetting::Specific(id)) => Some(Some(id)),
            Some(ContinueSetting::MostRecent(false)) | None => None,
        };
    }
    if args.allow_tool.is_empty() {
        args.allow_tool = std::mem::take(&mut profile.allow_tool);
        args.allow_tool_sources = vec![source.to_string(); args.allow_tool.len()];
    }
    if args.deny_tool.is_empty() {
        args.deny_tool = std::mem::take(&mut profile.deny_tool);
        args.deny_tool_sources = vec![source.to_string(); args.deny_tool.len()];
    }
    for (k, v) in profile.vars {
        if !args.var.iter().any(|(ak, _)| ak == &k) {
            args.var.push((k, v));
        }
    }
    if args.model.is_none()
        && let Some(m) = profile.model.take()
    {
        args.model = Some(m);
    }
    if args.effort.is_none() && profile.effort.is_some() {
        args.effort = profile.effort;
    }
    if args.agent.is_none()
        && let Some(a) = profile.agent.take()
    {
        args.agent = Some(a);
    }
    if let Some(v) = profile.stream
        && !args.stream
    {
        args.stream = v;
    }
    if let Some(v) = profile.show_thinking
        && !args.show_thinking
    {
        args.show_thinking = v;
    }
    if let Some(v) = profile.echo
        && !args.echo
    {
        args.echo = v;
    }
    if let Some(v) = profile.plain
        && !args.plain
    {
        args.plain = v;
    }
    if let Some(v) = profile.quiet
        && !args.quiet
    {
        args.quiet = v;
    }
    if let Some(v) = profile.json
        && !args.json
    {
        args.json = v;
    }
    if args.editor_history.is_none() && profile.editor_history.is_some() {
        args.editor_history = profile.editor_history;
    }
    if args.worktree.is_none() {
        args.worktree = match profile.worktree {
            Some(WorktreeSetting::Enabled(true)) => Some(None),
            Some(WorktreeSetting::Named(s)) => Some(Some(s)),
            Some(WorktreeSetting::Enabled(false)) | None => None,
        };
    }
    if let Some(v) = profile.no_retry
        && !args.no_retry
    {
        args.no_retry = v;
    }
    if let Some(v) = profile.bare
        && !args.bare
    {
        args.bare = v;
    }
    if args.trace.is_none()
        && let Some(p) = profile.trace.take()
    {
        args.trace = Some(expand_path(p));
    }
    if args.rates_file.is_none()
        && let Some(p) = profile.rates_file.take()
    {
        args.rates_file = Some(expand_path(p));
    }
    if let Some(v) = profile.no_dollars
        && !args.no_dollars
    {
        args.no_dollars = v;
    }
    if let Some(v) = profile.no_agent_check
        && !args.no_agent_check
    {
        args.no_agent_check = v;
    }
    if args.permission_mode.is_none()
        && let Some(mode) = profile.permission_mode.take()
    {
        args.permission_mode = Some(permission_mode_config_to_setting(mode));
        args.permission_mode_source = Some(source.to_string());
    }
    if args.system_prompt.is_none()
        && let Some(s) = profile.system_prompt.take()
    {
        args.system_prompt = Some(s);
    }
    if args.append_system_prompt.is_none()
        && let Some(s) = profile.append_system_prompt.take()
    {
        args.append_system_prompt = Some(s);
    }
    if args.max_turns.is_none()
        && let Some(n) = profile.max_turns
    {
        args.max_turns = Some(n);
    }
    if args.max_budget_usd.is_none()
        && let Some(v) = profile.max_budget_usd
    {
        args.max_budget_usd = Some(v);
    }
    if args.json_schema.is_none()
        && let Some(s) = profile.json_schema.take()
    {
        args.json_schema = Some(s);
    }
    // mcp_config is a list: the CLI list (if any) replaces the profile's,
    // matching --allow-tool's per-layer replace semantics.
    if args.mcp_config.is_empty() {
        args.mcp_config = std::mem::take(&mut profile.mcp_config);
    }
    if let Some(v) = profile.strict_mcp_config
        && !args.strict_mcp_config
    {
        args.strict_mcp_config = v;
    }
    // add_dir is a list: the CLI list (if any) replaces the profile's,
    // matching mcp_config / --allow-tool's per-layer replace semantics.
    if args.add_dir.is_empty() {
        args.add_dir = std::mem::take(&mut profile.add_dir);
        args.add_dir_sources = vec![source.to_string(); args.add_dir.len()];
    }
    if args.fallback_model.is_none()
        && let Some(m) = profile.fallback_model.take()
    {
        args.fallback_model = Some(m);
    }
    if let Some(v) = profile.no_session_persistence
        && !args.no_session_persistence
    {
        args.no_session_persistence = v;
    }
    if let Some(v) = profile.no_agent_notice
        && !args.no_agent_notice
    {
        args.no_agent_notice = v;
    }
    if args.agent_notice.is_none()
        && let Some(s) = profile.agent_notice.take()
    {
        args.agent_notice = Some(s);
    }
}

/// Convert a profile-layer `PermissionModeConfig` to the CLI's
/// `PermMode` so both types stay independent of each other.
fn permission_mode_config_to_setting(mode: PermissionModeConfig) -> PermMode {
    match mode {
        PermissionModeConfig::Default => PermMode::Default,
        PermissionModeConfig::AcceptEdits => PermMode::AcceptEdits,
        PermissionModeConfig::DontAsk => PermMode::DontAsk,
        PermissionModeConfig::Plan => PermMode::Plan,
        PermissionModeConfig::Auto => PermMode::Auto,
        #[allow(deprecated)]
        PermissionModeConfig::BypassPermissions => PermMode::BypassPermissions,
    }
}

// ---------------------------------------------------------------------------
// Path expansion helpers
// ---------------------------------------------------------------------------

/// Expand a leading `~/` in a path.
fn expand_path(path: PathBuf) -> PathBuf {
    let Some(s) = path.to_str() else {
        return path;
    };
    let Some(rest) = s.strip_prefix("~/") else {
        return path;
    };
    match home_dir() {
        Some(home) => home.join(rest),
        None => path,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn empty_args() -> AskArgs {
        use clap::Parser;
        let cli = crate::cli::Cli::try_parse_from(["roba", "placeholder"]).unwrap();
        cli.ask
    }

    fn args_with(extra: &[&str]) -> AskArgs {
        use clap::Parser;
        let mut argv = vec!["roba", "placeholder"];
        argv.extend(extra);
        crate::cli::Cli::try_parse_from(&argv).unwrap().ask
    }

    fn pool_of(defaults: Profile, named: &[(&str, Profile)]) -> Pool {
        let mut profiles = HashMap::new();
        for (name, profile) in named {
            profiles.insert((*name).to_string(), profile.clone());
        }
        Pool {
            defaults,
            profiles,
            aliases: HashMap::new(),
            sessions: HashMap::new(),
            sources: vec![],
        }
    }

    // -- Resolve precedence ------------------------------------------------

    #[test]
    fn resolve_explicit_profile_overlays_defaults() {
        let defaults = Profile {
            readonly: Some(true),
            prepend: vec![PathBuf::from("/d.md")],
            ..Default::default()
        };
        let foo = Profile {
            git_diff: Some(true),
            prepend: vec![PathBuf::from("/foo.md")],
            ..Default::default()
        };
        let pool = pool_of(defaults, &[("foo", foo)]);
        let args = args_with(&["--profile", "foo"]);
        let resolved = resolve(&args, &pool).unwrap().unwrap();
        // defaults still apply
        assert_eq!(resolved.readonly, Some(true));
        // overlay merged in
        assert_eq!(resolved.git_diff, Some(true));
        // lists concat (defaults first, overlay second)
        assert_eq!(
            resolved.prepend,
            vec![PathBuf::from("/d.md"), PathBuf::from("/foo.md")]
        );
    }

    #[test]
    fn resolve_default_when_no_explicit() {
        let p_default = Profile {
            readonly: Some(true),
            ..Default::default()
        };
        let pool = pool_of(Profile::default(), &[("default", p_default)]);
        let args = empty_args();
        let resolved = resolve(&args, &pool).unwrap().unwrap();
        assert_eq!(resolved.readonly, Some(true));
    }

    #[test]
    fn resolve_no_default_profile_skips_overlay_but_keeps_defaults() {
        let defaults = Profile {
            readonly: Some(true),
            ..Default::default()
        };
        let p_default = Profile {
            full_auto: Some(true),
            ..Default::default()
        };
        let pool = pool_of(defaults, &[("default", p_default)]);
        let args = args_with(&["--no-default-profile"]);
        let resolved = resolve(&args, &pool).unwrap().unwrap();
        // top-level defaults still apply
        assert_eq!(resolved.readonly, Some(true));
        // [profile.default] overlay was skipped
        assert_eq!(resolved.full_auto, None);
    }

    #[test]
    fn resolve_profile_full_auto_overrides_top_level_readonly() {
        // The "safe default + named loaded gun" pattern: top-level readonly,
        // a [profile.worker] full_auto. Resolving with worker active must
        // escalate to full_auto -- the named gun wins, readonly is cleared.
        let defaults = Profile {
            readonly: Some(true),
            ..Default::default()
        };
        let worker = Profile {
            full_auto: Some(true),
            ..Default::default()
        };
        let pool = pool_of(defaults, &[("worker", worker)]);
        let args = args_with(&["--profile", "worker"]);
        let resolved = resolve(&args, &pool).unwrap().unwrap();
        assert_eq!(resolved.full_auto, Some(true));
        assert_eq!(resolved.readonly, None);
        assert_eq!(resolved.writable, None);
    }

    #[test]
    fn resolve_unknown_explicit_profile_errors() {
        let pool = Pool::default();
        let args = args_with(&["--profile", "nope"]);
        let err = resolve(&args, &pool).unwrap_err();
        assert!(format!("{err:#}").contains("no profile named `nope`"));
    }

    #[test]
    fn resolve_returns_none_when_pool_empty() {
        let pool = Pool::default();
        let args = empty_args();
        let resolved = resolve(&args, &pool).unwrap();
        assert!(resolved.is_none());
    }

    // -- Merge into AskArgs ------------------------------------------------

    #[test]
    fn merge_fills_unset_fields() {
        let mut args = empty_args();
        let profile = Profile {
            readonly: Some(true),
            git_log: Some(3),
            attach: vec!["src/**/*.rs".to_string()],
            ..Default::default()
        };
        merge_into_args(&mut args, profile, "profile.test");
        assert!(args.readonly);
        assert_eq!(args.git_log, Some(3));
        assert_eq!(args.attach, vec!["src/**/*.rs".to_string()]);
    }

    #[test]
    fn merge_does_not_override_cli_values() {
        let mut args = args_with(&["--git-log", "7"]);
        let profile = Profile {
            git_log: Some(3),
            ..Default::default()
        };
        merge_into_args(&mut args, profile, "profile.test");
        assert_eq!(args.git_log, Some(7));
    }

    #[test]
    fn merge_vars_skip_keys_already_on_cli() {
        let mut args = args_with(&["--var", "NAME=cli-josh"]);
        let mut vars = HashMap::new();
        vars.insert("NAME".to_string(), "profile-josh".to_string());
        vars.insert("TICKET".to_string(), "ABC-123".to_string());
        let profile = Profile {
            vars,
            ..Default::default()
        };
        merge_into_args(&mut args, profile, "profile.test");
        let map: HashMap<_, _> = args.var.iter().cloned().collect();
        assert_eq!(map.get("NAME"), Some(&"cli-josh".to_string()));
        assert_eq!(map.get("TICKET"), Some(&"ABC-123".to_string()));
    }

    #[test]
    fn merge_continue_session_most_recent_when_unset() {
        let mut args = empty_args();
        let profile = Profile {
            continue_session: Some(ContinueSetting::MostRecent(true)),
            ..Default::default()
        };
        merge_into_args(&mut args, profile, "profile.test");
        // `continue = true` -> continue most recent (`Some(None)`).
        assert_eq!(args.continue_session, Some(None));
    }

    #[test]
    fn merge_continue_session_specific_id_when_unset() {
        let mut args = empty_args();
        let profile = Profile {
            continue_session: Some(ContinueSetting::Specific("abc12345".to_string())),
            ..Default::default()
        };
        merge_into_args(&mut args, profile, "profile.test");
        // `continue = "id"` -> resume that id (`Some(Some(id))`).
        assert_eq!(args.continue_session, Some(Some("abc12345".to_string())));
    }

    #[test]
    fn merge_continue_session_false_stays_fresh() {
        let mut args = empty_args();
        let profile = Profile {
            continue_session: Some(ContinueSetting::MostRecent(false)),
            ..Default::default()
        };
        merge_into_args(&mut args, profile, "profile.test");
        assert_eq!(args.continue_session, None);
    }

    #[test]
    fn merge_continue_session_cli_wins_over_profile() {
        // CLI `-c=cli-id` must survive a profile `continue = true`:
        // the higher layer's explicit id is not clobbered.
        let mut args = args_with(&["-c=cli-id"]);
        assert_eq!(args.continue_session, Some(Some("cli-id".to_string())));
        let profile = Profile {
            continue_session: Some(ContinueSetting::MostRecent(true)),
            ..Default::default()
        };
        merge_into_args(&mut args, profile, "profile.test");
        assert_eq!(args.continue_session, Some(Some("cli-id".to_string())));
    }

    #[test]
    fn merge_allow_tool_from_profile_when_cli_empty() {
        let mut args = empty_args();
        let profile = Profile {
            allow_tool: vec!["Bash(git status)".to_string(), "Bash(git diff)".to_string()],
            deny_tool: vec!["WebFetch".to_string()],
            ..Default::default()
        };
        merge_into_args(&mut args, profile, "profile.test");
        assert_eq!(
            args.allow_tool,
            vec!["Bash(git status)".to_string(), "Bash(git diff)".to_string()]
        );
        assert_eq!(args.deny_tool, vec!["WebFetch".to_string()]);
    }

    /// Spec-as-test for the precedence docs in README.md ("Permissions
    /// precedence") and docs/profiles.md ("Permissions precedence").
    ///
    /// `--readonly` on the CLI actively suppresses a `writable = true`
    /// coming from a lower-priority profile layer: the trio
    /// `readonly` / `writable` / `full_auto` is mutually exclusive
    /// across layers, so a higher-priority `--readonly` blocks a
    /// lower-layer `writable` from landing. See issue #52.
    #[test]
    fn merge_cli_readonly_suppresses_profile_writable() {
        let mut args = args_with(&["--readonly"]);
        assert!(args.readonly, "CLI --readonly should set args.readonly");
        assert!(!args.writable, "CLI didn't pass --writable");

        let profile = Profile {
            writable: Some(true),
            ..Default::default()
        };
        merge_into_args(&mut args, profile, "profile.test");

        // CLI value preserved.
        assert!(args.readonly, "CLI --readonly stays set after merge");
        // Profile's writable=true is suppressed by the higher-layer
        // --readonly: the trio is mutually exclusive across layers.
        assert!(
            !args.writable,
            "profile writable=true is suppressed when CLI passed --readonly"
        );
    }

    #[test]
    fn merge_cli_readonly_suppresses_profile_full_auto() {
        let mut args = args_with(&["--readonly"]);
        assert!(args.readonly, "CLI --readonly should set args.readonly");

        let profile = Profile {
            full_auto: Some(true),
            ..Default::default()
        };
        merge_into_args(&mut args, profile, "profile.test");

        assert!(args.readonly, "CLI --readonly stays set after merge");
        assert!(
            !args.full_auto,
            "profile full_auto=true is suppressed when CLI passed --readonly"
        );
    }

    #[test]
    fn merge_cli_writable_suppresses_profile_full_auto() {
        let mut args = args_with(&["--writable"]);
        assert!(args.writable, "CLI --writable should set args.writable");

        let profile = Profile {
            full_auto: Some(true),
            ..Default::default()
        };
        merge_into_args(&mut args, profile, "profile.test");

        assert!(args.writable, "CLI --writable stays set after merge");
        assert!(
            !args.full_auto,
            "profile full_auto=true is suppressed when CLI passed --writable"
        );
    }

    #[test]
    fn merge_agent_applies_when_cli_unset() {
        let mut args = empty_args();
        assert!(args.agent.is_none());
        let profile = Profile {
            agent: Some("reviewer".to_string()),
            ..Default::default()
        };
        merge_into_args(&mut args, profile, "profile.test");
        assert_eq!(args.agent.as_deref(), Some("reviewer"));
    }

    #[test]
    fn merge_agent_cli_wins_over_profile() {
        let mut args = empty_args();
        args.agent = Some("planner".to_string());
        let profile = Profile {
            agent: Some("reviewer".to_string()),
            ..Default::default()
        };
        merge_into_args(&mut args, profile, "profile.test");
        assert_eq!(args.agent.as_deref(), Some("planner"));
    }

    #[test]
    fn merge_records_profile_provenance() {
        let mut args = empty_args();
        let profile = Profile {
            writable: Some(true),
            deny_tool: vec!["WebFetch".to_string()],
            ..Default::default()
        };
        merge_into_args(&mut args, profile, "profile.review");
        assert_eq!(args.writable_source.as_deref(), Some("profile.review"));
        assert_eq!(args.deny_tool_sources, vec!["profile.review".to_string()]);
    }

    #[test]
    fn select_profile_name_prefers_explicit_flag() {
        let pool = pool_of(Profile::default(), &[("review", Profile::default())]);
        let args = args_with(&["--profile", "review"]);
        assert_eq!(select_profile_name(&args, &pool).as_deref(), Some("review"));
        assert_eq!(profile_source_label(&args, &pool), "profile.review");
    }

    #[test]
    fn profile_source_label_is_config_when_no_named_profile() {
        let pool = Pool::default();
        let args = empty_args();
        assert!(select_profile_name(&args, &pool).is_none());
        assert_eq!(profile_source_label(&args, &pool), "config");
    }

    #[test]
    fn merge_no_retry_applies_when_cli_unset() {
        let mut args = empty_args();
        assert!(!args.no_retry);
        let profile = Profile {
            no_retry: Some(true),
            ..Default::default()
        };
        merge_into_args(&mut args, profile, "profile.test");
        assert!(args.no_retry, "profile no_retry=true should flow through");
    }

    #[test]
    fn merge_no_retry_profile_false_leaves_cli_off() {
        // Profile sets no_retry=false and CLI didn't pass it: stays off.
        let mut args = empty_args();
        let profile = Profile {
            no_retry: Some(false),
            ..Default::default()
        };
        merge_into_args(&mut args, profile, "profile.test");
        assert!(!args.no_retry);
    }

    #[test]
    fn merge_no_retry_cli_already_set_is_unaffected() {
        // The CLI override gate: a profile value never *clears* a flag
        // the user already set on the command line.
        let mut args = args_with(&["--no-retry"]);
        assert!(args.no_retry, "CLI --no-retry should set args.no_retry");
        let profile = Profile {
            no_retry: Some(false),
            ..Default::default()
        };
        merge_into_args(&mut args, profile, "profile.test");
        assert!(
            args.no_retry,
            "CLI --no-retry survives a profile no_retry=false"
        );
    }

    #[test]
    fn merge_trace_applies_when_cli_unset() {
        let mut args = empty_args();
        assert!(args.trace.is_none());
        let profile = Profile {
            trace: Some(PathBuf::from("/tmp/run.jsonl")),
            ..Default::default()
        };
        merge_into_args(&mut args, profile, "profile.test");
        assert_eq!(
            args.trace.as_deref(),
            Some(std::path::Path::new("/tmp/run.jsonl"))
        );
    }

    #[test]
    fn merge_trace_cli_wins_over_profile() {
        let mut args = args_with(&["--trace", "/cli/path.jsonl"]);
        let profile = Profile {
            trace: Some(PathBuf::from("/tmp/run.jsonl")),
            ..Default::default()
        };
        merge_into_args(&mut args, profile, "profile.test");
        assert_eq!(
            args.trace.as_deref(),
            Some(std::path::Path::new("/cli/path.jsonl"))
        );
    }

    #[test]
    fn merge_trace_expands_tilde() {
        unsafe {
            std::env::set_var("HOME", "/fake/home");
        }
        let mut args = empty_args();
        let profile = Profile {
            trace: Some(PathBuf::from("~/run.jsonl")),
            ..Default::default()
        };
        merge_into_args(&mut args, profile, "profile.test");
        assert_eq!(
            args.trace.as_deref(),
            Some(std::path::Path::new("/fake/home/run.jsonl"))
        );
    }

    #[test]
    fn merge_allow_tool_cli_replaces_profile() {
        let mut args = args_with(&["--allow-tool", "Edit"]);
        let profile = Profile {
            allow_tool: vec!["Bash(git status)".to_string()],
            ..Default::default()
        };
        merge_into_args(&mut args, profile, "profile.test");
        assert_eq!(args.allow_tool, vec!["Edit".to_string()]);
    }

    #[test]
    fn merge_rates_file_applies_and_cli_wins() {
        let mut args = empty_args();
        let profile = Profile {
            rates_file: Some(PathBuf::from("/tmp/rates.toml")),
            ..Default::default()
        };
        merge_into_args(&mut args, profile, "profile.test");
        assert_eq!(
            args.rates_file.as_deref(),
            Some(std::path::Path::new("/tmp/rates.toml"))
        );

        let mut args = args_with(&["--rates-file", "/cli/rates.toml"]);
        let profile = Profile {
            rates_file: Some(PathBuf::from("/tmp/rates.toml")),
            ..Default::default()
        };
        merge_into_args(&mut args, profile, "profile.test");
        assert_eq!(
            args.rates_file.as_deref(),
            Some(std::path::Path::new("/cli/rates.toml"))
        );
    }

    #[test]
    fn merge_no_dollars_applies_when_cli_unset() {
        let mut args = empty_args();
        let profile = Profile {
            no_dollars: Some(true),
            ..Default::default()
        };
        merge_into_args(&mut args, profile, "profile.test");
        assert!(args.no_dollars);
    }

    #[test]
    fn merge_effort_applies_when_cli_unset() {
        use crate::cli::EffortLevel;
        let mut args = empty_args();
        let profile = Profile {
            effort: Some(EffortLevel::High),
            ..Default::default()
        };
        merge_into_args(&mut args, profile, "profile.test");
        assert_eq!(args.effort, Some(EffortLevel::High));
    }

    #[test]
    fn merge_effort_cli_wins_over_profile() {
        use crate::cli::EffortLevel;
        let mut args = empty_args();
        args.effort = Some(EffortLevel::Low);
        let profile = Profile {
            effort: Some(EffortLevel::Max),
            ..Default::default()
        };
        merge_into_args(&mut args, profile, "profile.test");
        assert_eq!(args.effort, Some(EffortLevel::Low));
    }

    #[test]
    fn merge_bare_applies_when_cli_unset() {
        let mut args = empty_args();
        let profile = Profile {
            bare: Some(true),
            ..Default::default()
        };
        merge_into_args(&mut args, profile, "profile.test");
        assert!(args.bare);
    }

    #[test]
    fn merge_system_prompt_applies_when_cli_unset() {
        let mut args = empty_args();
        let profile = Profile {
            system_prompt: Some("You are a reviewer.".to_string()),
            ..Default::default()
        };
        merge_into_args(&mut args, profile, "profile.test");
        assert_eq!(args.system_prompt.as_deref(), Some("You are a reviewer."));
    }

    #[test]
    fn merge_system_prompt_cli_wins_over_profile() {
        let mut args = empty_args();
        args.system_prompt = Some("cli".to_string());
        let profile = Profile {
            system_prompt: Some("profile".to_string()),
            ..Default::default()
        };
        merge_into_args(&mut args, profile, "profile.test");
        assert_eq!(args.system_prompt.as_deref(), Some("cli"));
    }

    #[test]
    fn merge_permission_mode_applies_when_cli_unset() {
        use crate::cli::PermMode;
        use crate::profile::types::PermissionModeConfig;
        let mut args = empty_args();
        let profile = Profile {
            permission_mode: Some(PermissionModeConfig::Plan),
            ..Default::default()
        };
        merge_into_args(&mut args, profile, "profile.test");
        assert_eq!(args.permission_mode, Some(PermMode::Plan));
    }

    #[test]
    fn merge_permission_mode_cli_wins_over_profile() {
        use crate::cli::PermMode;
        use crate::profile::types::PermissionModeConfig;
        let mut args = empty_args();
        args.permission_mode = Some(PermMode::Auto);
        let profile = Profile {
            permission_mode: Some(PermissionModeConfig::Plan),
            ..Default::default()
        };
        merge_into_args(&mut args, profile, "profile.test");
        assert_eq!(args.permission_mode, Some(PermMode::Auto));
    }

    #[test]
    fn merge_json_schema_applies_when_cli_unset() {
        let mut args = empty_args();
        assert!(args.json_schema.is_none());
        let profile = Profile {
            json_schema: Some("/path/to/schema.json".to_string()),
            ..Default::default()
        };
        merge_into_args(&mut args, profile, "profile.test");
        assert_eq!(args.json_schema.as_deref(), Some("/path/to/schema.json"));
    }

    #[test]
    fn merge_json_schema_cli_wins_over_profile() {
        let mut args = empty_args();
        args.json_schema = Some("/cli/schema.json".to_string());
        let profile = Profile {
            json_schema: Some("/profile/schema.json".to_string()),
            ..Default::default()
        };
        merge_into_args(&mut args, profile, "profile.test");
        assert_eq!(args.json_schema.as_deref(), Some("/cli/schema.json"));
    }

    // -- MCP (mcp_config / strict_mcp_config) ------------------------------

    #[test]
    fn merge_mcp_config_applies_when_cli_empty() {
        let mut args = empty_args();
        let profile = Profile {
            mcp_config: vec!["a.json".to_string(), "b.json".to_string()],
            ..Default::default()
        };
        merge_into_args(&mut args, profile, "profile.test");
        assert_eq!(
            args.mcp_config,
            vec!["a.json".to_string(), "b.json".to_string()]
        );
    }

    #[test]
    fn merge_mcp_config_cli_replaces_profile() {
        let mut args = args_with(&["--mcp-config", "cli.json"]);
        let profile = Profile {
            mcp_config: vec!["profile.json".to_string()],
            ..Default::default()
        };
        merge_into_args(&mut args, profile, "profile.test");
        assert_eq!(args.mcp_config, vec!["cli.json".to_string()]);
    }

    #[test]
    fn merge_strict_mcp_config_applies_when_cli_unset() {
        let mut args = empty_args();
        let profile = Profile {
            strict_mcp_config: Some(true),
            ..Default::default()
        };
        merge_into_args(&mut args, profile, "profile.test");
        assert!(args.strict_mcp_config);
    }

    #[test]
    fn merge_strict_mcp_config_cli_already_set_is_unaffected() {
        let mut args = args_with(&["--strict-mcp-config"]);
        assert!(args.strict_mcp_config);
        let profile = Profile {
            strict_mcp_config: Some(false),
            ..Default::default()
        };
        merge_into_args(&mut args, profile, "profile.test");
        assert!(
            args.strict_mcp_config,
            "CLI --strict-mcp-config survives a profile strict_mcp_config=false"
        );
    }

    // -- med-tier pass-throughs (add_dir / fallback_model / no_session_persistence) --

    #[test]
    fn merge_add_dir_applies_when_cli_empty() {
        let mut args = empty_args();
        let profile = Profile {
            add_dir: vec!["/extra/a".to_string(), "/extra/b".to_string()],
            ..Default::default()
        };
        merge_into_args(&mut args, profile, "profile.test");
        assert_eq!(
            args.add_dir,
            vec!["/extra/a".to_string(), "/extra/b".to_string()]
        );
    }

    #[test]
    fn merge_add_dir_cli_replaces_profile() {
        let mut args = args_with(&["--add-dir", "/cli/dir"]);
        let profile = Profile {
            add_dir: vec!["/profile/dir".to_string()],
            ..Default::default()
        };
        merge_into_args(&mut args, profile, "profile.test");
        assert_eq!(args.add_dir, vec!["/cli/dir".to_string()]);
    }

    #[test]
    fn merge_fallback_model_applies_when_cli_unset() {
        let mut args = empty_args();
        let profile = Profile {
            fallback_model: Some("haiku".to_string()),
            ..Default::default()
        };
        merge_into_args(&mut args, profile, "profile.test");
        assert_eq!(args.fallback_model.as_deref(), Some("haiku"));
    }

    #[test]
    fn merge_fallback_model_cli_wins_over_profile() {
        let mut args = args_with(&["--fallback-model", "opus"]);
        let profile = Profile {
            fallback_model: Some("haiku".to_string()),
            ..Default::default()
        };
        merge_into_args(&mut args, profile, "profile.test");
        assert_eq!(args.fallback_model.as_deref(), Some("opus"));
    }

    #[test]
    fn merge_no_session_persistence_applies_when_cli_unset() {
        let mut args = empty_args();
        let profile = Profile {
            no_session_persistence: Some(true),
            ..Default::default()
        };
        merge_into_args(&mut args, profile, "profile.test");
        assert!(args.no_session_persistence);
    }

    #[test]
    fn merge_no_session_persistence_cli_already_set_is_unaffected() {
        let mut args = args_with(&["--no-session-persistence"]);
        assert!(args.no_session_persistence);
        let profile = Profile {
            no_session_persistence: Some(false),
            ..Default::default()
        };
        merge_into_args(&mut args, profile, "profile.test");
        assert!(
            args.no_session_persistence,
            "CLI --no-session-persistence survives a profile false"
        );
    }

    // -- Limits (max_turns / max_budget_usd) -------------------------------

    #[test]
    fn merge_max_turns_applies_when_cli_unset() {
        let mut args = empty_args();
        let profile = Profile {
            max_turns: Some(5),
            ..Default::default()
        };
        merge_into_args(&mut args, profile, "profile.test");
        assert_eq!(args.max_turns, Some(5));
    }

    #[test]
    fn merge_max_turns_cli_wins_over_profile() {
        let mut args = args_with(&["--max-turns", "3"]);
        let profile = Profile {
            max_turns: Some(5),
            ..Default::default()
        };
        merge_into_args(&mut args, profile, "profile.test");
        assert_eq!(args.max_turns, Some(3));
    }

    #[test]
    fn merge_max_budget_usd_applies_when_cli_unset() {
        let mut args = empty_args();
        let profile = Profile {
            max_budget_usd: Some(12.5),
            ..Default::default()
        };
        merge_into_args(&mut args, profile, "profile.test");
        assert_eq!(args.max_budget_usd, Some(12.5));
    }

    #[test]
    fn merge_max_budget_usd_cli_wins_over_profile() {
        let mut args = args_with(&["--max-budget-usd", "4.0"]);
        let profile = Profile {
            max_budget_usd: Some(12.5),
            ..Default::default()
        };
        merge_into_args(&mut args, profile, "profile.test");
        assert_eq!(args.max_budget_usd, Some(4.0));
    }

    // -- agent notice (no_agent_notice / agent_notice) ---------------------

    #[test]
    fn merge_no_agent_notice_applies_when_cli_unset() {
        let mut args = empty_args();
        let profile = Profile {
            no_agent_notice: Some(true),
            ..Default::default()
        };
        merge_into_args(&mut args, profile, "profile.test");
        assert!(args.no_agent_notice);
    }

    #[test]
    fn merge_no_agent_notice_cli_already_set_is_unaffected() {
        let mut args = args_with(&["--no-agent-notice"]);
        assert!(args.no_agent_notice);
        let profile = Profile {
            no_agent_notice: Some(false),
            ..Default::default()
        };
        merge_into_args(&mut args, profile, "profile.test");
        assert!(
            args.no_agent_notice,
            "CLI --no-agent-notice survives a profile false"
        );
    }

    #[test]
    fn merge_agent_notice_applies_when_cli_unset() {
        let mut args = empty_args();
        let profile = Profile {
            agent_notice: Some("custom".to_string()),
            ..Default::default()
        };
        merge_into_args(&mut args, profile, "profile.test");
        assert_eq!(args.agent_notice.as_deref(), Some("custom"));
    }

    #[test]
    fn merge_agent_notice_cli_wins_over_profile() {
        let mut args = args_with(&["--agent-notice", "cli"]);
        let profile = Profile {
            agent_notice: Some("profile".to_string()),
            ..Default::default()
        };
        merge_into_args(&mut args, profile, "profile.test");
        assert_eq!(args.agent_notice.as_deref(), Some("cli"));
    }

    // -- Path expansion ----------------------------------------------------

    #[test]
    fn expand_path_handles_tilde() {
        unsafe {
            std::env::set_var("HOME", "/fake/home");
        }
        let out = expand_path(PathBuf::from("~/.config/roba/prompt.md"));
        assert_eq!(out, PathBuf::from("/fake/home/.config/roba/prompt.md"));
    }

    #[test]
    fn expand_path_leaves_absolute_alone() {
        let out = expand_path(PathBuf::from("/absolute/path"));
        assert_eq!(out, PathBuf::from("/absolute/path"));
    }
}
