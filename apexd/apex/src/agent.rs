//! `apex agent` and `apex project` — the user-facing half of the agent runtime.
//!
//! Every verb here is a thin client over `apex-agentd`'s control socket. The
//! CLI never spawns an agent itself and never holds session state, so the
//! runtime remains the single owner of every PTY and `apex agent list` says the
//! same thing whether it is asked by the terminal, by a keybind or by APEX
//! Shell.

use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use apex_agent_core::client::{self, Client};
use apex_agent_core::protocol::{
    AgentState, Request, Response, RunRequest, SandboxPolicy, SessionInfo,
};
use apex_agent_core::term::{self, RawMode, WinSize};
use apex_agent_core::{adapter, checkpoint, config, git, layout, project};
use clap::{Args, Subcommand};

use crate::ops;

/// `apex agent <verb>`.
#[derive(Subcommand)]
pub enum AgentCmd {
    /// Start an agent on a managed terminal and attach to it.
    ///
    /// The real upstream binary runs in a real PTY; APEX owns the terminal so
    /// the session survives this window closing. Detach with the detach key
    /// (ctrl-] by default) and reattach later with `apex agent attach`.
    Run(RunArgs),
    /// List sessions.
    List {
        /// Include sessions that have already finished.
        #[arg(long, short)]
        all: bool,
        /// Machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// Reattach to a session's terminal.
    Attach {
        id: u32,
        /// Do not repaint the scrollback first.
        #[arg(long)]
        no_replay: bool,
    },
    /// Suspend a session and everything it started.
    Pause { id: u32 },
    /// Resume a paused session.
    Resume { id: u32 },
    /// Stop a session.
    Kill {
        id: u32,
        /// int | term | kill. Default term, which lets the agent clean up.
        #[arg(long, default_value = "term")]
        signal: String,
    },
    /// Print a session's transcript.
    Logs {
        id: u32,
        /// How many bytes of the tail to show.
        #[arg(long, default_value_t = 64 * 1024)]
        bytes: usize,
    },
    /// Show one session in detail, or the runtime's own status.
    Status { id: Option<u32> },
    /// Show or set the agent that `a` and an unqualified run use.
    Default { agent: Option<String> },
    /// List the agents this runtime can launch.
    Adapters,
    /// What an agent changed since its checkpoint.
    Diff {
        /// Session id. Defaults to the most recent session in this project.
        id: Option<u32>,
        /// Names only, no patch.
        #[arg(long)]
        stat: bool,
    },
    /// Restore the project to a session's checkpoint.
    Undo {
        /// Session id. Defaults to the most recent session in this project.
        id: Option<u32>,
        /// Undo to a specific checkpoint instead.
        #[arg(long, conflicts_with = "id")]
        checkpoint: Option<String>,
        /// Do not ask for confirmation.
        #[arg(long, short)]
        yes: bool,
    },
    /// Capture a checkpoint of the current project now.
    Checkpoint {
        /// What this checkpoint is for.
        label: Option<String>,
    },
    /// Publish a state change for a session.
    ///
    /// This is the open agent event protocol. A process running inside a
    /// session already knows its id from `$APEX_AGENT_SESSION`, so an agent
    /// hook needs no arguments beyond the state.
    Event {
        /// working | waiting_for_user | permission_request | complete | failed
        state: String,
        /// Session id. Defaults to `$APEX_AGENT_SESSION`.
        #[arg(long)]
        session: Option<u32>,
        /// Text shown alongside the state.
        #[arg(long)]
        detail: Option<String>,
    },
    /// Forget a finished session and delete its transcript.
    Rm { id: u32 },
    /// Forget every finished session.
    Prune,
}

#[derive(Args)]
pub struct RunArgs {
    /// Opening instruction for the agent.
    pub prompt: Option<String>,
    /// Which agent to run. Defaults to the configured one.
    #[arg(long, short)]
    pub agent: Option<String>,
    /// strict | project | unrestricted. Defaults to the configured policy.
    #[arg(long, short)]
    pub sandbox: Option<String>,
    /// Run in a dedicated git worktree, creating it if needed.
    #[arg(long, short)]
    pub worktree: Option<String>,
    /// Capture a checkpoint first, so `apex agent undo` can put it back.
    #[arg(long, short)]
    pub checkpoint: bool,
    /// Where to run. Defaults to the current directory.
    #[arg(long)]
    pub cwd: Option<PathBuf>,
    /// Start it and return, instead of attaching.
    #[arg(long, short)]
    pub detach: bool,
    /// Arguments passed straight to the agent binary. With `--agent generic`
    /// the first one is the program to run.
    #[arg(last = true)]
    pub args: Vec<String>,
}

/// `apex project <verb>`.
#[derive(Subcommand)]
pub enum ProjectCmd {
    /// Projects the runtime has seen, most recent first.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Describe the project containing the current directory.
    Info,
    /// Agent worktrees of the current project.
    Worktrees,
    /// Checkpoints recorded for the current project.
    Checkpoints,
    /// Remove an agent worktree and its branch.
    Remove {
        name: String,
        /// Keep the branch.
        #[arg(long)]
        keep_branch: bool,
    },
    /// Stop tracking a project. The checkout is never touched.
    Forget { slug: String },
    /// The capsule (§8) this project's work belongs in.
    ///
    /// With no argument it reports the binding, and suggests an image alias
    /// when there is none. The suggestion is printed and never acted on:
    /// creating a container because a `package.json` exists would be a
    /// surprise measured in gigabytes.
    Env {
        /// Bind this project to a capsule. `apex env list` shows the ones you
        /// have; `apex env create <name>` makes one.
        #[arg(value_name = "CAPSULE")]
        name: Option<String>,
        /// Remove the binding. The capsule itself is untouched.
        #[arg(long, conflicts_with = "name")]
        clear: bool,
    },
    /// Go to a project: switch to the workspace its windows are on.
    ///
    /// §6's "allow switching by project, not only by numeric workspace". Needs
    /// a saved layout, because that is what records which workspace a project
    /// lives on — `apex project layout save` first.
    Switch {
        /// Project name or slug. Defaults to the one containing the current
        /// directory.
        name: Option<String>,
    },
    /// Remember or restore the windows and terminals of a project (§6).
    ///
    /// A saved layout stores how to RECREATE each window — its argv, its
    /// working directory and the workspace it was on — not a window handle,
    /// which no compositor honours after a restart.
    ///
    /// Which windows count is decided from the working directory of the
    /// process tree behind each one, never from the title: a title is whatever
    /// an application chose to print.
    Layout {
        #[command(subcommand)]
        cmd: LayoutCmd,
    },
}

/// `apex project layout <verb>`.
#[derive(Subcommand)]
pub enum LayoutCmd {
    /// Capture the windows currently working inside this project.
    Save,
    /// Show the saved layout.
    Show {
        #[arg(long)]
        json: bool,
    },
    /// Reopen the saved layout.
    ///
    /// Deliberately a command and not a login hook: a session that reopens
    /// fourteen windows nobody asked for is worse than one that reopens none.
    Restore {
        /// Print what would be started, and start nothing.
        #[arg(long)]
        dry_run: bool,
    },
    /// Discard the saved layout.
    Forget,
}

// ── agent verbs ─────────────────────────────────────────────────────────────

pub fn agent(cmd: AgentCmd) -> i32 {
    let result = match cmd {
        AgentCmd::Run(args) => run(args),
        AgentCmd::List { all, json } => list(all, json),
        AgentCmd::Attach { id, no_replay } => attach(id, !no_replay),
        AgentCmd::Pause { id } => signal(id, "stop", "paused"),
        AgentCmd::Resume { id } => signal(id, "cont", "resumed"),
        AgentCmd::Kill { id, signal: sig } => signal(id, &sig, "signalled"),
        AgentCmd::Logs { id, bytes } => logs(id, bytes),
        AgentCmd::Status { id } => status(id),
        AgentCmd::Default { agent } => default_agent(agent),
        AgentCmd::Adapters => adapters(),
        AgentCmd::Diff { id, stat } => diff(id, stat),
        AgentCmd::Undo {
            id,
            checkpoint: cp,
            yes,
        } => undo(id, cp, yes),
        AgentCmd::Checkpoint { label } => make_checkpoint(label),
        AgentCmd::Event {
            state,
            session,
            detail,
        } => event(state, session, detail),
        AgentCmd::Rm { id } => remove(id),
        AgentCmd::Prune => prune(),
    };
    report(result)
}

fn report(result: Result<i32>) -> i32 {
    match result {
        Ok(code) => code,
        Err(e) => {
            eprintln!("apex: {e:#}");
            1
        }
    }
}

fn run(args: RunArgs) -> Result<i32> {
    let cfg = config::Config::load();
    let cwd = match args.cwd {
        Some(dir) => dir
            .canonicalize()
            .with_context(|| format!("{} does not exist", dir.display()))?,
        None => std::env::current_dir().context("reading the current directory")?,
    };

    let sandbox = match args.sandbox.as_deref() {
        Some(name) => SandboxPolicy::parse(name).with_context(|| {
            format!("unknown sandbox policy {name:?}; use strict, project or unrestricted")
        })?,
        None => cfg.sandbox,
    };

    let size = term::stdout_window_size();
    let request = RunRequest {
        agent: args.agent.clone(),
        prompt: args.prompt.clone(),
        args: args.args.clone(),
        cwd: cwd.to_string_lossy().into_owned(),
        sandbox,
        worktree: args.worktree.clone(),
        checkpoint: args.checkpoint,
        cols: size.cols,
        rows: size.rows,
        env: Vec::new(),
    };

    let mut c = Client::connect()?;
    let info = match c.call(&Request::Run(request))? {
        Response::Session(info) => *info,
        other => bail!("unexpected reply: {other:?}"),
    };

    if let Some(wt) = &info.worktree {
        eprintln!("apex: worktree {wt} at {}", info.cwd);
    }
    if let Some(cp) = &info.checkpoint {
        eprintln!("apex: checkpoint {cp} — undo with `apex agent undo {}`", info.id);
    }

    if args.detach {
        println!(
            "session {} — {} in {}",
            info.id,
            info.agent,
            short_path(&info.cwd)
        );
        println!("attach with: apex agent attach {}", info.id);
        return Ok(0);
    }

    eprintln!(
        "apex: session {} ({}, sandbox {}) — detach with {}",
        info.id, info.agent, info.sandbox, cfg.detach_key
    );
    attach_session(info.id, true, &cfg)
}

fn list(all: bool, json: bool) -> Result<i32> {
    let sessions = client::sessions()?;
    let shown: Vec<&SessionInfo> = sessions
        .iter()
        .filter(|s| all || s.is_live())
        .collect();

    if json {
        println!("{}", serde_json::to_string_pretty(&shown)?);
        return Ok(0);
    }

    if shown.is_empty() {
        println!(
            "no {}sessions. start one with `apex agent run`",
            if all { "" } else { "running " }
        );
        return Ok(0);
    }

    println!(
        "{:>3}  {:<10} {:<20} {:<22} {:<12} WHERE",
        "ID", "AGENT", "STATE", "PROJECT", "SANDBOX"
    );
    for s in shown {
        let state = match s.exit_summary() {
            Some(summary) if !s.is_live() => summary,
            _ => s.state.to_string(),
        };
        let project = s
            .project_name
            .clone()
            .or_else(|| s.worktree.clone())
            .unwrap_or_else(|| "-".to_string());
        println!(
            "{:>3}  {:<10} {:<20} {:<22} {:<12} {}",
            s.id,
            truncate(&s.agent, 10),
            // 20 fits the longest real value, "killed by signal 15".
            truncate(&state, 20),
            truncate(&project, 22),
            s.sandbox,
            short_path(&s.cwd)
        );
    }
    Ok(0)
}

fn attach(id: u32, replay: bool) -> Result<i32> {
    let cfg = config::Config::load();
    attach_session(id, replay, &cfg)
}

/// Take over a session's terminal until the user detaches or it ends.
fn attach_session(id: u32, replay: bool, cfg: &config::Config) -> Result<i32> {
    let size = term::stdout_window_size();
    let replay_bytes = if replay {
        apex_agent_core::session::SCROLLBACK_BYTES
    } else {
        0
    };

    let mut c = Client::connect()?;
    match c.call(&Request::Attach {
        id,
        cols: size.cols,
        rows: size.rows,
        replay: replay_bytes,
    })? {
        Response::Attached { .. } => {}
        other => bail!("unexpected reply: {other:?}"),
    }
    c.clear_timeouts();

    // Anything the daemon sent alongside the response line is already session
    // output and must be printed before the live stream.
    let prelude = c.take_buffered();
    let read_half = c.try_clone_stream()?;
    let write_half = c.into_raw()?;

    // Raw mode for the duration, restored by the guard however this exits.
    let _raw = RawMode::enter(libc::STDIN_FILENO)?;
    install_winch_forwarder(id, size);

    let detached = client::relay(read_half, write_half, &prelude, cfg.detach_byte())?;

    // Leave the cursor somewhere sane: a TUI that was mid-repaint when the user
    // detached would otherwise leave the shell prompt in the middle of a line.
    print!("\r\n");
    std::io::stdout().flush().ok();

    if detached {
        eprintln!("apex: detached from session {id} (still running — `apex agent attach {id}`)");
        return Ok(0);
    }

    match client::session(id) {
        Ok(info) => {
            if let Some(summary) = info.exit_summary() {
                eprintln!("apex: session {id} {summary}");
            }
            Ok(info.exit_code.unwrap_or(0))
        }
        Err(_) => Ok(0),
    }
}

/// Forward terminal resizes to the session on their own short-lived
/// connections, so the raw PTY stream stays byte-transparent.
fn install_winch_forwarder(id: u32, initial: WinSize) {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    static RESIZED: AtomicBool = AtomicBool::new(false);

    extern "C" fn on_winch(_sig: libc::c_int) {
        // Async-signal-safe: one relaxed atomic store and nothing else.
        RESIZED.store(true, Ordering::Relaxed);
    }
    // Safe: installing a handler that only touches an atomic. Cast through a
    // function pointer rather than straight from the function item, which is
    // what `fn_to_numeric_cast_any` warns about.
    let handler = on_winch as extern "C" fn(libc::c_int);
    unsafe {
        libc::signal(libc::SIGWINCH, handler as libc::sighandler_t);
    }

    let last = Arc::new(std::sync::Mutex::new(initial));
    std::thread::Builder::new()
        .name("apex-winch".into())
        .spawn(move || loop {
            std::thread::sleep(std::time::Duration::from_millis(120));
            if !RESIZED.swap(false, Ordering::Relaxed) {
                continue;
            }
            let size = term::stdout_window_size();
            {
                let mut guard = last.lock().expect("winch lock");
                if *guard == size {
                    continue;
                }
                *guard = size;
            }
            if let Ok(mut c) = Client::connect() {
                let _ = c.request(&Request::Resize {
                    id,
                    cols: size.cols,
                    rows: size.rows,
                });
            }
        })
        .ok();
}

fn signal(id: u32, name: &str, past_tense: &str) -> Result<i32> {
    client::call(&Request::Signal {
        id,
        signal: name.to_string(),
    })?;
    eprintln!("apex: session {id} {past_tense}");
    Ok(0)
}

fn logs(id: u32, bytes: usize) -> Result<i32> {
    let text = client::logs(id, bytes)?;
    print!("{text}");
    if !text.ends_with('\n') {
        println!();
    }
    Ok(0)
}

fn status(id: Option<u32>) -> Result<i32> {
    match id {
        Some(id) => {
            let s = client::session(id)?;
            print_session(&s);
            Ok(0)
        }
        None => {
            let cfg = config::Config::load();
            let running = Client::is_running();
            println!("runtime      {}", if running { "running" } else { "stopped" });
            println!("socket       {}", client::socket_path().display());
            println!("default      {}", cfg.default_agent);
            println!("sandbox      {}", cfg.sandbox);
            println!("detach key   {}", cfg.detach_key);
            if !running {
                println!();
                println!("start it with: systemctl --user enable --now apex-agentd");
                return Ok(1);
            }
            let sessions = client::sessions()?;
            let live = sessions.iter().filter(|s| s.is_live()).count();
            println!("sessions     {live} running, {} recorded", sessions.len());
            Ok(0)
        }
    }
}

fn print_session(s: &SessionInfo) {
    println!("session      {}", s.id);
    println!("agent        {}", s.agent);
    println!(
        "command      {} {}",
        s.program,
        s.args.join(" ")
    );
    println!("state        {}", s.state);
    if let Some(detail) = &s.detail {
        println!("detail       {detail}");
    }
    println!("sandbox      {}", s.sandbox);
    println!("cwd          {}", s.cwd);
    if let Some(p) = &s.project_name {
        println!("project      {p}");
    }
    if let Some(w) = &s.worktree {
        println!("worktree     {w}");
    }
    if let Some(c) = &s.checkpoint {
        println!("checkpoint   {c}");
    }
    println!("pid          {}", s.pid);
    println!("terminal     {}x{}", s.cols, s.rows);
    println!("attached     {}", s.attached);
    if let Some(summary) = s.exit_summary() {
        println!("outcome      {summary}");
    }
}

fn default_agent(agent: Option<String>) -> Result<i32> {
    let (mut cfg, notes) = config::load_reporting();
    for note in &notes {
        eprintln!("apex: {note}");
    }
    let Some(agent) = agent else {
        println!("{}", cfg.default_agent);
        return Ok(0);
    };
    if adapter::by_id(&agent).is_none() {
        bail!(
            "no agent named {agent:?}. known agents: {}",
            adapter::ids().join(", ")
        );
    }
    cfg.default_agent = agent.clone();
    cfg.save()?;
    println!("default agent is now {agent}");
    Ok(0)
}

fn adapters() -> Result<i32> {
    let cfg = config::Config::load();
    println!("{:<10} {:<16} {:<12} PROGRAM", "ID", "AGENT", "INSTALLED");
    for a in adapter::ADAPTERS {
        let installed = if a.program.is_empty() {
            "n/a".to_string()
        } else if which(a.program).is_some() {
            "yes".to_string()
        } else {
            "no".to_string()
        };
        let marker = if a.id == cfg.default_agent { "*" } else { " " };
        println!(
            "{marker}{:<9} {:<16} {:<12} {}",
            a.id,
            a.display,
            installed,
            if a.program.is_empty() {
                "(caller supplies)"
            } else {
                a.program
            }
        );
    }
    Ok(0)
}

fn which(program: &str) -> Option<PathBuf> {
    // A name containing a slash is a path, not something to look up: $TERMINAL
    // is frequently set to /usr/bin/something, and searching PATH for a string
    // with a slash in it never matches.
    if program.contains('/') {
        let p = PathBuf::from(program);
        return p.is_file().then_some(p);
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join(program))
        .find(|p| p.is_file())
}

fn event(state: String, session: Option<u32>, detail: Option<String>) -> Result<i32> {
    let id = session
        .or_else(client::current_session)
        .context("no session id: pass --session, or run this inside an agent session")?;
    if AgentState::parse(&state).is_none() {
        bail!(
            "unknown state {state:?}; use working, waiting_for_user, \
             permission_request, complete or failed"
        );
    }
    client::publish_event(id, &state, detail)?;
    Ok(0)
}

fn remove(id: u32) -> Result<i32> {
    client::call(&Request::Remove { id })?;
    eprintln!("apex: session {id} removed");
    Ok(0)
}

fn prune() -> Result<i32> {
    client::call(&Request::Prune)?;
    eprintln!("apex: finished sessions removed");
    Ok(0)
}

// ── checkpoints ─────────────────────────────────────────────────────────────

/// The directory a checkpoint verb operates on, and the session it came from.
fn session_context(id: Option<u32>) -> Result<(PathBuf, Option<SessionInfo>)> {
    let cwd = std::env::current_dir().context("reading the current directory")?;
    match id {
        Some(id) => {
            let s = client::session(id)?;
            Ok((PathBuf::from(&s.cwd), Some(s)))
        }
        None => {
            // The most recent session whose working directory is this project.
            let root = git::toplevel(&cwd);
            let latest = client::sessions()
                .unwrap_or_default()
                .into_iter()
                .rfind(|s| match &root {
                    Some(r) => Path::new(&s.cwd).starts_with(r),
                    None => false,
                });
            Ok((cwd, latest))
        }
    }
}

fn make_checkpoint(label: Option<String>) -> Result<i32> {
    let cwd = std::env::current_dir()?;
    let label = label.unwrap_or_else(|| "manual".to_string());
    let cp = checkpoint::create(&cwd, &label, None)?;
    println!("checkpoint {} ({})", cp.id, cp.short_commit());
    println!("restore with: apex agent undo --checkpoint {}", cp.id);
    Ok(0)
}

fn diff(id: Option<u32>, stat: bool) -> Result<i32> {
    let (dir, session) = session_context(id)?;
    let root = git::toplevel(&dir)
        .with_context(|| format!("{} is not inside a git repository", dir.display()))?;

    // Prefer the session's own checkpoint; fall back to the newest one.
    let base = match session.as_ref().and_then(|s| s.checkpoint.clone()) {
        Some(cp_id) => Some(checkpoint::find(&root, &cp_id)?),
        None => checkpoint::latest(&root)?,
    };

    let Some(base) = base else {
        eprintln!(
            "apex: no checkpoint for this project — showing uncommitted changes instead.\n\
             apex: run agents with `--checkpoint` to get a precise before-and-after."
        );
        return run_git(&root, &["diff", "--"]);
    };

    eprintln!(
        "apex: diff against checkpoint {} ({})",
        base.id, base.label
    );

    // Diff tree-against-tree, not tree-against-working-tree. `git diff <commit>`
    // only considers tracked paths, so a file the agent created would be
    // missing from the diff of what the agent did — which is precisely the
    // question being asked.
    let now = checkpoint::current_tree(&root)?;
    if stat {
        run_git(&root, &["diff", "--stat", &base.commit, &now, "--"])
    } else {
        run_git(&root, &["diff", &base.commit, &now, "--"])
    }
}

fn run_git(dir: &Path, args: &[&str]) -> Result<i32> {
    let status = Command::new("git")
        .current_dir(dir)
        .args(args)
        .status()
        .context("running git")?;
    Ok(status.code().unwrap_or(1))
}

fn undo(id: Option<u32>, explicit: Option<String>, yes: bool) -> Result<i32> {
    let (dir, session) = session_context(id)?;
    let root = git::toplevel(&dir)
        .with_context(|| format!("{} is not inside a git repository", dir.display()))?;

    let target = match explicit {
        Some(cp_id) => checkpoint::find(&root, &cp_id)?,
        None => match session.as_ref().and_then(|s| s.checkpoint.clone()) {
            Some(cp_id) => checkpoint::find(&root, &cp_id)?,
            None => checkpoint::latest(&root)?.context(
                "no checkpoint for this project.\n\
                 run agents with `apex agent run --checkpoint`, or capture one now with \
                 `apex agent checkpoint`",
            )?,
        },
    };

    if !yes {
        eprintln!(
            "apex: restore {} to checkpoint {} ({}) taken {}?",
            root.display(),
            target.id,
            target.label,
            format_age(target.created)
        );
        eprintln!("apex: uncommitted work since then will be replaced. A safety checkpoint is taken first.");
        if !confirm()? {
            eprintln!("apex: nothing changed");
            return Ok(1);
        }
    }

    let report = checkpoint::restore(&root, &target)?;
    println!("restored to checkpoint {}", report.restored.id);
    println!("safety checkpoint {} — `apex agent undo --checkpoint {}` puts it back",
        report.safety.id, report.safety.id);
    if !report.removed.is_empty() {
        println!(
            "removed {} file(s) created after the checkpoint:",
            report.removed.len()
        );
        for f in report.removed.iter().take(20) {
            println!("  - {f}");
        }
        if report.removed.len() > 20 {
            println!("  … and {} more", report.removed.len() - 20);
        }
    }
    if report.head_moved {
        println!(
            "HEAD moved back to {}",
            &report.restored.head.clone().unwrap_or_default()[..12.min(
                report.restored.head.as_ref().map(|h| h.len()).unwrap_or(0)
            )]
        );
    }
    if !report.packages.is_empty() {
        println!();
        println!("packages changed since the checkpoint (not undone automatically):");
        for p in &report.packages.added {
            println!("  + {p}");
        }
        for p in &report.packages.removed {
            println!("  - {p}");
        }
        if let Some(cmd) = report.packages.undo_command() {
            println!("to remove the added ones: {cmd}");
        }
    }
    Ok(0)
}

/// Ask for a yes/no on the terminal. A non-interactive caller must pass
/// `--yes`; assuming consent from a script would be how somebody loses work.
fn confirm() -> Result<bool> {
    if !std::io::stdin().is_terminal() {
        bail!("not a terminal; pass --yes to confirm non-interactively");
    }
    eprint!("apex: type 'yes' to continue: ");
    std::io::stderr().flush().ok();
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer)?;
    Ok(answer.trim().eq_ignore_ascii_case("yes"))
}

// ── project verbs ───────────────────────────────────────────────────────────

pub fn project_cmd(cmd: ProjectCmd) -> i32 {
    let result = match cmd {
        ProjectCmd::List { json } => project_list(json),
        ProjectCmd::Info => project_info(),
        ProjectCmd::Worktrees => project_worktrees(),
        ProjectCmd::Checkpoints => project_checkpoints(),
        ProjectCmd::Remove { name, keep_branch } => project_remove(name, !keep_branch),
        ProjectCmd::Forget { slug } => {
            project::forget(&slug).map(|()| {
                println!("forgot {slug}");
                0
            })
        }
        ProjectCmd::Switch { name } => project_switch(name),
        ProjectCmd::Env { name, clear } => project_env(name, clear),
        ProjectCmd::Layout { cmd } => match cmd {
            LayoutCmd::Save => layout_save(),
            LayoutCmd::Show { json } => layout_show(json),
            LayoutCmd::Restore { dry_run } => layout_restore(dry_run),
            LayoutCmd::Forget => layout_forget(),
        },
    };
    report(result)
}

// ── project layouts (§6) ────────────────────────────────────────────────────

/// Where the compositor adapter lives. A fixed path, like the sandbox's bwrap:
/// resolving it through `PATH` would let a shadowing script decide what "the
/// windows of this project" means.
const WINDOW_ADAPTER: &str = "/usr/libexec/apex-project-windows";

fn window_adapter() -> String {
    // Overridable for development only, and named so it is obvious in a process
    // list. The image installs the real one.
    std::env::var("APEX_WINDOW_ADAPTER").unwrap_or_else(|_| WINDOW_ADAPTER.to_string())
}

fn layout_save() -> Result<i32> {
    let p = current_project()?;
    let adapter = window_adapter();
    let out = Command::new(&adapter)
        .arg("list")
        .output()
        .with_context(|| format!("running {adapter} list"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        bail!(
            "cannot enumerate windows: {}",
            if err.is_empty() { "the compositor has no window query".into() } else { err }
        );
    }
    let reports: Vec<layout::WindowReport> = serde_json::from_slice(&out.stdout)
        .context("parsing the window list")?;

    let children = layout::child_map();
    let captured = layout::capture(&reports, Path::new(&p.root), &children);
    if captured.is_empty() {
        // Not an error, and not silently overwriting the previous layout with
        // nothing: a capture that found no windows is far more likely to mean
        // "the adapter reported no pids" than "this project genuinely has no
        // windows open right now".
        println!(
            "no windows are working inside {} — nothing saved, and the previous \n\
             layout (if any) is untouched",
            p.name
        );
        return Ok(1);
    }
    layout::save(&p.slug, &captured)?;
    // Register the project too. Saving a layout is a strong statement that this
    // is somewhere you work, and without it the project is absent from
    // `apex project list` — which makes `apex project switch <name>` unable to
    // find it, i.e. the §6 feature this layout exists for does not work from
    // anywhere but inside the project.
    project::remember(&p)?;
    println!(
        "saved {} window(s) across workspace(s) {} for {}",
        captured.entries.len(),
        captured.workspaces().join(", "),
        p.name
    );
    Ok(0)
}

fn layout_show(json: bool) -> Result<i32> {
    let p = current_project()?;
    let Some(l) = layout::load(&p.slug) else {
        if json {
            println!("null");
        } else {
            println!("no layout saved for {} — capture one with `apex project layout save`", p.name);
        }
        return Ok(0);
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&l)?);
        return Ok(0);
    }
    println!("{:<4} {:<14} {:<10} COMMAND", "WS", "APP", "KIND");
    for e in &l.entries {
        println!(
            "{:<4} {:<14} {:<10} {}",
            if e.workspace.is_empty() { "-" } else { &e.workspace },
            e.app_id,
            if e.terminal { "terminal" } else { "app" },
            e.argv.join(" ")
        );
    }
    Ok(0)
}

fn layout_restore(dry_run: bool) -> Result<i32> {
    let p = current_project()?;
    let Some(l) = layout::load(&p.slug) else {
        println!("no layout saved for {}", p.name);
        return Ok(1);
    };

    let term = layout::choose_terminal(
        std::env::var("TERMINAL").ok().as_deref(),
        |name| which(name).is_some(),
    );
    if term.is_none() {
        eprintln!(
            "apex: no terminal emulator found; terminal windows will be restored \n\
             with the command they were originally started by"
        );
    }
    let term = term.unwrap_or_default();

    let mut started = 0;
    let mut failed = 0;
    for e in &l.entries {
        let argv = layout::restore_argv(e, &term);
        if argv.is_empty() {
            continue;
        }
        if dry_run {
            println!("would run (ws {}): {}", e.workspace, argv.join(" "));
            started += 1;
            continue;
        }
        // No shell, ever. A layout file is a list of argv vectors that this
        // executes, so it is executed as a vector — nothing in a stored entry
        // can be a shell metacharacter because nothing parses it as one.
        let spawned = Command::new(&argv[0])
            .args(&argv[1..])
            .current_dir(&e.cwd)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
        match spawned {
            Ok(_) => started += 1,
            Err(err) => {
                eprintln!("apex: cannot start {}: {err}", argv[0]);
                failed += 1;
            }
        }
    }

    if dry_run {
        println!("{started} window(s) would be restored");
        return Ok(0);
    }
    println!("started {started} window(s){}", if failed > 0 { format!(", {failed} failed") } else { String::new() });
    // Placement is best-effort and deliberately not fatal. A window has to
    // exist before it can be moved, and it does not exist until its process
    // has mapped a surface — which is asynchronous and unbounded. Restoring
    // the windows is the valuable part; getting them onto the right workspaces
    // is a nicety that would otherwise hold the command open for seconds
    // guessing at startup times.
    let workspaces = l.workspaces();
    if !workspaces.is_empty() {
        println!(
            "workspaces in this layout: {} — windows open where the compositor \n\
             puts them; run `apex project layout show` to see the intended split",
            workspaces.join(", ")
        );
    }
    if failed > 0 { Ok(1) } else { Ok(0) }
}

/// Go to a project's workspace.
///
/// The workspace comes from the saved layout, not from anything live: a project
/// does not own a workspace, it merely has windows that were on one. Which
/// means this needs a layout, and says so rather than guessing at the current
/// workspace.
///
/// Where a layout spans several workspaces, the one with the most windows wins.
/// That is a choice, not an obvious truth — the alternative is the first one
/// captured — and the most-populated one is what a person means by "where the
/// project is".
fn project_switch(name: Option<String>) -> Result<i32> {
    let p = match name {
        Some(n) => project::list()
            .into_iter()
            .find(|p| p.name == n || p.slug == n)
            .with_context(|| format!("no known project called {n:?}"))?,
        None => current_project()?,
    };

    let Some(l) = layout::load(&p.slug) else {
        bail!(
            "no layout saved for {}, so there is nothing recording which \n\
             workspace it lives on. Capture one with `apex project layout save` \n\
             while its windows are open.",
            p.name
        );
    };

    // Count windows per workspace, then take the most populated. BTreeMap so
    // ties break on the workspace name rather than on hash order — a command
    // that sends you somewhere different each time it is run is worse than one
    // that sends you somewhere arguable.
    let mut counts: std::collections::BTreeMap<&str, usize> =
        std::collections::BTreeMap::new();
    for e in &l.entries {
        if !e.workspace.is_empty() {
            *counts.entry(e.workspace.as_str()).or_default() += 1;
        }
    }
    let Some((workspace, count)) = counts.iter().max_by_key(|(_, n)| **n).map(|(w, n)| (*w, *n))
    else {
        bail!(
            "the saved layout for {} records no workspace — the compositor it \n\
             was captured under does not report one (labwc does not)",
            p.name
        );
    };

    let adapter = window_adapter();
    let status = Command::new(&adapter)
        .args(["workspace", workspace])
        .status()
        .with_context(|| format!("running {adapter} workspace {workspace}"))?;
    if !status.success() {
        bail!(
            "could not switch to workspace {workspace}: this compositor has no \n\
             workspace-switch verb (labwc exposes no IPC at all)"
        );
    }
    println!(
        "{} — workspace {} ({} of {} window(s))",
        p.name,
        workspace,
        count,
        l.entries.len()
    );
    Ok(0)
}

fn layout_forget() -> Result<i32> {
    let p = current_project()?;
    layout::forget(&p.slug)?;
    println!("discarded the saved layout for {}", p.name);
    Ok(0)
}

fn current_project() -> Result<project::Project> {
    let cwd = std::env::current_dir()?;
    project::detect(&cwd)
        .with_context(|| format!("{} is not inside a git repository", cwd.display()))
}

fn project_list(json: bool) -> Result<i32> {
    let projects = project::list();
    if json {
        println!("{}", serde_json::to_string_pretty(&projects)?);
        return Ok(0);
    }
    if projects.is_empty() {
        println!("no projects yet — they are recorded the first time an agent runs in one");
        return Ok(0);
    }
    println!("{:<24} {:<20} PATH", "NAME", "TOOLCHAINS");
    for p in projects {
        println!(
            "{:<24} {:<20} {}",
            truncate(&p.name, 24),
            truncate(&p.languages.join(","), 20),
            short_path(&p.root)
        );
    }
    Ok(0)
}

fn project_info() -> Result<i32> {
    let p = current_project()?;
    println!("name         {}", p.name);
    println!("root         {}", p.root);
    println!("slug         {}", p.slug);
    println!(
        "toolchains   {}",
        if p.languages.is_empty() {
            "-".to_string()
        } else {
            p.languages.join(", ")
        }
    );
    println!(
        "capsule      {}",
        p.capsule.clone().unwrap_or_else(|| "-".to_string())
    );
    if let Some(branch) = git::current_branch(Path::new(&p.root)) {
        println!("branch       {branch}");
    }
    let worktrees = project::worktrees(&p).unwrap_or_default();
    println!("worktrees    {}", worktrees.iter().filter(|w| w.is_agent).count());
    println!(
        "checkpoints  {}",
        checkpoint::list(Path::new(&p.root)).unwrap_or_default().len()
    );
    let sessions = client::sessions().unwrap_or_default();
    let mine = sessions
        .iter()
        .filter(|s| s.is_live() && Path::new(&s.cwd).starts_with(&p.root))
        .count();
    println!("sessions     {mine} running");
    Ok(0)
}

/// `apex project env [CAPSULE|--clear]` — §8's binding, from the project side.
///
/// Deliberately does not create anything. A capsule is hundreds of megabytes
/// and belongs to the user's decision; this records which one their work
/// belongs in and says how to make it if it does not exist yet.
fn project_env(name: Option<String>, clear: bool) -> Result<i32> {
    let p = current_project()?;

    if clear {
        project::bind_capsule(&p, None)?;
        println!("{}: no capsule (the capsule itself is untouched)", p.name);
        return Ok(0);
    }

    let Some(name) = name else {
        match &p.capsule {
            Some(c) => {
                println!("{}: {c}", p.name);
                // Naming the capsule is not the same as it existing: a
                // binding survives `apex env rm`, and a stale one that only
                // shows up when a command fails is worse than one reported
                // here.
                if !capsule_exists(c) {
                    println!(
                        "note: no capsule called '{c}' on this machine — \
                         apex env create {c}"
                    );
                }
            }
            None => {
                println!("{}: no capsule", p.name);
                if let Some(alias) = project::suggested_capsule(&p.languages) {
                    println!(
                        "this looks like a {} project; a capsule keeps its toolchain off the host:\n  \
                         apex env create {alias}\n  \
                         apex project env {alias}",
                        p.languages.join("/"),
                    );
                }
            }
        }
        return Ok(0);
    };

    project::bind_capsule(&p, Some(&name))?;
    println!("{}: {name}", p.name);
    if !capsule_exists(&name) {
        println!("note: it does not exist yet — apex env create {name}");
    }
    Ok(0)
}

/// Does `apex env` know this capsule?
///
/// A hint, so it fails open: a machine whose capsule engine is missing or
/// broken must still be able to record a binding. Reported as "exists" when
/// the answer cannot be obtained, because printing "no such capsule" for a
/// capsule that is right there is the more confusing wrong answer.
fn capsule_exists(name: &str) -> bool {
    match Command::new(ops::ENV_ENGINE)
        .args(["info", name])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
    {
        Ok(status) => status.success(),
        Err(_) => true,
    }
}

fn project_worktrees() -> Result<i32> {
    let p = current_project()?;
    let worktrees = project::worktrees(&p)?;
    println!("{:<24} {:<28} PATH", "NAME", "BRANCH");
    for w in worktrees {
        println!(
            "{:<24} {:<28} {}",
            truncate(&w.name, 24),
            truncate(w.branch.as_deref().unwrap_or("(detached)"), 28),
            short_path(&w.path.to_string_lossy())
        );
    }
    Ok(0)
}

fn project_checkpoints() -> Result<i32> {
    let p = current_project()?;
    let list = checkpoint::list(Path::new(&p.root))?;
    if list.is_empty() {
        println!("no checkpoints — capture one with `apex agent checkpoint`");
        return Ok(0);
    }
    println!("{:<24} {:<14} {:<12} LABEL", "ID", "COMMIT", "AGE");
    for cp in list {
        println!(
            "{:<24} {:<14} {:<12} {}",
            cp.id,
            cp.short_commit(),
            format_age(cp.created),
            cp.label
        );
    }
    Ok(0)
}

fn project_remove(name: String, delete_branch: bool) -> Result<i32> {
    let p = current_project()?;
    project::remove_worktree(&p, &name, delete_branch)?;
    println!(
        "removed worktree {name}{}",
        if delete_branch { " and its branch" } else { "" }
    );
    Ok(0)
}

// ── formatting ──────────────────────────────────────────────────────────────

fn truncate(s: &str, width: usize) -> String {
    if s.chars().count() <= width {
        return s.to_string();
    }
    let keep = width.saturating_sub(1);
    let mut out: String = s.chars().take(keep).collect();
    out.push('…');
    out
}

/// Replace the home prefix with `~`, the way every other listing does.
fn short_path(path: &str) -> String {
    let home = apex_agent_core::paths::home();
    let home = home.to_string_lossy();
    if !home.is_empty() && path.starts_with(home.as_ref()) {
        return format!("~{}", &path[home.len()..]);
    }
    path.to_string()
}

/// A coarse age, which is all a listing needs.
fn format_age(unix_secs: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let delta = now.saturating_sub(unix_secs);
    match delta {
        0..=59 => format!("{delta}s ago"),
        60..=3599 => format!("{}m ago", delta / 60),
        3600..=86_399 => format!("{}h ago", delta / 3600),
        _ => format!("{}d ago", delta / 86_400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncation_keeps_the_column_width() {
        assert_eq!(truncate("short", 10), "short");
        assert_eq!(truncate("exactlyten", 10), "exactlyten");
        assert_eq!(truncate("waytoolongforthis", 10).chars().count(), 10);
        assert!(truncate("waytoolongforthis", 10).ends_with('…'));
    }

    #[test]
    fn truncation_does_not_split_a_multibyte_character() {
        // Slicing by bytes here would panic on a non-ASCII project name.
        let s = "проектснадлиннымименем";
        let out = truncate(s, 8);
        assert_eq!(out.chars().count(), 8);
    }

    #[test]
    fn ages_read_in_the_largest_sensible_unit() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        assert!(format_age(now).ends_with("s ago"));
        assert!(format_age(now - 120).starts_with('2'));
        assert!(format_age(now - 120).ends_with("m ago"));
        assert!(format_age(now - 7200).ends_with("h ago"));
        assert!(format_age(now - 172_800).ends_with("d ago"));
    }

    #[test]
    fn a_future_timestamp_does_not_underflow() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        assert_eq!(format_age(now + 10_000), "0s ago");
    }

    #[test]
    fn paths_outside_home_are_left_alone() {
        assert_eq!(short_path("/usr/share/apex"), "/usr/share/apex");
    }
}
