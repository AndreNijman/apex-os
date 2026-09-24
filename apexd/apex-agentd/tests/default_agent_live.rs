//! `apex agent default <agent>` writes agent.json and returns. The daemon that
//! is already running must honour it without a restart: it used to read the
//! file once at startup, so every later `a` started the agent it booted with
//! (L16, 2026-09-24: default set to codex, `a` started claude).

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

struct Daemon {
    child: Child,
    socket: std::path::PathBuf,
    root: std::path::PathBuf,
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn write_default(config_home: &Path, agent: &str) {
    let dir = config_home.join("apex");
    std::fs::create_dir_all(&dir).expect("config dir");
    std::fs::write(dir.join("agent.json"), format!("{{\"default_agent\":\"{agent}\"}}\n"))
        .expect("write agent.json");
}

fn hello_default(socket: &Path) -> String {
    let mut stream = UnixStream::connect(socket).expect("connect");
    stream.set_read_timeout(Some(Duration::from_secs(10))).expect("timeout");
    writeln!(stream, "{{\"cmd\":\"hello\"}}").expect("write");
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).expect("read");
    let reply: serde_json::Value = serde_json::from_str(line.trim()).expect("json");
    reply["default_agent"].as_str().unwrap_or_default().to_string()
}

#[test]
fn the_default_agent_follows_the_file_without_a_restart() {
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH).expect("clock").as_nanos();
    let root = std::env::temp_dir().join(format!("apex-default-live-{}-{stamp}", std::process::id()));
    let (runtime, state, config) = (root.join("run"), root.join("state"), root.join("config"));
    for d in [&runtime, &state, &config] {
        std::fs::create_dir_all(d).expect("dir");
    }
    write_default(&config, "claude");

    let child = Command::new(env!("CARGO_BIN_EXE_apex-agentd"))
        .env("XDG_RUNTIME_DIR", &runtime)
        .env("XDG_STATE_HOME", &state)
        .env("XDG_CONFIG_HOME", &config)
        .env("HOME", &root)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn apex-agentd");
    let daemon = Daemon { child, socket: runtime.join("apex-agentd").join("control.sock"), root };

    let deadline = Instant::now() + Duration::from_secs(10);
    while UnixStream::connect(&daemon.socket).is_err() {
        assert!(Instant::now() < deadline, "the daemon never opened its socket");
        std::thread::sleep(Duration::from_millis(25));
    }

    assert_eq!(hello_default(&daemon.socket), "claude", "the default it started with");
    write_default(&config, "codex");
    assert_eq!(
        hello_default(&daemon.socket),
        "codex",
        "`apex agent default codex` must reach a daemon that is already running"
    );
}
