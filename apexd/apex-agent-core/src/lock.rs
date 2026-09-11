//! What a screen lock means for a running session (roadmap §7).
//!
//! §7's three lock rules, the reader that answers whether the screen is
//! locked, and the transition logic that turns the two into work for the
//! daemon to do. The effects themselves — stopping a session, taking a grant
//! back — belong to `apex-agentd`, which owns the PTYs and the grant
//! authority; what is here is every decision that leads to one.
//!
//! ## The two gaps this used to have, and what closed them
//!
//! When this module was written it was policy with nothing behind it and
//! nothing in front of it, and it said so:
//!
//! * **The lock state was not observable.** logind exposes `LockedHint` on a
//!   session and expects the lock screen to set it, and APEX Shell locks with
//!   `ext-session-lock` and never called `SetLockedHint` — so the property
//!   read `no` on a session that had been locked for an hour, exactly as it
//!   did on one nobody had touched. One value for two states is not a
//!   measurement. apex-shell `8d081ff` fixed that: `LockedHintService.qml`
//!   mirrors `WlSessionLock.secure` — the compositor-acknowledged state, not
//!   the request — into logind. [`Loginctl`] is the reader that consumes it.
//!
//!   That call has to be the shell's. logind only accepts `SetLockedHint`
//!   from the process that owns the session, and `apex-agentd` cannot tell
//!   the shell from any other process running as the same user — so a "the
//!   screen is locked" verb on the control socket would be a lock policy any
//!   local process, a managed session included, could switch off.
//!
//! * **There was no actor.** [`LockWatch`] is it, driven from the daemon's
//!   grant-expiry thread, which already wakes on a timer and already owns the
//!   session registry and the grant authority.
//!
//! * **The second rule had no subject.** A short-lived root grant is
//!   `SystemAccess::Session` or `SystemAccess::Unsafe`, and both were refused
//!   by `AgentPolicy::validate` at the time. P0-006 and P0-007 built them, so
//!   [`LockPolicy::revoke_root_grants`] now has something to revoke.
//!
//! ## Why the decision is separated from the effect
//!
//! The same seam `auth.rs` uses: a policy decision must be unit-testable and
//! stopping a live process cannot be. [`LockWatch::step`] takes a lock state,
//! a list of sessions and a list of grants and returns [`LockActions`] — a
//! list of things to do — touching nothing. The daemon performs them. Every
//! rule about what a lock does is asserted against that function.
//!
//! ## Why the defaults are what they are
//!
//! Straight from §7:
//!
//! > ordinary agents may continue; Remote Control may continue if configured;
//! > short-lived root grants should default to revocation; user policy may
//! > override.
//!
//! "If configured" is the whole of [`LockPolicy::remote_control_continues`],
//! and its default is `false` because that is what "if configured" means in a
//! build nobody has configured. It is also the one default Andre will want to
//! change: Remote Control is his normal workflow and his screen locks while he
//! is away from it, which is precisely when he is using it. One command:
//! `apex agent lock --remote continue`.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::policy::RequestOrigin;

/// Whether this machine's screen is locked.
///
/// Four answers rather than a `bool`, because three of them are genuinely
/// different and the fourth is the one that gets lost: a machine with no
/// screen cannot be locked, a machine whose screen state could not be read is
/// not the same as one that answered "no", and treating either as unlocked is
/// how a lock policy stops being one.
///
/// The same shape as `task::Found` and for the same reason — "unknown" and
/// "absent" are different answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LockState {
    /// The graphical session reported itself unlocked.
    Unlocked,
    /// The graphical session reported itself locked.
    Locked,
    /// There is no graphical session on this machine, so there is no screen to
    /// lock. A *measurement*, not a failure: a headless box that answered
    /// "locked" here could never run anything.
    NoDisplaySession,
    /// A graphical session exists and its lock state could not be determined.
    /// Carries the reason, which is what makes the resulting refusal
    /// actionable. Never a synonym for unlocked.
    Unreadable(String),
}

impl LockState {
    /// Whether the machine should be treated as locked.
    ///
    /// [`LockState::Unreadable`] counts as locked. That is the entire point of
    /// the variant existing: a lock policy that fails open when it cannot read
    /// the lock is worth less than no policy, because it also stops anybody
    /// looking for one.
    pub fn treat_as_locked(&self) -> bool {
        matches!(self, LockState::Locked | LockState::Unreadable(_))
    }

    /// The reason the state could not be read, when it could not be.
    pub fn reason(&self) -> Option<&str> {
        match self {
            LockState::Unreadable(why) => Some(why),
            _ => None,
        }
    }
}

impl std::fmt::Display for LockState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LockState::Unlocked => f.pad("unlocked"),
            LockState::Locked => f.pad("locked"),
            LockState::NoDisplaySession => f.pad("no display session"),
            LockState::Unreadable(why) => write!(f, "unknown — {why}"),
        }
    }
}

/// §7's three lock rules, as the owner's settings.
///
/// `Default` is §7 verbatim, and `#[serde(default)]` is on the container as
/// well as implied for each field: a configuration file carrying `{"lock":
/// {"remote_control_continues": true}}` keeps §7's answer for the other two
/// rather than deserialising them as `false`, which would silently turn
/// revoke-on-lock off for anyone who set the one key they cared about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LockPolicy {
    /// "ordinary agents may continue".
    pub agents_continue: bool,
    /// "Remote Control may continue if configured".
    pub remote_control_continues: bool,
    /// "short-lived root grants should default to revocation".
    pub revoke_root_grants: bool,
}

impl Default for LockPolicy {
    fn default() -> LockPolicy {
        LockPolicy {
            agents_continue: true,
            remote_control_continues: false,
            revoke_root_grants: true,
        }
    }
}

/// What happens to a session when the screen locks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LockDecision {
    /// It keeps running.
    Continue,
    /// It is held until the machine is unlocked, with the reason.
    Hold(String),
}

impl LockDecision {
    pub fn continues(&self) -> bool {
        matches!(self, LockDecision::Continue)
    }
}

impl LockPolicy {
    /// Whether a session driven from `origin` keeps running.
    ///
    /// The origin question is [`RequestOrigin::lock_gated`], which is true for
    /// Remote Control alone: §7 separates "ordinary agents may continue" from
    /// "Remote Control may continue if configured", and nothing else in the
    /// vocabulary is in the second sentence.
    pub fn decide(&self, origin: RequestOrigin, lock: &LockState) -> LockDecision {
        if !lock.treat_as_locked() {
            return LockDecision::Continue;
        }
        let (allowed, why) = if origin.lock_gated() {
            (
                self.remote_control_continues,
                "Remote Control continues past a screen lock only when the owner has allowed \
                 it; set remote_control_continues",
            )
        } else {
            (
                self.agents_continue,
                "this machine is set to hold agent sessions while the screen is locked",
            )
        };
        if allowed {
            return LockDecision::Continue;
        }
        LockDecision::Hold(match lock.reason() {
            Some(r) => format!("{why}. The screen state could not be read ({r}), which is \
                                treated as locked"),
            None => why.to_string(),
        })
    }

    /// [`LockPolicy::decide`] for a session whose origin may not be known.
    ///
    /// An origin the daemon could not establish is not a local one, and it is
    /// not an ordinary agent either — it could be either, so it gets the
    /// stricter of the two rules and continues only when both would let it.
    /// Same reasoning as [`LockState::Unreadable`] counting as locked: this
    /// codebase has already swept a defect class where a failed read was
    /// recorded as a checked absence, and "origin unknown" collapsing into
    /// "ordinary agent, carry on" would be another instance of it.
    pub fn decide_session(
        &self,
        origin: Option<RequestOrigin>,
        lock: &LockState,
    ) -> LockDecision {
        match origin {
            Some(o) => self.decide(o, lock),
            None => {
                if !lock.treat_as_locked() {
                    return LockDecision::Continue;
                }
                if self.agents_continue && self.remote_control_continues {
                    return LockDecision::Continue;
                }
                LockDecision::Hold(
                    "this session's origin could not be established, so it is held under \
                     whichever of the two §7 lock rules is the stricter — it could be a \
                     Remote Control session, and an unreadable origin is not a local one"
                        .to_string(),
                )
            }
        }
    }

    /// Whether a short-lived root grant survives a screen lock.
    ///
    /// §7 says it should not, by default. P0-006 and P0-007 built the grants
    /// this refers to, so it is a live rule rather than a default waiting for
    /// a subject: [`LockWatch::step`] turns a `false` here into a revocation
    /// for every grant in force at the moment the screen locks.
    pub fn root_grant_survives(&self, lock: &LockState) -> bool {
        !(lock.treat_as_locked() && self.revoke_root_grants)
    }
}


// ── Reading the lock state ───────────────────────────────────────────────────

/// Whatever can answer "is this machine's screen locked".
///
/// A trait for the same reason `auth.rs` has [`crate::auth::Authenticator`]:
/// the transition logic must be testable and logind must not be reached from
/// a test. [`Loginctl`] is the one implementation that talks to a machine.
///
/// `&mut self` rather than `&self` because the real reader caches the session
/// id between observations — see [`Loginctl`].
pub trait LockObserver: Send {
    fn observe(&mut self) -> LockState;
}

/// The lock state as logind holds it, read with `loginctl`.
///
/// Two questions, in order: which session is the graphical one, and is that
/// session's `LockedHint` set. `loginctl` rather than a D-Bus client because
/// this daemon deliberately has neither an async runtime nor a bus
/// connection, and the property is a string that changes every few hours at
/// most.
///
/// The session id is cached between observations and dropped whenever a read
/// fails, so the steady state is one short-lived process per tick rather than
/// two, and a session that goes away is still re-resolved on the next tick
/// rather than answered from a stale id forever.
pub struct Loginctl {
    uid: u32,
    session: Option<String>,
}

impl Loginctl {
    /// A reader for the user this process runs as.
    pub fn new() -> Loginctl {
        Loginctl {
            uid: crate::paths::uid(),
            session: None,
        }
    }

    /// A reader for a specific uid.
    pub fn for_uid(uid: u32) -> Loginctl {
        Loginctl { uid, session: None }
    }

    fn run(args: &[&str]) -> Result<(Option<i32>, String, String), String> {
        let out = std::process::Command::new("loginctl")
            .args(args)
            .output()
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    "loginctl is not on PATH, so logind cannot be asked whether the screen is \
                     locked"
                        .to_string()
                } else {
                    format!("running loginctl: {e}")
                }
            })?;
        Ok((
            out.status.code(),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        ))
    }
}

impl Default for Loginctl {
    fn default() -> Loginctl {
        Loginctl::new()
    }
}

impl LockObserver for Loginctl {
    fn observe(&mut self) -> LockState {
        if self.session.is_none() {
            let uid = self.uid.to_string();
            let (code, stdout, stderr) =
                match Loginctl::run(&["show-user", &uid, "-p", "Display", "--value"]) {
                    Ok(v) => v,
                    Err(why) => return LockState::Unreadable(why),
                };
            match parse_display(code, &stdout, &stderr) {
                Ok(id) => self.session = Some(id),
                Err(state) => return state,
            }
        }
        let id = self.session.clone().expect("resolved just above");
        let (code, stdout, stderr) =
            match Loginctl::run(&["show-session", &id, "-p", "LockedHint", "--value"]) {
                Ok(v) => v,
                Err(why) => {
                    self.session = None;
                    return LockState::Unreadable(why);
                }
            };
        let state = parse_locked_hint(&id, code, &stdout, &stderr);
        if matches!(state, LockState::Unreadable(_)) {
            // The cached id is the most likely thing to have gone wrong — the
            // session ended and a new one took its place — so it is dropped
            // and re-resolved next tick rather than asked about forever.
            self.session = None;
        }
        state
    }
}

/// What `loginctl show-user <uid> -p Display --value` said.
///
/// `Ok` is the graphical session's id. The two failure shapes are different
/// answers and are kept apart: a user with no graphical session prints
/// nothing and exits 0, which is [`LockState::NoDisplaySession`] and a
/// measurement; a user logind does not know prints an explanation on stderr
/// and exits 1, which is [`LockState::Unreadable`] and is treated as locked.
///
/// Both shapes were read off the tool rather than assumed — see the test
/// `the_loginctl_contract_this_parser_relies_on_is_still_true`, which runs it.
pub fn parse_display(code: Option<i32>, stdout: &str, stderr: &str) -> Result<String, LockState> {
    if code != Some(0) {
        return Err(LockState::Unreadable(format!(
            "loginctl show-user {}: {}",
            exit_phrase(code),
            first_line(stderr)
        )));
    }
    let id = stdout.trim();
    if id.is_empty() {
        return Err(LockState::NoDisplaySession);
    }
    Ok(id.to_string())
}

/// What `loginctl show-session <id> -p LockedHint --value` said.
///
/// **An empty value is not "no".** `loginctl -p <property> --value` prints
/// nothing and exits 0 for a property it does not recognise — verified
/// against the tool, not inferred — so a build talking to a logind without
/// `LockedHint`, or a typo in the property name, would read as an unlocked
/// screen forever. It reads as [`LockState::Unreadable`], which
/// [`LockState::treat_as_locked`] counts as locked.
pub fn parse_locked_hint(session: &str, code: Option<i32>, stdout: &str, stderr: &str) -> LockState {
    if code != Some(0) {
        return LockState::Unreadable(format!(
            "loginctl show-session {session} {}: {}",
            exit_phrase(code),
            first_line(stderr)
        ));
    }
    match stdout.trim() {
        "yes" => LockState::Locked,
        "no" => LockState::Unlocked,
        "" => LockState::Unreadable(format!(
            "logind reported no LockedHint at all for session {session}; an absent property is \
             not an unlocked screen"
        )),
        other => LockState::Unreadable(format!(
            "logind reported LockedHint={other:?} for session {session}, which is neither yes \
             nor no"
        )),
    }
}

fn exit_phrase(code: Option<i32>) -> String {
    match code {
        Some(c) => format!("exited {c}"),
        None => "was killed by a signal".to_string(),
    }
}

fn first_line(text: &str) -> String {
    let line = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    if line.is_empty() {
        "it said nothing".to_string()
    } else {
        line.trim().to_string()
    }
}

// ── The actor ───────────────────────────────────────────────────────────────

/// One session, as much of it as §7's lock rules need.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionView {
    pub id: u32,
    /// The origin driving it, or `None` when the daemon could not establish
    /// one. `None` is not "local": see [`LockPolicy::decide_session`].
    pub origin: Option<RequestOrigin>,
    /// Already stopped, by the user or by an earlier tick of this watch.
    pub paused: bool,
}

/// A system-access grant in force, as much of it as the lock rules need.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GrantView {
    pub id: u32,
}

/// What the daemon should do about a lock transition.
///
/// A list rather than the work itself, so every rule about what a lock does
/// is asserted over a value.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct LockActions {
    /// Sessions to stop, each with the reason to record against it.
    pub hold: Vec<(u32, String)>,
    /// Sessions to start again — only ones this watch stopped.
    pub resume: Vec<u32>,
    /// Grants to take back.
    pub revoke: Vec<u32>,
}

impl LockActions {
    pub fn is_empty(&self) -> bool {
        self.hold.is_empty() && self.resume.is_empty() && self.revoke.is_empty()
    }
}

/// §7's lock rules over time.
///
/// Holds two pieces of state and nothing else: whether the last observation
/// was a locked screen, and which sessions *this watch* stopped. The second
/// is what makes unlocking safe — a session the user paused by hand is never
/// resumed by an unlock, because it was never held here.
#[derive(Debug, Default)]
pub struct LockWatch {
    /// `None` until the first observation. A daemon that starts up onto an
    /// already-locked screen therefore sees a lock event, which is the
    /// fail-closed answer and the one that matches the shell's initial sync.
    last_locked: Option<bool>,
    held: BTreeSet<u32>,
}

impl LockWatch {
    pub fn new() -> LockWatch {
        LockWatch::default()
    }

    /// The sessions this watch is currently holding.
    pub fn held(&self) -> Vec<u32> {
        self.held.iter().copied().collect()
    }

    /// Whether the last observation was of a locked screen.
    pub fn locked(&self) -> Option<bool> {
        self.last_locked
    }

    /// One observation, turned into work.
    ///
    /// Holds are re-evaluated on every locked tick, because a session can
    /// start while the screen is locked — that is precisely what Remote
    /// Control is for — and a rule that only fired on the transition would
    /// let the next one straight through. It is idempotent: a session already
    /// held, or already stopped by the user, is passed over.
    ///
    /// Revocation fires on the *transition* only. §7 words it as an event
    /// ("On screen lock: short-lived root grants should default to
    /// revocation"), and a grant issued afterwards was issued by a human whom
    /// `grants.rs` had already established to be local — re-revoking it every
    /// five seconds would be a different rule than the one written down.
    pub fn step(
        &mut self,
        policy: &LockPolicy,
        state: &LockState,
        sessions: &[SessionView],
        grants: &[GrantView],
    ) -> LockActions {
        let locked = state.treat_as_locked();
        let became_locked = locked && self.last_locked != Some(true);
        self.last_locked = Some(locked);

        let mut actions = LockActions::default();
        if locked {
            for s in sessions {
                if self.held.contains(&s.id) || s.paused {
                    continue;
                }
                if let LockDecision::Hold(why) = policy.decide_session(s.origin, state) {
                    self.held.insert(s.id);
                    actions.hold.push((s.id, why));
                }
            }
            if became_locked && policy.revoke_root_grants {
                actions.revoke = grants.iter().map(|g| g.id).collect();
            }
        } else {
            actions.resume = self
                .held
                .iter()
                .copied()
                .filter(|id| sessions.iter().any(|s| s.id == *id))
                .collect();
            self.held.clear();
        }
        // A session this watch was holding and that has since exited is not
        // held any more. Without this the set grows for the life of the
        // daemon and an id reused by a later session would be resumed on an
        // unlock it had nothing to do with.
        self.held.retain(|id| sessions.iter().any(|s| s.id == *id));
        actions
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every state, including one of each unreadable form.
    fn states() -> Vec<LockState> {
        vec![
            LockState::Unlocked,
            LockState::Locked,
            LockState::NoDisplaySession,
            LockState::Unreadable("loginctl is not on PATH".into()),
        ]
    }

    #[test]
    fn the_defaults_are_section_seven_verbatim() {
        let p = LockPolicy::default();
        assert!(p.agents_continue, "§7: ordinary agents may continue");
        assert!(
            !p.remote_control_continues,
            "§7: Remote Control may continue IF CONFIGURED, and nobody has"
        );
        assert!(
            p.revoke_root_grants,
            "§7: short-lived root grants should default to revocation"
        );
    }

    #[test]
    fn an_unreadable_lock_state_is_treated_exactly_like_a_locked_one() {
        // The property the fourth variant exists for. A policy that failed
        // open here would be worse than no policy, because it would also stop
        // anybody looking for one.
        let locked = LockState::Locked;
        let unknown = LockState::Unreadable("no display session could be resolved".into());
        assert!(locked.treat_as_locked());
        assert!(unknown.treat_as_locked());

        for policy in every_policy() {
            for origin in RequestOrigin::ALL {
                assert_eq!(
                    policy.decide(*origin, &locked).continues(),
                    policy.decide(*origin, &unknown).continues(),
                    "{origin} was treated differently when the lock state was unknown"
                );
                assert_eq!(
                    policy.root_grant_survives(&locked),
                    policy.root_grant_survives(&unknown)
                );
            }
        }
    }

    #[test]
    fn a_machine_with_no_screen_is_not_a_permanently_locked_one() {
        // The other half, and the one a blanket fail-closed gets wrong: a
        // headless box has no screen to lock, so "locked" there is not a
        // cautious answer, it is a machine on which nothing can ever run.
        assert!(!LockState::NoDisplaySession.treat_as_locked());
        assert!(!LockState::Unlocked.treat_as_locked());
        for policy in every_policy() {
            for origin in RequestOrigin::ALL {
                assert!(
                    policy.decide(*origin, &LockState::NoDisplaySession).continues(),
                    "{origin} was held on a machine with no screen"
                );
            }
            assert!(policy.root_grant_survives(&LockState::NoDisplaySession));
        }
    }

    #[test]
    fn an_ordinary_agent_continues_and_remote_control_waits_to_be_allowed() {
        // §7's first two lock rules, on the defaults.
        let p = LockPolicy::default();
        for origin in RequestOrigin::ALL {
            let d = p.decide(*origin, &LockState::Locked);
            assert_eq!(
                d.continues(),
                !origin.lock_gated(),
                "{origin} on a locked screen"
            );
        }
        assert!(!p.decide(RequestOrigin::RemoteControl, &LockState::Locked).continues());
        assert!(p.decide(RequestOrigin::Subagent, &LockState::Locked).continues());
        assert!(p.decide(RequestOrigin::LocalTerminal, &LockState::Locked).continues());
    }

    #[test]
    fn the_owner_can_override_either_rule_in_either_direction() {
        // "user policy may override", which cuts both ways: Andre turns
        // Remote Control on because that is his workflow, and somebody else
        // turns ordinary agents off.
        let remote_ok = LockPolicy {
            remote_control_continues: true,
            ..LockPolicy::default()
        };
        assert!(remote_ok
            .decide(RequestOrigin::RemoteControl, &LockState::Locked)
            .continues());

        let nothing_runs = LockPolicy {
            agents_continue: false,
            remote_control_continues: false,
            revoke_root_grants: true,
        };
        for origin in RequestOrigin::ALL {
            assert!(!nothing_runs.decide(*origin, &LockState::Locked).continues(), "{origin}");
        }
        // And on an unlocked screen the settings do not apply at all.
        for origin in RequestOrigin::ALL {
            assert!(nothing_runs.decide(*origin, &LockState::Unlocked).continues());
        }
    }

    #[test]
    fn a_hold_says_which_setting_would_change_it() {
        let p = LockPolicy::default();
        let LockDecision::Hold(why) = p.decide(RequestOrigin::RemoteControl, &LockState::Locked)
        else {
            panic!("remote control must be held on the defaults");
        };
        assert!(why.contains("remote_control_continues"), "{why}");

        // And when the state was unreadable, the reason it could not be read
        // is carried through — otherwise the user sees a hold with no way to
        // find out why the machine thinks it is locked.
        let LockDecision::Hold(why) = p.decide(
            RequestOrigin::RemoteControl,
            &LockState::Unreadable("loginctl exited 1".into()),
        ) else {
            panic!("must be held");
        };
        assert!(why.contains("loginctl exited 1"), "{why}");
    }

    #[test]
    fn a_root_grant_does_not_survive_a_lock_unless_the_owner_said_so() {
        // §7's third rule. Nothing issues such a grant yet; this is the
        // default P0-006 and P0-007 should read.
        let p = LockPolicy::default();
        assert!(!p.root_grant_survives(&LockState::Locked));
        assert!(!p.root_grant_survives(&LockState::Unreadable("no hint".into())));
        assert!(p.root_grant_survives(&LockState::Unlocked));

        let keep = LockPolicy {
            revoke_root_grants: false,
            ..LockPolicy::default()
        };
        for s in states() {
            assert!(keep.root_grant_survives(&s), "{s}");
        }
    }

    #[test]
    fn the_lock_decision_does_not_depend_on_anything_but_origin_and_state() {
        // Every policy, every origin, every state. The function is small
        // enough to check exhaustively, so it is.
        for policy in every_policy() {
            for origin in RequestOrigin::ALL {
                for state in states() {
                    let d = policy.decide(*origin, &state);
                    let expected = if !state.treat_as_locked() {
                        true
                    } else if origin.lock_gated() {
                        policy.remote_control_continues
                    } else {
                        policy.agents_continue
                    };
                    assert_eq!(d.continues(), expected, "{policy:?} {origin} {state}");
                }
            }
        }
    }

    #[test]
    fn an_unreadable_state_renders_as_unknown_and_never_as_unlocked() {
        // Printed in a status line one day. `mode.rs` renders an unavailable
        // signal as "unavailable — reason"; this follows it.
        let s = LockState::Unreadable("the display session could not be resolved".into());
        let shown = s.to_string();
        assert!(shown.starts_with("unknown — "), "{shown}");
        assert!(shown.contains("display session"), "{shown}");
        assert_ne!(shown, LockState::Unlocked.to_string());
    }

    // ── the reader ──────────────────────────────────────────────────────

    #[test]
    fn a_user_with_no_graphical_session_is_measured_and_a_user_logind_refuses_is_not() {
        // The two failure shapes of `loginctl show-user -p Display --value`,
        // which are different answers: exit 0 with nothing means there is no
        // screen to lock, exit 1 means the question could not be asked.
        assert_eq!(parse_display(Some(0), "3\n", ""), Ok("3".to_string()));
        assert_eq!(parse_display(Some(0), "  7 \n", ""), Ok("7".to_string()));
        assert_eq!(parse_display(Some(0), "\n", ""), Err(LockState::NoDisplaySession));

        let Err(LockState::Unreadable(why)) =
            parse_display(Some(1), "", "Failed to look up user 4242: No such process\n")
        else {
            panic!("a non-zero exit must be unreadable, never a measurement");
        };
        assert!(why.contains("exited 1"), "{why}");
        assert!(why.contains("No such process"), "{why}");

        // Killed by a signal is not exit 0 either, and must not read as a
        // machine with no screen.
        assert!(matches!(
            parse_display(None, "", ""),
            Err(LockState::Unreadable(_))
        ));
    }

    #[test]
    fn an_absent_lockedhint_is_unreadable_and_never_an_unlocked_screen() {
        // The defect this parser exists to avoid, and it is not hypothetical:
        // `loginctl show-session 3 -p NoSuchProperty --value` prints nothing
        // and exits 0. Run against the tool, not reasoned about. So a build
        // asking for a property this logind does not have would see exactly
        // the same bytes as an unlocked screen.
        assert_eq!(parse_locked_hint("3", Some(0), "yes\n", ""), LockState::Locked);
        assert_eq!(parse_locked_hint("3", Some(0), "no\n", ""), LockState::Unlocked);

        let empty = parse_locked_hint("3", Some(0), "", "");
        assert!(empty.treat_as_locked(), "an absent property read as unlocked");
        assert!(empty.reason().unwrap().contains("no LockedHint"), "{empty}");

        let odd = parse_locked_hint("3", Some(0), "maybe\n", "");
        assert!(odd.treat_as_locked());
        assert!(odd.reason().unwrap().contains("maybe"), "{odd}");

        let failed = parse_locked_hint("999", Some(1), "", "No session '999' known\n");
        assert!(failed.treat_as_locked());
        assert!(failed.reason().unwrap().contains("No session '999' known"), "{failed}");
    }

    /// The one test here that talks to the machine. Read-only: it asks
    /// logind two questions and changes nothing, raises no prompt and locks
    /// nothing.
    ///
    /// It exists because every parser above is a claim about how a tool
    /// behaves, and a mutant of a parser only checks that the code matches
    /// the model that produced it. This checks the model.
    #[test]
    fn the_loginctl_contract_this_parser_relies_on_is_still_true() {
        use std::process::Command;

        let uid = crate::paths::uid().to_string();
        let Ok(out) = Command::new("loginctl")
            .args(["show-user", &uid, "-p", "Display", "--value"])
            .output()
        else {
            eprintln!("skipped: loginctl is not on PATH");
            return;
        };
        assert_eq!(
            out.status.code(),
            Some(0),
            "loginctl show-user exited non-zero for our own uid: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let display = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if display.is_empty() {
            // A build box or a container: no graphical session, which
            // parse_display calls NoDisplaySession. That half is confirmed;
            // the rest needs a session to ask about.
            eprintln!("skipped the LockedHint half: this user has no graphical session");
            return;
        }

        let hint = Command::new("loginctl")
            .args(["show-session", &display, "-p", "LockedHint", "--value"])
            .output()
            .expect("loginctl ran a moment ago");
        assert_eq!(hint.status.code(), Some(0));
        let value = String::from_utf8_lossy(&hint.stdout).trim().to_string();
        assert!(
            value == "yes" || value == "no",
            "LockedHint printed {value:?}, so the yes/no parse above is wrong about this logind"
        );

        // And the property that makes the empty case dangerous: an unknown
        // property is not an error, it is silence.
        let bogus = Command::new("loginctl")
            .args(["show-session", &display, "-p", "ApexNoSuchProperty", "--value"])
            .output()
            .expect("loginctl ran a moment ago");
        assert_eq!(
            bogus.status.code(),
            Some(0),
            "an unknown property now fails loudly, which would make the empty-value rule dead \
             code rather than a guard"
        );
        assert!(
            String::from_utf8_lossy(&bogus.stdout).trim().is_empty(),
            "an unknown property printed something"
        );
    }

    // ── the actor ───────────────────────────────────────────────────────

    fn session(id: u32, origin: RequestOrigin) -> SessionView {
        SessionView {
            id,
            origin: Some(origin),
            paused: false,
        }
    }

    #[test]
    fn locking_holds_remote_control_leaves_ordinary_agents_alone_and_revokes_once() {
        let policy = LockPolicy::default();
        let mut watch = LockWatch::new();
        let sessions = [
            session(1, RequestOrigin::Subagent),
            session(2, RequestOrigin::RemoteControl),
        ];
        let grants = [GrantView { id: 10 }, GrantView { id: 11 }];

        // Unlocked: nothing happens, and nothing is revoked.
        let a = watch.step(&policy, &LockState::Unlocked, &sessions, &grants);
        assert!(a.is_empty(), "{a:?}");

        let a = watch.step(&policy, &LockState::Locked, &sessions, &grants);
        assert_eq!(a.hold.iter().map(|(id, _)| *id).collect::<Vec<_>>(), vec![2]);
        assert_eq!(a.revoke, vec![10, 11]);
        assert!(a.resume.is_empty());

        // Still locked, nothing new: the hold is not repeated and — the point
        // of the transition rule — the grants are not revoked a second time.
        let sessions_paused = [
            session(1, RequestOrigin::Subagent),
            SessionView {
                id: 2,
                origin: Some(RequestOrigin::RemoteControl),
                paused: true,
            },
        ];
        let a = watch.step(&policy, &LockState::Locked, &sessions_paused, &grants);
        assert!(a.is_empty(), "a second locked tick did work: {a:?}");

        // Unlock: exactly what was held comes back.
        let a = watch.step(&policy, &LockState::Unlocked, &sessions_paused, &grants);
        assert_eq!(a.resume, vec![2]);
        assert!(a.hold.is_empty() && a.revoke.is_empty());
        assert!(watch.held().is_empty(), "the held set was not cleared");

        // And a second unlocked tick resumes nothing.
        let a = watch.step(&policy, &LockState::Unlocked, &sessions_paused, &grants);
        assert!(a.is_empty(), "{a:?}");
    }

    #[test]
    fn a_session_started_while_the_screen_is_locked_is_held_on_the_next_tick() {
        // The reason holds are re-evaluated every locked tick rather than
        // only on the transition: starting work while the owner is away from
        // the machine is the entire point of Remote Control, so the session
        // that most needs the rule is the one that did not exist when the
        // screen locked.
        let policy = LockPolicy::default();
        let mut watch = LockWatch::new();
        let first = [session(1, RequestOrigin::Subagent)];
        let a = watch.step(&policy, &LockState::Locked, &first, &[]);
        assert!(a.hold.is_empty());

        let later = [
            session(1, RequestOrigin::Subagent),
            session(2, RequestOrigin::RemoteControl),
        ];
        let a = watch.step(&policy, &LockState::Locked, &later, &[]);
        assert_eq!(a.hold.iter().map(|(id, _)| *id).collect::<Vec<_>>(), vec![2]);
    }

    #[test]
    fn a_session_the_user_paused_is_not_held_and_is_not_resumed_by_an_unlock() {
        // `apex agent pause 2` and a screen lock both end in SIGSTOP, and
        // only one of them should be undone by typing a password. The watch
        // resumes what it stopped and nothing else.
        let policy = LockPolicy {
            agents_continue: false,
            ..LockPolicy::default()
        };
        let mut watch = LockWatch::new();
        let sessions = [
            SessionView {
                id: 1,
                origin: Some(RequestOrigin::LocalTerminal),
                paused: true,
            },
            session(2, RequestOrigin::LocalTerminal),
        ];
        let a = watch.step(&policy, &LockState::Locked, &sessions, &[]);
        assert_eq!(a.hold.iter().map(|(id, _)| *id).collect::<Vec<_>>(), vec![2]);
        assert_eq!(watch.held(), vec![2]);

        let a = watch.step(&policy, &LockState::Unlocked, &sessions, &[]);
        assert_eq!(a.resume, vec![2], "the hand-paused session must stay paused");
    }

    #[test]
    fn a_daemon_that_starts_onto_a_locked_screen_treats_it_as_a_lock() {
        // There is no earlier observation to have transitioned from, and the
        // fail-closed answer is the one that matches the shell's initial
        // sync: a shell restart while logind believes the session locked must
        // not read as "it was never locked".
        let mut watch = LockWatch::new();
        assert_eq!(watch.locked(), None);
        let a = watch.step(
            &LockPolicy::default(),
            &LockState::Locked,
            &[session(1, RequestOrigin::RemoteControl)],
            &[GrantView { id: 5 }],
        );
        assert_eq!(a.revoke, vec![5]);
        assert_eq!(a.hold.len(), 1);
        assert_eq!(watch.locked(), Some(true));
    }

    #[test]
    fn an_unreadable_lock_state_revokes_and_holds_exactly_as_a_locked_one_does() {
        let policy = LockPolicy::default();
        let sessions = [session(2, RequestOrigin::RemoteControl)];
        let grants = [GrantView { id: 9 }];
        let mut a = LockWatch::new();
        let mut b = LockWatch::new();
        let locked = a.step(&policy, &LockState::Locked, &sessions, &grants);
        let unknown = b.step(
            &policy,
            &LockState::Unreadable("loginctl is not on PATH".into()),
            &sessions,
            &grants,
        );
        assert_eq!(locked.revoke, unknown.revoke);
        assert_eq!(
            locked.hold.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
            unknown.hold.iter().map(|(id, _)| *id).collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_machine_with_no_screen_never_holds_and_never_revokes() {
        for policy in every_policy() {
            let mut watch = LockWatch::new();
            let a = watch.step(
                &policy,
                &LockState::NoDisplaySession,
                &[
                    session(1, RequestOrigin::RemoteControl),
                    session(2, RequestOrigin::CloudJob),
                ],
                &[GrantView { id: 3 }],
            );
            assert!(a.is_empty(), "{policy:?} acted on a machine with no screen: {a:?}");
        }
    }

    #[test]
    fn the_owner_turning_revocation_off_stops_the_revocation_and_nothing_else() {
        let keep = LockPolicy {
            revoke_root_grants: false,
            ..LockPolicy::default()
        };
        let mut watch = LockWatch::new();
        let a = watch.step(
            &keep,
            &LockState::Locked,
            &[session(2, RequestOrigin::RemoteControl)],
            &[GrantView { id: 7 }],
        );
        assert!(a.revoke.is_empty(), "revocation was turned off");
        assert_eq!(a.hold.len(), 1, "the hold rule is a separate setting");
    }

    #[test]
    fn the_owner_letting_remote_control_run_stops_the_hold_and_nothing_else() {
        // Andre's setting: Remote Control is his normal workflow and his
        // screen locks while he is away from the machine, which is exactly
        // when he is using it. The root-grant rule is untouched by it.
        let allow = LockPolicy {
            remote_control_continues: true,
            ..LockPolicy::default()
        };
        let mut watch = LockWatch::new();
        let a = watch.step(
            &allow,
            &LockState::Locked,
            &[session(2, RequestOrigin::RemoteControl)],
            &[GrantView { id: 7 }],
        );
        assert!(a.hold.is_empty());
        assert_eq!(a.revoke, vec![7]);
    }

    #[test]
    fn a_session_whose_origin_could_not_be_read_gets_the_stricter_of_the_two_rules() {
        let unknown = SessionView {
            id: 4,
            origin: None,
            paused: false,
        };
        for policy in every_policy() {
            let mut watch = LockWatch::new();
            let a = watch.step(&policy, &LockState::Locked, std::slice::from_ref(&unknown), &[]);
            let should_continue = policy.agents_continue && policy.remote_control_continues;
            assert_eq!(
                a.hold.is_empty(),
                should_continue,
                "{policy:?} let an unestablished origin follow only one of the two rules"
            );
        }
        // And on an unlocked screen an unknown origin is not held either.
        assert!(LockPolicy::default()
            .decide_session(None, &LockState::Unlocked)
            .continues());
    }

    #[test]
    fn a_held_session_that_exits_is_forgotten_rather_than_resumed_later() {
        // Session ids are handed out by a counter that restarts with the
        // daemon, so a set that never forgets would eventually resume a
        // session that had nothing to do with the lock.
        let policy = LockPolicy::default();
        let mut watch = LockWatch::new();
        let sessions = [session(2, RequestOrigin::RemoteControl)];
        watch.step(&policy, &LockState::Locked, &sessions, &[]);
        assert_eq!(watch.held(), vec![2]);

        // It exits while the screen is still locked.
        watch.step(&policy, &LockState::Locked, &[], &[]);
        assert!(watch.held().is_empty(), "an exited session stayed held");

        // A new session takes the id, and the unlock does not resume it.
        let reused = [session(2, RequestOrigin::LocalTerminal)];
        let a = watch.step(&policy, &LockState::Unlocked, &reused, &[]);
        assert!(a.resume.is_empty(), "{a:?}");
    }

    #[test]
    fn a_lock_policy_with_one_key_set_keeps_section_sevens_answer_for_the_others() {
        // The whole reason for `#[serde(default)]` on the container. Andre
        // will write exactly this file, and it must not turn revoke-on-lock
        // off as a side effect.
        let p: LockPolicy =
            serde_json::from_str(r#"{"remote_control_continues": true}"#).expect("parses");
        assert!(p.remote_control_continues);
        assert!(p.agents_continue, "§7: ordinary agents may continue");
        assert!(p.revoke_root_grants, "§7: root grants revoke by default");

        // An empty object is §7 verbatim, and a round trip is lossless.
        assert_eq!(
            serde_json::from_str::<LockPolicy>("{}").expect("parses"),
            LockPolicy::default()
        );
        let text = serde_json::to_string(&p).expect("serialises");
        assert_eq!(serde_json::from_str::<LockPolicy>(&text).expect("parses"), p);
    }

    fn every_policy() -> Vec<LockPolicy> {
        let mut out = Vec::new();
        for agents in [true, false] {
            for remote in [true, false] {
                for revoke in [true, false] {
                    out.push(LockPolicy {
                        agents_continue: agents,
                        remote_control_continues: remote,
                        revoke_root_grants: revoke,
                    });
                }
            }
        }
        out
    }
}
