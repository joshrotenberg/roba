//! `roba worktree list` -- read-only enumeration of the repo's git
//! worktrees.
//!
//! Thin CLI over `claude_wrapper::worktrees::WorktreeRoot`, which shells
//! to `git worktree list --porcelain`. Read-only on purpose: no prune,
//! no remove, no add -- mutating worktrees is git's (or claude's) job,
//! not roba's.
//!
//! The output is a SUPERSET of the worktrees claude's `--worktree` flag
//! creates: `git worktree list` reports *every* worktree for the repo
//! (user-made + claude-created), not just the ones under
//! `.claude/worktrees/`.

use anyhow::{Context, Result};

use crate::cli::{WorktreeCmd, WorktreeListArgs};

/// Dispatch the `roba worktree` subcommand.
pub fn run(cmd: WorktreeCmd) -> Result<()> {
    match cmd {
        WorktreeCmd::List(args) => run_list(args),
    }
}

/// `roba worktree list`: enumerate the repo's git worktrees.
fn run_list(args: WorktreeListArgs) -> Result<()> {
    use claude_wrapper::worktrees::WorktreeRoot;

    // `-C/--cwd` (a global flag) already changed the process cwd in
    // `dispatch`, so the current directory is the repo to inspect.
    let dir = std::env::current_dir().context("resolving current directory")?;
    let worktrees = WorktreeRoot::for_repo(&dir)
        .list()
        .context("listing git worktrees")?;

    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&crate::VersionedResult::new(&worktrees))?
        );
        return Ok(());
    }

    if worktrees.is_empty() {
        eprintln!("no worktrees found");
        return Ok(());
    }

    println!("{:<48} {:<24} {:<8}  FLAGS", "PATH", "BRANCH", "HEAD");
    for wt in &worktrees {
        let path = wt.path.display().to_string();
        let branch = branch_label(wt);
        let head = wt.head.as_deref().map(short_head).unwrap_or("-");
        let flags = flags_label(wt);
        println!("{path:<48} {branch:<24} {head:<8}  {flags}");
    }
    Ok(())
}

/// The branch column: the branch name, or a bracketed state when there
/// isn't one (detached HEAD or a bare worktree).
fn branch_label(wt: &claude_wrapper::worktrees::Worktree) -> &str {
    if let Some(branch) = wt.branch.as_deref() {
        branch
    } else if wt.is_bare {
        "(bare)"
    } else if wt.is_detached {
        "(detached)"
    } else {
        "-"
    }
}

/// First 7 chars of a HEAD sha (git's conventional short form), or the
/// whole thing if it's somehow shorter.
fn short_head(head: &str) -> &str {
    head.get(..7).unwrap_or(head)
}

/// Space-joined status markers for the FLAGS column.
fn flags_label(wt: &claude_wrapper::worktrees::Worktree) -> String {
    let mut flags: Vec<&str> = Vec::new();
    if wt.is_main {
        flags.push("[main]");
    }
    if wt.is_locked {
        flags.push("[locked]");
    }
    if wt.is_prunable {
        flags.push("[prunable]");
    }
    flags.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use claude_wrapper::worktrees::Worktree;
    use std::path::PathBuf;

    fn wt() -> Worktree {
        Worktree {
            path: PathBuf::from("/repo/main"),
            head: Some("abcdef0123456789".to_string()),
            branch: Some("main".to_string()),
            is_main: false,
            is_detached: false,
            is_bare: false,
            is_locked: false,
            lock_reason: None,
            is_prunable: false,
            prune_reason: None,
        }
    }

    #[test]
    fn short_head_takes_first_seven() {
        assert_eq!(short_head("abcdef0123456789"), "abcdef0");
    }

    #[test]
    fn short_head_passes_through_short_input() {
        assert_eq!(short_head("abc"), "abc");
    }

    #[test]
    fn branch_label_uses_branch_name() {
        assert_eq!(branch_label(&wt()), "main");
    }

    #[test]
    fn branch_label_detached() {
        let mut w = wt();
        w.branch = None;
        w.is_detached = true;
        assert_eq!(branch_label(&w), "(detached)");
    }

    #[test]
    fn branch_label_bare() {
        let mut w = wt();
        w.branch = None;
        w.is_bare = true;
        assert_eq!(branch_label(&w), "(bare)");
    }

    #[test]
    fn flags_label_marks_main() {
        let mut w = wt();
        w.is_main = true;
        assert_eq!(flags_label(&w), "[main]");
    }

    #[test]
    fn flags_label_combines_markers() {
        let mut w = wt();
        w.is_main = true;
        w.is_locked = true;
        w.is_prunable = true;
        assert_eq!(flags_label(&w), "[main] [locked] [prunable]");
    }

    #[test]
    fn flags_label_empty_for_plain_worktree() {
        assert_eq!(flags_label(&wt()), "");
    }
}
