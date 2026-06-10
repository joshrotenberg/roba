//! `--detach` -- fire a run that survives the caller.
//!
//! roba prints a session handle (a UUID) and exits 0, after re-execing
//! itself disowned so the actual `claude` run outlives this process. The
//! parent owns NOTHING afterward: no supervisor, no socket, no resume
//! machinery. Observation is via `roba show --wait` / `--trace`; the run's
//! state lives in claude's own session records. This is `nohup` baked in,
//! not a daemon (the scope line: roba owns no runtime state).
//!
//! The flow ([`run_detached`]):
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
pub fn run_detached(args: &AskArgs) -> Result<()> {
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
    if stdin_would_lose_data().unwrap_or(false) {
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
    let child_args = detached_argv(std::env::args().skip(1), mint.then_some(handle.as_str()));

    let mut cmd = Command::new(exe);
    cmd.args(child_args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    detach_process_group(&mut cmd);
    cmd.spawn()
        .map_err(|e| anyhow::anyhow!("--detach: failed to spawn the detached run: {e}"))?;
    // Drop the child handle without waiting -- the run is on its own now.

    // (5) Emit the handle (stdout = the answer) and metadata (stderr).
    if rails_nudge_needed(args) {
        eprintln!(
            "warning: detached run has no --max-turns / --max-budget-usd cap; nothing is watching it"
        );
    }
    eprintln!("re-attach: roba show {handle} --wait");
    println!("{handle}");
    Ok(())
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

/// True when the detached run has no turn or budget cap. Nobody watches a
/// detached run, so the guardrails matter more, not less -- one nudge.
fn rails_nudge_needed(args: &AskArgs) -> bool {
    args.max_turns.is_none() && args.max_budget_usd.is_none()
}

/// Would the detached run silently lose data on this process's stdin?
///
/// The detached child's stdin is `/dev/null`, so anything pending on THIS
/// process's stdin can never reach it. We block exactly that -- real input
/// we'd drop -- while letting benign non-TTY stdin through (a TTY, /dev/null,
/// or a closed/EOF pipe from a spawner), so an orchestrator firing
/// `roba --detach -f task.md` with a null stdin is not rejected.
///
/// Classification (unix), by `fstat` mode:
/// - TTY                     -> `false` (interactive, nothing to lose)
/// - regular file (`< file`) -> `true` iff the file has bytes
/// - FIFO / pipe             -> poll (zero timeout) + one nonblocking 1-byte
///   read: readable-with-a-byte -> `true`; clean EOF -> `false`;
///   open-but-silent -> `true` (a writer exists; treat as data intent)
/// - char device (/dev/null) / socket / anything else / closed fd -> `false`
///
/// The single byte is read ONLY on the `true` (we're about to error and exit)
/// path, so the proceed path never consumes stdin. The Windows counterpart
/// (`cfg(not(unix))`) is coarser: it classifies by handle TYPE only (a pipe
/// is treated as data, a console / `NUL` is not) and never peeks.
#[cfg(unix)]
fn stdin_would_lose_data() -> std::io::Result<bool> {
    use std::os::unix::io::AsRawFd;
    fd_would_lose_data(std::io::stdin().as_raw_fd())
}

/// Classify an arbitrary fd for the data-loss check (the testable core of
/// [`stdin_would_lose_data`]).
#[cfg(unix)]
fn fd_would_lose_data(fd: std::os::unix::io::RawFd) -> std::io::Result<bool> {
    // A TTY is interactive -- there is nothing buffered to lose.
    if unsafe { libc::isatty(fd) } == 1 {
        return Ok(false);
    }

    // fstat to classify the fd.
    // SAFETY: an all-zero `libc::stat` is a valid out-param for `fstat` to
    // fill; we only read fields after a success return.
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstat(fd, &mut st) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    match st.st_mode & libc::S_IFMT {
        // A `< file` redirect: data to lose iff the file is non-empty.
        libc::S_IFREG => Ok(st.st_size > 0),
        // A pipe: distinguish pending data from a closed/EOF pipe.
        libc::S_IFIFO => fifo_has_pending_data(fd),
        // Char device (/dev/null), socket, or anything else: nothing to lose.
        _ => Ok(false),
    }
}

/// Poll a FIFO fd (zero timeout) and, if readable, do one 1-byte read to tell
/// pending data (`true`) from a closed/EOF pipe (`false`). An open pipe whose
/// writer is silent (not readable yet) is treated as data intent (`true`): a
/// writer exists and may send, and a detached child must not silently drop it.
#[cfg(unix)]
fn fifo_has_pending_data(fd: std::os::unix::io::RawFd) -> std::io::Result<bool> {
    let mut pfd = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    // Zero timeout -> never blocks.
    let n = unsafe { libc::poll(&mut pfd, 1, 0) };
    if n < 0 {
        return Err(std::io::Error::last_os_error());
    }
    if n == 0 {
        // Not readable right now: an open pipe whose writer hasn't sent yet.
        // Conservative -- treat as intent to send data we'd lose.
        return Ok(true);
    }
    // Readable: real data or EOF (POLLHUP). poll guaranteed readiness, so a
    // 1-byte read returns immediately (a byte, or 0 at EOF). The byte is
    // consumed only here, on the about-to-error path.
    let mut byte = [0u8; 1];
    let r = unsafe { libc::read(fd, byte.as_mut_ptr() as *mut libc::c_void, 1) };
    if r > 0 {
        Ok(true) // real pending data the child would lose
    } else if r == 0 {
        Ok(false) // clean EOF -- a closed pipe, nothing to lose
    } else {
        // poll said readable but read would block / errored: no settled data.
        // Don't block the caller on a transient (EAGAIN/EWOULDBLOCK -> ok).
        let err = std::io::Error::last_os_error();
        match err.raw_os_error() {
            Some(e) if e == libc::EAGAIN || e == libc::EWOULDBLOCK => Ok(false),
            _ => Err(err),
        }
    }
}

/// Windows piped-data classification.
///
/// Unlike the unix path (which peeks for actual bytes), Windows classifies
/// by handle TYPE only, via `GetFileType` on stdin:
/// - `FILE_TYPE_PIPE`  -> `true` (an anonymous/named pipe -- `echo ... |
///   roba --detach`; treat as data the child would lose, no peek)
/// - `FILE_TYPE_CHAR` (a console or `NUL`), a disk file, or an
///   unknown/erroring type -> `false` (proceed)
///
/// Pipes are classified with `PeekNamedPipe`, which reports available
/// bytes WITHOUT consuming them: data present -> error; an empty or
/// EOF'd pipe (a spawner's closed stdin -- the common orchestrator
/// shape) -> proceed. This matches the unix semantics; the only
/// asymmetry is an open-but-silent pipe (unix conservatively errors,
/// windows proceeds on zero available bytes).
#[cfg(not(unix))]
fn stdin_would_lose_data() -> std::io::Result<bool> {
    use std::os::windows::io::AsRawHandle;
    let handle = std::io::stdin().as_raw_handle();
    if !handle_is_pipe(handle) {
        return Ok(false);
    }
    Ok(pipe_has_data(handle))
}

/// True iff the pipe handle has bytes available to read, per
/// `PeekNamedPipe` (non-consuming). A broken pipe (writer closed, no
/// data -- the EOF case) proceeds; any other peek failure is treated
/// conservatively as data-risk.
#[cfg(not(unix))]
fn pipe_has_data(handle: std::os::windows::io::RawHandle) -> bool {
    const ERROR_BROKEN_PIPE: u32 = 109;
    // SAFETY: PeekNamedPipe with a null buffer only queries the byte
    // count; GetLastError reads thread-local state. Both are read-only
    // with respect to the pipe contents.
    unsafe extern "system" {
        fn PeekNamedPipe(
            handle: *mut core::ffi::c_void,
            buffer: *mut core::ffi::c_void,
            buffer_size: u32,
            bytes_read: *mut u32,
            total_bytes_avail: *mut u32,
            bytes_left_this_message: *mut u32,
        ) -> i32;
        fn GetLastError() -> u32;
    }
    let mut avail: u32 = 0;
    let ok = unsafe {
        PeekNamedPipe(
            handle,
            core::ptr::null_mut(),
            0,
            core::ptr::null_mut(),
            &mut avail,
            core::ptr::null_mut(),
        )
    };
    if ok == 0 {
        // Writer already closed with nothing buffered = EOF = benign.
        return unsafe { GetLastError() } != ERROR_BROKEN_PIPE;
    }
    avail > 0
}

/// True iff the Windows handle is an anonymous/named pipe
/// (`FILE_TYPE_PIPE`). The testable core of the Windows
/// [`stdin_would_lose_data`].
#[cfg(not(unix))]
fn handle_is_pipe(handle: std::os::windows::io::RawHandle) -> bool {
    // GetFileType (kernel32) returns the type of a file handle. We only need
    // to single out a pipe, so declare the one call rather than depend on a
    // full Win32 bindings crate.
    const FILE_TYPE_PIPE: u32 = 0x0003;
    // SAFETY: GetFileType only reads the handle value and has no other side
    // effects; any handle is a valid argument (an invalid one yields
    // FILE_TYPE_UNKNOWN, which is correctly treated as "not a pipe").
    unsafe extern "system" {
        fn GetFileType(handle: *mut core::ffi::c_void) -> u32;
    }
    unsafe { GetFileType(handle) == FILE_TYPE_PIPE }
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

    // -- stdin data-loss classification (unix) -----------------------------

    #[cfg(unix)]
    mod data_loss {
        use super::super::fd_would_lose_data;
        use std::io::Write;
        use std::os::unix::io::AsRawFd;

        /// RAII pipe pair so a panicking assert still closes the fds.
        struct Pipe {
            read: libc::c_int,
            write: libc::c_int,
        }

        impl Pipe {
            fn new() -> Self {
                let mut fds = [0 as libc::c_int; 2];
                let rc = unsafe { libc::pipe(fds.as_mut_ptr()) };
                assert_eq!(rc, 0, "pipe() failed");
                Pipe {
                    read: fds[0],
                    write: fds[1],
                }
            }

            fn close_write(&mut self) {
                if self.write >= 0 {
                    unsafe { libc::close(self.write) };
                    self.write = -1;
                }
            }
        }

        impl Drop for Pipe {
            fn drop(&mut self) {
                self.close_write();
                if self.read >= 0 {
                    unsafe { libc::close(self.read) };
                }
            }
        }

        #[test]
        fn pipe_with_bytes_would_lose_data() {
            let p = Pipe::new();
            let wrote = unsafe { libc::write(p.write, b"x".as_ptr() as *const libc::c_void, 1) };
            assert_eq!(wrote, 1);
            assert!(
                fd_would_lose_data(p.read).unwrap(),
                "a pipe carrying a byte is data we'd lose"
            );
        }

        #[test]
        fn closed_pipe_eof_is_safe() {
            // A spawner that opened then closed the child's stdin pipe with no
            // data: the read end sees clean EOF -> nothing to lose -> proceed.
            let mut p = Pipe::new();
            p.close_write();
            assert!(
                !fd_would_lose_data(p.read).unwrap(),
                "a closed/EOF pipe has nothing to lose"
            );
        }

        #[test]
        fn open_but_silent_pipe_is_data_intent() {
            // Write end open, nothing sent yet: a writer exists and may send,
            // so the conservative verdict is "data intent" -> block.
            let p = Pipe::new();
            assert!(
                fd_would_lose_data(p.read).unwrap(),
                "an open pipe with a live writer is treated as data intent"
            );
        }

        #[test]
        fn empty_file_redirect_is_safe() {
            let f = tempfile::NamedTempFile::new().unwrap();
            let fd = f.as_file().as_raw_fd();
            assert!(
                !fd_would_lose_data(fd).unwrap(),
                "an empty `< file` redirect has nothing to lose"
            );
        }

        #[test]
        fn nonempty_file_redirect_would_lose_data() {
            let mut f = tempfile::NamedTempFile::new().unwrap();
            f.write_all(b"piped context\n").unwrap();
            f.flush().unwrap();
            let fd = f.as_file().as_raw_fd();
            assert!(
                fd_would_lose_data(fd).unwrap(),
                "a non-empty `< file` redirect is data the child would lose"
            );
        }

        #[test]
        fn dev_null_is_safe() {
            // /dev/null is a char device -- the canonical benign non-TTY
            // stdin an agent supplies; must proceed.
            let f = std::fs::File::open("/dev/null").unwrap();
            assert!(
                !fd_would_lose_data(f.as_raw_fd()).unwrap(),
                "/dev/null is a char device with nothing to lose"
            );
        }
    }
}
