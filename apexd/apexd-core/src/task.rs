//! Tasks (roadmap §21): the schema, the validation, and the pure resume
//! planner.
//!
//! §21 — "make APEX understand intent" — asks for a system that models what a
//! person is actually doing, and gives one example:
//!
//! ```text
//! Task: Fix APEX installer bug
//!   Project:     apex-os
//!   Environment: Fedora build capsule
//!   Windows:     editor, browser, logs
//!   Agents:      Claude, Codex reviewer
//!   Checkpoint:  before changes
//!   Permissions: project files, GitHub apex-os, network
//! ```
//!
//! Seven of the eight concepts it then lists already ship: projects (§6,
//! `apex project`), agents (§2, `apex agent`), environments (§8 capsules,
//! `apex env`), worktrees (§7, `apex project worktrees`), checkpoints (§5,
//! `apex agent checkpoint`), capabilities and permissions (§4, `apex request`
//! and the secret broker), trusted devices (§20, `apex host`). The one that
//! does not is **Task** — the binder that references the others and can be put
//! down and picked back up.
//!
//! So this module is one record type. It creates no second project system, no
//! second checkpoint store and no second permission model; a task *names*
//! things that exist elsewhere, and everything a task can be asked about is
//! either in the record or observed from the thing it names. Nothing here
//! performs I/O.
//!
//! ── Where a task lives, and who writes it ───────────────────────────────────
//!
//! Two files, because a task genuinely has two halves and
//! [`crate::gameprofile`]'s rule separates them: state written only in response
//! to an explicit user command is user-owned and hand-editable, and anything a
//! program observes belongs elsewhere.
//!
//! | file | kind | writer |
//! | --- | --- | --- |
//! | `~/.config/apex/tasks.toml` | desired, user-owned, `deny_unknown_fields` | only `apex task new`/`set`/`rm`, only with what it was told |
//! | `~/.local/state/apex/tasks/<id>.json` | generated measurement, tolerant | `apex task resume`/`checkpoint`, on their own |
//!
//! The split is not tidiness. Three of the values a task carries fail the
//! user-owned test outright:
//!
//! * **"last opened"** is written by a command that was asked to *resume*, not
//!   asked to record a time. It exists so `apex task list` can order by
//!   recency, exactly as `apex project list` does.
//! * **the checkpoint id** is produced by the checkpoint engine
//!   (`<millis>-<commit>`), not typed by anybody, and it can go stale on its
//!   own — `apex agent undo` and pruning both remove checkpoints. A generated
//!   identifier that decays is the definition of a measurement.
//! * **which agent sessions are running** is not stored at all. It is asked of
//!   the agent runtime, whose sessions are already keyed by working directory,
//!   because a stored session id would be wrong the moment the session ended.
//!
//! Putting any of them in `tasks.toml` would break that file's contract the way
//! [`crate::gameprofile`] describes: it is the file a person edits, so no
//! program may rewrite it behind their back.
//!
//! ── "Windows: editor, browser, logs", honestly ──────────────────────────────
//!
//! A task stores **no window list**, and `windows` is a key that exists only to
//! be refused.
//!
//! Restoring a window set means one of two things. Remembering geometry is
//! compositor-specific state, and §17's work deliberately made the shell
//! compositor-neutral — a stored Hyprland address or niri window id is
//! meaningless after a restart, so a layout that named them would be restorable
//! exactly zero times. The other thing it can mean is *launching applications
//! again*, which is what APEX already does: `apex project layout save` records
//! each window's argv, working directory and workspace, and
//! `apex project layout restore` starts them.
//!
//! That is a per-*root* fact, and a task already names a root — the project, or
//! the worktree inside it. So a task needs no window field: the layout for its
//! root is the layout, and `apex task show` reports whether one has been saved.
//! A list of application names in a task file would read as a setting and do
//! nothing, which is the failure [`crate::gameprofile`] refuses `scheduler` and
//! `gpu` over.
//!
//! Nothing here restores a window, and `apex task resume` does not launch one:
//! reopening windows stays the explicit `apex project layout restore` it already
//! was, for the reason that module gives — a command that reopens fourteen
//! windows nobody asked for is worse than one that reopens none.
//!
//! ── A task is not a second permission system ────────────────────────────────
//!
//! §4's brokers own permissions, and a task must not become a path by which a
//! request is approved. So `permissions` is also a refused key, and there is no
//! field of any kind that a broker consults:
//!
//! * a task cannot grant anything, because nothing reads a task when deciding a
//!   request — `apex request` reads its own grants file and `apex secret` reads
//!   its own;
//! * a task cannot *record* a grant either, because `tasks.toml` is
//!   hand-editable and carried between machines by hand, and a permission in a
//!   file like that would be a grant nobody reviewed;
//! * `sandbox` is refused for the same reason one step further: a stored
//!   `sandbox = "unrestricted"` would be a standing weakening of confinement,
//!   applied by whatever later read it, reviewed by nobody.
//!
//! What is left for §21's "Permissions:" line is a pointer, which is what the
//! refusal prints: the project tree and the network are what the default
//! `project` sandbox policy already gives a confined session, a credential is
//! granted with `apex secret grant`, and a privileged operation is an
//! `apex request ask` that a human approves. A task that wants a reminder of
//! which of those its work needs has `note`, which is free text and which
//! nothing keys off.
//!
//! ── Resume refuses rather than half-works ───────────────────────────────────
//!
//! [`plan`] is pure: it takes the record and one [`Observed`] snapshot and
//! returns the ordered steps, the parts that are **gone**, and the parts it
//! could not check. Every part a task references is looked at *before* any of
//! them is used, so a task whose capsule was deleted, whose worktree was
//! removed or whose checkpoint was pruned is refused by name — it never
//! resumes three of four parts and reports success.
//!
//! A part that could not be checked is reported and does not refuse:
//! "unknown" and "gone" are different answers, which is the rule
//! [`crate::host`] applies to an unprobed device.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Schema version of `tasks.toml`. Absent means this.
pub const SCHEMA_VERSION: u32 = 1;

/// The longest a task id may be. It is a path component (the state file) and an
/// argv element, so it is bounded well below either limit.
const MAX_ID: usize = 64;

/// The longest a title may be. Printed only, but it arrives from a file.
const MAX_TITLE: usize = 200;

/// The longest a note may be. Same reasoning, with room for a sentence or two.
const MAX_NOTE: usize = 400;

/// The longest a project root may be. `PATH_MAX` on Linux is 4096.
const MAX_ROOT: usize = 4096;

/// The longest a capsule, worktree or agent name may be.
const MAX_NAME: usize = 64;

/// The most agents one task may name. §21's example names two; the slack is for
/// a reviewer and a documentation pass alongside the two doing the work.
const MAX_AGENTS: usize = 8;

// ── the file ─────────────────────────────────────────────────────────────────

/// `~/.config/apex/tasks.toml` — every task on this machine.
///
/// A `BTreeMap` keyed by id, for the reason [`crate::gameprofile`] and
/// [`crate::host`] both use one: the id is the identity, so a duplicate is
/// impossible by construction rather than by a validation pass, and a sorted
/// map serialises deterministically — which is what makes the round trip
/// lossless rather than merely reversible.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Tasks {
    /// File-format version. Absent means [`SCHEMA_VERSION`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<u32>,
    /// Tasks by id.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub task: BTreeMap<String, Task>,
}

/// One task: what a person is working on, and what it is bound to.
///
/// Every binding is optional except the project, and absent means *not bound* —
/// never "the default". A task that silently claimed a capsule it had not been
/// given would resume into an environment nobody chose.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Task {
    /// What the task is, in a sentence. §21's "Fix APEX installer bug".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Absolute project root — a git working tree, because that is what a
    /// project is (`apex_agent_core::project::detect` returns nothing for a
    /// bare directory, since without a repository there is no checkpoint to
    /// take and no worktree to create, and a task references both).
    pub project: String,
    /// The §8 capsule this task's work belongs in, by name. `apex env` owns the
    /// capsule; this is only the name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<String>,
    /// The §7 agent worktree this task works in, by name. `apex project` owns
    /// the worktree; this is only the name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree: Option<String>,
    /// Adapter ids the task's agents run under (`claude`, `codex`, …).
    ///
    /// Shape is checked here; whether an id is one the runtime can actually
    /// launch is checked by the caller, because the adapter table lives in
    /// `apex-agent-core` and this crate deliberately does not depend on it —
    /// the agent runtime must not be pulled into the privileged daemon's core.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub agents: Vec<String>,
    /// A free-text reminder of anything the record cannot express.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,

    // ── recognised only to be refused ────────────────────────────────────────
    //
    // `deny_unknown_fields` already rejects an unknown key, with a message that
    // lists the legal ones and explains nothing. These four are declared so the
    // refusal can say where the thing really lives — and they are exactly the
    // keys somebody would write after copying §21's example block, which is the
    // whole reason they are worth the lines. None can survive `validate`, so
    // none is ever serialised.
    /// **Refused.** Windows are recreated from the project's saved layout; see
    /// [`Tasks::validate`] and this module's docs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub windows: Option<Vec<String>>,
    /// **Refused.** §4's brokers own permissions; see [`Tasks::validate`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permissions: Option<Vec<String>>,
    /// **Refused.** A checkpoint id is generated state and lives in the state
    /// file; see [`Tasks::validate`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkpoint: Option<String>,
    /// **Refused.** A stored confinement policy would be a weakening nobody
    /// reviewed; see [`Tasks::validate`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox: Option<String>,
}

/// Why a task file was refused. One variant per refusal so a caller can print a
/// message naming the offending entry rather than "invalid config".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskError {
    /// A version this build does not understand.
    UnsupportedVersion(u32),
    /// The id is empty, too long, or carries a character that cannot appear in
    /// both a path component and an argv element.
    BadId(String),
    /// An id `ssh`-style option parsing or a shell would misread.
    OptionLike(String),
    /// The project root is empty, relative, too long, or unprintable.
    BadProject { id: String, why: &'static str },
    /// A capsule name `apex env` would not accept.
    BadCapsule { id: String, name: String },
    /// A worktree name that is not already the slug `apex project` derives.
    BadWorktree { id: String, name: String, why: &'static str },
    /// An agent id that is not a plausible adapter id.
    BadAgent { id: String, name: String },
    /// More agents than one task may name.
    TooManyAgents { id: String, count: usize },
    /// A title or note longer than its bound.
    TooLong { id: String, field: &'static str, max: usize },
    /// A key that exists only so its refusal can say where the thing lives.
    Refused { id: String, key: &'static str, because: &'static str },
}

impl std::fmt::Display for TaskError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedVersion(v) => write!(
                f,
                "tasks.toml is version {v}, but this apex understands up to {SCHEMA_VERSION}"
            ),
            Self::BadId(id) => write!(
                f,
                "task id {id:?} is not usable: 1-{MAX_ID} characters, letters, digits, \
                 '-', '_' or '.', and it may not be '.' or '..'"
            ),
            Self::OptionLike(id) => write!(
                f,
                "task id {id:?} starts with '-', which a command line would read as an \
                 option rather than a name, so it is refused rather than quoted"
            ),
            Self::BadProject { id, why } => write!(
                f,
                "task {id:?} has an unusable project root: {why}. A task binds a git \
                 working tree — `apex project info` prints the one you are in"
            ),
            Self::BadCapsule { id, name } => write!(
                f,
                "task {id:?} names capsule {name:?}, which `apex env` would not accept \
                 (lowercase letters, digits, '.', '_', '-'; at most 40 characters, \
                 starting with a letter or digit)"
            ),
            Self::BadWorktree { id, name, why } => write!(
                f,
                "task {id:?} names worktree {name:?}: {why}"
            ),
            Self::BadAgent { id, name } => write!(
                f,
                "task {id:?} names agent {name:?}, which is not a usable adapter id \
                 (lowercase letters, digits, '-' and '_'; at most {MAX_NAME} characters)"
            ),
            Self::TooManyAgents { id, count } => write!(
                f,
                "task {id:?} names {count} agents; at most {MAX_AGENTS} so the record \
                 stays readable"
            ),
            Self::TooLong { id, field, max } => write!(
                f,
                "task {id:?} has a {field} longer than {max} characters"
            ),
            Self::Refused { id, key, because } => write!(
                f,
                "task {id:?} sets {key}, which a task does not carry: {because}"
            ),
        }
    }
}

impl std::error::Error for TaskError {}

/// Why `windows` is refused. A constant so the message the file's refusal
/// prints and the message this module's docs describe cannot drift.
const WHY_NO_WINDOWS: &str = "a task stores no window list. Windows are recreated from the \
     project's saved layout, which records each window's argv, working directory and workspace \
     — run `apex project layout save` in the task's root, and `apex project layout restore` \
     brings them back. A list of application names here would read as a setting and do nothing";

/// Why `permissions` is refused.
const WHY_NO_PERMISSIONS: &str = "a task grants nothing, and must not: `tasks.toml` is \
     hand-editable, so a permission in it would be a grant nobody reviewed. A confined session \
     already gets the project tree and the network from the default `project` sandbox policy; a \
     credential is granted with `apex secret grant <service> <capability>`; a privileged \
     operation is an `apex request ask` that you approve with `apex request approve`. Use `note` \
     for a reminder of which of those this work needs";

/// Why `checkpoint` is refused in the user-owned file.
const WHY_NO_CHECKPOINT: &str = "a checkpoint id is generated by the checkpoint engine and can \
     be pruned, so it is a measurement and lives in the state file rather than in the file you \
     edit — `apex task path` prints where. Take one with `apex task checkpoint <id>`";

/// Why `sandbox` is refused.
const WHY_NO_SANDBOX: &str = "a stored confinement policy would be a standing weakening that \
     nobody reviewed. The policy is chosen when a session starts — `apex agent run --sandbox \
     strict|project|unrestricted`, or the default in `apex agent default`";

impl Tasks {
    /// Parse and validate a task file. Refuses rather than repairs: a file this
    /// does not fully understand is one the user should be told about.
    pub fn parse(text: &str) -> Result<Self, anyhow::Error> {
        let tasks: Self = toml::from_str(text)?;
        tasks.validate()?;
        Ok(tasks)
    }

    /// Serialise. Deterministic, because the map is sorted and every empty
    /// field is skipped.
    pub fn to_toml(&self) -> Result<String, anyhow::Error> {
        Ok(toml::to_string_pretty(self)?)
    }

    /// The first reason this file is unusable, or `Ok`.
    ///
    /// One error rather than a list, matching [`crate::host`]: the caller
    /// prints one refusal and exits non-zero, and somebody fixing a hand-edited
    /// file wants the first thing that is wrong.
    pub fn validate(&self) -> Result<(), TaskError> {
        if let Some(v) = self.version {
            if v > SCHEMA_VERSION {
                return Err(TaskError::UnsupportedVersion(v));
            }
        }
        for (id, task) in &self.task {
            check_id(id)?;

            // The refusals first, before anything that might mask them: a file
            // written by somebody who copied §21's example block should be told
            // where each of those lines really lives, not told about a
            // character class.
            for (present, key, because) in [
                (task.windows.is_some(), "windows", WHY_NO_WINDOWS),
                (task.permissions.is_some(), "permissions", WHY_NO_PERMISSIONS),
                (task.checkpoint.is_some(), "checkpoint", WHY_NO_CHECKPOINT),
                (task.sandbox.is_some(), "sandbox", WHY_NO_SANDBOX),
            ] {
                if present {
                    return Err(TaskError::Refused {
                        id: id.clone(),
                        key,
                        because,
                    });
                }
            }

            if let Err(why) = check_project_root(&task.project) {
                return Err(TaskError::BadProject { id: id.clone(), why });
            }
            if let Some(name) = &task.env {
                if !valid_capsule_name(name) {
                    return Err(TaskError::BadCapsule {
                        id: id.clone(),
                        name: name.clone(),
                    });
                }
            }
            if let Some(name) = &task.worktree {
                if let Err(why) = check_worktree_name(name) {
                    return Err(TaskError::BadWorktree {
                        id: id.clone(),
                        name: name.clone(),
                        why,
                    });
                }
            }
            if task.agents.len() > MAX_AGENTS {
                return Err(TaskError::TooManyAgents {
                    id: id.clone(),
                    count: task.agents.len(),
                });
            }
            for name in &task.agents {
                if !valid_agent_id(name) {
                    return Err(TaskError::BadAgent {
                        id: id.clone(),
                        name: name.clone(),
                    });
                }
            }
            if task.title.as_ref().is_some_and(|t| t.len() > MAX_TITLE) {
                return Err(TaskError::TooLong {
                    id: id.clone(),
                    field: "title",
                    max: MAX_TITLE,
                });
            }
            if task.note.as_ref().is_some_and(|n| n.len() > MAX_NOTE) {
                return Err(TaskError::TooLong {
                    id: id.clone(),
                    field: "note",
                    max: MAX_NOTE,
                });
            }
        }
        Ok(())
    }

    /// Look a task up by id, validating the id first so a lookup cannot be the
    /// thing that lets a bad one through.
    pub fn get(&self, id: &str) -> Result<&Task, anyhow::Error> {
        check_id(id)?;
        self.task.get(id).ok_or_else(|| {
            let known: Vec<&str> = self.task.keys().map(String::as_str).collect();
            if known.is_empty() {
                anyhow::anyhow!(
                    "no task named {id:?}, and no tasks exist yet. \
                     Start one with `apex task new {id}`"
                )
            } else {
                anyhow::anyhow!("no task named {id:?}. Tasks: {}", known.join(", "))
            }
        })
    }

    /// Ids in listing order.
    pub fn ids(&self) -> Vec<&str> {
        self.task.keys().map(String::as_str).collect()
    }

    /// Whether anything is stored at all.
    pub fn is_empty(&self) -> bool {
        self.task.is_empty()
    }
}

impl Task {
    /// What to show for a task in a one-line listing.
    pub fn label(&self) -> &str {
        self.title.as_deref().unwrap_or("(no title)")
    }
}

// ── validation ───────────────────────────────────────────────────────────────

/// A task id must work as both a path component and a command-line argument.
///
/// The same allowlist [`crate::host::validate_name`] applies to a device name,
/// for the same two reasons: the id becomes a file name under
/// `~/.local/state/apex/tasks/`, so `..` and `/` would escape it, and it
/// reaches argv, where a leading `-` is an option. Validated to an allowlist,
/// never escaped — there is no quoting function in this module.
pub fn check_id(id: &str) -> Result<(), TaskError> {
    if id.starts_with('-') {
        return Err(TaskError::OptionLike(id.to_string()));
    }
    if id.is_empty()
        || id.len() > MAX_ID
        || id == "."
        || id == ".."
        || !id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        return Err(TaskError::BadId(id.to_string()));
    }
    Ok(())
}

/// Why a project root is unusable, if it is.
///
/// Absolute, because the record is read from a different working directory than
/// the one it was written in. No `..` component, because the stored path is
/// later used as a working directory and printed into a `cd` a person will
/// paste. No control characters, because the value is printed to a terminal and
/// arrives from a file — the rule `apex_agent_core::request` applies to a
/// package name, for the same reason: a value that renders misleadingly must
/// never reach a human's decision.
pub fn check_project_root(root: &str) -> Result<(), &'static str> {
    if root.is_empty() {
        return Err("it is empty");
    }
    if root.len() > MAX_ROOT {
        return Err("it is longer than a path can be");
    }
    if !root.starts_with('/') {
        return Err("it is relative, and a task is resumed from somewhere else");
    }
    if root.split('/').any(|c| c == "..") {
        return Err("it contains '..', which does not name one fixed directory");
    }
    if root.chars().any(|c| c.is_control()) {
        return Err("it contains a control character, which would render misleadingly");
    }
    Ok(())
}

/// Why a worktree name is unusable, if it is.
///
/// Narrower than a filesystem name, and deliberately: `apex_agent_core::project`
/// derives the worktree *directory* by slugifying the name, so a task storing
/// `Issue-217` would name a directory called `issue-217` and the record would
/// no longer say where its own worktree is. The accepted set is therefore
/// exactly the shape a slug already has, and the CLI half asserts that
/// agreement against the shipped `slugify` rather than restating it here.
pub fn check_worktree_name(name: &str) -> Result<(), &'static str> {
    if name.is_empty() {
        return Err("it is empty");
    }
    if name.len() > MAX_NAME {
        return Err("it is longer than 64 characters");
    }
    if name.starts_with('-') || name.starts_with('_') {
        return Err("it starts with '-' or '_', which no slug does");
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_'))
    {
        return Err(
            "APEX derives the worktree directory by slugifying the name, so store the slug: \
             lowercase letters, digits, '-' and '_'",
        );
    }
    Ok(())
}

/// Is `name` a capsule name `apex env` would accept?
///
/// The same rule `apex_agent_core::project::valid_capsule_name` applies, and the
/// duplication is deliberate for the reason that function gives: the name
/// becomes a container name and a file path, so a binding must never store
/// something that later expands into a path somewhere else, and refusing it
/// here means the user finds out when they bind rather than when a command
/// inside the capsule fails. The two are asserted to agree by a test in the CLI
/// half, which can see both crates.
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

/// Is `name` shaped like an adapter id?
///
/// Shape only. Whether the runtime can launch it is a question for the adapter
/// table, which lives in the agent runtime library — see [`Task::agents`].
pub fn valid_agent_id(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_NAME
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_'))
        && name
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
}

// ── the generated half ───────────────────────────────────────────────────────

/// `~/.local/state/apex/tasks/<id>.json` — what has been observed about a task.
///
/// Not `deny_unknown_fields`, unlike [`Tasks`], and the reason is the same one
/// [`crate::host::HostCaps`] gives inverted: nobody hand-edits this file, so an
/// unrecognised key is not a typo somebody can act on, and refusing to read a
/// measurement costs more than ignoring a field. Unknown keys are not preserved
/// either — there is no second writer whose fields would be lost.
///
/// A file that cannot be parsed is treated as absent, for the same reason a
/// corrupt probe cache is: it can be produced again.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct TaskState {
    /// Unix seconds the task was created.
    #[serde(default)]
    pub created: u64,
    /// Unix seconds `apex task resume` last ran for it. Zero means never.
    #[serde(default)]
    pub last_opened: u64,
    /// The checkpoint `apex task checkpoint` last took, by engine id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkpoint: Option<String>,
}

// ── the resume planner ───────────────────────────────────────────────────────

/// What was found for one part a task references.
///
/// Four answers rather than a `bool`, because three of them are genuinely
/// different: a part the task never bound is not a problem, a part that is
/// there is fine, a part that is gone must refuse, and a part this machine
/// could not check must be reported and must not be treated as either of the
/// last two. That distinction is [`crate::host`]'s — "unknown" and "absent" are
/// different answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Found {
    /// The task does not reference this part at all.
    NotBound,
    /// Referenced and present.
    Present,
    /// Referenced and gone.
    Gone,
    /// Referenced, and it could not be determined. Carries the reason.
    Unknown(&'static str),
}

/// One snapshot of everything a task references, taken by the caller.
///
/// Every field is a measurement the CLI half makes; nothing in this module
/// touches a filesystem or a socket. That is what makes [`plan`] the same
/// function whether it is answering `apex task show`, `apex task resume` or a
/// unit test.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Observed {
    /// The directory the task's work happens in: its worktree when it has one,
    /// otherwise the project root. Resolved by the caller, because the worktree
    /// layout is the agent runtime's (`project::WORKTREE_DIR`) and this crate
    /// does not depend on it.
    pub working_root: String,
    /// The project root.
    pub project: Found,
    /// The named worktree's directory.
    pub worktree: Found,
    /// The named capsule's APEX record.
    pub capsule: Found,
    /// The recorded checkpoint, in the project's checkpoint list.
    pub checkpoint: Found,
    /// How many windows the saved layout for `working_root` holds, or `None`
    /// when no layout has been saved. Never a refusal: a task with no saved
    /// layout is perfectly resumable, it just reopens no windows.
    pub layout_windows: Option<usize>,
    /// Live agent sessions the runtime reports working inside `working_root`,
    /// or `None` when the runtime could not be asked. `Some(vec![])` and `None`
    /// are different: "no sessions" and "no runtime" lead to different advice.
    pub sessions: Option<Vec<u32>>,
}

/// One part of a task that is referenced and gone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gone {
    /// Which part, as it is named in `apex task show`.
    pub part: &'static str,
    /// What is missing and how to put it back.
    pub message: String,
}

/// What resuming a task amounts to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResumePlan {
    /// The task resumed.
    pub id: String,
    /// Parts that are referenced and gone. Non-empty means the task is refused
    /// rather than partly resumed.
    pub gone: Vec<Gone>,
    /// Parts that could not be checked, with the reason. Reported, never
    /// treated as present.
    pub unknown: Vec<String>,
    /// The ordered commands that pick the task back up. Empty when `gone` is
    /// non-empty, so a refusal can never print half a resume.
    pub steps: Vec<String>,
    /// Why the plan is shaped the way it is, and what it deliberately does not
    /// do. Printed, because a rule nobody can see is one the next edit removes.
    pub notes: Vec<String>,
}

impl ResumePlan {
    /// True when every part the task references is there.
    pub fn is_resumable(&self) -> bool {
        self.gone.is_empty()
    }
}

/// Turn a task and one observation of the machine into an ordered resume plan.
///
/// Pure: same inputs, same plan, every time. `apex task resume` and
/// `apex task show` call this once each and differ only in what they do with
/// the result, which is what keeps the reported plan and the executed one from
/// being two programs.
///
/// ## The ordering, and the one part of it that is not cosmetic
///
/// 1. `cd` into the working root — the worktree when there is one, because that
///    is where the task's branch is checked out.
/// 2. `apex project layout restore`, when a layout has been saved: the windows
///    come back before the work does, and this is the only thing in APEX that
///    reopens them.
/// 3. the agents: `apex agent attach <id>` for a session that is still running,
///    or `apex agent run` when none is.
/// 4. `apex env enter <capsule>` **last**, and this one is load-bearing rather
///    than aesthetic: `apex env enter` starts an interactive shell *inside* the
///    capsule, so anything listed after it would run in the container. The
///    capsule is where the task's build commands belong; the agent runtime is a
///    per-user host daemon and its sandbox is the host's, so the agent does not
///    run in there.
///
/// The recorded checkpoint is deliberately **not** a step. Restoring it
/// discards the work done since it was taken, so it is named in the notes as
/// the way back and never as part of picking the task up.
pub fn plan(id: &str, task: &Task, obs: &Observed) -> ResumePlan {
    let mut gone = Vec::new();
    let mut unknown = Vec::new();
    let mut notes = Vec::new();

    if obs.project == Found::Gone {
        gone.push(Gone {
            part: "project",
            message: format!(
                "the project root {:?} is not there. The task is kept rather than deleted — a \
                 missing directory is as likely to be an unmounted disk as a deleted checkout. \
                 Rebind it with `apex task set {id} --project <path>`, or drop the task with \
                 `apex task rm {id}`",
                task.project
            ),
        });
    }
    if let Found::Unknown(why) = &obs.project {
        unknown.push(format!("project {:?}: {why}", task.project));
    }

    match (&task.worktree, &obs.worktree) {
        (Some(name), Found::Gone) => gone.push(Gone {
            part: "worktree",
            message: format!(
                "worktree {name:?} is not at {}. `apex agent run --worktree {name}` recreates \
                 it — that is idempotent, so it reattaches an existing one rather than failing \
                 — or unbind it with `apex task set {id} --no-worktree`",
                obs.working_root
            ),
        }),
        (Some(name), Found::Unknown(why)) => unknown.push(format!("worktree {name:?}: {why}")),
        _ => {}
    }

    match (&task.env, &obs.capsule) {
        (Some(name), Found::Gone) => gone.push(Gone {
            part: "environment",
            message: format!(
                "capsule {name:?} has no APEX record. `apex env list` shows the ones you have, \
                 `apex env create {name}` makes it again, or unbind it with \
                 `apex task set {id} --no-env`"
            ),
        }),
        (Some(name), Found::Unknown(why)) => unknown.push(format!("capsule {name:?}: {why}")),
        _ => {}
    }

    match &obs.checkpoint {
        Found::Gone => gone.push(Gone {
            part: "checkpoint",
            message: format!(
                "the recorded checkpoint is no longer in the project's checkpoint list — it was \
                 pruned, or the tree it belonged to is gone. `apex project checkpoints` lists \
                 what is left, `apex task checkpoint {id}` takes a new one, and \
                 `apex task checkpoint {id} --forget` drops the reference"
            ),
        }),
        Found::Unknown(why) => unknown.push(format!("checkpoint: {why}")),
        _ => {}
    }

    if !gone.is_empty() {
        // No steps at all. A refusal that also printed a plan would be an
        // invitation to run three of the four commands and discover the fourth
        // was the one that mattered.
        return ResumePlan {
            id: id.to_string(),
            gone,
            unknown,
            steps: Vec::new(),
            notes,
        };
    }

    let mut steps = vec![format!("cd {}", obs.working_root)];

    match obs.layout_windows {
        Some(n) => {
            steps.push("apex project layout restore".to_string());
            notes.push(format!(
                "{n} window{} in the saved layout for this root. Reopening them stays an \
                 explicit command: a resume that reopened windows nobody asked for would be \
                 worse than one that reopened none",
                if n == 1 { "" } else { "s" }
            ));
        }
        None => notes.push(
            "no window layout has been saved for this root, so nothing reopens windows. \
             `apex project layout save` in it records the ones currently working there"
                .to_string(),
        ),
    }

    match obs.sessions.as_deref() {
        None => notes.push(
            "the agent runtime is not running, so this cannot say which sessions belong to the \
             task. Start it with `systemctl --user enable --now apex-agentd`"
                .to_string(),
        ),
        Some([]) => {
            let mut run = "apex agent run".to_string();
            if let Some(a) = task.agents.first() {
                run.push_str(&format!(" --agent {a}"));
            }
            if let Some(w) = &task.worktree {
                run.push_str(&format!(" --worktree {w}"));
            }
            steps.push(run);
            if task.agents.len() > 1 {
                notes.push(format!(
                    "the task names {} agents; the step above starts the first. The others are \
                     `apex agent run --agent <id>` in the same root",
                    task.agents.len()
                ));
            }
        }
        Some(ids) => {
            for sid in ids {
                steps.push(format!("apex agent attach {sid}"));
            }
            notes.push(format!(
                "{} session{} already running in this root, found by working directory — the \
                 same rule the project layout uses, because a window title is whatever a \
                 program chose to print",
                ids.len(),
                if ids.len() == 1 { " is" } else { "s are" }
            ));
        }
    }

    if let Some(name) = &task.env {
        steps.push(format!("apex env enter {name}"));
        notes.push(
            "`apex env enter` is last because it starts an interactive shell inside the \
             capsule: anything after it would run in the container. The capsule is where the \
             task's build commands belong — the agent runtime is a per-user host daemon"
                .to_string(),
        );
    }

    ResumePlan {
        id: id.to_string(),
        gone,
        unknown,
        steps,
        notes,
    }
}

// ── attaching, and the guard on it ───────────────────────────────────────────

/// What `apex task resume` should do about the task's agent sessions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Attach {
    /// Attach to this session, taking over the terminal.
    Session(u32),
    /// Do not attach, and say why. The string is printed as-is.
    No(String),
}

/// Whether a resume may take over the terminal, and which session it would.
///
/// Separated from the plan and made pure because it is a **guard**, and a guard
/// that is only exercised on a developer's desktop is one nobody has tested.
/// The rules, all four of which are refusals:
///
/// * not when stdout is not a terminal — a resume inside a script, a pipe or a
///   CI job must not become an interactive PTY relay that blocks forever;
/// * not when `--no-attach` was given, or when `--json` was, because a caller
///   asking for machine-readable output is not asking for a terminal;
/// * not when the runtime could not be asked, because there is no session id to
///   attach to;
/// * not when more than one session belongs to the task, because picking one of
///   them would be a guess about which the user meant.
pub fn choose_attach(sessions: Option<&[u32]>, opted_out: bool, interactive: bool) -> Attach {
    if opted_out {
        return Attach::No("not attaching, as asked".to_string());
    }
    if !interactive {
        return Attach::No(
            "not attaching: stdout is not a terminal, and attaching needs one".to_string(),
        );
    }
    match sessions {
        None => Attach::No("not attaching: the agent runtime is not running".to_string()),
        Some([]) => Attach::No(
            "not attaching: no agent session is running in this task's root".to_string(),
        ),
        Some([one]) => Attach::Session(*one),
        Some(many) => Attach::No(format!(
            "not attaching: {} sessions belong to this task, so which one is your choice — \
             `apex agent attach <id>`",
            many.len()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task() -> Task {
        Task {
            title: Some("Fix APEX installer bug".into()),
            project: "/home/tester/Projects/apex-os".into(),
            env: Some("fedora-build".into()),
            worktree: Some("installer-bug".into()),
            agents: vec!["claude".into(), "codex".into()],
            note: Some("needs the github credential".into()),
            ..Default::default()
        }
    }

    fn one(id: &str, t: Task) -> Tasks {
        let mut ts = Tasks::default();
        ts.task.insert(id.to_string(), t);
        ts
    }

    /// Everything present, one live session, a saved layout.
    fn healthy() -> Observed {
        Observed {
            working_root: "/home/tester/Projects/apex-os/.apex/worktrees/installer-bug".into(),
            project: Found::Present,
            worktree: Found::Present,
            capsule: Found::Present,
            checkpoint: Found::Present,
            layout_windows: Some(3),
            sessions: Some(vec![7]),
        }
    }

    // ── the file ─────────────────────────────────────────────────────────────

    #[test]
    fn a_fully_populated_file_round_trips_losslessly() {
        let original = Tasks {
            version: Some(SCHEMA_VERSION),
            ..one("installer-bug", task())
        };
        let text = original.to_toml().expect("serialises");
        let back = Tasks::parse(&text).expect("parses back");
        assert_eq!(original, back, "round trip lost something:\n{text}");
        // And again, so a normalising serialiser cannot pass by converging on
        // the second pass.
        assert_eq!(text, back.to_toml().unwrap(), "serialisation is not stable");
    }

    #[test]
    fn a_hand_written_file_parses_with_the_values_written() {
        let ts = Tasks::parse(
            r#"
            version = 1
            [task.installer-bug]
            title = "Fix APEX installer bug"
            project = "/home/tester/Projects/apex-os"
            env = "fedora-build"
            worktree = "installer-bug"
            agents = ["claude", "codex"]
            "#,
        )
        .expect("valid");
        let t = ts.get("installer-bug").expect("present");
        assert_eq!(t.project, "/home/tester/Projects/apex-os");
        assert_eq!(t.env.as_deref(), Some("fedora-build"));
        assert_eq!(t.agents, vec!["claude", "codex"]);
    }

    #[test]
    fn an_empty_file_is_valid_and_empty() {
        let ts = Tasks::parse("").expect("an empty file is an empty set");
        assert!(ts.is_empty());
        assert!(ts.ids().is_empty());
    }

    #[test]
    fn an_unknown_key_is_a_typo_and_is_refused() {
        // deny_unknown_fields: exactly one program writes this file, so an
        // unrecognised key is a mistake rather than version skew.
        let e = Tasks::parse("[task.x]\nproject = \"/p\"\nprojekt = \"/p\"\n").unwrap_err();
        assert!(format!("{e}").contains("projekt"), "{e}");
    }

    #[test]
    fn a_task_with_no_project_will_not_parse() {
        // The one required field. A task that named no project could not be
        // checkpointed, could not have a worktree, and would not be a task.
        assert!(Tasks::parse("[task.x]\ntitle = \"t\"\n").is_err());
    }

    #[test]
    fn a_future_version_is_refused_rather_than_guessed_at() {
        let e = Tasks::parse(&format!("version = {}\n", SCHEMA_VERSION + 1)).unwrap_err();
        assert!(format!("{e}").contains("understands up to"), "{e}");
    }

    // ── the keys that exist only to be refused ───────────────────────────────

    #[test]
    fn a_window_list_is_refused_and_says_where_windows_live() {
        let ts = one(
            "x",
            Task {
                windows: Some(vec!["editor".into(), "browser".into()]),
                ..task()
            },
        );
        let e = ts.validate().unwrap_err().to_string();
        assert!(e.contains("apex project layout save"), "{e}");
        assert!(e.contains("read as a setting and do nothing"), "{e}");
    }

    #[test]
    fn a_permission_list_is_refused_and_points_at_the_brokers() {
        let ts = one(
            "x",
            Task {
                permissions: Some(vec!["network".into()]),
                ..task()
            },
        );
        let e = ts.validate().unwrap_err().to_string();
        assert!(e.contains("grants nothing"), "{e}");
        assert!(e.contains("apex secret grant"), "{e}");
        assert!(e.contains("apex request ask"), "{e}");
    }

    #[test]
    fn a_checkpoint_id_in_the_user_owned_file_is_refused() {
        // The mistake gameprofile.rs describes: program-written state in the
        // one file whose contract is that no program writes it.
        let ts = one(
            "x",
            Task {
                checkpoint: Some("1788439662000-a1b2c3d".into()),
                ..task()
            },
        );
        let e = ts.validate().unwrap_err().to_string();
        assert!(e.contains("state file"), "{e}");
        assert!(e.contains("apex task checkpoint"), "{e}");
    }

    #[test]
    fn a_stored_sandbox_policy_is_refused() {
        // The one refusal that is a security property: a task file is
        // hand-editable and travels, so a stored `unrestricted` would be a
        // standing weakening nobody reviewed.
        let ts = one(
            "x",
            Task {
                sandbox: Some("unrestricted".into()),
                ..task()
            },
        );
        let e = ts.validate().unwrap_err().to_string();
        assert!(e.contains("weakening"), "{e}");
        assert!(e.contains("apex agent run --sandbox"), "{e}");
    }

    #[test]
    fn a_refused_key_names_the_task_that_carried_it() {
        let ts = one(
            "installer-bug",
            Task {
                windows: Some(vec!["editor".into()]),
                ..task()
            },
        );
        assert!(ts
            .validate()
            .unwrap_err()
            .to_string()
            .contains("installer-bug"));
    }

    #[test]
    fn a_refused_key_is_never_serialised() {
        // It cannot survive validate, so it can never be written back out.
        let t = Task {
            windows: Some(vec!["editor".into()]),
            ..task()
        };
        assert!(one("x", t).validate().is_err());
        let text = one("x", task()).to_toml().unwrap();
        for key in ["windows", "permissions", "checkpoint", "sandbox"] {
            assert!(!text.contains(key), "{key} was serialised:\n{text}");
        }
    }

    // ── ids ──────────────────────────────────────────────────────────────────

    #[test]
    fn hostile_ids_are_refused_one_reason_each() {
        assert!(check_id("").is_err());
        assert!(check_id(&"a".repeat(MAX_ID + 1)).is_err());
        assert!(check_id(".").is_err());
        assert!(check_id("..").is_err());
        assert!(check_id("../../etc/passwd").is_err());
        assert!(check_id("a/b").is_err());
        for bad in ["a b", "a\nb", "a;b", "a$b", "a`b", "a|b", "a&b", "a'b", "a\"b"] {
            assert!(check_id(bad).is_err(), "{bad:?} was accepted");
        }
        // The one whose refusal has to explain itself differently.
        let e = check_id("-rf").unwrap_err();
        assert!(matches!(e, TaskError::OptionLike(_)), "got {e:?}");
        assert!(e.to_string().contains("option"), "{e}");
    }

    #[test]
    fn ordinary_ids_are_accepted() {
        for id in ["installer-bug", "issue-217", "a", "v1.2", "Fix_It"] {
            assert!(check_id(id).is_ok(), "{id} was refused");
        }
        assert!(check_id(&"a".repeat(MAX_ID)).is_ok());
    }

    #[test]
    fn lookup_validates_the_id_so_it_cannot_be_the_hole() {
        assert!(Tasks::default().get("../../etc").is_err());
    }

    #[test]
    fn a_missing_task_names_the_ones_that_exist() {
        let ts = one("installer-bug", task());
        let e = ts.get("other").unwrap_err().to_string();
        assert!(e.contains("installer-bug"), "{e}");
    }

    #[test]
    fn a_missing_task_with_no_tasks_at_all_says_how_to_make_one() {
        let e = Tasks::default().get("x").unwrap_err().to_string();
        assert!(e.contains("apex task new"), "{e}");
    }

    // ── the other fields ─────────────────────────────────────────────────────

    #[test]
    fn a_relative_or_traversing_project_root_is_refused() {
        assert!(check_project_root("Projects/apex-os").is_err());
        assert!(check_project_root("/home/../etc").is_err());
        assert!(check_project_root("").is_err());
        assert!(check_project_root(&format!("/{}", "a".repeat(MAX_ROOT))).is_err());
        assert!(check_project_root("/home/tester/Projects/apex-os").is_ok());
        // A path with a space in it is a legal path and must stay accepted:
        // this module validates to an allowlist and never quotes, so the
        // allowlist has to be about what breaks, not about what looks tidy.
        assert!(check_project_root("/home/tester/My Projects/x").is_ok());
    }

    #[test]
    fn a_project_root_with_a_control_character_is_refused() {
        // It is printed to a terminal and it came out of a file.
        let e = check_project_root("/home/\u{1b}[2Jtester").unwrap_err();
        assert!(e.contains("control character"), "{e}");
        assert!(check_project_root("/home/tester\n").is_err());
    }

    #[test]
    fn a_worktree_name_that_is_not_its_own_slug_is_refused_with_the_reason() {
        let e = check_worktree_name("Issue-217").unwrap_err();
        assert!(e.contains("slug"), "{e}");
        assert!(check_worktree_name("issue 217").is_err());
        assert!(check_worktree_name("../escape").is_err());
        assert!(check_worktree_name("").is_err());
        assert!(check_worktree_name("issue-217").is_ok());
        assert!(check_worktree_name("fix_the_login_bug").is_ok());
    }

    #[test]
    fn capsule_names_are_held_to_the_engines_rule() {
        assert!(valid_capsule_name("fedora-build"));
        assert!(valid_capsule_name("py_3.13"));
        assert!(!valid_capsule_name("../../etc/passwd"));
        assert!(!valid_capsule_name("a..b"));
        assert!(!valid_capsule_name("Fedora"));
        assert!(!valid_capsule_name(""));
        assert!(!valid_capsule_name(&"a".repeat(41)));
    }

    #[test]
    fn too_many_agents_is_refused_with_the_count() {
        let t = Task {
            agents: (0..MAX_AGENTS + 1).map(|i| format!("a{i}")).collect(),
            ..task()
        };
        let e = one("x", t).validate().unwrap_err().to_string();
        assert!(e.contains(&format!("{}", MAX_AGENTS + 1)), "{e}");
    }

    #[test]
    fn an_overlong_title_or_note_is_refused() {
        let long = "t".repeat(MAX_TITLE + 1);
        let e = one("x", Task { title: Some(long), ..task() })
            .validate()
            .unwrap_err()
            .to_string();
        assert!(e.contains("title"), "{e}");
        let e = one(
            "x",
            Task {
                note: Some("n".repeat(MAX_NOTE + 1)),
                ..task()
            },
        )
        .validate()
        .unwrap_err()
        .to_string();
        assert!(e.contains("note"), "{e}");
    }

    #[test]
    fn a_bad_value_in_a_file_is_a_parse_failure_not_a_silent_entry() {
        let e = Tasks::parse(
            "[task.x]\nproject = \"/p\"\nworktree = \"Issue-217\"\n",
        )
        .unwrap_err();
        assert!(format!("{e}").contains("Issue-217"), "{e}");
    }

    #[test]
    fn a_task_with_no_title_still_has_something_to_print() {
        let t = Task { title: None, ..task() };
        assert_eq!(t.label(), "(no title)");
        assert_eq!(task().label(), "Fix APEX installer bug");
    }

    // ── the planner: the happy path ──────────────────────────────────────────

    #[test]
    fn a_healthy_task_plans_every_part_in_order() {
        let p = plan("installer-bug", &task(), &healthy());
        assert!(p.is_resumable(), "{:?}", p.gone);
        assert_eq!(
            p.steps,
            vec![
                "cd /home/tester/Projects/apex-os/.apex/worktrees/installer-bug".to_string(),
                "apex project layout restore".to_string(),
                "apex agent attach 7".to_string(),
                "apex env enter fedora-build".to_string(),
            ]
        );
    }

    #[test]
    fn entering_the_capsule_is_always_the_last_step() {
        // Load-bearing: `apex env enter` starts a shell INSIDE the capsule, so
        // anything after it would run in the container.
        for sessions in [None, Some(vec![]), Some(vec![7]), Some(vec![7, 9])] {
            let obs = Observed {
                sessions,
                ..healthy()
            };
            let p = plan("x", &task(), &obs);
            assert_eq!(
                p.steps.last().map(String::as_str),
                Some("apex env enter fedora-build"),
                "{:?}",
                p.steps
            );
        }
        assert!(
            p_notes_contain(&plan("x", &task(), &healthy()), "interactive shell inside the"),
            "the reason must travel with the plan"
        );
    }

    fn p_notes_contain(p: &ResumePlan, needle: &str) -> bool {
        p.notes.iter().any(|n| n.contains(needle))
    }

    #[test]
    fn a_task_with_no_capsule_plans_no_capsule_step() {
        let t = Task { env: None, ..task() };
        let obs = Observed {
            capsule: Found::NotBound,
            ..healthy()
        };
        let p = plan("x", &t, &obs);
        assert!(
            !p.steps.iter().any(|s| s.contains("env enter")),
            "{:?}",
            p.steps
        );
    }

    #[test]
    fn the_working_root_is_the_first_step_and_comes_from_the_caller() {
        let obs = Observed {
            working_root: "/somewhere/else".into(),
            ..healthy()
        };
        assert_eq!(plan("x", &task(), &obs).steps[0], "cd /somewhere/else");
    }

    #[test]
    fn a_task_with_no_running_session_is_told_how_to_start_one() {
        let obs = Observed {
            sessions: Some(vec![]),
            ..healthy()
        };
        let p = plan("x", &task(), &obs);
        assert!(
            p.steps
                .iter()
                .any(|s| s == "apex agent run --agent claude --worktree installer-bug"),
            "{:?}",
            p.steps
        );
    }

    #[test]
    fn every_running_session_gets_its_own_attach_step() {
        let obs = Observed {
            sessions: Some(vec![7, 9]),
            ..healthy()
        };
        let p = plan("x", &task(), &obs);
        assert!(p.steps.contains(&"apex agent attach 7".to_string()));
        assert!(p.steps.contains(&"apex agent attach 9".to_string()));
    }

    #[test]
    fn no_runtime_is_reported_as_such_and_not_as_no_sessions() {
        // The distinction the whole `Option<Vec<u32>>` exists for.
        let quiet = plan("x", &task(), &Observed { sessions: Some(vec![]), ..healthy() });
        let absent = plan("x", &task(), &Observed { sessions: None, ..healthy() });
        assert!(p_notes_contain(&absent, "runtime is not running"), "{:?}", absent.notes);
        assert!(
            !absent.steps.iter().any(|s| s.starts_with("apex agent run")),
            "a resume must not tell you to start an agent when it cannot see the runtime"
        );
        assert!(quiet.steps.iter().any(|s| s.starts_with("apex agent run")));
    }

    #[test]
    fn a_saved_layout_is_restored_and_an_absent_one_is_explained() {
        let with = plan("x", &task(), &healthy());
        assert!(with.steps.contains(&"apex project layout restore".to_string()));
        assert!(p_notes_contain(&with, "3 windows"), "{:?}", with.notes);

        let without = plan(
            "x",
            &task(),
            &Observed {
                layout_windows: None,
                ..healthy()
            },
        );
        assert!(
            !without
                .steps
                .iter()
                .any(|s| s.contains("layout restore")),
            "{:?}",
            without.steps
        );
        assert!(
            p_notes_contain(&without, "apex project layout save"),
            "{:?}",
            without.notes
        );
    }

    #[test]
    fn one_window_is_not_reported_as_1_windows() {
        let p = plan("x", &task(), &Observed { layout_windows: Some(1), ..healthy() });
        assert!(p_notes_contain(&p, "1 window in"), "{:?}", p.notes);
    }

    #[test]
    fn the_checkpoint_is_never_a_resume_step() {
        // Restoring it discards the work since it was taken.
        let p = plan("x", &task(), &healthy());
        assert!(
            !p.steps.iter().any(|s| s.contains("undo")),
            "{:?}",
            p.steps
        );
    }

    #[test]
    fn the_plan_is_pure() {
        let (t, o) = (task(), healthy());
        assert_eq!(plan("x", &t, &o), plan("x", &t, &o));
    }

    // ── the planner: refusing ────────────────────────────────────────────────

    #[test]
    fn a_missing_capsule_refuses_and_names_the_capsule() {
        let obs = Observed {
            capsule: Found::Gone,
            ..healthy()
        };
        let p = plan("installer-bug", &task(), &obs);
        assert!(!p.is_resumable());
        assert_eq!(p.gone.len(), 1);
        assert_eq!(p.gone[0].part, "environment");
        assert!(p.gone[0].message.contains("fedora-build"), "{:?}", p.gone[0]);
        assert!(p.gone[0].message.contains("apex env create fedora-build"));
    }

    #[test]
    fn a_missing_worktree_refuses_and_says_the_command_that_recreates_it() {
        let obs = Observed {
            worktree: Found::Gone,
            ..healthy()
        };
        let p = plan("installer-bug", &task(), &obs);
        assert!(!p.is_resumable());
        assert_eq!(p.gone[0].part, "worktree");
        assert!(
            p.gone[0]
                .message
                .contains("apex agent run --worktree installer-bug"),
            "{:?}",
            p.gone[0]
        );
    }

    #[test]
    fn a_pruned_checkpoint_refuses_and_offers_both_ways_out() {
        let obs = Observed {
            checkpoint: Found::Gone,
            ..healthy()
        };
        let p = plan("installer-bug", &task(), &obs);
        assert!(!p.is_resumable());
        assert_eq!(p.gone[0].part, "checkpoint");
        assert!(p.gone[0].message.contains("apex task checkpoint installer-bug"));
        assert!(p.gone[0].message.contains("--forget"));
    }

    #[test]
    fn a_missing_project_refuses_and_keeps_the_task() {
        let obs = Observed {
            project: Found::Gone,
            ..healthy()
        };
        let p = plan("installer-bug", &task(), &obs);
        assert_eq!(p.gone[0].part, "project");
        assert!(p.gone[0].message.contains("unmounted disk"), "{:?}", p.gone[0]);
        assert!(p.gone[0].message.contains("apex task rm installer-bug"));
    }

    #[test]
    fn a_refusal_carries_no_steps_at_all() {
        // The property the honesty rests on: it cannot print half a resume.
        for broken in [
            Observed { project: Found::Gone, ..healthy() },
            Observed { worktree: Found::Gone, ..healthy() },
            Observed { capsule: Found::Gone, ..healthy() },
            Observed { checkpoint: Found::Gone, ..healthy() },
        ] {
            let p = plan("x", &task(), &broken);
            assert!(p.steps.is_empty(), "{:?}", p.steps);
            assert!(!p.is_resumable());
        }
    }

    #[test]
    fn every_missing_part_is_named_not_only_the_first() {
        let obs = Observed {
            worktree: Found::Gone,
            capsule: Found::Gone,
            checkpoint: Found::Gone,
            ..healthy()
        };
        let p = plan("x", &task(), &obs);
        let parts: Vec<&str> = p.gone.iter().map(|g| g.part).collect();
        assert_eq!(parts, vec!["worktree", "environment", "checkpoint"]);
    }

    #[test]
    fn an_unknown_part_is_reported_and_does_not_refuse() {
        // "unknown" and "gone" are different answers. A machine that cannot
        // list checkpoints right now must not be told its task is broken.
        let obs = Observed {
            checkpoint: Found::Unknown("the root is not a git repository"),
            ..healthy()
        };
        let p = plan("x", &task(), &obs);
        assert!(p.is_resumable(), "{:?}", p.gone);
        assert_eq!(p.unknown.len(), 1);
        assert!(p.unknown[0].contains("not a git repository"), "{:?}", p.unknown);
        assert!(!p.steps.is_empty());
    }

    #[test]
    fn a_part_the_task_never_bound_is_neither_gone_nor_unknown() {
        let t = Task {
            env: None,
            worktree: None,
            ..task()
        };
        let obs = Observed {
            worktree: Found::NotBound,
            capsule: Found::NotBound,
            checkpoint: Found::NotBound,
            ..healthy()
        };
        let p = plan("x", &t, &obs);
        assert!(p.gone.is_empty());
        assert!(p.unknown.is_empty());
    }

    // ── the attach guard ─────────────────────────────────────────────────────

    #[test]
    fn a_single_session_on_a_terminal_is_attached_to() {
        assert_eq!(choose_attach(Some(&[7]), false, true), Attach::Session(7));
    }

    #[test]
    fn nothing_is_attached_without_a_terminal() {
        // The guard that keeps a resume in a script, a pipe or CI from becoming
        // a PTY relay that blocks forever.
        let a = choose_attach(Some(&[7]), false, false);
        assert!(matches!(a, Attach::No(ref why) if why.contains("not a terminal")), "{a:?}");
    }

    #[test]
    fn nothing_is_attached_when_the_caller_opted_out() {
        assert!(matches!(choose_attach(Some(&[7]), true, true), Attach::No(_)));
    }

    #[test]
    fn nothing_is_attached_when_the_choice_would_be_a_guess() {
        let a = choose_attach(Some(&[7, 9]), false, true);
        assert!(matches!(a, Attach::No(ref why) if why.contains("2 sessions")), "{a:?}");
    }

    #[test]
    fn nothing_is_attached_with_no_session_or_no_runtime_and_the_two_differ() {
        let none = choose_attach(Some(&[]), false, true);
        let absent = choose_attach(None, false, true);
        assert!(matches!(none, Attach::No(ref w) if w.contains("no agent session")), "{none:?}");
        assert!(matches!(absent, Attach::No(ref w) if w.contains("not running")), "{absent:?}");
        assert_ne!(none, absent);
    }

    // ── the state file ───────────────────────────────────────────────────────

    #[test]
    fn state_round_trips_and_tolerates_a_key_it_does_not_know() {
        let s = TaskState {
            created: 100,
            last_opened: 200,
            checkpoint: Some("1788439662000-a1b2c3d".into()),
        };
        let back: TaskState = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back, s);
        // Nobody hand-edits this file, so an unknown key is not a typo a user
        // can act on — refusing to read a measurement would cost more.
        let tolerant: TaskState =
            serde_json::from_str(r#"{"last_opened":5,"invented_later":true}"#).unwrap();
        assert_eq!(tolerant.last_opened, 5);
        assert_eq!(tolerant.checkpoint, None);
    }

    #[test]
    fn an_empty_state_file_reads_as_never_opened() {
        let s: TaskState = serde_json::from_str("{}").unwrap();
        assert_eq!(s.last_opened, 0);
        assert_eq!(s, TaskState::default());
    }
}
