//! The mutating half: storing, rotating, removing and granting.
//!
//! Reachable only on the `0600` admin socket, which in the packaged
//! configuration means root. That boundary is the answer to a question the
//! broker socket cannot answer: a managed agent runs under the owner's own uid,
//! so if the owner could grant a capability over the broker socket, so could
//! the agent — and a permission an agent can give itself is not a permission.
//!
//! The cost is that `apex capability store` and `apex capability grant` need
//! `sudo`. That is the same shape as `sudo apex ai pull` writing the shared
//! model store, and it is deliberate: adding a credential and widening what an
//! agent may do with it are owner actions, and the owner authenticates.
//!
//! An administrator acts on somebody's namespace with `--owner`, which is why
//! every record here carries both `owner_uid` (whose store changed) and
//! `peer_uid` (who changed it).

use apex_secret_core::audit::{AuditRecord, Event};
use apex_secret_core::capability::valid_secret_name;
use apex_secret_core::paths::Layout;
use apex_secret_core::policy::{Grant, Grants};
use apex_secret_core::protocol::{
    ErrorKind, GrantRequest, Response, RotateRequest, StoreRequest,
};
use apex_secret_core::provider;
use apex_secret_core::store::{self, NewSecret};

use crate::broker::{store_error, write};
use crate::peer::Peer;

/// Which namespace an administrative request acts on.
///
/// `None` means the connecting uid. Under `sudo` that is root, which is almost
/// never what the human meant — so the CLI fills it in from `$SUDO_UID` and
/// this is the fallback rather than the normal path.
fn owner_of(peer: &Peer, requested: Option<u32>) -> u32 {
    requested.unwrap_or(peer.uid)
}

pub fn store(layout: &Layout, peer: &Peer, req: StoreRequest) -> Response {
    let owner = owner_of(peer, req.owner);
    let value = match req.value.into_value() {
        Ok(v) => v,
        Err(e) => return Response::error(ErrorKind::BadRequest, e.to_string()),
    };
    let new = NewSecret {
        name: req.secret,
        provider: req.provider,
        base_url: req.base_url,
        label: req.label,
        sealing: req.sealing,
    };
    match store::store(layout, owner, &new, &value) {
        Ok(meta) => {
            write(
                layout,
                owner,
                AuditRecord {
                    provider: Some(meta.provider.clone()),
                    detail: Some(format!("sealed with {}", meta.sealing.as_str())),
                    ..AuditRecord::administrative(
                        Event::Stored,
                        owner,
                        peer.uid,
                        peer.audit_pid(),
                        Some(meta.name.clone()),
                    )
                },
            );
            Response::Secret(Box::new(meta))
        }
        Err(e) => store_error(e),
    }
}

pub fn rotate(layout: &Layout, peer: &Peer, req: RotateRequest) -> Response {
    let owner = owner_of(peer, req.owner);
    let value = match req.value.into_value() {
        Ok(v) => v,
        Err(e) => return Response::error(ErrorKind::BadRequest, e.to_string()),
    };
    match store::rotate(layout, owner, &req.secret, &value) {
        Ok(meta) => {
            write(
                layout,
                owner,
                AuditRecord {
                    provider: Some(meta.provider.clone()),
                    ..AuditRecord::administrative(
                        Event::Rotated,
                        owner,
                        peer.uid,
                        peer.audit_pid(),
                        Some(meta.name.clone()),
                    )
                },
            );
            Response::Secret(Box::new(meta))
        }
        Err(e) => store_error(e),
    }
}

/// Delete a credential, and every grant that named it.
///
/// The grants go too. A grant that outlived its credential would silently start
/// applying again the moment somebody stored a new one under the same name —
/// which is exactly how a name gets reused after a mistake.
pub fn remove(layout: &Layout, peer: &Peer, requested: Option<u32>, secret: &str) -> Response {
    let owner = owner_of(peer, requested);
    if let Err(e) = store::remove(layout, owner, secret) {
        return store_error(e);
    }
    let path = layout.grants_file(owner);
    let mut grants = Grants::load(&path);
    let dropped = grants.revoke_secret(secret);
    if dropped > 0 {
        if let Err(e) = grants.save(&path) {
            return Response::error(
                ErrorKind::Internal,
                format!("the credential is gone but its grants could not be withdrawn: {e}"),
            );
        }
    }
    write(
        layout,
        owner,
        AuditRecord {
            detail: Some(format!("{dropped} grant(s) withdrawn with it")),
            ..AuditRecord::administrative(
                Event::Removed,
                owner,
                peer.uid,
                peer.audit_pid(),
                Some(secret.to_string()),
            )
        },
    );
    Response::Ok
}

pub fn grant(layout: &Layout, peer: &Peer, req: GrantRequest) -> Response {
    let owner = owner_of(peer, req.owner);
    if !valid_secret_name(&req.secret) {
        return Response::error(
            ErrorKind::BadRequest,
            format!(
                "'{}' is not a credential name",
                req.secret.escape_debug()
            ),
        );
    }

    // A grant on a credential that does not exist is a typo that would silently
    // never match. A grant on an operation the credential's provider does not
    // offer is the same mistake one level down.
    let meta = match store::meta(layout, owner, &req.secret) {
        Ok(m) => m,
        Err(e) => return store_error(e),
    };
    let Some(spec) = provider::provider(&meta.provider) else {
        return Response::error(
            ErrorKind::Internal,
            format!("'{}' is a provider this build no longer knows", meta.provider),
        );
    };
    let Some(op) = spec.operation(&req.operation) else {
        return Response::error(
            ErrorKind::NoSuchOperation,
            provider::ProviderError::UnknownOperation {
                provider: spec.id.to_string(),
                operation: req.operation.clone(),
            }
            .to_string(),
        );
    };
    // Pinning a grant to a resource the operation cannot accept would create a
    // grant that never matches anything.
    if let Some(resource) = &req.resource {
        if let Err(e) = provider::resolve_path(op, Some(resource)) {
            return Response::error(ErrorKind::BadRequest, e.to_string());
        }
    }

    let path = layout.grants_file(owner);
    let mut grants = Grants::load(&path);
    let (event, detail) = if req.revoke {
        if !grants.revoke(&req.secret, &req.operation, req.resource.as_deref()) {
            return Response::error(
                ErrorKind::BadRequest,
                format!(
                    "{}:{} was not granted{}",
                    req.secret,
                    req.operation,
                    match &req.resource {
                        Some(r) => format!(" for {r}"),
                        None => String::new(),
                    }
                ),
            );
        }
        (Event::Revoked, None)
    } else {
        let expiry_ms = req
            .ttl_secs
            .map(|ttl| apex_secret_core::now_ms().saturating_add(ttl.saturating_mul(1000)));
        grants.allow(Grant {
            secret: req.secret.clone(),
            operation: req.operation.clone(),
            resource: req.resource.clone(),
            granted_ms: apex_secret_core::now_ms(),
            expiry_ms,
        });
        (
            Event::Granted,
            req.ttl_secs.map(|t| format!("expires in {t}s")),
        )
    };
    if let Err(e) = grants.save(&path) {
        return Response::error(ErrorKind::Internal, format!("saving the grants: {e}"));
    }

    write(
        layout,
        owner,
        AuditRecord {
            provider: Some(spec.id.to_string()),
            operation: Some(req.operation.clone()),
            resource: req.resource.clone(),
            detail,
            ..AuditRecord::administrative(
                event,
                owner,
                peer.uid,
                peer.audit_pid(),
                Some(req.secret.clone()),
            )
        },
    );
    Response::Grants {
        grants: grants.grants,
    }
}
