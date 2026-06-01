//! Session continuation and permission preset application.
//!
//! These are small mutators on a [`QueryCommand`] -- they translate
//! [`AskArgs`] flags (`-c`, `--resume`, `--fork`, `--readonly`,
//! `--full-auto`) into the matching builder method calls.

use claude_wrapper::{QueryCommand, RetryPolicy};

use crate::cli::AskArgs;

/// Apply session-related flags (-c, --resume, --fork), the model
/// override (--model), and then permission-related flags. Returns
/// the configured QueryCommand.
pub fn apply_session(mut cmd: QueryCommand, args: &AskArgs) -> QueryCommand {
    if args.continue_session {
        cmd = cmd.continue_session();
    }
    if let Some(id) = &args.resume {
        cmd = cmd.resume(id.clone());
    }
    if args.fork {
        cmd = cmd.fork_session();
    }
    if let Some(m) = &args.model {
        cmd = cmd.model(m.clone());
    }
    if let Some(name) = &args.worktree {
        cmd = match name {
            Some(n) => cmd.worktree_named(n.clone()),
            None => cmd.worktree(),
        };
    }
    if args.show_thinking {
        cmd = cmd.include_partial_messages();
    }
    if args.no_retry {
        // Force a per-command retry policy of a single attempt. This
        // overrides any client-level default (the wrapper resolves
        // per-command policy first), so a transient failure surfaces
        // immediately instead of being silently re-tried with backoff.
        // `max_attempts(1)` means "no retries."
        cmd = cmd.retry(RetryPolicy::new().max_attempts(1));
    }
    apply_permissions(cmd, args)
}

/// Apply permission policy.
///
/// The default behavior is "readonly": claude can use Read, Glob,
/// and Grep but nothing else. To open more up, layer additions:
///
/// - `--readonly` -- explicit form of the default; no-op.
/// - `--writable` -- preset that adds Edit + Write.
/// - `--allow-tool` / profile `allow_tool` -- add specific tools or
///   patterns (e.g. `"Bash(git status)"`).
/// - `--deny-tool` / profile `deny_tool` -- block patterns. Applied
///   independently; useful with `--full-auto` to keep some teeth.
/// - `--full-auto` -- bypass everything (overrides above).
pub fn apply_permissions(mut cmd: QueryCommand, args: &AskArgs) -> QueryCommand {
    if args.full_auto {
        return cmd.dangerously_skip_permissions();
    }

    // Always-on safe defaults. --readonly is the explicit form;
    // either way these three are in the allow list.
    let mut allow: Vec<String> = vec!["Read".to_string(), "Glob".to_string(), "Grep".to_string()];
    if args.writable {
        push_unique(&mut allow, "Edit");
        push_unique(&mut allow, "Write");
    }
    for t in &args.allow_tool {
        push_unique(&mut allow, t);
    }
    cmd = cmd.allowed_tools(allow);

    if !args.deny_tool.is_empty() {
        cmd = cmd.disallowed_tools(args.deny_tool.clone());
    }
    cmd
}

fn push_unique(list: &mut Vec<String>, item: &str) {
    if !list.iter().any(|s| s == item) {
        list.push(item.to_string());
    }
}

/// Derive a display name from the resolved prompt for `claude
/// --name`. Shows up in the `claude --resume` picker, the terminal
/// title, and the prompt box. Without it, sessions roba creates
/// are effectively invisible to the picker.
///
/// Shape: `roba: <first 40 chars of the first non-empty line>`. The
/// `roba:` prefix makes sessions distinguishable from interactive
/// Claude Code sessions in the same project's history.
pub fn derive_session_name(prompt: &str) -> String {
    let first_line = prompt
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("");
    let preview: String = if first_line.chars().count() > 40 {
        let head: String = first_line.chars().take(40).collect();
        format!("{head}…")
    } else {
        first_line.to_string()
    };
    if preview.is_empty() {
        "roba".to_string()
    } else {
        format!("roba: {preview}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_short_prompt_passes_through() {
        assert_eq!(derive_session_name("hello"), "roba: hello");
    }

    #[test]
    fn name_truncates_at_40_chars_with_ellipsis() {
        let prompt = "this is a fairly long prompt that should get cut off somewhere";
        let name = derive_session_name(prompt);
        assert!(name.starts_with("roba: "), "got: {name}");
        assert!(name.ends_with('…'), "got: {name}");
        let body = name.trim_start_matches("roba: ").trim_end_matches('…');
        assert_eq!(body.chars().count(), 40, "got body: {body:?}");
    }

    #[test]
    fn name_uses_first_nonempty_line() {
        let prompt = "\n\n   \nthe real prompt\nignored continuation";
        assert_eq!(derive_session_name(prompt), "roba: the real prompt");
    }

    #[test]
    fn name_empty_prompt_falls_back_to_bare_roba() {
        assert_eq!(derive_session_name(""), "roba");
        assert_eq!(derive_session_name("   \n  \n"), "roba");
    }

    #[test]
    fn name_handles_unicode_correctly() {
        // 40 chars of CJK; truncation must use char boundaries, not byte
        let prompt = "あ".repeat(50);
        let name = derive_session_name(&prompt);
        // "roba: " + 40 "あ" + "…"
        let body = name.trim_start_matches("roba: ").trim_end_matches('…');
        assert_eq!(body.chars().count(), 40);
    }
}
