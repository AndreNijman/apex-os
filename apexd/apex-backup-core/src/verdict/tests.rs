use super::*;

/// The assertion this repository has had to make sixteen times.
#[test]
fn a_permission_denial_is_could_not_run_and_never_absent() {
    let denied = std::io::Error::from_raw_os_error(libc::EACCES);
    let verdict = from_io("the snapshot directory", &denied);
    assert!(
        matches!(verdict, Verdict::CouldNotRun(_)),
        "EACCES became {verdict:?}"
    );
    let reason = verdict.reason().unwrap_or_default();
    let squashed: String = reason.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        squashed.contains("a refusal and not an absence"),
        "the message has to say so in words an operator will read: {verdict}"
    );
    assert!(!verdict.is_intact());
}

#[test]
fn only_a_missing_thing_is_absent() {
    let missing = std::io::Error::from_raw_os_error(libc::ENOENT);
    assert!(matches!(
        from_io("the snapshot directory", &missing),
        Verdict::Absent(_)
    ));

    // A component of the path is a file: nothing of ours is there, and that is
    // a real answer rather than a failure to look.
    let not_a_dir = std::io::Error::from_raw_os_error(libc::ENOTDIR);
    assert!(matches!(
        from_io("the snapshot directory", &not_a_dir),
        Verdict::Absent(_)
    ));
}

/// Every errno that is neither "not there" nor "a file is in the way" has to
/// land on `CouldNotRun`. Enumerated rather than argued, because the default
/// arm is where this goes wrong.
#[test]
fn every_other_errno_is_could_not_run() {
    for errno in [
        libc::EACCES,
        libc::EPERM,
        libc::EIO,
        libc::ELOOP,
        libc::ENAMETOOLONG,
        libc::ENOMEM,
        libc::EMFILE,
        libc::ENFILE,
        libc::ESTALE,
        libc::ETIMEDOUT,
        libc::EROFS,
        libc::ENOSPC,
        libc::EBUSY,
        libc::EAGAIN,
    ] {
        let err = std::io::Error::from_raw_os_error(errno);
        let verdict = from_io("the target", &err);
        assert!(
            matches!(verdict, Verdict::CouldNotRun(_)),
            "errno {errno} became {verdict:?}, which claims to know something \
             it does not"
        );
    }
}

#[test]
fn exactly_one_state_may_be_relied_on() {
    assert!(Verdict::Intact { checked: "all of it".into() }.is_intact());
    assert!(!Verdict::Absent("gone".into()).is_intact());
    assert!(!Verdict::Failed("bad".into()).is_intact());
    assert!(!Verdict::CouldNotRun("unknown".into()).is_intact());
}

/// Rolling many objects into one answer must not lose the fact that something
/// was not looked at.
#[test]
fn could_not_run_outranks_absent_when_a_snapshot_is_summarised() {
    let unknown = Verdict::CouldNotRun("EACCES on chunk 3".into());
    let gone = Verdict::Absent("chunk 4 is not there".into());
    assert_eq!(
        unknown.clone().worse_of(gone.clone()).as_str(),
        "could-not-run"
    );
    assert_eq!(gone.worse_of(unknown).as_str(), "could-not-run");
}

#[test]
fn a_failure_outranks_everything() {
    let failed = Verdict::Failed("chunk 1 does not open".into());
    for other in [
        Verdict::Intact { checked: "x".into() },
        Verdict::Absent("y".into()),
        Verdict::CouldNotRun("z".into()),
    ] {
        assert_eq!(failed.clone().worse_of(other.clone()).as_str(), "failed");
        assert_eq!(other.worse_of(failed.clone()).as_str(), "failed");
    }
}

#[test]
fn intact_is_the_only_state_with_no_reason_and_it_says_what_it_checked() {
    let intact = Verdict::Intact { checked: "4 chunks, 12 files".into() };
    assert_eq!(intact.reason(), None);
    assert_eq!(
        intact.to_json()["checked"],
        serde_json::Value::from("4 chunks, 12 files")
    );
    for v in [
        Verdict::Absent("a".into()),
        Verdict::Failed("b".into()),
        Verdict::CouldNotRun("c".into()),
    ] {
        assert!(v.reason().is_some(), "{v:?} has no reason");
        assert!(v.to_json().get("reason").is_some());
    }
}

/// The four names are a wire surface: `apex backup list --json` prints them and
/// something will branch on them.
#[test]
fn the_four_names_are_pinned() {
    assert_eq!(Verdict::Intact { checked: String::new() }.as_str(), "intact");
    assert_eq!(Verdict::Absent(String::new()).as_str(), "absent");
    assert_eq!(Verdict::Failed(String::new()).as_str(), "failed");
    assert_eq!(Verdict::CouldNotRun(String::new()).as_str(), "could-not-run");
}

/// The wording is `verify.rs`'s, so an operator who has read one report has
/// read both.
#[test]
fn could_not_run_renders_as_not_checked_rather_than_as_a_failure() {
    let v = Verdict::CouldNotRun("the mount is not there".into());
    assert_eq!(v.to_string(), "not checked — the mount is not there");
    assert!(!v.to_string().contains("FAIL"));
}
