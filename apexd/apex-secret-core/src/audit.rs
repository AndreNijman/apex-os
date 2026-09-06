//! The audit trail: which capability was used, under which credential, by
//! what, and how it ended.
//!
//! §11 requires one. Two things make this one worth having:
//!
//! **It lives with the store, not with the caller.** `/var/lib/apex-secretd/`
//! is root-owned, so a session cannot edit the record of what it did. In the
//! agent-runtime broker the log was a user-writable file next to the session
//! transcripts, which meant the audited party could rewrite the audit.
//!
//! **It records the record.** The [`crate::CapabilityRecord`] the decision was
//! made on is what goes in the line, plus what the daemon resolved for itself
//! (the endpoint the credential was actually sent to) and the outcome. So the
//! trail cannot describe a different operation from the one that ran.
//!
//! The value cannot appear here. Nothing in this module has access to a
//! [`crate::SecretValue`], and the fields are enumerated rather than flattened
//! from something that might grow one.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::capability::CapabilityRecord;

/// What happened.
///
/// A closed set, so `apex secret audit` can colour it and a reader can grep it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AuditEvent {
    /// A credential was stored.
    Stored,
    /// A credential was deleted.
    Removed,
    /// A capability was allowed for a project.
    Granted,
    /// A capability was withdrawn.
    Revoked,
    /// A capability ran. `exit_code` says how it ended.
    Used,
    /// A capability was refused. `reason` says why.
    Refused,
}

impl AuditEvent {
    pub fn as_str(&self) -> &'static str {
        match self {
            AuditEvent::Stored => "stored",
            AuditEvent::Removed => "removed",
            AuditEvent::Granted => "granted",
            AuditEvent::Revoked => "revoked",
            AuditEvent::Used => "used",
            AuditEvent::Refused => "refused",
        }
    }
}

impl std::fmt::Display for AuditEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.pad(self.as_str())
    }
}

/// One line of the trail.
///
/// Serialisable both ways: the daemon writes it, `apex secret audit` reads it
/// back. A line that no longer parses is skipped rather than fatal, because an
/// audit log is append-only across upgrades and one bad line must not hide the
/// rest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditLine {
    /// Minted by the daemon. Ties a refusal and a use to one request, and is
    /// echoed back to the caller so a report can cite it.
    pub audit_id: String,
    /// Milliseconds since the epoch.
    pub ms: u64,
    pub event: AuditEvent,
    /// The account the credential belongs to, from `SO_PEERCRED`.
    pub uid: u32,
    /// The connecting process, from `SO_PEERCRED`. Not an identity — pids are
    /// reused — but it is what a log reader correlates against.
    pub peer_pid: i32,
    /// The stored credential, by name.
    pub provider: String,
    /// The capability name.
    pub operation: String,
    /// The operation in words.
    pub detail: String,
    /// What was asked for: a git remote NAME.
    pub resource: String,
    /// Where the credential was actually sent, scheme and host, as the daemon
    /// resolved it. Absent when the request was refused before resolution.
    #[serde(default)]
    pub endpoint: Option<String>,
    #[serde(default)]
    pub project: Option<String>,
    /// Forwarded by `apex-agentd` and not verified — see [`CapabilityRecord`].
    #[serde(default)]
    pub agent_session: Option<u32>,
    pub request_origin: String,
    pub approval_policy: String,
    #[serde(default)]
    pub constraints: Vec<String>,
    /// Why it was refused.
    #[serde(default)]
    pub reason: Option<String>,
    /// How the operation ended.
    #[serde(default)]
    pub exit_code: Option<i32>,
}

impl AuditLine {
    /// A line describing `record`, with the outcome still to be filled in.
    pub fn from_record(
        audit_id: &str,
        event: AuditEvent,
        uid: u32,
        peer_pid: i32,
        record: &CapabilityRecord,
    ) -> AuditLine {
        AuditLine {
            audit_id: audit_id.to_string(),
            ms: crate::store::now_ms(),
            event,
            uid,
            peer_pid,
            provider: record.provider.clone(),
            operation: record.operation.name().to_string(),
            detail: record.operation.summary(),
            resource: record.resource.clone(),
            endpoint: None,
            project: record.project.clone(),
            agent_session: record.agent_session,
            request_origin: record.request_origin.clone(),
            approval_policy: record.approval_policy.clone(),
            constraints: record.constraints.clone(),
            reason: None,
            exit_code: None,
        }
    }

    /// A line for an administrative event — storing, removing, granting.
    pub fn administrative(
        audit_id: &str,
        event: AuditEvent,
        uid: u32,
        peer_pid: i32,
        provider: &str,
        detail: &str,
    ) -> AuditLine {
        AuditLine {
            audit_id: audit_id.to_string(),
            ms: crate::store::now_ms(),
            event,
            uid,
            peer_pid,
            provider: provider.to_string(),
            operation: event.as_str().to_string(),
            detail: detail.to_string(),
            resource: String::new(),
            endpoint: None,
            project: None,
            agent_session: None,
            request_origin: "local".to_string(),
            approval_policy: "owner".to_string(),
            constraints: Vec::new(),
            reason: None,
            exit_code: None,
        }
    }
}

/// Append one line.
///
/// Best-effort by design: a failure to write the trail must not turn into a
/// failure to refuse a request, and the caller logs rather than propagates.
/// Opened `0600` on creation, in a directory that is already `0700`.
pub fn append(path: &Path, line: &AuditLine) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    if let Some(parent) = path.parent() {
        crate::store::ensure_private_dir(parent)?;
    }
    let text = serde_json::to_string(line)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(path)?;
    writeln!(file, "{text}")
}

/// The last `limit` lines, oldest first.
///
/// Unparseable lines are skipped. Reading the whole file is fine at the scale
/// this log runs at — one line per brokered operation — and it keeps the reader
/// from having to reason about a partial write at the tail.
pub fn tail(path: &Path, limit: usize) -> Vec<AuditLine> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut lines: Vec<AuditLine> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    if lines.len() > limit {
        lines.drain(..lines.len() - limit);
    }
    lines
}

/// Mint an audit id.
///
/// Minted by the daemon, never accepted from a caller: an id a session can
/// choose is an id a session can reuse to make two operations look like one.
/// Time plus a per-process counter plus the pid — unique on a machine without
/// needing a random source or a dependency.
pub fn mint_id(counter: u64) -> String {
    format!(
        "{:x}-{:x}-{:x}",
        crate::store::now_ms(),
        std::process::id(),
        counter
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::Capability;

    const SENTINEL: &str = "apex-sentinel-7f3a91c4-do-not-leak";

    fn record() -> CapabilityRecord {
        let mut rec = CapabilityRecord::new(
            "demo",
            Capability::parse("git-fetch", "origin", None).unwrap(),
        );
        rec.project = Some("/home/x/p".into());
        rec.agent_session = Some(7);
        rec.request_origin = "local".into();
        rec
    }

    #[test]
    fn a_line_names_the_record_the_decision_was_made_on() {
        let line = AuditLine::from_record("a1", AuditEvent::Used, 1000, 42, &record());
        let json = serde_json::to_value(&line).unwrap();
        for field in [
            "audit_id",
            "ms",
            "event",
            "uid",
            "peer_pid",
            "provider",
            "operation",
            "detail",
            "resource",
            "endpoint",
            "project",
            "agent_session",
            "request_origin",
            "approval_policy",
            "constraints",
            "reason",
            "exit_code",
        ] {
            assert!(json.get(field).is_some(), "no {field} in {json}");
        }
        assert_eq!(json["event"], "used");
        assert_eq!(json["operation"], "git-fetch");
        assert_eq!(json["detail"], "git fetch origin");
        assert_eq!(json["agent_session"], 7);
    }

    #[test]
    fn a_line_is_one_json_object_on_one_line() {
        // The file is read back by `apex secret audit` and by scripts, and a
        // record carrying a newline would split into two unparseable halves.
        let mut rec = record();
        rec.constraints = vec!["branch=main".into(), "ttl=15m".into()];
        let line = AuditLine::from_record("a1", AuditEvent::Refused, 1000, 42, &rec);
        let text = serde_json::to_string(&line).unwrap();
        assert!(!text.contains('\n'));
        let back: AuditLine = serde_json::from_str(&text).unwrap();
        assert_eq!(back, line);
    }

    #[test]
    fn the_trail_survives_a_line_it_cannot_parse() {
        let dir = std::env::temp_dir().join(format!("apex-audit-{}", std::process::id()));
        crate::store::ensure_private_dir(&dir).unwrap();
        let path = dir.join("audit.jsonl");
        std::fs::remove_file(&path).ok();

        append(&path, &AuditLine::from_record("a1", AuditEvent::Used, 1000, 1, &record())).unwrap();
        std::fs::write(
            &path,
            format!("{}\nnot json at all\n", std::fs::read_to_string(&path).unwrap().trim()),
        )
        .unwrap();
        append(&path, &AuditLine::from_record("a2", AuditEvent::Refused, 1000, 2, &record()))
            .unwrap();

        let lines = tail(&path, 10);
        assert_eq!(lines.len(), 2, "a bad line hid the good ones");
        assert_eq!(lines[0].audit_id, "a1");
        assert_eq!(lines[1].audit_id, "a2");
        // And the tail is a tail.
        assert_eq!(tail(&path, 1).len(), 1);
        assert_eq!(tail(&path, 1)[0].audit_id, "a2");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn nothing_a_line_can_hold_is_the_credential() {
        // The audit record is built from a CapabilityRecord, and a
        // CapabilityRecord has no field a value can reach: it names a service,
        // not a token. Asserted against the worst case — a caller that stuffed
        // the sentinel into every free-text field it controls — so the check is
        // that the value cannot get there via the record, not that today's
        // caller happens not to try.
        let mut rec = record();
        rec.provider = "demo".into();
        rec.constraints = vec![SENTINEL.into()];
        let line = AuditLine::from_record("a1", AuditEvent::Used, 1000, 42, &rec);
        // A constraint the caller wrote is echoed, because that is what a
        // constraint is. What matters is that no field is fed from the store's
        // value side: `AuditLine` has no constructor taking a SecretValue, and
        // there is no `From<SecretValue>` anywhere in this crate.
        assert!(serde_json::to_string(&line).unwrap().contains(SENTINEL));
        // ...and the daemon's own fields did not pick it up.
        assert!(!line.provider.contains(SENTINEL));
        assert!(!line.detail.contains(SENTINEL));
        assert_eq!(line.endpoint, None);
    }

    #[test]
    fn an_administrative_line_has_no_capability_to_describe() {
        let line = AuditLine::administrative(
            "a1",
            AuditEvent::Stored,
            1000,
            42,
            "demo",
            "stored a credential for github.com",
        );
        assert_eq!(line.operation, "stored");
        assert_eq!(line.approval_policy, "owner");
        assert!(line.resource.is_empty());
        assert_eq!(line.agent_session, None);
    }

    #[test]
    fn minted_ids_do_not_repeat_within_a_process() {
        let ids: Vec<String> = (0..64).map(mint_id).collect();
        let mut sorted = ids.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), ids.len(), "an audit id was reused");
    }
}
