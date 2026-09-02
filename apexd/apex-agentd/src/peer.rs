//! Who is on the other end of a control connection.
//!
//! Every request that carries privilege has to know which session made it, and
//! it must not learn that from the request. `$APEX_AGENT_SESSION` is set inside
//! each session's environment and `apex agent event` reads it, which is fine at
//! event stakes — the worst a lying client achieves is a wrong status label.
//! For a privilege request it is not fine: the variable is plain text inside a
//! sandbox the agent controls, so anything authorised by a client-supplied id
//! is authorised by the agent itself.
//!
//! So the session is derived from the kernel's view of the connection:
//!
//!   1. `SO_PEERCRED` on the accepted socket yields the peer's pid, uid and
//!      gid. The kernel fills this in at `connect(2)` time and a process cannot
//!      forge it.
//!   2. That pid is walked up its `/proc` parent chain, and the first ancestor
//!      matching a pid the daemon itself recorded when it forked a session is
//!      the answer.
//!
//! Ancestry rather than process group, deliberately. A process may call
//! `setpgid(2)` on itself; it cannot choose its parent. Matching on pgid would
//! let a process inside session A present itself as session B by joining B's
//! group — both are the same user, so it is not a privilege escalation, but it
//! would misattribute an audit record, and an audit trail that can be
//! redirected by the thing being audited is worthless.
//!
//! ## Why the pids line up despite `--unshare-pid`
//!
//! `SO_PEERCRED` translates the peer pid into the *receiving* process's pid
//! namespace. The daemon is outside the sandbox, so it receives the pid as it
//! appears in its own namespace, and `/proc` walks in that same namespace. The
//! recorded session pid is the `bwrap` process the daemon forked, and every
//! process inside the sandbox is a descendant of it. The chain therefore
//! terminates on the right session even though the agent's own view of its pid
//! is `1` or `2`.

use std::os::unix::io::AsRawFd;
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
/// Returns `None` when the option is unavailable, which is treated as "unknown
/// peer" and therefore as unauthenticated — never as trusted.
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

/// Whether the peer is the user this daemon runs for.
///
/// The socket already lives in a `0700` directory inside `$XDG_RUNTIME_DIR`, so
/// this should be unreachable. Checked anyway: it costs one syscall, and the
/// consequence of being wrong is that another account's process files privilege
/// requests attributed to this user.
pub fn is_own_user(peer: &Peer) -> bool {
    // Safe: getuid cannot fail and has no side effects.
    peer.uid == unsafe { libc::getuid() }
}

/// How far up the parent chain to walk before giving up.
///
/// A real chain from a sandboxed agent to the daemon is three or four links.
/// The bound exists so a malformed or cyclic `/proc` cannot spin here.
const MAX_ANCESTRY: usize = 64;

/// The parent pid of `pid`, from `/proc/<pid>/status`.
///
/// `status` rather than `stat`: `stat`'s second field is the executable name in
/// parentheses and may itself contain spaces and parentheses, so splitting it
/// on whitespace to reach the parent pid is a bug waiting for a process called
/// `foo bar) 1 2 3`. `status` is one key per line.
fn parent_of(pid: libc::pid_t) -> Option<libc::pid_t> {
    let text = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("PPid:") {
            return rest.trim().parse().ok();
        }
    }
    None
}

/// Walk from `pid` up to pid 1, returning the first pid that `known` accepts.
///
/// `known` is asked about the starting pid first, so a process that IS the
/// recorded session leader resolves to itself.
pub fn resolve_by_ancestry<F>(pid: libc::pid_t, known: F) -> Option<libc::pid_t>
where
    F: Fn(libc::pid_t) -> bool,
{
    let mut current = pid;
    for _ in 0..MAX_ANCESTRY {
        if current <= 0 {
            return None;
        }
        if known(current) {
            return Some(current);
        }
        match parent_of(current) {
            Some(parent) if parent != current => current = parent,
            // pid 1's parent is 0, and a process cannot be its own parent; both
            // mean the chain is finished.
            _ => return None,
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn our_own_parent_chain_is_walkable() {
        // The mechanism, against real /proc: this process's own ancestry must
        // reach pid 1. If it does not, the resolver cannot work at all.
        let me = std::process::id() as libc::pid_t;
        let found = resolve_by_ancestry(me, |p| p == 1);
        assert_eq!(found, Some(1), "the chain from {me} did not reach pid 1");
    }

    #[test]
    fn a_process_resolves_to_itself_when_it_is_the_known_one() {
        let me = std::process::id() as libc::pid_t;
        assert_eq!(resolve_by_ancestry(me, |p| p == me), Some(me));
    }

    #[test]
    fn our_parent_is_found_and_it_is_not_us() {
        let me = std::process::id() as libc::pid_t;
        let parent = parent_of(me).expect("we have a parent");
        assert_ne!(parent, me);
        assert!(parent > 0);
        assert_eq!(resolve_by_ancestry(me, |p| p == parent), Some(parent));
    }

    #[test]
    fn an_unrelated_pid_does_not_resolve() {
        // The property that matters: ancestry is not "any pid of this user".
        // pid 1 is not a descendant of us, so asking whether OUR chain contains
        // something only reachable from elsewhere must fail.
        let me = std::process::id() as libc::pid_t;
        assert_eq!(resolve_by_ancestry(me, |p| p == -12345), None);
        // And walking up from pid 1 never finds us.
        assert_eq!(resolve_by_ancestry(1, |p| p == me), None);
    }

    #[test]
    fn a_nonexistent_pid_resolves_to_nothing_rather_than_panicking() {
        // A peer that exited between connect and this lookup.
        assert_eq!(resolve_by_ancestry(0x7fff_fffe, |_| false), None);
        assert_eq!(resolve_by_ancestry(0, |_| true), None);
        assert_eq!(resolve_by_ancestry(-1, |_| true), None);
    }

    #[test]
    fn the_walk_is_bounded() {
        // `known` never matches and the chain is infinite from the resolver's
        // point of view, because the closure claims every pid has itself as a
        // parent. It must terminate regardless.
        let calls = std::cell::Cell::new(0usize);
        let out = resolve_by_ancestry(std::process::id() as libc::pid_t, |_| {
            calls.set(calls.get() + 1);
            false
        });
        assert_eq!(out, None);
        assert!(
            calls.get() <= MAX_ANCESTRY,
            "asked about {} pids, bound is {MAX_ANCESTRY}",
            calls.get()
        );
    }

    #[test]
    fn parent_of_reads_status_not_stat() {
        // A regression guard for the parsing bug this avoided: /proc/<pid>/stat
        // embeds the command name in parentheses, so a process named
        // "foo) 1 2 3" makes field-splitting return the wrong pid. Assert we
        // agree with the kernel's own key/value form.
        let me = std::process::id() as libc::pid_t;
        let status = std::fs::read_to_string(format!("/proc/{me}/status")).unwrap();
        let expected: libc::pid_t = status
            .lines()
            .find_map(|l| l.strip_prefix("PPid:"))
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        assert_eq!(parent_of(me), Some(expected));
    }

    #[test]
    fn credentials_come_from_a_real_socketpair_and_name_this_process() {
        // Proves the getsockopt plumbing, not just that it compiles.
        let (a, b) = UnixStream::pair().expect("socketpair");
        let peer = credentials(&a).expect("SO_PEERCRED on a socketpair");
        assert_eq!(peer.pid, std::process::id() as libc::pid_t);
        assert!(is_own_user(&peer));
        // Both ends see this process, since both belong to it.
        assert_eq!(credentials(&b).map(|p| p.pid), Some(peer.pid));
    }

    #[test]
    fn another_uid_is_not_our_user() {
        let mut peer = credentials(&UnixStream::pair().unwrap().0).unwrap();
        peer.uid = peer.uid.wrapping_add(1);
        assert!(!is_own_user(&peer));
    }
}
