//! Project checkpoints — the Agent Time Machine.
//!
//! Before a substantial agent task the runtime captures enough state to put the
//! project back. This is deliberately more than `git reset`:
//!
//! * **tracked and untracked files** are both captured, as a real git tree, so
//!   a file the agent created and then broke can be restored alongside a file
//!   it edited;
//! * **`HEAD` and the branch** are recorded, so undo also unwinds commits the
//!   agent made;
//! * **the user's package set** (`/var/lib/apex/pkg/requested`) is recorded, so
//!   packages installed for the task are reported on undo.
//!
//! ## What it does not touch
//!
//! Ignored files are not captured. `.gitignore` exists precisely to name build
//! output and local secrets, and a checkpoint that swept `.env` and a 4 GB
//! `target/` into a git object would be both a security problem and unusably
//! slow. This is a documented boundary, not an oversight.
//!
//! Packages are recorded but never removed automatically. Removing one is a
//! privileged, system-wide operation and undoing a project's working tree is
//! not a good enough reason to run it without asking; [`PackageDelta`] reports
//! exactly what to run instead.
//!
//! ## Why it cannot disturb the user's work
//!
//! Capture runs entirely through plumbing against a temporary index
//! (`GIT_INDEX_FILE`), so the user's staged changes, their stash and their
//! branch are untouched. Restore takes its own safety checkpoint first, so undo
//! is itself undoable.

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::git;
use crate::paths;

/// Ref namespace for checkpoint commits. Under `refs/apex/` rather than
/// `refs/heads/` so checkpoints never appear as branches, are never pushed by
/// a default `git push`, and cannot collide with the user's own refs.
pub const REF_PREFIX: &str = "refs/apex/checkpoints";

/// The package set the engine builds from.
const PKG_REQUESTED: &str = "/var/lib/apex/pkg/requested";

/// Identity used for checkpoint commits.
///
/// Fixed rather than inherited: `git commit-tree` fails outright when the user
/// has no `user.email` configured, and a checkpoint must not depend on the
/// user's git configuration being complete.
const AUTHOR_NAME: &str = "APEX Agent Runtime";
const AUTHOR_EMAIL: &str = "agent-runtime@apex-os.localhost";

/// A captured project state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Checkpoint {
    /// Sortable identifier, `<unix-milliseconds>-<short-commit>`.
    pub id: String,
    pub label: String,
    /// Absolute project root.
    pub project: String,
    /// The commit holding the captured tree.
    pub commit: String,
    /// `HEAD` at capture time, absent in a repository with no commits.
    pub head: Option<String>,
    /// Branch at capture time, absent when detached.
    pub branch: Option<String>,
    /// Unix seconds, for display.
    pub created: u64,
    /// Unix milliseconds, and the ordering key.
    ///
    /// Seconds are not enough to order checkpoints. Two taken in the same
    /// second sorted by id, which tiebreaks on the commit hash — an arbitrary
    /// order, so `apex agent undo` with no argument could pick the older of
    /// the two. Defaulted so a record written before this field existed still
    /// parses and falls back to second precision.
    #[serde(default)]
    pub created_ms: u64,
    /// Session that triggered the capture, when one did.
    pub session: Option<u32>,
    /// The user's requested-package list at capture time.
    pub packages: Vec<String>,
    /// Whether the working tree had uncommitted changes when captured.
    pub dirty: bool,
}

impl Checkpoint {
    /// The git ref holding this checkpoint's tree.
    pub fn git_ref(&self) -> String {
        format!("{REF_PREFIX}/{}", self.id)
    }

    /// The value checkpoints are ordered by, newest largest.
    ///
    /// Falls back to second precision for a record written before `created_ms`
    /// existed, so old and new records still order sensibly against each other.
    pub fn order_key(&self) -> u64 {
        if self.created_ms > 0 {
            self.created_ms
        } else {
            self.created.saturating_mul(1000)
        }
    }

    /// Short form for listings.
    pub fn short_commit(&self) -> &str {
        let n = self.commit.len().min(12);
        &self.commit[..n]
    }
}

/// Packages present now that were not present at checkpoint time, and vice
/// versa.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PackageDelta {
    /// Installed since the checkpoint.
    pub added: Vec<String>,
    /// Removed since the checkpoint.
    pub removed: Vec<String>,
}

impl PackageDelta {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty()
    }

    /// The command that would undo the additions, if there are any.
    pub fn undo_command(&self) -> Option<String> {
        if self.added.is_empty() {
            return None;
        }
        Some(format!("sudo apex remove {}", self.added.join(" ")))
    }
}

/// Compare two package lists.
pub fn package_delta(before: &[String], after: &[String]) -> PackageDelta {
    let before: BTreeSet<&String> = before.iter().collect();
    let after: BTreeSet<&String> = after.iter().collect();
    PackageDelta {
        added: after.difference(&before).map(|s| (*s).clone()).collect(),
        removed: before.difference(&after).map(|s| (*s).clone()).collect(),
    }
}

/// The user's current requested-package list.
///
/// An absent file means the machine has no user packages, which is the common
/// case and must not be an error.
pub fn current_packages() -> Vec<String> {
    read_package_list(Path::new(PKG_REQUESTED))
}

/// Parse a requested-package list. Blank lines and `#` comments are skipped.
pub fn read_package_list(path: &Path) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    parse_package_list(&text)
}

/// Parse the requested-list format: one entry per line.
pub fn parse_package_list(text: &str) -> Vec<String> {
    let mut out: Vec<String> = text
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| l.to_string())
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Unix milliseconds. The only clock read here: `created` is derived from it,
/// so the seconds shown and the milliseconds ordered by can never disagree.
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Where a project's checkpoint metadata lives.
fn meta_dir(project: &Path) -> PathBuf {
    paths::state_dir()
        .join("checkpoints")
        .join(git::slugify(&project.to_string_lossy()))
}

/// Write a git tree for the project's current state, tracked and untracked.
///
/// Uses a temporary index, so the user's staged changes are untouched. This is
/// the same capture `create` performs, exposed on its own because a diff
/// against a checkpoint needs *both* sides built the same way: plain
/// `git diff <commit>` compares only tracked paths, so a file the agent created
/// would not appear in its own diff.
pub fn current_tree(dir: &Path) -> Result<String> {
    let root = git::toplevel(dir)
        .with_context(|| format!("{} is not inside a git repository", dir.display()))?;
    let tmp_index = meta_dir(&root).join(format!("index-diff-{}", std::process::id()));
    paths::ensure_private_dir(
        tmp_index
            .parent()
            .expect("meta_dir always yields a parent for the index file"),
    )?;
    let _ = std::fs::remove_file(&tmp_index);
    let index_env: &OsStr = tmp_index.as_os_str();

    let result = (|| -> Result<String> {
        if git::head_commit(&root).is_some() {
            git::git_env(&root, &["read-tree", "HEAD"], &[("GIT_INDEX_FILE", index_env)])?;
        }
        git::git_env(
            &root,
            &["add", "-A", "--", "."],
            &[("GIT_INDEX_FILE", index_env)],
        )?;
        git::git_env(&root, &["write-tree"], &[("GIT_INDEX_FILE", index_env)])
    })();

    let _ = std::fs::remove_file(&tmp_index);
    result
}

/// Capture the current state of the project containing `dir`.
pub fn create(dir: &Path, label: &str, session: Option<u32>) -> Result<Checkpoint> {
    if !git::available() {
        bail!("git is not installed, so project checkpoints are unavailable");
    }
    let root = git::toplevel(dir)
        .with_context(|| format!("{} is not inside a git repository", dir.display()))?;

    let head = git::head_commit(&root);
    let branch = git::current_branch(&root);
    let dirty = git::is_dirty(&root);

    // A temporary index, so nothing here touches what the user has staged.
    let tmp_index = meta_dir(&root).join(format!("index-{}", std::process::id()));
    paths::ensure_private_dir(
        tmp_index
            .parent()
            .expect("meta_dir always yields a parent for the index file"),
    )?;
    let _ = std::fs::remove_file(&tmp_index);
    let index_env: &OsStr = tmp_index.as_os_str();

    let result = (|| -> Result<Checkpoint> {
        // Seed from HEAD so unchanged tracked files need no re-hashing, then
        // stage everything not ignored. `add -A` from the top level covers
        // modifications, additions and deletions in one pass.
        if head.is_some() {
            git::git_env(&root, &["read-tree", "HEAD"], &[("GIT_INDEX_FILE", index_env)])?;
        }
        git::git_env(
            &root,
            &["add", "-A", "--", "."],
            &[("GIT_INDEX_FILE", index_env)],
        )?;
        let tree = git::git_env(&root, &["write-tree"], &[("GIT_INDEX_FILE", index_env)])?;

        let message = format!(
            "apex checkpoint: {label}\n\nsession: {}\nbranch: {}\n",
            session
                .map(|s| s.to_string())
                .unwrap_or_else(|| "-".to_string()),
            branch.as_deref().unwrap_or("(detached)")
        );

        let mut args: Vec<&str> = vec!["commit-tree", &tree];
        if let Some(h) = head.as_deref() {
            args.push("-p");
            args.push(h);
        }
        args.push("-m");
        args.push(&message);

        let ident: &OsStr = OsStr::new(AUTHOR_NAME);
        let email: &OsStr = OsStr::new(AUTHOR_EMAIL);
        let commit = git::git_env(
            &root,
            &args,
            &[
                ("GIT_INDEX_FILE", index_env),
                ("GIT_AUTHOR_NAME", ident),
                ("GIT_AUTHOR_EMAIL", email),
                ("GIT_COMMITTER_NAME", ident),
                ("GIT_COMMITTER_EMAIL", email),
            ],
        )?;

        let short = &commit[..commit.len().min(8)];

        // Milliseconds, not seconds, and first: the id is fixed-width for the
        // next few centuries, so it sorts chronologically on its own.
        //
        // The commit hash alone is not unique. `git commit-tree` stamps with
        // one-second granularity, so capturing an identical tree twice with the
        // same label and parent inside one second yields the *same* commit —
        // and then the same id, and one metadata file overwriting the other.
        // The millisecond makes that vanishingly unlikely rather than
        // impossible, so claim the id by taking the next free one. A capture
        // takes several git subprocesses and so several milliseconds, which is
        // why this is expected to spin zero times; it exists so the guarantee
        // does not rest on that being true.
        let (id, created_ms) = claim_id(&meta_dir(&root), now_ms(), short);
        let created = created_ms / 1000;

        let cp = Checkpoint {
            id,
            label: label.to_string(),
            project: root.to_string_lossy().into_owned(),
            commit: commit.clone(),
            head,
            branch,
            created,
            created_ms,
            session,
            packages: current_packages(),
            dirty,
        };

        // The ref keeps the commit reachable so `git gc` cannot collect it.
        git::update_ref(&root, &cp.git_ref(), &commit)?;
        write_meta(&cp)?;
        Ok(cp)
    })();

    let _ = std::fs::remove_file(&tmp_index);
    result
}

/// Pick an id no existing checkpoint has taken, and the stamp that goes with it.
///
/// The commit hash alone is not unique. `git commit-tree` stamps with
/// one-second granularity, so capturing an identical tree twice with the same
/// label and parent inside one second yields the *same commit* — and then, with
/// only the commit in the id, the same id, and one metadata file silently
/// overwriting the other. The millisecond makes that unlikely; stepping to the
/// next free stamp makes it impossible.
///
/// A capture runs several git subprocesses and so takes several milliseconds,
/// which is why this is expected to step zero times in practice. It exists so
/// the guarantee does not rest on that remaining true.
///
/// Stepping forward rather than back keeps the ordering invariant: a later
/// capture always gets a strictly larger key than an earlier one.
fn claim_id(meta: &Path, mut created_ms: u64, short: &str) -> (String, u64) {
    loop {
        let id = format!("{created_ms}-{short}");
        if !meta.join(format!("{id}.json")).exists() {
            return (id, created_ms);
        }
        created_ms += 1;
    }
}

fn write_meta(cp: &Checkpoint) -> Result<()> {
    let dir = meta_dir(Path::new(&cp.project));
    paths::ensure_private_dir(&dir)?;
    let path = dir.join(format!("{}.json", cp.id));
    let text = serde_json::to_string_pretty(cp)?;
    // Write-then-rename, so a crash mid-write cannot leave a half-parsed record
    // that makes the whole list unreadable.
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

/// Every checkpoint recorded for the project containing `dir`, newest first.
///
/// A checkpoint whose git ref has gone (a re-cloned repository, a manual
/// `update-ref -d`) is skipped: the metadata without the tree cannot restore
/// anything, and listing it would offer the user an undo that fails.
pub fn list(dir: &Path) -> Result<Vec<Checkpoint>> {
    let Some(root) = git::toplevel(dir) else {
        return Ok(Vec::new());
    };
    let dir = meta_dir(&root);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Ok(Vec::new());
    };

    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(cp) = serde_json::from_str::<Checkpoint>(&text) else {
            continue;
        };
        if git::ref_exists(&root, &cp.git_ref()) {
            out.push(cp);
        }
    }
    // Millisecond precision first, then the id (which itself begins with the
    // millisecond stamp) so the order is total and deterministic.
    out.sort_by(|a, b| {
        b.order_key()
            .cmp(&a.order_key())
            .then_with(|| b.id.cmp(&a.id))
    });
    Ok(out)
}

/// Find a checkpoint by exact id, or by unique id prefix.
pub fn find(dir: &Path, id: &str) -> Result<Checkpoint> {
    let all = list(dir)?;
    if let Some(exact) = all.iter().find(|c| c.id == id) {
        return Ok(exact.clone());
    }
    let matches: Vec<&Checkpoint> = all.iter().filter(|c| c.id.starts_with(id)).collect();
    match matches.len() {
        0 => bail!("no checkpoint {id} for this project"),
        1 => Ok(matches[0].clone()),
        n => bail!("{id} matches {n} checkpoints; use the full id"),
    }
}

/// The most recent checkpoint, if any.
pub fn latest(dir: &Path) -> Result<Option<Checkpoint>> {
    Ok(list(dir)?.into_iter().next())
}

/// What a restore did.
#[derive(Debug, Clone)]
pub struct RestoreReport {
    pub restored: Checkpoint,
    /// The safety checkpoint taken immediately before restoring.
    pub safety: Checkpoint,
    pub packages: PackageDelta,
    /// True when `HEAD` was moved back as well as the working tree.
    pub head_moved: bool,
    /// Files created after the checkpoint and deleted by the restore.
    pub removed: Vec<String>,
}

/// Put the project back to `cp`.
///
/// Order matters: the safety checkpoint is taken *first*, so a user who undoes
/// the wrong thing can undo the undo.
pub fn restore(dir: &Path, cp: &Checkpoint) -> Result<RestoreReport> {
    let root = git::toplevel(dir)
        .with_context(|| format!("{} is not inside a git repository", dir.display()))?;
    if root.to_string_lossy() != cp.project {
        bail!(
            "checkpoint {} belongs to {}, not {}",
            cp.id,
            cp.project,
            root.display()
        );
    }
    if !git::ref_exists(&root, &cp.git_ref()) {
        bail!("checkpoint {} no longer has a git object", cp.id);
    }

    let safety = create(&root, &format!("before undo of {}", cp.id), cp.session)?;

    // Files that exist now but not in the checkpoint, computed before anything
    // is touched.
    //
    // `read-tree -u` only removes paths that were in the *index*, so a file the
    // agent created and never staged would survive the undo — which is exactly
    // the file most likely to be unwanted. They are recoverable from the safety
    // checkpoint taken above.
    let removed = added_since(&root, &cp.commit).unwrap_or_default();

    // `--reset` discards local changes; `-u` updates the working tree and
    // removes files the checkpoint tree does not have. Together they make the
    // tracked side match the checkpoint exactly.
    git::git(&root, &["read-tree", "-u", "--reset", &cp.commit])?;

    for rel in &removed {
        let path = root.join(rel);
        if path.is_file() || path.is_symlink() {
            let _ = std::fs::remove_file(&path);
        }
        // Prune directories the removal emptied, stopping at the project root
        // and at the first directory that still has something in it.
        let mut parent = path.parent().map(|p| p.to_path_buf());
        while let Some(dir) = parent {
            if dir == root || !dir.starts_with(&root) {
                break;
            }
            if std::fs::remove_dir(&dir).is_err() {
                break;
            }
            parent = dir.parent().map(|p| p.to_path_buf());
        }
    }

    // The tree above staged everything, including files that were untracked
    // when captured. Resetting the index back to the recorded HEAD leaves the
    // working tree alone and makes those files show as untracked again, which
    // is the state the user actually had.
    let head_moved = match cp.head.as_deref() {
        Some(head) => {
            git::git(&root, &["reset", "--quiet", "--mixed", head])?;
            true
        }
        None => false,
    };

    let packages = package_delta(&cp.packages, &current_packages());

    Ok(RestoreReport {
        restored: cp.clone(),
        safety,
        packages,
        head_moved,
        removed,
    })
}

/// Paths present in the working tree (tracked or not) but absent from `base`.
fn added_since(root: &Path, base: &str) -> Result<Vec<String>> {
    let now = current_tree(root)?;
    let out = git::git(
        root,
        &["diff", "--name-only", "--diff-filter=A", base, &now],
    )?;
    Ok(out
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .map(|l| l.to_string())
        .collect())
}

/// Delete a checkpoint's ref and metadata.
pub fn remove(dir: &Path, cp: &Checkpoint) -> Result<()> {
    let root = git::toplevel(dir).unwrap_or_else(|| PathBuf::from(&cp.project));
    git::delete_ref(&root, &cp.git_ref())?;
    let path = meta_dir(Path::new(&cp.project)).join(format!("{}.json", cp.id));
    let _ = std::fs::remove_file(path);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_lists_are_parsed_sorted_and_deduplicated() {
        let text = "ripgrep\n\n# a comment\nfd-find\nripgrep\n  jq  \n";
        assert_eq!(
            parse_package_list(text),
            vec!["fd-find".to_string(), "jq".to_string(), "ripgrep".to_string()]
        );
    }

    #[test]
    fn an_absent_package_list_is_empty_not_an_error() {
        assert!(read_package_list(Path::new("/nonexistent/apex/requested")).is_empty());
    }

    #[test]
    fn package_delta_reports_both_directions() {
        let before = vec!["a".to_string(), "b".to_string()];
        let after = vec!["b".to_string(), "c".to_string()];
        let d = package_delta(&before, &after);
        assert_eq!(d.added, vec!["c".to_string()]);
        assert_eq!(d.removed, vec!["a".to_string()]);
        assert!(!d.is_empty());
    }

    #[test]
    fn an_unchanged_package_set_yields_an_empty_delta() {
        let pkgs = vec!["a".to_string(), "b".to_string()];
        let d = package_delta(&pkgs, &pkgs);
        assert!(d.is_empty());
        assert_eq!(d.undo_command(), None);
    }

    #[test]
    fn the_undo_command_names_only_the_added_packages() {
        let d = package_delta(&[], &["clang".to_string(), "cmake".to_string()]);
        assert_eq!(
            d.undo_command().as_deref(),
            Some("sudo apex remove clang cmake")
        );
    }

    #[test]
    fn an_id_already_on_disk_is_stepped_over() {
        // The case the clock cannot be forced into: an identical tree captured
        // twice within one millisecond produces the same commit AND the same
        // stamp, so without this the second record overwrites the first.
        let base = std::env::temp_dir().join(format!("apex-claim-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).expect("create");

        let (first, ms1) = claim_id(&base, 1_756_800_000_000, "abc12345");
        assert_eq!(first, "1756800000000-abc12345");
        assert_eq!(ms1, 1_756_800_000_000);

        // Nothing written yet, so the same inputs still yield the same id.
        let (again, _) = claim_id(&base, 1_756_800_000_000, "abc12345");
        assert_eq!(again, first);

        // Now the record exists: the next claim must not reuse it.
        std::fs::write(base.join(format!("{first}.json")), "{}").expect("write");
        let (second, ms2) = claim_id(&base, 1_756_800_000_000, "abc12345");
        assert_ne!(second, first, "the taken id was handed out twice");
        assert!(ms2 > ms1, "the stamp must move forward, not back");
        assert_eq!(second, "1756800000001-abc12345");

        // And it steps as far as it needs to.
        std::fs::write(base.join(format!("{second}.json")), "{}").expect("write");
        let (third, ms3) = claim_id(&base, 1_756_800_000_000, "abc12345");
        assert_eq!(third, "1756800000002-abc12345");
        assert!(ms3 > ms2);

        // A different commit at the same instant is already distinct.
        let (other, _) = claim_id(&base, 1_756_800_000_000, "def67890");
        assert_eq!(other, "1756800000000-def67890");

        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn ordering_separates_checkpoints_taken_in_the_same_second() {
        // The regression CI caught: with second precision both of these had the
        // same `created`, so the sort fell through to comparing commit hashes —
        // an arbitrary order, and `apex agent undo` with no argument follows it.
        let mut older = sample();
        older.created = 1_756_800_000;
        older.created_ms = 1_756_800_000_120;
        let mut newer = sample();
        newer.created = 1_756_800_000;
        newer.created_ms = 1_756_800_000_880;

        assert_eq!(older.created, newer.created, "same second, by construction");
        assert!(newer.order_key() > older.order_key());
    }

    #[test]
    fn a_record_without_millisecond_precision_still_orders() {
        let mut legacy = sample();
        legacy.created = 1_756_800_000;
        legacy.created_ms = 0;
        assert_eq!(legacy.order_key(), 1_756_800_000_000);

        let mut newer = sample();
        newer.created = 1_756_800_001;
        newer.created_ms = 1_756_800_001_000;
        assert!(newer.order_key() > legacy.order_key());
    }

    fn sample() -> Checkpoint {
        Checkpoint {
            id: "1756800000000-abc12345".into(),
            label: "s".into(),
            project: "/p".into(),
            commit: "abc12345def67890".into(),
            head: None,
            branch: None,
            created: 1_756_800_000,
            created_ms: 1_756_800_000_000,
            session: None,
            packages: vec![],
            dirty: false,
        }
    }

    #[test]
    fn checkpoint_refs_live_outside_refs_heads() {
        // A checkpoint that appeared as a branch would be pushed by a default
        // `git push` and would clutter every branch listing.
        assert!(REF_PREFIX.starts_with("refs/apex/"));
        assert!(!REF_PREFIX.starts_with("refs/heads/"));
    }

    #[test]
    fn a_checkpoint_ref_is_built_from_its_id() {
        let cp = Checkpoint {
            id: "1756800000000-abc12345".into(),
            label: "before task".into(),
            project: "/home/tester/p".into(),
            commit: "abc12345def67890".into(),
            head: Some("deadbeef".into()),
            branch: Some("main".into()),
            created: 1_756_800_000,
            created_ms: 1_756_800_000_000,
            session: Some(3),
            packages: vec![],
            dirty: true,
        };
        assert_eq!(cp.git_ref(), "refs/apex/checkpoints/1756800000000-abc12345");
        assert_eq!(cp.short_commit(), "abc12345def6");
    }

    #[test]
    fn a_short_commit_does_not_panic_on_a_short_string() {
        let cp = Checkpoint {
            id: "1000-ab".into(),
            label: String::new(),
            project: "/p".into(),
            commit: "ab".into(),
            head: None,
            branch: None,
            created: 1,
            created_ms: 1_000,
            session: None,
            packages: vec![],
            dirty: false,
        };
        assert_eq!(cp.short_commit(), "ab");
    }
}
