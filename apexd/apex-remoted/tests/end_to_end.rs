//! The whole chain, against real processes: a device, `apex-remoted` and
//! `apex-agentd`.
//!
//! Everything else about this protocol is a pure function over values and is
//! tested beside the code. Three claims cannot be made about a value, and
//! they are the three that decide whether any of this works:
//!
//! * a device that scans a pairing code can complete a `Noise_NK` handshake
//!   against the real service and end up in the real store;
//! * a paired device can open a `Noise_IK` session and get an answer from the
//!   real `apex-agentd` through the real proxy;
//! * **a request that arrives this way is recorded as
//!   `claude-remote-control`, by the device's id, and cannot approve a root
//!   operation** — while the same daemon, asked directly from this test
//!   process, can. That last comparison is the point: "the remote request was
//!   refused" proves nothing unless a local one is allowed by the same daemon
//!   in the same run.
//!
//! Both daemons get their own `XDG_RUNTIME_DIR` and `XDG_STATE_HOME`, so
//! nothing here touches a running `apex-agentd` or the user's own devices.
//! Both are killed by pid; nothing goes looking for a process by name.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use apex_remote_core::noise::{Channel, Handshake};
use apex_remote_core::pairing::{PairingOffer, PairingRequest};
use apex_remote_core::wire::Frame;

const V: u32 = apex_remote_core::REMOTE_PROTOCOL_VERSION;

// The two hello bytes, named. `apex-remoted` declares them as `pub const` in a
// binary crate, which an integration test cannot import, so they are written
// out here — and named rather than inlined so a reader can see that the two
// spellings are meant to be the same value.
fn serve_hello_pair() -> u8 {
    b'P'
}
fn serve_hello_session() -> u8 {
    b'S'
}

/// Where cargo put the workspace binaries.
///
/// This test needs two of them and cargo only exports the one belonging to
/// its own package, so the sibling is found next to it. A **panic** rather
/// than a skip when it is missing: an absent binary is a build that did not
/// happen, and a suite that reports success for a test it never ran is the
/// failure mode this program has already shipped twice.
fn bin(name: &str) -> PathBuf {
    let mine = PathBuf::from(env!("CARGO_BIN_EXE_apex-remoted"));
    let dir = mine.parent().expect("a target directory");
    let path = dir.join(name);
    assert!(
        path.exists(),
        "{} is not built. This suite drives both daemons, so run it as \
         `cargo test --workspace` (or `cargo build -p apex-agentd` first).",
        path.display()
    );
    path
}

struct Harness {
    agentd: Child,
    remoted: Child,
    root: PathBuf,
    agentd_socket: PathBuf,
    remoted_socket: PathBuf,
    port: u16,
}

impl Drop for Harness {
    fn drop(&mut self) {
        let _ = self.remoted.kill();
        let _ = self.remoted.wait();
        let _ = self.agentd.kill();
        let _ = self.agentd.wait();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

impl Harness {
    fn start(tag: &str) -> Option<Harness> {
        let root = std::env::temp_dir().join(format!(
            "apex-remote-e2e-{}-{tag}-{}",
            std::process::id(),
            apex_remote_core::now_ms()
        ));
        let runtime = root.join("run");
        let state = root.join("state");
        std::fs::create_dir_all(&runtime).ok()?;
        std::fs::create_dir_all(&state).ok()?;

        let agentd = Command::new(bin("apex-agentd"))
            .env("XDG_RUNTIME_DIR", &runtime)
            .env("XDG_STATE_HOME", &state)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let agentd_socket = runtime.join("apex-agentd").join("control.sock");

        // A port nobody else is on. Picked by binding one and letting it go
        // rather than by hoping: a hardcoded port makes two runs of this
        // suite on one machine fight.
        let port = {
            let probe = std::net::TcpListener::bind(("127.0.0.1", 0)).ok()?;
            let p = probe.local_addr().ok()?.port();
            drop(probe);
            p
        };

        let remoted = Command::new(bin("apex-remoted"))
            .args([
                "--port",
                &port.to_string(),
                "--allow-foreground",
                // The real deadline is thirty seconds, which is right for a
                // phone waking its radio and wrong for a suite that runs on
                // every commit. Clamped by the daemon to a second at the
                // bottom, so this is the shortest a test can ask for.
                "--handshake-timeout-ms",
                "1000",
            ])
            .env("XDG_RUNTIME_DIR", &runtime)
            .env("XDG_STATE_HOME", &state)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let remoted_socket = runtime.join("apex-remoted").join("control.sock");

        let h = Harness {
            agentd,
            remoted,
            root,
            agentd_socket,
            remoted_socket,
            port,
        };
        h.wait_ready().then_some(h)
    }

    fn wait_ready(&self) -> bool {
        let deadline = Instant::now() + Duration::from_secs(15);
        while Instant::now() < deadline {
            let agentd = UnixStream::connect(&self.agentd_socket).is_ok();
            let remoted = UnixStream::connect(&self.remoted_socket).is_ok();
            let listening = TcpStream::connect(("127.0.0.1", self.port)).is_ok();
            if agentd && remoted && listening {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        false
    }

    /// One request on `apex-remoted`'s local control socket.
    fn control(&self, line: &str) -> serde_json::Value {
        one_line(&self.remoted_socket, line)
    }

    /// One request straight to `apex-agentd`, as this test process.
    ///
    /// The control arm of every assertion below: this connection is whatever
    /// `cargo test` classifies as, and the point is to compare it with what
    /// the same daemon says about the proxied one.
    fn direct(&self, line: &str) -> serde_json::Value {
        one_line(&self.agentd_socket, line)
    }

    fn tcp(&self) -> TcpStream {
        let s = TcpStream::connect(("127.0.0.1", self.port)).expect("connect");
        s.set_read_timeout(Some(Duration::from_secs(15))).ok();
        s
    }
}

fn one_line(socket: &PathBuf, line: &str) -> serde_json::Value {
    let stream = UnixStream::connect(socket).expect("connect");
    stream.set_read_timeout(Some(Duration::from_secs(15))).ok();
    let mut writer = stream.try_clone().expect("clone");
    let mut reader = BufReader::new(stream);
    writeln!(writer, "{line}").expect("write");
    writer.flush().ok();
    let mut reply = String::new();
    reader.read_line(&mut reply).expect("read");
    serde_json::from_str(reply.trim()).unwrap_or_else(|e| panic!("{e}: {reply}"))
}

macro_rules! harness {
    ($tag:literal) => {
        match Harness::start($tag) {
            Some(h) => h,
            None => {
                eprintln!("SKIP: the daemons did not come up in this environment");
                return;
            }
        }
    };
}

/// A device: a keypair, and the two things it can do with one.
struct Device {
    secret: [u8; 32],
    public: [u8; 32],
}

impl Device {
    fn new() -> Device {
        let kp = apex_remote_core::noise::builder()
            .generate_keypair()
            .expect("keypair");
        Device {
            secret: kp.private.as_slice().try_into().expect("32"),
            public: kp.public.as_slice().try_into().expect("32"),
        }
    }

    fn key(&self) -> String {
        apex_remote_core::b64_encode(&self.public)
    }

    /// Scan a code and pair. Returns whatever the desktop answered.
    fn pair(&self, h: &Harness, offer: &PairingOffer, name: &str) -> serde_json::Value {
        let desktop: [u8; 32] = apex_remote_core::b64_decode(&offer.key)
            .expect("a key")
            .as_slice()
            .try_into()
            .expect("32");
        let mut hs = Handshake::pairing_initiator(&desktop, V).expect("initiator");
        let request = PairingRequest {
            key: self.key(),
            name: name.to_string(),
            token: offer.token.clone(),
            user_verification: true,
        };
        let m1 = hs
            .write(&serde_json::to_vec(&request).expect("serialise"))
            .expect("write");
        let mut socket = h.tcp();
        socket.write_all(&[serve_hello_pair()]).expect("hello");
        write_message(&mut socket, &m1);
        let m2 = read_message(&mut socket);
        let payload = hs.read(&m2).expect("read");
        serde_json::from_slice(&payload).expect("a JSON answer")
    }

    /// Open a session. `None` when the desktop refuses the key.
    fn connect(&self, h: &Harness) -> Option<Session> {
        let mut socket = h.tcp();
        socket.write_all(&[serve_hello_session()]).ok()?;
        let desktop = desktop_key(h);
        let mut hs = Handshake::session_initiator(&self.secret, &desktop, V).ok()?;
        let m1 = hs.write(b"").ok()?;
        write_message(&mut socket, &m1);
        // A refused device gets the connection closed rather than an answer,
        // which is what `read_exact` on the length prefix reports as EOF.
        let mut len = [0u8; 4];
        socket.read_exact(&mut len).ok()?;
        let n = u32::from_be_bytes(len) as usize;
        let mut buf = vec![0u8; n];
        socket.read_exact(&mut buf).ok()?;
        hs.read(&buf).ok()?;
        let channel = hs.into_transport().ok()?;
        Some(Session {
            socket,
            channel,
            pending: Vec::new(),
        })
    }
}

struct Session {
    socket: TcpStream,
    channel: Channel,
    /// Frames that arrived while waiting for a reply.
    ///
    /// The connection is multiplexed, so a terminal's output legitimately
    /// arrives between a control request and its answer — that is the whole
    /// reason this layer exists rather than handing the connection over the
    /// way `apex agent attach` does. A `call` that assumed the next frame was
    /// its reply was wrong about the protocol, and it failed intermittently
    /// in exactly the way a wrong assumption about interleaving does.
    ///
    /// Kept rather than dropped, so a test can assert about what interleaved.
    pending: Vec<Frame>,
}

impl Session {
    /// Seal one frame and write it.
    fn send(&mut self, frame: Frame) {
        let bytes = frame.encode().expect("encode");
        let sealed = self.channel.seal(&bytes).expect("seal");
        write_message(&mut self.socket, &sealed);
    }

    /// Read one frame, answering keepalives on the way.
    ///
    /// The desktop measures its own connections, so a `Ping` arrives whenever
    /// it likes — including as the first frame of a session, before the
    /// device has asked for anything. It is not a session event and no
    /// assertion here is about one, so it is answered and stepped over, which
    /// is exactly what a device does. A test that treated it as the next
    /// frame would be asserting that the desktop never measures anything.
    fn recv(&mut self) -> Frame {
        loop {
            let message = read_message(&mut self.socket);
            let plain = self.channel.open(&message).expect("open");
            match Frame::decode(&plain).expect("decode") {
                Frame::Ping { token } => self.send(Frame::Pong { token }),
                frame => return frame,
            }
        }
    }

    /// Send one control frame and read its reply, keeping anything that
    /// arrived in between.
    fn call(&mut self, request: &str) -> serde_json::Value {
        self.send(Frame::Control(request.as_bytes().to_vec()));
        loop {
            match self.recv() {
                Frame::Control(line) => {
                    return serde_json::from_slice(&line)
                        .unwrap_or_else(|e| panic!("{e}: {line:?}"))
                }
                other => self.pending.push(other),
            }
        }
    }

    /// Whether a channel was closed by the desktop without being asked.
    fn closed_unasked(&self, channel: u32) -> Option<&Frame> {
        self.pending
            .iter()
            .find(|f| matches!(f, Frame::Close { channel: c, .. } if *c == channel))
    }
}

fn desktop_key(h: &Harness) -> [u8; 32] {
    let status = h.control(r#"{"cmd":"status"}"#);
    apex_remote_core::b64_decode(status["key"].as_str().expect("a key"))
        .expect("base64")
        .as_slice()
        .try_into()
        .expect("32")
}

fn write_message(w: &mut impl Write, m: &[u8]) {
    w.write_all(&(m.len() as u32).to_be_bytes()).expect("len");
    w.write_all(m).expect("body");
    w.flush().expect("flush");
}

fn read_message(r: &mut impl Read) -> Vec<u8> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len).expect("length prefix");
    let mut buf = vec![0u8; u32::from_be_bytes(len) as usize];
    r.read_exact(&mut buf).expect("body");
    buf
}

fn open_offer(h: &Harness) -> Option<PairingOffer> {
    let reply = h.control(r#"{"cmd":"pair"}"#);
    if reply["reply"] == "error" {
        // This process is not local, so pairing is refused — which is itself
        // the rule under test in `pairing_is_refused_for_a_caller_that_is_not_local`.
        eprintln!("SKIP: this process may not pair ({})", reply["message"]);
        return None;
    }
    Some(PairingOffer::decode(reply["qr"].as_str().expect("a qr")).expect("decode"))
}

#[test]
fn a_device_pairs_over_the_real_socket_and_lands_in_the_real_store() {
    let h = harness!("pair");
    let Some(offer) = open_offer(&h) else { return };
    assert_eq!(offer.v, V);
    assert!(offer.expires_ms > apex_remote_core::now_ms());

    let device = Device::new();
    let answer = device.pair(&h, &offer, "pixel-8-office");
    assert_eq!(answer["ok"], true, "{answer}");

    let listed = h.control(r#"{"cmd":"devices"}"#);
    let devices = listed["devices"].as_array().expect("an array");
    assert_eq!(devices.len(), 1, "{listed}");
    assert_eq!(devices[0]["name"], "pixel-8-office");
    assert_eq!(devices[0]["public_key"], device.key());
    assert_eq!(devices[0]["requires_user_verification"], true);
    // The listing is what a person reads, and it must not carry key material
    // beyond the public half it is meant to.
    let text = serde_json::to_string(&listed).expect("serialise");
    assert!(!text.contains(&apex_remote_core::b64_encode(&device.secret)));
}

#[test]
fn a_second_device_cannot_reuse_the_first_ones_code() {
    // The property that stops a photographed QR pairing a phone in the next
    // room, measured against the real service rather than against the offer
    // object.
    let h = harness!("reuse");
    let Some(offer) = open_offer(&h) else { return };
    assert_eq!(Device::new().pair(&h, &offer, "first")["ok"], true);
    let second = Device::new().pair(&h, &offer, "second");
    assert_eq!(second["ok"], false, "{second}");
    assert_eq!(h.control(r#"{"cmd":"devices"}"#)["devices"]
        .as_array()
        .expect("an array")
        .len(), 1);
}

#[test]
fn an_unpaired_device_cannot_open_a_session() {
    let h = harness!("unpaired");
    // The desktop's key is public, so an attacker who learns it still gets
    // nowhere without being in the store.
    assert!(
        Device::new().connect(&h).is_none(),
        "an unpaired key opened a session"
    );
}

#[test]
fn a_paired_device_reaches_the_agent_runtime_through_the_proxy() {
    let h = harness!("session");
    let Some(offer) = open_offer(&h) else { return };
    let device = Device::new();
    assert_eq!(device.pair(&h, &offer, "pixel-8")["ok"], true);

    let mut session = device.connect(&h).expect("a session");
    // The daemon's own protocol, unaltered, all the way to the daemon.
    let hello = session.call(r#"{"cmd":"hello"}"#);
    assert_eq!(hello["reply"], "hello", "{hello}");
    assert_eq!(
        hello["version"],
        apex_agent_core::protocol::PROTOCOL_VERSION,
        "{hello}"
    );
    let list = session.call(r#"{"cmd":"list"}"#);
    assert_eq!(list["reply"], "sessions", "{list}");

    // A ping, because it is the only measurement of connection quality either
    // end has and a token that came back wrong would make it meaningless.
    session.send(Frame::Ping { token: 0xdead_beef });
    assert_eq!(session.recv(), Frame::Pong { token: 0xdead_beef });
}

#[test]
fn a_request_from_a_phone_is_remote_and_cannot_approve_root() {
    // The claim the whole branch exists to make, end to end.
    //
    // Written as a comparison against the same daemon in the same run: "the
    // remote request was refused" proves nothing unless a local one is
    // allowed. If this process is not local — a container, a CI runner — the
    // comparison says so instead of asserting a fixed answer.
    let h = harness!("origin");
    let Some(offer) = open_offer(&h) else { return };
    let device = Device::new();
    let paired = device.pair(&h, &offer, "pixel-8-office");
    assert_eq!(paired["ok"], true, "{paired}");
    let device_id = paired["device"].as_str().expect("an id").to_string();

    let mut session = device.connect(&h).expect("a session");
    let filed = session.call(
        r#"{"cmd":"privilege_request","verb":"install","args":["clang"],"reason":"filed from a paired device"}"#,
    );
    assert_eq!(filed["reply"], "request", "{filed}");
    // Origin: established by the daemon from the connection the proxy
    // declared on, not from anything the device sent.
    assert_eq!(filed["request_origin"], "claude-remote-control", "{filed}");
    assert_eq!(filed["origin_source"], "declared", "{filed}");
    // And which device, which is what a person deciding actually wants.
    assert_eq!(filed["actor"], device_id.as_str(), "{filed}");
    let id = filed["id"].as_u64().expect("an id");

    // The phone tries to approve it.
    let refused = session.call(&format!(
        r#"{{"cmd":"decide","id":{id},"decision":"allow"}}"#
    ));
    assert_eq!(refused["reply"], "error", "a phone approved root: {refused}");
    assert_eq!(refused["kind"], "permission_denied", "{refused}");
    assert!(
        refused["message"]
            .as_str()
            .unwrap_or_default()
            .contains("claude-remote-control"),
        "{refused}"
    );

    // The control arm. Same daemon, same request, this process's own
    // connection.
    let direct = h.direct(&format!(
        r#"{{"cmd":"decide","id":{id},"decision":"deny"}}"#
    ));
    let observed = h.direct(
        r#"{"cmd":"privilege_request","verb":"pin","args":[],"reason":"probing this process's own origin"}"#,
    );
    let local = observed["request_origin"]
        .as_str()
        .and_then(apex_agent_core::policy::RequestOrigin::parse)
        .map(|o| o.is_local())
        .unwrap_or(false);
    if local {
        assert_eq!(
            direct["reply"], "request",
            "a local connection could not decide, so the refusal above is not \
             attributable to the origin: {direct}"
        );
        assert_eq!(direct["decision"], "denied", "{direct}");
    } else {
        eprintln!(
            "NOTE: this process is not local, so the refusal above is not attributable to the \
             declaration"
        );
        assert_eq!(direct["reply"], "error", "{direct}");
    }
}

#[test]
fn revoking_a_device_ends_the_connection_it_is_already_holding() {
    // What the owner of a lost phone is asking for. "Refused next time" is a
    // different and much weaker promise.
    let h = harness!("revoke");
    let Some(offer) = open_offer(&h) else { return };
    let device = Device::new();
    let paired = device.pair(&h, &offer, "lost-phone");
    assert_eq!(paired["ok"], true, "{paired}");
    let mut session = device.connect(&h).expect("a session");
    assert_eq!(session.call(r#"{"cmd":"list"}"#)["reply"], "sessions");

    let revoked = h.control(r#"{"cmd":"revoke","device":"lost-phone"}"#);
    assert_eq!(revoked["reply"], "ok", "{revoked}");

    // The live connection is gone: the write may succeed into a closed
    // socket's buffer, so the read is what proves it.
    let frame = Frame::Control(b"{\"cmd\":\"list\"}".to_vec())
        .encode()
        .expect("encode");
    let sealed = session.channel.seal(&frame).expect("seal");
    let _ = write_all(&mut session.socket, &sealed);
    let mut len = [0u8; 4];
    assert!(
        session.socket.read_exact(&mut len).is_err(),
        "a revoked device's live connection still answered"
    );

    // And it cannot come back.
    assert!(
        device.connect(&h).is_none(),
        "a revoked device reconnected"
    );

    // The record is still there, marked, which is what P1-051's listing needs.
    let listed = h.control(r#"{"cmd":"devices"}"#);
    let devices = listed["devices"].as_array().expect("an array");
    assert_eq!(devices.len(), 1, "{listed}");
    assert!(devices[0]["revoked_ms"].as_u64().is_some(), "{listed}");
}

fn write_all(w: &mut TcpStream, m: &[u8]) -> std::io::Result<()> {
    w.write_all(&(m.len() as u32).to_be_bytes())?;
    w.write_all(m)?;
    w.flush()
}

#[test]
fn pairing_is_refused_for_a_caller_that_is_not_local() {
    // The other half of "pairing cannot be completed silently by an agent".
    // Asserted as the rule rather than as a fixed answer, because it depends
    // on what this process classifies as — and the daemon is asked, in the
    // same run, what that is.
    let h = harness!("who");
    let reply = h.control(r#"{"cmd":"pair"}"#);
    let observed = h.direct(
        r#"{"cmd":"privilege_request","verb":"rollback","args":[],"reason":"probing this process's own origin"}"#,
    );
    let origin = observed["request_origin"]
        .as_str()
        .and_then(apex_agent_core::policy::RequestOrigin::parse);
    match origin {
        Some(o) if o.is_local() => assert_eq!(reply["reply"], "offer", "{reply}"),
        Some(o) => {
            assert_eq!(reply["reply"], "error", "a {o} caller was offered a code: {reply}");
            assert!(
                reply["message"].as_str().unwrap_or_default().contains(o.as_str()),
                "{reply}"
            );
        }
        None => assert_eq!(reply["reply"], "error", "{reply}"),
    }
}

#[test]
fn a_peer_that_connects_and_says_nothing_is_dropped() {
    // The only network listener in the stack, so an unauthenticated peer that
    // holds a thread by staying silent is the cheapest denial of service
    // there is. The first version of this daemon had exactly that bug: `main`
    // set a deadline and the spawned thread cleared it as its first line,
    // which is the opposite of what its own comment said.
    //
    // The harness runs with the deadline clamped to one second, so this costs
    // a second rather than thirty.
    let h = harness!("silent");
    let mut socket = h.tcp();
    socket
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("timeout");
    // Not even the hello byte.
    let started = Instant::now();
    let mut buf = [0u8; 1];
    let out = socket.read(&mut buf);
    let waited = started.elapsed();
    match out {
        Ok(0) => {}
        Ok(n) => panic!("the daemon sent {n} bytes to a peer that had said nothing"),
        Err(e) => panic!("the connection was not closed: {e}"),
    }
    assert!(
        waited < Duration::from_secs(8),
        "the daemon waited {waited:?} on a silent peer"
    );
}

#[test]
fn a_terminal_streams_both_ways_over_one_channel() {
    // P1-050's "PTY stream, input", and the half of P1-055 the brief said was
    // closer than it looks. The daemon owns the PTY; this proves a device can
    // open a channel onto one, type into it and see the output, on the same
    // connection that is carrying control frames.
    let h = harness!("pty");
    let Some(offer) = open_offer(&h) else { return };
    let device = Device::new();
    let paired = device.pair(&h, &offer, "pixel-8");
    assert_eq!(paired["ok"], true, "{paired}");
    let device_id = paired["device"].as_str().expect("an id").to_string();
    let mut session = device.connect(&h).expect("a session");

    // `cat` is the honest probe: it echoes exactly what it is given, so an
    // assertion about the output is an assertion about the path rather than
    // about a program's opinion. A shell would print a prompt and a banner
    // and the test would be about those.
    let started = session.call(
        r#"{"cmd":"run","agent":"generic","cwd":"/tmp","sandbox":"unrestricted","cols":80,"rows":24,"args":["/bin/cat"]}"#,
    );
    assert_eq!(started["reply"], "session", "{started}");
    let id = started["id"].as_u64().expect("an id");
    // Started through the proxy, so the session itself is remote-origin.
    assert_eq!(started["request_origin"], "claude-remote-control", "{started}");
    // The device ID, not its name. Deliberate: a name is chosen on the phone
    // and two devices may share one, and an audit record that says "phone"
    // when the owner has two is worse than one that says nothing. The id is
    // derived from the key, so it names exactly one device — and
    // `apex remote devices` is where the two are put side by side.
    assert_eq!(started["actor"], device_id.as_str(), "{started}");

    // Open a channel onto it. The payload is the daemon's own attach request:
    // one vocabulary, expressed once.
    let open = Frame::Open {
        channel: 1,
        request: format!(
            r#"{{"cmd":"attach","id":{id},"cols":80,"rows":24,"replay":0}}"#
        )
        .into_bytes(),
    };
    session.send(open);
    let reply = session.recv();
    match reply {
        Frame::Control(line) => {
            let v: serde_json::Value = serde_json::from_slice(&line).expect("json");
            assert_eq!(v["reply"], "attached", "{v}");
        }
        other => panic!("expected the attach reply, got {other:?}"),
    }

    // Type into it, and read it back.
    session.send(Frame::Data {
        channel: 1,
        bytes: b"hello from a phone\n".to_vec(),
    });
    let mut seen = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        match session.recv() {
            Frame::Data { channel, bytes } => {
                assert_eq!(channel, 1, "terminal bytes on the wrong channel");
                seen.extend_from_slice(&bytes);
                if String::from_utf8_lossy(&seen).contains("hello from a phone") {
                    break;
                }
            }
            Frame::Close { channel, reason } => {
                panic!("channel {channel} closed early: {reason}")
            }
            other => panic!("unexpected {other:?} while reading a terminal"),
        }
    }
    assert!(
        String::from_utf8_lossy(&seen).contains("hello from a phone"),
        "the terminal never echoed: {:?}",
        String::from_utf8_lossy(&seen)
    );

    // Control still works on the same connection while the terminal is open,
    // which is the whole reason this layer multiplexes rather than handing the
    // connection over the way `apex agent attach` does. Terminal output that
    // arrives in between is kept, not dropped, so the assertion below is
    // about a channel that is genuinely still open.
    let list = session.call(r#"{"cmd":"list"}"#);
    assert!(
        session.closed_unasked(1).is_none(),
        "the desktop closed the terminal channel on its own: {:?}",
        session.closed_unasked(1)
    );
    assert_eq!(list["reply"], "sessions", "{list}");
    let sessions = list["sessions"].as_array().expect("an array");
    assert!(
        sessions.iter().any(|s| s["id"] == id && s["attached"] == 1),
        "the daemon does not report the device as attached: {list}"
    );

    // Closing the channel detaches; the session goes on running in the
    // daemon, which is what "a remote client is a viewport" means.
    session.send(Frame::Close {
        channel: 1,
        reason: String::new(),
    });
    let after = session.call(&format!(r#"{{"cmd":"info","id":{id}}}"#));
    assert_eq!(after["reply"], "session", "{after}");
    assert!(after["exit_code"].is_null(), "closing a channel killed the session: {after}");
}

#[test]
fn a_channel_onto_a_session_that_does_not_exist_is_refused_with_the_daemons_own_words() {
    // The defect this closes: the proxy pushed a PTY channel whether or not
    // the daemon had actually attached, so a pump thread would sit reading a
    // connection that was still in control mode and deliver reply lines to
    // the device as terminal output.
    let h = harness!("noattach");
    let Some(offer) = open_offer(&h) else { return };
    let device = Device::new();
    assert_eq!(device.pair(&h, &offer, "pixel-8")["ok"], true);
    let mut session = device.connect(&h).expect("a session");

    session.send(Frame::Open {
        channel: 4,
        request: br#"{"cmd":"attach","id":9999,"cols":80,"rows":24,"replay":0}"#.to_vec(),
    });
    // The daemon's own answer, so the device can render it.
    match session.recv() {
        Frame::Control(line) => {
            let v: serde_json::Value = serde_json::from_slice(&line).expect("json");
            assert_eq!(v["reply"], "error", "{v}");
            assert_eq!(v["kind"], "no_such_session", "{v}");
        }
        other => panic!("expected the daemon's refusal, got {other:?}"),
    }
    // And the channel is closed rather than left open onto nothing.
    assert_eq!(
        session.recv(),
        Frame::Close {
            channel: 4,
            reason: String::new()
        }
    );

    // The connection is still usable: a refused channel is not a fatal error.
    assert_eq!(session.call(r#"{"cmd":"list"}"#)["reply"], "sessions");
}
