//! Content digests for a directory tree, and the four answers one can give.
//!
//! Shared by [`crate::skill`] (P1-025) and [`crate::provenance`] (P1-026),
//! which both need the same thing: *the hash of what is actually on disk*, as
//! opposed to a version string some registry recorded and nothing re-checks.
//!
//! ## Why `sha256sum` and not a crate
//!
//! `trust.rs` and `verify.rs` already establish the pattern — that module does
//! civil-to-days arithmetic by hand rather than take a date library for one
//! conversion, and it verifies signatures by driving `openssl` and `skopeo`
//! rather than linking a Sigstore stack. Following it here has a second
//! benefit beyond consistency: a hash produced by a program that might not be
//! installed makes "I could not measure this" a state the type system has to
//! carry, which is precisely the distinction this codebase keeps having to
//! re-learn. A linked-in hasher can never fail, so it would quietly encourage
//! a two-valued answer.
//!
//! ## What "permission denied is not absence" means for a digest
//!
//! Roughly fourteen defects in this repository have had the same shape: a
//! failed `stat` or a refused `read_dir` reported as a fact about the world.
//! An inventory is where the fifteenth would live, so the walk below
//! distinguishes, per directory:
//!
//!   * it was read, and here is what is in it;
//!   * it exists and could not be read — [`Walk::denied`] is non-empty and the
//!     digest is [`Digest::Unmeasured`], never a hash over the part that
//!     happened to be readable;
//!   * it is not there at all.
//!
//! A digest computed over a partially-readable tree would be a number that
//! looked exactly like a real one and meant nothing, and it would compare
//! unequal on the next run for a reason that has nothing to do with the
//! content changing. So a tree with any unreadable part has no digest.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// How many paths are handed to one `sha256sum`.
///
/// `ARG_MAX` is on the order of two million bytes on Linux, so this is far
/// below any real limit; it exists so that a pathological tree cannot turn into
/// an `E2BIG` that would read as a hashing failure.
const CHUNK: usize = 256;

/// The name of the hashing program, overridable so the suite can point at a
/// stub that fails on purpose and prove the [`Digest::Unmeasured`] arm.
fn sha256sum() -> String {
    std::env::var("APEX_SHA256SUM").unwrap_or_else(|_| "sha256sum".to_string())
}

/// A content digest, or the reason there is not one.
///
/// Two values, and the second is not a failure — it is the honest answer when
/// the bytes could not be read or the hasher could not be run. Nothing may
/// treat it as a mismatch: `verify.rs` learned this the expensive way, that
/// collapsing "no" into "I could not tell" either refuses everything on an
/// offline machine or accepts everything unverified, depending which way the
/// collapse goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Digest {
    /// Lowercase hex, 64 characters, over the manifest described by [`digest`].
    Sha256(String),
    /// No digest, and why. Never rendered as a hash and never compared equal
    /// or unequal to one.
    Unmeasured(String),
}

impl Digest {
    pub fn describe(&self) -> String {
        match self {
            Digest::Sha256(h) => h.clone(),
            Digest::Unmeasured(why) => format!("not measured: {why}"),
        }
    }

    /// The short form for a table.
    pub fn short(&self) -> String {
        match self {
            Digest::Sha256(h) => h.chars().take(12).collect(),
            Digest::Unmeasured(_) => "—".to_string(),
        }
    }

    pub fn is_measured(&self) -> bool {
        matches!(self, Digest::Sha256(_))
    }

    /// Compare two digests, keeping "could not tell" out of the answer.
    ///
    /// Returns `None` when either side is unmeasured, so a caller cannot
    /// accidentally read an unmeasurable pair as a mismatch. Every caller has
    /// to handle the third case explicitly, which is the point.
    pub fn same_as(&self, other: &Digest) -> Option<bool> {
        match (self, other) {
            (Digest::Sha256(a), Digest::Sha256(b)) => Some(a == b),
            _ => None,
        }
    }

    pub fn to_json(&self) -> serde_json::Value {
        match self {
            Digest::Sha256(h) => serde_json::json!({"sha256": h, "measured": true}),
            Digest::Unmeasured(why) => {
                serde_json::json!({"sha256": null, "measured": false, "why": why})
            }
        }
    }
}

/// What a walk of one directory tree found.
#[derive(Debug, Clone, Default)]
pub struct Walk {
    /// Relative path → whether any execute bit is set.
    pub files: BTreeMap<String, bool>,
    /// Directories that exist and could not be read, with the error. A
    /// non-empty list means the walk is incomplete and **no digest is taken**.
    pub denied: Vec<String>,
    /// Symlinks found, by relative path → target as written.
    ///
    /// Counted for the same reason `apex-plugin`'s `enumerate` counts them: a
    /// component shipping a symlink out of its own directory turns an approved
    /// relative path into a read of something else, and only the filesystem can
    /// see that. Not followed.
    pub links: BTreeMap<String, String>,
    /// Whether the root existed at all.
    pub present: bool,
    /// Root-relative paths never descended into, as asked for by
    /// [`walk_excluding`]. Reported so a digest is never quietly partial.
    pub exclude: Vec<String>,
    /// Relative paths actually skipped because of [`Walk::exclude`].
    ///
    /// The difference between "we would skip this name" and "we did skip
    /// something": a reader of a digest is entitled to know which bytes it does
    /// not cover, and an exclusion that matched nothing is worth seeing too.
    pub excluded: Vec<String>,
}

impl Walk {
    pub fn file_count(&self) -> usize {
        self.files.len()
    }

    /// The files with an execute bit, sorted.
    pub fn executables(&self) -> Vec<&str> {
        self.files
            .iter()
            .filter(|(_, x)| **x)
            .map(|(p, _)| p.as_str())
            .collect()
    }

    pub fn complete(&self) -> bool {
        self.present && self.denied.is_empty()
    }
}

/// Walk one tree, recording refusals rather than swallowing them.
///
/// Symlinks are recorded and never followed: following one would let a
/// component's digest cover bytes outside its own directory, and a link into
/// `$HOME` would make the digest change whenever unrelated files did.
pub fn walk(root: &Path) -> Walk {
    walk_excluding(root, &[])
}

/// [`walk`], skipping the named **root-relative paths**.
///
/// ## Why an exclusion list exists at all, and why it is not a convenience
///
/// A digest is only useful for re-checking if an untouched component hashes the
/// same tomorrow. Claude Code writes runtime state *inside* an installed
/// plugin's own tree — `.in_use/<pid>`, one file per live process — so a digest
/// that covered it would change every time the plugin was used, and a
/// provenance check built on it would report "changed" constantly. A check that
/// cries wolf on every run is worse than no check: it trains whoever reads it
/// to ignore the one time it means something.
///
/// ## Why the match is root-relative and not by name at any depth
///
/// The first version of this matched a bare component wherever it appeared,
/// which is the wrong shape for a digest: it meant that anything placed under a
/// directory called `.in_use` **anywhere** in the tree was invisible to both
/// the hash and [`Walk::executables`], so a script hidden at
/// `tools/.in_use/go.sh` would never be measured at all. Provenance is the hash
/// of what runs; an exclusion broad enough to hide a program from it is a hole,
/// not a convenience.
///
/// So each entry is matched against the whole root-relative path. `.in_use`
/// skips exactly the one entry at the root — which is where Claude Code
/// actually writes it, measured on this machine: every `installPath` in
/// `installed_plugins.json` carries `.in_use` as a direct child — while
/// `tools/.in_use` is hashed like any other directory. A whole path component
/// still has to match, so `.in_use` never matches a file called `not.in_used`,
/// and every path actually skipped is *reported* alongside the digest — see
/// [`Walk::excluded`]. A digest that quietly skipped part of a tree would be
/// the same lie as one taken over a partially-readable one.
pub fn walk_excluding(root: &Path, exclude: &[&str]) -> Walk {
    let mut w = Walk {
        exclude: exclude.iter().map(|s| s.to_string()).collect(),
        ..Default::default()
    };
    match std::fs::symlink_metadata(root) {
        Ok(m) if m.is_dir() => w.present = true,
        Ok(_) => {
            // A path that is there but is not a directory. Present, and the
            // walk finds no files — said this way rather than as an absence.
            w.present = true;
            w.denied.push(". is not a directory".to_string());
            return w;
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return w,
        Err(e) => {
            // The root itself could not even be stat'ed. This is the exact
            // shape of the defect the header names: it is NOT "no files".
            w.present = true;
            w.denied.push(format!(". could not be read: {e}"));
            return w;
        }
    }
    let exclude: Vec<String> = w.exclude.clone();
    descend(root, Path::new(""), &exclude, &mut w);
    w
}

fn descend(abs: &Path, rel: &Path, exclude: &[String], w: &mut Walk) {
    let entries = match std::fs::read_dir(abs) {
        Ok(e) => e,
        Err(e) => {
            let shown = if rel.as_os_str().is_empty() {
                ".".to_string()
            } else {
                rel.display().to_string()
            };
            w.denied.push(format!("{shown} could not be read: {e}"));
            return;
        }
    };
    // Collected and sorted, so the manifest a digest is taken over does not
    // depend on the order the filesystem happened to return.
    let mut kids: Vec<PathBuf> = Vec::new();
    for entry in entries {
        match entry {
            Ok(e) => kids.push(e.path()),
            Err(e) => w.denied.push(format!(
                "an entry of {} could not be read: {e}",
                if rel.as_os_str().is_empty() {
                    ".".to_string()
                } else {
                    rel.display().to_string()
                }
            )),
        }
    }
    kids.sort();

    for path in kids {
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            w.denied
                .push(format!("{} holds a name that is not UTF-8", rel.display()));
            continue;
        };
        let child_rel = if rel.as_os_str().is_empty() {
            PathBuf::from(name)
        } else {
            rel.join(name)
        };
        let key = child_rel.display().to_string();
        // Matched against the whole root-relative path, so `.in_use` skips the
        // root entry and not a `tools/.in_use` deeper in the tree, and
        // `.in_use` still cannot match `not.in_used`. Recorded rather than
        // silently dropped.
        if exclude.iter().any(|e| e == &key) {
            w.excluded.push(key);
            continue;
        }
        let meta = match std::fs::symlink_metadata(&path) {
            Ok(m) => m,
            Err(e) => {
                w.denied.push(format!("{key} could not be read: {e}"));
                continue;
            }
        };
        if meta.file_type().is_symlink() {
            let target = std::fs::read_link(&path)
                .map(|t| t.display().to_string())
                .unwrap_or_else(|e| format!("<unreadable: {e}>"));
            w.links.insert(key, target);
            continue;
        }
        if meta.is_dir() {
            descend(&path, &child_rel, exclude, w);
            continue;
        }
        #[cfg(unix)]
        let exec = {
            use std::os::unix::fs::PermissionsExt;
            meta.permissions().mode() & 0o111 != 0
        };
        #[cfg(not(unix))]
        let exec = false;
        w.files.insert(key, exec);
    }
}

/// The digest of a walked tree.
///
/// The manifest is one line per file, sorted by relative path:
///
/// ```text
/// <per-file sha256>  <x|-> <relative path>
/// ```
///
/// and the digest is the sha256 of that text. The path and the execute bit are
/// inside the hash deliberately: a component that renames a script, or that
/// makes a documentation file executable, has changed what it will do, and a
/// digest over file *contents* alone would call both of those the same tree.
///
/// Symlinks contribute nothing to the hash and are reported separately by
/// [`Walk::links`]. A tree with any unreadable part gets no digest at all —
/// see the module header.
pub fn digest(root: &Path, w: &Walk) -> Digest {
    digest_with(root, w, &sha256sum())
}

/// [`digest`] with the hasher named explicitly.
///
/// Exists so the suite can force the [`Digest::Unmeasured`] arm by naming a
/// program that is not there, without mutating a process-wide environment
/// variable that the other tests in this binary are reading concurrently.
/// (That is not a hypothetical: the first version of these tests set
/// `APEX_SHA256SUM` and broke two unrelated tests running in parallel.)
pub fn digest_with(root: &Path, w: &Walk, prog: &str) -> Digest {
    if !w.present {
        return Digest::Unmeasured(format!("{} is not there", root.display()));
    }
    if !w.denied.is_empty() {
        return Digest::Unmeasured(format!(
            "{} of {} could not be read, so a hash over the rest would be a number that meant nothing: {}",
            w.denied.len(),
            root.display(),
            w.denied.join("; ")
        ));
    }
    if w.files.is_empty() {
        // Distinct from unreadable, and it is a real measurement: an empty tree
        // that stays empty has a stable digest, so this is the hash of the
        // empty manifest rather than a refusal.
        return hash_text_with(prog, "");
    }

    let names: Vec<&String> = w.files.keys().collect();
    let mut per_file: BTreeMap<&str, String> = BTreeMap::new();
    for chunk in names.chunks(CHUNK) {
        let mut cmd = Command::new(prog);
        cmd.arg("--");
        for n in chunk {
            cmd.arg(root.join(n.as_str()));
        }
        let out = match cmd.output() {
            Ok(o) => o,
            Err(e) => {
                return Digest::Unmeasured(format!("{prog} could not be run: {e}"))
            }
        };
        if !out.status.success() {
            return Digest::Unmeasured(format!(
                "{} refused {} of the files: {}",
                prog,
                chunk.len(),
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        let text = String::from_utf8_lossy(&out.stdout);
        // `sha256sum` prints "<hash>  <path>" in the order it was given, so the
        // digests are zipped back onto the chunk by position rather than by
        // parsing a path that may contain spaces.
        let lines: Vec<&str> = text.lines().collect();
        if lines.len() != chunk.len() {
            return Digest::Unmeasured(format!(
                "{} answered for {} files out of {}",
                prog,
                lines.len(),
                chunk.len()
            ));
        }
        for (name, line) in chunk.iter().zip(lines) {
            let Some(h) = line.split_whitespace().next() else {
                return Digest::Unmeasured(format!("{prog} printed a line with no hash"));
            };
            per_file.insert(name.as_str(), h.to_string());
        }
    }

    let mut manifest = String::new();
    for (name, exec) in &w.files {
        let h = match per_file.get(name.as_str()) {
            Some(h) => h,
            None => return Digest::Unmeasured(format!("no digest came back for {name}")),
        };
        manifest.push_str(h);
        manifest.push_str(if *exec { "  x " } else { "  - " });
        manifest.push_str(name);
        manifest.push('\n');
    }
    hash_text_with(prog, &manifest)
}

/// The sha256 of a string, via the same program.
fn hash_text_with(prog: &str, text: &str) -> Digest {
    let mut child = match Command::new(prog)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => return Digest::Unmeasured(format!("{prog} could not be run: {e}")),
    };
    if let Some(mut stdin) = child.stdin.take() {
        // A hasher that closed its input early must not become a hash.
        if let Err(e) = stdin.write_all(text.as_bytes()) {
            return Digest::Unmeasured(format!("{} would not take the manifest: {e}", prog));
        }
    }
    let out = match child.wait_with_output() {
        Ok(o) => o,
        Err(e) => return Digest::Unmeasured(format!("{} did not finish: {e}", prog)),
    };
    if !out.status.success() {
        return Digest::Unmeasured(format!(
            "{} exited {}: {}",
            prog,
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let Some(h) = text.split_whitespace().next() else {
        return Digest::Unmeasured(format!("{} printed nothing", prog));
    };
    if h.len() != 64 || !h.chars().all(|c| c.is_ascii_hexdigit()) {
        return Digest::Unmeasured(format!("{} printed '{h}', which is not a sha256", prog));
    }
    Digest::Sha256(h.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "apex-digest-{}-{tag}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("fixture");
        dir
    }

    #[test]
    fn a_missing_tree_and_an_unreadable_one_are_different_answers() {
        // The whole point of the module. Both are "no digest", and they say
        // different things, and neither is an empty tree.
        let root = fixture("absent");
        let missing = root.join("nope");
        let w = walk(&missing);
        assert!(!w.present, "a missing directory must not be 'present'");
        assert!(w.denied.is_empty(), "missing is not denied");
        let d = digest(&missing, &w);
        assert!(matches!(d, Digest::Unmeasured(ref why) if why.contains("is not there")), "{d:?}");

        // And an empty-but-readable one IS measured — it is a real fact.
        let empty = root.join("empty");
        std::fs::create_dir_all(&empty).expect("mkdir");
        let w = walk(&empty);
        assert!(w.present && w.complete());
        assert_eq!(w.file_count(), 0);
        assert!(digest(&empty, &w).is_measured(), "an empty readable tree has a digest");
        std::fs::remove_dir_all(&root).ok();
    }

    #[cfg(unix)]
    #[test]
    fn an_unreadable_subdirectory_removes_the_digest_rather_than_shrinking_it() {
        use std::os::unix::fs::PermissionsExt;
        // This is the fifteenth defect, prevented: a tree whose subdirectory
        // cannot be read must not hash as though that subdirectory were empty,
        // because that hash is a real-looking number over a partial tree.
        let root = fixture("denied");
        std::fs::write(root.join("SKILL.md"), "readable\n").expect("write");
        let shut = root.join("closed");
        std::fs::create_dir_all(&shut).expect("mkdir");
        std::fs::write(shut.join("secret.sh"), "#!/bin/sh\n").expect("write");

        let open = walk(&root);
        let open_digest = digest(&root, &open);
        assert!(open_digest.is_measured(), "{open_digest:?}");
        assert_eq!(open.file_count(), 2);

        std::fs::set_permissions(&shut, std::fs::Permissions::from_mode(0o000)).expect("chmod");
        let closed = walk(&root);
        // Running as root defeats a 0000 directory, so this asserts the
        // behaviour only when the mode actually took effect. Stated rather
        // than skipped silently: a check that quietly passes for root is the
        // "skipped counts as success" failure this repo has recorded.
        if std::fs::read_dir(&shut).is_err() {
            assert!(!closed.denied.is_empty(), "an unreadable subdir must be recorded");
            assert!(!closed.complete(), "the walk is not complete");
            let d = digest(&root, &closed);
            assert!(!d.is_measured(), "a partial tree must have NO digest, got {d:?}");
            assert_eq!(d.same_as(&open_digest), None, "unmeasured compares to neither");
        } else {
            eprintln!("note: 0000 did not deny this uid, so the denied arm was not exercised");
        }
        std::fs::set_permissions(&shut, std::fs::Permissions::from_mode(0o755)).ok();
        std::fs::remove_dir_all(&root).ok();
    }

    #[cfg(unix)]
    #[test]
    fn the_execute_bit_is_inside_the_hash() {
        use std::os::unix::fs::PermissionsExt;
        // A file that becomes executable has changed what the component can
        // do. A digest over contents alone would call the two trees identical,
        // which is exactly the label-not-provenance failure.
        let root = fixture("exec");
        let f = root.join("run.sh");
        std::fs::write(&f, "#!/bin/sh\necho hi\n").expect("write");
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o644)).expect("chmod");
        let before = digest(&root, &walk(&root));
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        let w = walk(&root);
        let after = digest(&root, &w);
        assert!(before.is_measured() && after.is_measured());
        assert_eq!(before.same_as(&after), Some(false), "chmod +x must change the digest");
        assert_eq!(w.executables(), vec!["run.sh"]);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_symlink_is_recorded_and_not_followed() {
        let root = fixture("link");
        std::fs::write(root.join("SKILL.md"), "x\n").expect("write");
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("/etc/passwd", root.join("sneaky")).expect("symlink");
            let w = walk(&root);
            assert_eq!(w.file_count(), 1, "the link is not a file of this tree");
            assert_eq!(w.links.get("sneaky").map(String::as_str), Some("/etc/passwd"));
        }
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn an_exclusion_skips_the_root_entry_only_and_never_hides_a_program() {
        // Two claims in the doc comment, both of which have been wrong in a
        // draft of this file:
        //   * the exclusion must not hide anything below a same-named
        //     directory deeper in the tree, or a script at
        //     `tools/.in_use/go.sh` would never be measured — and provenance
        //     is the hash of what runs;
        //   * a whole path component must match, so `.in_use` is not a prefix
        //     test that also eats `not.in_used`.
        let root = fixture("exclude");
        std::fs::write(root.join("plugin.json"), "{}\n").expect("write");
        std::fs::write(root.join("not.in_used"), "kept\n").expect("write");
        std::fs::create_dir_all(root.join(".in_use")).expect("mkdir");
        std::fs::write(root.join(".in_use/4242"), "").expect("write");
        std::fs::create_dir_all(root.join("tools/.in_use")).expect("mkdir");
        let hidden = root.join("tools/.in_use/go.sh");
        std::fs::write(&hidden, "#!/bin/sh\necho hi\n").expect("write");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&hidden, std::fs::Permissions::from_mode(0o755))
                .expect("chmod");
        }

        let w = walk_excluding(&root, &[".in_use"]);
        let files: Vec<&str> = w.files.keys().map(String::as_str).collect();

        // The root marker is skipped, and reported as skipped.
        assert!(
            !files.iter().any(|f| f.starts_with(".in_use/")),
            "the root runtime marker must be outside the digest: {files:?}"
        );
        assert_eq!(w.excluded, vec![".in_use".to_string()], "{:?}", w.excluded);

        // A whole component has to match.
        assert!(
            files.contains(&"not.in_used"),
            "the exclusion must match a whole path component: {files:?}"
        );

        // And nothing deeper is hidden — the hole this asserts is closed.
        assert!(
            files.contains(&"tools/.in_use/go.sh"),
            "an exclusion at the root must not hide a file deeper in the tree: {files:?}"
        );
        #[cfg(unix)]
        assert!(
            w.executables().contains(&"tools/.in_use/go.sh"),
            "and an executable hidden that way would never be reported: {:?}",
            w.executables()
        );

        // The digest still covers the rest, and it is a real measurement.
        assert!(digest(&root, &w).is_measured());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_hasher_that_cannot_be_run_is_unmeasured_and_never_a_hash() {
        // The tri-state, forced. `verify.rs`'s lesson in one assertion.
        let root = fixture("nohash");
        std::fs::write(root.join("a"), "a\n").expect("write");
        let w = walk(&root);
        // Named explicitly rather than through the environment: these tests
        // run in one process, in parallel, so a `set_var` here is a `set_var`
        // in every other test's digest too.
        let d = digest_with(&root, &w, "/nonexistent/apex-no-such-hasher");
        assert!(!d.is_measured(), "a missing hasher must not produce a hash: {d:?}");
        assert!(d.short() == "—", "{}", d.short());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn same_as_refuses_to_answer_when_either_side_is_unmeasured() {
        let a = Digest::Sha256("a".repeat(64));
        let b = Digest::Sha256("b".repeat(64));
        let u = Digest::Unmeasured("no".into());
        assert_eq!(a.same_as(&a), Some(true));
        assert_eq!(a.same_as(&b), Some(false));
        assert_eq!(a.same_as(&u), None);
        assert_eq!(u.same_as(&a), None);
        assert_eq!(u.same_as(&u), None, "two unmeasured trees are not thereby equal");
    }
}
