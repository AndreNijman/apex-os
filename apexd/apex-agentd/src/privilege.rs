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

use apex_agent_core::origin::{OriginSource, SessionOrigin};
use apex_agent_core::paths;
use apex_agent_core::policy::AgentPolicy;
use apex_agent_core::protocol::{ErrorKind, Response};
use apex_agent_core::request::{
    self, Decision, Grants, PrivilegeRequest, RequestError, Verb,
};

use crate::peer::{self, Peer};
use crate::Daemon;

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
pub fn origin(daemon: &Arc<Daemon>, peer: Option<Peer>) -> Origin {
    let Some(peer) = peer else {
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
        return match observed {
            Ok(o) => Origin {
                request_origin: Some(SessionOrigin::observed(o)),
                ..Origin::default()
            },
            Err(why) => Origin::unreadable(why),
        };
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
        // answer.
        return match observed {
            Ok(o) => Origin {
                request_origin: Some(SessionOrigin::observed(o)),
                ..Origin::default()
            },
            Err(why) => Origin::unreadable(why),
        };
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
            origin_unreadable: Some(format!(
                "session {found} disappeared while this request was being attributed"
            )),
        },
    }
}

/// Record a session's own, narrower, origin (§7).
///
/// Only a session may call this, and only about itself — the session comes
/// from the peer credentials, so there is no id to get wrong or to forge. The
/// declaration is checked by [`apex_agent_core::origin::may_declare`], which
/// accepts it only when it gives something up.
///
/// This is the route Remote Control takes. It is switched on after `claude`
/// has started, so nothing observable about the connection ever changes and
/// an observation alone could never reach §7's second column.
pub fn declare(daemon: &Arc<Daemon>, peer: Option<Peer>, wanted: &str) -> Response {
    use apex_agent_core::policy::RequestOrigin;

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

    let who = origin(daemon, peer);
    let Some(session) = who.session else {
        return Response::error(
            ErrorKind::PermissionDenied,
            "only a managed session can narrow its own origin; this connection is not one"
                .to_string(),
        );
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

/// File a request.
///
/// The verb is parsed here rather than accepted pre-parsed, so a client cannot
/// hand over a `Verb` that never went through validation. On a project where a
/// human has already allowed exactly this, the request is recorded as decided
/// and needs no prompt — see [`Decision::AllowForProject`] for what that does
/// and does not buy.
pub fn file(daemon: &Arc<Daemon>, peer: Option<Peer>, verb: &str, args: &[String], reason: &str) -> Response {
    let parsed = match Verb::parse(verb, args) {
        Ok(v) => v,
        Err(e) => return refusal(e),
    };
    let reason = match request::check_reason(reason) {
        Ok(r) => r,
        Err(e) => return refusal(e),
    };

    let who = origin(daemon, peer);

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
    let pre_approved = grants.allows(who.project.as_deref(), &parsed);

    let now = request::now_ms();
    let req = PrivilegeRequest {
        id: request::next_id(&dir),
        verb: parsed,
        reason,
        session: who.session,
        agent: who.agent,
        project: who.project,
        request_origin: Some(source.origin),
        origin_source: Some(source.source),
        decision: if pre_approved {
            Decision::AllowForProject
        } else {
            Decision::Pending
        },
        created_ms: now,
        decided_ms: if pre_approved { Some(now) } else { None },
        executed_ms: None,
        exit_code: None,
    };

    if let Err(e) = request::save(&dir, &req) {
        return Response::error(ErrorKind::Internal, format!("recording the request: {e}"));
    }
    let _ = request::audit(
        &request::audit_log(),
        if pre_approved {
            "requested-and-granted"
        } else {
            "requested"
        },
        &req,
    );
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
pub fn decide(daemon: &Arc<Daemon>, peer: Option<Peer>, id: u32, decision: Decision) -> Response {
    // A session may not decide its own requests, or the approval step is
    // decoration. Only an unsessioned peer — the human's shell, or the shell
    // itself — may decide.
    let who = origin(daemon, peer);
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
pub fn revoke(daemon: &Arc<Daemon>, peer: Option<Peer>, project: &str, key: Option<&str>) -> Response {
    // Revoking is a policy change, so it is subject to the same rule as
    // deciding: a session cannot widen or narrow its own permissions.
    if let Some(session) = origin(daemon, peer).session {
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
