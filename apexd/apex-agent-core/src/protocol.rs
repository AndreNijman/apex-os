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

/// Protocol revision. Bumped when a change is not backward compatible; the
/// daemon reports it in [`Response::Hello`] so a mismatched CLI can say so
/// plainly instead of failing on a missing field.
pub const PROTOCOL_VERSION: u32 = 1;

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

/// How much of the machine a session may reach. See `sandbox.rs` for what each
/// one actually builds.
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
    pub sandbox: SandboxPolicy,
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
    #[serde(default)]
    pub sandbox: SandboxPolicy,
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
        assert_eq!(req.sandbox, SandboxPolicy::Project);
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
            sandbox: SandboxPolicy::Project,
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
    fn every_request_variant_round_trips() {
        let variants = vec![
            Request::Hello,
            Request::Run(RunRequest {
                agent: Some("claude".into()),
                prompt: Some("go".into()),
                args: vec!["--verbose".into()],
                cwd: "/home/t/p".into(),
                sandbox: SandboxPolicy::Strict,
                worktree: Some("issue-217".into()),
                checkpoint: true,
                cols: 80,
                rows: 24,
                env: vec![("K".into(), "V".into())],
            }),
            Request::List,
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
            sandbox: SandboxPolicy::Project,
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
