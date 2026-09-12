//! `apex mcp confine` — put an MCP server's own command behind its sandbox.
//!
//! The typed half of [`super::sidecar`], and the same shape as
//! [`super::connect`]: one verb a person runs once, which rewrites the
//! definition the agent reads.
//!
//! ```text
//! "memory": {"command": "npx", "args": ["-y", "@modelcontextprotocol/server-memory"]}
//! ```
//!
//! becomes
//!
//! ```text
//! "memory": {"command": "apex",
//!            "args": ["mcp", "run", "memory", "--",
//!                     "npx", "-y", "@modelcontextprotocol/server-memory"]}
//! ```
//!
//! The original command stays in the definition rather than moving into a
//! policy file, so what a server runs is still visible where somebody would
//! look for it, and the wrapper is not a layer of indirection to chase.

use anyhow::{bail, Result};
use serde_json::Value;

use super::servers::{self, Surface, Transport};
use super::sidecar;

pub fn main(name: &str, dry_run: bool) -> Result<i32> {
    crate::migrate::refuse_from_inside_a_session()?;
    if !dry_run {
        crate::migrate::refuse_while_an_agent_is_running()?;
    }

    let home = super::home();
    let cwd = std::env::current_dir().ok();
    let found = servers::discover(&home, cwd.as_deref());
    let Some(server) = found.iter().find(|s| s.name == name) else {
        bail!(
            "no MCP server called '{name}' is defined here.\n  \
             apex mcp list   what this machine has"
        );
    };

    let (command, args) = match &server.transport {
        Transport::Stdio { command, args } => (command.clone(), args.clone()),
        Transport::Endpoint { url, .. } => bail!(
            "'{name}' is an endpoint ({url}), not a program this machine runs. There is no \
             process here to confine — the request is made by apex-secretd, which holds the \
             credential:\n  apex mcp connect {name}"
        ),
        Transport::Other(what) => bail!("'{name}' is defined as {what}, with no command"),
    };
    if let Some(already) = servers::confined_server(&command, &args) {
        println!("'{name}' already starts confined, as '{already}'");
        for line in sidecar::load(&already)?.describe() {
            println!("  {line}");
        }
        return Ok(0);
    }
    if !server.surface.is_writable_here() {
        bail!("{}", refuse(server, &command, &args));
    }

    let policy = sidecar::load(name)?;
    println!("'{name}' will start with:");
    for line in policy.describe() {
        println!("  {line}");
    }
    println!("  decided by  {}", policy.source.describe());

    // The definition's own `env` block is applied by the agent to the process
    // it spawns, which is now `apex` rather than the server — and `--clearenv`
    // stops it there. Named rather than carried through silently: one of those
    // variables may be the credential the server needs, and a server that lost
    // it would look broken rather than confined.
    if let Some(missing) = env_not_carried(&home, name, &policy) {
        println!(
            "\nIts definition sets {missing}, and a confined process is started with a cleared \
             environment. Add the names to the policy to carry them through:\n  \
             {}/{name}.toml   env = [{missing}]",
            sidecar::user_dir().display()
        );
    }

    if dry_run {
        println!("\nnothing was written. Run without --dry-run to confine.");
        return Ok(0);
    }

    let directory = match &server.surface {
        Surface::Directory(dir) => Some(dir.clone()),
        _ => None,
    };
    write(&home, &server.key, directory.as_deref(), name, &command, &args)?;
    println!("\nwrote   ~/.claude.json — '{name}' now starts through `apex mcp run`");
    Ok(0)
}

/// The names in a definition's `env` block that the policy does not carry.
fn env_not_carried(home: &std::path::Path, name: &str, policy: &sidecar::McpPolicy) -> Option<String> {
    let doc = servers::read_json(&home.join(".claude.json"))?;
    let env = doc
        .get("mcpServers")?
        .get(name)?
        .get("env")?
        .as_object()?;
    let missing: Vec<String> = env
        .keys()
        .filter(|k| !policy.env.contains(k))
        .map(|k| format!("\"{k}\""))
        .collect();
    (!missing.is_empty()).then(|| missing.join(", "))
}

/// The refusal for a definition that is not the user's to edit.
///
/// Unlike a brokered endpoint, there is something useful to say here: the
/// wrapper is a command, so it can be added as a server of the user's own with
/// the plugin's copy switched off.
fn refuse(server: &servers::Server, command: &str, args: &[String]) -> String {
    let own = server.name.rsplit(':').next().unwrap_or(&server.name);
    let mut wrapped = vec!["mcp".to_string(), "run".into(), own.into(), "--".into()];
    wrapped.push(command.to_string());
    wrapped.extend(args.iter().cloned());
    format!(
        "'{}' is defined in {}, which this may not rewrite — an edit there is undone \
         without warning. Add a confined copy of your own instead:\n  \
         claude mcp add-json {own} '{}'\n\nthen stop the original being started.",
        server.name,
        server.surface.describe(),
        serde_json::json!({"type": "stdio", "command": "apex", "args": wrapped})
    )
}

/// Replace the definition with one that starts the same command confined.
fn write(
    home: &std::path::Path,
    key: &str,
    directory: Option<&str>,
    name: &str,
    command: &str,
    args: &[String],
) -> Result<()> {
    let mut wrapped: Vec<Value> = ["mcp", "run", name, "--", command]
        .iter()
        .map(|s| Value::String((*s).to_string()))
        .collect();
    wrapped.extend(args.iter().map(|a| Value::String(a.clone())));

    let key = key.to_string();
    let directory = directory.map(str::to_string);
    crate::migrate::edit_json(&home.join(".claude.json"), move |doc| {
        let block = match &directory {
            Some(dir) => doc
                .get_mut("projects")
                .and_then(|p| p.get_mut(dir))
                .and_then(|p| p.get_mut("mcpServers"))
                .and_then(Value::as_object_mut),
            None => doc.get_mut("mcpServers").and_then(Value::as_object_mut),
        };
        let Some(servers) = block else { return };
        let Some(entry) = servers.get_mut(&key).and_then(Value::as_object_mut) else {
            return;
        };
        // Edited in place rather than replaced, so an `env` block or a
        // `timeout` the person set is still there afterwards.
        entry.insert("command".into(), Value::String("apex".into()));
        entry.insert("args".into(), Value::Array(wrapped));
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fixture(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "apex-mcp-confine-{}-{tag}-{:?}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
        ));
        std::fs::create_dir_all(&dir).expect("fixture");
        dir
    }

    #[test]
    fn the_wrapper_keeps_the_command_where_a_person_would_look_for_it() {
        // The definition still says what runs. A wrapper that moved the command
        // into a policy file would make `apex mcp list` show every local server
        // as "apex", which is worse than no wrapper for anyone reading it.
        let home = fixture("wrap");
        std::fs::write(
            home.join(".claude.json"),
            serde_json::json!({"mcpServers": {"memory": {
                "command": "npx", "args": ["-y", "@modelcontextprotocol/server-memory"],
                "env": {"MEMORY_FILE_PATH": "/tmp/m.json"}
            }}})
            .to_string(),
        )
        .expect("write");

        write(
            &home,
            "memory",
            None,
            "memory",
            "npx",
            &["-y".to_string(), "@modelcontextprotocol/server-memory".to_string()],
        )
        .expect("confine");

        let doc: Value =
            serde_json::from_str(&std::fs::read_to_string(home.join(".claude.json")).unwrap())
                .expect("json");
        let entry = &doc["mcpServers"]["memory"];
        assert_eq!(entry["command"], "apex");
        assert_eq!(
            entry["args"],
            serde_json::json!(["mcp", "run", "memory", "--", "npx", "-y",
                              "@modelcontextprotocol/server-memory"])
        );
        // Everything else the person had set survives.
        assert_eq!(entry["env"]["MEMORY_FILE_PATH"], "/tmp/m.json");

        // And the round trip: the discovery layer reads the original command
        // back out, so confining twice is a no-op rather than a nesting.
        let found = servers::discover(&home, None);
        let Transport::Stdio { command, args } = &found[0].transport else {
            panic!("not stdio");
        };
        let (name, inner) = servers::confined(command, args).expect("recognised");
        assert_eq!(name, "memory");
        assert_eq!(inner, vec!["npx", "-y", "@modelcontextprotocol/server-memory"]);
        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn an_endpoint_is_refused_and_pointed_at_the_broker() {
        // Confining an endpoint would confine nothing: the request is made by
        // apex-secretd, in a process this does not start.
        let home = fixture("endpoint");
        std::fs::write(
            home.join(".claude.json"),
            r#"{"mcpServers":{"remote":{"type":"http","url":"https://m.example.com/mcp"}}}"#,
        )
        .expect("write");
        let found = servers::discover(&home, None);
        assert!(matches!(found[0].transport, Transport::Endpoint { .. }));
        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn a_definition_that_is_not_ours_gets_a_command_that_adds_one_that_is() {
        let server = servers::Server {
            name: "plugin:x:srv".into(),
            key: "srv".into(),
            surface: Surface::Plugin {
                plugin: "x".into(),
                file: "/h/.mcp.json".into(),
            },
            transport: Transport::Stdio {
                command: "node".into(),
                args: vec!["server.js".into()],
            },
            credential: servers::Credential::None,
        };
        let message = refuse(&server, "node", &["server.js".to_string()]);
        assert!(message.contains("claude mcp add-json srv"), "{message}");
        assert!(message.contains(r#""mcp","run","srv","--","node","server.js""#), "{message}");
    }
}
