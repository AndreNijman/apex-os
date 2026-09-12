//! S3, and every S3-compatible endpoint. §14's `AWSProvider`, P2-001's fifth
//! target kind.
//!
//! This is the half of `s3` that was missing. `target::Kind::S3` refused with a
//! reason for a whole round, and the reason was true: nothing in this workspace
//! could sign a request, and a REST transport that can only send a bearer token
//! cannot spend SigV4 credentials. [`sigv4`] is the signer; this module is what
//! spends it, **inside the daemon**, so that the secret access key is never in
//! a process the agent can read — the same property every other provider here
//! has, and the reason an S3 target could not simply be given a credential.
//!
//! # What a project has to declare
//!
//! ```toml
//! [s3]
//! buckets = ["example-backups"]
//! region  = "ap-southeast-2"
//! ```
//!
//! `buckets` is the binding, and it is the check that is not vacuous: a bucket
//! this project did not write down is refused before anything is signed, which
//! is exactly what `[cloudflare] buckets` does for R2.
//!
//! # Where the endpoint comes from, and why the host pin is not the check here
//!
//! The endpoint is **the stored credential's own host and scheme**, from
//! `ServiceInfo`. That is deliberate and it is worth saying plainly rather than
//! letting a reader assume the pin is doing work it is not: because this
//! provider derives the endpoint from the credential, the framework's host pin
//! is satisfied by construction and can refuse nothing. It could not be
//! otherwise — S3-compatible endpoints are everywhere, and a build that
//! hard-coded `s3.amazonaws.com` would be a build that only spoke to AWS.
//!
//! So what constrains this operation is:
//!
//! * **the grant**, which is per project, per credential and per operation;
//! * **the bucket binding** in the project's own `apex.toml`;
//! * **the credential's own scope**, which is the far side's business.
//!
//! Storing the credential is where the host is chosen, and `apex secret add`
//! already refuses `http` for anything that is not loopback — which is what
//! lets a test point this at a double on `127.0.0.1` without that being a way
//! to send a real key in clear over a network.
//!
//! # Path style, not virtual-host style
//!
//! `https://<host>/<bucket>/<key>`, never `https://<bucket>.<host>/<key>`. Two
//! reasons and both are practical: a bucket in the host name is a different
//! host, so it would have to be pinned separately and could not be checked
//! against a credential stored for the endpoint; and every S3-compatible server
//! anyone runs on loopback speaks path style, while virtual-host style needs
//! wildcard DNS.
//!
//! # The listing is rendered, not passed through
//!
//! S3 answers `ListObjectsV2` with XML. This provider renders it as the same
//! `{"result":[{"key":…}]}` envelope `cloudflare.r2.object.read` answers with,
//! so that one consumer parses one shape rather than two — the backup framework
//! then reaches R2 and S3 with the same target code and one operation id
//! swapped.
//!
//! **A truncated listing is an error and never a short list.** S3 returns at
//! most 1000 keys per response and says `<IsTruncated>true</IsTruncated>` when
//! there are more. This follows the continuation token up to
//! [`MAX_LIST_PAGES`] pages and, if it is *still* truncated, refuses — because a
//! listing that is quietly short is a backup history with snapshots missing
//! from it, which is the defect this whole subsystem is careful about.

pub mod sigv4;

use apex_secret_core::operation::{
    Effect, OperationSpec, ParamSpec, ProviderSpec, ResourceKind, Syntax,
};
use apex_secret_core::project::{self, ProjectConfig};
use apex_secret_core::SecretValue;

use crate::broker;
use crate::provider::{
    Approval, Bind, Bound, Endpoint, Performed, Provider, ProviderError,
};

/// Largest object this will upload, matching `cloudflare.r2.object.write`.
pub const MAX_PAYLOAD: u64 = 10 * 1024 * 1024;

/// How many `ListObjectsV2` pages one `s3.object.read` will follow.
///
/// 20 pages is 20,000 keys. Beyond that the listing refuses rather than
/// returning what it has: see the module note.
pub const MAX_LIST_PAGES: usize = 20;

/// The region used when the project names none.
///
/// `us-east-1` because it is what S3 itself treats as the default and what
/// every S3-compatible server accepts when the region is not meaningful to it.
pub const DEFAULT_REGION: &str = "us-east-1";

/// How long one call may take, end to end.
const TIMEOUT_SECS: u64 = 120;
const CONNECT_TIMEOUT_SECS: u64 = 20;

const FILE: ParamSpec = ParamSpec {
    name: "file",
    syntax: Syntax::Path,
    required: true,
    summary: "the file inside this project to upload",
};

pub const SPEC: ProviderSpec = ProviderSpec {
    id: "s3",
    summary: "read and write objects in the S3 buckets this project declares, \
              against the endpoint its credential was stored for",
    operations: &[
        OperationSpec {
            id: "s3.object.read",
            // The same one-operation-for-both shape `cloudflare.r2.object.read`
            // has, and for the same reason: the grant an owner gives is the
            // same either way — this project's buckets, read.
            summary: "read one object out of one of this project's S3 buckets, \
                      or list what is in it",
            effect: Effect::Read,
            resource: ResourceKind::Path,
            params: &[],
            aliases: &[],
            same_everywhere: false,
        },
        OperationSpec {
            id: "s3.object.write",
            summary: "put a file from this project into one of its S3 buckets",
            effect: Effect::Write,
            resource: ResourceKind::Path,
            params: &[FILE],
            aliases: &[],
            same_everywhere: false,
        },
    ],
};

/// What the caller named, resolved against the project.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Target {
    bucket: String,
    /// `None` means the whole bucket, which is a listing.
    key: Option<String>,
    region: String,
}

pub struct S3Provider;

impl S3Provider {
    /// Read `[s3]` out of the project's own `apex.toml`.
    ///
    /// The same reader `[identity.cloudflare]` and `[cloudflare] buckets` go
    /// through: `O_NOFOLLOW` per component, owner checked, size capped, and a
    /// parse failure reported as a position and never as the text at it.
    ///
    /// **A file that could not be read refuses.** A project whose `apex.toml`
    /// is unreadable has not told this build that any bucket is allowed, and
    /// treating it as binding nothing is how an agent that can `chmod 000
    /// apex.toml` would take the binding away.
    fn resolve(&self, req: &Bind<'_>) -> Result<Target, ProviderError> {
        let (bucket, key) = match req.resource.split_once('/') {
            Some((bucket, key)) if !key.is_empty() => (bucket, Some(key.to_string())),
            _ => (req.resource, None),
        };
        if bucket.is_empty() {
            return Err(ProviderError::NoSuchResource(
                "this operation needs a bucket: <bucket> to list it, or \
                 <bucket>/<key> for one object"
                    .to_string(),
            ));
        }

        let config = ProjectConfig::read(
            std::path::Path::new(req.project),
            req.owner.uid,
            &req.owner.name,
        )
        .map_err(|e| {
            ProviderError::Refused(format!(
                "this project's apex.toml could not be read, so which S3 \
                 buckets it binds is unknown, and this build will not write to \
                 a bucket it cannot check: {e}"
            ))
        })?;

        let buckets = config
            .strings(&["s3", "buckets"])
            .map_err(|e| ProviderError::Refused(e.to_string()))?;
        if !buckets.iter().any(|b| b == bucket) {
            // `NoSuchResource` and not `Refused`: the name does not RESOLVE in
            // this project, which is a different answer from "you may not
            // write objects", and sending somebody to the grant table over a
            // typo is a defect this repository has produced before.
            return Err(ProviderError::NoSuchResource(format!(
                "'{}' is not one of the S3 buckets this project declares. \
                 Add it to [s3] buckets in apex.toml. It binds: [{}]",
                bucket.escape_debug(),
                buckets.join(", ")
            )));
        }

        let region = config
            .string(&["s3", "region"])
            .map_err(|e| ProviderError::Refused(e.to_string()))?
            .unwrap_or(DEFAULT_REGION)
            .to_string();
        if region.is_empty() || !region.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
            return Err(ProviderError::Refused(format!(
                "[s3] region = '{}' is not a region name — letters, digits and \
                 dashes",
                region.escape_debug()
            )));
        }

        Ok(Target {
            bucket: bucket.to_string(),
            key,
            region,
        })
    }
}

/// The `Host` header value, which is what gets signed.
///
/// The port is included only when it is not the scheme's default, because that
/// is what every HTTP client sends and the signature has to match the bytes on
/// the wire.
fn host_header(service: &apex_secret_core::store::ServiceInfo) -> String {
    match service.port {
        Some(port) if !is_default_port(&service.scheme, port) => {
            format!("{}:{}", service.host, port)
        }
        _ => service.host.clone(),
    }
}

fn is_default_port(scheme: &str, port: u16) -> bool {
    (scheme == "https" && port == 443) || (scheme == "http" && port == 80)
}

/// `https://host[:port]` — no path.
fn origin(service: &apex_secret_core::store::ServiceInfo) -> String {
    format!("{}://{}", service.scheme, host_header(service))
}

impl Provider for S3Provider {
    fn spec(&self) -> &'static ProviderSpec {
        &SPEC
    }

    fn bind(&self, req: &Bind<'_>) -> Result<Bound, ProviderError> {
        let target = self.resolve(req)?;
        if req.operation.id == "s3.object.write" && target.key.is_none() {
            return Err(ProviderError::NoSuchResource(format!(
                "'{}' names a bucket and not an object. Writing needs a key as \
                 well: {}/<key>",
                target.bucket, target.bucket
            )));
        }
        let detail = match (&target.key, req.params.get("file")) {
            (Some(key), Some(file)) => format!(
                "write {file} to object {key} in S3 bucket {} [{}] at {}",
                target.bucket,
                target.region,
                req.service.host
            ),
            (Some(key), None) => format!(
                "read object {key} in S3 bucket {} [{}] at {}",
                target.bucket, target.region, req.service.host
            ),
            (None, _) => format!(
                "list the objects in S3 bucket {} [{}] at {}",
                target.bucket, target.region, req.service.host
            ),
        };
        Ok(Bound {
            endpoint: Endpoint::from_url(&origin(req.service))?,
            detail,
            creates: None,
            // Nothing here reaches a second thing the owner would want to be
            // asked about separately: the bucket is bound in the project's own
            // file and the endpoint is the credential's.
            approval: Approval::Standing,
        })
    }

    fn perform(
        &self,
        req: &Bind<'_>,
        _bound: &Bound,
        value: &SecretValue,
    ) -> Result<Performed, ProviderError> {
        let target = self.resolve(req)?;
        let access_key_id = req.service.username.trim();
        if access_key_id.is_empty() || access_key_id == "x-access-token" {
            return Err(ProviderError::Refused(format!(
                "the credential '{}' has no access key id. An S3 credential is \
                 two halves — store it with `apex secret add {} --username \
                 <ACCESS_KEY_ID>` and the secret access key as the value",
                req.service.service.escape_debug(),
                req.service.service.escape_debug()
            )));
        }
        let Some(secret) = value.as_str() else {
            return Err(ProviderError::Refused(
                "the stored secret access key is not text, so it cannot be a \
                 SigV4 secret"
                    .to_string(),
            ));
        };

        match (req.operation.id, &target.key) {
            ("s3.object.read", None) => self.list(req, &target, access_key_id, secret),
            ("s3.object.read", Some(key)) => {
                let reply = self.call(
                    req,
                    &target,
                    "GET",
                    key,
                    "",
                    &[],
                    access_key_id,
                    secret,
                    None,
                )?;
                Ok(Performed {
                    code: i32::from(!reply.ok),
                    output: reply.body,
                    created: None,
                })
            }
            ("s3.object.write", Some(key)) => {
                let Some(file) = req.params.get("file") else {
                    // The framework checks `required`, so this is unreachable
                    // through the pipeline. It is here because a provider that
                    // trusted the framework's checks would stop being correct
                    // the day they move.
                    return Err(ProviderError::Refused(
                        "this operation needs a 'file' option naming the file \
                         inside the project to upload"
                            .to_string(),
                    ));
                };
                let bytes = project::read_file(
                    std::path::Path::new(req.project),
                    file,
                    req.owner.uid,
                    &req.owner.name,
                    MAX_PAYLOAD,
                )
                .map_err(|e| ProviderError::NoSuchResource(e.to_string()))?;
                let reply = self.call(
                    req,
                    &target,
                    "PUT",
                    key,
                    "",
                    &[],
                    access_key_id,
                    secret,
                    Some(&bytes),
                )?;
                Ok(Performed {
                    code: i32::from(!reply.ok),
                    output: reply.body,
                    created: None,
                })
            }
            // `bind` refused a write with no key, and the framework checked the
            // operation id against the registry. Unreachable through the
            // pipeline, and answered rather than panicked.
            (id, _) => Err(ProviderError::Failed(format!(
                "'{id}' is not an operation this provider performs"
            ))),
        }
    }
}

/// What one signed call produced.
struct Reply {
    ok: bool,
    status: u16,
    body: String,
}

impl S3Provider {
    /// Sign one request and spend the credential inside a `curl` the broker
    /// owns.
    ///
    /// The signature goes in the configuration on curl's stdin, beside the URL
    /// — never on an argument list, where `/proc` would show it to anybody on
    /// the machine. The secret access key itself never leaves this function.
    #[allow(clippy::too_many_arguments)]
    fn call(
        &self,
        req: &Bind<'_>,
        target: &Target,
        method: &'static str,
        key: &str,
        query_for_signing: &str,
        query_pairs: &[(&str, String)],
        access_key_id: &str,
        secret: &str,
        body: Option<&[u8]>,
    ) -> Result<Reply, ProviderError> {
        // A listing addresses the bucket itself, so the trailing slash the
        // format leaves is removed — and only then, rather than by trimming
        // every path, because a trailing slash in an object KEY is part of the
        // key and S3 will store one.
        let path = if key.is_empty() {
            sigv4::encode_path(&format!("/{}", target.bucket))
        } else {
            sigv4::encode_path(&format!("/{}/{}", target.bucket, key))
        };

        let canonical_query = if query_for_signing.is_empty() {
            canonical_query(query_pairs)
        } else {
            query_for_signing.to_string()
        };

        let payload_sha256 = match body {
            Some(bytes) => sigv4::sha256_hex(bytes),
            None => sigv4::EMPTY_PAYLOAD_SHA256.to_string(),
        };
        let host = host_header(req.service);
        let amz_date = sigv4::amz_date(now_secs());
        let signed = sigv4::Request {
            method,
            host: &host,
            canonical_uri: &path,
            canonical_query: &canonical_query,
            payload_sha256: &payload_sha256,
            amz_date: &amz_date,
            region: &target.region,
        };
        let authorization = signed.authorization(access_key_id, secret);

        let url = if canonical_query.is_empty() {
            format!("{}{path}", origin(req.service))
        } else {
            format!("{}{path}?{canonical_query}", origin(req.service))
        };

        // Every value that reaches the configuration is checked, not escaped.
        // A newline in any of them would be a second configuration line, and
        // this build composed all four, so one that cannot be sent is a bug
        // here rather than something to sanitise away.
        for (what, value) in [
            ("the url", url.as_str()),
            ("the authorization", authorization.as_str()),
            ("the date", amz_date.as_str()),
            ("the payload digest", payload_sha256.as_str()),
        ] {
            if !printable(value) {
                return Err(ProviderError::Failed(format!(
                    "{what} this build composed is not something it will put in \
                     a curl configuration"
                )));
            }
        }

        let mut config = String::new();
        config.push_str(&format!("url = {}\n", quoted(&url)));
        config.push_str(&format!("request = {}\n", quoted(method)));
        // The scheme the credential was stored for and nothing else, so a
        // redirect cannot downgrade this to http.
        config.push_str(&format!(
            "proto = {}\n",
            quoted(&format!("={}", req.service.scheme))
        ));
        config.push_str(&format!(
            "header = {}\n",
            quoted(&format!("Authorization: {authorization}"))
        ));
        config.push_str(&format!(
            "header = {}\n",
            quoted(&format!("x-amz-date: {amz_date}"))
        ));
        config.push_str(&format!(
            "header = {}\n",
            quoted(&format!("x-amz-content-sha256: {payload_sha256}"))
        ));
        // curl waits for a 100-continue that a hand-written double never sends.
        config.push_str("header = \"Expect:\"\n");
        config.push_str(&format!(
            "header = {}\n",
            quoted(&format!(
                "User-Agent: apex-secretd/{}",
                env!("CARGO_PKG_VERSION")
            ))
        ));

        // Held to the end of the function so the file outlives the child.
        let scratch;
        if let Some(bytes) = body {
            scratch = broker::Scratch::new(req.owner).map_err(ProviderError::Failed)?;
            let path = scratch
                .write("body", bytes, req.owner)
                .map_err(ProviderError::Failed)?;
            config.push_str("header = \"Content-Type: application/octet-stream\"\n");
            config.push_str(&format!(
                "data-binary = {}\n",
                quoted(&format!("@{}", path.display()))
            ));
        }

        config.push_str(&format!("max-time = {TIMEOUT_SECS}\n"));
        config.push_str(&format!("connect-timeout = {CONNECT_TIMEOUT_SECS}\n"));
        // No `location`: a redirect is a server choosing where this credential
        // goes next, and a SigV4 signature is bound to the host it was made for.
        config.push_str("silent\n");
        config.push_str("show-error\n");
        config.push_str("write-out = \"\\n%{http_code}\"\n");

        let out = broker::run_curl(&config, req.owner).map_err(ProviderError::Failed)?;
        let stdout = out.stdout.as_str();
        let (body, status) = match stdout.rsplit_once('\n') {
            Some((body, tail)) => (body.to_string(), tail.trim().parse::<u16>().ok()),
            None => (String::new(), stdout.trim().parse::<u16>().ok()),
        };
        let Some(status) = status else {
            return Err(ProviderError::Failed(format!(
                "curl produced no HTTP status, so nothing is known about \
                 whether this reached {}: {}",
                req.service.host,
                one_line(out.stderr.as_str())
            )));
        };
        Ok(Reply {
            ok: (200..300).contains(&status),
            status,
            body,
        })
    }

    /// `ListObjectsV2`, following continuation tokens.
    fn list(
        &self,
        req: &Bind<'_>,
        target: &Target,
        access_key_id: &str,
        secret: &str,
    ) -> Result<Performed, ProviderError> {
        let mut keys: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        for _ in 0..MAX_LIST_PAGES {
            let mut pairs: Vec<(&str, String)> = vec![("list-type", "2".to_string())];
            if let Some(token) = &token {
                pairs.push(("continuation-token", token.clone()));
            }
            let reply = self.call(
                req, target, "GET", "", "", &pairs, access_key_id, secret, None,
            )?;
            if !reply.ok {
                // The status IS available here, unlike on the R2 path, so the
                // answer says which refusal it was rather than collapsing them.
                return Ok(Performed {
                    code: 1,
                    output: format!("HTTP {}: {}", reply.status, one_line(&reply.body)),
                    created: None,
                });
            }
            keys.extend(parse_keys(&reply.body));
            match next_token(&reply.body) {
                Some(next) => token = Some(next),
                None => {
                    return Ok(Performed {
                        code: 0,
                        output: render_listing(&keys),
                        created: None,
                    })
                }
            }
        }
        // Still truncated after the cap. A short list here would be a backup
        // history with snapshots missing from it, reported as a healthy one.
        Err(ProviderError::Failed(format!(
            "bucket '{}' holds more than {} objects, which is more than one \
             listing will follow. {} keys were read and are NOT being \
             returned, because a listing that is quietly short is worse than \
             one that fails",
            target.bucket,
            MAX_LIST_PAGES * 1000,
            keys.len()
        )))
    }
}

/// `name=value&…`, encoded and sorted by name, as SigV4 requires.
fn canonical_query(pairs: &[(&str, String)]) -> String {
    let mut encoded: Vec<(String, String)> = pairs
        .iter()
        .map(|(name, value)| {
            (
                sigv4::encode_query_component(name),
                sigv4::encode_query_component(value),
            )
        })
        .collect();
    // By the ENCODED name, which is what AWS specifies — sorting before
    // encoding gives a different order for names that encode differently.
    encoded.sort();
    encoded
        .iter()
        .map(|(n, v)| format!("{n}={v}"))
        .collect::<Vec<_>>()
        .join("&")
}

/// Every `<Key>…</Key>` in a `ListObjectsV2` reply.
///
/// A hand-written scan and not an XML parser, for the same reason the R2 path
/// reads one field out of a JSON envelope: what is wanted is one repeated
/// element in a document this build asked for, and a dependency that parses
/// arbitrary XML is a larger surface than the question.
///
/// It is deliberately literal: `<Key>` with no attributes is the only form
/// `ListObjectsV2` emits for this element, and anything else is left alone
/// rather than guessed at.
fn parse_keys(xml: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find("<Key>") {
        let after = &rest[start + "<Key>".len()..];
        let Some(end) = after.find("</Key>") else {
            break;
        };
        out.push(unescape_xml(&after[..end]));
        rest = &after[end..];
    }
    out
}

/// The continuation token, when the reply says it is truncated.
///
/// **Both halves, and both matter.** A reply that is not truncated has no more
/// pages whatever else it carries; a reply that IS truncated and carries no
/// token is a reply this build cannot follow, and it returns `None` so the
/// caller stops — which ends the listing early. That is why the truncation
/// refusal above counts pages rather than trusting this to be the only way out.
fn next_token(xml: &str) -> Option<String> {
    if !element(xml, "IsTruncated").is_some_and(|v| v == "true") {
        return None;
    }
    element(xml, "NextContinuationToken")
}

fn element(xml: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = xml.find(&open)?;
    let after = &xml[start + open.len()..];
    let end = after.find(&close)?;
    Some(unescape_xml(&after[..end]))
}

/// The five entities XML defines. S3 escapes a key's `&`, `<` and `>`.
fn unescape_xml(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        // Last, so that `&amp;lt;` comes back as `&lt;` and not as `<`.
        .replace("&amp;", "&")
}

/// The same envelope `cloudflare.r2.object.read` answers a listing with.
fn render_listing(keys: &[String]) -> String {
    let result: Vec<serde_json::Value> = keys
        .iter()
        .map(|key| serde_json::json!({ "key": key }))
        .collect();
    serde_json::json!({ "result": result }).to_string()
}

fn one_line(text: &str) -> String {
    let flat: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let trimmed = flat.trim();
    if trimmed.len() > 400 {
        format!("{}…", &trimmed[..400])
    } else {
        trimmed.to_string()
    }
}

/// Whether every byte is printable ASCII, so it cannot end a curl
/// configuration line early and start another.
fn printable(value: &str) -> bool {
    !value.is_empty() && value.len() <= 8192 && value.bytes().all(|b| (0x20..=0x7e).contains(&b))
}

/// A curl configuration value, quoted the way curl reads one.
fn quoted(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for c in value.chars() {
        if c == '\\' || c == '"' {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests;
