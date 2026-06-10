//! Cross-platform stdin classification, shared by two callers.
//!
//! Two questions, two answers, one shared scaffolding (isatty + `fstat`
//! on unix, `GetFileType` on windows):
//!
//! - `stdin_would_lose_data` -- the `--detach` guard. "Is there input on
//!   stdin that the detached child (whose stdin is `/dev/null`) would
//!   silently drop?" Conservative: an open-but-silent pipe counts as data
//!   *intent* (`true`), because a writer exists and may yet send.
//! - `stdin_is_readable` -- the prompt stdin-as-context guard. "Can I read
//!   stdin right now without blocking?" An open-but-idle inherited pipe (no
//!   data, no EOF -- a backgrounded/orchestrated roba) is NOT readable
//!   (`false`): reading it would hang forever (#288), so the caller skips it.
//!
//! The two differ deliberately on exactly one case -- the open-but-silent
//! pipe: detach blocks on it (can't observe the future), the prompt path
//! skips it (would otherwise hang). Everything else (a pipe with bytes, a
//! non-empty file, a clean EOF, a TTY, `/dev/null`) they agree on.

/// Would a detached run silently lose data on this process's stdin?
///
/// The detached child's stdin is `/dev/null`, so anything pending on THIS
/// process's stdin can never reach it. Block exactly that -- real input we'd
/// drop -- while letting benign non-TTY stdin through (a TTY, `/dev/null`, or
/// a closed/EOF pipe from a spawner), so an orchestrator firing
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
pub(crate) fn stdin_would_lose_data() -> std::io::Result<bool> {
    use std::os::unix::io::AsRawFd;
    fd_would_lose_data(std::io::stdin().as_raw_fd())
}

/// Classify an arbitrary fd for the data-loss check (the testable core of
/// [`stdin_would_lose_data`]).
#[cfg(unix)]
pub(crate) fn fd_would_lose_data(fd: std::os::unix::io::RawFd) -> std::io::Result<bool> {
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
    if !fifo_poll_readable(fd)? {
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

/// Poll a FIFO fd with a zero timeout: `true` iff it is readable right now
/// (data pending OR a clean EOF/POLLHUP), `false` iff an open writer exists
/// but has sent nothing yet. Non-consuming -- it never reads a byte. Shared
/// by the consuming detach probe and the non-consuming prompt probe.
#[cfg(unix)]
fn fifo_poll_readable(fd: std::os::unix::io::RawFd) -> std::io::Result<bool> {
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
    Ok(n > 0)
}

/// Can stdin be read right now WITHOUT blocking? The prompt stdin-as-context
/// guard (#288): read piped stdin only when something is actually there.
///
/// Unlike [`stdin_would_lose_data`], this is NON-consuming and treats an
/// open-but-idle pipe as NOT readable -- reading it would hang forever, which
/// is exactly the backgrounded/orchestrator freeze #288 fixes.
#[cfg(unix)]
pub(crate) fn stdin_is_readable() -> std::io::Result<bool> {
    use std::os::unix::io::AsRawFd;
    fd_is_readable(std::io::stdin().as_raw_fd())
}

/// Non-consuming readability classification for an arbitrary fd (the testable
/// core of [`stdin_is_readable`]).
///
/// - TTY                     -> `false` (don't drain an interactive terminal)
/// - regular file (`< file`) -> `true` iff non-empty (an empty file adds
///   nothing; a file read never blocks regardless)
/// - FIFO / pipe             -> readable now (data OR clean EOF) -> `true`;
///   open-but-idle (writer present, no bytes) -> `false` (the no-hang case)
/// - char device (/dev/null) / socket / anything else -> `false`
#[cfg(unix)]
pub(crate) fn fd_is_readable(fd: std::os::unix::io::RawFd) -> std::io::Result<bool> {
    if unsafe { libc::isatty(fd) } == 1 {
        return Ok(false);
    }
    // SAFETY: see `fd_would_lose_data`.
    let mut st: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstat(fd, &mut st) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    match st.st_mode & libc::S_IFMT {
        libc::S_IFREG => Ok(st.st_size > 0),
        // Readable now (data or EOF) -> safe to read_to_string without
        // blocking. Open-but-idle -> not readable -> skip (no hang).
        libc::S_IFIFO => fifo_poll_readable(fd),
        _ => Ok(false),
    }
}

/// Windows piped-data classification for the `--detach` guard.
///
/// Unlike the unix path (which peeks for actual bytes), Windows classifies
/// by handle TYPE only, via `GetFileType` on stdin:
/// - `FILE_TYPE_PIPE`  -> peek via `PeekNamedPipe` (data present -> `true`,
///   empty/EOF -> `false`)
/// - `FILE_TYPE_CHAR` (a console or `NUL`), a disk file, or an
///   unknown/erroring type -> `false` (proceed)
#[cfg(not(unix))]
pub(crate) fn stdin_would_lose_data() -> std::io::Result<bool> {
    use std::os::windows::io::AsRawHandle;
    let handle = std::io::stdin().as_raw_handle();
    if !handle_is_pipe(handle) {
        return Ok(false);
    }
    Ok(pipe_has_data(handle))
}

/// Can stdin be read right now WITHOUT blocking? The Windows prompt
/// stdin-as-context guard (#288).
///
/// - a console (TTY)        -> `false` (don't drain interactive input)
/// - a pipe                 -> readable iff it has bytes (`PeekNamedPipe`);
///   an empty/EOF pipe -> `false` (the no-hang case)
/// - a disk file / `NUL` / other -> `true` (a file read never blocks; `NUL`
///   returns EOF immediately)
#[cfg(not(unix))]
pub(crate) fn stdin_is_readable() -> std::io::Result<bool> {
    use std::io::IsTerminal;
    use std::os::windows::io::AsRawHandle;
    if std::io::stdin().is_terminal() {
        return Ok(false);
    }
    let handle = std::io::stdin().as_raw_handle();
    if !handle_is_pipe(handle) {
        return Ok(true);
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

#[cfg(test)]
#[cfg(unix)]
mod tests {
    use super::{fd_is_readable, fd_would_lose_data};
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

    // -- detach data-loss classification (moved from detach.rs, unchanged) --

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

    // -- prompt stdin readability classification (#288) --------------------

    #[test]
    fn pipe_with_bytes_is_readable() {
        // Data present -> readable -> `cat err.log | roba "..."` still reads.
        let p = Pipe::new();
        let wrote = unsafe { libc::write(p.write, b"log\n".as_ptr() as *const libc::c_void, 4) };
        assert_eq!(wrote, 4);
        assert!(
            fd_is_readable(p.read).unwrap(),
            "a pipe with bytes is readable"
        );
    }

    #[test]
    fn open_but_idle_pipe_is_not_readable() {
        // THE #288 fix: write end open, no bytes, no EOF. Reading would hang
        // forever; classify it as not-readable so the prompt path skips it.
        let p = Pipe::new();
        assert!(
            !fd_is_readable(p.read).unwrap(),
            "an open-but-idle pipe is not readable -- the no-hang guard"
        );
    }

    #[test]
    fn closed_pipe_eof_is_readable() {
        // A clean EOF pipe is readable (read_to_string returns immediately
        // with an empty string -> no context, no block).
        let mut p = Pipe::new();
        p.close_write();
        assert!(
            fd_is_readable(p.read).unwrap(),
            "a clean-EOF pipe is readable (read returns empty without blocking)"
        );
    }

    #[test]
    fn nonempty_file_is_readable() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(b"context\n").unwrap();
        f.flush().unwrap();
        assert!(
            fd_is_readable(f.as_file().as_raw_fd()).unwrap(),
            "a non-empty `< file` redirect is readable"
        );
    }

    #[test]
    fn empty_file_is_not_readable() {
        let f = tempfile::NamedTempFile::new().unwrap();
        assert!(
            !fd_is_readable(f.as_file().as_raw_fd()).unwrap(),
            "an empty file adds nothing -- skip it"
        );
    }

    #[test]
    fn dev_null_is_not_readable() {
        let f = std::fs::File::open("/dev/null").unwrap();
        assert!(
            !fd_is_readable(f.as_raw_fd()).unwrap(),
            "/dev/null carries no context"
        );
    }
}
