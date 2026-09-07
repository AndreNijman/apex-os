//! Proving a security key was touched, before a remote origin may elevate.
//!
//! §7 treats Remote Control as a normal workflow and then draws one line
//! through it: "Root capability: local approval required. Unsafe everything:
//! local approval required." [`crate::policy::OriginPolicy`]'s opt-in,
//! `RemoteElevationAllowed`, is the owner saying that line may be crossed —
//! and the roadmap allows that only behind a WebAuthn/FIDO2 security key.
//! Until this module existed, `AgentPolicy::validate` refused the setting
//! outright, because turning it on would have dropped the local-approval rule
//! and put nothing in its place.
//!
//! What is here is the verification half: given a credential the owner
//! enrolled, a challenge this daemon issued, and an assertion a security key
//! produced, decide whether a human touched that key for *this* request. It
//! is the whole security value of the feature, and it is a pure function of
//! its inputs apart from one subprocess.
//!
//! ## Why the CTAP2 level and not the WebAuthn level
//!
//! There is no browser here. The operator runs `fido2-assert` against a key
//! plugged into whatever machine they are sitting at, and pastes four lines
//! back. So the client data is APEX's own challenge rather than a
//! browser-built `clientDataJSON`, and the signed message is CTAP2's:
//!
//! ```text
//! signature = ES256( authenticatorData || SHA-256(clientData) )
//! ```
//!
//! `fido2-assert` takes the client data hash on its first input line by
//! default and the raw client data with `-w`; either way the hash it signs
//! over is `SHA-256(challenge)`, which is what [`Challenge::client_data_hash`]
//! computes. That symmetry is why the challenge is a byte string this module
//! defines rather than a JSON document: both sides derive the same 32 bytes
//! from it with no canonicalisation question to get wrong.
//!
//! ## No new crates, on purpose
//!
//! `apex-agent-core` depends on `anyhow`, `serde`, `serde_json` and `libc`,
//! and this module adds none. ECDSA verification goes to the `openssl` CLI,
//! which the image ships and which every CI runner already has; SHA-256,
//! base64 and the COSE key parse are here, because they have to be pure
//! functions the tests can drive without a subprocess — the rp-id-hash
//! comparison is a security check and it must be asserted directly.
//!
//! The one primitive that must not be hand-written is the signature check
//! itself, and it is not: [`verify_signature`] hands the key, the signature
//! and the message to `openssl pkeyutl`.
//!
//! ## The 26-byte prefix
//!
//! A CTAP2 authenticator reports an ES256 public key as a COSE_Key map, and
//! `openssl` wants a SubjectPublicKeyInfo. For P-256 with an uncompressed
//! point the whole of the difference is a constant: the same 26 bytes of DER
//! precede `0x04 || X || Y` in every such SPKI, because every field in them —
//! the two OIDs, the two lengths, the unused-bits byte — is fixed by the curve
//! and the encoding. So the conversion is a concatenation
//! ([`PublicKey::to_spki_der`]), with no DER writer to get wrong, and the
//! inverse is a prefix check ([`PublicKey::from_spki_der`]), which is what
//! reads back what `fido2-cred -V` printed.
//!
//! ## What the test vectors are, and why they are not APEX's
//!
//! A verifier tested against signatures it generated itself has proved that it
//! agrees with itself. Every positive vector in this module's tests was
//! produced by a real authenticator and is copied from a third party's test
//! suite:
//!
//! * **libfido2 `regress/assert.c`** — Yubico's own regression vector. Its
//!   `flags` byte is `0x00`: it was taken with `up=false`, a silent assertion.
//!   That makes it the one case a fabricated vector cannot produce — a
//!   cryptographically valid assertion with **no user presence** — and it is
//!   used here as the negative for [`AssertionError::NoUserPresence`].
//! * **go-webauthn `testAssertionSpecVectorNoneES256`** — `flags = 0x19`,
//!   counter 0, rp `example.org`.
//! * **go-webauthn's macOS TouchID capture** — `flags = 0x45`, counter
//!   1553097241, and 187 bytes of authenticator data because the `AT` bit is
//!   set and the credential's own key is embedded in it.
//!
//! That last one settles a design question: an assertion's authenticator data
//! is **not** always 37 bytes. This parser requires at least 37 and ignores
//! what follows, rather than rejecting it. Trailing bytes are inside the
//! signed message, so an attacker cannot add, remove or alter them without
//! invalidating the signature — there is nothing to defend by refusing them,
//! and refusing them would lock out a platform authenticator that a real user
//! really has.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::grant::GrantKind;
use crate::paths;

// ---------------------------------------------------------------------------
// SHA-256
// ---------------------------------------------------------------------------

const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

/// SHA-256 of `data`, FIPS 180-4.
///
/// Written out rather than pulled in. The two places it is used are both
/// security checks over short inputs — the rp-id hash an assertion must match
/// and the client data hash a key signs over — and both have to be assertable
/// in a unit test without a subprocess or a workspace change. It is pinned to
/// the published vectors in this module's tests, including the two message
/// lengths where a padding bug hides (55 and 56 bytes modulo the block), and
/// to `sha256("localhost")`, which libfido2's regress vector independently
/// contains as an rp-id hash.
pub fn sha256(data: &[u8]) -> [u8; 32] {
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];

    let bit_len = (data.len() as u64).wrapping_mul(8);
    let mut padded = Vec::with_capacity(data.len() + 72);
    padded.extend_from_slice(data);
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&bit_len.to_be_bytes());

    for block in padded.chunks_exact(64) {
        let mut w = [0u32; 64];
        for (i, word) in block.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }

        let (mut a, mut b, mut c, mut d) = (h[0], h[1], h[2], h[3]);
        let (mut e, mut f, mut g, mut hh) = (h[4], h[5], h[6], h[7]);
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let t1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }

    let mut out = [0u8; 32];
    for (i, word) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

// ---------------------------------------------------------------------------
// base64
// ---------------------------------------------------------------------------

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard base64 with padding — the alphabet `fido2-cred` and
/// `fido2-assert` print and the one PEM uses.
pub fn b64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(B64[(n >> 18) as usize & 63] as char);
        out.push(B64[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { B64[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { B64[n as usize & 63] as char } else { '=' });
    }
    out
}

/// The inverse, tolerating whitespace and both the standard and the URL-safe
/// alphabet.
///
/// URL-safe is accepted because a value copied out of a browser's developer
/// tools or a WebAuthn library's fixture arrives that way, and rejecting it
/// would be a paste that fails for a reason nobody can see. `-` and `_` cannot
/// collide with `+` and `/`: no encoder emits both.
pub fn b64_decode(text: &str) -> Option<Vec<u8>> {
    let mut acc: u32 = 0;
    let mut bits = 0u32;
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    for ch in text.chars() {
        if ch.is_whitespace() || ch == '=' {
            continue;
        }
        let v = match ch {
            'A'..='Z' => ch as u32 - 'A' as u32,
            'a'..='z' => ch as u32 - 'a' as u32 + 26,
            '0'..='9' => ch as u32 - '0' as u32 + 52,
            '+' | '-' => 62,
            '/' | '_' => 63,
            _ => return None,
        };
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    // Leftover bits must be zero padding, never data: `QUJD?` would otherwise
    // decode to the same bytes as `QUJD`, and two spellings of one credential
    // id is a comparison waiting to go wrong.
    if bits > 0 && (acc & ((1 << bits) - 1)) != 0 {
        return None;
    }
    Some(out)
}

// ---------------------------------------------------------------------------
// The public key
// ---------------------------------------------------------------------------

/// The DER that precedes `0x04 || X || Y` in every P-256 SubjectPublicKeyInfo.
///
/// `SEQUENCE(91) { SEQUENCE(19) { OID id-ecPublicKey, OID prime256v1 },
/// BIT STRING(66, 0 unused) }`. Every length in it is fixed by the curve, so
/// this is a constant rather than something a DER writer produces.
pub const P256_SPKI_PREFIX: [u8; 26] = [
    0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06, 0x08, 0x2a,
    0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07, 0x03, 0x42, 0x00,
];

/// An ES256 (P-256 / SHA-256) public key, held as the uncompressed point.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PublicKey {
    point: [u8; 64],
}

/// Why a key could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyError {
    /// Not a COSE_Key map this build understands.
    NotCose(&'static str),
    /// A COSE_Key for an algorithm or curve that is not ES256 over P-256.
    UnsupportedAlgorithm { alg: i64, crv: i64 },
    /// Not a P-256 SubjectPublicKeyInfo with an uncompressed point.
    NotP256Spki,
    /// A PEM block that is not `PUBLIC KEY`, or that will not base64-decode.
    NotPem,
}

impl std::fmt::Display for KeyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KeyError::NotCose(why) => write!(f, "not a CTAP2 ES256 public key: {why}"),
            KeyError::UnsupportedAlgorithm { alg, crv } => write!(
                f,
                "this build verifies ES256 over P-256 only (COSE alg -7, crv 1); this key is \
                 alg {alg}, crv {crv}. Enrol an ES256 credential: `fido2-cred -M -t es256`"
            ),
            KeyError::NotP256Spki => write!(
                f,
                "not a P-256 public key in the shape `fido2-cred -V` prints: expected a 91-byte \
                 SubjectPublicKeyInfo carrying an uncompressed point"
            ),
            KeyError::NotPem => write!(f, "no `-----BEGIN PUBLIC KEY-----` block that decodes"),
        }
    }
}

impl std::error::Error for KeyError {}

impl PublicKey {
    /// The uncompressed point, `X || Y`, 64 bytes.
    pub fn point(&self) -> &[u8; 64] {
        &self.point
    }

    /// Build from `X` and `Y` directly.
    pub fn from_xy(x: &[u8; 32], y: &[u8; 32]) -> PublicKey {
        let mut point = [0u8; 64];
        point[..32].copy_from_slice(x);
        point[32..].copy_from_slice(y);
        PublicKey { point }
    }

    /// Parse a CTAP2 COSE_Key map.
    ///
    /// Deliberately not a general CBOR reader. A CTAP2 authenticator emits
    /// exactly one shape for an ES256 key — a five-entry map with canonical
    /// key ordering, `kty=2, alg=-7, crv=1, x=bstr(32), y=bstr(32)` — and the
    /// entries are located by scanning for those two-byte `bstr(32)` headers
    /// rather than by decoding the map, because a CBOR decoder written for
    /// this one job is more code to be wrong in than the job is worth. Every
    /// field that matters is checked: a map with the wrong `kty`, `alg` or
    /// `crv` is refused by name rather than parsed for its coordinates.
    pub fn from_cose(cose: &[u8]) -> Result<PublicKey, KeyError> {
        if cose.len() < 77 {
            return Err(KeyError::NotCose("too short to be a COSE_Key map"));
        }
        // `a5` — a five-entry map. CTAP2 requires exactly these five.
        if cose[0] != 0xa5 {
            return Err(KeyError::NotCose("expected a five-entry CBOR map"));
        }
        let kty = cose_small_int(cose, 0x01).ok_or(KeyError::NotCose("no kty"))?;
        let alg = cose_small_int(cose, 0x03).ok_or(KeyError::NotCose("no alg"))?;
        let crv = cose_small_int(cose, 0x20).ok_or(KeyError::NotCose("no crv"))?;
        if kty != 2 {
            return Err(KeyError::NotCose("kty is not EC2"));
        }
        if alg != -7 || crv != 1 {
            return Err(KeyError::UnsupportedAlgorithm { alg, crv });
        }
        let x = cose_bstr32(cose, 0x21).ok_or(KeyError::NotCose("no x coordinate"))?;
        let y = cose_bstr32(cose, 0x22).ok_or(KeyError::NotCose("no y coordinate"))?;
        Ok(PublicKey::from_xy(&x, &y))
    }

    /// The SubjectPublicKeyInfo `openssl` wants: the constant, then the point.
    pub fn to_spki_der(&self) -> Vec<u8> {
        let mut der = Vec::with_capacity(91);
        der.extend_from_slice(&P256_SPKI_PREFIX);
        der.push(0x04);
        der.extend_from_slice(&self.point);
        der
    }

    /// The inverse of [`PublicKey::to_spki_der`].
    pub fn from_spki_der(der: &[u8]) -> Result<PublicKey, KeyError> {
        if der.len() != 91 || der[..26] != P256_SPKI_PREFIX || der[26] != 0x04 {
            return Err(KeyError::NotP256Spki);
        }
        let mut point = [0u8; 64];
        point.copy_from_slice(&der[27..]);
        Ok(PublicKey { point })
    }

    /// Read what `fido2-cred -V` printed after the credential id.
    pub fn from_spki_pem(pem: &str) -> Result<PublicKey, KeyError> {
        const BEGIN: &str = "-----BEGIN PUBLIC KEY-----";
        const END: &str = "-----END PUBLIC KEY-----";
        let start = pem.find(BEGIN).ok_or(KeyError::NotPem)? + BEGIN.len();
        let end = pem[start..].find(END).ok_or(KeyError::NotPem)? + start;
        let der = b64_decode(&pem[start..end]).ok_or(KeyError::NotPem)?;
        PublicKey::from_spki_der(&der)
    }

    /// The PEM form, for handing to a tool that wants one.
    pub fn to_spki_pem(&self) -> String {
        let body = b64_encode(&self.to_spki_der());
        let mut out = String::from("-----BEGIN PUBLIC KEY-----\n");
        for line in body.as_bytes().chunks(64) {
            out.push_str(std::str::from_utf8(line).expect("base64 is ASCII"));
            out.push('\n');
        }
        out.push_str("-----END PUBLIC KEY-----\n");
        out
    }
}

/// A CBOR small integer stored under a one-byte COSE label.
///
/// Labels 1, 3 and -1 (`0x20`) are all one byte; the values CTAP2 uses for
/// them are `2`, `-7` and `1`, all of which are one byte too.
fn cose_small_int(cose: &[u8], label: u8) -> Option<i64> {
    let i = cose.iter().position(|b| *b == label)?;
    let v = *cose.get(i + 1)?;
    match v {
        0x00..=0x17 => Some(i64::from(v)),
        0x20..=0x37 => Some(-1 - i64::from(v - 0x20)),
        _ => None,
    }
}

/// A 32-byte CBOR byte string stored under a one-byte COSE label.
fn cose_bstr32(cose: &[u8], label: u8) -> Option<[u8; 32]> {
    let mut from = 0usize;
    while let Some(rel) = cose[from..].iter().position(|b| *b == label) {
        let i = from + rel;
        if cose.get(i + 1) == Some(&0x58) && cose.get(i + 2) == Some(&0x20) {
            let body = cose.get(i + 3..i + 35)?;
            let mut out = [0u8; 32];
            out.copy_from_slice(body);
            return Some(out);
        }
        from = i + 1;
    }
    None
}

// ---------------------------------------------------------------------------
// Authenticator data
// ---------------------------------------------------------------------------

/// The flag bits of an authenticator data block, WebAuthn §6.1.
pub mod flags {
    /// A human touched the key.
    pub const UP: u8 = 0x01;
    /// A human proved who they are as well — PIN or biometric.
    pub const UV: u8 = 0x04;
    /// Attested credential data follows the fixed 37 bytes.
    pub const AT: u8 = 0x40;
    /// Extension output follows.
    pub const ED: u8 = 0x80;
}

/// The fixed head of an authenticator data block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthData {
    /// SHA-256 of the relying party id the key signed for.
    pub rp_id_hash: [u8; 32],
    pub flags: u8,
    pub counter: u32,
    /// The raw block, head and everything after it. This, not the head, is
    /// what the signature covers.
    pub raw: Vec<u8>,
}

impl AuthData {
    /// Parse `fido2-assert`'s third line, with or without its CBOR wrapper.
    ///
    /// `fido2-assert` prints `fido_assert_authdata_ptr`, which is the CBOR
    /// byte string — `0x58 <len>` and then the block — not the block itself.
    /// A caller who took the raw bytes from somewhere else (a WebAuthn
    /// `response.authenticatorData`, say) has no wrapper. Both are accepted,
    /// and the wrapper is only stripped when its declared length accounts for
    /// exactly the rest of the input, so a block that happens to begin with
    /// `0x58` cannot be mistaken for a wrapped one.
    pub fn parse(bytes: &[u8]) -> Result<AuthData, AssertionError> {
        let body = unwrap_cbor_bstr(bytes);
        if body.len() < 37 {
            return Err(AssertionError::AuthDataTooShort(body.len()));
        }
        let mut rp_id_hash = [0u8; 32];
        rp_id_hash.copy_from_slice(&body[..32]);
        Ok(AuthData {
            rp_id_hash,
            flags: body[32],
            counter: u32::from_be_bytes([body[33], body[34], body[35], body[36]]),
            raw: body.to_vec(),
        })
    }

    pub fn user_present(&self) -> bool {
        self.flags & flags::UP != 0
    }

    pub fn user_verified(&self) -> bool {
        self.flags & flags::UV != 0
    }
}

/// Strip a CBOR byte-string header if, and only if, one accounts for the input.
fn unwrap_cbor_bstr(bytes: &[u8]) -> &[u8] {
    match bytes.first() {
        // 0x58: byte string, one length byte follows.
        Some(0x58) if bytes.len() >= 2 && bytes.len() == 2 + bytes[1] as usize => &bytes[2..],
        // 0x59: byte string, two length bytes follow. A platform authenticator
        // with a large extension output reaches this.
        Some(0x59)
            if bytes.len() >= 3
                && bytes.len() == 3 + u16::from_be_bytes([bytes[1], bytes[2]]) as usize =>
        {
            &bytes[3..]
        }
        _ => bytes,
    }
}

// ---------------------------------------------------------------------------
// The challenge
// ---------------------------------------------------------------------------

/// How long an unanswered challenge stays good for.
///
/// Long enough to walk to the machine the key is plugged into and touch it,
/// short enough that a challenge left on a screen is not a standing permit.
pub const CHALLENGE_TTL_MS: u64 = 120_000;

/// How many challenges may be outstanding at once.
///
/// A remote origin can ask for a challenge without proving anything, so
/// without a ceiling it could ask forever and the daemon would remember every
/// one. Issuing past the ceiling evicts the oldest rather than refusing: a
/// refusal here would be a denial of service that any unauthenticated caller
/// could inflict on the owner.
pub const MAX_OUTSTANDING_CHALLENGES: usize = 8;

/// The domain separator that starts every challenge.
///
/// Present so that these bytes can never be mistaken for, or replayed as, some
/// other thing APEX asks a key to sign. The version is in it because the
/// binding below is a wire format between this daemon and a future one.
pub const CHALLENGE_CONTEXT: &str = "apex-agent/remote-elevation/v1";

/// One outstanding request for a touch.
///
/// Bound to the session, the grant being asked for and the ttl being asked
/// for, so that an assertion is proof of consent to *this* elevation and not
/// to elevation in general. A challenge that named only a nonce would let a
/// touch collected for a two-minute `system-access` grant be replayed against
/// an eight-hour break-glass one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Challenge {
    /// The session the elevation is for.
    pub session: u32,
    /// Which of the two grants §4.4 and §4.5 name.
    pub kind: GrantKind,
    /// The ttl being asked for, in milliseconds.
    pub grant_ttl_ms: u64,
    /// 32 bytes from the kernel, base64. What makes this challenge this one.
    pub nonce: String,
    /// When it was issued, and when it stops being good for anything.
    pub issued_ms: u64,
    pub expires_ms: u64,
}

impl Challenge {
    /// A challenge with a caller-supplied nonce. Tests use it; the daemon uses
    /// [`ChallengeStore::issue`], which draws the nonce from the kernel.
    pub fn with_nonce(
        session: u32,
        kind: GrantKind,
        grant_ttl_ms: u64,
        now_ms: u64,
        nonce: &[u8],
    ) -> Challenge {
        Challenge {
            session,
            kind,
            grant_ttl_ms,
            nonce: b64_encode(nonce),
            issued_ms: now_ms,
            expires_ms: now_ms.saturating_add(CHALLENGE_TTL_MS),
        }
    }

    /// The bytes a key is asked to sign over — the client data itself.
    ///
    /// One line per field, in a fixed order, with a context string first.
    /// Newline-separated rather than length-prefixed because every field here
    /// renders without one: three integers, one word from a closed set, and
    /// base64. Nothing in it can contain a newline, so no two different
    /// challenges can produce the same bytes.
    pub fn binding(&self) -> Vec<u8> {
        format!(
            "{CHALLENGE_CONTEXT}\nsession={}\nkind={}\nttl_ms={}\nissued_ms={}\nnonce={}\n",
            self.session,
            self.kind.as_str(),
            self.grant_ttl_ms,
            self.issued_ms,
            self.nonce,
        )
        .into_bytes()
    }

    /// `SHA-256(clientData)` — what the authenticator appends to the
    /// authenticator data before signing, and what `fido2-assert` wants on its
    /// first input line.
    pub fn client_data_hash(&self) -> [u8; 32] {
        sha256(&self.binding())
    }

    pub fn expired_at(&self, now_ms: u64) -> bool {
        now_ms >= self.expires_ms
    }

    /// The four lines to feed `fido2-assert -G -w`, minus the client data,
    /// which is [`Challenge::binding`].
    ///
    /// Printed for the operator rather than executed: the key is plugged into
    /// whatever machine the human is at, which by construction is not this
    /// one.
    pub fn instructions(&self, credential_id: &[u8], rp_id: &str) -> String {
        format!(
            "printf '%s' \"$APEX_CHALLENGE\" | base64 -w0 > cd.b64\n\
             {{ cat cd.b64; echo; echo '{rp_id}'; echo '{}'; }} \\\n  \
             | fido2-assert -G -w -p -i /dev/stdin /dev/hidraw0\n",
            b64_encode(credential_id),
        )
    }
}

/// The challenges this daemon has issued and not yet seen answered.
///
/// In memory only, and deliberately: a challenge that survived a restart would
/// be a touch request outliving the process that asked for it, and the daemon
/// restarting is exactly when a stale one should stop being good.
#[derive(Debug, Default)]
pub struct ChallengeStore {
    outstanding: Vec<Challenge>,
}

impl ChallengeStore {
    pub fn new() -> ChallengeStore {
        ChallengeStore::default()
    }

    /// Issue one, with a nonce from the kernel.
    pub fn issue(&mut self, session: u32, kind: GrantKind, grant_ttl_ms: u64, now_ms: u64) -> Challenge {
        self.issue_with_nonce(session, kind, grant_ttl_ms, now_ms, &random_nonce())
    }

    /// [`ChallengeStore::issue`] with the nonce supplied, so a test can name it.
    pub fn issue_with_nonce(
        &mut self,
        session: u32,
        kind: GrantKind,
        grant_ttl_ms: u64,
        now_ms: u64,
        nonce: &[u8],
    ) -> Challenge {
        self.expire(now_ms);
        while self.outstanding.len() >= MAX_OUTSTANDING_CHALLENGES {
            self.outstanding.remove(0);
        }
        let challenge = Challenge::with_nonce(session, kind, grant_ttl_ms, now_ms, nonce);
        self.outstanding.push(challenge.clone());
        challenge
    }

    /// Take a challenge back out, by nonce, once.
    ///
    /// Removal happens whether or not the assertion that follows verifies. A
    /// challenge that survived a failed attempt would let an attacker grind
    /// against one nonce; one attempt per issue is the whole point of a nonce.
    pub fn redeem(&mut self, nonce: &str, now_ms: u64) -> Option<Challenge> {
        self.expire(now_ms);
        let at = self.outstanding.iter().position(|c| c.nonce == nonce)?;
        Some(self.outstanding.remove(at))
    }

    /// Drop everything past its window.
    pub fn expire(&mut self, now_ms: u64) {
        self.outstanding.retain(|c| !c.expired_at(now_ms));
    }

    pub fn outstanding(&self) -> usize {
        self.outstanding.len()
    }
}

/// 32 bytes from `/dev/urandom`.
///
/// The kernel, not a PRNG seeded here. If the device cannot be read the nonce
/// falls back to the clock and the address of a heap allocation, which is a
/// worse nonce — but the alternative is refusing to issue a challenge because
/// `/dev/urandom` is missing, and on a machine where that is true the daemon
/// has larger problems than this one.
fn random_nonce() -> [u8; 32] {
    let mut nonce = [0u8; 32];
    // `read_exact` on an open handle, never `std::fs::read`: that reads to
    // EOF, and `/dev/urandom` has none.
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        use std::io::Read;
        if f.read_exact(&mut nonce).is_ok() {
            return nonce;
        }
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let boxed = Box::new(0u8);
    let addr = (&*boxed as *const u8) as usize as u128;
    let mix = sha256(&format!("{now}:{addr}").into_bytes());
    nonce.copy_from_slice(&mix);
    nonce
}

// ---------------------------------------------------------------------------
// The credential
// ---------------------------------------------------------------------------

/// A security key the owner enrolled.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Credential {
    /// What the owner calls it. For the listing, and for `remove`.
    pub label: String,
    /// The credential id the authenticator issued, base64.
    pub id: String,
    /// The public key as a PEM SubjectPublicKeyInfo — the second half of what
    /// `fido2-cred -V` printed, stored as printed so an operator can compare
    /// the file with the paste.
    pub public_key_pem: String,
    /// The relying party id the credential was made for.
    pub rp_id: String,
    /// The highest signature counter seen from this credential.
    #[serde(default)]
    pub counter: u32,
    /// When it was enrolled, milliseconds since the epoch.
    #[serde(default)]
    pub enrolled_ms: u64,
}

impl Credential {
    pub fn id_bytes(&self) -> Option<Vec<u8>> {
        b64_decode(&self.id)
    }

    pub fn key(&self) -> Result<PublicKey, KeyError> {
        PublicKey::from_spki_pem(&self.public_key_pem)
    }

    /// Read what `fido2-cred -V` printed: the credential id in base64 on the
    /// first line, then a PEM public key.
    pub fn parse_fido2_cred(
        label: &str,
        rp_id: &str,
        text: &str,
        now_ms: u64,
    ) -> Result<Credential, EnrolError> {
        let first = text
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .ok_or(EnrolError::Empty)?;
        if first.starts_with("-----BEGIN") {
            return Err(EnrolError::NoCredentialId);
        }
        let id = b64_decode(first).ok_or(EnrolError::NoCredentialId)?;
        if id.is_empty() {
            return Err(EnrolError::NoCredentialId);
        }
        let key = PublicKey::from_spki_pem(text).map_err(EnrolError::Key)?;
        Ok(Credential {
            label: label.to_string(),
            id: b64_encode(&id),
            public_key_pem: key.to_spki_pem(),
            rp_id: rp_id.to_string(),
            counter: 0,
            enrolled_ms: now_ms,
        })
    }
}

/// Why an enrolment paste could not be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnrolError {
    Empty,
    NoCredentialId,
    Key(KeyError),
    /// A label already in the store. Silently replacing one would let a paste
    /// retire a key its owner still believes is enrolled.
    DuplicateLabel(String),
    Io(String),
}

impl std::fmt::Display for EnrolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EnrolError::Empty => write!(f, "nothing to enrol"),
            EnrolError::NoCredentialId => write!(
                f,
                "the first line must be the credential id in base64, as `fido2-cred -V` prints \
                 it above the public key"
            ),
            EnrolError::Key(e) => write!(f, "{e}"),
            EnrolError::DuplicateLabel(l) => write!(
                f,
                "a security key called {l:?} is already enrolled; remove it first, or choose \
                 another name"
            ),
            EnrolError::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for EnrolError {}

/// Every security key the owner enrolled.
///
/// A file rather than the config, because the config is normalised, rewritten
/// and merged with unknown keys on every save, and a credential store is the
/// one thing in the runtime whose emptiness is a security decision: an empty
/// store is what makes `--origin-policy remote` refuse to start.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CredentialStore {
    #[serde(default)]
    pub credentials: Vec<Credential>,
}

/// Where the store lives.
pub fn store_path() -> PathBuf {
    paths::state_home().join("apex/agent/webauthn.json")
}

impl CredentialStore {
    pub fn is_empty(&self) -> bool {
        self.credentials.is_empty()
    }

    pub fn len(&self) -> usize {
        self.credentials.len()
    }

    /// Find by credential id, the way an assertion names one.
    pub fn by_id(&self, id: &[u8]) -> Option<&Credential> {
        self.credentials
            .iter()
            .find(|c| c.id_bytes().as_deref() == Some(id))
    }

    pub fn by_label(&self, label: &str) -> Option<&Credential> {
        self.credentials.iter().find(|c| c.label == label)
    }

    pub fn add(&mut self, credential: Credential) -> Result<(), EnrolError> {
        if self.by_label(&credential.label).is_some() {
            return Err(EnrolError::DuplicateLabel(credential.label));
        }
        self.credentials.push(credential);
        Ok(())
    }

    pub fn remove(&mut self, label: &str) -> bool {
        let before = self.credentials.len();
        self.credentials.retain(|c| c.label != label);
        self.credentials.len() != before
    }

    /// Record a counter an assertion proved. Only ever upward.
    pub fn record_counter(&mut self, id: &[u8], counter: u32) {
        for cred in &mut self.credentials {
            if cred.id_bytes().as_deref() == Some(id) && counter > cred.counter {
                cred.counter = counter;
            }
        }
    }

    /// Read the store, or an empty one.
    ///
    /// An unreadable or unparseable file reads as empty, which fails closed:
    /// an empty store is a store that authorises nothing, and the alternative
    /// — refusing to start — would take the daemon down over a file that is
    /// only consulted by one setting nobody has turned on.
    pub fn load_from(path: &Path) -> CredentialStore {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_json::from_str::<CredentialStore>(&t).ok())
            .unwrap_or_default()
    }

    pub fn load() -> CredentialStore {
        CredentialStore::load_from(&store_path())
    }

    pub fn save_to(&self, path: &Path) -> Result<(), EnrolError> {
        let dir = path.parent().ok_or_else(|| {
            EnrolError::Io(format!("{} has no parent directory", path.display()))
        })?;
        paths::ensure_private_dir(dir).map_err(|e| EnrolError::Io(e.to_string()))?;
        let text = serde_json::to_string_pretty(self)
            .map_err(|e| EnrolError::Io(e.to_string()))?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, format!("{text}\n")).map_err(|e| EnrolError::Io(e.to_string()))?;
        std::fs::rename(&tmp, path).map_err(|e| EnrolError::Io(e.to_string()))?;
        Ok(())
    }

    pub fn save(&self) -> Result<(), EnrolError> {
        self.save_to(&store_path())
    }
}

// ---------------------------------------------------------------------------
// The assertion
// ---------------------------------------------------------------------------

/// What a security key produced, as `fido2-assert` prints it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Assertion {
    /// Which credential answered. `fido2-assert` does not print it — the
    /// caller asked for it by id — so it comes from the request.
    pub credential_id: Vec<u8>,
    /// The client data hash the key signed under, line 1.
    pub client_data_hash: Vec<u8>,
    /// The relying party id, line 2.
    pub rp_id: String,
    /// The authenticator data, line 3, still CBOR-wrapped as printed.
    pub auth_data: Vec<u8>,
    /// The DER ECDSA signature, line 4.
    pub signature: Vec<u8>,
}

impl Assertion {
    /// Parse `fido2-assert -G`'s four lines.
    pub fn parse_fido2_assert(credential_id: &[u8], text: &str) -> Result<Assertion, AssertionError> {
        let lines: Vec<&str> = text
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .collect();
        if lines.len() < 4 {
            return Err(AssertionError::Malformed(
                "expected the four lines `fido2-assert -G` prints: client data hash, relying \
                 party id, authenticator data, signature",
            ));
        }
        let client_data_hash =
            b64_decode(lines[0]).ok_or(AssertionError::Malformed("line 1 is not base64"))?;
        let auth_data =
            b64_decode(lines[2]).ok_or(AssertionError::Malformed("line 3 is not base64"))?;
        let signature =
            b64_decode(lines[3]).ok_or(AssertionError::Malformed("line 4 is not base64"))?;
        Ok(Assertion {
            credential_id: credential_id.to_vec(),
            client_data_hash,
            rp_id: lines[1].to_string(),
            auth_data,
            signature,
        })
    }
}

/// Why an assertion did not prove what it had to prove.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssertionError {
    Malformed(&'static str),
    /// The authenticator data was shorter than the fixed head.
    AuthDataTooShort(usize),
    /// No enrolled credential has that id.
    UnknownCredential,
    /// The key signed for a different relying party.
    WrongRelyingParty { expected: String },
    /// The key signed under a client data hash that is not this challenge's.
    WrongChallenge,
    /// The challenge had already been answered, or never issued.
    NoSuchChallenge,
    /// The challenge ran out before the key was touched.
    ChallengeExpired { issued_ms: u64, now_ms: u64 },
    /// A cryptographically valid assertion that nobody touched a key for.
    NoUserPresence,
    /// A cryptographically valid assertion with no PIN or biometric behind it,
    /// where the policy asked for one.
    NoUserVerification,
    /// The signature counter did not move forward: a replay, or a cloned key.
    CounterWentBackwards { stored: u32, presented: u32 },
    /// The signature is not this key's signature over this message.
    BadSignature,
    /// The key could not be read back out of the store.
    Key(KeyError),
    /// `openssl` could not be asked. Distinguished from a bad signature on
    /// purpose: one means somebody is lying and the other means this machine
    /// is broken, and an operator needs to be able to tell them apart.
    VerifierUnavailable(String),
}

impl std::fmt::Display for AssertionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AssertionError::Malformed(why) => write!(f, "{why}"),
            AssertionError::AuthDataTooShort(n) => write!(
                f,
                "authenticator data is {n} bytes; the fixed head alone is 37"
            ),
            AssertionError::UnknownCredential => write!(
                f,
                "no security key with that credential id is enrolled; `apex agent key list` \
                 shows the ones that are"
            ),
            AssertionError::WrongRelyingParty { expected } => write!(
                f,
                "the key signed for a different relying party; this credential was enrolled for \
                 {expected:?}"
            ),
            AssertionError::WrongChallenge => write!(
                f,
                "the key signed a different challenge from the one this request was issued"
            ),
            AssertionError::NoSuchChallenge => write!(
                f,
                "that challenge was never issued, or has already been answered once"
            ),
            AssertionError::ChallengeExpired { issued_ms, now_ms } => write!(
                f,
                "the challenge issued at {issued_ms} ran out {}s ago; ask for another",
                (now_ms.saturating_sub(*issued_ms + CHALLENGE_TTL_MS)) / 1000
            ),
            AssertionError::NoUserPresence => write!(
                f,
                "the assertion is valid but the user-presence bit is clear: the key answered \
                 without anybody touching it. Use `fido2-assert -G -p`"
            ),
            AssertionError::NoUserVerification => write!(
                f,
                "this credential requires user verification and the assertion has none; use \
                 `fido2-assert -G -p -v` and enter the key's PIN"
            ),
            AssertionError::CounterWentBackwards { stored, presented } => write!(
                f,
                "the signature counter went from {stored} to {presented}: this is a replay of an \
                 assertion already used, or a cloned key"
            ),
            AssertionError::BadSignature => write!(
                f,
                "the signature is not this credential's signature over this challenge"
            ),
            AssertionError::Key(e) => write!(f, "{e}"),
            AssertionError::VerifierUnavailable(e) => write!(
                f,
                "the assertion could not be checked at all: {e}. This is not a failed \
                 verification; nothing was verified"
            ),
        }
    }
}

impl std::error::Error for AssertionError {}

/// Whether a PIN or biometric is required as well as a touch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UserVerification {
    /// A touch is enough. The default: a security key with no PIN set is a
    /// perfectly ordinary thing to own, and requiring what the key cannot do
    /// would lock the owner out of their own setting.
    #[default]
    Discouraged,
    /// A PIN or biometric as well.
    Required,
}

/// The whole check, over the client data the key actually signed.
///
/// Returns the signature counter the assertion proved, for the caller to
/// record. Every cheap check runs before the subprocess, and the counter check
/// runs before it too — a replayed assertion is refused without spending a
/// process on it.
///
/// `client_data` is bytes rather than a [`Challenge`] so that the shipped
/// function can be driven end to end by an assertion a real authenticator
/// produced, whose client data was some other program's. The daemon never
/// calls this directly; it calls [`verify_for_challenge`], which supplies
/// `challenge.binding()` and nothing else.
///
/// The comparison against `assertion.client_data_hash` is a courtesy, not the
/// defence. The signature is computed over `authData || SHA-256(client_data)`,
/// so an assertion collected under different client data fails the signature
/// check even if this comparison is removed — which is asserted, by removing
/// it.
pub fn verify_assertion(
    credential: &Credential,
    client_data: &[u8],
    assertion: &Assertion,
    uv: UserVerification,
) -> Result<u32, AssertionError> {
    if credential.id_bytes().as_deref() != Some(assertion.credential_id.as_slice()) {
        return Err(AssertionError::UnknownCredential);
    }
    let expected_cdh = sha256(client_data);
    if assertion.client_data_hash != expected_cdh {
        return Err(AssertionError::WrongChallenge);
    }
    let auth = AuthData::parse(&assertion.auth_data)?;
    if auth.rp_id_hash != sha256(credential.rp_id.as_bytes()) {
        return Err(AssertionError::WrongRelyingParty {
            expected: credential.rp_id.clone(),
        });
    }
    if !auth.user_present() {
        return Err(AssertionError::NoUserPresence);
    }
    if uv == UserVerification::Required && !auth.user_verified() {
        return Err(AssertionError::NoUserVerification);
    }
    // WebAuthn §7.2 step 21: a counter of zero on both sides means the
    // authenticator does not keep one, and is not evidence of anything. Any
    // other pair must move forward.
    if !(auth.counter == 0 && credential.counter == 0) && auth.counter <= credential.counter {
        return Err(AssertionError::CounterWentBackwards {
            stored: credential.counter,
            presented: auth.counter,
        });
    }
    let key = credential.key().map_err(AssertionError::Key)?;
    let mut message = auth.raw.clone();
    message.extend_from_slice(&expected_cdh);
    match verify_signature(&key.to_spki_der(), &assertion.signature, &message) {
        Ok(true) => Ok(auth.counter),
        Ok(false) => Err(AssertionError::BadSignature),
        Err(e) => Err(AssertionError::VerifierUnavailable(e)),
    }
}

/// [`verify_assertion`], against a challenge this daemon issued.
///
/// The only entry point anything outside this module should use. The
/// `challenge` must be one the caller took out of a [`ChallengeStore`] with
/// [`ChallengeStore::redeem`], never one the client sent back: a challenge a
/// client can choose is a challenge a client can reuse.
pub fn verify_for_challenge(
    credential: &Credential,
    challenge: &Challenge,
    assertion: &Assertion,
    uv: UserVerification,
    now_ms: u64,
) -> Result<u32, AssertionError> {
    if challenge.expired_at(now_ms) {
        return Err(AssertionError::ChallengeExpired {
            issued_ms: challenge.issued_ms,
            now_ms,
        });
    }
    verify_assertion(credential, &challenge.binding(), assertion, uv)
}

// ---------------------------------------------------------------------------
// The one subprocess
// ---------------------------------------------------------------------------

/// ECDSA-P256/SHA-256 verification, through `openssl pkeyutl`.
///
/// `Ok(false)` is "openssl ran and said no"; `Err` is "openssl could not
/// answer". Told apart by openssl's own words rather than by its exit status,
/// which is 1 for both a bad signature and a missing file.
///
/// `-rawin -digest sha256` hands openssl the message and lets it hash, which
/// is what a CTAP2 signature is over. Passing a digest to `-verify` without
/// `-rawin` would verify a signature over the digest of the digest.
pub fn verify_signature(spki_der: &[u8], signature: &[u8], message: &[u8]) -> Result<bool, String> {
    let dir = TempDir::new("apex-webauthn")?;
    let key = dir.path().join("key.der");
    let sig = dir.path().join("sig.der");
    let msg = dir.path().join("msg.bin");
    for (path, bytes) in [(&key, spki_der), (&sig, signature), (&msg, message)] {
        std::fs::write(path, bytes).map_err(|e| format!("writing {}: {e}", path.display()))?;
    }
    let out = Command::new("openssl")
        .arg("pkeyutl")
        .arg("-verify")
        .arg("-pubin")
        .arg("-keyform")
        .arg("DER")
        .arg("-inkey")
        .arg(&key)
        .arg("-rawin")
        .arg("-digest")
        .arg("sha256")
        .arg("-sigfile")
        .arg(&sig)
        .arg("-in")
        .arg(&msg)
        .output()
        .map_err(|e| format!("cannot run openssl: {e}"))?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    if out.status.success() && stdout.contains("Signature Verified Successfully") {
        return Ok(true);
    }
    if stdout.contains("Signature Verification Failure")
        || stderr.contains("Signature Verification Failure")
    {
        return Ok(false);
    }
    Err(format!(
        "openssl pkeyutl exited {} without a verdict: {}",
        out.status.code().unwrap_or(-1),
        stderr.trim().lines().next().unwrap_or("no output")
    ))
}

/// A private directory that removes itself.
///
/// `mkdtemp` rather than a name this process invents, and under
/// `$XDG_RUNTIME_DIR` rather than `/tmp`: the runtime directory is the user's
/// own tmpfs, mode 0700, and nothing written here outlives the check.
struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(prefix: &str) -> Result<TempDir, String> {
        let base = paths::runtime_dir();
        paths::ensure_private_dir(&base).map_err(|e| format!("{}: {e}", base.display()))?;
        let template = base.join(format!("{prefix}-XXXXXX"));
        let mut bytes: Vec<u8> = template.as_os_str().as_encoded_bytes().to_vec();
        bytes.push(0);
        // Safe: `bytes` is a NUL-terminated, mutable, correctly sized buffer,
        // and mkdtemp writes only inside it.
        let made = unsafe { libc::mkdtemp(bytes.as_mut_ptr() as *mut libc::c_char) };
        if made.is_null() {
            return Err(format!(
                "cannot create a working directory under {}: {}",
                base.display(),
                std::io::Error::last_os_error()
            ));
        }
        bytes.pop();
        // Safe: mkdtemp only ever replaced the six X bytes with portable
        // filename characters, so what comes back is the same encoding.
        let path = PathBuf::from(unsafe { std::ffi::OsString::from_encoded_bytes_unchecked(bytes) });
        Ok(TempDir { path })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        assert!(s.len() % 2 == 0, "hex must be even length");
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("hex"))
            .collect()
    }

    fn hex32(s: &str) -> [u8; 32] {
        let mut out = [0u8; 32];
        out.copy_from_slice(&hex(s));
        out
    }

    // -----------------------------------------------------------------
    // The vectors. Every one of these was produced by a real authenticator
    // and is copied from somebody else's test suite; none was produced here.
    // Provenance is on each, because a verifier tested against signatures it
    // generated itself has only proved that it agrees with itself.
    // -----------------------------------------------------------------

    /// libfido2 `regress/assert.c`, Yubico's own regression vector.
    ///
    /// Taken with `up=false` — `fido_assert_set_up(a, FIDO_OPT_FALSE)` is in
    /// the file — so its flags byte is `0x00`. That is the one thing a
    /// fabricated vector cannot be: a cryptographically valid assertion that
    /// nobody touched a key for.
    mod yubico {
        pub const RP_ID: &str = "localhost";
        pub const COSE_XY: &str = "34eb9977029c3638bbc2aea0a018c664fce84992d7749e0c468c9da6df46f784\
                                   601e0f8b23854a9aecc1089f30d00dd7767b5548917c4f0f641a1df8be14908a";
        /// As `fido2-assert` prints it: the CBOR byte string, `0x58 0x25`.
        pub const AUTHDATA_CBOR: &str =
            "582549960de5880e8c687434170f6476605b8fe4aeb9a28632c7995cf3ba831d97630000000003";
        pub const CDH: &str = "ec8d8f78424a2bb78234aaca07a1f656421cb6f6b30086523\
                               52da2624abe8976";
        pub const SIG: &str = "3046022100f6d1a3d5242bdeeea09089cdf89ebd6b4d5579e4c14227b79b9ba40a\
                               e247640e022100e5c9c2834731c726e525b2b439a7fc3d70bee9810d4a62a9ab4a\
                               91c07d2d231e";
    }

    /// go-webauthn `protocol/assertion_test.go`,
    /// `testAssertionSpecVectorNoneES256`. `flags = 0x19` (user present, and
    /// the two backup bits), counter 0, relying party `example.org`.
    mod spec_vector {
        pub const RP_ID: &str = "example.org";
        pub const CRED_ID: &str = "f91f391db4c9b2fde0ea70189cba3fb63f579ba6122b33ad94ff3ec330084be4";
        pub const COSE: &str = "a5010203262001215820afefa16f97ca9b2d23eb86ccb64098d20db90856062e\
                                b249c33a9b672f26df61225820930a56b87a2fca66334b03458abf879717c12cc\
                                68ed73290af2e2664796b9220";
        /// Raw, not CBOR-wrapped: this one came from a WebAuthn response.
        pub const AUTHDATA: &str =
            "bfabc37432958b063360d3ad6461c9c4735ae7f8edd46592a5e0f01452b2e4b51900000000";
        pub const CLIENT_DATA: &str = "7b2274797065223a22776562617574686e2e676574222c226368616c6c\
                                       656e6765223a224f63446e55685158756c5455506f334a555854304939\
                                       3770767a7a59425039745a63685879617630314167222c226f72696769\
                                       6e223a2268747470733a2f2f6578616d706c652e6f7267222c2263726f\
                                       73734f726967696e223a66616c73657d";
        pub const SIG: &str = "3046022100f50a4e2e4409249c4a853ba361282f09841df4dd4547a13a87780218\
                               deffcd380221008480ac0f0b93538174f575bf11a1dd5d78c6e486013f937295ea\
                               13653e331e87";
    }

    /// go-webauthn's macOS Touch ID capture from webauthn.io. AAGUID
    /// `adce0002-35bc-c60a-648b-0b25f1f05503` is Apple's. `flags = 0x45`:
    /// user present, user verified, and **attested credential data present**,
    /// so its authenticator data is 187 bytes rather than 37 and the
    /// credential's own key is inside the bytes it signed.
    mod touchid {
        pub const RP_ID: &str = "webauthn.io";
        pub const CRED_ID: &str = "008ec3e6ad8fd0b4be15a97d653ec21ccd8de412db52e9c5f764fc6fa8980b\
                                   5f7d6ceda46a04ae534e7ee5d646a9bd523f40349724d69e";
        pub const COSE: &str = "a501020326200121582028085fb1d1dc04873428f800711c8020affe562fa709\
                                71e44473cd68230167ee2258207107c7c661048138d8bdbafab482ff797465a58\
                                49ac1c02b23fdae0b2502951d";
        pub const AUTHDATA: &str = "74a6ea9213c99c2f74b22492b320cf40262a94c1a950a0397f29250b60841\
                                    ef0455c926219adce000235bcc60a648b0b25f1f055030037008ec3e6ad8f\
                                    d0b4be15a97d653ec21ccd8de412db52e9c5f764fc6fa8980b5f7d6ceda46\
                                    a04ae534e7ee5d646a9bd523f40349724d69ea50102032620012158202808\
                                    5fb1d1dc04873428f800711c8020affe562fa70971e44473cd68230167ee2\
                                    258207107c7c661048138d8bdbafab482ff797465a5849ac1c02b23fdae0b\
                                    2502951d";
        pub const CLIENT_DATA: &str = "7b226368616c6c656e6765223a22453450546349485f48665831704336\
                                       5369676b315343394e416c67657a744e303433397669387a5f63396b22\
                                       2c226e65775f6b6579735f6d61795f62655f61646465645f6865726522\
                                       3a22646f206e6f7420636f6d7061726520636c69656e74446174614a53\
                                       4f4e20616761696e737420612074656d706c6174652e20536565206874\
                                       7470733a2f2f676f6f2e676c2f796162506578222c226f726967696e22\
                                       3a2268747470733a2f2f776562617574686e2e696f222c227479706522\
                                       3a22776562617574686e2e676574227d";
        pub const SIG: &str = "304502201b4854e431cc561dc96432c5a2d1d2d8a4d539ee3e11958575523601702\
                               e63790221009f15dd0aad147805465b139a15c8c167f9846685ea35ba18e489100d\
                               d9566587";
    }

    /// A credential and an assertion built out of one of the vectors above.
    struct Vector {
        credential: Credential,
        client_data: Vec<u8>,
        assertion: Assertion,
    }

    fn spec_vector() -> Vector {
        build(
            spec_vector::RP_ID,
            spec_vector::CRED_ID,
            spec_vector::COSE,
            spec_vector::AUTHDATA,
            spec_vector::CLIENT_DATA,
            spec_vector::SIG,
        )
    }

    fn touchid_vector() -> Vector {
        build(
            touchid::RP_ID,
            touchid::CRED_ID,
            touchid::COSE,
            touchid::AUTHDATA,
            touchid::CLIENT_DATA,
            touchid::SIG,
        )
    }

    fn build(
        rp_id: &str,
        cred_id: &str,
        cose: &str,
        authdata: &str,
        client_data: &str,
        sig: &str,
    ) -> Vector {
        let key = PublicKey::from_cose(&hex(cose)).expect("a real authenticator's COSE key");
        let id = hex(cred_id);
        let client_data = hex(client_data);
        Vector {
            credential: Credential {
                label: "yubikey".into(),
                id: b64_encode(&id),
                public_key_pem: key.to_spki_pem(),
                rp_id: rp_id.into(),
                counter: 0,
                enrolled_ms: 1,
            },
            assertion: Assertion {
                credential_id: id,
                client_data_hash: sha256(&client_data).to_vec(),
                rp_id: rp_id.into(),
                auth_data: hex(authdata),
                signature: hex(sig),
            },
            client_data,
        }
    }

    fn verify(v: &Vector) -> Result<u32, AssertionError> {
        verify_assertion(
            &v.credential,
            &v.client_data,
            &v.assertion,
            UserVerification::Discouraged,
        )
    }

    // -----------------------------------------------------------------
    // SHA-256
    // -----------------------------------------------------------------

    #[test]
    fn the_digest_is_sha_256_and_not_something_that_looks_like_it() {
        // FIPS 180-4's published examples, including both message lengths
        // where a padding bug hides: 55 bytes (one block after padding) and
        // 56 (two).
        let cases: &[(&[u8], &str)] = &[
            (b"", "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"),
            (b"abc", "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"),
            (
                b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq",
                "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1",
            ),
            (
                b"abcdefghbcdefghicdefghijdefghijkefghijklfghijklmghijklmnhijklmnoijklmnopjklmnopq\
                  klmnopqrlmnopqrsmnopqrstnopqrstu",
                "cf5b16a778af8380036ce59e7b0492370b249b11e8f07a51afac45037afee9d1",
            ),
        ];
        for (msg, want) in cases {
            assert_eq!(
                sha256(msg).to_vec(),
                hex(want),
                "message of {} bytes",
                msg.len()
            );
        }
        let million = vec![b'a'; 1_000_000];
        assert_eq!(
            sha256(&million).to_vec(),
            hex("cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0")
        );
    }

    #[test]
    fn the_digest_reproduces_rp_id_hashes_three_real_authenticators_wrote() {
        // Not a second opinion about SHA-256: these are the exact 32 bytes
        // three different devices put in front of an assertion, so a digest
        // that disagreed here would make every rp-id check wrong in exactly
        // the way the published vectors above would not catch.
        assert_eq!(
            sha256(yubico::RP_ID.as_bytes()).to_vec(),
            hex(&yubico::AUTHDATA_CBOR[4..68])
        );
        assert_eq!(
            sha256(spec_vector::RP_ID.as_bytes()).to_vec(),
            hex(&spec_vector::AUTHDATA[..64])
        );
        assert_eq!(
            sha256(touchid::RP_ID.as_bytes()).to_vec(),
            hex(&touchid::AUTHDATA[..64])
        );
    }

    // -----------------------------------------------------------------
    // base64
    // -----------------------------------------------------------------

    #[test]
    fn base64_round_trips_at_every_padding_length() {
        for n in 0..40usize {
            let bytes: Vec<u8> = (0..n).map(|i| (i * 7 + 3) as u8).collect();
            let text = b64_encode(&bytes);
            assert_eq!(text.len() % 4, 0, "{n} bytes encodes unpadded");
            assert_eq!(b64_decode(&text).as_deref(), Some(bytes.as_slice()), "{n}");
        }
    }

    #[test]
    fn base64_accepts_the_url_safe_alphabet_and_whitespace() {
        let bytes = hex("fbf0");
        assert_eq!(b64_encode(&bytes), "+/A=");
        assert_eq!(b64_decode("+/A=").as_deref(), Some(bytes.as_slice()));
        assert_eq!(b64_decode("-_A").as_deref(), Some(bytes.as_slice()));
        assert_eq!(b64_decode(" +/\nA =\t").as_deref(), Some(bytes.as_slice()));
    }

    #[test]
    fn base64_refuses_anything_that_would_decode_two_ways() {
        assert_eq!(b64_decode("not base64!"), None);
        assert_eq!(b64_decode("QUJD").as_deref(), Some(b"ABC".as_slice()));
        // `QQ==` and `QR==` differ, and both would decode to a single `A` if
        // the four leftover bits were ignored. A credential id with two
        // spellings is a comparison waiting to go wrong, so the second is
        // refused rather than folded onto the first.
        assert_eq!(b64_decode("QQ==").as_deref(), Some(b"A".as_slice()));
        assert_eq!(b64_decode("QR=="), None);
    }

    // -----------------------------------------------------------------
    // The key
    // -----------------------------------------------------------------

    #[test]
    fn the_twenty_six_byte_prefix_is_a_p256_spki_openssl_reads_back() {
        let key = PublicKey::from_cose(&hex(spec_vector::COSE)).expect("cose");
        let der = key.to_spki_der();
        assert_eq!(der.len(), 91, "a P-256 SPKI is 91 bytes");
        assert_eq!(der[..26], P256_SPKI_PREFIX);
        assert_eq!(der[26], 0x04, "an uncompressed point");
        assert_eq!(PublicKey::from_spki_der(&der), Ok(key));
        assert_eq!(PublicKey::from_spki_pem(&key.to_spki_pem()), Ok(key));
        // And openssl agrees it is a key: the whole point of the constant.
        // Verification against it succeeds elsewhere in this file, which it
        // could not if openssl could not parse this.
        assert!(key.to_spki_pem().starts_with("-----BEGIN PUBLIC KEY-----\n"));
    }

    #[test]
    fn a_cose_key_from_a_real_authenticator_parses_into_its_two_coordinates() {
        let key = PublicKey::from_cose(&hex(touchid::COSE)).expect("cose");
        assert_eq!(
            key.point()[..32].to_vec(),
            hex("28085fb1d1dc04873428f800711c8020affe562fa70971e44473cd68230167ee")
        );
        assert_eq!(
            key.point()[32..].to_vec(),
            hex("7107c7c661048138d8bdbafab482ff797465a5849ac1c02b23fdae0b2502951d")
        );
        // The Yubico vector reports its key as the bare point rather than as
        // COSE, so it exercises the other constructor.
        let raw = hex(yubico::COSE_XY);
        let bare = PublicKey::from_xy(
            &hex32(&yubico::COSE_XY[..64]),
            &hex32(&yubico::COSE_XY[64..]),
        );
        assert_eq!(bare.point().to_vec(), raw);
    }

    #[test]
    fn a_key_that_is_not_es256_over_p256_is_refused_by_name() {
        // The same map with alg RS256 (-257 does not fit a one-byte encoding,
        // so this uses -8, EdDSA, which does).
        let mut cose = hex(spec_vector::COSE);
        let alg = cose.iter().position(|b| *b == 0x03).expect("alg label");
        cose[alg + 1] = 0x27; // -8, EdDSA
        assert_eq!(
            PublicKey::from_cose(&cose),
            Err(KeyError::UnsupportedAlgorithm { alg: -8, crv: 1 })
        );
        assert!(PublicKey::from_cose(&cose)
            .unwrap_err()
            .to_string()
            .contains("es256"));
    }

    #[test]
    fn a_key_that_is_not_a_cose_map_at_all_is_refused_rather_than_scanned() {
        assert!(matches!(
            PublicKey::from_cose(&[]),
            Err(KeyError::NotCose(_))
        ));
        let mut cose = hex(spec_vector::COSE);
        cose[0] = 0xa4; // a four-entry map
        assert!(matches!(
            PublicKey::from_cose(&cose),
            Err(KeyError::NotCose(_))
        ));
    }

    #[test]
    fn a_spki_that_is_not_p256_is_refused_and_a_pem_without_a_block_is_too() {
        let key = PublicKey::from_cose(&hex(spec_vector::COSE)).expect("cose");
        let mut der = key.to_spki_der();
        der[3] = 0x14; // a different inner length: not this shape
        assert_eq!(PublicKey::from_spki_der(&der), Err(KeyError::NotP256Spki));
        assert_eq!(PublicKey::from_spki_der(&der[..90]), Err(KeyError::NotP256Spki));
        assert_eq!(PublicKey::from_spki_pem("no pem here"), Err(KeyError::NotPem));
        assert_eq!(
            PublicKey::from_spki_pem("-----BEGIN PUBLIC KEY-----\n!!\n-----END PUBLIC KEY-----"),
            Err(KeyError::NotPem)
        );
    }

    // -----------------------------------------------------------------
    // Authenticator data
    // -----------------------------------------------------------------

    #[test]
    fn authenticator_data_parses_with_and_without_the_cbor_wrapper() {
        // `fido2-assert` prints `fido_assert_authdata_ptr`, which is wrapped;
        // a WebAuthn response is not. Both are real, and both are here.
        let wrapped = AuthData::parse(&hex(yubico::AUTHDATA_CBOR)).expect("wrapped");
        assert_eq!(wrapped.raw.len(), 37);
        assert_eq!(wrapped.flags, 0x00);
        assert_eq!(wrapped.counter, 3);

        let bare = AuthData::parse(&hex(spec_vector::AUTHDATA)).expect("bare");
        assert_eq!(bare.raw.len(), 37);
        assert_eq!(bare.flags, 0x19);
        assert_eq!(bare.counter, 0);
        assert_eq!(bare.raw, hex(spec_vector::AUTHDATA));
    }

    #[test]
    fn a_wrapper_is_only_stripped_when_its_length_accounts_for_the_input() {
        // 37 raw bytes whose first byte happens to be 0x58 must not lose two
        // of them: 0x58 is a plausible first byte of an rp-id hash.
        let mut raw = hex(spec_vector::AUTHDATA);
        raw[0] = 0x58;
        raw[1] = 0x22; // would be a length of 34, and 2 + 34 != 37
        let parsed = AuthData::parse(&raw).expect("not a wrapper");
        assert_eq!(parsed.raw.len(), 37);
        assert_eq!(parsed.raw, raw);
    }

    #[test]
    fn attested_credential_data_after_the_head_is_kept_and_not_refused() {
        // Touch ID returns 187 bytes. Refusing them would lock out a platform
        // authenticator a real person really has, and there is nothing to
        // defend: every one of those bytes is inside the signed message.
        let auth = AuthData::parse(&hex(touchid::AUTHDATA)).expect("attested");
        assert_eq!(auth.raw.len(), 187);
        assert_eq!(auth.flags, 0x45);
        assert!(auth.flags & flags::AT != 0);
        assert!(auth.user_present() && auth.user_verified());
        assert_eq!(auth.counter, 1_553_097_241);
    }

    #[test]
    fn authenticator_data_shorter_than_the_head_is_refused_with_its_length() {
        let short = hex(&spec_vector::AUTHDATA[..72]);
        assert_eq!(AuthData::parse(&short), Err(AssertionError::AuthDataTooShort(36)));
        assert_eq!(AuthData::parse(&[]), Err(AssertionError::AuthDataTooShort(0)));
    }

    // -----------------------------------------------------------------
    // The challenge
    // -----------------------------------------------------------------

    fn challenge_at(session: u32, kind: GrantKind, ttl: u64, now: u64, nonce: u8) -> Challenge {
        Challenge::with_nonce(session, kind, ttl, now, &[nonce; 32])
    }

    #[test]
    fn every_field_of_a_challenge_changes_the_bytes_a_key_is_asked_to_sign() {
        let base = challenge_at(7, GrantKind::SystemAccess, 900_000, 1_000, 0xaa);
        let others = [
            challenge_at(8, GrantKind::SystemAccess, 900_000, 1_000, 0xaa),
            challenge_at(7, GrantKind::BreakGlass, 900_000, 1_000, 0xaa),
            challenge_at(7, GrantKind::SystemAccess, 900_001, 1_000, 0xaa),
            challenge_at(7, GrantKind::SystemAccess, 900_000, 1_001, 0xaa),
            challenge_at(7, GrantKind::SystemAccess, 900_000, 1_000, 0xab),
        ];
        for other in &others {
            assert_ne!(base.binding(), other.binding(), "{other:?}");
            assert_ne!(base.client_data_hash(), other.client_data_hash(), "{other:?}");
        }
        // A touch collected for a fifteen-minute capability grant must not be
        // spendable on break-glass, and this is the mechanism that stops it.
        assert_ne!(base.binding(), others[1].binding());
    }

    #[test]
    fn a_challenge_starts_with_a_versioned_context_and_ends_with_a_newline() {
        let c = challenge_at(1, GrantKind::BreakGlass, 60_000, 5, 0x11);
        let text = String::from_utf8(c.binding()).expect("ascii");
        assert!(text.starts_with(CHALLENGE_CONTEXT), "{text}");
        assert!(text.ends_with('\n'), "{text}");
        assert!(text.contains("\nkind=break-glass\n"), "{text}");
        assert!(text.contains("\nsession=1\n"), "{text}");
        assert!(text.contains("\nttl_ms=60000\n"), "{text}");
        assert_eq!(c.client_data_hash(), sha256(&c.binding()));
    }

    #[test]
    fn a_challenge_runs_out_and_says_when_it_did() {
        let c = challenge_at(1, GrantKind::SystemAccess, 60_000, 1_000, 0x22);
        assert!(!c.expired_at(1_000));
        assert!(!c.expired_at(1_000 + CHALLENGE_TTL_MS - 1));
        assert!(c.expired_at(1_000 + CHALLENGE_TTL_MS));
        assert_eq!(c.expires_ms, 1_000 + CHALLENGE_TTL_MS);
    }

    #[test]
    fn a_challenge_is_answerable_once_and_then_never_again() {
        let mut store = ChallengeStore::new();
        let c = store.issue_with_nonce(3, GrantKind::BreakGlass, 60_000, 100, &[9u8; 32]);
        assert_eq!(store.outstanding(), 1);
        assert_eq!(store.redeem(&c.nonce, 100).as_ref(), Some(&c));
        assert_eq!(store.redeem(&c.nonce, 100), None, "a nonce is spent by use");
        assert_eq!(store.outstanding(), 0);
    }

    #[test]
    fn an_unanswered_challenge_is_forgotten_when_its_window_closes() {
        let mut store = ChallengeStore::new();
        let c = store.issue_with_nonce(3, GrantKind::SystemAccess, 60_000, 100, &[9u8; 32]);
        assert_eq!(store.redeem(&c.nonce, 100 + CHALLENGE_TTL_MS), None);
        assert_eq!(store.outstanding(), 0);
    }

    #[test]
    fn an_unauthenticated_caller_cannot_make_the_daemon_remember_forever() {
        let mut store = ChallengeStore::new();
        let mut nonces = Vec::new();
        for i in 0..(MAX_OUTSTANDING_CHALLENGES as u8 + 4) {
            nonces.push(
                store
                    .issue_with_nonce(1, GrantKind::SystemAccess, 60_000, 100, &[i; 32])
                    .nonce,
            );
        }
        assert_eq!(store.outstanding(), MAX_OUTSTANDING_CHALLENGES);
        // The oldest went, the newest stayed: evicting rather than refusing,
        // so a flood cannot lock the owner out of their own machine.
        assert_eq!(store.redeem(&nonces[0], 100), None);
        assert!(store.redeem(nonces.last().expect("issued"), 100).is_some());
    }

    #[test]
    fn a_nonce_comes_from_the_kernel_and_is_not_the_same_twice() {
        let mut store = ChallengeStore::new();
        let a = store.issue(1, GrantKind::SystemAccess, 60_000, 1);
        let b = store.issue(1, GrantKind::SystemAccess, 60_000, 1);
        assert_ne!(a.nonce, b.nonce);
        assert_eq!(b64_decode(&a.nonce).map(|n| n.len()), Some(32));
    }

    // -----------------------------------------------------------------
    // Enrolment and the store
    // -----------------------------------------------------------------

    /// What `fido2-cred -V` prints: the credential id in base64, then a PEM
    /// public key. Built here from a real authenticator's key so the parser
    /// is exercised against the shape the tool actually emits.
    fn fido2_cred_output() -> String {
        let key = PublicKey::from_cose(&hex(touchid::COSE)).expect("cose");
        format!("{}\n{}", b64_encode(&hex(touchid::CRED_ID)), key.to_spki_pem())
    }

    #[test]
    fn enrolment_reads_exactly_what_fido2_cred_printed() {
        let cred =
            Credential::parse_fido2_cred("yubikey", "webauthn.io", &fido2_cred_output(), 42)
                .expect("enrol");
        assert_eq!(cred.id_bytes(), Some(hex(touchid::CRED_ID)));
        assert_eq!(cred.rp_id, "webauthn.io");
        assert_eq!(cred.enrolled_ms, 42);
        assert_eq!(cred.counter, 0);
        assert_eq!(
            cred.key().expect("key"),
            PublicKey::from_cose(&hex(touchid::COSE)).expect("cose")
        );
    }

    #[test]
    fn a_paste_that_is_missing_either_half_is_refused_and_says_which() {
        let key = PublicKey::from_cose(&hex(touchid::COSE)).expect("cose");
        assert_eq!(
            Credential::parse_fido2_cred("k", "rp", "", 1),
            Err(EnrolError::Empty)
        );
        assert_eq!(
            Credential::parse_fido2_cred("k", "rp", &key.to_spki_pem(), 1),
            Err(EnrolError::NoCredentialId)
        );
        assert_eq!(
            Credential::parse_fido2_cred("k", "rp", "AAAA\n", 1),
            Err(EnrolError::Key(KeyError::NotPem))
        );
        assert!(
            Credential::parse_fido2_cred("k", "rp", &key.to_spki_pem(), 1)
                .unwrap_err()
                .to_string()
                .contains("fido2-cred -V")
        );
    }

    #[test]
    fn a_second_key_with_the_same_name_does_not_quietly_retire_the_first() {
        let mut store = CredentialStore::default();
        let cred =
            Credential::parse_fido2_cred("desk", "webauthn.io", &fido2_cred_output(), 1).expect("enrol");
        assert!(store.is_empty());
        store.add(cred.clone()).expect("first");
        assert_eq!(store.len(), 1);
        assert_eq!(
            store.add(cred.clone()),
            Err(EnrolError::DuplicateLabel("desk".into()))
        );
        assert!(store.remove("desk"));
        assert!(!store.remove("desk"));
        assert!(store.is_empty());
    }

    #[test]
    fn the_store_survives_a_round_trip_and_an_unreadable_one_reads_as_empty() {
        let dir = TempDir::new("apex-webauthn-test").expect("temp dir");
        let path = dir.path().join("webauthn.json");
        let mut store = CredentialStore::default();
        store
            .add(
                Credential::parse_fido2_cred("desk", "webauthn.io", &fido2_cred_output(), 7)
                    .expect("enrol"),
            )
            .expect("add");
        store.save_to(&path).expect("save");
        assert_eq!(CredentialStore::load_from(&path), store);

        // Fails closed. An empty store authorises nothing, which is what a
        // corrupt file must mean here.
        std::fs::write(&path, "{ not json").expect("write");
        assert!(CredentialStore::load_from(&path).is_empty());
        assert!(CredentialStore::load_from(&dir.path().join("absent.json")).is_empty());
    }

    #[test]
    fn a_counter_is_recorded_upward_only() {
        let mut store = CredentialStore::default();
        store
            .add(
                Credential::parse_fido2_cred("desk", "webauthn.io", &fido2_cred_output(), 1)
                    .expect("enrol"),
            )
            .expect("add");
        let id = hex(touchid::CRED_ID);
        store.record_counter(&id, 12);
        assert_eq!(store.by_id(&id).expect("found").counter, 12);
        store.record_counter(&id, 5);
        assert_eq!(store.by_id(&id).expect("found").counter, 12);
        store.record_counter(&hex("00"), 99);
        assert_eq!(store.by_id(&id).expect("found").counter, 12);
    }

    // -----------------------------------------------------------------
    // Reading what fido2-assert printed
    // -----------------------------------------------------------------

    #[test]
    fn an_assertion_is_read_out_of_the_four_lines_fido2_assert_prints() {
        let cdh = sha256(&hex(spec_vector::CLIENT_DATA));
        let printed = format!(
            "{}\n{}\n{}\n{}\n",
            b64_encode(&cdh),
            spec_vector::RP_ID,
            b64_encode(&hex(spec_vector::AUTHDATA)),
            b64_encode(&hex(spec_vector::SIG)),
        );
        let id = hex(spec_vector::CRED_ID);
        let a = Assertion::parse_fido2_assert(&id, &printed).expect("four lines");
        assert_eq!(a.client_data_hash, cdh.to_vec());
        assert_eq!(a.rp_id, spec_vector::RP_ID);
        assert_eq!(a.auth_data, hex(spec_vector::AUTHDATA));
        assert_eq!(a.signature, hex(spec_vector::SIG));
        assert_eq!(a.credential_id, id);
        // And it is the same assertion the fixture builds by hand, so the
        // vectors below are the ones the parser would have produced.
        assert_eq!(a, spec_vector().assertion);
    }

    #[test]
    fn a_paste_that_is_not_four_lines_of_base64_is_refused() {
        let id = hex(spec_vector::CRED_ID);
        assert!(matches!(
            Assertion::parse_fido2_assert(&id, "one\ntwo\nthree\n"),
            Err(AssertionError::Malformed(_))
        ));
        assert!(matches!(
            Assertion::parse_fido2_assert(&id, "!!\nrp\nAAAA\nAAAA\n"),
            Err(AssertionError::Malformed(_))
        ));
    }

    // -----------------------------------------------------------------
    // The verification itself, against real authenticators
    // -----------------------------------------------------------------

    #[test]
    fn an_assertion_from_a_real_security_key_verifies() {
        assert_eq!(verify(&spec_vector()), Ok(0));
    }

    #[test]
    fn an_assertion_from_a_real_platform_authenticator_verifies_too() {
        // Touch ID, 187 bytes of authenticator data, user verified, and a
        // counter of 1,553,097,241 against a stored 0.
        assert_eq!(verify(&touchid_vector()), Ok(1_553_097_241));
    }

    #[test]
    fn a_valid_assertion_that_nobody_touched_a_key_for_is_recognised_as_one() {
        // Yubico's own vector, taken with `up=false`. Two facts, both
        // measured rather than argued: the signature is genuinely valid, and
        // the user-presence bit is genuinely clear. Nothing this module could
        // have manufactured proves both at once.
        let auth = AuthData::parse(&hex(yubico::AUTHDATA_CBOR)).expect("authdata");
        assert!(!auth.user_present(), "the flags byte is 0x00");
        let key = PublicKey::from_xy(
            &hex32(&yubico::COSE_XY[..64]),
            &hex32(&yubico::COSE_XY[64..]),
        );
        let mut message = auth.raw.clone();
        message.extend_from_slice(&hex(yubico::CDH));
        assert_eq!(
            verify_signature(&key.to_spki_der(), &hex(yubico::SIG), &message),
            Ok(true),
            "a real device signed this"
        );
    }

    // -----------------------------------------------------------------
    // The mutants. Each is a mutation of a vector above, not of the code.
    // -----------------------------------------------------------------

    #[test]
    fn clearing_the_user_presence_bit_is_refused_before_the_signature_is_looked_at() {
        let mut v = spec_vector();
        v.assertion.auth_data[32] &= !flags::UP;
        assert_eq!(verify(&v), Err(AssertionError::NoUserPresence));
        assert!(verify(&v).unwrap_err().to_string().contains("-p"));
    }

    #[test]
    fn user_verification_is_demanded_only_when_the_policy_asks_for_it() {
        let v = spec_vector();
        assert_eq!(v.assertion.auth_data[32] & flags::UV, 0);
        assert_eq!(
            verify_assertion(
                &v.credential,
                &v.client_data,
                &v.assertion,
                UserVerification::Required
            ),
            Err(AssertionError::NoUserVerification)
        );
        // Touch ID's does have it, so the same policy passes it.
        let t = touchid_vector();
        assert_eq!(
            verify_assertion(
                &t.credential,
                &t.client_data,
                &t.assertion,
                UserVerification::Required
            ),
            Ok(1_553_097_241)
        );
    }

    #[test]
    fn one_flipped_bit_anywhere_in_the_signature_is_refused() {
        // Both halves of the DER signature, r and s, because a verifier that
        // read only one of them would pass one of these.
        for offset in [6usize, 45] {
            let mut v = spec_vector();
            v.assertion.signature[offset] ^= 0x01;
            assert_eq!(verify(&v), Err(AssertionError::BadSignature), "byte {offset}");
        }
    }

    #[test]
    fn a_counter_moved_inside_the_signed_bytes_is_refused() {
        let mut v = touchid_vector();
        v.assertion.auth_data[36] ^= 0x01;
        assert_eq!(verify(&v), Err(AssertionError::BadSignature));
    }

    #[test]
    fn an_assertion_for_a_different_relying_party_is_refused() {
        let mut v = spec_vector();
        v.credential.rp_id = "example.com".into();
        assert_eq!(
            verify(&v),
            Err(AssertionError::WrongRelyingParty {
                expected: "example.com".into()
            })
        );
        // And the other direction: the same credential, an rp-id hash that is
        // not its own.
        let mut w = spec_vector();
        w.assertion.auth_data[0] ^= 0x01;
        assert!(matches!(
            verify(&w),
            Err(AssertionError::WrongRelyingParty { .. })
        ));
    }

    #[test]
    fn an_assertion_from_another_credential_is_refused_before_anything_else() {
        let mut v = spec_vector();
        v.assertion.credential_id = hex(touchid::CRED_ID);
        assert_eq!(verify(&v), Err(AssertionError::UnknownCredential));
    }

    #[test]
    fn one_real_key_cannot_stand_in_for_another() {
        // Touch ID's assertion, the spec vector's key. Everything else about
        // the assertion is untouched and internally consistent.
        let mut v = touchid_vector();
        v.credential.public_key_pem = PublicKey::from_cose(&hex(spec_vector::COSE))
            .expect("cose")
            .to_spki_pem();
        assert_eq!(verify(&v), Err(AssertionError::BadSignature));
    }

    #[test]
    fn an_assertion_collected_under_other_client_data_is_refused() {
        let mut v = spec_vector();
        v.client_data.push(b' ');
        assert_eq!(verify(&v), Err(AssertionError::WrongChallenge));
    }

    #[test]
    fn the_client_data_binding_is_held_by_the_signature_and_not_by_the_comparison() {
        // The same mutation as above, with the assertion's own client data
        // hash line moved to match — which is what an attacker who could
        // choose both would do. The early comparison passes; the signature
        // does not. So deleting that comparison would lose an error message
        // and nothing else.
        let mut v = spec_vector();
        v.client_data.push(b' ');
        v.assertion.client_data_hash = sha256(&v.client_data).to_vec();
        assert_eq!(verify(&v), Err(AssertionError::BadSignature));
    }

    #[test]
    fn an_assertion_already_used_once_is_refused_by_the_counter() {
        let mut v = touchid_vector();
        v.credential.counter = 1_553_097_241;
        assert_eq!(
            verify(&v),
            Err(AssertionError::CounterWentBackwards {
                stored: 1_553_097_241,
                presented: 1_553_097_241
            })
        );
        v.credential.counter = 1_553_097_242;
        assert!(matches!(
            verify(&v),
            Err(AssertionError::CounterWentBackwards { .. })
        ));
    }

    #[test]
    fn a_key_that_keeps_no_counter_is_not_treated_as_a_replay() {
        // WebAuthn §7.2 step 21: zero on both sides is "this authenticator
        // does not count", not evidence of anything. The spec vector's
        // counter is 0, and it verified above with a stored 0 — this asserts
        // the rule rather than the coincidence.
        let mut v = spec_vector();
        v.credential.counter = 0;
        assert_eq!(verify(&v), Ok(0));
        v.credential.counter = 1;
        assert!(matches!(
            verify(&v),
            Err(AssertionError::CounterWentBackwards { .. })
        ));
    }

    #[test]
    fn a_truncated_authenticator_data_block_is_refused_as_too_short() {
        let mut v = spec_vector();
        v.assertion.auth_data.truncate(20);
        assert_eq!(verify(&v), Err(AssertionError::AuthDataTooShort(20)));
    }

    #[test]
    fn a_credential_whose_stored_key_is_unreadable_refuses_rather_than_passes() {
        let mut v = spec_vector();
        v.credential.public_key_pem = "-----BEGIN PUBLIC KEY-----\nAAAA\n-----END PUBLIC KEY-----\n"
            .into();
        assert_eq!(verify(&v), Err(AssertionError::Key(KeyError::NotP256Spki)));
    }

    // -----------------------------------------------------------------
    // The challenge-bound entry point
    // -----------------------------------------------------------------

    #[test]
    fn a_challenge_that_ran_out_is_refused_without_asking_openssl() {
        let v = spec_vector();
        let c = challenge_at(1, GrantKind::SystemAccess, 60_000, 1_000, 0x33);
        assert!(matches!(
            verify_for_challenge(
                &v.credential,
                &c,
                &v.assertion,
                UserVerification::Discouraged,
                1_000 + CHALLENGE_TTL_MS
            ),
            Err(AssertionError::ChallengeExpired { .. })
        ));
    }

    #[test]
    fn an_assertion_for_some_other_challenge_does_not_answer_this_one() {
        // The whole point of the binding: a real, valid, untampered assertion
        // from a real security key, against a challenge this daemon issued,
        // is still no.
        let v = spec_vector();
        let c = challenge_at(1, GrantKind::BreakGlass, 60_000, 1_000, 0x44);
        assert_eq!(
            verify_for_challenge(
                &v.credential,
                &c,
                &v.assertion,
                UserVerification::Discouraged,
                1_000
            ),
            Err(AssertionError::WrongChallenge)
        );
    }

    #[test]
    fn the_only_client_data_the_bound_entry_point_will_use_is_the_challenges() {
        // `verify_for_challenge` must pass `challenge.binding()` and nothing
        // else. Asserted by taking a challenge whose binding is the client
        // data of a real assertion — impossible — and settling for the
        // observable half: the two disagree, and the refusal names the
        // challenge rather than the signature, which is only true if the
        // challenge's own bytes were the ones used.
        let v = spec_vector();
        let c = challenge_at(2, GrantKind::SystemAccess, 60_000, 10, 0x55);
        assert_ne!(c.binding(), v.client_data);
        assert_eq!(
            verify_for_challenge(&v.credential, &c, &v.assertion, UserVerification::Discouraged, 10),
            Err(AssertionError::WrongChallenge)
        );
        assert_eq!(
            verify_assertion(
                &v.credential,
                &c.binding(),
                &v.assertion,
                UserVerification::Discouraged
            ),
            Err(AssertionError::WrongChallenge)
        );
    }

    // -----------------------------------------------------------------
    // The subprocess
    // -----------------------------------------------------------------

    #[test]
    fn a_verifier_that_cannot_answer_is_not_a_verifier_that_said_no() {
        // openssl exits 1 for a bad signature and for a key it cannot load.
        // Telling them apart is the difference between "somebody is lying"
        // and "this machine is broken", and an operator has to be able to.
        let key = PublicKey::from_cose(&hex(spec_vector::COSE)).expect("cose");
        let mut der = key.to_spki_der();
        der.truncate(10);
        let err = verify_signature(&der, &hex(spec_vector::SIG), b"message")
            .expect_err("openssl cannot load this");
        assert!(err.contains("openssl"), "{err}");
        assert!(AssertionError::VerifierUnavailable(err)
            .to_string()
            .contains("nothing was verified"));
    }

    #[test]
    fn the_working_directory_is_private_and_does_not_outlive_the_check() {
        let path = {
            let dir = TempDir::new("apex-webauthn-test").expect("temp dir");
            let path = dir.path().to_path_buf();
            assert!(path.is_dir());
            let mode = std::fs::metadata(&path).expect("stat").permissions();
            {
                use std::os::unix::fs::PermissionsExt;
                assert_eq!(mode.mode() & 0o777, 0o700, "mkdtemp creates 0700");
            }
            assert!(path.starts_with(paths::runtime_dir()), "{path:?}");
            path
        };
        assert!(!path.exists(), "the directory is removed when it goes away");
    }
}
