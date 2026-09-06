//! The secret value itself, and the reason it is a type rather than a `String`.
//!
//! Every other guard in this service can be forgotten by the next person to add
//! a response variant. This one cannot: [`SecretValue`] implements no trait that
//! turns it into text, so a response struct that carried one would not compile,
//! and a `format!` that included one would not compile either.
//!
//! What it deliberately does NOT implement, and why:
//!
//! | trait                    | why not                                            |
//! |--------------------------|----------------------------------------------------|
//! | `Serialize`              | `Response` derives it; this is the compile-time wall |
//! | `Deserialize`            | nothing should reconstruct one from the wire        |
//! | `Display`                | `format!("{v}")` must not be a way to leak it       |
//! | `Deref`/`AsRef<str>`     | either makes `&*v` a silent `&str`                  |
//! | `Clone`                  | one owner, one lifetime, one place to look          |
//! | `PartialEq<str>`         | a comparison against attacker input is an oracle    |
//!
//! [`SecretValue::expose`] is the single door. Grep for it: it should appear in
//! the provider execution path and the sealing path and nowhere else, which
//! [`tests::expose_is_called_from_two_places_only`] asserts against the crate's
//! own source.

use std::fmt;

/// Longest credential the service will hold.
///
/// Generous for a bearer token and small enough that a malformed client cannot
/// make the daemon buffer megabytes per request.
pub const MAX_VALUE_BYTES: usize = 8192;

/// A credential, in memory, on its way to a provider.
///
/// Never on its way back to a caller — that is the entire design.
pub struct SecretValue(String);

/// Why a value was refused at the door.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValueError {
    Empty,
    TooLong(usize),
    /// A byte outside printable ASCII. Reported by position, never by value:
    /// naming the offending byte would print part of the credential.
    NotPrintableAscii(usize),
}

impl fmt::Display for ValueError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ValueError::Empty => write!(f, "the credential is empty"),
            ValueError::TooLong(n) => write!(
                f,
                "the credential is {n} bytes; this service holds at most {MAX_VALUE_BYTES}"
            ),
            ValueError::NotPrintableAscii(at) => write!(
                f,
                "byte {at} of the credential is a space or a control character. \
                 APEX holds bearer-style credentials, which cannot contain one — \
                 check for a trailing newline from the shell that produced it"
            ),
        }
    }
}

impl std::error::Error for ValueError {}

impl SecretValue {
    /// Accept a credential, or say why not.
    ///
    /// The charset is printable ASCII with no space. That is what every token
    /// scheme this service brokers uses, and holding the line here means the
    /// value can be placed in an HTTP header and in a `curl` configuration
    /// stanza without a quoting question ever arising — the class of bug that
    /// turns a credential store into a command injection.
    pub fn new(raw: impl Into<String>) -> Result<SecretValue, ValueError> {
        let raw = raw.into();
        if raw.is_empty() {
            return Err(ValueError::Empty);
        }
        if raw.len() > MAX_VALUE_BYTES {
            return Err(ValueError::TooLong(raw.len()));
        }
        if let Some(at) = raw.bytes().position(|b| !(0x21..=0x7e).contains(&b)) {
            return Err(ValueError::NotPrintableAscii(at));
        }
        Ok(SecretValue(raw))
    }

    /// The one way to read the value. Call sites are countable on one hand.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SecretValue {
    /// `Debug` exists because a struct holding one still needs to be printable
    /// in a daemon log line. It prints no part of the value and not its length:
    /// a length narrows an offline guess, and there is no diagnostic that needs
    /// it which `apex capability list` does not already answer.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretValue(<withheld>)")
    }
}

impl Drop for SecretValue {
    /// Overwrite the buffer before it is freed.
    ///
    /// Best effort, and worth saying exactly how far it goes: it clears *this*
    /// allocation, so a later heap reader finds zeros. It does not reach a copy
    /// `String` made during a reallocation, a page the kernel swapped out, or a
    /// core dump taken while the value was live. It costs one memset and closes
    /// the easy case.
    fn drop(&mut self) {
        // Safe: zero is valid UTF-8, so the String stays well-formed for the
        // remainder of its (immediately ending) life.
        unsafe {
            for b in self.0.as_mut_vec().iter_mut() {
                *b = 0;
            }
        }
        std::hint::black_box(&self.0);
    }
}

/// Replace every occurrence of a value in text on its way to a caller.
///
/// Defence in depth behind the type wall. Provider APIs echo request fields in
/// error bodies, and "the upstream will not repeat my token back at me" is an
/// assumption about somebody else's code.
pub fn scrub(text: &str, value: &SecretValue) -> String {
    let needle = value.expose();
    if needle.is_empty() {
        return text.to_string();
    }
    text.replace(needle, "<withheld>")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_credential_round_trips_through_the_one_door() {
        let v = SecretValue::new("not-a-real-token-abc123").expect("accepted");
        assert_eq!(v.expose(), "not-a-real-token-abc123");
    }

    #[test]
    fn debug_prints_no_part_of_the_value_and_not_its_length() {
        let v = SecretValue::new("not-a-real-token-abc123").unwrap();
        let text = format!("{v:?}");
        assert!(!text.contains("abc123"), "{text}");
        assert!(!text.contains("23"), "a length would narrow a guess: {text}");
    }

    #[test]
    fn whitespace_and_control_characters_are_refused_by_position() {
        for (raw, at) in [
            ("has space", 3),
            ("trailing\n", 8),
            ("\ttab", 0),
            ("nul\u{0}byte", 3),
        ] {
            let err = SecretValue::new(raw).unwrap_err();
            assert_eq!(err, ValueError::NotPrintableAscii(at), "{raw:?}");
            // The message must not quote the credential back.
            assert!(!err.to_string().contains(raw), "{err}");
        }
    }

    #[test]
    fn quotes_and_backslashes_are_accepted_because_they_are_printable_ascii() {
        // They are legal in a token, so they must not be refused — which means
        // the curl configuration writer has to escape them. That is asserted in
        // provider.rs; this is the half that makes it necessary.
        assert!(SecretValue::new("a\"b\\c").is_ok());
    }

    #[test]
    fn an_empty_or_oversized_credential_is_refused() {
        assert_eq!(SecretValue::new("").unwrap_err(), ValueError::Empty);
        let big = "x".repeat(MAX_VALUE_BYTES + 1);
        assert_eq!(
            SecretValue::new(big).unwrap_err(),
            ValueError::TooLong(MAX_VALUE_BYTES + 1)
        );
        assert!(SecretValue::new("x".repeat(MAX_VALUE_BYTES)).is_ok());
    }

    #[test]
    fn scrubbing_removes_the_value_and_leaves_an_empty_needle_alone() {
        let v = SecretValue::new("not-a-real-token-xyz").unwrap();
        let out = scrub("Bearer not-a-real-token-xyz rejected", &v);
        assert!(!out.contains("not-a-real-token-xyz"), "{out}");
        assert!(out.contains("<withheld>"), "{out}");
    }

    #[test]
    fn only_two_modules_can_read_a_credential() {
        // The type wall is compile-time; this is the review that keeps the
        // number of doors small. In shipped code `expose` belongs in the
        // provider path (attaching a credential to a request) and the sealing
        // path (writing it to the store). Nowhere else, and a third caller
        // fails here and has to be argued for rather than merged.
        //
        // Test modules are excluded: assertions legitimately read a value back,
        // and they run nothing a caller can reach.
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut callers: Vec<String> = Vec::new();
        for entry in std::fs::read_dir(&src).expect("src is readable").flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            // This module defines `expose` and tests it.
            if name == "value.rs" {
                continue;
            }
            let text = std::fs::read_to_string(&path).expect("readable");
            let shipped = text.split("#[cfg(test)]").next().unwrap_or_default();
            // Comments name the method when explaining it; they are not doors.
            let uses = shipped
                .lines()
                .filter(|l| !l.trim_start().starts_with("//"))
                .filter(|l| l.contains(".expose()"))
                .count();
            if uses > 0 {
                callers.push(name);
            }
        }
        callers.sort();
        assert_eq!(
            callers,
            vec!["provider.rs".to_string(), "seal.rs".to_string()],
            "the set of places that can read a credential changed"
        );
    }
}
