//! Persistent-state schema migration, and the answer each store gives to a
//! rollback. Roadmap §25.
//!
//! ## The problem an atomic OS does not solve
//!
//! `bootc rollback` swaps a pointer and reboots. `/usr` goes back; `/etc`,
//! `/var` and the user's home do not — that is the point of them. So the
//! sequence that breaks a machine is not an update, it is an update followed by
//! a rollback: the new build migrates a config, the user hits an unrelated bug,
//! rolls back, and the old build is now reading a file it has never seen the
//! shape of. The image rolled back; the state did not.
//!
//! §25 asks for five things per store — schema before, schema after, forward
//! migration, rollback strategy, checkpoint requirements — and this module is
//! where a store declares all five in one place instead of implying them in a
//! `#[serde(default)]` somewhere.
//!
//! ## Two honest answers to a rollback, and no third
//!
//! [`Rollback::ReadableByOlder`] and [`Rollback::RefuseAndExplain`]. There is
//! deliberately no down-migration variant: a down-migration is the right answer
//! only when the new schema drops information the old reader needs, and no
//! store in [`STORES`] does that. Adding the variant before a store needs it
//! would be machinery with no caller, which is the failure mode this framework
//! is most at risk of. When a store needs one, this enum grows an arm and the
//! store declares it.
//!
//! What each answer costs:
//!
//! - **`ReadableByOlder`** binds the migration to be additive. Keys may appear;
//!   nothing already there may change name, type or meaning. The claim is not
//!   taken on trust — [`additive_only`] replays the migration and checks that
//!   every key and value of the older document survives it unchanged, and the
//!   store's test calls it. It also binds the *reader*: the older build must
//!   ignore keys it does not know AND preserve them when it rewrites the file,
//!   or a rollback silently deletes the newer build's fields on the next save.
//! - **`RefuseAndExplain`** admits the older build cannot read the file, and
//!   makes it say so in those words. This is a property of code that already
//!   shipped, so it can only ever be arranged in advance: a build that refuses
//!   an unknown version is a build a future migration can rely on. The message
//!   has to name the file, both versions, and the checkpoint that holds the
//!   pre-migration copy, because a refusal a user cannot act on is a machine
//!   they have to reinstall.
//!
//! ## Checkpoints
//!
//! §25's "checkpoint requirements", and AGENTS.md's first rule for editing a
//! live configuration: back it up before touching it. A forward migration of a
//! machine-written store copies the file to `<path>.pre-v<from>` before it
//! writes, and leaves an existing checkpoint alone — the copy already there is
//! from the same version, and overwriting it after a rollback-then-forward
//! cycle would destroy the only original.
//!
//! ## Why a person's file is never rewritten
//!
//! A blueprint is TOML somebody typed, with comments and an order they chose. A
//! parse-and-reserialise loses both, and AGENTS.md prohibits silently
//! overwriting a user's edits. So [`Store::authored`] splits the two cases:
//! `Machine` stores migrate on disk, `Human` stores migrate **in memory** on
//! every read and the file changes only when the user next saves it. That is
//! also the better rollback story, for free: a file nobody rewrote is a file
//! the older build can still read.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

/// How a store's document is encoded on disk.
///
/// Both are read into a [`serde_json::Value`], which is the one document model
/// a migration step is written against. TOML's date-time type has no JSON
/// equivalent and would survive the round trip as a tagged map; no store here
/// uses one, and a store that wanted one would need this noted rather than
/// discovered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Toml,
    Json,
}

/// Who writes the file, which decides whether a migration may touch the disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Authored {
    /// A person types it. Migrated in memory on read; never rewritten here.
    Human,
    /// A program generates it. Migrated on disk, behind a checkpoint.
    Machine,
}

/// What an older build does when it meets a file this build wrote.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rollback {
    /// Every migration is additive and the older reader ignores what it does
    /// not know, so the file survives a rollback intact.
    ReadableByOlder,
    /// The older build cannot read it, and refuses with an explanation rather
    /// than reading it wrongly.
    RefuseAndExplain,
}

impl Rollback {
    pub fn as_str(self) -> &'static str {
        match self {
            Rollback::ReadableByOlder => "readable-by-older",
            Rollback::RefuseAndExplain => "refuse-and-explain",
        }
    }
}

/// One forward step between two adjacent schema versions.
///
/// `apply` receives the document's top-level table and mutates it. It never
/// sees the version key: [`migrate_text`] stamps that afterwards, so a step
/// cannot forget to, and cannot disagree with its own `to`.
pub struct Step {
    pub from: u32,
    pub to: u32,
    /// One line, in the past tense, naming what changed. It is printed to the
    /// user by `apex schema migrate`, so it says what happened to their file
    /// rather than what the code did.
    pub summary: &'static str,
    pub apply: fn(&mut Map<String, Value>) -> Result<(), String>,
}

impl std::fmt::Debug for Step {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Step({} -> {}: {})", self.from, self.to, self.summary)
    }
}

/// A step is identified by the transition it makes, not by the address of its
/// closure. Comparing function pointers would make two steps with identical
/// behaviour unequal, which is the wrong answer for a `Plan` a test asserts on.
impl PartialEq for Step {
    fn eq(&self, other: &Self) -> bool {
        self.from == other.from && self.to == other.to && self.summary == other.summary
    }
}
impl Eq for Step {}

/// One persistent store, and the five things §25 asks it to declare.
pub struct Store {
    /// Stable identifier. `apex schema status --json` keys on it and APEX
    /// Settings would too, so it is a compatibility surface: renaming one is
    /// the same class of change as renaming a `Health` row id.
    pub id: &'static str,
    /// What the file is, in one line, for somebody who has not read the code.
    pub what: &'static str,
    /// Where it lives, written the way a user would type it. Not resolved here
    /// — this crate resolves no paths, which is what lets every test point at a
    /// temp directory without going near `$XDG_STATE_HOME`.
    pub path: &'static str,
    pub format: Format,
    pub authored: Authored,
    /// The top-level key carrying the schema version.
    pub version_key: &'static str,
    /// What a document with no version key IS.
    ///
    /// The version that existed when the key was introduced — never "whatever
    /// this build is". Getting this wrong is silent and invisible until the
    /// second version exists, at which point every file written before the key
    /// claims to be current and is read with the wrong rules.
    pub unversioned: u32,
    /// The version this build writes.
    pub current: u32,
    pub rollback: Rollback,
    /// Forward steps, ascending, each `to` equal to the next `from`.
    pub steps: &'static [Step],
    /// A realistic document at [`Store::unversioned`], in this store's format.
    ///
    /// It is what the additive-only property runs against, and it is a field
    /// rather than a fixture because the first version of that check fed every
    /// store an EMPTY document. A step that renames a key never saw the key, so
    /// the check passed for a migration that broke the very claim it was
    /// written to prove — the same phantom pass this repository has now found
    /// four times. A store's sample must carry every key its oldest readers
    /// wrote, or the property is decorative again.
    pub sample: &'static str,
}

/// Every store this build knows how to migrate.
///
/// Three of them, and the report says which stores are NOT here: the agent
/// runtime's session records, both grant tables and both audit trails live in
/// crates that do not depend on `apexd-core`, so wiring them means either a new
/// dependency edge or moving this module. That is a decision, not an oversight,
/// and it belongs in front of whoever makes it rather than in a gap nobody
/// notices.
pub const STORES: &[Store] = &[
    Store {
        id: "blueprint",
        what: "the declarative blueprint: what this machine should be",
        path: "~/.config/apex/blueprint.toml",
        format: Format::Toml,
        // Hand-written, with comments. Migrated in memory on every read.
        authored: Authored::Human,
        version_key: "version",
        unversioned: 1,
        current: crate::blueprint::SCHEMA_VERSION,
        // `deny_unknown_fields`, because an unrecognised key in a file a person
        // typed is a typo they want told about. That same strictness is what
        // makes a rollback refuse rather than misread, so the honest answer
        // here is to refuse WELL: name the file, both versions, and the copy
        // that still has the old shape.
        rollback: Rollback::RefuseAndExplain,
        steps: &[],
        sample: "version = 1\n\n[desktop]\ncompositor = \"hyprland\"\ntheme = \"dark\"\n\n[apps]\ninstall = [\"neovim\", \"org.gimp.GIMP\"]\n\n[development]\nlanguages = [\"rust\"]\n\n[gaming]\nsteam = true\n",
    },
    Store {
        id: "tasks",
        what: "the task list: what you are working on and what each task is bound to",
        path: "~/.config/apex/tasks.toml",
        format: Format::Toml,
        // `apex task add` writes it, but nothing marks it generated and its
        // whole shape invites a hand edit, so it is treated as a person's file:
        // migrated in memory, never reserialised behind their back.
        authored: Authored::Human,
        version_key: "version",
        unversioned: 1,
        current: crate::task::SCHEMA_VERSION,
        // `deny_unknown_fields`, and `Tasks::validate` already refuses a
        // version above its own. §25 gave that refusal a remedy to name.
        rollback: Rollback::RefuseAndExplain,
        steps: &[],
        sample: "version = 1\n\n[task.ship-045]\ntitle = \"ship the migration framework\"\nproject = \"/home/a/apex\"\n",
    },
    Store {
        id: "blueprint-state",
        what: "the record `apex apply` leaves of the last convergence",
        path: "~/.local/state/apex/blueprint-state.toml",
        format: Format::Toml,
        authored: Authored::Machine,
        version_key: "schema",
        unversioned: 1,
        current: crate::blueprint::SCHEMA_VERSION,
        // Also `deny_unknown_fields`. Losing this file costs history and
        // nothing else, so refusing is cheap — but it has to refuse OUT LOUD.
        // It did not: nothing read the `schema` field it requires, and a record
        // from a newer build failed to parse into a `None` the caller reported
        // as "no apply has ever happened".
        rollback: Rollback::RefuseAndExplain,
        steps: &[],
        sample: "schema = 1\napplied_at = 1788700000\ndomain = \"user\"\nblueprint_digest = \"abc123\"\nsteps = [\"installed neovim\"]\nfailures = []\n",
    },
    Store {
        id: "task-state",
        what: "what has been observed about one task: created, last opened, last checkpoint",
        path: "~/.local/state/apex/tasks/<id>.json",
        format: Format::Json,
        authored: Authored::Machine,
        version_key: "schema",
        // Every task record on disk today has no `schema` key. Version 0 is
        // that shape, and it is a real version rather than a missing one.
        unversioned: 0,
        current: crate::task::STATE_SCHEMA_VERSION,
        rollback: Rollback::ReadableByOlder,
        steps: &[Step {
            from: 0,
            to: 1,
            summary: "stamped the schema version, so a later build can tell which rules to read it by",
            // Additive by construction: the version stamp is added by
            // `migrate_text`, and this step changes nothing else. The property
            // is asserted rather than asserted-in-a-comment — see
            // `additive_only`.
            apply: |_doc| Ok(()),
        }],
        // Every key a pre-§25 task record could carry. `checkpoint` and the
        // two timestamps are the whole of version 0.
        sample: "{\"created\": 1788600000, \"last_opened\": 1788700000, \"checkpoint\": \"1788439662000-a1b2c3d\"}",
    },
];

/// The store with this id, if this build knows it.
pub fn store(id: &str) -> Option<&'static Store> {
    STORES.iter().find(|s| s.id == id)
}

// ── reading a version without committing to a shape ──────────────────────────

/// Parse a document into its top-level table, whatever the format.
pub fn document(store: &Store, text: &str) -> Result<Map<String, Value>, String> {
    let v: Value = match store.format {
        Format::Toml => toml::from_str(text).map_err(|e| e.to_string())?,
        Format::Json => serde_json::from_str(text).map_err(|e| e.to_string())?,
    };
    match v {
        Value::Object(m) => Ok(m),
        _ => Err(format!("{} is not a table at the top level", store.path)),
    }
}

/// The schema version a document is on.
///
/// Deliberately a separate, shape-free read: the whole point is to learn the
/// version of a document this build may not be able to parse into its own
/// struct. A strict parse that fails first would answer "malformed" for a file
/// whose only problem is that it is from the future.
pub fn peek(store: &Store, text: &str) -> Result<u32, String> {
    let doc = document(store, text)?;
    version_of(store, &doc)
}

/// The version recorded in an already-parsed document.
pub fn version_of(store: &Store, doc: &Map<String, Value>) -> Result<u32, String> {
    match doc.get(store.version_key) {
        None => Ok(store.unversioned),
        Some(Value::Number(n)) => n
            .as_u64()
            .filter(|v| *v <= u32::MAX as u64)
            .map(|v| v as u32)
            .ok_or_else(|| {
                format!("{} = {n} is not a schema version", store.version_key)
            }),
        Some(other) => Err(format!(
            "{} = {other} is not a schema version",
            store.version_key
        )),
    }
}

// ── planning ─────────────────────────────────────────────────────────────────

/// What this build must do with a document at a given version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plan {
    /// Already the version this build writes.
    UpToDate,
    /// A chain of forward steps exists.
    Forward(Vec<&'static Step>),
    /// Written by a newer build. The rollback case, by definition: nothing
    /// else produces a version this build has never heard of.
    TooNew { found: u32, current: u32 },
    /// Older, and no chain of steps reaches the current version. A declared
    /// store with a hole in its history — a programming error, reported rather
    /// than papered over, because the alternative is reading a v1 document with
    /// v3 rules.
    NoRoute { found: u32, current: u32 },
}

pub fn plan(store: &Store, found: u32) -> Plan {
    if found == store.current {
        return Plan::UpToDate;
    }
    if found > store.current {
        return Plan::TooNew { found, current: store.current };
    }
    let mut at = found;
    let mut chain: Vec<&'static Step> = Vec::new();
    while at < store.current {
        match store.steps.iter().find(|s| s.from == at) {
            Some(step) => {
                at = step.to;
                chain.push(step);
            }
            None => return Plan::NoRoute { found, current: store.current },
        }
    }
    Plan::Forward(chain)
}

/// The refusal an older build owes a user whose file is from the future.
///
/// Every part of it is load-bearing. The path, because the user has more than
/// one config. Both versions, because "incompatible" does not say which
/// direction. The remedy, because the answer is almost always "boot the newer
/// deployment again", and a user who does not know that reinstalls.
pub fn too_new_message(store: &Store, path: &Path, found: u32, current: u32) -> String {
    let mut s = format!(
        "{}: schema {found}, and this build of APEX reads schema {current}.\n\
         This file was written by a newer APEX, so it is not read rather than \
         read wrongly.\n\
         The usual cause is a rollback: {} does not roll back with the image.",
        path.display(),
        store.what,
    );
    let ck = checkpoint_path(path, current);
    if ck.exists() {
        s.push_str(&format!(
            "\nA copy from before the migration is at {}.",
            ck.display()
        ));
    }
    s.push_str(
        "\nBoot the newer deployment again to use it, or move this file aside \
         to start from the defaults.",
    );
    s
}

/// One line saying what a `bootc rollback` would do to this store.
pub fn rollback_note(store: &Store) -> &'static str {
    match store.rollback {
        Rollback::ReadableByOlder => {
            "an older APEX still reads it: every migration only adds keys, and the reader keeps the ones it does not know"
        }
        Rollback::RefuseAndExplain => {
            "an older APEX refuses it by name and says so; nothing is misread, and nothing is lost"
        }
    }
}

// ── migrating ────────────────────────────────────────────────────────────────

/// What a migration did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub from: u32,
    pub to: u32,
    pub summaries: Vec<&'static str>,
    /// The migrated document, serialised in the store's own format.
    pub text: String,
}

/// Run a document forward to this build's schema, in memory.
///
/// Returns `Ok(None)` when nothing was needed, so a caller can tell "migrated"
/// from "already current" without comparing strings.
pub fn migrate_text(store: &Store, text: &str) -> Result<Option<Outcome>, String> {
    let mut doc = document(store, text)?;
    let found = version_of(store, &doc)?;
    let chain = match plan(store, found) {
        Plan::UpToDate => return Ok(None),
        Plan::Forward(c) => c,
        Plan::TooNew { found, current } => {
            return Err(format!(
                "schema {found} is newer than the schema {current} this build reads"
            ))
        }
        Plan::NoRoute { found, current } => {
            return Err(format!(
                "no migration reaches schema {current} from schema {found}"
            ))
        }
    };

    let mut summaries = Vec::new();
    for step in &chain {
        (step.apply)(&mut doc)
            .map_err(|e| format!("migrating {} from {} to {}: {e}", store.id, step.from, step.to))?;
        summaries.push(step.summary);
    }
    // The version stamp is written here rather than by a step, so a step
    // cannot forget it and cannot claim a version it did not reach.
    doc.insert(store.version_key.to_string(), Value::from(store.current));

    let out = serialise(store, &doc)?;

    // The precedent is `apex task`'s save path, which refuses to write a file
    // it cannot read back. A migration that produced something unparseable
    // would otherwise replace a readable old file with an unreadable new one,
    // which is the one outcome worse than not migrating.
    let back = document(store, &out)?;
    let landed = version_of(store, &back)?;
    if landed != store.current {
        return Err(format!(
            "the migrated document reads back as schema {landed}, not {}",
            store.current
        ));
    }

    Ok(Some(Outcome { from: found, to: store.current, summaries, text: out }))
}

fn serialise(store: &Store, doc: &Map<String, Value>) -> Result<String, String> {
    match store.format {
        Format::Toml => toml::to_string_pretty(doc).map_err(|e| e.to_string()),
        Format::Json => serde_json::to_string_pretty(doc)
            .map(|s| s + "\n")
            .map_err(|e| e.to_string()),
    }
}

/// Where the pre-migration copy of a file at schema `from` is kept.
pub fn checkpoint_path(path: &Path, from: u32) -> PathBuf {
    let mut s = path.as_os_str().to_os_string();
    s.push(format!(".pre-v{from}"));
    PathBuf::from(s)
}

/// What [`migrate_file`] did to a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileOutcome {
    /// No file there. Not an error: an unwritten store is a store at whatever
    /// version the next write gives it.
    Absent,
    UpToDate(u32),
    Migrated { outcome: Outcome, checkpoint: PathBuf },
    /// From a newer build. Carries the message the user is owed.
    TooNew { found: u32, message: String },
}

/// Run one machine-written file forward, keeping a copy of what it was.
///
/// Refuses a [`Authored::Human`] store outright. A person's TOML has comments
/// and an order they chose, and this function reserialises from a parsed
/// document — it would silently rewrite their file into the shape a serialiser
/// prefers. Human stores migrate in memory, inside their own parser.
pub fn migrate_file(store: &Store, path: &Path) -> Result<FileOutcome, String> {
    if store.authored == Authored::Human {
        return Err(format!(
            "{} is written by a person; it is migrated in memory on read, never rewritten here",
            store.id
        ));
    }
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(FileOutcome::Absent),
        // Anything else is a read that did not happen. Treating it as absent
        // is the EACCES mistake, and here it would go on to write a default
        // file over one it merely could not read.
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    let found = peek(store, &text)?;
    match plan(store, found) {
        Plan::UpToDate => return Ok(FileOutcome::UpToDate(found)),
        Plan::TooNew { found, current } => {
            return Ok(FileOutcome::TooNew {
                found,
                message: too_new_message(store, path, found, current),
            })
        }
        Plan::NoRoute { found, current } => {
            return Err(format!(
                "{}: no migration reaches schema {current} from schema {found}",
                path.display()
            ))
        }
        Plan::Forward(_) => {}
    }

    let outcome = match migrate_text(store, &text)? {
        Some(o) => o,
        None => return Ok(FileOutcome::UpToDate(found)),
    };

    // §25's checkpoint requirement. `copy` rather than `rename`, so the file
    // stays in place for anything reading it concurrently, and so the copy
    // inherits the original's mode — several of these are 0600.
    let checkpoint = checkpoint_path(path, found);
    if !checkpoint.exists() {
        std::fs::copy(path, &checkpoint)
            .map_err(|e| format!("writing {}: {e}", checkpoint.display()))?;
    }
    // An existing checkpoint is left alone deliberately. It is a copy of the
    // same version, and a rollback-then-forward cycle would otherwise overwrite
    // the original with a file the newer build had already touched.

    let tmp = {
        let mut s = path.as_os_str().to_os_string();
        s.push(format!(".migrate.{}", std::process::id()));
        PathBuf::from(s)
    };
    std::fs::write(&tmp, &outcome.text).map_err(|e| format!("writing {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("renaming into {}: {e}", path.display()))?;
    Ok(FileOutcome::Migrated { outcome, checkpoint })
}

// ── proving the rollback claim ───────────────────────────────────────────────

/// Whether running `store`'s migration over `text` only ADDS keys.
///
/// This is what makes [`Rollback::ReadableByOlder`] a checked property instead
/// of a hope. Every key present before must still be present after, with the
/// same value; the version stamp is the one permitted change, because a reader
/// that ignores unknown keys ignores that one too.
///
/// It walks nested tables, because a rename one level down is exactly the kind
/// of change that looks additive from the top.
pub fn additive_only(store: &Store, text: &str) -> Result<(), String> {
    let before = document(store, text)?;
    let after = match migrate_text(store, text)? {
        Some(o) => document(store, &o.text)?,
        None => return Ok(()),
    };
    compare(store, "", &before, &after)
}

fn compare(
    store: &Store,
    at: &str,
    before: &Map<String, Value>,
    after: &Map<String, Value>,
) -> Result<(), String> {
    for (k, v) in before {
        let here = if at.is_empty() { k.clone() } else { format!("{at}.{k}") };
        if at.is_empty() && k == store.version_key {
            continue;
        }
        let Some(now) = after.get(k) else {
            return Err(format!("{here} was removed, so an older build loses it"));
        };
        match (v, now) {
            (Value::Object(a), Value::Object(b)) => compare(store, &here, a, b)?,
            (a, b) if a == b => {}
            (a, b) => {
                return Err(format!(
                    "{here} changed from {a} to {b}, so an older build reads the wrong value"
                ))
            }
        }
    }
    Ok(())
}

// ── the report ───────────────────────────────────────────────────────────────

/// One store's situation on this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub id: &'static str,
    pub what: &'static str,
    pub path: String,
    /// The version on disk, or why it could not be read. `None` with no reason
    /// means there is no file.
    pub found: Option<u32>,
    pub reason: Option<String>,
    pub current: u32,
    pub rollback: Rollback,
    pub authored: Authored,
    pub plan: Option<Plan>,
}

/// Look at one store's file and say where it stands.
///
/// `path` is resolved by the caller: this crate resolves no paths, which is
/// what lets the tests run entirely inside a temp directory.
pub fn inspect(store: &'static Store, path: &Path) -> Row {
    let mut row = Row {
        id: store.id,
        what: store.what,
        path: path.display().to_string(),
        found: None,
        reason: None,
        current: store.current,
        rollback: store.rollback,
        authored: store.authored,
        plan: None,
    };
    match std::fs::read_to_string(path) {
        Ok(text) => match peek(store, &text) {
            Ok(found) => {
                row.found = Some(found);
                row.plan = Some(plan(store, found));
            }
            Err(e) => row.reason = Some(e),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        // A file this build could not read is not a file that is not there.
        Err(e) => row.reason = Some(format!("{e}")),
    }
    row
}

/// Every store's row, given a resolver from store id to path.
pub fn survey(paths: &BTreeMap<&str, PathBuf>) -> Vec<Row> {
    STORES
        .iter()
        .filter_map(|s| paths.get(s.id).map(|p| inspect(s, p)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A store that exists only here, to exercise a multi-step chain and a
    /// migration that changes content. Registering a fake in [`STORES`] to get
    /// test coverage would put a store in the product that nothing writes.
    static CHAIN: &[Step] = &[
        Step {
            from: 1,
            to: 2,
            summary: "added a count",
            apply: |d| {
                d.insert("count".into(), Value::from(0));
                Ok(())
            },
        },
        Step {
            from: 2,
            to: 3,
            summary: "added a label",
            apply: |d| {
                d.insert("label".into(), Value::from("none"));
                Ok(())
            },
        },
    ];

    static TESTSTORE: Store = Store {
        id: "test",
        what: "a store that exists only in this test module",
        path: "/nonexistent/test.json",
        format: Format::Json,
        authored: Authored::Machine,
        version_key: "schema",
        unversioned: 1,
        current: 3,
        rollback: Rollback::ReadableByOlder,
        steps: CHAIN,
        sample: r#"{"a": 1}"#,
    };

    #[test]
    fn every_declared_store_has_a_route_from_every_version_it_admits() {
        // A store that declares `current: 3` with a step chain that stops at 2
        // is a store whose users cannot be migrated, and the failure would be
        // a runtime `NoRoute` on somebody's machine rather than here.
        for s in STORES {
            for v in s.unversioned..s.current {
                assert!(
                    matches!(plan(s, v), Plan::Forward(_)),
                    "{}: no route from schema {v} to {}",
                    s.id,
                    s.current
                );
            }
            assert_eq!(plan(s, s.current), Plan::UpToDate, "{}", s.id);
            // Steps must chain, ascend, and not overshoot.
            let mut at = s.unversioned;
            for step in s.steps {
                assert_eq!(step.from, at, "{}: steps do not chain", s.id);
                assert!(step.to > step.from, "{}: a step goes backwards", s.id);
                at = step.to;
            }
            if !s.steps.is_empty() {
                assert_eq!(at, s.current, "{}: the chain does not reach current", s.id);
            }
            assert!(!s.summaries_empty(), "{}: a step has no summary", s.id);
        }
    }

    impl Store {
        fn summaries_empty(&self) -> bool {
            self.steps.iter().any(|s| s.summary.trim().is_empty())
        }
    }

    #[test]
    fn every_sample_is_a_real_document_at_the_version_it_claims() {
        // A sample that does not parse, or that sits at the wrong version,
        // makes `additive_only` test nothing while looking like it tests
        // something. That is the failure mode the sample field exists to fix,
        // so it gets its own guard rather than being trusted.
        for s in STORES {
            let doc = document(s, s.sample)
                .unwrap_or_else(|e| panic!("{}: its sample does not parse: {e}", s.id));
            assert!(!doc.is_empty(), "{}: its sample is an empty document", s.id);
            let v = version_of(s, &doc)
                .unwrap_or_else(|e| panic!("{}: its sample has no readable version: {e}", s.id));
            assert!(
                v == s.unversioned || v == s.current,
                "{}: its sample is at schema {v}, which is neither {} nor {}",
                s.id,
                s.unversioned,
                s.current
            );
            // And it must carry more than the version stamp, or a migration
            // that mangles a real key still has nothing to mangle.
            let content = doc.keys().filter(|k| *k != s.version_key).count();
            assert!(content > 0, "{}: its sample carries only a version", s.id);
        }
    }

    #[test]
    fn a_toml_document_survives_the_round_trip_the_framework_puts_it_through() {
        // Dead code today: every TOML store declares no steps, so `plan`
        // answers UpToDate and `serialise` is never reached. The first
        // blueprint migration would run `toml::to_string_pretty` over a
        // `serde_json::Map` for the first time on somebody's machine, and the
        // types are not obviously compatible — TOML has no null and its tables
        // must come after its scalars.
        static TOMLSTEPS: &[Step] = &[Step {
            from: 1,
            to: 2,
            summary: "added a nested table",
            apply: |d| {
                let mut inner = Map::new();
                inner.insert("nested".into(), Value::from("value"));
                d.insert("added".into(), Value::Object(inner));
                Ok(())
            },
        }];
        static TOMLSTORE: Store = Store {
            id: "toml-round-trip",
            what: "a TOML store that exists only in this test module",
            path: "/nonexistent/round-trip.toml",
            format: Format::Toml,
            authored: Authored::Machine,
            version_key: "version",
            unversioned: 1,
            current: 2,
            rollback: Rollback::ReadableByOlder,
            steps: TOMLSTEPS,
            sample: "version = 1\ntitle = \"a task\"\ncount = 3\nflags = [\"a\", \"b\"]\n\n[section]\nkey = \"v\"\n",
        };
        let o = migrate_text(&TOMLSTORE, TOMLSTORE.sample).unwrap().unwrap();
        let back = document(&TOMLSTORE, &o.text).unwrap();
        assert_eq!(back["version"], Value::from(2));
        assert_eq!(back["title"], Value::from("a task"));
        assert_eq!(back["count"], Value::from(3));
        assert_eq!(back["flags"], serde_json::json!(["a", "b"]));
        assert_eq!(back["section"]["key"], Value::from("v"));
        assert_eq!(back["added"]["nested"], Value::from("value"));
        // And the additive claim holds through a format that reorders keys.
        additive_only(&TOMLSTORE, TOMLSTORE.sample).unwrap();
    }

    #[test]
    fn store_ids_are_unique_and_stable_shaped() {
        let mut ids: Vec<&str> = STORES.iter().map(|s| s.id).collect();
        let n = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), n, "two stores share an id");
        for s in STORES {
            assert!(!s.id.is_empty() && !s.what.is_empty() && !s.path.is_empty());
            assert!(!s.version_key.is_empty());
            assert!(s.unversioned <= s.current, "{}", s.id);
        }
    }

    #[test]
    fn an_absent_version_key_is_the_version_it_meant_not_the_current_one() {
        // The defect this encodes: `Blueprint::version` was documented as
        // "absent means SCHEMA_VERSION", so the day SCHEMA_VERSION became 2
        // every file written before the key existed would have claimed to be
        // version 2 and been read with version 2's rules.
        let s = &TESTSTORE;
        assert_eq!(peek(s, r#"{"a": 1}"#).unwrap(), 1);
        assert_ne!(peek(s, r#"{"a": 1}"#).unwrap(), s.current);
    }

    #[test]
    fn a_chain_runs_every_step_in_order_and_stamps_the_version_once() {
        let o = migrate_text(&TESTSTORE, r#"{"a": 1}"#).unwrap().unwrap();
        assert_eq!(o.from, 1);
        assert_eq!(o.to, 3);
        assert_eq!(o.summaries, vec!["added a count", "added a label"]);
        let d = document(&TESTSTORE, &o.text).unwrap();
        assert_eq!(d["schema"], Value::from(3));
        assert_eq!(d["count"], Value::from(0));
        assert_eq!(d["label"], Value::from("none"));
        assert_eq!(d["a"], Value::from(1));
    }

    #[test]
    fn a_document_already_current_is_left_alone() {
        assert!(migrate_text(&TESTSTORE, r#"{"schema": 3}"#).unwrap().is_none());
    }

    #[test]
    fn a_document_from_the_future_is_refused_rather_than_read() {
        let e = migrate_text(&TESTSTORE, r#"{"schema": 9}"#).unwrap_err();
        assert!(e.contains("newer"), "{e}");
        assert_eq!(
            plan(&TESTSTORE, 9),
            Plan::TooNew { found: 9, current: 3 }
        );
    }

    #[test]
    fn a_hole_in_a_chain_is_a_refusal_not_a_guess() {
        static GAP: &[Step] = &[Step {
            from: 1,
            to: 2,
            summary: "one step",
            apply: |_| Ok(()),
        }];
        static HOLEY: Store = Store {
            id: "holey",
            what: "a store whose steps stop short",
            path: "/nonexistent/holey.json",
            format: Format::Json,
            authored: Authored::Machine,
            version_key: "schema",
            unversioned: 1,
            current: 4,
            rollback: Rollback::ReadableByOlder,
            steps: GAP,
            sample: r#"{"a": 1}"#,
        };
        assert_eq!(plan(&HOLEY, 1), Plan::NoRoute { found: 1, current: 4 });
        assert!(migrate_text(&HOLEY, r#"{"schema": 1}"#)
            .unwrap_err()
            .contains("no migration reaches"));
    }

    #[test]
    fn a_step_that_renames_a_key_fails_the_additive_claim() {
        // The property `ReadableByOlder` rests on, tested against a migration
        // that violates it. Without this, "additive" is a word in a comment.
        static RENAME: &[Step] = &[Step {
            from: 1,
            to: 2,
            summary: "renamed a key",
            apply: |d| {
                let v = d.remove("a").unwrap_or(Value::Null);
                d.insert("b".into(), v);
                Ok(())
            },
        }];
        static RENAMER: Store = Store {
            id: "renamer",
            what: "a store whose migration renames a key",
            path: "/nonexistent/rename.json",
            format: Format::Json,
            authored: Authored::Machine,
            version_key: "schema",
            unversioned: 1,
            current: 2,
            rollback: Rollback::ReadableByOlder,
            steps: RENAME,
            sample: r#"{"a": 1}"#,
        };
        let e = additive_only(&RENAMER, r#"{"a": 1}"#).unwrap_err();
        assert!(e.contains("was removed"), "{e}");
        // And the honest chain passes it.
        additive_only(&TESTSTORE, r#"{"a": 1}"#).unwrap();
    }

    #[test]
    fn a_rename_one_level_down_does_not_pass_as_additive() {
        static NESTED: &[Step] = &[Step {
            from: 1,
            to: 2,
            summary: "moved a nested key",
            apply: |d| {
                if let Some(Value::Object(inner)) = d.get_mut("inner") {
                    let v = inner.remove("x").unwrap_or(Value::Null);
                    inner.insert("y".into(), v);
                }
                Ok(())
            },
        }];
        static DEEP: Store = Store {
            id: "deep",
            what: "a store whose migration edits a nested table",
            path: "/nonexistent/deep.json",
            format: Format::Json,
            authored: Authored::Machine,
            version_key: "schema",
            unversioned: 1,
            current: 2,
            rollback: Rollback::ReadableByOlder,
            steps: NESTED,
            sample: r#"{"inner": {"x": 1}}"#,
        };
        let e = additive_only(&DEEP, r#"{"inner": {"x": 1}}"#).unwrap_err();
        assert!(e.contains("inner.x"), "{e}");
    }

    #[test]
    fn every_store_claiming_readable_by_older_is_actually_additive() {
        // The registry's own claim, checked against the registry's own steps.
        // A future store that declares ReadableByOlder and then renames a field
        // fails here rather than on a user's machine after a rollback.
        for s in STORES {
            if s.rollback != Rollback::ReadableByOlder {
                continue;
            }
            additive_only(s, s.sample)
                .unwrap_or_else(|e| panic!("{} declares readable-by-older, but {e}", s.id));
        }
    }

    #[test]
    fn a_human_written_store_is_never_rewritten_on_disk() {
        let bp = store("blueprint").unwrap();
        assert_eq!(bp.authored, Authored::Human);
        let e = migrate_file(bp, Path::new("/nonexistent/blueprint.toml")).unwrap_err();
        assert!(e.contains("written by a person"), "{e}");
    }

    #[test]
    fn a_file_migration_keeps_a_copy_of_what_the_file_was() {
        let dir = std::env::temp_dir().join(format!("apex-migrate-ck-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("test.json");
        std::fs::write(&p, "{\"a\": 1}\n").unwrap();

        let out = migrate_file(&TESTSTORE, &p).unwrap();
        let FileOutcome::Migrated { checkpoint, outcome } = out else {
            panic!("expected a migration, got {out:?}");
        };
        assert_eq!(outcome.from, 1);
        assert_eq!(checkpoint, checkpoint_path(&p, 1));
        // The checkpoint is the file as it WAS, not as it is.
        assert_eq!(std::fs::read_to_string(&checkpoint).unwrap(), "{\"a\": 1}\n");
        assert!(std::fs::read_to_string(&p).unwrap().contains("\"schema\": 3"));

        // Running again is a no-op that does not touch the checkpoint.
        assert_eq!(migrate_file(&TESTSTORE, &p).unwrap(), FileOutcome::UpToDate(3));
        assert_eq!(std::fs::read_to_string(&checkpoint).unwrap(), "{\"a\": 1}\n");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_existing_checkpoint_is_not_overwritten_by_a_second_migration() {
        // The rollback-then-forward cycle: a machine migrates, rolls back, the
        // older build rewrites the file, and it is migrated again. The
        // checkpoint must still hold the ORIGINAL, not the copy the older build
        // left behind.
        let dir = std::env::temp_dir().join(format!("apex-migrate-ck2-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("test.json");
        std::fs::write(&p, "{\"a\": 1}\n").unwrap();
        migrate_file(&TESTSTORE, &p).unwrap();

        // The older build's rewrite: same version, different content.
        std::fs::write(&p, "{\"a\": 2}\n").unwrap();
        migrate_file(&TESTSTORE, &p).unwrap();
        assert_eq!(
            std::fs::read_to_string(checkpoint_path(&p, 1)).unwrap(),
            "{\"a\": 1}\n",
            "the original checkpoint was overwritten"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_from_the_future_is_reported_with_a_message_a_user_can_act_on() {
        let dir = std::env::temp_dir().join(format!("apex-migrate-new-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("test.json");
        std::fs::write(&p, "{\"schema\": 99}\n").unwrap();
        let out = migrate_file(&TESTSTORE, &p).unwrap();
        let FileOutcome::TooNew { found, message } = out else {
            panic!("expected TooNew, got {out:?}");
        };
        assert_eq!(found, 99);
        // Path, both versions, cause, remedy.
        assert!(message.contains("test.json"), "{message}");
        assert!(message.contains("99") && message.contains('3'), "{message}");
        assert!(message.contains("rollback"), "{message}");
        assert!(message.contains("Boot the newer deployment"), "{message}");
        // And the file was NOT touched.
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "{\"schema\": 99}\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_that_could_not_be_read_is_not_a_file_that_is_not_there() {
        // Absent is a normal state and returns Ok(Absent). A refused read is
        // an error — collapsing them here would go on to migrate, and write, a
        // default over a file this process merely could not open.
        let dir = std::env::temp_dir().join(format!("apex-migrate-eacces-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("test.json");
        std::fs::write(&p, "{\"a\": 1}\n").unwrap();
        set_mode(&p, 0o000);
        let refused = std::fs::read_to_string(&p)
            .err()
            .map(|e| e.kind() == std::io::ErrorKind::PermissionDenied)
            .unwrap_or(false);
        if refused {
            let e = migrate_file(&TESTSTORE, &p).unwrap_err();
            assert!(e.contains("Permission denied"), "{e}");
        } else {
            eprintln!("skipping: this user reads a 0000 file (root or CAP_DAC_OVERRIDE)");
        }
        set_mode(&p, 0o600);
        assert_eq!(
            migrate_file(&TESTSTORE, &dir.join("nothing-here.json")).unwrap(),
            FileOutcome::Absent
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn set_mode(p: &Path, mode: u32) {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode));
    }

    #[test]
    fn a_version_key_that_is_not_a_number_is_refused() {
        assert!(peek(&TESTSTORE, r#"{"schema": "two"}"#).is_err());
        assert!(peek(&TESTSTORE, r#"{"schema": -1}"#).is_err());
        assert!(peek(&TESTSTORE, r#"{"schema": 1.5}"#).is_err());
    }

    #[test]
    fn both_rollback_answers_say_what_happens_and_neither_says_nothing() {
        for r in [Rollback::ReadableByOlder, Rollback::RefuseAndExplain] {
            let n = rollback_note(&Store { rollback: r, ..blueprint_shape() });
            assert!(n.len() > 40, "{n}");
            assert!(n.contains("older APEX"), "{n}");
        }
    }

    fn blueprint_shape() -> Store {
        Store {
            id: "shape",
            what: "shape",
            path: "shape",
            format: Format::Toml,
            authored: Authored::Human,
            version_key: "version",
            unversioned: 1,
            current: 1,
            rollback: Rollback::RefuseAndExplain,
            steps: &[],
            sample: "",
        }
    }

    #[test]
    fn inspect_tells_absent_from_unreadable_from_present() {
        let dir = std::env::temp_dir().join(format!("apex-migrate-insp-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let missing = dir.join("gone.json");
        let r = inspect(&TESTSTORE, &missing);
        assert_eq!(r.found, None);
        assert_eq!(r.reason, None);
        assert_eq!(r.plan, None);

        let p = dir.join("there.json");
        std::fs::write(&p, "{\"schema\": 1}").unwrap();
        let r = inspect(&TESTSTORE, &p);
        assert_eq!(r.found, Some(1));
        assert!(matches!(r.plan, Some(Plan::Forward(_))));

        let bad = dir.join("bad.json");
        std::fs::write(&bad, "{not json").unwrap();
        let r = inspect(&TESTSTORE, &bad);
        assert_eq!(r.found, None);
        assert!(r.reason.is_some(), "a malformed file reports a reason");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
