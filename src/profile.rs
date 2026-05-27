//! Config system for `roba`.
//!
//! A `roba.toml` file holds two kinds of content:
//!
//! - **Top-level keys** -- defaults applied to every call in this dir
//! - **`[profile.NAME]` tables** -- named overlays opted into with
//!   `--profile NAME` or `ROBA_PROFILE=NAME`
//!
//! # Resolution
//!
//! For any setting, the highest layer that defines it wins:
//!
//! 1. CLI flag
//! 2. `[profile.NAME]` overlay (when activated)
//! 3. Top-level keys in `roba.toml` files
//! 4. roba's built-in defaults
//! 5. claude's defaults
//!
//! (The `ROBA_<PARAM>` env-var override layer slots in between CLI
//! and the profile overlay; it lands in a follow-up PR.)
//!
//! # File discovery
//!
//! 1. **User-level:** `$XDG_CONFIG_HOME/roba.toml` or
//!    `~/.config/roba.toml`
//! 2. **Project chain:** every ancestor `roba.toml` walking up from
//!    cwd to the git root (or `~` if no git root); farther-from-cwd
//!    files are loaded first so closer files override on conflict
//!
//! `ROBA_PROFILES_FILE` (point-at-an-extra-file env var) was retired
//! in favour of the per-knob `ROBA_<PARAM>` override layer; see
//! [`crate::env`].
//!
//! # Merge semantics
//!
//! When the same key appears in multiple files (top-level or inside
//! a `[profile.NAME]` of the same name):
//!
//! - Scalars: closer-to-cwd file wins.
//! - Lists: concat. Closer-file items appended after farther ones.
//! - Maps (vars): per-key merge with closer winning on key conflicts.
//!
//! CLI flags then override the merged result via [`merge_into_args`].

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::cli::AskArgs;

/// A profile = a bundle of optional defaults. Used both for top-level
/// keys (the unnamed "defaults" baseline) and for named
/// `[profile.NAME]` overlays.
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
    pub writable: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub full_auto: Option<bool>,
    /// `-c` / `--continue`. TOML key is `continue` (a Rust keyword,
    /// so the struct field uses a non-keyword name).
    #[serde(rename = "continue", skip_serializing_if = "Option::is_none")]
    pub continue_session: Option<bool>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub allow_tool: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub deny_tool: Vec<String>,
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub vars: HashMap<String, String>,
    /// Override the claude model (alias or full id).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub echo: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plain: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quiet: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub json: Option<bool>,
}

impl Profile {
    /// True if every field is at its default (no overrides).
    pub fn is_empty(&self) -> bool {
        self.prepend.is_empty()
            && self.append.is_empty()
            && self.attach.is_empty()
            && self.git_diff.is_none()
            && self.git_log.is_none()
            && self.git_status.is_none()
            && self.readonly.is_none()
            && self.writable.is_none()
            && self.full_auto.is_none()
            && self.continue_session.is_none()
            && self.allow_tool.is_empty()
            && self.deny_tool.is_empty()
            && self.vars.is_empty()
            && self.model.is_none()
            && self.stream.is_none()
            && self.echo.is_none()
            && self.plain.is_none()
            && self.quiet.is_none()
            && self.json.is_none()
    }

    /// Merge `other` on top of `self`. Used to layer roba.toml files
    /// closer-to-cwd on top of farther-from-cwd ones.
    ///
    /// - `Option<T>`: when `other.field.is_some()`, it overrides
    /// - `Vec<T>`: concat (self's items first, then other's)
    /// - `HashMap` (vars): per-key merge; other wins on key conflict
    pub fn merge_in(&mut self, other: Profile) {
        let Profile {
            mut prepend,
            mut append,
            mut attach,
            git_diff,
            git_log,
            git_status,
            readonly,
            writable,
            full_auto,
            continue_session,
            mut allow_tool,
            mut deny_tool,
            vars,
            model,
            stream,
            echo,
            plain,
            quiet,
            json,
        } = other;

        self.prepend.append(&mut prepend);
        self.append.append(&mut append);
        self.attach.append(&mut attach);
        if git_diff.is_some() {
            self.git_diff = git_diff;
        }
        if git_log.is_some() {
            self.git_log = git_log;
        }
        if git_status.is_some() {
            self.git_status = git_status;
        }
        if readonly.is_some() {
            self.readonly = readonly;
        }
        if writable.is_some() {
            self.writable = writable;
        }
        if full_auto.is_some() {
            self.full_auto = full_auto;
        }
        if continue_session.is_some() {
            self.continue_session = continue_session;
        }
        self.allow_tool.append(&mut allow_tool);
        self.deny_tool.append(&mut deny_tool);
        for (k, v) in vars {
            self.vars.insert(k, v);
        }
        if model.is_some() {
            self.model = model;
        }
        if stream.is_some() {
            self.stream = stream;
        }
        if echo.is_some() {
            self.echo = echo;
        }
        if plain.is_some() {
            self.plain = plain;
        }
        if quiet.is_some() {
            self.quiet = quiet;
        }
        if json.is_some() {
            self.json = json;
        }
    }
}

/// One loaded `roba.toml` file: top-level keys parsed as the
/// unnamed defaults profile, plus the map of `[profile.NAME]`
/// overlays.
#[derive(Debug, Default, Clone)]
pub struct ConfigFile {
    pub defaults: Profile,
    pub profile: HashMap<String, Profile>,
}

/// Resolved view across every config source for one roba invocation.
#[derive(Debug, Default, Clone)]
pub struct Pool {
    /// Merged top-level defaults across all loaded files.
    pub defaults: Profile,
    /// Merged named profiles. When the same name appears in multiple
    /// files, fields are merged per [`Profile::merge_in`].
    pub profiles: HashMap<String, Profile>,
    /// Source files that contributed, in load order. Used by
    /// `roba profile path` for diagnostics.
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

/// User-level config path: `$XDG_CONFIG_HOME/roba.toml` or
/// `~/.config/roba.toml`.
pub fn user_config_path() -> Option<PathBuf> {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME")
        && !xdg.is_empty()
    {
        return Some(PathBuf::from(xdg).join("roba.toml"));
    }
    home_dir().map(|h| h.join(".config").join("roba.toml"))
}

/// Walk up from `start` collecting every `roba.toml`. Stops at the
/// git root (a directory containing `.git`) if encountered, else at
/// the filesystem root.
///
/// Results are ordered **farther-first**: index 0 is the farthest-
/// from-cwd file (likely closest to the git root); the last entry is
/// the closest to cwd. This matches the load order used by the pool,
/// so closer-to-cwd files overlay farther-from-cwd ones.
pub fn discover_project_configs(start: &Path) -> Vec<PathBuf> {
    let mut hits: Vec<PathBuf> = Vec::new();
    let mut current = start.to_path_buf();
    loop {
        let candidate = current.join("roba.toml");
        if candidate.is_file() {
            hits.push(candidate);
        }
        let is_git_root = current.join(".git").exists();
        if is_git_root {
            break;
        }
        match current.parent() {
            Some(p) => current = p.to_path_buf(),
            None => break,
        }
    }
    // Collected closer-first; reverse so callers iterate
    // farther-first (lowest priority loaded earliest).
    hits.reverse();
    hits
}

// ---------------------------------------------------------------------------
// File loading
// ---------------------------------------------------------------------------

/// Parse a `roba.toml`. Splits top-level keys (the defaults profile)
/// from `[profile.NAME]` tables before deserializing each as a
/// [`Profile`] so `#[serde(deny_unknown_fields)]` catches typos in
/// either place.
fn load_file(path: &Path) -> Result<ConfigFile> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("reading config at {}", path.display()))?;
    let mut value: toml::Value = toml::from_str(&content)
        .with_context(|| format!("parsing config at {}", path.display()))?;

    let profile_map: HashMap<String, Profile> = if let toml::Value::Table(table) = &mut value {
        match table.remove("profile") {
            Some(v) => v
                .try_into()
                .with_context(|| format!("parsing [profile.*] tables in {}", path.display()))?,
            None => HashMap::new(),
        }
    } else {
        HashMap::new()
    };

    let defaults: Profile = value
        .try_into()
        .with_context(|| format!("parsing top-level keys in {}", path.display()))?;

    Ok(ConfigFile {
        defaults,
        profile: profile_map,
    })
}

/// Build the merged pool for the given cwd. Missing files are
/// silently treated as empty. Parse errors propagate.
///
/// Load order (lowest priority first):
///
/// 1. User-level config
/// 2. Project chain, farther-from-cwd first
pub fn load_pool_from(cwd: &Path) -> Result<Pool> {
    let mut pool = Pool::default();

    let mut layers: Vec<PathBuf> = Vec::new();
    if let Some(user) = user_config_path()
        && user.is_file()
    {
        layers.push(user);
    }
    layers.extend(discover_project_configs(cwd));

    for path in layers {
        let cfg = load_file(&path)?;
        pool.defaults.merge_in(cfg.defaults);
        for (name, profile) in cfg.profile {
            pool.profiles
                .entry(name)
                .or_insert_with(Profile::default)
                .merge_in(profile);
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

fn missing_profile_error(name: &str, pool: &Pool) -> anyhow::Error {
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
}

// ---------------------------------------------------------------------------
// Starter template + subcommand
// ---------------------------------------------------------------------------

/// Starter `roba.toml` content used by `roba profile init`. Kept
/// minimal -- the user is expected to edit and extend.
pub const STARTER_CONFIG_TOML: &str = include_str!("starter_roba.toml");

/// Run a `roba profile <action>` subcommand.
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
            eprintln!("hint: `roba profile init` to drop a starter file");
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
    let pool = load_pool()?;
    let profile = pool
        .get(name)
        .cloned()
        .ok_or_else(|| missing_profile_error(name, &pool))?;
    let rendered = render_named_profile(name, &profile)?;
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
    std::fs::write(&path, STARTER_CONFIG_TOML)
        .with_context(|| format!("writing {}", path.display()))?;
    println!("wrote {}", path.display());
    Ok(())
}

fn run_active() -> Result<()> {
    let pool = load_pool()?;
    let env_name = std::env::var("ROBA_PROFILE").ok().filter(|s| !s.is_empty());

    let (name, reason) = if let Some(name) = env_name {
        if pool.get(&name).is_none() {
            bail!("ROBA_PROFILE={name} but no such profile in the pool");
        }
        (name, "from ROBA_PROFILE env")
    } else if pool.get("default").is_some() {
        ("default".to_string(), "auto-applied")
    } else {
        eprintln!("no profile would auto-apply");
        if pool.profiles.is_empty() {
            eprintln!("hint: `roba profile init` to drop a starter file");
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
    let rendered = render_named_profile(&name, &profile)?;
    print!("{rendered}");
    Ok(())
}

fn run_path() -> Result<()> {
    let pool = load_pool()?;
    let user = user_config_path();
    println!(
        "user:    {}",
        user.as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "(none)".to_string())
    );
    let cwd = std::env::current_dir().unwrap_or_default();
    let project = discover_project_configs(&cwd);
    if project.is_empty() {
        println!("project: (none found above {})", cwd.display());
    } else {
        for (i, p) in project.iter().enumerate() {
            let label = if i == 0 { "project:" } else { "        " };
            println!("{label} {}", p.display());
        }
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

/// Render one named profile back to TOML for `profile show` / `active`.
fn render_named_profile(name: &str, profile: &Profile) -> Result<String> {
    let mut wrapper: HashMap<String, HashMap<String, Profile>> = HashMap::new();
    let mut inner: HashMap<String, Profile> = HashMap::new();
    inner.insert(name.to_string(), profile.clone());
    wrapper.insert("profile".to_string(), inner);
    toml::to_string_pretty(&wrapper).context("re-serializing profile")
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

    // -- Profile parsing ---------------------------------------------------

    #[test]
    fn parse_minimal_profile() {
        let toml = r#"
[profile.review]
readonly = true
git_diff = true
"#;
        let cfg = load_file_from_str(toml).unwrap();
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
        let cfg = load_file_from_str(toml).unwrap();
        let p = &cfg.profile["fancy"];
        assert_eq!(p.prepend.len(), 2);
        assert_eq!(p.attach, vec!["**/*.rs"]);
        assert_eq!(p.git_log, Some(5));
        assert_eq!(p.vars.get("NAME"), Some(&"Josh".to_string()));
    }

    #[test]
    fn parse_rejects_unknown_fields_in_profile() {
        let toml = r#"
[profile.bad]
typo_field = "oops"
"#;
        assert!(load_file_from_str(toml).is_err());
    }

    #[test]
    fn parse_rejects_unknown_top_level_keys() {
        let toml = r#"
prependz = ["/tmp/a"]
"#;
        assert!(load_file_from_str(toml).is_err());
    }

    #[test]
    fn parse_continue_field_uses_renamed_key() {
        let toml = r#"
[profile.persist]
continue = true
"#;
        let cfg = load_file_from_str(toml).unwrap();
        assert_eq!(cfg.profile["persist"].continue_session, Some(true));
    }

    #[test]
    fn parse_allow_tool_singular() {
        let toml = r#"
[profile.x]
allow_tool = ["Edit", "Write"]
deny_tool = ["WebFetch"]
"#;
        let cfg = load_file_from_str(toml).unwrap();
        let p = &cfg.profile["x"];
        assert_eq!(p.allow_tool, vec!["Edit".to_string(), "Write".to_string()]);
        assert_eq!(p.deny_tool, vec!["WebFetch".to_string()]);
    }

    #[test]
    fn parse_top_level_defaults() {
        let toml = r#"
readonly = true
attach = ["**/*.rs"]

[profile.review]
git_diff = true
"#;
        let cfg = load_file_from_str(toml).unwrap();
        assert_eq!(cfg.defaults.readonly, Some(true));
        assert_eq!(cfg.defaults.attach, vec!["**/*.rs"]);
        assert_eq!(cfg.profile["review"].git_diff, Some(true));
    }

    /// Test helper: parse a TOML string the way load_file would
    /// (splitting top-level vs [profile.*]).
    fn load_file_from_str(s: &str) -> Result<ConfigFile> {
        let mut value: toml::Value = toml::from_str(s)?;
        let profile_map: HashMap<String, Profile> = if let toml::Value::Table(t) = &mut value {
            match t.remove("profile") {
                Some(v) => v.try_into()?,
                None => HashMap::new(),
            }
        } else {
            HashMap::new()
        };
        let defaults: Profile = value.try_into()?;
        Ok(ConfigFile {
            defaults,
            profile: profile_map,
        })
    }

    // -- Profile merging ---------------------------------------------------

    #[test]
    fn merge_in_concats_lists() {
        let mut a = Profile {
            prepend: vec![PathBuf::from("/a")],
            allow_tool: vec!["Edit".into()],
            ..Default::default()
        };
        let b = Profile {
            prepend: vec![PathBuf::from("/b")],
            allow_tool: vec!["Write".into()],
            ..Default::default()
        };
        a.merge_in(b);
        assert_eq!(a.prepend, vec![PathBuf::from("/a"), PathBuf::from("/b")]);
        assert_eq!(a.allow_tool, vec!["Edit".to_string(), "Write".to_string()]);
    }

    #[test]
    fn merge_in_other_wins_on_scalars() {
        let mut a = Profile {
            readonly: Some(false),
            git_log: Some(3),
            ..Default::default()
        };
        let b = Profile {
            readonly: Some(true),
            git_log: None,
            git_diff: Some(true),
            ..Default::default()
        };
        a.merge_in(b);
        assert_eq!(a.readonly, Some(true)); // other overrode
        assert_eq!(a.git_log, Some(3)); // other was None, keep self
        assert_eq!(a.git_diff, Some(true)); // self was None, take other
    }

    #[test]
    fn merge_in_vars_other_wins_per_key() {
        let mut vars_a = HashMap::new();
        vars_a.insert("X".to_string(), "from_a".to_string());
        vars_a.insert("Y".to_string(), "from_a".to_string());
        let mut a = Profile {
            vars: vars_a,
            ..Default::default()
        };
        let mut vars_b = HashMap::new();
        vars_b.insert("X".to_string(), "from_b".to_string());
        vars_b.insert("Z".to_string(), "from_b".to_string());
        let b = Profile {
            vars: vars_b,
            ..Default::default()
        };
        a.merge_in(b);
        assert_eq!(a.vars["X"], "from_b"); // other won
        assert_eq!(a.vars["Y"], "from_a"); // kept from self
        assert_eq!(a.vars["Z"], "from_b"); // added by other
    }

    // -- Discovery walk ----------------------------------------------------

    #[test]
    fn discover_finds_roba_toml_in_starting_dir() {
        let tmp = tempfile::tempdir().unwrap();
        write_file(tmp.path(), "roba.toml", "");
        let found = discover_project_configs(tmp.path());
        assert_eq!(found, vec![tmp.path().join("roba.toml")]);
    }

    #[test]
    fn discover_walks_up_collecting_all_hits() {
        let tmp = tempfile::tempdir().unwrap();
        // git root at tmp/repo
        write_file(tmp.path(), "repo/.git/HEAD", "");
        write_file(tmp.path(), "repo/roba.toml", "");
        write_file(tmp.path(), "repo/sub/roba.toml", "");
        write_file(tmp.path(), "repo/sub/deeper/.gitkeep", "");
        let found = discover_project_configs(&tmp.path().join("repo/sub/deeper"));
        // farther-first, so repo/ before repo/sub/
        assert_eq!(
            found,
            vec![
                tmp.path().join("repo/roba.toml"),
                tmp.path().join("repo/sub/roba.toml"),
            ]
        );
    }

    #[test]
    fn discover_stops_at_git_root() {
        let tmp = tempfile::tempdir().unwrap();
        // roba.toml at the PARENT of the git root, should NOT be found
        write_file(tmp.path(), "roba.toml", "");
        write_file(tmp.path(), "repo/.git/HEAD", "");
        write_file(tmp.path(), "repo/sub/.gitkeep", "");
        let found = discover_project_configs(&tmp.path().join("repo/sub"));
        assert!(found.is_empty());
    }

    #[test]
    fn discover_finds_at_git_root_itself() {
        let tmp = tempfile::tempdir().unwrap();
        write_file(tmp.path(), "repo/.git/HEAD", "");
        write_file(tmp.path(), "repo/roba.toml", "");
        write_file(tmp.path(), "repo/sub/.gitkeep", "");
        let found = discover_project_configs(&tmp.path().join("repo/sub"));
        assert_eq!(found, vec![tmp.path().join("repo/roba.toml")]);
    }

    #[test]
    fn discover_returns_empty_when_no_file_anywhere() {
        let tmp = tempfile::tempdir().unwrap();
        write_file(tmp.path(), "a/b/c/.gitkeep", "");
        let found = discover_project_configs(&tmp.path().join("a/b/c"));
        assert!(found.is_empty());
    }

    // -- File load round-trip ----------------------------------------------

    #[test]
    fn load_file_splits_defaults_and_profiles() {
        let f = write_tmp(
            r#"
readonly = true

[profile.x]
git_diff = true
"#,
        );
        let cfg = load_file(f.path()).unwrap();
        assert_eq!(cfg.defaults.readonly, Some(true));
        assert_eq!(cfg.profile["x"].git_diff, Some(true));
    }

    #[test]
    fn load_file_errors_on_typo_top_level() {
        let f = write_tmp("readonlyz = true\n");
        let err = load_file(f.path()).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("parsing top-level keys"));
    }

    // -- Pool walk-up merge ------------------------------------------------

    #[test]
    fn pool_walkup_merges_top_level_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        write_file(tmp.path(), "repo/.git/HEAD", "");
        // farther file sets readonly + an allow_tool
        write_file(
            tmp.path(),
            "repo/roba.toml",
            r#"
readonly = true
allow_tool = ["Bash(git status)"]
"#,
        );
        // closer file overrides readonly and adds another allow_tool
        write_file(
            tmp.path(),
            "repo/sub/roba.toml",
            r#"
readonly = false
allow_tool = ["Bash(git diff)"]
"#,
        );
        write_file(tmp.path(), "repo/sub/inner/.gitkeep", "");
        let pool = load_pool_from(&tmp.path().join("repo/sub/inner")).unwrap();
        // Closer wins on scalar
        assert_eq!(pool.defaults.readonly, Some(false));
        // Lists concat, farther first
        assert_eq!(
            pool.defaults.allow_tool,
            vec!["Bash(git status)".to_string(), "Bash(git diff)".to_string()]
        );
    }

    #[test]
    fn pool_walkup_merges_named_profile_across_files() {
        let tmp = tempfile::tempdir().unwrap();
        write_file(tmp.path(), "repo/.git/HEAD", "");
        write_file(
            tmp.path(),
            "repo/roba.toml",
            r#"
[profile.review]
readonly = true
prepend = ["/farther.md"]
"#,
        );
        write_file(
            tmp.path(),
            "repo/sub/roba.toml",
            r#"
[profile.review]
git_diff = true
prepend = ["/closer.md"]
"#,
        );
        write_file(tmp.path(), "repo/sub/inner/.gitkeep", "");
        let pool = load_pool_from(&tmp.path().join("repo/sub/inner")).unwrap();
        let p = pool.get("review").unwrap();
        assert_eq!(p.readonly, Some(true));
        assert_eq!(p.git_diff, Some(true));
        assert_eq!(
            p.prepend,
            vec![PathBuf::from("/farther.md"), PathBuf::from("/closer.md")]
        );
    }

    // -- Resolve precedence ------------------------------------------------

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
