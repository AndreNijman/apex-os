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
        Some(slug) => match project::load(slug) {
            Some(p) => vec![p],
            None => {
                return Response::error(
                    ErrorKind::BadRequest,
                    format!(
                        "no remembered project with slug {slug:?}; \
                         `apex project list` shows the ones there are"
                    ),
                )
            }
        },
        None => project::list(),
    };

    let mut out = Vec::new();
    for proj in &projects {
        // One unreadable project must not fail the whole listing — a checkout
        // on an unmounted disk is the ordinary case, not an error worth
        // refusing every other project's status over.
        let statuses = worktree::statuses(
            proj,
            |path| observations.get(path),
            |path| sessions_in(sessions, path),
        );
        match statuses {
            Ok(mut s) => out.append(&mut s),
            Err(_) => continue,
        }
    }
    Response::Worktrees { worktrees: out }
}

/// The ids of sessions working in `path`.
///
/// Matched on the session's recorded `cwd`, so a session started in a
/// subdirectory of the worktree still counts — which is the common case, since
/// `apex agent run --worktree` puts the agent at the tree root but a person
/// attaching later may be anywhere under it.
///
/// Exited sessions are included when they are still in the registry: "session
/// 12 worked here" is the fact somebody reading a conflict wants, and hiding
/// it would make the busiest worktree on the machine look untouched.
fn sessions_in(sessions: &[SessionWhere], path: &Path) -> Vec<u32> {
    let mut ids: Vec<u32> = sessions
        .iter()
        .filter(|s| Path::new(&s.cwd) == path || Path::new(&s.cwd).starts_with(path))
        .map(|s| s.id)
        .collect();
    ids.sort_unstable();
    ids
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

    fn info(id: u32, cwd: &str) -> SessionWhere {
        SessionWhere {
            id,
            cwd: cwd.to_string(),
        }
    }

    #[test]
    fn a_session_below_the_worktree_still_counts_as_in_it() {
        let sessions = vec![
            info(1, "/w/tree"),
            info(2, "/w/tree/apexd/apex-agent-core"),
            info(3, "/w/other"),
        ];
        assert_eq!(sessions_in(&sessions, Path::new("/w/tree")), vec![1, 2]);
    }

    #[test]
    fn a_sibling_with_a_shared_prefix_is_not_inside() {
        // The bug a string `starts_with` would have: `/w/tree-2` starts with
        // the TEXT `/w/tree`, and reporting its sessions against the wrong
        // worktree would put an agent in a tree it has never been in. Path
        // comparison is by component, which is why this passes.
        let sessions = vec![info(1, "/w/tree-2/src"), info(2, "/w/tree")];
        assert_eq!(sessions_in(&sessions, Path::new("/w/tree")), vec![2]);
    }

    #[test]
    fn ids_come_back_sorted() {
        let sessions = vec![info(9, "/w/t"), info(2, "/w/t"), info(5, "/w/t")];
        assert_eq!(sessions_in(&sessions, Path::new("/w/t")), vec![2, 5, 9]);
    }
}
