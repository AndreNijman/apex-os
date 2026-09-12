//! Four answers about a snapshot, and the reason there are four and not two.
//!
//! This is `apex/src/verify.rs`'s [`Verdict`] applied to backups, deliberately
//! and almost field for field. That module's sentence — *"Any of them failing
//! to run … is not a failure, and is not a pass either"* — is the one this
//! repository has had to learn about sixteen times, and a backup system is
//! where getting it wrong is worst:
//!
//! * **a directory that cannot be read is not a directory with no backups in
//!   it.** An unreadable target reported as `Absent` tells an operator their
//!   backups are missing when they are fine, or — the dangerous direction —
//!   lets `apex backup list` show an empty history for a target that is merely
//!   not mounted, right before someone decides the data is not being kept and
//!   turns something off.
//! * **a restore that could not verify is not a restore that succeeded.** The
//!   only honest report of "I wrote the files but could not check them" is a
//!   verdict that is neither pass nor failure.
//!
//! The distinction is not free: every call site that produces a verdict has to
//! know which of the four it is holding, and an `io::Error` does not say. That
//! is what [`from_io`] is for — it reads the errno and refuses to guess.

use std::fmt;

use serde_json::{Map, Value};

/// What is known about one snapshot, or one target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Every check ran and every one of them passed.
    Intact {
        /// What was actually checked, in the words a person needs.
        checked: String,
    },
    /// The target answered, and holds no such snapshot. Nobody backed this up.
    /// That is not the same as a backup that is damaged.
    Absent(String),
    /// A check ran to a conclusion and the conclusion is no.
    Failed(String),
    /// A check did not run to a conclusion. Never a pass, never a failure.
    CouldNotRun(String),
}

impl Verdict {
    pub fn as_str(&self) -> &'static str {
        match self {
            Verdict::Intact { .. } => "intact",
            Verdict::Absent(_) => "absent",
            Verdict::Failed(_) => "failed",
            Verdict::CouldNotRun(_) => "could-not-run",
        }
    }

    /// Whether this verdict may be relied on as "the data is there".
    ///
    /// Exactly one variant answers yes, and the method exists so that no call
    /// site writes `!matches!(v, Verdict::Failed(_))` — which is the shape of
    /// the bug this whole module is about.
    pub fn is_intact(&self) -> bool {
        matches!(self, Verdict::Intact { .. })
    }

    pub fn reason(&self) -> Option<&str> {
        match self {
            Verdict::Intact { .. } => None,
            Verdict::Absent(w) | Verdict::Failed(w) | Verdict::CouldNotRun(w) => Some(w),
        }
    }

    pub fn to_json(&self) -> Value {
        let mut m = Map::new();
        m.insert("state".into(), Value::from(self.as_str()));
        if let Some(w) = self.reason() {
            m.insert("reason".into(), Value::from(w));
        }
        if let Verdict::Intact { checked } = self {
            m.insert("checked".into(), Value::from(checked.clone()));
        }
        Value::Object(m)
    }

    /// The worse of two verdicts, for rolling many objects into one answer.
    ///
    /// The order is `Failed` worst, then `CouldNotRun`, then `Absent`, then
    /// `Intact`. `CouldNotRun` above `Absent` is the load-bearing half: if one
    /// object of a snapshot could not be read and another is genuinely missing,
    /// the honest summary is that the snapshot could not be established, not
    /// that part of it is missing — because the unreadable one might have been
    /// missing too, or might have been fine.
    pub fn worse_of(self, other: Verdict) -> Verdict {
        fn rank(v: &Verdict) -> u8 {
            match v {
                Verdict::Failed(_) => 3,
                Verdict::CouldNotRun(_) => 2,
                Verdict::Absent(_) => 1,
                Verdict::Intact { .. } => 0,
            }
        }
        if rank(&other) > rank(&self) {
            other
        } else {
            self
        }
    }
}

impl fmt::Display for Verdict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Verdict::Intact { checked } => write!(f, "intact — {checked}"),
            Verdict::Absent(w) => write!(f, "no backup there — {w}"),
            Verdict::Failed(w) => write!(f, "DOES NOT VERIFY — {w}"),
            // The wording is `verify.rs`'s, on purpose. An operator who has
            // read one of these reports has read both.
            Verdict::CouldNotRun(w) => write!(f, "not checked — {w}"),
        }
    }
}

/// Turn a filesystem error into the right one of the four.
///
/// The whole module exists for this function. `ENOENT` — and only `ENOENT`,
/// plus `ENOTDIR`, which is what a path component that is a file gives — means
/// the thing is not there. **Everything else means the question was not
/// answered**, and `EACCES` is the one that matters: a permission denial says
/// nothing at all about whether a backup exists behind it.
///
/// `what` names the thing in the words the operator needs, e.g.
/// "the snapshot directory".
pub fn from_io(what: &str, err: &std::io::Error) -> Verdict {
    match err.kind() {
        std::io::ErrorKind::NotFound => Verdict::Absent(format!("{what} does not exist")),
        std::io::ErrorKind::PermissionDenied => Verdict::CouldNotRun(format!(
            "{what} could not be opened: {err}. That is a refusal and not an \
             absence — there may be a perfectly good backup behind it"
        )),
        _ => {
            // `ENOTDIR` has no `ErrorKind` of its own on this toolchain, so it
            // is matched by errno. It means a component of the path is a file,
            // which is a real answer: nothing of ours is there.
            if err.raw_os_error() == Some(libc::ENOTDIR) {
                Verdict::Absent(format!(
                    "{what} is not there: something on the way to it is a file"
                ))
            } else {
                Verdict::CouldNotRun(format!("{what} could not be read: {err}"))
            }
        }
    }
}

#[cfg(test)]
mod tests;
