//! The wire protocol: newline-delimited JSON over a unix socket.
//!
//! The same shape `apex-agentd` uses, for the same reasons — one line in, one
//! line out, readable with `socat` when something is wrong, and no async
//! runtime in a program whose hot path is `read` and `write`.
//!
//! # Criterion two lives here
//!
//! *"The normal API cannot return raw secret values."* Not "does not" — cannot.
//! Two mechanisms, and the second exists because the first can be forgotten:
//!
//! 1. [`Response`] derives `Serialize`. [`crate::value::SecretValue`] does not
//!    implement it. A response variant carrying a value therefore does not
//!    compile, and there is no way to write one that does without deleting the
//!    wall in `value.rs` — a diff nobody merges by accident.
//! 2. [`payload_kind`] matches exhaustively over every variant and classifies
//!    it as metadata, an operation result, an acknowledgement or an error.
//!    [`Payload`] has no variant meaning "a credential". Adding a response
//!    variant fails to compile until somebody classifies it, and the only
//!    classifications available are ones that cannot hold a value.
//!
//! Values do travel the other way: [`Request::Store`] carries one *inbound*,
//! which is how a credential gets in at all. That is [`InboundSecret`], which
//! redacts itself in `Debug` so a daemon that logs a request cannot log a
//! credential.
//!
//! # Two sockets, not one verb set
//!
//! [`Request::admin_only`] splits the vocabulary. The broker socket is
//! reachable by any local uid and offers metadata and operations; the admin
//! socket is `0600` and offers mutation. The split is enforced by the daemon on
//! the socket a request arrived on, and stated here so both ends agree on which
//! is which.

use serde::{Deserialize, Serialize};

use crate::audit::AuditRecord;
use crate::capability::CapabilityRequest;
use crate::policy::Grant;
use crate::provider::Outcome;
use crate::seal::Sealing;
use crate::store::SecretMeta;
use crate::value::{SecretValue, ValueError};

/// Bumped when a change would make an old client misread a new daemon.
pub const PROTOCOL_VERSION: u32 = 1;

/// Longest request line the daemon will read.
///
/// A credential is at most 8 KiB and everything else on the wire is short, so
/// this is generous. It exists so a client that never sends a newline cannot
/// make the daemon buffer without limit.
pub const MAX_LINE_BYTES: usize = 64 * 1024;

/// A credential on its way *in*.
///
/// Distinct from [`SecretValue`] because it has to cross the wire, and
/// therefore has to implement `Serialize` and `Deserialize` — which
/// `SecretValue` deliberately does not. What it keeps is the other half: a
/// `Debug` that prints nothing, so the daemon's own diagnostics cannot leak a
/// credential the moment somebody adds `eprintln!("{request:?}")`.
#[derive(Clone, Serialize, Deserialize)]
#[serde(transparent)]
pub struct InboundSecret(String);

impl InboundSecret {
    pub fn new(value: impl Into<String>) -> InboundSecret {
        InboundSecret(value.into())
    }

    /// Convert to the type the rest of the service uses, applying the charset
    /// rules at the same time. Consumes itself: there is no reason to keep the
    /// unvalidated form around after this.
    pub fn into_value(self) -> Result<SecretValue, ValueError> {
        SecretValue::new(self.0)
    }
}

impl std::fmt::Debug for InboundSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("InboundSecret(<withheld>)")
    }
}

/// Everything a client can ask for.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "kebab-case")]
pub enum Request {
    /// Protocol version and the provider vocabulary.
    Hello,
    /// The providers and operations this build offers.
    Providers,
    /// Metadata for every credential in the caller's namespace.
    List,
    /// Metadata for one.
    Info { secret: String },
    /// The caller's standing grants.
    Grants,
    /// The tail of the caller's audit log.
    Audit {
        #[serde(default = "default_audit_limit")]
        limit: usize,
    },
    /// Perform an operation. The credential is used and not returned.
    Use(Box<CapabilityRequest>),

    // ── admin socket only ───────────────────────────────────────────────────
    /// Put a credential in.
    Store(Box<StoreRequest>),
    /// Replace the value of one that exists, keeping its grants.
    Rotate(Box<RotateRequest>),
    /// Delete a credential and every grant on it.
    Remove {
        #[serde(default)]
        owner: Option<u32>,
        secret: String,
    },
    /// Add or withdraw a standing grant.
    Grant(Box<GrantRequest>),
}

fn default_audit_limit() -> usize {
    50
}

/// The one request that carries a credential.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoreRequest {
    /// Whose namespace to store it in. `None` means the connecting uid, which
    /// under `sudo` is root — so the CLI fills this in from `$SUDO_UID`.
    #[serde(default)]
    pub owner: Option<u32>,
    pub secret: String,
    pub provider: String,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub sealing: Sealing,
    pub value: InboundSecret,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RotateRequest {
    #[serde(default)]
    pub owner: Option<u32>,
    pub secret: String,
    pub value: InboundSecret,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GrantRequest {
    #[serde(default)]
    pub owner: Option<u32>,
    pub secret: String,
    pub operation: String,
    #[serde(default)]
    pub resource: Option<String>,
    /// Seconds from now after which the grant lapses. `None` is standing.
    #[serde(default)]
    pub ttl_secs: Option<u64>,
    #[serde(default)]
    pub revoke: bool,
}

impl Request {
    /// Whether this verb may only arrive on the admin socket.
    ///
    /// Exhaustive on purpose: a new verb has to be classified, and the compiler
    /// says so. Getting this wrong in the permissive direction would let a
    /// managed agent grant itself a capability, which is the one thing the
    /// grant model exists to prevent.
    pub fn admin_only(&self) -> bool {
        match self {
            Request::Hello
            | Request::Providers
            | Request::List
            | Request::Info { .. }
            | Request::Grants
            | Request::Audit { .. }
            | Request::Use(_) => false,
            Request::Store(_)
            | Request::Rotate(_)
            | Request::Remove { .. }
            | Request::Grant(_) => true,
        }
    }

    /// The verb name, for the daemon's own diagnostics. Never the payload.
    pub fn verb(&self) -> &'static str {
        match self {
            Request::Hello => "hello",
            Request::Providers => "providers",
            Request::List => "list",
            Request::Info { .. } => "info",
            Request::Grants => "grants",
            Request::Audit { .. } => "audit",
            Request::Use(_) => "use",
            Request::Store(_) => "store",
            Request::Rotate(_) => "rotate",
            Request::Remove { .. } => "remove",
            Request::Grant(_) => "grant",
        }
    }
}

/// One operation, as `apex capability providers` lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationInfo {
    pub id: String,
    pub summary: String,
    pub method: String,
    /// `owner/repo`, or absent when the operation takes no resource.
    #[serde(default)]
    pub resource_shape: Option<String>,
    /// The response fields this operation may return.
    pub fields: Vec<String>,
}

/// One provider, as `apex capability providers` lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderInfo {
    pub id: String,
    pub summary: String,
    pub default_base: String,
    pub operations: Vec<OperationInfo>,
}

/// What an operation produced, plus the id that joins it to the audit log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Performed {
    pub audit_id: String,
    pub provider: String,
    pub operation: String,
    #[serde(default)]
    pub resource: Option<String>,
    pub outcome: Outcome,
}

/// Everything the daemon can say.
///
/// Every variant is classified by [`payload_kind`]. None of them can carry a
/// credential, and that is checked two ways — see the module note.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reply", rename_all = "kebab-case")]
pub enum Response {
    Hello {
        version: u32,
        providers: Vec<String>,
    },
    Providers {
        providers: Vec<ProviderInfo>,
    },
    Secrets {
        secrets: Vec<SecretMeta>,
    },
    Secret(Box<SecretMeta>),
    Grants {
        grants: Vec<Grant>,
    },
    Audit {
        entries: Vec<AuditRecord>,
    },
    Performed(Box<Performed>),
    Ok,
    Error {
        kind: ErrorKind,
        message: String,
    },
}

/// Why a request failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    /// Malformed, or self-contradictory.
    BadRequest,
    /// No credential of that name in the caller's namespace.
    NoSuchSecret,
    /// Unknown provider or operation.
    NoSuchOperation,
    /// The caller may not do this: no grant, or a mutating verb on the broker
    /// socket.
    PermissionDenied,
    /// The provider was reached and refused, or could not be reached.
    ProviderFailed,
    /// Anything else, including OS errors.
    Internal,
}

/// What a response variant is allowed to be.
///
/// There is deliberately no variant meaning "a credential". This enum is the
/// thing a future author has to look at when they add a response, and the fact
/// that none of the four options can hold a value is the answer to "may I
/// return one".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Payload {
    /// Facts about what is stored or granted. Never a value.
    Metadata,
    /// The result of an operation the broker performed. A status and
    /// allow-listed provider fields, scrubbed at the provider boundary.
    OperationResult,
    /// An acknowledgement with no body.
    Ack,
    /// A failure, described in text that never quotes a credential — it cannot,
    /// because `SecretValue` has no `Display`.
    Error,
}

/// Classify a response.
///
/// The match is exhaustive and there is no wildcard arm. That is the entire
/// mechanism: adding a variant to [`Response`] stops this compiling, and the
/// author has to choose one of four kinds, none of which is a credential.
pub fn payload_kind(response: &Response) -> Payload {
    match response {
        Response::Hello { .. }
        | Response::Providers { .. }
        | Response::Secrets { .. }
        | Response::Secret(_)
        | Response::Grants { .. }
        | Response::Audit { .. } => Payload::Metadata,
        Response::Performed(_) => Payload::OperationResult,
        Response::Ok => Payload::Ack,
        Response::Error { .. } => Payload::Error,
    }
}

impl Response {
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
    use crate::audit::{AuditRecord, Event};
    use crate::capability::{Claimed, Origin};
    use crate::provider::Outcome;

    /// One of every response variant.
    ///
    /// Built through an exhaustive classification below, so this list cannot
    /// silently fall behind the enum.
    fn every_response() -> Vec<Response> {
        vec![
            Response::Hello {
                version: PROTOCOL_VERSION,
                providers: vec!["github".into()],
            },
            Response::Providers {
                providers: vec![ProviderInfo {
                    id: "github".into(),
                    summary: "GitHub REST API".into(),
                    default_base: "https://api.github.com".into(),
                    operations: vec![OperationInfo {
                        id: "whoami".into(),
                        summary: "who".into(),
                        method: "GET".into(),
                        resource_shape: None,
                        fields: vec!["login".into()],
                    }],
                }],
            },
            Response::Secrets {
                secrets: vec![meta()],
            },
            Response::Secret(Box::new(meta())),
            Response::Grants {
                grants: vec![Grant {
                    secret: "gh".into(),
                    operation: "whoami".into(),
                    resource: None,
                    granted_ms: 1,
                    expiry_ms: None,
                }],
            },
            Response::Audit {
                entries: vec![AuditRecord::administrative(
                    Event::Stored,
                    1000,
                    0,
                    1,
                    Some("gh".into()),
                )],
            },
            Response::Performed(Box::new(Performed {
                audit_id: "0011223344556677".into(),
                provider: "github".into(),
                operation: "whoami".into(),
                resource: None,
                outcome: Outcome {
                    status: 200,
                    fields: [("login".to_string(), "andre".to_string())]
                        .into_iter()
                        .collect(),
                    detail: None,
                },
            })),
            Response::Ok,
            Response::error(ErrorKind::NoSuchSecret, "no credential called 'gh'"),
        ]
    }

    fn meta() -> SecretMeta {
        SecretMeta {
            name: "gh".into(),
            provider: "github".into(),
            base_url: None,
            label: Some("work".into()),
            sealing: Sealing::Plain,
            created_ms: 1,
            rotated_ms: None,
        }
    }

    // ── criterion two ───────────────────────────────────────────────────────

    #[test]
    fn no_response_variant_can_carry_a_credential() {
        // The classification wall. If someone adds a response variant, this
        // function stops compiling until they classify it — and none of the
        // four kinds is "a credential". The assertion below is the cheap half;
        // the compile error is the half that matters.
        for r in every_response() {
            let kind = payload_kind(&r);
            assert!(
                matches!(
                    kind,
                    Payload::Metadata | Payload::OperationResult | Payload::Ack | Payload::Error
                ),
                "{r:?} classified as {kind:?}"
            );
        }
    }

    #[test]
    fn every_response_variant_is_covered_by_the_sample_set() {
        // Keeps `every_response` honest. Each variant appears at least once, so
        // the serialisation sweep below really does sweep everything.
        let mut kinds: Vec<&str> = every_response()
            .iter()
            .map(|r| match r {
                Response::Hello { .. } => "hello",
                Response::Providers { .. } => "providers",
                Response::Secrets { .. } => "secrets",
                Response::Secret(_) => "secret",
                Response::Grants { .. } => "grants",
                Response::Audit { .. } => "audit",
                Response::Performed(_) => "performed",
                Response::Ok => "ok",
                Response::Error { .. } => "error",
            })
            .collect();
        kinds.sort_unstable();
        kinds.dedup();
        assert_eq!(
            kinds.len(),
            9,
            "a response variant is missing from the sample set: {kinds:?}"
        );
    }

    #[test]
    fn no_response_has_a_field_whose_name_says_it_holds_a_credential() {
        // Belt to the type wall's braces. A real credential cannot reach these
        // structs, so this checks the shape rather than a value: it walks every
        // JSON key a response can produce and refuses the names a careless
        // addition would use. Keys, not the whole document — "no credential
        // called 'gh'" is a correct error message and must stay sayable.
        fn keys(v: &serde_json::Value, out: &mut Vec<String>) {
            match v {
                serde_json::Value::Object(map) => {
                    for (k, child) in map {
                        out.push(k.to_ascii_lowercase());
                        keys(child, out);
                    }
                }
                serde_json::Value::Array(items) => {
                    for child in items {
                        keys(child, out);
                    }
                }
                _ => {}
            }
        }

        for r in every_response() {
            let mut found = Vec::new();
            keys(&serde_json::to_value(&r).expect("serialises"), &mut found);
            for key in &found {
                for banned in ["token", "password", "credential", "bearer", "value", "secret_"] {
                    assert!(
                        !key.contains(banned),
                        "response field '{key}' is named like a credential: {r:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn responses_round_trip_so_a_client_reads_what_the_daemon_wrote() {
        for r in every_response() {
            let text = serde_json::to_string(&r).expect("serialises");
            let back: Response = serde_json::from_str(&text)
                .unwrap_or_else(|e| panic!("cannot parse {text}: {e}"));
            assert_eq!(back, r);
        }
    }

    // ── the inbound direction ───────────────────────────────────────────────

    #[test]
    fn an_inbound_credential_redacts_itself_in_debug_output() {
        // The realistic leak: a daemon that logs the request it could not
        // handle. `Request` derives Debug, so this has to hold through it.
        let req = Request::Store(Box::new(StoreRequest {
            owner: Some(1000),
            secret: "gh".into(),
            provider: "github".into(),
            base_url: None,
            label: None,
            sealing: Sealing::Plain,
            value: InboundSecret::new("not-a-real-token-inbound"),
        }));
        let text = format!("{req:?}");
        assert!(!text.contains("not-a-real-token-inbound"), "{text}");
        assert!(text.contains("<withheld>"), "{text}");
    }

    #[test]
    fn an_inbound_credential_becomes_a_checked_value_or_an_error() {
        assert_eq!(
            InboundSecret::new("not-a-real-token-ok")
                .into_value()
                .unwrap()
                .expose(),
            "not-a-real-token-ok"
        );
        assert!(InboundSecret::new("has space").into_value().is_err());
    }

    // ── the socket split ────────────────────────────────────────────────────

    #[test]
    fn every_mutating_verb_is_admin_only_and_no_read_verb_is() {
        for r in [
            Request::Store(Box::new(StoreRequest {
                owner: None,
                secret: "gh".into(),
                provider: "github".into(),
                base_url: None,
                label: None,
                sealing: Sealing::Plain,
                value: InboundSecret::new("x"),
            })),
            Request::Rotate(Box::new(RotateRequest {
                owner: None,
                secret: "gh".into(),
                value: InboundSecret::new("x"),
            })),
            Request::Remove {
                owner: None,
                secret: "gh".into(),
            },
            Request::Grant(Box::new(GrantRequest {
                owner: None,
                secret: "gh".into(),
                operation: "whoami".into(),
                resource: None,
                ttl_secs: None,
                revoke: false,
            })),
        ] {
            assert!(r.admin_only(), "{} must be admin only", r.verb());
        }

        for r in [
            Request::Hello,
            Request::Providers,
            Request::List,
            Request::Info { secret: "gh".into() },
            Request::Grants,
            Request::Audit { limit: 10 },
            Request::Use(Box::new(CapabilityRequest {
                secret: "gh".into(),
                operation: "whoami".into(),
                resource: None,
                project: None,
                agent_session: None,
                origin: Origin::Local,
            })),
        ] {
            assert!(!r.admin_only(), "{} must not need the admin socket", r.verb());
        }
    }

    #[test]
    fn using_a_capability_is_not_a_mutation() {
        // Worth its own assertion: `use` is the verb an agent calls, and if it
        // were classified admin-only the service would be useless, while the
        // reverse mistake on `grant` would let an agent widen its own access.
        let use_req = Request::Use(Box::new(CapabilityRequest {
            secret: "gh".into(),
            operation: "whoami".into(),
            resource: None,
            project: Some("/p".into()),
            agent_session: Some(3),
            origin: Origin::Local,
        }));
        assert!(!use_req.admin_only());
        assert_eq!(use_req.verb(), "use");
    }

    #[test]
    fn requests_round_trip_over_the_wire() {
        let req = Request::Use(Box::new(CapabilityRequest {
            secret: "gh".into(),
            operation: "repo-metadata".into(),
            resource: Some("AndreNijman/apex-os".into()),
            project: Some("/p".into()),
            agent_session: Some(3),
            origin: Origin::Remote,
        }));
        let text = serde_json::to_string(&req).unwrap();
        let back: Request = serde_json::from_str(&text).unwrap();
        assert_eq!(back.verb(), "use");
        assert_eq!(back.admin_only(), req.admin_only());
    }

    #[test]
    fn a_claimed_field_is_optional_on_the_wire() {
        // A minimal client — a shell script with `jq` — must be able to ask.
        let req: Request =
            serde_json::from_str(r#"{"op":"use","secret":"gh","operation":"whoami"}"#).unwrap();
        let Request::Use(c) = req else {
            panic!("wrong verb")
        };
        assert_eq!(c.project, None);
        assert_eq!(c.agent_session, None);
        assert_eq!(c.origin, Origin::Local);
    }

    #[test]
    fn an_audit_record_survives_the_wire_with_its_claimed_fields_marked() {
        let mut rec = AuditRecord::administrative(Event::Used, 1000, 1000, 5, Some("gh".into()));
        rec.project = Claimed(Some("/p".into()));
        let text = serde_json::to_string(&Response::Audit {
            entries: vec![rec.clone()],
        })
        .unwrap();
        let back: Response = serde_json::from_str(&text).unwrap();
        assert_eq!(back, Response::Audit { entries: vec![rec] });
    }
}
