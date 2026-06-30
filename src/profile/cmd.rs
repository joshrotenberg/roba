//! The `roba profile {list,show,init,path,active}` subcommand
//! runners and their rendering helpers.

use anyhow::{Context, Result, bail};
use std::collections::HashMap;
use std::path::Path;

use super::pool::{discover_project_configs, load_pool, user_config_path};
use super::resolve::missing_profile_error;
use super::types::{Pool, Profile};

// ---------------------------------------------------------------------------
// Starter template + subcommand
// ---------------------------------------------------------------------------

/// The documented sample config, embedded from the repo-root
/// `roba-config.sample.toml`. Written verbatim by `roba profile init`,
/// and the canonical reference for the roba.toml schema (its comments
/// document every profile/alias key). A unit test parses it, so it
/// cannot drift from the actual `Profile`/`Alias` shapes.
pub const STARTER_CONFIG_TOML: &str = include_str!("../../roba-config.sample.toml");

/// Run a `roba profile <action>` subcommand.
pub fn run(action: crate::cli::ProfileAction) -> Result<()> {
    use crate::cli::ProfileAction;
    match action {
        ProfileAction::List => run_list(),
        ProfileAction::Show { name } => run_show(&name),
        ProfileAction::Init { force } => run_init(force),
        ProfileAction::Path => run_path(),
        ProfileAction::Active => run_active(),
        // `draft` is async (it makes a claude call), so it is dispatched via
        // [`run_draft`] by [`crate::dispatch`], never here.
        ProfileAction::Draft(_) => unreachable!("profile draft is dispatched via run_draft"),
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

/// Resolve which profile auto-applies for the current env + pool, with a
/// short human reason. The auto-apply order (mirrors `super::resolve`):
/// `ROBA_PROFILE` wins (and is a HARD ERROR if it names a profile not in
/// the pool), else a `default` profile if present, else none.
///
/// Shared by `profile active` and `config show` so the "what auto-applies"
/// answer is computed in exactly one place.
pub fn active_profile(pool: &Pool) -> Result<Option<(String, &'static str)>> {
    let env_name = std::env::var("ROBA_PROFILE").ok().filter(|s| !s.is_empty());
    if let Some(name) = env_name {
        if pool.get(&name).is_none() {
            bail!("ROBA_PROFILE={name} but no such profile in the pool");
        }
        return Ok(Some((name, "from ROBA_PROFILE env")));
    }
    if pool.get("default").is_some() {
        return Ok(Some(("default".to_string(), "auto-applied")));
    }
    Ok(None)
}

fn run_active() -> Result<()> {
    let pool = load_pool()?;

    let (name, reason) = match active_profile(&pool)? {
        Some(pair) => pair,
        None => {
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
        }
    };

    let profile = pool.get(&name).cloned().expect("checked above");
    // The header is metadata -- route it to stderr (matching
    // `config show`) so stdout carries only the re-parseable
    // `[profile.NAME]` TOML block (principle #2).
    eprintln!("active: {name} ({reason})");
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

/// Render one named profile back to TOML for `profile show` / `active`
/// / `draft` and the merged `config show` view.
pub(crate) fn render_named_profile(name: &str, profile: &Profile) -> Result<String> {
    let mut wrapper: HashMap<String, HashMap<String, Profile>> = HashMap::new();
    let mut inner: HashMap<String, Profile> = HashMap::new();
    inner.insert(name.to_string(), profile.clone());
    wrapper.insert("profile".to_string(), inner);
    toml::to_string_pretty(&wrapper).context("re-serializing profile")
}

// ---------------------------------------------------------------------------
// Draft: claude-assisted, parse-validated profile generation
// ---------------------------------------------------------------------------

/// Run `roba profile draft DESCRIPTION`.
///
/// The profile twin of `roba alias draft`: deterministic bookends around
/// one generation. Build a prompt from the bundled (parse-tested) profile
/// schema + the user's words via the shared [`crate::draft`] core, make a
/// single lean claude call, then validate the output with roba's REAL
/// [`Profile`] deserializer (`deny_unknown_fields`) and normalize it back
/// through `render_named_profile`. stdout gets only the canonical block;
/// collision warnings and write confirmations go to stderr. No retry loop.
pub async fn run_draft(args: crate::cli::ProfileDraftArgs) -> Result<()> {
    // 1. Deterministic prompt: schema-by-example + the user's description.
    let prompt = draft_prompt(&args.description);

    // 2. One lean claude call -- NOT routed through `run_ask`. The shared
    //    draft core handles the read-only, no-session, stdin-fed posture.
    let raw = crate::draft::generate(prompt, args.model.as_deref(), "roba: profile draft").await?;

    // 3. Validate with the real deserializer; require exactly one entry.
    let (name, profile) = parse_drafted_profile(&raw)?;

    // 4. Normalize so stdout is canonical regardless of model formatting.
    let block = render_named_profile(&name, &profile)?;

    match &args.write {
        Some(target) => {
            let path = match target {
                Some(p) => p.clone(),
                None => user_config_path().ok_or_else(|| {
                    anyhow::anyhow!(
                        "--write: cannot locate your user config; pass an explicit path (`--write PATH`)"
                    )
                })?,
            };
            // A duplicate `[profile.NAME]` table in the target would
            // hard-break the next config load -- hard error.
            if file_defines_profile(&path, &name)? {
                bail!(
                    "{} already defines [profile.{name}]; refusing to append a duplicate (it would break the next config load)",
                    path.display()
                );
            }
            crate::draft::append_block(&path, &block)?;
            eprintln!("wrote [profile.{name}] to {}", path.display());
        }
        None => {
            // Print mode: a name already in the pool is a warning, not an error.
            if load_pool()?.profiles.contains_key(&name) {
                eprintln!(
                    "warning: profile `{name}` already exists in your config pool; this draft would shadow or duplicate it"
                );
            }
        }
    }

    // A profile literally named `default` is legal and meaningful: it
    // auto-applies. Flag that so the user knows this draft changes the
    // no-flag behavior, not just adds an opt-in overlay.
    if name == "default" {
        eprintln!("note: a profile named `default` auto-applies when no --profile is given");
    }

    // stdout = the block only, byte-clean (pipeable to `>> roba.toml`).
    print!("{block}");
    Ok(())
}

/// Build the deterministic generation prompt: the bundled profile schema
/// (by example, so it cannot drift from the real deserializer) + the
/// user's description + firm single-block output instructions.
fn draft_prompt(description: &str) -> String {
    let schema = profile_sample_section();
    format!(
        "You are generating a single roba profile definition in TOML.\n\n\
         A roba profile is a named bundle of flag defaults, activated with \
         `--profile NAME`. Here is the profile schema, documented by \
         example -- these are the ONLY allowed keys, do not invent \
         fields:\n\n\
         {schema}\n\n\
         The user wants a profile for: {description}\n\n\
         Output requirements (follow exactly):\n\
         - Produce EXACTLY ONE `[profile.NAME]` TOML block and nothing else \
           (a sub-table like `[profile.NAME.vars]` is allowed).\n\
         - Pick a short, memorable kebab-case or single-word NAME from the description.\n\
         - Use ONLY the keys shown above.\n\
         - The block must be valid TOML that parses against that schema.\n\
         - Do NOT wrap the output in markdown code fences.\n\
         - Do NOT include any prose, comments, or explanation -- only the TOML block."
    )
}

/// Slice the schema section out of the bundled, parse-tested sample
/// config: the top of the file (the per-key catalog IS the profile
/// schema, since every key is a valid profile key) through the start of
/// the `# Aliases` banner. Falls back to the whole sample if that marker
/// moves, so a future sample reshuffle degrades to "embed everything"
/// rather than silently shipping an empty schema.
fn profile_sample_section() -> String {
    const SAMPLE: &str = STARTER_CONFIG_TOML;
    match SAMPLE.find("# Aliases") {
        Some(e) => SAMPLE[..e].trim_end().to_string(),
        None => SAMPLE.trim_end().to_string(),
    }
}

/// Parse a drafted profile block. Strip optional markdown fences (a model
/// may add them despite instructions), deserialize via the real
/// `{ profile: { NAME = Profile } }` shape so `deny_unknown_fields` on
/// [`Profile`] polices hallucinated keys, and require EXACTLY one entry.
/// On any failure, the error carries the raw model output for stderr.
fn parse_drafted_profile(raw: &str) -> Result<(String, Profile)> {
    #[derive(serde::Deserialize)]
    struct Wrapper {
        #[serde(default)]
        profile: HashMap<String, Profile>,
    }
    let cleaned = crate::draft::strip_code_fences(raw);
    let wrapper: Wrapper = toml::from_str(&cleaned).map_err(|e| {
        anyhow::anyhow!("drafted profile did not parse: {e}\n\n--- raw model output ---\n{raw}")
    })?;
    let mut entries: Vec<(String, Profile)> = wrapper.profile.into_iter().collect();
    match entries.len() {
        1 => Ok(entries.pop().expect("len checked == 1")),
        0 => {
            bail!(
                "drafted output defined no [profile.NAME] block\n\n--- raw model output ---\n{raw}"
            )
        }
        n => bail!(
            "drafted output defined {n} profile blocks (expected exactly one)\n\n--- raw model output ---\n{raw}"
        ),
    }
}

/// True when `path` already defines `[profile.name]`. A missing file
/// defines nothing. Unknown top-level tables (aliases, sessions, the
/// top-level defaults) are ignored -- only the profile map is probed --
/// but malformed TOML surfaces as an error rather than a silent "no".
fn file_defines_profile(path: &Path, name: &str) -> Result<bool> {
    #[derive(serde::Deserialize)]
    struct Probe {
        #[serde(default)]
        profile: HashMap<String, Profile>,
    }
    if !path.exists() {
        return Ok(false);
    }
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("reading --write target {}", path.display()))?;
    let probe: Probe = toml::from_str(&text)
        .with_context(|| format!("--write target {} is not valid TOML", path.display()))?;
    Ok(probe.profile.contains_key(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_sample_section_includes_schema_not_aliases() {
        let section = profile_sample_section();
        // The per-key catalog + a profile example are present...
        assert!(section.contains("[profile.review]"), "got:\n{section}");
        // ...but the slice stops before the aliases section.
        assert!(!section.contains("[alias.review]"), "got:\n{section}");
        assert!(!section.contains("Named sessions"), "got:\n{section}");
    }

    #[test]
    fn profile_sample_section_falls_back_to_whole_sample() {
        // Sanity: the marker exists in the real sample, so the slice is a
        // strict prefix of the whole file (the fallback is the full file).
        let section = profile_sample_section();
        assert!(STARTER_CONFIG_TOML.starts_with(&section[..section.len().min(40)]));
        assert!(section.len() < STARTER_CONFIG_TOML.len());
    }

    #[test]
    fn parse_drafted_profile_accepts_one_block() {
        let (name, profile) =
            parse_drafted_profile("[profile.worker]\nfull_auto = true\nmax_turns = 80").unwrap();
        assert_eq!(name, "worker");
        assert_eq!(profile.full_auto, Some(true));
        assert_eq!(profile.max_turns, Some(80));
    }

    #[test]
    fn parse_drafted_profile_strips_fences_first() {
        let (name, _) =
            parse_drafted_profile("```toml\n[profile.quick]\nmodel = \"claude-haiku-4-5\"\n```")
                .unwrap();
        assert_eq!(name, "quick");
    }

    #[test]
    fn parse_drafted_profile_rejects_zero_entries() {
        let err = parse_drafted_profile("# nothing here").unwrap_err();
        assert!(
            format!("{err:#}").contains("no [profile.NAME] block"),
            "{err:#}"
        );
    }

    #[test]
    fn parse_drafted_profile_rejects_two_entries() {
        let raw = "[profile.a]\nreadonly = true\n[profile.b]\nwritable = true";
        let err = parse_drafted_profile(raw).unwrap_err();
        assert!(format!("{err:#}").contains("2 profile blocks"), "{err:#}");
    }

    #[test]
    fn parse_drafted_profile_rejects_unknown_field() {
        // `deny_unknown_fields` on `Profile` rejects a hallucinated key; the
        // raw output is echoed for the user.
        let raw = "[profile.x]\nreadonly = true\nmade_up_key = true";
        let err = parse_drafted_profile(raw).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("did not parse"), "{msg}");
        assert!(msg.contains("made_up_key"), "{msg}");
        assert!(msg.contains("raw model output"), "{msg}");
    }

    #[test]
    fn parse_drafted_profile_accepts_default_name() {
        // `default` is a legal, meaningful profile name (it auto-applies).
        let (name, _) = parse_drafted_profile("[profile.default]\nreadonly = true").unwrap();
        assert_eq!(name, "default");
    }

    #[test]
    fn file_defines_profile_detects_duplicate() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("roba.toml");
        std::fs::write(
            &path,
            "[alias.review]\ndescription = \"r\"\n\n[profile.review]\nreadonly = true\n",
        )
        .unwrap();
        assert!(file_defines_profile(&path, "review").unwrap());
        assert!(!file_defines_profile(&path, "nope").unwrap());
    }

    #[test]
    fn file_defines_profile_missing_file_is_false() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("absent.toml");
        assert!(!file_defines_profile(&path, "anything").unwrap());
    }

    #[test]
    fn active_profile_resolution_order() {
        // ROBA_PROFILE wins -> else `default` -> else None. The three
        // branches live in one test so the process-global ROBA_PROFILE is
        // mutated sequentially (no cross-test race).
        let mut pool = Pool::default();
        pool.profiles
            .insert("worker".to_string(), Profile::default());
        pool.profiles
            .insert("default".to_string(), Profile::default());

        // No env, `default` present -> default auto-applies.
        unsafe {
            std::env::remove_var("ROBA_PROFILE");
        }
        let (name, reason) = active_profile(&pool).unwrap().unwrap();
        assert_eq!(name, "default");
        assert_eq!(reason, "auto-applied");

        // ROBA_PROFILE names a real profile -> it wins.
        unsafe {
            std::env::set_var("ROBA_PROFILE", "worker");
        }
        let (name, reason) = active_profile(&pool).unwrap().unwrap();
        assert_eq!(name, "worker");
        assert_eq!(reason, "from ROBA_PROFILE env");

        // ROBA_PROFILE names a missing profile -> hard error.
        unsafe {
            std::env::set_var("ROBA_PROFILE", "nope");
        }
        assert!(active_profile(&pool).is_err());

        // No env, no `default` -> None.
        unsafe {
            std::env::remove_var("ROBA_PROFILE");
        }
        let mut bare = Pool::default();
        bare.profiles
            .insert("worker".to_string(), Profile::default());
        assert!(active_profile(&bare).unwrap().is_none());
    }

    #[test]
    fn render_named_profile_round_trips() {
        let (name, profile) =
            parse_drafted_profile("[profile.worker]\nfull_auto = true\nmax_turns = 80").unwrap();
        let block = render_named_profile(&name, &profile).unwrap();
        assert!(block.contains("[profile.worker]"), "{block}");
        // The canonical block re-parses back to the same profile.
        let (name2, profile2) = parse_drafted_profile(&block).unwrap();
        assert_eq!(name2, "worker");
        assert_eq!(profile2.max_turns, Some(80));
    }
}
