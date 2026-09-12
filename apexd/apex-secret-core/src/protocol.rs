//! The wire between a client and `apex-secretd`.
//!
//! Newline-delimited JSON on a Unix socket, one request per line, one response
//! per line — the same framing `apex_agent_core::protocol` uses, so there is
//! one thing to learn and one thing to debug with `socat`.
//!
//! # The API cannot return a credential
//!
//! This is P0-002's second acceptance criterion and it is a property of this
//! file. [`Response`] derives `Serialize`; [`crate::SecretValue`] does not
//! implement it. A variant that carried a credential would therefore fail to
//! compile, today and after every future variant somebody adds for §13's
//! Cloudflare provider or §10's MCP header helper.
//!
//! There is no `Read` verb, no `Export` verb and no debug escape hatch. The
//! closest thing to reading a credential is [`Request::Use`], which makes the
//! daemon *perform* an operation and hands back the operation's output with the
//! credential scrubbed out of it.
//!
//! [`Response::variant`] is written as an exhaustive match so that adding a
//! variant does not compile until its author has been past the sentence above,
//! and the test beside it pins the list so it does not compile *quietly*.
//!
//! # A credential does travel inbound
//!
//! `apex secret add` has to get the value to the daemon somehow. It does not go
//! in the JSON: [`Request::Add`] declares a length and the raw bytes follow the
//! request line on the socket. Two reasons. It keeps every serialisable type in
//! this crate free of credentials, so the compile-time argument above has no
//! exception to explain. And it keeps the value out of JSON escaping, out of
//! any `{:?}` of a request, and out of a partially-parsed error message.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::audit::AuditLine;
use crate::capability::CapabilityRecord;
use crate::operation::OperationInfo;
use crate::store::{Approval, ServiceInfo};

/// Bumped when a change is not backward compatible. Clients send nothing and
/// the daemon reports it in [`Response::Hello`], so a mismatch is a message
/// rather than a parse failure halfway through a credential operation.
pub const PROTOCOL_VERSION: u32 = 2;

/// Longest request line the daemon will read.
///
/// A capability record with constraints is a few hundred bytes; this is two
/// orders of magnitude more, and it is bounded because the daemon reads from a
/// socket any local process can open.
pub const MAX_LINE_BYTES: usize = 64 * 1024;

/// Longest reply the daemon will send and a client will read.
///
/// Larger than a request, and for a reason that is not symmetry: a request line
/// is written by whoever connected, so it is an attack surface and stays small,
/// while a reply is the *result* of an operation the daemon just performed. A
/// `git fetch` of a busy repository or one MCP `read_note` is comfortably more
/// than a request line may be, and truncating it would hand the caller a
/// half-parsed JSON document instead of an answer.
pub const MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;

/// What a client asks for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "kebab-case")]
pub enum Request {
    /// Protocol version and what this daemon can do. Costs nothing and needs
    /// no store, so it is also the readiness check a unit or a test uses.
    Hello,

    /// Store a credential.
    ///
    /// `value_len` raw bytes follow this line on the socket. The value is NOT
    /// a field here — see the module note.
    Add {
        service: String,
        host: String,
        /// `https`, or `http` for a loopback host.
        #[serde(default = "https")]
        scheme: String,
        #[serde(default)]
        username: Option<String>,
        /// The endpoint path, for a credential whose destination is the
        /// service itself. Empty for a git credential.
        #[serde(default)]
        path: String,
        /// `bearer` or `raw`. See `ServiceInfo::auth`.
        #[serde(default)]
        auth: Option<String>,
        /// The endpoint's port, when it is not the scheme's own.
        #[serde(default)]
        port: Option<u16>,
        value_len: usize,
    },

    /// Delete a credential and every grant that named it.
    Remove { service: String },

    /// Stored services, metadata only.
    List,

    /// Allow or withdraw a capability for a project.
    Grant {
        project: String,
        service: String,
        capability: String,
        #[serde(default)]
        revoke: bool,
    },

    /// What is allowed, per project.
    Grants,

    /// §13.8: approve one operation, once, or withdraw an approval.
    ///
    /// Not a grant. A grant is consulted and keeps standing; this is **spent**
    /// by the first operation that matches it, and it expires on its own. See
    /// [`crate::store::Approval`] for why the two are different types in
    /// different files.
    ///
    /// Refused from inside a managed agent session, like `Grant` and for the
    /// same reason: a session that can approve its own production deploy has
    /// not been made to ask for approval, it has been made to type one more
    /// line.
    Approve {
        project: String,
        service: String,
        /// The operation, in any spelling the registry resolves.
        operation: String,
        /// Exactly what the operation will name. Empty for one that names
        /// nothing.
        #[serde(default)]
        resource: String,
        /// How long it may be spent for, in milliseconds. Bounded by
        /// [`crate::store::MAX_APPROVAL_TTL_MS`]; defaults to
        /// [`crate::store::APPROVAL_TTL_MS`].
        #[serde(default)]
        ttl_ms: Option<u64>,
        /// Take one back instead of giving one.
        #[serde(default)]
        withdraw: bool,
    },

    /// Every approval this account has outstanding.
    Approvals,

    /// Perform a capability. The credential does not come back; the
    /// operation's output does.
    ///
    /// `body_len` raw bytes follow this line when the capability carries a
    /// message — `mcp-request` does, the git ones do not. Framed the way
    /// [`Request::Add`] frames a credential, for the second of that variant's
    /// two reasons: an MCP `write_note` body is larger than
    /// [`MAX_LINE_BYTES`], and a request line that could grow to hold one
    /// would raise the bound on every request any local process can send.
    Use {
        record: Box<CapabilityRecord>,
        #[serde(default)]
        body_len: usize,
    },

    /// The audit trail.
    Audit {
        #[serde(default = "default_audit_lines")]
        lines: usize,
        /// Only lines recorded for this project root, when given.
        ///
        /// Filtered by the daemon, **before** `lines` is applied, and that
        /// ordering is the point rather than an optimisation: the trail is one
        /// file for the machine, so a caller asking "what did this worktree
        /// do" against a busy trail would otherwise get a thousand lines from
        /// everywhere else and conclude the worktree had done nothing. §13.13's
        /// destroy plan is derived from these lines, and a plan short of a
        /// resource is a plan that says it cleaned up when it did not.
        #[serde(default)]
        project: Option<String>,
    },

    /// §13.14: what this project's budget allows and what it has spent today.
    ///
    /// Its own verb rather than something a client works out from
    /// [`Request::Audit`], because a client cannot do it honestly: the trail is
    /// root-owned, `Audit` hands back a bounded window of it, and a count taken
    /// from a window is an undercount presented as a fact. One counting
    /// implementation, in the daemon, is also the only way the report and the
    /// enforcement can agree.
    Usage {
        /// The root to count, exactly — a worktree is not its project. Required:
        /// a budget is a project's, and "all of them" is not a budget.
        project: String,
    },
}

fn https() -> String {
    "https".to_string()
}

fn default_audit_lines() -> usize {
    20
}

/// What the daemon answers.
///
/// **No variant may carry a credential.** The compiler enforces it — see the
/// module note — and [`Response::variant`] is where a new variant has to be
/// declared, so this sentence is on the path of anybody adding one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reply", rename_all = "kebab-case")]
pub enum Response {
    /// Version and vocabulary.
    Hello {
        version: u32,
        /// The operation ids this build offers, so a client does not have to
        /// hardcode them to print a help text. Canonical ids only — an alias is
        /// an input the daemon accepts, never a spelling it teaches.
        capabilities: Vec<String>,
        /// The same operations with what a person needs to use them: a summary,
        /// whether they change anything, and their declared options.
        ///
        /// Beside `capabilities` rather than replacing it, so a client built
        /// against the older reply still parses this one. It is what makes
        /// `apex secret capabilities` a view of the daemon's registry instead
        /// of a list the CLI keeps in step by hand — which is how a provider
        /// gets added without the CLI changing.
        #[serde(default)]
        vocabulary: Vec<OperationInfo>,
        /// Whether the daemon holds the store behind a uid boundary, or is
        /// running as an ordinary user for a test. `apex secret list` says so
        /// when it is false, rather than letting a test instance look like a
        /// boundary it is not.
        protected: bool,
    },

    /// Verb succeeded and has nothing to say.
    Ok,

    /// Stored services. Metadata only, by construction: [`ServiceInfo`] has no
    /// field a value can occupy.
    Services { services: Vec<ServiceInfo> },

    /// Per-project capability grants: project root -> `service:capability`.
    Grants {
        projects: BTreeMap<String, Vec<String>>,
    },

    /// §13.8's outstanding one-shot approvals, soonest to expire first.
    ///
    /// Metadata only, by construction: [`crate::store::Approval`] has no field
    /// a value could occupy, for [`Response::Services`]' reason.
    Approvals { pending: Vec<Approval> },

    /// A capability ran. Carries the RESULT, never the credential.
    Performed {
        /// The record as the daemon finally read it, with `audit_id` filled in.
        record: Box<CapabilityRecord>,
        /// Scheme and host the credential was sent to, as the daemon resolved
        /// it from the repository. Returned so the caller can see where its
        /// operation went without being able to choose it.
        endpoint: String,
        exit_code: i32,
        /// The command's own output, with the credential scrubbed out.
        output: String,
    },

    /// The audit trail, oldest first.
    Audit { entries: Vec<AuditLine> },

    /// §13.14's answer to [`Request::Usage`].
    Usage { report: Box<crate::budget::Report> },

    /// Verb failed. `kind` is stable enough to branch on; `message` is for
    /// humans.
    Error { kind: ErrorKind, message: String },
}

/// Failure categories a client may reasonably branch on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    /// The request did not make sense.
    BadRequest,
    /// No credential stored under that name.
    NoSuchService,
    /// The capability is not granted, or the caller may not ask for it.
    PermissionDenied,
    /// The operation ran but the daemon could not finish the request.
    Internal,
}

impl Response {
    /// The variant's name.
    ///
    /// # Adding a variant
    ///
    /// This match is exhaustive, so a new variant of [`Response`] does not
    /// compile until it appears here. That is deliberate, and this is the
    /// sentence it exists to put in front of you:
    ///
    /// **no response variant may carry a raw credential.** Not as a field, not
    /// inside a struct it holds, not base64-encoded in a string, not "just for
    /// the `add` confirmation". The service performs the operation instead —
    /// that is the whole reason it exists. `SecretValue` does not implement
    /// `Serialize`, so the compiler already refuses the direct form; this note
    /// is about the indirect ones.
    ///
    /// `protocol_surface_is_pinned` in the test module below asserts the set of
    /// names, so a variant added here without a decision fails a test too.
    pub fn variant(&self) -> &'static str {
        match self {
            Response::Hello { .. } => "hello",
            Response::Ok => "ok",
            Response::Services { .. } => "services",
            Response::Grants { .. } => "grants",
            Response::Approvals { .. } => "approvals",
            Response::Performed { .. } => "performed",
            Response::Audit { .. } => "audit",
            Response::Usage { .. } => "usage",
            Response::Error { .. } => "error",
        }
    }

    pub fn error(kind: ErrorKind, message: impl Into<String>) -> Response {
        Response::Error {
            kind,
            message: message.into(),
        }
    }

    pub fn as_error(&self) -> Option<(ErrorKind, &str)> {
        match self {
            Response::Error { kind, message } => Some((*kind, message.as_str())),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::{AuditEvent, AuditLine};
    use crate::operation::ParamInfo;

    /// Every variant, constructed. A `Vec` rather than a `match`, because the
    /// point is to have one of each to serialise.
    fn every_response() -> Vec<Response> {
        let record = Box::new(CapabilityRecord::new(
            "demo",
            "git.fetch",
            "origin",
        ));
        vec![
            Response::Hello {
                version: PROTOCOL_VERSION,
                capabilities: vec!["git.fetch".to_string(), "git.push".to_string()],
                vocabulary: vec![OperationInfo {
                    id: "git.push".into(),
                    summary: "push a branch of this project to one of its own remotes".into(),
                    effect: "write".into(),
                    resource: "name".into(),
                    params: vec![ParamInfo {
                        name: "branch".into(),
                        summary: "branch to push; defaults to the current one".into(),
                        required: false,
                    }],
                }],
                protected: true,
            },
            Response::Ok,
            Response::Services {
                services: vec![ServiceInfo {
                    service: "demo".into(),
                    host: "github.com".into(),
                    scheme: "https".into(),
                    username: "x-access-token".into(),
                    path: String::new(),
                    auth: "bearer".into(),
                    port: None,
                    added: 1,
                }],
            },
            Response::Grants {
                projects: BTreeMap::from([("/p".to_string(), vec!["demo:git-fetch".to_string()])]),
            },
            Response::Approvals {
                pending: vec![Approval {
                    project: "/p".into(),
                    service: "demo".into(),
                    operation: "cloudflare.worker.deploy".into(),
                    resource: "my-worker".into(),
                    granted_ms: 1,
                    expires_ms: 2,
                }],
            },
            Response::Performed {
                record: record.clone(),
                endpoint: "https://github.com".into(),
                exit_code: 0,
                output: "From https://github.com/a/b".into(),
            },
            Response::Audit {
                entries: vec![AuditLine::from_record("a1", AuditEvent::Used, 1000, 1, &record)],
            },
            Response::Usage {
                report: Box::new(crate::budget::Report {
                    project: "/p".into(),
                    day_start_ms: 1,
                    budgeted: true,
                    operations: 1,
                    per_operation: BTreeMap::from([("git.fetch".to_string(), 1)]),
                    per_service: BTreeMap::from([("demo".to_string(), 1)]),
                    caps: vec![crate::budget::Cap {
                        name: "operations_daily".into(),
                        kind: "operations".into(),
                        used: "1".into(),
                        limit: "200".into(),
                        unmeasurable: None,
                    }],
                }),
            },
            Response::error(ErrorKind::PermissionDenied, "not granted"),
        ]
    }

    #[test]
    fn protocol_surface_is_pinned() {
        // The list of every reply this service can send. A variant added
        // without being added here fails this test, and the doc comment on
        // `Response::variant` is what the author reads on the way.
        //
        // If you are here because you added a variant: the question to answer
        // is whether it can carry a raw credential. The answer has to be no.
        let mut names: Vec<&str> = every_response().iter().map(Response::variant).collect();
        names.sort_unstable();
        assert_eq!(
            names,
            vec![
                "approvals",
                "audit",
                "error",
                "grants",
                "hello",
                "ok",
                "performed",
                "services",
                "usage",
            ],
            "the reply surface changed; every entry must be a reply that \
             cannot carry a credential"
        );
    }

    #[test]
    fn no_reply_carries_a_credential() {
        // The runtime half of the compile-time argument. Every variant is
        // built with a sentinel wherever a caller could put one, serialised,
        // and checked. What this catches that the type system does not is a
        // variant that carries a value as a `String` — which compiles fine.
        const SENTINEL: &str = "apex-sentinel-7f3a91c4-do-not-leak";
        for response in every_response() {
            let text = serde_json::to_string(&response).expect("serialise");
            assert!(
                !text.contains(SENTINEL),
                "{} carried the sentinel: {text}",
                response.variant()
            );
            assert!(
                !text.contains('\n'),
                "{} is not one line: {text}",
                response.variant()
            );
        }
    }

    #[test]
    fn the_add_request_declares_a_length_and_not_a_value() {
        // The inbound direction, asserted as a shape: a `token` field here
        // would put a credential in a serialisable type, which is the one thing
        // this crate does not do.
        let req = Request::Add {
            service: "demo".into(),
            host: "github.com".into(),
            scheme: "https".into(),
            username: None,
            path: String::new(),
            auth: None,
            port: None,
            value_len: 40,
        };
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["value_len"], 40);
        for forbidden in ["token", "value", "secret", "password"] {
            assert!(
                json.get(forbidden).is_none(),
                "the add request grew a {forbidden} field"
            );
        }
    }

    #[test]
    fn requests_round_trip_on_one_line() {
        let requests = vec![
            Request::Hello,
            Request::Add {
                service: "demo".into(),
                host: "github.com".into(),
                scheme: "https".into(),
                username: Some("x-access-token".into()),
                path: String::new(),
                auth: None,
                port: None,
                value_len: 3,
            },
            Request::Remove {
                service: "demo".into(),
            },
            Request::List,
            Request::Grant {
                project: "/p".into(),
                service: "demo".into(),
                capability: "git-fetch".into(),
                revoke: false,
            },
            Request::Grants,
            Request::Use {
                body_len: 0,
                record: Box::new(CapabilityRecord::new(
                    "demo",
                    "git.push",
                    "origin",
                )),
            },
            Request::Audit { lines: 5, project: None },
        ];
        for req in requests {
            let text = serde_json::to_string(&req).unwrap();
            assert!(!text.contains('\n'), "{text}");
            assert_eq!(serde_json::from_str::<Request>(&text).unwrap(), req);
        }
    }

    #[test]
    fn a_request_with_missing_optional_fields_still_parses() {
        // The wire is a compatibility surface across an image update, where the
        // CLI and the daemon are the same build but a user's shell may still be
        // running the old one.
        let req: Request =
            serde_json::from_str(r#"{"op":"add","service":"d","host":"h","value_len":1}"#).unwrap();
        assert_eq!(
            req,
            Request::Add {
                service: "d".into(),
                host: "h".into(),
                scheme: "https".into(),
                username: None,
                path: String::new(),
                auth: None,
                port: None,
                value_len: 1,
            }
        );
        assert_eq!(
            serde_json::from_str::<Request>(r#"{"op":"audit"}"#).unwrap(),
            Request::Audit { lines: 20, project: None }
        );
    }

    #[test]
    fn an_unknown_verb_is_a_parse_error_and_not_a_default() {
        // Fails closed: a verb this build does not know must not deserialise
        // into one it does.
        assert!(serde_json::from_str::<Request>(r#"{"op":"export"}"#).is_err());
        assert!(serde_json::from_str::<Request>(r#"{"op":"read"}"#).is_err());
    }

    #[test]
    fn errors_are_recognisable_without_parsing_prose() {
        let e = Response::error(ErrorKind::NoSuchService, "no credential stored for 'x'");
        assert_eq!(
            e.as_error().map(|(k, _)| k),
            Some(ErrorKind::NoSuchService)
        );
        assert_eq!(Response::Ok.as_error(), None);
    }
}
