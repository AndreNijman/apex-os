//! Performing a capability: the one path in this service that reads a value.
//!
//! The order of the checks below is the security argument, and it is the same
//! shape as the agent runtime's broker:
//!
//! 1. the owner comes from `SO_PEERCRED`, never from the request;
//! 2. the provider and operation are looked up in a closed table;
//! 3. the resource is validated against the operation's own shape;
//! 4. the grant is checked;
//! 5. **only then** is the credential read;
//! 6. the operation runs, and what comes back is filtered and scrubbed.
//!
//! Every step before five can refuse, and the credential is not unsealed until
//! all of them pass — so no refusal path can leak it, because on a refusal path
//! it was never in memory.

use apex_secret_core::audit::{self, AuditRecord, Event};
use apex_secret_core::capability::{
    ApprovalPolicy, CapabilityRecord, CapabilityRequest, Claimed, Constraints,
};
use apex_secret_core::paths::Layout;
use apex_secret_core::policy::{self, Decision, Grants, PolicyInput};
use apex_secret_core::protocol::{ErrorKind, Performed, Response};
use apex_secret_core::provider::{self, ProviderError};
use apex_secret_core::store::{self, StoreError};
use apex_secret_core::{capability, random_id};

use crate::peer::Peer;

/// Perform one capability for `peer`.
pub fn use_capability(
    layout: &Layout,
    peer: &Peer,
    request: &CapabilityRequest,
) -> Response {
    // 1. The owner. From the kernel, at connect time. The request has no field
    //    that could name a different one.
    let owner = peer.uid;

    if !capability::valid_secret_name(&request.secret) {
        return Response::error(
            ErrorKind::BadRequest,
            format!(
                "'{}' is not a credential name; use letters, digits, _ - and .",
                request.secret.escape_debug()
            ),
        );
    }
    if let Some(project) = &request.project {
        if !capability::valid_project_claim(project) {
            return Response::error(
                ErrorKind::BadRequest,
                "a project must be an absolute path on one line".to_string(),
            );
        }
    }

    // The metadata says which provider's vocabulary applies. Read before the
    // value, and it cannot carry one.
    let meta = match store::meta(layout, owner, &request.secret) {
        Ok(m) => m,
        Err(e) => return store_error(e),
    };

    // 2. Provider and operation, from the table.
    let Some(spec) = provider::provider(&meta.provider) else {
        return Response::error(
            ErrorKind::Internal,
            format!(
                "'{}' is stored against a provider this build no longer knows",
                meta.provider.escape_debug()
            ),
        );
    };
    let Some(op) = spec.operation(&request.operation) else {
        return Response::error(
            ErrorKind::NoSuchOperation,
            ProviderError::UnknownOperation {
                provider: spec.id.to_string(),
                operation: request.operation.clone(),
            }
            .to_string(),
        );
    };

    // The §11 record. Minted here so that every outcome below — refusal,
    // provider failure, success — is audited against the same id, and the id is
    // what the caller gets back.
    let record = CapabilityRecord {
        audit_id: random_id(),
        provider: spec.id.to_string(),
        operation: op.id.to_string(),
        resource: request.resource.clone(),
        project: Claimed(request.project.clone()),
        agent_session: Claimed(request.agent_session),
        request_origin: Claimed(request.origin),
        expiry_ms: None,
        constraints: Constraints::default(),
        approval_policy: ApprovalPolicy::Standing,
        owner_uid: owner,
    };
    let pid = peer.audit_pid();

    // 3. The resource, against the operation's shape. Before the grant check so
    //    that a malformed resource cannot be recorded as a policy refusal.
    if let Err(e) = provider::resolve_path(op, request.resource.as_deref()) {
        write(
            layout,
            owner,
            AuditRecord {
                detail: Some(e.to_string()),
                ..AuditRecord::from_capability(
                    Event::Refused,
                    &record,
                    &request.secret,
                    peer.uid,
                    pid,
                    "deny:bad-resource".to_string(),
                )
            },
        );
        return Response::error(ErrorKind::BadRequest, e.to_string());
    }

    // 4. The grant.
    let grants = Grants::load(&layout.grants_file(owner));
    let decision = policy::decide(
        &grants,
        &PolicyInput {
            owner_uid: owner,
            secret: &request.secret,
            operation: &request.operation,
            resource: request.resource.as_deref(),
            spec: op,
            now_ms: apex_secret_core::now_ms(),
            claimed_origin: request.origin,
            root_peer: peer.uid == 0,
        },
    );
    if let Decision::Deny(denial) = &decision {
        write(
            layout,
            owner,
            AuditRecord::from_capability(
                Event::Refused,
                &record,
                &request.secret,
                peer.uid,
                pid,
                decision.as_str(),
            ),
        );
        return Response::error(ErrorKind::PermissionDenied, denial.to_string());
    }

    let Some(base) = meta.base() else {
        return Response::error(
            ErrorKind::Internal,
            "this credential names no endpoint and its provider has no default",
        );
    };
    let base = base.to_string();

    // 5. Every check has passed. Only now is the credential unsealed.
    let value = match store::open(layout, owner, &request.secret) {
        Ok((_, v)) => v,
        Err(e) => return store_error(e),
    };

    // 6. Perform it.
    let outcome = provider::perform(
        spec,
        op,
        &base,
        request.resource.as_deref(),
        &value,
        &record.constraints,
    );
    drop(value);

    match outcome {
        Ok(outcome) => {
            write(
                layout,
                owner,
                AuditRecord {
                    status: Some(outcome.status),
                    detail: outcome.detail.clone(),
                    ..AuditRecord::from_capability(
                        if outcome.ok() { Event::Used } else { Event::Failed },
                        &record,
                        &request.secret,
                        peer.uid,
                        pid,
                        decision.as_str(),
                    )
                },
            );
            Response::Performed(Box::new(Performed {
                audit_id: record.audit_id,
                provider: record.provider,
                operation: record.operation,
                resource: record.resource,
                outcome,
            }))
        }
        Err(e) => {
            // The provider was not reached. The message is curl's, already
            // scrubbed at the provider boundary.
            let detail = e.to_string();
            write(
                layout,
                owner,
                AuditRecord {
                    detail: Some(detail.clone()),
                    ..AuditRecord::from_capability(
                        Event::Failed,
                        &record,
                        &request.secret,
                        peer.uid,
                        pid,
                        decision.as_str(),
                    )
                },
            );
            Response::error(ErrorKind::ProviderFailed, detail)
        }
    }
}

/// Append a record, reporting a failure to the daemon's own log rather than to
/// the caller.
///
/// An audit write that fails must not be invisible, and must not be the
/// caller's problem to interpret — they asked for an operation, not for a log
/// entry.
pub fn write(layout: &Layout, owner: u32, record: AuditRecord) {
    if let Err(e) = audit::append(layout, owner, &record) {
        eprintln!(
            "apex-secretd: could not write the audit record for {}: {e}",
            record.audit_id
        );
    }
}

pub fn store_error(e: StoreError) -> Response {
    let kind = match e {
        StoreError::NoSuchSecret(_) => ErrorKind::NoSuchSecret,
        StoreError::BadName(_) | StoreError::BadBaseUrl(_) | StoreError::Exists(_) => {
            ErrorKind::BadRequest
        }
        StoreError::UnknownProvider(_) => ErrorKind::NoSuchOperation,
        StoreError::Seal(_) | StoreError::Io(_) => ErrorKind::Internal,
    };
    Response::error(kind, e.to_string())
}
