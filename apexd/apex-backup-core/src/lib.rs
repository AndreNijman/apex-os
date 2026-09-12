//! The encrypted backup framework: format, crypto, targets, restore.
//!
//! Roadmap P2-001, and the half of §13.5 the Cloudflare provider's note said
//! could not be written until a backup format existed:
//!
//! > R2 … "the preferred first-party cloud target for encrypted APEX backups".
//! > That is P1-010's, and the object write below is the half of it that does
//! > not need a backup format to exist first.
//!
//! This crate is the format.
//!
//! # The shape
//!
//! * [`crypto`] — one X25519 sealed box per snapshot and an XChaCha20-Poly1305
//!   chunk AEAD. The public half seals, so backing up needs no privilege; the
//!   private half opens, so restoring needs root.
//! * [`format`] — what a snapshot consists of, independent of where it is kept.
//! * [`verdict`] — four answers about a snapshot, because "I could not look" is
//!   not "there is nothing there".
//! * [`target`] — where a snapshot is kept: a local directory, a NAS mount, an
//!   R2 bucket through the `apex-secretd` broker.
//! * [`keys`] — the root-owned key directory, and the registered recipient a
//!   project's declared one is checked against.
//! * [`session`] — writing a snapshot, listing them, verifying one, restoring
//!   one.
//! * [`config`] — the `[backup]` section of a project's `apex.toml`.
//!
//! # Two disciplines this crate is held to
//!
//! **Permission denied is not absence.** An unreadable target directory is not
//! an empty one; a snapshot whose chunks cannot be fetched has not been shown
//! to be damaged. Every answer about stored data is a [`verdict::Verdict`] with
//! a distinct `CouldNotRun`, and [`verdict::from_io`] is the only place an
//! `io::Error` is turned into one.
//!
//! **Encryption is measured, not asserted.** `tests/test-apex-backup.sh` plants
//! a high-entropy canary in the source tree and fails if it appears in any byte
//! written to the target — chunks and manifest both.

pub mod crypto;
pub mod format;
pub mod verdict;

pub use verdict::Verdict;

/// Unix milliseconds now.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or_default()
}
