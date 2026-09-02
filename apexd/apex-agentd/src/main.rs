//! `apex-agentd` — the APEX agent runtime.
//!
//! Unprivileged, one per user, started by `systemd --user`. It owns the PTYs
//! that agent sessions run on, the sandbox each session is confined by, the
//! project and checkpoint state around them, and the control socket the `apex`
//! CLI and APEX Shell talk to.
//!
//! It deliberately holds no privilege and never talks to `apexd`. Agent
//! orchestration inside the privileged daemon is the thing the roadmap
//! forbids; when a session eventually needs a system change, that will be a
//! narrow request to the frozen `org.apexos.Apexd1` surface, made by the user's
//! own `apex` invocation, not a right this process holds.
//!
//! Threading: one thread accepting connections, one per connection, one per
//! session. Sessions are few and the work is I/O, so this is simpler and more
//! predictable than an async runtime — and the blocking `read` on a PTY is
//! exactly what the kernel is good at.

mod broker;
mod peer;
mod privilege;
mod pty;
mod registry;
mod session;

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use apex_agent_core::adapter;
use apex_agent_core::config::Config;
use apex_agent_core::paths;
use apex_agent_core::protocol::{
    ErrorKind, Request, Response, SessionInfo, PROTOCOL_VERSION,
};
use apex_agent_core::request;

use crate::registry::Registry;

/// Everything the connection threads share.
pub struct Daemon {
    pub registry: Mutex<Registry>,
    pub config: Mutex<Config>,
}

impl Daemon {
    fn new() -> Daemon {
        Daemon {
            registry: Mutex::new(Registry::new()),
            config: Mutex::new(Config::load()),
        }
    }
}

fn main() {
    if let Err(e) = run() {
        eprintln!("apex-agentd: {e:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    // Writing to an attached client that has gone away must not kill the
    // daemon. Every write site already handles the error.
    // Safe: setting a signal disposition before any thread exists.
    unsafe { libc::signal(libc::SIGPIPE, libc::SIG_IGN) };

    let socket = paths::control_socket();
    let dir = socket
        .parent()
        .context("the control socket path has no parent directory")?;
    paths::ensure_private_dir(dir)?;
    paths::ensure_private_dir(&paths::state_dir())?;
    privilege::ensure_dirs();

    // Sessions never outlive the daemon that owned their PTYs, so any record
    // left claiming to be running is from a previous life.
    registry::reconcile_stale_records();

    let listener = bind(&socket)?;
    let daemon = Arc::new(Daemon::new());

    // Shutdown runs on its own thread waiting on a blocked signal set, rather
    // than in a handler: stopping sessions means taking locks and iterating a
    // map, neither of which is legal in a signal handler.
    block_termination_signals();
    spawn_signal_thread(Arc::clone(&daemon), socket.clone());

    eprintln!(
        "apex-agentd: listening on {} ({} adapters)",
        socket.display(),
        adapter::ADAPTERS.len()
    );

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let daemon = Arc::clone(&daemon);
                std::thread::Builder::new()
                    .name("apex-agentd-conn".into())
                    .spawn(move || {
                        if let Err(e) = serve(&daemon, stream) {
                            eprintln!("apex-agentd: connection ended: {e:#}");
                        }
                    })
                    .context("spawning a connection thread")?;
            }
            Err(e) => eprintln!("apex-agentd: accept failed: {e}"),
        }
    }
    Ok(())
}

/// Bind the control socket, replacing a stale one.
///
/// A socket file left by a daemon that was killed cannot be bound over, but it
/// also cannot be connected to. Probing before unlinking is what stops this
/// from stealing the socket out from under a daemon that is genuinely running.
fn bind(path: &Path) -> Result<UnixListener> {
    if path.exists() {
        if UnixStream::connect(path).is_ok() {
            anyhow::bail!(
                "another apex-agentd is already listening on {}",
                path.display()
            );
        }
        std::fs::remove_file(path)
            .with_context(|| format!("removing the stale socket {}", path.display()))?;
    }
    let listener = UnixListener::bind(path)
        .with_context(|| format!("binding the control socket {}", path.display()))?;

    // The socket is the control plane for the user's agents; nobody else on
    // the machine may connect to it. The parent directory is already 0700.
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .with_context(|| format!("restricting {}", path.display()))?;
    Ok(listener)
}

/// Block the termination signals in every thread, so only the signal thread
/// receives them.
fn block_termination_signals() {
    // Safe: building and installing a signal mask for this process.
    unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        libc::sigaddset(&mut set, libc::SIGTERM);
        libc::sigaddset(&mut set, libc::SIGINT);
        libc::sigaddset(&mut set, libc::SIGHUP);
        libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());
    }
}

fn spawn_signal_thread(daemon: Arc<Daemon>, socket: PathBuf) {
    std::thread::Builder::new()
        .name("apex-agentd-signal".into())
        .spawn(move || {
            let mut sig: libc::c_int = 0;
            // Safe: sigwait blocks until one of the masked signals arrives and
            // writes its number into an int we own.
            unsafe {
                let mut set: libc::sigset_t = std::mem::zeroed();
                libc::sigemptyset(&mut set);
                libc::sigaddset(&mut set, libc::SIGTERM);
                libc::sigaddset(&mut set, libc::SIGINT);
                libc::sigaddset(&mut set, libc::SIGHUP);
                libc::sigwait(&set, &mut sig);
            }
            eprintln!("apex-agentd: signal {sig}, stopping sessions");
            shutdown(&daemon, &socket);
            std::process::exit(0);
        })
        .ok();
}

/// Stop every session and remove the socket.
fn shutdown(daemon: &Daemon, socket: &Path) {
    let handles = {
        let reg = daemon.registry.lock().expect("registry lock");
        reg.list()
    };
    for handle in handles {
        let mut s = handle.lock().expect("session lock");
        registry::terminate(&mut s);
        registry::write_record(&s.info);
    }
    let _ = std::fs::remove_file(socket);
}

/// Serve one control connection: newline-delimited JSON requests until the
/// client goes away, or until an `Attach` turns it into a raw PTY pipe.
fn serve(daemon: &Arc<Daemon>, stream: UnixStream) -> Result<()> {
    // Read the peer credentials ONCE, from the accepted socket, before any
    // request is parsed. The kernel filled them in at connect(2) and they
    // cannot change for the life of the connection — whereas anything read out
    // of a request line is whatever the client chose to send.
    let creds = peer::credentials(&stream);

    let mut reader = BufReader::new(stream.try_clone().context("cloning the connection")?);
    let mut writer = stream;

    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line)?;
        if n == 0 {
            return Ok(());
        }
        let line = line.trim_end();
        if line.is_empty() {
            continue;
        }

        let request: Request = match serde_json::from_str(line) {
            Ok(r) => r,
            Err(e) => {
                respond(
                    &mut writer,
                    &Response::error(ErrorKind::BadRequest, format!("unparseable request: {e}")),
                )?;
                continue;
            }
        };

        // Attach is the one verb that does not return to this loop: the
        // connection stops being a control channel and becomes the session's
        // terminal.
        if let Request::Attach {
            id,
            cols,
            rows,
            replay,
        } = request
        {
            return session::handle_attach(daemon, writer, reader, id, cols, rows, replay);
        }

        let response = dispatch(daemon, request, creds);
        respond(&mut writer, &response)?;
    }
}

fn respond(writer: &mut UnixStream, response: &Response) -> Result<()> {
    let mut line = serde_json::to_string(response)?;
    line.push('\n');
    writer.write_all(line.as_bytes())?;
    writer.flush().ok();
    Ok(())
}

/// Handle every verb except `Attach`.
///
/// `creds` is the connection's peer credentials, or `None` when the kernel
/// would not report them. It is passed rather than looked up so that no handler
/// can accidentally consult the request for identity instead.
fn dispatch(daemon: &Arc<Daemon>, request: Request, creds: Option<peer::Peer>) -> Response {
    match request {
        Request::Hello => {
            let cfg = daemon.config.lock().expect("config lock");
            Response::Hello {
                version: PROTOCOL_VERSION,
                agents: adapter::ids().into_iter().map(|s| s.to_string()).collect(),
                default_agent: cfg.default_agent.clone(),
            }
        }

        Request::Run(req) => match session::start(daemon, req) {
            Ok(info) => Response::Session(Box::new(info)),
            Err(e) => session::run_error(e),
        },

        Request::List => Response::Sessions {
            sessions: collect_sessions(daemon),
        },

        Request::Info { id } => match live_info(daemon, id) {
            Some(info) => Response::Session(Box::new(info)),
            None => match registry::historical_records()
                .into_iter()
                .find(|i| i.id == id)
            {
                Some(info) => Response::Session(Box::new(info)),
                None => no_such_session(id),
            },
        },

        Request::Attach { .. } => Response::error(
            ErrorKind::Internal,
            "attach is handled before dispatch and must never reach it",
        ),

        Request::Resize { id, cols, rows } => {
            let Some(handle) = lookup(daemon, id) else {
                return no_such_session(id);
            };
            let mut s = handle.lock().expect("session lock");
            if !s.info.is_live() {
                return Response::error(
                    ErrorKind::SessionExited,
                    format!("session {id} has already exited"),
                );
            }
            let size = apex_agent_core::term::WinSize { cols, rows }.or_fallback();
            match pty::resize(s.master, size) {
                Ok(()) => {
                    s.info.cols = size.cols;
                    s.info.rows = size.rows;
                    Response::Ok
                }
                Err(e) => Response::error(ErrorKind::Internal, e.to_string()),
            }
        }

        Request::Signal { id, signal } => {
            let Some(number) = apex_agent_core::session::signal_number(&signal) else {
                return Response::error(
                    ErrorKind::BadRequest,
                    format!("unknown signal {signal:?}; use int, term, kill, stop or cont"),
                );
            };
            let Some(handle) = lookup(daemon, id) else {
                return no_such_session(id);
            };
            let mut s = handle.lock().expect("session lock");
            if !s.info.is_live() {
                return Response::error(
                    ErrorKind::SessionExited,
                    format!("session {id} has already exited"),
                );
            }
            match pty::signal_group(s.pgid, number) {
                Ok(()) => {
                    // A stopped session is neither working nor waiting on the
                    // user, and nothing will produce output to correct the
                    // state later, so record it now. Recorded AFTER the signal
                    // succeeded: a flag set before the kill would claim a
                    // session was paused when the signal failed.
                    if number == libc::SIGSTOP {
                        s.info.paused = true;
                        s.info.detail = Some("paused".to_string());
                    } else if number == libc::SIGCONT {
                        s.info.paused = false;
                        s.info.detail = None;
                    }
                    Response::Ok
                }
                Err(e) => Response::error(ErrorKind::Internal, e.to_string()),
            }
        }

        Request::Event { id, state, detail } => {
            let Some(parsed) = apex_agent_core::protocol::AgentState::parse(&state) else {
                return Response::error(
                    ErrorKind::BadRequest,
                    format!("unknown state {state:?}; expected one of \
                             working, waiting_for_user, permission_request, complete, failed"),
                );
            };
            let Some(handle) = lookup(daemon, id) else {
                return no_such_session(id);
            };
            let mut s = handle.lock().expect("session lock");
            s.set_state(parsed, detail);
            registry::write_record(&s.info);
            Response::Ok
        }

        Request::Logs { id, bytes } => {
            // Bound the request so a client cannot ask the daemon to read a
            // 32 MiB transcript into memory by accident.
            let bytes = bytes.min(registry::LOG_LIMIT_BYTES as usize);
            match registry::read_log(id, bytes) {
                Ok(text) => Response::Logs { id, text },
                Err(e) => Response::error(ErrorKind::NoSuchSession, e.to_string()),
            }
        }

        Request::Remove { id } => {
            let handle = lookup(daemon, id);
            if let Some(handle) = handle {
                let live = {
                    let s = handle.lock().expect("session lock");
                    s.info.is_live()
                };
                if live {
                    return Response::error(
                        ErrorKind::BadRequest,
                        format!("session {id} is still running; stop it first with `apex agent kill {id}`"),
                    );
                }
                daemon.registry.lock().expect("registry lock").remove(id);
            }
            registry::forget_record(id);
            Response::Ok
        }

        Request::Prune => {
            let handles = daemon.registry.lock().expect("registry lock").list();
            let mut removed = Vec::new();
            for handle in handles {
                let s = handle.lock().expect("session lock");
                if !s.info.is_live() {
                    removed.push(s.info.id);
                }
            }
            {
                let mut reg = daemon.registry.lock().expect("registry lock");
                for id in &removed {
                    reg.remove(*id);
                }
            }
            for info in registry::historical_records() {
                if info.state.is_terminal() {
                    registry::forget_record(info.id);
                }
            }
            for id in removed {
                registry::forget_record(id);
            }
            Response::Ok
        }

        // ── privilege requests ──────────────────────────────────────────────
        // Every one of these takes `creds` and none of them takes a session id
        // from the wire.
        Request::PrivilegeRequest { verb, args, reason } => {
            privilege::file(daemon, creds, &verb, &args, &reason)
        }

        Request::Requests => privilege::list(),

        Request::Decide { id, decision } => match request::Decision::parse(&decision) {
            Some(d) => privilege::decide(daemon, creds, id, d),
            None => Response::error(
                ErrorKind::BadRequest,
                format!("'{decision}' is not a decision; use once, project or deny"),
            ),
        },

        Request::RequestExecuted { id, exit_code } => privilege::executed(id, exit_code),

        Request::Grants => privilege::grants(),

        Request::Revoke { project, key } => {
            privilege::revoke(daemon, creds, &project, key.as_deref())
        }

        // ── the secret broker ───────────────────────────────────────────────
        Request::SecretUse {
            service,
            capability,
            remote,
            branch,
            project,
        } => broker::use_capability(
            daemon,
            creds,
            &service,
            &capability,
            &remote,
            branch.as_deref(),
            project.as_deref(),
        ),

        Request::SecretGrant {
            project,
            service,
            capability,
            revoke,
        } => broker::grant(daemon, creds, &project, &service, &capability, revoke),

        Request::SecretGrants => broker::grants(),
    }
}

fn lookup(daemon: &Arc<Daemon>, id: u32) -> Option<registry::Handle> {
    daemon.registry.lock().expect("registry lock").get(id)
}

fn no_such_session(id: u32) -> Response {
    Response::error(ErrorKind::NoSuchSession, format!("no session {id}"))
}

fn live_info(daemon: &Arc<Daemon>, id: u32) -> Option<SessionInfo> {
    let handle = lookup(daemon, id)?;
    let info = handle.lock().expect("session lock").info.clone();
    Some(info)
}

/// Live sessions plus persisted records for ones this daemon no longer owns.
fn collect_sessions(daemon: &Arc<Daemon>) -> Vec<SessionInfo> {
    let live: Vec<SessionInfo> = daemon
        .registry
        .lock()
        .expect("registry lock")
        .list()
        .into_iter()
        .map(|h| h.lock().expect("session lock").info.clone())
        .collect();

    let mut out = live;
    let known: std::collections::HashSet<u32> = out.iter().map(|i| i.id).collect();
    for info in registry::historical_records() {
        if !known.contains(&info.id) {
            out.push(info);
        }
    }
    out.sort_by_key(|i| i.id);
    out
}
