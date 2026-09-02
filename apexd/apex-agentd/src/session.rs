//! Starting sessions, reading their terminals, and attaching to them.

use std::io::{BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use apex_agent_core::adapter;
use apex_agent_core::checkpoint;
use apex_agent_core::client::SESSION_ENV;
use apex_agent_core::paths;
use apex_agent_core::project;
use apex_agent_core::protocol::{AgentState, ErrorKind, Response, RunRequest, SessionInfo};
use apex_agent_core::sandbox::{self, SandboxError, SandboxSpec};
use apex_agent_core::session as logic;
use apex_agent_core::term::WinSize;

use crate::pty;
use crate::registry::{self, now_secs, Handle};
use crate::Daemon;

/// How long the reader thread waits for output before re-evaluating state.
///
/// One second: fast enough that the idle transition lands on time, slow enough
/// that an idle session costs one wakeup per second rather than a spin.
const POLL_INTERVAL_MS: i32 = 1000;

/// Start a session.
pub fn start(daemon: &Arc<Daemon>, req: RunRequest) -> Result<SessionInfo> {
    let cwd = PathBuf::from(&req.cwd);
    if !cwd.is_absolute() {
        bail!("working directory {} must be absolute", cwd.display());
    }
    if !cwd.is_dir() {
        bail!("working directory {} does not exist", cwd.display());
    }

    let cfg = daemon.config.lock().expect("config lock").clone();
    let agent_id = req.agent.clone().unwrap_or_else(|| cfg.default_agent.clone());
    let adapter = adapter::by_id(&agent_id)
        .with_context(|| format!("no agent adapter named {agent_id:?}"))?;

    // The generic adapter carries no program of its own, so the caller has to
    // supply one; anything else would be a session with nothing to run.
    let explicit = req.args.first().filter(|_| adapter.id == "generic");
    let program = adapter
        .resolve_program(explicit.map(|s| s.as_str()))
        .with_context(|| {
            format!("the {agent_id} adapter needs a program to run; pass one after `--`")
        })?;
    let extra: Vec<String> = if explicit.is_some() {
        req.args[1..].to_vec()
    } else {
        req.args.clone()
    };

    if pty::resolve_program(&program).is_none() {
        bail!(
            "{program} is not installed or not on PATH.\n\
             install it, or run a different agent with `apex agent run --agent <name>`"
        );
    }

    // Fail closed before anything is created: a session must never start with
    // weaker confinement than was asked for.
    sandbox::preflight(req.sandbox).map_err(SandboxRefused)?;

    // Resolve the project, then the worktree, then the working directory. Each
    // step can change where the session actually runs.
    let detected = project::detect(&cwd);
    let mut workdir = cwd.clone();
    let mut worktree_name = None;

    if let Some(name) = req.worktree.as_deref() {
        let proj = detected
            .as_ref()
            .with_context(|| format!("{} is not in a git repository, so --worktree cannot be used", cwd.display()))?;
        workdir = project::ensure_worktree(proj, name)
            .with_context(|| format!("creating the worktree {name}"))?;
        worktree_name = Some(project::Project::worktree_branch(proj, name));
    }

    if let Some(proj) = detected.as_ref() {
        let _ = project::remember(proj);
    }

    // The checkpoint is taken against the directory the agent will actually
    // work in, which for a worktree run is the worktree, not the main tree.
    let checkpoint_id = if req.checkpoint || cfg.auto_checkpoint {
        match checkpoint::create(&workdir, "before agent task", None) {
            Ok(cp) => Some(cp.id),
            Err(e) => {
                // A project without git, or a git failure, must not stop the
                // agent from running — but the user has to be told the undo
                // they asked for does not exist.
                eprintln!("apex-agentd: checkpoint skipped: {e:#}");
                None
            }
        }
    } else {
        None
    };

    let id = daemon.registry.lock().expect("registry lock").allocate();
    let scratch = paths::scratch_dir(id);
    // Not best-effort: the sandbox binds this path read-write and sets TMPDIR
    // to it. If it cannot be created, or cannot be made private, the session
    // would start with an unexpected scratch directory and fail later in a much
    // harder place to diagnose.
    paths::ensure_private_dir(&scratch)
        .with_context(|| format!("preparing the session scratch directory {}", scratch.display()))?;

    let args = adapter.build_args(req.prompt.as_deref(), &extra);
    let size = WinSize {
        cols: req.cols,
        rows: req.rows,
    }
    .or_fallback();

    // Build the sandbox.
    let mut spec = SandboxSpec::new(req.sandbox, paths::home(), paths::runtime_dir());
    spec.control_socket = paths::control_socket();
    spec.scratch = scratch.clone();
    spec.cwd = workdir.clone();
    spec.rw.push(workdir.clone());
    if let Some(proj) = detected.as_ref() {
        // The main checkout as well as the worktree: a worktree's `.git` file
        // points into the main repository, so a worktree session that cannot
        // reach it cannot run git at all.
        let root = PathBuf::from(&proj.root);
        if !spec.rw.contains(&root) {
            spec.rw.push(root);
        }
    }
    adapter.apply_sandbox(&mut spec);

    spec.env_set.push(("HOME".into(), paths::home().to_string_lossy().into_owned()));
    spec.env_set.push(("PWD".into(), workdir.to_string_lossy().into_owned()));
    spec.env_set.push(("PATH".into(), inherited_path()));
    spec.env_set.push((SESSION_ENV.into(), id.to_string()));
    spec.env_set
        .push(("APEX_AGENT_SANDBOX".into(), req.sandbox.to_string()));
    spec.env_set
        .push(("TMPDIR".into(), scratch.to_string_lossy().into_owned()));
    for (k, v) in &req.env {
        spec.env_set.push((k.clone(), v.clone()));
    }
    for name in ["USER", "LOGNAME", "SHELL"] {
        if let Ok(val) = std::env::var(name) {
            spec.env_set.push((name.to_string(), val));
        }
    }

    let argv = sandbox::build_argv(&spec, &program, &args).map_err(SandboxRefused)?;
    let env = sandbox::resolved_env(&spec);

    // A confined session gets its environment from bwrap's --setenv, so the
    // process environment is only used for the unconfined path.
    let spawned = pty::spawn(&argv, &workdir, &env, true, size)
        .with_context(|| format!("starting {program}"))?;

    let info = SessionInfo {
        id,
        agent: adapter.id.to_string(),
        program: program.clone(),
        args: args.clone(),
        cwd: workdir.to_string_lossy().into_owned(),
        project: detected.as_ref().map(|p| p.root.clone()),
        project_name: detected.as_ref().map(|p| p.name.clone()),
        worktree: worktree_name,
        state: AgentState::Starting,
        detail: None,
        sandbox: req.sandbox,
        pid: spawned.pid,
        started: now_secs(),
        last_activity: now_secs(),
        exit_code: None,
        exit_signal: None,
        attached: 0,
        checkpoint: checkpoint_id,
        cols: size.cols,
        rows: size.rows,
    };

    let handle = {
        let mut reg = daemon.registry.lock().expect("registry lock");
        reg.insert(info.clone(), spawned.master, spawned.pid, spawned.pgid)
    };
    registry::write_record(&info);
    spawn_reader(Arc::clone(daemon), handle, id);

    Ok(info)
}

/// The `PATH` a session inherits.
///
/// Taken from the daemon's environment, which is the user's login environment,
/// so a toolchain the user installed to `~/.local/bin` still resolves. The
/// sandbox decides separately whether those directories are actually visible.
fn inherited_path() -> String {
    std::env::var("PATH").unwrap_or_else(|_| "/usr/local/bin:/usr/bin:/bin".to_string())
}

/// A sandbox refusal, carried so `dispatch` can map it to the right error kind.
#[derive(Debug)]
pub struct SandboxRefused(pub SandboxError);

impl std::fmt::Display for SandboxRefused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for SandboxRefused {}

/// Map a `start` failure to a response, keeping the sandbox distinction the
/// client needs in order to explain the escape hatch.
pub fn run_error(e: anyhow::Error) -> Response {
    // The whole chain, not just the innermost error: the sandbox refusal
    // carries the remedy ("re-run with --sandbox unrestricted") and the outer
    // context says which step was refused.
    if e.downcast_ref::<SandboxRefused>().is_some() {
        return Response::error(ErrorKind::SandboxUnavailable, format!("{e:#}"));
    }
    Response::error(ErrorKind::BadRequest, format!("{e:#}"))
}

/// Read a session's terminal until the process ends.
fn spawn_reader(daemon: Arc<Daemon>, handle: Handle, id: u32) {
    let name = format!("apex-agentd-s{id}");
    let worker = Arc::clone(&handle);
    let spawned = std::thread::Builder::new()
        .name(name)
        .spawn(move || reader_loop(&daemon, &worker));
    if spawned.is_err() {
        // Without a reader the session would produce no output and never be
        // reaped, which is worse than not having started it.
        let mut s = handle.lock().expect("session lock");
        registry::terminate(&mut s);
        s.set_exited(Some(-1), None);
        registry::write_record(&s.info);
    }
}

fn reader_loop(daemon: &Arc<Daemon>, handle: &Handle) {
    let (master, pid, id) = {
        let s = handle.lock().expect("session lock");
        (s.master, s.pid, s.info.id)
    };
    let mut buf = vec![0u8; 64 * 1024];

    loop {
        let readable = pty::wait_readable(master, POLL_INTERVAL_MS);

        let mut ended = false;
        if readable {
            match pty::read_nonblocking(master, &mut buf) {
                Ok(Some(0)) => {}
                Ok(Some(n)) => absorb(handle, &buf[..n]),
                // EOF or EIO: the child closed the terminal.
                Ok(None) => ended = true,
                Err(_) => ended = true,
            }
        }

        match pty::try_wait(pid) {
            pty::Wait::Running => {
                if ended {
                    // The terminal closed but the process is still around —
                    // it detached or handed the PTY to a child that exited.
                    // Keep waiting rather than reporting a session that is
                    // still burning CPU as finished.
                    std::thread::sleep(std::time::Duration::from_millis(100));
                    continue;
                }
                update_idle_state(handle);
            }
            pty::Wait::Exited(code) => {
                drain(handle, master, &mut buf);
                finish(daemon, handle, Some(code), None);
                break;
            }
            pty::Wait::Signalled(sig) => {
                drain(handle, master, &mut buf);
                finish(daemon, handle, None, Some(sig));
                break;
            }
            pty::Wait::Gone => {
                drain(handle, master, &mut buf);
                finish(daemon, handle, Some(-1), None);
                break;
            }
        }

        if handle.lock().expect("session lock").closing {
            break;
        }
    }

    let _ = id;
}

/// Push output through the scanner and into the session.
fn absorb(handle: &Handle, data: &[u8]) {
    let mut s = handle.lock().expect("session lock");
    let signals = s.scanner.feed(data);
    s.absorb(data);
    let next = logic::next_state(s.info.state, &signals, true, 0);
    let detail = signals
        .iter()
        .rev()
        .find_map(|sig| sig.detail().map(|d| d.to_string()));
    s.set_state(next, detail);
}

/// Re-evaluate state for a session that produced nothing this tick.
fn update_idle_state(handle: &Handle) {
    let mut s = handle.lock().expect("session lock");
    if !s.info.is_live() {
        return;
    }
    let idle = s.idle_secs();
    let next = logic::next_state(s.info.state, &[], false, idle);
    if next != s.info.state {
        s.set_state(next, None);
        // Recording only on a change keeps an idle session from rewriting its
        // record once a second for hours.
        registry::write_record(&s.info);
    }
}

/// Read whatever the terminal still holds after the process exited.
fn drain(handle: &Handle, master: libc::c_int, buf: &mut [u8]) {
    for _ in 0..64 {
        match pty::read_nonblocking(master, buf) {
            Ok(Some(0)) | Ok(None) | Err(_) => break,
            Ok(Some(n)) => absorb(handle, &buf[..n]),
        }
    }
}

/// Record the exit and release the terminal.
fn finish(daemon: &Arc<Daemon>, handle: &Handle, code: Option<i32>, signal: Option<i32>) {
    let (id, master) = {
        let mut s = handle.lock().expect("session lock");
        s.set_exited(code, signal);
        registry::write_record(&s.info);
        (s.info.id, s.master)
    };

    pty::close(master);
    {
        let mut s = handle.lock().expect("session lock");
        s.master = -1;
    }

    // The scratch directory is the session's, and nothing outside it should be
    // holding a path into it once the session is gone.
    let _ = std::fs::remove_dir_all(paths::scratch_dir(id));

    // Keep the record in the registry so `apex agent list` still shows the
    // outcome; `apex agent prune` is what clears it.
    let _ = daemon;
}

/// Turn a control connection into a session's terminal.
///
/// The response line goes out first, then the connection carries only PTY
/// bytes in both directions.
pub fn handle_attach(
    daemon: &Arc<Daemon>,
    mut writer: UnixStream,
    reader: BufReader<UnixStream>,
    id: u32,
    cols: u16,
    rows: u16,
    replay: usize,
) -> Result<()> {
    let Some(handle) = daemon.registry.lock().expect("registry lock").get(id) else {
        let resp = Response::error(ErrorKind::NoSuchSession, format!("no session {id}"));
        return write_response(&mut writer, &resp);
    };

    let master = {
        let s = handle.lock().expect("session lock");
        if !s.info.is_live() {
            let resp = Response::error(
                ErrorKind::SessionExited,
                format!(
                    "session {id} has already {}; use `apex agent logs {id}` to read its output",
                    s.info.exit_summary().unwrap_or_else(|| "exited".into())
                ),
            );
            return write_response(&mut writer, &resp);
        }
        s.master
    };

    write_response(&mut writer, &Response::Attached { id })?;

    // Adopt the attaching terminal's size, so the agent repaints correctly.
    let size = WinSize { cols, rows }.or_fallback();
    if pty::resize(master, size).is_ok() {
        let mut s = handle.lock().expect("session lock");
        s.info.cols = size.cols;
        s.info.rows = size.rows;
    }

    // Register the output direction before replaying, so nothing produced
    // between the two is lost.
    {
        let mirror = writer.try_clone().context("cloning for output mirroring")?;
        let mut s = handle.lock().expect("session lock");
        s.attach(mirror, replay)?;
    }

    // This thread becomes the input pump. It ends when the client shuts down
    // its write half (a detach) or disconnects.
    let mut source = reader.into_inner();
    let mut buf = [0u8; 8192];
    loop {
        let n = match source.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        };
        let live_master = {
            let s = handle.lock().expect("session lock");
            if !s.info.is_live() {
                break;
            }
            s.master
        };
        if live_master < 0 || pty::write_all(live_master, &buf[..n]).is_err() {
            break;
        }
    }

    // Detaching removes this client and nothing else: the session keeps
    // running, which is the whole point of the runtime owning the PTY.
    detach(&handle, &writer);
    Ok(())
}

/// Remove one attached client from a session.
fn detach(handle: &Handle, stream: &UnixStream) {
    use std::os::unix::io::AsRawFd;
    let target = stream.as_raw_fd();
    let mut s = handle.lock().expect("session lock");
    // Compare by the peer's identity rather than by index: another client may
    // have detached while this one was reading.
    s.attachers.retain(|a| !same_peer(a.as_raw_fd(), target));
    s.info.attached = s.attachers.len() as u32;
}

/// Whether two descriptors refer to the same socket.
///
/// `try_clone` produces a different descriptor number for the same open file
/// description, so the numbers cannot be compared directly; `st_ino` on a
/// socket identifies the socket itself.
fn same_peer(a: libc::c_int, b: libc::c_int) -> bool {
    fn inode(fd: libc::c_int) -> Option<u64> {
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        // Safe: fstat writes one struct we own.
        if unsafe { libc::fstat(fd, &mut st) } != 0 {
            return None;
        }
        Some(st.st_ino)
    }
    match (inode(a), inode(b)) {
        (Some(x), Some(y)) => x == y,
        _ => false,
    }
}

fn write_response(writer: &mut UnixStream, response: &Response) -> Result<()> {
    let mut line = serde_json::to_string(response)?;
    line.push('\n');
    writer.write_all(line.as_bytes())?;
    writer.flush().ok();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sandbox_refusal_keeps_its_error_kind() {
        let e = anyhow::Error::new(SandboxRefused(SandboxError::MissingBwrap));
        let resp = run_error(e);
        assert_eq!(
            resp.as_error().map(|(k, _)| k),
            Some(ErrorKind::SandboxUnavailable)
        );
    }

    #[test]
    fn a_sandbox_refusal_keeps_its_remedy_through_the_error_chain() {
        let e = anyhow::Error::new(SandboxRefused(SandboxError::TiocstiEnabled));
        let resp = run_error(e);
        let (_, message) = resp.as_error().expect("error");
        assert!(message.contains("unrestricted"), "{message}");
    }

    #[test]
    fn an_ordinary_failure_is_a_bad_request_not_a_sandbox_problem() {
        let e = anyhow::anyhow!("working directory /nope does not exist");
        let resp = run_error(e);
        assert_eq!(resp.as_error().map(|(k, _)| k), Some(ErrorKind::BadRequest));
    }

    #[test]
    fn a_cloned_socket_is_recognised_as_the_same_peer() {
        use std::os::unix::io::AsRawFd;
        let (a, _b) = UnixStream::pair().unwrap();
        let clone = a.try_clone().unwrap();
        assert_ne!(a.as_raw_fd(), clone.as_raw_fd(), "expected a new descriptor");
        assert!(same_peer(a.as_raw_fd(), clone.as_raw_fd()));

        let (c, _d) = UnixStream::pair().unwrap();
        assert!(!same_peer(a.as_raw_fd(), c.as_raw_fd()));
    }
}
