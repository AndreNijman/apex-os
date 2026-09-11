//! Working out where a connection came from (roadmap §7).
//!
//! [`crate::peer`] answers "which session owns this connection" from the
//! kernel's view of it. This answers the other half — "and if it is not a
//! session, what is it" — from the same kind of evidence, because §7's origins
//! are only worth having if they cannot be claimed.
//!
//! ## Where the rule lives, and why not here
//!
//! The classification itself moved to [`apex_agent_core::origin`], which is
//! where the §7 vocabulary and the declaration rule already are. It is not
//! only the agent runtime's question: `apex-remoted` asks the same one about
//! the caller of `apex remote pair`, because "may this caller do something §7
//! reserves for a human at this machine" has exactly one right answer per
//! process and two implementations of it would be two answers waiting to
//! disagree.
//!
//! What stays here is the one line that turns this daemon's `Peer` into that
//! question, and the reason the answer is never guessed at: a `/proc` read
//! that fails is an error, not a fallback to the default — and the default is
//! `local-terminal`, the origin §7 reserves root and break-glass for.

use apex_agent_core::policy::RequestOrigin;

use crate::peer::Peer;

/// Classify an unsessioned peer.
///
/// Only ever called for a connection that did NOT resolve to a managed
/// session; a session's origin is the one recorded when the daemon forked it.
pub fn observe(peer: &Peer) -> Result<RequestOrigin, String> {
    apex_agent_core::origin::observe_pid(peer.pid)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_process_classifies_the_same_way_through_both_doors() {
        // The wrapper is one line and this is what keeps it one line: the
        // daemon's answer about a peer and the shared answer about that pid
        // have to be the same value, or `apex-remoted` and `apex-agentd`
        // would disagree about who is a human at this machine.
        let me = Peer {
            pid: std::process::id() as libc::pid_t,
            uid: 0,
            gid: 0,
        };
        assert_eq!(
            observe(&me),
            apex_agent_core::origin::observe_pid(me.pid),
        );
    }

    #[test]
    fn a_pid_that_is_gone_is_an_error_naming_what_could_not_be_read() {
        let gone = Peer {
            pid: 0x7fff_fffe,
            uid: 0,
            gid: 0,
        };
        let why = observe(&gone).expect_err("a dead pid must not classify");
        assert!(why.contains("/proc/"), "{why}");
    }
}
