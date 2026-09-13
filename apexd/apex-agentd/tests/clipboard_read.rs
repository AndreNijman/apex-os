//! `Request::Clipboard` against a real daemon over a real socket.
//!
//! P1-059's third criterion, receive half. The unit tests beside
//! `clipboard.rs` settle the arithmetic, the ranking and the refusals as pure
//! functions. Five things cannot be said about a value, and they are the five
//! this file is for:
//!
//!   * the verb is **reachable** — `dispatch` routes `{"cmd":"clipboard"}` to
//!     the handler rather than answering "unparseable request", which is what
//!     every version of this before the arm existed did;
//!   * the daemon passes its discovered `WAYLAND_DISPLAY` and
//!     `XDG_RUNTIME_DIR` to the tool, which is the whole of the fix for a
//!     daemon that has neither;
//!   * it asks for a **named text type**, so a clipboard holding a screenshot
//!     is refused rather than served as a wall of PNG;
//!   * a wedged application cannot hold the connection — the bound is real,
//!     measured against the clock, not asserted about a constant;
//!   * a **session** is refused, from inside a real session, with the origin
//!     resolved from `SO_PEERCRED` and real `/proc` ancestry. That is the
//!     claim the verb's design rests on, and a gate that was written but never
//!     wired into the handler would pass every unit test in the module.
//!
//! ## What is faked, and why it is only this
//!
//! `wl-paste` is a script on `PATH`, and the compositor is a `UnixListener`
//! the harness binds. Nothing else is: the daemon is the real binary, the
//! socket is a real socket, the session is a real PTY under a real sandbox
//! policy, and the JSON is the protocol's own.
//!
//! A real compositor cannot be in a test — this machine's rule is that no test
//! opens a window on the user's desktop — and asking the *user's* clipboard
//! would be worse than useless: it is the user's, it changes under the test,
//! and it regularly holds a password, which is the entire reason this verb has
//! a refusal.
//!
//! The daemon runs with its own `XDG_RUNTIME_DIR` and `XDG_STATE_HOME`, binds
//! a socket of its own, and is killed BY PID. It never touches a running
//! `apex-agentd`.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use apex_agent_core::policy::AgentPolicy;
use apex_agent_core::protocol::{Request, RunRequest, SandboxPolicy};

/// A daemon of our own, a fake compositor, and a fake `wl-paste`.
struct Harness {
    child: Child,
    socket: PathBuf,
    root: PathBuf,
    runtime: PathBuf,
    /// The compositor. Held so the socket keeps answering `connect(2)`;
    /// dropping it is what makes the "no display" case.
    _seat: Option<UnixListener>,
}

impl Drop for Harness {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// The fake `wl-paste`.
///
/// It reads its fixture out of `XDG_RUNTIME_DIR` rather than out of an
/// environment variable of its own, because `XDG_RUNTIME_DIR` is one of the
/// two variables the daemon sets on the child — so a daemon that passed the
/// wrong one, or none, finds no fixture and this test fails instead of quietly
/// reading the real desktop's clipboard.
///
/// It also records what it was asked, which is how the "a picture is refused
/// without reading it" assertion is made: the proof is that the second
/// invocation never happened.
const FAKE_WL_PASTE: &str = r#"#!/bin/sh
d="${XDG_RUNTIME_DIR:-/nonexistent}"
printf '%s\n' "$*" >> "$d/seen-args"
printf '%s' "$WAYLAND_DISPLAY" > "$d/seen-display"
if [ -e "$d/hang" ]; then
    sleep 120
    exit 0
fi
case "$1" in
    --list-types)
        [ -s "$d/types" ] || exit 1
        cat "$d/types"
        exit 0
        ;;
esac
[ -e "$d/body" ] || exit 1
cat "$d/body"
exit 0
"#;

impl Harness {
    fn start(tag: &str) -> Option<Harness> {
        Harness::start_inner(tag, true)
    }

    /// The same daemon with no compositor listening. Its own constructor
    /// because "there is no display" is an answer this verb has to give
    /// correctly, and giving it correctly is indistinguishable from giving it
    /// by accident unless the other case is also tested.
    fn start_without_a_compositor(tag: &str) -> Option<Harness> {
        Harness::start_inner(tag, false)
    }

    fn start_inner(tag: &str, with_seat: bool) -> Option<Harness> {
        let root = std::env::temp_dir().join(format!(
            "apex-clip-e2e-{}-{tag}-{}",
            std::process::id(),
            now_ms()
        ));
        let runtime = root.join("run");
        let state = root.join("state");
        let bin = root.join("bin");
        std::fs::create_dir_all(&runtime).ok()?;
        std::fs::create_dir_all(&state).ok()?;
        std::fs::create_dir_all(&bin).ok()?;

        let tool = bin.join("wl-paste");
        std::fs::write(&tool, FAKE_WL_PASTE).ok()?;
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).ok()?;
        }

        // `wayland-0`, because the daemon's own runtime dir is this one and
        // `candidates` only ever probes `wayland-<digits>` there.
        let seat = if with_seat {
            Some(UnixListener::bind(runtime.join("wayland-0")).ok()?)
        } else {
            None
        };

        let path = match std::env::var("PATH") {
            Ok(p) => format!("{}:{p}", bin.display()),
            Err(_) => bin.display().to_string(),
        };

        let child = Command::new(env!("CARGO_BIN_EXE_apex-agentd"))
            .env("XDG_RUNTIME_DIR", &runtime)
            .env("XDG_STATE_HOME", &state)
            .env("PATH", &path)
            // Removed rather than left alone: this test asserts that the
            // daemon DISCOVERS a display, and inheriting the developer's would
            // have it pass while discovering nothing.
            .env_remove("WAYLAND_DISPLAY")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;

        let socket = runtime.join("apex-agentd").join("control.sock");
        let harness = Harness {
            child,
            socket,
            root,
            runtime,
            _seat: seat,
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

    /// Put something on the fake clipboard.
    fn set_clipboard(&self, types: &str, body: &[u8]) {
        std::fs::write(self.runtime.join("types"), types).expect("types fixture");
        std::fs::write(self.runtime.join("body"), body).expect("body fixture");
    }

    fn seen_args(&self) -> String {
        std::fs::read_to_string(self.runtime.join("seen-args")).unwrap_or_default()
    }

    fn call(&self, req: &Request) -> serde_json::Value {
        let line = serde_json::to_string(req).expect("serialise");
        self.call_line(&line)
    }

    fn call_line(&self, line: &str) -> serde_json::Value {
        assert!(
            !line.contains('\n'),
            "the framing is one JSON object per line; this payload would desynchronise it"
        );
        let mut stream = UnixStream::connect(&self.socket).expect("connect");
        stream
            .set_read_timeout(Some(Duration::from_secs(60)))
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
            eprintln!("SKIP: the daemon would not start a session: {reply}");
            return None;
        }
        // Internally-tagged newtype variant: SessionInfo's fields are at the
        // top level of the reply, so this is `reply["id"]` and not
        // `reply["session"]["id"]`. An unreadable id PANICS rather than
        // skipping — a fixture that cannot tell "skipped" from "passed" is
        // worse than one that fails.
        let id = reply["id"]
            .as_u64()
            .unwrap_or_else(|| panic!("a session reply carrying no id: {reply}"));
        Some(id as u32)
    }

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

fn error_message(reply: &serde_json::Value) -> String {
    reply["message"].as_str().unwrap_or_default().to_string()
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

const TEXT_TYPES: &str = "image/png\ntext/plain\ntext/plain;charset=utf-8\n";

#[test]
fn the_verb_is_reachable_and_the_selection_comes_back_verbatim() {
    let h = harness!("read");
    // Everything JSON has to escape and a terminal would act on, because the
    // response travels as one NDJSON line and the phone pastes what it gets.
    let body = "git log --oneline\n\"quoted\"\tand\ttabbed\n\\ backslash\n";
    h.set_clipboard(TEXT_TYPES, body.as_bytes());

    let reply = h.call(&Request::Clipboard);
    assert_eq!(
        reply["reply"], "clipboard",
        "the verb did not reach its handler: {reply}"
    );
    assert_eq!(
        reply["text"].as_str(),
        Some(body),
        "the selection changed on the way out: {reply}"
    );

    // The two variables the daemon had to work out for itself. Its own
    // environment has neither.
    let seen = std::fs::read_to_string(h.runtime.join("seen-display")).expect("the tool ran");
    assert_eq!(
        seen, "wayland-0",
        "the daemon did not pass the display it discovered"
    );

    // A NAMED text type, which is what stops a screenshot being served as the
    // answer to this verb.
    let args = h.seen_args();
    assert!(
        args.contains("--type text/plain;charset=utf-8"),
        "the daemon asked for no particular type, so whatever the owner listed \
         first would have come back: {args:?}"
    );
    assert!(
        args.contains("--no-newline"),
        "a newline this daemon appends is the return key on the phone's \
         terminal: {args:?}"
    );
}

#[test]
fn an_empty_clipboard_is_an_answer_and_never_an_error() {
    let h = harness!("empty");
    // `wl-paste --list-types` exits non-zero with nothing copied.
    h.set_clipboard("", b"");
    let reply = h.call(&Request::Clipboard);
    assert_eq!(
        reply["reply"], "clipboard",
        "an empty clipboard must not be reported as a refusal, or the user \
         goes looking for a permission to grant: {reply}"
    );
    assert_eq!(reply["text"], "");
}

#[test]
fn a_clipboard_holding_a_picture_is_refused_and_its_bytes_are_never_read() {
    let h = harness!("picture");
    // A screenshot tool offers exactly this. The body is deliberately not
    // text, so a daemon that fetched it anyway would produce a response this
    // test can see.
    h.set_clipboard("image/png\nimage/bmp\n", &[0x89, b'P', b'N', b'G', 0x0d, 0x0a]);

    let reply = h.call(&Request::Clipboard);
    assert_eq!(error_kind(&reply), "bad_request", "{reply}");
    let message = error_message(&reply);
    assert!(
        message.contains("image/png"),
        "the refusal must say what the clipboard does hold: {message}"
    );

    // The proof that nothing was read: the second invocation never happened.
    let args = h.seen_args();
    assert!(
        args.contains("--list-types"),
        "the tool was never asked at all: {args:?}"
    );
    assert!(
        !args.contains("--no-newline"),
        "the daemon fetched a picture's bytes before deciding it did not want \
         them: {args:?}"
    );
}

#[test]
fn a_clipboard_past_the_cap_is_refused_with_the_limit_named() {
    let h = harness!("oversize");
    // One byte over. The frame arithmetic is checked in the unit tests; what
    // this checks is that the cap is applied to the thing that actually comes
    // out of the tool.
    let big = vec![b'x'; 8 * 1024 + 1];
    h.set_clipboard(TEXT_TYPES, &big);

    let reply = h.call(&Request::Clipboard);
    assert_eq!(error_kind(&reply), "bad_request", "{reply}");
    let message = error_message(&reply);
    assert!(
        message.contains("8192"),
        "the refusal must name the limit: {message}"
    );

    // And the boundary the other way, on the same daemon: exactly the cap is
    // carried. A cap that refused what it was sized for would pass the
    // assertion above.
    h.set_clipboard(TEXT_TYPES, &vec![b'x'; 8 * 1024]);
    let reply = h.call(&Request::Clipboard);
    assert_eq!(
        reply["reply"], "clipboard",
        "exactly the cap must be carried: {reply}"
    );
    assert_eq!(reply["text"].as_str().map(str::len), Some(8 * 1024));
}

#[test]
fn a_wedged_application_cannot_hold_the_connection() {
    let h = harness!("wedged");
    h.set_clipboard(TEXT_TYPES, b"never served");
    // A Wayland clipboard is served by the application that owns it. This is
    // what an application that has stopped serving looks like from here, and
    // without the bound it is a phone whose terminal freezes for five minutes
    // — `apex-remoted` calls this inline in its frame loop with a 300-second
    // socket timeout.
    std::fs::write(h.runtime.join("hang"), b"").expect("hang fixture");

    let started = Instant::now();
    let reply = h.call(&Request::Clipboard);
    let took = started.elapsed();

    assert_eq!(error_kind(&reply), "internal", "{reply}");
    let message = error_message(&reply);
    assert!(
        message.contains('5'),
        "the refusal must say which clock ran out: {message}"
    );
    // Generous, because this asserts that a bound EXISTS rather than that it is
    // precise; the fake sleeps for 120 seconds, so anything under that is the
    // daemon's own deadline firing and not the fake finishing.
    assert!(
        took < Duration::from_secs(30),
        "the read was not bounded: it took {took:?}"
    );
}

#[test]
fn a_machine_with_no_compositor_says_so_rather_than_reporting_an_empty_clipboard() {
    // The defect this whole module exists because of. The agent runtime starts
    // before a graphical session and never gets `WAYLAND_DISPLAY`; a verb that
    // reported that as "your clipboard is empty" would be lying about the one
    // thing it exists to report, and the user would keep copying things and
    // keep being told there is nothing there.
    let h = match Harness::start_without_a_compositor("nodisplay") {
        Some(h) => h,
        None => {
            eprintln!("SKIP: apex-agentd did not come up in this environment");
            return;
        }
    };
    h.set_clipboard(TEXT_TYPES, b"this must never be reached");

    let reply = h.call(&Request::Clipboard);
    assert_ne!(
        reply["reply"], "clipboard",
        "no display was reported as a clipboard answer: {reply}"
    );
    assert_eq!(error_kind(&reply), "internal", "{reply}");
    let message = error_message(&reply);
    assert!(
        message.contains("Wayland"),
        "the refusal must say what is missing: {message}"
    );
    assert!(
        !message.contains("empty"),
        "no display must never be described as an empty clipboard: {message}"
    );
    // And the tool was never run, because there was nothing to run it against.
    assert_eq!(h.seen_args(), "", "wl-paste was run with no display to ask");
}

#[test]
fn a_session_may_not_read_this_machines_clipboard() {
    // The claim the verb's design rests on, made the only way it can be made:
    // from inside a real session, with the origin resolved by the daemon from
    // SO_PEERCRED and real /proc ancestry. A gate written in `privilege.rs`
    // and never called from the handler passes every unit test in the module
    // and fails here.
    //
    // A person copies a password out of a password manager several times a
    // day. An agent that could ask this verb would have a thirty-second window
    // on every one of them, outside every sandbox, with nothing on screen and
    // no grant spent.
    if Command::new("python3")
        .arg("-c")
        .arg("import socket")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| !s.success())
        .unwrap_or(true)
    {
        // Said out loud. A silent skip is a test that asserts nothing.
        eprintln!("SKIP: no python3 with sockets, so nothing can speak the socket from inside a session");
        return;
    }

    let h = harness!("session");
    h.set_clipboard(TEXT_TYPES, b"a password, as far as this test is concerned");

    let sock = h.socket.display().to_string();
    let prog = format!(
        "import socket\n\
         s = socket.socket(socket.AF_UNIX)\n\
         s.connect({sock:?})\n\
         s.sendall(b'{{\"cmd\":\"clipboard\"}}\\n')\n\
         print('REPLY', s.makefile().readline().strip())\n\
         print('DONE')\n"
    );
    let script = format!("python3 -c {} 2>&1; echo DONE", shell_quote(&prog));
    let Some(id) = h.run_shell(&script) else {
        return;
    };

    let logs = h.logs_until(id, "DONE", 20_000);
    assert!(
        logs.contains("REPLY"),
        "the session never got an answer at all: {logs:?}"
    );
    assert!(
        logs.contains("permission_denied"),
        "a session read this machine's clipboard: {logs:?}"
    );
    assert!(
        !logs.contains("a password, as far as this test is concerned"),
        "the refusal happened AFTER the clipboard was read: {logs:?}"
    );
    // The tool was never run for this caller. `set_clipboard` above is the
    // control: there WAS something to read, and the daemon did not read it.
    assert_eq!(
        h.seen_args(),
        "",
        "the daemon read the clipboard and then refused to hand it over, which \
         is not the same thing as refusing"
    );
}

/// Single-quote for `sh`, the way `dispatch.rs` does.
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}
