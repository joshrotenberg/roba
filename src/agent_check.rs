//! Agent frontmatter permission check.
//!
//! When `--agent NAME` is set, roba looks up the agent's `AGENT.md`
//! in Claude Code's standard paths, parses the `tools:` field from
//! the YAML frontmatter, and warns on stderr if any declared tools
//! are not covered by the resolved allowlist.
//!
//! The check is **best-effort**: if the file is not found or the
//! frontmatter is unparseable, nothing is emitted and dispatch
//! proceeds normally. "Not found" is not a warning -- that's a
//! misconfiguration the user already knows about.

use std::path::{Path, PathBuf};

use crate::cli::AskArgs;

// ---------------------------------------------------------------------------
// Agent file lookup
// ---------------------------------------------------------------------------

/// Try to find the agent file for `name` in Claude Code's standard
/// lookup order:
///
/// 1. `<cwd>/.claude/agents/<name>/AGENT.md`
/// 2. `<cwd>/.claude/agents/<name>.md`
/// 3. `~/.claude/agents/<name>/AGENT.md`
/// 4. `~/.claude/agents/<name>.md`
///
/// Returns `None` when no file is found.
pub fn find_agent_file(name: &str, cwd: &Path) -> Option<PathBuf> {
    let cwd_candidates = [
        cwd.join(format!(".claude/agents/{name}/AGENT.md")),
        cwd.join(format!(".claude/agents/{name}.md")),
    ];
    for path in &cwd_candidates {
        if path.exists() {
            return Some(path.clone());
        }
    }
    if let Some(home) = home_dir() {
        let home_candidates = [
            home.join(format!(".claude/agents/{name}/AGENT.md")),
            home.join(format!(".claude/agents/{name}.md")),
        ];
        for path in &home_candidates {
            if path.exists() {
                return Some(path.clone());
            }
        }
    }
    None
}

fn home_dir() -> Option<PathBuf> {
    if let Ok(h) = std::env::var("HOME")
        && !h.is_empty()
    {
        return Some(PathBuf::from(h));
    }
    if let Ok(h) = std::env::var("USERPROFILE")
        && !h.is_empty()
    {
        return Some(PathBuf::from(h));
    }
    None
}

// ---------------------------------------------------------------------------
// Frontmatter parser
// ---------------------------------------------------------------------------

/// Parse the `tools:` field from YAML frontmatter in an agent file.
///
/// Two valid shapes:
///
/// Inline list:
/// ```text
/// tools: Read, Edit, Write, Bash
/// ```
///
/// YAML list:
/// ```text
/// tools:
///   - Read
///   - Edit
/// ```
///
/// Returns `None` when:
/// - No frontmatter is present (no leading `---`)
/// - No `tools:` field in the frontmatter
/// - The frontmatter is unparseable
///
/// This is best-effort: `None` means "couldn't determine", not "no tools."
pub fn parse_tools(content: &str) -> Option<Vec<String>> {
    let rest = content.strip_prefix("---\n")?;
    let fm_end = rest.find("\n---")?;
    let frontmatter = &rest[..fm_end];
    parse_tools_from_frontmatter(frontmatter)
}

fn parse_tools_from_frontmatter(frontmatter: &str) -> Option<Vec<String>> {
    let mut lines = frontmatter.lines().peekable();
    while let Some(line) = lines.next() {
        if let Some(rest) = line.strip_prefix("tools:") {
            let rest = rest.trim();
            if !rest.is_empty() {
                // Inline list: `tools: Read, Edit, Write`
                let tools: Vec<String> = rest
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                return if tools.is_empty() { None } else { Some(tools) };
            } else {
                // YAML list:
                // tools:
                //   - Read
                //   - Edit
                let mut tools = Vec::new();
                while let Some(next) = lines.peek() {
                    let trimmed = next.trim();
                    if let Some(item) = trimmed.strip_prefix("- ") {
                        tools.push(item.trim().to_string());
                        lines.next();
                    } else {
                        break;
                    }
                }
                return if tools.is_empty() { None } else { Some(tools) };
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Coverage check
// ---------------------------------------------------------------------------

/// Build the effective allowlist from the resolved `AskArgs`.
/// Mirrors the logic in [`crate::session::apply_permissions`].
fn effective_allowlist(args: &AskArgs) -> Vec<String> {
    let mut allow: Vec<String> = vec!["Read".to_string(), "Glob".to_string(), "Grep".to_string()];
    if args.writable {
        if !allow.iter().any(|s| s == "Edit") {
            allow.push("Edit".to_string());
        }
        if !allow.iter().any(|s| s == "Write") {
            allow.push("Write".to_string());
        }
    }
    for t in &args.allow_tool {
        if !allow.iter().any(|s| s == t) {
            allow.push(t.clone());
        }
    }
    allow
}

/// Return true when the effective allowlist covers the declared tool.
///
/// Coverage rules:
/// - Exact match: `Bash` covers `Bash`
/// - Granular covers bare: `Bash(git:*)` covers `Bash` (any `TOOL(...)`
///   entry satisfies a bare `TOOL` declaration)
fn is_covered(tool: &str, allowlist: &[String]) -> bool {
    allowlist
        .iter()
        .any(|entry| entry == tool || entry.starts_with(&format!("{tool}(")))
}

/// Return the subset of `declared` tools NOT covered by the resolved
/// allowlist.
///
/// Returns an empty vec when `--full-auto` is set (all tools covered).
pub fn find_missing_tools(declared: &[String], args: &AskArgs) -> Vec<String> {
    if args.full_auto {
        return Vec::new();
    }
    let allowlist = effective_allowlist(args);
    declared
        .iter()
        .filter(|tool| !is_covered(tool, &allowlist))
        .cloned()
        .collect()
}

// ---------------------------------------------------------------------------
// Warning emission
// ---------------------------------------------------------------------------

/// Check whether the agent's declared tools are covered by the
/// resolved allowlist and emit a warning to stderr if not.
///
/// Does nothing when:
/// - `--agent` is not set
/// - `--full-auto` is set (all tools covered)
/// - `--quiet` is set (metadata suppressed)
/// - `--no-agent-check` is set (explicitly suppressed)
/// - The agent file isn't found (silent -- misconfiguration the user
///   already knows about)
/// - The frontmatter has no `tools:` field (nothing to check)
pub fn maybe_warn(args: &AskArgs, cwd: &Path) {
    let agent_name = match &args.agent {
        Some(n) => n,
        None => return,
    };
    if args.full_auto || args.quiet || args.no_agent_check {
        return;
    }
    let agent_path = match find_agent_file(agent_name, cwd) {
        Some(p) => p,
        None => return,
    };
    let content = match std::fs::read_to_string(&agent_path) {
        Ok(c) => c,
        Err(_) => return,
    };
    let declared = match parse_tools(&content) {
        Some(t) => t,
        None => return,
    };
    let missing = find_missing_tools(&declared, args);
    if !missing.is_empty() {
        let tools_str = missing.join(", ");
        eprintln!(
            "[roba] warning: agent '{agent_name}' declares tools not in the resolved allowlist: [{tools_str}]"
        );
        eprintln!(
            "  hint: pass --full-auto, --allow-tool 'Bash(...)', or --no-agent-check to suppress"
        );
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::Cli;
    use clap::Parser;

    fn args_from(extra: &[&str]) -> AskArgs {
        let mut argv = vec!["roba", "placeholder"];
        argv.extend_from_slice(extra);
        Cli::try_parse_from(&argv).unwrap().ask
    }

    // -- parse_tools ---------------------------------------------------------

    #[test]
    fn parse_tools_inline() {
        let content = "---\nname: My Agent\ntools: Read, Edit, Write, Bash\n---\n# body\n";
        let tools = parse_tools(content).unwrap();
        assert_eq!(
            tools,
            vec!["Read", "Edit", "Write", "Bash"]
                .into_iter()
                .map(String::from)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn parse_tools_yaml_list() {
        let content = "---\nname: My Agent\ntools:\n  - Read\n  - Edit\n  - Bash\n---\n";
        let tools = parse_tools(content).unwrap();
        assert_eq!(
            tools,
            vec!["Read", "Edit", "Bash"]
                .into_iter()
                .map(String::from)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn parse_tools_missing_field() {
        let content = "---\nname: My Agent\ndescription: no tools field here\n---\n";
        assert!(
            parse_tools(content).is_none(),
            "missing tools field should return None"
        );
    }

    #[test]
    fn parse_tools_malformed_frontmatter() {
        // No opening `---`; should return None gracefully.
        let content = "name: My Agent\ntools: Read\n";
        assert!(
            parse_tools(content).is_none(),
            "missing frontmatter should return None"
        );
    }

    #[test]
    fn parse_tools_no_closing_delimiter() {
        // Has opening `---` but no closing `---`; strip_prefix succeeds
        // but find("\n---") returns None.
        let content = "---\nname: My Agent\ntools: Read\n";
        assert!(
            parse_tools(content).is_none(),
            "unclosed frontmatter should return None"
        );
    }

    #[test]
    fn parse_tools_inline_trims_whitespace() {
        let content = "---\ntools:  Read ,  Bash , Write \n---\n";
        let tools = parse_tools(content).unwrap();
        assert_eq!(tools, vec!["Read", "Bash", "Write"]);
    }

    // -- find_missing_tools --------------------------------------------------

    #[test]
    fn tool_coverage_bash_satisfied_by_granular() {
        // A granular `Bash(git:*)` in allow_tool covers a bare `Bash`
        // declaration in the agent frontmatter.
        let args = args_from(&["--allow-tool", "Bash(git:*)"]);
        let declared = vec!["Bash".to_string()];
        let missing = find_missing_tools(&declared, &args);
        assert!(
            missing.is_empty(),
            "Bash(git:*) should cover declared Bash; got missing: {missing:?}"
        );
    }

    #[test]
    fn tool_coverage_edit_satisfied_by_writable() {
        let args = args_from(&["--writable"]);
        let declared = vec!["Edit".to_string(), "Write".to_string()];
        let missing = find_missing_tools(&declared, &args);
        assert!(
            missing.is_empty(),
            "--writable should cover Edit and Write; got missing: {missing:?}"
        );
    }

    #[test]
    fn tool_coverage_full_auto_covers_all() {
        let args = args_from(&["--full-auto"]);
        let declared = vec![
            "Bash".to_string(),
            "WebFetch".to_string(),
            "Edit".to_string(),
        ];
        let missing = find_missing_tools(&declared, &args);
        assert!(
            missing.is_empty(),
            "--full-auto should cover all tools; got missing: {missing:?}"
        );
    }

    #[test]
    fn tool_coverage_missing() {
        // Bash is declared but only Read/Glob/Grep are in the default
        // allowlist. No --allow-tool, no --writable, no --full-auto.
        let args = args_from(&[]);
        let declared = vec!["Bash".to_string()];
        let missing = find_missing_tools(&declared, &args);
        assert_eq!(
            missing,
            vec!["Bash"],
            "Bash should be missing from the default read-only allowlist"
        );
    }

    #[test]
    fn tool_coverage_builtin_trio_always_covered() {
        // Read, Glob, Grep are always covered even with no explicit flags.
        let args = args_from(&[]);
        let declared = vec!["Read".to_string(), "Glob".to_string(), "Grep".to_string()];
        let missing = find_missing_tools(&declared, &args);
        assert!(
            missing.is_empty(),
            "built-in trio should always be covered; got missing: {missing:?}"
        );
    }

    #[test]
    fn tool_coverage_exact_match_in_allow_tool() {
        let args = args_from(&["--allow-tool", "Bash"]);
        let declared = vec!["Bash".to_string()];
        let missing = find_missing_tools(&declared, &args);
        assert!(
            missing.is_empty(),
            "exact Bash in allow_tool should cover declared Bash"
        );
    }
}
