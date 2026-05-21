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

/// Apply permission presets (--readonly, --full-auto).
pub fn apply_permissions(mut cmd: QueryCommand, args: &AskArgs) -> QueryCommand {
    if args.readonly {
        cmd = cmd.allowed_tools(["Read", "Glob", "Grep"]);
    }
    if args.full_auto {
        cmd = cmd.dangerously_skip_permissions();
    }
    cmd
}
