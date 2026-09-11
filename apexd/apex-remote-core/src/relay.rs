//! Dialling a relay: the endpoint, the roles, and a WebSocket client.
//!
//! [`crate::rendezvous`] says what a relay is *for* and what its operator
//! learns. This module is how a process actually reaches one.
//!
//! ## Why WebSocket, and why hand-rolled
//!
//! The relay has to be reachable from a machine behind NAT with no inbound
//! port, which means both ends dial out and the meeting point copies bytes.
//! On Cloudflare that is a Worker plus a Durable Object, and the only
//! long-lived bidirectional transport a Worker offers is a WebSocket. So the
//! relay leg is a WebSocket carrying, as binary payloads, exactly the byte
//! stream the LAN leg carries over TCP: `[u32 big-endian length][Noise
//! ciphertext]`, framed by `apex-remoted`'s `net` module. Nothing above the
//! transport knows which leg it is on, which is what makes "the relay carries
//! ciphertext it cannot read" a property of the code rather than a promise.
//!
//! Hand-rolled for the same reason `apex-secretd`'s Cloudflare provider
//! hand-rolls its HTTP: nothing in this workspace links a WebSocket stack, and
//! the client half of RFC 6455 that this needs — one upgrade request, masked
//! binary frames out, unmasked frames in, close and ping — is a few hundred
//! lines with an exhaustively specified test vector. A crate would bring a
//! runtime with it.
//!
//! ## What is deliberately not here
//!
//! **TLS.** `apex-secretd/src/providers/cloudflare/api.rs` already settled how
//! this workspace reaches a TLS endpoint — it shells out to `curl`, because
//! nothing here speaks TLS — and a WebSocket cannot be carried over a one-shot
//! `curl`. So everything below is generic over `Read`/`Write`: the caller
//! supplies the stream. [`Endpoint::secure`] records whether the URL asked for
//! TLS, and it is the connector's business, not this module's, to refuse or
//! satisfy it. See `ROADMAP/design/P1-052-relay.md` for what deploying behind
//! `wss://` would take.
//!
//! **Server framing.** A relay double lives in this crate's tests and a real
//! relay is the Worker under `relay/`. Neither needs the server half of the
//! handshake in shipped code, so [`accept_for`] — the one piece both sides
//! compute — is public and the rest is not.

use std::io::{Read, Write};

use data_encoding::BASE64;
use sha1::{Digest, Sha1};

/// The constant RFC 6455 §1.3 appends to the client's key before hashing.
///
/// Its only job is to make the accept value impossible to produce by
/// accident, so that a server which merely echoes bytes cannot be mistaken
/// for one that speaks the protocol.
pub const GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

/// The largest single frame this client will reassemble.
///
/// The stream being carried is chunked by whoever is copying it, so a frame
/// size is a relay's choice rather than a protocol constant. A quarter of a
/// megabyte is four times the largest Noise message that can exist and still
/// bounds what one announced length can make this process allocate.
pub const MAX_FRAME: usize = 256 * 1024;

/// Which end of a rendezvous a connection is.
///
/// Explicit, and sent in the query string, because a relay that simply paired
/// the first two arrivals would happily join two phones to each other. They
/// would then discover the mistake only when the Noise handshake failed,
/// which is a confusing way to learn that a desktop is not running.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// The desktop. Waits; there is at most one of these per rendezvous id.
    Host,
    /// A paired device. Arrives, and is joined to the waiting host.
    Guest,
}

impl Role {
    pub fn as_str(&self) -> &'static str {
        match self {
            Role::Host => "host",
            Role::Guest => "guest",
        }
    }

    pub fn parse(text: &str) -> Option<Role> {
        match text {
            "host" => Some(Role::Host),
            "guest" => Some(Role::Guest),
            _ => None,
        }
    }

    /// The other end of the same rendezvous.
    pub fn peer(&self) -> Role {
        match self {
            Role::Host => Role::Guest,
            Role::Guest => Role::Host,
        }
    }
}

impl std::fmt::Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.pad(self.as_str())
    }
}

/// Everything that can go wrong before or during a relay connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RelayError {
    /// The configured relay URL is not one this client can dial.
    Endpoint(String),
    /// The server answered the upgrade with something other than a switch.
    Upgrade(String),
    /// A frame arrived that RFC 6455 does not permit a server to send.
    Protocol(&'static str),
    /// A frame announced more than [`MAX_FRAME`] bytes.
    TooLong(usize),
}

impl std::fmt::Display for RelayError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RelayError::Endpoint(why) => write!(f, "this relay address cannot be used: {why}"),
            RelayError::Upgrade(why) => write!(f, "the relay refused the connection: {why}"),
            RelayError::Protocol(why) => write!(f, "the relay broke the protocol: {why}"),
            RelayError::TooLong(n) => {
                write!(f, "the relay announced a {n}-byte frame; the limit is {MAX_FRAME}")
            }
        }
    }
}

impl std::error::Error for RelayError {}

/// A relay's address, as configured by `apex-remoted --relay`.
///
/// Parsed once at startup so that a typo is a refusal to start rather than a
/// connection attempt every few seconds for the life of the machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    /// Whether the URL asked for TLS. Not acted on here — see the module note.
    pub secure: bool,
    pub host: String,
    pub port: u16,
    /// Any path the URL carried, without a trailing slash. Usually empty.
    pub prefix: String,
}

impl Endpoint {
    /// Parse a relay base URL.
    ///
    /// `https`/`wss` and `http`/`ws` are both accepted and mean the same
    /// thing, because the address a person has in front of them is the
    /// Worker's `https://` URL and asking them to rewrite the scheme is a
    /// step that exists only to be got wrong.
    pub fn parse(base: &str) -> Result<Endpoint, RelayError> {
        let base = base.trim();
        let (scheme, rest) = base
            .split_once("://")
            .ok_or_else(|| RelayError::Endpoint(format!("{base:?} has no scheme")))?;
        let secure = match scheme.to_ascii_lowercase().as_str() {
            "wss" | "https" => true,
            "ws" | "http" => false,
            other => {
                return Err(RelayError::Endpoint(format!(
                    "{other:?} is not a scheme this client speaks; use wss, ws, https or http"
                )))
            }
        };

        // A query or a fragment on the base would be silently dropped when the
        // role is appended, and a relay URL that does not mean what it says is
        // worse than one that is refused.
        for bad in ['?', '#'] {
            if rest.contains(bad) {
                return Err(RelayError::Endpoint(format!(
                    "a relay address may not carry {bad:?}; the client appends its own query"
                )));
            }
        }
        // Credentials in a URL end up in logs and in the QR code.
        if rest.split('/').next().unwrap_or("").contains('@') {
            return Err(RelayError::Endpoint(
                "a relay address may not carry credentials".into(),
            ));
        }

        let (authority, path) = match rest.find('/') {
            Some(i) => (&rest[..i], &rest[i..]),
            None => (rest, ""),
        };
        if authority.is_empty() {
            return Err(RelayError::Endpoint(format!("{base:?} names no host")));
        }

        // Split host from port, allowing a bracketed IPv6 literal.
        let (host, port) = if let Some(close) = authority.strip_prefix('[') {
            let (inside, after) = close
                .split_once(']')
                .ok_or_else(|| RelayError::Endpoint(format!("{authority:?} is an unclosed IPv6 literal")))?;
            (inside.to_string(), after.strip_prefix(':').map(str::to_string))
        } else {
            match authority.rsplit_once(':') {
                Some((h, p)) => (h.to_string(), Some(p.to_string())),
                None => (authority.to_string(), None),
            }
        };
        if host.is_empty() {
            return Err(RelayError::Endpoint(format!("{base:?} names no host")));
        }
        let port = match port {
            Some(text) => text
                .parse::<u16>()
                .map_err(|_| RelayError::Endpoint(format!("{text:?} is not a port")))
                .and_then(|p| {
                    if p == 0 {
                        Err(RelayError::Endpoint("port 0 is not a relay".into()))
                    } else {
                        Ok(p)
                    }
                })?,
            None if secure => 443,
            None => 80,
        };

        let prefix = path.trim_end_matches('/').to_string();
        Ok(Endpoint { secure, host, port, prefix })
    }

    /// The `Host:` header value: the port is omitted when it is the default.
    pub fn authority(&self) -> String {
        let host = if self.host.contains(':') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };
        let default = if self.secure { 443 } else { 80 };
        if self.port == default {
            host
        } else {
            format!("{host}:{}", self.port)
        }
    }

    /// The request target for one end of one rendezvous.
    ///
    /// The rendezvous id is URL-safe unpadded base64 by construction
    /// ([`crate::B64`]), so it needs no escaping here — and a value that would
    /// have needed it is a value this client did not derive.
    pub fn request_target(&self, rendezvous: &str, role: Role) -> String {
        format!("{}/r/{rendezvous}?role={}", self.prefix, role.as_str())
    }
}

impl std::fmt::Display for Endpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let scheme = if self.secure { "wss" } else { "ws" };
        write!(f, "{scheme}://{}{}", self.authority(), self.prefix)
    }
}

/// The value a server must return for a given `Sec-WebSocket-Key`.
///
/// Public because the relay double in this crate's tests and the Worker both
/// compute it, and a second implementation of the same four lines is a second
/// place for it to be wrong.
pub fn accept_for(key: &str) -> String {
    let mut h = Sha1::new();
    h.update(key.as_bytes());
    h.update(GUID.as_bytes());
    // Standard alphabet with padding — RFC 6455's, not this crate's URL-safe
    // one. A URL-safe accept value is simply the wrong string.
    BASE64.encode(&h.finalize())
}

/// The client half of the opening handshake.
pub struct Opening {
    key: String,
}

impl Opening {
    /// Start a handshake with a caller-supplied nonce.
    ///
    /// The nonce is taken rather than generated so a test can pin the RFC's
    /// own vector. [`Opening::new`] is what production uses.
    pub fn with_nonce(nonce: [u8; 16]) -> Opening {
        Opening { key: BASE64.encode(&nonce) }
    }

    /// Start a handshake with a fresh random nonce.
    ///
    /// RFC 6455 §4.1 requires it to be unpredictable. It is not a secret and
    /// it authenticates nothing: its whole job is to make a cached or
    /// replayed 101 response detectable.
    pub fn new() -> Opening {
        let mut nonce = [0u8; 16];
        rand::RngCore::fill_bytes(&mut rand::rng(), &mut nonce);
        Opening::with_nonce(nonce)
    }

    pub fn key(&self) -> &str {
        &self.key
    }

    /// The bytes of the upgrade request.
    pub fn request(&self, endpoint: &Endpoint, rendezvous: &str, role: Role) -> Vec<u8> {
        let target = endpoint.request_target(rendezvous, role);
        format!(
            "GET {target} HTTP/1.1\r\n\
             Host: {}\r\n\
             Upgrade: websocket\r\n\
             Connection: Upgrade\r\n\
             Sec-WebSocket-Key: {}\r\n\
             Sec-WebSocket-Version: 13\r\n\
             \r\n",
            endpoint.authority(),
            self.key,
        )
        .into_bytes()
    }

    /// Read and check the server's answer, consuming exactly the response
    /// head and not one byte of what follows it.
    ///
    /// Byte-at-a-time on purpose: the first frame may arrive in the same
    /// packet as the response, and a buffered read would swallow it. This runs
    /// once per connection, so the syscalls do not matter.
    pub fn accept(self, from: &mut impl Read) -> Result<(), RelayError> {
        let mut head = Vec::new();
        let mut byte = [0u8; 1];
        loop {
            match from.read(&mut byte) {
                Ok(0) => {
                    return Err(RelayError::Upgrade(
                        "it closed the connection without answering".into(),
                    ))
                }
                Ok(_) => head.push(byte[0]),
                Err(e) => return Err(RelayError::Upgrade(format!("{e}"))),
            }
            if head.ends_with(b"\r\n\r\n") {
                break;
            }
            // A response head that never ends is a way to make this process
            // grow without ever completing a connection.
            if head.len() > 16 * 1024 {
                return Err(RelayError::Upgrade(
                    "its response head never ended".into(),
                ));
            }
        }
        self.check(&head)
    }

    /// The check itself, over a complete response head.
    pub fn check(&self, head: &[u8]) -> Result<(), RelayError> {
        let text = String::from_utf8_lossy(head);
        let mut lines = text.split("\r\n");
        let status = lines.next().unwrap_or("");
        let code = status.split_whitespace().nth(1).unwrap_or("");
        if code != "101" {
            // The relay's own status is the useful part of the message: a 404
            // means the path is wrong, a 409 that someone else holds this
            // rendezvous, a 401 that the deployment wants a credential.
            return Err(RelayError::Upgrade(format!(
                "expected HTTP 101, got {:?}",
                status.trim()
            )));
        }

        let mut upgrade = None;
        let mut connection = None;
        let mut accept = None;
        for line in lines {
            let Some((name, value)) = line.split_once(':') else { continue };
            let value = value.trim();
            match name.trim().to_ascii_lowercase().as_str() {
                "upgrade" => upgrade = Some(value.to_ascii_lowercase()),
                "connection" => connection = Some(value.to_ascii_lowercase()),
                "sec-websocket-accept" => accept = Some(value.to_string()),
                _ => {}
            }
        }
        if upgrade.as_deref() != Some("websocket") {
            return Err(RelayError::Upgrade(
                "its 101 did not upgrade to websocket".into(),
            ));
        }
        // Comma-separated by RFC 7230, so a contains rather than an equality.
        if !connection.unwrap_or_default().contains("upgrade") {
            return Err(RelayError::Upgrade(
                "its 101 did not name the upgrade in Connection".into(),
            ));
        }
        let want = accept_for(&self.key);
        match accept {
            Some(got) if got == want => Ok(()),
            Some(_) => Err(RelayError::Upgrade(
                "its Sec-WebSocket-Accept does not match the key this client sent".into(),
            )),
            None => Err(RelayError::Upgrade(
                "its 101 carried no Sec-WebSocket-Accept".into(),
            )),
        }
    }
}

impl Default for Opening {
    fn default() -> Opening {
        Opening::new()
    }
}

/// The frame opcodes this client handles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Continuation,
    Text,
    Binary,
    Close,
    Ping,
    Pong,
}

impl Op {
    fn from_code(code: u8) -> Option<Op> {
        match code {
            0x0 => Some(Op::Continuation),
            0x1 => Some(Op::Text),
            0x2 => Some(Op::Binary),
            0x8 => Some(Op::Close),
            0x9 => Some(Op::Ping),
            0xa => Some(Op::Pong),
            _ => None,
        }
    }

    fn code(&self) -> u8 {
        match self {
            Op::Continuation => 0x0,
            Op::Text => 0x1,
            Op::Binary => 0x2,
            Op::Close => 0x8,
            Op::Ping => 0x9,
            Op::Pong => 0xa,
        }
    }

    fn is_control(&self) -> bool {
        matches!(self, Op::Close | Op::Ping | Op::Pong)
    }
}

/// One complete message, after any fragments have been joined.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub op: Op,
    pub payload: Vec<u8>,
}

/// Encode one frame as a client sends it: always masked, per RFC 6455 §5.3.
///
/// The mask is taken rather than generated so a test can assert the exact
/// bytes on the wire. It is not a security measure — the payload is already
/// ciphertext by the time it gets here — it exists so that a client cannot be
/// tricked into making a proxy see an HTTP request in the payload.
pub fn encode_client(op: Op, payload: &[u8], mask: [u8; 4]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 14);
    out.push(0x80 | op.code()); // FIN, never fragmented on the way out
    let n = payload.len();
    if n < 126 {
        out.push(0x80 | n as u8);
    } else if n <= u16::MAX as usize {
        out.push(0x80 | 126);
        out.extend_from_slice(&(n as u16).to_be_bytes());
    } else {
        out.push(0x80 | 127);
        out.extend_from_slice(&(n as u64).to_be_bytes());
    }
    out.extend_from_slice(&mask);
    out.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
    out
}

/// A fresh mask for one frame.
fn fresh_mask() -> [u8; 4] {
    let mut mask = [0u8; 4];
    rand::RngCore::fill_bytes(&mut rand::rng(), &mut mask);
    mask
}

/// The sending half of a relay connection.
///
/// Separate from the reading half so the two can live on different threads
/// with no lock between a read and a write — which is what the splice in
/// `apex-remoted` needs, since one thread pumps each direction.
pub struct Sender<W: Write> {
    out: W,
}

impl<W: Write> Sender<W> {
    pub fn new(out: W) -> Sender<W> {
        Sender { out }
    }

    /// Send one chunk of the carried byte stream.
    pub fn binary(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        self.frame(Op::Binary, bytes)
    }

    pub fn pong(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        self.frame(Op::Pong, bytes)
    }

    pub fn ping(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        self.frame(Op::Ping, bytes)
    }

    /// A clean close: status 1000, "normal closure".
    pub fn close(&mut self) -> std::io::Result<()> {
        self.frame(Op::Close, &1000u16.to_be_bytes())
    }

    fn frame(&mut self, op: Op, bytes: &[u8]) -> std::io::Result<()> {
        self.out.write_all(&encode_client(op, bytes, fresh_mask()))?;
        self.out.flush()
    }

    pub fn into_inner(self) -> W {
        self.out
    }
}

/// The reading half of a relay connection.
pub struct Receiver<R: Read> {
    inn: R,
    /// Bytes of a message whose fragments have not all arrived.
    partial: Vec<u8>,
    /// The opcode the current fragment sequence started with.
    started: Option<Op>,
}

impl<R: Read> Receiver<R> {
    pub fn new(inn: R) -> Receiver<R> {
        Receiver { inn, partial: Vec::new(), started: None }
    }

    /// Read until one complete message is available.
    ///
    /// Control frames are returned as they arrive — they may be interleaved
    /// inside a fragmented message and must not be buffered into it.
    pub fn message(&mut self) -> Result<Message, RelayError> {
        loop {
            let (fin, op, payload) = self.frame()?;
            if op.is_control() {
                // RFC 6455 §5.5: a control frame is never fragmented and
                // carries at most 125 bytes.
                if !fin {
                    return Err(RelayError::Protocol("a fragmented control frame"));
                }
                if payload.len() > 125 {
                    return Err(RelayError::Protocol("an oversized control frame"));
                }
                return Ok(Message { op, payload });
            }

            match (op, self.started) {
                (Op::Continuation, None) => {
                    return Err(RelayError::Protocol("a continuation with nothing to continue"))
                }
                (Op::Continuation, Some(_)) => self.partial.extend_from_slice(&payload),
                (_, Some(_)) => {
                    return Err(RelayError::Protocol("a new message inside an unfinished one"))
                }
                (op, None) => {
                    if fin {
                        return Ok(Message { op, payload });
                    }
                    self.started = Some(op);
                    self.partial = payload;
                }
            }
            if self.partial.len() > MAX_FRAME {
                return Err(RelayError::TooLong(self.partial.len()));
            }
            if fin {
                let op = self.started.take().unwrap_or(Op::Binary);
                return Ok(Message { op, payload: std::mem::take(&mut self.partial) });
            }
        }
    }

    /// One frame off the wire, header and all.
    fn frame(&mut self) -> Result<(bool, Op, Vec<u8>), RelayError> {
        let mut head = [0u8; 2];
        self.read_exact(&mut head)?;
        let fin = head[0] & 0x80 != 0;
        if head[0] & 0x70 != 0 {
            // No extension was negotiated, so a reserved bit set means the
            // far end is speaking a protocol this client did not agree to.
            return Err(RelayError::Protocol("a reserved bit is set"));
        }
        let op = Op::from_code(head[0] & 0x0f)
            .ok_or(RelayError::Protocol("an opcode this client does not know"))?;

        // RFC 6455 §5.1: a server must not mask. A masked frame from a server
        // is refused rather than unmasked, because accepting it would mean
        // this client cannot tell a relay from a client.
        if head[1] & 0x80 != 0 {
            return Err(RelayError::Protocol("a masked frame from the server"));
        }
        let len = match head[1] & 0x7f {
            126 => {
                let mut n = [0u8; 2];
                self.read_exact(&mut n)?;
                u16::from_be_bytes(n) as usize
            }
            127 => {
                let mut n = [0u8; 8];
                self.read_exact(&mut n)?;
                let n = u64::from_be_bytes(n);
                if n > MAX_FRAME as u64 {
                    return Err(RelayError::TooLong(n as usize));
                }
                n as usize
            }
            short => short as usize,
        };
        if len > MAX_FRAME {
            return Err(RelayError::TooLong(len));
        }
        // Allocated only after the length has been checked.
        let mut payload = vec![0u8; len];
        self.read_exact(&mut payload)?;
        Ok((fin, op, payload))
    }

    fn read_exact(&mut self, into: &mut [u8]) -> Result<(), RelayError> {
        self.inn
            .read_exact(into)
            .map_err(|e| RelayError::Upgrade(format!("{e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── the handshake ────────────────────────────────────────────────────

    #[test]
    fn the_accept_value_is_the_rfc_s_own() {
        // RFC 6455 §1.3, verbatim. The whole handshake is worthless if this
        // is computed with the wrong hash, the wrong alphabet or the wrong
        // constant, and each of those mistakes passes a round-trip test
        // against an implementation that makes the same one.
        assert_eq!(accept_for("dGhlIHNhbXBsZSBub25jZQ=="), "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
    }

    #[test]
    fn the_accept_value_is_padded_standard_base64_not_this_crate_s_alphabet() {
        // The crate's own b64 is URL-safe and unpadded, which is right for a
        // QR code and wrong here. A handshake computed with it fails against
        // every real server and against none of this crate's other tests.
        let accept = accept_for("dGhlIHNhbXBsZSBub25jZQ==");
        assert!(accept.ends_with('='), "padding was stripped: {accept}");
        assert_ne!(accept, crate::b64_encode(&BASE64.decode(accept.as_bytes()).unwrap()));
    }

    #[test]
    fn the_request_names_the_rendezvous_the_role_and_the_host() {
        let e = Endpoint::parse("https://relay.example.com").expect("parse");
        let o = Opening::with_nonce([0u8; 16]);
        let text = String::from_utf8(o.request(&e, "abc123", Role::Host)).expect("utf8");
        assert!(text.starts_with("GET /r/abc123?role=host HTTP/1.1\r\n"), "{text}");
        assert!(text.contains("\r\nHost: relay.example.com\r\n"), "{text}");
        assert!(text.contains("\r\nUpgrade: websocket\r\n"), "{text}");
        assert!(text.contains("\r\nConnection: Upgrade\r\n"), "{text}");
        assert!(text.contains("\r\nSec-WebSocket-Version: 13\r\n"), "{text}");
        assert!(text.contains(&format!("\r\nSec-WebSocket-Key: {}\r\n", o.key())), "{text}");
        assert!(text.ends_with("\r\n\r\n"), "the request head does not end");
    }

    #[test]
    fn a_guest_asks_for_the_other_role() {
        let e = Endpoint::parse("wss://r.example").expect("parse");
        assert_eq!(e.request_target("id", Role::Guest), "/r/id?role=guest");
        assert_eq!(Role::Host.peer(), Role::Guest);
        assert_eq!(Role::Guest.peer(), Role::Host);
        assert_eq!(Role::parse("host"), Some(Role::Host));
        assert_eq!(Role::parse("Host"), None, "the role is not case-folded");
        assert_eq!(Role::parse(""), None);
    }

    #[test]
    fn a_101_is_accepted_only_when_the_accept_value_matches() {
        let o = Opening::with_nonce([3u8; 16]);
        let good = format!(
            "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\n\
             Connection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n\r\n",
            accept_for(o.key())
        );
        assert_eq!(o.check(good.as_bytes()), Ok(()));

        // The same response with somebody else's accept value: this is the
        // check that stops a relay replaying one 101 to every client.
        let other = accept_for("c29tZWJvZHkgZWxzZSEhIQ==");
        let replayed = format!(
            "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\n\
             Connection: Upgrade\r\nSec-WebSocket-Accept: {other}\r\n\r\n"
        );
        assert!(matches!(o.check(replayed.as_bytes()), Err(RelayError::Upgrade(_))));
    }

    #[test]
    fn anything_that_is_not_a_101_upgrade_is_refused_with_the_relay_s_own_words() {
        let o = Opening::with_nonce([1u8; 16]);
        let accept = accept_for(o.key());

        let conflict = "HTTP/1.1 409 Conflict\r\nContent-Length: 0\r\n\r\n";
        let Err(RelayError::Upgrade(why)) = o.check(conflict.as_bytes()) else {
            panic!("a 409 was accepted");
        };
        // The status is carried through, because "409" is what tells an
        // operator that something else already holds this rendezvous.
        assert!(why.contains("409"), "{why}");

        let no_upgrade = format!(
            "HTTP/1.1 101 Switching Protocols\r\nConnection: Upgrade\r\n\
             Sec-WebSocket-Accept: {accept}\r\n\r\n"
        );
        assert!(o.check(no_upgrade.as_bytes()).is_err(), "a 101 with no Upgrade was accepted");

        let no_connection = format!(
            "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\n\
             Sec-WebSocket-Accept: {accept}\r\n\r\n"
        );
        assert!(o.check(no_connection.as_bytes()).is_err(), "a 101 with no Connection was accepted");

        let no_accept = "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\n\
             Connection: Upgrade\r\n\r\n";
        assert!(o.check(no_accept.as_bytes()).is_err(), "a 101 with no accept value was accepted");
    }

    #[test]
    fn header_names_and_values_are_matched_the_way_http_defines_them() {
        // A server is free to send `upgrade: WebSocket` and
        // `Connection: keep-alive, Upgrade`. Both are correct HTTP and both
        // break a client that compares strings exactly.
        let o = Opening::with_nonce([9u8; 16]);
        let head = format!(
            "HTTP/1.1 101 Switching Protocols\r\nupgrade: WebSocket\r\n\
             Connection: keep-alive, Upgrade\r\nSec-WebSocket-Accept: {}\r\n\r\n",
            accept_for(o.key())
        );
        assert_eq!(o.check(head.as_bytes()), Ok(()));
    }

    #[test]
    fn the_handshake_reader_stops_at_the_blank_line_and_leaves_the_first_frame() {
        // The failure this prevents: a buffered reader swallows the first
        // frame with the response head, and the connection hangs waiting for
        // bytes that have already arrived.
        let o = Opening::with_nonce([5u8; 16]);
        let mut wire = format!(
            "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\n\
             Connection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n\r\n",
            accept_for(o.key())
        )
        .into_bytes();
        wire.extend_from_slice(&[0x82, 0x03, b'h', b'i', b'!']);

        let mut cursor = std::io::Cursor::new(wire);
        o.accept(&mut cursor).expect("handshake");
        let mut rx = Receiver::new(cursor);
        assert_eq!(
            rx.message().expect("frame"),
            Message { op: Op::Binary, payload: b"hi!".to_vec() }
        );
    }

    // ── the endpoint ─────────────────────────────────────────────────────

    #[test]
    fn a_worker_url_is_dialled_without_being_rewritten_by_hand() {
        let e = Endpoint::parse("https://apex-relay.andre.workers.dev").expect("parse");
        assert_eq!(e, Endpoint {
            secure: true,
            host: "apex-relay.andre.workers.dev".into(),
            port: 443,
            prefix: String::new(),
        });
        assert_eq!(e.authority(), "apex-relay.andre.workers.dev");
        assert_eq!(e.to_string(), "wss://apex-relay.andre.workers.dev");
    }

    #[test]
    fn the_default_port_follows_the_scheme_and_is_left_out_of_the_host_header() {
        assert_eq!(Endpoint::parse("ws://r.example").expect("ws").port, 80);
        assert_eq!(Endpoint::parse("wss://r.example").expect("wss").port, 443);
        assert_eq!(Endpoint::parse("http://r.example").expect("http").port, 80);
        assert_eq!(Endpoint::parse("https://r.example").expect("https").port, 443);
        // An explicit non-default port has to appear in Host, or a virtual
        // host on that port answers for the wrong site.
        assert_eq!(Endpoint::parse("ws://r.example:8787").expect("p").authority(), "r.example:8787");
        // An explicit default port does not.
        assert_eq!(Endpoint::parse("ws://r.example:80").expect("p").authority(), "r.example");
    }

    #[test]
    fn a_path_prefix_is_kept_and_a_trailing_slash_is_not() {
        let e = Endpoint::parse("https://example.com/apex/").expect("parse");
        assert_eq!(e.prefix, "/apex");
        assert_eq!(e.request_target("xy", Role::Host), "/apex/r/xy?role=host");
    }

    #[test]
    fn an_ipv6_literal_keeps_its_brackets_in_the_host_header_and_not_in_the_host() {
        let e = Endpoint::parse("ws://[2001:db8::1]:8787").expect("parse");
        assert_eq!(e.host, "2001:db8::1");
        assert_eq!(e.port, 8787);
        assert_eq!(e.authority(), "[2001:db8::1]:8787");
    }

    #[test]
    fn a_relay_address_that_cannot_mean_what_it_says_is_refused_at_startup() {
        for bad in [
            "relay.example.com",              // no scheme
            "ftp://relay.example.com",        // not a scheme this speaks
            "https://",                       // no host
            "https://:443",                   // no host, port only
            "https://r.example:0",            // port 0
            "https://r.example:https",        // not a number
            "https://r.example/?role=guest",  // a query the client would drop
            "https://r.example/#x",           // a fragment the client would drop
            "https://user:pw@r.example",      // credentials that end up in logs
        ] {
            assert!(
                matches!(Endpoint::parse(bad), Err(RelayError::Endpoint(_))),
                "{bad:?} was accepted"
            );
        }
    }

    // ── the frame codec ──────────────────────────────────────────────────

    #[test]
    fn a_client_frame_is_masked_and_says_so() {
        let out = encode_client(Op::Binary, b"abcde", [0x01, 0x02, 0x03, 0x04]);
        assert_eq!(out[0], 0x82, "FIN and the binary opcode");
        assert_eq!(out[1], 0x80 | 5, "the mask bit and the short length");
        assert_eq!(&out[2..6], &[0x01, 0x02, 0x03, 0x04]);
        assert_eq!(&out[6..], &[b'a' ^ 1, b'b' ^ 2, b'c' ^ 3, b'd' ^ 4, b'e' ^ 1]);
    }

    #[test]
    fn the_mask_is_not_the_same_twice() {
        // A fixed mask is the mistake that makes the payload recoverable by
        // anyone who can guess four bytes of it, and it passes every
        // round-trip test.
        let mut seen = std::collections::HashSet::new();
        for _ in 0..64 {
            let framed = {
                let mut sink = Vec::new();
                Sender::new(&mut sink).binary(b"the same payload every time").expect("send");
                sink
            };
            seen.insert(framed[2..6].to_vec());
        }
        assert!(seen.len() > 32, "only {} distinct masks in 64 frames", seen.len());
    }

    #[test]
    fn the_three_length_encodings_are_each_used_at_their_boundary() {
        let short = encode_client(Op::Binary, &vec![0u8; 125], [0; 4]);
        assert_eq!(short[1] & 0x7f, 125, "125 bytes must use the short form");
        let medium = encode_client(Op::Binary, &vec![0u8; 126], [0; 4]);
        assert_eq!(medium[1] & 0x7f, 126, "126 bytes must use the 16-bit form");
        assert_eq!(&medium[2..4], &126u16.to_be_bytes());
        let long = encode_client(Op::Binary, &vec![0u8; 65536], [0; 4]);
        assert_eq!(long[1] & 0x7f, 127, "65536 bytes must use the 64-bit form");
        assert_eq!(&long[2..10], &65536u64.to_be_bytes());
    }

    /// Turn client frames back into server frames: same bytes, mask removed.
    ///
    /// The codec's two halves are a client encoder and a server decoder, so a
    /// naive round-trip would prove nothing. This is the one place the two
    /// are bridged, and it is the unmasking a real relay does.
    fn as_from_server(client_frame: &[u8]) -> Vec<u8> {
        let mut out = client_frame.to_vec();
        let short = out[1] & 0x7f;
        let head = 2 + match short {
            126 => 2,
            127 => 8,
            _ => 0,
        };
        let mask: [u8; 4] = out[head..head + 4].try_into().expect("mask");
        let body: Vec<u8> = out[head + 4..]
            .iter()
            .enumerate()
            .map(|(i, b)| b ^ mask[i % 4])
            .collect();
        out.truncate(head);
        out[1] &= 0x7f; // the server does not mask
        out.extend_from_slice(&body);
        out
    }

    #[test]
    fn a_payload_survives_the_wire_at_every_length_form() {
        for n in [0usize, 1, 125, 126, 127, 65535, 65536, 70000] {
            let payload: Vec<u8> = (0..n).map(|i| (i % 251) as u8).collect();
            let wire = as_from_server(&encode_client(Op::Binary, &payload, [0x5a; 4]));
            let mut rx = Receiver::new(std::io::Cursor::new(wire));
            let got = rx.message().expect("message");
            assert_eq!(got.op, Op::Binary);
            assert_eq!(got.payload.len(), n, "length {n} did not survive");
            assert_eq!(got.payload, payload, "payload of {n} bytes did not survive");
        }
    }

    #[test]
    fn a_fragmented_message_is_rejoined_in_order() {
        // A relay is free to split a message, and the byte stream carried
        // inside is order-sensitive: a reassembly that dropped or reordered a
        // fragment would corrupt a Noise message and kill the session with a
        // decryption failure that names nothing.
        let mut wire = Vec::new();
        wire.extend_from_slice(&[0x02, 0x03]); // binary, not final
        wire.extend_from_slice(b"one");
        wire.extend_from_slice(&[0x00, 0x03]); // continuation, not final
        wire.extend_from_slice(b"two");
        wire.extend_from_slice(&[0x80, 0x05]); // continuation, final
        wire.extend_from_slice(b"three");

        let mut rx = Receiver::new(std::io::Cursor::new(wire));
        assert_eq!(
            rx.message().expect("message"),
            Message { op: Op::Binary, payload: b"onetwothree".to_vec() }
        );
    }

    #[test]
    fn a_control_frame_inside_a_fragmented_message_is_delivered_not_swallowed() {
        // RFC 6455 §5.4 allows this, and a client that buffered the ping into
        // the message would both corrupt the message and stop answering
        // keepalives — a connection that dies after exactly one long write.
        let mut wire = Vec::new();
        wire.extend_from_slice(&[0x02, 0x03]);
        wire.extend_from_slice(b"one");
        wire.extend_from_slice(&[0x89, 0x02]); // ping, final
        wire.extend_from_slice(b"pi");
        wire.extend_from_slice(&[0x80, 0x03]);
        wire.extend_from_slice(b"two");

        let mut rx = Receiver::new(std::io::Cursor::new(wire));
        assert_eq!(rx.message().expect("ping"), Message { op: Op::Ping, payload: b"pi".to_vec() });
        assert_eq!(
            rx.message().expect("message"),
            Message { op: Op::Binary, payload: b"onetwo".to_vec() }
        );
    }

    #[test]
    fn a_frame_a_server_may_not_send_is_refused() {
        let cases: Vec<(&str, Vec<u8>, RelayError)> = vec![
            (
                "a masked frame",
                vec![0x82, 0x81, 1, 2, 3, 4, b'x' ^ 1],
                RelayError::Protocol("a masked frame from the server"),
            ),
            (
                "a reserved bit",
                vec![0xc2, 0x00],
                RelayError::Protocol("a reserved bit is set"),
            ),
            (
                "an unknown opcode",
                vec![0x83, 0x00],
                RelayError::Protocol("an opcode this client does not know"),
            ),
            (
                "a fragmented control frame",
                vec![0x09, 0x00],
                RelayError::Protocol("a fragmented control frame"),
            ),
            (
                "a continuation with nothing to continue",
                vec![0x80, 0x00],
                RelayError::Protocol("a continuation with nothing to continue"),
            ),
        ];
        for (what, wire, want) in cases {
            let mut rx = Receiver::new(std::io::Cursor::new(wire));
            assert_eq!(rx.message(), Err(want), "{what} was accepted");
        }
    }

    #[test]
    fn an_oversized_frame_is_refused_before_a_byte_is_allocated() {
        // The header claims four gigabytes and the connection carries none of
        // them. A client that allocated first would be killed by a two-byte
        // write from the relay.
        let mut wire = vec![0x82, 127];
        wire.extend_from_slice(&(4u64 << 30).to_be_bytes());
        let mut rx = Receiver::new(std::io::Cursor::new(wire));
        assert!(matches!(rx.message(), Err(RelayError::TooLong(_))));

        // And the same through the 16-bit form is under the cap, so it is the
        // 64-bit form alone that this guard has to catch.
        let mut over = vec![0x82, 127];
        over.extend_from_slice(&((MAX_FRAME + 1) as u64).to_be_bytes());
        let mut rx = Receiver::new(std::io::Cursor::new(over));
        assert_eq!(rx.message(), Err(RelayError::TooLong(MAX_FRAME + 1)));
    }

    #[test]
    fn a_message_fragmented_past_the_cap_is_refused_too() {
        // Each fragment is legal; the joined message is not. A cap applied
        // only per frame is no cap at all against a hostile relay.
        let chunk = vec![7u8; 60 * 1024];
        let mut wire = Vec::new();
        wire.extend_from_slice(&[0x02, 126]);
        wire.extend_from_slice(&(chunk.len() as u16).to_be_bytes());
        wire.extend_from_slice(&chunk);
        for _ in 0..5 {
            wire.extend_from_slice(&[0x00, 126]);
            wire.extend_from_slice(&(chunk.len() as u16).to_be_bytes());
            wire.extend_from_slice(&chunk);
        }
        let mut rx = Receiver::new(std::io::Cursor::new(wire));
        assert!(matches!(rx.message(), Err(RelayError::TooLong(_))));
    }

    #[test]
    fn a_close_is_sent_with_a_status_a_relay_can_log() {
        let mut sink = Vec::new();
        Sender::new(&mut sink).close().expect("close");
        let wire = as_from_server(&sink);
        let mut rx = Receiver::new(std::io::Cursor::new(wire));
        let got = rx.message().expect("close");
        assert_eq!(got.op, Op::Close);
        assert_eq!(u16::from_be_bytes(got.payload[..2].try_into().unwrap()), 1000);
    }
}
