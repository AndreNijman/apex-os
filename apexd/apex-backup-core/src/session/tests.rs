//! Fixtures live under `/var/tmp`; a restore only ever writes into one; and
//! nothing outside one is touched.

use std::os::unix::fs::PermissionsExt;

use super::*;
use crate::config::Where;
use crate::crypto::Recipient;
use crate::target::fs::{FsTarget, Marker};
use crate::target::Kind as TargetKind;

/// A high-entropy string planted in the source, looked for in every byte that
/// reaches the target. Fixed rather than random so a failure is greppable.
const CANARY: &str = "CANARY-9f3a1d7e4b0c2856-SECRET-PAYROLL-ROW";

fn work() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("apex-backup-session-")
        .tempdir_in("/var/tmp")
        .expect("a fixture directory under /var/tmp")
}

fn config() -> BackupConfig {
    BackupConfig {
        recipient: String::new(),
        kind: TargetKind::Local,
        prefix: "apex-backup".to_string(),
        exclude: vec!["excluded".to_string()],
        where_to: Where::Directory {
            path: PathBuf::from("/var/tmp"),
            marker: None,
        },
    }
}

fn target_at(root: &Path) -> FsTarget {
    let t = FsTarget::local(root.to_path_buf(), "apex-backup", None);
    t.write_marker(&Marker::new(1).expect("a marker")).expect("writes");
    t
}

/// A small tree with one of everything the format carries.
fn source_tree(root: &Path) {
    std::fs::create_dir_all(root.join("src/deep")).expect("mkdir");
    std::fs::create_dir_all(root.join("excluded/inside")).expect("mkdir");
    std::fs::write(root.join("README.md"), b"# a project\n").expect("write");
    std::fs::write(
        root.join("src/main.rs"),
        format!("fn main() {{ println!(\"{CANARY}\"); }}\n").as_bytes(),
    )
    .expect("write");
    std::fs::write(root.join("src/deep/notes.txt"), b"deep notes").expect("write");
    std::fs::write(root.join("excluded/inside/huge.bin"), b"not in the snapshot")
        .expect("write");
    std::fs::write(root.join("script.sh"), b"#!/bin/sh\necho hi\n").expect("write");
    std::fs::set_permissions(root.join("script.sh"), std::fs::Permissions::from_mode(0o755))
        .expect("chmod");
    std::os::unix::fs::symlink("src/main.rs", root.join("link-to-main")).expect("symlink");
    std::os::unix::fs::symlink("/etc/passwd", root.join("link-outside")).expect("symlink");
}

struct Fixture {
    _dir: tempfile::TempDir,
    source: PathBuf,
    target_root: PathBuf,
    identity: Identity,
    recipient: Recipient,
}

impl Fixture {
    fn new() -> Fixture {
        let dir = work();
        let source = dir.path().join("source");
        let target_root = dir.path().join("target");
        std::fs::create_dir_all(&source).expect("mkdir");
        std::fs::create_dir_all(&target_root).expect("mkdir");
        source_tree(&source);
        let identity = Identity::generate().expect("a key");
        let recipient = identity.recipient();
        Fixture {
            _dir: dir,
            source,
            target_root,
            identity,
            recipient,
        }
    }

    fn target(&self) -> FsTarget {
        target_at(&self.target_root)
    }

    fn run(&self) -> Written {
        self.run_with(&RunOptions::default(), crate::now_ms())
    }

    fn run_with(&self, options: &RunOptions, now_ms: u64) -> Written {
        write(
            &self.source,
            &self.target(),
            &self.recipient,
            &config(),
            options,
            now_ms,
        )
        .expect("the run succeeds")
    }

    /// Every byte of every object this target holds.
    fn all_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        fn walk(dir: &Path, out: &mut Vec<u8>) {
            for entry in std::fs::read_dir(dir).expect("reads").flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, out);
                } else {
                    out.extend_from_slice(&std::fs::read(&path).expect("reads"));
                    // The names too: a file name is data about the tree.
                    out.extend_from_slice(path.to_string_lossy().as_bytes());
                }
            }
        }
        walk(&self.target_root, &mut out);
        out
    }

    fn object_path(&self, id: &SnapshotId, object: &str) -> PathBuf {
        self.target_root
            .join("apex-backup")
            .join(id.as_str())
            .join(object)
    }
}

// ── the round trip ──────────────────────────────────────────────────────────

#[test]
fn a_tree_round_trips_through_a_snapshot_and_a_restore() {
    let f = Fixture::new();
    let written = f.run();
    assert!(written.files >= 4, "{} files", written.files);

    let into = f.target_root.parent().expect("parent").join("restored");
    let restored = restore(
        &f.target(),
        &written.id,
        &f.identity,
        &into,
        &RestoreOptions::default(),
    )
    .expect("restores");

    assert!(
        restored.verdict.is_intact(),
        "the restore did not verify: {}",
        restored.verdict
    );

    assert_eq!(
        std::fs::read(into.join("README.md")).expect("reads"),
        b"# a project\n"
    );
    assert!(std::fs::read_to_string(into.join("src/main.rs"))
        .expect("reads")
        .contains(CANARY));
    assert_eq!(
        std::fs::read(into.join("src/deep/notes.txt")).expect("reads"),
        b"deep notes"
    );

    // Modes survive.
    let mode = std::fs::metadata(into.join("script.sh"))
        .expect("stats")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o755, "script.sh came back {mode:o}");

    // Symlinks come back as symlinks, pointing where they did.
    let link = std::fs::symlink_metadata(into.join("link-to-main")).expect("stats");
    assert!(link.file_type().is_symlink());
    assert_eq!(
        std::fs::read_link(into.join("link-to-main")).expect("reads"),
        PathBuf::from("src/main.rs")
    );
    assert_eq!(
        std::fs::read_link(into.join("link-outside")).expect("reads"),
        PathBuf::from("/etc/passwd")
    );
}

#[test]
fn an_excluded_directory_is_not_in_the_snapshot_at_all() {
    let f = Fixture::new();
    let written = f.run();
    let into = f.target_root.parent().expect("parent").join("restored");
    restore(
        &f.target(),
        &written.id,
        &f.identity,
        &into,
        &RestoreOptions::default(),
    )
    .expect("restores");
    assert!(!into.join("excluded").exists(), "the exclusion was ignored");

    // And its contents are not in the ciphertext under another name either.
    let everything = f.all_bytes();
    assert!(
        !contains(&everything, b"not in the snapshot"),
        "an excluded file's contents reached the target"
    );
}

/// A symlink is recorded, never followed. Following one would pull whatever it
/// points at into the snapshot — `/etc/passwd`, in this fixture.
#[test]
fn a_symlink_out_of_the_tree_is_recorded_and_never_followed() {
    let f = Fixture::new();
    f.run();
    let everything = f.all_bytes();
    // `/etc/passwd` on this machine starts with a root line; whatever it holds,
    // the snapshot must not.
    let passwd = std::fs::read("/etc/passwd").unwrap_or_default();
    if passwd.len() > 32 {
        assert!(
            !contains(&everything, &passwd[..32]),
            "following a symlink pulled /etc/passwd into the snapshot"
        );
    }
}

// ── the encryption claim, measured ──────────────────────────────────────────

/// P2-001 criterion 2. Not "the code calls an AEAD" — the bytes on the target
/// are looked at.
#[test]
fn no_byte_written_to_the_target_holds_the_plaintext() {
    let f = Fixture::new();
    f.run();
    let everything = f.all_bytes();
    assert!(!everything.is_empty(), "nothing was written at all");
    assert!(
        !contains(&everything, CANARY.as_bytes()),
        "the canary planted in the source is in what was written to the target"
    );
    assert!(
        !contains(&everything, b"# a project"),
        "a file's contents reached the target in the clear"
    );
}

/// The standard hole in an "encrypted backup" claim, closed and measured.
#[test]
fn file_names_are_not_in_the_clear_either() {
    let f = Fixture::new();
    let written = f.run();
    let everything = f.all_bytes();
    for name in ["README.md", "main.rs", "notes.txt", "script.sh", "link-to-main"] {
        assert!(
            !contains(&everything, name.as_bytes()),
            "the file name {name} is readable on the target, so the manifest is \
             not encrypted"
        );
    }
    // What IS in the clear is exactly what the head declares it to be.
    let head_bytes = std::fs::read(f.object_path(&written.id, HEAD_OBJECT)).expect("reads");
    let head: Head = serde_json::from_slice(&head_bytes).expect("parses");
    assert_eq!(head.snapshot, written.id.as_str());
    assert_eq!(head.recipient, f.recipient.to_string_value());
}

#[test]
fn a_snapshot_does_not_open_with_another_machines_key() {
    let f = Fixture::new();
    let written = f.run();
    let stranger = Identity::generate().expect("a key");

    let verification = verify(&f.target(), &written.id, Some(&stranger));
    assert!(
        verification.presence.is_intact(),
        "presence should be fine: {}",
        verification.presence
    );
    assert_eq!(
        verification.contents.as_str(),
        "failed",
        "another machine's key produced {}",
        verification.contents
    );
}

// ── the four verdicts ───────────────────────────────────────────────────────

/// Without a key, "the contents are good" is not a thing that can be said.
#[test]
fn verifying_without_a_key_can_never_say_the_contents_are_intact() {
    let f = Fixture::new();
    let written = f.run();
    let verification = verify(&f.target(), &written.id, None);
    assert!(verification.presence.is_intact(), "{}", verification.presence);
    assert_eq!(verification.contents.as_str(), "could-not-run");
    assert!(!verification.contents.is_intact());
    assert!(verification
        .contents
        .reason()
        .is_some_and(|r| r.contains("only root can check")));
}

#[test]
fn a_whole_snapshot_verifies_when_it_is_good() {
    let f = Fixture::new();
    let written = f.run();
    let verification = verify(&f.target(), &written.id, Some(&f.identity));
    assert!(verification.presence.is_intact(), "{}", verification.presence);
    assert!(
        verification.contents.is_intact(),
        "a snapshot just written did not verify: {}",
        verification.contents
    );
    assert!(verification
        .contents
        .reason()
        .is_none());
}

#[test]
fn a_flipped_bit_in_a_data_chunk_is_failed_and_is_never_intact() {
    let f = Fixture::new();
    let written = f.run();
    let path = f.object_path(&written.id, "data.000000");
    let mut bytes = std::fs::read(&path).expect("reads");
    let last = bytes.len() - 1;
    bytes[last] ^= 0x01;
    std::fs::write(&path, &bytes).expect("writes");

    let verification = verify(&f.target(), &written.id, Some(&f.identity));
    assert!(
        verification.presence.is_intact(),
        "the object is still there: {}",
        verification.presence
    );
    assert_eq!(verification.contents.as_str(), "failed");
    assert!(!verification.contents.is_intact());
    // And for the right reason. A mutation that swallowed the AEAD's refusal
    // and treated the chunk as empty ALSO produced "failed" — from the stream
    // running out early — so asserting only the state let a much worse bug
    // through: a chunk whose bytes changed, silently read as no bytes at all.
    assert!(
        verification
            .contents
            .reason()
            .is_some_and(|r| r.contains("does not open")),
        "the failure has to name the chunk that did not open: {}",
        verification.contents
    );
}

/// The digest is what catches *this program*, not an attacker: a snapshot
/// whose chunks all open and whose bytes are still the wrong ones.
///
/// A positive test cannot catch a weakened digest check — a mutation that
/// compared a digest with itself left `a_whole_snapshot_verifies_when_it_is_good`
/// green. So the chunk is resealed, correctly, with different content of the
/// same length: the AEAD is satisfied and only the digest is not.
#[test]
fn a_chunk_resealed_with_different_content_of_the_same_length_is_failed() {
    let f = Fixture::new();
    let written = f.run();
    let head_bytes = std::fs::read(f.object_path(&written.id, HEAD_OBJECT)).expect("reads");
    let head: Head = serde_json::from_slice(&head_bytes).expect("parses");
    let key = open_with(
        &f.identity,
        &head.ephemeral_bytes().expect("decodes"),
        written.id.as_str(),
    )
    .expect("derives");

    let path = f.object_path(&written.id, "data.000000");
    let sealed = std::fs::read(&path).expect("reads");
    let plain = key
        .open_chunk(Stream::Data, 0, head.data_chunks == 1, &sealed)
        .expect("opens");
    // Same length, different bytes — so every length and offset in the
    // manifest still adds up and only the contents are wrong.
    let swapped: Vec<u8> = plain.iter().map(|b| b ^ 0x20).collect();
    assert_eq!(swapped.len(), plain.len());
    let resealed = key
        .seal_chunk(Stream::Data, 0, head.data_chunks == 1, &swapped)
        .expect("seals");
    std::fs::write(&path, &resealed).expect("writes");

    let verification = verify(&f.target(), &written.id, Some(&f.identity));
    assert!(
        verification.presence.is_intact(),
        "everything is still there: {}",
        verification.presence
    );
    assert_eq!(
        verification.contents.as_str(),
        "failed",
        "a chunk that opens and holds the wrong bytes was reported as {}",
        verification.contents
    );
    assert!(
        verification
            .contents
            .reason()
            .is_some_and(|r| r.contains("digest is")),
        "the failure has to be the digest: {}",
        verification.contents
    );

    // And a restore of it writes nothing rather than handing back damage.
    let into = f.target_root.parent().expect("parent").join("restored");
    let restored = restore(
        &f.target(),
        &written.id,
        &f.identity,
        &into,
        &RestoreOptions::default(),
    )
    .expect("returns a report");
    assert_eq!(restored.verdict.as_str(), "failed", "{}", restored.verdict);
    assert_eq!(restored.files, 0, "a file that did not match was written");
}

/// The manifest's offsets have to be load-bearing on the path that writes
/// files, not only on the path that checks them.
#[test]
fn a_manifest_whose_offsets_disagree_with_itself_restores_nothing() {
    let f = Fixture::new();
    let written = f.run();
    let head_bytes = std::fs::read(f.object_path(&written.id, HEAD_OBJECT)).expect("reads");
    let head: Head = serde_json::from_slice(&head_bytes).expect("parses");
    let key = open_with(
        &f.identity,
        &head.ephemeral_bytes().expect("decodes"),
        written.id.as_str(),
    )
    .expect("derives");

    // Reseal the manifest with every offset zeroed, which is what a writer
    // that forgot to advance them would produce.
    let mut reader_bytes = Vec::new();
    for index in 0..head.manifest_chunks {
        let sealed = std::fs::read(f.object_path(
            &written.id,
            &Head::chunk_object(Stream::Manifest, index),
        ))
        .expect("reads");
        reader_bytes.extend_from_slice(
            &key.open_chunk(
                Stream::Manifest,
                u64::from(index),
                index + 1 == head.manifest_chunks,
                &sealed,
            )
            .expect("opens"),
        );
    }
    let mut manifest: Manifest = serde_json::from_slice(&reader_bytes).expect("parses");
    for entry in &mut manifest.entries {
        entry.offset = 0;
    }
    let text = serde_json::to_vec(&manifest).expect("serialises");
    assert!(
        text.len() <= CHUNK_BYTES,
        "the fixture's manifest no longer fits one chunk"
    );
    let resealed = key
        .seal_chunk(Stream::Manifest, 0, true, &text)
        .expect("seals");
    for index in 0..head.manifest_chunks {
        std::fs::remove_file(f.object_path(
            &written.id,
            &Head::chunk_object(Stream::Manifest, index),
        ))
        .expect("removes");
    }
    std::fs::write(
        f.object_path(&written.id, "manifest.000000"),
        &resealed,
    )
    .expect("writes");
    let mut head = head;
    head.manifest_chunks = 1;
    std::fs::write(
        f.object_path(&written.id, HEAD_OBJECT),
        serde_json::to_vec(&head).expect("serialises"),
    )
    .expect("writes");

    let into = f.target_root.parent().expect("parent").join("restored");
    let restored = restore(
        &f.target(),
        &written.id,
        &f.identity,
        &into,
        &RestoreOptions::default(),
    )
    .expect("returns a report");
    assert_eq!(
        restored.verdict.as_str(),
        "failed",
        "a self-contradicting manifest restored quietly: {}",
        restored.verdict
    );
    assert!(restored
        .verdict
        .reason()
        .is_some_and(|r| r.contains("disagrees with itself")));
}

/// A chunk the head declares and the target does not have is a broken snapshot,
/// not an absent one.
#[test]
fn a_missing_chunk_is_a_failure_and_not_an_absence() {
    let f = Fixture::new();
    let written = f.run();
    std::fs::remove_file(f.object_path(&written.id, "data.000000")).expect("removes");

    let verification = verify(&f.target(), &written.id, Some(&f.identity));
    assert_eq!(
        verification.presence.as_str(),
        "failed",
        "a declared chunk that is gone was reported as {}",
        verification.presence
    );
    assert!(
        verification
            .presence
            .reason()
            .is_some_and(|r| r.contains("not the same as a snapshot that was never taken")),
        "{}",
        verification.presence
    );
    assert!(!verification.contents.is_intact());
}

/// The sharpest form of the rule, at the top of the stack.
#[test]
fn an_unreadable_chunk_is_could_not_run_and_never_a_damaged_snapshot() {
    if unsafe { libc::geteuid() } == 0 {
        eprintln!(
            "SKIP  an_unreadable_chunk_is_could_not_run_and_never_a_damaged_snapshot: \
             running as root, which mode bits do not stop. NOT ASSERTED: that a \
             chunk the kernel refuses produces could-not-run rather than failed \
             or absent."
        );
        return;
    }
    let f = Fixture::new();
    let written = f.run();
    let path = f.object_path(&written.id, "data.000000");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).expect("chmod");
    let verification = verify(&f.target(), &written.id, Some(&f.identity));
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).expect("chmod");

    assert_eq!(
        verification.presence.as_str(),
        "could-not-run",
        "an unreadable chunk was reported as {}, which claims to know whether \
         the backup is good",
        verification.presence
    );
    assert_eq!(verification.contents.as_str(), "could-not-run");
    assert!(!verification.contents.is_intact());
}

/// The head is written last, so its absence is an interrupted run.
#[test]
fn a_snapshot_with_no_head_is_an_interrupted_write_and_says_so() {
    let f = Fixture::new();
    let written = f.run();
    std::fs::remove_file(f.object_path(&written.id, HEAD_OBJECT)).expect("removes");

    let verification = verify(&f.target(), &written.id, Some(&f.identity));
    assert_eq!(verification.presence.as_str(), "absent");
    assert!(verification
        .presence
        .reason()
        .is_some_and(|r| r.contains("interrupted")));
    // And nothing was opened, so nothing may be claimed about the contents.
    assert_eq!(verification.contents.as_str(), "could-not-run");
}

#[test]
fn a_head_that_names_another_snapshot_is_refused_rather_than_believed() {
    let f = Fixture::new();
    let written = f.run();
    let path = f.object_path(&written.id, HEAD_OBJECT);
    let mut head: Head =
        serde_json::from_slice(&std::fs::read(&path).expect("reads")).expect("parses");
    head.snapshot = "20200101T000000Z-00000000".to_string();
    std::fs::write(&path, serde_json::to_vec(&head).expect("serialises")).expect("writes");

    let verification = verify(&f.target(), &written.id, Some(&f.identity));
    assert_eq!(verification.presence.as_str(), "failed");
    assert!(verification
        .presence
        .reason()
        .is_some_and(|r| r.contains("will not guess which")));
}

#[test]
fn a_snapshot_in_a_format_this_build_does_not_know_is_not_reported_as_damaged() {
    let f = Fixture::new();
    let written = f.run();
    let path = f.object_path(&written.id, HEAD_OBJECT);
    let mut head: Head =
        serde_json::from_slice(&std::fs::read(&path).expect("reads")).expect("parses");
    head.format = "apex-backup/2".to_string();
    std::fs::write(&path, serde_json::to_vec(&head).expect("serialises")).expect("writes");

    let verification = verify(&f.target(), &written.id, Some(&f.identity));
    assert_eq!(
        verification.presence.as_str(),
        "could-not-run",
        "a newer format was reported as {}",
        verification.presence
    );
    assert!(verification
        .presence
        .reason()
        .is_some_and(|r| r.contains("not damaged")));
}

/// Truncation: the last chunk of a stream removed, and the head still claiming
/// it.
#[test]
fn a_truncated_data_stream_is_failed_and_never_intact() {
    let f = Fixture::new();
    let big = f.source.join("big.bin");
    std::fs::write(&big, vec![7u8; CHUNK_BYTES + 4096]).expect("writes");
    let written = f.run();
    assert!(
        written.head.data_chunks >= 2,
        "the fixture did not produce more than one chunk"
    );

    let last = written.head.data_chunks - 1;
    std::fs::remove_file(f.object_path(&written.id, &Head::chunk_object(Stream::Data, last)))
        .expect("removes");

    let verification = verify(&f.target(), &written.id, Some(&f.identity));
    assert_eq!(verification.presence.as_str(), "failed");
    assert!(!verification.contents.is_intact());
}

#[test]
fn a_data_stream_of_many_chunks_round_trips_exactly() {
    let f = Fixture::new();
    let payload: Vec<u8> = (0..(CHUNK_BYTES * 2 + 12_345))
        .map(|i| (i % 251) as u8)
        .collect();
    std::fs::write(f.source.join("big.bin"), &payload).expect("writes");
    let written = f.run();
    assert!(written.head.data_chunks >= 3, "{} chunks", written.head.data_chunks);

    let into = f.target_root.parent().expect("parent").join("restored");
    let restored = restore(
        &f.target(),
        &written.id,
        &f.identity,
        &into,
        &RestoreOptions::default(),
    )
    .expect("restores");
    assert!(restored.verdict.is_intact(), "{}", restored.verdict);
    assert_eq!(
        std::fs::read(into.join("big.bin")).expect("reads"),
        payload,
        "a file spanning three chunks did not come back byte for byte"
    );
}

// ── versioned restore, P2-001 criterion 3 ───────────────────────────────────

#[test]
fn three_versions_of_a_tree_restore_to_the_three_things_they_were() {
    let f = Fixture::new();
    let mut ids = Vec::new();
    for (n, text) in [(1u64, "first"), (2, "second"), (3, "third")] {
        std::fs::write(f.source.join("README.md"), text.as_bytes()).expect("writes");
        // Distinct instants, so the ids sort the way the versions happened.
        let written = f.run_with(&RunOptions::default(), 1_700_000_000_000 + n * 86_400_000);
        ids.push((written.id, text));
    }

    let listed = list(&f.target()).expect("lists");
    assert_eq!(listed.len(), 3, "three runs produced {} snapshots", listed.len());
    assert!(
        listed.iter().all(|l| l.verdict.is_intact()),
        "a listing reported a snapshot that is not intact"
    );
    let listed_ids: Vec<&str> = listed.iter().map(|l| l.id.as_str()).collect();
    let expected: Vec<&str> = ids.iter().map(|(i, _)| i.as_str()).collect();
    assert_eq!(listed_ids, expected, "the listing is not oldest-first");

    for (index, (id, text)) in ids.iter().enumerate() {
        let into = f
            .target_root
            .parent()
            .expect("parent")
            .join(format!("restored-{index}"));
        let restored = restore(
            &f.target(),
            id,
            &f.identity,
            &into,
            &RestoreOptions::default(),
        )
        .expect("restores");
        assert!(restored.verdict.is_intact(), "{}", restored.verdict);
        assert_eq!(
            std::fs::read_to_string(into.join("README.md")).expect("reads"),
            *text,
            "restoring version {index} gave the wrong contents"
        );
    }
}

// ── what a restore refuses ──────────────────────────────────────────────────

#[test]
fn a_restore_refuses_a_destination_that_already_holds_something() {
    let f = Fixture::new();
    let written = f.run();
    let into = f.target_root.parent().expect("parent").join("occupied");
    std::fs::create_dir_all(&into).expect("mkdir");
    std::fs::write(into.join("precious.txt"), b"do not lose me").expect("writes");

    let err = restore(
        &f.target(),
        &written.id,
        &f.identity,
        &into,
        &RestoreOptions::default(),
    )
    .expect_err("refuses");
    assert!(err.to_string().contains("is not empty"), "{err}");
    assert_eq!(
        std::fs::read(into.join("precious.txt")).expect("reads"),
        b"do not lose me",
        "the refusal still wrote something"
    );

    // And says so explicitly when told.
    let restored = restore(
        &f.target(),
        &written.id,
        &f.identity,
        &into,
        &RestoreOptions {
            into_non_empty: true,
        },
    )
    .expect("restores when told to");
    assert!(restored.verdict.is_intact(), "{}", restored.verdict);
    assert!(into.join("README.md").exists());
}

/// Found by `tests/test-apex-backup.sh`: a second restore into the same
/// directory failed with EEXIST on the symlink the first one made.
///
/// The fix is not only about EEXIST. A symlink already at an entry's path is
/// something `File::create` FOLLOWS — so a destination holding
/// `etc/passwd -> /etc/passwd` would have had a root restore write the
/// snapshot's bytes outside the destination entirely.
#[test]
fn restoring_again_over_an_existing_tree_replaces_it_rather_than_failing() {
    let f = Fixture::new();
    let written = f.run();
    let into = f.target_root.parent().expect("parent").join("restored");
    let first = restore(
        &f.target(),
        &written.id,
        &f.identity,
        &into,
        &RestoreOptions::default(),
    )
    .expect("restores");
    assert!(first.verdict.is_intact(), "{}", first.verdict);

    let again = restore(
        &f.target(),
        &written.id,
        &f.identity,
        &into,
        &RestoreOptions {
            into_non_empty: true,
        },
    )
    .expect("restores a second time");
    assert!(
        again.verdict.is_intact(),
        "a second restore over the same tree did not verify: {}",
        again.verdict
    );
    assert_eq!(again.symlinks, first.symlinks);
    assert_eq!(
        std::fs::read_link(into.join("link-to-main")).expect("reads"),
        PathBuf::from("src/main.rs")
    );
}

/// The dangerous half, on its own: a symlink planted where a file goes must not
/// be written through.
#[test]
fn a_symlink_in_the_way_is_replaced_and_never_written_through() {
    let f = Fixture::new();
    let written = f.run();
    let into = f.target_root.parent().expect("parent").join("restored");
    let outside = f.target_root.parent().expect("parent").join("outside.txt");
    std::fs::write(&outside, b"not the snapshot's business").expect("writes");

    std::fs::create_dir_all(&into).expect("mkdir");
    // Exactly what a previous run, or anything else that can write here, could
    // leave behind.
    std::os::unix::fs::symlink(&outside, into.join("README.md")).expect("symlink");

    let restored = restore(
        &f.target(),
        &written.id,
        &f.identity,
        &into,
        &RestoreOptions {
            into_non_empty: true,
        },
    )
    .expect("restores");
    assert!(restored.verdict.is_intact(), "{}", restored.verdict);

    assert_eq!(
        std::fs::read(&outside).expect("reads"),
        b"not the snapshot's business",
        "the restore wrote THROUGH a symlink, outside its destination"
    );
    assert_eq!(
        std::fs::read(into.join("README.md")).expect("reads"),
        b"# a project\n"
    );
    assert!(
        !std::fs::symlink_metadata(into.join("README.md"))
            .expect("stats")
            .file_type()
            .is_symlink(),
        "the symlink is still there"
    );
}

/// Overwriting a file is one thing; deleting a directory to make room for a
/// file of the same name is another, and this build does not do it.
#[test]
fn a_directory_in_the_way_is_reported_and_never_deleted() {
    let f = Fixture::new();
    let written = f.run();
    let into = f.target_root.parent().expect("parent").join("restored");
    std::fs::create_dir_all(into.join("README.md")).expect("mkdir");
    std::fs::write(into.join("README.md/precious.txt"), b"do not lose me").expect("writes");

    let restored = restore(
        &f.target(),
        &written.id,
        &f.identity,
        &into,
        &RestoreOptions {
            into_non_empty: true,
        },
    )
    .expect("returns a report");
    assert_eq!(restored.verdict.as_str(), "failed", "{}", restored.verdict);
    assert!(restored
        .verdict
        .reason()
        .is_some_and(|r| r.contains("will not delete a directory")));
    assert_eq!(
        std::fs::read(into.join("README.md/precious.txt")).expect("reads"),
        b"do not lose me"
    );
}

/// A manifest comes back from storage. A storage that can put `../../x` in one
/// can have this program write it — as root, during a restore.
#[test]
fn a_manifest_path_that_climbs_out_of_the_destination_writes_nothing_outside_it() {
    for bad in [
        "../escape",
        "a/../../escape",
        "/etc/cron.d/x",
        "..",
        "",
        "a//b",
        "./a",
    ] {
        assert!(
            safe_relative(bad).is_err(),
            "{bad:?} was accepted as a path to write"
        );
    }
    for good in ["a", "a/b", "a/b/c.txt", "dot.in.name", "a-b_c"] {
        assert!(safe_relative(good).is_ok(), "{good:?} was refused");
    }
}

#[test]
fn a_restore_whose_manifest_climbs_out_writes_nothing_outside_and_says_failed() {
    let f = Fixture::new();
    let written = f.run();

    // Rewrite the manifest with a hostile entry, resealing it the way a far
    // side that held the recipient could.
    let head_bytes = std::fs::read(f.object_path(&written.id, HEAD_OBJECT)).expect("reads");
    let head: Head = serde_json::from_slice(&head_bytes).expect("parses");
    let key = open_with(
        &f.identity,
        &head.ephemeral_bytes().expect("decodes"),
        written.id.as_str(),
    )
    .expect("derives");

    let mut manifest = Manifest {
        entries: vec![Entry {
            path: "../../escaped.txt".to_string(),
            kind: Kind::File,
            size: 0,
            offset: 0,
            mode: 0o644,
            mtime_ms: 0,
            digest: digest_hex(b""),
        }],
        data_bytes: 0,
        skipped: Vec::new(),
    };
    manifest.entries[0].size = 0;
    let text = serde_json::to_vec(&manifest).expect("serialises");
    let sealed = key
        .seal_chunk(Stream::Manifest, 0, true, &text)
        .expect("seals");
    // One manifest chunk, and a data stream of one empty chunk.
    let empty = key.seal_chunk(Stream::Data, 0, true, b"").expect("seals");
    let dir = f
        .target_root
        .join("apex-backup")
        .join(written.id.as_str());
    for old in std::fs::read_dir(&dir).expect("reads").flatten() {
        std::fs::remove_file(old.path()).expect("removes");
    }
    std::fs::write(dir.join("manifest.000000"), &sealed).expect("writes");
    std::fs::write(dir.join("data.000000"), &empty).expect("writes");
    let mut head = head;
    head.manifest_chunks = 1;
    head.data_chunks = 1;
    std::fs::write(
        dir.join(HEAD_OBJECT),
        serde_json::to_vec(&head).expect("serialises"),
    )
    .expect("writes");

    let into = f.target_root.parent().expect("parent").join("restored");
    let restored = restore(
        &f.target(),
        &written.id,
        &f.identity,
        &into,
        &RestoreOptions::default(),
    )
    .expect("returns a report");

    assert_eq!(
        restored.verdict.as_str(),
        "failed",
        "a traversal was restored quietly: {}",
        restored.verdict
    );
    assert_eq!(restored.files, 0);
    let outside = f.target_root.parent().expect("parent").join("escaped.txt");
    assert!(!outside.exists(), "a manifest wrote outside the destination");
    assert!(!PathBuf::from("/var/tmp/escaped.txt").exists());
}

// ── incomplete sources ──────────────────────────────────────────────────────

#[test]
fn a_source_that_cannot_be_read_whole_refuses_rather_than_leaving_files_out() {
    if unsafe { libc::geteuid() } == 0 {
        eprintln!(
            "SKIP  a_source_that_cannot_be_read_whole_refuses_rather_than_leaving_files_out: \
             running as root. NOT ASSERTED: that a run refuses when part of the \
             source cannot be read, rather than writing a snapshot that silently \
             omits it."
        );
        return;
    }
    let f = Fixture::new();
    let locked = f.source.join("locked");
    std::fs::create_dir(&locked).expect("mkdir");
    std::fs::write(locked.join("inside.txt"), b"unreachable").expect("writes");
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).expect("chmod");

    let refused = write(
        &f.source,
        &f.target(),
        &f.recipient,
        &config(),
        &RunOptions::default(),
        crate::now_ms(),
    );
    let carried_on = write(
        &f.source,
        &f.target(),
        &f.recipient,
        &config(),
        &RunOptions {
            skip_unreadable: true,
            ..RunOptions::default()
        },
        crate::now_ms(),
    );
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o700)).expect("chmod");

    let err = refused.expect_err("a run that cannot read the tree refuses");
    assert!(
        err.to_string().contains("claims to hold a tree it does not"),
        "{err}"
    );

    // And when told to carry on, it records what it left out.
    let written = carried_on.expect("carries on when told to");
    assert_eq!(written.skipped.len(), 1, "{:?}", written.skipped);
    assert_eq!(written.skipped[0].path, "locked");

    // The record survives into the restore, so a restore never looks complete
    // when the backup was not.
    let into = f.target_root.parent().expect("parent").join("restored");
    let restored = restore(
        &f.target(),
        &written.id,
        &f.identity,
        &into,
        &RestoreOptions::default(),
    )
    .expect("restores");
    assert_eq!(restored.skipped_at_backup.len(), 1);
    assert_eq!(restored.skipped_at_backup[0].path, "locked");
}

// ── the head's own claims ───────────────────────────────────────────────────

/// A target that records the ORDER its objects were stored in.
///
/// The first version of the test below compared mtimes, and a mutation showed
/// what that is worth: a filesystem's timestamp granularity and a writer's
/// speed decide the answer, not the program. What matters is the order of the
/// `put` calls, so that is what is recorded.
struct Recording {
    inner: FsTarget,
    puts: std::cell::RefCell<Vec<String>>,
}

impl Recording {
    fn new(inner: FsTarget) -> Recording {
        Recording {
            inner,
            puts: std::cell::RefCell::new(Vec::new()),
        }
    }
}

impl Target for Recording {
    fn describe(&self) -> String {
        self.inner.describe()
    }
    fn prepare(&self) -> Result<(), crate::target::TargetError> {
        self.inner.prepare()
    }
    fn put(
        &self,
        snapshot: &str,
        object: &str,
        bytes: &[u8],
    ) -> Result<(), crate::target::TargetError> {
        self.puts.borrow_mut().push(object.to_string());
        self.inner.put(snapshot, object, bytes)
    }
    fn get(&self, snapshot: &str, object: &str) -> Result<Vec<u8>, crate::target::TargetError> {
        self.inner.get(snapshot, object)
    }
    fn list(&self) -> Result<Vec<SnapshotId>, crate::target::TargetError> {
        self.inner.list()
    }
}

#[test]
fn the_head_is_the_last_object_a_run_writes() {
    let f = Fixture::new();
    let target = Recording::new(f.target());
    let written = write(
        &f.source,
        &target,
        &f.recipient,
        &config(),
        &RunOptions::default(),
        crate::now_ms(),
    )
    .expect("runs");

    let puts = target.puts.borrow().clone();
    assert!(puts.len() > 2, "only {} object(s) were written", puts.len());
    assert_eq!(
        puts.last().map(String::as_str),
        Some(HEAD_OBJECT),
        "the head was not the last object written; the order was {puts:?}"
    );
    assert_eq!(
        puts.iter().filter(|p| p.as_str() == HEAD_OBJECT).count(),
        1,
        "the head was written more than once"
    );
    // And the data stream is written before the manifest that describes it.
    let first_manifest = puts
        .iter()
        .position(|p| p.starts_with("manifest."))
        .expect("a manifest chunk");
    let last_data = puts
        .iter()
        .rposition(|p| p.starts_with("data."))
        .expect("a data chunk");
    assert!(last_data < first_manifest, "{puts:?}");
    assert_eq!(written.head.objects().len(), puts.len());
}

#[test]
fn the_head_declares_exactly_the_objects_that_are_there() {
    let f = Fixture::new();
    let written = f.run();
    let dir = f.target_root.join("apex-backup").join(written.id.as_str());
    let mut on_disk: Vec<String> = std::fs::read_dir(&dir)
        .expect("reads")
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    on_disk.sort();
    let mut declared = written.head.objects();
    declared.sort();
    assert_eq!(on_disk, declared);
}

/// A listing must not need a key, and must not claim anything a key would be
/// needed for.
#[test]
fn a_listing_reads_heads_and_opens_nothing() {
    let f = Fixture::new();
    let written = f.run();
    let listed = list(&f.target()).expect("lists");
    assert_eq!(listed.len(), 1);
    let only = &listed[0];
    assert_eq!(only.id, written.id);
    let head = only.head.as_ref().expect("a head");
    assert_eq!(head.recipient, f.recipient.to_string_value());
    assert!(only.verdict.is_intact());
    // The label is a label and never a path.
    assert_eq!(head.label, "");
}

#[test]
fn a_label_is_carried_and_is_not_a_path() {
    let f = Fixture::new();
    let written = f.run_with(
        &RunOptions {
            label: "nightly".to_string(),
            ..RunOptions::default()
        },
        crate::now_ms(),
    );
    assert_eq!(written.head.label, "nightly");
    let head_bytes = std::fs::read(f.object_path(&written.id, HEAD_OBJECT)).expect("reads");
    let text = String::from_utf8_lossy(&head_bytes);
    assert!(
        !text.contains(&f.source.to_string_lossy().into_owned()),
        "the source path is in the plaintext head: {text}"
    );
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}
