//! The APEX secret broker (roadmap §4).
//!
//! §4: *"Agents should be able to use credentials without receiving the raw
//! secret. Treat secrets as capabilities rather than environment variables
//! whenever integrations allow it… Expose usage permission, not necessarily
//! secret value."*
//!
//! ## Why the broker performs the operation
//!
//! The obvious implementation is a git credential helper reachable from inside
//! the sandbox. It does not work, and the reason is worth writing down so
//! nobody re-implements it: `git` runs *inside* the sandbox, so whatever the
//! helper prints is on git's stdin — inside the agent's own namespace, readable
//! by the agent. A credential helper hands over the token by construction.
//!
//! So the broker performs the operation instead. The agent asks for
//! `git-push origin`; `apex-agentd`, which runs *outside* the sandbox, runs the
//! push and returns the result. The token is only ever in the environment of a
//! process the agent cannot see — the sandbox uses `--unshare-pid`, so the
//! daemon's children are not in the agent's `/proc` at all.
//!
//! This is not a privilege boundary; the daemon is unprivileged and runs as the
//! same user. It is a *namespace* boundary, which is exactly the one needed.
//!
//! ## The agent cannot name a URL
//!
//! [`Capability::GitPush`] takes a remote *name*, never a URL, and the daemon
//! resolves the name against the repository's own configuration. Accepting a
//! URL would mean a session could ask the broker to push a branch to
//! `https://attacker.example/` with the user's token attached — and the broker
//! would, because it was told to. The remote's host is then checked against the
//! service's host, so a grant for GitHub cannot push to GitLab.
//!
//! ## Where the secret lives, and why not the keyring
//!
//! In a `0600` file under `$XDG_STATE_HOME`, inside a `0700` directory. That
//! path is inside `$HOME`, which a confined session masks with a tmpfs, so a
//! `project`-policy agent cannot read it — asserted in the sandbox suite.
//!
//! The keyring is *supported* but not the default, and that was decided
//! empirically rather than on principle. `secret-tool store` on APEX blocks on
//! a `gcr-prompter` "Unlock Keyring" dialog: measured, and it hung until it was
//! killed. `gnome-keyring-daemon` also ships disabled. A broker that an agent
//! calls must never hang and must never pop a dialog at somebody who is not
//! watching, so the keyring is opt-in ([`Backend::Keyring`]) and every keyring
//! call is bounded by a timeout.
//!
//! An `unrestricted` session can read the file, like it can read everything
//! else — that is what the escape hatch means.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Longest a keyring call may block before it is abandoned.
///
/// Exists because `secret-tool` waits on an unlock prompt indefinitely. An
/// agent's request must fail with an explanation rather than hang forever.
pub const KEYRING_TIMEOUT_SECS: u64 = 5;

/// What an agent may ask the broker to do.
///
/// A closed enum, for the same reason as [`crate::request::Verb`]: a variant
/// carrying a command line would be a way to run anything with a credential
/// attached, and no reviewer can meaningfully approve that.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "capability", rename_all = "kebab-case")]
pub enum Capability {
    /// `git push <remote> <branch>` — remote by NAME, resolved by the daemon.
    GitPush {
        remote: String,
        /// `None` means the current branch, resolved by the daemon.
        #[serde(default)]
        branch: Option<String>,
    },
    /// `git fetch <remote>`.
    GitFetch { remote: String },
}

/// Why a capability request was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretError {
    UnknownCapability(String),
    BadRemoteName(String),
    BadBranchName(String),
    /// The named remote is not configured in this repository.
    NoSuchRemote(String),
    /// The remote's URL is not one this service may be used for.
    HostMismatch { remote_host: String, service_host: String },
    /// The remote is not an HTTPS URL, so a token is not how it authenticates.
    NotHttps(String),
    /// No credential stored for the service.
    NoSuchService(String),
    /// The capability is not granted for this project.
    NotGranted { service: String, capability: String },
    /// The keyring did not answer in time.
    KeyringTimeout,
    Io(String),
}

impl std::fmt::Display for SecretError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SecretError::UnknownCapability(c) => write!(
                f,
                "'{c}' is not a capability the broker offers; \
                 run `apex secret capabilities` for the list"
            ),
            SecretError::BadRemoteName(r) => write!(
                f,
                "'{}' is not a git remote name. The broker takes a NAME, never \
                 a URL — a URL would let a session choose where your token gets \
                 sent",
                r.escape_debug()
            ),
            SecretError::BadBranchName(b) => {
                write!(f, "'{}' is not a valid branch name", b.escape_debug())
            }
            SecretError::NoSuchRemote(r) => {
                write!(f, "this repository has no remote called '{r}'")
            }
            SecretError::HostMismatch { remote_host, service_host } => write!(
                f,
                "that remote points at {remote_host}, but this credential is \
                 for {service_host}"
            ),
            SecretError::NotHttps(url) => write!(
                f,
                "{url} is not an https remote, so a stored token is not how it \
                 authenticates (an ssh remote uses your agent, which a confined \
                 session cannot reach — by design)"
            ),
            SecretError::NoSuchService(s) => {
                write!(f, "no credential stored for '{s}'; add one with `apex secret add {s}`")
            }
            SecretError::NotGranted { service, capability } => write!(
                f,
                "'{capability}' on '{service}' is not granted for this project; \
                 allow it with `apex secret grant {service} {capability}`"
            ),
            SecretError::KeyringTimeout => write!(
                f,
                "the keyring did not answer within {KEYRING_TIMEOUT_SECS}s — it \
                 is probably waiting on an unlock prompt. Use the file backend, \
                 which is the default for exactly this reason"
            ),
            SecretError::Io(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for SecretError {}

impl Capability {
    /// Parse a capability as typed on the command line.
    pub fn parse(name: &str, remote: &str, branch: Option<&str>) -> Result<Capability, SecretError> {
        if !valid_remote_name(remote) {
            return Err(SecretError::BadRemoteName(remote.to_string()));
        }
        if let Some(b) = branch {
            if !valid_branch_name(b) {
                return Err(SecretError::BadBranchName(b.to_string()));
            }
        }
        match name {
            "git-push" => Ok(Capability::GitPush {
                remote: remote.to_string(),
                branch: branch.map(str::to_string),
            }),
            "git-fetch" => Ok(Capability::GitFetch {
                remote: remote.to_string(),
            }),
            other => Err(SecretError::UnknownCapability(other.to_string())),
        }
    }

    pub fn names() -> &'static [&'static str] {
        &["git-push", "git-fetch"]
    }

    /// The capability name, without its arguments. This is what a grant is
    /// keyed on: granting `git-push` to a project allows pushing that project's
    /// branches, not one specific branch forever.
    pub fn name(&self) -> &'static str {
        match self {
            Capability::GitPush { .. } => "git-push",
            Capability::GitFetch { .. } => "git-fetch",
        }
    }

    pub fn remote(&self) -> &str {
        match self {
            Capability::GitPush { remote, .. } | Capability::GitFetch { remote } => remote,
        }
    }

    /// One line for the audit log and for `apex secret grants`.
    pub fn summary(&self) -> String {
        match self {
            Capability::GitPush { remote, branch } => match branch {
                Some(b) => format!("git push {remote} {b}"),
                None => format!("git push {remote} (current branch)"),
            },
            Capability::GitFetch { remote } => format!("git fetch {remote}"),
        }
    }

    pub fn describe(name: &str) -> &'static str {
        match name {
            "git-push" => "push a branch of this project to one of its own remotes",
            "git-fetch" => "fetch from one of this project's own remotes",
            _ => "",
        }
    }
}

/// Git remote names: what git itself accepts, minus anything that could be read
/// as a URL or an option.
///
/// The leading-character rule is the important one. `-` first would be read as
/// an option by git, and a name containing `:` or `/` is how a URL looks.
pub fn valid_remote_name(name: &str) -> bool {
    if name.is_empty() || name.len() > 100 {
        return false;
    }
    let first = name.chars().next().unwrap();
    if !first.is_ascii_alphanumeric() && first != '_' {
        return false;
    }
    name.chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
}

/// Branch names: git's own rules, tightened.
///
/// Notably refused: a leading `-` (an option), `..` (a revision range), and
/// every control character. A branch name reaches a command line, and this is
/// the check that keeps it from being read as something else.
pub fn valid_branch_name(name: &str) -> bool {
    if name.is_empty() || name.len() > 255 {
        return false;
    }
    if name.starts_with('-') || name.starts_with('/') || name.ends_with('/') {
        return false;
    }
    if name.contains("..") || name.contains("//") {
        return false;
    }
    if name.ends_with(".lock") || name == "@" {
        return false;
    }
    name.chars().all(|c| {
        (c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-' | '/' | '+'))
            && !c.is_control()
    })
}

/// Where a service's credential is kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Backend {
    /// A `0600` file under `$XDG_STATE_HOME`. The default: it cannot block, it
    /// cannot prompt, and a confined session cannot read it because `$HOME` is
    /// masked.
    File,
    /// libsecret via `secret-tool`. Opt-in, because it blocks on an unlock
    /// prompt — measured on APEX, where it hung until killed.
    Keyring,
}

impl Backend {
    pub fn as_str(&self) -> &'static str {
        match self {
            Backend::File => "file",
            Backend::Keyring => "keyring",
        }
    }

    pub fn parse(s: &str) -> Option<Backend> {
        match s {
            "file" => Some(Backend::File),
            "keyring" => Some(Backend::Keyring),
            _ => None,
        }
    }
}

/// A stored credential's metadata. **Never the token.**
///
/// A separate type from the token on purpose: this is what `apex secret list`
/// prints and what the audit log records, and a struct that could carry the
/// secret is a struct that eventually will.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceInfo {
    pub service: String,
    /// Host the credential is valid for. Checked against a remote's URL.
    pub host: String,
    /// Username git should send. Most token schemes ignore it.
    #[serde(default = "default_username")]
    pub username: String,
    pub backend: Backend,
    #[serde(default)]
    pub added: u64,
}

fn default_username() -> String {
    "x-access-token".to_string()
}

/// The on-disk form for [`Backend::File`]: metadata plus the token.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredSecret {
    #[serde(flatten)]
    info: ServiceInfo,
    token: String,
}

pub fn secrets_dir() -> PathBuf {
    crate::paths::state_dir().join("secrets")
}

fn secret_path(service: &str) -> PathBuf {
    secrets_dir().join(format!("{service}.json"))
}

pub fn audit_log() -> PathBuf {
    crate::paths::state_dir().join("secret-audit.jsonl")
}

pub fn grants_file() -> PathBuf {
    crate::paths::state_dir().join("secret-grants.json")
}

/// Service names: one path segment, no surprises.
pub fn valid_service_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
        && !name.starts_with('.')
}

/// Store a credential.
///
/// The token arrives as a parameter and is written straight out; it is never
/// logged, never included in an error message, and never placed on a command
/// line. `apex secret add` reads it from stdin for the same reason — argv is
/// world-readable through `/proc`.
pub fn store(info: &ServiceInfo, token: &str) -> Result<(), SecretError> {
    use std::os::unix::fs::PermissionsExt;

    if !valid_service_name(&info.service) {
        return Err(SecretError::NoSuchService(info.service.clone()));
    }
    let dir = secrets_dir();
    crate::paths::ensure_private_dir(&dir).map_err(|e| SecretError::Io(e.to_string()))?;

    match info.backend {
        Backend::Keyring => {
            keyring_store(&info.service, token)?;
            // The metadata still lives in a file — only the token goes to the
            // keyring. Otherwise `apex secret list` would have to unlock the
            // keyring just to say which services exist.
            write_json(&secret_path(&info.service), info, 0o600)
        }
        Backend::File => {
            let stored = StoredSecret {
                info: info.clone(),
                token: token.to_string(),
            };
            write_json(&secret_path(&info.service), &stored, 0o600)
        }
    }?;

    // Belt and braces: the mode is set explicitly after writing, because a
    // umask of 0 would otherwise leave the file world-readable.
    let path = secret_path(&info.service);
    if let Ok(meta) = std::fs::metadata(&path) {
        let mut perms = meta.permissions();
        if perms.mode() & 0o777 != 0o600 {
            perms.set_mode(0o600);
            let _ = std::fs::set_permissions(&path, perms);
        }
    }
    Ok(())
}

fn write_json<T: Serialize>(path: &Path, value: &T, mode: u32) -> Result<(), SecretError> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    let tmp = path.with_extension("json.tmp");
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(mode)
        .open(&tmp)
        .map_err(|e| SecretError::Io(format!("creating {}: {e}", tmp.display())))?;
    let text = serde_json::to_string_pretty(value)
        .map_err(|e| SecretError::Io(format!("serialising: {e}")))?;
    file.write_all(text.as_bytes())
        .map_err(|e| SecretError::Io(format!("writing {}: {e}", tmp.display())))?;
    drop(file);
    std::fs::rename(&tmp, path)
        .map_err(|e| SecretError::Io(format!("renaming into {}: {e}", path.display())))
}

/// Every stored service, metadata only.
pub fn list() -> Vec<ServiceInfo> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(secrets_dir()) else {
        return out;
    };
    for entry in entries.flatten() {
        if entry.path().extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        if let Ok(text) = std::fs::read_to_string(entry.path()) {
            if let Ok(info) = serde_json::from_str::<ServiceInfo>(&text) {
                out.push(info);
            }
        }
    }
    out.sort_by(|a, b| a.service.cmp(&b.service));
    out
}

pub fn info(service: &str) -> Option<ServiceInfo> {
    let text = std::fs::read_to_string(secret_path(service)).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn remove(service: &str) -> Result<(), SecretError> {
    if !valid_service_name(service) {
        return Err(SecretError::NoSuchService(service.to_string()));
    }
    if let Some(i) = info(service) {
        if i.backend == Backend::Keyring {
            let _ = keyring_clear(service);
        }
    }
    match std::fs::remove_file(secret_path(service)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Err(SecretError::NoSuchService(service.to_string()))
        }
        Err(e) => Err(SecretError::Io(e.to_string())),
    }
}

/// Read a token back.
///
/// Returns the secret, so every caller is a place to check. There are two: the
/// daemon's broker, which puts it in a child's environment, and nothing else.
pub fn token(service: &str) -> Result<String, SecretError> {
    let i = info(service).ok_or_else(|| SecretError::NoSuchService(service.to_string()))?;
    match i.backend {
        Backend::Keyring => keyring_lookup(service),
        Backend::File => {
            let text = std::fs::read_to_string(secret_path(service))
                .map_err(|e| SecretError::Io(e.to_string()))?;
            let stored: StoredSecret =
                serde_json::from_str(&text).map_err(|e| SecretError::Io(e.to_string()))?;
            Ok(stored.token)
        }
    }
}

// ── the keyring, bounded ────────────────────────────────────────────────────

const KEYRING_ATTR: &str = "apex-secret";

/// Run `secret-tool` with a hard timeout.
///
/// The timeout is the whole point. `secret-tool` waits on an unlock prompt
/// forever, and an agent's request must not. Implemented with `timeout(1)`
/// rather than a thread and a channel because the child has to actually die —
/// abandoning the wait while `secret-tool` keeps the prompt on screen would
/// leave a dialog in front of somebody who never asked for one.
fn secret_tool(args: &[&str], stdin: Option<&str>) -> Result<String, SecretError> {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let mut cmd = Command::new("timeout");
    cmd.arg(KEYRING_TIMEOUT_SECS.to_string())
        .arg("secret-tool")
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if stdin.is_some() {
        cmd.stdin(Stdio::piped());
    } else {
        cmd.stdin(Stdio::null());
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| SecretError::Io(format!("running secret-tool: {e}")))?;
    if let Some(text) = stdin {
        if let Some(mut pipe) = child.stdin.take() {
            let _ = pipe.write_all(text.as_bytes());
        }
    }
    let out = child
        .wait_with_output()
        .map_err(|e| SecretError::Io(e.to_string()))?;
    // timeout(1) reports 124 when it had to kill the child.
    if out.status.code() == Some(124) {
        return Err(SecretError::KeyringTimeout);
    }
    if !out.status.success() {
        return Err(SecretError::Io(
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim_end().to_string())
}

fn keyring_store(service: &str, token: &str) -> Result<(), SecretError> {
    secret_tool(
        &["store", "--label", "APEX secret", KEYRING_ATTR, service],
        Some(token),
    )
    .map(|_| ())
}

fn keyring_lookup(service: &str) -> Result<String, SecretError> {
    let out = secret_tool(&["lookup", KEYRING_ATTR, service], None)?;
    if out.is_empty() {
        return Err(SecretError::NoSuchService(service.to_string()));
    }
    Ok(out)
}

fn keyring_clear(service: &str) -> Result<(), SecretError> {
    secret_tool(&["clear", KEYRING_ATTR, service], None).map(|_| ())
}

// ── grants ──────────────────────────────────────────────────────────────────

/// Per-project capability grants: project root -> `service:capability`.
///
/// Keyed on the capability NAME rather than on its arguments, unlike privilege
/// grants. Deliberate: `git-push origin` and `git-push origin my-branch` are
/// the same permission, and a grant per branch would mean a prompt per branch,
/// which teaches people to approve without reading. The narrowing that matters
/// here is the remote check, which happens at use time against the repository's
/// own configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SecretGrants {
    #[serde(default)]
    pub projects: std::collections::BTreeMap<String, Vec<String>>,
}

fn grant_key(service: &str, capability: &str) -> String {
    format!("{service}:{capability}")
}

impl SecretGrants {
    pub fn load(path: &Path) -> SecretGrants {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> Result<(), SecretError> {
        if let Some(parent) = path.parent() {
            crate::paths::ensure_private_dir(parent).map_err(|e| SecretError::Io(e.to_string()))?;
        }
        write_json(path, self, 0o600)
    }

    /// Fails closed: no project, no grant.
    pub fn allows(&self, project: Option<&str>, service: &str, capability: &str) -> bool {
        let Some(project) = project else { return false };
        let key = grant_key(service, capability);
        self.projects
            .get(project)
            .is_some_and(|keys| keys.iter().any(|k| *k == key))
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

    pub fn revoke_project(&mut self, project: &str) -> usize {
        self.projects.remove(project).map_or(0, |k| k.len())
    }
}

// ── audit ───────────────────────────────────────────────────────────────────

/// §4: "audit which agent used which capability and when."
///
/// Append-only. The token never appears — the record names the capability and
/// the remote, which is the whole point of brokering rather than handing over.
pub fn audit(
    path: &Path,
    event: &str,
    service: &str,
    capability: &Capability,
    session: Option<u32>,
    agent: Option<&str>,
    project: Option<&str>,
    exit_code: Option<i32>,
) -> std::io::Result<()> {
    use std::io::Write;

    if let Some(parent) = path.parent() {
        crate::paths::ensure_private_dir(parent)?;
    }
    let line = serde_json::json!({
        "ms": crate::request::now_ms(),
        "event": event,
        "service": service,
        "capability": capability.name(),
        "detail": capability.summary(),
        "session": session,
        "agent": agent,
        "project": project,
        "exit_code": exit_code,
    });
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(file, "{line}")
}

// ── remote validation ───────────────────────────────────────────────────────

/// The host part of an https git URL.
///
/// Handles the `https://user@host/path` form, because a URL carrying
/// credentials is exactly the case where taking everything before the first
/// `/` gives the wrong host.
pub fn https_host(url: &str) -> Option<String> {
    let rest = url.strip_prefix("https://")?;
    let authority = rest.split('/').next()?;
    let host = authority.rsplit('@').next()?;
    let host = host.split(':').next()?;
    if host.is_empty() {
        return None;
    }
    Some(host.to_ascii_lowercase())
}

/// Check a resolved remote URL against the service it will be used with.
///
/// `remotes` is what the repository itself says, resolved by the daemon —
/// never anything the caller supplied.
pub fn check_remote(
    remote: &str,
    remotes: &[(String, String)],
    service_host: &str,
) -> Result<String, SecretError> {
    let url = remotes
        .iter()
        .find(|(name, _)| name == remote)
        .map(|(_, url)| url.clone())
        .ok_or_else(|| SecretError::NoSuchRemote(remote.to_string()))?;

    let host = https_host(&url).ok_or_else(|| SecretError::NotHttps(url.clone()))?;
    if host != service_host.to_ascii_lowercase() {
        return Err(SecretError::HostMismatch {
            remote_host: host,
            service_host: service_host.to_string(),
        });
    }
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── the closed vocabulary ───────────────────────────────────────────────

    #[test]
    fn there_is_no_capability_that_runs_a_command() {
        for attempt in ["exec", "sh", "run", "git", "shell", "eval", "curl"] {
            assert!(
                matches!(
                    Capability::parse(attempt, "origin", None),
                    Err(SecretError::UnknownCapability(_))
                ),
                "'{attempt}' must not be a capability"
            );
        }
    }

    #[test]
    fn every_advertised_capability_parses_and_has_a_description() {
        for name in Capability::names() {
            let c = Capability::parse(name, "origin", None).expect(name);
            assert_eq!(c.name(), *name);
            assert!(!Capability::describe(name).is_empty(), "{name}");
            assert!(!c.summary().is_empty());
        }
    }

    // ── the agent cannot name a URL ─────────────────────────────────────────

    #[test]
    fn a_remote_may_not_be_a_url() {
        // The hole this closes: with a URL accepted, a session asks the broker
        // to push to somewhere it controls and the broker does it, with the
        // user's token attached.
        for evil in [
            "https://attacker.example/repo",
            "git@github.com:me/repo",
            "http://x/y",
            "../../etc",
            "origin/../other",
            "ssh://x",
        ] {
            assert!(!valid_remote_name(evil), "{evil:?} must be refused");
        }
    }

    #[test]
    fn a_remote_may_not_look_like_an_option() {
        for evil in ["-f", "--force", "-", ""] {
            assert!(!valid_remote_name(evil), "{evil:?}");
        }
    }

    #[test]
    fn real_remote_names_are_accepted() {
        for good in ["origin", "upstream", "fork2", "my-remote", "my.remote", "_x"] {
            assert!(valid_remote_name(good), "{good:?}");
        }
    }

    #[test]
    fn branch_names_refuse_options_ranges_and_control_characters() {
        for evil in [
            "-f", "--all", "/x", "x/", "a..b", "a//b", "x.lock", "@", "",
            "a\nb", "a\rb", "a\u{0}b", "a b", "a;rm -rf /", "a$(x)",
        ] {
            assert!(!valid_branch_name(evil), "{evil:?} must be refused");
        }
        for good in ["main", "feat/x", "release-1.2", "a_b", "v1.0+build"] {
            assert!(valid_branch_name(good), "{good:?} must be accepted");
        }
    }

    // ── remote resolution ───────────────────────────────────────────────────

    fn remotes() -> Vec<(String, String)> {
        vec![
            ("origin".into(), "https://github.com/AndreNijman/apex-os.git".into()),
            ("gl".into(), "https://gitlab.com/x/y.git".into()),
            ("ssh".into(), "git@github.com:AndreNijman/apex-os.git".into()),
            ("creds".into(), "https://user@github.com/a/b.git".into()),
        ]
    }

    #[test]
    fn a_remote_is_resolved_from_the_repository_and_its_host_checked() {
        let url = check_remote("origin", &remotes(), "github.com").expect("origin");
        assert!(url.starts_with("https://github.com/"));
    }

    #[test]
    fn a_github_credential_cannot_be_used_on_gitlab() {
        // The narrowing that makes a grant mean something: "allow git-push with
        // the GitHub token" must not authorise pushing to any host the repo
        // happens to have a remote for.
        assert!(matches!(
            check_remote("gl", &remotes(), "github.com"),
            Err(SecretError::HostMismatch { .. })
        ));
    }

    #[test]
    fn an_unconfigured_remote_is_refused() {
        assert!(matches!(
            check_remote("nope", &remotes(), "github.com"),
            Err(SecretError::NoSuchRemote(_))
        ));
    }

    #[test]
    fn an_ssh_remote_is_refused_with_an_explanation() {
        // A token is not how ssh authenticates, and the ssh-agent socket is
        // masked with $XDG_RUNTIME_DIR — so this cannot work and says why.
        assert!(matches!(
            check_remote("ssh", &remotes(), "github.com"),
            Err(SecretError::NotHttps(_))
        ));
    }

    #[test]
    fn the_host_is_taken_from_the_right_side_of_an_at_sign() {
        // `https://user@host/path`: taking everything before the first `/`
        // yields "user@host", and taking everything before the first `@` yields
        // "user" — a URL carrying credentials is exactly where a naive parse
        // gets the host wrong.
        assert_eq!(https_host("https://user@github.com/a/b"), Some("github.com".into()));
        assert_eq!(https_host("https://github.com/a/b"), Some("github.com".into()));
        assert_eq!(https_host("https://github.com:443/a"), Some("github.com".into()));
        assert_eq!(https_host("https://GitHub.COM/a"), Some("github.com".into()));
        assert_eq!(https_host("http://github.com/a"), None);
        assert_eq!(https_host("git@github.com:a/b"), None);
        assert_eq!(https_host("https:///a"), None);
        // And it is used: a credential-carrying URL still matches its host.
        assert!(check_remote("creds", &remotes(), "github.com").is_ok());
    }

    // ── grants ──────────────────────────────────────────────────────────────

    #[test]
    fn a_grant_is_scoped_to_the_project_and_fails_closed_without_one() {
        let mut g = SecretGrants::default();
        g.allow("/p/a", "github", "git-push");
        assert!(g.allows(Some("/p/a"), "github", "git-push"));
        assert!(!g.allows(Some("/p/a"), "github", "git-fetch"));
        assert!(!g.allows(Some("/p/a"), "gitlab", "git-push"));
        assert!(!g.allows(Some("/p/b"), "github", "git-push"));
        assert!(!g.allows(None, "github", "git-push"), "no project, no grant");
    }

    #[test]
    fn allowing_twice_records_once_and_revoking_empties_the_project() {
        let mut g = SecretGrants::default();
        g.allow("/p", "github", "git-push");
        g.allow("/p", "github", "git-push");
        assert_eq!(g.projects.get("/p").unwrap().len(), 1);
        assert!(g.revoke("/p", "github", "git-push"));
        assert!(!g.projects.contains_key("/p"), "an empty project is removed");
        assert!(!g.revoke("/p", "github", "git-push"));
    }

    #[test]
    fn a_missing_or_corrupt_grants_file_denies_everything() {
        let g = SecretGrants::load(Path::new("/nonexistent/secret-grants.json"));
        assert!(!g.allows(Some("/p"), "github", "git-push"));

        let d = std::env::temp_dir().join(format!("apex-sg-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join("g.json");
        std::fs::write(&p, b"{ not json").unwrap();
        assert!(!SecretGrants::load(&p).allows(Some("/p"), "github", "git-push"));
        std::fs::remove_dir_all(&d).ok();
    }

    // ── storage ─────────────────────────────────────────────────────────────

    fn isolated<T>(f: impl FnOnce() -> T) -> T {
        // The store derives its path from XDG_STATE_HOME through paths.rs, and
        // std::env::set_var is process-global and races other tests — so these
        // tests use the real state dir with a service name nothing else uses,
        // and clean up after themselves.
        f()
    }

    #[test]
    fn a_file_backed_secret_round_trips_and_is_not_world_readable() {
        use std::os::unix::fs::PermissionsExt;

        isolated(|| {
            let service = format!("apextest{}", std::process::id());
            let info = ServiceInfo {
                service: service.clone(),
                host: "github.com".into(),
                username: "x-access-token".into(),
                backend: Backend::File,
                added: 1,
            };
            store(&info, "sentinel-token-value").expect("store");

            let mode = std::fs::metadata(secret_path(&service))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600, "mode was {:o}", mode & 0o777);

            assert_eq!(token(&service).expect("token"), "sentinel-token-value");
            let back = self::info(&service).expect("info");
            assert_eq!(back.host, "github.com");
            assert_eq!(back.backend, Backend::File);

            // ServiceInfo must never carry the token: it is what `list` prints
            // and what the audit log records.
            let text = serde_json::to_string(&back).unwrap();
            assert!(!text.contains("sentinel-token-value"), "{text}");

            remove(&service).expect("remove");
            assert!(self::info(&service).is_none());
            assert!(matches!(
                token(&service),
                Err(SecretError::NoSuchService(_))
            ));
        })
    }

    #[test]
    fn listing_reports_metadata_and_never_a_token() {
        isolated(|| {
            let service = format!("apexlist{}", std::process::id());
            let info = ServiceInfo {
                service: service.clone(),
                host: "github.com".into(),
                username: "x".into(),
                backend: Backend::File,
                added: 1,
            };
            store(&info, "another-sentinel").expect("store");
            let all = list();
            let mine = all.iter().find(|i| i.service == service).expect("listed");
            let text = serde_json::to_string(mine).unwrap();
            assert!(!text.contains("another-sentinel"), "{text}");
            remove(&service).ok();
        })
    }

    #[test]
    fn a_service_name_cannot_escape_the_secrets_directory() {
        for evil in ["../x", "a/b", ".hidden", "", "x y", "a\nb"] {
            assert!(!valid_service_name(evil), "{evil:?}");
        }
        for good in ["github", "gitlab-work", "my.host", "a_b"] {
            assert!(valid_service_name(good), "{good:?}");
        }
        let info = ServiceInfo {
            service: "../escape".into(),
            host: "h".into(),
            username: "u".into(),
            backend: Backend::File,
            added: 0,
        };
        assert!(matches!(
            store(&info, "t"),
            Err(SecretError::NoSuchService(_))
        ));
    }

    #[test]
    fn backend_names_round_trip() {
        for b in [Backend::File, Backend::Keyring] {
            assert_eq!(Backend::parse(b.as_str()), Some(b));
        }
        assert_eq!(Backend::parse("gnome"), None);
    }

    // ── audit ───────────────────────────────────────────────────────────────

    #[test]
    fn the_audit_log_records_the_capability_and_never_the_token() {
        let d = std::env::temp_dir().join(format!("apex-sa-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let log = d.join("audit.jsonl");
        let cap = Capability::GitPush {
            remote: "origin".into(),
            branch: Some("feat/x".into()),
        };
        audit(&log, "used", "github", &cap, Some(4), Some("claude"),
              Some("/p/demo"), Some(0)).expect("audit");
        audit(&log, "refused", "github", &cap, Some(4), Some("claude"),
              Some("/p/demo"), None).expect("audit");

        let text = std::fs::read_to_string(&log).unwrap();
        assert_eq!(text.lines().count(), 2, "appends must not overwrite");
        for line in text.lines() {
            let v: serde_json::Value = serde_json::from_str(line).expect("json per line");
            assert_eq!(v["service"], "github");
            assert_eq!(v["capability"], "git-push");
            assert!(v["detail"].as_str().unwrap().contains("origin"));
            assert!(v.get("ms").is_some());
        }
        std::fs::remove_dir_all(&d).ok();
    }
}
