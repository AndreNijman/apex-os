//! The Android client and this crate agree about the `projects` and `profiles`
//! replies.
//!
//! ## The defect this is shaped to catch
//!
//! Both verbs were added because two rounds of Android work recorded that they
//! did not exist and designed around the absence. The record was right — they
//! did not — but the *method* that produced it was reading source and
//! remembering, and that same method had already been wrong once about
//! `worktrees`, which had been on the wire since `473b7f60`.
//!
//! So neither half of either verb is asserted by reading. The fixtures at
//! `android/core/src/test/resources/{projects,profiles}-reply.json` are parsed
//! by Kotlin tests, which proves the phone can read *those files* and nothing
//! about whether they are what `apex-agentd` sends. This half closes that: the
//! same files must
//!
//! * **deserialize** into [`Response`] — a field the phone reads that this
//!   crate does not send fails here; and
//! * **round-trip** — re-serialized, they are the same JSON value — so a field
//!   this crate sends that the fixture omits fails too. Compared as
//!   `serde_json::Value` rather than as text, because key order and whitespace
//!   are not the contract.
//!
//! The fixtures live under `android/` because a Kotlin test can only load
//! resources from its own source set, and a copy in two places is a copy that
//! drifts. This is `android_worktrees_wire.rs`'s pattern, pointed at the two
//! verbs it did not cover.

use apex_agent_core::protocol::Response;

fn fixture(name: &str) -> String {
    // `CARGO_MANIFEST_DIR` is `apexd/apex-agent-core`; the repository root is
    // two levels up. Resolved rather than hardcoded so the test works from any
    // worktree, which on this project is the normal case.
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("android/core/src/test/resources")
        .join(name);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("the shared fixture is missing at {path:?}: {e}"))
}

/// Deserialize, re-serialize, and assert the value is unchanged.
fn round_trips(name: &str) -> Response {
    let text = fixture(name);
    let parsed: Response = serde_json::from_str(&text).unwrap_or_else(|e| {
        panic!("{name} is not a Response this crate accepts: {e}")
    });
    let back = serde_json::to_value(&parsed).expect("re-serialise");
    let original: serde_json::Value = serde_json::from_str(&text).expect("the fixture is JSON");
    assert_eq!(
        back, original,
        "{name} is not what this crate would have sent; the fixture and the type have drifted"
    );
    parsed
}

#[test]
fn the_projects_fixture_is_a_reply_this_crate_could_have_sent() {
    let Response::Projects { projects } = round_trips("projects-reply.json") else {
        panic!("the fixture is tagged as something other than a projects reply");
    };

    assert_eq!(projects.len(), 3, "the fixture lost a row");

    // A bound capsule and an unbound one both appear. §8's capsule is the
    // binding a client renders as the project's workspace, and a fixture
    // carrying only bound projects would not exercise the null — which is the
    // value every project written before capsules existed has.
    assert!(
        projects.iter().any(|p| p.capsule.as_deref() == Some("rust")),
        "no project with a capsule binding"
    );
    assert!(
        projects.iter().any(|p| p.capsule.is_none()),
        "no project without one"
    );

    // An empty language list is a real answer — a repository with no marker
    // file this runtime knows — and is not the same as a missing field.
    assert!(
        projects.iter().any(|p| p.languages.is_empty()),
        "no project with no detected toolchain"
    );

    // `root` is the field a client joins to `SessionInfo::project`, which is
    // the project root verbatim. Asserted absolute, because a relative one
    // would join to nothing and the failure would show as a project with no
    // sessions rather than as an error.
    for p in &projects {
        assert!(p.root.starts_with('/'), "{} has a relative root", p.name);
        assert!(!p.slug.is_empty(), "{} has no slug", p.name);
    }
}

#[test]
fn the_profiles_fixture_is_a_reply_this_crate_could_have_sent() {
    let Response::Profiles { profiles } = round_trips("profiles-reply.json") else {
        panic!("the fixture is tagged as something other than a profiles reply");
    };

    assert_eq!(profiles.len(), 3, "the fixture lost a row");

    // Every row names an adapter this runtime actually has. A fixture that
    // drifted onto an id the daemon cannot launch would still parse, and would
    // have the phone offer a button that could only fail — which is precisely
    // the class of defect a device found on the Start screen.
    for p in &profiles {
        assert!(
            apex_agent_core::adapter::by_id(&p.agent).is_some(),
            "the fixture names `{}`, which is not an adapter",
            p.agent
        );
    }

    // The three shapes a client has to render differently, each present once:
    // an adapter with a described profile installed here; one whose profile
    // this runtime cannot describe; and the one that needs a command.
    let claude = profiles.iter().find(|p| p.agent == "claude").unwrap();
    assert!(claude.described && claude.installed);
    assert_eq!(claude.root.as_deref(), Some("~/.claude"));
    assert!(!claude.command_required);

    let generic = profiles.iter().find(|p| p.agent == "generic").unwrap();
    assert!(generic.command_required, "generic must ask for a program");
    assert_eq!(generic.program, None);

    assert!(
        profiles.iter().any(|p| !p.described && !p.command_required),
        "no row for an adapter whose profile this runtime does not describe"
    );

    // The root is home-relative wherever it is stated, so a reply cannot be
    // used to learn the user's name. The unit test beside `summarise_adapters`
    // asserts the producer; this asserts the fixture the phone parses agrees.
    for p in &profiles {
        if let Some(root) = &p.root {
            assert!(
                root.starts_with("~/"),
                "{} states an absolute profile root: {root}",
                p.agent
            );
        }
    }
}

#[test]
fn the_fixtures_carry_no_profile_content() {
    // The audit restated at the fixture, because a fixture is what a future
    // round copies when it extends the reply. Counts and compiled-in strings
    // only: no model, no plugin, no marketplace, no MCP server, no skill, no
    // credential.
    let text = fixture("profiles-reply.json");
    for leak in ["settings.json", "credentials", "mcpServers", "plugins", "SKILL"] {
        assert!(
            !text.contains(leak),
            "the profiles fixture carries {leak:?}, which is content and not a count"
        );
    }
}
