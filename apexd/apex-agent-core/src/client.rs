//! Blocking client for the `apex-agentd` control socket.
//!
//! Blocking rather than async on purpose: the `apex` CLI does one thing and
//! exits, and `apex agent attach` is a raw terminal relay whose whole job is to
//! block on two file descriptors. A runtime would add a dependency and a
//! scheduler to a program whose hot path is `read`/`write`.

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};

use crate::paths;
use crate::protocol::{ErrorKind, Request, Response, SessionInfo};

/// How long to wait for the daemon to answer a control request.
///
/// Generous, because `Run` does real work (checkpoint capture on a large
/// repository, worktree creation) before it can answer, and a spurious timeout
/// there would leave a session running that the CLI then claims failed.
const CONTROL_TIMEOUT: Duration = Duration::from_secs(120);

/// A connection to the daemon.
#[derive(Debug)]
pub struct Client {
    stream: UnixStream,
    reader: Option<BufReader<UnixStream>>,
}

impl Client {
    /// Connect to the daemon's control socket.
    pub fn connect() -> Result<Client> {
        Client::connect_at(&paths::control_socket())
    }

    /// Connect to a specific socket path, for tests and for a non-default
    /// runtime directory.
    pub fn connect_at(path: &Path) -> Result<Client> {
        let stream = UnixStream::connect(path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound
                || e.kind() == std::io::ErrorKind::ConnectionRefused
            {
                anyhow!(
                    "the agent runtime is not running.\n\
                     enable it with: apex agent enable"
                )
            } else {
                anyhow!("cannot reach the agent runtime at {}: {e}", path.display())
            }
        })?;
        stream.set_read_timeout(Some(CONTROL_TIMEOUT)).ok();
        stream.set_write_timeout(Some(CONTROL_TIMEOUT)).ok();
        let reader = BufReader::new(stream.try_clone().context("cloning the control socket")?);
        Ok(Client {
            stream,
            reader: Some(reader),
        })
    }

    /// Whether a daemon is listening, without producing an error if not.
    pub fn is_running() -> bool {
        UnixStream::connect(paths::control_socket()).is_ok()
    }

    /// Send one request and read one response.
    pub fn request(&mut self, req: &Request) -> Result<Response> {
        let mut line = serde_json::to_string(req)?;
        line.push('\n');
        self.stream
            .write_all(line.as_bytes())
            .context("sending to the agent runtime")?;
        self.stream.flush().ok();

        // A request that authenticates has no deadline the CLI can pick: the
        // deadline belongs to the person reading the polkit dialog. See
        // [`Request::waits_on_a_human`].
        let waiting = req.waits_on_a_human();
        if waiting {
            self.stream.set_read_timeout(None).ok();
        }
        let reply = self.read_reply();
        if waiting {
            // Back to the ordinary deadline: this connection may carry more
            // requests, and only the authenticating one was open-ended.
            self.stream.set_read_timeout(Some(CONTROL_TIMEOUT)).ok();
        }
        reply
    }

    /// Read one reply line off the connection.
    fn read_reply(&mut self) -> Result<Response> {
        let reader = self
            .reader
            .as_mut()
            .ok_or_else(|| anyhow!("this connection has been switched to raw mode"))?;
        let mut buf = String::new();
        let n = reader
            .read_line(&mut buf)
            .context("reading from the agent runtime")?;
        if n == 0 {
            bail!("the agent runtime closed the connection without replying");
        }
        serde_json::from_str(buf.trim_end())
            .with_context(|| format!("cannot parse the runtime's reply: {}", buf.trim_end()))
    }

    /// Send a request and turn an error response into an `Err`.
    pub fn call(&mut self, req: &Request) -> Result<Response> {
        let resp = self.request(req)?;
        if let Some((kind, message)) = resp.as_error() {
            bail!(describe_error(kind, message));
        }
        Ok(resp)
    }

    /// Consume the client, yielding the raw stream after an accepted attach.
    pub fn into_raw(self) -> Result<UnixStream> {
        // Drop the buffered reader, but keep whatever it had already read: for
        // attach the daemon sends the response line and then only PTY bytes, so
        // anything buffered past the newline is session output that must not be
        // lost.
        Ok(self.stream)
    }

    /// The buffered bytes that arrived alongside the response line.
    ///
    /// Read after an attach response so the first burst of replayed scrollback
    /// is not stranded inside the reader's buffer.
    pub fn take_buffered(&mut self) -> Vec<u8> {
        let Some(reader) = self.reader.as_mut() else {
            return Vec::new();
        };
        let buffered = reader.buffer().to_vec();
        if !buffered.is_empty() {
            // Mark them consumed so a later read does not repeat them.
            reader.consume(buffered.len());
        }
        buffered
    }

    /// A clone of the underlying stream.
    pub fn try_clone_stream(&self) -> Result<UnixStream> {
        self.stream
            .try_clone()
            .context("cloning the control socket")
    }

    /// Clear the read timeout, for a connection that is about to relay a
    /// session that may sit idle for hours.
    pub fn clear_timeouts(&self) {
        self.stream.set_read_timeout(None).ok();
        self.stream.set_write_timeout(None).ok();
    }
}

/// Turn a protocol error into a message that tells the user what to do.
pub fn describe_error(kind: ErrorKind, message: &str) -> String {
    match kind {
        ErrorKind::NoSuchSession => {
            format!("{message}\nrun `apex agent list` to see current sessions")
        }
        ErrorKind::NoSuchAgent => {
            format!("{message}\nknown agents: {}", crate::adapter::ids().join(", "))
        }
        ErrorKind::SandboxUnavailable => message.to_string(),
        _ => message.to_string(),
    }
}

/// Convenience: one request on a fresh connection.
pub fn call(req: &Request) -> Result<Response> {
    Client::connect()?.call(req)
}

/// Every session the daemon knows about.
pub fn sessions() -> Result<Vec<SessionInfo>> {
    match call(&Request::List)? {
        Response::Sessions { sessions } => Ok(sessions),
        other => bail!("unexpected reply to list: {other:?}"),
    }
}

/// One session.
pub fn session(id: u32) -> Result<SessionInfo> {
    match call(&Request::Info { id })? {
        Response::Session(info) => Ok(*info),
        other => bail!("unexpected reply to info: {other:?}"),
    }
}

/// Publish a state event for a session.
///
/// This is the open agent event protocol from the client side: an agent hook,
/// a shell function or a script calls `apex agent event`, which lands here.
pub fn publish_event(id: u32, state: &str, detail: Option<String>) -> Result<()> {
    call(&Request::Event {
        id,
        state: Some(state.to_string()),
        event: None,
        detail,
        // `apex agent event` is the open protocol: a shell function or a
        // script publishing a state. Nothing there is an agent describing its
        // own permission mode, so this stays absent rather than being made a
        // flag anybody could set.
        native: None,
        // Nor is anything there a subagent. The graph is built from Claude's
        // own lifecycle events; a script that could name a subagent could name
        // one that never ran.
        agent_id: None,
        agent_type: None,
    })?;
    Ok(())
}

/// Publish one of Claude's lifecycle events (§6.1).
///
/// Separate from [`publish_event`] because the two carry different things: a
/// state is an opinion about what the session is doing, and an event is a fact
/// about what happened. Some events imply a state and some do not, and
/// [`crate::hook::observe`] is what decides which — not this function, and not
/// the daemon.
///
/// Takes the whole [`crate::hook::Observation`] rather than its fields one by
/// one. It grew a sixth and a seventh with the session graph, and a call site
/// passing seven positional `Option<String>`s is one reordering away from
/// publishing a subagent's type as its id.
pub fn publish_hook(id: u32, obs: &crate::hook::Observation) -> Result<()> {
    call(&Request::Event {
        id,
        state: obs.state.map(|s| s.as_str().to_string()),
        event: Some(obs.event.as_str().to_string()),
        detail: obs.detail.clone(),
        native: obs.native.clone(),
        agent_id: obs.agent_id.clone(),
        agent_type: obs.agent_type.clone(),
    })?;
    Ok(())
}

/// Publish what Claude's status line reported (§P1-021).
///
/// Separate from [`publish_hook`] because a status line is not an event:
/// nothing happened, a timer fired. See [`Request::Telemetry`].
pub fn publish_telemetry(id: u32, t: &crate::statusline::Telemetry) -> Result<()> {
    call(&Request::Telemetry {
        id,
        telemetry: Box::new(t.clone()),
    })?;
    Ok(())
}

/// Read the session id from the environment, for a process running *inside* a
/// session that wants to report on itself.
///
/// The daemon sets `APEX_AGENT_SESSION` in every session it starts, so a hook
/// script needs no arguments to know which session it belongs to.
pub const SESSION_ENV: &str = "APEX_AGENT_SESSION";

/// The session this process is running inside, if any.
pub fn current_session() -> Option<u32> {
    std::env::var(SESSION_ENV).ok()?.trim().parse().ok()
}

/// Read `bytes` of a session's transcript.
pub fn logs(id: u32, bytes: usize) -> Result<String> {
    match call(&Request::Logs { id, bytes })? {
        Response::Logs { text, .. } => Ok(text),
        other => bail!("unexpected reply to logs: {other:?}"),
    }
}

/// The socket path a client will use, for diagnostics.
pub fn socket_path() -> PathBuf {
    paths::control_socket()
}

/// Relay bytes between the terminal and an attached session.
///
/// Returns `Ok(true)` when the user detached with the detach key and
/// `Ok(false)` when the session ended on its own. Both are normal outcomes and
/// the distinction is what the caller prints afterwards.
pub fn relay(
    mut sock_read: UnixStream,
    mut sock_write: UnixStream,
    prelude: &[u8],
    detach: u8,
) -> Result<bool> {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    let detached = Arc::new(AtomicBool::new(false));

    {
        let mut out = std::io::stdout().lock();
        out.write_all(prelude).ok();
        out.flush().ok();
    }

    // Terminal -> session. A separate thread because both directions block.
    let writer_detached = Arc::clone(&detached);
    let input = std::thread::spawn(move || {
        let mut stdin = std::io::stdin().lock();
        let mut buf = [0u8; 4096];
        loop {
            let n = match stdin.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            };
            if let Some(at) = buf[..n].iter().position(|b| *b == detach) {
                // Forward everything before the detach key, then stop.
                if at > 0 && sock_write.write_all(&buf[..at]).is_err() {
                    break;
                }
                writer_detached.store(true, Ordering::SeqCst);
                // Shutting the write half down makes the daemon drop this
                // attachment without touching the session itself.
                let _ = sock_write.shutdown(std::net::Shutdown::Write);
                break;
            }
            if sock_write.write_all(&buf[..n]).is_err() {
                break;
            }
        }
    });

    // Session -> terminal, on this thread.
    let mut buf = [0u8; 8192];
    loop {
        let n = match sock_read.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        };
        let mut out = std::io::stdout().lock();
        if out.write_all(&buf[..n]).is_err() {
            break;
        }
        out.flush().ok();
    }

    let was_detached = detached.load(Ordering::SeqCst);
    if was_detached {
        // The input thread has already finished.
        let _ = input.join();
    } else {
        // The session ended. The input thread is parked in read() on the
        // terminal and cannot be interrupted portably, so it is left to die
        // with the process rather than blocking exit forever.
        drop(input);
    }
    Ok(was_detached)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connecting_to_a_missing_socket_explains_how_to_start_the_runtime() {
        let err = Client::connect_at(Path::new("/nonexistent/apex-agentd.sock")).unwrap_err();
        let text = err.to_string();
        assert!(text.contains("not running"), "{text}");
        // Points at the one command that opts in for any user, root included,
        // rather than a raw systemctl line that does not work as root.
        assert!(text.contains("apex agent enable"), "{text}");
    }

    #[test]
    fn a_missing_session_error_points_at_the_list_command() {
        let text = describe_error(ErrorKind::NoSuchSession, "no session 9");
        assert!(text.contains("no session 9"));
        assert!(text.contains("apex agent list"));
    }

    #[test]
    fn an_unknown_agent_error_lists_the_known_ones() {
        let text = describe_error(ErrorKind::NoSuchAgent, "no agent 'clod'");
        assert!(text.contains("claude"), "{text}");
        assert!(text.contains("generic"), "{text}");
    }

    #[test]
    fn a_sandbox_error_is_passed_through_unchanged() {
        // It already carries its own remedy; appending another would bury it.
        let msg = "bubblewrap is not installed; re-run with `--sandbox unrestricted`";
        assert_eq!(describe_error(ErrorKind::SandboxUnavailable, msg), msg);
    }

    #[test]
    fn the_session_environment_variable_is_read_when_set() {
        // Not set in the test process, so this must be None rather than a
        // panic or a zero.
        if std::env::var(SESSION_ENV).is_err() {
            assert_eq!(current_session(), None);
        }
    }

    #[test]
    fn the_socket_path_is_under_the_runtime_directory() {
        let p = socket_path();
        assert!(p.is_absolute());
        assert!(p.ends_with("apex-agentd/control.sock"), "{}", p.display());
    }
}
