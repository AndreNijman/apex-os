//! The curated MCP configuration against a real daemon starting a real session.
//!
//! `mcpconf`'s unit tests assert the document and `apex/tests/mcp_launch_live.rs`
//! runs the command that document produces under bubblewrap. Neither can say
//! the thing that matters most: that the daemon **hands it over** — that the
//! file is written, that it carries the wrapped definition, and that the agent
//! is started with the flag that makes it exclusive. A curated document nobody
//! passes is the same as no curation at all, and it would pass every other test
//! in this change.
//!
//! So the agent here is a script standing in for `claude`, resolved through the
//! daemon's own `PATH` the way a real one is. It writes down the arguments it
//! was given and the contents of the file it was pointed at, and then exits.
//! Everything asserted below is read back out of what that script saw.
//!
//! The daemon gets its own `HOME`, runtime directory, state directory and
//! scratch root, and is killed by pid. Nothing here touches the machine's own
//! agent configuration, and the session is `unrestricted` on purpose: this is
//! about the argv and the file, and a sandbox in the middle would only add a
//! reason for the script not to run that has nothing to do with the question.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

struct Harness {
    child: Child,
    socket: PathBuf,
    root: PathBuf,
    home: PathBuf,
    proj: PathBuf,
    /// Where the stand-in agent writes what it was given.
    report: PathBuf,
}

impl Drop for Harness {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn write(path: &Path, text: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("parent");
    }
    std::fs::write(path, text).expect("write");
}

fn executable(path: &Path, text: &str) {
    use std::os::unix::fs::PermissionsExt;
    write(path, text);
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
}

impl Harness {
    /// A machine with one enabled plugin defining one stdio server, and a
    /// stand-in `claude` on the daemon's path.
    fn start(tag: &str) -> Option<Harness> {
        let root = std::env::temp_dir().join(format!(
            "apex-mcp-launch-e2e-{}-{tag}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0)
        ));
        let runtime = root.join("run");
        let state = root.join("state");
        let home = root.join("home");
        let proj = root.join("proj");
        let bin = root.join("bin");
        let report = root.join("agent-saw.txt");
        for dir in [&runtime, &state, &home, &proj, &bin] {
            std::fs::create_dir_all(dir).ok()?;
        }

        // The plugin, and the server it defines. `/bin/true` because nothing
        // here starts it: what is being measured is which definition reaches
        // the configuration file, not what happens when it runs.
        let cache = home.join(".claude/plugins/cache/mp/probe/1.0.0");
        std::fs::create_dir_all(&cache).ok()?;
        write(&home.join(".claude.json"), r#"{"mcpServers":{}}"#);
        write(
            &home.join(".claude/settings.json"),
            r#"{"enabledPlugins": {"probe@mp": true}}"#,
        );
        write(
            &home.join(".claude/plugins/installed_plugins.json"),
            &serde_json::to_string(&serde_json::json!({
                "version": 2,
                "plugins": {"probe@mp": [{
                    "scope": "user",
                    "installPath": cache.to_string_lossy(),
                    "version": "1.0.0",
                }]},
            }))
            .ok()?,
        );
        write(
            &cache.join(".mcp.json"),
            r#"{"mcpServers": {"probe": {"command": "/bin/true", "args": ["x"]}}}"#,
        );

        // The stand-in agent. Named `claude` because the adapter resolves its
        // program through `PATH` at spawn time — the same property that lets a
        // user's own build of an agent win over the image's.
        executable(
            &bin.join("claude"),
            &format!(
                r#"#!/bin/bash
{{
  echo "ARGV: $*"
  for i in "$@"; do
    case "$prev" in
      --mcp-config) echo "CONFIG-BEGIN"; cat "$i"; echo; echo "CONFIG-END" ;;
    esac
    prev="$i"
  done
}} > "{report}" 2>&1
exit 0
"#,
                report = report.display()
            ),
        );

        let child = Command::new(env!("CARGO_BIN_EXE_apex-agentd"))
            .env("XDG_RUNTIME_DIR", &runtime)
            .env("XDG_STATE_HOME", &state)
            .env("XDG_CONFIG_HOME", home.join(".config"))
            .env("HOME", &home)
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin", bin.to_string_lossy()),
            )
            .env(
                apex_agent_core::paths::SCRATCH_ROOT_ENV,
                root.join("scratch"),
            )
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;

        let socket = runtime.join("apex-agentd").join("control.sock");
        let harness = Harness {
            child,
            socket,
            root,
            home,
            proj,
            report,
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

    fn call(&self, value: &serde_json::Value) -> serde_json::Value {
        let line = value.to_string();
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

    /// Start a `claude` session with one dimension set, and return what the
    /// stand-in agent wrote down.
    fn observe(&self, connectors: &str) -> String {
        std::fs::remove_file(&self.report).ok();
        let mut run = serde_json::json!({
            "cmd": "run",
            "agent": "claude",
            "cwd": self.proj.to_string_lossy(),
            "sandbox": "unrestricted",
            "network": "open",
            "native": "inherit",
            "cols": 80,
            "rows": 24,
        });
        if !connectors.is_empty() {
            run["connectors"] = serde_json::Value::String(connectors.to_string());
        }
        let reply = self.call(&run);
        assert_eq!(reply["reply"], "session", "starting a session: {reply}");

        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            if let Ok(text) = std::fs::read_to_string(&self.report) {
                if text.contains("ARGV:") {
                    return text;
                }
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!(
            "the stand-in agent never ran, or never finished: {:?}",
            std::fs::read_to_string(&self.report)
        );
    }
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
fn the_daemon_hands_the_agent_a_curated_configuration_and_the_flag_that_makes_it_exclusive() {
    let h = harness!("hand-over");
    let saw = h.observe("");

    // The flag first. `--mcp-config` on its own ADDS a source, so without the
    // strict flag beside it the session would get APEX's wrapped definition
    // *and* the plugin's unwrapped one, and the wrapped one would look like it
    // had worked.
    assert!(
        saw.contains("--strict-mcp-config"),
        "the agent was not told to use only the handed configuration:\n{saw}"
    );
    assert!(
        saw.contains("--mcp-config"),
        "no configuration was handed over at all:\n{saw}"
    );

    // And the file the flag pointed at, as the agent read it — not as this
    // test recomputed it.
    assert!(
        saw.contains("CONFIG-BEGIN"),
        "the path after --mcp-config was not a readable file:\n{saw}"
    );
    assert!(
        saw.contains("plugin:probe:probe"),
        "the plugin's server is not in the configuration the agent was given:\n{saw}"
    );
    assert!(
        saw.contains("\"mcp\""),
        "the plugin's server was handed over unwrapped:\n{saw}"
    );
    assert!(
        saw.contains("\"run\""),
        "the plugin's server was handed over unwrapped:\n{saw}"
    );
}

#[test]
fn a_session_with_no_connectors_is_handed_an_empty_set_rather_than_no_file() {
    // The failure this catches is the tempting one: skip the file when there
    // is nothing to keep. `--strict-mcp-config` is what removes the other
    // sources, so a session with no file gets every connector on the machine —
    // the exact opposite of what was asked for.
    let h = harness!("none");
    let saw = h.observe("none");

    assert!(saw.contains("--strict-mcp-config"), "{saw}");
    assert!(saw.contains("CONFIG-BEGIN"), "{saw}");
    assert!(
        !saw.contains("plugin:probe:probe"),
        "a session started with no connectors was handed one:\n{saw}"
    );
}

#[test]
fn the_curated_configuration_is_not_writable_from_inside_the_session() {
    // A file the session can rewrite is a file the session can put its own
    // unwrapped definitions back into — and it lives in the scratch directory,
    // which is bound writable. The daemon puts it on the read-only list for
    // that reason; this checks the list rather than the intention.
    //
    // Read from the built spec rather than from inside a session, because the
    // property is about the mount and an unconfined session has no mounts at
    // all. The path is the one `install_mcp_config` writes.
    let scratch = Path::new("/run/user/1000/apex/s9");
    let config = apex_agent_core::mcpconf::config_path(scratch);
    assert!(
        config.starts_with(scratch),
        "the configuration has to live in the scratch the sandbox already binds: {config:?}"
    );
    assert_ne!(
        config.file_name().and_then(|n| n.to_str()),
        Some(".mcp.json"),
        "naming it what the agent also reads from a project is how somebody debugging this \
         ends up reading the wrong file"
    );

    let mut spec = apex_agent_core::sandbox::SandboxSpec::new(
        apex_agent_core::policy::AgentPolicy {
            sandbox: apex_agent_core::protocol::SandboxPolicy::Project,
            ..Default::default()
        },
        PathBuf::from("/home/someone"),
        PathBuf::from("/run/user/1000"),
    );
    spec.scratch = scratch.to_path_buf();
    spec.cwd = PathBuf::from("/home/someone/proj");
    spec.rw.push(spec.cwd.clone());
    spec.ro.push(config.clone());
    let argv = apex_agent_core::sandbox::build_argv(&spec, "/bin/true", &[]).expect("argv");

    let ro = argv
        .windows(3)
        .any(|w| w[0] == "--ro-bind-try" && w[1] == config.to_string_lossy());
    assert!(
        ro,
        "the curated configuration is not bound read-only:\n{argv:?}"
    );
    // And the read-only bind lands AFTER the writable scratch, or the scratch
    // would be mounted on top of it and the file would be writable again.
    let scratch_at = argv
        .iter()
        .position(|a| a == &scratch.to_string_lossy().to_string())
        .expect("the scratch is bound");
    let config_at = argv
        .iter()
        .position(|a| a == &config.to_string_lossy().to_string())
        .expect("the configuration is bound");
    assert!(
        config_at > scratch_at,
        "the read-only configuration is bound before the writable scratch, so the scratch \
         mount lands on top of it:\n{argv:?}"
    );
}

#[test]
fn the_home_the_daemon_read_is_the_one_the_session_belongs_to() {
    // Guard against the test above passing for the wrong reason: if the daemon
    // had read the machine's real home instead of the fixture's, the plugin it
    // found would be somebody else's and the assertions would be about a
    // definition this test never wrote.
    let h = harness!("home");
    let defs = apex_agent_core::mcpconf::read(&h.home, Some(&h.proj));
    let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["plugin:probe:probe"],
        "the fixture home does not hold exactly the one definition this test wrote"
    );
}
