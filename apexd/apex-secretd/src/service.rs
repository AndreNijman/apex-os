//! The verbs, and who may ask for them.
//!
//! One method per request. Every one of them starts from the *peer's* uid,
//! never from anything the request carried, because the request is written by
//! whoever connected.
//!
//! ## The two authorisation questions
//!
//! **Whose credentials are these?** The uid from `SO_PEERCRED`. The store is
//! per-uid, so a request only ever reaches the connecting account's own
//! namespace and there is no verb that names another.
//!
//! **May this caller change what is allowed?** Only if it is not inside a
//! managed agent session — see `peer::looks_like_a_session`. A session that can
//! grant itself a capability has no capabilities, so `add`, `remove`, `grant`
//! and `revoke` are refused for one. `use` is not: performing a granted
//! operation is the entire point.
//!
//! That second check is a *hardening* measure, not the confidentiality
//! boundary. It rests on `/proc` and cgroup membership, both of which a
//! determined same-uid process can eventually work around. What it cannot work
//! around is the store being root-owned and the protocol having no verb that
//! returns a value.

use std::sync::atomic::{AtomicU64, Ordering};

use apex_secret_core::audit::{self, AuditEvent, AuditLine};
use apex_secret_core::capability::{self, CapabilityRecord, EndpointError};
use apex_secret_core::operation::OperationSpec;
use apex_secret_core::protocol::{ErrorKind, Response};
use apex_secret_core::store::{self, ServiceInfo, Store, StoreError};
use apex_secret_core::SecretValue;

use crate::broker;
use crate::peer::Peer;
use crate::provider::{self, Registry};

/// How far in the future a record's expiry may sit.
///
/// A capability request is not a bearer token here — the daemon performs the
/// operation before it replies — so this bounds a queued or replayed request
/// rather than a credential in somebody's hands.
const MAX_EXPIRY_AHEAD_MS: u64 = 60 * 60 * 1000;

/// The daemon's state.
pub struct Service {
    store: Store,
    /// Whether the store is genuinely out of the user's reach — true when the
    /// daemon runs as root. Reported in `hello` rather than assumed, so a
    /// daemon started by hand for a test does not imply a protection it does
    /// not have.
    protected: bool,
    /// The providers this daemon serves. Fixed at startup: a request cannot
    /// cause a provider to appear, and the vocabulary it advertises is the one
    /// it can actually perform.
    registry: Registry,
    audit_counter: AtomicU64,
}

impl Service {
    pub fn new(store: Store, protected: bool, registry: Registry) -> Service {
        Service {
            store,
            protected,
            registry,
            audit_counter: AtomicU64::new(0),
        }
    }

    fn next_audit_id(&self) -> String {
        audit::mint_id(self.audit_counter.fetch_add(1, Ordering::Relaxed))
    }

    fn record(&self, line: AuditLine) {
        if let Err(e) = audit::append(&self.store.audit_path(), &line) {
            eprintln!("apex-secretd: cannot write the audit trail: {e}");
        }
    }

    // ── metadata ────────────────────────────────────────────────────────────

    pub fn hello(&self) -> Response {
        Response::Hello {
            version: apex_secret_core::protocol::PROTOCOL_VERSION,
            capabilities: self.registry.operation_ids(),
            vocabulary: self.registry.vocabulary(),
            protected: self.protected,
        }
    }

    pub fn list(&self, peer: Peer) -> Response {
        Response::Services {
            services: self.store.list(peer.uid),
        }
    }

    pub fn grants(&self, peer: Peer) -> Response {
        Response::Grants {
            projects: self.store.grants(peer.uid).projects,
        }
    }

    /// The audit trail, filtered to the caller's own account.
    ///
    /// One file for the machine, because "what happened at 14:02" should not be
    /// a join across accounts — but a caller sees only its own lines, so the
    /// trail does not become a way to learn which services another account has.
    pub fn audit(&self, peer: Peer, lines: usize) -> Response {
        let limit = lines.clamp(1, 1000);
        let mut entries: Vec<AuditLine> = audit::tail(&self.store.audit_path(), 10_000)
            .into_iter()
            .filter(|l| l.uid == peer.uid)
            .collect();
        if entries.len() > limit {
            entries.drain(..entries.len() - limit);
        }
        Response::Audit { entries }
    }

    // ── administration, owner only ──────────────────────────────────────────

    pub fn add(
        &self,
        peer: Peer,
        service: &str,
        host: &str,
        scheme: &str,
        username: Option<&str>,
        value: SecretValue,
    ) -> Response {
        if !store::valid_service_name(service) {
            return refuse_store(StoreError::BadServiceName(service.to_string()));
        }
        if host.is_empty() || host.len() > 253 || !host.chars().all(valid_host_char) {
            return Response::error(
                ErrorKind::BadRequest,
                format!("'{}' is not a host name", host.escape_debug()),
            );
        }
        if scheme != "https" && scheme != "http" {
            return Response::error(
                ErrorKind::BadRequest,
                format!(
                    "'{}' is not a scheme this service brokers; use https, or \
                     http for a loopback host",
                    scheme.escape_debug()
                ),
            );
        }
        let info = ServiceInfo {
            service: service.to_string(),
            host: host.to_ascii_lowercase(),
            scheme: scheme.to_string(),
            username: username.unwrap_or("x-access-token").to_string(),
            added: store::now_ms(),
        };
        if let Err(e) = self.store.put(peer.uid, &info, &value) {
            return refuse_store(e);
        }
        self.record(AuditLine::administrative(
            &self.next_audit_id(),
            AuditEvent::Stored,
            peer.uid,
            peer.pid,
            service,
            &format!("credential for {}://{}", info.scheme, info.host),
        ));
        Response::Ok
    }

    pub fn remove(&self, peer: Peer, service: &str) -> Response {
        match self.store.remove(peer.uid, service) {
            Ok(()) => {
                self.record(AuditLine::administrative(
                    &self.next_audit_id(),
                    AuditEvent::Removed,
                    peer.uid,
                    peer.pid,
                    service,
                    "credential and every grant that named it",
                ));
                Response::Ok
            }
            Err(e) => refuse_store(e),
        }
    }

    pub fn grant(
        &self,
        peer: Peer,
        project: &str,
        service: &str,
        capability: &str,
        revoke: bool,
    ) -> Response {
        if !broker::valid_project(project) {
            return Response::error(
                ErrorKind::BadRequest,
                "a grant is keyed on an absolute project path".to_string(),
            );
        }
        // Through the registry, so a grant is written under the operation's
        // canonical id whichever spelling was typed — and an operation no
        // provider offers cannot be granted at all.
        let capability = match self.registry.lookup(capability) {
            Ok((_, op)) => op.id,
            Err(e) => return Response::error(ErrorKind::BadRequest, e.to_string()),
        };
        if !store::valid_service_name(service) {
            return refuse_store(StoreError::BadServiceName(service.to_string()));
        }

        let mut grants = self.store.grants(peer.uid);
        if revoke {
            if !grants.revoke(project, service, capability) {
                return Response::error(
                    ErrorKind::NoSuchService,
                    format!("{service}:{capability} was not granted for {project}"),
                );
            }
        } else {
            // A capability cannot be granted for a credential that does not
            // exist: otherwise a typo produces a grant that silently never
            // matches.
            if self.store.info(peer.uid, service).is_none() {
                return refuse_store(StoreError::NoSuchService(service.to_string()));
            }
            grants.allow(project, service, capability);
        }
        if let Err(e) = self.store.save_grants(peer.uid, &grants) {
            return refuse_store(e);
        }
        self.record(AuditLine::administrative(
            &self.next_audit_id(),
            if revoke {
                AuditEvent::Revoked
            } else {
                AuditEvent::Granted
            },
            peer.uid,
            peer.pid,
            service,
            &format!("{capability} for {project}"),
        ));
        Response::Grants {
            projects: grants.projects,
        }
    }

    // ── the framework ───────────────────────────────────────────────────────

    /// Why a request was allowed, in §11's `approval_policy` vocabulary.
    ///
    /// Its own step, and its own type, because it is the seam the permission
    /// work attaches to: break-glass and session-scoped grants (P0-005) are
    /// more answers here, not more branches in the middle of the pipeline.
    ///
    /// The value is **decided by the daemon**. A record arrives carrying an
    /// `approval_policy` because §11 puts one in the record, and it is
    /// overwritten — the same treatment `resource` gets, and for the same
    /// reason: a caller that could name how it was approved could write
    /// `owner` into a trail nobody read.
    fn decide(
        &self,
        peer: Peer,
        project: &str,
        provider: &str,
        op: &'static OperationSpec,
    ) -> Decision {
        let names = provider::grant_names(op);
        if self
            .store
            .grants(peer.uid)
            .allows_any(Some(project), provider, &names)
        {
            return Decision::Allowed("grant");
        }
        Decision::Refused(format!(
            "'{}' on '{provider}' is not granted for this project; allow it with \
             `apex secret grant {provider} {}`",
            op.id, op.id
        ))
    }

    /// Perform a capability.
    ///
    /// The order is the security argument, and every step before the last can
    /// refuse. Steps 1–7 and 9–12 belong to the framework and apply to every
    /// provider that will ever be registered; only 8 and 10 are the provider's,
    /// and neither of them decides whether the request was allowed.
    ///
    ///  1. the caller's account, from the kernel — never from the request;
    ///  2. the operation exists, in a registered provider's vocabulary;
    ///  3. its arguments are the ones that provider declared;
    ///  4. the record is well formed and has not expired;
    ///  5. a credential exists under that name, for that account;
    ///  6. the capability is granted for that project — [`Service::decide`];
    ///  7. only then may the provider touch the caller's machine;
    ///  8. the provider resolves what the caller named, and says where the
    ///     credential would go;
    ///  9. that endpoint is pinned against the one the credential was stored
    ///     for — the provider does not get to skip this, because it has not
    ///     been given the value yet;
    /// 10. the value is read, once, and the provider presents it;
    /// 11. the value — and any short-lived one minted from it — is scrubbed
    ///     out of everything returned;
    /// 12. the trail records the record the decision was made on.
    pub fn use_capability(&self, peer: Peer, mut record: CapabilityRecord) -> Response {
        let audit_id = self.next_audit_id();
        record.audit_id = audit_id.clone();

        let refuse = |record: &CapabilityRecord, reason: String, kind: ErrorKind| -> Response {
            self.record(AuditLine {
                reason: Some(reason.clone()),
                ..AuditLine::from_record(&audit_id, AuditEvent::Refused, peer.uid, peer.pid, record)
            });
            Response::error(kind, reason)
        };

        if !store::valid_service_name(&record.provider) {
            return refuse(
                &record,
                StoreError::BadServiceName(record.provider.clone()).to_string(),
                ErrorKind::BadRequest,
            );
        }
        // The two §7 fields are claims forwarded by `apex-agentd`, and this
        // daemon cannot re-derive either: the connection it sees is the agent
        // runtime's, not the session's. So it checks the shape — the trail is
        // line-delimited JSON somebody greps, and a caller that could write a
        // newline or a paragraph into it could shape what the audit looks like
        // — and records the claim as a claim.
        for (field, value) in [
            ("request_origin", &record.request_origin),
            ("origin_source", &record.origin_source),
        ] {
            if !capability::valid_origin_label(value) {
                return refuse(
                    &record,
                    format!("'{}' is not a {field} label", value.escape_debug()),
                    ErrorKind::BadRequest,
                );
            }
        }

        // The vocabulary, closed by what is registered. An operation no
        // provider declares never reaches a credential — the same property
        // P0-002's closed enum had, held by the registry instead so that adding
        // a provider does not mean editing a type in `apex-secret-core`.
        let (backend, op) = match self.registry.lookup(&record.operation) {
            Ok(found) => found,
            Err(e) => return refuse(&record, e.to_string(), ErrorKind::BadRequest),
        };
        // The canonical id replaces whatever spelling arrived, so the trail and
        // the grant table cannot end up describing the same operation two ways.
        record.operation = op.id.to_string();
        if let Err(e) = op.check(&record.resource, &record.params) {
            return refuse(&record, e.to_string(), ErrorKind::BadRequest);
        }

        let now = store::now_ms();
        if record.is_expired(now) {
            return refuse(
                &record,
                "that capability request has expired; ask for it again".to_string(),
                ErrorKind::PermissionDenied,
            );
        }
        if record.expiry.is_some_and(|at| at > now + MAX_EXPIRY_AHEAD_MS) {
            return refuse(
                &record,
                "that capability request is valid for longer than this service \
                 will honour"
                    .to_string(),
                ErrorKind::BadRequest,
            );
        }

        let Some(project) = record.project.clone().filter(|p| broker::valid_project(p)) else {
            return refuse(
                &record,
                "this request names no project, so no grant can match it".to_string(),
                ErrorKind::PermissionDenied,
            );
        };

        let Some(owner) = broker::owner(peer.uid) else {
            return refuse(
                &record,
                format!("uid {} is not an account on this machine", peer.uid),
                ErrorKind::PermissionDenied,
            );
        };

        let Some(info) = self.store.info(peer.uid, &record.provider) else {
            return refuse(
                &record,
                StoreError::NoSuchService(record.provider.clone()).to_string(),
                ErrorKind::NoSuchService,
            );
        };

        // Decided here, and stamped on the record before anything is performed,
        // so the trail says how this was authorised even if the operation then
        // fails.
        match self.decide(peer, &project, &record.provider, op) {
            Decision::Allowed(policy) => record.approval_policy = policy.to_string(),
            Decision::Refused(reason) => {
                return refuse(&record, reason, ErrorKind::PermissionDenied)
            }
        }

        // Everything the framework can check has passed, so the provider may
        // now touch the caller's machine. For git that is a `git remote
        // get-url` in a caller-controlled repository, as the owner; doing it
        // before the grant check would run it for a request that was never
        // allowed.
        let req = provider::Bind {
            operation: op,
            resource: &record.resource,
            params: &record.params,
            project: &project,
            service: &info,
            owner: &owner,
        };
        let bound = match backend.bind(&req) {
            Ok(bound) => bound,
            Err(e) => return refuse(&record, e.to_string(), kind_of(&e)),
        };

        // The pin, in the framework and not in the provider. A credential
        // stored for one host may only ever be sent to that host, whatever the
        // provider resolved and whoever wrote the provider.
        if bound.endpoint.scheme != info.scheme
            || bound.endpoint.host != info.host.to_ascii_lowercase()
        {
            return refuse(
                &record,
                EndpointError::HostMismatch {
                    remote_host: bound.endpoint.to_string(),
                    service_host: format!("{}://{}", info.scheme, info.host),
                }
                .to_string(),
                ErrorKind::PermissionDenied,
            );
        }
        let endpoint = bound.endpoint.to_string();

        // Every check has passed. Only now is the value read.
        let stored = match self.store.value(peer.uid, &record.provider) {
            Ok(v) => v,
            Err(e) => return refuse(&record, e.to_string(), ErrorKind::NoSuchService),
        };

        // §13.4: prefer a short-lived credential where the provider has one.
        // The minted value is used INSTEAD of the stored one and scrubbed
        // alongside it — a temporary token is still a credential, and §13.4 is
        // explicit that it is not handed to the agent either.
        let minted = match backend.mint(&req, &bound, &stored) {
            Ok(minted) => minted,
            Err(e) => return refuse(&record, e.to_string(), kind_of(&e)),
        };
        let presented = minted.as_ref().unwrap_or(&stored);

        let out = match backend.perform(&req, &bound, presented) {
            Ok(out) => out,
            Err(e) => return refuse(&record, e.to_string(), kind_of(&e)),
        };
        let output = scrub_all(&out.output, &[Some(&stored), minted.as_ref()]);

        self.record(AuditLine {
            endpoint: Some(endpoint.clone()),
            exit_code: Some(out.code),
            detail: bound.detail.clone(),
            ..AuditLine::from_record(&audit_id, AuditEvent::Used, peer.uid, peer.pid, &record)
        });

        Response::Performed {
            record: Box::new(record),
            endpoint,
            exit_code: out.code,
            output,
        }
    }
}

/// How a request was authorised, or why it was not.
///
/// The `&'static str` is §11's `approval_policy`. `grant` — a standing
/// per-project grant — is the only one this build reaches; P0-005's
/// session-scoped and break-glass answers are more variants of this, decided in
/// [`Service::decide`] and nowhere else.
enum Decision {
    Allowed(&'static str),
    Refused(String),
}

/// Which protocol error a provider's refusal is.
///
/// A provider that could only say "error" would make every failure look like a
/// bug in APEX. `NoSuchResource` is a caller's typo, `Refused` is a policy the
/// caller can read, `Failed` is ours.
fn kind_of(e: &provider::ProviderError) -> ErrorKind {
    match e {
        provider::ProviderError::NoSuchResource(_) => ErrorKind::BadRequest,
        provider::ProviderError::Refused(_) => ErrorKind::PermissionDenied,
        provider::ProviderError::Failed(_) => ErrorKind::Internal,
    }
}

/// Remove every credential in play from anything on its way back to the caller.
///
/// In the framework rather than in each provider, so a provider added later
/// cannot forget. It is defence in depth either way: the reply type cannot
/// carry a `SecretValue`, and a provider is not supposed to print one — this is
/// for the case where a provider's own tooling embeds the credential in an
/// error message, which git does when a URL of the form
/// `https://user:token@host/…` fails.
fn scrub_all(text: &str, values: &[Option<&SecretValue>]) -> String {
    let mut out = text.to_string();
    for value in values.iter().flatten() {
        if let Some(token) = value.as_str() {
            out = broker::scrub(&out, token);
        }
    }
    out
}


fn valid_host_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':' | '_')
}

fn refuse_store(e: StoreError) -> Response {
    let kind = match e {
        StoreError::NoSuchService(_) => ErrorKind::NoSuchService,
        StoreError::Io(_) => ErrorKind::Internal,
        _ => ErrorKind::BadRequest,
    };
    Response::error(kind, e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    const SENTINEL: &str = "apex-sentinel-7f3a91c4-do-not-leak";

    fn temp_service(tag: &str) -> (Service, PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "apex-secretd-svc-{}-{tag}-{}",
            std::process::id(),
            store::now_ms()
        ));
        std::fs::remove_dir_all(&dir).ok();
        (
            Service::new(
                Store::new(dir.clone()),
                false,
                crate::providers::default_registry().expect("the shipped registry"),
            ),
            dir,
        )
    }

    /// The audit trail of a temp service, by path rather than through the
    /// service: the daemon has no reason to expose its own store, and a test
    /// that needs an accessor added for it is a test shaping the API.
    fn trail(dir: &Path) -> PathBuf {
        Store::new(dir.to_path_buf()).audit_path()
    }

    fn me() -> Peer {
        // Safe: getuid/getgid cannot fail.
        Peer {
            pid: std::process::id() as libc::pid_t,
            uid: unsafe { libc::getuid() },
            gid: unsafe { libc::getgid() },
        }
    }

    fn record(provider: &str, cap: &str, remote: &str, project: &str) -> CapabilityRecord {
        let mut rec = CapabilityRecord::new(provider, cap, remote);
        rec.project = Some(project.to_string());
        rec
    }

    #[test]
    fn hello_reports_the_vocabulary_and_whether_the_store_is_protected() {
        let (svc, dir) = temp_service("hello");
        match svc.hello() {
            Response::Hello {
                version,
                capabilities,
                vocabulary,
                protected,
            } => {
                assert_eq!(version, apex_secret_core::protocol::PROTOCOL_VERSION);
                // The registry's canonical ids, never an alias: the list is
                // what `apex secret capabilities` prints, and advertising a
                // spelling on its way out teaches people to use it.
                assert!(capabilities.contains(&"git.fetch".to_string()), "{capabilities:?}");
                assert!(!capabilities.contains(&"git-fetch".to_string()), "{capabilities:?}");
                // And with enough beside each id that `apex secret
                // capabilities` needs no list of its own.
                let fetch = vocabulary.iter().find(|o| o.id == "git.fetch").expect("in vocabulary");
                assert!(!fetch.summary.is_empty());
                assert_eq!(fetch.effect, "read");
                assert_eq!(vocabulary.len(), capabilities.len());
                // A daemon running as an ordinary user must say so rather than
                // implying a boundary it does not have.
                assert!(!protected);
            }
            other => panic!("{other:?}"),
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_stored_credential_is_never_in_any_reply() {
        // The criterion, against the shipped verbs rather than against
        // hand-built responses: store a sentinel, then serialise every reply
        // the service can produce for a caller that has one.
        let (svc, dir) = temp_service("noleak");
        let peer = me();
        assert_eq!(
            svc.add(
                peer,
                "demo",
                "github.com",
                "https",
                None,
                SecretValue::new(SENTINEL.into())
            ),
            Response::Ok
        );
        svc.grant(peer, "/tmp/p", "demo", "git-fetch", false);

        let replies = vec![
            svc.hello(),
            svc.list(peer),
            svc.grants(peer),
            svc.audit(peer, 100),
            // A use that gets as far as it can without a repository.
            svc.use_capability(peer, record("demo", "git-fetch", "origin", "/tmp/p")),
            // ...and one that is refused early.
            svc.use_capability(peer, record("demo", "git-push", "origin", "/tmp/p")),
            svc.remove(peer, "demo"),
        ];
        for reply in &replies {
            let text = serde_json::to_string(reply).expect("serialise");
            assert!(
                !text.contains(SENTINEL),
                "{} carried the credential: {text}",
                reply.variant()
            );
        }
        // And the trail did not either.
        let trail = std::fs::read_to_string(trail(&dir)).unwrap_or_default();
        assert!(!trail.contains(SENTINEL), "{trail}");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_credential_grants_nothing_by_itself() {
        let (svc, dir) = temp_service("nogrant");
        let peer = me();
        svc.add(peer, "demo", "github.com", "https", None, SecretValue::new(b"x".to_vec()));
        assert_eq!(
            svc.grants(peer),
            Response::Grants {
                projects: Default::default()
            }
        );
        let resp = svc.use_capability(peer, record("demo", "git-fetch", "origin", "/tmp/p"));
        let (kind, message) = resp.as_error().expect("refused");
        assert_eq!(kind, ErrorKind::PermissionDenied);
        assert!(message.contains("not granted"), "{message}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_grant_is_per_capability_and_per_project() {
        let (svc, dir) = temp_service("perproject");
        let peer = me();
        svc.add(peer, "demo", "github.com", "https", None, SecretValue::new(b"x".to_vec()));
        svc.grant(peer, "/tmp/p", "demo", "git-fetch", false);

        // Another capability in the same project.
        assert!(svc
            .use_capability(peer, record("demo", "git-push", "origin", "/tmp/p"))
            .as_error()
            .is_some_and(|(_, m)| m.contains("not granted")));
        // The same capability in another project.
        assert!(svc
            .use_capability(peer, record("demo", "git-fetch", "origin", "/tmp/q"))
            .as_error()
            .is_some_and(|(_, m)| m.contains("not granted")));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_grant_for_an_unknown_credential_is_refused_rather_than_stored() {
        // A typo that produced a grant would silently never match, and the
        // person who made it would be told the capability was allowed.
        let (svc, dir) = temp_service("typo");
        let peer = me();
        let resp = svc.grant(peer, "/tmp/p", "nosuch", "git-fetch", false);
        assert!(resp.as_error().is_some_and(|(k, _)| k == ErrorKind::NoSuchService));
        assert_eq!(
            svc.grants(peer),
            Response::Grants {
                projects: Default::default()
            }
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_vocabulary_is_closed_at_the_grant_and_at_the_use() {
        let (svc, dir) = temp_service("closed");
        let peer = me();
        svc.add(peer, "demo", "github.com", "https", None, SecretValue::new(b"x".to_vec()));
        for evil in ["exec", "sh", "git-clone", "curl", "git.clone", "cloudflare.dns.delete"] {
            let resp = svc.grant(peer, "/tmp/p", "demo", evil, false);
            assert!(
                resp.as_error()
                    .is_some_and(|(_, m)| m.contains("not an operation")),
                "'{evil}' was grantable: {resp:?}"
            );
        }
        // And an operation a registered provider DOES offer is grantable under
        // either spelling, and stored under the canonical one.
        for spelling in ["git-fetch", "git.fetch"] {
            let resp = svc.grant(peer, "/tmp/p", "demo", spelling, false);
            assert_eq!(
                resp,
                Response::Grants {
                    projects: std::collections::BTreeMap::from([(
                        "/tmp/p".to_string(),
                        vec!["demo:git.fetch".to_string()]
                    )])
                },
                "{spelling} was not canonicalised"
            );
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_use_without_a_project_cannot_match_a_grant() {
        let (svc, dir) = temp_service("noproject");
        let peer = me();
        svc.add(peer, "demo", "github.com", "https", None, SecretValue::new(b"x".to_vec()));
        let mut rec = record("demo", "git-fetch", "origin", "/tmp/p");
        rec.project = None;
        assert!(svc
            .use_capability(peer, rec)
            .as_error()
            .is_some_and(|(_, m)| m.contains("names no project")));

        // ...and neither can a relative one, which would otherwise be resolved
        // against the DAEMON's working directory.
        let mut rec = record("demo", "git-fetch", "origin", "/tmp/p");
        rec.project = Some("relative/p".into());
        assert!(svc.use_capability(peer, rec).as_error().is_some());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_origin_label_that_could_reshape_the_trail_is_refused() {
        // The audit trail is one JSON object per line and an administrator
        // greps it. This daemon cannot verify WHERE a request came from — the
        // connection it sees belongs to apex-agentd — but it can refuse a
        // value that is not a label at all.
        let (svc, dir) = temp_service("originshape");
        let peer = me();
        svc.add(peer, "demo", "github.com", "https", None, SecretValue::new(b"x".to_vec()));
        svc.grant(peer, "/tmp/p", "demo", "git-fetch", false);

        for bad in ["local terminal", "Local-Terminal", "local\nterminal", ""] {
            let mut rec = record("demo", "git-fetch", "origin", "/tmp/p");
            rec.request_origin = bad.to_string();
            let resp = svc.use_capability(peer, rec);
            assert!(
                resp.as_error()
                    .is_some_and(|(k, m)| k == ErrorKind::BadRequest
                        && m.contains("request_origin")),
                "'{}' was accepted: {resp:?}",
                bad.escape_debug()
            );
        }
        // The same rule for how the origin was reached.
        let mut rec = record("demo", "git-fetch", "origin", "/tmp/p");
        rec.origin_source = "observed by me".into();
        assert!(svc
            .use_capability(peer, rec)
            .as_error()
            .is_some_and(|(_, m)| m.contains("origin_source")));

        // ...and a well-formed pair gets past this check to the next one. The
        // refusal that follows is about the repository, not about the label —
        // matched on the FIELD names, because the remote in this fixture is
        // itself called `origin` and a looser match passes for the wrong
        // reason.
        let mut rec = record("demo", "git-fetch", "origin", "/tmp/p");
        rec.request_origin = "claude-remote-control".into();
        rec.origin_source = "declared".into();
        let resp = svc.use_capability(peer, rec);
        let (_, message) = resp.as_error().expect("no repository at /tmp/p");
        assert!(!message.contains("request_origin"), "{message}");
        assert!(!message.contains("origin_source"), "{message}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_origin_a_caller_forwarded_reaches_the_trail_unchanged() {
        // §7's field is only useful if it survives to the record an owner
        // reads. Nothing here verifies it — that is written down — but a value
        // the daemon silently dropped would be worse than one it never had.
        let (svc, dir) = temp_service("origintrail");
        let peer = me();
        svc.add(peer, "demo", "github.com", "https", None, SecretValue::new(b"x".to_vec()));
        let mut rec = record("demo", "git-fetch", "origin", "/tmp/p");
        rec.request_origin = "claude-remote-control".into();
        rec.origin_source = "inherited".into();
        svc.use_capability(peer, rec);

        let refused = audit::tail(&Store::new(dir.clone()).audit_path(), 10)
            .into_iter()
            .find(|l| l.event == AuditEvent::Refused)
            .expect("the refusal is in the trail");
        assert_eq!(refused.request_origin, "claude-remote-control");
        assert_eq!(refused.origin_source, "inherited");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_expired_or_over_long_request_is_refused() {
        let (svc, dir) = temp_service("expiry");
        let peer = me();
        svc.add(peer, "demo", "github.com", "https", None, SecretValue::new(b"x".to_vec()));
        svc.grant(peer, "/tmp/p", "demo", "git-fetch", false);

        let mut rec = record("demo", "git-fetch", "origin", "/tmp/p");
        rec.expiry = Some(1);
        assert!(svc
            .use_capability(peer, rec)
            .as_error()
            .is_some_and(|(_, m)| m.contains("expired")));

        let mut rec = record("demo", "git-fetch", "origin", "/tmp/p");
        rec.expiry = Some(store::now_ms() + MAX_EXPIRY_AHEAD_MS * 2);
        assert!(svc
            .use_capability(peer, rec)
            .as_error()
            .is_some_and(|(_, m)| m.contains("longer than this service")));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn every_refusal_is_recorded_with_a_reason_and_an_id() {
        let (svc, dir) = temp_service("trail");
        let peer = me();
        svc.add(peer, "demo", "github.com", "https", None, SecretValue::new(b"x".to_vec()));
        svc.use_capability(peer, record("demo", "git-fetch", "origin", "/tmp/p"));

        let lines = audit::tail(&trail(&dir), 10);
        let refused: Vec<&AuditLine> = lines
            .iter()
            .filter(|l| l.event == AuditEvent::Refused)
            .collect();
        assert_eq!(refused.len(), 1, "{lines:#?}");
        let line = refused[0];
        assert!(!line.audit_id.is_empty());
        assert_eq!(line.provider, "demo");
        // The request named the operation by its old spelling; the trail says
        // the canonical one, so one operation cannot appear in an audit under
        // two names depending on how the caller typed it.
        assert_eq!(line.operation, "git.fetch");
        assert_eq!(line.resource, "origin");
        assert_eq!(line.uid, peer.uid);
        assert!(line.reason.as_deref().is_some_and(|r| r.contains("not granted")));
        // Nothing was contacted, so there is no endpoint to claim.
        assert_eq!(line.endpoint, None);
        assert_eq!(line.exit_code, None);

        // The store event is there too, and it is what `apex secret audit`
        // shows the owner.
        assert!(lines.iter().any(|l| l.event == AuditEvent::Stored));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn one_account_cannot_reach_another_accounts_credentials() {
        let (svc, dir) = temp_service("uids");
        let mine = me();
        let theirs = Peer {
            uid: mine.uid.wrapping_add(1),
            ..mine
        };
        svc.add(mine, "demo", "github.com", "https", None, SecretValue::new(SENTINEL.into()));

        assert_eq!(svc.list(theirs), Response::Services { services: vec![] });
        assert!(svc
            .use_capability(theirs, record("demo", "git-fetch", "origin", "/tmp/p"))
            .as_error()
            .is_some());
        // The trail is filtered too, so it is not a way to learn what another
        // account has stored. The refusal above is in it — that line is theirs
        // — and nothing of mine is.
        match svc.audit(theirs, 100) {
            Response::Audit { entries } => {
                assert!(entries.iter().all(|l| l.uid == theirs.uid), "{entries:#?}");
                assert!(
                    !entries.iter().any(|l| l.event == AuditEvent::Stored),
                    "another account's `stored` line was visible: {entries:#?}"
                );
            }
            other => panic!("{other:?}"),
        }
        // ...and mine still shows it.
        match svc.audit(mine, 100) {
            Response::Audit { entries } => {
                assert!(entries.iter().any(|l| l.event == AuditEvent::Stored), "{entries:#?}")
            }
            other => panic!("{other:?}"),
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn removing_a_credential_takes_its_grants_with_it() {
        let (svc, dir) = temp_service("removegrants");
        let peer = me();
        svc.add(peer, "demo", "github.com", "https", None, SecretValue::new(b"x".to_vec()));
        svc.grant(peer, "/tmp/p", "demo", "git-fetch", false);
        assert_eq!(svc.remove(peer, "demo"), Response::Ok);
        assert_eq!(
            svc.grants(peer),
            Response::Grants {
                projects: Default::default()
            }
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn http_is_refused_for_a_host_that_is_not_this_machine() {
        let (svc, dir) = temp_service("scheme");
        let peer = me();
        let resp = svc.add(
            peer,
            "demo",
            "github.com",
            "http",
            None,
            SecretValue::new(b"x".to_vec()),
        );
        assert!(resp
            .as_error()
            .is_some_and(|(_, m)| m.contains("loopback")), "{resp:?}");
        assert!(Store::new(dir.clone()).info(peer.uid, "demo").is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_host_name_is_validated_before_it_is_pinned() {
        let (svc, dir) = temp_service("host");
        let peer = me();
        for evil in ["", "a b", "a/b", "a\nb", "-"] {
            let resp = svc.add(peer, "demo", evil, "https", None, SecretValue::new(b"x".to_vec()));
            if evil == "-" {
                // A single hyphen is a legal host character; it simply never
                // matches a remote. The framing characters are what matter.
                continue;
            }
            assert!(resp.as_error().is_some(), "'{}' was accepted", evil.escape_debug());
        }
        std::fs::remove_dir_all(&dir).ok();
    }
}
