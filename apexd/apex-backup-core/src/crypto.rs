//! Client-side encryption for a snapshot: the sealed box, and the chunk AEAD.
//!
//! P2-001's second criterion is "client-side encryption". This module is the
//! whole of that claim, and `tests/test-apex-backup.sh` is what stops it from
//! being an assertion — it measures the bytes a target actually received and
//! fails if a canary planted in the source appears in any of them.
//!
//! # Why the key is asymmetric
//!
//! The obvious home for a backup key is `apex-secretd`, and it is the wrong
//! one — not as a matter of taste but because of that service's defining
//! invariant. `apex_secret_core::protocol`'s module note says a response may
//! never carry a raw credential, "not as a field, not inside a struct it holds,
//! not base64-encoded in a string"; `SecretValue` does not implement
//! `Serialize` so the compiler refuses the direct form, and
//! `protocol_surface_is_pinned` refuses the indirect ones. Client-side
//! encryption means the process doing the restore holds a key. A `wrap`/
//! `unwrap` verb would be exactly the exception that sentence exists to refuse.
//!
//! So the key has two halves that live in two places:
//!
//! * the **public** half is a [`Recipient`]. Creating a snapshot needs only
//!   this, so an ordinary user — and an agent — can back up.
//! * the **private** half is a [`Identity`], root-owned and 0600. Restoring
//!   needs it, so restoring needs root.
//!
//! That is a stronger answer to P2-002's second criterion than a symmetric key
//! would be. The backup path never touches the private half at all, so there is
//! nothing on that path to leak: an agent that is wholly compromised cannot
//! decrypt even the snapshot it just wrote.
//!
//! It is also why the private half is not in `apex-secretd`'s store even as an
//! opaque value. `Store::put` requires a host and `service.rs` refuses an empty
//! one, because every credential that store holds is a credential *for* some
//! host and the host is the pin every use is held to. A backup key has no
//! honest host. Storing it under a fabricated one would put a service in
//! `apex secret list` and in the grants table that is not a service, which is
//! the same category error `provider::Created`'s note refuses in the other
//! direction: "a provider that cannot name an honest host must not create one
//! at all."
//!
//! # The key schedule
//!
//! One X25519 sealed box per snapshot, and one AEAD key derived from it:
//!
//! ```text
//! esk, epk      = a fresh X25519 keypair, this snapshot's only
//! shared        = X25519(esk, recipient)          refused if !was_contributory
//! K             = SHA-256(SNAPSHOT_KEY_DOMAIN || shared || epk || recipient)
//! ```
//!
//! `epk` travels in the snapshot head in the clear — it is a public key, and
//! without `esk` or the recipient's private half it says nothing. The recipient
//! is mixed into `K` as well as `shared` so that a key derived for one
//! recipient cannot be replayed against another.
//!
//! **The raw X25519 output is never used as a key.** It is a curve point, not a
//! uniformly random string, and feeding it straight to an AEAD is the classic
//! mistake. One SHA-256 over a domain-separated concatenation is the standard
//! one-step KDF and needs no `hkdf`/`hmac` crate, which is why neither is in
//! this crate's dependencies.
//!
//! # The chunk AEAD
//!
//! XChaCha20-Poly1305, and not AES-GCM, for two reasons that are about this
//! program rather than about the primitives. Its 24-byte nonce is large enough
//! to be **random per chunk** rather than a counter, so a resumed or retried
//! upload cannot reuse one — AES-GCM's 12 bytes are not, and a nonce-reuse bug
//! in a backup tool is silent and total. And it is constant-time in software on
//! every machine, where AES without AES-NI is not; APEX ships to hardware this
//! project does not enumerate.
//!
//! Each chunk is `nonce || ciphertext||tag`, and the additional data binds
//! everything an attacker could otherwise rearrange:
//!
//! ```text
//! AAD = CHUNK_AAD_DOMAIN || snapshot id || 0 || stream || 0
//!       || index as 8 big-endian bytes || last as one byte
//! ```
//!
//! So a chunk cannot be moved to another index, moved between the manifest
//! stream and the data stream, moved into another snapshot, or presented as the
//! last chunk when it is not. Truncation is caught twice: the final chunk
//! carries `last = 1`, and the head declares how many there are.
//!
//! # What is not encrypted
//!
//! Exactly one object per snapshot, `head.json`, and it holds the format
//! version, the snapshot id, the creation time, the recipient it was sealed to,
//! the ephemeral public key, the chunk size and the chunk counts. **File names,
//! file sizes, file contents and the directory structure are all inside the
//! encrypted manifest**, which is a stream like any other. Leaving the manifest
//! in the clear is the standard hole in an "encrypted backup" claim and this
//! format does not have it.

use std::fmt;

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};
use sha2::{Digest, Sha256};
use zeroize::{Zeroize, Zeroizing};

/// Domain separator for the one-step KDF over the X25519 output.
const SNAPSHOT_KEY_DOMAIN: &[u8] = b"apex-backup/v1 snapshot-key\0";

/// Domain separator for a chunk's additional data.
const CHUNK_AAD_DOMAIN: &[u8] = b"apex-backup/v1 chunk\0";

/// Domain separator for a recipient string's checksum.
const RECIPIENT_CHECKSUM_DOMAIN: &[u8] = b"apex-backup/v1 recipient\0";

/// The human-facing prefix of a recipient string.
pub const RECIPIENT_PREFIX: &str = "apexbk1";

/// Bytes of checksum carried in a recipient string.
///
/// Four, and they are the difference between a typo being refused and a run of
/// backups nobody can ever open. See [`Recipient::parse`].
const RECIPIENT_CHECKSUM_BYTES: usize = 4;

/// An X25519 public key, and the length of a raw AEAD key.
const KEY_BYTES: usize = 32;

/// XChaCha20-Poly1305's nonce.
pub const NONCE_BYTES: usize = 24;

/// Poly1305's tag.
pub const TAG_BYTES: usize = 16;

/// How many bytes a sealed chunk adds to its plaintext.
pub const CHUNK_OVERHEAD: usize = NONCE_BYTES + TAG_BYTES;

/// Which stream a chunk belongs to.
///
/// Bound into every chunk's additional data, so a manifest chunk presented as a
/// data chunk does not open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stream {
    /// The encrypted file list.
    Manifest,
    /// The encrypted concatenation of file contents.
    Data,
}

impl Stream {
    pub fn as_str(self) -> &'static str {
        match self {
            Stream::Manifest => "manifest",
            Stream::Data => "data",
        }
    }
}

/// Why a cryptographic step refused.
///
/// Every variant is a *conclusion*. Nothing here reports "I could not tell" —
/// that distinction belongs to the caller, which knows whether it failed to
/// read a file or read one that did not open. Keeping it out of this enum is
/// deliberate: a module that conflated the two would make
/// [`crate::verdict::Verdict`] impossible to compute honestly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CryptoError {
    /// The recipient string is not one this build will parse.
    BadRecipient(String),
    /// The recipient string parsed, and its checksum does not match. Almost
    /// always a typo, and the reason a checksum is there at all.
    RecipientChecksum { got: String },
    /// A key or nonce was not the length it has to be.
    BadLength {
        what: &'static str,
        want: usize,
        got: usize,
    },
    /// The X25519 exchange produced an all-zero shared secret, which means the
    /// far public key was a small-order point.
    NotContributory,
    /// The AEAD refused: wrong key, wrong additional data, or altered bytes.
    /// One error and not three, because the primitive cannot tell them apart
    /// and a message that guessed would be a message that lied.
    Unopenable { stream: &'static str, index: u64 },
    /// A sealed chunk is shorter than its own framing.
    Truncated { stream: &'static str, index: u64 },
}

impl fmt::Display for CryptoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CryptoError::BadRecipient(why) => write!(
                f,
                "that is not a backup recipient: {why}. One looks like \
                 `{RECIPIENT_PREFIX}` followed by 48 characters, and \
                 `apex backup key show` prints this machine's"
            ),
            CryptoError::RecipientChecksum { got } => write!(
                f,
                "the recipient '{}' has the right shape and the wrong \
                 checksum, so it is a mistyped key rather than another \
                 machine's. It is refused here, before anything is written, \
                 because a backup sealed to a key nobody holds cannot be \
                 told apart from a good one until the day you need it",
                got.escape_debug()
            ),
            CryptoError::BadLength { what, want, got } => {
                write!(f, "a {what} must be {want} bytes and this one is {got}")
            }
            CryptoError::NotContributory => write!(
                f,
                "the key exchange produced an all-zero shared secret, which \
                 means that recipient is a small-order point and not a real \
                 public key"
            ),
            CryptoError::Unopenable { stream, index } => write!(
                f,
                "chunk {index} of the {stream} stream does not open. Either it \
                 is not sealed to this key, or its bytes changed after it was \
                 written. Authenticated encryption cannot tell those apart, so \
                 this message does not guess"
            ),
            CryptoError::Truncated { stream, index } => write!(
                f,
                "chunk {index} of the {stream} stream is shorter than the \
                 {CHUNK_OVERHEAD} bytes of framing every chunk carries, so it \
                 was cut short in transit or in storage"
            ),
        }
    }
}

impl std::error::Error for CryptoError {}

/// The public half: who a snapshot is sealed to.
#[derive(Clone, PartialEq, Eq)]
pub struct Recipient([u8; KEY_BYTES]);

impl Recipient {
    pub fn from_bytes(bytes: [u8; KEY_BYTES]) -> Recipient {
        Recipient(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; KEY_BYTES] {
        &self.0
    }

    /// Render as the string that goes in `apex.toml`.
    ///
    /// Prefix, then base64url of the key and a four-byte checksum over it. No
    /// padding, so the whole thing is one word with no `=` for a shell or a
    /// TOML parser to argue about.
    pub fn to_string_value(&self) -> String {
        let mut raw = Vec::with_capacity(KEY_BYTES + RECIPIENT_CHECKSUM_BYTES);
        raw.extend_from_slice(&self.0);
        raw.extend_from_slice(&recipient_checksum(&self.0));
        format!(
            "{RECIPIENT_PREFIX}{}",
            data_encoding::BASE64URL_NOPAD.encode(&raw)
        )
    }

    /// Parse one, refusing a typo rather than sealing to a key nobody holds.
    ///
    /// The checksum is the whole point of this function. A backup sealed to a
    /// mistyped public key is indistinguishable from a good one at the moment
    /// it is written — it encrypts, it uploads, it verifies, and it is
    /// unopenable. There is no later check that catches it, because "this does
    /// not open" is exactly what a snapshot for another machine looks like. So
    /// the check has to be here, before the first byte is read.
    pub fn parse(text: &str) -> Result<Recipient, CryptoError> {
        let Some(body) = text.strip_prefix(RECIPIENT_PREFIX) else {
            return Err(CryptoError::BadRecipient(format!(
                "'{}' does not begin with {RECIPIENT_PREFIX}",
                text.escape_debug()
            )));
        };
        let raw = data_encoding::BASE64URL_NOPAD
            .decode(body.as_bytes())
            .map_err(|_| {
                CryptoError::BadRecipient(
                    "the part after the prefix is not unpadded base64url".to_string(),
                )
            })?;
        if raw.len() != KEY_BYTES + RECIPIENT_CHECKSUM_BYTES {
            return Err(CryptoError::BadRecipient(format!(
                "it decodes to {} bytes and a recipient is {}",
                raw.len(),
                KEY_BYTES + RECIPIENT_CHECKSUM_BYTES
            )));
        }
        let mut key = [0u8; KEY_BYTES];
        key.copy_from_slice(&raw[..KEY_BYTES]);
        if recipient_checksum(&key) != raw[KEY_BYTES..] {
            return Err(CryptoError::RecipientChecksum {
                got: text.to_string(),
            });
        }
        Ok(Recipient(key))
    }
}

impl fmt::Debug for Recipient {
    /// The recipient string itself. It is public, and a `Recipient` that
    /// printed as `[u8; 32]` would make every debugging session harder for no
    /// security at all.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Recipient({})", self.to_string_value())
    }
}

fn recipient_checksum(key: &[u8; KEY_BYTES]) -> [u8; RECIPIENT_CHECKSUM_BYTES] {
    let mut h = Sha256::new();
    h.update(RECIPIENT_CHECKSUM_DOMAIN);
    h.update(key);
    let digest = h.finalize();
    let mut out = [0u8; RECIPIENT_CHECKSUM_BYTES];
    out.copy_from_slice(&digest[..RECIPIENT_CHECKSUM_BYTES]);
    out
}

/// The private half: what opens a snapshot.
///
/// Never `Clone`, never `Serialize`, never `Debug`-printed as its bytes, and
/// zeroed when it drops — the same four properties `apex_secret_core::
/// SecretValue` has, for the same reason.
pub struct Identity([u8; KEY_BYTES]);

impl Identity {
    /// Generate one from the kernel's randomness.
    pub fn generate() -> Result<Identity, std::io::Error> {
        let mut bytes = [0u8; KEY_BYTES];
        getrandom::getrandom(&mut bytes).map_err(std::io::Error::other)?;
        Ok(Identity(bytes))
    }

    pub fn from_bytes(bytes: [u8; KEY_BYTES]) -> Identity {
        Identity(bytes)
    }

    /// The bytes, for writing to the root-owned key file and nothing else.
    pub fn expose(&self) -> &[u8; KEY_BYTES] {
        &self.0
    }

    /// The public half this identity opens.
    pub fn recipient(&self) -> Recipient {
        let secret = x25519_dalek::StaticSecret::from(self.0);
        Recipient(*x25519_dalek::PublicKey::from(&secret).as_bytes())
    }
}

impl Drop for Identity {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl fmt::Debug for Identity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // No bytes, and no length either: the length of a key is a fact about
        // the algorithm, so printing it teaches nothing and printing it from a
        // `Debug` that is supposed to reveal nothing invites the next field to
        // be "just" the first four bytes.
        f.write_str("Identity(<private>)")
    }
}

/// A snapshot's AEAD key, derived once and dropped as soon as the snapshot is.
///
/// `Zeroizing` rather than a hand-written `Drop`, because this one really is
/// just an array and the crate that owns the trait does it correctly.
pub struct SnapshotKey {
    key: Zeroizing<[u8; KEY_BYTES]>,
    snapshot: String,
}

impl fmt::Debug for SnapshotKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SnapshotKey(<private>, snapshot: {})", self.snapshot)
    }
}

/// A fresh sealed box for one snapshot.
///
/// Returns the key the chunks are sealed with, and the ephemeral public key
/// that goes in the head so the holder of the private half can derive the same
/// key later.
pub fn seal_to(
    recipient: &Recipient,
    snapshot: &str,
) -> Result<(SnapshotKey, [u8; KEY_BYTES]), CryptoError> {
    let mut esk_bytes = [0u8; KEY_BYTES];
    getrandom::getrandom(&mut esk_bytes)
        .map_err(|_| CryptoError::BadLength { what: "ephemeral secret", want: KEY_BYTES, got: 0 })?;
    let esk = x25519_dalek::StaticSecret::from(esk_bytes);
    esk_bytes.zeroize();
    let epk = *x25519_dalek::PublicKey::from(&esk).as_bytes();
    let shared = esk.diffie_hellman(&x25519_dalek::PublicKey::from(recipient.0));
    if !shared.was_contributory() {
        return Err(CryptoError::NotContributory);
    }
    let key = derive(shared.as_bytes(), &epk, &recipient.0);
    Ok((
        SnapshotKey {
            key,
            snapshot: snapshot.to_string(),
        },
        epk,
    ))
}

/// Re-derive a snapshot's key from the private half and the head's ephemeral
/// public key.
pub fn open_with(
    identity: &Identity,
    ephemeral: &[u8; KEY_BYTES],
    snapshot: &str,
) -> Result<SnapshotKey, CryptoError> {
    let secret = x25519_dalek::StaticSecret::from(*identity.expose());
    let recipient = *x25519_dalek::PublicKey::from(&secret).as_bytes();
    let shared = secret.diffie_hellman(&x25519_dalek::PublicKey::from(*ephemeral));
    if !shared.was_contributory() {
        return Err(CryptoError::NotContributory);
    }
    Ok(SnapshotKey {
        key: derive(shared.as_bytes(), ephemeral, &recipient),
        snapshot: snapshot.to_string(),
    })
}

/// The one-step KDF. See the module note for why this is not the raw X25519
/// output and why it needs no `hkdf` crate.
fn derive(
    shared: &[u8; KEY_BYTES],
    ephemeral: &[u8; KEY_BYTES],
    recipient: &[u8; KEY_BYTES],
) -> Zeroizing<[u8; KEY_BYTES]> {
    let mut h = Sha256::new();
    h.update(SNAPSHOT_KEY_DOMAIN);
    h.update(shared);
    h.update(ephemeral);
    h.update(recipient);
    let digest = h.finalize();
    let mut out = Zeroizing::new([0u8; KEY_BYTES]);
    out.copy_from_slice(&digest);
    out
}

/// What a chunk's additional data binds. See the module note.
fn chunk_aad(snapshot: &str, stream: Stream, index: u64, last: bool) -> Vec<u8> {
    let mut aad = Vec::with_capacity(CHUNK_AAD_DOMAIN.len() + snapshot.len() + 32);
    aad.extend_from_slice(CHUNK_AAD_DOMAIN);
    aad.extend_from_slice(snapshot.as_bytes());
    aad.push(0);
    aad.extend_from_slice(stream.as_str().as_bytes());
    aad.push(0);
    aad.extend_from_slice(&index.to_be_bytes());
    aad.push(u8::from(last));
    aad
}

impl SnapshotKey {
    /// Seal one chunk. The result is `nonce || ciphertext||tag`.
    pub fn seal_chunk(
        &self,
        stream: Stream,
        index: u64,
        last: bool,
        plaintext: &[u8],
    ) -> Result<Vec<u8>, CryptoError> {
        let mut nonce = [0u8; NONCE_BYTES];
        getrandom::getrandom(&mut nonce).map_err(|_| CryptoError::BadLength {
            what: "nonce",
            want: NONCE_BYTES,
            got: 0,
        })?;
        let cipher = XChaCha20Poly1305::new(Key::from_slice(self.key.as_ref()));
        let aad = chunk_aad(&self.snapshot, stream, index, last);
        let sealed = cipher
            .encrypt(
                XNonce::from_slice(&nonce),
                Payload {
                    msg: plaintext,
                    aad: &aad,
                },
            )
            // The AEAD's encrypt side fails only if the plaintext is longer
            // than the primitive can address, which a chunk never is. It is
            // still not an `unwrap`: a panic in a backup tool loses the run.
            .map_err(|_| CryptoError::Unopenable {
                stream: stream.as_str(),
                index,
            })?;
        let mut out = Vec::with_capacity(NONCE_BYTES + sealed.len());
        out.extend_from_slice(&nonce);
        out.extend_from_slice(&sealed);
        Ok(out)
    }

    /// Open one chunk, or say it does not open.
    pub fn open_chunk(
        &self,
        stream: Stream,
        index: u64,
        last: bool,
        sealed: &[u8],
    ) -> Result<Vec<u8>, CryptoError> {
        if sealed.len() < CHUNK_OVERHEAD {
            return Err(CryptoError::Truncated {
                stream: stream.as_str(),
                index,
            });
        }
        let cipher = XChaCha20Poly1305::new(Key::from_slice(self.key.as_ref()));
        let aad = chunk_aad(&self.snapshot, stream, index, last);
        cipher
            .decrypt(
                XNonce::from_slice(&sealed[..NONCE_BYTES]),
                Payload {
                    msg: &sealed[NONCE_BYTES..],
                    aad: &aad,
                },
            )
            .map_err(|_| CryptoError::Unopenable {
                stream: stream.as_str(),
                index,
            })
    }
}

#[cfg(test)]
mod tests;
