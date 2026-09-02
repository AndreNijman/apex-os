//! `apex secret` — the user-facing half of the secret broker (§4).
//!
//! ```text
//! apex secret add github --host github.com     # token on stdin
//! apex secret list
//! apex secret grant github git-push
//! apex secret use github git-push origin       # run by an agent
//! apex secret audit
//! ```
//!
//! `use` is the interesting one: an agent runs it, the DAEMON performs the git
//! operation, and what comes back is git's output with the token scrubbed. The
//! agent never holds the credential. See `apex-agentd`'s broker module for why
//! a git credential helper cannot achieve that.

use std::io::Read;

use anyhow::{bail, Result};
use apex_agent_core::client;
use apex_agent_core::protocol::{Request, Response};
use apex_agent_core::secret::{self, Backend, Capability, ServiceInfo};
use clap::Subcommand;

/// `apex secret <verb>`.
#[derive(Subcommand)]
pub enum SecretCmd {
    /// Store a credential. The token is read from stdin, never from the
    /// command line — argv is world-readable through /proc.
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
        /// Store the token in the login keyring instead of a 0600 file.
        ///
        /// Not the default, and deliberately so: on APEX `secret-tool` blocks
        /// on an "Unlock Keyring" dialog, and gnome-keyring-daemon ships
        /// disabled. A broker an agent calls must not hang or raise a prompt
        /// nobody is watching for. Every keyring call is bounded by a timeout.
        #[arg(long)]
        keyring: bool,
    },
    /// Stored services. Never prints a token.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Delete a stored credential.
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
    /// Use a capability. The broker performs it; you get the result.
    Use {
        service: String,
        /// One of `apex secret capabilities`.
        capability: String,
        /// A git remote NAME, never a URL. The daemon resolves it against this
        /// repository's own configuration — accepting a URL would let a
        /// session choose where the token gets sent.
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
            keyring,
        } => add(&service, &host, &username, keyring),
        SecretCmd::List { json } => list(json),
        SecretCmd::Remove { service } => remove(&service),
        SecretCmd::Capabilities => {
            capabilities();
            Ok(0)
        }
        SecretCmd::Grant { service, capability } => grant(&service, &capability, false),
        SecretCmd::Revoke { service, capability } => grant(&service, &capability, true),
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

fn add(service: &str, host: &str, username: &str, keyring: bool) -> Result<i32> {
    if !secret::valid_service_name(service) {
        bail!("'{service}' is not a usable service name (letters, digits, _ - .)");
    }
    if host.trim().is_empty() {
        bail!("--host is required: it is what a remote's URL is checked against");
    }

    // stdin, never argv. A token on a command line is visible in
    // /proc/<pid>/cmdline to every process on the machine for as long as this
    // runs, and in the shell history forever.
    let mut token = String::new();
    std::io::stdin().read_to_string(&mut token)?;
    let token = token.trim();
    if token.is_empty() {
        bail!(
            "no token on stdin. Pipe it in:\n  \
             printf %s \"$TOKEN\" | apex secret add {service} --host {host}"
        );
    }

    let info = ServiceInfo {
        service: service.to_string(),
        host: host.trim().to_ascii_lowercase(),
        username: username.to_string(),
        backend: if keyring { Backend::Keyring } else { Backend::File },
        added: apex_agent_core::request::now_ms() / 1000,
    };
    secret::store(&info, token)?;
    println!(
        "stored a credential for {} ({}), backend {}",
        info.service,
        info.host,
        info.backend.as_str()
    );
    println!("nothing is allowed yet — grant a capability with:");
    println!("  apex secret grant {} git-push", info.service);
    Ok(0)
}

fn list(json: bool) -> Result<i32> {
    let all = secret::list();
    if json {
        println!("{}", serde_json::to_string_pretty(&all)?);
        return Ok(0);
    }
    if all.is_empty() {
        println!("no credentials stored");
        return Ok(0);
    }
    println!("{:<16} {:<22} {:<10} USERNAME", "SERVICE", "HOST", "BACKEND");
    for i in &all {
        println!(
            "{:<16} {:<22} {:<10} {}",
            i.service,
            i.host,
            i.backend.as_str(),
            i.username
        );
    }
    Ok(0)
}

fn remove(service: &str) -> Result<i32> {
    secret::remove(service)?;
    println!("removed {service}");
    Ok(0)
}

fn capabilities() {
    println!("Capabilities an agent can be granted:\n");
    for name in Capability::names() {
        println!("  {:<12} {}", name, Capability::describe(name));
    }
    println!(
        "\nThe broker PERFORMS these; it never hands over the credential. A git\n\
         credential helper cannot do that, because git runs inside the sandbox and\n\
         whatever the helper prints is readable by the agent.\n\
         \n\
         A remote is named, never given as a URL: the daemon resolves the name\n\
         against the repository's own remotes and checks the host against the\n\
         credential, so a grant cannot be turned into a push to anywhere else."
    );
}

fn grant(service: &str, capability: &str, revoke: bool) -> Result<i32> {
    let project = current_project_root()?;
    match client::call(&Request::SecretGrant {
        project: project.clone(),
        service: service.to_string(),
        capability: capability.to_string(),
        revoke,
    })? {
        Response::SecretGrants { .. } => {
            if revoke {
                println!("withdrew {service}:{capability} for {project}");
            } else {
                println!("allowed {service}:{capability} for {project}");
            }
            Ok(0)
        }
        Response::Error { message, .. } => {
            eprintln!("apex secret: {message}");
            Ok(1)
        }
        other => bail!("unexpected reply: {other:?}"),
    }
}

fn grants(json: bool) -> Result<i32> {
    let projects = match client::call(&Request::SecretGrants)? {
        Response::SecretGrants { projects } => projects,
        Response::Error { message, .. } => bail!("{message}"),
        other => bail!("unexpected reply: {other:?}"),
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
///   1  the broker refused, or the operation failed
///   2  the request was malformed
fn use_it(service: &str, capability: &str, remote: &str, branch: Option<&str>) -> Result<i32> {
    // Validated locally first so a typo is immediate. The daemon validates
    // again and trusts none of this.
    if let Err(e) = Capability::parse(capability, remote, branch) {
        eprintln!("apex secret: {e}");
        return Ok(2);
    }

    // Sent because the daemon cannot see this process's working directory. It
    // is ignored for a managed session, whose project the daemon already knows.
    let project = current_project_root().ok();

    match client::call(&Request::SecretUse {
        service: service.to_string(),
        capability: capability.to_string(),
        remote: remote.to_string(),
        branch: branch.map(str::to_string),
        project,
    })? {
        Response::Brokered {
            detail,
            exit_code,
            output,
            ..
        } => {
            if !output.trim().is_empty() {
                println!("{}", output.trim_end());
            }
            if exit_code == 0 {
                eprintln!("apex secret: {detail} — done");
                Ok(0)
            } else {
                eprintln!("apex secret: {detail} — exited {exit_code}");
                Ok(1)
            }
        }
        Response::Error { message, .. } => {
            eprintln!("apex secret: {message}");
            Ok(1)
        }
        other => bail!("unexpected reply: {other:?}"),
    }
}

fn audit(lines: usize) -> Result<i32> {
    let path = secret::audit_log();
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            println!("no capability has been used on this machine yet");
            return Ok(0);
        }
        Err(e) => return Err(e.into()),
    };
    let all: Vec<&str> = text.lines().collect();
    for line in all.iter().rev().take(lines).rev() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        println!(
            "{:<10} {:<12} {:<10} {}",
            v["event"].as_str().unwrap_or("?"),
            v["service"].as_str().unwrap_or("-"),
            v["agent"].as_str().unwrap_or("-"),
            v["detail"].as_str().unwrap_or("")
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
}
