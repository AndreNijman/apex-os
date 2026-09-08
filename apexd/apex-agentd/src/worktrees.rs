//! Per-worktree status, and the record of test runs seen going past (§P1-036).
//!
//! Two things live here. [`TestObservations`] is the daemon's memory of test
//! runs it watched happen; [`handle`] answers `Request::Worktrees` by joining
//! that memory to what git can say about each worktree.
//!
//! ## Why the observations are keyed by worktree and not by session
//!
//! A test run belongs to the tree it ran in. Sessions come and go — an agent
//! finishes, `apex agent rm` forgets it, the daemon restarts — and the fact
//! that `cargo test` failed in `wt-p1-035` an hour ago is still the last thing
//! anybody knows about that tree. Keyed by session, that fact would vanish
//! with the session that produced it, and a fresh session in the same worktree
//! would report `Unobserved` while the failure it needs to know about sat one
//! record away.
//!
//! It also keeps this out of `SessionInfo`, which is written to disk on every
//! event and is already a rebase hazard with four literal constructors.
//!
//! ## Deliberately not persisted
//!
//! The map is process memory and a daemon restart empties it. That is the
//! honest behaviour: `Unobserved` means "nobody here saw a test run", and
//! after a restart nobody here did. Writing it to disk would let a stale
//! `passed` outlive the daemon, the branch and the commit it referred to, and
//! be read as a fresh verdict.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use apex_agent_core::git;
use apex_agent_core::project;

use apex_agent_core::protocol::{ErrorKind, Response};
use apex_agent_core::worktree::{self, TestNote, TestState};

/// The part of a session this module needs: its id, and where it works.
///
/// Deliberately not `SessionInfo`. That struct is a wire type with four
/// literal constructors across the workspace and a pending rebase that adds
/// fields to all of them; a status join has no business widening that surface.
/// Two fields also make the tests below readable without a session fixture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionWhere {
    pub id: u32,
    pub cwd: String,
}

/// Most worktrees remembered at once.
///
/// A bound rather than a growing map: the daemon runs for weeks, every session
/// contributes a key, and an unbounded cache in a long-lived process is a leak
/// with a slow fuse. Well above any real machine's worktree count — the
/// eviction path exists to be correct, not to be used.
const MAX_TRACKED: usize = 256;

/// What the daemon has seen of test runs, by worktree top level.
#[derive(Default)]
pub struct TestObservations {
    seen: Mutex<HashMap<PathBuf, TestState>>,
}

impl TestObservations {
    pub fn new() -> TestObservations {
        TestObservations::default()
    }

    /// Record what a hook reported, against the worktree `cwd` sits in.
    ///
    /// `cwd` is the SESSION's working directory as the daemon recorded it at
    /// start — not a path from the request. A session cannot nominate a
    /// worktree it does not live in.
    ///
    /// Resolved to the top level so that a run started three directories down
    /// (`cd apexd && cargo test`) lands against the tree, not the
    /// subdirectory. A `cwd` in no repository at all is dropped: there is no
    /// worktree for it to be about.
    pub fn record(&self, cwd: &Path, note: &TestNote) {
        let Some(top) = git::toplevel(cwd) else {
            return;
        };
        let head = git::head_commit(&top);
        let state = worktree::observe(note.phase, &note.command, head);

        let mut seen = match self.seen.lock() {
            Ok(g) => g,
            // A poisoned lock means another thread panicked holding it. A
            // status field is not worth propagating that panic into an event
            // the session is waiting on.
            Err(e) => e.into_inner(),
        };
        if seen.len() >= MAX_TRACKED && !seen.contains_key(&top) {
            if let Some(oldest) = seen
                .iter()
                .min_by_key(|(_, s)| s.when())
                .map(|(k, _)| k.clone())
            {
                seen.remove(&oldest);
            }
        }
        seen.insert(top, state);
    }

    /// What was last seen in `path`, or [`TestState::Unobserved`].
    pub fn get(&self, path: &Path) -> TestState {
        let seen = match self.seen.lock() {
            Ok(g) => g,
            Err(e) => e.into_inner(),
        };
        seen.get(path).cloned().unwrap_or(TestState::Unobserved)
    }
}

/// Answer `Request::Worktrees`.
///
/// `sessions` is a snapshot taken by the caller before any git command runs.
/// Deliberately a snapshot: enumerating worktrees and probing merges takes
/// long enough that holding the registry lock across it would block every
/// other request, including the event a session is waiting on to report its
/// own state.
pub fn handle(
    slug: Option<String>,
    observations: &TestObservations,
    sessions: &[SessionWhere],
) -> Response {
    let projects = match slug.as_deref() {
        // Resolved by SEARCHING the remembered set, not by joining the slug
        // onto the store path. The difference is the whole security argument
        // of this request: answering it makes the daemon run git — including
        // `merge-tree --write-tree`, which writes objects — in the project's
        // `root`, and every confined session has the control socket bound in
        // so that it can publish events. Built by joining, a slug of
        // `../../../var/tmp/mine` would name a record of the caller's own
        // writing and the daemon would run there. Searched, the set of
        // directories this request can reach is exactly the set the user
        // already chose to remember, whatever the caller sends.
        //
        // `project::load` refuses a path-shaped slug too, as of the same
        // change. Two barriers on purpose: that one protects every other
        // caller of the store, this one does not depend on it.
        Some(slug) => match project::list().into_iter().find(|p| p.slug == slug) {
            Some(p) => vec![p],
            None => {
                return Response::error(
                    ErrorKind::BadRequest,
                    format!(
                        "no remembered project with slug {:?}; \
                         `apex project list` shows the ones there are",
                        clip(slug)
                    ),
                )
            }
        },
        None => project::list(),
    };

    // `(id, cwd)` once, for every project. Which worktree each session
    // belongs to is decided inside `worktree::statuses`, which is the only
    // place that has the whole worktree list to compare against.
    let where_they_are: Vec<(u32, PathBuf)> = sessions
        .iter()
        .map(|s| (s.id, PathBuf::from(&s.cwd)))
        .collect();

    let mut out = Vec::new();
    for proj in &projects {
        // A LINKED WORKTREE REMEMBERED AS A PROJECT IS NOT A PROJECT.
        //
        // `project::detect` resolves a project from `git rev-parse
        // --show-toplevel`, and inside a linked worktree that is the worktree
        // itself — so `apex agent run --cwd <a worktree>` records the worktree
        // as a project of its own. Every worktree of the repository is visible
        // from in there, so listing it produces a SECOND copy of every row,
        // all carrying the linked worktree's directory name and all with
        // `is_agent` false because none of them sit under ITS
        // `.apex/worktrees`. Measured, not theorised: the fixture suite
        // produced six rows for a three-worktree repository, three of them
        // named after one worktree.
        //
        // Skipped here rather than fixed in `detect`, which is not this
        // unit's: the record is also what checkpoints and layouts key on, and
        // a status listing has no business changing what a project is. The
        // rows are not lost — they are the real project's, if it is
        // remembered.
        if git::is_linked_worktree(Path::new(&proj.root)) {
            if slug.is_some() {
                return Response::error(
                    ErrorKind::BadRequest,
                    format!(
                        "{:?} is a linked git worktree, not a project — its \
                         worktrees are the repository's, and belong to the \
                         project whose root is the main working tree",
                        clip(&proj.slug)
                    ),
                );
            }
            continue;
        }

        // One unreadable project must not fail the whole listing — a checkout
        // on an unmounted disk is the ordinary case, not an error worth
        // refusing every other project's status over.
        let statuses = worktree::statuses(proj, |path| observations.get(path), &where_they_are);
        match statuses {
            Ok(mut s) => out.append(&mut s),
            Err(_) => continue,
        }
    }
    Response::Worktrees { worktrees: out }
}

/// Trim a caller's string before it goes into an error message.
///
/// The slug is echoed back so that a typo is obvious, and a caller can send a
/// megabyte of it. An error is a log line and a terminal row, not a mirror.
fn clip(text: &str) -> String {
    if text.chars().count() <= 64 {
        return text.to_string();
    }
    let keep: String = text.chars().take(64).collect();
    format!("{keep}…")
}

#[cfg(test)]
mod tests {
    use super::*;
    use apex_agent_core::worktree::TestPhase;

    #[test]
    fn an_unseen_worktree_is_unobserved_not_a_pass() {
        let obs = TestObservations::new();
        assert_eq!(
            obs.get(Path::new("/var/tmp/nowhere")),
            TestState::Unobserved
        );
    }

    #[test]
    fn a_cwd_in_no_repository_records_nothing() {
        // There is no worktree for it to be about, and inventing a key from a
        // bare directory would report a test status for something that is not
        // a checkout.
        let obs = TestObservations::new();
        let note = TestNote {
            phase: TestPhase::Failed,
            command: "cargo test".to_string(),
        };
        obs.record(Path::new("/proc"), &note);
        let seen = obs.seen.lock().unwrap();
        assert!(seen.is_empty(), "recorded {:?}", *seen);
    }

    #[test]
    fn a_long_slug_is_clipped_before_it_reaches_an_error() {
        // The slug is echoed back so a typo is obvious, and a caller can send
        // a megabyte of it. An error is a log line, not a mirror.
        let out = clip(&"a".repeat(500));
        assert_eq!(out.chars().count(), 65, "64 plus the ellipsis");
        assert!(out.ends_with('…'));
        assert_eq!(clip("short"), "short");
    }
}
