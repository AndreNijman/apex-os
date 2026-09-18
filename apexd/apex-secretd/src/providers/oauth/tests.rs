//! The `oauth` provider, against a loopback token endpoint.
//!
//! # Nothing here reaches a real authorisation server, and it cannot
//!
//! The double is on `127.0.0.1`, and `account::oauth_for_auth_host` — the
//! shipped table, which is the ONLY route `OAuthProvider::new()` has — does not
//! contain `127.0.0.1` and never will. A fixture reaches it by constructing
//! `OAuthProvider::at(port)`, which nothing outside `#[cfg(test)]` can call.
//! There is no real Cloudflare credential in this repository and no request
//! shape that would send one anywhere but `dash.cloudflare.com`.
//!
//! # What the double checks
//!
//! It parses the **form body that actually arrived** and answers on it. That
//! is the half a unit test of `perform` could not do: the credential goes down
//! curl's stdin as a configuration and the encoding is curl's, so the only way
//! to know what was sent is to read what a server received. It also records
//! curl's own command line out of `/proc`, which is how "the token is never on
//! an argument list" stops being a claim.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use apex_secret_core::capability::CapabilityRecord;
use apex_secret_core::protocol::Response;
use apex_secret_core::store::Store;
use apex_secret_core::SecretValue;

use crate::peer::Peer;
use crate::provider::Registry;
use crate::service::{NewService, Service};

use super::*;

/// What `apex cf connect` stored, in the fixture's own spelling.
const REFRESH_SERVICE: &str = "demo-refresh";
const ACCESS_SERVICE: &str = "demo";

/// The refresh token the fixture starts with. Distinctive enough that finding
/// it anywhere downstream means it travelled.
const OLD_REFRESH: &str = "apex-old-refresh-3d10fa-do-not-leak";
const OLD_ACCESS: &str = "apex-old-access-7b22cd-do-not-leak";
/// What the double issues.
const NEW_ACCESS: &str = "apex-new-access-51ce90-do-not-leak";
const NEW_REFRESH: &str = "apex-new-refresh-a4f7e2-do-not-leak";

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// Issues a new access token AND rotates the refresh token, which is what
    /// this build assumes Cloudflare does.
    Rotating,
    /// Issues a new access token and no `refresh_token`, which RFC 6749 §6
    /// also allows.
    RenewOnly,
    /// `400 invalid_grant` — a revoked or already-rotated refresh token, the
    /// commonest real failure.
    Refuses,
    /// Answers 200 with a token that has a `"`, a `\` and a newline in it.
    /// A server this build does not control can say anything.
    HostileToken,
    /// Answers 200 with something that is not JSON.
    NotJson,
    /// Answers 200 with JSON that has no `access_token` in it.
    NoToken,
    /// A `200` whose `Content-Length` is far over `broker::HTTP_MAX_BYTES`,
    /// with a few kilobytes behind it. curl reads the length before the body
    /// and aborts with none of it written, so what this provider would see
    /// without a guard is a 200 with an empty document under it.
    Oversize,
}

/// One request the double saw.
#[derive(Debug, Clone)]
struct Seen {
    method: String,
    target: String,
    headers: BTreeMap<String, String>,
    /// The form body, parsed: `grant_type`, `refresh_token`, `client_id`.
    form: BTreeMap<String, String>,
}

struct Fake {
    port: u16,
    seen: Arc<Mutex<Vec<Seen>>>,
}

impl Fake {
    fn start(mode: Mode) -> Fake {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let port = listener.local_addr().expect("addr").port();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let recorder = Arc::clone(&seen);
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let recorder = Arc::clone(&recorder);
                std::thread::spawn(move || serve(stream, &recorder, mode));
            }
        });
        Fake { port, seen }
    }

    fn seen(&self) -> Vec<Seen> {
        self.seen.lock().expect("lock").clone()
    }
}

fn serve(mut stream: TcpStream, recorder: &Arc<Mutex<Vec<Seen>>>, mode: Mode) {
    let mut reader = BufReader::new(stream.try_clone().expect("clone"));
    let mut first = String::new();
    if reader.read_line(&mut first).is_err() {
        return;
    }
    let mut words = first.split_whitespace();
    let method = words.next().unwrap_or("").to_string();
    let target = words.next().unwrap_or("").to_string();

    let mut headers: BTreeMap<String, String> = BTreeMap::new();
    let mut length = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).is_err() || line.trim().is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            let name = name.trim().to_ascii_lowercase();
            let value = value.trim().to_string();
            if name == "content-length" {
                length = value.parse().unwrap_or(0);
            }
            headers.insert(name, value);
        }
    }
    let mut body = vec![0u8; length];
    if length > 0 && reader.read_exact(&mut body).is_err() {
        return;
    }
    let form = parse_form(&String::from_utf8_lossy(&body));
    recorder.lock().expect("lock").push(Seen {
        method,
        target,
        headers,
        form,
    });

    if mode == Mode::Oversize {
        let _ = stream.write_all(
            b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
              Content-Length: 40000000\r\nConnection: close\r\n\r\n",
        );
        let _ = stream.write_all(&[b'x'; 4096]);
        let _ = stream.flush();
        return;
    }

    let (status, payload) = match mode {
        Mode::Rotating => (
            "200 OK",
            format!(
                r#"{{"access_token":"{NEW_ACCESS}","refresh_token":"{NEW_REFRESH}",
                     "token_type":"bearer","expires_in":3600}}"#
            ),
        ),
        Mode::RenewOnly => (
            "200 OK",
            format!(r#"{{"access_token":"{NEW_ACCESS}","token_type":"bearer"}}"#),
        ),
        Mode::Refuses => (
            "400 Bad Request",
            r#"{"error":"invalid_grant","error_description":"refresh token is invalid"}"#
                .to_string(),
        ),
        Mode::HostileToken => (
            "200 OK",
            // A `"`, a `\` and a newline, each of which would end or extend a
            // curl configuration line in whatever presents this next.
            "{\"access_token\":\"a\\\"b\\\\c\\nd\",\"token_type\":\"bearer\"}".to_string(),
        ),
        Mode::NotJson => ("200 OK", "<html>we moved</html>".to_string()),
        Mode::NoToken => ("200 OK", r#"{"token_type":"bearer"}"#.to_string()),
        // Written and returned above, before anything here is composed: its
        // whole point is a `Content-Length` that does not match the body.
        Mode::Oversize => unreachable!("the oversized reply is written above"),
    };
    let reply = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{payload}",
        payload.len()
    );
    let _ = stream.write_all(reply.as_bytes());
    let _ = stream.flush();
}

/// `a=1&b=2`, percent-decoded.
fn parse_form(body: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for pair in body.split('&').filter(|p| !p.is_empty()) {
        let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
        out.insert(percent_decode(name), percent_decode(value));
    }
    out
}

fn percent_decode(text: &str) -> String {
    let bytes = text.replace('+', " ").into_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(byte) = u8::from_str_radix(
                &String::from_utf8_lossy(&bytes[i + 1..i + 3]),
                16,
            ) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

// ── the fixture ────────────────────────────────────────────────────────────

struct Fixture {
    service: Service,
    store: PathBuf,
    project: PathBuf,
    fake: Fake,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.store).ok();
        std::fs::remove_dir_all(&self.project).ok();
    }
}

fn me() -> Peer {
    // Safe: getuid/getgid cannot fail.
    Peer {
        pid: std::process::id() as libc::pid_t,
        uid: unsafe { libc::getuid() },
        gid: unsafe { libc::getgid() },
    }
}

impl Fixture {
    fn new(name: &str, mode: Mode) -> Fixture {
        Fixture::with(name, mode, REFRESH_SERVICE, None)
    }

    /// `refresh_name` is the name the request runs against; `username` is what
    /// goes in the store's non-secret half, which is where a per-grant OAuth
    /// client id would live.
    fn with(name: &str, mode: Mode, refresh_name: &str, username: Option<&str>) -> Fixture {
        let fake = Fake::start(mode);
        let tag = format!("{name}-{}-{}", std::process::id(), fake.port);
        // /var/tmp, this project's rule for anything a suite creates.
        let store = PathBuf::from("/var/tmp").join(format!("apex-oauth-store-{tag}"));
        let project = PathBuf::from("/var/tmp").join(format!("apex-oauth-project-{tag}"));
        std::fs::remove_dir_all(&store).ok();
        std::fs::remove_dir_all(&project).ok();
        std::fs::create_dir_all(&project).expect("project");

        let mut registry = Registry::new();
        registry
            .register(Box::new(OAuthProvider::at(fake.port)))
            .expect("register");
        let service = Service::new(Store::new(store.clone()), false, registry);

        let peer = me();
        // The refresh token, pinned to the AUTHORISATION host — which here is
        // the loopback double. `http` is allowed for a loopback host: nothing
        // crosses a network.
        assert_eq!(
            service.add(
                peer,
                NewService {
                    service: refresh_name,
                    host: "127.0.0.1",
                    scheme: "http",
                    username,
                    path: "",
                    auth: None,
                    port: Some(fake.port),
                },
                SecretValue::new(OLD_REFRESH.as_bytes().to_vec()),
            ),
            Response::Ok
        );
        // The access token, pinned to a DIFFERENT host — the shape
        // `apex cf connect` stores, and the reason the second credential is
        // harmless if the first is granted away. It is also what makes the
        // "the pin did not move" assertions mean something: a replace that
        // rewrote `ServiceInfo` would have to put this on `127.0.0.1`.
        if let Some(access) = account::renewed_service(refresh_name) {
            assert_eq!(
                service.add(
                    peer,
                    NewService {
                        service: &access,
                        host: "api.example.test",
                        scheme: "https",
                        username: None,
                        path: "",
                        auth: None,
                        port: None,
                    },
                    SecretValue::new(OLD_ACCESS.as_bytes().to_vec()),
                ),
                Response::Ok
            );
        }
        let granted = service.grant(
            peer,
            project.to_str().expect("utf8"),
            refresh_name,
            "oauth.token.refresh",
            false,
        );
        assert!(matches!(granted, Response::Grants { .. }), "{granted:?}");

        Fixture {
            service,
            store,
            project,
            fake,
        }
    }

    fn refresh(&self, refresh_name: &str) -> Response {
        let mut rec = CapabilityRecord::new(refresh_name, "oauth.token.refresh", "");
        rec.project = Some(self.project.to_string_lossy().into_owned());
        self.service.use_capability(me(), rec, Vec::new())
    }

    fn value(&self, name: &str) -> Option<String> {
        Store::new(self.store.clone())
            .value(me().uid, name)
            .ok()
            .and_then(|v| v.as_str().map(str::to_string))
    }

    fn info(&self, name: &str) -> Option<apex_secret_core::store::ServiceInfo> {
        Store::new(self.store.clone()).info(me().uid, name)
    }

    fn trail(&self) -> String {
        std::fs::read_to_string(Store::new(self.store.clone()).audit_path()).unwrap_or_default()
    }
}

// ── routing ────────────────────────────────────────────────────────────────

#[test]
fn the_shipped_provider_has_no_route_to_a_loopback_host() {
    // The property the whole fixture rests on, asserted rather than assumed.
    // `OAuthProvider::new()` is what `default_registry` builds, and its only
    // route is `account::oauth_for_auth_host` — a list of real authorisation
    // servers. If a `127.0.0.1` entry were ever added to that table, every
    // test below would still pass and the daemon would have gained a route to
    // whatever is listening on this machine.
    let shipped = OAuthProvider::new();
    assert!(shipped.extra.is_empty(), "the shipped provider carries an extra route");
    assert!(shipped.oauth_for("127.0.0.1").is_none());
    assert!(shipped.oauth_for("localhost").is_none());
    // And it DOES route the one this build can actually renew, so the
    // assertion above is not passing because routing is broken outright.
    let cf = shipped
        .oauth_for("dash.cloudflare.com")
        .expect("Cloudflare is in the shipped table");
    assert_eq!(cf.token_url, "https://dash.cloudflare.com/oauth2/token");
    assert_eq!(cf.client_id, Some(account::WRANGLER_CLIENT_ID));
}

#[test]
fn a_credential_stored_for_something_that_is_not_an_authorisation_server_is_refused() {
    // A refresh is routed by the host the credential was pinned to and by
    // nothing else. There is no resource and no parameter on this operation,
    // so this refusal is the whole of "you cannot point it somewhere else".
    let f = Fixture::new("unrouted", Mode::Rotating);
    let peer = me();
    assert_eq!(
        f.service.add(
            peer,
            NewService {
                service: "elsewhere-refresh",
                host: "api.example.test",
                scheme: "https",
                username: None,
                path: "",
                auth: None,
                port: None,
            },
            SecretValue::new(OLD_REFRESH.as_bytes().to_vec()),
        ),
        Response::Ok
    );
    let granted = f.service.grant(
        peer,
        f.project.to_str().expect("utf8"),
        "elsewhere-refresh",
        "oauth.token.refresh",
        false,
    );
    assert!(matches!(granted, Response::Grants { .. }), "{granted:?}");

    let reply = f.refresh("elsewhere-refresh");
    let (_, message) = reply.as_error().expect("it was routed somewhere");
    assert!(message.contains("api.example.test"), "{message}");
    assert!(message.contains("not an authorisation server"), "{message}");
    assert!(f.fake.seen().is_empty(), "the double was contacted anyway");
}

#[test]
fn a_credential_whose_name_does_not_say_what_it_renews_is_refused() {
    // `replaces` is derived from the name the request is already running
    // against. A name that carries neither suffix names nothing to renew, and
    // guessing one would be inventing a credential to overwrite.
    let f = Fixture::with("nameless", Mode::Rotating, "demo", None);
    let reply = f.refresh("demo");
    let (_, message) = reply.as_error().expect("a nameless credential was accepted");
    assert!(message.contains("not a refresh credential"), "{message}");
    assert!(message.contains(".refresh"), "{message}");
    assert!(f.fake.seen().is_empty(), "the double was contacted anyway");
}

#[test]
fn both_refresh_suffixes_route_to_the_same_place() {
    // `.refresh` is what `AccountRef::refresh_service` writes and `-refresh` is
    // what `apex cf connect` has been writing since before accounts existed.
    // Neither can be renamed without orphaning a credential somebody holds, so
    // both have to work — and the one that would silently not work is the one
    // with the real Cloudflare token behind it.
    for name in ["demo-refresh", "demo.refresh"] {
        let f = Fixture::with("suffix", Mode::RenewOnly, name, None);
        let reply = f.refresh(name);
        let Response::Performed { exit_code, output, .. } = &reply else {
            panic!("{name}: {reply:?}");
        };
        assert_eq!(*exit_code, 0, "{name}: {output}");
        assert_eq!(f.value("demo").as_deref(), Some(NEW_ACCESS), "{name}");
    }
}

// ── the refresh itself ─────────────────────────────────────────────────────

#[test]
fn a_refresh_replaces_the_access_token_and_the_rotated_refresh_token() {
    let f = Fixture::new("rotating", Mode::Rotating);
    let before = f.info(ACCESS_SERVICE).expect("stored");
    let reply = f.refresh(REFRESH_SERVICE);

    let Response::Performed { exit_code, output, .. } = &reply else {
        panic!("{reply:?}");
    };
    assert_eq!(*exit_code, 0, "{output}");

    // The request, as the server received it. Every field is RFC 6749 §6's.
    let seen = f.fake.seen();
    assert_eq!(seen.len(), 1, "{seen:?}");
    assert_eq!(seen[0].method, "POST");
    assert_eq!(seen[0].target, "/oauth2/token");
    assert_eq!(seen[0].form.get("grant_type").map(String::as_str), Some("refresh_token"));
    assert_eq!(seen[0].form.get("refresh_token").map(String::as_str), Some(OLD_REFRESH));
    assert_eq!(seen[0].form.get("client_id").map(String::as_str), Some("test-client-id"));
    assert_eq!(
        seen[0].headers.get("content-type").map(String::as_str),
        Some("application/x-www-form-urlencoded"),
        "a token endpoint takes a form, not JSON"
    );

    // Both credentials now hold what the server issued, byte for byte.
    assert_eq!(f.value(ACCESS_SERVICE).as_deref(), Some(NEW_ACCESS));
    assert_eq!(f.value(REFRESH_SERVICE).as_deref(), Some(NEW_REFRESH));

    // Neither secret came back, and neither is in the trail. For a token
    // endpoint the reply IS the secret, so this is the assertion that matters.
    for secret in [NEW_ACCESS, NEW_REFRESH, OLD_REFRESH] {
        assert!(!output.contains(secret), "a secret came back: {output}");
        assert!(!f.trail().contains(secret), "a secret is in the audit trail");
    }
    assert!(output.contains("renewed the access token"), "{output}");
    assert!(output.contains("'demo' and 'demo-refresh'"), "{output}");
    assert!(output.contains("about 60 minutes"), "expires_in was dropped: {output}");

    // The pin did not move, and could not have: `Replaced` carries no host.
    // The access credential is on a different host from the token endpoint, so
    // a replace that rewrote `ServiceInfo` would be visible here.
    let after = f.info(ACCESS_SERVICE).expect("still stored");
    assert_eq!(after.host, "api.example.test");
    assert_eq!(after.host, before.host);
    assert_eq!(after.scheme, before.scheme);
    assert_eq!(after.username, before.username);
    assert_eq!(after.port, before.port);
}

#[test]
fn a_reply_that_rotates_no_refresh_token_leaves_the_stored_one_alone() {
    // RFC 6749 §6 says the server MAY issue a new refresh token. A reply
    // without one means the token that was just spent is still good, so
    // writing anything over it would destroy a working credential — and the
    // account would then be broken in exactly the way a refresh exists to
    // prevent.
    let f = Fixture::new("renew-only", Mode::RenewOnly);
    let reply = f.refresh(REFRESH_SERVICE);

    let Response::Performed { exit_code, output, .. } = &reply else {
        panic!("{reply:?}");
    };
    assert_eq!(*exit_code, 0, "{output}");
    assert_eq!(f.value(ACCESS_SERVICE).as_deref(), Some(NEW_ACCESS));
    assert_eq!(
        f.value(REFRESH_SERVICE).as_deref(),
        Some(OLD_REFRESH),
        "the refresh token was replaced by a reply that did not rotate it"
    );
    assert!(output.contains("'demo' now holds"), "{output}");
    assert!(!output.contains("demo-refresh"), "{output}");
}

#[test]
fn a_token_endpoint_that_refuses_leaves_both_credentials_exactly_as_they_were() {
    // `invalid_grant` is the commonest real failure — a revoked grant, or a
    // refresh token that was already rotated somewhere else. The reason has to
    // reach the caller, because it is the only thing that says which of those
    // it was; and nothing may be written, because a failed renewal that
    // blanked the credential would turn a recoverable problem into a lost
    // account.
    let f = Fixture::new("refused", Mode::Refuses);
    let reply = f.refresh(REFRESH_SERVICE);

    let Response::Performed { exit_code, output, .. } = &reply else {
        panic!("{reply:?}");
    };
    assert_eq!(*exit_code, 1, "{output}");
    assert!(output.contains("HTTP 400"), "{output}");
    assert!(output.contains("invalid_grant"), "{output}");
    assert!(output.contains("left exactly as they were"), "{output}");
    assert_eq!(f.value(ACCESS_SERVICE).as_deref(), Some(OLD_ACCESS));
    assert_eq!(f.value(REFRESH_SERVICE).as_deref(), Some(OLD_REFRESH));
}

#[test]
fn a_token_a_server_made_hostile_is_refused_rather_than_stored() {
    // The far side is a server the owner chose and this daemon does not
    // control, so what it answers with is input. A token carrying a `"`, a `\`
    // or a newline would be a second configuration line in the curl that
    // presents it next, or a second header — and it would have been WRITTEN TO
    // THE STORE first, so the damage would outlive the request that caused it.
    let f = Fixture::new("hostile", Mode::HostileToken);
    let reply = f.refresh(REFRESH_SERVICE);

    let (_, message) = reply.as_error().expect("a hostile token was accepted");
    assert!(message.contains("no usable access_token"), "{message}");
    assert_eq!(f.value(ACCESS_SERVICE).as_deref(), Some(OLD_ACCESS));
    assert_eq!(f.value(REFRESH_SERVICE).as_deref(), Some(OLD_REFRESH));
}

/// An aborted read of a token endpoint must say so, not blame the server.
///
/// This one refuses either way — an empty body is not JSON — so the assertion
/// that matters is about the REASON. Without the guard the message is "…
/// answered 200 with something that is not JSON", which points a reader at the
/// authorisation server when the truth is that this build stopped reading. Its
/// control is `Mode::NotJson` in the test below, which must keep saying exactly
/// that, so this is not a test that any refusal satisfies.
#[test]
fn a_token_reply_this_build_stopped_reading_says_so_rather_than_blaming_the_server() {
    let f = Fixture::new("oversize", Mode::Oversize);
    let reply = f.refresh(REFRESH_SERVICE);

    // It really went: the refusal is about the reply, not about the request.
    assert_eq!(f.fake.seen().len(), 1, "the refresh never reached the double");

    let (_, message) = reply.as_error().expect("an aborted refresh was accepted");
    assert!(message.contains("curl exited"), "{message}");
    assert!(message.contains("did not finish"), "{message}");
    // 63 and not 18: with `max-filesize` curl stops at the limit having
    // written nothing; without it, it reads to EOF and the cap this build
    // applies afterwards is a cap on memory already spent.
    assert!(
        message.contains("curl exited 63"),
        "the reply was read to the end before it was refused: {message}"
    );
    assert!(
        !message.contains("not JSON"),
        "the refusal blames the server for this build's own cap: {message}"
    );
    // Nothing was replaced, which is the property every failure on this path
    // shares and the one that would hurt most to lose.
    assert_eq!(f.value(ACCESS_SERVICE).as_deref(), Some(OLD_ACCESS));
    assert_eq!(f.value(REFRESH_SERVICE).as_deref(), Some(OLD_REFRESH));
}

#[test]
fn a_reply_that_is_not_a_token_is_an_error_and_not_a_silent_success() {
    // Two shapes a captive portal or a moved endpoint produces: HTML with a
    // 200 on it, and JSON with no token in it. Reporting either as a clean run
    // would leave the caller believing the account had been renewed.
    for (mode, tag) in [(Mode::NotJson, "nonjson"), (Mode::NoToken, "notoken")] {
        let f = Fixture::new(tag, mode);
        let reply = f.refresh(REFRESH_SERVICE);
        let (_, message) = reply.as_error().unwrap_or_else(|| panic!("{tag}: {reply:?}"));
        assert!(
            message.contains("not JSON") || message.contains("no usable access_token"),
            "{tag}: {message}"
        );
        assert_eq!(f.value(ACCESS_SERVICE).as_deref(), Some(OLD_ACCESS), "{tag}");
        assert_eq!(f.value(REFRESH_SERVICE).as_deref(), Some(OLD_REFRESH), "{tag}");
    }
}

// ── the client the grant was issued to ─────────────────────────────────────

#[test]
fn the_client_id_stored_with_the_credential_wins_over_the_table() {
    // RFC 6749 §6 requires a refresh to present the same client the grant was
    // issued to. The table's entry is a DEFAULT — `apex cf connect --client-id`
    // lets somebody sign in as a different one — so the stored half has to win,
    // or a user who overrode the client would get `invalid_client` from the
    // far side and no explanation.
    //
    // Nothing in this build writes that field yet. It is tested now because
    // the field it reads is the store's own non-secret half, and the commit
    // that starts writing it should not also be the commit that discovers this
    // path was never exercised.
    let f = Fixture::with("stored-client", Mode::RenewOnly, REFRESH_SERVICE, Some("an-overridden-client"));
    let reply = f.refresh(REFRESH_SERVICE);
    let Response::Performed { exit_code, output, .. } = &reply else {
        panic!("{reply:?}");
    };
    assert_eq!(*exit_code, 0, "{output}");
    let seen = f.fake.seen();
    assert_eq!(
        seen[0].form.get("client_id").map(String::as_str),
        Some("an-overridden-client"),
        "the table's default was presented instead of the stored client"
    );
}

#[test]
fn an_authorisation_server_apex_has_no_client_at_is_refused_with_the_reason() {
    // Google and Microsoft are in the table and cannot be renewed by this
    // build: APEX registers no application at either and will not borrow
    // somebody else's, so `client_id` is `None` and the client the user signed
    // in with is not recorded anywhere. That is a real limit and the refusal
    // says so — including, for Google, that signing in again will not fix it,
    // because its flow also wants a client secret this daemon cannot hold.
    //
    // Checked against the shipped table rather than a synthetic one, so it
    // fails the day somebody adds a client id without adding a way to renew.
    for (host, also_needs_a_secret) in [
        ("oauth2.googleapis.com", true),
        ("login.microsoftonline.com", false),
    ] {
        let oauth = account::oauth_for_auth_host(host).expect("in the shipped table");
        assert_eq!(oauth.client_id, None, "{host} now ships a client id");
        let info = apex_secret_core::store::ServiceInfo {
            service: format!("account.x.y{}", ".refresh"),
            host: host.to_string(),
            scheme: "https".to_string(),
            username: "x-access-token".to_string(),
            path: String::new(),
            auth: "bearer".to_string(),
            port: None,
            added: 0,
        };
        let err = OAuthProvider::client_id(&info, oauth).expect_err("it found a client");
        let message = err.to_string();
        assert!(message.contains("no registered OAuth client"), "{host}: {message}");
        assert_eq!(
            message.contains("client secret"),
            also_needs_a_secret,
            "{host}: {message}"
        );
    }
}
