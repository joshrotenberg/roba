//! File discovery and loading: walk up from cwd collecting every
//! `roba.toml`, parse each into a [`ConfigFile`], and merge them into
//! a single [`Pool`].

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::home_dir;
use super::types::{ConfigFile, Pool, Profile};
use crate::aliases::{Alias, is_builtin_subcommand};

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
    parse_config_str(&content).with_context(|| format!("parsing config at {}", path.display()))
}

/// Parse a `roba.toml`'s text into a [`ConfigFile`], the same way
/// `load_file` does but from a string rather than a path. Splits
/// top-level keys (the defaults profile) from `[profile.NAME]` /
/// `[alias.NAME]` / `[session]` tables before deserializing each, so
/// `#[serde(deny_unknown_fields)]` catches typos in any section.
///
/// This is the REAL file-level validator the pool loader uses, exposed
/// for `roba config init`, which drafts a whole-file config and validates
/// it through this exact path before printing/writing.
pub fn parse_config_str(content: &str) -> Result<ConfigFile> {
    let mut value: toml::Value = toml::from_str(content).context("parsing config TOML")?;

    let profile_map: HashMap<String, Profile> = if let toml::Value::Table(table) = &mut value {
        match table.remove("profile") {
            Some(v) => v.try_into().context("parsing [profile.*] tables")?,
            None => HashMap::new(),
        }
    } else {
        HashMap::new()
    };

    let alias_map: HashMap<String, Alias> = if let toml::Value::Table(table) = &mut value {
        match table.remove("alias") {
            Some(v) => v.try_into().context("parsing [alias.*] tables")?,
            None => HashMap::new(),
        }
    } else {
        HashMap::new()
    };

    let session_map: HashMap<String, String> = if let toml::Value::Table(table) = &mut value {
        match table.remove("session") {
            Some(v) => v.try_into().context("parsing [session] table")?,
            None => HashMap::new(),
        }
    } else {
        HashMap::new()
    };

    let defaults: Profile = value.try_into().context("parsing top-level keys")?;

    Ok(ConfigFile {
        defaults,
        profile: profile_map,
        alias: alias_map,
        session: session_map,
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
        // Aliases don't merge field-by-field; the closest-to-cwd file
        // wins wholesale. Layers load farther-first, so a later insert
        // (closer file) overwrites.
        for (name, alias) in cfg.alias {
            pool.aliases.insert(name, alias);
        }
        // Session bindings, same wholesale closest-to-cwd-wins rule as
        // aliases: a later insert (closer file) overwrites the name.
        for (name, uuid) in cfg.session {
            pool.sessions.insert(name, uuid);
        }
        pool.sources.push(path);
    }

    warn_on_shadowed_aliases(&pool);
    Ok(pool)
}

/// Warn (loudly, on stderr) when a loaded alias name collides with a
/// built-in subcommand. The built-in always wins the lookup, so such
/// an alias is dead -- surface it instead of letting it silently
/// no-op.
fn warn_on_shadowed_aliases(pool: &Pool) {
    for name in shadowed_alias_names(pool) {
        eprintln!(
            "warning: alias `{name}` is shadowed by the built-in `{name}` subcommand; rename it to use this alias"
        );
    }
}

/// The (sorted) alias names in `pool` that collide with a built-in
/// subcommand. Pure half of [`warn_on_shadowed_aliases`], split out so
/// the shadow logic is unit-testable without capturing stderr.
fn shadowed_alias_names(pool: &Pool) -> Vec<&String> {
    let mut shadowed: Vec<&String> = pool
        .aliases
        .keys()
        .filter(|n| is_builtin_subcommand(n))
        .collect();
    shadowed.sort();
    shadowed
}

/// Convenience: load the pool keyed off the current cwd.
pub fn load_pool() -> Result<Pool> {
    let cwd = std::env::current_dir().context("getting current dir")?;
    load_pool_from(&cwd)
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

    // -- Shadowed-alias detection ------------------------------------------

    #[test]
    fn shadowed_aliases_track_the_real_subcommand_set() {
        let mut pool = Pool::default();
        for name in ["show", "doctor", "skill", "agent", "my-verb"] {
            pool.aliases.insert(name.to_string(), Alias::default());
        }
        let shadowed: Vec<&str> = shadowed_alias_names(&pool)
            .iter()
            .map(|s| s.as_str())
            .collect();
        // Real subcommands shadow (regression: these were missing from the
        // old hand-list, so they warned for nobody).
        assert!(shadowed.contains(&"show"));
        assert!(shadowed.contains(&"doctor"));
        // #268: `skill`/`agent` were removed in #130 -- legal alias names
        // again, no spurious warning. A genuine user verb never shadows.
        assert!(!shadowed.contains(&"skill"));
        assert!(!shadowed.contains(&"agent"));
        assert!(!shadowed.contains(&"my-verb"));
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
    fn pool_parses_session_bindings() {
        let tmp = tempfile::tempdir().unwrap();
        write_file(tmp.path(), "repo/.git/HEAD", "");
        write_file(
            tmp.path(),
            "repo/roba.toml",
            r#"
[session]
meta = "0199aabb-ccdd"
worktree-a = "0199eeff-0011"
"#,
        );
        write_file(tmp.path(), "repo/sub/.gitkeep", "");
        let pool = load_pool_from(&tmp.path().join("repo/sub")).unwrap();
        assert_eq!(
            pool.sessions.get("meta").map(String::as_str),
            Some("0199aabb-ccdd")
        );
        assert_eq!(
            pool.sessions.get("worktree-a").map(String::as_str),
            Some("0199eeff-0011")
        );
    }

    #[test]
    fn pool_walkup_session_closest_wins() {
        let tmp = tempfile::tempdir().unwrap();
        write_file(tmp.path(), "repo/.git/HEAD", "");
        // farther file binds `meta` and `only-far`
        write_file(
            tmp.path(),
            "repo/roba.toml",
            r#"
[session]
meta = "far-uuid"
only-far = "far-only-uuid"
"#,
        );
        // closer file rebinds `meta` (wholesale) and adds `only-near`
        write_file(
            tmp.path(),
            "repo/sub/roba.toml",
            r#"
[session]
meta = "near-uuid"
only-near = "near-only-uuid"
"#,
        );
        write_file(tmp.path(), "repo/sub/inner/.gitkeep", "");
        let pool = load_pool_from(&tmp.path().join("repo/sub/inner")).unwrap();
        // Closer file wins wholesale on the colliding name.
        assert_eq!(
            pool.sessions.get("meta").map(String::as_str),
            Some("near-uuid")
        );
        // Non-colliding names from both files survive.
        assert_eq!(
            pool.sessions.get("only-far").map(String::as_str),
            Some("far-only-uuid")
        );
        assert_eq!(
            pool.sessions.get("only-near").map(String::as_str),
            Some("near-only-uuid")
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
}
