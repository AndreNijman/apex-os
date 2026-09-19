//! A browser capsule's connection, authenticated by the daemon that holds the
//! credential (P2-012, route B).
//!
//! The real `apex-secretd` binary on a private socket and store, a real
//! `SO_PEERCRED` connection, a real credential in a real store, and a real HTTP
//! server on loopback that records what it was sent. What is faked is only the
//! site.
//!
//! The caller here plays the part `apex-agentd` plays in the shipped path: it
//! sends `Present` and then writes the bytes a capsule's browser would have
//! written. That is the whole of what the runtime does on this leg, which is
//! why the substitution is honest — the runtime terminates TLS toward the
//! capsule and copies plaintext, and this file is the plaintext.
//!
//! Every assertion here is a pair. It is not enough that the credential reached
//! the site; the same run has to show it did **not** reach the caller. And a
//! refusal is not enough on its own either: the site's log has to be empty, or
//! "refused" would be indistinguishable from "reached the site and the site
//! said no".

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::{Arc, Mutex};

use apex_secret_core::capability::CapabilityRecord;
use apex_secret_core::client::Client;
use apex_secret_core::operation::BROWSER_PRESENT;
use apex_secret_core::protocol::{Request, Response};
use apex_secret_core::SecretValue;

/// Distinctive enough that a grep for it cannot match by accident.
const SENTINEL: &str = "apex-present-7c1e45b2-do-not-leak";

// ── the site ────────────────────────────────────────────────────────────────

/// A loopback HTTP server that records what it was sent and echoes it back.
///
/// Echoing the `Authorization` header into the body is deliberate and is the
/// harder half of the test: it is the one way a capsule could end up holding
/// the credential without APEX ever handing it one, so the scrubber has to be
/// exercised by a site that really does send it back.
struct Site {
    port: u16,
    seen: Arc<Mutex<Vec<String>>>,
    requests: Arc<Mutex<Vec<String>>>,
}

impl Site {
    fn start() -> Site {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let port = listener.local_addr().expect("addr").port();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let a = Arc::clone(&seen);
        let b = Arc::clone(&requests);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let a = Arc::clone(&a);
                let b = Arc::clone(&b);
                std::thread::spawn(move || answer(stream, &a, &b));
            }
        });
        Site {
            port,
            seen,
            requests,
        }
    }

    fn authorizations(&self) -> Vec<String> {
        self.seen.lock().expect("lock").clone()
    }

    fn request_lines(&self) -> Vec<String> {
        self.requests.lock().expect("lock").clone()
    }
}

fn answer(mut stream: TcpStream, seen: &Arc<Mutex<Vec<String>>>, requests: &Arc<Mutex<Vec<String>>>) {
    let mut reader = BufReader::new(stream.try_clone().expect("clone"));
    // EVERY `Authorization` header, not the last one. A relay that appended
    // the credential without removing what the capsule wrote would send two,
    // and a recorder that kept only the last would report exactly what a
    // correct relay reports.
    let mut authorizations: Vec<String> = Vec::new();
    let mut first = String::new();
    reader.read_line(&mut first).ok();
    // A connection with no request line on it is not a request. The daemon
    // opens its connection to the site before it has read the capsule's head —
    // the same order a `CONNECT` tunnel is established in — so a refused head
    // leaves a socket that was accepted and never spoken on. Recording it
    // would make "nothing was sent" and "something was sent" the same
    // observation, which is the whole of what these tests measure.
    if first.trim().is_empty() {
        return;
    }
    requests
        .lock()
        .expect("lock")
        .push(first.trim_end().to_string());
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            break;
        }
        if line == "\r\n" || line == "\n" {
            break;
        }
        if line.to_ascii_lowercase().starts_with("authorization:") {
            authorizations.push(
                line.split_once(':')
                    .map(|(_, v)| v.trim().to_string())
                    .unwrap_or_default(),
            );
        }
    }
    seen.lock().expect("lock").extend(authorizations.iter().cloned());
    let body = format!("you sent [{}]\n", authorizations.join(" AND "));
    let head = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body.as_bytes());
    let _ = stream.flush();
    let _ = stream.shutdown(std::net::Shutdown::Write);
}

// ── the daemon under test ───────────────────────────────────────────────────

struct Daemon {
    child: Child,
    socket: PathBuf,
    dir: PathBuf,
}

impl Daemon {
    fn start(tag: &str) -> Daemon {
        let dir = std::env::temp_dir().join(format!(
            "apex-present-{}-{tag}-{}",
            std::process::id(),
            now_ms()
        ));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).expect("temp dir");
        let socket = dir.join("control.sock");
        let store = dir.join("store");
        let child = Command::new(env!("CARGO_BIN_EXE_apex-secretd"))
            .arg("--socket")
            .arg(&socket)
            .arg("--store")
            .arg(&store)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("start apex-secretd");
        let daemon = Daemon {
            child,
            socket,
            dir,
        };
        for _ in 0..200 {
            if Client::connect_at(&daemon.socket).is_ok() {
                return daemon;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        panic!("apex-secretd did not come up on {}", daemon.socket.display());
    }

    fn client(&self) -> Client {
        Client::connect_at(&self.socket).expect("connect")
    }

    fn trail(&self) -> String {
        let mut found = String::new();
        walk(&self.dir, &mut |p| {
            if p.file_name().is_some_and(|n| n == "audit.jsonl") {
                found.push_str(&std::fs::read_to_string(p).unwrap_or_default());
            }
        });
        found
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        // Only ever this test's own child.
        let _ = self.child.kill();
        let _ = self.child.wait();
        std::fs::remove_dir_all(&self.dir).ok();
    }
}

fn walk(dir: &Path, f: &mut impl FnMut(&Path)) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, f);
        } else {
            f(&path);
        }
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Store the sentinel for the site's port, and optionally grant the capsule
/// capability for `project`.
fn arrange(daemon: &Daemon, site: &Site, project: &Path, grant: Option<&str>) {
    let mut client = daemon.client();
    client
        .add(
            "intranet",
            "127.0.0.1",
            "http",
            Some("x-access-token"),
            "",
            "bearer",
            Some(site.port),
            &SecretValue::new(SENTINEL.into()),
        )
        .expect("add");
    if let Some(capability) = grant {
        client
            .call(&Request::Grant {
                project: project.to_string_lossy().into_owned(),
                service: "intranet".into(),
                capability: capability.into(),
                revoke: false,
            })
            .expect("grant");
    }
}

fn record(project: &Path) -> CapabilityRecord {
    let mut rec = CapabilityRecord::new("intranet", BROWSER_PRESENT, "");
    rec.project = Some(project.to_string_lossy().into_owned());
    rec
}

/// Play the runtime's part: open the relay, write one request, read the answer.
fn through(daemon: &Daemon, rec: CapabilityRecord, destination: &str, request: &str) -> Vec<u8> {
    let stream: UnixStream = daemon
        .client()
        .present(rec, destination)
        .expect("the relay opened");
    let mut writer = stream.try_clone().expect("clone");
    writer.write_all(request.as_bytes()).expect("write request");
    writer.flush().ok();
    let mut answer = Vec::new();
    let mut reader = stream;
    reader.read_to_end(&mut answer).expect("read the answer");
    answer
}

/// The one audit line with this event, as JSON.
///
/// Exactly one: a trail with two `used` lines in it would mean the relay ran
/// twice, and picking the first would hide that.
fn one_line(trail: &str, event: &str) -> serde_json::Value {
    let mut found: Vec<serde_json::Value> = Vec::new();
    for line in trail.lines().filter(|l| !l.trim().is_empty()) {
        let parsed: serde_json::Value =
            serde_json::from_str(line).unwrap_or_else(|e| panic!("audit line {line}: {e}"));
        if parsed["event"] == event {
            found.push(parsed);
        }
    }
    assert_eq!(
        found.len(),
        1,
        "expected exactly one '{event}' line, found {} in:\n{trail}",
        found.len()
    );
    found.remove(0)
}

fn get(host_port: &str) -> String {
    format!("GET /page HTTP/1.1\r\nHost: {host_port}\r\nUser-Agent: capsule\r\n\r\n")
}

// ── the tests ───────────────────────────────────────────────────────────────

#[test]
fn the_site_gets_the_credential_and_the_capsule_gets_it_back_redacted() {
    let site = Site::start();
    let daemon = Daemon::start("carried");
    let project = daemon.dir.join("capsule");
    std::fs::create_dir_all(&project).expect("project");
    arrange(&daemon, &site, &project, Some(BROWSER_PRESENT));

    let destination = format!("127.0.0.1:{}", site.port);
    let answer = through(&daemon, record(&project), &destination, &get(&destination));
    let text = String::from_utf8_lossy(&answer).into_owned();

    // The site was reached, with the credential, by a request that carried it
    // and nothing the caller wrote.
    assert_eq!(
        site.authorizations(),
        vec![format!("Bearer {SENTINEL}")],
        "the credential did not reach the site, so nothing about this run is \
         about authentication"
    );
    assert_eq!(site.request_lines(), vec!["GET /page HTTP/1.1".to_string()]);

    // And the caller — which is the runtime, and beyond it the capsule — never
    // saw it, even though the site sent it straight back.
    assert!(text.starts_with("HTTP/1.1 200 OK"), "{text}");
    assert!(
        !text.contains(SENTINEL),
        "the credential came back to the caller: {text}"
    );
    // The redaction is the same length as what it replaced, or the browser
    // would be waiting on a `Content-Length` that no longer matches.
    assert!(
        text.contains(&format!("you sent [Bearer {}]", "x".repeat(SENTINEL.len()))),
        "the echo was not redacted in place: {text}"
    );

    // The trail says it happened, names where the credential went, and does
    // not contain it.
    //
    // Read as JSON and looked at line by line, not grepped. The trail also
    // holds the `added` and `granted` lines this test wrote, and both name the
    // same host — so `trail.contains("http://127.0.0.1")` passes with the
    // `used` line carrying no endpoint at all, which is exactly the assertion
    // that inspects nothing.
    let trail = daemon.trail();
    assert!(!trail.contains(SENTINEL), "the trail holds the credential: {trail}");
    let used = one_line(&trail, "used");
    assert_eq!(used["operation"], BROWSER_PRESENT);
    assert_eq!(used["provider"], "intranet");
    assert_eq!(
        used["endpoint"], "http://127.0.0.1",
        "the used line does not say where the credential went: {used}"
    );
    // The echo is worth knowing about, so the line says so.
    assert!(
        used["detail"].as_str().unwrap_or_default().contains("ECHOED BACK AND REDACTED"),
        "{used}"
    );
}

#[test]
fn a_capsule_without_a_grant_never_reaches_the_site() {
    let site = Site::start();
    let daemon = Daemon::start("nogrant");
    let project = daemon.dir.join("capsule");
    std::fs::create_dir_all(&project).expect("project");
    // Stored, not granted. A credential grants nothing by itself, and a relay
    // that skipped the grant would be a way for anything running as this
    // account to spend it.
    arrange(&daemon, &site, &project, None);

    let destination = format!("127.0.0.1:{}", site.port);
    let e = daemon
        .client()
        .present(record(&project), &destination)
        .expect_err("refused");
    assert!(e.to_string().contains("not granted"), "{e}");
    // The half that makes the refusal mean something: nothing was contacted.
    assert!(
        site.authorizations().is_empty(),
        "the site was reached by a request that was refused"
    );
}

#[test]
fn the_grant_may_be_held_everywhere_because_a_capsule_has_no_project() {
    // The shipped shape. `apex browser run` starts its session with `--cwd`
    // pointing at a throwaway capsule tree, so the grant an owner writes is
    // `apex secret grant intranet browser.present --everywhere`.
    let site = Site::start();
    let daemon = Daemon::start("everywhere");
    let project = daemon.dir.join("capsule");
    std::fs::create_dir_all(&project).expect("project");
    arrange(&daemon, &site, &project, None);
    daemon
        .client()
        .call(&Request::Grant {
            project: apex_secret_core::store::ANY_PROJECT.to_string(),
            service: "intranet".into(),
            capability: BROWSER_PRESENT.into(),
            revoke: false,
        })
        .expect("grant everywhere");

    // A directory the grant never named, which is what every capsule is.
    let elsewhere = daemon.dir.join("some-other-capsule");
    std::fs::create_dir_all(&elsewhere).expect("dir");
    let destination = format!("127.0.0.1:{}", site.port);
    let answer = through(&daemon, record(&elsewhere), &destination, &get(&destination));
    assert!(
        String::from_utf8_lossy(&answer).starts_with("HTTP/1.1 200 OK"),
        "{}",
        String::from_utf8_lossy(&answer)
    );
    assert_eq!(site.authorizations(), vec![format!("Bearer {SENTINEL}")]);
}

#[test]
fn a_destination_that_is_not_the_pin_is_refused_by_this_daemon_and_not_only_by_the_runtime() {
    let site = Site::start();
    let daemon = Daemon::start("wrongdest");
    let project = daemon.dir.join("capsule");
    std::fs::create_dir_all(&project).expect("project");
    arrange(&daemon, &site, &project, Some(BROWSER_PRESENT));

    // The runtime checks this too, and the runtime runs as the user. A caller
    // that skipped the runtime entirely — which any process of this account
    // can do, because the socket is reachable — must still be refused.
    let e = daemon
        .client()
        .present(record(&project), &format!("127.0.0.1:{}", site.port + 1))
        .expect_err("refused");
    assert!(e.to_string().contains("is pinned to"), "{e}");
    assert!(site.authorizations().is_empty());
}

#[test]
fn only_the_capsule_operation_can_be_spent_this_way() {
    let site = Site::start();
    let daemon = Daemon::start("wrongop");
    let project = daemon.dir.join("capsule");
    std::fs::create_dir_all(&project).expect("project");
    arrange(&daemon, &site, &project, Some("git.push"));

    // Granted — for something else. Without the check this verb would relay a
    // connection on the authority of a grant the owner wrote for pushing.
    let mut rec = record(&project);
    rec.operation = "git.push".into();
    let e = daemon
        .client()
        .present(rec, &format!("127.0.0.1:{}", site.port))
        .expect_err("refused");
    assert!(e.to_string().contains(BROWSER_PRESENT), "{e}");
    assert!(site.authorizations().is_empty());
}

#[test]
fn a_capsules_own_authorization_header_is_replaced_and_not_appended() {
    let site = Site::start();
    let daemon = Daemon::start("ownheader");
    let project = daemon.dir.join("capsule");
    std::fs::create_dir_all(&project).expect("project");
    arrange(&daemon, &site, &project, Some(BROWSER_PRESENT));

    let destination = format!("127.0.0.1:{}", site.port);
    let request = format!(
        "GET /page HTTP/1.1\r\nHost: {destination}\r\nAuthorization: Bearer capsule-chose-this\r\n\r\n"
    );
    let _ = through(&daemon, record(&project), &destination, &request);
    assert_eq!(
        site.authorizations(),
        vec![format!("Bearer {SENTINEL}")],
        "what the capsule wrote under that name reached the site"
    );
}

#[test]
fn a_request_for_another_host_is_refused_after_the_relay_has_opened() {
    // The `Host` header is how a virtual host is chosen, so a request that
    // names a different one is an attempt to spend this credential somewhere
    // else on the same address. The relay is already open by then — the
    // refusal has to close it rather than send the request on.
    let site = Site::start();
    let daemon = Daemon::start("wronghost");
    let project = daemon.dir.join("capsule");
    std::fs::create_dir_all(&project).expect("project");
    arrange(&daemon, &site, &project, Some(BROWSER_PRESENT));

    let destination = format!("127.0.0.1:{}", site.port);
    let request = "GET /page HTTP/1.1\r\nHost: elsewhere.example\r\n\r\n";
    let answer = through(&daemon, record(&project), &destination, request);
    assert!(
        answer.is_empty(),
        "the capsule got {} bytes back from a request that should not have been sent",
        answer.len()
    );
    assert!(
        site.request_lines().is_empty() && site.authorizations().is_empty(),
        "a request naming another host reached the site: {:?}",
        site.request_lines()
    );
    let trail = daemon.trail();
    let refused = one_line(&trail, "refused");
    assert_eq!(refused["operation"], BROWSER_PRESENT);
    assert!(
        refused["reason"].as_str().unwrap_or_default().contains("pinned to"),
        "the refusal does not say why: {refused}"
    );
    assert!(!trail.contains(SENTINEL), "{trail}");
}

#[test]
fn the_reply_to_a_present_is_an_ok_and_carries_nothing() {
    // The wire shape, asserted rather than assumed: everything after this
    // reply is the capsule's own bytes, so a reply that carried a field would
    // be a field somebody could put a credential in later.
    let site = Site::start();
    let daemon = Daemon::start("shape");
    let project = daemon.dir.join("capsule");
    std::fs::create_dir_all(&project).expect("project");
    arrange(&daemon, &site, &project, Some(BROWSER_PRESENT));

    let mut raw = UnixStream::connect(&daemon.socket).expect("connect");
    let line = serde_json::to_string(&Request::Present {
        record: Box::new(record(&project)),
        destination: format!("127.0.0.1:{}", site.port),
    })
    .expect("serialise");
    raw.write_all(line.as_bytes()).expect("write");
    raw.write_all(b"\n").expect("newline");
    let mut reader = BufReader::new(raw.try_clone().expect("clone"));
    let mut reply = String::new();
    reader.read_line(&mut reply).expect("reply");
    let parsed: Response = serde_json::from_str(reply.trim_end()).expect("parse");
    assert!(matches!(parsed, Response::Ok), "{reply}");
    assert_eq!(reply.trim_end(), r#"{"reply":"ok"}"#);
}
