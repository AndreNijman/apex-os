//! The frames that travel between a paired device and this machine.
//!
//! ## Why there is a framing layer at all
//!
//! `apex-agentd`'s control protocol is one JSON object per line on a Unix
//! socket, and `Attach` turns the connection it arrived on into a raw PTY
//! pipe. That works because a local client can open as many connections as it
//! likes and they all cost nothing.
//!
//! A phone cannot. It has one channel — a TCP connection on the LAN, or one
//! outbound tunnel through a relay — and it needs to hold a terminal open on
//! it while still listing sessions, answering a prompt and resizing a window.
//! So this layer multiplexes: several logical channels over one encrypted
//! byte stream.
//!
//! ## Why the control payload is agentd's own protocol, verbatim
//!
//! [`Frame::Control`] carries one line of `apex-agentd`'s NDJSON. Not a
//! translation of it, not a subset: the bytes a local `apex` would have
//! written. Three things follow from that, and they are the whole reason
//! the design is shaped this way.
//!
//! * **Provider neutrality is inherited rather than built.** The daemon
//!   already launches Claude, OpenCode, Codex, Gemini and a generic managed
//!   PTY through `adapter.rs`, and already reports project, worktree,
//!   checkpoint, policy and origin in one `SessionInfo`. A remote protocol
//!   that re-described any of that would be a second vocabulary to keep in
//!   step with the first, and the second one always falls behind.
//! * **No screen scraping**, which P1-050 asks for in as many words. The
//!   phone gets the same typed records the Agent Center does.
//! * **A new daemon verb reaches the phone for free.** P1-020's agent graph
//!   is being added to that protocol on another branch; nothing here has to
//!   change to carry it.
//!
//! The cost is that this layer cannot check what it forwards, which is why
//! the daemon does the checking and why every proxied connection declares its
//! origin before it forwards anything. See `apex-remoted`.
//!
//! ## Why PTY bytes are not JSON
//!
//! [`Frame::Data`] is raw. A terminal emits arbitrary bytes at whatever rate
//! the program feels like, base64 costs a third more of them, and JSON
//! escaping costs more again on exactly the control sequences a TUI is made
//! of. On a mobile connection that is the difference between a responsive
//! editor and a slideshow.
//!
//! ## The shape
//!
//! ```text
//! [u8 tag][u32 channel, big-endian][payload …]
//! ```
//!
//! Big-endian because every other length on a wire is, and a reader written
//! from the description should get it right without checking. The frame's own
//! length is not in here: it belongs to the transport, which has to know it in
//! order to decrypt, and writing it twice is how the two disagree.

use std::fmt;

/// The largest payload one frame may carry.
///
/// Set by the transport rather than by taste: a Noise transport message is at
/// most 65535 bytes including the 16-byte authentication tag, and this leaves
/// room for the 5-byte header as well. A `Data` frame is split by the writer
/// and reassembled by the reader as an ordinary byte stream, so the limit is
/// invisible above this layer.
pub const MAX_PAYLOAD: usize = 65535 - 16 - 5;

/// Channel 0, which every [`Frame::Control`] uses.
///
/// Control frames are request/response on one logical channel and are
/// correlated by the `id` inside agentd's own JSON, not by a channel number.
/// Giving them a channel of their own anyway keeps the header uniform, and
/// makes "a control frame arrived on channel 4" a detectable protocol error
/// rather than an ambiguity.
pub const CONTROL_CHANNEL: u32 = 0;

/// One frame, decoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Frame {
    /// One line of `apex-agentd`'s control protocol, without its newline.
    ///
    /// In both directions: a `Request` from the device, a `Response` back.
    /// The newline is stripped because the framing already says where the
    /// payload ends, and a payload that carried its own terminator would let
    /// a client smuggle a second request into one frame.
    Control(Vec<u8>),
    /// Open a PTY channel onto a session.
    ///
    /// The payload is the `attach` request as agentd would receive it, so the
    /// session id, size and replay window are expressed once, in the daemon's
    /// vocabulary. What differs is the answer: the daemon turns the
    /// *connection* into the PTY, and here it turns one *channel* into it.
    Open { channel: u32, request: Vec<u8> },
    /// Terminal bytes, in either direction.
    Data { channel: u32, bytes: Vec<u8> },
    /// A channel is finished. `reason` is empty for an ordinary close.
    Close { channel: u32, reason: String },
    /// Keepalive, and the only measurement of connection quality either side
    /// has.
    ///
    /// P1-052 asks for the connection path and its quality to be visible at
    /// both ends. Round-trip time on a frame that crosses the same path as
    /// everything else is the honest measure of that; a ping to the relay's
    /// front door would report the health of a machine nobody is talking to.
    /// The `token` is echoed so a reply can be matched to its request rather
    /// than to whichever ping was most recent.
    Ping { token: u64 },
    Pong { token: u64 },
}

/// Why a frame could not be decoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WireError {
    /// Fewer bytes than a header.
    Short(usize),
    /// A tag this build does not know.
    ///
    /// Not skippable. A reader that ignored unknown frames would silently drop
    /// half of a protocol it was told it could speak, and the version
    /// handshake exists so the two ends agree on the vocabulary before either
    /// sends anything.
    UnknownTag(u8),
    /// A control frame on a channel other than [`CONTROL_CHANNEL`], or a
    /// data/open/close frame on it.
    WrongChannel { tag: u8, channel: u32 },
    /// A payload longer than [`MAX_PAYLOAD`].
    TooLong(usize),
    /// A frame whose payload is not the shape its tag requires.
    Malformed(&'static str),
}

impl fmt::Display for WireError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WireError::Short(n) => write!(f, "a frame needs at least {HEADER} bytes, got {n}"),
            WireError::UnknownTag(t) => write!(
                f,
                "frame tag {t} is not one this build speaks; the version handshake should have \
                 caught that before anything was sent"
            ),
            WireError::WrongChannel { tag, channel } => write!(
                f,
                "frame tag {tag} arrived on channel {channel}, which is not where it belongs"
            ),
            WireError::TooLong(n) => {
                write!(f, "a payload of {n} bytes exceeds the {MAX_PAYLOAD}-byte limit")
            }
            WireError::Malformed(why) => write!(f, "{why}"),
        }
    }
}

impl std::error::Error for WireError {}

/// Tag byte, header length: named so the encoder and the decoder cannot
/// disagree about either.
const TAG_CONTROL: u8 = 0;
const TAG_OPEN: u8 = 1;
const TAG_DATA: u8 = 2;
const TAG_CLOSE: u8 = 3;
const TAG_PING: u8 = 4;
const TAG_PONG: u8 = 5;
const HEADER: usize = 5;

impl Frame {
    /// This frame's tag byte.
    pub fn tag(&self) -> u8 {
        match self {
            Frame::Control(_) => TAG_CONTROL,
            Frame::Open { .. } => TAG_OPEN,
            Frame::Data { .. } => TAG_DATA,
            Frame::Close { .. } => TAG_CLOSE,
            Frame::Ping { .. } => TAG_PING,
            Frame::Pong { .. } => TAG_PONG,
        }
    }

    /// The channel this frame belongs to.
    pub fn channel(&self) -> u32 {
        match self {
            Frame::Control(_) | Frame::Ping { .. } | Frame::Pong { .. } => CONTROL_CHANNEL,
            Frame::Open { channel, .. } | Frame::Data { channel, .. } | Frame::Close { channel, .. } => {
                *channel
            }
        }
    }

    /// Encode, or say why it cannot be.
    ///
    /// Returns an error rather than truncating or splitting. A `Data` frame
    /// too long for one message is the writer's problem — it knows the stream
    /// is a stream and can split it anywhere — and silently splitting a
    /// `Control` frame would deliver half a JSON object.
    pub fn encode(&self) -> Result<Vec<u8>, WireError> {
        let payload: Vec<u8> = match self {
            Frame::Control(line) => {
                if line.contains(&b'\n') {
                    return Err(WireError::Malformed(
                        "a control payload carries one request and must not contain a newline",
                    ));
                }
                line.clone()
            }
            Frame::Open { request, .. } => request.clone(),
            Frame::Data { bytes, .. } => bytes.clone(),
            Frame::Close { reason, .. } => reason.as_bytes().to_vec(),
            Frame::Ping { token } | Frame::Pong { token } => token.to_be_bytes().to_vec(),
        };
        if payload.len() > MAX_PAYLOAD {
            return Err(WireError::TooLong(payload.len()));
        }
        let mut out = Vec::with_capacity(HEADER + payload.len());
        out.push(self.tag());
        out.extend_from_slice(&self.channel().to_be_bytes());
        out.extend_from_slice(&payload);
        Ok(out)
    }

    /// Decode one frame from a complete plaintext message.
    ///
    /// The transport hands over exactly one message, so there is no partial
    /// read to handle here and no length prefix to trust: the caller already
    /// knows how many bytes there are because it had to in order to decrypt
    /// them.
    pub fn decode(buf: &[u8]) -> Result<Frame, WireError> {
        if buf.len() < HEADER {
            return Err(WireError::Short(buf.len()));
        }
        let tag = buf[0];
        let channel = u32::from_be_bytes([buf[1], buf[2], buf[3], buf[4]]);
        let payload = &buf[HEADER..];
        if payload.len() > MAX_PAYLOAD {
            return Err(WireError::TooLong(payload.len()));
        }
        // Channel discipline, checked on the way in. A `Data` frame on the
        // control channel would be terminal bytes delivered to the request
        // parser, and a `Control` frame on a PTY channel would be a request
        // nothing answers. Both are protocol errors, and treating them as
        // such is cheaper than working out later which end is confused.
        let on_control = channel == CONTROL_CHANNEL;
        match tag {
            TAG_CONTROL | TAG_PING | TAG_PONG if !on_control => {
                Err(WireError::WrongChannel { tag, channel })
            }
            TAG_OPEN | TAG_DATA | TAG_CLOSE if on_control => {
                Err(WireError::WrongChannel { tag, channel })
            }
            TAG_CONTROL => {
                if payload.contains(&b'\n') {
                    return Err(WireError::Malformed(
                        "a control payload carries one request and must not contain a newline",
                    ));
                }
                Ok(Frame::Control(payload.to_vec()))
            }
            TAG_OPEN => Ok(Frame::Open {
                channel,
                request: payload.to_vec(),
            }),
            TAG_DATA => Ok(Frame::Data {
                channel,
                bytes: payload.to_vec(),
            }),
            TAG_CLOSE => Ok(Frame::Close {
                channel,
                reason: String::from_utf8_lossy(payload).into_owned(),
            }),
            TAG_PING | TAG_PONG => {
                let token = payload
                    .try_into()
                    .map(u64::from_be_bytes)
                    .map_err(|_| WireError::Malformed("a ping carries an eight-byte token"))?;
                Ok(if tag == TAG_PING {
                    Frame::Ping { token }
                } else {
                    Frame::Pong { token }
                })
            }
            other => Err(WireError::UnknownTag(other)),
        }
    }

    /// Split a byte run into as many `Data` frames as it needs.
    ///
    /// The one place the size limit is allowed to matter. Callers push
    /// whatever a PTY read produced and get frames that will encode.
    pub fn data_frames(channel: u32, bytes: &[u8]) -> Vec<Frame> {
        if bytes.is_empty() {
            return Vec::new();
        }
        bytes
            .chunks(MAX_PAYLOAD)
            .map(|c| Frame::Data {
                channel,
                bytes: c.to_vec(),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn samples() -> Vec<Frame> {
        vec![
            Frame::Control(br#"{"cmd":"list"}"#.to_vec()),
            Frame::Control(Vec::new()),
            Frame::Open {
                channel: 1,
                request: br#"{"cmd":"attach","id":4,"cols":80,"rows":24,"replay":0}"#.to_vec(),
            },
            Frame::Data {
                channel: 7,
                bytes: vec![0x1b, b'[', b'2', b'K', 0x00, 0xff, b'\n'],
            },
            Frame::Close {
                channel: 7,
                reason: String::new(),
            },
            Frame::Close {
                channel: 9,
                reason: "session exited".into(),
            },
            Frame::Ping { token: 0 },
            Frame::Pong { token: u64::MAX },
        ]
    }

    #[test]
    fn every_frame_round_trips() {
        for f in samples() {
            let bytes = f.encode().unwrap_or_else(|e| panic!("{f:?}: {e}"));
            let back = Frame::decode(&bytes).unwrap_or_else(|e| panic!("{f:?}: {e}"));
            assert_eq!(back, f);
        }
    }

    #[test]
    fn a_data_frame_is_transparent_to_arbitrary_bytes() {
        // The property a terminal needs and the reason this is not JSON:
        // every byte value survives, including NUL, 0x1b and invalid UTF-8.
        let all: Vec<u8> = (0..=255u8).collect();
        let f = Frame::Data {
            channel: 3,
            bytes: all.clone(),
        };
        let back = Frame::decode(&f.encode().expect("encode")).expect("decode");
        assert_eq!(back, Frame::Data { channel: 3, bytes: all });
    }

    #[test]
    fn a_control_frame_cannot_carry_two_requests() {
        // The framing above this one is line-based, so a payload containing a
        // newline is a second request the far end would parse and act on.
        // Refused in both directions, because a proxy that only checked on the
        // way out would still forward one that arrived.
        let sneaky = br#"{"cmd":"list"}
{"cmd":"decide","id":1,"decision":"allow"}"#
            .to_vec();
        assert_eq!(
            Frame::Control(sneaky.clone()).encode(),
            Err(WireError::Malformed(
                "a control payload carries one request and must not contain a newline"
            ))
        );
        // And hand-built on the wire, bypassing the encoder entirely.
        let mut raw = vec![TAG_CONTROL, 0, 0, 0, 0];
        raw.extend_from_slice(&sneaky);
        assert!(matches!(Frame::decode(&raw), Err(WireError::Malformed(_))));
    }

    #[test]
    fn channel_discipline_is_enforced_on_the_way_in() {
        // A control frame claiming a PTY channel, and terminal bytes claiming
        // the control channel. Both are hand-built: the encoder cannot produce
        // either, which is exactly why the decoder has to check.
        let mut control_off_channel = vec![TAG_CONTROL, 0, 0, 0, 9];
        control_off_channel.extend_from_slice(br#"{"cmd":"list"}"#);
        assert_eq!(
            Frame::decode(&control_off_channel),
            Err(WireError::WrongChannel {
                tag: TAG_CONTROL,
                channel: 9
            })
        );

        let data_on_control = vec![TAG_DATA, 0, 0, 0, 0, b'x'];
        assert_eq!(
            Frame::decode(&data_on_control),
            Err(WireError::WrongChannel {
                tag: TAG_DATA,
                channel: 0
            })
        );

        for tag in [TAG_OPEN, TAG_CLOSE] {
            assert!(
                matches!(
                    Frame::decode(&[tag, 0, 0, 0, 0]),
                    Err(WireError::WrongChannel { .. })
                ),
                "tag {tag} was accepted on the control channel"
            );
        }
        for tag in [TAG_PING, TAG_PONG] {
            let mut off = vec![tag, 0, 0, 0, 1];
            off.extend_from_slice(&7u64.to_be_bytes());
            assert!(
                matches!(Frame::decode(&off), Err(WireError::WrongChannel { .. })),
                "tag {tag} was accepted off the control channel"
            );
        }
    }

    #[test]
    fn an_unknown_tag_is_an_error_and_not_a_skip() {
        for tag in 6..=255u8 {
            assert_eq!(
                Frame::decode(&[tag, 0, 0, 0, 0]),
                Err(WireError::UnknownTag(tag))
            );
        }
    }

    #[test]
    fn a_truncated_frame_is_short_rather_than_a_panic() {
        for n in 0..HEADER {
            assert_eq!(Frame::decode(&vec![0u8; n]), Err(WireError::Short(n)));
        }
    }

    #[test]
    fn a_ping_with_the_wrong_payload_length_is_refused() {
        // The token is fixed width, so anything else is either a truncated
        // frame or a client that has misread the protocol. Both are errors,
        // and `try_into` is what makes the second one impossible to mistake
        // for a valid token of zero.
        for len in [0usize, 1, 7, 9, 16] {
            let mut raw = vec![TAG_PING, 0, 0, 0, 0];
            raw.extend(std::iter::repeat_n(0u8, len));
            let out = Frame::decode(&raw);
            if len == 8 {
                assert_eq!(out, Ok(Frame::Ping { token: 0 }));
            } else {
                assert!(matches!(out, Err(WireError::Malformed(_))), "{len}: {out:?}");
            }
        }
    }

    #[test]
    fn an_over_long_payload_is_refused_rather_than_truncated() {
        let f = Frame::Data {
            channel: 1,
            bytes: vec![0u8; MAX_PAYLOAD + 1],
        };
        assert_eq!(f.encode(), Err(WireError::TooLong(MAX_PAYLOAD + 1)));
        // At the limit it encodes, and the encoded frame is exactly what a
        // Noise transport message can hold.
        let ok = Frame::Data {
            channel: 1,
            bytes: vec![0u8; MAX_PAYLOAD],
        };
        let bytes = ok.encode().expect("at the limit");
        assert_eq!(bytes.len(), MAX_PAYLOAD + HEADER);
        assert!(bytes.len() + 16 <= 65535, "a Noise message cannot hold it");
    }

    #[test]
    fn splitting_produces_frames_that_all_encode_and_rejoin() {
        let payload: Vec<u8> = (0..MAX_PAYLOAD * 2 + 13).map(|i| (i % 251) as u8).collect();
        let frames = Frame::data_frames(5, &payload);
        assert_eq!(frames.len(), 3);
        let mut rejoined = Vec::new();
        for f in &frames {
            let back = Frame::decode(&f.encode().expect("encode")).expect("decode");
            match back {
                Frame::Data { channel, bytes } => {
                    assert_eq!(channel, 5);
                    rejoined.extend_from_slice(&bytes);
                }
                other => panic!("{other:?}"),
            }
        }
        assert_eq!(rejoined, payload);
        assert!(Frame::data_frames(5, &[]).is_empty());
    }
}
