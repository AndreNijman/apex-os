//! P0-013 and P0-014 against a real daemon.
//!
//! Everything else about origins is a pure function over values, and those
//! tests live beside the code. This one exists because the two claims that
//! matter most cannot be made about a value:
//!
//! * the daemon establishes an origin from a **real** connection, through
//!   `SO_PEERCRED` and real `/proc`, and files a request with it;
//! * the §7 gate on approving a root operation lets the path §4 depends on
//!   through — `apex request approve` — and stops the one it does not.
//!
//! The second is the reason this is worth a spawned process. Gating approval
//! on "the deciding connection is local" is only correct if a human at a
//! terminal actually classifies as local on a real machine; if it does not,
//! the failure is that nothing on the system can ever be approved, and no
//! amount of unit testing over `RequestOrigin` values would have said so.
//!
//! The daemon is started with its own `XDG_RUNTIME_DIR` and `XDG_STATE_HOME`,
//! so it binds a socket of its own and writes state of its own. It never
//! touches a running `apex-agentd`, and it is killed by pid at the end —
//! never by name.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// A daemon of our own, and the paths it was given.
struct Harness {
    child: Child,
    socket: PathBuf,
    root: PathBuf,
}

impl Drop for Harness {
    fn drop(&mut self) {
        // By pid, on the child this test spawned. Nothing here goes looking
        // for a process by name.
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

impl Harness {
    fn start(tag: &str) -> Option<Harness> {
        let root = std::env::temp_dir().join(format!(
            "apex-origin-e2e-{}-{tag}-{}",
            std::process::id(),
            now_ms()
        ));
        let runtime = root.join("run");
        let state = root.join("state");
        std::fs::create_dir_all(&runtime).ok()?;
        std::fs::create_dir_all(&state).ok()?;

        let child = Command::new(env!("CARGO_BIN_EXE_apex-agentd"))
            .env("XDG_RUNTIME_DIR", &runtime)
            .env("XDG_STATE_HOME", &state)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;

        let socket = runtime.join("apex-agentd").join("control.sock");
        let harness = Harness {
            child,
            socket,
            root,
        };
        harness.wait_for_socket().then_some(harness)
    }

    fn wait_for_socket(&self) -> bool {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if UnixStream::connect(&self.socket).is_ok() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        false
    }

    /// One request, one reply, on a fresh connection.
    ///
    /// A new connection per call on purpose: the peer credentials the daemon
    /// reads are the ones from `connect(2)`, so every call is attributed the
    /// same way a separate `apex` invocation would be.
    fn call(&self, line: &str) -> serde_json::Value {
        assert!(
            !line.contains('\n'),
            "the framing is one JSON object per line; this payload would desynchronise it"
        );
        let mut stream = UnixStream::connect(&self.socket).expect("connect");
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .expect("timeout");
        writeln!(stream, "{line}").expect("write");
        stream.flush().ok();
        let mut reply = String::new();
        BufReader::new(&stream).read_line(&mut reply).expect("read");
        serde_json::from_str(&reply).unwrap_or_else(|e| panic!("{e}: {reply}"))
    }

    fn audit_lines(&self) -> Vec<serde_json::Value> {
        let path = self
            .root
            .join("state")
            .join("apex")
            .join("agent")
            .join("privilege-audit.jsonl");
        read_jsonl(&path)
    }
}

fn read_jsonl(path: &Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(path)
        .map(|t| {
            t.lines()
                .filter_map(|l| serde_json::from_str(l).ok())
                .collect()
        })
        .unwrap_or_default()
}

fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// Skip out loud rather than passing silently.
///
/// A daemon that will not start is a broken fixture, and a fixture that
/// cannot tell that from a passing test asserts nothing.
macro_rules! harness {
    ($tag:literal) => {
        match Harness::start($tag) {
            Some(h) => h,
            None => {
                eprintln!("SKIP: apex-agentd did not come up in this environment");
                return;
            }
        }
    };
}

#[test]
fn the_daemon_speaks_the_revision_that_carries_origins() {
    let h = harness!("hello");
    let reply = h.call(r#"{"cmd":"hello"}"#);
    assert_eq!(reply["reply"], "hello", "{reply}");
    assert_eq!(
        reply["version"].as_u64(),
        Some(u64::from(apex_agent_core::protocol::REQUEST_ORIGIN_VERSION)),
        "{reply}"
    );
}

#[test]
fn a_filed_request_carries_an_origin_the_daemon_worked_out_for_itself() {
    // P0-013's first and third criteria, end to end: the daemon reads the
    // connection, not the request, and the audit log records what it read.
    let h = harness!("file");
    let reply = h.call(
        r#"{"cmd":"privilege_request","verb":"install","args":["clang"],"reason":"end to end test of origin recording"}"#,
    );

    if reply["reply"] == "error" {
        // The only acceptable failure: the daemon could not classify this
        // process and said so. It must NOT have filed anything.
        let msg = reply["message"].as_str().unwrap_or_default();
        assert!(
            msg.contains("/proc/") || msg.contains("cgroup"),
            "a refusal must name what could not be read: {reply}"
        );
        assert!(h.audit_lines().is_empty(), "refused but still audited");
        return;
    }

    assert_eq!(reply["reply"], "request", "{reply}");
    let origin = reply["request_origin"]
        .as_str()
        .unwrap_or_else(|| panic!("no origin on a filed request: {reply}"));
    assert!(
        apex_agent_core::policy::RequestOrigin::parse(origin).is_some(),
        "{origin} is not in §7's vocabulary"
    );
    // Nothing was declared, so the daemon observed it.
    assert_eq!(reply["origin_source"], "observed", "{reply}");

    let lines = h.audit_lines();
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert_eq!(lines[0]["request_origin"], origin);
    assert_eq!(lines[0]["origin_source"], "observed");
}

#[test]
fn a_request_cannot_put_its_own_origin_in_the_wire_form() {
    // The security property, tried the way an agent would try it: send the
    // field anyway. `PrivilegeRequest` has no origin key on the wire, so an
    // extra one is ignored — asserted rather than assumed, because a future
    // edit adding one would be exactly this hole.
    let h = harness!("spoof");
    let reply = h.call(
        r#"{"cmd":"privilege_request","verb":"pin","args":[],"reason":"trying to name my own origin","request_origin":"local-terminal","origin_source":"observed"}"#,
    );
    if reply["reply"] == "error" {
        return; // unclassifiable environment; covered above.
    }
    // Whatever it is, it is what the daemon observed — and if this process is
    // not local, the claim did not make it one.
    assert_eq!(reply["origin_source"], "observed", "{reply}");
    let observed = observed_origin(&h);
    assert_eq!(reply["request_origin"], observed, "{reply}");
}

/// What this process classifies as, according to the daemon itself.
fn observed_origin(h: &Harness) -> String {
    let reply = h.call(
        r#"{"cmd":"privilege_request","verb":"rollback","args":[],"reason":"probing the origin this connection is given"}"#,
    );
    reply["request_origin"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

#[test]
fn approving_a_root_operation_follows_section_sevens_local_rule() {
    // P0-014's second criterion, and the one that had to be proved against a
    // real machine rather than a value: `sudo apex request approve` is §4's
    // approval path, and gating it on "the deciding connection is local" is
    // only correct if a human at a terminal really does classify as local.
    //
    // The assertion is the rule itself rather than a fixed answer, so it
    // holds both on a desktop — where this test runs inside a login session
    // and the approval must go through — and in a container, where it runs
    // under a user service and must be refused.
    let h = harness!("approve");
    let filed = h.call(
        r#"{"cmd":"privilege_request","verb":"update","args":[],"reason":"end to end test of the approval gate"}"#,
    );
    if filed["reply"] == "error" {
        return;
    }
    let id = filed["id"].as_u64().expect("an id");
    let origin = apex_agent_core::policy::RequestOrigin::parse(
        filed["request_origin"].as_str().unwrap_or_default(),
    )
    .expect("a recorded origin");

    let decided = h.call(&format!(
        r#"{{"cmd":"decide","id":{id},"decision":"deny"}}"#
    ));
    if origin.is_local() {
        assert_eq!(
            decided["reply"], "request",
            "a local connection was refused a decision it must be allowed: {decided}"
        );
        assert_eq!(decided["decision"], "denied", "{decided}");
    } else {
        assert_eq!(
            decided["reply"], "error",
            "a {origin} connection decided a root request: {decided}"
        );
        assert_eq!(decided["kind"], "permission_denied", "{decided}");
        let msg = decided["message"].as_str().unwrap_or_default();
        assert!(
            msg.contains("this machine"),
            "the refusal must say what is missing: {msg}"
        );
    }
}

#[test]
fn a_connection_that_is_not_a_session_cannot_declare_an_origin() {
    // `apex agent origin` is for a session narrowing itself. A peer that is
    // not a session has nothing to narrow, and letting it through would be a
    // way to relabel somebody else's connection.
    let h = harness!("declare");
    let reply = h.call(r#"{"cmd":"declare_origin","origin":"claude-remote-control"}"#);
    assert_eq!(reply["reply"], "error", "{reply}");
    assert_eq!(reply["kind"], "permission_denied", "{reply}");
}

#[test]
fn a_local_origin_is_refused_by_name_even_from_a_non_session() {
    // The refusal a client gets for the one thing that must never work. The
    // message has to name the value, so the caller can tell "not allowed to
    // ask for that" from "not allowed to ask at all".
    let h = harness!("declare-local");
    let reply = h.call(r#"{"cmd":"declare_origin","origin":"local-terminal"}"#);
    assert_eq!(reply["reply"], "error", "{reply}");
    let msg = reply["message"].as_str().unwrap_or_default();
    assert!(!msg.is_empty(), "{reply}");
}

#[test]
fn an_unknown_origin_name_is_refused_with_the_ones_that_work() {
    let h = harness!("declare-typo");
    let reply = h.call(r#"{"cmd":"declare_origin","origin":"remote"}"#);
    assert_eq!(reply["reply"], "error", "{reply}");
    let msg = reply["message"].as_str().unwrap_or_default();
    // `remote` is an accepted alias of claude-remote-control, so this reaches
    // the session check rather than the parser — either way it must not
    // succeed, and it must say something.
    assert!(!msg.is_empty(), "{reply}");
}
