//! Private mapping from provider-neutral Claude configuration to the wrapper.

use claude_wrapper::QueryCommand;

use crate::engine::{Config, Permissions, Session};

pub(crate) fn apply_session(mut command: QueryCommand, config: &Config) -> QueryCommand {
    if let Session::Resume(id) = &config.session {
        command = command.resume(id.clone());
    }
    if let Some(model) = &config.model {
        command = command.model(model.clone());
    }
    if let Some(effort) = config.effort {
        command = command.effort(effort);
    }
    if let Some(instructions) = &config.append_system_prompt {
        command = command.append_system_prompt(instructions.clone());
    }
    if let Some(max_turns) = config.max_turns {
        command = command.max_turns(max_turns);
    }
    if let Some(max_budget_usd) = config.max_budget_usd {
        command = command.max_budget_usd(max_budget_usd);
    }
    for path in &config.mcp_config {
        command = command.mcp_config(path.clone());
    }
    apply_permissions(command, config)
}

fn apply_permissions(mut command: QueryCommand, config: &Config) -> QueryCommand {
    if matches!(config.permissions, Permissions::FullAuto) {
        return command.dangerously_skip_permissions();
    }

    let mut allowed = vec!["Read".to_owned(), "Glob".to_owned(), "Grep".to_owned()];
    if matches!(config.permissions, Permissions::WorkspaceWrite) {
        push_unique(&mut allowed, "Edit");
        push_unique(&mut allowed, "Write");
    }
    for tool in &config.allow_tools {
        push_unique(&mut allowed, tool);
    }
    command = command.allowed_tools(allowed);

    if !config.deny_tools.is_empty() {
        command = command.disallowed_tools(config.deny_tools.clone());
    }
    command
}

fn push_unique(values: &mut Vec<String>, value: &str) {
    if !values.iter().any(|candidate| candidate == value) {
        values.push(value.to_owned());
    }
}

pub(crate) fn derive_session_name(prompt: &str) -> String {
    let first_line = prompt
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("");
    let preview = if first_line.chars().count() > 40 {
        let head: String = first_line.chars().take(40).collect();
        format!("{head}…")
    } else {
        first_line.to_owned()
    };
    if preview.is_empty() {
        "roba".to_owned()
    } else {
        format!("roba: {preview}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_only_defaults_and_exact_additions_are_applied_once() {
        let config = Config {
            allow_tools: vec!["Read".to_owned(), "mcp__roba__self".to_owned()],
            deny_tools: vec!["Bash".to_owned()],
            ..Config::new("inspect")
        };
        let debug = format!("{:?}", apply_session(QueryCommand::new("inspect"), &config));
        assert!(debug.contains("Read"), "{debug}");
        assert!(debug.contains("Glob"), "{debug}");
        assert!(debug.contains("Grep"), "{debug}");
        assert!(debug.contains("mcp__roba__self"), "{debug}");
        assert!(debug.contains("Bash"), "{debug}");
    }

    #[test]
    fn workspace_write_adds_edit_and_write() {
        let config = Config {
            permissions: Permissions::WorkspaceWrite,
            ..Config::new("edit")
        };
        let debug = format!("{:?}", apply_session(QueryCommand::new("edit"), &config));
        assert!(debug.contains("Edit"), "{debug}");
        assert!(debug.contains("Write"), "{debug}");
    }

    #[test]
    fn session_name_handles_unicode_on_character_boundaries() {
        let name = derive_session_name(&"あ".repeat(50));
        let body = name.trim_start_matches("roba: ").trim_end_matches('…');
        assert_eq!(body.chars().count(), 40);
    }
}
