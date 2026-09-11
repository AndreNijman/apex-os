//! Issuing an elevation challenge, against a real daemon (§7, P0-014).
//!
//! Everything the challenge/assertion sequence does with a *key* is tested in
//! `apex-agent-core`'s `webauthn` module, beside the only signer in this
//! repository that can produce a valid assertion. Three claims are not
//! properties of a value and are why this file spawns a process:
//!
//! * the request reaches a handler at all — a `cmd` the daemon does not know
//!   is a `bad_request`, and a variant that was added to the protocol and not
//!   to the dispatch would look exactly like a working feature in every unit
//!   test;
//! * the credential store is read from disk **per call**, so a key enrolled by
//!   `apex agent key add` while the daemon is running is a key it can ask for;
//! * the three refusals — nothing enrolled, several enrolled and none named, a
//!   name that is not enrolled — are the ones an operator actually hits, and
//!   each has to say something different.
//!
//! **Nothing here can raise a prompt.** Issuing a challenge grants nothing,
//! asks polkit nothing and consults §7's gate not at all; it hands back a
//! nonce and some instructions. That is asserted at the end rather than
//! assumed: the grant list is still empty afterwards.
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
    /// A daemon whose state directory `seed` was allowed to fill in first.
    ///
    /// A daemon that will not start is a **failure**, not a skip. The two
    /// older suites beside this one skip on it, and that habit is why eight
    /// assertions in `tests/test-privilege-requests.sh` went three integration
    /// rounds without executing: a suite that reports a green tick over
    /// nothing asserted is worse than a red one. Starting needs two
    /// environment variables and a writable directory, both of which are set
    /// three lines up, so there is no environment where this failing is
    /// anything other than news.
    fn start(tag: &str, seed: impl FnOnce(&Path)) -> Harness {
        let root = std::env::temp_dir().join(format!(
            "apex-elevation-e2e-{}-{tag}-{}",
            std::process::id(),
            now_ms()
        ));
        let runtime = root.join("run");
        let state = root.join("state");
        std::fs::create_dir_all(&runtime).expect("a runtime directory");
        std::fs::create_dir_all(state.join("apex").join("agent")).expect("a state directory");
        seed(&state);

        let child = Command::new(env!("CARGO_BIN_EXE_apex-agentd"))
            .env("XDG_RUNTIME_DIR", &runtime)
            .env("XDG_STATE_HOME", &state)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("apex-agentd is built beside this test and must be startable");

        let socket = runtime.join("apex-agentd").join("control.sock");
        let harness = Harness {
            child,
            socket,
            root,
        };
        assert!(
            harness.wait_for_socket(),
            "apex-agentd did not bind {} within ten seconds",
            harness.socket.display()
        );
        harness
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
}

fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// Write a credential store with these labels into a fixture state directory.
///
/// The public key is a placeholder and that is not a shortcut: issuing a
/// challenge never reads it. It is read when an assertion comes back, and that
/// path is tested in `apex-agent-core` against a key `openssl` actually
/// generated. What this fixture has to be honest about is the credential *id*,
/// because the daemon base64-decodes it to put it in the reply.
fn enrol(state: &Path, labels: &[&str]) {
    let credentials: Vec<serde_json::Value> = labels
        .iter()
        .map(|label| {
            serde_json::json!({
                "label": label,
                "id": base64_of(format!("credential-id-for-{label}").as_bytes()),
                "public_key_pem": "-----BEGIN PUBLIC KEY-----\nnot read on this path\n-----END PUBLIC KEY-----\n",
                "rp_id": "apex-agent.localhost",
                "counter": 0,
                "enrolled_ms": 1,
            })
        })
        .collect();
    let store = serde_json::json!({ "credentials": credentials });
    std::fs::write(
        state.join("apex").join("agent").join("webauthn.json"),
        serde_json::to_vec_pretty(&store).expect("serialise"),
    )
    .expect("write the credential store");
}

/// Standard base64, written out rather than pulled in: this crate's test
/// dependencies are deliberately just `serde_json`.
fn base64_of(bytes: &[u8]) -> String {
    const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(A[(n >> 18) as usize & 63] as char);
        out.push(A[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            A[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            A[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

fn decode_base64(text: &str) -> Vec<u8> {
    const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut bits = Vec::new();
    for c in text.bytes().filter(|c| *c != b'=' && !c.is_ascii_whitespace()) {
        bits.push(A.iter().position(|a| *a == c).expect("base64 alphabet") as u32);
    }
    let mut out = Vec::new();
    for group in bits.chunks(4) {
        let mut n = 0u32;
        for (i, v) in group.iter().enumerate() {
            n |= v << (18 - 6 * i);
        }
        let bytes = [(n >> 16) as u8, (n >> 8) as u8, n as u8];
        out.extend_from_slice(&bytes[..group.len() - 1]);
    }
    out
}

const ASK: &str = r#"{"cmd":"elevation_challenge","session":null,"kind":"system_access","ttl_ms":900000}"#;

#[test]
fn a_challenge_names_the_key_the_bytes_and_how_to_sign_them() {
    let h = Harness::start("issue", |state| enrol(state, &["yubikey"]));
    let reply = h.call(ASK);

    assert_eq!(reply["reply"], "elevation_challenge", "{reply}");
    assert_eq!(reply["credential"], "yubikey", "{reply}");
    assert_eq!(reply["rp_id"], "apex-agent.localhost", "{reply}");
    assert_eq!(
        reply["credential_id"],
        base64_of(b"credential-id-for-yubikey"),
        "the reply must name the id `fido2-assert` is asked for: {reply}"
    );

    // A nonce worth the name: 32 bytes, from the kernel.
    let nonce = decode_base64(reply["nonce"].as_str().expect("a nonce"));
    assert_eq!(nonce.len(), 32, "a {}-byte nonce", nonce.len());

    // The bytes the key signs are the client data itself, not its hash — the
    // whole reason the field exists, because `fido2-assert -w` hashes what it
    // is given and a caller handed a digest could not use that mode.
    let binding = decode_base64(reply["binding"].as_str().expect("the binding"));
    let text = String::from_utf8(binding).expect("the binding renders as text");
    assert!(text.contains("session=none"), "{text}");
    assert!(text.contains("ttl_ms=900000"), "{text}");
    // `kind=system-access`, not `system_access`. The request field is serde
    // snake_case and the signed bytes are `GrantKind::as_str`, which is §4's
    // own spelling — two spellings of one value, which is the shape
    // `RequestOrigin::RemoteControl` carries an explicit `#[serde(rename)]` to
    // avoid. It is not a defect here, because `Challenge::binding` is the only
    // thing that ever writes these bytes and `verify_for_challenge` the only
    // thing that reads them, so the two never have to agree with each other.
    // It is pinned because a reader comparing the wire with the signature will
    // notice the difference and needs to know it is deliberate.
    assert!(text.contains("kind=system-access"), "{text}");
    assert!(
        text.contains(reply["nonce"].as_str().expect("a nonce")),
        "the signed bytes do not name the nonce they answer: {text}"
    );

    // And it tells a human at another machine what to run, which is the point
    // of the whole path: by construction the key is not plugged in here.
    let how = reply["instructions"].as_str().expect("instructions");
    assert!(how.contains("fido2-assert"), "{how}");
    assert!(how.contains("-p"), "a touch has to be demanded: {how}");
}

#[test]
fn the_scope_asked_for_is_the_scope_signed_over() {
    // A touch is consent to one elevation. If the daemon issued a challenge
    // whose bytes did not carry the session, kind and ttl, `may_answer_for`
    // would be checking a scope nobody signed.
    let h = Harness::start("scope", |state| enrol(state, &["k"]));
    let reply = h.call(
        r#"{"cmd":"elevation_challenge","session":7,"kind":"break_glass","ttl_ms":60000}"#,
    );
    assert_eq!(reply["reply"], "elevation_challenge", "{reply}");
    let text = String::from_utf8(decode_base64(reply["binding"].as_str().expect("binding")))
        .expect("text");
    assert!(text.contains("session=7"), "{text}");
    // §4's spelling in the signed bytes; `break_glass` on the wire. See the
    // note in the test above.
    assert!(text.contains("kind=break-glass"), "{text}");
    assert!(text.contains("ttl_ms=60000"), "{text}");
}

#[test]
fn two_challenges_are_two_different_questions() {
    let h = Harness::start("twice", |state| enrol(state, &["k"]));
    let a = h.call(ASK);
    let b = h.call(ASK);
    assert_ne!(a["nonce"], b["nonce"], "the same nonce twice: {a} {b}");
    assert_ne!(
        a["binding"], b["binding"],
        "two challenges that sign the same bytes are one challenge"
    );
}

#[test]
fn an_empty_store_is_refused_by_naming_the_command_that_fixes_it() {
    // The state that matters: an empty credential store is what makes
    // `--origin-policy remote` refuse to start, so this refusal is one an
    // owner turning the policy on will meet first.
    let h = Harness::start("empty", |_| {});
    let reply = h.call(ASK);
    assert_eq!(reply["reply"], "error", "{reply}");
    let message = reply["message"].as_str().expect("a message");
    assert!(message.contains("no security key is enrolled"), "{message}");
    assert!(
        message.contains("apex agent key add"),
        "a refusal has to name the way out: {message}"
    );
}

#[test]
fn several_keys_and_none_named_is_refused_rather_than_guessed() {
    // Picking the first would produce `UnknownCredential` later, once the
    // owner had touched the key they actually have — an error reading "your
    // key is not enrolled" when the true answer is "say which one".
    let h = Harness::start("several", |state| enrol(state, &["desk", "travel"]));
    let reply = h.call(ASK);
    assert_eq!(reply["reply"], "error", "{reply}");
    let message = reply["message"].as_str().expect("a message");
    assert!(message.contains("--credential"), "{message}");
    assert!(message.contains("desk"), "{message}");
    assert!(message.contains("travel"), "{message}");

    // Named, it goes through — and the reply is about that key and no other.
    let named = h.call(
        r#"{"cmd":"elevation_challenge","kind":"system_access","ttl_ms":900000,"credential":"travel"}"#,
    );
    assert_eq!(named["reply"], "elevation_challenge", "{named}");
    assert_eq!(named["credential"], "travel", "{named}");
    assert_eq!(
        named["credential_id"],
        base64_of(b"credential-id-for-travel"),
        "{named}"
    );
}

#[test]
fn a_name_that_is_not_enrolled_is_refused_and_lists_what_is() {
    let h = Harness::start("unknown", |state| enrol(state, &["desk"]));
    let reply = h.call(
        r#"{"cmd":"elevation_challenge","kind":"system_access","ttl_ms":900000,"credential":"nope"}"#,
    );
    assert_eq!(reply["reply"], "error", "{reply}");
    let message = reply["message"].as_str().expect("a message");
    assert!(message.contains("nope"), "{message}");
    assert!(message.contains("desk"), "{message}");
}

#[test]
fn asking_for_a_challenge_grants_nothing() {
    // The constraint this whole file is shaped by. A challenge is a question:
    // it consults §7's gate not at all, asks polkit nothing, and must leave no
    // authority behind. If issuing one ever starts granting, this is what
    // says so.
    let h = Harness::start("grants-nothing", |state| enrol(state, &["k"]));
    let before = h.call(r#"{"cmd":"system_grants"}"#);
    assert_eq!(
        before["grants"].as_array().map(|a| a.len()),
        Some(0),
        "{before}"
    );

    for _ in 0..3 {
        assert_eq!(h.call(ASK)["reply"], "elevation_challenge");
    }

    let after = h.call(r#"{"cmd":"system_grants"}"#);
    assert_eq!(
        after["grants"].as_array().map(|a| a.len()),
        Some(0),
        "issuing a challenge created a grant: {after}"
    );
}

#[test]
fn a_key_enrolled_while_the_daemon_is_running_is_one_it_can_ask_for() {
    // The store is read per call rather than cached at startup, because
    // enrolment is `apex agent key add` writing the file — so a cache would
    // mean a key the owner just enrolled did not exist until the daemon
    // restarted. Asserted by doing exactly that.
    let h = Harness::start("late-enrol", |_| {});
    assert_eq!(h.call(ASK)["reply"], "error", "nothing is enrolled yet");

    enrol(&h.root.join("state"), &["arrived-later"]);

    let reply = h.call(ASK);
    assert_eq!(reply["reply"], "elevation_challenge", "{reply}");
    assert_eq!(reply["credential"], "arrived-later", "{reply}");
}
