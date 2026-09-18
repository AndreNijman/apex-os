//! The Graph provider, against **two** loopback doubles: the API, and the host
//! it redirects the download to.
//!
//! # What this file can and cannot say
//!
//! Neither double is Microsoft. They are HTTP servers that record the method,
//! the request target and the headers they were sent, and answer on whether
//! the `Authorization` header carries the token this fixture stored. So what
//! is checked here is the **wiring**, which is the part that can be wrong in a
//! way no reading of the code shows:
//!
//!   * the path requested is the stored endpoint's with
//!     `/drive/items/<id>/content` under it;
//!   * the token is presented to Graph as `Authorization: Bearer <token>`;
//!   * the `302` is followed exactly once, to the host the `Location` named;
//!   * **the download host is sent no `Authorization` header and no byte of
//!     the token** — measured off the wire, not read off the code;
//!   * the pre-authenticated URL the `Location` carried appears in nothing
//!     that comes back: not the reply, not an error, not the audit trail;
//!   * a `Location` that is not http(s), or that changes scheme, is refused
//!     and the download host is never dialled;
//!   * a second redirect is not followed.
//!
//! Each of those is a way for this provider to compose a request a real Graph
//! answers wrongly — or to leak a capability — and each one goes red here if
//! it breaks.
//!
//! # Nothing here reaches Microsoft
//!
//! There is no Microsoft account on any machine this repository is built on.
//! Both doubles are on `127.0.0.1`, the token is a literal in this file that
//! authorises nothing, and the only way a credential on `127.0.0.1` reaches
//! this provider is [`MsgraphProvider::at`], which is `#[cfg(test)]`. What is
//! therefore NOT proven anywhere: that a real Microsoft access token, obtained
//! by the real device grant and renewed by the real `oauth` provider, is
//! accepted by the real Graph; that `Files.Read` resolves on the `common`
//! tenant for a public client; or that Graph in fact answers `302` rather than
//! something this code would treat differently.

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
const TOKEN: &str = "eyJ0eXAiOiJKV1QiLCJhbGciOiJSUzI1NiJ9.ExampleGraphAccessToken.nope";
/// A driveItem id shaped like a work-or-school one: `01` and base32.
const ITEM_ID: &str = "01BYE5RZ6QN3ZWBTUFOFD3GSPGOHDJD36K";
/// What the download host hands back for a file it has.
const CONTENTS: &str = "the quick brown fox\n";
/// The account this fixture stores, in the store's own naming.
const SERVICE: &str = "account.microsoft.work";
/// Where a Microsoft account's credential is stored with its path pinned.
const GRAPH_PATH: &str = "/v1.0/me";
/// The pre-authentication in the download URL's query.
///
/// A real one is a `tempauth=` blob that reads the file for anyone who has it,
/// which is why every assertion below looks for this string in places it must
/// never be.
const PREAUTH: &str = "apex-msgraph-preauth-4c1d8e-do-not-leak";

/// One request a double saw.
#[derive(Debug, Clone)]
struct Seen {
    method: String,
    /// Path and query, as they arrived on the request line.
    target: String,
    headers: BTreeMap<String, String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// Graph redirects to the download host, which answers with the file.
    /// What the documented flow looks like.
    Normal,
    /// The token has expired. Graph's own shape for it.
    Expired,
    /// A 302 with no `Location` at all.
    NoLocation,
    /// A `Location` that is not an http(s) URL.
    NotHttp,
    /// A `Location` that is `https` while the credential is stored `http`.
    SchemeChange,
    /// The download host answers with a redirect of its own.
    DownloadRedirects,
    /// The download link has expired, and the far side echoes the query it was
    /// sent — which is the part that authorises.
    DownloadGone,
    /// A 200 whose body quotes the request back. Storage front ends wrap a
    /// reply in an envelope that names what was asked for, and this is the
    /// only way the SUCCESS path's scrub can be measured: a file does not
    /// contain its own download link, so nothing else would ever exercise it.
    DownloadEchoes,
}

struct Fake {
    graph_port: u16,
    download_port: u16,
    graph_seen: Arc<Mutex<Vec<Seen>>>,
    download_seen: Arc<Mutex<Vec<Seen>>>,
}

impl Fake {
    fn start(mode: Mode) -> Fake {
        // The download host first: the API double has to be able to name it.
        let download = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let download_port = download.local_addr().expect("addr").port();
        let download_seen = Arc::new(Mutex::new(Vec::new()));
        {
            let recorder = Arc::clone(&download_seen);
            std::thread::spawn(move || {
                for stream in download.incoming().flatten() {
                    let recorder = Arc::clone(&recorder);
                    std::thread::spawn(move || serve_download(stream, &recorder, mode));
                }
            });
        }

        let graph = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let graph_port = graph.local_addr().expect("addr").port();
        let graph_seen = Arc::new(Mutex::new(Vec::new()));
        {
            let recorder = Arc::clone(&graph_seen);
            std::thread::spawn(move || {
                for stream in graph.incoming().flatten() {
                    let recorder = Arc::clone(&recorder);
                    std::thread::spawn(move || {
                        serve_graph(stream, &recorder, mode, download_port)
                    });
                }
            });
        }

        Fake {
            graph_port,
            download_port,
            graph_seen,
            download_seen,
        }
    }

    fn graph_seen(&self) -> Vec<Seen> {
        self.graph_seen.lock().expect("lock").clone()
    }

    fn download_seen(&self) -> Vec<Seen> {
        self.download_seen.lock().expect("lock").clone()
    }
}

/// Read a request off the wire, recording exactly what arrived.
fn take_request(reader: &mut BufReader<TcpStream>, recorder: &Arc<Mutex<Vec<Seen>>>) -> Seen {
    let mut first = String::new();
    let _ = reader.read_line(&mut first);
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

    let seen = Seen {
        method,
        target,
        headers,
    };
    recorder.lock().expect("lock").push(seen.clone());
    seen
}

fn reply(stream: &mut TcpStream, status: &str, extra: &[(&str, String)], payload: &str) {
    let mut head = format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\n",
        payload.len()
    );
    for (name, value) in extra {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("\r\n");
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(payload.as_bytes());
    let _ = stream.flush();
}

fn serve_graph(
    mut stream: TcpStream,
    recorder: &Arc<Mutex<Vec<Seen>>>,
    mode: Mode,
    download_port: u16,
) {
    let mut reader = BufReader::new(stream.try_clone().expect("clone"));
    let got = take_request(&mut reader, recorder);

    // The credential is checked against what ARRIVED, never against what the
    // provider meant to send. A request with no bearer, or with the token
    // spelled some other way, is a 401 here exactly as it would be at Graph.
    let presented = got
        .headers
        .get("authorization")
        .map(String::as_str)
        .unwrap_or("");
    let path = got.target.split('?').next().unwrap_or("").to_string();
    let download = format!("http://127.0.0.1:{download_port}/download/{ITEM_ID}?tempauth={PREAUTH}");

    if presented != format!("Bearer {TOKEN}") {
        return reply(
            &mut stream,
            "401 Unauthorized",
            &[("Content-Type", "application/json".into())],
            r#"{"error":{"code":"InvalidAuthenticationToken","message":"Access token is empty."}}"#,
        );
    }
    if mode == Mode::Expired {
        return reply(
            &mut stream,
            "401 Unauthorized",
            &[("Content-Type", "application/json".into())],
            r#"{"error":{"code":"InvalidAuthenticationToken","message":"Lifetime validation failed, the token is expired."}}"#,
        );
    }
    if path != format!("{GRAPH_PATH}/drive/items/{ITEM_ID}/content") {
        return reply(
            &mut stream,
            "404 Not Found",
            &[("Content-Type", "application/json".into())],
            r#"{"error":{"code":"itemNotFound","message":"The resource could not be found."}}"#,
        );
    }
    match mode {
        // The documented answer, and the Location carries a capability.
        Mode::Normal | Mode::DownloadRedirects | Mode::DownloadGone | Mode::DownloadEchoes => reply(
            &mut stream,
            "302 Found",
            &[("Location", download)],
            "",
        ),
        // A 302 that names nowhere. Not something Graph does; something a
        // proxy or a broken gateway does, and the provider must not read an
        // empty `%{redirect_url}` as a URL.
        Mode::NoLocation => reply(&mut stream, "302 Found", &[], ""),
        Mode::NotHttp => reply(
            &mut stream,
            "302 Found",
            &[("Location", "file:///etc/shadow".to_string())],
            "",
        ),
        Mode::SchemeChange => reply(
            &mut stream,
            "302 Found",
            &[(
                "Location",
                format!("https://127.0.0.1:{download_port}/download/{ITEM_ID}?tempauth={PREAUTH}"),
            )],
            "",
        ),
        Mode::Expired => unreachable!("handled above"),
    }
}

fn serve_download(mut stream: TcpStream, recorder: &Arc<Mutex<Vec<Seen>>>, mode: Mode) {
    let mut reader = BufReader::new(stream.try_clone().expect("clone"));
    let got = take_request(&mut reader, recorder);
    let query = got.target.split_once('?').map(|(_, q)| q).unwrap_or("");

    match mode {
        Mode::DownloadRedirects => reply(
            &mut stream,
            "302 Found",
            &[("Location", "http://127.0.0.1:9/somewhere-else".to_string())],
            "",
        ),
        // A real expired link answers 403 with an XML document that quotes the
        // request back. The echo is deliberate: it is how a pre-authenticated
        // URL leaks through a failure path.
        Mode::DownloadGone => reply(
            &mut stream,
            "403 Forbidden",
            &[("Content-Type", "application/xml".into())],
            &format!("<Error><Message>Access denied for ?{query}</Message></Error>"),
        ),
        Mode::DownloadEchoes => reply(
            &mut stream,
            "200 OK",
            &[("Content-Type", "text/plain".into())],
            &format!("{CONTENTS}<!-- served for ?{query} -->"),
        ),
        _ => reply(
            &mut stream,
            "200 OK",
            &[("Content-Type", "text/plain".into())],
            CONTENTS,
        ),
    }
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

    /// `host` is what the credential is PINNED to. The doubles always listen on
    /// loopback; a fixture that stores some other host is testing the host pin
    /// and expects to be refused before anything is sent.
    fn with(name: &str, mode: Mode, granted: &[&str], host: &str) -> Fixture {
        let fake = Fake::start(mode);
        let tag = format!("{name}-{}-{}", std::process::id(), fake.graph_port);
        // /var/tmp, this project's rule for anything a suite creates.
        let store = PathBuf::from("/var/tmp").join(format!("apex-msgraph-store-{tag}"));
        let project = PathBuf::from("/var/tmp").join(format!("apex-msgraph-project-{tag}"));
        std::fs::remove_dir_all(&store).ok();
        std::fs::remove_dir_all(&project).ok();
        std::fs::create_dir_all(&project).expect("project");

        let mut registry = Registry::new();
        // The `#[cfg(test)]` route to a loopback host. `MsgraphProvider::new()`
        // — the only constructor that ships — would refuse every request in
        // this file at `bind`, which is the point of it.
        registry
            .register(Box::new(MsgraphProvider::at("127.0.0.1")))
            .expect("register");
        let service = Service::new(Store::new(store.clone()), false, registry);

        let peer = me();
        // `http` is accepted for a loopback host: nothing crosses a network.
        // A real account is stored `https` against `graph.microsoft.com`.
        let scheme = if host == "127.0.0.1" { "http" } else { "https" };
        let port = if host == "127.0.0.1" {
            Some(fake.graph_port)
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
                    path: GRAPH_PATH,
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
    SPEC.validate()
        .expect("the msgraph provider must declare validly");
    // Every scope the account model offers for this transport must exist here.
    // The two halves are in different crates — the vocabulary a user is shown
    // and the operations a daemon will run — and a scope naming an operation
    // nothing implements is a grant that can never be used. The cross-crate
    // gate in `providers::tests` says the same thing over the whole table;
    // this says it for the provider whose file it is.
    let mut checked = 0;
    for p in account::PROVIDERS.iter().filter(|p| p.transport == SPEC.id) {
        for scope in p.scopes {
            assert!(
                SPEC.operations.iter().any(|o| o.id == scope.operation),
                "{} offers scope {} -> {}, which this provider does not implement",
                p.id,
                scope.name,
                scope.operation
            );
            checked += 1;
        }
    }
    // A loop over an empty table proves nothing, and this provider exists
    // precisely because `GRAPH_SCOPES` was empty for six rounds.
    assert!(
        checked > 0,
        "no account scope routes into '{}', so it is unreachable from `apex \
         account grant` and the loop above checked nothing",
        SPEC.id
    );
}

#[test]
fn the_api_host_is_the_one_the_account_table_pins_microsoft_to() {
    // Read off the table rather than spelled twice. A build where these
    // disagreed would refuse every account `apex account add microsoft.<name>`
    // had itself just stored.
    assert_eq!(
        MsgraphProvider::api_host().expect("a fixed host for microsoft"),
        "graph.microsoft.com"
    );
    let shipped = MsgraphProvider::new();
    assert!(shipped.is_graph("graph.microsoft.com").expect("host"));
    // Case and the DNS root dot name the same host.
    assert!(shipped.is_graph("GRAPH.microsoft.com.").expect("host"));
    // And the loopback a double runs on is NOT in the shipped table, which is
    // the whole reason `at()` exists.
    assert!(!shipped.is_graph("127.0.0.1").expect("host"));
    assert!(MsgraphProvider::at("127.0.0.1")
        .is_graph("127.0.0.1")
        .expect("host"));
}

#[test]
fn a_resource_is_a_onedrive_item_id_and_nothing_that_could_leave_the_endpoint() {
    let op = &SPEC.operations[0];
    assert!(matches!(op.resource, ResourceKind::Name));
    let none = Params::new();
    assert!(op.check(ITEM_ID, &none).is_ok());
    for escape in [
        "https://attacker.example/x",
        "../../etc/passwd",
        "a/b",
        "x?select=id",
        "x#frag",
        "host:8080",
        "-oProxyCommand",
        "",
    ] {
        assert!(
            op.check(escape, &none).is_err(),
            "{escape:?} was accepted as a OneDrive item id"
        );
    }
    // The narrowing this transport ships with, asserted so it is a documented
    // property rather than a surprise: Microsoft's own example response on the
    // `driveItem: content` page is `{"id": "12319191!11919"}`, and a consumer
    // OneDrive id of that form is refused by the shared resource vocabulary.
    // Named here so that lifting it is a deliberate change to `valid_name`.
    assert!(op.check("12319191!11919", &none).is_err());
    // And a parameter nobody declared is refused, not ignored.
    let mut smuggled = Params::new();
    smuggled.insert("select".into(), "id".into());
    assert!(op.check(ITEM_ID, &smuggled).is_err());
}

fn stored(host: &str, scheme: &str) -> apex_secret_core::store::ServiceInfo {
    apex_secret_core::store::ServiceInfo {
        service: SERVICE.into(),
        host: host.into(),
        scheme: scheme.into(),
        username: "me@example.com".into(),
        path: GRAPH_PATH.into(),
        auth: "bearer".into(),
        port: None,
        added: 0,
    }
}

#[test]
fn the_url_is_the_stored_endpoint_with_the_item_id_under_it() {
    let info = stored("graph.microsoft.com", "https");
    assert_eq!(
        graph_url(&info, ITEM_ID).expect("an item id"),
        format!("https://graph.microsoft.com/v1.0/me/drive/items/{ITEM_ID}/content")
    );
    // A second, independent refusal of the same escapes. `OperationSpec::check`
    // runs first in production; this one runs in the function that actually
    // builds the string, because "the caller checked" is not a property that
    // function can see.
    for escape in ["../../../etc/passwd", "a/b", "x?select=id", "-x", "", "1!2"] {
        assert!(graph_url(&info, escape).is_err(), "{escape}");
    }
}

#[test]
fn no_operation_here_may_be_granted_everywhere() {
    // An item id resolves to a different file on every account, and the grant
    // is per account; nothing here earns `*`.
    for op in SPEC.operations {
        assert!(!op.same_everywhere, "{}", op.id);
        assert!(!op.supersedes_credentials, "{}", op.id);
    }
}

// ── the two configurations, side by side ───────────────────────────────────

/// The property the whole redirect design rests on, as a property of a
/// function rather than a line in a longer one.
#[test]
fn the_download_configuration_has_nowhere_to_put_a_credential() {
    let authorization = format!("Bearer {TOKEN}");
    let hop1 = content_config(
        &format!("https://graph.microsoft.com{GRAPH_PATH}/drive/items/{ITEM_ID}/content"),
        &authorization,
        "https",
    )
    .expect("hop one");
    // Hop one MUST carry it — otherwise the assertion below would pass on a
    // build that authenticated nothing at all.
    assert!(hop1.contains(&format!("Authorization: {authorization}")), "{hop1}");
    assert!(hop1.contains(TOKEN));
    // And it must not be told to follow anything: following is what would put
    // this header on the download host's connection.
    assert!(
        !hop1.contains("location"),
        "hop one was told to follow redirects: {hop1}"
    );
    assert!(hop1.contains("%{redirect_url}"), "{hop1}");

    let hop2 = download_config("https://files.example/y23vmag?tempauth=x", "https")
        .expect("hop two");
    assert!(
        !hop2.to_ascii_lowercase().contains("authorization"),
        "the download hop carries an Authorization header: {hop2}"
    );
    assert!(
        !hop2.contains(TOKEN),
        "the download hop carries the token: {hop2}"
    );
    assert!(
        !hop2.contains("location"),
        "the download hop was told to follow redirects: {hop2}"
    );
    // The scheme is pinned on this hop too, so curl refuses a protocol change
    // even if the check above it were removed.
    assert!(hop2.contains("proto = \"=https\""), "{hop2}");
    assert!(hop2.contains("max-filesize"), "{hop2}");
}

/// The other half of the same design: the words this hop returns are composed
/// by a function that is not given the URL.
#[test]
fn what_the_download_hop_reports_is_built_without_the_url_in_scope() {
    // A 2xx hands the body straight back.
    let ok = download_outcome("https://files.example", "graph.microsoft.com", Some(200), "bytes", 0)
        .expect("a 2xx");
    assert_eq!(ok.code, 0);
    assert_eq!(ok.output, "bytes");
    assert!(ok.created.is_none() && ok.replaced.is_empty());

    // A non-2xx names the target and carries the far side's reason.
    let gone = download_outcome(
        "https://files.example",
        "graph.microsoft.com",
        Some(403),
        "<Error>link expired</Error>",
        0,
    )
    .expect("a 403 is an outcome, not an error");
    assert_eq!(gone.code, 1);
    assert!(gone.output.contains("403"), "{}", gone.output);
    assert!(gone.output.contains("https://files.example"), "{}", gone.output);
    assert!(gone.output.contains("link expired"), "{}", gone.output);

    // A 3xx says why it stopped rather than looking like a read that worked.
    let again = download_outcome("https://files.example", "graph.microsoft.com", Some(302), "", 0)
        .expect("a 302");
    assert_eq!(again.code, 1);
    assert!(again.output.contains("one redirect"), "{}", again.output);

    // No status at all is an error, and it says curl's exit code and NOT
    // curl's words — which is the whole reason this function exists.
    let e = download_outcome("https://files.example", "graph.microsoft.com", None, "", 7)
        .expect_err("no status must not read as an empty file");
    let said = e.to_string();
    assert!(said.contains("curl exited 7"), "{said}");
    assert!(said.contains("https://files.example"), "{said}");
}

#[test]
fn a_download_location_that_could_be_a_second_configuration_line_is_refused() {
    // The far side chose this string. A newline in it is a second curl
    // configuration directive of somebody else's choosing.
    for bad in [
        "http://files.example/x\nheader = \"Authorization: Bearer stolen\"",
        "http://files.example/x\r\nurl = \"http://elsewhere\"",
        "http://files.example/\u{7f}",
        "",
    ] {
        let e = download_config(bad, "http").expect_err("accepted a hostile Location");
        assert!(
            matches!(e, ProviderError::Refused(_)),
            "{bad:?} -> {e:?}"
        );
    }
}

#[test]
fn a_download_location_that_is_not_the_credentials_own_scheme_is_refused() {
    for (bad, scheme) in [
        ("file:///etc/shadow", "https"),
        ("ftp://files.example/x", "https"),
        ("gopher://files.example/x", "http"),
        // The downgrade: an https credential redirected onto plain http.
        ("http://files.example/x", "https"),
        // And the other direction, which is the loopback fixture's case.
        ("https://files.example/x", "http"),
    ] {
        let e = download_config(bad, scheme).expect_err("accepted a scheme change");
        assert!(matches!(e, ProviderError::Refused(_)), "{bad} -> {e:?}");
    }
    // The control: the same scheme is accepted, so the assertions above are
    // about the scheme and not about the function refusing everything.
    assert!(download_config("https://files.example/x", "https").is_ok());
}

#[test]
fn the_write_out_line_is_read_off_the_end_and_the_body_above_it_is_left_alone() {
    // The body is the far side's and may be anything, including something that
    // looks like the status line. Split from the END.
    // THREE lines, not two: with a single newline in the input, splitting
    // from the front and splitting from the back give the same answer, and a
    // test built on two lines says nothing about which end was read.
    let (body, status, location) = split_status_and_redirect(
        "302 http://not-this-one/\nsecond line of the body\n302 http://the-real-one/?t=1",
    );
    assert_eq!(body, "302 http://not-this-one/\nsecond line of the body");
    assert_eq!(status, Some(302));
    assert_eq!(location.as_deref(), Some("http://the-real-one/?t=1"));

    // The download hop's parse is a second function and gets the same
    // three-line treatment.
    let (body, status) = split_status("200 not the status\nnor this\n404 ");
    assert_eq!(body, "200 not the status\nnor this");
    assert_eq!(status, Some(404));

    // No redirect: `%{redirect_url}` is empty and must not read as a URL.
    let (body, status, location) = split_status_and_redirect("hello\n200 ");
    assert_eq!(body, "hello");
    assert_eq!(status, Some(200));
    assert_eq!(location, None);

    // An empty body still leaves the line readable.
    let (body, status, location) = split_status_and_redirect("\n404 ");
    assert_eq!(body, "");
    assert_eq!(status, Some(404));
    assert_eq!(location, None);
}

// ── the round trip ─────────────────────────────────────────────────────────

/// The whole of what this transport is for, and the five things about the two
/// requests that have to be right.
#[test]
fn a_granted_read_follows_one_redirect_without_the_token_and_hands_back_the_file() {
    let f = Fixture::new("roundtrip", Mode::Normal, &["msgraph.file.read"]);

    let reply = f.use_it(f.record("msgraph.file.read", ITEM_ID));
    let Response::Performed {
        exit_code,
        output: text,
        ..
    } = &reply
    else {
        panic!("a granted read did not produce output: {reply:?}");
    };
    assert_eq!(*exit_code, 0, "{text}");
    assert!(
        text.contains(CONTENTS.trim()),
        "the file did not come back: {text}"
    );

    // Hop one: Graph, with the token.
    let graph = f.fake.graph_seen();
    assert_eq!(graph.len(), 1, "{graph:?}");
    assert_eq!(graph[0].method, "GET");
    assert_eq!(
        graph[0].target,
        format!("{GRAPH_PATH}/drive/items/{ITEM_ID}/content"),
        "the request target is the stored endpoint with the item id under it"
    );
    assert_eq!(
        graph[0].headers.get("authorization").map(String::as_str),
        Some(format!("Bearer {TOKEN}").as_str()),
        "the token has to be presented as a bearer: {:?}",
        graph[0].headers
    );

    // Hop two: the download host the `Location` named, with NOTHING.
    let download = f.fake.download_seen();
    assert_eq!(
        download.len(),
        1,
        "the redirect was not followed exactly once: {download:?}"
    );
    assert_eq!(download[0].method, "GET");
    assert_eq!(
        download[0].target,
        format!("/download/{ITEM_ID}?tempauth={PREAUTH}")
    );
    // It went to the AUTHORITY the `Location` named and not back to Graph.
    // Both doubles are on loopback, so the port is the only thing that tells
    // them apart — and without this the assertion above would also pass on a
    // build that re-asked Graph for the same path.
    assert_ne!(f.fake.download_port, f.fake.graph_port);
    assert_eq!(
        download[0].headers.get("host").map(String::as_str),
        Some(format!("127.0.0.1:{}", f.fake.download_port).as_str()),
        "{:?}",
        download[0].headers
    );
    assert_eq!(
        graph[0].headers.get("host").map(String::as_str),
        Some(format!("127.0.0.1:{}", f.fake.graph_port).as_str()),
    );
    assert!(
        !download[0].headers.contains_key("authorization"),
        "the access token was sent to the download host: {:?}",
        download[0].headers
    );
    // Not just that header: no header at all carries a byte of the token.
    for (name, value) in &download[0].headers {
        assert!(
            !value.contains(TOKEN),
            "the token reached the download host in '{name}': {value}"
        );
    }

    // The pre-authenticated URL is a capability and comes back nowhere.
    assert!(!text.contains(PREAUTH), "the download link was returned: {text}");
    assert!(!text.contains(TOKEN));
    let trail = f.trail();
    assert!(
        !trail.contains(PREAUTH),
        "the download link reached the audit trail: {trail}"
    );
    assert!(!trail.contains(TOKEN), "the token reached the audit trail");
    // The audit line names the account and the item id, which is what makes it
    // worth reading.
    assert!(trail.contains(ITEM_ID), "{trail}");
    assert!(trail.contains(SERVICE), "{trail}");
}

/// The success path's scrub, which nothing else can reach.
///
/// A file does not contain its own download link, so a double that answers
/// with plain contents would let `without_the_download_url` be deleted from
/// the 2xx path and stay green. This one answers 200 with an envelope that
/// quotes the request — which is what a storage front end does — and the
/// assertion is that the capability still does not come back.
#[test]
fn a_reply_that_quotes_the_download_link_back_still_does_not_carry_it() {
    let f = Fixture::new("echo", Mode::DownloadEchoes, &["msgraph.file.read"]);

    let reply = f.use_it(f.record("msgraph.file.read", ITEM_ID));
    let Response::Performed {
        exit_code,
        output: text,
        ..
    } = &reply
    else {
        panic!("{reply:?}");
    };
    assert_eq!(*exit_code, 0, "{text}");
    assert!(text.contains(CONTENTS.trim()), "{text}");
    assert!(
        !text.contains(PREAUTH),
        "a 2xx body carried the pre-authenticated URL back: {text}"
    );
    assert!(!f.trail().contains(PREAUTH));
}

/// A grant is per operation, and this provider offers exactly one.
#[test]
fn an_ungranted_read_never_reaches_graph() {
    let f = Fixture::new("ungranted", Mode::Normal, &[]);

    let reply = f.use_it(f.record("msgraph.file.read", ITEM_ID));
    assert!(
        matches!(reply, Response::Error { .. }),
        "an ungranted read was performed: {reply:?}"
    );
    assert!(
        f.fake.graph_seen().is_empty(),
        "a request reached the far side without a grant"
    );
    assert!(f.fake.download_seen().is_empty());
    assert!(!f.trail().contains(TOKEN));
}

/// The pin that makes this Microsoft's transport rather than any host's.
#[test]
fn a_credential_pinned_somewhere_other_than_microsoft_is_refused_before_anything_is_sent() {
    // Stored for a host that is neither Microsoft's nor the test injection's.
    // The framework's own pin cannot catch this — it only checks that the
    // provider agrees with the store, and here it would.
    let f = Fixture::with(
        "wronghost",
        Mode::Normal,
        &["msgraph.file.read"],
        "graph.attacker.example",
    );

    let reply = f.use_it(f.record("msgraph.file.read", ITEM_ID));
    let Response::Error { message, .. } = &reply else {
        panic!("a Microsoft token was spent at another host: {reply:?}");
    };
    assert!(
        message.contains("graph.attacker.example"),
        "the refusal has to name the host it refused: {message}"
    );
    assert!(
        message.contains("graph.microsoft.com"),
        "and the host it would accept: {message}"
    );
    assert!(
        f.fake.graph_seen().is_empty(),
        "a Microsoft access token was sent to a host that is not Graph"
    );
    assert!(!f.trail().contains(TOKEN));
}

/// A 401 is the far side's answer and travels, with the command that fixes it.
#[test]
fn an_expired_token_comes_back_as_microsofts_own_reason_and_replaces_nothing() {
    let f = Fixture::new("expired", Mode::Expired, &["msgraph.file.read"]);

    let reply = f.use_it(f.record("msgraph.file.read", ITEM_ID));
    let Response::Performed {
        exit_code,
        output: text,
        ..
    } = &reply
    else {
        panic!("a 401 was not carried back as output: {reply:?}");
    };
    assert_eq!(*exit_code, 1, "a 401 must not read as success: {text}");
    assert!(text.contains("401"), "{text}");
    // Microsoft's own words, not this build's guess at them.
    assert!(
        text.contains("Lifetime validation failed"),
        "the far side's reason has to travel: {text}"
    );
    // And the one command a person can act on. Derived from the service name,
    // so a credential that is not an account gets no suggestion.
    assert!(
        text.contains("apex account refresh microsoft.work"),
        "a 401 on an account should name the renewal command: {text}"
    );
    assert!(!text.contains(TOKEN));
    assert!(!f.trail().contains(TOKEN));
    // Nothing was downloaded, because nothing was redirected.
    assert!(f.fake.download_seen().is_empty());
}

#[test]
fn a_file_this_client_cannot_see_is_a_404_that_says_so() {
    let f = Fixture::new("notfound", Mode::Normal, &["msgraph.file.read"]);

    let reply = f.use_it(f.record("msgraph.file.read", "01SOMEOTHERITEMIDENTIRELY"));
    let Response::Performed {
        exit_code,
        output: text,
        ..
    } = &reply
    else {
        panic!("a 404 was not carried back as output: {reply:?}");
    };
    assert_eq!(*exit_code, 1, "{text}");
    assert!(text.contains("404"), "{text}");
    assert!(text.contains("itemNotFound"), "{text}");
    // A 404 is about the FILE, so it must not send somebody to renew a token
    // that is working.
    assert!(
        !text.contains("apex account refresh"),
        "a 404 blamed the token: {text}"
    );
    // It did reach the far side, which is what tells a 404 apart from a
    // refusal: the request was well-formed and the file is not this client's.
    assert_eq!(f.fake.graph_seen().len(), 1);
    assert!(f.fake.download_seen().is_empty());
}

#[test]
fn a_resource_that_is_not_an_item_id_is_refused_before_the_credential_is_read() {
    let f = Fixture::new("badid", Mode::Normal, &["msgraph.file.read"]);

    for bad in ["../../etc/passwd", "a/b", "x?select=id", "12319191!11919"] {
        let reply = f.use_it(f.record("msgraph.file.read", bad));
        assert!(
            matches!(reply, Response::Error { .. }),
            "{bad} was accepted: {reply:?}"
        );
    }
    assert!(
        f.fake.graph_seen().is_empty(),
        "a malformed item id reached the far side"
    );
    assert!(!f.trail().contains(TOKEN));
}

// ── the redirect, and the four ways it can go wrong ────────────────────────

#[test]
fn a_redirect_that_names_nowhere_is_an_error_rather_than_a_fetch() {
    let f = Fixture::new("nolocation", Mode::NoLocation, &["msgraph.file.read"]);

    let reply = f.use_it(f.record("msgraph.file.read", ITEM_ID));
    let Response::Error { message, .. } = &reply else {
        panic!("a 302 with no Location was not an error: {reply:?}");
    };
    assert!(message.contains("302"), "{message}");
    assert_eq!(f.fake.graph_seen().len(), 1);
    assert!(
        f.fake.download_seen().is_empty(),
        "something was fetched with no Location to fetch it from"
    );
}

#[test]
fn a_redirect_to_something_that_is_not_http_is_refused_and_fetches_nothing() {
    let f = Fixture::new("nothttp", Mode::NotHttp, &["msgraph.file.read"]);

    let reply = f.use_it(f.record("msgraph.file.read", ITEM_ID));
    let Response::Error { message, .. } = &reply else {
        panic!("a file:// Location was followed: {reply:?}");
    };
    assert!(
        message.contains("http"),
        "the refusal has to say what it would accept: {message}"
    );
    // The path it would have read is nowhere in the message either.
    assert!(!message.contains("/etc/shadow"), "{message}");
    assert!(f.fake.download_seen().is_empty());
}

#[test]
fn a_redirect_that_changes_scheme_is_refused_and_fetches_nothing() {
    // The credential is stored `http` (loopback); the Location says `https`.
    // On a real account the interesting direction is the downgrade, and it is
    // the same check — `download_config`'s unit test runs both.
    let f = Fixture::new("schemechange", Mode::SchemeChange, &["msgraph.file.read"]);

    let reply = f.use_it(f.record("msgraph.file.read", ITEM_ID));
    let Response::Error { message, .. } = &reply else {
        panic!("a scheme-changing Location was followed: {reply:?}");
    };
    assert!(message.contains("scheme"), "{message}");
    assert!(
        !message.contains(PREAUTH),
        "the refusal carried the pre-authenticated URL: {message}"
    );
    assert!(f.fake.download_seen().is_empty());
    assert!(!f.trail().contains(PREAUTH));
}

#[test]
fn a_second_redirect_is_not_followed() {
    let f = Fixture::new("chain", Mode::DownloadRedirects, &["msgraph.file.read"]);

    let reply = f.use_it(f.record("msgraph.file.read", ITEM_ID));
    let Response::Performed {
        exit_code,
        output: text,
        ..
    } = &reply
    else {
        panic!("{reply:?}");
    };
    assert_eq!(*exit_code, 1, "a redirect read as a file: {text}");
    assert!(text.contains("302"), "{text}");
    assert!(
        text.contains("one redirect"),
        "the reply has to say the chain stops here: {text}"
    );
    // One hop, and exactly one: the download host was asked once and its own
    // redirect was not walked.
    assert_eq!(f.fake.download_seen().len(), 1);
    assert!(!text.contains(PREAUTH));
}

/// The failure path a pre-authenticated URL leaks through, if it leaks at all.
#[test]
fn a_download_that_fails_names_the_host_and_never_the_preauthenticated_url() {
    let f = Fixture::new("gone", Mode::DownloadGone, &["msgraph.file.read"]);

    let reply = f.use_it(f.record("msgraph.file.read", ITEM_ID));
    let Response::Performed {
        exit_code,
        output: text,
        ..
    } = &reply
    else {
        panic!("{reply:?}");
    };
    assert_eq!(*exit_code, 1, "a 403 read as a file: {text}");
    assert!(text.contains("403"), "{text}");
    // The far side's own reason travels, for `gdrive`'s argument: it is the
    // only thing that says whether the link expired or the storage is down.
    assert!(
        text.contains("Access denied"),
        "the download host's reason was thrown away: {text}"
    );
    // It says WHERE, because that is the part a person can act on...
    assert!(text.contains("http://127.0.0.1"), "{text}");
    // ...and not WHAT, because the rest of that URL reads the file for anyone
    // who has it. The far side echoed the query back deliberately.
    assert!(
        !text.contains(PREAUTH),
        "the pre-authenticated URL came back in a failure: {text}"
    );
    assert!(!text.contains("tempauth"), "{text}");
    assert!(
        !f.trail().contains(PREAUTH),
        "the pre-authenticated URL reached the audit trail"
    );
    assert_eq!(f.fake.download_seen().len(), 1);
}

/// The scrub itself, in both halves, because the assertions above would also
/// pass if the far side simply never echoed anything.
#[test]
fn the_download_url_is_removed_whole_and_by_its_query() {
    let url = format!("https://files.example/y23vmag?tempauth={PREAUTH}");
    // Whole.
    let said = without_the_download_url(&format!("fetching {url} failed"), &url);
    assert!(!said.contains(PREAUTH), "{said}");
    // Query alone, which is what an echoing far side sends back.
    let said = without_the_download_url(
        &format!("<Error>Access denied for ?tempauth={PREAUTH}</Error>"),
        &url,
    );
    assert!(!said.contains(PREAUTH), "{said}");
    // And a URL with no query is not a reason to mangle anything.
    assert_eq!(
        without_the_download_url("untouched", "https://files.example/y23vmag"),
        "untouched"
    );
}
