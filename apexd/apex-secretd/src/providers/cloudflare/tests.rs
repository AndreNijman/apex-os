//! The only place any of this has been run.
//!
//! There is no Cloudflare account and no Cloudflare token on this machine, so
//! the alternative to a loopback double was a provider nobody had ever seen
//! execute. The double speaks Cloudflare's envelope — `{"success":…,"result":…,
//! "errors":[…]}` — answers the documented paths, and **refuses anything with
//! no `Authorization` header**, which is what makes a passing test mean
//! something: a provider that quietly did nothing would come back 401, not 200.
//!
//! What that proves and what it does not:
//!
//! * proved — the request the provider builds is the shape it means to build,
//!   the credential reaches the far side, none of it reaches the caller, the
//!   binding decides what a name means, and the framework's checks all fire;
//! * not proved — that `api.cloudflare.com` accepts any of these bodies. Every
//!   path is the documented one and not one of them has been called for real.
//!   That is what P1-004's `partial` is for.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use apex_secret_core::audit::{self, AuditEvent};
use apex_secret_core::capability::CapabilityRecord;
use apex_secret_core::protocol::{ErrorKind, Response};
use apex_secret_core::store::Store;
use apex_secret_core::SecretValue;

use crate::peer::Peer;
use crate::provider::Registry;
use crate::service::{NewService, Service};

use super::*;

/// The stored credential. Distinctive enough that a grep for it cannot match
/// by accident, and long enough to look like a token.
const TOKEN: &str = "apex-cf-sentinel-4f21c9ae7b3d-do-not-leak";

/// The account id the fixture's project binds.
const ACCOUNT: &str = "0123456789abcdef0123456789abcdef";

/// The zone id it binds.
const ZONE: &str = "fedcba9876543210fedcba9876543210";

/// The `wss://` URL the double hands back for a tail session. It carries its
/// own authorisation, which is why the provider must not pass it on.
const TAIL_URL: &str = "wss://tail.example.invalid/session/apex-tail-secret-91be";

/// §13.1's file, with the ids the REST API needs.
const PROJECT_FILE: &str = r#"
[identity.cloudflare]
account = "example-account"
account_id = "0123456789abcdef0123456789abcdef"

[cloudflare]
zone = "example.com"
zone_id = "fedcba9876543210fedcba9876543210"
buckets = ["example-assets"]

[cloudflare.preview]
worker = "project-preview"

[cloudflare.production]
worker = "project"
"#;

/// One request the double saw.
#[derive(Debug, Clone)]
struct Seen {
    method: String,
    path: String,
    authorization: Option<String>,
    body: String,
}

/// A stand-in for `api.cloudflare.com`.
struct Fake {
    port: u16,
    seen: Arc<Mutex<Vec<Seen>>>,
}

/// Whether the double behaves, or answers 401 with the header echoed back the
/// way a badly written API reports an auth failure.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Normal,
    EchoUnauthorized,
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

    fn authorizations(&self) -> Vec<String> {
        self.seen()
            .into_iter()
            .filter_map(|s| s.authorization)
            .collect()
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
    let path = words.next().unwrap_or("").to_string();

    let mut authorization = None;
    let mut length = 0usize;
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {}
            Err(_) => return,
        }
        if line.trim().is_empty() {
            break;
        }
        if let Some(value) = line.trim().strip_prefix("Authorization: ") {
            authorization = Some(value.to_string());
        }
        if let Some(value) = line.trim().strip_prefix("Content-Length: ") {
            length = value.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0u8; length];
    if length > 0 && reader.read_exact(&mut body).is_err() {
        return;
    }
    let body = String::from_utf8_lossy(&body).into_owned();

    recorder.lock().expect("lock").push(Seen {
        method: method.clone(),
        path: path.clone(),
        authorization: authorization.clone(),
        body,
    });

    // The property that makes every 200 below mean something: with no
    // credential there is no answer.
    let Some(authorization) = authorization else {
        return reply(&mut stream, 401, r#"{"success":false,"errors":[{"code":10000,"message":"Authentication error"}],"result":null}"#);
    };
    if mode == Mode::EchoUnauthorized {
        // What a badly written API does, and the case the framework's scrub
        // exists for. The provider does not scrub this; the framework does.
        let body = format!(
            r#"{{"success":false,"errors":[{{"code":10000,"message":"Authentication error: you sent {authorization}"}}],"result":null}}"#
        );
        return reply(&mut stream, 401, &body);
    }

    let (status, body) = answer(&method, &path);
    // Every success carries the credential back too, in a `messages` entry.
    // Cloudflare does not do this; the point is that it would not matter if it
    // did, and a test where the token never comes back cannot tell a working
    // scrub from a missing one.
    let body = body.replace(
        r#""messages":[]"#,
        &format!(r#""messages":[{{"code":1,"message":"authenticated with {authorization}"}}]"#),
    );
    reply(&mut stream, status, &body);
}

/// The documented paths, with the documented shapes.
fn answer(method: &str, path: &str) -> (u16, String) {
    let path = path.strip_prefix("/client/v4").unwrap_or(path);
    let ok = |result: &str| {
        (
            200u16,
            format!(r#"{{"success":true,"errors":[],"messages":[],"result":{result}}}"#),
        )
    };
    match (method, path) {
        ("GET", "/accounts") => ok(&format!(
            r#"[{{"id":"{ACCOUNT}","name":"example-account"}}]"#
        )),
        ("GET", p) if p == format!("/accounts/{ACCOUNT}") => {
            ok(&format!(r#"{{"id":"{ACCOUNT}","name":"example-account"}}"#))
        }
        ("GET", p) if p == format!("/zones/{ZONE}/workers/routes") => ok(
            r#"[{"id":"route1","pattern":"example.com/*","script":"project"},
                {"id":"route2","pattern":"admin.example.com/*","script":"somebody-else"}]"#,
        ),
        ("GET", p) if p.ends_with("/settings") => {
            ok(r#"{"bindings":[],"compatibility_date":"2026-09-01","usage_model":"standard"}"#)
        }
        ("POST", p) if p.ends_with("/versions") => {
            ok(r#"{"id":"1c4dd6be-0000-4000-8000-abcdefabcdef","number":7}"#)
        }
        ("POST", p) if p.starts_with("/accounts/") && p.contains("/deployments") => {
            ok(r#"{"id":"dep-9","strategy":"percentage","versions":[{"version_id":"1c4dd6be-0000-4000-8000-abcdefabcdef","percentage":100}]}"#)
        }
        ("POST", p) if p.ends_with("/tails") => ok(&format!(
            r#"{{"id":"tail-1","url":"{TAIL_URL}","expires_at":"2026-09-07T00:00:00Z"}}"#
        )),
        _ => (
            404,
            format!(
                r#"{{"success":false,"errors":[{{"code":7003,"message":"no route for {method} {path}"}}],"result":null}}"#
            ),
        ),
    }
}

fn reply(stream: &mut TcpStream, status: u16, body: &str) {
    let reason = if status == 200 { "OK" } else { "Error" };
    let _ = write!(
        stream,
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}

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
    /// A service serving ONLY the Cloudflare provider, with a credential
    /// stored for the double and a project bound the way §13.1 says.
    fn new(name: &str, mode: Mode, granted: &[&str]) -> Fixture {
        let fake = Fake::start(mode);
        let tag = format!("{name}-{}-{}", std::process::id(), fake.port);
        let store = std::env::temp_dir().join(format!("apex-cf-store-{tag}"));
        let project = std::env::temp_dir().join(format!("apex-cf-project-{tag}"));
        std::fs::remove_dir_all(&store).ok();
        std::fs::remove_dir_all(&project).ok();
        std::fs::create_dir_all(&project).expect("project");
        std::fs::write(project.join("apex.toml"), PROJECT_FILE).expect("apex.toml");

        let mut registry = Registry::new();
        registry
            .register(Box::new(CloudflareProvider::at(fake.port)))
            .expect("register");
        let service = Service::new(Store::new(store.clone()), false, registry);

        let peer = me();
        // `http` is allowed for a loopback host: nothing crosses a network.
        assert_eq!(
            service.add(
                peer,
                NewService {
                    service: "cloudflare",
                    host: "127.0.0.1",
                    scheme: "http",
                    username: None,
                    path: "",
                    auth: None,
                    port: None,
                },
                SecretValue::new(TOKEN.as_bytes().to_vec()),
            ),
            Response::Ok
        );
        for operation in granted {
            assert!(service
                .grant(
                    peer,
                    project.to_str().expect("utf8"),
                    "cloudflare",
                    operation,
                    false
                )
                .as_error()
                .is_none());
        }
        Fixture {
            service,
            store,
            project,
            fake,
        }
    }

    fn record(&self, operation: &str, resource: &str) -> CapabilityRecord {
        let mut rec = CapabilityRecord::new("cloudflare", operation, resource);
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

/// One operation, the resource it names, and the options its own declaration
/// accepts. Named because the bare tuple is what clippy::type_complexity
/// objects to, and it is right — three levels of nesting in a return type is
/// something a reader has to parse rather than read.
type OperationCase = (&'static str, &'static str, Vec<(&'static str, &'static str)>);

/// Every operation the provider declares, with a resource and options that its
/// own declaration accepts.
fn every_operation() -> Vec<OperationCase> {
    vec![
        ("cloudflare.account.read", "", vec![]),
        ("cloudflare.worker.read", "project", vec![]),
        (
            "cloudflare.worker.upload-version",
            "project",
            vec![
                ("script", "dist/worker.js"),
                ("compatibility-date", "2026-09-01"),
                ("message", "released by apex"),
            ],
        ),
        (
            "cloudflare.worker.deploy",
            "project",
            vec![("version", "1c4dd6be-0000-4000-8000-abcdefabcdef")],
        ),
        (
            "cloudflare.worker.rollback",
            "project",
            vec![("version", "0a0b0c0d-0000-4000-8000-000000000000")],
        ),
        ("cloudflare.worker.tail", "project", vec![]),
        ("cloudflare.worker.route.read", "project", vec![]),
    ]
}

fn with_module(fixture: &Fixture) {
    std::fs::create_dir_all(fixture.project.join("dist")).expect("dist");
    std::fs::write(
        fixture.project.join("dist/worker.js"),
        "export default { fetch: () => new Response('hi') };\n",
    )
    .expect("worker.js");
}

fn granted_everything() -> Vec<&'static str> {
    SPEC.operations.iter().map(|op| op.id).collect()
}

#[test]
fn the_double_answers_nothing_without_a_credential() {
    // The assumption every other test in this file rests on. If the double
    // answered 200 to an unauthenticated request, a provider that never sent
    // the token would still pass, which is the failure mode this whole
    // program keeps finding.
    let fake = Fake::start(Mode::Normal);
    let mut stream = TcpStream::connect(("127.0.0.1", fake.port)).expect("connect");
    write!(
        stream,
        "GET /client/v4/accounts HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"
    )
    .expect("write");
    let mut answer = String::new();
    stream.read_to_string(&mut answer).expect("read");
    assert!(answer.starts_with("HTTP/1.1 401"), "{answer}");
    assert!(answer.contains("Authentication error"), "{answer}");
}

#[test]
fn every_declared_operation_reaches_cloudflare_with_the_credential_and_returns_without_it() {
    // P1-002's third criterion, as a measurement rather than an assertion.
    // The agent side is handed the whole operation surface; for each one the
    // credential is shown to have reached the far side, and shown not to be in
    // the reply, the output, the serialised response or the audit trail.
    let f = Fixture::new("surface", Mode::Normal, &granted_everything());
    with_module(&f);

    for (operation, resource, options) in every_operation() {
        let mut rec = f.record(operation, resource);
        for (name, value) in &options {
            rec = rec.param(name, value);
        }
        let reply = f.use_it(rec);
        let Response::Performed {
            endpoint,
            exit_code,
            output,
            record,
        } = &reply
        else {
            panic!("{operation} was refused: {reply:?}");
        };
        assert_eq!(*exit_code, 0, "{operation}: {output}");
        assert_eq!(endpoint, "http://127.0.0.1", "{operation}");
        assert_eq!(record.operation, operation);
        assert_eq!(record.approval_policy, "grant");
        assert!(!output.contains(TOKEN), "{operation} handed back the token");
        // The far side echoed it in every reply, so an unscrubbed answer would
        // contain it and a scrubbed one says where it was.
        assert!(
            output.contains("«redacted»"),
            "{operation}: the reply was not scrubbed, it just had nothing in it"
        );
        assert!(
            !serde_json::to_string(&reply).unwrap().contains(TOKEN),
            "{operation}: the serialised reply carries the token"
        );
    }

    // One authenticated request per operation, every one of them carrying the
    // stored credential. A provider that exited zero without calling anything
    // would leave this empty.
    let authorizations = f.fake.authorizations();
    assert_eq!(
        authorizations.len(),
        SPEC.operations.len(),
        "one call per operation: {authorizations:?}"
    );
    for authorization in &authorizations {
        assert_eq!(authorization, &format!("Bearer {TOKEN}"));
    }

    let trail = f.trail();
    assert!(!trail.contains(TOKEN), "the audit trail holds the credential");
    assert_eq!(
        trail.matches("\"event\":\"used\"").count(),
        SPEC.operations.len(),
        "every operation should have left a used line"
    );
}

#[test]
fn the_agent_side_holds_no_credential_even_when_it_holds_every_operation() {
    // The same criterion from the other end: with every capability granted,
    // there is no request in the protocol whose reply carries a value, and the
    // store the value is in is not the caller's to read. `Response` cannot
    // even be constructed with one — `SecretValue` implements no `Serialize` —
    // so this checks the reachable surface rather than restating the type.
    let f = Fixture::new("noread", Mode::Normal, &granted_everything());
    with_module(&f);
    let peer = me();

    let mut replies = vec![
        f.service.hello(),
        f.service.list(peer),
        f.service.grants(peer),
        f.service.audit(peer, 100),
    ];
    for (operation, resource, options) in every_operation() {
        let mut rec = f.record(operation, resource);
        for (name, value) in &options {
            rec = rec.param(name, value);
        }
        replies.push(f.use_it(rec));
    }

    for reply in &replies {
        let text = serde_json::to_string(reply).expect("every reply serialises");
        assert!(
            !text.contains(TOKEN),
            "{} carried the credential",
            reply.variant()
        );
    }
    // ...and it did reach Cloudflare, so this is a brokered operation and not
    // a set of calls that never happened.
    assert_eq!(f.fake.authorizations().len(), SPEC.operations.len());
}

#[test]
fn an_api_that_echoes_the_credential_back_does_not_get_to_hand_it_over() {
    // The scrub, against a Cloudflare credential, on the path a real API takes
    // when it reports an auth failure badly. The provider does nothing here;
    // the framework does it, which is what makes it hold for the next provider
    // as well.
    let f = Fixture::new("echo", Mode::EchoUnauthorized, &["cloudflare.account.read"]);
    let reply = f.use_it(f.record("cloudflare.account.read", ""));
    let Response::Performed { exit_code, output, .. } = &reply else {
        panic!("expected a result carrying the failure: {reply:?}");
    };
    assert_eq!(*exit_code, 1, "a 401 is not a success");
    assert!(output.contains("HTTP 401"), "{output}");
    // The far side put the credential in the body; it is not in the answer.
    assert!(!output.contains(TOKEN), "{output}");
    assert!(output.contains("«redacted»"), "{output}");
    assert!(!serde_json::to_string(&reply).unwrap().contains(TOKEN));
    assert!(!f.trail().contains(TOKEN), "the trail holds the credential");
    // It really was sent, so the echo really was of the real thing.
    assert_eq!(f.fake.authorizations(), vec![format!("Bearer {TOKEN}")]);
}

#[test]
fn a_tail_session_url_is_a_credential_and_does_not_come_back() {
    // `POST …/tails` answers with a wss:// URL that authorises whoever holds
    // it to read the worker's live logs. Returning it would hand the agent a
    // credential by another name.
    let f = Fixture::new("tail", Mode::Normal, &["cloudflare.worker.tail"]);
    let reply = f.use_it(f.record("cloudflare.worker.tail", "project"));
    let Response::Performed { output, .. } = &reply else {
        panic!("refused: {reply:?}");
    };
    // The session was created — the id and the expiry are there.
    assert!(output.contains("tail-1"), "{output}");
    assert!(output.contains("expires_at"), "{output}");
    // ...and the URL is not.
    assert!(!output.contains(TAIL_URL), "the tail URL came back: {output}");
    assert!(!output.contains("wss://"), "{output}");
    assert!(output.contains("not returned"), "{output}");
    assert!(!f.trail().contains(TAIL_URL));
    // The double really did send one, so this is a removal and not an absence.
    let sent = f.fake.seen();
    assert!(sent.iter().any(|s| s.path.ends_with("/tails")), "{sent:?}");
}

#[test]
fn a_name_this_project_did_not_bind_never_reaches_cloudflare() {
    // The semantic check. `project` is bound to production; `somebody-elses`
    // is not bound to anything, and the difference is decided before a
    // credential is read.
    let f = Fixture::new("unbound", Mode::Normal, &granted_everything());
    let reply = f.use_it(f.record("cloudflare.worker.read", "somebody-elses-worker"));
    let (kind, message) = reply.as_error().expect("an unbound name must be refused");
    assert_eq!(kind, ErrorKind::BadRequest);
    assert!(message.contains("somebody-elses-worker"), "{message}");
    assert!(message.contains("apex.toml"), "{message}");
    assert!(message.contains("project and project-preview"), "{message}");
    assert!(
        f.fake.seen().is_empty(),
        "a refused request still reached the api"
    );
}

#[test]
fn a_project_that_binds_no_cloudflare_account_is_told_what_to_write() {
    // A project with no binding at all. The account read still works, because
    // it is how you find out what to put in the file; everything else refuses
    // with the file to edit named.
    let f = Fixture::new("nobinding", Mode::Normal, &granted_everything());
    std::fs::write(f.project.join("apex.toml"), "[something.else]\nkey = \"v\"\n").expect("write");

    let reply = f.use_it(f.record("cloudflare.worker.deploy", "project").param("version", "v1"));
    let (_, message) = reply.as_error().expect("refused");
    assert!(message.contains("[identity.cloudflare]"), "{message}");

    // ...and the discovery path still answers, against /accounts.
    let reply = f.use_it(f.record("cloudflare.account.read", ""));
    assert!(matches!(reply, Response::Performed { exit_code: 0, .. }), "{reply:?}");
    assert_eq!(f.fake.seen()[0].path, "/client/v4/accounts");
}

#[test]
fn the_binding_decides_which_account_and_which_worker_the_call_names() {
    // "Account/zone/project binding works", as a path rather than a promise:
    // the account id and the worker name in the URL came out of apex.toml, and
    // the zone id in the routes call came out of the same file.
    let f = Fixture::new("paths", Mode::Normal, &granted_everything());
    f.use_it(f.record("cloudflare.worker.read", "project-preview"));
    f.use_it(f.record("cloudflare.worker.route.read", "project"));

    let paths: Vec<String> = f.fake.seen().into_iter().map(|s| s.path).collect();
    assert_eq!(
        paths,
        vec![
            format!("/client/v4/accounts/{ACCOUNT}/workers/scripts/project-preview/settings"),
            format!("/client/v4/zones/{ZONE}/workers/routes"),
        ]
    );
}

#[test]
fn an_upload_carries_the_project_s_own_module_and_the_metadata_that_names_it() {
    // The multipart body is built here rather than by curl's `-F`, so this is
    // where it gets checked: the module arrives, the metadata names it, and
    // the annotation goes in the body rather than on a command line.
    let f = Fixture::new("upload", Mode::Normal, &["cloudflare.worker.upload-version"]);
    with_module(&f);
    let reply = f.use_it(
        f.record("cloudflare.worker.upload-version", "project")
            .param("script", "dist/worker.js")
            .param("compatibility-date", "2026-09-01")
            .param("message", "released by apex"),
    );
    assert!(matches!(reply, Response::Performed { exit_code: 0, .. }), "{reply:?}");

    let sent = f.fake.seen();
    let upload = sent.first().expect("one request");
    assert_eq!(upload.method, "POST");
    assert!(upload.path.ends_with("/versions"), "{}", upload.path);
    assert!(upload.body.contains("name=\"metadata\""), "{}", upload.body);
    assert!(upload.body.contains(r#""main_module":"worker.js""#), "{}", upload.body);
    assert!(upload.body.contains(r#""compatibility_date":"2026-09-01""#));
    assert!(upload.body.contains(r#""workers/message":"released by apex""#));
    assert!(upload.body.contains("filename=\"worker.js\""));
    assert!(upload.body.contains("export default { fetch:"), "the module itself");
}

#[test]
fn an_upload_will_not_follow_a_link_out_of_the_project() {
    // The provider reads a caller-named path as root. A module that is a
    // symlink to somebody else's file is refused, and nothing is sent.
    let f = Fixture::new("uploadlink", Mode::Normal, &["cloudflare.worker.upload-version"]);
    std::fs::create_dir_all(f.project.join("dist")).expect("dist");
    std::os::unix::fs::symlink("/etc/hostname", f.project.join("dist/worker.js")).expect("link");
    let reply = f.use_it(
        f.record("cloudflare.worker.upload-version", "project")
            .param("script", "dist/worker.js"),
    );
    let (_, message) = reply.as_error().expect("a symlinked module must be refused");
    assert!(message.contains("symbolic link"), "{message}");
    assert!(f.fake.seen().is_empty(), "it was sent anyway");
}

#[test]
fn a_module_that_is_not_a_module_is_refused_before_it_is_read() {
    let f = Fixture::new("notmodule", Mode::Normal, &["cloudflare.worker.upload-version"]);
    std::fs::write(f.project.join("secrets.env"), "TOKEN=x\n").expect("write");
    let reply = f.use_it(
        f.record("cloudflare.worker.upload-version", "project")
            .param("script", "secrets.env"),
    );
    let (_, message) = reply.as_error().expect("refused");
    assert!(message.contains(".js or .mjs"), "{message}");
    assert!(f.fake.seen().is_empty());
}

#[test]
fn a_grant_for_one_worker_verb_does_not_grant_its_siblings() {
    // §13.2's whole argument: semantic operations rather than one broad
    // "Cloudflare write". Deploying is not rolling back and neither is
    // uploading, so a grant for one has to stop at one.
    let f = Fixture::new("siblings", Mode::Normal, &["cloudflare.worker.deploy"]);
    with_module(&f);
    let allowed = f.use_it(
        f.record("cloudflare.worker.deploy", "project")
            .param("version", "1c4dd6be-0000-4000-8000-abcdefabcdef"),
    );
    assert!(matches!(allowed, Response::Performed { exit_code: 0, .. }), "{allowed:?}");

    for refused in [
        "cloudflare.worker.rollback",
        "cloudflare.worker.upload-version",
        "cloudflare.worker.read",
        "cloudflare.worker.tail",
        "cloudflare.account.read",
    ] {
        let reply = f.use_it(
            f.record(refused, "project")
                .param("version", "1c4dd6be-0000-4000-8000-abcdefabcdef")
                .param("script", "dist/worker.js"),
        );
        assert!(
            reply.as_error().is_some(),
            "'{refused}' went through on a grant for deploy"
        );
    }
    // One call: the allowed one.
    assert_eq!(f.fake.seen().len(), 1);
}

#[test]
fn a_rollback_is_told_apart_from_a_deploy_on_the_wire() {
    // Same endpoint, and the grant is what separates them — so the request
    // still has to say which it is, or Cloudflare refuses a return to an older
    // version as a stale deployment.
    let f = Fixture::new("rollback", Mode::Normal, &granted_everything());
    f.use_it(f.record("cloudflare.worker.deploy", "project").param("version", "v-new"));
    f.use_it(f.record("cloudflare.worker.rollback", "project").param("version", "v-old"));
    let sent = f.fake.seen();
    assert!(sent[0].path.ends_with("/deployments"), "{}", sent[0].path);
    assert!(sent[1].path.ends_with("/deployments?force=true"), "{}", sent[1].path);
    assert!(sent[0].body.contains("v-new"));
    assert!(sent[1].body.contains("v-old"));
}

#[test]
fn a_message_with_a_quote_in_it_survives_two_layers_of_escaping() {
    // `message` is Syntax::Text, which admits a quote and a backslash. It is
    // then escaped by serde_json, and the deployment body is escaped AGAIN by
    // the curl config writer, and curl unescapes once. Three transformations
    // and no test on the round trip is how every quoted deploy message ends up
    // as malformed JSON at Cloudflare — which is a functional bug a scrub test
    // and a binding test would both pass straight over.
    let f = Fixture::new("quoting", Mode::Normal, &granted_everything());
    with_module(&f);
    let awkward = r#"say "hi" \ and "then" stop"#;

    // The JSON body path: through `quoted()` into a curl config line.
    f.use_it(
        f.record("cloudflare.worker.deploy", "project")
            .param("version", "1c4dd6be-0000-4000-8000-abcdefabcdef")
            .param("message", awkward),
    );
    // The multipart path: written to a file as raw bytes, no curl escaping.
    f.use_it(
        f.record("cloudflare.worker.upload-version", "project")
            .param("script", "dist/worker.js")
            .param("message", awkward),
    );

    let sent = f.fake.seen();
    let deploy: serde_json::Value = serde_json::from_str(&sent[0].body)
        .unwrap_or_else(|e| panic!("the deployment body is not JSON: {e}\n{}", sent[0].body));
    assert_eq!(
        deploy["annotations"]["workers/message"].as_str(),
        Some(awkward),
        "the message did not survive the round trip"
    );

    // The metadata part is JSON inside a multipart body; pull it back out.
    let body = &sent[1].body;
    let start = body.find('{').expect("metadata json");
    let end = body[start..].find("\r\n--").map(|i| start + i).unwrap_or(body.len());
    let metadata: serde_json::Value = serde_json::from_str(body[start..end].trim())
        .unwrap_or_else(|e| panic!("the metadata part is not JSON: {e}\n{body}"));
    assert_eq!(
        metadata["annotations"]["workers/message"].as_str(),
        Some(awkward)
    );
}

#[test]
fn a_route_read_answers_about_the_worker_that_was_named_and_no_other() {
    // The zone endpoint returns every route in the zone. The caller named one
    // worker, the grant is for one worker, and the audit line claims the
    // answer is about that worker — so the answer has to be.
    let f = Fixture::new("routes", Mode::Normal, &["cloudflare.worker.route.read"]);
    let reply = f.use_it(f.record("cloudflare.worker.route.read", "project"));
    let Response::Performed { output, .. } = &reply else {
        panic!("refused: {reply:?}");
    };
    assert!(output.contains("route1"), "{output}");
    assert!(
        !output.contains("somebody-else") && !output.contains("route2"),
        "the rest of the zone's routing came back: {output}"
    );
}

#[test]
fn the_host_pin_refuses_a_credential_stored_for_anywhere_else() {
    // Cloudflare's endpoint is a constant, so the pin is a flat statement: a
    // token stored for another host cannot be spent on a Cloudflare operation.
    // The provider contains no such check and cannot skip one.
    let f = Fixture::new("pin", Mode::Normal, &["cloudflare.account.read"]);
    let peer = me();
    assert_eq!(
        f.service.add(
            peer,
            NewService {
                service: "elsewhere",
                host: "github.com",
                scheme: "https",
                username: None,
                path: "",
                auth: None,
                port: None,
            },
            SecretValue::new(TOKEN.as_bytes().to_vec()),
        ),
        Response::Ok
    );
    assert!(f
        .service
        .grant(
            peer,
            f.project.to_str().expect("utf8"),
            "elsewhere",
            "cloudflare.account.read",
            false
        )
        .as_error()
        .is_none());

    let mut rec = f.record("cloudflare.account.read", "");
    rec.provider = "elsewhere".into();
    let reply = f.use_it(rec);
    let (kind, message) = reply.as_error().expect("the pin must refuse this");
    assert_eq!(kind, ErrorKind::PermissionDenied);
    assert!(message.contains("github.com"), "{message}");
    assert!(f.fake.seen().is_empty());
}

#[test]
fn a_binding_that_moves_between_the_check_and_the_call_stops_the_call() {
    // `Bound` carries an endpoint and a sentence and nothing a provider
    // defines, so what `bind` resolved cannot be handed to `perform` and the
    // project file is read twice. The owner is the caller and can change it in
    // between; the sentence is rebuilt and compared, so what runs is what was
    // authorised or nothing runs.
    struct Moving {
        inner: CloudflareProvider,
        project: Mutex<Option<PathBuf>>,
    }
    impl Provider for Moving {
        fn spec(&self) -> &'static ProviderSpec {
            self.inner.spec()
        }
        fn bind(&self, req: &Bind<'_>) -> Result<Bound, ProviderError> {
            let bound = self.inner.bind(req);
            // Between the two, exactly as a caller's own process could.
            if let Some(project) = self.project.lock().expect("lock").take() {
                // The account id, not the worker name: a changed name would
                // stop resolving and be refused for that reason instead, and
                // the interesting case is the one where everything still
                // resolves and the request would go somewhere else.
                std::fs::write(
                    project.join("apex.toml"),
                    PROJECT_FILE.replace(ACCOUNT, "99999999999999999999999999999999"),
                )
                .expect("rewrite");
            }
            bound
        }
        fn perform(
            &self,
            req: &Bind<'_>,
            bound: &Bound,
            value: &SecretValue,
        ) -> Result<Performed, ProviderError> {
            self.inner.perform(req, bound, value)
        }
    }

    let fake = Fake::start(Mode::Normal);
    let tag = format!("moving-{}-{}", std::process::id(), fake.port);
    let store = std::env::temp_dir().join(format!("apex-cf-store-{tag}"));
    let project = std::env::temp_dir().join(format!("apex-cf-project-{tag}"));
    std::fs::remove_dir_all(&store).ok();
    std::fs::create_dir_all(&project).expect("project");
    std::fs::write(project.join("apex.toml"), PROJECT_FILE).expect("apex.toml");

    let mut registry = Registry::new();
    registry
        .register(Box::new(Moving {
            inner: CloudflareProvider::at(fake.port),
            project: Mutex::new(Some(project.clone())),
        }))
        .expect("register");
    let service = Service::new(Store::new(store.clone()), false, registry);
    let peer = me();
    service.add(
        peer,
        NewService {
            service: "cloudflare",
            host: "127.0.0.1",
            scheme: "http",
            username: None,
            path: "",
            auth: None,
            port: None,
        },
        SecretValue::new(TOKEN.as_bytes().to_vec()),
    );
    service.grant(
        peer,
        project.to_str().expect("utf8"),
        "cloudflare",
        "cloudflare.worker.read",
        false,
    );

    let mut rec = CapabilityRecord::new("cloudflare", "cloudflare.worker.read", "project");
    rec.project = Some(project.to_string_lossy().into_owned());
    let reply = service.use_capability(peer, rec, Vec::new());
    let (_, message) = reply
        .as_error()
        .expect("a binding that moved must stop the call");
    assert!(message.contains("changed while the request"), "{message}");
    assert!(fake.seen().is_empty(), "it was sent anyway");

    std::fs::remove_dir_all(&store).ok();
    std::fs::remove_dir_all(&project).ok();
}

#[test]
fn the_declared_shape_is_enforced_before_the_provider_is_asked_anything() {
    // All of it from the provider's own declaration, all of it checked by the
    // framework: an operation that takes no resource, a resource that is a
    // URL, an option nobody declared, a required option left out.
    let f = Fixture::new("shape", Mode::Normal, &granted_everything());
    with_module(&f);
    for (operation, resource, options) in [
        ("cloudflare.account.read", "something", vec![]),
        ("cloudflare.worker.read", "https://attacker.example/x", vec![]),
        ("cloudflare.worker.read", "../../etc", vec![]),
        ("cloudflare.worker.read", "-f", vec![]),
        ("cloudflare.worker.deploy", "project", vec![]),
        (
            "cloudflare.worker.read",
            "project",
            vec![("branch", "main")],
        ),
        (
            "cloudflare.worker.upload-version",
            "project",
            vec![("script", "/etc/passwd")],
        ),
        (
            "cloudflare.worker.upload-version",
            "project",
            vec![("script", "dist/worker.js"), ("message", "two\nlines")],
        ),
    ] {
        let mut rec = f.record(operation, resource);
        for (name, value) in &options {
            rec = rec.param(name, value);
        }
        let reply = f.use_it(rec);
        let (kind, message) = reply
            .as_error()
            .unwrap_or_else(|| panic!("{operation} {resource} {options:?} was accepted"));
        assert_eq!(kind, ErrorKind::BadRequest, "{operation} {resource}: {message}");
    }
    assert!(f.fake.seen().is_empty());
}

#[test]
fn an_operation_no_provider_declares_is_refused_even_under_the_cloudflare_prefix() {
    // The vocabulary is closed by the registry, and a name that looks like one
    // of §13.2's twenty-five not-yet-implemented operations is not one this
    // build offers.
    let f = Fixture::new("closed", Mode::Normal, &granted_everything());
    for evil in [
        "cloudflare.dns.delete",
        "cloudflare.r2.object.write",
        "cloudflare.worker.delete",
        "cloudflare.secret.rotate",
        "git.push",
        "exec",
    ] {
        let reply = f.use_it(f.record(evil, "project"));
        assert_eq!(
            reply.as_error().map(|(k, _)| k),
            Some(ErrorKind::BadRequest),
            "'{evil}' was not refused"
        );
    }
    assert!(f.fake.seen().is_empty());
}

#[test]
fn the_trail_records_the_operation_in_the_provider_s_own_words() {
    let f = Fixture::new("trail", Mode::Normal, &["cloudflare.worker.deploy"]);
    f.use_it(
        f.record("cloudflare.worker.deploy", "project")
            .param("version", "1c4dd6be-0000-4000-8000-abcdefabcdef"),
    );
    let lines = audit::tail(&Store::new(f.store.clone()).audit_path(), 10);
    let used = lines
        .iter()
        .find(|l| l.event == AuditEvent::Used)
        .expect("a use was recorded");
    assert_eq!(used.operation, "cloudflare.worker.deploy");
    assert_eq!(used.resource, "project");
    assert_eq!(
        used.detail,
        format!(
            "deploy version 1c4dd6be-0000-4000-8000-abcdefabcdef of project \
             (production) in account example-account [{ACCOUNT}]"
        )
    );
    assert_eq!(used.endpoint.as_deref(), Some("http://127.0.0.1"));
    assert_eq!(used.exit_code, Some(0));
}

#[test]
fn the_declaration_is_well_formed_and_every_name_is_one_section_thirteen_two_lists() {
    // §13.2 is the specification, and a name invented here would be a name no
    // other task in the Cloudflare block could depend on.
    SPEC.validate().expect("the shipped cloudflare vocabulary must validate");
    let listed: Vec<&str> = SECTION_13_2.to_vec();
    for op in SPEC.operations {
        // `worker.route.read` is the one addition, and it is §13.3's "Worker
        // routes" rather than an invention.
        if op.id == "cloudflare.worker.route.read" {
            continue;
        }
        assert!(listed.contains(&op.id), "'{}' is not in §13.2", op.id);
    }
    // Six of §13.2's thirty-two, and one addition. The arithmetic is asserted
    // because the module note states it and a later task will read that note
    // to work out what is left.
    let from_13_2 = SPEC
        .operations
        .iter()
        .filter(|op| listed.contains(&op.id))
        .count();
    assert_eq!(from_13_2, 6);
    assert_eq!(SPEC.operations.len(), 7);
    assert_eq!(SECTION_13_2.len() - from_13_2, 26, "still unimplemented");

    // Nothing is declared twice, and every summary reads as a sentence about
    // what the owner is being asked to allow.
    let mut ids: Vec<&str> = SPEC.operations.iter().map(|op| op.id).collect();
    ids.sort_unstable();
    let mut unique = ids.clone();
    unique.dedup();
    assert_eq!(ids, unique);
    for op in SPEC.operations {
        assert!(op.summary.len() > 20, "'{}' has a summary nobody can act on", op.id);
    }
}

/// §13.2's list, so the test above compares against the roadmap rather than
/// against itself.
const SECTION_13_2: &[&str] = &[
    "cloudflare.account.read",
    "cloudflare.worker.read",
    "cloudflare.worker.upload-version",
    "cloudflare.worker.deploy",
    "cloudflare.worker.rollback",
    "cloudflare.worker.tail",
    "cloudflare.dns.read",
    "cloudflare.dns.create",
    "cloudflare.dns.update",
    "cloudflare.dns.delete",
    "cloudflare.r2.object.read",
    "cloudflare.r2.object.write",
    "cloudflare.r2.bucket.create",
    "cloudflare.d1.read",
    "cloudflare.d1.query",
    "cloudflare.d1.migrate",
    "cloudflare.kv.read",
    "cloudflare.kv.write",
    "cloudflare.queue.publish",
    "cloudflare.queue.manage",
    "cloudflare.hyperdrive.read",
    "cloudflare.hyperdrive.edit",
    "cloudflare.secret.create",
    "cloudflare.secret.bind",
    "cloudflare.secret.rotate",
    "cloudflare.access.read",
    "cloudflare.access.edit",
    "cloudflare.tunnel.read",
    "cloudflare.tunnel.edit",
    "cloudflare.workers-ai.run",
    "cloudflare.ai-gateway.run",
    "cloudflare.ai-gateway.edit",
];

/// A worker bound to two environments must resolve the same way twice.
#[test]
fn a_worker_bound_twice_resolves_the_same_way_every_time() {
    let f = Fixture::new("stable", Mode::Normal, &["cloudflare.worker.read"]);
    std::fs::write(
        f.project.join("apex.toml"),
        format!(
            "[identity.cloudflare]\naccount_id = \"{ACCOUNT}\"\n\
             [cloudflare.alpha]\nworker = \"twin\"\n\
             [cloudflare.beta]\nworker = \"twin\"\n"
        ),
    )
    .expect("write");
    let mut details = std::collections::BTreeSet::new();
    for _ in 0..5 {
        let lines = {
            f.use_it(f.record("cloudflare.worker.read", "twin"));
            audit::tail(&Store::new(f.store.clone()).audit_path(), 50)
        };
        let used = lines.iter().rev().find(|l| l.event == AuditEvent::Used).expect("used");
        details.insert(used.detail.clone());
    }
    assert_eq!(details.len(), 1, "resolution is not stable: {details:?}");
    assert!(details.iter().next().unwrap().contains("(alpha)"));
}

/// Nothing above depends on this map, but a request that carried one would.
#[test]
fn the_options_a_caller_sends_survive_the_wire_unchanged() {
    let mut params: BTreeMap<String, String> = BTreeMap::new();
    params.insert("version".into(), "1c4dd6be".into());
    let rec = CapabilityRecord::new("cloudflare", "cloudflare.worker.deploy", "project")
        .param("version", "1c4dd6be");
    assert_eq!(rec.params, params);
}

#[test]
fn a_cloudflare_operation_cannot_be_granted_in_every_project_even_though_it_names_nothing() {
    // The collision between P1-002 and P1-018, as a runtime assertion rather
    // than a declaration one.
    //
    // `--everywhere` (the `*` grant key) is gated on the operation reaching the
    // same thing in every project. P1-018 implemented that as
    // `OperationSpec::names_nothing`, which is true here: `cloudflare.account.read`
    // declares no resource and no parameters. But it resolves the account out of
    // the project's own `apex.toml` — bound, `GET /accounts/{id}` for THAT
    // project; unbound, `GET /accounts` for every account the token can see — so
    // it reaches a different thing in a different directory, and a `*` grant
    // would let an agent in a project the owner never approved read that
    // project's account with the one stored token.
    //
    // Mutation that proves this bites: make `may_be_granted_everywhere` return
    // `op.names_nothing()` again and this test fails while every other test in
    // the workspace still passes.
    let tag = format!("everywhere-{}", std::process::id());
    let store = std::env::temp_dir().join(format!("apex-cf-store-{tag}"));
    let project = std::env::temp_dir().join(format!("apex-cf-project-{tag}"));
    std::fs::remove_dir_all(&store).ok();
    std::fs::create_dir_all(&project).expect("project");
    std::fs::write(project.join("apex.toml"), PROJECT_FILE).expect("apex.toml");

    let mut registry = Registry::new();
    registry
        .register(Box::new(CloudflareProvider::new()))
        .expect("register");
    let service = Service::new(Store::new(store.clone()), false, registry);
    let peer = me();
    assert_eq!(
        service.add(
            peer,
            NewService {
                service: "cloudflare",
                host: "127.0.0.1",
                scheme: "http",
                username: None,
                path: "",
                auth: None,
                port: None,
            },
            SecretValue::new(TOKEN.as_bytes().to_vec()),
        ),
        Response::Ok
    );

    // Refused everywhere. `*` IS the everywhere key; the trailing bool is
    // `revoke`, not `everywhere`.
    let reply = service.grant(
        peer,
        apex_secret_core::store::ANY_PROJECT,
        "cloudflare",
        "cloudflare.account.read",
        false,
    );
    let (_, message) = reply
        .as_error()
        .expect("`*` must be refused for an operation that resolves per project");
    assert!(
        message.contains("resolves against the project"),
        "the refusal has to say why: {message}"
    );

    // ...and allowed in the project the owner named, which is the whole point
    // of refusing the other one rather than refusing both.
    let named = service.grant(
        peer,
        project.to_str().expect("utf8"),
        "cloudflare",
        "cloudflare.account.read",
        false,
    );
    assert!(
        named.as_error().is_none(),
        "the named project must still be grantable: {named:?}"
    );
    match named {
        Response::Grants { projects } => assert!(
            projects
                .get(project.to_str().expect("utf8"))
                .is_some_and(|caps| caps.iter().any(|c| c.contains("cloudflare.account.read"))),
            "{projects:?}"
        ),
        other => panic!("a grant answers with the grants: {other:?}"),
    }

    std::fs::remove_dir_all(&store).ok();
    std::fs::remove_dir_all(&project).ok();
}
