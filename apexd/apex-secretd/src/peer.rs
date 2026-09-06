//! Who is on the other end, and how much of that the kernel actually vouches for.
//!
//! `SO_PEERCRED` yields three numbers: uid, gid and pid. They are not worth the
//! same, and treating them as if they were is how a service ends up authorising
//! on something forgeable.
//!
//! # uid — authoritative
//!
//! The kernel writes it at `connect(2)`. It cannot change for the life of the
//! socket and a client cannot choose it. Every authorisation decision in this
//! service rests on it and on nothing else: the uid selects the namespace whose
//! credentials and grants apply.
//!
//! # pid — metadata, and here is the failure mode
//!
//! The pid is real at `connect(2)` and then starts decaying. The peer may exit
//! a microsecond later and its pid may be reused by an unrelated process before
//! the daemon looks at `/proc`. Anything derived from the pid *after* the fact
//! — "which session is this", "what is its working directory" — can therefore
//! be answered about the wrong process.
//!
//! So this service does not derive anything from the pid. It records it, and it
//! pins it: [`Peer::pin`] reads the process start time from `/proc/<pid>/stat`
//! at accept, and [`Peer::still_pinned`] re-reads it before each request. A pid
//! that has been reused has a different start time, and the record then says
//! "the peer is gone" rather than naming a stranger. The start time is the
//! standard defence because it is the one field a reused pid cannot match.
//!
//! Two honest limits. First, this makes the *audit* accurate; it is not a
//! privilege boundary, because the uid never depended on the pid. Second, the
//! window between `connect(2)` and the first `stat` read is unclosable from
//! userspace — `SO_PEERPIDFD` closes it, and needs a kernel and a `libc` this
//! workspace does not yet pin. Noted rather than pretended away.
//!
//! # What the pid is NOT used for
//!
//! Resolving an agent session. `apex-agentd` can do that because it forked the
//! sandbox and owns the pid table for it. This daemon runs under a different
//! uid, cannot read another user's `/proc/<pid>/cwd`, and has no session table.
//! Project and session therefore arrive as claims — see
//! `apex_secret_core::capability::Claimed`.

use std::os::unix::io::AsRawFd;
use std::os::unix::net::UnixStream;

/// The other end of a connection, as far as it can be established.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Peer {
    /// Authoritative. The basis of every decision.
    pub uid: libc::uid_t,
    pub gid: libc::gid_t,
    /// Metadata. Accurate at accept, pinned by `starttime` thereafter.
    pub pid: libc::pid_t,
    /// The peer's process start time in clock ticks since boot, or `None` when
    /// `/proc` would not answer — a process that had already exited.
    starttime: Option<u64>,
}

impl Peer {
    /// Read `SO_PEERCRED` and pin the peer's start time.
    ///
    /// `None` means the kernel would not report credentials, which is treated
    /// as an unknown and therefore unauthorised peer — never as a trusted one.
    pub fn pin(stream: &UnixStream) -> Option<Peer> {
        let mut cred = libc::ucred {
            pid: 0,
            uid: 0,
            gid: 0,
        };
        let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
        // Safe: getsockopt writes at most `len` bytes into `cred`, which is
        // owned here and exactly that size, and the fd is valid for the borrow.
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
            uid: cred.uid,
            gid: cred.gid,
            pid: cred.pid,
            starttime: starttime(cred.pid),
        })
    }

    /// Whether the process this connection was accepted from is still the one
    /// running under that pid.
    ///
    /// False after the peer exits, and false after its pid is reused. Used to
    /// decide whether the pid is worth writing into an audit record, not to
    /// decide whether the request is allowed — the uid settles that and does not
    /// decay.
    pub fn still_pinned(&self) -> bool {
        match self.starttime {
            Some(pinned) => starttime(self.pid) == Some(pinned),
            // No start time was readable at accept, so there is nothing to
            // compare against and nothing to claim.
            None => false,
        }
    }

    /// The pid to record: the real one while it is still that process, and `0`
    /// once it cannot be vouched for.
    ///
    /// `0` rather than the stale number on purpose. A record naming a pid that
    /// now belongs to somebody else is worse than a record admitting it does
    /// not know.
    pub fn audit_pid(&self) -> libc::pid_t {
        if self.still_pinned() {
            self.pid
        } else {
            0
        }
    }

    /// Whether this peer may use the admin socket.
    ///
    /// Root, or the uid the daemon itself runs as. The second is not a
    /// loophole: that uid already owns every file in the store and can read and
    /// rewrite it directly, so reaching it through the socket grants nothing
    /// new. It is what lets the daemon be run by an ordinary user in a
    /// temporary directory, which is how the wire test drives it.
    ///
    /// In the packaged configuration the daemon runs as `apex-secret`, an
    /// account with no shell and no login, so in practice this reads "root".
    pub fn may_administer(&self) -> bool {
        // Safe: geteuid cannot fail and has no side effects.
        self.uid == 0 || self.uid == unsafe { libc::geteuid() }
    }
}

/// Field 22 of `/proc/<pid>/stat`: the process start time.
///
/// Parsed from after the last `)` rather than by splitting the whole line.
/// Field 2 is the executable name in parentheses and may itself contain spaces
/// and parentheses, so a process called `foo) 1 2 3` breaks a naive split — the
/// same trap `apex-agentd`'s peer module documents for the parent pid.
fn starttime(pid: libc::pid_t) -> Option<u64> {
    if pid <= 0 {
        return None;
    }
    let text = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let after = text.rfind(')')?;
    // Fields from here are: state(3) ppid(4) ... starttime(22). After the
    // closing parenthesis the next token is field 3, so start time is the 20th.
    text[after + 1..].split_whitespace().nth(19)?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credentials_come_from_a_real_socket_and_name_this_process() {
        let (a, b) = UnixStream::pair().expect("socketpair");
        let peer = Peer::pin(&a).expect("SO_PEERCRED on a socketpair");
        assert_eq!(peer.pid, std::process::id() as libc::pid_t);
        // Safe: getuid has no side effects.
        assert_eq!(peer.uid, unsafe { libc::getuid() });
        assert_eq!(Peer::pin(&b).map(|p| p.pid), Some(peer.pid));
    }

    #[test]
    fn a_live_peer_stays_pinned_and_is_worth_recording() {
        let (a, _b) = UnixStream::pair().unwrap();
        let peer = Peer::pin(&a).unwrap();
        assert!(peer.still_pinned(), "this process is still running");
        assert_eq!(peer.audit_pid(), peer.pid);
    }

    #[test]
    fn a_reused_pid_does_not_get_recorded_as_the_original_peer() {
        // The failure this guards. Same pid, different process: the start time
        // differs, so the pin fails and the audit record says 0 rather than
        // naming a stranger.
        let (a, _b) = UnixStream::pair().unwrap();
        let mut peer = Peer::pin(&a).unwrap();
        peer.starttime = Some(peer.starttime.unwrap().wrapping_add(1));
        assert!(!peer.still_pinned());
        assert_eq!(peer.audit_pid(), 0);
        // The uid is untouched by any of this, which is why authorisation is
        // not affected.
        assert_eq!(peer.uid, unsafe { libc::getuid() });
    }

    #[test]
    fn a_peer_that_has_exited_is_not_pinned() {
        let (a, _b) = UnixStream::pair().unwrap();
        let mut peer = Peer::pin(&a).unwrap();
        peer.pid = 0x7fff_fffe; // no such process
        assert!(!peer.still_pinned());
        assert_eq!(peer.audit_pid(), 0);

        // And a peer whose start time was never readable claims nothing.
        peer.starttime = None;
        assert!(!peer.still_pinned());
    }

    #[test]
    fn the_start_time_is_read_from_after_the_command_name() {
        // The parse this uses, against real /proc. A process named "foo) 1 2 3"
        // would break a whitespace split of the whole line; taking everything
        // after the LAST ')' cannot be fooled that way.
        let me = std::process::id() as libc::pid_t;
        let mine = starttime(me).expect("our own start time");
        assert!(mine > 0);
        // Stable across reads — it is a boot-relative constant for a process.
        assert_eq!(starttime(me), Some(mine));
        // pid 1 started before us, or at the same tick on a very fast boot.
        let init = starttime(1).expect("pid 1 has a start time");
        assert!(init <= mine, "init {init} started after us {mine}");
    }

    #[test]
    fn an_impossible_pid_yields_nothing_rather_than_panicking() {
        assert_eq!(starttime(0), None);
        assert_eq!(starttime(-1), None);
        assert_eq!(starttime(0x7fff_fffe), None);
    }

    #[test]
    fn this_process_may_administer_its_own_daemon_and_a_stranger_may_not() {
        let (a, _b) = UnixStream::pair().unwrap();
        let mut peer = Peer::pin(&a).unwrap();
        assert!(peer.may_administer(), "the daemon's own uid administers it");

        peer.uid = 0;
        assert!(peer.may_administer(), "root administers it");

        // Some other account. Under the packaged unit this is every human user.
        peer.uid = unsafe { libc::geteuid() }.wrapping_add(1);
        if peer.uid != 0 {
            assert!(!peer.may_administer(), "uid {} must not administer", peer.uid);
        }
    }
}
