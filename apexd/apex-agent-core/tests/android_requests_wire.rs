//! Every request the Android client sends is one this crate can parse.
//!
//! ## Why this test exists, stated as the defect it would have caught
//!
//! Two separate rounds of Android work were built on the claim that
//! `apex-agent-core` has no `worktrees` verb. One round built the request and
//! its parser, then deleted both; the next was briefed to design around the
//! absence. The claim came from reading `request.rs`'s `Verb` — the
//! *privileged operation* vocabulary, which correctly has no worktrees — and
//! taking it for `protocol.rs`'s `Request`, which is the wire and has had
//! `Worktrees` since `473b7f60`.
//!
//! No test could have caught that, because there was nothing asserting what
//! the phone sends against what this crate accepts. There is now. The fixture
//! at `android/core/src/test/resources/requests.json` is built by `Agentd`'s
//! own builders on the Kotlin side (`AgentdRequestWireTest`) and deserialized
//! into [`Request`] here. A verb the phone sends that does not exist fails to
//! deserialize; a verb that exists and is spelled wrong fails too; and a verb
//! believed not to exist cannot be argued about, because the daemon's serde
//! either takes it or does not.
//!
//! The fixture lives under `android/` for the same reason the worktrees one
//! does: a Kotlin test can only load resources from its own source set, and a
//! copy in two places is a copy that drifts.

use apex_agent_core::protocol::Request;

fn fixture() -> serde_json::Map<String, serde_json::Value> {
    // `CARGO_MANIFEST_DIR` is `apexd/apex-agent-core`; the repository root is
    // two levels up. Resolved rather than hardcoded so this works from any
    // worktree, which on this project is the normal case.
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("android/core/src/test/resources/requests.json");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("the shared request fixture is missing at {path:?}: {e}"));
    serde_json::from_str::<serde_json::Value>(&text)
        .expect("the shared request fixture is not JSON")
        .as_object()
        .expect("the shared request fixture is not an object")
        .clone()
}

/// Parse one named request, or fail saying which.
fn parse(name: &str) -> Request {
    let f = fixture();
    let value = f
        .get(name)
        .unwrap_or_else(|| panic!("the shared fixture has no request called `{name}`"));
    serde_json::from_value(value.clone()).unwrap_or_else(|e| {
        panic!("the Android client sends `{name}` and this crate cannot parse it: {e}\n  {value}")
    })
}

#[test]
fn every_request_the_phone_builds_deserializes() {
    let f = fixture();
    for (name, value) in f.iter() {
        if name.starts_with('_') {
            continue;
        }
        let parsed: Result<Request, _> = serde_json::from_value(value.clone());
        assert!(
            parsed.is_ok(),
            "the Android client sends `{name}` and this crate cannot parse it: {:?}\n  {value}",
            parsed.err()
        );
    }
}

#[test]
fn the_worktrees_verb_exists_in_both_forms() {
    // The claim this whole file answers. Both spellings: the flat listing, and
    // the narrow one keyed on a project SLUG — which was unreachable from any
    // client until `WorktreeStatus.slug` was added, because nothing in the
    // reply was a slug.
    match parse("worktrees_all") {
        Request::Worktrees { project } => assert_eq!(project, None),
        other => panic!("`worktrees` parsed as {other:?}"),
    }
    match parse("worktrees_one") {
        Request::Worktrees { project } => assert_eq!(project.as_deref(), Some("apex")),
        other => panic!("`worktrees` with a project parsed as {other:?}"),
    }
}

#[test]
fn input_is_a_verb_and_carries_the_terminator_the_daemon_does_not_add() {
    // `session::write_input` writes raw bytes and appends nothing, so a reply
    // built without a terminator would sit on the agent's input line
    // unsubmitted — which on a phone looks exactly like nothing happening.
    // CR, not LF: a terminal delivers `\r` for the return key.
    match parse("input") {
        Request::Input { id, data } => {
            assert_eq!(id, 7);
            assert_eq!(data, "yes\r", "the reply must arrive submitted");
        }
        other => panic!("`input` parsed as {other:?}"),
    }
}


#[test]
fn receive_is_a_verb_and_the_caller_chooses_no_part_of_the_path() {
    // The verb the note in this fixture used to say did not exist, with the
    // arithmetic for why one could not: 65514 bytes per `Frame::Control`,
    // base64's 4/3, so 48 KB against a screenshot's 100 KB to 2 MB. All of it
    // measured ONE frame type. `Open`/`Data`/`Close` is begin/chunk/end and
    // always was; what was missing was a sink, and this is it.
    match parse("receive") {
        Request::Receive { id, name, len } => {
            assert_eq!(id, 7);
            assert_eq!(name, "shot.png");
            // Past what a single control frame could ever have carried, which
            // is the point of the number rather than an arbitrary size.
            assert!(
                len > 65_514,
                "a fixture that fits in one frame would not exercise the thing that was \
                 said to be impossible"
            );
        }
        other => panic!("`receive` parsed as {other:?}"),
    }

    // The name crosses the wire as the phone was given it, and `safe_name` is
    // what makes it a name. Asserted HERE, against the daemon's own reducer,
    // so the guarantee is the one the daemon actually applies rather than the
    // one the phone believes it applies.
    match parse("receive_hostile_name") {
        Request::Receive { name, .. } => {
            assert_eq!(name, "../../.ssh/authorized_keys", "the phone sanitised it itself");
            let safe = apex_agent_core::inject::safe_name(&name).expect("a name");
            assert_eq!(safe, ".._.._.ssh_authorized_keys");
            assert!(!safe.contains('/'), "a name that survived with a separator in it");
        }
        other => panic!("`receive` with a hostile name parsed as {other:?}"),
    }
}

#[test]
fn run_carries_the_checkpoint_flag_and_omits_what_it_has_no_value_for() {
    match parse("run_minimal") {
        Request::Run(r) => {
            assert_eq!(r.cwd, "/home/andre/Projects/apex");
            assert!(r.agent.is_none() && r.prompt.is_none() && r.worktree.is_none());
            assert!(!r.checkpoint, "a request that named no checkpoint asked for one");
        }
        other => panic!("`run` parsed as {other:?}"),
    }
    match parse("run_full") {
        Request::Run(r) => {
            assert_eq!(r.agent.as_deref(), Some("claude"));
            assert_eq!(r.worktree.as_deref(), Some("wt-review"));
            assert!(r.checkpoint, "the checkpoint flag did not survive the wire");
        }
        other => panic!("`run` parsed as {other:?}"),
    }
}

#[test]
fn the_approval_verbs_a_phone_is_allowed() {
    assert!(matches!(parse("requests"), Request::Requests));
    assert!(matches!(parse("grants"), Request::Grants));
    assert!(matches!(parse("system_grants"), Request::SystemGrants));
    assert!(matches!(parse("revoke_system_grant"), Request::RevokeSystemGrant { id: 3 }));

    // An absent `key` means every grant for the project, which is the wide
    // action — so the two forms must not collapse into one.
    match parse("revoke_one") {
        Request::Revoke { key, .. } => assert_eq!(key.as_deref(), Some("pkg-upgrade")),
        other => panic!("`revoke` parsed as {other:?}"),
    }
    match parse("revoke_all") {
        Request::Revoke { key, .. } => assert_eq!(key, None),
        other => panic!("`revoke` with no key parsed as {other:?}"),
    }
}

#[test]
fn the_phone_sends_no_decide_request() {
    // `privilege.rs` refuses `decide` from any non-local origin before it even
    // checks whether the request is pending, so a paired phone can neither
    // approve nor deny. The fixture is where a builder for it would first show
    // up, because the Kotlin half asserts the two sets match.
    let f = fixture();
    for name in f.keys() {
        let request = &f[name];
        let cmd = request.get("cmd").and_then(|c| c.as_str()).unwrap_or("");
        assert_ne!(
            cmd, "decide",
            "`{name}` is a decide request; a phone's decision is refused unconditionally"
        );
    }
}
