//! Projects as a first-class runtime object.
//!
//! A project is a git working tree plus what the runtime has learned about it:
//! the toolchains it uses, the worktrees agents are running in, and when it was
//! last opened. Detection is cheap and file-based — no language server, no
//! build invocation — because it runs on every `apex agent run`.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::git;
use crate::paths;

/// Directory under a project root holding agent worktrees.
///
/// Inside the repository so `git worktree list` and the user's file manager
/// both find them, and a single directory so one `.gitignore` line covers all
/// of them.
pub const WORKTREE_DIR: &str = ".apex/worktrees";

/// Branch prefix for agent worktrees, matching the roadmap's `agent/issue-217`.
pub const BRANCH_PREFIX: &str = "agent";

/// A marker file and the toolchain it implies.
const MARKERS: &[(&str, &str)] = &[
    ("Cargo.toml", "rust"),
    ("package.json", "node"),
    ("pyproject.toml", "python"),
    ("requirements.txt", "python"),
    ("setup.py", "python"),
    ("go.mod", "go"),
    ("pom.xml", "java"),
    ("build.gradle", "java"),
    ("build.gradle.kts", "kotlin"),
    ("CMakeLists.txt", "cmake"),
    ("Makefile", "make"),
    ("meson.build", "meson"),
    ("Gemfile", "ruby"),
    ("composer.json", "php"),
    ("mix.exs", "elixir"),
    ("Package.swift", "swift"),
    ("pubspec.yaml", "dart"),
    ("flake.nix", "nix"),
    ("Containerfile", "container"),
    ("Dockerfile", "container"),
    ("*.qml", "qml"),
];

/// A detected project.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Project {
    /// Absolute repository root.
    pub root: String,
    /// Directory name, used for display.
    pub name: String,
    /// Stable identifier derived from the path.
    pub slug: String,
    /// Toolchains detected from marker files, sorted and deduplicated.
    pub languages: Vec<String>,
    /// Unix seconds this project was last used by the runtime.
    pub last_opened: u64,
}

impl Project {
    /// Where agent worktrees for this project live.
    pub fn worktree_root(&self) -> PathBuf {
        Path::new(&self.root).join(WORKTREE_DIR)
    }

    /// The path a named worktree gets.
    pub fn worktree_path(&self, name: &str) -> PathBuf {
        self.worktree_root().join(git::slugify(name))
    }

    /// The branch a named worktree gets.
    pub fn worktree_branch(&self, name: &str) -> String {
        format!("{BRANCH_PREFIX}/{}", git::slugify(name))
    }
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Identify the project containing `dir`, if any.
///
/// Returns `None` outside a git repository rather than inventing a project from
/// a bare directory: without a repository there is nothing to checkpoint, no
/// worktrees to create, and calling it a project would promise both.
pub fn detect(dir: &Path) -> Option<Project> {
    let root = git::toplevel(dir)?;
    let name = root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| root.to_string_lossy().into_owned());
    Some(Project {
        slug: git::path_slug(&root.to_string_lossy()),
        languages: detect_languages(&root),
        root: root.to_string_lossy().into_owned(),
        name,
        last_opened: now_secs(),
    })
}

/// Toolchains implied by marker files at the repository root.
///
/// Root-level only, and one `read_dir` — a recursive scan of a large monorepo
/// on every `apex agent run` would be a visible pause for no extra accuracy.
pub fn detect_languages(root: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let names: Vec<String> = entries
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    languages_from_names(&names)
}

/// The marker-matching half of [`detect_languages`], without the filesystem.
pub fn languages_from_names(names: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (marker, lang) in MARKERS {
        let hit = if let Some(ext) = marker.strip_prefix("*.") {
            names
                .iter()
                .any(|n| n.rsplit('.').next() == Some(ext) && n.len() > ext.len() + 1)
        } else {
            names.iter().any(|n| n == marker)
        };
        if hit && !out.iter().any(|l| l == lang) {
            out.push((*lang).to_string());
        }
    }
    out.sort();
    out
}

fn record_path(slug: &str) -> PathBuf {
    paths::projects_dir().join(format!("{slug}.json"))
}

/// Record that a project was used, so `apex project list` can order by recency.
pub fn remember(project: &Project) -> Result<()> {
    let dir = paths::projects_dir();
    paths::ensure_private_dir(&dir)?;
    let mut p = project.clone();
    p.last_opened = now_secs();
    let path = record_path(&p.slug);
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(&p)?)
        .with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

/// Every remembered project, most recently opened first.
///
/// Projects whose directory has gone are dropped from the listing and their
/// record deleted: a stale entry pointing at a deleted checkout is noise the
/// user cannot act on.
pub fn list() -> Vec<Project> {
    let Ok(entries) = std::fs::read_dir(paths::projects_dir()) else {
        return Vec::new();
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
        let Ok(p) = serde_json::from_str::<Project>(&text) else {
            continue;
        };
        if Path::new(&p.root).is_dir() {
            out.push(p);
        } else {
            let _ = std::fs::remove_file(&path);
        }
    }
    out.sort_by(|a, b| b.last_opened.cmp(&a.last_opened).then(a.name.cmp(&b.name)));
    out
}

/// Forget a remembered project. The checkout itself is never touched.
pub fn forget(slug: &str) -> Result<()> {
    let path = record_path(slug);
    if path.exists() {
        std::fs::remove_file(&path)?;
    }
    Ok(())
}

/// A worktree belonging to a project, as the runtime presents it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentWorktree {
    pub name: String,
    pub path: PathBuf,
    pub branch: Option<String>,
    /// False for the project's own main working tree.
    pub is_agent: bool,
}

/// Every worktree of `project`, main tree first.
pub fn worktrees(project: &Project) -> Result<Vec<AgentWorktree>> {
    let root = Path::new(&project.root);
    let wt_root = project.worktree_root();
    Ok(git::worktrees(root)?
        .into_iter()
        .map(|w| {
            let is_agent = !w.is_main && w.path.starts_with(&wt_root);
            let name = if is_agent {
                w.path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default()
            } else {
                project.name.clone()
            };
            AgentWorktree {
                name,
                path: w.path,
                branch: w.branch,
                is_agent,
            }
        })
        .collect())
}

/// Create (or reuse) the worktree `name` for `project` and return its path.
///
/// Idempotent, so re-running `apex agent run --worktree issue-217` reattaches
/// to the same tree instead of failing or making a second one.
pub fn ensure_worktree(project: &Project, name: &str) -> Result<PathBuf> {
    let root = Path::new(&project.root);
    let path = project.worktree_path(name);
    let branch = project.worktree_branch(name);

    paths::ensure_private_dir(&project.worktree_root())?;
    ensure_ignored(project)?;

    let base = git::current_branch(root)
        .or_else(|| git::head_commit(root))
        .unwrap_or_else(|| "HEAD".to_string());
    git::add_worktree(root, &path, &branch, Some(&base))?;
    Ok(path)
}

/// Remove an agent worktree and its branch.
pub fn remove_worktree(project: &Project, name: &str, delete_branch: bool) -> Result<()> {
    let path = project.worktree_path(name);
    git::remove_worktree(Path::new(&project.root), &path, delete_branch)
}

/// Make sure `.apex/` is ignored, without disturbing the user's `.gitignore`.
///
/// Written to `.git/info/exclude` rather than `.gitignore`: the worktree
/// directory is this machine's runtime state, not something to commit into the
/// user's repository and push to their colleagues.
fn ensure_ignored(project: &Project) -> Result<()> {
    let root = Path::new(&project.root);
    let Some(common) = git::common_dir(root) else {
        return Ok(());
    };
    let exclude = common.join("info/exclude");
    let entry = "/.apex/";

    let existing = std::fs::read_to_string(&exclude).unwrap_or_default();
    if existing.lines().any(|l| l.trim() == entry) {
        return Ok(());
    }
    if let Some(parent) = exclude.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut text = existing;
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str("# APEX agent worktrees\n");
    text.push_str(entry);
    text.push('\n');
    std::fs::write(&exclude, text).with_context(|| format!("writing {}", exclude.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> Project {
        Project {
            root: "/home/tester/Projects/demo".into(),
            name: "demo".into(),
            slug: "home-tester-projects-demo".into(),
            languages: vec![],
            last_opened: 0,
        }
    }

    #[test]
    fn marker_files_map_to_toolchains() {
        let names = vec!["Cargo.toml".to_string(), "README.md".to_string()];
        assert_eq!(languages_from_names(&names), vec!["rust".to_string()]);
    }

    #[test]
    fn several_markers_are_all_reported_and_sorted() {
        let names: Vec<String> = ["Cargo.toml", "package.json", "go.mod"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            languages_from_names(&names),
            vec!["go".to_string(), "node".to_string(), "rust".to_string()]
        );
    }

    #[test]
    fn two_markers_for_one_language_do_not_duplicate_it() {
        let names: Vec<String> = ["pyproject.toml", "requirements.txt", "setup.py"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(languages_from_names(&names), vec!["python".to_string()]);
    }

    #[test]
    fn a_glob_marker_matches_by_extension() {
        let names = vec!["shell.qml".to_string()];
        assert_eq!(languages_from_names(&names), vec!["qml".to_string()]);
        // A file that is only the extension is not a match.
        assert!(languages_from_names(&[".qml".to_string()]).is_empty());
    }

    #[test]
    fn an_unmarked_directory_reports_no_language() {
        let names = vec!["README.md".to_string(), "LICENSE".to_string()];
        assert!(languages_from_names(&names).is_empty());
    }

    #[test]
    fn worktree_paths_and_branches_are_derived_from_the_name() {
        let p = project();
        assert_eq!(
            p.worktree_path("issue-217"),
            PathBuf::from("/home/tester/Projects/demo/.apex/worktrees/issue-217")
        );
        assert_eq!(p.worktree_branch("issue-217"), "agent/issue-217");
    }

    #[test]
    fn worktree_names_are_slugified_so_a_free_text_task_is_safe() {
        let p = project();
        assert_eq!(
            p.worktree_path("Fix the login bug!"),
            PathBuf::from("/home/tester/Projects/demo/.apex/worktrees/fix-the-login-bug")
        );
        assert_eq!(p.worktree_branch("Fix the login bug!"), "agent/fix-the-login-bug");
    }

    #[test]
    fn a_worktree_name_cannot_escape_the_worktree_root() {
        // Slugification strips the separators a traversal needs.
        let p = project();
        let path = p.worktree_path("../../etc/passwd");
        assert!(
            path.starts_with(p.worktree_root()),
            "escaped to {}",
            path.display()
        );
        assert!(!path.to_string_lossy().contains(".."));
    }

    #[test]
    fn the_worktree_directory_lives_inside_the_project() {
        let p = project();
        assert!(p.worktree_root().starts_with(&p.root));
        assert!(p.worktree_root().ends_with("worktrees"));
    }
}
