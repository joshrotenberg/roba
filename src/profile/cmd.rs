//! The `roba profile {list,show,init,path,active}` subcommand
//! runners and their rendering helpers.

use anyhow::{Context, Result, bail};
use std::collections::HashMap;

use super::pool::{discover_project_configs, load_pool, user_config_path};
use super::resolve::missing_profile_error;
use super::types::Profile;

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
