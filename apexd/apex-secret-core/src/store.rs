//! The credential store, on disk.
//!
//! ```text
//! /var/lib/apex-secretd/                     0700  root
//!   users/<uid>/                             0700  root
//!     <service>.json                         0600  metadata — never a value
//!     <service>.secret                       0600  the value, raw bytes
//!     grants.json                            0600  which project may do what
//!   audit.jsonl                              0600  append-only
//! ```
//!
//! Three decisions worth the words:
//!
//! **The value is in its own file.** Not a field in the JSON record. That is
//! what lets [`crate::SecretValue`] refuse to implement `Serialize`: nothing in
//! this crate ever hands a credential to serde, so no derived serialisation
//! anywhere can pick one up. It also means `apex secret list`, which reads
//! metadata, never opens the file that holds a token.
//!
//! **Per-uid subdirectories.** Two accounts on one machine get two namespaces,
//! so a service name is not a shared identifier and one user cannot probe for
//! another's. The daemon derives the uid from `SO_PEERCRED`, never from the
//! request.
//!
//! **Grants live here too, not in `$HOME`.** In the agent-runtime broker they
//! were a user-writable JSON file, which meant an unconfined session could
//! grant itself every capability and then use it. Behind the root boundary it
//! cannot: writing a grant now needs the daemon's agreement, and the daemon
//! refuses a mutating request that comes from inside a session.
//!
//! The audit log is one file for the machine rather than one per uid, because
//! it is the record an administrator reads and splitting it per account makes
//! "what happened at 14:02" a join.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::value::SecretValue;

/// A stored credential's metadata. **Never the value.**
///
/// A separate type from the value on purpose: this is what `apex secret list`
/// prints and what a reply carries, and a struct that could hold the secret is
/// a struct that eventually will.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceInfo {
    pub service: String,
    /// Host the credential is valid for. Pinned: a remote pointing anywhere
    /// else is refused at use time.
    pub host: String,
    /// `https`, or `http` for a loopback host.
    #[serde(default = "default_scheme")]
    pub scheme: String,
    /// Username git should send. Most token schemes ignore it.
    #[serde(default = "default_username")]
    pub username: String,
    /// The path on that host the credential's own endpoint lives at, for a
    /// capability whose destination is the service rather than a repository.
    ///
    /// Empty for a git credential: a git operation resolves its URL from the
    /// repository and the host is what gets pinned. An MCP credential has to
    /// carry one, because there is no repository to ask and the caller may not
    /// name a destination — that is the entire reason `mcp-request` has no
    /// arguments.
    #[serde(default)]
    pub path: String,
    /// How the credential is presented over HTTP: `bearer` sends
    /// `Authorization: Bearer <value>`, `raw` sends the value as the whole
    /// header. Git ignores it and authenticates through a credential helper.
    ///
    /// `raw` exists because a server can want `Basic`, or a header shape
    /// nobody here has seen, and a migration that could not carry such a header
    /// would leave it in `~/.claude.json` — which is the file this task is
    /// about emptying.
    #[serde(default = "default_auth")]
    pub auth: String,
    /// The port the endpoint is on, when it is not the scheme's own.
    ///
    /// Separate from [`Self::host`] rather than written into it, because the
    /// host is what a git remote's URL is *pinned* against and that comparison
    /// deliberately ignores ports — a credential for `github.com` must still
    /// match a remote that spells out `:443`. An endpoint the broker dials
    /// itself is the other case, and it needs the port.
    #[serde(default)]
    pub port: Option<u16>,
    /// When it was stored, ms since the epoch.
    #[serde(default)]
    pub added: u64,
}

impl ServiceInfo {
    /// The endpoint this credential is for, as a URL.
    ///
    /// Built from the three pinned fields and nothing else. No caller
    /// contributes to it, which is what makes `mcp-request` safe to offer to a
    /// session at all.
    pub fn url(&self) -> String {
        let path = if self.path.starts_with('/') {
            self.path.clone()
        } else {
            format!("/{}", self.path)
        };
        let host = match self.port {
            // Bracketed for an IPv6 literal, which is what a bare `::1` would
            // otherwise turn into `::1:9000` — a different address.
            Some(port) if self.host.contains(':') => format!("[{}]:{port}", self.host),
            Some(port) => format!("{}:{port}", self.host),
            None => self.host.clone(),
        };
        format!("{}://{}{}", self.scheme, host, path)
    }

    /// The `Authorization` header value for a credential, per [`Self::auth`].
    ///
    /// Takes the value rather than returning one built from it, so the caller
    /// that has the credential is the caller that builds the header, and no
    /// intermediate holds a second copy.
    pub fn header_value(&self, value: &str) -> String {
        match self.auth.as_str() {
            "raw" => value.to_string(),
            _ => format!("Bearer {value}"),
        }
    }
}

fn default_scheme() -> String {
    "https".to_string()
}

fn default_username() -> String {
    "x-access-token".to_string()
}

fn default_auth() -> String {
    "bearer".to_string()
}

/// Whether `path` is one a stored endpoint may carry.
///
/// Absolute or empty, no query and no fragment, no framing characters. It is
/// pinned at `add` time and never revisited, so this is the only place it is
/// judged — and a path that could carry a `?` would let whoever stored it hide
/// a second destination inside one that reads as innocent.
pub fn valid_endpoint_path(path: &str) -> bool {
    if path.is_empty() {
        return true;
    }
    path.starts_with('/')
        && path.len() <= 512
        && !path.contains("..")
        && !path.contains('?')
        && !path.contains('#')
        && !path.contains('@')
        && path
            .chars()
            .all(|c| c.is_ascii_graphic() && !matches!(c, '"' | '\\' | '\''))
}

/// Why a store operation failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreError {
    /// No credential stored under that name.
    NoSuchService(String),
    /// The name is not one this store will accept.
    BadServiceName(String),
    /// `http` was asked for on a host that is not this machine.
    InsecureScheme { host: String, scheme: String },
    /// The value is longer than [`SecretValue::MAX_BYTES`].
    TooLong(usize),
    Io(String),
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::NoSuchService(s) => write!(
                f,
                "no credential stored for '{}'; add one with `apex secret add {}`",
                s.escape_debug(),
                s.escape_debug()
            ),
            StoreError::BadServiceName(s) => write!(
                f,
                "'{}' is not a service name; use letters, digits, '_', '-' and '.'",
                s.escape_debug()
            ),
            StoreError::InsecureScheme { host, scheme } => write!(
                f,
                "{scheme} is only allowed for a loopback host, and {host} is not \
                 one — a credential sent in clear over a network is a credential \
                 you no longer have"
            ),
            StoreError::TooLong(n) => write!(
                f,
                "that credential is {n} bytes; the limit is {}",
                SecretValue::MAX_BYTES
            ),
            StoreError::Io(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for StoreError {}

/// Service names: one path segment, no surprises.
pub fn valid_service_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
        && !name.starts_with('.')
}

/// Per-project capability grants: project root -> `service:capability`.
///
/// Keyed on the capability NAME rather than on its arguments. Deliberate:
/// `git-push origin` and `git-push origin my-branch` are the same permission,
/// and a grant per branch would mean a prompt per branch, which teaches people
/// to approve without reading. The narrowing that matters happens at use time,
/// against the repository's own configuration.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Grants {
    #[serde(default)]
    pub projects: BTreeMap<String, Vec<String>>,
}

fn grant_key(service: &str, capability: &str) -> String {
    format!("{service}:{capability}")
}

impl Grants {
    /// Fails closed: no project, no grant.
    pub fn allows(&self, project: Option<&str>, service: &str, capability: &str) -> bool {
        let Some(project) = project else { return false };
        let key = grant_key(service, capability);
        self.projects
            .get(project)
            .is_some_and(|keys| keys.contains(&key))
    }

    /// Whether any of `names` is granted. Fails closed the same way.
    ///
    /// The names are one operation's canonical id and the older spellings it
    /// answers to — see `OperationSpec::aliases`. A grant written before a
    /// rename says `github:git-push` on disk, and the request now arrives as
    /// `git.push`; without this the grant silently stops matching and the
    /// owner is told a capability they granted is not granted.
    ///
    /// Only ever called with names the registry produced from ONE operation's
    /// declaration, so it cannot be used to widen a grant across operations.
    pub fn allows_any(&self, project: Option<&str>, service: &str, names: &[&str]) -> bool {
        names.iter().any(|name| self.allows(project, service, name))
    }

    pub fn allow(&mut self, project: &str, service: &str, capability: &str) {
        let keys = self.projects.entry(project.to_string()).or_default();
        let key = grant_key(service, capability);
        if !keys.contains(&key) {
            keys.push(key);
            keys.sort();
        }
    }

    pub fn revoke(&mut self, project: &str, service: &str, capability: &str) -> bool {
        let Some(keys) = self.projects.get_mut(project) else {
            return false;
        };
        let key = grant_key(service, capability);
        let before = keys.len();
        keys.retain(|k| *k != key);
        let removed = keys.len() != before;
        if keys.is_empty() {
            self.projects.remove(project);
        }
        removed
    }

    /// Withdraw any of `names`, and say whether one was there.
    ///
    /// The counterpart to [`Grants::allows_any`], and the reason it has to
    /// exist: a grant written before a rename says `github:git-push` on disk,
    /// and a revoke that only looked for the canonical name would find nothing
    /// and report "was not granted" — leaving a grant the owner asked to remove
    /// in place, under a name they can no longer type.
    pub fn revoke_any(&mut self, project: &str, service: &str, names: &[&str]) -> bool {
        let mut removed = false;
        for name in names {
            removed |= self.revoke(project, service, name);
        }
        removed
    }

    /// Drop every grant that names `service`, across every project.
    ///
    /// Called when a credential is removed. Without it, re-adding a service
    /// under the same name silently inherits whatever the last one was allowed
    /// to do — which is how a credential gets more permission than the person
    /// adding it thought they were giving.
    pub fn forget_service(&mut self, service: &str) -> usize {
        let prefix = format!("{service}:");
        let mut dropped = 0;
        for keys in self.projects.values_mut() {
            let before = keys.len();
            keys.retain(|k| !k.starts_with(&prefix));
            dropped += before - keys.len();
        }
        self.projects.retain(|_, keys| !keys.is_empty());
        dropped
    }
}

/// The store, rooted at a directory.
///
/// The root is a parameter rather than a constant so the daemon can be pointed
/// at a temporary directory and run as an ordinary user in a test. In the image
/// it is `/var/lib/apex-secretd`, created `0700` and owned by root by the unit's
/// `StateDirectory=`.
#[derive(Debug, Clone)]
pub struct Store {
    root: PathBuf,
}

impl Store {
    pub fn new(root: PathBuf) -> Store {
        Store { root }
    }

    /// The store at whatever [`crate::paths::store_root`] resolves to.
    pub fn system() -> Store {
        Store::new(crate::paths::store_root())
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// One user's namespace.
    pub fn user_dir(&self, uid: u32) -> PathBuf {
        self.root.join("users").join(uid.to_string())
    }

    fn meta_path(&self, uid: u32, service: &str) -> PathBuf {
        self.user_dir(uid).join(format!("{service}.json"))
    }

    fn value_path(&self, uid: u32, service: &str) -> PathBuf {
        self.user_dir(uid).join(format!("{service}.secret"))
    }

    fn grants_path(&self, uid: u32) -> PathBuf {
        self.user_dir(uid).join("grants.json")
    }

    /// The machine's audit log.
    pub fn audit_path(&self) -> PathBuf {
        self.root.join("audit.jsonl")
    }

    /// Where a brokered operation puts a file its child has to read.
    ///
    /// Inside the store root, which only root may write, rather than in `/tmp`,
    /// where any account may create a name first. Nothing here is a credential;
    /// what is at stake is that this daemon is root and must not be tricked
    /// into writing through somebody else's symlink.
    pub fn run_dir(&self) -> PathBuf {
        self.root.join("run")
    }

    /// Store a credential, replacing any previous one under the same name.
    ///
    /// The value goes to its own file and is never logged, never included in an
    /// error, and never placed on a command line — `apex secret add` reads it
    /// from stdin and the wire carries it as raw bytes after the request line,
    /// both because argv is world-readable through `/proc`.
    pub fn put(
        &self,
        uid: u32,
        info: &ServiceInfo,
        value: &SecretValue,
    ) -> Result<(), StoreError> {
        if !valid_service_name(&info.service) {
            return Err(StoreError::BadServiceName(info.service.clone()));
        }
        if value.len() > SecretValue::MAX_BYTES {
            return Err(StoreError::TooLong(value.len()));
        }
        if info.scheme != "https" && !crate::capability::is_loopback_host(&info.host) {
            return Err(StoreError::InsecureScheme {
                host: info.host.clone(),
                scheme: info.scheme.clone(),
            });
        }
        let dir = self.user_dir(uid);
        ensure_private_dir(&dir).map_err(|e| StoreError::Io(e.to_string()))?;

        // The value first. A metadata record with no value behind it reads as a
        // stored credential that then fails at use time; a value with no
        // metadata is inert and is cleaned up by the next `put` or `remove`.
        write_private(&self.value_path(uid, &info.service), value.expose())?;
        let text = serde_json::to_vec_pretty(info)
            .map_err(|e| StoreError::Io(format!("serialising: {e}")))?;
        write_private(&self.meta_path(uid, &info.service), &text)
    }

    /// One service's metadata.
    pub fn info(&self, uid: u32, service: &str) -> Option<ServiceInfo> {
        if !valid_service_name(service) {
            return None;
        }
        let text = std::fs::read_to_string(self.meta_path(uid, service)).ok()?;
        serde_json::from_str(&text).ok()
    }

    /// Every stored service, metadata only.
    pub fn list(&self, uid: u32) -> Vec<ServiceInfo> {
        let mut out = Vec::new();
        let Ok(entries) = std::fs::read_dir(self.user_dir(uid)) else {
            return out;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            if path.file_name().and_then(|s| s.to_str()) == Some("grants.json") {
                continue;
            }
            if let Ok(text) = std::fs::read_to_string(&path) {
                if let Ok(info) = serde_json::from_str::<ServiceInfo>(&text) {
                    out.push(info);
                }
            }
        }
        out.sort_by(|a, b| a.service.cmp(&b.service));
        out
    }

    /// Delete a credential and every grant that named it.
    pub fn remove(&self, uid: u32, service: &str) -> Result<(), StoreError> {
        if !valid_service_name(service) {
            return Err(StoreError::BadServiceName(service.to_string()));
        }
        let meta = self.meta_path(uid, service);
        if !meta.exists() {
            return Err(StoreError::NoSuchService(service.to_string()));
        }
        // The value first again, for the same reason as `put`: whatever fails,
        // no credential is left behind reachable.
        shred(&self.value_path(uid, service))?;
        std::fs::remove_file(&meta).map_err(|e| StoreError::Io(e.to_string()))?;

        let mut grants = self.grants(uid);
        if grants.forget_service(service) > 0 {
            self.save_grants(uid, &grants)?;
        }
        Ok(())
    }

    /// Read a credential back.
    ///
    /// The only function in the tree that produces a [`SecretValue`] from disk,
    /// and it has exactly one caller: the daemon's broker, immediately before
    /// it puts the value in the environment of a child process. Nothing that
    /// can reach a socket calls this.
    pub fn value(&self, uid: u32, service: &str) -> Result<SecretValue, StoreError> {
        if !valid_service_name(service) {
            return Err(StoreError::BadServiceName(service.to_string()));
        }
        match std::fs::read(self.value_path(uid, service)) {
            Ok(bytes) => Ok(SecretValue::new(bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Err(StoreError::NoSuchService(service.to_string()))
            }
            Err(e) => Err(StoreError::Io(e.to_string())),
        }
    }

    pub fn grants(&self, uid: u32) -> Grants {
        std::fs::read_to_string(self.grants_path(uid))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn save_grants(&self, uid: u32, grants: &Grants) -> Result<(), StoreError> {
        ensure_private_dir(&self.user_dir(uid)).map_err(|e| StoreError::Io(e.to_string()))?;
        let text = serde_json::to_vec_pretty(grants)
            .map_err(|e| StoreError::Io(format!("serialising: {e}")))?;
        write_private(&self.grants_path(uid), &text)
    }
}

/// Create `dir` and every missing parent with `0700`.
///
/// `create_dir_all` applies the process umask, which may be anything; the mode
/// is set explicitly afterwards rather than left to inherited state.
pub fn ensure_private_dir(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    std::fs::create_dir_all(dir)?;
    let mut perms = std::fs::metadata(dir)?.permissions();
    if perms.mode() & 0o777 != 0o700 {
        perms.set_mode(0o700);
        std::fs::set_permissions(dir, perms)?;
    }
    Ok(())
}

/// Write `bytes` to `path` at `0600`, atomically.
///
/// The temporary file is created with the mode already set, so there is no
/// window in which the credential exists world-readable — which `create` then
/// `set_permissions` would leave under a permissive umask.
fn write_private(path: &Path, bytes: &[u8]) -> Result<(), StoreError> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    let tmp = path.with_extension("tmp");
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)
        .map_err(|e| StoreError::Io(format!("creating {}: {e}", tmp.display())))?;
    file.write_all(bytes)
        .map_err(|e| StoreError::Io(format!("writing {}: {e}", tmp.display())))?;
    file.sync_all()
        .map_err(|e| StoreError::Io(format!("syncing {}: {e}", tmp.display())))?;
    drop(file);
    std::fs::rename(&tmp, path)
        .map_err(|e| StoreError::Io(format!("renaming into {}: {e}", path.display())))
}

/// Overwrite a credential file before unlinking it.
///
/// Not a guarantee — a copy-on-write filesystem may keep the old extent, and
/// btrfs is what APEX installs — so it is written down as best-effort rather
/// than claimed as erasure. It costs one write and removes the value from the
/// obvious place: a file whose blocks are handed to the next allocation.
fn shred(path: &Path) -> Result<(), StoreError> {
    use std::io::Write;

    match std::fs::OpenOptions::new().write(true).open(path) {
        Ok(mut file) => {
            if let Ok(meta) = file.metadata() {
                let zeros = vec![0u8; meta.len() as usize];
                let _ = file.write_all(&zeros);
                let _ = file.sync_all();
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(StoreError::Io(e.to_string())),
    }
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(StoreError::Io(e.to_string())),
    }
}

/// Milliseconds since the epoch.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    const SENTINEL: &str = "apex-sentinel-7f3a91c4-do-not-leak";

    fn temp_store(tag: &str) -> (Store, PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "apex-secret-store-{}-{tag}-{}",
            std::process::id(),
            now_ms()
        ));
        std::fs::remove_dir_all(&dir).ok();
        (Store::new(dir.clone()), dir)
    }

    fn info(service: &str) -> ServiceInfo {
        ServiceInfo {
            service: service.to_string(),
            host: "github.com".to_string(),
            scheme: "https".to_string(),
            username: "x-access-token".to_string(),
            path: String::new(),
            auth: "bearer".to_string(),
            port: None,
            added: now_ms(),
        }
    }

    #[test]
    fn a_stored_credential_round_trips_and_the_metadata_never_holds_it() {
        let (store, dir) = temp_store("roundtrip");
        store
            .put(1000, &info("demo"), &SecretValue::new(SENTINEL.into()))
            .expect("put");

        assert_eq!(
            store.value(1000, "demo").unwrap().expose(),
            SENTINEL.as_bytes()
        );
        assert_eq!(store.info(1000, "demo").unwrap().host, "github.com");

        // The record `apex secret list` reads must not contain the value —
        // this is the file that gets printed, serialised and sent over a
        // socket, and the split into two files is what makes that structural.
        let meta = std::fs::read_to_string(dir.join("users/1000/demo.json")).unwrap();
        assert!(!meta.contains(SENTINEL), "{meta}");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_store_is_private_on_disk() {
        let (store, dir) = temp_store("modes");
        store
            .put(1000, &info("demo"), &SecretValue::new(SENTINEL.into()))
            .expect("put");

        let mode = |p: PathBuf| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(dir.join("users/1000/demo.secret")), 0o600);
        assert_eq!(mode(dir.join("users/1000/demo.json")), 0o600);
        assert_eq!(mode(dir.join("users/1000")), 0o700);

        // A loosened directory is tightened again rather than trusted.
        std::fs::set_permissions(dir.join("users/1000"), std::fs::Permissions::from_mode(0o755))
            .unwrap();
        store
            .put(1000, &info("other"), &SecretValue::new(SENTINEL.into()))
            .expect("put");
        assert_eq!(mode(dir.join("users/1000")), 0o700);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn one_uid_cannot_see_another_uids_services() {
        let (store, dir) = temp_store("uids");
        store
            .put(1000, &info("demo"), &SecretValue::new(b"a".to_vec()))
            .expect("put");
        assert_eq!(store.list(1000).len(), 1);
        assert!(store.list(1001).is_empty());
        assert!(store.info(1001, "demo").is_none());
        assert_eq!(
            store.value(1001, "demo").err(),
            Some(StoreError::NoSuchService("demo".into()))
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_service_name_cannot_escape_its_directory() {
        let (store, dir) = temp_store("escape");
        for evil in ["../x", "a/b", ".hidden", "", "with space", "a\nb"] {
            let mut i = info("x");
            i.service = evil.to_string();
            assert!(
                matches!(
                    store.put(1000, &i, &SecretValue::new(b"a".to_vec())),
                    Err(StoreError::BadServiceName(_))
                ),
                "'{}' was accepted",
                evil.escape_debug()
            );
            assert!(store.info(1000, evil).is_none());
            assert!(store.value(1000, evil).is_err());
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn http_is_refused_off_loopback() {
        let (store, dir) = temp_store("scheme");
        let mut i = info("demo");
        i.scheme = "http".to_string();
        assert!(matches!(
            store.put(1000, &i, &SecretValue::new(b"a".to_vec())),
            Err(StoreError::InsecureScheme { .. })
        ));

        // ...and allowed on it, which is the carve-out the end-to-end test
        // needs and the only one there is.
        i.host = "127.0.0.1".to_string();
        assert!(store.put(1000, &i, &SecretValue::new(b"a".to_vec())).is_ok());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_oversized_value_is_refused_rather_than_written() {
        let (store, dir) = temp_store("toolong");
        let big = vec![b'x'; SecretValue::MAX_BYTES + 1];
        assert!(matches!(
            store.put(1000, &info("demo"), &SecretValue::new(big)),
            Err(StoreError::TooLong(_))
        ));
        assert!(store.info(1000, "demo").is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn removing_takes_the_value_and_the_grants_with_it() {
        let (store, dir) = temp_store("remove");
        store
            .put(1000, &info("demo"), &SecretValue::new(SENTINEL.into()))
            .expect("put");
        let mut grants = Grants::default();
        grants.allow("/p", "demo", "git-fetch");
        grants.allow("/p", "other", "git-fetch");
        store.save_grants(1000, &grants).expect("grants");

        store.remove(1000, "demo").expect("remove");
        assert!(store.info(1000, "demo").is_none());
        assert!(store.value(1000, "demo").is_err());

        // A re-added credential under the same name must not inherit what the
        // last one was allowed to do.
        let after = store.grants(1000);
        assert!(!after.allows(Some("/p"), "demo", "git-fetch"));
        assert!(after.allows(Some("/p"), "other", "git-fetch"));

        // And nothing under the store still holds the value.
        let mut found = Vec::new();
        walk(&dir, &mut found);
        for path in found {
            let bytes = std::fs::read(&path).unwrap_or_default();
            assert!(
                !String::from_utf8_lossy(&bytes).contains(SENTINEL),
                "{} still holds the credential",
                path.display()
            );
        }

        assert_eq!(
            store.remove(1000, "demo"),
            Err(StoreError::NoSuchService("demo".into()))
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else {
                out.push(path);
            }
        }
    }

    #[test]
    fn grants_fail_closed_without_a_project() {
        let mut grants = Grants::default();
        grants.allow("/p", "demo", "git-fetch");
        assert!(grants.allows(Some("/p"), "demo", "git-fetch"));
        // No project at all: nothing matches, rather than everything.
        assert!(!grants.allows(None, "demo", "git-fetch"));
        // A grant is per capability and per project.
        assert!(!grants.allows(Some("/p"), "demo", "git-push"));
        assert!(!grants.allows(Some("/q"), "demo", "git-fetch"));
        assert!(!grants.allows(Some("/p"), "other", "git-fetch"));
    }

    #[test]
    fn granting_twice_does_not_duplicate_and_revoking_empties_the_project() {
        let mut grants = Grants::default();
        grants.allow("/p", "demo", "git-fetch");
        grants.allow("/p", "demo", "git-fetch");
        assert_eq!(grants.projects["/p"].len(), 1);
        assert!(grants.revoke("/p", "demo", "git-fetch"));
        assert!(!grants.revoke("/p", "demo", "git-fetch"));
        // The project key goes with its last grant, so `apex secret grants`
        // does not list projects that are allowed nothing.
        assert!(grants.projects.is_empty());
    }

    #[test]
    fn a_service_prefix_is_not_matched_loosely_when_forgetting() {
        // `demo` and `demo-staging` are different credentials. Removing one
        // must not silently drop the other's grants.
        let mut grants = Grants::default();
        grants.allow("/p", "demo", "git-fetch");
        grants.allow("/p", "demo-staging", "git-fetch");
        assert_eq!(grants.forget_service("demo"), 1);
        assert!(grants.allows(Some("/p"), "demo-staging", "git-fetch"));
    }

    #[test]
    fn a_metadata_record_from_an_older_build_still_loads() {
        // The store is persistent state across an update. A record written
        // before the scheme field existed must read back as https, not fail.
        let info: ServiceInfo =
            serde_json::from_str(r#"{"service":"demo","host":"github.com"}"#).unwrap();
        assert_eq!(info.scheme, "https");
        assert_eq!(info.username, "x-access-token");
    }
}
