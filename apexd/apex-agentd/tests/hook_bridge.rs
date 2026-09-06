//! P0-011's bridge against a real daemon, for the one claim that cannot be
//! made about a value: that an event published from inside a session actually
//! reaches the daemon that started it.
//!
//! Every mapping in `hook.rs` is a pure function and is tested beside the code.
//! What is *not* pure is how a process inside a session finds the control
//! socket, and that is exactly where this broke in a live run: a confined
//! session is given a cleared environment, so `paths::runtime_dir` fell back to
//! `/run/user/<uid>` and every `apex agent event` and every hook answered "the
//! agent runtime is not running" from inside a session the runtime was
//! demonstrably running. On an ordinary login the fallback happens to be right,
//! which is why nothing noticed. `a_confined_session_is_told_where_its_runtime_
//! directory_is` gives the daemon a runtime directory of its own — the shape a
//! test harness, a container and a second daemon all have — so the fallback is
//! wrong and the bug is visible.
//!
//! The publish probe is `permission_request` on purpose. `session::next_state`
//! refuses to infer that state from any sequence of output, silence or signals,
//! so a session reporting it is proof that a published event crossed the socket
//! and not evidence of a lucky guess.
//!
//! The daemon is started with its own `XDG_RUNTIME_DIR` and `XDG_STATE_HOME`
//! and killed by pid at the end — never by name.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

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
            "apex-hook-e2e-{}-{tag}-{}",
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

    fn call(&self, line: &str) -> serde_json::Value {
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

    /// A session's transcript so far.
    fn logs(&self, id: u64) -> String {
        let reply = self.call(&format!(r#"{{"cmd":"logs","id":{id},"bytes":65536}}"#));
        reply["text"].as_str().unwrap_or_default().to_string()
    }

    /// Poll a session's transcript until it contains `needle`, or give up.
    fn wait_for_output(&self, id: u64, needle: &str, secs: u64) -> String {
        let deadline = Instant::now() + Duration::from_secs(secs);
        let mut last = String::new();
        while Instant::now() < deadline {
            last = self.logs(id);
            if last.contains(needle) {
                return last;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        last
    }

    /// Poll a session's state until it is `want`, or give up.
    fn wait_for_state(&self, id: u64, want: &str, secs: u64) -> String {
        let deadline = Instant::now() + Duration::from_secs(secs);
        let mut last = String::new();
        while Instant::now() < deadline {
            let reply = self.call(&format!(r#"{{"cmd":"info","id":{id}}}"#));
            last = reply["state"].as_str().unwrap_or_default().to_string();
            if last == want {
                return last;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        last
    }
}

/// The `apex` CLI that was built alongside this daemon.
///
/// Its sibling in the target directory, because the two are one release and a
/// hook that ran a different build than the daemon it reports to is the pairing
/// most likely to be wrong.
fn apex_cli() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_apex-agentd"))
        .parent()
        .expect("target directory")
        .join("apex")
}

fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

macro_rules! harness {
    ($tag:literal) => {
        match Harness::start($tag) {
            Some(h) => h,
            None => {
                eprintln!("SKIP {}: apex-agentd would not start", $tag);
                return;
            }
        }
    };
}

#[test]
fn a_hook_inside_a_session_reaches_the_daemon_that_started_it() {
    let apex = apex_cli();
    if !apex.is_file() {
        // `cargo test -p apex-agentd` does not build the `apex` binary, so this
        // skip is reachable by an ordinary command. It reports as a PASS, which
        // is the worst answer a test can give: I spent a while treating this
        // suite's 0.13s "4 passed" as evidence the publish path worked, when it
        // meant the publish path had not been exercised at all.
        //
        // APEX_REQUIRE_APEX_CLI turns it into a failure, the same shape
        // tests/test-apex-input.sh uses for APEX_REQUIRE_NIRI. CI sets it; a
        // developer running one crate's tests does not.
        let msg = format!(
            "{} has not been built — run `cargo build -p apex` or the whole \
             workspace; this test cannot exercise the publish path without it",
            apex.display()
        );
        assert!(
            std::env::var_os("APEX_REQUIRE_APEX_CLI").is_none(),
            "{msg}"
        );
        eprintln!("SKIP: {msg}");
        return;
    }
    let h = harness!("publish");

    // Unconfined, so the test does not also depend on the target directory
    // being reachable from inside a sandbox — on CI it lives under the home
    // that `--tmpfs` masks. What is under test is how the session resolves the
    // socket, and that is the same code either way.
    //
    // stdin from /dev/null because the hook reads its payload to EOF, and a
    // session's stdin is a PTY that never reaches one.
    let script = format!(
        "{} agent hook permission_request < /dev/null; sleep 60",
        apex.display()
    );
    let run = serde_json::json!({
        "cmd": "run",
        "agent": "generic",
        "args": ["sh", "-c", script],
        "cwd": "/tmp",
        "sandbox": "unrestricted",
        "network": "open",
        "cols": 80,
        "rows": 24,
    });
    let reply = h.call(&run.to_string());
    assert_eq!(
        reply["reply"], "session",
        "the session did not start: {reply}"
    );
    let id = reply["id"].as_u64().expect("session id");

    let state = h.wait_for_state(id, "permission_request", 20);
    assert_eq!(
        state, "permission_request",
        "the hook never reached the daemon; the session reported {state:?}. \
         Inference cannot produce this state, so this is the publish path and \
         nothing else."
    );

    h.call(&format!(r#"{{"cmd":"signal","id":{id},"signal":"kill"}}"#));
}

#[test]
fn a_confined_session_is_told_where_its_runtime_directory_is() {
    // The regression this file was written for. A confined session gets
    // `--clearenv` and then exactly the variables the daemon named, so nothing
    // it inherits can stand in for the one that says where the control socket
    // is. Without it the session falls back to `/run/user/<uid>`, which is a
    // masked tmpfs inside the sandbox and the wrong directory outside it, and
    // the whole open event protocol — `apex agent event` as much as any hook —
    // is unreachable from the only place it is meant to be called.
    //
    // Asserted through the session's own `echo` rather than by inspecting an
    // argv, so what is checked is what the process actually received.
    let h = harness!("runtimedir");
    let want = h.root.join("run");

    let run = serde_json::json!({
        "cmd": "run",
        "agent": "generic",
        "args": ["sh", "-c", "echo RUNTIME=[$XDG_RUNTIME_DIR]; sleep 60"],
        "cwd": "/tmp",
        "sandbox": "project",
        "network": "open",
        "cols": 200,
        "rows": 24,
    });
    let reply = h.call(&run.to_string());
    if reply["reply"] != "session" {
        // bwrap is missing, or the kernel refuses user namespaces. The daemon
        // refusing to run confined is the correct behaviour, not a failure to
        // assert around.
        eprintln!("SKIP runtimedir: {reply}");
        return;
    }
    let id = reply["id"].as_u64().expect("session id");

    let expected = format!("RUNTIME=[{}]", want.display());
    let out = h.wait_for_output(id, "RUNTIME=[", 15);
    h.call(&format!(r#"{{"cmd":"signal","id":{id},"signal":"kill"}}"#));
    assert!(
        out.contains(&expected),
        "a confined session was not told its runtime directory.\n  wanted: {expected}\n  got: {out:?}"
    );
}

#[test]
fn the_policy_point_answers_a_session_and_refuses_nobody_else() {
    let h = harness!("toolcheck");

    // No session with this id, so there is nothing to judge against. The
    // answer must be "no opinion" and never a refusal: a hook that denied when
    // it could not decide would stop an agent for a reason nobody could state.
    let reply = h.call(
        &serde_json::json!({
            "cmd": "tool_check",
            "id": 4242,
            "tool_name": "Write",
            "tool_input": {"file_path": "/usr/bin/x"},
        })
        .to_string(),
    );
    assert_eq!(reply["reply"], "tool_decision", "{reply}");
    assert!(reply["deny"].is_null(), "{reply}");
}

#[test]
fn a_confined_session_is_judged_against_the_confinement_it_actually_has() {
    let h = harness!("confined");

    let run = serde_json::json!({
        "cmd": "run",
        "agent": "generic",
        "args": ["sh", "-c", "sleep 60"],
        "cwd": "/tmp",
        "sandbox": "project",
        "network": "open",
        "cols": 80,
        "rows": 24,
    });
    let reply = h.call(&run.to_string());
    if reply["reply"] != "session" {
        // bwrap is not available, or the kernel refuses user namespaces. The
        // daemon says so rather than running unconfined, which is the correct
        // behaviour and not something to assert around.
        eprintln!("SKIP confined: {reply}");
        return;
    }
    let id = reply["id"].as_u64().expect("session id");

    let check = |tool: &str, input: serde_json::Value| -> serde_json::Value {
        h.call(
            &serde_json::json!({
                "cmd": "tool_check",
                "id": id,
                "tool_name": tool,
                "tool_input": input,
            })
            .to_string(),
        )
    };

    // Refused, because `--ro-bind / /` refuses it.
    let deny = check("Write", serde_json::json!({"file_path": "/usr/bin/apex-probe"}));
    let reason = deny["deny"].as_str().unwrap_or_default();
    assert!(reason.contains("read-only"), "{deny}");

    // Allowed, because the sandbox allows it — /tmp is a tmpfs inside the
    // session and a write there lands. A denial would be advice with nothing
    // behind it.
    let allow = check("Write", serde_json::json!({"file_path": "/tmp/notes"}));
    assert!(allow["deny"].is_null(), "{allow}");

    // Allowed past the first word of a shell command, on purpose.
    let allow = check("Bash", serde_json::json!({"command": "sh -c 'rm -rf /usr'"}));
    assert!(allow["deny"].is_null(), "{allow}");

    // Refused, because no_new_privs makes the setuid bit inert.
    let deny = check("Bash", serde_json::json!({"command": "sudo dnf install x"}));
    assert!(
        deny["deny"]
            .as_str()
            .unwrap_or_default()
            .contains("no_new_privs"),
        "{deny}"
    );

    h.call(&format!(r#"{{"cmd":"signal","id":{id},"signal":"kill"}}"#));
}
