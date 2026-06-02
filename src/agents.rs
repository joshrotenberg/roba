//! Bundled agent library + the `roba agent {install,list,show}`
//! subcommands.
//!
//! `BUNDLED_AGENTS` is generated at build time by `build.rs` (see
//! `OUT_DIR/bundled_agents.rs`): a `&[(name, relative_path, body)]`
//! array embedding every file under the repo's `agents/` directory.
//! The handlers delegate to [`crate::library`], which holds the logic
//! shared with the skill library.

use anyhow::Result;

use crate::cli::AgentAction;
use crate::library;

include!(concat!(env!("OUT_DIR"), "/bundled_agents.rs"));

/// Run a `roba agent <action>` subcommand.
pub fn run(action: AgentAction) -> Result<()> {
    match action {
        AgentAction::Install(args) => library::run_install(BUNDLED_AGENTS, args, library::AGENTS),
        AgentAction::List { urls } => library::run_list(BUNDLED_AGENTS, library::AGENTS, urls),
        AgentAction::Show { name, url } => {
            library::run_show(BUNDLED_AGENTS, &name, library::AGENTS, url)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundle_is_non_empty() {
        assert!(
            !BUNDLED_AGENTS.is_empty(),
            "build.rs should embed the agents/ directory"
        );
    }

    #[test]
    fn bundle_includes_roba_runner_doc() {
        let found = BUNDLED_AGENTS
            .iter()
            .any(|(_, rel, _)| *rel == "roba-runner/AGENT.md");
        assert!(found, "expected roba-runner/AGENT.md in the bundle");
    }
}
