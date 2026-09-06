//! Where the secret service keeps its files, and why none of them is in `$HOME`.
//!
//! Acceptance criterion one for §11 is "secrets are no longer stored in
//! agent-readable home paths". The agent runs as the owner and can read
//! anything the owner can read, so a mode-`0600` file under `$XDG_STATE_HOME`
//! does not satisfy that — it is protected from other accounts, which is not
//! the threat. The store therefore lives under a system state directory owned
//! by the service's own uid:
//!
//! ```text
//! /var/lib/apex-secretd/                 0700  apex-secret
//!   owners/<uid>/secrets/<name>.json     0600  metadata, never a value
//!   owners/<uid>/secrets/<name>.blob     0600  the sealed value
//!   owners/<uid>/grants.json             0600
//!   owners/<uid>/audit.jsonl             0600
//! /run/apex-secretd/broker.sock          0666  authorised by SO_PEERCRED uid
//! /run/apex-secretd/admin.sock           0600  the daemon's own uid, and root
//! ```
//!
//! `systemd` creates both roots (`StateDirectory=`, `RuntimeDirectory=`) with
//! the right owner before the daemon starts. Both are overridable on the
//! command line so the daemon can be run by an ordinary user in a temporary
//! directory — which is how the wire test drives it without root and without
//! touching the machine's real service.

use std::io;
use std::path::{Path, PathBuf};

/// The packaged state root. Matches `StateDirectory=apex-secretd`.
pub const DEFAULT_STATE_DIR: &str = "/var/lib/apex-secretd";

/// The packaged runtime root. Matches `RuntimeDirectory=apex-secretd`.
pub const DEFAULT_RUNTIME_DIR: &str = "/run/apex-secretd";

/// The two roots the daemon works from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    state: PathBuf,
    runtime: PathBuf,
}

impl Default for Layout {
    fn default() -> Layout {
        Layout {
            state: PathBuf::from(DEFAULT_STATE_DIR),
            runtime: PathBuf::from(DEFAULT_RUNTIME_DIR),
        }
    }
}

impl Layout {
    pub fn new(state: impl Into<PathBuf>, runtime: impl Into<PathBuf>) -> Layout {
        Layout {
            state: state.into(),
            runtime: runtime.into(),
        }
    }

    pub fn state_dir(&self) -> &Path {
        &self.state
    }

    pub fn runtime_dir(&self) -> &Path {
        &self.runtime
    }

    /// The socket agents and the CLI use. Metadata and operations; no mutation.
    pub fn broker_socket(&self) -> PathBuf {
        self.runtime.join("broker.sock")
    }

    /// The socket that stores, rotates, grants and revokes.
    pub fn admin_socket(&self) -> PathBuf {
        self.runtime.join("admin.sock")
    }

    /// One directory per owning uid. Separate directories rather than one file
    /// with a uid column: a bug in name handling then cannot reach across
    /// accounts, because the account is a path component the caller never
    /// supplies.
    pub fn owner_dir(&self, uid: u32) -> PathBuf {
        self.state.join("owners").join(uid.to_string())
    }

    pub fn secrets_dir(&self, uid: u32) -> PathBuf {
        self.owner_dir(uid).join("secrets")
    }

    pub fn meta_file(&self, uid: u32, name: &str) -> PathBuf {
        self.secrets_dir(uid).join(format!("{name}.json"))
    }

    pub fn blob_file(&self, uid: u32, name: &str) -> PathBuf {
        self.secrets_dir(uid).join(format!("{name}.blob"))
    }

    pub fn grants_file(&self, uid: u32) -> PathBuf {
        self.owner_dir(uid).join("grants.json")
    }

    pub fn audit_log(&self, uid: u32) -> PathBuf {
        self.owner_dir(uid).join("audit.jsonl")
    }
}

/// Create `dir`, and every missing parent, `0700`.
///
/// Every level, not just the leaf, and that distinction is a real bug this
/// closes: `create_dir_all` applies the process umask, so creating
/// `owners/1000/secrets` and tightening only `secrets` leaves `owners/1000`
/// world-executable. A directory a stranger can traverse is a directory whose
/// contents they can `stat` and whose names they can guess — and the names here
/// say which providers this user has credentials for.
///
/// Directories that already existed are left alone. The service's own state
/// root is created by `systemd`, and walking up chmodding `/var/lib` would be
/// worse than the problem.
pub fn ensure_private_dir(dir: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    // Deepest-first, so `missing` ends up ordered shallowest-first.
    let mut missing: Vec<&Path> = Vec::new();
    let mut cursor = Some(dir);
    while let Some(path) = cursor {
        if path.exists() {
            break;
        }
        missing.push(path);
        cursor = path.parent();
    }

    if missing.is_empty() {
        // It is already there. Tighten it anyway: a store directory that was
        // loosened by hand must not stay loose.
        let mut perms = std::fs::metadata(dir)?.permissions();
        if perms.mode() & 0o777 != 0o700 {
            perms.set_mode(0o700);
            std::fs::set_permissions(dir, perms)?;
        }
        return Ok(());
    }

    for path in missing.into_iter().rev() {
        std::fs::create_dir(path)?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Write bytes to `path` atomically, at `mode`.
///
/// Through a temporary file and a rename, so a crash mid-write leaves the old
/// record rather than a truncated one. The mode is on the `open`, not applied
/// afterwards, so the file is never briefly world-readable.
pub fn write_private(path: &Path, bytes: &[u8], mode: u32) -> io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    if let Some(parent) = path.parent() {
        ensure_private_dir(parent)?;
    }
    let tmp = path.with_extension("tmp");
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(mode)
        .open(&tmp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_the_service_owns_is_under_a_home_directory() {
        // Acceptance criterion one, as an assertion rather than a claim.
        let l = Layout::default();
        for p in [
            l.state_dir().to_path_buf(),
            l.owner_dir(1000),
            l.secrets_dir(1000),
            l.meta_file(1000, "github"),
            l.blob_file(1000, "github"),
            l.grants_file(1000),
            l.audit_log(1000),
        ] {
            let s = p.to_string_lossy().into_owned();
            assert!(p.is_absolute(), "{s}");
            assert!(s.starts_with(DEFAULT_STATE_DIR), "{s}");
            for home in ["/home/", "/var/home/", "/root/", ".local/state", ".config"] {
                assert!(!s.contains(home), "{s} is inside an agent-readable path");
            }
        }
    }

    #[test]
    fn each_owner_gets_its_own_directory() {
        let l = Layout::default();
        assert_ne!(l.owner_dir(1000), l.owner_dir(1001));
        assert_ne!(l.audit_log(1000), l.audit_log(0));
        // The uid is a path component, so a name cannot reach another owner
        // even if name validation were bypassed: it would still be resolved
        // under the peer's own directory.
        assert!(l
            .meta_file(1000, "github")
            .starts_with(l.owner_dir(1000)));
    }

    #[test]
    fn the_two_sockets_are_distinct_and_under_the_runtime_root() {
        let l = Layout::default();
        assert_ne!(l.broker_socket(), l.admin_socket());
        assert!(l.broker_socket().starts_with("/run/apex-secretd"));
        assert!(l.admin_socket().starts_with("/run/apex-secretd"));
    }

    #[test]
    fn a_private_write_is_atomic_and_never_world_readable() {
        use std::os::unix::fs::PermissionsExt;

        let base = std::env::temp_dir().join(format!("apex-secret-paths-{}", std::process::id()));
        let file = base.join("nested/record.json");
        write_private(&file, b"{}", 0o600).expect("write");
        let mode = std::fs::metadata(&file).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "mode was {:o}", mode & 0o777);
        let dir = std::fs::metadata(file.parent().unwrap()).unwrap().permissions().mode();
        assert_eq!(dir & 0o777, 0o700, "mode was {:o}", dir & 0o777);

        // Rewriting replaces the content and leaves no temporary behind.
        write_private(&file, b"{\"a\":1}", 0o600).expect("rewrite");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "{\"a\":1}");
        assert!(!file.with_extension("tmp").exists());

        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn ensure_private_dir_tightens_a_loosened_directory() {
        use std::os::unix::fs::PermissionsExt;

        let base = std::env::temp_dir().join(format!("apex-secret-mode-{}", std::process::id()));
        std::fs::remove_dir_all(&base).ok();
        ensure_private_dir(&base).expect("create");
        std::fs::set_permissions(&base, std::fs::Permissions::from_mode(0o755)).unwrap();
        ensure_private_dir(&base).expect("re-create");
        let mode = std::fs::metadata(&base).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700, "mode was {:o}", mode & 0o777);
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn every_directory_created_on_the_way_down_is_private_not_just_the_last() {
        use std::os::unix::fs::PermissionsExt;

        // The bug this caught: `create_dir_all` applies the umask, so
        // tightening only the leaf left `owners/<uid>` at 0755 — traversable by
        // anyone, and the names inside say which providers a user holds
        // credentials for.
        let base = std::env::temp_dir().join(format!("apex-secret-deep-{}", std::process::id()));
        std::fs::remove_dir_all(&base).ok();
        std::fs::create_dir_all(&base).unwrap();
        // A pre-existing, deliberately loose root, standing in for /var/lib.
        std::fs::set_permissions(&base, std::fs::Permissions::from_mode(0o755)).unwrap();

        let leaf = base.join("owners/1000/secrets");
        ensure_private_dir(&leaf).expect("create");
        for p in [base.join("owners"), base.join("owners/1000"), leaf] {
            let mode = std::fs::metadata(&p).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o700, "{} was {:o}", p.display(), mode & 0o777);
        }
        // The directory that already existed is not touched: walking up and
        // chmodding /var/lib would be worse than the problem.
        let root = std::fs::metadata(&base).unwrap().permissions().mode();
        assert_eq!(root & 0o777, 0o755, "an existing parent was modified");

        std::fs::remove_dir_all(&base).ok();
    }
}
