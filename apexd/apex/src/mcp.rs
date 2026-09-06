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

use anyhow::{bail, Result};
use apex_agent_core::protocol::{
    Request as AgentRequest, Response as AgentResponse, MCP_BRIDGE_VERSION,
};
use clap::Subcommand;

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
    }
}

/// The longest JSON-RPC message this will carry in either direction.
///
/// The same bound `apex-secretd` applies to a message body, restated here so a
/// message too large to send is refused with an explanation rather than by a
/// daemon closing the connection.
const MAX_MESSAGE_BYTES: usize = 1024 * 1024;

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
            capability: "mcp-request".to_string(),
            remote: String::new(),
            branch: None,
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

fn first_line(text: &str) -> String {
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
    fn an_empty_body_still_produces_a_message_a_client_can_read() {
        // first_line of nothing must not be an empty error message: an MCP
        // client shows it to the user, and "" says less than nothing.
        assert!(!first_line("").is_empty());
        assert!(!first_line("\n \n").is_empty());
        assert_eq!(first_line("401 Unauthorized\nsecond"), "401 Unauthorized");
    }

    #[test]
    fn the_guard_names_a_revision_the_protocol_has_reached() {
        // A guard ahead of the protocol can never fire, which is the fail-open
        // this and the two beside it exist to catch.
        assert!(MCP_BRIDGE_VERSION <= apex_agent_core::protocol::PROTOCOL_VERSION);
    }
}
