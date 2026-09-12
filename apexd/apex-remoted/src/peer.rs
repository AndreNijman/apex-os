//! Who is on the other end of the local control socket.
//!
//! The same `SO_PEERCRED` read `apex-agentd` does, and deliberately a second
//! small copy rather than a shared crate: it is twenty lines of `getsockopt`,
//! and the alternative was to promote `apex-agentd`'s private `peer` module
//! into `apex-agent-core`, which would put the agent runtime's session
//! resolution — the `/proc` ancestry walk, the registry lookup — into a
//! library this service has no business linking.
//!
//! What *is* shared is the part that matters: the classification of a pid
//! into a §7 origin lives in `apex_agent_core::origin`, so the two daemons
//! cannot disagree about who counts as a human at this machine.

use std::os::unix::io::AsRawFd;
use std::os::unix::net::UnixStream;

/// What the kernel says about the other end of a connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Peer {
    pub pid: libc::pid_t,
    pub uid: libc::uid_t,
    pub gid: libc::gid_t,
}

/// Read `SO_PEERCRED`, or `None`.
///
/// `None` is treated as an unknown peer and therefore as unauthenticated —
/// never as trusted.
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

/// Whether the peer is the user this service runs for.
///
/// The socket already lives in a 0700 directory inside `$XDG_RUNTIME_DIR`, so
/// this should be unreachable. Checked anyway: it costs one syscall, and the
/// consequence of being wrong is another account pairing a device with this
/// user's agents.
pub fn is_own_user(peer: &Peer) -> bool {
    // Safe: getuid cannot fail and has no side effects.
    peer.uid == unsafe { libc::getuid() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credentials_come_from_a_real_socketpair_and_name_this_process() {
        let (a, b) = UnixStream::pair().expect("socketpair");
        let peer = credentials(&a).expect("SO_PEERCRED on a socketpair");
        assert_eq!(peer.pid, std::process::id() as libc::pid_t);
        assert!(is_own_user(&peer));
        assert_eq!(credentials(&b).map(|p| p.pid), Some(peer.pid));
    }

    #[test]
    fn another_uid_is_not_our_user() {
        let mut peer = credentials(&UnixStream::pair().unwrap().0).unwrap();
        peer.uid = peer.uid.wrapping_add(1);
        assert!(!is_own_user(&peer));
    }
}
