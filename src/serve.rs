//! `roba serve` -- launch the roba-server MCP server, optionally as a persona.
//!
//! roba-server is env-configured (it reads `ROBA_*`). This launcher resolves a
//! named persona (a role-bearing `[profile.NAME]`) from the config pool and maps
//! it onto that env, then execs the server binary. Name -> config resolution
//! happens HERE, in the config-aware bin; the server only ever consumes RESOLVED
//! config. That invariant keeps one-process-one-persona (today, resolved config
//! as launch env) and a future multi-session server (#426, resolved config in a
//! per-session request) on the same server contract. See #428 / #424.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::cli::{EffortLevel, ServeArgs};
use crate::profile::{self, Pool, Profile};

/// Resolve `--profile` (if any) into `ROBA_*` env, then exec `roba-server`.
pub fn run(args: ServeArgs) -> Result<()> {
    // Resolve the persona FIRST so a bad name fails fast (and testably) before
    // we go looking for the server binary.
    let mut extra_env: Vec<(String, String)> = Vec::new();
    if let Some(name) = &args.profile {
        let pool = profile::load_pool()?;
        let named = pool
            .get(name)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!(unknown_profile_msg(name, &pool)))?;
        // Effective persona = top-level defaults + [profile.NAME], exactly as a
        // one-shot `roba --profile NAME` merges them.
        let mut effective = pool.defaults.clone();
        effective.merge_in(named);
        if !effective.mcp_config.is_empty() {
            eprintln!(
                "note: persona `{name}` sets mcp_config, which `roba serve` does not \
                 yet forward to server mode"
            );
        }
        extra_env = persona_env(&effective);
        // A structured persona pins a json_schema PATH; the server's ROBA_SCHEMA
        // is inline JSON, so read the file here (skipped if ROBA_SCHEMA is
        // already set -- env > profile).
        if std::env::var_os("ROBA_SCHEMA").is_none()
            && let Some(schema_path) = &effective.json_schema
        {
            let inline = std::fs::read_to_string(schema_path)
                .with_context(|| format!("reading persona json_schema `{schema_path}`"))?;
            extra_env.push(("ROBA_SCHEMA".into(), inline));
        }
    }

    let server = find_server()?;
    let mut cmd = std::process::Command::new(&server);
    // env > profile: only fill a var the caller has not already set, so an
    // explicit `ROBA_*` in the environment overrides the persona.
    for (k, v) in extra_env {
        if std::env::var_os(&k).is_none() {
            cmd.env(k, v);
        }
    }
    exec(cmd, &server)
}

/// Map a resolved persona onto the `ROBA_*` env roba-server reads. Emits a var
/// only for a field the profile actually sets; posture is the mutually-exclusive
/// {full_auto > writable > readonly-default} knob.
fn persona_env(p: &Profile) -> Vec<(String, String)> {
    let mut env: Vec<(String, String)> = Vec::new();
    if let Some(a) = &p.agent {
        env.push(("ROBA_AGENT".into(), a.clone()));
    }
    if let Some(m) = &p.model {
        env.push(("ROBA_MODEL".into(), m.clone()));
    }
    if let Some(e) = &p.effort {
        env.push(("ROBA_EFFORT".into(), effort_str(e).into()));
    }
    if let Some(f) = &p.fallback_model {
        env.push(("ROBA_FALLBACK_MODEL".into(), f.clone()));
    }
    if let Some(t) = p.max_turns {
        env.push(("ROBA_MAX_TURNS".into(), t.to_string()));
    }
    if let Some(b) = p.max_budget_usd {
        env.push(("ROBA_MAX_USD".into(), b.to_string()));
    }
    if p.full_auto == Some(true) {
        env.push(("ROBA_FULL_AUTO".into(), "1".into()));
    } else if p.writable == Some(true) {
        env.push(("ROBA_WRITABLE".into(), "1".into()));
    }
    if !p.allow_tool.is_empty() {
        env.push(("ROBA_ALLOW_TOOLS".into(), p.allow_tool.join(",")));
    }
    if !p.deny_tool.is_empty() {
        env.push(("ROBA_DENY_TOOLS".into(), p.deny_tool.join(",")));
    }
    env
}

fn effort_str(e: &EffortLevel) -> &'static str {
    match e {
        EffortLevel::Low => "low",
        EffortLevel::Medium => "medium",
        EffortLevel::High => "high",
        EffortLevel::Xhigh => "xhigh",
        EffortLevel::Max => "max",
    }
}

fn unknown_profile_msg(name: &str, pool: &Pool) -> String {
    let mut names: Vec<&str> = pool.profiles.keys().map(String::as_str).collect();
    names.sort_unstable();
    if names.is_empty() {
        format!("no profile named `{name}` (no profiles defined)")
    } else {
        format!(
            "no profile named `{name}`\nknown profiles: {}",
            names.join(", ")
        )
    }
}

/// Locate the `roba-server` binary next to this `roba` executable. It ships as a
/// separate binary, so a missing one is a clear, actionable error.
fn find_server() -> Result<PathBuf> {
    let name = if cfg!(windows) {
        "roba-server.exe"
    } else {
        "roba-server"
    };
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        let cand = dir.join(name);
        if cand.is_file() {
            return Ok(cand);
        }
    }
    bail!(
        "roba-server binary not found next to `roba`.\n\
         `roba serve` launches the roba-server MCP server, which ships as a separate \
         binary; build it (`cargo build -p roba-server`) or install it alongside `roba`."
    )
}

#[cfg(unix)]
fn exec(mut cmd: std::process::Command, server: &Path) -> Result<()> {
    use std::os::unix::process::CommandExt;
    // `exec` replaces this process image; it only returns on failure.
    Err(anyhow::Error::new(cmd.exec()).context(format!("exec {}", server.display())))
}

#[cfg(not(unix))]
fn exec(mut cmd: std::process::Command, server: &Path) -> Result<()> {
    let status = cmd
        .status()
        .with_context(|| format!("running {}", server.display()))?;
    std::process::exit(status.code().unwrap_or(1));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env_map(p: &Profile) -> HashMap<String, String> {
        persona_env(p).into_iter().collect()
    }

    #[test]
    fn persona_env_maps_role_and_envelope() {
        let p = Profile {
            agent: Some("reviewer".into()),
            model: Some("claude-opus-4-8".into()),
            effort: Some(EffortLevel::High),
            fallback_model: Some("claude-fable-5".into()),
            max_turns: Some(60),
            max_budget_usd: Some(5.0),
            allow_tool: vec!["Bash(gh pr view:*)".into()],
            deny_tool: vec!["Bash(git push:*)".into()],
            ..Default::default()
        };
        let m = env_map(&p);
        assert_eq!(m["ROBA_AGENT"], "reviewer");
        assert_eq!(m["ROBA_MODEL"], "claude-opus-4-8");
        assert_eq!(m["ROBA_EFFORT"], "high");
        assert_eq!(m["ROBA_FALLBACK_MODEL"], "claude-fable-5");
        assert_eq!(m["ROBA_MAX_TURNS"], "60");
        assert_eq!(m["ROBA_MAX_USD"], "5");
        assert_eq!(m["ROBA_ALLOW_TOOLS"], "Bash(gh pr view:*)");
        assert_eq!(m["ROBA_DENY_TOOLS"], "Bash(git push:*)");
        // readonly default -> no posture env
        assert!(!m.contains_key("ROBA_FULL_AUTO"));
        assert!(!m.contains_key("ROBA_WRITABLE"));
    }

    #[test]
    fn persona_env_posture_is_mutually_exclusive() {
        // full_auto wins even if writable is also somehow set.
        let full = Profile {
            full_auto: Some(true),
            writable: Some(true),
            ..Default::default()
        };
        let m = env_map(&full);
        assert_eq!(m["ROBA_FULL_AUTO"], "1");
        assert!(!m.contains_key("ROBA_WRITABLE"));

        let w = Profile {
            writable: Some(true),
            ..Default::default()
        };
        let m = env_map(&w);
        assert_eq!(m["ROBA_WRITABLE"], "1");
        assert!(!m.contains_key("ROBA_FULL_AUTO"));
    }

    #[test]
    fn persona_env_empty_for_bare_profile() {
        assert!(persona_env(&Profile::default()).is_empty());
    }
}
