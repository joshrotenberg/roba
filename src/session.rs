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

/// Apply permission presets (--readonly, --full-auto) and the
/// explicit allow/deny lists. Composition:
///
/// - `--readonly` seeds the allow list with Read, Glob, Grep.
/// - `--allow-tool` / profile `allow_tools` add to that list.
/// - `--deny-tool` / profile `deny_tools` add to the deny list.
/// - `--full-auto` bypasses all checks; the lists become irrelevant.
pub fn apply_permissions(mut cmd: QueryCommand, args: &AskArgs) -> QueryCommand {
    let mut allow: Vec<String> = Vec::new();
    if args.readonly {
        allow.extend(["Read", "Glob", "Grep"].iter().map(|s| (*s).to_string()));
    }
    allow.extend(args.allow_tool.iter().cloned());
    if !allow.is_empty() {
        cmd = cmd.allowed_tools(allow);
    }
    if !args.deny_tool.is_empty() {
        cmd = cmd.disallowed_tools(args.deny_tool.clone());
    }
    if args.full_auto {
        cmd = cmd.dangerously_skip_permissions();
    }
    cmd
}
