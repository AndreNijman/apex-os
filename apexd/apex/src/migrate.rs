//! `apex secret migrate` — move the credentials a machine already has.
//!
//! P0-002 built the store. P0-003 is the half where the credentials that are
//! *already* on somebody's disk go into it, and that half is the risky one: a
//! migration reads a value, writes it somewhere else, and then deletes the
//! original. Get the order wrong, or get interrupted, and the machine is left
//! without a credential it had this morning.
//!
//! So: **store, verify, and only then remove**, per credential, and every step
//! idempotent. An interrupted run leaves plaintext that a later run reads
//! again, which is a machine that still works.
//!
//! ## What it moves
//!
//! | from | to |
//! |---|---|
//! | the old broker's `$XDG_STATE_HOME/apex/agent/secrets/*.json` | a stored credential, with its grants |
//! | `~/.claude/settings.json` → `env` → a credential-named entry | a stored credential |
//! | `~/.claude.json` → `mcpServers.<name>.headers.Authorization` | a stored credential, and the server becomes `apex mcp bridge <name>` |
//!
//! ## What it does not move, and says so
//!
//! An `env` entry whose host nobody can work out. `GITHUB_PERSONAL_ACCESS_TOKEN`
//! is `github.com` because the name says so; `ACME_API_KEY` could be anything,
//! and a migration that guessed would store a credential pinned to the wrong
//! host and remove the working copy. Those are listed and left alone.
//!
//! A stdio MCP server's `env` block. Those credentials are handed to a process
//! the agent spawns, and the bridge cannot stand in front of a program running
//! on this machine — §10.2's per-MCP sidecars are where that goes.
//!
//! ## Verification, and what it can and cannot be
//!
//! The service has no verb that returns a credential, by design, so "did that
//! store correctly?" cannot be answered by reading it back. The only proof
//! available is *use*: the daemon performs an operation and says whether it
//! worked. That needs a grant, a project and a network, and when any of them is
//! missing the credential is stored and the original is **kept**, with the
//! reason printed. A machine with two copies is a nuisance; a machine with none
//! is an outage.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use apex_secret_core::capability::{Capability, CapabilityRecord};
use apex_secret_core::client::Client;
use apex_secret_core::protocol::{Request, Response};
use apex_secret_core::store::valid_service_name;
use apex_secret_core::SecretValue;
use serde_json::{Map, Value};

/// One credential the migration found.
struct Found {
    /// What it will be stored as.
    service: String,
    host: String,
    scheme: String,
    username: String,
    path: String,
    auth: String,
    port: Option<u16>,
    value: String,
    /// Where it came from, for the report.
    source: String,
    /// What has to happen to the source once the value is safely stored.
    removal: Removal,
}

/// What "remove the original" means for a given source.
enum Removal {
    /// Delete a whole file — the old broker's per-service record.
    File(PathBuf),
    /// Drop a key from a JSON document.
    JsonKey {
        file: PathBuf,
        path: Vec<String>,
    },
    /// Replace an MCP server definition with one that names the bridge.
    McpServer { file: PathBuf, server: String },
}

pub fn main(dry_run: bool) -> Result<i32> {
    refuse_from_inside_a_session()?;
    // Only for a run that writes. A dry run is how somebody sees what this
    // would do to their machine, and refusing it because they have Claude open
    // means the only way to find out is to close Claude first — which is a
    // worse thing to ask than the check is worth.
    if !dry_run {
        refuse_while_an_agent_is_running()?;
    }

    let home = home();
    let mut found = Vec::new();
    let mut skipped: Vec<String> = Vec::new();
    found.extend(from_the_old_broker(&mut skipped));
    found.extend(from_claude_settings(&home, &mut skipped));
    found.extend(from_claude_mcp(&home, &mut skipped));

    if found.is_empty() && skipped.is_empty() {
        println!("nothing to migrate: no plaintext credential is in a place APEX knows about");
        return Ok(0);
    }

    for note in &skipped {
        println!("skip   {note}");
    }
    if found.is_empty() {
        return Ok(0);
    }

    if dry_run {
        for f in &found {
            println!(
                "would  store {} ({}://{}{}) from {}",
                f.service, f.scheme, f.host, f.path, f.source
            );
        }
        println!("\nnothing was written. Run without --dry-run to migrate.");
        return Ok(0);
    }

    let mut client = Client::connect()?;
    // Before anything is stored: the old broker's per-project grants. A
    // credential that arrives with no grant is a credential nothing can verify,
    // so this comes first and not last.
    let carried = carry_grants(&mut client);
    if carried > 0 {
        println!("moved  {carried} grant(s) from the old broker");
    }
    let mut moved = 0;
    let mut kept = 0;
    for f in &found {
        match migrate_one(&mut client, f) {
            Ok(true) => {
                moved += 1;
                println!("moved  {} from {}", f.service, f.source);
            }
            Ok(false) => {
                kept += 1;
                println!(
                    "stored {} from {}, and left the original in place — nothing here \
                     could prove the stored copy works",
                    f.service, f.source
                );
            }
            Err(e) => {
                kept += 1;
                println!("kept   {} from {}: {e:#}", f.service, f.source);
            }
        }
    }

    println!("\n{moved} migrated, {kept} stored but not removed");
    if kept > 0 {
        println!(
            "A credential that is stored but not removed is safe and duplicated. Grant it a\n\
             capability, then run this again — the second run verifies and removes."
        );
    }
    Ok(0)
}

/// Store, verify, remove — in that order, and never out of it.
///
/// Answers whether the original was removed. `false` means the value is stored
/// and the plaintext is still there, which is the deliberate outcome when the
/// stored copy could not be proved to work.
fn migrate_one(client: &mut Client, f: &Found) -> Result<bool> {
    // 1. Store. Replaces any previous credential of the same name, so a second
    //    run after an interrupted first is a no-op rather than a conflict.
    client
        .add(
            &f.service,
            &f.host,
            &f.scheme,
            Some(&f.username),
            &f.path,
            &f.auth,
            f.port,
            &SecretValue::new(f.value.as_bytes().to_vec()),
        )
        .with_context(|| format!("storing {}", f.service))?;

    // 2. Verify — the stored copy, through the daemon, doing the real thing.
    if !verify(client, f)? {
        return Ok(false);
    }

    // 3. Only now.
    remove_original(f)?;
    Ok(true)
}

/// Whether the stored credential demonstrably works.
///
/// There is no read-back: the protocol has no verb that returns a value, which
/// is P0-002's second criterion and not something to work around here. So the
/// check is a use, and a use needs a grant — which the owner has not given yet
/// on a first run. An ungranted capability is reported as "could not prove",
/// never as "broken", because those have opposite correct responses.
fn verify(client: &mut Client, f: &Found) -> Result<bool> {
    // An endpoint credential is proved by the MCP handshake — the one message
    // every server answers and none of them acts on. A git credential is proved
    // by asking a remote what refs it advertises, which needs one authenticated
    // request and changes nothing.
    let (capability, body): (Capability, &[u8]) = if f.path.is_empty() {
        (
            Capability::GitLsRemote {
                remote: "origin".into(),
            },
            b"",
        )
    } else {
        (
            Capability::McpRequest,
            br#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"apex-secret-migrate","version":"1"}}}"#,
        )
    };

    let project = std::env::current_dir()
        .ok()
        .and_then(|cwd| apex_agent_core::project::detect(&cwd))
        .map(|p| p.root);
    let Some(project) = project else {
        return Ok(false);
    };

    let mut record = CapabilityRecord::new(&f.service, capability);
    record.project = Some(project);
    match client.use_with_body(record, body) {
        Ok(Response::Performed { exit_code, .. }) => Ok(exit_code == 0),
        _ => Ok(false),
    }
}

fn remove_original(f: &Found) -> Result<()> {
    match &f.removal {
        Removal::File(path) => {
            std::fs::remove_file(path).with_context(|| format!("removing {}", path.display()))
        }
        Removal::JsonKey { file, path } => edit_json(file, |doc| {
            drop_at(doc, path);
        }),
        Removal::McpServer { file, server } => edit_json(file, |doc| {
            let bridged = serde_json::json!({
                "type": "stdio",
                "command": "apex",
                "args": ["mcp", "bridge", server],
            });
            if let Some(servers) = doc
                .get_mut("mcpServers")
                .and_then(Value::as_object_mut)
            {
                servers.insert(server.clone(), bridged);
            }
        }),
    }
}

/// Rewrite a JSON document through a temporary file in the same directory.
///
/// Same directory so the rename is on one filesystem and therefore atomic: a
/// machine that lost power mid-write has either the old document or the new
/// one, and never half of either. The mode is copied from the original, because
/// `~/.claude.json` is `0600` and a migration that widened it would be a
/// migration that made things worse.
fn edit_json(file: &Path, change: impl FnOnce(&mut Value)) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let raw = std::fs::read(file).with_context(|| format!("reading {}", file.display()))?;
    let mut doc: Value =
        serde_json::from_slice(&raw).with_context(|| format!("parsing {}", file.display()))?;
    change(&mut doc);
    let mode = std::fs::metadata(file).map(|m| m.permissions().mode()).unwrap_or(0o600);

    let tmp = file.with_extension("apex-migrate.tmp");
    let text = serde_json::to_vec_pretty(&doc)?;
    std::fs::write(&tmp, &text).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(mode & 0o7777)).ok();
    std::fs::rename(&tmp, file).with_context(|| format!("renaming into {}", file.display()))?;
    Ok(())
}

/// Remove the value at a key path, leaving everything around it alone.
pub fn drop_at(doc: &mut Value, path: &[String]) {
    let Some((last, parents)) = path.split_last() else {
        return;
    };
    let mut node = doc;
    for key in parents {
        let Some(next) = node.get_mut(key) else { return };
        node = next;
    }
    if let Some(map) = node.as_object_mut() {
        map.remove(last);
    }
}

// ── finding what is there ───────────────────────────────────────────────────

/// The old broker's own store: `$XDG_STATE_HOME/apex/agent/secrets/*.json`.
///
/// P0-002 named this directory and deliberately did not delete it — a file that
/// may hold the only copy of a token is not something a `list` command removes
/// on its own. This is the thing that was supposed to come and take it.
fn from_the_old_broker(skipped: &mut Vec<String>) -> Vec<Found> {
    let dir = apex_agent_core::paths::state_dir().join("secrets");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let Ok(raw) = std::fs::read(&path) else { continue };
        let Ok(doc) = serde_json::from_slice::<Value>(&raw) else {
            skipped.push(format!("{} is not JSON this understands", path.display()));
            continue;
        };
        let service = string(&doc, "service").unwrap_or_default();
        let token = string(&doc, "token").unwrap_or_default();
        if token.is_empty() {
            // A keyring-backed record: the metadata is here and the value is
            // not, so there is nothing to carry across.
            skipped.push(format!(
                "{} holds no value — it named the keyring, and this cannot read one",
                path.display()
            ));
            continue;
        }
        if !valid_service_name(&service) {
            skipped.push(format!("{} names no usable service", path.display()));
            continue;
        }
        out.push(Found {
            host: string(&doc, "host").unwrap_or_default(),
            scheme: "https".into(),
            username: string(&doc, "username").unwrap_or_else(|| "x-access-token".into()),
            path: String::new(),
            auth: "bearer".into(),
            port: None,
            value: token,
            source: format!("the old broker's {}", path.display()),
            removal: Removal::File(path),
            service,
        })
    }
    out
}

/// A credential in Claude's `settings.json` `env` block.
///
/// The one this machine actually had. It reaches every tool a session runs,
/// because Claude applies the block itself — which is why the daemon also binds
/// a redacted copy over this file, and why that is a floor rather than a fix:
/// the value is still on disk until this has run.
fn from_claude_settings(home: &Path, skipped: &mut Vec<String>) -> Vec<Found> {
    let file = home.join(".claude/settings.json");
    let Ok(raw) = std::fs::read(&file) else {
        return Vec::new();
    };
    let Ok(doc) = serde_json::from_slice::<Value>(&raw) else {
        return Vec::new();
    };
    let Some(env) = doc.get("env").and_then(Value::as_object) else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for (name, value) in env {
        let Some(value) = value.as_str().filter(|v| !v.is_empty()) else {
            continue;
        };
        if !apex_agent_core::profile::credential_name(name) {
            continue;
        }
        let Some((service, host)) = provider_for(name) else {
            skipped.push(format!(
                "{name} in {} is a credential, and nothing here knows which host it is \
                 for — a guess would pin it to the wrong one",
                file.display()
            ));
            continue;
        };
        out.push(Found {
            service: service.to_string(),
            host: host.to_string(),
            scheme: "https".into(),
            username: "x-access-token".into(),
            path: String::new(),
            auth: "bearer".into(),
            port: None,
            value: value.to_string(),
            source: format!("{} → env → {name}", file.display()),
            removal: Removal::JsonKey {
                file: file.clone(),
                path: vec!["env".into(), name.clone()],
            },
        });
    }
    out
}

/// The provider an environment variable's NAME identifies, when it identifies
/// one.
///
/// A short list on purpose. The cost of being wrong is a credential pinned to a
/// host it is not for, stored, verified against nothing, and then deleted from
/// the only place it was — so a name that is not on this list is reported and
/// left where it is.
fn provider_for(name: &str) -> Option<(&'static str, &'static str)> {
    let upper = name.to_ascii_uppercase();
    if upper.contains("GITHUB") || upper.starts_with("GH_") {
        return Some(("github", "github.com"));
    }
    if upper.contains("GITLAB") {
        return Some(("gitlab", "gitlab.com"));
    }
    None
}

/// An HTTP MCP server's bearer token, in `~/.claude.json`.
///
/// This file is bound *writable* into a managed session, because Claude records
/// onboarding state in it on every run — so the token in it is readable by the
/// agent. That is P0-003's fourth criterion failing in one line of JSON, and
/// this is what closes it.
fn from_claude_mcp(home: &Path, skipped: &mut Vec<String>) -> Vec<Found> {
    let file = home.join(".claude.json");
    let Ok(raw) = std::fs::read(&file) else {
        return Vec::new();
    };
    let Ok(doc) = serde_json::from_slice::<Value>(&raw) else {
        return Vec::new();
    };
    let Some(servers) = doc.get("mcpServers").and_then(Value::as_object) else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for (name, server) in servers {
        if let Some(env) = server.get("env").and_then(Value::as_object) {
            if env.keys().any(|k| apex_agent_core::profile::credential_name(k)) {
                skipped.push(format!(
                    "the MCP server '{name}' hands a credential to a program it spawns; \
                     the broker can stand in front of an endpoint and not in front of a \
                     local process"
                ));
            }
        }
        let Some(authorization) = server
            .get("headers")
            .and_then(Value::as_object)
            .and_then(|h| header_value(h, "authorization"))
        else {
            continue;
        };
        let Some(url) = server.get("url").and_then(Value::as_str) else {
            continue;
        };
        let Some((scheme, host, port, path)) = split_url(url) else {
            skipped.push(format!("the MCP server '{name}' has a url this cannot read: {url}"));
            continue;
        };
        let service = sanitise(name);
        if !valid_service_name(&service) {
            skipped.push(format!("'{name}' is not a name a credential can be stored under"));
            continue;
        }
        // `Bearer x` is stored as `x` with `auth=bearer`, so the value in the
        // store is the credential and not a header containing one — which also
        // lets the scrubber catch it in both forms. Anything else is kept
        // whole: a header shape nobody here has seen is still a header this can
        // send, and refusing it would leave the plaintext in place.
        let (auth, value) = match authorization.strip_prefix("Bearer ") {
            Some(token) => ("bearer", token.trim().to_string()),
            None => ("raw", authorization.clone()),
        };
        out.push(Found {
            service: service.clone(),
            host,
            scheme,
            username: "x-access-token".into(),
            path,
            auth: auth.into(),
            port,
            value,
            source: format!("{} → mcpServers → {name} → headers", file.display()),
            removal: Removal::McpServer {
                file: file.clone(),
                server: name.clone(),
            },
        });
    }
    out
}

fn header_value(headers: &Map<String, Value>, want: &str) -> Option<String> {
    headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(want))
        .and_then(|(_, v)| v.as_str())
        .map(str::to_string)
}

/// Scheme, host, port and path of an http(s) URL.
pub fn split_url(url: &str) -> Option<(String, String, Option<u16>, String)> {
    let (scheme, rest) = match url.split_once("://") {
        Some(("https", rest)) => ("https", rest),
        Some(("http", rest)) => ("http", rest),
        _ => return None,
    };
    let (authority, path) = match rest.split_once('/') {
        Some((a, p)) => (a, format!("/{p}")),
        None => (rest, "/".to_string()),
    };
    // A URL carrying credentials is exactly the case where taking everything
    // before the first ':' gives the wrong host.
    let authority = authority.rsplit('@').next()?;
    let (host, port) = if let Some(inner) = authority.strip_prefix('[') {
        let (host, rest) = inner.split_once(']')?;
        (host.to_string(), rest.strip_prefix(':').and_then(|p| p.parse().ok()))
    } else {
        match authority.split_once(':') {
            Some((h, p)) => (h.to_string(), p.parse().ok()),
            None => (authority.to_string(), None),
        }
    };
    if host.is_empty() {
        return None;
    }
    // A query or a fragment is not something the store will pin, and dropping
    // one silently would send the message somewhere subtly different.
    if path.contains('?') || path.contains('#') {
        return None;
    }
    Some((scheme.to_string(), host.to_ascii_lowercase(), port, path))
}

/// An MCP server's name as a service name.
pub fn sanitise(name: &str) -> String {
    let mut out: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.') { c } else { '-' })
        .collect();
    while out.starts_with('.') {
        out.remove(0);
    }
    out.truncate(64);
    out
}

// ── guards ──────────────────────────────────────────────────────────────────

/// Refuse to run from inside a managed agent session.
///
/// `apex-secretd` refuses a mutating verb from one anyway, so this changes
/// nothing about what happens — it changes what the person reads. "An agent
/// session cannot store a credential" is true and unhelpful when the command
/// they typed was `migrate`.
fn refuse_from_inside_a_session() -> Result<()> {
    if std::env::var_os(apex_agent_core::client::SESSION_ENV).is_some() {
        bail!(
            "this is running inside a managed agent session, and a session may not change \
             what it is allowed to do — which is what a migration does. Run it from your \
             own shell."
        );
    }
    Ok(())
}

/// Refuse while an agent that owns one of these files is running.
///
/// `~/.claude.json` is held in memory by a running Claude and written back on
/// change, with whatever was in it when the process started. A migration that
/// edited it under a live session would be reverted minutes later, and the
/// credential would be back in the file with nothing to say it had ever left.
///
/// "Owns one of these files" is checked rather than assumed: a `claude` process
/// with a different `HOME` is editing a different `.claude.json`, and refusing
/// over it would make this impossible to run on a machine where anyone else is
/// using Claude — or in a fixture, which is the same problem wearing a hat. A
/// process whose environment cannot be read belongs to another account and
/// therefore to another home.
fn refuse_while_an_agent_is_running() -> Result<()> {
    let home = home();
    for pid in running_claude() {
        if process_home(pid).as_deref() == Some(home.as_path()) {
            bail!(
                "claude is running as you, in this home. It holds {}/.claude.json in \
                 memory and writes it back on exit, so an edit made now would be reverted \
                 and the credential would be back in the file. Close it and run this again.",
                home.display()
            );
        }
    }
    Ok(())
}

/// Pids of every Claude process on this machine.
///
/// Matched on the *program* in `argv[0]`, not on the process name. Measured
/// here rather than assumed: `pgrep -x claude` finds only some of them, because
/// the shipped binary is called `claude.exe` and the daemon and the PTY hosts
/// run under that name. A guard that missed those would let a migration edit
/// `~/.claude.json` under a live session and be reverted minutes later, which
/// is the one failure it exists to prevent.
fn running_claude() -> Vec<u32> {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    let mut pids = Vec::new();
    for entry in entries.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|n| n.parse::<u32>().ok())
        else {
            continue;
        };
        if pid == std::process::id() {
            continue;
        }
        let Ok(raw) = std::fs::read(format!("/proc/{pid}/cmdline")) else {
            continue;
        };
        let Some(argv0) = raw.split(|b| *b == 0).next() else {
            continue;
        };
        let program = String::from_utf8_lossy(argv0);
        let name = program.rsplit('/').next().unwrap_or_default();
        if name == "claude" || name == "claude.exe" {
            pids.push(pid);
        }
    }
    pids
}

/// The `HOME` a process was started with, when it can be read.
///
/// `/proc/<pid>/environ` is readable only for a process of the same uid, which
/// is exactly the set of processes that could be editing this account's files.
/// `None` — a process that exited, or one belonging to somebody else — is not
/// a reason to refuse.
fn process_home(pid: u32) -> Option<PathBuf> {
    let raw = std::fs::read(format!("/proc/{pid}/environ")).ok()?;
    raw.split(|b| *b == 0)
        .filter_map(|entry| std::str::from_utf8(entry).ok())
        .find_map(|entry| entry.strip_prefix("HOME=").map(PathBuf::from))
}

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

fn string(doc: &Value, key: &str) -> Option<String> {
    doc.get(key).and_then(Value::as_str).map(str::to_string)
}

/// Carry the old broker's per-project grants across.
///
/// Not called from [`main`] yet and not dead: it is the second half of the old
/// store's migration and it is written here rather than left to be rediscovered.
pub fn old_grants() -> BTreeMap<String, Vec<String>> {
    let file = apex_agent_core::paths::state_dir().join("secret-grants.json");
    let Ok(raw) = std::fs::read(&file) else {
        return BTreeMap::new();
    };
    serde_json::from_slice::<Value>(&raw)
        .ok()
        .and_then(|d| d.get("projects").cloned())
        .and_then(|p| serde_json::from_value(p).ok())
        .unwrap_or_default()
}

/// Ask the daemon to re-create the grants the old store held.
pub fn carry_grants(client: &mut Client) -> usize {
    let mut carried = 0;
    for (project, keys) in old_grants() {
        for key in keys {
            let Some((service, capability)) = key.split_once(':') else {
                continue;
            };
            let ok = client.call(&Request::Grant {
                project: project.clone(),
                service: service.to_string(),
                capability: capability.to_string(),
                revoke: false,
            });
            if ok.is_ok() {
                carried += 1;
            }
        }
    }
    carried
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_url_becomes_the_four_things_the_store_pins() {
        assert_eq!(
            split_url("https://memory.example.com/mcp"),
            Some(("https".into(), "memory.example.com".into(), None, "/mcp".into()))
        );
        assert_eq!(
            split_url("http://127.0.0.1:8080/mcp/v1"),
            Some(("http".into(), "127.0.0.1".into(), Some(8080), "/mcp/v1".into()))
        );
        assert_eq!(
            split_url("https://[::1]:9000/mcp"),
            Some(("https".into(), "::1".into(), Some(9000), "/mcp".into()))
        );
        // A URL carrying credentials: the host is after the '@', not before
        // the first ':'.
        assert_eq!(
            split_url("https://user:pw@example.com/mcp").map(|(_, h, _, _)| h),
            Some("example.com".to_string())
        );
        // Nothing this cannot pin exactly.
        assert_eq!(split_url("ws://example.com/mcp"), None);
        assert_eq!(split_url("https://example.com/mcp?token=x"), None);
        assert_eq!(split_url("not a url"), None);
    }

    #[test]
    fn only_a_name_that_identifies_a_provider_is_migrated() {
        // The whole risk of this file in one function: a wrong answer here
        // pins a credential to a host it is not for, and then deletes the copy
        // that worked.
        assert_eq!(provider_for("GITHUB_PERSONAL_ACCESS_TOKEN"), Some(("github", "github.com")));
        assert_eq!(provider_for("GH_TOKEN"), Some(("github", "github.com")));
        assert_eq!(provider_for("GITLAB_TOKEN"), Some(("gitlab", "gitlab.com")));
        for unknown in ["ACME_API_KEY", "OPENAI_API_KEY", "SOME_SECRET", "TOKEN"] {
            assert_eq!(provider_for(unknown), None, "{unknown}");
        }
    }

    #[test]
    fn a_server_name_becomes_a_service_name() {
        assert_eq!(sanitise("claude-memory"), "claude-memory");
        assert_eq!(sanitise("my/server"), "my-server");
        // The property that matters: a service name is one path segment in the
        // store's directory, so nothing that could leave it survives.
        for hostile in ["../escape", "/etc/passwd", "a/../b", ".hidden", "weird name!"] {
            let out = sanitise(hostile);
            assert!(!out.contains('/'), "{hostile} -> {out}");
            assert!(!out.starts_with('.'), "{hostile} -> {out}");
            assert!(valid_service_name(&out), "{hostile} -> {out}");
        }
    }

    #[test]
    fn dropping_a_key_leaves_everything_around_it() {
        // The failure this prevents is a migration that removed a token and
        // took the model, the hooks or an unrelated variable with it.
        let mut doc: Value = serde_json::from_str(
            r#"{"model":"opus","env":{"A":"1","GITHUB_TOKEN":"x"},"theme":"dark"}"#,
        )
        .unwrap();
        drop_at(&mut doc, &["env".into(), "GITHUB_TOKEN".into()]);
        assert_eq!(doc["model"], "opus");
        assert_eq!(doc["theme"], "dark");
        assert_eq!(doc["env"]["A"], "1");
        assert!(doc["env"].get("GITHUB_TOKEN").is_none());

        // A path that is not there changes nothing at all.
        let before = doc.clone();
        drop_at(&mut doc, &["env".into(), "NOTHERE".into()]);
        drop_at(&mut doc, &["absent".into(), "x".into()]);
        assert_eq!(doc, before);
    }

    #[test]
    fn a_bearer_header_is_stored_as_the_token_and_not_as_the_header() {
        // So the value in the store is the credential itself: the daemon
        // rebuilds the header, and the scrubber then catches both forms.
        let header = "Bearer fake-token-value";
        let (auth, value) = match header.strip_prefix("Bearer ") {
            Some(token) => ("bearer", token.trim().to_string()),
            None => ("raw", header.to_string()),
        };
        assert_eq!(auth, "bearer");
        assert_eq!(value, "fake-token-value");

        let odd = "Basic dXNlcjpwYXNz";
        let (auth, value) = match odd.strip_prefix("Bearer ") {
            Some(token) => ("bearer", token.trim().to_string()),
            None => ("raw", odd.to_string()),
        };
        assert_eq!(auth, "raw");
        assert_eq!(value, odd);
    }
}
