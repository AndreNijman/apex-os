//! Per-worktree status: tests, conflicts, diff, readiness (§P1-036).
//!
//! An agent worktree is where the work actually is, and until now the runtime
//! could list them and say nothing about them. The four questions a person
//! asks about one — has it got a diff, would it merge, did the tests pass, is
//! it ready to hand over — each needed a different source of truth, and two of
//! them have a hard constraint attached.
//!
//! ## Nothing here may touch the working tree
//!
//! Every one of these worktrees has somebody in it: an agent typing into a
//! session, or Andre. So a status query is not allowed to change what is
//! there. That rules out the two obvious implementations:
//!
//! * **conflicts** are computed with [`git::merge_tree_probe`], never
//!   `git merge --no-commit`. A real merge leaves `MERGE_HEAD` and a
//!   half-merged index in a checkout somebody is working in; `merge-tree`
//!   computes the same result in the object database. The probe is run from
//!   the project root against ref names, so the answer cannot depend on the
//!   worktree's own HEAD, index or dirt. (It does write unreferenced objects
//!   into the shared object store — see that function's note. No working tree,
//!   index, stash or branch is touched, and that is the guarantee, stated
//!   exactly.)
//!
//! * **tests are observed, never run.** Running somebody's suite to answer a
//!   status query would be a side effect with a build directory, a CPU cost
//!   and, for a suite that touches a daemon or a port, a real chance of
//!   breaking the session that is mid-task. So [`TestState`] reports the last
//!   test run APEX *saw go past*, and its honest default is
//!   [`TestState::Unobserved`].
//!
//! ## What "the tests pass" does and does not mean here
//!
//! The observation channel is the hook stream the daemon already receives.
//! `hook::Payload` carries no exit code, so a tool's outcome is knowable only
//! from which event fired — `post_tool_use` against
//! `post_tool_use_failure`. Two limits follow, and neither may be papered
//! over:
//!
//! 1. This is the last run APEX observed, not a fresh verdict. The tree may
//!    have changed since. `TestState` carries `head`, the commit the run was
//!    observed at, so a client can say "and the tree has moved since".
//! 2. Whether the agent upstream really emits a failure event for a non-zero
//!    exit is an upstream behaviour this crate cannot compel. If it never
//!    arrives, the state stays `Running` and then ages out — which reads as
//!    "nobody told us how it ended", not as a pass.
//!
//! So the wording everywhere is "the last test run APEX observed". A field
//! called `tests_pass` would be a claim this evidence cannot support.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::git;
use crate::project::{self, AgentWorktree, Project};

/// Unix seconds. Inline rather than shared because that is what every other
/// module in this crate does (`project.rs:94`, `checkpoint.rs:185`).
fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Files and lines changed, as a wire type.
///
/// A mirror of [`git::DiffStat`] rather than a serde derive on it, so `git.rs`
/// stays a plain wrapper over the binary with no wire-format opinions.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffSummary {
    pub files: u32,
    pub insertions: u32,
    pub deletions: u32,
}

impl From<git::DiffStat> for DiffSummary {
    fn from(s: git::DiffStat) -> Self {
        DiffSummary {
            files: s.files,
            insertions: s.insertions,
            deletions: s.deletions,
        }
    }
}

/// Whether this worktree would merge back cleanly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ConflictState {
    /// It would merge cleanly.
    Clean,
    /// It would conflict, in these paths.
    Conflicted { paths: Vec<String> },
    /// Git declined to answer, and the reason is carried rather than guessed.
    ///
    /// A branch with no commits, a base that was deleted, unrelated
    /// histories. Reported as unknown so that one odd worktree cannot fail a
    /// whole listing — and never collapsed into `Clean`, which would be the
    /// one wrong answer that costs somebody a broken merge.
    Unknown { reason: String },
    /// Not a question for this tree: the project's own main working tree has
    /// nothing to merge into itself.
    NotApplicable,
}

impl ConflictState {
    /// The paths, for a client that wants to list them.
    pub fn paths(&self) -> &[String] {
        match self {
            ConflictState::Conflicted { paths } => paths,
            _ => &[],
        }
    }

    pub fn is_conflicted(&self) -> bool {
        matches!(self, ConflictState::Conflicted { .. })
    }
}

/// The last test run APEX observed in a worktree.
///
/// Deliberately not a boolean. "Never saw one", "one is running", "the last
/// one failed" are three different things to a person deciding whether to hand
/// work over, and a boolean would have to lie about two of them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum TestState {
    /// No test run has been seen in this worktree. The default, and not a
    /// failure — most worktrees have simply not had a suite run in them since
    /// the daemon started watching.
    Unobserved,
    /// A test run started and nothing has reported its end.
    ///
    /// Also where a run whose completion event never arrived stays. That is
    /// the intended reading: "nobody told us how it ended" is not a pass.
    Running {
        command: String,
        started: u64,
        /// The commit the worktree was on when the run started.
        head: Option<String>,
    },
    /// The last observed run completed and the agent reported no failure.
    Passed {
        command: String,
        finished: u64,
        head: Option<String>,
    },
    /// The last observed run reported a failure.
    Failed {
        command: String,
        finished: u64,
        head: Option<String>,
    },
}

impl TestState {
    /// The command, for the three states that have one.
    pub fn command(&self) -> Option<&str> {
        match self {
            TestState::Unobserved => None,
            TestState::Running { command, .. }
            | TestState::Passed { command, .. }
            | TestState::Failed { command, .. } => Some(command),
        }
    }

    /// When this was observed, for whoever has to evict the oldest.
    pub fn when(&self) -> u64 {
        match self {
            TestState::Unobserved => 0,
            TestState::Running { started, .. } => *started,
            TestState::Passed { finished, .. } | TestState::Failed { finished, .. } => *finished,
        }
    }

    /// One word for a table.
    pub fn as_str(&self) -> &'static str {
        match self {
            TestState::Unobserved => "unobserved",
            TestState::Running { .. } => "running",
            TestState::Passed { .. } => "passed",
            TestState::Failed { .. } => "failed",
        }
    }
}

/// Whether the work in a worktree is ready to be handed to a human.
///
/// `ready_to_propose`, NOT `pr_status`, and the name is load-bearing: nothing
/// here asks GitHub anything. It is a local judgement from local facts, and a
/// field called `pr_status` would be read as a live query against a forge —
/// which this cannot do and must not imply.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Readiness {
    pub ready_to_propose: bool,
    /// Why not, in the order a person would fix them. Empty when ready.
    pub blockers: Vec<String>,
}

/// Everything the runtime can say about one worktree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorktreeStatus {
    pub name: String,
    pub path: String,
    pub branch: Option<String>,
    pub head: Option<String>,
    /// False for the project's own main working tree.
    pub is_agent: bool,
    /// The branch this one is measured against.
    ///
    /// The main tree's current branch, read AT QUERY TIME.
    /// `project::ensure_worktree` bases a new worktree on whatever the main
    /// tree was on and records that nowhere, so there is no stored base to
    /// recover — this is an observation now, not a memory of the branch point.
    pub base: Option<String>,
    /// Uncommitted changes, staged or not, including untracked files.
    pub dirty: bool,
    /// The committed delta against `base`.
    pub diff: DiffSummary,
    /// Commits this worktree has that `base` does not.
    pub ahead: Option<u32>,
    /// Commits `base` has that this worktree does not.
    pub behind: Option<u32>,
    /// Its tracking branch, when it has one.
    pub upstream: Option<String>,
    /// Commits not yet pushed to `upstream`.
    pub unpushed: Option<u32>,
    pub conflicts: ConflictState,
    pub tests: TestState,
    /// Sessions the daemon has working in this worktree.
    ///
    /// Ids only, on purpose. What a session in turn spawned is `graph.rs`'s
    /// record (§P1-020) and modelling the hierarchy a second time here would
    /// give the Agent Center two answers to the same question. This join —
    /// worktree to session — is the part a client running git itself could not
    /// work out.
    pub sessions: Vec<u32>,
    pub ready: Readiness,
}

/// Decide readiness from the facts already gathered.
///
/// Split out from [`status`] so it can be tested without a repository: this is
/// the function whose answer a person acts on, and it should not require a
/// filesystem to check.
///
/// `Unobserved` tests do NOT block. Most worktrees have never had a suite run
/// in them, and a rule that called all of those unready would make the field
/// mean "has anybody run tests lately" instead of "is this work ready". A
/// FAILED run does block: that is evidence, not absence.
pub fn readiness(
    dirty: bool,
    ahead: Option<u32>,
    conflicts: &ConflictState,
    tests: &TestState,
    upstream: Option<&str>,
    unpushed: Option<u32>,
) -> Readiness {
    let mut blockers = Vec::new();

    if dirty {
        blockers.push("uncommitted changes in the worktree".to_string());
    }
    match ahead {
        Some(0) => blockers.push("no commits of its own yet".to_string()),
        None => blockers.push("cannot tell what it is ahead of".to_string()),
        Some(_) => {}
    }
    match conflicts {
        ConflictState::Conflicted { paths } => blockers.push(format!(
            "would conflict in {} file{}",
            paths.len(),
            if paths.len() == 1 { "" } else { "s" }
        )),
        ConflictState::Unknown { reason } => {
            blockers.push(format!("conflict state unknown: {reason}"))
        }
        ConflictState::Clean | ConflictState::NotApplicable => {}
    }
    if let TestState::Failed { command, .. } = tests {
        blockers.push(format!("the last test run APEX observed failed: {command}"));
    }
    if upstream.is_none() {
        blockers.push("never pushed anywhere".to_string());
    } else if let Some(n) = unpushed {
        if n > 0 {
            blockers.push(format!(
                "{n} commit{} not pushed",
                if n == 1 { "" } else { "s" }
            ));
        }
    }

    Readiness {
        ready_to_propose: blockers.is_empty(),
        blockers,
    }
}

/// Status for one worktree.
///
/// `repo` is the project's MAIN working tree, and every git question that
/// could be answered from a ref is asked there rather than in `wt.path` — the
/// point being that a worktree somebody is typing in is never the thing
/// interrogated.
pub fn status(
    repo: &Path,
    wt: &AgentWorktree,
    base: Option<&str>,
    tests: TestState,
    sessions: Vec<u32>,
) -> WorktreeStatus {
    let branch = wt.branch.clone();
    let head = git::head_commit(&wt.path);

    // Dirt is the one question that HAS to be asked in the worktree itself:
    // it is about that checkout's own uncommitted state, which no ref knows.
    // Read-only — `git status --porcelain` writes nothing.
    let dirty = git::is_dirty(&wt.path);

    let (mut diff, mut ahead, mut behind) = (DiffSummary::default(), None, None);
    let mut conflicts = if wt.is_agent {
        ConflictState::Unknown {
            reason: "no base branch to compare against".to_string(),
        }
    } else {
        ConflictState::NotApplicable
    };

    if let (Some(base), Some(branch)) = (base, branch.as_deref()) {
        if let Some(s) = git::diff_stat(repo, base, branch) {
            diff = s.into();
        }
        if let Some((a, b)) = git::ahead_behind(repo, base, branch) {
            ahead = Some(a);
            behind = Some(b);
        }
        // The main tree has nothing to merge into itself, and a branch
        // compared against itself is trivially clean — reporting that as a
        // real answer would be noise.
        if wt.is_agent && base != branch {
            conflicts = match git::merge_tree_probe(repo, base, branch) {
                git::MergeProbe::Clean => ConflictState::Clean,
                git::MergeProbe::Conflicted(paths) => ConflictState::Conflicted { paths },
                git::MergeProbe::Unknown(reason) => ConflictState::Unknown { reason },
            };
        } else if !wt.is_agent {
            conflicts = ConflictState::NotApplicable;
        }
    }

    let upstream = branch.as_deref().and_then(|b| git::upstream_of(repo, b));
    let unpushed = match (upstream.as_deref(), branch.as_deref()) {
        (Some(up), Some(b)) => git::ahead_behind(repo, up, b).map(|(a, _)| a),
        _ => None,
    };

    let ready = readiness(
        dirty,
        ahead,
        &conflicts,
        &tests,
        upstream.as_deref(),
        unpushed,
    );

    WorktreeStatus {
        name: wt.name.clone(),
        path: wt.path.to_string_lossy().into_owned(),
        branch,
        head,
        is_agent: wt.is_agent,
        base: base.map(|b| b.to_string()),
        dirty,
        diff,
        ahead,
        behind,
        upstream,
        unpushed,
        conflicts,
        tests,
        sessions,
        ready,
    }
}

/// Status for every worktree of `project`, main tree first.
///
/// `tests_for` and `sessions_for` are supplied by the caller — the daemon,
/// which is the only thing that holds those records — keyed by worktree path.
/// Passed in rather than read here so this function needs no daemon state and
/// can be tested against a bare repository.
pub fn statuses<F, G>(
    project: &Project,
    mut tests_for: F,
    mut sessions_for: G,
) -> anyhow::Result<Vec<WorktreeStatus>>
where
    F: FnMut(&Path) -> TestState,
    G: FnMut(&Path) -> Vec<u32>,
{
    let repo = Path::new(&project.root);
    let base = git::current_branch(repo);
    let worktrees = project::worktrees(project)?;
    Ok(worktrees
        .iter()
        .map(|wt| {
            let tests = tests_for(&wt.path);
            let sessions = sessions_for(&wt.path);
            status(repo, wt, base.as_deref(), tests, sessions)
        })
        .collect())
}

/// Whether a shell command is a test run, and the runner it names.
///
/// Used by the hook bridge to notice a suite going past. Deliberately a short
/// table of known runners rather than "anything containing the word test":
/// `cargo build --tests`, `git commit -m "fix the test"` and
/// `vim tests/foo.rs` are not test runs, and a status field that flipped on
/// any of them would be worse than an empty one.
///
/// Command chains are scanned segment by segment, because the real thing a
/// harness runs is `cd apexd && cargo test --locked`.
pub fn test_command(command: &str) -> Option<String> {
    for segment in command.split(&['&', '|', ';', '\n'][..]) {
        if let Some(found) = test_segment(segment) {
            return Some(found);
        }
    }
    None
}

/// The known runners, as leading token sequences.
///
/// **Order matters: a longer sequence MUST come before any prefix of itself.**
/// `npm run test` sits before `npm test` because the words are matched
/// contiguously from the start of the arguments and the first entry that
/// matches wins — with the short one first, `npm run test` reported itself as
/// `npm test`. A test pins this.
const RUNNERS: &[&[&str]] = &[
    &["cargo", "nextest", "run"],
    &["cargo", "nextest"],
    &["cargo", "test"],
    &["npm", "run", "test"],
    &["npm", "test"],
    &["pnpm", "run", "test"],
    &["pnpm", "test"],
    &["yarn", "test"],
    &["bun", "test"],
    &["pytest"],
    &["py.test"],
    &["tox"],
    &["go", "test"],
    &["make", "test"],
    &["make", "check"],
    &["ctest"],
    &["meson", "test"],
    &["ninja", "test"],
    &["gradle", "test"],
    &["mvn", "test"],
    &["dotnet", "test"],
    &["rspec"],
    &["jest"],
    &["vitest"],
    &["phpunit"],
    &["mix", "test"],
];

/// Interpreters that stand in front of the thing actually being run.
///
/// `bash tests/test-agent-inject.sh` names `bash` as its program, and without
/// this the suite that matters would be invisible.
const INTERPRETERS: &[&str] = &[
    "bash", "sh", "zsh", "dash", "python", "python3", "python2", "perl", "ruby", "node",
];

fn test_segment(segment: &str) -> Option<String> {
    let tokens: Vec<&str> = segment.split_whitespace().collect();
    // Skip the noise a real command line carries in front of the program:
    // `sudo`, `time`, `env`, and `FOO=bar` assignments.
    let mut i = 0;
    while i < tokens.len() {
        let t = tokens[i];
        if matches!(t, "sudo" | "time" | "env" | "command" | "exec" | "nice") || t.contains('=') {
            i += 1;
        } else {
            break;
        }
    }
    let mut rest = &tokens[i..];
    if rest.is_empty() {
        return None;
    }

    // The program may be a path: `/usr/bin/cargo`, `./node_modules/.bin/jest`.
    let basename = |t: &str| t.rsplit('/').next().unwrap_or(t).to_string();
    let mut program = basename(rest[0]);

    // Step over an interpreter to the script it is running, skipping the
    // interpreter's own flags (`python -m pytest` is handled by the runner
    // table below, so only a script argument advances us).
    if INTERPRETERS.contains(&program.as_str()) {
        if let Some(pos) = rest[1..].iter().position(|w| !w.starts_with('-')) {
            let script = basename(rest[1 + pos]);
            // `python -m pytest` leaves the runner table to match `pytest`;
            // anything else is the script itself.
            rest = &rest[1 + pos..];
            program = script;
        }
    }

    for runner in RUNNERS {
        if program != runner[0] {
            continue;
        }
        // The runner's remaining words must appear CONTIGUOUSLY from the start
        // of the arguments. So `cargo test --locked` matches, `npm run test`
        // matches only the three-word entry, and `cargo build --tests` matches
        // nothing.
        let args = &rest[1..];
        let want = &runner[1..];
        if args.len() >= want.len() && args[..want.len()] == *want {
            return Some(runner.join(" "));
        }
    }

    // A project's own suite, run directly: `./tests/test-agent-inject.sh`.
    // Recognised because that is exactly how this repository runs its own
    // integration suites, and they are the runs most worth reporting.
    //
    // The naming rule is deliberately narrow. A `run-` prefix was tried and
    // dropped: `tests/run-clippy.sh` is a LINT gate, and reporting a clippy
    // run as this worktree's test status would be a false green (or a false
    // red) on a field somebody decides a handover from.
    let stem = program
        .strip_suffix(".sh")
        .or_else(|| program.strip_suffix(".py"))
        .or_else(|| program.strip_suffix(".bats"));
    if let Some(stem) = stem {
        if stem.starts_with("test-")
            || stem.starts_with("test_")
            || stem.starts_with("check-")
            || stem.ends_with("-test")
            || stem.ends_with("_test")
            || stem.ends_with("-tests")
        {
            return Some(program.to_string());
        }
    }

    None
}

/// A test observation, as the hook bridge reports it.
///
/// Carried separately from [`TestState`] because the bridge knows two of the
/// three things — what ran and whether it failed — and the daemon supplies the
/// third, the time it recorded it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TestPhase {
    Started,
    Passed,
    Failed,
}

impl TestPhase {
    pub fn as_str(&self) -> &'static str {
        match self {
            TestPhase::Started => "started",
            TestPhase::Passed => "passed",
            TestPhase::Failed => "failed",
        }
    }

    pub fn parse(s: &str) -> Option<TestPhase> {
        match s {
            "started" => Some(TestPhase::Started),
            "passed" => Some(TestPhase::Passed),
            "failed" => Some(TestPhase::Failed),
            _ => None,
        }
    }
}

/// A test run the hook bridge noticed, on its way to the daemon.
///
/// Two fields and no timestamp: the bridge runs inside the session and the
/// daemon is the thing that records when it heard, so a time sent from here
/// would be a second clock to disagree with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TestNote {
    pub phase: TestPhase,
    /// The runner, as [`test_command`] named it — not the whole command line.
    /// A command line can carry a heredoc, a secret in an environment
    /// assignment, or a megabyte of arguments, and none of that belongs in a
    /// status field that gets rendered in a table.
    pub command: String,
}

/// Fold an observation into a state, at `head`.
pub fn observe(phase: TestPhase, command: &str, head: Option<String>) -> TestState {
    let command = command.to_string();
    match phase {
        TestPhase::Started => TestState::Running {
            command,
            started: now(),
            head,
        },
        TestPhase::Passed => TestState::Passed {
            command,
            finished: now(),
            head,
        },
        TestPhase::Failed => TestState::Failed {
            command,
            finished: now(),
            head,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_runners_are_recognised() {
        assert_eq!(test_command("cargo test").as_deref(), Some("cargo test"));
        assert_eq!(
            test_command("cargo test --locked --workspace").as_deref(),
            Some("cargo test")
        );
        assert_eq!(test_command("pytest -q").as_deref(), Some("pytest"));
        assert_eq!(test_command("go test ./...").as_deref(), Some("go test"));
        assert_eq!(test_command("npm test").as_deref(), Some("npm test"));
        assert_eq!(
            test_command("npm run test -- --watch=false").as_deref(),
            Some("npm run test")
        );
        assert_eq!(test_command("make check").as_deref(), Some("make check"));
    }

    #[test]
    fn a_command_that_merely_mentions_tests_is_not_a_test_run() {
        // The whole reason the runner table is a table. Each of these would
        // flip a "tests" field that matched on the word "test", and each is
        // something an agent does constantly.
        for command in [
            "cargo build --tests",
            "git commit -m 'fix the test'",
            "vim tests/foo.rs",
            "grep -rn test src/",
            "ls tests",
            "cargo fmt",
            "rm -rf target/debug/test-artifacts",
            "cargo clippy --all-targets",
        ] {
            assert_eq!(test_command(command), None, "{command}");
        }
    }

    #[test]
    fn a_runner_inside_a_chain_is_found() {
        // What a harness actually runs.
        assert_eq!(
            test_command("cd apexd && cargo test --locked").as_deref(),
            Some("cargo test")
        );
        assert_eq!(
            test_command("set -e; pytest -x").as_deref(),
            Some("pytest")
        );
    }

    #[test]
    fn env_prefixes_and_absolute_paths_do_not_hide_the_runner() {
        assert_eq!(
            test_command("RUST_LOG=debug cargo test").as_deref(),
            Some("cargo test")
        );
        assert_eq!(
            test_command("/usr/bin/cargo test").as_deref(),
            Some("cargo test")
        );
        assert_eq!(
            test_command("time cargo nextest run").as_deref(),
            Some("cargo nextest run")
        );
    }

    #[test]
    fn this_repositorys_own_suites_are_recognised() {
        assert_eq!(
            test_command("./tests/test-agent-inject.sh").as_deref(),
            Some("test-agent-inject.sh")
        );
        // An interpreter in front must not hide the script.
        assert_eq!(
            test_command("bash tests/run-labwc-matrix-test.sh").as_deref(),
            Some("run-labwc-matrix-test.sh")
        );
        assert_eq!(
            test_command("./tests/check-screenshot-dispatch.sh").as_deref(),
            Some("check-screenshot-dispatch.sh")
        );

        // ...but not any old script, and specifically NOT the lint gate.
        // `tests/run-clippy.sh` runs clippy, and reporting a clippy pass as
        // this worktree's TEST status would be a false green on a field
        // somebody decides a handover from. This assertion is the reason the
        // broad `run-` prefix was dropped.
        assert_eq!(test_command("bash tests/run-clippy.sh"), None);
        assert_eq!(test_command("./scripts/deploy.sh"), None);
    }

    #[test]
    fn an_unobserved_suite_does_not_make_a_worktree_unready() {
        // Most worktrees have never had a suite run in them. If absence
        // blocked, this field would mean "has anybody run tests lately".
        let r = readiness(
            false,
            Some(3),
            &ConflictState::Clean,
            &TestState::Unobserved,
            Some("origin/task/x"),
            Some(0),
        );
        assert!(r.ready_to_propose, "blockers: {:?}", r.blockers);
        assert!(r.blockers.is_empty());
    }

    #[test]
    fn a_failed_suite_does_block() {
        let r = readiness(
            false,
            Some(3),
            &ConflictState::Clean,
            &TestState::Failed {
                command: "cargo test".to_string(),
                finished: 1,
                head: None,
            },
            Some("origin/task/x"),
            Some(0),
        );
        assert!(!r.ready_to_propose);
        assert!(
            r.blockers.iter().any(|b| b.contains("cargo test")),
            "{:?}",
            r.blockers
        );
    }

    #[test]
    fn an_unknown_conflict_state_is_never_treated_as_clean() {
        // The one wrong answer that costs somebody a broken merge.
        let r = readiness(
            false,
            Some(1),
            &ConflictState::Unknown {
                reason: "refusing to merge unrelated histories".to_string(),
            },
            &TestState::Unobserved,
            Some("origin/x"),
            Some(0),
        );
        assert!(!r.ready_to_propose);
        assert!(
            r.blockers.iter().any(|b| b.contains("unrelated histories")),
            "the reason travels with the refusal: {:?}",
            r.blockers
        );
    }

    #[test]
    fn every_blocker_is_reported_not_just_the_first() {
        // A person fixing one and finding two more would stop trusting it.
        let r = readiness(
            true,
            Some(0),
            &ConflictState::Conflicted {
                paths: vec!["a.rs".to_string(), "b.rs".to_string()],
            },
            &TestState::Failed {
                command: "pytest".to_string(),
                finished: 1,
                head: None,
            },
            None,
            None,
        );
        assert!(!r.ready_to_propose);
        assert_eq!(r.blockers.len(), 5, "{:?}", r.blockers);
        assert!(r.blockers.iter().any(|b| b.contains("2 files")));
    }

    #[test]
    fn one_conflicted_file_is_not_pluralised() {
        let r = readiness(
            false,
            Some(1),
            &ConflictState::Conflicted {
                paths: vec!["a.rs".to_string()],
            },
            &TestState::Unobserved,
            Some("origin/x"),
            Some(0),
        );
        assert!(r.blockers.iter().any(|b| b == "would conflict in 1 file"));
    }

    #[test]
    fn unpushed_commits_block_and_say_how_many() {
        let r = readiness(
            false,
            Some(2),
            &ConflictState::Clean,
            &TestState::Unobserved,
            Some("origin/x"),
            Some(2),
        );
        assert!(!r.ready_to_propose);
        assert!(r.blockers.iter().any(|b| b == "2 commits not pushed"));
    }

    #[test]
    fn test_state_words_are_stable() {
        // These reach a shell script and a QML table.
        assert_eq!(TestState::Unobserved.as_str(), "unobserved");
        assert_eq!(
            TestState::Running {
                command: "c".into(),
                started: 0,
                head: None
            }
            .as_str(),
            "running"
        );
        assert_eq!(TestPhase::Failed.as_str(), "failed");
        assert_eq!(TestPhase::parse("passed"), Some(TestPhase::Passed));
        assert_eq!(TestPhase::parse("nonsense"), None);
    }

    #[test]
    fn conflict_state_round_trips_as_json() {
        // It crosses the protocol, so the tagged shape is part of the wire.
        let c = ConflictState::Conflicted {
            paths: vec!["src/main.rs".to_string()],
        };
        let json = serde_json::to_string(&c).unwrap();
        assert!(json.contains("\"state\":\"conflicted\""), "{json}");
        let back: ConflictState = serde_json::from_str(&json).unwrap();
        assert_eq!(back, c);

        let u = TestState::Unobserved;
        let json = serde_json::to_string(&u).unwrap();
        assert_eq!(json, "{\"state\":\"unobserved\"}");
    }
}
