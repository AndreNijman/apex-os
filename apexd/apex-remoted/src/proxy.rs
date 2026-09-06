//! Forwarding a device's frames into `apex-agentd`, with an origin it can
//! trust.
//!
//! ## The rule, and it is the only rule that matters here
//!
//! **Every connection this module opens to `apex-agentd` sends
//! `DeclareOrigin` first, and refuses to send anything else if that fails.**
//!
//! `apex-remoted` is not a managed session, so the daemon classifies it from
//! its own cgroup. As a systemd user unit that is `scheduled-job` — not
//! local, so it cannot approve a root operation, which is right by accident.
//! Started any other way it is `local-terminal`, and then every request this
//! process forwards for a phone is filed as a human at the keyboard. The
//! declaration is what makes the origin a property of *what this connection
//! is carrying* rather than of how the process happened to be started.
//!
//! The daemon checks it: `apex_agent_core::origin::may_declare` refuses any
//! declaration that is not a narrowing, and refuses the two local origins by
//! name whatever was observed. So this cannot launder a remote request into a
//! local one even if it tried, and a build of the daemon that predates the
//! connection latch answers `permission_denied` — loudly, which is why
//! [`Agentd::open`] treats that as fatal rather than carrying on.
//!
//! ## Why one agentd connection per control frame
//!
//! The daemon serves one connection per thread and reads one request per
//! line. Sharing a single connection between a phone's control traffic and
//! its terminals would serialise them behind each other: a `list` that
//! arrived while an `attach` was streaming would wait for the terminal to
//! close. A connection per control frame costs a `connect(2)` on a Unix
//! socket, which is cheap, and a connection per attached PTY for as long as
//! it is attached, which is what the daemon expects anyway.
//!
//! The cost is that the declaration is sent every time. That is a feature: a
//! latch that survived across requests would be state, and state is what has
//! to be got right.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

/// How long to wait for the daemon on a control request.
///
/// Generous, because a request that reaches a human — a system-access grant
/// prompting for authentication — legitimately takes as long as the person
/// does. `apex-agentd`'s own `Request::waits_on_a_human` names those, and a
/// proxy that timed out under them would report a failure while a dialog was
/// still on screen.
const CONTROL_TIMEOUT: Duration = Duration::from_secs(300);

/// One connection to `apex-agentd`, already told where it came from.
pub struct Agentd {
    stream: UnixStream,
    reader: BufReader<UnixStream>,
}

/// Why a proxied connection could not be established or used.
#[derive(Debug)]
pub enum ProxyError {
    /// The daemon is not running, or would not accept a connection.
    Unreachable(String),
    /// The daemon refused the origin declaration.
    ///
    /// Fatal for the connection, and deliberately not recoverable. A daemon
    /// that will not record where a request came from is a daemon this
    /// service must not send it to.
    OriginRefused(String),
    Io(std::io::Error),
}

impl std::fmt::Display for ProxyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProxyError::Unreachable(w) => write!(
                f,
                "the agent runtime is not reachable: {w}. Start it with `apex agent enable`"
            ),
            ProxyError::OriginRefused(w) => write!(
                f,
                "the agent runtime would not record this connection as remote ({w}), so nothing \
                 was forwarded: a remote request filed under a local origin is what §7 exists to \
                 prevent"
            ),
            ProxyError::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for ProxyError {}

impl From<std::io::Error> for ProxyError {
    fn from(e: std::io::Error) -> ProxyError {
        ProxyError::Io(e)
    }
}

impl Agentd {
    /// Open a connection and declare its origin, in that order and with
    /// nothing in between.
    ///
    /// `actor` is the paired device's id. It reaches the daemon's session
    /// records and privilege requests, so a human deciding on a root
    /// operation is told *which* device asked and not only that a remote
    /// something did.
    pub fn open(socket: &Path, actor: &str) -> Result<Agentd, ProxyError> {
        let stream = UnixStream::connect(socket)
            .map_err(|e| ProxyError::Unreachable(format!("{}: {e}", socket.display())))?;
        stream.set_read_timeout(Some(CONTROL_TIMEOUT)).ok();
        stream.set_write_timeout(Some(CONTROL_TIMEOUT)).ok();
        let reader = BufReader::new(stream.try_clone()?);
        let mut agentd = Agentd { stream, reader };
        agentd.declare(actor)?;
        Ok(agentd)
    }

    /// The first line on every connection this service opens.
    fn declare(&mut self, actor: &str) -> Result<(), ProxyError> {
        let line = serde_json::json!({
            "cmd": "declare_origin",
            // §7's name for "a human elsewhere is driving this machine". The
            // wire spelling is `claude-remote-control` and APEX Remote is not
            // Claude Remote Control; reusing it is a deliberate decision, made
            // because the policy table is keyed on it and an eighth origin
            // would be a protocol break for a rename. See the design note.
            "origin": "claude-remote-control",
            "actor": actor,
        })
        .to_string();
        writeln!(self.stream, "{line}")?;
        self.stream.flush()?;
        let mut reply = String::new();
        self.reader.read_line(&mut reply)?;
        let value: serde_json::Value = serde_json::from_str(reply.trim())
            .map_err(|e| ProxyError::OriginRefused(format!("unreadable reply: {e}")))?;
        if value["reply"] == "ok" {
            return Ok(());
        }
        Err(ProxyError::OriginRefused(
            value["message"]
                .as_str()
                .unwrap_or("the runtime did not accept the declaration")
                .to_string(),
        ))
    }

    /// Send one request line and read one reply line.
    pub fn round_trip(&mut self, request: &[u8]) -> Result<Vec<u8>, ProxyError> {
        self.stream.write_all(request)?;
        self.stream.write_all(b"\n")?;
        self.stream.flush()?;
        let mut reply = String::new();
        self.reader.read_line(&mut reply)?;
        Ok(reply.trim_end_matches('\n').as_bytes().to_vec())
    }

    /// Send an `attach` and take the raw stream, plus whatever PTY bytes
    /// arrived alongside the reply.
    ///
    /// The buffered bytes matter: the daemon writes the response line and
    /// then immediately begins the scrollback replay, so a reader that
    /// dropped its buffer would lose the first burst of a terminal the phone
    /// asked to see.
    pub fn attach(mut self, request: &[u8]) -> Result<(UnixStream, Vec<u8>, Vec<u8>), ProxyError> {
        self.stream.write_all(request)?;
        self.stream.write_all(b"\n")?;
        self.stream.flush()?;
        let mut reply = String::new();
        self.reader.read_line(&mut reply)?;
        let buffered = self.reader.buffer().to_vec();
        // A PTY may sit idle for hours; a deadline on it would end the
        // terminal rather than the request.
        self.stream.set_read_timeout(None).ok();
        self.stream.set_write_timeout(None).ok();
        Ok((
            self.stream,
            reply.trim_end_matches('\n').as_bytes().to_vec(),
            buffered,
        ))
    }
}

/// Whether a control payload is an `attach`, which needs a channel rather
/// than a reply.
///
/// Read out of the JSON rather than out of the frame tag, so a client that
/// sends `attach` as an ordinary control frame gets the same treatment as one
/// that opens a channel — and does not get a connection wedged half in the
/// PTY state with the proxy still expecting reply lines.
pub fn is_attach(payload: &[u8]) -> bool {
    serde_json::from_slice::<serde_json::Value>(payload)
        .map(|v| v["cmd"] == "attach")
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_attach_is_recognised_however_the_client_sends_it() {
        assert!(is_attach(br#"{"cmd":"attach","id":1,"cols":80,"rows":24}"#));
        // Key order is not fixed on the wire.
        assert!(is_attach(br#"{"id":1,"cmd":"attach"}"#));
        assert!(!is_attach(br#"{"cmd":"list"}"#));
        assert!(!is_attach(br#"{"cmd":"attach_something_else"}"#));
        // Not JSON at all: not an attach, and not a panic either.
        assert!(!is_attach(b"attach"));
        assert!(!is_attach(b""));
    }

    #[test]
    fn the_declaration_is_the_first_thing_on_the_wire_and_is_fatal_when_refused() {
        // A real Unix socket standing in for the daemon, so this measures the
        // order bytes actually leave rather than the order the code reads in.
        let dir = std::env::temp_dir().join(format!(
            "apex-remoted-proxy-{}-{}",
            std::process::id(),
            apex_remote_core::now_ms()
        ));
        std::fs::create_dir_all(&dir).expect("dir");
        let socket = dir.join("control.sock");
        let listener = std::os::unix::net::UnixListener::bind(&socket).expect("bind");

        // A stand-in daemon that refuses the declaration, as a build without
        // the connection latch does.
        let handle = std::thread::spawn(move || {
            let (stream, _) = listener.accept().expect("accept");
            let mut reader = BufReader::new(stream.try_clone().expect("clone"));
            let mut first = String::new();
            reader.read_line(&mut first).expect("read");
            let mut w = stream;
            writeln!(
                w,
                r#"{{"reply":"error","kind":"permission_denied","message":"only a managed session can narrow its own origin"}}"#
            )
            .expect("write");
            w.flush().ok();
            // Anything the proxy sends after being refused would arrive here.
            let mut second = String::new();
            let n = reader.read_line(&mut second).unwrap_or(0);
            (first, n, second)
        });

        let err = match Agentd::open(&socket, "pixel-8-office") {
            Err(e) => e,
            Ok(_) => panic!("a refused declaration produced a usable connection"),
        };
        assert!(
            matches!(err, ProxyError::OriginRefused(_)),
            "{err}"
        );
        let (first, n, second) = handle.join().expect("join");
        let v: serde_json::Value = serde_json::from_str(first.trim()).expect("json");
        assert_eq!(v["cmd"], "declare_origin", "{first}");
        assert_eq!(v["origin"], "claude-remote-control", "{first}");
        assert_eq!(v["actor"], "pixel-8-office", "{first}");
        assert_eq!(
            n, 0,
            "the proxy sent {second:?} after its declaration was refused"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_accepted_declaration_yields_a_connection_that_forwards() {
        let dir = std::env::temp_dir().join(format!(
            "apex-remoted-proxy-ok-{}-{}",
            std::process::id(),
            apex_remote_core::now_ms()
        ));
        std::fs::create_dir_all(&dir).expect("dir");
        let socket = dir.join("control.sock");
        let listener = std::os::unix::net::UnixListener::bind(&socket).expect("bind");
        let handle = std::thread::spawn(move || {
            let (stream, _) = listener.accept().expect("accept");
            let mut reader = BufReader::new(stream.try_clone().expect("clone"));
            let mut w = stream;
            let mut decl = String::new();
            reader.read_line(&mut decl).expect("read");
            writeln!(w, r#"{{"reply":"ok"}}"#).expect("write");
            w.flush().ok();
            let mut req = String::new();
            reader.read_line(&mut req).expect("read");
            writeln!(w, r#"{{"reply":"sessions","sessions":[]}}"#).expect("write");
            w.flush().ok();
            req
        });

        let mut agentd = Agentd::open(&socket, "pixel-8").expect("open");
        let reply = agentd.round_trip(br#"{"cmd":"list"}"#).expect("round trip");
        assert_eq!(reply, br#"{"reply":"sessions","sessions":[]}"#);
        assert_eq!(handle.join().expect("join").trim(), r#"{"cmd":"list"}"#);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_absent_daemon_is_unreachable_and_names_the_way_out() {
        let e = match Agentd::open(Path::new("/nonexistent/apex-agentd.sock"), "d") {
            Err(e) => e,
            Ok(_) => panic!("connected to nothing"),
        };
        assert!(matches!(e, ProxyError::Unreachable(_)), "{e}");
        assert!(e.to_string().contains("apex agent enable"), "{e}");
    }
}
