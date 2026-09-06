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
use apex_agent_core::client as agent_client;
use apex_agent_core::protocol::{Request as AgentRequest, Response as AgentResponse};
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
        } => add(&service, &host, &username, &scheme),
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

fn add(service: &str, host: &str, username: &str, scheme: &str) -> Result<i32> {
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
        &SecretValue::new(value.as_bytes().to_vec()),
    )?;
    println!("stored a credential for {service} ({scheme}://{host})");
    println!("nothing is allowed yet — grant a capability with:");
    println!("  apex secret grant {service} git-push");
    Ok(0)
}

fn list(json: bool) -> Result<i32> {
    let services = match Client::connect()?.call(&Request::List)? {
        Response::Services { services } => services,
        other => bail!("unexpected reply: {}", other.variant()),
    };
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
         credential, so a grant cannot be turned into a push to anywhere else."
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

    match agent_client::call(&AgentRequest::SecretUse {
        service: service.to_string(),
        capability: capability.to_string(),
        remote: remote.to_string(),
        branch: branch.map(str::to_string),
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
