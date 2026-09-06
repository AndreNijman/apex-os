//! The capability record (roadmap §11).
//!
//! §11 names ten fields a generic capability should carry:
//!
//! ```text
//! provider  operation  resource  project  agent_session
//! request_origin  expiry  constraints  approval_policy  audit_id
//! ```
//!
//! All ten are here, and [`CapabilityRecord`] is what the audit log stores. But
//! they are not all worth the same, and pretending otherwise is how an audit
//! trail becomes decoration. Three of them — `project`, `agent_session` and
//! `request_origin` — are **claimed by the caller and cannot be verified by this
//! service**, so they are wrapped in [`Claimed`] and are never an input to a
//! policy decision. See that type for the argument.

use serde::{Deserialize, Serialize};

/// Where a request came from, as the caller describes it (§11, §7).
///
/// Remote-origin requests must be distinguishable from local ones — that is a
/// `security_invariants` entry. This service records the distinction; it cannot
/// yet *establish* it, because a local socket peer is local by construction and
/// "I am relaying something from Remote Control" is a statement only the relay
/// can make. Treated accordingly: recorded, not trusted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Origin {
    #[default]
    Local,
    Remote,
}

impl Origin {
    pub fn as_str(&self) -> &'static str {
        match self {
            Origin::Local => "local",
            Origin::Remote => "remote",
        }
    }
}

/// A field the caller supplied and this service cannot check.
///
/// The wrapper is the point. `project` and `agent_session` come from a peer
/// running under the owner's own uid; a confined agent can send whatever it
/// likes for both. `apex-secretd` runs under a different uid and cannot read
/// another user's `/proc/<pid>/cwd` to verify a project, and it has no view of
/// the session table — that belongs to `apex-agentd`, which forked the sandbox.
///
/// So these fields are audit metadata, not authorisation. Wrapping them makes a
/// policy that reaches for one visible in review: `PolicyInput` takes the
/// claimed values under names that say so, and the grant check never looks at
/// them. Per-project narrowing is `apex-agentd`'s job, one layer up, where the
/// session is knowable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Claimed<T>(pub T);

impl<T> Claimed<T> {
    pub fn get(&self) -> &T {
        &self.0
    }
}

/// Limits carried with a capability (§11 `constraints`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Constraints {
    /// Seconds a provider operation may take before it is killed.
    pub timeout_secs: u64,
    /// Bytes of provider response read before the rest is discarded.
    pub max_response_bytes: usize,
    /// Whether the operation may change state at the provider. Every operation
    /// this service ships is read-only; the field exists so that a write
    /// operation cannot be added without a reviewer seeing it flip.
    pub mutates: bool,
}

impl Default for Constraints {
    fn default() -> Constraints {
        Constraints {
            timeout_secs: 20,
            max_response_bytes: 256 * 1024,
            mutates: false,
        }
    }
}

/// What had to happen before this capability could be used (§11
/// `approval_policy`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum ApprovalPolicy {
    /// A standing grant, made by the owner ahead of time. The only policy
    /// implemented: this service has no way to raise a prompt, and one that
    /// blocked on an unanswered dialog would hang every agent that called it.
    #[default]
    Standing,
    /// Reserved: an interactive confirmation per use, once there is a component
    /// that can ask the owner and time out rather than block.
    PerUse,
}

impl ApprovalPolicy {
    pub fn as_str(&self) -> &'static str {
        match self {
            ApprovalPolicy::Standing => "standing",
            ApprovalPolicy::PerUse => "per-use",
        }
    }
}

/// What a caller asks the broker to do.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityRequest {
    /// The stored credential to use, by name.
    pub secret: String,
    /// One of [`crate::provider::ProviderSpec::operations`].
    pub operation: String,
    /// The thing the operation acts on, when it takes one — a repository, a
    /// zone. Validated against the operation's own shape, never interpolated
    /// into a URL unchecked.
    #[serde(default)]
    pub resource: Option<String>,
    /// Claimed: the project the caller says it is working in.
    #[serde(default)]
    pub project: Option<String>,
    /// Claimed: the agent session the caller says it belongs to.
    #[serde(default)]
    pub agent_session: Option<u32>,
    /// Claimed: local or relayed from Remote Control.
    #[serde(default)]
    pub origin: Origin,
}

/// The full §11 record, minted once per attempt and written to the audit log
/// whether the attempt succeeded, was refused, or failed at the provider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityRecord {
    pub audit_id: String,
    pub provider: String,
    pub operation: String,
    #[serde(default)]
    pub resource: Option<String>,
    /// Unverifiable — see [`Claimed`].
    #[serde(default)]
    pub project: Claimed<Option<String>>,
    /// Unverifiable — see [`Claimed`].
    #[serde(default)]
    pub agent_session: Claimed<Option<u32>>,
    /// Unverifiable — see [`Claimed`].
    pub request_origin: Claimed<Origin>,
    /// When this capability stops being usable, in epoch milliseconds.
    ///
    /// `None` means a standing grant with no expiry, which is what every grant
    /// is today. The field is modelled now because §13.4 wants temporary task
    /// credentials, and a record shape that gains a field later cannot describe
    /// what was granted before it did.
    #[serde(default)]
    pub expiry_ms: Option<u64>,
    pub constraints: Constraints,
    pub approval_policy: ApprovalPolicy,
    /// The uid whose namespace the credential lives in. Verified: it comes from
    /// `SO_PEERCRED`, which the kernel fills in at `connect(2)`.
    pub owner_uid: u32,
}

impl<T: Default> Default for Claimed<T> {
    fn default() -> Claimed<T> {
        Claimed(T::default())
    }
}

impl CapabilityRecord {
    /// Whether this capability is still within its expiry.
    ///
    /// Spelled out rather than `is_none_or`, which is stable since 1.82 and
    /// this workspace's MSRV is 1.75.
    pub fn is_live(&self, now_ms: u64) -> bool {
        match self.expiry_ms {
            None => true,
            Some(expiry) => now_ms < expiry,
        }
    }
}

/// Names of stored credentials: one path segment, no surprises.
///
/// The same rule the agent runtime uses for a service name. It has to hold: the
/// name becomes a filename inside the owner's directory, and a name containing
/// `/` or `..` is a way out of it.
pub fn valid_secret_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
        && !name.starts_with('.')
}

/// Project paths, when one is claimed: absolute, and no newline that would
/// split an audit record into two.
pub fn valid_project_claim(path: &str) -> bool {
    path.starts_with('/') && path.len() <= 4096 && !path.chars().any(|c| c.is_control())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_secret_name_cannot_escape_its_owner_directory() {
        for evil in ["../x", "a/b", ".hidden", "", "x y", "a\nb", "/etc/passwd"] {
            assert!(!valid_secret_name(evil), "{evil:?} must be refused");
        }
        for good in ["github", "cloudflare-work", "my.host", "a_b"] {
            assert!(valid_secret_name(good), "{good:?} must be accepted");
        }
        assert!(!valid_secret_name(&"x".repeat(65)));
    }

    #[test]
    fn a_claimed_project_must_be_absolute_and_single_line() {
        // A newline would end the JSONL record early and let the caller write a
        // second, forged record after it.
        assert!(!valid_project_claim("relative/path"));
        assert!(!valid_project_claim("/p\n{\"event\":\"used\"}"));
        assert!(valid_project_claim("/var/home/andre/Projects/apex"));
    }

    #[test]
    fn the_record_carries_every_field_section_eleven_names() {
        // The acceptance check for §11, written against the serialised form
        // because that is what a reviewer of the audit log actually sees.
        let record = CapabilityRecord {
            audit_id: "0011223344556677".into(),
            provider: "github".into(),
            operation: "whoami".into(),
            resource: None,
            project: Claimed(Some("/p".into())),
            agent_session: Claimed(Some(7)),
            request_origin: Claimed(Origin::Local),
            expiry_ms: None,
            constraints: Constraints::default(),
            approval_policy: ApprovalPolicy::Standing,
            owner_uid: 1000,
        };
        let v: serde_json::Value = serde_json::to_value(&record).unwrap();
        for field in [
            "provider",
            "operation",
            "resource",
            "project",
            "agent_session",
            "request_origin",
            "expiry_ms",
            "constraints",
            "approval_policy",
            "audit_id",
        ] {
            assert!(v.get(field).is_some(), "§11 field {field} is missing");
        }
        // And it survives a round trip, because the audit log is read back.
        let back: CapabilityRecord = serde_json::from_value(v).unwrap();
        assert_eq!(back, record);
    }

    #[test]
    fn a_claimed_field_serialises_as_its_inner_value() {
        // `Claimed` must not change the shape of the log: it is a marker for
        // Rust reviewers, not a wire format.
        let c = Claimed(Some("/p".to_string()));
        assert_eq!(serde_json::to_string(&c).unwrap(), "\"/p\"");
    }

    #[test]
    fn expiry_governs_liveness_and_absent_means_standing() {
        let mut r = CapabilityRecord {
            audit_id: "a".into(),
            provider: "github".into(),
            operation: "whoami".into(),
            resource: None,
            project: Claimed(None),
            agent_session: Claimed(None),
            request_origin: Claimed(Origin::Local),
            expiry_ms: None,
            constraints: Constraints::default(),
            approval_policy: ApprovalPolicy::Standing,
            owner_uid: 0,
        };
        assert!(r.is_live(u64::MAX), "no expiry means always live");
        r.expiry_ms = Some(1_000);
        assert!(r.is_live(999));
        assert!(!r.is_live(1_000), "expiry is exclusive at the boundary");
        assert!(!r.is_live(1_001));
    }

    #[test]
    fn the_default_constraints_are_read_only_and_bounded() {
        let c = Constraints::default();
        assert!(!c.mutates, "no shipped operation may change provider state");
        assert!(c.timeout_secs > 0 && c.timeout_secs <= 60);
        assert!(c.max_response_bytes > 0);
    }
}
