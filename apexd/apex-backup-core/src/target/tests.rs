use super::*;

/// P2-001 criterion 1 names five kinds, and this build now carries all five.
///
/// It carried three and refused two — `ssh` and `s3` — until P2-001's second
/// round added an ssh transport and a SigV4 signer. The list is pinned rather
/// than computed so that a kind quietly ceasing to be carried fails here.
#[test]
fn all_five_kinds_parse_and_all_five_are_carried() {
    for kind in Kind::ALL {
        assert_eq!(Kind::parse(kind.as_str()), Some(kind));
    }
    let implemented: Vec<&str> = Kind::ALL
        .into_iter()
        .filter(|k| k.is_implemented())
        .map(Kind::as_str)
        .collect();
    assert_eq!(implemented, vec!["local", "nas", "ssh", "s3", "r2"]);
}

/// The two halves of the refusal machinery, held together.
///
/// **Nothing refuses today**, so the `Some` arm below runs zero times, and
/// saying so is the point: the invariant is not "there is a refusal", it is
/// "implemented and refusing are opposites". A build that adds a sixth kind
/// gets the `Some` arm back, and a build that flips `is_implemented` without
/// clearing the reason — which would tell an operator to give up on a target
/// that works — fails here either way.
#[test]
fn every_unimplemented_kind_says_what_is_missing_and_every_implemented_one_says_nothing() {
    let mut refusing = 0;
    for kind in Kind::ALL {
        match kind.unimplemented_reason() {
            Some(why) => {
                assert!(!kind.is_implemented(), "{kind} refuses and is implemented");
                assert!(why.len() > 80, "{kind}'s reason is a stub: {why}");
                refusing += 1;
            }
            None => assert!(
                kind.is_implemented(),
                "{kind} has no reason and is not implemented, so an operator \
                 who names it is told nothing at all"
            ),
        }
    }
    assert_eq!(
        refusing, 0,
        "a kind started refusing; add it to the vocabulary tests and to \
         `apex backup targets`'s expected output"
    );
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
