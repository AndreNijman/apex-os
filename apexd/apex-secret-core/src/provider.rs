//! Providers and the operations the broker will perform for a caller (§11, §14).
//!
//! # The shape of an operation
//!
//! An operation is a *named, closed* thing: `github:whoami`, not "issue this
//! request with my token attached". The distinction is the whole service. A
//! vocabulary a caller can extend is a vocabulary in which "use the credential
//! against a host I control" is one of the words, and no reviewer can
//! meaningfully approve a grant written in it.
//!
//! So a caller supplies exactly two pieces of free text — the credential's name
//! and an operation id, both looked up in tables — plus an optional `resource`
//! that is validated against the operation's own shape. The method, the path,
//! the headers and the fields that may come back are all constants in this file.
//!
//! # What comes back
//!
//! The status code, and the values of an allow-list of named scalar fields. Not
//! the response body: an API that echoes a request header into an error message
//! would otherwise hand the credential straight back, and §12's whole point is
//! that it never gets there. The allow-list is per operation, the extraction
//! refuses objects and arrays, and the result is scrubbed of the value anyway.
//!
//! # Why `curl`
//!
//! A workspace whose dependency list is `serde`, `libc` and `clap` is not the
//! place to add a TLS stack. `curl` is on every APEX host, is the most reviewed
//! HTTP client in existence, and `--config -` takes its whole configuration on
//! stdin — so the credential is never an argument (`/proc/<pid>/cmdline` is
//! world-readable), never an environment variable of a process the owner can
//! read, and never a file left behind by a crash.

use std::collections::BTreeMap;
use std::io::Write;
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};

use crate::capability::Constraints;
use crate::value::{self, SecretValue};

/// How a credential is presented to a provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthScheme {
    /// `Authorization: Bearer <value>`.
    Bearer,
}

/// The HTTP method an operation uses.
///
/// One variant. Every operation this service ships is read-only, and adding
/// `Post` is a change a reviewer will see rather than a flag flipped in a table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Get,
}

impl Method {
    pub fn as_str(&self) -> &'static str {
        match self {
            Method::Get => "GET",
        }
    }
}

/// How an operation builds its path, and therefore what `resource` may be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathShape {
    /// No resource. The path is used as written.
    Fixed(&'static str),
    /// `owner/repo`, substituted into the template's two `{}` placeholders.
    OwnerRepo(&'static str),
}

/// One operation a provider offers.
#[derive(Debug, Clone, Copy)]
pub struct OperationSpec {
    pub id: &'static str,
    /// One line, shown by `apex capability providers`.
    pub summary: &'static str,
    pub method: Method,
    pub path: PathShape,
    /// Top-level scalar response fields that may be returned to a caller.
    /// Everything else in the body is discarded unread.
    pub fields: &'static [&'static str],
    /// Whether performing this hands the caller the credential itself.
    ///
    /// Always `false`, and it must stay that way — the policy layer refuses any
    /// operation that sets it, so an export-shaped operation added later is
    /// dead on arrival rather than quietly reachable. This is the
    /// `security_invariants` line "root/system grants must not implicitly
    /// export raw brokered secrets", written where it can be enforced.
    pub exports_value: bool,
}

/// A provider: an endpoint, an auth scheme and a vocabulary.
#[derive(Debug, Clone, Copy)]
pub struct ProviderSpec {
    pub id: &'static str,
    pub summary: &'static str,
    /// Used when a stored credential names no base of its own.
    pub default_base: &'static str,
    pub auth: AuthScheme,
    /// Sent with every request. Never caller-supplied.
    pub headers: &'static [&'static str],
    pub operations: &'static [OperationSpec],
}

/// Every provider this build knows.
///
/// A table rather than a trait object graph, because the roadmap's requirement
/// is that provider plugins can be added "without changing agent core" — and
/// nothing outside this crate names a provider. Adding one is adding a `const`
/// here plus its operations.
pub const PROVIDERS: &[ProviderSpec] = &[GITHUB];

const GITHUB: ProviderSpec = ProviderSpec {
    id: "github",
    summary: "GitHub REST API",
    default_base: "https://api.github.com",
    auth: AuthScheme::Bearer,
    headers: &[
        "Accept: application/vnd.github+json",
        "X-GitHub-Api-Version: 2022-11-28",
        "User-Agent: apex-secretd",
    ],
    operations: &[
        OperationSpec {
            id: "whoami",
            summary: "which account this credential belongs to, and what it can reach",
            method: Method::Get,
            path: PathShape::Fixed("/user"),
            fields: &["login", "id", "type"],
            exports_value: false,
        },
        OperationSpec {
            id: "repo-metadata",
            summary: "visibility and default branch of one repository",
            method: Method::Get,
            path: PathShape::OwnerRepo("/repos/{}/{}"),
            fields: &["full_name", "private", "default_branch", "archived"],
            exports_value: false,
        },
    ],
};

pub fn provider(id: &str) -> Option<&'static ProviderSpec> {
    PROVIDERS.iter().find(|p| p.id == id)
}

impl ProviderSpec {
    pub fn operation(&self, id: &str) -> Option<&'static OperationSpec> {
        self.operations.iter().find(|o| o.id == id)
    }
}

/// Why an operation could not be performed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderError {
    UnknownProvider(String),
    UnknownOperation { provider: String, operation: String },
    /// The operation takes a resource and none was given.
    ResourceRequired { operation: String, shape: &'static str },
    /// The operation takes no resource and one was given.
    ResourceNotAccepted(String),
    BadResource(String),
    /// A stored base URL that is not an absolute http(s) origin.
    BadBaseUrl(String),
    /// `curl` could not be run, or did not finish.
    Transport(String),
    Timeout,
}

impl std::fmt::Display for ProviderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProviderError::UnknownProvider(p) => write!(
                f,
                "'{}' is not a provider this build knows; known providers: {}",
                p.escape_debug(),
                PROVIDERS.iter().map(|p| p.id).collect::<Vec<_>>().join(", ")
            ),
            ProviderError::UnknownOperation { provider, operation } => write!(
                f,
                "'{}' is not an operation {provider} offers; \
                 run `apex capability providers` for the list",
                operation.escape_debug()
            ),
            ProviderError::ResourceRequired { operation, shape } => {
                write!(f, "{operation} needs a resource of the form {shape}")
            }
            ProviderError::ResourceNotAccepted(op) => {
                write!(f, "{op} takes no resource")
            }
            ProviderError::BadResource(r) => write!(
                f,
                "'{}' is not a resource this operation accepts. It goes into a \
                 URL path, so it may only contain letters, digits and _ . -",
                r.escape_debug()
            ),
            ProviderError::BadBaseUrl(u) => write!(
                f,
                "'{}' is not an absolute http or https origin",
                u.escape_debug()
            ),
            ProviderError::Transport(m) => write!(f, "{m}"),
            ProviderError::Timeout => write!(f, "the provider did not answer in time"),
        }
    }
}

impl std::error::Error for ProviderError {}

/// What performing an operation produced. **Never a credential.**
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Outcome {
    /// The provider's HTTP status.
    pub status: u16,
    /// The allow-listed fields the provider returned, stringified.
    pub fields: BTreeMap<String, String>,
    /// A short message when the provider refused, taken from a `message` field
    /// if it sent one. Scrubbed like everything else.
    #[serde(default)]
    pub detail: Option<String>,
}

impl Outcome {
    /// Whether the provider accepted the request.
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

/// Check a base URL before a credential is ever sent to it.
///
/// Only reachable by an administrator storing a credential (a GitHub Enterprise
/// host, a Cloudflare tenant endpoint), never by the caller of an operation —
/// which is the point. A caller who could choose the base could choose where
/// the token goes.
pub fn valid_base_url(url: &str) -> bool {
    let Some(rest) = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
    else {
        return false;
    };
    if rest.is_empty() || url.len() > 512 {
        return false;
    }
    // No userinfo, no query, no fragment: each is a way to make the effective
    // request target differ from what a reviewer reads.
    if rest.contains('@') || rest.contains('?') || rest.contains('#') {
        return false;
    }
    let authority = rest.split('/').next().unwrap_or("");
    if authority.is_empty() {
        return false;
    }
    if url.ends_with('/') {
        return false;
    }
    !url.chars().any(|c| c.is_control() || c == ' ' || c == '"' || c == '\\')
}

/// One path segment of a resource: what may be interpolated into a URL.
fn valid_segment(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 100
        && s != "."
        && s != ".."
        && !s.starts_with('-')
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
}

/// Build the request path, validating `resource` against the operation's shape.
pub fn resolve_path(
    op: &OperationSpec,
    resource: Option<&str>,
) -> Result<String, ProviderError> {
    match op.path {
        PathShape::Fixed(p) => match resource {
            None => Ok(p.to_string()),
            Some(_) => Err(ProviderError::ResourceNotAccepted(op.id.to_string())),
        },
        PathShape::OwnerRepo(template) => {
            let Some(resource) = resource else {
                return Err(ProviderError::ResourceRequired {
                    operation: op.id.to_string(),
                    shape: "owner/repo",
                });
            };
            let mut parts = resource.split('/');
            let (Some(owner), Some(repo), None) = (parts.next(), parts.next(), parts.next())
            else {
                return Err(ProviderError::BadResource(resource.to_string()));
            };
            if !valid_segment(owner) || !valid_segment(repo) {
                return Err(ProviderError::BadResource(resource.to_string()));
            }
            Ok(template.replacen("{}", owner, 1).replacen("{}", repo, 1))
        }
    }
}

/// The `curl` configuration for one request, as it goes onto curl's stdin.
///
/// Split out from [`perform`] so the shape of what carries the credential is
/// something a test can read, and so the escaping is checked without a network.
pub fn curl_config(
    spec: &ProviderSpec,
    op: &OperationSpec,
    url: &str,
    value: &SecretValue,
    constraints: &Constraints,
) -> String {
    let mut cfg = String::new();
    cfg.push_str(&format!("url = \"{}\"\n", escape(url)));
    cfg.push_str(&format!("request = \"{}\"\n", op.method.as_str()));
    match spec.auth {
        AuthScheme::Bearer => {
            cfg.push_str(&format!(
                "header = \"Authorization: Bearer {}\"\n",
                escape(value.expose())
            ));
        }
    }
    for h in spec.headers {
        cfg.push_str(&format!("header = \"{}\"\n", escape(h)));
    }
    cfg.push_str(&format!("max-time = {}\n", constraints.timeout_secs));
    // Follow nothing. A redirect is the provider choosing a new host for the
    // request, and the request carries a credential. Spelled `no-location`
    // rather than `location = false`: curl's configuration parser reads a
    // boolean as a bare flag and rejects a value after one, so the `= false`
    // form fails the whole config — which would have meant no request at all
    // rather than an unfollowed redirect.
    cfg.push_str("no-location\n");
    cfg.push_str("silent\n");
    cfg.push_str("show-error\n");
    // The body, then a newline, then the status on its own last line.
    cfg.push_str("write-out = \"\\n%{http_code}\"\n");
    cfg
}

/// Escape a value for curl's configuration parser.
///
/// It reads double-quoted values with backslash escapes, so those two
/// characters are the whole job. [`crate::value::SecretValue`] already refuses
/// control characters, which is what makes this two lines rather than a parser.
fn escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Perform one operation and return what may be returned.
///
/// This is the only function in the workspace that reads a credential and puts
/// it on a wire. Everything above it decides whether it may be called.
pub fn perform(
    spec: &ProviderSpec,
    op: &OperationSpec,
    base: &str,
    resource: Option<&str>,
    value: &SecretValue,
    constraints: &Constraints,
) -> Result<Outcome, ProviderError> {
    if !valid_base_url(base) {
        return Err(ProviderError::BadBaseUrl(base.to_string()));
    }
    let path = resolve_path(op, resource)?;
    let url = format!("{base}{path}");
    let config = curl_config(spec, op, &url, value, constraints);

    // `timeout` outside curl's own `max-time` as well: curl's timer covers the
    // transfer, not a process wedged before it starts one.
    let mut child = Command::new("timeout")
        .arg((constraints.timeout_secs + 5).to_string())
        .arg("curl")
        .args(["--config", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| ProviderError::Transport(format!("running curl: {e}")))?;

    let mut pipe = child
        .stdin
        .take()
        .ok_or_else(|| ProviderError::Transport("curl gave us no stdin".to_string()))?;
    let writer = std::thread::spawn(move || {
        let _ = pipe.write_all(config.as_bytes());
        drop(pipe);
    });
    let out = child
        .wait_with_output()
        .map_err(|e| ProviderError::Transport(e.to_string()))?;
    let _ = writer.join();

    if out.status.code() == Some(124) {
        return Err(ProviderError::Timeout);
    }
    let body = String::from_utf8_lossy(&out.stdout);
    let body = &body[..body.len().min(constraints.max_response_bytes)];

    if !out.status.success() {
        // curl failed before it had a response. Its stderr describes the
        // network, not the request — but it is scrubbed anyway, because "curl
        // never prints the configuration back" is an assumption about somebody
        // else's code.
        let err = value::scrub(String::from_utf8_lossy(&out.stderr).trim(), value);
        return Err(ProviderError::Transport(if err.is_empty() {
            format!("curl exited {}", out.status.code().unwrap_or(-1))
        } else {
            err
        }));
    }

    Ok(extract(op, body, value))
}

/// Pull the allow-listed fields out of a response body.
///
/// Everything not on the list is discarded without being looked at, and a field
/// that is an object or an array is discarded too: a response can only leave
/// here as a handful of named scalars.
fn extract(op: &OperationSpec, raw: &str, value: &SecretValue) -> Outcome {
    let (body, status) = split_status(raw);
    let parsed: Option<serde_json::Value> = serde_json::from_str(body).ok();

    let mut fields = BTreeMap::new();
    let mut detail = None;
    if let Some(serde_json::Value::Object(map)) = parsed {
        for name in op.fields {
            if let Some(v) = map.get(*name) {
                if let Some(text) = scalar(v) {
                    fields.insert((*name).to_string(), value::scrub(&text, value));
                }
            }
        }
        // Providers put their refusal in `message`. Worth having: "401 Bad
        // credentials" is the answer to "has this token expired".
        if let Some(text) = map.get("message").and_then(scalar) {
            detail = Some(value::scrub(&text, value));
        }
    }
    Outcome {
        status,
        fields,
        detail,
    }
}

/// Split curl's output into the body and the status code it appended.
fn split_status(raw: &str) -> (&str, u16) {
    match raw.rfind('\n') {
        Some(at) => {
            let status = raw[at + 1..].trim().parse().unwrap_or(0);
            (&raw[..at], status)
        }
        None => (raw, 0),
    }
}

/// A JSON scalar as text. Objects and arrays return `None` and are dropped.
fn scalar(v: &serde_json::Value) -> Option<String> {
    match v {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        serde_json::Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn github() -> &'static ProviderSpec {
        provider("github").expect("github is a provider")
    }

    fn op(id: &str) -> &'static OperationSpec {
        github().operation(id).expect(id)
    }

    // ── the vocabulary is closed ────────────────────────────────────────────

    #[test]
    fn there_is_no_operation_that_issues_a_request_of_the_callers_choosing() {
        for attempt in ["get", "request", "http", "fetch", "raw", "api", "curl", "exec"] {
            assert!(
                github().operation(attempt).is_none(),
                "'{attempt}' must not be an operation"
            );
        }
    }

    #[test]
    fn no_shipped_operation_exports_the_credential() {
        // The `security_invariants` line, as an assertion. policy.rs refuses
        // any operation that sets this; here we check none does.
        for p in PROVIDERS {
            for o in p.operations {
                assert!(!o.exports_value, "{}:{} exports its credential", p.id, o.id);
                assert!(!o.summary.is_empty(), "{}:{} has no summary", p.id, o.id);
                assert_eq!(o.method, Method::Get, "{}:{} is not read-only", p.id, o.id);
            }
            assert!(valid_base_url(p.default_base), "{}", p.default_base);
        }
    }

    #[test]
    fn provider_and_operation_lookup_is_by_exact_name() {
        assert!(provider("github").is_some());
        assert!(provider("GitHub").is_none());
        assert!(provider("github.com").is_none());
        assert!(github().operation("whoami").is_some());
        assert!(github().operation("whoami ").is_none());
    }

    // ── the resource cannot reshape the URL ─────────────────────────────────

    #[test]
    fn a_fixed_operation_refuses_a_resource() {
        assert_eq!(resolve_path(op("whoami"), None).unwrap(), "/user");
        assert!(matches!(
            resolve_path(op("whoami"), Some("anything")),
            Err(ProviderError::ResourceNotAccepted(_))
        ));
    }

    #[test]
    fn an_owner_repo_resource_is_two_validated_segments() {
        assert_eq!(
            resolve_path(op("repo-metadata"), Some("AndreNijman/apex-os")).unwrap(),
            "/repos/AndreNijman/apex-os"
        );
        assert!(matches!(
            resolve_path(op("repo-metadata"), None),
            Err(ProviderError::ResourceRequired { .. })
        ));
    }

    #[test]
    fn a_resource_cannot_escape_the_path_it_is_substituted_into() {
        // The hole this closes: a resource that walks out of /repos/ turns a
        // grant for "read one repository" into a grant for any GitHub endpoint
        // the token can reach.
        for evil in [
            "../../user",
            "a/../../x",
            "a/b/c",
            "a",
            "/absolute/x",
            "a/b?query",
            "a/b#frag",
            "a b/c",
            "a/b\nc",
            "-x/y",
            "../x/y",
            "a/..",
            "a/.",
            "@evil/x",
            "a%2f../y",
        ] {
            assert!(
                resolve_path(op("repo-metadata"), Some(evil)).is_err(),
                "{evil:?} must be refused"
            );
        }
    }

    // ── the base URL is not the caller's to choose ──────────────────────────

    #[test]
    fn a_base_url_must_be_a_bare_http_origin() {
        for good in [
            "https://api.github.com",
            "http://127.0.0.1:8080",
            "https://github.example.com/api/v3",
        ] {
            assert!(valid_base_url(good), "{good:?}");
        }
        for evil in [
            "https://user@evil.example",
            "https://api.github.com/",
            "ftp://x",
            "//evil.example",
            "https://",
            "https://x?a=b",
            "https://x#f",
            "https://x y",
            "https://x\"",
            "https://x\\",
            "https://x\nurl = \"https://evil.example\"",
            "",
        ] {
            assert!(!valid_base_url(evil), "{evil:?} must be refused");
        }
    }

    // ── the credential's route to curl ──────────────────────────────────────

    #[test]
    fn the_credential_travels_in_the_configuration_and_never_in_argv() {
        let v = SecretValue::new("not-a-real-token-argv").unwrap();
        let cfg = curl_config(
            github(),
            op("whoami"),
            "https://api.github.com/user",
            &v,
            &Constraints::default(),
        );
        assert!(cfg.contains("Authorization: Bearer not-a-real-token-argv"), "{cfg}");
        assert!(cfg.contains("url = \"https://api.github.com/user\""), "{cfg}");
        // Redirects off: a 302 is the provider choosing a new host for a
        // request that carries a credential.
        assert!(cfg.contains("no-location"), "{cfg}");
        assert!(cfg.contains("max-time = 20"), "{cfg}");
    }

    #[test]
    fn a_credential_containing_a_quote_cannot_break_out_of_the_configuration() {
        // Printable ASCII includes `"` and `\`, so a token may contain both.
        // Unescaped, `"` would close the header value and the rest of the token
        // would be read as further curl directives — `url = ...` among them.
        let v = SecretValue::new("a\"b\\c").unwrap();
        let cfg = curl_config(
            github(),
            op("whoami"),
            "https://api.github.com/user",
            &v,
            &Constraints::default(),
        );
        assert!(cfg.contains(r#"Bearer a\"b\\c"#), "{cfg}");
        // Exactly one url directive, one request directive: nothing was injected.
        assert_eq!(cfg.matches("\nurl = ").count() + usize::from(cfg.starts_with("url = ")), 1, "{cfg}");
        // And every line is a directive, not a fragment of a broken one.
        for line in cfg.lines() {
            assert!(
                line.contains(" = ") || matches!(line, "silent" | "show-error" | "no-location"),
                "stray line {line:?} in {cfg}"
            );
        }
    }

    // ── what may come back ──────────────────────────────────────────────────

    #[test]
    fn only_allow_listed_scalar_fields_survive_extraction() {
        let v = SecretValue::new("not-a-real-token-extract").unwrap();
        let body = r#"{"login":"andre","id":42,"type":"User","email":"private@example.com",
            "plan":{"name":"pro"},"orgs":["a","b"]}"#;
        let out = extract(op("whoami"), &format!("{body}\n200"), &v);
        assert_eq!(out.status, 200);
        assert!(out.ok());
        assert_eq!(out.fields.get("login").map(String::as_str), Some("andre"));
        assert_eq!(out.fields.get("id").map(String::as_str), Some("42"));
        assert_eq!(out.fields.get("type").map(String::as_str), Some("User"));
        // Not on the list, so it never leaves — even though the provider sent it.
        assert!(!out.fields.contains_key("email"), "{:?}", out.fields);
        // On no list and not a scalar either.
        assert!(!out.fields.contains_key("plan"));
        assert!(!out.fields.contains_key("orgs"));
    }

    #[test]
    fn a_provider_that_echoes_the_credential_back_does_not_get_to() {
        // Not hypothetical: APIs put request headers into error messages. The
        // allow-list would already drop an unknown field, so this is the case
        // where the echo lands in one that IS allow-listed.
        let v = SecretValue::new("not-a-real-token-echo").unwrap();
        let body = r#"{"login":"used not-a-real-token-echo","message":"bad credentials: Bearer not-a-real-token-echo"}"#;
        let out = extract(op("whoami"), &format!("{body}\n401"), &v);
        assert_eq!(out.status, 401);
        assert!(!out.ok());
        let all = format!("{out:?}");
        assert!(!all.contains("not-a-real-token-echo"), "{all}");
        assert!(all.contains("<withheld>"), "{all}");
    }

    #[test]
    fn a_refusal_keeps_its_message_so_an_expired_token_is_diagnosable() {
        let v = SecretValue::new("not-a-real-token-401").unwrap();
        let out = extract(op("whoami"), "{\"message\":\"Bad credentials\"}\n401", &v);
        assert_eq!(out.detail.as_deref(), Some("Bad credentials"));
        assert!(out.fields.is_empty());
    }

    #[test]
    fn a_body_that_is_not_json_yields_a_status_and_nothing_else() {
        // An HTML error page from a proxy, which is what a misconfigured base
        // URL actually produces.
        let v = SecretValue::new("not-a-real-token-html").unwrap();
        let out = extract(op("whoami"), "<html>502 Bad Gateway</html>\n502", &v);
        assert_eq!(out.status, 502);
        assert!(out.fields.is_empty());
        assert_eq!(out.detail, None);
    }

    #[test]
    fn the_status_is_taken_from_the_last_line_even_when_the_body_has_newlines() {
        assert_eq!(split_status("{\n \"a\": 1\n}\n204").1, 204);
        assert_eq!(split_status("no newline at all").1, 0);
        assert_eq!(split_status("body\nnot-a-number").1, 0);
    }
}
