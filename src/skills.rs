//! Bundled skill library + the `roba skill {install,list,show}`
//! subcommands.
//!
//! `BUNDLED_SKILLS` is generated at build time by `build.rs` (see
//! `OUT_DIR/bundled_skills.rs`): a `&[(name, relative_path, body)]`
//! array embedding every file under the repo's `skills/` directory.
//! The handlers delegate to [`crate::library`], which holds the logic
//! shared with the agent library.

use anyhow::Result;

use crate::cli::SkillAction;
use crate::library;

include!(concat!(env!("OUT_DIR"), "/bundled_skills.rs"));

/// Run a `roba skill <action>` subcommand.
pub fn run(action: SkillAction) -> Result<()> {
    match action {
        SkillAction::Install(args) => library::run_install(BUNDLED_SKILLS, args, library::SKILLS),
        SkillAction::List => library::run_list(BUNDLED_SKILLS, library::SKILLS),
        SkillAction::Show { name } => library::run_show(BUNDLED_SKILLS, &name, library::SKILLS),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundle_is_non_empty() {
        assert!(
            !BUNDLED_SKILLS.is_empty(),
            "build.rs should embed the skills/ directory"
        );
    }

    #[test]
    fn bundle_includes_draft_pr_first_doc() {
        let found = BUNDLED_SKILLS
            .iter()
            .any(|(_, rel, _)| *rel == "draft-pr-first/SKILL.md");
        assert!(found, "expected draft-pr-first/SKILL.md in the bundle");
    }
}
