//! Microsoft Graph: read one OneDrive file, with a token the device grant
//! obtained (roadmap P2-017).
//!
//! This is the transport behind the `microsoft` entry in
//! [`apex_secret_core::account::PROVIDERS`], and it is the last of the five
//! providers that criterion to have one. Until it existed, `apex account add
//! microsoft.<name> --client-id <id>` could sign a person in, store an access
//! token, file a refresh token beside it and renew that token forever — and
//! the token could be spent on **nothing**, because no operation in
//! `default_registry()` began with `msgraph.`. `apex account scopes microsoft`
//! said so in as many words.
//!
//! It is [`crate::providers::gdrive`]'s shape, deliberately, down to the two
//! pins and the per-file audit line. Three things differ, and the second is
//! the whole reason this file is longer than that one.
//!
//! # 1. The endpoint, and the resource that goes in it
//!
//! `GET {stored endpoint}/drive/items/{item-id}/content`, where the stored
//! endpoint is `https://graph.microsoft.com/v1.0/me` — host and path both
//! read off the account table rather than spelled here.
//!
//! The resource is a **driveItem id**, so [`ResourceKind::Name`] and not
//! `Path`: it is an opaque handle, not a route through a directory. That
//! choice carries a narrowing which is real and is written down rather than
//! discovered:
//!
//! > **A consumer OneDrive item id containing `!` is refused.** Microsoft's
//! > own example response on the `driveItem: content` page is
//! > `{"id": "12319191!11919", …}` — personal-account ids are
//! > `{driveId}!{n}`, and
//! > [`apex_secret_core::operation::valid_name`] takes ASCII letters, digits,
//! > `_`, `.` and `-` and nothing else. Work and school ids (`01BYE5RZ…`) are
//! > unaffected. The vocabulary is shared by every operation in this build and
//! > is not widened for one provider; the refusal names the rule, which is a
//! > better outcome than a `!` reaching a URL path unencoded. Lifting it needs
//! > a resource kind that percent-encodes, which is a decision about the
//! > vocabulary and not about Graph.
//!
//! Every byte `valid_name` does accept is unreserved in a URL, so the id goes
//! into the path with no encoding and there is nothing to get wrong; `?`
//! cannot appear, so this request carries no query at all.
//!
//! # 2. Graph does not answer the file. It answers where the file is.
//!
//! `/content` replies **`302 Found`** with a `Location:` header naming a
//! *pre-authenticated* download URL on a different host — Microsoft's own
//! example is `https://b0mpua-by3301.files.1drv.com/y23vmag…` — and the same
//! page says, in those words, *"You don't need to include an `Authorization`
//! header when you access the download URL."*
//!
//! So this transport has to decide what to do about `location`, and the
//! decision is the one property worth reading this file for:
//!
//! * **curl is never told to follow anything.** No `location`, on either hop.
//!   Whether a redirect carries the `Authorization` header across a host
//!   boundary is a curl *policy* — it has been a CVE once already — and a
//!   policy is a thing that varies by version. This build does not depend on
//!   it.
//! * **The download is a second `run_curl` whose configuration contains no
//!   credential at all.** Not a stripped header: an absent one.
//!   [`download_config`] takes no token and has nowhere to put one, and
//!   [`tests::the_download_configuration_has_nowhere_to_put_a_credential`]
//!   holds it to that. The property is structural rather than checked.
//! * **One hop, and one only.** A 3xx from the download host comes back as a
//!   status. There is no chain to walk.
//! * **The download hop is scheme-pinned to the credential's own scheme**,
//!   in Rust before curl sees it and again with `proto = "=<scheme>"`, so a
//!   `Location:` of `file:///etc/shadow` or `ftp://…` is refused with a
//!   message rather than fetched.
//! * **The pre-authenticated URL is itself a bearer capability, and it never
//!   comes back.** Anyone holding that URL can read the file until it expires,
//!   so it is in no reply, no error message and no audit line: the failure
//!   paths name the download target's **scheme and host only**, and
//!   [`without_the_download_url`] takes both the whole URL and its query out
//!   of the far side's body before either outcome sees it — the success path
//!   included, so the property is *"this URL never comes back"* rather than
//!   *"it does not come back from the paths somebody remembered"* — and
//!   [`download_outcome`], which composes every word this hop returns, is
//!   **not given the URL at all**. curl's own stderr is not carried on this
//!   hop for the same reason: curl quotes the URL it was handed, and a
//!   loopback double cannot make it do so, so "carry it but scrub it" would be
//!   an unprovable claim. The exit code travels; curl's words do not.
//!
//! **What the framework's pin does and does not cover.** `Bound::endpoint` is
//! the Graph URL, and `apex-secretd` pins that against the stored credential
//! before the value is read. It covers hop one. Nothing in `service.rs` sees
//! hop two, and that is not a gap left open — it is why hop two carries no
//! credential, cannot change scheme, cannot chain, is bounded by
//! [`crate::broker::HTTP_MAX_BYTES`], and returns bytes and nothing else.
//!
//! # 3. Where the token goes, and the two pins that decide it
//!
//! Unchanged from `gdrive`. The URL is the **stored record's own** scheme,
//! host, port and path with `/drive/items/<id>/content` under it, and the
//! framework pins what [`MsgraphProvider::bind`] returns against that record.
//! That pin alone checks the provider agrees with the store; it cannot check
//! that the store is Microsoft. So `bind` **also** refuses a credential pinned
//! to any host but the one [`apex_secret_core::account::PROVIDERS`] fixes
//! `microsoft` to, read off that table rather than spelled again here.
//! `Host::Fixed` stops `apex account add` storing another, but `apex secret
//! add` is a lower-level command with no account vocabulary, and this is what
//! stops an `msgraph` grant on such a record putting a Microsoft access token
//! on somebody else's server.
//!
//! # Nothing here reaches Microsoft
//!
//! There is no Microsoft account on any machine this repository is built on.
//! The tests run against loopback doubles that parse the requests that
//! actually arrived, and a real credential is pinned to `graph.microsoft.com`,
//! which [`MsgraphProvider::new`] is the only constructor that accepts — the
//! loopback host reaches the table through a `#[cfg(test)]` constructor and
//! through nothing that ships.
//!
//! `printable`, `quoted`, `one_line` and `split_status` are a copy of the ones
//! in `gdrive`, `s3` and `oauth`, which are already copies of each other. Four
//! copies of eight lines is the house style here and a fifth is not the round
//! to change it in; the shared thing worth extracting is a curl-config
//! builder, not four predicates.

use apex_secret_core::account::{self, Host};
use apex_secret_core::capability::http_endpoint;
use apex_secret_core::operation::{Effect, OperationSpec, ProviderSpec, ResourceKind};
use apex_secret_core::store::ServiceInfo;
use apex_secret_core::SecretValue;

use crate::broker;
use crate::provider::{Approval, Bind, Bound, Endpoint, Performed, Provider, ProviderError};

/// The account provider whose transport this is.
///
/// Its row in [`apex_secret_core::account::PROVIDERS`] is where the API host
/// comes from, so the two cannot drift.
const ACCOUNT_PROVIDER: &str = "microsoft";

/// How long one call may take, end to end. Each hop gets this, not half of it:
/// they are separate transfers and a file download is the slow one.
const TIMEOUT_SECS: u64 = 120;
const CONNECT_TIMEOUT_SECS: u64 = 20;

pub const SPEC: ProviderSpec = ProviderSpec {
    id: "msgraph",
    summary: "read a file from a Microsoft 365 or OneDrive account, by its item id",
    operations: &[OperationSpec {
        id: "msgraph.file.read",
        summary: "read a file from this Microsoft account, by its OneDrive item id",
        effect: Effect::Read,
        resource: ResourceKind::Name,
        params: &[],
        aliases: &[],
        // FALSE, for `gdrive`'s reason rather than because the claim would be
        // untrue: `bind` reads `req.service` and never `req.project`, so it
        // does reach the same thing in every directory. The claim is held to
        // `providers::tests::an_operation_that_claims_to_reach_the_same_thing_
        // everywhere_binds_the_same_in_two_projects`, whose fixture binds with
        // an empty resource — and an operation that names something cannot
        // bind there, which makes that test panic rather than pass.
        same_everywhere: false,
        supersedes_credentials: false,
    }],
};

/// The Graph transport.
pub struct MsgraphProvider {
    /// API hosts accepted besides Microsoft's own.
    ///
    /// Empty in every shipped build — [`MsgraphProvider::new`] builds it empty
    /// and nothing else that compiles into the daemon constructs one. It
    /// exists because a loopback double is `127.0.0.1`, and the account table
    /// fixes `microsoft` to `graph.microsoft.com` **by design**: putting a
    /// test host in that table would be putting a route to a test host in the
    /// daemon.
    extra_hosts: Vec<String>,
}

impl MsgraphProvider {
    pub fn new() -> MsgraphProvider {
        MsgraphProvider {
            extra_hosts: Vec::new(),
        }
    }

    /// A stand-in Graph API on a loopback host, for tests only.
    #[cfg(test)]
    pub fn at(host: &str) -> MsgraphProvider {
        MsgraphProvider {
            extra_hosts: vec![host.to_string()],
        }
    }

    /// Microsoft Graph's API host, read off the account table.
    ///
    /// Not a literal here. The table is what `apex account add
    /// microsoft.<name>` pins a credential to, and a build where the two
    /// disagreed would refuse every account it had itself just stored.
    fn api_host() -> Result<&'static str, ProviderError> {
        match account::provider(ACCOUNT_PROVIDER).map(|p| p.host) {
            Some(Host::Fixed(host)) => Ok(host),
            // Unreachable while the table has a `microsoft` row with a fixed
            // host, which `account`'s own tests require. Answered rather than
            // panicked: a daemon does not abort because a table moved.
            _ => Err(ProviderError::Failed(format!(
                "this build has no fixed API host for '{ACCOUNT_PROVIDER}' \
                 accounts, so it cannot tell whether a credential is stored \
                 for Microsoft"
            ))),
        }
    }

    /// Whether a credential pinned to `host` is one this transport will spend.
    fn is_graph(&self, host: &str) -> Result<bool, ProviderError> {
        // A trailing dot is the DNS root and names the same host; comparing
        // without trimming it would refuse a credential that works.
        let host = host.trim().trim_end_matches('.');
        if self.extra_hosts.iter().any(|h| h.eq_ignore_ascii_case(host)) {
            return Ok(true);
        }
        Ok(MsgraphProvider::api_host()?.eq_ignore_ascii_case(host))
    }

    /// Hop two: fetch the pre-authenticated URL Graph named, with no
    /// credential.
    ///
    /// Separated from [`MsgraphProvider::perform`] so that "the download hop
    /// carries no token" is a property of a function a test can call, rather
    /// than a line somebody has to find in a longer one.
    fn download(&self, req: &Bind<'_>, url: &str) -> Result<Performed, ProviderError> {
        let config = download_config(url, &req.service.scheme)?;
        let out = broker::run_curl(&config, req.owner).map_err(|e| {
            // curl says the URL back in several of its own errors, and the URL
            // is a capability. Taken out before this becomes a message.
            ProviderError::Failed(without_the_download_url(&e, url))
        })?;
        let (body, status) = split_status(&out.stdout);
        // The body is the far side's and the commonest shape for a storage
        // error document is one that quotes the request back — and the request
        // is the capability. Taken out here, once, for both outcomes.
        let body = without_the_download_url(&body, url);
        // Scheme and host, never the URL. Everything after the authority is
        // the part that authorises.
        let target = match http_endpoint(url) {
            Some((scheme, host)) => format!("{scheme}://{host}"),
            None => "the download host".to_string(),
        };
        download_outcome(&target, &req.service.host, status, &body, out.code)
    }
}

/// What a finished download hop says.
///
/// **It is not given the URL.** A free function whose arguments are the
/// target's scheme-and-host, the API host, the status, the already-scrubbed
/// body and curl's exit code — so no message composed here *can* contain the
/// pre-authenticated link, which is a property of the signature rather than a
/// check somebody has to remember to apply. It is the same argument
/// `Replaced { name, value }` makes about a refresh not being able to repoint
/// a credential.
///
/// curl's own stderr is deliberately absent for the same reason: curl quotes
/// the URL it was handed in several of its messages, and there is no version
/// of "carry it but scrub it" that a test here can prove, because a loopback
/// double cannot make curl produce those messages. So the download hop reports
/// curl's exit code and nothing curl wrote.
fn download_outcome(
    target: &str,
    api_host: &str,
    status: Option<u16>,
    body: &str,
    curl_code: i32,
) -> Result<Performed, ProviderError> {
    let Some(status) = status else {
        return Err(ProviderError::Failed(format!(
            "curl produced no HTTP status for the download {api_host} redirected \
             to, so nothing is known about whether it reached {target} (curl \
             exited {curl_code}). curl's own message is not carried here: it \
             quotes the URL it was given, and that URL reads the file for anyone \
             who has it"
        )));
    };
    if !(200..300).contains(&status) {
        return Ok(Performed {
            code: 1,
            output: format!(
                "HTTP {status} from {target}, which is where {api_host} redirected \
                 this read. A pre-authenticated download link is valid for minutes, \
                 so the commonest cause is that too long passed between the two \
                 requests; ask again.{} {}",
                if (300..400).contains(&status) {
                    " This build follows exactly one redirect and that one is \
                     already spent, so a second is not followed."
                } else {
                    ""
                },
                // The far side's own reason, for `gdrive`'s argument: it is the
                // only thing that says whether the link expired, the file moved
                // or the storage is down.
                one_line(body)
            ),
            created: None,
            replaced: Vec::new(),
        });
    }
    Ok(Performed {
        code: 0,
        output: body.to_string(),
        created: None,
        replaced: Vec::new(),
    })
}

impl Default for MsgraphProvider {
    fn default() -> MsgraphProvider {
        MsgraphProvider::new()
    }
}

impl Provider for MsgraphProvider {
    fn spec(&self) -> &'static ProviderSpec {
        &SPEC
    }

    fn bind(&self, req: &Bind<'_>) -> Result<Bound, ProviderError> {
        if !self.is_graph(&req.service.host)? {
            return Err(ProviderError::Refused(format!(
                "'{}' is stored for {}, and this operation spends a Microsoft \
                 OAuth access token, which goes to {} and nowhere else. Store \
                 the account with `apex account add {ACCOUNT_PROVIDER}.<name> \
                 --client-id <id>`, which pins the right host for you.",
                req.service.service.escape_debug(),
                req.service.host.escape_debug(),
                MsgraphProvider::api_host()?
            )));
        }
        // Built here as well as in `perform`, so an id this provider could not
        // turn into a request is refused BEFORE the grant check spends
        // anything and before a credential is read.
        graph_url(req.service, req.resource)?;
        Ok(Bound {
            endpoint: Endpoint::from_url(&req.service.url())?,
            // The account and the item id, not the URL. The id is the caller's
            // own word and is what makes the line useful; the endpoint is the
            // same on every one of these lines and printing it would only
            // invite somebody to grep the trail for where a person's files
            // are. The download URL is in no audit line at all — see the
            // module note.
            detail: format!(
                "{} {} on {}",
                req.operation.id, req.resource, req.service.service
            ),
            // Reading a file issues no credential and supersedes none.
            creates: None,
            replaces: Vec::new(),
            // Per-file approval would be a prompt per file, which is how
            // people are taught to approve without reading. The grant is per
            // project, per account and per operation, and that is the decision
            // an owner makes here.
            approval: Approval::Standing,
        })
    }

    fn perform(
        &self,
        req: &Bind<'_>,
        _bound: &Bound,
        value: &SecretValue,
    ) -> Result<Performed, ProviderError> {
        // `bind` established this. Asked again rather than assumed, because a
        // provider that trusted its own earlier call would stop being correct
        // the day the framework's ordering moves.
        if !self.is_graph(&req.service.host)? {
            return Err(ProviderError::Failed(
                "this credential is not stored for Microsoft Graph's API host".to_string(),
            ));
        }
        if req.operation.id != "msgraph.file.read" {
            // The registry routed here on the id's first segment, so an id
            // this provider declares and does not implement is a registry
            // entry with no implementation — a refusal, never a default.
            return Err(ProviderError::Refused(format!(
                "{} is declared by this provider and not implemented by it",
                req.operation.id
            )));
        }
        let url = graph_url(req.service, req.resource)?;
        let Some(token) = value.as_str() else {
            return Err(ProviderError::Refused(
                "the stored Microsoft access token is not text, so it cannot be \
                 an Authorization header"
                    .to_string(),
            ));
        };
        // The store's own mapping from `ServiceInfo::auth` to a header, rather
        // than a second copy of it here: an account stored by `apex account
        // add microsoft.<name>` is `bearer`, and a build that spelled `Bearer`
        // again here would keep working while the two drifted.
        let authorization = req.service.header_value(token);
        let config = content_config(&url, &authorization, &req.service.scheme)?;

        let out = broker::run_curl(&config, req.owner).map_err(ProviderError::Failed)?;
        let (body, status, location) = split_status_and_redirect(&out.stdout);
        let Some(status) = status else {
            return Err(ProviderError::Failed(format!(
                "curl produced no HTTP status, so nothing is known about \
                 whether this reached {}: {}",
                req.service.host,
                one_line(&out.stderr)
            )));
        };
        if is_redirect(status) {
            // The documented answer: 302 with a pre-authenticated Location.
            let Some(location) = location else {
                return Err(ProviderError::Failed(format!(
                    "{} answered HTTP {status} with no Location, so there is \
                     nowhere to read the file from",
                    req.service.host
                )));
            };
            return self.download(req, &location);
        }
        if !(200..300).contains(&status) {
            // Not an `Err`: Graph's refusals are JSON documents with a reason
            // in them, and the reason is the only thing that tells a person
            // whether the file is not theirs, the id is wrong or the token has
            // expired. It travels back through the framework's scrub like
            // every other reply.
            return Ok(Performed {
                code: 1,
                output: format!(
                    "HTTP {status} from {}: {}{}",
                    req.service.host,
                    one_line(&body),
                    expiry_hint(status, &req.service.service)
                ),
                created: None,
                replaced: Vec::new(),
            });
        }
        // A 2xx straight from Graph. Not what `/content` documents — it
        // documents a 302 — but a body that arrived is the file, and refusing
        // it would be this build insisting the far side is wrong about its own
        // API.
        Ok(Performed {
            code: 0,
            output: body,
            created: None,
            replaced: Vec::new(),
        })
    }
}

/// `<stored endpoint>/drive/items/<id>/content`.
///
/// The scheme, host, port and path all come from the credential as it was
/// stored; the caller contributes the id and nothing else. Checked here as
/// well as by `OperationSpec::check`, because the function that builds the
/// string cannot see that somebody else checked — the same argument
/// [`crate::broker::webdav_url`] makes.
fn graph_url(service: &ServiceInfo, item_id: &str) -> Result<String, ProviderError> {
    if !apex_secret_core::operation::valid_name(item_id) {
        return Err(ProviderError::NoSuchResource(format!(
            "'{}' is not a OneDrive item id this build can address: letters, \
             digits, '_', '.' and '-', not starting with '-'. A consumer \
             OneDrive id of the form <driveId>!<n> contains a character the \
             shared resource vocabulary does not accept",
            item_id.escape_debug()
        )));
    }
    let base = service.url();
    let base = base.strip_suffix('/').unwrap_or(&base);
    Ok(format!("{base}/drive/items/{item_id}/content"))
}

/// Hop one's curl configuration: the Graph request, with the token on it.
///
/// A function so that the two configurations can be compared side by side in a
/// test — this one MUST carry an `Authorization` header and
/// [`download_config`] must have nowhere to put one.
fn content_config(
    url: &str,
    authorization: &str,
    scheme: &str,
) -> Result<String, ProviderError> {
    // Every value that reaches the configuration is CHECKED and not escaped.
    // `quoted` handles `"` and `\` and does nothing about a newline, and a
    // newline in any of these would be a second configuration line — which for
    // the header is a second header of the attacker's choosing, carrying this
    // token.
    for (what, text) in [("the url", url), ("the authorization", authorization)] {
        if !printable(text) {
            return Err(ProviderError::Failed(format!(
                "{what} this build composed is not something it will put in a \
                 curl configuration"
            )));
        }
    }
    let mut config = String::new();
    config.push_str(&format!("url = {}\n", quoted(url)));
    config.push_str("request = \"GET\"\n");
    // The scheme the credential was stored for and nothing else.
    config.push_str(&format!("proto = {}\n", quoted(&format!("={scheme}"))));
    config.push_str(&format!(
        "header = {}\n",
        quoted(&format!("Authorization: {authorization}"))
    ));
    config.push_str("header = \"Accept: */*\"\n");
    // curl waits for a 100-continue a hand-written double never sends.
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
    // **No `location`.** `/content` answers 302 and the Location is
    // pre-authenticated; whether curl would carry this header across the host
    // boundary is a curl policy and this build does not depend on one. The
    // redirect is read off `%{redirect_url}`, which curl fills in precisely
    // because it was not told to follow.
    config.push_str("silent\nshow-error\n");
    config.push_str(&format!("max-filesize = {}\n", broker::HTTP_MAX_BYTES));
    config.push_str("write-out = \"\\n%{http_code} %{redirect_url}\"\n");
    Ok(config)
}

/// Hop two's curl configuration: the pre-authenticated download, with nothing.
///
/// **This function takes no credential and has nowhere to put one.** That is
/// the design, not an omission — Microsoft's own page says the download URL
/// needs no `Authorization` header, and an absent header cannot be leaked by a
/// redirect policy the way a stripped one can.
///
/// `scheme` is the credential's own, and the redirect may not change it: a
/// `Location:` naming `file://`, `ftp://` or a plain-`http` downgrade of an
/// `https` account is refused here with a message, and refused again by curl's
/// own `proto` if this check were ever removed.
fn download_config(url: &str, scheme: &str) -> Result<String, ProviderError> {
    if !printable(url) {
        // A newline here would be a second configuration line, and the far
        // side chose this string.
        return Err(ProviderError::Refused(
            "the download location the far side named is not something this \
             build will put in a curl configuration"
                .to_string(),
        ));
    }
    let Some((redirect_scheme, host)) = http_endpoint(url) else {
        return Err(ProviderError::Refused(
            "the download location the far side named is not an http or https \
             URL, so this build will not fetch it"
                .to_string(),
        ));
    };
    if redirect_scheme != scheme {
        return Err(ProviderError::Refused(format!(
            "this read was redirected to a {redirect_scheme} URL on {host}, and \
             the credential is stored for {scheme}. A download may not change \
             scheme, so nothing was fetched"
        )));
    }
    let mut config = String::new();
    config.push_str(&format!("url = {}\n", quoted(url)));
    config.push_str("request = \"GET\"\n");
    config.push_str(&format!("proto = {}\n", quoted(&format!("={scheme}"))));
    config.push_str("header = \"Accept: */*\"\n");
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
    // Still no `location`. One hop is the whole budget: a 3xx here comes back
    // as a status.
    config.push_str("silent\nshow-error\n");
    config.push_str(&format!("max-filesize = {}\n", broker::HTTP_MAX_BYTES));
    config.push_str("write-out = \"\\n%{http_code}\"\n");
    Ok(config)
}

/// The statuses that mean "the thing you asked for is somewhere else".
///
/// The whole standard set rather than only the 302 Graph documents: a service
/// that moved to 307 would otherwise have its redirect returned to the caller
/// as a failed read, and every one of them is handled identically here —
/// one hop, no credential, no scheme change.
fn is_redirect(status: u16) -> bool {
    matches!(status, 301 | 302 | 303 | 307 | 308)
}

/// Take the pre-authenticated download URL out of anything that travels.
///
/// The URL **is** the capability: anyone holding it can read the file until it
/// expires. It is removed whole, and its query removed again on its own,
/// because a far side that echoes the request may echo only the part after the
/// `?` — which is the part that authorises.
fn without_the_download_url(text: &str, url: &str) -> String {
    let out = broker::scrub(text, url);
    match url.split_once('?') {
        Some((_, query)) => broker::scrub(&out, query),
        None => out,
    }
}

/// What to say after a status that means the token, rather than the file.
///
/// 401 is the one a person can act on without knowing anything about Graph,
/// and the action is a command this build actually has. Derived from the
/// service name through [`apex_secret_core::account::AccountRef::parse`], so a
/// credential that is not an account gets no suggestion rather than a wrong
/// one.
fn expiry_hint(status: u16, service: &str) -> String {
    if status != 401 {
        return String::new();
    }
    match account::AccountRef::parse(service) {
        // `<provider>.<name>` is the reference `apex account` takes, and it is
        // what `AccountRef` is made of — not the store's `account.` -prefixed
        // service name, which is not a command anybody can type.
        Some(account) => format!(
            "\napex: a Microsoft access token lasts about an hour. Renew it with \
             `apex account refresh {}.{}`.",
            account.provider.id, account.name
        ),
        None => String::new(),
    }
}

/// Whether every byte is printable ASCII, so nothing in it can be a second
/// configuration line.
fn printable(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|b| (0x20..=0x7e).contains(&b))
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

/// Split curl's `write-out` line off the end of hop one's stdout.
///
/// The line is `<status> <redirect url>`, and the URL half is empty whenever
/// the reply was not a redirect. Split from the END, because the body above it
/// is the far side's and may contain anything at all.
fn split_status_and_redirect(stdout: &str) -> (String, Option<u16>, Option<String>) {
    let (body, tail) = match stdout.rsplit_once('\n') {
        Some((body, tail)) => (body.to_string(), tail),
        None => (String::new(), stdout),
    };
    let (status, location) = match tail.trim_start().split_once(' ') {
        Some((status, rest)) => (status, rest.trim()),
        None => (tail.trim(), ""),
    };
    let location = if location.is_empty() {
        None
    } else {
        Some(location.to_string())
    };
    (body, status.trim().parse::<u16>().ok(), location)
}

/// Split curl's `write-out` status off the end of hop two's stdout.
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

#[cfg(test)]
mod tests;
