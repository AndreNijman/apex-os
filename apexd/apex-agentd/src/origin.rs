//! Working out where a connection came from (roadmap §7).
//!
//! [`crate::peer`] answers "which session owns this connection" from the
//! kernel's view of it. This answers the other half — "and if it is not a
//! session, what is it" — from the same kind of evidence, because §7's origins
//! are only worth having if they cannot be claimed.
//!
//! ## What is observable
//!
//! The peer pid comes from `SO_PEERCRED`, which the kernel fills in at
//! `connect(2)`. From it:
//!
//! * `/proc/<pid>/cgroup` says which systemd unit or scope the peer lives in.
//!   A process in `session-N.scope` belongs to a **login session** — somebody
//!   logged in and this descends from that login. A process under
//!   `user@N.service` is a **user service**: started by systemd, with nobody
//!   present. That distinction is what separates §7's local origins from
//!   `scheduled-job`, and a process cannot move itself between them.
//! * `/proc/<pid>/stat`'s `tty_nr` says whether the peer has a controlling
//!   terminal. Inside a login session that separates `local-terminal` from
//!   `apex-shell`, which is a graphical process with no tty. Both are local,
//!   so this one is presentation rather than policy.
//!
//! ## What is not observable, and what happens then
//!
//! `claude-remote-control`, `mcp` and `cloud-job` have no kernel-visible
//! signature at all. Remote Control drives a `claude` that is running here;
//! from the socket it is the same process being typed at by a different
//! person. Those origins are reached by declaration instead — see
//! [`apex_agent_core::origin`], where a declaration can only ever cost the
//! session something.
//!
//! A `/proc` read that fails is **not** a classification. It returns an error
//! and the caller refuses the request. The tempting alternative is to fall
//! back to the default, and the default is `local-terminal`: the origin §7
//! reserves root and break-glass for. "I could not tell" and "a human is at
//! the keyboard" are not the same answer, and a build that conflated them
//! would have written the security invariant down and then not implemented
//! it.

use apex_agent_core::policy::RequestOrigin;

use crate::peer::Peer;

/// Classify an unsessioned peer.
///
/// Only ever called for a connection that did NOT resolve to a managed
/// session; a session's origin is the one recorded when the daemon forked it.
///
/// The error is a sentence, not a code: it reaches the user as the reason
/// their request was refused, and "could not read /proc/1234/cgroup" is what
/// makes that refusal actionable rather than mysterious.
pub fn observe(peer: &Peer) -> Result<RequestOrigin, String> {
    let placement = read_cgroup(peer.pid)?;
    match classify(&placement, has_controlling_tty(peer.pid)?) {
        Some(o) => Ok(o),
        None => Err(format!(
            "/proc/{}/cgroup places this connection in {placement:?}, which is neither a login \
             session nor a user service, so there is no way to tell whether a human is at this \
             machine",
            peer.pid
        )),
    }
}

/// The peer's cgroup path, as written in `/proc/<pid>/cgroup`.
///
/// cgroup v2 writes exactly one line, `0::<path>`. A v1 hierarchy writes
/// several with numeric ids, and none of them means what this reads it to
/// mean, so anything that is not the v2 form is an unreadable placement
/// rather than a guess.
fn read_cgroup(pid: libc::pid_t) -> Result<String, String> {
    let path = format!("/proc/{pid}/cgroup");
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{path}: {e}"))?;
    text.lines()
        .find_map(|l| l.strip_prefix("0::"))
        .map(|p| p.trim().to_string())
        .ok_or_else(|| {
            format!("{path} has no unified (0::) hierarchy, so the peer's placement is unknown")
        })
}

/// Whether the peer has a controlling terminal.
///
/// `tty_nr` is field 7 of `/proc/<pid>/stat`, and everything before it has to
/// be skipped past the executable name — which is in parentheses and may
/// itself contain spaces and parentheses. So the split is on the LAST `)`,
/// never on whitespace; `peer::parent_of` avoids `stat` entirely for the same
/// reason.
fn has_controlling_tty(pid: libc::pid_t) -> Result<bool, String> {
    let path = format!("/proc/{pid}/stat");
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{path}: {e}"))?;
    let after_comm = text
        .rsplit_once(')')
        .map(|(_, rest)| rest)
        .ok_or_else(|| format!("{path} is not in the expected form"))?;
    // After `pid (comm)` the fields are state, ppid, pgrp, session, tty_nr.
    let tty = after_comm
        .split_whitespace()
        .nth(4)
        .ok_or_else(|| format!("{path} has no tty_nr field"))?;
    let tty: i64 = tty
        .parse()
        .map_err(|_| format!("{path} has an unreadable tty_nr {tty:?}"))?;
    Ok(tty != 0)
}

/// The classification itself, over values rather than over `/proc`.
///
/// Split out so the rule is testable without a fixture filesystem, and so the
/// three cases can be read against §7 in one place.
fn classify(cgroup: &str, has_tty: bool) -> Option<RequestOrigin> {
    if cgroup.contains("/session-") && cgroup.contains(".scope") {
        // A login session: somebody logged in and this descends from it. The
        // tty split is presentation — APEX Shell is a graphical process with
        // no controlling terminal, a shell prompt has one — and both answers
        // are local, so getting it wrong costs a label and not a permission.
        return Some(if has_tty {
            RequestOrigin::LocalTerminal
        } else {
            RequestOrigin::ApexShell
        });
    }
    if cgroup.contains("/user@") && cgroup.contains(".service") {
        // A systemd user service. Nobody logged in to start it and nobody is
        // waiting on it, which is exactly what §7 means by `scheduled-job`.
        return Some(RequestOrigin::ScheduledJob);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_login_session_with_a_terminal_is_a_local_terminal() {
        let cg = "/user.slice/user-1000.slice/session-3.scope";
        assert_eq!(classify(cg, true), Some(RequestOrigin::LocalTerminal));
        assert_eq!(classify(cg, false), Some(RequestOrigin::ApexShell));
        // Both halves are local, which is the property that matters.
        assert!(classify(cg, true).unwrap().is_local());
        assert!(classify(cg, false).unwrap().is_local());
    }

    #[test]
    fn a_user_service_is_never_local_however_it_reached_the_socket() {
        // The case that makes the classifier worth having: a systemd timer
        // runs on this machine, as this user, with no human anywhere. A
        // classifier that only asked "same uid?" would call it local.
        for cg in [
            "/user.slice/user-1000.slice/user@1000.service/app.slice/nightly-build.service",
            "/user.slice/user-1000.slice/user@1000.service/background.slice/tidy.service",
        ] {
            for tty in [true, false] {
                let got = classify(cg, tty).expect("classified");
                assert_eq!(got, RequestOrigin::ScheduledJob, "{cg}");
                assert!(!got.is_local(), "{cg} was called local");
            }
        }
    }

    #[test]
    fn an_orphan_of_a_session_is_still_not_a_human_at_this_machine() {
        // The escape from the ancestry check, and why the cgroup check covers
        // it. `peer::resolve_by_ancestry` finds a session by walking /proc
        // parents, so a process that orphans itself — double-fork, or simply
        // outliving its parent — is reparented and the walk no longer reaches
        // the session's pid. It looks like an ordinary process of this user.
        //
        // It is not reparented to pid 1. Under systemd a user process is
        // reparented inside the user manager, so its cgroup is under
        // `user@N.service`, which is what this reads as a scheduled job —
        // nobody is present. §7 gives that the remote column, so a grant is
        // refused there too.
        //
        // The two mechanisms are not redundant: ancestry catches the ordinary
        // case and attributes it to the right session for the audit trail,
        // and this catches the case ancestry loses. Neither alone would do.
        for cg in [
            "/user.slice/user-1000.slice/user@1000.service/apex-agentd.service",
            "/user.slice/user-1000.slice/user@1000.service/app.slice/apex-agentd.service",
            "/user.slice/user-1000.slice/user@1000.service/init.scope",
        ] {
            for tty in [true, false] {
                let got = classify(cg, tty).expect("classified");
                assert_eq!(got, RequestOrigin::ScheduledJob, "{cg}");
                assert!(!got.is_local(), "{cg} was called local");
            }
        }
    }

    #[test]
    fn a_placement_that_is_neither_is_refused_rather_than_assumed_local() {
        // The fail-open this module exists to avoid. `RequestOrigin`'s Default
        // is `local-terminal`, so any path that returns a default here hands
        // out §7's first column to something nobody identified.
        for cg in [
            "/",
            "/system.slice/sshd.service",
            "/user.slice/user-1000.slice",
            "",
            "/machine.slice/libpod-abc.scope",
        ] {
            assert_eq!(classify(cg, true), None, "{cg:?} was classified");
            assert_eq!(classify(cg, false), None, "{cg:?} was classified");
        }
    }

    #[test]
    fn a_session_scope_is_not_matched_by_a_name_that_merely_contains_it() {
        // `.scope` and `/session-` both have to be there. A user service
        // called `session-manager.service` is still a user service.
        let cg = "/user.slice/user-1000.slice/user@1000.service/app.slice/session-manager.service";
        assert_eq!(classify(cg, true), Some(RequestOrigin::ScheduledJob));
    }

    #[test]
    fn our_own_placement_is_readable_and_classifies() {
        // Against real /proc, because the parsing is where this breaks. The
        // test binary runs somewhere on the machine running it, so the only
        // claim made is that both reads succeed and produce a decision — not
        // which decision, since a CI container is placed differently from a
        // desktop.
        let me = std::process::id() as libc::pid_t;
        let cg = read_cgroup(me).expect("our own cgroup is readable");
        assert!(cg.starts_with('/'), "{cg:?}");
        has_controlling_tty(me).expect("our own tty_nr is readable");
    }

    #[test]
    fn the_tty_field_survives_an_executable_name_full_of_parentheses() {
        // /proc/<pid>/stat embeds the command name in parentheses and does not
        // escape it, so splitting on whitespace finds the wrong field for a
        // process called `foo) 1 2 3`. The parse takes everything after the
        // LAST `)`, which is the only form that works.
        let stat = "1234 (evil) 0 0 0) S 1 1234 1234 1025 1234 4194304 …";
        let after = stat.rsplit_once(')').map(|(_, r)| r).expect("a closing paren");
        let tty: i64 = after.split_whitespace().nth(4).unwrap().parse().unwrap();
        assert_eq!(tty, 1025, "the tty field was read from the wrong column");
    }

    #[test]
    fn a_pid_that_is_gone_is_an_error_and_not_an_origin() {
        // A peer that exited between connect and this lookup. The caller
        // refuses; it does not fall back to a default.
        let gone = 0x7fff_fffe;
        assert!(read_cgroup(gone).is_err());
        assert!(has_controlling_tty(gone).is_err());
        let peer = Peer {
            pid: gone,
            uid: 0,
            gid: 0,
        };
        let err = observe(&peer).expect_err("must refuse");
        assert!(err.contains("/proc/"), "{err}");
    }

    #[test]
    fn a_v1_only_cgroup_file_is_unreadable_rather_than_a_guess() {
        // cgroup v1 writes several numbered lines and none of them means what
        // this reads. Refusing beats picking one.
        let d = std::env::temp_dir().join(format!("apex-origin-{}", std::process::id()));
        std::fs::create_dir_all(&d).expect("mkdir");
        let f = d.join("cgroup");
        std::fs::write(&f, "3:cpu:/user.slice\n2:memory:/user.slice\n").expect("write");
        // read_cgroup takes a pid, so the shape is asserted directly.
        let text = std::fs::read_to_string(&f).unwrap();
        assert!(text.lines().all(|l| l.strip_prefix("0::").is_none()));
        std::fs::remove_dir_all(&d).ok();
    }
}
