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
//!
//! ## Keeping the waiting connection honest
//!
//! A phone can only be joined to a waiting connection that is really there,
//! and before this nothing checked. The keepalive pinged every 30 s and the
//! pongs were thrown away; there was no read deadline. So a waiting socket
//! the network had quietly dropped — a NAT that forgot the mapping, a Wi-Fi
//! change with no FIN — sat here looking parked until the relay closed its
//! end, and every phone that arrived meanwhile was joined to nothing or told
//! 409. Now a waiting connection that hears NOTHING for [`WAIT_DEADLINE`],
//! pongs included, is dropped and dialled again.
//!
//! **The deadline is armed only once the relay has shown it answers pings.**
//! The Worker cannot be run from this machine and its hibernation API is the
//! part of it this code knows least about; a relay that did not pong would,
//! under a hard deadline, be re-dialled every 75 s for ever, with a 409
//! window each time. So the first pong — on this connection or any earlier
//! one this process made — is what turns the deadline on. Against a relay
//! that never pongs this behaves exactly as it did before, and says so once.
//!
//! **Re-dialling is immediate after a connection that was healthy**, and
//! backs off only for dials that fail and connections that die young. The
//! Worker drops a long-lived waiting socket every twenty minutes to an hour
//! (the journal: "the relay refused the connection: failed to fill whole
//! buffer" — which was never a refusal, it was the WAIT ending), and each of
//! those used to be followed by a two-second sleep during which every phone
//! was told 409.
//!
//! **A second waiting connection cannot be held ahead of time.** The obvious
//! way to close the 409 window after a device is joined — dial the next
//! waiting socket before the current one is consumed — is refused by the
//! relay by design: `room.js`'s `decide` answers a second host with 409, so a
//! room holds one waiting desktop and no spare. What this can do, it does: the
//! next dial starts the moment a device is announced, on this thread, while
//! the splice runs on its own.

use std::collections::HashSet;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rime_remote_core::relay::{Endpoint, Notice, Op, Opening, Receiver, RelayError, Role, Sender};
use rime_remote_core::tls::{TlsError, Trust};

use crate::state::State;

/// How long to wait before dialling again after the first failure.
///
/// Also the fixed pause after a `409`, which is not a failure (see
/// [`next_delay`]) but is still a relay saying "not yet". Two seconds keeps a
/// desktop that is waiting its turn at 30 upgrades a minute, half of the
/// Worker's per-address limit (`UPGRADES_PER_MINUTE` in `room.js`) — and a
/// phone on the same Wi-Fi shares that address.
pub const BACKOFF_MIN: Duration = Duration::from_secs(2);

/// The longest this will ever wait between attempts.
///
/// Ten seconds, down from a minute. The common failure is a laptop whose
/// Wi-Fi has just changed, and the cost of this number is paid by a phone
/// that is trying to reach the machine RIGHT NOW: with a minute, a desktop
/// that had failed five times in a row was unreachable for up to a minute
/// after its network came back. Ten seconds of retrying is six upgrades a
/// minute, far under the relay's limit.
pub const BACKOFF_MAX: Duration = Duration::from_secs(10);

/// How often a waiting connection proves it is still there.
///
/// A relay that has been waiting for a phone all day has sent nothing all
/// day, and every NAT and load balancer between here and it will eventually
/// decide the connection is dead. A WebSocket ping is the protocol's own
/// answer to that.
pub const KEEPALIVE: Duration = Duration::from_secs(30);

/// How long a waiting connection may hear nothing before it is dialled again.
///
/// Two and a half keepalives. The relay answers each ping, so a healthy
/// waiting connection hears something every 30 s; 75 s of silence is two
/// pings unanswered and the third due, and a connection that has lost two
/// round trips in a row is not one a phone should be joined to.
pub const WAIT_DEADLINE: Duration = Duration::from_secs(75);

/// A waiting connection that lived this long before it ended was healthy.
///
/// Its end is the relay's routine, not a failure, and the next dial is made
/// at once with the backoff reset. Anything shorter is a relay that accepts
/// and drops, and is backed off like a failed dial — otherwise a relay that
/// did that would be dialled in a tight loop.
pub const HEALTHY: Duration = Duration::from_secs(30);

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

/// Why a dial did not produce a waiting connection, by the step that failed.
///
/// By step because the journal used to say "the relay refused the
/// connection: failed to fill whole buffer" for all of them — and for the end
/// of a wait that had been going for an hour, which is not a dial failure at
/// all (that is [`WaitEnd`] now). An owner reading "TCP" knows to look at
/// the network, "TLS" at the clock or the CA store, and "upgrade" at the
/// relay's address or the relay itself.
#[derive(Debug)]
pub enum DialError {
    /// Resolving the relay's name or opening the TCP connection.
    Tcp(std::io::Error),
    /// The address asks for TLS and the TLS leg could not be established —
    /// most importantly because the relay's certificate did not verify.
    Tls(TlsError),
    /// The HTTP upgrade: the request could not be sent, or the answer to it
    /// was not a switch to WebSocket. A relay's `409` lands here.
    Upgrade(RelayError),
    /// A local socket operation between the steps (duplicating the socket).
    Io(std::io::Error),
}

impl std::fmt::Display for DialError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DialError::Tcp(e) => write!(f, "TCP: {e}"),
            DialError::Tls(e) => write!(f, "TLS: {e}"),
            DialError::Upgrade(e) => write!(f, "WebSocket upgrade: {e}"),
            DialError::Io(e) => write!(f, "{e}"),
        }
    }
}

impl From<std::io::Error> for DialError {
    fn from(e: std::io::Error) -> DialError {
        DialError::Io(e)
    }
}

impl From<TlsError> for DialError {
    fn from(e: TlsError) -> DialError {
        DialError::Tls(e)
    }
}

impl DialError {
    /// The HTTP status the relay refused the upgrade with, if that is what
    /// happened.
    ///
    /// Read out of the refusal's text, which `Opening::check` builds from the
    /// relay's status line (`expected HTTP 101, got "HTTP/1.1 409 Conflict"`).
    /// A parse of another crate's sentence is fragile, so the test that pins
    /// it builds the error with `Opening::check` itself: a rewording there
    /// fails here rather than silently turning every 409 into a failure.
    pub fn refused_with(&self) -> Option<u16> {
        let DialError::Upgrade(RelayError::Upgrade(why)) = self else {
            return None;
        };
        let (_, status) = why.split_once("got \"")?;
        status.split_whitespace().nth(1)?.parse().ok()
    }
}

/// How a waiting connection ended after the upgrade had succeeded.
#[derive(Debug)]
pub enum WaitEnd {
    /// Nothing at all arrived for this long, keepalive pongs included.
    Silent(Duration),
    /// The relay, or something between here and it, ended the connection.
    Closed(String),
    /// The relay sent something the protocol does not allow.
    Broke(RelayError),
}

impl std::fmt::Display for WaitEnd {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WaitEnd::Silent(d) => {
                write!(f, "nothing arrived for {} s despite keepalive pings", d.as_secs())
            }
            WaitEnd::Closed(why) => write!(f, "{why}"),
            WaitEnd::Broke(e) => write!(f, "{e}"),
        }
    }
}

/// The clocks a waiting connection runs on. A value rather than constants so
/// a test can run a wait in milliseconds; the daemon always uses
/// [`Timing::SHIPPED`].
#[derive(Debug, Clone, Copy)]
pub struct Timing {
    pub keepalive: Duration,
    pub deadline: Duration,
}

impl Timing {
    pub const SHIPPED: Timing = Timing {
        keepalive: KEEPALIVE,
        deadline: WAIT_DEADLINE,
    };
}

/// One attempt at holding the rendezvous, from the dial to the end of the
/// wait.
pub enum Attempt {
    /// A device arrived. The connection is the device's now.
    Paired(Joined),
    /// The dial failed at one of its steps.
    Failed(DialError),
    /// The relay answered the upgrade with 409: something already holds this
    /// rendezvous as its host. Usually this machine's OWN previous waiting
    /// connection, dropped here and not yet noticed there.
    Held,
    /// The waiting connection was established and later ended.
    Ended { lasted: Duration, why: WaitEnd },
}

/// Keep this machine reachable through the relay, for as long as it runs.
///
/// Never returns. One waiting connection at a time: the moment the relay says
/// a device has arrived, the session is handed to its own thread and this
/// loop immediately opens the next waiting connection, so a desktop is
/// reachable by a second device while the first is connected.
pub fn supervise(state: Arc<State>, endpoint: Endpoint) {
    let rendezvous = rime_remote_core::rendezvous::rendezvous_id(&state.identity.public_bytes());

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
                eprintln!("rime-remoted: the relay {endpoint} cannot be verified: {e}");
                return;
            }
        }
    } else {
        None
    };

    eprintln!(
        "rime-remoted: relay {endpoint}, rendezvous {rendezvous} \
         (outbound only; no inbound port is opened)"
    );
    // Whether this relay has ever answered a keepalive. See the module note:
    // it is what arms the read deadline.
    let answers_pings = AtomicBool::new(false);
    let mut backoff = BACKOFF_MIN;
    let mut log = Log::default();
    loop {
        let attempt = attempt(
            &endpoint,
            &rendezvous,
            trust.as_ref(),
            Timing::SHIPPED,
            &answers_pings,
            &mut log,
        );
        let delay = next_delay(&attempt, &mut backoff);
        log.after(&attempt, delay, &answers_pings);
        if let Attempt::Paired(joined) = attempt {
            let state = Arc::clone(&state);
            std::thread::spawn(move || {
                if let Err(e) = splice(joined, &state) {
                    eprintln!("rime-remoted: a relayed session ended: {e}");
                }
            });
        }
        if !delay.is_zero() {
            std::thread::sleep(delay);
        }
    }
}

/// How long to wait before the next dial, given how the last attempt went.
///
/// A pure function of the outcome and the running backoff, so the rules are
/// asserted directly rather than by timing a loop:
///
/// * **Paired**, or a waiting connection that lived [`HEALTHY`] or longer —
///   including one dropped for silence, which by construction had lived
///   [`WAIT_DEADLINE`] — re-dial NOW and reset the backoff. Neither says
///   anything is wrong with reaching the relay.
/// * **409** is not a failure and does not grow the backoff: the relay is
///   reachable and answering. It is retried after [`BACKOFF_MIN`], not
///   instantly, because each retry is an upgrade against the relay's
///   per-address limit.
/// * A dial that failed, or a connection that died young, backs off
///   2 → 4 → 8 → 10 s and stays at [`BACKOFF_MAX`].
fn next_delay(attempt: &Attempt, backoff: &mut Duration) -> Duration {
    match attempt {
        Attempt::Paired(_) => {
            *backoff = BACKOFF_MIN;
            Duration::ZERO
        }
        Attempt::Ended { lasted, .. } if *lasted >= HEALTHY => {
            *backoff = BACKOFF_MIN;
            Duration::ZERO
        }
        Attempt::Held => {
            *backoff = BACKOFF_MIN;
            BACKOFF_MIN
        }
        Attempt::Failed(_) | Attempt::Ended { .. } => {
            let delay = *backoff;
            *backoff = (*backoff * 2).min(BACKOFF_MAX);
            delay
        }
    }
}

/// What the relay loop says in the journal, and how often.
///
/// With the backoff capped at ten seconds, a machine that is offline for an
/// afternoon would log a line every ten seconds if every attempt were
/// reported. So a run of failed dials — or of waiting connections that die
/// young — is reported when it starts and then every [`Log::REPEAT`] while it
/// lasts, and the recovery is reported once; a run of 409s is reported once.
/// A healthy waiting connection that ends is always reported: that happens a
/// few times an hour, and it is the line that used to be mislabelled.
#[derive(Default)]
struct Log {
    /// Consecutive dials that failed.
    failing: u32,
    /// Consecutive waiting connections that ended before [`HEALTHY`].
    dropping: u32,
    /// When a line about the current run was last written.
    said: Option<Instant>,
    held: bool,
    /// Whether "this relay does not answer pings" has been said.
    said_no_pongs: bool,
}

impl Log {
    const REPEAT: Duration = Duration::from_secs(600);

    /// A dial completed: a waiting connection is up.
    fn reached(&mut self) {
        if self.failing > 0 {
            eprintln!(
                "rime-remoted: the relay is reachable again after {} failed attempt(s); waiting \
                 for a device",
                self.failing
            );
        } else if self.held {
            eprintln!(
                "rime-remoted: the relay has released this machine's rendezvous; waiting for a \
                 device"
            );
        }
        self.failing = 0;
        self.held = false;
    }

    /// Whether the `n`th line of a run is worth writing.
    fn due(&mut self, n: u32) -> bool {
        let due = n == 1 || self.said.map_or(true, |t| t.elapsed() >= Self::REPEAT);
        if due {
            self.said = Some(Instant::now());
        }
        due
    }

    fn after(&mut self, attempt: &Attempt, delay: Duration, answers_pings: &AtomicBool) {
        let retry = if delay.is_zero() {
            "re-dialling now".to_string()
        } else {
            format!("retrying in {} s", delay.as_secs())
        };
        match attempt {
            Attempt::Paired(_) => self.dropping = 0,
            Attempt::Held => {
                if !self.held {
                    eprintln!(
                        "rime-remoted: the relay says another connection already holds this \
                         machine's rendezvous (409) — usually a waiting connection this machine \
                         dropped that the relay has not noticed yet; {retry}, until it has"
                    );
                }
                self.held = true;
            }
            Attempt::Ended { lasted, why } if *lasted >= HEALTHY => {
                self.dropping = 0;
                eprintln!(
                    "rime-remoted: the relay's waiting connection ended after {} ({why}); {retry}",
                    human(*lasted)
                );
                if !answers_pings.load(Ordering::Relaxed)
                    && *lasted >= KEEPALIVE
                    && !self.said_no_pongs
                {
                    self.said_no_pongs = true;
                    eprintln!(
                        "rime-remoted: this relay has not answered a keepalive ping, so a waiting \
                         connection it loses without closing cannot be noticed from here; it is \
                         re-dialled only when the relay closes it"
                    );
                }
            }
            Attempt::Ended { lasted, why } => {
                self.dropping += 1;
                if self.due(self.dropping) {
                    eprintln!(
                        "rime-remoted: the relay dropped the waiting connection after {} ({why}; \
                         {} in a row); {retry}",
                        human(*lasted),
                        self.dropping
                    );
                }
            }
            Attempt::Failed(e) => {
                self.failing += 1;
                if self.due(self.failing) {
                    if self.failing == 1 {
                        eprintln!("rime-remoted: the relay is not reachable ({e}); {retry}");
                    } else {
                        eprintln!(
                            "rime-remoted: the relay is still not reachable after {} attempts \
                             ({e}); retrying every {} s",
                            self.failing,
                            delay.as_secs()
                        );
                    }
                }
            }
        }
    }
}

/// A duration the way a person reads one in a log line.
fn human(d: Duration) -> String {
    let s = d.as_secs();
    if s < 120 {
        format!("{s} s")
    } else if s < 7200 {
        format!("{} min", s / 60)
    } else {
        format!("{} h {} min", s / 3600, (s % 3600) / 60)
    }
}

/// Dial, then wait for a device, and say how it went.
fn attempt(
    endpoint: &Endpoint,
    rendezvous: &str,
    trust: Option<&Trust>,
    timing: Timing,
    answers_pings: &AtomicBool,
    log: &mut Log,
) -> Attempt {
    let joined = match dial(endpoint, rendezvous, Role::Host, trust) {
        Ok(joined) => joined,
        // Only a HOST dial reaches this, and the relay has one 409 for a
        // host: "this rendezvous already has a host waiting" (`room.js`,
        // `decide`). The other 409, "no desktop is waiting", is a guest's.
        Err(e) if e.refused_with() == Some(409) => return Attempt::Held,
        Err(e) => return Attempt::Failed(e),
    };
    log.reached();
    let since = Instant::now();
    match wait(joined, timing, answers_pings) {
        Ok(joined) => Attempt::Paired(joined),
        Err(why) => Attempt::Ended {
            lasted: since.elapsed(),
            why,
        },
    }
}

/// What the socket under a relay connection last reported, kept for after a
/// [`Receiver`] error has flattened it into text.
///
/// The receiver turns every read failure into `RelayError::Upgrade` with the
/// error's words — which is how an end of file after an hour of waiting came
/// to be logged as "the relay refused the connection: failed to fill whole
/// buffer". Reading the facts at the source is simpler than parsing the
/// sentence back.
#[derive(Default)]
pub struct ReadWatch {
    eof: AtomicBool,
    timed_out: AtomicBool,
}

struct Watched {
    inner: Box<dyn Read + Send>,
    watch: Arc<ReadWatch>,
}

impl Read for Watched {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self.inner.read(buf) {
            Ok(0) if !buf.is_empty() => {
                self.watch.eof.store(true, Ordering::Relaxed);
                Ok(0)
            }
            Err(e) => {
                match e.kind() {
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut => {
                        self.watch.timed_out.store(true, Ordering::Relaxed)
                    }
                    std::io::ErrorKind::UnexpectedEof => self.watch.eof.store(true, Ordering::Relaxed),
                    _ => {}
                }
                Err(e)
            }
            ok => ok,
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
/// blocked in a plain `read` on exactly this socket. It is also where the
/// waiting deadline is set, for the same reason.
pub struct Joined {
    pub receiver: Receiver<Box<dyn Read + Send>>,
    pub sender: Arc<Mutex<Sender<Box<dyn Write + Send>>>>,
    /// Held so the connection can be shut down from another thread.
    pub socket: TcpStream,
    /// What the reading half last saw at the socket.
    pub watch: Arc<ReadWatch>,
}

/// Block until a device arrives on a waiting connection, or the wait ends.
///
/// A keepalive thread pings while this waits, and stops when the wait ends.
fn wait(mut joined: Joined, timing: Timing, answers_pings: &AtomicBool) -> Result<Joined, WaitEnd> {
    // The keepalive runs only while waiting. Once a device is on the far end
    // the session's own traffic keeps the connection warm, and an extra ping
    // interleaved with it is a frame the splice has to step over.
    let waiting = Arc::new(AtomicBool::new(true));
    {
        let sender = Arc::clone(&joined.sender);
        let waiting = Arc::clone(&waiting);
        std::thread::spawn(move || {
            while waiting.load(Ordering::Relaxed) {
                std::thread::sleep(timing.keepalive);
                if !waiting.load(Ordering::Relaxed) {
                    return;
                }
                let Ok(mut s) = sender.lock() else { return };
                if s.ping(b"rime").is_err() {
                    return;
                }
            }
        });
    }

    // SO_RCVTIMEO on the socket itself, which the TLS reader's clone shares:
    // one `read(2)` that waits this long is the deadline, and any frame at
    // all — the waiting notice, a pong, a ping from the relay — restarts it.
    let arm = |joined: &Joined| joined.socket.set_read_timeout(Some(timing.deadline)).is_ok();
    let mut armed = answers_pings.load(Ordering::Relaxed) && arm(&joined);

    let outcome = loop {
        let message = match joined.receiver.message() {
            Ok(m) => m,
            Err(e) => break Err(ended_by(&joined.watch, e, timing)),
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
            // The relay answers pings: the deadline can be trusted, for this
            // connection and every later one.
            Op::Pong => {
                answers_pings.store(true, Ordering::Relaxed);
                if !armed {
                    armed = arm(&joined);
                }
            }
            Op::Close => break Err(WaitEnd::Closed("it sent a WebSocket close".into())),
            // Binary before a device has been announced means the relay
            // joined this connection to something without saying so, and the
            // bytes would be fed to a splice that has not been set up.
            Op::Binary | Op::Continuation => {
                break Err(WaitEnd::Broke(RelayError::Protocol(
                    "payload before the relay said a device had arrived",
                )))
            }
        }
    };
    waiting.store(false, Ordering::Relaxed);
    match outcome {
        Ok(()) => {
            // Off before the splice. A relayed terminal may be idle for
            // hours, and a waiting-room deadline left on its socket would end
            // it after 75 s of nobody typing.
            joined.socket.set_read_timeout(None).ok();
            Ok(joined)
        }
        Err(end) => {
            // A close frame first, best effort: if the path is fine and only
            // the relay went quiet, this is what frees the room for the next
            // dial instead of leaving it held — and a 409 — until the relay
            // notices on its own. On a dead path it goes nowhere and costs
            // nothing.
            if matches!(end, WaitEnd::Silent(_)) {
                if let Ok(mut s) = joined.sender.lock() {
                    let _ = s.close();
                }
            }
            let _ = joined.socket.shutdown(std::net::Shutdown::Both);
            Err(end)
        }
    }
}

/// Name the end of a wait from what the socket saw, not from the receiver's
/// sentence.
fn ended_by(watch: &ReadWatch, e: RelayError, timing: Timing) -> WaitEnd {
    if watch.timed_out.load(Ordering::Relaxed) {
        WaitEnd::Silent(timing.deadline)
    } else if watch.eof.load(Ordering::Relaxed) {
        WaitEnd::Closed("it closed the connection without a WebSocket close".into())
    } else {
        match e {
            // The receiver's wrapper for a read error; the words are the
            // socket's (a reset, a TLS alert).
            RelayError::Upgrade(why) => WaitEnd::Closed(format!("the connection failed: {why}")),
            other => WaitEnd::Broke(other),
        }
    }
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
    let socket =
        TcpStream::connect((endpoint.host.as_str(), endpoint.port)).map_err(DialError::Tcp)?;
    // Nagle off: the carried stream is a terminal, and coalescing a keystroke
    // with whatever comes next is exactly the latency this feature is judged
    // on.
    socket.set_nodelay(true).ok();

    let (reading, mut writing): (Box<dyn Read + Send>, Box<dyn Write + Send>) =
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
    let watch = Arc::new(ReadWatch::default());
    let mut reading: Box<dyn Read + Send> = Box::new(Watched {
        inner: reading,
        watch: Arc::clone(&watch),
    });
    let opening = Opening::new();
    writing
        .write_all(&opening.request(endpoint, rendezvous, role))
        .and_then(|()| writing.flush())
        .map_err(|e| DialError::Upgrade(RelayError::Upgrade(format!("the request was not sent: {e}"))))?;
    opening.accept(&mut reading).map_err(DialError::Upgrade)?;

    Ok(Joined {
        receiver: Receiver::new(reading),
        sender: Arc::new(Mutex::new(Sender::new(writing))),
        socket,
        watch,
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
    let relay_socket = joined.socket.try_clone()?;
    // Set by the pump when THIS side ended the session, so the reader below
    // does not report the socket the pump shut as a failure.
    let ours = Arc::new(AtomicBool::new(false));
    let hung_up = Arc::clone(&ours);
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
        // And the relay socket down, so the reader below stops too. The
        // local side ends first whenever THIS machine ends the session — the
        // phone stopped answering pings, or it was revoked — and on a relay
        // path that has gone dead the close frame above reaches nobody, so
        // the reader would otherwise wait on it for as long as TCP let it:
        // a thread and a socket per dead relayed session.
        hung_up.store(true, Ordering::Relaxed);
        let _ = relay_socket.shutdown(std::net::Shutdown::Both);
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
            Err(_) if ours.load(Ordering::Relaxed) => break Ok(()),
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
        // A plain-HTTP server presented no certificate at all, so this must
        // NOT be reported as a certificate refusal — an operator reading that
        // would go and inspect a CA store over a server that speaks no TLS.
        assert!(
            matches!(e, DialError::Tls(TlsError::Handshake(_))),
            "a server that speaks no TLS was blamed on its certificate: {e}"
        );
        assert!(e.to_string().contains("handshake"), "{e}");

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

    // ─────────────────────────────────────────────────────────────────────
    //  A relay that really speaks TLS, and a dial that really reaches it
    // ─────────────────────────────────────────────────────────────────────

    /// Mint a CA and a server certificate for `127.0.0.1`, and return both
    /// paths plus the directory holding them.
    ///
    /// `subjectAltName = IP:127.0.0.1` rather than a DNS name, because
    /// `dial` checks the certificate against `Endpoint.host` — which for a
    /// loopback relay URL IS the address. That is the property being relied
    /// on, so the certificate has to be the kind that can satisfy it.
    fn mint_for_loopback() -> (std::path::PathBuf, std::path::PathBuf, std::path::PathBuf) {
        use std::process::Command;
        let dir = std::env::temp_dir().join(format!(
            "rime-remoted-wss-{}-{}",
            std::process::id(),
            rime_remote_core::now_ms()
        ));
        std::fs::create_dir_all(&dir).expect("scratch");
        let run = |args: &[&str]| {
            let out = Command::new("openssl").args(args).output().expect("openssl");
            assert!(
                out.status.success(),
                "openssl {args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        let s = |p: &std::path::Path| p.display().to_string();
        let (ca_key, ca_pem) = (dir.join("ca.key"), dir.join("ca.pem"));
        let (key, csr, pem, ext) = (
            dir.join("leaf.key"),
            dir.join("leaf.csr"),
            dir.join("leaf.pem"),
            dir.join("leaf.ext"),
        );
        for k in [&ca_key, &key] {
            run(&["genpkey", "-algorithm", "EC", "-pkeyopt", "ec_paramgen_curve:P-256", "-out", &s(k)]);
        }
        run(&[
            "req", "-x509", "-new", "-key", &s(&ca_key), "-sha256", "-days", "3650",
            "-subj", "/CN=rime relay loopback CA",
            "-addext", "basicConstraints=critical,CA:TRUE",
            "-addext", "keyUsage=critical,keyCertSign,cRLSign",
            "-out", &s(&ca_pem),
        ]);
        std::fs::write(
            &ext,
            "subjectAltName = IP:127.0.0.1\n\
             basicConstraints = critical,CA:FALSE\n\
             keyUsage = critical,digitalSignature,keyEncipherment\n\
             extendedKeyUsage = serverAuth\n",
        )
        .expect("ext");
        run(&["req", "-new", "-key", &s(&key), "-subj", "/CN=127.0.0.1", "-out", &s(&csr)]);
        run(&[
            "x509", "-req", "-in", &s(&csr), "-CA", &s(&ca_pem), "-CAkey", &s(&ca_key),
            "-CAcreateserial", "-sha256", "-days", "365", "-extfile", &s(&ext),
            "-out", &s(&pem),
        ]);
        (dir, ca_pem, pem)
    }

    /// Read one frame a CLIENT sent. Clients mask; that is RFC 6455 §5.3.
    fn read_client_frame(r: &mut impl Read) -> Option<(u8, Vec<u8>)> {
        let mut head = [0u8; 2];
        r.read_exact(&mut head).ok()?;
        let op = head[0] & 0x0f;
        let masked = head[1] & 0x80 != 0;
        let mut len = (head[1] & 0x7f) as usize;
        if len == 126 {
            let mut ext = [0u8; 2];
            r.read_exact(&mut ext).ok()?;
            len = u16::from_be_bytes(ext) as usize;
        }
        let mut mask = [0u8; 4];
        if masked {
            r.read_exact(&mut mask).ok()?;
        }
        let mut payload = vec![0u8; len];
        r.read_exact(&mut payload).ok()?;
        if masked {
            for (i, b) in payload.iter_mut().enumerate() {
                *b ^= mask[i % 4];
            }
        }
        Some((op, payload))
    }

    /// Write one frame as a SERVER. Servers must not mask.
    fn write_server_frame(w: &mut impl Write, op: u8, payload: &[u8]) -> std::io::Result<()> {
        let mut out = vec![0x80 | op];
        if payload.len() < 126 {
            out.push(payload.len() as u8);
        } else {
            out.push(126);
            out.extend_from_slice(&(payload.len() as u16).to_be_bytes());
        }
        out.extend_from_slice(payload);
        w.write_all(&out)?;
        w.flush()
    }

    /// A relay that terminates TLS, completes the RFC 6455 upgrade, announces
    /// itself, and echoes what it is sent.
    fn a_tls_relay(cert: &std::path::Path, key: &std::path::Path) -> u16 {
        use rustls::pki_types::pem::PemObject;
        use rustls::pki_types::{CertificateDer, PrivateKeyDer};

        let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_file_iter(cert)
            .expect("cert")
            .collect::<Result<_, _>>()
            .expect("cert parse");
        let key = PrivateKeyDer::from_pem_file(key).expect("key");
        let config = Arc::new(
            rustls::ServerConfig::builder_with_provider(Arc::new(
                rustls::crypto::ring::default_provider(),
            ))
            .with_safe_default_protocol_versions()
            .expect("versions")
            .with_no_client_auth()
            .with_single_cert(certs, key)
            .expect("server config"),
        );

        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind");
        let port = listener.local_addr().expect("addr").port();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let config = Arc::clone(&config);
                std::thread::spawn(move || {
                    let Ok(conn) = rustls::ServerConnection::new(config) else { return };
                    let mut tls = rustls::StreamOwned::new(conn, stream);

                    // Byte at a time, so the first frame is not swallowed with
                    // the response head — the same reason the client reads
                    // this way.
                    let mut head = Vec::new();
                    let mut byte = [0u8; 1];
                    while tls.read_exact(&mut byte).is_ok() {
                        head.push(byte[0]);
                        if head.ends_with(b"\r\n\r\n") || head.len() > 16 * 1024 {
                            break;
                        }
                    }
                    let text = String::from_utf8_lossy(&head).to_string();
                    let Some(key) = text.split("\r\n").find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        (name.trim().eq_ignore_ascii_case("sec-websocket-key"))
                            .then(|| value.trim().to_string())
                    }) else {
                        return;
                    };
                    let reply = format!(
                        "HTTP/1.1 101 Switching Protocols\r\n\
                         Upgrade: websocket\r\n\
                         Connection: Upgrade\r\n\
                         Sec-WebSocket-Accept: {}\r\n\r\n",
                        rime_remote_core::relay::accept_for(&key)
                    );
                    if tls.write_all(reply.as_bytes()).is_err() {
                        return;
                    }
                    let _ = tls.flush();
                    let _ = write_server_frame(&mut tls, 0x1, Notice::Waiting.text().as_bytes());

                    while let Some((op, payload)) = read_client_frame(&mut tls) {
                        let answered = match op {
                            0x2 => write_server_frame(&mut tls, 0x2, &payload),
                            0x9 => write_server_frame(&mut tls, 0xa, &payload),
                            0x8 => break,
                            _ => Ok(()),
                        };
                        if answered.is_err() {
                            break;
                        }
                    }
                });
            }
        });
        port
    }

    #[test]
    fn a_wss_relay_is_dialled_verified_and_carries_frames_both_ways() {
        // THE headline claim of this unit, and the one thing every other test
        // here only approaches from one side: `tests/tls.rs` proves what the
        // TLS layer refuses over a raw byte stream, and the relay suite proves
        // the room protocol over ws://. Neither runs `dial`'s secure branch to
        // a SUCCESS — the upgrade request written into a `TlsWriter`, the
        // response read a byte at a time back out of a `TlsReader`, and then
        // real frames through `Sender`/`Receiver` over the split connection.
        let (dir, ca, cert) = mint_for_loopback();
        let key = dir.join("leaf.key");
        let port = a_tls_relay(&cert, &key);

        let trust = Trust::from_pem_file(&ca).expect("trust the minted CA");
        let endpoint = Endpoint::parse(&format!("wss://127.0.0.1:{port}")).expect("parse");
        assert!(endpoint.secure, "the test is not exercising the TLS branch");

        let mut joined = dial(&endpoint, "aaaabbbbccccdddd", Role::Host, Some(&trust))
            .expect("a wss:// relay that verifies should be dialled");

        // The relay's own notice, decoded from a frame that arrived encrypted.
        let first = joined.receiver.message().expect("the waiting notice");
        assert_eq!(first.op, Op::Text);
        assert_eq!(Notice::parse(&first.payload), Some(Notice::Waiting));

        // A keepalive ping from ANOTHER thread while this one is blocked in a
        // read. That overlap is not hypothetical — it is exactly what
        // `wait_for_a_device` does for as long as a desktop waits — and it is
        // what deadlocks if the reader holds the TLS connection's lock across
        // its socket read. A deadlock here hangs the test rather than failing
        // it, which is why the ping is sent from a thread that cannot be the
        // one parked in `message()`.
        {
            let sender = Arc::clone(&joined.sender);
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(50));
                if let Ok(mut s) = sender.lock() {
                    let _ = s.ping(b"rime");
                }
            });
        }
        let pong = joined.receiver.message().expect("a pong");
        assert_eq!(pong.op, Op::Pong, "the keepalive did not survive the TLS leg");
        assert_eq!(pong.payload, b"rime");

        // And the carried stream itself: a payload out and the same bytes
        // back, through the codec, through TLS, both ways.
        let carried = b"rime-remote-wss-carried-payload-4f2a91c7";
        joined.sender.lock().expect("sender").binary(carried).expect("binary");
        let echoed = joined.receiver.message().expect("the echo");
        assert_eq!(echoed.op, Op::Binary);
        assert_eq!(echoed.payload, carried, "the payload did not survive the TLS leg");

        let _ = joined.socket.shutdown(std::net::Shutdown::Both);
        let _ = std::fs::remove_dir_all(&dir);
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

    // ─────────────────────────────────────────────────────────────────────
    //  Re-dialling: when, how soon, and what it is called in the journal
    // ─────────────────────────────────────────────────────────────────────

    fn tcp_failure() -> Attempt {
        Attempt::Failed(DialError::Tcp(std::io::Error::from(
            std::io::ErrorKind::ConnectionRefused,
        )))
    }

    #[test]
    fn the_backoff_grows_to_ten_seconds_and_stops() {
        let mut backoff = BACKOFF_MIN;
        let seen: Vec<u64> = (0..8)
            .map(|_| next_delay(&tcp_failure(), &mut backoff).as_secs())
            .collect();
        assert_eq!(seen, vec![2, 4, 8, 10, 10, 10, 10, 10]);
        // A laptop whose Wi-Fi just came back must not wait out a backoff it
        // spent asleep accumulating, and the phone trying to reach it pays
        // every second of this number.
        assert!(BACKOFF_MAX <= Duration::from_secs(10));
        // A connection that died young is a failure too, or a relay that
        // accepted and dropped would be dialled in a tight loop.
        let young = Attempt::Ended {
            lasted: Duration::from_millis(300),
            why: WaitEnd::Closed("x".into()),
        };
        assert_eq!(next_delay(&young, &mut backoff), BACKOFF_MAX);
    }

    #[test]
    fn a_409_is_not_a_failure_and_a_healthy_connection_is_redialled_at_once() {
        // The 409 a host gets means "a host is already waiting here" —
        // usually this machine's own previous connection, which the relay
        // has not noticed is gone. The relay is reachable and answering, so
        // it must not push the next attempt out to the cap; but it is still
        // retried after a pause, because each retry is an upgrade counted
        // against the relay's per-address limit.
        let mut backoff = BACKOFF_MAX;
        for _ in 0..5 {
            assert_eq!(next_delay(&Attempt::Held, &mut backoff), BACKOFF_MIN);
            assert_eq!(backoff, BACKOFF_MIN, "a 409 grew the backoff");
        }
        // And a failure after a run of 409s starts from the bottom.
        assert_eq!(next_delay(&tcp_failure(), &mut backoff), BACKOFF_MIN);

        // The Worker drops a waiting socket every twenty minutes to an hour.
        // That is not trouble reaching the relay, and the sleep that used to
        // follow it was a window of 409s for every phone.
        let mut backoff = BACKOFF_MAX;
        for why in [
            WaitEnd::Closed("it closed the connection without a WebSocket close".into()),
            WaitEnd::Silent(WAIT_DEADLINE),
        ] {
            let healthy = Attempt::Ended {
                lasted: WAIT_DEADLINE,
                why,
            };
            assert_eq!(next_delay(&healthy, &mut backoff), Duration::ZERO);
            assert_eq!(backoff, BACKOFF_MIN);
        }
        // A silent connection has by construction lived the whole deadline,
        // so it is always re-dialled at once.
        assert!(WAIT_DEADLINE >= HEALTHY);
        assert!(WAIT_DEADLINE > KEEPALIVE * 2, "the deadline must outlast two keepalives");
    }

    #[test]
    fn a_409_is_read_from_the_relays_own_status_line() {
        // Built with `Opening::check` itself rather than a string written out
        // here: `refused_with` parses that function's sentence, and a
        // rewording in rime-remote-core must fail this test instead of
        // silently turning every 409 back into a failure.
        let opening = Opening::new();
        for (head, want) in [
            (&b"HTTP/1.1 409 Conflict\r\nContent-Length: 0\r\n\r\n"[..], Some(409)),
            (&b"HTTP/1.1 404 Not Found\r\n\r\n"[..], Some(404)),
            (&b"HTTP/1.1 429 Too Many Requests\r\n\r\n"[..], Some(429)),
        ] {
            let e = DialError::Upgrade(opening.check(head).expect_err("not a 101"));
            assert_eq!(e.refused_with(), want, "{e}");
        }
        // A 101 that is wrong in some other way is not a status refusal.
        let bad_accept = b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\n\
                           Connection: Upgrade\r\nSec-WebSocket-Accept: nope\r\n\r\n";
        let e = DialError::Upgrade(opening.check(bad_accept).expect_err("a wrong accept"));
        assert_eq!(e.refused_with(), None, "{e}");
        assert_eq!(
            DialError::Tcp(std::io::ErrorKind::TimedOut.into()).refused_with(),
            None
        );
    }

    #[test]
    fn a_dial_failure_names_the_step_that_failed() {
        // The journal said "the relay refused the connection" for every one
        // of these, and for the end of an hour-long wait besides.
        let closed = {
            let l = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind");
            l.local_addr().expect("addr").port()
        };
        let endpoint = Endpoint::parse(&format!("ws://127.0.0.1:{closed}")).expect("parse");
        let Err(e) = dial(&endpoint, "aaaabbbbccccdddd", Role::Host, None) else {
            panic!("dialled a port nothing listens on");
        };
        assert!(matches!(e, DialError::Tcp(_)), "{e:?}");
        assert!(e.to_string().starts_with("TCP: "), "{e}");

        // Something that answers HTTP but will not upgrade.
        let (port, _) = a_plain_listener();
        let endpoint = Endpoint::parse(&format!("ws://127.0.0.1:{port}")).expect("parse");
        let Err(e) = dial(&endpoint, "aaaabbbbccccdddd", Role::Host, None) else {
            panic!("a server that answered 400 was taken for a relay");
        };
        assert!(matches!(e, DialError::Upgrade(_)), "{e:?}");
        assert!(e.to_string().starts_with("WebSocket upgrade: "), "{e}");
        assert_eq!(e.refused_with(), Some(400), "{e}");
    }

    // ─────────────────────────────────────────────────────────────────────
    //  A waiting room that misbehaves on purpose
    // ─────────────────────────────────────────────────────────────────────

    /// What a parked host connection gets from [`a_waiting_room`] after the
    /// `waiting` notice.
    #[derive(Clone, Copy)]
    enum Parked {
        /// Answer this many pings, then say nothing at all, with the socket
        /// left open: a path that has gone dead without a FIN.
        PongsThenSilence(usize),
        /// Answer nothing, ever, and keep the socket open.
        NeverPongs,
        /// Answer pings, and announce a device after this long.
        PairsAfter(Duration),
    }

    /// A ws:// relay that holds whatever arrives as a waiting host and then
    /// behaves as told. Every accepted socket is kept, so the test can cut it.
    fn a_waiting_room(parked: Parked) -> (u16, Arc<Mutex<Vec<TcpStream>>>) {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let held = Arc::new(Mutex::new(Vec::<TcpStream>::new()));
        let kept = Arc::clone(&held);
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                if let Ok(c) = stream.try_clone() {
                    kept.lock().expect("held").push(c);
                }
                std::thread::spawn(move || {
                    let mut s = stream;
                    let mut head = Vec::new();
                    let mut byte = [0u8; 1];
                    while !head.ends_with(b"\r\n\r\n") {
                        if s.read(&mut byte).unwrap_or(0) == 0 {
                            return;
                        }
                        head.push(byte[0]);
                    }
                    let text = String::from_utf8_lossy(&head).to_string();
                    let Some(key) = text.split("\r\n").find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.trim()
                            .eq_ignore_ascii_case("sec-websocket-key")
                            .then(|| value.trim().to_string())
                    }) else {
                        return;
                    };
                    let reply = format!(
                        "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\n\
                         Connection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n\r\n",
                        rime_remote_core::relay::accept_for(&key)
                    );
                    if s.write_all(reply.as_bytes()).is_err()
                        || write_server_frame(&mut s, 0x1, Notice::Waiting.text().as_bytes())
                            .is_err()
                    {
                        return;
                    }
                    match parked {
                        Parked::PongsThenSilence(n) => {
                            let mut answered = 0;
                            while answered < n {
                                match read_client_frame(&mut s) {
                                    Some((0x9, p)) => {
                                        if write_server_frame(&mut s, 0xa, &p).is_err() {
                                            return;
                                        }
                                        answered += 1;
                                    }
                                    Some(_) => {}
                                    None => return,
                                }
                            }
                            // Silence. The clone in `held` keeps it open.
                        }
                        Parked::NeverPongs => {}
                        Parked::PairsAfter(after) => {
                            std::thread::sleep(after);
                            let _ =
                                write_server_frame(&mut s, 0x1, Notice::Paired.text().as_bytes());
                        }
                    }
                });
            }
        });
        (port, held)
    }

    const FAST: Timing = Timing {
        keepalive: Duration::from_millis(40),
        deadline: Duration::from_millis(250),
    };

    /// Run one attempt on its own thread and hand back what it returned.
    fn attempt_in_background(
        port: u16,
        answers_pings: Arc<AtomicBool>,
    ) -> std::sync::mpsc::Receiver<(Attempt, Duration)> {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let endpoint = Endpoint::parse(&format!("ws://127.0.0.1:{port}")).expect("parse");
            let started = Instant::now();
            let outcome = attempt(
                &endpoint,
                "aaaabbbbccccdddd",
                None,
                FAST,
                &answers_pings,
                &mut Log::default(),
            );
            let _ = tx.send((outcome, started.elapsed()));
        });
        rx
    }

    #[test]
    fn a_waiting_connection_that_goes_silent_is_dropped_at_the_deadline() {
        // The black hole: the relay answered a ping, so it is one that
        // answers, and then nothing came back at all — no FIN, no close. The
        // old loop would have sat on this connection until the relay closed
        // its end, looking parked the whole time.
        let (port, _held) = a_waiting_room(Parked::PongsThenSilence(1));
        let answers = Arc::new(AtomicBool::new(false));
        let rx = attempt_in_background(port, Arc::clone(&answers));
        let (outcome, took) = rx
            .recv_timeout(Duration::from_secs(10))
            .expect("the silent waiting connection was never given up on");
        let Attempt::Ended { lasted, why } = outcome else {
            panic!("expected the wait to end");
        };
        assert!(matches!(why, WaitEnd::Silent(_)), "{why}");
        assert!(why.to_string().contains("despite keepalive pings"), "{why}");
        assert!(lasted >= FAST.deadline, "dropped after {lasted:?}, before the deadline");
        assert!(took < Duration::from_secs(5), "took {took:?}");
        assert!(answers.load(Ordering::Relaxed), "the pong was not taken as liveness");

        // Now the process knows this relay answers, so the NEXT waiting
        // connection is held to the deadline from its first byte, even if it
        // never gets a pong of its own.
        let (port, _held) = a_waiting_room(Parked::NeverPongs);
        let rx = attempt_in_background(port, Arc::clone(&answers));
        let (outcome, _) = rx
            .recv_timeout(Duration::from_secs(10))
            .expect("a relay known to answer pings went silent and was not noticed");
        assert!(
            matches!(outcome, Attempt::Ended { why: WaitEnd::Silent(_), .. }),
            "the learned deadline was not applied"
        );
    }

    #[test]
    fn a_relay_that_has_never_answered_a_ping_is_not_held_to_the_deadline() {
        // The hedge. If the relay does not pong at all, a deadline would
        // re-dial it every 75 s for ever with a 409 window each time — worse
        // than the defect it fixes. So it waits as it always did, until the
        // relay closes its end...
        let (port, held) = a_waiting_room(Parked::NeverPongs);
        let rx = attempt_in_background(port, Arc::new(AtomicBool::new(false)));
        assert!(
            rx.recv_timeout(FAST.deadline * 4).is_err(),
            "a relay that never ponged was dropped for not ponging"
        );
        // ...and then the end is called what it is. This is the event the
        // journal logged as "the relay refused the connection: failed to fill
        // whole buffer" every twenty minutes to an hour.
        for s in held.lock().expect("held").iter() {
            let _ = s.shutdown(std::net::Shutdown::Both);
        }
        let (outcome, _) = rx
            .recv_timeout(Duration::from_secs(10))
            .expect("the closed waiting connection was not noticed");
        let Attempt::Ended { why, .. } = outcome else {
            panic!("expected the wait to end");
        };
        assert!(matches!(why, WaitEnd::Closed(_)), "{why}");
        let said = why.to_string();
        assert!(said.contains("closed the connection"), "{said}");
        assert!(!said.contains("refused"), "the end of a wait was called a refusal: {said}");
    }

    #[test]
    fn a_paired_connection_carries_no_waiting_deadline_into_the_splice() {
        // A relayed terminal can be idle for hours. The deadline belongs to
        // the waiting room, and one left on the socket would end every quiet
        // relayed session 75 s after its last byte.
        let (port, _held) = a_waiting_room(Parked::PairsAfter(Duration::from_millis(120)));
        let rx = attempt_in_background(port, Arc::new(AtomicBool::new(true)));
        let (outcome, _) = rx.recv_timeout(Duration::from_secs(10)).expect("paired");
        let Attempt::Paired(joined) = outcome else {
            panic!("the device's arrival was not seen");
        };
        assert_eq!(joined.socket.read_timeout().expect("SO_RCVTIMEO"), None);
        let _ = joined.socket.shutdown(std::net::Shutdown::Both);
    }
}
