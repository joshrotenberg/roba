//! `roba persona` -- inspect personas (role-bearing profiles).
//!
//! A persona is not a new config primitive: it is a `[profile.NAME]` whose
//! defining field is `agent` (the role, a native claude agent selected via
//! claude's own `--agent`). This module is a thin, read-only veneer that lists
//! and shows the role-bearing profiles in the merged config pool. See #428.
//!
//! `list` and `show` mirror `roba alias` / `roba profile`: stdout carries the
//! re-parseable data, stderr carries metadata (principle: stdout = answer).

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
        .filter(|(_, p)| p.agent.is_some())
        .collect();
    if personas.is_empty() {
        eprintln!("no personas defined (a persona is a [profile.NAME] with `agent` set)");
        if pool.sources.is_empty() {
            eprintln!("hint: add a [profile.NAME] with `agent = \"...\"` to a roba.toml");
        } else {
            eprintln!("sources checked:");
            for s in &pool.sources {
                eprintln!("  {}", s.display());
            }
        }
        return Ok(());
    }
    personas.sort_by(|a, b| a.0.cmp(b.0));
    print!("{}", render_persona_list(&personas));
    Ok(())
}

/// Render the `persona list` table (assumes a non-empty slice, sorted by name).
/// Columns: NAME, AGENT, DESCRIPTION. A `full_auto` persona is flagged unsafe
/// inline; the text marker survives `--plain` / `NO_COLOR`. Ends with a newline.
fn render_persona_list(personas: &[(&String, &Profile)]) -> String {
    let name_w = personas
        .iter()
        .map(|(n, _)| n.len())
        .max()
        .unwrap_or(4)
        .max(4);
    let agent_w = personas
        .iter()
        .map(|(_, p)| p.agent.as_deref().unwrap_or("").len())
        .max()
        .unwrap_or(5)
        .max(5);
    let mut out = format!("{:<name_w$}  {:<agent_w$}  DESCRIPTION\n", "NAME", "AGENT");
    for (name, p) in personas {
        let agent = p.agent.as_deref().unwrap_or("");
        let desc = p.description.as_deref().unwrap_or("");
        let marker = if p.full_auto == Some(true) {
            "  [!] unsafe: full_auto"
        } else {
            ""
        };
        let line = format!("{name:<name_w$}  {agent:<agent_w$}  {desc}{marker}");
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
}

fn run_show(name: &str) -> Result<()> {
    let pool = profile::load_pool()?;
    match pool.get(name) {
        // A persona: a profile with a role pinned. stdout = the re-parseable
        // [profile.NAME] block; stderr = the resolved agent file (metadata).
        Some(p) if p.agent.is_some() => {
            print!("{}", render_named_profile(name, p)?);
            let agent = p.agent.as_deref().unwrap_or_default();
            let cwd = std::env::current_dir().unwrap_or_default();
            match find_agent_file(agent, &cwd) {
                Some(path) => eprintln!("agent `{agent}` -> {}", path.display()),
                None => {
                    eprintln!("agent `{agent}` not found under .claude/agents/ (project or ~)")
                }
            }
            Ok(())
        }
        // A profile, but not a persona (no role): point at the profile view.
        Some(_) => bail!(
            "profile `{name}` is not a persona (no `agent` set)\n\
             hint: `roba persona list` shows personas; `roba profile show {name}` shows the profile"
        ),
        // No such profile: list the personas that do exist.
        None => {
            let mut names: Vec<&str> = pool
                .profiles
                .iter()
                .filter(|(_, p)| p.agent.is_some())
                .map(|(n, _)| n.as_str())
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
