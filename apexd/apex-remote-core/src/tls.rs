//! TLS for the relay leg, so that `wss://` is an address this client can dial.
//!
//! [`crate::relay`] speaks RFC 6455 over anything that is `Read + Write`. This
//! module is what supplies that "anything" when the relay's URL asked for TLS.
//!
//! ## Why this exists at all
//!
//! `*.workers.dev` is HTTPS-only. Until this module the client refused a
//! `wss://` relay by name — correctly, because the alternative was opening a
//! plain socket to port 443 and carrying a remote-control session in the clear
//! while the configuration said otherwise — and the consequence was that the
//! relay under `relay/` could be deployed and then not dialled by anything.
//! `ROADMAP/design/P1-052-relay.md` §5.1 costed three ways out and §6 named
//! this as the gap that stops "deployable" meaning "deployed and working".
//!
//! The other two were a custom domain with HTTPS turned off, and an
//! `openssl s_client` child per connection. The first gives up transport
//! encryption for a rendezvous, which would hide the rendezvous id and the
//! traffic pattern from nobody — an on-path observer such as an ISP or a café
//! network is a different party from the relay operator whose view
//! [`crate::rendezvous::Path::disclosure`] already discloses. The second leans
//! on the `curl` precedent in `apex-secretd`, but that precedent is for
//! **one-shot** requests; a relay connection is long-lived and reconnects
//! across every network change, so each reconnect would become a process
//! lifecycle problem and backpressure would run through a child's stdio.
//!
//! ## The root store is the machine's, and that is a decision
//!
//! [`Trust::system`] reads the OS store — `/etc/pki` on this distribution, or
//! whatever `SSL_CERT_FILE` / `SSL_CERT_DIR` name. It does **not** use a root
//! bundle vendored into the binary.
//!
//! The tempting counter-example is the Sigstore root this project pins by
//! fingerprint (`files/system/trust/fulcio-root.pem`), and it is worth being
//! precise about why that is a different case rather than a contradiction:
//! there, the thing being verified **is the OS image**, so the store that
//! ships inside the OS image cannot be the authority on it. A relay dial
//! happens afterwards, on a booted and already-trusted machine, and at that
//! point the machine is exactly the right authority. Two consequences follow
//! and both are the reason for the choice:
//!
//! * An administrator who removes a CA with `update-ca-trust` means it. A
//!   vendored bundle would go on trusting what was deleted, which is a
//!   security regression wearing reproducibility as a disguise.
//! * The realistic private deployment of this relay sits behind an internal CA
//!   or an enterprise TLS proxy. A vendored bundle cannot reach one at all.
//!
//! The price is that a broken `/etc/pki` becomes a relay outage, and that
//! price is paid openly: an empty store is [`TlsError::NoRoots`], **not** a
//! quiet fall back to a vendored bundle, because falling back would re-trust
//! precisely the CA an administrator had just removed. The error names where
//! it looked, because this tree has already had an `/etc` removal pass eat
//! `/etc/pki/nssdb` once and "handshake failed" would have been a useless
//! thing to read that day.
//!
//! ## The hard part: one connection, two threads
//!
//! `apex-remoted` reads relay frames on one thread while another writes —
//! a keepalive ping while a host waits, and the loopback pump once a device is
//! spliced on. For TCP that is two `try_clone`d descriptors and needs no
//! thought. For TLS it needs some: a [`rustls::ClientConnection`] is a single
//! state machine covering both directions, it cannot be cloned, and the
//! obvious `Arc<Mutex<StreamOwned>>` deadlocks the first time the reader
//! blocks in a socket read while holding the lock.
//!
//! So the halves share the connection through a mutex and obey one invariant:
//!
//! > **The reader never holds the connection lock while it is blocked on
//! > socket IO.**
//!
//! [`TlsReader::read`] takes the lock only to feed bytes it already has and to
//! pull plaintext out; when rustls says it needs more, the lock is dropped
//! *before* the blocking `read` on the socket. [`TlsWriter`] may therefore
//! hold the lock across its own socket writes, because the thread it could
//! block against is never holding it.

use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::sync::{Arc, Mutex, MutexGuard};

use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, ServerName};
use rustls::{ClientConfig, ClientConnection, RootCertStore};

/// How much is taken off the socket in one blocking read.
///
/// A TLS record is at most 16 KiB of plaintext plus its overhead, so this is
/// sized to take a whole record in one syscall in the common case without
/// making a per-connection allocation that matters.
const NET_CHUNK: usize = 18 * 1024;

/// Everything that can stop a TLS relay connection from being established.
#[derive(Debug)]
pub enum TlsError {
    /// The machine has no usable root certificates.
    NoRoots(String),
    /// The relay's host is not a name a certificate can be checked against.
    Name(String),
    /// The TLS client could not be constructed at all.
    Setup(rustls::Error),
    /// The relay's **certificate** did not verify — wrong issuer, wrong name,
    /// out of date.
    Refused(rustls::Error),
    /// The handshake failed for a reason that is not about a certificate.
    ///
    /// Separate from [`TlsError::Refused`] because the two send an operator to
    /// different places, and saying "certificate" about a server that
    /// presented none is worse than saying nothing. The case that proves the
    /// split is worth its two lines is in this tree: dialling `wss://` at
    /// something speaking plain HTTP produces a record-decode error, and the
    /// log would otherwise blame a certificate that never existed.
    Handshake(rustls::Error),
    Io(io::Error),
}

impl TlsError {
    /// The certificate complaint inside a refusal, when that is what it was.
    ///
    /// Exists so that a test can assert *which* refusal happened rather than
    /// that something went wrong: a client that accepted any certificate would
    /// also satisfy "it connected", and a client that refused everything for
    /// the wrong reason would satisfy "it failed".
    pub fn certificate(&self) -> Option<&rustls::CertificateError> {
        match self {
            TlsError::Refused(rustls::Error::InvalidCertificate(e)) => Some(e),
            _ => None,
        }
    }
}

impl std::fmt::Display for TlsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TlsError::NoRoots(why) => write!(
                f,
                "this machine has no usable TLS root certificates, so no relay can be \
                 verified and none is dialled ({why}). The store is the system one — \
                 /etc/pki/tls/certs on this distribution, or $SSL_CERT_FILE / $SSL_CERT_DIR \
                 when either is set. APEX Remote still works on this network."
            ),
            TlsError::Name(host) => write!(
                f,
                "{host:?} is not a host name or IP address a certificate can be checked \
                 against, so this relay address cannot be verified"
            ),
            TlsError::Setup(e) => write!(f, "the TLS client could not be built: {e}"),
            TlsError::Refused(e) => {
                write!(f, "the relay's TLS certificate was refused: {e}")
            }
            TlsError::Handshake(e) => write!(f, "the TLS handshake with the relay failed: {e}"),
            TlsError::Io(e) => write!(f, "the relay's TLS connection failed: {e}"),
        }
    }
}

impl std::error::Error for TlsError {}

impl From<io::Error> for TlsError {
    fn from(e: io::Error) -> TlsError {
        TlsError::Io(e)
    }
}

/// A set of root certificates, and the client configuration built from it.
///
/// Built once and shared: loading the system store is a directory walk and a
/// few hundred certificate parses, and the relay reconnects on a two-second
/// backoff.
#[derive(Clone)]
pub struct Trust {
    config: Arc<ClientConfig>,
}

impl Trust {
    /// Trust what this machine trusts.
    ///
    /// This is what the daemon uses. See the module note for why it is the
    /// system store and not a vendored bundle, and why an empty store is an
    /// error rather than a fall back to one.
    pub fn system() -> Result<Trust, TlsError> {
        let found = rustls_native_certs::load_native_certs();
        let complaints: Vec<String> = found.errors.iter().map(|e| e.to_string()).collect();
        let mut roots = RootCertStore::empty();
        let (added, unparsable) = roots.add_parsable_certificates(found.certs);
        if added == 0 {
            return Err(TlsError::NoRoots(format!(
                "nothing in it parsed as a certificate: {unparsable} unparsable, \
                 {} error(s) reading it{}",
                complaints.len(),
                if complaints.is_empty() {
                    String::new()
                } else {
                    format!(" — {}", complaints.join("; "))
                }
            )));
        }
        Trust::with_roots(roots)
    }

    /// Trust exactly the roots handed in.
    ///
    /// The suite uses this with a CA it mints itself, which is how the
    /// refusals can be asserted without a real relay and without reaching the
    /// network at all — the same move `tests/test-apex-trust-enforcement.sh`
    /// makes against a minted CA rather than production sigstore.
    pub fn with_roots(roots: RootCertStore) -> Result<Trust, TlsError> {
        if roots.is_empty() {
            return Err(TlsError::NoRoots("the root store handed in is empty".into()));
        }
        // The provider is named rather than left to
        // `CryptoProvider::get_default_or_install_from_crate_features`, which
        // depends on a process-global that another crate in the same binary
        // could have set first.
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let config = ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(TlsError::Setup)?
            .with_root_certificates(roots)
            .with_no_client_auth();
        Ok(Trust { config: Arc::new(config) })
    }

    /// Trust the certificates in one PEM file, and nothing else.
    pub fn from_pem_file(path: impl AsRef<std::path::Path>) -> Result<Trust, TlsError> {
        let path = path.as_ref();
        let mut roots = RootCertStore::empty();
        let iter = CertificateDer::pem_file_iter(path)
            .map_err(|e| TlsError::NoRoots(format!("{}: {e}", path.display())))?;
        for cert in iter {
            let cert = cert.map_err(|e| TlsError::NoRoots(format!("{}: {e}", path.display())))?;
            roots
                .add(cert)
                .map_err(|e| TlsError::NoRoots(format!("{}: {e}", path.display())))?;
        }
        Trust::with_roots(roots)
    }

    /// Complete a TLS handshake over an already-connected socket.
    ///
    /// `server_name` is the name the certificate is checked against, which is
    /// the relay's host from its URL — never the address that was dialled. The
    /// two differ in exactly the case the suite exercises: a test connects to
    /// `127.0.0.1` and demands a certificate for `relay.test`.
    ///
    /// Returns the two halves, which share one connection. The caller keeps
    /// the `TcpStream` it passed a clone of, because shutting that socket down
    /// is what unblocks a reader parked in [`TlsReader::read`].
    pub fn connect(
        &self,
        socket: TcpStream,
        server_name: &str,
    ) -> Result<(TlsReader, TlsWriter), TlsError> {
        let name = ServerName::try_from(server_name.to_string())
            .map_err(|_| TlsError::Name(server_name.to_string()))?;
        let mut conn = ClientConnection::new(Arc::clone(&self.config), name)
            .map_err(TlsError::Setup)?;

        let mut reading = socket.try_clone()?;
        let mut writing = socket.try_clone()?;
        handshake(&mut conn, &mut reading, &mut writing)?;

        let conn = Arc::new(Mutex::new(conn));
        Ok((
            TlsReader {
                conn: Arc::clone(&conn),
                socket: reading,
                raw: Vec::new(),
                taken: 0,
                seen_eof: false,
            },
            TlsWriter { conn, socket: writing },
        ))
    }
}

/// Drive the opening handshake to completion on one thread.
///
/// Deliberately not `complete_io`: that wraps a [`rustls::Error`] in an
/// `io::Error` and the whole point of this unit is that "your certificate does
/// not verify" survives as something a caller can read and a test can assert
/// on, rather than becoming a generic IO failure.
fn handshake(
    conn: &mut ClientConnection,
    reading: &mut TcpStream,
    writing: &mut TcpStream,
) -> Result<(), TlsError> {
    loop {
        while conn.wants_write() {
            conn.write_tls(writing)?;
        }
        writing.flush()?;
        if !conn.is_handshaking() {
            return Ok(());
        }
        if !conn.wants_read() {
            // Neither wants to read nor to write, and still handshaking: there
            // is no state in which that is progress.
            return Err(TlsError::Io(io::Error::new(
                io::ErrorKind::InvalidData,
                "the TLS handshake stalled",
            )));
        }
        if conn.read_tls(reading)? == 0 {
            return Err(TlsError::Io(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "the relay closed the connection during the TLS handshake",
            )));
        }
        if let Err(e) = conn.process_new_packets() {
            // rustls has queued an alert saying why. Sending it is what lets
            // the far end log a real reason instead of a reset, and it costs
            // one write on a connection that is over anyway.
            while conn.wants_write() {
                if conn.write_tls(writing).is_err() {
                    break;
                }
            }
            let _ = writing.flush();
            // Only a certificate complaint is called a refusal. Everything
            // else — a record this client could not decode, a version or a
            // cipher with no overlap — is a handshake failure, and calling it
            // a certificate problem would send somebody to their CA store
            // over a server that speaks no TLS at all.
            return Err(match e {
                rustls::Error::InvalidCertificate(_) => TlsError::Refused(e),
                _ => TlsError::Handshake(e),
            });
        }
    }
}

fn poisoned() -> io::Error {
    io::Error::other("the TLS connection's lock is poisoned")
}

fn lock(conn: &Mutex<ClientConnection>) -> io::Result<MutexGuard<'_, ClientConnection>> {
    conn.lock().map_err(|_| poisoned())
}

/// The reading half of a TLS connection.
pub struct TlsReader {
    conn: Arc<Mutex<ClientConnection>>,
    socket: TcpStream,
    /// Bytes taken off the socket that rustls has not consumed yet.
    ///
    /// `read_tls` is allowed to take less than it is offered — its own buffer
    /// is bounded — so the remainder has to be carried to the next pass rather
    /// than dropped.
    raw: Vec<u8>,
    taken: usize,
    seen_eof: bool,
}

impl Read for TlsReader {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if out.is_empty() {
            return Ok(0);
        }
        loop {
            {
                let mut conn = lock(&self.conn)?;

                // Feed what is already in hand. Guarded on being non-empty:
                // `read_tls` over an empty slice returns Ok(0), which rustls
                // reads as end-of-file, and inventing an EOF here would end
                // live sessions.
                while self.taken < self.raw.len() {
                    let n = conn.read_tls(&mut &self.raw[self.taken..])?;
                    self.taken += n;
                    conn.process_new_packets()
                        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
                    if n == 0 {
                        // rustls is full; the rest waits until plaintext has
                        // been drained out of it below.
                        break;
                    }
                }
                if self.taken >= self.raw.len() {
                    self.raw.clear();
                    self.taken = 0;
                }

                match conn.reader().read(out) {
                    // `Ok(0)` here is a clean close: the peer sent
                    // close_notify. A truncated connection is an
                    // `UnexpectedEof` error instead, and it stays an error —
                    // silently treating a cut connection as an orderly end is
                    // how a truncation attack goes unnoticed.
                    Ok(n) => return Ok(n),
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
                    Err(e) => return Err(e),
                }

                if self.seen_eof {
                    return Ok(0);
                }

                // A renegotiation or a keyupdate can leave rustls wanting to
                // write from inside a read. Flushing it here is safe: no
                // other thread can be holding the lock, because the writer
                // only ever takes it briefly and never blocks on this reader.
                while conn.wants_write() {
                    conn.write_tls(&mut self.socket)?;
                }
            }

            // MUTANT: take the lock BEFORE the blocking read, which is the
            // invariant the whole design turns on.
            let held = lock(&self.conn)?;
            let mut buf = vec![0u8; NET_CHUNK];
            let n = self.socket.read(&mut buf)?;
            drop(held);
            if n == 0 {
                self.seen_eof = true;
                // Tell rustls, so that `reader()` can distinguish a clean
                // close from a truncated one on the next pass.
                let mut conn = lock(&self.conn)?;
                conn.read_tls(&mut io::empty())?;
                conn.process_new_packets()
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
            } else {
                buf.truncate(n);
                self.raw = buf;
                self.taken = 0;
            }
        }
    }
}

/// The writing half of a TLS connection.
pub struct TlsWriter {
    conn: Arc<Mutex<ClientConnection>>,
    socket: TcpStream,
}

impl Write for TlsWriter {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        if data.is_empty() {
            return Ok(0);
        }
        let mut conn = lock(&self.conn)?;
        let mut n = conn.writer().write(data)?;
        if n == 0 {
            // rustls' send buffer is full. Draining it to the socket is the
            // only thing that can make room, and returning Ok(0) instead
            // would make a caller's `write_all` fail with `WriteZero`.
            while conn.wants_write() {
                conn.write_tls(&mut self.socket)?;
            }
            n = conn.writer().write(data)?;
        }
        while conn.wants_write() {
            conn.write_tls(&mut self.socket)?;
        }
        if n == 0 {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "the TLS connection accepted no bytes even after being flushed",
            ));
        }
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        let mut conn = lock(&self.conn)?;
        conn.writer().flush()?;
        while conn.wants_write() {
            conn.write_tls(&mut self.socket)?;
        }
        self.socket.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_root_store_is_a_named_refusal_and_never_a_silent_fallback() {
        // The whole argument for using the machine's store rather than a
        // vendored one rests on this: if the store cannot be read, the answer
        // is to stop, not to reach for a bundle compiled in months ago that
        // still trusts whatever an administrator has since removed.
        let Err(e) = Trust::with_roots(RootCertStore::empty()) else {
            panic!("an empty root store built a TLS client");
        };
        assert!(matches!(e, TlsError::NoRoots(_)), "{e:?}");
        let said = e.to_string();
        assert!(said.contains("root certificates"), "{said}");
        // It has to say where it looked. "handshake failed" is what this tree
        // would otherwise have read on the day /etc/pki/nssdb was deleted.
        assert!(said.contains("/etc/pki"), "{said}");
        assert!(said.contains("SSL_CERT_FILE"), "{said}");
    }

    #[test]
    fn a_relay_address_that_is_not_a_verifiable_name_is_refused_before_any_socket() {
        let trust = Trust::with_roots(one_real_root()).expect("build");
        // An underscore is legal in a DNS label and illegal in a certificate's
        // SAN, so there is no certificate that could ever match this. Better
        // refused by name than dialled and mysteriously never verified.
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let socket = TcpStream::connect(("127.0.0.1", port)).expect("connect");
        let Err(e) = trust.connect(socket, "not a host name") else {
            panic!("an unverifiable name was dialled");
        };
        assert!(matches!(e, TlsError::Name(_)), "{e:?}");
        assert!(e.to_string().contains("not a host name"), "{e}");
    }

    /// A root store with one certificate in it, for tests that only need the
    /// store to be non-empty. The bytes are this machine's own store, so the
    /// test says nothing about which CA it is.
    fn one_real_root() -> RootCertStore {
        let found = rustls_native_certs::load_native_certs();
        let mut roots = RootCertStore::empty();
        let (added, _) = roots.add_parsable_certificates(found.certs.into_iter().take(1));
        assert!(added > 0, "this machine's certificate store is empty");
        roots
    }

    #[test]
    fn the_system_store_is_what_this_machine_trusts() {
        // Not an assertion about any particular CA — an assertion that the
        // production path reads the machine and finds something, which is the
        // claim the root-store decision rests on.
        let found = rustls_native_certs::load_native_certs();
        assert!(
            found.errors.is_empty(),
            "reading this machine's certificate store reported errors: {:?}",
            found.errors
        );
        assert!(
            found.certs.len() > 10,
            "this machine's certificate store holds {} certificates",
            found.certs.len()
        );
        Trust::system().expect("the system store did not build a TLS client");
    }
}
