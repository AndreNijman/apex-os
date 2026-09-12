//! §36's `[identity.agent]`, against a real daemon over its real socket.
//!
//! P2-013. `identity::AgentIdentity::check` is unit-tested beside the type, and
//! a unit test of a pure function cannot say the thing that matters here:
//! whether `apex-agentd` *asks* it, whether it asks about the right directory,
//! and whether the answer reaches the client as a refusal rather than as a
//! session.
//!
//! Four things this file establishes that nothing else can:
//!
//!   1. A session naming an agent the project did not bind is **refused**, with
//!      `policy_refused` and a message naming `apex.toml`.
//!   2. The refusal holds when the session starts in a **subdirectory**. This
//!      is the one that decides whether the check is real: a session's own
//!      working directory is the first thing an agent can change, so a check
//!      keyed on `req.cwd` would be stepped around by `cd src`.
//!   3. A session naming nothing gets the **bound** agent, not the user's
//!      `default_agent` — the difference between a binding and a suggestion.
//!      Asserted on `SessionInfo.agent` coming back from the daemon, with the
//!      user's default deliberately set to something else.
//!   4. An `apex.toml` that cannot be read **refuses**. A file that could not be
//!      read is not a file that says nothing, and starting the user's default
//!      agent because the binding was unreadable is exactly how a session would
//!      unbind the project it is starting in.
//!
//! The daemon gets its own `XDG_RUNTIME_DIR`, `XDG_STATE_HOME` and
//! `XDG_CONFIG_HOME`, so it binds a socket of its own, writes state of its own
//! and reads a configuration of its own. It never touches a running
//! `apex-agentd`, and it is killed BY PID — never by name. Sessions run
//! `--sandbox unrestricted` because `bwrap` is not what is under test.
//!
//! Fixtures live under `/var/tmp`, which is this project's rule for anything a
//! suite creates: `/tmp` here is a tmpfs sized for a desktop.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use apex_agent_core::policy::AgentPolicy;
use apex_agent_core::protocol::{Request, RunRequest, SandboxPolicy};

/// What the user's own configuration says, so that assertion 3 has something to
/// be different from.
///
/// `codex` rather than the compiled-in `claude` for one reason: the assertion
/// is that the BOUND agent wins, and a default that happened to equal the bound
/// one would make the test pass with the binding ignored.
const USER_DEFAULT_AGENT: &str = "codex";

/// What the fixture project binds.
///
/// `generic` because it is the one adapter that runs a program the test names
/// rather than one that has to be installed on the machine — so assertion 3 can
/// check a session that actually started, on a laptop or on a CI runner, with
/// no agent installed anywhere.
const BOUND_AGENT: &str = "generic";

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
        // A fixture left at 0000 by the unreadable-file test would make the
        // removal fail and leave a directory behind every run.
        let _ = restore_mode(&self.root);
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn restore_mode(root: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    for entry in walk(root) {
        let _ = std::fs::set_permissions(&entry, std::fs::Permissions::from_mode(0o700));
    }
    Ok(())
}

fn walk(root: &Path) -> Vec<PathBuf> {
    let mut out = vec![root.to_path_buf()];
    let mut i = 0;
    while i < out.len() {
        if let Ok(entries) = std::fs::read_dir(&out[i]) {
            for entry in entries.flatten() {
                out.push(entry.path());
            }
        }
        i += 1;
    }
    out
}

impl Harness {
    fn start(tag: &str) -> Option<Harness> {
        let root = PathBuf::from("/var/tmp").join(format!(
            "apex-identity-e2e-{}-{tag}-{}",
            std::process::id(),
            now_ms()
        ));
        let runtime = root.join("run");
        let state = root.join("state");
        let config = root.join("config");
        std::fs::create_dir_all(&runtime).ok()?;
        std::fs::create_dir_all(&state).ok()?;
        std::fs::create_dir_all(config.join("apex")).ok()?;
        std::fs::write(
            config.join("apex/agent.json"),
            format!("{{\"default_agent\":\"{USER_DEFAULT_AGENT}\"}}"),
        )
        .ok()?;

        let child = Command::new(env!("CARGO_BIN_EXE_apex-agentd"))
            .env("XDG_RUNTIME_DIR", &runtime)
            .env("XDG_STATE_HOME", &state)
            .env("XDG_CONFIG_HOME", &config)
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

    /// Ask for a session in `cwd`, optionally naming an agent.
    fn run(&self, cwd: &Path, agent: Option<&str>) -> serde_json::Value {
        self.call(&Request::Run(RunRequest {
            agent: agent.map(str::to_string),
            prompt: None,
            args: vec!["/bin/sh".into(), "-c".into(), "sleep 30".into()],
            cwd: cwd.to_string_lossy().into_owned(),
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
        }))
    }
}

fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// A git repository with an `apex.toml` binding an agent, and a subdirectory.
///
/// A repository, because `project::detect` is `git rev-parse --show-toplevel`
/// and that is deliberately the same rule `apex secret` keys a grant on: the
/// binding and the grant have to describe the same project or one of them is
/// about a different directory.
fn project(root: &Path, binds: Option<&str>) -> PathBuf {
    let dir = root.join("project");
    std::fs::create_dir_all(dir.join("src/deep")).expect("fixture tree");
    let ok = Command::new("git")
        .args(["init", "-q"])
        .current_dir(&dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    assert!(ok, "git init failed in the fixture");
    if let Some(binds) = binds {
        std::fs::write(dir.join("apex.toml"), binds).expect("apex.toml");
    }
    dir
}

fn error_of(reply: &serde_json::Value) -> (String, String) {
    (
        reply["kind"].as_str().unwrap_or_default().to_string(),
        reply["message"].as_str().unwrap_or_default().to_string(),
    )
}

/// §36's refusal, over the socket.
#[test]
fn a_session_naming_an_agent_this_project_did_not_bind_is_refused() {
    let Some(h) = Harness::start("refuse") else {
        panic!("the daemon under test would not start");
    };
    let dir = project(
        &h.root,
        Some("[identity.agent]\ndefault = \"generic\"\n"),
    );

    let reply = h.run(&dir, Some("claude"));
    let (kind, message) = error_of(&reply);
    assert_eq!(kind, "policy_refused", "{reply}");
    assert!(message.contains("claude"), "{message}");
    assert!(message.contains("generic"), "{message}");
    assert!(
        message.contains("apex.toml"),
        "the refusal has to name the file the remedy is in: {message}"
    );

    // The control: the bound one is not refused. Without this the test above
    // would pass against a daemon that refused every session.
    let reply = h.run(&dir, Some(BOUND_AGENT));
    assert_eq!(reply["reply"], "session", "the bound agent must start: {reply}");
}

/// The one that decides whether the check is about the project or about the
/// session's own working directory.
///
/// A session can `cd`. If the binding were read out of `req.cwd`, `cd src` —
/// or `cd src/deep`, two levels down — would step around it, and §36's whole
/// sentence would hold only for sessions that never moved.
#[test]
fn the_refusal_holds_from_a_subdirectory_of_the_project() {
    let Some(h) = Harness::start("subdir") else {
        panic!("the daemon under test would not start");
    };
    let dir = project(
        &h.root,
        Some("[identity.agent]\ndefault = \"generic\"\n"),
    );

    for depth in ["src", "src/deep"] {
        let reply = h.run(&dir.join(depth), Some("claude"));
        let (kind, message) = error_of(&reply);
        assert_eq!(
            kind, "policy_refused",
            "starting in {depth} stepped around the binding: {reply}"
        );
        assert!(message.contains("apex.toml"), "{message}");
    }
}

/// A binding displaces the user's own default; a suggestion would not.
#[test]
fn a_session_that_names_no_agent_gets_the_bound_one_and_not_the_users_default() {
    let Some(h) = Harness::start("displace") else {
        panic!("the daemon under test would not start");
    };
    let dir = project(
        &h.root,
        Some("[identity.agent]\ndefault = \"generic\"\n"),
    );

    let reply = h.run(&dir, None);
    assert_eq!(reply["reply"], "session", "{reply}");
    // `Response::Session` is an internally-tagged newtype variant, so
    // `SessionInfo`'s fields sit at the TOP LEVEL of the reply.
    assert_eq!(
        reply["agent"], BOUND_AGENT,
        "the project binds {BOUND_AGENT}; the user's default is {USER_DEFAULT_AGENT}. \
         A binding that did not displace the default would give the second: {reply}"
    );
    assert_ne!(
        BOUND_AGENT, USER_DEFAULT_AGENT,
        "this test proves nothing if the two are the same name"
    );
}

/// A file that could not be read is not a file that says nothing.
#[test]
fn an_apex_toml_that_cannot_be_read_refuses_the_session() {
    use std::os::unix::fs::PermissionsExt;

    // Running as root, DAC is bypassed and a 0000 file is still readable, so
    // the mode would prove nothing. Said out loud rather than skipped in
    // silence — and the malformed-file half below runs either way, so the
    // refusal is still exercised as root.
    let as_root = unsafe { libc::geteuid() } == 0;

    let Some(h) = Harness::start("unreadable") else {
        panic!("the daemon under test would not start");
    };
    let dir = project(
        &h.root,
        Some("[identity.agent]\ndefault = \"generic\"\n"),
    );

    if as_root {
        eprintln!(
            "NOTE: running as root, so the chmod-000 half of this test cannot \
             refuse anything and is not run. The malformed half below is."
        );
    } else {
        std::fs::set_permissions(dir.join("apex.toml"), std::fs::Permissions::from_mode(0o000))
            .expect("chmod");
        let reply = h.run(&dir, Some(BOUND_AGENT));
        let (kind, message) = error_of(&reply);
        assert_eq!(
            kind, "policy_refused",
            "an unreadable binding must refuse, never bind nothing: {reply}"
        );
        assert!(message.contains("apex.toml"), "{message}");
        std::fs::set_permissions(dir.join("apex.toml"), std::fs::Permissions::from_mode(0o600))
            .expect("chmod back");
    }

    // The half that refuses whoever is running it: a file that is there,
    // readable, and not TOML.
    std::fs::write(dir.join("apex.toml"), "[identity.agent\ndefault = ").expect("write");
    let reply = h.run(&dir, Some(BOUND_AGENT));
    let (kind, message) = error_of(&reply);
    assert_eq!(
        kind, "policy_refused",
        "a malformed binding must refuse, never bind nothing: {reply}"
    );
    assert!(message.contains("apex.toml"), "{message}");
}

/// A project that binds nothing is the machine it always was.
///
/// The regression this file could most easily introduce: a check that refused
/// every session in every directory without an `apex.toml` would pass all four
/// tests above.
#[test]
fn a_project_with_no_binding_starts_the_agent_it_was_asked_for() {
    let Some(h) = Harness::start("unbound") else {
        panic!("the daemon under test would not start");
    };
    let dir = project(&h.root, None);

    let reply = h.run(&dir, Some(BOUND_AGENT));
    assert_eq!(reply["reply"], "session", "{reply}");
    assert_eq!(reply["agent"], BOUND_AGENT, "{reply}");
}
