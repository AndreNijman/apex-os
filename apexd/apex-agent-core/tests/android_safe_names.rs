//! What the phone SHOWS a file will be called is what this crate will call it.
//!
//! ## Why a second shared fixture
//!
//! `android_requests_wire.rs` settles which verbs exist by making the daemon's
//! own serde parse what the phone builds. This does the same job for the one
//! piece of daemon LOGIC the phone mirrors: `Request::Receive` carries the
//! file's name as the picker gave it, the daemon reduces it with
//! [`apex_agent_core::inject::safe_name`], and the phone shows the user what
//! that reduction will produce before they send anything.
//!
//! A mirror is a second implementation, and a second implementation is a thing
//! that drifts. The only symptom of drift here is quiet and user-facing: the
//! phone names a file, the agent is handed a different one, and nobody notices
//! until somebody goes looking for `resumé.pdf` in an inbox that holds
//! `resum_.pdf`.
//!
//! So the pairs live in one file that both sides read.
//! `android/core/src/test/resources/safe-names.json` is asserted by
//! `HandoffTest` against `Handoff.Files.preview` and by this test against
//! `safe_name` itself. A divergence fails on one side or the other, and it
//! cannot be argued about.
//!
//! Writing the fixture found three of them in a preview that looked obviously
//! right: `..` was repaired to `file..` where the daemon refuses it outright,
//! `é` was guessed as two underscores and is one, and an emoji really is two
//! replacements in a Kotlin loop over `Char` and one in a Rust loop over
//! `chars()`.

use apex_agent_core::inject::{safe_name, NameError};

fn fixture() -> serde_json::Map<String, serde_json::Value> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("android/core/src/test/resources/safe-names.json");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("the shared name fixture is missing at {path:?}: {e}"));
    serde_json::from_str::<serde_json::Value>(&text)
        .expect("the shared name fixture is not JSON")
        .as_object()
        .expect("the shared name fixture is not an object")
        .clone()
}

#[test]
fn every_name_the_phone_previews_is_the_name_this_crate_produces() {
    let f = fixture();
    let mut checked = 0;
    for (raw, expected) in f.iter() {
        if raw.starts_with('_') && raw == "_note" {
            continue;
        }
        checked += 1;
        match (safe_name(raw), expected) {
            (Ok(got), serde_json::Value::String(want)) => assert_eq!(
                &got, want,
                "safe_name({raw:?}) is {got:?}; the phone shows the user {want:?}"
            ),
            (Err(e), serde_json::Value::Null) => {
                // Refused is refused; WHICH refusal is not the phone's
                // business, because it shows one sentence for both.
                assert!(
                    matches!(e, NameError::NoFileName | NameError::ControlByte(_)),
                    "{raw:?} was refused as {e:?}"
                );
            }
            (Ok(got), serde_json::Value::Null) => panic!(
                "the phone tells the user {raw:?} will be refused, and this crate accepts it \
                 as {got:?}"
            ),
            (Err(e), want) => panic!(
                "the phone tells the user {raw:?} becomes {want}, and this crate refuses it: {e}"
            ),
            (_, other) => panic!("{raw:?} maps to {other}, which is neither a name nor null"),
        }
    }
    // A fixture that silently emptied would make this test a green tick over
    // nothing, which is the failure this repository has already shipped.
    assert!(
        checked >= 15,
        "only {checked} names were checked; the fixture has been gutted"
    );
}

#[test]
fn the_reduction_can_never_produce_a_path_or_a_shell_word() {
    // The property behind every pair in the fixture, asserted as a property so
    // a name nobody thought of is covered too. These are the bytes that make
    // the difference between a path typed into a terminal and a command.
    let f = fixture();
    for raw in f.keys() {
        if raw == "_note" {
            continue;
        }
        let Ok(safe) = safe_name(raw) else { continue };
        for bad in ['/', ' ', '\'', '"', '$', '`', ';', '&', '|', '\n', '\r', '\\'] {
            assert!(
                !safe.contains(bad),
                "safe_name({raw:?}) kept {bad:?}, which a terminal acts on"
            );
        }
        assert!(safe.is_ascii(), "safe_name({raw:?}) is not ASCII: {safe:?}");
        assert!(!safe.is_empty(), "safe_name({raw:?}) produced nothing");
        assert!(
            !safe.chars().all(|c| c == '.'),
            "safe_name({raw:?}) is {safe:?}, a traversal spelled as a filename"
        );
    }
}
