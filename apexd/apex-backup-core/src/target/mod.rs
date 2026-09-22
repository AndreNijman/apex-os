//! Where a snapshot is kept.
//!
//! A target does four things and knows nothing about encryption: put an object,
//! get an object, list the snapshots, and say whether it is ready. That is the
//! whole seam, and it is why the same snapshot bytes go to a directory, a NAS
//! mount, another machine over ssh and a bucket without five formats
//! existing.
//!
//! # The five kinds P2-001 names, and which are real here
//!
//! | kind | state |
//! |---|---|
//! | `local` | a directory on this machine. Real. |
//! | `nas` | a directory on a mounted filesystem, with the mount's identity checked. Real. |
//! | `r2` | an R2 bucket, through the `apex-secretd` broker. Real. |
//! | `ssh` | a directory on another machine, over a real `ssh`. Real. |
//! | `s3` | an S3 bucket, or any S3-compatible endpoint, through the broker. Real. |
//!
//! All five are carried as of P2-001's second round; `ssh` and `s3` were
//! declared-and-refusing before it, each naming what it would take. The
//! refusal machinery stays anyway — [`Kind::unimplemented_reason`] still
//! exists and [`crate::config::BackupConfig`] still checks it while reading
//! the file — because a target that silently does nothing is the worst defect
//! this subsystem could ship, and the next kind anybody adds should have
//! somewhere to be honest before it works.
//!
//! # Why a missing object is not an error
//!
//! [`Target::get`] distinguishes three answers, because the caller has to build
//! a [`crate::Verdict`] out of them and the difference between "the far side
//! says there is no such object" and "the far side did not say" is the whole
//! point of that type.

use std::fmt;

use crate::format::SnapshotId;
use crate::verdict::Verdict;

pub mod bucket;
pub mod fs;
pub mod ssh;

pub use bucket::BucketTarget;
pub use fs::FsTarget;
pub use ssh::SshTarget;

/// Why a target could not do what was asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TargetError {
    /// The target answered, and holds no such thing. A real answer.
    Absent(String),
    /// The target refused. It did **not** say whether the thing is there, and
    /// this variant exists so that no caller can mistake the two.
    Denied(String),
    /// The question did not reach a conclusion: unreachable, timed out, a
    /// mount that is not mounted, a reply that did not parse.
    Unavailable(String),
    /// The caller asked for something this target will not do.
    Refused(String),
}

impl TargetError {
    /// The verdict this error is, about a thing named `what`.
    ///
    /// The single most important function in the module. Exactly one variant
    /// becomes `Absent`; everything else becomes `CouldNotRun`, including
    /// `Denied` — a permission denial is not an absence, and a target that
    /// refused to answer has told you nothing about your backups.
    pub fn verdict(&self, what: &str) -> Verdict {
        match self {
            TargetError::Absent(why) => Verdict::Absent(format!("{what}: {why}")),
            TargetError::Denied(why) => Verdict::CouldNotRun(format!(
                "{what} could not be read: {why}. That is a refusal and not an \
                 absence — whatever is behind it was not looked at"
            )),
            TargetError::Unavailable(why) => {
                Verdict::CouldNotRun(format!("{what} could not be reached: {why}"))
            }
            TargetError::Refused(why) => {
                Verdict::CouldNotRun(format!("{what} was not looked for: {why}"))
            }
        }
    }
}

impl fmt::Display for TargetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TargetError::Absent(why) => write!(f, "{why}"),
            TargetError::Denied(why) => write!(f, "{why}"),
            TargetError::Unavailable(why) => write!(f, "{why}"),
            TargetError::Refused(why) => write!(f, "{why}"),
        }
    }
}

impl std::error::Error for TargetError {}

/// Turn a filesystem error into the right kind of target error.
///
/// The same three-way split [`crate::verdict::from_io`] makes, and it is here
/// rather than there because a target error is what a target returns; keeping
/// one function per boundary is what stops a second, laxer mapping appearing.
pub fn from_io(what: &str, err: &std::io::Error) -> TargetError {
    match err.kind() {
        std::io::ErrorKind::NotFound => TargetError::Absent(format!("{what} does not exist")),
        std::io::ErrorKind::PermissionDenied => {
            TargetError::Denied(format!("{what}: {err}"))
        }
        _ => {
            if err.raw_os_error() == Some(libc::ENOTDIR) {
                TargetError::Absent(format!(
                    "{what} is not there: something on the way to it is a file"
                ))
            } else {
                TargetError::Unavailable(format!("{what}: {err}"))
            }
        }
    }
}

/// Where a snapshot's objects go, and come back from.
pub trait Target {
    /// What this target is, in the words an operator needs. Never a
    /// credential, and never more of a path than the operator gave.
    fn describe(&self) -> String;

    /// Whether the target is ready to be written to.
    ///
    /// Called once, before the first byte of a run. This is where a NAS that
    /// is not mounted is caught — see [`fs::FsTarget`] — and catching it here
    /// rather than at the first `put` is the difference between a refusal and
    /// half a snapshot in the mountpoint of an unmounted filesystem.
    fn prepare(&self) -> Result<(), TargetError>;

    /// Store one object of one snapshot.
    fn put(&self, snapshot: &str, object: &str, bytes: &[u8]) -> Result<(), TargetError>;

    /// Fetch one object of one snapshot.
    fn get(&self, snapshot: &str, object: &str) -> Result<Vec<u8>, TargetError>;

    /// Every snapshot id this target holds, oldest first.
    ///
    /// **An error here is never an empty list.** That is the shape of the
    /// defect this whole crate is careful about: a target that could not be
    /// listed reported as a target with no backups in it.
    fn list(&self) -> Result<Vec<SnapshotId>, TargetError>;
}

/// The kinds a project may name, including the two that refuse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Local,
    Nas,
    Ssh,
    S3,
    R2,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Local => "local",
            Kind::Nas => "nas",
            Kind::Ssh => "ssh",
            Kind::S3 => "s3",
            Kind::R2 => "r2",
        }
    }

    /// Every kind this build's vocabulary contains, in the order §13.5 lists
    /// them. `apex backup targets` prints this, so an operator can see that
    /// `ssh` is a name this build knows and refuses rather than a name it has
    /// never heard of.
    pub const ALL: [Kind; 5] = [Kind::Local, Kind::Nas, Kind::Ssh, Kind::S3, Kind::R2];

    pub fn parse(text: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|k| k.as_str() == text)
    }

    /// Whether this build can actually carry a snapshot there.
    pub fn is_implemented(self) -> bool {
        matches!(
            self,
            Kind::Local | Kind::Nas | Kind::Ssh | Kind::S3 | Kind::R2
        )
    }

    /// Why not, in the words the operator needs, for any kind this build does
    /// not carry.
    ///
    /// **Every one of §13.5's five is carried now**, so this answers `None` for
    /// all of them — and it stays, rather than being deleted with the last
    /// refusal, because the shape it enforces is the thing that matters: a
    /// target kind that parsed and then did nothing at run time would be the
    /// worst defect this subsystem could ship, and a build that adds a sixth
    /// name has somewhere to put the reason it is not ready.
    ///
    /// `every_unimplemented_kind_says_what_is_missing_and_every_implemented_one_says_nothing`
    /// holds the two halves together, so a kind cannot be implemented and carry
    /// a reason, or be unimplemented and carry none.
    pub fn unimplemented_reason(self) -> Option<&'static str> {
        match self {
            Kind::Local | Kind::Nas | Kind::Ssh | Kind::S3 | Kind::R2 => None,
        }
    }
}

impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests;
