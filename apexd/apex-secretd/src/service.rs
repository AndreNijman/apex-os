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

    pub fn add(&self, peer: Peer, new: NewService<'_>, value: SecretValue) -> Response {
        let NewService {
            service,
            host,
            scheme,
            username,
            path,
            auth,
            port,
        } = new;
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
        if !store::valid_endpoint_path(path) {
            return Response::error(
                ErrorKind::BadRequest,
                format!(
                    "'{}' is not an endpoint path; it must start with '/' and \
                     carry no query, fragment or '..'",
                    path.escape_debug()
                ),
            );
        }
        let auth = auth.unwrap_or("bearer");
        if auth != "bearer" && auth != "raw" {
            return Response::error(
                ErrorKind::BadRequest,
                format!(
                    "'{}' is not an auth scheme; use bearer or raw",
                    auth.escape_debug()
                ),
            );
        }
        let info = ServiceInfo {
            service: service.to_string(),
            host: host.to_ascii_lowercase(),
            scheme: scheme.to_string(),
            username: username.unwrap_or("x-access-token").to_string(),
            path: path.to_string(),
            auth: auth.to_string(),
            port,
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
        let everywhere = project == store::ANY_PROJECT;
        if !everywhere && !broker::valid_project(project) {
            return Response::error(
                ErrorKind::BadRequest,
                "a grant is keyed on an absolute project path".to_string(),
            );
        }
        // Through the registry, so a grant is written under the operation's
        // canonical id whichever spelling was typed — and an operation no
        // provider offers cannot be granted at all.
        let op = match self.registry.lookup(capability) {
            Ok((_, op)) => op,
            Err(e) => return Response::error(ErrorKind::BadRequest, e.to_string()),
        };
        // The gate on `*`, here rather than in the CLI, because the CLI is not
        // the trust boundary: this socket takes a request from anything running
        // as the account. See [`may_be_granted_everywhere`] for what earns it.
        if everywhere && !may_be_granted_everywhere(op) {
            return Response::error(
                ErrorKind::BadRequest,
                format!(
                    "'{}' resolves against the project it is asked in, so it is granted \
                     where you name it and not everywhere. Run `apex secret grant` in \
                     the project.",
                    op.id
                ),
            );
        }
        let capability = op.id;
        if !store::valid_service_name(service) {
            return refuse_store(StoreError::BadServiceName(service.to_string()));
        }

        let mut grants = self.store.grants(peer.uid);
        if revoke {
            // Every name the operation answers to, not just the canonical one.
            // A grant written before a rename is on disk under the old
            // spelling, and a revoke that missed it would report "was not
            // granted" and leave it there.
            if !grants.revoke_any(project, service, &provider::grant_names(op)) {
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
        // The one message somebody reads in a worktree where a global MCP
        // server has stopped working, relayed to them by the agent as a
        // JSON-RPC error. It has to carry the way out, and for an operation
        // that names nothing the way out is usually `--everywhere` rather than
        // running the same grant again in each new directory.
        let everywhere = if may_be_granted_everywhere(op) {
            format!(
                ", or in every project with `apex secret grant {provider} {} --everywhere`",
                op.id
            )
        } else {
            String::new()
        };
        Decision::Refused(format!(
            "'{}' on '{provider}' is not granted for this project; allow it with \
             `apex secret grant {provider} {}`{everywhere}",
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
    ///
    /// `body` is the message an operation carries, for the one operation that
    /// carries one. It is not checked here and never can be: what a message
    /// means is the provider's, and the framework's job is to make sure the
    /// operation carrying it was allowed. Empty for every git operation.
    pub fn use_capability(
        &self,
        peer: Peer,
        mut record: CapabilityRecord,
        body: Vec<u8>,
    ) -> Response {
        let audit_id = self.next_audit_id();
        record.audit_id = audit_id.clone();
        // §11's `approval_policy` is the daemon's answer, and until `decide`
        // gives one there is no answer. Cleared before the refusal path can
        // capture it: a caller that arrived claiming `owner` must not have that
        // word appear on a `refused` line, where nobody would think to doubt it.
        record.approval_policy = UNDECIDED.to_string();

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
            body: &body,
            project: &project,
            service: &info,
            owner: &owner,
            audit_id: &audit_id,
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

        // §13.10's half that has to happen BEFORE anything runs.
        //
        // An operation that will create a credential — an Access service token,
        // a Tunnel credential — declares the name it would be stored under when
        // it binds. The far side issues such a secret **once** and never shows
        // it again, so a name that is already taken has to be refused here,
        // while refusing is still free. Refusing after the call would leave a
        // credential that exists at Cloudflare, is in nobody's hands, and
        // cannot be fetched again.
        if let Some(name) = &bound.creates {
            if !store::valid_service_name(name) {
                return refuse(
                    &record,
                    StoreError::BadServiceName(name.clone()).to_string(),
                    ErrorKind::BadRequest,
                );
            }
            if self.store.info(peer.uid, name).is_some() {
                return refuse(
                    &record,
                    format!(
                        "this operation would store the credential it creates as \
                         '{name}', and a credential is already stored under that \
                         name. Overwriting it would destroy a secret the provider \
                         will not show again, so this service will not do it. \
                         Remove the stored one with `apex secret remove {name}` \
                         if that is what you meant"
                    ),
                    ErrorKind::BadRequest,
                );
            }
        }

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
            // Scrubbed, like every refusal from here on. See the note below.
            Err(e) => {
                let reason = scrub_all(&e.to_string(), &[Some(&stored)]);
                return refuse(&record, reason, kind_of(&e));
            }
        };
        let presented = minted.as_ref().unwrap_or(&stored);

        let out = match backend.perform(&req, &bound, presented) {
            Ok(out) => out,
            // A provider is not supposed to put a credential in an error, and
            // one that does is not hypothetical: the natural way to write
            // `perform` is to hand back whatever the tool said, and what a
            // tool says about a failed request is often the request. Before
            // this, `refuse` was the one path out of `use_capability` that ran
            // no scrub — everything else either cannot carry a value
            // (`Response` does not implement it) or goes through `scrub_all`
            // below — so an `Err` raised after the value was read at the line
            // above put the credential in the reply AND in the audit trail.
            // Refusals before that point cannot: there is nothing read yet.
            Err(e) => {
                let reason = scrub_all(&e.to_string(), &[Some(&stored), minted.as_ref()]);
                return refuse(&record, reason, kind_of(&e));
            }
        };
        // Three credentials in play now, not two: the stored one, a minted one,
        // and one this operation may just have CREATED. The third is scrubbed
        // here for the same reason the other two are — a provider that puts a
        // `client_secret` in its own output is not hypothetical, because the
        // natural way to write such an operation is to hand back the reply, and
        // for these operations the reply *is* the secret.
        let mut output = scrub_all(
            &out.output,
            &[
                Some(&stored),
                minted.as_ref(),
                out.created.as_ref().map(|c| &c.value),
            ],
        );

        self.record(AuditLine {
            endpoint: Some(endpoint.clone()),
            exit_code: Some(out.code),
            detail: bound.detail.clone(),
            ..AuditLine::from_record(&audit_id, AuditEvent::Used, peer.uid, peer.pid, &record)
        });

        // §13.10's other half: *"the agent receives handles/capabilities, not
        // plaintext secrets"*. The provider could not have done this itself —
        // `Bind` deliberately gives it no way to reach the store — so the
        // framework is where a created credential is kept, and therefore the
        // one place it cannot be forgotten.
        let mut code = out.code;
        match (bound.creates.as_deref(), out.created) {
            (Some(name), Some(created)) => match self.keep(peer, name, created) {
                Ok(note) => output.push_str(&note),
                // The operation HAPPENED. Saying so plainly matters more than
                // the exit code: somewhere there is now a credential nobody
                // holds, and the only person who can clean that up is the one
                // reading this sentence.
                Err(why) => {
                    code = 1;
                    output.push_str(&format!(
                        "\napex: this operation succeeded and the credential it \
                         created COULD NOT BE STORED as '{name}': {why}. The \
                         provider issues that secret once, so it is now lost — \
                         delete what this created at the provider and run it \
                         again."
                    ));
                }
            },
            // A provider that hands back a credential without having declared
            // it would. Nothing is stored and nothing is returned: the value is
            // dropped here, which loses it, and losing it is the right end for
            // a secret that arrived through a path this service did not check.
            (None, Some(_)) => {
                code = 1;
                output.push_str(
                    "\napex: this provider returned a credential without \
                     declaring that it creates one, so there was no name to \
                     store it under and no check that the name was free. It has \
                     been discarded rather than handed over.",
                );
            }
            (_, None) => {}
        }

        Response::Performed {
            record: Box::new(record),
            endpoint,
            exit_code: code,
            output,
        }
    }

    /// Put a credential a brokered operation just created into the store.
    ///
    /// §13.10, and the narrow reading of it: the secret goes where the owner's
    /// other secrets go, under a name the operation declared before it ran, and
    /// what comes back is that name. The value never enters a [`Response`] —
    /// it cannot, `SecretValue` is not `Serialize` — and it is not returned
    /// here either.
    ///
    /// The host is the pin every future use of this credential will be held to,
    /// which is why a provider that cannot name an honest one must not create a
    /// credential at all.
    fn keep(
        &self,
        peer: Peer,
        name: &str,
        created: provider::Created,
    ) -> Result<String, String> {
        if created.host.is_empty()
            || created.host.len() > 253
            || !created.host.chars().all(valid_host_char)
        {
            return Err(format!(
                "'{}' is not a host name to pin it to",
                created.host.escape_debug()
            ));
        }
        let info = ServiceInfo {
            service: name.to_string(),
            host: created.host.to_ascii_lowercase(),
            scheme: created.scheme.clone(),
            // The half of a service token that is not a secret. Stored beside
            // the half that is, because a `client_id` on its own is useless and
            // a `client_secret` on its own cannot be presented.
            username: created
                .username
                .clone()
                .unwrap_or_else(|| "x-access-token".to_string()),
            path: String::new(),
            auth: "bearer".to_string(),
            port: None,
            added: store::now_ms(),
        };
        // `put` refuses a scheme that is not https outside loopback, so a
        // credential this daemon creates gets the same protection as one the
        // owner typed in.
        self.store
            .put(peer.uid, &info, &created.value)
            .map_err(|e| e.to_string())?;
        self.record(AuditLine::administrative(
            &self.next_audit_id(),
            AuditEvent::Stored,
            peer.uid,
            peer.pid,
            name,
            &format!(
                "credential created by a brokered operation, for {}://{}",
                info.scheme, info.host
            ),
        ));
        Ok(format!(
            "\napex: the credential this created is stored as '{name}', pinned to \
             {}://{}. It is not in this reply and cannot be read back out of \
             this service; `apex secret list` shows that it is there.",
            info.scheme, info.host
        ))
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

/// What `approval_policy` says before [`Service::decide`] has said anything.
///
/// Every line the refusal path writes carries this, so a trail can be read as
/// "these requests were never authorised" rather than as a mix of the daemon's
/// answers and the caller's claims.
const UNDECIDED: &str = "undecided";

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


/// What an `add` was asked to store, before any of it has been judged.
///
/// A struct rather than seven parameters: an endpoint is one thing described
/// seven ways, and a call site that passed `path` where `scheme` goes would
/// compile.
pub struct NewService<'a> {
    pub service: &'a str,
    pub host: &'a str,
    pub scheme: &'a str,
    pub username: Option<&'a str>,
    pub path: &'a str,
    pub auth: Option<&'a str>,
    pub port: Option<u16>,
}

fn valid_host_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':' | '_')
}

/// Whether `*` (every project) is a safe key for this operation.
///
/// **The operation's own declaration answers this**, and nothing here computes
/// it. That is the whole of the fix, and it took two wrong shapes to get here:
///
/// 1. P1-018 gated `--everywhere` on [`OperationSpec::names_nothing`],
///    reasoning that an operation naming nothing "can only ever reach the
///    endpoint pinned when its credential was stored". P1-002 landed the
///    counterexample in the same integration round: `cloudflare.account.read`
///    declares no resource and no parameters and still resolves its account out
///    of the project's own `apex.toml`. A rule over the declaration cannot see
///    what a provider's `bind` reads.
/// 2. The integration fix was `names_nothing() && id == "mcp.request"` — an
///    allow-list. Fail-closed, and **silent**: the next provider to declare an
///    operation like this gets the safe answer, the registry test keeps
///    passing, and nobody is ever asked the question. Safe by accident is not
///    safe by design.
///
/// [`OperationSpec::same_everywhere`] is the question, asked of the one party
/// that can answer it. There is no `Default` for `OperationSpec` and nothing
/// constructs one with `..`, so an operation added later does not compile until
/// its author has stated which of the two it is; and
/// [`apex_secret_core::operation::ProviderSpec::validate`] refuses the
/// incoherent half of the claim at registration, so this can read the field
/// alone.
pub(crate) fn may_be_granted_everywhere(op: &'static OperationSpec) -> bool {
    op.same_everywhere
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
        let store = Store::new(dir.clone());
        let registry =
            crate::providers::default_registry(store.run_dir()).expect("the shipped registry");
        (Service::new(store, false, registry), dir)
    }

    /// The audit trail of a temp service, by path rather than through the
    /// service: the daemon has no reason to expose its own store, and a test
    /// that needs an accessor added for it is a test shaping the API.
    fn trail(dir: &Path) -> PathBuf {
        Store::new(dir.to_path_buf()).audit_path()
    }


    /// The seven fields an `add` takes, for a test that cares about two.
    fn demo_service<'a>(service: &'a str, host: &'a str, scheme: &'a str) -> NewService<'a> {
        NewService {
            service,
            host,
            scheme,
            username: None,
            path: "",
            auth: None,
            port: None,
        }
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
    fn a_grant_already_on_disk_under_an_old_spelling_still_works_and_can_be_removed() {
        // The compatibility claim, against a grants.json the way P0-002 wrote
        // one — written straight to the store rather than through `grant`,
        // which canonicalises on the way in and so could never produce a
        // legacy key. Andre's machine has exactly these.
        let (svc, dir) = temp_service("legacy");
        let peer = me();
        svc.add(peer, demo_service("demo", "github.com", "https"), SecretValue::new(b"x".to_vec()));

        let store = Store::new(dir.clone());
        let mut grants = apex_secret_core::store::Grants::default();
        grants.allow("/tmp/p", "demo", "git-fetch");
        store.save_grants(peer.uid, &grants).expect("save");

        // It matches under the name the request now uses. The operation then
        // fails for its own reasons — /tmp/p is not a repository — and that is
        // the point: it got PAST the grant check.
        let past = svc.use_capability(peer, record("demo", "git.fetch", "origin", "/tmp/p"), Vec::new());
        assert!(
            !past.as_error().is_some_and(|(_, m)| m.contains("not granted")),
            "a grant on disk under the old name stopped matching: {past:?}"
        );

        // The contrast, so the assertion above cannot pass by accident: with
        // nothing granted, the same request is refused for the grant.
        let (empty, empty_dir) = temp_service("legacy-contrast");
        empty.add(peer, demo_service("demo", "github.com", "https"), SecretValue::new(b"x".to_vec()));
        assert!(
            empty
                .use_capability(peer, record("demo", "git.fetch", "origin", "/tmp/p"), Vec::new())
                .as_error()
                .is_some_and(|(_, m)| m.contains("not granted")),
            "the ungranted case must be refused for the grant"
        );
        std::fs::remove_dir_all(&empty_dir).ok();

        // ...and it can be withdrawn, under either spelling. Without this the
        // owner is told the grant is not there while it goes on working.
        let reply = svc.grant(peer, "/tmp/p", "demo", "git.fetch", true);
        assert!(reply.as_error().is_none(), "revoke refused: {reply:?}");
        assert_eq!(
            svc.grants(peer),
            Response::Grants {
                projects: Default::default()
            },
            "the legacy key survived a revoke"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_refusal_never_records_an_approval_the_caller_claimed() {
        // §11's `approval_policy` is the daemon's answer. A request refused
        // before `decide` has no answer, and a trail that carried the caller's
        // claim there would say `owner` on a line nobody authorised.
        let (svc, dir) = temp_service("undecided");
        let peer = me();
        svc.add(peer, demo_service("demo", "github.com", "https"), SecretValue::new(b"x".to_vec()));
        let mut rec = record("demo", "git.fetch", "origin", "/tmp/p");
        rec.approval_policy = "owner".into();
        svc.use_capability(peer, rec, Vec::new());

        let line = audit::tail(&trail(&dir), 10)
            .into_iter()
            .find(|l| l.event == AuditEvent::Refused)
            .expect("a refusal was recorded");
        assert_eq!(line.approval_policy, UNDECIDED, "the caller's claim reached the trail");
        std::fs::remove_dir_all(&dir).ok();
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
            svc.add(peer, demo_service("demo", "github.com", "https"), SecretValue::new(SENTINEL.into())
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
            svc.use_capability(peer, record("demo", "git-fetch", "origin", "/tmp/p"), Vec::new()),
            // ...and one that is refused early.
            svc.use_capability(peer, record("demo", "git-push", "origin", "/tmp/p"), Vec::new()),
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
        svc.add(peer, demo_service("demo", "github.com", "https"), SecretValue::new(b"x".to_vec()));
        assert_eq!(
            svc.grants(peer),
            Response::Grants {
                projects: Default::default()
            }
        );
        let resp = svc.use_capability(peer, record("demo", "git-fetch", "origin", "/tmp/p"), Vec::new());
        let (kind, message) = resp.as_error().expect("refused");
        assert_eq!(kind, ErrorKind::PermissionDenied);
        assert!(message.contains("not granted"), "{message}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_grant_is_per_capability_and_per_project() {
        let (svc, dir) = temp_service("perproject");
        let peer = me();
        svc.add(peer, demo_service("demo", "github.com", "https"), SecretValue::new(b"x".to_vec()));
        svc.grant(peer, "/tmp/p", "demo", "git-fetch", false);

        // Another capability in the same project.
        assert!(svc
            .use_capability(peer, record("demo", "git-push", "origin", "/tmp/p"), Vec::new())
            .as_error()
            .is_some_and(|(_, m)| m.contains("not granted")));
        // The same capability in another project.
        assert!(svc
            .use_capability(peer, record("demo", "git-fetch", "origin", "/tmp/q"), Vec::new())
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
        svc.add(peer, demo_service("demo", "github.com", "https"), SecretValue::new(b"x".to_vec()));
        // `cloudflare.account.delete` is deliberately not a §13.2 name and
        // never will be: this list wants a well-formed id that no provider
        // declares, and it used to hold `cloudflare.dns.delete`, which stopped
        // being one the day P1-007 implemented it. A name taken from the
        // unimplemented end of §13.2 is a trap for whoever implements it next.
        for evil in ["exec", "sh", "git-clone", "curl", "git.clone", "cloudflare.account.delete"] {
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
        svc.add(peer, demo_service("demo", "github.com", "https"), SecretValue::new(b"x".to_vec()));
        let mut rec = record("demo", "git-fetch", "origin", "/tmp/p");
        rec.project = None;
        assert!(svc
            .use_capability(peer, rec, Vec::new())
            .as_error()
            .is_some_and(|(_, m)| m.contains("names no project")));

        // ...and neither can a relative one, which would otherwise be resolved
        // against the DAEMON's working directory.
        let mut rec = record("demo", "git-fetch", "origin", "/tmp/p");
        rec.project = Some("relative/p".into());
        assert!(svc.use_capability(peer, rec, Vec::new()).as_error().is_some());
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
        svc.add(peer, demo_service("demo", "github.com", "https"), SecretValue::new(b"x".to_vec()));
        svc.grant(peer, "/tmp/p", "demo", "git-fetch", false);

        for bad in ["local terminal", "Local-Terminal", "local\nterminal", ""] {
            let mut rec = record("demo", "git-fetch", "origin", "/tmp/p");
            rec.request_origin = bad.to_string();
            let resp = svc.use_capability(peer, rec, Vec::new());
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
            .use_capability(peer, rec, Vec::new())
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
        let resp = svc.use_capability(peer, rec, Vec::new());
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
        svc.add(peer, demo_service("demo", "github.com", "https"), SecretValue::new(b"x".to_vec()));
        let mut rec = record("demo", "git-fetch", "origin", "/tmp/p");
        rec.request_origin = "claude-remote-control".into();
        rec.origin_source = "inherited".into();
        svc.use_capability(peer, rec, Vec::new());

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
        svc.add(peer, demo_service("demo", "github.com", "https"), SecretValue::new(b"x".to_vec()));
        svc.grant(peer, "/tmp/p", "demo", "git-fetch", false);

        let mut rec = record("demo", "git-fetch", "origin", "/tmp/p");
        rec.expiry = Some(1);
        assert!(svc
            .use_capability(peer, rec, Vec::new())
            .as_error()
            .is_some_and(|(_, m)| m.contains("expired")));

        let mut rec = record("demo", "git-fetch", "origin", "/tmp/p");
        rec.expiry = Some(store::now_ms() + MAX_EXPIRY_AHEAD_MS * 2);
        assert!(svc
            .use_capability(peer, rec, Vec::new())
            .as_error()
            .is_some_and(|(_, m)| m.contains("longer than this service")));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn every_refusal_is_recorded_with_a_reason_and_an_id() {
        let (svc, dir) = temp_service("trail");
        let peer = me();
        svc.add(peer, demo_service("demo", "github.com", "https"), SecretValue::new(b"x".to_vec()));
        svc.use_capability(peer, record("demo", "git-fetch", "origin", "/tmp/p"), Vec::new());

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
        svc.add(mine, demo_service("demo", "github.com", "https"), SecretValue::new(SENTINEL.into()));

        assert_eq!(svc.list(theirs), Response::Services { services: vec![] });
        assert!(svc
            .use_capability(theirs, record("demo", "git-fetch", "origin", "/tmp/p"), Vec::new())
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
        svc.add(peer, demo_service("demo", "github.com", "https"), SecretValue::new(b"x".to_vec()));
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
        let resp = svc.add(peer, demo_service("demo", "github.com", "http"), SecretValue::new(b"x".to_vec()),
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
            let resp = svc.add(peer, demo_service("demo", evil, "https"), SecretValue::new(b"x".to_vec()));
            if evil == "-" {
                // A single hyphen is a legal host character; it simply never
                // matches a remote. The framing characters are what matter.
                continue;
            }
            assert!(resp.as_error().is_some(), "'{}' was accepted", evil.escape_debug());
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    // ── §13.10: a credential an operation CREATES ───────────────────────────
    //
    // The seam these three tests exist for is small and easy to get subtly
    // wrong, so each one is written against a provider that behaves the way the
    // careless implementation would: it puts the secret it just issued straight
    // into its own output. A test whose fake provider politely withheld the
    // value could not tell a working scrub from a missing one.
    //
    // Four mutations were run against the arms these cover, one at a time, each
    // restored by copying the pristine file back so that cargo rebuilt rather
    // than measuring a mutant against a stale binary. Every one turns a named
    // test red:
    //
    // * the framework's scrub no longer given the created value, so the
    //   provider's own output hands it over —
    //   `a_credential_an_operation_creates_is_stored_and_never_comes_back`;
    // * `keep` reporting that it stored the credential without storing it —
    //   the same test, which is why that one asserts the stored BYTES and not
    //   merely that a service appeared;
    // * the collision check removed, so a second create destroys the first
    //   token's secret — `a_created_credential_never_overwrites_one_…`;
    // * an undeclared credential reported as a clean run —
    //   `a_provider_that_returns_a_credential_it_never_declared_…`.

    /// What the fake provider issues. Distinctive enough that finding it
    /// anywhere downstream means it travelled.
    const ISSUED: &str = "apex-issued-secret-6b41f0-do-not-leak";

    use apex_secret_core::operation::ProviderSpec;

    /// A provider that creates a credential, the way §13.10's Access service
    /// token and Tunnel credential do.
    struct Creator {
        /// Whether `perform` ran. A refusal that is supposed to happen BEFORE
        /// anything is issued has to be measured, not assumed.
        ran: std::sync::atomic::AtomicBool,
    }

    const CREATOR: ProviderSpec = ProviderSpec {
        id: "creator",
        summary: "issues credentials, so the framework's half of §13.10 can be tested",
        operations: &[
            OperationSpec {
                id: "creator.token.create",
                summary: "issue a token and leave it with this service rather than the caller",
                effect: apex_secret_core::operation::Effect::Write,
                resource: apex_secret_core::operation::ResourceKind::Name,
                params: &[],
                aliases: &[],
                same_everywhere: false,
            },
            OperationSpec {
                id: "creator.token.smuggle",
                summary: "hand back a credential without declaring one, which must not work",
                effect: apex_secret_core::operation::Effect::Write,
                resource: apex_secret_core::operation::ResourceKind::Name,
                params: &[],
                aliases: &[],
                same_everywhere: false,
            },
        ],
    };

    impl provider::Provider for Creator {
        fn spec(&self) -> &'static ProviderSpec {
            &CREATOR
        }

        fn bind(&self, req: &provider::Bind<'_>) -> Result<provider::Bound, provider::ProviderError> {
            Ok(provider::Bound {
                endpoint: provider::Endpoint {
                    scheme: "https".to_string(),
                    host: "api.example.test".to_string(),
                },
                detail: format!("issue a token for {}", req.resource),
                // The declaration the framework checks before anything runs —
                // except for `smuggle`, which deliberately does not make it.
                creates: match req.operation.id {
                    "creator.token.create" => Some(format!("issued-{}", req.resource)),
                    _ => None,
                },
            })
        }

        fn perform(
            &self,
            _req: &provider::Bind<'_>,
            _bound: &provider::Bound,
            _value: &SecretValue,
        ) -> Result<provider::Performed, provider::ProviderError> {
            self.ran.store(true, Ordering::SeqCst);
            Ok(provider::Performed {
                code: 0,
                // What the careless provider does: hands back the reply, and
                // for this operation the reply IS the secret.
                output: format!(r#"{{"client_id":"id-42","client_secret":"{ISSUED}"}}"#),
                created: Some(provider::Created {
                    host: "origin.example.test".to_string(),
                    scheme: "https".to_string(),
                    username: Some("id-42".to_string()),
                    value: SecretValue::new(ISSUED.as_bytes().to_vec()),
                }),
            })
        }
    }

    /// A service serving only [`Creator`], with a credential stored for it and
    /// both its operations granted in `/tmp/p`.
    fn creator_service(tag: &str) -> (Service, PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "apex-secretd-creates-{}-{tag}-{}",
            std::process::id(),
            store::now_ms()
        ));
        std::fs::remove_dir_all(&dir).ok();
        let mut registry = Registry::new();
        registry
            .register(Box::new(Creator {
                ran: std::sync::atomic::AtomicBool::new(false),
            }))
            .expect("register");
        let service = Service::new(Store::new(dir.clone()), false, registry);
        let peer = me();
        service.add(
            peer,
            demo_service("creator", "api.example.test", "https"),
            SecretValue::new(b"a-stored-credential".to_vec()),
        );
        for op in ["creator.token.create", "creator.token.smuggle"] {
            service.grant(peer, "/tmp/p", "creator", op, false);
        }
        (service, dir)
    }

    #[test]
    fn a_credential_an_operation_creates_is_stored_and_never_comes_back() {
        // §13.10: "the agent receives handles/capabilities, not plaintext
        // secrets". Both halves are measured — that the caller did NOT get the
        // secret, and that it IS in the store, byte for byte. A build that
        // scrubbed the output and stored nothing would pass the first half and
        // lose the credential forever, which is worse than not implementing it.
        let (svc, dir) = creator_service("stored");
        let peer = me();
        let reply = svc.use_capability(peer, record("creator", "creator.token.create", "alpha", "/tmp/p"), Vec::new());

        let Response::Performed { output, exit_code, .. } = &reply else {
            panic!("{reply:?}");
        };
        assert_eq!(*exit_code, 0, "{output}");
        assert!(!output.contains(ISSUED), "the secret came back to the caller: {output}");
        assert!(output.contains("stored as 'issued-alpha'"), "{output}");
        assert!(output.contains("https://origin.example.test"), "{output}");

        // It is in the store, it is the real value, and it is pinned to the
        // host the provider named rather than to the one it was created at.
        let store = Store::new(dir.clone());
        let info = store.info(peer.uid, "issued-alpha").expect("stored");
        assert_eq!(info.host, "origin.example.test");
        assert_eq!(info.scheme, "https");
        assert_eq!(info.username, "id-42");
        let kept = store.value(peer.uid, "issued-alpha").expect("value");
        assert_eq!(kept.as_str(), Some(ISSUED), "a different secret was stored");

        // And the trail says a credential was stored, not merely that an
        // operation ran: this is the line an owner greps after an incident.
        let lines = audit::tail(&trail(&dir), 50);
        let stored = lines
            .iter()
            .find(|l| l.event == AuditEvent::Stored && l.provider == "issued-alpha")
            .expect("no `stored` line for the created credential");
        assert!(stored.detail.contains("created by a brokered operation"), "{stored:?}");
        assert!(
            !std::fs::read_to_string(trail(&dir)).unwrap_or_default().contains(ISSUED),
            "the secret is in the audit trail"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_created_credential_never_overwrites_one_that_is_already_stored() {
        // The provider issues such a secret ONCE. So the collision has to be
        // refused before the call, not after it: a refusal afterwards would
        // leave a credential that exists at the provider, is in nobody's hands,
        // and cannot be fetched again. `ran` is what proves the order.
        let (svc, dir) = creator_service("collision");
        let peer = me();
        svc.add(
            peer,
            demo_service("issued-alpha", "somewhere.example.test", "https"),
            SecretValue::new(b"the one that was already there".to_vec()),
        );

        let reply = svc.use_capability(peer, record("creator", "creator.token.create", "alpha", "/tmp/p"), Vec::new());
        let (kind, message) = reply.as_error().expect("a collision must be refused");
        assert_eq!(kind, ErrorKind::BadRequest, "{message}");
        assert!(message.contains("issued-alpha"), "{message}");
        assert!(message.contains("apex secret remove"), "{message}");

        // Nothing ran, so nothing was issued...
        let store = Store::new(dir.clone());
        assert_eq!(
            store.value(peer.uid, "issued-alpha").expect("value").as_str(),
            Some("the one that was already there"),
            "the stored credential was overwritten"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_provider_that_returns_a_credential_it_never_declared_has_it_discarded() {
        // The framework checks the NAME before the call. A provider that
        // produced a value without having declared one would slip past that
        // check, so the value is dropped rather than stored under a name nobody
        // vetted — and dropped rather than returned, which is the whole point.
        let (svc, dir) = creator_service("smuggle");
        let peer = me();
        let reply = svc.use_capability(peer, record("creator", "creator.token.smuggle", "beta", "/tmp/p"), Vec::new());

        let Response::Performed { output, exit_code, .. } = &reply else {
            panic!("{reply:?}");
        };
        assert!(!output.contains(ISSUED), "the smuggled secret came back: {output}");
        assert_eq!(*exit_code, 1, "a discarded credential is not a clean run: {output}");
        assert!(output.contains("discarded"), "{output}");
        assert!(
            Store::new(dir.clone()).info(peer.uid, "issued-beta").is_none(),
            "it was stored under a name nothing checked"
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
