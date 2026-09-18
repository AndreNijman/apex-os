//! Online accounts: a cloud identity is an entry in the credential store
//! (roadmap P2-017).
//!
//! ## The one decision this module exists to make
//!
//! An online account is **not a new kind of thing**. It is a [`ServiceInfo`]
//! in the store `apex-secretd` already owns, under a reserved name, with the
//! provider written down so the rest of APEX can tell an account from a git
//! token without guessing.
//!
//! [`ServiceInfo`]: crate::store::ServiceInfo
//!
//! That is the whole architecture, and it is worth saying why rather than
//! shipping a second store next to the first:
//!
//! * P0-002 moved credentials out of `$HOME` into
//!   `/var/lib/apex-secretd/users/<uid>/`, root-owned and `0700`, so a process
//!   with the user's uid cannot read one at rest. A Nextcloud app password in
//!   `~/.config/rclone/rclone.conf` is exactly the file that move was about.
//!   P2-017's first criterion — "without spraying credentials across user
//!   config" — is that property, and it is already built. Re-implementing it
//!   in an accounts daemon would mean re-earning it.
//! * P2-016 found what a second write path costs: `apex-remoted` checked the
//!   caller for `Pair` and for nothing else, and a second account read the
//!   owner's device list out of `Devices`. One store means one `SO_PEERCRED`
//!   check, in one place, on every verb.
//! * A grant is keyed `service:capability` per project
//!   (`crate::store::grant_key`). An account that is a service gets the
//!   existing per-project scoped grant for free, and account removal is
//!   `Request::Remove`, which already deletes *every grant that named it* —
//!   which is P2-017's third criterion, not a new feature.
//!
//! ## Names
//!
//! `account.<provider>.<name>` — `account.nextcloud.home`, `account.s3.backups`.
//!
//! The prefix is a namespace and not decoration: it is how `apex account list`
//! shows accounts without showing the user's git token, and how a git token
//! cannot be revoked by an accounts command. It costs nothing, because
//! [`crate::store::valid_service_name`] already allows `.` and already refuses
//! both `/` and `:` — so an account name can neither escape the store's
//! per-service file nor forge a grant key by carrying the separator in it.
//! Both of those are asserted below rather than assumed.
//!
//! ## Providers, and the second namespace nobody expects
//!
//! Five providers are named in the criterion — Nextcloud, Google, Microsoft,
//! WebDAV and S3/R2 — and they are **not** five protocols. Nextcloud *is*
//! WebDAV; a Nextcloud account and a bare WebDAV account differ in how the
//! credential is obtained and in nothing the broker does afterwards. So a
//! provider declares the operation namespace it routes into
//! ([`Provider::transport`]) separately from its own id, and both Nextcloud and
//! WebDAV route into `webdav`.
//!
//! This matters because of P1-001's routing rule: an operation id is
//! `provider.class.verb` and *the first segment routes*, with no
//! operation-to-provider table anywhere. If `nextcloud.file.read` and
//! `webdav.file.read` were separate ids, `apex-secretd` would need two
//! identical registry entries and they would drift. One transport, two ways in.
//!
//! ## What this module deliberately does not do
//!
//! It holds no credential, performs no operation and talks to no network. It
//! is the vocabulary: which providers exist, what a scope means, and what an
//! account is called. Obtaining the credential is the front-end's job and
//! spending it is `apex-secretd`'s.

use crate::operation::Effect;

/// The reserved prefix. See the module note.
pub const NAMESPACE: &str = "account";

/// Longest an account's local name may be.
///
/// [`crate::store::valid_service_name`] caps the whole service name at 64 bytes, and the
/// prefix plus the longest provider id spends some of that. Bounded here so
/// the refusal names the account name the user typed rather than a derived
/// string they never wrote.
pub const MAX_NAME: usize = 32;

/// How the credential is obtained from the provider.
///
/// Recorded because it is the difference between the providers rather than a
/// label: it decides what `apex account add` can do unattended, and whether a
/// stored credential can be refreshed or only replaced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flow {
    /// The user creates a long-lived token in the provider's own UI and pastes
    /// it. No browser, no redirect, scriptable, and therefore the only flow a
    /// headless test can exercise end to end.
    Token,
    /// An app password: a per-application secret the user generates, presented
    /// as HTTP Basic. Nextcloud's Settings → Security, and any WebDAV server
    /// with per-device passwords.
    AppPassword,
    /// An access key id and secret, signed per request. S3 and R2.
    AccessKey,
    /// OAuth 2.0 device authorization grant (RFC 8628): the machine prints a
    /// code, the user approves it on another device. No loopback listener and
    /// no browser on this machine, which is why it is preferred here over the
    /// authorization-code flow where a provider offers both.
    DeviceCode,
    /// OAuth 2.0 authorization code with PKCE. Needs a browser and a loopback
    /// redirect on this machine.
    AuthCode,
}

impl Flow {
    pub fn as_str(&self) -> &'static str {
        match self {
            Flow::Token => "token",
            Flow::AppPassword => "app-password",
            Flow::AccessKey => "access-key",
            Flow::DeviceCode => "device-code",
            Flow::AuthCode => "auth-code",
        }
    }

    /// Whether the flow can complete without a browser or a second device.
    ///
    /// Not a nicety: `apex account add` runs on a machine that may have no
    /// working graphics at all — P2-018 is the item next to this one — and a
    /// flow that needs a browser has to say so before it starts rather than
    /// after.
    pub fn is_unattended(&self) -> bool {
        matches!(self, Flow::Token | Flow::AppPassword | Flow::AccessKey)
    }

    /// Whether the credential the flow yields expires and can be renewed
    /// without the user.
    ///
    /// An OAuth refresh token can; a pasted app password cannot, and telling
    /// the user their Nextcloud account "will refresh" would be a lie they
    /// find out about when a backup stops running.
    pub fn is_refreshable(&self) -> bool {
        matches!(self, Flow::DeviceCode | Flow::AuthCode)
    }
}

/// How the stored value is presented to the endpoint.
///
/// [`crate::store::ServiceInfo::auth`] has two values, `bearer` and `raw`,
/// because that is all an HTTP header needs. This enum is finer, and the
/// difference is deliberate: `Basic` and `SigV4` both *store* as `raw`, and
/// what turns the stored bytes into a header is the operation provider, at use
/// time, inside the daemon. [`Presentation::service_auth`] is the mapping, and
/// it exists so no caller has to remember it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Presentation {
    /// `Authorization: Bearer <value>`.
    Bearer,
    /// `Authorization: Basic <base64(user:value)>`, computed in the daemon.
    Basic,
    /// AWS Signature Version 4, computed per request in the daemon.
    SigV4,
}

impl Presentation {
    /// What goes in `ServiceInfo::auth`.
    pub fn service_auth(&self) -> &'static str {
        match self {
            Presentation::Bearer => "bearer",
            // Everything else is bytes the provider interprets. `raw` is the
            // store's word for "not a bearer token"; it is NOT an instruction
            // to send the value as a whole header for these two.
            Presentation::Basic | Presentation::SigV4 => "raw",
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Presentation::Bearer => "bearer",
            Presentation::Basic => "basic",
            Presentation::SigV4 => "sigv4",
        }
    }
}

/// Whether the token endpoint wants a `client_secret`, and what that costs.
///
/// Three states rather than a bool, because they send `apex account add`
/// somewhere different and collapsing them would make one provider's hard
/// requirement look like another's optional extra.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientSecret {
    /// A public client: RFC 8252 §8.5's case. `client_id` alone is enough at
    /// every step. Microsoft, and Cloudflare's own device flow.
    None,
    /// The token endpoint refuses the poll without one.
    ///
    /// Google's limited-input-device flow, read from
    /// `developers.google.com/identity/protocols/oauth2/limited-input-device`:
    /// `client_secret` is listed **Required** on the poll and *Optional* on the
    /// refresh. This is why APEX ships no Google `client_id` by default — see
    /// [`OAuth::client_id`].
    RequiredToObtain,
}

/// An authorisation server APEX speaks RFC 8628 and RFC 6749 §6 to.
///
/// ## Why the table is here and not in either front-end
///
/// Two different binaries need it and neither can link the other. `apex`
/// runs the device grant (`apex account add`, `apex cf connect`);
/// `apex-secretd` runs the refresh, because a refresh spends a stored
/// credential and only the daemon may read one. A second copy of these URLs
/// would be a second copy of these URLs, and the one that drifts is the one
/// that sends a credential somewhere.
///
/// It also makes the refresh **pin-consistent**: the daemon does not take a
/// token endpoint from the caller or from the credential, it looks one up by
/// the host the credential was already pinned to
/// ([`oauth_for_auth_host`]). A refresh credential filed under
/// `dash.cloudflare.com` can be POSTed to `dash.cloudflare.com`'s token
/// endpoint and to nowhere else.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OAuth {
    /// RFC 8628 §3.1's device authorization endpoint.
    pub device_url: &'static str,
    /// RFC 6749 §3.2's token endpoint: polled during the grant, and POSTed
    /// again to refresh. Must be on [`OAuth::auth_host`].
    pub token_url: &'static str,
    /// The host both URLs are on, and therefore the host the REFRESH
    /// credential is pinned to.
    ///
    /// Separate from the provider's API host on purpose, and it is the single
    /// best idea in `apex cloudflare connect`: a refresh token filed under
    /// `dash.cloudflare.com` **cannot be spent as an API token** against
    /// `api.cloudflare.com`, because the framework pins a credential to the
    /// host it was stored for and refuses a request that goes anywhere else.
    /// The separation is not tidiness; it is what makes the second secret
    /// harmless if the first is granted away.
    pub auth_host: &'static str,
    /// Asked for, and no more — §13.5's rule at the only moment it can be
    /// applied, because a token's scopes are fixed when it is issued.
    pub scopes: &'static [&'static str],
    pub client_secret: ClientSecret,
    /// The OAuth client to ask as, when this build ships one.
    ///
    /// `None` means **APEX has no registered application at this provider and
    /// will not borrow somebody else's**, so `--client-id` is required and the
    /// refusal says why. That is the honest answer for Google and Microsoft
    /// and it is worth stating plainly:
    ///
    /// * Every third-party tool that speaks these flows embeds a client id it
    ///   registered — and for Google, a `client_secret` with it, because the
    ///   flow requires one. Shipping a borrowed pair would put another
    ///   project's credential in APEX's binary and put APEX's users on another
    ///   project's consent screen and quota.
    /// * `apex cf connect` already makes the opposite call for Cloudflare, and
    ///   says so where a user can read it: the default is **Wrangler's** id,
    ///   the consent screen says Wrangler, and the grant a person approves is
    ///   Wrangler's. That was a decision about one provider, made with its
    ///   reasoning written down, not a precedent to copy silently.
    pub client_id: Option<&'static str>,
}

/// Google's limited-input-device flow.
///
/// `openid` and `profile` name the user; `drive.file` is the one that can be
/// spent, and it is asked for **because** [`DRIVE_SCOPES`] now names an
/// operation a provider performs. It was deliberately absent while nothing
/// could spend it: asking for a permission this build cannot use is a consent
/// screen that overstates what APEX is about to do.
///
/// `drive.file` and not `drive` or `drive.readonly`, and that is Google's
/// rule rather than a choice — the limited-input-device grant does not accept
/// either of those. See [`DRIVE_SCOPES`] for what `drive.file` can and cannot
/// see.
///
/// A token's scopes are fixed when it is issued, so an account signed in
/// before this line existed holds a token with no Drive scope in it: its
/// reads answer 403 until `apex account add google.<name>` is run again. A
/// refresh cannot widen a grant, by RFC 6749 §6's design.
pub const GOOGLE_OAUTH: OAuth = OAuth {
    device_url: "https://oauth2.googleapis.com/device/code",
    token_url: "https://oauth2.googleapis.com/token",
    auth_host: "oauth2.googleapis.com",
    scopes: &[
        "openid",
        "profile",
        "https://www.googleapis.com/auth/drive.file",
    ],
    client_secret: ClientSecret::RequiredToObtain,
    client_id: None,
};

/// Microsoft identity platform, `common` tenant.
///
/// `common` rather than `consumers` or `organizations` because it takes both a
/// personal Microsoft account and a work or school one, and APEX has no way to
/// know which the user has before they sign in. The cost is documented by
/// Microsoft and worth repeating: on `common` and `consumers` a personal
/// account is asked to sign in a second time, because the device cannot reach
/// the browser's cookies.
///
/// `offline_access` is what makes a refresh token come back at all, so it is
/// not optional for a flow whose whole point is that it can be renewed.
pub const MICROSOFT_OAUTH: OAuth = OAuth {
    device_url: "https://login.microsoftonline.com/common/oauth2/v2.0/devicecode",
    token_url: "https://login.microsoftonline.com/common/oauth2/v2.0/token",
    auth_host: "login.microsoftonline.com",
    scopes: &["openid", "profile", "offline_access"],
    client_secret: ClientSecret::None,
    client_id: None,
};

/// The OAuth client `apex cloudflare connect` asks as.
///
/// **Wrangler's**, and that is a decision with its reasoning written down
/// rather than an accident: Cloudflare registers no application for APEX, the
/// consent screen a person approves says Wrangler, and `apex cf connect` tells
/// them so. It is here, in the crate both binaries link, because the CLI runs
/// the grant and the daemon runs the refresh, and RFC 6749 §6 requires the
/// refresh to present the SAME client. Two copies of this string is one copy
/// that can drift, and the one that drifts is the one that stops refreshing.
pub const WRANGLER_CLIENT_ID: &str = "54d11594-84e4-41aa-b438-e81b8fa78ee7";

/// Cloudflare's device grant, as `apex cloudflare connect` already uses it.
///
/// Listed here so `apex-secretd` can refresh the token that command stored.
/// It is deliberately NOT in [`PROVIDERS`] — a Cloudflare credential is not an
/// "online account", it is §13.1's project identity, and `apex account` must
/// not be able to delete it.
pub const CLOUDFLARE_OAUTH: OAuth = OAuth {
    device_url: "https://dash.cloudflare.com/oauth2/device/auth",
    token_url: "https://dash.cloudflare.com/oauth2/token",
    auth_host: "dash.cloudflare.com",
    // The list `apex/src/cloudflare.rs` asks for, which is one scope per
    // operation the shipped provider can actually perform. It is not repeated
    // here: the CLI owns the grant and this entry exists for the refresh,
    // which re-presents whatever was granted and asks for nothing new.
    scopes: &[],
    client_secret: ClientSecret::None,
    client_id: Some(WRANGLER_CLIENT_ID),
};

/// Every authorisation server this build knows how to refresh against.
pub const OAUTH: &[&OAuth] = &[&GOOGLE_OAUTH, &MICROSOFT_OAUTH, &CLOUDFLARE_OAUTH];

/// The endings that mark a stored name as a refresh credential.
///
/// Two, because two conventions already exist on disk and neither may be
/// renamed: [`AccountRef::refresh_service`] appends `.refresh`, and
/// `apex cloudflare connect` — which invented the split-host idea this is all
/// carried over from — has been filing `cloudflare-refresh` since before there
/// were accounts. Renaming either would orphan a credential somebody is
/// holding, and the whole value of a refresh token is that it was stored once
/// and has not had to be typed again.
pub const REFRESH_SUFFIXES: &[&str] = &[".refresh", "-refresh"];

/// The operation that renews an access token with the refresh token beside it.
///
/// Named here, in the crate the CLI and the daemon both link, rather than in
/// either of them. `apex-secretd`'s `oauth` provider declares its one
/// operation with this constant and `apex cloudflare refresh` asks for it with
/// this constant, so the two cannot drift into a request for an operation no
/// provider offers — the exact defect the cross-crate scope gate in
/// `apex-secretd::providers` was written for.
pub const REFRESH_OPERATION: &str = "oauth.token.refresh";

/// Which stored credential a refresh credential renews.
///
/// [`AccountRef::refresh_service`]'s inverse, generalised to cover the CLI's
/// older spelling as well, and the daemon's only way to answer the question:
/// `bind` never sees a value and must not take a name from the caller, so the
/// one thing it has is [`crate::store::ServiceInfo::service`] — the name the
/// request is already running against.
///
/// Deriving the sibling from that name is safe because it does not widen
/// anything. The framework has already matched a grant on *this* credential,
/// the name it produces still has to name a credential the same uid stored,
/// and the value written is the authorisation server's rather than the
/// caller's. What it cannot do is reach another user's store or invent a host
/// — the first because the uid comes from `SO_PEERCRED`, the second because
/// the record the value is written into is the one that was already there.
///
/// `None` when the name carries neither suffix, which means it is not a
/// refresh credential at all and there is nothing to renew.
pub fn renewed_service(refresh_service: &str) -> Option<String> {
    REFRESH_SUFFIXES
        .iter()
        .find_map(|suffix| refresh_service.strip_suffix(suffix))
        .filter(|stem| !stem.is_empty())
        .map(str::to_string)
}

/// The authorisation server a credential pinned to `host` belongs to.
///
/// The daemon's only way to find a token endpoint. It takes the host the
/// credential was *already pinned to* rather than anything the caller said, so
/// there is no request shape that can aim a refresh somewhere else.
pub fn oauth_for_auth_host(host: &str) -> Option<&'static OAuth> {
    let host = host.trim().trim_end_matches('.');
    OAUTH
        .iter()
        .copied()
        .find(|o| o.auth_host.eq_ignore_ascii_case(host))
}

/// Where the endpoint lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Host {
    /// The provider has one, and an account may not name another. A Google
    /// account that pointed at a host of the caller's choosing would be a
    /// credential pinned to somewhere Google is not.
    Fixed(&'static str),
    /// Self-hosted: the user's own server, required at `add` time.
    PerAccount,
}

/// One thing an account may be granted.
///
/// The name is what a person types and reads; the operation is what the broker
/// enforces. Separate, because `files.read` is a vocabulary a user can hold in
/// their head and `webdav.file.read` is a routing decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Scope {
    pub name: &'static str,
    /// The §13.2 operation id this scope grants. Its first segment must be the
    /// provider's [`Provider::transport`] — asserted by a test, because P1-001
    /// routes on that segment and a mismatch would be a grant for an operation
    /// that goes somewhere else.
    pub operation: &'static str,
    pub effect: Effect,
    /// One line, second person, for `apex account scopes`.
    pub summary: &'static str,
}

/// A provider APEX can hold an account with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Provider {
    pub id: &'static str,
    pub label: &'static str,
    /// The operation namespace this provider's scopes route into. Often the
    /// same string as [`Provider::id`]; deliberately not required to be. See
    /// the module note.
    pub transport: &'static str,
    pub flow: Flow,
    pub presentation: Presentation,
    pub host: Host,
    /// The endpoint path stored with the credential, for a provider whose
    /// destination is fixed. Empty when the path is per-account.
    pub path: &'static str,
    /// Whether [`Provider::path`] is a prefix that the account's own username
    /// completes.
    ///
    /// Nextcloud's files endpoint is `/remote.php/dav/files/<username>/`, and
    /// a credential stored without that last segment points at a collection
    /// the server answers 404 for — on every real Nextcloud, for every file.
    /// A boolean rather than a `{username}` placeholder in the path, because a
    /// placeholder invites a provider to put one somewhere the join cannot
    /// safely substitute.
    pub path_carries_username: bool,
    /// What the user is told to go and do to get a credential. A provider
    /// whose instructions are wrong is a support burden, so this is one line
    /// naming the page, not a tutorial.
    pub obtain: &'static str,
    pub scopes: &'static [Scope],
    /// The authorisation server, for a provider whose [`Provider::flow`] is an
    /// OAuth one. `None` for every other flow.
    ///
    /// `Flow::DeviceCode` without this is a provider `apex account add` would
    /// offer to sign in to and then have nowhere to ask, so
    /// [`Provider::validate`] refuses the combination and a test runs it over
    /// the shipped table.
    pub oauth: Option<&'static OAuth>,
}

impl Provider {
    /// Look a scope up by the name a person types.
    pub fn scope(&self, name: &str) -> Option<&'static Scope> {
        self.scopes.iter().find(|s| s.name == name)
    }

    /// The endpoint path to store for an account with this username.
    ///
    /// The join is here rather than in the front-end so that a second caller —
    /// a Settings page, a migration — cannot get it wrong in a different way.
    pub fn path_for(&self, username: &str) -> String {
        if self.path_carries_username && !username.is_empty() {
            format!("{}/{}", self.path.trim_end_matches('/'), username)
        } else {
            self.path.to_string()
        }
    }

    /// The host, for a listing: the fixed one, or what the user must supply.
    pub fn host_display(&self) -> String {
        match self.host {
            Host::Fixed(h) => h.to_string(),
            Host::PerAccount => "yours (--host)".to_string(),
        }
    }

    /// Whether this provider needs `--host` at `add` time.
    pub fn needs_host(&self) -> bool {
        matches!(self.host, Host::PerAccount)
    }

    /// Whether the declaration is internally consistent.
    ///
    /// Modelled on [`crate::operation::ProviderSpec::validate`], and here for
    /// the same reason: these are programming errors, and a table checked by a
    /// test that runs over every row is cheaper than one found when somebody
    /// is halfway through signing in.
    pub fn validate(&self) -> Result<(), String> {
        match (self.flow, self.oauth) {
            (Flow::DeviceCode | Flow::AuthCode, None) => {
                return Err(format!(
                    "{} declares the {} flow and names no authorisation server, so \
                     `apex account add` would offer to sign in and have nowhere to ask",
                    self.id,
                    self.flow.as_str()
                ))
            }
            (flow, Some(_)) if !flow.is_refreshable() => {
                return Err(format!(
                    "{} names an authorisation server but its flow ({}) is not an OAuth \
                     one, so nothing would ever use it",
                    self.id,
                    flow.as_str()
                ))
            }
            _ => {}
        }
        if let (Host::Fixed(api), Some(oauth)) = (self.host, self.oauth) {
            // The whole point of the second service. If the two were the same
            // host the endpoint pin would stop distinguishing them, and a
            // refresh token would become spendable as an access token.
            if oauth.auth_host.eq_ignore_ascii_case(api) {
                return Err(format!(
                    "{}'s API host and authorisation host are both '{api}'. The refresh \
                     token is kept unspendable as an API token BY the host pin, so this \
                     would silently give up that property",
                    self.id
                ));
            }
        }
        if let Some(oauth) = self.oauth {
            for (what, url) in [("device", oauth.device_url), ("token", oauth.token_url)] {
                let expected = format!("https://{}/", oauth.auth_host);
                if !url.starts_with(&expected) {
                    return Err(format!(
                        "{}'s {what} endpoint is '{url}', which is not on its declared \
                         authorisation host '{}'. The daemon finds a token endpoint by \
                         looking up the host a credential is PINNED to, so a mismatch \
                         here would make a refresh unroutable",
                        self.id, oauth.auth_host
                    ));
                }
            }
        }
        Ok(())
    }

    /// The host to store, given what the caller asked for.
    ///
    /// Refuses rather than silently preferring one: a caller who passes
    /// `--host` to a fixed-host provider has a belief about where their
    /// credential is going, and quietly storing a different host would leave
    /// that belief in place.
    pub fn resolve_host(&self, asked: Option<&str>) -> Result<String, AccountError> {
        match (self.host, asked) {
            (Host::Fixed(h), None) => Ok(h.to_string()),
            (Host::Fixed(h), Some(a)) if a == h => Ok(h.to_string()),
            (Host::Fixed(h), Some(a)) => Err(AccountError::HostFixed {
                provider: self.id,
                fixed: h,
                asked: a.to_string(),
            }),
            (Host::PerAccount, Some(a)) if !a.is_empty() => Ok(a.to_string()),
            (Host::PerAccount, _) => Err(AccountError::HostRequired { provider: self.id }),
        }
    }
}

/// Read, list and write over WebDAV. Shared by every WebDAV-speaking provider.
const WEBDAV_SCOPES: &[Scope] = &[
    Scope {
        name: "files.list",
        operation: "webdav.file.list",
        effect: Effect::Read,
        summary: "list the files in a folder on this account",
    },
    Scope {
        name: "files.read",
        operation: "webdav.file.read",
        effect: Effect::Read,
        summary: "read a file from this account",
    },
    Scope {
        name: "files.write",
        operation: "webdav.file.write",
        effect: Effect::Write,
        summary: "write a file to this account, replacing what is there",
    },
];

/// Google Drive: **one**, and the count is Google's arithmetic rather than
/// this build's ambition.
///
/// This table used to name `gdrive.file.list`, `gdrive.file.read` and
/// `gdrive.file.write`. No provider in `apex-secretd` offered any of them, so
/// `apex account grant google files.read` recorded a grant for an operation
/// that could never be performed — a permission the user was told they had
/// made. It was emptied for that reason, and [`crate::account::PROVIDERS`] is
/// now checked against the shipped registry by a test in `apex-secretd`, the
/// only crate that can see both. `files.read` is back because the operation it
/// names now exists: `providers::gdrive` in that crate performs it.
///
/// It is ONE scope and not three because of what Google's limited-input flow
/// allows, read from
/// `developers.google.com/identity/protocols/oauth2/limited-input-device`
/// rather than from memory: the device grant accepts **only** `email`,
/// `openid`, `profile`, `drive.appdata`, `drive.file`, `youtube` and
/// `youtube.readonly`. Full `drive` and `drive.readonly` are not on the list.
/// `drive.file` sees only files the calling app itself created or the user
/// individually picked — so "list the files in a folder on this Drive" is not
/// a thing a device-code Google account can do **at all**, whatever transport
/// is written for it, and there is no `files.list` here rather than one that
/// returns an empty listing and reads as an empty Drive.
///
/// `files.write` is the scope worth adding next, and it is not a line in this
/// table on its own: a Drive upload goes to `/upload/drive/v3/files`, which is
/// a different path from the `/drive/v3` an account is stored with, so it
/// needs the transport before it needs the vocabulary. Until it exists, a
/// Drive that APEX has never written to has no file `files.read` can reach —
/// which is the scope working, and is said out loud in `apex account scopes
/// google` rather than left to be discovered as a 404.
const DRIVE_SCOPES: &[Scope] = &[Scope {
    name: "files.read",
    operation: "gdrive.file.read",
    effect: Effect::Read,
    summary: "read a file from this Google Drive, by its file id",
}];

/// Microsoft Graph: **none yet**, for the plainer reason.
///
/// Unlike Google, nothing about the device grant narrows this: a public client
/// may ask for `Files.Read`, `Files.ReadWrite` and `Mail.Read`. What is missing
/// is the transport — there is no `msgraph` provider in `default_registry()` —
/// and a scope table that names operations no provider serves is the defect
/// described on [`DRIVE_SCOPES`]. The four names this used to carry
/// (`msgraph.file.{list,read,write}`, `msgraph.mail.read`) are the right
/// vocabulary for that provider when somebody writes it.
const GRAPH_SCOPES: &[Scope] = &[];

const S3_SCOPES: &[Scope] = &[
    Scope {
        name: "objects.read",
        operation: "s3.object.read",
        effect: Effect::Read,
        // `objects.list` used to be a fourth scope here, pointing at
        // `s3.object.list`. The S3 provider has no such operation and never
        // had one: it renders a listing through `s3.object.read` when the
        // resource names a bucket without a key, "the same
        // one-operation-for-both shape `cloudflare.r2.object.read` has". One
        // operation, so one scope, and the summary says both things it does.
        summary: "read an object from a bucket, or list what is in one",
    },
    Scope {
        name: "objects.write",
        operation: "s3.object.write",
        effect: Effect::Write,
        summary: "write an object to a bucket, replacing what is there",
    },
];

/// Every provider APEX knows, in the order `apex account providers` prints
/// them: self-hosted first, because that is the one APEX can hold an account
/// with today without a browser.
pub const PROVIDERS: &[Provider] = &[
    Provider {
        id: "webdav",
        label: "WebDAV",
        transport: "webdav",
        flow: Flow::AppPassword,
        presentation: Presentation::Basic,
        host: Host::PerAccount,
        path: "",
        path_carries_username: false,
        obtain: "your server's own account settings; most offer a per-device password",
        scopes: WEBDAV_SCOPES,
        oauth: None,
    },
    Provider {
        id: "nextcloud",
        label: "Nextcloud",
        // Nextcloud is WebDAV. One transport, two ways in — see the module
        // note.
        transport: "webdav",
        flow: Flow::AppPassword,
        presentation: Presentation::Basic,
        host: Host::PerAccount,
        path: "/remote.php/dav/files",
        // Nextcloud's files endpoint ends in the account's own name.
        path_carries_username: true,
        obtain: "Settings -> Security -> Devices & sessions -> Create new app password",
        scopes: WEBDAV_SCOPES,
        oauth: None,
    },
    Provider {
        id: "s3",
        label: "S3 or Cloudflare R2",
        transport: "s3",
        flow: Flow::AccessKey,
        presentation: Presentation::SigV4,
        host: Host::PerAccount,
        path: "",
        path_carries_username: false,
        obtain: "an access key pair from the bucket's own console",
        scopes: S3_SCOPES,
        oauth: None,
    },
    Provider {
        id: "google",
        label: "Google",
        transport: "gdrive",
        flow: Flow::DeviceCode,
        presentation: Presentation::Bearer,
        host: Host::Fixed("www.googleapis.com"),
        path: "/drive/v3",
        path_carries_username: false,
        obtain: "sign in on another device when APEX prints the code",
        scopes: DRIVE_SCOPES,
        oauth: Some(&GOOGLE_OAUTH),
    },
    Provider {
        id: "microsoft",
        label: "Microsoft 365",
        transport: "msgraph",
        flow: Flow::DeviceCode,
        presentation: Presentation::Bearer,
        host: Host::Fixed("graph.microsoft.com"),
        path: "/v1.0/me",
        path_carries_username: false,
        obtain: "sign in on another device when APEX prints the code",
        scopes: GRAPH_SCOPES,
        oauth: Some(&MICROSOFT_OAUTH),
    },
];

/// Look up a provider by id.
pub fn provider(id: &str) -> Option<&'static Provider> {
    PROVIDERS.iter().find(|p| p.id == id)
}

/// What went wrong, said in terms the caller can act on.
///
/// `verify.rs::Verdict`'s rule applies here too: an unknown provider, a
/// malformed name and a host the provider does not have are three different
/// answers, and collapsing them into "invalid account" tells the user nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccountError {
    UnknownProvider(String),
    BadName(String),
    /// The whole token, when it does not even have a provider in it.
    BadRef(String),
    UnknownScope {
        provider: &'static str,
        scope: String,
    },
    /// The provider has no scopes at all yet, which is a different answer from
    /// "not that one".
    ///
    /// Told apart because the two send the reader somewhere different: an
    /// unknown scope means read the list, and an empty list means there is
    /// nothing to read and no amount of trying other names will help. Pointing
    /// at `apex account scopes google` when that command prints a header and
    /// no rows is the shape of refusal this repository keeps finding in its own
    /// code.
    NoScopesYet {
        provider: &'static str,
    },
    HostRequired {
        provider: &'static str,
    },
    HostFixed {
        provider: &'static str,
        fixed: &'static str,
        asked: String,
    },
}

impl std::fmt::Display for AccountError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AccountError::UnknownProvider(p) => {
                write!(
                    f,
                    "'{}' is not a provider APEX knows; `apex account providers` lists them",
                    p.escape_debug()
                )
            }
            AccountError::BadName(n) => write!(
                f,
                "'{}' is not an account name; use up to {MAX_NAME} characters of \
                 lowercase letters, digits, '_' and '-', starting with a letter or digit",
                n.escape_debug()
            ),
            AccountError::BadRef(t) => write!(
                f,
                "'{}' does not name an account; accounts are written \
                 <provider>.<name>, as in nextcloud.home",
                t.escape_debug()
            ),
            AccountError::UnknownScope { provider, scope } => write!(
                f,
                "{provider} has no scope '{}'; `apex account scopes {provider}` lists them",
                scope.escape_debug()
            ),
            AccountError::NoScopesYet { provider } => write!(
                f,
                "a {provider} account can be stored and refreshed, but this build has no \
                 {provider} transport, so there is no operation to grant it yet and \
                 nothing can spend the credential. `apex account scopes {provider}` says \
                 the same thing at more length"
            ),
            AccountError::HostRequired { provider } => write!(
                f,
                "a {provider} account is on your own server, so it needs --host"
            ),
            AccountError::HostFixed {
                provider,
                fixed,
                asked,
            } => write!(
                f,
                "{provider} accounts live at {fixed}; --host {} would pin the credential \
                 somewhere {provider} is not",
                asked.escape_debug()
            ),
        }
    }
}

impl std::error::Error for AccountError {}

/// Account names: the part the user chooses.
///
/// Stricter than [`crate::store::valid_service_name`] on purpose. That function allows `.`,
/// which is this namespace's own separator, and allows uppercase, which would
/// make `Home` and `home` two accounts that look like one in a list.
pub fn valid_account_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_NAME
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '_' | '-'))
        && name
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
}

/// An account, as a provider and the name the user gave it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountRef {
    pub provider: &'static Provider,
    pub name: String,
}

impl AccountRef {
    /// Build one, refusing an unknown provider and a malformed name.
    pub fn new(provider_id: &str, name: &str) -> Result<AccountRef, AccountError> {
        let provider =
            provider(provider_id).ok_or_else(|| AccountError::UnknownProvider(provider_id.into()))?;
        if !valid_account_name(name) {
            return Err(AccountError::BadName(name.into()));
        }
        Ok(AccountRef {
            provider,
            name: name.to_string(),
        })
    }

    /// The store's name for this account.
    pub fn service(&self) -> String {
        format!("{NAMESPACE}.{}.{}", self.provider.id, self.name)
    }

    /// Where this account's REFRESH token is filed, which is not where its
    /// access token is.
    ///
    /// Two properties, and both are load-bearing:
    ///
    /// 1. It is a **separate service pinned to the authorisation host**
    ///    ([`OAuth::auth_host`]), never the API host. The framework refuses a
    ///    request whose endpoint is not the host the credential was stored
    ///    for, so granting an agent every scope on `account.google.home` still
    ///    cannot get it the refresh token, and the refresh token cannot be
    ///    presented to the API as though it were an access token. `apex
    ///    cloudflare connect` invented this and it is carried across rather
    ///    than reinvented.
    /// 2. It **does not parse back as an account**. `.` is not a legal
    ///    character in an account name, so [`AccountRef::parse`] returns `None`
    ///    for this string: it never appears in `apex account list`, it cannot
    ///    be granted a scope, and `apex account rm` cannot be pointed at it
    ///    directly. `rm` removes it because it computes this name from the
    ///    account, which is the only way in.
    pub fn refresh_service(&self) -> String {
        format!("{}.refresh", self.service())
    }

    /// Read an account out of the form a person types: `nextcloud.home`.
    ///
    /// The same string as [`AccountRef::service`] with the namespace taken
    /// off, and that is deliberate rather than convenient — an account is
    /// addressed by one token everywhere, so the name in `apex account list`,
    /// the name in a grant, and the name in the audit trail are the same word.
    pub fn parse_ref(text: &str) -> Result<AccountRef, AccountError> {
        // A caller who pasted the stored name gets what they meant rather than
        // "no provider called account". The reverse — accepting `account.x.y`
        // as provider `account` — is impossible, because there is no provider
        // by that name.
        let text = text
            .strip_prefix(NAMESPACE)
            .and_then(|t| t.strip_prefix('.'))
            .unwrap_or(text);
        let (provider_id, name) = text
            .split_once('.')
            .ok_or_else(|| AccountError::BadRef(text.to_string()))?;
        AccountRef::new(provider_id, name)
    }

    /// Read an account back out of a service name, or `None` when the service
    /// is not an account at all.
    ///
    /// `None` rather than an error, and the distinction is the point: the
    /// store holds git tokens and MCP credentials next to accounts, and
    /// `apex account list` asking "is this one of mine" must get "no" for
    /// `github` rather than a refusal it has to special-case.
    pub fn parse(service: &str) -> Option<AccountRef> {
        let rest = service.strip_prefix(NAMESPACE)?.strip_prefix('.')?;
        let (provider_id, name) = rest.split_once('.')?;
        AccountRef::new(provider_id, name).ok()
    }

    /// Resolve a scope name to the operation id a grant will carry.
    pub fn operation(&self, scope: &str) -> Result<&'static str, AccountError> {
        self.provider
            .scope(scope)
            .map(|s| s.operation)
            .ok_or_else(|| {
                if self.provider.scopes.is_empty() {
                    AccountError::NoScopesYet {
                        provider: self.provider.id,
                    }
                } else {
                    AccountError::UnknownScope {
                        provider: self.provider.id,
                        scope: scope.to_string(),
                    }
                }
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::operation::OperationId;
    use crate::store::valid_service_name;

    #[test]
    fn every_provider_derives_a_service_name_the_store_accepts() {
        // The join is the whole mechanism: if a provider id or the namespace
        // put a character in the derived name that `valid_service_name`
        // refuses, every account with that provider fails at `add` with an
        // error naming a string the user never typed.
        for p in PROVIDERS {
            let service = AccountRef::new(p.id, "a-name_9").unwrap().service();
            assert!(
                valid_service_name(&service),
                "{service} is not a service name the store will take"
            );
            // And the longest name the namespace allows still fits the store's
            // own 64-byte cap, which is the bound this one is derived from.
            let longest = AccountRef::new(p.id, &"z".repeat(MAX_NAME)).unwrap().service();
            assert!(valid_service_name(&longest), "{longest} is too long to store");
        }
    }

    #[test]
    fn a_name_can_carry_neither_separator_out_of_its_own_namespace() {
        // Two separators, two different escapes, and both are refusals rather
        // than sanitisation:
        //
        //   '/' would leave the store's per-service file — the store writes one
        //       file per service name.
        //   ':' is `grant_key`'s separator (`service:capability`), so a name
        //       carrying one would describe a grant for a service that is not
        //       this account.
        //   '.' is THIS namespace's separator, so `parse` would read the tail
        //       as the name and the head as something else.
        for bad in ["a/b", "a:b", "a.b", "../x", "", "A", " a", "a b"] {
            assert!(
                AccountRef::new("webdav", bad).is_err(),
                "{bad:?} was accepted as an account name"
            );
        }
        assert!(AccountRef::new("webdav", &"z".repeat(MAX_NAME + 1)).is_err());
    }

    #[test]
    fn a_service_name_round_trips_and_a_foreign_one_is_not_an_account() {
        for p in PROVIDERS {
            let made = AccountRef::new(p.id, "home").unwrap();
            let read = AccountRef::parse(&made.service()).expect("round trip");
            assert_eq!(read, made);
        }
        // The credentials that were in this store before accounts existed.
        // Each must read as "not an account", because `apex account rm` is
        // built on this answer and a git token removed by an accounts command
        // is a credential deleted by a command that had no business naming it.
        for foreign in [
            "github",
            "claude-memory",
            "account",
            "account.",
            "accounts.webdav.home",
            "account.nosuchprovider.home",
            "account.webdav",
            "account.webdav.",
            "account.webdav.Home",
        ] {
            assert!(
                AccountRef::parse(foreign).is_none(),
                "{foreign} parsed as an account"
            );
        }
    }

    #[test]
    fn the_form_a_person_types_is_the_stored_name_without_the_prefix() {
        let typed = AccountRef::parse_ref("nextcloud.home").unwrap();
        assert_eq!(typed.service(), "account.nextcloud.home");
        // Pasting back what `apex account list` printed is the same account
        // and not a provider called `account`.
        assert_eq!(AccountRef::parse_ref("account.nextcloud.home").unwrap(), typed);
        // The refusals are three different answers, not one.
        assert!(matches!(
            AccountRef::parse_ref("home").unwrap_err(),
            AccountError::BadRef(_)
        ));
        assert!(matches!(
            AccountRef::parse_ref("nosuch.home").unwrap_err(),
            AccountError::UnknownProvider(_)
        ));
        assert!(matches!(
            AccountRef::parse_ref("nextcloud.Home").unwrap_err(),
            AccountError::BadName(_)
        ));
    }

    #[test]
    fn every_scope_routes_to_its_own_providers_transport() {
        // P1-001's rule: an operation id is `provider.class.verb` and the FIRST
        // SEGMENT routes, with no operation-to-provider table anywhere. A scope
        // whose operation id begins with a different segment is a grant that
        // sends this account's credential to another provider's code.
        for p in PROVIDERS {
            for s in p.scopes {
                let id = OperationId::parse(s.operation)
                    .unwrap_or_else(|e| panic!("{} does not parse: {e}", s.operation));
                assert_eq!(
                    id.provider(),
                    p.transport,
                    "{} routes to {} but {} declares transport {}",
                    s.operation,
                    id.provider(),
                    p.id,
                    p.transport
                );
            }
        }
    }

    #[test]
    fn nextcloud_and_webdav_are_one_transport_and_not_two() {
        // The module's load-bearing claim, as an assertion. If these ever
        // diverge, `apex-secretd` needs two identical registry entries and the
        // second one will drift from the first.
        let nc = provider("nextcloud").unwrap();
        let dav = provider("webdav").unwrap();
        assert_eq!(nc.transport, dav.transport);
        assert_eq!(nc.scopes, dav.scopes);
        // ...and they are still two providers, because the credential is
        // obtained differently and the path is not the same.
        assert_ne!(nc.id, dav.id);
        assert_ne!(nc.path, dav.path);
    }

    #[test]
    fn nextcloud_endpoints_end_in_the_account_name_and_the_others_do_not() {
        // Measured against the published shape rather than guessed: Nextcloud
        // serves files at /remote.php/dav/files/<username>/, so a credential
        // stored at the bare prefix points at a collection the server answers
        // 404 for — on every real Nextcloud, for every file, which is the kind
        // of defect that looks like a wrong password.
        let nc = provider("nextcloud").unwrap();
        assert_eq!(nc.path_for("me"), "/remote.php/dav/files/me");
        // A bare WebDAV server's path is whatever the user gave, so nothing is
        // appended and an empty default stays empty.
        assert_eq!(provider("webdav").unwrap().path_for("me"), "");
        assert_eq!(provider("google").unwrap().path_for("me"), "/drive/v3");
        // And with no username there is nothing to append; `add` refuses that
        // case for a Basic provider before it gets here, and this is the
        // belt-and-braces half.
        assert_eq!(nc.path_for(""), "/remote.php/dav/files");
    }

    #[test]
    fn a_fixed_host_provider_refuses_a_host_of_the_callers_choosing() {
        let google = provider("google").unwrap();
        assert_eq!(
            google.resolve_host(None).unwrap(),
            "www.googleapis.com".to_string()
        );
        // Passing the right one is not an error — it is a caller being
        // explicit, and refusing it would be pedantry.
        assert_eq!(
            google.resolve_host(Some("www.googleapis.com")).unwrap(),
            "www.googleapis.com".to_string()
        );
        // Passing another one is the case this exists for.
        let e = google.resolve_host(Some("evil.example")).unwrap_err();
        assert!(matches!(e, AccountError::HostFixed { .. }), "{e:?}");
        assert!(e.to_string().contains("evil.example"), "{e}");
    }

    #[test]
    fn a_self_hosted_provider_refuses_to_guess_a_host() {
        for id in ["webdav", "nextcloud", "s3"] {
            let p = provider(id).unwrap();
            assert!(p.needs_host(), "{id}");
            let e = p.resolve_host(None).unwrap_err();
            assert!(matches!(e, AccountError::HostRequired { .. }), "{id}: {e:?}");
            // An empty --host is how a shell hands over "unset", and storing a
            // credential pinned to "" would pin it to nothing.
            assert!(p.resolve_host(Some("")).is_err(), "{id} took an empty host");
            assert_eq!(
                p.resolve_host(Some("cloud.example")).unwrap(),
                "cloud.example".to_string()
            );
        }
    }

    #[test]
    fn a_scope_the_provider_does_not_have_is_refused_rather_than_ignored() {
        let a = AccountRef::new("webdav", "home").unwrap();
        assert_eq!(a.operation("files.read").unwrap(), "webdav.file.read");
        // S3 has `objects.read` and WebDAV does not. A silent miss here would
        // grant nothing and report success.
        let e = a.operation("objects.read").unwrap_err();
        assert!(matches!(e, AccountError::UnknownScope { .. }), "{e:?}");
        assert_eq!(
            AccountRef::new("s3", "backups")
                .unwrap()
                .operation("objects.read")
                .unwrap(),
            "s3.object.read"
        );
    }

    /// A provider with no scopes refuses differently, and says which.
    ///
    /// This test exists because of what it replaced. Until P2-017's round 3
    /// the line above read `AccountRef::new("microsoft", "work")
    /// .operation("mail.read").unwrap() == "msgraph.mail.read"` — a green
    /// assertion about a mapping into an operation **no provider has ever
    /// offered**. The test was true about the table and said nothing about
    /// whether the grant it produced could be spent, which is the only
    /// question that matters. `apex-secretd`'s
    /// `every_account_scope_names_an_operation_some_provider_actually_offers`
    /// is the one that can ask it; this one holds the other half — that the
    /// refusal a user now meets tells them the truth.
    ///
    /// Google has left this set. `gdrive.file.read` is an operation
    /// `apex-secretd` performs, so `google` is asserted the other way round
    /// below — which is the first time since round 3 that a positive
    /// assertion about a Google scope has been allowed to exist here.
    #[test]
    fn a_provider_with_no_transport_yet_says_so_instead_of_listing_nothing() {
        // COMPUTED from the table and then spelled out, in that order. The
        // loop runs over what the table actually says rather than over a name
        // written a second time here, and the equality is what makes a
        // provider joining or leaving this set a line somebody writes on
        // purpose. An empty set would make the loop prove nothing, which the
        // assertion catches.
        let empty: Vec<&str> = PROVIDERS
            .iter()
            .filter(|p| p.scopes.is_empty())
            .map(|p| p.id)
            .collect();
        assert_eq!(
            empty,
            vec!["microsoft"],
            "the set of account providers with nothing grantable changed"
        );
        for id in &empty {
            let a = AccountRef::new(id, "mine").unwrap();
            // `files.read` is the name somebody would reach for first, and it
            // is the exact scope that used to be accepted here.
            let e = a.operation("files.read").unwrap_err();
            assert!(
                matches!(e, AccountError::NoScopesYet { .. }),
                "{id}: {e:?} — an empty table must not read as 'not that one'"
            );
            let said = e.to_string();
            assert!(said.contains("no operation to grant"), "{id}: {said}");
            // And it must not send the reader to a command that prints a
            // header and no rows without warning them that is what it does.
            assert!(said.contains("nothing can spend the credential"), "{id}: {said}");
        }
    }

    /// The assertion round 3 had to delete, now true.
    ///
    /// `apex account grant google files.read` records a grant for
    /// `gdrive.file.read`, and — this is the half this crate cannot check —
    /// `apex-secretd`'s `providers::gdrive` performs it. The cross-crate gate
    /// `every_account_scope_names_an_operation_some_provider_actually_offers`
    /// is what holds the two together; without it this would be exactly the
    /// green-assertion-about-vapour the test above describes.
    #[test]
    fn googles_files_read_names_the_operation_the_gdrive_provider_performs() {
        let google = AccountRef::new("google", "work").unwrap();
        assert_eq!(google.operation("files.read").unwrap(), "gdrive.file.read");
        // Read, not write: `apex secret capabilities` prints this, and an
        // operation that reads a file must not be presented as one that
        // changes it.
        let scope = provider("google").unwrap().scope("files.read").unwrap();
        assert_eq!(scope.effect, Effect::Read);
        // And the scope that made the token spendable is asked for at the
        // grant. Without it the stored token carries no Drive permission and
        // every read answers 403 — a failure that arrives an hour later,
        // somewhere else.
        assert!(
            GOOGLE_OAUTH
                .scopes
                .contains(&"https://www.googleapis.com/auth/drive.file"),
            "a Drive scope is offered that the sign-in never asks for: {:?}",
            GOOGLE_OAUTH.scopes
        );
    }

    #[test]
    fn presentation_maps_onto_the_two_values_the_store_actually_has() {
        // `ServiceInfo::auth` is `bearer` or `raw` and nothing else. A provider
        // whose presentation mapped to a third string would be stored with an
        // auth the daemon does not recognise.
        for p in PROVIDERS {
            let auth = p.presentation.service_auth();
            assert!(auth == "bearer" || auth == "raw", "{}: {auth}", p.id);
        }
        assert_eq!(Presentation::Basic.service_auth(), "raw");
        assert_eq!(Presentation::SigV4.service_auth(), "raw");
        assert_eq!(Presentation::Bearer.service_auth(), "bearer");
    }

    #[test]
    fn the_criterions_five_providers_are_all_present_and_unique() {
        // P2-017's first criterion names five by name. This is that sentence
        // as a test, so deleting a provider is a failure here rather than a
        // discovery later.
        for wanted in ["nextcloud", "google", "microsoft", "webdav", "s3"] {
            assert!(provider(wanted).is_some(), "{wanted} is missing");
        }
        let mut ids: Vec<&str> = PROVIDERS.iter().map(|p| p.id).collect();
        ids.sort_unstable();
        let before = ids.len();
        ids.dedup();
        assert_eq!(before, ids.len(), "two providers share an id");

        // The operation namespaces those five route into, spelled out. It is
        // a smaller set than the providers, which is the module's claim about
        // Nextcloud and WebDAV, and `docs/online-accounts.md` states the
        // count in prose — it said "three transports" while the table held
        // four, from the round `google` and `microsoft` were added until the
        // round `gdrive` was built. A number in a document that nothing
        // computes is a number that goes stale silently.
        let mut transports: Vec<&str> = PROVIDERS.iter().map(|p| p.transport).collect();
        transports.sort_unstable();
        transports.dedup();
        assert_eq!(transports, vec!["gdrive", "msgraph", "s3", "webdav"]);
    }

    #[test]
    fn a_flow_that_needs_a_browser_says_so() {
        // P2-018 is the item next to this one: a machine may have no working
        // graphics. `apex account add` has to refuse before it starts, not
        // after opening nothing.
        assert!(Flow::Token.is_unattended());
        assert!(Flow::AppPassword.is_unattended());
        assert!(Flow::AccessKey.is_unattended());
        assert!(!Flow::AuthCode.is_unattended());
        // A device code is not unattended either — it needs a second device —
        // but it needs no browser HERE, which is the distinction that matters
        // on a machine whose compositor is down.
        assert!(!Flow::DeviceCode.is_unattended());
        // And only the OAuth flows can renew without the user.
        assert!(Flow::DeviceCode.is_refreshable());
        assert!(Flow::AuthCode.is_refreshable());
        assert!(!Flow::AppPassword.is_refreshable());
        assert!(!Flow::AccessKey.is_refreshable());
    }

    // ── the OAuth table ─────────────────────────────────────────────────────

    /// A row of the table that exists only to be broken.
    ///
    /// Every negative below is this one with a single field changed, so what
    /// each test proves is the field it changed and not the fixture.
    const SYNTHETIC: Provider = Provider {
        id: "synthetic",
        label: "Synthetic",
        transport: "synthetic",
        flow: Flow::Token,
        presentation: Presentation::Bearer,
        host: Host::Fixed("api.synthetic.example"),
        path: "",
        path_carries_username: false,
        obtain: "it does not exist",
        scopes: &[],
        oauth: None,
    };

    const SYNTHETIC_OAUTH: OAuth = OAuth {
        device_url: "https://auth.synthetic.example/device",
        token_url: "https://auth.synthetic.example/token",
        auth_host: "auth.synthetic.example",
        scopes: &["openid"],
        client_secret: ClientSecret::None,
        client_id: None,
    };

    #[test]
    fn every_shipped_provider_declares_an_authorisation_server_it_can_reach() {
        // The whole table, at the one moment the answer is cheap. The
        // alternative is finding out when a person is halfway through signing
        // in on their phone.
        //
        // The count is asserted for the reason every loop in this workspace
        // asserts one: emptying `PROVIDERS` would make this pass on nothing,
        // which is the failure this repository keeps finding in its own gates.
        let mut checked = 0;
        for p in PROVIDERS {
            p.validate()
                .unwrap_or_else(|e| panic!("the shipped '{}' row is incoherent: {e}", p.id));
            // And the pairing itself, stated rather than implied: a flow that
            // can renew has somewhere to renew AT, and one that cannot has no
            // authorisation server hanging off it pretending otherwise.
            assert_eq!(
                p.flow.is_refreshable(),
                p.oauth.is_some(),
                "'{}' says is_refreshable()={} and names {} authorisation server",
                p.id,
                p.flow.is_refreshable(),
                if p.oauth.is_some() { "an" } else { "no" }
            );
            checked += 1;
        }
        assert_eq!(
            checked,
            PROVIDERS.len(),
            "the loop did not visit every provider"
        );
        assert!(checked >= 5, "only {checked} providers were checked");

        // The two that carry one, named. Adding a third has to be a line
        // somebody writes here, because a new OAuth provider is a new place a
        // credential gets sent.
        let oauthed: Vec<&str> = PROVIDERS
            .iter()
            .filter(|p| p.oauth.is_some())
            .map(|p| p.id)
            .collect();
        assert_eq!(oauthed, vec!["google", "microsoft"]);
    }

    #[test]
    fn a_provider_that_offers_to_sign_in_must_say_where() {
        // `apex account add` reads `flow` to decide what to do. A device-code
        // provider with no authorisation server would print "sign in on
        // another device" and then have no endpoint to ask, which is a dead
        // end the user cannot act on.
        let err = Provider {
            flow: Flow::DeviceCode,
            oauth: None,
            ..SYNTHETIC
        }
        .validate()
        .unwrap_err();
        assert!(err.contains("nowhere to ask"), "{err}");

        assert!(Provider {
            flow: Flow::AuthCode,
            oauth: None,
            ..SYNTHETIC
        }
        .validate()
        .is_err());

        // And the coherent version of the same row is accepted, so this is a
        // rule about the contradiction and not a ban on the flow.
        assert_eq!(
            Provider {
                flow: Flow::DeviceCode,
                oauth: Some(&SYNTHETIC_OAUTH),
                ..SYNTHETIC
            }
            .validate(),
            Ok(())
        );
    }

    #[test]
    fn a_flow_that_cannot_renew_may_not_name_an_authorisation_server() {
        // The other direction, and it is not symmetry for its own sake: an
        // app-password provider carrying a token endpoint is a token endpoint
        // nothing will ever POST to, and the next person to read the table
        // will believe that account refreshes.
        for flow in [Flow::Token, Flow::AppPassword, Flow::AccessKey] {
            let err = Provider {
                flow,
                oauth: Some(&SYNTHETIC_OAUTH),
                ..SYNTHETIC
            }
            .validate()
            .unwrap_err();
            assert!(err.contains("nothing would ever use it"), "{flow:?}: {err}");
        }
    }

    #[test]
    fn the_authorisation_host_may_not_be_the_api_host_it_guards() {
        // The property `apex cloudflare connect` invented and this table
        // carries across: the framework pins a credential to the host it was
        // stored for, so a refresh token filed under the AUTH host cannot be
        // presented to the API. Collapse the two hosts and that protection is
        // gone — silently, because everything still compiles and every request
        // still works.
        const SAME: OAuth = OAuth {
            device_url: "https://api.synthetic.example/device",
            token_url: "https://api.synthetic.example/token",
            auth_host: "api.synthetic.example",
            scopes: &[],
            client_secret: ClientSecret::None,
            client_id: None,
        };
        let err = Provider {
            flow: Flow::DeviceCode,
            oauth: Some(&SAME),
            ..SYNTHETIC
        }
        .validate()
        .unwrap_err();
        assert!(err.contains("silently give up that property"), "{err}");

        // The shipped rows really are in the safe shape, checked against the
        // API host each one declares rather than against a list written here.
        for p in PROVIDERS {
            if let (Host::Fixed(api), Some(oauth)) = (p.host, p.oauth) {
                assert_ne!(
                    api.to_ascii_lowercase(),
                    oauth.auth_host.to_ascii_lowercase(),
                    "'{}' stores its access token and its refresh token on one host",
                    p.id
                );
            }
        }
    }

    #[test]
    fn an_endpoint_off_the_declared_authorisation_host_is_refused() {
        // `oauth_for_auth_host` is the daemon's ONLY way to find a token
        // endpoint, and it looks one up by the host the credential is already
        // pinned to. A `token_url` that is not on `auth_host` would therefore
        // be unroutable — the lookup would succeed and hand back an endpoint
        // the pin then refuses — so the mismatch is refused here instead.
        for (device, token) in [
            ("https://elsewhere.example/device", "https://auth.synthetic.example/token"),
            ("https://auth.synthetic.example/device", "https://elsewhere.example/token"),
            // The prefix check has to be on `https://host/` and not on the
            // bare host, or `auth.synthetic.example.attacker.example` passes.
            (
                "https://auth.synthetic.example.attacker.example/device",
                "https://auth.synthetic.example/token",
            ),
            // And http is not https.
            ("http://auth.synthetic.example/device", "https://auth.synthetic.example/token"),
        ] {
            let oauth = OAuth {
                device_url: device,
                token_url: token,
                ..SYNTHETIC_OAUTH
            };
            // `&'static` is what the field wants; a test-local leak is the
            // cheapest honest way to get one and the process is about to end.
            let leaked: &'static OAuth = Box::leak(Box::new(oauth));
            let err = Provider {
                flow: Flow::DeviceCode,
                oauth: Some(leaked),
                ..SYNTHETIC
            }
            .validate()
            .unwrap_err();
            assert!(err.contains("not on its declared"), "{device} / {token}: {err}");
        }
    }

    #[test]
    fn a_refresh_is_routed_by_the_host_its_credential_is_already_pinned_to() {
        // The one property that makes a daemon-side refresh safe: the token
        // endpoint is not taken from the caller and not taken from the
        // credential's contents, it is looked up from the host the credential
        // was stored for. There is no request shape that can aim a refresh
        // somewhere else, because the caller never names a host at all.
        let cf = oauth_for_auth_host("dash.cloudflare.com").expect("Cloudflare is in the table");
        assert_eq!(cf.token_url, "https://dash.cloudflare.com/oauth2/token");
        assert_eq!(cf.client_id, Some(WRANGLER_CLIENT_ID));

        assert_eq!(
            oauth_for_auth_host("oauth2.googleapis.com").map(|o| o.token_url),
            Some("https://oauth2.googleapis.com/token")
        );
        assert_eq!(
            oauth_for_auth_host("login.microsoftonline.com").map(|o| o.auth_host),
            Some("login.microsoftonline.com")
        );

        // A host is matched case-insensitively and with the root label
        // stripped, because a `ServiceInfo` host comes from a person typing
        // one and both spellings name the same server.
        assert!(oauth_for_auth_host("DASH.Cloudflare.COM").is_some());
        assert!(oauth_for_auth_host("dash.cloudflare.com.").is_some());
        assert!(oauth_for_auth_host("  dash.cloudflare.com  ").is_some());

        // And the ones that must NOT resolve. `api.cloudflare.com` is the
        // headline: an access token pinned there must never find a token
        // endpoint, or the refresh path becomes a second way to spend it.
        for miss in [
            "api.cloudflare.com",
            "www.googleapis.com",
            "graph.microsoft.com",
            "dash.cloudflare.com.attacker.example",
            "attacker.example",
            "",
            "127.0.0.1",
        ] {
            assert!(
                oauth_for_auth_host(miss).is_none(),
                "'{miss}' found an authorisation server"
            );
        }

        // Every row is reachable through the lookup, so a row added to the
        // table and not to `OAUTH` cannot sit there looking registered.
        for oauth in OAUTH {
            assert_eq!(
                oauth_for_auth_host(oauth.auth_host).map(|o| o.token_url),
                Some(oauth.token_url),
                "'{}' is in the table and the lookup does not find it",
                oauth.auth_host
            );
        }
        assert_eq!(OAUTH.len(), 3);
    }

    #[test]
    fn a_refresh_token_s_name_does_not_parse_back_as_an_account() {
        // The second load-bearing property of `refresh_service`. It is a name
        // in the same flat namespace as the account's own, so if it parsed
        // back as an account it would appear in `apex account list`, could be
        // granted a scope, and could be handed to `apex account rm` directly —
        // which is a way to point account commands at the credential that
        // renews every other one.
        //
        // `.` is not legal in an account name, which is what makes this true;
        // the test is here so a future loosening of `AccountRef::new` cannot
        // quietly make the refresh token addressable.
        let account = AccountRef::new("google", "home").unwrap();
        let refresh = account.refresh_service();
        assert_eq!(refresh, "account.google.home.refresh");
        assert!(
            AccountRef::parse(&refresh).is_none(),
            "'{refresh}' parses back as an account"
        );
        // It is still a name the store will take — a property that has to hold
        // or the credential cannot be filed at all.
        assert!(valid_service_name(&refresh));
        assert_ne!(refresh, account.service());

        // And the longest account the namespace allows still yields a service
        // name inside the store's 64-byte cap once `.refresh` is on the end.
        // This is the bound `every_provider_derives_a_service_name_the_store_
        // accepts` checks for the account itself, re-checked for the sibling
        // that is seven bytes longer.
        for p in PROVIDERS {
            let longest = AccountRef::new(p.id, &"z".repeat(MAX_NAME))
                .unwrap()
                .refresh_service();
            assert!(
                valid_service_name(&longest),
                "{longest} is too long to store"
            );
        }
    }

    #[test]
    fn a_provider_that_needs_a_client_secret_says_which_step_needs_it() {
        // Three states and not a bool, because they send `apex account add`
        // somewhere different. Google refuses the poll without one; Microsoft
        // and Cloudflare are public clients.
        assert_eq!(GOOGLE_OAUTH.client_secret, ClientSecret::RequiredToObtain);
        assert_eq!(MICROSOFT_OAUTH.client_secret, ClientSecret::None);
        assert_eq!(CLOUDFLARE_OAUTH.client_secret, ClientSecret::None);

        // APEX ships no registered application at Google or Microsoft and will
        // not borrow one, so `--client-id` is required there. Cloudflare is the
        // deliberate exception, and the exception is Wrangler's id, which
        // `apex cf connect` says out loud on the consent screen.
        assert_eq!(GOOGLE_OAUTH.client_id, None);
        assert_eq!(MICROSOFT_OAUTH.client_id, None);
        assert_eq!(CLOUDFLARE_OAUTH.client_id, Some(WRANGLER_CLIENT_ID));

        // `offline_access` is what makes Microsoft return a refresh token at
        // all. Without it the flow this table exists to serve cannot run.
        assert!(MICROSOFT_OAUTH.scopes.contains(&"offline_access"));
    }

    #[test]
    fn the_name_a_refresh_credential_renews_is_its_own_with_the_suffix_taken_off() {
        // `AccountRef::refresh_service`'s inverse. It is the daemon's ONLY way
        // to know what an RFC 6749 §6 refresh is renewing — `bind` may not take
        // a name from the caller — so a round trip is the property, not an
        // example.
        for account in ["account.google.work", "account.nextcloud.home"] {
            let refresh = format!("{account}.refresh");
            assert_eq!(renewed_service(&refresh).as_deref(), Some(account));
        }
        // The CLI's older spelling, which predates accounts and cannot be
        // renamed without orphaning a stored token.
        assert_eq!(renewed_service("cloudflare-refresh").as_deref(), Some("cloudflare"));

        // A name that carries neither suffix renews nothing. Guessing one
        // would be inventing a credential to overwrite.
        for plain in ["cloudflare", "account.google.work", "refresh", ""] {
            assert_eq!(renewed_service(plain), None, "'{plain}' named something to renew");
        }
        // ...and neither does a name that is ONLY a suffix. The stem would be
        // empty, and an empty service name is not a credential — without the
        // filter these would name `""` and a replace of `""` would be checked
        // against the store rather than refused here.
        for bare in [".refresh", "-refresh"] {
            assert_eq!(renewed_service(bare), None, "'{bare}' named something to renew");
        }

        // Every suffix in the table works, so adding one cannot half-land.
        for suffix in REFRESH_SUFFIXES {
            assert_eq!(renewed_service(&format!("demo{suffix}")).as_deref(), Some("demo"));
        }
    }

    #[test]
    fn the_operation_that_renews_a_token_is_named_once() {
        // Spelled in this crate because `apex` (the CLI, which asks for it) and
        // `apex-secretd` (the provider, which offers it) both link this one and
        // neither links the other. A test in `apex-secretd::providers` holds
        // the registry to it.
        assert_eq!(REFRESH_OPERATION, "oauth.token.refresh");
        // A §13.2 operation id: `<provider>.<noun>.<verb>`, lowercase.
        let parts: Vec<&str> = REFRESH_OPERATION.split('.').collect();
        assert_eq!(parts.len(), 3, "{REFRESH_OPERATION}");
        assert_eq!(parts[0], "oauth", "the provider half names the provider");
    }
}
