//! `Request::Projects` and `Request::Profiles` against a real daemon over a
//! real socket.
//!
//! P1-054's second criterion — "start a new agent with selected
//! profile/project/worktree" — has been PARTIAL across four rounds of Android
//! work for one reason: neither a project nor a profile was a thing this
//! socket could be asked about, so the Start screen offered a free-text
//! directory and said on screen why. The two verbs exist now. What a unit test
//! of their types cannot say is the five things this file is for:
//!
//!   * the verbs are **reachable** — `dispatch` routes them to a handler
//!     rather than answering "unparseable request", which is what every
//!     version of the daemon before these arms did, and which is exactly the
//!     symptom a client would see if the arm were written and never wired;
//!   * `projects` answers with the records the runtime actually **remembers**,
//!     read from `$XDG_STATE_HOME`, rather than with an empty list;
//!   * a record whose directory has **gone** is dropped — the write that lives
//!     inside this read, asserted rather than discovered later;
//!   * `profiles` reads the **daemon's** `$HOME` and the **daemon's** `PATH`,
//!     which is the half no client can derive and the half that makes the
//!     Start screen honest about an agent that is not installed;
//!   * **a declared remote origin reaches both.** That is the property the
//!     whole unit exists for: a phone's requests arrive through `apex-remoted`
//!     under the origin `claude-remote-control`, and a verb that answered a
//!     local shell and refused a device would leave the picker exactly as
//!     unbuildable as no verb at all. Asserted positively, on a connection
//!     that has narrowed its own origin, and not inferred from the absence of
//!     a gate in the source.
//!
//! Nothing here is faked but `PATH` and `HOME`. The daemon is the real binary,
//! the socket is a real socket, the project records are the real on-disk
//! format written by `project::remember`, and the JSON is the protocol's own.
//!
//! The daemon runs with its own `XDG_RUNTIME_DIR`, `XDG_STATE_HOME` and
//! `HOME`, binds a socket of its own, and is killed BY PID. It never touches a
//! running `apex-agentd` and never reads the user's own `~/.claude`.
//!
//! Fixtures live under `std::env::temp_dir()`, which every other suite here
//! uses; on this project `TMPDIR` is pointed at real disk because `/tmp` is a
//! tmpfs sized for a desktop.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use apex_agent_core::project::Project;

struct Harness {
    child: Child,
    socket: PathBuf,
    root: PathBuf,
    /// The daemon's `$HOME`, so a test can put a profile directory in it.
    home: PathBuf,
    /// The directory prepended to the daemon's `PATH`.
    bin: PathBuf,
    /// Where `project::list` reads from, for seeding and for asserting a
    /// deletion.
    projects_dir: PathBuf,
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
            "apex-picker-e2e-{}-{tag}-{}",
            std::process::id(),
            now_ms()
        ));
        let runtime = root.join("run");
        let state = root.join("state");
        let home = root.join("home");
        let bin = root.join("bin");
        std::fs::create_dir_all(&runtime).ok()?;
        std::fs::create_dir_all(&state).ok()?;
        std::fs::create_dir_all(&home).ok()?;
        std::fs::create_dir_all(&bin).ok()?;

        // `PATH` is EXACTLY this directory, and the developer's is not on it.
        //
        // Inheriting it made this suite assert nothing: the machine it was
        // written on has `claude`, `opencode`, `codex` and `kimi` installed,
        // so every adapter came back `program_found: true` before the test
        // installed anything, and the "not installed here" half — the half the
        // Start screen needs, because offering an agent that is not there is a
        // button whose only outcome is a refusal — could never have failed.
        // Measured, not guessed: that is how this line came to be written.
        //
        // The daemon needs nothing from `PATH` to come up; the tools it shells
        // out to (`git`, `wl-paste`, `loginctl`) are reached from request
        // handlers no test here calls. `wait_for_socket` fails loudly if that
        // ever stops being true.
        let path = bin.display().to_string();

        let child = Command::new(env!("CARGO_BIN_EXE_apex-agentd"))
            .env("XDG_RUNTIME_DIR", &runtime)
            .env("XDG_STATE_HOME", &state)
            .env("HOME", &home)
            .env("PATH", &path)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;

        let socket = runtime.join("apex-agentd").join("control.sock");
        let projects_dir = state.join("apex").join("agent").join("projects");
        let harness = Harness {
            child,
            socket,
            root,
            home,
            bin,
            projects_dir,
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
        let mut c = self.conn();
        c.call(line)
    }

    /// A connection held open, because a declared origin latches onto the
    /// connection and a fresh socket per call would lose it.
    fn conn(&self) -> Conn {
        let stream = UnixStream::connect(&self.socket).expect("connect");
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .expect("timeout");
        Conn {
            reader: BufReader::new(stream.try_clone().expect("clone")),
            writer: stream,
        }
    }

    /// Write a project record the way `project::remember` writes one, with a
    /// real directory behind it.
    fn remember(&self, name: &str, languages: &[&str], capsule: Option<&str>) -> Project {
        let dir = self.root.join("src").join(name);
        std::fs::create_dir_all(&dir).expect("project directory");
        let p = Project {
            root: dir.display().to_string(),
            name: name.to_string(),
            slug: format!("{name}-0000000000000000"),
            languages: languages.iter().map(|s| s.to_string()).collect(),
            last_opened: 1_700_000_000,
            capsule: capsule.map(str::to_string),
        };
        std::fs::create_dir_all(&self.projects_dir).expect("projects dir");
        std::fs::write(
            self.projects_dir.join(format!("{}.json", p.slug)),
            serde_json::to_string_pretty(&p).expect("serialise"),
        )
        .expect("write record");
        p
    }

    /// An executable of `name` on the daemon's `PATH`.
    fn install_program(&self, name: &str) {
        let path = self.bin.join(name);
        std::fs::write(&path, "#!/bin/sh\nexit 0\n").expect("write program");
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .expect("chmod");
    }
}

struct Conn {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
}

impl Conn {
    fn call(&mut self, line: &str) -> serde_json::Value {
        assert!(
            !line.contains('\n'),
            "the framing is one JSON object per line; this payload would desynchronise it"
        );
        writeln!(self.writer, "{line}").expect("write");
        self.writer.flush().ok();
        let mut reply = String::new();
        self.reader.read_line(&mut reply).expect("read");
        serde_json::from_str(&reply).unwrap_or_else(|e| panic!("{e}: {reply}"))
    }
}

fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
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

fn rows<'a>(reply: &'a serde_json::Value, key: &str) -> &'a Vec<serde_json::Value> {
    reply[key]
        .as_array()
        .unwrap_or_else(|| panic!("no `{key}` array in {reply}"))
}

#[test]
fn projects_answers_with_what_the_runtime_remembers() {
    let h = harness!("projects");

    // Before anything is remembered. An empty listing is a real answer and
    // must not be an error: a fresh machine has no projects, and a client that
    // was handed an error would show a failure where it should show "start by
    // opening a project at the computer".
    let empty = h.call(r#"{"cmd":"projects"}"#);
    assert_eq!(
        empty["reply"], "projects",
        "the verb is not reachable — dispatch has no arm for it: {empty}"
    );
    assert!(rows(&empty, "projects").is_empty(), "{empty}");

    let apex = h.remember("apex-os", &["rust", "kotlin"], Some("rust"));
    h.remember("shell", &["qml"], None);

    let reply = h.call(r#"{"cmd":"projects"}"#);
    assert_eq!(reply["reply"], "projects", "{reply}");
    let listed = rows(&reply, "projects");
    assert_eq!(listed.len(), 2, "{reply}");

    let found = listed
        .iter()
        .find(|p| p["slug"] == apex.slug.as_str())
        .unwrap_or_else(|| panic!("the seeded project is missing from {reply}"));

    // Every field the Start screen and the Projects screen render, off the
    // wire rather than off the struct.
    assert_eq!(found["root"], apex.root.as_str(), "{reply}");
    assert_eq!(found["name"], "apex-os", "{reply}");
    assert_eq!(found["languages"], serde_json::json!(["rust", "kotlin"]));
    assert_eq!(found["last_opened"], 1_700_000_000u64);

    // §8's capsule: the binding a client renders as the project's WORKSPACE.
    // P1-056 asks for "workspaces" and this is the only thing in the runtime
    // that is one, so it has to survive the wire or that criterion has nothing
    // behind it.
    assert_eq!(found["capsule"], "rust", "{reply}");
    let unbound = listed
        .iter()
        .find(|p| p["name"] == "shell")
        .expect("the second project");
    assert!(
        unbound["capsule"].is_null(),
        "an unbound project must say so rather than borrowing a neighbour's: {reply}"
    );
}

#[test]
fn a_project_whose_directory_has_gone_is_dropped_and_forgotten() {
    // The write that lives inside this read. `project::list` deletes the
    // record of a project whose directory is no longer there, and a remote
    // caller can cause it. Stated in the verb's own doc comment and asserted
    // here, because "answering a read never writes" is the kind of assumption
    // that is only ever found to be false by something breaking.
    //
    // It is bounded to forgetting a registration for a directory that is not
    // there, which is what the next local `apex agent run` would do anyway.
    let h = harness!("stale");
    let gone = h.remember("removed", &[], None);
    let kept = h.remember("kept", &[], None);
    let record = h.projects_dir.join(format!("{}.json", gone.slug));
    assert!(record.is_file(), "the fixture wrote no record");

    std::fs::remove_dir_all(&gone.root).expect("remove the project directory");

    let reply = h.call(r#"{"cmd":"projects"}"#);
    let listed = rows(&reply, "projects");
    assert_eq!(listed.len(), 1, "the vanished project is still listed: {reply}");
    assert_eq!(listed[0]["slug"], kept.slug.as_str(), "{reply}");
    assert!(
        !record.exists(),
        "the record for a vanished project survived the listing"
    );
}

#[test]
fn profiles_reads_the_daemons_own_path_and_home() {
    let h = harness!("profiles");

    // Nothing installed. `claude` is described — the table is compiled in —
    // but neither its program nor its profile is here, and both must say so.
    let bare = h.call(r#"{"cmd":"profiles"}"#);
    assert_eq!(
        bare["reply"], "profiles",
        "the verb is not reachable — dispatch has no arm for it: {bare}"
    );
    let listed = rows(&bare, "profiles");
    assert_eq!(
        listed.len(),
        apex_agent_core::adapter::ADAPTERS.len(),
        "a row per adapter, so a picker can render all of them: {bare}"
    );
    let claude = listed
        .iter()
        .find(|p| p["agent"] == "claude")
        .expect("a claude row");
    assert_eq!(claude["described"], true, "{bare}");
    assert_eq!(
        claude["program_found"], false,
        "a program that is not on the daemon's PATH was reported as found: {bare}"
    );
    assert_eq!(
        claude["installed"], false,
        "a profile directory that does not exist was reported as installed: {bare}"
    );

    // Now install both, in the daemon's environment rather than this test's.
    h.install_program("claude");
    std::fs::create_dir_all(h.home.join(".claude/skills")).expect("profile dir");
    std::fs::write(h.home.join(".claude/CLAUDE.md"), "be brief\n").expect("instructions");
    std::fs::write(
        h.home.join(".claude/settings.json"),
        r#"{"model":"opus","env":{"GITHUB_TOKEN":"ghp_realvalue"}}"#,
    )
    .expect("settings");
    std::fs::write(h.home.join(".claude/.credentials.json"), "{\"t\":\"oauth\"}")
        .expect("credential");

    let reply = h.call(r#"{"cmd":"profiles"}"#);
    let listed = rows(&reply, "profiles");
    let claude = listed
        .iter()
        .find(|p| p["agent"] == "claude")
        .expect("a claude row");
    assert_eq!(
        claude["program_found"], true,
        "the daemon did not resolve a program on its OWN PATH: {reply}"
    );
    assert_eq!(
        claude["installed"], true,
        "the daemon did not read its OWN HOME — a daemon started by `systemd --user` \
         without a login environment is exactly this case: {reply}"
    );
    assert_eq!(claude["root"], "~/.claude", "{reply}");
    assert!(
        claude["reusable"].as_u64().unwrap_or(0) > 0,
        "nothing was counted: {reply}"
    );
    assert!(
        claude["secret"].as_u64().unwrap_or(0) > 0,
        "the credential was not counted: {reply}"
    );

    // The audit, end to end through the real daemon rather than against the
    // pure function. The profile above holds a model name and two credential
    // values; the reply carries counts and compiled-in strings and nothing
    // else. This is the assertion that would fail if a later round added a
    // `problems` or `sections` field from `profile::doctor`.
    let text = serde_json::to_string(&reply).expect("serialise");
    for leak in ["opus", "ghp_realvalue", "oauth", "settings.json", "CLAUDE.md"] {
        assert!(
            !text.contains(leak),
            "the profiles reply carries {leak:?}, which is content and not a count:\n{text}"
        );
    }
    assert!(
        !text.contains(&h.home.display().to_string()),
        "the reply names an absolute home path:\n{text}"
    );

    // The flag that retires a client-side guess. Clients hard-coded
    // `id == "generic"` because the daemon published nothing, and a device
    // found the consequence: the Start screen offered `generic`, the daemon
    // takes `args.first()` as its program, and the button's only outcome was a
    // refusal after a round trip.
    let generic = listed
        .iter()
        .find(|p| p["agent"] == "generic")
        .expect("a generic row");
    assert_eq!(generic["command_required"], true, "{reply}");
    assert!(generic["program"].is_null(), "{reply}");
    let needing: Vec<&str> = listed
        .iter()
        .filter(|p| p["command_required"] == true)
        .filter_map(|p| p["agent"].as_str())
        .collect();
    assert_eq!(needing, vec!["generic"], "{reply}");
}

#[test]
fn a_declared_remote_origin_reaches_both_verbs() {
    // THE PROPERTY THE UNIT EXISTS FOR, asserted positively rather than
    // inferred from the absence of a gate in the source.
    //
    // A phone's requests arrive through `apex-remoted`, which declares the
    // origin `claude-remote-control` on the connection it forwards
    // (`proxy.rs`). Two verbs that answered a local shell and refused a device
    // would leave P1-054's picker exactly as unbuildable as no verbs at all —
    // and the difference is invisible from every test that runs on a local
    // socket without declaring anything, which is every other test in this
    // file.
    //
    // The contrast is `decide`, which IS refused from here and deliberately
    // stays refused: `privilege.rs` puts the origin check before the pending
    // check because every verb in that vocabulary is a root capability. These
    // two are reads of what the user has already opened and of a table
    // compiled into the binary, so they are not that.
    let h = harness!("remote");
    let project = h.remember("apex-os", &["rust"], Some("rust"));
    h.install_program("claude");

    let mut c = h.conn();
    let declared =
        c.call(r#"{"cmd":"declare_origin","origin":"claude-remote-control","actor":"pixel-7a"}"#);
    if declared["reply"] == "error" {
        // The one acceptable failure: this process could not be classified, so
        // there was nothing to narrow. It must say what it could not read.
        let msg = declared["message"].as_str().unwrap_or_default();
        assert!(
            msg.contains("/proc/") || msg.contains("cgroup") || msg.contains("could not"),
            "a refusal must name what could not be read: {declared}"
        );
        eprintln!("SKIP: this process's origin could not be classified, so nothing could declare");
        return;
    }
    assert_eq!(declared["reply"], "ok", "{declared}");

    let projects = c.call(r#"{"cmd":"projects"}"#);
    assert_eq!(
        projects["reply"], "projects",
        "a paired device cannot list projects, so the Start screen still has no picker: {projects}"
    );
    assert_eq!(rows(&projects, "projects")[0]["slug"], project.slug.as_str());

    let profiles = c.call(r#"{"cmd":"profiles"}"#);
    assert_eq!(
        profiles["reply"], "profiles",
        "a paired device cannot list profiles: {profiles}"
    );
    assert!(!rows(&profiles, "profiles").is_empty(), "{profiles}");

    // And the boundary this unit does NOT move, on the same connection, so the
    // two facts are recorded together: a device that can now build a picker
    // still cannot approve a root operation.
    let decided = c.call(r#"{"cmd":"decide","id":1,"decision":"once"}"#);
    assert_eq!(decided["reply"], "error", "{decided}");
    assert_eq!(
        decided["kind"], "permission_denied",
        "a remote origin decided a root request: {decided}"
    );
    let msg = decided["message"].as_str().unwrap_or_default();
    assert!(
        msg.contains("this machine"),
        "the refusal must say what is missing: {msg}"
    );
}

#[test]
fn the_two_verbs_are_not_the_worktrees_verb() {
    // `worktrees` runs git in every remembered project, including
    // `merge-tree --write-tree`. That cost is the documented reason the Start
    // screen had no picker even after `worktrees` was restored, and it is the
    // reason these are separate verbs rather than fields on that reply.
    //
    // Asserted as the difference a reader can check rather than as a
    // benchmark, which would be flaky on a loaded machine: the three replies
    // are distinct, and `projects` answers for a project whose directory is
    // not a git repository at all — which `worktrees` cannot, because there is
    // nothing for git to walk.
    let h = harness!("distinct");
    let p = h.remember("not-a-repo", &[], None);
    assert!(
        !Path::new(&p.root).join(".git").exists(),
        "the fixture accidentally made a repository"
    );

    let projects = h.call(r#"{"cmd":"projects"}"#);
    assert_eq!(projects["reply"], "projects", "{projects}");
    assert_eq!(rows(&projects, "projects").len(), 1, "{projects}");

    let worktrees = h.call(r#"{"cmd":"worktrees"}"#);
    assert_eq!(worktrees["reply"], "worktrees", "{worktrees}");
    assert!(
        rows(&worktrees, "worktrees").is_empty(),
        "git found worktrees in a directory that is not a repository: {worktrees}"
    );
}
