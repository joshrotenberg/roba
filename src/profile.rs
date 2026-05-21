//! Named profiles for `cwr`.
//!
//! A profile is a bundle of [`AskArgs`] defaults. CLI flags always
//! override profile values; the profile only fills in fields the
//! user didn't set on the command line.
//!
//! # Lookup pool
//!
//! Profiles are merged from these sources, later sources overriding
//! earlier ones on the same name:
//!
//! 1. **User-level:** `$XDG_CONFIG_HOME/cwr/profiles.toml` or
//!    `~/.config/cwr/profiles.toml`
//! 2. **Project-local:** the closest `.cwr/profiles.toml` walking up
//!    from cwd; stops at the git root if there is one, else the
//!    filesystem root
//! 3. **Env file:** `CWR_PROFILES_FILE=path` adds a file at the
//!    highest priority -- useful for ephemeral overrides
//!
//! # Auto-apply
//!
//! When the default ask path runs:
//!
//! 1. `--profile NAME` -> apply that, no auto-apply
//! 2. `--no-default-profile` -> skip env + default
//! 3. `CWR_PROFILE=NAME` env -> apply that
//! 4. `default` profile present in the pool -> apply silently
//! 5. otherwise -> no profile

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::cli::AskArgs;

/// One named profile. Each field is optional so users only specify
/// what they want to override.
#[derive(Debug, Default, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Profile {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub prepend: Vec<PathBuf>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub append: Vec<PathBuf>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub attach: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub git_diff: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub git_log: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub git_status: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub readonly: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub full_auto: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub continue_session: Option<bool>,
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub vars: HashMap<String, String>,
}

/// Top-level file shape: `[profile.NAME]` tables under a `profile`
/// key. Other top-level keys are rejected so typos surface fast.
#[derive(Debug, Default, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct ProfilesConfig {
    pub profile: HashMap<String, Profile>,
}

/// Resolved view of all profile sources for a single cwr invocation.
#[derive(Debug, Default, Clone)]
pub struct Pool {
    /// Merged profiles, later sources winning on the same name.
    pub profiles: HashMap<String, Profile>,
    /// Source files that contributed, in load order. Used for
    /// diagnostics and `cwr profile path`.
    pub sources: Vec<PathBuf>,
}

impl Pool {
    pub fn get(&self, name: &str) -> Option<&Profile> {
        self.profiles.get(name)
    }
}

// ---------------------------------------------------------------------------
// Path discovery
// ---------------------------------------------------------------------------

/// User-level config path: `$XDG_CONFIG_HOME/cwr/profiles.toml` or
/// `~/.config/cwr/profiles.toml`.
pub fn user_config_path() -> Option<PathBuf> {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME")
        && !xdg.is_empty()
    {
        return Some(PathBuf::from(xdg).join("cwr").join("profiles.toml"));
    }
    home_dir().map(|h| h.join(".config").join("cwr").join("profiles.toml"))
}

/// Walk up from `start` looking for `.cwr/profiles.toml`. Stops at
/// the git root (a directory containing `.git`) if encountered, else
/// at the filesystem root. Returns the first match, or `None`.
pub fn discover_project_config(start: &Path) -> Option<PathBuf> {
    let mut current = start.to_path_buf();
    loop {
        let candidate = current.join(".cwr").join("profiles.toml");
        if candidate.is_file() {
            return Some(candidate);
        }
        let is_git_root = current.join(".git").exists();
        if is_git_root {
            return None;
        }
        match current.parent() {
            Some(p) => current = p.to_path_buf(),
            None => return None,
        }
    }
}

/// Optional path from the `CWR_PROFILES_FILE` env var.
fn env_profiles_file() -> Option<PathBuf> {
    std::env::var("CWR_PROFILES_FILE")
        .ok()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
}

/// Back-compat alias for the old name. Prefer `user_config_path`.
pub fn default_config_path() -> Option<PathBuf> {
    user_config_path()
}

// ---------------------------------------------------------------------------
// File loading
// ---------------------------------------------------------------------------

fn load_file(path: &Path) -> Result<ProfilesConfig> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("reading profiles config at {}", path.display()))?;
    toml::from_str(&content)
        .with_context(|| format!("parsing profiles config at {}", path.display()))
}

/// Build the merged pool for the given cwd. Missing files are
/// silently treated as empty. Parse errors propagate.
pub fn load_pool_from(cwd: &Path) -> Result<Pool> {
    let mut pool = Pool::default();

    let mut layers: Vec<PathBuf> = Vec::new();
    if let Some(user) = user_config_path()
        && user.is_file()
    {
        layers.push(user);
    }
    if let Some(project) = discover_project_config(cwd) {
        layers.push(project);
    }
    if let Some(env_path) = env_profiles_file() {
        if !env_path.exists() {
            bail!(
                "CWR_PROFILES_FILE points to {} but the file doesn't exist",
                env_path.display()
            );
        }
        layers.push(env_path);
    }

    for path in layers {
        let cfg = load_file(&path)?;
        for (name, profile) in cfg.profile {
            pool.profiles.insert(name, profile);
        }
        pool.sources.push(path);
    }

    Ok(pool)
}

/// Convenience: load the pool keyed off the current cwd.
pub fn load_pool() -> Result<Pool> {
    let cwd = std::env::current_dir().context("getting current dir")?;
    load_pool_from(&cwd)
}

/// Look up one named profile across the merged pool. Errors with a
/// helpful path list if the name is missing.
pub fn load_profile(name: &str) -> Result<Profile> {
    let pool = load_pool()?;
    pool.get(name).cloned().ok_or_else(|| {
        let sources = if pool.sources.is_empty() {
            "(no profile sources found)".to_string()
        } else {
            pool.sources
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        };
        anyhow::anyhow!("no profile named `{name}` in {sources}")
    })
}

/// Lower-level: load from one explicit path. Kept for tests.
pub fn load_profile_from(path: &Path, name: &str) -> Result<Profile> {
    if !path.exists() {
        bail!("no profiles config at {}", path.display());
    }
    let config = load_file(path)?;
    config
        .profile
        .get(name)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("no profile named `{name}` in {}", path.display()))
}

// ---------------------------------------------------------------------------
// Resolution: which profile should this invocation apply?
// ---------------------------------------------------------------------------

/// Pick the profile to apply (if any), given parsed args + the
/// resolved pool. The full precedence model:
///
/// 1. `--profile NAME` -> that profile (error if missing)
/// 2. `--no-default-profile` -> None
/// 3. `CWR_PROFILE=NAME` env -> that profile (error if missing)
/// 4. `default` profile in pool -> that profile
/// 5. otherwise -> None
pub fn resolve(args: &AskArgs, pool: &Pool) -> Result<Option<Profile>> {
    if let Some(name) = &args.profile {
        return pool
            .get(name)
            .cloned()
            .map(Some)
            .ok_or_else(|| missing_profile_error(name, pool));
    }
    if args.no_default_profile {
        return Ok(None);
    }
    if let Ok(name) = std::env::var("CWR_PROFILE")
        && !name.is_empty()
    {
        return pool
            .get(&name)
            .cloned()
            .map(Some)
            .ok_or_else(|| missing_profile_error(&name, pool));
    }
    Ok(pool.get("default").cloned())
}

fn missing_profile_error(name: &str, pool: &Pool) -> anyhow::Error {
    let sources = if pool.sources.is_empty() {
        "(no profile sources found)".to_string()
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
// Merging
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
    for (k, v) in profile.vars {
        if !args.var.iter().any(|(ak, _)| ak == &k) {
            args.var.push((k, v));
        }
    }
}

// ---------------------------------------------------------------------------
// Starter template + subcommand
// ---------------------------------------------------------------------------

/// Starter `profiles.toml` content used by `cwr profile init`. Kept
/// minimal -- the user is expected to edit and extend.
pub const STARTER_PROFILES_TOML: &str = include_str!("starter_profiles.toml");

/// Run a `cwr profile <action>` subcommand.
pub fn run(action: crate::cli::ProfileAction) -> Result<()> {
    use crate::cli::ProfileAction;
    match action {
        ProfileAction::List => run_list(),
        ProfileAction::Show { name } => run_show(&name),
        ProfileAction::Init { force } => run_init(force),
        ProfileAction::Path => run_path(),
        ProfileAction::Active => run_active(),
    }
}

fn run_list() -> Result<()> {
    let pool = load_pool()?;
    if pool.profiles.is_empty() {
        eprintln!("no profiles defined");
        if pool.sources.is_empty() {
            eprintln!("hint: `cwr profile init` to drop a starter file");
        } else {
            eprintln!("sources checked:");
            for s in &pool.sources {
                eprintln!("  {}", s.display());
            }
        }
        return Ok(());
    }
    let mut names: Vec<&String> = pool.profiles.keys().collect();
    names.sort();
    for name in names {
        println!("{name}");
    }
    Ok(())
}

fn run_show(name: &str) -> Result<()> {
    let profile = load_profile(name)?;
    let mut wrapper = HashMap::new();
    wrapper.insert(name.to_string(), profile);
    let config = ProfilesConfig { profile: wrapper };
    let rendered = toml::to_string_pretty(&config).context("re-serializing profile")?;
    print!("{rendered}");
    Ok(())
}

fn run_init(force: bool) -> Result<()> {
    let path = user_config_path()
        .ok_or_else(|| anyhow::anyhow!("could not determine config directory"))?;
    if path.exists() && !force {
        bail!(
            "{} already exists -- pass --force to overwrite",
            path.display()
        );
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    std::fs::write(&path, STARTER_PROFILES_TOML)
        .with_context(|| format!("writing {}", path.display()))?;
    println!("wrote {}", path.display());
    Ok(())
}

fn run_active() -> Result<()> {
    let pool = load_pool()?;
    let env_name = std::env::var("CWR_PROFILE").ok().filter(|s| !s.is_empty());

    let (name, reason) = if let Some(name) = env_name {
        if pool.get(&name).is_none() {
            bail!("CWR_PROFILE={name} but no such profile in the pool");
        }
        (name, "from CWR_PROFILE env")
    } else if pool.get("default").is_some() {
        ("default".to_string(), "auto-applied")
    } else {
        eprintln!("no profile would auto-apply");
        if pool.profiles.is_empty() {
            eprintln!("hint: `cwr profile init` to drop a starter file");
        } else {
            let mut names: Vec<&String> = pool.profiles.keys().collect();
            names.sort();
            eprintln!(
                "available: {}",
                names
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        return Ok(());
    };

    let profile = pool.get(&name).cloned().expect("checked above");
    println!("active: {name} ({reason})");
    println!();
    let mut wrapper = HashMap::new();
    wrapper.insert(name, profile);
    let cfg = ProfilesConfig { profile: wrapper };
    let rendered = toml::to_string_pretty(&cfg).context("re-serializing profile")?;
    print!("{rendered}");
    Ok(())
}

fn run_path() -> Result<()> {
    let pool = load_pool()?;
    let user = user_config_path();
    println!(
        "user:    {}",
        user.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "(none)".to_string())
    );
    let cwd = std::env::current_dir().unwrap_or_default();
    match discover_project_config(&cwd) {
        Some(p) => println!("project: {}", p.display()),
        None => println!("project: (none found above {})", cwd.display()),
    }
    if let Some(env_path) = env_profiles_file() {
        println!("env:     {}", env_path.display());
    }
    if !pool.sources.is_empty() {
        println!();
        println!("loaded {} source(s):", pool.sources.len());
        for s in &pool.sources {
            println!("  {}", s.display());
        }
    }
    Ok(())
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

fn home_dir() -> Option<PathBuf> {
    if let Ok(h) = std::env::var("HOME")
        && !h.is_empty()
    {
        return Some(PathBuf::from(h));
    }
    if let Ok(h) = std::env::var("USERPROFILE")
        && !h.is_empty()
    {
        return Some(PathBuf::from(h));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_tmp(content: &str) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        write!(f, "{content}").unwrap();
        f.flush().unwrap();
        f
    }

    fn write_file(dir: &Path, rel: &str, content: &str) {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, content).unwrap();
    }

    #[test]
    fn parse_minimal_profile() {
        let toml = r#"
[profile.review]
readonly = true
git_diff = true
"#;
        let cfg: ProfilesConfig = toml::from_str(toml).unwrap();
        let p = &cfg.profile["review"];
        assert_eq!(p.readonly, Some(true));
        assert_eq!(p.git_diff, Some(true));
        assert!(p.attach.is_empty());
    }

    #[test]
    fn parse_profile_with_vars_and_lists() {
        let toml = r#"
[profile.fancy]
prepend = ["/tmp/a", "/tmp/b"]
attach = ["**/*.rs"]
git_log = 5

[profile.fancy.vars]
NAME = "Josh"
TICKET = "ABC-123"
"#;
        let cfg: ProfilesConfig = toml::from_str(toml).unwrap();
        let p = &cfg.profile["fancy"];
        assert_eq!(p.prepend.len(), 2);
        assert_eq!(p.attach, vec!["**/*.rs"]);
        assert_eq!(p.git_log, Some(5));
        assert_eq!(p.vars.get("NAME"), Some(&"Josh".to_string()));
    }

    #[test]
    fn parse_rejects_unknown_fields() {
        let toml = r#"
[profile.bad]
typo_field = "oops"
"#;
        assert!(toml::from_str::<ProfilesConfig>(toml).is_err());
    }

    #[test]
    fn parse_continue_session_field() {
        let toml = r#"
[profile.persist]
continue_session = true
"#;
        let cfg: ProfilesConfig = toml::from_str(toml).unwrap();
        assert_eq!(cfg.profile["persist"].continue_session, Some(true));
    }

    #[test]
    fn load_profile_from_finds_named_block() {
        let file = write_tmp(
            r#"
[profile.x]
readonly = true

[profile.y]
git_diff = true
"#,
        );
        let p = load_profile_from(file.path(), "y").unwrap();
        assert_eq!(p.git_diff, Some(true));
        assert_eq!(p.readonly, None);
    }

    #[test]
    fn load_profile_from_missing_name_errors_with_path() {
        let file = write_tmp("[profile.x]\nreadonly = true\n");
        let err = load_profile_from(file.path(), "nope").unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("no profile named `nope`"));
        assert!(msg.contains(file.path().to_str().unwrap()));
    }

    #[test]
    fn load_profile_from_missing_file_errors() {
        let err = load_profile_from(Path::new("/no/such/profiles.toml"), "x").unwrap_err();
        assert!(format!("{err:#}").contains("no profiles config"));
    }

    // -- discovery walk ----------------------------------------------------

    #[test]
    fn discover_finds_profiles_in_starting_dir() {
        let tmp = tempfile::tempdir().unwrap();
        write_file(tmp.path(), ".cwr/profiles.toml", "[profile.x]\n");
        let found = discover_project_config(tmp.path());
        assert_eq!(found, Some(tmp.path().join(".cwr/profiles.toml")));
    }

    #[test]
    fn discover_walks_up_until_match() {
        let tmp = tempfile::tempdir().unwrap();
        write_file(tmp.path(), ".cwr/profiles.toml", "[profile.x]\n");
        write_file(tmp.path(), "a/b/c/.gitkeep", "");
        let found = discover_project_config(&tmp.path().join("a/b/c"));
        assert_eq!(found, Some(tmp.path().join(".cwr/profiles.toml")));
    }

    #[test]
    fn discover_stops_at_git_root() {
        let tmp = tempfile::tempdir().unwrap();
        // .cwr at the PARENT of the git root, should NOT be found
        write_file(tmp.path(), ".cwr/profiles.toml", "[profile.x]\n");
        write_file(tmp.path(), "repo/.git/HEAD", "");
        write_file(tmp.path(), "repo/sub/.gitkeep", "");
        let found = discover_project_config(&tmp.path().join("repo/sub"));
        assert_eq!(found, None);
    }

    #[test]
    fn discover_finds_at_git_root_itself() {
        let tmp = tempfile::tempdir().unwrap();
        write_file(tmp.path(), "repo/.git/HEAD", "");
        write_file(tmp.path(), "repo/.cwr/profiles.toml", "[profile.x]\n");
        write_file(tmp.path(), "repo/sub/.gitkeep", "");
        let found = discover_project_config(&tmp.path().join("repo/sub"));
        assert_eq!(found, Some(tmp.path().join("repo/.cwr/profiles.toml")));
    }

    #[test]
    fn discover_returns_none_when_no_file_anywhere() {
        let tmp = tempfile::tempdir().unwrap();
        write_file(tmp.path(), "a/b/c/.gitkeep", "");
        let found = discover_project_config(&tmp.path().join("a/b/c"));
        assert_eq!(found, None);
    }

    // -- merge / resolve helpers -------------------------------------------

    fn empty_args() -> AskArgs {
        use clap::Parser;

        let cli = crate::cli::Cli::try_parse_from(["cwr", "placeholder"]).unwrap();
        cli.ask
    }

    fn args_with(extra: &[&str]) -> AskArgs {
        use clap::Parser;

        let mut argv = vec!["cwr", "placeholder"];
        argv.extend(extra);
        crate::cli::Cli::try_parse_from(&argv).unwrap().ask
    }

    fn pool_of(entries: &[(&str, Profile)]) -> Pool {
        let mut profiles = HashMap::new();
        for (name, profile) in entries {
            profiles.insert((*name).to_string(), profile.clone());
        }
        Pool {
            profiles,
            sources: vec![],
        }
    }

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
        // CLI --resume wins -- the conflict means -c stays off
        assert!(!args.continue_session);
    }

    // -- resolve precedence ------------------------------------------------

    #[test]
    fn resolve_explicit_profile_wins() {
        let p_foo = Profile {
            readonly: Some(true),
            ..Default::default()
        };
        let p_default = Profile {
            full_auto: Some(true),
            ..Default::default()
        };
        let pool = pool_of(&[("foo", p_foo), ("default", p_default)]);
        let args = args_with(&["--profile", "foo"]);
        let resolved = resolve(&args, &pool).unwrap().unwrap();
        assert_eq!(resolved.readonly, Some(true));
        assert_eq!(resolved.full_auto, None);
    }

    #[test]
    fn resolve_default_when_no_explicit() {
        let p_default = Profile {
            readonly: Some(true),
            ..Default::default()
        };
        let pool = pool_of(&[("default", p_default)]);
        let args = empty_args();
        let resolved = resolve(&args, &pool).unwrap().unwrap();
        assert_eq!(resolved.readonly, Some(true));
    }

    #[test]
    fn resolve_no_default_profile_skips_auto() {
        let p_default = Profile {
            readonly: Some(true),
            ..Default::default()
        };
        let pool = pool_of(&[("default", p_default)]);
        let args = args_with(&["--no-default-profile"]);
        let resolved = resolve(&args, &pool).unwrap();
        assert!(resolved.is_none());
    }

    #[test]
    fn resolve_unknown_explicit_profile_errors() {
        let pool = pool_of(&[]);
        let args = args_with(&["--profile", "nope"]);
        let err = resolve(&args, &pool).unwrap_err();
        assert!(format!("{err:#}").contains("no profile named `nope`"));
    }

    #[test]
    fn resolve_no_default_when_pool_has_no_default() {
        let p_foo = Profile {
            readonly: Some(true),
            ..Default::default()
        };
        let pool = pool_of(&[("foo", p_foo)]);
        let args = empty_args();
        let resolved = resolve(&args, &pool).unwrap();
        assert!(resolved.is_none());
    }

    // -- path expansion ----------------------------------------------------

    #[test]
    fn expand_path_handles_tilde() {
        unsafe {
            std::env::set_var("HOME", "/fake/home");
        }
        let out = expand_path(PathBuf::from("~/.config/cwr/prompt.md"));
        assert_eq!(out, PathBuf::from("/fake/home/.config/cwr/prompt.md"));
    }

    #[test]
    fn expand_path_leaves_absolute_alone() {
        let out = expand_path(PathBuf::from("/absolute/path"));
        assert_eq!(out, PathBuf::from("/absolute/path"));
    }
}
