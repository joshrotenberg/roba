//! Launch configuration.
//!
//! Env-sourced for now. A later pass maps roba's CLI / config-pool layering
//! (profiles, `ROBA_*`) onto this, and the posture becomes a roba-core
//! `Permissions` instead of the fixed read-only default in `backend`.

use claude_wrapper::Effort;

/// How the single session is configured for this process's lifetime.
#[derive(Debug, Clone, Default)]
pub struct ServerConfig {
    /// Model override (e.g. `"haiku"`). `None` uses claude's default.
    pub model: Option<String>,
    /// The role the session runs as: a native claude agent, selected via
    /// claude's own `--agent` (`ROBA_AGENT`). A persona's defining field
    /// (#428): the agent file carries the role's system prompt, tool posture,
    /// and model; roba-server just names it and wraps it in the run envelope.
    pub agent: Option<String>,
    /// Reasoning effort (`ROBA_EFFORT`: low / medium / high / xhigh / max).
    pub effort: Option<Effort>,
    /// Fallback model when the primary is overloaded (`ROBA_FALLBACK_MODEL`).
    pub fallback_model: Option<String>,
    /// Cap the agentic turn count per prompt (`ROBA_MAX_TURNS`).
    pub max_turns: Option<u32>,
    /// Inline JSON schema. `Some` puts the session in structured mode for its
    /// whole life (every turn returns `structuredContent`); `None` is prose.
    pub schema: Option<String>,
    /// Optional session spend ceiling in USD (a `Conversation` budget hard-stop).
    pub max_usd: Option<f64>,
    /// Expose the inward (south) MCP surface: the running claude session gets a
    /// reflexive `context` tool to introspect its own roba-server execution.
    /// Default on; disable with `ROBA_INWARD=0`.
    pub inward: bool,
    /// Full-auto posture: the session bypasses permission checks (all tools,
    /// including Bash / git / edits). Off by default (read-only Read/Glob/Grep).
    /// Enable with `ROBA_FULL_AUTO=1`. A loaded gun -- opt in per task.
    pub full_auto: bool,
    /// Writable posture: read-only plus Edit + Write (`ROBA_WRITABLE=1`).
    /// Ignored under full_auto.
    pub writable: bool,
    /// Extra tool patterns allowed on top of the posture (`ROBA_ALLOW_TOOLS`,
    /// comma-separated), e.g. `Bash(gh:*)` for a read-plus-review session
    /// without full-auto.
    pub allow_tools: Vec<String>,
    /// Tool patterns to deny (`ROBA_DENY_TOOLS`, comma-separated), applied on
    /// top of any posture (e.g. full_auto minus `Bash(git push:*)`).
    pub deny_tools: Vec<String>,
}

impl ServerConfig {
    /// Read the config from `ROBA_MODEL` / `ROBA_SCHEMA` / `ROBA_MAX_USD` /
    /// `ROBA_INWARD`.
    pub fn from_env() -> Self {
        Self {
            model: env_nonempty("ROBA_MODEL"),
            agent: env_nonempty("ROBA_AGENT"),
            effort: env_effort("ROBA_EFFORT"),
            fallback_model: env_nonempty("ROBA_FALLBACK_MODEL"),
            max_turns: env_nonempty("ROBA_MAX_TURNS").and_then(|s| s.parse().ok()),
            schema: env_nonempty("ROBA_SCHEMA"),
            max_usd: env_nonempty("ROBA_MAX_USD").and_then(|s| s.parse().ok()),
            inward: env_bool("ROBA_INWARD", true),
            full_auto: env_bool("ROBA_FULL_AUTO", false),
            writable: env_bool("ROBA_WRITABLE", false),
            allow_tools: env_list("ROBA_ALLOW_TOOLS"),
            deny_tools: env_list("ROBA_DENY_TOOLS"),
        }
    }

    /// True when a schema is set, i.e. the session is in structured mode.
    pub fn structured(&self) -> bool {
        self.schema.is_some()
    }
}

fn env_nonempty(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|s| !s.is_empty())
}

/// Parse `ROBA_EFFORT` into a claude [`Effort`]. Case-insensitive and trimmed;
/// an unrecognized or unset value yields `None` (the flag is simply not passed).
fn env_effort(key: &str) -> Option<Effort> {
    match env_nonempty(key)?.trim().to_ascii_lowercase().as_str() {
        "low" => Some(Effort::Low),
        "medium" => Some(Effort::Medium),
        "high" => Some(Effort::High),
        "xhigh" => Some(Effort::Xhigh),
        "max" => Some(Effort::Max),
        _ => None,
    }
}

/// A truthy/falsy env flag: `1`/`true`/`yes`/`on` => true, `0`/`false`/`no`/`off`
/// => false, anything else (or unset) => `default`.
fn env_bool(key: &str, default: bool) -> bool {
    match std::env::var(key).ok().as_deref().map(str::trim) {
        Some("1" | "true" | "yes" | "on") => true,
        Some("0" | "false" | "no" | "off") => false,
        _ => default,
    }
}

/// A comma-separated env list. Unset or empty yields an empty Vec; entries are
/// trimmed and empties dropped.
fn env_list(key: &str) -> Vec<String> {
    std::env::var(key)
        .ok()
        .map(|s| {
            s.split(',')
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn structured_reflects_schema_presence() {
        let prose = ServerConfig::default();
        assert!(!prose.structured());

        let structured = ServerConfig {
            schema: Some(r#"{"type":"object"}"#.to_string()),
            ..Default::default()
        };
        assert!(structured.structured());
    }

    #[test]
    fn env_list_splits_trims_and_drops_empties() {
        // SAFETY: sets a uniquely-named var, reads it, removes it.
        unsafe { std::env::set_var("ROBA_TEST_ALLOW_LIST", " Bash(gh:*) , Read ,, Grep ") };
        assert_eq!(
            env_list("ROBA_TEST_ALLOW_LIST"),
            vec![
                "Bash(gh:*)".to_string(),
                "Read".to_string(),
                "Grep".to_string()
            ]
        );
        unsafe { std::env::remove_var("ROBA_TEST_ALLOW_LIST") };
        assert!(env_list("ROBA_TEST_ALLOW_LIST_UNSET").is_empty());
    }

    #[test]
    fn env_effort_parses_case_insensitively_and_ignores_unknown() {
        // SAFETY: sets a uniquely-named var, reads it, removes it.
        unsafe { std::env::set_var("ROBA_TEST_EFFORT", "High") };
        assert_eq!(env_effort("ROBA_TEST_EFFORT"), Some(Effort::High));
        unsafe { std::env::set_var("ROBA_TEST_EFFORT", "  xhigh ") };
        assert_eq!(env_effort("ROBA_TEST_EFFORT"), Some(Effort::Xhigh));
        unsafe { std::env::set_var("ROBA_TEST_EFFORT", "bogus") };
        assert_eq!(env_effort("ROBA_TEST_EFFORT"), None);
        unsafe { std::env::remove_var("ROBA_TEST_EFFORT") };
        assert_eq!(env_effort("ROBA_TEST_EFFORT_UNSET"), None);
    }
}
