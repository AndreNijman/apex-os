//! The key directory: a public half anyone may read, a private half only root
//! may.
//!
//! ```text
//! /var/lib/apex-backup/                 0755  root
//!   recipients/<uid>.pub                0644  root   the registered recipient
//!   keys/                               0700  root
//!     <uid>.key                         0600  root   what opens a snapshot
//! ```
//!
//! # Why the recipient is registered as well as declared
//!
//! A project declares its recipient in `apex.toml`, which is the pattern
//! `[cloudflare] buckets` established: the project writes down what it means,
//! and the daemon checks it. But `apex.toml` is **owner-writable**, and
//! `apex_secret_core::project` says so in as many words — it is "a guardrail,
//! not a boundary… it does not stop a user from rebinding their own project".
//!
//! For a bucket that is fine. For a backup recipient it is not, and the attack
//! is one line long:
//!
//! > an agent that can edit `apex.toml` writes its own public key into
//! > `recipient`, waits for the next backup, and reads the snapshot it was
//! > handed. "The agent cannot read the raw backup key" stays literally true
//! > and completely hollow — it never needed the key, because the data was
//! > sealed to a key it already had.
//!
//! So the recipient is *registered* by `apex backup key init`, running as root,
//! into a file an unprivileged process cannot write. The declared one is
//! checked against it at every run, and a mismatch refuses. The declaration
//! still earns its place: it is what makes the binding visible in the project,
//! and it is what P2-013's `[identity.*]` sections look like.
//!
//! # Reading the private half is where "could not" must not become "is not"
//!
//! [`KeyStore::identity`] distinguishes three answers, and an unprivileged
//! caller gets `Denied` — not `Absent`. A restore that reported "there is no
//! key on this machine" when the truth is "you are not root" would send an
//! operator looking for a key they are holding.

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use crate::crypto::{CryptoError, Identity, Recipient};
use crate::verdict::Verdict;

/// Where the key directory lives on a running system.
pub const SYSTEM_ROOT: &str = "/var/lib/apex-backup";

/// Why a key could not be read, written or trusted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyError {
    /// There is no key for that account. A real answer.
    Absent { path: PathBuf },
    /// The caller may not read it. **Not** an answer about whether it exists.
    Denied { path: PathBuf, why: String },
    /// The question did not reach a conclusion.
    Unavailable { path: PathBuf, why: String },
    /// There, and not a key.
    Malformed { path: PathBuf, why: String },
    /// A key is already there and this would have replaced it.
    Exists { path: PathBuf },
    /// The project declared a recipient that is not the one registered for
    /// this account.
    NotRegistered { declared: String, registered: String },
    /// The declared recipient did not parse.
    BadRecipient(CryptoError),
}

impl std::fmt::Display for KeyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KeyError::Absent { path } => write!(
                f,
                "there is no backup key at {}. `sudo apex backup key init` \
                 makes one",
                path.display()
            ),
            KeyError::Denied { path, why } => write!(
                f,
                "{} could not be read: {why}. That is a refusal and not an \
                 absence — the key is almost certainly there, and restoring \
                 needs root because the private half is root-owned on purpose",
                path.display()
            ),
            KeyError::Unavailable { path, why } => {
                write!(f, "{} could not be read: {why}", path.display())
            }
            KeyError::Malformed { path, why } => {
                write!(f, "{} is not a backup key: {why}", path.display())
            }
            KeyError::Exists { path } => write!(
                f,
                "{} already holds a key. Replacing it would make every \
                 snapshot sealed to the old one unopenable, so this build will \
                 not do it by accident",
                path.display()
            ),
            KeyError::NotRegistered {
                declared,
                registered,
            } => write!(
                f,
                "this project declares the recipient {declared} and this \
                 machine has {registered} registered. apex.toml is writable by \
                 the account that owns the project — and by anything running as \
                 it — so a recipient it names is a declaration and never an \
                 authority. Sealing to the declared one would hand the snapshot \
                 to whoever wrote that line. If the change is yours, \
                 `sudo apex backup key init` is what registers a new key"
            ),
            KeyError::BadRecipient(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for KeyError {}

impl KeyError {
    /// What this error means for a question about a snapshot.
    pub fn verdict(&self, what: &str) -> Verdict {
        match self {
            KeyError::Absent { .. } => Verdict::Absent(format!("{what}: {self}")),
            KeyError::Denied { .. } | KeyError::Unavailable { .. } => {
                Verdict::CouldNotRun(format!("{what}: {self}"))
            }
            KeyError::Malformed { .. }
            | KeyError::Exists { .. }
            | KeyError::NotRegistered { .. }
            | KeyError::BadRecipient(_) => Verdict::Failed(format!("{what}: {self}")),
        }
    }
}

fn from_io(path: &Path, err: &std::io::Error) -> KeyError {
    match err.kind() {
        std::io::ErrorKind::NotFound => KeyError::Absent {
            path: path.to_path_buf(),
        },
        std::io::ErrorKind::PermissionDenied => KeyError::Denied {
            path: path.to_path_buf(),
            why: err.to_string(),
        },
        _ => {
            if err.raw_os_error() == Some(libc::ENOTDIR) {
                KeyError::Absent {
                    path: path.to_path_buf(),
                }
            } else {
                KeyError::Unavailable {
                    path: path.to_path_buf(),
                    why: err.to_string(),
                }
            }
        }
    }
}

/// The key directory.
pub struct KeyStore {
    root: PathBuf,
}

impl KeyStore {
    pub fn new(root: PathBuf) -> KeyStore {
        KeyStore { root }
    }

    pub fn system() -> KeyStore {
        KeyStore::new(PathBuf::from(SYSTEM_ROOT))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn recipient_path(&self, uid: u32) -> PathBuf {
        self.root.join("recipients").join(format!("{uid}.pub"))
    }

    pub fn identity_path(&self, uid: u32) -> PathBuf {
        self.root.join("keys").join(format!("{uid}.key"))
    }

    /// Make a keypair for an account, refusing to replace one.
    ///
    /// Root's job. Returns the recipient, which is what goes in `apex.toml`.
    pub fn generate(&self, uid: u32) -> Result<Recipient, KeyError> {
        let key_path = self.identity_path(uid);
        if key_path.exists() {
            return Err(KeyError::Exists { path: key_path });
        }
        let identity = Identity::generate().map_err(|e| KeyError::Unavailable {
            path: key_path.clone(),
            why: e.to_string(),
        })?;
        let recipient = identity.recipient();

        // The private directory first, and at 0700 before anything is in it —
        // creating it world-readable and tightening afterwards is a window.
        make_dir(&self.root, 0o755)?;
        make_dir(&self.root.join("keys"), 0o700)?;
        make_dir(&self.root.join("recipients"), 0o755)?;

        write_private(&key_path, identity.expose(), 0o600)?;
        write_private(
            &self.recipient_path(uid),
            recipient.to_string_value().as_bytes(),
            0o644,
        )?;
        Ok(recipient)
    }

    /// The recipient registered for an account.
    pub fn registered(&self, uid: u32) -> Result<Recipient, KeyError> {
        let path = self.recipient_path(uid);
        let text = std::fs::read_to_string(&path).map_err(|e| from_io(&path, &e))?;
        Recipient::parse(text.trim()).map_err(|e| KeyError::Malformed {
            path,
            why: e.to_string(),
        })
    }

    /// The private half. Root only, by construction.
    pub fn identity(&self, uid: u32) -> Result<Identity, KeyError> {
        let path = self.identity_path(uid);
        let bytes = std::fs::read(&path).map_err(|e| from_io(&path, &e))?;
        if bytes.len() != 32 {
            return Err(KeyError::Malformed {
                path,
                why: format!("{} bytes, and an X25519 secret is 32", bytes.len()),
            });
        }
        let mut key = [0u8; 32];
        key.copy_from_slice(&bytes);
        Ok(Identity::from_bytes(key))
    }

    /// Check a project's declared recipient against the registered one.
    ///
    /// The whole reason [`KeyStore`] has a public half at all. See the module
    /// note for the one-line attack this refuses.
    pub fn check_declared(&self, uid: u32, declared: &str) -> Result<Recipient, KeyError> {
        let parsed = Recipient::parse(declared).map_err(KeyError::BadRecipient)?;
        let registered = self.registered(uid)?;
        if parsed != registered {
            return Err(KeyError::NotRegistered {
                declared: parsed.to_string_value(),
                registered: registered.to_string_value(),
            });
        }
        Ok(parsed)
    }
}

fn make_dir(path: &Path, mode: u32) -> Result<(), KeyError> {
    match std::fs::create_dir(path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(from_io(path, &e)),
    }
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
        .map_err(|e| from_io(path, &e))
}

/// Create with the mode already set, then write.
///
/// `File::create` then `set_permissions` leaves the file readable for as long
/// as the write takes, and the thing being written here is a private key. The
/// mode goes on the `open(2)` call.
fn write_private(path: &Path, bytes: &[u8], mode: u32) -> Result<(), KeyError> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(path)
        .map_err(|e| from_io(path, &e))?;
    file.write_all(bytes).map_err(|e| from_io(path, &e))?;
    file.sync_all().map_err(|e| from_io(path, &e))
}

#[cfg(test)]
mod tests;
