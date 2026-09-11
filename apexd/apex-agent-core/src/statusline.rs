//! Claude's status line, as a telemetry source (§P1-021).
//!
//! Claude runs a program of the user's choosing once a minute and prints what
//! it writes under the prompt. The document it hands that program on stdin is
//! the only place several things exist: which model the session is actually
//! on, how full its context window is, and — for a Pro or Max account — how
//! much of the five-hour and seven-day rate limit windows is gone and when
//! each one resets.
//!
//! None of that reaches the daemon any other way. The hook bridge carries
//! lifecycle events; it has no model, no context and no limits. So §P1-021
//! asks for the status line, and this module is the parse.
//!
//! ## The one requirement that shapes everything here
//!
//! "without breaking terminal statusline". A user's own status line must keep
//! working exactly as it did, and that is not automatic — the settings file
//! APEX passes with `--settings` sits SECOND in Claude's precedence chain,
//! above the project's files and above `~/.claude/settings.json`. List keys
//! like `hooks` combine across sources, which is why the hook bridge can add
//! itself and leave the user's hooks alone. `statusLine` is an object, and the
//! documentation does not say objects merge; the precedence chain says the
//! highest source wins outright. So an APEX overlay that names a `statusLine`
//! REPLACES the user's.
//!
//! Two consequences, and both are load-bearing:
//!
//! * `apex agent statusline` must run the user's own command and copy its
//!   output through, so what appears under the prompt is byte-for-byte what
//!   appeared before. [`chain`] is that, and [`user_status_line`] is how the
//!   command is found.
//! * the presentation keys have to be carried across too. `refreshInterval`,
//!   `padding` and `hideVimModeIndicator` live in the same object, so an
//!   overlay that omits them silently changes how often the line updates and
//!   how it is spaced. [`overlay`] copies them.
//!
//! ## And what it must never do
//!
//! Write to the user's settings file. The point of `--settings` is that APEX
//! adds a source and owns nothing; a runtime that edited `~/.claude` to make
//! its own integration work would leave that edit behind after it was
//! uninstalled.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// What one status-line document says about a session.
///
/// Every field optional, for the reason [`crate::hook::Payload`]'s are: this
/// deserialises a document produced by a program that ships far more often
/// than APEX does, and a renamed key must cost one number in the Agent Center
/// rather than the whole telemetry feed.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Telemetry {
    /// `model.display_name` — "Opus 4.5", not `claude-opus-4-5`. The name the
    /// user chose it by.
    #[serde(default)]
    pub model: Option<String>,
    /// `context_window.used_percentage`, 0-100.
    #[serde(default)]
    pub context_pct: Option<f64>,
    /// `rate_limits.five_hour.used_percentage`, 0-100.
    ///
    /// ACCOUNT-wide, not per session. Six sessions on one login report the
    /// same number, and a client that renders it per row is repeating one fact
    /// six times. Recorded per session anyway, because the freshest
    /// observation is the one to show and only the session knows when it
    /// observed.
    #[serde(default)]
    pub five_hour_pct: Option<f64>,
    /// Unix seconds at which that window resets.
    #[serde(default)]
    pub five_hour_reset: Option<u64>,
    #[serde(default)]
    pub seven_day_pct: Option<f64>,
    #[serde(default)]
    pub seven_day_reset: Option<u64>,
    /// The git branch the session is on.
    ///
    /// NOT in the payload — Claude sends `workspace.current_dir` and
    /// `workspace.project_dir` and no branch at all, which is why every
    /// hand-written status line shells out to git for it. Read here from
    /// `.git/HEAD` instead: it is one file, it is the answer, and running git
    /// once a minute per session to learn something a symref already says is
    /// a process spawn nobody needs.
    #[serde(default)]
    pub branch: Option<String>,
    /// Unix seconds at which this was observed.
    ///
    /// The field that keeps the rest honest. A status line runs on a timer and
    /// on events, so an observation can be a minute old or an hour old
    /// depending on whether the session is doing anything, and a client
    /// showing "context 87%" with no idea when that was true is showing a
    /// number it cannot defend. Set by the publisher, never by the daemon.
    pub observed_at: u64,
}

/// Longest string kept out of the payload.
///
/// A model name and a branch name are both short, and both come off documents
/// APEX does not control — one from Claude, one from a repository whose branch
/// names anybody can write.
const MAX_FIELD: usize = 64;

impl Telemetry {
    /// True when the document said nothing worth recording.
    ///
    /// `observed_at` alone is not telemetry. Publishing an empty one would
    /// rewrite the session record once a minute per session for nothing, and
    /// would make "we have never heard from the status line" indistinguishable
    /// from "the status line is running and has nothing to say".
    pub fn is_empty(&self) -> bool {
        self.model.is_none()
            && self.context_pct.is_none()
            && self.five_hour_pct.is_none()
            && self.seven_day_pct.is_none()
            && self.branch.is_none()
    }
}

/// Read a status-line payload.
///
/// Pure over the parsed JSON, so the whole mapping is asserted in this file
/// rather than through a live agent.
pub fn parse(doc: &serde_json::Value, observed_at: u64) -> Telemetry {
    let limits = doc.get("rate_limits");
    let window = |name: &str| limits.and_then(|l| l.get(name));

    Telemetry {
        model: doc
            .get("model")
            .and_then(|m| m.get("display_name"))
            .and_then(|v| v.as_str())
            .and_then(clamp),
        context_pct: doc
            .get("context_window")
            .and_then(|c| c.get("used_percentage"))
            .and_then(percent),
        five_hour_pct: window("five_hour")
            .and_then(|w| w.get("used_percentage"))
            .and_then(percent),
        five_hour_reset: window("five_hour")
            .and_then(|w| w.get("resets_at"))
            .and_then(seconds),
        seven_day_pct: window("seven_day")
            .and_then(|w| w.get("used_percentage"))
            .and_then(percent),
        seven_day_reset: window("seven_day")
            .and_then(|w| w.get("resets_at"))
            .and_then(seconds),
        // Not from the document. See the field.
        branch: doc
            .get("workspace")
            .and_then(|w| w.get("current_dir"))
            .and_then(|v| v.as_str())
            .and_then(|d| git_branch(Path::new(d))),
        observed_at,
    }
}

/// A percentage, clamped to the range it claims to be in.
///
/// Clamped rather than passed through, because it drives a progress bar in the
/// Agent Center and a value of 140 would draw outside its own track. A value
/// that is not a number at all is absent, not zero: "we do not know" and "none
/// used" are different facts and the bar draws them differently.
fn percent(v: &serde_json::Value) -> Option<f64> {
    let n = v.as_f64()?;
    if !n.is_finite() {
        return None;
    }
    Some(n.clamp(0.0, 100.0))
}

/// A unix timestamp, as a number or as a string holding one.
///
/// Both, because the field is read off a document APEX does not own and the
/// cost of the second branch is three lines. Anything else — an RFC 3339
/// string, say — is `None` rather than a guess: a reset time that is wrong is
/// worse than a reset time that is missing, because the missing one is
/// visible.
fn seconds(v: &serde_json::Value) -> Option<u64> {
    if let Some(n) = v.as_u64() {
        return Some(n);
    }
    if let Some(f) = v.as_f64() {
        if f.is_finite() && f >= 0.0 {
            return Some(f as u64);
        }
    }
    v.as_str()?.trim().parse().ok()
}

fn clamp(raw: &str) -> Option<String> {
    let kept: String = raw
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_FIELD)
        .collect();
    let kept = kept.trim().to_string();
    if kept.is_empty() {
        None
    } else {
        Some(kept)
    }
}

/// The branch a directory is on, from `.git/HEAD`.
///
/// Walks up, because a status line's `current_dir` is wherever the user has
/// cd'd to and that is usually not the repository root. A detached HEAD holds
/// a hash rather than a symref and is reported as `None`: "detached" is not a
/// branch, and printing a truncated sha where every other row shows a name
/// invites the reader to treat it as one.
///
/// One file read per status-line run, against `git branch --show-current`'s
/// process spawn. That is the whole reason this is not shelled out: the status
/// line runs once a minute per session, and a fleet of six would be six
/// process spawns a minute to read a symref.
pub fn git_branch(dir: &Path) -> Option<String> {
    let mut here = dir;
    loop {
        let git = here.join(".git");
        // A worktree's `.git` is a FILE holding `gitdir: <path>`, which is why
        // this reads a metadata check rather than assuming a directory. §7
        // makes a worktree the unit of parallel work, so this is the common
        // case here rather than an exotic one.
        let head = if git.is_dir() {
            Some(git.join("HEAD"))
        } else if git.is_file() {
            std::fs::read_to_string(&git)
                .ok()
                .and_then(|t| {
                    t.lines()
                        .find_map(|l| l.strip_prefix("gitdir:"))
                        .map(|p| PathBuf::from(p.trim()).join("HEAD"))
                })
        } else {
            None
        };
        if let Some(head) = head {
            let text = std::fs::read_to_string(head).ok()?;
            let name = text.trim().strip_prefix("ref: refs/heads/")?;
            return clamp(name);
        }
        here = here.parent()?;
    }
}

// ---------------------------------------------------------------------------
// The user's own status line
// ---------------------------------------------------------------------------

/// A `statusLine` object out of a settings file.
#[derive(Debug, Clone, PartialEq)]
pub struct UserStatusLine {
    /// The command Claude would have run. Absent for a `statusLine` of a type
    /// this build does not know — `type` is `command` today and a future one
    /// must not be run as a shell string.
    pub command: Option<String>,
    /// The presentation keys, carried across verbatim.
    pub presentation: serde_json::Map<String, serde_json::Value>,
}

/// The keys that describe how the line is DRAWN rather than what draws it.
///
/// Copied into the overlay unchanged. They live in the same object as
/// `command`, so an overlay that names the object and omits these silently
/// changes the user's refresh cadence and spacing — which is exactly the
/// "without breaking the terminal statusline" criterion, one level below where
/// anybody would look for it.
const PRESENTATION_KEYS: &[&str] = &["refreshInterval", "padding", "hideVimModeIndicator"];

/// Where Claude looks for settings, highest precedence first.
///
/// Managed policy is deliberately absent: it outranks `--settings` and APEX
/// cannot override it, so a `statusLine` there is one APEX never replaces and
/// never needs to chain to.
pub fn settings_sources(home: &Path, project: Option<&Path>) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(p) = project {
        out.push(p.join(".claude/settings.local.json"));
        out.push(p.join(".claude/settings.json"));
    }
    out.push(home.join(".claude/settings.json"));
    out
}

/// The `statusLine` the user would have got, from the highest-precedence file
/// that names one.
///
/// `None` when nobody has configured one, which is the common case and is not
/// a failure: [`chain`] then prints nothing and Claude shows no status line,
/// exactly as it did before APEX was involved.
pub fn user_status_line(home: &Path, project: Option<&Path>) -> Option<UserStatusLine> {
    for path in settings_sources(home, project) {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(doc) = serde_json::from_str::<serde_json::Value>(&text) else {
            // A settings file that will not parse is one Claude ignores too.
            continue;
        };
        let Some(status) = doc.get("statusLine").and_then(|v| v.as_object()) else {
            continue;
        };
        let mut presentation = serde_json::Map::new();
        for key in PRESENTATION_KEYS {
            if let Some(v) = status.get(*key) {
                presentation.insert((*key).to_string(), v.clone());
            }
        }
        let command = match status.get("type").and_then(|v| v.as_str()) {
            Some("command") => status
                .get("command")
                .and_then(|v| v.as_str())
                .map(str::to_string),
            _ => None,
        };
        return Some(UserStatusLine {
            command,
            presentation,
        });
    }
    None
}

/// The `statusLine` object APEX's settings overlay carries.
///
/// Points at `apex agent statusline`, and keeps the user's presentation keys.
/// The command is quoted the same way the hook commands are, and for the same
/// reason: it is a shell string, and the daemon's own path can contain a
/// space.
pub fn overlay(apex_quoted: &str, user: Option<&UserStatusLine>) -> serde_json::Value {
    let mut obj = serde_json::Map::new();
    obj.insert("type".into(), serde_json::json!("command"));
    obj.insert(
        "command".into(),
        serde_json::json!(format!("{apex_quoted} agent statusline")),
    );
    if let Some(u) = user {
        for (k, v) in &u.presentation {
            obj.insert(k.clone(), v.clone());
        }
    }
    serde_json::Value::Object(obj)
}

/// Whether a command is APEX's own, so [`chain`] cannot call itself forever.
///
/// A user who has read this file and pointed their own `statusLine` at
/// `apex agent statusline` would otherwise get a fork bomb one status-line
/// refresh at a time. Matched on the two words rather than on a path, because
/// the binary can be anywhere and `command` is a shell string.
pub fn is_apex_statusline(command: &str) -> bool {
    let flat = command.split_whitespace().collect::<Vec<_>>().join(" ");
    flat.contains("agent statusline")
}

/// Run the user's own status-line command with the payload it expected, and
/// return what it printed.
///
/// `None` when there is nothing to run, when it is APEX's own command, or when
/// it could not be started — all three print nothing, which is what Claude
/// showed before any of this existed.
///
/// stderr is inherited rather than captured. Claude sends a status line's
/// stderr to `--debug` and shows only stdout, so a script that diagnoses
/// itself there keeps working; capturing it would silently swallow the one
/// channel its author chose.
pub fn chain(command: &str, payload: &[u8]) -> Option<Vec<u8>> {
    use std::io::Write;
    use std::process::{Command, Stdio};

    if is_apex_statusline(command) {
        return None;
    }
    // Through a shell, because that is how Claude runs it: the configured
    // command is a shell string, `~/.claude/statusline.sh` relies on tilde
    // expansion, and a command with arguments or a pipe in it is normal.
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(command)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .ok()?;
    if let Some(mut stdin) = child.stdin.take() {
        // A script that ignores its stdin closes the pipe, and writing to it
        // then raises EPIPE. That is the script working, not a failure, so the
        // result is discarded rather than propagated.
        let _ = stdin.write_all(payload);
    }
    let out = child.wait_with_output().ok()?;
    Some(out.stdout)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc() -> serde_json::Value {
        // The shape Claude actually sends, from the documented schema and from
        // a working status line on this machine.
        serde_json::json!({
            "hook_event_name": "Status",
            "session_id": "abc",
            "cwd": "/tmp",
            "version": "2.1.0",
            "model": { "id": "claude-opus-4-5", "display_name": "Opus 4.5" },
            "workspace": { "current_dir": "/tmp", "project_dir": "/tmp" },
            "context_window": {
                "context_window_size": 200000,
                "used_percentage": 41.7,
                "remaining_percentage": 58.3
            },
            "rate_limits": {
                "five_hour": { "used_percentage": 62.0, "resets_at": 1_757_300_000u64 },
                "seven_day": { "used_percentage": 18.5, "resets_at": 1_757_800_000u64 }
            },
            "cost": { "total_cost_usd": 1.5 },
            "output_style": { "name": "default" }
        })
    }

    #[test]
    fn a_real_payload_yields_every_field_the_criterion_names() {
        let t = parse(&doc(), 1000);
        assert_eq!(t.model.as_deref(), Some("Opus 4.5"));
        assert_eq!(t.context_pct, Some(41.7));
        assert_eq!(t.five_hour_pct, Some(62.0));
        assert_eq!(t.five_hour_reset, Some(1_757_300_000));
        assert_eq!(t.seven_day_pct, Some(18.5));
        assert_eq!(t.seven_day_reset, Some(1_757_800_000));
        assert_eq!(t.observed_at, 1000);
        assert!(!t.is_empty());
    }

    #[test]
    fn a_payload_without_rate_limits_is_not_a_failed_parse() {
        // `rate_limits` is present only for a Pro or Max account, and only
        // after the first API response. A build that treated its absence as an
        // error would report nothing at all for everybody else.
        let mut d = doc();
        d.as_object_mut().unwrap().remove("rate_limits");
        let t = parse(&d, 1);
        assert_eq!(t.five_hour_pct, None);
        assert_eq!(t.model.as_deref(), Some("Opus 4.5"));
        assert!(!t.is_empty());
    }

    #[test]
    fn an_empty_document_is_empty_telemetry_and_says_so() {
        let t = parse(&serde_json::json!({}), 5);
        assert!(t.is_empty());
        assert_eq!(t.observed_at, 5);
    }

    #[test]
    fn a_percentage_outside_its_own_range_is_clamped_and_a_non_number_is_absent() {
        let over = serde_json::json!({"context_window": {"used_percentage": 140}});
        assert_eq!(parse(&over, 0).context_pct, Some(100.0));
        let under = serde_json::json!({"context_window": {"used_percentage": -3}});
        assert_eq!(parse(&under, 0).context_pct, Some(0.0));
        let text = serde_json::json!({"context_window": {"used_percentage": "lots"}});
        assert_eq!(
            parse(&text, 0).context_pct,
            None,
            "unknown must not become zero: a bar at 0% is a claim"
        );
    }

    #[test]
    fn a_reset_time_is_read_as_a_number_or_a_string_and_never_guessed() {
        let as_str = serde_json::json!({
            "rate_limits": {"five_hour": {"resets_at": "1757300000"}}
        });
        assert_eq!(parse(&as_str, 0).five_hour_reset, Some(1_757_300_000));
        let rfc = serde_json::json!({
            "rate_limits": {"five_hour": {"resets_at": "2026-09-08T04:00:00Z"}}
        });
        assert_eq!(
            parse(&rfc, 0).five_hour_reset,
            None,
            "a form this build cannot read must be absent, not a wrong instant"
        );
    }

    #[test]
    fn a_model_name_is_bounded_and_stripped() {
        let d = serde_json::json!({"model": {"display_name": "Opus\u{7}4.5"}});
        assert_eq!(parse(&d, 0).model.as_deref(), Some("Opus4.5"));
        let long = serde_json::json!({"model": {"display_name": "x".repeat(400)}});
        assert_eq!(parse(&long, 0).model.unwrap().chars().count(), MAX_FIELD);
        let blank = serde_json::json!({"model": {"display_name": "   "}});
        assert_eq!(parse(&blank, 0).model, None);
    }

    struct Fixture {
        root: PathBuf,
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }
    fn fixture(tag: &str) -> Fixture {
        let root = std::env::temp_dir().join(format!(
            "apex-statusline-{}-{tag}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&root).unwrap();
        Fixture { root }
    }

    #[test]
    fn the_branch_comes_from_a_symref_and_a_detached_head_is_not_a_branch() {
        let f = fixture("branch");
        let deep = f.root.join("src/services");
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::create_dir_all(f.root.join(".git")).unwrap();
        std::fs::write(f.root.join(".git/HEAD"), "ref: refs/heads/roadmap/v2.2\n").unwrap();
        assert_eq!(
            git_branch(&deep).as_deref(),
            Some("roadmap/v2.2"),
            "the walk must reach the root from a subdirectory"
        );

        std::fs::write(f.root.join(".git/HEAD"), "9c0f2b1a\n").unwrap();
        assert_eq!(git_branch(&deep), None, "a detached HEAD is not a branch");
    }

    #[test]
    fn a_worktrees_git_file_is_followed() {
        // §7 makes a worktree the unit of parallel work, so this is the common
        // shape here: `.git` is a file holding a pointer, not a directory.
        let f = fixture("worktree");
        let wt = f.root.join("wt");
        let real = f.root.join("store/worktrees/wt");
        std::fs::create_dir_all(&wt).unwrap();
        std::fs::create_dir_all(&real).unwrap();
        std::fs::write(real.join("HEAD"), "ref: refs/heads/task/p1-021\n").unwrap();
        std::fs::write(wt.join(".git"), format!("gitdir: {}\n", real.display())).unwrap();
        assert_eq!(git_branch(&wt).as_deref(), Some("task/p1-021"));
    }

    #[test]
    fn a_directory_that_is_not_a_repository_has_no_branch() {
        let f = fixture("norepo");
        assert_eq!(git_branch(&f.root), None);
    }

    #[test]
    fn the_users_own_status_line_is_found_in_precedence_order() {
        let f = fixture("settings");
        let home = f.root.join("home");
        let project = f.root.join("project");
        std::fs::create_dir_all(home.join(".claude")).unwrap();
        std::fs::create_dir_all(project.join(".claude")).unwrap();
        std::fs::write(
            home.join(".claude/settings.json"),
            r#"{"statusLine":{"type":"command","command":"~/.claude/statusline.sh",
                "refreshInterval":60}}"#,
        )
        .unwrap();

        let found = user_status_line(&home, None).expect("the user's own");
        assert_eq!(found.command.as_deref(), Some("~/.claude/statusline.sh"));
        assert_eq!(
            found.presentation.get("refreshInterval"),
            Some(&serde_json::json!(60)),
            "an overlay that drops this changes how often the line updates"
        );

        std::fs::write(
            project.join(".claude/settings.json"),
            r#"{"statusLine":{"type":"command","command":"./bin/line","padding":2}}"#,
        )
        .unwrap();
        let found = user_status_line(&home, Some(&project)).expect("the project's");
        assert_eq!(found.command.as_deref(), Some("./bin/line"));
        assert_eq!(found.presentation.get("padding"), Some(&serde_json::json!(2)));
        assert_eq!(
            found.presentation.get("refreshInterval"),
            None,
            "the winning source's object is the whole answer; keys are not \
             gathered from the ones it outranks"
        );
    }

    #[test]
    fn a_settings_file_that_will_not_parse_is_skipped_rather_than_fatal() {
        let f = fixture("badjson");
        let home = f.root.join("home");
        let project = f.root.join("project");
        std::fs::create_dir_all(home.join(".claude")).unwrap();
        std::fs::create_dir_all(project.join(".claude")).unwrap();
        std::fs::write(project.join(".claude/settings.json"), "{ this is not json").unwrap();
        std::fs::write(
            home.join(".claude/settings.json"),
            r#"{"statusLine":{"type":"command","command":"line"}}"#,
        )
        .unwrap();
        let found = user_status_line(&home, Some(&project)).expect("the user's own");
        assert_eq!(found.command.as_deref(), Some("line"));
    }

    #[test]
    fn a_status_line_of_an_unknown_type_is_not_run_as_a_shell_string() {
        let f = fixture("type");
        let home = f.root.join("home");
        std::fs::create_dir_all(home.join(".claude")).unwrap();
        std::fs::write(
            home.join(".claude/settings.json"),
            r#"{"statusLine":{"type":"widget","command":"rm -rf /","padding":1}}"#,
        )
        .unwrap();
        let found = user_status_line(&home, None).expect("the object is there");
        assert_eq!(
            found.command, None,
            "a type this build does not know must not have its `command` run"
        );
        assert_eq!(found.presentation.get("padding"), Some(&serde_json::json!(1)));
    }

    #[test]
    fn the_overlay_points_at_apex_and_carries_the_presentation_keys() {
        let user = UserStatusLine {
            command: Some("~/.claude/statusline.sh".into()),
            presentation: serde_json::json!({"refreshInterval": 60, "padding": 3})
                .as_object()
                .unwrap()
                .clone(),
        };
        let v = overlay("/usr/bin/apex", Some(&user));
        assert_eq!(v["type"], "command");
        assert_eq!(v["command"], "/usr/bin/apex agent statusline");
        assert_eq!(v["refreshInterval"], 60);
        assert_eq!(v["padding"], 3);
        assert!(
            v.get("hideVimModeIndicator").is_none(),
            "a key the user did not set must not be invented"
        );
    }

    #[test]
    fn the_overlay_never_names_the_users_command() {
        // The failure this guards is subtle: an overlay that copied `command`
        // across would point Claude straight at the user's script and APEX
        // would observe nothing, which looks exactly like a working
        // integration until somebody asks why the Agent Center is empty.
        let user = UserStatusLine {
            command: Some("~/.claude/statusline.sh".into()),
            presentation: serde_json::Map::new(),
        };
        let v = overlay("/usr/bin/apex", Some(&user));
        assert_eq!(v["command"], "/usr/bin/apex agent statusline");
    }

    #[test]
    fn apex_refuses_to_chain_to_itself() {
        assert!(is_apex_statusline("/usr/bin/apex agent statusline"));
        assert!(is_apex_statusline("apex   agent   statusline"));
        assert!(!is_apex_statusline("~/.claude/statusline.sh"));
        assert_eq!(chain("/usr/bin/apex agent statusline", b"{}"), None);
    }

    #[test]
    fn chaining_passes_the_payload_through_and_returns_what_was_printed() {
        let out = chain("cat", b"{\"model\":1}").expect("cat ran");
        assert_eq!(String::from_utf8_lossy(&out), "{\"model\":1}");
    }

    #[test]
    fn a_command_that_ignores_its_stdin_is_not_an_error() {
        // The EPIPE case. A status line that prints a constant closes its
        // stdin immediately, and a build that propagated the write failure
        // would show nothing for a script that works.
        let out = chain("echo hello", b"{}").expect("echo ran");
        assert_eq!(String::from_utf8_lossy(&out).trim(), "hello");
    }

    #[test]
    fn a_command_that_does_not_exist_prints_nothing_rather_than_failing() {
        let out = chain("apex-no-such-status-line-program", b"{}");
        assert_eq!(
            out.map(|o| o.is_empty()),
            Some(true),
            "sh runs, the program does not, and stdout is empty"
        );
    }
}
