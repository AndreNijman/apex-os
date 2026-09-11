//! Filing and deciding privilege requests, daemon side.
//!
//! The daemon's whole job here is to be the thing that cannot be talked out of
//! the truth. It answers three questions:
//!
//!   * *who is asking* — from [`crate::peer`], never from the request;
//!   * *is this in the vocabulary* — from [`apex_agent_core::request::Verb`],
//!     which cannot express an arbitrary command;
//!   * *has a human already allowed exactly this here* — from the grant store.
//!
//! It does not execute anything. `apex-agentd` is unprivileged, and keeping it
//! that way is the point: §2 says agent orchestration must not live inside the
//! privileged daemon. An approved request is run by `apex request approve`
//! under the approving human's own root, and the daemon only records that it
//! happened.

use std::sync::Arc;

use apex_agent_core::grant::GrantKind;
use apex_agent_core::origin::{OriginSource, SessionOrigin};
use apex_agent_core::paths;
use apex_agent_core::policy::{AgentPolicy, RequestOrigin};
use apex_agent_core::protocol::{ErrorKind, Response};
use apex_agent_core::request::{
    self, Decision, Grants, PrivilegeRequest, RequestError, Verb,
};

use crate::grants::{authenticate, Authenticated, GrantError};
use crate::peer::{self, Peer};
use crate::Daemon;

/// The connection a request arrived on.
///
/// Two things, and the reason they travel together is that neither is safe on
/// its own. [`Caller::peer`] is what the kernel says — the pid, uid and gid
/// `SO_PEERCRED` filled in at `connect(2)`, which no client can forge.
/// [`Caller::declared`] is a narrowing the connection asked for and was
/// granted, which is a claim — but a claim that [`apex_agent_core::origin::may_declare`]
/// has already refused unless it gives something up, re-checked against the
/// live observation every time it is read.
///
/// It replaces the bare `Option<Peer>` that used to be threaded through every
/// handler, and that is the point of the type rather than a second parameter:
/// a handler cannot read the peer and forget the declaration, because there is
/// nothing to read but this.
#[derive(Debug, Clone, Default)]
pub struct Caller {
    /// What the kernel says about the other end, or `None` when it would not
    /// say. `None` is unauthenticated, never trusted.
    pub peer: Option<Peer>,
    /// An origin this connection declared for itself, latched for the life of
    /// the connection.
    ///
    /// Only ever consulted for a connection that does not resolve to a managed
    /// session: a session's origin is the one recorded on the session, and a
    /// session narrows itself by narrowing that record.
    declared: Option<RequestOrigin>,
    /// Who the declaration was made for, from `DeclareOrigin`'s `actor`.
    actor: Option<String>,
}

impl Caller {
    /// A connection with nothing declared on it yet.
    pub fn new(peer: Option<Peer>) -> Caller {
        Caller {
            peer,
            declared: None,
            actor: None,
        }
    }

    /// Record an accepted declaration.
    ///
    /// Only [`declare`] calls this, and only after `may_declare` has passed.
    /// The value is stored rather than the resulting [`SessionOrigin`] so that
    /// [`origin`] re-derives it from the *live* observation on every request:
    /// a stored `SessionOrigin` would keep asserting a narrowing whose basis
    /// had gone, and re-deriving costs two `/proc` reads that were already
    /// happening.
    fn latch(&mut self, origin: RequestOrigin, actor: Option<String>) {
        self.declared = Some(origin);
        if actor.is_some() {
            self.actor = actor;
        }
    }
}

/// Which session a connection belongs to, what that session was doing, and
/// where it is being driven from.
///
/// `session` is `None` for a connection from an ordinary terminal — the user's
/// own shell running `apex request approve`, or APEX Shell. That is not an
/// error: an unsessioned peer is the human, and the human is who decides.
#[derive(Debug, Clone, Default)]
pub struct Origin {
    pub session: Option<u32>,
    pub agent: Option<String>,
    pub project: Option<String>,
    /// The permission dimensions the asking session runs under.
    ///
    /// Taken from what the daemon recorded when it forked the session, never
    /// from the request: a session that could name its own policy could name a
    /// looser one. An unsessioned peer is the human, whose policy is whatever
    /// their shell allows, so the default applies.
    pub policy: AgentPolicy,
    /// Where the connection came from (§7's `request_origin`), when that could
    /// be established.
    pub request_origin: Option<SessionOrigin>,
    /// Why [`Origin::request_origin`] is not a measurement, when it is not
    /// one.
    ///
    /// A separate field rather than a `Result`, so `Origin::default()` and the
    /// callers that only want `session` keep working, and so the reason
    /// survives to the refusal the user reads. `None` here with `None` above
    /// cannot happen from [`origin`]: one of the two is always set.
    pub origin_unreadable: Option<String>,
    /// Which remote actor this connection declared itself to be acting for.
    ///
    /// Only ever set from an accepted [`Request::DeclareOrigin`], so it exists
    /// exactly where `request_origin` is a declaration and is `None`
    /// everywhere else. A display string; never a key.
    pub actor: Option<String>,
}

impl Origin {
    /// An origin the daemon could not establish, with the reason.
    ///
    /// Deliberately not a fallback to [`RequestOrigin::default`], which is
    /// `local-terminal` — the origin §7 reserves root and break-glass for.
    /// "Could not tell" and "a human is at the keyboard" are different
    /// answers, and this is the whole defect this field exists to avoid.
    fn unreadable(why: impl Into<String>) -> Origin {
        Origin {
            origin_unreadable: Some(why.into()),
            ..Origin::default()
        }
    }

}

/// The origin of a connection that is not a managed session, with the
/// connection's own declaration applied over the observation.
///
/// The declaration is checked here, at the moment it is used, and not only
/// when it was accepted. Two reasons, and the second is the one that matters:
///
///  * the observation is re-read every request anyway, and a latch that
///    narrowed `scheduled-job` is a different claim once the peer's cgroup
///    says something else;
///  * `may_declare` is the whole security property, and a design where it runs
///    once and a stored value is trusted afterwards has moved the check away
///    from the thing it protects. Here, a narrowing that has stopped being a
///    narrowing simply stops applying, and the observation stands.
///
/// A declaration that is refused is not an error: the connection gets the
/// origin it actually has, which is never *less* restricted than what it
/// asked for. Refusing outright would let a client take a connection down by
/// asking for something it was already going to be denied.
fn connection_origin(observed: Result<RequestOrigin, String>, caller: &Caller) -> Origin {
    let observed = match observed {
        Ok(o) => o,
        Err(why) => return Origin::unreadable(why),
    };
    let base = SessionOrigin::observed(observed);
    let (request_origin, actor) = match caller.declared {
        Some(wanted) => match base.declare(wanted) {
            Ok(narrowed) => (narrowed, caller.actor.clone()),
            Err(_) => (base, None),
        },
        None => (base, None),
    };
    Origin {
        request_origin: Some(request_origin),
        actor,
        ..Origin::default()
    }
}

/// Resolve a connection to the session that owns it, and to §7's origin.
///
/// The pid comes from `SO_PEERCRED` and is walked up its `/proc` ancestry until
/// it meets a pid the daemon recorded when it forked a session. Nothing the
/// client sent is consulted.
///
/// A connection that belongs to a session **inherits that session's origin**
/// rather than being classified afresh. That is the point: a Remote Control
/// session's agent files a request from a local process, so re-observing it
/// would answer `local-terminal` every time and the second column of §7's
/// table would be unreachable. The session's origin is what is actually
/// driving it, and the daemon recorded it when it forked the session.
///
/// A connection that is **not** a session may have latched a narrower origin
/// onto itself with [`declare`], and that is applied here rather than at the
/// point it was accepted. Re-derived from the live observation on every
/// request, so the narrowing stands only while it is still a narrowing: a
/// latch is a claim, and the check that makes it safe is the one that runs
/// when it is used.
pub fn origin(daemon: &Arc<Daemon>, caller: &Caller) -> Origin {
    let Some(peer) = caller.peer else {
        return Origin::unreadable(
            "the kernel would not report the peer credentials of this connection, so there is \
             no way to tell where it came from",
        );
    };
    // Where a connection came from is a question about the process, not about
    // which account it runs as, so it is answered before the uid is looked at.
    //
    // That ordering matters for exactly one caller and it is the important
    // one: `apex request approve` runs under `sudo`, so it reaches this socket
    // as uid 0. It is still a human at a terminal, and §4's whole approval
    // path is that command. Classifying it from its cgroup — which is
    // world-readable and which sudo does not let a process choose — is what
    // keeps that true without widening anything: root can already read the
    // request files and run the operation directly.
    let observed = crate::origin::observe(&peer);

    if !peer::is_own_user(&peer) {
        // Not this user's process, so it is not one of this user's sessions —
        // the ancestry walk below would be attributing a foreign process to a
        // session it cannot be inside. The origin still stands.
        return connection_origin(observed, caller);
    }

    // Snapshot (pid, id) pairs, then release the lock: the ancestry walk does
    // filesystem I/O and must not hold the registry while it does.
    let live: Vec<(libc::pid_t, u32)> = {
        let reg = daemon.registry.lock().expect("registry lock");
        reg.list()
            .iter()
            .filter_map(|h| {
                let s = h.lock().ok()?;
                Some((s.pid, s.info.id))
            })
            .collect()
    };

    let matched = if live.is_empty() {
        None
    } else {
        peer::resolve_by_ancestry(peer.pid, |p| live.iter().any(|(pid, _)| *pid == p))
    };

    let Some(found) = matched.and_then(|f| live.iter().find(|(pid, _)| *pid == f).map(|(_, id)| *id))
    else {
        // Not a managed session, so the peer's own classification is the
        // answer — narrowed by whatever this connection has latched.
        return connection_origin(observed, caller);
    };

    // Re-take the lock for the details, so the snapshot above stays short.
    let reg = daemon.registry.lock().expect("registry lock");
    let found_session = reg.get(found).and_then(|h| {
        h.lock().ok().map(|s| {
            (
                s.info.agent.clone(),
                s.info.project.clone(),
                s.info.policy,
                s.info.request_origin,
            )
        })
    });
    match found_session {
        Some((agent, project, policy, recorded)) => Origin {
            session: Some(found),
            agent: Some(agent),
            project,
            policy,
            actor: None,
            request_origin: recorded.map(SessionOrigin::inherited),
            // A session record from a daemon that predates origin tracking.
            // Not treated as local: the field was never written, and an
            // absent field is not a measurement.
            origin_unreadable: recorded.is_none().then(|| {
                format!(
                    "session {found} was started before this runtime recorded origins, so where \
                     it is driven from was never established"
                )
            }),
        },
        None => Origin {
            session: Some(found),
            agent: None,
            project: None,
            // A session whose record vanished between the ancestry walk and
            // this lookup gets the default, which is the strict value for
            // every dimension. Failing toward the loose one here would make a
            // race into a permission.
            policy: AgentPolicy::default(),
            request_origin: None,
            actor: None,
            origin_unreadable: Some(format!(
                "session {found} disappeared while this request was being attributed"
            )),
        },
    }
}

/// Whether a connection may write text into session `target`'s terminal.
///
/// `None` means it may. A [`Response`] means it may not, and says why.
///
/// A function over an established [`Origin`] and nothing else, so the decision
/// is testable without a daemon, a socket or a session — which is the pattern
/// the rest of this file follows, and the reason `tests/request_origin.rs`
/// exists only for the two claims that genuinely need a spawned process.
///
/// `Request::Input` is the one verb on this socket that acts on a session
/// other than the caller's own AND is invisible to the target as anything but
/// a person at the keyboard. `Signal` reaches another session too, but an
/// agent sees a signal as a signal; bytes on a PTY are typing. So a session
/// that could send them could instruct a sibling agent and borrow whatever
/// that sibling was granted, which is dimension 3 leaking sideways.
///
/// Two refusals, not one, and the second is the one worth writing down:
///
///  * the caller IS a session. Refused.
///  * the caller could not be classified at all. Also refused. A connection
///    whose credentials the kernel would not report is not evidence of a
///    human; treating "could not read" as "not a session" is exactly the
///    defect [`Origin::unreadable`] exists to prevent, one verb further on.
///    Fail closed: the cost is a `apex agent input` that reports it could not
///    establish the caller, against an agent quietly driving another agent.
pub fn refuse_input(who: &Origin, target: u32) -> Option<Response> {
    if let Some(caller) = who.session {
        return Some(Response::error(
            ErrorKind::PermissionDenied,
            format!(
                "session {caller} may not write into session {target}'s terminal; text on a \
                 terminal cannot be told apart from a person typing"
            ),
        ));
    }
    if let Some(why) = who.origin_unreadable.as_deref() {
        return Some(Response::error(
            ErrorKind::PermissionDenied,
            format!(
                "refusing to write into session {target}: this connection could not be \
                 classified, so there is no way to tell it is not a session driving another \
                 session ({why})"
            ),
        ));
    }
    None
}

/// Record a narrower origin (§7), for a session or for a connection.
///
/// Two callers, one rule. A **session** narrows its own record, permanently:
/// the session comes from the peer credentials, so there is no id to get wrong
/// or to forge, and every later request the session makes inherits it. That is
/// the route Remote Control takes — it is switched on after `claude` has
/// started, so nothing observable about the connection ever changes and an
/// observation alone could never reach §7's second column.
///
/// A **connection that is not a session** narrows itself, for as long as it is
/// open. That is the route a remote proxy takes, and it is the case the daemon
/// had no answer for: `apex-remoted` is not a session and never will be, so
/// every request it forwarded for a paired phone would carry the origin its
/// own cgroup implies. Started as a user unit that is `scheduled-job`, which
/// is at least not local; started from a login session — a developer running
/// it in a terminal, a shell launching it as a child — it is `local-terminal`,
/// and `decide` accepts that as a human at the keyboard. A remote request
/// approving root is precisely what §7 forbids, and nothing in the daemon
/// stood between the two.
///
/// Both routes go through [`apex_agent_core::origin::may_declare`], which
/// accepts a declaration only when it gives something up, and neither can
/// produce a local origin: [`RequestOrigin::may_be_declared`] refuses those by
/// name, whatever was observed.
///
/// `caller` is `&mut` for the connection case, which is the only mutation:
/// the latch lives on the connection, not in any store, so it dies with the
/// socket and cannot be inherited by anything else.
pub fn declare(
    daemon: &Arc<Daemon>,
    caller: &mut Caller,
    wanted: &str,
    actor: Option<String>,
) -> Response {

    let Some(wanted) = RequestOrigin::parse(wanted) else {
        return Response::error(
            ErrorKind::BadRequest,
            format!(
                "'{wanted}' is not an origin; use one of {}",
                RequestOrigin::ALL
                    .iter()
                    .filter(|o| o.may_be_declared())
                    .map(|o| o.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        );
    };

    let who = origin(daemon, caller);
    let Some(session) = who.session else {
        return declare_on_connection(who, caller, wanted, actor);
    };
    let Some(current) = who.request_origin else {
        return Response::error(
            ErrorKind::PermissionDenied,
            who.origin_unreadable
                .unwrap_or_else(|| "this session has no recorded origin".to_string()),
        );
    };
    let narrowed = match current.declare(wanted) {
        Ok(o) => o,
        Err(e) => return Response::error(ErrorKind::PermissionDenied, e.to_string()),
    };

    let reg = daemon.registry.lock().expect("registry lock");
    let Some(handle) = reg.get(session) else {
        return Response::error(ErrorKind::NoSuchSession, format!("no session {session}"));
    };
    let mut s = handle.lock().expect("session lock");
    s.info.request_origin = Some(narrowed.origin);
    s.info.origin_source = Some(narrowed.source);
    crate::registry::write_record(&s.info);
    Response::Session(Box::new(s.info.clone()))
}

/// Latch a narrower origin onto a connection that is not a session.
///
/// Answers [`Response::Ok`] rather than a record, because there is no record:
/// the latch is connection state and dies with the socket. `Ok` is still the
/// answer a proxy needs, and it needs it to be load-bearing — `apex-remoted`
/// declares before it forwards anything and refuses to serve the device if
/// this comes back an error, because a proxy whose declaration was dropped is
/// a proxy forwarding remote requests under a local origin.
///
/// An unreadable observation is refused, not ignored. The latch would be inert
/// — [`unsessioned`] never applies one over an origin it could not establish —
/// and a silently inert narrowing is the failure mode this whole module is
/// written against.
fn declare_on_connection(
    who: Origin,
    caller: &mut Caller,
    wanted: RequestOrigin,
    actor: Option<String>,
) -> Response {
    let Some(current) = who.request_origin else {
        return Response::error(
            ErrorKind::PermissionDenied,
            who.origin_unreadable.unwrap_or_else(|| {
                "the origin of this connection could not be established, so there is nothing to \
                 narrow"
                    .to_string()
            }),
        );
    };
    if let Err(e) = current.declare(wanted) {
        return Response::error(ErrorKind::PermissionDenied, e.to_string());
    }
    if let Some(bad) = actor.as_deref().and_then(unprintable_actor) {
        return Response::error(ErrorKind::BadRequest, bad);
    }
    caller.latch(wanted, actor);
    Response::Ok
}

/// The longest actor id the daemon will record.
///
/// Bounded because it is written into records the user reads and into the
/// audit log, and an unbounded string from a client is a line of that log a
/// client chose the length of.
const MAX_ACTOR: usize = 64;

/// Why an actor id is not fit to be recorded, or `None` when it is.
///
/// Refused rather than sanitised. The value ends up on a privilege prompt a
/// human reads before handing out root, and a control character there can
/// rewrite the line around it — an actor called `phone\r\nAPPROVED` is a
/// display attack, not a naming choice. Silently stripping it would leave the
/// caller believing they named a device that the record calls something else.
fn unprintable_actor(actor: &str) -> Option<String> {
    if actor.is_empty() {
        return Some("an actor was named but it is empty; omit the field instead".to_string());
    }
    if actor.chars().count() > MAX_ACTOR {
        return Some(format!(
            "an actor id may be at most {MAX_ACTOR} characters; this one is {}",
            actor.chars().count()
        ));
    }
    if actor.chars().any(|c| c.is_control()) {
        return Some(
            "an actor id may not contain control characters: it is printed on the prompt a human \
             reads before approving a root operation"
                .to_string(),
        );
    }
    None
}

/// The origin a new session gets, and the error when it cannot have one.
///
/// Split out of `session::start` so the rule is one readable function: a Run
/// from inside a session produces a child of that session's origin, a Run from
/// anywhere else is classified from the connection, and a declaration is
/// applied on top of whichever it was.
pub fn for_new_session(
    who: &Origin,
    declared: Option<apex_agent_core::policy::RequestOrigin>,
) -> Result<SessionOrigin, String> {
    let base = match (&who.request_origin, who.session) {
        (Some(parent), Some(_)) => SessionOrigin {
            origin: apex_agent_core::origin::child_of(parent.origin),
            source: OriginSource::Inherited,
        },
        (Some(observed), None) => *observed,
        (None, _) => {
            return Err(who
                .origin_unreadable
                .clone()
                .unwrap_or_else(|| "the origin of this connection could not be established".into()))
        }
    };
    match declared {
        None => Ok(base),
        Some(wanted) => base.declare(wanted).map_err(|e| e.to_string()),
    }
}

/// The one gate every system-access grant passes through, in the one order
/// that makes it mean anything (§3.3, §4.4, §7).
///
/// Four steps, and the first three are refusals that happen **before** polkit
/// is asked anything. That ordering is not tidiness: it is what makes the
/// checks testable against a real daemon with no risk of a password dialog,
/// and it is what puts the eventual prompt outside the agent's terminal.
///
///   1. **Not from inside a session.** This is P0-007's fourth criterion —
///      "the agent cannot renew its own grant" — and it is the only form that
///      criterion can take. The agent runs as the user, so "is this the user?"
///      cannot distinguish it from the human; every uid, every group, every
///      environment variable and every field in the request says the same
///      thing for both. What differs is the *connection*: the peer pid comes
///      from `SO_PEERCRED`, which the kernel fills in at `connect(2)`, and
///      `peer::resolve_by_ancestry` walks it up `/proc` to the pid the daemon
///      recorded when it forked the session. Every process inside the sandbox
///      is a descendant of that pid, and a process cannot choose its parent.
///
///      The escape a reader will think of — reparenting to pid 1 by orphaning
///      — does not work either, and not by luck: an orphan of a session is
///      reparented inside the user manager, so its cgroup is
///      `…/user@N.service/…`, and [`crate::origin::classify`] reads that as
///      `scheduled-job`, which fails step 2. The two mechanisms cover each
///      other, which is why both are here.
///
///   2. **Local origin.** §7's table gives root capability and
///      unsafe-everything "local auth" locally and "local approval required"
///      from everywhere else. A `scheduled-job`, an `mcp` server and a
///      `subagent` are all non-local by [`RequestOrigin::is_local`], because
///      the property that matters is whether a human is present.
///
///   3. **An origin at all.** An origin that could not be established is
///      refused, never defaulted — `RequestOrigin::default()` is
///      `local-terminal`, which is precisely the column being asked for.
///
///   4. **polkit**, with the peer as the subject.
pub fn may_be_granted(who: &Origin, what: &'static str) -> Result<RequestOrigin, GrantError> {
    if let Some(session) = who.session {
        return Err(GrantError::FromInsideASession { session, what });
    }
    let Some(source) = who.request_origin else {
        return Err(GrantError::OriginUnknown(
            who.origin_unreadable
                .clone()
                .unwrap_or_else(|| "the origin of this connection could not be established".into()),
        ));
    };
    if !source.is_local() {
        return Err(GrantError::NotLocal {
            origin: source.origin,
            what,
        });
    }
    Ok(source.origin)
}

/// [`may_be_granted`], then the password.
///
/// Returns the origin to record on the grant alongside the proof that a human
/// was asked. The proof is a token only this function and its sibling can
/// produce, so `GrantAuthority::issue` cannot be reached without one.
pub fn authorise_grant(
    daemon: &Arc<Daemon>,
    who: &Origin,
    caller: &Caller,
    kind: GrantKind,
    what: &'static str,
) -> Result<(RequestOrigin, Authenticated), GrantError> {
    let origin = may_be_granted(who, what)?;
    let Some(peer) = caller.peer else {
        return Err(GrantError::OriginUnknown(
            "the kernel would not report the peer credentials of this connection".into(),
        ));
    };
    let proof = authenticate(daemon.auth.as_ref(), kind, &peer)?;
    Ok((origin, proof))
}

/// Every system-access grant, with the state each is in now.
///
/// Readable from anywhere, including from inside a session: an agent being
/// able to see that it holds a grant and when it runs out is the opposite of
/// a risk, and §3.4's "revocation control always visible" wants the listing
/// available wherever somebody is looking.
pub fn system_grants(daemon: &Arc<Daemon>) -> Response {
    let now = request::now_ms();
    let listed = daemon.grants.list(now);
    Response::SystemGrants {
        grants: listed.iter().map(|(g, _, _)| g.clone()).collect(),
        states: listed
            .iter()
            .map(|(_, state, said)| (state.as_str().to_string(), said.clone()))
            .collect(),
    }
}

/// Take a grant back before its window runs out.
///
/// §3.4: "revocation control always visible". No authentication — giving up
/// privilege is free, the same rule [`apex_agent_core::auth::required_for`]
/// states over values and the same one P0-016's toggle follows. It still
/// refuses a connection from inside a session, because a session revoking
/// ANOTHER session's grant is a session changing somebody else's permissions.
pub fn revoke_system_grant(daemon: &Arc<Daemon>, caller: &Caller, id: u32) -> Response {
    let who = origin(daemon, caller);
    if let Some(session) = who.session {
        return Response::error(
            ErrorKind::PermissionDenied,
            GrantError::FromInsideASession {
                session,
                what: "revoke a system-access grant",
            }
            .to_string(),
        );
    }
    match daemon.grants.revoke(id, request::now_ms()) {
        Ok(g) => Response::SystemGrants {
            states: vec![(
                g.state_at(request::now_ms(), daemon.grants.boot())
                    .as_str()
                    .to_string(),
                g.describe(request::now_ms(), daemon.grants.boot()),
            )],
            grants: vec![g],
        },
        Err(e @ GrantError::NoSuchGrant(_)) => {
            Response::error(ErrorKind::NoSuchRequest, e.to_string())
        }
        Err(e) => Response::error(ErrorKind::BadRequest, e.to_string()),
    }
}

/// Extend a grant that is still in force.
///
/// The verb P0-007's fourth criterion is about. It goes through
/// [`authorise_grant`], so the agent holding the grant is refused at step 1 —
/// before polkit is asked and regardless of what it sends — and a fresh
/// password is required of everybody else. A renewal is a new window on a new
/// consent, not a longer one on the old.
pub fn renew_system_grant(
    daemon: &Arc<Daemon>,
    caller: &Caller,
    id: u32,
    ttl_ms: u64,
) -> Response {
    let who = origin(daemon, caller);
    // Who is asking, before what they are asking about. Two reasons, and the
    // first is the criterion: a session must be told that renewing is not a
    // session's business, not that the grant it named has gone — a refusal
    // that depended on which id was passed would be one an agent could probe
    // its way around. The second is that neither check needs polkit, so the
    // ordering costs nothing.
    if let Err(e) = may_be_granted(&who, "renew a system-access grant") {
        return Response::error(ErrorKind::PermissionDenied, e.to_string());
    }
    // Then: the grant has to exist and be alive before anybody is asked for a
    // password. Prompting for a grant that has already expired teaches people
    // to type their password at dialogs that achieve nothing.
    let Some(kind) = daemon
        .grants
        .list(request::now_ms())
        .into_iter()
        .find(|(g, state, _)| g.id == id && state.is_active())
        .map(|(g, _, _)| g.kind)
    else {
        return Response::error(
            ErrorKind::NoSuchRequest,
            format!(
                "no active system-access grant {id}; a grant is issued to a session when it \
                 starts, so start a new session with the mode you need"
            ),
        );
    };
    let proof = match authorise_grant(daemon, &who, caller, kind, "renew a system-access grant") {
        Ok((_, proof)) => proof,
        Err(e) => return Response::error(ErrorKind::PermissionDenied, e.to_string()),
    };
    match daemon.grants.renew(proof, id, ttl_ms, request::now_ms()) {
        Ok(g) => Response::SystemGrants {
            states: vec![(
                g.state_at(request::now_ms(), daemon.grants.boot())
                    .as_str()
                    .to_string(),
                g.describe(request::now_ms(), daemon.grants.boot()),
            )],
            grants: vec![g],
        },
        Err(e) => Response::error(ErrorKind::BadRequest, e.to_string()),
    }
}


/// Which authority decided a request, and how the record must say so.
///
/// Two things can decide a request before a human sees it, and they are not
/// the same decision:
///
/// * a standing per-project grant (P0-013) — a human said yes to this
///   operation in this project, once, and meant it to keep holding. The
///   record says `allow_for_project`, which is what the next identical
///   request in that project will find.
/// * a session-scoped system-access grant (§4.4) — a human authenticated at
///   the top of *this* session for a stated window. It decided this request
///   and nothing beyond it: the window can be revoked or run out before the
///   next one. The record says `allow_once` and names the grant, so this
///   trail can be joined to the `APEX_GRANT_ID` journald holds for the same
///   window.
///
/// Recording a session grant as `allow_for_project` would put a standing
/// grant in the trail that no human ever made and that nothing on disk
/// backs — the audit would be describing an authority that does not exist.
///
/// A project grant wins when both apply, and the request is *not* attributed
/// to the session grant: it would have been allowed with no window open.
fn decided_by(
    by_project: bool,
    by_grant: Option<u32>,
) -> (Option<Decision>, Option<u32>, &'static str) {
    match (by_project, by_grant) {
        (true, _) => (Some(Decision::AllowForProject), None, "requested-and-granted"),
        (false, Some(id)) => (Some(Decision::AllowOnce), Some(id), "requested-and-covered"),
        (false, None) => (None, None, "requested"),
    }
}

/// File a request.
///
/// The verb is parsed here rather than accepted pre-parsed, so a client cannot
/// hand over a `Verb` that never went through validation. On a project where a
/// human has already allowed exactly this, the request is recorded as decided
/// and needs no prompt — see [`Decision::AllowForProject`] for what that does
/// and does not buy.
pub fn file(daemon: &Arc<Daemon>, caller: &Caller, verb: &str, args: &[String], reason: &str) -> Response {
    let parsed = match Verb::parse(verb, args) {
        Ok(v) => v,
        Err(e) => return refusal(e),
    };
    let reason = match request::check_reason(reason) {
        Ok(r) => r,
        Err(e) => return refusal(e),
    };

    let who = origin(daemon, caller);

    // Every verb in this vocabulary is a root operation, and §7's table gives
    // a different answer for a remote origin than a local one. A request the
    // daemon cannot attribute to an origin therefore cannot be filed: the
    // human deciding it would be reading a prompt that could not tell them
    // which column of that table they were in.
    let Some(source) = who.request_origin else {
        return Response::error(
            ErrorKind::PermissionDenied,
            format!(
                "{}, so this request cannot be attributed to an origin and will not be filed",
                who.origin_unreadable
                    .unwrap_or_else(|| "the origin could not be established".to_string())
            ),
        );
    };

    let dir = request::requests_dir();
    let grants = Grants::load(&request::grants_file());
    // Two ways a request can arrive already decided, and they are different
    // things. A per-project grant (P0-013) is a standing decision a human made
    // about this project and it outlives every session. A system-access grant
    // (§4.4) is a decision a human made about THIS session, for a stated
    // window, and it dies with the session or the clock.
    //
    // The system-access half is read from the daemon's memory, never from the
    // grant store, because the session this is being decided for can write the
    // store — see `grants.rs`.
    //
    // They are recorded differently for the same reason. A project grant is a
    // standing decision, so the record says `allow_for_project` and the next
    // identical request in that project finds it again. A session grant is
    // not: it decided this one request, and the window it came from may be
    // gone by the next, so the record says `allow_once` and names the grant.
    let now = request::now_ms();
    let by_project = grants.allows(who.project.as_deref(), &parsed);
    let by_grant = daemon
        .grants
        .covering_grant(who.session, parsed.name(), now);
    let (decision, attributed, event) = decided_by(by_project, by_grant);

    let req = PrivilegeRequest {
        id: request::next_id(&dir),
        verb: parsed,
        reason,
        session: who.session,
        agent: who.agent,
        project: who.project,
        request_origin: Some(source.origin),
        origin_source: Some(source.source),
        actor: who.actor,
        decision: decision.unwrap_or(Decision::Pending),
        created_ms: now,
        decided_ms: decision.map(|_| now),
        executed_ms: None,
        exit_code: None,
        system_grant: attributed,
    };

    if let Err(e) = request::save(&dir, &req) {
        return Response::error(ErrorKind::Internal, format!("recording the request: {e}"));
    }
    let _ = request::audit(&request::audit_log(), event, &req);
    Response::Request(Box::new(req))
}

/// Every request on disk.
pub fn list() -> Response {
    match request::list(&request::requests_dir()) {
        Ok(requests) => Response::Requests { requests },
        Err(e) => Response::error(ErrorKind::Internal, format!("reading requests: {e}")),
    }
}

/// Record a human's decision.
///
/// Refuses to re-decide a request that has already been decided. Without that,
/// a denied request could be flipped to approved by anything that can reach the
/// socket — including the agent that filed it.
pub fn decide(daemon: &Arc<Daemon>, caller: &Caller, id: u32, decision: Decision) -> Response {
    // A session may not decide its own requests, or the approval step is
    // decoration. Only an unsessioned peer — the human's shell, or the shell
    // itself — may decide.
    let who = origin(daemon, caller);
    if let Some(session) = who.session {
        return Response::error(
            ErrorKind::PermissionDenied,
            format!(
                "session {session} cannot decide its own privilege request; \
                 a human decides with `apex request allow|deny`"
            ),
        );
    }

    // §7, both columns: "Root capability — Local: local auth. Claude Remote
    // Control: local approval required." Every verb in the vocabulary is a
    // root capability, so both columns end in the same place — a human at
    // this machine — and this is the check that makes that true.
    //
    // Without it, a systemd user timer running `apex request allow 3` is an
    // unattended approval of root: it is not a session, so the check above
    // lets it through, and it is not a human either. That is the hole this
    // closes, and it is the whole of "root/unsafe mode requires local
    // approval by default".
    //
    // Refused rather than defaulted when the origin could not be established.
    // The message names what could not be read, because the alternative to
    // being able to act on it is a machine on which nothing can be approved
    // and nothing says why.
    let Some(decider) = who.request_origin else {
        return Response::error(
            ErrorKind::PermissionDenied,
            format!(
                "{}, so this connection cannot be shown to be a human at this machine — and \
                 §7 reserves approving a root operation for one",
                who.origin_unreadable
                    .unwrap_or_else(|| "the origin could not be established".to_string())
            ),
        );
    };
    if !decider.is_local() {
        return Response::error(
            ErrorKind::PermissionDenied,
            format!(
                "a {} request cannot approve a root operation; §7 reserves that for a human at \
                 this machine, whichever origin filed it. Approve it from a terminal or from \
                 the Agent Center",
                decider.origin
            ),
        );
    }
    if decision == Decision::Pending {
        return Response::error(
            ErrorKind::BadRequest,
            "'pending' is not a decision".to_string(),
        );
    }

    let dir = request::requests_dir();
    let mut req = match request::load(&dir, id) {
        Ok(Some(r)) => r,
        Ok(None) => {
            return Response::error(ErrorKind::NoSuchRequest, format!("no request {id}"));
        }
        Err(e) => return Response::error(ErrorKind::Internal, format!("reading request {id}: {e}")),
    };
    if req.decision != Decision::Pending {
        return Response::error(
            ErrorKind::BadRequest,
            format!(
                "request {id} was already {}; file a new one rather than \
                 changing a recorded decision",
                req.decision.as_str()
            ),
        );
    }

    req.decision = decision;
    req.decided_ms = Some(request::now_ms());

    if decision == Decision::AllowForProject {
        match req.project.clone() {
            Some(project) => {
                let path = request::grants_file();
                let mut grants = Grants::load(&path);
                grants.allow(&project, &req.verb);
                if let Err(e) = grants.save(&path) {
                    return Response::error(
                        ErrorKind::Internal,
                        format!("recording the grant: {e}"),
                    );
                }
            }
            None => {
                // Nothing to scope the grant to. Downgraded rather than
                // silently stored globally: a grant with no project is a grant
                // for every project, which is not what the user chose.
                req.decision = Decision::AllowOnce;
            }
        }
    }

    if let Err(e) = request::save(&dir, &req) {
        return Response::error(ErrorKind::Internal, format!("saving request {id}: {e}"));
    }
    let _ = request::audit(&request::audit_log(), "decided", &req);
    Response::Request(Box::new(req))
}

/// Mark an approved request as executed, recording its exit status.
///
/// Called by `apex request approve` after it has run the operation as root.
/// Refuses a request that is not approved, and refuses one that has already
/// run — an approval is for one execution.
pub fn executed(id: u32, exit_code: i32) -> Response {
    let dir = request::requests_dir();
    let mut req = match request::load(&dir, id) {
        Ok(Some(r)) => r,
        Ok(None) => return Response::error(ErrorKind::NoSuchRequest, format!("no request {id}")),
        Err(e) => return Response::error(ErrorKind::Internal, format!("reading request {id}: {e}")),
    };
    if !req.decision.is_allowed() {
        return Response::error(
            ErrorKind::PermissionDenied,
            format!("request {id} is {}, not approved", req.decision.as_str()),
        );
    }
    if req.executed_ms.is_some() {
        return Response::error(
            ErrorKind::BadRequest,
            format!("request {id} has already been executed"),
        );
    }
    req.executed_ms = Some(request::now_ms());
    req.exit_code = Some(exit_code);
    if let Err(e) = request::save(&dir, &req) {
        return Response::error(ErrorKind::Internal, format!("saving request {id}: {e}"));
    }
    let _ = request::audit(&request::audit_log(), "executed", &req);
    Response::Request(Box::new(req))
}

/// The recorded grants.
pub fn grants() -> Response {
    let g = Grants::load(&request::grants_file());
    Response::Grants {
        projects: g.projects,
    }
}

/// Drop a grant, or every grant for a project when `key` is `None`.
pub fn revoke(daemon: &Arc<Daemon>, caller: &Caller, project: &str, key: Option<&str>) -> Response {
    // Revoking is a policy change, so it is subject to the same rule as
    // deciding: a session cannot widen or narrow its own permissions.
    if let Some(session) = origin(daemon, caller).session {
        return Response::error(
            ErrorKind::PermissionDenied,
            format!("session {session} cannot change its own grants"),
        );
    }
    let path = request::grants_file();
    let mut g = Grants::load(&path);
    let removed = match key {
        Some(k) => usize::from(g.revoke(project, k)),
        None => g.revoke_project(project),
    };
    if removed == 0 {
        return Response::error(
            ErrorKind::NoSuchRequest,
            format!("nothing granted for {project}"),
        );
    }
    if let Err(e) = g.save(&path) {
        return Response::error(ErrorKind::Internal, format!("saving grants: {e}"));
    }
    Response::Grants {
        projects: g.projects,
    }
}

/// Turn a validation failure into a response. Always `BadRequest`: these are
/// all "you asked for something that is not askable", never internal faults.
fn refusal(e: RequestError) -> Response {
    Response::error(ErrorKind::BadRequest, e.to_string())
}

/// Ensure the state directory exists, so a first request does not fail on a
/// missing path. `0700`, because the pending requests are a description of what
/// privileged operations this machine is about to run.
pub fn ensure_dirs() {
    let _ = paths::ensure_private_dir(&request::requests_dir());
}

#[cfg(test)]
mod tests {
    use super::*;
    use apex_agent_core::origin::SessionOrigin;

    /// A caller with no peer, carrying a latch. Enough for
    /// [`connection_origin`], which never touches the peer itself.
    fn latched(origin: RequestOrigin, actor: Option<&str>) -> Caller {
        let mut c = Caller::new(None);
        c.latch(origin, actor.map(|a| a.to_string()));
        c
    }

    #[test]
    fn a_latch_narrows_an_observation_and_says_it_was_declared() {
        let who = connection_origin(
            Ok(RequestOrigin::LocalTerminal),
            &latched(RequestOrigin::RemoteControl, Some("pixel-8")),
        );
        let got = who.request_origin.expect("an origin");
        assert_eq!(got.origin, RequestOrigin::RemoteControl);
        assert_eq!(got.source, OriginSource::Declared);
        assert_eq!(who.actor.as_deref(), Some("pixel-8"));
    }

    #[test]
    fn a_latch_that_would_loosen_is_ignored_and_the_observation_stands() {
        // The use-time half of the property. A latch set when the peer was a
        // login session must not survive the peer becoming something the
        // narrowing no longer narrows — here `mcp` over `claude-remote-control`,
        // which drops the lock gate.
        let who = connection_origin(
            Ok(RequestOrigin::RemoteControl),
            &latched(RequestOrigin::Mcp, Some("pixel-8")),
        );
        let got = who.request_origin.expect("an origin");
        assert_eq!(got.origin, RequestOrigin::RemoteControl);
        assert_eq!(got.source, OriginSource::Observed);
        assert_eq!(
            who.actor, None,
            "an actor survived a declaration that did not"
        );
    }

    #[test]
    fn the_pair_a_remote_proxy_actually_declares_is_accepted() {
        // `apex-remoted` runs as a systemd user service, so what it declares
        // in production is `scheduled-job` -> `claude-remote-control`. Every
        // test that drives it declares from a login session instead, because
        // that is what `cargo test` is — so if this pair ever stopped being
        // accepted, the feature would be dead in production while the whole
        // suite stayed green.
        //
        // It has to be accepted for a reason worth stating: `scheduled-job`
        // is already non-local, so the declaration buys no elevation. What it
        // buys is the LOCK GATE, which `scheduled-job` does not carry and
        // Remote Control does — a human elsewhere driving the machine past a
        // locked screen is the case §7 says the owner is asked about.
        let who = connection_origin(
            Ok(RequestOrigin::ScheduledJob),
            &latched(RequestOrigin::RemoteControl, Some("pixel-8")),
        );
        let got = who.request_origin.expect("an origin");
        assert_eq!(got.origin, RequestOrigin::RemoteControl);
        assert_eq!(got.source, OriginSource::Declared);
        assert!(!got.origin.is_local(), "the production pair reached a local origin");
        assert!(
            got.origin.lock_gated() && !RequestOrigin::ScheduledJob.lock_gated(),
            "the declaration did not take on the lock gate, which is what it is for"
        );
        assert_eq!(who.actor.as_deref(), Some("pixel-8"));
    }

    #[test]
    fn no_latch_leaves_the_observation_exactly_as_it_was() {
        for o in RequestOrigin::ALL {
            let who = connection_origin(Ok(*o), &Caller::new(None));
            let got = who.request_origin.expect("an origin");
            assert_eq!(got.origin, *o);
            assert_eq!(got.source, OriginSource::Observed);
            assert!(who.actor.is_none());
        }
    }

    #[test]
    fn a_latch_cannot_rescue_an_observation_that_could_not_be_read() {
        // The defect class this codebase keeps finding: "could not tell" must
        // not become an answer. A declaration over an unreadable observation
        // has no basis to be more restricted than, so there is nothing to
        // apply it to.
        let who = connection_origin(
            Err("/proc/1234/cgroup: no such file".into()),
            &latched(RequestOrigin::RemoteControl, None),
        );
        assert!(who.request_origin.is_none());
        assert!(who
            .origin_unreadable
            .as_deref()
            .unwrap_or_default()
            .contains("cgroup"));
    }

    #[test]
    fn no_latch_can_ever_produce_a_local_origin() {
        // Over the whole product, not over the pairs someone thought of.
        // `declare_on_connection` refuses a local wanted by name before it
        // latches, so this asserts the property from the other side: even if
        // one were latched, applying it must not produce a local origin from
        // a non-local observation.
        for observed in RequestOrigin::ALL {
            for wanted in RequestOrigin::ALL {
                let who = connection_origin(Ok(*observed), &latched(*wanted, None));
                let got = who.request_origin.expect("an origin").origin;
                assert!(
                    !got.is_local() || observed.is_local(),
                    "{observed} + a latch of {wanted} produced the local origin {got}"
                );
            }
        }
    }

    #[test]
    fn an_actor_is_refused_for_the_reasons_it_is_refused_for() {
        assert!(unprintable_actor("pixel-8-office").is_none());
        assert!(unprintable_actor("Andre's phone (kitchen)").is_none());
        assert!(unprintable_actor("").is_some(), "an empty actor");
        assert!(
            unprintable_actor(&"a".repeat(MAX_ACTOR)).is_none(),
            "the bound is inclusive"
        );
        assert!(unprintable_actor(&"a".repeat(MAX_ACTOR + 1)).is_some());
        for bad in ["a\nb", "a\rb", "a\u{1b}[2Kb", "a\tb", "a\0b"] {
            assert!(
                unprintable_actor(bad).is_some(),
                "{bad:?} was accepted onto a root approval prompt"
            );
        }
        // Counted in characters, not bytes: a name in a non-Latin script must
        // not be refused for being long when it is not.
        assert!(unprintable_actor(&"é".repeat(MAX_ACTOR)).is_none());
    }

    fn unsessioned(origin: RequestOrigin) -> Origin {
        Origin {
            request_origin: Some(SessionOrigin::observed(origin)),
            ..Origin::default()
        }
    }

    #[test]
    fn a_person_at_a_terminal_may_write_into_a_session() {
        // The whole point of the verb. A connection this daemon classified and
        // found outside every session is the shell's push-to-talk route or a
        // person running `apex agent input`, and it is allowed.
        let who = unsessioned(RequestOrigin::LocalTerminal);
        assert!(refuse_input(&who, 4).is_none());
    }

    #[test]
    fn a_session_may_not_write_into_another_session() {
        // Dimension 3 leaking sideways: an agent that could type into a
        // sibling could ask it to run what the sibling was granted and the
        // caller was not. The refusal names both ids, because the interesting
        // half of the log line is WHICH session tried.
        let who = Origin {
            session: Some(7),
            ..Origin::default()
        };
        let refusal = refuse_input(&who, 4).expect("a session must be refused");
        let (kind, message) = refusal.as_error().expect("an error");
        assert_eq!(kind, ErrorKind::PermissionDenied);
        assert!(message.contains("session 7"), "{message}");
        assert!(message.contains("session 4"), "{message}");
    }

    #[test]
    fn a_session_may_not_write_into_itself_either() {
        // Not a special case worth allowing. An agent typing into its own
        // terminal is an agent writing its own next prompt, which is the same
        // borrowed-authority problem with one hop taken out, and there is no
        // caller that wants it: an agent already owns its stdout.
        let who = Origin {
            session: Some(4),
            ..Origin::default()
        };
        assert!(refuse_input(&who, 4).is_some());
    }

    #[test]
    fn a_connection_that_could_not_be_classified_is_refused_and_not_assumed_human() {
        // The important one. `session: None` on an unreadable origin means
        // "the walk never happened", not "the walk happened and found
        // nothing" — the two are indistinguishable in that field, which is why
        // the reason field is consulted instead of the absence. Reading a
        // failed classification as a human is the defect this repository has
        // already shipped once, in the other direction.
        let who = Origin::unreadable("the kernel would not report the peer credentials");
        assert!(who.session.is_none(), "the fixture must exercise the gap");
        let refusal = refuse_input(&who, 4).expect("an unclassified caller must be refused");
        let (kind, message) = refusal.as_error().expect("an error");
        assert_eq!(kind, ErrorKind::PermissionDenied);
        assert!(
            message.contains("could not be classified"),
            "the refusal must say why it could not decide: {message}"
        );
    }

    #[test]
    fn a_request_a_session_grant_covered_is_not_recorded_as_a_standing_grant() {
        // §4.4's grant is a window, not a policy. If a request it covered were
        // filed as `allow_for_project`, the audit would name an authority no
        // human created: a standing permission for that project, readable by
        // anyone who lists grants later, backed by nothing.
        let (decision, attributed, event) = decided_by(false, Some(9));
        assert_eq!(decision, Some(Decision::AllowOnce));
        assert_eq!(attributed, Some(9), "the trail must name the grant");
        assert_eq!(event, "requested-and-covered");

        // A standing grant is the other authority, and says so.
        let (decision, attributed, event) = decided_by(true, None);
        assert_eq!(decision, Some(Decision::AllowForProject));
        assert_eq!(attributed, None);
        assert_eq!(event, "requested-and-granted");

        // Both: the human's standing decision is the reason it was allowed,
        // and it would have been allowed with no window open, so the record
        // does not hang it on a window that will be gone in an hour.
        let (decision, attributed, event) = decided_by(true, Some(9));
        assert_eq!(decision, Some(Decision::AllowForProject));
        assert_eq!(attributed, None, "a grant that decided nothing is not cited");
        assert_eq!(event, "requested-and-granted");

        // Neither: a human still has to look at it.
        assert_eq!(decided_by(false, None), (None, None, "requested"));
    }

    #[test]
    fn a_connection_inside_a_session_is_refused_whatever_its_origin_says() {
        // P0-007's fourth criterion as the exhaustive claim it has to be. The
        // session check comes first, so it holds over every origin — including
        // `local-terminal`, which is what a session's own connection would
        // classify as if it were classified afresh rather than inherited.
        for origin in RequestOrigin::ALL {
            let who = Origin {
                session: Some(4),
                ..unsessioned(*origin)
            };
            let err = may_be_granted(&who, "ask for a grant").expect_err("must refuse");
            assert_eq!(
                err,
                GrantError::FromInsideASession {
                    session: 4,
                    what: "ask for a grant"
                },
                "{origin}"
            );
        }
    }

    #[test]
    fn only_a_local_origin_outside_a_session_may_be_granted_anything() {
        // §7's table, at the gate. The two local origins pass; the five that
        // mean "nobody is at this keyboard" do not, and that includes the two
        // that run on this machine — a scheduled job and a subagent.
        for origin in RequestOrigin::ALL {
            let who = unsessioned(*origin);
            match may_be_granted(&who, "ask for a grant") {
                Ok(got) => {
                    assert!(origin.is_local(), "{origin} was granted the local column");
                    assert_eq!(got, *origin);
                }
                Err(GrantError::NotLocal { origin: got, .. }) => {
                    assert!(!origin.is_local(), "{origin} was refused the local column");
                    assert_eq!(got, *origin);
                }
                Err(other) => panic!("{origin}: {other}"),
            }
        }
    }

    #[test]
    fn an_origin_that_could_not_be_read_is_refused_rather_than_defaulted() {
        // `RequestOrigin::default()` is `local-terminal`, which is exactly the
        // column being asked for, so a fallback here would hand a grant to a
        // connection nobody identified.
        let who = Origin {
            origin_unreadable: Some("/proc/9/cgroup: no such file or directory".into()),
            ..Origin::default()
        };
        let err = may_be_granted(&who, "ask for a grant").expect_err("must refuse");
        assert!(matches!(err, GrantError::OriginUnknown(_)), "{err}");
        assert!(err.to_string().contains("/proc/9/cgroup"), "{err}");

        // And with nothing recorded at all, which is the shape a future caller
        // could arrive in.
        assert!(may_be_granted(&Origin::default(), "ask for a grant").is_err());
    }

    #[test]
    fn the_session_check_beats_the_origin_check_so_the_refusal_names_the_session() {
        // Order matters for what the user is told. A session whose origin is
        // also non-local must hear "a session cannot do this", which is
        // actionable, rather than "a subagent cannot do this", which invites
        // the wrong fix.
        let who = Origin {
            session: Some(9),
            ..unsessioned(RequestOrigin::Subagent)
        };
        let err = may_be_granted(&who, "renew a system-access grant").expect_err("refuse");
        assert!(err.to_string().contains("session 9"), "{err}");
        assert!(err.to_string().contains("renew a system-access grant"), "{err}");
    }
}
