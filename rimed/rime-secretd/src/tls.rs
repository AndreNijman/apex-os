//! Originating TLS to a site this daemon holds a credential for.
//!
//! The only client in this daemon that is not `curl`. Every provider drives a
//! tool — `git`, `curl` — and hands it a credential through a file only root
//! can read; [`crate::present`] cannot, because what it carries is a browser's
//! own connection rather than one request this daemon composed.
//!
//! The roots are the machine's, through `rustls-native-certs`, and an empty
//! store is an error rather than a fall back to a vendored bundle: a daemon
//! that silently trusted a different set of authorities from the rest of the
//! machine would be a daemon whose refusals nobody could reproduce. The
//! `SSL_CERT_FILE` and `SSL_CERT_DIR` environment variables are what
//! `rustls-native-certs` reads first, which is also how a test points this at a
//! CA it minted rather than at the machine's.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::Arc;

use rustls::pki_types::ServerName;
use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned};

/// A connected, handshaken TLS session.
pub struct Session {
    inner: StreamOwned<ClientConnection, TcpStream>,
}

impl Session {
    /// Connect over an already-open socket and complete the handshake.
    ///
    /// The handshake is completed HERE rather than left to the first write, so
    /// that a refused certificate is reported as "this site's certificate was
    /// refused" at the moment the capsule's connection is being set up, and not
    /// later as a write error with no chain in it.
    pub fn connect(sock: TcpStream, server_name: &str) -> Result<Session, String> {
        let name = ServerName::try_from(server_name.to_string())
            .map_err(|e| format!("'{server_name}' is not a server name: {e}"))?;
        let config = config()?;
        let mut conn = ClientConnection::new(config, name)
            .map_err(|e| format!("could not start TLS to {server_name}: {e}"))?;
        let mut sock = sock;
        while conn.is_handshaking() {
            conn.complete_io(&mut sock).map_err(|e| {
                format!("the TLS handshake with {server_name} did not complete: {e}")
            })?;
        }
        Ok(Session {
            inner: StreamOwned::new(conn, sock),
        })
    }

    /// End this side of the conversation: a `close_notify` and then a TCP
    /// half-close, so a site that reads until end-of-request answers.
    pub fn shutdown_write(&mut self) {
        self.inner.conn.send_close_notify();
        let _ = self.inner.conn.complete_io(&mut self.inner.sock);
        let _ = self.inner.sock.shutdown(std::net::Shutdown::Write);
    }
}

impl Read for Session {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.inner.read(buf)
    }
}

impl Write for Session {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.inner.write(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

/// The client configuration, built per connection.
///
/// Per connection rather than cached, because this path opens one connection
/// per request a capsule makes and the store is read in milliseconds — and a
/// cached root set is a daemon that keeps trusting an authority the
/// administrator removed until it is restarted.
fn config() -> Result<Arc<ClientConfig>, String> {
    let found = rustls_native_certs::load_native_certs();
    let mut roots = RootCertStore::empty();
    let (added, unparsable) = roots.add_parsable_certificates(found.certs);
    if added == 0 {
        let complaints: Vec<String> = found.errors.iter().map(|e| e.to_string()).collect();
        return Err(format!(
            "this machine has no usable certificate authorities, so no site's identity \
             can be checked ({unparsable} unparsable{})",
            if complaints.is_empty() {
                String::new()
            } else {
                format!("; {}", complaints.join("; "))
            }
        ));
    }
    // The provider is named rather than left to the crate-features default,
    // which depends on a process-global another crate in the same binary could
    // have set first.
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| format!("TLS could not be configured: {e}"))?
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(Arc::new(config))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The machine's own roots load, and a session cannot be built without
    /// them.
    ///
    /// Not "the function returns Ok": the count is what says the store was
    /// actually read, and a config built over an empty store would accept
    /// nothing while looking exactly like this one from the outside.
    #[test]
    fn the_machines_own_authorities_are_what_a_site_is_checked_against() {
        let found = rustls_native_certs::load_native_certs();
        let mut roots = RootCertStore::empty();
        let (added, _) = roots.add_parsable_certificates(found.certs);
        if added == 0 {
            eprintln!("SKIP: this machine has no certificate authorities installed");
            return;
        }
        assert!(config().is_ok());
        assert!(
            added > 1,
            "only {added} authority parsed out of the system store, which is not a \
             system store"
        );
    }

    /// A name a certificate could not be checked against is refused before a
    /// socket is used, rather than turning into a handshake against whatever
    /// the address resolves to.
    #[test]
    fn a_destination_that_is_not_a_server_name_is_refused() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        std::thread::spawn(move || {
            let _ = listener.accept();
        });
        let sock = TcpStream::connect(addr).expect("connect");
        let e = match Session::connect(sock, "not a host name") {
            Ok(_) => panic!("a name no certificate could be checked against was accepted"),
            Err(e) => e,
        };
        assert!(e.contains("is not a server name"), "{e}");
    }
}
