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

/// Google Drive: **none**, and the reason is a property of the flow rather
/// than a gap somebody forgot to fill.
///
/// This table used to name `gdrive.file.list`, `gdrive.file.read` and
/// `gdrive.file.write`. No provider in `apex-secretd` offers any of them, so
/// `apex account grant google files.read` recorded a grant for an operation
/// that could never be performed — a permission the user was told they had
/// made. [`crate::account::PROVIDERS`] is now checked against the shipped
/// registry by a test in `apex-secretd`, which is the only crate that can see
/// both.
///
/// It is empty rather than repointed because of what Google's limited-input
/// flow allows, read from
/// `developers.google.com/identity/protocols/oauth2/limited-input-device`
/// rather than from memory: the device grant accepts **only** `email`,
/// `openid`, `profile`, `drive.appdata`, `drive.file`, `youtube` and
/// `youtube.readonly`. Full `drive` and `drive.readonly` are not on the list.
/// `drive.file` sees only files the calling app itself created or the user
/// individually picked — so "list the files in a folder on this Drive" is not
/// a thing a device-code Google account can do **at all**, whatever transport
/// is written for it. A `gdrive` provider is worth building against
/// `drive.file` for APEX's own files; it is not worth pretending it can read
/// the user's Drive.
const DRIVE_SCOPES: &[Scope] = &[];

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
    #[test]
    fn a_provider_with_no_transport_yet_says_so_instead_of_listing_nothing() {
        for id in ["google", "microsoft"] {
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
}
