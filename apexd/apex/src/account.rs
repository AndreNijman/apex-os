//! `apex account` — online accounts, as a provider-aware front-end over the
//! credential store (roadmap P2-017).
//!
//! ```text
//! apex account providers
//! apex account scopes nextcloud
//! apex account add nextcloud.home --host cloud.example   # credential on stdin
//! apex account list
//! apex account grant nextcloud.home files.read
//! apex account rm nextcloud.home
//! ```
//!
//! ## What this adds over `apex secret`, and what it deliberately does not
//!
//! Every verb here is a `apex-secretd` request the daemon already served
//! before this file existed: `Add`, `List`, `Grant`, `Remove`. **There is no
//! second store and no second write path**, which is the point rather than an
//! implementation detail — P2-016 measured what a second path into account
//! state costs, and one store means one `SO_PEERCRED` check on every verb.
//!
//! What it adds is the thing a person cannot be expected to hold in their
//! head:
//!
//! * `apex secret add nextcloud-home --host cloud.example --auth raw
//!   --username me --path /remote.php/dav/files` is the same command as
//!   `apex account add nextcloud.home --host cloud.example --username me`.
//!   The provider table knows the path and the presentation.
//! * A scope is `files.read`; the grant is `webdav.file.read`. The user should
//!   not have to know that Nextcloud routes through the WebDAV transport.
//! * `list` shows accounts and not the user's git token, because
//!   `AccountRef::parse` says which stored services are accounts.
//! * `rm` refuses a service that is not an account, so an accounts command
//!   cannot delete a credential it had no business naming.
//!
//! ## `Flow::DeviceCode`, and the half of it that is still the user's problem
//!
//! Google and Microsoft sign in through RFC 8628 now: `apex account add
//! google.work --client-id <id>` prints a code, the user approves it on a
//! device that has a browser, and both tokens land in the same store — the
//! access token pinned to the provider's API host, the refresh token under a
//! separate name pinned to the authorisation host, where the endpoint pin
//! makes it unspendable as an API token. The transport is
//! [`crate::oauth_device`], shared with `apex cf connect`.
//!
//! `--client-id` is **required**, and that is the honest part rather than an
//! oversight. APEX registers no OAuth application at either provider and will
//! not borrow another project's, so there is no default to fall back to; see
//! [`account::OAuth::client_id`] for the whole argument. Google also wants a
//! `client_secret`, which is read from stdin and never from argv.
//!
//! What a token obtained this way can be spent on today is **nothing**. No
//! `gdrive` or `msgraph` transport exists in `apex-secretd`, so every scope in
//! [`account::DRIVE_SCOPES`] and [`account::GRAPH_SCOPES`] names an operation
//! no provider offers — `apex account grant` refuses them for exactly that
//! reason. Signing in, storing, and renewing work end to end; using is the
//! next transport, not the next flag.

use std::io::Read;

use anyhow::{bail, Context, Result};
use apex_secret_core::account::{
    self, AccountError, AccountRef, ClientSecret, Flow, OAuth, Presentation, Provider,
};
use apex_secret_core::capability::CapabilityRecord;
use apex_secret_core::client::Client;
use apex_secret_core::protocol::{ErrorKind, Request, Response};
use apex_secret_core::store::ServiceInfo;
use apex_secret_core::SecretValue;
use clap::Subcommand;

use crate::oauth_device::{self, device_grant, Endpoints, OAuthClient, Prompt};

/// `apex account <verb>`.
#[derive(Subcommand)]
pub enum AccountCmd {
    /// The cloud providers APEX can hold an account with.
    ///
    /// Needs no daemon and no network: it is a table compiled into this
    /// build, so it answers on a machine whose secret service is not running
    /// and on one with no network at all.
    Providers {
        #[arg(long)]
        json: bool,
    },
    /// What an account with that provider can be granted.
    Scopes {
        /// `nextcloud`, `s3`, … — one of `apex account providers`.
        provider: String,
        #[arg(long)]
        json: bool,
    },
    /// Add an account. The credential is read from stdin, never from argv.
    ///
    /// argv is world-readable through `/proc/<pid>/cmdline` for as long as the
    /// command runs, and it is in the shell history afterwards. The same rule
    /// `apex secret add` follows, for the same reason.
    Add {
        /// `<provider>.<name>`, as in `nextcloud.home`.
        account: String,
        /// Your server. Required for a self-hosted provider; refused for one
        /// whose host is fixed, because pinning a Google credential to a host
        /// of the caller's choosing would pin it somewhere Google is not.
        #[arg(long)]
        host: Option<String>,
        /// The account name on that server. An app password is Basic auth, so
        /// this half is not optional for a WebDAV provider.
        #[arg(long, default_value = "")]
        username: String,
        /// Override the endpoint path the provider table gives.
        #[arg(long)]
        path: Option<String>,
        /// Port, when the endpoint is not on the scheme's own.
        #[arg(long)]
        port: Option<u16>,
        /// The OAuth client to sign in as, for a device-code provider.
        ///
        /// Required for Google and Microsoft, because APEX registers no
        /// application at either and will not sign your account in under
        /// somebody else's. A client id is public — it is on the consent
        /// screen you are about to read — so it may be typed here. A client
        /// SECRET may not, and is read from stdin.
        #[arg(long)]
        client_id: Option<String>,
    },
    /// The accounts this user has. Never prints a credential.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Allow one scope for the current project.
    Grant {
        /// `<provider>.<name>`.
        account: String,
        /// One of `apex account scopes <provider>`.
        scope: String,
    },
    /// Withdraw one.
    Revoke {
        account: String,
        scope: String,
    },
    /// Renew an account's access token with the refresh token beside it.
    ///
    /// Spends a credential, so it is a capability like any other: granted per
    /// project, and `add` does not grant it. Nothing here ever sees either
    /// token — the daemon performs the request and replaces both.
    Refresh {
        account: String,
    },
    /// Remove an account: the credential, and every grant that named it.
    Rm {
        account: String,
    },
}

pub fn main(cmd: AccountCmd) -> i32 {
    match run(cmd) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("apex account: {e:#}");
            1
        }
    }
}

fn run(cmd: AccountCmd) -> Result<i32> {
    match cmd {
        AccountCmd::Providers { json } => providers(json),
        AccountCmd::Scopes { provider, json } => scopes(&provider, json),
        AccountCmd::Add {
            account,
            host,
            username,
            path,
            port,
            client_id,
        } => add(
            &account,
            host.as_deref(),
            &username,
            path.as_deref(),
            port,
            client_id.as_deref(),
        ),
        AccountCmd::List { json } => list(json),
        AccountCmd::Grant { account, scope } => grant(&account, &scope, false),
        AccountCmd::Revoke { account, scope } => grant(&account, &scope, true),
        AccountCmd::Refresh { account } => refresh(&account),
        AccountCmd::Rm { account } => rm(&account),
    }
}

fn providers(json: bool) -> Result<i32> {
    if json {
        let rows: Vec<serde_json::Value> = account::PROVIDERS
            .iter()
            .map(|p| {
                serde_json::json!({
                    "id": p.id,
                    "label": p.label,
                    "transport": p.transport,
                    "flow": p.flow.as_str(),
                    "unattended": p.flow.is_unattended(),
                    "refreshable": p.flow.is_refreshable(),
                    "presentation": p.presentation.as_str(),
                    "needsHost": p.needs_host(),
                    "obtain": p.obtain,
                    "scopes": p.scopes.iter().map(|s| s.name).collect::<Vec<_>>(),
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(0);
    }
    println!("{:<12} {:<20} {:<14} HOST", "PROVIDER", "NAME", "CREDENTIAL");
    for p in account::PROVIDERS {
        println!(
            "{:<12} {:<20} {:<14} {}",
            p.id,
            p.label,
            p.flow.as_str(),
            p.host_display()
        );
    }
    println!();
    println!("apex account scopes <provider>   what an account there can be granted");
    Ok(0)
}

fn scopes(id: &str, json: bool) -> Result<i32> {
    let p = look_up(id)?;
    if json {
        let rows: Vec<serde_json::Value> = p
            .scopes
            .iter()
            .map(|s| {
                serde_json::json!({
                    "scope": s.name,
                    "operation": s.operation,
                    "effect": s.effect.as_str(),
                    "summary": s.summary,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(0);
    }
    // A header with no rows under it reads as "nothing matched your filter",
    // and there is no filter. Say the actual reason, which is that the
    // credential can be held and cannot yet be spent.
    if p.scopes.is_empty() {
        println!(
            "{} accounts have no grantable scopes in this build.",
            p.label
        );
        println!();
        println!("The credential can be stored and refreshed — `apex account add {}.<name>`", p.id);
        println!("puts it in the root-owned store and keeps it renewed — but there is no");
        println!("{} transport in apex-secretd yet, so no operation can spend it.", p.transport);
        println!();
        println!("This is checked rather than described: apex-secretd's");
        println!("every_account_scope_names_an_operation_some_provider_actually_offers");
        println!("fails the build if a scope here names an operation no provider offers.");
        return Ok(0);
    }
    println!("{:<16} {:<22} {:<6} SUMMARY", "SCOPE", "OPERATION", "EFFECT");
    for s in p.scopes {
        println!(
            "{:<16} {:<22} {:<6} {}",
            s.name,
            s.operation,
            s.effect.as_str(),
            s.summary
        );
    }
    Ok(0)
}

/// A provider by id, with a refusal that lists the ones there are.
fn look_up(id: &str) -> Result<&'static Provider> {
    account::provider(id).ok_or_else(|| {
        let known: Vec<&str> = account::PROVIDERS.iter().map(|p| p.id).collect();
        anyhow::anyhow!(
            "{}\nknown providers: {}",
            AccountError::UnknownProvider(id.to_string()),
            known.join(", ")
        )
    })
}

fn add(
    reference: &str,
    host: Option<&str>,
    username: &str,
    path: Option<&str>,
    port: Option<u16>,
    client_id: Option<&str>,
) -> Result<i32> {
    let account = AccountRef::parse_ref(reference)?;
    let provider = account.provider;
    let host = provider.resolve_host(host.map(str::trim))?.to_ascii_lowercase();

    if provider.flow == Flow::DeviceCode {
        return add_by_device_code(reference, &account, &host, username, path, port, client_id);
    }
    // Refused rather than ignored: a flag that did nothing would read as a
    // client id having been recorded, and the credential would be renewed —
    // or not — under a client the user never chose.
    if client_id.is_some() {
        bail!(
            "--client-id belongs to an OAuth sign-in and {} uses the {} flow,              which has no OAuth client",
            provider.label,
            provider.flow.as_str()
        );
    }

    // Basic auth is a PAIR. Stored without the username half, the provider
    // falls back to sending the value as a bare `Authorization:` — which the
    // server answers with 401, so a forgotten flag reads back as a wrong
    // password and sends the user to reset a credential that was fine. Refused
    // before stdin is read, so nothing is stored and nothing is half-stored.
    if provider.presentation == Presentation::Basic && username.trim().is_empty() {
        bail!(
            "a {} account signs in with a name and a password, so --username is \
             required; without it APEX would send the password on its own and \
             the server would answer 401",
            provider.label
        );
    }

    let mut value = String::new();
    std::io::stdin().read_to_string(&mut value)?;
    let value = value.trim();
    if value.is_empty() {
        bail!(
            "nothing on stdin. Pipe the credential in:\n  \
             printf %s \"$TOKEN\" | apex account add {reference}{}\n  \
             get one from: {}",
            if provider.needs_host() {
                format!(" --host {host}")
            } else {
                String::new()
            },
            provider.obtain
        );
    }

    // The provider's own path, completed with the account name where the
    // provider says its endpoint ends in one — Nextcloud's does. `--path`
    // still wins: a deployment behind a reverse proxy can have any prefix.
    let path = match path {
        Some(p) => p.to_string(),
        None => provider.path_for(username.trim()),
    };
    let service = account.service();
    Client::connect()?.add(
        &service,
        &host,
        "https",
        Some(username),
        &path,
        provider.presentation.service_auth(),
        port,
        &SecretValue::new(value.as_bytes().to_vec()),
    )?;
    println!("stored {reference} (https://{host}{path})");
    println!("nothing is allowed yet. What this account can be granted:");
    println!("  apex account scopes {}", provider.id);
    println!("allow one for this project:");
    println!("  apex account grant {reference} <scope>");
    Ok(0)
}

/// Which OAuth client to sign in as, or why there is none to fall back to.
///
/// The table's own `client_id` when this build ships one — Cloudflare's entry
/// does and it is Wrangler's, with that decision written down where a person
/// reads it. Google's and Microsoft's are `None`, which does not mean "not
/// filled in yet": APEX registers no application at either and will not put
/// another project's credential in this binary or another project's name on
/// the consent screen its users approve. So the refusal is the honest answer
/// and it says what to do about it.
fn client_for(provider: &Provider, oauth: &OAuth, asked: Option<&str>) -> Result<String> {
    if let Some(given) = asked.map(str::trim).filter(|g| !g.is_empty()) {
        // Checked here so a client id that cannot go in a form body is refused
        // before a device code is issued against it, rather than after.
        if !oauth_device::valid_form_value(given) {
            bail!("'{}' is not an OAuth client id", given.escape_debug());
        }
        return Ok(given.to_string());
    }
    let Some(shipped) = oauth.client_id else {
        bail!(
            "--client-id is required for {}. APEX registers no OAuth application at \
             {} and will not sign your account in under another project's — that \
             would put their credential in this binary and their name on the \
             consent screen you approve. Register a client of your own (the \
             \"limited-input device\" or \"device code\" kind) and pass its id.{}",
            provider.label,
            oauth.auth_host,
            match oauth.client_secret {
                ClientSecret::RequiredToObtain =>
                    " That server issues a client secret with it and wants it on the \
                     poll; pipe the secret in on stdin.",
                ClientSecret::None => "",
            }
        );
    };
    Ok(shipped.to_string())
}

/// Sign in by RFC 8628 and file both halves of what comes back.
///
/// The two credentials are stored under different names pinned to different
/// hosts, which is `apex cloudflare connect`'s idea carried across rather than
/// reinvented: the framework refuses a request whose endpoint is not the host
/// its credential was stored for, so a refresh token filed under the
/// authorisation host cannot be presented to the API as an access token, and
/// granting an agent every scope on the account still cannot reach it.
fn add_by_device_code(
    reference: &str,
    account: &AccountRef,
    host: &str,
    username: &str,
    path: Option<&str>,
    port: Option<u16>,
    client_id: Option<&str>,
) -> Result<i32> {
    let provider = account.provider;
    let Some(oauth) = provider.oauth else {
        // `Provider::validate` refuses this combination at startup and a test
        // runs it over the shipped table, so reaching it means the table
        // changed without the gate. Answered rather than unwrapped, because
        // the person running the command did nothing wrong.
        bail!(
            "{} signs in by device code and this build names no authorisation \
             server for it, so there is nowhere to ask. That is a defect in the \
             provider table, not in what you typed.",
            provider.label
        );
    };
    let client_id = client_for(provider, oauth, client_id)?;

    let secret = match oauth.client_secret {
        // From stdin, never argv: argv is world-readable through
        // `/proc/<pid>/cmdline` while the command runs and is in the shell
        // history afterwards. The same rule `apex secret add` follows, and the
        // reason there is no `--client-secret` flag to find.
        ClientSecret::RequiredToObtain => {
            let mut value = String::new();
            std::io::stdin()
                .read_to_string(&mut value)
                .context("reading the client secret from stdin")?;
            let value = value.trim().to_string();
            if value.is_empty() {
                bail!(
                    "nothing on stdin. {} wants the client secret issued with that \
                     client id, so pipe it in:\n  \
                     printf %s \"$CLIENT_SECRET\" | apex account add {reference} \
                     --client-id {client_id}",
                    provider.label
                );
            }
            if !oauth_device::valid_form_value(&value) {
                bail!(
                    "that client secret has characters this build will not put in a \
                     form body. Paste the secret on its own, with no surrounding \
                     quotes or JSON"
                );
            }
            Some(value)
        }
        // NOT read, and this is the branch that matters for a person at a
        // terminal. A public client has no secret to send, and
        // `read_to_string` on a tty waits for an end of file nobody has a
        // reason to type — the command would look hung before it ever printed
        // the code they are waiting for.
        ClientSecret::None => None,
    };

    // Before the flow, not after. A daemon that is not answering should be
    // found out now rather than once somebody has approved a code on their
    // phone for a token this cannot store.
    let mut client = Client::connect()?;

    let retry = match oauth.client_secret {
        // The secret comes from stdin, so the command to run again has to
        // carry the pipe or it is a command that will stop and wait.
        ClientSecret::RequiredToObtain => format!(
            "printf %s \"$CLIENT_SECRET\" | apex account add {reference} --client-id {client_id}"
        ),
        ClientSecret::None => format!("apex account add {reference} --client-id {client_id}"),
    };
    let grant = device_grant(
        &Endpoints::for_oauth(oauth),
        &OAuthClient {
            id: &client_id,
            secret: secret.as_deref(),
        },
        // The table's list, asked for and no more — §13.5 at the only moment
        // it can be applied, because a token's scopes are fixed when it is
        // issued. Not the caller's: a flag here would let a grant be widened
        // by whoever typed the command.
        oauth.scopes,
        &Prompt {
            label: provider.label,
            retry: &retry,
        },
        &mut std::io::stdout(),
    )?;

    let path = match path {
        Some(p) => p.to_string(),
        None => provider.path_for(username.trim()),
    };
    client.add(
        &account.service(),
        host,
        "https",
        Some(username),
        &path,
        provider.presentation.service_auth(),
        port,
        &SecretValue::new(grant.access_token.into_bytes()),
    )?;
    println!("stored {reference} (https://{host}{path})");
    if let Some(seconds) = grant.expires_in {
        println!("it stops working in about {}.", oauth_device::roughly(seconds));
    }

    match &grant.refresh_token {
        Some(refresh) => {
            let filed = refresh_record(account, oauth, &client_id);
            client.add(
                &filed.service,
                filed.host,
                "https",
                Some(&filed.username),
                "",
                "bearer",
                None,
                &SecretValue::new(refresh.as_bytes().to_vec()),
            )?;
            println!(
                "stored its refresh token for {}, where it cannot be spent as an \
                 access token",
                oauth.auth_host
            );
            println!("renew it without signing in again, once per project:");
            println!(
                "  apex secret grant {} {}",
                account.refresh_service(),
                account::REFRESH_OPERATION
            );
            println!("  apex account refresh {reference}");
            if oauth.client_secret == ClientSecret::RequiredToObtain {
                // Said now rather than discovered at renewal. Google lists the
                // secret as optional on the refresh and required on the poll,
                // and this build has nowhere to keep one — so if it turns out
                // to want it, the renewal answers `invalid_client`, nothing is
                // replaced, and signing in again is the way through.
                println!(
                    "note: {} may also want the client secret when renewing, and this",
                    provider.label
                );
                println!("      build stores no secret. If a renewal answers `invalid_client`,");
                println!("      nothing was replaced — sign in again with the same command.");
            }
        }
        None => {
            println!(
                "this grant carried no refresh token, so it cannot be renewed — sign \
                 in again when it expires"
            );
        }
    }

    // Honest rather than encouraging. The cross-crate scope gate refuses a
    // grant for an operation no provider offers, and no `gdrive` or `msgraph`
    // transport exists, so sending somebody to `apex account grant` here would
    // send them to a refusal they did not earn.
    println!(
        "nothing can spend it yet: this build ships no {} transport, so every scope",
        provider.transport
    );
    println!(
        "`apex account scopes {}` lists names an operation no provider offers.",
        provider.id
    );
    Ok(0)
}

/// Where the refresh half of a device-code grant is filed.
///
/// A struct rather than three arguments written out at the store call,
/// because the store call cannot be tested from here — it needs the daemon —
/// and these three values are the whole of what makes a renewal possible.
struct RefreshRecord {
    service: String,
    host: &'static str,
    /// **The OAuth client the grant was issued to.** RFC 6749 §6 requires a
    /// refresh to present the same client, and `apex-secretd`'s `oauth`
    /// provider reads it out of [`ServiceInfo::username`] — the store's field
    /// for the half of a credential that is not a secret, which an OAuth
    /// client id is by definition. Nothing wrote it for these two providers
    /// before, and that, in one field, is why the daemon refused to renew
    /// them: its module note says the day something records it here, the
    /// refresh starts working with no change there.
    username: String,
}

fn refresh_record(account: &AccountRef, oauth: &OAuth, client_id: &str) -> RefreshRecord {
    RefreshRecord {
        service: account.refresh_service(),
        // The authorisation host, never the API host. The framework refuses a
        // request whose endpoint is not the host its credential was stored
        // for, so this pin is what stops a refresh token being spent as an
        // access token however the account is granted.
        host: oauth.auth_host,
        username: client_id.to_string(),
    }
}

/// The request `apex account refresh` sends, as a value a test can read.
///
/// The `Use` itself needs the daemon; what it asks for does not, and what it
/// asks for is the part that can silently stop matching — a refresh filed
/// under one name and requested under another fails as "not granted".
fn refresh_request(account: &AccountRef, project: String) -> CapabilityRecord {
    let mut record =
        CapabilityRecord::new(&account.refresh_service(), account::REFRESH_OPERATION, "");
    // Per project, like every other capability, and `add` does not write it:
    // storing a credential and deciding which project may spend it are two
    // decisions, and only the first one is "sign in".
    record.project = Some(project);
    record
}

/// Renew an account's access token with the refresh token beside it.
fn refresh(reference: &str) -> Result<i32> {
    let account = AccountRef::parse_ref(reference)?;
    if !account.provider.flow.is_refreshable() {
        bail!(
            "a {} account signs in with {}, which yields nothing to renew. Replace \
             the credential instead:\n  apex account add {reference}",
            account.provider.label,
            account.provider.flow.as_str()
        );
    }
    let record = refresh_request(&account, crate::secret::current_project_root()?);
    let reply = Client::connect()?.request(&Request::Use {
        record: Box::new(record),
        body_len: 0,
    })?;
    match reply {
        // A non-zero exit code is the authorisation server refusing, which is
        // not this command failing to run: the output carries `invalid_grant`
        // or `invalid_client`, and that is the only thing that says whether the
        // grant was revoked, the client was wrong, or the token had already
        // been rotated.
        Response::Performed {
            exit_code, output, ..
        } => {
            println!("{}", output.trim_end());
            if exit_code != 0 {
                println!("sign in again to get a new one, the same way you did before:");
                println!("  apex account add {reference} --client-id <id>");
                if account.provider.oauth.map(|o| o.client_secret)
                    == Some(ClientSecret::RequiredToObtain)
                {
                    println!("  ...with the client secret on stdin, as {} requires.", account.provider.label);
                }
            }
            Ok(exit_code)
        }
        Response::Error { message, .. } => bail!("{message}"),
        other => bail!("the secret service answered a refresh with {}", other.variant()),
    }
}

/// Every stored service that is an account, with its provider resolved.
fn stored_accounts(client: &mut Client) -> Result<Vec<(AccountRef, ServiceInfo)>> {
    let services = match client.call(&Request::List)? {
        Response::Services { services } => services,
        other => bail!("unexpected reply: {}", other.variant()),
    };
    Ok(services
        .into_iter()
        .filter_map(|i| AccountRef::parse(&i.service).map(|a| (a, i)))
        .collect())
}

fn list(json: bool) -> Result<i32> {
    let mut client = Client::connect()?;
    let rows = stored_accounts(&mut client)?;
    if json {
        let out: Vec<serde_json::Value> = rows
            .iter()
            .map(|(a, i)| {
                serde_json::json!({
                    "account": format!("{}.{}", a.provider.id, a.name),
                    "provider": a.provider.id,
                    "service": i.service,
                    "endpoint": format!("{}://{}{}", i.scheme, i.host, i.path),
                    "username": i.username,
                    "refreshable": a.provider.flow.is_refreshable(),
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(0);
    }
    if rows.is_empty() {
        println!("no online accounts");
        println!("  apex account providers");
        return Ok(0);
    }
    println!("{:<22} {:<12} {:<34} USERNAME", "ACCOUNT", "PROVIDER", "ENDPOINT");
    for (a, i) in &rows {
        println!(
            "{:<22} {:<12} {:<34} {}",
            format!("{}.{}", a.provider.id, a.name),
            a.provider.id,
            format!("{}://{}{}", i.scheme, i.host, i.path),
            i.username
        );
    }
    Ok(0)
}

fn grant(reference: &str, scope: &str, revoke: bool) -> Result<i32> {
    let account = AccountRef::parse_ref(reference)?;
    let operation = account.operation(scope)?;
    // Per project, always. An account's scopes name a resource the caller
    // supplies — a path, a bucket — so `--everywhere` would be a different
    // permission in every directory. `apex secret grant --everywhere` exists
    // for an operation whose provider declares it reaches the same thing
    // wherever it is asked, and no file operation does.
    Client::connect()?.call(&Request::Grant {
        project: crate::secret::current_project_root()?,
        service: account.service(),
        capability: operation.to_string(),
        revoke,
    })?;
    let verb = if revoke { "withdrew" } else { "allowed" };
    println!("{verb} {scope} ({operation}) for {reference} in this project");
    Ok(0)
}

/// Every stored name `apex account rm` has to remove.
///
/// Two, and the second is the one that cannot be reached any other way — so
/// this is a function a test can read rather than two calls buried in `rm`.
/// `add` and `rm` deriving the same name from the same account is the whole of
/// why removal is complete, and nothing else checks that they agree.
fn names_to_remove(account: &AccountRef) -> [String; 2] {
    [account.service(), account.refresh_service()]
}

fn rm(reference: &str) -> Result<i32> {
    let account = AccountRef::parse_ref(reference)?;
    let [service, refresh] = names_to_remove(&account);
    let mut client = Client::connect()?;
    // Refuse a name that is not stored, rather than reporting success for a
    // removal that removed nothing — which is the answer a user reads as "the
    // credential is gone".
    if !stored_accounts(&mut client)?
        .iter()
        .any(|(_, i)| i.service == service)
    {
        bail!("no account {reference} is stored; `apex account list` shows what is");
    }
    client.call(&Request::Remove {
        service: service.clone(),
    })?;
    println!("removed {reference}, and every grant that named it");

    // The refresh credential is not an account and cannot be reached any other
    // way: `.` is not legal in an account name, so `AccountRef::parse` does not
    // see it, `apex account list` never shows it, and `rm` refuses to be
    // pointed at it directly. Computing the name here is the only way in — and
    // without this, removing a Google account would leave a live refresh token
    // on disk that nothing in this command surface could find or delete. That
    // is criterion 3, and it became reachable the moment `add` started storing
    // one.
    match client.request(&Request::Remove {
        service: refresh.clone(),
    })? {
        // There was none. An app password has no refresh token, and neither
        // has an OAuth grant the server returned without one.
        Response::Error {
            kind: ErrorKind::NoSuchService,
            ..
        } => {}
        // Anything else is the failure this whole block exists to prevent, so
        // it is reported rather than swallowed: the account is gone from the
        // listing and a credential that can mint new access tokens is not.
        Response::Error { message, .. } => bail!(
            "{reference} is gone, but its refresh token is still stored as \
             '{refresh}' and could not be removed: {message}"
        ),
        _ => println!("removed its refresh token too"),
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Parser)]
    struct Harness {
        #[command(subcommand)]
        cmd: AccountCmd,
    }

    fn parse(args: &[&str]) -> Result<AccountCmd, clap::Error> {
        let mut full = vec!["account"];
        full.extend_from_slice(args);
        Harness::try_parse_from(full).map(|h| h.cmd)
    }

    #[test]
    fn the_verbs_that_need_an_account_refuse_to_run_without_one() {
        for missing in [
            vec!["add"],
            vec!["grant"],
            vec!["grant", "nextcloud.home"],
            vec!["revoke", "nextcloud.home"],
            vec!["rm"],
            vec!["scopes"],
        ] {
            assert!(parse(&missing).is_err(), "{missing:?} parsed without a name");
        }
        assert!(parse(&["list"]).is_ok());
        assert!(parse(&["providers"]).is_ok());
    }

    #[test]
    fn there_is_no_flag_that_puts_a_credential_on_the_command_line() {
        // The rule `apex secret add` follows. A `--token` or `--password`
        // option would put the credential in /proc/<pid>/cmdline and in the
        // shell history, and it is the kind of convenience that gets added
        // later by somebody who did not read this file — so it is asserted,
        // not commented.
        let help = {
            use clap::CommandFactory;
            let mut c = Harness::command();
            c.render_long_help().to_string()
        };
        // `--client-secret` is spelled out rather than left to `--secret`,
        // which does not match it: the device-code flow needs one for Google,
        // and a flag is exactly how somebody would add it without noticing
        // that this is the file saying not to.
        for forbidden in [
            "--token",
            "--password",
            "--secret",
            "--client-secret",
            "--value",
            "--key",
        ] {
            assert!(
                !help.contains(forbidden),
                "{forbidden} appears in the argument surface; credentials come from stdin"
            );
        }
    }

    #[test]
    fn every_device_code_provider_needs_a_client_id_and_says_why_it_has_none() {
        // Not "not filled in yet". APEX registers no OAuth application at
        // either provider and will not borrow one, so the refusal is the
        // answer and it has to carry the reason and the way out — a bare
        // "missing --client-id" would read as a flag somebody forgot.
        let device: Vec<&Provider> = account::PROVIDERS
            .iter()
            .filter(|p| p.flow == Flow::DeviceCode)
            .collect();
        assert!(!device.is_empty(), "the table lost its OAuth providers");
        for provider in device {
            let oauth = provider.oauth.expect("a device-code flow has a server");
            assert!(
                oauth.client_id.is_none(),
                "{} ships a client id; the refusal below is now unreachable and \
                 this build is signing users in as somebody",
                provider.id
            );
            let said = client_for(provider, oauth, None)
                .expect_err("a missing client id is a refusal")
                .to_string();
            assert!(said.contains("--client-id"), "{said}");
            assert!(said.contains(oauth.auth_host), "{said}");
            assert!(said.contains("another project"), "{said}");
            // Google wants a secret on the poll, and being told that after
            // registering a client is worse than being told before.
            if oauth.client_secret == ClientSecret::RequiredToObtain {
                assert!(said.contains("client secret"), "{said}");
                assert!(said.contains("stdin"), "{said}");
            }
        }
    }

    #[test]
    fn a_client_id_is_checked_before_a_device_code_is_issued_against_it() {
        let google = account::provider("google").expect("google is in the table");
        let oauth = google.oauth.expect("google has a server");
        assert_eq!(
            client_for(google, oauth, Some("  my-client.apps.example  ")).unwrap(),
            "my-client.apps.example",
            "the id is trimmed, because a pasted one carries whitespace"
        );
        // An id that could open a second form field is refused now rather than
        // sent: the device request would otherwise carry a scope nobody asked
        // for, and the user would approve it.
        for evil in ["a&scope=admin", "a=b", "a\nb", " "] {
            assert!(
                client_for(google, oauth, Some(evil)).is_err(),
                "'{}' was accepted",
                evil.escape_debug()
            );
        }
    }

    #[test]
    fn the_refresh_half_is_filed_where_the_daemon_will_look_for_it() {
        // Three facts, and a renewal needs all three. The name is the one
        // `account::renewed_service` inverts; the host is the authorisation
        // host, so the pin stops the token being spent at the API; and the
        // username is the client the grant was issued to, which is the field
        // `apex-secretd`'s oauth provider reads and the one thing that was
        // missing for these providers.
        for provider in account::PROVIDERS.iter().filter(|p| p.flow == Flow::DeviceCode) {
            let oauth = provider.oauth.expect("a device-code flow has a server");
            let account = AccountRef::new(provider.id, "work").expect("a legal name");
            let filed = refresh_record(&account, oauth, "my-client");

            assert_eq!(filed.service, account.refresh_service());
            assert_eq!(
                account::renewed_service(&filed.service).as_deref(),
                Some(account.service().as_str()),
                "the daemon derives the credential to renew from this name"
            );
            assert_eq!(filed.host, oauth.auth_host);
            assert_eq!(filed.username, "my-client");
            // The pin is only worth something if the two hosts differ.
            assert_ne!(
                filed.host,
                provider.resolve_host(None).expect("a fixed host"),
                "{}'s refresh token is filed where its access token lives",
                provider.id
            );
            // And the refresh credential is not an account, so no scope can be
            // granted on it and `apex account list` never shows it.
            assert!(AccountRef::parse(&filed.service).is_none());
        }
    }

    #[test]
    fn what_refresh_asks_for_matches_what_add_stored() {
        // The silent failure this prevents: a refresh filed under one name and
        // requested under another is not an error anywhere — it is "not
        // granted", reported against a grant the user can see they wrote.
        let account = AccountRef::new("microsoft", "work").expect("a legal name");
        let record = refresh_request(&account, "/p".to_string());
        assert_eq!(record.provider, account.refresh_service());
        assert_eq!(record.operation, account::REFRESH_OPERATION);
        assert_eq!(record.project.as_deref(), Some("/p"));
        // No resource: the operation declares none, so this CLI has no field
        // with which to aim a refresh at a host other than the pinned one.
        assert!(record.resource.is_empty());
    }

    #[test]
    fn refresh_refuses_an_account_whose_flow_yields_nothing_to_renew() {
        // `Flow::is_refreshable` is a value the table carries so this is read
        // rather than remembered. Telling somebody their Nextcloud app
        // password "will refresh" is a lie they find out about when a backup
        // stops running.
        let said = refresh("nextcloud.home")
            .expect_err("an app password cannot be renewed")
            .to_string();
        assert!(said.contains("apex account add nextcloud.home"), "{said}");
        for p in account::PROVIDERS.iter().filter(|p| p.flow.is_refreshable()) {
            assert_eq!(p.flow, Flow::DeviceCode, "{} would reach the daemon", p.id);
        }
    }

    #[test]
    fn removing_an_account_removes_the_refresh_token_add_stored() {
        // Criterion 3 — "account removal revokes local capabilities cleanly" —
        // and this is the half that only became reachable when `add` started
        // storing a refresh token. The refresh credential is not an account:
        // `.` is illegal in an account name, so it never appears in
        // `apex account list` and `rm` refuses to be pointed at it. Computing
        // it from the account is the only way in, and a `rm` that removed one
        // name would leave a live credential that can mint access tokens on a
        // machine whose owner was told the account was gone.
        for provider in account::PROVIDERS.iter().filter(|p| p.flow == Flow::DeviceCode) {
            let oauth = provider.oauth.expect("a device-code flow has a server");
            let account = AccountRef::new(provider.id, "work").expect("a legal name");
            let [access, refresh] = names_to_remove(&account);

            assert_eq!(access, account.service());
            // The name `add` writes IS the name `rm` deletes. Nothing else
            // compares the two, and a rename on either side would be silent.
            assert_eq!(refresh, refresh_record(&account, oauth, "my-client").service);
            assert_ne!(access, refresh);
            assert!(
                AccountRef::parse(&refresh).is_none(),
                "the refresh credential parses as an account, so `rm` could reach \
                 it directly and this derivation is not the only way in"
            );
        }
    }

    #[test]
    fn a_scope_is_translated_to_its_operation_before_anything_is_granted() {
        // The translation is the reason this front-end exists, and doing it
        // wrong would grant an operation that routes to another provider's
        // code. Checked without a daemon: this is the pure half of `grant`.
        let a = AccountRef::parse_ref("nextcloud.home").unwrap();
        assert_eq!(a.operation("files.read").unwrap(), "webdav.file.read");
        assert_eq!(a.service(), "account.nextcloud.home");
        // And an unknown scope is a refusal here rather than a grant for a
        // capability string nothing will ever match.
        assert!(a.operation("files.delete").is_err());
    }

    #[test]
    fn a_stored_service_that_is_not_an_account_is_invisible_to_this_command() {
        // `stored_accounts` is what `list` shows and what `rm` checks against.
        // Its filter is `AccountRef::parse`, so a git token cannot be listed
        // as an account and cannot be removed by `apex account rm`.
        let github = ServiceInfo {
            service: "github".to_string(),
            ..fixture("account.webdav.home")
        };
        let account = fixture("account.webdav.home");
        let kept: Vec<String> = [github, account]
            .into_iter()
            .filter_map(|i| AccountRef::parse(&i.service).map(|_| i.service))
            .collect();
        assert_eq!(kept, vec!["account.webdav.home".to_string()]);
    }

    #[test]
    fn a_grant_is_keyed_the_way_apex_secret_keys_one() {
        // The defect this rules out: `apex secret grant` keys on the project
        // ROOT (apex_agent_core::project::detect), and apex-agentd forwards a
        // session's root when it uses a capability. A second derivation here
        // that stored the current DIRECTORY would write a key that nothing
        // matches from any subdirectory — a grant that was made, reported, and
        // silently never applies. One function, called from both.
        let source = include_str!("account.rs");
        let shipped = source.split("#[cfg(test)]").next().expect("source");
        assert!(
            shipped.contains("crate::secret::current_project_root()"),
            "apex account derives a project key of its own"
        );
        assert!(
            !shipped.contains("current_dir"),
            "apex account reads the current directory rather than the project root"
        );
    }

    #[test]
    fn a_basic_provider_is_the_pair_or_it_is_nothing() {
        // Which providers this rule binds, asserted over the table rather than
        // over the two it was written for.
        for p in account::PROVIDERS {
            if p.presentation == Presentation::Basic {
                assert!(
                    matches!(p.flow, apex_secret_core::account::Flow::AppPassword),
                    "{} presents Basic without an app-password flow",
                    p.id
                );
            }
        }
        // And the refusal is in the shipped half of this file, before stdin.
        let shipped = include_str!("account.rs")
            .split("#[cfg(test)]")
            .next()
            .expect("source");
        let refusal = shipped
            .find("Presentation::Basic && username.trim().is_empty()")
            .expect("the username refusal is gone");
        let stdin = shipped.find("read_to_string").expect("stdin read");
        assert!(refusal < stdin, "the username is checked after the credential is read");
    }

    fn fixture(service: &str) -> ServiceInfo {
        ServiceInfo {
            service: service.to_string(),
            host: "cloud.example".to_string(),
            scheme: "https".to_string(),
            username: "me".to_string(),
            path: String::new(),
            auth: "raw".to_string(),
            port: None,
            added: 0,
        }
    }
}
