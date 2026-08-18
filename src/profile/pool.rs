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

/// Load one ambient legacy config layer.
///
/// Project `roba.toml` is shared with the provider-neutral startup contract
/// during the compatibility window. A top-level `version` marker belongs to
/// that strict contract, so the legacy profile loader must ignore the file
/// instead of trying to reinterpret `[agent]`, `[execution]`, and `[context]`
/// as legacy profile keys. Explicit legacy bundle/config validation continues
/// to use [`load_file`] and therefore still fails loudly on the wrong schema.
fn load_ambient_legacy_file(path: &Path) -> Result<Option<ConfigFile>> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("reading config at {}", path.display()))?;
    let value: toml::Value = toml::from_str(&content)
        .with_context(|| format!("parsing config TOML at {}", path.display()))?;
    if value
        .as_table()
        .is_some_and(|table| table.contains_key("version"))
    {
        return Ok(None);
    }
    parse_config_str(&content)
        .with_context(|| format!("parsing config at {}", path.display()))
        .map(Some)
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
    load_pool_with_bundle(cwd, None, false)
}

/// Load the config pool, optionally overlaying a `.roba/` bundle.
///
/// `bundle` is a bundle directory (its `roba.toml` is the config). When
/// `bundle_only` is true (the roba-hermetic axis), ONLY the bundle loads -- the
/// ambient user + project walk is skipped, so the bundle is the sole config
/// (`Pool::default()` if it has no `roba.toml`). Otherwise the bundle layers as
/// the CLOSEST (highest-precedence) source on top of the ambient pool.
pub fn load_pool_with_bundle(cwd: &Path, bundle: Option<&Path>, bundle_only: bool) -> Result<Pool> {
    let bundle = bundle.map(load_bundle_pool).transpose()?;
    load_pool_with_preparsed_bundle(cwd, bundle, bundle_only)
}

/// Parse only one bundle's optional `roba.toml` into an owned snapshot.
/// Callers can retain this so validation/inspection and later execution use
/// the same bytes rather than reopening the file.
pub(crate) fn load_bundle_pool(bundle: &Path) -> Result<Pool> {
    let mut pool = Pool::default();
    let path = bundle.join("roba.toml");
    if path.is_file() {
        let config = load_file(&path)?;
        merge_config_file(&mut pool, path, config);
    }
    Ok(pool)
}

/// Merge ambient configuration with an already-parsed bundle snapshot.
/// The bundle remains the closest/highest-precedence layer.
pub(crate) fn load_pool_with_preparsed_bundle(
    cwd: &Path,
    bundle: Option<Pool>,
    bundle_only: bool,
) -> Result<Pool> {
    let mut pool = Pool::default();

    let mut layers: Vec<PathBuf> = Vec::new();
    if !bundle_only {
        if let Some(user) = user_config_path()
            && user.is_file()
        {
            layers.push(user);
        }
        layers.extend(discover_project_configs(cwd));
    }
    for path in layers {
        if let Some(cfg) = load_ambient_legacy_file(&path)? {
            merge_config_file(&mut pool, path, cfg);
        }
    }
    if let Some(bundle) = bundle {
        merge_pool(&mut pool, bundle);
    }

    warn_on_shadowed_aliases(&pool);
    Ok(pool)
}

fn merge_config_file(pool: &mut Pool, path: PathBuf, config: ConfigFile) {
    pool.defaults.merge_in(config.defaults);
    for (name, profile) in config.profile {
        pool.profiles.entry(name).or_default().merge_in(profile);
    }
    // Aliases and session bindings merge wholesale; the later/closer layer
    // wins on name collisions.
    pool.aliases.extend(config.alias);
    pool.sessions.extend(config.session);
    pool.sources.push(path);
}

fn merge_pool(pool: &mut Pool, bundle: Pool) {
    pool.defaults.merge_in(bundle.defaults);
    for (name, profile) in bundle.profiles {
        pool.profiles.entry(name).or_default().merge_in(profile);
    }
    pool.aliases.extend(bundle.aliases);
    pool.sessions.extend(bundle.sessions);
    pool.sources.extend(bundle.sources);
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

/// The ordered config layers (source path + parsed file) for `cwd`,
/// lowest precedence first -- the SAME order [`load_pool_from`] merges
/// (user config, then the project chain farther-from-cwd first).
///
/// Exposed for `roba config show --sources`, which needs PER-FILE
/// attribution that the merged [`Pool`] has lost: knowing a key's final
/// value is not enough, you also have to know which file set it.
pub(crate) fn load_layers_from(cwd: &Path) -> Result<Vec<(PathBuf, ConfigFile)>> {
    let mut paths: Vec<PathBuf> = Vec::new();
    if let Some(user) = user_config_path()
        && user.is_file()
    {
        paths.push(user);
    }
    paths.extend(discover_project_configs(cwd));

    let mut out = Vec::with_capacity(paths.len());
    for path in paths {
        if let Some(cfg) = load_ambient_legacy_file(&path)? {
            out.push((path, cfg));
        }
    }
    Ok(out)
}

/// Convenience: load the per-file layers keyed off the current cwd.
pub(crate) fn load_layers() -> Result<Vec<(PathBuf, ConfigFile)>> {
    let cwd = std::env::current_dir().context("getting current dir")?;
    load_layers_from(&cwd)
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

    /// Every shipped `examples/roba-*.toml` bundle must parse through the real
    /// `deny_unknown_fields` deserializer, so a schema change can never leave a
    /// published example broken (the #308 "tested setups" guarantee).
    #[test]
    fn example_bundles_parse() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples");
        let mut checked = 0;
        for entry in std::fs::read_dir(&dir).expect("examples/ dir exists") {
            let path = entry.unwrap().path();
            if path.extension().and_then(|e| e.to_str()) != Some("toml") {
                continue;
            }
            let content = std::fs::read_to_string(&path).unwrap();
            parse_config_str(&content)
                .unwrap_or_else(|e| panic!("example {} failed to parse: {e:#}", path.display()));
            checked += 1;
        }
        assert!(
            checked >= 3,
            "expected the shipped example bundles, found {checked}"
        );
    }

    /// The worker-lifecycle bundle carries its three holes (`<GATE>` etc.) only
    /// inside template strings, so it still deserializes through the real path.
    /// Mirrors `sample_config_parses_and_documents_the_schema`: parse, then
    /// assert the lifecycle profiles and verbs are present.
    #[test]
    fn worker_lifecycle_example_parses_and_documents_the_lifecycle() {
        let toml = include_str!("../../examples/roba-worker-lifecycle.toml");
        let cfg = parse_config_str(toml)
            .expect("examples/roba-worker-lifecycle.toml must parse as a valid config");
        for name in ["plan", "worker", "review"] {
            assert!(
                cfg.profile.contains_key(name),
                "worker-lifecycle bundle is missing [profile.{name}]"
            );
        }
        for name in ["issue", "ship", "revise", "review"] {
            assert!(
                cfg.alias.contains_key(name),
                "worker-lifecycle bundle is missing [alias.{name}]"
            );
        }
        // The holes live inside template strings, never as bare keys.
        assert!(
            cfg.alias["ship"]
                .template
                .as_deref()
                .unwrap()
                .contains("<GATE>")
        );
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

    #[test]
    fn bundle_layers_closest_and_sole() {
        let tmp = tempfile::tempdir().unwrap();
        // .git marker so the project walk stops at this dir.
        write_file(tmp.path(), ".git/HEAD", "");
        // Ambient project config: a top-level model plus a profile `p`.
        write_file(
            tmp.path(),
            "roba.toml",
            "model = \"opus\"\n[profile.p]\nmodel = \"opus\"\n",
        );
        // The bundle overrides the top-level model.
        write_file(tmp.path(), ".roba/roba.toml", "model = \"haiku\"\n");
        let bundle = tmp.path().join(".roba");

        // Additive: the bundle is the closest layer, so it wins per-key, and the
        // ambient profile is still present.
        let pool = load_pool_with_bundle(tmp.path(), Some(&bundle), false).unwrap();
        assert_eq!(pool.defaults.model.as_deref(), Some("haiku"));
        assert!(pool.profiles.contains_key("p"));

        // Sole (roba-hermetic): only the bundle; the ambient project config,
        // including its profile `p`, is skipped.
        let pool = load_pool_with_bundle(tmp.path(), Some(&bundle), true).unwrap();
        assert_eq!(pool.defaults.model.as_deref(), Some("haiku"));
        assert!(!pool.profiles.contains_key("p"));
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

    #[test]
    fn ambient_legacy_pool_ignores_versioned_startup_config() {
        let tmp = tempfile::tempdir().unwrap();
        write_file(tmp.path(), ".git/HEAD", "");
        write_file(
            tmp.path(),
            "roba.toml",
            "version = 1\n[agent]\nprovider = 'codex'\n",
        );

        let pool = load_pool_from(tmp.path()).unwrap();
        assert!(
            !pool.sources.contains(&tmp.path().join("roba.toml")),
            "versioned startup config must not enter the legacy pool: {:?}",
            pool.sources
        );

        let explicit = load_file(&tmp.path().join("roba.toml")).unwrap_err();
        assert!(format!("{explicit:#}").contains("parsing top-level keys"));
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
