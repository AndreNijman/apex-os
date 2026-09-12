use super::*;

/// The paragraph `apex backup targets` would print for a kind this build did
/// not carry.
///
/// A literal, and it used to be `Kind::S3.unimplemented_reason()`. That stopped
/// working the moment S3 became real — every kind now answers `None` — and the
/// interesting part is which way it broke: `.expect("s3 refuses")` PANICKED,
/// loudly, rather than wrapping an empty string and passing. A test whose
/// subject disappears should fail, and this one did.
const A_REFUSAL: &str = "this build cannot sign an S3 request. S3 needs SigV4, \
                         and nothing in this workspace has a signer — the \
                         Cloudflare broker spends a bearer token inside a curl \
                         it owns and speaks no TLS of its own. Use r2, which is \
                         brokered and bucket-scoped";

#[test]
fn a_paragraph_is_wrapped_at_word_boundaries_and_never_mid_word() {
    let text = A_REFUSAL;
    let lines = wrap(text, 68);
    assert!(lines.len() > 2, "a whole paragraph on {} line(s)", lines.len());
    for line in &lines {
        assert!(line.len() <= 68, "'{line}' is {} characters", line.len());
        assert!(!line.starts_with(' '), "'{line}'");
        assert!(!line.ends_with(' '), "'{line}'");
    }
    // Nothing is lost: the words come back in order.
    assert_eq!(
        lines.join(" ").split_whitespace().collect::<Vec<_>>(),
        text.split_whitespace().collect::<Vec<_>>()
    );
}

/// Every one of §13.5's five is carried, and none of them still prints a
/// reason it is not.
///
/// The pair of assertions, not one: a build that flipped `is_implemented`
/// without clearing the reason would tell an operator to give up on a target
/// that works, and one that cleared the reason without implementing it would
/// print nothing at all where an explanation belongs.
#[test]
fn every_target_kind_this_build_names_is_carried_and_none_still_refuses() {
    for kind in Kind::ALL {
        assert!(kind.is_implemented(), "{kind} is not carried");
        assert!(
            kind.unimplemented_reason().is_none(),
            "{kind} is carried and still prints a refusal"
        );
    }
    assert_eq!(Kind::ALL.len(), 5, "§13.5 names five");
}

#[test]
fn a_single_word_longer_than_the_width_is_still_printed() {
    let lines = wrap("short aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa end", 10);
    assert_eq!(lines.join(" ").split_whitespace().count(), 3);
}

/// `sudo apex backup key init` must make a key for the person who typed it,
/// not for root — otherwise every account on a machine shares one key and the
/// per-uid store means nothing.
///
/// Asked of a value rather than of the process: `set_var` is process-wide and
/// these tests run in parallel threads, so a test that moved `SUDO_UID` would
/// be moving it under every other test in this binary at the same time.
#[test]
fn under_sudo_the_key_belongs_to_the_account_that_invoked_the_command() {
    assert_eq!(owner_uid_from(Some("1000")).expect("parses"), 1000);
    // Whitespace is what an environment picks up in passing, not an attack.
    assert_eq!(owner_uid_from(Some(" 1001\n")).expect("parses"), 1001);
    // No sudo: the account that is actually running.
    assert_eq!(owner_uid_from(None).expect("no sudo"), unsafe {
        libc::geteuid()
    });
    // And something that is not a user id is refused rather than quietly
    // becoming root's key.
    for bad in ["", "root", "-1", "1000; rm -rf /", "99999999999999999999", "0x0"] {
        assert!(
            owner_uid_from(Some(bad)).is_err(),
            "SUDO_UID={bad:?} was accepted as a user id"
        );
    }
}

/// The override exists for the suite. A build that ignored it would have the
/// suite writing to /var/lib on a developer's machine.
#[test]
fn the_key_directory_is_the_system_one_unless_the_environment_moves_it() {
    assert_eq!(
        store_at(None).root(),
        std::path::Path::new(apex_backup_core::keys::SYSTEM_ROOT)
    );
    assert_eq!(
        store_at(Some("/var/tmp/somewhere-else".into())).root(),
        std::path::Path::new("/var/tmp/somewhere-else")
    );
}
