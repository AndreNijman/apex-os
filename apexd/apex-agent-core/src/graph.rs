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

use std::path::Path;

use serde::{Deserialize, Serialize};

/// What kind of thing a child is.
///
/// Two variants because the two are learned from different evidence and fail in
/// different ways. A subagent is reported by the agent, so it can go
/// unreported, and what is written down about it is history. A process is read
/// out of `/proc` at the moment the graph is asked for, so it is never history
/// and never stale — it is either there or it is not.
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
/// Recorded rather than collapsed to a boolean because the three are not the
/// same news. `Reported` is a subagent that finished and said so; the other two
/// are the daemon noticing that it cannot still be running. A reader debugging
/// a hook that never fires needs to be able to tell those apart, and a user
/// reading the graph needs "finished" and "we stopped being able to tell" to
/// be different words.
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
    // There is deliberately no `Gone`. A process child is not remembered
    // between reads — it is observed in `/proc` at the moment the graph is
    // asked for, and one that is not there is simply not in the answer. A
    // variant this crate never produces would be a wire value a client could
    // reasonably branch on and never see.
}

impl ChildEnd {
    pub fn as_str(&self) -> &'static str {
        match self {
            ChildEnd::Reported => "reported",
            ChildEnd::ParentStop => "parent_stop",
            ChildEnd::ParentExit => "parent_exit",
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
    /// Which of the three routes in [`ChildEnd`] closed it.
    #[serde(default)]
    pub ended_by: Option<ChildEnd>,
    /// The id of the child this one hangs off, when it is not the session
    /// itself.
    ///
    /// This is what makes the graph a graph rather than a list: an MCP server
    /// forked by a language server forked by the agent is three levels down,
    /// and flattening it would put the agent's own runtime beside the thing it
    /// started. A subagent is always directly under the session, because
    /// nesting is not something the hook payload describes.
    #[serde(default)]
    pub parent: Option<String>,
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
            // A subagent hangs off the session, never off another node: the
            // hook payload says nothing about a subagent that delegated
            // further, and drawing a guessed nesting would be inventing one.
            parent: None,
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
            parent: None,
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

// ---------------------------------------------------------------------------
// The other half of the graph: what the session actually forked
// ---------------------------------------------------------------------------
//
// MCP servers, language servers, the compiler a tool call started. None of
// them publish anything, and none of them need to: they are processes, and the
// kernel already has the answer.
//
// This works for a confined session too. `bwrap --unshare-pid` gives the
// session its own pid namespace, so a process inside sees only itself — but
// the daemon is outside that namespace and the host `/proc` still lists every
// descendant with its host pid. The tree is therefore readable for every
// adapter, confined or not, without asking the agent anything.

/// One process, as `/proc` describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcEntry {
    pub pid: i32,
    pub ppid: i32,
    /// `/proc/<pid>/comm` — the executable name, at most 15 characters.
    pub comm: String,
    /// Unix seconds at which the process started.
    pub started: u64,
}

/// Most process children reported for one session.
///
/// Lower than [`MAX_CHILDREN`] because this list is rebuilt on every read and
/// travels down the socket each time, and because the tail of a large one is
/// noise: a `cargo test` is briefly a hundred `rustc` processes, and the
/// hundredth tells a supervisor nothing the ninety-ninth did not. The walk is
/// breadth-first, so what survives the cap is what is nearest the agent.
pub const MAX_PROCESS_CHILDREN: usize = 64;

/// The descendants of `root`, nearest first, as graph nodes.
///
/// `root` itself is excluded: it is the session, and a session that is its own
/// child would be drawn under itself.
pub fn process_tree(procs: &[ProcEntry], root: i32) -> Vec<ChildInfo> {
    let mut by_parent: std::collections::BTreeMap<i32, Vec<&ProcEntry>> =
        std::collections::BTreeMap::new();
    for p in procs {
        by_parent.entry(p.ppid).or_default().push(p);
    }

    let mut out: Vec<ChildInfo> = Vec::new();
    let mut queue: std::collections::VecDeque<(i32, Option<String>)> =
        std::collections::VecDeque::new();
    queue.push_back((root, None));
    // A pid cannot be its own ancestor, but `/proc` is read without a lock and
    // a recycled pid could in principle close a cycle between two reads. The
    // seen set costs one allocation and turns that into a missing node rather
    // than a daemon thread that never returns.
    let mut seen: std::collections::BTreeSet<i32> = std::collections::BTreeSet::new();
    seen.insert(root);

    while let Some((pid, parent)) = queue.pop_front() {
        let Some(kids) = by_parent.get(&pid) else {
            continue;
        };
        for kid in kids {
            if out.len() >= MAX_PROCESS_CHILDREN {
                return out;
            }
            if !seen.insert(kid.pid) {
                continue;
            }
            let id = format!("pid:{}", kid.pid);
            out.push(ChildInfo {
                id: id.clone(),
                kind: ChildKind::Process,
                label: clamp_process_label(&kid.comm),
                started: kid.started,
                // A process observed in `/proc` is running by definition. It
                // is never closed, because it is never remembered: the next
                // read either sees it or does not.
                ended: None,
                ended_by: None,
                parent: parent.clone(),
                pid: Some(kid.pid),
                // Filled by `fill_rss` when a caller wants the number. The
                // walk itself must stay cheap enough to run on every read.
                rss_kb: None,
            });
            queue.push_back((kid.pid, Some(id)));
        }
    }
    out
}

/// Read every process the kernel is showing.
///
/// `proc_root` is a parameter so the parser can be driven from a fixture
/// directory: the shape of `/proc/<pid>/stat` is the part that is easy to get
/// wrong, and a comm containing a space or a bracket has broken more than one
/// process-tree reader.
///
/// One file per process, not three. The name is field 2 of `stat` and the
/// parent is field 4, so `comm` need not be opened at all, and `statm` is read
/// afterwards for the handful of processes that turn out to be descendants —
/// see [`fill_rss`]. On this machine, with 524 processes, three files each
/// cost 16.6 ms and one costs 7.3 ms; the walk runs on every `apex agent list`
/// and the Agent Center asks twice a second while it is open.
///
/// Anything unreadable is skipped rather than reported as an error. A process
/// exiting between the directory listing and the read is the normal case, not
/// a fault.
pub fn read_processes(proc_root: &Path) -> Vec<ProcEntry> {
    let boot = boot_time(proc_root);
    let ticks = clock_ticks().max(1);
    let Ok(entries) = std::fs::read_dir(proc_root) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Ok(pid) = name.parse::<i32>() else {
            continue;
        };
        let Ok(stat) = std::fs::read_to_string(entry.path().join("stat")) else {
            continue;
        };
        let Some(parsed) = parse_stat(&stat) else {
            continue;
        };
        out.push(ProcEntry {
            pid,
            ppid: parsed.ppid,
            comm: parsed.comm,
            started: boot.saturating_add(parsed.start_ticks / ticks),
        });
    }
    out
}

/// Read the resident set size of each node in a tree, in place.
///
/// Separate from [`read_processes`] because it is the expensive half and it is
/// only ever wanted for a few dozen processes. A node whose `statm` has gone —
/// the process exited between the two passes — keeps `None`, which the shell
/// draws as no number rather than as zero: a process using no memory and a
/// process that has just left are different facts.
pub fn fill_rss(proc_root: &Path, tree: &mut [ChildInfo]) {
    let page_kb = page_size_kb();
    for node in tree.iter_mut() {
        let Some(pid) = node.pid else { continue };
        node.rss_kb = std::fs::read_to_string(proc_root.join(pid.to_string()).join("statm"))
            .ok()
            .and_then(|s| parse_statm(&s))
            .map(|pages| pages.saturating_mul(page_kb));
    }
}

/// What one `/proc/<pid>/stat` line says.
#[derive(Debug, PartialEq, Eq)]
struct Stat {
    comm: String,
    ppid: i32,
    start_ticks: u64,
}

/// Parse one `/proc/<pid>/stat` line.
///
/// The parse everybody gets wrong. Field 2 is the executable name in
/// parentheses and it is NOT escaped: a process called `foo) 0 (bar` produces
/// a line that splitting on whitespace reads as six fields with the wrong
/// values in them — including a ppid that points at another real process,
/// which is a session showing somebody else's tree as its own. Splitting after
/// the LAST `)` is the only correct way, and it is what the kernel's own
/// documentation tells readers to do.
fn parse_stat(line: &str) -> Option<Stat> {
    let close = line.rfind(')')?;
    let open = line.find('(')?;
    if open >= close {
        return None;
    }
    let fields: Vec<&str> = line[close + 1..].split_whitespace().collect();
    // The remainder begins at field 3 (state), so field N is at index N - 3.
    Some(Stat {
        comm: line[open + 1..close].to_string(),
        ppid: fields.get(1)?.parse().ok()?,
        start_ticks: fields.get(19)?.parse().ok()?,
    })
}

/// Resident pages out of `/proc/<pid>/statm` — the second of seven numbers.
fn parse_statm(line: &str) -> Option<u64> {
    line.split_whitespace().nth(1)?.parse().ok()
}

/// Unix seconds at which the machine booted, from `/proc/stat`.
///
/// Zero when it cannot be read, which makes every `started` a small number
/// rather than a wrong-looking future one: an elapsed time computed against it
/// is visibly absurd instead of quietly plausible.
fn boot_time(proc_root: &Path) -> u64 {
    std::fs::read_to_string(proc_root.join("stat"))
        .ok()
        .and_then(|text| {
            text.lines()
                .find_map(|l| l.strip_prefix("btime "))
                .and_then(|v| v.trim().parse().ok())
        })
        .unwrap_or(0)
}

fn clock_ticks() -> u64 {
    // SAFETY: `sysconf` reads a static configuration value and touches nothing
    // the caller owns.
    let v = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    if v > 0 {
        v as u64
    } else {
        100
    }
}

fn page_size_kb() -> u64 {
    // SAFETY: as above.
    let v = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    if v > 0 {
        (v as u64) / 1024
    } else {
        4
    }
}

/// A process name, bounded and printable.
///
/// `comm` is fifteen characters and is chosen by the process itself, so it is
/// short already; this exists because a thread can rename itself to anything a
/// byte string can hold and the result is rendered.
fn clamp_process_label(comm: &str) -> String {
    let kept: String = comm
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_LABEL)
        .collect();
    if kept.trim().is_empty() {
        return "process".to_string();
    }
    kept
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
                parent: None,
                pid: Some(10),
                rss_kb: Some(4096),
            },
            ChildInfo {
                id: "pid:11".into(),
                kind: ChildKind::Process,
                label: "gone".into(),
                started: 100,
                ended: Some(150),
                ended_by: Some(ChildEnd::ParentStop),
                parent: None,
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

    fn proc(pid: i32, ppid: i32, comm: &str) -> ProcEntry {
        ProcEntry {
            pid,
            ppid,
            comm: comm.into(),
            started: 1000,
        }
    }

    fn stat(comm: &str, ppid: i32, start_ticks: u64) -> Option<Stat> {
        Some(Stat {
            comm: comm.into(),
            ppid,
            start_ticks,
        })
    }

    #[test]
    fn the_tree_is_the_descendants_and_never_the_session_itself() {
        //   9 (the session leader)
        //   └ 10 node          ← an MCP server
        //     └ 12 rg          ← something the MCP server ran
        //   └ 11 cargo
        //   13 unrelated, under init
        let procs = vec![
            proc(9, 1, "bwrap"),
            proc(10, 9, "node"),
            proc(11, 9, "cargo"),
            proc(12, 10, "rg"),
            proc(13, 1, "firefox"),
        ];
        let tree = process_tree(&procs, 9);
        let ids: Vec<&str> = tree.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["pid:10", "pid:11", "pid:12"], "breadth first, root out");
        assert!(
            tree.iter().all(|c| c.pid != Some(13)),
            "a process that is not a descendant is not in the tree"
        );
        assert_eq!(tree[0].parent, None, "a direct child hangs off the session");
        assert_eq!(
            tree[2].parent.as_deref(),
            Some("pid:10"),
            "the grandchild hangs off the child, not off the session"
        );
        assert!(tree.iter().all(|c| c.is_open()));
        assert!(tree.iter().all(|c| c.kind == ChildKind::Process));
    }

    #[test]
    fn a_recycled_pid_closing_a_cycle_does_not_hang_the_walk() {
        // /proc is read without a lock, so a self-consistent tree is not
        // guaranteed. A cycle must cost a missing node, never a daemon thread
        // that never returns.
        let procs = vec![proc(10, 9, "a"), proc(9, 10, "b")];
        let tree = process_tree(&procs, 9);
        assert_eq!(tree.len(), 1);
    }

    #[test]
    fn the_tree_is_bounded() {
        let mut procs = vec![proc(9, 1, "leader")];
        for i in 0..(MAX_PROCESS_CHILDREN + 20) {
            procs.push(proc(100 + i as i32, 9, "child"));
        }
        assert_eq!(process_tree(&procs, 9).len(), MAX_PROCESS_CHILDREN);
    }

    #[test]
    fn a_process_name_with_a_bracket_in_it_does_not_shift_every_field() {
        // The parse everybody gets wrong. `comm` is not escaped in
        // /proc/<pid>/stat, so a process that renamed itself to something
        // containing ") 0 (" turns a whitespace split into six fields with
        // plausible wrong values — a ppid that points at another real process,
        // which is a session showing somebody else's processes as its own.
        let honest = "42 (node) S 9 42 42 0 -1 4194304 100 0 0 0 1 2 0 0 20 0                       12 0 777777 1 2 3";
        assert_eq!(parse_stat(honest), stat("node", 9, 777_777));

        let hostile = "42 (evil) 0 (x) S 9 42 42 0 -1 4194304 100 0 0 0 1 2 0 0                        20 0 12 0 777777 1 2 3";
        assert_eq!(
            parse_stat(hostile),
            stat("evil) 0 (x", 9, 777_777),
            "the split must be after the LAST bracket"
        );
    }

    #[test]
    fn a_stat_line_that_is_not_one_is_skipped_rather_than_guessed() {
        assert_eq!(parse_stat(""), None);
        assert_eq!(parse_stat("42 (node) S 9"), None, "too few fields");
        assert_eq!(parse_stat("42 node S 9 42"), None, "no bracket at all");
    }

    #[test]
    fn resident_pages_come_from_the_second_number() {
        assert_eq!(parse_statm("2000 512 300 1 0 400 0"), Some(512));
        assert_eq!(parse_statm(""), None);
    }

    #[test]
    fn a_fixture_proc_reads_as_processes() {
        // The reader against a directory shaped like /proc, so the three file
        // formats are asserted together rather than one at a time.
        let root = std::env::temp_dir().join(format!(
            "apex-graph-proc-{}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("stat"), "cpu 1 2 3
btime 1700000000
").unwrap();
        for (pid, ppid, comm, start, pages) in
            [(9, 1, "bwrap", 0u64, 50u64), (10, 9, "node", 100, 512)]
        {
            let dir = root.join(pid.to_string());
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join("stat"),
                format!(
                    "{pid} ({comm}) S {ppid} 0 0 0 -1 0 0 0 0 0 0 0 0 0 20 0 1 0 {start} 0 0"
                ),
            )
            .unwrap();
            std::fs::write(dir.join("comm"), format!("{comm}
")).unwrap();
            std::fs::write(dir.join("statm"), format!("9999 {pages} 0 0 0 0 0")).unwrap();
        }
        // A directory that is not a pid, and a pid whose files vanished — both
        // are the normal case in a real /proc and neither may abort the read.
        std::fs::create_dir_all(root.join("self")).unwrap();
        std::fs::create_dir_all(root.join("77")).unwrap();

        let mut got = read_processes(&root);
        got.sort_by_key(|p| p.pid);
        let root_kept = root.clone();

        assert_eq!(got.len(), 2, "{got:?}");
        assert_eq!(got[1].pid, 10);
        assert_eq!(got[1].ppid, 9);
        assert_eq!(got[1].comm, "node");
        assert_eq!(got[1].started, 1_700_000_000 + 100 / clock_ticks());

        let mut tree = process_tree(&got, 9);
        assert_eq!(tree.len(), 1);
        assert_eq!(tree[0].label, "node");
        assert_eq!(tree[0].rss_kb, None, "the walk itself reads no statm");

        fill_rss(&root_kept, &mut tree);
        assert_eq!(tree[0].rss_kb, Some(512 * page_size_kb()));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn an_id_made_of_nothing_usable_becomes_anonymous() {
        let mut c = kids();
        subagent_started(&mut c, Some("«»"), Some("Explore"), 100);
        assert_eq!(c[0].id, "anon:unnamed");
    }
}
