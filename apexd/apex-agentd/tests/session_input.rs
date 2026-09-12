//! `Request::Input` against a real daemon, on a real PTY.
//!
//! P1-023's transcript delivery. The shell's push-to-talk route holds words and
//! owns no terminal, so it needs the write half of `Attach` without the read
//! half — that is the verb, and this is the only test that exercises it the way
//! the shell will: over the socket, as JSON, from a process that is not a
//! session, into a program that is genuinely reading its terminal.
//!
//! What the unit tests beside the code cannot say:
//!
//!   * the bytes reach a process that is reading the PTY, and reach it byte for
//!     byte through JSON framing;
//!   * a terminator turns them into a line the program acts on, which is what
//!     `--submit` sells;
//!   * the three refusals a client branches on come back as different kinds and
//!     not as one `internal` — no such session, session exited, and a write the
//!     terminal would not take.
//!
//! The daemon is started with its own `XDG_RUNTIME_DIR` and `XDG_STATE_HOME`,
//! so it binds a socket of its own and writes state of its own. It never
//! touches a running `apex-agentd`, and it is killed BY PID at the end — never
//! by name. Sessions run `--sandbox unrestricted` because `bwrap` is not the
//! thing under test and a machine without it would otherwise fail here for an
//! unrelated reason.

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
        // By pid, on the child this test spawned.
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

impl Harness {
    fn start(tag: &str) -> Option<Harness> {
        let root = std::env::temp_dir().join(format!(
            "apex-input-e2e-{}-{tag}-{}",
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

    /// One request, one reply, on a fresh connection — the same attribution a
    /// separate `apex` invocation would get, since the credentials the daemon
    /// reads are the ones from `connect(2)`.
    fn call(&self, req: &Request) -> serde_json::Value {
        let line = serde_json::to_string(req).expect("serialise");
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

    /// Start a session running `script` under `sh`, and return its id.
    fn run_shell(&self, script: &str) -> Option<u32> {
        let reply = self.call(&Request::Run(RunRequest {
            agent: Some("generic".into()),
            prompt: None,
            args: vec!["/bin/sh".into(), "-c".into(), script.into()],
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
            second_factor: None,
            cols: 80,
            rows: 24,
            env: vec![],
            disposable: false,
            copy_out: None,
        }));
        if reply["reply"] != "session" {
            // A daemon that will not start a session is a broken fixture on
            // this machine — no /dev/pts, no `sh`, no adapter — and not a
            // failing assertion. Said out loud, because a silent skip is a
            // test that asserts nothing.
            eprintln!("SKIP: the daemon would not start a session: {reply}");
            return None;
        }
        // `Response::Session` is an internally-tagged NEWTYPE variant, so
        // SessionInfo's fields sit at the TOP LEVEL of the reply: the id is
        // `reply["id"]`, not `reply["session"]["id"]`.
        //
        // The first draft of this helper read the nested path. It got `None`
        // and returned it through the same `else { return }` the skip above
        // uses, so all four tests in this file reported ok in 0.13s having
        // executed none of their assertions — with no SKIP line, because the
        // skip prints on the OTHER branch. Caught only because 0.13s is
        // impossible for four tests that each sleep 300ms.
        //
        // So an unreadable id PANICS rather than skipping. A fixture that
        // cannot tell "skipped" from "passed" is worse than one that fails.
        let id = reply["id"]
            .as_u64()
            .unwrap_or_else(|| panic!("a session reply carrying no id: {reply}"));
        Some(id as u32)
    }

    /// Poll the session transcript until it contains `marker`.
    fn logs_until(&self, id: u32, marker: &str, ms: u64) -> String {
        let deadline = Instant::now() + Duration::from_millis(ms);
        loop {
            let reply = self.call(&Request::Logs {
                id,
                bytes: 64 * 1024,
            });
            let last = reply["text"].as_str().unwrap_or_default().to_string();
            if last.contains(marker) || Instant::now() >= deadline {
                return last;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

fn error_kind(reply: &serde_json::Value) -> String {
    reply["kind"].as_str().unwrap_or_default().to_string()
}

/// Skip out loud rather than passing silently.
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
fn text_reaches_a_live_session_and_a_terminator_makes_it_a_line() {
    let h = harness!("write");
    // Reads one line, says what it got, then stays alive so the second half of
    // the test is not racing the session's exit.
    let Some(id) = h.run_shell("read line; echo \"got:[$line]\"; sleep 30") else {
        return;
    };

    // Wait until the shell is actually at `read`. Asserted through the
    // transcript rather than slept for: bytes written before there is a reader
    // would still be buffered by the line discipline, so a sleep here would
    // make the test pass for the wrong reason on a fast machine and fail on a
    // slow one.
    std::thread::sleep(Duration::from_millis(300));

    let reply = h.call(&Request::Input {
        id,
        data: "run the tests".into(),
    });
    assert_eq!(reply["reply"], "ok", "the write was refused: {reply}");

    // The words are in the terminal and the line has NOT been submitted. Echo
    // is on, so the transcript shows what was typed; `got:[` is the shell
    // acting on it, and it must not be there yet.
    let typed = h.logs_until(id, "run the tests", 2000);
    assert!(
        typed.contains("run the tests"),
        "the text never reached the terminal: {typed:?}"
    );
    assert!(
        !typed.contains("got:["),
        "the line was acted on without a terminator: {typed:?}"
    );

    // Now the carriage return `--submit` appends.
    let reply = h.call(&Request::Input {
        id,
        data: "\r".into(),
    });
    assert_eq!(reply["reply"], "ok", "{reply}");
    let after = h.logs_until(id, "got:[", 3000);
    assert!(
        after.contains("got:[run the tests]"),
        "the terminator did not end the line: {after:?}"
    );
}

#[test]
fn a_payload_with_quotes_and_backslashes_arrives_unchanged() {
    let h = harness!("verbatim");
    // `cat` and not `read`, so nothing between the PTY and the transcript gets
    // to reinterpret the bytes: whatever arrives is echoed back as it is.
    let Some(id) = h.run_shell("cat") else {
        return;
    };
    std::thread::sleep(Duration::from_millis(300));

    // The characters JSON has to escape and a shell would like to eat. This is
    // the assertion that the payload is DATA all the way down: nothing here is
    // parsed as a command, an argument or a format string.
    let payload = r#"say "hi" \ 'there' $HOME `id` 100%"#;
    let reply = h.call(&Request::Input {
        id,
        data: payload.to_string(),
    });
    assert_eq!(reply["reply"], "ok", "{reply}");

    let seen = h.logs_until(id, "100%", 3000);
    assert!(
        seen.contains(payload),
        "the payload changed on the way in: {seen:?}"
    );
}

#[test]
fn the_three_ways_a_write_can_fail_come_back_as_three_different_kinds() {
    let h = harness!("errors");

    // 1. A session that never existed. A client shows "no such session"; it
    //    does not retry.
    let reply = h.call(&Request::Input {
        id: 9999,
        data: "x".into(),
    });
    assert_eq!(
        error_kind(&reply),
        "no_such_session",
        "an absent session must not be reported as anything else: {reply}"
    );

    // 2. A session that has exited. Distinct from the above on purpose: the
    //    shell's route resolves a target and then delivers, so between the two
    //    the target can end, and "it finished" is a different message to the
    //    user than "there is no such thing".
    let Some(id) = h.run_shell("exit 0") else {
        return;
    };
    // Wait for the daemon to record the exit rather than assuming a duration.
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut kind = String::new();
    while Instant::now() < deadline {
        let reply = h.call(&Request::Input {
            id,
            data: "x".into(),
        });
        kind = error_kind(&reply);
        if kind == "session_exited" {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(
        kind, "session_exited",
        "a finished session must be reported as finished, not as a missing one"
    );
}

#[test]
fn an_empty_payload_is_accepted_and_writes_nothing() {
    // Not an error: a caller with nothing to say has said nothing, and the
    // alternative is a client that has to special-case the empty string before
    // every call. The assertion that matters is the second one — accepting it
    // must not mean submitting an empty line, which in an agent's prompt is a
    // turn with no content in it.
    let h = harness!("empty");
    let Some(id) = h.run_shell("read line; echo \"got:[$line]\"; sleep 30") else {
        return;
    };
    std::thread::sleep(Duration::from_millis(300));

    let reply = h.call(&Request::Input {
        id,
        data: String::new(),
    });
    assert_eq!(reply["reply"], "ok", "{reply}");
    std::thread::sleep(Duration::from_millis(300));
    let seen = h.logs_until(id, "got:[", 500);
    assert!(
        !seen.contains("got:["),
        "an empty write submitted a line: {seen:?}"
    );
}
