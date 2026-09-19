//! The one connection a capsule opens that this daemon does not tunnel
//! (P2-012, route B).
//!
//! `docs/browser-capsule-auth.md` asked the owner whether `apex-agentd` may
//! read the plaintext of a capsule's connection to the one destination that
//! capsule was pinned to, in order to add a credential the capsule is never
//! given. The answer was yes, and this is the half that runs as the user.
//!
//! ```text
//!   capsule ──TLS(minted leaf)──▶ apex-agentd ──plaintext──▶ apex-secretd ──TLS──▶ site
//!                                 (this file)                (adds the header)
//! ```
//!
//! **This daemon never holds the credential.** That is not a leftover of the
//! old design, it is the design: `apex-agentd` runs as the user, so a value it
//! held an unconfined session of that user could read, and P0-002 is stated in
//! `broker.rs` as a property — no verb in `apex_secret_core::protocol` returns
//! a credential. The owner's yes was about plaintext. So what crosses to
//! `apex-secretd` here is the capsule's own request, and what comes back is the
//! site's answer with the value scrubbed out of it by the daemon that holds it.
//!
//! ## What "the one destination" means, exactly
//!
//! Not "a destination the allowlist covers". The session's narrowed allowlist
//! has to be exactly one rule, that rule has to name the host and port the
//! credential is pinned to, and the `CONNECT` target has to equal that host and
//! port literally — [`Intercept::covers`] compares strings and a number, and
//! deliberately does not ask the allowlist, whose rules can be wildcards. Every
//! other `CONNECT` a session makes stays what it was: an opaque tunnel this
//! daemon carries bytes through and cannot read.
//!
//! ## The certificate
//!
//! A CA and one leaf, minted per session with the `openssl` the image already
//! ships, into the session's own scratch directory at `0600`. The CA is
//! installed into the capsule's browser through the same Firefox
//! enterprise-policy bind `--trust-ca` uses ([`crate::browser_ca`]), which is
//! why the two flags are refused together: a capsule with `--present` is pinned
//! to one destination and the daemon terminates that destination itself, so
//! there is no connection left for a caller-supplied root to be about.
//!
//! ALPN is `http/1.1` and nothing else. Firefox offers h2 through a `CONNECT`
//! tunnel, and an h2 stream would defeat the head rewrite on the far side
//! silently — binary frames carried through, no header added, a 401 nobody can
//! explain.

use std::io::{Read, Write};
use std::os::unix::io::AsRawFd;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

use apex_agent_core::destination::Destination;
use apex_secret_core::capability::CapabilityRecord;
use apex_secret_core::client::Client;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::{ServerConfig, ServerConnection};

/// How long one intercepted connection may live.
///
/// A capsule is an automated run with a timeout of its own; a connection open
/// past this is not a page loading.
const CONNECTION_LIFETIME: Duration = Duration::from_secs(300);

/// How long to block in one `poll` before checking the deadline again.
const POLL_INTERVAL_MS: libc::c_int = 1000;

/// How long a write to either side may stall before the connection is dropped.
///
/// It matters on the `apex-secretd` side: that daemon reads one request head
/// and a declared body and then stops reading, so a capsule that kept sending
/// would otherwise block this thread for as long as it liked.
const WRITE_TIMEOUT: Duration = Duration::from_secs(30);

/// Days the minted certificates are valid for.
///
/// One. They exist for the length of one capsule and are deleted with its
/// scratch directory; a longer life would only matter if one escaped, which is
/// the case worth making small.
const VALIDITY_DAYS: &str = "1";

/// What a session was told to authenticate, and everything needed to do it.
pub struct Intercept {
    /// The one destination, literally. See the module note.
    dest: Destination,
    config: Arc<ServerConfig>,
    /// The record sent to `apex-secretd` with every connection. Built once,
    /// at session start, from what the daemon recorded about the session —
    /// never from anything the capsule can reach.
    record: CapabilityRecord,
    /// Where the secret service is, resolved ONCE when the session started.
    ///
    /// A field rather than `paths::socket()` per connection, so that what a
    /// session talks to is decided at the moment the rest of its confinement
    /// is — and so a test can stand a private service up without touching a
    /// process-wide environment variable that other threads are reading.
    secret_socket: PathBuf,
}

/// Where the minted files live, so the caller can bind the CA into the capsule.
pub struct Minted {
    /// The CA certificate, to be installed in the capsule's browser.
    pub ca: PathBuf,
    pub intercept: Intercept,
}

impl Intercept {
    /// Whether this `CONNECT` is the destination the session was pinned to.
    ///
    /// Exact equality on both halves, and not a question for the allowlist: a
    /// narrowed list may hold a wildcard rule, and a rule that *covers* the pin
    /// is not the pin. A session confined to `*.example` with a credential for
    /// `intranet.example` must terminate `intranet.example` and tunnel
    /// everything else — though the start-time guard refuses that session
    /// anyway, which is belt and braces on purpose.
    pub fn covers(&self, dest: &Destination) -> bool {
        dest.host() == self.dest.host() && dest.port() == self.dest.port()
    }

    /// Terminate this connection, hand the plaintext to `apex-secretd`, and
    /// carry its answer back.
    ///
    /// The `200` that opens the tunnel has already been written by the caller,
    /// so what arrives here is the capsule's TLS `ClientHello`.
    pub fn serve(&self, client: UnixStream) -> Result<u64, String> {
        let mut conn = ServerConnection::new(Arc::clone(&self.config))
            .map_err(|e| format!("could not start TLS for {}: {e}", self.dest))?;
        let mut client = client;
        client.set_write_timeout(Some(WRITE_TIMEOUT)).ok();
        // The handshake first and on its own: a capsule that does not trust the
        // minted root fails HERE, which is the failure this daemon can name.
        while conn.is_handshaking() {
            conn.complete_io(&mut client)
                .map_err(|e| format!("the capsule's TLS handshake failed: {e}"))?;
        }

        let peer = Client::connect_at(&self.secret_socket)
            .map_err(|e| {
                format!(
                    "{e:#}\nthe secret service holds this capsule's credential; without it \
                     the connection cannot be authenticated"
                )
            })?
            .present(self.record.clone(), &self.dest.to_string())
            .map_err(|e| format!("{e:#}"))?;
        peer.set_write_timeout(Some(WRITE_TIMEOUT)).ok();

        pump(&mut conn, &mut client, peer)
    }
}

/// Carry plaintext both ways until one side is done.
///
/// Both sockets stay BLOCKING and `poll` decides which to read. The alternative
/// — non-blocking sockets — would mean handling `WouldBlock` on every write as
/// well, and rustls' own `reader()` already answers `WouldBlock` for "no
/// plaintext buffered", so the two meanings would be spelled the same way in
/// one function.
///
/// Deadlock is bounded rather than reasoned away: `apex-secretd` reads one head
/// and a declared body and then stops reading, so a capsule that kept writing
/// would block this thread. Both sockets carry a write timeout for that.
fn pump(conn: &mut ServerConnection, client: &mut UnixStream, mut peer: UnixStream) -> Result<u64, String> {
    let deadline = Instant::now() + CONNECTION_LIFETIME;
    let mut returned: u64 = 0;
    let mut client_eof = false;
    let mut peer_eof = false;
    let mut half_closed = false;
    let mut buf = [0u8; 16 * 1024];

    loop {
        // Plaintext first, and OUTSIDE the readable branch. rustls can consume
        // application data during the handshake — a TLS 1.3 client may send
        // its first request in the same flight as its `Finished` — so bytes
        // can already be buffered before this loop has polled anything. A
        // drain that only ran when the socket was readable would leave that
        // first request sitting in rustls for ever, and the capsule would wait
        // out its timeout for an answer to a request nothing had forwarded.
        // That is not hypothetical: it is what this function did first.
        loop {
            match conn.reader().read(&mut buf) {
                Ok(0) => {
                    client_eof = true;
                    break;
                }
                Ok(n) => peer
                    .write_all(&buf[..n])
                    .map_err(|e| format!("handing the request to the secret service: {e}"))?,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(_) => {
                    client_eof = true;
                    break;
                }
            }
        }
        // The capsule is finished asking. Telling the far side so is what lets
        // a site that reads to end-of-request answer at all.
        if client_eof && !half_closed {
            let _ = peer.shutdown(std::net::Shutdown::Write);
            half_closed = true;
        }

        while conn.wants_write() {
            conn.write_tls(client)
                .map_err(|e| format!("writing to the capsule: {e}"))?;
        }
        if peer_eof {
            break;
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "this connection was open for more than {} seconds and was closed",
                CONNECTION_LIFETIME.as_secs()
            ));
        }

        let mut fds = [
            libc::pollfd {
                fd: client.as_raw_fd(),
                events: if client_eof { 0 } else { libc::POLLIN },
                revents: 0,
            },
            libc::pollfd {
                fd: peer.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        // Safe: the array and its length are this function's, and both fds are
        // owned by locals that outlive the call.
        let ready = unsafe { libc::poll(fds.as_mut_ptr(), 2, POLL_INTERVAL_MS) };
        if ready < 0 {
            let e = std::io::Error::last_os_error();
            if e.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(format!("waiting on the capsule's connection: {e}"));
        }

        if fds[0].revents != 0 && !client_eof {
            match conn.read_tls(client) {
                Ok(0) => client_eof = true,
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(e) => return Err(format!("reading from the capsule: {e}")),
            }
            conn.process_new_packets()
                .map_err(|e| format!("the capsule sent something TLS could not read: {e}"))?;
        }

        if fds[1].revents != 0 {
            match peer.read(&mut buf) {
                Ok(0) => peer_eof = true,
                Ok(n) => {
                    returned += n as u64;
                    conn.writer()
                        .write_all(&buf[..n])
                        .map_err(|e| format!("returning the answer to the capsule: {e}"))?;
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(e) => return Err(format!("reading the secret service's answer: {e}")),
            }
        }
    }

    conn.send_close_notify();
    while conn.wants_write() {
        conn.write_tls(client)
            .map_err(|e| format!("closing the capsule's connection: {e}"))?;
    }
    Ok(returned)
}

/// Mint a CA and one leaf for `dest`, and build the session's [`Intercept`].
///
/// `openssl` rather than a crate, because it is in the image already and
/// certificate minting has no dependency in this tree — the same choice
/// `docs/browser-capsule-auth.md` recorded when it costed this work. Elliptic
/// curve rather than RSA because a 2048-bit key generation is a visible pause
/// on every capsule and P-256 is what `ring` signs with anyway.
pub fn mint(
    scratch: &Path,
    dest: &Destination,
    record: CapabilityRecord,
    secret_socket: PathBuf,
) -> Result<Minted, String> {
    let dir = scratch.join("present");
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    restrict(&dir, 0o700)?;

    let ca_key = dir.join("ca.key");
    let ca_pem = dir.join("ca.pem");
    let leaf_key = dir.join("leaf.key");
    let leaf_csr = dir.join("leaf.csr");
    let leaf_pem = dir.join("leaf.pem");

    openssl(&[
        "req",
        "-x509",
        "-newkey",
        "ec",
        "-pkeyopt",
        "ec_paramgen_curve:P-256",
        "-nodes",
        "-days",
        VALIDITY_DAYS,
        "-subj",
        "/CN=APEX capsule authority",
        "-addext",
        "basicConstraints=critical,CA:TRUE,pathlen:0",
        "-addext",
        "keyUsage=critical,keyCertSign",
        "-keyout",
        &ca_key.to_string_lossy(),
        "-out",
        &ca_pem.to_string_lossy(),
    ])?;
    openssl(&[
        "req",
        "-new",
        "-newkey",
        "ec",
        "-pkeyopt",
        "ec_paramgen_curve:P-256",
        "-nodes",
        "-subj",
        &format!("/CN={}", dest.host()),
        "-keyout",
        &leaf_key.to_string_lossy(),
        "-out",
        &leaf_csr.to_string_lossy(),
    ])?;

    // The SAN is what a browser checks, and a certificate with only a CN is one
    // Firefox refuses outright. An address pin needs `IP:`, a name needs
    // `DNS:`, and getting that wrong is a refusal with no useful message in it.
    let san = if dest.host().parse::<std::net::IpAddr>().is_ok() {
        format!("subjectAltName=IP:{}", dest.host())
    } else {
        format!("subjectAltName=DNS:{}", dest.host())
    };
    let ext = dir.join("leaf.ext");
    std::fs::write(
        &ext,
        format!("{san}\nextendedKeyUsage=serverAuth\nbasicConstraints=critical,CA:FALSE\n"),
    )
    .map_err(|e| format!("{}: {e}", ext.display()))?;
    openssl(&[
        "x509",
        "-req",
        "-days",
        VALIDITY_DAYS,
        "-in",
        &leaf_csr.to_string_lossy(),
        "-CA",
        &ca_pem.to_string_lossy(),
        "-CAkey",
        &ca_key.to_string_lossy(),
        "-set_serial",
        "1",
        "-extfile",
        &ext.to_string_lossy(),
        "-out",
        &leaf_pem.to_string_lossy(),
    ])?;

    for path in [&ca_key, &leaf_key] {
        restrict(path, 0o600)?;
    }
    // The CA's PRIVATE key has done its one job. Deleting it here means a
    // capsule that somehow reached the scratch directory could not sign a
    // second certificate with a root its own browser trusts.
    std::fs::remove_file(&ca_key).map_err(|e| format!("{}: {e}", ca_key.display()))?;

    let config = server_config(&leaf_pem, &leaf_key)?;
    Ok(Minted {
        ca: ca_pem,
        intercept: Intercept {
            dest: dest.clone(),
            config,
            record,
            secret_socket,
        },
    })
}

/// A TLS server configuration for one minted leaf.
pub fn server_config(leaf_pem: &Path, leaf_key: &Path) -> Result<Arc<ServerConfig>, String> {
    let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_file_iter(leaf_pem)
        .map_err(|e| format!("{}: {e}", leaf_pem.display()))?
        .collect::<Result<_, _>>()
        .map_err(|e| format!("{}: {e}", leaf_pem.display()))?;
    if certs.is_empty() {
        return Err(format!("{} holds no certificate", leaf_pem.display()));
    }
    let key = PrivateKeyDer::from_pem_file(leaf_key)
        .map_err(|e| format!("{}: {e}", leaf_key.display()))?;
    // The provider is named rather than left to the crate-features default,
    // which depends on a process-global another crate in the same binary could
    // have set first.
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut config = ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| format!("TLS could not be configured: {e}"))?
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| format!("the minted certificate was not usable: {e}"))?;
    // See the module note: h2 through the tunnel would be carried as frames
    // nothing adds a header to.
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(Arc::new(config))
}

fn openssl(args: &[&str]) -> Result<(), String> {
    let out = Command::new("openssl")
        .args(args)
        .output()
        .map_err(|e| format!("openssl could not be run ({e}); it is what mints a capsule's certificate"))?;
    if !out.status.success() {
        return Err(format!(
            "openssl {} failed: {}",
            args.first().copied().unwrap_or_default(),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(())
}

fn restrict(path: &Path, mode: u32) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
        .map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("apex-intercept-{}-{tag}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).expect("scratch");
        dir
    }

    fn record() -> CapabilityRecord {
        let mut rec = CapabilityRecord::new(
            "intranet",
            apex_secret_core::operation::BROWSER_PRESENT,
            "",
        );
        rec.project = Some("/tmp/capsule".into());
        rec
    }

    #[test]
    fn the_pin_is_compared_exactly_and_not_through_the_allowlist() {
        let dir = scratch("covers");
        let dest = Destination::parse("intranet.example:8443").expect("dest");
        let minted = mint(&dir, &dest, record(), PathBuf::from("/nonexistent")).expect("mint");
        let i = &minted.intercept;
        assert!(i.covers(&Destination::parse("intranet.example:8443").unwrap()));
        // The same host on another port is another endpoint, and the same port
        // on a subdomain is another host. A capsule reaching either of these
        // gets the tunnel it always got.
        assert!(!i.covers(&Destination::parse("intranet.example:443").unwrap()));
        assert!(!i.covers(&Destination::parse("eu.intranet.example:8443").unwrap()));
        assert!(!i.covers(&Destination::parse("example:8443").unwrap()));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_ca_private_key_does_not_outlive_the_minting() {
        // A capsule that reached the scratch directory with the CA key in it
        // could sign a certificate for anything, for a root its own browser
        // trusts. The key has one job and it is done before the session starts.
        let dir = scratch("cakey");
        let dest = Destination::parse("intranet.example:443").expect("dest");
        let minted = mint(&dir, &dest, record(), PathBuf::from("/nonexistent")).expect("mint");
        assert!(minted.ca.exists(), "the CA certificate is what the capsule installs");
        assert!(
            !dir.join("present/ca.key").exists(),
            "the CA private key is still in the session's scratch directory"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The minted leaf is what a client sees, it names the pinned host, and it
    /// chains to the per-run CA and to nothing the machine already trusts.
    #[test]
    fn a_client_that_trusts_the_minted_ca_completes_a_handshake_and_one_that_does_not_fails() {
        let dir = scratch("handshake");
        let dest = Destination::parse("intranet.example:443").expect("dest");
        let minted = mint(&dir, &dest, record(), PathBuf::from("/nonexistent")).expect("mint");
        let config = Arc::clone(&minted.intercept.config);

        let (server_side, client_side) = UnixStream::pair().expect("socketpair");
        std::thread::spawn(move || {
            let mut conn = ServerConnection::new(config).expect("server");
            let mut sock = server_side;
            while conn.is_handshaking() {
                if conn.complete_io(&mut sock).is_err() {
                    return;
                }
            }
            let _ = conn.writer().write_all(b"hello");
            while conn.wants_write() {
                if conn.write_tls(&mut sock).is_err() {
                    return;
                }
            }
        });

        let mut roots = rustls::RootCertStore::empty();
        for cert in CertificateDer::pem_file_iter(&minted.ca).expect("ca") {
            roots.add(cert.expect("cert")).expect("add");
        }
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let mut client = rustls::ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .expect("versions")
            .with_root_certificates(roots)
            .with_no_client_auth();
        // What Firefox offers through a tunnel. The server must pick http/1.1,
        // or the head rewrite on the far side is carrying h2 frames.
        client.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
        let name = rustls::pki_types::ServerName::try_from("intranet.example").expect("name");
        let mut conn =
            rustls::ClientConnection::new(Arc::new(client), name).expect("client connection");
        let mut sock = client_side;
        while conn.is_handshaking() {
            conn.complete_io(&mut sock).expect("handshake");
        }
        assert_eq!(
            conn.alpn_protocol(),
            Some(&b"http/1.1"[..]),
            "the capsule negotiated something other than HTTP/1.1"
        );
        // Read until the five bytes are there. `complete_io` returns as soon
        // as it has moved what was available, so a single call is a race that
        // passes on a fast machine and reports `WouldBlock` on a loaded one.
        let mut got = Vec::new();
        for _ in 0..200 {
            conn.complete_io(&mut sock).expect("io");
            let mut chunk = [0u8; 16];
            match conn.reader().read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => got.extend_from_slice(&chunk[..n]),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(e) => panic!("plaintext: {e}"),
            }
            if got.len() >= 5 {
                break;
            }
        }
        assert_eq!(&got, b"hello");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// The control the test above needs: without the per-run CA the same leaf
    /// is refused, so "the handshake completed" means the CA and not merely
    /// that something answered.
    #[test]
    fn the_same_leaf_is_refused_by_a_client_that_was_not_given_the_per_run_ca() {
        let dir = scratch("nocontrol");
        let dest = Destination::parse("intranet.example:443").expect("dest");
        let minted = mint(&dir, &dest, record(), PathBuf::from("/nonexistent")).expect("mint");
        let config = Arc::clone(&minted.intercept.config);

        let (server_side, client_side) = UnixStream::pair().expect("socketpair");
        std::thread::spawn(move || {
            let mut conn = ServerConnection::new(config).expect("server");
            let mut sock = server_side;
            while conn.is_handshaking() {
                if conn.complete_io(&mut sock).is_err() {
                    return;
                }
            }
        });

        // The machine's own roots, which is what every other program in the
        // capsule keeps using.
        let found = rustls_native_certs::load_native_certs();
        let mut roots = rustls::RootCertStore::empty();
        let (added, _) = roots.add_parsable_certificates(found.certs);
        if added == 0 {
            eprintln!("SKIP: this machine has no certificate authorities installed");
            std::fs::remove_dir_all(&dir).ok();
            return;
        }
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let client = rustls::ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .expect("versions")
            .with_root_certificates(roots)
            .with_no_client_auth();
        let name = rustls::pki_types::ServerName::try_from("intranet.example").expect("name");
        let mut conn =
            rustls::ClientConnection::new(Arc::new(client), name).expect("client connection");
        let mut sock = client_side;
        let mut refused = None;
        while conn.is_handshaking() {
            match conn.complete_io(&mut sock) {
                Ok(_) => {}
                Err(e) => {
                    refused = Some(e.to_string());
                    break;
                }
            }
        }
        let refused = refused.expect("a leaf signed by a CA this client does not hold was accepted");
        assert!(
            refused.to_lowercase().contains("certificate"),
            "refused for something other than the certificate: {refused}"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// An address pin gets an `IP:` SAN and a name pin a `DNS:` one. A
    /// certificate with the wrong kind is refused by every client, with a
    /// message about the name that says nothing about the reason.
    #[test]
    fn an_address_pin_is_checked_as_an_address() {
        let dir = scratch("ip");
        let dest = Destination::parse("127.0.0.1:9443").expect("dest");
        let minted = mint(&dir, &dest, record(), PathBuf::from("/nonexistent")).expect("mint");
        let text = String::from_utf8_lossy(
            &Command::new("openssl")
                .args(["x509", "-noout", "-text", "-in"])
                .arg(dir.join("present/leaf.pem"))
                .output()
                .expect("openssl")
                .stdout,
        )
        .into_owned();
        assert!(text.contains("IP Address:127.0.0.1"), "{text}");
        assert!(!text.contains("DNS:"), "{text}");
        let _ = minted;
        std::fs::remove_dir_all(&dir).ok();
    }
}
