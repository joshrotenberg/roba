//! `roba persona` -- inspect personas (role-bearing profiles).
//!
//! A persona is not a new config primitive: it is a `[profile.NAME]` whose
//! defining field is `agent`, a native Claude agent selected by `--agent`.
//! This module is a thin, read-only compatibility view over the merged legacy
//! config pool.

use anyhow::{Result, bail};

use crate::agent_check::find_agent_file;
use crate::cli::PersonaAction;
use crate::profile::cmd::render_named_profile;
use crate::profile::{self, Profile};

/// Run a `roba persona <action>` subcommand.
pub fn run(action: PersonaAction) -> Result<()> {
    match action {
        PersonaAction::List => run_list(),
        PersonaAction::Show { name } => run_show(&name),
    }
}

fn run_list() -> Result<()> {
    let pool = profile::load_pool()?;
    let mut personas: Vec<(&String, &Profile)> = pool
        .profiles
        .iter()
        .filter(|(_, profile)| profile.agent.is_some())
        .collect();
    if personas.is_empty() {
        eprintln!("no personas defined (a persona is a [profile.NAME] with `agent` set)");
        if pool.sources.is_empty() {
            eprintln!("hint: add a [profile.NAME] with `agent = \"...\"` to a roba.toml");
        } else {
            eprintln!("sources checked:");
            for source in &pool.sources {
                eprintln!("  {}", source.display());
            }
        }
        return Ok(());
    }
    personas.sort_by(|a, b| a.0.cmp(b.0));
    print!("{}", render_persona_list(&personas));
    Ok(())
}

/// Render the `persona list` table. Columns are NAME, AGENT, DESCRIPTION.
fn render_persona_list(personas: &[(&String, &Profile)]) -> String {
    let name_width = personas
        .iter()
        .map(|(name, _)| name.len())
        .max()
        .unwrap_or(4)
        .max(4);
    let agent_width = personas
        .iter()
        .map(|(_, profile)| profile.agent.as_deref().unwrap_or("").len())
        .max()
        .unwrap_or(5)
        .max(5);
    let mut output = format!(
        "{:<name_width$}  {:<agent_width$}  DESCRIPTION\n",
        "NAME", "AGENT"
    );
    for (name, profile) in personas {
        let agent = profile.agent.as_deref().unwrap_or("");
        let description = profile.description.as_deref().unwrap_or("");
        let marker = if profile.full_auto == Some(true) {
            "  [!] unsafe: full_auto"
        } else {
            ""
        };
        let line = format!("{name:<name_width$}  {agent:<agent_width$}  {description}{marker}");
        output.push_str(line.trim_end());
        output.push('\n');
    }
    output
}

fn run_show(name: &str) -> Result<()> {
    let pool = profile::load_pool()?;
    match pool.get(name) {
        Some(profile) if profile.agent.is_some() => {
            print!("{}", render_named_profile(name, profile)?);
            let agent = profile.agent.as_deref().unwrap_or_default();
            let cwd = std::env::current_dir().unwrap_or_default();
            match find_agent_file(agent, &cwd) {
                Some(path) => eprintln!("agent `{agent}` -> {}", path.display()),
                None => eprintln!("agent `{agent}` not found under .claude/agents/ (project or ~)"),
            }
            Ok(())
        }
        Some(_) => bail!(
            "profile `{name}` is not a persona (no `agent` set)\n\
             hint: `roba persona list` shows personas; `roba profile show {name}` shows the profile"
        ),
        None => {
            let mut names: Vec<&str> = pool
                .profiles
                .iter()
                .filter(|(_, profile)| profile.agent.is_some())
                .map(|(name, _)| name.as_str())
                .collect();
            names.sort_unstable();
            let known = if names.is_empty() {
                "no personas defined".to_string()
            } else {
                format!("known personas: {}", names.join(", "))
            };
            bail!("no persona named `{name}`\n{known}");
        }
    }
}
