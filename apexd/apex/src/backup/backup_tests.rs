use super::*;

#[test]
fn a_paragraph_is_wrapped_at_word_boundaries_and_never_mid_word() {
    let text = Kind::S3.unimplemented_reason().expect("s3 refuses");
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
