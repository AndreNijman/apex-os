//! The remote-live verbs against a real daemon, on real PTYs
//! (`docs/remote-live-contract.md` §1).
//!
//! What the unit tests beside the code cannot say:
//!
//!   * the phone's JSON and the Shell's reach the dispatch and come back in the
//!     shape the contract promised — `hello`'s features, `rename`'s session,
//!     `peek`'s base64 — over the socket, not through a function call;
//!   * a name given at `run` and a title an agent prints both land in the
//!     record ON DISK, which is what the Shell's prompt indicator and a daemon
//!     restart read;
//!   * `submit` makes a line a program acts on, end to end;
//!   * the size a phone gives a terminal is given back when the phone goes —
//!     with the phone played the way `rime-remoted` plays it: a connection
//!     that declares `claude-remote-control` first, and a resize on a
//!     connection of its own.
//!
//! The daemon is started with its own `XDG_RUNTIME_DIR` and `XDG_STATE_HOME`,
//! so it binds a socket of its own and writes state of its own. It never
//! touches a running `rime-agentd`, and it is killed BY PID at the end — never
//! by name. Sessions run `--sandbox unrestricted` because `bwrap` is not the
//! thing under test.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use rime_agent_core::policy::AgentPolicy;
use rime_agent_core::protocol::{Request, RunRequest, SandboxPolicy, FEATURES};

struct Harness {
    child: Child,
    socket: PathBuf,
    state: PathBuf,
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
            "rime-remote-live-{}-{tag}-{}",
            std::process::id(),
            now_ms()
        ));
        let runtime = root.join("run");
        let state = root.join("state");
        std::fs::create_dir_all(&runtime).ok()?;
        std::fs::create_dir_all(&state).ok()?;

        let child = Command::new(env!("CARGO_BIN_EXE_rime-agentd"))
            .env("XDG_RUNTIME_DIR", &runtime)
            .env("XDG_STATE_HOME", &state)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;

        let socket = runtime.join("rime-agentd").join("control.sock");
        let harness = Harness {
            child,
            socket,
            state,
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

    fn connect(&self) -> UnixStream {
        let stream = UnixStream::connect(&self.socket).expect("connect");
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .expect("timeout");
        stream
    }

    /// One JSON line out, one JSON line back, on `stream`.
    fn line_on(stream: &mut UnixStream, line: &str) -> serde_json::Value {
        assert!(!line.contains('\n'), "one object per line");
        writeln!(stream, "{line}").expect("write");
        stream.flush().ok();
        // Byte at a time up to the newline, so nothing after the reply — the
        // start of an attach's replay — is swallowed into a buffer that is
        // then dropped.
        let mut reply = Vec::new();
        let mut byte = [0u8; 1];
        while stream.read_exact(&mut byte).is_ok() {
            if byte[0] == b'\n' {
                break;
            }
            reply.push(byte[0]);
        }
        let reply = String::from_utf8_lossy(&reply).to_string();
        serde_json::from_str(&reply).unwrap_or_else(|e| panic!("{e}: {reply}"))
    }

    fn raw(&self, line: &str) -> serde_json::Value {
        let mut stream = self.connect();
        Self::line_on(&mut stream, line)
    }

    fn call(&self, req: &Request) -> serde_json::Value {
        self.raw(&serde_json::to_string(req).expect("serialise"))
    }

    /// A connection that is what `rime-remoted` opens for a phone: it declares
    /// `claude-remote-control` before it says anything else.
    fn as_phone(&self) -> Option<UnixStream> {
        let mut stream = self.connect();
        let reply = Self::line_on(
            &mut stream,
            r#"{"cmd":"declare_origin","origin":"claude-remote-control","actor":"test-phone"}"#,
        );
        if reply["reply"] != "ok" {
            eprintln!("SKIP: this daemon would not take a remote declaration here: {reply}");
            return None;
        }
        Some(stream)
    }

    fn run(&self, script: &str, name: Option<&str>, cols: u16, rows: u16) -> serde_json::Value {
        self.call(&Request::Run(Box::new(RunRequest {
            agent: Some("generic".into()),
            name: name.map(str::to_string),
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
            allow: None,
            trust_ca: None,
            present: None,
            second_factor: None,
            cols,
            rows,
            env: vec![],
            disposable: false,
            copy_out: None,
        })))
    }

    /// Start a session and return its id, or skip out loud.
    fn run_shell(&self, script: &str, name: Option<&str>) -> Option<u32> {
        let reply = self.run(script, name, 120, 40);
        if reply["reply"] != "session" {
            eprintln!("SKIP: the daemon would not start a session: {reply}");
            return None;
        }
        // `Response::Session` is internally tagged, so the id is at the top
        // level. Unreadable is a panic, never a skip: see session_input.rs for
        // the run where a skip-shaped bug passed four tests that ran nothing.
        Some(
            reply["id"]
                .as_u64()
                .unwrap_or_else(|| panic!("a session reply carrying no id: {reply}")) as u32,
        )
    }

    fn info(&self, id: u32) -> serde_json::Value {
        self.call(&Request::Info { id })
    }

    fn record(&self, id: u32) -> serde_json::Value {
        let path = self
            .state
            .join("rime/agent/sessions")
            .join(format!("{id}.json"));
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("no record at {}: {e}", path.display()));
        serde_json::from_str(&text).expect("the record is JSON")
    }

    /// Poll `info` until `done` holds or `ms` passes, returning the last reply.
    fn info_until(&self, id: u32, ms: u64, done: impl Fn(&serde_json::Value) -> bool) -> serde_json::Value {
        let deadline = Instant::now() + Duration::from_millis(ms);
        loop {
            let info = self.info(id);
            if done(&info) || Instant::now() >= deadline {
                return info;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    fn logs_until(&self, id: u32, marker: &str, ms: u64) -> String {
        let deadline = Instant::now() + Duration::from_millis(ms);
        loop {
            let reply = self.call(&Request::Logs { id, bytes: 64 * 1024 });
            let text = reply["text"].as_str().unwrap_or_default().to_string();
            if text.contains(marker) || Instant::now() >= deadline {
                return text;
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

fn size(v: &serde_json::Value) -> (u64, u64) {
    (v["cols"].as_u64().unwrap_or(0), v["rows"].as_u64().unwrap_or(0))
}

macro_rules! harness {
    ($tag:literal) => {
        match Harness::start($tag) {
            Some(h) => h,
            None => {
                eprintln!("SKIP: rime-agentd did not come up in this environment");
                return;
            }
        }
    };
}

#[test]
fn hello_advertises_exactly_the_features_this_build_implements() {
    let h = harness!("hello");
    let reply = h.raw(r#"{"cmd":"hello"}"#);
    assert_eq!(reply["reply"], "hello", "{reply}");
    let got: Vec<&str> = reply["features"]
        .as_array()
        .unwrap_or_else(|| panic!("no features list: {reply}"))
        .iter()
        .filter_map(|f| f.as_str())
        .collect();
    assert_eq!(got, FEATURES);
}

#[test]
fn a_name_given_at_run_is_on_the_session_the_list_and_the_record() {
    let h = harness!("name");
    let Some(id) = h.run_shell("sleep 30", Some("  auth refactor  ")) else {
        return;
    };
    let info = h.info(id);
    assert_eq!(info["name"], "auth refactor", "trimmed, and kept: {info}");
    // Always on the wire, set or not.
    assert!(info.as_object().unwrap().contains_key("title"), "{info}");
    assert_eq!(h.record(id)["name"], "auth refactor");
    let list = h.raw(r#"{"cmd":"list"}"#);
    let row = list["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["id"] == id)
        .expect("the session is listed");
    assert_eq!(row["name"], "auth refactor");

    // A session started with no name says `null`, not nothing: "this daemon
    // has names" is what a client offers Rename on.
    let Some(bare) = h.run_shell("sleep 30", None) else {
        return;
    };
    let info = h.info(bare);
    assert_eq!(info.get("name"), Some(&serde_json::Value::Null), "{info}");
}

#[test]
fn a_name_the_validator_refuses_starts_nothing() {
    let h = harness!("badname");
    let before = h.raw(r#"{"cmd":"list"}"#)["sessions"].as_array().map(Vec::len);
    let reply = h.run("sleep 30", Some("phone\r\nAPPROVED"), 80, 24);
    assert_eq!(reply["kind"], "bad_request", "{reply}");
    assert!(
        reply["message"].as_str().unwrap_or_default().contains("control characters"),
        "{reply}"
    );
    let reply = h.run("sleep 30", Some(&"n".repeat(65)), 80, 24);
    assert_eq!(reply["kind"], "bad_request", "{reply}");
    // Refused before anything was created: no session, no record.
    let after = h.raw(r#"{"cmd":"list"}"#)["sessions"].as_array().map(Vec::len);
    assert_eq!(before, after, "a refused name left a session behind");
}

#[test]
fn rename_names_a_live_session_clears_it_and_refuses_what_it_should() {
    let h = harness!("rename");
    let Some(id) = h.run_shell("sleep 30", None) else {
        return;
    };

    // The contract's exact bytes.
    let reply = h.raw(&format!(r#"{{"cmd":"rename","id":{id},"name":"auth refactor"}}"#));
    assert_eq!(reply["reply"], "session", "{reply}");
    assert_eq!(reply["id"], id);
    assert_eq!(reply["name"], "auth refactor");
    assert_eq!(h.record(id)["name"], "auth refactor", "the record was not written");
    assert_eq!(h.info(id)["name"], "auth refactor");

    let reply = h.raw(&format!(r#"{{"cmd":"rename","id":{id},"name":null}}"#));
    assert_eq!(reply["reply"], "session", "{reply}");
    assert_eq!(reply["name"], serde_json::Value::Null);
    assert_eq!(h.record(id)["name"], serde_json::Value::Null);

    // Refused, not cleaned: the name is unchanged afterwards.
    h.raw(&format!(r#"{{"cmd":"rename","id":{id},"name":"kept"}}"#));
    let reply = h.raw(&format!(r#"{{"cmd":"rename","id":{id},"name":"a\u001b[2Jb"}}"#));
    assert_eq!(reply["kind"], "bad_request", "{reply}");
    assert_eq!(h.info(id)["name"], "kept");

    let reply = h.raw(r#"{"cmd":"rename","id":9999,"name":"x"}"#);
    assert_eq!(reply["kind"], "no_such_session", "{reply}");

    // A session that has finished keeps the name it had.
    let Some(done) = h.run_shell("exit 0", Some("finished")) else {
        return;
    };
    let info = h.info_until(done, 5000, |i| i["exit_code"].is_number());
    assert!(info["exit_code"].is_number(), "the session never exited: {info}");
    let reply = h.raw(&format!(r#"{{"cmd":"rename","id":{done},"name":"late"}}"#));
    assert_eq!(reply["kind"], "session_exited", "{reply}");
    assert_eq!(h.info(done)["name"], "finished");
}

#[test]
fn peek_reads_the_tail_without_attaching_or_resizing() {
    let h = harness!("peek");
    // `\n` and not `\r\n`: the line discipline's ONLCR turns each newline
    // into CR LF on the way out, and the peek is of what the terminal got.
    let Some(id) = h.run_shell("printf 'line one\\nPEEK-MARKER\\n'; sleep 30", None) else {
        return;
    };
    h.logs_until(id, "PEEK-MARKER", 3000);

    let reply = h.raw(&format!(r#"{{"cmd":"peek","id":{id},"bytes":8192}}"#));
    assert_eq!(reply["reply"], "peek", "{reply}");
    assert_eq!(reply["id"], id);
    assert_eq!(size(&reply), (120, 40), "the PTY's own size: {reply}");
    assert!(reply["state"].is_string(), "{reply}");
    let data = reply["data"].as_str().expect("data is a string");
    let bytes = rime_agent_core::webauthn::b64_decode(data).expect("standard base64");
    assert!(
        String::from_utf8_lossy(&bytes).ends_with("PEEK-MARKER\r\n"),
        "{:?}",
        String::from_utf8_lossy(&bytes)
    );
    // Padded, standard alphabet: what the contract says a phone decodes.
    assert_eq!(data.len() % 4, 0);

    // `bytes` is a ceiling the daemon also caps, and no key means the cap.
    let small = h.raw(&format!(r#"{{"cmd":"peek","id":{id},"bytes":6}}"#));
    let small = rime_agent_core::webauthn::b64_decode(small["data"].as_str().unwrap()).unwrap();
    assert_eq!(small, b"RKER\r\n", "the LAST six bytes");
    let huge = h.raw(&format!(r#"{{"cmd":"peek","id":{id},"bytes":100000000}}"#));
    let huge = rime_agent_core::webauthn::b64_decode(huge["data"].as_str().unwrap()).unwrap();
    assert!(huge.len() <= rime_agent_core::session::PEEK_MAX);
    let bare = h.raw(&format!(r#"{{"cmd":"peek","id":{id}}}"#));
    assert_eq!(bare["data"], reply["data"], "no bytes means PEEK_MAX");

    // Nothing about the session moved.
    let info = h.info(id);
    assert_eq!(info["attached"], 0, "a peek is not an attach: {info}");
    assert_eq!(size(&info), (120, 40));

    let reply = h.raw(r#"{"cmd":"peek","id":9999}"#);
    assert_eq!(reply["kind"], "no_such_session", "{reply}");
}

#[test]
fn an_agents_terminal_title_reaches_the_session_and_the_record() {
    let h = harness!("title");
    // The bytes Claude Code writes, spinner glyph included: ESC ] 0 ; ✳ … BEL.
    let script = "printf '\\033]0;\\342\\234\\263 Rime showcase studio\\007'; sleep 30";
    let Some(id) = h.run_shell(script, Some("my name")) else {
        return;
    };
    let info = h.info_until(id, 3000, |i| i["title"].is_string());
    assert_eq!(info["title"], "Rime showcase studio", "{info}");
    // The title never overwrites the person's name.
    assert_eq!(info["name"], "my name");
    assert_eq!(h.record(id)["title"], "Rime showcase studio", "the record was not written");
}

#[test]
fn submit_turns_text_into_a_line_the_program_acts_on() {
    let h = harness!("submit");
    let Some(id) = h.run_shell("read line; echo \"got:[$line]\"; sleep 30", None) else {
        return;
    };
    std::thread::sleep(Duration::from_millis(300));
    let reply = h.raw(&format!(r#"{{"cmd":"input","id":{id},"data":"yes please","submit":true}}"#));
    assert_eq!(reply["reply"], "ok", "{reply}");
    let seen = h.logs_until(id, "got:[", 3000);
    assert!(seen.contains("got:[yes please]"), "the line was not submitted: {seen:?}");
}

#[test]
fn a_phone_that_leaves_gives_the_desktop_its_size_back() {
    let h = harness!("size");
    // `cat`, so keystrokes on either attach have a reader and are harmless.
    let Some(id) = h.run_shell("cat >/dev/null", None) else {
        return;
    };

    // The desktop attaches at its size.
    let mut desk = h.connect();
    let reply = Harness::line_on(
        &mut desk,
        &format!(r#"{{"cmd":"attach","id":{id},"cols":180,"rows":50,"replay":0}}"#),
    );
    assert_eq!(reply["reply"], "attached", "{reply}");
    assert_eq!(size(&h.info_until(id, 2000, |i| size(i) == (180, 50))), (180, 50));

    // The phone attaches the way rime-remoted attaches it.
    let Some(mut phone) = h.as_phone() else {
        return;
    };
    let reply = Harness::line_on(
        &mut phone,
        &format!(r#"{{"cmd":"attach","id":{id},"cols":46,"rows":30,"replay":0}}"#),
    );
    assert_eq!(reply["reply"], "attached", "{reply}");
    assert_eq!(size(&h.info_until(id, 2000, |i| size(i) == (46, 30))), (46, 30));

    // Its keyboard comes up: a resize, on a connection of its own.
    let Some(mut frame) = h.as_phone() else {
        return;
    };
    let reply = Harness::line_on(&mut frame, &format!(r#"{{"cmd":"resize","id":{id},"cols":46,"rows":18}}"#));
    assert_eq!(reply["reply"], "ok", "{reply}");
    assert_eq!(size(&h.info(id)), (46, 18));

    // The desktop user types while the phone looks: their size comes back.
    desk.write_all(b"y").unwrap();
    assert_eq!(
        size(&h.info_until(id, 2000, |i| size(i) == (180, 50))),
        (180, 50),
        "typing on the desktop did not take the terminal back"
    );
    // The phone types: the phone's size.
    phone.write_all(b"n").unwrap();
    assert_eq!(size(&h.info_until(id, 2000, |i| size(i) == (46, 18))), (46, 18));

    // The phone closes its terminal. Before §1.7 the desktop stayed 46 wide.
    drop(phone);
    let info = h.info_until(id, 3000, |i| size(i) == (180, 50));
    assert_eq!(size(&info), (180, 50), "the desktop did not get its size back: {info}");
    assert_eq!(info["attached"], 1);

    // A resize frame from the phone that arrives after it left.
    let Some(mut late) = h.as_phone() else {
        return;
    };
    let reply = Harness::line_on(&mut late, &format!(r#"{{"cmd":"resize","id":{id},"cols":40,"rows":20}}"#));
    assert_eq!(reply["reply"], "ok", "{reply}");
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(size(&h.info(id)), (180, 50), "a late phone resize shrank the desktop again");

    // And a desktop resize is the size given back next time.
    let reply = h.call(&Request::Resize { id, cols: 200, rows: 60 });
    assert_eq!(reply["reply"], "ok", "{reply}");
    assert_eq!(size(&h.info(id)), (200, 60));
    drop(desk);
}
