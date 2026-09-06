//! Blocking client for the `apex-secretd` control socket.
//!
//! Blocking rather than async for the same reason `apex_agent_core::client` is:
//! the callers are the `apex` CLI, which does one thing and exits, and one
//! request-handling thread inside `apex-agentd`. A runtime would add a
//! scheduler to a program whose hot path is `read`/`write`.
//!
//! The client is the *only* thing in the tree that constructs a
//! [`crate::SecretValue`] from user input, and it does it in one place:
//! [`Client::add`], which writes the bytes after the request line rather than
//! inside it.

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};

use crate::paths;
use crate::protocol::{ErrorKind, Request, Response, MAX_RESPONSE_BYTES};
use crate::value::SecretValue;

/// How long to wait for the daemon to answer.
///
/// Generous, because [`Request::Use`] does real work — a push to a slow remote
/// — before it can answer, and a spurious timeout there would leave the caller
/// believing an operation failed that in fact ran.
const TIMEOUT: Duration = Duration::from_secs(180);

/// A connection to `apex-secretd`.
#[derive(Debug)]
pub struct Client {
    stream: UnixStream,
    reader: BufReader<UnixStream>,
}

impl Client {
    /// Connect to the daemon's control socket.
    pub fn connect() -> Result<Client> {
        Client::connect_at(&paths::socket())
    }

    /// Connect to a specific socket, for tests and for a second instance.
    pub fn connect_at(path: &Path) -> Result<Client> {
        let stream = UnixStream::connect(path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound
                || e.kind() == std::io::ErrorKind::ConnectionRefused
            {
                anyhow!(
                    "the secret service is not running.\n\
                     start it with: sudo systemctl start apex-secretd"
                )
            } else {
                anyhow!(
                    "cannot reach the secret service at {}: {e}",
                    path.display()
                )
            }
        })?;
        stream.set_read_timeout(Some(TIMEOUT)).ok();
        stream.set_write_timeout(Some(TIMEOUT)).ok();
        let reader = BufReader::new(stream.try_clone().context("cloning the control socket")?);
        Ok(Client { stream, reader })
    }

    /// Whether a daemon is listening, without producing an error if not.
    pub fn is_running() -> bool {
        UnixStream::connect(paths::socket()).is_ok()
    }

    /// Send one request and read one response.
    pub fn request(&mut self, req: &Request) -> Result<Response> {
        self.send(req)?;
        self.read_response()
    }

    /// Send a request and turn an error response into an `Err`.
    pub fn call(&mut self, req: &Request) -> Result<Response> {
        let resp = self.request(req)?;
        if let Some((_, message)) = resp.as_error() {
            bail!("{message}");
        }
        Ok(resp)
    }

    /// Store a credential.
    ///
    /// Two writes: the request line, then exactly `value.len()` raw bytes. The
    /// value is never part of the JSON — see [`crate::protocol`].
    #[allow(clippy::too_many_arguments)]
    pub fn add(
        &mut self,
        service: &str,
        host: &str,
        scheme: &str,
        username: Option<&str>,
        path: &str,
        auth: &str,
        port: Option<u16>,
        value: &SecretValue,
    ) -> Result<Response> {
        if value.is_empty() {
            bail!("that credential is empty; nothing was stored");
        }
        if value.len() > SecretValue::MAX_BYTES {
            bail!(
                "that credential is {} bytes; the limit is {}",
                value.len(),
                SecretValue::MAX_BYTES
            );
        }
        self.send(&Request::Add {
            service: service.to_string(),
            host: host.to_string(),
            scheme: scheme.to_string(),
            username: username.map(str::to_string),
            path: path.to_string(),
            auth: Some(auth.to_string()),
            port,
            value_len: value.len(),
        })?;
        self.stream
            .write_all(value.expose())
            .context("sending to the secret service")?;
        self.stream.flush().ok();
        let resp = self.read_response()?;
        if let Some((_, message)) = resp.as_error() {
            bail!("{message}");
        }
        Ok(resp)
    }

    /// Perform a capability that carries a message body.
    ///
    /// Two writes, like [`Client::add`]: the request line, then exactly
    /// `body.len()` raw bytes. Used by `mcp-request`, whose body is the caller's
    /// own JSON-RPC message and can be larger than a request line may be.
    pub fn use_with_body(
        &mut self,
        record: crate::capability::CapabilityRecord,
        body: &[u8],
    ) -> Result<Response> {
        self.send(&Request::Use {
            record: Box::new(record),
            body_len: body.len(),
        })?;
        if !body.is_empty() {
            self.stream
                .write_all(body)
                .context("sending to the secret service")?;
        }
        self.stream.flush().ok();
        self.read_response()
    }

    fn send(&mut self, req: &Request) -> Result<()> {
        let mut line = serde_json::to_string(req)?;
        line.push('\n');
        self.stream
            .write_all(line.as_bytes())
            .context("sending to the secret service")?;
        self.stream.flush().ok();
        Ok(())
    }

    fn read_response(&mut self) -> Result<Response> {
        let mut buf = String::new();
        let n = (&mut self.reader)
            .take(MAX_RESPONSE_BYTES as u64)
            .read_line(&mut buf)
            .context("reading from the secret service")?;
        if n == 0 {
            bail!("the secret service closed the connection without replying");
        }
        serde_json::from_str(buf.trim_end())
            .with_context(|| format!("cannot parse the reply: {}", buf.trim_end()))
    }
}

/// Convenience: one request on a fresh connection.
pub fn call(req: &Request) -> Result<Response> {
    Client::connect()?.call(req)
}

/// Turn a protocol error into a message that tells the user what to do.
pub fn describe_error(kind: ErrorKind, message: &str) -> String {
    match kind {
        ErrorKind::NoSuchService => {
            format!("{message}\nrun `apex secret list` to see stored credentials")
        }
        _ => message.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connecting_to_a_missing_socket_explains_how_to_start_the_service() {
        let err = Client::connect_at(Path::new("/nonexistent/apex-secretd.sock")).unwrap_err();
        let text = err.to_string();
        assert!(text.contains("not running"), "{text}");
        assert!(text.contains("systemctl start apex-secretd"), "{text}");
    }

    #[test]
    fn a_missing_service_error_points_at_the_list_command() {
        let text = describe_error(ErrorKind::NoSuchService, "no credential stored for 'x'");
        assert!(text.contains("apex secret list"), "{text}");
    }

    #[test]
    fn an_empty_credential_is_refused_before_a_connection_is_needed() {
        // `apex secret add` reads the value from stdin, and an empty stdin —
        // a pipe from a command that printed nothing — must be an error rather
        // than a stored credential that fails mysteriously at use time.
        let (a, _b) = UnixStream::pair().unwrap();
        let mut client = Client {
            reader: BufReader::new(a.try_clone().unwrap()),
            stream: a,
        };
        let err = client
            .add("demo", "github.com", "https", None, "", "bearer", None, &SecretValue::new(Vec::new()))
            .unwrap_err();
        assert!(err.to_string().contains("empty"), "{err}");
    }
}
