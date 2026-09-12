//! `apex mcp` — an MCP server the agent talks to instead of the credential.
//!
//! §10 asks for MCP authentication to be brokered. §12 asks for it without
//! rewriting anything: the agent must still see an MCP server, configured the
//! way it configures every other one.
//!
//! What was there before this: `~/.claude.json` holds
//!
//! ```json
//! "claude-memory": {"type": "http", "url": "https://…/mcp",
//!                   "headers": {"Authorization": "Bearer …"}}
//! ```
//!
//! and that file is bound *writable* into a managed session, because Claude
//! records onboarding state in it on every run. So the bearer token is readable
//! by the agent, which is P0-003's fourth criterion failing in one line of
//! JSON.
//!
//! What replaces it:
//!
//! ```json
//! "claude-memory": {"type": "stdio", "command": "apex",
//!                   "args": ["mcp", "bridge", "claude-memory"]}
//! ```
//!
//! The agent speaks stdio MCP to this process. This process speaks the agent
//! runtime's protocol to `apex-agentd`, which stamps the session, checks the
//! session's secret policy and forwards a capability record to `apex-secretd`,
//! which attaches the credential and makes the HTTPS request. The token is in
//! none of those first three places.
//!
//! ## Why a bridge rather than a helper that fetches the header
//!
//! Because whatever this process could learn, the agent could learn: it runs
//! inside the sandbox, as the agent, and the agent can run it too. Every design
//! where the value arrives *here* is a design where the value is in the
//! session. So the value never arrives; the request leaves instead. That is the
//! same reason `apex secret` has no verb that returns a credential, applied to
//! a second protocol.
//!
//! ## What this does not do
//!
//! * **A server-initiated message** — an MCP server pushing a notification down
//!   an SSE stream it holds open — does not arrive. Each message is one request
//!   and one reply. `claude-memory` is request/reply, and a server that needs
//!   more will look like one that never notifies.
//! * **One conversation per credential.** `apex-secretd` remembers the
//!   `Mcp-Session-Id` per account and service, so two sessions talking to the
//!   same server share the server's idea of the conversation.
//! * **Nothing is cached.** Every message is a round trip through two daemons
//!   and one HTTPS request, which is what the previous arrangement did too,
//!   minus the two Unix sockets.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;

use anyhow::{bail, Result};
use apex_agent_core::mcpconf;
use apex_agent_core::protocol::{
    Request as AgentRequest, Response as AgentResponse, MCP_BRIDGE_VERSION,
};
use clap::Subcommand;

pub mod confine;
pub mod connect;
pub mod servers;
pub mod sidecar;

/// `apex mcp <verb>`.
#[derive(Subcommand)]
pub enum McpCmd {
    /// Serve one brokered MCP server on stdin and stdout.
    ///
    /// Meant to be spawned by an agent, not typed. `apex secret migrate` is
    /// what puts it in an agent's configuration.
    Bridge {
        /// The stored credential, by the name `apex secret list` shows.
        service: String,
    },

    /// Every MCP server this machine has, and where each one's credential is.
    ///
    /// Reads all four places a server can be defined — your own
    /// `~/.claude.json`, its per-directory block, a repository's `.mcp.json`
    /// and every enabled plugin's — and says for each whether the agent can
    /// read the credential.
    List {
        #[arg(long)]
        json: bool,
    },

    /// Move one MCP server's credential into the broker, once, by hand.
    ///
    /// The credential is read from stdin, stored, proved against the server
    /// itself, and only then removed from the file the agent reads. What is
    /// left behind names `apex mcp bridge`, so the agent still sees an MCP
    /// server and no longer sees a token.
    Connect {
        /// The server, as `apex mcp list` names it.
        name: String,
        /// The endpoint, when the server is not already defined here.
        #[arg(long)]
        url: Option<String>,
        /// Store it under a different credential name.
        #[arg(long)]
        service: Option<String>,
        /// Say what would happen and write nothing.
        #[arg(long)]
        dry_run: bool,
    },

    /// Run one MCP server inside its own sandbox.
    ///
    /// Meant to be spawned by an agent, not typed: `apex mcp confine` is what
    /// puts it in the server's definition. Without a policy file the server
    /// gets no network, no project, no access to the broker and a private home
    /// of its own — `apex mcp policy <name>` prints what it will actually get.
    Run {
        /// The server, as `apex mcp list` names it.
        name: String,
        /// The server's own command, after `--`.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<String>,
    },

    /// Rewrite a local MCP server's definition so it starts confined.
    Confine {
        /// The server, as `apex mcp list` names it.
        name: String,
        /// Say what would happen and write nothing.
        #[arg(long)]
        dry_run: bool,
    },

    /// What each MCP server may reach, and where that was decided.
    Policy {
        /// One server. Every one this machine defines, when left out.
        name: Option<String>,
    },

    /// The two trust planes: programs on this machine, and endpoints off it.
    ///
    /// The same set `list` shows, grouped by which side of this machine each
    /// connector is on, because the two get different mitigations and neither
    /// is the other's: a local program is confined by bubblewrap and a cloud
    /// endpoint is not confined at all — its credential is the boundary.
    ///
    /// Also says what each named policy does to the cloud plane, and — because
    /// the table would otherwise read as something it is not — that the
    /// reduction is all-or-nothing rather than a per-connector switch.
    Planes {
        #[arg(long)]
        json: bool,
    },

    /// Memory providers: which server holds your memory, and whether it is up.
    ///
    /// APEX stores no memory of its own. This is a view over MCP servers
    /// something else already defined, so the provider named authoritative
    /// stays the source of truth and there is no migration to do.
    ///
    /// Which server is a memory provider is declared, in
    /// `~/.config/apex/memory.toml`:
    ///
    /// ```text
    /// authoritative = "claude-memory"
    /// providers = ["claude-memory"]
    /// ```
    ///
    /// With no declaration, a server whose name looks like a memory server's
    /// is reported and labelled as a guess. Where two candidates exist and
    /// nothing says which is authoritative, this reports the ambiguity rather
    /// than choosing where your notes live.
    Memory {
        #[arg(long)]
        json: bool,
        /// Ask each provider to answer an MCP `initialize` handshake.
        ///
        /// Off by default. `initialize` changes nothing, but it is still a
        /// request to somebody's server and a status command should not make
        /// one unasked. It needs the credential in the broker and
        /// `mcp.request` already granted here — this never grants one in order
        /// to report on it.
        #[arg(long)]
        probe: bool,
    },
}

pub fn main(cmd: McpCmd) -> i32 {
    match cmd {
        McpCmd::Bridge { service } => match bridge(&service) {
            Ok(code) => code,
            Err(e) => {
                // stderr, because stdout is the MCP transport and a line of
                // English on it is a protocol error at the far end.
                eprintln!("apex mcp bridge: {e:#}");
                1
            }
        },
        McpCmd::List { json } => report(list(json)),
        McpCmd::Connect {
            name,
            url,
            service,
            dry_run,
        } => report(connect::main(&name, url.as_deref(), service.as_deref(), dry_run)),
        McpCmd::Run { name, command } => match sidecar::run(&name, &command) {
            Ok(code) => code,
            Err(e) => {
                // stderr for the same reason the bridge uses it: stdout is the
                // MCP transport, and English on it is a protocol error.
                eprintln!("apex mcp run: {e:#}");
                1
            }
        },
        McpCmd::Confine { name, dry_run } => report(confine::main(&name, dry_run)),
        McpCmd::Policy { name } => report(policy(name.as_deref())),
        McpCmd::Planes { json } => report(crate::connector::planes_main(json)),
        McpCmd::Memory { json, probe } => report(crate::connector::memory_main(json, probe)),
    }
}

fn report(result: Result<i32>) -> i32 {
    match result {
        Ok(code) => code,
        Err(e) => {
            eprintln!("apex mcp: {e:#}");
            1
        }
    }
}

/// Where this account's Claude profile is.
pub fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

/// `apex mcp list`.
fn list(json: bool) -> Result<i32> {
    let cwd = std::env::current_dir().ok();
    let found = servers::discover(&home(), cwd.as_deref());
    // What a session APEX starts would actually be handed, so the `sandbox`
    // line below is the launcher's verdict rather than a second reading of the
    // same definition. See `connector::launch_verdicts`.
    let launch = crate::connector::launch_verdicts(&home(), cwd.as_deref());
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&servers::as_json(&found, &launch))?
        );
        return Ok(0);
    }
    if found.is_empty() {
        println!("no MCP server is defined for this account");
        return Ok(0);
    }

    let mut readable = 0;
    for server in &found {
        println!("{}", server.name);
        // P1-028's first criterion, in the listing a person already reads
        // rather than only in a new verb. "stdio" and "http" are facts about
        // the definition; which side of this machine the connector is on is
        // what decides what can confine it, and a reader should not have to
        // know that the first implies the second.
        println!(
            "  plane       {}",
            crate::connector::Plane::of(&server.transport).describe()
        );
        println!("  transport   {}", describe_transport(&server.transport));
        println!("  credential  {}", describe_credential(&server.credential));
        if let servers::Transport::Stdio { command, args } = &server.transport {
            // Only for a program, because only a program is a thing to confine:
            // an endpoint's request is made by apex-secretd, in a process this
            // machine's agent never starts.
            println!(
                "  sandbox     {}",
                describe_sandbox(
                    &server.name,
                    command,
                    args,
                    launch
                        .decisions
                        .iter()
                        .find(|d| d.name == server.name)
                        .and_then(|d| d.confined),
                )
            );
        }
        println!("  defined in  {}", server.surface.describe());
        if server.credential.agent_readable() {
            readable += 1;
            if server.surface.is_writable_here() {
                println!("  fix         apex mcp connect {}", server.name);
            }
        }
        println!();
    }

    // The count, not a lecture: somebody who has already decided is not helped
    // by being told again, and somebody who has not needs the number.
    match readable {
        0 => println!("no MCP credential on this machine is readable by an agent"),
        1 => println!("1 MCP credential is readable by any agent that runs as you"),
        n => println!("{n} MCP credentials are readable by any agent that runs as you"),
    }
    Ok(0)
}

/// `apex mcp policy`.
fn policy(only: Option<&str>) -> Result<i32> {
    let cwd = std::env::current_dir().ok();
    let found = servers::discover(&home(), cwd.as_deref());
    let wanted: Vec<&servers::Server> = match only {
        Some(name) => found.iter().filter(|s| s.name == name).collect(),
        // Only the local ones: an endpoint is a request the broker makes, and
        // there is no process of its own to confine.
        None => found
            .iter()
            .filter(|s| matches!(s.transport, servers::Transport::Stdio { .. }))
            .collect(),
    };
    if wanted.is_empty() {
        match only {
            Some(name) => bail!("no MCP server called '{name}' is defined here"),
            None => println!("no MCP server on this machine is a program this could confine"),
        }
        return Ok(0);
    }

    for server in wanted {
        println!("{}", server.name);
        if let servers::Transport::Endpoint { .. } = server.transport {
            println!("  this is an endpoint the broker reaches, not a program to confine");
            println!();
            continue;
        }
        let policy = sidecar::load(&server.name)?;
        for line in policy.describe() {
            println!("  {line}");
        }
        println!("  decided by  {}", policy.source.describe());
        println!(
            "  started     {}",
            match &server.transport {
                servers::Transport::Stdio { command, args }
                    if servers::confined_server(command, args).as_deref()
                        == Some(server.name.as_str()) =>
                    "inside that sandbox".to_string(),
                _ => format!(
                    "with everything the agent session has — apex mcp confine {}",
                    server.name
                ),
            }
        );
        println!();
    }
    println!("policies are read from {} then {}", sidecar::user_dir().display(), sidecar::MACHINE_DIR);
    Ok(0)
}

/// Whether a local server starts confined, and what it gets if it does.
///
/// A line in the listing rather than only in `apex mcp policy`, because the
/// listing is where somebody looks to find out what their machine runs — and a
/// server that starts with everything the session has should say so there.
fn describe_sandbox(
    name: &str,
    command: &str,
    args: &[String],
    wrap: Option<mcpconf::Wrap>,
) -> String {
    let policy = |n: &str| match sidecar::load(n) {
        Ok(policy) => summarise(&policy),
        // Not "none", and not silence: an unreadable policy is not an absent
        // one, and a listing that fell back to describing the default would
        // describe a confinement the server is not going to get.
        Err(e) => format!("but the policy could not be read: {e:#}"),
    };
    if let Some(confined) = servers::confined_server(command, args) {
        return format!("its own, {}", policy(&confined));
    }
    // Nothing on disk wraps it — which used to settle the question, and stopped
    // being the whole of it when the session launcher started wrapping
    // third-party definitions at launch. "none" here is false of every session
    // `apex agent` starts, and "its own" is false of every session anybody else
    // starts, so neither on its own may be printed.
    match wrap {
        Some(mcpconf::Wrap::AtLaunch) => format!(
            "not in this definition — `apex agent` wraps it at launch, {}. \
             Start the agent yourself and it gets none; `apex mcp confine {name}` \
             writes the wrapper to disk so it holds either way",
            policy(name)
        ),
        _ => format!("none — everything the session has (apex mcp confine {name})"),
    }
}

/// One line: every dimension a policy opened, and where it was opened.
///
/// The three lines `apex mcp policy` prints are too much for a listing, and a
/// bare "its own" would read identically for a server that is closed and one
/// whose file opened every dimension — which is the single mistake a listing
/// about confinement must not make.
fn summarise(policy: &sidecar::McpPolicy) -> String {
    let mut widened = Vec::new();
    if policy.network {
        widened.push("the network");
    }
    if policy.project {
        widened.push("the project");
    }
    if policy.broker {
        widened.push("the broker");
    }
    if !policy.read.is_empty() {
        widened.push("paths it reads");
    }
    if !policy.write.is_empty() {
        widened.push("paths it writes");
    }
    if !policy.env.is_empty() {
        widened.push("variables from the session");
    }
    if widened.is_empty() {
        return "closed in every dimension".to_string();
    }
    format!(
        "opened to {} by {}",
        widened.join(", "),
        policy.source.describe()
    )
}

fn describe_transport(t: &servers::Transport) -> String {
    match t {
        servers::Transport::Stdio { command, args } => {
            // The server's own command, not the wrapper's. `apex mcp run memory
            // --` says nothing about what runs, and the listing is where
            // somebody looks to find out.
            let (command, args) = match servers::confined(command, args) {
                Some((_, inner)) => {
                    let (program, rest) = inner.split_first().expect("confined() refuses an empty command");
                    (program.clone(), rest.to_vec())
                }
                None => (command.clone(), args.clone()),
            };
            let mut line = command;
            for a in args.iter().take(4) {
                line.push(' ');
                line.push_str(a);
            }
            format!("stdio, {line}")
        }
        servers::Transport::Endpoint { kind, url } => format!("{kind}, {url}"),
        servers::Transport::Other(what) => format!("{what} — not one this understands"),
    }
}

fn describe_credential(c: &servers::Credential) -> String {
    match c {
        servers::Credential::None => "none in the definition".to_string(),
        servers::Credential::Brokered { service } => {
            format!("held by apex-secretd as '{service}' — the agent cannot read it")
        }
        servers::Credential::InConfig { header, shape } => format!(
            "{shape} in the definition's {header} header, which the agent reads",
            shape = shape.describe(),
        ),
        servers::Credential::FromEnvironment {
            header,
            var,
            defined,
            source,
        } => match (defined, source) {
            (Some(shape), Some(source)) => format!(
                "{header}: ${var}, which is {} in {source}",
                shape.describe()
            ),
            _ => format!("{header}: ${var}, and nothing on this machine defines it"),
        },
    }
}

/// The longest JSON-RPC message this will carry in either direction.
///
/// The same bound `apex-secretd` applies to a message body, restated here so a
/// message too large to send is refused with an explanation rather than by a
/// daemon closing the connection.
const MAX_MESSAGE_BYTES: usize = 1024 * 1024;

/// A guard ahead of the protocol can never fire, which is the fail-open the
/// three named revisions exist to catch. Checked when the crate compiles rather
/// than when a test runs: it is a fact about two constants.
const _: () = assert!(MCP_BRIDGE_VERSION <= apex_agent_core::protocol::PROTOCOL_VERSION);

fn bridge(service: &str) -> Result<i32> {
    let mut agent = apex_agent_core::client::Client::connect()?;
    require_a_runtime_that_carries_a_body(&mut agent)?;

    // Sent for an unsessioned caller, ignored for a managed session, whose
    // project the daemon already knows. Same rule as `apex secret use`.
    let project = current_project_root();

    let stdin = std::io::stdin();
    let mut reader = BufReader::new(stdin.lock());
    let stdout = std::io::stdout();
    let mut out = stdout.lock();

    let mut line = String::new();
    loop {
        line.clear();
        let n = (&mut reader)
            .take(MAX_MESSAGE_BYTES as u64)
            .read_line(&mut line)?;
        if n == 0 {
            return Ok(0);
        }
        // A line that filled the bound without ending is a message this cannot
        // carry, and reading on would parse its tail as the next message.
        if n == MAX_MESSAGE_BYTES && !line.ends_with('\n') {
            bail!("that message is longer than the {MAX_MESSAGE_BYTES} bytes this bridge carries");
        }
        let message = line.trim();
        if message.is_empty() {
            continue;
        }

        // Parsed here for one thing: the `id`, which says whether this message
        // expects a reply and what the reply has to be addressed to. A JSON-RPC
        // notification has no `id` and the far end answers it with `202
        // Accepted` and no body; answering one would be a reply to a request
        // that was never made.
        let id = serde_json::from_str::<serde_json::Value>(message)
            .ok()
            .and_then(|v| v.get("id").cloned())
            .filter(|id| !id.is_null());

        let answer = match agent.request(&AgentRequest::SecretUse {
            service: service.to_string(),
            // §13.2's id for it. The operation declares no resource and no
            // parameters, which is the whole security argument: there is no
            // field here a session could put a URL in, so the endpoint can
            // only be the one pinned when the credential was stored.
            operation: "mcp.request".to_string(),
            resource: String::new(),
            params: Default::default(),
            body: Some(message.to_string()),
            project: project.clone(),
        })? {
            AgentResponse::Brokered {
                exit_code, output, ..
            } => {
                let replies = replies(&output);
                if exit_code == 0 && !replies.is_empty() {
                    replies
                } else {
                    // The far end answered something that is not a message —
                    // a 401, an HTML error page, an empty body. The agent is
                    // waiting on the `id` it sent, so it gets an error with
                    // that `id` rather than silence.
                    eprintln!("apex mcp bridge: {}", first_line(&output));
                    vec![rpc_error(id.as_ref(), &first_line(&output))]
                }
            }
            AgentResponse::Error { message, .. } => {
                // Not fatal, and not silent. A capability that is not granted
                // for this project refuses one message; answering nothing would
                // hang the agent's handshake and be reported as a broken
                // server rather than as an unauthorised one.
                eprintln!("apex mcp bridge: {message}");
                vec![rpc_error(id.as_ref(), &message)]
            }
            other => bail!("unexpected reply: {other:?}"),
        };

        if id.is_none() {
            continue;
        }
        for reply in answer {
            writeln!(out, "{reply}")?;
        }
        out.flush()?;
    }
}

/// A JSON-RPC error addressed to the message that caused it.
///
/// `-32603` is "internal error", the code for a failure that is not the
/// caller's request being malformed — which it is not: the request was fine and
/// the broker or the server would not carry it. Built with `serde_json` rather
/// than `format!` so a message containing a quote cannot produce a document the
/// agent's client refuses to parse, on top of an error it already had.
fn rpc_error(id: Option<&serde_json::Value>, message: &str) -> String {
    let doc = serde_json::json!({
        "jsonrpc": "2.0",
        "id": id.cloned().unwrap_or(serde_json::Value::Null),
        "error": {"code": -32603, "message": message},
    });
    doc.to_string()
}

/// The JSON-RPC messages in an HTTP reply body.
///
/// A streamable-HTTP MCP server answers either with one JSON document or with
/// `text/event-stream` framing, and which one is the server's choice per
/// request. Both shapes are unwrapped to the same thing: one JSON message per
/// line, which is what stdio MCP is.
///
/// Anything that parses as JSON is passed through unchanged rather than
/// re-serialised, so a reply is byte-for-byte what the server sent.
pub fn replies(body: &str) -> Vec<String> {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    if serde_json::from_str::<serde_json::Value>(trimmed).is_ok() {
        return vec![trimmed.to_string()];
    }
    let mut out = Vec::new();
    for line in trimmed.lines() {
        let Some(data) = line.strip_prefix("data:") else {
            continue;
        };
        let data = data.trim();
        if data.is_empty() || data == "[DONE]" {
            continue;
        }
        if serde_json::from_str::<serde_json::Value>(data).is_ok() {
            out.push(data.to_string());
        }
    }
    out
}

pub fn first_line(text: &str) -> String {
    let line = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
    if line.is_empty() {
        "the broker returned nothing that is an MCP message".to_string()
    } else {
        line.to_string()
    }
}

/// Refuse a runtime that would drop the message on the floor.
///
/// A daemon below [`MCP_BRIDGE_VERSION`] parses `SecretUse` and ignores the
/// `body` it has never heard of, so the secret service is asked to carry an
/// empty message and refuses one — a true error about a cause that is not the
/// real one. The guard names the constant for the reason every guard beside it
/// does.
fn require_a_runtime_that_carries_a_body(
    agent: &mut apex_agent_core::client::Client,
) -> Result<()> {
    let AgentResponse::Hello { version, .. } = agent.call(&AgentRequest::Hello)? else {
        bail!("the agent runtime did not answer the protocol handshake");
    };
    if version < MCP_BRIDGE_VERSION {
        bail!(
            "the running agent runtime speaks protocol {version} and cannot carry an \
             MCP message; restart it with `systemctl --user restart apex-agentd`"
        );
    }
    Ok(())
}

fn current_project_root() -> Option<String> {
    let cwd = std::env::current_dir().ok()?;
    apex_agent_core::project::detect(&cwd).map(|p| p.root)
}

use std::io::Read;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_json_reply_is_passed_through_untouched() {
        // Byte-for-byte: an MCP client checks the `id` it sent against the `id`
        // it gets, and re-serialising is how a number becomes a float.
        let body = r#"{"jsonrpc":"2.0","id":1,"result":{"tools":[]}}"#;
        assert_eq!(replies(body), vec![body.to_string()]);
    }

    #[test]
    fn an_event_stream_reply_is_unwrapped_to_its_messages() {
        // The other shape a streamable-HTTP server may answer with, chosen by
        // the server per request. A bridge that understood only one of them
        // would work until the day the server changed its mind.
        let body = "event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{}}\n\n";
        assert_eq!(
            replies(body),
            vec![r#"{"jsonrpc":"2.0","id":2,"result":{}}"#.to_string()]
        );
    }

    #[test]
    fn an_empty_or_unparseable_body_produces_no_reply() {
        // Writing a non-message onto the transport is worse than writing
        // nothing: the agent's MCP client would fail to parse it and drop the
        // server, and the error it reported would be about JSON.
        assert!(replies("").is_empty());
        assert!(replies("   \n\n").is_empty());
        assert!(replies("Internal Server Error").is_empty());
        assert!(replies("data: [DONE]\n").is_empty());
    }

    #[test]
    fn several_events_in_one_body_become_several_lines() {
        let body = "data: {\"id\":1}\n\ndata: {\"id\":2}\n\n";
        assert_eq!(replies(body).len(), 2);
    }

    #[test]
    fn a_refusal_is_answered_with_an_error_addressed_to_the_message() {
        // The failure this closes: a refused message answered with silence
        // leaves the agent waiting on the `id` it sent, so a server that is
        // merely unauthorised is reported as one that is broken.
        let id = serde_json::json!(7);
        let reply = rpc_error(Some(&id), "'mcp-request' is not granted for this project");
        let doc: serde_json::Value = serde_json::from_str(&reply).expect("valid json-rpc");
        assert_eq!(doc["id"], 7);
        assert_eq!(doc["jsonrpc"], "2.0");
        assert_eq!(doc["error"]["code"], -32603);
        assert!(doc["error"]["message"].as_str().unwrap().contains("not granted"));
        assert!(!reply.contains('\n'), "the transport is line-delimited: {reply}");
    }

    #[test]
    fn a_message_with_a_quote_in_it_still_produces_parseable_json() {
        let reply = rpc_error(None, "the server said \"no\"\nand then stopped");
        let doc: serde_json::Value = serde_json::from_str(&reply).expect("valid json-rpc");
        assert!(doc["id"].is_null());
        assert!(!reply.contains('\n'), "{reply}");
    }

    #[test]
    fn the_listing_shows_the_server_that_runs_and_not_the_wrapper_around_it() {
        // The regression this closes: once `apex mcp confine` has run, every
        // local server's definition begins `apex mcp run`, and a listing that
        // reported the definition verbatim would say `stdio, apex mcp run
        // memory --` for all of them — turning the one place a person looks to
        // find out what their machine runs into a list of the same four words.
        let wrapped = servers::Transport::Stdio {
            command: "apex".to_string(),
            args: ["mcp", "run", "memory", "--", "npx", "-y", "@modelcontextprotocol/server-memory"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
        };
        assert_eq!(
            describe_transport(&wrapped),
            "stdio, npx -y @modelcontextprotocol/server-memory"
        );

        // And an unwrapped definition is still reported as itself, so the
        // unwrapping cannot be what produces the command.
        let plain = servers::Transport::Stdio {
            command: "npx".to_string(),
            args: vec!["-y".to_string(), "@modelcontextprotocol/server-memory".to_string()],
        };
        assert_eq!(
            describe_transport(&plain),
            "stdio, npx -y @modelcontextprotocol/server-memory"
        );

        // A command that merely looks like the wrapper is not unwrapped: this
        // must match on the argv, not on the word `apex`.
        let lookalike = servers::Transport::Stdio {
            command: "apex-helper".to_string(),
            args: vec!["mcp".to_string(), "run".to_string(), "x".to_string(), "--".to_string()],
        };
        assert!(describe_transport(&lookalike).starts_with("stdio, apex-helper"));
    }

    #[test]
    fn a_server_that_starts_unconfined_says_so_and_says_what_to_run() {
        // Pure: no policy file is read for a definition that has no wrapper in
        // it and that the launcher leaves alone, so this branch cannot depend
        // on the machine it runs on.
        let line = describe_sandbox(
            "memory",
            "npx",
            &["-y".to_string()],
            Some(mcpconf::Wrap::Not),
        );
        assert!(line.starts_with("none —"), "{line}");
        assert!(line.contains("apex mcp confine memory"), "{line}");
    }

    #[test]
    fn a_server_the_launcher_wraps_is_not_listed_as_starting_unconfined() {
        // The same bare definition, belonging to a plugin rather than to the
        // user. `confined_server` says nothing wraps it and that is still
        // true of the FILE; it is false of the session, and the line that
        // stopped at the file was telling a person their plugin's server runs
        // with everything the session has.
        let line = describe_sandbox(
            "plugin:p:srv",
            "node",
            &["s.js".to_string()],
            Some(mcpconf::Wrap::AtLaunch),
        );
        assert!(!line.starts_with("none —"), "{line}");
        assert!(line.contains("wraps it at launch"), "{line}");
        // And the qualifier, without which the line over-claims in the other
        // direction.
        assert!(line.contains("Start the agent yourself"), "{line}");
        assert!(line.contains("apex mcp confine plugin:p:srv"), "{line}");
        // This arm does consult the policy file the wrapper would apply, so
        // unlike the one above it touches the machine — deliberately not
        // asserted on, because both a policy and an unreadable policy are
        // correct answers here and only the wrapping is this test's subject.
    }

    #[test]
    fn a_widened_policy_cannot_read_the_same_as_a_closed_one() {
        // The mistake a one-line summary is most likely to make: "its own"
        // printed for a server whose policy file handed back the network, the
        // project and the broker, which is every dimension P1-019 has.
        let closed = summarise(&sidecar::McpPolicy::closed("memory"));
        assert_eq!(closed, "closed in every dimension");

        let mut open = sidecar::McpPolicy::closed("memory");
        open.network = true;
        open.project = true;
        open.broker = true;
        open.source = sidecar::Source::File("/etc/apex/mcp/memory.toml".into());
        let line = summarise(&open);
        assert!(line.contains("the network"), "{line}");
        assert!(line.contains("the project"), "{line}");
        assert!(line.contains("the broker"), "{line}");
        // Where it was decided, because the next question after "opened" is
        // "by which file".
        assert!(line.contains("/etc/apex/mcp/memory.toml"), "{line}");
        assert_ne!(line, closed);

        // Each of the quieter three on its own, so none of them can be widened
        // without the listing saying a word about it.
        for (mutate, expected) in [
            (
                Box::new(|p: &mut sidecar::McpPolicy| p.read = vec!["/usr/share/dict".into()])
                    as Box<dyn Fn(&mut sidecar::McpPolicy)>,
                "paths it reads",
            ),
            (
                Box::new(|p: &mut sidecar::McpPolicy| p.write = vec!["/tmp/notes".into()]),
                "paths it writes",
            ),
            (
                Box::new(|p: &mut sidecar::McpPolicy| p.env = vec!["NODE_OPTIONS".to_string()]),
                "variables from the session",
            ),
        ] {
            let mut policy = sidecar::McpPolicy::closed("memory");
            mutate(&mut policy);
            let line = summarise(&policy);
            assert!(line.contains(expected), "{expected} was silent: {line}");
        }
    }

    #[test]
    fn an_empty_body_still_produces_a_message_a_client_can_read() {
        // first_line of nothing must not be an empty error message: an MCP
        // client shows it to the user, and "" says less than nothing.
        assert!(!first_line("").is_empty());
        assert!(!first_line("\n \n").is_empty());
        assert_eq!(first_line("401 Unauthorized\nsecond"), "401 Unauthorized");
    }

}
