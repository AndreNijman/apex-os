//! The `apex-agentd` control protocol.
//!
//! Newline-delimited JSON over a `SOCK_STREAM` Unix socket. One request per
//! line, one response per line. JSON rather than the repo's usual TOML because
//! APEX Shell is the third consumer after the daemon and the CLI, and QML
//! parses JSON natively while it has no TOML reader at all.
//!
//! `Attach` is the one verb that changes the shape of the connection: the
//! daemon answers with a normal response line and then the *same* connection
//! becomes a raw bidirectional pipe to the session's PTY. Terminal resizes do
//! not travel down that pipe — they arrive as an ordinary [`Request::Resize`]
//! on a second, short-lived connection. Multiplexing control frames into a byte
//! stream that must stay transparent to arbitrary terminal output is how you
//! end up corrupting somebody's editor.
//!
//! This is a stability surface. APEX Shell reads [`SessionInfo`] to draw the
//! Agent Center, so field renames are breaking changes and need the same care
//! as `org.apexos.Apexd1`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::origin::OriginSource;
use crate::policy::{AgentPolicy, RequestOrigin};

/// Protocol revision. Bumped when a change is not backward compatible; the
/// daemon reports it in [`Response::Hello`] so a mismatched CLI can say so
/// plainly instead of failing on a missing field.
///
/// 2 — the six permission dimensions (§3.1). The added keys are flattened
/// alongside `sandbox` rather than nested under it, so an older daemon still
/// reads a new client's `sandbox` correctly. What it does *not* read is the
/// other five: a `--network offline` an old daemon ignores is a session that
/// runs with the network, which is exactly the fail-open a version number
/// exists to catch. The CLI compares this against [`Response::Hello`] and
/// refuses to send a non-default dimension to a daemon that predates it.
///
/// 3 — `request_origin` (§7). Same failure shape and the worse instance of
/// it: a daemon that predates this drops a declared `claude-remote-control`
/// and records the session as whatever it observed, which is local. A remote
/// session filed under a local origin is precisely the thing
/// `request_origin` exists to prevent, so the CLI refuses to send a
/// declaration to a daemon below [`REQUEST_ORIGIN_VERSION`].
///
/// 4 — the credential store left this daemon (§11, P0-002). `SecretGrant` and
/// `SecretGrants` are gone from this protocol, because a grant is now a change
/// to `apex-secretd`'s own store and the CLI asks it directly; `Brokered`
/// gained `audit_id` and `endpoint`. The failure this guards is quieter than
/// the two above and still worth naming: a daemon below this reads its OWN old
/// store, so a credential added to the secret service is simply not found and
/// the user is told they never stored it.
///
/// 5 — capabilities became generic (§13.2, §14, P1-001), `SecretUse` began
/// carrying a message body (§10, P0-003), and system access became a grant
/// (§4.4, §4.5, P0-006/P0-007). Three changes, one revision, because they
/// landed together.
///
/// `SecretUse` used to carry git's arguments as fields — `capability`,
/// `remote`, `branch` — which meant every provider §14 names would have had to
/// widen this protocol. It now carries an operation id, a resource, a declared
/// parameter map and an optional body, so `cloudflare.worker.deploy` and
/// `mcp.request` both need nothing here. A daemon below this does not
/// understand `operation` and answers as though no capability was named, so
/// the CLI refuses to ask one.
///
/// `--system-access` and `--unsafe-everything` used to be refused by every
/// build, so a client could send them to any daemon and get the same honest
/// no. Now they are a request for a grant, carrying a `ttl_ms` a daemon below
/// this drops — and a break-glass session that started with no TTL would be a
/// break-glass session that never expires, which is the one thing §3.4 forbids
/// outright. The CLI refuses to send either mode to a daemon below
/// [`SYSTEM_GRANT_VERSION`].
pub const PROTOCOL_VERSION: u32 = 5;

/// The revision at which the credential store moved to `apex-secretd`.
///
/// Named for the same reason the two below it are: the check is a boundary,
/// and a bare `< 4` in the CLI is one careless edit away from meaning nothing.
pub const BROKERED_SECRET_SERVICE_VERSION: u32 = 4;

/// The revision at which a capability stopped being git-shaped.
///
/// Below this, `SecretUse` has `capability`/`remote`/`branch` and no
/// `operation`, so a request from a current CLI deserialises into a request for
/// nothing. Named for the same reason as the three around it.
pub const GENERIC_CAPABILITY_VERSION: u32 = 5;

/// The revision at which `SecretUse` began carrying a message body.
///
/// `apex mcp bridge` checks it. A daemon below this parses the request, ignores
/// the field it has never heard of, and forwards a capability with no message —
/// which the secret service refuses, correctly, with an error about an empty
/// message that says nothing about the actual cause. Named so the bridge can
/// say the actual cause instead.
///
/// The same number as [`GENERIC_CAPABILITY_VERSION`], and defined as it rather
/// than written out: generic capabilities and the message body shipped in one
/// revision, so there is one wire change and two reasons a daemon below it
/// cannot serve this request. Two names, because the two reasons are what a
/// reader of either guard needs to know.
pub const MCP_BRIDGE_VERSION: u32 = GENERIC_CAPABILITY_VERSION;

/// The revision that first issued system-access grants.
///
/// `apex agent run --system-access session` and `--unsafe-everything` check
/// it. Below this both modes are refused by the daemon outright, so a client
/// that sent one anyway would get a refusal about the mode rather than about
/// the daemon — and the `ttl_ms` beside it would be dropped in silence, which
/// is a break-glass session with no expiry.
///
/// The same number as [`GENERIC_CAPABILITY_VERSION`], and defined as it for
/// the same reason [`MCP_BRIDGE_VERSION`] is: the three changes shipped in one
/// revision, so there is one wire change and three reasons a daemon below it
/// cannot serve a current client. Three names, because the three reasons are
/// what a reader of any one guard needs to know.
pub const SYSTEM_GRANT_VERSION: u32 = GENERIC_CAPABILITY_VERSION;

/// The guards arrive in order, checked when the crate compiles rather than
/// when a test runs: they are facts about three constants, and a revision
/// numbered behind the one before it would make a `<` comparison in the CLI
/// mean something nobody intended.
const _: () = assert!(POLICY_DIMENSIONS_VERSION < REQUEST_ORIGIN_VERSION);
const _: () = assert!(REQUEST_ORIGIN_VERSION < BROKERED_SECRET_SERVICE_VERSION);
const _: () = assert!(BROKERED_SECRET_SERVICE_VERSION < GENERIC_CAPABILITY_VERSION);
// Not `<`: the three are one revision, and a guard claiming otherwise would
// make a later `<` comparison in the bridge or the CLI mean something nobody
// intended.
const _: () = assert!(MCP_BRIDGE_VERSION == GENERIC_CAPABILITY_VERSION);
const _: () = assert!(SYSTEM_GRANT_VERSION == GENERIC_CAPABILITY_VERSION);

/// The revision that first carried the six dimensions.
///
/// Named rather than written as a literal at the comparison, because the check
/// is a security boundary and a bare `< 2` in the CLI is one careless edit away
/// from meaning nothing.
pub const POLICY_DIMENSIONS_VERSION: u32 = 2;

/// The revision that first carried `request_origin`, for the same reason.
pub const REQUEST_ORIGIN_VERSION: u32 = 3;

/// What a session is doing. The five user-facing values come straight from the
/// roadmap's agent event protocol; `Starting` and `Exited` are the lifecycle
/// bookends the runtime itself owns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentState {
    /// Spawned, no output observed yet.
    Starting,
    /// Producing output.
    Working,
    /// Quiet, still alive — most likely waiting on the human.
    WaitingForUser,
    /// Asked for a permission decision.
    PermissionRequest,
    /// Finished successfully.
    Complete,
    /// Finished unsuccessfully.
    Failed,
    /// Process is gone and the exit status has been recorded.
    Exited,
}

impl AgentState {
    /// The wire/display name.
    pub fn as_str(&self) -> &'static str {
        match self {
            AgentState::Starting => "starting",
            AgentState::Working => "working",
            AgentState::WaitingForUser => "waiting_for_user",
            AgentState::PermissionRequest => "permission_request",
            AgentState::Complete => "complete",
            AgentState::Failed => "failed",
            AgentState::Exited => "exited",
        }
    }

    /// Parse a state published by a cooperating client (`apex agent event`).
    /// Unknown names are rejected rather than mapped to a default, so a typo in
    /// a user's agent hook surfaces as an error instead of silently reporting
    /// the wrong thing in the Agent Center.
    pub fn parse(s: &str) -> Option<AgentState> {
        match s {
            "starting" => Some(AgentState::Starting),
            "working" => Some(AgentState::Working),
            "waiting_for_user" | "waiting" => Some(AgentState::WaitingForUser),
            "permission_request" | "permission" => Some(AgentState::PermissionRequest),
            "complete" | "completed" | "done" => Some(AgentState::Complete),
            "failed" | "error" => Some(AgentState::Failed),
            "exited" => Some(AgentState::Exited),
            _ => None,
        }
    }

    /// True once the process is gone. Terminal states are never overwritten by
    /// a late observation from the output detector.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            AgentState::Complete | AgentState::Failed | AgentState::Exited
        )
    }
}

impl std::fmt::Display for AgentState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // `pad`, not `write_str`: these are printed in aligned columns, and
        // width specifiers are silently ignored by a Display impl that writes
        // directly.
        f.pad(self.as_str())
    }
}

/// Dimension 2 of §3.1: how much of the filesystem and process table a session
/// may reach. See `sandbox.rs` for what each one actually builds, and
/// `policy.rs` for the other five dimensions this one is deliberately not
/// coupled to.
///
/// It stays in `protocol.rs` because it is the one dimension that predates the
/// split and is therefore a wire-compatibility surface in its own right;
/// `policy::SandboxPolicy` re-exports it so the six can be reached from one
/// place.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SandboxPolicy {
    /// No confinement. The escape hatch — an agent runs exactly as the user
    /// would have run it by hand.
    Unrestricted,
    /// The default. Project files writable, the rest of `$HOME` invisible,
    /// `/usr` read-only, no camera or microphone, network allowed.
    #[default]
    Project,
    /// `Project` with the network removed.
    Strict,
}

impl SandboxPolicy {
    /// Every value, so a test that has to hold for all of them can say so
    /// rather than listing three and missing the fourth somebody adds.
    pub const ALL: &'static [SandboxPolicy] = &[
        SandboxPolicy::Unrestricted,
        SandboxPolicy::Project,
        SandboxPolicy::Strict,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            SandboxPolicy::Unrestricted => "unrestricted",
            SandboxPolicy::Project => "project",
            SandboxPolicy::Strict => "strict",
        }
    }

    pub fn parse(s: &str) -> Option<SandboxPolicy> {
        match s {
            "unrestricted" | "none" | "off" => Some(SandboxPolicy::Unrestricted),
            "project" => Some(SandboxPolicy::Project),
            "strict" => Some(SandboxPolicy::Strict),
            _ => None,
        }
    }

    /// Whether this policy runs the process under `bwrap` at all.
    pub fn is_confined(&self) -> bool {
        !matches!(self, SandboxPolicy::Unrestricted)
    }
}

impl std::fmt::Display for SandboxPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.pad(self.as_str())
    }
}

/// Everything the CLI and the shell need to describe one session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfo {
    pub id: u32,
    /// Adapter id (`claude`, `codex`, `generic`, …).
    pub agent: String,
    /// The program actually executed, before sandbox wrapping.
    pub program: String,
    pub args: Vec<String>,
    /// Working directory inside the session.
    pub cwd: String,
    /// Project root, when `cwd` sits inside a detected project.
    pub project: Option<String>,
    /// Project name for display, when known.
    pub project_name: Option<String>,
    /// Git worktree this session was given, when it was created with one.
    pub worktree: Option<String>,
    pub state: AgentState,
    /// Free-text detail attached to the current state by a published event.
    pub detail: Option<String>,
    /// Whether the session is stopped (`apex agent pause`).
    ///
    /// A separate field rather than a value inside `detail`, which is where it
    /// started. `detail` is free text that any cooperating client can write
    /// with `apex agent event --detail`, so an agent could set it to "paused"
    /// and make the Agent Center offer Resume on a running session — a control
    /// that reads the wrong state is worse than no control. This one only the
    /// runtime writes, and only when it has actually delivered the signal.
    ///
    /// `#[serde(default)]` so a record written by an older daemon still loads;
    /// the default is "not paused", which is what an absent field meant.
    #[serde(default)]
    pub paused: bool,
    /// The six permission dimensions this session actually runs under, already
    /// normalised by the daemon.
    ///
    /// Flattened, not nested: `sandbox` stays a top-level key, so APEX Shell's
    /// existing read of it keeps working and a record written before the split
    /// still loads with the other five at their defaults. Nesting would have
    /// moved the key and silently reported every old session as `project`.
    #[serde(flatten)]
    pub policy: AgentPolicy,
    /// Where this session is driven from (§7's `request_origin`).
    ///
    /// Established by the daemon when it forked the session, from the
    /// connection that asked for it — never from the request. `None` is a
    /// record written before origin tracking existed, and it is deliberately
    /// not `local-terminal`: an absent field must not read as the origin §7
    /// reserves root for. Policy treats `None` as non-local.
    ///
    /// `request_origin`, not `origin`, because [`AgentPolicy`] is flattened
    /// into this struct and already owns the `origin` key for dimension 6.
    /// The two are different things: dimension 6 is which origins may
    /// authorise elevation, this is which origin is asking.
    #[serde(default)]
    pub request_origin: Option<RequestOrigin>,
    /// How [`SessionInfo::request_origin`] was arrived at.
    #[serde(default)]
    pub origin_source: Option<OriginSource>,
    /// The system-access grant this session runs under, when it has one.
    ///
    /// Present exactly when `policy.system` is not `none`, because the daemon
    /// will not start such a session without a grant. It is the id, not the
    /// grant: the record is on disk and the state depends on the clock, so a
    /// field carrying "active" would be stale the moment it was written.
    #[serde(default)]
    pub grant: Option<u32>,
    /// Unix milliseconds at which that grant runs out.
    ///
    /// Carried beside the id so the Agent Center and `apex agent list` can
    /// render a countdown without a second round trip, and so the red
    /// indicator §3.4 asks for can say how much of the window is left. The
    /// grant record remains the authority.
    #[serde(default)]
    pub grant_expires_ms: Option<u64>,
    /// The agent's own report of its own permission mode (dimension 1).
    ///
    /// §4.1 says APEX passes no permission flag and lets the agent's profile
    /// decide, so `policy.native` reads `inherit` for the normal case — which
    /// says what APEX did, not what the agent is doing. Claude reports its
    /// real mode on every hook event, and this is where that lands, so the
    /// Agent Center can show `bypassPermissions` rather than `inherit`.
    ///
    /// Deliberately NOT a permission input. Dimension 1 is the agent's own
    /// layer and APEX does not enforce it; this is the agent describing
    /// itself, in the same class as `detail`, and nothing branches on it.
    /// `None` until the agent has said, and for agents that never do.
    #[serde(default)]
    pub native_observed: Option<String>,
    /// PID of the session leader (the sandbox wrapper when confined).
    pub pid: i32,
    /// Unix seconds when the session was created.
    pub started: u64,
    /// Unix seconds of the most recent PTY output or published event.
    pub last_activity: u64,
    /// Exit status, once the process is gone.
    pub exit_code: Option<i32>,
    /// Signal that killed the process, when it died from one.
    pub exit_signal: Option<i32>,
    /// Number of clients currently attached.
    pub attached: u32,
    /// Checkpoint taken before the session started, when one was requested.
    pub checkpoint: Option<String>,
    pub cols: u16,
    pub rows: u16,
}

impl SessionInfo {
    /// True while the process is still around.
    pub fn is_live(&self) -> bool {
        self.exit_code.is_none() && self.exit_signal.is_none()
    }

    /// Human-readable exit summary, or `None` while still running.
    pub fn exit_summary(&self) -> Option<String> {
        if let Some(sig) = self.exit_signal {
            return Some(format!("killed by signal {sig}"));
        }
        self.exit_code.map(|c| {
            if c == 0 {
                "exited 0".to_string()
            } else {
                format!("exited {c}")
            }
        })
    }
}

/// A control request. `cmd` is the tag, so the wire form reads
/// `{"cmd":"attach","id":4,...}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Request {
    /// Protocol handshake. Cheap, and the only request a mismatched client can
    /// rely on.
    Hello,
    /// Start a session.
    Run(RunRequest),
    /// Every session the daemon knows about, newest last.
    List,
    /// One session.
    Info { id: u32 },
    /// Take over the session's PTY. The response line is followed by raw bytes.
    Attach {
        id: u32,
        cols: u16,
        rows: u16,
        /// Replay up to this many bytes of scrollback before live output.
        /// Zero means "no replay".
        #[serde(default = "default_replay")]
        replay: usize,
    },
    /// Tell the PTY its window changed. Sent on its own connection.
    Resize { id: u32, cols: u16, rows: u16 },
    /// Write text into a live session's terminal.
    ///
    /// The same write [`Request::Attach`] already performs, without the read
    /// half. `handle_attach` turns its connection into the session's terminal
    /// and pumps the client's stdin into the PTY master; a client that has one
    /// thing to say and nothing to display needs only that half. APEX Shell's
    /// push-to-talk route is the first: it holds a transcript and owns no
    /// terminal.
    ///
    /// `data` is written verbatim and no byte is added. Whether the line is
    /// SENT is the caller's decision, because it is the difference between
    /// putting words in a prompt and making an agent act on them: `apex agent
    /// input --submit` appends the carriage return that means Enter, and
    /// without it the text waits in the prompt for a person.
    ///
    /// Refused when the caller is itself a managed session. Every other verb
    /// on this socket is either a question or an action on the caller's own
    /// session; this one puts words in another agent's mouth, and hooks run
    /// inside the sandbox with reach to this socket.
    ///
    /// Not a protocol bump, by the criterion on [`Request::Event`] below: a
    /// daemon that predates this answers "unparseable request", the client
    /// reports that it could not deliver, and nothing has been typed. The
    /// failure loses a message, never a restriction.
    Input { id: u32, data: String },
    /// Deliver a signal by name (`int`, `term`, `kill`, `stop`, `cont`).
    Signal { id: u32, signal: String },
    /// Publish a state transition. This is the open event protocol: any client
    /// that knows its session id can report what it is doing.
    ///
    /// Both payload fields are optional, and the two absences mean different
    /// things. No `state` is an event that records something without changing
    /// what the session is doing — a task created, a compaction, a config
    /// change; §6.1 asks for those and none of them is a state. No `event` is
    /// the original form, which every existing client still sends and which
    /// still works: a bare state with no lifecycle name attached.
    ///
    /// Deliberately additive rather than a new request. A daemon that predates
    /// this drops `event`, keeps `state`, and reports exactly what it reported
    /// before — the version guards above exist for changes where an old daemon
    /// dropping a key loses a *restriction*, and this one loses only detail.
    /// It is therefore not a protocol bump, which matters when three branches
    /// are open on this file at once.
    Event {
        id: u32,
        #[serde(default)]
        state: Option<String>,
        /// One of [`crate::hook::HookEvent`]'s names, when the publisher has a
        /// lifecycle event rather than an opinion about state.
        #[serde(default)]
        event: Option<String>,
        #[serde(default)]
        detail: Option<String>,
        /// The agent's own report of its own permission mode (§4.1).
        ///
        /// Claude puts `permission_mode` on every hook payload, so the bridge
        /// carries it here and the daemon records it on
        /// [`SessionInfo::native_observed`]. It is what makes dimension 1
        /// visible in the Agent Center: `policy.native` says what APEX did,
        /// which for the default is *nothing*, and "inherit" is not a
        /// permission mode a user recognises.
        ///
        /// Not a permission input, and no daemon behaviour branches on it.
        /// Dimension 1 is the agent's own layer; APEX neither enforces nor
        /// second-guesses it, so an agent reporting it is the authority on it
        /// in exactly the way an agent reporting its own `detail` is.
        #[serde(default)]
        native: Option<String>,
    },
    /// Ask whether a tool call this session is about to make is one its own
    /// confinement would refuse (§6.2).
    ///
    /// Asked by `apex agent hook pre_tool_use`, which runs inside the sandbox
    /// and so knows three environment variables and nothing about the mounts.
    /// The daemon holds the spec it built, so the decision is made where the
    /// evidence is.
    ///
    /// Not a protocol bump, for the same reason the optional keys on
    /// [`Request::Event`] are not: a daemon that predates this answers "unknown
    /// request", the hook prints nothing, Claude proceeds, and the sandbox that
    /// daemon did build refuses the operation exactly as it always would have.
    /// The failure loses a message, never a restriction — which is the test the
    /// version guards above are applying.
    ToolCheck {
        id: u32,
        tool_name: String,
        /// The tool's own arguments, verbatim from Claude. Untyped because
        /// every tool shapes them differently and a typed union would break on
        /// the next tool added upstream.
        #[serde(default)]
        tool_input: serde_json::Value,
    },
    /// Read the tail of a session's transcript.
    Logs {
        id: u32,
        #[serde(default = "default_log_bytes")]
        bytes: usize,
    },
    /// Forget an exited session (and delete its transcript).
    Remove { id: u32 },
    /// Forget every exited session.
    Prune,

    /// Narrow the calling session's own origin (§7).
    ///
    /// No session id, for the same reason the privilege verbs have none: the
    /// daemon resolves the session from the connection's peer credentials, so
    /// a session can only ever speak about itself. `origin` must be one
    /// [`RequestOrigin::may_be_declared`] accepts and must be at least as
    /// restricted as what the session already has — a session cannot declare
    /// its way back to local, and a Remote Control session cannot declare its
    /// way out of the lock gate.
    ///
    /// This is what Remote Control uses. It is enabled after `claude` has
    /// started, so the session was genuinely local when it was created and
    /// nothing observable about the connection ever changes.
    DeclareOrigin { origin: String },

    // ── privilege requests (§4) ─────────────────────────────────────────────
    //
    // Note what is absent: no session id. The daemon resolves the asking
    // session from the connection's peer credentials, because
    // `$APEX_AGENT_SESSION` lives inside a sandbox the agent controls and
    // anything authorised by a client-supplied id is authorised by the agent.
    /// Ask for a privileged operation. `verb` must name one of
    /// [`crate::request::Verb::names`]; the daemon parses and validates it.
    PrivilegeRequest {
        verb: String,
        #[serde(default)]
        args: Vec<String>,
        reason: String,
    },
    /// Every privilege request on record.
    Requests,
    /// Record a human's decision on one. Refused when the connection belongs
    /// to a session — an agent may not approve itself.
    Decide { id: u32, decision: String },
    /// Report that an approved request has been run, with its exit status.
    RequestExecuted { id: u32, exit_code: i32 },
    /// Per-project grants.
    Grants,
    /// Drop a grant, or every grant for the project when `key` is absent.
    Revoke {
        project: String,
        #[serde(default)]
        key: Option<String>,
    },

    // ── system-access grants (§4.4, §4.5) ───────────────────────────────────
    //
    // Named `SystemGrant*` and not `Grant*`: the four verbs above are
    // P0-013's per-project verb grants, which are a different thing with a
    // different store, and one vocabulary meaning two things is how a client
    // ends up revoking the wrong one.
    //
    // None of these carries a session id either, for the same reason
    // `PrivilegeRequest` does not.
    /// Every system-access grant on record, with the state each is in now.
    SystemGrants,
    /// Take a grant back before its TTL runs out.
    ///
    /// Refused when the connection belongs to a managed session: a session
    /// revoking its own grant is harmless, but a session revoking ANOTHER
    /// session's is not, and the daemon does not have to tell the two apart
    /// if neither is allowed.
    RevokeSystemGrant { id: u32 },
    /// Extend a grant that is still active.
    ///
    /// P0-007's fourth criterion — "the agent cannot renew its own grant" —
    /// is enforced here, and it is the reason this verb exists at all rather
    /// than renewal being a side effect of asking again. The refusal is not
    /// "are you the user": the agent *is* the user. It is that the connection
    /// resolves, through `SO_PEERCRED` and `/proc` ancestry, to a managed
    /// session — and nothing running inside one can present a connection that
    /// does not.
    RenewSystemGrant {
        id: u32,
        /// The new window, from now. Bounded like any other, and it is a
        /// fresh authentication rather than an extension of the old consent.
        ttl_ms: u64,
    },

    // ── the secret broker (§4) ──────────────────────────────────────────────
    //
    // Note what `SecretUse` does NOT carry: a session id, and a URL. The
    // session comes from the connection's peer credentials, and the resource is
    // a NAME the provider resolves for itself — a URL would let a session
    // choose where its token gets sent.
    //
    // Note also what it does not carry any more: git. `capability`, `remote`
    // and `branch` were one provider's arguments in a protocol every provider
    // has to fit through, and P1-001 replaced them with an operation id, a
    // resource and a declared parameter map. This is what "provider plugins can
    // be added without changing agent core" means concretely — a Cloudflare
    // worker deployment is `operation: "cloudflare.worker.deploy"` and needs no
    // line here.
    /// Ask the broker to perform a capability. The token never comes back.
    SecretUse {
        service: String,
        /// The §13.2 operation id: `git.push`, `cloudflare.worker.deploy`.
        operation: String,
        /// What it acts on, as a NAME. Empty for an operation that names
        /// nothing, such as reading an account.
        #[serde(default)]
        resource: String,
        /// The operation's own arguments. Checked by `apex-secretd` against
        /// what the provider declared; an undeclared one is refused, so this
        /// map is not a way to smuggle a command line through.
        #[serde(default)]
        params: BTreeMap<String, String>,
        /// The message a capability carries, for the one operation that carries
        /// a message: `mcp.request`.
        ///
        /// A field here and raw bytes on the secret service's own wire, which
        /// is not an inconsistency. That protocol caps a request line because
        /// any local process may write one; this one does not, and a second
        /// framing convention on a socket APEX Shell also parses would be a
        /// compatibility surface for no gain. What both refuse is a credential
        /// in a serialisable type, and a JSON-RPC message the caller wrote is
        /// not one.
        ///
        /// Separate from `params` deliberately. A parameter is declared by the
        /// operation and checked against its syntax; a message body is opaque
        /// bytes the provider forwards, and putting it in the map would mean
        /// declaring a parameter whose value nothing can validate.
        #[serde(default)]
        body: Option<String>,
        /// The caller's project root.
        ///
        /// Honoured ONLY when the peer is not a managed session. A session's
        /// project is whatever the daemon recorded when it forked it, and this
        /// field is ignored for one — otherwise a confined agent could claim
        /// to be in a project whose capabilities it was never granted.
        ///
        /// It has to be sent because the daemon cannot see the caller's
        /// working directory: resolving it from `current_dir()` gave the
        /// DAEMON's cwd, so every grant silently failed to match.
        #[serde(default)]
        project: Option<String>,
    },
}

impl Request {
    /// Whether answering this can block on a person at a keyboard.
    ///
    /// §4.4 and §4.5 both authenticate, and polkit dispatches the challenge to
    /// the login session's own agent — a dialog on the desktop, not in the
    /// PTY. The daemon sits in `pkcheck` until the person answers it, which is
    /// as long as they take. The control socket's ordinary read timeout is
    /// generous but finite, and a person who walks away from the dialog would
    /// otherwise get a socket error from the CLI while the dialog is still on
    /// screen and the grant is still being decided behind it — a failure
    /// message about the wrong thing entirely.
    ///
    /// Erring towards `true` costs nothing: it removes a deadline from one
    /// request, and the daemon still answers when it is done.
    pub fn waits_on_a_human(&self) -> bool {
        match self {
            Request::Run(r) => r.policy.needs_grant().is_some(),
            Request::RenewSystemGrant { .. } => true,
            _ => false,
        }
    }
}

fn default_replay() -> usize {
    crate::session::SCROLLBACK_BYTES
}

fn default_log_bytes() -> usize {
    64 * 1024
}

/// The parameters of a new session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunRequest {
    /// Adapter id, or `None` to use the configured default agent.
    #[serde(default)]
    pub agent: Option<String>,
    /// Prompt handed to the agent, when it takes one.
    #[serde(default)]
    pub prompt: Option<String>,
    /// Extra arguments appended after the adapter's own.
    #[serde(default)]
    pub args: Vec<String>,
    /// Run here. Must be absolute.
    pub cwd: String,
    /// The six permission dimensions, flattened for the same reason as
    /// [`SessionInfo::policy`]: an older client sends `{"sandbox":"strict"}`
    /// and nothing else, and that must keep meaning what it meant.
    ///
    /// The daemon normalises and validates this; it never trusts it as the
    /// final word, because a client is free to send any combination.
    #[serde(flatten)]
    pub policy: AgentPolicy,
    /// A declared origin for the new session (§7).
    ///
    /// `None` — the usual case — means "observe it", and the daemon derives
    /// the origin from the connection asking. A value here is a *declaration*,
    /// checked by [`crate::origin::may_declare`] against what was observed and
    /// only ever accepted when it gives something up. A local origin is never
    /// accepted here, whatever is sent.
    #[serde(default)]
    pub request_origin: Option<RequestOrigin>,
    /// Create/reuse this git worktree under the project and run there.
    #[serde(default)]
    pub worktree: Option<String>,
    /// Take a checkpoint before starting.
    #[serde(default)]
    pub checkpoint: bool,
    /// How long a system-access grant should last, in milliseconds (§3.4's
    /// `--ttl`).
    ///
    /// Meaningless without an elevated `policy.system`, and the daemon refuses
    /// the pair rather than ignoring it: a `--ttl` on an ordinary session is a
    /// user who believes they asked for something they did not.
    ///
    /// `None` with `--system-access session` takes the default window;
    /// `None` with `--unsafe-everything` is refused, because §3.4 asks for the
    /// window to be explicit. See [`crate::grant::ttl_for`].
    #[serde(default)]
    pub ttl_ms: Option<u64>,
    pub cols: u16,
    pub rows: u16,
    /// Environment additions, applied after the sandbox is built.
    #[serde(default)]
    pub env: Vec<(String, String)>,
}

/// A control response.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "reply", rename_all = "snake_case")]
pub enum Response {
    Hello {
        version: u32,
        /// Adapter ids the daemon can launch.
        agents: Vec<String>,
        /// Configured default agent.
        default_agent: String,
    },
    /// A session was created or inspected.
    Session(Box<SessionInfo>),
    /// A list of sessions.
    ///
    /// A struct variant, not a newtype around the `Vec`: serde's
    /// internally-tagged representation cannot serialize a newtype variant
    /// containing a sequence, and fails at runtime rather than at compile
    /// time. `every_response_variant_round_trips` pins this.
    Sessions { sessions: Vec<SessionInfo> },
    /// Attach accepted; the connection is now a raw PTY pipe.
    Attached { id: u32 },
    Logs {
        id: u32,
        /// UTF-8 lossy transcript tail.
        text: String,
    },
    /// A privilege request was filed, decided or executed.
    Request(Box<crate::request::PrivilegeRequest>),
    /// A list of privilege requests.
    ///
    /// A struct variant for the same reason as `Sessions`: serde's
    /// internally-tagged representation cannot serialize a newtype variant
    /// wrapping a sequence, and it fails at runtime rather than at compile
    /// time.
    Requests {
        requests: Vec<crate::request::PrivilegeRequest>,
    },
    /// Per-project grants: project root -> grant keys.
    Grants {
        projects: std::collections::BTreeMap<String, Vec<String>>,
    },
    /// System-access grants, each with the state it is in right now.
    ///
    /// The state is computed by the daemon and sent, rather than left for the
    /// client to derive: it depends on the running kernel's boot id, and a
    /// client deriving it would have to read `/proc` itself and could get a
    /// different answer from the daemon that issued the grant.
    SystemGrants {
        grants: Vec<crate::grant::SystemGrant>,
        /// Same order as `grants`: the state word, and the sentence.
        states: Vec<(String, String)>,
    },
    /// A brokered capability ran. Carries the RESULT, never the credential.
    Brokered {
        service: String,
        capability: String,
        /// The operation in words, for the log and the transcript.
        detail: String,
        /// The secret service's audit id for this operation, so a report can
        /// cite the trail entry rather than describing it.
        #[serde(default)]
        audit_id: String,
        /// Scheme and host the credential was sent to, as `apex-secretd`
        /// resolved it from the repository. The caller sees where its operation
        /// went without being able to choose it.
        #[serde(default)]
        endpoint: String,
        exit_code: i32,
        /// git's own output, with the credential scrubbed out.
        output: String,
    },
    /// The §6.2 policy point's answer to one `PreToolUse`.
    ///
    /// `deny` absent is an allow, and absent is also what every failure on the
    /// way here produces — an unreachable daemon, a session that has gone, a
    /// request an older daemon does not know. A hook that cannot get an answer
    /// must not invent a refusal, and it does not need to: the sandbox is what
    /// enforces this, and it is still there.
    ToolDecision {
        #[serde(default)]
        deny: Option<String>,
    },
    /// Verb succeeded and has nothing to say.
    Ok,
    /// Verb failed. `kind` is stable enough to branch on; `message` is for
    /// humans.
    Error { kind: ErrorKind, message: String },
}

/// Failure categories a client may reasonably branch on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    /// No session with that id.
    NoSuchSession,
    /// The session exists but has already exited.
    SessionExited,
    /// Unknown adapter name.
    NoSuchAgent,
    /// The request was malformed or self-contradictory.
    BadRequest,
    /// The sandbox could not be built as requested. Never downgraded silently.
    SandboxUnavailable,
    /// A policy dimension this build cannot enforce was asked for. Distinct
    /// from [`ErrorKind::SandboxUnavailable`], whose remedy is "re-run with
    /// `--sandbox unrestricted`" — advice that would be actively wrong for a
    /// system-access refusal, since loosening the sandbox is not what the user
    /// was denied.
    PolicyRefused,
    /// No privilege request with that id.
    NoSuchRequest,
    /// The caller is not allowed to do this — notably, a session trying to
    /// decide its own privilege request.
    PermissionDenied,
    /// Anything else, including OS errors.
    Internal,
}

impl Response {
    /// Build an error response.
    pub fn error(kind: ErrorKind, message: impl Into<String>) -> Response {
        Response::Error {
            kind,
            message: message.into(),
        }
    }

    /// The error message, if this is an error.
    pub fn as_error(&self) -> Option<(ErrorKind, &str)> {
        match self {
            Response::Error { kind, message } => Some((*kind, message.as_str())),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_names_round_trip() {
        for s in [
            AgentState::Starting,
            AgentState::Working,
            AgentState::WaitingForUser,
            AgentState::PermissionRequest,
            AgentState::Complete,
            AgentState::Failed,
            AgentState::Exited,
        ] {
            assert_eq!(AgentState::parse(s.as_str()), Some(s), "{s}");
        }
    }

    #[test]
    fn unknown_state_is_rejected_not_defaulted() {
        assert_eq!(AgentState::parse("busy"), None);
        assert_eq!(AgentState::parse(""), None);
        assert_eq!(AgentState::parse("Working"), None);
    }

    #[test]
    fn policy_names_round_trip() {
        for p in [
            SandboxPolicy::Unrestricted,
            SandboxPolicy::Project,
            SandboxPolicy::Strict,
        ] {
            assert_eq!(SandboxPolicy::parse(p.as_str()), Some(p), "{p}");
        }
        assert_eq!(SandboxPolicy::parse("loose"), None);
    }

    #[test]
    fn default_policy_is_project_not_unrestricted() {
        // A default that fails open would make every unqualified `apex agent
        // run` an unconfined one.
        assert_eq!(SandboxPolicy::default(), SandboxPolicy::Project);
        assert!(SandboxPolicy::default().is_confined());
    }

    #[test]
    fn display_honours_column_width() {
        // These are printed in aligned tables; a Display impl that ignores the
        // width specifier silently breaks every listing.
        assert_eq!(format!("[{:<10}]", SandboxPolicy::Project), "[project   ]");
        assert_eq!(format!("[{:<18}]", AgentState::Working), "[working           ]");
        assert_eq!(format!("[{:>8}]", SandboxPolicy::Strict), "[  strict]");
    }

    #[test]
    fn terminal_states_are_exactly_the_finished_ones() {
        assert!(AgentState::Complete.is_terminal());
        assert!(AgentState::Failed.is_terminal());
        assert!(AgentState::Exited.is_terminal());
        assert!(!AgentState::Working.is_terminal());
        assert!(!AgentState::WaitingForUser.is_terminal());
        assert!(!AgentState::PermissionRequest.is_terminal());
        assert!(!AgentState::Starting.is_terminal());
    }

    #[test]
    fn requests_serialise_with_a_cmd_tag() {
        let json = serde_json::to_string(&Request::Info { id: 7 }).unwrap();
        assert!(json.contains(r#""cmd":"info""#), "{json}");
        assert!(json.contains(r#""id":7"#), "{json}");
    }

    #[test]
    fn attach_replay_defaults_when_the_field_is_absent() {
        let req: Request = serde_json::from_str(r#"{"cmd":"attach","id":1,"cols":80,"rows":24}"#)
            .expect("parse without replay");
        match req {
            Request::Attach { replay, .. } => {
                assert_eq!(replay, crate::session::SCROLLBACK_BYTES)
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn input_carries_its_text_through_the_wire_byte_for_byte() {
        // The payload is a person's words, so it can hold anything a keyboard
        // or a speech-to-text hook produces: a newline, a carriage return, a
        // quote, a backslash, a tab. The framing is NDJSON, so a raw newline
        // in the serialised line would desynchronise the stream for every
        // request after it, and the bytes typed into the agent's terminal have
        // to be the bytes the caller asked for and no others.
        let text = "say \"hi\"\tthen\\stop\nrun it\r";
        let req = Request::Input {
            id: 4,
            data: text.to_string(),
        };
        let line = serde_json::to_string(&req).expect("serialise");
        assert!(!line.contains('\n'), "{line} would break NDJSON framing");
        assert!(!line.contains('\r'), "{line} would break NDJSON framing");
        match serde_json::from_str::<Request>(&line).expect("round-trip") {
            Request::Input { id, data } => {
                assert_eq!(id, 4);
                assert_eq!(data, text, "the payload changed on the wire");
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn input_is_tagged_input_and_takes_no_default() {
        // The shell calls `apex agent input <id> <text>` and the CLI builds
        // this; a rename of either the tag or a field is a silent break, since
        // an unknown `cmd` is answered as "unparseable request" and looks to
        // the shell exactly like an old daemon.
        let line = serde_json::to_string(&Request::Input {
            id: 9,
            data: "x".into(),
        })
        .unwrap();
        assert!(line.contains(r#""cmd":"input""#), "{line}");
        assert!(line.contains(r#""id":9"#), "{line}");
        assert!(line.contains(r#""data":"x""#), "{line}");

        // No `#[serde(default)]` on `data`: an Input with no text is a caller
        // bug, and defaulting it to the empty string would turn that into a
        // successful write of nothing.
        assert!(
            serde_json::from_str::<Request>(r#"{"cmd":"input","id":9}"#).is_err(),
            "an Input without text must not parse"
        );
    }

    #[test]
    fn run_request_sandbox_defaults_to_project_when_omitted() {
        let req: RunRequest =
            serde_json::from_str(r#"{"cwd":"/tmp","cols":80,"rows":24}"#).expect("parse");
        assert_eq!(req.policy.sandbox, SandboxPolicy::Project);
        assert_eq!(req.policy, AgentPolicy::default());
        assert!(req.worktree.is_none());
        assert!(!req.checkpoint);
    }

    #[test]
    fn responses_round_trip_through_json() {
        let r = Response::error(ErrorKind::NoSuchSession, "no session 9");
        let text = serde_json::to_string(&r).unwrap();
        let back: Response = serde_json::from_str(&text).unwrap();
        assert_eq!(
            back.as_error().map(|(k, _)| k),
            Some(ErrorKind::NoSuchSession)
        );
    }

    fn sample_request() -> crate::request::PrivilegeRequest {
        crate::request::PrivilegeRequest {
            id: 7,
            verb: crate::request::Verb::Install {
                packages: vec!["clang".into(), "cmake".into()],
            },
            reason: "Required to compile the project".into(),
            session: Some(4),
            agent: Some("claude".into()),
            project: Some("/home/t/p".into()),
            decision: crate::request::Decision::Pending,
            request_origin: Some(RequestOrigin::LocalTerminal),
            origin_source: Some(OriginSource::Inherited),
            created_ms: 1_700_000_000_000,
            decided_ms: None,
            executed_ms: None,
            exit_code: None,
            system_grant: None,
        }
    }

    fn sample_session() -> SessionInfo {
        SessionInfo {
            id: 1,
            agent: "claude".into(),
            program: "claude".into(),
            args: vec!["fix it".into()],
            cwd: "/home/t/p".into(),
            project: Some("/home/t/p".into()),
            project_name: Some("p".into()),
            worktree: None,
            state: AgentState::Working,
            detail: None,
            paused: false,
            policy: AgentPolicy::default(),
            request_origin: Some(RequestOrigin::LocalTerminal),
            origin_source: Some(OriginSource::Observed),
            grant: None,
            grant_expires_ms: None,
            native_observed: None,
            pid: 42,
            started: 1,
            last_activity: 2,
            exit_code: None,
            exit_signal: None,
            attached: 1,
            checkpoint: None,
            cols: 80,
            rows: 24,
        }
    }

    #[test]
    fn every_response_variant_round_trips() {
        // serde's internally-tagged enums reject some shapes only at runtime —
        // a newtype variant wrapping a Vec serialises to an error, not to
        // JSON, and the daemon discovers it by dropping the connection. Every
        // variant is exercised here so the failure is a test, not a hang.
        let variants = vec![
            Response::Hello {
                version: PROTOCOL_VERSION,
                agents: vec!["claude".into(), "generic".into()],
                default_agent: "claude".into(),
            },
            Response::Session(Box::new(sample_session())),
            Response::Sessions {
                sessions: vec![sample_session(), sample_session()],
            },
            Response::Attached { id: 3 },
            Response::Logs {
                id: 3,
                text: "output\n".into(),
            },
            // Response::Request is the riskiest shape in this enum: an
            // internally-tagged variant wrapping a struct that itself
            // #[serde(flatten)]s an internally-tagged enum. Both layers use a
            // map representation, which is the one case internal tagging
            // supports — but it is supported at RUNTIME, not by the type
            // system, so it is asserted rather than assumed.
            Response::Request(Box::new(sample_request())),
            Response::Requests {
                requests: vec![sample_request(), sample_request()],
            },
            Response::Grants {
                projects: [("/home/t/p".to_string(), vec!["install:clang".to_string()])]
                    .into_iter()
                    .collect(),
            },
            Response::Brokered {
                service: "github".into(),
                capability: "git-push".into(),
                detail: "git push origin feat/x".into(),
                audit_id: "1a07-1-0".into(),
                endpoint: "https://github.com".into(),
                exit_code: 0,
                output: "Everything up-to-date".into(),
            },
            Response::ToolDecision { deny: None },
            Response::ToolDecision {
                deny: Some("no".into()),
            },
            Response::Ok,
            Response::error(ErrorKind::Internal, "boom"),
        ];

        for v in variants {
            let text = serde_json::to_string(&v)
                .unwrap_or_else(|e| panic!("{v:?} does not serialise: {e}"));
            assert!(!text.contains('\n'), "{text} would break NDJSON framing");
            let _: Response = serde_json::from_str(&text)
                .unwrap_or_else(|e| panic!("{v:?} does not round-trip: {e} from {text}"));
        }
    }

    #[test]
    fn the_pause_flag_is_its_own_field_and_defaults_to_not_paused() {
        // It began life as `detail: Some("paused")`, which any cooperating
        // client can write with `apex agent event --detail` — so an agent could
        // make the Agent Center show Resume on a running session. This is the
        // field the UI branches on, and only the runtime writes it.
        let mut s = sample_session();
        assert!(!s.paused, "a fresh session is not paused");

        s.paused = true;
        let text = serde_json::to_string(&s).expect("serialise");
        let back: SessionInfo = serde_json::from_str(&text).expect("deserialise");
        assert!(back.paused, "{text}");

        // A record from an older daemon has no such key. It must load as
        // not-paused rather than failing, which is what `#[serde(default)]`
        // buys — asserted, because dropping the attribute would make every
        // pre-existing session record unreadable.
        let mut v: serde_json::Value = serde_json::from_str(&text).unwrap();
        v.as_object_mut().unwrap().remove("paused");
        let old: SessionInfo = serde_json::from_value(v).expect("an old record still loads");
        assert!(!old.paused);

        // And `detail` cannot set it.
        let mut v: serde_json::Value = serde_json::from_str(&text).unwrap();
        v["paused"] = serde_json::json!(false);
        v["detail"] = serde_json::json!("paused");
        let spoofed: SessionInfo = serde_json::from_value(v).unwrap();
        assert!(!spoofed.paused, "detail must not be able to claim paused");
    }

    #[test]
    fn a_privilege_request_survives_the_wire_with_its_verb_intact() {
        // Not just "it round-trips": the flattened verb must come back as the
        // same typed value, because the argv that eventually runs is built from
        // it. A shape that serialises but loses the packages would approve one
        // operation and run another.
        let original = sample_request();
        let wire = serde_json::to_string(&Response::Request(Box::new(original.clone())))
            .expect("serialise");
        let back: Response = serde_json::from_str(&wire).expect("deserialise");
        let Response::Request(r) = back else {
            panic!("wrong variant from {wire}");
        };
        assert_eq!(r.verb, original.verb, "{wire}");
        assert_eq!(r.argv(), original.argv());
        assert_eq!(r.reason, original.reason);
        assert_eq!(r.session, original.session);
        // And the tag lands where the protocol says it does.
        let v: serde_json::Value = serde_json::from_str(&wire).unwrap();
        assert_eq!(v["reply"], "request");
        assert_eq!(v["verb"], "install");
    }

    #[test]
    fn only_the_requests_that_authenticate_wait_on_a_person() {
        // The polkit dialog is on the desktop and the person may take a
        // minute or ten; the CLI must not give up on the socket and report a
        // connection failure while the dialog is still up.
        use crate::policy::SystemAccess;
        let run = |system| {
            Request::Run(RunRequest {
                agent: None,
                prompt: None,
                args: vec![],
                cwd: "/home/t/p".into(),
                policy: AgentPolicy {
                    system,
                    ..AgentPolicy::default()
                },
                request_origin: None,
                worktree: None,
                checkpoint: false,
                ttl_ms: None,
                cols: 80,
                rows: 24,
                env: vec![],
            })
        };
        assert!(run(SystemAccess::Session).waits_on_a_human());
        assert!(run(SystemAccess::Unsafe).waits_on_a_human());
        assert!(!run(SystemAccess::None).waits_on_a_human());
        assert!(Request::RenewSystemGrant { id: 1, ttl_ms: 60_000 }.waits_on_a_human());
        // Giving privilege up asks nobody, so it keeps its deadline.
        assert!(!Request::RevokeSystemGrant { id: 1 }.waits_on_a_human());
        assert!(!Request::SystemGrants.waits_on_a_human());
        assert!(!Request::List.waits_on_a_human());
    }

    #[test]
    fn every_request_variant_round_trips() {
        let variants = vec![
            Request::Hello,
            Request::Run(RunRequest {
                agent: Some("claude".into()),
                prompt: Some("go".into()),
                args: vec!["--verbose".into()],
                cwd: "/home/t/p".into(),
                policy: AgentPolicy {
                    sandbox: SandboxPolicy::Strict,
                    ..AgentPolicy::default()
                },
                request_origin: Some(RequestOrigin::RemoteControl),
                worktree: Some("issue-217".into()),
                checkpoint: true,
                ttl_ms: None,
                cols: 80,
                rows: 24,
                env: vec![("K".into(), "V".into())],
            }),
            Request::List,
            Request::PrivilegeRequest {
                verb: "install".into(),
                args: vec!["clang".into()],
                reason: "Required to compile the project".into(),
            },
            Request::Requests,
            Request::Decide {
                id: 1,
                decision: "once".into(),
            },
            Request::RequestExecuted { id: 1, exit_code: 0 },
            Request::Grants,
            Request::SystemGrants,
            Request::RevokeSystemGrant { id: 3 },
            Request::RenewSystemGrant { id: 3, ttl_ms: 900_000 },
            Request::Revoke {
                project: "/home/t/p".into(),
                key: Some("install:clang".into()),
            },
            Request::SecretUse {
                service: "github".into(),
                operation: "git.push".into(),
                resource: "origin".into(),
                params: BTreeMap::from([("branch".to_string(), "feat/x".to_string())]),
                body: None,
                project: Some("/home/t/p".into()),
            },
            Request::Info { id: 1 },
            Request::Attach {
                id: 1,
                cols: 80,
                rows: 24,
                replay: 1024,
            },
            Request::Resize {
                id: 1,
                cols: 100,
                rows: 30,
            },
            Request::Signal {
                id: 1,
                signal: "term".into(),
            },
            Request::Input {
                id: 1,
                // A transcript with a carriage return in it, because that is
                // what `--submit` appends and it is the byte most likely to be
                // mangled on the way through JSON.
                data: "run the tests\r".into(),
            },
            Request::Event {
                id: 1,
                state: Some("working".into()),
                event: Some("pre_tool_use".into()),
                detail: Some("d".into()),
                native: Some("bypassPermissions".into()),
            },
            Request::Event {
                id: 1,
                state: None,
                event: Some("task_created".into()),
                detail: None,
                native: None,
            },
            Request::ToolCheck {
                id: 1,
                tool_name: "Bash".into(),
                tool_input: serde_json::json!({"command": "ls"}),
            },
            Request::Logs { id: 1, bytes: 100 },
            Request::Remove { id: 1 },
            Request::Prune,
            Request::DeclareOrigin {
                origin: "claude-remote-control".into(),
            },
        ];

        for v in variants {
            let text = serde_json::to_string(&v)
                .unwrap_or_else(|e| panic!("{v:?} does not serialise: {e}"));
            assert!(!text.contains('\n'), "{text} would break NDJSON framing");
            let _: Request = serde_json::from_str(&text)
                .unwrap_or_else(|e| panic!("{v:?} does not round-trip: {e} from {text}"));
        }
    }

    #[test]
    fn ndjson_framing_never_embeds_a_newline() {
        // The framing is line-based, so a serialised request that contained a
        // raw newline would desynchronise the stream. serde_json escapes them;
        // this asserts that rather than assuming it.
        let req = Request::Event {
            id: 1,
            state: Some("working".into()),
            event: None,
            detail: Some("line one\nline two".into()),
            native: None,
        };
        let text = serde_json::to_string(&req).unwrap();
        assert!(!text.contains('\n'), "{text}");
        let back: Request = serde_json::from_str(&text).unwrap();
        match back {
            Request::Event { detail, .. } => {
                assert_eq!(detail.as_deref(), Some("line one\nline two"))
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn the_six_dimensions_sit_beside_sandbox_and_not_under_it() {
        // APEX Shell reads `sandbox` from the top level of a session record,
        // and so does every record already on disk. Nesting the dimensions
        // under a `policy` object would have moved that key, and the symptom
        // would have been every existing session quietly listed as `project`.
        use crate::policy::NativeMode;

        let mut s = sample_session();
        s.policy.sandbox = SandboxPolicy::Strict;
        s.policy.native = NativeMode::Bypass;
        let text = serde_json::to_string(&s).expect("serialise");
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["sandbox"], "strict", "{text}");
        assert_eq!(v["native"], "bypass", "{text}");
        assert!(v.get("policy").is_none(), "the policy must not be nested: {text}");

        let back: SessionInfo = serde_json::from_str(&text).expect("deserialise");
        assert_eq!(back.policy, s.policy);
    }

    #[test]
    fn a_session_record_written_before_the_split_still_loads() {
        // Same shape as the `paused` test: strip the keys an older daemon did
        // not write and assert the record comes back at the safe defaults,
        // never loose. A dropped record would lose a running session from the
        // Agent Center; a record that loaded with the network open would be
        // worse.
        use crate::policy::{NetworkPolicy, SecretPolicy, SystemAccess};

        let s = sample_session();
        let text = serde_json::to_string(&s).unwrap();
        let mut v: serde_json::Value = serde_json::from_str(&text).unwrap();
        {
            let obj = v.as_object_mut().unwrap();
            for key in ["native", "system", "secrets", "network", "origin"] {
                assert!(obj.remove(key).is_some(), "{key} was never written");
            }
        }
        let old: SessionInfo = serde_json::from_value(v).expect("an old record still loads");
        assert_eq!(old.policy, AgentPolicy::default());
        assert_eq!(old.policy.system, SystemAccess::None);
        assert_eq!(old.policy.secrets, SecretPolicy::Brokered);
        assert_eq!(old.policy.network, NetworkPolicy::Open);
        assert_eq!(old.policy.sandbox, SandboxPolicy::Project);
    }

    #[test]
    fn a_run_request_from_a_pre_split_client_keeps_its_sandbox() {
        // The wire form an older `apex agent run --sandbox strict` sends. It
        // must not become `project`, and it must not become unconfined.
        let req: RunRequest =
            serde_json::from_str(r#"{"cwd":"/tmp","sandbox":"strict","cols":80,"rows":24}"#)
                .expect("parse");
        assert_eq!(req.policy.sandbox, SandboxPolicy::Strict);
        assert_eq!(
            req.policy,
            AgentPolicy { sandbox: SandboxPolicy::Strict, ..AgentPolicy::default() }
        );
    }

    #[test]
    fn an_event_from_a_pre_hook_client_still_publishes_its_state() {
        // The wire form every existing `apex agent event working` sends. The
        // two new keys are additive, so this must keep parsing unchanged —
        // that is the whole reason the hook bridge is not a protocol bump.
        let req: Request =
            serde_json::from_str(r#"{"cmd":"event","id":4,"state":"working"}"#).expect("parse");
        match req {
            Request::Event { id, state, event, detail, .. } => {
                assert_eq!(id, 4);
                assert_eq!(state.as_deref(), Some("working"));
                assert_eq!(event, None);
                assert_eq!(detail, None);
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn an_event_may_record_a_lifecycle_without_claiming_a_state() {
        // §6.1 asks for task, compaction, config and worktree events. None of
        // them says what the session is doing, and inventing a state for them
        // would overwrite a truthful one.
        let req: Request =
            serde_json::from_str(r#"{"cmd":"event","id":4,"event":"task_created"}"#)
                .expect("parse");
        match req {
            Request::Event { state, event, .. } => {
                assert_eq!(state, None);
                assert_eq!(event.as_deref(), Some("task_created"));
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn every_version_guard_names_a_revision_that_exists() {
        // The CLI refuses to send a setting to a daemon older than the
        // revision that introduced it, because an old daemon drops the key
        // and runs without the restriction. Each guard therefore has to name
        // a revision at or below the current one — a guard pointing at a
        // future version would refuse every daemon, and one pointing past the
        // current version cannot be reached at all.
        for (name, since) in [
            ("the six dimensions", POLICY_DIMENSIONS_VERSION),
            ("request_origin", REQUEST_ORIGIN_VERSION),
            ("the secret service", BROKERED_SECRET_SERVICE_VERSION),
            ("generic capabilities", GENERIC_CAPABILITY_VERSION),
            ("the mcp bridge", MCP_BRIDGE_VERSION),
            ("system-access grants", SYSTEM_GRANT_VERSION),
        ] {
            assert!(
                since <= PROTOCOL_VERSION,
                "{name} claims to arrive in protocol {since}, which is ahead of {PROTOCOL_VERSION}"
            );
            assert!(since > 0, "{name} has no revision");
        }
        // The newest guard is the current revision: adding a wire field
        // without bumping the version is the fail-open these exist to catch.
        assert_eq!(GENERIC_CAPABILITY_VERSION, PROTOCOL_VERSION);
        assert_eq!(MCP_BRIDGE_VERSION, PROTOCOL_VERSION);
        assert_eq!(SYSTEM_GRANT_VERSION, PROTOCOL_VERSION);
    }

    #[test]
    fn session_info_reports_liveness_and_exit() {
        let mut info = SessionInfo {
            id: 1,
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
            policy: AgentPolicy::default(),
            request_origin: None,
            origin_source: None,
            grant: None,
            grant_expires_ms: None,
            native_observed: None,
            pid: 123,
            started: 0,
            last_activity: 0,
            exit_code: None,
            exit_signal: None,
            attached: 0,
            checkpoint: None,
            cols: 80,
            rows: 24,
        };
        assert!(info.is_live());
        assert_eq!(info.exit_summary(), None);

        info.exit_code = Some(2);
        assert!(!info.is_live());
        assert_eq!(info.exit_summary().as_deref(), Some("exited 2"));

        info.exit_code = None;
        info.exit_signal = Some(9);
        assert!(!info.is_live());
        assert_eq!(info.exit_summary().as_deref(), Some("killed by signal 9"));
    }
}
