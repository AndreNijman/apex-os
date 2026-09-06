//! One device connection, from the first handshake byte to the last frame.
//!
//! Two shapes arrive on the same listener and are told apart by the first
//! thing the device says: a **pairing** connection, which uses `Noise_NK` and
//! the one-time token, and a **session** connection, which uses `Noise_IK`
//! and an already-paired key. The device announces which in one plaintext
//! byte before the handshake — the only plaintext this protocol has after the
//! length prefix, and it says nothing an observer could not infer from the
//! message sizes anyway.
//!
//! ## What a connection may do, in order
//!
//! 1. Handshake. A session handshake yields the device's static key, which is
//!    looked up in the store. An unknown or revoked key ends the connection
//!    here, before a single frame is read.
//! 2. Frames. Control frames are round-tripped through `apex-agentd` on a
//!    connection that declared its origin first; `Open` starts a PTY channel;
//!    `Data` goes to the PTY it names.
//!
//! There is no step where the device says who it is. It cannot: the identity
//! is what the handshake proved, and every proxied request carries it as the
//! `actor` the daemon records.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::{Arc, Mutex};

use apex_remote_core::noise::{Channel, Handshake};
use apex_remote_core::pairing::{PairingRequest, PairingError};
use apex_remote_core::wire::Frame;

use crate::state::{Live, State};

/// The device's first byte: which handshake follows.
///
/// A byte rather than a negotiation. Both values lead to a Noise handshake
/// that either completes or does not, so an observer flipping it produces a
/// failed connection rather than a downgrade — there is no weaker option to
/// be steered towards.
pub const HELLO_PAIR: u8 = b'P';
pub const HELLO_SESSION: u8 = b'S';

/// Why a device connection ended.
#[derive(Debug)]
pub enum ServeError {
    Io(std::io::Error),
    Handshake(String),
    /// The handshake completed and the key is not one this machine accepts.
    ///
    /// Deliberately the same message for "never paired" and "revoked": a
    /// device that can tell the difference has an oracle for which keys this
    /// machine has ever seen.
    NotPaired,
    Protocol(String),
}

impl std::fmt::Display for ServeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ServeError::Io(e) => write!(f, "{e}"),
            ServeError::Handshake(w) => write!(f, "the handshake did not complete: {w}"),
            ServeError::NotPaired => write!(f, "that device is not paired with this machine"),
            ServeError::Protocol(w) => write!(f, "{w}"),
        }
    }
}

impl From<std::io::Error> for ServeError {
    fn from(e: std::io::Error) -> ServeError {
        ServeError::Io(e)
    }
}

/// Handle one accepted TCP connection.
pub fn connection(mut socket: TcpStream, state: Arc<State>, agentd: std::path::PathBuf) {
    let peer = socket.peer_addr().ok();
    let mut hello = [0u8; 1];
    if socket.read_exact(&mut hello).is_err() {
        return;
    }
    let outcome = match hello[0] {
        HELLO_PAIR => pair(&mut socket, &state),
        HELLO_SESSION => session(socket.try_clone().ok(), socket, &state, &agentd),
        other => Err(ServeError::Protocol(format!(
            "a connection opened with {other:#04x}, which is neither a pairing nor a session"
        ))),
    };
    if let Err(e) = outcome {
        // One line, on stderr, where journald keeps it. Never the key, never
        // the token, never a frame's contents: this log is read by whoever
        // can read the unit's journal, which on a shared machine is more
        // people than can read the device store.
        eprintln!(
            "apex-remoted: connection from {} ended: {e}",
            peer.map(|p| p.ip().to_string())
                .unwrap_or_else(|| "an unknown address".into())
        );
    }
}

/// A pairing connection: `Noise_NK`, then the token.
fn pair(socket: &mut TcpStream, state: &State) -> Result<(), ServeError> {
    let secret = crate::secret_of(state);
    let mut hs = Handshake::pairing_responder(&secret, apex_remote_core::REMOTE_PROTOCOL_VERSION)
        .map_err(|e| ServeError::Handshake(e.to_string()))?;
    let m1 = crate::net::read_message(socket)?;
    let payload = hs
        .read(&m1)
        .map_err(|e| ServeError::Handshake(e.to_string()))?;
    let request: PairingRequest = serde_json::from_slice(&payload)
        .map_err(|e| ServeError::Protocol(format!("the pairing request is malformed: {e}")))?;

    let now = apex_remote_core::now_ms();
    let result = complete_pairing(state, &request, now);

    // The answer goes back inside the finished handshake either way, so a
    // refusal is as confidential as an acceptance. A plaintext "no" would
    // tell anybody watching that this machine is not currently pairing.
    let answer = match &result {
        Ok(device) => serde_json::json!({"ok": true, "device": device.id, "machine": state.machine}),
        Err(e) => serde_json::json!({"ok": false, "error": e.to_string()}),
    };
    let m2 = hs
        .write(answer.to_string().as_bytes())
        .map_err(|e| ServeError::Handshake(e.to_string()))?;
    crate::net::write_message(socket, &m2)?;
    result.map(|_| ()).map_err(|e| ServeError::Protocol(e.to_string()))
}

/// The store side of pairing, so the socket side above stays readable.
fn complete_pairing(
    state: &State,
    request: &PairingRequest,
    now: u64,
) -> Result<apex_remote_core::device::Device, PairingError> {
    let mut offer = state.offer.lock().expect("offer lock");
    let Some(offer) = offer.as_mut() else {
        // No offer open. The same error a wrong token gets, so a device
        // cannot use the difference to find out whether the owner is
        // currently looking at a QR code.
        return Err(PairingError::BadToken);
    };
    let mut store = state
        .devices()
        .map_err(|e| PairingError::BadDevice(e.to_string()))?;
    let device = apex_remote_core::pairing::complete(offer, request, &mut store, now)?;
    state
        .save_devices(&store)
        .map_err(|e| PairingError::BadDevice(format!("the device was not written: {e}")))?;
    Ok(device)
}

/// A session connection: `Noise_IK`, then frames.
fn session(
    reader_socket: Option<TcpStream>,
    mut socket: TcpStream,
    state: &State,
    agentd: &std::path::Path,
) -> Result<(), ServeError> {
    let secret = crate::secret_of(state);
    let mut hs = Handshake::session_responder(&secret, apex_remote_core::REMOTE_PROTOCOL_VERSION)
        .map_err(|e| ServeError::Handshake(e.to_string()))?;
    let m1 = crate::net::read_message(&mut socket)?;
    hs.read(&m1)
        .map_err(|e| ServeError::Handshake(e.to_string()))?;
    let device_key = hs
        .remote_static()
        .ok_or_else(|| ServeError::Handshake("the device sent no static key".into()))?;

    // Identified, then authorised, in that order and both before a frame is
    // read. The lookup is on the full key: an id is a prefix and prefixes are
    // how two keys become one device.
    let key_text = apex_remote_core::b64_encode(&device_key);
    let (device_id, device_name) = {
        let store = state
            .devices()
            .map_err(|e| ServeError::Protocol(e.to_string()))?;
        let Some(d) = store.authenticate(&key_text) else {
            // The handshake is deliberately NOT completed for an unpaired
            // key. Completing it and then refusing would confirm to the
            // caller that this machine's key is what they think it is, which
            // is exactly what a scanner sweeping for APEX machines wants.
            return Err(ServeError::NotPaired);
        };
        (d.id.clone(), d.name.clone())
    };

    let m2 = hs
        .write(state.machine.as_bytes())
        .map_err(|e| ServeError::Handshake(e.to_string()))?;
    crate::net::write_message(&mut socket, &m2)?;
    let channel = hs
        .into_transport()
        .map_err(|e| ServeError::Handshake(e.to_string()))?;

    {
        let mut store = state
            .devices()
            .map_err(|e| ServeError::Protocol(e.to_string()))?;
        store.seen(&device_id, apex_remote_core::now_ms(), "lan");
        let _ = state.save_devices(&store);
    }
    if let Some(s) = socket.try_clone().ok() {
        state.register(Live {
            device_id: device_id.clone(),
            socket: s,
        });
    }

    let result = frames(
        reader_socket.unwrap_or(socket.try_clone()?),
        socket,
        channel,
        state,
        agentd,
        &device_id,
        &device_name,
    );
    state.unregister(&device_id, None);
    result
}

/// The frame loop.
///
/// One writer, several readers: PTY channels each get a thread that reads the
/// daemon's Unix socket and pushes `Data` frames into a queue. Everything
/// that goes out of this connection goes through that one queue, because the
/// Noise channel's nonce is a counter and two threads sealing concurrently
/// would produce two messages claiming the same one.
fn frames(
    mut inbound: TcpStream,
    outbound: TcpStream,
    channel: Channel,
    state: &State,
    agentd: &std::path::Path,
    device_id: &str,
    device_name: &str,
) -> Result<(), ServeError> {
    let sealer = Arc::new(Mutex::new(Sealer {
        channel,
        socket: outbound,
    }));
    let mut ptys: Vec<PtyChannel> = Vec::new();
    let _ = device_name;

    loop {
        let message = match crate::net::read_message(&mut inbound) {
            Ok(m) => m,
            // An ordinary close, or a revoked device's socket shut down under
            // it. Both end the loop and neither is an error worth a log line.
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(()),
            Err(e) => return Err(ServeError::Io(e)),
        };
        let plaintext = {
            let mut s = sealer.lock().expect("sealer lock");
            s.channel
                .open(&message)
                .map_err(|e| ServeError::Protocol(e.to_string()))?
        };
        let frame = Frame::decode(&plaintext).map_err(|e| ServeError::Protocol(e.to_string()))?;
        match frame {
            Frame::Ping { token } => send(&sealer, Frame::Pong { token })?,
            // A desktop never receives one it did not ask for; answering it
            // would let a device measure this machine's clock for free.
            Frame::Pong { .. } => {}
            Frame::Control(line) => {
                let reply = control(agentd, device_id, &line);
                send(&sealer, Frame::Control(reply))?;
            }
            Frame::Open { channel, request } => {
                match PtyChannel::open(agentd, device_id, channel, &request, &sealer) {
                    Ok((pty, reply, buffered)) => {
                        send(&sealer, Frame::Control(reply))?;
                        for f in Frame::data_frames(channel, &buffered) {
                            send(&sealer, f)?;
                        }
                        ptys.push(pty);
                    }
                    Err(e) => send(
                        &sealer,
                        Frame::Close {
                            channel,
                            reason: e.to_string(),
                        },
                    )?,
                }
            }
            Frame::Data { channel, bytes } => {
                if let Some(p) = ptys.iter_mut().find(|p| p.channel == channel) {
                    let _ = p.write(&bytes);
                }
            }
            Frame::Close { channel, .. } => {
                ptys.retain(|p| p.channel != channel);
            }
        }
        let _ = state;
    }
}

/// The one place a frame leaves this connection.
fn send(sealer: &Arc<Mutex<Sealer>>, frame: Frame) -> Result<(), ServeError> {
    let bytes = frame
        .encode()
        .map_err(|e| ServeError::Protocol(e.to_string()))?;
    let mut s = sealer.lock().expect("sealer lock");
    let sealed = s
        .channel
        .seal(&bytes)
        .map_err(|e| ServeError::Protocol(e.to_string()))?;
    crate::net::write_message(&mut s.socket, &sealed)?;
    Ok(())
}

/// The encrypting half of the connection, and the socket it writes to.
///
/// One mutex over both, on purpose. Sealing increments a nonce counter and
/// writing has to happen in the same order the sealing did; two locks would
/// let a thread seal message 5, lose the socket lock, and have message 6 go
/// out first — which the far end would refuse, correctly, and the connection
/// would die for a reason nothing explains.
struct Sealer {
    channel: Channel,
    socket: TcpStream,
}

/// Round-trip one control frame through the daemon.
///
/// Failures come back as a `Response::Error` the device can render, in the
/// daemon's own vocabulary, rather than as a dropped connection: a phone that
/// loses its terminal because the runtime was restarted is a worse experience
/// than one that is told the runtime was restarted.
fn control(agentd: &std::path::Path, device_id: &str, line: &[u8]) -> Vec<u8> {
    // An `attach` on the control channel would leave the daemon connection
    // half in PTY mode with this side still expecting reply lines, and the
    // device's terminal bytes would go nowhere. Refused with the verb that
    // does work, rather than by wedging.
    if crate::proxy::is_attach(line) {
        return serde_json::json!({
            "reply": "error",
            "kind": "bad_request",
            "message": "attach does not travel on the control channel; open a channel for it",
        })
        .to_string()
        .into_bytes();
    }
    match crate::proxy::Agentd::open(agentd, device_id).and_then(|mut a| a.round_trip(line)) {
        Ok(reply) => reply,
        Err(e) => serde_json::json!({
            "reply": "error",
            "kind": "internal",
            "message": e.to_string(),
        })
        .to_string()
        .into_bytes(),
    }
}

/// A PTY the device has open.
struct PtyChannel {
    channel: u32,
    stream: std::os::unix::net::UnixStream,
    _pump: std::thread::JoinHandle<()>,
}

impl PtyChannel {
    fn open(
        agentd: &std::path::Path,
        device_id: &str,
        channel: u32,
        request: &[u8],
        sealer: &Arc<Mutex<Sealer>>,
    ) -> Result<(PtyChannel, Vec<u8>, Vec<u8>), crate::proxy::ProxyError> {
        let (stream, reply, buffered) =
            crate::proxy::Agentd::open(agentd, device_id)?.attach(request)?;
        let mut read_half = stream.try_clone()?;
        let sealer = Arc::clone(sealer);
        let pump = std::thread::spawn(move || {
            let mut buf = [0u8; 16 * 1024];
            loop {
                match read_half.read(&mut buf) {
                    Ok(0) | Err(_) => {
                        let _ = send(
                            &sealer,
                            Frame::Close {
                                channel,
                                reason: String::new(),
                            },
                        );
                        return;
                    }
                    Ok(n) => {
                        for f in Frame::data_frames(channel, &buf[..n]) {
                            if send(&sealer, f).is_err() {
                                return;
                            }
                        }
                    }
                }
            }
        });
        Ok((
            PtyChannel {
                channel,
                stream,
                _pump: pump,
            },
            reply,
            buffered,
        ))
    }

    fn write(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        self.stream.write_all(bytes)?;
        self.stream.flush()
    }
}

impl Drop for PtyChannel {
    fn drop(&mut self) {
        // Closing the daemon connection detaches; the session goes on running
        // in the daemon, which is the whole point of a viewport.
        let _ = self.stream.shutdown(std::net::Shutdown::Both);
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_two_hello_bytes_are_distinct_and_printable() {
        // Printable so a packet capture or a hexdump during a support call
        // reads as `P` or `S` rather than as a control byte someone has to
        // look up.
        assert_ne!(HELLO_PAIR, HELLO_SESSION);
        assert!(HELLO_PAIR.is_ascii_graphic() && HELLO_SESSION.is_ascii_graphic());
    }

    #[test]
    fn an_unpaired_and_a_revoked_device_are_refused_with_the_same_words() {
        // A device that could tell them apart would have an oracle for which
        // keys this machine has ever seen.
        let unpaired = ServeError::NotPaired.to_string();
        assert!(!unpaired.contains("revoked"), "{unpaired}");
        assert!(!unpaired.contains("never"), "{unpaired}");
    }

    #[test]
    fn an_attach_on_the_control_channel_is_refused_rather_than_wedging_the_proxy() {
        // It would leave the daemon connection half in PTY mode while this
        // side still read reply lines, and the terminal would go nowhere.
        // Refused without a daemon being involved at all, which is why the
        // socket path here is nonsense and the test still passes.
        let reply = control(
            std::path::Path::new("/nonexistent/agentd.sock"),
            "pixel-8",
            br#"{"cmd":"attach","id":1,"cols":80,"rows":24}"#,
        );
        let v: serde_json::Value = serde_json::from_slice(&reply).expect("a JSON reply");
        assert_eq!(v["kind"], "bad_request", "{v}");
        assert!(
            v["message"].as_str().unwrap_or_default().contains("open a channel"),
            "{v}"
        );
    }

    #[test]
    fn a_control_failure_comes_back_as_a_response_the_device_can_render() {
        // Not a dropped connection. A phone that loses its terminal because
        // the runtime restarted is worse off than one that is told so.
        let reply = control(
            std::path::Path::new("/nonexistent/agentd.sock"),
            "pixel-8",
            br#"{"cmd":"list"}"#,
        );
        let v: serde_json::Value = serde_json::from_slice(&reply).expect("a JSON reply");
        assert_eq!(v["reply"], "error");
        assert!(
            v["message"].as_str().unwrap_or_default().contains("apex agent enable"),
            "{v}"
        );
        // And it is one line, so it cannot desynchronise the frame it rides in.
        assert!(!reply.contains(&b'\n'));
    }
}
