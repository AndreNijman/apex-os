//! `apex capability` — the user-facing half of `apex-secretd` (roadmap §11).
//!
//! ```text
//! sudo apex capability store github --provider github   # value on stdin
//! apex capability list
//! sudo apex capability grant github whoami
//! apex capability use github whoami                     # run by an agent
//! apex capability audit
//! ```
//!
//! Two commands need `sudo` and the rest do not, which is the design rather
//! than an inconvenience. Storing a credential and granting an operation on it
//! are owner decisions; a managed agent runs under the owner's own uid, so if
//! the owner could make them without authenticating, so could the agent.
//! Everything an agent legitimately does — listing, using, reading the audit
//! trail — needs no privilege at all.
//!
//! `apex secret` is the older, per-user broker that lives in `apex-agentd` and
//! keeps its store in `$HOME`. It is untouched here; migrating what it holds is
//! a separate piece of work.

use std::io::Read;

use anyhow::{bail, Context, Result};
use apex_secret_core::capability::{CapabilityRequest, Origin};
use apex_secret_core::client::{self, Channel, Client};
use apex_secret_core::protocol::{
    GrantRequest, InboundSecret, Request, Response, RotateRequest, StoreRequest,
};
use apex_secret_core::seal::Sealing;
use clap::Subcommand;

/// `apex capability <verb>`.
#[derive(Subcommand)]
pub enum CapabilityCmd {
    /// The providers and operations this build offers.
    Providers {
        #[arg(long)]
        json: bool,
    },
    /// Store a credential. Needs sudo. The value is read from stdin, never
    /// from the command line — argv is world-readable through /proc.
    Store {
        /// Name you will refer to it by, e.g. `github`.
        secret: String,
        /// Whose vocabulary applies. See `apex capability providers`.
        #[arg(long)]
        provider: String,
        /// Endpoint, when it is not the provider's default — a GitHub
        /// Enterprise host. An absolute http or https origin with no trailing
        /// slash.
        #[arg(long)]
        base_url: Option<String>,
        /// A note for yourself. The service does nothing with it.
        #[arg(long)]
        label: Option<String>,
        /// Encrypt the value to this machine's TPM.
        ///
        /// Off by default. It protects a copy of the disk taken elsewhere, not
        /// the running machine, and it needs the service's account to be
        /// permitted to use the TPM — which on a stock host it is not.
        #[arg(long)]
        seal: bool,
        /// Whose store to write to. Defaults to the user who ran sudo.
        #[arg(long)]
        owner: Option<u32>,
    },
    /// Replace the value of a stored credential, keeping its grants. Needs
    /// sudo. The new value is read from stdin.
    Rotate {
        secret: String,
        #[arg(long)]
        owner: Option<u32>,
    },
    /// Stored credentials. Never prints a value; there is no verb that does.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Delete a credential and every grant on it. Needs sudo.
    Remove {
        secret: String,
        #[arg(long)]
        owner: Option<u32>,
    },
    /// Allow an operation on a credential. Needs sudo.
    Grant {
        secret: String,
        /// One of `apex capability providers`.
        operation: String,
        /// Pin the grant to one resource, e.g. `AndreNijman/apex-os`. Without
        /// it the grant covers any resource the operation accepts.
        #[arg(long)]
        resource: Option<String>,
        /// Withdraw the grant after this many seconds.
        #[arg(long)]
        ttl: Option<u64>,
        #[arg(long)]
        owner: Option<u32>,
    },
    /// Withdraw a grant. Needs sudo.
    Revoke {
        secret: String,
        operation: String,
        /// Must match the resource the grant was made with.
        #[arg(long)]
        resource: Option<String>,
        #[arg(long)]
        owner: Option<u32>,
    },
    /// What is allowed.
    Grants {
        #[arg(long)]
        json: bool,
    },
    /// Perform an operation. The service does it; you get the result.
    Use {
        secret: String,
        operation: String,
        /// What the operation acts on, when it takes one.
        #[arg(long)]
        resource: Option<String>,
        /// The project you are working in. Recorded in the audit trail as a
        /// claim — this service cannot verify it and does not decide on it.
        #[arg(long)]
        project: Option<String>,
        /// The agent session this belongs to. Recorded as a claim, like
        /// --project.
        #[arg(long)]
        session: Option<u32>,
        /// Mark the request as relayed from Remote Control. Recorded as a
        /// claim.
        #[arg(long)]
        remote: bool,
        #[arg(long)]
        json: bool,
    },
    /// The audit trail: what was used, by whom, and when.
    Audit {
        #[arg(long, short, default_value_t = 20)]
        lines: usize,
        #[arg(long)]
        json: bool,
    },
}

pub fn main(cmd: CapabilityCmd) -> i32 {
    match run(cmd) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("apex capability: {e:#}");
            1
        }
    }
}

fn run(cmd: CapabilityCmd) -> Result<i32> {
    match cmd {
        CapabilityCmd::Providers { json } => providers(json),
        CapabilityCmd::Store {
            secret,
            provider,
            base_url,
            label,
            seal,
            owner,
        } => store(secret, provider, base_url, label, seal, owner),
        CapabilityCmd::Rotate { secret, owner } => rotate(secret, owner),
        CapabilityCmd::List { json } => list(json),
        CapabilityCmd::Remove { secret, owner } => remove(secret, owner),
        CapabilityCmd::Grant {
            secret,
            operation,
            resource,
            ttl,
            owner,
        } => grant(secret, operation, resource, ttl, owner, false),
        CapabilityCmd::Revoke {
            secret,
            operation,
            resource,
            owner,
        } => grant(secret, operation, resource, None, owner, true),
        CapabilityCmd::Grants { json } => grants(json),
        CapabilityCmd::Use {
            secret,
            operation,
            resource,
            project,
            session,
            remote,
            json,
        } => use_it(secret, operation, resource, project, session, remote, json),
        CapabilityCmd::Audit { lines, json } => audit(lines, json),
    }
}

/// Whose store an administrative command acts on.
///
/// `$SUDO_UID` rather than the effective uid, because under `sudo` the
/// effective uid is root and root's store is almost never the one meant. An
/// explicit `--owner` wins over both.
fn owner_default(explicit: Option<u32>) -> Option<u32> {
    if explicit.is_some() {
        return explicit;
    }
    std::env::var("SUDO_UID")
        .ok()
        .and_then(|v| v.trim().parse().ok())
        // Safe: getuid cannot fail and has no side effects.
        .or_else(|| Some(unsafe { libc::getuid() }))
}

/// Read a credential from stdin.
///
/// stdin, never argv: a value on a command line is in `/proc/<pid>/cmdline` for
/// every process on the machine while it runs, and in the shell history
/// afterwards.
fn value_from_stdin(what: &str) -> Result<InboundSecret> {
    value_from(std::io::stdin(), what)
}

/// The half that does the work, taking a reader so the empty-input path can be
/// tested without a test that blocks on a terminal.
fn value_from(mut source: impl Read, what: &str) -> Result<InboundSecret> {
    let mut raw = String::new();
    source
        .read_to_string(&mut raw)
        .context("reading the credential from stdin")?;
    let raw = raw.trim();
    if raw.is_empty() {
        bail!(
            "nothing on stdin. Pipe the credential in:\n  \
             printf %s \"$TOKEN\" | sudo apex capability {what}"
        );
    }
    Ok(InboundSecret::new(raw))
}

fn providers(json: bool) -> Result<i32> {
    let reply = client::call(Channel::Broker, &Request::Providers)?;
    let Response::Providers { providers } = reply else {
        bail!("unexpected reply to providers");
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&providers)?);
        return Ok(0);
    }
    for p in &providers {
        println!("{} — {}", p.id, p.summary);
        println!("  endpoint {}", p.default_base);
        for op in &p.operations {
            let shape = match &op.resource_shape {
                Some(s) => format!(" <{s}>"),
                None => String::new(),
            };
            println!("    {}{}", op.id, shape);
            println!("      {}", op.summary);
            println!("      returns: {}", op.fields.join(", "));
        }
        println!();
    }
    println!("An operation is performed by the service. Its result comes back;");
    println!("the credential does not, and there is no verb that returns one.");
    Ok(0)
}

fn store(
    secret: String,
    provider: String,
    base_url: Option<String>,
    label: Option<String>,
    seal: bool,
    owner: Option<u32>,
) -> Result<i32> {
    let value = value_from_stdin(&format!("store {secret} --provider {provider}"))?;
    let owner = owner_default(owner);
    let reply = Client::connect(Channel::Admin)?.call(&Request::Store(Box::new(StoreRequest {
        owner,
        secret: secret.clone(),
        provider,
        base_url,
        label,
        sealing: if seal { Sealing::Tpm2 } else { Sealing::Plain },
        value,
    })))?;
    let Response::Secret(meta) = reply else {
        bail!("unexpected reply to store");
    };
    println!(
        "stored {} for {} ({}), sealed with {}",
        meta.name,
        meta.provider,
        meta.base().unwrap_or("no endpoint"),
        meta.sealing.as_str()
    );
    println!("nothing is allowed with it yet. Grant an operation:");
    println!("  sudo apex capability grant {secret} <operation>");
    Ok(0)
}

fn rotate(secret: String, owner: Option<u32>) -> Result<i32> {
    let value = value_from_stdin(&format!("rotate {secret}"))?;
    let owner = owner_default(owner);
    let reply = Client::connect(Channel::Admin)?.call(&Request::Rotate(Box::new(RotateRequest {
        owner,
        secret: secret.clone(),
        value,
    })))?;
    let Response::Secret(meta) = reply else {
        bail!("unexpected reply to rotate");
    };
    println!("rotated {}; its grants are unchanged", meta.name);
    Ok(0)
}

fn list(json: bool) -> Result<i32> {
    let Response::Secrets { secrets } = client::call(Channel::Broker, &Request::List)? else {
        bail!("unexpected reply to list");
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&secrets)?);
        return Ok(0);
    }
    if secrets.is_empty() {
        println!("no credentials stored for you");
        println!("add one with: sudo apex capability store <name> --provider <provider>");
        return Ok(0);
    }
    println!("{:<16} {:<12} {:<8} ENDPOINT", "NAME", "PROVIDER", "SEALING");
    for s in &secrets {
        println!(
            "{:<16} {:<12} {:<8} {}",
            s.name,
            s.provider,
            s.sealing.as_str(),
            s.base().unwrap_or("-")
        );
    }
    Ok(0)
}

fn remove(secret: String, owner: Option<u32>) -> Result<i32> {
    let owner = owner_default(owner);
    Client::connect(Channel::Admin)?.call(&Request::Remove {
        owner,
        secret: secret.clone(),
    })?;
    println!("removed {secret} and every grant on it");
    Ok(0)
}

fn grant(
    secret: String,
    operation: String,
    resource: Option<String>,
    ttl: Option<u64>,
    owner: Option<u32>,
    revoke: bool,
) -> Result<i32> {
    let owner = owner_default(owner);
    let reply = Client::connect(Channel::Admin)?.call(&Request::Grant(Box::new(GrantRequest {
        owner,
        secret: secret.clone(),
        operation: operation.clone(),
        resource: resource.clone(),
        ttl_secs: ttl,
        revoke,
    })))?;
    let Response::Grants { grants } = reply else {
        bail!("unexpected reply to grant");
    };
    if revoke {
        println!("withdrew {secret}:{operation}");
    } else {
        let scope = match &resource {
            Some(r) => format!(" on {r}"),
            None => String::new(),
        };
        let window = match ttl {
            Some(t) => format!(", for {t}s"),
            None => String::new(),
        };
        println!("allowed {secret}:{operation}{scope}{window}");
    }
    println!("{} grant(s) now stand", grants.len());
    Ok(0)
}

fn grants(json: bool) -> Result<i32> {
    let Response::Grants { grants } = client::call(Channel::Broker, &Request::Grants)? else {
        bail!("unexpected reply to grants");
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&grants)?);
        return Ok(0);
    }
    if grants.is_empty() {
        println!("nothing is allowed");
        return Ok(0);
    }
    let now = apex_secret_core::now_ms();
    println!("{:<16} {:<16} {:<24} EXPIRES", "CREDENTIAL", "OPERATION", "RESOURCE");
    for g in &grants {
        let expires = match g.expiry_ms {
            None => "never".to_string(),
            Some(e) if e <= now => "expired".to_string(),
            Some(e) => format!("in {}s", (e - now) / 1000),
        };
        println!(
            "{:<16} {:<16} {:<24} {}",
            g.secret,
            g.operation,
            g.resource.as_deref().unwrap_or("any"),
            expires
        );
    }
    Ok(0)
}

fn use_it(
    secret: String,
    operation: String,
    resource: Option<String>,
    project: Option<String>,
    session: Option<u32>,
    remote: bool,
    json: bool,
) -> Result<i32> {
    // The working directory, when the caller did not name a project. Recorded
    // as a claim either way: this service runs under a different uid and cannot
    // check what any caller's working directory is.
    let project = project.or_else(|| {
        std::env::current_dir()
            .ok()
            .map(|p| p.to_string_lossy().into_owned())
    });
    let request = CapabilityRequest {
        secret,
        operation,
        resource,
        project,
        agent_session: session.or_else(|| {
            std::env::var("APEX_AGENT_SESSION")
                .ok()
                .and_then(|v| v.trim().parse().ok())
        }),
        origin: if remote { Origin::Remote } else { Origin::Local },
    };
    let reply = client::call(Channel::Broker, &Request::Use(Box::new(request)))?;
    let Response::Performed(performed) = reply else {
        bail!("unexpected reply to use");
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&performed)?);
        return Ok(if performed.outcome.ok() { 0 } else { 1 });
    }
    println!(
        "{}:{} -> {}",
        performed.provider, performed.operation, performed.outcome.status
    );
    for (k, v) in &performed.outcome.fields {
        println!("  {k}: {v}");
    }
    if let Some(detail) = &performed.outcome.detail {
        println!("  {detail}");
    }
    println!("audit id {}", performed.audit_id);
    Ok(if performed.outcome.ok() { 0 } else { 1 })
}

fn audit(lines: usize, json: bool) -> Result<i32> {
    let Response::Audit { entries } = client::call(Channel::Broker, &Request::Audit { limit: lines })?
    else {
        bail!("unexpected reply to audit");
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&entries)?);
        return Ok(0);
    }
    if entries.is_empty() {
        println!("nothing recorded yet");
        return Ok(0);
    }
    for e in &entries {
        let what = match (&e.provider, &e.operation) {
            (Some(p), Some(o)) => format!("{p}:{o}"),
            _ => e.secret.clone().unwrap_or_else(|| "-".to_string()),
        };
        let resource = match &e.resource {
            Some(r) => format!(" {r}"),
            None => String::new(),
        };
        let status = match e.status {
            Some(s) => format!(" [{s}]"),
            None => String::new(),
        };
        println!(
            "{:<9} {:<28}{resource}{status} {} pid {}",
            e.event.as_str(),
            what,
            e.decision,
            e.peer_pid
        );
        if let Some(detail) = &e.detail {
            println!("          {detail}");
        }
    }
    println!();
    println!("project and session are as the caller described them; this service");
    println!("cannot check either, and decides on neither.");
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_administrative_command_targets_the_user_who_ran_sudo() {
        // The bug this avoids: `sudo apex capability store` writing into root's
        // namespace, where the user's agent will never look for it.
        assert_eq!(owner_default(Some(4242)), Some(4242), "explicit wins");
        match std::env::var("SUDO_UID") {
            Ok(v) => assert_eq!(owner_default(None), v.trim().parse().ok()),
            // Safe: getuid has no side effects.
            Err(_) => assert_eq!(owner_default(None), Some(unsafe { libc::getuid() })),
        }
    }

    #[test]
    fn an_empty_stdin_says_how_to_pipe_the_credential_in() {
        // Reached whenever somebody runs the command interactively and waits
        // for a prompt that never comes.
        let err = value_from(std::io::empty(), "store gh --provider github").unwrap_err();
        let text = err.to_string();
        assert!(text.contains("printf"), "{text}");
        assert!(text.contains("store gh --provider github"), "{text}");
        // Whitespace only is the same case: a here-doc that produced a newline.
        assert!(value_from(&b"   \n"[..], "store gh").is_err());
    }

    #[test]
    fn a_credential_is_taken_from_stdin_with_its_surrounding_whitespace_removed() {
        // A trailing newline is what `echo` and every here-doc produce, and it
        // is not part of the credential.
        let v = value_from(&b"not-a-real-token-cli\n"[..], "store gh")
            .expect("accepted")
            .into_value()
            .expect("valid");
        assert_eq!(v.expose(), "not-a-real-token-cli");
    }
}
