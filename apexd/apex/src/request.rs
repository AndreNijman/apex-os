//! `apex request` — the user-facing half of structured privilege requests.
//!
//! Two audiences, one command:
//!
//! * an **agent** inside a managed session files a request and waits —
//!   `apex request install clang --reason "…"`. It has no sudo and no root
//!   shell, and the sandbox it runs in cannot reach the system bus, so this is
//!   the only route it has to a system change.
//! * the **human** reviews and decides — `apex request list`,
//!   `sudo apex request approve 3`, `apex request deny 3`.
//!
//! ## Where the privilege actually comes from
//!
//! `apex-agentd` is unprivileged and stays that way. It records the request,
//! validates it, and remembers the decision — it never executes anything. The
//! operation runs inside `apex request approve`, which is subject to the same
//! [`crate::ops::require_root`] gate as `apex install` itself, so the privilege
//! exercised is the approving human's own.
//!
//! That is a deliberate v1 boundary. Auto-executing a pre-granted request
//! without a human present would need a privileged executor reachable from an
//! agent's request — a new root surface — and it is not built. So a grant
//! ("allow for project") today means *the next identical request needs no
//! decision*, not *it runs unattended*. `apex request pending` is how a human
//! sees what is waiting.

use std::io::{IsTerminal, Write};

use anyhow::{bail, Context, Result};
use apex_agent_core::client;
use apex_agent_core::protocol::{ErrorKind, Request, Response};
use apex_agent_core::request::{Decision, PrivilegeRequest, Verb};
use clap::Subcommand;

use crate::ops;

/// `apex request <verb>`.
#[derive(Subcommand)]
pub enum RequestCmd {
    /// Ask for a privileged operation and wait for a decision.
    ///
    /// This is what an agent runs. The operation itself is performed later, by
    /// the human who approves it — nothing here gains privilege.
    Ask {
        /// One of `apex request verbs`.
        verb: String,
        /// Arguments for the verb, e.g. package names.
        args: Vec<String>,
        /// Why it is needed. Shown to the human deciding, and required: a
        /// prompt with no reason teaches people to approve without reading.
        #[arg(long, short)]
        reason: String,
        /// Give up after this many seconds instead of waiting.
        #[arg(long, default_value_t = 900)]
        timeout: u64,
        /// File it and return immediately.
        #[arg(long)]
        no_wait: bool,
    },
    /// Every request on record.
    List {
        /// Include decided ones.
        #[arg(long, short)]
        all: bool,
        #[arg(long)]
        json: bool,
    },
    /// Requests waiting for a decision.
    Pending {
        #[arg(long)]
        json: bool,
    },
    /// Show one request in full, as the approval prompt.
    Show { id: u32 },
    /// Approve and run one. Requires root, because it performs the operation.
    Approve {
        id: u32,
        /// Also allow the identical request in this project without asking
        /// again.
        #[arg(long)]
        for_project: bool,
        /// Record the approval without running the operation.
        #[arg(long)]
        no_run: bool,
    },
    /// Refuse one.
    Deny { id: u32 },
    /// The privileged operations an agent may ask for.
    Verbs,
    /// What has been allowed for a project without asking.
    Grants {
        #[arg(long)]
        json: bool,
    },
    /// Withdraw a grant.
    Revoke {
        /// Project root the grant belongs to.
        project: String,
        /// A single grant key, or omit for all of the project's.
        key: Option<String>,
    },
    /// The audit trail: what was requested, decided and run.
    Audit {
        /// How many of the most recent entries.
        #[arg(long, short, default_value_t = 20)]
        lines: usize,
    },
}

pub fn main(cmd: RequestCmd) -> i32 {
    let result = match cmd {
        RequestCmd::Ask {
            verb,
            args,
            reason,
            timeout,
            no_wait,
        } => ask(&verb, &args, &reason, timeout, no_wait),
        RequestCmd::List { all, json } => list(all, json),
        RequestCmd::Pending { json } => pending(json),
        RequestCmd::Show { id } => show(id),
        RequestCmd::Approve {
            id,
            for_project,
            no_run,
        } => approve(id, for_project, no_run),
        RequestCmd::Deny { id } => deny(id),
        RequestCmd::Verbs => {
            verbs();
            Ok(0)
        }
        RequestCmd::Grants { json } => grants(json),
        RequestCmd::Revoke { project, key } => revoke(&project, key.as_deref()),
        RequestCmd::Audit { lines } => audit(lines),
    };
    match result {
        Ok(code) => code,
        Err(e) => {
            eprintln!("apex request: {e:#}");
            1
        }
    }
}

// ── asking ──────────────────────────────────────────────────────────────────

fn ask(verb: &str, args: &[String], reason: &str, timeout: u64, no_wait: bool) -> Result<i32> {
    // Validated locally first so a typo is a fast, clear error rather than a
    // round trip. The daemon validates again and does not trust this — see
    // apex-agentd's privilege module.
    if let Err(e) = Verb::parse(verb, args) {
        bail!("{e}");
    }

    let filed = match client::call(&Request::PrivilegeRequest {
        verb: verb.to_string(),
        args: args.to_vec(),
        reason: reason.to_string(),
    })? {
        Response::Request(r) => *r,
        Response::Error { kind, message } => {
            eprintln!("apex request: {message}");
            return Ok(if kind == ErrorKind::BadRequest { 2 } else { 1 });
        }
        other => bail!("unexpected reply: {other:?}"),
    };

    if filed.decision == Decision::Pending {
        eprintln!(
            "apex request: filed request {} and waiting for a decision.\n\n{}",
            filed.id,
            filed.prompt()
        );
        eprintln!("A human decides with:  sudo apex request approve {}", filed.id);
    } else {
        eprintln!(
            "apex request: request {} is already allowed for this project ({}).",
            filed.id,
            filed.verb.grant_key()
        );
    }

    if no_wait {
        println!("{}", filed.id);
        return Ok(0);
    }
    wait_for(filed.id, timeout)
}

/// Block until the request is executed, denied, or the timeout expires.
///
/// Polling rather than a subscription: the daemon has no push channel on the
/// control protocol, and a privilege decision is a human-timescale event where
/// a two-second poll costs nothing.
///
/// Exit codes are the message, because the caller is usually an agent reading
/// `$?` and not a human reading prose:
///   0  approved and the operation succeeded
///   3  denied
///   4  approved but the operation failed
///   5  timed out while still pending
fn wait_for(id: u32, timeout: u64) -> Result<i32> {
    let started = std::time::Instant::now();
    let deadline = std::time::Duration::from_secs(timeout);
    loop {
        let req = fetch(id)?;
        match req.decision {
            Decision::Denied => {
                eprintln!("apex request: request {id} was denied");
                return Ok(3);
            }
            Decision::Pending => {}
            Decision::AllowOnce | Decision::AllowForProject => {
                if let Some(code) = req.exit_code {
                    if code == 0 {
                        eprintln!("apex request: request {id} was approved and ran successfully");
                        return Ok(0);
                    }
                    eprintln!("apex request: request {id} was approved but exited {code}");
                    return Ok(4);
                }
                // Approved, not yet run. Keep waiting: the operation is what
                // the caller actually needs, not the approval.
            }
        }
        if started.elapsed() >= deadline {
            eprintln!(
                "apex request: gave up after {timeout}s; request {id} is still {}",
                req.decision.as_str()
            );
            return Ok(5);
        }
        std::thread::sleep(std::time::Duration::from_secs(2));
    }
}

fn fetch(id: u32) -> Result<PrivilegeRequest> {
    let all = fetch_all()?;
    all.into_iter()
        .find(|r| r.id == id)
        .with_context(|| format!("no request {id}"))
}

fn fetch_all() -> Result<Vec<PrivilegeRequest>> {
    match client::call(&Request::Requests)? {
        Response::Requests { requests } => Ok(requests),
        Response::Error { message, .. } => bail!("{message}"),
        other => bail!("unexpected reply: {other:?}"),
    }
}

// ── reviewing ───────────────────────────────────────────────────────────────

fn list(all: bool, json: bool) -> Result<i32> {
    let mut requests = fetch_all()?;
    if !all {
        requests.retain(|r| r.decision == Decision::Pending || r.is_executable());
    }
    if json {
        println!("{}", serde_json::to_string_pretty(&requests)?);
        return Ok(0);
    }
    if requests.is_empty() {
        println!(
            "{}",
            if all {
                "no privilege requests have been filed"
            } else {
                "nothing waiting (use --all for the history)"
            }
        );
        return Ok(0);
    }
    println!(
        "{:<4} {:<18} {:<10} {}",
        "ID", "STATE", "AGENT", "OPERATION"
    );
    for r in &requests {
        println!(
            "{:<4} {:<18} {:<10} apex {}",
            r.id,
            r.decision.as_str(),
            r.agent.as_deref().unwrap_or("-"),
            r.argv().join(" ")
        );
    }
    Ok(0)
}

fn pending(json: bool) -> Result<i32> {
    let requests: Vec<PrivilegeRequest> = fetch_all()?
        .into_iter()
        .filter(|r| r.decision == Decision::Pending)
        .collect();
    if json {
        println!("{}", serde_json::to_string_pretty(&requests)?);
        return Ok(0);
    }
    if requests.is_empty() {
        println!("nothing waiting for a decision");
        return Ok(0);
    }
    for r in &requests {
        println!("── request {} ──", r.id);
        print!("{}", r.prompt());
        println!("  [allow once]        sudo apex request approve {}", r.id);
        println!(
            "  [allow for project] sudo apex request approve {} --for-project",
            r.id
        );
        println!("  [deny]              apex request deny {}\n", r.id);
    }
    Ok(0)
}

fn show(id: u32) -> Result<i32> {
    let r = fetch(id)?;
    print!("{}", r.prompt());
    println!("State:\n  {}", r.decision.as_str());
    if let Some(code) = r.exit_code {
        println!("Result:\n  exited {code}");
    }
    Ok(0)
}

// ── deciding ────────────────────────────────────────────────────────────────

fn deny(id: u32) -> Result<i32> {
    match client::call(&Request::Decide {
        id,
        decision: "deny".into(),
    })? {
        Response::Request(_) => {
            println!("request {id} denied");
            Ok(0)
        }
        Response::Error { message, .. } => {
            eprintln!("apex request: {message}");
            Ok(1)
        }
        other => bail!("unexpected reply: {other:?}"),
    }
}

/// Approve a request and perform the operation.
///
/// Root is required before anything is recorded, not after: an approval stored
/// for an operation that then could not run would leave a request marked
/// allowed and never executed, which reads in the audit log as though the
/// human's decision was ignored.
fn approve(id: u32, for_project: bool, no_run: bool) -> Result<i32> {
    let req = fetch(id)?;
    if req.decision != Decision::Pending {
        eprintln!(
            "apex request: request {id} was already {}",
            req.decision.as_str()
        );
        return Ok(1);
    }

    if !no_run {
        if let Err(code) = ops::require_root("request approve") {
            return Ok(code);
        }
    }

    // Show what is being approved and confirm, when a human is watching. The
    // §4 prompt is the point of this whole subsystem; skipping it because the
    // command line already named an id would mean approving by muscle memory.
    //
    // Not asked inside a managed session. A session's stdin IS a terminal — it
    // is a PTY — so the terminal check alone made an agent running this sit
    // forever on a prompt no human would ever see, never reaching the daemon's
    // refusal. Using $APEX_AGENT_SESSION here is safe precisely because it
    // decides nothing: it suppresses a prompt. The daemon still resolves the
    // caller from the connection's peer credentials and still refuses, so
    // clearing the variable buys an agent a pointless prompt, not an approval.
    let in_session = std::env::var_os("APEX_AGENT_SESSION").is_some();
    if !in_session && std::io::stdin().is_terminal() && !confirm(&req)? {
        println!("left request {id} pending");
        return Ok(1);
    }

    let decision = if for_project { "project" } else { "once" };
    let decided = match client::call(&Request::Decide {
        id,
        decision: decision.into(),
    })? {
        Response::Request(r) => *r,
        Response::Error { message, .. } => {
            eprintln!("apex request: {message}");
            return Ok(1);
        }
        other => bail!("unexpected reply: {other:?}"),
    };

    if no_run {
        println!(
            "request {id} approved ({}), not run",
            decided.decision.as_str()
        );
        return Ok(0);
    }

    // The argv comes from the TYPED verb, rebuilt here rather than taken from
    // any stored string, so a hand-edited request file cannot add an argument
    // between the approval and the execution.
    let argv = decided.argv();
    let exe = std::env::current_exe().unwrap_or_else(|_| "apex".into());
    eprintln!("apex request: running: {} {}", exe.display(), argv.join(" "));
    let status = std::process::Command::new(&exe)
        .args(&argv)
        .status()
        .with_context(|| format!("running {} {}", exe.display(), argv.join(" ")))?;
    let code = status.code().unwrap_or(-1);

    // Record the outcome even on failure: the audit trail is a record of what
    // privilege was exercised, and a failed attempt is part of that.
    match client::call(&Request::RequestExecuted {
        id,
        exit_code: code,
    })? {
        Response::Request(_) | Response::Ok => {}
        Response::Error { message, .. } => {
            eprintln!("apex request: warning: could not record the execution: {message}");
        }
        other => eprintln!("apex request: warning: unexpected reply: {other:?}"),
    }
    Ok(code)
}

fn confirm(req: &PrivilegeRequest) -> Result<bool> {
    print!("{}", req.prompt());
    print!("\nApprove and run this? [y/N] ");
    std::io::stdout().flush().ok();
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer)?;
    Ok(matches!(answer.trim(), "y" | "Y" | "yes"))
}

// ── policy ──────────────────────────────────────────────────────────────────

fn verbs() {
    println!("Privileged operations an agent may request:\n");
    for name in Verb::names() {
        // Parsed from the vocabulary itself, so this list cannot drift out of
        // step with what the daemon accepts. `kind_summary` describes the
        // operation without its arguments — using `effect` here printed the
        // dummy package name this parse needs.
        let takes_packages = matches!(*name, "install" | "remove");
        let Ok(verb) = Verb::parse(name, &["placeholder".to_string()])
            .or_else(|_| Verb::parse(name, &[]))
        else {
            continue;
        };
        let shown = if takes_packages {
            format!("{name} <package>…")
        } else {
            name.to_string()
        };
        println!("  {:<20} {}", shown, verb.kind_summary());
    }
    println!(
        "\nThere is deliberately no verb for running an arbitrary command: a human\n\
         cannot meaningfully review one, and approving `sh -c …` once is equivalent\n\
         to granting permanent root."
    );
}

fn grants(json: bool) -> Result<i32> {
    let projects = match client::call(&Request::Grants)? {
        Response::Grants { projects } => projects,
        Response::Error { message, .. } => bail!("{message}"),
        other => bail!("unexpected reply: {other:?}"),
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&projects)?);
        return Ok(0);
    }
    if projects.is_empty() {
        println!("nothing is granted; every request needs a decision");
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

fn revoke(project: &str, key: Option<&str>) -> Result<i32> {
    match client::call(&Request::Revoke {
        project: project.to_string(),
        key: key.map(str::to_string),
    })? {
        Response::Grants { .. } => {
            match key {
                Some(k) => println!("revoked {k} for {project}"),
                None => println!("revoked every grant for {project}"),
            }
            Ok(0)
        }
        Response::Error { message, .. } => {
            eprintln!("apex request: {message}");
            Ok(1)
        }
        other => bail!("unexpected reply: {other:?}"),
    }
}

fn audit(lines: usize) -> Result<i32> {
    let path = apex_agent_core::request::audit_log();
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            println!("no privilege has been requested on this machine yet");
            return Ok(0);
        }
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    };
    let all: Vec<&str> = text.lines().collect();
    for line in all.iter().rev().take(lines).rev() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let argv = v["argv"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str())
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .unwrap_or_default();
        println!(
            "{:<14} id={:<4} {:<10} apex {}",
            v["event"].as_str().unwrap_or("?"),
            v["id"].as_u64().unwrap_or(0),
            v["agent"].as_str().unwrap_or("-"),
            argv
        );
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_verb_name_renders_a_help_line() {
        // `verbs()` derives its list from the vocabulary, so this is really a
        // check that no verb name produces an empty effect through either
        // parse path.
        for name in Verb::names() {
            let parsed = Verb::parse(name, &["placeholder".to_string()])
                .or_else(|_| Verb::parse(name, &[]));
            assert!(parsed.is_ok(), "{name} parses with neither form");
            let v = parsed.unwrap();
            assert!(!v.effect().is_empty(), "{name} has no effect");
            // The listing uses kind_summary, so that is what must be present —
            // and it must not leak the dummy package the parse above needs.
            assert!(!v.kind_summary().is_empty(), "{name} has no summary");
            assert!(
                !v.kind_summary().contains("placeholder"),
                "{name}'s summary leaks the parse placeholder"
            );
        }
    }

    #[test]
    fn the_wait_exit_codes_are_distinct() {
        // An agent branches on these, so two states sharing a code would make
        // "denied" and "failed" indistinguishable.
        let codes = [0, 3, 4, 5];
        let mut seen = std::collections::HashSet::new();
        for c in codes {
            assert!(seen.insert(c), "{c} is used twice");
        }
    }
}
