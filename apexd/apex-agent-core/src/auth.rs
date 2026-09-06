//! Proving a human is at this machine, before the root boundary moves.
//!
//! §3.4 and §7 both end in the same place: root capability and unsafe-everything
//! need "local auth". This module is the two halves of that — the decision,
//! which is a pure function, and the prompt, which is a process.
//!
//! ## The decision and the prompt are separated on purpose
//!
//! A password dialog cannot be unit-tested and a policy decision must be. So
//! [`required_for`] answers "does moving dimension 3 from here to there need a
//! human?" over values, with no I/O anywhere near it, and every rule about
//! when authentication is required is asserted against that function. The
//! [`Authenticator`] trait is the seam: `PolkitAuthenticator` runs `pkcheck`
//! in production, and the tests use a stub that never touches polkit. Nothing
//! in this repository's test suite can raise a prompt.
//!
//! ## Why the subject is the peer, and why that answers "outside the PTY"
//!
//! P0-007's fifth criterion is that the authentication prompt happens outside
//! the agent's terminal. That is not a property of where the dialog is drawn;
//! it is a property of *whose* authorisation is being checked.
//!
//! polkit evaluates an action against a **subject**, and dispatches the
//! resulting challenge to the authentication agent registered for that
//! subject's login session. Pass the agent's process as the subject and the
//! challenge goes wherever that process's session agent is. Pass the peer that
//! connected to the control socket — which the daemon has already established
//! is *not* inside any managed session (see [`crate::policy`] and
//! `apex-agentd`'s `privilege::origin`) — and the challenge goes to that
//! human's session agent, which is the desktop.
//!
//! So the ordering in the daemon is load-bearing, and it is: resolve the peer,
//! refuse it if it resolves to a session, refuse it if its origin is not
//! local, and only then authenticate. By the time `pkcheck` runs, the subject
//! is known to be a process outside every agent sandbox. A session cannot make
//! itself the subject, because the subject comes from `SO_PEERCRED` on the
//! connection and the kernel fills that in.
//!
//! ## `--process pid,start-time,uid`, never bare `pid`
//!
//! A bare pid is a use-after-free waiting to happen: the process can exit
//! between the daemon reading the credentials and polkit looking the pid up,
//! and the pid can be reused by then. The start time from `/proc/<pid>/stat`
//! pins the identity — a reused pid has a different one — and polkit's own
//! documentation says to pass it. `apex-secretd` pins a `/proc` dirfd at
//! accept for the same reason; this is the same defence in the vocabulary
//! polkit speaks.
//!
//! ## Exit 127
//!
//! P0-016 shipped a polkit action file that was not valid XML — it quoted a
//! command line inside an XML comment, and every flag starts with two hyphens,
//! which an XML comment may not contain. polkitd does not complain about an
//! action it cannot parse; it simply never registers it, and `pkcheck` then
//! exits 127 forever with no other symptom. That failure is named here by
//! name, because the next person to see a 127 should not have to rediscover
//! it, and the action files this module uses are validated with `xmllint` at
//! image build time so it cannot recur silently.

use crate::grant::GrantKind;
use crate::policy::SystemAccess;

/// The polkit action behind §4.4's session grant.
///
/// Its own action rather than a shared one, so an administrator can allow one
/// and not the other in a local rule, and so the dialog says which of the two
/// modes is being asked for.
pub const ACTION_SYSTEM_ACCESS: &str = "org.apexos.agent.system-access";

/// The polkit action behind §4.5's break-glass mode.
pub const ACTION_BREAK_GLASS: &str = "org.apexos.agent.break-glass";

/// What moving dimension 3 requires.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthRequirement {
    /// Nothing. Either nothing moved, or it moved down.
    None,
    /// A human at this machine, proved through this polkit action.
    LocalAuth { action: &'static str },
}

impl AuthRequirement {
    pub fn action(&self) -> Option<&'static str> {
        match self {
            AuthRequirement::None => None,
            AuthRequirement::LocalAuth { action } => Some(action),
        }
    }

    pub fn needs_a_human(&self) -> bool {
        matches!(self, AuthRequirement::LocalAuth { .. })
    }
}

/// What moving the system dimension from `from` to `to` requires.
///
/// The pure half, and the whole of the policy. Three rules:
///
/// * **Giving up privilege is free.** Moving to [`SystemAccess::None`] asks
///   for nothing, the same way P0-016's toggle asks for nothing to turn off.
///   A protection you have to authenticate to switch *on* is one people leave
///   on by accident.
/// * **Every step toward privilege authenticates**, including one between the
///   two elevated modes. `session` → `unsafe` is a different and larger grant
///   and gets its own prompt.
/// * **Renewal authenticates too.** `from == to` at an elevated value is a
///   renewal, and it is not free. The value of the prompt is that there is no
///   cached yes to inherit — which is also why the shipped action is
///   `auth_admin` and not `auth_admin_keep`.
pub fn required_for(from: SystemAccess, to: SystemAccess) -> AuthRequirement {
    let _ = from;
    match to {
        SystemAccess::None => AuthRequirement::None,
        SystemAccess::Session => AuthRequirement::LocalAuth {
            action: ACTION_SYSTEM_ACCESS,
        },
        SystemAccess::Unsafe => AuthRequirement::LocalAuth {
            action: ACTION_BREAK_GLASS,
        },
    }
}

/// The action a grant of `kind` is authorised by.
pub fn action_for(kind: GrantKind) -> &'static str {
    match kind {
        GrantKind::SystemAccess => ACTION_SYSTEM_ACCESS,
        GrantKind::BreakGlass => ACTION_BREAK_GLASS,
    }
}

/// A process polkit can be asked about.
///
/// The three parts polkit's `unix-process` subject takes. Built from the
/// kernel's view of a connection, never from anything a client sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcessSubject {
    pub pid: libc::pid_t,
    /// Field 22 of `/proc/<pid>/stat`, in clock ticks since boot. What stops
    /// a recycled pid from inheriting an authorisation.
    pub start_time: u64,
    pub uid: libc::uid_t,
}

impl ProcessSubject {
    /// Read the start time for `pid` and build a subject.
    ///
    /// Returns `None` when the process is gone, which the caller must treat as
    /// a refusal: a subject that cannot be pinned cannot be authenticated.
    pub fn for_pid(pid: libc::pid_t, uid: libc::uid_t) -> Option<ProcessSubject> {
        let text = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        Some(ProcessSubject {
            pid,
            start_time: parse_start_time(&text)?,
            uid,
        })
    }

    /// polkit's spelling of this subject.
    pub fn as_polkit(&self) -> String {
        format!("{},{},{}", self.pid, self.start_time, self.uid)
    }
}

/// Field 22 of `/proc/<pid>/stat`.
///
/// The split is on the LAST `)`, never on whitespace: the executable name is
/// in parentheses, is not escaped, and may itself contain spaces and
/// parentheses. `origin.rs` reads `tty_nr` the same way and for the same
/// reason. After that split the fields begin at `state`, which is field 3, so
/// field 22 is index 19.
fn parse_start_time(stat: &str) -> Option<u64> {
    stat.rsplit_once(')')
        .map(|(_, rest)| rest)?
        .split_whitespace()
        .nth(19)?
        .parse()
        .ok()
}

/// What polkit said.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// A human authenticated, or was already authorised by the shipped policy.
    Authorized,
    /// They cancelled, got it wrong, or the action forbids this subject.
    Refused,
}

/// Why an authentication could not be attempted, as distinct from being
/// refused.
///
/// Kept apart from [`Verdict::Refused`] because the remedies are different:
/// a refusal means the human said no, and every one of these means the machine
/// could not ask. Both end in the grant not being issued.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthError {
    /// `pkcheck` is not installed.
    NoPkcheck,
    /// polkit does not know this action — the P0-016 failure. See the module
    /// docs.
    ActionNotRegistered(String),
    /// The peer exited before it could be pinned, or `/proc` would not answer.
    SubjectUnreadable(String),
    /// Anything else, with what `pkcheck` said.
    Failed { code: Option<i32>, stderr: String },
}

impl std::fmt::Display for AuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AuthError::NoPkcheck => write!(
                f,
                "pkcheck is not installed, so there is no way to ask for a local password; \
                 install polkit"
            ),
            AuthError::ActionNotRegistered(action) => write!(
                f,
                "polkit does not know the action {action}, so it cannot be authorised. The \
                 action file belongs at /usr/share/polkit-1/actions/; polkit silently ignores \
                 one it cannot parse, so check it with `xmllint --noout --nonet` as well as \
                 checking it is installed"
            ),
            AuthError::SubjectUnreadable(why) => write!(
                f,
                "the process asking for this could not be identified to polkit ({why}), and an \
                 authentication that cannot name its subject authorises nothing"
            ),
            AuthError::Failed { code, stderr } => {
                write!(f, "pkcheck failed")?;
                if let Some(c) = code {
                    write!(f, " with status {c}")?;
                }
                if !stderr.trim().is_empty() {
                    write!(f, ": {}", stderr.trim())?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for AuthError {}

/// Ask a human.
///
/// A trait with exactly one production implementation, so the daemon's grant
/// path can be exercised without a password dialog. Every test in this
/// repository uses a stub; nothing in `cargo test` can raise a prompt.
pub trait Authenticator: Send + Sync {
    fn check(&self, action: &str, subject: &ProcessSubject) -> Result<Verdict, AuthError>;
}

/// The real one: `pkcheck`, with interaction allowed.
#[derive(Debug, Default, Clone, Copy)]
pub struct PolkitAuthenticator;

/// The `pkcheck` command line for one check.
///
/// Built by a pure function so the flags are asserted without running
/// anything. `--allow-user-interaction` is what makes polkit raise a dialog
/// rather than answering "not authorized, but could be" — the whole point of
/// this path is that a human is asked.
pub fn pkcheck_argv(action: &str, subject: &ProcessSubject) -> Vec<String> {
    vec![
        "--action-id".to_string(),
        action.to_string(),
        "--process".to_string(),
        subject.as_polkit(),
        "--allow-user-interaction".to_string(),
    ]
}

/// Map `pkcheck`'s exit status to a verdict.
///
/// Separated from running it, because the mapping is the part that can be
/// wrong. 0 is authorised and 1 is not; 127 is the unregistered-action case
/// the module docs describe; anything else is a failure that must not be read
/// as either answer.
pub fn verdict_from_status(code: Option<i32>, action: &str, stderr: &str) -> Result<Verdict, AuthError> {
    match code {
        Some(0) => Ok(Verdict::Authorized),
        Some(1) => Ok(Verdict::Refused),
        Some(127) => Err(AuthError::ActionNotRegistered(action.to_string())),
        other => Err(AuthError::Failed {
            code: other,
            stderr: stderr.to_string(),
        }),
    }
}

impl Authenticator for PolkitAuthenticator {
    fn check(&self, action: &str, subject: &ProcessSubject) -> Result<Verdict, AuthError> {
        let out = std::process::Command::new("pkcheck")
            .args(pkcheck_argv(action, subject))
            .output();
        let out = match out {
            Ok(o) => o,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(AuthError::NoPkcheck),
            Err(e) => {
                return Err(AuthError::Failed {
                    code: None,
                    stderr: e.to_string(),
                })
            }
        };
        verdict_from_status(
            out.status.code(),
            action,
            &String::from_utf8_lossy(&out.stderr),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn giving_up_privilege_asks_for_nothing() {
        // P0-016's rule, generalised: turning a protection back on is
        // immediate. Over every starting point, so no path to `none`
        // authenticates.
        for from in SystemAccess::ALL {
            let r = required_for(*from, SystemAccess::None);
            assert_eq!(r, AuthRequirement::None, "{from} -> none");
            assert!(!r.needs_a_human());
        }
    }

    #[test]
    fn every_step_toward_privilege_authenticates_including_a_renewal() {
        // The rule with teeth. `from` is deliberately not consulted: a
        // session that already has a grant does not get a cheaper renewal,
        // because the value of the prompt is that there is no standing yes.
        for from in SystemAccess::ALL {
            for to in [SystemAccess::Session, SystemAccess::Unsafe] {
                let r = required_for(*from, to);
                assert!(r.needs_a_human(), "{from} -> {to} was free");
            }
        }
        // Including the renewal of the same value, which is what P0-007's
        // fourth criterion is about at the policy level.
        assert!(required_for(SystemAccess::Unsafe, SystemAccess::Unsafe).needs_a_human());
        assert!(required_for(SystemAccess::Session, SystemAccess::Session).needs_a_human());
    }

    #[test]
    fn the_two_modes_are_authorised_by_two_different_actions() {
        // So a local polkit rule can allow one without the other, and so the
        // dialog names the mode being asked for. Break-glass borrowing the
        // session grant's action would let an administrator who permitted the
        // smaller thing permit the larger one by accident.
        assert_eq!(
            required_for(SystemAccess::None, SystemAccess::Session).action(),
            Some(ACTION_SYSTEM_ACCESS)
        );
        assert_eq!(
            required_for(SystemAccess::None, SystemAccess::Unsafe).action(),
            Some(ACTION_BREAK_GLASS)
        );
        assert_ne!(ACTION_SYSTEM_ACCESS, ACTION_BREAK_GLASS);
        for kind in GrantKind::ALL {
            assert_eq!(action_for(*kind), required_for(
                SystemAccess::None,
                kind.system_access()
            ).action().expect("an action"));
        }
    }

    #[test]
    fn the_subject_is_pinned_by_start_time_not_by_pid_alone() {
        // A bare pid can be recycled between the daemon reading peer
        // credentials and polkit looking it up, and an authorisation granted
        // to a recycled pid is an authorisation granted to whoever got there
        // next.
        let s = ProcessSubject {
            pid: 4242,
            start_time: 987_654,
            uid: 1000,
        };
        assert_eq!(s.as_polkit(), "4242,987654,1000");
        let argv = pkcheck_argv(ACTION_BREAK_GLASS, &s);
        assert_eq!(
            argv,
            vec![
                "--action-id",
                "org.apexos.agent.break-glass",
                "--process",
                "4242,987654,1000",
                "--allow-user-interaction",
            ]
        );
        // Without interaction polkit answers "could be authorised" and never
        // asks, which would make this an authentication that authenticates
        // nobody.
        assert!(argv.iter().any(|a| a == "--allow-user-interaction"));
    }

    #[test]
    fn the_start_time_survives_an_executable_name_full_of_parentheses() {
        // /proc/<pid>/stat does not escape the command name, so splitting on
        // whitespace reads the wrong column for a process called `foo) 1 2`.
        // Fields after the last `)` start at `state`, field 3, so field 22 is
        // index 19. Built here with the columns numbered so the off-by-one is
        // visible.
        let after: Vec<String> = (3..=25).map(|n| n.to_string()).collect();
        let stat = format!("1234 (evil) 1 2 3) S {}", after[1..].join(" "));
        // The synthetic fields carry their own numbers, so field 22 reads 22.
        assert_eq!(parse_start_time(&stat), Some(22));
        assert_eq!(parse_start_time("no parens here"), None);
        assert_eq!(parse_start_time("1 (x) S 1 2 3"), None);
    }

    #[test]
    fn our_own_process_can_be_pinned() {
        // Against real /proc, because the parse is where this breaks.
        let me = std::process::id() as libc::pid_t;
        let uid = unsafe { libc::getuid() };
        let s = ProcessSubject::for_pid(me, uid).expect("our own subject");
        assert_eq!(s.pid, me);
        assert!(s.start_time > 0);
        // A pid that is gone cannot be pinned, and the caller refuses.
        assert_eq!(ProcessSubject::for_pid(0x7fff_fffe, uid), None);
    }

    #[test]
    fn only_zero_is_authorised_and_only_one_is_a_refusal() {
        // The mapping is the part that can be wrong, so it is asserted
        // exhaustively over the statuses that mean something and by class for
        // the rest. In particular an unrecognised status must not read as
        // either answer: a failed check that returned `Authorized` would hand
        // out root on a polkit that crashed.
        assert_eq!(
            verdict_from_status(Some(0), ACTION_BREAK_GLASS, ""),
            Ok(Verdict::Authorized)
        );
        assert_eq!(
            verdict_from_status(Some(1), ACTION_BREAK_GLASS, ""),
            Ok(Verdict::Refused)
        );
        for other in [2, 3, 4, 126, 255] {
            assert!(
                verdict_from_status(Some(other), ACTION_BREAK_GLASS, "boom").is_err(),
                "status {other} was read as an answer"
            );
        }
        // Killed by a signal: no status at all, and still not an answer.
        assert!(verdict_from_status(None, ACTION_BREAK_GLASS, "").is_err());
    }

    #[test]
    fn an_unregistered_action_is_named_as_such_and_not_as_a_refusal() {
        // P0-016's failure, which cost a working feature and produced no
        // other symptom. A 127 read as "the user said no" would send the next
        // person looking at their password instead of at the action file.
        let err = verdict_from_status(Some(127), ACTION_BREAK_GLASS, "")
            .expect_err("127 is not a verdict");
        assert_eq!(err, AuthError::ActionNotRegistered(ACTION_BREAK_GLASS.into()));
        let msg = err.to_string();
        assert!(msg.contains("/usr/share/polkit-1/actions/"), "{msg}");
        assert!(msg.contains("xmllint"), "{msg}");
    }

    /// The stub every test that needs a verdict uses.
    ///
    /// The existence of this type is what makes the rule "no test in this
    /// repository can raise a polkit prompt" enforceable rather than a
    /// convention.
    struct Stub(Result<Verdict, AuthError>);

    impl Authenticator for Stub {
        fn check(&self, _action: &str, _subject: &ProcessSubject) -> Result<Verdict, AuthError> {
            self.0.clone()
        }
    }

    #[test]
    fn the_trait_is_what_keeps_a_password_dialog_out_of_the_test_suite() {
        let subject = ProcessSubject {
            pid: 1,
            start_time: 1,
            uid: 0,
        };
        let yes = Stub(Ok(Verdict::Authorized));
        let no = Stub(Ok(Verdict::Refused));
        let broken = Stub(Err(AuthError::NoPkcheck));
        assert_eq!(yes.check(ACTION_BREAK_GLASS, &subject), Ok(Verdict::Authorized));
        assert_eq!(no.check(ACTION_BREAK_GLASS, &subject), Ok(Verdict::Refused));
        assert!(broken.check(ACTION_BREAK_GLASS, &subject).is_err());
    }
}
