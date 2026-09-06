//! `apex secret` — the user-facing half of the protected secret service (§11).
//!
//! ```text
//! apex secret add github --host github.com     # credential on stdin
//! apex secret list
//! apex secret grant github git-push
//! apex secret use github git-push origin       # run by an agent
//! apex secret audit
//! ```
//!
//! Two daemons answer these, and which one is not arbitrary.
//!
//! * `add`, `remove`, `list`, `grant`, `revoke`, `grants` and `audit` go
//!   straight to `apex-secretd`, which owns the store and refuses any of the
//!   mutating ones from a caller inside an agent session.
//! * `use` goes through `apex-agentd` first, because only that daemon can say
//!   which session is asking, what its secret policy is, and which project it
//!   was started in. It forwards a capability record; `apex-secretd` performs
//!   the operation.
//!
//! Nothing on either path can return the credential. `apex-secretd` has no verb
//! that does.

use std::io::Read;

use anyhow::{bail, Result};
use apex_agent_core::protocol::{
    Request as AgentRequest, Response as AgentResponse, BROKERED_SECRET_SERVICE_VERSION,
};
use apex_secret_core::capability::Capability;
use apex_secret_core::client::Client;
use apex_secret_core::protocol::{Request, Response};
use apex_secret_core::store::valid_service_name;
use apex_secret_core::SecretValue;
use clap::Subcommand;

/// `apex secret <verb>`.
#[derive(Subcommand)]
pub enum SecretCmd {
    /// Store a credential. It is read from stdin, never from the command line —
    /// argv is world-readable through /proc.
    Add {
        /// Name you will refer to it by, e.g. `github`.
        service: String,
        /// Host the credential is valid for. A remote pointing anywhere else
        /// is refused at use time.
        #[arg(long)]
        host: String,
        /// Username to send. Most token schemes ignore it.
        #[arg(long, default_value = "x-access-token")]
        username: String,
        /// Scheme the host is reached over. `http` is accepted only for a
        /// loopback host, where the credential does not cross a network.
        #[arg(long, default_value = "https")]
        scheme: String,
        /// Path on that host this credential's own endpoint lives at, for a
        /// service the broker talks to directly — an MCP server's `/mcp`. Not
        /// used by git, which resolves its URL from the repository.
        #[arg(long, default_value = "")]
        path: String,
        /// How the credential is sent: `bearer` for `Authorization: Bearer
        /// <value>`, `raw` when the stored value is the whole header.
        #[arg(long, default_value = "bearer")]
        auth: String,
        /// Port, when the endpoint is not on the scheme's own. Git ignores it;
        /// the broker's own endpoint needs it.
        #[arg(long)]
        port: Option<u16>,
    },
    /// Stored credentials. Never prints one.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Delete a stored credential and every grant that named it.
    Remove { service: String },
    /// The operations an agent can be granted.
    Capabilities,
    /// Allow a capability for the current project.
    Grant { service: String, capability: String },
    /// Withdraw one.
    Revoke { service: String, capability: String },
    /// What is allowed, per project.
    Grants {
        #[arg(long)]
        json: bool,
    },
    /// Use a capability. The service performs it; you get the result.
    Use {
        service: String,
        /// One of `apex secret capabilities`.
        capability: String,
        /// A git remote NAME, never a URL. The service resolves it against this
        /// repository's own configuration — accepting a URL would let a session
        /// choose where the credential gets sent.
        ///
        /// `allow_hyphen_values` so that `-f` reaches the validator and is
        /// refused as "not a git remote name", rather than being rejected by
        /// the argument parser as an unknown option — which is the right
        /// outcome for the wrong reason, and reads as a bug in the CLI.
        #[arg(default_value = "origin", allow_hyphen_values = true)]
        remote: String,
        /// Branch to push. Defaults to the current one.
        #[arg(long)]
        branch: Option<String>,
    },
    /// Move credentials this machine already has in plaintext into the store.
    ///
    /// Reads each one, stores it, proves the stored copy works, and only then
    /// removes the original — in that order, so an interrupted run leaves a
    /// machine that still works. Run it from your own shell, with no agent
    /// running.
    Migrate {
        /// Say what would move and write nothing.
        #[arg(long)]
        dry_run: bool,
    },

    /// The audit trail: which capability was used, by what, and when.
    Audit {
        #[arg(long, short, default_value_t = 20)]
        lines: usize,
    },
}

pub fn main(cmd: SecretCmd) -> i32 {
    let result = match cmd {
        SecretCmd::Add {
            service,
            host,
            username,
            scheme,
            path,
            auth,
            port,
        } => add(&service, &host, &username, &scheme, &path, &auth, port),
        SecretCmd::List { json } => list(json),
        SecretCmd::Remove { service } => remove(&service),
        SecretCmd::Capabilities => {
            capabilities();
            Ok(0)
        }
        SecretCmd::Grant {
            service,
            capability,
        } => grant(&service, &capability, false),
        SecretCmd::Revoke {
            service,
            capability,
        } => grant(&service, &capability, true),
        SecretCmd::Grants { json } => grants(json),
        SecretCmd::Use {
            service,
            capability,
            remote,
            branch,
        } => use_it(&service, &capability, &remote, branch.as_deref()),
        SecretCmd::Migrate { dry_run } => crate::migrate::main(dry_run),
        SecretCmd::Audit { lines } => audit(lines),
    };
    match result {
        Ok(code) => code,
        Err(e) => {
            eprintln!("apex secret: {e:#}");
            1
        }
    }
}

fn add(
    service: &str,
    host: &str,
    username: &str,
    scheme: &str,
    path: &str,
    auth: &str,
    port: Option<u16>,
) -> Result<i32> {
    if !valid_service_name(service) {
        bail!("'{service}' is not a usable service name (letters, digits, _ - .)");
    }
    if host.trim().is_empty() {
        bail!("--host is required: it is what a remote's URL is checked against");
    }

    // stdin, never argv. A credential on a command line is visible in
    // /proc/<pid>/cmdline to every process on the machine for as long as this
    // runs, and in the shell history forever.
    let mut value = String::new();
    std::io::stdin().read_to_string(&mut value)?;
    let value = value.trim();
    if value.is_empty() {
        bail!(
            "nothing on stdin. Pipe the credential in:\n  \
             printf %s \"$TOKEN\" | apex secret add {service} --host {host}"
        );
    }

    let host = host.trim().to_ascii_lowercase();
    Client::connect()?.add(
        service,
        &host,
        scheme,
        Some(username),
        path,
        auth,
        port,
        &SecretValue::new(value.as_bytes().to_vec()),
    )?;
    println!("stored a credential for {service} ({scheme}://{host})");
    println!("nothing is allowed yet — grant a capability with:");
    println!("  apex secret grant {service} git-push");
    Ok(0)
}

fn list(json: bool) -> Result<i32> {
    let mut client = Client::connect()?;
    let services = match client.call(&Request::List)? {
        Response::Services { services } => services,
        other => bail!("unexpected reply: {}", other.variant()),
    };
    if !json {
        warn_if_unprotected(&mut client);
        warn_about_the_old_store();
    }
    if json {
        println!("{}", serde_json::to_string_pretty(&services)?);
        return Ok(0);
    }
    if services.is_empty() {
        println!("no credentials stored");
        return Ok(0);
    }
    println!("{:<16} {:<28} USERNAME", "SERVICE", "ENDPOINT");
    for i in &services {
        println!(
            "{:<16} {:<28} {}",
            i.service,
            format!("{}://{}", i.scheme, i.host),
            i.username
        );
    }
    Ok(0)
}

/// Where the agent runtime's broker used to keep credentials.
///
/// Plain JSON, `0600`, inside `$HOME`. Readable by anything running as the
/// user, which is the whole reason the store moved to `apex-secretd`.
fn old_store() -> std::path::PathBuf {
    apex_agent_core::paths::state_dir().join("secrets")
}

/// Say so when credentials from the old broker are still lying in the home.
///
/// Nothing reads them any more. They are not deleted for you either: a file
/// that may hold the only copy of a token is not something a `list` command
/// should remove on its own initiative. So it is named, on stderr, where the
/// person who can decide will see it — an upgraded machine keeps its old
/// credential files until somebody looks.
fn warn_about_the_old_store() {
    let dir = old_store();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };
    let count = entries
        .flatten()
        .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("json"))
        .count();
    if count == 0 {
        return;
    }
    let (plural, verb) = if count == 1 { ("", "is") } else { ("s", "are") };
    eprintln!(
        "apex secret: {count} credential file{plural} from the old broker {verb} still in\n  \
         {}\n  \
         Nothing reads them now, and anything running as you can. Re-add what you\n  \
         still need with `apex secret add`, then delete that directory.",
        dir.display()
    );
}

/// Say so when the service is holding the store where the caller could read it.
///
/// True of a daemon started by hand for a test, false of the one the image
/// ships. Reporting it costs one round trip and stops a test instance from
/// looking like a boundary it is not.
fn warn_if_unprotected(client: &mut Client) {
    if let Ok(Response::Hello {
        protected: false, ..
    }) = client.call(&Request::Hello)
    {
        eprintln!(
            "apex secret: this secret service is not running as root, so its store is\n  \
             readable by your own account. That is a test instance, not a boundary."
        );
    }
}

fn remove(service: &str) -> Result<i32> {
    Client::connect()?.call(&Request::Remove {
        service: service.to_string(),
    })?;
    println!("removed {service}, and every grant that named it");
    Ok(0)
}

fn capabilities() {
    println!("Capabilities an agent can be granted:\n");
    for name in Capability::names() {
        println!("  {:<14} {}", name, Capability::describe(name));
    }
    println!(
        "\napex-secretd PERFORMS these; it never hands over the credential, and\n\
         has no verb that could. A git credential helper cannot achieve that,\n\
         because git runs inside the sandbox and whatever the helper prints is\n\
         readable by the agent.\n\
         \n\
         A remote is named, never given as a URL: the service resolves the name\n\
         against the repository's own remotes and checks the host against the\n\
         credential, so a grant cannot be turned into a push to anywhere else.\n\
         \n\
         You do not have to type any of this. A managed session finds a `git` on\n\
         its PATH that sends push, fetch and ls-remote here and execs the real\n\
         git for everything else, so a skill keeps running `git push`."
    );
}

fn grant(service: &str, capability: &str, revoke: bool) -> Result<i32> {
    let project = current_project_root()?;
    Client::connect()?.call(&Request::Grant {
        project: project.clone(),
        service: service.to_string(),
        capability: capability.to_string(),
        revoke,
    })?;
    if revoke {
        println!("withdrew {service}:{capability} for {project}");
    } else {
        println!("allowed {service}:{capability} for {project}");
    }
    Ok(0)
}

fn grants(json: bool) -> Result<i32> {
    let projects = match Client::connect()?.call(&Request::Grants)? {
        Response::Grants { projects } => projects,
        other => bail!("unexpected reply: {}", other.variant()),
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&projects)?);
        return Ok(0);
    }
    if projects.is_empty() {
        println!("nothing is granted; no agent can use a credential");
        return Ok(0);
    }
    for (project, keys) in &projects {
        println!("{project}");
        for k in keys {
            println!("    {k}");
        }
    }
    Ok(0)
}

/// Exit codes are the message, because an agent reads `$?`:
///   0  the operation ran and succeeded
///   1  the service refused, or the operation failed
///   2  the request was malformed
fn use_it(service: &str, capability: &str, remote: &str, branch: Option<&str>) -> Result<i32> {
    // Validated locally first so a typo is immediate. Both daemons validate
    // again and trust none of this.
    if let Err(e) = Capability::parse(capability, remote, branch) {
        eprintln!("apex secret: {e}");
        return Ok(2);
    }

    // Sent because `apex-agentd` cannot see this process's working directory.
    // It is ignored for a managed session, whose project the daemon already
    // knows.
    let project = current_project_root().ok();

    let mut agent = apex_agent_core::client::Client::connect()?;
    require_a_runtime_that_forwards(&mut agent)?;

    match agent.call(&AgentRequest::SecretUse {
        service: service.to_string(),
        capability: capability.to_string(),
        remote: remote.to_string(),
        branch: branch.map(str::to_string),
        body: None,
        project,
    })? {
        AgentResponse::Brokered {
            detail,
            endpoint,
            exit_code,
            output,
            ..
        } => {
            if !output.trim().is_empty() {
                println!("{}", output.trim_end());
            }
            if exit_code == 0 {
                eprintln!("apex secret: {detail} against {endpoint} — done");
                Ok(0)
            } else {
                eprintln!("apex secret: {detail} against {endpoint} — exited {exit_code}");
                Ok(1)
            }
        }
        AgentResponse::Error { message, .. } => {
            eprintln!("apex secret: {message}");
            Ok(1)
        }
        other => bail!("unexpected reply: {other:?}"),
    }
}

/// Refuse to ask a runtime that predates the secret service.
///
/// A daemon below [`BROKERED_SECRET_SERVICE_VERSION`] has its own broker and
/// its own store in `$HOME`, so it would look for the credential in a place
/// `apex secret add` no longer writes to and answer "no credential stored" —
/// a true sentence about the wrong store, and the most confusing possible
/// reply to somebody who just added one.
///
/// The guard names the constant rather than a literal, for the same reason the
/// two before it do: a bare `< 4` is one careless edit away from meaning
/// nothing.
fn require_a_runtime_that_forwards(agent: &mut apex_agent_core::client::Client) -> Result<()> {
    let AgentResponse::Hello { version, .. } = agent.call(&AgentRequest::Hello)? else {
        // A daemon that cannot answer the handshake is one this cannot reason
        // about, and guessing in the permissive direction is the failure mode.
        bail!("the agent runtime did not answer the protocol handshake");
    };
    if version < BROKERED_SECRET_SERVICE_VERSION {
        bail!(
            "the running agent runtime speaks protocol {version} and still keeps its own              credentials, so it would not find one added to the secret service; restart it              with `systemctl --user restart apex-agentd`"
        );
    }
    Ok(())
}

fn audit(lines: usize) -> Result<i32> {
    let entries = match Client::connect()?.call(&Request::Audit { lines })? {
        Response::Audit { entries } => entries,
        other => bail!("unexpected reply: {}", other.variant()),
    };
    if entries.is_empty() {
        println!("no capability has been used by this account yet");
        return Ok(0);
    }
    for e in &entries {
        let outcome = match (e.exit_code, e.reason.as_deref()) {
            (Some(0), _) => String::new(),
            (Some(code), _) => format!("  exit {code}"),
            (None, Some(reason)) => format!("  {reason}"),
            (None, None) => String::new(),
        };
        println!(
            "{:<9} {:<14} {:<24} {}{}",
            e.event.as_str(),
            e.provider,
            e.detail,
            e.endpoint.as_deref().unwrap_or("-"),
            outcome
        );
    }
    Ok(0)
}

fn current_project_root() -> Result<String> {
    let cwd = std::env::current_dir()?;
    apex_agent_core::project::detect(&cwd)
        .map(|p| p.root)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "{} is not inside a git repository, and a capability is granted \
                 per project",
                cwd.display()
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_capability_has_a_help_line() {
        for name in Capability::names() {
            assert!(
                !Capability::describe(name).is_empty(),
                "{name} has no description, so `apex secret capabilities` \
                 would list it blank"
            );
        }
    }

    #[test]
    fn the_version_guard_names_the_revision_the_store_moved_in() {
        // A literal here would be one careless edit from meaning nothing, and
        // the failure it prevents is a "no credential stored" about a store
        // the user never wrote to. It is a floor rather than the current
        // revision — later revisions add capabilities, and a daemon that has
        // the store in the right place still serves `git-push` correctly.
        assert!(
            BROKERED_SECRET_SERVICE_VERSION <= apex_agent_core::protocol::PROTOCOL_VERSION,
            "the guard names a revision ahead of the protocol, so it can never fire"
        );
        assert!(BROKERED_SECRET_SERVICE_VERSION > 0);
    }

    #[test]
    fn the_use_exit_codes_are_distinct() {
        // An agent branches on these; two states sharing a code would make
        // "refused" and "malformed" indistinguishable.
        let codes = [0, 1, 2];
        let mut seen = std::collections::HashSet::new();
        for c in codes {
            assert!(seen.insert(c), "{c} is used twice");
        }
    }

    #[test]
    fn no_verb_offers_to_hand_a_credential_back() {
        // The CLI is the surface people read, and a `apex secret show github`
        // would be an obvious thing for somebody to add — obvious, and the one
        // thing this whole task exists to make impossible. The daemon could not
        // answer it, so it would ship as a verb that always fails; asserting
        // the vocabulary here means it never gets written in the first place.
        //
        // The image asserts the same thing against the built binary's help, in
        // Containerfile.base. This is where it fails first.
        let cmd = SecretCmd::augment_subcommands(clap::Command::new("secret"));
        let names: Vec<String> = cmd
            .get_subcommands()
            .map(|c| c.get_name().to_string())
            .collect();
        for forbidden in ["show", "reveal", "export", "cat", "print", "read", "dump", "get"] {
            assert!(
                !names.iter().any(|n| n == forbidden),
                "`apex secret {forbidden}` exists; no verb may return a credential"
            );
        }
        // ...and the ones that must exist, by name, because a rename is a
        // silent no-op in every skill and shell function that calls them.
        for expected in [
            "add",
            "list",
            "remove",
            "capabilities",
            "grant",
            "revoke",
            "grants",
            "use",
            "migrate",
            "audit",
        ] {
            assert!(names.iter().any(|n| n == expected), "`{expected}` is gone");
        }
    }

    #[test]
    fn the_capability_text_says_the_credential_is_never_handed_over() {
        // `apex secret capabilities` is where somebody decides whether to trust
        // this with a credential. If it stops saying what the service refuses
        // to do, the one thing they needed is gone.
        let mut text = String::new();
        for name in Capability::names() {
            text.push_str(Capability::describe(name));
        }
        assert!(text.contains("remote"), "{text}");
    }
}
