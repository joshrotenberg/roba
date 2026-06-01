//! File discovery and loading: walk up from cwd collecting every
//! `roba.toml`, parse each into a [`ConfigFile`], and merge them into
//! a single [`Pool`].

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::home_dir;
use super::types::{ConfigFile, Pool, Profile};

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
}
