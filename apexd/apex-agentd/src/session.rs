//! Starting sessions, reading their terminals, and attaching to them.

use std::io::{BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use apex_agent_core::adapter;
use apex_agent_core::checkpoint;
use apex_agent_core::hook;
use apex_agent_core::client::SESSION_ENV;
use apex_agent_core::paths;
use apex_agent_core::config;
use apex_agent_core::policy::{NetworkPolicy, PolicyError};
use apex_agent_core::profile;
use apex_agent_core::project;
use apex_agent_core::protocol::{AgentState, ErrorKind, Response, RunRequest, SessionInfo};
use apex_agent_core::sandbox::{self, EgressBridge, SandboxError, SandboxSpec, BRIDGE_PORT};
use apex_agent_core::session as logic;
use apex_agent_core::term::WinSize;

use crate::egress;
use crate::peer::Peer;
use crate::pty;
use crate::registry::{self, now_secs, Handle};
use crate::Daemon;

/// How long the reader thread waits for output before re-evaluating state.
///
/// One second: fast enough that the idle transition lands on time, slow enough
/// that an idle session costs one wakeup per second rather than a spin.
const POLL_INTERVAL_MS: i32 = 1000;

/// Start a session.
///
/// `peer` is the connection's credentials, and it is what establishes the
/// session's origin (§7). It is passed rather than looked up for the same
/// reason the privilege verbs take it: the origin has to come from the
/// kernel's view of who connected, and a handler that could reach for the
/// request instead would eventually do so.
pub fn start(daemon: &Arc<Daemon>, req: RunRequest, peer: Option<Peer>) -> Result<SessionInfo> {
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

    // Resolve the six permission dimensions before anything is created.
    //
    // Normalised first, so the record and the enforcement agree about what the
    // session has — a `strict` request carries the client's default `open`
    // network, and storing that would list an isolated session as networked.
    // Validated second, so a client that skipped its own checks, or one built
    // against a newer vocabulary, is refused here rather than granted a
    // dimension nothing in this build enforces.
    let policy = req.policy.normalised();
    // The allowlist is read fresh rather than taken from the daemon's cached
    // configuration. A destination policy that only changed on a daemon
    // restart is one people widen once and never narrow again, and this is the
    // daemon reading its own user's file — nothing the session can write.
    let allowlist = config::Config::load().allowlist();
    policy.validate_for(&allowlist).map_err(PolicyRefused)?;

    // Dimension 1 is the agent's own, and only the adapter knows whether this
    // one can express it. Refused rather than dropped: a `--agent-bypass` that
    // silently did nothing would leave the user believing confirmations were
    // off.
    if let Some(why) = adapter.refuses_native_mode(policy.native, is_root()) {
        bail!("{why}");
    }

    // Fail closed before anything is created: a session must never start with
    // weaker confinement than was asked for.
    sandbox::preflight(policy.sandbox).map_err(SandboxRefused)?;

    // Dimension 6's companion: where this session will be driven from.
    //
    // Established here, once, from the connection that asked for it — and
    // never afterwards from anything the session says, because a session that
    // could name its own origin could name the local one. `--origin` on the
    // command line is a DECLARATION and is checked against the observation
    // before it is accepted; a local origin is refused whatever is sent.
    //
    // Refused rather than defaulted when it cannot be established: the default
    // is `local-terminal`, which is what §7 reserves root for.
    let who = crate::privilege::origin(daemon, peer);
    let session_origin = crate::privilege::for_new_session(&who, req.request_origin)
        .map_err(OriginRefused)?;

    // ── dimension 3: the grant, before anything exists to clean up ─────────
    //
    // §3.3: root is delegated, not inherited. A session that asks for either
    // elevated mode gets one only after a human at this machine has said so,
    // and the whole of that decision happens here — before the worktree, the
    // checkpoint, the reserved id and the PTY, so a refused password leaves
    // nothing behind and a refused ORIGIN never reaches the password at all.
    //
    // The order inside `authorise_grant` is the security property; it is
    // written out there. The TTL is checked first, so a typo in `--ttl` fails
    // in front of the user instead of after a password dialog they then find
    // out was pointless.
    let wanted_grant = policy.needs_grant();
    if wanted_grant.is_none() && req.ttl_ms.is_some() {
        // A `--ttl` with nothing to bound is a user who believes they asked
        // for something they did not. Refused rather than ignored.
        bail!(
            "--ttl bounds a system-access grant, and this session is not asking for one; add \
             `--system-access session` or `--unsafe-everything`, or drop the --ttl"
        );
    }
    let authorised = match wanted_grant {
        None => None,
        Some(kind) => {
            let ttl_ms = apex_agent_core::grant::ttl_for(kind, req.ttl_ms)
                .map_err(|e| TtlRefused(e.to_string()))?;
            let (grant_origin, proof) = crate::privilege::authorise_grant(
                daemon,
                &who,
                peer,
                kind,
                "ask for a system-access grant",
            )
            .map_err(|e| GrantRefused(e.to_string()))?;
            Some((kind, ttl_ms, grant_origin, proof))
        }
    };

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
        // Reported, not swallowed. `let _ =` here is how a bug in
        // project::remember stayed invisible for as long as it did: every real
        // project slug contained slashes, remember failed on the missing
        // parent directories for all of them, and nothing ever said so — so
        // `apex project list` was simply always empty. Failing to remember a
        // project must not stop a session from starting, but it must be
        // audible.
        if let Err(e) = project::remember(proj) {
            eprintln!("apex-agentd: could not record project {}: {e:#}", proj.name);
        }
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

    // Reserved on disk, not merely counted in memory: an id that collides with
    // a record left by an earlier daemon overwrites that session's transcript.
    // Held as a guard so a failure between here and the spawn gives the id back
    // instead of leaving an empty record behind.
    let reservation = daemon.registry.lock().expect("registry lock").allocate()?;
    let id = reservation.id();
    let scratch = paths::scratch_dir(id);
    // Not best-effort: the sandbox binds this path read-write and sets TMPDIR
    // to it. If it cannot be created, or cannot be made private, the session
    // would start with an unexpected scratch directory and fail later in a much
    // harder place to diagnose.
    paths::ensure_private_dir(&scratch)
        .with_context(|| format!("preparing the session scratch directory {}", scratch.display()))?;

    // §6.1: the settings document that subscribes Claude to its own lifecycle,
    // written into the scratch directory the sandbox already binds. Best-effort
    // by design — a session whose hooks could not be installed reports its
    // state from the PTY scanner, which is the fallback §6.1 keeps and not a
    // reason to refuse to start. `hook_settings` says what went wrong, once.
    let hook_settings = install_hook_settings(adapter, &scratch);

    // §12: the shim's directory goes first on the session's PATH, so a skill's
    // own `git push` reaches the broker without the skill knowing there is one.
    // Only for a confined session — an unconfined one has the user's own git,
    // the user's own credential helper, and no reason to be routed anywhere.
    let session_bin = policy
        .sandbox
        .is_confined()
        .then(|| install_git_shim(&scratch))
        .flatten();
    let mut extra = extra;
    if let Some(path) = hook_settings.as_ref() {
        let mut with_hooks = adapter.hook_settings_args(path);
        with_hooks.append(&mut extra);
        extra = with_hooks;
    }

    let args = adapter.build_args(policy.native, req.prompt.as_deref(), &extra);
    let size = WinSize {
        cols: req.cols,
        rows: req.rows,
    }
    .or_fallback();

    // Build the sandbox.
    let mut spec = SandboxSpec::new(policy, paths::home(), paths::runtime_dir());
    // /run is masked, which takes the resolv.conf symlink target with it. Bind
    // the target back or the session has no DNS at all.
    spec.run_ro = sandbox::resolv_binds();
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
    // Read-only, and that is the one part of this bridge an agent cannot undo.
    // `build_argv` applies the read-only allowlist after the scratch bind, so
    // this lands on top of a directory the session can otherwise write: the
    // hook subscriptions are fixed at spawn. It does not make the hook
    // authoritative — `--bare`, a nested agent and `disableAllHooks` in the
    // agent's own writable `~/.claude` all still silence it — which is why
    // nothing downstream is allowed to depend on the hook having run.
    if let Some(path) = hook_settings.as_ref() {
        spec.ro.push(path.clone());
    }
    // Read-only for the same reason the hook settings are: the scratch is
    // bound writable, and a shim the session could rewrite is one it could
    // point at something else. It holds no credential either way — this is
    // tidiness, not a boundary.
    if let Some(bin) = session_bin.as_ref() {
        spec.ro.push(bin.clone());
    }

    // P0-003's first criterion, enforced at spawn rather than left to whether
    // somebody has run the migration yet. `settings.json` is bound read-only
    // above; this puts a copy of it, with the credential values gone, on top.
    if let Some((from, at)) = install_redacted_settings(adapter, &scratch, &spec.home) {
        // The copy itself goes on the read-only list too, for the reason the
        // hook settings do: the scratch directory is bound writable, so a copy
        // that were writable through its own path would be one the session
        // could edit — and both files should have the same story.
        spec.ro.push(from.clone());
        spec.ro_at.push((from, at));
    }

    // The profile's writable directories have to exist before the sandbox binds
    // them: bwrap binds with `-try`, and a `-try` for a path that is not there
    // is a no-op, so the entry would resolve inside the tmpfs that masks $HOME.
    // The agent would write its transcripts and its trusted-directory list into
    // memory and lose both at exit — which reads as the agent forgetting, not
    // as a sandbox that dropped a mount. Not fatal: a session with a
    // session-local plugin cache still runs, and refusing to start over a
    // directory nobody has needed yet would be worse.
    if spec.policy.sandbox.is_confined() {
        if let Some(profile) = adapter.profile() {
            match profile.prepare(&spec.home) {
                Ok(made) if !made.is_empty() => eprintln!(
                    "apex-agentd: created {} missing {} profile director{}",
                    made.len(),
                    adapter.id,
                    if made.len() == 1 { "y" } else { "ies" }
                ),
                Ok(_) => {}
                Err(e) => eprintln!(
                    "apex-agentd: could not prepare the {} profile ({e}); \
                     anything it writes below a missing directory stays in the session",
                    adapter.id
                ),
            }
        }
    }

    // The allowlist's only route out. Started before the session, so an agent
    // that resolves a proxy on its first line finds one there; and inside the
    // scratch directory, which is already bound read-write, so it needs no
    // mount of its own and cannot disturb the ordering the `/run` mask
    // depends on. `finish` deletes that directory and the proxy stops with it.
    //
    // The `?` is the fail-closed half: a session that asked for an allowlist
    // and whose proxy did not start does not run with the host's network, and
    // does not run at all.
    if policy.effective_network() == NetworkPolicy::Allowlist {
        let program = bridge_program()?;
        let socket = scratch.join("egress.sock");
        egress::start(id, &socket, allowlist.clone())
            .with_context(|| format!("starting the egress proxy for session {id}"))?;
        spec.egress = Some(EgressBridge {
            program,
            socket,
            port: BRIDGE_PORT,
        });
    }

    spec.env_set.push(("HOME".into(), paths::home().to_string_lossy().into_owned()));
    spec.env_set.push(("PWD".into(), workdir.to_string_lossy().into_owned()));
    let path = match session_bin.as_ref() {
        Some(bin) => format!("{}:{}", bin.display(), inherited_path()),
        None => inherited_path(),
    };
    spec.env_set.push(("PATH".into(), path));
    spec.env_set.push((SESSION_ENV.into(), id.to_string()));
    spec.env_set
        .push(("APEX_AGENT_SANDBOX".into(), policy.sandbox.to_string()));
    // The other five dimensions a session may usefully know about itself. A
    // hook that wants to say "this session has no network" reads this rather
    // than guessing from the sandbox name, which stopped being the authority
    // on the network when the dimensions were split.
    spec.env_set
        .push(("APEX_AGENT_NATIVE_MODE".into(), policy.native.to_string()));
    spec.env_set
        .push(("APEX_AGENT_NETWORK".into(), policy.effective_network().to_string()));
    spec.env_set
        .push(("TMPDIR".into(), scratch.to_string_lossy().into_owned()));
    // Where the control socket is, because a session that cannot name its
    // runtime directory cannot find the socket it was just handed. Resolved
    // here rather than read later, so the name the session is given and the
    // path the socket is bound at cannot be two different strings.
    //
    // Without this a confined session gets `--clearenv` and nothing to replace
    // it, so `paths::runtime_dir` falls back to `/run/user/<uid>` — right on an
    // ordinary login and wrong for any daemon with an `XDG_RUNTIME_DIR` of its
    // own, where every `apex agent event` and every hook reports "the agent
    // runtime is not running" from inside a session the runtime is
    // demonstrably running.
    //
    // Before `req.env`, with the other variables the daemon owns: `resolved_env`
    // is first-seen-wins, and a caller redirecting a session's reporting at
    // another socket is not something a `--env` flag should be able to do.
    spec.env_set.push((
        "XDG_RUNTIME_DIR".into(),
        sandbox::real_target(&spec.runtime_dir)
            .to_string_lossy()
            .into_owned(),
    ));
    for (k, v) in &req.env {
        spec.env_set.push((k.clone(), v.clone()));
    }
    for name in ["USER", "LOGNAME", "SHELL"] {
        if let Ok(val) = std::env::var(name) {
            spec.env_set.push((name.to_string(), val));
        }
    }

    // bwrap will not mount on a path that traverses a symlink ("Can't mount on
    // symlink destination"), and an atomic OS reaches every home through one:
    // /root -> var/roothome, /home -> var/home. So the tmpfs that masks $HOME,
    // and any bind under a symlinked home, abort the session unless the target
    // is resolved to its real path first. The logical paths still exist inside
    // the sandbox as symlinks to the resolved ones, so $HOME and the working
    // directory keep resolving. cwd is deliberately not resolved: it becomes a
    // --chdir, which follows symlinks, not a mount point.
    spec.home = sandbox::real_target(&spec.home);
    spec.runtime_dir = sandbox::real_target(&spec.runtime_dir);
    if !spec.scratch.as_os_str().is_empty() {
        spec.scratch = sandbox::real_target(&spec.scratch);
    }
    if !spec.control_socket.as_os_str().is_empty() {
        spec.control_socket = sandbox::real_target(&spec.control_socket);
    }
    if let Some(bridge) = spec.egress.as_mut() {
        // Resolved for the same reason as the scratch directory it sits in:
        // the bridge connects to this path from inside the sandbox, where the
        // bind was made against the real one.
        bridge.socket = sandbox::real_target(&bridge.socket);
    }
    for p in spec.rw.iter_mut() {
        *p = sandbox::real_target(p);
    }
    for p in spec.ro.iter_mut() {
        *p = sandbox::real_target(p);
    }
    for p in spec.mask.iter_mut() {
        *p = sandbox::real_target(p);
    }
    for (from, at) in spec.ro_at.iter_mut() {
        *from = sandbox::real_target(from);
        *at = sandbox::real_target(at);
    }

    // The grant is minted now that the session has an id to be bound to, and
    // before the process starts: §3.3 wants the grant "bound to a concrete
    // agent session", and a grant issued after the agent was already running
    // would have a window in which the session existed and the record did not.
    let issued = authorised.map(|(kind, ttl_ms, grant_origin, proof)| {
        daemon.grants.issue(
            proof,
            kind,
            id,
            adapter.id,
            detected.as_ref().map(|p| p.root.as_str()),
            ttl_ms,
            grant_origin,
            apex_agent_core::request::now_ms(),
        )
    });

    let argv = sandbox::build_argv(&spec, &program, &args).map_err(SandboxRefused)?;
    let env = sandbox::resolved_env(&spec);

    // A confined session gets its environment from bwrap's --setenv, so the
    // process environment is only used for the unconfined path.
    let spawned = pty::spawn(&argv, &workdir, &env, true, policy.no_new_privs(), size)
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
        paused: false,
        policy,
        request_origin: Some(session_origin.origin),
        origin_source: Some(session_origin.source),
        grant: issued.as_ref().map(|g| g.id),
        grant_expires_ms: issued.as_ref().map(|g| g.expires_ms),
        // Nothing has been heard from the agent yet. Claude fills this in on
        // its first hook event; an agent that never publishes one leaves it
        // absent, which reads as "not reported" rather than as a mode.
        native_observed: None,
        // Empty, not absent: this daemon has the graph, and a session that has
        // delegated nothing yet must be distinguishable from one whose runtime
        // cannot tell. See `SessionInfo::children`.
        children: Vec::new(),
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
    // The spec as built, not as it could be rebuilt later: §6.2 must judge a
    // tool call against the confinement the session is actually running under.
    handle.lock().expect("session lock").confinement = Some(Box::new(registry::Confinement {
        spec,
        allowlist,
    }));
    registry::write_record(&info);
    // The session owns its record now, so the id stops being a reservation.
    reservation.commit();
    spawn_reader(Arc::clone(daemon), handle, id);

    Ok(info)
}

/// Write a credential-free copy of the agent's settings file, and say where it
/// goes and what it replaces.
///
/// The file Claude reads for its model, its hooks and its theme is also the
/// file it reads an `env` block from, and that block is applied to every tool
/// the session runs. A PAT put there reaches the session's tools whatever the
/// sandbox does with the environment it started the process in, because it does
/// not arrive through the environment at all. So the copy, and a bind of the
/// copy over the original.
///
/// `None` when there is nothing to strip, which is the common case and the one
/// the machine should end up in permanently once `apex secret migrate` has run.
/// Also `None` when the copy could not be written — best-effort in the same
/// sense the hook bridge is, and for the same reason: it is logged, and a
/// session that would otherwise start is not refused over it. What that costs
/// is stated where it happens.
fn install_redacted_settings(
    adapter: &adapter::Adapter,
    scratch: &Path,
    home: &Path,
) -> Option<(PathBuf, PathBuf)> {
    let profile = adapter.profile()?;
    let entry = profile.entry_for(profile::Base::Root, Path::new(SETTINGS_FILE))?;
    let real = profile.entry_path(home, entry);
    let raw = std::fs::read(&real).ok()?;
    let (redacted, names) = profile::settings_without_credentials(&raw)?;

    let copy = scratch.join(REDACTED_SETTINGS_FILE);
    if let Err(e) = std::fs::write(&copy, redacted) {
        eprintln!(
            "apex-agentd: could not write {} ({e}), so {} starts with {} as it is — \
             {} reach the session's tools",
            copy.display(),
            adapter.id,
            real.display(),
            names.join(", ")
        );
        return None;
    }
    eprintln!(
        "apex-agentd: {} is bound without {} — store credentials with \
         `apex secret add` and remove them from that file with `apex secret migrate`",
        real.display(),
        names.join(", ")
    );
    Some((copy, real))
}

/// Write the `git` a confined session finds first, and say where its directory
/// is.
///
/// §12: the user's existing skills keep using normal tools. A skill runs
/// `git push`, this is what runs, and it asks the broker to perform the push
/// rather than needing a credential of its own. Everything it does not
/// recognise it execs `/usr/bin/git` for.
///
/// A shell script rather than a symlink or a copy: `apex` has to be invoked as
/// `apex git-shim -- …`, and the session's `PATH` entry has to be called `git`.
/// Two lines of `sh` are the whole of it, and being readable matters more here
/// than being clever — the agent can read this file, and what it says is the
/// truth about what happens to its git commands.
///
/// `None` when the `apex` binary could not be found or the file could not be
/// written. The session then has no shim, `git` is the real one, and a push to
/// a private remote fails to authenticate the way it does today. Nothing is
/// less safe: the shim holds no credential and enforces nothing.
fn install_git_shim(scratch: &Path) -> Option<PathBuf> {
    let apex = apex_program()?;
    let bin = scratch.join(SESSION_BIN);
    if let Err(e) = std::fs::create_dir_all(&bin) {
        eprintln!(
            "apex-agentd: could not create {} ({e}), so git is not brokered in this session",
            bin.display()
        );
        return None;
    }
    let shim = bin.join("git");
    let script = format!(
        "#!/bin/sh\n\
         # Written by apex-agentd. `git` for a managed session: push, fetch and\n\
         # ls-remote go through the broker, everything else execs /usr/bin/git.\n\
         exec {} git-shim -- \"$@\"\n",
        apex.display()
    );
    if let Err(e) = std::fs::write(&shim, script) {
        eprintln!(
            "apex-agentd: could not write {} ({e}), so git is not brokered in this session",
            shim.display()
        );
        return None;
    }
    use std::os::unix::fs::PermissionsExt;
    if let Err(e) = std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755)) {
        eprintln!("apex-agentd: could not make {} executable ({e})", shim.display());
        return None;
    }
    Some(bin)
}

/// The directory inside the session scratch that goes first on its `PATH`.
const SESSION_BIN: &str = "bin";

/// The agent settings file a credential can arrive through.
const SETTINGS_FILE: &str = "settings.json";

/// What the redacted copy is called inside the session scratch. Not
/// `settings.json`: the scratch is bound writable and visible, and two files
/// with the same name and different contents is how somebody debugging this
/// ends up reading the wrong one.
const REDACTED_SETTINGS_FILE: &str = "claude-settings-redacted.json";

/// Write the hook subscriptions for a session, and say where they went.
///
/// `None` when this adapter has no way to be told about a settings file — every
/// agent but Claude today — and also when the write failed or the `apex` binary
/// could not be found. All three are the same thing downstream: no hooks, so
/// the PTY scanner decides state, exactly as it does for an agent nobody has
/// integrated. The failures are logged because a silently unintegrated Claude
/// looks identical to a working one until somebody measures the state.
fn install_hook_settings(adapter: &adapter::Adapter, scratch: &Path) -> Option<PathBuf> {
    if !adapter.hooks {
        return None;
    }
    let apex = match apex_program() {
        Some(p) => p,
        None => {
            eprintln!(
                "apex-agentd: no `apex` on PATH, so {} runs without its hook bridge and \
                 reports state from terminal output",
                adapter.id
            );
            return None;
        }
    };
    let path = hook::settings_path(scratch);
    let document = hook::settings_json(&apex).to_string();
    match std::fs::write(&path, document) {
        Ok(()) => Some(path),
        Err(e) => {
            eprintln!(
                "apex-agentd: writing {} failed ({e}), so {} runs without its hook bridge",
                path.display(),
                adapter.id
            );
            None
        }
    }
}

/// The `apex` binary a hook command will exec, as an absolute path.
///
/// Absolute because the hook runs inside the sandbox, whose `PATH` is the
/// daemon's but whose filesystem is not: a bare `apex` would resolve against
/// directories the home tmpfs has masked.
///
/// The daemon's own sibling first: `apex` and `apex-agentd` are built and
/// shipped together, and a hook command that ran a different build from the
/// daemon it reports to is the one pairing guaranteed to be wrong. Skipped when
/// a session could not exec it anyway — the masked home and the private `/tmp`
/// are the two places a confined process cannot see, which is the test
/// [`bridge_program`] already applies. Then `/usr/bin/apex`, where the image
/// puts it, and finally the daemon's `PATH`.
fn apex_program() -> Option<PathBuf> {
    let sibling = std::env::current_exe().ok().and_then(|exe| {
        let p = exe.parent()?.join("apex");
        let reachable = !p.starts_with("/tmp") && !p.starts_with(paths::home());
        (p.is_file() && reachable).then_some(p)
    });
    if sibling.is_some() {
        return sibling;
    }
    let installed = PathBuf::from("/usr/bin/apex");
    if installed.is_file() {
        return Some(installed);
    }
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|d| d.join("apex"))
        .find(|p| p.is_file())
}

/// The `PATH` a session inherits.
///
/// Taken from the daemon's environment, which is the user's login environment,
/// so a toolchain the user installed to `~/.local/bin` still resolves. The
/// sandbox decides separately whether those directories are actually visible.
fn inherited_path() -> String {
    std::env::var("PATH").unwrap_or_else(|_| "/usr/local/bin:/usr/bin:/bin".to_string())
}

/// This runtime's own binary, which is also the egress bridge.
///
/// Checked for existence rather than trusted, because `/proc/self/exe` answers
/// with a path that ends in " (deleted)" once the file behind it is gone, and
/// because a session's view of the filesystem is not the daemon's: `$HOME` and
/// `/tmp` are masked inside the sandbox, so a development build living in
/// either is a path the session cannot exec. Both cases would otherwise
/// surface as a session that starts and dies with status 127.
fn bridge_program() -> Result<PathBuf> {
    let program = std::env::current_exe()
        .context("finding this runtime's own binary, which is the egress bridge")?;
    if !program.is_file() {
        bail!(
            "the egress bridge for an allowlisted session is this runtime's own binary, and \
             {} is not there any more; restart the runtime, or use `--network offline`",
            program.display()
        );
    }
    // `/tmp` only. `/var/tmp` is covered by the read-only root like the rest of
    // the filesystem, and a build there is reachable — which is what makes a
    // live test of this mode possible at all.
    if program.starts_with("/tmp") {
        bail!(
            "an allowlisted session cannot reach {}, because a confined session's /tmp is a \
             fresh tmpfs; install the runtime, or use `--network offline`",
            program.display()
        );
    }
    if program.starts_with(paths::home()) {
        bail!(
            "an allowlisted session cannot reach {}, because a confined session's home is \
             masked; install the runtime, or use `--network offline`",
            program.display()
        );
    }
    Ok(program)
}

/// Whether this runtime is running as root.
///
/// Only consulted for the agent's own permission mode, which some upstream
/// CLIs refuse under root. Nothing in APEX's own policy branches on it: root
/// is a dimension, not a euid.
fn is_root() -> bool {
    // Safe: geteuid takes no arguments and cannot fail.
    unsafe { libc::geteuid() == 0 }
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

/// A permission-dimension refusal, carried for the same reason.
///
/// Kept distinct from [`SandboxRefused`] because the two need different
/// remedies: a sandbox refusal is answered with `--sandbox unrestricted`, and
/// telling somebody denied a system grant to loosen their sandbox would be
/// advice that both fails and makes them less safe.
#[derive(Debug)]
pub struct PolicyRefused(pub PolicyError);

impl std::fmt::Display for PolicyRefused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for PolicyRefused {}

/// An origin that could not be established, or a declaration that was refused.
///
/// Its own type, because the remedy is neither of the two above: nothing about
/// the sandbox or the six dimensions will help, and the message already says
/// which `/proc` read failed or which restriction the declaration tried to
/// drop.
#[derive(Debug)]
pub struct OriginRefused(pub String);

impl std::fmt::Display for OriginRefused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for OriginRefused {}

/// A system-access grant that was refused, or a TTL that was not issuable.
///
/// Its own type for the same reason as the three above: the remedies are
/// different and specific. A grant refused because the connection came from
/// inside a session is answered by asking from a terminal; one refused because
/// the origin was remote is answered by approving locally; one refused because
/// polkit said no is answered by getting the password right. None of them is
/// answered by changing the sandbox, which is what a shared error type would
/// eventually suggest.
#[derive(Debug)]
pub struct GrantRefused(pub String);

impl std::fmt::Display for GrantRefused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for GrantRefused {}

/// A dimension refusal that is a sentence rather than a [`PolicyError`].
///
/// The TTL bounds live in `grant.rs`, which knows nothing about `PolicyError`
/// and should not: a TTL is not one of the six dimensions, it is a parameter
/// of a grant.
#[derive(Debug)]
pub struct TtlRefused(pub String);

impl std::fmt::Display for TtlRefused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for TtlRefused {}

/// Map a `start` failure to a response, keeping the distinctions the client
/// needs in order to explain what to do next.
pub fn run_error(e: anyhow::Error) -> Response {
    // The whole chain, not just the innermost error: the sandbox refusal
    // carries the remedy ("re-run with --sandbox unrestricted") and the outer
    // context says which step was refused.
    if e.downcast_ref::<SandboxRefused>().is_some() {
        return Response::error(ErrorKind::SandboxUnavailable, format!("{e:#}"));
    }
    if e.downcast_ref::<PolicyRefused>().is_some() {
        return Response::error(ErrorKind::PolicyRefused, format!("{e:#}"));
    }
    if e.downcast_ref::<OriginRefused>().is_some() {
        return Response::error(ErrorKind::PermissionDenied, format!("{e:#}"));
    }
    // A refused grant is a permission answer, so it gets the kind a client
    // branches on for one. A refused TTL is the user asking for something
    // out of bounds, which is a bad request.
    if e.downcast_ref::<GrantRefused>().is_some() {
        return Response::error(ErrorKind::PermissionDenied, format!("{e:#}"));
    }
    if e.downcast_ref::<TtlRefused>().is_some() {
        return Response::error(ErrorKind::PolicyRefused, format!("{e:#}"));
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
    let in_flight = s.tool_in_flight();
    let next = logic::next_state(s.info.state, &signals, true, 0, in_flight);
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
    let in_flight = s.tool_in_flight();
    let next = logic::next_state(s.info.state, &[], false, idle, in_flight);
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
    fn a_policy_refusal_is_not_reported_as_a_sandbox_problem() {
        // The remedy differs. `SandboxUnavailable` means "re-run with
        // --sandbox unrestricted", which for a confined break-glass request
        // is the opposite of what the user should do.
        let e = anyhow::Error::new(PolicyRefused(PolicyError::BreakGlassCannotBeConfined(
            apex_agent_core::protocol::SandboxPolicy::Project,
        )));
        let resp = run_error(e);
        assert_eq!(resp.as_error().map(|(k, _)| k), Some(ErrorKind::PolicyRefused));
        let (_, message) = resp.as_error().expect("error");
        assert!(message.contains("no_new_privs"), "{message}");
    }

    #[test]
    fn a_refused_grant_is_a_permission_answer_and_a_refused_ttl_is_not() {
        // The two failures a `--unsafe-everything` run can hit, and a client
        // branches on the kind: `PermissionDenied` means somebody has to
        // authorise this, `PolicyRefused` means the request itself was out of
        // bounds and no amount of authorising will help.
        let denied = run_error(anyhow::Error::new(GrantRefused(
            "this connection belongs to session 3".into(),
        )));
        assert_eq!(
            denied.as_error().map(|(k, _)| k),
            Some(ErrorKind::PermissionDenied)
        );

        let ttl = run_error(anyhow::Error::new(TtlRefused(
            "break-glass caps at 1h".into(),
        )));
        assert_eq!(ttl.as_error().map(|(k, _)| k), Some(ErrorKind::PolicyRefused));
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
