//! Agent worktrees made before the rename to Rime OS, against real git.
//!
//! Such a tree is checked out under `.apex/worktrees/<name>` and git's own
//! records point there. The runtime has to keep finding it by name, reuse it,
//! and list it as an agent worktree; a new one goes under `.rime/worktrees`.

use std::path::{Path, PathBuf};
use std::process::Command;

use rime_agent_core::project::{self, Project, LEGACY_WORKTREE_DIR, WORKTREE_DIR};

fn scratch_root() -> PathBuf {
    PathBuf::from(format!("/tmp/rime-lwt-{}", std::process::id()))
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

fn repo(name: &str) -> PathBuf {
    let dir = scratch_root().join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create repo dir");
    git(&dir, &["init", "--quiet"]);
    git(&dir, &["config", "user.email", "test@example.invalid"]);
    git(&dir, &["config", "user.name", "Test"]);
    std::fs::write(dir.join("tracked.txt"), "original\n").unwrap();
    git(&dir, &["add", "-A"]);
    git(&dir, &["commit", "--quiet", "-m", "initial"]);
    dir
}

fn project_at(root: &Path) -> Project {
    Project {
        root: root.to_string_lossy().into_owned(),
        name: "demo".into(),
        slug: "demo".into(),
        languages: vec![],
        last_opened: 0,
        capsule: None,
    }
}

#[test]
fn a_worktree_made_before_the_rename_is_reused_and_listed() {
    let root = repo("reuse");
    // What an APEX machine left behind: the tree and its branch, made by the
    // runtime before the rename.
    let old = root.join(LEGACY_WORKTREE_DIR).join("issue-217");
    git(
        &root,
        &["worktree", "add", "--quiet", "-b", "agent/issue-217", &old.to_string_lossy()],
    );
    let p = project_at(&root);

    // Re-running `--worktree issue-217` reattaches instead of failing on a
    // branch git already has checked out elsewhere.
    let got = project::ensure_worktree(&p, "issue-217").expect("reuses the old tree");
    assert_eq!(got, old);

    // A new name goes under the new directory.
    let new = project::ensure_worktree(&p, "fresh").expect("makes a new tree");
    assert_eq!(new, root.join(WORKTREE_DIR).join("fresh"));

    // Both are listed as the project's agent worktrees, by name.
    let listed = project::worktrees(&p).expect("lists");
    for (name, path) in [("issue-217", &old), ("fresh", &new)] {
        let w = listed
            .iter()
            .find(|w| &w.path == path)
            .unwrap_or_else(|| panic!("{name} is not listed: {listed:?}"));
        assert!(w.is_agent, "{name} is not an agent worktree: {w:?}");
        assert_eq!(w.name, name);
    }

    // And removed by name from where it is.
    project::remove_worktree(&p, "issue-217", true).expect("removes the old tree");
    assert!(!old.exists());

    let _ = std::fs::remove_dir_all(scratch_root());
}
