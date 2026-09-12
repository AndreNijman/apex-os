//! §13.13: what a worktree owns at Cloudflare, and what destroying it means.
//!
//! ```text
//! apex cf preview plan        # what this worktree owns, and what would go
//! apex cf preview destroy     # the same plan, and nothing else
//! apex cf preview destroy --yes
//! apex project remove <name> --destroy-preview
//! ```
//!
//! §13.13 asks for an optional per-worktree cloud preview — a Worker version, a
//! preview hostname, a D1 database, a KV namespace, temporary secrets — and for
//! those to be destroyed when the worktree is removed, *"after showing a clear
//! destroy plan"*.
//!
//! ## Ownership is read out of the audit trail, not out of a manifest
//!
//! The obvious build is a file each worktree writes when it creates something.
//! It would be wrong here for the reason `apex_secret_core::audit` gives about
//! itself: a second record of what happened is a record that can disagree with
//! what happened. A manifest drifts the first time an agent creates a record
//! through `apex secret use` without telling this command, or the first time
//! somebody deletes one by hand.
//!
//! So ownership is derived from the trail `apex-secretd` already writes. A
//! worktree is its own project root — `git rev-parse --show-toplevel` inside
//! one answers the worktree, which is also why grants are already per-worktree
//! — and every brokered operation records the project it ran for. *What this
//! worktree created* is therefore a question the trail can answer, and it
//! answers it with the same lines an administrator would read.
//!
//! Two consequences, both stated rather than hidden:
//!
//! * **the trail is finite.** [`Reach`] says how many lines this plan was built
//!   from and how far back they go, and says so in the output. A plan that
//!   silently stopped at the daemon's line cap would be a plan that reports a
//!   clean worktree because the trail is busy;
//! * **a worktree at a path a previous one used inherits its trail.** The
//!   project key is a path, and `.apex/worktrees/fix-thing` is a path two
//!   worktrees can have one after the other. Named in the output rather than
//!   solved: solving it would need an identity the trail does not carry.
//!
//! ## Three answers, and none of them may be collapsed into another
//!
//! [`Disposal`] is the shape this repository keeps arriving at. For each thing
//! a worktree owns:
//!
//! * **destroyable** — there is an operation in §13.2's vocabulary that removes
//!   it. Today that is a DNS record and nothing else;
//! * **not destroyable by this build** — it was created here and the vocabulary
//!   has no operation that removes it. An R2 bucket, a Secrets Store secret and
//!   an Access service token are all in this state. The plan **names** them and
//!   says where they have to be removed by hand. A plan that left them out
//!   would say a worktree was cleaned up when a bucket it made is still being
//!   billed;
//! * **nothing to destroy** — a Worker version is superseded rather than
//!   deleted, costs nothing, and Cloudflare keeps it as the thing a rollback
//!   goes back to. Removing it would break §13.7.
//!
//! And for the destroyable kind, two more, because asking the zone is a real
//! request that can fail: [`Disposal::AlreadyGone`] is the zone answering that
//! there is nothing at that name, and [`Disposal::CouldNotRun`] is the zone not
//! answering. A build that read a refused lookup as "already gone" would print
//! a clean plan for a preview that is still up.
//!
//! ## Why the zone is asked at all
//!
//! Because [`AuditLine`] carries the operation and the resource but **not the
//! operation's parameters**, and `cloudflare.dns.delete` needs the record's
//! type. Reading it out of the line's `detail` sentence would be a
//! line-oriented grep against prose somebody may reword. So the trail says
//! *this worktree made a record at this name* and the zone says *what is there
//! now, and of what type* — and the plan is the intersection, which is also
//! what makes a record somebody deleted in the dashboard show as gone rather
//! than as a delete that is about to fail.
//!
//! ## What is not here, and why it could not be
//!
//! §13.13's D1 preview database and KV preview namespace. Neither is creatable
//! or deletable through §13.2's vocabulary — there is no `d1.create`, no
//! `kv.namespace.create` and no delete for either, and
//! [`super::super::cloudflare`]'s binding note explains the shape: those
//! resources are addressed by an id that does not exist until the resource
//! does, so the owner creates them and writes the id into `apex.toml`. A
//! worktree cannot own what nothing here can make. Adding those six operations
//! is a vocabulary expansion and its own task.

use std::collections::BTreeMap;

use anyhow::{bail, Result};
use apex_secret_core::audit::{AuditEvent, AuditLine};
use apex_secret_core::capability::CapabilityRecord;
use apex_secret_core::client::Client;
use apex_secret_core::protocol::{Request, Response};
use serde_json::{json, Map, Value};

use super::SERVICE;

/// How many audit lines a plan asks the daemon for.
///
/// The daemon's own ceiling, so the plan reaches as far as the trail can be
/// read at all. [`Reach`] reports whether that ceiling was hit.
const PLAN_LINES: usize = 1000;

/// A kind of thing a worktree can end up owning at Cloudflare.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    DnsRecord,
    R2Bucket,
    Secret,
    ServiceToken,
    WorkerVersion,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::DnsRecord => "dns-record",
            Kind::R2Bucket => "r2-bucket",
            Kind::Secret => "secret",
            Kind::ServiceToken => "service-token",
            Kind::WorkerVersion => "worker-version",
        }
    }

    /// What to call it in a sentence somebody reads before deleting things.
    pub fn noun(self) -> &'static str {
        match self {
            Kind::DnsRecord => "DNS record",
            Kind::R2Bucket => "R2 bucket",
            Kind::Secret => "Secrets Store secret",
            Kind::ServiceToken => "Access service token",
            Kind::WorkerVersion => "Worker version",
        }
    }
}

/// The operations that make this project the owner of something.
///
/// A table rather than a prefix rule — `cloudflare.*.create` would sweep in
/// anything named `create` later, including operations whose resource is not
/// the thing created.
const CREATES: &[(&str, Kind)] = &[
    ("cloudflare.dns.create", Kind::DnsRecord),
    ("cloudflare.r2.bucket.create", Kind::R2Bucket),
    ("cloudflare.secret.create", Kind::Secret),
    ("cloudflare.access.service-token.create", Kind::ServiceToken),
    ("cloudflare.worker.upload-version", Kind::WorkerVersion),
];

/// The operations that give it back.
const DESTROYS: &[(&str, Kind)] = &[("cloudflare.dns.delete", Kind::DnsRecord)];

/// One thing the trail says this project made and has not given back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Owned {
    pub kind: Kind,
    /// The resource as the operation named it. A NAME, never a URL — the
    /// vocabulary's grammar saw to that before it was ever recorded.
    pub resource: String,
    /// When the creating operation ran, ms since the epoch.
    pub created_ms: u64,
    /// The audit id of that operation, so a reader can find the line.
    pub audit_id: String,
}

/// How far back the trail this plan was built from reaches.
///
/// Its own type because a plan that could not say this would be a plan whose
/// emptiness means two different things — nothing was created, or nothing that
/// was created is still in the window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reach {
    /// Lines the daemon returned for this project.
    pub lines: usize,
    /// The oldest of them, ms since the epoch.
    pub oldest_ms: Option<u64>,
    /// Whether the daemon's line ceiling was hit, so there may be older work
    /// this plan cannot see.
    pub capped: bool,
}

/// What can be done about one owned thing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Disposal {
    /// The vocabulary has an operation that removes it, and these are the
    /// arguments it needs.
    Destroyable {
        operation: &'static str,
        /// `type=A`-style options, in the order they will be sent.
        options: Vec<(String, String)>,
        detail: String,
    },
    /// The far side answered and there is nothing there any more. Somebody
    /// removed it, or it expired. **Not** a refusal and not a failure.
    AlreadyGone(String),
    /// Created here, and nothing in this build removes it.
    NotDestroyable { why: String, fix: String },
    /// There is nothing to destroy, and that is a conclusion rather than an
    /// inability.
    Nothing(String),
    /// The question was not answered. Never an absence and never a refusal —
    /// this thing may well still be up.
    CouldNotRun(String),
}

impl Disposal {
    pub fn as_str(&self) -> &'static str {
        match self {
            Disposal::Destroyable { .. } => "destroyable",
            Disposal::AlreadyGone(_) => "already-gone",
            Disposal::NotDestroyable { .. } => "not-destroyable",
            Disposal::Nothing(_) => "nothing-to-destroy",
            Disposal::CouldNotRun(_) => "could-not-run",
        }
    }

    /// Whether `destroy` will act on this one.
    pub fn is_actionable(&self) -> bool {
        matches!(self, Disposal::Destroyable { .. })
    }

    /// Whether this one leaves something behind that a person has to deal with.
    ///
    /// `CouldNotRun` counts, and that is the point: a preview whose state could
    /// not be established has not been cleaned up.
    pub fn leaves_something(&self) -> bool {
        matches!(
            self,
            Disposal::NotDestroyable { .. } | Disposal::CouldNotRun(_)
        )
    }

    pub fn reason(&self) -> Option<&str> {
        match self {
            Disposal::Destroyable { detail, .. } => Some(detail),
            Disposal::AlreadyGone(w) | Disposal::Nothing(w) | Disposal::CouldNotRun(w) => Some(w),
            Disposal::NotDestroyable { why, .. } => Some(why),
        }
    }
}

/// One line of a destroy plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub owned: Owned,
    pub disposal: Disposal,
}

/// A destroy plan for one worktree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub project: String,
    pub reach: Reach,
    pub items: Vec<Item>,
}

/// What the trail says this project owns, netting creates against destroys.
///
/// Pure, so the netting can be tested against a trail written by hand rather
/// than against a live daemon. The rules, each of which is a way for a plan to
/// be wrong:
///
/// * only an [`AuditEvent::Used`] line counts. A `refused` line is a request
///   that never reached Cloudflare, and treating one as a create would put a
///   resource that does not exist into a destroy plan;
/// * only `exit_code == 0`. The Cloudflare provider sets the code from the HTTP
///   status, so a 4xx is a 1 — and a create that Cloudflare refused created
///   nothing;
/// * a create followed by a destroy of the same name owns nothing, and a create
///   **after** that owns it again. Order decides, so this walks the lines in
///   the order they were written rather than counting them.
pub fn owned(lines: &[AuditLine], project: &str) -> Vec<Owned> {
    let mut held: BTreeMap<(Kind, String), Owned> = BTreeMap::new();
    for line in lines {
        if line.event != AuditEvent::Used || line.exit_code != Some(0) {
            continue;
        }
        if line.project.as_deref() != Some(project) {
            continue;
        }
        if let Some((_, kind)) = DESTROYS.iter().find(|(id, _)| *id == line.operation) {
            held.remove(&(*kind, line.resource.clone()));
            continue;
        }
        if let Some((_, kind)) = CREATES.iter().find(|(id, _)| *id == line.operation) {
            held.insert(
                (*kind, line.resource.clone()),
                Owned {
                    kind: *kind,
                    resource: line.resource.clone(),
                    created_ms: line.ms,
                    audit_id: line.audit_id.clone(),
                },
            );
        }
    }
    held.into_values().collect()
}

/// How far back a set of lines reaches.
pub fn reach(lines: &[AuditLine]) -> Reach {
    Reach {
        lines: lines.len(),
        oldest_ms: lines.first().map(|l| l.ms),
        capped: lines.len() >= PLAN_LINES,
    }
}

/// The disposal of everything that needs no question asked of Cloudflare.
///
/// Split out from the live lookup so the three static verdicts can be tested
/// without a daemon, and so the one kind that costs a request is obvious.
pub fn static_disposal(kind: Kind) -> Option<Disposal> {
    match kind {
        // The one destroyable kind; its verdict needs the zone.
        Kind::DnsRecord => None,
        Kind::R2Bucket => Some(Disposal::NotDestroyable {
            why: "this build has no operation that deletes an R2 bucket — \
                  §13.2's vocabulary offers `cloudflare.r2.bucket.create` and \
                  no counterpart"
                .to_string(),
            fix: "delete it in the Cloudflare dashboard, or with \
                  `wrangler r2 bucket delete`, after checking nothing else \
                  writes to it"
                .to_string(),
        }),
        Kind::Secret => Some(Disposal::NotDestroyable {
            why: "this build has no operation that deletes a Secrets Store \
                  secret — it can create and rotate one and not remove it"
                .to_string(),
            fix: "delete it in the Cloudflare dashboard under Secrets Store"
                .to_string(),
        }),
        Kind::ServiceToken => Some(Disposal::NotDestroyable {
            why: "this build has no operation that deletes an Access service \
                  token. APEX also stored the credential it created, so there \
                  are two halves to remove"
                .to_string(),
            fix: "delete the token in the Cloudflare dashboard under Access, \
                  then `apex secret remove` the credential `apex secret list` \
                  shows for it"
                .to_string(),
        }),
        Kind::WorkerVersion => Some(Disposal::Nothing(
            "a Worker version is superseded rather than deleted. It serves no \
             traffic once something else is deployed, costs nothing, and is \
             what `cloudflare.worker.rollback` goes back to — removing it \
             would break §13.7's rollback path"
                .to_string(),
        )),
    }
}

/// Ask the zone what is at a name, and turn the answer into a verdict.
///
/// Every arm is a different fact and the differences are the reason this is not
/// a `bool`: a name with records is destroyable, a name with none has already
/// been dealt with, and a lookup that was refused or did not run has
/// established nothing at all.
fn live_dns(client: &mut Client, project: &str, name: &str) -> Disposal {
    let mut record = CapabilityRecord::new(SERVICE, "cloudflare.dns.read", name);
    record.project = Some(project.to_string());
    let reply = match client.request(&Request::Use {
        record: Box::new(record),
        body_len: 0,
    }) {
        Ok(reply) => reply,
        Err(e) => {
            return Disposal::CouldNotRun(format!(
                "the secret service could not be asked what is at {name}: {e}"
            ))
        }
    };
    let (exit_code, output) = match reply {
        Response::Performed {
            exit_code, output, ..
        } => (exit_code, output),
        Response::Error { message, .. } => {
            return Disposal::CouldNotRun(format!(
                "reading {name} was refused: {message}. Whatever is there is \
                 still there — grant `cloudflare.dns.read` for this worktree \
                 and run the plan again"
            ))
        }
        other => {
            return Disposal::CouldNotRun(format!(
                "the secret service answered a dns read with {}",
                other.variant()
            ))
        }
    };
    if exit_code != 0 {
        return Disposal::CouldNotRun(format!(
            "cloudflare did not answer what is at {name}: {}",
            one_line(&output)
        ));
    }
    let Ok(body) = serde_json::from_str::<Value>(&output) else {
        return Disposal::CouldNotRun(format!(
            "the reply to a dns read of {name} was not the json envelope"
        ));
    };
    let Some(results) = body.get("result").and_then(Value::as_array) else {
        return Disposal::CouldNotRun(format!(
            "the reply to a dns read of {name} carried no list of records"
        ));
    };
    // The type, per record, and only for records that really are at this name.
    // Cloudflare's list endpoint is filtered by name, but a build that trusted
    // that would delete whatever came back.
    let types: Vec<String> = results
        .iter()
        .filter(|r| r.get("name").and_then(Value::as_str) == Some(name))
        .filter_map(|r| r.get("type").and_then(Value::as_str))
        .map(str::to_string)
        .collect();
    if types.is_empty() {
        return Disposal::AlreadyGone(format!(
            "the zone answered and holds no record at {name}. Something removed \
             it already"
        ));
    }
    Disposal::Destroyable {
        operation: "cloudflare.dns.delete",
        options: types
            .iter()
            .map(|t| ("type".to_string(), t.clone()))
            .collect(),
        detail: format!("{} at {name}", types.join(", ")),
    }
}

/// Build the plan for one worktree.
pub fn plan(project: &str) -> Result<Plan> {
    let mut client = Client::connect()?;
    let entries = match client.call(&Request::Audit {
        lines: PLAN_LINES,
        project: Some(project.to_string()),
    })? {
        Response::Audit { entries } => entries,
        other => bail!("unexpected reply: {}", other.variant()),
    };
    let reach = reach(&entries);
    let held = owned(&entries, project);

    let mut items = Vec::with_capacity(held.len());
    for owned in held {
        let disposal = match static_disposal(owned.kind) {
            Some(d) => d,
            None => live_dns(&mut client, project, &owned.resource),
        };
        items.push(Item { owned, disposal });
    }
    Ok(Plan {
        project: project.to_string(),
        reach,
        items,
    })
}

/// Destroy everything in `plan` that this build can destroy.
///
/// Returns the number that failed. Nothing here decides whether it should run —
/// the caller does, after showing the plan.
pub fn destroy(plan: &Plan) -> Result<usize> {
    let mut client = Client::connect()?;
    let mut failed = 0;
    for item in &plan.items {
        let Disposal::Destroyable {
            operation, options, ..
        } = &item.disposal
        else {
            continue;
        };
        // One request per option set, because a DNS name can carry several
        // records of different types and each delete names one. A build that
        // sent the first type and reported success would leave the others up.
        for (name, value) in options {
            let mut record = CapabilityRecord::new(SERVICE, operation, &item.owned.resource);
            record.project = Some(plan.project.clone());
            record.params.insert(name.clone(), value.clone());
            let reply = client.request(&Request::Use {
                record: Box::new(record),
                body_len: 0,
            });
            match reply {
                Ok(Response::Performed { exit_code: 0, .. }) => {
                    println!("destroyed {} {}={}", item.owned.resource, name, value);
                }
                Ok(Response::Performed { output, .. }) => {
                    failed += 1;
                    eprintln!(
                        "apex cf preview: {} {}={} was NOT destroyed: {}",
                        item.owned.resource,
                        name,
                        value,
                        one_line(&output)
                    );
                }
                Ok(Response::Error { message, .. }) => {
                    failed += 1;
                    eprintln!(
                        "apex cf preview: {} {}={} was NOT destroyed: {message}",
                        item.owned.resource, name, value
                    );
                }
                Ok(other) => {
                    failed += 1;
                    eprintln!(
                        "apex cf preview: {} {}={} — the service answered {}",
                        item.owned.resource,
                        name,
                        value,
                        other.variant()
                    );
                }
                Err(e) => {
                    failed += 1;
                    eprintln!(
                        "apex cf preview: {} {}={} was NOT destroyed: {e}",
                        item.owned.resource, name, value
                    );
                }
            }
        }
    }
    Ok(failed)
}

/// The plan, for a person.
pub fn print(plan: &Plan) {
    println!("worktree:   {}", plan.project);
    println!(
        "trail:      {} line{} for this project{}{}",
        plan.reach.lines,
        if plan.reach.lines == 1 { "" } else { "s" },
        match plan.reach.oldest_ms {
            Some(ms) => format!(", back to {}", stamp(ms)),
            None => String::new(),
        },
        if plan.reach.capped {
            " (the service's line limit was reached, so anything older than \
             that is NOT in this plan)"
        } else {
            ""
        }
    );
    if plan.items.is_empty() {
        println!();
        println!("This worktree's trail shows no Cloudflare resource it created.");
        println!("That is not the same as none existing: see the trail line above,");
        println!("and note that a worktree reusing a previous one's path inherits");
        println!("its trail rather than starting clean.");
        return;
    }
    println!();
    for item in &plan.items {
        println!(
            "{} {}",
            item.owned.kind.noun(),
            if item.owned.resource.is_empty() {
                "(unnamed)"
            } else {
                &item.owned.resource
            }
        );
        println!("    created  {}", stamp(item.owned.created_ms));
        println!("    audit    {}", item.owned.audit_id);
        println!("    {:<9}{}", plan_verb(&item.disposal), item.disposal.as_str());
        if let Some(reason) = item.disposal.reason() {
            for line in wrap(reason, 66) {
                println!("             {line}");
            }
        }
        if let Disposal::NotDestroyable { fix, .. } = &item.disposal {
            for line in wrap(fix, 66) {
                println!("       fix   {line}");
            }
        }
    }
    let actionable = plan.items.iter().filter(|i| i.disposal.is_actionable()).count();
    let left = plan.items.iter().filter(|i| i.disposal.leaves_something()).count();
    println!();
    println!(
        "{actionable} would be destroyed; {left} would be left for you to deal with."
    );
}

fn plan_verb(d: &Disposal) -> &'static str {
    match d {
        Disposal::Destroyable { .. } => "destroy",
        _ => "state",
    }
}

pub fn to_json(plan: &Plan) -> Value {
    let items: Vec<Value> = plan
        .items
        .iter()
        .map(|item| {
            let mut m = Map::new();
            m.insert("kind".into(), Value::from(item.owned.kind.as_str()));
            m.insert("resource".into(), Value::from(item.owned.resource.clone()));
            m.insert("createdMs".into(), Value::from(item.owned.created_ms));
            m.insert("auditId".into(), Value::from(item.owned.audit_id.clone()));
            m.insert("disposal".into(), Value::from(item.disposal.as_str()));
            if let Some(reason) = item.disposal.reason() {
                m.insert("reason".into(), Value::from(reason));
            }
            if let Disposal::NotDestroyable { fix, .. } = &item.disposal {
                m.insert("fix".into(), Value::from(fix.clone()));
            }
            Value::Object(m)
        })
        .collect();
    json!({
        "project": plan.project,
        "reach": {
            "lines": plan.reach.lines,
            "oldestMs": plan.reach.oldest_ms,
            "capped": plan.reach.capped,
        },
        "items": items,
    })
}

/// Milliseconds since the epoch, as a date somebody can read.
///
/// Written out rather than pulled in as a dependency, like everything else in
/// this tree that formats a time.
fn stamp(ms: u64) -> String {
    let secs = ms / 1000;
    let days = secs / 86_400;
    let rest = secs % 86_400;
    let (y, m, d) = civil(days);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}Z",
        rest / 3600,
        (rest % 3600) / 60,
        rest % 60
    )
}

/// Days since the epoch to a civil date. Howard Hinnant's algorithm, which is
/// the one everything else uses and is short enough to write out.
fn civil(days: u64) -> (u64, u64, u64) {
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z % 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// The first line of something that may be a whole API reply.
fn one_line(text: &str) -> String {
    let trimmed = text.trim();
    let first = trimmed.lines().next().unwrap_or("");
    if first.len() > 200 {
        format!("{}…", &first[..200])
    } else {
        first.to_string()
    }
}

/// Wrap on spaces, so a paragraph in a plan does not run off a terminal.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        if !line.is_empty() && line.len() + 1 + word.len() > width {
            out.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        out.push(line);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(
        operation: &str,
        resource: &str,
        project: &str,
        exit_code: Option<i32>,
        event: AuditEvent,
        ms: u64,
    ) -> AuditLine {
        let mut record = CapabilityRecord::new(SERVICE, operation, resource);
        record.project = Some(project.to_string());
        AuditLine {
            ms,
            exit_code,
            ..AuditLine::from_record(&format!("a{ms}"), event, 1000, 1, &record)
        }
    }

    fn used(operation: &str, resource: &str, ms: u64) -> AuditLine {
        line(operation, resource, "/p", Some(0), AuditEvent::Used, ms)
    }

    #[test]
    fn a_create_and_then_a_delete_of_the_same_name_owns_nothing() {
        // And a create AFTER that owns it again. The order of the lines is
        // what decides, which is why this walks them rather than counting
        // them: a build that counted creates and destroys would call three
        // events a net one and offer to delete a record that is not there.
        let trail = vec![
            used("cloudflare.dns.create", "wt.example.com", 1),
            used("cloudflare.dns.delete", "wt.example.com", 2),
        ];
        assert!(owned(&trail, "/p").is_empty(), "{:?}", owned(&trail, "/p"));

        let mut again = trail.clone();
        again.push(used("cloudflare.dns.create", "wt.example.com", 3));
        let held = owned(&again, "/p");
        assert_eq!(held.len(), 1, "{held:?}");
        assert_eq!(held[0].created_ms, 3, "the LAST create is the one recorded");
    }

    #[test]
    fn a_refused_request_and_a_failed_one_create_nothing() {
        // A refusal never reached Cloudflare at all, and a non-zero exit is a
        // Cloudflare that said no — the provider sets the code from the HTTP
        // status. Either one in a destroy plan is an offer to delete something
        // that does not exist, which at best wastes a request and at worst
        // deletes a name somebody else owns.
        let trail = vec![
            line(
                "cloudflare.dns.create",
                "refused.example.com",
                "/p",
                None,
                AuditEvent::Refused,
                1,
            ),
            line(
                "cloudflare.dns.create",
                "failed.example.com",
                "/p",
                Some(1),
                AuditEvent::Used,
                2,
            ),
            used("cloudflare.dns.create", "real.example.com", 3),
        ];
        let held = owned(&trail, "/p");
        assert_eq!(held.len(), 1, "{held:?}");
        assert_eq!(held[0].resource, "real.example.com");
    }

    #[test]
    fn another_project_root_owns_its_own_resources_and_not_this_ones() {
        // The daemon filters by project too, and this is the second half of
        // the same property: a worktree's destroy plan must never offer to
        // delete the main tree's records, and `.apex/worktrees/x` is a path
        // UNDER the project root — so a prefix match would sweep both ways.
        let trail = vec![
            used("cloudflare.dns.create", "mine.example.com", 1),
            line(
                "cloudflare.dns.create",
                "theirs.example.com",
                "/p/.apex/worktrees/other",
                Some(0),
                AuditEvent::Used,
                2,
            ),
        ];
        let held = owned(&trail, "/p");
        assert_eq!(held.len(), 1, "{held:?}");
        assert_eq!(held[0].resource, "mine.example.com");
    }

    #[test]
    fn an_operation_that_creates_nothing_confers_no_ownership() {
        // `dns.update` changes a record somebody else may have made, and
        // `d1.query` and `kv.write` touch resources the project was bound to
        // rather than ones it created. A destroy plan built from "every write"
        // would offer to delete the project's own database.
        let trail = vec![
            used("cloudflare.dns.update", "existing.example.com", 1),
            used("cloudflare.kv.write", "cache/key", 2),
            used("cloudflare.d1.query", "project-db", 3),
            used("cloudflare.worker.deploy", "project", 4),
        ];
        assert!(owned(&trail, "/p").is_empty(), "{:?}", owned(&trail, "/p"));
    }

    #[test]
    fn every_kind_this_build_can_create_has_a_disposal_and_they_are_not_all_the_same() {
        // The list that must not quietly grow a hole. A kind added to CREATES
        // without a disposal would appear in a plan with no verdict at all,
        // and the one arm that is `None` is the one that costs a request.
        for (_, kind) in CREATES {
            match kind {
                Kind::DnsRecord => assert!(
                    static_disposal(*kind).is_none(),
                    "a dns record's verdict has to come from the zone"
                ),
                other => assert!(
                    static_disposal(*other).is_some(),
                    "{} has no disposal",
                    other.as_str()
                ),
            }
        }
        // And the three static answers are three answers. Collapsing
        // `not-destroyable` into `nothing-to-destroy` is the failure this
        // whole module is shaped to avoid: it would report a worktree as
        // cleaned up while an R2 bucket it made is still being billed.
        let bucket = static_disposal(Kind::R2Bucket).expect("a bucket has a verdict");
        let version = static_disposal(Kind::WorkerVersion).expect("a version has a verdict");
        assert_eq!(bucket.as_str(), "not-destroyable");
        assert_eq!(version.as_str(), "nothing-to-destroy");
        assert_ne!(bucket.as_str(), version.as_str());
        assert!(bucket.leaves_something());
        assert!(!version.leaves_something());
        assert!(!bucket.is_actionable() && !version.is_actionable());
    }

    #[test]
    fn a_lookup_that_could_not_run_leaves_something_and_a_gone_record_does_not() {
        // "Permission denied is not absence", in a destroy plan. A refused
        // lookup means whatever is at that name is still there; only the zone
        // answering with nothing means it is gone.
        let refused = Disposal::CouldNotRun("the read was refused".into());
        let gone = Disposal::AlreadyGone("the zone holds nothing there".into());
        assert!(refused.leaves_something());
        assert!(!gone.leaves_something());
        assert!(!refused.is_actionable() && !gone.is_actionable());
        assert_ne!(refused.as_str(), gone.as_str());
    }

    #[test]
    fn the_reach_of_an_empty_trail_is_not_the_reach_of_a_full_one() {
        // A plan with no items means two different things, and this is the
        // field that tells them apart.
        let empty = reach(&[]);
        assert_eq!(empty.lines, 0);
        assert_eq!(empty.oldest_ms, None);
        assert!(!empty.capped);

        let some = reach(&[used("cloudflare.dns.create", "a", 10), used("cloudflare.dns.create", "b", 20)]);
        assert_eq!(some.oldest_ms, Some(10), "the OLDEST, which is the first line");
        assert!(!some.capped);

        let full: Vec<AuditLine> = (0..PLAN_LINES as u64)
            .map(|i| used("cloudflare.dns.create", "a", i + 1))
            .collect();
        assert!(
            reach(&full).capped,
            "a trail at the service's ceiling may be hiding older work"
        );
    }

    #[test]
    fn a_timestamp_reads_as_a_date_and_not_as_a_number() {
        // The plan is read by somebody deciding whether to delete things, and
        // "1757635200000" is not a thing anybody decides on.
        assert_eq!(stamp(0), "1970-01-01 00:00:00Z");
        assert_eq!(stamp(1_757_635_200_000), "2025-09-12 00:00:00Z");
    }
}
