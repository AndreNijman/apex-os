//! The whole service, end to end, against a real credential-checking server.
//!
//! Everything here is the shipped path: the real `apex-secretd` binary, a real
//! Unix socket with real `SO_PEERCRED`, the real client, a real git repository
//! and a real `git` process being handed a credential. What is faked is only
//! the far side — a git smart-HTTP server on loopback that refuses without an
//! `Authorization` header and serves an empty ref advertisement with one.
//!
//! That fake is what makes P0-002's third criterion — "the broker can execute
//! credential-backed operations" — a demonstration rather than an assertion.
//! The server records the header it received, so the test can say two things at
//! once:
//!
//! * the credential **did** reach the provider, so the operation was genuinely
//!   credential-backed and not a no-op that happened to exit zero;
//! * the credential **did not** reach the caller, in any reply, in the output,
//!   or in the audit trail.
//!
//! ## No network, and no certificate
//!
//! The server binds `127.0.0.1:0`. Reaching it needs `http`, which the store
//! allows only for a loopback host — the credential does not cross a network,
//! and the alternative is that the credential path cannot be exercised without
//! either a real provider or a certificate authority in the fixture. It is not
//! a hole an agent can open: only the owner adds a service record, `add`
//! refuses `http` for any other host, and the host is pinned from then on.
//!
//! ## No root
//!
//! The daemon is started with `--store` and `--socket` and runs as whoever runs
//! the test, so the setuid step is a no-op (the target uid is already the
//! current one) and the daemon reports `protected: false`. What that costs the
//! test is the *at-rest* half of the boundary, which needs a uid this test does
//! not have; the API half — no reply carries a credential — is exactly the same
//! code either way.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::{Arc, Mutex};

use apex_secret_core::capability::CapabilityRecord;
use apex_secret_core::client::Client;
use apex_secret_core::protocol::{Request, Response};
use apex_secret_core::SecretValue;

/// Distinctive enough that a grep for it cannot match by accident.
const SENTINEL: &str = "apex-sentinel-7f3a91c4-do-not-leak";

// ── the fake provider ───────────────────────────────────────────────────────

/// A git smart-HTTP server that demands Basic auth.
///
/// One request matters: `GET /<repo>/info/refs?service=git-upload-pack`. Without
/// an `Authorization` header it answers `401` with a `WWW-Authenticate`
/// challenge, which is what makes git ask its credential helper. With one it
/// answers a well-formed v0 advertisement for a repository with no refs, which
/// `git ls-remote` and `git fetch` both accept and exit zero on.
struct FakeGit {
    port: u16,
    seen: Arc<Mutex<Vec<String>>>,
}

impl FakeGit {
    fn start() -> FakeGit {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let port = listener.local_addr().expect("addr").port();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let recorder = Arc::clone(&seen);

        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let recorder = Arc::clone(&recorder);
                std::thread::spawn(move || serve_one(stream, &recorder));
            }
        });
        FakeGit { port, seen }
    }

    /// Authority, with the port, for a URL.
    fn authority(&self) -> String {
        format!("127.0.0.1:{}", self.port)
    }

    /// What the credential is pinned to.
    ///
    /// No port: a credential is stored for a HOST, and `check_url` compares
    /// hosts. A fixture on a random port would otherwise be unpinnable, and a
    /// real credential for `github.com` should not stop working because a
    /// remote spelled out `:443`.
    fn pin_host(&self) -> &'static str {
        "127.0.0.1"
    }

    fn url(&self) -> String {
        format!("http://{}/demo.git", self.authority())
    }

    /// Every `Authorization` header the server was sent.
    fn authorizations(&self) -> Vec<String> {
        self.seen.lock().expect("lock").clone()
    }
}

fn serve_one(mut stream: TcpStream, recorder: &Arc<Mutex<Vec<String>>>) {
    let mut reader = BufReader::new(stream.try_clone().expect("clone"));
    let mut authorization = None;
    let mut first = String::new();
    if reader.read_line(&mut first).is_err() {
        return;
    }
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {}
            Err(_) => return,
        }
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            break;
        }
        if let Some(value) = trimmed
            .strip_prefix("Authorization: ")
            .or_else(|| trimmed.strip_prefix("authorization: "))
        {
            authorization = Some(value.to_string());
        }
    }

    let Some(authorization) = authorization else {
        let _ = stream.write_all(
            b"HTTP/1.1 401 Unauthorized\r\n\
              WWW-Authenticate: Basic realm=\"apex-test\"\r\n\
              Content-Length: 0\r\n\
              Connection: close\r\n\r\n",
        );
        return;
    };
    recorder.lock().expect("lock").push(authorization);

    // A v0 upload-pack advertisement for a repository with no refs. git's own
    // rule: when there is nothing to advertise, send one dummy ref at the null
    // object called `capabilities^{}`.
    let refs_line = "0000000000000000000000000000000000000000 capabilities^{}\0agent=apex-test\n";
    let mut body = Vec::new();
    body.extend_from_slice(pkt("# service=git-upload-pack\n").as_bytes());
    body.extend_from_slice(b"0000");
    body.extend_from_slice(pkt(refs_line).as_bytes());
    body.extend_from_slice(b"0000");

    let header = format!(
        "HTTP/1.1 200 OK\r\n\
         Content-Type: application/x-git-upload-pack-advertisement\r\n\
         Cache-Control: no-cache\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(header.as_bytes());
    let _ = stream.write_all(&body);
}

/// git's pkt-line framing: four hex digits of total length, then the payload.
fn pkt(payload: &str) -> String {
    format!("{:04x}{payload}", payload.len() + 4)
}

// ── the daemon under test ───────────────────────────────────────────────────

struct Daemon {
    child: Child,
    socket: PathBuf,
    store: PathBuf,
    dir: PathBuf,
}

impl Daemon {
    /// Start the real binary on a private socket and store.
    ///
    /// Under `std::env::temp_dir()` and not under the worktree: a Unix socket
    /// path is capped at 108 bytes, and a checkout path plus a cargo target
    /// directory is well past that.
    fn start(tag: &str) -> Daemon {
        let dir = std::env::temp_dir().join(format!(
            "apex-e2e-{}-{tag}-{}",
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

        // Wrapped before it is waited on, not after: if the daemon never comes
        // up this function panics, and a child that is still a local at that
        // point is leaked. Owned by the struct, `Drop` kills and reaps it on
        // every path out of here.
        let daemon = Daemon {
            child,
            socket,
            store,
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
}

impl Drop for Daemon {
    fn drop(&mut self) {
        // Only ever this test's own child.
        let _ = self.child.kill();
        let _ = self.child.wait();
        std::fs::remove_dir_all(&self.dir).ok();
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// A git repository whose `origin` points at the fake provider.
fn fixture_repo(dir: &Path, origin: &str) -> PathBuf {
    let repo = dir.join("demo");
    std::fs::create_dir_all(&repo).expect("repo dir");
    let git = |args: &[&str]| {
        let out = Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(args)
            .output()
            .expect("git");
        assert!(out.status.success(), "git {args:?}: {out:?}");
    };
    git(&["init", "-q"]);
    git(&["config", "user.email", "t@t"]);
    git(&["config", "user.name", "t"]);
    git(&["commit", "-q", "--allow-empty", "-m", "init"]);
    git(&["remote", "add", "origin", origin]);
    git(&["remote", "add", "elsewhere", "https://example.invalid/other.git"]);
    git(&["remote", "add", "viassh", "git@127.0.0.1:demo.git"]);
    repo
}

fn record(provider: &str, operation: &str, remote: &str, project: &Path) -> CapabilityRecord {
    let mut rec = CapabilityRecord::new(provider, operation, remote);
    rec.project = Some(project.to_string_lossy().into_owned());
    rec
}

/// Store the sentinel and allow one capability for `project`.
fn arrange(daemon: &Daemon, host: &str, project: &Path, capability: &str) {
    let mut client = daemon.client();
    client
        .add(
            "demo",
            host,
            "http",
            Some("x-access-token"),
            "",
            "bearer",
            None,
            &SecretValue::new(SENTINEL.into()),
        )
        .expect("add");
    client
        .call(&Request::Grant {
            project: project.to_string_lossy().into_owned(),
            service: "demo".into(),
            capability: capability.into(),
            revoke: false,
        })
        .expect("grant");
}

// ── the tests ───────────────────────────────────────────────────────────────

#[test]
fn a_credential_backed_operation_runs_and_the_credential_never_comes_back() {
    // P0-002's criteria 2, 3 and 4 in one place: the operation runs, the
    // provider is satisfied that it was authenticated, the caller gets the
    // result and not the credential, and the trail records it.
    let daemon = Daemon::start("perform");
    let provider = FakeGit::start();
    let repo = fixture_repo(&daemon.dir, &provider.url());
    arrange(&daemon, provider.pin_host(), &repo, "git.ls-remote");

    let reply = daemon
        .client()
        .request(&Request::Use {
            body_len: 0,
            record: Box::new(record("demo", "git.ls-remote", "origin", &repo)),
        })
        .expect("use");

    let Response::Performed {
        record,
        endpoint,
        exit_code,
        output,
    } = &reply
    else {
        panic!("expected the operation to be performed, got {reply:?}");
    };
    assert_eq!(*exit_code, 0, "git failed: {output}");
    assert_eq!(*endpoint, format!("http://{}", "127.0.0.1"));
    assert!(!record.audit_id.is_empty(), "the reply cites no audit entry");

    // The provider was actually asked, and asked WITH the credential — this is
    // what makes the operation credential-backed rather than an exit-zero
    // no-op.
    let seen = provider.authorizations();
    assert_eq!(seen.len(), 1, "the provider saw {} authorised requests", seen.len());
    let expected = base64(format!("x-access-token:{SENTINEL}").as_bytes());
    assert_eq!(seen[0], format!("Basic {expected}"), "wrong credential sent");

    // ...and the caller got none of it.
    let wire = serde_json::to_string(&reply).expect("serialise");
    assert!(!wire.contains(SENTINEL), "the reply carried it: {wire}");
    assert!(!wire.contains(&expected), "the reply carried it encoded: {wire}");

    // The trail records the use, names where it went, and holds no credential.
    let entries = match daemon
        .client()
        .call(&Request::Audit { lines: 50 })
        .expect("audit")
    {
        Response::Audit { entries } => entries,
        other => panic!("{other:?}"),
    };
    let used = entries
        .iter()
        .find(|e| e.event == apex_secret_core::AuditEvent::Used)
        .expect("no `used` entry in the trail");
    assert_eq!(used.audit_id, record.audit_id);
    assert_eq!(used.operation, "git.ls-remote");
    assert_eq!(used.resource, "origin");
    assert_eq!(used.exit_code, Some(0));
    assert_eq!(used.endpoint.as_deref(), Some("http://127.0.0.1"));
    assert_eq!(used.project.as_deref(), Some(repo.to_string_lossy().as_ref()));
    let trail = serde_json::to_string(&entries).expect("serialise");
    assert!(!trail.contains(SENTINEL), "the trail carried it: {trail}");
}

#[test]
fn a_fetch_is_brokered_the_same_way_a_read_only_capability_is() {
    // git.ls-remote is the one the fixture proves most cheaply, but the
    // capability that people actually reach for must go down the same path.
    let daemon = Daemon::start("fetch");
    let provider = FakeGit::start();
    let repo = fixture_repo(&daemon.dir, &provider.url());
    arrange(&daemon, provider.pin_host(), &repo, "git.fetch");

    let reply = daemon
        .client()
        .request(&Request::Use {
            body_len: 0,
            record: Box::new(record("demo", "git.fetch", "origin", &repo)),
        })
        .expect("use");
    let Response::Performed {
        exit_code, output, ..
    } = &reply
    else {
        panic!("{reply:?}");
    };
    assert_eq!(*exit_code, 0, "git fetch failed: {output}");
    assert!(!provider.authorizations().is_empty(), "nothing was contacted");
    assert!(!output.contains(SENTINEL), "{output}");
}

#[test]
fn nothing_the_socket_can_answer_contains_the_credential() {
    // Criterion 2, exercised across every verb on a live daemon rather than
    // against hand-built values. If a future verb returns one, this fails.
    let daemon = Daemon::start("noleak");
    let provider = FakeGit::start();
    let repo = fixture_repo(&daemon.dir, &provider.url());
    arrange(&daemon, provider.pin_host(), &repo, "git.ls-remote");

    let mut client = daemon.client();
    let mut replies = Vec::new();
    for request in [
        Request::Hello,
        Request::List,
        Request::Grants,
        Request::Audit { lines: 100 },
        // A use that succeeds, and three that are refused at different steps.
        Request::Use {
            body_len: 0,
            record: Box::new(record("demo", "git.ls-remote", "origin", &repo)),
        },
        Request::Use {
            body_len: 0,
            record: Box::new(record("demo", "git.push", "origin", &repo)),
        },
        Request::Use {
            body_len: 0,
            record: Box::new(record("demo", "git.ls-remote", "elsewhere", &repo)),
        },
        Request::Use {
            body_len: 0,
            record: Box::new(record("nosuch", "git.ls-remote", "origin", &repo)),
        },
        Request::Grant {
            project: repo.to_string_lossy().into_owned(),
            service: "demo".into(),
            capability: "git.fetch".into(),
            revoke: false,
        },
        Request::Remove {
            service: "demo".into(),
        },
    ] {
        replies.push(client.request(&request).expect("request"));
    }
    for reply in &replies {
        let wire = serde_json::to_string(reply).expect("serialise");
        assert!(
            !wire.contains(SENTINEL),
            "{} carried the credential: {wire}",
            reply.variant()
        );
    }
    // Exactly one of them performed something, so this is not passing because
    // every request was refused.
    assert!(
        replies
            .iter()
            .any(|r| matches!(r, Response::Performed { exit_code: 0, .. })),
        "no request got as far as performing an operation"
    );
}

#[test]
fn the_store_is_the_only_place_the_credential_exists() {
    // Criterion 1's shape, as far as a test without root can assert it: the
    // credential is in the daemon's own store directory and in nothing else
    // the run produced — not the repository, not a temporary file, not the
    // trail.
    let daemon = Daemon::start("store");
    let provider = FakeGit::start();
    let repo = fixture_repo(&daemon.dir, &provider.url());
    arrange(&daemon, provider.pin_host(), &repo, "git.ls-remote");
    daemon
        .client()
        .request(&Request::Use {
            body_len: 0,
            record: Box::new(record("demo", "git.ls-remote", "origin", &repo)),
        })
        .expect("use");

    let mut holders = Vec::new();
    walk(&daemon.dir, &mut holders);
    let holders: Vec<PathBuf> = holders
        .into_iter()
        .filter(|p| {
            std::fs::read(p)
                .map(|b| String::from_utf8_lossy(&b).contains(SENTINEL))
                .unwrap_or(false)
        })
        .collect();
    assert_eq!(
        holders.len(),
        1,
        "the credential is in {holders:?}, and it should be in the store alone"
    );
    assert!(
        holders[0].starts_with(&daemon.store),
        "{} is outside the store",
        holders[0].display()
    );
    assert_eq!(
        holders[0].extension().and_then(|s| s.to_str()),
        Some("secret"),
        "the credential is in a metadata file, which every listing reads"
    );

    // The mode, which is what a `0600` claim means on disk.
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(&holders[0]).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600, "mode is {:o}", mode & 0o777);
}

#[test]
fn a_grant_written_under_the_old_spelling_still_matches() {
    // P0-002 shipped `git-ls-remote`; P1-001 renamed it `git.ls-remote` so the
    // vocabulary reads the way §13.2 asks. A machine that already granted the
    // old name must not be told the capability it granted is not granted — and
    // the trail must say one name for one operation, whichever was typed.
    let daemon = Daemon::start("alias");
    let provider = FakeGit::start();
    let repo = fixture_repo(&daemon.dir, &provider.url());
    arrange(&daemon, provider.pin_host(), &repo, "git-ls-remote");

    // What the grant table holds is the canonical id, not what was typed.
    let Response::Grants { projects } = daemon.client().call(&Request::Grants).expect("grants")
    else {
        panic!("expected grants");
    };
    let keys = projects
        .get(&repo.to_string_lossy().into_owned())
        .expect("a grant for the fixture repository");
    assert_eq!(keys, &vec!["demo:git.ls-remote".to_string()]);

    // And both spellings reach it.
    for spelling in ["git-ls-remote", "git.ls-remote"] {
        let reply = daemon
            .client()
            .request(&Request::Use {
                body_len: 0,
                record: Box::new(record("demo", spelling, "origin", &repo)),
            })
            .expect("use");
        let Response::Performed { record, .. } = &reply else {
            panic!("'{spelling}' was refused: {reply:?}");
        };
        assert_eq!(record.operation, "git.ls-remote", "{spelling} was not canonicalised");
    }
    assert_eq!(provider.authorizations().len(), 2, "both must have run");
}

#[test]
fn a_remote_pointing_elsewhere_is_refused_before_anything_is_contacted() {
    // The hole this closes: a grant for one host turned into a request to
    // another, with the credential attached.
    let daemon = Daemon::start("hostpin");
    let provider = FakeGit::start();
    let repo = fixture_repo(&daemon.dir, &provider.url());
    arrange(&daemon, provider.pin_host(), &repo, "git.ls-remote");

    for (remote, expected) in [
        ("elsewhere", "example.invalid"),
        ("viassh", "not an http remote"),
        ("nosuchremote", "no remote called"),
    ] {
        let reply = daemon
            .client()
            .request(&Request::Use {
                body_len: 0,
                record: Box::new(record("demo", "git.ls-remote", remote, &repo)),
            })
            .expect("use");
        let (_, message) = reply
            .as_error()
            .unwrap_or_else(|| panic!("'{remote}' was not refused: {reply:?}"));
        assert!(message.contains(expected), "{remote}: {message}");
        assert!(!message.contains(SENTINEL), "{message}");
    }
    assert!(
        provider.authorizations().is_empty(),
        "a refused request still reached the provider"
    );
}

#[test]
fn a_capability_that_was_not_granted_is_refused_and_recorded() {
    let daemon = Daemon::start("ungranted");
    let provider = FakeGit::start();
    let repo = fixture_repo(&daemon.dir, &provider.url());
    arrange(&daemon, provider.pin_host(), &repo, "git.ls-remote");

    // Granted for ls-remote only.
    let reply = daemon
        .client()
        .request(&Request::Use {
            body_len: 0,
            record: Box::new(record("demo", "git.push", "origin", &repo)),
        })
        .expect("use");
    assert!(reply
        .as_error()
        .is_some_and(|(_, m)| m.contains("not granted")));
    assert!(provider.authorizations().is_empty());

    let entries = match daemon
        .client()
        .call(&Request::Audit { lines: 50 })
        .expect("audit")
    {
        Response::Audit { entries } => entries,
        other => panic!("{other:?}"),
    };
    assert!(
        entries
            .iter()
            .any(|e| e.event == apex_secret_core::AuditEvent::Refused
                && e.operation == "git.push"),
        "the refusal is not in the trail: {entries:#?}"
    );
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_symlink() {
            continue;
        }
        if path.is_dir() {
            walk(&path, out);
        } else {
            out.push(path);
        }
    }
}

/// Standard base64, for checking the header git sent.
///
/// Written out rather than pulled in: this crate has no base64 dependency and
/// adding one to a workspace that has none, to decode one test header, is a
/// worse trade than eighteen lines.
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[((n >> (18 - 6 * i)) & 0x3f) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[test]
fn the_base64_helper_agrees_with_a_known_vector() {
    // The header assertion above is only worth anything if this is right.
    assert_eq!(base64(b"a"), "YQ==");
    assert_eq!(base64(b"ab"), "YWI=");
    assert_eq!(base64(b"abc"), "YWJj");
    assert_eq!(base64(b"user:pass"), "dXNlcjpwYXNz");
}

#[test]
fn the_fixture_server_answers_the_way_git_expects() {
    // If the fake ever stops speaking the protocol, every test above would
    // fail for a reason that has nothing to do with the service. This one says
    // which.
    let provider = FakeGit::start();
    let url = format!(
        "http://{}/demo.git/info/refs?service=git-upload-pack",
        provider.authority()
    );

    let unauthenticated = http_get(&url, None);
    assert!(unauthenticated.starts_with("HTTP/1.1 401"), "{unauthenticated}");
    assert!(unauthenticated.contains("WWW-Authenticate: Basic"), "{unauthenticated}");

    let authenticated = http_get(&url, Some("Basic dXNlcjpwYXNz"));
    assert!(authenticated.starts_with("HTTP/1.1 200"), "{authenticated}");
    assert!(authenticated.contains("001e# service=git-upload-pack"), "{authenticated}");
    assert!(authenticated.contains("capabilities^{}"), "{authenticated}");
    assert_eq!(provider.authorizations(), vec!["Basic dXNlcjpwYXNz".to_string()]);
}

fn http_get(url: &str, authorization: Option<&str>) -> String {
    let rest = url.strip_prefix("http://").expect("http url");
    let (authority, path) = rest.split_once('/').expect("path");
    let mut stream = TcpStream::connect(authority).expect("connect");
    let mut request = format!("GET /{path} HTTP/1.1\r\nHost: {authority}\r\n");
    if let Some(value) = authorization {
        request.push_str(&format!("Authorization: {value}\r\n"));
    }
    request.push_str("Connection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).expect("write");
    let mut out = Vec::new();
    stream.read_to_end(&mut out).expect("read");
    String::from_utf8_lossy(&out).into_owned()
}

// ── the MCP half (P0-003) ───────────────────────────────────────────────────
//
// The same shape as the git half and for the same reason: a loopback server
// that refuses without an `Authorization` header, so "the credential reached
// the provider" is something the test observes rather than infers. What differs
// is that the destination is not resolved from a repository — it is the stored
// record itself, because `mcp-request` has no arguments a caller could put a
// URL in.

/// An MCP server that demands a bearer token.
///
/// Answers `401` without one and a JSON-RPC result with one, recording every
/// `Authorization` header and every body it was posted.
struct FakeMcp {
    port: u16,
    seen: Arc<Mutex<Vec<String>>>,
    bodies: Arc<Mutex<Vec<String>>>,
    /// Whether to answer in `text/event-stream` framing rather than plain JSON.
    /// Both are legal for the same server on the same endpoint.
    sse: bool,
}

impl FakeMcp {
    fn start(sse: bool) -> FakeMcp {
        FakeMcp::start_accepting_only(sse, None)
    }

    /// A server that accepts one credential and 401s every other.
    ///
    /// `start` accepts any header, which is enough to show a credential
    /// arriving. It cannot show what a *wrong* one produces, and the answer to
    /// that is the reason `apex mcp connect` reads the reply rather than the
    /// exit code: a refusal is a successful HTTP request.
    fn start_accepting_only(sse: bool, expect: Option<&str>) -> FakeMcp {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let port = listener.local_addr().expect("addr").port();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let bodies = Arc::new(Mutex::new(Vec::new()));
        let (h, b) = (Arc::clone(&seen), Arc::clone(&bodies));
        let expect = expect.map(str::to_string);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let (h, b, e) = (Arc::clone(&h), Arc::clone(&b), expect.clone());
                std::thread::spawn(move || serve_mcp(stream, &h, &b, sse, e.as_deref()));
            }
        });
        FakeMcp {
            port,
            seen,
            bodies,
            sse,
        }
    }

    fn authorizations(&self) -> Vec<String> {
        self.seen.lock().expect("lock").clone()
    }

    fn bodies(&self) -> Vec<String> {
        self.bodies.lock().expect("lock").clone()
    }
}

fn serve_mcp(
    mut stream: TcpStream,
    header_log: &Arc<Mutex<Vec<String>>>,
    body_log: &Arc<Mutex<Vec<String>>>,
    sse: bool,
    expect: Option<&str>,
) {
    let mut reader = BufReader::new(stream.try_clone().expect("clone"));
    let mut first = String::new();
    if reader.read_line(&mut first).is_err() {
        return;
    }
    let mut authorization = None;
    let mut length = 0usize;
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {}
            Err(_) => return,
        }
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            break;
        }
        let Some((name, value)) = trimmed.split_once(": ") else {
            continue;
        };
        if name.eq_ignore_ascii_case("authorization") {
            authorization = Some(value.to_string());
        }
        if name.eq_ignore_ascii_case("content-length") {
            length = value.parse().unwrap_or(0);
        }
    }
    let mut body = vec![0u8; length];
    if reader.read_exact(&mut body).is_err() {
        return;
    }
    body_log
        .lock()
        .expect("lock")
        .push(String::from_utf8_lossy(&body).into_owned());

    let Some(authorization) = authorization else {
        let _ = stream.write_all(
            b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        );
        return;
    };
    header_log.lock().expect("lock").push(authorization.clone());

    // A credential the server does not recognise. Answered the way a real one
    // answers: 200-shaped plumbing, a 401 status, and a JSON body explaining
    // itself — which is a *successful* request as far as curl is concerned.
    if expect.is_some_and(|want| want != authorization) {
        let payload = r#"{"error":"invalid_token","error_description":"the access token is invalid"}"#;
        let header = format!(
            "HTTP/1.1 401 Unauthorized\r\nContent-Type: application/json\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n",
            payload.len()
        );
        let _ = stream.write_all(header.as_bytes());
        let _ = stream.write_all(payload.as_bytes());
        return;
    }

    let payload = r#"{"jsonrpc":"2.0","id":1,"result":{"tools":["read_note"]}}"#;
    let (content_type, framed) = if sse {
        (
            "text/event-stream",
            format!("event: message\ndata: {payload}\n\n"),
        )
    } else {
        ("application/json", payload.to_string())
    };
    let header = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\n\
         Mcp-Session-Id: apex-test-session-1\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n",
        framed.len()
    );
    let _ = stream.write_all(header.as_bytes());
    let _ = stream.write_all(framed.as_bytes());
}

/// Store the sentinel as an MCP credential and grant `mcp-request`.
fn arrange_mcp(daemon: &Daemon, port: u16, project: &Path) {
    let mut client = daemon.client();
    client
        .add(
            "memory",
            "127.0.0.1",
            "http",
            Some("x-access-token"),
            "/mcp",
            "bearer",
            Some(port),
            &SecretValue::new(SENTINEL.into()),
        )
        .expect("add");
    client
        .call(&Request::Grant {
            project: project.to_string_lossy().into_owned(),
            service: "memory".into(),
            capability: "mcp-request".into(),
            revoke: false,
        })
        .expect("grant");
}

#[test]
fn an_mcp_message_is_carried_with_the_credential_and_the_credential_stays_here() {
    // P0-003's second and fourth criteria, in one run: the bearer token reaches
    // the MCP server and reaches nothing else.
    let provider = FakeMcp::start(false);
    let daemon = Daemon::start("mcp");
    let project = daemon.dir.join("proj");
    std::fs::create_dir_all(&project).expect("project dir");
    arrange_mcp(&daemon, provider.port, &project);

    let message = br#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#;
    let mut rec = CapabilityRecord::new("memory", "mcp.request", "");
    rec.project = Some(project.to_string_lossy().into_owned());
    let reply = daemon
        .client()
        .use_with_body(rec, message)
        .expect("use mcp-request");

    let (endpoint, exit_code, output) = match &reply {
        Response::Performed {
            endpoint,
            exit_code,
            output,
            ..
        } => (endpoint.clone(), *exit_code, output.clone()),
        other => panic!("expected a performed reply, got {other:?}"),
    };
    assert_eq!(exit_code, 0, "{output}");
    assert_eq!(endpoint, "http://127.0.0.1", "{endpoint}");

    // The server received the credential, as a bearer header the daemon built.
    assert_eq!(
        provider.authorizations(),
        vec![format!("Bearer {SENTINEL}")],
        "the credential did not reach the provider"
    );
    // ...and the message the caller wrote, unaltered.
    assert_eq!(
        provider.bodies(),
        vec![String::from_utf8_lossy(message).into_owned()]
    );
    // ...and the reply carries the server's answer and no credential.
    assert!(output.contains("read_note"), "{output}");
    assert!(!output.contains(SENTINEL), "{output}");
    let serialised = serde_json::to_string(&reply).expect("serialise");
    assert!(!serialised.contains(SENTINEL), "the reply carried it");

    // Nor does the trail.
    let trail = std::fs::read_to_string(daemon.store.join("audit.jsonl")).unwrap_or_default();
    assert!(!trail.is_empty(), "nothing was audited");
    assert!(trail.contains("mcp.request"), "{trail}");
    assert!(!trail.contains(SENTINEL), "the audit trail carried it");
}

#[test]
fn an_event_stream_answer_is_carried_back_as_its_message() {
    // The same endpoint may answer either way, per request. A bridge that only
    // understood one shape would work until the day the server changed.
    let provider = FakeMcp::start(true);
    let daemon = Daemon::start("mcp-sse");
    let project = daemon.dir.join("proj");
    std::fs::create_dir_all(&project).expect("project dir");
    arrange_mcp(&daemon, provider.port, &project);

    let mut rec = CapabilityRecord::new("memory", "mcp.request", "");
    rec.project = Some(project.to_string_lossy().into_owned());
    let reply = daemon
        .client()
        .use_with_body(rec, br#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#)
        .expect("use mcp-request");
    let output = match &reply {
        Response::Performed { output, .. } => output.clone(),
        other => panic!("expected a performed reply, got {other:?}"),
    };
    assert!(provider.sse);
    assert!(output.contains("data:"), "{output}");
    assert!(!output.contains(SENTINEL), "{output}");
}

#[test]
fn an_mcp_request_without_a_grant_never_reaches_the_provider() {
    // The grant is what stands between a session and a credential, and a
    // refusal that still made the request would be a refusal in name only.
    let provider = FakeMcp::start(false);
    let daemon = Daemon::start("mcp-nogrant");
    let project = daemon.dir.join("proj");
    std::fs::create_dir_all(&project).expect("project dir");

    let mut client = daemon.client();
    client
        .add(
            "memory",
            "127.0.0.1",
            "http",
            Some("x-access-token"),
            "/mcp",
            "bearer",
            Some(provider.port),
            &SecretValue::new(SENTINEL.into()),
        )
        .expect("add");

    let mut rec = CapabilityRecord::new("memory", "mcp.request", "");
    rec.project = Some(project.to_string_lossy().into_owned());
    let reply = daemon
        .client()
        .use_with_body(rec, br#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#)
        .expect("a reply, refused");
    assert!(
        reply.as_error().is_some_and(|(_, m)| m.contains("not granted")),
        "{reply:?}"
    );
    assert!(
        provider.authorizations().is_empty(),
        "the provider was contacted anyway"
    );
}

#[test]
fn a_credential_stored_without_an_endpoint_cannot_carry_a_message() {
    // The git credentials already on an upgraded machine have no path, and a
    // service that guessed one would be a service that picked a destination.
    let daemon = Daemon::start("mcp-nopath");
    let project = daemon.dir.join("proj");
    std::fs::create_dir_all(&project).expect("project dir");
    let mut client = daemon.client();
    client
        .add(
            "gh",
            "127.0.0.1",
            "http",
            Some("x-access-token"),
            "",
            "bearer",
            None,
            &SecretValue::new(SENTINEL.into()),
        )
        .expect("add");
    client
        .call(&Request::Grant {
            project: project.to_string_lossy().into_owned(),
            service: "gh".into(),
            capability: "mcp-request".into(),
            revoke: false,
        })
        .expect("grant");

    let mut rec = CapabilityRecord::new("gh", "mcp.request", "");
    rec.project = Some(project.to_string_lossy().into_owned());
    let reply = daemon
        .client()
        .use_with_body(rec, br#"{"id":1}"#)
        .expect("a reply, refused");
    assert!(
        reply.as_error().is_some_and(|(_, m)| m.contains("--path")),
        "{reply:?}"
    );
}

#[test]
fn a_credential_the_server_refuses_still_comes_back_as_a_successful_operation() {
    // The fact `apex mcp connect` is built on, measured rather than assumed.
    //
    // The broker's job ends when the request reaches the far end, so a 401 is
    // an operation that worked: `exit_code` is 0 and the trail records a
    // success. A verb that took that for proof would rewrite the agent's
    // configuration to name a credential the server has already refused, and
    // the person would find out at the start of their next session.
    let provider = FakeMcp::start_accepting_only(false, Some("Bearer the-only-one-it-takes"));
    let daemon = Daemon::start("mcp-refused");
    let project = daemon.dir.join("proj");
    std::fs::create_dir_all(&project).expect("project dir");
    arrange_mcp(&daemon, provider.port, &project);

    let mut rec = CapabilityRecord::new("memory", "mcp.request", "");
    rec.project = Some(project.to_string_lossy().into_owned());
    let reply = daemon
        .client()
        .use_with_body(rec, br#"{"jsonrpc":"2.0","id":1,"method":"initialize"}"#)
        .expect("the operation itself succeeds");

    let Response::Performed {
        exit_code, output, ..
    } = reply
    else {
        panic!("expected a performed reply, got {reply:?}");
    };
    // curl runs with `fail-with-body`, so an HTTP error status is a non-zero
    // exit that still carries the body. 22 is curl's code for it.
    assert_eq!(exit_code, 22, "{output}");
    // The credential did reach the server — this is a refusal, not a failure to
    // connect — and the reason is in the body, which is why the body is what
    // `apex mcp connect` reports rather than "could not reach the server".
    assert_eq!(provider.authorizations().len(), 1);
    assert!(output.contains("invalid_token"), "{output}");
    assert!(!output.contains("\"result\""), "{output}");
    // And the credential is still not in anything the caller can read.
    assert!(!output.contains(SENTINEL), "{output}");
}

#[test]
fn a_grant_held_in_every_project_is_only_for_an_operation_that_names_nothing() {
    // MCP servers are global and grants are per project, so a memory server
    // defined once in `~/.claude.json` is present in every directory and
    // unauthorised in every new worktree until somebody grants it again. The
    // `*` key closes that — and is refused for anything that acts on something
    // the caller names, because the same grant there would be a different
    // permission in every directory.
    let provider = FakeMcp::start(false);
    let daemon = Daemon::start("everywhere");
    let granted = daemon.dir.join("granted");
    let elsewhere = daemon.dir.join("a-new-worktree");
    for dir in [&granted, &elsewhere] {
        std::fs::create_dir_all(dir).expect("project dir");
    }
    // Adds the credential and grants `mcp-request` for `granted` only.
    arrange_mcp(&daemon, provider.port, &granted);

    let use_from = |project: &std::path::Path| {
        let mut rec = CapabilityRecord::new("memory", "mcp.request", "");
        rec.project = Some(project.to_string_lossy().into_owned());
        daemon
            .client()
            .use_with_body(rec, br#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#)
            .expect("a reply")
    };

    // The failure this exists to fix, measured first: the same server, one
    // directory across, refused.
    match use_from(&elsewhere) {
        Response::Error { message, .. } => {
            assert!(message.contains("not granted"), "{message}");
            // And the refusal names the way out, because this message is what
            // an agent relays to the person standing in the new worktree.
            assert!(message.contains("--everywhere"), "{message}");
        }
        other => panic!("a project with no grant was allowed: {other:?}"),
    }

    // `git.push` acts on a remote resolved out of the caller's repository, so
    // it is refused the key outright.
    let refused = daemon
        .client()
        .call(&Request::Grant {
            project: "*".into(),
            service: "memory".into(),
            capability: "git.push".into(),
            revoke: false,
        })
        .expect_err("git.push must not be grantable everywhere")
        .to_string();
    assert!(refused.contains("acts on something you name"), "{refused}");

    // `mcp.request` names nothing, so it is accepted.
    daemon
        .client()
        .call(&Request::Grant {
            project: "*".into(),
            service: "memory".into(),
            capability: "mcp.request".into(),
            revoke: false,
        })
        .expect("grant everywhere");

    // The same call from the same directory now works, and the credential
    // still only reaches the endpoint it was stored for.
    match use_from(&elsewhere) {
        Response::Performed {
            exit_code,
            endpoint,
            output,
            ..
        } => {
            assert_eq!(exit_code, 0, "{output}");
            assert_eq!(endpoint, "http://127.0.0.1", "{endpoint}");
            assert!(!output.contains(SENTINEL), "{output}");
        }
        other => panic!("the grant did not apply: {other:?}"),
    }

    // Withdrawn everywhere by the same key, in one call rather than per
    // directory — and the project's own grant is untouched by it.
    daemon
        .client()
        .call(&Request::Grant {
            project: "*".into(),
            service: "memory".into(),
            capability: "mcp.request".into(),
            revoke: true,
        })
        .expect("revoke everywhere");
    assert!(
        matches!(use_from(&elsewhere), Response::Error { .. }),
        "the withdrawn grant still applied"
    );
    assert!(
        matches!(use_from(&granted), Response::Performed { .. }),
        "revoking the wildcard took the project's own grant with it"
    );
}
