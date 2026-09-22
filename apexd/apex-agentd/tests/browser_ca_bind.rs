//! A session's browser is told to trust one private CA, and the session says
//! what it can see (P2-012, gap 5 — `RunRequest::trust_ca`).
//!
//! ## What the unit tests beside the code cannot say
//!
//! `browser_ca::install` is tested as a function: it copies the PEM, it merges
//! the machine's policy, it refuses a key. None of that says the thing the
//! feature is: that **inside the session's mount namespace**,
//! `/etc/firefox/policies/policies.json` is the merged document and the path
//! it names is a file the session can open.
//!
//! Every failure between those two is a one-line mistake that leaves the unit
//! tests green:
//!
//!   * the two paths pushed onto `spec.ro` and none onto `spec.ro_at`, so the
//!     document exists in the scratch directory and nothing is bound over the
//!     machine's file;
//!   * the bind made with the pre-canonical scratch path, which `build_argv`
//!     then resolves to something else — the policy names a file that is not
//!     there, and Firefox answers a policy it cannot read by ignoring it;
//!   * the order reversed, so the `spec.ro` pass lands on top of the `ro_at`
//!     one and the capsule sees the machine's own file again.
//!
//! So the measurement is made by the session itself. It copies out what it
//! sees at that path, reads the path the document names, and copies that out
//! too. What this file then asserts is about bytes a confined process
//! produced, not about a `SandboxSpec`.
//!
//! ## The control is half of it
//!
//! A session started the same way with no `trust_ca` must see the MACHINE's
//! own file, byte for byte, at the same path. Without that, "the capsule sees
//! a policy with a CA in it" would also hold for a daemon that had put one
//! there for every session on the machine.
//!
//! ## What it does not touch
//!
//! The daemon is started with its own `XDG_RUNTIME_DIR`, `XDG_STATE_HOME`,
//! `XDG_CONFIG_HOME` and scratch root, so it binds its own socket and never
//! the developer's. It is killed by pid, and so is every session's process
//! GROUP — `pty::spawn` calls `setsid`, so the reported pid is the group
//! leader and `bwrap` is a separate process inside it.
//!
//! The machine's `/etc/firefox/policies/policies.json` is READ and is
//! asserted, by hash, to be the same file afterwards. Nothing here writes to
//! `/etc`.
//!
//! Where the machine has no such file — a CI runner with no Firefox — every
//! case here SKIPS and says so, because `install` refuses without it and a
//! silent skip is a test that asserts nothing.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime};

use apex_agent_core::policy::{AgentPolicy, NetworkPolicy};
use apex_agent_core::protocol::{Request, RunRequest, SandboxPolicy};

/// The machine's policy — the bind target, and the document that must survive
/// the merge.
const HOST_POLICY: &str = "/etc/firefox/policies/policies.json";

/// One certificate, structurally valid as PEM and signing nothing.
///
/// The bytes do not have to be a real certificate: what is being measured is
/// which file a confined process can open and what is written about it, not
/// whether a chain verifies. That is measured in
/// `docs/browser-capsule-auth.md`, against a real Firefox, and is not
/// something to put in CI.
const CA_PEM: &str = "-----BEGIN CERTIFICATE-----\nQVBFWC1ST1VORC0zMC1HQVAtNQ==\n-----END CERTIFICATE-----\n";

/// What the session runs. It reports what it sees and then exits.
///
/// The path out of the document is found by looking for a quoted absolute
/// path ending in `.pem` rather than by rebuilding the daemon's own naming:
/// a test that recomputed `scratch/<id>/browser-ca.pem` would assert that the
/// test author and the daemon agree today, where this asserts that whatever
/// the document names is openable from in here.
const REPORT: &str = r#"
set -u
cp /etc/firefox/policies/policies.json seen-policy.json 2>/dev/null \
    || printf 'NO POLICY AT THAT PATH\n' > seen-policy.json
ca=$(grep -o '"/[^"]*\.pem"' seen-policy.json 2>/dev/null | tr -d '"' | head -1)
printf '%s\n' "${ca:-NONE}" > seen-ca-path
if [ -n "${ca:-}" ]; then
    cp "$ca" seen-ca.pem 2>/dev/null || printf 'UNREADABLE\n' > seen-ca.pem
fi
printf 'done\n' > finished
"#;

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

struct Harness {
    child: Child,
    socket: PathBuf,
    root: PathBuf,
    sessions: std::cell::RefCell<Vec<i64>>,
}

impl Drop for Harness {
    fn drop(&mut self) {
        // The GROUP and then the pid, by number and never by name: this
        // machine's own `apex-agentd` is running, and the egress bridge is
        // that same binary, so a pattern kill here would take out the user's
        // runtime.
        for pid in self.sessions.borrow().iter() {
            for target in [format!("-{pid}"), pid.to_string()] {
                let _ = Command::new("kill")
                    .args(["-9", &target])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
            }
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

impl Harness {
    fn start(tag: &str) -> Option<Harness> {
        let root = std::env::temp_dir().join(format!(
            "apex-browserca-e2e-{}-{tag}-{}",
            std::process::id(),
            now_ms()
        ));
        let runtime = root.join("run");
        let state = root.join("state");
        let config_home = root.join("config");
        std::fs::create_dir_all(&runtime).ok()?;
        std::fs::create_dir_all(&state).ok()?;
        std::fs::create_dir_all(root.join("work")).ok()?;
        std::fs::create_dir_all(config_home.join("apex")).ok()?;
        std::fs::write(config_home.join("apex/agent.json"), "{}").ok()?;

        let child = Command::new(env!("CARGO_BIN_EXE_apex-agentd"))
            .env("XDG_RUNTIME_DIR", &runtime)
            .env("XDG_STATE_HOME", &state)
            .env("XDG_CONFIG_HOME", &config_home)
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
            sessions: std::cell::RefCell::new(Vec::new()),
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

    fn call(&self, req: &Request) -> serde_json::Value {
        let line = serde_json::to_string(req).expect("serialise");
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

    /// A confined, OFFLINE session that reports what it can see.
    ///
    /// Offline rather than allowlisted: the CA bind has nothing to do with
    /// the network, and an allowlisted session would need the egress bridge
    /// to start before this could measure a mount.
    fn report(&self, workdir: &Path, trust_ca: Option<String>) -> serde_json::Value {
        let reply = self.call(&Request::Run(RunRequest {
            agent: Some("generic".into()),
            prompt: None,
            args: vec!["/bin/sh".into(), "-c".into(), REPORT.into()],
            cwd: workdir.to_string_lossy().into_owned(),
            policy: AgentPolicy {
                sandbox: SandboxPolicy::Project,
                network: NetworkPolicy::Offline,
                ..AgentPolicy::default()
            },
            request_origin: None,
            worktree: None,
            checkpoint: false,
            ttl_ms: None,
            capabilities: None,
            allow: None,
            trust_ca,
            present: None,
            second_factor: None,
            cols: 80,
            rows: 24,
            env: vec![],
            disposable: false,
            copy_out: None,
        }));
        if let Some(pid) = reply["pid"].as_i64() {
            self.sessions.borrow_mut().push(pid);
        }
        reply
    }

    fn workdir(&self, tag: &str) -> PathBuf {
        let d = self.root.join("work").join(tag);
        std::fs::create_dir_all(&d).expect("workdir");
        d
    }
}

/// Wait for the session to say it finished, rather than for a clock.
fn wait_for_report(workdir: &Path) -> bool {
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        if workdir.join("finished").exists() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

fn sha_of(path: &Path) -> Option<String> {
    let out = Command::new("sha256sum").arg(path).output().ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .next()
        .map(str::to_string)
}

/// A confined session can start here at all.
///
/// Its own function because one case below needs ONLY this: `install` refuses
/// a PEM carrying a private key before it ever reads the machine's policy, so
/// gating that case on Firefox being installed would skip the wire refusal on
/// every CI runner — a test that never runs where it is needed most.
fn have_bwrap() -> bool {
    if Command::new("bwrap")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| !s.success())
        .unwrap_or(true)
    {
        eprintln!("SKIP: no usable bwrap, so no confined session can start");
        return false;
    }
    true
}

/// Everything the BIND cases need from the machine, or the reason they skip.
fn preflight() -> Option<String> {
    if !Path::new(HOST_POLICY).exists() {
        eprintln!(
            "SKIP: this machine has no {HOST_POLICY}, which `browser_ca::install` refuses \
             without — there is nothing to bind a copy over"
        );
        return None;
    }
    if !have_bwrap() {
        return None;
    }
    sha_of(Path::new(HOST_POLICY)).or_else(|| {
        eprintln!("SKIP: no sha256sum, so this file cannot prove it left /etc alone");
        None
    })
}

macro_rules! harness {
    ($tag:literal) => {
        match Harness::start($tag) {
            Some(h) => h,
            None => {
                eprintln!("SKIP: no daemon could be started for `{}`", $tag);
                return;
            }
        }
    };
}

fn skip_unless_started(reply: &serde_json::Value, what: &str) -> bool {
    if reply["reply"] == "session" {
        return true;
    }
    eprintln!(
        "SKIP: this machine could not start a confined session ({what}): {}",
        reply["message"].as_str().unwrap_or("no message")
    );
    false
}

#[test]
fn the_capsule_sees_the_merged_policy_and_can_open_the_ca_it_names() {
    let Some(before) = preflight() else { return };
    let h = harness!("bound");

    let ca = h.root.join("corp-root.pem");
    std::fs::write(&ca, CA_PEM).expect("write the CA");

    let work = h.workdir("bound");
    let reply = h.report(&work, Some(ca.to_string_lossy().into_owned()));
    if !skip_unless_started(&reply, "with a CA") {
        return;
    }
    assert!(
        wait_for_report(&work),
        "the session never finished; it saw: {:?}",
        std::fs::read_to_string(work.join("seen-policy.json")).unwrap_or_default()
    );

    // 1. The document at the machine's policy path, INSIDE the namespace, is
    //    the merged one.
    let seen = std::fs::read(work.join("seen-policy.json")).expect("the session copied it out");
    let doc: serde_json::Value = serde_json::from_slice(&seen).unwrap_or_else(|e| {
        panic!(
            "what the session saw is not JSON ({e}): {}",
            String::from_utf8_lossy(&seen)
        )
    });
    let install = doc["policies"]["Certificates"]["Install"]
        .as_array()
        .unwrap_or_else(|| panic!("no Certificates.Install inside the namespace: {doc}"));
    assert_eq!(install.len(), 1, "one CA was asked for: {install:?}");

    // 2. And the path it names is a file the SESSION could open. This is the
    //    assertion the unit tests cannot make: a bind of the pre-canonical
    //    scratch path produces a document naming a file that is not there,
    //    and Firefox answers a policy it cannot read by ignoring it.
    let named = std::fs::read_to_string(work.join("seen-ca-path")).expect("the path it found");
    assert!(
        named.trim().starts_with('/'),
        "the document named no absolute path: {named:?}"
    );
    let got = std::fs::read(work.join("seen-ca.pem")).expect("the session copied the CA out");
    assert_eq!(
        got,
        CA_PEM.as_bytes(),
        "the file the policy names is not the CA that was asked for"
    );

    // 3. The machine's own preferences came through the merge, measured at
    //    the far end rather than in `merged_policy`'s unit test.
    let host: serde_json::Value =
        serde_json::from_slice(&std::fs::read(HOST_POLICY).expect("read the host policy"))
            .expect("the host policy parses");
    if let Some(prefs) = host["policies"]["Preferences"].as_object() {
        assert_eq!(
            doc["policies"]["Preferences"].as_object().map(|p| p.len()),
            Some(prefs.len()),
            "the capsule lost the machine's browser defaults: {doc}"
        );
    }

    // 4. And /etc was not touched, which is the claim the whole design rests
    //    on: no other browser on this machine sees the root.
    assert_eq!(
        sha_of(Path::new(HOST_POLICY)).as_deref(),
        Some(before.as_str()),
        "the machine's own Firefox policy changed"
    );
}

#[test]
fn a_session_that_asked_for_no_ca_sees_the_machines_own_file() {
    // THE CONTROL, and the file says why in its header: without it, "the
    // capsule sees a policy with a CA in it" would also hold for a daemon
    // that installed one into every session on the machine.
    let Some(before) = preflight() else { return };
    let h = harness!("control");

    let work = h.workdir("control");
    let reply = h.report(&work, None);
    if !skip_unless_started(&reply, "with no CA") {
        return;
    }
    assert!(wait_for_report(&work), "the control session never finished");

    let seen = std::fs::read(work.join("seen-policy.json")).expect("copied out");
    assert_eq!(
        seen,
        std::fs::read(HOST_POLICY).expect("the host policy"),
        "a session that asked for no CA did not see the machine's own file"
    );
    let doc: serde_json::Value = serde_json::from_slice(&seen).expect("JSON");
    assert!(
        doc["policies"]["Certificates"].is_null(),
        "a session that asked for nothing was given a trust anchor: {doc}"
    );
    assert_eq!(sha_of(Path::new(HOST_POLICY)).as_deref(), Some(before.as_str()));
}

#[test]
fn a_pem_carrying_a_private_key_starts_no_session_at_all() {
    // The refusal over the WIRE, which is where it has to happen: the unit
    // test proves `certificates_only` rejects the bytes, and this proves the
    // rejection reaches the caller as a refusal instead of a session that
    // started with a private key copied into its scratch directory.
    //
    // NOT gated on the machine's Firefox policy, and that is deliberate:
    // `install` refuses the key before it reads that file, so a `preflight()`
    // here would skip this case on every runner without Firefox — which is
    // every CI runner, and this is the case most worth running there.
    if !have_bwrap() {
        return;
    }
    let h = harness!("key");

    let bad = h.root.join("key-and-cert.pem");
    std::fs::write(
        &bad,
        format!("-----BEGIN PRIVATE KEY-----\nQQ==\n-----END PRIVATE KEY-----\n{CA_PEM}"),
    )
    .expect("write");

    let work = h.workdir("key");
    let reply = h.report(&work, Some(bad.to_string_lossy().into_owned()));
    assert_eq!(
        reply["reply"], "error",
        "a PEM carrying a private key started a session: {reply}"
    );
    let message = reply["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("PRIVATE KEY"),
        "the refusal must name what it found: {message}"
    );
    assert!(
        !work.join("finished").exists(),
        "a session ran for a request that was refused"
    );
}

#[test]
fn an_unconfined_session_is_refused_rather_than_given_a_field_that_does_nothing() {
    // No `preflight`: this refusal is decided from the request alone, before
    // anything is read, so it holds on a machine with no Firefox and no
    // bwrap. That is the point of `browser_ca::check` being separate.
    let h = harness!("unconfined");
    let ca = h.root.join("root.pem");
    std::fs::write(&ca, CA_PEM).expect("write");
    let work = h.workdir("unconfined");

    let reply = h.call(&Request::Run(RunRequest {
        agent: Some("generic".into()),
        prompt: None,
        args: vec!["/bin/sh".into(), "-c".into(), "sleep 5".into()],
        cwd: work.to_string_lossy().into_owned(),
        policy: AgentPolicy {
            sandbox: SandboxPolicy::Unrestricted,
            network: NetworkPolicy::Open,
            ..AgentPolicy::default()
        },
        request_origin: None,
        worktree: None,
        checkpoint: false,
        ttl_ms: None,
        capabilities: None,
        allow: None,
        trust_ca: Some(ca.to_string_lossy().into_owned()),
        present: None,
        second_factor: None,
        cols: 80,
        rows: 24,
        env: vec![],
        disposable: false,
        copy_out: None,
    }));
    if let Some(pid) = reply["pid"].as_i64() {
        h.sessions.borrow_mut().push(pid);
    }
    assert_eq!(
        reply["reply"], "error",
        "an unconfined session accepted a field it cannot honour: {reply}"
    );
    assert!(
        reply["message"]
            .as_str()
            .unwrap_or_default()
            .contains("unconfined"),
        "{reply}"
    );
}
