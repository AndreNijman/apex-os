//! The half of the audit trail the audited party cannot edit.
//!
//! §3.4 asks a break-glass grant to leave a "permanent audit record". The
//! runtime's own trail — `privilege-audit.jsonl` under `$XDG_STATE_HOME` — is
//! not that, and cannot be made into it from here. `apex-agentd` runs as the
//! user; a `--sandbox unrestricted` session runs as the user; a break-glass
//! session is unrestricted by definition. Every one of them can rewrite that
//! file. P0-002 met the same wall with the secret store and moved it behind
//! root, which is not available here: `apex-agentd` is unprivileged on purpose
//! (§2), and adding a root path for a log would be a new boundary built for a
//! smaller reason than the one it costs.
//!
//! The journal is the answer that needs no new privilege. `systemd-journald`
//! runs as root and owns its files; any process may send it a record, and no
//! unprivileged process can alter or remove one that is already there. So each
//! grant event is written twice: to the JSONL, which is what `apex agent
//! grants` reads and formats, and to the journal, which is what a human reads
//! when the question is whether the JSONL is telling the truth.
//!
//! ```text
//! journalctl --user -t apex-agentd APEX_GRANT_EVENT=issued
//! ```
//!
//! ## The wire format
//!
//! journald's native protocol is a `SOCK_DGRAM` socket at
//! `/run/systemd/journal/socket` carrying `FIELD=value` lines. A value with a
//! newline in it needs the binary length-prefixed form instead, so every value
//! written here is sanitised to one line — a field that silently truncated at
//! the first newline would be worse than one that shows the escape.
//!
//! Everything is best-effort. A machine with no journald (a container, a test
//! fixture) simply gets no mirror, and the caller is not told: an audit write
//! that could fail a grant would make the logging a denial-of-service on the
//! thing it logs.

use std::os::unix::net::UnixDatagram;
use std::path::Path;

/// journald's native datagram socket.
const SOCKET: &str = "/run/systemd/journal/socket";

/// What every record from this runtime is tagged with, so
/// `journalctl -t apex-agentd` finds all of them.
pub const IDENTIFIER: &str = "apex-agentd";

/// `LOG_NOTICE`. A grant being issued or ending is a normal, significant event
/// — not a warning, and emphatically not debug.
const PRIORITY_NOTICE: &str = "5";

/// Send one structured record.
///
/// `fields` are extra `KEY=value` pairs; keys should be uppercase with
/// underscores, which is journald's convention for a trusted-client field, and
/// APEX's own are prefixed `APEX_`.
pub fn send(message: &str, fields: &[(&str, String)]) {
    let mut record = String::new();
    push_field(&mut record, "MESSAGE", message);
    push_field(&mut record, "PRIORITY", PRIORITY_NOTICE);
    push_field(&mut record, "SYSLOG_IDENTIFIER", IDENTIFIER);
    for (key, value) in fields {
        push_field(&mut record, key, value);
    }
    if !Path::new(SOCKET).exists() {
        return;
    }
    if let Ok(sock) = UnixDatagram::unbound() {
        let _ = sock.send_to(record.as_bytes(), SOCKET);
    }
}

/// One `KEY=value` line, with the value flattened to a single line.
///
/// Split out so the escaping rule is testable without a journald to send to.
fn push_field(out: &mut String, key: &str, value: &str) {
    out.push_str(key);
    out.push('=');
    out.push_str(&one_line(value));
    out.push('\n');
}

/// A value with no newlines or carriage returns.
///
/// Replaced with a visible escape rather than stripped: a message that lost a
/// line without saying so is a message that can be made to read differently
/// from what happened.
fn one_line(value: &str) -> String {
    value.replace('\\', "\\\\").replace('\n', "\\n").replace('\r', "\\r")
}

/// Mirror one system-access grant event.
///
/// Every field an auditor would need to answer "who was granted what, when,
/// for how long, on whose authority, and how did it end" — as journald fields,
/// so `journalctl APEX_GRANT_ID=3` returns the whole life of one grant.
pub fn grant_event(
    event: &str,
    grant: &crate::grant::SystemGrant,
    state: &crate::grant::GrantState,
) {
    let message = format!(
        "{event}: {} grant {} for session {} ({}), {} until {}, authorised by {}",
        grant.kind,
        grant.id,
        grant.session,
        grant.agent,
        state.as_str(),
        grant.expires_ms,
        grant.authenticated_by,
    );
    send(
        &message,
        &[
            ("APEX_GRANT_EVENT", event.to_string()),
            ("APEX_GRANT_ID", grant.id.to_string()),
            ("APEX_GRANT_KIND", grant.kind.as_str().to_string()),
            ("APEX_GRANT_STATE", state.as_str().to_string()),
            ("APEX_SESSION", grant.session.to_string()),
            ("APEX_AGENT", grant.agent.clone()),
            ("APEX_GRANT_ISSUED_MS", grant.issued_ms.to_string()),
            ("APEX_GRANT_EXPIRES_MS", grant.expires_ms.to_string()),
            ("APEX_GRANT_BOOT_ID", grant.boot_id.clone()),
            ("APEX_GRANT_CAPABILITIES", grant.capabilities.join(",")),
            ("APEX_REQUEST_ORIGIN", grant.request_origin.as_str().to_string()),
            ("APEX_GRANT_AUTH", grant.authenticated_by.clone()),
        ],
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_record_is_key_equals_value_lines_and_carries_the_identifier() {
        let mut out = String::new();
        push_field(&mut out, "MESSAGE", "hello");
        push_field(&mut out, "PRIORITY", PRIORITY_NOTICE);
        push_field(&mut out, "SYSLOG_IDENTIFIER", IDENTIFIER);
        assert_eq!(out, "MESSAGE=hello\nPRIORITY=5\nSYSLOG_IDENTIFIER=apex-agentd\n");
    }

    #[test]
    fn a_multiline_value_cannot_forge_a_second_field() {
        // The reason values are escaped rather than passed through. A grant
        // whose agent name contained a newline would otherwise write its own
        // PRIORITY line, and a record that can be shaped by its subject is
        // not an audit record.
        let mut out = String::new();
        push_field(&mut out, "APEX_AGENT", "claude\nPRIORITY=7");
        assert_eq!(out, "APEX_AGENT=claude\\nPRIORITY=7\n");
        assert_eq!(out.lines().count(), 1);
        // Carriage returns and backslashes too, so the escape is unambiguous.
        assert_eq!(one_line("a\r\nb"), "a\\r\\nb");
        assert_eq!(one_line("a\\nb"), "a\\\\nb");
    }

    #[test]
    fn sending_with_no_journald_is_silent_rather_than_fatal() {
        // Every caller is on a path where a grant is being issued or ended.
        // A log that could fail one would be a denial of service on the thing
        // it exists to record.
        send("a message with no journal to reach", &[("APEX_TEST", "1".into())]);
    }
}
