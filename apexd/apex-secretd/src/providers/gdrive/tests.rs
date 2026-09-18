//! The Drive provider, against a loopback double that reads the request that
//! actually arrived.
//!
//! # What this file can and cannot say
//!
//! The double below is not a Drive. It is an HTTP server that records the
//! method, the request target and the headers it was sent, and answers on
//! whether the `Authorization` header carries the token this fixture stored.
//! So what is checked here is the **wiring**, which is the part that can be
//! wrong in a way no reading of the code shows:
//!
//!   * the path requested is the stored endpoint's with `/files/<id>` under it;
//!   * the query is `alt=media`, which is what asks for content rather than
//!     metadata;
//!   * the token is presented as `Authorization: Bearer <token>`;
//!   * the method is `GET`;
//!   * and the bytes the far side sent are what the caller gets back.
//!
//! Each of those is a way for this provider to compose a request a real Drive
//! answers wrongly — a metadata document instead of a file, a 401 instead of a
//! file — and each one goes red here if it breaks.
//!
//! # Nothing here reaches Google
//!
//! There is no Google account on any machine this repository is built on. The
//! double is on `127.0.0.1`, the token is a literal in this file that
//! authorises nothing, and the only way a credential on `127.0.0.1` reaches
//! this provider is [`GdriveProvider::at`], which is `#[cfg(test)]`. What is
//! therefore NOT proven anywhere: that a real Google access token, obtained by
//! the real device grant and renewed by the real `oauth` provider, is accepted
//! by the real Drive.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use apex_secret_core::capability::CapabilityRecord;
use apex_secret_core::operation::Params;
use apex_secret_core::protocol::Response;
use apex_secret_core::store::Store;
use apex_secret_core::SecretValue;

use crate::peer::Peer;
use crate::provider::Registry;
use crate::service::{NewService, Service};

use super::*;

/// The access token this fixture stores. It authorises nothing anywhere.
const TOKEN: &str = "ya29.a0ExampleAccessTokenThatAuthorisesNothing";
/// A Drive file id shaped like a real one: base64url, 44 characters.
const FILE_ID: &str = "1BxiMVs0XRA5nFMdKvBdBZjgmUUqptlbs74OgvE2upms";
/// What the double hands back for a file it has.
const CONTENTS: &str = "the quick brown fox\n";
/// The account this fixture stores, in the store's own naming.
const SERVICE: &str = "account.google.work";
/// Where a Google account's credential is stored with its path pinned.
const DRIVE_PATH: &str = "/drive/v3";

/// One request the double saw.
#[derive(Debug, Clone)]
struct Seen {
    method: String,
    /// Path and query, as they arrived on the request line.
    target: String,
    headers: BTreeMap<String, String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// Holds exactly [`FILE_ID`], and answers 404 for anything else — which is
    /// what a `drive.file` scope looks like from the outside.
    Normal,
    /// The token has expired. Drive's own shape for it.
    Expired,
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
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).is_err() || line.trim().is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
        }
    }

    recorder.lock().expect("lock").push(Seen {
        method: method.clone(),
        target: target.clone(),
        headers: headers.clone(),
    });

    // The credential is checked against what ARRIVED, never against what the
    // provider meant to send. A request with no bearer, or with the token
    // spelled some other way, is a 401 here exactly as it would be at Drive.
    let presented = headers.get("authorization").map(String::as_str).unwrap_or("");
    let (path, query) = match target.split_once('?') {
        Some((path, query)) => (path, query),
        None => (target.as_str(), ""),
    };

    let (status, content_type, payload) = if presented != format!("Bearer {TOKEN}") {
        (
            "401 Unauthorized",
            "application/json",
            r#"{"error":{"code":401,"message":"Invalid Credentials","errors":[{"reason":"authError"}]}}"#
                .to_string(),
        )
    } else if mode == Mode::Expired {
        (
            "401 Unauthorized",
            "application/json",
            r#"{"error":{"code":401,"message":"Request had invalid authentication credentials."}}"#
                .to_string(),
        )
    } else if path != format!("{DRIVE_PATH}/files/{FILE_ID}") {
        (
            "404 Not Found",
            "application/json",
            r#"{"error":{"code":404,"message":"File not found."}}"#.to_string(),
        )
    } else if query != "alt=media" {
        // What a real Drive does when `alt=media` is missing: it answers with
        // the file's METADATA and a 200. A test that let that pass would be a
        // test that called a JSON stub "the file".
        (
            "200 OK",
            "application/json",
            format!(r#"{{"kind":"drive#file","id":"{FILE_ID}","name":"metadata-not-content"}}"#),
        )
    } else {
        ("200 OK", "text/plain", CONTENTS.to_string())
    };

    let reply = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n\r\n{payload}",
        payload.len()
    );
    let _ = stream.write_all(reply.as_bytes());
    let _ = stream.flush();
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
    fn new(name: &str, mode: Mode, granted: &[&str]) -> Fixture {
        Fixture::with(name, mode, granted, "127.0.0.1")
    }

    /// `host` is what the credential is PINNED to. The double always listens on
    /// loopback; a fixture that stores some other host is testing the host pin
    /// and expects to be refused before anything is sent.
    fn with(name: &str, mode: Mode, granted: &[&str], host: &str) -> Fixture {
        let fake = Fake::start(mode);
        let tag = format!("{name}-{}-{}", std::process::id(), fake.port);
        // /var/tmp, this project's rule for anything a suite creates.
        let store = PathBuf::from("/var/tmp").join(format!("apex-gdrive-store-{tag}"));
        let project = PathBuf::from("/var/tmp").join(format!("apex-gdrive-project-{tag}"));
        std::fs::remove_dir_all(&store).ok();
        std::fs::remove_dir_all(&project).ok();
        std::fs::create_dir_all(&project).expect("project");

        let mut registry = Registry::new();
        // The `#[cfg(test)]` route to a loopback host. `GdriveProvider::new()`
        // — the only constructor that ships — would refuse every request in
        // this file at `bind`, which is the point of it.
        registry
            .register(Box::new(GdriveProvider::at("127.0.0.1")))
            .expect("register");
        let service = Service::new(Store::new(store.clone()), false, registry);

        let peer = me();
        // `http` is accepted for a loopback host: nothing crosses a network.
        // A real account is stored `https` against `www.googleapis.com`.
        let scheme = if host == "127.0.0.1" { "http" } else { "https" };
        let port = if host == "127.0.0.1" {
            Some(fake.port)
        } else {
            None
        };
        assert_eq!(
            service.add(
                peer,
                NewService {
                    service: SERVICE,
                    host,
                    scheme,
                    username: Some("me@example.com"),
                    path: DRIVE_PATH,
                    auth: Some("bearer"),
                    port,
                },
                SecretValue::new(TOKEN.as_bytes().to_vec()),
            ),
            Response::Ok
        );
        for operation in granted {
            let granted = service.grant(
                peer,
                project.to_str().expect("utf8"),
                SERVICE,
                operation,
                false,
            );
            assert!(
                matches!(granted, Response::Grants { .. }),
                "granting {operation}: {granted:?}"
            );
        }
        Fixture {
            service,
            store,
            project,
            fake,
        }
    }

    fn record(&self, operation: &str, resource: &str) -> CapabilityRecord {
        let mut rec = CapabilityRecord::new(SERVICE, operation, resource);
        rec.project = Some(self.project.to_string_lossy().into_owned());
        rec
    }

    fn use_it(&self, record: CapabilityRecord) -> Response {
        self.service.use_capability(me(), record, Vec::new())
    }

    fn trail(&self) -> String {
        std::fs::read_to_string(Store::new(self.store.clone()).audit_path()).unwrap_or_default()
    }
}

// ── the declaration ────────────────────────────────────────────────────────

#[test]
fn the_declaration_is_one_a_registry_will_take() {
    SPEC.validate().expect("the gdrive provider must declare validly");
    // Every scope the account model offers for this transport must exist here.
    // The two halves are in different crates — the vocabulary a user is shown
    // and the operations a daemon will run — and a scope naming an operation
    // nothing implements is a grant that can never be used. The cross-crate
    // gate in `providers::tests` says the same thing over the whole table;
    // this says it for the provider whose file it is.
    for p in account::PROVIDERS.iter().filter(|p| p.transport == SPEC.id) {
        for scope in p.scopes {
            assert!(
                SPEC.operations.iter().any(|o| o.id == scope.operation),
                "{} offers scope {} -> {}, which this provider does not implement",
                p.id,
                scope.name,
                scope.operation
            );
        }
    }
}

#[test]
fn the_api_host_is_the_one_the_account_table_pins_google_to() {
    // Read off the table rather than spelled twice. A build where these
    // disagreed would refuse every account `apex account add google.<name>`
    // had itself just stored.
    assert_eq!(
        GdriveProvider::api_host().expect("a fixed host for google"),
        "www.googleapis.com"
    );
    let shipped = GdriveProvider::new();
    assert!(shipped.is_google("www.googleapis.com").expect("host"));
    // Case and the DNS root dot name the same host.
    assert!(shipped.is_google("WWW.googleapis.com.").expect("host"));
    // And the loopback a double runs on is NOT in the shipped table, which is
    // the whole reason `at()` exists.
    assert!(!shipped.is_google("127.0.0.1").expect("host"));
    assert!(GdriveProvider::at("127.0.0.1")
        .is_google("127.0.0.1")
        .expect("host"));
}

#[test]
fn a_resource_is_a_drive_file_id_and_nothing_that_could_leave_the_endpoint() {
    let op = &SPEC.operations[0];
    assert!(matches!(op.resource, ResourceKind::Name));
    let none = Params::new();
    assert!(op.check(FILE_ID, &none).is_ok());
    for escape in [
        "https://attacker.example/x",
        "../../etc/passwd",
        "a/b",
        "x?alt=json",
        "x#frag",
        "host:8080",
        "-oProxyCommand",
        "",
    ] {
        assert!(
            op.check(escape, &none).is_err(),
            "{escape:?} was accepted as a Drive file id"
        );
    }
    // And a parameter nobody declared is refused, not ignored.
    let mut smuggled = Params::new();
    smuggled.insert("alt".into(), "json".into());
    assert!(op.check(FILE_ID, &smuggled).is_err());
}

#[test]
fn the_url_is_the_stored_endpoint_with_the_file_id_under_it() {
    let info = apex_secret_core::store::ServiceInfo {
        service: SERVICE.into(),
        host: "www.googleapis.com".into(),
        scheme: "https".into(),
        username: "me@example.com".into(),
        path: DRIVE_PATH.into(),
        auth: "bearer".into(),
        port: None,
        added: 0,
    };
    assert_eq!(
        drive_url(&info, FILE_ID).expect("a file id"),
        format!("https://www.googleapis.com/drive/v3/files/{FILE_ID}?alt=media")
    );
    // A second, independent refusal of the same escapes. `OperationSpec::check`
    // runs first in production; this one runs in the function that actually
    // builds the string, because "the caller checked" is not a property that
    // function can see.
    for escape in ["../../../etc/passwd", "a/b", "x?alt=json", "-x", ""] {
        assert!(drive_url(&info, escape).is_err(), "{escape}");
    }
}

#[test]
fn no_operation_here_may_be_granted_everywhere() {
    // A file id resolves to a different file on every account, and the grant
    // is per account; nothing here earns `*`.
    for op in SPEC.operations {
        assert!(!op.same_everywhere, "{}", op.id);
        assert!(!op.supersedes_credentials, "{}", op.id);
    }
}

// ── the round trip ─────────────────────────────────────────────────────────

/// The whole of what this transport is for, and the four things about the
/// request that have to be right for a real Drive to answer it.
#[test]
fn a_granted_read_sends_a_bearer_get_for_alt_media_and_hands_back_the_file() {
    let f = Fixture::new("roundtrip", Mode::Normal, &["gdrive.file.read"]);

    let reply = f.use_it(f.record("gdrive.file.read", FILE_ID));
    let Response::Performed { exit_code, output: text, .. } = &reply else {
        panic!("a granted read did not produce output: {reply:?}");
    };
    assert_eq!(*exit_code, 0, "{text}");
    assert!(text.contains(CONTENTS.trim()), "the file did not come back: {text}");
    // Not the metadata document the double answers when `alt=media` is absent.
    assert!(
        !text.contains("metadata-not-content"),
        "this asked Drive for the file's metadata, not its contents: {text}"
    );

    let seen = f.fake.seen();
    assert_eq!(seen.len(), 1, "{seen:?}");
    let got = &seen[0];
    assert_eq!(got.method, "GET");
    assert_eq!(
        got.target,
        format!("{DRIVE_PATH}/files/{FILE_ID}?alt=media"),
        "the request target is the stored endpoint with the file id under it"
    );
    assert_eq!(
        got.headers.get("authorization").map(String::as_str),
        Some(format!("Bearer {TOKEN}").as_str()),
        "the token has to be presented as a bearer: {:?}",
        got.headers
    );

    // The credential is in no audit line. `Output`'s own text is the file, and
    // the file is not the token.
    assert!(!f.trail().contains(TOKEN), "the token reached the audit trail");
    assert!(!text.contains(TOKEN));
    // The audit line names the account and the file id, which is what makes it
    // worth reading.
    let trail = f.trail();
    assert!(trail.contains(FILE_ID), "{trail}");
    assert!(trail.contains(SERVICE), "{trail}");
}

/// A grant is per operation, and this provider offers exactly one.
#[test]
fn an_ungranted_read_never_reaches_drive() {
    let f = Fixture::new("ungranted", Mode::Normal, &[]);

    let reply = f.use_it(f.record("gdrive.file.read", FILE_ID));
    assert!(
        matches!(reply, Response::Error { .. }),
        "an ungranted read was performed: {reply:?}"
    );
    assert!(
        f.fake.seen().is_empty(),
        "a request reached the far side without a grant"
    );
    assert!(!f.trail().contains(TOKEN));
}

/// The pin that makes this Google's transport rather than any host's.
#[test]
fn a_credential_pinned_somewhere_other_than_google_is_refused_before_anything_is_sent() {
    // Stored for a host that is neither Google's nor the test injection's. The
    // framework's own pin cannot catch this — it only checks that the provider
    // agrees with the store, and here it would.
    let f = Fixture::with(
        "wronghost",
        Mode::Normal,
        &["gdrive.file.read"],
        "drive.attacker.example",
    );

    let reply = f.use_it(f.record("gdrive.file.read", FILE_ID));
    let Response::Error { message, .. } = &reply else {
        panic!("a Google token was spent at another host: {reply:?}");
    };
    assert!(
        message.contains("drive.attacker.example"),
        "the refusal has to name the host it refused: {message}"
    );
    assert!(
        message.contains("www.googleapis.com"),
        "and the host it would accept: {message}"
    );
    assert!(
        f.fake.seen().is_empty(),
        "a Google access token was sent to a host that is not Google"
    );
    assert!(!f.trail().contains(TOKEN));
}

/// A 401 is the far side's answer and travels, with the command that fixes it.
#[test]
fn an_expired_token_comes_back_as_googles_own_reason_and_replaces_nothing() {
    let f = Fixture::new("expired", Mode::Expired, &["gdrive.file.read"]);

    let reply = f.use_it(f.record("gdrive.file.read", FILE_ID));
    let Response::Performed { exit_code, output: text, .. } = &reply else {
        panic!("a 401 was not carried back as output: {reply:?}");
    };
    assert_eq!(*exit_code, 1, "a 401 must not read as success: {text}");
    assert!(text.contains("401"), "{text}");
    // Google's own words, not this build's guess at them.
    assert!(
        text.contains("invalid authentication credentials"),
        "the far side's reason has to travel: {text}"
    );
    // And the one command a person can act on. Derived from the service name,
    // so a credential that is not an account gets no suggestion.
    assert!(
        text.contains("apex account refresh google.work"),
        "a 401 on an account should name the renewal command: {text}"
    );
    assert!(!text.contains(TOKEN));
    assert!(!f.trail().contains(TOKEN));
}

/// What a `drive.file` scope looks like from the outside, and it is not a bug.
#[test]
fn a_file_this_client_did_not_create_is_a_404_that_says_so() {
    let f = Fixture::new("notfound", Mode::Normal, &["gdrive.file.read"]);

    let reply = f.use_it(f.record("gdrive.file.read", "1SomeOtherFileIdEntirely"));
    let Response::Performed { exit_code, output: text, .. } = &reply else {
        panic!("a 404 was not carried back as output: {reply:?}");
    };
    assert_eq!(*exit_code, 1, "{text}");
    assert!(text.contains("404"), "{text}");
    assert!(text.contains("File not found"), "{text}");
    // A 404 is about the FILE, so it must not send somebody to renew a token
    // that is working.
    assert!(
        !text.contains("apex account refresh"),
        "a 404 blamed the token: {text}"
    );
    // It did reach the far side, which is what tells a 404 apart from a
    // refusal: the request was well-formed and the file is not this client's.
    assert_eq!(f.fake.seen().len(), 1);
}

/// A file id the vocabulary refuses dies at `bind`, before the grant is spent.
#[test]
fn a_resource_that_is_not_a_file_id_is_refused_before_the_credential_is_read() {
    let f = Fixture::new("badid", Mode::Normal, &["gdrive.file.read"]);

    for bad in ["../../etc/passwd", "a/b", "x?alt=json"] {
        let reply = f.use_it(f.record("gdrive.file.read", bad));
        assert!(
            matches!(reply, Response::Error { .. }),
            "{bad} was accepted: {reply:?}"
        );
    }
    assert!(
        f.fake.seen().is_empty(),
        "a malformed file id reached the far side"
    );
    assert!(!f.trail().contains(TOKEN));
}
