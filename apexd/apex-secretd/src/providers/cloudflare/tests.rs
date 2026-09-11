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

/// The two record ids the double hands out. 32 lowercase hex, which is the
/// shape the provider checks before one becomes part of a URL.
const RECORD_A: &str = "aa11bb22cc33dd44ee55ff6677889900";
const RECORD_B: &str = "00998877ff66ee55dd44cc33bb22aa11";

/// The one bucket §13.1's file binds.
const BUCKET: &str = "example-assets";

/// What the double stores for the one object a test reads back. Not JSON, and
/// it carries the credential the request arrived with, so a read that came back
/// unscrubbed would show it.
const OBJECT: &str = "-- apex backup\n-- fetched with {{authorization}}\nCREATE TABLE t (id INTEGER);\n";

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
    /// What the request said its body was. R2 puts an object's media type here
    /// and the provider chooses it from a table, so it is worth recording.
    content_type: Option<String>,
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
    let mut content_type = None;
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
        if let Some(value) = line.trim().strip_prefix("Content-Type: ") {
            content_type = Some(value.to_string());
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
        content_type: content_type.clone(),
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
    // ...and a reply that is not the envelope carries it too. R2 and KV answer
    // a read with the stored bytes and `application/octet-stream`, so there is
    // no `messages` array to put it in, and a scrub that only ever ran on JSON
    // would go unnoticed on exactly the two operations whose replies are not
    // JSON.
    let body = body.replace("{{authorization}}", &authorization);
    reply(&mut stream, status, &body);
}

/// The documented paths, with the documented shapes.
///
/// The query string is split off before matching: DNS is the only surface here
/// that carries one, and it carries it on the *lookup* that decides which
/// record a name means, so a double that matched on the whole request-target
/// would 404 exactly the request whose answer the mutation depends on.
fn answer(method: &str, target: &str) -> (u16, String) {
    let target = target.strip_prefix("/client/v4").unwrap_or(target);
    let (path, query) = match target.split_once('?') {
        Some((path, query)) => (path, query),
        None => (target, ""),
    };
    let param = |name: &str| -> String {
        query
            .split('&')
            .find_map(|pair| pair.strip_prefix(&format!("{name}=")))
            .unwrap_or("")
            .to_string()
    };
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

        // ── §13.5, R2 ───────────────────────────────────────────────────────
        ("POST", p) if p == format!("/accounts/{ACCOUNT}/r2/buckets") => ok(
            r#"{"name":"example-assets","location":"apac","storage_class":"Standard","creation_date":"2026-09-12T00:00:00.000Z"}"#,
        ),
        ("GET", p) if p == objects_path("") => ok(
            r#"[{"key":"db/today.sql","size":19,"etag":"aa","last_modified":"2026-09-12T00:00:00.000Z"}]"#,
        ),
        // A read answers with the object's own bytes and no envelope at all —
        // `application/octet-stream`, which is what the documented endpoint
        // returns on success even though its failures are JSON.
        ("GET", p) if p.starts_with(&objects_path("/")) => (200, OBJECT.to_string()),
        ("PUT", p) if p.starts_with(&objects_path("/")) => {
            ok(r#"{"key":"db/today.sql","size":19,"etag":"bb","version":"v2"}"#)
        }

        // ── §13.9, DNS ──────────────────────────────────────────────────────
        //
        // The zone answers about the name it was asked about, and each name
        // below is one of the five things a lookup can run into. A double that
        // only ever answered "here is your record" would let a build that
        // could not tell a 403 from an empty zone pass every test in this file.
        ("GET", p) if p == records_path() => {
            let name = param("name.exact");
            let kind = param("type");
            let one = |id: &str, name: &str, content: &str| {
                format!(r#"{{"id":"{id}","name":"{name}","type":"{kind}","content":"{content}","ttl":1,"proxied":false}}"#)
            };
            match name.as_str() {
                // Denied. A body that looks like every other failure, with the
                // one documented denial signal in it.
                "denied.example.com" => (
                    403,
                    r#"{"success":false,"errors":[{"code":10000,"message":"Forbidden","documentation_url":"https://developers.cloudflare.com/api/resources/dns/subresources/records/methods/list"}],"messages":[],"result":null}"#.to_string(),
                ),
                // Absent: the zone answered, successfully, with nothing.
                "gone.example.com" => ok("[]"),
                // Ambiguous: two A records at one name is ordinary
                // round-robin, and picking either would be a guess.
                "many.example.com" => ok(&format!(
                    "[{},{}]",
                    one(RECORD_A, "many.example.com", "203.0.113.1"),
                    one(RECORD_B, "many.example.com", "203.0.113.2")
                )),
                // A zone that ignored the filter and answered with the whole
                // thing. The build has to notice that none of it is the record
                // it asked for.
                "unfiltered.example.com" => ok(&format!(
                    "[{},{}]",
                    one(RECORD_A, "www.example.com", "203.0.113.1"),
                    one(RECORD_B, "other.example.com", "203.0.113.2")
                )),
                other => ok(&format!("[{}]", one(RECORD_A, other, "203.0.113.1"))),
            }
        }
        ("POST", p) if p == records_path() => ok(&format!(
            r#"{{"id":"{RECORD_A}","name":"new.example.com","type":"A","content":"203.0.113.7"}}"#
        )),
        ("PATCH", p) if p == format!("{}/{RECORD_A}", records_path()) => ok(&format!(
            r#"{{"id":"{RECORD_A}","name":"www.example.com","type":"A","content":"203.0.113.8"}}"#
        )),
        ("DELETE", p) if p == format!("{}/{RECORD_A}", records_path()) => {
            ok(&format!(r#"{{"id":"{RECORD_A}"}}"#))
        }

        _ => (
            404,
            format!(
                r#"{{"success":false,"errors":[{{"code":7003,"message":"no route for {method} {path}"}}],"result":null}}"#
            ),
        ),
    }
}

/// Where the double keeps this zone's records.
fn records_path() -> String {
    format!("/zones/{ZONE}/dns_records")
}

fn reply(stream: &mut TcpStream, status: u16, body: &str) {
    let reason = if status == 200 { "OK" } else { "Error" };
    // An object's bytes are not the envelope, and saying they are would be the
    // one lie in this double that a provider could come to depend on.
    let kind = if body.starts_with('{') || body.starts_with('[') {
        "application/json"
    } else {
        "application/octet-stream"
    };
    let _ = write!(
        stream,
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {kind}\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}

/// The prefix every R2 object path in the double shares, with `suffix` after
/// `objects` — `""` for the bucket's listing, `"/"` for one object.
fn objects_path(suffix: &str) -> String {
    format!("/accounts/{ACCOUNT}/r2/buckets/{BUCKET}/objects{suffix}")
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

/// One row of [`every_operation`]: the operation id, the resource to ask it
/// for, the options its own declaration accepts, and **how many authenticated
/// requests performing it should take**.
///
/// The count is a field rather than an assumption because it stopped being one
/// everywhere. `dns.update` and `dns.delete` ask the zone which record a name
/// means before changing it, so they are two requests each — and a test that
/// asserted "one call per operation" would either have to be loosened into
/// meaninglessness or be wrong. Written down, it stays a measurement: an
/// operation that quietly started making an extra call would fail here.
type OperationCase = (
    &'static str,
    &'static str,
    Vec<(&'static str, &'static str)>,
    usize,
);

/// Every operation the provider declares, with a resource and options that its
/// own declaration accepts.
fn every_operation() -> Vec<OperationCase> {
    vec![
        ("cloudflare.account.read", "", vec![], 1),
        ("cloudflare.worker.read", "project", vec![], 1),
        (
            "cloudflare.worker.upload-version",
            "project",
            vec![
                ("script", "dist/worker.js"),
                ("compatibility-date", "2026-09-01"),
                ("message", "released by apex"),
            ],
            1,
        ),
        (
            "cloudflare.worker.deploy",
            "project",
            vec![("version", "1c4dd6be-0000-4000-8000-abcdefabcdef")],
            1,
        ),
        (
            "cloudflare.worker.rollback",
            "project",
            vec![("version", "0a0b0c0d-0000-4000-8000-000000000000")],
            1,
        ),
        ("cloudflare.worker.tail", "project", vec![], 1),
        ("cloudflare.worker.route.read", "project", vec![], 1),
        ("cloudflare.r2.object.read", "example-assets/db/today.sql", vec![], 1),
        (
            "cloudflare.r2.object.write",
            "example-assets/db/today.sql",
            vec![("file", "dist/today.sql")],
            1,
        ),
        (
            "cloudflare.r2.bucket.create",
            "example-assets",
            vec![("location", "apac"), ("storage-class", "Standard")],
            1,
        ),
        ("cloudflare.dns.read", "www.example.com", vec![("type", "A")], 1),
        (
            "cloudflare.dns.create",
            "new.example.com",
            vec![
                ("type", "A"),
                ("content", "203.0.113.7"),
                ("ttl", "300"),
                ("proxied", "false"),
                ("comment", "added by apex"),
            ],
            1,
        ),
        (
            "cloudflare.dns.update",
            "www.example.com",
            vec![("type", "A"), ("content", "203.0.113.8")],
            2,
        ),
        ("cloudflare.dns.delete", "old.example.com", vec![("type", "A")], 2),
    ]
}

/// How many authenticated requests the whole surface should take.
fn expected_calls() -> usize {
    every_operation().iter().map(|(_, _, _, calls)| calls).sum()
}

/// The files the project holds for the operations that upload one: a Worker
/// module, and something to put in a bucket.
fn with_files(fixture: &Fixture) {
    std::fs::create_dir_all(fixture.project.join("dist")).expect("dist");
    std::fs::write(
        fixture.project.join("dist/worker.js"),
        "export default { fetch: () => new Response('hi') };\n",
    )
    .expect("worker.js");
    std::fs::write(fixture.project.join("dist/today.sql"), UPLOADED).expect("today.sql");
}

/// What a test puts into a bucket. Distinctive, so finding it in the body the
/// double received means the project's own file arrived and not something that
/// merely has the right length.
const UPLOADED: &str = "INSERT INTO t VALUES (1);\n";

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
    with_files(&f);

    for (operation, resource, options, _) in every_operation() {
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
        expected_calls(),
        "every request the surface makes, and no others: {authorizations:?}"
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
    // One trail line per operation even where an operation took two requests:
    // the trail records what was authorised, not what the wire carried.
    assert!(expected_calls() > SPEC.operations.len());
}

#[test]
fn the_agent_side_holds_no_credential_even_when_it_holds_every_operation() {
    // The same criterion from the other end: with every capability granted,
    // there is no request in the protocol whose reply carries a value, and the
    // store the value is in is not the caller's to read. `Response` cannot
    // even be constructed with one — `SecretValue` implements no `Serialize` —
    // so this checks the reachable surface rather than restating the type.
    let f = Fixture::new("noread", Mode::Normal, &granted_everything());
    with_files(&f);
    let peer = me();

    let mut replies = vec![
        f.service.hello(),
        f.service.list(peer),
        f.service.grants(peer),
        f.service.audit(peer, 100),
    ];
    for (operation, resource, options, _) in every_operation() {
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
    assert_eq!(f.fake.authorizations().len(), expected_calls());
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
    with_files(&f);
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
    with_files(&f);
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
    with_files(&f);
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
    with_files(&f);
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
    // Thirteen of §13.2's thirty-two, and one addition. The arithmetic is asserted
    // because the module note states it and a later task will read that note
    // to work out what is left.
    let from_13_2 = SPEC
        .operations
        .iter()
        .filter(|op| listed.contains(&op.id))
        .count();
    assert_eq!(from_13_2, 13);
    assert_eq!(SPEC.operations.len(), 14);
    assert_eq!(SECTION_13_2.len() - from_13_2, 19, "still unimplemented");

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

// ── §13.5, R2 ───────────────────────────────────────────────────────────────
//
// Six mutations were run against the arms below, one at a time, each restored
// by copying the pristine file back so that cargo rebuilt rather than reusing
// the mutant's binary. Every one turns a named test red:
//
// * the bucket dropped out of the object path — `an_r2_object_is_addressed_…`;
// * `binding.bucket()` replaced by the caller's own string, which is the
//   defect §13.5 exists to prevent — `a_bucket_this_project_does_not_bind_…`;
// * the upload sending the path instead of the file's bytes, the media type
//   table answering `text/plain` for `.sql`, and the `Content-Type` header
//   dropped from a raw body — all three, `an_upload_to_r2_carries_…`;
// * the location hint forwarded instead of checked against Cloudflare's own
//   set — `a_bucket_is_created_with_the_name_…`.

#[test]
fn an_r2_object_is_addressed_by_the_bucket_this_project_bound_and_the_key_under_it() {
    // The documented paths, exactly. `example-assets` on its own lists the
    // bucket; `example-assets/db/today.sql` is one object in it; and the
    // account id in front of both came out of apex.toml rather than out of the
    // request.
    let f = Fixture::new("r2paths", Mode::Normal, &granted_everything());
    with_files(&f);
    f.use_it(f.record("cloudflare.r2.object.read", "example-assets"));
    f.use_it(f.record("cloudflare.r2.object.read", "example-assets/db/today.sql"));
    f.use_it(
        f.record("cloudflare.r2.object.write", "example-assets/db/today.sql")
            .param("file", "dist/today.sql"),
    );
    f.use_it(f.record("cloudflare.r2.bucket.create", "example-assets"));

    let seen: Vec<(String, String)> = f
        .fake
        .seen()
        .into_iter()
        .map(|s| (s.method, s.path))
        .collect();
    assert_eq!(
        seen,
        vec![
            ("GET".into(), format!("/client/v4/accounts/{ACCOUNT}/r2/buckets/{BUCKET}/objects")),
            (
                "GET".into(),
                format!("/client/v4/accounts/{ACCOUNT}/r2/buckets/{BUCKET}/objects/db/today.sql")
            ),
            (
                "PUT".into(),
                format!("/client/v4/accounts/{ACCOUNT}/r2/buckets/{BUCKET}/objects/db/today.sql")
            ),
            ("POST".into(), format!("/client/v4/accounts/{ACCOUNT}/r2/buckets")),
        ]
    );
}

#[test]
fn a_bucket_this_project_does_not_bind_never_reaches_cloudflare() {
    // §13.5 asks for bucket-scoped capabilities, and this is what scoped means
    // here: the grant is per operation, and *which* bucket it may touch is the
    // project file's answer, not the caller's. All three R2 operations are
    // checked, because a guard on the two that read a key and not on the one
    // that creates a bucket would let an agent make buckets in the account at
    // will.
    let f = Fixture::new("r2unbound", Mode::Normal, &granted_everything());
    with_files(&f);
    for (operation, resource, options) in [
        ("cloudflare.r2.object.read", "somebody-elses-bucket/db/today.sql", vec![]),
        (
            "cloudflare.r2.object.write",
            "somebody-elses-bucket/db/today.sql",
            vec![("file", "dist/today.sql")],
        ),
        ("cloudflare.r2.bucket.create", "somebody-elses-bucket", vec![]),
    ] {
        let mut rec = f.record(operation, resource);
        for (name, value) in &options {
            rec = rec.param(name, value);
        }
        let reply = f.use_it(rec);
        let (kind, message) = reply
            .as_error()
            .unwrap_or_else(|| panic!("{operation} accepted an unbound bucket"));
        assert_eq!(kind, ErrorKind::BadRequest, "{operation}");
        assert!(message.contains("somebody-elses-bucket"), "{message}");
        assert!(message.contains("example-assets"), "{message}");
        assert!(message.contains("apex.toml"), "{message}");
    }
    assert!(
        f.fake.seen().is_empty(),
        "an unbound bucket still reached the api"
    );
}

#[test]
fn an_upload_to_r2_carries_the_project_s_own_file_and_a_media_type_from_a_table() {
    // The body IS the object, so there is nothing here to escape and nothing
    // to wrap — which makes the two things worth checking the two that could
    // go wrong: that the bytes are the project's file rather than a path, and
    // that the media type came from this build rather than from the caller.
    let f = Fixture::new("r2upload", Mode::Normal, &["cloudflare.r2.object.write"]);
    with_files(&f);
    let reply = f.use_it(
        f.record("cloudflare.r2.object.write", "example-assets/db/today.sql")
            .param("file", "dist/today.sql"),
    );
    assert!(matches!(reply, Response::Performed { exit_code: 0, .. }), "{reply:?}");

    let sent = f.fake.seen();
    let put = sent.first().expect("one request");
    assert_eq!(put.method, "PUT");
    assert_eq!(put.body, UPLOADED, "the object's bytes are the project's file");
    assert_eq!(put.content_type.as_deref(), Some("application/sql"));
    assert_eq!(sent.len(), 1);
}

#[test]
fn an_object_named_for_something_this_build_does_not_know_is_bytes_rather_than_a_guess() {
    let f = Fixture::new("r2type", Mode::Normal, &["cloudflare.r2.object.write"]);
    with_files(&f);
    std::fs::write(f.project.join("dist/blob.whatever"), "x").expect("blob");
    f.use_it(
        f.record("cloudflare.r2.object.write", "example-assets/db/blob.whatever")
            .param("file", "dist/blob.whatever"),
    );
    assert_eq!(
        f.fake.seen()[0].content_type.as_deref(),
        Some("application/octet-stream")
    );
}

#[test]
fn an_upload_to_r2_will_not_follow_a_link_out_of_the_project() {
    // The same root-reading-a-caller's-path problem as a Worker module, on the
    // operation that was written second. A guard that held for one upload and
    // not the other would be no guard at all.
    let f = Fixture::new("r2link", Mode::Normal, &["cloudflare.r2.object.write"]);
    std::fs::create_dir_all(f.project.join("dist")).expect("dist");
    std::os::unix::fs::symlink("/etc/hostname", f.project.join("dist/today.sql")).expect("link");
    let reply = f.use_it(
        f.record("cloudflare.r2.object.write", "example-assets/db/today.sql")
            .param("file", "dist/today.sql"),
    );
    let (_, message) = reply.as_error().expect("a symlinked payload must be refused");
    assert!(message.contains("symbolic link"), "{message}");
    assert!(f.fake.seen().is_empty(), "it was sent anyway");
}

#[test]
fn a_write_that_names_only_a_bucket_is_refused_rather_than_writing_the_bucket() {
    // `example-assets` is a legal resource for this operation's declaration —
    // a `Path` of one segment — and it names no object. Sending it would `PUT`
    // the bucket's own listing URL.
    let f = Fixture::new("r2nokey", Mode::Normal, &["cloudflare.r2.object.write"]);
    with_files(&f);
    let reply = f.use_it(
        f.record("cloudflare.r2.object.write", "example-assets")
            .param("file", "dist/today.sql"),
    );
    let (_, message) = reply.as_error().expect("refused");
    assert!(message.contains("names a bucket and not an object"), "{message}");
    assert!(f.fake.seen().is_empty());
}

#[test]
fn a_bucket_is_created_with_the_name_the_project_declares_and_options_from_a_closed_set() {
    let f = Fixture::new("r2create", Mode::Normal, &["cloudflare.r2.bucket.create"]);
    let reply = f.use_it(
        f.record("cloudflare.r2.bucket.create", "example-assets")
            .param("location", "apac")
            .param("storage-class", "InfrequentAccess"),
    );
    assert!(matches!(reply, Response::Performed { exit_code: 0, .. }), "{reply:?}");
    let sent = f.fake.seen();
    let create = sent.first().expect("one request");
    assert_eq!(create.method, "POST");
    assert!(create.body.contains(r#""name":"example-assets""#), "{}", create.body);
    assert!(create.body.contains(r#""locationHint":"apac""#), "{}", create.body);
    assert!(create.body.contains(r#""storageClass":"InfrequentAccess""#), "{}", create.body);

    // ...and a value outside the documented set is refused here rather than
    // spent on a request that will fail at the other end.
    for (param, value) in [("location", "mars"), ("storage-class", "Cheap")] {
        let reply = f.use_it(
            f.record("cloudflare.r2.bucket.create", "example-assets").param(param, value),
        );
        let (_, message) = reply.as_error().unwrap_or_else(|| panic!("{param}={value} was sent"));
        assert!(message.contains(value), "{message}");
    }
    assert_eq!(f.fake.seen().len(), 1, "a refused option still reached the api");
}

#[test]
fn an_object_read_answers_with_bytes_and_still_has_the_credential_taken_out() {
    // R2 answers a read with the object itself and no envelope. Every other
    // operation's reply is JSON, so a scrub that happened to key on the
    // envelope would pass everywhere except here — and here is where a
    // project's own data comes back through the broker.
    let f = Fixture::new("r2read", Mode::Normal, &["cloudflare.r2.object.read"]);
    let reply = f.use_it(f.record("cloudflare.r2.object.read", "example-assets/db/today.sql"));
    let Response::Performed { output, .. } = &reply else {
        panic!("refused: {reply:?}");
    };
    // The object arrived...
    assert!(output.contains("CREATE TABLE t (id INTEGER);"), "{output}");
    // ...the double really did echo the credential into it...
    assert!(output.contains("fetched with Bearer"), "{output}");
    // ...and it is not in what came back.
    assert!(!output.contains(TOKEN), "the object read handed back the token");
    assert!(output.contains("«redacted»"), "{output}");
    assert!(!f.trail().contains(TOKEN));
}

// ── §13.9, DNS ──────────────────────────────────────────────────────────────
//
// Eight mutations were run against the arms below, one at a time, each restored
// by copying the pristine file back so that cargo rebuilt rather than reusing
// the mutant's binary. Every one turns a named test red:
//
// * `inside()` suffix-matching without the label boundary, so `notexample.com`
//   is inside `example.com` — `a_name_outside_the_bound_zone_…`;
// * the §13.9 type guard skipped on writes — `changing_a_delegation_…`;
// * `Lookup::Denied` folded into `Lookup::Absent`, which is the defect this
//   whole module is shaped around — `a_lookup_that_was_refused_…`;
// * an ambiguous name answered with the first record instead of a refusal —
//   `a_name_that_more_than_one_record_answers_to_…`;
// * the name/type re-check dropped, so the far side's filter is trusted —
//   `a_zone_that_answered_about_other_records_…`;
// * `PUT` in place of `PATCH` on an update — `an_update_sends_only_the_fields_…`;
// * the project's own `records` narrowing ignored — `a_project_that_narrows_…`;
// * `is_record_id` accepting anything non-empty — `a_record_id_off_the_wire_…`.

#[test]
fn a_dns_record_is_addressed_by_name_in_the_zone_this_project_bound() {
    // The documented paths, and the shape of the two-request verbs. An update
    // asks the zone which record `www.example.com` means and only then changes
    // it, which is why there are six requests here for four operations.
    let f = Fixture::new("dnspaths", Mode::Normal, &granted_everything());
    f.use_it(f.record("cloudflare.dns.read", "www.example.com").param("type", "A"));
    f.use_it(
        f.record("cloudflare.dns.create", "new.example.com")
            .param("type", "A")
            .param("content", "203.0.113.7"),
    );
    f.use_it(
        f.record("cloudflare.dns.update", "www.example.com")
            .param("type", "A")
            .param("content", "203.0.113.8"),
    );
    f.use_it(f.record("cloudflare.dns.delete", "old.example.com").param("type", "A"));

    let seen: Vec<(String, String)> = f.fake.seen().into_iter().map(|s| (s.method, s.path)).collect();
    let records = format!("/client/v4/zones/{ZONE}/dns_records");
    assert_eq!(
        seen,
        vec![
            ("GET".into(), format!("{records}?name.exact=www.example.com&type=A")),
            ("POST".into(), records.clone()),
            ("GET".into(), format!("{records}?name.exact=www.example.com&type=A")),
            ("PATCH".into(), format!("{records}/{RECORD_A}")),
            ("GET".into(), format!("{records}?name.exact=old.example.com&type=A")),
            ("DELETE".into(), format!("{records}/{RECORD_A}")),
        ]
    );
}

#[test]
fn a_name_outside_the_bound_zone_never_reaches_cloudflare() {
    // §13.9's first sentence. The label boundary is the case worth writing
    // down: `notexample.com` ends in `example.com` and is a different
    // registration, and a suffix test without the dot would hand it over.
    let f = Fixture::new("dnszone", Mode::Normal, &granted_everything());
    for outside in [
        "notexample.com",
        "example.com.attacker.test",
        "attacker.test",
        "com",
    ] {
        let reply = f.use_it(
            f.record("cloudflare.dns.update", outside)
                .param("type", "A")
                .param("content", "203.0.113.9"),
        );
        let (kind, message) = reply
            .as_error()
            .unwrap_or_else(|| panic!("'{outside}' was accepted as a name in this zone"));
        assert_eq!(kind, ErrorKind::BadRequest, "{outside}");
        assert!(message.contains("example.com"), "{message}");
    }
    // ...and the apex itself IS in its own zone, which is the other half: a
    // build that refused everything would pass the loop above.
    let apex = f.use_it(f.record("cloudflare.dns.read", "example.com").param("type", "A"));
    assert!(matches!(apex, Response::Performed { exit_code: 0, .. }), "{apex:?}");
    assert_eq!(f.fake.seen().len(), 1, "only the apex read should have been sent");
}

#[test]
fn a_project_that_narrows_itself_to_some_records_cannot_touch_the_others() {
    // The second half of "bound zones/records": a project may cut itself down
    // to a list, and then the zone is not enough.
    let f = Fixture::new("dnsnarrow", Mode::Normal, &granted_everything());
    std::fs::write(
        f.project.join("apex.toml"),
        format!(
            "[identity.cloudflare]\naccount_id = \"{ACCOUNT}\"\n\
             [cloudflare]\nzone = \"example.com\"\nzone_id = \"{ZONE}\"\n\
             records = [\"www\", \"api.example.com\"]\n"
        ),
    )
    .expect("write");

    // Both spellings of a bound record resolve — `www` and the full name.
    for allowed in ["www.example.com", "api.example.com"] {
        let reply = f.use_it(f.record("cloudflare.dns.read", allowed).param("type", "A"));
        assert!(
            matches!(reply, Response::Performed { exit_code: 0, .. }),
            "{allowed} is bound and was refused: {reply:?}"
        );
    }
    // ...and a name in the same zone that is not on the list is not.
    let reply = f.use_it(
        f.record("cloudflare.dns.delete", "mail.example.com").param("type", "A"),
    );
    let (kind, message) = reply.as_error().expect("a narrowed project must not reach it");
    assert_eq!(kind, ErrorKind::BadRequest);
    assert!(message.contains("mail.example.com"), "{message}");
    assert!(message.contains("www and api.example.com"), "{message}");
    assert_eq!(f.fake.seen().len(), 2, "the refused name still reached the api");
}

#[test]
fn changing_a_delegation_or_a_dnssec_record_is_refused_as_an_elevated_shape() {
    // §13.9's second sentence, as far as a build with one capability class can
    // take it. The owner has granted all four DNS verbs; every reserved type
    // is still refused, and nothing reaches the api — which is the property
    // that matters whether or not an elevated class ever exists.
    let f = Fixture::new("dnselevated", Mode::Normal, &granted_everything());
    for kind in ["NS", "DS", "DNSKEY", "SOA", "ns", "ds"] {
        for (operation, options) in [
            ("cloudflare.dns.create", vec![("content", "ns1.attacker.test")]),
            ("cloudflare.dns.update", vec![("content", "ns1.attacker.test")]),
            ("cloudflare.dns.delete", vec![]),
        ] {
            let mut rec = f.record(operation, "example.com").param("type", kind);
            for (name, value) in &options {
                rec = rec.param(name, value);
            }
            let reply = f.use_it(rec);
            let (error, message) = reply
                .as_error()
                .unwrap_or_else(|| panic!("{operation} accepted a {kind} record"));
            assert_eq!(error, ErrorKind::PermissionDenied, "{operation} {kind}");
            assert!(message.contains("elevated"), "{message}");
            assert!(message.contains("§13.9"), "{message}");
        }
    }
    assert!(
        f.fake.seen().is_empty(),
        "a reserved record type reached the api: {:?}",
        f.fake.seen()
    );

    // Reading one is allowed: §13.9 reserves *changes*, and an agent that
    // cannot see where a zone is delegated cannot check its own work.
    let read = f.use_it(f.record("cloudflare.dns.read", "example.com").param("type", "NS"));
    assert!(matches!(read, Response::Performed { exit_code: 0, .. }), "{read:?}");
    assert_eq!(f.fake.seen().len(), 1);
}

#[test]
fn a_lookup_that_was_refused_is_not_reported_as_a_record_that_is_not_there() {
    // The defect shape this codebase has found about fifteen times, on the one
    // path here that could reintroduce it. The zone answers 403 — a body that
    // looks like every other failure — and the answer must not be "no such
    // record", because a caller told that would go and create a second one.
    let f = Fixture::new("dnsdenied", Mode::Normal, &granted_everything());
    let reply = f.use_it(
        f.record("cloudflare.dns.update", "denied.example.com")
            .param("type", "A")
            .param("content", "203.0.113.9"),
    );
    let (kind, message) = reply.as_error().expect("a refused lookup is not a success");
    assert_eq!(
        kind,
        ErrorKind::PermissionDenied,
        "a denial must not be reported as a bad request about a missing record"
    );
    assert!(message.contains("403"), "{message}");
    assert!(message.contains("not the same as the record not being there"), "{message}");
    assert!(!message.contains("holds no"), "{message}");
    // One request — the lookup. Nothing was changed.
    let seen = f.fake.seen();
    assert_eq!(seen.len(), 1, "{seen:?}");
    assert_eq!(seen[0].method, "GET");
}

#[test]
fn a_record_that_is_absent_says_the_zone_answered_and_holds_none() {
    // The other side of the same distinction, and the reason it is worth
    // having: this one really is an absence, and it says so in those words.
    let f = Fixture::new("dnsabsent", Mode::Normal, &granted_everything());
    let reply = f.use_it(f.record("cloudflare.dns.delete", "gone.example.com").param("type", "A"));
    let (kind, message) = reply.as_error().expect("deleting nothing is not a success");
    assert_eq!(kind, ErrorKind::BadRequest);
    assert!(message.contains("answered"), "{message}");
    assert!(message.contains("holds no A record"), "{message}");
    assert_eq!(f.fake.seen().len(), 1, "it deleted something anyway");
}

#[test]
fn a_name_that_more_than_one_record_answers_to_is_refused_rather_than_guessed() {
    // Two A records at one name is round-robin, not a fault. Changing the
    // first would change an arbitrary one of them.
    let f = Fixture::new("dnsmany", Mode::Normal, &granted_everything());
    let reply = f.use_it(
        f.record("cloudflare.dns.update", "many.example.com")
            .param("type", "A")
            .param("content", "203.0.113.9"),
    );
    let (kind, message) = reply.as_error().expect("an ambiguous name must be refused");
    assert_eq!(kind, ErrorKind::PermissionDenied);
    assert!(message.contains("2 A records"), "{message}");
    assert_eq!(f.fake.seen().len(), 1, "one of them was changed");
}

#[test]
fn a_zone_that_answered_about_other_records_is_not_taken_at_its_word() {
    // A filter that was sent is not a filter that was applied. If the query
    // parameter were ever spelled wrongly, or stopped being honoured, the
    // lookup would come back holding the whole zone — and the first record in
    // it would be modified. Checking the names here turns that into a refusal.
    let f = Fixture::new("dnsunfiltered", Mode::Normal, &granted_everything());
    let reply = f.use_it(
        f.record("cloudflare.dns.update", "unfiltered.example.com")
            .param("type", "A")
            .param("content", "203.0.113.9"),
    );
    let (kind, message) = reply.as_error().expect("records for other names are not this one");
    assert_eq!(kind, ErrorKind::BadRequest);
    assert!(message.contains("holds no A record"), "{message}");
    assert_eq!(f.fake.seen().len(), 1, "somebody else's record was changed");
}

#[test]
fn an_update_that_changes_nothing_is_refused_before_anything_is_looked_up() {
    let f = Fixture::new("dnsnoop", Mode::Normal, &granted_everything());
    let reply = f.use_it(f.record("cloudflare.dns.update", "www.example.com").param("type", "A"));
    let (_, message) = reply.as_error().expect("an update with no fields is not an update");
    assert!(message.contains("changes nothing"), "{message}");
    assert!(f.fake.seen().is_empty(), "it spent a request finding out");
}

#[test]
fn an_update_sends_only_the_fields_it_was_given() {
    // PATCH and not PUT. Cloudflare's PUT overwrites a record with what the
    // request carries, so an update that set only the content through PUT
    // would quietly reset the TTL and the proxy flag.
    let f = Fixture::new("dnspatch", Mode::Normal, &["cloudflare.dns.update"]);
    f.use_it(
        f.record("cloudflare.dns.update", "www.example.com")
            .param("type", "A")
            .param("content", "203.0.113.8"),
    );
    let sent = f.fake.seen();
    let patch = sent.last().expect("two requests");
    assert_eq!(patch.method, "PATCH");
    assert!(patch.body.contains(r#""content":"203.0.113.8""#), "{}", patch.body);
    assert!(!patch.body.contains("ttl"), "{}", patch.body);
    assert!(!patch.body.contains("proxied"), "{}", patch.body);
    assert!(!patch.body.contains("name"), "{}", patch.body);
}

#[test]
fn a_create_carries_the_whole_record_and_values_cloudflare_would_accept() {
    let f = Fixture::new("dnscreate", Mode::Normal, &["cloudflare.dns.create"]);
    f.use_it(
        f.record("cloudflare.dns.create", "new.example.com")
            .param("type", "a")
            .param("content", "203.0.113.7")
            .param("ttl", "300")
            .param("proxied", "true")
            .param("comment", "added by apex"),
    );
    let sent = f.fake.seen();
    let create = sent.first().expect("one request");
    // The type is upper-cased, because that is what the zone stores.
    assert!(create.body.contains(r#""type":"A""#), "{}", create.body);
    assert!(create.body.contains(r#""name":"new.example.com""#), "{}", create.body);
    assert!(create.body.contains(r#""content":"203.0.113.7""#), "{}", create.body);
    assert!(create.body.contains(r#""ttl":300"#), "{}", create.body);
    assert!(create.body.contains(r#""proxied":true"#), "{}", create.body);
    assert!(create.body.contains(r#""comment":"added by apex""#), "{}", create.body);

    // Values the far side would refuse are refused here, where refusing costs
    // nothing and the message can name the option.
    for (param, value) in [
        ("ttl", "30"),
        ("ttl", "999999"),
        ("ttl", "soon"),
        ("proxied", "yes"),
        ("type", "NOTATYPE"),
    ] {
        let reply = f.use_it(
            f.record("cloudflare.dns.create", "new.example.com")
                .param("type", "A")
                .param("content", "203.0.113.7")
                .param(param, value),
        );
        assert!(
            reply.as_error().is_some(),
            "{param}={value} was sent to cloudflare"
        );
    }
    assert_eq!(f.fake.seen().len(), 1, "a refused value still reached the api");
}

#[test]
fn the_four_dns_verbs_are_four_grants() {
    // §13.2's argument where it matters most: an agent that may point a
    // hostname at a new worker should not thereby be able to delete the zone's
    // records.
    let f = Fixture::new("dnsverbs", Mode::Normal, &["cloudflare.dns.update"]);
    let allowed = f.use_it(
        f.record("cloudflare.dns.update", "www.example.com")
            .param("type", "A")
            .param("content", "203.0.113.8"),
    );
    assert!(matches!(allowed, Response::Performed { exit_code: 0, .. }), "{allowed:?}");
    for refused in [
        "cloudflare.dns.read",
        "cloudflare.dns.create",
        "cloudflare.dns.delete",
    ] {
        let reply = f.use_it(
            f.record(refused, "www.example.com")
                .param("type", "A")
                .param("content", "203.0.113.8"),
        );
        assert!(reply.as_error().is_some(), "'{refused}' went through on a grant for update");
    }
    // The lookup and the patch, and nothing else.
    assert_eq!(f.fake.seen().len(), 2);
}

#[test]
fn the_trail_names_the_record_and_the_zone_that_was_changed() {
    let f = Fixture::new("dnstrail", Mode::Normal, &["cloudflare.dns.update"]);
    f.use_it(
        f.record("cloudflare.dns.update", "www.example.com")
            .param("type", "A")
            .param("content", "203.0.113.8"),
    );
    let lines = audit::tail(&Store::new(f.store.clone()).audit_path(), 10);
    let used = lines.iter().find(|l| l.event == AuditEvent::Used).expect("a use");
    assert_eq!(used.operation, "cloudflare.dns.update");
    assert_eq!(used.resource, "www.example.com");
    assert_eq!(
        used.detail,
        format!("change the A record at www.example.com in zone example.com [{ZONE}]")
    );
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
    // Two mutations prove this bites, and they fail it for different reasons:
    // make `may_be_granted_everywhere` compute `op.names_nothing()` again
    // instead of reading the declaration, or flip this operation's own
    // `same_everywhere` to true. Both were run, and both turn this red.
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
