//! Claude's own hook lifecycle, as a source of session state (§6.1) and as a
//! policy point (§6.2).
//!
//! ## Why this exists beside the PTY scanner
//!
//! [`crate::session`] infers state from bytes: a bell, an OSC notification,
//! silence past [`crate::session::IDLE_TO_WAITING_SECS`]. That is the right
//! fallback for an agent nobody has integrated, and §6.1 keeps it for exactly
//! that reason. It is also, for Claude specifically, a guess where an answer
//! exists — Claude publishes a structured event at every transition the Agent
//! Center cares about, and a `PreToolUse` that says "Bash, `cargo test`" beats
//! ten seconds of silence that the idle rule reads as the user being asked a
//! question.
//!
//! The measurable difference is not that hooks are tidier. It is:
//!
//! * a quiet tool call. `cargo test` prints nothing for two minutes, so the
//!   idle rule reports `waiting_for_user` after ten seconds and is wrong for
//!   the remaining hundred and ten. A `PreToolUse` with no matching
//!   `PostToolUse` yet says a tool is running, and [`crate::session::next_state`]
//!   holds `working` through the silence.
//! * the end of a turn. `Stop` fires when Claude finishes; the idle rule needs
//!   [`crate::session::IDLE_TO_WAITING_SECS`] of quiet before it agrees.
//!
//! ## A hook is advisory, and this module never pretends otherwise
//!
//! Every hook here runs inside the agent's own process tree, from a settings
//! file the agent can read. `--bare` skips hooks outright, a later
//! `--settings` in the user's own arguments replaces ours, and `~/.claude` is
//! writable to a managed session. So a hook can be silenced, and the only
//! honest response is to make silence cost nothing: state falls back to the
//! PTY scanner, and [`decide`] is a second opinion on top of a kernel sandbox
//! that was already going to refuse the operation. See [`decide`] for the rule
//! that keeps that true.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::protocol::AgentState;

/// How long a hook command may take before Claude ignores it.
///
/// Claude's own default is 600 seconds, which is a sane ceiling for a hook
/// that runs a linter and a terrible one for a hook that writes a line to a
/// Unix socket: the hook runs before *every* tool call, so a daemon that is
/// wedged or gone would stall the agent for ten minutes per call rather than
/// getting out of the way. Three seconds is far above the round trip to a
/// local socket and far below anything a person would sit through.
pub const HOOK_TIMEOUT_SECS: u32 = 3;

/// The file a managed Claude session is pointed at with `--settings`.
///
/// It lives in the session's scratch directory, which is bound into the
/// sandbox at the same path, so the name Claude is given resolves to the same
/// file inside and out.
pub const SETTINGS_FILE: &str = "claude-hooks.json";

/// The events APEX subscribes to.
///
/// A subset of what Claude emits, and deliberately not all of it. Each variant
/// below either moves the session state or is a policy point; subscribing to
/// an event APEX does nothing with would add a process spawn per occurrence to
/// buy nothing. §6.1's list of "compaction, config changes, worktree events,
/// file changes" is genuinely useful to the audit graph of §15 and is not
/// state, so those arrive here as [`HookEvent::Note`] rather than as their own
/// variants — one wire name each, no state transition, and §15 can read them
/// out of the record without this enum growing a variant per upstream feature.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HookEvent {
    /// A session began. Claude's own id arrives with it.
    SessionStart,
    /// The user sent a prompt, so the agent is working by definition.
    UserPromptSubmit,
    /// A tool is about to run. Also the policy point of §6.2.
    PreToolUse,
    /// A tool finished, successfully.
    PostToolUse,
    /// A tool finished, unsuccessfully.
    PostToolUseFailure,
    /// Claude is asking a human for a permission decision.
    PermissionRequest,
    /// A subagent started, and a subagent is the agent still working.
    SubagentStart,
    /// A subagent finished.
    SubagentStop,
    /// A task was created in the agent's own task list.
    TaskCreated,
    /// A task completed.
    TaskCompleted,
    /// A desktop notification, which for Claude is nearly always "your input
    /// is needed".
    Notification,
    /// The turn ended.
    Stop,
    /// The session ended.
    SessionEnd,
    /// Something worth recording that is not a state change: compaction,
    /// a configuration change, a worktree appearing, a watched file changing.
    Note,
}

impl HookEvent {
    /// Every event APEX subscribes to, in the order the settings file lists
    /// them.
    pub const ALL: &'static [HookEvent] = &[
        HookEvent::SessionStart,
        HookEvent::UserPromptSubmit,
        HookEvent::PreToolUse,
        HookEvent::PostToolUse,
        HookEvent::PostToolUseFailure,
        HookEvent::PermissionRequest,
        HookEvent::SubagentStart,
        HookEvent::SubagentStop,
        HookEvent::TaskCreated,
        HookEvent::TaskCompleted,
        HookEvent::Notification,
        HookEvent::Stop,
        HookEvent::SessionEnd,
        HookEvent::Note,
    ];

    /// The name APEX uses on its own wire and in `apex agent hook <event>`.
    ///
    /// Snake case, because it sits beside `waiting_for_user` in the same
    /// protocol, and distinct from Claude's camel-case spelling so that a
    /// change upstream is a change in one table rather than everywhere.
    pub fn as_str(&self) -> &'static str {
        match self {
            HookEvent::SessionStart => "session_start",
            HookEvent::UserPromptSubmit => "user_prompt_submit",
            HookEvent::PreToolUse => "pre_tool_use",
            HookEvent::PostToolUse => "post_tool_use",
            HookEvent::PostToolUseFailure => "post_tool_use_failure",
            HookEvent::PermissionRequest => "permission_request",
            HookEvent::SubagentStart => "subagent_start",
            HookEvent::SubagentStop => "subagent_stop",
            HookEvent::TaskCreated => "task_created",
            HookEvent::TaskCompleted => "task_completed",
            HookEvent::Notification => "notification",
            HookEvent::Stop => "stop",
            HookEvent::SessionEnd => "session_end",
            HookEvent::Note => "note",
        }
    }

    /// Parse an APEX event name. Unknown names are rejected rather than mapped
    /// to a default, for the same reason [`AgentState::parse`] rejects them: a
    /// typo that silently became "note" would report nothing and say nothing.
    pub fn parse(s: &str) -> Option<HookEvent> {
        HookEvent::ALL.iter().copied().find(|e| e.as_str() == s)
    }

    /// The Claude event names that map onto this one.
    ///
    /// More than one for [`HookEvent::Note`], which is the catch-all, and for
    /// nothing else. These are the strings that go into the settings file, and
    /// they are Claude's spelling, not APEX's.
    pub fn claude_names(&self) -> &'static [&'static str] {
        match self {
            HookEvent::SessionStart => &["SessionStart"],
            HookEvent::UserPromptSubmit => &["UserPromptSubmit"],
            HookEvent::PreToolUse => &["PreToolUse"],
            HookEvent::PostToolUse => &["PostToolUse"],
            HookEvent::PostToolUseFailure => &["PostToolUseFailure"],
            HookEvent::PermissionRequest => &["PermissionRequest"],
            HookEvent::SubagentStart => &["SubagentStart"],
            HookEvent::SubagentStop => &["SubagentStop"],
            HookEvent::TaskCreated => &["TaskCreated"],
            HookEvent::TaskCompleted => &["TaskCompleted"],
            HookEvent::Notification => &["Notification"],
            HookEvent::Stop => &["Stop"],
            HookEvent::SessionEnd => &["SessionEnd"],
            // §6.1 asks for compaction, config changes, worktree events and
            // file changes. None of them is a state, all of them belong in the
            // audit graph, so they share one subscription.
            HookEvent::Note => &[
                "PreCompact",
                "PostCompact",
                "ConfigChange",
                "WorktreeCreate",
                "WorktreeRemove",
                "FileChanged",
            ],
        }
    }

    /// Whether this event is the policy point, and therefore has to be able to
    /// answer on stdout rather than merely reporting.
    pub fn is_policy_point(&self) -> bool {
        matches!(self, HookEvent::PreToolUse)
    }

    /// What this event says a tool is doing, which is what lets
    /// [`crate::session::next_state`] hold `working` through a silent tool
    /// call instead of calling it `waiting_for_user` after ten seconds.
    pub fn tool_transition(&self) -> ToolTransition {
        match self {
            HookEvent::PreToolUse => ToolTransition::Started,
            HookEvent::PostToolUse
            | HookEvent::PostToolUseFailure
            // A turn that ended has no tool in flight, whatever the last
            // PreToolUse said. This is the recovery path for a PostToolUse
            // that never arrived.
            | HookEvent::Stop
            | HookEvent::SessionEnd
            | HookEvent::PermissionRequest => ToolTransition::Finished,
            _ => ToolTransition::Unchanged,
        }
    }
}

impl std::fmt::Display for HookEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What an event does to the "a tool is running" flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolTransition {
    Started,
    Finished,
    Unchanged,
}

/// The JSON Claude writes to a hook command's stdin.
///
/// Every field is optional and defaults, which is not laziness: this struct
/// deserialises a document produced by a program that ships far more often
/// than APEX does. A renamed field must cost the bridge some detail in the
/// Agent Center, never a failed parse that takes the whole state feed with it.
/// The one field APEX genuinely needs — which session — does not come from
/// here at all; it comes from `$APEX_AGENT_SESSION`, set by the daemon outside
/// anything Claude can write.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Payload {
    #[serde(default)]
    pub hook_event_name: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub permission_mode: Option<String>,
    /// Present on the tool events. `Bash`, `Edit`, `WebFetch`, `mcp__…`.
    #[serde(default)]
    pub tool_name: Option<String>,
    /// The tool's arguments, shaped differently per tool. Kept as raw JSON
    /// because [`decide`] reads three keys out of it and inventing a typed
    /// union of every tool's input would break on the next tool Claude adds.
    #[serde(default)]
    pub tool_input: serde_json::Value,
    /// Set on subagent and task events.
    #[serde(default)]
    pub agent_type: Option<String>,
    #[serde(default)]
    pub agent_id: Option<String>,
    /// `Notification` carries its text here.
    #[serde(default)]
    pub message: Option<String>,
    /// `SessionStart` carries `startup` | `resume` | `clear` | `compact` |
    /// `fork`.
    #[serde(default)]
    pub source: Option<String>,
}

/// What one hook run means to the daemon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Observation {
    pub event: HookEvent,
    /// The state to publish, when this event implies one.
    ///
    /// `None` for the events that record something without changing what the
    /// session is doing — a created task, a compaction, a config change.
    pub state: Option<AgentState>,
    /// One line for the Agent Center, already trimmed and bounded.
    pub detail: Option<String>,
    pub tool: ToolTransition,
}

/// Longest detail line a hook may publish.
///
/// A `tool_input` can hold a whole file, and the detail ends up in a session
/// record that is rewritten on every change. Truncated rather than dropped:
/// the first eighty characters of a command are what identifies it.
const MAX_DETAIL: usize = 120;

/// Read one hook payload as a state observation.
///
/// Pure, so the whole mapping is asserted in this file rather than through a
/// live agent. Note what it does *not* return: `Complete` or `Failed`.
/// `SessionEnd` means Claude is shutting down, not that it succeeded, and the
/// process's exit status — which [`crate::session::exit_state`] already reads
/// — is the only thing that knows which. A hook that published `complete`
/// would race the real answer and sometimes win.
pub fn observe(event: HookEvent, payload: &Payload) -> Observation {
    let state = match event {
        HookEvent::SessionStart => Some(AgentState::Starting),
        HookEvent::UserPromptSubmit
        | HookEvent::PreToolUse
        | HookEvent::PostToolUse
        | HookEvent::PostToolUseFailure
        | HookEvent::SubagentStart
        | HookEvent::SubagentStop => Some(AgentState::Working),
        // The one state the PTY scanner refuses to guess, because there is no
        // way to recognise a permission prompt in arbitrary terminal output.
        // A hook knows.
        HookEvent::PermissionRequest => Some(AgentState::PermissionRequest),
        // Claude's notifications are "Claude needs your permission" and
        // "Claude is waiting for your input". Both are the user's turn.
        HookEvent::Notification | HookEvent::Stop => Some(AgentState::WaitingForUser),
        // The process is on its way out; its exit status decides the outcome.
        HookEvent::SessionEnd => None,
        HookEvent::TaskCreated | HookEvent::TaskCompleted | HookEvent::Note => None,
    };

    Observation {
        event,
        state,
        detail: detail_for(event, payload),
        tool: event.tool_transition(),
    }
}

/// The one line the Agent Center shows beside the state.
fn detail_for(event: HookEvent, payload: &Payload) -> Option<String> {
    let text = match event {
        HookEvent::PreToolUse | HookEvent::PostToolUse | HookEvent::PostToolUseFailure => {
            let tool = payload.tool_name.as_deref()?;
            match tool_summary(tool, &payload.tool_input) {
                Some(arg) => format!("{tool}: {arg}"),
                None => tool.to_string(),
            }
        }
        HookEvent::SubagentStart | HookEvent::SubagentStop => {
            let kind = payload.agent_type.as_deref().unwrap_or("subagent");
            match event {
                HookEvent::SubagentStart => format!("{kind} started"),
                _ => format!("{kind} finished"),
            }
        }
        HookEvent::Notification | HookEvent::PermissionRequest => {
            payload.message.clone().or_else(|| {
                payload
                    .tool_name
                    .as_ref()
                    .map(|t| format!("{t} needs a decision"))
            })?
        }
        HookEvent::SessionStart => payload.source.clone()?,
        _ => return None,
    };

    Some(clamp(&text))
}

/// The argument that identifies a tool call, per tool.
///
/// Hand-written per tool rather than "the first string in the object": a map's
/// iteration order is not the argument order, so the generic version showed
/// `Edit`'s `new_string` about as often as its `file_path`.
fn tool_summary(tool: &str, input: &serde_json::Value) -> Option<String> {
    let key = match tool {
        "Bash" | "BashOutput" => "command",
        "Read" | "Write" | "Edit" | "NotebookEdit" => "file_path",
        "Glob" | "Grep" => "pattern",
        "WebFetch" => "url",
        "WebSearch" => "query",
        "Task" => "description",
        _ => return None,
    };
    let text = input.get(key)?.as_str()?.trim();
    if text.is_empty() {
        return None;
    }
    Some(text.to_string())
}

/// Bound a line of agent-supplied text to something a record can hold.
fn clamp(text: &str) -> String {
    // Newlines first: a heredoc in a Bash command would otherwise put its
    // second line into a field the Agent Center renders as one.
    let flat: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let flat = flat.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= MAX_DETAIL {
        return flat;
    }
    let cut: String = flat.chars().take(MAX_DETAIL - 1).collect();
    format!("{cut}…")
}

/// Build the settings document a managed Claude session is started with.
///
/// `apex` is the absolute path to this runtime's CLI, resolved by the daemon:
/// `PATH` inside the sandbox is the daemon's, and a development build living
/// somewhere `PATH` does not name would produce a settings file whose every
/// hook fails to exec. Failing that way is survivable — see the module docs —
/// but it is not something to arrange on purpose.
///
/// One subscription per event, each running `apex agent hook <event>`. The
/// hook command takes no session id: `$APEX_AGENT_SESSION` is in the
/// environment bwrap set, and an id on the command line would be an id the
/// agent could edit.
pub fn settings_json(apex: &Path) -> serde_json::Value {
    let mut hooks = serde_json::Map::new();

    for event in HookEvent::ALL {
        let entry = serde_json::json!({
            "hooks": [{
                "type": "command",
                "command": format!("{} agent hook {}", shell_quote(apex), event.as_str()),
                "timeout": HOOK_TIMEOUT_SECS,
            }]
        });
        for name in event.claude_names() {
            hooks
                .entry(name.to_string())
                .or_insert_with(|| serde_json::Value::Array(Vec::new()))
                .as_array_mut()
                .expect("just inserted an array")
                .push(entry.clone());
        }
    }

    serde_json::json!({ "hooks": hooks })
}

/// Quote a path for the shell Claude runs a `command` hook through.
///
/// Single quotes, with the one escape that matters. A path is not usually
/// hostile here — it is the daemon's own binary — but `resolve_program` walks
/// `PATH`, and a directory with a space in it would otherwise turn the hook
/// into a command with an argument.
fn shell_quote(path: &Path) -> String {
    let text = path.to_string_lossy();
    if text
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "/._-+=:@".contains(c))
    {
        return text.into_owned();
    }
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// Where the settings file for a session's scratch directory goes.
pub fn settings_path(scratch: &Path) -> PathBuf {
    scratch.join(SETTINGS_FILE)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload(json: serde_json::Value) -> Payload {
        serde_json::from_value(json).expect("payload")
    }

    #[test]
    fn every_event_name_round_trips_and_is_unique() {
        let mut names: Vec<&str> = HookEvent::ALL.iter().map(|e| e.as_str()).collect();
        let count = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), count, "duplicate event name");

        for e in HookEvent::ALL {
            assert_eq!(HookEvent::parse(e.as_str()), Some(*e));
        }
        assert_eq!(HookEvent::parse("PreToolUse"), None, "claude's spelling");
        assert_eq!(HookEvent::parse(""), None);
    }

    #[test]
    fn no_claude_event_is_subscribed_twice() {
        // Two subscriptions to one upstream event means two processes spawned
        // per occurrence and two state publications racing each other.
        let mut all: Vec<&str> = HookEvent::ALL
            .iter()
            .flat_map(|e| e.claude_names().iter().copied())
            .collect();
        let count = all.len();
        all.sort_unstable();
        all.dedup();
        assert_eq!(all.len(), count, "an upstream event is subscribed twice");
    }

    #[test]
    fn the_six_lifecycle_areas_of_6_1_all_have_an_event() {
        // Session, tool, task, subagent, notification and stop — the six the
        // acceptance criterion names. A refactor that dropped one would
        // otherwise only show up in a live session.
        for e in [
            HookEvent::SessionStart,
            HookEvent::SessionEnd,
            HookEvent::PreToolUse,
            HookEvent::PostToolUse,
            HookEvent::TaskCreated,
            HookEvent::TaskCompleted,
            HookEvent::SubagentStart,
            HookEvent::SubagentStop,
            HookEvent::Notification,
            HookEvent::Stop,
        ] {
            assert!(HookEvent::ALL.contains(&e), "{e} is not subscribed");
            assert!(!e.claude_names().is_empty(), "{e} has no upstream name");
        }
    }

    #[test]
    fn a_permission_request_is_reported_because_a_hook_knows_and_a_scanner_cannot() {
        // session.rs refuses to guess this state from output. The hook is the
        // only thing that can set it, so this mapping is the whole reason
        // permission_request is reachable at all.
        let o = observe(
            HookEvent::PermissionRequest,
            &payload(serde_json::json!({"tool_name": "Bash"})),
        );
        assert_eq!(o.state, Some(AgentState::PermissionRequest));
        assert_eq!(o.detail.as_deref(), Some("Bash needs a decision"));
    }

    #[test]
    fn session_end_does_not_publish_an_outcome() {
        // Only the exit status knows whether a session succeeded. A hook that
        // published `complete` would race it and sometimes win.
        let o = observe(HookEvent::SessionEnd, &Payload::default());
        assert_eq!(o.state, None);
        for e in HookEvent::ALL {
            let state = observe(*e, &Payload::default()).state;
            assert!(
                !matches!(
                    state,
                    Some(AgentState::Complete) | Some(AgentState::Failed) | Some(AgentState::Exited)
                ),
                "{e} published a terminal state"
            );
        }
    }

    #[test]
    fn a_tool_call_brackets_the_in_flight_flag() {
        assert_eq!(
            HookEvent::PreToolUse.tool_transition(),
            ToolTransition::Started
        );
        for e in [HookEvent::PostToolUse, HookEvent::PostToolUseFailure] {
            assert_eq!(e.tool_transition(), ToolTransition::Finished);
        }
        // And the recovery path: a turn that ended has no tool running, even
        // if the PostToolUse that should have said so never arrived.
        for e in [
            HookEvent::Stop,
            HookEvent::SessionEnd,
            HookEvent::PermissionRequest,
        ] {
            assert_eq!(e.tool_transition(), ToolTransition::Finished, "{e}");
        }
        assert_eq!(
            HookEvent::Notification.tool_transition(),
            ToolTransition::Unchanged
        );
    }

    #[test]
    fn stop_is_the_users_turn_and_a_prompt_is_the_agents() {
        assert_eq!(
            observe(HookEvent::Stop, &Payload::default()).state,
            Some(AgentState::WaitingForUser)
        );
        assert_eq!(
            observe(HookEvent::UserPromptSubmit, &Payload::default()).state,
            Some(AgentState::Working)
        );
    }

    #[test]
    fn a_tool_detail_names_the_argument_that_identifies_the_call() {
        let o = observe(
            HookEvent::PreToolUse,
            &payload(serde_json::json!({
                "tool_name": "Bash",
                "tool_input": {"command": "cargo test --workspace", "description": "run tests"}
            })),
        );
        assert_eq!(o.detail.as_deref(), Some("Bash: cargo test --workspace"));

        let o = observe(
            HookEvent::PreToolUse,
            &payload(serde_json::json!({
                "tool_name": "Edit",
                "tool_input": {"file_path": "/p/src/main.rs", "new_string": "fn main() {}"}
            })),
        );
        assert_eq!(o.detail.as_deref(), Some("Edit: /p/src/main.rs"));

        // An unknown tool still names itself.
        let o = observe(
            HookEvent::PreToolUse,
            &payload(serde_json::json!({"tool_name": "mcp__memory__search"})),
        );
        assert_eq!(o.detail.as_deref(), Some("mcp__memory__search"));
    }

    #[test]
    fn a_detail_is_flattened_and_bounded() {
        let long = "x".repeat(500);
        let o = observe(
            HookEvent::PreToolUse,
            &payload(serde_json::json!({
                "tool_name": "Bash",
                "tool_input": {"command": format!("echo 'line one\nline two' {long}")}
            })),
        );
        let detail = o.detail.expect("detail");
        assert!(detail.chars().count() <= MAX_DETAIL, "{}", detail.len());
        assert!(!detail.contains('\n'), "a record field holds one line");
        assert!(detail.ends_with('…'));
    }

    #[test]
    fn an_unknown_upstream_field_does_not_break_the_parse() {
        // Claude ships far more often than APEX. A renamed or added field must
        // cost detail, never the whole state feed.
        let p: Payload = serde_json::from_str(
            r#"{"hook_event_name":"PreToolUse","tool_name":"Bash",
                "something_new_upstream":{"nested":[1,2,3]}}"#,
        )
        .expect("unknown fields are ignored");
        assert_eq!(p.tool_name.as_deref(), Some("Bash"));

        // And an entirely empty document is a payload with no detail, not an
        // error that loses the event.
        let p: Payload = serde_json::from_str("{}").expect("empty payload");
        assert!(p.tool_name.is_none());
    }

    #[test]
    fn the_settings_document_subscribes_every_event_with_a_short_timeout() {
        let v = settings_json(Path::new("/usr/bin/apex"));
        let hooks = v["hooks"].as_object().expect("hooks object");

        for event in HookEvent::ALL {
            for name in event.claude_names() {
                let entries = hooks[*name].as_array().expect(name);
                assert_eq!(entries.len(), 1, "{name}");
                let hook = &entries[0]["hooks"][0];
                assert_eq!(hook["type"], "command");
                assert_eq!(
                    hook["command"],
                    format!("/usr/bin/apex agent hook {}", event.as_str())
                );
                // The default is 600 seconds and this runs before every tool
                // call. A wedged daemon must not stall the agent.
                assert_eq!(hook["timeout"], HOOK_TIMEOUT_SECS);
            }
        }
    }

    #[test]
    fn the_settings_document_never_carries_a_session_id() {
        // The id comes from $APEX_AGENT_SESSION, which bwrap set. On the
        // command line it would be a number the agent could edit into another
        // session's.
        let text = settings_json(Path::new("/usr/bin/apex")).to_string();
        assert!(!text.contains("--session"), "{text}");
        assert!(!text.contains("APEX_AGENT_SESSION"), "{text}");
    }

    #[test]
    fn a_path_with_a_space_is_quoted_for_the_shell() {
        let v = settings_json(Path::new("/opt/my apps/apex"));
        let cmd = v["hooks"]["Stop"][0]["hooks"][0]["command"]
            .as_str()
            .expect("command");
        assert_eq!(cmd, "'/opt/my apps/apex' agent hook stop");
        // The ordinary case stays unquoted and readable.
        let v = settings_json(Path::new("/usr/bin/apex"));
        assert_eq!(
            v["hooks"]["Stop"][0]["hooks"][0]["command"],
            "/usr/bin/apex agent hook stop"
        );
    }

    #[test]
    fn the_settings_file_lands_in_the_scratch_directory() {
        assert_eq!(
            settings_path(Path::new("/run/user/1000/apex/s7")),
            PathBuf::from("/run/user/1000/apex/s7/claude-hooks.json")
        );
    }
}
