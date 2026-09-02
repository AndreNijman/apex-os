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

use apex_agent_core::paths;
use apex_agent_core::protocol::{ErrorKind, Response};
use apex_agent_core::request::{
    self, Decision, Grants, PrivilegeRequest, RequestError, Verb,
};

use crate::peer::{self, Peer};
use crate::Daemon;

/// Which session a connection belongs to, and what that session was doing.
///
/// `None` for a connection from an ordinary terminal — the user's own shell
/// running `apex request approve`, or APEX Shell. That is not an error: an
/// unsessioned peer is the human, and the human is who decides.
#[derive(Debug, Clone, Default)]
pub struct Origin {
    pub session: Option<u32>,
    pub agent: Option<String>,
    pub project: Option<String>,
}

/// Resolve a connection to the session that owns it.
///
/// The pid comes from `SO_PEERCRED` and is walked up its `/proc` ancestry until
/// it meets a pid the daemon recorded when it forked a session. Nothing the
/// client sent is consulted.
pub fn origin(daemon: &Arc<Daemon>, peer: Option<Peer>) -> Origin {
    let Some(peer) = peer else {
        return Origin::default();
    };
    if !peer::is_own_user(&peer) {
        // Should be unreachable: the socket sits in a 0700 directory inside
        // $XDG_RUNTIME_DIR. Treated as unsessioned rather than trusted.
        return Origin::default();
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
    if live.is_empty() {
        return Origin::default();
    }

    let matched = peer::resolve_by_ancestry(peer.pid, |p| live.iter().any(|(pid, _)| *pid == p));
    let Some(found) = matched else {
        return Origin::default();
    };
    let Some((_, id)) = live.iter().find(|(pid, _)| *pid == found) else {
        return Origin::default();
    };

    // Re-take the lock for the details, so the snapshot above stays short.
    let reg = daemon.registry.lock().expect("registry lock");
    match reg.get(*id).and_then(|h| {
        h.lock()
            .ok()
            .map(|s| (s.info.agent.clone(), s.info.project.clone()))
    }) {
        Some((agent, project)) => Origin {
            session: Some(*id),
            agent: Some(agent),
            project,
        },
        None => Origin {
            session: Some(*id),
            agent: None,
            project: None,
        },
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
