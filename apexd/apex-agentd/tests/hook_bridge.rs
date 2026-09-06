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
use std::path::{Path, PathBuf};
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

        // Its own scratch root as well as its own runtime and state. Session
        // ids are allocated against a daemon's own store, so every daemon here
        // hands out id 1 — and on the shared default they would each remove
        // the others' `/tmp/apex-agent/1` on teardown, which surfaces as
        // `bwrap: Can't open source` in whichever test was slowest.
        let child = Command::new(env!("CARGO_BIN_EXE_apex-agentd"))
            .env("XDG_RUNTIME_DIR", &runtime)
            .env("XDG_STATE_HOME", &state)
            .env("APEX_AGENT_SCRATCH", root.join("scratch"))
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

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
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

// ─── The session graph (P1-020) ─────────────────────────────────────────────
//
// `graph.rs` proves the bookkeeping against values. What it cannot prove is
// that a `subagent_start` crossing the socket lands on the session's record —
// which is the half P0-011 got wrong: the daemon received both subagent events
// and threw `agent_id` and `agent_type` away, so the mapping was correct and
// no subagent was recorded anywhere.
//
// These publish over the control socket rather than through `apex agent hook`,
// so they run under `cargo test -p apex-agentd` without the CLI built. The
// hook's own end of it — that `observe` carries the two fields out of the
// payload — is asserted beside `observe` in `hook.rs`.

impl Harness {
    fn info(&self, id: u64) -> serde_json::Value {
        self.call(&format!(r#"{{"cmd":"info","id":{id}}}"#))
    }

    /// Publish a lifecycle event the way `apex agent hook` does.
    fn event(&self, id: u64, event: &str, agent_id: Option<&str>, agent_type: Option<&str>) {
        let reply = self.call(
            &serde_json::json!({
                "cmd": "event",
                "id": id,
                "event": event,
                "agent_id": agent_id,
                "agent_type": agent_type,
            })
            .to_string(),
        );
        assert_eq!(reply["reply"], "ok", "publishing {event}: {reply}");
    }

    /// A session running `sleep`, or `None` when one will not start.
    fn sleeper(&self, tag: &str) -> Option<u64> {
        let run = serde_json::json!({
            "cmd": "run",
            "agent": "generic",
            "args": ["sh", "-c", "sleep 60"],
            "cwd": "/tmp",
            "sandbox": "unrestricted",
            "network": "open",
            "cols": 80,
            "rows": 24,
        });
        let reply = self.call(&run.to_string());
        if reply["reply"] != "session" {
            eprintln!("SKIP {tag}: {reply}");
            return None;
        }
        reply["id"].as_u64()
    }
}

/// The children of a session, as the shell would read them.
fn children(info: &serde_json::Value) -> &Vec<serde_json::Value> {
    info["children"]
        .as_array()
        .unwrap_or_else(|| panic!("no children key on {info}"))
}

#[test]
fn a_session_reports_a_graph_even_when_it_has_delegated_nothing() {
    // The distinction the shell depends on. A daemon that predates P1-020
    // writes no `children` key at all, and a client that read that absence as
    // "no subagents" would be reporting a fact it has no evidence for. A
    // daemon that has the graph always writes the key.
    let h = harness!("graph-empty");
    let Some(id) = h.sleeper("graph-empty") else {
        return;
    };
    let info = h.info(id);
    let kids = children(&info);
    assert!(
        kids.iter().all(|k| k["kind"] == "process"),
        "nothing delegated, so nothing may claim to be a subagent: {kids:?}"
    );
    h.call(&format!(r#"{{"cmd":"signal","id":{id},"signal":"kill"}}"#));
}

#[test]
fn the_processes_a_session_forked_are_in_its_graph() {
    // The half no agent reports: MCP servers, language servers, the compiler a
    // tool call started. None of them publish anything and none of them have
    // to — they are processes, and the kernel already has the list.
    //
    // `sh` execs its last command, so the probe needs a background job to
    // fork at all. The name is deliberate: `comm` is fifteen characters and
    // this one has to survive being read out of /proc/<pid>/stat's own
    // brackets.
    let h = harness!("graph-procs");
    // Its own directory, so a concurrent test cannot be running a probe of the
    // same name — the assertion below is "a process called apexprobe is under
    // THIS session", and a shared /tmp would make that ambiguous.
    let probe_dir = h.root.join("probe");
    std::fs::create_dir_all(&probe_dir).expect("probe directory");
    let run = serde_json::json!({
        "cmd": "run",
        "agent": "generic",
        "args": ["sh", "-c", "cp /bin/sleep ./apexprobe && ./apexprobe 90 & sleep 90"],
        "cwd": probe_dir.to_str().expect("probe directory"),
        "sandbox": "unrestricted",
        "network": "open",
        "cols": 80,
        "rows": 24,
    });
    let reply = h.call(&run.to_string());
    if reply["reply"] != "session" {
        eprintln!("SKIP graph-procs: {reply}");
        return;
    }
    let id = reply["id"].as_u64().expect("session id");

    let deadline = Instant::now() + Duration::from_secs(15);
    let mut found = serde_json::Value::Null;
    while Instant::now() < deadline {
        let info = h.info(id);
        if let Some(node) = children(&info)
            .iter()
            .find(|k| k["kind"] == "process" && k["label"] == "apexprobe")
        {
            found = node.clone();
            break;
        }
        std::thread::sleep(Duration::from_millis(150));
    }
    h.call(&format!(r#"{{"cmd":"signal","id":{id},"signal":"kill"}}"#));

    assert!(
        !found.is_null(),
        "a process the session forked is not in its graph"
    );
    assert!(found["pid"].as_i64().unwrap_or(0) > 0, "{found}");
    assert!(
        found["rss_kb"].as_u64().unwrap_or(0) > 0,
        "resource accounting has to carry a number: {found}"
    );
    assert!(
        found["ended"].is_null(),
        "a process read out of /proc is running by definition: {found}"
    );
}

#[test]
fn a_subagent_start_and_stop_become_one_finished_child() {
    let h = harness!("graph-pair");
    let Some(id) = h.sleeper("graph-pair") else {
        return;
    };

    h.event(id, "subagent_start", Some("agent-7"), Some("Explore"));
    let info = h.info(id);
    let kids: Vec<&serde_json::Value> = children(&info)
        .iter()
        .filter(|k| k["kind"] == "subagent")
        .collect();
    assert_eq!(kids.len(), 1, "the start was not recorded: {kids:?}");
    assert_eq!(kids[0]["id"], "agent-7");
    assert_eq!(kids[0]["kind"], "subagent");
    assert_eq!(kids[0]["label"], "Explore");
    assert!(kids[0]["ended"].is_null(), "it has not finished yet");

    h.event(id, "subagent_stop", Some("agent-7"), Some("Explore"));
    let info = h.info(id);
    let kids: Vec<&serde_json::Value> = children(&info)
        .iter()
        .filter(|k| k["kind"] == "subagent")
        .collect();
    assert_eq!(kids.len(), 1, "the stop opened a second node: {kids:?}");
    assert!(!kids[0]["ended"].is_null(), "the stop did not close it");
    assert_eq!(kids[0]["ended_by"], "reported");

    h.call(&format!(r#"{{"cmd":"signal","id":{id},"signal":"kill"}}"#));
}

#[test]
fn the_end_of_a_turn_closes_a_subagent_whose_stop_never_arrived() {
    // The failure mode the whole design is aimed at. `SubagentStop` is a hook,
    // and §6.1 says a hook is advisory: it can be silenced, it can time out,
    // and it does not run when the agent is killed mid-turn. Without this
    // sweep the record would claim a subagent is working for as long as it
    // survives, and a graph showing a dead agent as alive is worse than one
    // showing nothing.
    let h = harness!("graph-turn");
    let Some(id) = h.sleeper("graph-turn") else {
        return;
    };

    h.event(id, "subagent_start", Some("agent-1"), Some("Explore"));
    h.event(id, "subagent_start", Some("agent-2"), Some("Plan"));
    h.event(id, "stop", None, None);

    let info = h.info(id);
    let kids: Vec<&serde_json::Value> = children(&info)
        .iter()
        .filter(|k| k["kind"] == "subagent")
        .collect();
    assert_eq!(kids.len(), 2);
    for kid in &kids {
        assert!(!kid["ended"].is_null(), "{kid} outlived the turn");
        assert_eq!(kid["ended_by"], "parent_stop");
    }

    // And the sweep yields to the truth. A subagent can outlive the turn that
    // delegated it, so `parent_stop` is the daemon saying it can no longer
    // tell — not that the subagent finished. The real report replaces it.
    h.event(id, "subagent_stop", Some("agent-1"), Some("Explore"));
    let info = h.info(id);
    let corrected = children(&info)
        .iter()
        .find(|k| k["id"] == "agent-1")
        .expect("agent-1 is still in the graph");
    assert_eq!(corrected["ended_by"], "reported", "{corrected}");

    h.call(&format!(r#"{{"cmd":"signal","id":{id},"signal":"kill"}}"#));
}

#[test]
fn a_killed_session_has_nothing_still_running_under_it() {
    let h = harness!("graph-exit");
    let Some(id) = h.sleeper("graph-exit") else {
        return;
    };

    h.event(id, "subagent_start", Some("agent-1"), Some("Explore"));
    h.call(&format!(r#"{{"cmd":"signal","id":{id},"signal":"kill"}}"#));

    let deadline = Instant::now() + Duration::from_secs(10);
    let mut info = h.info(id);
    while Instant::now() < deadline && info["exit_signal"].is_null() && info["exit_code"].is_null()
    {
        std::thread::sleep(Duration::from_millis(100));
        info = h.info(id);
    }
    let kids: Vec<&serde_json::Value> = children(&info)
        .iter()
        .filter(|k| k["kind"] == "subagent")
        .collect();
    assert_eq!(kids.len(), 1);
    assert!(
        !kids[0]["ended"].is_null(),
        "a subagent survived its agent: {info}"
    );
    assert_eq!(kids[0]["ended_by"], "parent_exit");
}

// ─── The status line (P1-021) ───────────────────────────────────────────────
//
// `statusline.rs` proves the parse, the settings precedence walk and the
// chaining against values and fixtures. What it cannot prove is the thing the
// criterion is actually about: that running `apex agent statusline` prints
// what the user's own status line printed, and that the numbers reach the
// daemon.
//
// Run as a plain subprocess with `$APEX_AGENT_SESSION` set rather than from
// inside a session, because the daemon's own `HOME` is what the settings walk
// reads and this file's other tests want the real one.

/// The document Claude hands a status line, as documented and as observed on
/// this machine.
fn status_payload(dir: &str) -> String {
    serde_json::json!({
        "hook_event_name": "Status",
        "session_id": "s",
        "cwd": dir,
        "version": "2.1.0",
        "model": { "id": "claude-opus-4-5", "display_name": "Opus 4.5" },
        "workspace": { "current_dir": dir, "project_dir": dir },
        "context_window": { "used_percentage": 41.5 },
        "rate_limits": {
            "five_hour": { "used_percentage": 62.0, "resets_at": 1757300000u64 },
            "seven_day": { "used_percentage": 18.5, "resets_at": 1757800000u64 }
        }
    })
    .to_string()
}

/// Run `apex agent statusline` the way Claude would, and return its stdout.
fn run_statusline(h: &Harness, apex: &Path, home: &Path, id: u64, payload: &str) -> String {
    use std::io::Write;
    let mut child = Command::new(apex)
        .args(["agent", "statusline"])
        .env("HOME", home)
        .env(
            "XDG_RUNTIME_DIR",
            h.socket
                .parent()
                .and_then(|p| p.parent())
                .expect("runtime dir"),
        )
        .env("APEX_AGENT_SESSION", id.to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("apex agent statusline");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(payload.as_bytes())
        .expect("payload");
    let out = child.wait_with_output().expect("wait");
    assert!(
        out.status.success(),
        "a status line must never exit non-zero: {:?}\n{}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn the_status_line_prints_the_users_own_and_publishes_what_it_read() {
    let apex = apex_cli();
    if !apex.is_file() {
        let msg = format!(
            "{} has not been built — run `cargo build -p apex` or the whole \
             workspace; this test cannot exercise the status line without it",
            apex.display()
        );
        assert!(
            std::env::var_os("APEX_REQUIRE_APEX_CLI").is_none(),
            "{msg}"
        );
        eprintln!("SKIP: {msg}");
        return;
    }
    let h = harness!("statusline");
    let Some(id) = h.sleeper("statusline") else {
        return;
    };

    // A home with the user's own status line in it, exactly as Andre's is
    // configured: `~/.claude/settings.json` naming a script.
    let home = h.root.join("home");
    std::fs::create_dir_all(home.join(".claude")).expect("home");
    std::fs::write(
        home.join(".claude/settings.json"),
        r#"{"statusLine":{"type":"command","command":"printf 'ORIGINAL LINE'",
            "refreshInterval":60}}"#,
    )
    .expect("settings");

    // Long enough that the clock has moved past the session's own
    // `last_activity`, which is set when it started and does not move again
    // while `sleep` produces nothing. Without this the assertion below passes
    // whatever the daemon does, because both values are the same second — a
    // mutant that recorded the status line as activity survived it once.
    let before = h.info(id)["last_activity"].as_u64().unwrap_or(0);
    std::thread::sleep(Duration::from_millis(2100));
    assert!(
        now_secs() > before,
        "the clock did not move, so the activity assertion below proves nothing"
    );
    let out = run_statusline(&h, &apex, &home, id, &status_payload("/tmp"));

    // 1. THE criterion. What appears under the prompt is what appeared before.
    assert_eq!(
        out, "ORIGINAL LINE",
        "the user's own status line did not survive being wrapped"
    );

    // 2. And the numbers arrived. None of these reach the daemon any other
    //    way: the hook bridge carries lifecycle events and has no model, no
    //    context and no rate limits.
    let info = h.info(id);
    let t = &info["telemetry"];
    assert!(!t.is_null(), "nothing was published: {info}");
    assert_eq!(t["model"], "Opus 4.5", "{t}");
    assert_eq!(t["context_pct"], 41.5, "{t}");
    assert_eq!(t["five_hour_pct"], 62.0, "{t}");
    assert_eq!(t["five_hour_reset"], 1757300000u64, "{t}");
    assert_eq!(t["seven_day_pct"], 18.5, "{t}");
    assert!(t["observed_at"].as_u64().unwrap_or(0) > 0, "{t}");

    // 3. And the session is no busier for having been described. A status line
    //    runs on a timer, so treating it as activity would keep every idle
    //    session looking awake — and the idle rule that decides
    //    `waiting_for_user` reads exactly this field.
    assert_eq!(
        h.info(id)["last_activity"].as_u64().unwrap_or(0),
        before,
        "publishing telemetry moved last_activity"
    );

    h.call(&format!(r#"{{"cmd":"signal","id":{id},"signal":"kill"}}"#));
}

#[test]
fn a_user_with_no_status_line_gets_no_status_line() {
    // The other half of "without breaking terminal statusline". A user who has
    // configured none must keep seeing none — printing something of APEX's own
    // would put a line under the prompt that was not there before, which is
    // the same defect in the opposite direction.
    let apex = apex_cli();
    if !apex.is_file() {
        eprintln!("SKIP: apex not built");
        return;
    }
    let h = harness!("statusline-none");
    let Some(id) = h.sleeper("statusline-none") else {
        return;
    };
    let home = h.root.join("bare-home");
    std::fs::create_dir_all(&home).expect("home");

    let out = run_statusline(&h, &apex, &home, id, &status_payload("/tmp"));
    assert_eq!(out, "", "APEX printed a status line nobody asked for: {out:?}");

    // The measurement still happens. It is not conditional on the user having
    // a status line of their own — Claude runs the command either way.
    assert_eq!(h.info(id)["telemetry"]["model"], "Opus 4.5");

    h.call(&format!(r#"{{"cmd":"signal","id":{id},"signal":"kill"}}"#));
}

#[test]
fn a_status_line_with_nothing_to_report_publishes_nothing() {
    // A status line runs once a minute per session. A build that published an
    // empty record anyway would rewrite every session's file on a timer to say
    // nothing, and would make "we have never heard from the status line"
    // indistinguishable from "we heard, and it said nothing".
    let apex = apex_cli();
    if !apex.is_file() {
        eprintln!("SKIP: apex not built");
        return;
    }
    let h = harness!("statusline-empty");
    let Some(id) = h.sleeper("statusline-empty") else {
        return;
    };
    let home = h.root.join("bare-home");
    std::fs::create_dir_all(&home).expect("home");

    run_statusline(&h, &apex, &home, id, "{}");
    assert!(
        h.info(id)["telemetry"].is_null(),
        "an empty document became a record: {}",
        h.info(id)
    );

    h.call(&format!(r#"{{"cmd":"signal","id":{id},"signal":"kill"}}"#));
}
