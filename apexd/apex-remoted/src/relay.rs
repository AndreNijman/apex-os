//! Holding a connection open to a relay, and splicing what arrives onto this
//! daemon's own listener.
//!
//! ## The shape, and why it is this shape
//!
//! P1-052 asks for internet access with **no inbound router port
//! forwarding**. A machine that cannot be dialled can only be reached if it
//! has already dialled out, so this thread keeps one outbound WebSocket open
//! to the relay, waiting. When a device arrives the relay joins the two
//! connections and copies bytes between them.
//!
//! What arrives over that WebSocket is then spliced onto
//! `127.0.0.1:<the LAN listener's port>` — a second connection this process
//! makes to itself. That is deliberate and it is the whole design:
//!
//! * `serve::connection` and everything under it keep taking a `TcpStream`.
//!   A relayed session is byte-for-byte the same session a LAN device gets,
//!   including the hello byte, the `Noise_IK` handshake and every frame,
//!   because it *is* that session — this module never looks inside it.
//! * There is therefore no second code path that could drift from the first,
//!   and no chance of a relayed connection being handled by a version of the
//!   session logic that has had a check left out of it.
//!
//! The cost is one extra copy through the loopback interface, which is not
//! measurable next to the Noise sealing that is already happening.
//!
//! ## The relay's vocabulary
//!
//! **Binary frames are opaque.** They are the carried byte stream and this
//! module copies them without inspection, in both directions. A relay that
//! looked inside one would find `Noise` ciphertext.
//!
//! **Text frames are the relay talking about itself** — [`Notice`]. Three
//! words, none of which carry session data. The split matters: it means "the
//! relay cannot read the session" does not depend on the relay being polite,
//! only on the payload being sealed before it gets here.
//!
//! This end never sends a text frame. A relay is not asked anything.
//!
//! ## What labels a session as relayed
//!
//! The splice registers its own loopback source port in [`State`] **before**
//! it writes the first byte to the loopback socket, and `serve::connection`
//! reads the hello byte before it asks. So the lookup cannot run early; the
//! ordering is the guarantee, not a sleep.

use std::collections::HashSet;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use apex_remote_core::relay::{Endpoint, Notice, Op, Opening, Receiver, RelayError, Role, Sender};
use apex_remote_core::tls::{TlsError, Trust};

use crate::state::State;

/// How long to wait before dialling again after the first failure.
pub const BACKOFF_MIN: Duration = Duration::from_secs(2);

/// The longest this will ever wait between attempts.
///
/// A minute rather than an hour: the common failure is a laptop whose Wi-Fi
/// has just changed, and a device that is being reached right now should not
/// have to wait out a backoff that grew while the lid was shut.
pub const BACKOFF_MAX: Duration = Duration::from_secs(60);

/// How often a waiting connection proves it is still there.
///
/// A relay that has been waiting for a phone all day has sent nothing all
/// day, and every NAT and load balancer between here and it will eventually
/// decide the connection is dead. A WebSocket ping is the protocol's own
/// answer to that.
pub const KEEPALIVE: Duration = Duration::from_secs(30);

/// How much of the carried stream is moved per frame.
const CHUNK: usize = 32 * 1024;

/// Which loopback source ports are relay splices right now.
///
/// A set rather than a flag: a desktop serves more than one device at a time,
/// and each splice is its own loopback connection.
#[derive(Default)]
pub struct Sources(Mutex<HashSet<u16>>);

impl Sources {
    pub fn arm(&self, port: u16) {
        if let Ok(mut s) = self.0.lock() {
            s.insert(port);
        }
    }

    pub fn disarm(&self, port: u16) {
        if let Ok(mut s) = self.0.lock() {
            s.remove(&port);
        }
    }

    pub fn holds(&self, port: u16) -> bool {
        self.0.lock().map(|s| s.contains(&port)).unwrap_or(false)
    }
}

/// Why an attempt to reach the relay ended.
#[derive(Debug)]
pub enum DialError {
    /// The address asks for TLS and the TLS leg could not be established —
    /// most importantly because the relay's certificate did not verify.
    Tls(TlsError),
    Io(std::io::Error),
    Relay(RelayError),
}

impl std::fmt::Display for DialError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DialError::Tls(e) => write!(f, "{e}"),
            DialError::Io(e) => write!(f, "{e}"),
            DialError::Relay(e) => write!(f, "{e}"),
        }
    }
}

impl From<std::io::Error> for DialError {
    fn from(e: std::io::Error) -> DialError {
        DialError::Io(e)
    }
}

impl From<RelayError> for DialError {
    fn from(e: RelayError) -> DialError {
        DialError::Relay(e)
    }
}

impl From<TlsError> for DialError {
    fn from(e: TlsError) -> DialError {
        DialError::Tls(e)
    }
}

/// Keep this machine reachable through the relay, for as long as it runs.
///
/// Never returns. One waiting connection at a time: the moment the relay says
/// a device has arrived, the session is handed to its own thread and this
/// loop immediately opens the next waiting connection, so a desktop is
/// reachable by a second device while the first is connected.
pub fn supervise(state: Arc<State>, endpoint: Endpoint) {
    let rendezvous = apex_remote_core::rendezvous::rendezvous_id(&state.identity.public_bytes());

    // The root store is read once, here, and not per dial. It is a directory
    // walk and a few hundred certificate parses, and the loop below can retry
    // every two seconds. Reading it up front also decides the failure mode: a
    // machine whose store is unreadable says so once and stops, instead of
    // logging the same thing every two seconds for as long as it is switched
    // on. Not dialling is the right answer there — the alternative is
    // carrying a remote-control session over a connection nothing verified.
    let trust = if endpoint.secure {
        match Trust::system() {
            Ok(trust) => Some(trust),
            Err(e) => {
                eprintln!("apex-remoted: the relay {endpoint} cannot be verified: {e}");
                return;
            }
        }
    } else {
        None
    };

    eprintln!(
        "apex-remoted: relay {endpoint}, rendezvous {rendezvous} \
         (outbound only; no inbound port is opened)"
    );
    let mut backoff = BACKOFF_MIN;
    loop {
        match wait_for_a_device(&endpoint, &rendezvous, trust.as_ref()) {
            Ok(joined) => {
                backoff = BACKOFF_MIN;
                let state = Arc::clone(&state);
                std::thread::spawn(move || {
                    if let Err(e) = splice(joined, &state) {
                        eprintln!("apex-remoted: a relayed session ended: {e}");
                    }
                });
            }
            Err(e) => {
                eprintln!("apex-remoted: the relay is not reachable ({e}); retrying");
                std::thread::sleep(backoff);
                backoff = (backoff * 2).min(BACKOFF_MAX);
            }
        }
    }
}

/// One joined relay connection: the two halves, and the socket under them.
///
/// The halves are boxed rather than `TcpStream`, because over `wss://` they
/// are the two ends of one `rustls::ClientConnection` and not two descriptors
/// for one socket. `socket` stays a real `TcpStream` either way: shutting it
/// down is what unblocks a reader parked in the receiver, and that has to keep
/// working on the TLS path too — a TLS reader blocked on the network is
/// blocked in a plain `read` on exactly this socket.
pub struct Joined {
    pub receiver: Receiver<Box<dyn Read + Send>>,
    pub sender: Arc<Mutex<Sender<Box<dyn Write + Send>>>>,
    /// Held so the connection can be shut down from another thread.
    pub socket: TcpStream,
}

/// Open the host side of the rendezvous and block until a device arrives.
///
/// A keepalive thread pings while this waits, and stops when the wait ends.
fn wait_for_a_device(
    endpoint: &Endpoint,
    rendezvous: &str,
    trust: Option<&Trust>,
) -> Result<Joined, DialError> {
    let joined = dial(endpoint, rendezvous, Role::Host, trust)?;

    // The keepalive runs only while waiting. Once a device is on the far end
    // the session's own traffic keeps the connection warm, and an extra ping
    // interleaved with it is a frame the splice has to step over.
    let waiting = Arc::new(std::sync::atomic::AtomicBool::new(true));
    {
        let sender = Arc::clone(&joined.sender);
        let waiting = Arc::clone(&waiting);
        std::thread::spawn(move || {
            while waiting.load(std::sync::atomic::Ordering::Relaxed) {
                std::thread::sleep(KEEPALIVE);
                if !waiting.load(std::sync::atomic::Ordering::Relaxed) {
                    return;
                }
                let Ok(mut s) = sender.lock() else { return };
                if s.ping(b"apex").is_err() {
                    return;
                }
            }
        });
    }

    let mut joined = joined;
    let outcome = loop {
        let message = match joined.receiver.message() {
            Ok(m) => m,
            Err(e) => break Err(DialError::Relay(e)),
        };
        match message.op {
            Op::Text => match Notice::parse(&message.payload) {
                Some(Notice::Paired) => break Ok(()),
                // `waiting` is the relay confirming it holds the rendezvous,
                // and an unknown notice is a newer relay talking about
                // something this build does not have. Neither is an event.
                _ => continue,
            },
            Op::Ping => {
                if let Ok(mut s) = joined.sender.lock() {
                    let _ = s.pong(&message.payload);
                }
            }
            Op::Pong => continue,
            Op::Close => {
                break Err(DialError::Relay(RelayError::Upgrade(
                    "it closed the waiting connection".into(),
                )))
            }
            // Binary before a device has been announced means the relay
            // joined this connection to something without saying so, and the
            // bytes would be fed to a splice that has not been set up.
            Op::Binary | Op::Continuation => {
                break Err(DialError::Relay(RelayError::Protocol(
                    "payload before the relay said a device had arrived",
                )))
            }
        }
    };
    waiting.store(false, std::sync::atomic::Ordering::Relaxed);
    outcome.map(|()| joined)
}

/// Dial one connection to the relay and complete the WebSocket handshake.
///
/// `trust` is the root store, and is required for a `wss://` endpoint. It is
/// an argument rather than something read in here so that it is read once per
/// daemon rather than once per reconnect — see [`supervise`].
pub fn dial(
    endpoint: &Endpoint,
    rendezvous: &str,
    role: Role,
    trust: Option<&Trust>,
) -> Result<Joined, DialError> {
    let socket = TcpStream::connect((endpoint.host.as_str(), endpoint.port))?;
    // Nagle off: the carried stream is a terminal, and coalescing a keystroke
    // with whatever comes next is exactly the latency this feature is judged
    // on.
    socket.set_nodelay(true).ok();

    let (mut reading, mut writing): (Box<dyn Read + Send>, Box<dyn Write + Send>) =
        if endpoint.secure {
            // TLS FIRST, and the upgrade request written into it afterwards.
            // The ordering is the point: the rendezvous id lives in that
            // request's path, so a client that wrote it before the handshake
            // would publish to every on-path observer the one value a device
            // pins when it scans a QR code.
            //
            // Arriving here with no root store is a programming error rather
            // than a configuration one — `supervise` refuses to start without
            // one — and it is still refused rather than downgraded, because
            // the cost of being wrong is a session carried in the clear.
            let trust = trust.ok_or_else(|| {
                TlsError::NoRoots("this dial was given no root store to verify against".into())
            })?;
            // The name checked is the relay's, from its URL. Never the address
            // that was resolved and dialled.
            let (r, w) = trust.connect(socket.try_clone()?, &endpoint.host)?;
            (Box::new(r), Box::new(w))
        } else {
            (Box::new(socket.try_clone()?), Box::new(socket.try_clone()?))
        };
    let opening = Opening::new();
    writing.write_all(&opening.request(endpoint, rendezvous, role))?;
    writing.flush()?;
    opening.accept(&mut reading)?;

    Ok(Joined {
        receiver: Receiver::new(reading),
        sender: Arc::new(Mutex::new(Sender::new(writing))),
        socket,
    })
}

/// Copy a joined relay connection onto this daemon's own listener.
fn splice(mut joined: Joined, state: &Arc<State>) -> std::io::Result<()> {
    let local = TcpStream::connect(("127.0.0.1", state.port))?;
    local.set_nodelay(true).ok();
    let source = local.local_addr()?.port();

    // Registered BEFORE a byte can reach the listener. `serve::connection`
    // blocks on the hello byte, and the hello byte is the first thing the
    // loop below forwards, so the label is in place by the time anything
    // asks for it. This ordering is the whole mechanism; a sleep here would
    // be a race with a comfortable-looking margin.
    state.relay_sources.arm(source);

    let to_relay = local.try_clone()?;
    let sender = Arc::clone(&joined.sender);
    let pump = std::thread::spawn(move || {
        let mut from_local = to_relay;
        let mut buf = vec![0u8; CHUNK];
        loop {
            let n = match from_local.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            let Ok(mut s) = sender.lock() else { break };
            if s.binary(&buf[..n]).is_err() {
                break;
            }
        }
        if let Ok(mut s) = sender.lock() {
            let _ = s.close();
        }
    });

    let mut to_local = local.try_clone()?;
    let outcome = loop {
        match joined.receiver.message() {
            Ok(m) => match m.op {
                // The only thing this module does with the session: move it.
                Op::Binary | Op::Continuation => {
                    if to_local.write_all(&m.payload).is_err() {
                        break Ok(());
                    }
                }
                Op::Ping => {
                    if let Ok(mut s) = joined.sender.lock() {
                        let _ = s.pong(&m.payload);
                    }
                }
                Op::Pong => {}
                Op::Text => {
                    if Notice::parse(&m.payload) == Some(Notice::PeerGone) {
                        break Ok(());
                    }
                }
                Op::Close => break Ok(()),
            },
            Err(e) => break Err(std::io::Error::other(e.to_string())),
        }
    };

    // Both directions down, then the label released. Shutting the loopback
    // socket is what unblocks the pump thread, which is sitting in a read.
    let _ = local.shutdown(std::net::Shutdown::Both);
    let _ = joined.socket.shutdown(std::net::Shutdown::Both);
    let _ = pump.join();
    state.relay_sources.disarm(source);
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A listener that speaks no TLS at all, and keeps every byte it was sent.
    ///
    /// Stands in for the worst thing that could be on the far end of a
    /// `wss://` address: something that will happily talk, in the clear, to a
    /// client sloppy enough to send first and encrypt later.
    fn a_plain_listener() -> (u16, Arc<Mutex<Vec<u8>>>) {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind loopback");
        let port = listener.local_addr().expect("addr").port();
        let heard = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&heard);
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut stream = stream;
                let mut buf = [0u8; 4096];
                if let Ok(n) = stream.read(&mut buf) {
                    if let Ok(mut h) = recorded.lock() {
                        h.extend_from_slice(&buf[..n]);
                    }
                }
                // Answer as a web server would, so the client fails on what it
                // reads rather than on a timeout.
                let _ = stream.write_all(b"HTTP/1.1 400 Bad Request\r\n\r\n");
            }
        });
        (port, heard)
    }

    #[test]
    fn a_wss_relay_that_cannot_be_verified_is_refused_and_never_downgraded() {
        // The worst failure available here is not "it did not connect". It is
        // a client that answers a wss:// address by opening a plain socket and
        // sending its upgrade request anyway: the rendezvous id lives in that
        // request's path, so it would be handed to every on-path observer, and
        // the session would then ride an unencrypted link while the
        // configuration said wss://.
        //
        // So the assertion is in two halves — it failed, AND the request never
        // reached the wire.
        let (port, heard) = a_plain_listener();
        let endpoint = Endpoint::parse(&format!("wss://127.0.0.1:{port}")).expect("parse");
        let trust = Trust::system().expect("this machine's root store");

        let Err(e) = dial(&endpoint, "aaaabbbbccccdddd", Role::Host, Some(&trust)) else {
            panic!("a relay that presented no certificate at all was dialled");
        };
        assert!(matches!(e, DialError::Tls(_)), "{e}");

        let said = heard.lock().expect("lock").clone();
        assert!(
            !window_contains(&said, b"GET /r/"),
            "the upgrade request went out in the clear: {:?}",
            String::from_utf8_lossy(&said)
        );
        assert!(
            !window_contains(&said, b"aaaabbbbccccdddd"),
            "the rendezvous id went out in the clear"
        );
        // What it did send is a TLS record: content type 22 (handshake),
        // legacy version 3.1. Proving the refusal was "it would not talk
        // plaintext" and not "it never opened the socket".
        assert!(
            said.starts_with(&[0x16, 0x03]),
            "the first bytes on the wire were not a TLS ClientHello: {:?}",
            &said[..said.len().min(16)]
        );
    }

    #[test]
    fn a_wss_relay_with_no_root_store_is_refused_rather_than_dialled_in_the_clear() {
        // `supervise` will not start without a store, so this is the
        // belt-and-braces half: even called directly, the secure path has no
        // branch that falls through to plaintext.
        let (port, heard) = a_plain_listener();
        let endpoint = Endpoint::parse(&format!("wss://127.0.0.1:{port}")).expect("parse");
        let Err(e) = dial(&endpoint, "aaaabbbbccccdddd", Role::Host, None) else {
            panic!("a wss relay was dialled with no root store");
        };
        assert!(matches!(e, DialError::Tls(_)), "{e}");
        assert!(e.to_string().contains("root certificates"), "{e}");
        assert!(heard.lock().expect("lock").is_empty(), "bytes were sent anyway");
    }

    fn window_contains(haystack: &[u8], needle: &[u8]) -> bool {
        haystack.windows(needle.len()).any(|w| w == needle)
    }

    #[test]
    fn a_source_port_is_only_a_relay_while_its_splice_is_running() {
        let s = Sources::default();
        assert!(!s.holds(4242));
        s.arm(4242);
        assert!(s.holds(4242));
        // A second splice does not disturb the first, and disarming one does
        // not disarm the other: the ports are reused by the kernel and a set
        // that collapsed them would mislabel a later session.
        s.arm(4243);
        s.disarm(4242);
        assert!(!s.holds(4242));
        assert!(s.holds(4243));
    }

    #[test]
    fn the_backoff_grows_and_stops_growing() {
        let mut backoff = BACKOFF_MIN;
        let mut seen = vec![backoff];
        for _ in 0..12 {
            backoff = (backoff * 2).min(BACKOFF_MAX);
            seen.push(backoff);
        }
        assert!(seen.windows(2).all(|w| w[1] >= w[0]), "the backoff went backwards");
        assert_eq!(*seen.last().expect("last"), BACKOFF_MAX);
        // A laptop whose Wi-Fi just came back must not wait out an hour it
        // spent asleep accumulating.
        assert!(BACKOFF_MAX <= Duration::from_secs(60));
    }
}
