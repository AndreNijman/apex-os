//! §11's capability record: everything one decision was made on, in one place.
//!
//! §3.2's flow is `capability request -> policy decision -> broker-owned
//! operation -> provider -> result`, and §11 names the fields the record
//! carries. [`CapabilityRecord`] is that list, and it is the single thing that
//! travels from the caller to `apex-secretd` and from `apex-secretd` into the
//! audit log — so the request, the decision and the trail cannot describe
//! different operations.
//!
//! ## Nothing here knows what git is
//!
//! P0-002 put a closed `Capability` enum in this crate, with git's arguments as
//! its fields. That made every future provider — §14 names seven — an edit to a
//! type in the crate the CLI and the agent runtime both link. P1-001 moved the
//! vocabulary into [`crate::operation`], where a provider *declares* its
//! operations, and moved git's own typed form into the git provider inside
//! `apex-secretd`.
//!
//! What is left here is the part that is the same for every provider: who
//! asked, for what, against which project, under what approval, and with which
//! audit id.
//!
//! ## Why the resource is a NAME
//!
//! Never a URL, for any provider. A session that could name a URL could ask the
//! broker to push to `https://attacker.example/` with the owner's token
//! attached, and the broker would, because it was told to. The provider
//! resolves the name against something the session does not control — for git,
//! the repository's own configuration — and `apex-secretd` then pins the
//! resulting host against the one the credential was stored for. The grammar
//! that keeps a name from *looking* like a URL is
//! [`crate::operation::ResourceKind`].

use serde::{Deserialize, Serialize};

use crate::operation::Params;

/// Why an endpoint was refused.
///
/// Endpoint concerns only. What a *name* means is a provider's business and its
/// errors live with it; where a credential may be sent is the framework's, and
/// these are the three ways that answer can be no.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EndpointError {
    /// The resolved destination is not an http(s) URL, so a stored token is not
    /// how it authenticates.
    NotHttp(String),
    /// The destination's host is not one this credential may be used for.
    HostMismatch {
        remote_host: String,
        service_host: String,
    },
    /// The destination's scheme is not the one the credential was stored for.
    SchemeMismatch { url: String, service_scheme: String },
}

impl std::fmt::Display for EndpointError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EndpointError::NotHttp(url) => write!(
                f,
                "{url} is not an http endpoint, so a stored token is not how it \
                 authenticates"
            ),
            EndpointError::HostMismatch {
                remote_host,
                service_host,
            } => write!(
                f,
                "that resolves to {remote_host}, but this credential is for \
                 {service_host}"
            ),
            EndpointError::SchemeMismatch {
                url,
                service_scheme,
            } => write!(
                f,
                "{url} is not {service_scheme}, which is the scheme this \
                 credential was stored for"
            ),
        }
    }
}

impl std::error::Error for EndpointError {}

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
/// the daemon establishes for itself. So is `approval_policy`, which arrives
/// with whatever the caller put in it and is overwritten before anything runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityRecord {
    /// The stored credential this operation runs under, by name.
    pub provider: String,
    /// The §13.2 operation id: `git.push`, `cloudflare.worker.deploy`.
    ///
    /// A string and not a parsed type, deliberately, and for the same reason
    /// `request_origin` is: what crosses the socket is whatever the caller
    /// wrote, and the daemon is the thing that decides whether it means
    /// anything. A typed field would turn a bad name into a JSON parse error
    /// halfway through a request instead of a sentence somebody can read.
    pub operation: String,
    /// What the operation touches, as the caller named it. A NAME, never a URL.
    pub resource: String,
    /// The operation's arguments, checked against what its provider declared.
    ///
    /// Not one of §11's ten fields, and here for the same reason
    /// `origin_source` is: an operation without arguments cannot say which
    /// branch or which version, and the alternative — a field per provider —
    /// is the coupling P1-001 exists to remove. Every key is one the operation
    /// declares; an undeclared one is refused, not ignored.
    #[serde(default)]
    pub params: Params,
    /// Project root the grant is keyed on. Absent means no grant can match.
    #[serde(default)]
    pub project: Option<String>,
    /// Attribution only. Forwarded by `apex-agentd`; not verified.
    #[serde(default)]
    pub agent_session: Option<u32>,
    /// §7's request origin, as `apex-agentd` established it from the
    /// connection: one of the seven names, or `unknown` when the daemon could
    /// not read the peer's placement.
    ///
    /// Attribution only. `apex-secretd` cannot re-derive it — the connection it
    /// sees is `apex-agentd`'s, not the session's — so it checks the shape and
    /// records the claim. What makes the claim worth having is that the daemon
    /// which does make it observes it from the kernel rather than reading it
    /// off the request.
    #[serde(default = "unknown_origin")]
    pub request_origin: String,
    /// How [`CapabilityRecord::request_origin`] was arrived at: `observed`,
    /// `inherited`, `declared`, or `unknown`.
    ///
    /// Separate from the origin rather than inferred from it, because "the
    /// daemon worked this out" and "something asked for this and was allowed"
    /// are different claims about the same value. A trail that cannot tell them
    /// apart cannot answer the only question worth asking of it.
    #[serde(default = "unknown_origin")]
    pub origin_source: String,
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
    /// How the decision was reached: `grant`, a standing per-project grant, is
    /// the only policy this build implements. Prompt-on-use is P0-009 and
    /// break-glass is P0-005.
    ///
    /// **Written by the daemon**, in `Service::decide`, over whatever arrived.
    /// A caller that could name how it was approved could write `owner` into a
    /// trail nobody then read twice.
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
    /// A record for `operation` on `resource`, with everything else defaulted.
    pub fn new(provider: &str, operation: &str, resource: &str) -> CapabilityRecord {
        CapabilityRecord {
            provider: provider.to_string(),
            operation: operation.to_string(),
            resource: resource.to_string(),
            params: Params::new(),
            project: None,
            agent_session: None,
            request_origin: unknown_origin(),
            origin_source: unknown_origin(),
            expiry: None,
            constraints: Vec::new(),
            approval_policy: grant_policy(),
            audit_id: String::new(),
        }
    }

    /// Add one argument. Checked against the operation's declaration by the
    /// daemon, not here.
    pub fn param(mut self, name: &str, value: &str) -> CapabilityRecord {
        self.params.insert(name.to_string(), value.to_string());
        self
    }

    /// The request in one line, for the audit trail and for a refusal.
    ///
    /// Generic on purpose: the framework has to be able to describe a request
    /// it could not resolve — a bad operation name, an unknown provider — and
    /// at that point nobody can render it in a provider's own words. A provider
    /// that resolves the request supplies a better line through `Bound::detail`,
    /// and the daemon uses that for the line that says the operation ran.
    pub fn summary(&self) -> String {
        let mut out = self.operation.clone();
        if !self.resource.is_empty() {
            out.push(' ');
            out.push_str(&self.resource);
        }
        for (name, value) in &self.params {
            out.push(' ');
            out.push_str(name);
            out.push('=');
            out.push_str(value);
        }
        out
    }

    /// Name the agent that asked, as a constraint.
    ///
    /// §11 fixes the record's fields and "which agent" is not one of them, but
    /// a trail that says `claude` rather than only `session 7` is the one an
    /// owner can read a week later. `constraints` is the field for things the
    /// caller applied to itself, and this is one — recorded, never enforced.
    pub fn agent_session_agent(&mut self, agent: Option<&str>) {
        if let Some(agent) = agent {
            let entry = format!("agent={agent}");
            if !self.constraints.contains(&entry) {
                self.constraints.push(entry);
            }
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

/// Whether a string is one of §7's origin names, or `unknown`.
///
/// A *shape* check, not a vocabulary one, and the difference is deliberate.
/// The vocabulary lives in `apex_agent_core::policy::RequestOrigin`, which is
/// where the value is produced from a typed enum; copying the seven names here
/// would put the same list in two crates and let them drift. What this crate
/// owes the audit trail is that a caller cannot write a paragraph, a newline or
/// a lookalike into the field — so the shape is bounded and kebab-case, and the
/// record says plainly that the content is a claim.
pub fn valid_origin_label(label: &str) -> bool {
    !label.is_empty()
        && label.len() <= 32
        && label
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !label.starts_with('-')
        && !label.ends_with('-')
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

/// The scheme and host of an http(s) URL.
///
/// Handles the `https://user@host/path` form, because a URL carrying
/// credentials is exactly the case where taking everything before the first `/`
/// gives the wrong host. The port stays out of the host: a credential is stored
/// for a host, and a loopback fixture on a random port must still match.
pub fn http_endpoint(url: &str) -> Option<(&'static str, String)> {
    let (scheme, rest) = match url.strip_prefix("https://") {
        Some(rest) => ("https", rest),
        None => ("http", url.strip_prefix("http://")?),
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

/// Check a resolved URL against the credential it will be used with.
///
/// `url` is what the *provider* resolved — never anything the caller supplied.
/// This is the pin, and `apex-secretd` applies it between a provider's `bind`
/// and its `perform`, so no provider can be written that skips it.
pub fn check_url(url: &str, service_host: &str, service_scheme: &str) -> Result<(), EndpointError> {
    let (scheme, host) =
        http_endpoint(url).ok_or_else(|| EndpointError::NotHttp(url.to_string()))?;
    if scheme != service_scheme {
        return Err(EndpointError::SchemeMismatch {
            url: url.to_string(),
            service_scheme: service_scheme.to_string(),
        });
    }
    if host != service_host.to_ascii_lowercase() {
        return Err(EndpointError::HostMismatch {
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
            Err(EndpointError::HostMismatch { .. })
        ));
        // The downgrade that matters: a credential stored for https must not be
        // sent in clear because the repository says http.
        assert!(matches!(
            check_url("http://github.com/a/b", "github.com", "https"),
            Err(EndpointError::SchemeMismatch { .. })
        ));
        assert!(matches!(
            check_url("git@github.com:a/b", "github.com", "https"),
            Err(EndpointError::NotHttp(_))
        ));
    }

    #[test]
    fn an_origin_label_is_bounded_and_cannot_carry_framing() {
        // The trail is line-delimited JSON an administrator greps. A label
        // that could hold a newline, a quote or a paragraph would let the
        // audited party shape what the audit looks like.
        for good in [
            "local-terminal",
            "apex-shell",
            "claude-remote-control",
            "scheduled-job",
            "mcp",
            "subagent",
            "cloud-job",
            "unknown",
        ] {
            assert!(valid_origin_label(good), "{good}");
        }
        for bad in [
            "",
            "-leading",
            "trailing-",
            "Local-Terminal",
            "local terminal",
            "local\nterminal",
            "local\"terminal",
            &"x".repeat(33),
        ] {
            assert!(!valid_origin_label(bad), "'{}' was accepted", bad.escape_debug());
        }
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
        // test failure rather than a silently different audit trail. The names
        // are §11's own — `operation`, not the `capability` P0-002 shipped.
        let mut rec = CapabilityRecord::new("demo", "git.fetch", "origin");
        rec.project = Some("/home/x/p".into());
        rec.agent_session = Some(7);
        let json = serde_json::to_value(&rec).unwrap();
        for field in [
            "provider",
            "operation",
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
        // And the two this build adds, both argued for on their fields.
        assert!(json.get("origin_source").is_some());
        assert!(json.get("params").is_some());
        assert_eq!(json["operation"], "git.fetch");
        assert_eq!(json["resource"], "origin");
    }

    #[test]
    fn a_record_round_trips_and_expires_on_its_own_clock() {
        let mut rec = CapabilityRecord::new("demo", "git.push", "origin").param("branch", "main");
        assert!(!rec.is_expired(u64::MAX), "no expiry must never expire");
        rec.expiry = Some(1_000);
        assert!(!rec.is_expired(1_000));
        assert!(rec.is_expired(1_001));

        let text = serde_json::to_string(&rec).unwrap();
        assert!(!text.contains('\n'), "the framing is line-based");
        let back: CapabilityRecord = serde_json::from_str(&text).unwrap();
        assert_eq!(back, rec);
        assert_eq!(back.params.get("branch").map(String::as_str), Some("main"));
    }

    #[test]
    fn a_record_with_missing_optional_fields_deserialises_with_defaults() {
        // The wire is a compatibility surface. A caller that predates the
        // origin, expiry, parameter or constraint fields must still be
        // understood.
        let rec: CapabilityRecord = serde_json::from_str(
            r#"{"provider":"demo","operation":"git.fetch","resource":"origin"}"#,
        )
        .unwrap();
        assert_eq!(rec.request_origin, "unknown");
        assert_eq!(rec.origin_source, "unknown");
        assert_eq!(rec.approval_policy, "grant");
        assert_eq!(rec.expiry, None);
        assert!(rec.constraints.is_empty());
        assert!(rec.params.is_empty());
        assert!(rec.audit_id.is_empty());
    }

    #[test]
    fn a_record_describes_itself_even_when_nothing_could_resolve_it() {
        // The line a refusal writes. It has to work for a request the daemon
        // rejected before it knew which provider owned it.
        let rec = CapabilityRecord::new("gh", "git.push", "origin").param("branch", "main");
        assert_eq!(rec.summary(), "git.push origin branch=main");
        assert_eq!(
            CapabilityRecord::new("cf", "cloudflare.account.read", "").summary(),
            "cloudflare.account.read"
        );
    }

    #[test]
    fn an_agent_name_is_recorded_once_and_only_when_there_is_one() {
        let mut rec = CapabilityRecord::new("demo", "git.fetch", "origin");
        rec.agent_session_agent(None);
        assert!(rec.constraints.is_empty());
        rec.agent_session_agent(Some("claude"));
        rec.agent_session_agent(Some("claude"));
        assert_eq!(rec.constraints, vec!["agent=claude".to_string()]);
    }
}
