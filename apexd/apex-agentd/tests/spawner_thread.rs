//! A session must outlive the connection that started it.
//!
//! `bwrap --die-with-parent` — which every confined session is launched with,
//! `apex_agent_core::sandbox::build_argv` — is `PR_SET_PDEATHSIG(SIGKILL)`,
//! and Linux delivers that signal when the THREAD that forked the process
//! exits, not when the parent process does (prctl(2): "the 'parent' in this
//! case is considered to be the thread that created this process"). The daemon
//! serves each connection on its own thread, and used to fork the session from
//! that thread, so a confined session was SIGKILLed the moment the client that
//! started it hung up — `apex agent run -d` returning was enough.
//!
//! It was a race, which is why it hid: if the connection thread exited BEFORE
//! bwrap reached its `prctl`, bwrap had already been reparented and the death
//! signal it then armed referred to a parent that never dies. On a developer
//! machine the client almost always won; on a loaded 4-vCPU CI runner it lost
//! often enough to fail `tests/test-secret-broker.sh` three times with one
//! transcript line and "killed by signal 9".
//!
//! This test takes the race out. The session arms the same death signal itself
//! with `setpriv --pdeathsig KILL` (no bwrap needed, so the test runs
//! everywhere), prints `armed`, and only THEN does the test close the
//! connection that started it. Against a daemon that forks on the connection
//! thread the session dies every time; against one that forks on a thread that
//! never exits, it prints `survived`.
//!
//! The daemon runs with its own `XDG_RUNTIME_DIR` and `XDG_STATE_HOME` and is
//! killed BY PID at the end — never by name.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use apex_agent_core::policy::AgentPolicy;
use apex_agent_core::protocol::{Request, RunRequest, SandboxPolicy};

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

fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

impl Harness {
    fn start() -> Harness {
        let root = std::env::temp_dir().join(format!(
            "apex-spawner-e2e-{}-{}",
            std::process::id(),
            now_ms()
        ));
        let runtime = root.join("run");
        let state = root.join("state");
        std::fs::create_dir_all(&runtime).expect("runtime dir");
        std::fs::create_dir_all(&state).expect("state dir");
        let child = Command::new(env!("CARGO_BIN_EXE_apex-agentd"))
            .env("XDG_RUNTIME_DIR", &runtime)
            .env("XDG_STATE_HOME", &state)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("starting apex-agentd");
        let socket = runtime.join("apex-agentd").join("control.sock");
        let h = Harness {
            child,
            socket,
            root,
        };
        let deadline = Instant::now() + Duration::from_secs(10);
        while UnixStream::connect(&h.socket).is_err() {
            assert!(Instant::now() < deadline, "apex-agentd never bound its socket");
            std::thread::sleep(Duration::from_millis(25));
        }
        h
    }

    fn connect(&self) -> UnixStream {
        let s = UnixStream::connect(&self.socket).expect("connect");
        s.set_read_timeout(Some(Duration::from_secs(10)))
            .expect("timeout");
        s
    }

    /// One request, one reply, on `stream` — which the caller keeps or drops.
    fn call_on(stream: &mut UnixStream, req: &Request) -> serde_json::Value {
        let line = serde_json::to_string(req).expect("serialise");
        writeln!(stream, "{line}").expect("write");
        stream.flush().ok();
        let mut reply = String::new();
        BufReader::new(&*stream)
            .read_line(&mut reply)
            .expect("read");
        serde_json::from_str(&reply).unwrap_or_else(|e| panic!("{e}: {reply}"))
    }

    fn call(&self, req: &Request) -> serde_json::Value {
        Self::call_on(&mut self.connect(), req)
    }

    fn logs(&self, id: u32) -> String {
        let reply = self.call(&Request::Logs {
            id,
            bytes: 64 * 1024,
        });
        reply["text"].as_str().unwrap_or_default().to_string()
    }

    fn logs_until(&self, id: u32, marker: &str, ms: u64) -> String {
        let deadline = Instant::now() + Duration::from_millis(ms);
        loop {
            let text = self.logs(id);
            if text.contains(marker) || Instant::now() >= deadline {
                return text;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

fn run_request(argv: Vec<String>) -> Request {
    Request::Run(RunRequest {
        agent: Some("generic".into()),
        prompt: None,
        args: argv,
        cwd: "/tmp".into(),
        policy: AgentPolicy {
            sandbox: SandboxPolicy::Unrestricted,
            ..AgentPolicy::default()
        },
        request_origin: None,
        worktree: None,
        checkpoint: false,
        ttl_ms: None,
        capabilities: None,
        allow: None,
        trust_ca: None,
        present: None,
        second_factor: None,
        cols: 80,
        rows: 24,
        env: vec![],
        disposable: false,
        copy_out: None,
    })
}

#[test]
fn a_session_with_a_parent_death_signal_survives_its_client_hanging_up() {
    // util-linux, essential on Fedora and Ubuntu alike. Required rather than
    // skipped: without it this test would assert nothing.
    let setpriv = ["/usr/bin/setpriv", "/bin/setpriv"]
        .into_iter()
        .find(|p| std::path::Path::new(p).exists())
        .expect("setpriv (util-linux) is needed to arm a parent-death signal");

    let h = Harness::start();

    // The connection the session is started on, held open until the session
    // has provably armed its death signal.
    let mut starter = h.connect();
    let reply = Harness::call_on(
        &mut starter,
        &run_request(vec![
            setpriv.into(),
            "--pdeathsig".into(),
            "KILL".into(),
            "--".into(),
            "/bin/sh".into(),
            "-c".into(),
            "echo armed; sleep 2; echo survived".into(),
        ]),
    );
    assert_eq!(reply["reply"], "session", "the daemon did not start a session: {reply}");
    let id = reply["id"]
        .as_u64()
        .unwrap_or_else(|| panic!("a session reply carrying no id: {reply}")) as u32;

    // `setpriv` sets PR_SET_PDEATHSIG before it execs sh, so `armed` in the
    // transcript proves the signal is armed — the ordering the race used to
    // decide is now fixed.
    let before = h.logs_until(id, "armed", 10_000);
    assert!(
        before.contains("armed"),
        "the session never armed its death signal: {before:?}"
    );

    // Hang up. The daemon's thread for this connection returns on EOF.
    drop(starter);

    let after = h.logs_until(id, "survived", 10_000);
    let status = h.call(&Request::Info { id });
    assert!(
        after.contains("survived"),
        "the session died when the connection that started it closed — it was \
         forked on the connection's thread, so its parent-death signal fired \
         when that thread exited.\n  transcript: {after:?}\n  status: {status}"
    );
}
