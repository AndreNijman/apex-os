//! Project ↔ capsule binding (§8), against the real record files.
//!
//! The interesting failure here is not a parse error, it is a silent unbind.
//! `project::remember` is called on every `apex agent run` and every layout
//! save, with a project freshly detected from the filesystem — and the
//! filesystem does not know which capsule the user chose. A `remember` that
//! simply replaced the record would drop the binding the first time an agent
//! started, and nothing would report it: the file would still be there, still
//! valid, just missing one field.
//!
//! So these drive the shipped functions over a real `$XDG_STATE_HOME` and read
//! the JSON back, rather than asserting anything about an in-memory struct.

use std::path::PathBuf;
use std::sync::Once;

use apex_agent_core::project::{self, Project};

static INIT: Once = Once::new();

fn init_state_dir() {
    INIT.call_once(|| {
        let dir = scratch_root().join("state");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create state dir");
        std::env::set_var("XDG_STATE_HOME", &dir);
    });
}

fn scratch_root() -> PathBuf {
    PathBuf::from(format!("/tmp/apex-capsule-{}", std::process::id()))
}

/// A project record with a slug unique to the calling test, so the cases can
/// run in any order and in parallel without sharing a file.
///
/// The root directory is created, and that is not housekeeping: `project::list`
/// DELETES the record of any project whose checkout has gone, so a case using
/// a root that does not exist has its record removed the moment another case
/// runs a listing. Found by exactly that failure.
fn project(slug: &str) -> Project {
    init_state_dir();
    let root = scratch_root().join(slug);
    std::fs::create_dir_all(&root).expect("create project root");
    Project {
        root: root.to_string_lossy().into_owned(),
        name: slug.to_string(),
        slug: slug.to_string(),
        languages: vec!["rust".to_string()],
        last_opened: 0,
        capsule: None,
    }
}

fn stored(slug: &str) -> Project {
    project::load(slug).unwrap_or_else(|| panic!("no record for {slug}"))
}

#[test]
fn a_binding_round_trips_through_the_record() {
    let p = project("bind-roundtrip");
    project::bind_capsule(&p, Some("fedora")).expect("bind");
    assert_eq!(stored("bind-roundtrip").capsule.as_deref(), Some("fedora"));
}

#[test]
fn binding_a_project_that_was_never_recorded_records_it() {
    // Binding is often the first thing a user does in a checkout, before any
    // agent has run in it. Accepting the binding and storing nothing would
    // look like it worked.
    let p = project("bind-unrecorded");
    assert!(project::load("bind-unrecorded").is_none());
    project::bind_capsule(&p, Some("python")).expect("bind");
    let back = stored("bind-unrecorded");
    assert_eq!(back.capsule.as_deref(), Some("python"));
    assert_eq!(back.name, "bind-unrecorded");
}

#[test]
fn remember_does_not_wipe_a_binding_it_knows_nothing_about() {
    // The regression this whole file exists for.
    let p = project("bind-survives-remember");
    project::bind_capsule(&p, Some("cuda")).expect("bind");

    // What the daemon does at the start of every session: a project detected
    // from the filesystem, with no capsule field.
    let fresh = project("bind-survives-remember");
    assert_eq!(fresh.capsule, None);
    project::remember(&fresh).expect("remember");

    assert_eq!(
        stored("bind-survives-remember").capsule.as_deref(),
        Some("cuda"),
        "starting an agent unbound the project's capsule"
    );
}

#[test]
fn remember_still_refreshes_everything_else() {
    let p = project("bind-refresh");
    project::bind_capsule(&p, Some("fedora")).expect("bind");

    let mut fresh = project("bind-refresh");
    fresh.languages = vec!["node".to_string(), "python".to_string()];
    project::remember(&fresh).expect("remember");

    let back = stored("bind-refresh");
    assert_eq!(back.languages, vec!["node", "python"]);
    assert_eq!(back.capsule.as_deref(), Some("fedora"));
    assert!(back.last_opened > 0, "recency was not refreshed");
}

#[test]
fn a_deliberate_clear_is_not_undone_by_the_merge() {
    // `remember` keeps an existing binding when the incoming one is None,
    // which is exactly wrong for `apex project env --clear`. If the clear went
    // back through remember, it would read the old value and write it again.
    let p = project("bind-clear");
    project::bind_capsule(&p, Some("rocm")).expect("bind");
    project::bind_capsule(&p, None).expect("clear");
    assert_eq!(stored("bind-clear").capsule, None);
}

#[test]
fn rebinding_replaces_rather_than_accumulates() {
    let p = project("bind-replace");
    project::bind_capsule(&p, Some("fedora")).expect("bind");
    project::bind_capsule(&p, Some("arch")).expect("rebind");
    assert_eq!(stored("bind-replace").capsule.as_deref(), Some("arch"));
}

#[test]
fn an_unusable_capsule_name_is_refused_and_stores_nothing() {
    let p = project("bind-refused");
    assert!(project::bind_capsule(&p, Some("../../etc/passwd")).is_err());
    assert!(
        project::load("bind-refused").is_none(),
        "a refused binding still wrote a record"
    );
}

#[test]
fn a_bound_project_reports_its_capsule_in_the_listing() {
    // `apex project list --json` serialises exactly this, and the shell reads
    // it. A binding that does not survive `list` is invisible in the UI.
    let p = project("bind-listed");
    project::bind_capsule(&p, Some("ubuntu")).expect("bind");
    let listed = project::list();
    let found = listed
        .iter()
        .find(|q| q.slug == "bind-listed")
        .expect("bound project is listed");
    assert_eq!(found.capsule.as_deref(), Some("ubuntu"));
}
