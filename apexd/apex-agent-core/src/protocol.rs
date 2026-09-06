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
pub const PROTOCOL_VERSION: u32 = 4;

/// The revision at which the credential store moved to `apex-secretd`.
///
/// Named for the same reason the two below it are: the check is a boundary,
/// and a bare `< 4` in the CLI is one careless edit away from meaning nothing.
pub const BROKERED_SECRET_SERVICE_VERSION: u32 = 4;

/// The guards arrive in order, checked when the crate compiles rather than
/// when a test runs: they are facts about three constants, and a revision
/// numbered behind the one before it would make a `<` comparison in the CLI
/// mean something nobody intended.
const _: () = assert!(POLICY_DIMENSIONS_VERSION < REQUEST_ORIGIN_VERSION);
const _: () = assert!(REQUEST_ORIGIN_VERSION < BROKERED_SECRET_SERVICE_VERSION);

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
    /// Deliver a signal by name (`int`, `term`, `kill`, `stop`, `cont`).
    Signal { id: u32, signal: String },
    /// Publish a state transition. This is the open event protocol: any client
    /// that knows its session id can report what it is doing.
    Event {
        id: u32,
        state: String,
        #[serde(default)]
        detail: Option<String>,
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

    // ── the secret broker (§4) ──────────────────────────────────────────────
    //
    // Note what `SecretUse` does NOT carry: a session id, and a remote URL.
    // The session comes from the connection's peer credentials, and the remote
    // is a NAME the daemon resolves against the repository — a URL would let a
    // session choose where its token gets sent.
    /// Ask the broker to perform a capability. The token never comes back.
    SecretUse {
        service: String,
        capability: String,
        remote: String,
        #[serde(default)]
        branch: Option<String>,
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
            Request::Revoke {
                project: "/home/t/p".into(),
                key: Some("install:clang".into()),
            },
            Request::SecretUse {
                service: "github".into(),
                capability: "git-push".into(),
                remote: "origin".into(),
                branch: Some("feat/x".into()),
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
            Request::Event {
                id: 1,
                state: "working".into(),
                detail: Some("d".into()),
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
            state: "working".into(),
            detail: Some("line one\nline two".into()),
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
        ] {
            assert!(
                since <= PROTOCOL_VERSION,
                "{name} claims to arrive in protocol {since}, which is ahead of {PROTOCOL_VERSION}"
            );
            assert!(since > 0, "{name} has no revision");
        }
        // The newest guard is the current revision: adding a wire field
        // without bumping the version is the fail-open these exist to catch.
        assert_eq!(BROKERED_SECRET_SERVICE_VERSION, PROTOCOL_VERSION);
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
