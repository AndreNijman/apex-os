//! The audit trail (§11: "maintain an audit trail"; §15).
//!
//! One JSON object per line, appended, `0600`, inside the service's own `0700`
//! directory. A file rather than a database because every other piece of state
//! in this workspace is a file, and because a human being able to `tail` the
//! record of what an agent did with their credentials is the feature.
//!
//! # What a record can and cannot be trusted to say
//!
//! `owner_uid`, `peer_uid` and `peer_pid` come from `SO_PEERCRED` — the kernel
//! fills them in at `connect(2)` and no client can choose them. `project`,
//! `agent_session` and `request_origin` come from the caller and are recorded
//! as claims. The distinction is in the field names and in
//! [`crate::capability::Claimed`], because an audit trail that does not say
//! which of its fields the subject chose is an audit trail that reads as more
//! than it is.
//!
//! # What never appears
//!
//! The credential. Not in `detail`, which is scrubbed at the provider boundary;
//! not in an error, because [`crate::value::SecretValue`] cannot be formatted.
//! The wire test greps this file for the sentinel along with everything else.

use serde::{Deserialize, Serialize};

use crate::capability::{CapabilityRecord, Claimed, Origin};
use crate::paths::{self, Layout};

/// What happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Event {
    /// A capability was used: the broker performed the operation.
    Used,
    /// A capability was refused before the credential was read.
    Refused,
    /// The operation was attempted and the provider or the network failed it.
    Failed,
    /// A credential was put in the store.
    Stored,
    /// A credential's value was replaced.
    Rotated,
    /// A credential was deleted.
    Removed,
    Granted,
    Revoked,
}

impl Event {
    pub fn as_str(&self) -> &'static str {
        match self {
            Event::Used => "used",
            Event::Refused => "refused",
            Event::Failed => "failed",
            Event::Stored => "stored",
            Event::Rotated => "rotated",
            Event::Removed => "removed",
            Event::Granted => "granted",
            Event::Revoked => "revoked",
        }
    }
}

/// One line of the audit log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditRecord {
    /// Joins this record to the capability record and to the reply the caller
    /// received, so "what did the thing I ran at 14:03 actually do" is
    /// answerable.
    pub audit_id: String,
    pub ms: u64,
    pub event: Event,
    /// The uid whose namespace was touched. From the kernel.
    pub owner_uid: u32,
    /// The uid that connected. From the kernel. Differs from `owner_uid` only
    /// on the admin socket, where an administrator acts on somebody's store.
    pub peer_uid: u32,
    /// The pid that connected, pinned by start time for the connection's life.
    /// From the kernel, and still only metadata: see `apex-secretd`'s `peer`.
    pub peer_pid: i32,
    #[serde(default)]
    pub secret: Option<String>,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub operation: Option<String>,
    #[serde(default)]
    pub resource: Option<String>,
    /// Claimed by the caller; not verifiable here.
    #[serde(default)]
    pub project: Claimed<Option<String>>,
    /// Claimed by the caller; not verifiable here.
    #[serde(default)]
    pub agent_session: Claimed<Option<u32>>,
    /// Claimed by the caller; not verifiable here.
    #[serde(default)]
    pub request_origin: Claimed<Origin>,
    /// `allow`, or `deny:<reason>`.
    pub decision: String,
    /// The provider's HTTP status, when one was reached.
    #[serde(default)]
    pub status: Option<u16>,
    /// One line of human context. Scrubbed of the credential upstream.
    #[serde(default)]
    pub detail: Option<String>,
}

impl AuditRecord {
    /// A record for an administrative change: storing, rotating, granting.
    ///
    /// No capability record exists for these — nothing was performed — so the
    /// capability fields are absent rather than invented.
    pub fn administrative(
        event: Event,
        owner_uid: u32,
        peer_uid: u32,
        peer_pid: i32,
        secret: Option<String>,
    ) -> AuditRecord {
        AuditRecord {
            audit_id: crate::random_id(),
            ms: crate::now_ms(),
            event,
            owner_uid,
            peer_uid,
            peer_pid,
            secret,
            provider: None,
            operation: None,
            resource: None,
            project: Claimed(None),
            agent_session: Claimed(None),
            request_origin: Claimed(Origin::Local),
            decision: "allow".to_string(),
            status: None,
            detail: None,
        }
    }

    /// A record for a capability attempt, built from the §11 record so the two
    /// cannot drift apart.
    pub fn from_capability(
        event: Event,
        record: &CapabilityRecord,
        secret: &str,
        peer_uid: u32,
        peer_pid: i32,
        decision: String,
    ) -> AuditRecord {
        AuditRecord {
            audit_id: record.audit_id.clone(),
            ms: crate::now_ms(),
            event,
            owner_uid: record.owner_uid,
            peer_uid,
            peer_pid,
            secret: Some(secret.to_string()),
            provider: Some(record.provider.clone()),
            operation: Some(record.operation.clone()),
            resource: record.resource.clone(),
            project: record.project.clone(),
            agent_session: record.agent_session.clone(),
            request_origin: record.request_origin.clone(),
            decision,
            status: None,
            detail: None,
        }
    }
}

/// Append one record.
///
/// Serialised with [`serde_json::to_string`], which escapes every control
/// character — so no field a caller supplied can contain a newline that would
/// split one record into two and let the caller forge the second.
pub fn append(layout: &Layout, uid: u32, record: &AuditRecord) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    let path = layout.audit_log(uid);
    if let Some(parent) = path.parent() {
        paths::ensure_private_dir(parent)?;
    }
    let line = serde_json::to_string(record).map_err(|e| std::io::Error::other(e.to_string()))?;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(&path)?;
    writeln!(file, "{line}")
}

/// The most recent `limit` records, oldest first.
///
/// Reads the whole file. Bounded in practice by the log being one line per
/// capability use, and bounded absolutely by [`MAX_READ_BYTES`] so a log that
/// grew unattended cannot be turned into a memory exhaustion by asking for it.
pub const MAX_READ_BYTES: u64 = 8 * 1024 * 1024;

/// Largest `limit` a caller may ask for.
pub const MAX_LIMIT: usize = 1000;

pub fn tail(layout: &Layout, uid: u32, limit: usize) -> Vec<AuditRecord> {
    use std::io::{Read, Seek, SeekFrom};

    let limit = limit.min(MAX_LIMIT);
    let Ok(mut file) = std::fs::File::open(layout.audit_log(uid)) else {
        return Vec::new();
    };
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    if len > MAX_READ_BYTES {
        // Seek to a bounded window at the end. The first line in it is likely
        // partial and is dropped by the parser.
        let _ = file.seek(SeekFrom::Start(len - MAX_READ_BYTES));
    }
    let mut text = String::new();
    if file.read_to_string(&mut text).is_err() {
        return Vec::new();
    }
    let mut out: Vec<AuditRecord> = text
        .lines()
        .rev()
        .filter_map(|l| serde_json::from_str(l).ok())
        .take(limit)
        .collect();
    out.reverse();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::{ApprovalPolicy, CapabilityRecord, Constraints};

    struct Temp(Layout, std::path::PathBuf);

    impl Drop for Temp {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.1).ok();
        }
    }

    fn temp(tag: &str) -> Temp {
        let base = std::env::temp_dir().join(format!(
            "apex-secret-audit-{tag}-{}",
            std::process::id()
        ));
        std::fs::remove_dir_all(&base).ok();
        Temp(Layout::new(base.join("state"), base.join("run")), base)
    }

    fn capability() -> CapabilityRecord {
        CapabilityRecord {
            audit_id: "00112233aabbccdd".into(),
            provider: "github".into(),
            operation: "whoami".into(),
            resource: None,
            project: Claimed(Some("/p/demo".into())),
            agent_session: Claimed(Some(7)),
            request_origin: Claimed(Origin::Local),
            expiry_ms: None,
            constraints: Constraints::default(),
            approval_policy: ApprovalPolicy::Standing,
            owner_uid: 1000,
        }
    }

    #[test]
    fn a_capability_record_becomes_an_audit_record_with_the_same_id() {
        // The join that makes the trail usable: the id in the reply is the id
        // in the log.
        let cap = capability();
        let rec = AuditRecord::from_capability(
            Event::Used,
            &cap,
            "gh",
            1000,
            4242,
            "allow".to_string(),
        );
        assert_eq!(rec.audit_id, cap.audit_id);
        assert_eq!(rec.provider.as_deref(), Some("github"));
        assert_eq!(rec.operation.as_deref(), Some("whoami"));
        assert_eq!(rec.secret.as_deref(), Some("gh"));
        assert_eq!(rec.owner_uid, 1000);
        assert_eq!(rec.peer_pid, 4242);
        assert_eq!(rec.project.get().as_deref(), Some("/p/demo"));
        assert_eq!(*rec.agent_session.get(), Some(7));
    }

    #[test]
    fn records_append_and_come_back_newest_last() {
        let t = temp("append");
        let l = &t.0;
        for i in 0..5u32 {
            let mut r = AuditRecord::administrative(Event::Stored, 1000, 0, 1, Some("gh".into()));
            r.detail = Some(format!("record {i}"));
            append(l, 1000, &r).expect("append");
        }
        let all = tail(l, 1000, 100);
        assert_eq!(all.len(), 5, "appends must not overwrite");
        assert_eq!(all[0].detail.as_deref(), Some("record 0"));
        assert_eq!(all[4].detail.as_deref(), Some("record 4"));

        let last_two = tail(l, 1000, 2);
        assert_eq!(last_two.len(), 2);
        assert_eq!(last_two[1].detail.as_deref(), Some("record 4"));
    }

    #[test]
    fn the_log_is_not_readable_by_anyone_but_the_service() {
        use std::os::unix::fs::PermissionsExt;

        let t = temp("mode");
        let l = &t.0;
        append(
            l,
            1000,
            &AuditRecord::administrative(Event::Stored, 1000, 0, 1, None),
        )
        .unwrap();
        let mode = std::fs::metadata(l.audit_log(1000)).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "mode was {:o}", mode & 0o777);
    }

    #[test]
    fn a_newline_in_a_claimed_field_cannot_forge_a_second_record() {
        // The attack: a caller claims a project containing a newline and a
        // complete JSON object, so the log gains a record it did not earn.
        // JSON escaping is what stops it; this asserts the escaping is on.
        let t = temp("forge");
        let l = &t.0;
        let mut cap = capability();
        cap.project = Claimed(Some(
            "/p\n{\"event\":\"used\",\"decision\":\"allow\"}".to_string(),
        ));
        let rec = AuditRecord::from_capability(
            Event::Refused,
            &cap,
            "gh",
            1000,
            1,
            "deny:not-granted".to_string(),
        );
        append(l, 1000, &rec).expect("append");

        let raw = std::fs::read_to_string(l.audit_log(1000)).unwrap();
        assert_eq!(raw.lines().count(), 1, "the record was split: {raw}");
        let back = tail(l, 1000, 10);
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].decision, "deny:not-granted");
    }

    #[test]
    fn one_owners_log_is_not_anothers() {
        let t = temp("owners");
        let l = &t.0;
        append(
            l,
            1000,
            &AuditRecord::administrative(Event::Stored, 1000, 0, 1, Some("gh".into())),
        )
        .unwrap();
        assert_eq!(tail(l, 1000, 10).len(), 1);
        assert!(tail(l, 1001, 10).is_empty());
    }

    #[test]
    fn a_missing_log_reads_as_empty_rather_than_failing() {
        let t = temp("missing");
        assert!(tail(&t.0, 1000, 10).is_empty());
    }

    #[test]
    fn a_partial_or_corrupt_line_is_skipped_and_the_rest_survives() {
        let t = temp("partial");
        let l = &t.0;
        append(
            l,
            1000,
            &AuditRecord::administrative(Event::Stored, 1000, 0, 1, Some("gh".into())),
        )
        .unwrap();
        // A torn write, which is what a crash mid-append leaves.
        {
            use std::io::Write;
            let mut f = std::fs::OpenOptions::new()
                .append(true)
                .open(l.audit_log(1000))
                .unwrap();
            writeln!(f, "{{\"audit_id\":\"tr").unwrap();
        }
        append(
            l,
            1000,
            &AuditRecord::administrative(Event::Removed, 1000, 0, 1, Some("gh".into())),
        )
        .unwrap();
        let all = tail(l, 1000, 10);
        assert_eq!(all.len(), 2);
        assert_eq!(all[1].event, Event::Removed);
    }

    #[test]
    fn a_limit_beyond_the_cap_is_clamped() {
        let t = temp("limit");
        assert!(tail(&t.0, 1000, usize::MAX).len() <= MAX_LIMIT);
    }

    #[test]
    fn every_event_has_a_distinct_name() {
        let names: Vec<&str> = [
            Event::Used,
            Event::Refused,
            Event::Failed,
            Event::Stored,
            Event::Rotated,
            Event::Removed,
            Event::Granted,
            Event::Revoked,
        ]
        .iter()
        .map(Event::as_str)
        .collect();
        let mut sorted = names.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), names.len(), "two events share a name");
    }
}
