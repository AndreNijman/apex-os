//! PTY creation and process spawning.
//!
//! The roadmap is explicit that the PTY is the primitive: APEX creates the
//! terminal, the sandbox and the environment, then launches the *normal* agent
//! binary inside it. The agent still sees an ordinary terminal, which is why
//! persistence, reattachment and status work without any cooperation from
//! upstream — and why nothing here scrapes terminal pixels.

use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::io::RawFd;
use std::path::Path;

use anyhow::{bail, Context, Result};
use apex_agent_core::term::WinSize;

/// A spawned session: the master side of its PTY and its process ids.
#[derive(Debug)]
pub struct Spawned {
    /// Master side. The daemon owns this; the child never sees it.
    pub master: RawFd,
    /// The child process id.
    pub pid: libc::pid_t,
    /// The child's process group, which is its own because it calls `setsid`.
    ///
    /// Signals go to the group, not the process, so `apex agent pause` stops
    /// the agent *and* everything it started — including through the sandbox's
    /// PID namespace, which does not hide group membership from the host.
    pub pgid: libc::pid_t,
}

/// Launch `argv` on a new PTY.
///
/// `env` fully replaces the child's environment when `clear_env` is set, which
/// is what an unconfined session uses to get the same default-deny treatment
/// the sandbox applies through `--clearenv`.
pub fn spawn(
    argv: &[String],
    cwd: &Path,
    env: &[(String, String)],
    clear_env: bool,
    size: WinSize,
) -> Result<Spawned> {
    if argv.is_empty() {
        bail!("no program to run");
    }

    // Everything that can allocate or fail must happen *before* fork: between
    // fork and exec only async-signal-safe calls are legal, and a failed
    // allocation there would be undebuggable.
    let c_argv: Vec<CString> = argv
        .iter()
        .map(|a| CString::new(a.as_bytes()))
        .collect::<std::result::Result<_, _>>()
        .context("an argument contained a NUL byte")?;
    let mut argv_ptrs: Vec<*const libc::c_char> =
        c_argv.iter().map(|s| s.as_ptr()).collect();
    argv_ptrs.push(std::ptr::null());

    let c_cwd = CString::new(cwd.as_os_str().as_bytes()).context("cwd contained a NUL byte")?;

    let c_env: Vec<CString> = env
        .iter()
        .map(|(k, v)| CString::new(format!("{k}={v}")))
        .collect::<std::result::Result<_, _>>()
        .context("an environment entry contained a NUL byte")?;
    let c_env_names: Vec<CString> = env
        .iter()
        .map(|(k, _)| CString::new(k.as_bytes()))
        .collect::<std::result::Result<_, _>>()
        .context("an environment name contained a NUL byte")?;

    let ws = libc::winsize {
        ws_row: size.rows,
        ws_col: size.cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };

    // An empty signal mask for the child, built before the fork so the child
    // only has to apply it.
    //
    // This matters more than it looks. A process's blocked-signal mask is
    // inherited across fork *and preserved across execve* — unlike signal
    // dispositions, which exec resets. The daemon blocks SIGTERM, SIGINT and
    // SIGHUP so its signal thread can wait on them, and without clearing that
    // here every agent session inherited the block: `apex agent kill` did
    // nothing, and ctrl-C inside an attached agent was swallowed.
    let empty_mask: libc::sigset_t = unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        set
    };

    // Synchronisation pipe. The child holds the write end with FD_CLOEXEC, so
    // a successful execvp closes it and the parent's read sees end-of-file;
    // any failure before exec writes its errno down it instead.
    //
    // This is not only for error reporting. The child becomes a session leader
    // inside login_tty, which happens *after* fork returns in the parent —
    // without waiting for it, an `apex agent kill` issued immediately after
    // `run` would call killpg on a process group that does not exist yet, get
    // ESRCH, and silently do nothing while the agent kept running.
    let mut sync_fds = [0 as RawFd; 2];
    // Safe: pipe2 writes two descriptors into an array we own.
    if unsafe { libc::pipe2(sync_fds.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
        return Err(std::io::Error::last_os_error()).context("creating the exec sync pipe");
    }
    let (sync_read, sync_write) = (sync_fds[0], sync_fds[1]);

    let mut master: RawFd = -1;
    let mut slave: RawFd = -1;
    // Safe: openpty writes two descriptors we own and reads the winsize we
    // just built. Passing null for name and termios takes the defaults.
    let rc = unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null(),
            &ws,
        )
    };
    if rc != 0 {
        let err = std::io::Error::last_os_error();
        unsafe {
            libc::close(sync_read);
            libc::close(sync_write);
        }
        return Err(err).context("allocating a pseudo-terminal");
    }

    // Safe: fork with no allocation in the child path below.
    let pid = unsafe { libc::fork() };
    if pid < 0 {
        let err = std::io::Error::last_os_error();
        unsafe {
            libc::close(master);
            libc::close(slave);
            libc::close(sync_read);
            libc::close(sync_write);
        }
        return Err(err).context("forking the agent process");
    }

    if pid == 0 {
        // ── child ────────────────────────────────────────────────────────
        // No allocation, no Rust I/O, no panicking: only raw syscalls.
        unsafe {
            libc::close(master);
            libc::close(sync_read);

            // Report a pre-exec failure to the parent and stop. The parent
            // distinguishes this from a successful exec by the pipe carrying
            // bytes instead of closing empty.
            let fail = |stage: u8, code: i32| -> ! {
                let errno = *libc::__errno_location();
                let msg = [stage, errno as u8, (errno >> 8) as u8];
                libc::write(sync_write, msg.as_ptr() as *const libc::c_void, msg.len());
                libc::_exit(code)
            };

            // Becomes a session leader, makes the slave its controlling
            // terminal, and wires it to stdin/stdout/stderr. This is what makes
            // the process group its own, and therefore signalable as a unit.
            if libc::login_tty(slave) != 0 {
                fail(STAGE_LOGIN_TTY, 126);
            }

            if libc::chdir(c_cwd.as_ptr()) != 0 {
                fail(STAGE_CHDIR, 125);
            }

            if clear_env {
                // clearenv can allocate on some libcs; unsetting the names we
                // know and then setting ours is enough, because the sandbox
                // does the authoritative clearing for confined sessions and an
                // unconfined session is the documented escape hatch.
                for name in &c_env_names {
                    libc::unsetenv(name.as_ptr());
                }
            }
            for entry in &c_env {
                libc::putenv(entry.as_ptr() as *mut libc::c_char);
            }

            // Default the dispositions the daemon changed. exec resets handlers
            // but not SIG_IGN, and an inherited ignored SIGPIPE changes how
            // every pipeline the agent runs behaves.
            libc::signal(libc::SIGPIPE, libc::SIG_DFL);
            libc::signal(libc::SIGCHLD, libc::SIG_DFL);
            libc::signal(libc::SIGINT, libc::SIG_DFL);
            libc::signal(libc::SIGTERM, libc::SIG_DFL);
            libc::signal(libc::SIGHUP, libc::SIG_DFL);

            // Unblock everything. exec does NOT clear the signal mask, so
            // without this the agent inherits the daemon's blocked SIGTERM and
            // cannot be killed or interrupted.
            libc::sigprocmask(libc::SIG_SETMASK, &empty_mask, std::ptr::null_mut());

            // On success this never returns and FD_CLOEXEC closes sync_write,
            // which is what the parent reads as "the agent is running".
            libc::execvp(argv_ptrs[0], argv_ptrs.as_ptr());
            // 127 is the shell's convention for "command not found", which is
            // what this almost always is.
            fail(STAGE_EXEC, 127);
        }
    }

    // ── parent ───────────────────────────────────────────────────────────
    // Safe: closing descriptors we own; the child has its own copies.
    unsafe {
        libc::close(slave);
        libc::close(sync_write);
    }

    // Block until the child has either exec'd or failed. This is what makes
    // the returned `pgid` real: login_tty's setsid has definitely run by the
    // time the pipe resolves, so a kill issued immediately after this returns
    // cannot race the process group into existence.
    let outcome = read_sync(sync_read);
    unsafe { libc::close(sync_read) };

    if let Some((stage, errno)) = outcome {
        // Reap the child that is already on its way out, so it does not linger
        // as a zombie for a session that never started.
        let mut status: libc::c_int = 0;
        unsafe { libc::waitpid(pid, &mut status, 0) };
        unsafe { libc::close(master) };
        let err = std::io::Error::from_raw_os_error(errno);
        let what = match stage {
            STAGE_LOGIN_TTY => "attaching the agent to its terminal",
            STAGE_CHDIR => "entering the working directory",
            _ => "starting the agent program",
        };
        return Err(err).context(what.to_string());
    }

    set_nonblocking(master)?;
    set_cloexec(master)?;

    Ok(Spawned {
        master,
        pid,
        // login_tty called setsid, so the child's group id is its own pid, and
        // the read above proves it has already happened.
        pgid: pid,
    })
}

/// Stage markers written down the sync pipe when the child fails before exec.
const STAGE_LOGIN_TTY: u8 = 1;
const STAGE_CHDIR: u8 = 2;
const STAGE_EXEC: u8 = 3;

/// Wait for the child to exec or report a failure.
///
/// `None` means the pipe closed empty: `FD_CLOEXEC` fired, so exec succeeded.
/// `Some((stage, errno))` is a pre-exec failure the child described.
fn read_sync(fd: RawFd) -> Option<(u8, i32)> {
    let mut buf = [0u8; 3];
    let mut filled = 0usize;
    while filled < buf.len() {
        // Safe: read into a buffer we own, bounded by its remaining length.
        let n = unsafe {
            libc::read(
                fd,
                buf.as_mut_ptr().add(filled) as *mut libc::c_void,
                buf.len() - filled,
            )
        };
        if n > 0 {
            filled += n as usize;
            continue;
        }
        if n == 0 {
            break;
        }
        if std::io::Error::last_os_error().raw_os_error() == Some(libc::EINTR) {
            continue;
        }
        break;
    }
    if filled < buf.len() {
        return None;
    }
    let errno = i32::from(buf[1]) | (i32::from(buf[2]) << 8);
    Some((buf[0], errno))
}

/// Put a descriptor into non-blocking mode.
pub fn set_nonblocking(fd: RawFd) -> Result<()> {
    // Safe: F_GETFL/F_SETFL only read and write this descriptor's flags.
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFL);
        if flags < 0 {
            return Err(std::io::Error::last_os_error()).context("reading descriptor flags");
        }
        if libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) < 0 {
            return Err(std::io::Error::last_os_error()).context("setting O_NONBLOCK");
        }
    }
    Ok(())
}

/// Close a descriptor on exec, so a later session never inherits an earlier
/// session's PTY.
pub fn set_cloexec(fd: RawFd) -> Result<()> {
    // Safe: F_GETFD/F_SETFD only read and write this descriptor's flags.
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFD);
        if flags < 0 {
            return Err(std::io::Error::last_os_error()).context("reading descriptor flags");
        }
        if libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) < 0 {
            return Err(std::io::Error::last_os_error()).context("setting FD_CLOEXEC");
        }
    }
    Ok(())
}

/// Read from a non-blocking descriptor.
///
/// A PTY master reports `EIO` rather than end-of-file once the last slave
/// descriptor closes, which is the normal way a session ends. Both are reported
/// as `Ok(None)`.
pub fn read_nonblocking(fd: RawFd, buf: &mut [u8]) -> Result<Option<usize>> {
    // Safe: read into a buffer we own, bounded by its length.
    let n = unsafe { libc::read(fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
    if n > 0 {
        return Ok(Some(n as usize));
    }
    if n == 0 {
        return Ok(None);
    }
    let err = std::io::Error::last_os_error();
    match err.raw_os_error() {
        Some(libc::EAGAIN) | Some(libc::EINTR) => Ok(Some(0)),
        // The child closed the terminal: end of session, not a failure.
        Some(libc::EIO) => Ok(None),
        _ => Err(err).context("reading the session terminal"),
    }
}

/// Write to the PTY master, retrying short writes.
pub fn write_all(fd: RawFd, mut buf: &[u8]) -> Result<()> {
    while !buf.is_empty() {
        // Safe: write from a buffer we own, bounded by its length.
        let n = unsafe { libc::write(fd, buf.as_ptr() as *const libc::c_void, buf.len()) };
        if n > 0 {
            buf = &buf[n as usize..];
            continue;
        }
        let err = std::io::Error::last_os_error();
        match err.raw_os_error() {
            Some(libc::EINTR) => continue,
            Some(libc::EAGAIN) => {
                // The agent is not reading. Wait for writability rather than
                // spinning; a busy loop here would burn a core whenever a TUI
                // paused its input.
                if !wait_writable(fd, 100) {
                    continue;
                }
            }
            _ => return Err(err).context("writing to the session terminal"),
        }
    }
    Ok(())
}

/// Wait until `fd` is readable or `timeout_ms` elapses. True when readable.
pub fn wait_readable(fd: RawFd, timeout_ms: i32) -> bool {
    poll_one(fd, libc::POLLIN, timeout_ms)
}

/// Wait until `fd` is writable or `timeout_ms` elapses. True when writable.
pub fn wait_writable(fd: RawFd, timeout_ms: i32) -> bool {
    poll_one(fd, libc::POLLOUT, timeout_ms)
}

fn poll_one(fd: RawFd, events: libc::c_short, timeout_ms: i32) -> bool {
    let mut pfd = libc::pollfd {
        fd,
        events,
        revents: 0,
    };
    // Safe: poll reads and writes one pollfd we own.
    let rc = unsafe { libc::poll(&mut pfd, 1, timeout_ms) };
    // POLLHUP and POLLERR also mean "stop waiting": the caller's next read
    // will see the end of the session.
    rc > 0 && (pfd.revents & (events | libc::POLLHUP | libc::POLLERR)) != 0
}

/// How a child process ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wait {
    /// Still running.
    Running,
    /// Exited with this status.
    Exited(i32),
    /// Killed by this signal.
    Signalled(i32),
    /// Already reaped, or never ours.
    Gone,
}

/// Reap `pid` without blocking.
pub fn try_wait(pid: libc::pid_t) -> Wait {
    let mut status: libc::c_int = 0;
    // Safe: waitpid writes one int we own.
    let rc = unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) };
    if rc == 0 {
        return Wait::Running;
    }
    if rc < 0 {
        return Wait::Gone;
    }
    decode_status(status)
}

/// Decode a `waitpid` status word.
pub fn decode_status(status: libc::c_int) -> Wait {
    // libc::WIFEXITED and friends are macros in C; the crate exposes them as
    // functions with the same semantics.
    if libc::WIFEXITED(status) {
        Wait::Exited(libc::WEXITSTATUS(status))
    } else if libc::WIFSIGNALED(status) {
        Wait::Signalled(libc::WTERMSIG(status))
    } else {
        Wait::Running
    }
}

/// Send a signal to a session's whole process group.
///
/// The group, not the process: an agent that started a build must have the
/// build stopped with it, or `apex agent pause` would leave a compiler running.
pub fn signal_group(pgid: libc::pid_t, signal: i32) -> Result<()> {
    if pgid <= 1 {
        bail!("refusing to signal process group {pgid}");
    }
    // Safe: killpg only delivers a signal.
    if unsafe { libc::killpg(pgid, signal) } != 0 {
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() == Some(libc::ESRCH) {
            // Already gone. Not a failure worth reporting to the user.
            return Ok(());
        }
        return Err(err).context("signalling the session");
    }
    Ok(())
}

/// Resize a session's terminal.
pub fn resize(master: RawFd, size: WinSize) -> Result<()> {
    apex_agent_core::term::set_window_size(master, size).context("resizing the session terminal")
}

/// Close a descriptor, ignoring failure.
pub fn close(fd: RawFd) {
    if fd >= 0 {
        // Safe: closing a descriptor we own exactly once.
        unsafe { libc::close(fd) };
    }
}

/// Resolve a program name the way `execvp` will, so a missing binary is
/// reported as a clear error at request time instead of as exit code 127
/// several seconds later inside a PTY nobody is watching yet.
pub fn resolve_program(program: &str) -> Option<std::path::PathBuf> {
    let p = Path::new(program);
    if program.contains('/') {
        return is_executable(p).then(|| p.to_path_buf());
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(program))
        .find(|candidate| is_executable(candidate))
}

fn is_executable(path: &Path) -> bool {
    let c = match CString::new(path.as_os_str().as_bytes()) {
        Ok(c) => c,
        Err(_) => return false,
    };
    // Safe: access only reads the path we just built.
    unsafe { libc::access(c.as_ptr(), libc::X_OK) == 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_status_word_decodes_to_the_right_outcome() {
        // Encoded the way the kernel does: low byte holds the signal, second
        // byte the exit status.
        assert_eq!(decode_status(0), Wait::Exited(0));
        assert_eq!(decode_status(3 << 8), Wait::Exited(3));
        assert_eq!(decode_status(libc::SIGKILL), Wait::Signalled(libc::SIGKILL));
    }

    #[test]
    fn refusing_to_signal_init_or_the_whole_world() {
        assert!(signal_group(0, libc::SIGTERM).is_err());
        assert!(signal_group(1, libc::SIGTERM).is_err());
        assert!(signal_group(-1, libc::SIGTERM).is_err());
    }

    #[test]
    fn an_absolute_program_resolves_only_when_executable() {
        assert_eq!(
            resolve_program("/bin/sh"),
            Some(std::path::PathBuf::from("/bin/sh"))
        );
        assert_eq!(resolve_program("/nonexistent/binary"), None);
        assert_eq!(resolve_program("/etc/hostname"), None, "not executable");
    }

    #[test]
    fn a_bare_name_resolves_through_path() {
        let sh = resolve_program("sh").expect("sh must be on PATH");
        assert!(sh.is_absolute());
        assert!(sh.ends_with("sh"));
        assert_eq!(resolve_program("definitely-not-a-real-binary-xyz"), None);
    }

    #[test]
    fn spawning_runs_the_real_program_on_a_real_terminal() {
        // The load-bearing property: the child must believe it has a terminal,
        // because every TUI agent behaves differently when it does not.
        let spawned = spawn(
            &[
                "/bin/sh".to_string(),
                "-c".to_string(),
                "test -t 1 && echo IS_A_TTY; exit 7".to_string(),
            ],
            Path::new("/tmp"),
            &[("TERM".to_string(), "xterm-256color".to_string())],
            true,
            WinSize {
                cols: 100,
                rows: 40,
            },
        )
        .expect("spawn");

        let mut collected = Vec::new();
        let mut buf = [0u8; 1024];
        for _ in 0..200 {
            wait_readable(spawned.master, 25);
            match read_nonblocking(spawned.master, &mut buf) {
                Ok(Some(0)) => {}
                Ok(Some(n)) => collected.extend_from_slice(&buf[..n]),
                // End of the terminal. The child may still not be reaped: the
                // PTY closing and waitpid reporting the exit are two separate
                // events and they race.
                Ok(None) | Err(_) => break,
            }
        }
        let status = wait_for_exit(spawned.pid);
        close(spawned.master);

        let text = String::from_utf8_lossy(&collected);
        assert!(text.contains("IS_A_TTY"), "child saw no tty: {text:?}");
        assert_eq!(status, Wait::Exited(7), "exit status was not propagated");
    }

    /// Reap `pid`, tolerating the gap between the PTY closing and the kernel
    /// making the exit status available.
    fn wait_for_exit(pid: libc::pid_t) -> Wait {
        for _ in 0..500 {
            match try_wait(pid) {
                Wait::Running => std::thread::sleep(std::time::Duration::from_millis(10)),
                other => return other,
            }
        }
        Wait::Running
    }

    #[test]
    fn the_child_gets_its_own_process_group() {
        // Required for signal_group: without setsid the group would be the
        // daemon's, and pausing a session would pause the daemon.
        let spawned = spawn(
            &["/bin/sh".to_string(), "-c".to_string(), "sleep 5".to_string()],
            Path::new("/tmp"),
            &[],
            true,
            WinSize::FALLBACK,
        )
        .expect("spawn");

        assert_eq!(spawned.pgid, spawned.pid);
        assert_ne!(spawned.pgid, unsafe { libc::getpgrp() });

        signal_group(spawned.pgid, libc::SIGKILL).expect("kill");
        let outcome = wait_for_exit(spawned.pid);
        close(spawned.master);
        assert_eq!(outcome, Wait::Signalled(libc::SIGKILL));
    }

    #[test]
    fn the_requested_window_size_reaches_the_child() {
        let spawned = spawn(
            &[
                "/bin/sh".to_string(),
                "-c".to_string(),
                "stty size 2>/dev/null || echo no-stty".to_string(),
            ],
            Path::new("/tmp"),
            &[("TERM".to_string(), "xterm".to_string())],
            true,
            WinSize {
                cols: 132,
                rows: 43,
            },
        )
        .expect("spawn");

        let mut collected = Vec::new();
        let mut buf = [0u8; 512];
        for _ in 0..200 {
            wait_readable(spawned.master, 25);
            match read_nonblocking(spawned.master, &mut buf) {
                Ok(Some(0)) => {}
                Ok(Some(n)) => collected.extend_from_slice(&buf[..n]),
                _ => break,
            }
            if String::from_utf8_lossy(&collected).contains('\n') {
                break;
            }
        }
        close(spawned.master);
        let text = String::from_utf8_lossy(&collected);
        assert!(text.contains("43 132"), "stty reported {text:?}");
    }

    #[test]
    fn a_missing_program_is_reported_before_it_is_spawned() {
        assert_eq!(resolve_program("definitely-not-a-real-binary-xyz"), None);
    }

    #[test]
    fn a_failed_exec_is_an_error_not_a_silently_started_session() {
        // Without the sync pipe this returned Ok with a live-looking session
        // whose process had already exited 127 into a terminal nobody was
        // watching yet.
        let err = spawn(
            &["/nonexistent/agent-binary".to_string()],
            Path::new("/tmp"),
            &[],
            true,
            WinSize::FALLBACK,
        )
        .expect_err("a missing binary must fail the spawn");
        let text = format!("{err:#}");
        assert!(text.contains("starting the agent program"), "{text}");
    }

    #[test]
    fn a_bad_working_directory_is_reported_with_its_own_cause() {
        let err = spawn(
            &["/bin/sh".to_string()],
            Path::new("/nonexistent/directory"),
            &[],
            true,
            WinSize::FALLBACK,
        )
        .expect_err("a missing cwd must fail the spawn");
        let text = format!("{err:#}");
        assert!(text.contains("entering the working directory"), "{text}");
    }

    #[test]
    fn a_session_does_not_inherit_the_daemons_blocked_signals() {
        // The regression: the daemon blocks SIGTERM/SIGINT/SIGHUP so its signal
        // thread can sigwait on them. That mask survives execve, so every
        // session started with SIGTERM blocked and `apex agent kill` was a
        // no-op — the process sat there with the signal permanently pending.
        //
        // Block them here the way the daemon does, then assert the child comes
        // out clean.
        let mut blocked: libc::sigset_t = unsafe { std::mem::zeroed() };
        let mut previous: libc::sigset_t = unsafe { std::mem::zeroed() };
        unsafe {
            libc::sigemptyset(&mut blocked);
            libc::sigaddset(&mut blocked, libc::SIGTERM);
            libc::sigaddset(&mut blocked, libc::SIGINT);
            libc::sigaddset(&mut blocked, libc::SIGHUP);
            libc::pthread_sigmask(libc::SIG_BLOCK, &blocked, &mut previous);
        }

        let spawned = spawn(
            &[
                "/bin/sh".to_string(),
                "-c".to_string(),
                // SigBlk from the child's own /proc entry, as a hex mask.
                "grep '^SigBlk' /proc/self/status".to_string(),
            ],
            Path::new("/tmp"),
            &[],
            true,
            WinSize::FALLBACK,
        )
        .expect("spawn");

        // Restore the test process's mask before asserting, so a failure here
        // does not leave the harness with signals blocked.
        unsafe { libc::pthread_sigmask(libc::SIG_SETMASK, &previous, std::ptr::null_mut()) };

        let mut collected = Vec::new();
        let mut buf = [0u8; 1024];
        for _ in 0..200 {
            wait_readable(spawned.master, 25);
            match read_nonblocking(spawned.master, &mut buf) {
                Ok(Some(0)) => {}
                Ok(Some(n)) => collected.extend_from_slice(&buf[..n]),
                Ok(None) | Err(_) => break,
            }
            if String::from_utf8_lossy(&collected).contains('\n') {
                break;
            }
        }
        wait_for_exit(spawned.pid);
        close(spawned.master);

        let text = String::from_utf8_lossy(&collected);
        let hex = text
            .split_whitespace()
            .next_back()
            .expect("SigBlk line: {text:?}");
        let mask = u64::from_str_radix(hex.trim(), 16).unwrap_or_else(|e| {
            panic!("cannot parse SigBlk {hex:?} from {text:?}: {e}");
        });

        for (signal, name) in [
            (libc::SIGTERM, "SIGTERM"),
            (libc::SIGINT, "SIGINT"),
            (libc::SIGHUP, "SIGHUP"),
        ] {
            let bit = 1u64 << (signal - 1);
            assert_eq!(
                mask & bit,
                0,
                "{name} is blocked in the session (SigBlk {hex}); it would be unkillable"
            );
        }
    }

    #[test]
    fn a_kill_issued_immediately_after_spawn_reaches_the_session() {
        // The regression this pins: login_tty (and therefore setsid) runs in
        // the child *after* fork returns. Before the sync pipe, killpg on the
        // freshly returned pgid raced that setsid, failed with ESRCH, and was
        // swallowed as success — so `apex agent kill` right after `run`
        // silently did nothing while the agent kept running.
        for attempt in 0..20 {
            let spawned = spawn(
                &[
                    "/bin/sh".to_string(),
                    "-c".to_string(),
                    "sleep 30".to_string(),
                ],
                Path::new("/tmp"),
                &[],
                true,
                WinSize::FALLBACK,
            )
            .expect("spawn");

            // No sleep, no poll: signal the group the instant spawn returns.
            signal_group(spawned.pgid, libc::SIGKILL).expect("kill");

            let outcome = wait_for_exit(spawned.pid);
            close(spawned.master);
            assert_eq!(
                outcome,
                Wait::Signalled(libc::SIGKILL),
                "attempt {attempt}: the immediate kill did not reach the session"
            );
        }
    }

    #[test]
    fn spawning_with_no_argv_is_an_error_not_a_panic() {
        assert!(spawn(&[], Path::new("/tmp"), &[], true, WinSize::FALLBACK).is_err());
    }
}
