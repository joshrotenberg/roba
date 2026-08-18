use crate::{CatalogDefinition, CatalogSource, PromptArgumentDefinition};

/// Return the shipped definition source of truth.
pub fn builtin_definitions() -> Vec<CatalogDefinition> {
    vec![
        CatalogDefinition::Agent {
            id: "roba.repo-worker".to_owned(),
            description: "A bounded worker for one repository-scoped goal.".to_owned(),
            source: inline(
                "You are a Roba-managed repository worker. Work on one bounded goal in the configured workspace. Inspect declared context and live resources before acting. Keep changes scoped, preserve unrelated work, validate the result, and report completed work and blockers. MCP capability availability does not grant authority beyond the operation policy.",
            ),
            default_skills: vec!["roba.repository-change".to_owned()],
        },
        CatalogDefinition::Skill {
            id: "roba.repository-change".to_owned(),
            description: "A conservative method for changing a source repository.".to_owned(),
            source: inline(
                "For repository changes: inspect current state and contribution guidance; identify the smallest coherent change; preserve unrelated work; add focused tests; run the repository-required gates; and report changed files, validation, and unresolved blockers. Do not commit, push, comment, merge, or release unless the operation explicitly authorizes that external action.",
            ),
        },
        CatalogDefinition::Prompt {
            id: "roba.issue-worker".to_owned(),
            description: "Implement one bounded repository issue.".to_owned(),
            source: inline(
                "Work on issue {{issue}} in this repository. Read the issue and current repository state, confirm the bounded scope, implement one coherent solution, run the required validation, and report the result or the exact blocker.",
            ),
            requires: vec!["roba.repository-change".to_owned()],
            arguments: vec![PromptArgumentDefinition {
                name: "issue".to_owned(),
                description: "Issue number or stable issue URL.".to_owned(),
                required: true,
                default: None,
            }],
        },
    ]
}

fn inline(content: &str) -> CatalogSource {
    CatalogSource::Inline {
        content: content.to_owned(),
    }
}
