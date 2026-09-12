//! The S3 provider, against a loopback double that verifies the signature.
//!
//! # What this file can and cannot say
//!
//! The double below recomputes the SigV4 signature **with this crate's own
//! signer** and answers 403 when it does not match. That is deliberately not a
//! check of the algorithm — a signer compared with itself agrees with itself.
//! What it checks is the *wiring*, which the known-answer tests in
//! `sigv4/tests.rs` cannot:
//!
//!   * the canonical URI signed is the path actually requested;
//!   * the `Host` header signed is the one curl actually sent, port and all;
//!   * the payload digest signed is the digest of the body that actually
//!     arrived;
//!   * the query string signed is the one on the URL.
//!
//! Each of those is a way for a correct signer to produce a request a real S3
//! rejects, and each one goes red here if it breaks.
//!
//! The algorithm itself is pinned against **botocore** in `sigv4/tests.rs`, and
//! `tests/test-apex-backup-s3.sh` closes the loop end to end against a Python
//! double that recomputes the whole signature with `hmac` and `hashlib` — a
//! third implementation, sharing no code with this one.
//!
//! # Nothing here reaches AWS
//!
//! There is no account, no live endpoint and no real key anywhere in this
//! repository. The double is on `127.0.0.1`, the access key id is AWS's own
//! documentation example, and `apex secret add` accepts `http` only for a
//! loopback host — so this fixture cannot be turned into a way to send a real
//! key in clear.

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

/// AWS's documented example credentials. They authorise nothing.
const ACCESS_KEY_ID: &str = "AKIDEXAMPLE";
const SECRET_KEY: &str = "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY";
const BUCKET: &str = "example-backups";
const OTHER_BUCKET: &str = "somebody-elses-bucket";
const REGION: &str = "ap-southeast-2";

const PROJECT_FILE: &str = r#"
[s3]
buckets = ["example-backups", "example-logs"]
region = "ap-southeast-2"
"#;

/// One request the double saw.
#[derive(Debug, Clone)]
struct Seen {
    method: String,
    /// Path and query, as they arrived on the request line.
    target: String,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
    /// Whether the signature this build sent verified against a signature the
    /// double recomputed from what actually arrived.
    signature_verified: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// An empty bucket that answers every listing and every read.
    Normal,
    /// A bucket whose listing is always truncated and always hands back a new
    /// continuation token — the shape that makes a short list possible.
    ForeverTruncated,
    /// A far side that refuses. The status is real, unlike on the R2 path.
    Forbidden,
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
                std::thread::spawn(move || serve(stream, &recorder, mode, port));
            }
        });
        Fake { port, seen }
    }

    fn seen(&self) -> Vec<Seen> {
        self.seen.lock().expect("lock").clone()
    }
}

fn serve(mut stream: TcpStream, recorder: &Arc<Mutex<Vec<Seen>>>, mode: Mode, port: u16) {
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

    // ── verify the signature against what actually arrived ──────────────────
    //
    // Every input is taken from the REQUEST, never from what the provider
    // meant to send: the path off the request line, the host off the header,
    // the digest off the body. That is what makes this a check of the wiring.
    let (path, query) = match target.split_once('?') {
        Some((path, query)) => (path, query),
        None => (target.as_str(), ""),
    };
    let host = headers
        .get("host")
        .cloned()
        .unwrap_or_else(|| format!("127.0.0.1:{port}"));
    let amz_date = headers.get("x-amz-date").cloned().unwrap_or_default();
    let digest_sent = headers
        .get("x-amz-content-sha256")
        .cloned()
        .unwrap_or_default();
    let digest_real = sigv4::sha256_hex(&body);
    let region = region_of(headers.get("authorization").map(String::as_str).unwrap_or(""));
    let expected = sigv4::Request {
        method: &method,
        host: &host,
        canonical_uri: path,
        canonical_query: query,
        // The digest the request CLAIMS, so that a claim which does not match
        // the body is caught by the separate equality below rather than
        // silently signed over.
        payload_sha256: &digest_sent,
        amz_date: &amz_date,
        region: &region,
    }
    .authorization(ACCESS_KEY_ID, SECRET_KEY);
    let signature_verified = headers.get("authorization") == Some(&expected)
        && digest_sent == digest_real
        && !amz_date.is_empty();

    recorder.lock().expect("lock").push(Seen {
        method: method.clone(),
        target: target.clone(),
        headers,
        body,
        signature_verified,
    });

    let (status, payload) = if !signature_verified {
        (
            "403 Forbidden",
            "<Error><Code>SignatureDoesNotMatch</Code></Error>".to_string(),
        )
    } else {
        match (mode, method.as_str()) {
            (Mode::Forbidden, _) => (
                "403 Forbidden",
                "<Error><Code>AccessDenied</Code></Error>".to_string(),
            ),
            (Mode::ForeverTruncated, _) => ("200 OK", truncated_listing()),
            (_, "PUT") => ("200 OK", String::new()),
            (_, _) if query.contains("list-type=2") => ("200 OK", listing()),
            _ => ("200 OK", "cGF5bG9hZA==".to_string()),
        }
    };
    let reply = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/xml\r\nContent-Length: {}\r\n\r\n{payload}",
        payload.len()
    );
    let _ = stream.write_all(reply.as_bytes());
    let _ = stream.flush();
}

/// The region out of an `Authorization` header's credential scope.
fn region_of(authorization: &str) -> String {
    authorization
        .split("Credential=")
        .nth(1)
        .and_then(|rest| rest.split(", ").next())
        .and_then(|scope| scope.split('/').nth(2))
        .unwrap_or("us-east-1")
        .to_string()
}

fn listing() -> String {
    // Two keys, and one of them carries an `&` so the unescaping is exercised
    // against a real reply rather than only against a unit fixture.
    "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
     <ListBucketResult><Name>example-backups</Name>\
     <IsTruncated>false</IsTruncated>\
     <Contents><Key>apex-backup/20260912T101112Z-abcd1234/head.json</Key></Contents>\
     <Contents><Key>notes/a&amp;b.txt</Key></Contents>\
     </ListBucketResult>"
        .to_string()
}

fn truncated_listing() -> String {
    "<ListBucketResult><IsTruncated>true</IsTruncated>\
     <NextContinuationToken>more</NextContinuationToken>\
     <Contents><Key>apex-backup/one</Key></Contents></ListBucketResult>"
        .to_string()
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
        Fixture::with(name, mode, granted, PROJECT_FILE, Some(ACCESS_KEY_ID))
    }

    fn with(
        name: &str,
        mode: Mode,
        granted: &[&str],
        file: &str,
        username: Option<&str>,
    ) -> Fixture {
        let fake = Fake::start(mode);
        let tag = format!("{name}-{}-{}", std::process::id(), fake.port);
        // /var/tmp, this project's rule for anything a suite creates.
        let store = PathBuf::from("/var/tmp").join(format!("apex-s3-store-{tag}"));
        let project = PathBuf::from("/var/tmp").join(format!("apex-s3-project-{tag}"));
        std::fs::remove_dir_all(&store).ok();
        std::fs::remove_dir_all(&project).ok();
        std::fs::create_dir_all(&project).expect("project");
        std::fs::write(project.join("apex.toml"), file).expect("apex.toml");

        let mut registry = Registry::new();
        registry.register(Box::new(S3Provider)).expect("register");
        let service = Service::new(Store::new(store.clone()), false, registry);

        let peer = me();
        // `http` is allowed for a loopback host: nothing crosses a network.
        assert_eq!(
            service.add(
                peer,
                NewService {
                    service: "aws",
                    host: "127.0.0.1",
                    scheme: "http",
                    username,
                    path: "",
                    auth: None,
                    port: Some(fake.port),
                },
                SecretValue::new(SECRET_KEY.as_bytes().to_vec()),
            ),
            Response::Ok
        );
        for operation in granted {
            // A successful grant answers with the table, not `Ok`; an error
            // is what a refused one looks like.
            let granted = service.grant(
                peer,
                project.to_str().expect("utf8"),
                "aws",
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
        let mut rec = CapabilityRecord::new("aws", operation, resource);
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

// ── the binding, which is the check that is not vacuous ────────────────────

/// P2-001's S3 half of P2-002's first criterion: the bucket is the project's
/// to declare, and nothing else reaches the far side.
#[test]
fn a_bucket_this_project_did_not_declare_never_reaches_s3() {
    let f = Fixture::new("unbound", Mode::Normal, &["s3.object.read", "s3.object.write"]);

    let reply = f.use_it(f.record("s3.object.read", OTHER_BUCKET));
    let Response::Error { message, .. } = &reply else {
        panic!("a bucket the project never declared was accepted: {reply:?}");
    };
    assert!(message.contains(OTHER_BUCKET), "{message}");
    assert!(
        message.contains("[s3] buckets"),
        "the refusal has to say where the binding is: {message}"
    );
    assert!(
        message.contains(BUCKET),
        "and what the project does bind: {message}"
    );

    // Nothing was signed and nothing was sent. The refusal is before the
    // credential is read, not after the request comes back.
    assert!(
        f.fake.seen().is_empty(),
        "a request reached the far side for a bucket the project never bound"
    );
    assert!(!f.trail().contains(SECRET_KEY));
}

/// A file that could not be read is not a file that says nothing.
#[test]
fn an_apex_toml_that_cannot_be_read_refuses_rather_than_binding_nothing() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new("unreadable", Mode::Normal, &["s3.object.read"]);

    // Malformed refuses whoever runs this, root included.
    std::fs::write(f.project.join("apex.toml"), "[s3\nbuckets = ").expect("write");
    let reply = f.use_it(f.record("s3.object.read", BUCKET));
    assert!(
        matches!(reply, Response::Error { .. }),
        "a malformed apex.toml bound nothing instead of refusing: {reply:?}"
    );

    if unsafe { libc::geteuid() } == 0 {
        eprintln!("NOTE: running as root, so the chmod-000 half is not run.");
    } else {
        std::fs::write(f.project.join("apex.toml"), PROJECT_FILE).expect("write");
        std::fs::set_permissions(
            f.project.join("apex.toml"),
            std::fs::Permissions::from_mode(0o000),
        )
        .expect("chmod");
        let reply = f.use_it(f.record("s3.object.read", BUCKET));
        assert!(
            matches!(reply, Response::Error { .. }),
            "an unreadable apex.toml bound nothing instead of refusing: {reply:?}"
        );
        std::fs::set_permissions(
            f.project.join("apex.toml"),
            std::fs::Permissions::from_mode(0o600),
        )
        .expect("chmod back");
    }
    assert!(f.fake.seen().is_empty(), "something was sent anyway");
}

// ── the round trip ─────────────────────────────────────────────────────────

/// The whole of what a backup needs from this provider, and the assertion that
/// the request a real S3 would see is one it would accept.
#[test]
fn a_chunk_reaches_the_bucket_under_the_key_it_chose_and_the_secret_does_not_come_back() {
    let f = Fixture::new("roundtrip", Mode::Normal, &["s3.object.read", "s3.object.write"]);

    let staged = "_apex-backup/20260912T101112Z-abcd1234/data.000000";
    let path = f.project.join(staged);
    std::fs::create_dir_all(path.parent().expect("parent")).expect("staging");
    // Base64, because a brokered reply is `String::from_utf8_lossy` — the same
    // constraint the R2 path documents at length.
    std::fs::write(&path, "q83vASNFZ4mrze//AAECA/79/A==").expect("chunk");

    let mut write = f.record(
        "s3.object.write",
        &format!("{BUCKET}/apex-backup/20260912T101112Z-abcd1234/data.000000"),
    );
    write.params.insert("file".into(), staged.to_string());
    let reply = f.use_it(write);
    let Response::Performed { output, exit_code, .. } = &reply else {
        panic!("the upload was refused: {reply:?}");
    };
    assert_eq!(*exit_code, 0, "{output}");

    let seen = f.fake.seen();
    let put = seen.iter().find(|s| s.method == "PUT").expect("an upload");
    assert_eq!(
        put.target, "/example-backups/apex-backup/20260912T101112Z-abcd1234/data.000000",
        "the object key is not the one the caller chose"
    );
    assert_eq!(put.body, b"q83vASNFZ4mrze//AAECA/79/A==");
    assert!(
        put.signature_verified,
        "the far side could not verify the signature over what actually arrived"
    );

    // The signature is derived from the secret and is not the secret, and the
    // secret is in neither the reply nor the trail nor the wire.
    assert!(!output.contains(SECRET_KEY), "the upload handed back the key");
    assert!(!f.trail().contains(SECRET_KEY), "the trail holds the key");
    for s in &seen {
        for value in s.headers.values() {
            assert!(!value.contains(SECRET_KEY), "a header holds the key: {value}");
        }
    }

    // And reading it back is addressed by the same key.
    let read = f.use_it(f.record(
        "s3.object.read",
        &format!("{BUCKET}/apex-backup/20260912T101112Z-abcd1234/data.000000"),
    ));
    let Response::Performed { output, .. } = &read else {
        panic!("the read was refused: {read:?}");
    };
    assert_eq!(output, "cGF5bG9hZA==");
    let get = seen_after(&f, "GET");
    assert!(get.signature_verified, "the read's signature did not verify");
}

fn seen_after(f: &Fixture, method: &str) -> Seen {
    f.fake
        .seen()
        .into_iter()
        .rev()
        .find(|s| s.method == method)
        .expect("a request")
}

/// The listing, rendered as the envelope the R2 operation answers with.
#[test]
fn a_listing_is_rendered_as_the_same_envelope_r2_answers_with() {
    let f = Fixture::new("listing", Mode::Normal, &["s3.object.read"]);
    let reply = f.use_it(f.record("s3.object.read", BUCKET));
    let Response::Performed { output, exit_code, .. } = &reply else {
        panic!("the listing was refused: {reply:?}");
    };
    assert_eq!(*exit_code, 0, "{output}");

    let parsed: serde_json::Value = serde_json::from_str(output).expect("the envelope is JSON");
    let keys: Vec<&str> = parsed["result"]
        .as_array()
        .expect("a result array")
        .iter()
        .filter_map(|o| o["key"].as_str())
        .collect();
    assert_eq!(
        keys,
        vec![
            "apex-backup/20260912T101112Z-abcd1234/head.json",
            // Unescaped: what S3 sent was `a&amp;b.txt`.
            "notes/a&b.txt",
        ]
    );

    let get = seen_after(&f, "GET");
    assert!(get.target.contains("list-type=2"), "{}", get.target);
    assert!(get.signature_verified, "the listing's signature did not verify");
}

/// A listing that could not be completed is an error, never a short list.
#[test]
fn a_listing_that_stays_truncated_is_an_error_and_never_a_shortened_history() {
    let f = Fixture::new("truncated", Mode::ForeverTruncated, &["s3.object.read"]);
    let reply = f.use_it(f.record("s3.object.read", BUCKET));
    let Response::Error { message, .. } = &reply else {
        panic!("a truncated listing was reported as a complete one: {reply:?}");
    };
    assert!(message.contains("more than"), "{message}");
    assert!(
        message.contains("NOT being returned"),
        "the refusal has to say that keys were read and withheld: {message}"
    );
    // It followed the token rather than giving up on the first page, and
    // stopped at the cap rather than forever.
    assert_eq!(f.fake.seen().len(), MAX_LIST_PAGES);
    assert!(f.fake.seen()[1].target.contains("continuation-token=more"));
}

/// The status is real here, unlike on the R2 path, so a refusal says which one.
#[test]
fn a_refusal_from_the_far_side_carries_its_status() {
    let f = Fixture::new("forbidden", Mode::Forbidden, &["s3.object.read"]);
    let reply = f.use_it(f.record("s3.object.read", BUCKET));
    let Response::Performed { output, exit_code, .. } = &reply else {
        panic!("{reply:?}");
    };
    assert_eq!(*exit_code, 1);
    assert!(output.contains("HTTP 403"), "{output}");
    assert!(output.contains("AccessDenied"), "{output}");
}

/// An S3 credential is two halves, and the message says how to store the other.
#[test]
fn a_credential_with_no_access_key_id_is_refused_and_says_how_to_store_one() {
    let f = Fixture::with("noak", Mode::Normal, &["s3.object.read"], PROJECT_FILE, None);
    let reply = f.use_it(f.record("s3.object.read", BUCKET));
    let Response::Error { message, .. } = &reply else {
        panic!("a credential with no access key id was spent: {reply:?}");
    };
    assert!(message.contains("access key id"), "{message}");
    assert!(message.contains("--username"), "{message}");
    assert!(f.fake.seen().is_empty(), "something was signed anyway");
}

/// Writing needs a key, and naming a bucket alone is a resource that does not
/// resolve — not a permission the caller lacks.
#[test]
fn a_write_addressed_at_a_bucket_rather_than_an_object_is_refused_before_anything_is_signed() {
    let f = Fixture::new("nokey", Mode::Normal, &["s3.object.write"]);
    let mut write = f.record("s3.object.write", BUCKET);
    write.params.insert("file".into(), "apex.toml".to_string());
    let reply = f.use_it(write);
    let Response::Error { message, .. } = &reply else {
        panic!("{reply:?}");
    };
    assert!(message.contains("names a bucket and not an object"), "{message}");
    assert!(f.fake.seen().is_empty());
}

// ── the pieces, unit by unit ───────────────────────────────────────────────

#[test]
fn the_region_defaults_when_the_project_names_none_and_is_refused_when_it_is_not_a_region() {
    let f = Fixture::with(
        "region",
        Mode::Normal,
        &["s3.object.read"],
        "[s3]\nbuckets = [\"example-backups\"]\n",
        Some(ACCESS_KEY_ID),
    );
    assert!(matches!(
        f.use_it(f.record("s3.object.read", BUCKET)),
        Response::Performed { .. }
    ));
    let scope = seen_after(&f, "GET")
        .headers
        .get("authorization")
        .cloned()
        .unwrap_or_default();
    assert!(
        scope.contains(&format!("/{DEFAULT_REGION}/s3/aws4_request")),
        "{scope}"
    );

    let bad = Fixture::with(
        "badregion",
        Mode::Normal,
        &["s3.object.read"],
        "[s3]\nbuckets = [\"example-backups\"]\nregion = \"ap southeast 2\"\n",
        Some(ACCESS_KEY_ID),
    );
    assert!(matches!(
        bad.use_it(bad.record("s3.object.read", BUCKET)),
        Response::Error { .. }
    ));
    assert!(bad.fake.seen().is_empty());
}

#[test]
fn the_project_s_region_is_the_one_in_the_credential_scope() {
    let f = Fixture::new("scope", Mode::Normal, &["s3.object.read"]);
    assert!(matches!(
        f.use_it(f.record("s3.object.read", BUCKET)),
        Response::Performed { .. }
    ));
    let scope = seen_after(&f, "GET")
        .headers
        .get("authorization")
        .cloned()
        .unwrap_or_default();
    assert!(scope.contains(&format!("/{REGION}/s3/aws4_request")), "{scope}");
}

#[test]
fn every_key_in_a_listing_is_read_and_its_entities_are_unescaped() {
    assert_eq!(
        parse_keys("<Contents><Key>a</Key></Contents><Contents><Key>b/c</Key></Contents>"),
        vec!["a", "b/c"]
    );
    assert_eq!(parse_keys("<ListBucketResult/>"), Vec::<String>::new());
    // An unterminated element ends the scan rather than producing a key made
    // of the rest of the document.
    assert_eq!(parse_keys("<Key>a</Key><Key>unterminated"), vec!["a"]);
    assert_eq!(parse_keys("<Key>a&amp;b</Key>"), vec!["a&b"]);
    assert_eq!(parse_keys("<Key>&lt;x&gt;</Key>"), vec!["<x>"]);
    // `&amp;lt;` is the text `&lt;`, not the character `<`. The order the
    // replacements are applied in is what makes that true.
    assert_eq!(parse_keys("<Key>&amp;lt;</Key>"), vec!["&lt;"]);
}

#[test]
fn a_listing_that_is_not_truncated_has_no_next_page_whatever_else_it_carries() {
    assert_eq!(next_token("<IsTruncated>false</IsTruncated>"), None);
    // Not truncated, and carrying a token anyway. Following it would page
    // forever on a server that always sends one.
    assert_eq!(
        next_token("<IsTruncated>false</IsTruncated><NextContinuationToken>t</NextContinuationToken>"),
        None
    );
    assert_eq!(
        next_token("<IsTruncated>true</IsTruncated><NextContinuationToken>t</NextContinuationToken>"),
        Some("t".to_string())
    );
    // Truncated with no token is a page this build cannot follow, and the
    // caller's page cap is what stops that becoming a silent short list.
    assert_eq!(next_token("<IsTruncated>true</IsTruncated>"), None);
}

#[test]
fn a_query_string_is_sorted_by_its_encoded_name() {
    assert_eq!(
        canonical_query(&[
            ("prefix", "apex-backup/".to_string()),
            ("list-type", "2".to_string()),
        ]),
        "list-type=2&prefix=apex-backup%2F"
    );
    assert_eq!(canonical_query(&[]), "");
}

#[test]
fn the_host_header_carries_a_port_only_when_it_is_not_the_schemes_default() {
    let info = |scheme: &str, port: Option<u16>| apex_secret_core::store::ServiceInfo {
        service: "aws".into(),
        host: "s3.example".into(),
        scheme: scheme.into(),
        username: ACCESS_KEY_ID.into(),
        path: String::new(),
        auth: "bearer".into(),
        port,
        added: 0,
    };
    assert_eq!(host_header(&info("https", None)), "s3.example");
    assert_eq!(host_header(&info("https", Some(443))), "s3.example");
    assert_eq!(host_header(&info("http", Some(80))), "s3.example");
    assert_eq!(host_header(&info("https", Some(9000))), "s3.example:9000");
    assert_eq!(host_header(&info("http", Some(9000))), "s3.example:9000");
    assert_eq!(origin(&info("http", Some(9000))), "http://s3.example:9000");
}

/// The vocabulary, pinned. Two operations and no more: an `s3.bucket.create`
/// or an `s3.object.delete` appearing here would be a new permission an owner
/// never granted.
#[test]
fn this_provider_offers_exactly_two_operations_and_neither_is_grantable_everywhere() {
    let ids: Vec<&str> = SPEC.operations.iter().map(|op| op.id).collect();
    assert_eq!(ids, vec!["s3.object.read", "s3.object.write"]);
    for op in SPEC.operations {
        assert!(
            !op.same_everywhere,
            "'{}' resolves its bucket out of the project's own file, so it is \
             a different permission in every directory",
            op.id
        );
    }
    SPEC.validate().expect("the declaration is well formed");
}
