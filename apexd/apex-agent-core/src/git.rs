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

/// Whether `dir` is a LINKED worktree rather than a repository's main one.
///
/// `--git-dir` and `--git-common-dir` name the same directory in a main
/// worktree and different ones in a linked worktree
/// (`.git/worktrees/<name>` against `.git`). MEASURED on git 2.55.0 rather
/// than assumed, from both sides.
///
/// Asked because a linked worktree is not a project: every worktree of the
/// repository is visible from inside it, so anything that enumerates
/// "this project's worktrees" from a linked one produces a second copy of
/// every row, with the linked worktree's own name on all of them.
pub fn is_linked_worktree(dir: &Path) -> bool {
    let Some(out) = git_opt(
        dir,
        &["rev-parse", "--path-format=absolute", "--git-dir", "--git-common-dir"],
    ) else {
        // Not a repository at all. Not a linked worktree either, and saying
        // "yes" here would hide an ordinary directory from a listing.
        return false;
    };
    let mut lines = out.lines();
    match (lines.next(), lines.next()) {
        (Some(git_dir), Some(common)) => git_dir != common,
        _ => false,
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
/// A slug for a filesystem PATH: [`slugify`] with the separators flattened.
///
/// `slugify` keeps `/` on purpose, because it also names worktree branches
/// (`agent/issue-217`) where a slash is meaningful. For a path that is wrong,
/// and it was wrong for a long time in a way nothing reported:
///
///   * `/var/home/andre/Projects/apex/apex-os` slugged to
///     `var/home/andre/projects/apex/apex-os`;
///   * the project record path became
///     `projects/var/home/andre/projects/apex/apex-os.json`;
///   * `project::remember` did not create those intermediate directories, so
///     it returned an error for EVERY real project;
///   * the daemon called it as `let _ = project::remember(proj)`, so the error
///     went nowhere;
///   * and `project::list` reads only the top level, so even a record that had
///     been written would have been invisible.
///
/// Net effect: `apex project list` never listed anything, on any machine, and
/// no test noticed because every fixture path was also nested.
pub fn path_slug(path: &str) -> String {
    slugify(&path.replace('/', "-"))
}

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

/// How many commits each side of `base...head` has that the other does not.
///
/// `(ahead, behind)` from `head`'s point of view: ahead is what `head` carries
/// and `base` does not. `None` when either ref does not resolve, which is a
/// legitimate answer for a worktree whose base branch was deleted.
pub fn ahead_behind(dir: &Path, base: &str, head: &str) -> Option<(u32, u32)> {
    let range = format!("{base}...{head}");
    let out = git_opt(dir, &["rev-list", "--left-right", "--count", &range])?;
    parse_ahead_behind(&out)
}

/// Parse `git rev-list --left-right --count`, which prints `behind<TAB>ahead`.
///
/// Split out because the column order is the one thing here that is easy to
/// get backwards and impossible to notice: the LEFT count is `base`'s own
/// commits, which is how far `head` is BEHIND.
pub fn parse_ahead_behind(text: &str) -> Option<(u32, u32)> {
    let mut parts = text.split_whitespace();
    let behind: u32 = parts.next()?.parse().ok()?;
    let ahead: u32 = parts.next()?.parse().ok()?;
    Some((ahead, behind))
}

/// The upstream of `branch` (`origin/foo`), or `None` when it has none.
pub fn upstream_of(dir: &Path, branch: &str) -> Option<String> {
    let spec = format!("{branch}@{{upstream}}");
    let out = git_opt(dir, &["rev-parse", "--abbrev-ref", &spec])?;
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

/// A summary of `git diff --numstat`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DiffStat {
    pub files: u32,
    pub insertions: u32,
    pub deletions: u32,
}

/// The committed delta of `head` against `base`, as files/insertions/deletions.
///
/// Three dots, so this is what `head` added since the merge base rather than
/// every difference between two branches — the same range a reviewer would
/// read.
pub fn diff_stat(dir: &Path, base: &str, head: &str) -> Option<DiffStat> {
    let range = format!("{base}...{head}");
    let out = git_opt(dir, &["diff", "--numstat", &range])?;
    Some(parse_numstat(&out))
}

/// Parse `git diff --numstat`.
///
/// A binary file's counts are `-` rather than a number. Those lines count as a
/// changed file and add nothing to the line totals, which is the honest answer:
/// a binary change has no line count, and treating `-` as zero would report a
/// changed file with an empty diff.
pub fn parse_numstat(text: &str) -> DiffStat {
    let mut stat = DiffStat::default();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.split('\t');
        let (Some(add), Some(del)) = (parts.next(), parts.next()) else {
            continue;
        };
        // A third field is the path. Without one this is not a numstat line.
        if parts.next().is_none() {
            continue;
        }
        stat.files += 1;
        stat.insertions += add.parse::<u32>().unwrap_or(0);
        stat.deletions += del.parse::<u32>().unwrap_or(0);
    }
    stat
}

/// What a merge of two branches WOULD do, without doing it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeProbe {
    /// The merge would apply cleanly.
    Clean,
    /// The merge would conflict, in these paths.
    Conflicted(Vec<String>),
    /// Git could not answer. Carries the reason for a human, never a guess.
    Unknown(String),
}

/// Whether merging `head` into `base` would conflict — asked WITHOUT merging.
///
/// This is the whole reason the module prefers plumbing. The obvious
/// implementation, `git merge --no-commit` in the worktree, would leave a
/// half-merged index and a `MERGE_HEAD` in a checkout that an agent — or
/// Andre — is sitting in and typing into. `merge-tree` computes the same merge
/// entirely in the object database and writes nothing any working tree can
/// see.
///
/// Run it from the project root against REF NAMES, never from inside the agent
/// worktree: then the answer cannot depend on that tree's HEAD, its index or
/// its uncommitted dirt.
///
/// One honest caveat, because "reads only" would be an overclaim:
/// `--write-tree` DOES write the merged tree and its blobs into the common
/// object store as unreferenced objects. Nothing points at them, `git gc`
/// removes them, and no index, stash, branch or working tree is touched — but
/// the repository is not left byte-for-byte identical, and a doc that said
/// otherwise would be wrong.
///
/// Exit codes, measured on git 2.55.0 rather than assumed: 0 is clean, 1 is
/// conflicts, and everything else is a question git declined to answer
/// (unrelated histories and an unresolvable ref both exit 128). Only 0 and 1
/// are trusted; anything else degrades to [`MergeProbe::Unknown`] so that one
/// odd worktree cannot fail a whole status listing.
pub fn merge_tree_probe(repo: &Path, base: &str, head: &str) -> MergeProbe {
    let out = Command::new("git")
        .current_dir(repo)
        .args(["merge-tree", "--write-tree", "--name-only", "-z", base, head])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output();
    let out = match out {
        Ok(o) => o,
        Err(e) => return MergeProbe::Unknown(format!("running git merge-tree: {e}")),
    };
    match out.status.code() {
        Some(0) => MergeProbe::Clean,
        Some(1) => {
            let names = parse_merge_tree_z(&String::from_utf8_lossy(&out.stdout));
            if names.is_empty() {
                // Exit 1 with nothing named is still a conflict, and calling it
                // clean because the parse came up empty would invert the
                // answer. Report it as unknown instead.
                return MergeProbe::Unknown(
                    "git merge-tree reported a conflict without naming a path".to_string(),
                );
            }
            MergeProbe::Conflicted(names)
        }
        _ => {
            let err = String::from_utf8_lossy(&out.stderr);
            let line = err.trim().lines().next().unwrap_or("git merge-tree failed");
            MergeProbe::Unknown(line.to_string())
        }
    }
}

/// Parse the NUL-separated `git merge-tree --write-tree --name-only -z` output.
///
/// Measured shape: the tree OID, then one conflicted path per field, then an
/// EMPTY field, then a structured message section this does not read. `-z`
/// rather than newlines because a path is allowed to contain a newline and the
/// line-based form would split one path into two.
pub fn parse_merge_tree_z(text: &str) -> Vec<String> {
    text.split('\0')
        .skip(1) // the tree OID
        .take_while(|f| !f.is_empty())
        .map(|f| f.to_string())
        .collect()
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

    #[test]
    fn ahead_behind_reads_the_columns_in_git_order() {
        // `--left-right --count` prints LEFT then RIGHT, and with `base...head`
        // the left side is base's own commits — which is how far head is
        // BEHIND. Getting this backwards would report a worktree with nothing
        // to propose as ready and vice versa, so it is pinned.
        assert_eq!(parse_ahead_behind("1\t1"), Some((1, 1)));
        assert_eq!(parse_ahead_behind("0\t4"), Some((4, 0)), "4 ahead, 0 behind");
        assert_eq!(parse_ahead_behind("3\t0"), Some((0, 3)), "0 ahead, 3 behind");
        assert_eq!(parse_ahead_behind(""), None);
        assert_eq!(parse_ahead_behind("7"), None, "one column is not an answer");
        assert_eq!(parse_ahead_behind("a\tb"), None);
    }

    #[test]
    fn numstat_totals_lines_and_counts_files() {
        let text = "1\t1\tf.txt\n1\t0\tn.txt\n";
        assert_eq!(
            parse_numstat(text),
            DiffStat {
                files: 2,
                insertions: 2,
                deletions: 1
            }
        );
        assert_eq!(parse_numstat(""), DiffStat::default());
    }

    #[test]
    fn a_binary_file_counts_as_changed_with_no_lines() {
        // git prints `-` for a binary file's counts. It is a changed file with
        // no line count; reporting zero changed files would hide it, and
        // parsing `-` as a number would drop the file.
        let stat = parse_numstat("-\t-\tlogo.png\n2\t1\tsrc/main.rs\n");
        assert_eq!(stat.files, 2, "the binary file is still a changed file");
        assert_eq!(stat.insertions, 2);
        assert_eq!(stat.deletions, 1);
    }

    #[test]
    fn merge_tree_z_yields_only_the_conflicted_paths() {
        // Real captured output (git 2.55.0): OID, the conflicted path, an
        // empty field, then the message section — which must NOT be mistaken
        // for more paths.
        //
        // `\x00` and not `\0`: the message section's first field is the digit
        // `1`, so `\01` reads as an octal escape that Rust does not have (it
        // is a NUL followed by a one) and clippy::octal_escapes refuses it.
        // Written unambiguously, because getting this literal wrong would
        // silently change which fields the parser is being shown.
        let text = concat!(
            "6513cc3bd3dcd48685fdde56dc293742f6f2f367\x00",
            "f.txt\x00",
            "\x00", // the empty field that ends the path list
            "1\x00f.txt\x00Auto-merging\x00Auto-merging f.txt\n\x00",
        );
        assert_eq!(parse_merge_tree_z(text), vec!["f.txt".to_string()]);
    }

    #[test]
    fn merge_tree_z_on_a_clean_merge_names_nothing() {
        // A clean merge prints the tree OID and nothing else. If this returned
        // the OID as a "conflicted path", every clean worktree would report a
        // conflict in a file named after a hash.
        let text = "4a9e153f2fbfe9a04e637ce84ecc51c8ef78201e\0";
        assert!(parse_merge_tree_z(text).is_empty());
        assert!(parse_merge_tree_z("4a9e153\0").is_empty());
    }

    #[test]
    fn merge_tree_z_keeps_a_path_containing_a_newline() {
        // The reason for `-z`. A line-based parser would split this one path
        // into two, and one of the halves would name a file that does not
        // exist.
        let text = "abc123\0weird\nname.txt\0other.txt\0\0info";
        assert_eq!(
            parse_merge_tree_z(text),
            vec!["weird\nname.txt".to_string(), "other.txt".to_string()]
        );
    }
}

#[cfg(test)]
mod path_slug_tests {
    use super::*;

    #[test]
    fn a_path_slug_is_flat() {
        // The bug: a slug containing `/` becomes a nested record path, whose
        // parent directories nothing created and whose contents `list` never
        // read. Every real project path is nested, so this affected all of them.
        for path in [
            "/var/home/andre/Projects/apex/apex-os",
            "/home/andre/Projects/demo",
            "/tmp/tmp.XXXX/mine",
        ] {
            let slug = path_slug(path);
            assert!(!slug.contains('/'), "{path} -> {slug}");
            assert!(!slug.is_empty(), "{path}");
            assert!(!slug.starts_with('-'), "{path} -> {slug}");
        }
    }

    #[test]
    fn distinct_paths_keep_distinct_slugs() {
        // Flattening must not collide two projects into one record.
        let a = path_slug("/home/me/Projects/apex");
        let b = path_slug("/home/me/Projects/apex-os");
        let c = path_slug("/home/me/other/apex");
        assert_ne!(a, b);
        assert_ne!(a, c);
        assert_ne!(b, c);
    }

    #[test]
    fn slugify_still_keeps_slashes_for_branch_names() {
        // The other caller wants them: worktree branches are `agent/<name>`.
        assert!(slugify("agent/issue-217").contains('/'));
    }
}
