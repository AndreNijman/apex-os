//! `apex schema` — which schema each persistent store is on, and what a
//! rollback would do to it. Roadmap §25's user-facing half.
//!
//! ## Why this is a verb and not a paragraph in the manual
//!
//! `bootc rollback` is one command and it is the right answer to most bad
//! updates. What it does not undo is everything under `/etc`, `/var` and the
//! user's home, so the honest question before running it is "which of my files
//! has the new build already changed, and can the old one still read them".
//! Nothing on the machine could answer that. This does.
//!
//! ## `status` never writes and `migrate` is a dry run
//!
//! `status` opens files read-only, so it is safe to poll and safe to run while
//! deciding. `migrate` prints its plan and changes nothing unless it is given
//! `--commit`, the same shape `apex recover reset` uses for the same reason:
//! the operations here are the ones a user would most want to have read first.
//!
//! ## The stores this does not cover, and why the gap is named
//!
//! `apexd-core` is the type library the CLI and the daemon share. The agent
//! runtime's session records, its grant table, the secret broker's store and
//! both audit trails live in `apex-agent-core` and `apex-secret-core`, neither
//! of which depends on `apexd-core`. Wiring them means either a new dependency
//! edge between crates that are deliberately siblings, or moving the framework
//! into a crate below both. That is a decision somebody should make on purpose,
//! so `status` lists those stores as unmanaged rather than letting them be
//! absent from a report that looks complete.

use std::collections::BTreeMap;
use std::path::PathBuf;

use apexd_core::migrate::{self, Authored, FileOutcome, Plan, Row, Store};
use clap::Subcommand;
use serde_json::{json, Value};

/// Stores §25 names that this build cannot manage yet, with the reason.
///
/// A list, in the product, rather than a gap: a report that silently omitted
/// the secret broker's grant table would read as "everything is covered".
const UNMANAGED: &[(&str, &str, &str)] = &[
    (
        "agent-sessions",
        "~/.local/state/apex/agent/sessions/<id>.json",
        "apex-agent-core does not depend on apexd-core",
    ),
    (
        "agent-grants",
        "~/.local/state/apex/agent/grants.json",
        "apex-agent-core does not depend on apexd-core",
    ),
    (
        "privilege-audit",
        "~/.local/state/apex/agent/privilege-audit.jsonl",
        "append-only JSONL; a line carries its own shape, so a document version does not fit",
    ),
    (
        "secret-store",
        "/var/lib/apex-secretd/users/<uid>/",
        "root-owned, and apex-secret-core does not depend on apexd-core",
    ),
    (
        "package-extensions",
        "/var/lib/apex/pkg/state.json",
        "written by apex-pkg in shell, which carries its own pkg_compat_level",
    ),
];

#[derive(Subcommand)]
pub enum SchemaCmd {
    /// Which schema each persistent store is on, and what a rollback does to it.
    ///
    /// Read-only. It opens each store's file, reads the version key, and
    /// closes it — no daemon, no root, no subprocess, so it is safe to run
    /// while deciding whether to roll back.
    Status {
        /// Emit machine-readable JSON instead of a report.
        #[arg(long)]
        json: bool,
    },
    /// Run machine-written stores forward to this build's schema.
    ///
    /// A dry run unless `--commit`. Each file that changes is copied to
    /// `<path>.pre-v<version>` first, and a file written by a NEWER APEX is
    /// reported and left alone — overwriting it would destroy the only copy of
    /// what that build recorded.
    Migrate {
        /// Actually write. Without it, nothing on disk changes.
        #[arg(long)]
        commit: bool,
        /// Emit machine-readable JSON instead of a report.
        #[arg(long)]
        json: bool,
    },
}

/// Where each declared store lives on this machine.
///
/// The framework resolves no paths on purpose, so this is the one place that
/// knows a store id maps to a file, and it uses the same resolvers the stores'
/// own readers use rather than a second interpretation of the base directories.
fn paths() -> BTreeMap<&'static str, PathBuf> {
    let mut m = BTreeMap::new();
    m.insert("blueprint", crate::blueprint::user_blueprint_path());
    m.insert("blueprint-state", crate::blueprint::applied_state_path());
    m.insert("tasks", crate::task::tasks_path());
    m
}

/// Every task's state file. Not in [`paths`] because the store is one file per
/// task, and the report has to say which ones.
fn task_state_files() -> Vec<PathBuf> {
    let dir = apex_agent_core::paths::state_home().join("apex/tasks");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut out: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    out.sort();
    out
}

fn plan_word(row: &Row) -> String {
    match (&row.found, &row.reason, &row.plan) {
        (None, None, _) => "no file yet".to_string(),
        // A store this build could not read is not a store that is absent, and
        // it is certainly not a store that is up to date.
        (None, Some(why), _) => format!("unavailable — {why}"),
        (Some(v), _, Some(Plan::UpToDate)) => format!("schema {v}, current"),
        (Some(v), _, Some(Plan::Forward(steps))) => {
            format!("schema {v}, {} step(s) behind schema {}", steps.len(), row.current)
        }
        (Some(v), _, Some(Plan::TooNew { current, .. })) => {
            format!("schema {v}, NEWER than the schema {current} this build reads")
        }
        (Some(v), _, Some(Plan::NoRoute { current, .. })) => {
            format!("schema {v}, and no migration reaches schema {current}")
        }
        (Some(v), _, None) => format!("schema {v}"),
    }
}

fn row_json(row: &Row) -> Value {
    let mut m = serde_json::Map::new();
    m.insert("id".into(), json!(row.id));
    m.insert("what".into(), json!(row.what));
    m.insert("path".into(), json!(row.path));
    m.insert("found".into(), row.found.map(Value::from).unwrap_or(Value::Null));
    if let Some(r) = &row.reason {
        m.insert("reason".into(), json!(r));
    }
    m.insert("current".into(), json!(row.current));
    m.insert("rollback".into(), json!(row.rollback.as_str()));
    m.insert(
        "rollbackMeans".into(),
        json!(migrate::rollback_note(
            migrate::store(row.id).expect("a row's id names a declared store")
        )),
    );
    m.insert(
        "authored".into(),
        json!(match row.authored {
            Authored::Human => "human",
            Authored::Machine => "machine",
        }),
    );
    m.insert("state".into(), json!(plan_word(row)));
    Value::Object(m)
}

fn rows() -> Vec<Row> {
    let p = paths();
    let mut out = migrate::survey(&p);
    if let Some(store) = migrate::store("task-state") {
        for f in task_state_files() {
            out.push(migrate::inspect(store, &f));
        }
    }
    out
}

fn status(as_json: bool) -> i32 {
    let rows = rows();
    if as_json {
        let doc = json!({
            "stores": rows.iter().map(row_json).collect::<Vec<_>>(),
            "unmanaged": UNMANAGED
                .iter()
                .map(|(id, path, why)| json!({"id": id, "path": path, "reason": why}))
                .collect::<Vec<_>>(),
        });
        println!("{}", serde_json::to_string_pretty(&doc).unwrap_or_default());
        return 0;
    }

    println!("Persistent state — what does NOT roll back with the image\n");
    println!("{:<18}{:<14}STATE", "STORE", "WRITTEN BY");
    for r in &rows {
        let who = match r.authored {
            Authored::Human => "you",
            Authored::Machine => "apex",
        };
        println!("{:<18}{:<14}{}", r.id, who, plan_word(r));
        println!("{:18}{:14}{}", "", "", r.path);
    }

    println!("\nIf you roll back to the previous deployment");
    for s in migrate::STORES {
        println!("  {:<16} {}", s.id, migrate::rollback_note(s));
    }

    println!("\nNot managed by this framework yet");
    for (id, path, why) in UNMANAGED {
        println!("  {id}");
        println!("      {path}");
        println!("      {why}");
    }
    0
}

fn migrate_all(commit: bool, as_json: bool) -> i32 {
    let mut results: Vec<Value> = Vec::new();
    let mut failed = false;

    let mut targets: Vec<(&'static Store, PathBuf)> = Vec::new();
    for (id, path) in paths() {
        if let Some(s) = migrate::store(id) {
            targets.push((s, path));
        }
    }
    if let Some(s) = migrate::store("task-state") {
        for f in task_state_files() {
            targets.push((s, f));
        }
    }

    for (store, path) in targets {
        // A file a person typed is never rewritten here. It is migrated in
        // memory every time it is read, so there is nothing to do and nothing
        // to report but that.
        if store.authored == Authored::Human {
            results.push(json!({
                "store": store.id,
                "path": path.display().to_string(),
                "action": "none",
                "why": "written by you; migrated in memory on every read, never rewritten",
            }));
            continue;
        }
        // The dry run reads; the commit delegates. `migrate_file` is the ONLY
        // thing that decides what happens to a file that is going to be
        // written, because two places deciding is two places to fix — and a
        // second guard here would hide a hole in the first one. It did: an
        // earlier version answered "left alone" for a newer file before
        // `migrate_file` was called at all, so a framework that clobbered such
        // a file passed the suite.
        let action = if !commit {
            let row = migrate::inspect(store, &path);
            match (&row.found, &row.plan) {
                (None, _) if row.reason.is_none() => json!({"action": "none", "why": "no file"}),
                (None, _) => json!({"action": "refused", "why": row.reason}),
                (Some(_), Some(Plan::UpToDate)) => json!({"action": "none", "why": "current"}),
                (Some(v), Some(Plan::TooNew { current, .. })) => json!({
                    "action": "left alone",
                    "why": format!("schema {v} was written by a newer APEX, which reads schema {current}"),
                }),
                (Some(_), Some(Plan::Forward(steps))) => json!({
                    "action": "would migrate",
                    "steps": steps.iter().map(|x| x.summary).collect::<Vec<_>>(),
                }),
                (Some(v), Some(Plan::NoRoute { current, .. })) => json!({
                    "action": "refused",
                    "why": format!("no migration reaches schema {current} from schema {v}"),
                }),
                (Some(v), None) => json!({"action": "none", "why": format!("schema {v}")}),
            }
        } else {
            match migrate::migrate_file(store, &path) {
                Ok(FileOutcome::Migrated { outcome, checkpoint }) => json!({
                    "action": "migrated",
                    "from": outcome.from,
                    "to": outcome.to,
                    "steps": outcome.summaries,
                    "checkpoint": checkpoint.display().to_string(),
                }),
                Ok(FileOutcome::UpToDate(v)) => json!({"action": "none", "why": format!("schema {v}, current")}),
                Ok(FileOutcome::Absent) => json!({"action": "none", "why": "no file"}),
                Ok(FileOutcome::TooNew { message, .. }) => {
                    json!({"action": "left alone", "why": message})
                }
                Err(e) => {
                    failed = true;
                    json!({"action": "failed", "why": e})
                }
            }
        };
        let mut m = action.as_object().cloned().unwrap_or_default();
        m.insert("store".into(), json!(store.id));
        m.insert("path".into(), json!(path.display().to_string()));
        results.push(Value::Object(m));
    }

    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({"commit": commit, "results": results}))
                .unwrap_or_default()
        );
    } else {
        if !commit {
            println!("Dry run. Nothing has been written. Add --commit to do it.\n");
        }
        for r in &results {
            let store = r["store"].as_str().unwrap_or("?");
            let action = r["action"].as_str().unwrap_or("?");
            println!("{store:<18}{action}");
            println!("{:18}{}", "", r["path"].as_str().unwrap_or(""));
            if let Some(why) = r["why"].as_str() {
                for line in why.lines() {
                    println!("{:18}{line}", "");
                }
            }
            if let Some(steps) = r["steps"].as_array() {
                for s in steps {
                    println!("{:18}- {}", "", s.as_str().unwrap_or(""));
                }
            }
            if let Some(ck) = r["checkpoint"].as_str() {
                println!("{:18}the file as it was: {ck}", "");
            }
        }
    }
    if failed {
        1
    } else {
        0
    }
}

pub fn main(cmd: SchemaCmd) -> i32 {
    match cmd {
        SchemaCmd::Status { json } => status(json),
        SchemaCmd::Migrate { commit, json } => migrate_all(commit, json),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_declared_store_has_somewhere_to_look() {
        // A store in the registry with no path resolver never appears in the
        // report, which is the quiet way for this command to stop covering
        // something. `task-state` is the one exception and is handled by its
        // own directory walk, so it is named here rather than exempted by a
        // rule that would also hide the next omission.
        let p = paths();
        for s in migrate::STORES {
            assert!(
                p.contains_key(s.id) || s.id == "task-state",
                "{} is declared but `apex schema status` would never look for it",
                s.id
            );
        }
    }

    #[test]
    fn the_unmanaged_list_names_the_security_relevant_stores() {
        // The gap is in the product on purpose. If a future change wires one of
        // these, the entry goes away and this test says so; if somebody deletes
        // the list to make the report look complete, it fails.
        let ids: Vec<&str> = UNMANAGED.iter().map(|(id, _, _)| *id).collect();
        for want in ["agent-grants", "secret-store", "privilege-audit"] {
            assert!(ids.contains(&want), "{want} is missing from the unmanaged list");
        }
        for (id, path, why) in UNMANAGED {
            assert!(!id.is_empty() && !path.is_empty());
            assert!(why.len() > 20, "{id}: the reason has to be a reason");
            assert!(
                migrate::store(id).is_none(),
                "{id} is both declared and listed as unmanaged"
            );
        }
    }

    #[test]
    fn an_unreadable_store_is_never_described_as_current() {
        // The report's own version of the EACCES rule. `plan_word` is what a
        // user reads, so it is where "I could not look" must not become "it is
        // fine".
        let row = Row {
            id: "blueprint",
            what: "x",
            path: "/etc/apex/blueprint.toml".into(),
            found: None,
            reason: Some("Permission denied".into()),
            current: 1,
            rollback: migrate::Rollback::RefuseAndExplain,
            authored: Authored::Human,
            plan: None,
        };
        let w = plan_word(&row);
        assert!(w.contains("unavailable"), "{w}");
        assert!(w.contains("Permission denied"), "{w}");
        assert!(!w.contains("current"), "{w}");
    }

    #[test]
    fn a_store_from_the_future_reads_as_newer_not_as_behind() {
        let row = Row {
            id: "blueprint",
            what: "x",
            path: "x".into(),
            found: Some(9),
            reason: None,
            current: 1,
            rollback: migrate::Rollback::RefuseAndExplain,
            authored: Authored::Human,
            plan: Some(Plan::TooNew { found: 9, current: 1 }),
        };
        let w = plan_word(&row);
        assert!(w.contains("NEWER"), "{w}");
        assert!(!w.contains("behind"), "{w}");
    }

    #[test]
    fn a_missing_file_is_not_confused_with_a_refused_one() {
        let absent = Row {
            id: "blueprint",
            what: "x",
            path: "x".into(),
            found: None,
            reason: None,
            current: 1,
            rollback: migrate::Rollback::RefuseAndExplain,
            authored: Authored::Human,
            plan: None,
        };
        assert_eq!(plan_word(&absent), "no file yet");
    }
}
