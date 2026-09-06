//! End-to-end assertions against a real `apex-secretd`, over real sockets.
//!
//! Everything below drives the shipped binary — spawned with its own state and
//! runtime directories, so it needs no root, touches nothing the machine is
//! using, and cannot reach a daemon somebody is relying on. The provider it
//! talks to is a `TcpListener` in this process, so no test here uses the
//! network and no real credential exists anywhere.
//!
//! The four acceptance criteria this file exists for:
//!
//! * **one** — the store is outside any home path:
//!   [`the_store_lives_where_no_agent_can_read_it`].
//! * **two** — the API cannot return a value:
//!   [`no_reply_to_any_verb_contains_the_credential`] drives every request the
//!   protocol has and greps the raw bytes of every reply.
//! * **three** — the broker performs a credential-backed operation:
//!   [`the_broker_performs_the_operation_and_the_caller_never_sees_the_token`].
//! * **four** — audit entries exist for secret use:
//!   [`the_audit_log_records_the_use_with_the_fields_section_eleven_names`].

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// The credential every test stores. Obviously not a real token, and unique per
/// process so a stray match cannot come from somewhere else.
fn sentinel() -> String {
    format!("not-a-real-token-{}", std::process::id())
}

// ── a provider that is really just a socket ─────────────────────────────────

/// A one-file HTTP/1.1 server standing in for `api.github.com`.
///
/// It records the headers of every request it receives, which is how the test
/// proves the credential actually reached the provider — the interesting half
/// of "the broker performed the operation".
struct FakeProvider {
    base: String,
    seen: Arc<Mutex<Vec<String>>>,
}

impl FakeProvider {
    fn start(body: &'static str, status: &'static str) -> FakeProvider {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind a loopback port");
        let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let recorder = Arc::clone(&seen);
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let recorder = Arc::clone(&recorder);
                std::thread::spawn(move || serve_once(stream, &recorder, body, status));
            }
        });
        FakeProvider { base, seen }
    }

    /// Every request line and header this provider has been sent.
    fn requests(&self) -> Vec<String> {
        self.seen.lock().expect("recorder lock").clone()
    }
}

fn serve_once(mut stream: TcpStream, seen: &Mutex<Vec<String>>, body: &str, status: &str) {
    let mut reader = BufReader::new(stream.try_clone().expect("clone"));
    let mut head = String::new();
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {
                if line == "\r\n" || line == "\n" {
                    break;
                }
                head.push_str(&line);
            }
            Err(_) => break,
        }
    }
    seen.lock().expect("recorder lock").push(head);
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.flush();
}

// ── the daemon under test ───────────────────────────────────────────────────

/// A running `apex-secretd` with its own directories, stopped on drop.
struct Daemon {
    child: std::process::Child,
    dir: PathBuf,
    broker: PathBuf,
    admin: PathBuf,
    log: PathBuf,
}

impl Daemon {
    fn start(tag: &str) -> Daemon {
        let dir = std::env::temp_dir().join(format!(
            "apex-secretd-wire-{tag}-{}",
            std::process::id()
        ));
        std::fs::remove_dir_all(&dir).ok();
        let state = dir.join("state");
        let runtime = dir.join("run");
        std::fs::create_dir_all(&state).expect("state dir");
        std::fs::create_dir_all(&runtime).expect("runtime dir");
        let log = dir.join("stderr.log");

        // stderr to a file rather than a pipe: a pipe nobody drains fills and
        // blocks the daemon, and the whole point of keeping it is to grep it
        // for the sentinel at the end.
        let stderr = std::fs::File::create(&log).expect("log file");
        let child = std::process::Command::new(env!("CARGO_BIN_EXE_apex-secretd"))
            .arg("--state-dir")
            .arg(&state)
            .arg("--runtime-dir")
            .arg(&runtime)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(stderr)
            .spawn()
            .expect("spawn apex-secretd");

        let daemon = Daemon {
            child,
            dir,
            broker: runtime.join("broker.sock"),
            admin: runtime.join("admin.sock"),
            log,
        };
        daemon.wait_until_listening();
        daemon
    }

    fn wait_until_listening(&self) {
        for _ in 0..600 {
            if UnixStream::connect(&self.broker).is_ok() && UnixStream::connect(&self.admin).is_ok()
            {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!(
            "apex-secretd did not start within 6s; its log said:\n{}",
            self.stderr()
        );
    }

    fn state_dir(&self) -> PathBuf {
        self.dir.join("state")
    }

    fn stderr(&self) -> String {
        std::fs::read_to_string(&self.log).unwrap_or_default()
    }

    /// Send one raw line and read the raw reply.
    ///
    /// Raw on purpose. A parsed `Response` proves nothing about criterion two —
    /// the question is what bytes crossed the socket, so the assertions run
    /// against exactly those.
    ///
    /// The request is folded onto one line first. The protocol is
    /// newline-delimited, and the literals below are written across several
    /// lines to stay readable; sending them verbatim would deliver a truncated
    /// first line and a pile of garbage after it.
    fn raw(&self, socket: &Path, request: &str) -> String {
        let request: String = request
            .lines()
            .map(str::trim)
            .collect::<Vec<_>>()
            .join(" ");
        let mut stream = UnixStream::connect(socket)
            .unwrap_or_else(|e| panic!("connecting to {}: {e}", socket.display()));
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(30)))
            .ok();
        writeln!(stream, "{request}").expect("write");
        stream.flush().ok();
        let mut reply = String::new();
        BufReader::new(stream)
            .read_line(&mut reply)
            .expect("read a reply");
        assert!(!reply.trim().is_empty(), "empty reply to {request}");
        reply
    }

    fn broker(&self, request: &str) -> String {
        self.raw(&self.broker.clone(), request)
    }

    fn admin(&self, request: &str) -> String {
        self.raw(&self.admin.clone(), request)
    }

    fn json(&self, reply: &str) -> serde_json::Value {
        serde_json::from_str(reply.trim()).unwrap_or_else(|e| panic!("{reply}: {e}"))
    }

    /// The audit log for this uid, as parsed records.
    fn audit_records(&self) -> Vec<serde_json::Value> {
        let path = self
            .state_dir()
            .join("owners")
            .join(uid().to_string())
            .join("audit.jsonl");
        std::fs::read_to_string(path)
            .unwrap_or_default()
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect()
    }

    /// Every byte the daemon has written under its state directory.
    fn state_bytes(&self) -> String {
        let mut out = String::new();
        collect(&self.state_dir(), &mut out);
        fn collect(dir: &Path, out: &mut String) {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    collect(&path, out);
                } else {
                    let mut bytes = Vec::new();
                    if let Ok(mut f) = std::fs::File::open(&path) {
                        let _ = f.read_to_end(&mut bytes);
                    }
                    out.push_str(&path.to_string_lossy());
                    out.push('\n');
                    out.push_str(&String::from_utf8_lossy(&bytes));
                    out.push('\n');
                }
            }
        }
        out
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        // Only ever this test's own child, never a pid found by name: killing
        // a daemon somebody is using is exactly the accident this avoids.
        let _ = self.child.kill();
        let _ = self.child.wait();
        std::fs::remove_dir_all(&self.dir).ok();
    }
}

fn uid() -> u32 {
    // Safe: getuid cannot fail and has no side effects.
    unsafe { libc::getuid() }
}

/// Store the sentinel against a fake provider, and grant `whoami` on it.
fn store_and_grant(daemon: &Daemon, base: &str) {
    let stored = daemon.admin(&format!(
        r#"{{"op":"store","owner":{},"secret":"gh","provider":"github",
            "base_url":"{base}","label":"a fake for the test","value":"{}"}}"#,
        uid(),
        sentinel()
    ));
    assert_eq!(
        daemon.json(&stored)["reply"], "secret",
        "storing failed: {stored}"
    );
    let granted = daemon.admin(&format!(
        r#"{{"op":"grant","owner":{},"secret":"gh","operation":"whoami"}}"#,
        uid()
    ));
    assert_eq!(
        daemon.json(&granted)["reply"], "grants",
        "granting failed: {granted}"
    );
}

const WHOAMI_BODY: &str = r#"{"login":"andre","id":4242,"type":"User","email":"nobody@example.com"}"#;

// ── criterion three: the broker performs the operation ──────────────────────

#[test]
fn the_broker_performs_the_operation_and_the_caller_never_sees_the_token() {
    let provider = FakeProvider::start(WHOAMI_BODY, "200 OK");
    let daemon = Daemon::start("perform");
    store_and_grant(&daemon, &provider.base);

    let reply = daemon.broker(
        r#"{"op":"use","secret":"gh","operation":"whoami","project":"/p/demo","agent_session":7}"#,
    );
    let v = daemon.json(&reply);
    assert_eq!(v["reply"], "performed", "{reply}");
    assert_eq!(v["outcome"]["status"], 200, "{reply}");
    // The allow-listed fields came back...
    assert_eq!(v["outcome"]["fields"]["login"], "andre", "{reply}");
    assert_eq!(v["outcome"]["fields"]["id"], "4242", "{reply}");
    // ...and the field that is not on the list did not, though the provider
    // sent it.
    assert!(
        v["outcome"]["fields"].get("email").is_none(),
        "a field outside the allow-list was returned: {reply}"
    );

    // The half that makes this a real brokered operation rather than a mock:
    // the credential genuinely reached the provider, in the Authorization
    // header, sent by the daemon.
    let requests = provider.requests();
    assert_eq!(requests.len(), 1, "the provider was not called once: {requests:?}");
    let request = &requests[0];
    assert!(request.starts_with("GET /user "), "{request}");
    assert!(
        request.contains(&format!("Authorization: Bearer {}", sentinel())),
        "the credential did not reach the provider: {request}"
    );

    // And it did not come back.
    assert!(
        !reply.contains(&sentinel()),
        "the credential was in the reply: {reply}"
    );
}

#[test]
fn a_provider_refusal_is_reported_without_the_credential() {
    // The 401 path, which is where an API most wants to quote the header it
    // did not like back at you.
    let provider = FakeProvider::start(
        r#"{"message":"Bad credentials","documentation_url":"https://example.invalid"}"#,
        "401 Unauthorized",
    );
    let daemon = Daemon::start("refused");
    store_and_grant(&daemon, &provider.base);

    let reply = daemon.broker(r#"{"op":"use","secret":"gh","operation":"whoami"}"#);
    let v = daemon.json(&reply);
    assert_eq!(v["reply"], "performed", "{reply}");
    assert_eq!(v["outcome"]["status"], 401, "{reply}");
    assert_eq!(v["outcome"]["detail"], "Bad credentials", "{reply}");
    assert!(!reply.contains(&sentinel()), "{reply}");

    // A provider failure is audited too, or a token that stopped working would
    // leave no trace of having been tried.
    let failed: Vec<_> = daemon
        .audit_records()
        .into_iter()
        .filter(|r| r["event"] == "failed")
        .collect();
    assert_eq!(failed.len(), 1, "{:?}", daemon.audit_records());
    assert_eq!(failed[0]["status"], 401);
}

// ── criterion four: the audit trail ─────────────────────────────────────────

#[test]
fn the_audit_log_records_the_use_with_the_fields_section_eleven_names() {
    let provider = FakeProvider::start(WHOAMI_BODY, "200 OK");
    let daemon = Daemon::start("audit");
    store_and_grant(&daemon, &provider.base);

    let reply = daemon.broker(
        r#"{"op":"use","secret":"gh","operation":"whoami","project":"/p/demo",
            "agent_session":7,"origin":"remote"}"#,
    );
    let performed = daemon.json(&reply);
    let audit_id = performed["audit_id"].as_str().expect("an audit id").to_string();
    assert_eq!(audit_id.len(), 16, "{reply}");

    let records = daemon.audit_records();
    let used: Vec<_> = records.iter().filter(|r| r["event"] == "used").collect();
    assert_eq!(used.len(), 1, "expected one use record in {records:?}");
    let r = used[0];

    // The id in the reply is the id in the log: that join is what makes the
    // trail answer "what did the thing I ran at 14:03 actually do".
    assert_eq!(r["audit_id"], audit_id.as_str());

    // §11's fields, as the log records them.
    assert_eq!(r["provider"], "github");
    assert_eq!(r["operation"], "whoami");
    assert!(r["resource"].is_null(), "whoami takes no resource");
    assert_eq!(r["secret"], "gh");
    assert_eq!(r["decision"], "allow");
    assert_eq!(r["status"], 200);

    // Kernel-established identity.
    assert_eq!(r["owner_uid"], uid());
    assert_eq!(r["peer_uid"], uid());
    assert_eq!(
        r["peer_pid"].as_i64(),
        Some(std::process::id() as i64),
        "the peer pid was not recorded as this test process"
    );

    // Claimed identity, recorded because the caller said it, not because the
    // service checked it.
    assert_eq!(r["project"], "/p/demo");
    assert_eq!(r["agent_session"], 7);
    assert_eq!(r["request_origin"], "remote");

    // A timestamp in milliseconds, this century.
    assert!(r["ms"].as_u64().unwrap_or(0) > 1_700_000_000_000, "{r}");

    // Administrative changes are audited too, and are distinguishable.
    let events: Vec<&str> = records.iter().filter_map(|r| r["event"].as_str()).collect();
    assert!(events.contains(&"stored"), "{events:?}");
    assert!(events.contains(&"granted"), "{events:?}");
}

#[test]
fn a_capability_without_a_grant_is_refused_and_the_refusal_is_audited() {
    let provider = FakeProvider::start(WHOAMI_BODY, "200 OK");
    let daemon = Daemon::start("ungranted");
    // Stored, deliberately not granted.
    let stored = daemon.admin(&format!(
        r#"{{"op":"store","owner":{},"secret":"gh","provider":"github",
            "base_url":"{}","value":"{}"}}"#,
        uid(),
        provider.base,
        sentinel()
    ));
    assert_eq!(daemon.json(&stored)["reply"], "secret", "{stored}");

    let reply = daemon.broker(r#"{"op":"use","secret":"gh","operation":"whoami"}"#);
    let v = daemon.json(&reply);
    assert_eq!(v["reply"], "error", "{reply}");
    assert_eq!(v["kind"], "permission_denied", "{reply}");

    // The credential was never read, so the provider was never called.
    assert!(
        provider.requests().is_empty(),
        "a refused capability still reached the provider: {:?}",
        provider.requests()
    );

    let refused: Vec<_> = daemon
        .audit_records()
        .into_iter()
        .filter(|r| r["event"] == "refused")
        .collect();
    assert_eq!(refused.len(), 1);
    assert_eq!(refused[0]["decision"], "deny:not-granted");
    assert_eq!(refused[0]["operation"], "whoami");
}

// ── criterion two: no reply can carry a value ───────────────────────────────

#[test]
fn no_reply_to_any_verb_contains_the_credential() {
    let provider = FakeProvider::start(WHOAMI_BODY, "200 OK");
    let daemon = Daemon::start("surface");
    store_and_grant(&daemon, &provider.base);

    let me = uid();
    // Every verb the protocol has, in an order that exercises the success path
    // and the failure path of each. The list is asserted complete below against
    // the protocol's own vocabulary, so a verb added later cannot slip past
    // this sweep.
    let broker_calls = vec![
        r#"{"op":"hello"}"#.to_string(),
        r#"{"op":"providers"}"#.to_string(),
        r#"{"op":"list"}"#.to_string(),
        r#"{"op":"info","secret":"gh"}"#.to_string(),
        r#"{"op":"info","secret":"absent"}"#.to_string(),
        r#"{"op":"grants"}"#.to_string(),
        r#"{"op":"audit","limit":100}"#.to_string(),
        r#"{"op":"use","secret":"gh","operation":"whoami"}"#.to_string(),
        // Failure paths, which is where a careless error message would quote
        // something it should not.
        r#"{"op":"use","secret":"gh","operation":"nonesuch"}"#.to_string(),
        r#"{"op":"use","secret":"absent","operation":"whoami"}"#.to_string(),
        r#"{"op":"use","secret":"gh","operation":"repo-metadata","resource":"../../user"}"#
            .to_string(),
        r#"{"op":"use","secret":"gh","operation":"repo-metadata"}"#.to_string(),
        r#"{"op":"use","secret":"../escape","operation":"whoami"}"#.to_string(),
        // Mutating verbs on the wrong socket.
        format!(r#"{{"op":"remove","owner":{me},"secret":"gh"}}"#),
        r#"{"op":"nonsense"}"#.to_string(),
        "{ not json".to_string(),
    ];
    let admin_calls = vec![
        // A duplicate store, which fails — and the failing request carried the
        // credential, so its reply is the most likely place to echo one.
        format!(
            r#"{{"op":"store","owner":{me},"secret":"gh","provider":"github","value":"{}"}}"#,
            sentinel()
        ),
        format!(
            r#"{{"op":"store","owner":{me},"secret":"second","provider":"github","base_url":"{}","value":"{}"}}"#,
            provider.base,
            sentinel()
        ),
        format!(
            r#"{{"op":"store","owner":{me},"secret":"bad","provider":"github","value":"has space"}}"#
        ),
        format!(
            r#"{{"op":"rotate","owner":{me},"secret":"gh","value":"{}-rotated"}}"#,
            sentinel()
        ),
        format!(
            r#"{{"op":"rotate","owner":{me},"secret":"absent","value":"{}"}}"#,
            sentinel()
        ),
        format!(r#"{{"op":"grant","owner":{me},"secret":"gh","operation":"repo-metadata"}}"#),
        format!(
            r#"{{"op":"grant","owner":{me},"secret":"gh","operation":"repo-metadata","revoke":true}}"#
        ),
        format!(r#"{{"op":"remove","owner":{me},"secret":"second"}}"#),
        format!(r#"{{"op":"remove","owner":{me},"secret":"absent"}}"#),
    ];

    let mut replies = Vec::new();
    for call in &broker_calls {
        replies.push((call.clone(), daemon.broker(call)));
    }
    for call in &admin_calls {
        replies.push((call.clone(), daemon.admin(call)));
    }

    // Every verb in the protocol was exercised. Hard-coded strings would drift
    // from the enum; this reads the vocabulary out of the requests actually
    // sent and checks the set.
    let mut exercised: Vec<String> = replies
        .iter()
        .filter_map(|(call, _)| serde_json::from_str::<serde_json::Value>(call).ok())
        .filter_map(|v| v["op"].as_str().map(str::to_string))
        .collect();
    exercised.sort();
    exercised.dedup();
    for verb in [
        "audit", "grant", "grants", "hello", "info", "list", "providers", "remove", "rotate",
        "store", "use",
    ] {
        assert!(
            exercised.contains(&verb.to_string()),
            "the verb '{verb}' was never sent, so this sweep does not cover it"
        );
    }

    // The assertion this file exists for.
    for (call, reply) in &replies {
        for needle in [sentinel(), format!("{}-rotated", sentinel())] {
            assert!(
                !reply.contains(&needle),
                "a credential came back over the socket.\n  request: {call}\n  reply:   {reply}"
            );
        }
        assert!(
            !reply.contains("Bearer "),
            "an Authorization header came back.\n  request: {call}\n  reply: {reply}"
        );
    }

    // Nor into the audit log, which is also read back over the API.
    let audit = daemon.audit_records();
    assert!(!audit.is_empty(), "nothing was audited at all");
    let audit_text = serde_json::to_string(&audit).unwrap();
    assert!(
        !audit_text.contains(&sentinel()),
        "a credential reached the audit log: {audit_text}"
    );

    // Nor into the daemon's own diagnostics, which is where a debug print of a
    // failed request would land.
    let stderr = daemon.stderr();
    assert!(
        !stderr.contains(&sentinel()),
        "a credential reached the daemon's log: {stderr}"
    );

    // The one place it IS allowed to be is the store, and only there.
    let state = daemon.state_bytes();
    assert!(
        state.contains(&format!("{}-rotated", sentinel())),
        "the rotated credential is not in the store, so this test proved nothing"
    );
}

#[test]
fn a_credential_the_provider_echoes_back_is_still_not_returned() {
    // The nastiest realistic case: the upstream repeats the Authorization
    // header into a field that IS on the allow-list. The type wall cannot help
    // here — the value arrives as ordinary provider text — so the scrub at the
    // provider boundary is what has to hold.
    let daemon = Daemon::start("echo");
    let body: &'static str = Box::leak(
        format!(
            r#"{{"login":"echoing {0}","type":"User","message":"rejected Bearer {0}"}}"#,
            sentinel()
        )
        .into_boxed_str(),
    );
    let provider = FakeProvider::start(body, "200 OK");
    store_and_grant(&daemon, &provider.base);

    let reply = daemon.broker(r#"{"op":"use","secret":"gh","operation":"whoami"}"#);
    assert!(
        !reply.contains(&sentinel()),
        "the provider's echo reached the caller: {reply}"
    );
    assert!(reply.contains("<withheld>"), "{reply}");

    // And the audit log records the same scrubbed text, not the raw echo.
    let audit = serde_json::to_string(&daemon.audit_records()).unwrap();
    assert!(!audit.contains(&sentinel()), "{audit}");
}

// ── criterion one: the store is not in a home directory ─────────────────────

#[test]
fn the_store_lives_where_no_agent_can_read_it() {
    use std::os::unix::fs::PermissionsExt;

    let provider = FakeProvider::start(WHOAMI_BODY, "200 OK");
    let daemon = Daemon::start("store");
    store_and_grant(&daemon, &provider.base);

    // Where the packaged service puts things, asserted against the binary's own
    // default rather than a string copied into the test.
    let help = std::process::Command::new(env!("CARGO_BIN_EXE_apex-secretd"))
        .arg("--help")
        .output()
        .expect("run --help");
    let help = String::from_utf8_lossy(&help.stdout);
    assert!(help.contains("--state-dir"), "{help}");

    // Every file the daemon wrote is inside its state directory, and every one
    // of them is unreadable by anyone else.
    let owner = daemon.state_dir().join("owners").join(uid().to_string());
    for path in [
        owner.join("secrets/gh.json"),
        owner.join("secrets/gh.blob"),
        owner.join("grants.json"),
        owner.join("audit.jsonl"),
    ] {
        assert!(path.exists(), "{} was never written", path.display());
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(
            mode & 0o777,
            0o600,
            "{} is {:o}",
            path.display(),
            mode & 0o777
        );
        let text = path.to_string_lossy();
        for home in ["/home/", "/var/home/", "/root/", ".local/state", ".config"] {
            assert!(!text.contains(home), "{text} is inside an agent-readable path");
        }
    }

    // Including every directory on the way down: a traversable parent leaks the
    // names of the credentials inside it.
    for dir in [
        daemon.state_dir(),
        daemon.state_dir().join("owners"),
        owner.clone(),
        owner.join("secrets"),
    ] {
        let mode = std::fs::metadata(&dir).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700, "{} is {:o}", dir.display(), mode & 0o777);
    }
}

// ── the socket split ────────────────────────────────────────────────────────

#[test]
fn the_broker_socket_refuses_every_mutating_verb() {
    // The boundary that makes a grant mean something. A managed agent runs
    // under the owner's uid and can open the broker socket; if it could grant
    // there, a grant would be something an agent gives itself.
    let daemon = Daemon::start("split");
    let me = uid();
    let mutations = [
        format!(
            r#"{{"op":"store","owner":{me},"secret":"gh","provider":"github","value":"{}"}}"#,
            sentinel()
        ),
        format!(r#"{{"op":"grant","owner":{me},"secret":"gh","operation":"whoami"}}"#),
        format!(
            r#"{{"op":"rotate","owner":{me},"secret":"gh","value":"{}"}}"#,
            sentinel()
        ),
        format!(r#"{{"op":"remove","owner":{me},"secret":"gh"}}"#),
    ];
    for call in &mutations {
        let reply = daemon.broker(call);
        let v = daemon.json(&reply);
        assert_eq!(v["reply"], "error", "{call} was answered: {reply}");
        assert_eq!(v["kind"], "permission_denied", "{reply}");
        assert!(
            v["message"].as_str().unwrap_or_default().contains("sudo"),
            "the refusal must say how to do it properly: {reply}"
        );
    }
    // Nothing was stored by any of that.
    let listed = daemon.broker(r#"{"op":"list"}"#);
    assert_eq!(daemon.json(&listed)["secrets"].as_array().unwrap().len(), 0);
}

#[test]
fn the_admin_socket_is_not_openable_by_other_accounts() {
    use std::os::unix::fs::PermissionsExt;

    let daemon = Daemon::start("modes");
    let admin = std::fs::metadata(&daemon.admin).unwrap().permissions().mode();
    assert_eq!(admin & 0o777, 0o600, "admin socket is {:o}", admin & 0o777);
    // The broker socket is deliberately open: every uid has its own namespace
    // behind it, keyed on SO_PEERCRED, and connecting as yourself reaches only
    // your own.
    let broker = std::fs::metadata(&daemon.broker).unwrap().permissions().mode();
    assert_eq!(broker & 0o777, 0o666, "broker socket is {:o}", broker & 0o777);
}

// ── the vocabulary is closed ────────────────────────────────────────────────

#[test]
fn there_is_no_verb_that_returns_a_credential() {
    // Tried by name, over the real socket. None of these is a verb, and the
    // point is that adding one would have to be a deliberate act visible in the
    // protocol enum rather than an accident.
    let daemon = Daemon::start("closed");
    for attempt in [
        r#"{"op":"get","secret":"gh"}"#,
        r#"{"op":"read","secret":"gh"}"#,
        r#"{"op":"export","secret":"gh"}"#,
        r#"{"op":"token","secret":"gh"}"#,
        r#"{"op":"value","secret":"gh"}"#,
        r#"{"op":"reveal","secret":"gh"}"#,
        r#"{"op":"open","secret":"gh"}"#,
    ] {
        let reply = daemon.broker(attempt);
        let v = daemon.json(&reply);
        assert_eq!(v["reply"], "error", "{attempt} was understood: {reply}");
        assert_eq!(v["kind"], "bad_request", "{reply}");
    }
}

#[test]
fn an_oversized_request_is_refused_rather_than_buffered() {
    let daemon = Daemon::start("huge");
    let huge = format!(
        r#"{{"op":"info","secret":"{}"}}"#,
        "x".repeat(128 * 1024)
    );
    let reply = daemon.broker(&huge);
    let v = daemon.json(&reply);
    assert_eq!(v["reply"], "error", "{reply}");
    assert_eq!(v["kind"], "bad_request", "{reply}");
    // The daemon is still alive afterwards.
    let hello = daemon.broker(r#"{"op":"hello"}"#);
    assert_eq!(daemon.json(&hello)["reply"], "hello", "{hello}");
}

#[test]
fn the_capability_vocabulary_is_advertised_with_what_each_operation_returns() {
    // What `apex capability providers` shows. A caller has to be able to see
    // the closed vocabulary, or "ask only for named operations" is advice
    // nobody can follow.
    let daemon = Daemon::start("vocab");
    let reply = daemon.broker(r#"{"op":"providers"}"#);
    let v = daemon.json(&reply);
    let providers = v["providers"].as_array().expect("a list");
    assert!(!providers.is_empty(), "{reply}");
    let github = providers
        .iter()
        .find(|p| p["id"] == "github")
        .expect("github is offered");
    let ops: BTreeMap<&str, &serde_json::Value> = github["operations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| (o["id"].as_str().unwrap(), o))
        .collect();
    assert!(ops.contains_key("whoami"), "{reply}");
    assert_eq!(ops["whoami"]["method"], "GET");
    assert!(ops["whoami"]["resource_shape"].is_null());
    assert_eq!(ops["repo-metadata"]["resource_shape"], "owner/repo");
    // The fields an operation may return are part of the contract.
    let fields = ops["whoami"]["fields"].as_array().unwrap();
    assert!(fields.iter().any(|f| f == "login"), "{reply}");
}
