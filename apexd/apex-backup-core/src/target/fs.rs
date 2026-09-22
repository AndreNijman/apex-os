//! A directory: `local`, and `nas` with the mount actually checked.
//!
//! # The empty mountpoint
//!
//! This is the defect the `nas` kind exists to refuse, and it is the
//! filesystem's own version of "permission denied is not absence":
//!
//! > `/mnt/nas/backups` is a directory on the machine's root filesystem. When
//! > the NAS is mounted, it is the NAS. When the NAS is **not** mounted, it is
//! > still a perfectly good, perfectly writable, perfectly empty directory on
//! > the local disk.
//!
//! So a backup run against an unmounted NAS succeeds. It writes a complete,
//! verifiable snapshot onto the machine whose loss the backup exists to
//! survive, and `apex backup list` — reading the same unmounted directory —
//! reports a healthy history. Nothing fails. Nothing warns. The first symptom
//! is the restore that was needed.
//!
//! Two checks, and a `nas` target needs both:
//!
//! * **it must be a mount point.** The root's `st_dev` must differ from its
//!   parent's. That alone catches the unmounted case on its own, and costs two
//!   `stat` calls.
//! * **the marker must be the one this project recorded.** `apex backup init`
//!   writes `apex-backup-target.json` holding a random id, and the project's
//!   `apex.toml` records that id. A different filesystem mounted at the same
//!   path is then a refusal rather than a second, silently separate history.
//!   This is the check that survives the case the `st_dev` test does not: a
//!   *different* NAS, or a re-exported share, mounted where the right one used
//!   to be.
//!
//! A missing marker is [`TargetError::Unavailable`] and never
//! [`TargetError::Absent`]. "There is no marker here" is a statement about
//! whether the target was established, not about whether it holds backups.
//!
//! `local` has the marker check too and not the mount check: a `local` target
//! is deliberately on this machine, so requiring a mount would be wrong, but a
//! path typo silently creating a fresh history in the wrong directory is the
//! same failure in a smaller coat.

use std::io::Write;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{from_io, Kind, Target, TargetError};
use crate::format::SnapshotId;

/// The file that says which filesystem this is.
pub const MARKER: &str = "apex-backup-target.json";

/// Largest marker this will read.
const MAX_MARKER_BYTES: u64 = 8 * 1024;

/// What the marker holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Marker {
    /// A random id, 16 lowercase hex characters. Recorded in the project's
    /// `apex.toml`, and compared before anything is written.
    pub id: String,
    /// When `apex backup init` wrote it.
    pub created_ms: u64,
    /// A sentence for whoever finds this file and wonders what it is.
    pub note: String,
}

impl Marker {
    pub fn new(now_ms: u64) -> Result<Marker, std::io::Error> {
        let mut raw = [0u8; 8];
        getrandom::getrandom(&mut raw).map_err(std::io::Error::other)?;
        Ok(Marker {
            id: data_encoding::HEXLOWER.encode(&raw),
            created_ms: now_ms,
            note: "APEX backup target. `apex backup` refuses to write here \
                   unless the project's apex.toml records this id, so that an \
                   unmounted share cannot quietly become a second history."
                .to_string(),
        })
    }
}

/// A directory holding snapshots.
pub struct FsTarget {
    root: PathBuf,
    prefix: String,
    kind: Kind,
    /// The marker id the project recorded, when it has one.
    expect_marker: Option<String>,
}

impl FsTarget {
    /// A `local` target.
    pub fn local(root: PathBuf, prefix: &str, expect_marker: Option<String>) -> FsTarget {
        FsTarget {
            root,
            prefix: prefix.to_string(),
            kind: Kind::Local,
            expect_marker,
        }
    }

    /// A `nas` target: the same directory, plus the mount check.
    pub fn nas(root: PathBuf, prefix: &str, expect_marker: Option<String>) -> FsTarget {
        FsTarget {
            root,
            prefix: prefix.to_string(),
            kind: Kind::Nas,
            expect_marker,
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Where snapshots live under the root.
    fn base(&self) -> PathBuf {
        self.root.join(&self.prefix)
    }

    pub fn marker_path(&self) -> PathBuf {
        self.root.join(MARKER)
    }

    /// Write the marker. `apex backup init`, and nothing else, calls this.
    pub fn write_marker(&self, marker: &Marker) -> Result<(), TargetError> {
        let text = serde_json::to_vec_pretty(marker)
            .map_err(|e| TargetError::Unavailable(format!("serialising the marker: {e}")))?;
        write_atomically(&self.marker_path(), &text, 0o644)
            .map_err(|e| from_io("the target marker", &e))
    }

    /// Read the marker, or say why not.
    pub fn read_marker(&self) -> Result<Marker, TargetError> {
        let path = self.marker_path();
        let meta = std::fs::metadata(&path).map_err(|e| from_io("the target marker", &e))?;
        if meta.len() > MAX_MARKER_BYTES {
            return Err(TargetError::Unavailable(format!(
                "{} is {} bytes, and a marker is a few hundred",
                path.display(),
                meta.len()
            )));
        }
        let text = std::fs::read(&path).map_err(|e| from_io("the target marker", &e))?;
        serde_json::from_slice(&text).map_err(|e| {
            TargetError::Unavailable(format!(
                "{} is not a target marker this build reads: {e}",
                path.display()
            ))
        })
    }

    /// Whether `root` is the root of a mounted filesystem.
    ///
    /// `st_dev` against the parent's. `/` is its own parent and is a mount, so
    /// it is answered directly rather than by the comparison, which would say
    /// no.
    fn is_mount_point(&self) -> Result<bool, TargetError> {
        let here =
            std::fs::metadata(&self.root).map_err(|e| from_io("the target directory", &e))?;
        let Some(parent) = self.root.parent() else {
            return Ok(true);
        };
        if parent.as_os_str().is_empty() {
            return Ok(true);
        }
        let up = std::fs::metadata(parent)
            .map_err(|e| from_io("the directory above the target", &e))?;
        Ok(here.dev() != up.dev())
    }

    /// Every check that does not write anything. Both `prepare` and `list` run
    /// this, so a read path cannot be laxer than a write path — which is how a
    /// listing of an unmounted NAS would otherwise come back empty and calm.
    fn preflight(&self) -> Result<(), TargetError> {
        let meta =
            std::fs::metadata(&self.root).map_err(|e| from_io("the target directory", &e))?;
        if !meta.is_dir() {
            return Err(TargetError::Unavailable(format!(
                "{} is not a directory",
                self.root.display()
            )));
        }

        if self.kind == Kind::Nas && !self.is_mount_point()? {
            return Err(TargetError::Unavailable(format!(
                "{} is not a mount point, so the filesystem you meant is not \
                 mounted. Writing here would put the backup on this machine's \
                 own disk, inside the empty directory the share mounts over — \
                 which succeeds, verifies, and is gone with the machine",
                self.root.display()
            )));
        }

        let Some(expected) = &self.expect_marker else {
            return Ok(());
        };
        let marker = self.read_marker().map_err(|e| match e {
            // A marker that is not there is not a target with no backups in
            // it. It is a target that was never established, or is not
            // mounted, and either way nothing has been looked at.
            TargetError::Absent(_) => TargetError::Unavailable(format!(
                "{} holds no {MARKER}. Either this target was never set up \
                 with `apex backup init`, or what should be mounted there is \
                 not. This is not the same as a target with no snapshots in \
                 it, and it is not reported as one",
                self.root.display()
            )),
            other => other,
        })?;
        if &marker.id != expected {
            return Err(TargetError::Unavailable(format!(
                "{} is target {}, and this project records {}. Something else \
                 is mounted where this project's backups live; writing would \
                 start a second history that nothing would ever look for",
                self.root.display(),
                marker.id,
                expected
            )));
        }
        Ok(())
    }

    fn snapshot_dir(&self, snapshot: &str) -> PathBuf {
        self.base().join(snapshot)
    }
}

impl Target for FsTarget {
    fn describe(&self) -> String {
        format!("{} at {}", self.kind, self.root.display())
    }

    fn prepare(&self) -> Result<(), TargetError> {
        self.preflight()?;
        let base = self.base();
        if let Err(e) = std::fs::create_dir_all(&base) {
            return Err(from_io("the snapshot directory", &e));
        }
        // 0700: the ciphertext is unopenable without the private key, but a
        // backup's *existence and size* are still facts about the owner, and
        // this costs nothing.
        let _ = std::fs::set_permissions(&base, std::fs::Permissions::from_mode(0o700));
        Ok(())
    }

    fn put(&self, snapshot: &str, object: &str, bytes: &[u8]) -> Result<(), TargetError> {
        let dir = self.snapshot_dir(snapshot);
        std::fs::create_dir_all(&dir).map_err(|e| from_io("the snapshot directory", &e))?;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
        write_atomically(&dir.join(object), bytes, 0o600)
            .map_err(|e| from_io(&format!("{object} of snapshot {snapshot}"), &e))
    }

    fn get(&self, snapshot: &str, object: &str) -> Result<Vec<u8>, TargetError> {
        let path = self.snapshot_dir(snapshot).join(object);
        std::fs::read(&path).map_err(|e| from_io(&format!("{object} of snapshot {snapshot}"), &e))
    }

    fn list(&self) -> Result<Vec<SnapshotId>, TargetError> {
        self.preflight()?;
        let base = self.base();
        let entries = match std::fs::read_dir(&base) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // The root is there, readable, and carries the right marker —
                // `preflight` established all three — and the snapshot
                // directory under it is not there. That is a real answer: no
                // snapshot has been written to this target yet.
                return Ok(Vec::new());
            }
            Err(e) => return Err(from_io("the snapshot directory", &e)),
        };

        let mut ids = Vec::new();
        for entry in entries {
            // One unreadable entry is not an empty directory either: an error
            // mid-iteration ends the listing rather than shortening it, because
            // a listing that is quietly short is a history with snapshots
            // missing from it.
            //
            // A mutation replacing this `?` with `continue` left every test in
            // this file green, and the reason is worth writing down rather than
            // hiding: `read_dir` opens the directory once, so the `chmod 000`
            // that `an_unreadable_target_is_could_not_run_and_not_an_empty_history`
            // uses fails at the `read_dir` above and never reaches an
            // iteration step. A `readdir(3)` that fails *after* a successful
            // open needs an I/O error or a stale handle — an NFS mount losing
            // its server mid-listing, which is exactly the case a NAS target
            // meets and exactly the case a fixture cannot stage. So this arm is
            // unreachable from this suite and is here anyway, for the reason
            // the Cloudflare provider keeps its own unreachable `script`
            // check: code that trusted a surrounding guarantee stops being
            // correct the day the guarantee moves.
            let entry = entry.map_err(|e| from_io("an entry of the snapshot directory", &e))?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if let Some(id) = SnapshotId::parse(name) {
                ids.push(id);
            }
        }
        ids.sort();
        Ok(ids)
    }
}

/// Write, then rename. A reader never sees a half-written object.
///
/// The temporary name is in the same directory, because `rename(2)` is only
/// atomic within a filesystem and a `/tmp` staging file would silently become
/// a copy across one.
fn write_atomically(path: &Path, bytes: &[u8], mode: u32) -> Result<(), std::io::Error> {
    let dir = path.parent().unwrap_or(Path::new("."));
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("object");
    let tmp = dir.join(format!(".{name}.partial"));
    {
        let mut file = std::fs::File::create(&tmp)?;
        file.set_permissions(std::fs::Permissions::from_mode(mode))?;
        file.write_all(bytes)?;
        // The rename is atomic; the *contents* reaching the disk before the
        // rename is not, without this.
        file.sync_all()?;
    }
    match std::fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests;
