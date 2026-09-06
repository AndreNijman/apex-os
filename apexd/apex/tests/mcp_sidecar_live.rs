//! The MCP sidecar with `bwrap` actually running, not just its argv.
//!
//! The unit tests in `mcp::sidecar` assert the argv, which is what stops a
//! policy dimension from silently disappearing. They cannot say whether the
//! flags do what they are believed to do — that a masked home is really
//! unreadable, that `--unshare-net` really stops a connection to a listener
//! that is definitely there, that `--clearenv` really drops a variable the
//! parent had. Those are facts about the kernel and about bubblewrap 0.12, and
//! the only way to have them is to run one.
//!
//! Nothing here touches the agent runtime, the secret service or the user's
//! configuration. Every path is under a fixture directory this test made.
//!
//! ## The nesting property
//!
//! An MCP server started by a confined agent is a sandbox inside a sandbox, and
//! the test that matters is that the inner one cannot undo the outer: a policy
//! that says `network = true` inside a session that has no network still has
//! none, because a network namespace cannot be un-shared upward. That is
//! checked here by running the sidecar inside an outer `bwrap --unshare-net`.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Where `bwrap` is. The same fixed path the sandbox module uses, and for the
/// same reason: a `PATH` lookup would let a shadowing binary decide what this
/// test measured.
const BWRAP: &str = "/usr/bin/bwrap";

fn fixture(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "apex-mcp-live-{}-{tag}-{:?}",
        std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
    ));
    std::fs::create_dir_all(&dir).expect("fixture");
    dir
}

/// The sidecar's argv for a policy, built the way `apex mcp run` builds it.
///
/// A copy of the wiring rather than a call into it: `apex` is a binary crate,
/// so an integration test cannot reach `mcp::sidecar` directly. What is being
/// measured is the *flags*, and the flags are asserted against the real ones in
/// the unit tests — this file exists to find out what they do.
fn sandbox_argv(
    user_home: &Path,
    server_home: &Path,
    runtime_dir: &Path,
    network: bool,
    program: &str,
    args: &[&str],
) -> Vec<String> {
    let mut a: Vec<String> = vec![
        BWRAP.into(),
        "--ro-bind".into(), "/".into(), "/".into(),
        "--proc".into(), "/proc".into(),
        "--dev".into(), "/dev".into(),
        "--tmpfs".into(), "/tmp".into(),
        "--tmpfs".into(), user_home.to_string_lossy().into_owned(),
        "--tmpfs".into(), "/run".into(),
        "--tmpfs".into(), runtime_dir.to_string_lossy().into_owned(),
        "--bind-try".into(),
        server_home.to_string_lossy().into_owned(),
        server_home.to_string_lossy().into_owned(),
        "--unshare-pid".into(),
        "--unshare-ipc".into(),
        "--unshare-uts".into(),
    ];
    if !network {
        a.push("--unshare-net".into());
    }
    a.push("--clearenv".into());
    for (k, v) in [
        ("HOME", server_home.to_string_lossy().into_owned()),
        ("PATH", "/usr/bin:/bin".to_string()),
    ] {
        a.push("--setenv".into());
        a.push(k.into());
        a.push(v);
    }
    a.push("--chdir".into());
    a.push(server_home.to_string_lossy().into_owned());
    a.push("--die-with-parent".into());
    a.push("--".into());
    a.push(program.into());
    a.extend(args.iter().map(|s| (*s).to_string()));
    a
}

fn run(argv: &[String]) -> (i32, String) {
    let out = Command::new(&argv[0])
        .args(&argv[1..])
        .output()
        .expect("run bwrap");
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    (out.status.code().unwrap_or(-1), text)
}

#[test]
fn a_confined_server_cannot_read_the_home_the_agent_can() {
    // The filesystem dimension, measured. The fixture home holds a file that
    // stands in for `~/.claude.json` and `~/.ssh/id_ed25519`: readable outside,
    // and not there at all inside, because the mask is a mount and not a
    // permission.
    let root = fixture("fs");
    let user_home = root.join("home");
    let server_home = user_home.join(".local/state/apex/agent/mcp/memory");
    std::fs::create_dir_all(&server_home).expect("server home");
    let secret = user_home.join("the-agents-own-file");
    std::fs::write(&secret, "a token the agent has").expect("write");

    assert!(secret.exists(), "the fixture is not set up");

    let argv = sandbox_argv(
        &user_home,
        &server_home,
        &root.join("run-user"),
        false,
        "/bin/sh",
        &["-c", "cat \"$1\" 2>&1 || true; echo rc=$?; ls -A \"$2\" 2>&1", "sh"],
    );
    let mut argv = argv;
    argv.push(secret.to_string_lossy().into_owned());
    argv.push(user_home.to_string_lossy().into_owned());
    let (code, text) = run(&argv);
    assert_eq!(code, 0, "{text}");
    assert!(
        !text.contains("a token the agent has"),
        "the masked home was readable: {text}"
    );
    assert!(text.contains("No such file"), "{text}");
    // The server's own directory is the one thing inside the mask, and it is
    // writable.
    let argv = sandbox_argv(
        &user_home,
        &server_home,
        &root.join("run-user"),
        false,
        "/bin/sh",
        &["-c", "echo mine > ./notes && cat ./notes"],
    );
    let (code, text) = run(&argv);
    assert_eq!(code, 0, "{text}");
    assert!(text.contains("mine"), "{text}");
    assert!(server_home.join("notes").exists(), "the write did not reach the disk");
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn a_confined_server_with_no_network_cannot_reach_a_listener_that_is_there() {
    // The network dimension, measured against a socket that is definitely
    // listening — the failure mode of a weaker test is that nothing was
    // listening and the connection would have failed anyway.
    use std::net::TcpListener;
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            drop(stream);
        }
    });

    let root = fixture("net");
    let user_home = root.join("home");
    let server_home = user_home.join("mcp/remote");
    std::fs::create_dir_all(&server_home).expect("dirs");
    let probe = format!(
        "if exec 3<>/dev/tcp/127.0.0.1/{port}; then echo CONNECTED; else echo REFUSED; fi"
    );

    // Without the flag, the same probe against the same listener connects. That
    // is what makes the refusal below a measurement rather than a coincidence.
    let (code, text) = run(&sandbox_argv(
        &user_home,
        &server_home,
        &root.join("run-user"),
        true,
        "/bin/bash",
        &["-c", &probe],
    ));
    assert_eq!(code, 0, "{text}");
    assert!(text.contains("CONNECTED"), "the control case did not connect: {text}");

    let (_, text) = run(&sandbox_argv(
        &user_home,
        &server_home,
        &root.join("run-user"),
        false,
        "/bin/bash",
        &["-c", &probe],
    ));
    assert!(!text.contains("CONNECTED"), "the default reached the network: {text}");
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn a_server_that_asks_for_the_network_inside_an_offline_session_still_has_none() {
    // The nesting property, and the reason `network = true` is a ceiling rather
    // than a grant: an MCP server cannot be a way out of a session that was
    // confined without a network. A namespace cannot be un-shared upward, so
    // this is a fact about the kernel — which is exactly why it is worth
    // measuring rather than asserting.
    use std::net::TcpListener;
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            drop(stream);
        }
    });

    let root = fixture("nest");
    let user_home = root.join("home");
    let server_home = user_home.join("mcp/remote");
    std::fs::create_dir_all(&server_home).expect("dirs");
    let probe = format!(
        "if exec 3<>/dev/tcp/127.0.0.1/{port}; then echo CONNECTED; else echo REFUSED; fi"
    );

    // The inner sandbox asks for the network. The outer one has none.
    let inner = sandbox_argv(
        &user_home,
        &server_home,
        &root.join("run-user"),
        true,
        "/bin/bash",
        &["-c", &probe],
    );
    let mut outer: Vec<String> = vec![
        BWRAP.into(),
        "--ro-bind".into(), "/".into(), "/".into(),
        "--proc".into(), "/proc".into(),
        "--dev".into(), "/dev".into(),
        "--unshare-net".into(),
        "--".into(),
    ];
    outer.extend(inner);
    let (_, text) = run(&outer);
    assert!(
        !text.contains("CONNECTED"),
        "a nested sandbox restored a network the session did not have: {text}"
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn a_variable_the_parent_has_does_not_reach_a_confined_server() {
    // `--clearenv` and then an explicit set. The parent here carries something
    // shaped like a credential, which is the case that matters: an agent's
    // environment is where one would be.
    let root = fixture("env");
    let user_home = root.join("home");
    let server_home = user_home.join("mcp/m");
    std::fs::create_dir_all(&server_home).expect("dirs");

    let argv = sandbox_argv(
        &user_home,
        &server_home,
        &root.join("run-user"),
        false,
        "/bin/sh",
        &["-c", "env"],
    );
    let out = Command::new(&argv[0])
        .args(&argv[1..])
        .env("APEX_LIVE_TEST_TOKEN", "sentinel-must-not-appear")
        .output()
        .expect("run");
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(
        !text.contains("sentinel-must-not-appear"),
        "a parent variable reached the server: {text}"
    );
    assert!(text.contains("HOME="), "{text}");
    std::fs::remove_dir_all(&root).ok();
}
