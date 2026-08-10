//! `--detach` -- fire a run that survives the caller.
//!
//! roba prints a session handle (a UUID) and exits 0, after re-execing
//! itself disowned so the actual `claude` run outlives this process. The
//! parent owns NOTHING afterward: no supervisor, no socket, no resume
//! machinery. Observation is via `roba show --wait` / `--trace`; the run's
//! state lives in claude's own session records. This is `nohup` baked in,
//! not a daemon (the scope line: roba owns no runtime state).
//!
//! The internal `run_detached` flow:
//! 1. Require an explicit prompt source -- the child can't read this
//!    process's stdin (it's redirected to /dev/null), so a promptless
//!    invocation would fail into the void. Reject it up front, then reject
//!    only stdin that carries data we'd lose (a pipe with bytes, a non-empty
//!    `< file` redirect) -- NOT a benign non-TTY caller (a closed/EOF pipe,
//!    /dev/null), so an orchestrator firing `roba --detach -f task.md`
//!    without a TTY is not blocked. See `stdin_would_lose_data`.
//! 2. Preflight the claude binary (reusing `doctor`'s check) so a
//!    dead-on-arrival child behind a printed handle is an error, not silence.
//! 3. Resolve the handle: a caller-supplied `--session-id` / `-c=ID` /
//!    `--session NAME`, else a freshly minted v4 UUID (which we inject as
//!    `--session-id` into the child so parent and child agree on it).
//! 4. Re-exec `current_exe()` with the raw argv minus `--detach`, in a new
//!    process group with all stdio detached. Drop the handle, never wait.
//! 5. Print the handle to stdout (the answer) and a re-attach hint to
//!    stderr, plus a rails nudge when no turn/budget cap is set.

use anyhow::{Result, bail};
use std::process::{Command, Stdio};

use crate::cli::AskArgs;

/// Run the detached-spawn flow and return once the child is launched.
///
/// Called from [`crate::run_ask`] AFTER the env/profile merge (so the
/// rails-nudge predicate sees resolved `--max-turns` / `--max-budget-usd`)
/// and BEFORE prompt resolution (the child re-resolves the prompt itself).
pub(crate) fn run_detached(
    args: &AskArgs,
    mut bundle_snapshot: Option<crate::bundle::DetachedBundleSnapshot>,
) -> Result<()> {
    // (1) Promptless guard FIRST. The detached child re-resolves its own
    // prompt; it can only do so from an explicit source (positional / -p /
    // -f). `-e`/`--editor` is a clap conflict, and stdin is unavailable to
    // the child, so those are not options. Checking this before the
    // stdin-TTY guard keeps both failure messages reachable: a promptless
    // invocation reports "needs a prompt", a prompted-but-piped one reports
    // "can't read piped stdin".
    if args.prompt.is_none() && args.prompt_flag.is_none() && args.file.is_none() {
        bail!(
            "--detach needs an explicit prompt: pass one as an argument, with -p, or with -f \
             (the detached run can't read this shell's stdin)"
        );
    }

    // (1, cont.) Piped-DATA guard. The child's stdin is /dev/null, so real
    // input on this process's stdin would silently vanish. Block exactly
    // that -- bytes we'd lose -- while letting benign non-TTY stdin through
    // (a closed/EOF pipe, /dev/null, an agent's null stdin), so an
    // orchestrator can fire `roba --detach -f task.md` without a TTY. The
    // unix path peeks for actual bytes; Windows is conservative and blocks
    // any pipe on stdin (see `stdin_would_lose_data`). An unexpected
    // classification error proceeds rather than block a caller on a stat
    // hiccup -- the common loss cases (a pipe with bytes, a non-empty
    // redirect) classify cleanly.
    if crate::stdin_probe::stdin_would_lose_data().unwrap_or(false) {
        bail!(
            "--detach cannot read piped stdin (the detached run's stdin is /dev/null); \
             pass the input with -f or --prepend instead"
        );
    }

    // (2) Preflight: the claude binary must resolve, or the detached child
    // dies on arrival behind a handle we already printed (silence). Reuse
    // doctor's claude check so the two agree on what "resolves" means.
    if !crate::doctor::claude_on_path() {
        bail!(
            "--detach: claude binary not found on PATH; refusing to spawn a detached run that \
             would die on arrival (install claude-code: https://github.com/anthropics/claude-code)"
        );
    }

    // (3) Resolve the handle the detached run will use, and whether we mint
    // and inject a fresh `--session-id`.
    let (handle, mint) = resolve_handle(args)?;

    // (4) Build the child argv and spawn it disowned.
    let exe = std::env::current_exe()
        .map_err(|e| anyhow::anyhow!("--detach: cannot locate the roba binary to re-exec: {e}"))?;
    let mut child_args = detached_argv(std::env::args().skip(1), mint.then_some(handle.as_str()));
    if let Some(snapshot) = &bundle_snapshot {
        child_args = with_bundle_snapshot(child_args, snapshot.path())?;
    }

    let mut cmd = Command::new(exe);
    cmd.args(child_args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // (4a) Hand the child its receipt path. The child -- not this process --
    // writes both the start and the terminal record, so a fast child can
    // never have its outcome clobbered by a late write from here (#441).
    // Env, not argv: the re-exec is raw surgery on the user's own tokens, and
    // this belongs nowhere near the clap surface. Its presence is also how
    // the child knows roba detached it, so a receipt is written for detached
    // runs only. A receipt path we cannot resolve just means no receipt --
    // `show` then behaves exactly as it did before.
    if let Some(path) = crate::receipt::path_for(&handle) {
        cmd.env(crate::receipt::RECEIPT_ENV, path);
    }
    detach_process_group(&mut cmd);
    cmd.spawn()
        .map_err(|e| anyhow::anyhow!("--detach: failed to spawn the detached run: {e}"))?;
    if let Some(snapshot) = &mut bundle_snapshot {
        snapshot.transfer_to_child();
    }
    // Drop the child handle without waiting -- the run is on its own now.

    // (5) Emit the handle (stdout = the answer) and metadata (stderr).
    if rails_nudge_needed(args) {
        eprintln!(
            "warning: detached run has no --max-turns / --max-budget-usd / --timeout cap; nothing is watching it"
        );
    }
    eprintln!("re-attach: roba show {handle} --wait");
    println!("{handle}");
    Ok(())
}

fn with_bundle_snapshot(args: Vec<String>, snapshot: &std::path::Path) -> Result<Vec<String>> {
    let snapshot = snapshot.to_str().ok_or_else(|| {
        anyhow::anyhow!(
            "detached bundle snapshot path is not valid UTF-8: {}",
            snapshot.display()
        )
    })?;
    let mut filtered = Vec::with_capacity(args.len() + 2);
    let mut iter = args.into_iter().peekable();
    let mut past_separator = false;
    while let Some(argument) = iter.next() {
        if !past_separator && argument == "--" {
            past_separator = true;
            filtered.push(argument);
            continue;
        }
        if !past_separator && argument == "--bundle" {
            let _ = iter.next();
            continue;
        }
        if !past_separator && argument.starts_with("--bundle=") {
            continue;
        }
        filtered.push(argument);
    }
    filtered.insert(0, snapshot.to_string());
    filtered.insert(0, "--bundle".to_string());
    Ok(filtered)
}

/// Resolve the session handle to print, and whether roba minted it (and so
/// must inject `--session-id <handle>` into the child argv).
///
/// - `--session-id ID` / `-c=ID` / `--session NAME` (the latter already
///   folded into `continue_session` as `Some(Some(uuid))` by `run_ask`):
///   reuse the known id, do NOT inject (the selector is already in argv).
/// - bare `-c` (continue most recent): there is no id to print before the
///   run starts -- error rather than print a handle we can't honor.
/// - `--fork`: the forked session gets a NEW id not known until the run
///   starts, so the parent id we have is the wrong handle -- error.
/// - otherwise: mint a fresh v4 UUID and inject it.
fn resolve_handle(args: &AskArgs) -> Result<(String, bool)> {
    if args.fork {
        bail!(
            "--detach cannot be combined with --fork: the forked session's id is not known until \
             the run starts, so there is no handle to print"
        );
    }
    if let Some(id) = &args.session_id {
        return Ok((id.clone(), false));
    }
    match &args.continue_session {
        Some(Some(id)) => Ok((id.clone(), false)),
        Some(None) => bail!(
            "--detach with bare -c can't pre-mint a handle (the most-recent session's id isn't \
             known yet); use -c=ID, --session NAME, or --session-id, or drop -c for a fresh \
             detached run"
        ),
        None => Ok((mint_uuid(), true)),
    }
}

/// Mint a fresh v4 UUID for a new detached session handle.
fn mint_uuid() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// Rebuild the child argv for the detached re-exec.
///
/// Drops every `--detach` token that was parsed as the flag -- i.e. only
/// those before a `--` end-of-options separator, so a literal `--detach`
/// appearing in the prompt after `--` survives verbatim. Everything else is
/// preserved as-is. When `inject_session_id` is `Some`, a freshly minted
/// `--session-id <uuid>` is prepended (before any positional / `--`), so it
/// can't be swallowed by an end-of-options separator.
fn detached_argv<I>(raw: I, inject_session_id: Option<&str>) -> Vec<String>
where
    I: IntoIterator<Item = String>,
{
    let mut out: Vec<String> = Vec::new();
    if let Some(id) = inject_session_id {
        out.push("--session-id".to_string());
        out.push(id.to_string());
    }
    let mut past_separator = false;
    for tok in raw {
        if !past_separator && tok == "--" {
            past_separator = true;
            out.push(tok);
            continue;
        }
        if !past_separator && tok == "--detach" {
            continue; // drop the flag itself
        }
        out.push(tok);
    }
    out
}

/// True when the detached run has no turn, budget, or wall-clock cap.
/// Nobody watches a detached run, so the guardrails matter more, not
/// less -- one nudge. Any one of the three caps silences it.
fn rails_nudge_needed(args: &AskArgs) -> bool {
    args.max_turns.is_none() && args.max_budget_usd.is_none() && args.timeout.is_none()
}

/// Put the spawned child in its own process group / detached session so it
/// survives the parent's exit and is not killed by signals delivered to the
/// parent's group.
#[cfg(unix)]
fn detach_process_group(cmd: &mut Command) {
    use std::os::unix::process::CommandExt;
    // process_group(0) -> the child leads a new process group, so it is not
    // in the parent's group and won't receive the parent's job-control
    // signals (e.g. the SIGHUP/SIGINT that would reach a foreground group).
    cmd.process_group(0);
}

/// Windows counterpart: detach the console and start a new process group so
/// the child outlives the launching process.
#[cfg(windows)]
fn detach_process_group(cmd: &mut Command) {
    use std::os::windows::process::CommandExt;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::Cli;
    use clap::Parser;

    fn argv(args: &[&str]) -> Vec<String> {
        args.iter().map(|s| s.to_string()).collect()
    }

    fn ask(args: &[&str]) -> AskArgs {
        Cli::try_parse_from(args).unwrap().ask
    }

    // -- detached_argv: argv surgery --------------------------------------

    #[test]
    fn strips_detach_token() {
        let out = detached_argv(argv(&["--detach", "prompt"]), None);
        assert_eq!(out, vec!["prompt".to_string()]);
    }

    #[test]
    fn detached_bundle_replaces_original_before_the_prompt_boundary() {
        let snapshot = std::path::Path::new("/tmp/roba-bundle-snapshot");
        let args = with_bundle_snapshot(
            argv(&[
                "--model",
                "sonnet",
                "--bundle",
                "original",
                "--",
                "--bundle=literal prompt",
            ]),
            snapshot,
        )
        .unwrap();
        assert_eq!(
            args,
            argv(&[
                "--bundle",
                "/tmp/roba-bundle-snapshot",
                "--model",
                "sonnet",
                "--",
                "--bundle=literal prompt",
            ])
        );

        let args = with_bundle_snapshot(argv(&["--bundle=original", "prompt"]), snapshot).unwrap();
        assert_eq!(
            args,
            argv(&["--bundle", "/tmp/roba-bundle-snapshot", "prompt"])
        );
    }

    #[test]
    fn strips_detach_anywhere_in_argv() {
        let out = detached_argv(
            argv(&["--model", "haiku", "--detach", "--writable", "prompt"]),
            None,
        );
        assert_eq!(
            out,
            vec![
                "--model".to_string(),
                "haiku".to_string(),
                "--writable".to_string(),
                "prompt".to_string(),
            ]
        );
    }

    #[test]
    fn preserves_everything_else_verbatim() {
        let raw = argv(&[
            "-C",
            "/repo",
            "--profile",
            "worker",
            "-f",
            "task.md",
            "--trace",
            "/tmp/t.jsonl",
            "--detach",
        ]);
        let out = detached_argv(raw, None);
        assert_eq!(
            out,
            vec![
                "-C",
                "/repo",
                "--profile",
                "worker",
                "-f",
                "task.md",
                "--trace",
                "/tmp/t.jsonl",
            ]
            .into_iter()
            .map(String::from)
            .collect::<Vec<_>>()
        );
    }

    #[test]
    fn injects_session_id_when_minted() {
        let out = detached_argv(argv(&["--detach", "prompt"]), Some("abc-uuid"));
        assert_eq!(
            out,
            vec![
                "--session-id".to_string(),
                "abc-uuid".to_string(),
                "prompt".to_string(),
            ]
        );
    }

    #[test]
    fn does_not_inject_session_id_when_not_minted() {
        let out = detached_argv(
            argv(&["--detach", "--session-id", "given-uuid", "prompt"]),
            None,
        );
        assert_eq!(
            out,
            vec![
                "--session-id".to_string(),
                "given-uuid".to_string(),
                "prompt".to_string(),
            ]
        );
    }

    #[test]
    fn keeps_literal_detach_after_separator() {
        // A `--detach` that appears AFTER `--` is the prompt, not the flag,
        // and must survive verbatim. The flag before `--` is still dropped.
        let out = detached_argv(argv(&["--detach", "--", "--detach"]), None);
        assert_eq!(out, vec!["--".to_string(), "--detach".to_string()]);
    }

    #[test]
    fn injected_session_id_precedes_separator() {
        // The injected --session-id must land before any `--` so it is
        // parsed as a flag, not swallowed as a positional.
        let out = detached_argv(argv(&["--detach", "--", "literal prompt"]), Some("u"));
        assert_eq!(
            out,
            vec![
                "--session-id".to_string(),
                "u".to_string(),
                "--".to_string(),
                "literal prompt".to_string(),
            ]
        );
    }

    // -- resolve_handle ----------------------------------------------------

    #[test]
    fn handle_uses_given_session_id_without_minting() {
        let args = ask(&[
            "roba",
            "--detach",
            "--session-id",
            "11111111-1111-4111-8111-111111111111",
            "prompt",
        ]);
        let (handle, mint) = resolve_handle(&args).unwrap();
        assert_eq!(handle, "11111111-1111-4111-8111-111111111111");
        assert!(!mint, "a given --session-id must not be re-minted");
    }

    #[test]
    fn handle_mints_a_uuid_for_a_fresh_run() {
        let args = ask(&["roba", "--detach", "prompt"]);
        let (handle, mint) = resolve_handle(&args).unwrap();
        assert!(mint, "a fresh detached run mints + injects a handle");
        // v4 UUID shape: 36 chars, 4 dashes.
        assert_eq!(handle.len(), 36, "got: {handle}");
        assert_eq!(handle.matches('-').count(), 4, "got: {handle}");
    }

    #[test]
    fn handle_uses_explicit_continue_id() {
        let args = ask(&["roba", "--detach", "-c=session-xyz", "prompt"]);
        let (handle, mint) = resolve_handle(&args).unwrap();
        assert_eq!(handle, "session-xyz");
        assert!(!mint);
    }

    #[test]
    fn handle_bare_continue_errors() {
        // Bare `-c` (continue most recent) followed by an explicit `-p`
        // prompt: most-recent has no pre-known id, so the handle can't be
        // minted. (A space-separated word after `-c` is consumed as the id,
        // hence `-p` here for a genuinely valueless `-c`.)
        let args = ask(&["roba", "--detach", "-c", "-p", "prompt"]);
        assert!(matches!(args.continue_session, Some(None)));
        assert!(resolve_handle(&args).is_err());
    }

    // -- rails_nudge_needed ------------------------------------------------

    #[test]
    fn nudge_when_no_caps() {
        let args = ask(&["roba", "--detach", "prompt"]);
        assert!(rails_nudge_needed(&args));
    }

    #[test]
    fn no_nudge_with_max_turns() {
        let args = ask(&["roba", "--detach", "--max-turns", "10", "prompt"]);
        assert!(!rails_nudge_needed(&args));
    }

    #[test]
    fn no_nudge_with_max_budget() {
        let args = ask(&["roba", "--detach", "--max-budget-usd", "5", "prompt"]);
        assert!(!rails_nudge_needed(&args));
    }
}
