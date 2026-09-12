//! Fixtures live under `/var/tmp`, never `/tmp`, and nothing outside one is
//! written, read or removed.

use super::*;
use crate::target::Target;

fn work() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("apex-backup-fs-")
        .tempdir_in("/var/tmp")
        .expect("a fixture directory under /var/tmp")
}

fn initialised(dir: &Path) -> String {
    let marker = Marker::new(1).expect("a marker");
    let target = FsTarget::local(dir.to_path_buf(), "backups", None);
    target.write_marker(&marker).expect("writes the marker");
    marker.id
}

#[test]
fn an_object_written_comes_back_byte_for_byte() {
    let dir = work();
    let id = initialised(dir.path());
    let target = FsTarget::local(dir.path().to_path_buf(), "backups", Some(id));
    target.prepare().expect("prepares");

    let bytes: Vec<u8> = (0..=255u8).cycle().take(5000).collect();
    target
        .put("20260912T014233Z-0badc0de", "data.000000", &bytes)
        .expect("puts");
    assert_eq!(
        target.get("20260912T014233Z-0badc0de", "data.000000"),
        Ok(bytes)
    );
}

#[test]
fn an_object_that_was_never_written_is_absent_and_not_a_failure() {
    let dir = work();
    let id = initialised(dir.path());
    let target = FsTarget::local(dir.path().to_path_buf(), "backups", Some(id));
    target.prepare().expect("prepares");

    let err = target
        .get("20260912T014233Z-0badc0de", "data.000000")
        .expect_err("nothing there");
    assert!(matches!(err, TargetError::Absent(_)), "{err:?}");
    assert_eq!(err.verdict("the chunk").as_str(), "absent");
}

/// The whole reason the `nas` kind exists.
///
/// A directory that is not a mount point is a perfectly good local directory,
/// and writing a backup into it succeeds, verifies, and is lost with the
/// machine.
#[test]
fn a_nas_target_that_is_not_a_mount_point_is_refused_before_anything_is_written() {
    let dir = work();
    let root = dir.path().join("mountpoint");
    std::fs::create_dir(&root).expect("makes the directory");
    let target = FsTarget::nas(root.clone(), "backups", None);

    let err = target.prepare().expect_err("refuses");
    assert!(matches!(err, TargetError::Unavailable(_)), "{err:?}");
    assert!(
        err.to_string().contains("not a mount point"),
        "the message has to name the cause: {err}"
    );
    assert!(
        !root.join("backups").exists(),
        "the refusal still created the snapshot directory"
    );

    // And the read path refuses identically, so a listing of an unmounted NAS
    // cannot come back empty and calm.
    let listed = target.list().expect_err("refuses");
    assert!(matches!(listed, TargetError::Unavailable(_)), "{listed:?}");
    assert_eq!(listed.verdict("the target").as_str(), "could-not-run");
}

/// The same path as a `local` target is fine: `local` means this machine.
#[test]
fn a_local_target_does_not_have_to_be_a_mount_point() {
    let dir = work();
    let id = initialised(dir.path());
    let target = FsTarget::local(dir.path().to_path_buf(), "backups", Some(id));
    target.prepare().expect("prepares");
    assert_eq!(target.list(), Ok(Vec::new()));
}

/// `st_dev` against the parent's, checked against the kernel's own answer.
#[test]
fn a_mount_point_is_recognised_as_one() {
    let root = FsTarget::nas(PathBuf::from("/"), "b", None);
    assert_eq!(root.is_mount_point(), Ok(true), "/ is a mount point");

    // /proc is a mount on every machine this can run on, and reading its
    // metadata writes nothing.
    let proc = FsTarget::nas(PathBuf::from("/proc"), "b", None);
    assert_eq!(proc.is_mount_point(), Ok(true), "/proc is a mount point");

    let dir = work();
    let inner = dir.path().join("inner");
    std::fs::create_dir(&inner).expect("makes it");
    let plain = FsTarget::nas(inner, "b", None);
    assert_eq!(plain.is_mount_point(), Ok(false));
}

/// A marker that is not there means the target was never established, or is
/// not mounted. Neither of those is "there are no backups here".
#[test]
fn a_missing_marker_is_could_not_run_and_never_an_empty_history() {
    let dir = work();
    let target = FsTarget::local(dir.path().to_path_buf(), "backups", Some("cafe1234".into()));

    let err = target.prepare().expect_err("refuses");
    assert!(matches!(err, TargetError::Unavailable(_)), "{err:?}");
    assert_eq!(err.verdict("the target").as_str(), "could-not-run");
    assert!(
        err.to_string().contains("not the same as a target with no snapshots"),
        "{err}"
    );

    let listed = target.list().expect_err("refuses");
    assert_eq!(listed.verdict("the target").as_str(), "could-not-run");
}

/// A different filesystem mounted where the right one used to be is the case
/// the mount-point check alone does not catch.
#[test]
fn a_marker_from_a_different_filesystem_is_refused_rather_than_started_afresh() {
    let dir = work();
    let actual = initialised(dir.path());
    let target = FsTarget::local(
        dir.path().to_path_buf(),
        "backups",
        Some("0000000000000000".to_string()),
    );

    let err = target.prepare().expect_err("refuses");
    assert!(err.to_string().contains(&actual), "names what it found: {err}");
    assert!(
        err.to_string().contains("second history"),
        "says what would happen: {err}"
    );
}

#[test]
fn a_target_with_no_recorded_marker_does_not_require_one() {
    // `apex backup init` records the id; a project that has not run it yet can
    // still read a target, which is what makes `init` able to report on one.
    let dir = work();
    let target = FsTarget::local(dir.path().to_path_buf(), "backups", None);
    target.prepare().expect("prepares");
    assert_eq!(target.list(), Ok(Vec::new()));
}

#[test]
fn snapshots_are_listed_oldest_first_and_only_the_ones_that_are_snapshots() {
    let dir = work();
    let id = initialised(dir.path());
    let target = FsTarget::local(dir.path().to_path_buf(), "backups", Some(id));
    target.prepare().expect("prepares");

    for snapshot in [
        "20260912T014233Z-0badc0de",
        "20250101T000000Z-aaaaaaaa",
        "20270630T120000Z-ffffffff",
    ] {
        target.put(snapshot, HEAD_LIKE, b"{}").expect("puts");
    }
    // Things that are not snapshots, in the same directory.
    std::fs::create_dir(dir.path().join("backups").join("notes")).expect("makes it");
    std::fs::write(dir.path().join("backups").join("README"), b"hello").expect("writes");

    let listed = target.list().expect("lists");
    let names: Vec<&str> = listed.iter().map(|i| i.as_str()).collect();
    assert_eq!(
        names,
        vec![
            "20250101T000000Z-aaaaaaaa",
            "20260912T014233Z-0badc0de",
            "20270630T120000Z-ffffffff",
        ]
    );
}

const HEAD_LIKE: &str = "head.json";

/// The one that matters most on the read path, and the one a test running as
/// root cannot make: root is not stopped by mode bits.
#[test]
fn an_unreadable_target_is_could_not_run_and_not_an_empty_history() {
    if unsafe { libc::geteuid() } == 0 {
        eprintln!(
            "SKIP  an_unreadable_target_is_could_not_run_and_not_an_empty_history: \
             running as root, which mode bits do not stop. NOT ASSERTED: that a \
             directory refused by the kernel is reported as could-not-run rather \
             than as a target with no snapshots in it."
        );
        return;
    }
    let dir = work();
    let id = initialised(dir.path());
    let target = FsTarget::local(dir.path().to_path_buf(), "backups", Some(id.clone()));
    target.prepare().expect("prepares");
    target
        .put("20260912T014233Z-0badc0de", HEAD_LIKE, b"{}")
        .expect("puts");

    // It really was listable a moment ago.
    assert_eq!(target.list().map(|v| v.len()), Ok(1));

    let base = dir.path().join("backups");
    std::fs::set_permissions(&base, std::fs::Permissions::from_mode(0o000))
        .expect("removes every bit");

    let listed = target.list();
    // Put the bits back before any assertion can fail and leave the fixture
    // undeletable.
    std::fs::set_permissions(&base, std::fs::Permissions::from_mode(0o700))
        .expect("puts them back");

    let err = listed.expect_err("an unreadable directory is not an empty one");
    assert_eq!(
        err.verdict("the snapshot directory").as_str(),
        "could-not-run",
        "an unreadable snapshot directory was reported as {err:?}, which tells \
         an operator their backups are gone"
    );
}

#[test]
fn an_unreadable_object_is_could_not_run_and_not_a_missing_one() {
    if unsafe { libc::geteuid() } == 0 {
        eprintln!(
            "SKIP  an_unreadable_object_is_could_not_run_and_not_a_missing_one: \
             running as root. NOT ASSERTED: that a chunk refused by the kernel \
             is reported as could-not-run rather than as absent."
        );
        return;
    }
    let dir = work();
    let id = initialised(dir.path());
    let target = FsTarget::local(dir.path().to_path_buf(), "backups", Some(id));
    target.prepare().expect("prepares");
    target
        .put("20260912T014233Z-0badc0de", "data.000000", b"ciphertext")
        .expect("puts");

    let path = dir
        .path()
        .join("backups")
        .join("20260912T014233Z-0badc0de")
        .join("data.000000");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).expect("chmod");
    let got = target.get("20260912T014233Z-0badc0de", "data.000000");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).expect("chmod");

    let err = got.expect_err("unreadable");
    assert_eq!(err.verdict("the chunk").as_str(), "could-not-run");
}

/// A reader must never see half an object, so the write is a rename.
#[test]
fn a_partial_write_never_appears_under_the_object_s_own_name() {
    let dir = work();
    let id = initialised(dir.path());
    let target = FsTarget::local(dir.path().to_path_buf(), "backups", Some(id));
    target.prepare().expect("prepares");
    target
        .put("20260912T014233Z-0badc0de", "data.000000", b"whole")
        .expect("puts");

    let snap = dir.path().join("backups").join("20260912T014233Z-0badc0de");
    let leftovers: Vec<String> = std::fs::read_dir(&snap)
        .expect("reads")
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".partial"))
        .collect();
    assert!(leftovers.is_empty(), "left {leftovers:?} behind");
}

#[test]
fn an_object_is_private_to_its_owner_on_disk() {
    let dir = work();
    let id = initialised(dir.path());
    let target = FsTarget::local(dir.path().to_path_buf(), "backups", Some(id));
    target.prepare().expect("prepares");
    target
        .put("20260912T014233Z-0badc0de", "data.000000", b"ciphertext")
        .expect("puts");

    let path = dir
        .path()
        .join("backups")
        .join("20260912T014233Z-0badc0de")
        .join("data.000000");
    let mode = std::fs::metadata(&path).expect("stats").permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "the object is mode {mode:o}");

    let dir_mode = std::fs::metadata(dir.path().join("backups"))
        .expect("stats")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(dir_mode, 0o700, "the snapshot directory is mode {dir_mode:o}");
}

#[test]
fn a_marker_round_trips_and_a_file_that_is_not_one_is_not_read_as_one() {
    let dir = work();
    let marker = Marker::new(1_789_177_353_000).expect("makes one");
    let target = FsTarget::local(dir.path().to_path_buf(), "backups", None);
    target.write_marker(&marker).expect("writes");
    assert_eq!(target.read_marker(), Ok(marker.clone()));
    assert_eq!(marker.id.len(), 16, "the id is 16 hex characters");

    std::fs::write(target.marker_path(), b"not json at all").expect("clobbers");
    let err = target.read_marker().expect_err("refuses");
    assert!(matches!(err, TargetError::Unavailable(_)), "{err:?}");
    assert_eq!(err.verdict("the marker").as_str(), "could-not-run");
}

#[test]
fn a_target_root_that_does_not_exist_is_absent_rather_than_created() {
    let dir = work();
    let target = FsTarget::local(dir.path().join("nowhere"), "backups", None);
    let err = target.prepare().expect_err("refuses");
    assert!(matches!(err, TargetError::Absent(_)), "{err:?}");
    assert!(
        !dir.path().join("nowhere").exists(),
        "a missing target root was created rather than reported"
    );
}

#[test]
fn a_target_root_that_is_a_file_is_not_a_directory_with_nothing_in_it() {
    let dir = work();
    let path = dir.path().join("afile");
    std::fs::write(&path, b"x").expect("writes");
    let target = FsTarget::local(path, "backups", None);
    let err = target.prepare().expect_err("refuses");
    assert!(err.to_string().contains("not a directory"), "{err}");
}
