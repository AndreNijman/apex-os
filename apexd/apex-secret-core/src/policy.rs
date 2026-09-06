//! Whether a capability may be used, and what this service can honestly decide.
//!
//! # The layering, and where P0-004 fits
//!
//! §3.1 lists six independent permission layers. This service owns exactly one
//! of them — layer 4, the secret/cloud capability layer — and it must not
//! pretend to own the others.
//!
//! What it can decide, because the kernel tells it: **which uid** is asking.
//! `SO_PEERCRED` is filled in at `connect(2)` and cannot be forged. So a grant
//! is keyed on `(owner_uid, secret, operation, resource)` and nothing else.
//!
//! What it cannot decide: which *project* or which *agent session* is asking.
//! `apex-secretd` runs under its own uid; it cannot read another user's
//! `/proc/<pid>/cwd`, and it has no view of the session table — the daemon that
//! forked the sandbox has that. Both fields therefore arrive as
//! [`crate::capability::Claimed`] values, go into the audit record, and are
//! never consulted here. A confined agent should reach this service *through*
//! `apex-agentd`, which knows the session it forked and can narrow by project
//! before relaying; that relay is P0-003's work, not this task's.
//!
//! # What P0-004 is expected to supply
//!
//! [`PolicyInput`] carries `origin` and `root_peer` today so the shape is
//! settled, and both are inert:
//!
//! * `origin` — §7 wants remote-origin requests distinguishable from local
//!   ones. A local socket peer is local by construction, so "this came from
//!   Remote Control" is a statement only the relay can make, and there is no
//!   authenticated relay yet. P0-004 owning the origin dimension would let this
//!   layer refuse a capability for remote-origin requests rather than record
//!   the claim and shrug.
//! * `root_peer` — a peer with uid 0. It changes nothing here, deliberately:
//!   "root/system grants must not implicitly export raw brokered secrets" is a
//!   `security_invariants` line, and the way it is kept is that *no* caller
//!   gets a value, root included. What P0-004 could add is the other half —
//!   whether a root grant was issued to this session at all — so that a root
//!   peer without one is refused instead of served.
//!
//! Neither is edited into `apex-agent-core`; this is the boundary, and it is a
//! struct P0-004 can fill in.

use serde::{Deserialize, Serialize};

use crate::capability::Origin;
use crate::provider::OperationSpec;

/// A standing permission: this owner may run this operation with this
/// credential.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grant {
    pub secret: String,
    pub operation: String,
    /// `None` means any resource the operation accepts. `Some` pins it to one.
    #[serde(default)]
    pub resource: Option<String>,
    pub granted_ms: u64,
    /// Epoch milliseconds after which the grant stops applying (§11 `expiry`).
    #[serde(default)]
    pub expiry_ms: Option<u64>,
}

impl Grant {
    fn covers(&self, secret: &str, operation: &str, resource: Option<&str>, now_ms: u64) -> bool {
        if self.secret != secret || self.operation != operation {
            return false;
        }
        if self.expiry_ms.is_some_and(|e| now_ms >= e) {
            return false;
        }
        match &self.resource {
            // A grant pinned to a resource covers only that resource.
            Some(pinned) => resource == Some(pinned.as_str()),
            // An unpinned grant covers any resource the operation validates.
            None => true,
        }
    }
}

/// One owner's standing grants.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grants {
    #[serde(default)]
    pub grants: Vec<Grant>,
}

impl Grants {
    /// Read an owner's grants. A missing or unparseable file grants nothing:
    /// the failure mode of a permissions file must be "no permissions".
    pub fn load(path: &std::path::Path) -> Grants {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &std::path::Path) -> std::io::Result<()> {
        let json = serde_json::to_vec_pretty(self)
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        crate::paths::write_private(path, &json, 0o600)
    }

    /// Add a grant, replacing an identical one rather than duplicating it.
    pub fn allow(&mut self, grant: Grant) {
        self.revoke(&grant.secret, &grant.operation, grant.resource.as_deref());
        self.grants.push(grant);
        self.grants.sort_by(|a, b| {
            (&a.secret, &a.operation, &a.resource).cmp(&(&b.secret, &b.operation, &b.resource))
        });
    }

    /// Withdraw one. Returns whether anything was withdrawn.
    pub fn revoke(&mut self, secret: &str, operation: &str, resource: Option<&str>) -> bool {
        let before = self.grants.len();
        self.grants.retain(|g| {
            !(g.secret == secret && g.operation == operation && g.resource.as_deref() == resource)
        });
        self.grants.len() != before
    }

    /// Withdraw every grant on one credential, as when it is removed.
    pub fn revoke_secret(&mut self, secret: &str) -> usize {
        let before = self.grants.len();
        self.grants.retain(|g| g.secret != secret);
        before - self.grants.len()
    }
}

/// Everything a decision is allowed to consider.
#[derive(Debug, Clone)]
pub struct PolicyInput<'a> {
    /// From `SO_PEERCRED`. The only identity here that the kernel vouches for.
    pub owner_uid: u32,
    pub secret: &'a str,
    pub operation: &'a str,
    pub resource: Option<&'a str>,
    /// The operation's own specification, for the properties a grant cannot
    /// override.
    pub spec: &'a OperationSpec,
    pub now_ms: u64,
    /// Claimed, not verified. Present so a reader can see it is unused.
    pub claimed_origin: Origin,
    /// Whether the peer is uid 0. Changes nothing; see the module note.
    pub root_peer: bool,
}

/// Why a capability was refused. Each variant is a distinct sentence a user can
/// act on, which is why this is not one `String`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Denial {
    /// No standing grant covers this.
    NotGranted,
    /// A grant exists but has expired.
    Expired,
    /// The operation would hand the caller the credential. Refused for every
    /// caller, root included.
    WouldExport,
}

impl Denial {
    pub fn as_str(&self) -> &'static str {
        match self {
            Denial::NotGranted => "not-granted",
            Denial::Expired => "expired",
            Denial::WouldExport => "would-export",
        }
    }
}

impl std::fmt::Display for Denial {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Denial::NotGranted => write!(
                f,
                "no grant covers this; allow it with `apex capability grant`"
            ),
            Denial::Expired => write!(f, "the grant that covered this has expired"),
            Denial::WouldExport => write!(
                f,
                "this operation would hand back the credential itself, which \
                 this service does not do for anyone — root included"
            ),
        }
    }
}

/// The answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Allow,
    Deny(Denial),
}

impl Decision {
    /// One word for the audit record.
    pub fn as_str(&self) -> String {
        match self {
            Decision::Allow => "allow".to_string(),
            Decision::Deny(d) => format!("deny:{}", d.as_str()),
        }
    }
}

/// Decide.
///
/// The export check is first and unconditional. Everything else is a grant
/// lookup: this layer has nothing else it can honestly weigh.
pub fn decide(grants: &Grants, input: &PolicyInput<'_>) -> Decision {
    if input.spec.exports_value {
        return Decision::Deny(Denial::WouldExport);
    }
    let matching = grants.grants.iter().any(|g| {
        g.covers(
            input.secret,
            input.operation,
            input.resource,
            input.now_ms,
        )
    });
    if matching {
        return Decision::Allow;
    }
    // Distinguish "never granted" from "granted, then expired": the remedy
    // differs and the second is otherwise baffling.
    let expired = grants.grants.iter().any(|g| {
        g.secret == input.secret
            && g.operation == input.operation
            && g.expiry_ms.is_some_and(|e| input.now_ms >= e)
    });
    Decision::Deny(if expired {
        Denial::Expired
    } else {
        Denial::NotGranted
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{self, Method, OperationSpec, PathShape};

    fn spec() -> &'static OperationSpec {
        provider::provider("github").unwrap().operation("whoami").unwrap()
    }

    fn input<'a>(secret: &'a str, operation: &'a str, resource: Option<&'a str>) -> PolicyInput<'a> {
        PolicyInput {
            owner_uid: 1000,
            secret,
            operation,
            resource,
            spec: spec(),
            now_ms: 1_000,
            claimed_origin: Origin::Local,
            root_peer: false,
        }
    }

    fn grant(secret: &str, operation: &str, resource: Option<&str>) -> Grant {
        Grant {
            secret: secret.into(),
            operation: operation.into(),
            resource: resource.map(str::to_string),
            granted_ms: 0,
            expiry_ms: None,
        }
    }

    #[test]
    fn nothing_is_allowed_without_a_grant() {
        let g = Grants::default();
        assert_eq!(
            decide(&g, &input("gh", "whoami", None)),
            Decision::Deny(Denial::NotGranted)
        );
    }

    #[test]
    fn a_grant_covers_its_own_credential_and_operation_and_no_other() {
        let mut g = Grants::default();
        g.allow(grant("gh", "whoami", None));
        assert_eq!(decide(&g, &input("gh", "whoami", None)), Decision::Allow);
        assert!(matches!(
            decide(&g, &input("gh", "repo-metadata", Some("a/b"))),
            Decision::Deny(_)
        ));
        assert!(matches!(
            decide(&g, &input("gh-work", "whoami", None)),
            Decision::Deny(_)
        ));
    }

    #[test]
    fn a_grant_pinned_to_a_resource_covers_only_that_resource() {
        // The narrowing that makes "read one repository" mean one repository.
        let mut g = Grants::default();
        g.allow(grant("gh", "repo-metadata", Some("AndreNijman/apex-os")));
        assert_eq!(
            decide(&g, &input("gh", "repo-metadata", Some("AndreNijman/apex-os"))),
            Decision::Allow
        );
        for other in [Some("AndreNijman/other"), Some("someone/apex-os"), None] {
            assert!(
                matches!(decide(&g, &input("gh", "repo-metadata", other)), Decision::Deny(_)),
                "{other:?}"
            );
        }
    }

    #[test]
    fn an_unpinned_grant_covers_any_resource_the_operation_validates() {
        let mut g = Grants::default();
        g.allow(grant("gh", "repo-metadata", None));
        assert_eq!(
            decide(&g, &input("gh", "repo-metadata", Some("a/b"))),
            Decision::Allow
        );
    }

    #[test]
    fn an_expired_grant_stops_covering_and_says_so() {
        let mut g = Grants::default();
        let mut expiring = grant("gh", "whoami", None);
        expiring.expiry_ms = Some(500);
        g.allow(expiring);
        let mut early = input("gh", "whoami", None);
        early.now_ms = 499;
        assert_eq!(decide(&g, &early), Decision::Allow);
        assert_eq!(
            decide(&g, &input("gh", "whoami", None)),
            Decision::Deny(Denial::Expired),
            "an expired grant must not read as never granted"
        );
    }

    #[test]
    fn an_operation_that_would_export_the_credential_is_refused_for_everyone() {
        // No shipped operation sets this (provider.rs asserts that). This is
        // the other half: if one ever did, it is refused before the grant is
        // consulted, and refused for root too.
        static EXPORTING: OperationSpec = OperationSpec {
            id: "export",
            summary: "hand back the credential",
            method: Method::Get,
            path: PathShape::Fixed("/x"),
            fields: &[],
            exports_value: true,
        };
        let mut g = Grants::default();
        g.allow(grant("gh", "export", None));
        let mut i = input("gh", "export", None);
        i.spec = &EXPORTING;
        assert_eq!(decide(&g, &i), Decision::Deny(Denial::WouldExport));
        i.root_peer = true;
        i.owner_uid = 0;
        assert_eq!(
            decide(&g, &i),
            Decision::Deny(Denial::WouldExport),
            "a root peer must not be a way around this"
        );
    }

    #[test]
    fn root_is_not_privileged_by_this_layer() {
        // Root gets exactly what any other uid gets: its own namespace, and
        // nothing without a grant in it.
        let g = Grants::default();
        let mut i = input("gh", "whoami", None);
        i.root_peer = true;
        i.owner_uid = 0;
        assert!(matches!(decide(&g, &i), Decision::Deny(Denial::NotGranted)));
    }

    #[test]
    fn a_claimed_origin_does_not_change_the_answer() {
        // Recorded, not trusted. If this ever starts mattering, it has to be
        // because something authenticated the claim — which is P0-004's job.
        let mut g = Grants::default();
        g.allow(grant("gh", "whoami", None));
        let mut i = input("gh", "whoami", None);
        assert_eq!(decide(&g, &i), Decision::Allow);
        i.claimed_origin = Origin::Remote;
        assert_eq!(decide(&g, &i), Decision::Allow);
    }

    #[test]
    fn granting_twice_records_once_and_revoking_removes_it() {
        let mut g = Grants::default();
        g.allow(grant("gh", "whoami", None));
        g.allow(grant("gh", "whoami", None));
        assert_eq!(g.grants.len(), 1);
        assert!(g.revoke("gh", "whoami", None));
        assert!(g.grants.is_empty());
        assert!(!g.revoke("gh", "whoami", None));
    }

    #[test]
    fn revoking_a_pinned_grant_leaves_the_unpinned_one_alone() {
        // They are different permissions and must be separately withdrawable.
        let mut g = Grants::default();
        g.allow(grant("gh", "repo-metadata", None));
        g.allow(grant("gh", "repo-metadata", Some("a/b")));
        assert_eq!(g.grants.len(), 2);
        assert!(g.revoke("gh", "repo-metadata", Some("a/b")));
        assert_eq!(g.grants.len(), 1);
        assert!(g.grants[0].resource.is_none());
    }

    #[test]
    fn removing_a_credential_takes_every_grant_on_it() {
        let mut g = Grants::default();
        g.allow(grant("gh", "whoami", None));
        g.allow(grant("gh", "repo-metadata", None));
        g.allow(grant("other", "whoami", None));
        assert_eq!(g.revoke_secret("gh"), 2);
        assert_eq!(g.grants.len(), 1);
        assert_eq!(g.grants[0].secret, "other");
    }

    #[test]
    fn a_missing_or_corrupt_grants_file_grants_nothing() {
        let g = Grants::load(std::path::Path::new("/nonexistent/grants.json"));
        assert!(g.grants.is_empty());

        let dir = std::env::temp_dir().join(format!("apex-secret-pol-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("grants.json");
        std::fs::write(&p, b"{ not json").unwrap();
        assert!(Grants::load(&p).grants.is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn grants_survive_a_save_and_load() {
        let dir = std::env::temp_dir().join(format!("apex-secret-pol2-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        let p = dir.join("grants.json");
        let mut g = Grants::default();
        g.allow(grant("gh", "whoami", None));
        g.save(&p).expect("save");
        assert_eq!(Grants::load(&p), g);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_decision_renders_one_word_for_the_audit_log() {
        assert_eq!(Decision::Allow.as_str(), "allow");
        assert_eq!(
            Decision::Deny(Denial::NotGranted).as_str(),
            "deny:not-granted"
        );
        for d in [Denial::NotGranted, Denial::Expired, Denial::WouldExport] {
            assert!(!d.to_string().is_empty(), "{d:?} has no explanation");
        }
    }
}
