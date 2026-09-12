//! `apex mcp connect` — authenticate an MCP server once, through the broker.
//!
//! §10.1 asks for helper-based authentication "so long-lived bearer tokens are
//! not stored in agent-readable config". P0-003 built the helper and a
//! migration for the tokens that were already on disk. This is the third piece:
//! the verb a person uses for a server they are setting up *now*, so that a
//! token never reaches `~/.claude.json` in the first place.
//!
//! ```text
//! printf %s "$TOKEN" | apex mcp connect claude-memory
//! ```
//!
//! ## The order, and why it is the same one the migration uses
//!
//! Store, prove, and only then rewrite. A run interrupted between any two steps
//! leaves a machine that still works: the stored copy is a duplicate until the
//! rewrite happens, and the rewrite happens only after the far end has answered
//! a real MCP handshake with the stored credential attached.
//!
//! "Prove" needs a grant, because `apex-secretd` refuses an ungranted operation
//! to the owner exactly as it refuses one to an agent — the grant table is not a
//! thing the owner is outside of. So this grants `mcp.request` for the project
//! it is run in, which is what the person asking to connect a server here is
//! asking for, and says so on stdout. If the proof then fails, the grant is
//! withdrawn again: a machine that ends a failed command with a permission it
//! did not start with is a machine nobody can reason about.
//!
//! ## What it will not do
//!
//! **Edit a definition that is not the user's.** A plugin's `.mcp.json` is
//! replaced whenever the plugin updates and a repository's belongs to whoever
//! wrote the repository. An edit to either is undone without warning, and what
//! comes back is the credential. Those are refused with the commands that do
//! work.
//!
//! **Stand in front of a local program.** A stdio server's credential is handed
//! to a process on this machine, and a broker that holds an endpoint's token has
//! nothing to offer it. That is P1-019's subject, not this one.

use std::io::Read;
use std::path::Path;

use anyhow::{bail, Context, Result};
use apex_secret_core::capability::CapabilityRecord;
use apex_secret_core::client::Client;
use apex_secret_core::protocol::{Request, Response};
use apex_secret_core::store::valid_service_name;
use apex_secret_core::SecretValue;
use serde_json::Value;

use super::servers::{self, Credential, Server, Surface, Transport};

/// The operation a brokered MCP server is used through.
const OPERATION: &str = "mcp.request";

/// The message the far end answers without acting on anything.
///
/// `initialize` is the one call every MCP server must answer, and answering it
/// changes nothing the person will have to undo. The same probe
/// `apex secret migrate` verifies with, for the same reason.
const HANDSHAKE: &[u8] = br#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"apex-mcp-connect","version":"1"}}}"#;

pub fn main(name: &str, url: Option<&str>, service: Option<&str>, dry_run: bool) -> Result<i32> {
    crate::migrate::refuse_from_inside_a_session()?;
    if !dry_run {
        crate::migrate::refuse_while_an_agent_is_running()?;
    }

    let home = super::home();
    let cwd = std::env::current_dir().ok();
    let found = servers::discover(&home, cwd.as_deref());
    let existing = found.iter().find(|s| s.name == name);

    let plan = plan(name, url, service, existing, &found)?;
    if dry_run {
        println!(
            "would store '{}' for {}://{}{} and rewrite {} to name the bridge",
            plan.service,
            plan.scheme,
            plan.host,
            plan.path,
            plan.file_says()
        );
        println!("\nnothing was written. Run without --dry-run to connect.");
        return Ok(0);
    }

    let value = read_credential(&plan)?;
    let mut client = Client::connect()?;

    // 1. Store. Replaces any credential of the same name, so a second run after
    //    an interrupted first is a repeat rather than a conflict.
    client
        .add(
            &plan.service,
            &plan.host,
            &plan.scheme,
            Some("x-access-token"),
            &plan.path,
            "bearer",
            plan.port,
            &SecretValue::new(value.into_bytes()),
        )
        .with_context(|| format!("storing {}", plan.service))?;
    println!("stored  {} for {}://{}{}", plan.service, plan.scheme, plan.host, plan.path);

    // 2. Prove it, which needs a project and a grant.
    let Some(project) = project_root() else {
        println!(
            "The credential is stored and {} still holds the old one: nothing here could \
             prove the stored copy works, because this directory is not inside a project \
             and a capability is granted per project. Run this again from the project you \
             want the server in.",
            plan.file_says()
        );
        return Ok(0);
    };

    let granted_here = already_granted(&mut client, &project, &plan.service);
    if !granted_here {
        grant(&mut client, &project, &plan.service, false)?;
        println!("granted {OPERATION} on {} in {project}", plan.service);
    }

    match probe(&mut client, &plan.service, &project) {
        Ok(()) => {}
        Err(e) => {
            if !granted_here {
                // Leave the machine as it was found. A failed command that
                // silently widened a permission is worse than one that failed.
                grant(&mut client, &project, &plan.service, true).ok();
            }
            println!(
                "The credential is stored and {} was left alone, so the server keeps working \
                 the way it did.",
                plan.file_says()
            );
            bail!("{e:#}");
        }
    }
    println!("proved  the server answered an MCP handshake with the stored credential");

    // 3. Only now is anything the agent reads allowed to change.
    write_bridge(&home, &plan)?;
    println!("wrote   {} — it now names `apex mcp bridge {}`", plan.file_says(), plan.service);
    if plan.replaced_a_credential {
        println!("removed the credential that was in it");
    }
    Ok(0)
}

/// What a connect will do, worked out before anything is written.
#[derive(Debug)]
struct Plan {
    /// The MCP server's key in `~/.claude.json`.
    key: String,
    /// The stored credential's name.
    service: String,
    scheme: String,
    host: String,
    port: Option<u16>,
    path: String,
    /// Whether the definition being replaced held a credential.
    replaced_a_credential: bool,
    /// The per-directory block this server is defined in, when it is.
    directory: Option<String>,
}

impl Plan {
    fn file_says(&self) -> String {
        match &self.directory {
            Some(dir) => format!("~/.claude.json (in {dir})"),
            None => "~/.claude.json".to_string(),
        }
    }
}

/// Work out what to do, and refuse everything that cannot be done honestly.
fn plan(
    name: &str,
    url: Option<&str>,
    service: Option<&str>,
    existing: Option<&Server>,
    found: &[Server],
) -> Result<Plan> {
    let url = match (url, existing) {
        (Some(url), _) => url.to_string(),
        (
            None,
            Some(Server {
                transport: Transport::Endpoint { url, .. },
                ..
            }),
        ) => url.clone(),
        (
            None,
            Some(Server {
                transport: Transport::Stdio { command, .. },
                ..
            }),
        ) => bail!(
            "'{name}' is a program this machine runs ({command}), not an endpoint. The \
             broker holds a credential and makes a request; it has nothing to stand in \
             front of a local process with. Confine it instead:\n  \
             apex mcp run {name}"
        ),
        (None, Some(server)) => bail!(
            "'{name}' is defined with no address this understands ({:?})",
            server.transport
        ),
        (None, None) => bail!(
            "no MCP server called '{name}' is defined here, and no --url was given.\n  \
             apex mcp list                         what this machine has\n  \
             apex mcp connect {name} --url <URL>   connect one it does not"
        ),
    };

    if let Some(server) = existing {
        if !server.surface.is_writable_here() {
            bail!("{}", refuse_foreign_surface(server, &url));
        }
    }

    let (scheme, host, port, path) = crate::migrate::split_url(&url)
        .with_context(|| format!("'{url}' is not an http or https URL this can pin"))?;
    if path.is_empty() || path == "/" {
        // The provider refuses a credential with no endpoint path at use time,
        // which would be a failure two steps later about a URL typed here.
        bail!(
            "'{url}' names a host and no path, so there is nowhere to carry a message to. \
             An MCP endpoint is usually the host plus /mcp."
        );
    }

    let service = match service {
        Some(explicit) => explicit.to_string(),
        // A plugin's name is `plugin:<p>:<server>`, and the last segment is the
        // part a person would recognise. Sanitised either way, because a
        // service name is one segment of a path in the store.
        None => crate::migrate::sanitise(name.rsplit(':').next().unwrap_or(name)),
    };
    if !valid_service_name(&service) {
        bail!("'{service}' is not a usable credential name (letters, digits, _ - .)");
    }
    if let Some(other) = found.iter().find(|s| {
        s.name != name
            && matches!(&s.credential, Credential::Brokered { service: s } if *s == service)
    }) {
        bail!(
            "'{service}' is already the credential behind '{}'. Storing this one under the \
             same name would replace it. Choose another with --service.",
            other.name
        );
    }

    Ok(Plan {
        key: existing.map(|s| s.key.clone()).unwrap_or_else(|| name.to_string()),
        service,
        scheme,
        host,
        port,
        path,
        replaced_a_credential: existing.is_some_and(|s| s.credential.agent_readable()),
        directory: existing.and_then(|s| match &s.surface {
            Surface::Directory(dir) => Some(dir.clone()),
            _ => None,
        }),
    })
}

/// The refusal for a definition that is not the user's to edit, with the two
/// commands that do work.
///
/// Written out rather than shortened to "not supported", because the person
/// reading it has a server that does not authenticate and no way to tell from
/// the agent's own error — "Authorization header is badly formatted" — that the
/// definition is a plugin's.
fn refuse_foreign_surface(server: &Server, url: &str) -> String {
    let own = server.name.rsplit(':').next().unwrap_or(&server.name);
    match &server.surface {
        Surface::Plugin { plugin, file } => format!(
            "'{}' is defined by the '{plugin}' plugin, in {}. That file is replaced whenever \
             the plugin updates, so a credential put there comes back and an edit made here \
             would be undone.\n\nBroker it as a server of your own:\n  \
             printf %s \"$TOKEN\" | apex mcp connect {own} --url {url}\n\n\
             then stop the plugin's copy being started. There is no per-server switch, so it \
             is the whole plugin:\n  \
             \"enabledPlugins\": {{\"{plugin}@<marketplace>\": false}}   in ~/.claude/settings.json",
            server.name,
            file.display()
        ),
        Surface::Repository(file) => format!(
            "'{}' is defined by {}, which is part of the repository and shared with everyone \
             who clones it. A credential does not belong in a file under version control, and \
             an edit here would be somebody else's to review.\n\nBroker it under a name of \
             your own instead:\n  \
             printf %s \"$TOKEN\" | apex mcp connect {own} --url {url}",
            server.name,
            file.display()
        ),
        other => format!("'{}' is defined in {}", server.name, other.describe()),
    }
}

/// The credential, from stdin and never from argv.
fn read_credential(plan: &Plan) -> Result<String> {
    let mut value = String::new();
    std::io::stdin().read_to_string(&mut value)?;
    let value = value.trim().to_string();
    if value.is_empty() {
        bail!(
            "nothing on stdin. Pipe the credential in:\n  \
             printf %s \"$TOKEN\" | apex mcp connect {}",
            plan.key
        );
    }
    // Not a refusal: only the far end knows whether a credential works, and a
    // command that refused a value it merely disliked would be a command people
    // work around. The probe below is what actually decides.
    if servers::shape_of(&value) == servers::Shape::Placeholder {
        eprintln!(
            "apex mcp connect: that value reads as a placeholder rather than a credential; \
             the handshake below is what will settle it"
        );
    }
    Ok(value)
}

fn project_root() -> Option<String> {
    let cwd = std::env::current_dir().ok()?;
    apex_agent_core::project::detect(&cwd).map(|p| p.root)
}

fn already_granted(client: &mut Client, project: &str, service: &str) -> bool {
    let Ok(Response::Grants { projects }) = client.call(&Request::Grants) else {
        return false;
    };
    projects
        .get(project)
        .is_some_and(|keys| keys.iter().any(|k| k == &format!("{service}:{OPERATION}")))
}

fn grant(client: &mut Client, project: &str, service: &str, revoke: bool) -> Result<()> {
    client.call(&Request::Grant {
        project: project.to_string(),
        service: service.to_string(),
        capability: OPERATION.to_string(),
        revoke,
    })?;
    Ok(())
}

/// Make the server answer, with the stored credential attached by the daemon.
fn probe(client: &mut Client, service: &str, project: &str) -> Result<()> {
    let mut record = CapabilityRecord::new(service, OPERATION, "");
    record.project = Some(project.to_string());
    let reply = client
        .use_with_body(record, HANDSHAKE)
        .context("asking the server to answer a handshake")?;
    match reply {
        Response::Performed {
            exit_code, output, ..
        } => accepted(exit_code, &output),
        Response::Error { message, .. } => bail!("{message}"),
        other => bail!("unexpected reply: {other:?}"),
    }
}

/// Whether an answer is the server accepting the credential.
///
/// The distinction the whole verb turns on, and neither half of the answer
/// settles it alone.
///
/// The broker runs curl with `fail-with-body`, so an HTTP error status is a
/// **non-zero** exit with the body still attached — measured, in
/// `apex-secretd/tests/end_to_end.rs`: a refused credential comes back as exit
/// 22 carrying `invalid_token`. That is the common case, and the body is where
/// the reason is, so a non-zero exit reported as "could not reach the server"
/// would be wrong about a server that answered.
///
/// A zero exit is not proof either. A server may answer `200` with a JSON-RPC
/// `error` — which means it authenticated the request and then disliked
/// something else — and a proxy in the way may answer `200` with an HTML page.
/// So the test is the reply: an `initialize` that worked carries `result`, and
/// nothing else counts.
fn accepted(exit_code: i32, output: &str) -> Result<()> {
    let messages = super::replies(output);
    let ok = exit_code == 0
        && messages.iter().any(|m| {
            serde_json::from_str::<Value>(m)
                .ok()
                .is_some_and(|v| v.get("result").is_some())
        });
    if ok {
        return Ok(());
    }
    let said = messages
        .first()
        .cloned()
        .unwrap_or_else(|| super::first_line(output));
    bail!("the server did not accept that credential: {said}")
}

/// Replace the definition with one that names the bridge.
///
/// `home` is a parameter rather than read here so that the write can be tested
/// against a fixture without setting a variable the whole process shares.
fn write_bridge(home: &Path, plan: &Plan) -> Result<()> {
    let file = home.join(".claude.json");
    let bridged = serde_json::json!({
        "type": "stdio",
        "command": "apex",
        "args": ["mcp", "bridge", plan.service],
    });
    let key = plan.key.clone();
    let directory = plan.directory.clone();
    crate::migrate::edit_json(&file, move |doc| {
        let block = match &directory {
            Some(dir) => doc
                .get_mut("projects")
                .and_then(|p| p.get_mut(dir))
                .and_then(|p| ensure_object(p, "mcpServers")),
            None => ensure_object(doc, "mcpServers"),
        };
        if let Some(servers) = block {
            servers.insert(key, bridged);
        }
    })
}

/// The named object inside `doc`, created empty when it is not there.
///
/// A `connect` for a server that was never defined has to add one, and a
/// `~/.claude.json` with no `mcpServers` at all is what a fresh account has.
fn ensure_object<'a>(doc: &'a mut Value, key: &str) -> Option<&'a mut serde_json::Map<String, Value>> {
    if !doc.get(key).is_some_and(Value::is_object) {
        doc.as_object_mut()?.insert(key.to_string(), serde_json::json!({}));
    }
    doc.get_mut(key)?.as_object_mut()
}

/// Whether an environment variable is interpolated by any MCP server this
/// machine defines.
///
/// Used by the migration, not here. A `${GITHUB_PERSONAL_ACCESS_TOKEN}` in a
/// plugin's `.mcp.json` is expanded by the agent from `settings.json`'s `env`
/// block, so removing the block's entry is removing the plugin server's
/// credential — and the migration cannot rewrite the plugin's definition to
/// name the bridge instead, because that file is replaced on every update.
pub fn interpolated_by_an_mcp_server(home: &Path, var: &str) -> Option<String> {
    servers::discover(home, None).into_iter().find_map(|s| {
        match &s.credential {
            Credential::FromEnvironment { var: name, .. } if name == var => Some(s.name),
            _ => None,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use servers::{Shape, Transport};

    fn server(name: &str, surface: Surface, transport: Transport, credential: Credential) -> Server {
        Server {
            key: name.rsplit(':').next().unwrap_or(name).to_string(),
            name: name.to_string(),
            surface,
            transport,
            credential,
        }
    }

    fn endpoint(url: &str) -> Transport {
        Transport::Endpoint {
            kind: "http".into(),
            url: url.into(),
        }
    }

    #[test]
    fn a_plugins_definition_is_refused_with_the_two_commands_that_work() {
        // The live case. `plugin:github:github` cannot be brokered in place,
        // and a refusal that only said so would leave somebody with a server
        // that does not authenticate and nothing to do about it.
        let s = server(
            "plugin:github:github",
            Surface::Plugin {
                plugin: "github".into(),
                file: "/h/.claude/plugins/cache/x/.mcp.json".into(),
            },
            endpoint("https://api.githubcopilot.com/mcp/"),
            Credential::FromEnvironment {
                header: "Authorization".into(),
                var: "GITHUB_PERSONAL_ACCESS_TOKEN".into(),
                defined: Some(Shape::Placeholder),
                source: Some("~/.claude/settings.json → env".into()),
            },
        );
        let e = plan("plugin:github:github", None, None, Some(&s), std::slice::from_ref(&s))
            .expect_err("a plugin's file is not ours to edit")
            .to_string();
        assert!(e.contains("replaced whenever the plugin updates"), "{e}");
        assert!(
            e.contains("apex mcp connect github --url https://api.githubcopilot.com/mcp/"),
            "{e}"
        );
        assert!(e.contains("enabledPlugins"), "{e}");
    }

    #[test]
    fn a_repositorys_definition_is_refused_rather_than_edited() {
        let s = server(
            "shared",
            Surface::Repository("/p/.mcp.json".into()),
            endpoint("https://x.example.com/mcp"),
            Credential::None,
        );
        let e = plan("shared", None, None, Some(&s), std::slice::from_ref(&s))
            .expect_err("a repository's file is somebody else's")
            .to_string();
        assert!(e.contains("under version control"), "{e}");
    }

    #[test]
    fn a_local_program_is_refused_and_pointed_at_the_sandbox() {
        // The broker holds an endpoint's credential. A stdio server's is handed
        // to a process on this machine, and there is nothing to stand in front
        // of — which is a different task, and the refusal names it.
        let s = server(
            "memory",
            Surface::User,
            Transport::Stdio {
                command: "npx".into(),
                args: vec!["-y".into(), "server-memory".into()],
            },
            Credential::None,
        );
        let e = plan("memory", None, None, Some(&s), std::slice::from_ref(&s))
            .expect_err("stdio is not an endpoint")
            .to_string();
        assert!(e.contains("apex mcp run memory"), "{e}");
    }

    #[test]
    fn a_url_with_no_path_is_refused_here_rather_than_two_steps_later() {
        // The provider refuses a credential stored without an endpoint path at
        // use time. Catching it here means the message names the URL that was
        // typed instead of a credential that was already stored.
        for bare in ["https://example.com", "https://example.com/"] {
            let e = plan("new", Some(bare), None, None, &[])
                .expect_err(bare)
                .to_string();
            assert!(e.contains("nowhere to carry a message to"), "{bare}: {e}");
        }
        assert!(plan("new", Some("https://example.com/mcp"), None, None, &[]).is_ok());
        assert!(plan("new", Some("ws://example.com/mcp"), None, None, &[]).is_err());
    }

    #[test]
    fn a_server_nobody_defined_says_what_to_type_instead() {
        let e = plan("nope", None, None, None, &[]).expect_err("no such server").to_string();
        assert!(e.contains("apex mcp list"), "{e}");
        assert!(e.contains("--url"), "{e}");
    }

    #[test]
    fn a_credential_name_that_is_already_another_servers_is_refused() {
        // Storing under an existing name replaces that credential, and the
        // other server would stop working with no message that said why.
        let mine = server(
            "memory",
            Surface::User,
            endpoint("https://a.example.com/mcp"),
            Credential::None,
        );
        let theirs = server(
            "other",
            Surface::User,
            endpoint("https://b.example.com/mcp"),
            Credential::Brokered {
                service: "memory".into(),
            },
        );
        let e = plan("memory", None, None, Some(&mine), &[mine.clone(), theirs])
            .expect_err("that name is taken")
            .to_string();
        assert!(e.contains("--service"), "{e}");
        // And with a name of its own it is fine.
        assert!(plan("memory", None, Some("memory-2"), Some(&mine), std::slice::from_ref(&mine)).is_ok());
    }

    #[test]
    fn a_plugin_server_gets_a_credential_name_a_person_would_recognise() {
        // `plugin:github:github` sanitised whole is `plugin-github-github`,
        // which is what `apex secret list` would then show forever.
        let p = plan("plugin:github:github", Some("https://api.example.com/mcp"), None, None, &[])
            .expect("a url was given");
        assert_eq!(p.service, "github");
        assert_eq!(p.host, "api.example.com");
        assert_eq!(p.path, "/mcp");
    }

    #[test]
    fn the_bridge_definition_replaces_the_one_that_held_the_token() {
        // P1-018's second criterion at the one place that decides it: after
        // this write, the file the agent reads holds no credential.
        let home = std::env::temp_dir().join(format!(
            "apex-connect-write-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
        ));
        std::fs::create_dir_all(&home).expect("dir");
        let file = home.join(".claude.json");
        std::fs::write(
            &file,
            serde_json::json!({
                "installMethod": "keep me",
                "mcpServers": {"memory": {"type": "http", "url": "https://m.example.com/mcp",
                                          "headers": {"Authorization": "Bearer sekrit-9f3a"}}}
            })
            .to_string(),
        )
        .expect("write");

        let found = servers::discover(&home, None);
        let existing = found.iter().find(|s| s.name == "memory");
        let plan = plan("memory", None, None, existing, &found).expect("a plan");
        assert!(plan.replaced_a_credential);
        write_bridge(&home, &plan).expect("write");

        let text = std::fs::read_to_string(&file).expect("read");
        assert!(!text.contains("sekrit-9f3a"), "the credential survived: {text}");
        assert!(text.contains("keep me"), "an unrelated key was dropped: {text}");
        let doc: Value = serde_json::from_str(&text).expect("json");
        assert_eq!(doc["mcpServers"]["memory"]["command"], "apex");
        assert_eq!(doc["mcpServers"]["memory"]["args"][2], "memory");
        // And the listing now answers the criterion the other way round.
        let after = servers::discover(&home, None);
        assert!(!after[0].credential.agent_readable());
        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn an_oauth_server_is_routed_through_the_broker_like_any_other() {
        // §13.12's named case, at the one place that decides it. A remote
        // server that authenticates by OAuth carries NO credential in the
        // definition — that is the whole point of OAuth — so before P1-017 it
        // looked identical to a server with nothing to protect, and `plan`
        // reported that connecting it replaced nothing. It replaces the thing
        // that matters: after this, the agent no longer has an account with
        // the server to authenticate as.
        //
        // Cloudflare's own remote servers are the instance §13.12 names, and
        // they accept a Cloudflare API token as `Authorization: Bearer` — so
        // the bridge is a route they can actually take. Nothing in this test
        // or in the code it drives knows their hostnames; the URL below is a
        // fixture, and the rule is about remote servers.
        let home = std::env::temp_dir().join(format!(
            "apex-connect-oauth-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
        ));
        std::fs::create_dir_all(&home).expect("dir");
        let file = home.join(".claude.json");
        std::fs::write(
            &file,
            serde_json::json!({
                "mcpServers": {"cf-bindings": {
                    "type": "http",
                    "url": "https://bindings.mcp.cloudflare.example/mcp"
                }}
            })
            .to_string(),
        )
        .expect("write");

        let found = servers::discover(&home, None);
        let existing = found.iter().find(|s| s.name == "cf-bindings");
        assert!(
            existing.expect("the server").credential.agent_readable(),
            "an OAuth server was reported as having nothing the agent can read"
        );
        let plan = plan("cf-bindings", None, None, existing, &found).expect("a plan");
        assert!(
            plan.replaced_a_credential,
            "connecting an OAuth server reported that it replaced nothing"
        );
        // The endpoint is taken from the definition, so nobody has to retype a
        // URL that is already on the machine.
        assert_eq!(plan.host, "bindings.mcp.cloudflare.example");
        assert_eq!(plan.path, "/mcp");
        assert_eq!(plan.scheme, "https");
        write_bridge(&home, &plan).expect("write");

        let after = servers::discover(&home, None);
        assert!(
            !after[0].credential.agent_readable(),
            "the rewritten definition is still one the agent can authenticate"
        );
        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn a_directory_scoped_server_is_rewritten_in_its_own_block() {
        // The failure this closes: writing the bridge into the top-level
        // `mcpServers` for a server defined per directory leaves the original
        // definition, credential and all, and adds a second server beside it.
        let home = std::env::temp_dir().join(format!(
            "apex-connect-dir-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
        ));
        let cwd = home.join("work");
        std::fs::create_dir_all(&cwd).expect("dir");
        let file = home.join(".claude.json");
        std::fs::write(
            &file,
            serde_json::json!({
                "projects": {cwd.to_string_lossy(): {"mcpServers": {
                    "here": {"type": "http", "url": "https://m.example.com/mcp",
                             "headers": {"Authorization": "Bearer local-only-token"}}
                }}}
            })
            .to_string(),
        )
        .expect("write");

        let found = servers::discover(&home, Some(&cwd));
        let existing = found.iter().find(|s| s.name == "here");
        let plan = plan("here", None, None, existing, &found).expect("a plan");
        write_bridge(&home, &plan).expect("write");

        let doc: Value = serde_json::from_str(&std::fs::read_to_string(&file).expect("read"))
            .expect("json");
        let block = &doc["projects"][cwd.to_string_lossy().as_ref()]["mcpServers"]["here"];
        assert_eq!(block["command"], "apex");
        assert!(doc.get("mcpServers").is_none(), "a second server was added: {doc}");
        assert!(
            !std::fs::read_to_string(&file).unwrap().contains("local-only-token"),
            "the credential survived"
        );
        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn a_reply_that_is_not_the_server_accepting_is_not_taken_for_one() {
        // Both halves of the answer are needed, and neither settles it alone.
        accepted(0, r#"{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-06-18"}}"#)
            .expect("a result is the server accepting");
        accepted(0, "event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}\n\n")
            .expect("the same answer in event-stream framing");

        // What a refused credential actually produces: curl runs with
        // `fail-with-body`, so the status is a non-zero exit and the reason is
        // in the body. Exit 22 and this body are what the end-to-end test
        // measured against a real daemon and a loopback server.
        let e = accepted(
            22,
            r#"{"error":"invalid_token","error_description":"the access token is invalid"}"#,
        )
        .expect_err("a 401 is not an acceptance")
        .to_string();
        assert!(e.contains("invalid_token"), "the reason has to survive: {e}");

        for (code, refused) in [
            // A JSON-RPC error over a 200: the server authenticated the request
            // and then disliked something else, which proves nothing about the
            // credential.
            (0, r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32600,"message":"bad version"}}"#),
            // A proxy in the way, answering 200 with a page.
            (0, "<html><head><title>Authorization Required</title></head></html>"),
            // Nothing at all.
            (0, ""),
            // And a result that arrived with a failing status is still not one.
            (22, r#"{"jsonrpc":"2.0","id":1,"result":{}}"#),
        ] {
            let e = accepted(code, refused).expect_err(refused).to_string();
            assert!(e.contains("did not accept"), "{code} {refused} -> {e}");
        }
    }

    #[test]
    fn a_config_with_no_mcp_block_gets_one_rather_than_being_left_alone() {
        // A fresh account has no `mcpServers` key at all, and an edit that
        // silently did nothing would report success and change nothing.
        let mut doc = serde_json::json!({"installMethod": "x"});
        ensure_object(&mut doc, "mcpServers")
            .expect("created")
            .insert("m".into(), serde_json::json!({"type": "stdio"}));
        assert_eq!(doc["mcpServers"]["m"]["type"], "stdio");
        assert_eq!(doc["installMethod"], "x");
        // And a key that is there but is not an object is replaced rather than
        // making every later insert silently vanish.
        let mut wrong = serde_json::json!({"mcpServers": 7});
        assert!(ensure_object(&mut wrong, "mcpServers").is_some());
    }
}
