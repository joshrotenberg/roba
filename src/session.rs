//! Session continuation and permission preset application.
//!
//! These are small mutators on a [`QueryCommand`] -- they translate
//! [`AskArgs`] flags (`-c`, `--resume`, `--fork`, `--readonly`,
//! `--full-auto`) into the matching builder method calls.

use claude_wrapper::QueryCommand;

use crate::cli::AskArgs;

/// Apply session-related flags (-c, --resume, --fork) and then
/// permission-related flags. Returns the configured QueryCommand.
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
    apply_permissions(cmd, args)
}

/// Apply permission policy.
///
/// The default behavior is "readonly": claude can use Read, Glob,
/// and Grep but nothing else. To open more up, layer additions:
///
/// - `--readonly` -- explicit form of the default; no-op.
/// - `--writable` -- preset that adds Edit + Write.
/// - `--allow-tool` / profile `allow_tools` -- add specific tools or
///   patterns (e.g. `"Bash(git status)"`).
/// - `--deny-tool` / profile `deny_tools` -- block patterns. Applied
///   independently; useful with `--full-auto` to keep some teeth.
/// - `--full-auto` -- bypass everything (overrides above).
pub fn apply_permissions(mut cmd: QueryCommand, args: &AskArgs) -> QueryCommand {
    if args.full_auto {
        return cmd.dangerously_skip_permissions();
    }

    // Always-on safe defaults. --readonly is the explicit form;
    // either way these three are in the allow list.
    let mut allow: Vec<String> = vec![
        "Read".to_string(),
        "Glob".to_string(),
        "Grep".to_string(),
    ];
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
