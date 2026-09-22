//! Google Drive: read one file, with a token the device grant obtained
//! (roadmap P2-017).
//!
//! This is the transport behind the `google` entry in
//! [`apex_secret_core::account::PROVIDERS`]. Until it existed, `apex account
//! add google.<name> --client-id <id>` could sign a person in, store an access
//! token, file a refresh token beside it and renew that token forever — and the
//! token could be spent on **nothing**, because no operation in
//! `default_registry()` began with `gdrive.`. `apex account scopes google`
//! said so in as many words. This is the operation that makes the credential
//! worth holding.
//!
//! # One operation, and the reason is Google's, not this build's
//!
//! `gdrive.file.read` and nothing else. The limit is the device grant's:
//! Google's limited-input-device guide allows only `email`, `openid`,
//! `profile`, `drive.appdata`, `drive.file`, `youtube` and `youtube.readonly`
//! — full `drive` and `drive.readonly` are not on the list. So
//! [`apex_secret_core::account::GOOGLE_OAUTH`] asks for `drive.file`, and
//! `drive.file` sees **only files this OAuth client created or the user
//! individually picked**. "List the folder" is not an operation a device-code
//! Google account can perform at all, whatever transport is written for it, so
//! there is no `gdrive.file.list` here rather than one that returns an empty
//! listing and reads as an empty Drive.
//!
//! What follows from that, stated where somebody will read it before they are
//! confused by it: **on a Drive that APEX has never written to, every file id
//! answers 404.** That is the scope working, not the transport failing. A write
//! operation is what makes the first file readable, and it is not a flag on
//! this one — Drive uploads go to `/upload/drive/v3/files`, a different path
//! from the `/drive/v3` an account is stored with, so it is its own commit.
//!
//! # The resource is a file id, so [`ResourceKind::Name`] and not `Path`
//!
//! A Drive file is addressed by an opaque id — `1BxiMVs0XRA5nFMdKvBdBZjgmUU…`
//! — not by a path, and there is no directory to walk. `Name` is the shape
//! that says so, and it is also the shape that refuses what matters:
//! [`apex_secret_core::operation::valid_name`] takes ASCII letters, digits,
//! `_`, `.` and `-`, refuses a leading `-`, refuses `/`, `..`, `.`, `:`, `?`
//! and `#`, and caps the length. Every byte it does accept is unreserved in a
//! URL, so the id goes into the path with no encoding and there is nothing to
//! get wrong.
//!
//! Two consequences worth naming rather than discovering:
//!
//! * A file id that begins with `-` is refused. Drive ids are base64url and one
//!   could in principle start that way; the refusal is the closed vocabulary's
//!   rule that a resource may never be read as an option, and it is a refusal
//!   with a reason rather than a silent mangling.
//! * `?` cannot appear, so `alt=media` is the only query this request can
//!   carry. The caller contributes the id and the id alone.
//!
//! # Where the token goes, and the two pins that decide it
//!
//! The URL is the **stored record's own** scheme, host, port and path with
//! `/files/<id>?alt=media` under it — the same construction
//! [`crate::providers::webdav`] uses, and the framework then pins what
//! [`GdriveProvider::bind`] returns against that record before the value is
//! read.
//!
//! That pin alone checks the provider agrees with the store; it cannot check
//! that the store is Google. So `bind` **also** refuses a credential pinned to
//! any host but the one [`apex_secret_core::account::PROVIDERS`] fixes `google`
//! to, read off that table rather than spelled again here. `Host::Fixed` stops
//! `apex account add` storing another, but `apex secret add` is a lower-level
//! command with no account vocabulary, and this is what stops a `gdrive` grant
//! on such a record putting a Google access token on somebody else's server.
//! It is one check, it is cheap, and it is what makes this "Google's
//! transport" rather than "any Drive-shaped host".
//!
//! # Nothing here reaches Google
//!
//! There is no Google account on any machine this repository is built on. The
//! tests run against a loopback double that parses the request that actually
//! arrived, and a real credential is pinned to `www.googleapis.com`, which
//! [`GdriveProvider::new`] is the only constructor that accepts — the
//! loopback host reaches the table through a `#[cfg(test)]` constructor and
//! through nothing that ships.

use apex_secret_core::account::{self, Host};
use apex_secret_core::operation::{Effect, OperationSpec, ProviderSpec, ResourceKind};
use apex_secret_core::store::ServiceInfo;
use apex_secret_core::SecretValue;

use crate::broker;
use crate::provider::{
    Approval, Bind, Bound, Endpoint, Performed, Provider, ProviderError,
};

/// The account provider whose transport this is.
///
/// Its row in [`apex_secret_core::account::PROVIDERS`] is where the API host
/// comes from, so the two cannot drift.
const ACCOUNT_PROVIDER: &str = "google";

/// How long one call may take, end to end.
const TIMEOUT_SECS: u64 = 120;
const CONNECT_TIMEOUT_SECS: u64 = 20;

pub const SPEC: ProviderSpec = ProviderSpec {
    id: "gdrive",
    summary: "read a file from a Google Drive account, by its file id",
    operations: &[OperationSpec {
        id: "gdrive.file.read",
        summary: "read a file from this Google account, by its Drive file id",
        effect: Effect::Read,
        resource: ResourceKind::Name,
        params: &[],
        aliases: &[],
        // FALSE, and for the same reason the OAuth refresh is false rather
        // than because the claim would be untrue: `bind` reads
        // `req.service` and never `req.project`, so it does reach the same
        // thing in every directory. The claim is held to
        // `providers::tests::an_operation_that_claims_to_reach_the_same_thing_
        // everywhere_binds_the_same_in_two_projects`, whose fixture binds with
        // an empty resource — and an operation that names something cannot
        // bind there, which makes that test panic rather than pass. Turning
        // this on means extending a shared fixture, which belongs with the
        // decision to grant `--everywhere` and not beside a new provider.
        same_everywhere: false,
        supersedes_credentials: false,
    }],
};

/// The Drive transport.
pub struct GdriveProvider {
    /// API hosts accepted besides Google's own.
    ///
    /// Empty in every shipped build — [`GdriveProvider::new`] builds it empty
    /// and nothing else that compiles into the daemon constructs one. It
    /// exists because a loopback double is `127.0.0.1`, and the account table
    /// fixes `google` to `www.googleapis.com` **by design**: putting a test
    /// host in that table would be putting a route to a test host in the
    /// daemon. The same shape [`crate::providers::oauth::OAuthProvider::at`]
    /// uses, for the same reason.
    extra_hosts: Vec<String>,
}

impl GdriveProvider {
    pub fn new() -> GdriveProvider {
        GdriveProvider {
            extra_hosts: Vec::new(),
        }
    }

    /// A stand-in Drive API on a loopback host, for tests only.
    #[cfg(test)]
    pub fn at(host: &str) -> GdriveProvider {
        GdriveProvider {
            extra_hosts: vec![host.to_string()],
        }
    }

    /// Google's Drive API host, read off the account table.
    ///
    /// Not a literal here. The table is what `apex account add google.<name>`
    /// pins a credential to, and a build where the two disagreed would refuse
    /// every account it had itself just stored.
    fn api_host() -> Result<&'static str, ProviderError> {
        match account::provider(ACCOUNT_PROVIDER).map(|p| p.host) {
            Some(Host::Fixed(host)) => Ok(host),
            // Unreachable while the table has a `google` row with a fixed
            // host, which `account`'s own tests require. Answered rather than
            // panicked: a daemon does not abort because a table moved.
            _ => Err(ProviderError::Failed(format!(
                "this build has no fixed API host for '{ACCOUNT_PROVIDER}' \
                 accounts, so it cannot tell whether a credential is stored \
                 for Google"
            ))),
        }
    }

    /// Whether a credential pinned to `host` is one this transport will spend.
    fn is_google(&self, host: &str) -> Result<bool, ProviderError> {
        // A trailing dot is the DNS root and names the same host; comparing
        // without trimming it would refuse a credential that works.
        let host = host.trim().trim_end_matches('.');
        if self
            .extra_hosts
            .iter()
            .any(|h| h.eq_ignore_ascii_case(host))
        {
            return Ok(true);
        }
        Ok(GdriveProvider::api_host()?.eq_ignore_ascii_case(host))
    }
}

impl Default for GdriveProvider {
    fn default() -> GdriveProvider {
        GdriveProvider::new()
    }
}

impl Provider for GdriveProvider {
    fn spec(&self) -> &'static ProviderSpec {
        &SPEC
    }

    fn bind(&self, req: &Bind<'_>) -> Result<Bound, ProviderError> {
        if !self.is_google(&req.service.host)? {
            return Err(ProviderError::Refused(format!(
                "'{}' is stored for {}, and this operation spends a Google \
                 OAuth access token, which goes to {} and nowhere else. Store \
                 the account with `apex account add {ACCOUNT_PROVIDER}.<name> \
                 --client-id <id>`, which pins the right host for you.",
                req.service.service.escape_debug(),
                req.service.host.escape_debug(),
                GdriveProvider::api_host()?
            )));
        }
        // Built here as well as in `perform`, so an id this provider could not
        // turn into a request is refused BEFORE the grant check spends
        // anything and before a credential is read.
        drive_url(req.service, req.resource)?;
        Ok(Bound {
            endpoint: Endpoint::from_url(&req.service.url())?,
            // The account and the file id, not the URL. The id is the caller's
            // own word and is what makes the line useful; the endpoint is the
            // same on every one of these lines and printing it would only
            // invite somebody to grep the trail for where a person's files
            // are.
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
        if !self.is_google(&req.service.host)? {
            return Err(ProviderError::Failed(
                "this credential is not stored for Google's API host".to_string(),
            ));
        }
        if req.operation.id != "gdrive.file.read" {
            // The registry routed here on the id's first segment, so an id
            // this provider declares and does not implement is a registry
            // entry with no implementation — a refusal, never a default.
            return Err(ProviderError::Refused(format!(
                "{} is declared by this provider and not implemented by it",
                req.operation.id
            )));
        }
        let url = drive_url(req.service, req.resource)?;
        let Some(token) = value.as_str() else {
            return Err(ProviderError::Refused(
                "the stored Google access token is not text, so it cannot be \
                 an Authorization header"
                    .to_string(),
            ));
        };
        // The store's own mapping from `ServiceInfo::auth` to a header,
        // rather than a second copy of it here: an account stored by `apex
        // account add google.<name>` is `bearer`, and a build that spelled
        // `Bearer` again here would keep working while the two drifted.
        let authorization = req.service.header_value(token);
        // Every value that reaches the configuration is CHECKED and not
        // escaped. `quoted` handles `"` and `\` and does nothing about a
        // newline, and a newline in any of these would be a second
        // configuration line — which for the header is a second header of the
        // attacker's choosing, carrying this token.
        for (what, text) in [("the url", url.as_str()), ("the authorization", authorization.as_str())] {
            if !printable(text) {
                return Err(ProviderError::Failed(format!(
                    "{what} this build composed is not something it will put \
                     in a curl configuration"
                )));
            }
        }

        let mut config = String::new();
        config.push_str(&format!("url = {}\n", quoted(&url)));
        config.push_str("request = \"GET\"\n");
        // The scheme the credential was stored for and nothing else, so a
        // redirect cannot downgrade this token onto http.
        config.push_str(&format!(
            "proto = {}\n",
            quoted(&format!("={}", req.service.scheme))
        ));
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
        // No `location`. Drive answers `alt=media` on this host for a file
        // small enough to carry, and a redirect would be the far end choosing
        // where this token goes next.
        config.push_str("silent\nshow-error\n");
        config.push_str(&format!("max-filesize = {}\n", broker::HTTP_MAX_BYTES));
        config.push_str("write-out = \"\\n%{http_code}\"\n");

        let out = broker::run_curl(&config, req.owner).map_err(ProviderError::Failed)?;
        // Before the status is read, because the status is printed by
        // `write-out` whether or not the transfer finished. A file over
        // `max-filesize` makes curl exit 63 with stdout of exactly `"\n200"`,
        // which reads as a successful read of an empty Drive file. See
        // `broker::aborted_transfer`.
        if let Some(why) = broker::aborted_transfer(&out) {
            return Err(ProviderError::Failed(why));
        }
        let (body, status) = split_status(&out.stdout);
        let Some(status) = status else {
            return Err(ProviderError::Failed(format!(
                "curl produced no HTTP status, so nothing is known about \
                 whether this reached {}: {}",
                req.service.host,
                one_line(&out.stderr)
            )));
        };
        if !(200..300).contains(&status) {
            // Not an `Err`: Drive's refusals are JSON documents with a reason
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
        Ok(Performed {
            code: 0,
            // What Drive sent, as text. `broker::run_curl` hands back a
            // `String`, so a file that is not UTF-8 arrives with its invalid
            // sequences replaced — the same property `s3.object.read` has, and
            // the reason neither of them is a way to move a binary.
            output: body,
            created: None,
            replaced: Vec::new(),
        })
    }
}

/// `<stored endpoint>/files/<id>?alt=media`.
///
/// The scheme, host, port and path all come from the credential as it was
/// stored; the caller contributes the id and nothing else. Checked here as
/// well as by `OperationSpec::check`, because the function that builds the
/// string cannot see that somebody else checked — the same argument
/// [`crate::broker::webdav_url`] makes.
fn drive_url(service: &ServiceInfo, file_id: &str) -> Result<String, ProviderError> {
    if !apex_secret_core::operation::valid_name(file_id) {
        return Err(ProviderError::NoSuchResource(format!(
            "'{}' is not a Drive file id: letters, digits, '_', '.' and '-', \
             not starting with '-'",
            file_id.escape_debug()
        )));
    }
    let base = service.url();
    let base = base.strip_suffix('/').unwrap_or(&base);
    // `alt=media` is what asks Drive for the file's CONTENT. Without it the
    // same URL answers with the file's metadata, which is a different thing
    // and would read as "the file is a small JSON document".
    Ok(format!("{base}/files/{file_id}?alt=media"))
}

/// What to say after a status that means the token, rather than the file.
///
/// 401 is the one a person can act on without knowing anything about Drive,
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
            "\napex: a Google access token lasts about an hour. Renew it with \
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

#[cfg(test)]
mod tests;
