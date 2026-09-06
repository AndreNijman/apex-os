//! Where a privileged or capability request came from (roadmap §7).
//!
//! §7 gives seven names — `local-terminal`, `apex-shell`,
//! `claude-remote-control`, `scheduled-job`, `mcp`, `subagent`, `cloud-job` —
//! and one table of what each may do. [`crate::policy::RequestOrigin`] is that
//! vocabulary; this module is the part that decides which of them a given
//! request is allowed to be, and the two properties policy reads off it.
//!
//! ## Origin is established, not asserted
//!
//! The whole field is decoration if a session can write `local-terminal` into
//! its own request. So the rule is one-directional:
//!
//! > The two local origins are **observed only**. Nothing a client sends can
//! > produce one. Every other origin may be *declared*, and only when the
//! > declaration gives something up.
//!
//! The daemon observes an origin from the kernel's view of the connection —
//! peer credentials, `/proc` ancestry, cgroup — the same way it already
//! resolves which session is asking. A client may then declare a *more*
//! restricted origin over the top of that, and the declaration is checked by
//! [`may_declare`] before it is recorded.
//!
//! ## Why a declaration is needed at all
//!
//! Claude Remote Control drives a `claude` process that is running on this
//! machine. From the socket's point of view it is indistinguishable from the
//! same process being driven by a human at the keyboard, because it *is* the
//! same process — what changed is who is typing, and the kernel cannot see
//! that. Remote Control is also usually turned on after the session started,
//! so the session was genuinely local when it was created.
//!
//! An observation therefore cannot produce `claude-remote-control`, and a
//! build that refused to let anything say so would have a §7 policy with no
//! way to ever reach its second column. What makes the declaration safe is
//! that it can only ever cost the session something: `local-terminal` can
//! become `claude-remote-control`, and never the other way around.
//!
//! ## The two properties, and why the rule is stated over both
//!
//! Policy asks an origin two independent questions:
//!
//! * [`RequestOrigin::is_local`] — is a human at this machine? §7's first
//!   column. Root and break-glass are reserved for it.
//! * [`RequestOrigin::lock_gated`] — does continuing past a screen lock need
//!   the owner's permission? §7 says "Remote Control may continue if
//!   configured" and "ordinary agents may continue", so this is true for
//!   Remote Control and false for everything else.
//!
//! Those do not order the same way, and that is the trap. `mcp` is *less*
//! trusted than `claude-remote-control` for elevation and *more* permitted
//! than it on lock, so a single "trust rank" would let a Remote Control
//! session redeclare itself `mcp` and buy the lock rule it was denied.
//! [`may_declare`] therefore requires both properties to move in the
//! restricting direction, and `origin_declarations_never_buy_anything` asserts
//! it over the whole 7×7 product rather than over the pairs someone thought of.

use serde::{Deserialize, Serialize};

use crate::policy::RequestOrigin;

impl RequestOrigin {
    /// Whether continuing past a screen lock needs the owner's permission.
    ///
    /// §7: "ordinary agents may continue; Remote Control may continue if
    /// configured". A locked screen means nobody is at the machine, and a
    /// human elsewhere still driving it is the case the owner is asked about.
    /// A scheduled job or a subagent is an ordinary unattended agent and was
    /// never attended in the first place.
    pub fn lock_gated(&self) -> bool {
        matches!(self, RequestOrigin::RemoteControl)
    }

    /// Whether a client may name this origin for itself.
    ///
    /// False for the two local ones, which is the entire security property of
    /// this module: `is_local()` is what §7 reserves root and break-glass for,
    /// so an origin that could be claimed would be an elevation anybody could
    /// ask for.
    pub fn may_be_declared(&self) -> bool {
        !self.is_local()
    }
}

/// How the recorded origin was arrived at.
///
/// Stored beside the origin rather than inferred from it, because "the daemon
/// worked this out" and "something asked for this and it was allowed" are
/// different claims about the same value, and an audit trail that cannot tell
/// them apart cannot answer the only question worth asking of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OriginSource {
    /// The daemon derived it from the connection: peer credentials, the
    /// `/proc` ancestry, the peer's cgroup. Nothing the client sent was read.
    Observed,
    /// The client asked for it and [`may_declare`] allowed it. Always a
    /// non-local origin, and always at least as restricted as what was
    /// observed.
    Declared,
    /// Copied from the session that owns the connection. A request made inside
    /// a session carries that session's origin, because that is what is
    /// actually driving it.
    Inherited,
}

impl OriginSource {
    pub const ALL: &'static [OriginSource] = &[
        OriginSource::Observed,
        OriginSource::Declared,
        OriginSource::Inherited,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            OriginSource::Observed => "observed",
            OriginSource::Declared => "declared",
            OriginSource::Inherited => "inherited",
        }
    }

    pub fn parse(s: &str) -> Option<OriginSource> {
        match s {
            "observed" => Some(OriginSource::Observed),
            "declared" => Some(OriginSource::Declared),
            "inherited" => Some(OriginSource::Inherited),
            _ => None,
        }
    }
}

impl std::fmt::Display for OriginSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.pad(self.as_str())
    }
}

/// An origin and how it was arrived at.
///
/// What the daemon records on a session and stamps on every request that
/// session makes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionOrigin {
    pub origin: RequestOrigin,
    pub source: OriginSource,
}

impl SessionOrigin {
    /// An origin the daemon worked out for itself.
    pub fn observed(origin: RequestOrigin) -> SessionOrigin {
        SessionOrigin {
            origin,
            source: OriginSource::Observed,
        }
    }

    /// The origin of the session this request was made inside.
    pub fn inherited(origin: RequestOrigin) -> SessionOrigin {
        SessionOrigin {
            origin,
            source: OriginSource::Inherited,
        }
    }

    /// Apply a declaration on top of this origin.
    ///
    /// Returns the new origin, or the reason it was refused. The result is
    /// always [`OriginSource::Declared`] on success, so a later reader can see
    /// that the value did not come from an observation.
    pub fn declare(&self, wanted: RequestOrigin) -> Result<SessionOrigin, OriginError> {
        may_declare(self.origin, wanted)?;
        Ok(SessionOrigin {
            origin: wanted,
            source: OriginSource::Declared,
        })
    }

    /// Whether a human is at this machine.
    pub fn is_local(&self) -> bool {
        self.origin.is_local()
    }
}

/// Why a declared origin was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OriginError {
    /// A local origin was asked for. Never granted, whatever was observed:
    /// this is the rule the security invariant rests on.
    NotDeclarable(RequestOrigin),
    /// The declaration would have removed a restriction the current origin
    /// carries.
    WouldLoosen {
        from: RequestOrigin,
        to: RequestOrigin,
    },
}

impl std::fmt::Display for OriginError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OriginError::NotDeclarable(o) => write!(
                f,
                "a request cannot declare itself {o}: the local origins are what \
                 §7 reserves root and break-glass for, so they are established by \
                 the daemon from the connection and never claimed"
            ),
            OriginError::WouldLoosen { from, to } => write!(
                f,
                "this session is already {from}, and {to} is not more restricted than \
                 that; a declared origin may only ever give something up"
            ),
        }
    }
}

impl std::error::Error for OriginError {}

/// Whether `wanted` may be declared over an origin that is currently
/// `current`.
///
/// Two conditions, one per property policy reads, and both stated as "the
/// restriction may appear but may not disappear":
///
/// * locality may go from local to remote, never back;
/// * the lock gate may be taken on, never dropped.
///
/// Everything else is a refinement between equally restricted values — `mcp`
/// to `subagent`, say — which changes what the audit trail says and nothing
/// about what is permitted.
pub fn may_declare(current: RequestOrigin, wanted: RequestOrigin) -> Result<(), OriginError> {
    if !wanted.may_be_declared() {
        return Err(OriginError::NotDeclarable(wanted));
    }
    let keeps_locality = current.is_local() || !wanted.is_local();
    let keeps_lock_gate = !current.lock_gated() || wanted.lock_gated();
    if keeps_locality && keeps_lock_gate {
        Ok(())
    } else {
        Err(OriginError::WouldLoosen {
            from: current,
            to: wanted,
        })
    }
}

/// The origin a session started from inside another session should get.
///
/// An agent spawning an agent is a `subagent` — §7 has the name, and
/// [`RequestOrigin::is_local`] already says nobody is present in an agent's
/// loop. But a subagent of a Remote Control session is still being driven from
/// elsewhere, so a remote parent is inherited rather than replaced: otherwise
/// spawning a child would be how a lock-gated session stopped being one.
pub fn child_of(parent: RequestOrigin) -> RequestOrigin {
    if parent.is_local() {
        RequestOrigin::Subagent
    } else {
        parent
    }
}

// ── §7's default policy table ───────────────────────────────────────────────

/// A row of §7's "Recommended default policy" table.
///
/// The rows are capability *classes*, not the individual operations that fall
/// into them: §7 writes one line for "GitHub push" and means every brokered
/// push. [`Capability::ruling`] is the table read across, and
/// `section_seven_table_is_reproduced_exactly` is the table read down, so the
/// two cannot drift.
///
/// Deliberately not merged into `secret::Capability`, which is the vocabulary
/// of operations the broker can actually perform. This is policy over classes
/// of operation, several of which nothing in this build performs at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capability {
    /// Write to the files of the project a session was started in.
    EditProject,
    /// Run the project's own test suite.
    RunTests,
    /// A brokered push to a git remote.
    GitHubPush,
    /// A brokered deploy to a preview environment.
    CloudflarePreviewDeploy,
    /// A brokered deploy to production.
    ProductionDeploy,
    /// Receive the value of a brokered credential.
    ReadRawSecret,
    /// Anything that needs root: §4's system-access grant, and every verb in
    /// [`crate::request::Verb`].
    RootCapability,
    /// §4.5's break-glass mode.
    UnsafeEverything,
}

/// A cell of that table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ruling {
    /// "allow" — it happens, with no separate decision.
    Allow,
    /// "configurable" — the owner decides, and until they have, it does not
    /// happen. Configurable is not a synonym for allowed.
    Configurable,
    /// "local auth" — a human at this machine authenticates. Distinct from
    /// [`Ruling::LocalApproval`] because the human is the one asking.
    LocalAuth,
    /// "local approval required" — the request came from elsewhere and a
    /// human at this machine has to approve it.
    LocalApproval,
    /// "deny" — from every origin, including the local one.
    Deny,
}

impl Ruling {
    pub fn as_str(&self) -> &'static str {
        match self {
            Ruling::Allow => "allow",
            Ruling::Configurable => "configurable",
            Ruling::LocalAuth => "local auth",
            Ruling::LocalApproval => "local approval required",
            Ruling::Deny => "deny",
        }
    }

    /// Whether this happens without anybody being asked.
    ///
    /// Only `allow`. `configurable` is not included and that is the point:
    /// the owner has not configured it, so the answer today is no.
    pub fn is_unattended(&self) -> bool {
        matches!(self, Ruling::Allow)
    }

    /// Whether satisfying this needs a human at this machine.
    pub fn needs_local_human(&self) -> bool {
        matches!(self, Ruling::LocalAuth | Ruling::LocalApproval)
    }
}

impl std::fmt::Display for Ruling {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.pad(self.as_str())
    }
}

impl Capability {
    pub const ALL: &'static [Capability] = &[
        Capability::EditProject,
        Capability::RunTests,
        Capability::GitHubPush,
        Capability::CloudflarePreviewDeploy,
        Capability::ProductionDeploy,
        Capability::ReadRawSecret,
        Capability::RootCapability,
        Capability::UnsafeEverything,
    ];

    /// The row label, as §7 writes it.
    pub fn as_str(&self) -> &'static str {
        match self {
            Capability::EditProject => "edit project",
            Capability::RunTests => "run tests",
            Capability::GitHubPush => "github push",
            Capability::CloudflarePreviewDeploy => "cloudflare preview deploy",
            Capability::ProductionDeploy => "production deploy",
            Capability::ReadRawSecret => "read raw brokered secret",
            Capability::RootCapability => "root capability",
            Capability::UnsafeEverything => "unsafe everything",
        }
    }

    /// §7's answer for this capability from this origin.
    ///
    /// The table has two columns — Local and Claude Remote Control — and the
    /// other five origins are not in it. They are read as the remote column,
    /// because the property the table is really keyed on is whether a human
    /// is at this machine, and a scheduled job has less of one than Remote
    /// Control does, not more.
    ///
    /// Note the two rows where remote is *not* more restricted than local:
    /// the first four are `allow` from both, which is §7's actual position and
    /// the reason Remote Control is described there as a normal workflow. A
    /// policy that quietly tightened them would be a different policy.
    pub fn ruling(&self, origin: RequestOrigin) -> Ruling {
        self.ruling_when(origin.is_local())
    }

    /// [`Capability::ruling`], keyed on the only property the table branches
    /// on.
    ///
    /// For the caller that has "a human is present" as a boolean and no
    /// origin to name — a record with none, which is not local and therefore
    /// gets the remote column without having to borrow an origin it never
    /// had.
    pub fn ruling_when(&self, local: bool) -> Ruling {
        match self {
            // "Remote Control is a normal workflow, not an edge case."
            Capability::EditProject
            | Capability::RunTests
            | Capability::GitHubPush
            | Capability::CloudflarePreviewDeploy => Ruling::Allow,
            Capability::ProductionDeploy => Ruling::Configurable,
            // The one row with the same answer in both columns for the
            // opposite reason: §3.2's brokered-not-exported rule holds even
            // for a human sitting at the keyboard.
            Capability::ReadRawSecret => Ruling::Deny,
            Capability::RootCapability | Capability::UnsafeEverything => {
                if local {
                    Ruling::LocalAuth
                } else {
                    Ruling::LocalApproval
                }
            }
        }
    }
}

impl std::fmt::Display for Capability {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.pad(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_local_origins_are_exactly_the_ones_that_cannot_be_declared() {
        for o in RequestOrigin::ALL {
            assert_eq!(o.may_be_declared(), !o.is_local(), "{o}");
        }
        assert!(!RequestOrigin::LocalTerminal.may_be_declared());
        assert!(!RequestOrigin::ApexShell.may_be_declared());
        assert!(RequestOrigin::RemoteControl.may_be_declared());
    }

    #[test]
    fn nothing_can_declare_itself_local_from_anywhere() {
        // The security invariant, stated as the exhaustive claim it has to be:
        // over every starting point, including the local ones, a local origin
        // is never a declarable answer.
        for current in RequestOrigin::ALL {
            for local in [RequestOrigin::LocalTerminal, RequestOrigin::ApexShell] {
                assert_eq!(
                    may_declare(*current, local),
                    Err(OriginError::NotDeclarable(local)),
                    "{current} was allowed to declare {local}"
                );
            }
        }
    }

    #[test]
    fn origin_declarations_never_buy_anything() {
        // The 7×7 product. Every accepted transition must keep both
        // properties policy reads, and the assertion is written as the
        // property rather than as a list of allowed pairs so a new origin
        // cannot be added without satisfying it.
        for from in RequestOrigin::ALL {
            for to in RequestOrigin::ALL {
                let Ok(()) = may_declare(*from, *to) else {
                    continue;
                };
                assert!(
                    from.is_local() || !to.is_local(),
                    "{from} -> {to} bought locality back"
                );
                assert!(
                    !from.lock_gated() || to.lock_gated(),
                    "{from} -> {to} dropped the lock gate"
                );
            }
        }
    }

    #[test]
    fn a_remote_control_session_cannot_redeclare_its_way_out_of_the_lock_gate() {
        // The concrete case the two-property rule exists for. `mcp` is less
        // trusted than Remote Control for elevation and more permitted on
        // lock, so a single trust rank would have allowed this.
        for escape in [
            RequestOrigin::Mcp,
            RequestOrigin::Subagent,
            RequestOrigin::ScheduledJob,
            RequestOrigin::CloudJob,
        ] {
            assert_eq!(
                may_declare(RequestOrigin::RemoteControl, escape),
                Err(OriginError::WouldLoosen {
                    from: RequestOrigin::RemoteControl,
                    to: escape,
                }),
                "remote control escaped into {escape}"
            );
        }
        // And it may still restate itself, which is what makes a repeated
        // declaration from a reconnecting client harmless.
        assert_eq!(
            may_declare(RequestOrigin::RemoteControl, RequestOrigin::RemoteControl),
            Ok(())
        );
    }

    #[test]
    fn a_local_session_can_hand_itself_over_to_remote_control() {
        // The workflow §7 exists for: Remote Control is turned on after the
        // session started, so the session was local when it was created.
        for local in [RequestOrigin::LocalTerminal, RequestOrigin::ApexShell] {
            let session = SessionOrigin::observed(local);
            let after = session
                .declare(RequestOrigin::RemoteControl)
                .expect("a local session may hand over");
            assert_eq!(after.origin, RequestOrigin::RemoteControl);
            assert_eq!(after.source, OriginSource::Declared);
            assert!(!after.is_local());
        }
    }

    #[test]
    fn an_unattended_origin_may_be_refined_but_not_relaxed() {
        // Between equally restricted values the declaration is descriptive: an
        // MCP bridge saying so is better audit than the scheduled-job the
        // daemon observed, and neither is permitted anything the other is not.
        assert_eq!(may_declare(RequestOrigin::ScheduledJob, RequestOrigin::Mcp), Ok(()));
        assert_eq!(may_declare(RequestOrigin::Mcp, RequestOrigin::CloudJob), Ok(()));
        // Taking the lock gate on is a cost, so it is allowed from anywhere.
        assert_eq!(
            may_declare(RequestOrigin::CloudJob, RequestOrigin::RemoteControl),
            Ok(())
        );
    }

    #[test]
    fn only_remote_control_is_gated_on_lock() {
        // §7 lists three lock rules and only one of them is about an origin.
        let gated: Vec<&RequestOrigin> =
            RequestOrigin::ALL.iter().filter(|o| o.lock_gated()).collect();
        assert_eq!(gated, vec![&RequestOrigin::RemoteControl]);
    }

    #[test]
    fn a_subagent_of_a_remote_session_is_still_remote() {
        assert_eq!(child_of(RequestOrigin::LocalTerminal), RequestOrigin::Subagent);
        assert_eq!(child_of(RequestOrigin::ApexShell), RequestOrigin::Subagent);
        for remote in [
            RequestOrigin::RemoteControl,
            RequestOrigin::ScheduledJob,
            RequestOrigin::Mcp,
            RequestOrigin::Subagent,
            RequestOrigin::CloudJob,
        ] {
            assert_eq!(child_of(remote), remote, "{remote}");
        }
        // Spawning a child is never a way out of a restriction.
        for parent in RequestOrigin::ALL {
            let child = child_of(*parent);
            assert!(!child.is_local(), "{parent} spawned a local child");
            assert!(
                !parent.lock_gated() || child.lock_gated(),
                "{parent} spawned a child without its lock gate"
            );
        }
    }

    #[test]
    fn the_source_names_round_trip_and_a_typo_is_rejected() {
        for s in OriginSource::ALL {
            assert_eq!(OriginSource::parse(s.as_str()), Some(*s), "{s}");
        }
        assert_eq!(OriginSource::parse("assumed"), None);
        assert_eq!(OriginSource::parse(""), None);
    }

    #[test]
    fn an_origin_has_one_spelling_on_the_wire_and_on_the_screen() {
        // `as_str` is what a user types and what the prompt prints; serde is
        // what the daemon writes into a session record and an audit line. Two
        // spellings of one origin is how a stored record stops matching a
        // policy somebody wrote by hand — and kebab-case gives
        // `remote-control` for the variant §7 calls `claude-remote-control`,
        // so this is not hypothetical.
        for o in RequestOrigin::ALL {
            let wire = serde_json::to_value(o).expect("serialise");
            assert_eq!(wire.as_str(), Some(o.as_str()), "{o}");
            let back: RequestOrigin = serde_json::from_value(wire).expect("deserialise");
            assert_eq!(back, *o);
            // And the name a user may type parses to the same value.
            assert_eq!(RequestOrigin::parse(o.as_str()), Some(*o), "{o}");
        }
    }

    #[test]
    fn a_session_origin_survives_the_wire() {
        let o = SessionOrigin {
            origin: RequestOrigin::RemoteControl,
            source: OriginSource::Declared,
        };
        let text = serde_json::to_string(&o).expect("serialise");
        assert_eq!(serde_json::from_str::<SessionOrigin>(&text).unwrap(), o);
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["origin"], "claude-remote-control");
        assert_eq!(v["source"], "declared");
    }

    #[test]
    fn section_seven_table_is_reproduced_exactly() {
        // ROADMAP.md §7, transcribed. Written as data rather than as
        // assertions over `ruling`'s branches, so this fails if the code is
        // edited to say something the roadmap does not.
        //
        // | Capability                | Local        | Claude Remote Control    |
        let table: &[(Capability, Ruling, Ruling)] = &[
            (Capability::EditProject, Ruling::Allow, Ruling::Allow),
            (Capability::RunTests, Ruling::Allow, Ruling::Allow),
            (Capability::GitHubPush, Ruling::Allow, Ruling::Allow),
            (Capability::CloudflarePreviewDeploy, Ruling::Allow, Ruling::Allow),
            (
                Capability::ProductionDeploy,
                Ruling::Configurable,
                Ruling::Configurable,
            ),
            (Capability::ReadRawSecret, Ruling::Deny, Ruling::Deny),
            (
                Capability::RootCapability,
                Ruling::LocalAuth,
                Ruling::LocalApproval,
            ),
            (
                Capability::UnsafeEverything,
                Ruling::LocalAuth,
                Ruling::LocalApproval,
            ),
        ];
        assert_eq!(table.len(), Capability::ALL.len(), "a row is missing");
        for (cap, local, remote) in table {
            assert_eq!(
                cap.ruling(RequestOrigin::LocalTerminal),
                *local,
                "{cap} from a local terminal"
            );
            assert_eq!(
                cap.ruling(RequestOrigin::ApexShell),
                *local,
                "{cap} from the shell"
            );
            assert_eq!(
                cap.ruling(RequestOrigin::RemoteControl),
                *remote,
                "{cap} from remote control"
            );
        }
    }

    #[test]
    fn the_work_andre_does_remotely_is_allowed_remotely() {
        // §7's opening line: "Remote Control is a normal workflow, not an edge
        // case." Four rows of the table say so, and a policy that tightened
        // them because remote sounds riskier would be the wrong policy rather
        // than a cautious one.
        for cap in [
            Capability::EditProject,
            Capability::RunTests,
            Capability::GitHubPush,
            Capability::CloudflarePreviewDeploy,
        ] {
            for origin in RequestOrigin::ALL {
                let r = cap.ruling(*origin);
                assert_eq!(r, Ruling::Allow, "{cap} from {origin}");
                assert!(r.is_unattended());
                assert!(!r.needs_local_human());
            }
        }
    }

    #[test]
    fn elevation_needs_a_local_human_from_every_origin_including_a_local_one() {
        // The two rows §7 never lets happen unattended. Both columns end at a
        // human at this machine; what differs is whether that human is also
        // the one asking.
        for cap in [Capability::RootCapability, Capability::UnsafeEverything] {
            for origin in RequestOrigin::ALL {
                let r = cap.ruling(*origin);
                assert!(r.needs_local_human(), "{cap} from {origin} needs nobody");
                assert!(!r.is_unattended(), "{cap} from {origin} runs unattended");
                assert_eq!(
                    r == Ruling::LocalAuth,
                    origin.is_local(),
                    "{cap} from {origin}"
                );
            }
        }
    }

    #[test]
    fn an_unattended_origin_is_read_as_the_remote_column_not_the_local_one() {
        // §7's table names only two columns. A scheduled job, an MCP server, a
        // subagent and a cloud job are in neither by name, and reading them as
        // the local one — they are, after all, running on this machine — is
        // the mistake this pins.
        for origin in [
            RequestOrigin::ScheduledJob,
            RequestOrigin::Mcp,
            RequestOrigin::Subagent,
            RequestOrigin::CloudJob,
        ] {
            for cap in [Capability::RootCapability, Capability::UnsafeEverything] {
                assert_eq!(cap.ruling(origin), Ruling::LocalApproval, "{cap}/{origin}");
            }
        }
    }

    #[test]
    fn a_raw_secret_is_refused_from_the_keyboard_too() {
        // The row where the two columns agree for the opposite reason to the
        // first four. §3.2 does not have a "unless a human asks" clause.
        for origin in RequestOrigin::ALL {
            assert_eq!(
                Capability::ReadRawSecret.ruling(*origin),
                Ruling::Deny,
                "{origin}"
            );
        }
    }

    #[test]
    fn configurable_is_not_a_synonym_for_allowed() {
        // Production deploy is `configurable` in both columns, and the
        // tempting reading of that is "allowed once somebody sets it up",
        // which in an unconfigured build means allowed.
        for origin in RequestOrigin::ALL {
            let r = Capability::ProductionDeploy.ruling(*origin);
            assert_eq!(r, Ruling::Configurable);
            assert!(!r.is_unattended(), "{origin} deployed to production unasked");
        }
    }

    #[test]
    fn a_refusal_says_which_rule_stopped_it() {
        let claimed = may_declare(RequestOrigin::Mcp, RequestOrigin::LocalTerminal)
            .expect_err("must refuse");
        assert!(claimed.to_string().contains("local-terminal"), "{claimed}");
        let loosened = may_declare(RequestOrigin::RemoteControl, RequestOrigin::Mcp)
            .expect_err("must refuse");
        assert!(loosened.to_string().contains("more restricted"), "{loosened}");
    }
}
