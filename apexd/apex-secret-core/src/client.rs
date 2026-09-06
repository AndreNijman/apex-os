//! Blocking client for `apex-secretd`.
//!
//! Blocking rather than async for the same reason the agent runtime's client is:
//! the `apex` CLI does one thing and exits, and a runtime would add a scheduler
//! to a program whose whole job is one `write` and one `read_line`.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};

use crate::paths::Layout;
use crate::protocol::{ErrorKind, Request, Response};

/// How long to wait for an answer.
///
/// Longer than any provider timeout in [`crate::capability::Constraints`], so a
/// slow provider produces the daemon's own explanation rather than a client-side
/// timeout that says nothing about what happened.
const TIMEOUT: Duration = Duration::from_secs(90);

/// Which socket to talk on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    /// Metadata and operations. Any local uid may connect.
    Broker,
    /// Mutation. `0600`, so in practice root and the service's own uid.
    Admin,
}

#[derive(Debug)]
pub struct Client {
    stream: UnixStream,
    reader: BufReader<UnixStream>,
}

/// Where a client looks for the service, overriding the packaged path.
///
/// For a developer running a private instance with `--runtime-dir`, and for the
/// CLI's own end-to-end tests. Client-side only: it changes which socket this
/// process dials, never where the daemon listens — a systemd unit starts with a
/// clean environment, so nothing can redirect the real service by setting it.
/// Redirecting your own client only fools your own client.
pub const RUNTIME_DIR_ENV: &str = "APEX_SECRETD_RUNTIME_DIR";

impl Client {
    /// Connect to the packaged socket for `channel`.
    pub fn connect(channel: Channel) -> Result<Client> {
        Client::connect_at(&socket_path(channel), channel)
    }

    /// Connect to a specific socket, for tests and for a daemon started with
    /// its own runtime directory.
    pub fn connect_at(path: &Path, channel: Channel) -> Result<Client> {
        let stream = UnixStream::connect(path).map_err(|e| explain(e, path, channel))?;
        stream.set_read_timeout(Some(TIMEOUT)).ok();
        stream.set_write_timeout(Some(TIMEOUT)).ok();
        let reader = BufReader::new(stream.try_clone().context("cloning the socket")?);
        Ok(Client { stream, reader })
    }

    /// Whether the broker socket is answering.
    pub fn is_running() -> bool {
        UnixStream::connect(socket_path(Channel::Broker)).is_ok()
    }

    /// Send one request, read one response.
    pub fn request(&mut self, req: &Request) -> Result<Response> {
        let mut line = serde_json::to_string(req)?;
        line.push('\n');
        self.stream
            .write_all(line.as_bytes())
            .context("sending to the secret service")?;
        self.stream.flush().ok();

        let mut buf = String::new();
        let n = self
            .reader
            .read_line(&mut buf)
            .context("reading from the secret service")?;
        if n == 0 {
            bail!("the secret service closed the connection without replying");
        }
        serde_json::from_str(buf.trim_end())
            .with_context(|| format!("cannot parse the reply: {}", buf.trim_end()))
    }

    /// Send a request and turn a refusal into an `Err`.
    pub fn call(&mut self, req: &Request) -> Result<Response> {
        let resp = self.request(req)?;
        if let Some((kind, message)) = resp.as_error() {
            bail!(describe_error(kind, message));
        }
        Ok(resp)
    }
}

fn explain(e: std::io::Error, path: &Path, channel: Channel) -> anyhow::Error {
    if e.kind() == std::io::ErrorKind::PermissionDenied && channel == Channel::Admin {
        return anyhow!(
            "changing what is stored needs the admin socket, which only root \
             can open.\nre-run this with sudo"
        );
    }
    if e.kind() == std::io::ErrorKind::NotFound || e.kind() == std::io::ErrorKind::ConnectionRefused
    {
        return anyhow!(
            "the secret service is not running.\n\
             start it with: sudo systemctl start apex-secretd"
        );
    }
    anyhow!("cannot reach the secret service at {}: {e}", path.display())
}

/// Turn a refusal into a message that says what to do next.
pub fn describe_error(kind: ErrorKind, message: &str) -> String {
    match kind {
        ErrorKind::NoSuchSecret => {
            format!("{message}\nrun `apex capability list` to see what is stored")
        }
        ErrorKind::NoSuchOperation => {
            format!("{message}\nrun `apex capability providers` for the vocabulary")
        }
        _ => message.to_string(),
    }
}

/// One request on a fresh connection.
pub fn call(channel: Channel, req: &Request) -> Result<Response> {
    Client::connect(channel)?.call(req)
}

/// The socket a client will use.
pub fn socket_path(channel: Channel) -> PathBuf {
    let layout = match std::env::var_os(RUNTIME_DIR_ENV) {
        Some(dir) if !dir.is_empty() => Layout::new(Layout::default().state_dir(), dir),
        _ => Layout::default(),
    };
    match channel {
        Channel::Broker => layout.broker_socket(),
        Channel::Admin => layout.admin_socket(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connecting_to_a_missing_socket_says_how_to_start_the_service() {
        let err = Client::connect_at(
            Path::new("/nonexistent/apex-secretd/broker.sock"),
            Channel::Broker,
        )
        .unwrap_err();
        let text = err.to_string();
        assert!(text.contains("not running"), "{text}");
        assert!(text.contains("systemctl start apex-secretd"), "{text}");
    }

    #[test]
    fn a_missing_credential_error_points_at_the_listing_command() {
        let text = describe_error(ErrorKind::NoSuchSecret, "no credential called 'gh'");
        assert!(text.contains("no credential called 'gh'"), "{text}");
        assert!(text.contains("apex capability list"), "{text}");
    }

    #[test]
    fn an_unknown_operation_error_points_at_the_vocabulary() {
        let text = describe_error(ErrorKind::NoSuchOperation, "'raw' is not an operation");
        assert!(text.contains("apex capability providers"), "{text}");
    }

    #[test]
    fn a_permission_error_on_the_admin_socket_says_to_use_sudo() {
        // What an ordinary user hits when they try to store a credential: the
        // socket is 0600 and owned by the service, so `connect` fails with
        // EACCES and the message has to explain rather than say "denied".
        let err = explain(
            std::io::Error::from(std::io::ErrorKind::PermissionDenied),
            Path::new("/run/apex-secretd/admin.sock"),
            Channel::Admin,
        );
        assert!(err.to_string().contains("sudo"), "{err}");
    }

    #[test]
    fn the_two_channels_resolve_to_different_sockets() {
        assert_ne!(socket_path(Channel::Broker), socket_path(Channel::Admin));
        // Written against whatever the environment actually is rather than by
        // setting a variable: `std::env::set_var` is process-global and races
        // every other test in this crate.
        match std::env::var_os(RUNTIME_DIR_ENV) {
            Some(dir) if !dir.is_empty() => {
                assert!(socket_path(Channel::Broker).starts_with(&dir));
            }
            _ => {
                let p = socket_path(Channel::Broker);
                assert!(p.starts_with("/run/apex-secretd"), "{}", p.display());
            }
        }
    }
}
