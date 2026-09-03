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
    /// The APEX capsule (§8) this project's work belongs in, if the user has
    /// bound one. `apex env` owns the capsule; this is only the name.
    ///
    /// `#[serde(default)]` because every project record written before capsules
    /// existed has no such key, and a missing field must read as "no binding",
    /// not as a parse failure that makes the project vanish from the listing.
    #[serde(default)]
    pub capsule: Option<String>,
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
    let slug = git::path_slug(&root.to_string_lossy());
    Some(Project {
        // Detection reads the filesystem; the capsule binding is a decision the
        // user made and only the record holds it. Reading it back here is what
        // makes `apex project info` and the resolver see the binding without
        // every caller having to remember to merge.
        capsule: load(&slug).and_then(|p| p.capsule),
        slug,
        languages: detect_languages(&root),
        root: root.to_string_lossy().into_owned(),
        name,
        last_opened: now_secs(),
    })
}

/// The stored record for `slug`, if there is one.
pub fn load(slug: &str) -> Option<Project> {
    let text = std::fs::read_to_string(record_path(slug)).ok()?;
    serde_json::from_str(&text).ok()
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
///
/// The record is REPLACED, not merged, with one exception: a capsule binding
/// already on disk survives a `remember` that does not carry one. This is not
/// defensive coding, it is the difference between the feature working and not
/// working — `remember` runs on every `apex agent run` and every layout save,
/// with a freshly detected project whose only source is the filesystem, and
/// the filesystem does not know which capsule the user chose. Without this,
/// binding a capsule and then starting an agent would silently unbind it.
pub fn remember(project: &Project) -> Result<()> {
    let dir = paths::projects_dir();
    paths::ensure_private_dir(&dir)?;
    let mut p = project.clone();
    p.last_opened = now_secs();
    if p.capsule.is_none() {
        p.capsule = load(&p.slug).and_then(|old| old.capsule);
    }
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

// ── project ↔ capsule binding (§8) ──────────────────────────────────────────

/// Is `name` a capsule name `apex env` would accept?
///
/// Duplicated from the engine on purpose, and kept narrow for the same reason
/// the engine keeps it narrow: the name becomes a container name and a file
/// path, so a binding must never store something that later expands into a
/// path somewhere else. Rejecting it here means the user finds out when they
/// bind, not when a command inside the capsule fails.
pub fn valid_capsule_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 40
        && !name.contains("..")
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-'))
        && name
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
}

/// Bind `project` to a capsule, or clear the binding with `None`.
///
/// Writes through `remember`, so a project that has never been recorded gets a
/// record here rather than silently accepting a binding that nothing stores.
pub fn bind_capsule(project: &Project, capsule: Option<&str>) -> Result<()> {
    if let Some(name) = capsule {
        if !valid_capsule_name(name) {
            anyhow::bail!(
                "'{name}' is not a usable capsule name \
                 (lowercase letters, digits, . _ - ; at most 40 characters)"
            );
        }
    }
    let mut p = project.clone();
    p.capsule = capsule.map(|c| c.to_string());
    // remember() preserves an existing binding when the incoming one is None,
    // which is exactly wrong for a deliberate clear, so the clear is written
    // directly instead of going back through it.
    let dir = paths::projects_dir();
    paths::ensure_private_dir(&dir)?;
    p.last_opened = now_secs();
    let path = record_path(&p.slug);
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(&p)?)
        .with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

/// The capsule image alias that suits this project's toolchains, if any.
///
/// A suggestion, printed and never acted on: creating a container because a
/// `package.json` exists would be a surprise measured in gigabytes.
///
/// The table is about what the APEX image ACTUALLY SHIPS, not about what looks
/// tidy. `python3` and `git` are on the host, so a Python project is suggested
/// a capsule for the pip-and-venv litter rather than for a missing
/// interpreter. Node, Go, Rust, Java, Ruby, PHP and the rest have no host
/// toolchain at all — on a read-only /usr that is a capsule, not a
/// `dnf install`.
///
/// Deliberately silent for `nix` (it manages its own store), `container` and
/// `qml` (podman and quickshell are in the image), and for a project with no
/// detected toolchain.
pub fn suggested_capsule(languages: &[String]) -> Option<&'static str> {
    if languages.iter().any(|l| l == "python") {
        return Some("python");
    }
    const NEEDS_A_TOOLCHAIN: &[&str] = &[
        "node", "go", "rust", "java", "kotlin", "ruby", "php", "elixir", "dart", "swift", "cmake",
        "meson",
    ];
    if languages
        .iter()
        .any(|l| NEEDS_A_TOOLCHAIN.contains(&l.as_str()))
    {
        return Some("fedora");
    }
    None
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
            capsule: None,
        }
    }

    #[test]
    fn a_record_written_before_capsules_existed_still_parses() {
        // Every project record on every installed machine looks like this. If
        // the new field were required, they would all fail to deserialise and
        // `apex project list` would report no projects at all.
        let old = r#"{"root":"/p/demo","name":"demo","slug":"p-demo",
                      "languages":["rust"],"last_opened":17}"#;
        let p: Project = serde_json::from_str(old).expect("old records parse");
        assert_eq!(p.capsule, None);
        assert_eq!(p.name, "demo");
    }

    #[test]
    fn capsule_names_are_validated_before_they_are_stored() {
        assert!(valid_capsule_name("fedora"));
        assert!(valid_capsule_name("ml-2024"));
        assert!(valid_capsule_name("py_3.13"));
        // The name becomes a container name and a file path in the engine.
        assert!(!valid_capsule_name("../../etc/passwd"));
        assert!(!valid_capsule_name("a..b"));
        assert!(!valid_capsule_name("a/b"));
        assert!(!valid_capsule_name("-rf"));
        assert!(!valid_capsule_name("Fedora"));
        assert!(!valid_capsule_name(""));
        assert!(!valid_capsule_name("a b"));
        assert!(!valid_capsule_name(&"a".repeat(41)));
        assert!(valid_capsule_name(&"a".repeat(40)));
    }

    #[test]
    fn binding_refuses_a_name_the_engine_would_not_accept() {
        let p = project();
        assert!(bind_capsule(&p, Some("../escape")).is_err());
    }

    #[test]
    fn a_python_project_is_suggested_a_python_capsule() {
        assert_eq!(
            suggested_capsule(&["python".to_string()]),
            Some("python")
        );
    }

    #[test]
    fn a_toolchain_the_image_does_not_ship_is_suggested_a_capsule() {
        for lang in ["node", "go", "rust", "java", "ruby"] {
            assert_eq!(
                suggested_capsule(&[lang.to_string()]),
                Some("fedora"),
                "{lang} got no suggestion"
            );
        }
    }

    #[test]
    fn nothing_is_suggested_where_the_host_already_serves() {
        // podman and quickshell are in the image; nix manages its own store.
        for lang in ["container", "qml", "nix"] {
            assert_eq!(suggested_capsule(&[lang.to_string()]), None, "{lang}");
        }
        assert_eq!(suggested_capsule(&[]), None);
    }

    #[test]
    fn python_wins_over_a_second_toolchain() {
        // A Python project with a Makefile is a Python project.
        let langs = vec!["make".to_string(), "python".to_string()];
        assert_eq!(suggested_capsule(&langs), Some("python"));
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
