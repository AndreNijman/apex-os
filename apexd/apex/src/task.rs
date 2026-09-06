//! `apex task` — §21's Task: the binder that references a project, an
//! environment, a worktree, agents and a checkpoint, and can be put down and
//! picked back up.
//!
//! [`apexd_core::task`] owns the record format, the validation and the pure
//! resume planner, and performs no I/O. This file is the other half: reading
//! and writing the two files, *observing* every part a task references, and
//! printing.
//!
//! ── What this file adds, and what it deliberately does not ──────────────────
//!
//! It adds one record type and the verbs that make it worth having. It adds no
//! second implementation of anything a task references:
//!
//! * the project is `apex_agent_core::project`'s — a git working tree, resolved
//!   with `git::toplevel` so a task cannot be bound to something that has no
//!   checkpoints and no worktrees;
//! * the worktree directory comes from `project::WORKTREE_DIR` and
//!   `Project::worktree_path`, not from a path this file spells out;
//! * the checkpoint list is `apex_agent_core::checkpoint`'s;
//! * the window layout is `apex_agent_core::layout`'s, and this file only ever
//!   *counts* it — `apex project layout restore` remains the one thing that
//!   reopens a window;
//! * the sessions are the agent runtime's, asked for over its control socket;
//! * the capsule is the capsule engine's, and the check is for the engine's own
//!   record rather than for a container, because that is what APEX owns.
//!
//! Every one of those is a *read*. `apex task` writes exactly two things: the
//! task file it was told to write, and the task's own state file.
//!
//! ── Unprivileged, and quiet ─────────────────────────────────────────────────
//!
//! Nothing here is privileged and nothing here can raise a prompt. It adds no
//! polkit action, no system-bus name and no helper; it never invokes `sudo`,
//! `pkexec`, `podman` or `secret-tool`; and the only programs it runs are `git`
//! (read-only plumbing, through `apex_agent_core::git`) and — when a resume
//! attaches — the agent runtime client this binary already is. The shell suite
//! asserts that, with recording stubs for all four first on `PATH`.
//!
//! ── Asking the agent runtime once ───────────────────────────────────────────
//!
//! `client::sessions()` is called **at most once per command**, and only after
//! `Client::is_running()` says a daemon is listening. Both halves matter: the
//! control socket has a 120-second read timeout, so a listener that never
//! replies would otherwise be paid for once per task in a listing, and a task
//! whose sessions cannot be determined is reported as such rather than as
//! having none.

use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use clap::{Args, Subcommand};

use apex_agent_core::protocol::SessionInfo;
use apex_agent_core::{adapter, checkpoint, client, git, layout, paths, project};
use apexd_core::task::{
    check_id, check_project_root, choose_attach, plan, Attach, Found, Observed, ResumePlan, Task,
    TaskState, Tasks,
};

use crate::blueprint::EXIT_ERROR;

#[derive(Args)]
pub struct TaskArgs {
    #[command(subcommand)]
    pub cmd: TaskCmd,
}

#[derive(Subcommand)]
pub enum TaskCmd {
    /// Start a task: bind a project, and optionally a capsule, a worktree and
    /// the agents it runs.
    ///
    /// Records the binding and nothing else. It creates no worktree, no capsule
    /// and no checkpoint — each of those is its own command, and a `new` that
    /// made a git worktree as a side effect would be a surprise in somebody's
    /// repository. It reports what is already there and what is not.
    New(NewArgs),
    /// Change a task's bindings. Only what you name is changed.
    Set(SetArgs),
    /// Every task, most recently resumed first.
    List {
        #[arg(long)]
        json: bool,
    },
    /// One task in full, with the state of every part it references.
    Show {
        id: String,
        #[arg(long)]
        json: bool,
    },
    /// Pick a task back up.
    ///
    /// Checks every part the task references FIRST. If any of them is gone it
    /// says which and stops, rather than resuming the rest and reporting
    /// success. Otherwise it prints the ordered commands that continue the work
    /// and — when exactly one agent session is running in the task's root, and
    /// stdout is a terminal — attaches to it.
    Resume {
        id: String,
        /// Print the plan and attach to nothing.
        #[arg(long)]
        no_attach: bool,
        #[arg(long)]
        json: bool,
    },
    /// Capture a checkpoint of the task's working tree and record it.
    ///
    /// The capture is `apex agent checkpoint`'s: a real git tree of tracked and
    /// untracked files, taken through a temporary index so the working tree,
    /// the index and the stash are untouched. What this adds is the binding —
    /// the checkpoint's id is written to the task's state file, so
    /// `apex task show` can say whether it is still there.
    Checkpoint {
        id: String,
        /// What this checkpoint is for.
        label: Option<String>,
        /// Drop the recorded reference instead of taking a new checkpoint. The
        /// checkpoint itself is not deleted; `apex agent undo --checkpoint`
        /// still reaches it.
        #[arg(long, conflicts_with = "label")]
        forget: bool,
    },
    /// Forget a task. Nothing it referenced is touched.
    Rm { id: String },
    /// Print where the task file and the state files live.
    Path,
}

#[derive(Args)]
pub struct NewArgs {
    /// How you will refer to it (`apex task resume installer-bug`).
    pub id: String,
    /// What the task is, in a sentence.
    #[arg(long, short)]
    pub title: Option<String>,
    /// The project. Defaults to the git working tree containing the current
    /// directory.
    #[arg(long, value_name = "PATH")]
    pub project: Option<PathBuf>,
    /// The capsule this task's work belongs in. `apex env list` shows yours.
    #[arg(long, value_name = "CAPSULE")]
    pub env: Option<String>,
    /// The agent worktree this task works in. `apex agent run --worktree
    /// <name>` creates it.
    #[arg(long, value_name = "NAME")]
    pub worktree: Option<String>,
    /// An agent this task runs. Repeatable; `apex agent adapters` lists them.
    #[arg(long, value_name = "ID")]
    pub agent: Vec<String>,
    /// Anything the record cannot express.
    #[arg(long)]
    pub note: Option<String>,
    /// Replace an existing task with this id.
    #[arg(long)]
    pub force: bool,
}

#[derive(Args)]
pub struct SetArgs {
    pub id: String,
    #[arg(long, short)]
    pub title: Option<String>,
    #[arg(long, value_name = "PATH")]
    pub project: Option<PathBuf>,
    #[arg(long, value_name = "CAPSULE")]
    pub env: Option<String>,
    #[arg(long, value_name = "NAME")]
    pub worktree: Option<String>,
    /// Replaces the whole list when given.
    #[arg(long, value_name = "ID")]
    pub agent: Vec<String>,
    #[arg(long)]
    pub note: Option<String>,
    /// Unbind the capsule. The capsule itself is untouched.
    #[arg(long, conflicts_with = "env")]
    pub no_env: bool,
    /// Unbind the worktree. The worktree itself is untouched.
    #[arg(long, conflicts_with = "worktree")]
    pub no_worktree: bool,
}

// ── where things live ────────────────────────────────────────────────────────

/// `~/.config/apex/tasks.toml`, or `$XDG_CONFIG_HOME`'s equivalent.
///
/// Resolved through `apex_agent_core::paths`, the same tested implementation of
/// the base-directory spec that `blueprint.rs`, `gaming.rs` and `host.rs` use,
/// rather than a fourth one.
pub fn tasks_path() -> PathBuf {
    paths::config_home().join("apex/tasks.toml")
}

/// `~/.local/state/apex/tasks/` — one JSON file per task.
///
/// Under the state directory because nothing in it is user-owned: it is what
/// commands observed, and deleting it costs a recency ordering and a checkpoint
/// reference, not a task.
fn state_dir() -> PathBuf {
    paths::state_home().join("apex/tasks")
}

fn state_path(id: &str) -> Result<PathBuf> {
    // Validated again here even though every caller validated already: this is
    // the function that turns an id into a *path*, so it is where a traversal
    // would land.
    check_id(id)?;
    Ok(state_dir().join(format!("{id}.json")))
}

/// The capsule engine's record directory.
///
/// `APEX_ENV_HOME` first, then `${XDG_DATA_HOME}/apex/env` — read exactly the
/// way `files/system/libexec/apex-env` reads it, including the override, which
/// is what lets the shell suite point both halves at a throwaway directory.
///
/// The check is for the engine's own JSON record, not for a container. That is
/// deliberate: the record is what APEX writes and owns, asking podman would put
/// a container-engine invocation on the path of `apex task list`, and a capsule
/// APEX has no record of is one `apex env` itself refuses to touch.
fn capsule_record(name: &str) -> PathBuf {
    let root = match std::env::var_os("APEX_ENV_HOME") {
        Some(v) if !v.is_empty() => PathBuf::from(v),
        _ => paths::data_home().join("apex/env"),
    };
    root.join(format!("{name}.json"))
}

const HEADER: &str = "\
# APEX tasks. Hand-editable.
#
# A task is a binder: it NAMES a project, a capsule, a worktree and the agents
# you run, and `apex task resume <id>` checks that each of them is still there.
# It creates none of them and grants nothing.
#
#   [task.installer-bug]
#   title     = \"Fix APEX installer bug\"
#   project   = \"/home/you/Projects/apex-os\"
#   env       = \"fedora-build\"     # a capsule from `apex env list`
#   worktree  = \"installer-bug\"    # an agent worktree, by name
#   agents    = [\"claude\", \"codex\"]
#
# What a task deliberately does NOT carry: a window list (windows come from
# `apex project layout save`), a permission of any kind (§4's brokers own
# those), a checkpoint id or a sandbox policy. Writing one of those keys here
# produces a message saying where it really lives.
";

/// Read the task file. A missing file is an empty set, never an error: `list`
/// on a machine nobody has configured should say so. A file that exists and is
/// *wrong* is always an error.
fn load() -> Result<Tasks> {
    let path = tasks_path();
    let tasks = match std::fs::read_to_string(&path) {
        Ok(text) => Tasks::parse(&text).with_context(|| path.display().to_string())?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Tasks::default(),
        Err(e) => return Err(e).with_context(|| path.display().to_string()),
    };
    // The one check the core crate cannot make. `apexd-core` does not depend on
    // the agent runtime library — the agent runtime must not be pulled into the
    // privileged daemon's core — so shape is validated there and membership
    // here, where the adapter table is visible.
    for (id, task) in &tasks.task {
        for name in &task.agents {
            if adapter::by_id(name).is_none() {
                return Err(anyhow!(
                    "task {id:?} names agent {name:?}, which this runtime cannot launch. \
                     Known agents: {}",
                    adapter::ids().join(", ")
                ))
                .with_context(|| path.display().to_string());
            }
        }
    }
    Ok(tasks)
}

/// Write the task file, atomically, after proving it reads back identically.
///
/// The round trip is `host.rs`'s rule and it earns its keep the same way:
/// without it a bad write is discovered by the *next* command, with no way to
/// tell which end was wrong.
fn save(tasks: &Tasks) -> Result<()> {
    let path = tasks_path();
    let body = tasks.to_toml().context("cannot render the task file")?;
    let text = format!("{HEADER}\n{body}");

    let reparsed =
        Tasks::parse(&text).context("refusing to write a task file that cannot be read back")?;
    if &reparsed != tasks {
        return Err(anyhow!(
            "refusing to write a task file that does not round-trip: \
             rendered {} tasks, read back {}",
            tasks.task.len(),
            reparsed.task.len()
        ));
    }

    let dir = path.parent().expect("the task path always has a parent");
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    // Same-directory temp then rename: a rename within one filesystem is
    // atomic, so a crash or a full disk leaves the old file intact rather than a
    // half-written one. The pid keeps two concurrent runs from sharing a path.
    let tmp = path.with_extension(format!("toml.tmp.{}", std::process::id()));
    std::fs::write(&tmp, &text).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, &path).with_context(|| format!("renaming into {}", path.display()))?;
    Ok(())
}

/// The task's observed state, or a default one.
///
/// A file that cannot be parsed is treated as absent, like a corrupt probe
/// cache: it is a measurement, so it can be produced again, and failing the
/// command over it would trade something recoverable for something not.
fn load_state(id: &str) -> TaskState {
    let Ok(path) = state_path(id) else {
        return TaskState::default();
    };
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

fn save_state(id: &str, state: &TaskState) -> Result<()> {
    let path = state_path(id)?;
    let dir = state_dir();
    paths::ensure_private_dir(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let tmp = path.with_extension(format!("json.tmp.{}", std::process::id()));
    std::fs::write(&tmp, serde_json::to_string_pretty(state)?)
        .with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, &path).with_context(|| format!("renaming into {}", path.display()))?;
    Ok(())
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ── observing ────────────────────────────────────────────────────────────────

/// A `Project` value for a task's recorded root.
///
/// Built rather than detected, because detection reads the filesystem and this
/// has to work for a task whose project has gone. Only the two fields
/// `worktree_path` uses are meaningful, which is why nothing else reads it.
fn project_for(task: &Task) -> project::Project {
    project::Project {
        root: task.project.clone(),
        name: String::new(),
        slug: git::path_slug(&task.project),
        languages: Vec::new(),
        last_opened: 0,
        capsule: None,
    }
}

/// The directory a task's work happens in: its worktree when it has one, the
/// project root otherwise.
///
/// The worktree path comes from `Project::worktree_path`, so this file does not
/// spell out `.apex/worktrees` and cannot drift from where `apex agent run`
/// actually puts one. A unit test asserts the two agree.
pub fn working_root(task: &Task) -> PathBuf {
    match &task.worktree {
        Some(name) => project_for(task).worktree_path(name),
        None => PathBuf::from(&task.project),
    }
}

/// The live session ids working inside `root`.
///
/// Matched by working directory, never by title or by a stored id: a cwd is
/// where the process actually is, which is the rule
/// `apex_agent_core::layout` uses to decide which windows belong to a project,
/// and a stored session id would be wrong the moment the session ended.
///
/// `starts_with` on components, not on the string: `/p/apex-os` must not match
/// a session in `/p/apex-os-fork`.
fn sessions_for(root: &Path, sessions: &[SessionInfo]) -> Vec<u32> {
    sessions
        .iter()
        .filter(|s| s.is_live() && Path::new(&s.cwd).starts_with(root))
        .map(|s| s.id)
        .collect()
}

/// Every live session the runtime knows about, or `None` when it cannot be
/// asked.
///
/// One call per command. `is_running()` first so a machine with no daemon pays
/// a `connect` that fails immediately rather than anything longer.
fn all_sessions() -> Option<Vec<SessionInfo>> {
    if !client::Client::is_running() {
        return None;
    }
    client::sessions().ok()
}

/// Look at every part `task` references.
fn observe(task: &Task, state: &TaskState, sessions: Option<&[SessionInfo]>) -> Observed {
    let root = Path::new(&task.project);
    let project_found = if root.is_dir() {
        Found::Present
    } else {
        Found::Gone
    };

    let work = working_root(task);
    let worktree = match &task.worktree {
        None => Found::NotBound,
        Some(_) if work.is_dir() => Found::Present,
        Some(_) => Found::Gone,
    };

    let capsule = match &task.env {
        None => Found::NotBound,
        Some(name) if capsule_record(name).is_file() => Found::Present,
        Some(_) => Found::Gone,
    };

    // Listed from the WORKING root, not the project root, and that is not a
    // detail: `apex_agent_core::checkpoint` keys its metadata directory on the
    // slugified git toplevel, and a git worktree's toplevel is the worktree
    // itself. So a checkpoint taken for a task working in a worktree is not in
    // the main tree's list, and asking the main tree would report every one of
    // them as pruned. This is the same root `cmd_checkpoint` captures in.
    let checkpoint_found = match &state.checkpoint {
        None => Found::NotBound,
        Some(_) if !work.is_dir() => {
            Found::Unknown("its working root is not there, so its checkpoints cannot be listed")
        }
        Some(want) => match checkpoint::list(&work) {
            Ok(list) => {
                if list.iter().any(|c| &c.id == want) {
                    Found::Present
                } else {
                    Found::Gone
                }
            }
            Err(_) => Found::Unknown(
                "the project's checkpoints could not be listed — git is unavailable, or the \
                 root is no longer a repository",
            ),
        },
    };

    // A layout that was saved and then emptied is reported as no layout: there
    // is nothing to restore either way, and "0 windows" would read as a claim.
    let layout_windows = layout::load(&git::path_slug(&work.to_string_lossy()))
        .map(|l| l.entries.len())
        .filter(|n| *n > 0);

    Observed {
        working_root: work.to_string_lossy().into_owned(),
        project: project_found,
        worktree,
        capsule,
        checkpoint: checkpoint_found,
        layout_windows,
        sessions: sessions.map(|ss| sessions_for(&work, ss)),
    }
}

// ── rendering ────────────────────────────────────────────────────────────────

/// How a part reads in `apex task show`.
fn found_text(found: &Found, bound: Option<&str>) -> String {
    match (found, bound) {
        (Found::NotBound, _) => "not bound".to_string(),
        (Found::Present, Some(v)) => format!("{v}  (present)"),
        (Found::Present, None) => "present".to_string(),
        (Found::Gone, Some(v)) => format!("{v}  (GONE)"),
        (Found::Gone, None) => "GONE".to_string(),
        (Found::Unknown(why), Some(v)) => format!("{v}  (unknown: {why})"),
        (Found::Unknown(why), None) => format!("unknown: {why}"),
    }
}

fn found_json(found: &Found) -> serde_json::Value {
    match found {
        Found::NotBound => serde_json::json!("not_bound"),
        Found::Present => serde_json::json!("present"),
        Found::Gone => serde_json::json!("gone"),
        Found::Unknown(why) => serde_json::json!({ "unknown": why }),
    }
}

fn plan_json(p: &ResumePlan) -> serde_json::Value {
    serde_json::json!({
        "id": p.id,
        "resumable": p.is_resumable(),
        "gone": p.gone.iter()
            .map(|g| serde_json::json!({ "part": g.part, "message": g.message }))
            .collect::<Vec<_>>(),
        "unknown": p.unknown,
        "steps": p.steps,
        "notes": p.notes,
    })
}

/// One task, its stored state and what was observed about it.
///
/// A named struct rather than a tuple because `list` sorts it and both JSON
/// paths render it: [`Row::to_json`] is the single definition of a task's
/// machine-readable shape, so `apex task list --json` and
/// `apex task show --json` cannot come to describe the same task differently.
struct Row {
    id: String,
    task: Task,
    state: TaskState,
    obs: Observed,
}

impl Row {
    fn to_json(&self) -> serde_json::Value {
        let (task, state, obs) = (&self.task, &self.state, &self.obs);
        serde_json::json!({
            "id": self.id,
            "title": task.title,
            "project": task.project,
            "env": task.env,
            "worktree": task.worktree,
            "agents": task.agents,
            "note": task.note,
            "created": state.created,
            "last_opened": state.last_opened,
            "checkpoint": state.checkpoint,
            "working_root": obs.working_root,
            "found": {
                "project": found_json(&obs.project),
                "environment": found_json(&obs.capsule),
                "worktree": found_json(&obs.worktree),
                "checkpoint": found_json(&obs.checkpoint),
            },
            "layout_windows": obs.layout_windows,
            "sessions": obs.sessions,
        })
    }
}

/// One line about a task for `apex task list`.
fn list_line(id: &str, task: &Task, obs: &Observed, state: &TaskState, width: usize) -> String {
    let mut bits = Vec::new();
    if let Some(n) = &task.env {
        bits.push(format!("env {n}"));
    }
    if let Some(n) = &task.worktree {
        bits.push(format!("worktree {n}"));
    }
    match obs.sessions.as_deref() {
        Some([]) | None => {}
        Some(ids) => bits.push(format!(
            "{} session{}",
            ids.len(),
            if ids.len() == 1 { "" } else { "s" }
        )),
    }
    let broken = [&obs.project, &obs.worktree, &obs.capsule, &obs.checkpoint]
        .into_iter()
        .filter(|f| **f == Found::Gone)
        .count();
    if broken > 0 {
        bits.push(format!(
            "{broken} part{} GONE",
            if broken == 1 { "" } else { "s" }
        ));
    }
    if state.last_opened == 0 {
        bits.push("never resumed".to_string());
    }
    let mut line = format!("{id:<width$}  {}", task.label());
    if !bits.is_empty() {
        line.push_str(&format!("  [{}]", bits.join(", ")));
    }
    line
}

fn print_show(id: &str, task: &Task, state: &TaskState, obs: &Observed) {
    println!("{id}  {}", task.label());
    println!("  project      {}", found_text(&obs.project, Some(&task.project)));
    println!("  environment  {}", found_text(&obs.capsule, task.env.as_deref()));
    println!(
        "  worktree     {}",
        found_text(&obs.worktree, task.worktree.as_deref())
    );
    println!(
        "  checkpoint   {}",
        found_text(&obs.checkpoint, state.checkpoint.as_deref())
    );
    // Named for what it is. There is no stored window list and nothing here
    // restores a window: this counts the project layout for the task's root,
    // which is what `apex project layout restore` would reopen.
    match obs.layout_windows {
        Some(n) => println!("  windows      {n} in the saved layout for this root"),
        None => println!("  windows      no layout saved (`apex project layout save`)"),
    }
    let agents = if task.agents.is_empty() {
        "not bound".to_string()
    } else {
        task.agents.join(", ")
    };
    match obs.sessions.as_deref() {
        None => println!("  agents       {agents}  (the agent runtime is not running)"),
        Some([]) => println!("  agents       {agents}  (no session running in this root)"),
        Some(ids) => println!(
            "  agents       {agents}  (running: {})",
            ids.iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
    println!("  working in   {}", obs.working_root);
    if let Some(n) = &task.note {
        println!("  note         {n}");
    }
    // Permissions, as §21 lists them, and the only honest thing to say: a task
    // records none and grants none.
    println!(
        "  permissions  none — a task grants nothing. `apex request pending` is where a \
         privileged operation is decided, `apex secret grants` shows credential capabilities"
    );
}

fn print_plan(p: &ResumePlan) {
    for u in &p.unknown {
        println!("  ? {u}");
    }
    println!();
    println!("resume it with:");
    for s in &p.steps {
        println!("  {s}");
    }
    for n in &p.notes {
        println!("  - {n}");
    }
}

// ── the commands ─────────────────────────────────────────────────────────────

/// `apex task`. Returns an exit code rather than a `Result` because that is the
/// dispatch's type in `main.rs`.
pub fn run(args: TaskArgs) -> i32 {
    match dispatch(args) {
        Ok(code) => code,
        Err(e) => {
            // `{e:#}` prints the anyhow context chain, so a failure three
            // layers down still says which file or task it was about.
            eprintln!("apex task: {e:#}");
            EXIT_ERROR
        }
    }
}

fn dispatch(args: TaskArgs) -> Result<i32> {
    match args.cmd {
        TaskCmd::New(a) => cmd_new(a),
        TaskCmd::Set(a) => cmd_set(a),
        TaskCmd::List { json } => cmd_list(json),
        TaskCmd::Show { id, json } => cmd_show(&id, json),
        TaskCmd::Resume { id, no_attach, json } => cmd_resume(&id, no_attach, json),
        TaskCmd::Checkpoint { id, label, forget } => cmd_checkpoint(&id, label, forget),
        TaskCmd::Rm { id } => cmd_rm(&id),
        TaskCmd::Path => {
            println!("tasks   {}", tasks_path().display());
            println!("state   {}", state_dir().display());
            Ok(0)
        }
    }
}

/// Resolve a `--project` (or the current directory) to a project root.
///
/// Through `git::toplevel`, so what is stored is the repository root rather
/// than wherever the command was run, and so a directory that is not a git
/// working tree is refused. That refusal is `project::detect`'s reasoning
/// applied here: without a repository there is nothing to checkpoint and no
/// worktree to create, and a task references both.
fn resolve_project(explicit: Option<&Path>) -> Result<String> {
    let here = std::env::current_dir().context("cannot read the current directory")?;
    let dir = explicit.map(Path::to_path_buf).unwrap_or(here);
    if !dir.is_dir() {
        return Err(anyhow!("{} is not a directory", dir.display()));
    }
    let root = git::toplevel(&dir).ok_or_else(|| {
        anyhow!(
            "{} is not inside a git working tree, and a task binds a project. \
             A project is a repository because that is what has checkpoints and \
             worktrees — `git init` it, or pass --project <path>",
            dir.display()
        )
    })?;
    let root = root.to_string_lossy().into_owned();
    check_project_root(&root).map_err(|why| anyhow!("{root:?} cannot be stored: {why}"))?;
    Ok(root)
}

/// Refuse an agent id the runtime cannot launch, before it is stored.
fn check_agents(agents: &[String]) -> Result<()> {
    for name in agents {
        if adapter::by_id(name).is_none() {
            return Err(anyhow!(
                "no agent {name:?}. Known agents: {}",
                adapter::ids().join(", ")
            ));
        }
    }
    Ok(())
}

fn cmd_new(a: NewArgs) -> Result<i32> {
    check_id(&a.id)?;
    check_agents(&a.agent)?;
    let mut tasks = load()?;
    if tasks.task.contains_key(&a.id) && !a.force {
        return Err(anyhow!(
            "task {:?} already exists. Change it with `apex task set {}`, or replace it with \
             --force",
            a.id,
            a.id
        ));
    }
    let task = Task {
        title: a.title,
        project: resolve_project(a.project.as_deref())?,
        env: a.env,
        worktree: a.worktree,
        agents: a.agent,
        note: a.note,
        ..Default::default()
    };

    // Validated as part of a whole file, not on its own: that is the function
    // the file is checked with, so a new task is held to exactly the same
    // standard as a hand-edited one.
    let mut candidate = tasks.clone();
    candidate.task.insert(a.id.clone(), task.clone());
    candidate.validate()?;

    let existed = tasks.task.insert(a.id.clone(), task.clone()).is_some();
    save(&tasks)?;

    let mut state = load_state(&a.id);
    if state.created == 0 {
        state.created = unix_now();
    }
    save_state(&a.id, &state)?;

    println!(
        "{} task {:?}",
        if existed { "replaced" } else { "started" },
        a.id
    );
    // Reported immediately, because a task that names a capsule or a worktree
    // that does not exist yet is a normal state — `apex env create` and
    // `apex agent run --worktree` are what make them — and the user should see
    // it now rather than at the first resume.
    let obs = observe(&task, &state, all_sessions().as_deref());
    print_show(&a.id, &task, &state, &obs);
    Ok(0)
}

fn cmd_set(a: SetArgs) -> Result<i32> {
    check_id(&a.id)?;
    check_agents(&a.agent)?;
    let mut tasks = load()?;
    let mut task = tasks.get(&a.id)?.clone();

    let mut changed = Vec::new();
    if let Some(t) = a.title {
        task.title = Some(t);
        changed.push("title");
    }
    if let Some(p) = &a.project {
        task.project = resolve_project(Some(p))?;
        changed.push("project");
    }
    if let Some(e) = a.env {
        task.env = Some(e);
        changed.push("env");
    }
    if a.no_env {
        task.env = None;
        changed.push("env");
    }
    if let Some(w) = a.worktree {
        task.worktree = Some(w);
        changed.push("worktree");
    }
    if a.no_worktree {
        task.worktree = None;
        changed.push("worktree");
    }
    if !a.agent.is_empty() {
        task.agents = a.agent;
        changed.push("agents");
    }
    if let Some(n) = a.note {
        task.note = Some(n);
        changed.push("note");
    }
    if changed.is_empty() {
        return Err(anyhow!(
            "nothing to change. Name at least one of --title, --project, --env, --worktree, \
             --agent, --note, --no-env or --no-worktree"
        ));
    }

    let mut candidate = tasks.clone();
    candidate.task.insert(a.id.clone(), task.clone());
    candidate.validate()?;

    tasks.task.insert(a.id.clone(), task.clone());
    save(&tasks)?;
    println!("task {:?}: changed {}", a.id, changed.join(", "));
    let state = load_state(&a.id);
    let obs = observe(&task, &state, all_sessions().as_deref());
    print_show(&a.id, &task, &state, &obs);
    Ok(0)
}

fn cmd_list(json: bool) -> Result<i32> {
    let tasks = load()?;
    // One call for the whole listing, not one per task.
    let sessions = all_sessions();

    // Most recently resumed first, then by id — the ordering
    // `apex project list` uses, for the same reason: what you were last doing
    // is what you are most likely to be picking up.
    //
    // A task whose project root has gone is NOT dropped, which is the one place
    // this deliberately differs from `project::list`: that function deletes the
    // record of a checkout that is no longer there, because a remembered
    // project is a convenience. A task is something a person wrote down, and a
    // missing directory is as likely to be an unmounted disk as a deletion.
    let mut rows: Vec<Row> = tasks
        .task
        .iter()
        .map(|(id, task)| {
            let state = load_state(id);
            let obs = observe(task, &state, sessions.as_deref());
            Row {
                id: id.clone(),
                task: task.clone(),
                state,
                obs,
            }
        })
        .collect();
    rows.sort_by(|a, b| {
        b.state
            .last_opened
            .cmp(&a.state.last_opened)
            .then(a.id.cmp(&b.id))
    });

    if json {
        let out: Vec<serde_json::Value> = rows.iter().map(Row::to_json).collect();
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(0);
    }

    if rows.is_empty() {
        println!("no tasks.");
        println!("start one with `apex task new <id> --title \"what you are doing\"` in a");
        println!("project directory. A task binds the project, and optionally a capsule, an");
        println!("agent worktree and the agents you run.");
        return Ok(0);
    }
    let width = rows.iter().map(|r| r.id.len()).max().unwrap_or(4).max(4);
    for r in &rows {
        println!("{}", list_line(&r.id, &r.task, &r.obs, &r.state, width));
    }
    Ok(0)
}

fn cmd_show(id: &str, json: bool) -> Result<i32> {
    let tasks = load()?;
    let task = tasks.get(id)?;
    let state = load_state(id);
    let obs = observe(task, &state, all_sessions().as_deref());
    if json {
        let row = Row {
            id: id.to_string(),
            task: task.clone(),
            state: state.clone(),
            obs: obs.clone(),
        };
        let mut out = row.to_json();
        // The same object `list --json` emits, plus the resume plan. Extending
        // one shape rather than writing a second keeps the two from drifting.
        out["resume"] = plan_json(&plan(id, task, &obs));
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(0);
    }
    print_show(id, task, &state, &obs);
    Ok(0)
}

fn cmd_resume(id: &str, no_attach: bool, json: bool) -> Result<i32> {
    let tasks = load()?;
    let task = tasks.get(id)?;
    let state = load_state(id);
    let obs = observe(task, &state, all_sessions().as_deref());
    let p = plan(id, task, &obs);

    if json {
        println!("{}", serde_json::to_string_pretty(&plan_json(&p))?);
        return Ok(if p.is_resumable() { 0 } else { EXIT_ERROR });
    }

    if !p.is_resumable() {
        eprintln!("apex task: task {id:?} cannot be resumed as it is recorded:");
        for g in &p.gone {
            eprintln!("  {}: {}", g.part, g.message);
        }
        for u in &p.unknown {
            eprintln!("  ? {u}");
        }
        return Ok(EXIT_ERROR);
    }

    print_show(id, task, &state, &obs);
    print_plan(&p);

    // Only now, once every part has been checked and the plan printed.
    let mut opened = state.clone();
    opened.last_opened = unix_now();
    save_state(id, &opened)?;

    match choose_attach(
        obs.sessions.as_deref(),
        no_attach,
        std::io::stdout().is_terminal(),
    ) {
        Attach::No(why) => {
            println!();
            println!("{why}");
            Ok(0)
        }
        Attach::Session(sid) => {
            println!();
            println!("attaching to session {sid}");
            // The shipped verb, called as itself rather than reimplemented: the
            // scrollback replay, the raw-mode handling and the detach key are
            // all `apex agent attach`'s, and a second relay here would be a
            // second thing to get wrong about a terminal.
            Ok(crate::agent::agent(attach_cmd(sid)))
        }
    }
}

/// The `apex agent attach <id>` invocation a resume hands off to.
///
/// A function rather than an inline literal so the one place that constructs it
/// is named, and so `host: None` is visible: a resume attaches to a session on
/// *this* machine, and continuing one on a trusted device stays the explicit
/// `apex agent attach --host` it already is.
fn attach_cmd(id: u32) -> crate::agent::AgentCmd {
    crate::agent::AgentCmd::Attach {
        id,
        no_replay: false,
        host: None,
    }
}

fn cmd_checkpoint(id: &str, label: Option<String>, forget: bool) -> Result<i32> {
    let tasks = load()?;
    let task = tasks.get(id)?;
    let mut state = load_state(id);

    if forget {
        match state.checkpoint.take() {
            None => {
                println!("task {id:?} has no recorded checkpoint");
                return Ok(0);
            }
            Some(old) => {
                save_state(id, &state)?;
                println!("task {id:?}: forgot the reference to checkpoint {old}");
                println!("the checkpoint itself is untouched — `apex project checkpoints` lists it");
                return Ok(0);
            }
        }
    }

    let root = working_root(task);
    if !root.is_dir() {
        return Err(anyhow!(
            "the task's working root {} is not there, so nothing can be captured",
            root.display()
        ));
    }
    let label = label.unwrap_or_else(|| format!("task {id}"));
    // `session: None` — this checkpoint belongs to the task, not to an agent
    // session, and claiming a session id would make `apex agent undo` with no
    // argument treat it as that session's.
    let cp = checkpoint::create(&root, &label, None)
        .with_context(|| format!("capturing a checkpoint of {}", root.display()))?;
    state.checkpoint = Some(cp.id.clone());
    save_state(id, &state)?;
    println!("task {id:?}: checkpoint {} — {}", cp.id, cp.label);
    println!("the way back is `apex agent undo --checkpoint {}`", cp.id);
    Ok(0)
}

fn cmd_rm(id: &str) -> Result<i32> {
    let mut tasks = load()?;
    tasks.get(id)?;
    tasks.task.remove(id);
    save(&tasks)?;
    // Best effort: a leftover state file for a removed task is noise, and
    // failing the removal over it would leave the two files disagreeing about
    // whether the task exists.
    if let Ok(p) = state_path(id) {
        let _ = std::fs::remove_file(p);
    }
    println!("removed task {id:?}");
    println!("nothing it referenced was touched: the project, worktree, capsule and any");
    println!("checkpoints are all still there.");
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use apex_agent_core::protocol::{AgentState, SandboxPolicy};

    fn task() -> Task {
        Task {
            title: Some("Fix APEX installer bug".into()),
            project: "/home/tester/Projects/apex-os".into(),
            env: Some("fedora-build".into()),
            worktree: Some("installer-bug".into()),
            agents: vec!["claude".into()],
            note: None,
            ..Default::default()
        }
    }

    fn session(id: u32, cwd: &str, live: bool) -> SessionInfo {
        SessionInfo {
            id,
            agent: "claude".into(),
            program: "claude".into(),
            args: Vec::new(),
            cwd: cwd.into(),
            project: None,
            project_name: None,
            worktree: None,
            state: AgentState::Working,
            detail: None,
            paused: false,
            sandbox: SandboxPolicy::Project,
            pid: 1,
            started: 0,
            last_activity: 0,
            exit_code: if live { None } else { Some(0) },
            exit_signal: None,
            attached: 0,
            checkpoint: None,
            cols: 80,
            rows: 24,
        }
    }

    // ── agreement with the modules a task references ─────────────────────────
    //
    // These are the assertions that keep the two crates from drifting. The core
    // schema validates shape without depending on the agent runtime; each test
    // below proves the shape it accepts is the shape the runtime actually uses.

    #[test]
    fn the_worktree_path_is_the_one_apex_agent_run_would_create() {
        // Not a path spelled out here: `Project::worktree_path` is what
        // `project::ensure_worktree` uses, so this proves the task resolves the
        // same directory rather than a similar one.
        let t = task();
        let p = project_for(&t);
        assert_eq!(
            working_root(&t),
            p.worktree_path("installer-bug"),
            "the task and the agent runtime disagree about where a worktree is"
        );
        assert!(working_root(&t).starts_with(&t.project));
        assert!(working_root(&t)
            .to_string_lossy()
            .contains(project::WORKTREE_DIR));
    }

    #[test]
    fn a_task_with_no_worktree_works_in_the_project_root() {
        let t = Task {
            worktree: None,
            ..task()
        };
        assert_eq!(working_root(&t), PathBuf::from("/home/tester/Projects/apex-os"));
    }

    #[test]
    fn every_worktree_name_the_schema_accepts_is_already_its_own_slug() {
        // The reason `check_worktree_name` is narrower than a filesystem name:
        // the runtime slugifies, so a name that is not its own slug would make
        // the record point at a directory that is not the worktree's.
        for name in ["installer-bug", "issue-217", "fix_the_login_bug", "a1"] {
            assert!(
                apexd_core::task::check_worktree_name(name).is_ok(),
                "{name} was refused"
            );
            assert_eq!(git::slugify(name), name, "{name} is not its own slug");
        }
        // And the converse: something the schema refuses is something slugify
        // would have changed.
        for name in ["Issue-217", "fix the login bug", "../escape"] {
            assert!(apexd_core::task::check_worktree_name(name).is_err(), "{name}");
            assert_ne!(git::slugify(name), name);
        }
    }

    #[test]
    fn the_schemas_capsule_rule_is_the_engines_capsule_rule() {
        // Two implementations of one rule, held together by this test rather
        // than by hope. `apexd-core` cannot call the agent runtime's copy.
        for name in [
            "fedora",
            "fedora-build",
            "py_3.13",
            "ml-2024",
            "a",
            "9x",
            "",
            "Fedora",
            "a..b",
            "../../etc/passwd",
            "a/b",
            "-rf",
            "a b",
            "toolong-toolong-toolong-toolong-toolong-toolong",
        ] {
            assert_eq!(
                apexd_core::task::valid_capsule_name(name),
                project::valid_capsule_name(name),
                "the two capsule-name rules disagree about {name:?}"
            );
        }
    }

    #[test]
    fn every_shipped_adapter_id_is_a_shape_the_schema_accepts() {
        // If an adapter were ever added with a capital letter or a dot, the
        // schema would refuse a task naming it — and the failure would surface
        // as "not a usable adapter id" for an id that plainly exists.
        for id in adapter::ids() {
            assert!(
                apexd_core::task::valid_agent_id(id),
                "the shipped adapter {id:?} is not a shape a task may name"
            );
        }
    }

    // ── the session match ────────────────────────────────────────────────────

    #[test]
    fn a_session_in_the_root_belongs_to_the_task() {
        let root = Path::new("/p/apex-os");
        let ss = vec![
            session(1, "/p/apex-os", true),
            session(2, "/p/apex-os/apexd/apex", true),
        ];
        assert_eq!(sessions_for(root, &ss), vec![1, 2]);
    }

    #[test]
    fn a_sibling_directory_with_a_shared_prefix_does_not_match() {
        // The bug a string `starts_with` would have: `/p/apex-os` matching
        // `/p/apex-os-fork`.
        let ss = vec![session(3, "/p/apex-os-fork", true)];
        assert!(sessions_for(Path::new("/p/apex-os"), &ss).is_empty());
    }

    #[test]
    fn a_finished_session_is_not_a_running_one() {
        let ss = vec![session(4, "/p/apex-os", false)];
        assert!(sessions_for(Path::new("/p/apex-os"), &ss).is_empty());
    }

    #[test]
    fn a_session_elsewhere_is_not_the_tasks() {
        let ss = vec![session(5, "/p/other", true)];
        assert!(sessions_for(Path::new("/p/apex-os"), &ss).is_empty());
    }

    #[test]
    fn a_worktree_task_matches_only_sessions_in_the_worktree() {
        // The point of resolving the working root first: an agent running in
        // the main tree is not working on this task.
        let t = task();
        let work = working_root(&t);
        let ss = vec![
            session(6, "/home/tester/Projects/apex-os", true),
            session(
                7,
                "/home/tester/Projects/apex-os/.apex/worktrees/installer-bug",
                true,
            ),
        ];
        assert_eq!(sessions_for(&work, &ss), vec![7]);
    }

    // ── paths ────────────────────────────────────────────────────────────────

    #[test]
    fn a_traversing_id_cannot_become_a_state_path() {
        assert!(state_path("../../etc/passwd").is_err());
        assert!(state_path("..").is_err());
        assert!(state_path("a/b").is_err());
    }

    #[test]
    fn a_legal_id_becomes_a_file_inside_the_state_directory() {
        let p = state_path("installer-bug").unwrap();
        assert!(p.starts_with(state_dir()));
        assert_eq!(p.file_name().unwrap(), "installer-bug.json");
    }

    #[test]
    fn the_task_file_lives_beside_the_other_apex_config() {
        assert!(
            tasks_path().ends_with("apex/tasks.toml"),
            "got {}",
            tasks_path().display()
        );
    }

    #[test]
    fn the_state_directory_is_not_the_config_directory() {
        // The whole point of the split: a measurement must not land in the file
        // a person edits.
        assert!(!state_dir().starts_with(tasks_path().parent().unwrap()));
    }

    #[test]
    fn the_capsule_record_is_the_one_the_engine_writes() {
        // The engine's own path shape, `<root>/<name>.json`, and its override.
        let p = capsule_record("fedora-build");
        assert_eq!(p.file_name().unwrap(), "fedora-build.json");
        assert!(
            p.parent().unwrap().ends_with("apex/env")
                || std::env::var_os("APEX_ENV_HOME").is_some(),
            "got {}",
            p.display()
        );
    }

    // ── rendering ────────────────────────────────────────────────────────────

    #[test]
    fn a_missing_part_renders_loudly_and_an_unbound_one_does_not() {
        assert!(found_text(&Found::Gone, Some("fedora-build")).contains("GONE"));
        assert!(found_text(&Found::Gone, Some("fedora-build")).contains("fedora-build"));
        assert_eq!(found_text(&Found::NotBound, None), "not bound");
        assert!(found_text(&Found::Present, Some("x")).contains("present"));
        assert!(found_text(&Found::Unknown("no git"), None).contains("no git"));
    }

    #[test]
    fn the_json_shape_of_a_part_is_stable() {
        assert_eq!(found_json(&Found::Present), serde_json::json!("present"));
        assert_eq!(found_json(&Found::Gone), serde_json::json!("gone"));
        assert_eq!(found_json(&Found::NotBound), serde_json::json!("not_bound"));
        assert_eq!(
            found_json(&Found::Unknown("why")),
            serde_json::json!({ "unknown": "why" })
        );
    }

    #[test]
    fn a_list_line_names_what_is_broken_and_how_much() {
        let t = task();
        let obs = Observed {
            working_root: "/w".into(),
            project: Found::Present,
            worktree: Found::Gone,
            capsule: Found::Gone,
            checkpoint: Found::NotBound,
            layout_windows: None,
            sessions: Some(vec![]),
        };
        let line = list_line("installer-bug", &t, &obs, &TaskState::default(), 13);
        assert!(line.contains("installer-bug"), "{line}");
        assert!(line.contains("2 parts GONE"), "{line}");
        assert!(line.contains("never resumed"), "{line}");
    }

    #[test]
    fn a_healthy_list_line_reports_the_running_sessions() {
        let obs = Observed {
            working_root: "/w".into(),
            project: Found::Present,
            worktree: Found::Present,
            capsule: Found::Present,
            checkpoint: Found::Present,
            layout_windows: Some(2),
            sessions: Some(vec![7]),
        };
        let state = TaskState {
            last_opened: 99,
            ..Default::default()
        };
        let line = list_line("installer-bug", &task(), &obs, &state, 13);
        assert!(line.contains("1 session"), "{line}");
        assert!(!line.contains("GONE"), "{line}");
        assert!(!line.contains("never resumed"), "{line}");
    }

    #[test]
    fn the_resume_json_says_whether_it_is_resumable() {
        let obs = Observed {
            working_root: "/w".into(),
            project: Found::Present,
            worktree: Found::Gone,
            capsule: Found::Present,
            checkpoint: Found::NotBound,
            layout_windows: None,
            sessions: None,
        };
        let p = plan("x", &task(), &obs);
        let j = plan_json(&p);
        assert_eq!(j["resumable"], serde_json::json!(false));
        assert_eq!(j["steps"].as_array().unwrap().len(), 0);
        assert_eq!(j["gone"][0]["part"], serde_json::json!("worktree"));
    }
}
