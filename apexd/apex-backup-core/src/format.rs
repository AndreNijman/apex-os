//! What a snapshot *is*: the objects it consists of, and what is in them.
//!
//! A snapshot is a set of named objects under one prefix. Nothing in this
//! module knows where they are kept — that is [`crate::target`]'s job, and it
//! is why a snapshot written to a local directory and one written to R2 are the
//! same snapshot rather than two formats that resemble each other.
//!
//! ```text
//! <prefix>/<snapshot id>/head.json          plaintext, and the only one
//! <prefix>/<snapshot id>/manifest.000000    the file list, encrypted
//! <prefix>/<snapshot id>/data.000000        file contents, encrypted
//! <prefix>/<snapshot id>/data.000001
//! ```
//!
//! ## Why the data stream is one stream and not one object per file
//!
//! Because a per-file layout puts the file count, and by extension a great deal
//! of the directory structure, into object names an operator of the far side
//! can read. The data stream is the concatenation of every file's contents in
//! manifest order, cut into fixed-size chunks that have nothing to do with
//! where one file ends and the next begins. Each manifest entry records its
//! byte span in that stream, so restoring one file still reads only the chunks
//! it falls in.
//!
//! ## Why the chunk is a mebibyte
//!
//! Measured off the landed R2 path rather than chosen:
//!
//! * `apex_secret_core::project::MAX_PAYLOAD` caps an upload at 10 MiB;
//! * `apex-secretd`'s `broker::HTTP_MAX_BYTES` caps a brokered *reply* at
//!   3 MiB, and that is the binding one, because reading a chunk back is a
//!   reply;
//! * a brokered reply is `String::from_utf8_lossy` of curl's stdout, so an R2
//!   chunk is base64 rather than raw bytes — see [`crate::target::bucket`] — and
//!   base64 costs four bytes for every three.
//!
//! A mebibyte of plaintext seals to 1 MiB + 40 bytes and base64s to about
//! 1.37 MiB, which fits the 3 MiB reply cap with room for the JSON around it.
//! Two mebibytes would not.

use std::fmt;

use serde::{Deserialize, Serialize};

/// The format this build writes, and the only one it reads.
pub const FORMAT: &str = "apex-backup/1";

/// Plaintext bytes per chunk. See the module note for where the number is from.
pub const CHUNK_BYTES: usize = 1024 * 1024;

/// The one object in a snapshot that is not encrypted.
pub const HEAD_OBJECT: &str = "head.json";

/// Largest head this will parse. A head is a few hundred bytes; this is room
/// for a format that grows without being room for a denial of service.
pub const MAX_HEAD_BYTES: usize = 64 * 1024;

/// Largest manifest this will hold in memory, decrypted.
///
/// A manifest entry is roughly 200 bytes, so this is about a quarter of a
/// million files. Bounded because a manifest arrives from storage and storage
/// is not trusted to be honest about its own size.
pub const MAX_MANIFEST_BYTES: usize = 64 * 1024 * 1024;

/// The head of a snapshot: everything a restore needs before it holds a key.
///
/// # What this leaks, stated plainly
///
/// This object is not encrypted and cannot be: a restore has to read the
/// ephemeral public key out of it before it can derive anything. Whoever can
/// read the target can therefore learn, for each snapshot, **that it exists,
/// when it was made, which recipient it is sealed to, and roughly how large it
/// is** (from the chunk count). They cannot learn a file name, a directory
/// name, a file size or a byte of content: all of those are in the manifest,
/// which is a sealed stream like the data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Head {
    /// [`FORMAT`]. Checked before anything else in the object is believed.
    pub format: String,
    /// The snapshot id, repeated inside the object it names so that an object
    /// moved to another id is detected rather than opened.
    pub snapshot: String,
    /// Unix milliseconds.
    pub created_ms: u64,
    /// The recipient string this snapshot is sealed to, so `apex backup list`
    /// can say "this machine cannot open that one" without trying.
    pub recipient: String,
    /// The snapshot's ephemeral X25519 public key, base64url unpadded.
    pub ephemeral: String,
    /// Plaintext bytes per chunk, as written. Read from the head rather than
    /// assumed, so a snapshot written by a build with a different
    /// [`CHUNK_BYTES`] still restores.
    pub chunk_bytes: u32,
    /// How many chunks the manifest stream has.
    pub manifest_chunks: u32,
    /// How many chunks the data stream has.
    pub data_chunks: u32,
    /// What the source root was called, for a person reading `apex backup
    /// list`. A label and never a path: the directory this came from is the
    /// caller's business and it would otherwise be in the clear.
    pub label: String,
}

impl Head {
    /// The object name of one chunk of a stream.
    pub fn chunk_object(stream: crate::crypto::Stream, index: u32) -> String {
        format!("{}.{index:06}", stream.as_str())
    }

    /// Every object this snapshot consists of, in the order a writer writes
    /// them and the reverse of the order a deleter should remove them.
    ///
    /// The head is **last**. A snapshot whose head is absent is an incomplete
    /// write rather than a corrupt snapshot, and writing the head last is what
    /// makes that distinction true rather than hopeful.
    pub fn objects(&self) -> Vec<String> {
        let mut names = Vec::new();
        for i in 0..self.data_chunks {
            names.push(Head::chunk_object(crate::crypto::Stream::Data, i));
        }
        for i in 0..self.manifest_chunks {
            names.push(Head::chunk_object(crate::crypto::Stream::Manifest, i));
        }
        names.push(HEAD_OBJECT.to_string());
        names
    }

    pub fn ephemeral_bytes(&self) -> Result<[u8; 32], FormatError> {
        let raw = data_encoding::BASE64URL_NOPAD
            .decode(self.ephemeral.as_bytes())
            .map_err(|_| FormatError::BadHead {
                field: "ephemeral",
                why: "not unpadded base64url".to_string(),
            })?;
        if raw.len() != 32 {
            return Err(FormatError::BadHead {
                field: "ephemeral",
                why: format!("{} bytes, and an X25519 public key is 32", raw.len()),
            });
        }
        let mut out = [0u8; 32];
        out.copy_from_slice(&raw);
        Ok(out)
    }
}

/// One file in a snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    /// Path relative to the source root, `/`-separated.
    pub path: String,
    /// What it is. A snapshot carries regular files and directories; a symlink
    /// carries its target rather than being followed.
    pub kind: Kind,
    /// Size in bytes of the content in the data stream. Zero for a directory.
    pub size: u64,
    /// Offset of this entry's content in the plaintext data stream.
    pub offset: u64,
    /// The low twelve bits of the mode, which is the part a restore sets.
    pub mode: u32,
    /// Modification time, unix milliseconds.
    pub mtime_ms: u64,
    /// SHA-256 of the content, lowercase hex. Empty for a directory.
    ///
    /// The AEAD already makes a chunk unforgeable, so this is not what catches
    /// an attacker. It is what catches **this program**: a restore that wrote
    /// the right bytes to the wrong place, or an off-by-one in an entry's span,
    /// passes every AEAD check in the format and fails this one.
    pub digest: String,
}

/// What a manifest entry is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    File,
    Directory,
    /// The content in the data stream is the link's target, as bytes.
    Symlink,
}

/// The file list. Encrypted, always.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub entries: Vec<Entry>,
    /// Total plaintext bytes in the data stream. Checked against what the
    /// chunks actually decrypt to.
    pub data_bytes: u64,
    /// What was in the source tree and is not in this snapshot, with the reason
    /// for each.
    ///
    /// In the manifest — encrypted, like everything that describes the tree —
    /// and not in the head, so that a list of paths this machine could not read
    /// is not readable by whoever can read the target. It is here at all
    /// because a restore years later has to be able to say "this tree was
    /// already incomplete when it was taken" rather than looking complete.
    #[serde(default)]
    pub skipped: Vec<crate::session::Skipped>,
}

/// Why a snapshot's own bytes were not the shape this build reads.
///
/// Distinct from [`crate::crypto::CryptoError`]: these are *format* faults,
/// which a well-formed snapshot from a future build could also produce, and the
/// difference matters when deciding whether a verdict is `Failed` or
/// `CouldNotRun`. A head naming a format this build does not know is not a
/// corrupt snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormatError {
    /// The head named a format this build does not read.
    UnknownFormat { got: String },
    /// The head is not the JSON this build parses.
    BadHead { field: &'static str, why: String },
    /// The head is there and names a different snapshot than the one it was
    /// found under.
    WrongSnapshot { found_under: String, says: String },
    /// The decrypted manifest is not the JSON this build parses.
    BadManifest(String),
    /// The manifest and the data stream do not agree.
    Inconsistent(String),
    /// Something the format bounds was over its bound.
    TooBig { what: &'static str, limit: usize },
}

impl fmt::Display for FormatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FormatError::UnknownFormat { got } => write!(
                f,
                "that snapshot says it is '{}' and this build reads '{FORMAT}'. \
                 It is not damaged; it was written by something newer",
                got.escape_debug()
            ),
            FormatError::BadHead { field, why } => {
                write!(f, "the snapshot head's '{field}' is unusable: {why}")
            }
            FormatError::WrongSnapshot { found_under, says } => write!(
                f,
                "the head stored under {} says it belongs to {}. One of the two \
                 was moved, and this build will not guess which",
                found_under.escape_debug(),
                says.escape_debug()
            ),
            FormatError::BadManifest(why) => write!(
                f,
                "the manifest decrypted and then did not parse: {why}. That is \
                 a format fault and not a wrong key — a wrong key does not get \
                 this far"
            ),
            FormatError::Inconsistent(why) => {
                write!(f, "the snapshot contradicts itself: {why}")
            }
            FormatError::TooBig { what, limit } => write!(
                f,
                "that snapshot's {what} is larger than the {limit} bytes this \
                 build will hold"
            ),
        }
    }
}

impl std::error::Error for FormatError {}

/// A snapshot id: sortable by time, unique without coordination.
///
/// `20260912T014233Z-0badc0de`. The instant is first so that the lexical order
/// of the ids is their chronological order, which is what lets a target that
/// can only list names answer "the most recent" without opening anything.
///
/// The shape is deliberately inside `apex_secret_core::operation::valid_name`:
/// an id becomes one segment of an R2 object key, and a key the capability
/// framework refuses is an id that cannot be written at all.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct SnapshotId(String);

impl SnapshotId {
    /// A new id for now.
    ///
    /// # The tail is three digits of milliseconds and five of randomness
    ///
    /// It was eight random digits, and that was a defect this crate's own
    /// suite found rather than one anybody reasoned to. `tests/
    /// test-apex-backup-s3.sh` writes two snapshots a fraction of a second
    /// apart — which is what a script, a test or a retry does — and then
    /// restores `latest`. About one run in three it restored the FIRST of the
    /// two, because the stamp is only accurate to the second and the order
    /// inside one second was the order of two random numbers.
    ///
    /// That is the whole reason the lexical order exists: a target that can
    /// only list names has to be able to answer "the most recent" without
    /// opening anything, and an order that is right to the second and a coin
    /// toss below it is not an order.
    ///
    /// So the first three hex digits of the tail are the millisecond within
    /// the second — 0 to 999, which is `0x000` to `0x3e7`, three digits
    /// exactly — and the remaining five are random. Two consequences, both
    /// wanted:
    ///
    /// * ids inside one second now sort by when they were made;
    /// * the shape does not change. Still `<16>-<8 lowercase hex>`, so
    ///   [`SnapshotId::parse`] is untouched, ids already on a target still
    ///   parse and still sort against new ones, and the key is still one
    ///   `valid_name` segment.
    ///
    /// 20 bits of randomness is left, which is what stops two snapshots in the
    /// same millisecond colliding. That is a far smaller window than the one
    /// second this replaces, and a collision there is two runs of the same
    /// project at once — which the target's own atomic write handles.
    pub fn new(now_ms: u64) -> Result<SnapshotId, std::io::Error> {
        let mut random = [0u8; 3];
        getrandom::getrandom(&mut random).map_err(std::io::Error::other)?;
        let millis = (now_ms % 1000) as u16;
        Ok(SnapshotId(format!(
            "{}-{millis:03x}{}",
            stamp(now_ms),
            // Five digits, so the whole tail is eight. `HEXLOWER` of three
            // bytes is six; the first is dropped rather than the last so that
            // the bytes that survive are whole.
            &data_encoding::HEXLOWER.encode(&random)[1..]
        )))
    }

    /// Accept one that came from storage, or refuse it.
    ///
    /// Storage supplies these — they are object-key segments a far side listed
    /// back — so they are checked rather than trusted. `valid_name` is the same
    /// function the capability framework applies to the key, so an id this
    /// accepts is an id that can be addressed.
    /// A mutation found this function's first draft calling
    /// `apex_secret_core::operation::valid_name` before the shape check. The
    /// call was **redundant** — every string the shape check below accepts is
    /// 25 characters of digits, `T`, `Z`, `-` and lowercase hex, which
    /// `valid_name` accepts by construction — so removing it changed no
    /// answer and no test went red. A check that cannot refuse anything is the
    /// same dead branch this repository found in `apex trust --verify`'s
    /// cosign arm, so it is gone, and the coupling it was there to express is
    /// asserted instead by
    /// `every_id_the_shape_check_accepts_is_addressable`.
    pub fn parse(text: &str) -> Option<SnapshotId> {
        // Shape, not just characters: `<16 chars of stamp>-<8 hex>`.
        let (stamp, tail) = text.split_once('-')?;
        if stamp.len() != 16 || tail.len() != 8 {
            return None;
        }
        if !tail.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()) {
            return None;
        }
        let bytes = stamp.as_bytes();
        let digits = |range: std::ops::Range<usize>| {
            bytes[range].iter().all(u8::is_ascii_digit)
        };
        if !(digits(0..8) && bytes[8] == b'T' && digits(9..15) && bytes[15] == b'Z') {
            return None;
        }
        Some(SnapshotId(text.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SnapshotId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// `YYYYMMDDTHHMMSSZ` from unix milliseconds, without pulling in a date crate.
///
/// The civil-from-days conversion is Howard Hinnant's, which is exact for every
/// date this program can see and is tested against known instants rather than
/// against itself.
fn stamp(now_ms: u64) -> String {
    let secs = now_ms / 1000;
    let days = (secs / 86_400) as i64;
    let tod = secs % 86_400;
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}{m:02}{d:02}T{:02}{:02}{:02}Z",
        tod / 3600,
        (tod % 3600) / 60,
        tod % 60
    )
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// SHA-256 of a slice, lowercase hex.
pub fn digest_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(bytes);
    data_encoding::HEXLOWER.encode(&h.finalize())
}

#[cfg(test)]
mod tests;
