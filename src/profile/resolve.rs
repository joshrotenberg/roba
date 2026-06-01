//! Resolution and merging: pick the effective [`Profile`] for an
//! invocation, then layer it onto the user's [`AskArgs`] without
//! clobbering anything the CLI set explicitly.

use anyhow::Result;
use std::path::PathBuf;

use super::home_dir;
use super::types::{Pool, Profile, WorktreeSetting};
use crate::cli::AskArgs;

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

    let chosen: Option<String> = if let Some(name) = &args.profile {
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
    };

    if let Some(name) = chosen {
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
/// win; this only fills in fields the user didn't set.
pub fn merge_into_args(args: &mut AskArgs, mut profile: Profile) {
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
    }
    if let Some(v) = profile.writable
        && !args.writable
    {
        args.writable = v;
    }
    if let Some(v) = profile.full_auto
        && !args.full_auto
    {
        args.full_auto = v;
    }
    // continue_session is silently skipped if the user passed
    // --resume; the two would conflict and explicit --resume wins.
    if let Some(v) = profile.continue_session
        && !args.continue_session
        && args.resume.is_none()
    {
        args.continue_session = v;
    }
    if args.allow_tool.is_empty() {
        args.allow_tool = std::mem::take(&mut profile.allow_tool);
    }
    if args.deny_tool.is_empty() {
        args.deny_tool = std::mem::take(&mut profile.deny_tool);
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
        merge_into_args(&mut args, profile);
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
        merge_into_args(&mut args, profile);
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
        merge_into_args(&mut args, profile);
        let map: HashMap<_, _> = args.var.iter().cloned().collect();
        assert_eq!(map.get("NAME"), Some(&"cli-josh".to_string()));
        assert_eq!(map.get("TICKET"), Some(&"ABC-123".to_string()));
    }

    #[test]
    fn merge_continue_session_applies_when_unset() {
        let mut args = empty_args();
        let profile = Profile {
            continue_session: Some(true),
            ..Default::default()
        };
        merge_into_args(&mut args, profile);
        assert!(args.continue_session);
    }

    #[test]
    fn merge_continue_session_skipped_when_resume_set() {
        let mut args = args_with(&["--resume", "abc123"]);
        let profile = Profile {
            continue_session: Some(true),
            ..Default::default()
        };
        merge_into_args(&mut args, profile);
        assert!(!args.continue_session);
    }

    #[test]
    fn merge_allow_tool_from_profile_when_cli_empty() {
        let mut args = empty_args();
        let profile = Profile {
            allow_tool: vec!["Bash(git status)".to_string(), "Bash(git diff)".to_string()],
            deny_tool: vec!["WebFetch".to_string()],
            ..Default::default()
        };
        merge_into_args(&mut args, profile);
        assert_eq!(
            args.allow_tool,
            vec!["Bash(git status)".to_string(), "Bash(git diff)".to_string()]
        );
        assert_eq!(args.deny_tool, vec!["WebFetch".to_string()]);
    }

    /// Spec-as-test for the precedence docs in README.md ("Permissions
    /// precedence") and docs/profiles.md ("Permissions precedence").
    ///
    /// `--readonly` on the CLI is the explicit name for the built-in
    /// default; it does NOT actively suppress a `writable = true`
    /// coming from a profile. Reason: [`apply_permissions`] inspects
    /// `args.writable` (and `args.full_auto`), not `args.readonly`,
    /// and [`merge_into_args`] gates the profile fill on
    /// `!args.<flag>` per-flag rather than cross-checking the trio
    /// for mutual exclusion. So profile `writable = true` lands on
    /// `args.writable` regardless of the CLI's `--readonly`.
    ///
    /// To get read-only behavior when a profile turns writable on,
    /// the documented workaround is `--no-default-profile`. See
    /// issue #52 for the proposed-but-unimplemented
    /// "CLI --readonly suppresses profile writable" semantics.
    #[test]
    fn merge_cli_readonly_does_not_suppress_profile_writable() {
        let mut args = args_with(&["--readonly"]);
        assert!(args.readonly, "CLI --readonly should set args.readonly");
        assert!(!args.writable, "CLI didn't pass --writable");

        let profile = Profile {
            writable: Some(true),
            ..Default::default()
        };
        merge_into_args(&mut args, profile);

        // CLI value preserved.
        assert!(args.readonly, "CLI --readonly stays set after merge");
        // Profile's writable=true still applied -- the gate is
        // `!args.writable`, which is independent of args.readonly.
        assert!(
            args.writable,
            "profile writable=true lands on args.writable even when CLI passed --readonly"
        );
    }

    #[test]
    fn merge_allow_tool_cli_replaces_profile() {
        let mut args = args_with(&["--allow-tool", "Edit"]);
        let profile = Profile {
            allow_tool: vec!["Bash(git status)".to_string()],
            ..Default::default()
        };
        merge_into_args(&mut args, profile);
        assert_eq!(args.allow_tool, vec!["Edit".to_string()]);
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
