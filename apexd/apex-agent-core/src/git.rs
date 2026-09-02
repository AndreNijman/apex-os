//! A thin, explicit wrapper over the `git` binary.
//!
//! Plumbing commands rather than porcelain throughout, because everything here
//! runs against a working tree the user is also using: `git write-tree` and
//! `git commit-tree` build a checkpoint without touching their index, their
//! stash or their branch. A checkpoint that clobbered a staged change would be
//! worse than no checkpoint at all.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{anyhow, bail, Context, Result};

/// Run `git` in `dir` and return trimmed stdout, failing on a non-zero exit.
pub fn git(dir: &Path, args: &[&str]) -> Result<String> {
    git_env(dir, args, &[])
}

/// As [`git`], with extra environment variables.
pub fn git_env(dir: &Path, args: &[&str], env: &[(&str, &OsStr)]) -> Result<String> {
    let mut cmd = Command::new("git");
    cmd.current_dir(dir)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in env {
        cmd.env(k, v);
    }
    let out = cmd
        .output()
        .with_context(|| format!("running git {}", args.join(" ")))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        bail!(
            "git {} failed: {}",
            args.join(" "),
            err.trim().lines().next().unwrap_or("no output")
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim_end().to_string())
}

/// Run `git`, returning `Ok(None)` instead of an error when it exits non-zero.
///
/// For questions with a legitimate "no" answer — does this directory have a
/// HEAD, does this ref exist — where an error would be noise.
pub fn git_opt(dir: &Path, args: &[&str]) -> Option<String> {
    git(dir, args).ok()
}

/// Whether `git` is available at all.
pub fn available() -> bool {
    Command::new("git")
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// The top level of the working tree containing `dir`, if any.
pub fn toplevel(dir: &Path) -> Option<PathBuf> {
    let out = git_opt(dir, &["rev-parse", "--show-toplevel"])?;
    if out.is_empty() {
        return None;
    }
    Some(PathBuf::from(out))
}

/// The commit `HEAD` points at, or `None` in a repository with no commits yet.
pub fn head_commit(dir: &Path) -> Option<String> {
    let out = git_opt(dir, &["rev-parse", "--verify", "HEAD"])?;
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

/// The current branch name, or `None` when detached.
pub fn current_branch(dir: &Path) -> Option<String> {
    let out = git_opt(dir, &["symbolic-ref", "--quiet", "--short", "HEAD"])?;
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

/// Whether the working tree has any modification, staged or not, including
/// untracked files that are not ignored.
pub fn is_dirty(dir: &Path) -> bool {
    match git(dir, &["status", "--porcelain", "--untracked-files=normal"]) {
        Ok(out) => !out.trim().is_empty(),
        // A repository we cannot read is not a repository we should claim is
        // clean.
        Err(_) => true,
    }
}

/// The common git directory, which for a worktree is the main repository's.
pub fn common_dir(dir: &Path) -> Option<PathBuf> {
    let out = git_opt(dir, &["rev-parse", "--path-format=absolute", "--git-common-dir"])?;
    if out.is_empty() {
        None
    } else {
        Some(PathBuf::from(out))
    }
}

/// One registered worktree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Worktree {
    pub path: PathBuf,
    pub branch: Option<String>,
    pub head: Option<String>,
    /// True for the repository's own main working tree.
    pub is_main: bool,
}

/// Every worktree of the repository containing `dir`, main tree first.
pub fn worktrees(dir: &Path) -> Result<Vec<Worktree>> {
    let out = git(dir, &["worktree", "list", "--porcelain"])?;
    Ok(parse_worktree_list(&out))
}

/// Parse the output of `git worktree list --porcelain`.
///
/// Records are separated by blank lines; the first is always the main working
/// tree. Split out from the command so the parser can be tested against real
/// captured output without a repository.
pub fn parse_worktree_list(text: &str) -> Vec<Worktree> {
    let mut out = Vec::new();
    let mut path: Option<PathBuf> = None;
    let mut branch: Option<String> = None;
    let mut head: Option<String> = None;

    let flush = |path: &mut Option<PathBuf>,
                     branch: &mut Option<String>,
                     head: &mut Option<String>,
                     out: &mut Vec<Worktree>| {
        if let Some(p) = path.take() {
            let is_main = out.is_empty();
            out.push(Worktree {
                path: p,
                branch: branch.take(),
                head: head.take(),
                is_main,
            });
        } else {
            *branch = None;
            *head = None;
        }
    };

    for line in text.lines() {
        let line = line.trim_end();
        if line.is_empty() {
            flush(&mut path, &mut branch, &mut head, &mut out);
            continue;
        }
        if let Some(rest) = line.strip_prefix("worktree ") {
            // A new record without a blank line before it.
            flush(&mut path, &mut branch, &mut head, &mut out);
            path = Some(PathBuf::from(rest));
        } else if let Some(rest) = line.strip_prefix("branch ") {
            branch = Some(rest.trim_start_matches("refs/heads/").to_string());
        } else if let Some(rest) = line.strip_prefix("HEAD ") {
            head = Some(rest.to_string());
        }
    }
    flush(&mut path, &mut branch, &mut head, &mut out);
    out
}

/// Whether a ref exists.
pub fn ref_exists(dir: &Path, name: &str) -> bool {
    git(dir, &["rev-parse", "--verify", "--quiet", name]).is_ok()
}

/// Create or move a ref.
pub fn update_ref(dir: &Path, name: &str, commit: &str) -> Result<()> {
    git(dir, &["update-ref", name, commit]).map(|_| ())
}

/// Delete a ref, succeeding if it was already gone.
pub fn delete_ref(dir: &Path, name: &str) -> Result<()> {
    if !ref_exists(dir, name) {
        return Ok(());
    }
    git(dir, &["update-ref", "-d", name]).map(|_| ())
}

/// Every ref under `prefix`, as `(short name, commit)`.
pub fn refs_under(dir: &Path, prefix: &str) -> Result<Vec<(String, String)>> {
    let out = git(
        dir,
        &["for-each-ref", "--format=%(refname) %(objectname)", prefix],
    )?;
    let mut refs = Vec::new();
    for line in out.lines() {
        let mut parts = line.split_whitespace();
        let (Some(name), Some(commit)) = (parts.next(), parts.next()) else {
            continue;
        };
        let short = name.strip_prefix(prefix).unwrap_or(name);
        refs.push((short.trim_start_matches('/').to_string(), commit.to_string()));
    }
    Ok(refs)
}

/// Slugify a name into something safe for a git ref and a directory.
///
/// Git ref names forbid a long list of sequences (`..`, `~`, `^`, `:`, `?`,
/// `*`, `[`, `\`, a trailing `.lock`, a leading or trailing `/`). Rather than
/// enumerate the rules, reduce to a conservative alphabet that satisfies all of
/// them.
pub fn slugify(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut last_dash = false;
    for c in name.chars() {
        let keep = if c.is_ascii_alphanumeric() {
            c.to_ascii_lowercase()
        } else if matches!(c, '-' | '_' | '/') && !out.is_empty() {
            c
        } else {
            '-'
        };
        if keep == '-' {
            if last_dash {
                continue;
            }
            last_dash = true;
        } else {
            last_dash = false;
        }
        out.push(keep);
    }
    let trimmed = out.trim_matches(|c| c == '-' || c == '/').to_string();
    if trimmed.is_empty() {
        "unnamed".to_string()
    } else {
        trimmed
    }
}

/// Add a worktree at `path` on a new branch `branch`, from `base`.
///
/// Reuses the worktree if it already exists and points at the same branch,
/// which is what makes `apex agent run --worktree fix-217` idempotent.
pub fn add_worktree(repo: &Path, path: &Path, branch: &str, base: Option<&str>) -> Result<()> {
    if path.exists() {
        let existing = worktrees(repo)?;
        let matches = existing
            .iter()
            .any(|w| w.path == path && w.branch.as_deref() == Some(branch));
        if matches {
            return Ok(());
        }
        return Err(anyhow!(
            "{} already exists and is not the worktree for branch {branch}",
            path.display()
        ));
    }

    let path_str = path.to_string_lossy().to_string();
    let mut args: Vec<&str> = vec!["worktree", "add"];
    // An existing branch is checked out; a new one is created with -b.
    if ref_exists(repo, &format!("refs/heads/{branch}")) {
        args.push(&path_str);
        args.push(branch);
    } else {
        args.push("-b");
        args.push(branch);
        args.push(&path_str);
        if let Some(base) = base {
            args.push(base);
        }
    }
    git(repo, &args).map(|_| ())
}

/// Remove a worktree and, when `delete_branch`, its branch.
pub fn remove_worktree(repo: &Path, path: &Path, delete_branch: bool) -> Result<()> {
    let path_str = path.to_string_lossy().to_string();
    let branch = worktrees(repo)?
        .into_iter()
        .find(|w| w.path == path)
        .and_then(|w| w.branch);

    git(repo, &["worktree", "remove", "--force", &path_str])?;
    if delete_branch {
        if let Some(b) = branch {
            // A branch that will not delete (unmerged work) is not an error
            // worth aborting on; the worktree is already gone.
            let _ = git(repo, &["branch", "-D", &b]);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worktree_list_is_parsed_main_tree_first() {
        let text = "\
worktree /home/a/repo
HEAD abc123
branch refs/heads/main

worktree /home/a/repo/.worktrees/issue-217
HEAD def456
branch refs/heads/agent/issue-217

worktree /home/a/repo/.worktrees/detached
HEAD 999aaa
detached
";
        let wts = parse_worktree_list(text);
        assert_eq!(wts.len(), 3);

        assert!(wts[0].is_main);
        assert_eq!(wts[0].path, PathBuf::from("/home/a/repo"));
        assert_eq!(wts[0].branch.as_deref(), Some("main"));
        assert_eq!(wts[0].head.as_deref(), Some("abc123"));

        assert!(!wts[1].is_main);
        assert_eq!(wts[1].branch.as_deref(), Some("agent/issue-217"));

        assert!(!wts[2].is_main);
        assert_eq!(wts[2].branch, None, "a detached worktree has no branch");
    }

    #[test]
    fn worktree_list_without_trailing_blank_line_still_yields_the_last_entry() {
        let text = "worktree /a\nHEAD 1\nbranch refs/heads/main\n\nworktree /b\nHEAD 2";
        let wts = parse_worktree_list(text);
        assert_eq!(wts.len(), 2);
        assert_eq!(wts[1].path, PathBuf::from("/b"));
    }

    #[test]
    fn empty_worktree_list_is_empty_not_a_phantom_entry() {
        assert!(parse_worktree_list("").is_empty());
        assert!(parse_worktree_list("\n\n\n").is_empty());
    }

    #[test]
    fn slugify_produces_valid_ref_components() {
        assert_eq!(slugify("Fix issue #217"), "fix-issue-217");
        assert_eq!(slugify("feature/new thing"), "feature/new-thing");
        assert_eq!(slugify("  spaces  "), "spaces");
        assert_eq!(slugify("a..b"), "a-b");
        assert_eq!(slugify("weird~^:?*[\\name"), "weird-name");
        assert_eq!(slugify(""), "unnamed");
        assert_eq!(slugify("---"), "unnamed");
        assert_eq!(slugify("UPPER"), "upper");
    }

    #[test]
    fn slugify_output_contains_no_sequence_git_rejects() {
        for input in [
            "a..b",
            "~tilde",
            "^caret",
            "colon:name",
            "star*",
            "[bracket]",
            "back\\slash",
            "question?",
            "/leading",
            "trailing/",
            "double//slash",
            "@{brace}",
        ] {
            let s = slugify(input);
            assert!(!s.contains(".."), "{input} -> {s}");
            assert!(!s.starts_with('/'), "{input} -> {s}");
            assert!(!s.ends_with('/'), "{input} -> {s}");
            assert!(!s.ends_with(".lock"), "{input} -> {s}");
            assert!(
                s.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '/')),
                "{input} -> {s}"
            );
            assert!(!s.is_empty(), "{input} produced an empty slug");
        }
    }

    #[test]
    fn slugify_collapses_runs_of_separators() {
        assert_eq!(slugify("a   b"), "a-b");
        assert_eq!(slugify("a---b"), "a-b");
    }
}
