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
//! ## The unmet half, said here rather than in a release note
//!
//! `Flow::DeviceCode` is **not implemented**. Google and Microsoft accounts can
//! be added today only by pasting a token the user obtained elsewhere, and APEX
//! cannot refresh it — so it stops working when it expires and the user is told
//! that at `add` time instead of finding out. The provider table carries
//! [`Flow::is_refreshable`] precisely so this is a value the code reads rather
//! than a caveat in prose.

use std::io::Read;

use anyhow::{bail, Result};
use apex_secret_core::account::{self, AccountError, AccountRef, Presentation, Provider};
use apex_secret_core::client::Client;
use apex_secret_core::protocol::{Request, Response};
use apex_secret_core::store::ServiceInfo;
use apex_secret_core::SecretValue;
use clap::Subcommand;

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
        } => add(&account, host.as_deref(), &username, path.as_deref(), port),
        AccountCmd::List { json } => list(json),
        AccountCmd::Grant { account, scope } => grant(&account, &scope, false),
        AccountCmd::Revoke { account, scope } => grant(&account, &scope, true),
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
) -> Result<i32> {
    let account = AccountRef::parse_ref(reference)?;
    let provider = account.provider;
    let host = provider.resolve_host(host.map(str::trim))?.to_ascii_lowercase();

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

    // Said BEFORE the credential is read, not after: a user who is about to
    // paste a token that APEX cannot renew should find that out while they
    // still have the provider's page open.
    if provider.flow.is_refreshable() {
        eprintln!(
            "apex account: the {} sign-in flow ({}) is not implemented. Paste an access\n  \
             token you already hold and APEX will store and broker it — but it cannot be\n  \
             refreshed, so it stops working when the provider expires it.",
            provider.label,
            provider.flow.as_str()
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

fn rm(reference: &str) -> Result<i32> {
    let account = AccountRef::parse_ref(reference)?;
    let service = account.service();
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
        for forbidden in ["--token", "--password", "--secret", "--value", "--key"] {
            assert!(
                !help.contains(forbidden),
                "{forbidden} appears in the argument surface; credentials come from stdin"
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
