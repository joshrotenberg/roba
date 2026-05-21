//! Named profiles loaded from `~/.config/cwr/profiles.toml`.
//!
//! A profile is a bundle of [`AskArgs`] defaults. CLI flags always
//! override profile values; the profile only fills in fields the
//! user didn't set on the command line.
//!
//! Schema example:
//!
//! ```toml
//! [profile.review]
//! readonly = true
//! git_diff = true
//! prepend = ["~/.config/cwr/prompts/review.md"]
//!
//! [profile.review.vars]
//! AUDIENCE = "senior engineer"
//! ```
//!
//! Lookup by `--profile NAME`; missing names error.

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

/// Default location: `$XDG_CONFIG_HOME/cwr/profiles.toml` falling
/// back to `~/.config/cwr/profiles.toml`.
pub fn default_config_path() -> Option<PathBuf> {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME")
        && !xdg.is_empty()
    {
        return Some(PathBuf::from(xdg).join("cwr").join("profiles.toml"));
    }
    home_dir().map(|h| h.join(".config").join("cwr").join("profiles.toml"))
}

/// Load one named profile by name from the default config path.
pub fn load_profile(name: &str) -> Result<Profile> {
    let path = default_config_path()
        .ok_or_else(|| anyhow::anyhow!("could not determine config directory"))?;
    load_profile_from(&path, name)
}

/// Lower-level: load from an explicit path. Used by tests and by
/// a future `--profiles-file` override.
pub fn load_profile_from(path: &Path, name: &str) -> Result<Profile> {
    if !path.exists() {
        bail!("no profiles config at {}", path.display());
    }
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("reading profiles config at {}", path.display()))?;
    let config: ProfilesConfig = toml::from_str(&content)
        .with_context(|| format!("parsing profiles config at {}", path.display()))?;
    config
        .profile
        .get(name)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("no profile named `{name}` in {}", path.display()))
}

/// Load the entire config (all profiles). Used by `cwr profile list`.
/// A missing file is treated as an empty config.
pub fn load_config() -> Result<(PathBuf, ProfilesConfig)> {
    let path = default_config_path()
        .ok_or_else(|| anyhow::anyhow!("could not determine config directory"))?;
    let config = if path.exists() {
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("reading profiles config at {}", path.display()))?;
        toml::from_str(&content)
            .with_context(|| format!("parsing profiles config at {}", path.display()))?
    } else {
        ProfilesConfig::default()
    };
    Ok((path, config))
}

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
    }
}

fn run_list() -> Result<()> {
    let (path, config) = load_config()?;
    if config.profile.is_empty() {
        eprintln!("no profiles defined in {}", path.display());
        eprintln!("hint: `cwr profile init` to drop a starter file");
        return Ok(());
    }
    let mut names: Vec<&String> = config.profile.keys().collect();
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
    let path = default_config_path()
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

fn run_path() -> Result<()> {
    let path = default_config_path()
        .ok_or_else(|| anyhow::anyhow!("could not determine config directory"))?;
    println!("{}", path.display());
    Ok(())
}

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
    for (k, v) in profile.vars {
        if !args.var.iter().any(|(ak, _)| ak == &k) {
            args.var.push((k, v));
        }
    }
}

/// Expand a leading `~/` in a path. Other home-relative forms
/// (`~user`) are not supported -- the user's own `~` is enough for
/// the common cases.
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
    fn load_profile_missing_name_errors_with_path() {
        let file = write_tmp("[profile.x]\nreadonly = true\n");
        let err = load_profile_from(file.path(), "nope").unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("no profile named `nope`"));
        assert!(msg.contains(file.path().to_str().unwrap()));
    }

    #[test]
    fn load_profile_missing_file_errors() {
        let err = load_profile_from(Path::new("/no/such/profiles.toml"), "x").unwrap_err();
        assert!(format!("{err:#}").contains("no profiles config"));
    }

    fn empty_args() -> AskArgs {
        // we can't easily construct AskArgs directly via clap; use a
        // minimal CLI parse to get defaults.
        use clap::Parser;

        let cli = crate::cli::Cli::try_parse_from(["cwr", "placeholder"]).unwrap();
        cli.ask
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
        use clap::Parser;

        let cli =
            crate::cli::Cli::try_parse_from(["cwr", "placeholder", "--git-log", "7"]).unwrap();
        let mut args = cli.ask;

        let profile = Profile {
            git_log: Some(3),
            ..Default::default()
        };

        merge_into_args(&mut args, profile);

        // CLI value wins
        assert_eq!(args.git_log, Some(7));
    }

    #[test]
    fn merge_vars_skip_keys_already_on_cli() {
        use clap::Parser;

        let cli = crate::cli::Cli::try_parse_from([
            "cwr",
            "placeholder",
            "--var",
            "NAME=cli-josh",
        ])
        .unwrap();
        let mut args = cli.ask;

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
    fn expand_path_handles_tilde() {
        // Set a known HOME so the test is hermetic.
        // SAFETY: tests run single-threaded for env vars OR we accept
        // they share -- nothing else in this suite touches HOME.
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
