//! What a screen lock means for a running session (roadmap §7).
//!
//! **Nothing calls this yet, and that is deliberate.** It is §7's three lock
//! rules as a decision function and a set of defaults, with no observation
//! behind it and no actor in front of it. Read the two sections below before
//! wiring anything to it; both gaps are in other people's code, and shipping
//! around either of them would produce a setting that claims a protection it
//! does not have.
//!
//! ## Gap one: the lock state is not observable on APEX-OS
//!
//! logind exposes `LockedHint` on a session, and a lock screen is expected to
//! set it by calling `org.freedesktop.login1.Session.SetLockedHint`. APEX
//! Shell locks with the `ext-session-lock` protocol and never calls it —
//! `LockedHint` reads `no` on a machine that has been locked for an hour,
//! exactly as it does on one nobody has touched.
//!
//! One value for two states is not a measurement, and it is the *interesting*
//! state that is missing. A reader built on it today would return "unlocked"
//! forever, and a policy keyed on that would be a switch with a wire running
//! to nothing. So there is no reader here.
//!
//! What would fix it is two calls in APEX Shell, at the places
//! `LockState.locked` changes. `SetLockedHint` is the right home for this
//! rather than a message to `apex-agentd`: logind only accepts it from the
//! process that owns the session, and this daemon cannot tell the shell from
//! any other process running as the same user — so a "the screen is locked"
//! verb on the control socket would be a lock policy that any local process,
//! including a managed session, could switch off.
//!
//! Once the hint is set, reading it needs no new dependency: `loginctl
//! show-user <uid> --property=Display --value` names the graphical session and
//! `loginctl show-session <id> --property=LockedHint --value` answers for it.
//!
//! ## Gap two: there is no actor
//!
//! §7's rules are about transitions — "may continue", "should default to
//! revocation" — and something has to notice one. The daemon has no D-Bus
//! client and deliberately no async runtime, so the natural home is the
//! per-session reader loop, which already wakes once a second and already
//! owns `SIGSTOP`/`SIGCONT` and the `paused` flag.
//!
//! The second rule has no subject at all in this build: a short-lived root
//! grant is `SystemAccess::Session` or `SystemAccess::Unsafe`, and
//! `AgentPolicy::validate` refuses both. There is nothing to revoke until
//! P0-006 and P0-007 issue one. [`LockPolicy::revoke_root_grants`] is the
//! default those tasks should read; it is not a promise this build keeps.
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
//! is away from it, which is precisely when he is using it.

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
/// `Default` is §7 verbatim. Nothing reads this yet; see the module docs for
/// what has to exist first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

    /// Whether a short-lived root grant survives a screen lock.
    ///
    /// §7 says it should not, by default. Nothing in this build issues such a
    /// grant — `SystemAccess::Session` and `SystemAccess::Unsafe` are both
    /// refused by `AgentPolicy::validate` — so this is the default P0-006 and
    /// P0-007 should read, not a protection that is running.
    pub fn root_grant_survives(&self, lock: &LockState) -> bool {
        !(lock.treat_as_locked() && self.revoke_root_grants)
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
