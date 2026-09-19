//! The credential a browser capsule spends and never holds (P2-012, route B).
//!
//! `docs/browser-capsule-auth.md` asked the owner one question — may the agent
//! runtime read the plaintext of a capsule's connection to the one destination
//! it was pinned to, in order to add a credential the capsule is never given —
//! and the answer was yes. This module is the half of that answer that runs as
//! root.
//!
//! ## Why the injection is here and not in `apex-agentd`
//!
//! Because of P0-002, which `broker.rs` states as a property rather than an
//! intention: **no verb in `apex_secret_core::protocol` returns a credential**,
//! and `apex-agentd` runs as the user, so anything it holds an unconfined
//! session of that user can read. The question the owner answered was about the
//! runtime reading *plaintext*; it was not a decision to give the runtime a
//! credential, and this design is arranged so that it does not have to be. The
//! runtime terminates TLS toward the capsule and becomes a byte pump; the
//! bytes cross to this daemon over the control socket; this daemon — which
//! already holds the value, already decides where it may be spent, and already
//! sees the plaintext of every `apex secret use` — is the only process that
//! ever puts the credential on a request.
//!
//! ```text
//!   capsule ──TLS(minted leaf)──▶ apex-agentd ──plaintext──▶ apex-secretd ──TLS──▶ site
//!                                 (no credential)            (adds the header)
//! ```
//!
//! ## What this relay is, deliberately, not
//!
//! It is not an HTTP proxy. One request per connection: the head is read once,
//! rewritten once, and everything after it — request body and the whole
//! response — is carried opaquely. `Connection: close` is forced onto the
//! request so the far side cannot keep the connection alive and send a second
//! request this module would then have to parse. A browser meets a closed
//! connection by opening another one, which arrives here as another `Present`
//! with the grant checked again.
//!
//! That is the whole parsing surface, and it is small on purpose: this is a
//! root process reading bytes that a capsule's browser chose.
//!
//! ## The response direction is scrubbed
//!
//! The capsule must not end up holding the credential, and the one way it could
//! is a site that echoes the header back. The response stream is scanned for
//! the value and any occurrence is replaced with `x` of the same length, so the
//! framing the site declared still holds. It catches the ordinary case — a
//! debug endpoint, an error page quoting the request — and cannot catch a site
//! that transforms the value first. That limit is real and is recorded in
//! `docs/browser-capsule-auth.md`: a credential spent at a site is a credential
//! that site has.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::os::unix::net::UnixStream;
use std::time::Duration;

use apex_secret_core::capability::CapabilityRecord;
use apex_secret_core::protocol::{ErrorKind, Response};
use apex_secret_core::store::ServiceInfo;
use apex_secret_core::SecretValue;

use crate::peer::Peer;
use crate::service::Service;

/// Hand one connection over to an authenticated relay.
///
/// The connection stops being a control channel at the [`Response::Ok`] below,
/// which is why this is reached from `main`'s accept loop rather than from
/// `dispatch`: everything after that reply is the capsule's own bytes, and a
/// loop that went back to reading request lines would read the first of them
/// as JSON.
///
/// The reply is written AFTER the site has been reached, so "the grant says no"
/// and "the site would not answer" are both errors the runtime can report
/// rather than a relay that opens and then dies.
pub fn serve(
    service: &Service,
    peer: Peer,
    record: CapabilityRecord,
    destination: &str,
    mut client: UnixStream,
) {
    let open = match service.present_open(peer, record, destination) {
        Ok(open) => open,
        Err(refusal) => {
            let _ = crate::write_response(&client, &refusal);
            return;
        }
    };

    let mut upstream = match connect(open.info(), open.port()) {
        Ok(upstream) => upstream,
        Err(why) => {
            let refusal = Response::error(ErrorKind::Internal, why.clone());
            // The trail first: this connection was ALLOWED and then could not
            // be made, which is a different line from a refusal and the one
            // somebody debugging an intranet certificate needs to find.
            service.present_finish(open, Err(why));
            let _ = crate::write_response(&client, &refusal);
            return;
        }
    };

    if crate::write_response(&client, &Response::Ok).is_err() {
        service.present_finish(
            open,
            Err("the agent runtime closed before the relay opened".to_string()),
        );
        return;
    }

    let outcome = relay(&mut client, &mut upstream, open.info(), open.value());
    service.present_finish(open, outcome);
}

/// Longest request head this will read before giving up on the client.
///
/// A browser's first request carries a `User-Agent`, an `Accept` list and
/// whatever cookies the profile holds; a capsule's profile is fresh, so this is
/// generous. It is bounded because the bytes come from a browser inside a
/// sandbox, and reading a head forever would hold a root thread open.
const MAX_HEAD_BYTES: usize = 16 * 1024;

/// Longest request body this will carry.
///
/// A form post or a `fetch` with a JSON body is kilobytes. A capsule uploading
/// a file through an authenticated endpoint is a thing this path does not do,
/// and saying so with a number is better than discovering the limit is the
/// timeout.
const MAX_BODY_BYTES: u64 = 8 * 1024 * 1024;

/// How long one intercepted connection may be idle before it is dropped.
const IO_TIMEOUT: Duration = Duration::from_secs(120);

/// How long the connection to the site may take to open.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// Headers this module owns and therefore drops from whatever arrived.
///
/// `authorization` and `proxy-authorization` because the credential is this
/// daemon's to add and a capsule must not be able to choose what is sent under
/// that name — a capsule that could append its own header would be a capsule
/// deciding what the site sees, which is the thing being taken away from it.
/// The rest are hop-by-hop: they describe the connection the capsule opened to
/// the runtime, not the one this daemon opens to the site.
const DROPPED: &[&str] = &[
    "authorization",
    "proxy-authorization",
    "connection",
    "proxy-connection",
    "keep-alive",
];

/// What the relay did, for the audit line.
pub struct Carried {
    /// The request line as it was sent on, so the trail says what was asked
    /// for rather than only that something was.
    pub detail: String,
    /// Bytes handed back to the capsule.
    pub returned: u64,
    /// Whether the scrubber found the credential in the response.
    pub echoed: bool,
}

/// The `Authorization` header value for a stored credential.
///
/// `bearer` and `raw` are the two shapes `ServiceInfo::auth` distinguishes, and
/// the reason `raw` exists is in its own doc: a server can want `Basic`, or a
/// header shape nobody here has seen.
pub fn authorization(info: &ServiceInfo, value: &SecretValue) -> Result<Vec<u8>, String> {
    let bytes = value.expose();
    if bytes.iter().any(|b| *b < 0x20 || *b == 0x7f) {
        // A credential with a newline in it would end the header and start a
        // line of the capsule's choosing inside a request this daemon signed.
        return Err(
            "that credential contains a control character and cannot be sent as a header"
                .to_string(),
        );
    }
    let mut out = Vec::with_capacity(bytes.len() + 8);
    match info.auth.as_str() {
        "bearer" => {
            out.extend_from_slice(b"Bearer ");
            out.extend_from_slice(bytes);
        }
        "raw" => out.extend_from_slice(bytes),
        other => {
            return Err(format!(
                "this credential is stored with auth '{other}', which is not a header shape \
                 a browser capsule can be authenticated with; store it with `--auth bearer` \
                 or `--auth raw`"
            ))
        }
    }
    Ok(out)
}

/// Rewrite one request head: drop what this daemon owns, force one request per
/// connection, add the credential.
///
/// Returns the head to send on, the request line for the trail, and how many
/// body bytes were declared. Refuses rather than repairs, everywhere: this runs
/// as root on bytes a browser inside a capsule chose.
pub fn rewrite_head(
    head: &[u8],
    pin_host: &str,
    authorization: &[u8],
) -> Result<Rewritten, String> {
    let text = std::str::from_utf8(head)
        .map_err(|_| "that request head is not text, so it is not a request".to_string())?;
    let mut lines = text.split("\r\n");
    let request_line = lines
        .next()
        .ok_or_else(|| "that request has no request line".to_string())?;
    let mut parts = request_line.split(' ');
    let method = parts.next().unwrap_or_default();
    let target = parts.next().unwrap_or_default();
    let version = parts.next().unwrap_or_default();
    if parts.next().is_some() || method.is_empty() || version.is_empty() {
        return Err(format!(
            "'{}' is not an HTTP request line",
            request_line.escape_debug()
        ));
    }
    // Origin-form only. An absolute-form target (`GET https://elsewhere/`) is
    // how a request gets re-aimed at a host this credential was never stored
    // for, and rewriting one is the parser bug family this whole module is
    // shaped to avoid.
    if !target.starts_with('/') {
        return Err(format!(
            "'{}' does not address a path on {pin_host}; a capsule's request is rewritten, \
             never re-aimed",
            target.escape_debug()
        ));
    }
    if !version.starts_with("HTTP/1.") {
        return Err(format!(
            "'{}' is not HTTP/1.x, and this path carries one request per connection",
            version.escape_debug()
        ));
    }

    let mut kept: Vec<&str> = Vec::new();
    let mut saw_host = false;
    let mut body_len: u64 = 0;
    for line in lines {
        if line.is_empty() {
            continue;
        }
        // A continuation line — obs-fold — belongs to the header above it and
        // could smuggle a second value under a name this module checked.
        if line.starts_with(' ') || line.starts_with('\t') {
            return Err("that request folds a header over two lines, which this path does not \
                        carry"
                .to_string());
        }
        let Some((name, value)) = line.split_once(':') else {
            return Err(format!("'{}' is not a header", line.escape_debug()));
        };
        let lower = name.trim().to_ascii_lowercase();
        if lower == "host" {
            saw_host = true;
            let asked = value.trim().to_ascii_lowercase();
            let bare = asked.split(':').next().unwrap_or_default();
            if bare != pin_host.to_ascii_lowercase() {
                return Err(format!(
                    "that request names host '{}' and this credential is pinned to \
                     '{pin_host}'",
                    asked.escape_debug()
                ));
            }
        }
        // A body is carried by LENGTH and not by reading until the capsule
        // stops: a browser that has sent its whole request waits for the
        // answer and never closes, so "read to EOF" is "wait for the timeout".
        // Chunked is refused rather than parsed — it would be a second framing
        // to get right in a root process, for request bodies a browser
        // essentially never sends.
        if lower == "transfer-encoding" {
            return Err(format!(
                "that request declares '{}' framing, and this path carries a body by its \
                 declared length only",
                value.trim().escape_debug()
            ));
        }
        if lower == "content-length" {
            let declared: u64 = value.trim().parse().map_err(|_| {
                format!("'{}' is not a Content-Length", value.trim().escape_debug())
            })?;
            if declared > MAX_BODY_BYTES {
                return Err(format!(
                    "that request declares a {declared}-byte body, and this path carries \
                     at most {MAX_BODY_BYTES}"
                ));
            }
            body_len = declared;
        }
        if DROPPED.contains(&lower.as_str()) {
            continue;
        }
        kept.push(line);
    }
    if !saw_host {
        return Err("that request carries no Host header".to_string());
    }

    let mut out = Vec::with_capacity(head.len() + authorization.len() + 64);
    out.extend_from_slice(request_line.as_bytes());
    out.extend_from_slice(b"\r\n");
    for line in kept {
        out.extend_from_slice(line.as_bytes());
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(b"Authorization: ");
    out.extend_from_slice(authorization);
    out.extend_from_slice(b"\r\nConnection: close\r\n\r\n");
    Ok(Rewritten {
        head: out,
        request_line: request_line.to_string(),
        body_len,
    })
}

/// What [`rewrite_head`] made of one request head.
#[derive(Debug)]
pub struct Rewritten {
    /// The head to send to the site.
    pub head: Vec<u8>,
    /// The request line, for the audit trail.
    pub request_line: String,
    /// Body bytes the request declared, and therefore exactly how many will be
    /// carried after the head.
    pub body_len: u64,
}

/// Read a request head, one byte at a time, stopping at the blank line.
///
/// Byte at a time for `egress::read_head`'s reason: anything buffered past the
/// blank line is the request BODY, and a buffered reader would swallow it.
fn read_head(stream: &mut UnixStream) -> Result<Vec<u8>, String> {
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        match stream.read(&mut byte) {
            Ok(0) => return Err("the capsule closed the connection without a request".to_string()),
            Ok(_) => {}
            Err(e) => return Err(format!("reading the capsule's request: {e}")),
        }
        head.push(byte[0]);
        if head.ends_with(b"\r\n\r\n") {
            return Ok(head);
        }
        if head.len() >= MAX_HEAD_BYTES {
            return Err("that request head is longer than this path will read".to_string());
        }
    }
}

/// Replaces a credential with `x` of the same length as it streams past.
///
/// Same length on purpose: the site declared a `Content-Length` and the capsule
/// is going to read exactly that many bytes. A redaction of a different length
/// would leave the browser waiting for bytes that are not coming, or reading
/// the start of nothing.
pub struct Scrubber {
    needle: Vec<u8>,
    held: Vec<u8>,
    pub found: bool,
}

impl Scrubber {
    pub fn new(needle: &[u8]) -> Scrubber {
        Scrubber {
            needle: needle.to_vec(),
            held: Vec::new(),
            found: false,
        }
    }

    /// Feed a chunk; get back what is safe to emit now.
    ///
    /// The last `needle.len() - 1` bytes are held back, because the value could
    /// straddle this chunk and the next. [`Scrubber::finish`] releases them.
    pub fn push(&mut self, chunk: &[u8]) -> Vec<u8> {
        self.held.extend_from_slice(chunk);
        self.redact();
        let keep = self.needle.len().saturating_sub(1);
        if self.held.len() <= keep {
            return Vec::new();
        }
        let emit = self.held.len() - keep;
        let out: Vec<u8> = self.held.drain(..emit).collect();
        out
    }

    /// Whatever is still held, at end of stream.
    pub fn finish(&mut self) -> Vec<u8> {
        self.redact();
        std::mem::take(&mut self.held)
    }

    fn redact(&mut self) {
        if self.needle.is_empty() {
            return;
        }
        let n = self.needle.len();
        let mut i = 0;
        while i + n <= self.held.len() {
            if self.held[i..i + n] == self.needle[..] {
                self.held[i..i + n].fill(b'x');
                self.found = true;
                i += n;
            } else {
                i += 1;
            }
        }
    }
}

/// The connection to the site: TLS for `https`, plain for the loopback `http`
/// the store permits.
pub enum Upstream {
    Plain(TcpStream),
    Tls(Box<crate::tls::Session>),
}

impl Upstream {
    fn write_all(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        match self {
            Upstream::Plain(s) => s.write_all(bytes),
            Upstream::Tls(s) => s.write_all(bytes),
        }
    }

    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Upstream::Plain(s) => s.read(buf),
            Upstream::Tls(s) => s.read(buf),
        }
    }

    fn shutdown_write(&mut self) {
        match self {
            Upstream::Plain(s) => {
                let _ = s.shutdown(std::net::Shutdown::Write);
            }
            Upstream::Tls(s) => s.shutdown_write(),
        }
    }
}

/// Open the connection to the site the credential is pinned to.
///
/// The host and port come from the STORE, never from the capsule: the
/// destination the runtime forwarded has already been checked equal to the pin,
/// and resolving the pinned name here rather than the forwarded string means a
/// mismatch cannot become a connection.
pub fn connect(info: &ServiceInfo, port: u16) -> Result<Upstream, String> {
    let addr = (info.host.as_str(), port);
    let tcp = std::net::ToSocketAddrs::to_socket_addrs(&addr)
        .map_err(|e| format!("{} could not be resolved: {e}", info.host))?
        .find_map(|a| TcpStream::connect_timeout(&a, CONNECT_TIMEOUT).ok())
        .ok_or_else(|| format!("could not connect to {}:{port}", info.host))?;
    tcp.set_read_timeout(Some(IO_TIMEOUT)).ok();
    tcp.set_write_timeout(Some(IO_TIMEOUT)).ok();
    match info.scheme.as_str() {
        "http" => Ok(Upstream::Plain(tcp)),
        "https" => Ok(Upstream::Tls(Box::new(crate::tls::Session::connect(
            tcp, &info.host,
        )?))),
        other => Err(format!("'{other}' is not a scheme this can originate")),
    }
}

/// Carry one request and its answer.
pub fn relay(
    client: &mut UnixStream,
    upstream: &mut Upstream,
    info: &ServiceInfo,
    value: &SecretValue,
) -> Result<Carried, String> {
    client.set_read_timeout(Some(IO_TIMEOUT)).ok();
    client.set_write_timeout(Some(IO_TIMEOUT)).ok();

    let head = read_head(client)?;
    let header = authorization(info, value)?;
    let rewritten = rewrite_head(&head, &info.host, &header)?;
    upstream
        .write_all(&rewritten.head)
        .map_err(|e| format!("sending the request to {}: {e}", info.host))?;

    // The body, by the length the head declared and not by reading until the
    // capsule stops: a browser that has sent its whole request waits for the
    // answer, so "read to EOF" would be "wait for the timeout" on every GET.
    let mut left = rewritten.body_len;
    let mut body = [0u8; 16 * 1024];
    while left > 0 {
        let want = std::cmp::min(left as usize, body.len());
        match client.read(&mut body[..want]) {
            Ok(0) => {
                return Err(format!(
                    "that request declared {} body bytes and stopped {left} short of them",
                    rewritten.body_len
                ))
            }
            Ok(n) => {
                upstream
                    .write_all(&body[..n])
                    .map_err(|e| format!("sending the request body to {}: {e}", info.host))?;
                left -= n as u64;
            }
            Err(e) => return Err(format!("reading the capsule's request body: {e}")),
        }
    }
    // Half-close, so a site that streams until end-of-request answers. Correct
    // here because `Connection: close` means one request and one answer.
    upstream.shutdown_write();

    let mut scrub = Scrubber::new(value.expose());
    let mut returned: u64 = 0;
    let mut buf = [0u8; 16 * 1024];
    loop {
        let n = match upstream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(_) => break,
        };
        let out = scrub.push(&buf[..n]);
        if !out.is_empty() {
            returned += out.len() as u64;
            if client.write_all(&out).is_err() {
                break;
            }
        }
    }
    let tail = scrub.finish();
    if !tail.is_empty() {
        returned += tail.len() as u64;
        let _ = client.write_all(&tail);
    }
    let _ = client.flush();

    Ok(Carried {
        detail: rewritten.request_line,
        returned,
        echoed: scrub.found,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(auth: &str) -> ServiceInfo {
        ServiceInfo {
            service: "intranet".into(),
            host: "intranet.example".into(),
            scheme: "https".into(),
            username: "x".into(),
            path: String::new(),
            auth: auth.into(),
            port: None,
            added: 0,
        }
    }

    const TOKEN: &str = "apex-present-9d41c7-do-not-leak";

    #[test]
    fn the_two_stored_header_shapes_are_the_two_that_are_sent() {
        let v = SecretValue::new(TOKEN.as_bytes().to_vec());
        assert_eq!(
            authorization(&info("bearer"), &v).expect("bearer"),
            format!("Bearer {TOKEN}").into_bytes()
        );
        assert_eq!(
            authorization(&info("raw"), &v).expect("raw"),
            TOKEN.as_bytes()
        );
        // Anything else is refused with the flag that fixes it, rather than
        // sent as a bearer token and rejected by the site as a 401 nobody can
        // explain.
        let e = authorization(&info("basic"), &v).expect_err("unknown shape");
        assert!(e.contains("--auth bearer"), "{e}");
    }

    #[test]
    fn a_credential_that_could_end_the_header_is_refused_rather_than_sent() {
        // A stored value with a CR in it would close the `Authorization` line
        // and start one this daemon did not write, inside a request it signed.
        let v = SecretValue::new(b"good\r\nX-Admin: yes".to_vec());
        let e = authorization(&info("bearer"), &v).expect_err("control character");
        assert!(e.contains("control character"), "{e}");
    }

    fn head(extra: &str) -> Vec<u8> {
        format!("GET /page HTTP/1.1\r\nHost: intranet.example\r\n{extra}\r\n").into_bytes()
    }

    #[test]
    fn the_credential_is_added_and_the_connection_is_not_kept_alive() {
        let r = rewrite_head(&head("User-Agent: firefox\r\n"), "intranet.example", b"Bearer t")
            .expect("rewrite");
        let text = String::from_utf8(r.head).expect("utf8");
        let detail = r.request_line;
        assert!(text.contains("\r\nAuthorization: Bearer t\r\n"), "{text}");
        assert!(text.contains("\r\nConnection: close\r\n"), "{text}");
        assert!(text.contains("User-Agent: firefox"), "{text}");
        assert_eq!(detail, "GET /page HTTP/1.1");
        // One request per connection is what keeps the parsing surface to one
        // head. A `Connection: keep-alive` that survived would mean a second
        // request arriving on this socket with no header added to it.
        assert!(!text.to_ascii_lowercase().contains("keep-alive"), "{text}");
    }

    #[test]
    fn a_capsules_own_authorization_header_never_reaches_the_site() {
        // The capsule choosing what is sent under this name is the thing being
        // taken away from it. Both spellings, because a proxy-form header on a
        // tunnelled request is the one somebody forgets.
        let r = rewrite_head(
            &head("Authorization: Bearer capsule-chose-this\r\nProxy-Authorization: x\r\n"),
            "intranet.example",
            b"Bearer real",
        )
        .expect("rewrite");
        let text = String::from_utf8(r.head).expect("utf8");
        assert!(!text.contains("capsule-chose-this"), "{text}");
        assert_eq!(text.matches("Authorization:").count(), 1, "{text}");
        assert!(text.contains("Authorization: Bearer real"), "{text}");
    }

    #[test]
    fn a_request_aimed_somewhere_else_is_refused_rather_than_rewritten() {
        // Absolute-form is how a request gets re-aimed at a host the credential
        // was never stored for.
        let absolute = b"GET https://elsewhere.example/ HTTP/1.1\r\nHost: intranet.example\r\n\r\n";
        let e = rewrite_head(absolute, "intranet.example", b"Bearer t").expect_err("absolute form");
        assert!(e.contains("never re-aimed"), "{e}");

        // And a Host that is not the pin, which is the same attempt spelled
        // through the header a virtual host is selected by.
        let e = rewrite_head(
            b"GET / HTTP/1.1\r\nHost: elsewhere.example\r\n\r\n",
            "intranet.example",
            b"Bearer t",
        )
        .expect_err("wrong host");
        assert!(e.contains("pinned to 'intranet.example'"), "{e}");
    }

    #[test]
    fn a_folded_header_is_refused_because_it_could_hide_a_second_value() {
        let folded = b"GET / HTTP/1.1\r\nHost: intranet.example\r\nX-Thing: a\r\n  b\r\n\r\n";
        let e = rewrite_head(folded, "intranet.example", b"Bearer t").expect_err("folded");
        assert!(e.contains("two lines"), "{e}");
    }

    #[test]
    fn a_request_with_no_host_is_refused_rather_than_sent_to_the_pin_anyway() {
        // Without this the pin check above would pass by never running, which
        // is the shape of refusal this repository keeps finding.
        let e = rewrite_head(b"GET / HTTP/1.1\r\nX: y\r\n\r\n", "intranet.example", b"Bearer t")
            .expect_err("no host");
        assert!(e.contains("no Host header"), "{e}");
    }

    #[test]
    fn the_scrubber_redacts_across_a_chunk_boundary_and_keeps_the_length() {
        let needle = TOKEN.as_bytes();
        let body = format!("before {TOKEN} after").into_bytes();
        // Split INSIDE the token, which is the case a scrubber that scans each
        // chunk on its own gets wrong.
        let cut = 7 + needle.len() / 2;
        let mut s = Scrubber::new(needle);
        let mut out = s.push(&body[..cut]);
        out.extend(s.push(&body[cut..]));
        out.extend(s.finish());
        assert_eq!(out.len(), body.len(), "a redaction must not change the length");
        assert!(s.found);
        let text = String::from_utf8(out).expect("utf8");
        assert!(!text.contains(TOKEN), "{text}");
        assert!(text.starts_with("before "), "{text}");
        assert!(text.ends_with(" after"), "{text}");
    }

    #[test]
    fn the_scrubber_passes_a_stream_that_does_not_contain_it_through_unchanged() {
        // The control the test above needs: a scrubber that corrupted every
        // stream would also pass the assertion above.
        let body: Vec<u8> = (0u8..=255).cycle().take(40_000).collect();
        let mut s = Scrubber::new(TOKEN.as_bytes());
        let mut out = Vec::new();
        for chunk in body.chunks(1000) {
            out.extend(s.push(chunk));
        }
        out.extend(s.finish());
        assert_eq!(out, body);
        assert!(!s.found);
    }

    #[test]
    fn a_body_is_carried_by_its_declared_length_and_chunked_is_refused() {
        let post = b"POST /f HTTP/1.1\r\nHost: intranet.example\r\nContent-Length: 12\r\n\r\n";
        let r = rewrite_head(post, "intranet.example", b"Bearer t").expect("rewrite");
        assert_eq!(r.body_len, 12);
        // Zero, not "no header": a GET with no body must not leave the relay
        // waiting for one.
        let get = b"GET / HTTP/1.1\r\nHost: intranet.example\r\n\r\n";
        assert_eq!(
            rewrite_head(get, "intranet.example", b"Bearer t").expect("rewrite").body_len,
            0
        );
        let chunked =
            b"POST / HTTP/1.1\r\nHost: intranet.example\r\nTransfer-Encoding: chunked\r\n\r\n";
        let e = rewrite_head(chunked, "intranet.example", b"Bearer t").expect_err("chunked");
        assert!(e.contains("declared length only"), "{e}");
    }
}
