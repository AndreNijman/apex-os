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

use crate::destination::{Allowlist, Destination};
use crate::policy::{AgentPolicy, NetworkPolicy};
use crate::protocol::AgentState;
use crate::sandbox::SandboxSpec;

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
    /// The agent's own permission mode, as the agent reported it (§4.1).
    ///
    /// §4.1's third criterion is that the agent-native mode is visible in the
    /// Agent Center, and until this existed there was nothing to show:
    /// `policy.native` is `inherit` for the default case, which describes what
    /// APEX did — pass no flag — rather than what the agent is doing.
    ///
    /// Bounded and stripped for the same reason `detail` is: it lands in a
    /// session record that the shell renders, and it comes off a document the
    /// agent writes.
    pub native: Option<String>,
    /// Which subagent this event is about, and what kind (§P1-020).
    ///
    /// `Some` only on the two subagent events, and only for the fields the
    /// payload actually carried. The daemon builds the session graph out of
    /// these; up to P1-020 they were read here for the detail line and then
    /// dropped, so no subagent was recorded anywhere.
    pub agent_id: Option<String>,
    pub agent_type: Option<String>,
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

    // The two subagent fields are carried ONLY for the two subagent events.
    // `agent_type` is also set on Claude's task events, and letting it through
    // there would have the daemon open a graph node for a to-do item.
    let is_subagent = matches!(
        event,
        HookEvent::SubagentStart | HookEvent::SubagentStop
    );

    Observation {
        event,
        state,
        detail: detail_for(event, payload),
        tool: event.tool_transition(),
        native: native_mode(payload),
        agent_id: if is_subagent {
            payload.agent_id.clone()
        } else {
            None
        },
        agent_type: if is_subagent {
            payload.agent_type.clone()
        } else {
            None
        },
    }
}

/// Longest permission-mode name kept.
///
/// Claude's are `default`, `acceptEdits`, `plan` and `bypassPermissions`. The
/// bound is not about those — it is about the field being read off a document
/// the agent writes, into a record the shell renders in a fixed-width column.
const MAX_NATIVE: usize = 32;

/// The agent's own permission mode, as it reported it.
///
/// Passed through rather than mapped onto [`crate::policy::NativeMode`], and
/// that is the point of the field. §4.1 says APEX passes no permission flag
/// and lets the agent's profile decide, so the interesting value is precisely
/// the one APEX has no vocabulary for: `bypassPermissions`, `acceptEdits`,
/// `plan`. Folding those three into "not ask" would answer the question
/// criterion 3 asks — what mode is this agent in — with a summary of what
/// APEX did about it, which is the thing the user can already see.
///
/// Bounded and stripped of anything that is not a plain identifier, because it
/// is rendered in a table by a client that trusts the record.
fn native_mode(payload: &Payload) -> Option<String> {
    let raw = payload.permission_mode.as_deref()?.trim();
    if raw.is_empty() || raw.chars().count() > MAX_NATIVE {
        return None;
    }
    if !raw
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return None;
    }
    Some(raw.to_string())
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
        // `NotebookEdit` spells it differently, which is the whole reason this
        // table is hand-written: a generic "first string in the object" showed
        // `new_source` for it and `new_string` for `Edit`.
        "NotebookEdit" => "notebook_path",
        "Read" | "Write" | "Edit" => "file_path",
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

// ---------------------------------------------------------------------------
// §6.2 — the policy point
// ---------------------------------------------------------------------------

/// Which kernel restriction refuses the operation a [`Decision::Deny`] names.
///
/// Every deny carries one. That is the rule §6.2 turns on: a hook runs inside
/// the agent's own process tree, from a settings file the agent's home mount
/// lets it rewrite, so a denial the sandbox would not also have refused is
/// advice the agent can switch off — theatre with a policy engine behind it.
/// Naming the refusal makes the claim checkable, and
/// `every_denial_names_a_restriction_the_argv_actually_carries` checks it
/// against the argv `build_argv` really produces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KernelRefusal {
    /// `--ro-bind / /` with no writable bind over this path: the write returns
    /// `EROFS`.
    ReadOnlyRoot,
    /// `--ro-bind-try /dev/null <path>`: the write returns `EROFS` and the read
    /// returns an empty file, so the credential does not reach the agent
    /// either way.
    MaskedFile,
    /// `--unshare-net` with nothing in the namespace: `connect` returns
    /// `ENETUNREACH`.
    EmptyNetNamespace,
    /// The namespace's only route out is the egress proxy, and the broker on
    /// the far end of its socket refuses this destination.
    Allowlist,
    /// `PR_SET_NO_NEW_PRIVS`, which `bwrap` sets for every confined session and
    /// which the runtime sets itself for an unconfined one: the setuid bit on
    /// `sudo` is inert, so it cannot become root however it is invoked.
    NoNewPrivs,
}

impl KernelRefusal {
    /// The clause that goes in the reason Claude shows the agent.
    fn as_clause(&self) -> &'static str {
        match self {
            KernelRefusal::ReadOnlyRoot => "the sandbox mounts it read-only",
            KernelRefusal::MaskedFile => "the sandbox replaces it with /dev/null",
            KernelRefusal::EmptyNetNamespace => "this session has no network namespace",
            KernelRefusal::Allowlist => "this destination is not on the session's allowlist",
            KernelRefusal::NoNewPrivs => "no_new_privs makes the setuid bit inert",
        }
    }
}

/// What the `PreToolUse` policy point says about one tool call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Say nothing. Claude proceeds under whatever its own permission mode
    /// decided, which in Andre's normal `bypassPermissions` is "yes".
    Allow,
    /// Refuse, with a reason and the restriction that would have refused it
    /// anyway.
    Deny {
        reason: String,
        refusal: KernelRefusal,
    },
}

impl Decision {
    pub fn is_deny(&self) -> bool {
        matches!(self, Decision::Deny { .. })
    }

    /// The document Claude reads from a `PreToolUse` hook's stdout.
    ///
    /// `None` for an allow: a hook that says nothing leaves the decision where
    /// it was, and printing `"allow"` would do something quite different —
    /// it would *skip* the permission system for that call. APEX has no
    /// business granting a permission; it is here to withhold one.
    pub fn to_hook_output(&self) -> Option<serde_json::Value> {
        let Decision::Deny { reason, .. } = self else {
            return None;
        };
        Some(deny_document(reason))
    }
}

/// The `PreToolUse` refusal document, for a caller that has the reason and not
/// the [`Decision`] it came from.
///
/// The CLI is that caller: the decision is made in the daemon, where the
/// mounts are, and what crosses the socket is the sentence a person reads.
pub fn deny_document(reason: &str) -> serde_json::Value {
    serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "deny",
            "permissionDecisionReason": reason,
        }
    })
}

/// Decide one `PreToolUse`, as a pure function of the tool call and the
/// confinement the session was actually started with.
///
/// ## The rule, and why it is the only defensible one
///
/// A deny is returned only where `spec` already refuses the operation. The
/// hook therefore buys three things and never a fourth:
///
/// * the agent is told *why*, in words, instead of getting `EROFS` in the
///   middle of a tool call and inventing a workaround;
/// * the call does not run at all, so a `Write` that would have half-succeeded
///   does not;
/// * the daemon records the attempt.
///
/// The fourth thing — stopping something the sandbox permits — is exactly what
/// this must not do, because the agent can rewrite the settings file this hook
/// is configured from and there would be nothing underneath.
///
/// ## What it therefore cannot see
///
/// Anything past the first word of a `Bash` command. Parsing a shell well
/// enough to find the writes inside it is a losing game, and a rule that
/// caught `rm -rf /usr` but not `sh -c 'rm -rf /usr'` would read as protection
/// while providing none. The sandbox catches both, which is why this one does
/// not try.
///
/// An `mcp__*` tool, whose input shape belongs to the server rather than to
/// Claude. And every tool call in a session started with the sandbox off,
/// where there is no restriction to mirror — see the first branch.
pub fn decide(payload: &Payload, spec: &SandboxSpec, allowlist: &Allowlist) -> Decision {
    // A session the user deliberately started unconfined has nothing behind
    // this hook, so a denial here would be the pure theatre §6.2 is trying to
    // avoid: the one layer that could enforce it is the one the agent can
    // switch off. The exception is no_new_privs, which the runtime sets on the
    // process itself and which therefore holds without any mount at all.
    let confined = spec.policy.sandbox.is_confined();

    let Some(tool) = payload.tool_name.as_deref() else {
        return Decision::Allow;
    };
    let input = &payload.tool_input;

    if tool == "Bash" {
        if let Some(refusal) = bash_refusal(&spec.policy, input) {
            return deny(
                format!(
                    "APEX policy: this session may not run a privileged command — {}",
                    refusal.as_clause()
                ),
                refusal,
            );
        }
        // Everything else a shell can do is the sandbox's problem, on purpose.
        return Decision::Allow;
    }

    if matches!(tool, "WebFetch" | "WebSearch") {
        if let Some((refusal, detail)) = network_refusal(tool, &spec.policy, allowlist, input) {
            return deny(
                format!("APEX policy: {detail} — {}", refusal.as_clause()),
                refusal,
            );
        }
        return Decision::Allow;
    }

    if !confined {
        return Decision::Allow;
    }

    let Some((path, writes)) = file_operation(tool, input) else {
        return Decision::Allow;
    };
    if let Some(refusal) = path_refusal(spec, &path, writes) {
        let verb = if writes { "write to" } else { "read" };
        return deny(
            format!(
                "APEX policy: this session may not {verb} {} — {}",
                path.display(),
                refusal.as_clause()
            ),
            refusal,
        );
    }
    Decision::Allow
}

fn deny(reason: String, refusal: KernelRefusal) -> Decision {
    Decision::Deny { reason, refusal }
}

/// The programs whose whole purpose is to acquire privilege.
///
/// Matched on the first word only, and deliberately not on anything deeper.
/// See [`decide`] for why a longer list here would be a worse module rather
/// than a safer one.
const PRIVILEGE_PROGRAMS: &[&str] = &["sudo", "pkexec", "doas", "su", "run0"];

fn bash_refusal(policy: &AgentPolicy, input: &serde_json::Value) -> Option<KernelRefusal> {
    if !policy.no_new_privs() {
        // Break-glass. §3.4 says "unsafe everything" must exist and must mean
        // it; a hook that kept saying no there would be a mode that lies.
        return None;
    }
    let command = input.get("command")?.as_str()?;
    let first = command.split_whitespace().next()?;
    let base = first.rsplit('/').next().unwrap_or(first);
    PRIVILEGE_PROGRAMS
        .contains(&base)
        .then_some(KernelRefusal::NoNewPrivs)
}

fn network_refusal(
    tool: &str,
    policy: &AgentPolicy,
    allowlist: &Allowlist,
    input: &serde_json::Value,
) -> Option<(KernelRefusal, String)> {
    let network = policy.effective_network();
    if !network.removes_direct_egress() {
        return None;
    }
    if network != NetworkPolicy::Allowlist {
        return Some((
            KernelRefusal::EmptyNetNamespace,
            format!("this session has no network, so {tool} cannot reach anything"),
        ));
    }
    // The allowlist mode has a proxy, so the question is where to, and the
    // answer comes from the same function the broker on the far end of the
    // socket uses. A second rule set here would drift from the enforcement it
    // is supposed to be predicting.
    let url = input.get("url")?.as_str()?;
    let dest = Destination::from_url(url).ok()?;
    if allowlist.decide(&dest).is_allowed() {
        return None;
    }
    Some((
        KernelRefusal::Allowlist,
        format!("{} is not on this session's allowlist", dest.host()),
    ))
}

/// The path a tool touches, and whether it writes to it.
fn file_operation(tool: &str, input: &serde_json::Value) -> Option<(PathBuf, bool)> {
    let (key, writes) = match tool {
        "Write" | "Edit" => ("file_path", true),
        "NotebookEdit" => ("notebook_path", true),
        "Read" => ("file_path", false),
        _ => return None,
    };
    let raw = input.get(key)?.as_str()?;
    let path = PathBuf::from(raw);
    // A relative path is resolved by the agent against a working directory
    // this function does not have, and guessing at one would produce a denial
    // about a path nobody named. The sandbox judges the real one.
    path.is_absolute().then_some((path, writes))
}

fn path_refusal(spec: &SandboxSpec, path: &Path, writes: bool) -> Option<KernelRefusal> {
    if spec.mask.iter().any(|m| m == path) {
        return Some(KernelRefusal::MaskedFile);
    }
    if !writes {
        // `--ro-bind / /` is exactly that: the whole filesystem is readable
        // inside a confined session. Refusing a read the kernel allows would
        // be the theatre this module is built to avoid, so the only read this
        // denies is one of a masked file.
        return None;
    }
    if writable(spec, path) {
        return None;
    }
    Some(KernelRefusal::ReadOnlyRoot)
}

/// Whether a confined session can write to `path` and have the write land
/// anywhere at all.
///
/// The tmpfs mounts count. A write into the masked `$HOME` succeeds and is
/// discarded when the session ends, which is not a refusal and must not be
/// reported as one — the sandbox has already made it harmless.
fn writable(spec: &SandboxSpec, path: &Path) -> bool {
    let tmpfs = [
        spec.home.as_path(),
        spec.runtime_dir.as_path(),
        Path::new("/tmp"),
        Path::new("/run"),
        Path::new("/proc"),
        Path::new("/dev"),
    ];
    if tmpfs
        .iter()
        .chain(std::iter::once(&spec.scratch.as_path()))
        .any(|base| !base.as_os_str().is_empty() && under(path, base))
    {
        return true;
    }
    spec.rw.iter().any(|base| under(path, base))
}

/// Whether `path` is `base` or sits under it.
///
/// Lexical, because both sides have already been through
/// [`crate::sandbox::real_target`] by the time a spec is stored, and because a
/// hook must not stat a path an agent chose: `decide` is called once per tool
/// call and has three seconds to answer.
fn under(path: &Path, base: &Path) -> bool {
    !base.as_os_str().is_empty() && (path == base || path.starts_with(base))
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
    fn the_subagent_events_carry_the_two_fields_the_graph_is_built_from() {
        // P0-011 delivered both events to the daemon and dropped `agent_id`
        // and `agent_type` here, so the detail line said "Explore started" and
        // nothing anywhere recorded which subagent that was. This is the
        // regression: the two fields leave `observe`, not just `detail_for`.
        let payload = Payload {
            agent_id: Some("agent-7".into()),
            agent_type: Some("Explore".into()),
            ..Default::default()
        };
        for event in [HookEvent::SubagentStart, HookEvent::SubagentStop] {
            let obs = observe(event, &payload);
            assert_eq!(obs.agent_id.as_deref(), Some("agent-7"), "{event}");
            assert_eq!(obs.agent_type.as_deref(), Some("Explore"), "{event}");
        }
    }

    #[test]
    fn no_other_event_opens_a_node_in_the_graph() {
        // `agent_type` is set on Claude's task events too, and letting it
        // through there would have the daemon open a subagent for a to-do
        // item. The daemon branches on the event, but a field that is only
        // ever meaningful for two events is carried for two events.
        let payload = Payload {
            agent_id: Some("agent-7".into()),
            agent_type: Some("Explore".into()),
            ..Default::default()
        };
        for event in HookEvent::ALL {
            if matches!(event, HookEvent::SubagentStart | HookEvent::SubagentStop) {
                continue;
            }
            let obs = observe(*event, &payload);
            assert!(obs.agent_id.is_none(), "{event} carried an agent id");
            assert!(obs.agent_type.is_none(), "{event} carried an agent type");
        }
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
    // -----------------------------------------------------------------------
    // §6.2 — the policy point
    // -----------------------------------------------------------------------

    use crate::destination::Allowlist;
    use crate::policy::{AgentPolicy, NetworkPolicy, SandboxPolicy, SystemAccess};
    use crate::sandbox::{build_argv, SandboxSpec};

    /// A spec shaped like the one `apex-agentd` builds for a project session:
    /// confined, the project writable, the credential files masked.
    fn confined_spec() -> SandboxSpec {
        let mut spec = SandboxSpec::new(
            AgentPolicy::default(),
            PathBuf::from("/home/tester"),
            PathBuf::from("/run/user/1000"),
        );
        spec.cwd = PathBuf::from("/home/tester/p");
        spec.scratch = PathBuf::from("/tmp/apex-agent/7");
        spec.rw.push(PathBuf::from("/home/tester/p"));
        crate::adapter::by_id("claude")
            .expect("the claude adapter")
            .apply_sandbox(&mut spec);
        spec
    }

    fn call(tool: &str, input: serde_json::Value) -> Payload {
        payload(serde_json::json!({"tool_name": tool, "tool_input": input}))
    }

    fn verdict(spec: &SandboxSpec, tool: &str, input: serde_json::Value) -> Decision {
        decide(&call(tool, input), spec, &Allowlist::default())
    }

    #[test]
    fn a_write_outside_every_writable_bind_is_denied() {
        let spec = confined_spec();
        let d = verdict(
            &spec,
            "Write",
            serde_json::json!({"file_path": "/usr/bin/apex", "content": "x"}),
        );
        assert!(d.is_deny(), "{d:?}");
        assert!(matches!(
            d,
            Decision::Deny {
                refusal: KernelRefusal::ReadOnlyRoot,
                ..
            }
        ));
    }

    #[test]
    fn a_write_the_sandbox_permits_is_not_denied() {
        // The project is bound writable, /tmp and the scratch directory are
        // tmpfs, and the masked home is a tmpfs too — a write there succeeds
        // and is discarded, which is not a refusal and must not be reported as
        // one. Denying any of these would be advice with nothing behind it.
        let spec = confined_spec();
        for path in [
            "/home/tester/p/src/main.rs",
            "/tmp/scratch.txt",
            "/tmp/apex-agent/7/notes",
            "/home/tester/anywhere",
        ] {
            let d = verdict(&spec, "Write", serde_json::json!({"file_path": path}));
            assert_eq!(d, Decision::Allow, "{path}");
        }
    }

    #[test]
    fn a_masked_credential_is_denied_in_both_directions() {
        let spec = confined_spec();
        for tool in ["Read", "Write"] {
            let d = verdict(
                &spec,
                tool,
                serde_json::json!({"file_path": "/home/tester/.npmrc"}),
            );
            assert!(matches!(
                d,
                Decision::Deny {
                    refusal: KernelRefusal::MaskedFile,
                    ..
                }
            ), "{tool}: {d:?}");
        }
    }

    #[test]
    fn a_read_the_kernel_allows_is_not_denied() {
        // `--ro-bind / /` means a confined session can read the whole
        // filesystem. That is the sandbox's decision, and a hook that pretended
        // otherwise would be denying something it cannot enforce.
        let spec = confined_spec();
        for path in ["/etc/passwd", "/var/log/messages", "/usr/lib/os-release"] {
            assert_eq!(
                verdict(&spec, "Read", serde_json::json!({"file_path": path})),
                Decision::Allow,
                "{path}"
            );
        }
    }

    #[test]
    fn a_relative_path_is_left_to_the_sandbox() {
        // It is resolved against a working directory this function does not
        // have, and a guess would produce a refusal naming a path nobody wrote.
        let spec = confined_spec();
        assert_eq!(
            verdict(&spec, "Write", serde_json::json!({"file_path": "../../etc/hosts"})),
            Decision::Allow
        );
    }

    #[test]
    fn an_offline_session_is_denied_the_network() {
        let mut spec = confined_spec();
        spec.policy.network = NetworkPolicy::Offline;
        for tool in ["WebFetch", "WebSearch"] {
            let d = verdict(&spec, tool, serde_json::json!({"url": "https://example.com/x"}));
            assert!(matches!(
                d,
                Decision::Deny {
                    refusal: KernelRefusal::EmptyNetNamespace,
                    ..
                }
            ), "{tool}: {d:?}");
        }
    }

    #[test]
    fn an_open_session_is_denied_nothing_about_the_network() {
        let mut spec = confined_spec();
        spec.policy.network = NetworkPolicy::Open;
        assert_eq!(
            verdict(&spec, "WebFetch", serde_json::json!({"url": "https://example.com"})),
            Decision::Allow
        );
    }

    #[test]
    fn an_allowlisted_session_is_judged_by_the_brokers_own_function() {
        // Not a second rule set: `Allowlist::decide` is what the egress broker
        // on the far end of the session's only socket uses, so the hook's
        // answer and the enforcement cannot drift apart.
        let mut spec = confined_spec();
        spec.policy.network = NetworkPolicy::Allowlist;
        let allowlist = Allowlist::parse(&["github.com:443"]).expect("allowlist");

        let allowed = decide(
            &call("WebFetch", serde_json::json!({"url": "https://github.com/a/b"})),
            &spec,
            &allowlist,
        );
        assert_eq!(allowed, Decision::Allow);

        let refused = decide(
            &call("WebFetch", serde_json::json!({"url": "https://evil.example/x"})),
            &spec,
            &allowlist,
        );
        assert!(matches!(
            refused,
            Decision::Deny {
                refusal: KernelRefusal::Allowlist,
                ..
            }
        ), "{refused:?}");
        // And the broker agrees, which is the point of reusing its function.
        let dest = Destination::from_url("https://evil.example/x").expect("destination");
        assert!(!allowlist.decide(&dest).is_allowed());
    }

    #[test]
    fn sudo_is_denied_because_no_new_privs_makes_it_pointless() {
        let spec = confined_spec();
        for command in ["sudo dnf install x", "/usr/bin/pkexec id", "su -", "doas ls"] {
            let d = verdict(&spec, "Bash", serde_json::json!({"command": command}));
            assert!(matches!(
                d,
                Decision::Deny {
                    refusal: KernelRefusal::NoNewPrivs,
                    ..
                }
            ), "{command}: {d:?}");
        }
    }

    #[test]
    fn break_glass_is_allowed_to_mean_it() {
        // §3.4: "unsafe everything" must exist. A mode that still said no to
        // sudo would be a mode that lies, and `no_new_privs` is genuinely off
        // there, so the refusal this mirrors does not exist either.
        let mut spec = confined_spec();
        spec.policy.system = SystemAccess::Unsafe;
        assert_eq!(
            verdict(&spec, "Bash", serde_json::json!({"command": "sudo id"})),
            Decision::Allow
        );
    }

    #[test]
    fn a_bash_command_is_not_read_past_its_first_word() {
        // Parsing a shell well enough to find the writes inside it is a losing
        // game, and a rule that caught `rm -rf /usr` but not `sh -c 'rm -rf
        // /usr'` would read as protection while providing none.
        let spec = confined_spec();
        for command in [
            "rm -rf /usr",
            "sh -c 'sudo id'",
            "echo x > /etc/passwd",
            "env sudo id",
        ] {
            assert_eq!(
                verdict(&spec, "Bash", serde_json::json!({"command": command})),
                Decision::Allow,
                "{command}"
            );
        }
    }

    #[test]
    fn an_unknown_tool_is_not_judged() {
        // An `mcp__*` tool's input shape belongs to its server. Inventing a
        // rule for it would deny on a key that means something else.
        let spec = confined_spec();
        assert_eq!(
            verdict(
                &spec,
                "mcp__memory__write",
                serde_json::json!({"file_path": "/usr/x"})
            ),
            Decision::Allow
        );
        assert_eq!(
            decide(&Payload::default(), &spec, &Allowlist::default()),
            Decision::Allow
        );
    }

    #[test]
    fn a_session_the_user_unconfined_is_denied_nothing_the_mounts_would_have_stopped() {
        // The honest half of §6.2. With `--sandbox unrestricted` there is no
        // mount to mirror, so a path denial here would be advice the agent can
        // switch off with nothing underneath. no_new_privs still holds,
        // because the runtime sets it on the process rather than by a mount.
        let mut spec = confined_spec();
        spec.policy.sandbox = SandboxPolicy::Unrestricted;
        spec.policy.network = NetworkPolicy::Open;
        assert_eq!(
            verdict(&spec, "Write", serde_json::json!({"file_path": "/usr/bin/apex"})),
            Decision::Allow
        );
        assert!(verdict(&spec, "Bash", serde_json::json!({"command": "sudo id"})).is_deny());
    }

    #[test]
    fn an_allow_prints_nothing_at_all() {
        // A hook that printed `"allow"` would not be agreeing, it would be
        // SKIPPING the permission system for that call. APEX is here to
        // withhold a permission, never to grant one.
        assert_eq!(Decision::Allow.to_hook_output(), None);
        let doc = Decision::Deny {
            reason: "because".into(),
            refusal: KernelRefusal::ReadOnlyRoot,
        }
        .to_hook_output()
        .expect("a deny document");
        assert_eq!(doc["hookSpecificOutput"]["hookEventName"], "PreToolUse");
        assert_eq!(doc["hookSpecificOutput"]["permissionDecision"], "deny");
        assert_eq!(
            doc["hookSpecificOutput"]["permissionDecisionReason"],
            "because"
        );
    }

    #[test]
    fn every_denial_names_a_restriction_the_argv_actually_carries() {
        // The no-theatre gate. Each deny below is re-checked against the argv
        // `build_argv` really produces for the same spec, so a rule added later
        // that refuses something the kernel would have allowed fails here
        // rather than shipping as security.
        let spec = confined_spec();
        let argv = build_argv(&spec, "claude", &[]).expect("argv");
        let has = |w: &[&str]| argv.windows(w.len()).any(|win| win == w);

        // ReadOnlyRoot: the whole filesystem is mounted read-only and nothing
        // binds a writable copy over the path.
        assert!(verdict(&spec, "Write", serde_json::json!({"file_path": "/usr/bin/apex"})).is_deny());
        assert!(has(&["--ro-bind", "/", "/"]));
        assert!(
            !argv.iter().any(|a| a == "/usr" || a == "/usr/bin"),
            "a writable bind over /usr would make that denial theatre"
        );

        // MaskedFile: the path is /dev/null, read-only.
        assert!(verdict(&spec, "Read", serde_json::json!({"file_path": "/home/tester/.npmrc"})).is_deny());
        assert!(has(&["--ro-bind-try", "/dev/null", "/home/tester/.npmrc"]));

        // EmptyNetNamespace: the namespace is unshared and nothing is put in it.
        let mut offline = confined_spec();
        offline.policy.network = NetworkPolicy::Offline;
        assert!(verdict(&offline, "WebFetch", serde_json::json!({"url": "https://x.example"})).is_deny());
        let offline_argv = build_argv(&offline, "claude", &[]).expect("argv");
        assert!(offline_argv.iter().any(|a| a == "--unshare-net"));
        assert!(offline.policy.effective_network().removes_direct_egress());

        // NoNewPrivs: the one refusal that is not a mount, and therefore the
        // one this argv cannot show. `bwrap` sets it for every confined
        // session, and `pty::spawn` calls `prctl(PR_SET_NO_NEW_PRIVS)` itself
        // for an unconfined one — refusing to spawn if the call fails, and
        // proved there by a test with a negative control. So the deny holds in
        // both, and what is asserted here is the policy this reads it from.
        assert!(verdict(&spec, "Bash", serde_json::json!({"command": "sudo id"})).is_deny());
        assert!(spec.policy.no_new_privs());
    }

    #[test]
    fn a_url_becomes_the_destination_the_broker_would_have_been_handed() {
        assert_eq!(
            Destination::from_url("https://api.github.com/repos").expect("d"),
            Destination::new("api.github.com", 443).expect("d")
        );
        assert_eq!(
            Destination::from_url("http://Example.COM./x?y#z").expect("d"),
            Destination::new("example.com", 80).expect("d")
        );
        assert_eq!(
            Destination::from_url("https://user:pw@host.example:8443/p").expect("d"),
            Destination::new("host.example", 8443).expect("d")
        );
        // A scheme with no proxy port is not a destination this can judge.
        assert!(Destination::from_url("file:///etc/passwd").is_err());
        assert!(Destination::from_url("not a url").is_err());
    }

}
