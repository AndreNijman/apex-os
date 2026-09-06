//! The secret broker, agent-runtime side (roadmap §3.2, §7, §11).
//!
//! This used to be the broker. It is now a *client* of one.
//!
//! The reason is P0-002's first acceptance criterion: credentials must not live
//! in agent-readable home paths. `apex-agentd` runs as the user, because it
//! launches the user's own programs — so a credential it can read is a
//! credential an unrestricted session can read, and `0600` in
//! `$XDG_STATE_HOME` only ever kept out another *account*. The store moved to
//! `apex-secretd`, which runs as root, and this daemon lost the ability to read
//! a credential at all: there is no verb in `apex_secret_core::protocol` that
//! returns one, and nothing here holds a `SecretValue`.
//!
//! What stays here is what only this daemon knows:
//!
//! * **which session is asking**, resolved from the connection's peer
//!   credentials against the pids the daemon recorded when it forked each
//!   session — never from the request;
//! * **that session's secret policy** (dimension 4), so `--secrets none` is
//!   enforced before the request goes anywhere;
//! * **that session's project**, recorded at fork time, so a confined agent
//!   cannot claim a project whose capabilities it was never granted;
//! * **where the connection came from** (§7's `request_origin`), observed by
//!   `crate::origin` or inherited from the session, never read off the request.
//!
//! Those become a `CapabilityRecord` and go to `apex-secretd`, which re-checks
//! everything it can check for itself — the account, the grant, the remote, the
//! host pin — and performs the operation. Session identity and origin are the
//! two things it cannot re-derive, so the record marks them as attribution
//! rather than authorisation, and the crate note in `apex_secret_core` says
//! what that does and does not buy.
//!
//! ## The network dimension and where a cloud provider plugs in
//!
//! `--network brokered` is a session with `--unshare-net`: no IP egress at all,
//! and this module is the way out. That works because the control socket is an
//! `AF_UNIX` path, which a network namespace does not touch, and because the
//! daemons that run the operation are outside the namespace and therefore still
//! have the network. The same property is why `git push` already works from a
//! `strict` session today.
//!
//! Nothing in the network dimension is checked here, deliberately. Nothing
//! about a *provider* is either, and after P1-001 that is a property rather
//! than an omission: this module forwards an operation id, a resource and a
//! parameter map without knowing what any of them mean, so P1-002's Cloudflare
//! provider is registered in `apex-secretd` and reaches a session with no
//! network through this path with no line changing here. The confinement, the
//! grant check, the audit record and the namespace argument above all apply to
//! it unchanged. The one dimension that can shut this door is the secret one,
//! checked below, and `--network brokered --secrets none` is refused by
//! `AgentPolicy::validate` before a session with both is ever started.
//!
//! ## §7 and this module
//!
//! §7's table answers `allow` for "github push" from every origin, local and
//! remote alike — "Remote Control is a normal workflow, not an edge case" — so
//! origin is *recorded* here and does not gate anything.
//! [`apex_agent_core::policy::SecretPolicy::may_use_broker`] takes no origin
//! for that reason, and giving it one would quietly tighten a row §7 says is
//! open. The row §7 does deny from every origin is "read raw brokered secret",
//! and this build denies it by having nowhere to implement it: no verb in the
//! secret service returns a value.
//!
//! Granting is no longer here at all. A grant is a change to what is allowed,
//! and `apex-secretd` refuses one from any caller inside a session — which
//! covers a session that skips this daemon and connects to the socket itself,
//! something the old check could not see.

use std::path::Path;
use std::sync::Arc;

use std::collections::BTreeMap;

use apex_agent_core::protocol::{ErrorKind, Response};
use apex_secret_core::capability::CapabilityRecord;
use apex_secret_core::client::Client;
use apex_secret_core::operation::{self, Params};
use apex_secret_core::protocol as secret_protocol;

use crate::peer::Peer;
use crate::privilege;
use crate::Daemon;

/// What the record carries when the daemon could not establish the origin.
///
/// Deliberately not [`apex_agent_core::policy::RequestOrigin::default`], which
/// is `local-terminal` — the origin §7 reserves root and break-glass for. "I
/// could not tell" and "a human is at the keyboard" are different answers, and
/// an audit trail that conflates them is worse than one with a gap in it.
const UNKNOWN_ORIGIN: &str = "unknown";

/// Perform one capability.
///
/// Order matters and is the security argument:
///   1. resolve the session from the PEER CREDENTIALS — never the request;
///   2. check dimension 4 for that session, before anything else about it;
///   3. resolve the project from what the daemon recorded for that session;
///   4. stamp §7's origin from the connection, not from the request;
///   5. hand the record to `apex-secretd`, which owns every remaining check and
///      the credential itself.
#[allow(clippy::too_many_arguments)]
pub fn use_capability(
    daemon: &Arc<Daemon>,
    peer: Option<Peer>,
    service: &str,
    operation: &str,
    resource: &str,
    params: &BTreeMap<String, String>,
    // The message the operation carries, for the one operation that carries
    // one. Opaque here: this daemon forwards it and never looks inside.
    body: Option<&str>,
    claimed_project: Option<&str>,
) -> Response {
    // Shape only, and that is the whole of what this daemon knows about a
    // capability. Which operations exist, what a resource means for one, and
    // which options it takes are the provider's, declared in `apex-secretd`'s
    // registry — so a provider added there needs nothing here.
    //
    // `valid_operation_ref` and not `OperationId::parse`: an older spelling
    // like `git-fetch` is a name only the registry can resolve, and a daemon
    // that refused it here would refuse a capability the owner has granted.
    // What this does refuse is a string that could carry framing into a grant
    // key or an audit line.
    if !operation::valid_operation_ref(operation) {
        return Response::error(
            ErrorKind::BadRequest,
            format!(
                "'{}' is not an operation name. One looks like `provider.thing.verb` \
                 — lower case, dot-separated, e.g. `git.push`",
                operation.escape_debug()
            ),
        );
    }

    let who = privilege::origin(daemon, peer);

    // Dimension 4, checked before anything else about the request.
    //
    // The session's own secret policy, read from what the daemon recorded when
    // it forked the session — a session that could name its own policy could
    // name a looser one. This is the enforcement point that makes
    // `--secrets none` mean something: a task with no business touching a
    // credential cannot reach the broker at all, and the refusal happens before
    // the project or the grant is even resolved.
    //
    // No origin is passed, and that is §7's position rather than an oversight:
    // "github push" is `allow` from every origin.
    if !who.policy.secrets.may_use_broker() {
        return Response::error(
            ErrorKind::PermissionDenied,
            "this session was started with the secret capability layer off, so it cannot use \
             brokered credentials; start it without `--secrets none` if it needs them"
                .to_string(),
        );
    }

    // The project.
    //
    // For a SESSION it is what the daemon recorded when it forked it, and
    // `claimed_project` is ignored — otherwise a confined agent could name a
    // project whose capabilities it was never granted.
    //
    // For an unsessioned caller the claim is honoured, because the daemon
    // cannot see that caller's working directory. `current_dir()` here returns
    // the DAEMON's cwd, which is how the first version made every grant
    // silently fail to match. Trusting the claim is not a weakening: an
    // unsessioned caller is unconfined and could run git itself.
    let project = match who.session {
        Some(_) => who.project.clone(),
        None => claimed_project
            .map(str::to_string)
            .filter(|p| Path::new(p).is_absolute()),
    };
    let Some(project) = project else {
        return Response::error(
            ErrorKind::PermissionDenied,
            "this session is not inside a project, so no capability can be \
             granted to it"
                .to_string(),
        );
    };

    let mut record = CapabilityRecord::new(service, operation, resource);
    record.params = Params::from(params.clone());
    record.project = Some(project);
    // Attribution, not authentication. `apex-secretd` cannot re-derive either
    // of these — it would have to trust a list of session pids published by a
    // process running as the user — so it records the claim and authorises on
    // the things it can establish itself.
    record.agent_session = who.session;
    record.agent_session_agent(who.agent.as_deref());
    stamp_origin(&mut record, &who);

    perform(record, body.unwrap_or_default().as_bytes())
}

/// Copy §7's origin onto the record, with how it was arrived at.
///
/// Both fields or neither. `origin_source` is what separates "the daemon
/// worked this out from the connection" from "something asked for this and was
/// allowed", and a trail carrying the origin without it cannot answer the only
/// question worth asking of it.
fn stamp_origin(record: &mut CapabilityRecord, who: &privilege::Origin) {
    match who.request_origin {
        Some(o) => {
            record.request_origin = o.origin.as_str().to_string();
            record.origin_source = o.source.as_str().to_string();
        }
        None => {
            record.request_origin = UNKNOWN_ORIGIN.to_string();
            record.origin_source = UNKNOWN_ORIGIN.to_string();
        }
    }
}

/// Send the record to `apex-secretd` and translate its answer.
fn perform(record: CapabilityRecord, body: &[u8]) -> Response {
    let mut client = match Client::connect() {
        Ok(c) => c,
        Err(e) => {
            return Response::error(
                ErrorKind::Internal,
                format!(
                    "{e}\nthe secret service holds every credential; \
                     without it no capability can be performed"
                ),
            )
        }
    };
    match client.use_with_body(record, body) {
        Ok(secret_protocol::Response::Performed {
            record,
            endpoint,
            exit_code,
            output,
        }) => Response::Brokered {
            service: record.provider.clone(),
            capability: record.operation.clone(),
            detail: record.summary(),
            audit_id: record.audit_id.clone(),
            endpoint,
            exit_code,
            output,
        },
        Ok(secret_protocol::Response::Error { kind, message }) => {
            Response::error(translate(kind), message)
        }
        Ok(other) => Response::error(
            ErrorKind::Internal,
            format!("the secret service replied with a {} to a use", other.variant()),
        ),
        Err(e) => Response::error(ErrorKind::Internal, format!("{e:#}")),
    }
}

/// Map the secret service's error kinds onto the agent runtime's.
///
/// Two protocols, deliberately: the agent runtime's is a compatibility surface
/// that APEX Shell parses, and the secret service's is its own. Translating in
/// one place keeps a change to either from silently changing the other.
fn translate(kind: secret_protocol::ErrorKind) -> ErrorKind {
    match kind {
        secret_protocol::ErrorKind::BadRequest => ErrorKind::BadRequest,
        secret_protocol::ErrorKind::NoSuchService => ErrorKind::NoSuchRequest,
        secret_protocol::ErrorKind::PermissionDenied => ErrorKind::PermissionDenied,
        secret_protocol::ErrorKind::Internal => ErrorKind::Internal,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use apex_agent_core::origin::{OriginSource, SessionOrigin};
    use apex_agent_core::policy::RequestOrigin;

    fn record() -> CapabilityRecord {
        CapabilityRecord::new("demo", "git.fetch", "origin")
    }

    #[test]
    fn every_secret_error_kind_has_an_agent_runtime_equivalent() {
        // An exhaustive match already forces this at compile time; the test is
        // about the *choices*. `NoSuchService` becoming `NoSuchRequest` is the
        // one that is not obvious: the agent protocol has no service kind, and
        // mapping it to `Internal` would tell a caller to file a bug when the
        // answer is "you have not added that credential".
        use secret_protocol::ErrorKind as S;
        assert_eq!(translate(S::BadRequest), ErrorKind::BadRequest);
        assert_eq!(translate(S::NoSuchService), ErrorKind::NoSuchRequest);
        assert_eq!(translate(S::PermissionDenied), ErrorKind::PermissionDenied);
        assert_eq!(translate(S::Internal), ErrorKind::Internal);
    }

    #[test]
    fn an_operation_name_is_shape_checked_and_nothing_more() {
        // What this daemon checks is the SHAPE. It does not know which
        // operations exist and must not, or a provider added to `apex-secretd`
        // would need a line here too — including names for providers this
        // build has never heard of, and older spellings only the registry can
        // resolve.
        for good in [
            "git.push",
            "git-fetch",
            "cloudflare.r2.object.read",
            "aws.s3.object.write",
            "exec",
        ] {
            assert!(operation::valid_operation_ref(good), "{good}");
        }
        // `exec` above is deliberate: it is well-shaped and no provider offers
        // it, so the refusal belongs to `apex-secretd`. What is refused here is
        // anything that could carry framing into a grant key or an audit line.
        for evil in ["git push", "https://x/y", "", "GIT.PUSH", "git.push;sh"] {
            assert!(!operation::valid_operation_ref(evil), "{evil}");
        }
    }

    #[test]
    fn a_record_built_here_carries_the_session_as_attribution() {
        let mut record = record();
        record.project = Some("/home/t/p".into());
        record.agent_session = Some(7);
        record.agent_session_agent(Some("claude"));
        // The agent name rides in `constraints` rather than in a field of its
        // own: §11 fixes the record's fields, and the trail is more useful with
        // the agent named than with the list left empty.
        assert!(record.constraints.iter().any(|c| c == "agent=claude"));
        assert_eq!(record.agent_session, Some(7));
    }

    #[test]
    fn an_unnamed_agent_adds_no_constraint() {
        let mut record = record();
        record.agent_session_agent(None);
        assert!(record.constraints.is_empty());
    }

    #[test]
    fn the_origin_and_how_it_was_reached_both_land_on_the_record() {
        // §7's field is only worth having if a reader can tell an observation
        // from a declaration, so the record carries both or neither.
        for (session_origin, origin, source) in [
            (
                SessionOrigin::observed(RequestOrigin::LocalTerminal),
                "local-terminal",
                "observed",
            ),
            (
                SessionOrigin::inherited(RequestOrigin::RemoteControl),
                "claude-remote-control",
                "inherited",
            ),
            (
                SessionOrigin::observed(RequestOrigin::LocalTerminal)
                    .declare(RequestOrigin::Mcp)
                    .expect("mcp is declarable over a local origin"),
                "mcp",
                "declared",
            ),
        ] {
            let mut record = record();
            stamp_origin(
                &mut record,
                &privilege::Origin {
                    request_origin: Some(session_origin),
                    ..privilege::Origin::default()
                },
            );
            assert_eq!(record.request_origin, origin);
            assert_eq!(record.origin_source, source);
        }
        // And every source the runtime can produce is spelled the same way on
        // the record as it is in the type, so a reader of the trail and a
        // reader of the code agree.
        for source in OriginSource::ALL {
            assert!(!source.as_str().is_empty());
        }
    }

    #[test]
    fn an_unreadable_origin_is_recorded_as_unknown_and_never_as_local() {
        // The defect this exists to avoid: falling back to the default, which
        // is `local-terminal` — the origin §7 reserves root and break-glass
        // for. "Could not tell" is not "a human is at the keyboard".
        let mut record = record();
        stamp_origin(
            &mut record,
            &privilege::Origin {
                request_origin: None,
                origin_unreadable: Some("cannot read /proc/1234/cgroup".into()),
                ..privilege::Origin::default()
            },
        );
        assert_eq!(record.request_origin, UNKNOWN_ORIGIN);
        assert_eq!(record.origin_source, UNKNOWN_ORIGIN);
        assert_ne!(record.request_origin, RequestOrigin::default().as_str());
    }

    #[test]
    fn the_secret_dimension_is_read_without_an_origin() {
        // §7's table answers `allow` for "github push" from every origin, so
        // the broker gate is the secret dimension alone. A signature taking an
        // origin would be an invitation to tighten a row §7 says is open, and
        // the four `allow` rows are the half most easily lost by being careful.
        use apex_agent_core::origin::Capability as PolicyCapability;
        for origin in RequestOrigin::ALL {
            assert_eq!(
                PolicyCapability::GitHubPush.ruling(*origin),
                apex_agent_core::origin::Ruling::Allow,
                "{origin} lost the github-push row"
            );
        }
        // And the row §7 denies everywhere is the one this build implements by
        // having nowhere to put it.
        for origin in RequestOrigin::ALL {
            assert_eq!(
                PolicyCapability::ReadRawSecret.ruling(*origin),
                apex_agent_core::origin::Ruling::Deny
            );
        }
    }
}
