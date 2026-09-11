//! `apex skill` — P1-025: what skills this machine has, where each came from,
//! what it hashes to, and whether it can run code.
//!
//! ## What already existed, and why this is not a second opinion
//!
//! [`apex_agent_core::profile`] already knows the skills directory two ways:
//! as a read-only bind into a confined session, and as a `Class::Reusable`
//! entry that `apex agent profile export` carries into a bundle. Its `doctor`
//! counts skill directories and reports the ones with no `SKILL.md`.
//!
//! What none of that does is answer P1-025's question. `doctor` is a health
//! check — it says how many and which are broken. The bundle manifest records
//! `path`, `class`, `bytes` and `edited` **per file**, and nothing per skill:
//! no origin, no digest, and no record of whether the skill ships a program.
//! So a skill that arrived in a profile bundle from another machine, a skill a
//! plugin shipped, and a skill the user wrote are indistinguishable after the
//! fact, and a skill whose script changed under you hashes to nothing at all.
//!
//! This module adds the inventory and leaves `doctor` alone. The two agree by
//! construction on the one fact they share — a directory with no `SKILL.md`
//! will not load — and [`crate::skill`]'s own suite asserts that they do.
//!
//! ## The defect this module was written next to
//!
//! `profile.rs`'s skill scan opens with `if let Ok(it) = read_dir(&skills_dir)`
//! and iterates `it.flatten()`. Both discard errors: a skills directory that
//! exists and cannot be read reports **"0 skills"**, and an entry that cannot
//! be stat'ed vanishes from the count. `SKILL.md` presence is `.exists()`,
//! which is false for a file that is there and unreadable.
//!
//! That is the shape this repository has now found about fourteen times — a
//! refused read reported as a fact about the world — and an inventory is the
//! most damaging place for it, because the answer it gives is the one somebody
//! would rely on to say a machine has no unexpected skills on it. Every arm
//! below therefore distinguishes *cannot read* from *not there*, and
//! [`Inventory::problems`] treats an unreadable directory as a problem rather
//! than as an empty one.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::digest::{self, Digest, Walk};

/// The file that makes a directory a skill.
const MANIFEST: &str = "SKILL.md";

/// Where a skill came from.
///
/// P1-025's "origin", and it is a measurement rather than a record: nothing
/// writes a provenance file for a skill, so origin is derived from *which tree
/// it was found in*. That is weaker than a signed record and it is what the
/// filesystem can actually support — said here so nobody reads the field as
/// more than it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    /// `~/.claude/skills/<name>` — the user's own, available everywhere.
    User,
    /// `<dir>/.claude/skills/<name>` — travels with a repository.
    Project(PathBuf),
    /// A plugin's `skills/<name>`, replaced whenever the plugin updates.
    Plugin {
        plugin: String,
        marketplace: Option<String>,
    },
}

impl Origin {
    pub fn describe(&self) -> String {
        match self {
            Origin::User => "your own profile".to_string(),
            Origin::Project(dir) => format!("the repository at {}", dir.display()),
            Origin::Plugin {
                plugin,
                marketplace: Some(m),
            } => format!("the '{plugin}' plugin from {m}, replaced on every update"),
            Origin::Plugin {
                plugin,
                marketplace: None,
            } => format!("the '{plugin}' plugin, replaced on every update"),
        }
    }

    /// The one-word form for a table column.
    pub fn tag(&self) -> &'static str {
        match self {
            Origin::User => "user",
            Origin::Project(_) => "project",
            Origin::Plugin { .. } => "plugin",
        }
    }

    fn to_json(&self) -> Value {
        match self {
            Origin::User => json!({"kind": "user"}),
            Origin::Project(dir) => json!({"kind": "project", "directory": dir.display().to_string()}),
            Origin::Plugin { plugin, marketplace } => {
                json!({"kind": "plugin", "plugin": plugin, "marketplace": marketplace})
            }
        }
    }
}

/// What the manifest said, or why nothing could be said about it.
///
/// Three arms, not two. `Unreadable` is the one that matters: `.exists()`
/// answers false for a `SKILL.md` that is present and refused, so a two-valued
/// version of this would report such a skill as "has no manifest, will not
/// load" — a specific, confident, wrong claim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Manifest {
    Present {
        /// The `name:` in the frontmatter, when there is one.
        name: Option<String>,
        description: Option<String>,
    },
    /// The directory was read and holds no `SKILL.md`. The agent will not load
    /// it — this is the fact `profile.rs`'s `doctor` also reports.
    Absent,
    /// It is there and could not be read.
    Unreadable(String),
}

impl Manifest {
    pub fn loads(&self) -> Option<bool> {
        match self {
            Manifest::Present { .. } => Some(true),
            Manifest::Absent => Some(false),
            // Unknown, and deliberately not `false`.
            Manifest::Unreadable(_) => None,
        }
    }

    fn to_json(&self) -> Value {
        match self {
            Manifest::Present { name, description } => {
                json!({"present": true, "name": name, "description": description})
            }
            Manifest::Absent => json!({"present": false}),
            Manifest::Unreadable(why) => json!({"present": null, "why": why}),
        }
    }
}

/// Whether a skill can run code.
///
/// P1-025's second criterion in one enum. "Executable" here means exactly one
/// measurable thing — the skill ships at least one file with an execute bit —
/// and the word is not stretched further. A reference skill whose `SKILL.md`
/// tells the agent to run `python foo.py` is still `Reference` by this
/// measure, because the file it names is not executable and the agent's own
/// tool permissions, not this, are what let it run anything. That limit is
/// stated in the report so the distinction is not read as a security boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    /// Ships programs. The paths are listed so a reviewer can read them.
    Executable { scripts: Vec<String> },
    /// Documents and data only.
    Reference,
    /// The tree could not be fully read, so this is not known. Never
    /// `Reference` by default: "we could not look" must not render as "it is
    /// only documentation".
    Undetermined(String),
}

impl Kind {
    pub fn tag(&self) -> &'static str {
        match self {
            Kind::Executable { .. } => "executable",
            Kind::Reference => "reference",
            Kind::Undetermined(_) => "unknown",
        }
    }

    fn to_json(&self) -> Value {
        match self {
            Kind::Executable { scripts } => json!({"kind": "executable", "scripts": scripts}),
            Kind::Reference => json!({"kind": "reference", "scripts": []}),
            Kind::Undetermined(why) => json!({"kind": null, "why": why}),
        }
    }
}

/// One skill.
#[derive(Debug, Clone)]
pub struct Skill {
    pub name: String,
    pub path: PathBuf,
    pub origin: Origin,
    pub manifest: Manifest,
    pub kind: Kind,
    pub digest: Digest,
    /// Files under the skill, excluding symlinks.
    pub files: usize,
    /// Symlinks, by relative path → target as written. Not followed.
    pub links: BTreeMap<String, String>,
    /// Directories inside the skill that could not be read.
    pub denied: Vec<String>,
}

impl Skill {
    fn to_json(&self) -> Value {
        json!({
            "name": self.name,
            "path": self.path.display().to_string(),
            "origin": self.origin.to_json(),
            "manifest": self.manifest.to_json(),
            "type": self.kind.to_json(),
            "digest": self.digest.to_json(),
            "files": self.files,
            "symlinks": self.links,
            "unreadable": self.denied,
        })
    }
}

/// Every skill this machine has, and every tree that could not be read.
#[derive(Debug, Clone, Default)]
pub struct Inventory {
    pub skills: Vec<Skill>,
    /// A skills *root* that exists and could not be read.
    ///
    /// Separate from a per-skill refusal, and the reason the whole module
    /// exists: this list being non-empty means the inventory is incomplete, and
    /// an incomplete inventory must never be printed as a complete one.
    pub unreadable_roots: Vec<String>,
    /// Roots that were looked at, whether or not they held anything.
    pub roots: Vec<String>,
}

impl Inventory {
    /// Whether every root was read. A caller may not report a count as
    /// authoritative when this is false.
    pub fn complete(&self) -> bool {
        self.unreadable_roots.is_empty()
    }

    /// The audit's findings, worst first.
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        for root in &self.unreadable_roots {
            out.push(format!(
                "{root} exists and could not be read, so this inventory is INCOMPLETE — \
                 it is not a machine with no skills there"
            ));
        }
        for s in &self.skills {
            for d in &s.denied {
                out.push(format!(
                    "{}: {d} — the skill is only partly measured and has no digest",
                    s.name
                ));
            }
            match &s.manifest {
                Manifest::Absent => out.push(format!(
                    "{}: no {MANIFEST}, so the agent will not load it",
                    s.name
                )),
                Manifest::Unreadable(why) => out.push(format!(
                    "{}: its {MANIFEST} could not be read ({why}), so whether it loads is unknown",
                    s.name
                )),
                Manifest::Present { .. } => {}
            }
            for (link, target) in &s.links {
                // The same security fact `apex-plugin`'s `enumerate` counts,
                // for the same reason: a link out of the tree makes the skill's
                // content something other than what is in its directory, and
                // the digest cannot cover it.
                if leaves_tree(target) {
                    out.push(format!(
                        "{}: {link} is a symlink to {target}, which is outside the skill — \
                         its content is not what this directory holds and no digest covers it",
                        s.name
                    ));
                }
            }
            if let Digest::Unmeasured(why) = &s.digest {
                out.push(format!("{}: no digest — {why}", s.name));
            }
        }
        out
    }
}

/// Whether a symlink target points out of the skill's own directory.
///
/// Textual and deliberately conservative: absolute, or any `..` component. A
/// target that resolves back inside by a route this does not model is reported
/// anyway, because over-reporting a link is a note and under-reporting one is
/// the defect.
fn leaves_tree(target: &str) -> bool {
    target.starts_with('/')
        || target.starts_with("~")
        || Path::new(target)
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
}

/// The user's profile root, `~/.claude`.
fn profile_root(home: &Path) -> PathBuf {
    home.join(".claude")
}

/// Read `name:` and `description:` out of a `SKILL.md`.
///
/// A three-line YAML reader, not a YAML parser, and that is on purpose: the
/// frontmatter this needs is two scalar keys at the top level of a block
/// fenced by `---`. Taking a YAML dependency to read two strings would be the
/// choice `verify.rs` declined for its date arithmetic. What this does NOT
/// handle is stated rather than silently mishandled: block scalars, quoted
/// multi-line values and nested keys are skipped, and a key it cannot read
/// simply comes back `None` — never a wrong value.
fn frontmatter(text: &str) -> (Option<String>, Option<String>) {
    let mut lines = text.lines();
    if lines.next().map(str::trim) != Some("---") {
        return (None, None);
    }
    let mut name = None;
    let mut description = None;
    for line in lines {
        let t = line.trim_end();
        if t.trim() == "---" {
            break;
        }
        // Top level only: an indented line belongs to a nested key such as
        // `metadata:`, and treating `  name:` under it as the skill's name
        // would be a wrong answer rather than a missing one.
        if t.starts_with(' ') || t.starts_with('\t') {
            continue;
        }
        let Some((key, value)) = t.split_once(':') else {
            continue;
        };
        let value = value.trim().trim_matches('"').trim_matches('\'').to_string();
        if value.is_empty() {
            continue;
        }
        match key.trim() {
            "name" if name.is_none() => name = Some(value),
            "description" if description.is_none() => description = Some(value),
            _ => {}
        }
    }
    (name, description)
}

/// Measure one skill directory.
fn measure(name: &str, path: &Path, origin: Origin) -> Skill {
    let w: Walk = digest::walk(path);
    let digest = digest::digest(path, &w);

    let manifest_path = path.join(MANIFEST);
    let manifest = match std::fs::read_to_string(&manifest_path) {
        Ok(text) => {
            let (n, d) = frontmatter(&text);
            Manifest::Present {
                name: n,
                description: d,
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Manifest::Absent,
        // Present and refused. THIS is the arm `.exists()` cannot express.
        Err(e) => Manifest::Unreadable(e.to_string()),
    };

    let kind = if !w.complete() {
        Kind::Undetermined(format!(
            "{} of the directory could not be read",
            w.denied.len().max(1)
        ))
    } else {
        let scripts: Vec<String> = w.executables().iter().map(|s| s.to_string()).collect();
        if scripts.is_empty() {
            Kind::Reference
        } else {
            Kind::Executable { scripts }
        }
    };

    Skill {
        name: name.to_string(),
        path: path.to_path_buf(),
        origin,
        manifest,
        kind,
        digest,
        files: w.file_count(),
        links: w.links.clone(),
        denied: w.denied.clone(),
    }
}

/// Every skill directory under one root, or the fact that the root is closed.
fn collect_root(root: &Path, origin: impl Fn(&str) -> Origin, into: &mut Inventory) {
    into.roots.push(root.display().to_string());
    let entries = match std::fs::read_dir(root) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
        Err(e) => {
            // The defect, refused. `profile.rs` discards exactly this.
            into.unreadable_roots
                .push(format!("{} ({e})", root.display()));
            return;
        }
    };
    let mut dirs: Vec<PathBuf> = Vec::new();
    for entry in entries {
        match entry {
            Ok(e) => dirs.push(e.path()),
            Err(e) => into
                .unreadable_roots
                .push(format!("an entry of {} ({e})", root.display())),
        }
    }
    dirs.sort();
    for path in dirs {
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        // A skill is a directory. A stray file is not a skill and not a
        // problem; a directory whose type cannot be read IS recorded.
        match std::fs::symlink_metadata(&path) {
            Ok(m) if m.is_dir() => {}
            Ok(m) if m.file_type().is_symlink() => {
                // A symlinked skill directory. `profile.rs`'s export already
                // treats one as not-exportable rather than following it; here
                // it is measured where it points but reported as a link, so an
                // inventory of "what this machine has" does not silently claim
                // a tree that lives somewhere else.
                match std::fs::metadata(&path) {
                    Ok(m2) if m2.is_dir() => {}
                    _ => continue,
                }
            }
            Ok(_) => continue,
            Err(e) => {
                into.unreadable_roots
                    .push(format!("{} ({e})", path.display()));
                continue;
            }
        }
        into.skills.push(measure(name, &path, origin(name)));
    }
}

/// The whole inventory: the user's skills, a project's, and every enabled
/// plugin's.
pub fn inventory(home: &Path, cwd: Option<&Path>) -> Inventory {
    let mut inv = Inventory::default();

    collect_root(
        &profile_root(home).join("skills"),
        |_| Origin::User,
        &mut inv,
    );

    if let Some(dir) = cwd {
        let project = dir.join(".claude/skills");
        if project.exists() || std::fs::symlink_metadata(&project).is_ok() {
            let owner = dir.to_path_buf();
            collect_root(&project, |_| Origin::Project(owner.clone()), &mut inv);
        }
    }

    // A plugin may ship skills. None on this machine does today, and a loop
    // that only ran when it found something would be a loop nobody noticed was
    // wrong — so the roots are recorded either way and a closed one is a
    // problem here exactly as it is for the user's own.
    for (plugin, mcp_file) in crate::mcp::servers::enabled_plugin_configs(home) {
        let Some(install) = mcp_file.parent() else {
            continue;
        };
        let skills = install.join("skills");
        // Absent is a reason to skip; refused is not. `.exists()` cannot tell
        // them apart, and skipping on a refusal would drop the plugin's skills
        // out of the inventory *and* out of `unreadable_roots` — a silent zero,
        // which is the one thing this module exists to refuse to say.
        // `collect_root` records the refusal, so it is handed the path.
        match std::fs::symlink_metadata(&skills) {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => {}
        }
        let marketplace = marketplace_of(home, &plugin);
        collect_root(
            &skills,
            |_| Origin::Plugin {
                plugin: plugin.clone(),
                marketplace: marketplace.clone(),
            },
            &mut inv,
        );
    }

    inv.skills.sort_by(|a, b| a.name.cmp(&b.name));
    inv
}

/// The marketplace an enabled plugin came from, from the key's `name@market`.
fn marketplace_of(home: &Path, plugin: &str) -> Option<String> {
    let settings = crate::mcp::servers::read_json(&home.join(".claude/settings.json"))?;
    let enabled = settings.get("enabledPlugins")?.as_object()?;
    for key in enabled.keys() {
        if let Some((name, market)) = key.rsplit_once('@') {
            if name == plugin {
                return Some(market.to_string());
            }
        }
    }
    None
}

pub fn to_json(inv: &Inventory) -> Value {
    json!({
        "skills": inv.skills.iter().map(Skill::to_json).collect::<Vec<_>>(),
        "count": inv.skills.len(),
        "executable": inv.skills.iter().filter(|s| matches!(s.kind, Kind::Executable { .. })).count(),
        "reference": inv.skills.iter().filter(|s| s.kind == Kind::Reference).count(),
        "roots": inv.roots,
        "unreadable_roots": inv.unreadable_roots,
        "complete": inv.complete(),
        "problems": inv.problems(),
    })
}

fn render(inv: &Inventory) -> String {
    let mut out = String::new();
    if inv.skills.is_empty() {
        if inv.complete() {
            out.push_str("no skills on this machine\n");
        } else {
            // The distinction, printed. A count of zero from an incomplete
            // walk is the one thing this module must never say plainly.
            out.push_str("no skills were READ, and the inventory is incomplete:\n");
            for r in &inv.unreadable_roots {
                out.push_str(&format!("  {r}\n"));
            }
        }
        for r in &inv.roots {
            out.push_str(&format!("looked in {r}\n"));
        }
        return out;
    }

    out.push_str(&format!(
        "{:<28}  {:<8}  {:<11}  {:<13}  {}\n",
        "SKILL", "ORIGIN", "TYPE", "DIGEST", "LOADS"
    ));
    for s in &inv.skills {
        let loads = match s.manifest.loads() {
            Some(true) => "yes".to_string(),
            Some(false) => format!("NO ({MANIFEST} missing)"),
            None => "unknown".to_string(),
        };
        out.push_str(&format!(
            "{:<28}  {:<8}  {:<11}  {:<13}  {}\n",
            s.name,
            s.origin.tag(),
            s.kind.tag(),
            s.digest.short(),
            loads
        ));
    }

    let exec = inv
        .skills
        .iter()
        .filter(|s| matches!(s.kind, Kind::Executable { .. }))
        .count();
    let unknown = inv
        .skills
        .iter()
        .filter(|s| matches!(s.kind, Kind::Undetermined(_)))
        .count();
    out.push('\n');
    out.push_str(&format!(
        "{} skills: {exec} ship a program, {} are documentation only",
        inv.skills.len(),
        inv.skills.len() - exec - unknown
    ));
    if unknown > 0 {
        out.push_str(&format!(", {unknown} could not be determined"));
    }
    out.push_str(".\n");
    if !inv.complete() {
        out.push_str("\nThis inventory is INCOMPLETE — a skills directory could not be read:\n");
        for r in &inv.unreadable_roots {
            out.push_str(&format!("  {r}\n"));
        }
    }
    // Said every time, like `apex plugin info`'s note, and for the same
    // reason: "executable" here is a measurement of the execute bit, not a
    // statement that anything confines what runs.
    out.push_str(
        "\n'executable' means the skill ships a file with an execute bit. Nothing here\n\
         confines a skill: its scripts run as you, in whatever the session's own sandbox\n\
         is. The digest covers this directory's files and their execute bits, so a\n\
         changed script changes it — it is not a signature and nobody countersigned it.\n",
    );
    out
}

/// `apex skill list`
pub fn list(json: bool) -> i32 {
    let home = crate::mcp::home();
    let cwd = std::env::current_dir().ok();
    let inv = inventory(&home, cwd.as_deref());
    if json {
        println!("{:#}", to_json(&inv));
    } else {
        print!("{}", render(&inv));
    }
    // An incomplete inventory is not a successful one, even from `list`.
    if inv.complete() {
        0
    } else {
        1
    }
}

/// `apex skill audit`
///
/// Exits non-zero when there is a problem, which is the convention
/// `test-agent-profile.sh` already holds `apex agent profile doctor` to.
pub fn audit(json: bool) -> i32 {
    let home = crate::mcp::home();
    let cwd = std::env::current_dir().ok();
    let inv = inventory(&home, cwd.as_deref());
    let problems = inv.problems();
    if json {
        println!(
            "{:#}",
            json!({
                "problems": problems,
                "complete": inv.complete(),
                "count": inv.skills.len(),
                "executable": inv.skills.iter()
                    .filter(|s| matches!(s.kind, Kind::Executable { .. }))
                    .map(|s| json!({"name": s.name, "scripts": match &s.kind {
                        Kind::Executable { scripts } => scripts.clone(),
                        _ => vec![],
                    }}))
                    .collect::<Vec<_>>(),
            })
        );
    } else {
        if problems.is_empty() {
            println!("{} skills, no problems", inv.skills.len());
        } else {
            println!("{} problem(s):", problems.len());
            for p in &problems {
                println!("  {p}");
            }
        }
        // Then what can execute, and where each one came from — printed
        // whether or not there were problems, which it was not before. A
        // machine WITH a problem is exactly where somebody needs the list of
        // things that can run on it, and the old shape hid that list behind a
        // clean bill of health.
        //
        // The origin in full, not `list`'s one-word column, because the word
        // cannot say the thing that decides what to do: a skill that came from
        // a plugin is REPLACED whenever the plugin updates, so editing it in
        // place does not survive, while one in your own profile is yours.
        for s in &inv.skills {
            if let Kind::Executable { scripts } = &s.kind {
                println!(
                    "  {} ships {}, from {}: {}",
                    s.name,
                    scripts.len(),
                    s.origin.describe(),
                    scripts.join(" ")
                );
            }
        }
    }
    i32::from(!problems.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "apex-skill-{}-{tag}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("fixture");
        dir
    }

    fn skill(home: &Path, name: &str, body: &str) -> PathBuf {
        let dir = home.join(".claude/skills").join(name);
        std::fs::create_dir_all(&dir).expect("mkdir");
        std::fs::write(dir.join(MANIFEST), body).expect("write");
        dir
    }

    #[test]
    fn an_executable_skill_is_distinguishable_from_a_reference_skill() {
        // P1-025's second criterion, as an assertion.
        let home = fixture("kind");
        skill(&home, "reference-only", "---\nname: reference-only\n---\ndocs\n");
        let runner = skill(&home, "has-a-script", "---\nname: has-a-script\n---\nrun it\n");
        let script = runner.join("scripts/go.sh");
        std::fs::create_dir_all(script.parent().unwrap()).expect("mkdir");
        std::fs::write(&script, "#!/bin/sh\necho hi\n").expect("write");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        }

        let inv = inventory(&home, None);
        assert_eq!(inv.skills.len(), 2, "{:?}", inv.skills);
        assert!(inv.complete());
        let by = |n: &str| inv.skills.iter().find(|s| s.name == n).expect("skill").clone();

        assert_eq!(by("reference-only").kind, Kind::Reference);
        #[cfg(unix)]
        assert_eq!(
            by("has-a-script").kind,
            Kind::Executable {
                scripts: vec!["scripts/go.sh".to_string()]
            }
        );
        // And both carry the other two pieces of metadata the criterion asks
        // for: an origin and a digest of what is on disk.
        assert_eq!(by("reference-only").origin, Origin::User);
        assert!(by("reference-only").digest.is_measured());
        assert!(by("has-a-script").digest.is_measured());
        std::fs::remove_dir_all(&home).ok();
    }

    #[cfg(unix)]
    #[test]
    fn an_unreadable_skills_directory_is_not_a_machine_with_no_skills() {
        use std::os::unix::fs::PermissionsExt;
        // The fifteenth defect, refused. `profile.rs`'s `doctor` reports "0
        // skills" for this exact tree; this must not.
        let home = fixture("denied-root");
        skill(&home, "one", "---\nname: one\n---\n");
        let root = home.join(".claude/skills");
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o000)).expect("chmod");
        if std::fs::read_dir(&root).is_err() {
            let inv = inventory(&home, None);
            assert!(inv.skills.is_empty(), "nothing could be read");
            assert!(!inv.complete(), "an unreadable root must make the inventory incomplete");
            assert_eq!(inv.unreadable_roots.len(), 1, "{:?}", inv.unreadable_roots);
            let problems = inv.problems();
            assert!(
                problems.iter().any(|p| p.contains("INCOMPLETE")),
                "{problems:?}"
            );
            // The rendered form must not read as an empty machine.
            let text = render(&inv);
            assert!(text.contains("no skills were READ"), "{text}");
            assert!(!text.contains("no skills on this machine"), "{text}");
        } else {
            eprintln!("note: 0000 did not deny this uid; the denied arm was not exercised");
        }
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755)).ok();
        std::fs::remove_dir_all(&home).ok();
    }

    #[cfg(unix)]
    #[test]
    fn a_manifest_that_cannot_be_read_is_not_a_manifest_that_is_absent() {
        use std::os::unix::fs::PermissionsExt;
        let home = fixture("denied-manifest");
        let dir = skill(&home, "shy", "---\nname: shy\n---\n");
        let m = dir.join(MANIFEST);
        std::fs::set_permissions(&m, std::fs::Permissions::from_mode(0o000)).expect("chmod");
        if std::fs::read_to_string(&m).is_err() {
            let inv = inventory(&home, None);
            let s = &inv.skills[0];
            assert!(
                matches!(s.manifest, Manifest::Unreadable(_)),
                "{:?}",
                s.manifest
            );
            // `.exists()` would have said "Absent" and therefore "will not
            // load". The honest answer is that it is not known.
            assert_eq!(s.manifest.loads(), None);
            let problems = inv.problems();
            assert!(problems.iter().any(|p| p.contains("unknown")), "{problems:?}");
        } else {
            eprintln!("note: 0000 did not deny this uid; the arm was not exercised");
        }
        std::fs::set_permissions(&m, std::fs::Permissions::from_mode(0o644)).ok();
        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn a_directory_with_no_manifest_agrees_with_what_doctor_reports() {
        // The one fact this module and `profile.rs`'s `doctor` share. If they
        // ever disagree, one of them is telling a user their skill loads when
        // it does not.
        let home = fixture("nomanifest");
        std::fs::create_dir_all(home.join(".claude/skills/bare")).expect("mkdir");
        std::fs::write(home.join(".claude/skills/bare/notes.md"), "x\n").expect("write");
        let inv = inventory(&home, None);
        assert_eq!(inv.skills.len(), 1);
        assert_eq!(inv.skills[0].manifest, Manifest::Absent);
        assert_eq!(inv.skills[0].manifest.loads(), Some(false));
        assert!(
            inv.problems().iter().any(|p| p.contains("will not load")),
            "{:?}",
            inv.problems()
        );
        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn the_digest_changes_when_a_script_changes_and_not_otherwise() {
        // Provenance means the hash of what runs. Two runs over an untouched
        // tree must agree, and an edited script must not.
        let home = fixture("stable");
        let dir = skill(&home, "s", "---\nname: s\n---\n");
        std::fs::write(dir.join("go.sh"), "#!/bin/sh\necho one\n").expect("write");
        let first = inventory(&home, None).skills[0].digest.clone();
        let again = inventory(&home, None).skills[0].digest.clone();
        assert_eq!(first.same_as(&again), Some(true), "an untouched tree must hash the same");
        std::fs::write(dir.join("go.sh"), "#!/bin/sh\necho two\n").expect("write");
        let changed = inventory(&home, None).skills[0].digest.clone();
        assert_eq!(
            first.same_as(&changed),
            Some(false),
            "an edited script must change the digest"
        );
        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn frontmatter_reads_the_two_keys_and_refuses_to_guess_at_nested_ones() {
        let (n, d) = frontmatter(
            "---\nname: resume-guard\ndescription: Survive an interrupted run.\nmetadata:\n  name: NOT-THIS\n---\n\n# body\n",
        );
        assert_eq!(n.as_deref(), Some("resume-guard"));
        assert_eq!(d.as_deref(), Some("Survive an interrupted run."));

        // No frontmatter at all is None, not an error and not a guess.
        let (n, d) = frontmatter("# just a heading\n");
        assert_eq!((n, d), (None, None));
    }

    #[test]
    fn a_symlink_out_of_the_skill_is_reported_as_content_the_digest_cannot_cover() {
        let home = fixture("link");
        let dir = skill(&home, "leaky", "---\nname: leaky\n---\n");
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("/etc/passwd", dir.join("data")).expect("symlink");
            let inv = inventory(&home, None);
            let problems = inv.problems();
            assert!(
                problems.iter().any(|p| p.contains("outside the skill")),
                "{problems:?}"
            );
            assert_eq!(inv.skills[0].links.len(), 1);
        }
        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn leaves_tree_catches_the_shapes_that_matter() {
        assert!(leaves_tree("/etc/passwd"));
        assert!(leaves_tree("../../secrets"));
        assert!(leaves_tree("~/notes"));
        assert!(!leaves_tree("scripts/go.sh"));
        assert!(!leaves_tree("./inner"));
    }
}
