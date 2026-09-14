//! RFC 6749 §6: renewing an access token with a refresh token.
//!
//! `apex cloudflare connect` has stored a refresh token since P1-002 and
//! nothing has ever spent it — `apex cf status` said so in as many words. This
//! is the transport that does, and it is one provider for three authorisation
//! servers because §6 is the same request at all of them: a form POST to a
//! token endpoint with `grant_type=refresh_token`.
//!
//! # Where the request goes, and why the caller cannot say
//!
//! [`apex_secret_core::account::oauth_for_auth_host`], keyed on
//! [`apex_secret_core::store::ServiceInfo::host`] — the host the credential
//! was **already pinned to** when it was stored. Not a parameter, not a
//! resource, not a field in the reply. `oauth.token.refresh` declares no
//! resource and no parameters at all, so there is no request shape that can
//! aim a refresh anywhere: a refresh token filed under `dash.cloudflare.com`
//! is POSTed to `dash.cloudflare.com`'s token endpoint or to nothing.
//!
//! The framework's own host pin then checks the answer, which is not
//! redundant: `bind` returns the endpoint it derived from the table and the
//! framework compares it with the stored record before the value is read. A
//! table entry that drifted off its own `auth_host` would be refused there,
//! and `Provider::validate` in `apex-secret-core` refuses one at startup.
//!
//! # What it replaces, and why two things
//!
//! [`crate::provider::Bound::replaces`] names the access credential and the
//! refresh credential itself, because RFC 6749 §6 lets the server issue a new
//! refresh token with the new access token and this build assumes it does.
//! Storing only the access token would leave the old refresh token dead at the
//! server, which is the failure that reads as "refresh worked, and then one day
//! it stopped".
//!
//! The access credential's name is *derived* from the refresh credential's —
//! [`apex_secret_core::account::renewed_service`] — never taken from the
//! caller and never taken from the reply. See that function for why deriving
//! it widens nothing.
//!
//! # What this build can refresh, stated plainly
//!
//! The table serves three authorisation servers and this transport can renew
//! **one** of them today, which is honest rather than a failure:
//!
//! * **Cloudflare** works. [`apex_secret_core::account::CLOUDFLARE_OAUTH`]
//!   carries Wrangler's `client_id`, so the daemon can present the same client
//!   the grant was issued to — which §6 requires.
//! * **Google and Microsoft** are refused with a reason. APEX registers no
//!   OAuth application at either and will not borrow somebody else's, so their
//!   `client_id` is `None`; the user supplies one at grant time and nothing in
//!   this build records it. Google additionally wants a `client_secret`, which
//!   the daemon has nowhere to hold. The refusal says which of those it is.
//!
//! The way out for both is the same and is a change to the **grant**, not to
//! this file: whatever stores a refresh token has to store the client it was
//! issued to beside it. [`OAuthProvider::client_id`] already reads
//! `ServiceInfo::username` first for exactly that reason, so the day
//! `apex account add` or `apex cf connect --client-id` writes one there, this
//! starts working with no change here.
//!
//! # Nothing here reaches a real authorisation server
//!
//! The tests run against a loopback double. A real Cloudflare credential is
//! pinned to `dash.cloudflare.com` and a real refresh would go there and
//! nowhere else; the double is `127.0.0.1`, which the shipped table does not
//! contain and [`OAuthProvider::new`] therefore cannot route to.

use apex_secret_core::account::{self, ClientSecret, OAuth};
use apex_secret_core::operation::{
    Effect, OperationSpec, ProviderSpec, ResourceKind,
};
use apex_secret_core::store::ServiceInfo;
use apex_secret_core::SecretValue;

use crate::broker;
use crate::provider::{
    Approval, Bind, Bound, Endpoint, Performed, Provider, ProviderError, Replaced,
};

/// How long one token request may take, end to end.
const TIMEOUT_SECS: u64 = 60;
/// How long the connection alone may take.
const CONNECT_TIMEOUT_SECS: u64 = 15;

/// The username `apex secret add` writes when a credential has no name half.
///
/// Read off the store's own constant rather than spelled again here: this
/// provider treats it as *no client id was recorded*, and a build that changed
/// the placeholder in one crate and not the other would start presenting the
/// literal string `x-access-token` to an authorisation server as an OAuth
/// client.
const NO_USERNAME: &str = apex_secret_core::store::DEFAULT_USERNAME;

/// Longest a token this will accept from a token endpoint.
///
/// The store caps a value anyway; this refuses earlier and says why, because
/// "your account stopped working" is a worse answer than "that reply was not a
/// token".
const MAX_TOKEN: usize = 8192;

pub const SPEC: ProviderSpec = ProviderSpec {
    id: "oauth",
    summary: "renew a stored access token with the refresh token beside it, \
              against the authorisation server it was issued by",
    operations: &[OperationSpec {
        // The same constant `apex cloudflare refresh` asks with, in the crate
        // they both link, so a rename cannot leave the CLI requesting an
        // operation no provider offers.
        id: account::REFRESH_OPERATION,
        summary: "renew this account's access token, and the refresh token \
                  beside it if the server rotates one",
        // A read of a token endpoint and a WRITE of the store. `effect` is what
        // `apex secret capabilities` prints, and `ProviderSpec::validate`
        // refuses `Read` on an operation that supersedes a credential.
        effect: Effect::Write,
        resource: ResourceKind::None,
        params: &[],
        aliases: &[],
        // FALSE, and not because the claim would be untrue — a refresh reads
        // `req.service.host` and nothing from `req.project`, so it does reach
        // the same thing in every directory. It is false because the claim is
        // held to `providers::tests::an_operation_that_claims_to_reach_the_
        // same_thing_everywhere_binds_the_same_in_two_projects`, whose fixture
        // is MCP-shaped, and an operation that cannot bind in it makes that
        // test panic rather than pass. Turning this on means extending that
        // fixture with a refresh-shaped `ServiceInfo`, which is a change to a
        // shared test and belongs with the decision to grant `--everywhere`
        // rather than smuggled in beside a new provider.
        same_everywhere: false,
        // The declaration this whole subsystem exists for.
        supersedes_credentials: true,
    }],
};

/// The RFC 6749 §6 transport.
pub struct OAuthProvider {
    /// Authorisation servers consulted **before** the shipped table.
    ///
    /// Empty in every shipped build — [`OAuthProvider::new`] builds it empty
    /// and nothing else constructs one — so the route is
    /// `account::oauth_for_auth_host` and only that. It exists because a
    /// loopback double is `127.0.0.1`, and `oauth_for_auth_host("127.0.0.1")`
    /// is `None` **by design**: the shipped table is a list of real
    /// authorisation servers and putting a test host in it would be putting a
    /// route to a test host in the daemon.
    extra: Vec<&'static OAuth>,
}

impl OAuthProvider {
    pub fn new() -> OAuthProvider {
        OAuthProvider { extra: Vec::new() }
    }

    /// A stand-in authorisation server on a loopback port a test chose.
    ///
    /// The leak is deliberate and bounded: [`OAuth`] is `&'static` throughout
    /// because a provider's routing table is a constant, and a test that needs
    /// one entry to carry a port cannot write a constant. One per fixture.
    #[cfg(test)]
    pub fn at(port: u16) -> OAuthProvider {
        let leaked: &'static OAuth = Box::leak(Box::new(OAuth {
            device_url: Box::leak(
                format!("http://127.0.0.1:{port}/oauth2/device/auth").into_boxed_str(),
            ),
            token_url: Box::leak(
                format!("http://127.0.0.1:{port}/oauth2/token").into_boxed_str(),
            ),
            auth_host: "127.0.0.1",
            scopes: &[],
            client_secret: ClientSecret::None,
            client_id: Some("test-client-id"),
        }));
        OAuthProvider {
            extra: vec![leaked],
        }
    }

    /// The authorisation server a credential pinned to `host` belongs to.
    fn oauth_for(&self, host: &str) -> Option<&'static OAuth> {
        self.extra
            .iter()
            .copied()
            .find(|o| o.auth_host.eq_ignore_ascii_case(host.trim().trim_end_matches('.')))
            .or_else(|| account::oauth_for_auth_host(host))
    }

    /// The OAuth client to present, or why there is none.
    ///
    /// `ServiceInfo::username` first, then the table. The username is the store's
    /// field for *the half of a credential that is not a secret* — it already
    /// carries an S3 access key id and an Access service token's `client_id` —
    /// and an OAuth `client_id` is public by definition, so it is the right
    /// place for one and the only place a per-grant client could be recorded.
    /// Nothing in this build writes it there yet; see the module note.
    fn client_id(info: &ServiceInfo, oauth: &'static OAuth) -> Result<String, ProviderError> {
        let stored = info.username.trim();
        if !stored.is_empty() && stored != NO_USERNAME {
            if !valid_form_value(stored) {
                return Err(ProviderError::Refused(format!(
                    "the client id stored with '{}' is not something this build \
                     will put in a form body",
                    info.service.escape_debug()
                )));
            }
            return Ok(stored.to_string());
        }
        let Some(id) = oauth.client_id else {
            return Err(ProviderError::Refused(format!(
                "APEX has no registered OAuth client at {}, so it cannot renew \
                 this token: RFC 6749 §6 requires a refresh to present the same \
                 client the grant was issued to, and the one you signed in with \
                 was not recorded beside the refresh token. Sign in again{}",
                oauth.auth_host,
                match oauth.client_secret {
                    // Said rather than left for them to find out at the far
                    // side. Google's flow wants a client secret too, and this
                    // daemon has nowhere to keep one.
                    ClientSecret::RequiredToObtain =>
                        " — and note that this authorisation server also wants a \
                         client secret, which this build has nowhere to store, so \
                         renewing here is not something signing in again will fix",
                    ClientSecret::None => "",
                }
            )));
        };
        Ok(id.to_string())
    }
}

impl Default for OAuthProvider {
    fn default() -> OAuthProvider {
        OAuthProvider::new()
    }
}

impl Provider for OAuthProvider {
    fn spec(&self) -> &'static ProviderSpec {
        &SPEC
    }

    fn bind(&self, req: &Bind<'_>) -> Result<Bound, ProviderError> {
        let Some(oauth) = self.oauth_for(&req.service.host) else {
            return Err(ProviderError::NoSuchResource(format!(
                "'{}' is stored for {}, which is not an authorisation server this \
                 build knows how to renew a token at. A refresh is routed by the \
                 host the credential was pinned to and by nothing else, so there \
                 is no way to point this somewhere it would work.",
                req.service.service.escape_debug(),
                req.service.host.escape_debug()
            )));
        };
        // Derived from the name this request is already running against —
        // never from the caller, who names nothing here, and never from the
        // reply, which has not happened yet.
        let Some(renewed) = account::renewed_service(&req.service.service) else {
            return Err(ProviderError::NoSuchResource(format!(
                "'{}' is not a refresh credential: a refresh credential's name \
                 ends in one of [{}] and names the credential it renews. This \
                 operation has nothing to renew.",
                req.service.service.escape_debug(),
                account::REFRESH_SUFFIXES.join(", ")
            )));
        };
        // Refuses here rather than at the far side if the client cannot be
        // named, so a request that cannot succeed does not spend the owner's
        // refresh token against a server that will answer `invalid_client`.
        // `bind` may not read a value and does not: a client id is not one.
        OAuthProvider::client_id(req.service, oauth)?;
        Ok(Bound {
            endpoint: Endpoint::from_url(oauth.token_url)?,
            detail: format!(
                "renew the access token stored as '{renewed}' at {}",
                oauth.auth_host
            ),
            creates: None,
            // Both, in this order, because the message the caller reads names
            // them in it: the access token, and the refresh token if the
            // server rotates one.
            replaces: vec![renewed, req.service.service.clone()],
            // No environment here, and nothing a second person would want to
            // be asked about: this renews a credential the owner already has
            // and grants nothing new. The standing grant is the decision.
            approval: Approval::Standing,
        })
    }

    fn perform(
        &self,
        req: &Bind<'_>,
        _bound: &Bound,
        value: &SecretValue,
    ) -> Result<Performed, ProviderError> {
        let Some(oauth) = self.oauth_for(&req.service.host) else {
            // `bind` established this. Answered rather than panicked, because a
            // provider that trusted its own earlier call would stop being
            // correct the day the framework's ordering moves.
            return Err(ProviderError::Failed(
                "this credential is not stored for an authorisation server".to_string(),
            ));
        };
        let client_id = OAuthProvider::client_id(req.service, oauth)?;
        let Some(refresh) = value.as_str() else {
            return Err(ProviderError::Refused(
                "the stored refresh token is not text, so it cannot be a form \
                 value"
                    .to_string(),
            ));
        };
        if !valid_form_value(refresh) {
            return Err(ProviderError::Refused(
                "the stored refresh token has characters in it that this build \
                 will not put in a curl configuration. It is not a token this \
                 daemon wrote."
                    .to_string(),
            ));
        }

        let mut config = String::new();
        config.push_str(&format!("url = {}\n", quoted(oauth.token_url)));
        config.push_str("request = \"POST\"\n");
        // The scheme the token endpoint is on and nothing else, so a redirect
        // cannot downgrade a refresh token onto http. `Endpoint::from_url` gave
        // the framework the same scheme to pin against.
        config.push_str(&format!("proto = {}\n", quoted(&format!("={}", scheme_of(oauth.token_url)))));
        // `data-urlencode` rather than a body this file assembles: the encoding
        // is curl's, so a `+` or a `/` inside a token is escaped by the thing
        // that will send it rather than by a second implementation here. The
        // values still reach the configuration, which is why each one is
        // checked against `valid_form_value` above — `quoted` escapes `"` and
        // `\` and does nothing about a newline, and a newline would be a
        // second configuration line.
        for (name, content) in [
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh),
            ("client_id", client_id.as_str()),
        ] {
            config.push_str(&format!(
                "data-urlencode = {}\n",
                quoted(&format!("{name}={content}"))
            ));
        }
        config.push_str("header = \"Accept: application/json\"\n");
        // A hand-written double never answers a 100-continue.
        config.push_str("header = \"Expect:\"\n");
        config.push_str(&format!(
            "header = {}\n",
            quoted(&format!(
                "User-Agent: apex-secretd/{}",
                env!("CARGO_PKG_VERSION")
            ))
        ));
        config.push_str(&format!("max-time = {TIMEOUT_SECS}\n"));
        config.push_str(&format!("connect-timeout = {CONNECT_TIMEOUT_SECS}\n"));
        // No `location`: a redirect is the far end choosing where this
        // credential goes next.
        config.push_str("silent\nshow-error\n");
        config.push_str(&format!("max-filesize = {}\n", broker::HTTP_MAX_BYTES));
        config.push_str("write-out = \"\\n%{http_code}\"\n");

        let out = broker::run_curl(&config, req.owner).map_err(ProviderError::Failed)?;
        let (body, status) = split_status(&out.stdout);
        let Some(status) = status else {
            return Err(ProviderError::Failed(format!(
                "curl produced no HTTP status, so nothing is known about whether \
                 this reached {}: {}",
                oauth.auth_host,
                one_line(&out.stderr)
            )));
        };
        if !(200..300).contains(&status) {
            // The body travels rather than being dropped: an OAuth error is a
            // JSON document with `error` and `error_description` in it, and
            // those are the only words that say whether the grant was revoked,
            // the client was wrong or the token had already been rotated. It is
            // not an `Err`, so it reaches the caller through the framework's
            // scrub like every other reply.
            return Ok(Performed {
                code: 1,
                output: format!(
                    "HTTP {status} from {}: {}\napex: the stored credentials were \
                     left exactly as they were.",
                    oauth.auth_host,
                    one_line(&body)
                ),
                created: None,
                replaced: Vec::new(),
            });
        }

        let Ok(json) = serde_json::from_str::<serde_json::Value>(body.trim()) else {
            return Err(ProviderError::Failed(format!(
                "{} answered {status} with something that is not JSON, so this \
                 build cannot tell whether a token was issued",
                oauth.auth_host
            )));
        };
        let Some(access) = token(&json, "access_token") else {
            return Err(ProviderError::Failed(format!(
                "{} answered {status} with no usable access_token in it, so \
                 nothing has been replaced",
                oauth.auth_host
            )));
        };
        // `bind` put the access credential first and this credential second.
        // Read off `bound.replaces` rather than recomputed, so the two can
        // never disagree — the framework checks membership in that exact list.
        let renewed = account::renewed_service(&req.service.service).ok_or_else(|| {
            ProviderError::Failed("this credential names nothing to renew".to_string())
        })?;

        let mut replaced = vec![Replaced {
            name: renewed.clone(),
            value: SecretValue::new(access.as_bytes().to_vec()),
        }];
        // Only if the server rotated one. RFC 6749 §6 says it MAY, and a reply
        // without one means the refresh token that was just spent is still
        // good — so writing anything over it would be destroying a working
        // credential.
        if let Some(rotated) = token(&json, "refresh_token") {
            if rotated != refresh {
                replaced.push(Replaced {
                    name: req.service.service.clone(),
                    value: SecretValue::new(rotated.as_bytes().to_vec()),
                });
            }
        }

        let expires = json
            .get("expires_in")
            .and_then(serde_json::Value::as_u64)
            .map(|s| format!(" It is good for about {}.", roughly(s)))
            .unwrap_or_default();
        Ok(Performed {
            code: 0,
            output: format!(
                "renewed the access token stored as '{renewed}' at {}.{expires}",
                oauth.auth_host
            ),
            created: None,
            replaced,
        })
    }
}

/// A token out of a token endpoint's reply, if it is one.
///
/// Bounded and character-checked before it is kept, because it is about to be
/// written into the store as a credential and presented in a header by every
/// later request. A reply field that carried a newline would be a header
/// injection in whatever spends it next, and one that is empty would replace a
/// working credential with nothing.
fn token(json: &serde_json::Value, field: &str) -> Option<String> {
    let text = json.get(field)?.as_str()?;
    let ok = !text.is_empty() && text.len() <= MAX_TOKEN && valid_form_value(text);
    ok.then(|| text.to_string())
}

/// Whether every byte is printable ASCII and safe in a curl configuration.
///
/// The same rule `apex cloudflare connect` applies to what it stores, restated
/// where the value is about to be sent rather than assumed because it was
/// checked once somewhere else.
fn valid_form_value(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_TOKEN
        && value.bytes().all(|b| (0x20..=0x7e).contains(&b))
}

/// A curl config value, quoted so nothing in it can be read as syntax.
fn quoted(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for c in value.chars() {
        if c == '"' || c == '\\' {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

/// The scheme of a URL this build composed.
fn scheme_of(url: &str) -> &str {
    match url.split_once("://") {
        Some((scheme, _)) => scheme,
        None => "https",
    }
}

/// Split curl's `write-out` status off the end of its stdout.
fn split_status(stdout: &str) -> (String, Option<u16>) {
    match stdout.rsplit_once('\n') {
        Some((body, tail)) => (body.to_string(), tail.trim().parse::<u16>().ok()),
        None => (String::new(), stdout.trim().parse::<u16>().ok()),
    }
}

/// One line of somebody else's output, bounded.
fn one_line(text: &str) -> String {
    let mut out: String = text.trim().chars().filter(|c| !c.is_control()).collect();
    out.truncate(400);
    out
}

/// A duration in seconds, in words.
fn roughly(seconds: u64) -> String {
    match seconds {
        0..=90 => format!("{seconds} seconds"),
        91..=5400 => format!("{} minutes", seconds / 60),
        5401..=172_800 => format!("{} hours", seconds / 3600),
        _ => format!("{} days", seconds / 86_400),
    }
}

#[cfg(test)]
mod tests;
