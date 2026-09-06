//! What a session started, and whether it is still going (§P1-020).
//!
//! A session is not one thing. Claude delegates to subagents, every adapter
//! forks MCP servers and tool processes, and the Agent Center up to now showed
//! a single row per session with no way to tell an agent that has spawned six
//! subagents from one sitting idle. This module is the record of that second
//! layer.
//!
//! ## Why the children are recorded here and not derived
//!
//! Claude's subagents are in-process. They have no pid, they never appear in
//! `/proc`, and their only on-disk trace is a directory of transcript symlinks
//! under `/tmp/claude-<uid>` that is harness-internal and changes between
//! releases. The supported channel is the hook: `SubagentStart` and
//! `SubagentStop` arrive with `agent_id` and `agent_type` on the payload, and
//! P0-011 already delivers both events to the daemon. It threw the two fields
//! away because there was nowhere to put them. This is that place.
//!
//! ## A child has no state field, on purpose
//!
//! Every entry carries `started` and an optional `ended`, and liveness is the
//! question "is `ended` absent, and is the parent still running?" — asked at
//! read time, never stored.
//!
//! This is not a style preference. A status field is a claim that was true when
//! it was written, and the failure it produces is specific: a subagent whose
//! `SubagentStop` never arrived (the turn was interrupted, the agent was
//! killed, the hook timed out) keeps claiming it is working for as long as the
//! record survives. A graph that shows a dead agent as alive is worse than one
//! that shows nothing, because the person reading it stops checking. So the
//! only thing written down is what was observed to happen, and the conclusion
//! is drawn from it every time.
//!
//! [`close_open`] is what makes that honest for the events that never came:
//! the turn ending and the process exiting both close whatever is still open,
//! and each records WHICH of those closed it, so "finished" and "we stopped
//! being able to tell" are different words on the screen.

use serde::{Deserialize, Serialize};

/// What kind of thing a child is.
///
/// Two variants because the two are learned from different evidence and fail
/// in different ways: a subagent is reported by the agent and can therefore go
/// unreported, and a process is observed in `/proc` and can therefore be seen
/// after the agent has forgotten about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChildKind {
    /// A Claude subagent. In-process, no pid, known only from hook events.
    Subagent,
    /// A process the session forked: an MCP server, a language server, a
    /// compiler, a test run.
    Process,
}

impl ChildKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            ChildKind::Subagent => "subagent",
            ChildKind::Process => "process",
        }
    }
}

/// Why a child stopped being open.
///
/// Recorded rather than collapsed to a boolean because the four are not the
/// same news. `Reported` is a subagent that finished and said so. The other
/// three are all "it is not running any more", arrived at by three different
/// routes, and a reader who is debugging a hook that never fires needs to be
/// able to tell them apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChildEnd {
    /// The agent published `subagent_stop` for it.
    Reported,
    /// The turn ended with the child still open. Claude's `Stop` fires once
    /// the assistant has finished replying, and nothing it delegated outlives
    /// that.
    ParentStop,
    /// The session's own process is gone.
    ParentExit,
    /// A process that was in the table and is not any more.
    Gone,
}

impl ChildEnd {
    pub fn as_str(&self) -> &'static str {
        match self {
            ChildEnd::Reported => "reported",
            ChildEnd::ParentStop => "parent_stop",
            ChildEnd::ParentExit => "parent_exit",
            ChildEnd::Gone => "gone",
        }
    }
}

/// One node under a session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChildInfo {
    /// Stable within the session. The agent's own `agent_id` when it sent one,
    /// `anon:<n>` when it did not, `pid:<n>` for a process.
    pub id: String,
    pub kind: ChildKind,
    /// What to call it: the `agent_type` Claude sent (`Explore`,
    /// `general-purpose`), or a process's command name. Never empty — see
    /// [`clamp_label`] for what an unusable one becomes.
    pub label: String,
    /// Unix seconds at which it was first seen.
    pub started: u64,
    /// Unix seconds at which it stopped, once something has said so.
    #[serde(default)]
    pub ended: Option<u64>,
    /// Which of the four routes in [`ChildEnd`] closed it.
    #[serde(default)]
    pub ended_by: Option<ChildEnd>,
    /// The process, for [`ChildKind::Process`].
    #[serde(default)]
    pub pid: Option<i32>,
    /// Resident set size in kilobytes, as of the last observation.
    ///
    /// `None` for a subagent, which has no memory of its own: it runs inside
    /// the agent's process and its cost is already in the parent's. Reporting
    /// a number there would be inventing one.
    #[serde(default)]
    pub rss_kb: Option<u64>,
}

impl ChildInfo {
    /// True while nothing has said it stopped.
    ///
    /// Deliberately not called `is_live`: this answers a question about the
    /// RECORD, and a caller still has to ask whether the parent is running.
    /// [`live_children`] is the one that answers the whole question.
    pub fn is_open(&self) -> bool {
        self.ended.is_none()
    }

    /// Seconds it ran for, or has been running for.
    pub fn duration(&self, now: u64) -> u64 {
        self.ended.unwrap_or(now).saturating_sub(self.started)
    }
}

/// Most children kept on one session.
///
/// The record is rewritten to disk on every state change, so an unbounded list
/// is an unbounded write. A hundred and twenty-eight is far above any real
/// turn — the busiest session on the machine this was written on had eleven
/// subagents — and the eviction below keeps the interesting end.
pub const MAX_CHILDREN: usize = 128;

/// Longest label kept. It is agent-supplied text rendered in a fixed-width
/// column, in the same class as `SessionInfo::detail`.
pub const MAX_LABEL: usize = 48;

/// Record that a subagent started.
///
/// A start for an id already in the list is ignored rather than appended.
/// `agent_id` is Claude's own identifier and is unique per delegation, so a
/// repeat is a hook that ran twice — a retry, a settings file that subscribed
/// the same event through two matchers — and appending would show one task as
/// two.
pub fn subagent_started(
    children: &mut Vec<ChildInfo>,
    agent_id: Option<&str>,
    agent_type: Option<&str>,
    at: u64,
) {
    let id = match agent_id.map(str::trim).filter(|s| !s.is_empty()) {
        Some(given) => clamp_id(given),
        // No id on the payload. Synthesised from the position rather than
        // dropped: the delegation happened, and a graph that omits the
        // subagents of an agent whose harness stopped sending ids is a graph
        // that is silently wrong rather than visibly incomplete.
        None => format!("anon:{}", children.len()),
    };
    if children.iter().any(|c| c.id == id) {
        return;
    }
    push(
        children,
        ChildInfo {
            id,
            kind: ChildKind::Subagent,
            label: clamp_label(agent_type),
            started: at,
            ended: None,
            ended_by: None,
            pid: None,
            rss_kb: None,
        },
    );
}

/// Record that a subagent stopped.
///
/// Three cases, and the third is the one worth stating. A stop naming an open
/// child closes it. A stop naming nothing closes the oldest open child that
/// also arrived without a name — never a named one, because attributing an
/// anonymous finish to a task that has an id would show the wrong task as
/// done, and a wrong answer here is worse than a missing one. A stop naming a
/// child nobody saw start is recorded as a child that began and ended at the
/// same instant: the delegation demonstrably happened, and the alternative is
/// to throw away the only evidence of it.
pub fn subagent_stopped(
    children: &mut Vec<ChildInfo>,
    agent_id: Option<&str>,
    agent_type: Option<&str>,
    at: u64,
) {
    let named = agent_id
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(clamp_id);

    let found = match named.as_deref() {
        Some(id) => children.iter_mut().find(|c| c.id == id && c.is_open()),
        None => children
            .iter_mut()
            .find(|c| c.is_open() && c.id.starts_with("anon:")),
    };
    if let Some(child) = found {
        child.ended = Some(at);
        child.ended_by = Some(ChildEnd::Reported);
        return;
    }
    // Nothing open answers to it.
    let id = named.unwrap_or_else(|| format!("anon:{}", children.len()));
    if children.iter().any(|c| c.id == id) {
        return;
    }
    push(
        children,
        ChildInfo {
            id,
            kind: ChildKind::Subagent,
            label: clamp_label(agent_type),
            started: at,
            ended: Some(at),
            ended_by: Some(ChildEnd::Reported),
            pid: None,
            rss_kb: None,
        },
    );
}

/// Close everything still open, and say what closed it. Returns how many.
///
/// The reason the record can be trusted. `SubagentStop` is a hook, and §6.1 is
/// explicit that a hook is advisory: it can be silenced, it can time out, and
/// it does not run at all when the agent is killed. So the two moments the
/// daemon knows about on its own — the turn ending and the process exiting —
/// both sweep the list, and an entry left open past either of those is a bug
/// in this file rather than a subagent that is still working.
pub fn close_open(children: &mut [ChildInfo], why: ChildEnd, at: u64) -> usize {
    let mut closed = 0;
    for child in children.iter_mut() {
        if child.is_open() {
            child.ended = Some(at);
            child.ended_by = Some(why);
            closed += 1;
        }
    }
    closed
}

/// The children that are genuinely running: still open, and under a parent
/// that has not exited.
///
/// `parent_live` is a parameter rather than something this reads, because the
/// caller is the one holding the session and this module deliberately knows
/// nothing about [`crate::protocol::SessionInfo`].
pub fn live_children(children: &[ChildInfo], parent_live: bool) -> Vec<&ChildInfo> {
    if !parent_live {
        return Vec::new();
    }
    children.iter().filter(|c| c.is_open()).collect()
}

/// Task accounting: how many were delegated, how many are still going, and how
/// much wall time they have used between them.
///
/// Wall time rather than CPU, and summed across children that overlap, so the
/// total can exceed the session's own age. That is the honest number for the
/// question it answers — how much work was delegated — and the alternative,
/// a union of the intervals, answers "how long was the session busy", which
/// the session's own elapsed time already says.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Accounting {
    pub total: usize,
    pub open: usize,
    pub seconds: u64,
    /// Summed resident set size of the child processes, in kilobytes.
    ///
    /// Processes only. A subagent contributes nothing because its memory is
    /// the parent's, so a session that delegates heavily and forks nothing
    /// reports zero here and is not being under-reported.
    pub rss_kb: u64,
}

pub fn account(children: &[ChildInfo], parent_live: bool, now: u64) -> Accounting {
    let mut acc = Accounting {
        total: children.len(),
        ..Default::default()
    };
    for child in children {
        let open = child.is_open() && parent_live;
        if open {
            acc.open += 1;
        }
        // A child left open under a dead parent contributes the time up to the
        // last thing anybody saw, not up to now. Otherwise a session that
        // exited last week would report a subagent that has been running for
        // a week.
        acc.seconds += if child.is_open() && !parent_live {
            0
        } else {
            child.duration(now)
        };
        if open {
            acc.rss_kb += child.rss_kb.unwrap_or(0);
        }
    }
    acc
}

/// Append, evicting the oldest finished entry when the list is full.
///
/// Finished first, and never an open one: an open child is a thing that is
/// still happening, and dropping it would leave the graph claiming the session
/// has nothing running while it does. A list of [`MAX_CHILDREN`] open children
/// therefore refuses the append — which is a bound being hit, not a state, and
/// it is bounded for the reason the constant says.
fn push(children: &mut Vec<ChildInfo>, child: ChildInfo) {
    if children.len() >= MAX_CHILDREN {
        let Some(oldest) = children
            .iter()
            .enumerate()
            .filter(|(_, c)| !c.is_open())
            .min_by_key(|(_, c)| c.ended.unwrap_or(0))
            .map(|(i, _)| i)
        else {
            return;
        };
        children.remove(oldest);
    }
    children.push(child);
}

/// Bound an id and strip anything that is not an identifier.
///
/// It comes off a document the agent writes and is compared for equality, put
/// in a JSON record and rendered. Filtered rather than escaped, because an id
/// is a name and there is no legitimate id with a newline in it.
fn clamp_id(raw: &str) -> String {
    let kept: String = raw
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_' || *c == '.')
        .take(MAX_LABEL)
        .collect();
    if kept.is_empty() {
        // Not the raw string, and not an error: an id made entirely of
        // characters an id may not contain is one this cannot pair, and
        // `anon:` is exactly the bucket for a child that cannot be paired.
        return "anon:unnamed".to_string();
    }
    kept
}

/// Bound a label and make it printable.
///
/// `subagent` for an absent or unusable one, which is the word `hook::observe`
/// already puts in the detail line for the same case.
pub fn clamp_label(raw: Option<&str>) -> String {
    let Some(text) = raw else {
        return "subagent".to_string();
    };
    let kept: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if kept.is_empty() {
        return "subagent".to_string();
    }
    kept.chars().take(MAX_LABEL).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kids() -> Vec<ChildInfo> {
        Vec::new()
    }

    #[test]
    fn a_start_and_a_stop_are_one_child() {
        let mut c = kids();
        subagent_started(&mut c, Some("a1"), Some("Explore"), 100);
        subagent_stopped(&mut c, Some("a1"), Some("Explore"), 160);
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].id, "a1");
        assert_eq!(c[0].label, "Explore");
        assert_eq!(c[0].ended, Some(160));
        assert_eq!(c[0].ended_by, Some(ChildEnd::Reported));
        assert_eq!(c[0].duration(999), 60);
    }

    #[test]
    fn a_repeated_start_is_one_child() {
        let mut c = kids();
        subagent_started(&mut c, Some("a1"), Some("Explore"), 100);
        subagent_started(&mut c, Some("a1"), Some("Explore"), 101);
        assert_eq!(c.len(), 1);
    }

    #[test]
    fn a_stop_nobody_saw_start_is_still_recorded() {
        let mut c = kids();
        subagent_stopped(&mut c, Some("ghost"), Some("Plan"), 50);
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].started, 50);
        assert_eq!(c[0].ended, Some(50));
        assert!(!c[0].is_open());
    }

    #[test]
    fn an_anonymous_stop_never_closes_a_named_child() {
        let mut c = kids();
        subagent_started(&mut c, Some("a1"), Some("Explore"), 100);
        subagent_stopped(&mut c, None, Some("Explore"), 120);
        assert!(c[0].is_open(), "the named child must still be open");
        assert_eq!(c.len(), 2, "the anonymous stop is recorded on its own");
    }

    #[test]
    fn an_anonymous_stop_closes_the_oldest_anonymous_child() {
        let mut c = kids();
        subagent_started(&mut c, None, Some("Explore"), 100);
        subagent_started(&mut c, None, Some("Plan"), 110);
        subagent_stopped(&mut c, None, None, 120);
        assert_eq!(c[0].ended, Some(120));
        assert!(c[1].is_open());
    }

    #[test]
    fn closing_says_which_route_closed_it() {
        let mut c = kids();
        subagent_started(&mut c, Some("a1"), Some("Explore"), 100);
        subagent_started(&mut c, Some("a2"), Some("Plan"), 101);
        subagent_stopped(&mut c, Some("a1"), None, 150);
        assert_eq!(close_open(&mut c, ChildEnd::ParentExit, 200), 1);
        assert_eq!(c[0].ended_by, Some(ChildEnd::Reported));
        assert_eq!(c[1].ended_by, Some(ChildEnd::ParentExit));
        assert_eq!(close_open(&mut c, ChildEnd::ParentExit, 300), 0);
    }

    #[test]
    fn a_dead_parent_has_no_live_children() {
        let mut c = kids();
        subagent_started(&mut c, Some("a1"), Some("Explore"), 100);
        assert_eq!(live_children(&c, true).len(), 1);
        assert_eq!(live_children(&c, false).len(), 0);
    }

    #[test]
    fn accounting_counts_delegated_work_and_not_a_dead_parents_open_child() {
        let mut c = kids();
        subagent_started(&mut c, Some("a1"), Some("Explore"), 100);
        subagent_stopped(&mut c, Some("a1"), None, 160);
        subagent_started(&mut c, Some("a2"), Some("Plan"), 120);

        let live = account(&c, true, 200);
        assert_eq!(live.total, 2);
        assert_eq!(live.open, 1);
        assert_eq!(live.seconds, 60 + 80);

        let dead = account(&c, false, 999_999);
        assert_eq!(dead.open, 0);
        assert_eq!(dead.seconds, 60, "the open child of a dead parent adds none");
    }

    #[test]
    fn resident_memory_is_processes_only_and_only_while_open() {
        let mut c = vec![
            ChildInfo {
                id: "pid:10".into(),
                kind: ChildKind::Process,
                label: "node".into(),
                started: 100,
                ended: None,
                ended_by: None,
                pid: Some(10),
                rss_kb: Some(4096),
            },
            ChildInfo {
                id: "pid:11".into(),
                kind: ChildKind::Process,
                label: "gone".into(),
                started: 100,
                ended: Some(150),
                ended_by: Some(ChildEnd::Gone),
                pid: Some(11),
                rss_kb: Some(9999),
            },
        ];
        assert_eq!(account(&c, true, 200).rss_kb, 4096);
        close_open(&mut c, ChildEnd::ParentExit, 200);
        assert_eq!(account(&c, true, 200).rss_kb, 0);
    }

    #[test]
    fn the_list_is_bounded_and_evicts_a_finished_entry_first() {
        let mut c = kids();
        for i in 0..MAX_CHILDREN {
            subagent_started(&mut c, Some(&format!("a{i}")), Some("Explore"), 100 + i as u64);
        }
        subagent_stopped(&mut c, Some("a5"), None, 500);
        subagent_started(&mut c, Some("late"), Some("Explore"), 600);
        assert_eq!(c.len(), MAX_CHILDREN);
        assert!(c.iter().all(|k| k.id != "a5"), "the finished one was evicted");
        assert!(c.iter().any(|k| k.id == "late"));
    }

    #[test]
    fn a_full_list_of_open_children_refuses_rather_than_dropping_one() {
        let mut c = kids();
        for i in 0..MAX_CHILDREN {
            subagent_started(&mut c, Some(&format!("a{i}")), Some("Explore"), 100);
        }
        subagent_started(&mut c, Some("late"), Some("Explore"), 600);
        assert_eq!(c.len(), MAX_CHILDREN);
        assert!(c.iter().all(|k| k.id != "late"));
    }

    #[test]
    fn a_label_is_bounded_printable_and_never_empty() {
        assert_eq!(clamp_label(None), "subagent");
        assert_eq!(clamp_label(Some("   ")), "subagent");
        assert_eq!(clamp_label(Some("Explore\nagent")), "Explore agent");
        assert_eq!(clamp_label(Some(&"x".repeat(400))).chars().count(), MAX_LABEL);
    }

    #[test]
    fn an_id_made_of_nothing_usable_becomes_anonymous() {
        let mut c = kids();
        subagent_started(&mut c, Some("«»"), Some("Explore"), 100);
        assert_eq!(c[0].id, "anon:unnamed");
    }
}
