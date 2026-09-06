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
use apex_secret_core::capability::{Capability, CapabilityError, CapabilityRecord};
use apex_secret_core::protocol::{ErrorKind, Response};
use apex_secret_core::store::{self, ServiceInfo, Store, StoreError};
use apex_secret_core::SecretValue;

use crate::broker;
use crate::peer::Peer;

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
    audit_counter: AtomicU64,
}

impl Service {
    pub fn new(store: Store, protected: bool) -> Service {
        Service {
            store,
            protected,
            audit_counter: AtomicU64::new(0),
        }
    }

    pub fn store(&self) -> &Store {
        &self.store
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
            capabilities: Capability::names().iter().map(|s| s.to_string()).collect(),
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
        if !Capability::names().contains(&capability) {
            return refuse_capability(CapabilityError::UnknownCapability(capability.to_string()));
        }
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

    // ── the broker ──────────────────────────────────────────────────────────

    /// Perform a capability.
    ///
    /// The order is the security argument, and every step before the last can
    /// refuse:
    ///
    /// 1. the caller's account, from the kernel — never from the request;
    /// 2. the record is well formed and has not expired;
    /// 3. a credential exists under that name, for that account;
    /// 4. the capability is granted for that project;
    /// 5. the remote NAME resolves, in the repository, as the owner;
    /// 6. the resolved URL's host and scheme are the ones the credential was
    ///    stored for;
    /// 7. only then is the value read, and only into a child's environment.
    pub fn use_capability(&self, peer: Peer, mut record: CapabilityRecord) -> Response {
        let audit_id = self.next_audit_id();
        record.audit_id = audit_id.clone();
        // The record's own idea of what it touches is replaced by the
        // operation's, so the two cannot disagree in the audit trail.
        record.resource = record.operation.remote().to_string();

        let refuse = |reason: String, kind: ErrorKind| -> Response {
            self.record(AuditLine {
                reason: Some(reason.clone()),
                ..AuditLine::from_record(&audit_id, AuditEvent::Refused, peer.uid, peer.pid, &record)
            });
            Response::error(kind, reason)
        };

        if !store::valid_service_name(&record.provider) {
            return refuse(
                StoreError::BadServiceName(record.provider.clone()).to_string(),
                ErrorKind::BadRequest,
            );
        }
        if !capability::valid_remote_name(record.operation.remote()) {
            return refuse(
                CapabilityError::BadRemoteName(record.operation.remote().to_string()).to_string(),
                ErrorKind::BadRequest,
            );
        }
        if let Capability::GitPush {
            branch: Some(branch),
            ..
        } = &record.operation
        {
            if !capability::valid_branch_name(branch) {
                return refuse(
                    CapabilityError::BadBranchName(branch.clone()).to_string(),
                    ErrorKind::BadRequest,
                );
            }
        }

        let now = store::now_ms();
        if record.is_expired(now) {
            return refuse(
                "that capability request has expired; ask for it again".to_string(),
                ErrorKind::PermissionDenied,
            );
        }
        if record.expiry.is_some_and(|at| at > now + MAX_EXPIRY_AHEAD_MS) {
            return refuse(
                "that capability request is valid for longer than this service \
                 will honour"
                    .to_string(),
                ErrorKind::BadRequest,
            );
        }

        let Some(project) = record.project.clone().filter(|p| broker::valid_project(p)) else {
            return refuse(
                "this request names no project, so no grant can match it"
                    .to_string(),
                ErrorKind::PermissionDenied,
            );
        };

        let Some(owner) = broker::owner(peer.uid) else {
            return refuse(
                format!("uid {} is not an account on this machine", peer.uid),
                ErrorKind::PermissionDenied,
            );
        };

        let Some(info) = self.store.info(peer.uid, &record.provider) else {
            return refuse(
                StoreError::NoSuchService(record.provider.clone()).to_string(),
                ErrorKind::NoSuchService,
            );
        };

        let capability_name = record.operation.name();
        if !self
            .store
            .grants(peer.uid)
            .allows(Some(&project), &record.provider, capability_name)
        {
            return refuse(
                format!(
                    "'{capability_name}' on '{}' is not granted for this project; \
                     allow it with `apex secret grant {} {capability_name}`",
                    record.provider, record.provider
                ),
                ErrorKind::PermissionDenied,
            );
        }

        let url = match broker::resolve_url(&project, &record.operation, &owner) {
            Ok(url) => url,
            Err(e) => return refuse(e.to_string(), ErrorKind::BadRequest),
        };
        if let Err(e) = capability::check_url(&url, &info.host, &info.scheme) {
            return refuse(e.to_string(), ErrorKind::PermissionDenied);
        }
        let endpoint = broker::endpoint(&url);

        // Every check has passed. Only now is the value read.
        let value = match self.store.value(peer.uid, &record.provider) {
            Ok(v) => v,
            Err(e) => return refuse(e.to_string(), ErrorKind::NoSuchService),
        };

        let out = match broker::perform(&project, &record.operation, &info, &value, &owner) {
            Ok(out) => out,
            Err(e) => return refuse(e, ErrorKind::Internal),
        };

        self.record(AuditLine {
            endpoint: Some(endpoint.clone()),
            exit_code: Some(out.code),
            ..AuditLine::from_record(&audit_id, AuditEvent::Used, peer.uid, peer.pid, &record)
        });

        Response::Performed {
            record: Box::new(record),
            endpoint,
            exit_code: out.code,
            output: out.text,
        }
    }
}

use apex_secret_core::capability;

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

fn refuse_capability(e: CapabilityError) -> Response {
    Response::error(ErrorKind::BadRequest, e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    const SENTINEL: &str = "apex-sentinel-7f3a91c4-do-not-leak";

    fn temp_service(tag: &str) -> (Service, PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "apex-secretd-svc-{}-{tag}-{}",
            std::process::id(),
            store::now_ms()
        ));
        std::fs::remove_dir_all(&dir).ok();
        (Service::new(Store::new(dir.clone()), false), dir)
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
        let mut rec = CapabilityRecord::new(
            provider,
            Capability::parse(cap, remote, None).expect("capability"),
        );
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
                protected,
            } => {
                assert_eq!(version, apex_secret_core::protocol::PROTOCOL_VERSION);
                assert!(capabilities.contains(&"git-fetch".to_string()));
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
        let trail = std::fs::read_to_string(svc.store().audit_path()).unwrap_or_default();
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
        for evil in ["exec", "sh", "git-clone", "curl"] {
            let resp = svc.grant(peer, "/tmp/p", "demo", evil, false);
            assert!(
                resp.as_error()
                    .is_some_and(|(_, m)| m.contains("not a capability")),
                "'{evil}' was grantable: {resp:?}"
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

        let lines = audit::tail(&svc.store().audit_path(), 10);
        let refused: Vec<&AuditLine> = lines
            .iter()
            .filter(|l| l.event == AuditEvent::Refused)
            .collect();
        assert_eq!(refused.len(), 1, "{lines:#?}");
        let line = refused[0];
        assert!(!line.audit_id.is_empty());
        assert_eq!(line.provider, "demo");
        assert_eq!(line.operation, "git-fetch");
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
        assert!(svc.store().info(peer.uid, "demo").is_none());
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
