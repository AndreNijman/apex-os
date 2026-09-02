//! Checkpoint capture and restore, against real git repositories.
//!
//! These drive the shipped functions rather than a reimplementation of them:
//! every assertion runs `checkpoint::create`, `checkpoint::restore` and
//! `checkpoint::current_tree` over a throwaway repository and then inspects the
//! working tree with `git` itself.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Once;

use apex_agent_core::checkpoint;

static INIT: Once = Once::new();

/// Point the state directory at a scratch path for the whole test binary, so
/// checkpoints never land in the developer's real `~/.local/state`.
fn init_state_dir() {
    INIT.call_once(|| {
        let dir = scratch_root().join("state");
        std::fs::create_dir_all(&dir).expect("create state dir");
        std::env::set_var("XDG_STATE_HOME", &dir);
    });
}

/// Short base path: a Unix socket elsewhere in the runtime caps at 108 bytes,
/// and keeping every test path short avoids surprises.
fn scratch_root() -> PathBuf {
    PathBuf::from(format!("/tmp/apex-cp-{}", std::process::id()))
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("git {args:?}: {e}"));
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim_end().to_string()
}

/// A fresh repository with one commit.
fn repo(name: &str) -> PathBuf {
    init_state_dir();
    let dir = scratch_root().join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create repo dir");

    git(&dir, &["init", "--quiet"]);
    git(&dir, &["config", "user.email", "test@apex-os.localhost"]);
    git(&dir, &["config", "user.name", "APEX Test"]);
    std::fs::write(dir.join("tracked.txt"), "original\n").unwrap();
    git(&dir, &["add", "-A"]);
    git(&dir, &["commit", "--quiet", "-m", "initial"]);
    dir
}

fn write(dir: &Path, rel: &str, contents: &str) {
    let path = dir.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, contents).unwrap();
}

fn read(dir: &Path, rel: &str) -> String {
    std::fs::read_to_string(dir.join(rel)).unwrap_or_default()
}

fn status(dir: &Path) -> String {
    git(dir, &["status", "--porcelain"])
}

#[test]
fn a_checkpoint_captures_tracked_and_untracked_files() {
    let dir = repo("capture");
    write(&dir, "tracked.txt", "edited\n");
    write(&dir, "untracked.txt", "brand new\n");

    let cp = checkpoint::create(&dir, "test capture", Some(7)).expect("create");
    assert_eq!(cp.session, Some(7));
    assert!(cp.dirty, "the working tree had changes");

    // Both files are in the checkpoint tree, which is what makes restore able
    // to bring back a file the agent created.
    let listing = git(&dir, &["ls-tree", "-r", "--name-only", &cp.commit]);
    assert!(listing.contains("tracked.txt"), "{listing}");
    assert!(listing.contains("untracked.txt"), "{listing}");
}

#[test]
fn capturing_does_not_disturb_the_users_index() {
    let dir = repo("index-untouched");
    write(&dir, "staged.txt", "staged content\n");
    git(&dir, &["add", "staged.txt"]);
    let before = status(&dir);

    checkpoint::create(&dir, "should not touch the index", None).expect("create");

    assert_eq!(
        status(&dir),
        before,
        "capture changed what the user had staged"
    );
}

#[test]
fn ignored_files_are_deliberately_not_captured() {
    let dir = repo("ignored");
    write(&dir, ".gitignore", "secret.env\nbuild/\n");
    write(&dir, "secret.env", "TOKEN=hunter2\n");
    write(&dir, "build/artifact.bin", "huge\n");

    let cp = checkpoint::create(&dir, "ignores", None).expect("create");
    let listing = git(&dir, &["ls-tree", "-r", "--name-only", &cp.commit]);

    assert!(
        !listing.contains("secret.env"),
        "an ignored secret was swept into a git object: {listing}"
    );
    assert!(!listing.contains("build/artifact.bin"), "{listing}");
    assert!(listing.contains(".gitignore"), "{listing}");
}

#[test]
fn restore_puts_back_edits_and_deletes_files_created_since() {
    let dir = repo("restore");
    let cp = checkpoint::create(&dir, "before the agent", None).expect("create");

    // Stand in for the agent's work.
    write(&dir, "tracked.txt", "the agent rewrote this\n");
    write(&dir, "added.txt", "the agent created this\n");
    write(&dir, "deep/nested/file.txt", "and this\n");

    let report = checkpoint::restore(&dir, &cp).expect("restore");

    assert_eq!(read(&dir, "tracked.txt"), "original\n", "edit not reverted");
    assert!(
        !dir.join("added.txt").exists(),
        "a file the agent created survived the undo"
    );
    assert!(
        !dir.join("deep/nested/file.txt").exists(),
        "a nested file the agent created survived the undo"
    );
    assert!(
        !dir.join("deep").exists(),
        "the emptied directory was left behind"
    );
    assert_eq!(status(&dir), "", "working tree not clean after restore");

    let mut removed = report.removed.clone();
    removed.sort();
    assert_eq!(removed, vec!["added.txt", "deep/nested/file.txt"]);
}

#[test]
fn restore_takes_a_safety_checkpoint_so_undo_is_itself_undoable() {
    let dir = repo("safety");
    let first = checkpoint::create(&dir, "first", None).expect("create");

    write(&dir, "tracked.txt", "work in progress\n");
    write(&dir, "wip.txt", "unsaved thinking\n");

    let report = checkpoint::restore(&dir, &first).expect("restore");
    assert_eq!(read(&dir, "tracked.txt"), "original\n");
    assert!(!dir.join("wip.txt").exists());

    // Undoing the undo brings the discarded work back.
    checkpoint::restore(&dir, &report.safety).expect("restore the safety checkpoint");
    assert_eq!(read(&dir, "tracked.txt"), "work in progress\n");
    assert_eq!(read(&dir, "wip.txt"), "unsaved thinking\n");
}

#[test]
fn restore_unwinds_commits_the_agent_made() {
    let dir = repo("commits");
    let head_before = git(&dir, &["rev-parse", "HEAD"]);
    let cp = checkpoint::create(&dir, "before commits", None).expect("create");

    write(&dir, "tracked.txt", "committed by the agent\n");
    git(&dir, &["add", "-A"]);
    git(&dir, &["commit", "--quiet", "-m", "agent commit"]);
    assert_ne!(git(&dir, &["rev-parse", "HEAD"]), head_before);

    let report = checkpoint::restore(&dir, &cp).expect("restore");
    assert!(report.head_moved);
    assert_eq!(
        git(&dir, &["rev-parse", "HEAD"]),
        head_before,
        "HEAD was not moved back, so the agent's commit survived"
    );
    assert_eq!(read(&dir, "tracked.txt"), "original\n");
}

#[test]
fn a_previously_untracked_file_comes_back_untracked() {
    let dir = repo("untracked-round-trip");
    write(&dir, "notes.txt", "my notes\n");
    let cp = checkpoint::create(&dir, "with an untracked file", None).expect("create");

    std::fs::remove_file(dir.join("notes.txt")).unwrap();
    checkpoint::restore(&dir, &cp).expect("restore");

    assert_eq!(read(&dir, "notes.txt"), "my notes\n", "file not restored");
    assert_eq!(
        status(&dir),
        "?? notes.txt",
        "the file came back staged instead of untracked"
    );
}

#[test]
fn checkpoints_are_listed_newest_first_and_found_by_prefix() {
    let dir = repo("listing");
    let first = checkpoint::create(&dir, "one", None).expect("create");
    write(&dir, "tracked.txt", "changed\n");
    let second = checkpoint::create(&dir, "two", None).expect("create");

    let list = checkpoint::list(&dir).expect("list");
    assert!(list.len() >= 2);
    assert_eq!(list[0].id, second.id, "newest checkpoint is not first");

    let found = checkpoint::find(&dir, &first.id).expect("find by full id");
    assert_eq!(found.commit, first.commit);

    assert_eq!(
        checkpoint::latest(&dir).expect("latest").map(|c| c.id),
        Some(second.id)
    );
}

#[test]
fn a_checkpoint_from_another_project_is_refused() {
    let a = repo("project-a");
    let b = repo("project-b");
    let cp = checkpoint::create(&a, "from a", None).expect("create");

    let err = checkpoint::restore(&b, &cp).expect_err("restoring across projects must fail");
    let text = format!("{err:#}");
    assert!(text.contains("belongs to"), "{text}");
}

#[test]
fn capture_works_without_any_user_git_identity() {
    // `git commit-tree` fails outright when user.email is unset, so a
    // checkpoint must supply its own identity rather than depend on the user
    // having configured git.
    let dir = repo("no-identity");
    git(&dir, &["config", "--unset", "user.email"]);
    git(&dir, &["config", "--unset", "user.name"]);

    let cp = checkpoint::create(&dir, "no identity configured", None)
        .expect("capture must not need the user's git identity");
    let author = git(&dir, &["show", "-s", "--format=%an <%ae>", &cp.commit]);
    assert!(author.contains("APEX"), "{author}");
}

#[test]
fn a_non_repository_is_a_clear_error_not_a_panic() {
    init_state_dir();
    let dir = scratch_root().join("not-a-repo");
    std::fs::create_dir_all(&dir).unwrap();
    let err = checkpoint::create(&dir, "nope", None).expect_err("must fail");
    assert!(format!("{err:#}").contains("git repository"));
}

#[test]
fn current_tree_reflects_untracked_work() {
    let dir = repo("current-tree");
    let cp = checkpoint::create(&dir, "base", None).expect("create");
    write(&dir, "fresh.txt", "created after the checkpoint\n");

    let now = checkpoint::current_tree(&dir).expect("current tree");
    // This is what makes `apex agent diff` show a file the agent created;
    // `git diff <commit>` alone would not.
    let diff = git(
        &dir,
        &["diff", "--name-only", "--diff-filter=A", &cp.commit, &now],
    );
    assert!(diff.contains("fresh.txt"), "{diff}");
}
