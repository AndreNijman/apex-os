//! The one type that ever holds a credential.
//!
//! Roadmap acceptance for P0-002 says the normal API "cannot return raw secret
//! values". That has to be a property of the types, not a rule in a review
//! checklist, because the API is going to grow: §13 adds a Cloudflare provider,
//! §10 adds MCP header helpers, and each of those is somebody adding a variant
//! to [`crate::protocol::Response`] months from now with no memory of this
//! sentence.
//!
//! So [`SecretValue`] implements neither `Serialize` nor `Deserialize`.
//! `Response` derives `Serialize`. Putting a value in a reply is therefore a
//! compile error, not a review finding — and it stays one for every variant
//! anybody adds later.
//!
//! The rest is damage limitation for the paths that legitimately hold one:
//!
//! * `Debug` prints the length and nothing else, so a value cannot reach a log
//!   through `{:?}` on some enclosing struct;
//! * there is no `Display`, no `Deref`, no `AsRef<str>` — the only way out is
//!   [`SecretValue::expose`], which is named so that `grep -rn expose` lists
//!   every place in the tree that reads one (there are two);
//! * `Drop` overwrites the bytes with volatile writes, so a freed allocation
//!   handed back to the daemon's next request does not still contain a token.

use std::fmt;

/// A credential, in memory.
///
/// Bytes rather than `String`: a token is whatever the provider issued, and
/// `String` would mean validating UTF-8 on a value we have no business
/// inspecting. [`SecretValue::as_str`] is the one place that asks, and it asks
/// because git needs an environment variable.
pub struct SecretValue(Vec<u8>);

impl SecretValue {
    /// Longest credential accepted. Generous — a GitHub fine-grained token is
    /// ~93 bytes and a signed JWT can be a few kilobytes — and bounded, because
    /// the daemon reads this many bytes off a socket on the word of the caller.
    pub const MAX_BYTES: usize = 16 * 1024;

    pub fn new(bytes: Vec<u8>) -> SecretValue {
        SecretValue(bytes)
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Read the credential.
    ///
    /// Deliberately blunt in name. Two call sites in this tree: the store
    /// writing it to disk, and the broker putting it in the environment of a
    /// `git` child. A third one appearing in a diff should be a conversation.
    pub fn expose(&self) -> &[u8] {
        &self.0
    }

    /// The credential as text, when it is text.
    ///
    /// `None` for bytes that are not UTF-8, which cannot be an environment
    /// variable and therefore cannot be used by the git broker at all.
    pub fn as_str(&self) -> Option<&str> {
        std::str::from_utf8(&self.0).ok()
    }
}

/// Length only. A token that reaches a log through a derived `Debug` on some
/// struct three levels up is exactly the accident this prevents.
impl fmt::Debug for SecretValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SecretValue(«redacted», {} bytes)", self.0.len())
    }
}

impl Drop for SecretValue {
    fn drop(&mut self) {
        // Volatile so the optimiser cannot delete a write to memory that is
        // about to be freed, which is precisely what it is allowed to do to a
        // plain `fill(0)`. A fence after it, so the writes are not sunk past
        // the deallocation either.
        for byte in self.0.iter_mut() {
            // Safe: `byte` is a valid, uniquely borrowed, correctly aligned
            // `u8` for the length of this loop.
            unsafe { std::ptr::write_volatile(byte, 0) };
        }
        std::sync::atomic::compiler_fence(std::sync::atomic::Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_prints_the_length_and_never_the_value() {
        let v = SecretValue::new(b"apex-sentinel-do-not-leak".to_vec());
        let text = format!("{v:?}");
        assert!(!text.contains("sentinel"), "{text}");
        assert!(text.contains("redacted"), "{text}");
        assert!(text.contains("25 bytes"), "{text}");
    }

    #[test]
    fn a_value_inside_another_struct_is_still_redacted_by_debug() {
        // The realistic leak: nobody formats the value itself, they format the
        // record that happens to carry it.
        #[derive(Debug)]
        #[allow(dead_code)]
        struct Holder {
            service: &'static str,
            token: SecretValue,
        }
        let text = format!(
            "{:?}",
            Holder {
                service: "demo",
                token: SecretValue::new(b"apex-sentinel-do-not-leak".to_vec()),
            }
        );
        assert!(!text.contains("sentinel"), "{text}");
    }

    #[test]
    fn the_bytes_come_back_exactly() {
        let v = SecretValue::new(vec![0, 159, 146, 150]);
        assert_eq!(v.expose(), &[0, 159, 146, 150]);
        assert_eq!(v.len(), 4);
        // Not UTF-8, so it cannot be an environment variable and the broker
        // must be told that rather than lossily converting it.
        assert_eq!(v.as_str(), None);
    }

    #[test]
    fn text_values_round_trip() {
        let v = SecretValue::new(b"ghp_obviously_fake".to_vec());
        assert_eq!(v.as_str(), Some("ghp_obviously_fake"));
        assert!(!v.is_empty());
        assert!(SecretValue::new(Vec::new()).is_empty());
    }
}
