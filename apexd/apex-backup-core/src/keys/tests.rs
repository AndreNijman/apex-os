//! The key directory, under a `/var/tmp` fixture root. The real
//! `/var/lib/apex-backup` is never read, written or created.

use super::*;

fn work() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix("apex-backup-keys-")
        .tempdir_in("/var/tmp")
        .expect("a fixture directory under /var/tmp")
}

#[test]
fn a_generated_key_opens_what_its_recipient_seals() {
    let dir = work();
    let store = KeyStore::new(dir.path().join("state"));
    let recipient = store.generate(1000).expect("generates");

    let identity = store.identity(1000).expect("reads it back");
    assert_eq!(identity.recipient(), recipient);
    assert_eq!(store.registered(1000).expect("reads"), recipient);
}

#[test]
fn the_private_half_is_root_only_and_the_public_half_is_not() {
    use std::os::unix::fs::PermissionsExt;
    let dir = work();
    let store = KeyStore::new(dir.path().join("state"));
    store.generate(1000).expect("generates");

    let key = std::fs::metadata(store.identity_path(1000)).expect("stats");
    assert_eq!(key.permissions().mode() & 0o777, 0o600, "the private half");

    let keys_dir = std::fs::metadata(store.root().join("keys")).expect("stats");
    assert_eq!(
        keys_dir.permissions().mode() & 0o777,
        0o700,
        "the directory holding private halves"
    );

    let pub_half = std::fs::metadata(store.recipient_path(1000)).expect("stats");
    assert_eq!(
        pub_half.permissions().mode() & 0o777,
        0o644,
        "the public half, which backing up needs and which is not a secret"
    );
}

/// Replacing a key makes every snapshot sealed to the old one unopenable.
#[test]
fn generating_over_an_existing_key_is_refused_rather_than_done() {
    let dir = work();
    let store = KeyStore::new(dir.path().join("state"));
    let first = store.generate(1000).expect("generates");
    let err = store.generate(1000).expect_err("refuses");
    assert!(matches!(err, KeyError::Exists { .. }), "{err:?}");
    assert!(err.to_string().contains("unopenable"), "{err}");
    assert_eq!(store.registered(1000).expect("reads"), first);
}

#[test]
fn two_accounts_get_two_keys() {
    let dir = work();
    let store = KeyStore::new(dir.path().join("state"));
    let a = store.generate(1000).expect("generates");
    let b = store.generate(1001).expect("generates");
    assert_ne!(a, b);
    assert_eq!(store.registered(1000).expect("reads"), a);
    assert_eq!(store.registered(1001).expect("reads"), b);
}

#[test]
fn an_account_with_no_key_is_absent_and_says_how_to_make_one() {
    let dir = work();
    let store = KeyStore::new(dir.path().join("state"));
    let err = store.registered(1000).expect_err("nothing there");
    assert!(matches!(err, KeyError::Absent { .. }), "{err:?}");
    assert_eq!(err.verdict("the key").as_str(), "absent");
    assert!(err.to_string().contains("apex backup key init"), "{err}");
}

/// The rule, applied to the one file whose unreadability is the NORMAL case:
/// an ordinary user cannot read the private half, and that is by design.
#[test]
fn a_key_that_cannot_be_read_is_could_not_run_and_never_a_key_that_is_not_there() {
    use std::os::unix::fs::PermissionsExt;
    if unsafe { libc::geteuid() } == 0 {
        eprintln!(
            "SKIP  a_key_that_cannot_be_read_is_could_not_run_and_never_a_key_that_is_not_there: \
             running as root, which mode bits do not stop. NOT ASSERTED: that an \
             unreadable private key produces could-not-run rather than absent."
        );
        return;
    }
    let dir = work();
    let store = KeyStore::new(dir.path().join("state"));
    store.generate(1000).expect("generates");
    let path = store.identity_path(1000);
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).expect("chmod");
    let got = store.identity(1000);
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).expect("chmod");

    let err = got.map(|_| ()).expect_err("refused");
    assert!(matches!(err, KeyError::Denied { .. }), "{err:?}");
    assert_eq!(
        err.verdict("the private key").as_str(),
        "could-not-run",
        "an unreadable key was reported as {err:?}, which sends an operator \
         looking for a key they are holding"
    );
    assert!(err.to_string().contains("refusal and not an absence"), "{err}");
}

#[test]
fn a_key_file_of_the_wrong_length_is_not_used_as_a_key() {
    let dir = work();
    let store = KeyStore::new(dir.path().join("state"));
    store.generate(1000).expect("generates");
    std::fs::write(store.identity_path(1000), b"too short").expect("clobbers");
    let err = store.identity(1000).expect_err("refuses");
    assert!(matches!(err, KeyError::Malformed { .. }), "{err:?}");
    assert_eq!(err.verdict("the key").as_str(), "failed");
}

#[test]
fn a_registered_recipient_that_is_not_a_recipient_is_refused() {
    let dir = work();
    let store = KeyStore::new(dir.path().join("state"));
    store.generate(1000).expect("generates");
    std::fs::write(store.recipient_path(1000), b"apexbk1nonsense").expect("clobbers");
    let err = store.registered(1000).expect_err("refuses");
    assert!(matches!(err, KeyError::Malformed { .. }), "{err:?}");
}

/// The one-line attack this whole two-file arrangement exists to refuse.
#[test]
fn a_recipient_the_project_declares_but_this_machine_never_registered_is_refused() {
    let dir = work();
    let store = KeyStore::new(dir.path().join("state"));
    let mine = store.generate(1000).expect("generates");

    // What an agent that can edit apex.toml would write: its own public key.
    let theirs = crate::crypto::Identity::generate()
        .expect("a key")
        .recipient();

    let err = store
        .check_declared(1000, &theirs.to_string_value())
        .expect_err("refuses");
    assert!(matches!(err, KeyError::NotRegistered { .. }), "{err:?}");
    let text = err.to_string();
    assert!(text.contains(&theirs.to_string_value()), "{text}");
    assert!(text.contains(&mine.to_string_value()), "{text}");
    assert!(
        text.contains("a declaration and never an authority"),
        "the message has to say why a file the project owns is not enough: {text}"
    );

    // And the one this machine really registered is accepted.
    assert_eq!(
        store
            .check_declared(1000, &mine.to_string_value())
            .expect("accepts"),
        mine
    );
}

#[test]
fn a_declared_recipient_that_does_not_parse_is_refused_before_the_comparison() {
    let dir = work();
    let store = KeyStore::new(dir.path().join("state"));
    store.generate(1000).expect("generates");
    let err = store.check_declared(1000, "not-a-recipient").expect_err("refuses");
    assert!(matches!(err, KeyError::BadRecipient(_)), "{err:?}");
}

/// Checking a declaration against a machine that has no key must not silently
/// accept the declaration.
#[test]
fn a_declaration_checked_against_no_registered_key_is_refused() {
    let dir = work();
    let store = KeyStore::new(dir.path().join("state"));
    let orphan = crate::crypto::Identity::generate()
        .expect("a key")
        .recipient();
    let err = store
        .check_declared(1000, &orphan.to_string_value())
        .expect_err("refuses");
    assert!(matches!(err, KeyError::Absent { .. }), "{err:?}");
}
