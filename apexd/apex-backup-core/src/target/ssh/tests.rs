use super::*;
use std::path::Path;
use std::cell::RefCell;

// ── the argument list, which is where the security properties are ───────────

fn endpoint() -> SshEndpoint {
    SshEndpoint {
        host: "backup.example".to_string(),
        user: Some("apex".to_string()),
        port: Some(2222),
        identity: PathBuf::from("/var/lib/apex-backup/ssh/id"),
        known_hosts: PathBuf::from("/var/lib/apex-backup/ssh/known_hosts"),
    }
}

/// Refusing a Fake that answers nothing, for the tests that only look at argv.
struct Silent;

impl Transport for Silent {
    fn run(&self, _argv: &[String], _input: &[u8]) -> Result<Output, TargetError> {
        Err(TargetError::Unavailable("not run".to_string()))
    }
}

/// The single most important test in this file.
///
/// Every one of these options is the difference between a backup target and a
/// way to spend the owner's ssh credentials or to back up to whoever answers.
/// They are asserted individually rather than as one golden argv, so that a
/// missing one names itself.
#[test]
fn the_command_line_can_never_reach_the_owners_agent_config_or_known_hosts() {
    let t = SshTarget::new(Silent, endpoint(), "/srv/backups", "apex-backup");
    let argv = t.argv("printf ready");

    assert_eq!(argv[0], SSH, "ssh is run by absolute path, never off PATH");

    for required in [
        "-oBatchMode=yes",
        "-oIdentitiesOnly=yes",
        "-oIdentityAgent=none",
        "-oPreferredAuthentications=publickey",
        "-oNumberOfPasswordPrompts=0",
        "-oStrictHostKeyChecking=yes",
        "-oGlobalKnownHostsFile=/dev/null",
        "-oUserKnownHostsFile=/var/lib/apex-backup/ssh/known_hosts",
        "-oConnectTimeout=15",
    ] {
        assert!(
            argv.iter().any(|a| a == required),
            "{required} is not on the command line: {argv:?}"
        );
    }

    // `-F /dev/null`: no ssh_config at all, so nothing in the owner's home can
    // add a ProxyCommand, change the user, or re-enable the agent.
    let f = argv.iter().position(|a| a == "-F").expect("-F is passed");
    assert_eq!(argv[f + 1], "/dev/null");

    // The key this target names, and only it.
    let i = argv.iter().position(|a| a == "-i").expect("-i is passed");
    assert_eq!(argv[i + 1], "/var/lib/apex-backup/ssh/id");

    // Nothing anywhere on the line points into a home directory.
    assert!(
        !argv.iter().any(|a| a.contains(".ssh/")),
        "an argument points at an ssh home directory: {argv:?}"
    );

    // The destination is last but one, after `--`, so a host beginning with a
    // dash is a host and not an option.
    let dashdash = argv.iter().position(|a| a == "--").expect("-- is passed");
    assert_eq!(argv[dashdash + 1], "backup.example");
    assert_eq!(argv[dashdash + 2], "printf ready");
    assert_eq!(argv.len(), dashdash + 3, "nothing follows the script");
}

#[test]
fn the_port_and_user_are_passed_only_when_the_project_names_them() {
    let bare = SshEndpoint {
        user: None,
        port: None,
        ..endpoint()
    };
    let t = SshTarget::new(Silent, bare, "/srv/backups", "apex-backup");
    let argv = t.argv("true");
    assert!(!argv.iter().any(|a| a == "-p"), "{argv:?}");
    assert!(!argv.iter().any(|a| a == "-l"), "{argv:?}");

    let t = SshTarget::new(Silent, endpoint(), "/srv/backups", "apex-backup");
    let argv = t.argv("true");
    let p = argv.iter().position(|a| a == "-p").expect("-p");
    assert_eq!(argv[p + 1], "2222");
    let l = argv.iter().position(|a| a == "-l").expect("-l");
    assert_eq!(argv[l + 1], "apex");
}

// ── quoting ────────────────────────────────────────────────────────────────

#[test]
fn a_single_quote_cannot_end_the_word_it_is_inside() {
    assert_eq!(shell_quote("plain"), "'plain'");
    assert_eq!(shell_quote("it's"), r"'it'\''s'");
    // The bytes a shell would otherwise act on are literal inside single
    // quotes, so the quoting is total rather than a list somebody maintains.
    for hostile in [
        "; rm -rf /",
        "$(id)",
        "`id`",
        "a\nb",
        "\\",
        "*",
        "$HOME",
    ] {
        let quoted = shell_quote(hostile);
        let out = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(format!("printf %s {quoted}"))
            .output()
            .expect("sh");
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            hostile,
            "{hostile:?} did not survive as data"
        );
    }
}

// ── the remote scripts, run for real by a local shell ───────────────────────

/// A transport that runs the remote script with `/bin/sh` on this machine.
///
/// Not a fake in the usual sense: the *script* is the real one, the exit codes
/// are the real ones, and `cat`, `mkdir`, `mv` and `ls` are the real programs.
/// What is missing is only the ssh hop — which is exactly what
/// `tests/test-apex-backup-ssh.sh` adds, against a real `sshd`.
///
/// So the three-way answer is proved here against a genuine "there is no such
/// file" and a genuine "you may not read it", rather than against a programmed
/// number, and the shell suite proves the hop.
struct LocalShell {
    /// Every command line this was handed, for the argv assertions.
    seen: RefCell<Vec<Vec<String>>>,
}

impl LocalShell {
    fn new() -> LocalShell {
        LocalShell {
            seen: RefCell::new(Vec::new()),
        }
    }
}

impl Transport for LocalShell {
    fn run(&self, argv: &[String], input: &[u8]) -> Result<Output, TargetError> {
        use std::io::{Read, Write};
        self.seen.borrow_mut().push(argv.to_vec());
        let script = argv.last().expect("a script").clone();
        let mut child = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(&script)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| TargetError::Unavailable(format!("sh: {e}")))?;
        let mut stdin = child.stdin.take().expect("piped");
        let payload = input.to_vec();
        let writer = std::thread::spawn(move || {
            let _ = stdin.write_all(&payload);
            drop(stdin);
        });
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        child
            .stdout
            .take()
            .expect("piped")
            .read_to_end(&mut stdout)
            .ok();
        child
            .stderr
            .take()
            .expect("piped")
            .read_to_end(&mut stderr)
            .ok();
        let status = child.wait().map_err(|e| TargetError::Unavailable(e.to_string()))?;
        let _ = writer.join();
        Ok(Output {
            status: status.code(),
            stdout,
            stderr: String::from_utf8_lossy(&stderr).trim().to_string(),
        })
    }
}

fn fixture() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("apex-backup-ssh-")
        .tempdir_in("/var/tmp")
        .expect("a fixture under /var/tmp")
}

fn target(root: &Path) -> SshTarget<LocalShell> {
    SshTarget::new(
        LocalShell::new(),
        endpoint(),
        root.to_str().expect("utf8"),
        "apex-backup",
    )
}

const SNAP: &str = "20260912T101112Z-abcd1234";

#[test]
fn an_object_written_comes_back_byte_for_byte() {
    let dir = fixture();
    let t = target(dir.path());
    t.prepare().expect("prepare");
    t.put(SNAP, "chunk.0", b"hello").expect("put");
    assert_eq!(t.get(SNAP, "chunk.0").expect("get"), b"hello");
}

/// The property the R2 target has to work for with base64, and this one gets
/// for free — asserted anyway, because "ssh is 8-bit clean" is exactly the kind
/// of assumption that is true until somebody adds a `-t` or a `String`
/// somewhere in the path.
#[test]
fn a_chunk_that_is_not_utf8_survives_the_round_trip() {
    let dir = fixture();
    let t = target(dir.path());
    t.prepare().expect("prepare");

    // Invalid UTF-8 four different ways, plus the bytes a line-oriented path
    // would eat: NUL, CR, LF, and a trailing newline that a `$(...)` would
    // strip.
    let mut body: Vec<u8> = vec![
        0xff, 0xfe, 0x80, 0x00, b'\r', b'\n', 0xc3, 0x28, 0xed, 0xa0, 0x80, 0xf4, 0x90, 0x80,
        0x80,
    ];
    body.extend_from_slice(b"tail\n\n");
    // Every byte value, so nothing in the path can be treating one specially.
    body.extend((0u8..=255).collect::<Vec<u8>>());

    t.put(SNAP, "chunk.0", &body).expect("put");
    let back = t.get(SNAP, "chunk.0").expect("get");
    assert_eq!(back, body, "the round trip changed the bytes");
}

#[test]
fn an_object_that_is_not_there_is_absent_and_not_a_failure() {
    let dir = fixture();
    let t = target(dir.path());
    t.prepare().expect("prepare");
    let err = t.get(SNAP, "chunk.7").expect_err("no such object");
    assert!(
        matches!(err, TargetError::Absent(_)),
        "a missing object must be Absent, got {err:?}"
    );
    assert!(matches!(
        err.verdict("chunk.7"),
        crate::Verdict::Absent(_)
    ));
}

/// Permission denied is not absence, over ssh as everywhere else.
#[test]
fn an_object_that_may_not_be_read_is_a_refusal_and_never_an_absence() {
    use std::os::unix::fs::PermissionsExt;
    if unsafe { libc::geteuid() } == 0 {
        eprintln!(
            "NOTE: running as root, which bypasses the mode bits, so this \
             assertion cannot be staged and is not made."
        );
        return;
    }
    let dir = fixture();
    let t = target(dir.path());
    t.prepare().expect("prepare");
    t.put(SNAP, "chunk.0", b"secret").expect("put");
    let path = dir
        .path()
        .join("apex-backup")
        .join(SNAP)
        .join("chunk.0");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).expect("chmod");

    let err = t.get(SNAP, "chunk.0").expect_err("unreadable");
    assert!(
        matches!(err, TargetError::Denied(_)),
        "an unreadable object must be Denied, got {err:?}"
    );
    // And the verdict it becomes is the one that says nothing was looked at.
    let verdict = err.verdict("chunk.0");
    assert!(
        matches!(verdict, crate::Verdict::CouldNotRun(_)),
        "{verdict:?}"
    );
}

#[test]
fn a_target_root_that_is_not_there_is_could_not_run_and_never_an_empty_history() {
    let dir = fixture();
    let t = target(&dir.path().join("nowhere"));
    let err = t.prepare().expect_err("no root");
    assert!(matches!(err, TargetError::Unavailable(_)), "{err:?}");

    let err = t.list().expect_err("no root");
    assert!(
        matches!(err, TargetError::Unavailable(_)),
        "a listing of a root that is not there must never be an empty list: {err:?}"
    );
}

/// The real answer that looks like the dangerous one: the root is fine and
/// nothing has been written yet.
#[test]
fn a_prepared_target_with_no_snapshots_lists_empty_because_that_is_true() {
    let dir = fixture();
    let t = target(dir.path());
    t.prepare().expect("prepare");
    assert_eq!(t.list().expect("list"), Vec::new());
}

#[test]
fn snapshots_are_listed_oldest_first_and_junk_is_ignored() {
    let dir = fixture();
    let t = target(dir.path());
    t.prepare().expect("prepare");
    for id in [
        "20260912T101112Z-abcd1234",
        "20260910T000000Z-00000000",
        "20260911T235959Z-ffffffff",
    ] {
        t.put(id, "manifest", b"x").expect("put");
    }
    // A directory that is not a snapshot id, which a far side may well have.
    std::fs::create_dir_all(dir.path().join("apex-backup/lost+found")).expect("mkdir");

    let ids: Vec<String> = t
        .list()
        .expect("list")
        .into_iter()
        .map(|id| id.to_string())
        .collect();
    assert_eq!(
        ids,
        vec![
            "20260910T000000Z-00000000",
            "20260911T235959Z-ffffffff",
            "20260912T101112Z-abcd1234",
        ]
    );
}

/// A half-written object must never be visible under its real name.
#[test]
fn an_object_is_renamed_into_place_rather_than_written_in_place() {
    let dir = fixture();
    let t = target(dir.path());
    t.prepare().expect("prepare");
    t.put(SNAP, "chunk.0", b"x").expect("put");
    let script = t
        .transport
        .seen
        .borrow()
        .last()
        .expect("a command")
        .last()
        .expect("a script")
        .clone();
    assert!(script.contains(".chunk.0.partial"), "{script}");
    assert!(script.contains("mv --"), "{script}");
    // And nothing is left behind.
    assert!(!dir
        .path()
        .join("apex-backup")
        .join(SNAP)
        .join(".chunk.0.partial")
        .exists());
}

// ── the exit-code mapping, including the two a local shell cannot stage ─────

/// A transport that answers with one programmed status.
struct Canned(Output);

impl Transport for Canned {
    fn run(&self, _argv: &[String], _input: &[u8]) -> Result<Output, TargetError> {
        Ok(self.0.clone())
    }
}

fn canned(status: Option<i32>, stderr: &str) -> SshTarget<Canned> {
    SshTarget::new(
        Canned(Output {
            status,
            stdout: Vec::new(),
            stderr: stderr.to_string(),
        }),
        endpoint(),
        "/srv/backups",
        "apex-backup",
    )
}

/// 255 is ssh's own, and it is the one that must never be read as an answer
/// about the backups.
#[test]
fn ssh_failing_to_connect_is_could_not_run_and_says_so() {
    let t = canned(
        Some(code::SSH_FAILED),
        "ssh: connect to host backup.example port 2222: Connection refused",
    );
    let err = t.get(SNAP, "chunk.0").expect_err("no connection");
    assert!(matches!(err, TargetError::Unavailable(_)), "{err:?}");
    let why = err.to_string();
    assert!(why.contains("did not reach"), "{why}");
    assert!(why.contains("Connection refused"), "{why}");

    // A listing that could not connect is NOT an empty history.
    let err = t.list().expect_err("no connection");
    assert!(matches!(err, TargetError::Unavailable(_)), "{err:?}");
}

/// Killed by a signal: no status at all, which is not zero.
#[test]
fn a_signal_is_not_a_status_and_is_never_success() {
    let t = canned(None, "");
    let err = t.get(SNAP, "chunk.0").expect_err("killed");
    assert!(matches!(err, TargetError::Unavailable(_)), "{err:?}");
    assert!(err.to_string().contains("signal"), "{err}");
}

/// An exit code this build did not choose is not quietly one that it did.
#[test]
fn an_unrecognised_exit_code_is_could_not_run_and_names_the_number() {
    let t = canned(Some(3), "sh: ls: not found");
    let err = t.list().expect_err("odd exit");
    assert!(matches!(err, TargetError::Unavailable(_)), "{err:?}");
    assert!(err.to_string().contains("exited 3"), "{err}");
}

/// The three codes this build defines, each mapping to its own answer.
#[test]
fn each_defined_exit_code_maps_to_its_own_answer() {
    assert!(matches!(
        canned(Some(code::ABSENT), "")
            .get(SNAP, "c")
            .expect_err("absent"),
        TargetError::Absent(_)
    ));
    assert!(matches!(
        canned(Some(code::DENIED), "")
            .get(SNAP, "c")
            .expect_err("denied"),
        TargetError::Denied(_)
    ));
    assert!(matches!(
        canned(Some(code::NO_ROOT), "")
            .get(SNAP, "c")
            .expect_err("no root"),
        TargetError::Unavailable(_)
    ));
    // And none of the four is the same number as another, which is the whole
    // basis of the mapping.
    let codes = [
        code::ABSENT,
        code::DENIED,
        code::NO_ROOT,
        code::SSH_FAILED,
    ];
    let mut sorted = codes.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), codes.len(), "two exit codes collide");
    assert!(!codes.contains(&0), "0 is success and cannot be an answer");
}

#[test]
fn what_the_target_calls_itself_names_the_host_and_the_path_and_no_key() {
    let t = SshTarget::new(Silent, endpoint(), "/srv/backups", "apex-backup");
    let described = t.describe();
    assert_eq!(described, "ssh at apex@backup.example:2222:/srv/backups");
    assert!(
        !described.contains("id") && !described.contains("known_hosts"),
        "the description names a key file: {described}"
    );
}
