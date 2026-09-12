//! What the TLS client *refuses*, proved against certificates this suite mints.
//!
//! ## Why the refusals are the test and "it connected" is not
//!
//! A TLS client that accepts every certificate presented to it also connects,
//! also completes a handshake, and also carries bytes. Every happy-path
//! assertion passes against it. So a suite that only proved a `wss://` dial
//! worked would prove nothing about whether the relay leg is actually
//! authenticated, and the failure it missed is the whole failure: an on-path
//! observer who can answer for `*.workers.dev` reads the rendezvous id and
//! sees every byte of traffic shape.
//!
//! What is asserted here is therefore the refusals, and each one by name:
//!
//! * a certificate signed by a CA the client does not trust — `UnknownIssuer`
//! * a certificate for a different host — `NotValidForName`
//! * a certificate whose validity has passed — `Expired`
//!
//! and, on the acceptance side, that a certificate which *does* verify carries
//! a byte stream in both directions at once, because the reader and writer
//! share one `rustls::ClientConnection` behind a mutex and getting that wrong
//! is a deadlock rather than a wrong answer.
//!
//! ## Against what
//!
//! A CA minted here with the `openssl` CLI, and a `rustls` listener on
//! loopback. Nothing reaches the network, no relay is dialled, and no
//! Cloudflare account is touched. This is the move
//! `tests/test-apex-trust-enforcement.sh` already makes — it proves the trust
//! gate against a CA it mints rather than against production sigstore — and
//! the move P1-005 makes for the Cloudflare capability sets, which run
//! entirely against a loopback double.
//!
//! The minting is left to `openssl` on purpose: it is an implementation
//! independent of the one under test, so a certificate this suite calls
//! expired is expired by somebody else's definition.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use apex_remote_core::tls::{TlsError, Trust};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::{CertificateError, ServerConfig, ServerConnection, StreamOwned};

/// The name every certificate below is for, except the one that is not.
const NAME: &str = "relay.test";

// ─────────────────────────────────────────────────────────────────────────────
//  A certificate authority, minted per test run
// ─────────────────────────────────────────────────────────────────────────────

/// A scratch directory that removes itself.
struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Scratch {
        let dir = std::env::temp_dir().join(format!(
            "apex-relay-tls-{}-{tag}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).expect("scratch dir");
        Scratch(dir)
    }

    fn at(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn openssl(args: &[&str]) {
    let out = Command::new("openssl")
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("openssl {args:?}: {e}"));
    assert!(
        out.status.success(),
        "openssl {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn key_at(path: &Path) {
    openssl(&[
        "genpkey",
        "-algorithm",
        "EC",
        "-pkeyopt",
        "ec_paramgen_curve:P-256",
        "-out",
        &path.display().to_string(),
    ]);
}

/// A self-signed CA: a key, and a certificate that is allowed to sign others.
fn mint_ca(scratch: &Scratch, stem: &str, cn: &str) -> (PathBuf, PathBuf) {
    let key = scratch.at(&format!("{stem}.key"));
    let cert = scratch.at(&format!("{stem}.pem"));
    key_at(&key);
    openssl(&[
        "req",
        "-x509",
        "-new",
        "-key",
        &key.display().to_string(),
        "-sha256",
        "-days",
        "3650",
        "-subj",
        &format!("/CN={cn}"),
        // Explicit rather than relying on what `req -x509` adds by default:
        // a "CA" without basicConstraints CA:TRUE cannot sign, and the test
        // would fail somewhere far from the cause.
        "-addext",
        "basicConstraints=critical,CA:TRUE",
        "-addext",
        "keyUsage=critical,keyCertSign,cRLSign",
        "-out",
        &cert.display().to_string(),
    ]);
    (key, cert)
}

/// A server certificate signed by the given CA.
///
/// `validity` is `None` for "valid from now", or an explicit
/// (not_before, not_after) pair for a certificate that is deliberately out of
/// date.
fn mint_leaf(
    scratch: &Scratch,
    stem: &str,
    san: &str,
    ca: &(PathBuf, PathBuf),
    validity: Option<(&str, &str)>,
) -> (PathBuf, PathBuf) {
    let key = scratch.at(&format!("{stem}.key"));
    let csr = scratch.at(&format!("{stem}.csr"));
    let cert = scratch.at(&format!("{stem}.pem"));
    let ext = scratch.at(&format!("{stem}.ext"));
    key_at(&key);

    // The name has to be in a subjectAltName. rustls-webpki ignores the
    // subject common name entirely — it has been deprecated for this purpose
    // since RFC 2818 — so a certificate with only a CN is a certificate for
    // no host at all.
    std::fs::write(
        &ext,
        format!(
            "subjectAltName = DNS:{san}\n\
             basicConstraints = critical,CA:FALSE\n\
             keyUsage = critical,digitalSignature,keyEncipherment\n\
             extendedKeyUsage = serverAuth\n"
        ),
    )
    .expect("write ext");

    openssl(&[
        "req",
        "-new",
        "-key",
        &key.display().to_string(),
        "-subj",
        &format!("/CN={san}"),
        "-out",
        &csr.display().to_string(),
    ]);

    match validity {
        // `openssl x509 -req` only learned `-not_before`/`-not_after` in
        // OpenSSL 3.5. This laptop has 3.5.7 and the CI runner does not, and
        // the flags are not rejected in a way anyone would read as a version
        // problem: x509 prints "Use -help for summary" and exits non-zero. So
        // this case passed here and failed there, and because the image build
        // needs the rust job green, it took the whole image down with it.
        //
        // `openssl ca` has taken `-startdate`/`-enddate` for decades. It wants
        // a config file, a certificate database and a serial file, which is
        // three more scratch files and no dependency on the openssl version.
        Some((from, to)) => {
            let db = scratch.at(&format!("{stem}.index"));
            let serial = scratch.at(&format!("{stem}.serial"));
            let cfg = scratch.at(&format!("{stem}.ca.cnf"));
            std::fs::write(&db, "").expect("write ca database");
            std::fs::write(&serial, "01\n").expect("write ca serial");
            std::fs::write(
                &cfg,
                format!(
                    "[ca]\n\
                     default_ca = t\n\
                     [t]\n\
                     database = {db}\n\
                     serial = {serial}\n\
                     new_certs_dir = {dir}\n\
                     certificate = {ca_cert}\n\
                     private_key = {ca_key}\n\
                     default_md = sha256\n\
                     policy = pol\n\
                     email_in_dn = no\n\
                     unique_subject = no\n\
                     [pol]\n\
                     commonName = supplied\n",
                    db = db.display(),
                    serial = serial.display(),
                    dir = scratch.0.display(),
                    ca_cert = ca.1.display(),
                    ca_key = ca.0.display(),
                ),
            )
            .expect("write ca config");
            openssl(&[
                "ca",
                "-batch",
                "-notext",
                "-config",
                &cfg.display().to_string(),
                "-in",
                &csr.display().to_string(),
                "-out",
                &cert.display().to_string(),
                "-extfile",
                &ext.display().to_string(),
                "-startdate",
                from,
                "-enddate",
                to,
            ]);
        }
        None => {
            openssl(&[
                "x509",
                "-req",
                "-in",
                &csr.display().to_string(),
                "-CA",
                &ca.1.display().to_string(),
                "-CAkey",
                &ca.0.display().to_string(),
                "-CAcreateserial",
                "-sha256",
                "-extfile",
                &ext.display().to_string(),
                "-out",
                &cert.display().to_string(),
                "-days",
                "365",
            ]);
        }
    }
    (key, cert)
}

// ─────────────────────────────────────────────────────────────────────────────
//  A TLS listener on loopback
// ─────────────────────────────────────────────────────────────────────────────

/// A server that presents one certificate and echoes whatever it is sent.
///
/// It is bound before the test dials, so there is no port race and no child
/// process whose lifetime has to be managed.
struct Listener {
    port: u16,
}

impl Listener {
    fn serving(cert: &Path, key: &Path) -> Listener {
        let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_file_iter(cert)
            .expect("read cert")
            .collect::<Result<_, _>>()
            .expect("parse cert");
        let key = PrivateKeyDer::from_pem_file(key).expect("read key");
        let config = ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .expect("versions")
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .expect("server config");
        let config = Arc::new(config);

        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind loopback");
        let port = listener.local_addr().expect("addr").port();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let config = Arc::clone(&config);
                std::thread::spawn(move || {
                    // Every failure here is the client refusing, which is what
                    // three of the four tests are about. The server's job is
                    // to present the certificate and say nothing about it.
                    let Ok(conn) = ServerConnection::new(config) else { return };
                    let mut tls = StreamOwned::new(conn, stream);
                    let mut buf = [0u8; 4096];
                    while let Ok(n) = tls.read(&mut buf) {
                        if n == 0 || tls.write_all(&buf[..n]).is_err() {
                            break;
                        }
                        let _ = tls.flush();
                    }
                });
            }
        });
        Listener { port }
    }

    fn dial(&self) -> TcpStream {
        let socket = TcpStream::connect(("127.0.0.1", self.port)).expect("connect loopback");
        // So that a client which somehow never completes cannot hang the
        // suite: a refusal must be a refusal, not a timeout.
        socket
            .set_read_timeout(Some(Duration::from_secs(10)))
            .expect("read timeout");
        socket
    }
}

/// The shape every test below shares: mint, serve, dial, and demand `NAME`.
fn attempt(
    scratch: &Scratch,
    trusted_ca: &Path,
    leaf: (&Path, &Path),
    ask_for: &str,
) -> Result<(apex_remote_core::tls::TlsReader, apex_remote_core::tls::TlsWriter), TlsError> {
    let _ = scratch;
    let server = Listener::serving(leaf.1, leaf.0);
    let trust = Trust::from_pem_file(trusted_ca).expect("trust the minted CA");
    trust.connect(server.dial(), ask_for)
}

// ─────────────────────────────────────────────────────────────────────────────
//  The refusals
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn a_certificate_from_an_untrusted_ca_is_refused_and_the_error_says_so() {
    let scratch = Scratch::new("issuer");
    let ca = mint_ca(&scratch, "ca", "apex relay test CA");
    let rogue = mint_ca(&scratch, "rogue-ca", "somebody else entirely");
    // The right name, the right dates, a perfectly well-formed certificate.
    // The only thing wrong with it is who signed it — which is the only thing
    // that was ever protecting the connection.
    let leaf = mint_leaf(&scratch, "rogue-leaf", NAME, &rogue, None);

    let outcome = attempt(&scratch, &ca.1, (&leaf.0, &leaf.1), NAME);
    let Err(e) = outcome else {
        panic!("a certificate signed by an untrusted CA was accepted");
    };
    assert_eq!(
        e.certificate(),
        Some(&CertificateError::UnknownIssuer),
        "refused, but not for the reason the test is about: {e}"
    );
    // "Swallowed" is the failure mode this guards: an error that reached the
    // caller as a bare IO failure would be indistinguishable from the relay
    // being offline, and an operator would go looking at their network.
    let said = e.to_string();
    assert!(said.contains("certificate was refused"), "{said}");
    assert!(said.contains("UnknownIssuer"), "{said}");
}

#[test]
fn a_certificate_for_another_host_is_refused_and_the_error_says_so() {
    let scratch = Scratch::new("name");
    let ca = mint_ca(&scratch, "ca", "apex relay test CA");
    // Signed by the CA the client trusts, in date, and for the wrong machine.
    // Without this check, anyone who can get *any* certificate from a CA in
    // the store can answer for the relay.
    let leaf = mint_leaf(&scratch, "other-leaf", "other.test", &ca, None);

    let outcome = attempt(&scratch, &ca.1, (&leaf.0, &leaf.1), NAME);
    let Err(e) = outcome else {
        panic!("a certificate for other.test was accepted for relay.test");
    };
    let complaint = e.certificate().unwrap_or_else(|| panic!("not a certificate refusal: {e}"));
    assert!(
        matches!(
            complaint,
            CertificateError::NotValidForName | CertificateError::NotValidForNameContext { .. }
        ),
        "refused, but not for the name: {complaint:?}"
    );
    // The message has to carry both names. "Swallowed" is the failure this
    // guards against, and an error that said only "refused" would send an
    // operator looking at their network instead of at their DNS.
    let said = e.to_string();
    assert!(said.contains("certificate was refused"), "{said}");
    assert!(said.contains("not valid for name"), "{said}");
    assert!(said.contains(NAME), "{said}");
    assert!(said.contains("other.test"), "{said}");
}

#[test]
fn an_expired_certificate_is_refused_and_the_error_says_so() {
    let scratch = Scratch::new("expiry");
    let ca = mint_ca(&scratch, "ca", "apex relay test CA");
    // Right CA, right name, and its validity ended years ago. Dates are the
    // only thing that makes a revoked-and-reissued key stop working, so a
    // client that ignored them would keep trusting a certificate whose
    // private key had been rotated away from.
    let leaf = mint_leaf(
        &scratch,
        "old-leaf",
        NAME,
        &ca,
        Some(("20200101000000Z", "20200102000000Z")),
    );

    let outcome = attempt(&scratch, &ca.1, (&leaf.0, &leaf.1), NAME);
    let Err(e) = outcome else {
        panic!("a certificate that expired in 2020 was accepted");
    };
    let complaint = e.certificate().unwrap_or_else(|| panic!("not a certificate refusal: {e}"));
    assert!(
        matches!(
            complaint,
            CertificateError::Expired | CertificateError::ExpiredContext { .. }
        ),
        "refused, but not for the dates: {complaint:?}"
    );
    let said = e.to_string();
    assert!(said.contains("certificate was refused"), "{said}");
    assert!(said.contains("certificate expired"), "{said}");
    assert!(said.contains("not valid after"), "{said}");
}

// ─────────────────────────────────────────────────────────────────────────────
//  The acceptance, and the thing that makes the acceptance hard
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn a_certificate_that_verifies_carries_a_stream_in_both_directions_at_once() {
    let scratch = Scratch::new("good");
    let ca = mint_ca(&scratch, "ca", "apex relay test CA");
    let leaf = mint_leaf(&scratch, "leaf", NAME, &ca, None);

    let (mut reader, mut writer) =
        attempt(&scratch, &ca.1, (&leaf.0, &leaf.1), NAME).expect("a valid certificate");

    // The point of this test is not that a byte survives a round trip; it is
    // that a WRITE happens while a READ is parked. `apex-remoted` does exactly
    // that — a keepalive ping goes out while the receiver waits for the relay
    // to announce a device — and the two halves share one
    // `rustls::ClientConnection` behind a mutex. If the reader held that lock
    // while blocked on the socket, this would deadlock rather than fail, so
    // the result is collected through a channel and a timeout turns a hang
    // into a red test instead of a suite that never finishes.
    let (done, heard) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut got = Vec::new();
        let mut buf = [0u8; 64];
        while got.len() < 12 {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => got.extend_from_slice(&buf[..n]),
                Err(e) => {
                    let _ = done.send(Err(format!("{e}")));
                    return;
                }
            }
        }
        let _ = done.send(Ok(got));
    });

    // Written in pieces, from this thread, while that one is inside a read.
    for part in ["apex", "-rel", "ay-tls"] {
        writer.write_all(part.as_bytes()).expect("write");
        writer.flush().expect("flush");
        std::thread::sleep(Duration::from_millis(20));
    }

    let got = heard
        .recv_timeout(Duration::from_secs(20))
        .expect("the reader never answered — the two halves deadlocked on the TLS connection")
        .expect("read failed");
    assert_eq!(
        String::from_utf8_lossy(&got),
        "apex-relay-tls",
        "the stream did not survive the TLS leg intact"
    );
}

#[test]
fn the_name_checked_is_the_relays_name_and_not_the_address_that_was_dialled() {
    // Every test above connects to 127.0.0.1 and demands a certificate for
    // relay.test, which is the property this pins deliberately rather than
    // incidentally: a client that checked the certificate against the address
    // it dialled would be useless behind any CDN, and one that checked it
    // against whatever the certificate happened to say would be checking
    // nothing at all.
    let scratch = Scratch::new("sni");
    let ca = mint_ca(&scratch, "ca", "apex relay test CA");
    let leaf = mint_leaf(&scratch, "leaf", NAME, &ca, None);
    let server = Listener::serving(&leaf.1, &leaf.0);
    let trust = Trust::from_pem_file(&ca.1).expect("trust");

    // The same socket-level destination, twice, with different demands.
    trust
        .connect(server.dial(), NAME)
        .map(|_| ())
        .expect("relay.test is what the certificate says");
    let Err(e) = trust.connect(server.dial(), "127.0.0.1") else {
        panic!("the address dialled was accepted as the name to verify");
    };
    assert!(e.certificate().is_some(), "{e}");
}
