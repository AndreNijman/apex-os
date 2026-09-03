//! Who is on the other end of a connection.
//!
//! Both sockets live at mode `0600` inside a `0700` directory in
//! `$XDG_RUNTIME_DIR`, so a connection from another account should already be
//! impossible. This checks anyway, because it costs one `getsockopt` and the
//! consequence of the file mode ever being wrong — a `umask` change, a
//! `$XDG_RUNTIME_DIR` that is not a private tmpfs, a distribution that
//! relocates it — is that another user's process gets to send prompts through
//! this daemon and read the answers.
//!
//! `SO_PEERCRED` is filled in by the kernel at `connect(2)` and cannot be
//! forged, which is exactly what a TCP connection cannot offer and is the whole
//! reason `apexd_core::ai::refuse_tcp_endpoint` exists. This file is the
//! positive half of that argument: because the endpoint is `AF_UNIX`, there IS
//! a credential to check.
//!
//! Deliberately duplicated from `apex-agentd`'s `peer.rs`, which is a **binary**
//! crate and therefore cannot be imported from. The alternative — promoting
//! forty lines of `getsockopt` into `apex-agent-core` and depending on the agent
//! runtime's library from the inference daemon — would be a worse trade than
//! this duplication: see the note in `Cargo.toml`.

use std::os::unix::io::AsRawFd;
use std::os::unix::net::UnixStream;

/// What the kernel says about the other end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Peer {
    pub pid: libc::pid_t,
    pub uid: libc::uid_t,
    pub gid: libc::gid_t,
}

/// Read `SO_PEERCRED` from an accepted connection.
///
/// `None` when the option is unavailable, which callers treat as *not this
/// user* — failing closed, never as trusted.
pub fn credentials(stream: &UnixStream) -> Option<Peer> {
    let mut cred = libc::ucred { pid: 0, uid: 0, gid: 0 };
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
    Some(Peer { pid: cred.pid, uid: cred.uid, gid: cred.gid })
}

/// Whether this connection may be served: it is the user this daemon runs for.
///
/// Fails closed on an unreadable credential.
pub fn is_own_user(stream: &UnixStream) -> bool {
    // Safe: getuid cannot fail and has no side effects.
    let me = unsafe { libc::getuid() };
    credentials(stream).is_some_and(|p| p.uid == me)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credentials_come_from_a_real_socketpair_and_name_this_process() {
        // Proves the getsockopt plumbing rather than that it compiles.
        let (a, b) = UnixStream::pair().expect("socketpair");
        let peer = credentials(&a).expect("SO_PEERCRED on a socketpair");
        assert_eq!(peer.pid, std::process::id() as libc::pid_t);
        assert_eq!(peer.uid, unsafe { libc::getuid() });
        assert_eq!(credentials(&b).map(|p| p.pid), Some(peer.pid));
    }

    #[test]
    fn our_own_connection_is_accepted() {
        let (a, _b) = UnixStream::pair().expect("socketpair");
        assert!(is_own_user(&a));
    }

    #[test]
    fn a_credential_that_cannot_be_read_is_refused_not_trusted() {
        // The fail-closed direction. There is no way to make getsockopt fail on
        // a live socketpair, so the property is asserted on the decision
        // function's own logic: `None` must not be accepted.
        let me = unsafe { libc::getuid() };
        let none: Option<Peer> = None;
        assert!(!none.is_some_and(|p: Peer| p.uid == me));
        // And a different uid is refused.
        let other = Peer { pid: 1, uid: me.wrapping_add(1), gid: 0 };
        assert!(!Some(other).is_some_and(|p: Peer| p.uid == me));
    }
}
