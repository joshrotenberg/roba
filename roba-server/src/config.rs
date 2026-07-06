//! Launch configuration.
//!
//! Env-sourced for now. A later pass maps roba's CLI / config-pool layering
//! (profiles, `ROBA_*`) onto this, and the posture becomes a roba-core
//! `Permissions` instead of the fixed read-only default in `backend`.

/// How the single session is configured for this process's lifetime.
#[derive(Debug, Clone, Default)]
pub struct ServerConfig {
    /// Model override (e.g. `"haiku"`). `None` uses claude's default.
    pub model: Option<String>,
    /// Inline JSON schema. `Some` puts the session in structured mode for its
    /// whole life (every turn returns `structuredContent`); `None` is prose.
    pub schema: Option<String>,
    /// Optional session spend ceiling in USD (a `Conversation` budget hard-stop).
    pub max_usd: Option<f64>,
    /// Expose the inward (south) MCP surface: the running claude session gets a
    /// reflexive `context` tool to introspect its own roba-server execution.
    /// Default on; disable with `ROBA_INWARD=0`.
    pub inward: bool,
}

impl ServerConfig {
    /// Read the config from `ROBA_MODEL` / `ROBA_SCHEMA` / `ROBA_MAX_USD` /
    /// `ROBA_INWARD`.
    pub fn from_env() -> Self {
        Self {
            model: env_nonempty("ROBA_MODEL"),
            schema: env_nonempty("ROBA_SCHEMA"),
            max_usd: env_nonempty("ROBA_MAX_USD").and_then(|s| s.parse().ok()),
            inward: env_bool("ROBA_INWARD", true),
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

/// A truthy/falsy env flag: `1`/`true`/`yes`/`on` => true, `0`/`false`/`no`/`off`
/// => false, anything else (or unset) => `default`.
fn env_bool(key: &str, default: bool) -> bool {
    match std::env::var(key).ok().as_deref().map(str::trim) {
        Some("1" | "true" | "yes" | "on") => true,
        Some("0" | "false" | "no" | "off") => false,
        _ => default,
    }
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
}
