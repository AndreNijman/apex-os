//! The cross-agent handoff packet (§16, roadmap P1-024).
//!
//! §16's reason for existing is quotas and context limits: Claude runs out of
//! room and the work has to continue somewhere else. So a handoff is a
//! mid-session act on a SESSION, and what it has to carry is everything the
//! next agent would otherwise have to reconstruct by reading a terminal it
//! cannot see.
//!
//! §16 names nine things:
//!
//! ```text
//! goal
//! plan
//! changed files
//! worktree
//! test state
//! important transcript summary
//! memory project slug
//! checkpoint
//! capability grants that may be re-requested
//! ```
//!
//! ── FOUR OF THE NINE HAVE NO PRODUCER, AND THAT IS THE DESIGN PROBLEM ───────
//!
//! Five can be established from what the runtime already records: the worktree
//! and the checkpoint sit on `SessionInfo`, the changed files come from the
//! same checkpoint-to-tree comparison `apex agent diff` uses, the transcript
//! comes from `Request::Logs`, and the grants come from `Request::Grants` and
//! `Request::SystemGrants` — two queries, because there are two grant families
//! and they transfer in opposite directions. See below; getting that one
//! backwards is how this module's first draft told an agent to ask a human for
//! authority it had already inherited.
//!
//! The other four cannot, and each is absent for its own reason:
//!
//! * `goal` — the session's opening instruction is passed to the adapter and
//!   ends up inside `SessionInfo.args` as a positional, with nothing marking
//!   which element it is. Recovering it means guessing that the last argument
//!   is a prompt, which is false for every session started without one. The
//!   ARGV is carried instead, as evidence rather than as an answer.
//! * `plan` — nothing in the runtime holds a plan. An agent's plan lives in
//!   its own transcript and its own files.
//! * `test state` — there is no per-worktree test status in this build. APEX
//!   Shell's own help says so in a `todo` block.
//! * `memory project slug` — memory is not a concept in apex-os at all.
//!
//! ── SO AN ABSENT FIELD SAYS WHY IT IS ABSENT ────────────────────────────────
//!
//! The temptation is to fill these in with something plausible: `plan` from the
//! transcript's first heading, `goal` from the last argv element, `test state`
//! from whether a test command exists. Each would be a guess wearing the label
//! of a fact, handed to an agent that has no way to check it — which is worse
//! than an empty field, because the receiving agent would act on it.
//!
//! Every field is therefore `Option`, and every `None` is accompanied by an
//! entry in [`Handoff::unavailable`] naming the field and the reason. The
//! rendered packet prints those reasons under their §16 heading, so the next
//! agent reads "no plan: the runtime does not record one" rather than a blank
//! it might mistake for "there was no plan".
//!
//! ── THE TWO GRANT FAMILIES TRANSFER IN OPPOSITE DIRECTIONS ──────────────────
//!
//! §16 asks for "capability grants that may be re-requested", which reads as
//! one list. It is two, and the difference decides what the receiving agent is
//! allowed to do:
//!
//! * **Project grants** — `Request::Grants`, stored as
//!   `request::Grants { projects: BTreeMap<project root, Vec<grant key>> }`.
//!   `Grants::allows(project, verb)` matches on the project root ALONE: no
//!   session, no expiry, no boot id. A handoff starts the incoming session in
//!   the outgoing session's `cwd`, so it is the same project, so every one of
//!   these ALREADY APPLIES to the new agent. Nothing is re-requested and no
//!   human is asked again.
//! * **System-access grants** — `Request::SystemGrants`, `grant::SystemGrant`,
//!   which carries `session: u32` because §3.3 requires a grant to be "bound
//!   to a concrete agent session". The session it is bound to is the one being
//!   handed off. These do NOT reach the new agent, and re-requesting one costs
//!   a polkit authentication.
//!
//! So the packet reports them separately and says which is which. A single
//! list under §16's heading would be wrong about one of the two families
//! whichever sentence it chose, and wrong in the direction that matters: an
//! agent told it must ask for authority it already holds will not notice, and
//! a human reading the packet to decide whether a handoff is safe would
//! believe a fresh authorisation gate stands where none does.
//!
//! ── WHY IT IS A FILE IN THE PROJECT ─────────────────────────────────────────
//!
//! The receiving session is sandboxed. Under `--sandbox project` the project
//! root is bound writable and the rest of `$HOME` is not merely hidden but
//! absent, so a packet under `$XDG_STATE_HOME` would be handed to an agent that
//! cannot open it. It goes in `.apex/` inside the project, beside
//! `.apex/worktrees`, which `project::ensure_ignored` already keeps out of the
//! user's `.gitignore` by writing to `.git/info/exclude`.
//!
//! Markdown and not JSON for the artifact the agent reads, because the agent
//! reading it is a language model with a file-reading tool and no schema. The
//! struct is serialisable as well, for anything that wants the packet as data.

use serde::{Deserialize, Serialize};

/// Bumped when a field's MEANING changes, never when one is added.
///
/// A reader that finds a version it does not know should say so rather than
/// interpret the fields it recognises: this is a document one agent acts on
/// after another wrote it, so a silent partial read is the failure to avoid.
pub const HANDOFF_VERSION: u32 = 1;

/// A field §16 asks for that this build cannot fill, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Missing {
    /// The §16 field name, spelled as §16 spells it.
    pub field: String,
    /// Why it is empty, in a sentence a person or an agent can act on.
    pub reason: String,
}

impl Missing {
    pub fn new(field: &str, reason: &str) -> Missing {
        Missing {
            field: field.to_string(),
            reason: reason.to_string(),
        }
    }
}

/// §16's portable handoff packet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Handoff {
    pub version: u32,
    /// The session this was taken from.
    pub from_session: u32,
    /// Adapter id the work was being done under (`claude`).
    pub from_agent: String,
    /// Adapter id it is being handed to (`codex`).
    pub to_agent: String,
    /// Unix milliseconds.
    pub created_ms: u64,

    pub project: Option<String>,
    pub worktree: Option<String>,
    /// Where the outgoing session was working.
    pub cwd: String,
    /// The command line the outgoing session was started with.
    ///
    /// Evidence, not a goal. It is here because it is the only durable trace
    /// of the opening instruction, and because a receiving agent can read a
    /// command line and draw its own conclusions — which is different from
    /// this code drawing them for it.
    pub argv: Vec<String>,

    pub goal: Option<String>,
    pub plan: Option<String>,
    pub changed_files: Option<Vec<String>>,
    pub test_state: Option<String>,
    pub transcript: Option<String>,
    pub memory_slug: Option<String>,
    pub checkpoint: Option<String>,

    /// Privilege verbs pre-approved for the PROJECT, which the incoming
    /// session inherits. `None` means the query failed, which is a different
    /// answer from `Some(vec![])`.
    pub project_grants: Option<Vec<String>>,
    /// System-access grants bound to the OUTGOING session, which end with it.
    ///
    /// Rendered as the daemon described them: `Response::SystemGrants` sends a
    /// state word and a sentence per grant because the state depends on the
    /// running kernel's boot id, and a client deriving it could disagree with
    /// the daemon that issued the grant.
    pub system_grants: Option<Vec<String>>,

    /// Every §16 field this build could not establish, with its reason.
    pub unavailable: Vec<Missing>,
}

/// §16's nine field names, in §16's order.
///
/// Exported so a test can assert the rendered packet covers all nine rather
/// than listing eight and missing the one somebody dropped.
pub const FIELDS: &[&str] = &[
    "goal",
    "plan",
    "changed files",
    "worktree",
    "test state",
    "important transcript summary",
    "memory project slug",
    "checkpoint",
    "capability grants that may be re-requested",
];

impl Handoff {
    /// Why `field` is empty, if this packet says.
    pub fn why_missing(&self, field: &str) -> Option<&str> {
        self.unavailable
            .iter()
            .find(|m| m.field == field)
            .map(|m| m.reason.as_str())
    }

    /// The reasons for the four fields no build can fill today.
    ///
    /// A function rather than four call sites, so the wording is in one place
    /// and a future build that gains a producer deletes one entry here instead
    /// of hunting for prose.
    pub fn structural_gaps() -> Vec<Missing> {
        vec![
            Missing::new(
                "goal",
                "The opening instruction is passed to the agent as a positional \
                 argument and is not recorded separately, so it cannot be told \
                 apart from the rest of the command line. The command line is \
                 under `Started as` below.",
            ),
            Missing::new(
                "plan",
                "The runtime does not record a plan. Whatever plan existed is in \
                 the outgoing agent's own transcript and files.",
            ),
            Missing::new(
                "test state",
                "This build has no per-worktree test status. Run the project's \
                 tests yourself and treat the result as the first thing you know.",
            ),
            Missing::new(
                "memory project slug",
                "This runtime has no memory system, so there is no memory slug. \
                 `Project::slug` exists and is a different thing: an identifier \
                 derived from the repository path, with no memory behind it.",
            ),
        ]
    }

    /// The packet as the document the receiving agent reads.
    ///
    /// Every §16 heading appears whether or not it has content, because a
    /// missing heading is indistinguishable from a heading with nothing under
    /// it, and one of those two means "this build cannot tell you".
    pub fn markdown(&self) -> String {
        let mut s = String::new();
        s.push_str(&format!(
            "# Handoff: session {} ({}) to {}\n\n",
            self.from_session, self.from_agent, self.to_agent
        ));
        s.push_str(
            "You are picking up work another agent was doing. Everything below was \
             read out of the APEX agent runtime's own records at the moment of the \
             handoff. Where a section says the runtime cannot supply it, that is a \
             statement about this build and not about the work: do not treat it as \
             \"there was none\".\n\n",
        );
        s.push_str(&format!("packet version {}\n\n", self.version));

        s.push_str("## goal\n\n");
        self.section(&mut s, "goal", self.goal.as_deref());

        s.push_str("## plan\n\n");
        self.section(&mut s, "plan", self.plan.as_deref());

        s.push_str("## changed files\n\n");
        match self.changed_files.as_ref() {
            Some(files) if files.is_empty() => {
                s.push_str("Nothing has changed since the checkpoint below.\n\n");
            }
            Some(files) => {
                for f in files {
                    s.push_str(&format!("- `{f}`\n"));
                }
                s.push('\n');
            }
            None => self.section(&mut s, "changed files", None),
        }

        s.push_str("## worktree\n\n");
        match self.worktree.as_deref() {
            Some(w) => s.push_str(&format!(
                "`{w}`. Work in `{}`, which is where the outgoing session was.\n\n",
                self.cwd
            )),
            None => s.push_str(&format!(
                "This session was not given a worktree. It worked directly in `{}`.\n\n",
                self.cwd
            )),
        }

        s.push_str("## test state\n\n");
        self.section(&mut s, "test state", self.test_state.as_deref());

        s.push_str("## important transcript summary\n\n");
        match self.transcript.as_deref() {
            Some(t) => {
                s.push_str(
                    "The tail of the outgoing session's terminal, verbatim. It is not a \
                     summary: nothing here decided what was important, so read it as \
                     evidence rather than as a briefing.\n\n```\n",
                );
                s.push_str(t);
                if !t.ends_with('\n') {
                    s.push('\n');
                }
                s.push_str("```\n\n");
            }
            None => self.section(&mut s, "important transcript summary", None),
        }

        s.push_str("## memory project slug\n\n");
        self.section(&mut s, "memory project slug", self.memory_slug.as_deref());

        s.push_str("## checkpoint\n\n");
        match self.checkpoint.as_deref() {
            Some(cp) => s.push_str(&format!(
                "`{cp}`. This is the before-state the changed files above are measured \
                 against. `apex agent undo --checkpoint {cp}` returns the project to it.\n\n"
            )),
            None => self.section(&mut s, "checkpoint", None),
        }

        s.push_str("## capability grants that may be re-requested\n\n");
        s.push_str(
            "Two kinds, and they do not transfer the same way. Read both: one of them \
             you have already, and the other you have not.\n\n",
        );

        s.push_str("### pre-approved in this project — you have these\n\n");
        match self.project_grants.as_ref() {
            Some(g) if g.is_empty() => s.push_str(
                "Nothing is pre-approved for this project. A privilege verb you invoke \
                 raises a request and waits for a person to decide it.\n\n",
            ),
            Some(g) => {
                s.push_str(
                    "These privilege verbs are recorded as allowed for the project itself, \
                     not for any one session: the runtime matches a grant on the project \
                     root alone, with no session and no expiry. You are starting in the \
                     same project, so they apply to you now, without a request and without \
                     asking anybody. If that is more than the handoff intended, the person \
                     overseeing it can withdraw one with `apex request revoke`.\n\n",
                );
                for k in g {
                    s.push_str(&format!("- `{k}`\n"));
                }
                s.push('\n');
            }
            None => self.section(&mut s, "project grants", None),
        }

        s.push_str("### held by the outgoing session — you do NOT have these\n\n");
        match self.system_grants.as_ref() {
            Some(g) if g.is_empty() => s.push_str(
                "The outgoing session held no system-access grant, so there is nothing \
                 here you might have expected to inherit.\n\n",
            ),
            Some(g) => {
                s.push_str(
                    "A system-access grant is bound to the session it was issued to (§3.3), \
                     and that is the session being handed off, so none of these covers you. \
                     Re-requesting one costs a human authentication: `apex request ask`. \
                     They are listed so you know what the work needed, not so you assume \
                     you can do it.\n\n",
                );
                for k in g {
                    s.push_str(&format!("- {k}\n"));
                }
                s.push('\n');
            }
            None => self.section(&mut s, "system grants", None),
        }

        s.push_str("## Started as\n\n```\n");
        s.push_str(&self.argv.join(" "));
        s.push_str("\n```\n");

        s
    }

    /// One section's body: the content, or the recorded reason it is absent.
    ///
    /// Never emits an empty section. A blank under a §16 heading is the one
    /// output this whole module exists to avoid, so a field that is both empty
    /// and unexplained says THAT, loudly, rather than saying nothing.
    fn section(&self, s: &mut String, field: &str, value: Option<&str>) {
        match value {
            Some(v) if !v.trim().is_empty() => {
                s.push_str(v.trim_end());
                s.push_str("\n\n");
            }
            _ => match self.why_missing(field) {
                Some(why) => s.push_str(&format!("Not supplied. {why}\n\n")),
                None => s.push_str(
                    "Not supplied, and no reason was recorded. Treat this packet as \
                     incomplete rather than as a report that there was nothing here.\n\n",
                ),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packet() -> Handoff {
        Handoff {
            version: HANDOFF_VERSION,
            from_session: 4,
            from_agent: "claude".into(),
            to_agent: "codex".into(),
            created_ms: 1_757_200_000_000,
            project: Some("/home/t/p".into()),
            worktree: Some("issue-217".into()),
            cwd: "/home/t/p/.apex/worktrees/issue-217".into(),
            argv: vec!["claude".into(), "fix the failing tests".into()],
            goal: None,
            plan: None,
            changed_files: Some(vec!["src/main.rs".into(), "tests/t.rs".into()]),
            test_state: None,
            transcript: Some("$ cargo test\nerror[E0308]: mismatched types".into()),
            memory_slug: None,
            checkpoint: Some("cp-20260908-1".into()),
            project_grants: Some(vec!["install:clang".into()]),
            system_grants: Some(vec!["#4 install (expired 20 minutes ago)".into()]),
            unavailable: Handoff::structural_gaps(),
        }
    }

    #[test]
    fn every_field_section_sixteen_names_appears_in_the_document() {
        // A heading that quietly stopped being rendered would look, to the
        // receiving agent, exactly like a field the outgoing session had
        // nothing to say about.
        let md = packet().markdown();
        for f in FIELDS {
            assert!(md.contains(&format!("## {f}")), "no `## {f}` heading in:\n{md}");
        }
    }

    #[test]
    fn an_empty_field_renders_its_reason_and_never_a_blank() {
        // The defect this module exists to prevent, asserted directly: a
        // receiving agent must never see an empty section it could read as
        // "there was no plan".
        let md = packet().markdown();
        for f in ["goal", "plan", "test state", "memory project slug"] {
            let start = md.find(&format!("## {f}\n")).expect(f);
            let body = &md[start + f.len() + 4..];
            let body = &body[..body.find("\n## ").unwrap_or(body.len())];
            assert!(
                body.trim().len() > 30,
                "the `{f}` section is effectively blank: {body:?}"
            );
            assert!(
                body.contains("Not supplied"),
                "the `{f}` section does not say it is unsupplied: {body:?}"
            );
        }
    }

    #[test]
    fn a_field_with_no_recorded_reason_says_the_packet_is_incomplete() {
        // The case a future edit creates: a field turned into an Option and
        // nobody added its reason. Silence there would be the guess-wearing-a
        // -label failure arriving by omission instead of by commission.
        let mut p = packet();
        p.unavailable.clear();
        let md = p.markdown();
        assert!(md.contains("no reason was recorded"), "{md}");
        assert!(md.contains("incomplete"), "{md}");
    }

    #[test]
    fn an_empty_change_list_is_not_the_same_as_an_unknown_one() {
        // `Some(vec![])` is "the runtime looked and nothing had changed".
        // `None` is "the runtime could not look". Collapsing them would tell
        // the next agent that no work had been done.
        let mut p = packet();
        p.changed_files = Some(vec![]);
        assert!(p.markdown().contains("Nothing has changed since the checkpoint"));

        p.changed_files = None;
        p.unavailable
            .push(Missing::new("changed files", "there is no checkpoint to compare against."));
        let md = p.markdown();
        assert!(md.contains("Not supplied"), "{md}");
        assert!(!md.contains("Nothing has changed since the checkpoint"), "{md}");
    }

    #[test]
    fn a_project_grant_is_reported_as_already_in_force() {
        // THE DEFECT THIS TEST EXISTS FOR. A project grant is matched on the
        // project root alone — `request::Grants::allows` takes no session and
        // no expiry — and a handoff starts the new session in the same
        // project. So it is inherited. The first draft of this module printed
        // "a grant belongs to the session that was given it" over exactly this
        // list, which is the opposite of what the runtime does.
        let md = packet().markdown();
        let start = md
            .find("### pre-approved in this project")
            .expect("no project-grant subsection");
        let body = &md[start..];
        let body = &body[..body.find("### held by").expect("no system subsection")];
        assert!(body.contains("install:clang"), "{body}");
        assert!(
            body.contains("they apply to you now"),
            "a project grant must be reported as in force: {body}"
        );
        assert!(
            !body.contains("do NOT transfer"),
            "the session-bound wording leaked onto the project grants: {body}"
        );
    }

    #[test]
    fn a_system_grant_is_reported_as_not_transferable() {
        // §3.3 binds a system-access grant to a concrete session, and that
        // session is the one ending. A packet that listed these without
        // saying so would read as an inheritance, and the next agent would
        // proceed as if it already had root.
        let md = packet().markdown();
        let start = md.find("### held by the outgoing session").expect("subsection");
        let body = &md[start..];
        assert!(body.contains("bound to the session"), "{body}");
        assert!(body.contains("expired 20 minutes ago"), "{body}");
    }

    #[test]
    fn the_two_grant_families_are_never_collapsed_into_one_list() {
        // A future edit that merges the two sections would have to pick one
        // family's semantics for both, and either choice is a false statement
        // about the other. Asserted structurally so the merge cannot pass.
        let md = packet().markdown();
        assert!(md.contains("## capability grants that may be re-requested"), "{md}");
        assert!(md.contains("### pre-approved in this project"), "{md}");
        assert!(md.contains("### held by the outgoing session"), "{md}");
        let empty_project = {
            let mut p = packet();
            p.project_grants = Some(vec![]);
            p.markdown()
        };
        assert!(
            empty_project.contains("Nothing is pre-approved"),
            "an empty project-grant list must say so rather than vanish: {empty_project}"
        );
        assert!(
            !empty_project.contains("install:clang"),
            "the project list rendered a grant it did not have: {empty_project}"
        );
    }

    #[test]
    fn a_grant_query_that_failed_is_not_reported_as_no_grants() {
        // "Permission denied is not absence", the defect this repository has
        // already shipped once. A grant query the daemon did not answer must
        // not render as "nothing is pre-approved": the receiving agent would
        // read a failed lookup as a checked fact about its own authority.
        let mut p = packet();
        p.project_grants = None;
        p.system_grants = None;
        p.unavailable
            .push(Missing::new("project grants", "the daemon did not answer."));
        p.unavailable
            .push(Missing::new("system grants", "the daemon did not answer."));
        let md = p.markdown();
        assert!(
            !md.contains("Nothing is pre-approved"),
            "a failed query rendered as an absence of grants: {md}"
        );
        assert!(
            !md.contains("held no system-access grant"),
            "a failed query rendered as an absence of grants: {md}"
        );
        assert_eq!(
            md.matches("Not supplied. the daemon did not answer.").count(),
            2,
            "both grant families must report the failure: {md}"
        );
    }

    #[test]
    fn the_transcript_is_labelled_evidence_and_not_a_summary() {
        // §16 asks for an "important transcript summary" and nothing in this
        // runtime can decide what was important. Shipping a tail under that
        // heading without saying it is a tail would be the packet's own most
        // plausible lie.
        let md = packet().markdown();
        assert!(md.contains("It is not a summary"), "{md}");
    }

    #[test]
    fn the_packet_round_trips_through_json() {
        let p = packet();
        let text = serde_json::to_string(&p).expect("serialise");
        let back: Handoff = serde_json::from_str(&text).expect("round-trip");
        assert_eq!(back, p);
    }

    #[test]
    fn the_four_structural_gaps_are_the_four_with_no_producer() {
        // If a producer ever lands, this test is what makes somebody delete
        // the corresponding entry rather than leave a packet claiming a field
        // is unobtainable when it is now sitting on the record.
        let gaps = Handoff::structural_gaps();
        let names: Vec<&str> = gaps.iter().map(|m| m.field.as_str()).collect();
        assert_eq!(names, ["goal", "plan", "test state", "memory project slug"]);
        for m in Handoff::structural_gaps() {
            assert!(
                m.reason.len() > 40 && m.reason.ends_with('.'),
                "a reason a person cannot act on is not a reason: {m:?}"
            );
        }
    }

    #[test]
    fn a_session_with_no_worktree_says_where_it_worked_instead() {
        let mut p = packet();
        p.worktree = None;
        let md = p.markdown();
        assert!(md.contains("was not given a worktree"), "{md}");
        assert!(md.contains("/home/t/p/.apex/worktrees/issue-217"), "{md}");
    }
}
