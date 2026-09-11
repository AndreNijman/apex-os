//! P0-006 and P0-007 against a real daemon.
//!
//! Everything about a grant that is a property of a value is tested beside the
//! code, and the acceptance criteria are in `apex-agent-core`'s
//! `tests/policy_invariants.rs`. Three claims are not properties of a value,
//! and they are why this file spawns a process:
//!
//! * **the boot case.** P0-006's fifth criterion is that a grant "does not
//!   silently persist across reboot", and the deliverable is what the machine
//!   *says* on the next boot. A grant record stamped with a boot id that is not
//!   this one is seeded into a fixture state directory, a daemon is started,
//!   and the audit trail is read. That is the criterion, executed.
//!
//! * **the refusals a connection decides.** P0-007's fourth and fifth criteria
//!   are about which process is on the other end of the socket, and no value
//!   carries that. A `--system-access` run and a renewal are both refused for
//!   a caller whose origin is not local, from a real `SO_PEERCRED` and a real
//!   `/proc` read.
//!
//! * **that the refusals happen before polkit.** This is the constraint the
//!   whole file is shaped by: **nothing here may raise an authentication
//!   prompt.** Every test drives a path that is refused *before* the daemon
//!   asks polkit anything, which is possible only because the refusals are
//!   ordered first — see `privilege::authorise_grant`. The one path that would
//!   prompt, a successful local grant, is deliberately not exercised here and
//!   is the one thing a human has to see once.
//!
//! The daemon gets its own `XDG_RUNTIME_DIR` and `XDG_STATE_HOME`, so it binds
//! its own socket and writes its own state. It never touches a running
//! `apex-agentd`, and it is killed by pid — never by name.

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
    /// A daemon with an empty state directory.
    fn start(tag: &str) -> Option<Harness> {
        Harness::start_with(tag, |_| {})
    }

    /// A daemon whose state directory `seed` was allowed to fill in first.
    ///
    /// The whole point of the boot test: the daemon has to find a grant left
    /// by something that is no longer running, which is what a reboot leaves.
    fn start_with(tag: &str, seed: impl FnOnce(&Path)) -> Option<Harness> {
        let root = std::env::temp_dir().join(format!(
            "apex-grants-e2e-{}-{tag}-{}",
            std::process::id(),
            now_ms()
        ));
        let runtime = root.join("run");
        let state = root.join("state");
        std::fs::create_dir_all(&runtime).ok()?;
        std::fs::create_dir_all(state.join("apex").join("agent")).ok()?;
        seed(&state);

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

    /// One request, one reply, on a fresh connection — so every call is
    /// attributed the way a separate `apex` invocation would be.
    fn call(&self, line: &str) -> serde_json::Value {
        assert!(!line.contains('\n'), "one JSON object per line");
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

    fn state_dir(&self) -> PathBuf {
        self.root.join("state").join("apex").join("agent")
    }

    fn audit_lines(&self) -> Vec<serde_json::Value> {
        std::fs::read_to_string(self.state_dir().join("privilege-audit.jsonl"))
            .map(|t| {
                t.lines()
                    .filter_map(|l| serde_json::from_str(l).ok())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The audit trail, once it has at least `want` lines.
    ///
    /// `start` waits for the control socket, and the startup sweep that writes
    /// these lines is not ordered against the socket appearing — so reading
    /// the file the instant the daemon answers is a race. Because
    /// `audit_lines` turns a missing file into an empty vector, losing that
    /// race did not read as "not yet", it read as "the daemon recorded
    /// nothing", and the assertion failed naming a trail of `[]`. Measured at
    /// roughly one run in eight, on a tree with no change to the sweep.
    ///
    /// Polled to a deadline rather than slept on, and it returns whatever it
    /// has when the deadline passes, so a genuine failure still fails — with
    /// the real contents printed — instead of hanging or being masked.
    fn audit_lines_once_written(&self, want: usize) -> Vec<serde_json::Value> {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let lines = self.audit_lines();
            if lines.len() >= want || Instant::now() >= deadline {
                return lines;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
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
                eprintln!("SKIP: apex-agentd did not come up in this environment");
                return;
            }
        }
    };
}

/// Whether this test process's own connection classifies as local.
///
/// Under a CI runner or a container the test binary may sit in neither a login
/// session nor a user service, and `origin::observe` refuses to classify it —
/// correctly, because "I could not tell" is not "a human is at the keyboard".
/// The refusal tests that turn on being non-local are meaningless there, and
/// the ones that turn on being local are unreachable, so each says which it
/// needs.
fn observed_origin(h: &Harness) -> Option<String> {
    let reply = h.call(r#"{"cmd":"requests"}"#);
    // `requests` never refuses, so it says nothing about the origin. File one
    // instead: the refusal, when there is one, names the /proc read that
    // failed.
    let filed = h.call(
        r#"{"cmd":"privilege_request","verb":"update","args":[],"reason":"deciding whether this environment classifies"}"#,
    );
    assert_eq!(reply["reply"], "requests");
    if filed["reply"] == "error" {
        return None;
    }
    filed["request_origin"].as_str().map(|s| s.to_string())
}

// ── P0-006 criterion 5: what the machine says on the next boot ──────────────

#[test]
fn a_grant_from_another_boot_is_reported_as_ended_on_the_next_start() {
    // The criterion, executed. A grant is left in the state directory
    // stamped with a boot id that is not this machine's and a window that had
    // not run out — which is exactly what a reboot in the middle of a
    // break-glass window leaves behind.
    //
    // The daemon must not adopt it, and must not merely drop it: it has to
    // say what happened, in the trail, once.
    let seeded_expiry = now_ms() as u64 + 3_600_000;
    let h = match Harness::start_with("reboot", |state| {
        let dir = state.join("apex").join("agent").join("system-grants");
        std::fs::create_dir_all(&dir).expect("mkdir");
        let grant = serde_json::json!({
            "id": 1,
            "kind": "break_glass",
            "session": 7,
            "agent": "claude",
            "project": null,
            "capabilities": [],
            "issued_ms": now_ms() as u64 - 60_000,
            "expires_ms": seeded_expiry,
            // Not this machine's. `/proc/sys/kernel/random/boot_id` is a uuid
            // and this is not one, so it cannot collide.
            "boot_id": "not-the-boot-this-test-is-running-on",
            "request_origin": "local-terminal",
            "authenticated_by": "org.apexos.agent.break-glass",
        });
        std::fs::write(
            dir.join("1.json"),
            serde_json::to_string_pretty(&grant).expect("serialise"),
        )
        .expect("write");
    }) {
        Some(h) => h,
        None => {
            eprintln!("SKIP: apex-agentd did not come up in this environment");
            return;
        }
    };

    // What the machine says: an audit line naming the reboot, with the grant
    // it was about.
    let trail = h.audit_lines_once_written(1);
    let ended: Vec<&serde_json::Value> = trail
        .iter()
        .filter(|l| l["event"] == "ended-at-reboot")
        .collect();
    assert_eq!(ended.len(), 1, "expected exactly one ending: {trail:#?}");
    assert_eq!(ended[0]["grant"], 1);
    assert_eq!(ended[0]["session"], 7);
    assert_eq!(ended[0]["kind"], "break-glass");
    assert_eq!(ended[0]["state"], "ended-at-reboot");
    // The authority that issued it survives into the ending, so an auditor
    // reading only this line still knows which of the two modes it was.
    assert_eq!(ended[0]["authenticated_by"], "org.apexos.agent.break-glass");

    // And the listing says it in words, which is the half a human reads.
    let listed = h.call(r#"{"cmd":"system_grants"}"#);
    assert_eq!(listed["reply"], "system_grants");
    let states = listed["states"].as_array().expect("states");
    assert_eq!(states.len(), 1, "{listed}");
    assert_eq!(states[0][0], "ended-at-reboot");
    let said = states[0][1].as_str().expect("a sentence");
    assert!(said.contains("rebooted"), "{said}");
    assert!(said.contains("nothing has been re-authorised"), "{said}");

    // Not adopted: the grant is not in force, and the record now carries the
    // ending rather than still looking live to the next reader.
    let stored: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(h.state_dir().join("system-grants").join("1.json"))
            .expect("the record is still there"),
    )
    .expect("parse");
    assert_eq!(stored["closed"]["why"], "reboot");
    // The window it was given is untouched — the record says what was
    // granted, and the closure says what became of it. Rewriting the expiry
    // would lose the fact that fifteen minutes were asked for.
    assert_eq!(stored["expires_ms"], seeded_expiry);
}

#[test]
fn the_ending_is_recorded_once_however_many_daemons_see_it() {
    // A second daemon over the same state directory must not write the
    // ending again. An audit trail that grows a line every time something
    // reads it is one nobody can count.
    let h = match Harness::start_with("reboot-twice", |state| {
        let dir = state.join("apex").join("agent").join("system-grants");
        std::fs::create_dir_all(&dir).expect("mkdir");
        let grant = serde_json::json!({
            "id": 4,
            "kind": "system_access",
            "session": 2,
            "agent": "claude",
            "project": null,
            "capabilities": ["install"],
            "issued_ms": 1,
            "expires_ms": 2,
            "boot_id": "another-boot-entirely",
            "request_origin": "local-terminal",
            "authenticated_by": "org.apexos.agent.system-access",
        });
        std::fs::write(dir.join("4.json"), grant.to_string()).expect("write");
    }) {
        Some(h) => h,
        None => {
            eprintln!("SKIP: apex-agentd did not come up in this environment");
            return;
        }
    };
    // This one's window had already run out before the reboot, so it expired
    // on its own — the distinction the boot rule keeps rather than collapses.
    let first = h.audit_lines_once_written(1);
    assert_eq!(
        first.iter().filter(|l| l["grant"] == 4).count(),
        1,
        "{first:#?}"
    );
    assert_eq!(first[0]["event"], "expired");

    // Start a second daemon over the same state. It finds a grant that is
    // already closed and says nothing new.
    let runtime2 = h.root.join("run2");
    std::fs::create_dir_all(&runtime2).expect("mkdir");
    let mut second = Command::new(env!("CARGO_BIN_EXE_apex-agentd"))
        .env("XDG_RUNTIME_DIR", &runtime2)
        .env("XDG_STATE_HOME", h.root.join("state"))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn");
    let deadline = Instant::now() + Duration::from_secs(10);
    let sock2 = runtime2.join("apex-agentd").join("control.sock");
    while Instant::now() < deadline && UnixStream::connect(&sock2).is_err() {
        std::thread::sleep(Duration::from_millis(25));
    }
    let after = h.audit_lines_once_written(first.len());
    let _ = second.kill();
    let _ = second.wait();
    assert_eq!(after.len(), first.len(), "the ending was written twice: {after:#?}");
}

// ── P0-007 criterion 4 and 5: refusals a connection decides ─────────────────

#[test]
fn renewing_a_grant_that_does_not_exist_is_refused_without_asking_anybody() {
    // The ordering that keeps this test suite prompt-free, asserted. A
    // renewal names a grant first; if there is no active grant by that id the
    // daemon says so and stops, rather than raising a password dialog for
    // something that could not have worked. Teaching people to type their
    // password at dialogs that achieve nothing is its own vulnerability.
    let h = harness!("renew-missing");
    let reply = h.call(r#"{"cmd":"renew_system_grant","id":99,"ttl_ms":900000}"#);
    assert_eq!(reply["reply"], "error", "{reply}");
    let message = reply["message"].as_str().expect("a message");
    assert!(message.contains("no active system-access grant"), "{message}");
    // And it names what to do instead, rather than only what failed.
    assert!(message.contains("start a new session"), "{message}");
}

#[test]
fn revoking_a_grant_that_does_not_exist_is_refused_and_asks_for_no_password() {
    // Revocation asks for nothing even when it succeeds: giving up privilege
    // is free, the same rule P0-016's toggle follows. So this exercises the
    // whole verb, not only its front door.
    let h = harness!("revoke-missing");
    let reply = h.call(r#"{"cmd":"revoke_system_grant","id":99}"#);
    assert_eq!(reply["reply"], "error", "{reply}");
    assert_eq!(reply["kind"], "no_such_request", "{reply}");
}

#[test]
fn a_non_local_caller_is_refused_a_grant_before_polkit_is_asked() {
    // P0-007 criterion 5's precondition, and the reason this whole file can
    // run without a prompt. When the test binary's own connection does not
    // classify as local — a CI runner, a container, anything outside a login
    // session — a `--system-access` run must be refused at the origin check,
    // which happens before the daemon asks polkit anything.
    let h = harness!("not-local");
    if observed_origin(&h).is_some() {
        eprintln!(
            "SKIP: this connection classifies as local, so a grant would reach polkit and \
             raise a prompt. That path is the one a human verifies once."
        );
        return;
    }
    let reply = h.call(
        r#"{"cmd":"run","agent":"generic","cwd":"/tmp","system":"unsafe","sandbox":"unrestricted","ttl_ms":900000,"cols":80,"rows":24,"args":["/bin/true"]}"#,
    );
    assert_eq!(reply["reply"], "error", "{reply}");
    assert_eq!(reply["kind"], "permission_denied", "{reply}");
    let message = reply["message"].as_str().expect("a message");
    // The refusal names what could not be established, not merely that
    // something was denied — the alternative is a machine on which nothing
    // can be granted and nothing says why.
    assert!(
        message.contains("/proc/") || message.contains("could not be established"),
        "{message}"
    );
    // And nothing was issued.
    let listed = h.call(r#"{"cmd":"system_grants"}"#);
    assert_eq!(listed["grants"].as_array().map(|a| a.len()), Some(0), "{listed}");
}

#[test]
fn a_ttl_is_refused_before_anything_is_created_or_anybody_is_asked() {
    // §3.4's bounds, at the daemon rather than at the CLI, because a client
    // that skipped its own checks has to get the same answer. Refused before
    // the worktree, the checkpoint, the reserved id and the password, so a
    // typo in `--ttl` costs nothing and leaves nothing behind.
    let h = harness!("ttl-bounds");
    for (ttl, expect) in [
        // Over the break-glass cap.
        (7_200_000u64, "caps at"),
        // Zero: a grant that has already expired.
        (0, "already expired"),
    ] {
        let reply = h.call(&format!(
            r#"{{"cmd":"run","agent":"generic","cwd":"/tmp","system":"unsafe","sandbox":"unrestricted","ttl_ms":{ttl},"cols":80,"rows":24,"args":["/bin/true"]}}"#
        ));
        assert_eq!(reply["reply"], "error", "{ttl}: {reply}");
        assert_eq!(reply["kind"], "policy_refused", "{ttl}: {reply}");
        let message = reply["message"].as_str().expect("a message");
        assert!(message.contains(expect), "{ttl}: {message}");
    }

    // And break-glass with no `--ttl` at all is refused, because §3.4 asks
    // for the window to be explicit rather than defaulted.
    let reply = h.call(
        r#"{"cmd":"run","agent":"generic","cwd":"/tmp","system":"unsafe","sandbox":"unrestricted","cols":80,"rows":24,"args":["/bin/true"]}"#,
    );
    assert_eq!(reply["reply"], "error", "{reply}");
    assert!(
        reply["message"]
            .as_str()
            .expect("a message")
            .contains("--ttl 15m"),
        "{reply}"
    );

    // Nothing was created by any of the four.
    let listed = h.call(r#"{"cmd":"system_grants"}"#);
    assert_eq!(listed["grants"].as_array().map(|a| a.len()), Some(0), "{listed}");
    let sessions = h.call(r#"{"cmd":"list"}"#);
    assert_eq!(
        sessions["sessions"].as_array().map(|a| a.len()),
        Some(0),
        "{sessions}"
    );
}

#[test]
fn break_glass_inside_a_sandbox_is_refused_by_the_daemon_too() {
    // `bwrap` sets PR_SET_NO_NEW_PRIVS unconditionally, so this pair would
    // run with the flag on while the policy said otherwise. The CLI refuses
    // it in front of the user who typed it; this is the daemon refusing a
    // client that skipped that.
    let h = harness!("confined-break-glass");
    let reply = h.call(
        r#"{"cmd":"run","agent":"generic","cwd":"/tmp","system":"unsafe","sandbox":"project","ttl_ms":900000,"cols":80,"rows":24,"args":["/bin/true"]}"#,
    );
    assert_eq!(reply["reply"], "error", "{reply}");
    assert_eq!(reply["kind"], "policy_refused", "{reply}");
    assert!(
        reply["message"]
            .as_str()
            .expect("a message")
            .contains("no_new_privs"),
        "{reply}"
    );
}

#[test]
fn a_ttl_on_a_session_that_asked_for_no_grant_is_refused_rather_than_ignored() {
    // A `--ttl` with nothing to bound is a user who believes they asked for
    // something they did not. Ignoring it would leave them thinking their
    // ordinary session was time-limited.
    let h = harness!("ttl-without-mode");
    let reply = h.call(
        r#"{"cmd":"run","agent":"generic","cwd":"/tmp","ttl_ms":900000,"cols":80,"rows":24,"args":["/bin/true"]}"#,
    );
    assert_eq!(reply["reply"], "error", "{reply}");
    assert!(
        reply["message"]
            .as_str()
            .expect("a message")
            .contains("--system-access"),
        "{reply}"
    );
}

#[test]
fn the_daemon_speaks_the_revision_that_carries_grants() {
    // The CLI refuses to send `--system-access` or `--ttl` to a daemon below
    // this, because one that predates them drops `ttl_ms` — and a break-glass
    // session that started with no window is the one thing §3.4 forbids
    // outright.
    let h = harness!("version");
    let hello = h.call(r#"{"cmd":"hello"}"#);
    assert_eq!(hello["reply"], "hello");
    let version = hello["version"].as_u64().expect("a version");
    assert!(
        version >= u64::from(apex_agent_core::protocol::SYSTEM_GRANT_VERSION),
        "the daemon speaks {version}, below the grant revision"
    );
}

#[test]
fn an_empty_machine_lists_no_grants_and_says_how_to_ask_for_one() {
    let h = harness!("empty");
    let listed = h.call(r#"{"cmd":"system_grants"}"#);
    assert_eq!(listed["reply"], "system_grants");
    assert_eq!(listed["grants"].as_array().map(|a| a.len()), Some(0));
    assert_eq!(listed["states"].as_array().map(|a| a.len()), Some(0));
}

// ── P0-007 criterion 4, proved from inside a real session ──────────────────

/// The name the test below runs in a child, and the variable that arms it.
const PROBE_TEST: &str = "the_probe_that_runs_inside_a_session";
const PROBE_OUT: &str = "APEX_GRANT_PROBE_OUT";

#[test]
fn the_probe_that_runs_inside_a_session() {
    // Not a test of anything on its own. This is the body that runs INSIDE a
    // managed session, started by the test below with this binary as the
    // session's program — which is how a connection genuinely originating
    // inside a sandbox is produced without a shell script, a helper binary or
    // any assumption about what is installed.
    //
    // Unarmed in an ordinary run, so `cargo test` sees a test that does
    // nothing rather than one that fails outside its fixture.
    let Ok(out) = std::env::var(PROBE_OUT) else {
        return;
    };
    let socket = PathBuf::from(std::env::var("XDG_RUNTIME_DIR").expect("the session was given one"))
        .join("apex-agentd")
        .join("control.sock");

    let ask = |line: &str| -> serde_json::Value {
        let Ok(mut stream) = UnixStream::connect(&socket) else {
            return serde_json::json!({"reply": "unreachable"});
        };
        let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
        let _ = writeln!(stream, "{line}");
        let _ = stream.flush();
        let mut reply = String::new();
        let _ = BufReader::new(&stream).read_line(&mut reply);
        serde_json::from_str(&reply).unwrap_or(serde_json::json!({"reply": "unparseable"}))
    };

    let replies = serde_json::json!({
        // The session id it was told it is, so the report can say whether the
        // probe really was inside one.
        "session": std::env::var("APEX_AGENT_SESSION").ok(),
        // 1. renew somebody's grant. The verb P0-007 criterion 4 names.
        "renew": ask(r#"{"cmd":"renew_system_grant","id":1,"ttl_ms":900000}"#),
        // 2. start a NEW session with break-glass, which is the obvious way
        //    round a renewal check that only looked at renewals.
        "elevate": ask(
            r#"{"cmd":"run","agent":"generic","cwd":"/tmp","system":"unsafe","sandbox":"unrestricted","ttl_ms":900000,"cols":80,"rows":24,"args":["/bin/true"]}"#,
        ),
        // 3. revoke one, which is a smaller change and still not the
        //    session's to make.
        "revoke": ask(r#"{"cmd":"revoke_system_grant","id":1}"#),
        // 4. and the control: a verb a session IS allowed, so a probe that
        //    simply could not reach the socket cannot pass by failing at
        //    everything.
        "control": ask(r#"{"cmd":"system_grants"}"#),
    });
    std::fs::write(out, replies.to_string()).expect("write the probe's answers");
}

#[test]
fn a_session_cannot_ask_for_a_grant_or_renew_one_however_it_asks() {
    // P0-007's fourth criterion, from inside a real session rather than over
    // an `Origin` value.
    //
    // The point of doing it live is that the refusal is not "are you the
    // user". The agent IS the user: same uid, same groups, same environment,
    // and it can put anything it likes in the request. What it cannot do is
    // present a connection whose peer pid does not walk up `/proc` to the pid
    // the daemon recorded when it forked the session — the kernel fills that
    // in at `connect(2)` and a process cannot choose its parent.
    //
    // No polkit anywhere in this: the session check is the FIRST thing
    // `authorise_grant` does, so all three refusals happen before a password
    // could be asked for. That ordering is what makes this test safe to run
    // on a desktop.
    let h = harness!("from-inside");
    let out = h.root.join("probe.json");
    let exe = std::env::current_exe().expect("this test binary");

    // The session's program is this binary, running only the probe above.
    // `generic` takes the program as its first argument.
    let run = serde_json::json!({
        "cmd": "run",
        "agent": "generic",
        // Unrestricted so there is no sandbox in the way of the probe
        // reaching the socket — and so the test is about the ancestry check
        // rather than about bwrap.
        "sandbox": "unrestricted",
        "cwd": "/tmp",
        "cols": 80,
        "rows": 24,
        "args": [
            exe.to_string_lossy(),
            "--exact",
            PROBE_TEST,
            "--test-threads=1",
        ],
        "env": [[PROBE_OUT, out.to_string_lossy()]],
    });
    let started = h.call(&run.to_string());
    if started["reply"] != "session" {
        eprintln!("SKIP: no session in this environment: {started}");
        return;
    }
    let parent_session = started["id"].as_u64().expect("an id");

    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline && !out.exists() {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(out.exists(), "the probe never reported: {started}");
    // Written in one `write`, but the read can still land mid-flush.
    let answers: serde_json::Value = loop {
        let text = std::fs::read_to_string(&out).expect("read");
        if let Ok(v) = serde_json::from_str(&text) {
            break v;
        }
        assert!(Instant::now() < deadline, "the probe's answers never parsed");
        std::thread::sleep(Duration::from_millis(50));
    };

    // The control first: if the probe could not reach the socket at all, the
    // three refusals below would be vacuous.
    assert_eq!(answers["control"]["reply"], "system_grants", "{answers}");
    assert_eq!(
        answers["session"].as_str().and_then(|s| s.parse::<u64>().ok()),
        Some(parent_session),
        "the probe did not run inside the session: {answers}"
    );

    for verb in ["renew", "elevate", "revoke"] {
        let reply = &answers[verb];
        assert_eq!(reply["reply"], "error", "{verb} was not refused: {answers}");
        assert_eq!(
            reply["kind"], "permission_denied",
            "{verb} was refused for the wrong reason: {reply}"
        );
        let message = reply["message"].as_str().expect("a message");
        // The refusal names the session it resolved the connection to, which
        // is the evidence that the ancestry walk is what stopped it and not
        // some unrelated failure.
        assert!(
            message.contains(&format!("session {parent_session}")),
            "{verb}: {message}"
        );
    }
    // And the two grant verbs say WHY it is a session's business to stay out
    // of, rather than only that it was denied.
    assert!(
        answers["renew"]["message"]
            .as_str()
            .expect("a message")
            .contains("§3.3"),
        "{answers}"
    );

    // Nothing was issued by any of it.
    let listed = h.call(r#"{"cmd":"system_grants"}"#);
    assert_eq!(listed["grants"].as_array().map(|a| a.len()), Some(0), "{listed}");
}
