//! `apex agent handoff` against a real daemon, in a real git project.
//!
//! §16, roadmap P1-024. The unit tests beside `apex_agent_core::handoff` assert
//! how a packet RENDERS, over a struct built by hand. They cannot say anything
//! about the four claims that make the verb usable, because every one of them
//! is about the world outside that struct:
//!
//!   * the packet lands INSIDE THE PROJECT and not in the runtime's state
//!     directory. This is the claim with teeth: the receiving session is
//!     sandboxed, and under `--sandbox project` the rest of `$HOME` is not
//!     hidden but absent, so a packet under `$XDG_STATE_HOME` would be handed
//!     to an agent that cannot open it — and the failure would look like the
//!     agent ignoring its instructions.
//!   * `.apex/` reaches `.git/info/exclude`, so a handoff does not leave a
//!     stray untracked file in somebody's repository.
//!   * an unknown target agent is refused BEFORE anything is written. A packet
//!     for an agent that cannot be launched is a file nobody will ever read.
//!   * a launch that fails does not report success, and does not take the
//!     packet down with it.
//!
//! And one that is the reason this file exists at all: THE TWO GRANT FAMILIES.
//! A project grant is matched on the project root alone, so the incoming agent
//! inherits it; a system-access grant is bound to the outgoing session, so it
//! does not. The packet has to say which is which, and the only way to know it
//! is reading the right one is to seed a grant into a real daemon and read the
//! document back.
//!
//! ## What this file does not touch
//!
//! The daemon is started with its own `XDG_RUNTIME_DIR`, `XDG_STATE_HOME` and
//! `HOME`, all under a fixture directory, so it binds a socket of its own and
//! writes state of its own. It never touches a running `apex-agentd` — five
//! other agents use the live one — and it is killed BY PID, never by name.
//! Sessions run `--sandbox unrestricted` because `bwrap` is not the thing
//! under test.
//!
//! `--to codex` ALWAYS carries `--no-start` here. `codex` is installed on this
//! machine, and a test that omitted the flag would launch a real agent
//! session. The failed-launch arm uses `generic` instead, whose adapter
//! carries `program: ""` — registered, so it passes target validation, and
//! unable to start with no argv, which is exactly the branch under test.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

use apex_agent_core::handoff::FIELDS;
use apex_agent_core::policy::AgentPolicy;
use apex_agent_core::protocol::{Request, RunRequest, SandboxPolicy};

const APEX: &str = env!("CARGO_BIN_EXE_apex");

/// The daemon binary, beside the `apex` this test was built with.
///
/// `CARGO_BIN_EXE_apex-agentd` is defined only for tests in the package that
/// declares that binary, and the thing under test here is the CLI, so the path
/// is derived from a sibling instead.
///
/// It PANICS when the daemon is not there rather than returning and letting
/// the test pass. `cargo test --locked` over the workspace always builds it,
/// and a fixture that cannot tell "not built" from "passed" is a defect this
/// repository has already shipped once — see the comment in
/// `apex-agentd/tests/session_input.rs` about four tests reporting ok in
/// 0.13 s having asserted nothing.
fn daemon_bin() -> PathBuf {
    let p = Path::new(APEX)
        .parent()
        .expect("a built test binary has a directory")
        .join("apex-agentd");
    assert!(
        p.exists(),
        "{} is not built, so this test cannot run. `cargo test --locked` from apexd/ \
         builds it; after `cargo test -p apex` alone it will be missing, and a skip \
         here would be indistinguishable from a pass.",
        p.display()
    );
    p
}

struct Harness {
    child: Child,
    socket: PathBuf,
    root: PathBuf,
    state: PathBuf,
    repo: PathBuf,
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
            "apex-handoff-e2e-{}-{tag}-{}",
            std::process::id(),
            now_ms()
        ));
        let runtime = root.join("run");
        let state = root.join("state");
        let home = root.join("home");
        let repo = root.join("project");
        for d in [&runtime, &state, &home, &repo] {
            std::fs::create_dir_all(d).ok()?;
        }

        // A real repository, because `handoff` refuses to write a packet
        // anywhere a sandboxed session could not read it and finds the place
        // with `git rev-parse --show-toplevel`.
        git(&repo, &["init", "-q", "-b", "main"])?;
        git(&repo, &["config", "user.email", "t@example.invalid"])?;
        git(&repo, &["config", "user.name", "Handoff Test"])?;
        std::fs::write(repo.join("README.md"), "before\n").ok()?;
        git(&repo, &["add", "README.md"])?;
        git(&repo, &["commit", "-qm", "first"])?;

        let child = Command::new(daemon_bin())
            .env("XDG_RUNTIME_DIR", &runtime)
            .env("XDG_STATE_HOME", &state)
            .env("HOME", &home)
            // Without this, `paths::scratch_root` falls back to the fixed
            // `/tmp/apex-agent`, which is the LIVE daemon's scratch root: a
            // test daemon would create session directories in it, under ids
            // that collide with the real ones. Every other daemon fixture sets
            // it (apex-agentd/tests/hook_bridge.rs:66); this one did not.
            .env(
                apex_agent_core::paths::SCRATCH_ROOT_ENV,
                root.join("scratch"),
            )
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;

        let socket = runtime.join("apex-agentd").join("control.sock");
        let h = Harness {
            child,
            socket,
            root,
            state,
            repo,
        };
        h.wait_for_socket().then_some(h)
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
    fn call(&self, req: &Request) -> serde_json::Value {
        let line = serde_json::to_string(req).expect("serialise");
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

    /// A live session in the fixture repository. Returns its id and the
    /// project root THE DAEMON reports for it.
    ///
    /// The daemon's own value is returned rather than `self.repo` because it
    /// is the key the grant lookup uses. A test that seeded a grant under a
    /// path spelled differently — `/tmp` against a resolved `/private/tmp`,
    /// say — would be asserting that a lookup misses, and would have passed
    /// just as happily against the defect this file was written for.
    fn session(&self) -> Option<(u32, String)> {
        let reply = self.call(&Request::Run(RunRequest {
            agent: Some("generic".into()),
            prompt: None,
            // It must WRITE something. A session with a silent transcript
            // exercises the "the transcript is empty" arm and never the one
            // that carries the tail, and the tail is most of what §16 is for.
            args: vec![
                "/bin/sh".into(),
                "-c".into(),
                "echo mismatched types in src/main.rs; sleep 300".into(),
            ],
            cwd: self.repo.to_string_lossy().into_owned(),
            policy: AgentPolicy {
                sandbox: SandboxPolicy::Unrestricted,
                ..AgentPolicy::default()
            },
            request_origin: None,
            worktree: None,
            checkpoint: false,
            ttl_ms: None,
            cols: 80,
            rows: 24,
            env: vec![],
            // An ordinary session: the fixture asserts on the project root and
            // the grants the daemon reports for it, and a disposable capsule
            // reports neither the same way.
            disposable: false,
            copy_out: None,
            // This fixture asks for no elevation at all, so there is nothing
            // for a second factor to authorise.
            second_factor: None,
        }));
        if reply["reply"] != "session" {
            // A daemon that will not start a session at all is a broken
            // fixture on this machine, not a failing assertion. Said out loud.
            eprintln!("SKIP: the daemon would not start a session: {reply}");
            return None;
        }
        // `Response::Session` is an internally-tagged NEWTYPE variant, so
        // SessionInfo's fields are at the TOP LEVEL of the reply.
        let id = reply["id"]
            .as_u64()
            .unwrap_or_else(|| panic!("a session reply carrying no id: {reply}"));
        let project = reply["project"]
            .as_str()
            .unwrap_or_else(|| panic!("a session reply carrying no project: {reply}"))
            .to_string();
        Some((id as u32, project))
    }

    /// Wait until the session's transcript holds `marker`.
    ///
    /// The session writes as soon as it starts, but the daemon captures the
    /// PTY on its own schedule, so a handoff taken immediately would race the
    /// capture and record "the transcript is empty" — a real answer for the
    /// wrong reason, and one that would make this test pass while proving
    /// nothing about the tail being carried. Returns whether it arrived.
    fn transcript_holds(&self, id: u32, marker: &str) -> bool {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            let reply = self.call(&Request::Logs {
                id,
                bytes: 64 * 1024,
            });
            if reply["text"].as_str().unwrap_or_default().contains(marker) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        false
    }

    /// Run the CLI under test against this fixture's daemon.
    fn apex(&self, args: &[&str]) -> Output {
        Command::new(APEX)
            .args(args)
            .current_dir(&self.repo)
            .env("XDG_RUNTIME_DIR", self.root.join("run"))
            .env("XDG_STATE_HOME", &self.state)
            .env("HOME", self.root.join("home"))
            .output()
            .expect("run apex")
    }

    /// Record a system-access grant against `session`, the way an approved
    /// break-glass request would have.
    ///
    /// Seeded on disk rather than obtained for real, because obtaining one
    /// needs a human at a polkit prompt. The daemon lists these from the
    /// state directory on every query, so a file written after it started is
    /// still seen. The shape is the one `apex-agentd/tests/system_grants.rs`
    /// already seeds.
    fn grant_system(&self, id: u32, session: u32) {
        let dir = self.state.join("apex/agent/system-grants");
        std::fs::create_dir_all(&dir).expect("grants dir");
        let grant = serde_json::json!({
            "id": id,
            "kind": "break_glass",
            "session": session,
            "agent": "generic",
            "project": null,
            "capabilities": [],
            "issued_ms": now_ms() as u64 - 60_000,
            "expires_ms": now_ms() as u64 + 3_600_000,
            "boot_id": "not-the-boot-this-test-is-running-on",
            "request_origin": "local-terminal",
            "authenticated_by": "org.apexos.agent.break-glass",
        });
        std::fs::write(
            dir.join(format!("{id}.json")),
            serde_json::to_string_pretty(&grant).expect("serialise"),
        )
        .expect("write grant");
    }

    /// Pre-approve a privilege verb for a project, the way an approved
    /// request would have.
    fn grant_project(&self, project: &str, key: &str) {
        let dir = self.state.join("apex/agent");
        std::fs::create_dir_all(&dir).expect("state dir");
        let body = serde_json::json!({ "projects": { project: [key] } });
        std::fs::write(
            dir.join("grants.json"),
            serde_json::to_string_pretty(&body).expect("serialise"),
        )
        .expect("write grants");
    }
}

fn git(dir: &Path, args: &[&str]) -> Option<()> {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("HOME", dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .ok()?;
    out.status.success().then_some(())
}

fn git_out(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("HOME", dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .expect("git");
    String::from_utf8_lossy(&out.stdout).to_string()
}

fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// Skip out loud rather than passing silently.
macro_rules! fixture {
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

macro_rules! session {
    ($h:expr) => {
        match $h.session() {
            Some(s) => s,
            None => return,
        }
    };
}

#[test]
fn the_packet_is_written_into_the_project_and_stdout_is_its_path() {
    let h = fixture!("write");
    let (id, _project) = session!(h);
    assert!(
        h.transcript_holds(id, "mismatched types"),
        "the session never wrote to its terminal, so this test would be asserting \
         against an empty transcript for a reason that has nothing to do with §16"
    );

    let out = h.apex(&["agent", "handoff", &id.to_string(), "--to", "codex", "--no-start"]);
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(
        out.status.success(),
        "handoff failed: {stderr}\n{}",
        String::from_utf8_lossy(&out.stdout)
    );

    // stdout is the path and nothing else, so the verb composes.
    let printed = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let path = PathBuf::from(&printed);
    assert!(
        printed.lines().count() == 1 && path.is_absolute(),
        "stdout should be one absolute path: {printed:?} (stderr: {stderr})"
    );
    assert!(
        printed.ends_with(&format!(".apex/handoff/session-{id}-to-codex.md")),
        "unexpected packet path: {printed}"
    );
    assert!(path.is_file(), "no packet at {printed}");

    // THE CLAIM WITH TEETH. Inside the project, where a sandboxed session can
    // read it; not in the runtime's state directory, where it could not.
    let repo_real = git_out(&h.repo, &["rev-parse", "--show-toplevel"])
        .trim()
        .to_string();
    assert!(
        path.starts_with(&repo_real),
        "the packet is outside the project: {printed} is not under {repo_real}"
    );
    assert!(
        !path.starts_with(&h.state),
        "the packet is in the runtime state directory, which a sandboxed session \
         cannot read: {printed}"
    );

    let md = std::fs::read_to_string(&path).expect("read packet");

    // Every §16 heading, whether or not this build can fill it. A heading that
    // stopped being rendered would look, to the receiving agent, exactly like
    // a field the outgoing session had nothing to say about.
    for f in FIELDS {
        assert!(md.contains(&format!("## {f}")), "no `## {f}` heading in:\n{md}");
    }
    // The body under a heading, up to the next one.
    let section = |f: &str| -> String {
        let at = md
            .find(&format!("## {f}\n"))
            .unwrap_or_else(|| panic!("no `## {f}` heading"));
        let body = &md[at + f.len() + 4..];
        body[..body.find("\n## ").unwrap_or(body.len())].to_string()
    };

    // And the three with no producer say WHY, rather than leaving a blank the
    // next agent would read as "there was no plan".
    for f in ["goal", "plan", "memory project slug"] {
        let body = section(f);
        assert!(
            body.contains("Not supplied") && body.trim().len() > 30,
            "the `{f}` section is blank or unexplained: {body:?}"
        );
    }

    // `test state` is NOT one of them any more, and this is the assertion that
    // says so end to end rather than in a unit test against a constructed
    // packet: the daemon was asked, it answered, and the answer reached the
    // document. The fixture never runs a suite, so the true answer here is
    // that APEX has observed none — which the packet must state as an
    // observation and not as "this build cannot tell you", the two being the
    // distinction the whole module exists to keep.
    let tests = section("test state");
    assert!(
        !tests.contains("Not supplied"),
        "the runtime has a per-worktree test record now, so `test state` must not be \
         rendered as a field this build cannot supply: {tests:?}"
    );
    assert!(
        tests.contains("has not observed a test run"),
        "the `test state` section does not carry the daemon's observation: {tests:?}"
    );
    assert!(
        !tests.contains("no per-worktree test status"),
        "the packet still claims this build has no test record: {tests:?}"
    );

    // The outgoing session is identified, and the transcript is carried and
    // labelled as evidence rather than as the summary §16 asks for and
    // nothing in this runtime can make.
    assert!(md.contains(&format!("session {id}")), "{md}");
    assert!(md.contains("It is not a summary"), "{md}");
    assert!(
        md.contains("mismatched types in src/main.rs"),
        "the outgoing session's terminal output is not in the packet:\n{md}"
    );

    // .apex/ is excluded through git's own private file, not the user's
    // .gitignore, and the repository is left clean.
    let exclude = std::fs::read_to_string(h.repo.join(".git/info/exclude")).unwrap_or_default();
    assert!(
        exclude.contains(".apex"),
        ".apex/ never reached .git/info/exclude: {exclude:?}"
    );
    let status = git_out(&h.repo, &["status", "--porcelain"]);
    assert!(
        status.trim().is_empty(),
        "the handoff left the repository dirty:\n{status}"
    );
    assert!(
        !std::fs::read_to_string(h.repo.join(".gitignore"))
            .unwrap_or_default()
            .contains(".apex"),
        "the user's .gitignore was edited; the exclude belongs in .git/info/exclude"
    );
}

#[test]
fn a_project_grant_is_reported_as_inherited_and_a_session_grant_is_not() {
    // THE DEFECT THIS FILE WAS WRITTEN FOR. `request::Grants::allows` matches
    // a grant on the project root ALONE — no session, no expiry — and a
    // handoff starts the incoming agent in the outgoing session's cwd, so it
    // is the same project and the grant applies to it. An earlier draft of
    // the packet printed "a grant belongs to the session that was given it"
    // over exactly this list, which is the opposite of what the runtime does,
    // and told the next agent to go and ask a human for authority it had
    // already inherited.
    let h = fixture!("grants");
    let (id, project) = session!(h);
    h.grant_project(&project, "install:clang");

    let out = h.apex(&["agent", "handoff", &id.to_string(), "--to", "codex", "--no-start"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let md = std::fs::read_to_string(String::from_utf8_lossy(&out.stdout).trim())
        .expect("read packet");

    let project_half = {
        let at = md
            .find("### pre-approved in this project")
            .expect("no project-grant subsection");
        let rest = &md[at..];
        rest[..rest.find("### held by").expect("no system subsection")].to_string()
    };
    assert!(
        project_half.contains("install:clang"),
        "the grant the daemon holds for this project is missing from the packet. \
         This is what a mismatched lookup key looks like:\n{project_half}"
    );
    assert!(
        project_half.contains("they apply to you now"),
        "a project grant must be reported as already in force:\n{project_half}"
    );

    let system_half = &md[md.find("### held by the outgoing session").expect("subsection")..];
    assert!(
        system_half.contains("no system-access grant"),
        "the outgoing session held none, and the packet should say so:\n{system_half}"
    );
    // The two families must not be described by one sentence: whichever it
    // chose would be false about the other.
    assert!(
        !project_half.contains("bound to the session"),
        "session-bound wording leaked onto the inherited grants:\n{project_half}"
    );
}

#[test]
fn only_the_outgoing_sessions_system_grants_are_listed() {
    // A grant belonging to a sibling session is not this handoff's business,
    // and listing it would read to the incoming agent as something the work
    // had been given. `SystemGrant.session` is the field that makes a grant a
    // grant rather than a standing root capability (§3.3), so it is also what
    // decides which ones belong in the packet.
    //
    // Two grants, one for each side of the filter, because a filter is only
    // observable when something is on the wrong side of it: seeding just the
    // matching grant would pass identically with no filter at all.
    let h = fixture!("sysgrants");
    let (id, _project) = session!(h);
    h.grant_system(1, id);
    h.grant_system(2, id + 500);

    let out = h.apex(&["agent", "handoff", &id.to_string(), "--to", "codex", "--no-start"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let md = std::fs::read_to_string(String::from_utf8_lossy(&out.stdout).trim())
        .expect("read packet");
    let system_half = &md[md.find("### held by the outgoing session").expect("subsection")..];

    assert!(
        system_half.contains("#1 break-glass"),
        "the outgoing session's own grant is missing:\n{system_half}"
    );
    assert!(
        !system_half.contains("#2 break-glass"),
        "another session's grant was reported as this session's:\n{system_half}"
    );
    // And it is described as not transferring, which is the whole reason it is
    // in a section of its own.
    assert!(
        system_half.contains("bound to the session"),
        "{system_half}"
    );
    // The daemon's own state sentence is carried rather than re-derived: the
    // state depends on the running kernel's boot id, and this grant was
    // stamped with another boot's.
    assert!(
        system_half.contains("ended-at-reboot") || system_half.contains("reboot"),
        "the daemon's state for the grant did not reach the packet:\n{system_half}"
    );
}

#[test]
fn an_unknown_target_is_refused_before_anything_is_written() {
    // A packet for an agent the runtime cannot launch is a file nobody will
    // ever read, so the target is validated first. Asserted by looking for
    // the artefact, not by reading the code: the ordering is the claim.
    let h = fixture!("badtarget");
    let (id, _project) = session!(h);

    let out = h.apex(&["agent", "handoff", &id.to_string(), "--to", "nosuchagent"]);
    assert!(
        !out.status.success(),
        "an unknown agent was accepted: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("nosuchagent") && stderr.contains("adapters"),
        "the refusal should name the target and where to find the real list: {stderr}"
    );
    assert!(
        !h.repo.join(".apex/handoff").exists(),
        "a packet was written for an agent that cannot be launched"
    );
}

#[test]
fn a_launch_that_fails_says_so_and_leaves_the_packet_behind() {
    // The `generic` adapter is registered — so it passes target validation —
    // and carries `program: ""`, so a session with no argv cannot start. That
    // is the branch: the document is on disk and worth having, and the exit
    // status must not claim the handoff completed.
    let h = fixture!("nostart");
    let (id, _project) = session!(h);

    let out = h.apex(&["agent", "handoff", &id.to_string(), "--to", "generic"]);
    let printed = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();

    assert!(
        !printed.is_empty() && Path::new(&printed).is_file(),
        "the packet should survive a failed launch: stdout {printed:?}, stderr {stderr}"
    );
    assert_eq!(
        out.status.code(),
        Some(1),
        "a session that did not start must not report success: {stderr}"
    );
    assert!(
        stderr.contains("did not start"),
        "the failure should say the session did not start: {stderr}"
    );
    // And it must NAME the packet, because that is the thing the user still
    // has. Returning the launch error through `?` would print a true message
    // that left them with no idea a document had been written.
    assert!(
        stderr.contains(&printed),
        "the failure message does not name the packet that survived it: {stderr}"
    );
    assert!(
        stderr.contains("nothing is lost"),
        "the failure should say how to carry on by hand: {stderr}"
    );
}

#[test]
fn changed_files_name_the_base_they_were_measured_against() {
    // The session is started without `--checkpoint`, so the comparison falls
    // back to the project's most recent checkpoint. A file list measured
    // against a base the packet never named is a number without a unit: the
    // next agent cannot tell whether README.md changed during this session's
    // work or during somebody else's, last week. So the `checkpoint` section
    // has to name the fallback and say what it means.
    let h = fixture!("base");
    let (id, _project) = session!(h);

    let cp = h.apex(&["agent", "checkpoint", "before-handoff"]);
    assert!(
        cp.status.success(),
        "could not make a checkpoint: {}",
        String::from_utf8_lossy(&cp.stderr)
    );
    std::fs::write(h.repo.join("README.md"), "after\n").expect("edit");

    let out = h.apex(&["agent", "handoff", &id.to_string(), "--to", "codex", "--no-start"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let md = std::fs::read_to_string(String::from_utf8_lossy(&out.stdout).trim())
        .expect("read packet");

    assert!(
        md.contains("README.md"),
        "the edited file is missing from the changed list:\n{md}"
    );
    let at = md.find("## checkpoint\n").expect("checkpoint section");
    let section = &md[at..];
    let section = &section[..section.find("\n## ").unwrap_or(section.len())];
    assert!(
        section.contains("before-handoff"),
        "the fallback base is not named, so the file list above has no base:\n{section}"
    );
    assert!(
        section.contains("this session did not do"),
        "the packet should say the fallback base may include other work:\n{section}"
    );
}

#[test]
fn the_task_spelling_from_section_sixteen_writes_the_packet_for_the_tasks_session() {
    // §16's Target is `apex task handoff <task-id> codex`, and until now the
    // only way to get a packet was `apex agent handoff <session-id>`. This
    // asserts the spec's own spelling reaches the same document — by looking at
    // the artefact, not by reading the delegation: the packet is named after
    // the SESSION, so a `task handoff` that resolved the task to the wrong
    // session (or invented a packet of its own from the task record) produces a
    // file with a different name and a different body.
    let h = fixture!("taskverb");
    let (id, _project) = session!(h);
    assert!(
        h.transcript_holds(id, "mismatched types"),
        "the session never wrote to its terminal, so the carried-transcript \
         assertion below would be vacuous"
    );

    let new = h.apex(&["task", "new", "handoff-demo"]);
    assert!(
        new.status.success(),
        "could not create the task: {}",
        String::from_utf8_lossy(&new.stderr)
    );

    let out = h.apex(&["task", "handoff", "handoff-demo", "codex", "--no-start"]);
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(out.status.success(), "task handoff failed: {stderr}");

    // Same stdout contract as the agent form: the path, alone, so it composes.
    let printed = String::from_utf8_lossy(&out.stdout).trim().to_string();
    assert!(
        printed.ends_with(&format!(".apex/handoff/session-{id}-to-codex.md")),
        "the task verb did not resolve to this task's session: {printed:?} (stderr {stderr})"
    );
    let path = PathBuf::from(&printed);
    assert!(path.is_file(), "no packet at {printed}");

    // And it is the WHOLE document, not a thinner one written from the task
    // record: every §16 heading, the outgoing session named, the transcript
    // tail carried, and the test state that only the daemon can answer.
    let md = std::fs::read_to_string(&path).expect("read packet");
    for f in FIELDS {
        assert!(md.contains(&format!("## {f}")), "no `## {f}` heading in:\n{md}");
    }
    assert!(md.contains(&format!("session {id}")), "{md}");
    assert!(
        md.contains("mismatched types in src/main.rs"),
        "the outgoing session's terminal output is not in the packet:\n{md}"
    );
    assert!(
        md.contains("has not observed a test run"),
        "the daemon's test-state observation did not reach the packet, so the task \
         verb is not going through the same producer:\n{md}"
    );

    // The refusal path's promise, kept: it said which session it chose.
    assert!(
        stderr.contains(&format!("session {id}")),
        "the command did not say which session it handed off: {stderr}"
    );
}

#[test]
fn a_task_with_two_sessions_is_refused_and_both_ids_are_named() {
    // The case a developer with one session open never sees. Picking either
    // one would be a guess about which work the user meant to hand over, and
    // because the packet is named after the session it picked, the wrong guess
    // is a document that looks entirely correct.
    //
    // Two sessions in the same root, because a refusal is only observable when
    // there is something to be ambiguous about: one session would take the
    // success path and prove nothing about the guard.
    let h = fixture!("tasktwo");
    let (first, _) = session!(h);
    let (second, _) = session!(h);
    assert_ne!(first, second, "the fixture started one session twice");

    let new = h.apex(&["task", "new", "two-sessions"]);
    assert!(
        new.status.success(),
        "could not create the task: {}",
        String::from_utf8_lossy(&new.stderr)
    );

    let out = h.apex(&["task", "handoff", "two-sessions", "codex", "--no-start"]);
    assert!(
        !out.status.success(),
        "an ambiguous handoff was accepted: {}",
        String::from_utf8_lossy(&out.stdout)
    );
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    assert!(
        stderr.contains(&first.to_string()) && stderr.contains(&second.to_string()),
        "the refusal names neither or only one of the two sessions, so the user cannot \
         act on it: {stderr}"
    );
    assert!(
        stderr.contains("apex agent handoff"),
        "the refusal should name the command that resolves it: {stderr}"
    );
    // Refused BEFORE anything was written, the same ordering the unknown-target
    // test asserts for the agent form.
    assert!(
        !h.repo.join(".apex/handoff").exists(),
        "a packet was written for an ambiguous handoff"
    );
}
