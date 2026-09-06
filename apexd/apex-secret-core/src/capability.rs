//! What may be asked of the broker, and the record that describes one ask.
//!
//! §3.2's flow is `capability request -> policy decision -> broker-owned
//! operation -> provider -> result`, and §11 names the fields a capability
//! record carries. [`CapabilityRecord`] is that list, verbatim, and it is the
//! single thing that travels from the caller to `apex-secretd` and from
//! `apex-secretd` into the audit log — so the request, the decision and the
//! trail cannot describe different operations.
//!
//! ## Why the vocabulary is a closed enum
//!
//! A variant carrying a command line would be a way to run anything with a
//! credential attached, and no reviewer can meaningfully approve that. Same
//! argument as `apex_agent_core::request::Verb`.
//!
//! ## Why the resource is a remote NAME
//!
//! Never a URL. A session that could name a URL could ask the broker to push a
//! branch to `https://attacker.example/` with the user's token attached, and
//! the broker would, because it was told to. `apex-secretd` resolves the name
//! against the repository's own configuration and then pins the resulting
//! host against the one the credential was stored for.

use serde::{Deserialize, Serialize};

/// What an agent may ask the broker to do.
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
    /// `git ls-remote <remote>` — the refs the remote advertises.
    ///
    /// Read-only and it changes nothing, which is why it is the capability the
    /// end-to-end test drives: it needs one authenticated HTTP request and no
    /// working tree, so a hermetic loopback server can prove the credential was
    /// attached without a network or a fixture repository on the far side.
    GitLsRemote { remote: String },
}

/// Why a capability request was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CapabilityError {
    UnknownCapability(String),
    BadRemoteName(String),
    BadBranchName(String),
    /// The named remote is not configured in this repository.
    NoSuchRemote(String),
    /// The remote's URL is not one this credential may be used for.
    HostMismatch {
        remote_host: String,
        service_host: String,
    },
    /// The remote's scheme is not the one the credential was stored for.
    SchemeMismatch {
        url: String,
        service_scheme: String,
    },
    /// The remote is not an http(s) URL, so a token is not how it
    /// authenticates.
    NotHttp(String),
}

impl std::fmt::Display for CapabilityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CapabilityError::UnknownCapability(c) => write!(
                f,
                "'{}' is not a capability the broker offers; run \
                 `apex secret capabilities` for the list",
                c.escape_debug()
            ),
            CapabilityError::BadRemoteName(r) => write!(
                f,
                "'{}' is not a git remote name. The broker takes a NAME, never \
                 a URL — a URL would let a session choose where your token gets \
                 sent",
                r.escape_debug()
            ),
            CapabilityError::BadBranchName(b) => {
                write!(f, "'{}' is not a valid branch name", b.escape_debug())
            }
            CapabilityError::NoSuchRemote(r) => {
                write!(f, "this repository has no remote called '{}'", r.escape_debug())
            }
            CapabilityError::HostMismatch {
                remote_host,
                service_host,
            } => write!(
                f,
                "that remote points at {remote_host}, but this credential is \
                 for {service_host}"
            ),
            CapabilityError::SchemeMismatch {
                url,
                service_scheme,
            } => write!(
                f,
                "{url} is not {service_scheme}, which is the scheme this \
                 credential was stored for"
            ),
            CapabilityError::NotHttp(url) => write!(
                f,
                "{url} is not an http remote, so a stored token is not how it \
                 authenticates (an ssh remote uses your agent, which a confined \
                 session cannot reach — by design)"
            ),
        }
    }
}

impl std::error::Error for CapabilityError {}

impl Capability {
    /// Parse a capability as typed on the command line.
    pub fn parse(
        name: &str,
        remote: &str,
        branch: Option<&str>,
    ) -> Result<Capability, CapabilityError> {
        if !valid_remote_name(remote) {
            return Err(CapabilityError::BadRemoteName(remote.to_string()));
        }
        if let Some(b) = branch {
            if !valid_branch_name(b) {
                return Err(CapabilityError::BadBranchName(b.to_string()));
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
            "git-ls-remote" => Ok(Capability::GitLsRemote {
                remote: remote.to_string(),
            }),
            other => Err(CapabilityError::UnknownCapability(other.to_string())),
        }
    }

    pub fn names() -> &'static [&'static str] {
        &["git-push", "git-fetch", "git-ls-remote"]
    }

    /// The capability name, without its arguments. This is what a grant is
    /// keyed on: granting `git-push` to a project allows pushing that project's
    /// branches, not one specific branch forever.
    pub fn name(&self) -> &'static str {
        match self {
            Capability::GitPush { .. } => "git-push",
            Capability::GitFetch { .. } => "git-fetch",
            Capability::GitLsRemote { .. } => "git-ls-remote",
        }
    }

    pub fn remote(&self) -> &str {
        match self {
            Capability::GitPush { remote, .. }
            | Capability::GitFetch { remote }
            | Capability::GitLsRemote { remote } => remote,
        }
    }

    /// Whether the operation can change the remote.
    ///
    /// Decides which URL is resolved: git honours `remote.<name>.pushurl` and
    /// `url.<base>.pushInsteadOf`, so a repository can send pushes somewhere
    /// the fetch URL never mentions. Pinning the host against the fetch URL and
    /// then pushing to the push URL would check the wrong thing.
    pub fn is_write(&self) -> bool {
        matches!(self, Capability::GitPush { .. })
    }

    /// One line for the audit log and for `apex secret grants`.
    pub fn summary(&self) -> String {
        match self {
            Capability::GitPush { remote, branch } => match branch {
                Some(b) => format!("git push {remote} {b}"),
                None => format!("git push {remote} (current branch)"),
            },
            Capability::GitFetch { remote } => format!("git fetch {remote}"),
            Capability::GitLsRemote { remote } => format!("git ls-remote {remote}"),
        }
    }

    pub fn describe(name: &str) -> &'static str {
        match name {
            "git-push" => "push a branch of this project to one of its own remotes",
            "git-fetch" => "fetch from one of this project's own remotes",
            "git-ls-remote" => "list the refs one of this project's own remotes advertises",
            _ => "",
        }
    }
}

/// §11's capability record: everything the decision was made on, in one place.
///
/// Built by the caller, re-checked by `apex-secretd`, written to the audit log.
/// Two fields are *claims* rather than facts and the daemon says so where it
/// uses them:
///
/// * `agent_session` and `request_origin` are attribution. They come from
///   `apex-agentd`, which runs as the same user as the agent it supervises, so
///   a process with that uid can forge them. They label the trail; they do not
///   authorise anything.
/// * `project` is checked against the grant table, so claiming a different
///   project only reaches capabilities that project was already granted.
///
/// `provider`, `operation`, `resource` and the host pin behind them are facts
/// the daemon establishes for itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityRecord {
    /// The stored credential this operation runs under, by name.
    pub provider: String,
    /// The capability, with its arguments.
    #[serde(flatten)]
    pub operation: Capability,
    /// What the operation touches, as the caller named it: a git remote NAME.
    pub resource: String,
    /// Project root the grant is keyed on. Absent means no grant can match.
    #[serde(default)]
    pub project: Option<String>,
    /// Attribution only. Forwarded by `apex-agentd`; not verified.
    #[serde(default)]
    pub agent_session: Option<u32>,
    /// Attribution only: `local`, `remote-control`, `unknown`.
    #[serde(default = "unknown_origin")]
    pub request_origin: String,
    /// When the decision stops being valid, ms since the epoch.
    ///
    /// A capability request is not a bearer token here — the daemon performs
    /// the operation before it replies — so this exists to bound clock skew and
    /// replay from a queued request, not to expire a credential in an agent's
    /// hands. There is never a credential in an agent's hands.
    #[serde(default)]
    pub expiry: Option<u64>,
    /// Extra restrictions the caller applied to itself, recorded verbatim.
    #[serde(default)]
    pub constraints: Vec<String>,
    /// How the decision was reached: `grant` (a standing per-project grant) is
    /// the only policy this build implements. Prompt-on-use is P0-009.
    #[serde(default = "grant_policy")]
    pub approval_policy: String,
    /// Minted by `apex-secretd`, never by the caller. Empty on the way in.
    #[serde(default)]
    pub audit_id: String,
}

fn unknown_origin() -> String {
    "unknown".to_string()
}

fn grant_policy() -> String {
    "grant".to_string()
}

impl CapabilityRecord {
    /// A record for `operation` on `provider`, with everything else defaulted.
    pub fn new(provider: &str, operation: Capability) -> CapabilityRecord {
        CapabilityRecord {
            provider: provider.to_string(),
            resource: operation.remote().to_string(),
            operation,
            project: None,
            agent_session: None,
            request_origin: unknown_origin(),
            expiry: None,
            constraints: Vec::new(),
            approval_policy: grant_policy(),
            audit_id: String::new(),
        }
    }

    /// Whether the record has passed its expiry.
    ///
    /// A record with no expiry never expires; the daemon applies its own bound
    /// instead. `now` is passed in so this is testable without waiting.
    pub fn is_expired(&self, now: u64) -> bool {
        self.expiry.is_some_and(|at| now > at)
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
    name.chars()
        .all(|c| (c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-' | '/' | '+')) && !c.is_control())
}

/// The scheme and host of an http(s) git URL.
///
/// Handles the `https://user@host/path` form, because a URL carrying
/// credentials is exactly the case where taking everything before the first `/`
/// gives the wrong host. The port stays out of the host: a credential is stored
/// for a host, and a loopback fixture on a random port must still match.
pub fn http_endpoint(url: &str) -> Option<(&'static str, String)> {
    let (scheme, rest) = if let Some(r) = url.strip_prefix("https://") {
        ("https", r)
    } else if let Some(r) = url.strip_prefix("http://") {
        ("http", r)
    } else {
        return None;
    };
    let authority = rest.split('/').next()?;
    let host = authority.rsplit('@').next()?;
    // An IPv6 literal is bracketed, and its colons are not a port separator.
    let host = if let Some(inner) = host.strip_prefix('[') {
        inner.split(']').next()?
    } else {
        host.split(':').next()?
    };
    if host.is_empty() {
        return None;
    }
    Some((scheme, host.to_ascii_lowercase()))
}

/// Whether a host names this machine and nothing else.
///
/// Used for one carve-out: a credential may be stored with the `http` scheme
/// only for a loopback host. The reason is that the credential then never
/// crosses a network, and the alternative is that the credential path cannot be
/// tested end to end without either a real provider or a certificate authority
/// in the test fixture.
///
/// It is not a hole an agent can open. Only the owner adds a service record,
/// `apex secret add` refuses `--scheme http` for anything but these hosts, and
/// the host is pinned from then on.
pub fn is_loopback_host(host: &str) -> bool {
    if host == "localhost" || host == "::1" {
        return true;
    }
    // 127.0.0.0/8, checked as an address rather than a prefix string so
    // `127.0.0.1.attacker.example` is not read as loopback.
    match host.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(v4)) => v4.is_loopback(),
        Ok(std::net::IpAddr::V6(v6)) => v6.is_loopback(),
        Err(_) => false,
    }
}

/// Check a resolved remote URL against the credential it will be used with.
///
/// `url` is what the repository itself says, resolved by `apex-secretd` — never
/// anything the caller supplied.
pub fn check_url(url: &str, service_host: &str, service_scheme: &str) -> Result<(), CapabilityError> {
    let (scheme, host) =
        http_endpoint(url).ok_or_else(|| CapabilityError::NotHttp(url.to_string()))?;
    if scheme != service_scheme {
        return Err(CapabilityError::SchemeMismatch {
            url: url.to_string(),
            service_scheme: service_scheme.to_string(),
        });
    }
    if host != service_host.to_ascii_lowercase() {
        return Err(CapabilityError::HostMismatch {
            remote_host: host,
            service_host: service_host.to_string(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_vocabulary_is_closed() {
        for evil in ["exec", "sh", "git-clone", "curl", "run", ""] {
            assert!(
                matches!(
                    Capability::parse(evil, "origin", None),
                    Err(CapabilityError::UnknownCapability(_))
                ),
                "'{evil}' was accepted as a capability"
            );
        }
        for good in Capability::names() {
            assert!(Capability::parse(good, "origin", None).is_ok(), "{good}");
        }
    }

    #[test]
    fn every_name_has_a_description_and_a_summary() {
        // `apex secret capabilities` prints both. A capability added without a
        // description prints a blank line and reads as a bug in the CLI.
        for name in Capability::names() {
            assert!(!Capability::describe(name).is_empty(), "{name}");
            let cap = Capability::parse(name, "origin", None).unwrap();
            assert_eq!(cap.name(), *name);
            assert!(cap.summary().contains("origin"), "{name}");
        }
    }

    #[test]
    fn a_remote_may_not_be_a_url_or_an_option() {
        // The hole this closes: with a URL accepted, a session asks the broker
        // to push to a host it controls and the broker does it, token attached.
        for evil in [
            "https://attacker.example/r",
            "git@github.com:a/b",
            "-f",
            "--force",
            "../x",
            "a b",
            "",
            "ori\ngin",
        ] {
            assert!(
                matches!(
                    Capability::parse("git-fetch", evil, None),
                    Err(CapabilityError::BadRemoteName(_))
                ),
                "'{}' was accepted as a remote name",
                evil.escape_debug()
            );
        }
    }

    #[test]
    fn a_branch_may_not_be_an_option_or_a_revision_range() {
        for evil in ["-f", "a..b", "x/", "/x", "a//b", "a.lock", "@", ""] {
            assert!(
                matches!(
                    Capability::parse("git-push", "origin", Some(evil)),
                    Err(CapabilityError::BadBranchName(_))
                ),
                "'{evil}' was accepted as a branch"
            );
        }
        assert!(Capability::parse("git-push", "origin", Some("feat/x-1.2")).is_ok());
    }

    #[test]
    fn only_push_is_a_write() {
        // Which decides whether the pushurl or the fetch url is resolved and
        // host-pinned. Getting this backwards checks the wrong URL.
        assert!(Capability::parse("git-push", "origin", None).unwrap().is_write());
        assert!(!Capability::parse("git-fetch", "origin", None).unwrap().is_write());
        assert!(!Capability::parse("git-ls-remote", "origin", None)
            .unwrap()
            .is_write());
    }

    #[test]
    fn the_host_of_a_url_ignores_credentials_and_ports() {
        assert_eq!(
            http_endpoint("https://x:tok@github.com:443/a/b.git"),
            Some(("https", "github.com".to_string()))
        );
        assert_eq!(
            http_endpoint("http://127.0.0.1:8080/demo.git"),
            Some(("http", "127.0.0.1".to_string()))
        );
        assert_eq!(
            http_endpoint("https://[::1]:9000/demo.git"),
            Some(("https", "::1".to_string()))
        );
        assert_eq!(http_endpoint("git@github.com:a/b"), None);
        assert_eq!(http_endpoint("ssh://git@github.com/a/b"), None);
        assert_eq!(http_endpoint("https:///a/b"), None);
    }

    #[test]
    fn a_url_must_match_both_the_host_and_the_scheme() {
        assert_eq!(check_url("https://github.com/a/b", "github.com", "https"), Ok(()));
        // Case is not significant in a host, and a credential stored for
        // `GitHub.com` must still match a remote written in lower case.
        assert_eq!(check_url("https://GITHUB.com/a/b", "github.com", "https"), Ok(()));

        assert!(matches!(
            check_url("https://gitlab.com/a/b", "github.com", "https"),
            Err(CapabilityError::HostMismatch { .. })
        ));
        // The downgrade that matters: a credential stored for https must not be
        // sent in clear because the repository says http.
        assert!(matches!(
            check_url("http://github.com/a/b", "github.com", "https"),
            Err(CapabilityError::SchemeMismatch { .. })
        ));
        assert!(matches!(
            check_url("git@github.com:a/b", "github.com", "https"),
            Err(CapabilityError::NotHttp(_))
        ));
    }

    #[test]
    fn loopback_is_recognised_as_an_address_not_as_a_prefix() {
        for host in ["127.0.0.1", "127.9.9.9", "localhost", "::1"] {
            assert!(is_loopback_host(host), "{host}");
        }
        for host in [
            "127.0.0.1.attacker.example",
            "localhost.attacker.example",
            "example.com",
            "0.0.0.0",
            "10.0.0.1",
            "",
        ] {
            assert!(!is_loopback_host(host), "{host}");
        }
    }

    #[test]
    fn a_record_carries_every_field_section_eleven_names() {
        // §11's list, asserted against the serialised form so a rename is a
        // test failure rather than a silently different audit trail.
        let mut rec = CapabilityRecord::new(
            "demo",
            Capability::parse("git-fetch", "origin", None).unwrap(),
        );
        rec.project = Some("/home/x/p".into());
        rec.agent_session = Some(7);
        let json = serde_json::to_value(&rec).unwrap();
        for field in [
            "provider",
            "capability",
            "resource",
            "project",
            "agent_session",
            "request_origin",
            "expiry",
            "constraints",
            "approval_policy",
            "audit_id",
        ] {
            assert!(json.get(field).is_some(), "record has no {field}: {json}");
        }
        // The operation is flattened in, so the record reads as one object
        // rather than a record wrapping an operation.
        assert_eq!(json["capability"], "git-fetch");
        assert_eq!(json["remote"], "origin");
    }

    #[test]
    fn a_record_round_trips_and_expires_on_its_own_clock() {
        let mut rec = CapabilityRecord::new(
            "demo",
            Capability::parse("git-push", "origin", Some("main")).unwrap(),
        );
        assert!(!rec.is_expired(u64::MAX), "no expiry must never expire");
        rec.expiry = Some(1_000);
        assert!(!rec.is_expired(1_000));
        assert!(rec.is_expired(1_001));

        let text = serde_json::to_string(&rec).unwrap();
        assert!(!text.contains('\n'), "the framing is line-based");
        let back: CapabilityRecord = serde_json::from_str(&text).unwrap();
        assert_eq!(back, rec);
    }

    #[test]
    fn an_old_client_record_deserialises_with_defaults() {
        // The wire is a compatibility surface. A caller that predates the
        // origin, expiry or constraint fields must still be understood.
        let rec: CapabilityRecord = serde_json::from_str(
            r#"{"provider":"demo","capability":"git-fetch","remote":"origin","resource":"origin"}"#,
        )
        .unwrap();
        assert_eq!(rec.request_origin, "unknown");
        assert_eq!(rec.approval_policy, "grant");
        assert_eq!(rec.expiry, None);
        assert!(rec.constraints.is_empty());
        assert!(rec.audit_id.is_empty());
    }
}
