//! A bucket, through the `apex-secretd` broker: R2 (P2-002) and S3 (P2-001).
//!
//! One target and not two. R2 and S3 differ here in exactly three things — the
//! two operation ids this spends and the word it calls itself — so an [`Ops`]
//! carries them and everything below is shared. The alternative was a second
//! module that was this one with two strings changed, which is the kind of
//! duplication that agrees with itself until one of the two is fixed.
//!
//! The `s3` provider renders a bucket listing as the **same** envelope the R2
//! operation answers with, deliberately, so that even the listing parser below
//! is one parser and not two. See `apex-secretd/src/providers/s3/mod.rs`.
//!
//! Everything below was written for R2 and every constraint it documents is
//! equally true of the S3 path: the same 3 MiB brokered reply cap, the same
//! `String::from_utf8_lossy`, the same `valid_name` rule on the staging
//! directory.
//!
//! This target holds no credential and cannot. It asks `apex-secretd` to
//! perform `cloudflare.r2.object.read` and `cloudflare.r2.object.write`, and
//! the daemon spends the token inside a `curl` child it owns, reading it off a
//! pipe as a config file so it never reaches an argument list. What comes back
//! is the operation's output with the credential scrubbed out of it. That is
//! P2-002's first criterion — a bucket-scoped brokered credential — and none of
//! it is this file's work: it landed with P1-005.
//!
//! What *is* this file's work is fitting a backup inside three properties of
//! that path, each of which was read out of the landed code rather than
//! assumed.
//!
//! # 1. The bucket must be one the project bound
//!
//! `Binding::bucket()` resolves every R2 operation's bucket through
//! `[cloudflare] buckets` in the project's own `apex.toml` and refuses anything
//! else — including `bucket.create`, so an agent cannot create a bucket the
//! owner never wrote down. This target therefore does not need, and does not
//! have, a bucket check of its own. It names a bucket and the daemon decides.
//!
//! # 2. An object is a base64 line, not raw ciphertext
//!
//! `apex-secretd`'s `broker::run_curl` ends with
//! `String::from_utf8_lossy(&out.stdout)`. Raw ciphertext fetched back out of
//! R2 would have every invalid UTF-8 sequence **silently replaced with
//! U+FFFD** — a restore that returns corrupt bytes and no error anywhere. So a
//! chunk object is base64 of the sealed chunk. The 4/3 expansion is one of the
//! three numbers that set [`crate::format::CHUNK_BYTES`].
//!
//! # 3. An upload is a file in the project, and the staging directory cannot
//!    start with a dot
//!
//! `cloudflare.r2.object.write` takes a `file` naming something inside the
//! project, read through `project::read_file`'s `O_NOFOLLOW` walk with an owner
//! check. There is a bytes path — `Request::Use` frames a body — but its
//! `MAX_BODY_BYTES` is 1 MiB against the file path's 10 MiB, and using it would
//! change a landed operation's contract. So chunks are staged in the project
//! and uploaded from there.
//!
//! The staging directory is `_apex-backup/` and **not** `.apex-backup/`:
//! `operation::valid_name` requires the first byte of every path segment to be
//! alphanumeric or `_`, so a dot-prefixed directory is refused by the framework
//! before the provider ever sees it. Each staged file is removed as soon as its
//! upload returns.
//!
//! # Why a failed fetch is never reported as "no such object"
//!
//! **The broker does not surface the HTTP status.** `CloudflareProvider::
//! perform` ends `Ok(Performed { code: i32::from(!reply.ok()), … })` — one bit,
//! zero or one, plus the body. A 404, a 403, a 500 and a proxy's HTML error
//! page arrive identically.
//!
//! So [`BucketTarget::get`] cannot tell "there is no such object" from "the
//! credential was refused", and it does not try. Every failed fetch is
//! [`TargetError::Unavailable`], which becomes `CouldNotRun`. Absence is
//! established by [`BucketTarget::list`] instead, which is a *successful* call
//! whose answer is the set of keys that exist — a real answer from the far
//! side, and the only one available here.
//!
//! Guessing from the body was the alternative and it is refused on this
//! project's own terms: it would mean matching a far side's error text to
//! decide whether a backup exists, and being wrong in the direction that says
//! "you have no backups". If the broker grows a status field, this is the
//! module that should use it.

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use super::{Target, TargetError};
use crate::format::SnapshotId;

/// The directory inside the project that chunks are staged in on their way up.
///
/// Underscore and not a dot; see the module note.
pub const STAGING_DIR: &str = "_apex-backup";

/// What this target needs from `apex-secretd`.
///
/// A trait, so that the object-key construction, the base64 framing and the
/// listing's JSON can be exercised without a daemon, a socket or an account —
/// and so that the *end-to-end* suite, which does run a real daemon against a
/// loopback double, is testing the wiring rather than re-testing this logic.
pub trait Broker {
    /// Perform one capability, returning its output with the credential
    /// already scrubbed out by the daemon.
    fn perform(
        &mut self,
        operation: &str,
        resource: &str,
        params: &[(&str, &str)],
    ) -> Result<String, TargetError>;
}

/// An R2 bucket as a backup target.
/// The two operation ids a brokered target spends, and what it calls itself.
///
/// A struct rather than a generic parameter or an enum: the only thing that
/// varies between R2 and S3 here is three strings, and a type that made the
/// difference look larger than it is would invite the two paths to drift.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ops {
    pub read: &'static str,
    pub write: &'static str,
    /// The word an operator sees in `apex backup list` and in a refusal.
    pub label: &'static str,
}

/// Cloudflare R2, through `cloudflare.r2.object.*`. P2-002.
pub const R2: Ops = Ops {
    read: "cloudflare.r2.object.read",
    write: "cloudflare.r2.object.write",
    label: "r2",
};

/// S3, and every S3-compatible endpoint, through `s3.object.*`. P2-001.
pub const S3: Ops = Ops {
    read: "s3.object.read",
    write: "s3.object.write",
    label: "s3",
};

pub struct BucketTarget<B: Broker> {
    ops: Ops,
    broker: RefCell<B>,
    bucket: String,
    prefix: String,
    project: PathBuf,
}

impl<B: Broker> BucketTarget<B> {
    /// An R2 bucket.
    pub fn r2(broker: B, bucket: &str, prefix: &str, project: &Path) -> BucketTarget<B> {
        BucketTarget::new(R2, broker, bucket, prefix, project)
    }

    /// An S3 bucket, at whatever endpoint the credential was stored for.
    pub fn s3(broker: B, bucket: &str, prefix: &str, project: &Path) -> BucketTarget<B> {
        BucketTarget::new(S3, broker, bucket, prefix, project)
    }

    pub fn new(
        ops: Ops,
        broker: B,
        bucket: &str,
        prefix: &str,
        project: &Path,
    ) -> BucketTarget<B> {
        BucketTarget {
            ops,
            broker: RefCell::new(broker),
            bucket: bucket.to_string(),
            prefix: prefix.to_string(),
            project: project.to_path_buf(),
        }
    }

    /// The R2 object key for one object of one snapshot.
    pub fn key(&self, snapshot: &str, object: &str) -> String {
        format!("{}/{}/{}", self.prefix, snapshot, object)
    }

    /// What the capability framework is handed: `<bucket>/<key>`.
    fn resource(&self, snapshot: &str, object: &str) -> String {
        format!("{}/{}", self.bucket, self.key(snapshot, object))
    }

    /// Where a chunk is staged, relative to the project root.
    fn staged_relative(&self, snapshot: &str, object: &str) -> String {
        format!("{STAGING_DIR}/{snapshot}/{object}")
    }

    fn staged_absolute(&self, snapshot: &str, object: &str) -> PathBuf {
        self.project.join(self.staged_relative(snapshot, object))
    }

    /// Pull the object keys out of an R2 bucket listing.
    ///
    /// The reply is Cloudflare's envelope, `{"result":[{"key":…}],…}`. A reply
    /// that does not parse is [`TargetError::Unavailable`] — the bucket was not
    /// listed, which is not the same as the bucket being empty, and a listing
    /// that fell back to "no keys" would report exactly the wrong thing.
    fn keys_from_listing(body: &str) -> Result<Vec<String>, TargetError> {
        let value: serde_json::Value = serde_json::from_str(body).map_err(|e| {
            TargetError::Unavailable(format!(
                "the bucket listing did not parse as JSON ({e}), so this build \
                 does not know what is in the bucket. It is not reporting an \
                 empty one"
            ))
        })?;
        let Some(result) = value.get("result").and_then(|r| r.as_array()) else {
            return Err(TargetError::Unavailable(
                "the bucket listing has no 'result' array, so this build does \
                 not know what is in the bucket. It is not reporting an empty \
                 one"
                    .to_string(),
            ));
        };
        Ok(result
            .iter()
            .filter_map(|o| o.get("key").and_then(|k| k.as_str()))
            .map(str::to_string)
            .collect())
    }

    /// The snapshot ids in a set of keys under this target's prefix.
    fn ids_from_keys(&self, keys: &[String]) -> Vec<SnapshotId> {
        let want = format!("{}/", self.prefix);
        let mut ids: Vec<SnapshotId> = keys
            .iter()
            .filter_map(|k| k.strip_prefix(&want))
            .filter_map(|rest| rest.split('/').next())
            // Ids come back from a far side, so they are parsed rather than
            // trusted: a key this build did not write is not turned into a
            // snapshot id by being in the right place.
            .filter_map(SnapshotId::parse)
            .collect();
        ids.sort();
        ids.dedup();
        ids
    }
}

impl<B: Broker> Target for BucketTarget<B> {
    fn describe(&self) -> String {
        // The bucket and the prefix. Not the account id, which is the
        // project's business, and not the credential's name.
        format!("{} bucket {} under {}/", self.ops.label, self.bucket, self.prefix)
    }

    fn prepare(&self) -> Result<(), TargetError> {
        // Listing the bucket is the cheapest call that proves all three of the
        // things a run depends on: the credential is stored, the project binds
        // this bucket, and the far side answers. Doing it here means an
        // unbound bucket or a refused token is a refusal before the first
        // chunk is sealed rather than a half-written snapshot.
        self.broker
            .borrow_mut()
            .perform(self.ops.read, &self.bucket, &[])?;

        let staging = self.project.join(STAGING_DIR);
        std::fs::create_dir_all(&staging)
            .map_err(|e| super::from_io("the staging directory", &e))?;
        Ok(())
    }

    fn put(&self, snapshot: &str, object: &str, bytes: &[u8]) -> Result<(), TargetError> {
        let text = data_encoding::BASE64.encode(bytes);
        let staged = self.staged_absolute(snapshot, object);
        if let Some(dir) = staged.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|e| super::from_io("the staging directory", &e))?;
        }
        std::fs::write(&staged, text.as_bytes())
            .map_err(|e| super::from_io("the staged chunk", &e))?;

        let relative = self.staged_relative(snapshot, object);
        let outcome = self.broker.borrow_mut().perform(
            self.ops.write,
            &self.resource(snapshot, object),
            &[("file", &relative)],
        );
        // Removed whether the upload worked or not: a staged chunk left in the
        // project is the plaintext-adjacent file this program has the least
        // excuse for leaving behind, and a failed run must not turn into one.
        let _ = std::fs::remove_file(&staged);
        // And the directories with it, once they are empty. `remove_dir`
        // refuses a directory that still holds something, so this needs no
        // check of its own and cannot take a chunk with it — which is why it
        // is `remove_dir` and emphatically not `remove_dir_all` on a path
        // inside somebody's project.
        if let Some(dir) = staged.parent() {
            let _ = std::fs::remove_dir(dir);
        }
        let _ = std::fs::remove_dir(self.project.join(STAGING_DIR));
        outcome.map(|_| ())
    }

    fn get(&self, snapshot: &str, object: &str) -> Result<Vec<u8>, TargetError> {
        let body = self.broker.borrow_mut().perform(
            self.ops.read,
            &self.resource(snapshot, object),
            &[],
        )?;
        data_encoding::BASE64
            .decode(body.trim().as_bytes())
            .map_err(|_| {
                TargetError::Unavailable(format!(
                    "{object} of snapshot {snapshot} came back from R2 as \
                     something that is not base64, so it is not the object \
                     this build wrote. That may be an error page the broker \
                     handed through, which does not report its status — so \
                     this is not a report that the object is missing"
                ))
            })
    }

    fn list(&self) -> Result<Vec<SnapshotId>, TargetError> {
        let body = self
            .broker
            .borrow_mut()
            .perform(self.ops.read, &self.bucket, &[])?;
        let keys = BucketTarget::<B>::keys_from_listing(&body)?;
        Ok(self.ids_from_keys(&keys))
    }
}

/// The real broker: `apex-secretd` over its socket.
pub struct SecretdBroker {
    service: String,
    project: String,
}

impl SecretdBroker {
    /// `service` is the stored credential's name — what `apex secret add`
    /// called it — and `project` is the absolute project root the grant is
    /// keyed on.
    pub fn new(service: &str, project: &str) -> SecretdBroker {
        SecretdBroker {
            service: service.to_string(),
            project: project.to_string(),
        }
    }
}

impl Broker for SecretdBroker {
    fn perform(
        &mut self,
        operation: &str,
        resource: &str,
        params: &[(&str, &str)],
    ) -> Result<String, TargetError> {
        use apex_secret_core::capability::CapabilityRecord;
        use apex_secret_core::protocol::{ErrorKind, Request, Response};

        let mut record = CapabilityRecord::new(&self.service, operation, resource);
        record.project = Some(self.project.clone());
        for (name, value) in params {
            record.params.insert((*name).to_string(), (*value).to_string());
        }

        let reply = apex_secret_core::client::call(&Request::Use {
            record: Box::new(record),
            body_len: 0,
        })
        .map_err(|e| {
            TargetError::Unavailable(format!(
                "apex-secretd did not answer: {e}. Nothing was sent to \
                 Cloudflare, and nothing is known about what is in the bucket"
            ))
        })?;

        match reply {
            Response::Performed {
                output,
                exit_code: 0,
                ..
            } => Ok(output),
            Response::Performed { output, .. } => Err(TargetError::Unavailable(format!(
                "cloudflare refused {operation} on {resource}. The broker does \
                 not hand back the HTTP status, so this build cannot tell a \
                 missing object from a refused credential and does not guess. \
                 What came back: {}",
                one_line(&output)
            ))),
            // The daemon's own vocabulary maps cleanly, and this is the one
            // place a status-like distinction really is available.
            Response::Error { kind, message } => Err(match kind {
                ErrorKind::PermissionDenied => TargetError::Denied(message),
                ErrorKind::NoSuchService => TargetError::Refused(format!(
                    "{message}. `apex secret add` is what stores a Cloudflare \
                     credential, and `apex secret grant` is what lets this \
                     project spend it"
                )),
                ErrorKind::BadRequest => TargetError::Refused(message),
                ErrorKind::Internal => TargetError::Unavailable(message),
            }),
            other => Err(TargetError::Unavailable(format!(
                "apex-secretd answered '{}', which is not an answer to a \
                 capability request",
                other.variant()
            ))),
        }
    }
}

/// One line of a far side's message, bounded.
///
/// An error body can be a whole HTML page, and it reaches an operator's
/// terminal and this program's own messages.
fn one_line(text: &str) -> String {
    let squashed: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if squashed.chars().count() > 300 {
        let cut: String = squashed.chars().take(300).collect();
        format!("{cut}…")
    } else {
        squashed
    }
}

#[cfg(test)]
mod tests;
