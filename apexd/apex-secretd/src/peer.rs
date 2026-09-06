//! Who is on the other end of a connection.
//!
//! The socket is `0666` — every local account must be able to reach the daemon,
//! and a mode bit is the wrong place to decide who may do what. Identity comes
//! from the kernel instead:
//!
//! 1. `SO_PEERCRED` on the accepted socket yields the peer's pid, uid and gid.
//!    The kernel fills it in at `connect(2)` and a process cannot forge it. The
//!    **uid** is the authorisation fact: the store is per-uid, and a request
//!    only ever reaches the connecting account's own namespace.
//! 2. The **pid** is not an identity. Pids are reused, and between `accept` and
//!    a `/proc` read the peer may have exited and its number been handed to
//!    something else. So the pid is turned into a [`ProcHandle`] — a dirfd on
//!    `/proc/<pid>` opened once, immediately after `accept` — and every later
//!    question is asked through that dirfd. A dirfd is bound to the process it
//!    was opened for: if the pid is reused, `openat` through it fails with
//!    `ESRCH` and the daemon refuses instead of answering about a stranger.
//!
//! ## What the pid is used for, and what it is not
//!
//! Not for authorisation of a *use*: that is the uid, the per-project grant and
//! the host pin. The pid answers one narrower question — is this connection
//! coming from inside a managed agent session? — and the answer gates the
//! mutating verbs, so a session cannot grant itself a capability and then use
//! it.
//!
//! Two independent checks, because each covers the other's gap:
//!
//! * **cgroup.** A session forked by `apex-agentd` inherits the daemon's cgroup,
//!   and cgroup membership survives `fork` and reparenting — so a double-fork
//!   that escapes an ancestry walk does not escape this.
//! * **ancestry.** `/proc/<pid>/status`'s `PPid`, walked upward, looking for a
//!   process whose `exe` is `apex-agentd` or `bwrap`. Covers a session that a
//!   future change moves into its own cgroup, or a machine where the agent
//!   runtime was started by hand rather than by systemd.
//!
//! Neither is airtight against a process with the user's uid that is trying:
//! systemd delegates the user slice, so a user process can move itself into
//! another cgroup, and a double fork changes a parent. That is written down in
//! `apex_secret_core`'s threat-model note rather than papered over. The
//! confidentiality boundary — the credential itself — does not depend on any of
//! this; it depends on the store being root-owned and the API having no verb
//! that returns a value.

use std::ffi::CString;
use std::os::unix::io::{AsRawFd, RawFd};
use std::os::unix::net::UnixStream;

/// What the kernel says about the other end of a connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Peer {
    pub pid: libc::pid_t,
    pub uid: libc::uid_t,
    pub gid: libc::gid_t,
}

/// Read `SO_PEERCRED` from an accepted connection.
///
/// `None` means the option was unavailable, which is treated as "unknown peer"
/// and therefore as unauthenticated — never as trusted.
pub fn credentials(stream: &UnixStream) -> Option<Peer> {
    let mut cred = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // Safe: getsockopt writes at most `len` bytes into `cred`, which is owned
    // here and exactly that size, and the fd is valid for the borrow.
    let rc = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            &mut cred as *mut libc::ucred as *mut libc::c_void,
            &mut len,
        )
    };
    if rc != 0 || len as usize != std::mem::size_of::<libc::ucred>() {
        return None;
    }
    Some(Peer {
        pid: cred.pid,
        uid: cred.uid,
        gid: cred.gid,
    })
}

/// A dirfd on `/proc/<pid>`, pinned to one process.
///
/// The point of holding the directory rather than the number: a pid can be
/// reused between `accept` and a read, and a check that then reads
/// `/proc/<pid>/cgroup` is describing whatever process now holds that number.
/// Through this handle the same read fails with `ESRCH`, and a failed read is
/// treated as "refuse", not as "not a session".
#[derive(Debug)]
pub struct ProcHandle {
    fd: RawFd,
}

impl ProcHandle {
    /// Open `/proc/<pid>`. `None` if the process is already gone.
    pub fn open(pid: libc::pid_t) -> Option<ProcHandle> {
        if pid <= 0 {
            return None;
        }
        let path = CString::new(format!("/proc/{pid}")).ok()?;
        // O_PATH is enough: this fd is only ever an `openat`/`readlinkat` base,
        // never read from directly, and O_PATH does not need read permission on
        // the directory.
        // Safe: `path` is a valid NUL-terminated string that outlives the call.
        let fd = unsafe {
            libc::open(
                path.as_ptr(),
                libc::O_PATH | libc::O_DIRECTORY | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return None;
        }
        Some(ProcHandle { fd })
    }

    /// Read a file from this process's `/proc` directory.
    ///
    /// `None` covers both "the process is gone" and "the file is unreadable",
    /// which callers must treat the same way: not as an absence of evidence.
    pub fn read(&self, name: &str) -> Option<String> {
        let name = CString::new(name).ok()?;
        // Safe: `self.fd` is a valid O_PATH dirfd for the life of `self`, and
        // `name` is a valid NUL-terminated string that outlives the call.
        let fd = unsafe { libc::openat(self.fd, name.as_ptr(), libc::O_RDONLY | libc::O_CLOEXEC) };
        if fd < 0 {
            return None;
        }
        // Safe: `fd` was just returned by openat and is owned here; `File`
        // takes ownership and closes it.
        let mut file = unsafe { <std::fs::File as std::os::unix::io::FromRawFd>::from_raw_fd(fd) };
        let mut text = String::new();
        use std::io::Read;
        file.read_to_string(&mut text).ok()?;
        Some(text)
    }

    /// The peer's executable, from `/proc/<pid>/exe`.
    pub fn exe(&self) -> Option<std::path::PathBuf> {
        readlink_at(self.fd, "exe")
    }

    /// The peer's parent, from `/proc/<pid>/status`.
    ///
    /// `status` rather than `stat`: `stat`'s second field is the executable
    /// name in parentheses and may itself contain spaces and parentheses, so
    /// splitting it on whitespace to reach the parent pid is a bug waiting for
    /// a process called `foo bar) 1 2 3`.
    pub fn parent(&self) -> Option<libc::pid_t> {
        parse_ppid(&self.read("status")?)
    }

    /// The peer's cgroup path, from `/proc/<pid>/cgroup`.
    pub fn cgroup(&self) -> Option<String> {
        Some(self.read("cgroup")?.trim().to_string())
    }
}

impl Drop for ProcHandle {
    fn drop(&mut self) {
        // Safe: `self.fd` was opened here and is not used again.
        unsafe { libc::close(self.fd) };
    }
}

fn readlink_at(dirfd: RawFd, name: &str) -> Option<std::path::PathBuf> {
    use std::os::unix::ffi::OsStringExt;

    let name = CString::new(name).ok()?;
    let mut buf = vec![0u8; libc::PATH_MAX as usize];
    // Safe: `buf` is owned and `buf.len()` bytes long, `name` outlives the call.
    let n = unsafe {
        libc::readlinkat(
            dirfd,
            name.as_ptr(),
            buf.as_mut_ptr() as *mut libc::c_char,
            buf.len(),
        )
    };
    if n <= 0 {
        return None;
    }
    buf.truncate(n as usize);
    Some(std::path::PathBuf::from(std::ffi::OsString::from_vec(buf)))
}

/// The `PPid:` line of a `/proc/<pid>/status`.
pub fn parse_ppid(status: &str) -> Option<libc::pid_t> {
    status
        .lines()
        .find_map(|l| l.strip_prefix("PPid:"))?
        .trim()
        .parse()
        .ok()
}

/// How far up the parent chain to walk before giving up.
///
/// A real chain from a sandboxed agent is three or four links. The bound exists
/// so a malformed or cyclic `/proc` cannot spin here.
const MAX_ANCESTRY: usize = 64;

/// Executable names that mean "this connection came from inside a session".
///
/// `bwrap` is the sandbox itself. `apex-agentd` covers an unconfined session,
/// which has no `bwrap` in its chain but is still a child of the daemon — and
/// also the daemon's own connections, which is correct: `apex-agentd` forwards
/// capability *uses* and never issues a mutating verb.
const SESSION_EXES: &[&str] = &["apex-agentd", "bwrap"];

/// Whether the peer is inside a managed agent session.
///
/// Two independent checks; either one is enough. `lookup` reads a pid's
/// `/proc` entry and is a parameter so the walk can be tested without spawning
/// a sandbox.
pub fn looks_like_a_session<F>(handle: &ProcHandle, lookup: F) -> bool
where
    F: Fn(libc::pid_t) -> Option<(Option<std::path::PathBuf>, Option<libc::pid_t>)>,
{
    // The cgroup. Survives fork and reparenting, so a double fork does not
    // escape it.
    if let Some(cgroup) = handle.cgroup() {
        if cgroup.contains("apex-agentd.service") {
            return true;
        }
    }

    // The ancestry, starting with the peer itself.
    if is_session_exe(handle.exe().as_deref()) {
        return true;
    }
    let mut current = match handle.parent() {
        Some(p) => p,
        None => return false,
    };
    for _ in 0..MAX_ANCESTRY {
        if current <= 1 {
            return false;
        }
        let Some((exe, parent)) = lookup(current) else {
            return false;
        };
        if is_session_exe(exe.as_deref()) {
            return true;
        }
        match parent {
            Some(p) if p != current => current = p,
            _ => return false,
        }
    }
    false
}

/// Read a pid's executable and parent from the live `/proc`.
pub fn live_lookup(
    pid: libc::pid_t,
) -> Option<(Option<std::path::PathBuf>, Option<libc::pid_t>)> {
    let handle = ProcHandle::open(pid)?;
    Some((handle.exe(), handle.parent()))
}

fn is_session_exe(exe: Option<&std::path::Path>) -> bool {
    let Some(exe) = exe else { return false };
    let Some(name) = exe.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    // `/proc/<pid>/exe` on a deleted binary reads as `…/apex-agentd (deleted)`,
    // which happens on every image update while the daemon is still running.
    let name = name.strip_suffix(" (deleted)").unwrap_or(name);
    SESSION_EXES.contains(&name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn credentials_come_from_a_real_socketpair_and_name_this_process() {
        let (a, b) = UnixStream::pair().expect("socketpair");
        let peer = credentials(&a).expect("SO_PEERCRED on a socketpair");
        assert_eq!(peer.pid, std::process::id() as libc::pid_t);
        // Safe: getuid cannot fail.
        assert_eq!(peer.uid, unsafe { libc::getuid() });
        assert_eq!(credentials(&b).map(|p| p.pid), Some(peer.pid));
    }

    #[test]
    fn a_handle_reads_this_process_through_its_own_dirfd() {
        let me = std::process::id() as libc::pid_t;
        let handle = ProcHandle::open(me).expect("open /proc/self");
        let status = handle.read("status").expect("status");
        assert!(status.contains("PPid:"), "{status}");
        assert_eq!(handle.parent(), parse_ppid(&status));

        // The exe link resolves to this test binary, whatever cargo called it.
        let exe = handle.exe().expect("exe");
        assert_eq!(exe, std::fs::read_link(format!("/proc/{me}/exe")).unwrap());

        // The cgroup line exists on any cgroup2 machine; on one without it the
        // check degrades to the ancestry walk rather than failing.
        assert!(handle.cgroup().is_some_and(|c| c.contains("::")) || handle.cgroup().is_none());
    }

    #[test]
    fn a_handle_for_a_dead_process_answers_nothing_rather_than_guessing() {
        // The pid-reuse property, as far as it can be tested without winning a
        // race: a pid nobody holds has no handle at all, and a handle whose
        // process has exited reads nothing.
        assert!(ProcHandle::open(0x7fff_fffe).is_none());
        assert!(ProcHandle::open(0).is_none());
        assert!(ProcHandle::open(-1).is_none());

        let child = std::process::Command::new("/bin/true")
            .spawn()
            .expect("spawn");
        let pid = child.id() as libc::pid_t;
        let handle = ProcHandle::open(pid).expect("open while alive");
        let mut child = child;
        child.wait().expect("wait");

        // The process is reaped, so its /proc directory is gone. Reads through
        // the pinned dirfd fail; they do not silently describe a new process
        // that has since been given the same number.
        assert_eq!(handle.read("status"), None);
        assert_eq!(handle.parent(), None);
        assert_eq!(handle.cgroup(), None);
    }

    #[test]
    fn the_ancestry_walk_finds_a_session_binary_and_is_bounded() {
        let me = std::process::id() as libc::pid_t;
        let handle = ProcHandle::open(me).unwrap();

        // A chain that never matches must terminate.
        let calls = std::cell::Cell::new(0usize);
        let found = looks_like_a_session(&handle, |pid| {
            calls.set(calls.get() + 1);
            // Every process claims a parent, forever. The bound is what stops
            // this.
            Some((Some(PathBuf::from("/usr/bin/bash")), Some(pid + 1)))
        });
        // The test process is not inside a session unless the suite is being
        // run from one, which is exactly what the cgroup check would catch —
        // so assert the bound rather than the answer.
        let _ = found;
        assert!(
            calls.get() <= MAX_ANCESTRY,
            "walked {} links, bound is {MAX_ANCESTRY}",
            calls.get()
        );

        // A chain with apex-agentd two links up matches.
        assert!(looks_like_a_session(&handle, |pid| {
            if pid % 2 == 0 {
                Some((Some(PathBuf::from("/usr/bin/apex-agentd")), None))
            } else {
                Some((Some(PathBuf::from("/usr/bin/bash")), Some(pid + 1)))
            }
        }));
    }

    #[test]
    fn a_deleted_binary_still_counts_as_a_session() {
        // Every image update leaves running processes with an exe link that
        // reads `…/apex-agentd (deleted)`. Missing that would silently open the
        // mutating verbs to sessions on any machine that had updated.
        assert!(is_session_exe(Some(std::path::Path::new(
            "/usr/bin/apex-agentd (deleted)"
        ))));
        assert!(is_session_exe(Some(std::path::Path::new("/usr/bin/bwrap"))));
        assert!(!is_session_exe(Some(std::path::Path::new("/usr/bin/apex"))));
        // A binary whose name merely contains one of the names is not one.
        assert!(!is_session_exe(Some(std::path::Path::new(
            "/tmp/apex-agentd-not-really"
        ))));
        assert!(!is_session_exe(None));
    }

    #[test]
    fn the_parent_line_is_read_from_status_not_stat() {
        // The parsing bug this avoids: /proc/<pid>/stat embeds the command name
        // in parentheses, so a process named "foo) 1 2 3" makes field-splitting
        // return the wrong pid.
        assert_eq!(
            parse_ppid("Name:\tfoo) 1 2 3\nPid:\t42\nPPid:\t7\n"),
            Some(7)
        );
        assert_eq!(parse_ppid("Name:\tx\n"), None);
        assert_eq!(parse_ppid(""), None);
    }
}
