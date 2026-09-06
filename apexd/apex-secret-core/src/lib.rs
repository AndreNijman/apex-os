//! The APEX secret service, shared half (roadmap §11, §12, §3.2).
//!
//! §11 asks for "a dedicated protected secret service separate from both
//! `apex-agentd` and broad `apexd`". This crate is the half both the daemon and
//! the `apex` CLI need: the wire protocol, the capability record, the store,
//! the provider vocabulary and the audit log.
//!
//! # The one property everything else serves
//!
//! **No reply this service can produce carries a secret value.** Not to an
//! agent, not to the owner, not to root. The store takes values in and never
//! gives them back; what comes back out is the *result of an operation the
//! service performed on the caller's behalf*.
//!
//! That is enforced in three layers, deliberately overlapping:
//!
//! 1. [`value::SecretValue`] implements neither `Serialize` nor `Display` nor
//!    `Deref`. [`protocol::Response`] derives `Serialize`. A response variant
//!    that carried a value would therefore not compile.
//! 2. [`protocol::payload_kind`] matches exhaustively over every response
//!    variant and classifies it. There is no variant of [`protocol::Payload`]
//!    meaning "a secret". Adding a response variant fails to compile until
//!    somebody classifies it.
//! 3. Everything a provider returns is filtered to an allow-list of named
//!    scalar fields and then scrubbed of the value, because an upstream API
//!    that echoes a credential back is not a hypothetical.
//!
//! # What this does not defend against
//!
//! The daemon runs under its own system uid, so the store is out of reach of a
//! managed agent running as the user. It is not out of reach of root, which can
//! read any file on the machine. Sealing ([`seal`]) raises the cost of an
//! *offline* copy of the disk; it does not put the store beyond a local
//! privileged process. See `docs/secret-service.md`.

pub mod audit;
pub mod capability;
pub mod client;
pub mod paths;
pub mod policy;
pub mod protocol;
pub mod provider;
pub mod seal;
pub mod store;
pub mod value;

use std::time::{SystemTime, UNIX_EPOCH};

/// Milliseconds since the epoch, the timestamp every record here uses.
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// A short random hex id, used for `audit_id`.
///
/// From `/dev/urandom` rather than a counter or the clock: an audit id that can
/// be predicted can be *claimed* by a later record in a log an attacker can
/// append to, and the whole point of the id is to join a request to what the
/// service did about it.
pub fn random_id() -> String {
    use std::io::Read;

    let mut bytes = [0u8; 8];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        if f.read_exact(&mut bytes).is_err() {
            bytes = [0u8; 8];
        }
    }
    if bytes == [0u8; 8] {
        // /dev/urandom is not optional on Linux, but a caller must still get an
        // id rather than a panic. Fall back to something unique-per-process
        // rather than to a constant, which would collide every record.
        let pid = std::process::id() as u64;
        let mix = now_ms() ^ (pid << 32);
        bytes = mix.to_le_bytes();
    }
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_timestamp_is_this_century() {
        // Catches a units mistake: seconds where milliseconds are meant reads
        // as 1970 to every consumer of the audit log.
        let ms = now_ms();
        assert!(ms > 1_700_000_000_000, "{ms} is not milliseconds since 1970");
    }

    #[test]
    fn audit_ids_are_sixteen_hex_characters_and_do_not_repeat() {
        let a = random_id();
        assert_eq!(a.len(), 16, "{a}");
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()), "{a}");
        let mut seen = std::collections::HashSet::new();
        for _ in 0..256 {
            assert!(seen.insert(random_id()), "an audit id repeated");
        }
    }
}
