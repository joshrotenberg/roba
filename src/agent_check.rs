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
use crate::cli::PermMode;

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

/// The permission knobs that determine the effective tool allowlist:
/// the three a posture's coverage actually depends on. Extracted so the
/// coverage check runs against either a live [`AskArgs`] (run time) or a
/// config profile/alias's resolved flags (lint time, via
/// [`crate::lint`]).
pub struct Posture {
    pub writable: bool,
    pub full_auto: bool,
    pub allow_tool: Vec<String>,
}

impl Posture {
    /// The posture implied by a resolved invocation.
    pub fn from_args(args: &AskArgs) -> Self {
        Self {
            writable: args.writable,
            full_auto: args.full_auto,
            allow_tool: args.allow_tool.clone(),
        }
    }
}

/// Build the effective allowlist from a [`Posture`].
/// Mirrors the logic in [`crate::session::apply_permissions`].
fn effective_allowlist(posture: &Posture) -> Vec<String> {
    let mut allow: Vec<String> = vec!["Read".to_string(), "Glob".to_string(), "Grep".to_string()];
    if posture.writable {
        if !allow.iter().any(|s| s == "Edit") {
            allow.push("Edit".to_string());
        }
        if !allow.iter().any(|s| s == "Write") {
            allow.push("Write".to_string());
        }
    }
    for t in &posture.allow_tool {
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

/// Return the subset of `declared` tools NOT covered by `posture`'s
/// effective allowlist. Empty when `full_auto` is set (all tools
/// covered). The posture-based entry point shared by the run-time
/// [`find_missing_tools`] and the lint-time check in [`crate::lint`].
pub fn missing_tools_for_posture(declared: &[String], posture: &Posture) -> Vec<String> {
    if posture.full_auto {
        return Vec::new();
    }
    let allowlist = effective_allowlist(posture);
    declared
        .iter()
        .filter(|tool| !is_covered(tool, &allowlist))
        .cloned()
        .collect()
}

/// Return the subset of `declared` tools NOT covered by the resolved
/// allowlist.
///
/// Returns an empty vec when `--full-auto` is set (all tools covered).
pub fn find_missing_tools(declared: &[String], args: &AskArgs) -> Vec<String> {
    missing_tools_for_posture(declared, &Posture::from_args(args))
}

/// Return true when a single tool implies file mutation:
/// `Edit`, `Write`, `MultiEdit`, or `Bash` (bare or granular `Bash(...)`).
fn is_write_tool(tool: &str) -> bool {
    matches!(tool, "Edit" | "Write" | "MultiEdit" | "Bash") || tool.starts_with("Bash(")
}

/// Return true when any declared tool implies file mutation:
/// `Edit`, `Write`, `MultiEdit`, or `Bash` (bare or granular `Bash(...)`).
pub fn declares_write_tools(declared: &[String]) -> bool {
    declared.iter().any(|t| is_write_tool(t))
}

/// Return true when the agent declares write tools AND those write tools are
/// **resolved into the effective allowlist** (granted via `--writable`,
/// `--allow-tool`, or `--full-auto`).
///
/// This is the real stall hazard (Key Lesson 11): a write tool that claude is
/// permitted to use but a default permission mode won't auto-approve, so the
/// first write attempt awaits approval and the dispatch hangs.
///
/// A *declared-but-unresolved* write tool cannot stall -- it is absent from the
/// allowlist, so claude simply works without it. Those surface in the
/// missing-tools warning instead (the two warnings are mutually exclusive). The
/// resolution mirrors [`find_missing_tools`]: a declared write tool counts as
/// resolved exactly when it is NOT in that missing set.
pub fn declares_resolved_write_tools(declared: &[String], args: &AskArgs) -> bool {
    let missing = find_missing_tools(declared, args);
    declared
        .iter()
        .filter(|t| is_write_tool(t))
        .any(|t| !missing.iter().any(|m| m == t))
}

/// Return true when the resolved args leave the **permission mode** at its
/// default -- the mode that prompts for (rather than auto-approves) writes.
///
/// This keys ONLY off `--permission-mode` (and `--full-auto`, which bypasses
/// permissions entirely). It deliberately does NOT consider `--writable`:
/// `--writable` grants write *tools* into the allowlist but sets no permission
/// mode, so under it a write still stalls in default mode -- that is precisely
/// the hazard the stall warning guards.
///
/// Permissive `--permission-mode` values (returns false):
///   AcceptEdits, Auto, BypassPermissions, DontAsk
///
/// Non-permissive (returns true, same as None):
///   Default, Plan
pub fn is_default_permission_mode(args: &AskArgs) -> bool {
    if args.full_auto {
        return false;
    }
    match args.permission_mode {
        None | Some(PermMode::Default) | Some(PermMode::Plan) => true,
        Some(PermMode::AcceptEdits)
        | Some(PermMode::Auto)
        | Some(PermMode::BypassPermissions)
        | Some(PermMode::DontAsk) => false,
    }
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
            "  hint: intentional? --no-agent-check suppresses; otherwise --allow-tool 'Bash(...)' or --full-auto"
        );
    }

    // Permission-mode check: warn only when the agent declares write tools that
    // are RESOLVED into the allowlist (e.g. via --writable) AND the permission
    // mode is default. That is the real stall hazard -- the first write awaits
    // approval. Unresolved write tools cannot stall (they surface in the
    // missing-tools warning above instead), so the two warnings are mutually
    // exclusive.
    if declares_resolved_write_tools(&declared, args) && is_default_permission_mode(args) {
        eprintln!(
            "[roba] warning: agent '{agent_name}' declares write tools (Edit/Write) but permission mode is default"
        );
        eprintln!("         -- dispatch will stall at first write attempt");
        eprintln!("         hint: add --full-auto or --permission-mode acceptEdits");
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

    // -- permission-mode check -----------------------------------------------

    #[test]
    fn write_tools_detected() {
        let declared = vec!["Edit".to_string(), "Write".to_string()];
        assert!(
            declares_write_tools(&declared),
            "Edit/Write should be detected as write tools"
        );
        let args = args_from(&[]);
        assert!(is_default_permission_mode(&args), "no flags = default mode");
        // But with no grant the write tools are unresolved, so no stall (see
        // stall_false_positive_under_readonly).
    }

    #[test]
    fn agent_check_no_warn_when_full_auto() {
        let declared = vec!["Edit".to_string(), "Write".to_string()];
        assert!(declares_write_tools(&declared));
        let args = args_from(&["--full-auto"]);
        assert!(
            !is_default_permission_mode(&args),
            "--full-auto should not be default mode"
        );
    }

    #[test]
    fn stall_false_positive_under_readonly() {
        // The #264 repro: a write-declaring agent run read-only. The write
        // tools are NOT resolved (absent from the allowlist), so there is no
        // stall hazard -- they surface in the missing-tools warning instead.
        let declared = vec!["Edit".to_string(), "Write".to_string()];
        let args = args_from(&[]);
        assert!(is_default_permission_mode(&args), "no flags = default mode");
        assert!(
            !declares_resolved_write_tools(&declared, &args),
            "unresolved write tools must NOT count as a stall hazard"
        );
        // The stall predicate is therefore false even though the agent
        // declares writes and the mode is default.
        assert!(
            !(declares_resolved_write_tools(&declared, &args) && is_default_permission_mode(&args))
        );
    }

    #[test]
    fn stall_fires_when_writable_in_default_mode() {
        // --writable grants Edit/Write into the allowlist but sets no
        // permission mode, so the first write still awaits approval: stall.
        let declared = vec!["Edit".to_string(), "Write".to_string()];
        let args = args_from(&["--writable"]);
        assert!(
            is_default_permission_mode(&args),
            "--writable sets no permission mode -> still default mode"
        );
        assert!(
            declares_resolved_write_tools(&declared, &args),
            "--writable resolves Edit/Write into the allowlist"
        );
        assert!(
            declares_resolved_write_tools(&declared, &args) && is_default_permission_mode(&args)
        );
    }

    #[test]
    fn stall_silent_when_writable_and_accept_edits() {
        // --writable resolves the write tools, but accept-edits auto-approves
        // them, so there is no stall.
        let declared = vec!["Edit".to_string(), "Write".to_string()];
        let args = args_from(&["--writable", "--permission-mode", "accept-edits"]);
        assert!(
            declares_resolved_write_tools(&declared, &args),
            "--writable still resolves the write tools"
        );
        assert!(
            !is_default_permission_mode(&args),
            "accept-edits is a permissive permission mode"
        );
        assert!(
            !(declares_resolved_write_tools(&declared, &args) && is_default_permission_mode(&args))
        );
    }

    #[test]
    fn agent_check_no_warn_when_permission_mode_set() {
        let declared = vec!["Write".to_string()];
        assert!(declares_write_tools(&declared));
        // accept-edits is permissive (clap ValueEnum is kebab-case)
        let args = args_from(&["--permission-mode", "accept-edits"]);
        assert!(
            !is_default_permission_mode(&args),
            "--permission-mode accept-edits should not be default mode"
        );
        // auto is permissive
        let args2 = args_from(&["--permission-mode", "auto"]);
        assert!(!is_default_permission_mode(&args2));
        // dont-ask is permissive
        let args3 = args_from(&["--permission-mode", "dont-ask"]);
        assert!(!is_default_permission_mode(&args3));
    }

    #[test]
    fn agent_check_no_warn_for_readonly_agent() {
        let declared = vec!["Read".to_string(), "Glob".to_string()];
        assert!(
            !declares_write_tools(&declared),
            "Read/Glob are not write tools"
        );
        let args = args_from(&[]);
        assert!(is_default_permission_mode(&args));
        // Both conditions must be true to warn; readonly agent should not warn.
        assert!(
            !(declares_write_tools(&declared) && is_default_permission_mode(&args)),
            "readonly agent in default mode should not trigger the warning"
        );
    }
}
