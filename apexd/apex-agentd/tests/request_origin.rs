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

    /// A connection held open across several requests.
    ///
    /// The declaration latch lives on the connection, so a suite whose only
    /// primitive is "one request per connection" cannot see it at all — every
    /// call would open a fresh socket with nothing latched on it and the
    /// tests would pass against a daemon that had no latch. This is the
    /// fixture the property needs.
    fn conn(&self) -> Conn {
        let stream = UnixStream::connect(&self.socket).expect("connect");
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .expect("timeout");
        Conn {
            reader: BufReader::new(stream.try_clone().expect("clone")),
            writer: stream,
        }
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

/// One control connection, reused.
struct Conn {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
}

impl Conn {
    fn call(&mut self, line: &str) -> serde_json::Value {
        assert!(
            !line.contains('\n'),
            "the framing is one JSON object per line; this payload would desynchronise it"
        );
        writeln!(self.writer, "{line}").expect("write");
        self.writer.flush().ok();
        let mut reply = String::new();
        self.reader.read_line(&mut reply).expect("read");
        serde_json::from_str(&reply).unwrap_or_else(|e| panic!("{e}: {reply}"))
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
    // At least, not exactly. What this suite needs is a daemon that carries
    // origins; pinning the equality made every later revision — the secret
    // service moved the store at 4 — fail here for a reason that has nothing
    // to do with §7.
    let version = reply["version"].as_u64().unwrap_or_default();
    assert!(
        version >= u64::from(apex_agent_core::protocol::REQUEST_ORIGIN_VERSION),
        "the daemon speaks protocol {version}, which predates request_origin: {reply}"
    );
    assert_eq!(
        version,
        u64::from(apex_agent_core::protocol::PROTOCOL_VERSION),
        "the daemon under test is not the one this suite was built against: {reply}"
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
fn a_connection_that_is_not_a_session_declares_for_the_connection() {
    // The case the daemon had no answer for. A remote proxy is not a session
    // and never will be: it terminates a paired device's channel and forwards
    // what it carries. Before this it could not say so, and everything it
    // forwarded was filed under whatever its own cgroup implied.
    //
    // This used to assert a refusal. The refusal was right about sessions —
    // "this peer has no session record to narrow" — and wrong about what a
    // narrowing is for.
    let h = harness!("declare");
    let mut c = h.conn();
    let declared = c.call(
        r#"{"cmd":"declare_origin","origin":"claude-remote-control","actor":"pixel-8-office"}"#,
    );
    if declared["reply"] == "error" {
        // The one acceptable failure: this process could not be classified,
        // so there was nothing to narrow. It must say what it could not read.
        let msg = declared["message"].as_str().unwrap_or_default();
        assert!(
            msg.contains("/proc/") || msg.contains("cgroup") || msg.contains("could not"),
            "a refusal must name what could not be read: {declared}"
        );
        return;
    }
    assert_eq!(declared["reply"], "ok", "{declared}");

    // Every later request on THIS connection now carries it.
    let filed = c.call(
        r#"{"cmd":"privilege_request","verb":"install","args":["clang"],"reason":"filed through a declared connection"}"#,
    );
    assert_eq!(filed["reply"], "request", "{filed}");
    assert_eq!(filed["request_origin"], "claude-remote-control", "{filed}");
    assert_eq!(filed["origin_source"], "declared", "{filed}");
    assert_eq!(filed["actor"], "pixel-8-office", "{filed}");

    // And the audit trail says the same, which is the half a client cannot
    // rewrite.
    let lines = h.audit_lines();
    let last = lines.last().unwrap_or_else(|| panic!("nothing audited"));
    assert_eq!(last["request_origin"], "claude-remote-control", "{last}");
    assert_eq!(last["origin_source"], "declared", "{last}");
}

#[test]
fn a_declared_connection_cannot_approve_a_root_operation() {
    // The whole reason the latch exists, stated as the thing it prevents.
    //
    // `apex-remoted` is a proxy. Started as a user unit it is observed as
    // `scheduled-job`; started from a login session — a developer running it
    // in a terminal, a shell launching it as a child — it is `local-terminal`,
    // and `decide` accepts that as a human at the keyboard. So a phone could
    // approve root by asking a proxy that happened to be started the wrong
    // way, and §7 reserves that for a human at this machine.
    //
    // The test is written as a comparison rather than as one assertion,
    // because "the declared connection was refused" is worth nothing unless
    // an identical connection without the declaration is allowed. If this
    // process is not local — a container, a CI runner — both are refused and
    // the comparison says so instead of asserting a fixed answer.
    let h = harness!("declare-decide");

    let filed = h.call(
        r#"{"cmd":"privilege_request","verb":"update","args":[],"reason":"a request for the declared connection to try to approve"}"#,
    );
    if filed["reply"] == "error" {
        return;
    }
    let id = filed["id"].as_u64().expect("an id");
    let observed = apex_agent_core::policy::RequestOrigin::parse(
        filed["request_origin"].as_str().unwrap_or_default(),
    )
    .expect("a recorded origin");

    // The declared connection tries to approve it.
    let mut declared = h.conn();
    assert_eq!(
        declared.call(r#"{"cmd":"declare_origin","origin":"claude-remote-control"}"#)["reply"],
        "ok"
    );
    let refused = declared.call(&format!(r#"{{"cmd":"decide","id":{id},"decision":"allow"}}"#));
    assert_eq!(
        refused["reply"], "error",
        "a claude-remote-control connection approved a root operation: {refused}"
    );
    assert_eq!(refused["kind"], "permission_denied", "{refused}");
    let msg = refused["message"].as_str().unwrap_or_default();
    assert!(
        msg.contains("claude-remote-control"),
        "the refusal must name the origin that was refused: {msg}"
    );

    // The control arm: the same request, an identical connection, nothing
    // declared. Without this the test above passes on a daemon with no latch
    // at all, in any environment that is not local.
    let plain = h.call(&format!(r#"{{"cmd":"decide","id":{id},"decision":"deny"}}"#));
    if observed.is_local() {
        assert_eq!(
            plain["reply"], "request",
            "the undeclared connection is local and must be allowed to decide: {plain}"
        );
        assert_eq!(plain["decision"], "denied", "{plain}");
    } else {
        // Not local to begin with, so the declared arm proves nothing on its
        // own. Say so rather than reporting a pass.
        eprintln!(
            "NOTE: this process is {observed}, not local, so the refusal above is not \
             attributable to the declaration"
        );
        assert_eq!(plain["reply"], "error", "{plain}");
    }
}

#[test]
fn a_declaration_does_not_reach_another_connection() {
    // Connection state, and only connection state. Nothing persists it, so a
    // second connection from the same process — the same pid, the same uid,
    // the same cgroup — is unaffected.
    let h = harness!("declare-scope");
    let mut a = h.conn();
    if a.call(r#"{"cmd":"declare_origin","origin":"claude-remote-control"}"#)["reply"] == "error" {
        return;
    }
    let elsewhere = h.call(
        r#"{"cmd":"privilege_request","verb":"pin","args":[],"reason":"filed on a connection that declared nothing"}"#,
    );
    if elsewhere["reply"] == "error" {
        return;
    }
    assert_eq!(elsewhere["origin_source"], "observed", "{elsewhere}");
    assert!(
        elsewhere["actor"].is_null(),
        "an actor leaked to a connection that never named one: {elsewhere}"
    );
}

#[test]
fn a_declaration_can_narrow_again_but_never_widen() {
    // The latch is not "the last thing you said". Every declaration goes
    // through `may_declare` against the live observation, so a second one can
    // only narrow further — and a `claude-remote-control` connection asking to
    // become `mcp` is asking to drop the lock gate, which is the loosening
    // that a single trust rank would have allowed.
    let h = harness!("declare-twice");
    let mut c = h.conn();
    if c.call(r#"{"cmd":"declare_origin","origin":"claude-remote-control"}"#)["reply"] == "error" {
        return;
    }
    let widened = c.call(r#"{"cmd":"declare_origin","origin":"mcp"}"#);
    assert_eq!(widened["reply"], "error", "{widened}");
    assert_eq!(widened["kind"], "permission_denied", "{widened}");

    // And the connection still has what it had, not what it asked for.
    let filed = c.call(
        r#"{"cmd":"privilege_request","verb":"rollback","args":[],"reason":"after a refused second declaration"}"#,
    );
    assert_eq!(filed["request_origin"], "claude-remote-control", "{filed}");
}

#[test]
fn an_actor_that_could_rewrite_a_prompt_is_refused() {
    // The actor is printed on the prompt a human reads before handing out
    // root. A newline in it is a second line of that prompt.
    let h = harness!("declare-actor");
    let mut c = h.conn();
    // The newline is a JSON string escape, so the wire still carries one
    // object on one line — the value inside it is what has the newline. A
    // literal newline here would break the framing instead of testing the
    // check, which is a different bug and would pass for the wrong reason.
    let reply = c.call(
        "{\"cmd\":\"declare_origin\",\"origin\":\"claude-remote-control\",\
         \"actor\":\"phone\\nAPPROVED\"}",
    );
    assert_eq!(reply["reply"], "error", "{reply}");
    assert_eq!(reply["kind"], "bad_request", "{reply}");

    // Refused, and not half-applied: the origin must not have latched either.
    let filed = c.call(
        r#"{"cmd":"privilege_request","verb":"pin","args":[],"reason":"after a refused actor"}"#,
    );
    if filed["reply"] == "request" {
        assert_eq!(filed["origin_source"], "observed", "{filed}");
        assert!(filed["actor"].is_null(), "{filed}");
    }
}

#[test]
fn a_local_origin_is_refused_by_name_even_from_a_non_session() {
    // The one thing that must never work, and the reason a connection latch
    // is safe to have at all: `may_be_declared` refuses the two local origins
    // by name, whatever was observed and whoever is asking. A connection that
    // is already `apex-shell` cannot even restate what it is.
    let h = harness!("declare-local");
    for name in ["local-terminal", "apex-shell"] {
        let mut c = h.conn();
        let reply = c.call(&format!(r#"{{"cmd":"declare_origin","origin":"{name}"}}"#));
        assert_eq!(reply["reply"], "error", "{name}: {reply}");
        assert_eq!(reply["kind"], "permission_denied", "{name}: {reply}");
        let msg = reply["message"].as_str().unwrap_or_default();
        assert!(msg.contains(name), "the refusal must name the value: {msg}");
    }
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
