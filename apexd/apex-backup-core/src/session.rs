//! Writing a snapshot, listing them, verifying one, restoring one.
//!
//! # Versioned restore
//!
//! P2-001's third criterion. A snapshot id sorts by the instant it was taken,
//! so the versions of a source tree are the snapshot ids on a target and
//! restoring a particular one is [`restore`] with that id. There is no "latest"
//! pointer object to go stale, and no index to race: the target's own listing
//! is the history.
//!
//! # Two verdicts per snapshot, for the same reason `verify.rs` has two
//!
//! [`verify`] answers separately about **presence** and **contents**, and the
//! split is the whole point:
//!
//! * an unprivileged caller can establish that every object a snapshot declares
//!   is fetchable. That is worth having and it is not a claim that the data is
//!   good.
//! * only the holder of the private half can establish that the chunks open and
//!   that every file's digest matches.
//!
//! So without a key, `contents` is [`Verdict::CouldNotRun`] — never `Intact`,
//! and never `Failed` either. "I checked what I could" reported as "verified"
//! is the failure mode a backup system cannot have.
//!
//! # What a restore refuses
//!
//! A destination that already holds anything, unless the caller says
//! otherwise in as many words. A restore is the operation with the most
//! authority in this crate — it writes a whole tree — and "restore over the
//! thing you were trying to recover" is a keystroke away from "restore beside
//! it".
//!
//! Every path in a manifest is checked before it is joined to the destination.
//! A manifest comes back from storage; storage is a far side; a far side that
//! can put `../../etc/cron.d/x` in a manifest can write it as root.

use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

use crate::config::BackupConfig;
use crate::crypto::{open_with, seal_to, Identity, Recipient, SnapshotKey, Stream};
use crate::format::{
    digest_hex, Entry, FormatError, Head, Kind, Manifest, SnapshotId, CHUNK_BYTES, FORMAT,
    HEAD_OBJECT, MAX_HEAD_BYTES, MAX_MANIFEST_BYTES,
};
use crate::target::{Target, TargetError};
use crate::verdict::Verdict;

/// Why a run did not produce a snapshot.
#[derive(Debug)]
pub enum SessionError {
    /// The source tree could not be read.
    Source { path: PathBuf, why: String },
    /// Something under the source could not be read, and the caller did not
    /// say to carry on without it.
    Unreadable { skipped: Vec<Skipped> },
    /// The target refused.
    Target(TargetError),
    /// A cryptographic step refused.
    Crypto(crate::crypto::CryptoError),
    /// The snapshot's own bytes were not the shape this build reads.
    Format(FormatError),
    /// The destination of a restore is not one this build will write into.
    Destination { path: PathBuf, why: String },
}

impl std::fmt::Display for SessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SessionError::Source { path, why } => {
                write!(f, "{} could not be read: {why}", path.display())
            }
            SessionError::Unreadable { skipped } => {
                write!(
                    f,
                    "{} thing(s) under the source could not be read, and a \
                     snapshot that quietly leaves files out is a snapshot that \
                     claims to hold a tree it does not. Either fix the \
                     permissions, run the backup as an account that can read \
                     them, or say --skip-unreadable, which records every one of \
                     them in the manifest so a restore can say what is missing. \
                     First: {}",
                    skipped.len(),
                    skipped
                        .iter()
                        .take(3)
                        .map(|s| format!("{} ({})", s.path, s.why))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }
            SessionError::Target(e) => write!(f, "{e}"),
            SessionError::Crypto(e) => write!(f, "{e}"),
            SessionError::Format(e) => write!(f, "{e}"),
            SessionError::Destination { path, why } => {
                write!(f, "{}: {why}", path.display())
            }
        }
    }
}

impl std::error::Error for SessionError {}

impl From<TargetError> for SessionError {
    fn from(e: TargetError) -> SessionError {
        SessionError::Target(e)
    }
}

impl From<crate::crypto::CryptoError> for SessionError {
    fn from(e: crate::crypto::CryptoError) -> SessionError {
        SessionError::Crypto(e)
    }
}

impl From<FormatError> for SessionError {
    fn from(e: FormatError) -> SessionError {
        SessionError::Format(e)
    }
}

/// What a walk of the source found: the things to back up, in order, and the
/// things that could not be.
type Walk = (Vec<(String, Kind)>, Vec<Skipped>);

/// Something under the source that was not backed up, and why.
///
/// Recorded *in the manifest*, so it is encrypted like everything else and so
/// that a restore years later can say "this tree was incomplete when it was
/// taken" rather than looking complete.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Skipped {
    pub path: String,
    pub why: String,
}

/// What a finished run produced.
#[derive(Debug, Clone)]
pub struct Written {
    pub id: SnapshotId,
    pub head: Head,
    pub files: usize,
    pub bytes: u64,
    pub skipped: Vec<Skipped>,
}

/// Options for a run.
#[derive(Debug, Clone, Default)]
pub struct RunOptions {
    /// Carry on past something that cannot be read, recording it.
    pub skip_unreadable: bool,
    /// What to call this snapshot in a listing. Never a path.
    pub label: String,
}

// ── writing ─────────────────────────────────────────────────────────────────

/// Take a snapshot of `source` and put it on `target`.
pub fn write(
    source: &Path,
    target: &dyn Target,
    recipient: &Recipient,
    config: &BackupConfig,
    options: &RunOptions,
    now_ms: u64,
) -> Result<Written, SessionError> {
    // Everything that can refuse, refuses before a byte is sealed: the target
    // is checked, the tree is walked, and only then is a key derived.
    target.prepare()?;

    let (paths, skipped) = collect(source, config)?;
    if !skipped.is_empty() && !options.skip_unreadable {
        return Err(SessionError::Unreadable { skipped });
    }

    let id = SnapshotId::new(now_ms).map_err(|e| SessionError::Source {
        path: source.to_path_buf(),
        why: e.to_string(),
    })?;
    let (key, ephemeral) = seal_to(recipient, id.as_str())?;

    let mut data = ChunkWriter::new(target, &key, id.as_str(), Stream::Data);
    let mut entries = Vec::with_capacity(paths.len());
    let mut files = 0usize;

    for (relative, kind) in &paths {
        let absolute = source.join(relative);
        let meta = std::fs::symlink_metadata(&absolute).map_err(|e| SessionError::Source {
            path: absolute.clone(),
            why: e.to_string(),
        })?;
        use std::os::unix::fs::MetadataExt;
        use std::os::unix::fs::PermissionsExt;
        let offset = data.written();
        let mut hasher = Digest::new();

        let size = match kind {
            Kind::Directory => 0,
            Kind::Symlink => {
                let link = std::fs::read_link(&absolute).map_err(|e| SessionError::Source {
                    path: absolute.clone(),
                    why: e.to_string(),
                })?;
                let bytes = link.as_os_str().as_encoded_bytes().to_vec();
                hasher.update(&bytes);
                data.push(&bytes)?;
                bytes.len() as u64
            }
            Kind::File => {
                files += 1;
                let mut file =
                    std::fs::File::open(&absolute).map_err(|e| SessionError::Source {
                        path: absolute.clone(),
                        why: e.to_string(),
                    })?;
                let mut buffer = vec![0u8; 256 * 1024];
                let mut total = 0u64;
                loop {
                    let n = file.read(&mut buffer).map_err(|e| SessionError::Source {
                        path: absolute.clone(),
                        why: e.to_string(),
                    })?;
                    if n == 0 {
                        break;
                    }
                    hasher.update(&buffer[..n]);
                    data.push(&buffer[..n])?;
                    total += n as u64;
                }
                total
            }
        };

        entries.push(Entry {
            path: relative.clone(),
            kind: *kind,
            // The size ACTUALLY READ, never the one `stat` reported. A file
            // that grew or shrank between the walk and the read would
            // otherwise put a length in the manifest that the data stream does
            // not have, and every later restore would be off by that much for
            // every file after it.
            size,
            offset,
            mode: meta.permissions().mode() & 0o7777,
            mtime_ms: (meta.mtime() as u64).saturating_mul(1000)
                + (meta.mtime_nsec() as u64) / 1_000_000,
            digest: if *kind == Kind::Directory {
                String::new()
            } else {
                hasher.finish()
            },
        });
    }

    let data_bytes = data.written();
    let data_chunks = data.finish()?;

    let manifest = Manifest {
        entries,
        data_bytes,
        skipped: skipped.clone(),
    };
    let text = serde_json::to_vec(&manifest).map_err(|e| SessionError::Source {
        path: source.to_path_buf(),
        why: format!("serialising the manifest: {e}"),
    })?;
    let mut manifest_stream = ChunkWriter::new(target, &key, id.as_str(), Stream::Manifest);
    manifest_stream.push(&text)?;
    let manifest_chunks = manifest_stream.finish()?;

    let head = Head {
        format: FORMAT.to_string(),
        snapshot: id.to_string(),
        created_ms: now_ms,
        recipient: recipient.to_string_value(),
        ephemeral: data_encoding::BASE64URL_NOPAD.encode(&ephemeral),
        chunk_bytes: CHUNK_BYTES as u32,
        manifest_chunks,
        data_chunks,
        label: options.label.clone(),
    };
    // LAST. A snapshot whose head is absent is an interrupted write rather
    // than a corrupt snapshot, and writing the head last is what makes that
    // true rather than hopeful.
    let head_bytes = serde_json::to_vec_pretty(&head).map_err(|e| SessionError::Source {
        path: source.to_path_buf(),
        why: format!("serialising the head: {e}"),
    })?;
    target.put(id.as_str(), HEAD_OBJECT, &head_bytes)?;

    Ok(Written {
        id,
        head,
        files,
        bytes: data_bytes,
        skipped,
    })
}

/// Seals and stores a stream, one chunk at a time.
///
/// A chunk is emitted only once there are *more* than [`CHUNK_BYTES`] bytes
/// buffered, so the final chunk — the one carrying `last = 1` — is whatever is
/// left at [`ChunkWriter::finish`]. That is what makes truncation detectable
/// without a trailing empty chunk in the usual case, and it means an empty
/// stream is one empty chunk rather than none: a stream with no chunks at all
/// has no `last` flag anywhere, so nothing distinguishes it from a stream whose
/// chunks all vanished.
struct ChunkWriter<'a> {
    target: &'a dyn Target,
    key: &'a SnapshotKey,
    snapshot: &'a str,
    stream: Stream,
    buffer: Vec<u8>,
    index: u32,
    written: u64,
}

impl<'a> ChunkWriter<'a> {
    fn new(
        target: &'a dyn Target,
        key: &'a SnapshotKey,
        snapshot: &'a str,
        stream: Stream,
    ) -> ChunkWriter<'a> {
        ChunkWriter {
            target,
            key,
            snapshot,
            stream,
            buffer: Vec::with_capacity(CHUNK_BYTES + 64 * 1024),
            index: 0,
            written: 0,
        }
    }

    fn written(&self) -> u64 {
        self.written
    }

    fn push(&mut self, bytes: &[u8]) -> Result<(), SessionError> {
        self.buffer.extend_from_slice(bytes);
        self.written += bytes.len() as u64;
        while self.buffer.len() > CHUNK_BYTES {
            let rest = self.buffer.split_off(CHUNK_BYTES);
            let full = std::mem::replace(&mut self.buffer, rest);
            self.emit(&full, false)?;
        }
        Ok(())
    }

    fn emit(&mut self, bytes: &[u8], last: bool) -> Result<(), SessionError> {
        let sealed = self
            .key
            .seal_chunk(self.stream, u64::from(self.index), last, bytes)?;
        self.target.put(
            self.snapshot,
            &Head::chunk_object(self.stream, self.index),
            &sealed,
        )?;
        self.index += 1;
        Ok(())
    }

    fn finish(mut self) -> Result<u32, SessionError> {
        let tail = std::mem::take(&mut self.buffer);
        self.emit(&tail, true)?;
        Ok(self.index)
    }
}

/// SHA-256, fed incrementally.
struct Digest(sha2::Sha256);

impl Digest {
    fn new() -> Digest {
        use sha2::Digest as _;
        Digest(sha2::Sha256::new())
    }
    fn update(&mut self, bytes: &[u8]) {
        use sha2::Digest as _;
        self.0.update(bytes);
    }
    fn finish(self) -> String {
        use sha2::Digest as _;
        data_encoding::HEXLOWER.encode(&self.0.finalize())
    }
}

/// Walk the source, in a deterministic order, skipping what the project
/// excludes.
///
/// Symlinks are **recorded, never followed**, so a link pointing out of the
/// source tree stores its target as text and does not pull in whatever is
/// there. Anything that is not a file, a directory or a symlink — a socket, a
/// fifo, a device node — is recorded as skipped with a reason rather than
/// silently dropped: it is a thing that was in the tree and is not in the
/// snapshot, which is exactly what a restore needs told.
fn collect(source: &Path, config: &BackupConfig) -> Result<Walk, SessionError> {
    let mut out = Vec::new();
    let mut skipped = Vec::new();
    let mut stack = vec![String::new()];

    while let Some(relative) = stack.pop() {
        let dir = if relative.is_empty() {
            source.to_path_buf()
        } else {
            source.join(&relative)
        };
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(e) if relative.is_empty() => {
                // The source root itself. Nothing can carry on from here.
                return Err(SessionError::Source {
                    path: dir,
                    why: e.to_string(),
                });
            }
            Err(e) => {
                skipped.push(Skipped {
                    path: relative.clone(),
                    why: e.to_string(),
                });
                continue;
            }
        };

        let mut here = Vec::new();
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(e) => {
                    skipped.push(Skipped {
                        path: relative.clone(),
                        why: format!("reading an entry: {e}"),
                    });
                    continue;
                }
            };
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                skipped.push(Skipped {
                    path: format!("{relative}/<a name that is not UTF-8>"),
                    why: "this build addresses paths as text".to_string(),
                });
                continue;
            };
            let child = if relative.is_empty() {
                name.to_string()
            } else {
                format!("{relative}/{name}")
            };
            if config.is_excluded(&child) {
                continue;
            }
            let meta = match std::fs::symlink_metadata(source.join(&child)) {
                Ok(meta) => meta,
                Err(e) => {
                    skipped.push(Skipped {
                        path: child,
                        why: e.to_string(),
                    });
                    continue;
                }
            };
            let kind = if meta.file_type().is_symlink() {
                Kind::Symlink
            } else if meta.is_dir() {
                Kind::Directory
            } else if meta.is_file() {
                Kind::File
            } else {
                skipped.push(Skipped {
                    path: child,
                    why: "not a file, a directory or a symbolic link".to_string(),
                });
                continue;
            };
            here.push((child, kind));
        }
        // Deterministic: the same tree produces the same manifest order, which
        // is what makes two snapshots of an unchanged tree comparable.
        here.sort();
        for (child, kind) in here {
            if kind == Kind::Directory {
                stack.push(child.clone());
            }
            out.push((child, kind));
        }
    }
    out.sort();
    Ok((out, skipped))
}

// ── listing ─────────────────────────────────────────────────────────────────

/// One snapshot, as a listing sees it.
#[derive(Debug, Clone)]
pub struct Listing {
    pub id: SnapshotId,
    /// The head, when it could be read and parsed.
    pub head: Option<Head>,
    /// What is known about the head. `Absent` here means the write was
    /// interrupted — the head is the last object written.
    pub verdict: Verdict,
}

/// Every snapshot on a target, oldest first.
pub fn list(target: &dyn Target) -> Result<Vec<Listing>, TargetError> {
    let ids = target.list()?;
    Ok(ids
        .into_iter()
        .map(|id| {
            let (head, verdict) = match read_head(target, &id) {
                Ok(head) => {
                    let verdict = Verdict::Intact {
                        checked: format!(
                            "the head reads, and declares {} data and {} manifest chunk(s)",
                            head.data_chunks, head.manifest_chunks
                        ),
                    };
                    (Some(head), verdict)
                }
                Err(verdict) => (None, verdict),
            };
            Listing { id, head, verdict }
        })
        .collect())
}

/// Read and check one snapshot's head.
fn read_head(target: &dyn Target, id: &SnapshotId) -> Result<Head, Verdict> {
    let bytes = target
        .get(id.as_str(), HEAD_OBJECT)
        .map_err(|e| match &e {
            // The head is written last, so its absence is an interrupted
            // write. Saying that is more useful than saying "absent".
            TargetError::Absent(_) => Verdict::Absent(format!(
                "snapshot {id} has no {HEAD_OBJECT}. The head is the last \
                 object a run writes, so this is a backup that was interrupted \
                 rather than one that is damaged"
            )),
            _ => e.verdict(&format!("the head of snapshot {id}")),
        })?;
    if bytes.len() > MAX_HEAD_BYTES {
        return Err(Verdict::Failed(format!(
            "the head of snapshot {id} is {} bytes, and a head is a few hundred",
            bytes.len()
        )));
    }
    let head: Head = serde_json::from_slice(&bytes).map_err(|e| {
        Verdict::Failed(format!("the head of snapshot {id} did not parse: {e}"))
    })?;
    if head.format != FORMAT {
        return Err(Verdict::CouldNotRun(
            FormatError::UnknownFormat {
                got: head.format.clone(),
            }
            .to_string(),
        ));
    }
    if head.snapshot != id.as_str() {
        return Err(Verdict::Failed(
            FormatError::WrongSnapshot {
                found_under: id.to_string(),
                says: head.snapshot.clone(),
            }
            .to_string(),
        ));
    }
    Ok(head)
}

// ── verifying ───────────────────────────────────────────────────────────────

/// Both verdicts about one snapshot.
///
/// Shaped after `apex/src/verify.rs`'s `Verification`, which carries a verdict
/// for the signature and one for the provenance, for the same reason: two
/// different questions, two answers, and neither standing in for the other.
#[derive(Debug, Clone)]
pub struct Verification {
    pub id: SnapshotId,
    /// Is every object this snapshot declares actually there?
    pub presence: Verdict,
    /// Does it open, and does every file match its digest?
    ///
    /// [`Verdict::CouldNotRun`] when the caller held no private key. Presence
    /// is not contents, and a report that said "verified" after checking only
    /// that the files exist would be the failure this crate is built to avoid.
    pub contents: Verdict,
}

impl Verification {
    /// One line, worst-first.
    pub fn summary(&self) -> String {
        format!(
            "{}: objects {} / contents {}",
            self.id,
            self.presence.as_str(),
            self.contents.as_str()
        )
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "snapshot": self.id.as_str(),
            "presence": self.presence.to_json(),
            "contents": self.contents.to_json(),
        })
    }
}

/// Check one snapshot. With `identity`, check what is in it too.
pub fn verify(
    target: &dyn Target,
    id: &SnapshotId,
    identity: Option<&Identity>,
) -> Verification {
    let head = match read_head(target, id) {
        Ok(head) => head,
        Err(verdict) => {
            return Verification {
                id: id.clone(),
                contents: Verdict::CouldNotRun(format!(
                    "the head could not be established, so nothing in the \
                     snapshot was opened: {}",
                    verdict.reason().unwrap_or("no reason given")
                )),
                presence: verdict,
                }
        }
    };

    // Presence. Every object the head declares, fetched.
    let mut presence = Verdict::Intact {
        checked: format!(
            "{} object(s): the head, {} manifest chunk(s) and {} data chunk(s)",
            head.objects().len(),
            head.manifest_chunks,
            head.data_chunks
        ),
    };
    for object in head.objects() {
        if object == HEAD_OBJECT {
            continue;
        }
        // Fetched and dropped: this loop's question is whether every object
        // the head declares can be got, and it is asked separately from
        // whether the bytes are any good because only one of the two needs a
        // key. Keeping them would mean holding a whole snapshot in memory to
        // answer a question that does not need the bytes.
        match target.get(id.as_str(), &object) {
            Ok(_) => {}
            Err(e) => {
                presence = presence
                    .worse_of(declared_object_verdict(&e, &format!("{object} of snapshot {id}")));
            }
        }
    }

    let Some(identity) = identity else {
        return Verification {
            id: id.clone(),
            presence,
            contents: Verdict::CouldNotRun(
                "no private key was available, so nothing was decrypted. This \
                 is the part of a backup only root can check, and the part \
                 that says whether the data is any good"
                    .to_string(),
            ),
        };
    };

    let contents = match check_contents(target, id, &head, identity) {
        Ok(checked) => Verdict::Intact { checked },
        Err(verdict) => verdict,
    };
    Verification {
        id: id.clone(),
        presence,
        contents,
    }
}

fn check_contents(
    target: &dyn Target,
    id: &SnapshotId,
    head: &Head,
    identity: &Identity,
) -> Result<String, Verdict> {
    let key = snapshot_key(head, id, identity)?;
    let manifest = read_manifest(target, id, head, &key)?;

    let mut reader = StreamReader::new(target, id, head, &key, Stream::Data);
    let mut seen = 0u64;
    let mut files = 0usize;
    for entry in &manifest.entries {
        if entry.kind == Kind::Directory {
            continue;
        }
        if entry.offset != seen {
            return Err(Verdict::Failed(format!(
                "snapshot {id}'s manifest puts {} at offset {} and the entries \
                 before it account for {seen} bytes",
                entry.path.escape_debug(),
                entry.offset
            )));
        }
        let bytes = reader.take(entry.size)?;
        let got = digest_hex(&bytes);
        if got != entry.digest {
            return Err(Verdict::Failed(format!(
                "snapshot {id}'s {} is {} bytes whose digest is {got}, and the \
                 manifest says {}. The chunk it came out of opened, so this is \
                 not a wrong key — it is this program having written or read \
                 the wrong bytes",
                entry.path.escape_debug(),
                bytes.len(),
                entry.digest
            )));
        }
        seen += entry.size;
        files += 1;
    }
    if seen != manifest.data_bytes {
        return Err(Verdict::Failed(format!(
            "snapshot {id}'s entries account for {seen} bytes and its manifest \
             says the data stream is {}",
            manifest.data_bytes
        )));
    }
    reader.expect_end()?;
    Ok(format!(
        "{files} file(s), {seen} byte(s), every chunk opened and every digest \
         matched"
    ))
}

/// The verdict for an object the head **declares**.
///
/// Different from [`TargetError::verdict`] in exactly one variant, and the
/// difference is the point. `Absent` normally means "the far side answered and
/// holds no such thing", which about a snapshot means nobody backed this up.
/// About a chunk the head says exists it means something else: the snapshot is
/// incomplete. That is a conclusion, so it is `Failed` — reporting it as
/// `Absent` would put a damaged snapshot in the same bucket as one that was
/// never taken.
///
/// Every other variant is unchanged, so a chunk that could not be read is still
/// `CouldNotRun` and is still not evidence of anything.
fn declared_object_verdict(e: &TargetError, what: &str) -> Verdict {
    match e {
        TargetError::Absent(why) => Verdict::Failed(format!(
            "{what} is not there ({why}), and the head declares it. This \
             snapshot is incomplete, which is not the same as a snapshot \
             that was never taken"
        )),
        other => other.verdict(what),
    }
}

fn snapshot_key(
    head: &Head,
    id: &SnapshotId,
    identity: &Identity,
) -> Result<SnapshotKey, Verdict> {
    let ephemeral = head
        .ephemeral_bytes()
        .map_err(|e| Verdict::Failed(e.to_string()))?;
    open_with(identity, &ephemeral, id.as_str())
        .map_err(|e| Verdict::Failed(format!("snapshot {id}: {e}")))
}

fn read_manifest(
    target: &dyn Target,
    id: &SnapshotId,
    head: &Head,
    key: &SnapshotKey,
) -> Result<Manifest, Verdict> {
    let mut reader = StreamReader::new(target, id, head, key, Stream::Manifest);
    let bytes = reader.rest(MAX_MANIFEST_BYTES)?;
    serde_json::from_slice(&bytes).map_err(|e| {
        Verdict::Failed(
            FormatError::BadManifest(format!("{e}"))
                .to_string()
                .replace("{}", &id.to_string()),
        )
    })
}

/// Reads one stream of a snapshot, chunk by chunk, in order.
struct StreamReader<'a> {
    target: &'a dyn Target,
    id: &'a SnapshotId,
    key: &'a SnapshotKey,
    stream: Stream,
    chunks: u32,
    chunk_bytes: u32,
    next: u32,
    buffer: Vec<u8>,
    at: usize,
}

impl<'a> StreamReader<'a> {
    fn new(
        target: &'a dyn Target,
        id: &'a SnapshotId,
        head: &Head,
        key: &'a SnapshotKey,
        stream: Stream,
    ) -> StreamReader<'a> {
        StreamReader {
            target,
            id,
            key,
            stream,
            chunks: match stream {
                Stream::Data => head.data_chunks,
                Stream::Manifest => head.manifest_chunks,
            },
            chunk_bytes: head.chunk_bytes,
            next: 0,
            buffer: Vec::new(),
            at: 0,
        }
    }

    /// Pull one more chunk in. `Ok(false)` means the stream is finished.
    fn fill(&mut self) -> Result<bool, Verdict> {
        if self.next >= self.chunks {
            return Ok(false);
        }
        let index = self.next;
        let last = index + 1 == self.chunks;
        let object = Head::chunk_object(self.stream, index);
        let sealed = self
            .target
            .get(self.id.as_str(), &object)
            .map_err(|e| {
                declared_object_verdict(&e, &format!("{object} of snapshot {}", self.id))
            })?;
        let plain = self
            .key
            .open_chunk(self.stream, u64::from(index), last, &sealed)
            .map_err(|e| Verdict::Failed(format!("snapshot {}: {e}", self.id)))?;
        // Every chunk but the last is exactly the size the head declares.
        //
        // A mutation that made `ChunkWriter` cut chunks one byte short left
        // every round-trip test green, because nothing on the read side
        // depended on the size — the reader simply reassembles whatever
        // arrives. That made `Head::chunk_bytes` a field this build wrote and
        // never used, which is the decorative-check smell this repository has
        // a name for. It is checked now, so a writer that miscounts is caught
        // by the next thing that reads what it wrote.
        if !last && plain.len() != self.chunk_bytes as usize {
            return Err(Verdict::Failed(format!(
                "snapshot {}'s {} chunk {index} holds {} byte(s) and the head \
                 declares {}. Only the last chunk of a stream may be short",
                self.id,
                self.stream.as_str(),
                plain.len(),
                self.chunk_bytes
            )));
        }
        self.buffer.drain(..self.at);
        self.at = 0;
        self.buffer.extend_from_slice(&plain);
        self.next += 1;
        Ok(true)
    }

    fn take(&mut self, want: u64) -> Result<Vec<u8>, Verdict> {
        let want = want as usize;
        while self.buffer.len() - self.at < want {
            if !self.fill()? {
                return Err(Verdict::Failed(format!(
                    "snapshot {}'s {} stream ran out {} byte(s) early. Every \
                     chunk it has opened, so this is a snapshot whose chunks \
                     are all genuine and whose manifest describes more data \
                     than was stored",
                    self.id,
                    self.stream.as_str(),
                    want - (self.buffer.len() - self.at)
                )));
            }
        }
        let out = self.buffer[self.at..self.at + want].to_vec();
        self.at += want;
        Ok(out)
    }

    fn rest(&mut self, limit: usize) -> Result<Vec<u8>, Verdict> {
        while self.fill()? {
            if self.buffer.len() - self.at > limit {
                return Err(Verdict::Failed(
                    FormatError::TooBig {
                        what: "manifest",
                        limit,
                    }
                    .to_string(),
                ));
            }
        }
        Ok(self.buffer[self.at..].to_vec())
    }

    /// Everything the manifest described has been read, and nothing is left.
    fn expect_end(&mut self) -> Result<(), Verdict> {
        let mut left = self.buffer.len() - self.at;
        while self.fill()? {
            left = self.buffer.len() - self.at;
        }
        if left != 0 {
            return Err(Verdict::Failed(format!(
                "snapshot {}'s data stream has {left} byte(s) no manifest entry \
                 accounts for",
                self.id
            )));
        }
        Ok(())
    }
}

// ── restoring ───────────────────────────────────────────────────────────────

/// Options for a restore.
#[derive(Debug, Clone, Default)]
pub struct RestoreOptions {
    /// Write into a destination that already holds something.
    ///
    /// Off by default, and the default is the point: "restore over the thing
    /// you were trying to recover" is a keystroke away from "restore beside
    /// it".
    pub into_non_empty: bool,
}

/// What a restore did.
#[derive(Debug, Clone)]
pub struct Restored {
    pub id: SnapshotId,
    pub files: usize,
    pub directories: usize,
    pub symlinks: usize,
    pub bytes: u64,
    /// What the snapshot itself records as having been missing when it was
    /// taken. Carried through so a restore does not look complete when the
    /// backup never was.
    pub skipped_at_backup: Vec<Skipped>,
    /// The verdict on what was written. Only `Intact` means a restore that
    /// verified.
    pub verdict: Verdict,
}

/// Restore one snapshot into `destination`.
pub fn restore(
    target: &dyn Target,
    id: &SnapshotId,
    identity: &Identity,
    destination: &Path,
    options: &RestoreOptions,
) -> Result<Restored, SessionError> {
    match std::fs::read_dir(destination) {
        Ok(mut entries) => {
            if entries.next().is_some() && !options.into_non_empty {
                return Err(SessionError::Destination {
                    path: destination.to_path_buf(),
                    why: "is not empty. A restore writes a whole tree, and \
                          writing one over a directory that already holds \
                          something is how the thing being recovered gets \
                          overwritten. Pass --into-non-empty if that is really \
                          what you mean, or restore somewhere new and move \
                          afterwards"
                        .to_string(),
                });
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir_all(destination).map_err(|e| SessionError::Destination {
                path: destination.to_path_buf(),
                why: format!("could not be created: {e}"),
            })?;
        }
        Err(e) => {
            return Err(SessionError::Destination {
                path: destination.to_path_buf(),
                why: format!("could not be read: {e}"),
            })
        }
    }

    let head = read_head(target, id).map_err(|v| SessionError::Format(FormatError::BadHead {
        field: "head",
        why: v.to_string(),
    }))?;
    let key = snapshot_key(&head, id, identity).map_err(|v| {
        SessionError::Format(FormatError::BadHead {
            field: "ephemeral",
            why: v.to_string(),
        })
    })?;
    let manifest = read_manifest(target, id, &head, &key).map_err(|v| {
        SessionError::Format(FormatError::BadManifest(v.to_string()))
    })?;

    let mut reader = StreamReader::new(target, id, &head, &key, Stream::Data);
    let mut files = 0usize;
    let mut directories = 0usize;
    let mut symlinks = 0usize;
    let mut bytes = 0u64;
    let mut problems: Vec<String> = Vec::new();

    let mut consumed = 0u64;
    for entry in &manifest.entries {
        // The offset is checked here and not only in `verify`.
        //
        // A mutation that set every entry's offset to zero left the whole
        // round trip green, because this loop reads the data stream
        // sequentially and never looked at the field — so a manifest could
        // disagree with itself about where a file is and a restore would hand
        // back a tree that looked right. `verify` checked it; the path that
        // actually writes files did not, which is the wrong way round.
        if entry.kind != Kind::Directory && entry.offset != consumed {
            problems.push(format!(
                "{} says it starts at {} and the entries before it account for \
                 {consumed} byte(s); this manifest disagrees with itself",
                entry.path.escape_debug(),
                entry.offset
            ));
            continue;
        }
        let relative = match safe_relative(&entry.path) {
            Ok(path) => path,
            Err(why) => {
                // A manifest comes back from storage. A storage that can put
                // `../../etc/cron.d/x` in one can write it as root.
                problems.push(why);
                continue;
            }
        };
        let path = destination.join(&relative);
        match entry.kind {
            Kind::Directory => {
                std::fs::create_dir_all(&path).map_err(|e| SessionError::Destination {
                    path: path.clone(),
                    why: e.to_string(),
                })?;
                directories += 1;
            }
            Kind::Symlink => {
                let raw = reader.take(entry.size).map_err(|v| {
                    SessionError::Format(FormatError::Inconsistent(v.to_string()))
                })?;
                let got = digest_hex(&raw);
                if got != entry.digest {
                    problems.push(format!(
                        "{} is a symlink whose target does not match its digest",
                        entry.path.escape_debug()
                    ));
                    consumed += entry.size;
                    continue;
                }
                let target_text = String::from_utf8_lossy(&raw).into_owned();
                if let Some(parent) = path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                if let Err(why) = clear_the_way(&path, Kind::Symlink) {
                    problems.push(why);
                    consumed += entry.size;
                    continue;
                }
                // A link is recreated exactly as it was, including one that
                // points outside the tree. It is data, not a path this program
                // follows.
                std::os::unix::fs::symlink(&target_text, &path).map_err(|e| {
                    SessionError::Destination {
                        path: path.clone(),
                        why: e.to_string(),
                    }
                })?;
                symlinks += 1;
                bytes += entry.size;
                consumed += entry.size;
            }
            Kind::File => {
                let raw = reader.take(entry.size).map_err(|v| {
                    SessionError::Format(FormatError::Inconsistent(v.to_string()))
                })?;
                let got = digest_hex(&raw);
                if got != entry.digest {
                    // Written anyway? No. A file whose digest does not match is
                    // not the file, and putting it on disk under its own name
                    // is how a restore quietly hands back damage.
                    problems.push(format!(
                        "{} does not match its digest and was not written",
                        entry.path.escape_debug()
                    ));
                    consumed += entry.size;
                    continue;
                }
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| SessionError::Destination {
                        path: parent.to_path_buf(),
                        why: e.to_string(),
                    })?;
                }
                if let Err(why) = clear_the_way(&path, Kind::File) {
                    problems.push(why);
                    consumed += entry.size;
                    continue;
                }
                write_file(&path, &raw, entry.mode).map_err(|e| SessionError::Destination {
                    path: path.clone(),
                    why: e.to_string(),
                })?;
                files += 1;
                bytes += entry.size;
                consumed += entry.size;
            }
        }
    }

    // Modes on directories are set on the way out, after their contents are
    // written: a directory restored 0500 cannot be written into.
    for entry in manifest.entries.iter().rev() {
        if entry.kind != Kind::Directory {
            continue;
        }
        if let Ok(relative) = safe_relative(&entry.path) {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(
                destination.join(relative),
                std::fs::Permissions::from_mode(entry.mode),
            );
        }
    }

    let verdict = if problems.is_empty() {
        match reader.expect_end() {
            Ok(()) => Verdict::Intact {
                checked: format!(
                    "{files} file(s), {directories} director(ies), {symlinks} \
                     symlink(s); every chunk opened and every digest matched"
                ),
            },
            Err(v) => v,
        }
    } else {
        Verdict::Failed(format!(
            "{} entr(ies) were not restored: {}",
            problems.len(),
            problems.join("; ")
        ))
    };

    Ok(Restored {
        id: id.clone(),
        files,
        directories,
        symlinks,
        bytes,
        skipped_at_backup: manifest.skipped.clone(),
        verdict,
    })
}

/// Make room for one restored entry, or say why there is none.
///
/// Only reachable with `--into-non-empty`, since an empty destination has
/// nothing in the way. It exists because the suite found `apex backup restore`
/// failing with `EEXIST` on a symlink that a previous restore had made — and
/// looking at why turned up the worse half:
///
/// > if something at the path is a **symlink** and the entry is a regular
/// > file, `File::create` FOLLOWS it. A destination that already holds
/// > `etc/passwd -> /etc/passwd` would have this program — running as root,
/// > because restoring needs the key — write the snapshot's bytes straight
/// > through it, outside the destination entirely.
///
/// So anything in the way that is a symlink or a file is removed first, and the
/// write itself is `O_NOFOLLOW` as well, so the guarantee does not depend on
/// this function having run. A **directory** in the way is never removed: a
/// restore may overwrite a file, and quietly deleting a tree because a snapshot
/// has a file of that name is a different and much larger thing to do.
fn clear_the_way(path: &Path, want: Kind) -> Result<(), String> {
    let Ok(existing) = std::fs::symlink_metadata(path) else {
        return Ok(());
    };
    if existing.is_dir() {
        return Err(format!(
            "{} is a directory and the snapshot has a {} of that name. This \
             build will not delete a directory to make room for one",
            path.display(),
            match want {
                Kind::File => "file",
                Kind::Symlink => "symbolic link",
                Kind::Directory => "directory",
            }
        ));
    }
    std::fs::remove_file(path)
        .map_err(|e| format!("{} could not be replaced: {e}", path.display()))
}

fn write_file(path: &Path, bytes: &[u8], mode: u32) -> Result<(), std::io::Error> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        // The mode goes on the open, so a private file is never briefly
        // world-readable while its contents are written.
        .mode(mode)
        // O_NOFOLLOW, so this can never write THROUGH a symbolic link that is
        // already at the path. `clear_the_way` removes one first; this is what
        // makes that a defence in depth rather than the only defence.
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    file.write_all(bytes)?;
    Ok(())
}

/// A manifest path, checked before it is joined to anything.
///
/// Refused: absolute paths, `..` anywhere, a root or prefix component, an empty
/// path, and a path with an empty segment. What is left can only land inside
/// the destination.
pub fn safe_relative(path: &str) -> Result<PathBuf, String> {
    if path.is_empty() {
        return Err("the manifest holds an entry with no path".to_string());
    }
    if path.starts_with('/') {
        return Err(format!(
            "{} is absolute, and a restore writes inside its destination",
            path.escape_debug()
        ));
    }
    if path.split('/').any(str::is_empty) {
        return Err(format!(
            "{} has an empty path segment",
            path.escape_debug()
        ));
    }
    let candidate = PathBuf::from(path);
    for component in candidate.components() {
        match component {
            Component::Normal(_) => {}
            Component::CurDir => {
                return Err(format!("{} contains '.'", path.escape_debug()))
            }
            Component::ParentDir => {
                return Err(format!(
                    "{} climbs out of the destination with '..', which is how a \
                     manifest from a far side would have this program write \
                     wherever it liked",
                    path.escape_debug()
                ))
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(format!("{} is not relative", path.escape_debug()))
            }
        }
    }
    Ok(candidate)
}

#[cfg(test)]
mod tests;
