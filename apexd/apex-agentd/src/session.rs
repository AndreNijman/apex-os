//! Starting sessions, reading their terminals, and attaching to them.

use std::io::{BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use apex_agent_core::adapter;
use apex_agent_core::checkpoint;
use apex_agent_core::hook;
use apex_agent_core::mcpconf;
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
use crate::privilege::Caller;
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
pub fn start(daemon: &Arc<Daemon>, req: RunRequest, caller: &Caller) -> Result<SessionInfo> {
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

    // Resolve the permission dimensions before anything is created.
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
    let runtime_config = config::Config::load();
    let allowlist = runtime_config.allowlist();
    policy
        .validate_for(&allowlist, &runtime_config.connector_allow)
        .map_err(PolicyRefused)?;

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
    let who = crate::privilege::origin(daemon, caller);
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
    // §P1-037. Checked here, with the TTL, and for the same reason: a caller
    // who asked for two mechanisms that cannot both apply should find that out
    // in front of their own terminal, not after a capsule has been created.
    crate::disposable::check(
        req.disposable,
        policy.sandbox.is_confined(),
        req.copy_out.as_deref(),
        req.worktree.is_some(),
        req.checkpoint,
    )?;

    let wanted_grant = policy.needs_grant();
    if wanted_grant.is_none() && req.ttl_ms.is_some() {
        // A `--ttl` with nothing to bound is a user who believes they asked
        // for something they did not. Refused rather than ignored.
        bail!(
            "--ttl bounds a system-access grant, and this session is not asking for one; add \
             `--system-access session` or `--unsafe-everything`, or drop the --ttl"
        );
    }
    if wanted_grant.is_none() && req.capabilities.is_some() {
        // The same refusal as a `--ttl` with nothing to bound, and for the
        // same reason: a caller who narrowed a grant they did not ask for
        // believes they asked for something they did not.
        bail!(
            "--capabilities narrows a system-access grant, and this session is not asking for \
             one; add `--system-access session`, or drop the --capabilities"
        );
    }
    let authorised = match wanted_grant {
        None => None,
        Some(kind) => {
            let ttl_ms = apex_agent_core::grant::ttl_for(kind, req.ttl_ms)
                .map_err(|e| TtlRefused(e.to_string()))?;
            // Ahead of `authorise_grant` for the reason the TTL is: a typo in
            // `--capabilities` should fail in front of the person who typed
            // it, not after a password dialog they then find out was
            // pointless.
            let capabilities =
                apex_agent_core::grant::capabilities_for(kind, req.capabilities.as_deref())
                    .map_err(|e| CapabilitiesRefused(e.to_string()))?;
            let (grant_origin, proof) = crate::privilege::authorise_grant(
                daemon,
                &who,
                caller,
                kind,
                "ask for a system-access grant",
                // §7's second column, for the session being started (P0-014).
                //
                // The policy is THIS request's dimension 6 — normalised and
                // validated above — and not the daemon's configured default,
                // which `Config::policy()` only assembles for a session
                // started without the flags. A session started with
                // `--origin-policy remote` overrides it, and reading the
                // config here would mean the per-session dimension governs
                // nothing.
                //
                // `scope: None` is not an oversight: this call is deliberately
                // ahead of `registry.allocate()`, so that a refused password
                // leaves no reserved id behind, and there is therefore no id
                // for the key to have signed over. `Challenge.session` is
                // `Option<u32>` for exactly this caller.
                &crate::privilege::Elevating {
                    policy: policy.origin,
                    scope: None,
                    ttl_ms,
                    factor: req.second_factor.as_ref(),
                },
            )
            .map_err(|e| GrantRefused(e.to_string()))?;
            Some((kind, ttl_ms, capabilities, grant_origin, proof))
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
    let hook_settings = install_hook_settings(adapter, &scratch, detected.as_ref().map(|p| std::path::Path::new(&p.root)));

    // §12: the shim's directory goes first on the session's PATH, so a skill's
    // own `git push` reaches the broker without the skill knowing there is one.
    // Only for a confined session — an unconfined one has the user's own git,
    // the user's own credential helper, and no reason to be routed anywhere.
    let session_bin = policy
        .sandbox
        .is_confined()
        .then(|| install_git_shim(&scratch))
        .flatten();
    // §10.2 and P1-026/P1-028: the connectors this session gets, decided by the
    // runtime and handed over as a file, rather than whatever the agent finds
    // on the machine. Best-effort in the same sense the hook settings are — a
    // session whose configuration could not be written starts with the
    // connectors it would have had — but not silently: `install_mcp_config`
    // says what went wrong and the session record says the configuration is
    // absent, so nothing downstream reports a confinement that did not happen.
    let mcp_config = install_mcp_config(
        adapter,
        &scratch,
        &workdir,
        &policy,
        &runtime_config.connector_allow,
    );

    let mut extra = extra;
    if let Some(path) = mcp_config.as_ref() {
        let mut with_mcp = adapter.mcp_config_args(path);
        with_mcp.append(&mut extra);
        extra = with_mcp;
    }
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
    // Read-only for the reason the hook settings are, and here it is the whole
    // point rather than tidiness: the scratch is bound writable, so a curated
    // MCP configuration the session could rewrite is one it could put its own
    // unwrapped definitions back into — which is the hole this closes.
    if let Some(path) = mcp_config.as_ref() {
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
    let issued = authorised.map(|(kind, ttl_ms, capabilities, grant_origin, proof)| {
        daemon.grants.issue(
            proof,
            kind,
            id,
            adapter.id,
            detected.as_ref().map(|p| p.root.as_str()),
            ttl_ms,
            capabilities,
            grant_origin,
            apex_agent_core::request::now_ms(),
        )
    });

    // §P1-037. A disposable session's PTY child is the disposable ENGINE, and
    // the adapter runs inside the capsule it creates. `build_argv` is not in
    // that path at all: the policy is `unrestricted` here — `disposable::
    // check` refused any other above — so bwrap would add nothing, and if it
    // were added it would confine the container client rather than the agent.
    //
    // The engine's own EXIT/INT/TERM traps are what remove the environment,
    // so there is no teardown here to get wrong: killing this child tears the
    // capsule down, which is exactly the behaviour `apex agent kill` should
    // have.
    let capsule = req.disposable.then(|| crate::disposable::name_for(id));
    let argv = if req.disposable {
        // The engine's own overrides, set EXPLICITLY rather than relied on to
        // arrive by inheritance. They decide which directory it removes
        // recursively and which program it drives, and the inheritance that
        // carries them today is a bug elsewhere that a correct fix would take
        // away — see `disposable::engine_env`.
        for pair in crate::disposable::engine_env(|n| std::env::var(n).ok()) {
            spec.env_set.push(pair);
        }
        crate::disposable::argv(id, &workdir, req.copy_out.as_deref(), &program, &args)?
    } else {
        sandbox::build_argv(&spec, &program, &args).map_err(SandboxRefused)?
    };
    // §P2-011. A pure exec-chain prefix when a budget is configured, and the
    // identity function when one is not — which is the default, so an
    // unbudgeted session's argv is byte-identical to what it was before this
    // line existed. `spec.runtime_dir` and not the daemon's own: the child's
    // `XDG_RUNTIME_DIR` is what decides whether systemd-run can reach a user
    // manager, and they are not always the same directory.
    let argv = crate::budget::wrap(argv, id, req.disposable, &spec.runtime_dir, &cfg)
        .map_err(BudgetRefused)?;
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
        // Which remote device asked for this session, when the connection
        // that asked named one. Carried from the connection rather than from
        // the `Run` request: a session that could name its own actor could
        // name somebody else's phone.
        actor: who.actor.clone(),
        capsule: capsule.clone(),
        grant: issued.as_ref().map(|g| g.id),
        grant_expires_ms: issued.as_ref().map(|g| g.expires_ms),
        // Nothing has been heard from the agent yet. Claude fills this in on
        // its first hook event; an agent that never publishes one leaves it
        // absent, which reads as "not reported" rather than as a mode.
        native_observed: None,
        // Empty, not absent: this daemon has the graph, and a session that has
        // delegated nothing yet must be distinguishable from one whose runtime
        // cannot tell. See `SessionInfo::children`.
        telemetry: None,
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
        injected: 0,
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
    install_session_bin(scratch, &apex_program()?, Path::new(TOOL_SHIM_DIR))
}

/// The half of [`install_git_shim`] that names what it installs FROM.
///
/// Split out so a test can drive the real wiring: which shims end up in the
/// one directory that goes first on a session's `PATH` is the property, and a
/// test that called each installer separately would prove each works and not
/// that either is reached.
fn install_session_bin(scratch: &Path, apex: &Path, tools: &Path) -> Option<PathBuf> {
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
    install_tool_shims_from(&bin, tools);
    Some(bin)
}

/// Where the image installs P1-012's `wrangler` and `terraform` shim.
///
/// A directory rather than the script, because what goes in a session's `bin`
/// is one symlink per tool: the shim reads `argv[0]` to decide which tool it is
/// standing in for.
pub const TOOL_SHIM_DIR: &str = "/usr/libexec/apex/tools";

/// §13.4's tool half, put where a session's own `wrangler` will find it.
///
/// ## Why here and not in `/etc/profile.d`
///
/// There is a profile.d drop-in that does the same thing, and it is **not what
/// makes this work for an agent**. `/etc/profile.d/*.sh` is read by a *login*
/// shell. An agent's tool calls are `bash -c '…'` — non-login,
/// non-interactive — and never read it. The drop-in is for a person who opens
/// a terminal inside a managed session; this is for the agent, and this is the
/// one that matters for P1-012's "existing skills can continue invoking normal
/// tools".
///
/// Symlinks into the same `bin` the git shim uses, so there is one directory
/// at the front of the session's `PATH` rather than two, and so an unconfined
/// session gets neither — for the reason the git shim gives: an unconfined
/// session has the user's own tools and the user's own credentials, and no
/// reason to be routed anywhere.
///
/// Best-effort, like the hook settings and for the same reason: a session
/// whose tool shims could not be installed is a session where `wrangler`
/// reaches the real binary with no credential and says so. That is a worse
/// experience, not a hole — nothing here is a boundary, and the note in the
/// shim itself says so.
fn install_tool_shims_from(bin: &Path, source: &Path) {
    for tool in ["wrangler", "terraform"] {
        let from = source.join(tool);
        if !from.exists() {
            // The image did not install it. Not an error: a development build
            // running from a checkout has no /usr/libexec/apex.
            continue;
        }
        let link = bin.join(tool);
        let _ = std::fs::remove_file(&link);
        if let Err(e) = std::os::unix::fs::symlink(&from, &link) {
            eprintln!(
                "apex-agentd: could not link {} ({e}), so {tool} is not brokered in this session",
                link.display()
            );
        }
    }
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
fn install_hook_settings(
    adapter: &adapter::Adapter,
    scratch: &Path,
    project: Option<&Path>,
) -> Option<PathBuf> {
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
    // The user's own status line, read here rather than inside the settings
    // document, because `hook::settings_json` is pure and this is a filesystem
    // question. See `statusline::overlay` for why the presentation keys have
    // to travel with it and why the command must not.
    let status = apex_agent_core::statusline::user_status_line(&paths::home(), project);
    let path = hook::settings_path(scratch);
    let document = hook::settings_json(&apex, status.as_ref()).to_string();
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

/// Write the curated MCP configuration for a session, and say where it went.
///
/// `None` when this adapter cannot be told to use one configuration and ignore
/// the rest, when the write failed, and — the case worth naming — when the
/// session asked for nothing to be reduced and nothing needed confining. All
/// three end the same way downstream: the agent loads the definitions it finds,
/// exactly as it did before this existed.
///
/// That last skip is not laziness. A curated document is `--strict-mcp-config`,
/// and strict means the session's own later edits to `~/.claude.json` stop
/// reaching the agent. Paying that for a session with nothing to confine and
/// nothing to remove would be a behaviour change bought for nothing — so the
/// file is written when the policy reduces something, or when there is
/// third-party executable content to wrap, and not otherwise.
fn install_mcp_config(
    adapter: &adapter::Adapter,
    scratch: &Path,
    workdir: &Path,
    policy: &apex_agent_core::policy::AgentPolicy,
    allow: &[String],
) -> Option<PathBuf> {
    if !adapter.strict_mcp {
        return None;
    }
    let home = paths::home();
    let defs = mcpconf::read(&home, Some(workdir));
    let approval = mcpconf::approvals(&home, Some(workdir));
    // The same resolver the hook bridge uses, and deliberately not a second
    // one: a wrapper that pointed at a different build from the daemon it
    // reports to is the one pairing guaranteed to be wrong.
    let apex = apex_program();
    let curated = mcpconf::curate(&defs, &approval, policy.connectors, allow, apex.as_deref());

    let wraps = curated.confined() > 0;
    if !policy.connectors.reduces() && !wraps {
        return None;
    }

    let path = mcpconf::config_path(scratch);
    let document = curated.document.to_string();
    match std::fs::write(&path, document) {
        Ok(()) => {
            if curated.dropped() > 0 || wraps {
                eprintln!(
                    "apex-agentd: {} connector(s) for this session, {} of them sandboxed \
                     ({} of those by their own definition, so only by this file for the rest), \
                     {} removed",
                    curated.kept(),
                    curated.confined(),
                    curated.confined_everywhere(),
                    curated.dropped()
                );
            }
            Some(path)
        }
        Err(e) => {
            // Loud, because the quiet version of this is a session that looks
            // curated and is not. Every connector the policy meant to remove
            // is reachable after this line.
            eprintln!(
                "apex-agentd: writing {} failed ({e}), so {} starts with the connectors it \
                 finds and NOT the ones this session asked for",
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

/// A `--capabilities` list this build will not issue a grant for (P0-007).
///
/// Its own type rather than folded into [`TtlRefused`], following the same
/// rule the four above follow: the remedies are different and specific. A
/// refused TTL is answered by asking for a shorter window; a refused
/// capability list is answered by spelling the verb correctly, or by not
/// narrowing a break-glass grant that has no verbs to narrow.
#[derive(Debug)]
pub struct CapabilitiesRefused(pub String);

impl std::fmt::Display for CapabilitiesRefused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for CapabilitiesRefused {}

/// A resource budget that cannot be delivered (§P2-011).
///
/// Its own type rather than a bare `anyhow!` because the kind is the point: a
/// budget refusal is never fixed by authorising anything, and it is never the
/// caller's request that is wrong — it is the machine or the configuration —
/// so `BadRequest` would send the user to look in the wrong place. The rule
/// behind every one of these is the same: a budget that is silently not
/// applied is worse than a session that did not start.
#[derive(Debug)]
pub struct BudgetRefused(pub String);

impl std::fmt::Display for BudgetRefused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for BudgetRefused {}

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
    if e.downcast_ref::<CapabilitiesRefused>().is_some() {
        return Response::error(ErrorKind::PolicyRefused, format!("{e:#}"));
    }
    if e.downcast_ref::<BudgetRefused>().is_some() {
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

/// What came of writing into a session's terminal.
///
/// Three outcomes and not a `Result`, because two of the three are ordinary
/// answers a client acts on differently: an exited session means "pick another
/// target", an I/O failure means "the terminal is broken".
pub enum Input {
    Written,
    Exited,
    Failed(String),
}

/// Write bytes into a live session's terminal.
///
/// This is the input pump of [`handle_attach`] with the loop taken off: the
/// daemon owns the PTY master, so a client with nothing to display does not
/// need to become the terminal to be heard. `Request::Input` is the caller.
///
/// The master descriptor is copied out and the lock RELEASED before the write,
/// which is the whole reason this is a function rather than four lines in the
/// dispatch arm. `pty::write_all` blocks when the agent is not draining its
/// input: it waits for writability rather than spinning, so a TUI that has
/// paused its reader can hold the write open indefinitely. Holding the session
/// lock across that would freeze every other verb for the session, including
/// the `Signal` a user reaches for precisely when an agent has stopped
/// reading — the deadlock would be worst at the only moment it mattered. The
/// pump above takes the lock the same way for the same reason; `Resize` is the
/// one that holds it across the syscall, and `pty::resize` cannot block.
pub fn write_input(handle: &Handle, data: &[u8]) -> Input {
    let master = {
        let s = handle.lock().expect("session lock");
        if !s.info.is_live() {
            return Input::Exited;
        }
        s.master
    };
    // A live session with a closed master is a race, not a state: the reaper
    // sets the fd to -1 as the child goes away. Writing to -1 would be an
    // EBADF reported as a broken terminal, when the truth is the same as the
    // check above.
    if master < 0 {
        return Input::Exited;
    }
    match pty::write_all(master, data) {
        Ok(()) => Input::Written,
        Err(e) => Input::Failed(e.to_string()),
    }
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
    use std::os::fd::AsRawFd;
    use std::os::unix::io::RawFd;
    use std::path::PathBuf;

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

    /// A real session on a real PTY, running a shell that reads one line and
    /// says what it got.
    ///
    /// Deliberately not a mock and not a socket pair. What `Request::Input`
    /// has to be right about is the LINE DISCIPLINE — whether the byte it
    /// appends for `--submit` is the byte that ends a line — and a socket
    /// carries every byte equally, so it would prove the plumbing and hide the
    /// only interesting question. `pty::spawn` is the same call a session is
    /// started with, so the terminal modes are the shipped ones.
    fn a_session_reading_one_line() -> Option<(registry::Handle, pty::Spawned, PathBuf)> {
        let script = "read line; echo \"got:[$line]\"; sleep 30";
        let argv = vec!["/bin/sh".to_string(), "-c".to_string(), script.to_string()];
        let spawned = pty::spawn(
            &argv,
            std::path::Path::new("/tmp"),
            &[],
            false,
            true,
            apex_agent_core::term::WinSize { cols: 80, rows: 24 },
        )
        .ok()?;

        let dir = std::env::temp_dir().join(format!(
            "apex-agentd-input-{}-{}",
            std::process::id(),
            spawned.pid
        ));
        let mut reg = registry::Registry::with_store(dir.clone());
        let mut info = sample_live_info(1);
        info.pid = spawned.pid;
        let handle = reg.insert(info, spawned.master, spawned.pid, spawned.pgid);
        Some((handle, spawned, dir))
    }

    fn sample_live_info(id: u32) -> apex_agent_core::protocol::SessionInfo {
        use apex_agent_core::protocol::{AgentState, SessionInfo};
        SessionInfo {
            id,
            agent: "generic".into(),
            program: "sh".into(),
            args: vec![],
            cwd: "/tmp".into(),
            project: None,
            project_name: None,
            worktree: None,
            state: AgentState::Working,
            detail: None,
            paused: false,
            policy: apex_agent_core::AgentPolicy::default(),
            request_origin: Some(apex_agent_core::policy::RequestOrigin::LocalTerminal),
            origin_source: Some(apex_agent_core::origin::OriginSource::Observed),
            grant: None,
            grant_expires_ms: None,
            native_observed: None,
            pid: 0,
            started: 0,
            last_activity: 0,
            exit_code: None,
            exit_signal: None,
            checkpoint: None,
            cols: 80,
            rows: 24,
            attached: 0,
            actor: None,
            telemetry: None,
            children: vec![],
            injected: 0,
            capsule: None,
        }
    }

    /// Drain the master for up to `ms`, stopping early once `marker` is seen.
    fn read_until(master: RawFd, marker: &str, ms: u64) -> String {
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(ms);
        let mut seen = Vec::new();
        while std::time::Instant::now() < deadline {
            if !pty::wait_readable(master, 50) {
                continue;
            }
            let mut buf = [0u8; 4096];
            match pty::read_nonblocking(master, &mut buf) {
                Ok(Some(n)) if n > 0 => seen.extend_from_slice(&buf[..n]),
                Ok(_) => {}
                Err(_) => break,
            }
            if String::from_utf8_lossy(&seen).contains(marker) {
                break;
            }
        }
        String::from_utf8_lossy(&seen).to_string()
    }

    #[test]
    fn text_written_into_a_session_arrives_and_only_submit_ends_the_line() {
        let Some((handle, spawned, dir)) = a_session_reading_one_line() else {
            // No PTY available (a container without /dev/pts). Skipping is
            // correct here and a false pass is not, so it says so.
            eprintln!("skipping: pty::spawn failed on this machine");
            return;
        };

        // Let the shell reach `read` before anything is typed, or the bytes
        // land before there is a reader and the test proves the timing rather
        // than the write.
        std::thread::sleep(std::time::Duration::from_millis(200));

        // Phase 1: the words, with no terminator. This is what `apex agent
        // input` does WITHOUT --submit, and what the shell's push-to-talk
        // route does today.
        match write_input(&handle, b"run the tests") {
            Input::Written => {}
            Input::Exited => panic!("the session was reported as exited"),
            Input::Failed(e) => panic!("the write failed: {e}"),
        }
        let echoed = read_until(spawned.master, "run the tests", 2000);
        assert!(
            echoed.contains("run the tests"),
            "the text never reached the terminal: {echoed:?}"
        );
        assert!(
            !echoed.contains("got:["),
            "the line was submitted without --submit: {echoed:?}"
        );

        // Phase 2: the carriage return alone. What this proves is that the
        // terminator is what turns written bytes into a line the agent acts
        // on, which is the property `--submit` sells. What it does NOT prove
        // is that CR is the only byte that would: measured on this machine,
        // CR and LF both end the line here, because ICRNL is on by default in
        // cooked mode. The CR is chosen for raw-mode TUIs, and that case is
        // out of reach of this fixture. Said plainly rather than left to be
        // inferred from a passing assertion.
        match write_input(&handle, b"\r") {
            Input::Written => {}
            other => panic!(
                "the submit write failed: {}",
                match other {
                    Input::Failed(e) => e,
                    _ => "session reported exited".to_string(),
                }
            ),
        }
        let after = read_until(spawned.master, "got:[", 3000);
        assert!(
            after.contains("got:[run the tests]"),
            "the carriage return did not end the line: {after:?}"
        );

        // Tidy: the fixture sleeps 30s, so it is killed rather than waited on.
        pty::signal_group(spawned.pgid, libc::SIGKILL).ok();
        pty::close(spawned.master);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_exited_session_is_reported_as_exited_and_not_as_a_broken_terminal() {
        // The two failures a caller branches on differently: "pick another
        // target" against "the terminal is broken". A dead session that came
        // back as an I/O error would send the shell's push-to-talk route
        // looking for a fault in the PTY layer.
        let dir = std::env::temp_dir().join(format!("apex-agentd-input-dead-{}", std::process::id()));
        let mut reg = registry::Registry::with_store(dir.clone());
        let mut info = sample_live_info(2);
        info.exit_code = Some(0);
        assert!(!info.is_live());
        // A VALID descriptor, so a write would genuinely succeed if the live
        // check were dropped. With -1 here the test would pass on the fd guard
        // and prove nothing about the state check.
        let (a, _b) = UnixStream::pair().unwrap();
        let handle = reg.insert(info, a.as_raw_fd(), 0, 0);
        assert!(matches!(write_input(&handle, b"x"), Input::Exited));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_live_session_whose_master_is_already_closed_is_exited_too() {
        // The reaper sets `master` to -1 as the child goes away, so a session
        // can be marked live for the moment between the two. Writing to -1
        // would be EBADF surfaced as `Internal`, which reads as a bug in the
        // runtime rather than as a session that has gone.
        let dir = std::env::temp_dir().join(format!("apex-agentd-input-fd-{}", std::process::id()));
        let mut reg = registry::Registry::with_store(dir.clone());
        let handle = reg.insert(sample_live_info(3), -1, 0, 0);
        assert!(matches!(write_input(&handle, b"x"), Input::Exited));
        std::fs::remove_dir_all(&dir).ok();
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

    /// P1-012's first criterion, at the only place that can deliver it.
    ///
    /// There is an `/etc/profile.d` drop-in that puts the tool shims on PATH,
    /// and it is NOT what makes this work for an agent: profile.d is read by a
    /// login shell, and an agent's tool calls are `bash -c '…'`. The session's
    /// `bin` directory is what goes first on its `PATH`, so this is where a
    /// skill's own `wrangler deploy` either finds the broker or does not.
    ///
    /// Mutation: drop the `install_tool_shims` call from `install_git_shim`.
    /// Red.
    #[test]
    fn a_session_gets_the_tool_shims_on_the_path_its_own_commands_use() {
        let root = std::env::temp_dir().join(format!(
            "apex-toolshim-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let source = root.join("image");
        std::fs::create_dir_all(&source).expect("source");
        for tool in ["wrangler", "terraform"] {
            std::fs::write(source.join(tool), "#!/bin/sh\nexit 0\n").expect("shim");
        }
        // A test that installed from an empty directory would pass without
        // linking anything, so the fixture has to be real first.
        assert!(source.join("wrangler").exists());

        // The REAL wiring, not the installer on its own: what a session gets
        // is whatever ends up in the one directory that goes first on its
        // PATH, and a test that called each installer separately would prove
        // each works rather than that either is reached.
        let scratch = root.join("scratch");
        std::fs::create_dir_all(&scratch).expect("scratch");
        let apex = root.join("apex");
        std::fs::write(&apex, "#!/bin/sh\nexit 0\n").expect("apex");
        let bin = install_session_bin(&scratch, &apex, &source).expect("a session bin");

        // git first, because that is what this directory has always been for
        // and a regression there would be the louder failure.
        assert!(bin.join("git").exists(), "the git shim is gone");

        for tool in ["wrangler", "terraform"] {
            let link = bin.join(tool);
            assert!(
                link.exists(),
                "{tool} is not on the session's own PATH, so a skill's \
                 `{tool} deploy` reaches the real tool with no credential"
            );
            assert_eq!(
                std::fs::read_link(&link).expect("a link"),
                source.join(tool),
                "{tool} does not point at the shim"
            );
        }

        // Installing again is not an error: a session is set up once, but a
        // link left behind by anything else must not stop this.
        install_session_bin(&scratch, &apex, &source).expect("again");
        assert!(bin.join("wrangler").exists());

        // An image that never installed them leaves nothing behind and does
        // not fail — a development build running from a checkout has no
        // /usr/libexec/apex, and a session must still start.
        let empty = root.join("no-image");
        let bare_scratch = root.join("bare");
        std::fs::create_dir_all(&bare_scratch).expect("bare");
        let bare = install_session_bin(&bare_scratch, &apex, &empty).expect("still a bin");
        assert!(!bare.join("wrangler").exists());
        assert!(bare.join("git").exists(), "git must still be brokered");

        std::fs::remove_dir_all(&root).ok();
    }

    /// The constant the image installs to and the one a session links from are
    /// the same string, and the Containerfile is the other half of it.
    #[test]
    fn the_tool_shim_directory_is_the_one_the_image_writes() {
        assert_eq!(TOOL_SHIM_DIR, "/usr/libexec/apex/tools");
        assert!(Path::new(TOOL_SHIM_DIR).is_absolute());
    }
}
