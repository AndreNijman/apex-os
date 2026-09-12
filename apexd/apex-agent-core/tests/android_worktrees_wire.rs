//! The Android client and this crate agree about the `worktrees` reply.
//!
//! ## Why this test exists in Rust at all
//!
//! `android/core/src/test/resources/worktrees-reply.json` is parsed by a
//! Kotlin test (`WorktreesTest`) which asserts the phone reads every field.
//! That test passing proves the phone can parse *that file* — it proves
//! nothing about whether the file is what `apex-agentd` actually sends. A
//! field renamed here, or a variant tag changed, would leave the Kotlin suite
//! green and the phone showing an empty worktree list against a real machine.
//!
//! So the same file is the fixture for both halves, and this half asserts it
//! against the daemon's own types:
//!
//! * it **deserializes** into [`Response`], so a field the phone reads that
//!   this crate does not send is caught here;
//! * it **round-trips** — re-serialized, it is the same JSON value — so a
//!   field this crate sends that the fixture omits is caught too. Compared as
//!   `serde_json::Value` rather than as text, because key order and whitespace
//!   are not the contract and a string comparison would fail on reformatting.
//!
//! This is the pattern `AgentStateAgreementTest` already uses for the desktop
//! colours, pointed at the wire instead.
//!
//! The fixture lives under `android/` rather than here because the Kotlin test
//! can only load resources from its own source set, and a copy in two places
//! is a copy that drifts.

use apex_agent_core::protocol::Response;
use apex_agent_core::worktree::{ConflictState, TestState};

/// The fixture, read from the Android module's test resources.
fn fixture() -> String {
    // `CARGO_MANIFEST_DIR` is `apexd/apex-agent-core`; the repository root is
    // two levels up. Resolved rather than hardcoded so the test works from any
    // worktree, which on this project is the normal case.
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("android/core/src/test/resources/worktrees-reply.json");
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("the shared worktrees fixture is missing at {path:?}: {e}"))
}

#[test]
fn the_android_fixture_is_a_reply_this_crate_could_have_sent() {
    let text = fixture();
    let parsed: Response = serde_json::from_str(&text)
        .expect("the fixture the Android suite parses is not a Response this crate accepts");

    let Response::Worktrees { worktrees } = parsed else {
        panic!("the fixture is tagged as something other than a worktrees reply");
    };

    assert_eq!(worktrees.len(), 5, "the fixture lost a row");

    // Every variant of both tagged enums appears, because a fixture that only
    // ever carries the happy one proves nothing about the tags that matter.
    assert!(
        worktrees
            .iter()
            .any(|w| matches!(w.conflicts, ConflictState::Clean)),
        "no clean row"
    );
    assert!(
        worktrees
            .iter()
            .any(|w| matches!(w.conflicts, ConflictState::Conflicted { .. })),
        "no conflicted row"
    );
    assert!(
        worktrees
            .iter()
            .any(|w| matches!(w.conflicts, ConflictState::Unknown { .. })),
        "no unknown-conflict row"
    );
    assert!(
        worktrees
            .iter()
            .any(|w| matches!(w.conflicts, ConflictState::NotApplicable)),
        "no not-applicable row"
    );
    assert!(
        worktrees
            .iter()
            .any(|w| matches!(w.tests, TestState::Unobserved)),
        "no unobserved row"
    );
    assert!(
        worktrees
            .iter()
            .any(|w| matches!(w.tests, TestState::Running { .. })),
        "no running row"
    );
    assert!(
        worktrees
            .iter()
            .any(|w| matches!(w.tests, TestState::Passed { .. })),
        "no passed row"
    );
    assert!(
        worktrees
            .iter()
            .any(|w| matches!(w.tests, TestState::Failed { .. })),
        "no failed row"
    );

    // The join that a client running git itself could not compute.
    let mine = worktrees
        .iter()
        .find(|w| w.name == "wt-p1-053b")
        .expect("the fixture's agent worktree is gone");
    assert_eq!(mine.sessions, vec![4, 11]);
    assert!(mine.ready.ready_to_propose);
    assert!(mine.ready.blockers.is_empty());
}

#[test]
fn every_field_this_crate_sends_is_in_the_fixture() {
    let text = fixture();
    let parsed: Response =
        serde_json::from_str(&text).expect("the fixture is not a Response this crate accepts");

    let round_tripped: serde_json::Value =
        serde_json::to_value(&parsed).expect("a Response this crate built will not serialize");
    let original: serde_json::Value =
        serde_json::from_str(&text).expect("the fixture is not valid JSON");

    // A field ADDED to `WorktreeStatus` and not added to the fixture shows up
    // here as an extra key on the left, and the Android parser would then be
    // reading a shape the daemon no longer sends. A field RENAMED shows up as
    // one key gone and one appeared. Both are the failure this file exists to
    // make loud.
    assert_eq!(
        round_tripped, original,
        "the worktrees fixture the Android suite parses is no longer byte-equivalent to what \
         this crate serializes; update android/core/src/test/resources/worktrees-reply.json \
         AND the Kotlin WorktreesTest that reads it"
    );
}
