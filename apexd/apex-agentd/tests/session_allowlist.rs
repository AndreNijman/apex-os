//! A session reaches fewer destinations than the runtime allows (P2-012).
//!
//! `RunRequest::allow` against a real daemon, over the socket, as JSON.
//!
//! ## What the unit tests beside the code cannot say
//!
//! `session_allowlist` is a pure function and is tested as one, and
//! `Allowlist::narrow` is tested harder still. Neither of them can say what
//! this file is for: that the narrowed list is the one the DAEMON ends up
//! using, and that what a user is shown is that same list.
//!
//! The failure those unit tests cannot see is a `start` that computes the
//! narrowing correctly and then hands the runtime's list to `egress::start`
//! anyway, or reports the request's list rather than the enforced one. Both
//! are one-binding mistakes in a four-hundred-line function, both leave every
//! unit test green, and both are the whole of the security property: a capsule
//! started to visit one host reaching every destination the machine permits,
//! with `apex agent status` saying otherwise.
//!
//! So the assertions here are about a session that actually started:
//!
//!   * a narrowed session reports the destinations it NAMED, and — the half
//!     that is the boundary rather than the bookkeeping — NOT the destination
//!     the runtime allows and it did not name;
//!   * an unnarrowed session reports the runtime's whole list, which is what
//!     every allowlisted session got before this field existed;
//!   * a widening is refused, as `PolicyRefused` and not as a bad request,
//!     with the `apex agent allow` line that would permit it;
//!   * a narrowing on a session with no allowlist to narrow is refused rather
//!     than accepted and ignored.
//!
//! The last two need no session and run everywhere. The first two need an
//! allowlisted session, which needs `bwrap` and a runtime binary a confined
//! session can reach; where that is not true the test SAYS SO and skips,
//! because a silent skip is a test that asserts nothing.
//!
//! ## What it does not touch
//!
//! The daemon is started with its own `XDG_RUNTIME_DIR`, `XDG_STATE_HOME` and
//! `XDG_CONFIG_HOME`, so it binds its own socket, writes its own state and
//! reads its own `agent.json` — never the developer's, whose `network_allow`
//! would otherwise decide what this file measures. It is killed BY PID. No
//! destination in this file resolves to anything: nothing here opens a
//! connection, and the assertions are about the policy the daemon recorded.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime};

use apex_agent_core::policy::{AgentPolicy, NetworkPolicy};
use apex_agent_core::protocol::{Request, RunRequest, SandboxPolicy};

/// The runtime's own allowlist for every case here.
///
/// Two destinations, because one would make "the session got what it asked
/// for" and "the session got the runtime's list" the same observation — which
/// is exactly the defect this file exists to catch.
const RUNTIME_ALLOW: [&str; 2] = ["api.example.com", "files.example.com"];

fn config() -> String {
    format!(
        r#"{{"network_allow":["{}","{}"]}}"#,
        RUNTIME_ALLOW[0], RUNTIME_ALLOW[1]
    )
}

struct Harness {
    child: Child,
    socket: PathBuf,
    root: PathBuf,
}

impl Drop for Harness {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

impl Harness {
    fn start(tag: &str) -> Option<Harness> {
        let root = std::env::temp_dir().join(format!(
            "apex-allowlist-e2e-{}-{tag}-{}",
            std::process::id(),
            now_ms()
        ));
        let runtime = root.join("run");
        let state = root.join("state");
        let config_home = root.join("config");
        let work = root.join("work");
        std::fs::create_dir_all(&runtime).ok()?;
        std::fs::create_dir_all(&state).ok()?;
        std::fs::create_dir_all(&work).ok()?;
        std::fs::create_dir_all(config_home.join("apex")).ok()?;
        std::fs::write(config_home.join("apex/agent.json"), config()).ok()?;

        let child = Command::new(env!("CARGO_BIN_EXE_apex-agentd"))
            .env("XDG_RUNTIME_DIR", &runtime)
            .env("XDG_STATE_HOME", &state)
            .env("XDG_CONFIG_HOME", &config_home)
            .env(
                apex_agent_core::paths::SCRATCH_ROOT_ENV,
                root.join("scratch"),
            )
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

    fn call(&self, req: &Request) -> serde_json::Value {
        let line = serde_json::to_string(req).expect("serialise");
        assert!(!line.contains('\n'), "one JSON object per line");
        let mut stream = UnixStream::connect(&self.socket).expect("connect");
        stream
            .set_read_timeout(Some(Duration::from_secs(20)))
            .expect("timeout");
        writeln!(stream, "{line}").expect("write");
        stream.flush().ok();
        let mut reply = String::new();
        BufReader::new(&stream).read_line(&mut reply).expect("read");
        serde_json::from_str(&reply).unwrap_or_else(|e| panic!("{e}: {reply}"))
    }

    /// Ask for a session that sits still, with the network mode and the
    /// narrowing under test.
    fn run(&self, network: NetworkPolicy, allow: Option<Vec<String>>) -> serde_json::Value {
        let sandbox = match network {
            // `allowlist` is refused outright on an unconfined session — the
            // proxy is the namespace's only route out — so the confined
            // sandbox is not an extra here, it is the mode.
            NetworkPolicy::Allowlist => SandboxPolicy::Project,
            _ => SandboxPolicy::Unrestricted,
        };
        self.call(&Request::Run(RunRequest {
            agent: Some("generic".into()),
            prompt: None,
            args: vec!["/bin/sh".into(), "-c".into(), "sleep 5".into()],
            cwd: self.root.join("work").to_string_lossy().into_owned(),
            policy: AgentPolicy {
                sandbox,
                network,
                ..AgentPolicy::default()
            },
            request_origin: None,
            worktree: None,
            checkpoint: false,
            ttl_ms: None,
            capabilities: None,
            allow,
            second_factor: None,
            cols: 80,
            rows: 24,
            env: vec![],
            disposable: false,
            copy_out: None,
        }))
    }
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// The destinations a `session` reply says the session may reach.
///
/// Top level, not nested: `SessionInfo` is `#[serde(flatten)]`ed into the
/// reply, so `allowlist` sits beside `sandbox` and `network` exactly as the
/// dimensions do.
fn destinations(reply: &serde_json::Value) -> Vec<String> {
    reply["allowlist"]
        .as_array()
        .unwrap_or_else(|| panic!("no allowlist in the session reply: {reply}"))
        .iter()
        .map(|v| v.as_str().expect("a destination is a string").to_string())
        .collect()
}

macro_rules! harness {
    ($tag:literal) => {
        match Harness::start($tag) {
            Some(h) => h,
            None => {
                eprintln!("SKIP: no daemon could be started for `{}`", $tag);
                return;
            }
        }
    };
}

/// A session that asked for an allowlisted one and did not get it, with the
/// reason — so a machine that cannot run this half says which part is missing
/// rather than failing an assertion about the narrowing.
fn skip_unless_started(reply: &serde_json::Value, what: &str) -> bool {
    if reply["reply"] == "session" {
        return true;
    }
    eprintln!(
        "SKIP: this machine could not start an allowlisted session ({what}): {}",
        reply["message"].as_str().unwrap_or("no message")
    );
    false
}

#[test]
fn a_narrowed_session_runs_under_its_own_allowlist_and_not_the_runtimes() {
    let h = harness!("narrowed");
    let reply = h.run(
        NetworkPolicy::Allowlist,
        Some(vec![RUNTIME_ALLOW[0].to_string()]),
    );
    if !skip_unless_started(&reply, "narrowed") {
        return;
    }
    let got = destinations(&reply);
    assert_eq!(
        got,
        vec![RUNTIME_ALLOW[0]],
        "the session reports destinations it did not name"
    );
    // THE HALF THAT IS THE BOUNDARY. `assert_eq!` above would hold for a
    // daemon that reported the request back verbatim while confining the
    // session to the runtime's list; this says the destination the RUNTIME
    // allows and this session never named is not among them.
    assert!(
        !got.iter().any(|d| d == RUNTIME_ALLOW[1]),
        "a narrowed session was given a destination the machine permits and it did not \
         ask for: {got:?}"
    );
}

#[test]
fn a_session_that_narrows_nothing_still_gets_the_whole_runtime_allowlist() {
    // The control for the test above, and a regression guard in its own
    // right: the narrowing must not change what an ordinary allowlisted
    // session gets. Without this pair, a `start` that gave every session an
    // empty allowlist would pass the first test's second assertion.
    let h = harness!("unnarrowed");
    let reply = h.run(NetworkPolicy::Allowlist, None);
    if !skip_unless_started(&reply, "unnarrowed") {
        return;
    }
    let mut got = destinations(&reply);
    got.sort();
    let mut want: Vec<String> = RUNTIME_ALLOW.iter().map(|s| s.to_string()).collect();
    want.sort();
    assert_eq!(got, want);
}

#[test]
fn a_session_cannot_widen_the_allowlist_by_naming_a_destination() {
    // No session is started for this one — the refusal happens before
    // anything is built — so it runs on every machine.
    let h = harness!("widen");
    let reply = h.run(
        NetworkPolicy::Allowlist,
        Some(vec!["evil.example.com".to_string()]),
    );
    assert_eq!(reply["reply"], "error", "a widening was accepted: {reply}");
    // The KIND, not just the refusal. `BadRequest` would send the user to
    // look at their own command line; the remedy for this is on the machine.
    assert_eq!(
        reply["kind"], "policy_refused",
        "a refused narrowing is a policy answer: {reply}"
    );
    let message = reply["message"].as_str().unwrap_or_default();
    assert!(message.contains("evil.example.com"), "{message}");
    assert!(message.contains("apex agent allow"), "{message}");
    // And it says what the runtime DOES allow, so the caller can see whether
    // they mistyped a destination that is on the list.
    assert!(message.contains(RUNTIME_ALLOW[0]), "{message}");
}

#[test]
fn naming_destinations_for_a_session_with_no_allowlist_is_refused_not_ignored() {
    let h = harness!("no-allowlist");
    let reply = h.run(
        NetworkPolicy::Open,
        Some(vec![RUNTIME_ALLOW[0].to_string()]),
    );
    assert_eq!(
        reply["reply"], "error",
        "an `open` session accepted a list of destinations it will never be held to: {reply}"
    );
    let message = reply["message"].as_str().unwrap_or_default();
    assert!(message.contains("open"), "{message}");
    assert!(message.contains("--network allowlist"), "{message}");
}
