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
use apex_agent_core::grant::{GrantKind, SystemGrant};
use apex_agent_core::policy::{
    AgentPolicy, NativeMode, NetworkPolicy, OriginPolicy, PolicyPreset, RequestOrigin, SecretPolicy,
    SystemAccess,
};
use apex_agent_core::protocol::{
    AgentState, Request, Response, RunRequest, SandboxPolicy, SessionInfo,
    POLICY_DIMENSIONS_VERSION, REQUEST_ORIGIN_VERSION, SYSTEM_GRANT_VERSION,
};
use apex_agent_core::hook::{self as hook_core, HookEvent};
use apex_agent_core::term::{self, RawMode, WinSize};
use apex_agent_core::{adapter, checkpoint, config, git, layout, profile, project};
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
        /// List the sessions on a trusted device instead (§20).
        #[arg(long, value_name = "HOST")]
        host: Option<String>,
    },
    /// Reattach to a session's terminal.
    Attach {
        id: u32,
        /// Do not repaint the scrollback first.
        #[arg(long)]
        no_replay: bool,
        /// Attach to a session on a trusted device (§20).
        ///
        /// This is §20's "continue a terminal or agent session elsewhere": the
        /// session keeps running there, and this becomes a view onto it. The id
        /// is the remote's, which is why `apex agent list --host` exists.
        #[arg(long, value_name = "HOST")]
        host: Option<String>,
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
    /// Show or change where an `--network allowlist` session may connect.
    ///
    /// A destination is a host, or a host and a port: `api.example.com`,
    /// `*.example.com`, `git.example.com:22`. A rule with no port means 443.
    /// With no argument this prints the list.
    ///
    /// The list is the runtime's, not a session's — an agent that could name
    /// its own destinations would be writing its own allowlist. A change
    /// applies to sessions started after it; one already running keeps the
    /// list it was started with.
    Allow {
        /// The destination to add. Omit to list what is allowed.
        destination: Option<String>,
        /// Remove it instead of adding it.
        #[arg(long)]
        remove: bool,
    },
    /// System-access grants: what has been granted, and to what (§4.4, §4.5).
    ///
    /// §3.4 asks that "revocation control always be visible", which starts
    /// with the grants themselves being visible. Every grant this machine has
    /// ever issued is listed, with how each one ended — including the ones
    /// that ended because the machine rebooted, which is the answer to "was
    /// that break-glass window still open?".
    Grants {
        /// Only the ones still in force.
        #[arg(long)]
        active: bool,
        /// Machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// Take a system-access grant back before its window runs out.
    ///
    /// Immediate, and it asks for no password: giving up privilege is free.
    /// A break-glass session whose grant is revoked keeps running — its
    /// `no_new_privs` was cleared at exec and cannot be put back — so the
    /// runtime ends it, the same way it does when the window runs out.
    RevokeGrant { id: u32 },
    /// Extend a grant that is still in force, with a fresh password.
    ///
    /// Not an extension of the old consent: the window is recomputed from now
    /// and the same local authentication is asked for again. A managed
    /// session cannot renew its own grant, and cannot renew anybody's — the
    /// runtime refuses any connection that resolves to a session, from the
    /// kernel's view of it rather than from anything the request says.
    RenewGrant {
        id: u32,
        /// The new window, from now: `15m`, `1h`.
        #[arg(long, value_name = "DURATION", value_parser = parse_ttl)]
        ttl: u64,
    },
    /// List the agents this runtime can launch.
    Adapters,
    /// Inspect, check and carry an agent's own configuration (§5).
    ///
    /// An agent installation is a profile, not just a binary: instructions,
    /// settings, commands, skills, plugins and MCP definitions decide what the
    /// binary does. These verbs say which of that is the same on any machine
    /// and which belongs to this one.
    Profile {
        #[command(subcommand)]
        cmd: ProfileCmd,
    },
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
    /// Report one of Claude's own lifecycle events (§6.1), and for
    /// `pre_tool_use` ask APEX whether the tool call is one this session's
    /// sandbox would refuse (§6.2).
    ///
    /// Not meant to be typed. `apex-agentd` writes a settings file that runs
    /// this once per event, and the payload arrives on stdin as the JSON
    /// document Claude produces. It takes no session id: `$APEX_AGENT_SESSION`
    /// is set by the sandbox, and an id on the command line would be an id the
    /// agent could edit into another session's.
    ///
    /// It always exits 0 and prints nothing it does not have to. A hook that
    /// failed loudly would be a broken daemon stopping the agent from working.
    #[command(hide = true)]
    Hook {
        /// session_start | pre_tool_use | post_tool_use | stop | …
        event: String,
    },
    /// Narrow where this session says it is driven from (§7).
    ///
    /// Run from inside a managed session — the runtime works out which session
    /// that is from the connection, so there is no id to pass and no way to
    /// speak about another session.
    ///
    /// The declaration can only ever cost the session something. A local
    /// session may hand itself to Remote Control; nothing may declare itself
    /// local, and a Remote Control session may not declare its way back out.
    Origin {
        /// claude-remote-control | scheduled-job | mcp | subagent | cloud-job
        origin: String,
    },
    /// Forget a finished session and delete its transcript.
    Rm { id: u32 },
    /// Forget every finished session.
    Prune,
    /// Turn the per-user agent runtime on for this account.
    ///
    /// The runtime is a `systemd --user` service, opt-in by design. This is the
    /// one command that opts in, so it never matters whether you remember the
    /// `systemctl --user` incantation. Run as root it first gives root a
    /// lingering systemd user instance (root has none by default) and then
    /// prints what running agents as root costs.
    Enable,
}

/// `apex agent profile <verb>`.
#[derive(Subcommand)]
pub enum ProfileCmd {
    /// Agents whose profile this runtime understands.
    List {
        /// Machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// Show every part of a profile, its class and how a session mounts it.
    Inspect {
        /// Which agent. Defaults to the configured one.
        agent: Option<String>,
        /// Machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// Check a profile: config, hooks, plugins, MCP and skills.
    ///
    /// Reads and reports; it repairs nothing. Exits non-zero when it found
    /// something wrong, so it is usable from a script.
    Doctor {
        /// Which agent. Defaults to the configured one.
        agent: Option<String>,
    },
    /// Write the reusable half of a profile to a directory.
    ///
    /// Instructions, skills, commands, settings and MCP definitions. Never
    /// credentials, conversation transcripts, caches or install paths — and
    /// the values of anything the settings put in the environment are replaced
    /// with blanks, so the bundle says which variables are needed without
    /// carrying what is in them.
    Export {
        /// Which agent. Defaults to the configured one.
        agent: Option<String>,
        /// Where to write it. Defaults to ./<agent>-profile.
        #[arg(long, short, value_name = "DIR")]
        to: Option<PathBuf>,
        /// Write into a directory that already has files in it.
        #[arg(long)]
        force: bool,
    },
    /// Bring this machine's profile up to date with an exported one.
    ///
    /// Files are added and updated; nothing local is deleted, and the two
    /// files that hold both reusable and machine-local state are merged key by
    /// key rather than overwritten — so importing a profile cannot remove the
    /// environment values the export refused to carry.
    #[command(name = "sync", visible_alias = "import")]
    Import {
        /// Which agent. Defaults to the configured one.
        agent: Option<String>,
        /// The directory `apex agent profile export` wrote.
        #[arg(long, short, value_name = "DIR")]
        from: PathBuf,
        /// Print what would change and change nothing.
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Args)]
pub struct RunArgs {
    /// Opening instruction for the agent.
    pub prompt: Option<String>,
    /// Which agent to run. Defaults to the configured one.
    #[arg(long, short)]
    pub agent: Option<String>,
    /// strict | project | unrestricted. The APEX filesystem and process
    /// sandbox (dimension 2). Defaults to the configured policy.
    #[arg(long, short, value_parser = parse_sandbox)]
    pub sandbox: Option<SandboxPolicy>,

    // ── §3.1: the other five permission dimensions ──────────────────────────
    //
    // Six separate flags rather than one mode word, because these are six
    // separate controls. A preset below sets several at once for the modes
    // §4 names; these override whatever a preset chose.
    //
    // Parsed into their own types here rather than carried as strings, so an
    // unknown value is refused by the argument parser with the list of real
    // ones, and so nothing downstream can be handed a dimension value that was
    // never checked.
    /// inherit | ask | bypass. The agent's OWN permission mode (dimension 1).
    /// `inherit` leaves the agent's profile alone, which is the default.
    #[arg(long, value_parser = parse_native)]
    pub native: Option<NativeMode>,
    /// §4.2. The agent stops asking for confirmations. The APEX sandbox, root
    /// boundary, secret broker and audit are all untouched.
    #[arg(long, conflicts_with = "native")]
    pub agent_bypass: bool,
    /// none | session | unsafe. The root capability layer (dimension 3).
    #[arg(long, value_parser = parse_system_access)]
    pub system_access: Option<SystemAccess>,
    /// brokered | none | export. The secret capability layer (dimension 4).
    #[arg(long, value_parser = parse_secrets)]
    pub secrets: Option<SecretPolicy>,
    /// open | allowlist | brokered | offline. Network policy (dimension 5).
    #[arg(long, value_parser = parse_network)]
    pub network: Option<NetworkPolicy>,
    /// local | remote. Which origins may authorise elevation (dimension 6).
    #[arg(long, value_parser = parse_origin_policy)]
    pub origin_policy: Option<OriginPolicy>,

    // ── §7: where the session is driven from ────────────────────────────────
    /// Declare where this session is driven from (§7's request_origin).
    ///
    /// Not the same thing as `--origin-policy`, which decides which origins
    /// may authorise elevation. This says which origin THIS session is, and
    /// it can only ever narrow: the daemon establishes the origin from the
    /// connection, and a declaration is accepted only when it gives something
    /// up. The two local origins cannot be declared at all.
    ///
    /// For a wrapper starting a session on somebody else's behalf — a
    /// scheduler, an MCP bridge, Remote Control. A session already running
    /// narrows itself with `apex agent origin` instead.
    #[arg(long, value_name = "ORIGIN", value_parser = parse_request_origin)]
    pub origin: Option<RequestOrigin>,
    /// §4.5 break-glass: take the APEX protections off.
    #[arg(long)]
    pub unsafe_everything: bool,
    /// How long a system-access grant lasts: `15m`, `90s`, `1h`.
    ///
    /// Required with `--unsafe-everything` — §3.4 asks for an "explicit short
    /// TTL", and a default would be the opposite of explicit. Optional with
    /// `--system-access session`, which is a smaller grant and takes a
    /// half-hour default. Refused without either, because a TTL on an
    /// ordinary session bounds nothing.
    ///
    /// A bare number is refused: `15` is fifteen seconds or fifteen minutes
    /// depending on who is reading, and the value is a security window.
    #[arg(long, value_name = "DURATION", value_parser = parse_ttl)]
    pub ttl: Option<u64>,
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

    // ── §20: run it on another device instead ───────────────────────────────
    /// Run the agent on this trusted device instead of here.
    ///
    /// The whole invocation is forwarded to that machine's own
    /// `apex agent run`, which applies its own sandbox policy, default agent
    /// and checkpointing. Reconstructing those decisions locally would mean
    /// two implementations of one policy, and the remote's is the one that
    /// matters because that is where the agent actually runs.
    #[arg(long, value_name = "HOST")]
    pub host: Option<String>,
    /// The project directory on the remote, when it is not the same absolute
    /// path as here. Skips the same-repository check.
    #[arg(long, value_name = "PATH", requires = "host")]
    pub remote_path: Option<String>,
    /// Run remotely even though this worktree has uncommitted changes.
    ///
    /// They are NOT sent: the remote works from its own checkout.
    #[arg(long, requires = "host")]
    pub allow_dirty: bool,
}

impl RunArgs {
    /// Rebuild the flags this invocation carried, for forwarding to a remote
    /// `apex agent run`.
    ///
    /// Reconstructed from the parsed struct rather than taken from
    /// `std::env::args`, so a flag that clap normalised or defaulted is
    /// forwarded in its normalised form — and so the local-only flags
    /// (`--host`, `--remote-path`, `--allow-dirty`) cannot leak into the
    /// remote command and make it try to dispatch again.
    pub fn forward_argv(&self) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(p) = &self.prompt {
            out.push(p.clone());
        }
        for (flag, value) in [
            ("--agent", self.agent.clone()),
            ("--worktree", self.worktree.clone()),
        ] {
            if let Some(v) = value {
                out.push(flag.to_string());
                out.push(v);
            }
        }
        // Every permission dimension, or a remote run would silently get the
        // remote's defaults instead of what was asked for. Losing
        // `--network offline` on the way to another machine is the kind of
        // omission nobody notices until it matters.
        for (flag, value) in [
            ("--sandbox", self.sandbox.map(|v| v.as_str())),
            ("--native", self.native.map(|v| v.as_str())),
            ("--system-access", self.system_access.map(|v| v.as_str())),
            ("--secrets", self.secrets.map(|v| v.as_str())),
            ("--network", self.network.map(|v| v.as_str())),
            ("--origin-policy", self.origin_policy.map(|v| v.as_str())),
            ("--origin", self.origin.map(|v| v.as_str())),
        ] {
            if let Some(v) = value {
                out.push(flag.to_string());
                out.push(v.to_string());
            }
        }
        if self.agent_bypass {
            out.push("--agent-bypass".to_string());
        }
        if self.unsafe_everything {
            out.push("--unsafe-everything".to_string());
        }
        // Forwarded in its parsed form, in milliseconds' worth of seconds, so
        // the remote applies the window that was asked for rather than its own
        // default. Losing a `--ttl` on the way to another machine would leave
        // a break-glass session there with a longer window than the user typed
        // — or, with `--unsafe-everything`, refuse outright, which is at least
        // loud. This is the quiet half, so it is forwarded.
        if let Some(ms) = self.ttl {
            out.push("--ttl".to_string());
            out.push(format!("{}s", ms / 1000));
        }
        if self.checkpoint {
            out.push("--checkpoint".to_string());
        }
        if self.detach {
            out.push("--detach".to_string());
        }
        // `--cwd` is deliberately NOT forwarded: the remote command already
        // cds into the resolved project directory, and a local path would be
        // meaningless or wrong there.
        if !self.args.is_empty() {
            out.push("--".to_string());
            out.extend(self.args.iter().cloned());
        }
        out
    }
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
        AgentCmd::Run(args) => match &args.host {
            // §20. Checked before anything local happens, so a remote run
            // never starts a local session as a side effect.
            Some(host) => {
                let (h, rp, ad) = (host.clone(), args.remote_path.clone(), args.allow_dirty);
                let forward = args.forward_argv();
                // `Result<i32>`, matching the local arm: this function's
                // contract is the exit code, and the surrounding dispatcher
                // owns the error printing.
                crate::dispatch::agent_run_remote(&h, rp.as_deref(), ad, &forward).map(|()| 0)
            }
            None => run(args),
        },
        AgentCmd::List { all, json, host } => match host {
            Some(h) => {
                let mut argv = vec!["agent".to_string(), "list".to_string()];
                if all {
                    argv.push("--all".to_string());
                }
                if json {
                    argv.push("--json".to_string());
                }
                // No tty: this is output to be read or parsed, and asking for
                // one would make ssh warn when stdin is a pipe.
                crate::dispatch::forward_to_host(
                    &h,
                    &argv,
                    apexd_core::host::Tty::None,
                    Some(crate::dispatch::Capability::Agentd),
                )
                .map(|()| 0)
            }
            None => list(all, json),
        },
        AgentCmd::Attach { id, no_replay, host } => match host {
            Some(h) => {
                let mut argv =
                    vec!["agent".to_string(), "attach".to_string(), id.to_string()];
                if no_replay {
                    argv.push("--no-replay".to_string());
                }
                // A terminal, always: attaching is the interactive case, and
                // the remote pty has to be forwarded for it to be usable.
                crate::dispatch::forward_to_host(
                    &h,
                    &argv,
                    apexd_core::host::Tty::Interactive,
                    Some(crate::dispatch::Capability::Agentd),
                )
                .map(|()| 0)
            }
            None => attach(id, !no_replay),
        },
        AgentCmd::Pause { id } => signal(id, "stop", "paused"),
        AgentCmd::Resume { id } => signal(id, "cont", "resumed"),
        AgentCmd::Kill { id, signal: sig } => signal(id, &sig, "signalled"),
        AgentCmd::Logs { id, bytes } => logs(id, bytes),
        AgentCmd::Status { id } => status(id),
        AgentCmd::Default { agent } => default_agent(agent),
        AgentCmd::Allow {
            destination,
            remove,
        } => allow(destination, remove),
        AgentCmd::Grants { active, json } => grants(active, json),
        AgentCmd::RevokeGrant { id } => revoke_grant(id),
        AgentCmd::RenewGrant { id, ttl } => renew_grant(id, ttl),
        AgentCmd::Adapters => adapters(),
        AgentCmd::Profile { cmd } => profile_cmd(cmd),
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
        AgentCmd::Hook { event } => return hook(&event),
        AgentCmd::Origin { origin } => declare_origin(&origin),
        AgentCmd::Rm { id } => remove(id),
        AgentCmd::Prune => prune(),
        AgentCmd::Enable => enable(),
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

/// The `value_parser`s for the six dimension flags.
///
/// One per dimension rather than one generic helper, because each has to name
/// its own values in the refusal: a user who mistypes `--network allow` needs
/// to be shown `allowlist`, not told that something was invalid.
macro_rules! dimension_parser {
    ($name:ident, $ty:ty, $choices:literal) => {
        fn $name(s: &str) -> std::result::Result<$ty, String> {
            <$ty>::parse(s).ok_or_else(|| format!("use {}", $choices))
        }
    };
}

dimension_parser!(parse_native, NativeMode, "inherit, ask or bypass");
dimension_parser!(parse_sandbox, SandboxPolicy, "strict, project or unrestricted");
dimension_parser!(parse_system_access, SystemAccess, "none, session or unsafe");
dimension_parser!(parse_secrets, SecretPolicy, "brokered, none or export");
dimension_parser!(parse_network, NetworkPolicy, "open, allowlist, brokered or offline");
dimension_parser!(parse_origin_policy, OriginPolicy, "local or remote");

/// `--ttl`, in milliseconds.
///
/// Only the shape is checked here. Whether the window is issuable — the caps,
/// the zero, and whether break-glass may default — is
/// [`apex_agent_core::grant::ttl_for`], because it depends on which mode was
/// asked for and the daemon has to apply the same rule to a client that
/// skipped this.
fn parse_ttl(s: &str) -> std::result::Result<u64, String> {
    apex_agent_core::grant::parse_ttl(s).map_err(|e| e.to_string())
}

/// `--origin`, which is not a dimension and does not accept every value.
///
/// The two local origins parse — they are real §7 names and `RequestOrigin`
/// has to read them back off a record — but they are refused here, at the
/// only place a human types one. A flag that accepted `local-terminal` and
/// left the daemon to refuse it would read like something that works.
fn parse_request_origin(s: &str) -> std::result::Result<RequestOrigin, String> {
    let declarable = || {
        RequestOrigin::ALL
            .iter()
            .filter(|o| o.may_be_declared())
            .map(|o| o.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    };
    match RequestOrigin::parse(s) {
        Some(o) if o.may_be_declared() => Ok(o),
        Some(o) => Err(format!(
            "{o} is established by the runtime from the connection, not declared; use {}",
            declarable()
        )),
        None => Err(format!("use {}", declarable())),
    }
}

/// Resolve the six permission dimensions for one invocation.
///
/// Three layers, each overriding the one before it: the configured defaults,
/// then the preset a mode flag names, then each explicit dimension flag. So
/// `--unsafe-everything --secrets none` is break-glass with the secret layer
/// still off, and no combination is unreachable because a preset claimed it.
///
/// Pure, so the resolution order is asserted directly rather than inferred
/// from what a session ended up with.
pub fn resolve_policy(cfg: &config::Config, args: &RunArgs) -> Result<AgentPolicy> {
    if args.unsafe_everything && args.agent_bypass {
        bail!("--unsafe-everything already turns the agent's own confirmations off");
    }

    // 1. the configured defaults.
    let mut policy = cfg.policy();

    // 2. the preset, when a mode flag named one.
    if args.unsafe_everything {
        policy = PolicyPreset::UnsafeEverything.policy();
    } else if args.agent_bypass {
        // Only dimension 1 moves. §4.2's whole point is that the other five
        // stay where the user's configuration left them, so this is not the
        // preset's full six-tuple.
        policy.native = NativeMode::Bypass;
    }

    // 3. the explicit flags, which win over everything.
    if let Some(v) = args.native {
        policy.native = v;
    }
    if let Some(v) = args.sandbox {
        policy.sandbox = v;
    }
    if let Some(v) = args.system_access {
        policy.system = v;
    }
    if let Some(v) = args.secrets {
        policy.secrets = v;
    }
    if let Some(v) = args.network {
        // The contradiction is caught here rather than in the policy type,
        // because only the CLI can tell "the user typed --network open" from
        // "an older client sent no network key at all". Silently tightening
        // would leave somebody believing they had a network they did not.
        if policy.sandbox == SandboxPolicy::Strict && v != NetworkPolicy::Offline {
            bail!(
                "`--sandbox strict` removes the network, so it cannot be combined with \
                 `--network {v}`; use `--sandbox project --network {v}` instead"
            );
        }
        policy.network = v;
    }
    if let Some(v) = args.origin_policy {
        policy.origin = v;
    }

    // Refuse anything this build cannot enforce, here as well as in the
    // daemon: the message is better in front of the user who typed the flag.
    // The allowlist goes in because `--network allowlist` with nothing on it
    // is a refusal too, and the user who typed the flag is the one who can
    // fix it.
    policy.validate_for(&cfg.allowlist())?;
    Ok(policy.normalised())
}

/// The dimensions this invocation moved away from the safe default.
///
/// Used for the version check and for the banner: only a dimension that is
/// doing something needs saying.
fn non_default_dimensions(policy: &AgentPolicy) -> Vec<(&'static str, &'static str)> {
    let base = AgentPolicy::default();
    policy
        .dimensions()
        .into_iter()
        .zip(base.dimensions())
        .filter(|((_, got), (_, want))| got != want)
        .map(|((name, got), _)| (name, got))
        .collect()
}

fn run(args: RunArgs) -> Result<i32> {
    let cfg = config::Config::load();
    let cwd = match args.cwd.as_ref() {
        Some(dir) => dir
            .canonicalize()
            .with_context(|| format!("{} does not exist", dir.display()))?,
        None => std::env::current_dir().context("reading the current directory")?,
    };

    let policy = resolve_policy(&cfg, &args)?;

    let size = term::stdout_window_size();
    let request = RunRequest {
        agent: args.agent.clone(),
        prompt: args.prompt.clone(),
        args: args.args.clone(),
        cwd: cwd.to_string_lossy().into_owned(),
        policy,
        request_origin: args.origin,
        worktree: args.worktree.clone(),
        checkpoint: args.checkpoint,
        ttl_ms: args.ttl,
        cols: size.cols,
        rows: size.rows,
        env: Vec::new(),
    };

    let mut c = Client::connect()?;
    check_daemon_understands(&mut c, &policy, args.origin)?;
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

    // §3.4's "prominent red indicator", in the surface a terminal user is
    // looking at. The Agent Center shows the same thing from the same field.
    if let (Some(grant), Some(expires)) = (info.grant, info.grant_expires_ms) {
        let left = expires.saturating_sub(apex_agent_core::request::now_ms());
        let window = apex_agent_core::grant::format_ms(left);
        if info.policy.system == SystemAccess::Unsafe {
            eprintln!(
                "{}",
                red(&format!(
                    "apex: BREAK-GLASS — grant {grant} removes the APEX root boundary for \
                     {window}. This session can become root. It ends when the window does; \
                     `apex agent revoke-grant {grant}` ends it now"
                ))
            );
        } else {
            eprintln!(
                "apex: system-access grant {grant} for {window} — the privilege operations \
                 this session files are pre-approved until it runs out; \
                 `apex agent revoke-grant {grant}` ends it now"
            );
        }
    }
    eprintln!(
        "apex: session {} ({}, {}) — detach with {}",
        info.id,
        info.agent,
        describe_policy(&info.policy),
        cfg.detach_key
    );
    attach_session(info.id, true, &cfg)
}

/// One line naming the sandbox and anything else that is not at its default.
///
/// §4.1's second criterion — "APEX does not duplicate the dangerous-mode
/// warning" — is a property of this function and of what surrounds it. Claude
/// prints its own banner when it is in `bypassPermissions`, every launch, and
/// a user who has set that as their profile default has already agreed to see
/// it. A second APEX warning saying the same thing would be noise on every
/// run, and noise on every run is how a warning stops being read — including
/// the one warning here that is worth reading, which is break-glass.
///
/// So this line states what APEX's OWN layers are, in APEX's own vocabulary,
/// and says nothing evaluative about dimension 1. `native bypass` appears in
/// it when the user asked APEX for that mode, as a fact among five other
/// facts. It is a status line, not a caution; `dimension_warnings_are_apexs_own`
/// holds it to that.
fn describe_policy(policy: &AgentPolicy) -> String {
    let mut parts = vec![format!("sandbox {}", policy.sandbox)];
    for (name, value) in non_default_dimensions(policy) {
        if name != "sandbox" {
            parts.push(format!("{name} {value}"));
        }
    }
    parts.join(", ")
}

/// Refuse to send a dimension a daemon that old would drop.
///
/// A daemon predating the split reads `sandbox` and ignores the other five
/// keys, so `--network offline` would come back as a session with a network
/// and nothing anywhere would say so. That is the fail-open a protocol version
/// exists to catch, and the check is skipped entirely when every dimension is
/// at its default, so an all-defaults run still works against an old daemon.
fn check_daemon_understands(
    c: &mut Client,
    policy: &AgentPolicy,
    origin: Option<RequestOrigin>,
) -> Result<()> {
    let moved = non_default_dimensions(policy);
    let mut needs: Vec<(&str, u32)> = moved
        .iter()
        .map(|(name, _)| *name)
        .filter(|name| *name != "sandbox")
        .map(|name| (name, POLICY_DIMENSIONS_VERSION))
        .collect();
    // A declared origin is the same failure and a worse one. A daemon that
    // predates it drops the key and records whatever it observed, which for
    // Remote Control is the local origin §7 reserves root for.
    if origin.is_some() {
        needs.push(("--origin", REQUEST_ORIGIN_VERSION));
    }
    // The worst of the three, which is why it is checked even though `system`
    // is already in `moved`. A daemon below this refuses both elevated modes
    // outright, so the failure is loud — but it also drops `ttl_ms`, and a
    // future daemon that accepted the mode while ignoring the window would
    // give a break-glass session no expiry at all. Named separately so the
    // refusal says which setting would be lost.
    if policy.needs_grant().is_some() {
        needs.push(("--system-access", SYSTEM_GRANT_VERSION));
        needs.push(("--ttl", SYSTEM_GRANT_VERSION));
    }
    if needs.is_empty() {
        return Ok(());
    }
    let Response::Hello { version, .. } = c.call(&Request::Hello)? else {
        // A daemon that cannot answer Hello is one this cannot reason about,
        // and guessing in the permissive direction is the whole failure mode.
        bail!("the agent runtime did not answer the protocol handshake");
    };
    let ignored: Vec<&str> = needs
        .iter()
        .filter(|(_, since)| version < *since)
        .map(|(name, _)| *name)
        .collect();
    if !ignored.is_empty() {
        bail!(
            "the running agent runtime speaks protocol {version} and would ignore {}; \
             restart it with `systemctl --user restart apex-agentd` so the setting takes \
             effect",
            ignored.join(", ")
        );
    }
    Ok(())
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
            s.policy.sandbox,
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
            for (name, value) in cfg.policy().dimensions() {
                println!("{name:<12} {value}");
            }
            println!("detach key   {}", cfg.detach_key);
            if !running {
                println!();
                println!("enable it with: apex agent enable");
                return Ok(1);
            }
            let sessions = client::sessions()?;
            let live = sessions.iter().filter(|s| s.is_live()).count();
            println!("sessions     {live} running, {} recorded", sessions.len());
            Ok(0)
        }
    }
}

/// `apex agent enable` — turn the per-user runtime on for whoever runs it.
///
/// The runtime is opt-in by design: a per-user daemon holding PTYs is not
/// something every account should start. This is the single command that opts
/// in, so `apex agent` and the `not running` message can point at one thing
/// instead of a `systemctl --user` line the user has to get right — and, for
/// root, a line that does not work at all until root has a lingering user
/// instance.
fn enable() -> Result<i32> {
    if apex_agent_core::paths::uid() == 0 {
        enable_root()
    } else {
        enable_user()
    }
}

/// The ordinary case: an interactive login already has a running user instance
/// and an `$XDG_RUNTIME_DIR`, so this is exactly the documented one-liner.
fn enable_user() -> Result<i32> {
    let code = run_tool("systemctl", &["--user", "enable", "--now", "apex-agentd"], None)?;
    if code == 0 {
        println!("agent runtime enabled. `a` and `apex agent run` work now.");
    }
    Ok(code)
}

/// Root has no `systemd --user` instance unless it lingers, and no
/// `$XDG_RUNTIME_DIR` in a sudo shell, so the ordinary line fails with a bus
/// error. Give root a lingering instance, wait for its runtime directory to
/// appear, then enable the service against it.
///
/// A deliberate, user-invoked choice, not a default, and it says what it costs:
/// an agent started by a root runtime runs as root. The per-session sandbox
/// still keeps `/` read-only and masks the home, but the working directory is
/// writable, so a root agent can write as root wherever it is started. Running
/// the upstream agent (`claude`, `opencode`, …) directly as root is strictly
/// less confined than this; a normal user account is safer than either.
fn enable_root() -> Result<i32> {
    let code = run_tool("loginctl", &["enable-linger", "root"], None)?;
    if code != 0 {
        bail!("could not enable a lingering systemd user instance for root");
    }
    // enable-linger brings user@0 up asynchronously; nudge it, then wait for the
    // runtime directory where the control socket will live.
    run_tool("systemctl", &["start", "user@0.service"], None).ok();
    let runtime = Path::new("/run/user/0");
    for _ in 0..50 {
        if runtime.exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    if !runtime.exists() {
        bail!("root's systemd user instance did not come up (/run/user/0 is absent)");
    }
    // XDG_RUNTIME_DIR is how `systemctl --user` finds the bus; a sudo shell does
    // not set it, so point it at root's own instance explicitly.
    let code = run_tool(
        "systemctl",
        &["--user", "enable", "--now", "apex-agentd"],
        Some(("XDG_RUNTIME_DIR", "/run/user/0")),
    )?;
    if code == 0 {
        println!("agent runtime enabled for root.");
        println!();
        println!("note: agents started here run AS ROOT. The per-session sandbox keeps /");
        println!("read-only and masks the home, but the working directory is writable, so a");
        println!("root agent can write as root wherever you start it. A normal user account");
        println!("is safer. To undo:  systemctl --user disable --now apex-agentd");
        println!("                    loginctl disable-linger root");
    }
    Ok(code)
}

/// Spawn a helper, inheriting stdio, optionally with one extra environment
/// variable. Returns its exit code.
fn run_tool(program: &str, args: &[&str], env: Option<(&str, &str)>) -> Result<i32> {
    let mut cmd = Command::new(program);
    cmd.args(args);
    if let Some((k, v)) = env {
        cmd.env(k, v);
    }
    let status = cmd.status().with_context(|| format!("running {program}"))?;
    Ok(status.code().unwrap_or(1))
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
    for (name, value) in s.policy.dimensions() {
        println!("{name:<12} {value}");
    }
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

/// `apex agent allow [DESTINATION] [--remove]`.
///
/// The rule is parsed before it is stored, so a line that would empty the
/// whole allowlist on the next load is refused here, with the reason, rather
/// than written and then silently dropped.
fn allow(destination: Option<String>, remove: bool) -> Result<i32> {
    use apex_agent_core::destination::Rule;

    let (mut cfg, notes) = config::load_reporting();
    for note in &notes {
        eprintln!("apex: {note}");
    }

    let Some(destination) = destination else {
        if cfg.network_allow.is_empty() {
            println!(
                "nothing is allowed, so `--network allowlist` has nothing to reach.\n\
                 add a destination with `apex agent allow <host>`"
            );
            return Ok(0);
        }
        for line in cfg.allowlist().lines() {
            println!("{line}");
        }
        return Ok(0);
    };

    // Normalised through the parser, so `API.Example.COM.` and
    // `api.example.com` cannot both end up in the file as separate rules that
    // mean one thing.
    let rule = Rule::parse(&destination).map_err(|e| anyhow::anyhow!("{e}"))?;
    let line = rule.as_line();

    if remove {
        let before = cfg.network_allow.len();
        cfg.network_allow
            .retain(|d| Rule::parse(d).map(|r| r.as_line()) != Ok(line.clone()));
        if cfg.network_allow.len() == before {
            bail!("{line} is not on the allowlist");
        }
        cfg.save()?;
        println!("{line} removed");
        return Ok(0);
    }

    if cfg.allowlist().lines().contains(&line) {
        println!("{line} is already allowed");
        return Ok(0);
    }
    cfg.network_allow.push(line.clone());
    cfg.save()?;
    println!("{line} allowed for `--network allowlist` sessions started from now on");
    Ok(0)
}

// ── system-access grants (§4.4, §4.5) ───────────────────────────────────────

/// The escape that makes break-glass unmissable, and the one that undoes it.
///
/// §3.4 asks for a "prominent red Agent Center indicator". The Agent Center is
/// APEX Shell's, and it reads `SessionInfo.grant`; this is the same statement
/// in the place a terminal user is actually looking. Colour is a strong claim
/// to make about somebody's terminal, so it is made only when stderr is a
/// terminal and `NO_COLOR` is unset — and every message that uses it still
/// says the words, so a pipe, a log file and a screen reader all carry the
/// same information.
const RED: &str = "\x1b[1;31m";
const OFF: &str = "\x1b[0m";

fn red(text: &str) -> String {
    let colour = std::io::stderr().is_terminal() && std::env::var_os("NO_COLOR").is_none();
    if colour {
        format!("{RED}{text}{OFF}")
    } else {
        text.to_string()
    }
}

/// The grants, and for each one the state word and the sentence.
///
/// Parallel vectors because that is the wire shape: the daemon computes the
/// state, since it depends on the running kernel's boot id and a client
/// deriving it could get a different answer from the daemon that issued the
/// grant.
type GrantListing = (Vec<SystemGrant>, Vec<(String, String)>);

/// Ask the daemon for its grants.
fn fetch_grants() -> Result<GrantListing> {
    let mut c = Client::connect()?;
    match c.call(&Request::SystemGrants)? {
        Response::SystemGrants { grants, states } => Ok((grants, states)),
        Response::Error { message, .. } => bail!(message),
        other => bail!("unexpected reply: {other:?}"),
    }
}

fn grants(active_only: bool, json: bool) -> Result<i32> {
    let (grants, states) = fetch_grants()?;
    let rows: Vec<_> = grants
        .iter()
        .zip(states.iter())
        .filter(|(_, (state, _))| !active_only || state == "active")
        .collect();

    if json {
        let out: Vec<serde_json::Value> = rows
            .iter()
            .map(|(g, (state, said))| {
                let mut v = serde_json::to_value(g).unwrap_or_default();
                if let Some(o) = v.as_object_mut() {
                    o.insert("state".into(), state.clone().into());
                    o.insert("summary".into(), said.clone().into());
                }
                v
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(0);
    }

    if rows.is_empty() {
        println!(
            "no {}system-access grants. `apex agent run --system-access session` or \
             `--unsafe-everything --ttl 15m` asks for one, and each takes a local password",
            if active_only { "active " } else { "" }
        );
        return Ok(0);
    }

    println!(
        "{:>3}  {:<13} {:>7}  {:<21} {:<8} WHAT IT COVERS",
        "ID", "MODE", "SESSION", "STATE", "WINDOW"
    );
    for (g, (state, _)) in &rows {
        let window = apex_agent_core::grant::format_ms(g.expires_ms.saturating_sub(g.issued_ms));
        let covers = if g.capabilities.is_empty() {
            // Break-glass carries none, and saying "-" would read as a gap in
            // the record rather than as the point of the mode.
            "root inside the session (sudo)".to_string()
        } else {
            g.capabilities.join(", ")
        };
        let line = format!(
            "{:>3}  {:<13} {:>7}  {:<21} {:<8} {}",
            g.id, g.kind, g.session, state, window, covers
        );
        // Only an active break-glass grant is red. A grant that has ended is
        // history, and colouring history teaches people to ignore the colour.
        if state == "active" && g.kind == GrantKind::BreakGlass {
            println!("{}", red(&line));
        } else {
            println!("{line}");
        }
    }
    // The sentence, under the table, for anything that is not simply active:
    // "ended at the reboot" and "ended with the runtime" are the answers §3.4
    // asks the machine to be able to give, and a STATE column alone gives
    // them too quietly.
    for (_, (state, said)) in &rows {
        if state != "active" {
            println!("  {said}");
        }
    }
    if rows.iter().any(|(_, (s, _))| s == "active") {
        println!("\nrevoke one with `apex agent revoke-grant <id>`");
    }
    Ok(0)
}

fn revoke_grant(id: u32) -> Result<i32> {
    let mut c = Client::connect()?;
    match c.call(&Request::RevokeSystemGrant { id })? {
        Response::SystemGrants { states, .. } => {
            for (_, said) in states {
                println!("{said}");
            }
            Ok(0)
        }
        Response::Error { message, .. } => bail!(message),
        other => bail!("unexpected reply: {other:?}"),
    }
}

fn renew_grant(id: u32, ttl_ms: u64) -> Result<i32> {
    let mut c = Client::connect()?;
    match c.call(&Request::RenewSystemGrant { id, ttl_ms })? {
        Response::SystemGrants { states, .. } => {
            for (_, said) in states {
                println!("{said}");
            }
            Ok(0)
        }
        Response::Error { message, .. } => bail!(message),
        other => bail!("unexpected reply: {other:?}"),
    }
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

// ── agent profiles (§5) ─────────────────────────────────────────────────────

fn profile_cmd(cmd: ProfileCmd) -> Result<i32> {
    match cmd {
        ProfileCmd::List { json } => profile_list(json),
        ProfileCmd::Inspect { agent, json } => profile_inspect(agent, json),
        ProfileCmd::Doctor { agent } => profile_doctor(agent),
        ProfileCmd::Export { agent, to, force } => profile_export(agent, to, force),
        ProfileCmd::Import {
            agent,
            from,
            dry_run,
        } => profile_import(agent, from, dry_run),
    }
}

/// Resolve the agent name a profile verb was given.
///
/// An adapter without a profile is refused by name rather than by an empty
/// table: `apex agent profile doctor codex` reporting nothing wrong would be a
/// worse answer than saying APEX does not know what a codex profile is.
fn profile_for(agent: Option<String>) -> Result<&'static profile::Profile> {
    let id = agent.unwrap_or_else(|| config::Config::load().default_agent);
    profile::by_agent(&id).with_context(|| {
        let known: Vec<&str> = profile::PROFILES.iter().map(|p| p.agent).collect();
        format!(
            "APEX has no profile description for {id}; it knows: {}",
            known.join(", ")
        )
    })
}

fn profile_list(json: bool) -> Result<i32> {
    let home = apex_agent_core::paths::home();
    if json {
        let all: Vec<_> = profile::PROFILES
            .iter()
            .map(|p| profile::summary(p, &home))
            .collect();
        println!("{}", serde_json::to_string_pretty(&all)?);
        return Ok(0);
    }
    println!(
        "{:<10} {:<24} {:<10} REUSABLE / LOCAL / SECRET",
        "AGENT", "ROOT", "INSTALLED"
    );
    for p in profile::PROFILES {
        let s = profile::summary(p, &home);
        let n = |k: &str| s.get(k).and_then(|v| v.as_u64()).unwrap_or(0);
        println!(
            "{:<10} {:<24} {:<10} {} / {} / {}",
            p.agent,
            s["root"].as_str().unwrap_or_default(),
            if s["installed"] == true { "yes" } else { "no" },
            n("reusable") + n("mixed"),
            n("machine_local"),
            n("secret"),
        );
    }
    Ok(0)
}

fn profile_inspect(agent: Option<String>, json: bool) -> Result<i32> {
    let p = profile_for(agent)?;
    let home = apex_agent_core::paths::home();
    let found = profile::inspect(p, &home);
    if json {
        let rows: Vec<_> = found
            .iter()
            .map(|f| {
                serde_json::json!({
                    "path": f.path,
                    "class": f.class.as_str(),
                    "mount": match f.mount {
                        profile::Mount::ReadOnly => "read-only",
                        profile::Mount::Writable => "writable",
                    },
                    "role": f.role.as_str(),
                    "present": f.present,
                    "files": f.files,
                    "what": f.what,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(0);
    }
    println!("{} — {}", p.display, profile::display_home(&p.root_dir(&home), &home));
    println!();
    println!(
        "{:<42} {:<14} {:<10} {:<14} FILES",
        "PATH", "CLASS", "MOUNT", "ROLE"
    );
    for f in &found {
        println!(
            "{:<42} {:<14} {:<10} {:<14} {}",
            f.path,
            f.class.as_str(),
            match f.mount {
                profile::Mount::ReadOnly => "read-only",
                profile::Mount::Writable => "writable",
            },
            f.role.as_str(),
            if f.present {
                f.files.to_string()
            } else {
                "-".to_string()
            },
        );
    }
    println!();
    println!("reusable and mixed are what `apex agent profile export` carries.");
    println!("mixed files are carried in part: see `apex agent profile export --help`.");
    Ok(0)
}

fn profile_doctor(agent: Option<String>) -> Result<i32> {
    let p = profile_for(agent)?;
    let home = apex_agent_core::paths::home();
    let report = profile::doctor(p, &home);

    println!(
        "{} — {}",
        p.display,
        profile::display_home(&p.root_dir(&home), &home)
    );
    for section in &report.sections {
        println!();
        println!("{}", section.title);
        for line in &section.lines {
            println!("  {line}");
        }
    }
    println!();
    if report.problems.is_empty() {
        println!("no problems found");
        return Ok(0);
    }
    println!(
        "{} problem{}",
        report.problems.len(),
        if report.problems.len() == 1 { "" } else { "s" }
    );
    for problem in &report.problems {
        println!("  {problem}");
    }
    Ok(1)
}

fn profile_export(agent: Option<String>, to: Option<PathBuf>, force: bool) -> Result<i32> {
    let p = profile_for(agent)?;
    let home = apex_agent_core::paths::home();
    let dest = to.unwrap_or_else(|| PathBuf::from(format!("{}-profile", p.agent)));

    if !force {
        let occupied = std::fs::read_dir(&dest)
            .map(|mut it| it.next().is_some())
            .unwrap_or(false);
        if occupied {
            bail!(
                "{} already has files in it; pass --force to write into it anyway",
                dest.display()
            );
        }
    }

    let report = profile::export(p, &home, &dest)
        .with_context(|| format!("exporting the {} profile", p.agent))?;
    println!(
        "wrote {} file{} ({}) to {}",
        report.files,
        if report.files == 1 { "" } else { "s" },
        human_bytes(report.bytes),
        report.dest.display()
    );
    for (path, gave_up) in &report.edited {
        println!("  redacted  {}  ({gave_up})", path.display());
    }
    for (path, class) in &report.excluded {
        println!(
            "  left      {}  ({class})",
            profile::display_home(path, &home)
        );
    }
    Ok(0)
}

fn profile_import(agent: Option<String>, from: PathBuf, dry_run: bool) -> Result<i32> {
    let p = profile_for(agent)?;
    let home = apex_agent_core::paths::home();
    let changes = if dry_run {
        profile::plan_import(p, &from, &home)
    } else {
        profile::import(p, &from, &home)
    }
    .with_context(|| format!("importing {}", from.display()))?;

    let writes = changes
        .iter()
        .filter(|c| !matches!(c, profile::Change::Unfilled(_, _)))
        .count();
    if writes == 0 {
        println!("nothing to change: this machine already has what the bundle carries");
    } else {
        println!(
            "{} {} file{}",
            if dry_run { "would change" } else { "changed" },
            writes,
            if writes == 1 { "" } else { "s" }
        );
    }
    for change in &changes {
        match change {
            profile::Change::Unfilled(_, _) => {}
            other => println!("  {other}"),
        }
    }
    let unfilled: Vec<&profile::Change> = changes
        .iter()
        .filter(|c| matches!(c, profile::Change::Unfilled(_, _)))
        .collect();
    if !unfilled.is_empty() {
        println!();
        println!("set these yourself — the export carried the names, not the values:");
        for change in unfilled {
            println!("  {change}");
        }
    }
    Ok(0)
}

/// Bytes as a person reads them. Binary units, because that is what `du -h`
/// and every file manager on this machine show.
fn human_bytes(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KiB", "MiB", "GiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
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

/// `apex agent hook <event>` — the bridge from Claude's own lifecycle.
///
/// Returns an exit code directly rather than a `Result`, because there is only
/// one: **0**, always. Claude reads a non-zero exit from a `PreToolUse` as a
/// reason to stop the tool call, and 2 as a reason to block it and hand the
/// agent the stderr — so a daemon that is down, a payload that will not parse
/// or an event name from a newer settings file must not become a refusal. Every
/// one of those is a hook that said nothing, and a hook that says nothing
/// leaves the sandbox doing the enforcing, which is where §6.2 puts it anyway.
///
/// Diagnostics go to stderr, which Claude shows in its transcript in verbose
/// mode and otherwise discards. stdout carries the deny document and nothing
/// else — anything else there would be read as a malformed hook response.
fn hook(event: &str) -> i32 {
    let Some(parsed) = HookEvent::parse(event) else {
        eprintln!("apex agent hook: unknown event {event:?}; ignoring");
        return 0;
    };
    let payload = read_payload();

    // The policy point first, because its answer decides whether the tool runs
    // at all and it is the only part with a deadline the agent can feel.
    if parsed.is_policy_point() {
        if let Some(doc) = policy_decision(&payload) {
            println!("{doc}");
        }
    }

    let Some(id) = client::current_session() else {
        // Not inside a managed session: a user's own `~/.claude/settings.json`
        // pointing here, or a nested agent. Nothing to report to.
        return 0;
    };
    let observation = hook_core::observe(parsed, &payload);
    if let Err(e) = client::publish_hook(id, &observation) {
        eprintln!("apex agent hook: {parsed} not published: {e:#}");
    }
    0
}

/// Read the payload Claude writes to a hook's stdin.
///
/// An unreadable or unparseable document is an empty payload, not an error:
/// the event still happened and the state it implies does not depend on the
/// detail. Bounded, because this is a document an agent's own tool arguments
/// end up inside and the process has no reason to hold a large one.
fn read_payload() -> hook_core::Payload {
    use std::io::Read;
    const MAX_PAYLOAD: u64 = 1024 * 1024;
    let mut text = String::new();
    if std::io::stdin()
        .take(MAX_PAYLOAD)
        .read_to_string(&mut text)
        .is_err()
    {
        return hook_core::Payload::default();
    }
    serde_json::from_str(&text).unwrap_or_default()
}

/// Ask the daemon whether this tool call is one the sandbox would refuse, and
/// render the answer as the document Claude expects.
///
/// `None` for anything but a refusal, including every failure. See [`hook`].
fn policy_decision(payload: &hook_core::Payload) -> Option<String> {
    let id = client::current_session()?;
    let tool_name = payload.tool_name.clone()?;
    let reply = client::call(&Request::ToolCheck {
        id,
        tool_name,
        tool_input: payload.tool_input.clone(),
    });
    let deny = match reply {
        Ok(Response::ToolDecision { deny }) => deny?,
        // An older daemon that does not know this request, or no daemon at all.
        Ok(_) | Err(_) => return None,
    };
    Some(hook_core::deny_document(&deny).to_string())
}

/// `apex agent origin <origin>` — narrow the calling session's own origin.
///
/// The session is resolved by the daemon from the connection, so this cannot
/// be pointed at another session and does not read `$APEX_AGENT_SESSION`.
/// Handing a session id to a verb that changes a permission-relevant property
/// is exactly what the privilege verbs avoid, and for the same reason.
fn declare_origin(origin: &str) -> Result<i32> {
    let wanted = parse_request_origin(origin).map_err(|e| anyhow::anyhow!("{e}"))?;
    match client::call(&Request::DeclareOrigin {
        origin: wanted.as_str().to_string(),
    })? {
        Response::Session(info) => {
            eprintln!(
                "apex: session {} is now {} (declared)",
                info.id,
                info.request_origin
                    .map(|o| o.to_string())
                    .unwrap_or_else(|| "unrecorded".into())
            );
            Ok(0)
        }
        Response::Error { message, .. } => {
            eprintln!("apex: {message}");
            Ok(1)
        }
        other => bail!("unexpected reply: {other:?}"),
    }
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

    /// A `run` invocation with nothing set, so a test can turn on exactly the
    /// one flag it is about.
    fn run_args() -> RunArgs {
        RunArgs {
            prompt: None,
            agent: None,
            sandbox: None,
            native: None,
            agent_bypass: false,
            system_access: None,
            secrets: None,
            network: None,
            origin_policy: None,
            origin: None,
            unsafe_everything: false,
            ttl: None,
            worktree: None,
            checkpoint: false,
            cwd: None,
            detach: false,
            args: Vec::new(),
            host: None,
            remote_path: None,
            allow_dirty: false,
        }
    }

    #[test]
    fn no_flags_at_all_resolves_to_the_configured_defaults() {
        let cfg = config::Config::default();
        let p = resolve_policy(&cfg, &run_args()).expect("resolve");
        assert_eq!(p, AgentPolicy::default());
    }

    #[test]
    fn agent_bypass_moves_dimension_one_and_leaves_the_other_five() {
        // §4.2. The flag that most invites a collapse: the obvious "helpful"
        // edit is to have it also loosen the sandbox, and that is criterion 2.
        let cfg = config::Config::default();
        let args = RunArgs {
            agent_bypass: true,
            ..run_args()
        };
        let p = resolve_policy(&cfg, &args).expect("resolve");
        assert_eq!(p.native, NativeMode::Bypass);
        assert_eq!(p, AgentPolicy { native: NativeMode::Bypass, ..AgentPolicy::default() });
    }

    #[test]
    fn a_flag_overrides_the_preset_it_was_given_alongside() {
        // Presets are a starting point, not a fence. Break-glass with the
        // secret layer explicitly off must be expressible, or the named modes
        // would be the only reachable points in the space.
        let cfg = config::Config::default();
        let args = RunArgs {
            unsafe_everything: true,
            secrets: Some(SecretPolicy::None),
            ..run_args()
        };
        let p = resolve_policy(&cfg, &args).expect("resolve");
        // Break-glass set three dimensions; the flag overrode the one it
        // named and left the other two where the preset put them.
        assert_eq!(p.system, SystemAccess::Unsafe);
        assert_eq!(p.sandbox, SandboxPolicy::Unrestricted);
        assert_eq!(p.native, NativeMode::Bypass);
        assert_eq!(p.secrets, SecretPolicy::None, "the flag did not win");
    }

    #[test]
    fn the_configured_default_is_the_starting_point_and_flags_win() {
        let cfg = config::Config {
            sandbox: SandboxPolicy::Strict,
            native: NativeMode::Bypass,
            ..config::Config::default()
        };
        // Nothing given: the configuration stands.
        let p = resolve_policy(&cfg, &run_args()).expect("resolve");
        assert_eq!(p.sandbox, SandboxPolicy::Strict);
        assert_eq!(p.native, NativeMode::Bypass);
        assert_eq!(p.network, NetworkPolicy::Offline, "strict normalises to offline");

        // A flag beats it.
        let args = RunArgs {
            sandbox: Some(SandboxPolicy::Project),
            ..run_args()
        };
        let p = resolve_policy(&cfg, &args).expect("resolve");
        assert_eq!(p.sandbox, SandboxPolicy::Project);
        assert_eq!(p.network, NetworkPolicy::Open);
    }

    #[test]
    fn an_explicit_contradiction_is_refused_rather_than_quietly_tightened() {
        // `--sandbox strict --network open` cannot be honoured. Tightening it
        // in silence would leave somebody believing they had a network; this
        // is the one check the CLI has to make, because only the CLI can tell
        // a typed `--network open` from a client that sent no network key.
        let cfg = config::Config::default();
        let args = RunArgs {
            sandbox: Some(SandboxPolicy::Strict),
            network: Some(NetworkPolicy::Open),
            ..run_args()
        };
        let err = resolve_policy(&cfg, &args).expect_err("a contradiction");
        assert!(err.to_string().contains("removes the network"), "{err}");

        // The same pair spelled compatibly is fine.
        let args = RunArgs {
            sandbox: Some(SandboxPolicy::Strict),
            network: Some(NetworkPolicy::Offline),
            ..run_args()
        };
        assert!(resolve_policy(&cfg, &args).is_ok());
    }

    #[test]
    fn an_unenforceable_dimension_is_refused_in_front_of_the_user_who_typed_it() {
        let cfg = config::Config::default();
        let cases: [(RunArgs, &str); 3] = [
            (
                // Break-glass inside a sandbox that would keep no_new_privs
                // on anyway: bwrap sets it unconditionally, so the pair would
                // report a boundary it had not moved.
                RunArgs {
                    system_access: Some(SystemAccess::Unsafe),
                    sandbox: Some(SandboxPolicy::Project),
                    ..run_args()
                },
                "no_new_privs",
            ),
            (
                RunArgs { secrets: Some(SecretPolicy::Export), ..run_args() },
                "never placed in a session",
            ),
            (
                RunArgs {
                    origin_policy: Some(OriginPolicy::RemoteElevationAllowed),
                    ..run_args()
                },
                "Approve the operation locally",
            ),
        ];
        for (args, expect) in cases {
            let err = resolve_policy(&cfg, &args).expect_err(expect);
            assert!(err.to_string().contains(expect), "{err}");
        }
    }

    #[test]
    fn an_allowlist_with_nothing_on_it_is_refused_where_it_can_still_be_fixed() {
        // Fail-closed either way — an empty allowlist denies everything — but
        // a session reporting `allowlist` while reaching nothing is an offline
        // session nobody asked for, and the failure would otherwise arrive
        // minutes later as a network error inside the agent.
        let cfg = config::Config::default();
        assert!(cfg.network_allow.is_empty());
        let args = RunArgs {
            network: Some(NetworkPolicy::Allowlist),
            ..run_args()
        };
        let err = resolve_policy(&cfg, &args).expect_err("an empty allowlist");
        assert!(err.to_string().contains("apex agent allow"), "{err}");

        // With a destination configured it is a mode that runs.
        let cfg = config::Config {
            network_allow: vec!["api.anthropic.com".into()],
            ..config::Config::default()
        };
        let p = resolve_policy(&cfg, &args).expect("resolve");
        assert_eq!(p.network, NetworkPolicy::Allowlist);
        assert_eq!(p.sandbox, SandboxPolicy::Project, "it still needs a namespace");
    }

    #[test]
    fn a_destination_is_normalised_before_it_is_stored() {
        // Otherwise `API.Example.COM.` and `api.example.com` become two rules
        // in the file that mean one thing, and removing one leaves the other.
        use apex_agent_core::destination::Rule;
        assert_eq!(Rule::parse("API.Example.COM.").unwrap().as_line(), "api.example.com");
        assert_eq!(Rule::parse("api.example.com:443").unwrap().as_line(), "api.example.com");
        assert_eq!(Rule::parse("git.example.com:22").unwrap().as_line(), "git.example.com:22");
        // And a rule that would empty the whole list on the next load is
        // refused before it is written.
        assert!(Rule::parse("*.com").is_err());
    }

    #[test]
    fn brokered_egress_is_refused_when_the_broker_has_been_shut() {
        // Two flags that each make sense and cannot both be honoured. Refused
        // where the user typed them, so the message names both rather than
        // arriving from the daemon as a policy error about one.
        let cfg = config::Config::default();
        let args = RunArgs {
            sandbox: Some(SandboxPolicy::Project),
            network: Some(NetworkPolicy::Brokered),
            secrets: Some(SecretPolicy::None),
            ..run_args()
        };
        let err = resolve_policy(&cfg, &args).expect_err("a contradiction");
        assert!(err.to_string().contains("--secrets none"), "{err}");

        // Brokered on its own, under project confinement, is a mode that runs.
        let args = RunArgs {
            sandbox: Some(SandboxPolicy::Project),
            network: Some(NetworkPolicy::Brokered),
            ..run_args()
        };
        let p = resolve_policy(&cfg, &args).expect("resolve");
        assert_eq!(p.network, NetworkPolicy::Brokered);
        assert_eq!(p.secrets, SecretPolicy::Brokered);
    }

    #[test]
    fn a_misspelled_dimension_value_names_the_ones_that_exist() {
        // Refused by the argument parser, so nothing downstream is ever handed
        // a dimension value that was not checked.
        let err = parse_network("allow").expect_err("a typo");
        assert!(err.contains("allowlist"), "{err}");
        assert!(err.contains("offline"), "{err}");
        assert_eq!(parse_network("offline"), Ok(NetworkPolicy::Offline));
        assert!(parse_native("yolo").is_err());
        assert!(parse_sandbox("loose").is_err());
    }

    #[test]
    fn a_remote_run_carries_every_dimension_it_was_given() {
        // Losing `--network offline` on the way to another machine would run
        // the agent with a network on a host nobody was watching.
        let args = RunArgs {
            agent_bypass: true,
            native: None,
            sandbox: Some(SandboxPolicy::Project),
            system_access: Some(SystemAccess::Session),
            secrets: Some(SecretPolicy::None),
            network: Some(NetworkPolicy::Offline),
            origin_policy: Some(OriginPolicy::LocalElevationOnly),
            host: Some("katana".into()),
            ..run_args()
        };
        let forwarded = args.forward_argv().join(" ");
        for flag in [
            "--sandbox project",
            "--system-access session",
            "--secrets none",
            "--network offline",
            "--origin-policy local_elevation_only",
            "--agent-bypass",
        ] {
            assert!(forwarded.contains(flag), "{flag} was not forwarded: {forwarded}");
        }
        // The local-only flags still stay behind.
        assert!(!forwarded.contains("--host"), "{forwarded}");
    }

    #[test]
    fn only_the_dimensions_that_moved_are_announced() {
        let p = AgentPolicy {
            native: NativeMode::Bypass,
            ..AgentPolicy::default()
        };
        assert_eq!(
            non_default_dimensions(&p),
            vec![("native", "bypass")],
            "a banner that listed all six would be noise on every run"
        );
        assert!(non_default_dimensions(&AgentPolicy::default()).is_empty());
        assert_eq!(describe_policy(&p), "sandbox project, native bypass");
    }

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
