//! How a Cloudflare credential is presented: a `curl` the broker owns.
//!
//! §13.4 says it in as many words — *"do not pass even the temporary token
//! directly to the agent if a broker-owned `wrangler`/API child process can
//! perform the operation"*. This is that child. The agent asks for an
//! operation; `apex-secretd` reads the credential out of its own root-owned
//! store, spends it inside a process it forked, and hands back the reply.
//!
//! ## Why curl and not an HTTP crate
//!
//! `api.cloudflare.com` is TLS, and nothing in this workspace speaks TLS. The
//! two ways to change that are a rustls-shaped dependency tree inside the root
//! daemon, or the TLS stack the image already ships and already trusts. curl is
//! in `Containerfile.base`'s own tool check, it is what §13.4 describes, and it
//! keeps the credential out of this process's address space no longer than it
//! has to be there.
//!
//! ## The whole request goes down a pipe
//!
//! `curl -q -K -`, and nothing else on the command line. Every part of the
//! request — the URL, the method, the headers, the body — is written to the
//! child's stdin as a curl config file. Three things follow from that and none
//! of them are incidental:
//!
//! * **the credential is never in `argv`**, which `/proc/<pid>/cmdline` makes
//!   world-readable for as long as the child runs;
//! * **no caller-controlled string reaches curl's option parser as an
//!   argument**, so there is no `-F 'name=@file;type=…'` to smuggle anything
//!   through;
//! * `-q` is the first thing curl sees, so the owner's `~/.curlrc` is not read.
//!   A file the caller can write must not get to choose a proxy or a CA.
//!
//! What is *not* on stdin is a request body big enough to be binary: a Worker
//! bundle goes into a file the child reads, because stdin is already the
//! config. That file holds the project's own script and the metadata built for
//! it — no credential — and it is created root-owned in a directory nothing
//! else can list, then handed to the owner.
//!
//! ## What this does not hide
//!
//! The credential is on the child's stdin, so it is in that process's memory
//! while it runs. A same-uid process outside the sandbox can reach a child's
//! `/proc` entry, exactly as it can for the `git` child P0-002 documents; a
//! confined session cannot, because the sandbox is `--unshare-pid` and the
//! daemon's children are not in its `/proc` at all. This is the same exposure
//! that crate note already states, not a new one.
//!
//! ## Why a failure comes back as a result and not as an error
//!
//! [`Reply`] carries a status and a body for every outcome that got as far as
//! running curl, including a 401 and a refused connection. The framework
//! scrubs a *result*; it does not scrub a refusal reason. So anything that
//! contains a byte the far side or the child chose travels in a `Reply`, and
//! the `Err` side of this module carries only sentences composed here.

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use apex_secret_core::SecretValue;

use crate::broker::Owner;

/// How long one API call may take, end to end.
pub const TIMEOUT_SECS: u64 = 120;

/// How long the connection alone may take.
pub const CONNECT_TIMEOUT_SECS: u64 = 15;

/// Cloudflare's REST API host.
pub const API_HOST: &str = "api.cloudflare.com";

/// The versioned prefix every path in this module is relative to.
pub const API_PREFIX: &str = "/client/v4";

/// Where the API lives.
///
/// A field and not a constant because a test has to be able to point the whole
/// path at a loopback server: nothing here has ever been run against a real
/// Cloudflare account, and a provider that could only talk to one could not be
/// tested at all. `Api::cloudflare()` is what the shipped registry builds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Api {
    pub scheme: String,
    pub host: String,
    pub port: Option<u16>,
}

impl Api {
    /// The real one.
    pub fn cloudflare() -> Api {
        Api {
            scheme: "https".to_string(),
            host: API_HOST.to_string(),
            port: None,
        }
    }

    /// A loopback stand-in on a port a test chose.
    #[cfg(test)]
    pub fn loopback(port: u16) -> Api {
        Api {
            scheme: "http".to_string(),
            host: "127.0.0.1".to_string(),
            port: Some(port),
        }
    }

    /// The URL for a path under [`API_PREFIX`].
    ///
    /// `path` is assembled by the provider out of values it has already
    /// checked — a 32-hex id, a name, a path of names — so there is nothing
    /// here to escape.
    pub fn url(&self, path: &str) -> String {
        match self.port {
            Some(port) => format!("{}://{}:{port}{API_PREFIX}{path}", self.scheme, self.host),
            None => format!("{}://{}{API_PREFIX}{path}", self.scheme, self.host),
        }
    }
}

/// What is being sent.
#[derive(Debug, Clone)]
pub enum Body {
    /// No body.
    None,
    /// A JSON document this module built. Small, and goes on stdin with the
    /// rest of the config.
    Json(String),
    /// A multipart body, already assembled. Goes in a file, because stdin is
    /// taken and because a Worker bundle is not text.
    Multipart { boundary: String, bytes: Vec<u8> },
    /// The bytes of one file, sent as they are.
    ///
    /// R2's object upload and KV's value write are not JSON and not multipart
    /// — the body *is* the object. Same reason as [`Body::Multipart`] for the
    /// file: stdin already carries the configuration, and a `data-binary` read
    /// from a path is the only way left to hand curl bytes that may be binary.
    Raw {
        content_type: &'static str,
        bytes: Vec<u8>,
    },
}

/// One API call.
#[derive(Debug, Clone)]
pub struct Call {
    pub method: &'static str,
    /// Path under [`API_PREFIX`], leading slash included.
    pub path: String,
    pub body: Body,
    /// Extra request headers, beyond the ones every call carries.
    ///
    /// The NAME is `&'static str` — it comes out of this build and never out of
    /// a request, so there is no header a caller can invent. The value is a
    /// `String` because some of them are composed (a gateway id from the
    /// project's file, a metadata document built here), and every byte of it is
    /// checked against [`printable`] before it reaches curl's configuration:
    /// [`quoted`] escapes `\` and `"` and does nothing about a newline, and a
    /// newline in a header value is a second configuration line.
    pub headers: Vec<(&'static str, String)>,
}

impl Call {
    /// A call with no extra headers, which is all of them but two.
    pub fn new(method: &'static str, path: String, body: Body) -> Call {
        Call {
            method,
            path,
            body,
            headers: Vec::new(),
        }
    }
}

/// Whether every byte of a header value is printable ASCII.
///
/// The guard that makes [`Call::headers`] safe to build from composed strings.
/// A control character would end the curl configuration line early and start
/// another; anything above `0x7e` is not something this build has any business
/// putting in a header.
pub fn printable(value: &str) -> bool {
    !value.is_empty() && value.len() <= 4096 && value.bytes().all(|b| (0x20..=0x7e).contains(&b))
}

/// What came back.
///
/// A `status` of 0 means curl never got an answer. The body then holds curl's
/// own message, which is a sentence about a connection and not a credential —
/// but it still travels here rather than in an `Err`, so the framework scrubs
/// it either way.
#[derive(Debug, Clone)]
pub struct Reply {
    pub status: u16,
    pub body: String,
}

impl Reply {
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

/// Why a call could not be made at all.
///
/// Every variant is a sentence composed in this file or in `crate::broker`.
/// None of them can contain a credential, a response body or a child's output,
/// which is what makes them safe to return through the framework's unscrubbed
/// refusal path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransportError {
    /// The stored credential is not a token this will send.
    BadCredential,
    /// The child could not be run to completion — it could not be started, its
    /// configuration could not be written, or the reply it began to read was
    /// larger than [`crate::broker::HTTP_MAX_BYTES`]. All three are
    /// `broker::run_curl`'s sentences, and none of them can carry a credential
    /// or a response body.
    NoCurl(String),
    /// A temporary file for the request body could not be made.
    NoScratch(String),
    /// The child ran but its output could not be understood.
    Unreadable,
    /// A header this build composed carries something that cannot go in one.
    BadHeader(&'static str),
}

impl std::fmt::Display for TransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TransportError::BadCredential => f.write_str(
                "the credential stored for this service is not a Cloudflare API \
                 token. One is a run of letters, digits and `-` `_` `.` `~` `+` \
                 `/` `=`; store it again with `apex cf connect`",
            ),
            TransportError::NoCurl(reason) => {
                // Not "could not be started": since this moved onto
                // `broker::run_curl`, the same variant also carries the reply
                // size cap, and a request that was refused mid-read is not one
                // that failed to start.
                write!(f, "the api client could not complete the request: {reason}")
            }
            TransportError::NoScratch(reason) => {
                write!(f, "a working file for the request could not be made: {reason}")
            }
            TransportError::BadHeader(name) => write!(
                f,
                "the '{name}' header this build composed carries a character \
                 that cannot go in a header, so the request was not made"
            ),
            TransportError::Unreadable => f.write_str(
                "the api client returned something this build could not read as \
                 a response",
            ),
        }
    }
}

impl std::error::Error for TransportError {}

/// Whether a stored value is a shape this will put in an `Authorization`
/// header.
///
/// Narrow on purpose. A credential holding a quote or a backslash would end a
/// curl config line early; one holding a carriage return would inject a header.
/// Cloudflare's own tokens are 40 characters of `[A-Za-z0-9_-]`, a global key
/// is 37 hex, and an OAuth access token adds `.`, so the set below is already
/// wider than anything Cloudflare issues.
pub fn valid_token(token: &str) -> bool {
    !token.is_empty()
        && token.len() <= 4096
        && token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~' | b'+' | b'/' | b'='))
}

/// Escape a value for a curl config line.
///
/// Only `\` and `"` need it, and neither can appear in anything this module
/// sends — the token is checked by [`valid_token`], the URL is built from
/// checked names, and JSON escapes both. It is here so that stays true when
/// somebody adds a field.
fn quoted(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for c in value.chars() {
        if c == '\\' || c == '"' {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

/// A directory the child can read one file out of and nobody else can list.
///
/// Root-owned and `0711`: the owner can open a path it is told, and cannot
/// enumerate what is in there. The file itself is `0400` and handed to the
/// owner, because the child has already dropped privileges by the time it
/// reads it.
struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}

impl Scratch {
    fn new(owner: &Owner) -> Result<Scratch, TransportError> {
        let mut name = [0u8; 16];
        // Not for secrecy — the file is the caller's own data and is mode 0400
        // to the caller. It is so that two calls at once cannot collide and so
        // that the path is not one another user could have created first.
        std::fs::File::open("/dev/urandom")
            .and_then(|mut f| std::io::Read::read_exact(&mut f, &mut name))
            .map_err(|e| TransportError::NoScratch(e.to_string()))?;
        let hex: String = name.iter().map(|b| format!("{b:02x}")).collect();
        let dir = std::env::temp_dir().join(format!("apex-cf-{hex}"));
        std::fs::create_dir(&dir).map_err(|e| TransportError::NoScratch(e.to_string()))?;
        let scratch = Scratch(dir);
        std::fs::set_permissions(&scratch.0, std::fs::Permissions::from_mode(0o711))
            .map_err(|e| TransportError::NoScratch(e.to_string()))?;
        let _ = owner;
        Ok(scratch)
    }

    fn write(&self, name: &str, bytes: &[u8], owner: &Owner) -> Result<PathBuf, TransportError> {
        let path = self.0.join(name);
        let mut file =
            std::fs::File::create(&path).map_err(|e| TransportError::NoScratch(e.to_string()))?;
        file.write_all(bytes)
            .and_then(|()| file.sync_all())
            .map_err(|e| TransportError::NoScratch(e.to_string()))?;
        drop(file);
        chown(&path, owner.uid, owner.gid)?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o400))
            .map_err(|e| TransportError::NoScratch(e.to_string()))?;
        Ok(path)
    }
}

fn chown(path: &Path, uid: u32, gid: u32) -> Result<(), TransportError> {
    let Ok(c) = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()) else {
        return Err(TransportError::NoScratch("that path cannot be named".into()));
    };
    // Safe: `c` is a NUL-terminated path that outlives the call. `lchown` and
    // not `chown` so this cannot be redirected through a link, though the
    // directory it is in is not writable by anyone but root.
    if unsafe { libc::lchown(c.as_ptr(), uid, gid) } != 0 {
        return Err(TransportError::NoScratch(
            std::io::Error::last_os_error().to_string(),
        ));
    }
    Ok(())
}

/// Make the call.
///
/// The credential is written to the child's stdin and nowhere else.
pub fn call(
    api: &Api,
    request: &Call,
    value: &SecretValue,
    owner: &Owner,
) -> Result<Reply, TransportError> {
    let token = value.as_str().filter(|t| valid_token(t)).ok_or(TransportError::BadCredential)?;

    let scratch;
    let mut config = String::new();
    config.push_str(&format!("url = {}\n", quoted(&api.url(&request.path))));
    config.push_str(&format!("request = {}\n", quoted(request.method)));
    config.push_str(&format!("proto = {}\n", quoted(&format!("={}", api.scheme))));
    config.push_str(&format!(
        "header = {}\n",
        quoted(&format!("Authorization: Bearer {token}"))
    ));
    // curl sends `Expect: 100-continue` for a body over 1 KiB and then waits
    // for a server that may never answer it. Cloudflare does; a hand-written
    // loopback double does not, and a test that hangs for a second per upload
    // is a test somebody eventually deletes.
    config.push_str("header = \"Expect:\"\n");
    config.push_str("header = \"Accept: application/json\"\n");
    config.push_str(&format!(
        "header = {}\n",
        quoted(&format!("User-Agent: apex-secretd/{}", env!("CARGO_PKG_VERSION")))
    ));

    for (name, value) in &request.headers {
        // Refused rather than sanitised. A value this build composed and cannot
        // send is a bug in this build, and quietly stripping the byte that
        // makes it unsendable would hide it.
        if !printable(value) {
            return Err(TransportError::BadHeader(name));
        }
        config.push_str(&format!("header = {}\n", quoted(&format!("{name}: {value}"))));
    }

    match &request.body {
        Body::None => {}
        Body::Json(json) => {
            config.push_str("header = \"Content-Type: application/json\"\n");
            config.push_str(&format!("data-binary = {}\n", quoted(json)));
        }
        Body::Multipart { boundary, bytes } => {
            scratch = Scratch::new(owner)?;
            let path = scratch.write("body", bytes, owner)?;
            config.push_str(&format!(
                "header = {}\n",
                quoted(&format!(
                    "Content-Type: multipart/form-data; boundary={boundary}"
                ))
            ));
            config.push_str(&format!(
                "data-binary = {}\n",
                quoted(&format!("@{}", path.display()))
            ));
        }
        Body::Raw {
            content_type,
            bytes,
        } => {
            scratch = Scratch::new(owner)?;
            let path = scratch.write("body", bytes, owner)?;
            // `content_type` is `&'static str` and not a caller's string, so
            // there is no header to inject here. That is the whole reason the
            // type is what it is: an R2 object's media type is chosen from a
            // table in this build, not sent by whoever asked for the upload.
            config.push_str(&format!(
                "header = {}\n",
                quoted(&format!("Content-Type: {content_type}"))
            ));
            config.push_str(&format!(
                "data-binary = {}\n",
                quoted(&format!("@{}", path.display()))
            ));
        }
    }

    config.push_str(&format!("max-time = {TIMEOUT_SECS}\n"));
    config.push_str(&format!("connect-timeout = {CONNECT_TIMEOUT_SECS}\n"));
    // No `location`: a redirect is a server choosing where this credential
    // goes next, and the host pin was applied to the URL above.
    config.push_str("silent\n");
    config.push_str("show-error\n");
    // The status arrives after the body, on its own line, so a body that
    // happens to end in digits cannot be mistaken for one.
    config.push_str("write-out = \"\\n%{http_code}\"\n");

    // One curl in this build, not two. `broker::run_curl` owns the child: the
    // absolute program, `-q` first so the owner's `~/.curlrc` cannot configure
    // it, `env_clear` with `NO_PROXY=*`, the drop to the owner's uid, the
    // configuration on stdin where `/proc` cannot read it, and the cap on how
    // large a reply this daemon will carry. Everything below is this provider's
    // own: `run_curl` hands back curl's exit code with the two streams apart,
    // because the status this model needs is on the last line of stdout and
    // stderr appended to it would make that line unparseable.
    let out = crate::broker::run_curl(&config, owner).map_err(TransportError::NoCurl)?;

    let stdout = out.stdout.as_str();
    let stderr = out.stderr.as_str();
    let (body, status) = match stdout.rsplit_once('\n') {
        Some((body, tail)) => (body.to_string(), tail.trim().parse::<u16>().ok()),
        None => (String::new(), stdout.trim().parse::<u16>().ok()),
    };
    let Some(status) = status else {
        // `code` is CURL'S EXIT CODE, not an HTTP status. Zero means the
        // transfer happened, so there should have been a status line, and its
        // absence is output this build cannot read.
        if out.code == 0 {
            return Err(TransportError::Unreadable);
        }
        // curl failed before it had a status: a refused connection, a name that
        // does not resolve, a TLS handshake. Its message is the only useful
        // thing there is, and it belongs in the reply where it gets scrubbed.
        return Ok(Reply {
            status: 0,
            body: stderr.trim().to_string(),
        });
    };
    let mut body = body;
    if !stderr.trim().is_empty() {
        if !body.is_empty() && !body.ends_with('\n') {
            body.push('\n');
        }
        body.push_str(stderr.trim_end());
    }
    Ok(Reply { status, body })
}

/// A multipart body, assembled here rather than by curl's `-F`.
///
/// curl's own form parser reads `;`, `,`, `<` and `@` out of the strings it is
/// given, and two of the values here — a file name and a project path — come
/// from the caller. Building the body means the caller's strings are bytes in
/// a buffer and never options.
pub struct Multipart {
    boundary: String,
    bytes: Vec<u8>,
}

impl Multipart {
    pub fn new() -> Result<Multipart, TransportError> {
        let mut raw = [0u8; 16];
        std::fs::File::open("/dev/urandom")
            .and_then(|mut f| std::io::Read::read_exact(&mut f, &mut raw))
            .map_err(|e| TransportError::NoScratch(e.to_string()))?;
        let hex: String = raw.iter().map(|b| format!("{b:02x}")).collect();
        Ok(Multipart {
            boundary: format!("apex{hex}"),
            bytes: Vec::new(),
        })
    }

    /// One part. `name` and `filename` are checked by the caller against
    /// `operation::valid_name`, so neither can carry a quote or a newline —
    /// which is the only thing that could break a part header.
    pub fn part(&mut self, name: &str, filename: Option<&str>, kind: &str, content: &[u8]) {
        let mut header = format!(
            "--{}\r\nContent-Disposition: form-data; name=\"{name}\"",
            self.boundary
        );
        if let Some(filename) = filename {
            header.push_str(&format!("; filename=\"{filename}\""));
        }
        header.push_str(&format!("\r\nContent-Type: {kind}\r\n\r\n"));
        self.bytes.extend_from_slice(header.as_bytes());
        self.bytes.extend_from_slice(content);
        self.bytes.extend_from_slice(b"\r\n");
    }

    pub fn finish(mut self) -> Body {
        self.bytes
            .extend_from_slice(format!("--{}--\r\n", self.boundary).as_bytes());
        Body::Multipart {
            boundary: self.boundary,
            bytes: self.bytes,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_url_is_the_versioned_prefix_and_the_path() {
        let api = Api::cloudflare();
        assert_eq!(
            api.url("/accounts/abc/workers/scripts/w"),
            "https://api.cloudflare.com/client/v4/accounts/abc/workers/scripts/w"
        );
        assert_eq!(
            Api::loopback(9).url("/accounts/abc"),
            "http://127.0.0.1:9/client/v4/accounts/abc"
        );
    }

    #[test]
    fn a_credential_that_could_break_out_of_a_config_line_is_refused() {
        // The token goes into `header = "Authorization: Bearer …"`. A quote
        // ends the value, a backslash escapes the next character and a newline
        // starts a new option — so a credential holding one is not sent at
        // all, rather than sent as something else.
        for evil in [
            "tok\"en",
            "tok\\en",
            "tok\nheader = \"X-Evil: 1\"",
            "tok\ren",
            "tok en",
            "",
            "tok;en",
            "tok'en",
        ] {
            assert!(!valid_token(evil), "'{}' was accepted", evil.escape_debug());
        }
        // What Cloudflare actually issues.
        assert!(valid_token("v1.0-abcDEF_123-456789012345678901234567890"));
        assert!(valid_token(&"a".repeat(40)));
        assert!(valid_token(&"0123456789abcdef".repeat(2)));
    }

    #[test]
    fn a_config_value_is_escaped_even_though_nothing_reaching_it_needs_to_be() {
        assert_eq!(quoted("plain"), "\"plain\"");
        assert_eq!(quoted("a\"b"), "\"a\\\"b\"");
        assert_eq!(quoted("a\\b"), "\"a\\\\b\"");
    }

    #[test]
    fn a_multipart_body_carries_its_parts_and_closes_its_boundary() {
        let mut form = Multipart::new().expect("boundary");
        form.part("metadata", None, "application/json", b"{\"main_module\":\"w.js\"}");
        form.part("w.js", Some("w.js"), "application/javascript+module", b"export default {};");
        let Body::Multipart { boundary, bytes } = form.finish() else {
            panic!("expected a multipart body");
        };
        let text = String::from_utf8(bytes).expect("utf8 in this test");
        assert!(text.starts_with(&format!("--{boundary}\r\n")));
        assert!(text.ends_with(&format!("--{boundary}--\r\n")));
        assert!(text.contains("name=\"metadata\""));
        assert!(text.contains("filename=\"w.js\""));
        assert!(text.contains("export default {};"));
        // Two parts and one closing delimiter.
        assert_eq!(text.matches(&format!("--{boundary}")).count(), 3);
    }

    /// The path nothing covered before this call moved onto `broker::run_curl`.
    ///
    /// When curl never gets a status — a refused connection, a name that does
    /// not resolve, a TLS handshake that fails — `Reply.status` is 0 and the
    /// body is curl's own message. `mod.rs` keys on `status == 0` to say "the
    /// api could not be reached" rather than "cloudflare answered HTTP 0", and
    /// the whole distinction rests on telling curl's EXIT CODE apart from an
    /// HTTP status. `run_curl` returns the former, which is exactly the mixup
    /// this move had to avoid, so it is measured rather than asserted.
    ///
    /// Port 9 is `discard`, reserved and not listening: no server to stand up
    /// and nothing to leak.
    #[test]
    fn a_connection_that_never_happens_is_a_status_of_zero_and_curls_own_message() {
        // Safe: getuid cannot fail.
        let owner = crate::broker::owner(unsafe { libc::getuid() }).expect("own uid");
        let reply = call(
            &Api::loopback(9),
            &Call::new("GET", "/accounts".to_string(), Body::None),
            &SecretValue::new(b"apex-cf-unreachable-token".to_vec()),
            &owner,
        )
        .expect("a refused connection is a Reply, not an Err");
        assert_eq!(reply.status, 0, "{}", reply.body);
        assert!(!reply.ok());
        // curl's message, not an empty body — an empty one would make
        // `mod.rs`'s "the api could not be reached\n{body}" a bare heading.
        assert!(!reply.body.trim().is_empty(), "stderr was dropped");
        // And the credential is not in it, which is the reason this comes back
        // as a Reply the framework scrubs rather than as an Err it does not.
        assert!(!reply.body.contains("apex-cf-unreachable-token"), "{}", reply.body);
    }

    /// The guard at the point where it matters, not the predicate behind it.
    ///
    /// [`printable`] has its own test, but a test of a predicate stays green if
    /// the caller stops consulting it. This one goes through [`call`], which is
    /// the only thing that writes a header into curl's configuration file —
    /// where [`quoted`] escapes `\` and `"` and does nothing whatever about a
    /// newline, and a newline is a second configuration line: another header,
    /// or an option like `output` pointed somewhere it should not be.
    ///
    /// Port 9 again, so nothing is contacted. Which is itself the assertion:
    /// the refusal must come back **before** the request is built, so a
    /// connection that would otherwise report status 0 never even happens.
    #[test]
    fn a_header_carrying_a_second_line_is_refused_before_curl_is_configured() {
        // Safe: getuid cannot fail.
        let owner = crate::broker::owner(unsafe { libc::getuid() }).expect("own uid");
        for evil in ["one\ntwo", "one\r\nheader: injected", "one\u{0}two", ""] {
            let request = Call {
                method: "POST",
                path: "/accounts".to_string(),
                body: Body::None,
                headers: vec![("cf-aig-metadata", evil.to_string())],
            };
            let error = call(
                &Api::loopback(9),
                &request,
                &SecretValue::new(b"apex-cf-header-token".to_vec()),
                &owner,
            )
            .expect_err(&format!("'{}' was sent", evil.escape_debug()));
            assert!(
                matches!(error, TransportError::BadHeader("cf-aig-metadata")),
                "'{}' produced {error:?}",
                evil.escape_debug()
            );
            // Named in the message, because a build that cannot send a header it
            // composed has a bug and the reader needs to know which header.
            assert!(error.to_string().contains("cf-aig-metadata"), "{error}");
        }

        // And a well-formed one is not refused — so this is a check on the
        // value and not a refusal of every extra header.
        let fine = Call {
            method: "POST",
            path: "/accounts".to_string(),
            body: Body::None,
            headers: vec![("cf-aig-gateway-id", "apex-gateway".to_string())],
        };
        let reply = call(
            &Api::loopback(9),
            &fine,
            &SecretValue::new(b"apex-cf-header-token".to_vec()),
            &owner,
        )
        .expect("a printable header must be sendable");
        assert_eq!(reply.status, 0, "nothing is listening on discard");
    }

    #[test]
    fn a_boundary_is_different_every_time() {
        // Two uploads in flight at once must not share one, and a predictable
        // boundary in a body built from caller data is a body a caller can
        // terminate early.
        let a = Multipart::new().expect("one").boundary;
        let b = Multipart::new().expect("two").boundary;
        assert_ne!(a, b);
        assert!(a.starts_with("apex") && a.len() > 20);
    }
}
