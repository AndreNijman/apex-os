//! Every `clipboard` reply the Android client parses is one this crate sends.
//!
//! ## Why this exists, stated as the gap it closes
//!
//! The request direction has had both halves since `clipboard` landed:
//! `AgentdRequestWireTest` builds `{"cmd":"clipboard"}` and
//! `android_requests_wire.rs` deserializes it into [`Request::Clipboard`].
//! The REPLY direction had neither. `Agentd.readClipboard` was written,
//! documented at length and shipped without a single test on either side, so
//! the one thing nobody had checked was whether the field it reads is the
//! field this crate writes.
//!
//! That is not a hypothetical. `Response::Clipboard` is an **internally
//! tagged** variant, so `text` sits at the top level beside `"reply"` and not
//! inside a nested object — and the sibling parser `readSession` reads a
//! different variant of the same enum the same way for the same reason, with a
//! doc comment saying so. A `readClipboard` that reached one level too deep
//! would return the empty string for every reply, which is indistinguishable
//! from the machine's clipboard being empty. That is the defect this file
//! exists to make impossible: the failure would have been silent, permanent,
//! and reported to the user as "the clipboard is empty".
//!
//! The fixture lives under `android/` rather than here for the same reason the
//! requests and worktrees ones do: a Kotlin test can only load resources from
//! its own source set, and a copy in two places is a copy that drifts.

use apex_agent_core::protocol::Response;

fn fixture() -> serde_json::Map<String, serde_json::Value> {
    // `CARGO_MANIFEST_DIR` is `apexd/apex-agent-core`; the repository root is
    // two levels up. Resolved rather than hardcoded so this works from any
    // worktree, which on this project is the normal case.
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("android/core/src/test/resources/clipboard-replies.json");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("the shared clipboard reply fixture is missing at {path:?}: {e}"));
    serde_json::from_str::<serde_json::Value>(&text)
        .expect("the shared clipboard reply fixture is not JSON")
        .as_object()
        .expect("the shared clipboard reply fixture is not an object")
        .clone()
}

/// The named cases, `_comment` and friends dropped.
fn cases() -> Vec<(String, serde_json::Value)> {
    fixture()
        .into_iter()
        .filter(|(name, _)| !name.starts_with('_'))
        .collect()
}

#[test]
fn the_fixture_was_actually_read_and_has_the_cases_that_matter() {
    // The guard against this whole file passing because the fixture stopped
    // being found or the filter stopped matching. A loop over nothing asserts
    // nothing, and that is the dominant defect family in this repo's gates.
    let names: Vec<String> = cases().into_iter().map(|(n, _)| n).collect();
    assert!(
        names.len() >= 5,
        "the clipboard reply fixture collapsed to {names:?}"
    );
    for required in ["text", "empty", "multiline"] {
        assert!(
            names.iter().any(|n| n == required),
            "the fixture lost its `{required}` case, which is one of the three the \
             Android parser's documented behaviour depends on; have: {names:?}"
        );
    }
}

#[test]
fn every_clipboard_reply_the_phone_parses_is_one_this_crate_could_have_sent() {
    for (name, value) in cases() {
        let parsed: Response = serde_json::from_value(value.clone()).unwrap_or_else(|e| {
            panic!("the Android client parses `{name}` and this crate cannot produce it: {e}\n  {value}")
        });
        assert!(
            matches!(parsed, Response::Clipboard { .. }),
            "`{name}` is tagged as something other than a clipboard reply"
        );

        // Round-tripped as a `Value`, so a field this crate sends that the
        // fixture omits is caught as well as the other way round. Compared as
        // parsed JSON and not as text because key order and whitespace are not
        // the contract, and a comparison that failed on reformatting would be
        // deleted the first time somebody tidied the file.
        let back = serde_json::to_value(&parsed)
            .unwrap_or_else(|e| panic!("`{name}` did not re-serialize: {e}"));
        assert_eq!(
            back, value,
            "`{name}` does not survive a round trip through Response"
        );
    }
}

#[test]
fn the_text_sits_beside_the_tag_and_not_inside_a_nested_object() {
    // The exact shape `Agentd.readClipboard` depends on, asserted from the
    // daemon's side. `Response` is `#[serde(tag = "reply")]`, so the payload
    // is flattened next to the tag. A future refactor to an adjacently- or
    // externally-tagged enum would keep every Rust test above green and break
    // every phone in the field; this is the assertion that would not survive
    // it.
    let (_, value) = cases()
        .into_iter()
        .find(|(n, _)| n == "text")
        .expect("the fixture lost its `text` case");
    let object = value.as_object().expect("a reply is a JSON object");

    assert_eq!(
        object.get("reply").and_then(|v| v.as_str()),
        Some("clipboard"),
        "the tag is not `clipboard`, so the phone's `require(reply, \"clipboard\")` \
         would throw on a reply it is supposed to accept"
    );
    assert!(
        object.get("text").and_then(|v| v.as_str()).is_some(),
        "`text` is not a top-level string; the phone reads it from the top level"
    );
    assert_eq!(
        object.len(),
        2,
        "a clipboard reply grew a field. That is not automatically wrong, but the \
         phone ignores unknown keys and `Response::Clipboard` deliberately carries \
         no byte count beside the content — see its doc comment."
    );
}

#[test]
fn an_empty_clipboard_is_a_real_reply_and_not_an_error_reply() {
    // The distinction the Android parser documents and that nothing else
    // asserts. If this crate ever answered an empty clipboard with an `error`
    // reply instead, the phone would correctly raise it as a failure and the
    // user would be told the machine refused when it simply had nothing.
    let (_, value) = cases()
        .into_iter()
        .find(|(n, _)| n == "empty")
        .expect("the fixture lost its `empty` case");

    let parsed: Response =
        serde_json::from_value(value).expect("an empty clipboard is not a parseable reply");
    let Response::Clipboard { text } = parsed else {
        panic!("an empty clipboard came back as something other than a clipboard reply");
    };
    assert_eq!(text, "", "the empty case is not empty, so it tests nothing");
}

#[test]
fn a_clipboard_reply_never_carries_a_raw_newline_because_the_wire_is_ndjson() {
    // A clipboard holds whatever was last copied, which is routinely a stack
    // trace. The transport is newline-delimited JSON, so a raw newline in the
    // serialized form would split one reply into two frames and desynchronise
    // the connection — the phone's terminal included, not just this feature.
    // serde escapes it; asserted rather than assumed, on the case that
    // actually contains one.
    for (name, value) in cases() {
        let parsed: Response = serde_json::from_value(value).expect("a parseable reply");
        let line = serde_json::to_string(&parsed).expect("a serializable reply");
        assert!(
            !line.contains('\n'),
            "`{name}` serializes to a line containing a raw newline, which NDJSON \
             framing would read as the end of the reply"
        );
    }

    // And the case that makes the loop above meaningful actually holds one.
    let (_, value) = cases()
        .into_iter()
        .find(|(n, _)| n == "multiline")
        .expect("the fixture lost its `multiline` case");
    let Response::Clipboard { text } =
        serde_json::from_value::<Response>(value).expect("a parseable reply")
    else {
        panic!("the multiline case is not a clipboard reply");
    };
    assert!(
        text.contains('\n') && text.contains('"'),
        "the multiline case no longer contains the newline and quote it exists for"
    );
}
