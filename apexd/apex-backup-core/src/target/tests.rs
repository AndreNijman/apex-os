use super::*;

/// P2-001 criterion 1 names five kinds. This build carries three and refuses
/// two, and the refusals are part of the vocabulary rather than gaps in it —
/// an operator who writes `target = "ssh"` must be told this build will not do
/// it, not told that `ssh` is not a word.
#[test]
fn all_five_kinds_parse_and_exactly_two_refuse() {
    for kind in Kind::ALL {
        assert_eq!(Kind::parse(kind.as_str()), Some(kind));
    }
    let implemented: Vec<&str> = Kind::ALL
        .into_iter()
        .filter(|k| k.is_implemented())
        .map(Kind::as_str)
        .collect();
    assert_eq!(implemented, vec!["local", "nas", "r2"]);

    let refusing: Vec<&str> = Kind::ALL
        .into_iter()
        .filter(|k| !k.is_implemented())
        .map(Kind::as_str)
        .collect();
    assert_eq!(refusing, vec!["ssh", "s3"]);
}

/// A kind that is not implemented must say what it would take. "Not
/// implemented" on its own sends an operator to the source.
#[test]
fn every_unimplemented_kind_says_what_is_missing_and_every_implemented_one_says_nothing() {
    for kind in Kind::ALL {
        match kind.unimplemented_reason() {
            Some(why) => {
                assert!(!kind.is_implemented(), "{kind} refuses and is implemented");
                assert!(why.len() > 80, "{kind}'s reason is a stub: {why}");
            }
            None => assert!(kind.is_implemented(), "{kind} has no reason and is not implemented"),
        }
    }
    assert!(Kind::S3.unimplemented_reason().is_some_and(|w| w.contains("SigV4")));
    assert!(Kind::Ssh.unimplemented_reason().is_some_and(|w| w.contains("sshd")));
}

#[test]
fn a_kind_this_build_has_never_heard_of_is_not_quietly_a_local_directory() {
    for bad in ["", "LOCAL", "nfs", "ftp", "local ", "r2/", "s3:"] {
        assert_eq!(Kind::parse(bad), None, "accepted {bad:?}");
    }
}

/// The function every target error passes through on its way to an operator.
#[test]
fn exactly_one_target_error_is_an_absence() {
    assert_eq!(
        TargetError::Absent("no such object".into())
            .verdict("chunk 3")
            .as_str(),
        "absent"
    );
    for e in [
        TargetError::Denied("EACCES".into()),
        TargetError::Unavailable("connection refused".into()),
        TargetError::Refused("not configured".into()),
    ] {
        assert_eq!(
            e.verdict("chunk 3").as_str(),
            "could-not-run",
            "{e:?} claimed to know something it does not"
        );
    }
}

/// The sharpest form of the rule, and the one this repository has had to learn
/// repeatedly: a refusal reported as an absence.
#[test]
fn a_denial_becomes_could_not_run_and_says_why_in_words() {
    let verdict = TargetError::Denied("403 from the far side".into()).verdict("the bucket");
    assert_eq!(verdict.as_str(), "could-not-run");
    let reason = verdict.reason().unwrap_or_default();
    let squashed: String = reason.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(
        squashed.contains("a refusal and not an absence"),
        "{verdict}"
    );
}

#[test]
fn an_io_error_becomes_a_target_error_by_the_same_three_way_split() {
    let cases = [
        (libc::ENOENT, "absent"),
        (libc::ENOTDIR, "absent"),
        (libc::EACCES, "could-not-run"),
        (libc::EPERM, "could-not-run"),
        (libc::EIO, "could-not-run"),
        (libc::ESTALE, "could-not-run"),
        (libc::ETIMEDOUT, "could-not-run"),
    ];
    for (errno, want) in cases {
        let err = std::io::Error::from_raw_os_error(errno);
        assert_eq!(
            from_io("the target", &err).verdict("the target").as_str(),
            want,
            "errno {errno}"
        );
    }
}
