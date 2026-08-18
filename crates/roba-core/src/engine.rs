//! Private Claude command configuration derived from a provider-neutral turn.

use claude_wrapper::{Effort, QueryCommand};

use crate::session::{apply_session, derive_session_name};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) enum Session {
    #[default]
    Fresh,
    Resume(String),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) enum Permissions {
    #[default]
    ReadOnly,
    WorkspaceWrite,
    FullAuto,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Config {
    pub(crate) prompt: String,
    pub(crate) model: Option<String>,
    pub(crate) effort: Option<Effort>,
    pub(crate) permissions: Permissions,
    pub(crate) allow_tools: Vec<String>,
    pub(crate) deny_tools: Vec<String>,
    pub(crate) session: Session,
    pub(crate) max_turns: Option<u32>,
    pub(crate) max_budget_usd: Option<f64>,
    pub(crate) timeout_secs: Option<u64>,
    pub(crate) append_system_prompt: Option<String>,
    pub(crate) mcp_config: Vec<String>,
}

impl Config {
    pub(crate) fn new(prompt: impl Into<String>) -> Self {
        Self {
            prompt: prompt.into(),
            ..Self::default()
        }
    }
}

pub(crate) fn query_command(config: &Config) -> QueryCommand {
    let name = derive_session_name(&config.prompt);
    apply_session(
        QueryCommand::new(config.prompt.clone())
            .name(name)
            .prompt_via_stdin(true),
        config,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_provider_neutral_and_read_only() {
        let config = Config::new("hello");
        assert_eq!(config.prompt, "hello");
        assert_eq!(config.permissions, Permissions::ReadOnly);
        assert_eq!(config.session, Session::Fresh);
        assert!(config.model.is_none());
        assert!(config.max_turns.is_none());
        assert!(config.max_budget_usd.is_none());
        assert!(config.timeout_secs.is_none());
    }
}
