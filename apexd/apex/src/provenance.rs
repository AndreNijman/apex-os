//! `apex provenance` — P1-026: where an agent plugin came from, and whether
//! what is on disk is still what arrived.
//!
//! **Not `apex plugin`.** That verb is the OS side of apex-shell's QML plugin
//! platform (§16) and its subject is `~/.config/apex-shell/plugins`. This one
//! is about the *agent's* plugins — Claude Code's, under `~/.claude/plugins`,
//! installed from marketplaces. They share a word and nothing else: no code,
//! no state, no vocabulary. The two are kept apart deliberately, because a
//! single `apex plugin provenance` would have to answer for both and the
//! honest answers are opposite.
//!
//! ## What was tracked before this, and why it is a label rather than provenance
//!
//! Claude Code records real facts. `known_marketplaces.json` holds each
//! marketplace's source (`github`, plus `owner/repo`), its `installLocation`
//! and a `lastUpdated`; `installed_plugins.json` holds each plugin's
//! `installPath`, a `version` and an `installedAt`. APEX already reads both —
//! `servers::enabled_plugin_configs` resolves an enabled plugin to its
//! `.mcp.json`, and `profile.rs`'s `doctor` reports an enabled plugin whose
//! marketplace is unknown or whose checkout is missing.
//!
//! What none of it does is check any of it against the bytes on disk. There is
//! no hash anywhere in either file; `doctor`'s strongest test is
//! `installPath.is_dir()`. On this machine two of the recorded versions are the
//! literal string `"unknown"`. A recorded origin that nothing ever re-checks
//! is a label — it says where a directory is *supposed* to have come from, and
//! it would say exactly the same thing after somebody edited a script inside
//! it.
//!
//! So this module measures three things instead of restating one:
//!
//! 1. **The marketplace's revision**, and how strong that answer is.
//!    A marketplace checkout that is a git repository has a real commit and
//!    can be asked whether its working tree still matches it. One that is not
//!    a repository may still carry a revision beside itself — the official
//!    marketplace ships `.gcs-sha` — and that is a *claim* by whoever wrote
//!    the file, not something re-checkable here. The two are different answers
//!    and get different names.
//!
//! 2. **A digest of the plugin tree**, recomputed from disk every run, over
//!    file contents *and* execute bits — so a script that gained `+x` is a
//!    change, which a content-only hash would miss.
//!
//! 3. **Whether that digest still matches the one recorded**, which is the
//!    part that makes it provenance rather than a fingerprint.
//!
//! ## The baseline is trust-on-first-use, and is labelled as that everywhere
//!
//! Nobody signs a Claude Code plugin, and there is no digest in the registry
//! to compare against, so the only baseline available is one APEX records the
//! first time it sees a tree. That detects a plugin modified *after* it was
//! installed, which is the threat a person can actually act on; it says
//! nothing at all about whether the tree was what the marketplace intended at
//! the moment it arrived. That limit is not buried — [`Attest::FirstSight`] is
//! its own verdict, the JSON carries `trustOnFirstUse: true`, and the report
//! says so in words. `verify.rs` can do better than this for an image because
//! an image is signed; nothing here can, and pretending otherwise would be the
//! label problem again, one level up.
//!
//! ## Isolation: what is honestly true, component by component
//!
//! P1-026's second criterion says executable plugin content runs inside a
//! sandbox. That cannot be delivered whole, and the shortfall is architectural
//! rather than unfinished work, so it is reported per component instead of
//! claimed in one word:
//!
//!   * **An MCP server a plugin defines** really can be confined — bubblewrap,
//!     through `apex mcp run` — and there are now two ways it comes to be.
//!     Its definition on disk may name the wrapper, in which case it is
//!     confined however the agent is started; or the launch configuration
//!     `apex-agentd` writes may wrap it, in which case it is confined in a
//!     session `apex agent` started and **not** in one a person started
//!     themselves. Those are different facts and the report does not merge
//!     them: the answer comes from a real [`mcpconf::curate`] run, so it is the
//!     session launcher's own verdict rather than a second reading of the same
//!     file that could disagree with it.
//!   * **Hooks and scripts** a plugin ships are run by the agent, not by
//!     anything in APEX. Nothing here adds a sandbox of its own; they run
//!     inside whatever the *session* is confined to, which for a confined
//!     session is a real boundary and for `--sandbox unrestricted` is none.
//!     Reported as that, never as unknown and never as "sandboxed".
//!   * **Commands, agents, prompts and skills** are content the model reads.
//!     There is no process to confine, which is not the same as being safe.
//!
//! The word "sandbox" is therefore spent only where bubblewrap actually runs,
//! and never without the clause saying when.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::Result;
use serde_json::{json, Value};

use apex_agent_core::mcpconf::Wrap;

use crate::digest::{self, Digest};
use crate::mcp::servers;

/// Runtime state Claude Code writes inside an installed plugin's own tree.
///
/// One file per live process, so a digest that covered it would change
/// whenever the plugin was in use and every re-check would report a change.
/// See [`digest::walk_excluding`] for why an exclusion is load-bearing here
/// rather than a convenience.
const VOLATILE: &[&str] = &[".in_use"];

/// Where the recorded baselines live.
const STORE: &str = "apex/plugin-provenance.json";

// ═════════════════════════════════════════════════════════════════════════════
//  revisions
// ═════════════════════════════════════════════════════════════════════════════

/// What revision a marketplace checkout is at, and how strong the answer is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Revision {
    /// A git checkout. The commit is real and `dirty` says whether the working
    /// tree still matches it — the one case where a recorded origin can be
    /// re-checked against what is on disk.
    Commit {
        sha: String,
        /// `None` when `git status` could not be run, which is not "clean".
        dirty: Option<bool>,
        remote: Option<String>,
    },
    /// A revision recorded beside the tree by whatever installed it. A claim,
    /// not a measurement: nothing here can check the files against it.
    Claimed { sha: String, from: String },
    /// No revision could be established, and why.
    Unmeasured(String),
}

impl Revision {
    pub fn describe(&self) -> String {
        match self {
            Revision::Commit {
                sha,
                dirty,
                remote,
            } => {
                let state = match dirty {
                    Some(true) => "and its working tree has been MODIFIED since",
                    Some(false) => "and its working tree still matches it",
                    // Deliberately does NOT contain the clean arm's phrase.
                    // It used to read "whether the tree still matches could not
                    // be checked", which any grep for "still matches" also hit
                    // — so a check for the clean answer matched the arm that
                    // means nobody looked.
                    None => "and whether its working tree matches could not be checked",
                };
                match remote {
                    Some(r) => format!("git {sha} from {r}, {state}"),
                    None => format!("git {sha}, {state}"),
                }
            }
            Revision::Claimed { sha, from } => format!(
                "{sha}, claimed by {from} — not a git checkout, so nothing here can check the \
                 files against it"
            ),
            Revision::Unmeasured(why) => format!("none: {why}"),
        }
    }

    fn to_json(&self) -> Value {
        match self {
            Revision::Commit { sha, dirty, remote } => json!({
                "kind": "git", "commit": sha, "dirty": dirty, "remote": remote,
                "recheckable": true,
            }),
            Revision::Claimed { sha, from } => json!({
                "kind": "claimed", "commit": sha, "from": from, "recheckable": false,
            }),
            Revision::Unmeasured(why) => json!({
                "kind": null, "commit": null, "why": why, "recheckable": false,
            }),
        }
    }
}

/// Run a git query in a directory, returning the trimmed stdout on success.
///
/// A git that is not installed, or a directory that is not a repository, comes
/// back `Err` with the reason — never an empty string that a caller could read
/// as "clean" or "no remote".
/// `--no-optional-locks` because this is a *read*: a plain
/// `git status --porcelain` refreshes and rewrites `.git/index` in whatever
/// checkout it is pointed at, and `apex provenance show` runs against real
/// marketplace checkouts on somebody's machine. A measurement that modifies
/// what it measures is not one.
fn git(prog: &str, dir: &Path, args: &[&str]) -> Result<String, GitErr> {
    let out = Command::new(prog)
        .arg("--no-optional-locks")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .map_err(|e| GitErr::CouldNotRun(format!("git could not be run: {e}")))?;
    if !out.status.success() {
        return Err(GitErr::Refused(
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Why a git query did not answer, in the two arms this module exists to keep
/// apart.
///
/// It was one `String`, and both arms landed in the same `Err(_) => {}` in
/// [`revision_of_with`], which then fell through to
/// "is not a git checkout and carries no revision file". So a git that could
/// not be STARTED — not installed, refused by the kernel, killed by a resource
/// limit — was reported as a directory that is simply not a repository, which
/// is the reassuring answer and the wrong one. That is the exact collapse the
/// `symlink_metadata` call above this one refuses to make about the directory,
/// made about the program instead, in the module whose whole subject is telling
/// a measurement apart from a guess.
///
/// The distinction is not academic here. It is what turned a flaky test in this
/// file into a mystery: `rev-parse` failed to exec, and the panic said the
/// fixture "is not a git checkout" about a directory the test had just written
/// a git into.
enum GitErr {
    /// The program could not be started at all. Nothing was measured.
    CouldNotRun(String),
    /// git started, ran, and answered no. That IS a measurement — a directory
    /// that is not a repository reaches here, and so does one with no commits.
    Refused(String),
}

/// The revision of one marketplace checkout.
///
/// git first, because it is the only answer that can be re-checked. Then a
/// revision file beside the tree, reported as a claim. Then nothing, with the
/// reason — and "the directory is not there" and "git refused" are different
/// reasons that both end up here saying which they were.
pub fn revision_of(dir: &Path) -> Revision {
    revision_of_with(dir, "git")
}

/// [`revision_of`], with the git program named.
///
/// A parameter and not a constant for one reason: the arm where `rev-parse`
/// answers and `status` does **not** decides whether a checkout nobody could
/// check reads as clean, and there is no way to make the real git fail that way
/// on demand. `digest_with` exists for the same reason. Taken as an argument
/// rather than an environment variable so one test cannot move another's git
/// mid-run — the defect that broke two unrelated tests in this branch already.
pub fn revision_of_with(dir: &Path, prog: &str) -> Revision {
    // NOT `dir.exists()`. That returns false for a directory that is there and
    // refused — a marketplace checkout under a mode-000 parent would have been
    // reported as "not on this machine", which reads as nothing-to-check. This
    // repository has found that exact collapse in about fourteen places, and it
    // is not going to be reintroduced by the one module whose entire subject is
    // telling a measurement apart from a guess.
    match std::fs::symlink_metadata(dir) {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Revision::Unmeasured(format!("{} is not on this machine", dir.display()));
        }
        Err(e) => {
            return Revision::Unmeasured(format!(
                "{} is there and could not be read: {e}",
                dir.display()
            ));
        }
    }
    let mut refused: Option<String> = None;
    match git(prog, dir, &["rev-parse", "HEAD"]) {
        Ok(sha) if !sha.is_empty() => {
            let dirty = match git(prog, dir, &["status", "--porcelain"]) {
                Ok(text) => Some(!text.trim().is_empty()),
                // Could not ask. NOT clean — `Some(false)` here would report a
                // checkout nobody could check as one whose tree still matches
                // its commit, which is the module's whole thesis inverted.
                Err(_) => None,
            };
            let remote = git(prog, dir, &["remote", "get-url", "origin"])
                .ok()
                .filter(|s| !s.is_empty());
            return Revision::Commit { sha, dirty, remote };
        }
        Ok(_) => {}
        // Could not START git. Not "this is not a checkout": nothing was
        // measured, and the caller must be told which of the two it is.
        Err(GitErr::CouldNotRun(why)) => {
            return Revision::Unmeasured(format!("{}: {why}", dir.display()));
        }
        // git ran and said no. A directory that is not a repository lands here,
        // which is a real answer, so the revision-file fallback below is right.
        // Its reason is kept rather than dropped: if the fallback finds nothing
        // either, git's own sentence is the most specific thing anyone has.
        Err(GitErr::Refused(why)) => refused = Some(why),
    }
    // Not a git checkout. The official marketplace ships its revision in
    // `.gcs-sha`; read it as a claim rather than ignoring a real fact.
    for name in [".gcs-sha", ".revision", ".commit"] {
        let p = dir.join(name);
        match std::fs::read_to_string(&p) {
            Ok(text) => {
                let sha = text.trim().to_string();
                if !sha.is_empty() && sha.chars().all(|c| c.is_ascii_hexdigit()) {
                    return Revision::Claimed {
                        sha,
                        from: name.to_string(),
                    };
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => {
                // Present and refused: not absence.
                return Revision::Unmeasured(format!(
                    "{} exists and could not be read: {e}",
                    p.display()
                ));
            }
        }
    }
    // Both fallbacks are exhausted. git's own sentence, where it gave one, is
    // the most specific thing anybody has about why — an empty repository and a
    // directory that is not one at all both reach here, and they are not the
    // same problem to whoever has to fix it.
    let mut why = format!(
        "{} is not a git checkout and carries no revision file",
        dir.display()
    );
    if let Some(reason) = refused.filter(|r| !r.is_empty()) {
        why.push_str(&format!(" (git said: {reason})"));
    }
    Revision::Unmeasured(why)
}

// ═════════════════════════════════════════════════════════════════════════════
//  the digest re-check
// ═════════════════════════════════════════════════════════════════════════════

/// Whether a plugin's files are still the ones that were recorded.
///
/// Four values, and they are `verify.rs`'s four under different names. That
/// module verifies a signed image; this one re-hashes an unsigned plugin, so
/// the words differ, but the shape is deliberately identical and the mapping
/// is exact:
///
/// | here | `verify::Verdict` | what it is a measurement about |
/// |------|-------------------|--------------------------------|
/// | [`Attest::Match`] | `Verified` | the bytes |
/// | [`Attest::Changed`] | `Failed` | the bytes — the answer is no |
/// | [`Attest::FirstSight`] | `Absent` | the *records*: nothing was recorded |
/// | [`Attest::Unmeasurable`] | `CouldNotRun` | this machine: the check did not run |
///
/// The two right-hand rows are the pair that keeps getting collapsed, and
/// `verify.rs`'s header says why they must not be: fold `CouldNotRun` into
/// `Failed` and every unreadable tree becomes an accusation; fold it into
/// `Match` and a tree nobody could hash reports as unchanged. Neither
/// `FirstSight` nor `Unmeasurable` is a pass here, and only `Changed` is a
/// failure — see [`Attest::is_finding`].
///
/// Both are now on one tip, so `verify::Verdict` **can** be imported here —
/// and it is deliberately not, because the payloads say different things and
/// merging them would cost the words that make each one true:
///
///   * `Verified { signer, at }` carries a certificate identity and the instant
///     a chain was checked. Nobody signs a Claude Code plugin. There is no
///     signer to put in it, and "verified" is the exact word this module exists
///     to refuse — a match here means the tree has not changed since APEX first
///     saw it, which is a far weaker claim.
///   * `Changed { recorded, found }` carries two hashes so a reader can see
///     which pair disagreed. `Failed(String)` has one free-text slot and would
///     reduce that to prose.
///   * `Absent` is about a publisher that signed nothing; [`Attest::FirstSight`]
///     is about APEX's own records. They are not the same absence.
///
/// What is shared is the shape, on purpose: four values, `reason()` and `tag()`
/// named after `verify::Verdict::reason` and `as_str`, and the same rule that
/// could-not-run is neither a pass nor a failure. `trust.rs` imports
/// `verify::Verdict` directly because it is about the same subject — a signed
/// image. This is not, and borrowing the type would be borrowing the claim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Attest {
    /// The tree on disk hashes to what was recorded.
    Match,
    /// It does not. This is the finding the whole module exists to produce.
    Changed { recorded: String, found: String },
    /// Nothing was recorded for this tree, so there is nothing to compare.
    /// Trust-on-first-use, and not a pass.
    FirstSight,
    /// The tree could not be hashed, so neither a pass nor a failure.
    Unmeasurable(String),
}

impl Attest {
    /// The machine-readable state, and the **only** vocabulary for it.
    ///
    /// `to_json` used to hand-write its own four words, so each arm had two
    /// names and nothing kept them in step — the kind of drift that ends with a
    /// consumer matching on a string the enum stopped emitting. Named and
    /// spelled after `verify::Verdict::as_str`, whose four arms these are.
    pub fn tag(&self) -> &'static str {
        match self {
            Attest::Match => "unchanged",
            Attest::Changed { .. } => "changed",
            Attest::FirstSight => "first-sight",
            Attest::Unmeasurable(_) => "could-not-run",
        }
    }

    /// The reason, for every state that has one — `verify::Verdict::reason`'s
    /// counterpart.
    pub fn reason(&self) -> Option<&str> {
        match self {
            Attest::Match | Attest::FirstSight => None,
            Attest::Unmeasurable(w) => Some(w),
            Attest::Changed { .. } => Some("the digest on disk is not the recorded one"),
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Attest::Match => "the files on disk hash to what was recorded".to_string(),
            Attest::Changed { recorded, found } => format!(
                "the files on disk have CHANGED since they were recorded\n      recorded {recorded}\n      found    {found}"
            ),
            Attest::FirstSight => {
                "no digest was ever recorded for this plugin, so there is nothing to compare \
                 (`apex provenance record` takes one)"
                    .to_string()
            }
            Attest::Unmeasurable(why) => format!("could not be checked: {why}"),
        }
    }

    /// Whether this is a finding somebody has to act on.
    pub fn is_finding(&self) -> bool {
        matches!(self, Attest::Changed { .. })
    }

    /// Built the way `verify::Verdict::to_json` builds its own: `state` from
    /// [`Attest::tag`] and `reason` from [`Attest::reason`], so the JSON cannot
    /// say a different word from the enum.
    fn to_json(&self) -> Value {
        let mut m = serde_json::Map::new();
        m.insert("state".into(), Value::from(self.tag()));
        if let Some(w) = self.reason() {
            m.insert("reason".into(), Value::from(w));
        }
        // `unchanged` is a statement in exactly the two arms that make one, and
        // null in the other two: neither first-sight nor could-not-run says
        // anything about whether the tree changed, and a `false` there would be
        // an accusation this module has not earned.
        m.insert(
            "unchanged".into(),
            match self {
                Attest::Match => Value::Bool(true),
                Attest::Changed { .. } => Value::Bool(false),
                Attest::FirstSight | Attest::Unmeasurable(_) => Value::Null,
            },
        );
        if let Attest::Changed { recorded, found } = self {
            m.insert("recorded".into(), Value::from(recorded.clone()));
            m.insert("found".into(), Value::from(found.clone()));
        }
        Value::Object(m)
    }
}

/// The recorded baselines, by plugin key.
#[derive(Debug, Clone, Default)]
pub struct Baselines {
    pub path: PathBuf,
    pub digests: BTreeMap<String, String>,
    /// A store that exists and could not be read or parsed.
    ///
    /// Never silently an empty store: that would turn every plugin into
    /// [`Attest::FirstSight`] and so report a clean bill of health for a
    /// machine whose records had been deleted — which is exactly what somebody
    /// tampering with a plugin would want.
    pub error: Option<String>,
}

/// Where the store lives for a given `$HOME`, honouring `XDG_STATE_HOME`.
///
/// **Read the env var here and nowhere deeper.** Everything below takes the
/// path as an argument ([`read_baselines_at`], [`build_at`]) so that a test can
/// name a throwaway store without touching a process-wide variable. That is
/// not a style preference: `store_path` prefers `$XDG_STATE_HOME` over the
/// `home` it is handed, so a test that set the variable to point at its own
/// fixture was one unset away from writing over the *real* store — and a test
/// that set it while another test in the same binary was reading it would move
/// that test's store mid-run. `digest.rs`'s `digest_with` exists for the same
/// reason, after `APEX_SHA256SUM` broke two unrelated tests running in
/// parallel; this is the second occurrence of that defect shape in this branch.
pub fn store_path(home: &Path) -> PathBuf {
    match std::env::var_os("XDG_STATE_HOME") {
        Some(x) if !x.is_empty() => PathBuf::from(x).join(STORE),
        _ => home.join(".local/state").join(STORE),
    }
}

pub fn read_baselines_at(path: &Path) -> Baselines {
    let path = path.to_path_buf();
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Baselines {
                path,
                ..Default::default()
            }
        }
        Err(e) => {
            return Baselines {
                path,
                error: Some(format!("it exists and could not be read: {e}")),
                ..Default::default()
            }
        }
    };
    let doc: Value = match serde_json::from_str(&text) {
        Ok(d) => d,
        Err(e) => {
            return Baselines {
                path,
                error: Some(format!("it is not valid JSON: {e}")),
                ..Default::default()
            }
        }
    };
    let digests = doc
        .get("plugins")
        .and_then(Value::as_object)
        .map(|m| {
            m.iter()
                .filter_map(|(k, v)| {
                    v.get("sha256")
                        .and_then(Value::as_str)
                        .map(|s| (k.clone(), s.to_string()))
                })
                .collect()
        })
        .unwrap_or_default();
    Baselines {
        path,
        digests,
        error: None,
    }
}

// ═════════════════════════════════════════════════════════════════════════════
//  isolation
// ═════════════════════════════════════════════════════════════════════════════

/// What executable content a plugin ships, and what confines each kind.
#[derive(Debug, Clone, Default)]
pub struct Confinement {
    /// MCP servers this plugin defines, and what confines each — as the
    /// session launcher itself answered it, not as this module guessed.
    ///
    /// [`Wrap`] rather than a boolean because there are two ways to be
    /// confined and they do not hold in the same places: a definition that
    /// names `apex mcp run` survives anybody starting the agent, and one the
    /// launch configuration rewrites holds only for a session `apex agent`
    /// started. `None` is the launcher not having classified it at all, which
    /// is a disagreement between two readers of the same file and is printed
    /// as that rather than as a verdict.
    pub mcp: Vec<(String, Option<Wrap>)>,
    /// Files with an execute bit anywhere in the tree.
    pub executables: Vec<String>,
    /// Whether it ships a `hooks` directory.
    pub hooks: bool,
    /// Skills it ships, which `apex skill list` also reports.
    pub skills: usize,
}

impl Confinement {
    /// Lines for the report, in the order of decreasing honesty about
    /// confinement: what really is confined, then what is not.
    pub fn lines(&self) -> Vec<String> {
        let mut out = Vec::new();
        for (name, confined) in &self.mcp {
            out.push(format!(
                "MCP server '{name}' — {}",
                match confined {
                    Some(Wrap::InDefinition) =>
                        "sandboxed however the agent is started: its own definition names \
                         `apex mcp run`, so bubblewrap confines it",
                    // The clause after the colon is the whole value of this
                    // arm. Without it the line is true of a session `apex
                    // agent` started and false of the one the reader is
                    // probably in.
                    Some(Wrap::AtLaunch) =>
                        "sandboxed in a session `apex agent` starts: the launch configuration \
                         wraps it. Start the agent yourself and it runs exactly as the plugin \
                         defined it — `apex mcp confine` writes the wrapper to disk instead",
                    Some(Wrap::Not) =>
                        "NOT sandboxed, though it could be: `apex mcp confine` would wrap it",
                    // Reported through the plane, not here: there is no local
                    // process, and "NOT sandboxed" would describe a program
                    // that does not exist.
                    Some(Wrap::NoProcess) =>
                        "an endpoint off this machine — no local process to confine; \
                         `apex mcp planes` is where its credential is reported",
                    None =>
                        "the session launcher did not classify it, which means two readers of \
                         the same file disagree — report this rather than trusting either",
                }
            ));
        }
        if self.hooks {
            out.push(
                "hooks — run by the agent, not by APEX, so nothing here adds a sandbox of \
                 its own. They run inside whatever the SESSION is confined to: a real \
                 boundary under `--sandbox project` or `strict`, and none at all under \
                 `unrestricted`"
                    .to_string(),
            );
        }
        if !self.executables.is_empty() {
            out.push(format!(
                "{} executable file(s) — run by the agent if it is told to, inside the \
                 session's own sandbox and nothing narrower",
                self.executables.len()
            ));
        }
        // The control APEX does have over the two lines above, said where the
        // reader is looking at the shortfall rather than left to be found.
        //
        // Round 2 of this unit had to delete five sentences telling the reader
        // that a control did not exist on a tip that had built it, and a
        // readout that understates what is available is worse than one that
        // never raised the subject. This is the same defect caught before it
        // shipped: dimension 8 exists, it is the only thing that reaches this
        // content, and it REMOVES rather than confines — so the sentence says
        // removes.
        if self.removable_content() {
            out.push(
                "  ↳ APEX does not own the agent's process, so it cannot put a sandbox \
                 around something the agent spawns. What it can do is not load the \
                 plugin: `apex agent run --plugins none`, or `--plugins curated` with \
                 the names in `plugin_allow`, starts the session without it and none of \
                 the above runs"
                    .to_string(),
            );
        }
        if self.skills > 0 {
            out.push(format!(
                "{} skill(s) — content the model reads; `apex skill list` reports them",
                self.skills
            ));
        }
        if out.is_empty() {
            out.push(
                "no MCP server, no hooks and no executable file: content the model reads, with \
                 no process of its own to confine"
                    .to_string(),
            );
        }
        out
    }

    /// Whether the plugin ships anything with a process of its own.
    pub fn has_executable_content(&self) -> bool {
        !self.mcp.is_empty() || self.hooks || !self.executables.is_empty()
    }

    fn to_json(&self) -> Value {
        json!({
            "mcpServers": self.mcp.iter()
                .map(|(n, c)| json!({
                    "name": n,
                    "sandboxed": c.and_then(|w| w.confined()),
                    "sandboxedBy": c.map_or("unknown", |w| w.tag()),
                    "sandboxSurvivesAHandRun": c.is_some_and(|w| w.survives_a_hand_run()),
                }))
                .collect::<Vec<_>>(),
            "hooks": self.hooks,
            "executables": self.executables,
            "skills": self.skills,
            // Whether there is anything to confine at all. Without this,
            // `everythingExecutableIsSandboxed` is vacuously true for a plugin
            // that ships nothing but Markdown, and a JSON consumer reading only
            // that field would report a documentation-only plugin as
            // "sandboxed" — the label problem, in a field name.
            "hasExecutableContent": self.has_executable_content(),
            // The criterion, answered honestly rather than with one boolean —
            // and now with the qualifier that decides whether the answer is
            // about this machine or about one path through it.
            "everythingExecutableIsSandboxed":
                self.mcp.iter().all(|(_, c)| c.and_then(|w| w.confined()) != Some(false))
                && !self.hooks
                && self.executables.is_empty(),
            // The same question asked of a session nobody started through
            // `apex agent`. Lower than the field above whenever the launch
            // configuration is doing the confining, and the gap is the point:
            // a reader who takes the first field for a property of the machine
            // is reading the wrong one.
            "everythingExecutableIsSandboxedWithoutApexAgent":
                self.mcp.iter().all(|(_, c)| match c {
                    Some(w) => w.survives_a_hand_run() || w.confined().is_none(),
                    None => false,
                })
                && !self.hooks
                && self.executables.is_empty(),
            "unconfinedExecutableContent":
                self.hooks || !self.executables.is_empty()
                || self.mcp.iter().any(|(_, c)| c.and_then(|w| w.confined()) == Some(false)),
            // Dimension 8. Beside the field above rather than instead of it,
            // because they are different claims: `unconfinedExecutableContent`
            // says a sandbox does not reach this, and this says the session
            // can be started without it. A consumer that read only the first
            // would report a shortfall with no remedy, which is the readout
            // defect round 2 of this unit had to go back and fix.
            "removableByPluginPolicy": self.removable_content(),
        })
    }

    /// Whether this plugin ships content that only removal can deal with.
    ///
    /// Hooks and loose executables, not MCP servers: dimension 7 already
    /// confines those, and naming them here would make the removal sound
    /// necessary for content that is already handled.
    fn removable_content(&self) -> bool {
        self.hooks || !self.executables.is_empty()
    }
}

// ═════════════════════════════════════════════════════════════════════════════
//  the model
// ═════════════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct Marketplace {
    pub name: String,
    /// `github`, or whatever the registry says.
    pub source_kind: Option<String>,
    /// `owner/repo` for a github source.
    pub source: Option<String>,
    pub location: Option<PathBuf>,
    pub last_updated: Option<String>,
    pub revision: Revision,
}

impl Marketplace {
    fn to_json(&self) -> Value {
        json!({
            "name": self.name,
            "sourceKind": self.source_kind,
            "source": self.source,
            "location": self.location.as_ref().map(|p| p.display().to_string()),
            "lastUpdated": self.last_updated,
            "revision": self.revision.to_json(),
        })
    }
}

#[derive(Debug, Clone)]
pub struct Plugin {
    /// `name@marketplace`, the key both registries use.
    pub key: String,
    pub name: String,
    pub marketplace: String,
    /// Whether the marketplace it names is one this machine knows.
    pub marketplace_known: bool,
    pub install_path: PathBuf,
    /// The registry's `version`, which on this machine is sometimes the
    /// literal string "unknown".
    pub version: Option<String>,
    pub installed_at: Option<String>,
    pub enabled: bool,
    pub digest: Digest,
    pub attest: Attest,
    pub confinement: Confinement,
    /// Bytes the digest deliberately does not cover.
    pub excluded: Vec<String>,
}

impl Plugin {
    fn to_json(&self) -> Value {
        json!({
            "key": self.key,
            "name": self.name,
            "marketplace": self.marketplace,
            "marketplaceKnown": self.marketplace_known,
            "installPath": self.install_path.display().to_string(),
            "recordedVersion": self.version,
            "installedAt": self.installed_at,
            "enabled": self.enabled,
            "digest": self.digest.to_json(),
            "attest": self.attest.to_json(),
            "confinement": self.confinement.to_json(),
            "digestExcludes": self.excluded,
        })
    }
}

#[derive(Debug, Clone, Default)]
pub struct Report {
    pub marketplaces: Vec<Marketplace>,
    pub plugins: Vec<Plugin>,
    pub baselines: Baselines,
    /// Registry files that exist and could not be read.
    pub unreadable: Vec<String>,
}

impl Report {
    /// Whether this report covers everything it set out to.
    ///
    /// A plugin whose tree could not be hashed counts against it: the report
    /// says nothing about that plugin, and `skill.rs` already holds the same
    /// line — an incomplete inventory is not a successful one, because the
    /// clean-looking output is what somebody would rely on to say nothing has
    /// been tampered with.
    pub fn complete(&self) -> bool {
        self.unreadable.is_empty()
            && self.baselines.error.is_none()
            && !self
                .plugins
                .iter()
                .any(|p| matches!(p.attest, Attest::Unmeasurable(_)))
    }

    pub fn findings(&self) -> Vec<String> {
        let mut out = Vec::new();
        for u in &self.unreadable {
            out.push(format!(
                "{u} — so this report is INCOMPLETE; it is not a machine with no plugins"
            ));
        }
        if let Some(e) = &self.baselines.error {
            out.push(format!(
                "the provenance store at {} could not be used: {e}. Every plugin below reads as \
                 'not recorded', which is what a deleted store looks like — do not read it as a \
                 clean bill of health",
                self.baselines.path.display()
            ));
        }
        for m in &self.marketplaces {
            if let Revision::Commit {
                dirty: Some(true), ..
            } = &m.revision
            {
                out.push(format!(
                    "the {} marketplace checkout has been modified since its commit",
                    m.name
                ));
            }
        }
        for p in &self.plugins {
            if p.attest.is_finding() {
                out.push(format!("{}: {}", p.key, p.attest.describe()));
            }
            // Neither a pass nor a failure, and still something somebody has
            // to see: this is the row the report does not cover. Silence here
            // would look exactly like a plugin that checked out clean.
            if let Attest::Unmeasurable(why) = &p.attest {
                out.push(format!(
                    "{}: its files could not be checked ({why}) — that is neither a pass nor a \
                     failure, and it means this report does NOT cover that plugin",
                    p.key
                ));
            }
            if !p.marketplace_known {
                out.push(format!(
                    "{}: the {} marketplace is not known on this machine, so where it came from \
                     cannot be established at all",
                    p.key, p.marketplace
                ));
            }
            // A version of "unknown" is not a version, and it is what the
            // registry actually holds for two plugins here.
            if p.version.as_deref() == Some("unknown") {
                out.push(format!(
                    "{}: its recorded version is the literal string \"unknown\", so the registry \
                     cannot say which release is installed",
                    p.key
                ));
            }
        }
        out
    }
}

/// Build the whole report, with the baseline store where this `$HOME` keeps it.
pub fn build(home: &Path) -> Report {
    build_at(home, &store_path(home))
}

/// [`build`] with the baseline store named explicitly.
///
/// The store path is a parameter and not an environment lookup so that the
/// suite can point one at a throwaway directory — see [`store_path`] for the
/// defect that makes this necessary rather than tidy.
pub fn build_at(home: &Path, store: &Path) -> Report {
    let mut r = Report {
        baselines: read_baselines_at(store),
        ..Default::default()
    };
    let store_error = r.baselines.error.clone();

    let plugins_dir = home.join(".claude/plugins");
    let known_path = plugins_dir.join("known_marketplaces.json");
    let installed_path = plugins_dir.join("installed_plugins.json");
    let settings_path = home.join(".claude/settings.json");

    // Each read distinguishes absent from refused. `servers::read_json` returns
    // Option and cannot, so the refusal is checked separately first — the whole
    // point of this unit.
    let known = read_registry(&known_path, &mut r.unreadable);
    let installed = read_registry(&installed_path, &mut r.unreadable);
    let settings = read_registry(&settings_path, &mut r.unreadable);

    let mut known_names: Vec<String> = Vec::new();
    if let Some(doc) = known.as_ref().and_then(Value::as_object) {
        for (name, def) in doc {
            known_names.push(name.clone());
            let location = def
                .get("installLocation")
                .and_then(Value::as_str)
                .map(PathBuf::from);
            let revision = match &location {
                Some(dir) => revision_of(dir),
                None => Revision::Unmeasured(
                    "the registry records no installLocation for it".to_string(),
                ),
            };
            r.marketplaces.push(Marketplace {
                name: name.clone(),
                source_kind: def
                    .get("source")
                    .and_then(|s| s.get("source"))
                    .and_then(Value::as_str)
                    .map(str::to_string),
                source: def
                    .get("source")
                    .and_then(|s| s.get("repo"))
                    .and_then(Value::as_str)
                    .map(str::to_string),
                location,
                last_updated: def
                    .get("lastUpdated")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                revision,
            });
        }
    }
    // A marketplace named in settings but never fetched is still known —
    // `profile.rs`'s doctor makes the same allowance, and disagreeing with it
    // would mean one of the two reported a problem the other did not.
    if let Some(extra) = settings
        .as_ref()
        .and_then(|s| s.get("extraKnownMarketplaces"))
        .and_then(Value::as_object)
    {
        known_names.extend(extra.keys().cloned());
    }
    known_names.sort();
    known_names.dedup();
    r.marketplaces.sort_by(|a, b| a.name.cmp(&b.name));

    let enabled: Vec<String> = settings
        .as_ref()
        .and_then(|s| s.get("enabledPlugins"))
        .and_then(Value::as_object)
        .map(|m| {
            m.iter()
                .filter(|(_, v)| v.as_bool() == Some(true))
                .map(|(k, _)| k.clone())
                .collect()
        })
        .unwrap_or_default();

    let found_servers = servers::discover(home, None);
    // What a session started through `apex agent` would be handed, for this
    // `$HOME`. Through the one function `apex mcp list` and `apex mcp planes`
    // also call, so no two readouts can disagree about what confines a
    // plugin's server. No cwd, for the same reason `discover` is given none
    // here: a repository's definitions are not a plugin's.
    let launch = crate::connector::launch_verdicts(home, None);

    if let Some(doc) = installed
        .as_ref()
        .and_then(|d| d.get("plugins"))
        .and_then(Value::as_object)
    {
        for (key, entries) in doc {
            let (name, marketplace) = match key.rsplit_once('@') {
                Some((n, m)) => (n.to_string(), m.to_string()),
                None => (key.clone(), String::new()),
            };
            for entry in entries.as_array().map(|a| a.as_slice()).unwrap_or(&[]) {
                let Some(dir) = entry.get("installPath").and_then(Value::as_str) else {
                    continue;
                };
                let install_path = PathBuf::from(dir);
                let w = digest::walk_excluding(&install_path, VOLATILE);
                let d = digest::digest(&install_path, &w);
                // The stored baseline read back as the `Digest` it was
                // written from, so the comparison goes through
                // `Digest::same_as` rather than `==` on two strings. That is
                // not ceremony: `same_as` answers `None` whenever either side
                // is unmeasured, so this cannot read a tree nobody could hash
                // as a mismatch — which is the single mistake that would turn
                // an unreadable plugin into an accusation.
                let recorded = r
                    .baselines
                    .digests
                    .get(key)
                    .map(|h| Digest::Sha256(h.clone()));
                let attest = match (&d, &recorded) {
                    // Unmeasured first, baseline or no baseline: a tree nobody
                    // could hash is a fact about this machine, never one about
                    // the records.
                    (Digest::Unmeasured(why), _) => Attest::Unmeasurable(why.clone()),
                    (found, Some(rec)) => match found.same_as(rec) {
                        Some(true) => Attest::Match,
                        Some(false) => Attest::Changed {
                            recorded: rec.describe(),
                            found: found.describe(),
                        },
                        // Unreachable as written — the arm above takes every
                        // unmeasured `found`, and `rec` is a `Sha256` by
                        // construction. Spelled as a verdict rather than an
                        // `unwrap` because the one thing this module must never
                        // do is turn "could not tell" into a panic, or into a
                        // mismatch.
                        None => Attest::Unmeasurable(
                            "the two digests could not be compared".to_string(),
                        ),
                    },
                    // No recorded digest. Which of the two reasons matters:
                    // *nobody recorded one* is `FirstSight` (verify.rs's
                    // `Absent` — a fact about the records), while *the store
                    // could not be read* is `Unmeasurable` (`CouldNotRun` — a
                    // fact about this machine). Calling the second one
                    // "not recorded" is what a deleted store would want said.
                    (Digest::Sha256(_), None) => match &store_error {
                        Some(e) => Attest::Unmeasurable(format!(
                            "the baseline store {e}, so whether this tree has changed cannot be \
                             established at all"
                        )),
                        None => Attest::FirstSight,
                    },
                };

                let mcp: Vec<(String, Option<Wrap>)> = found_servers
                    .iter()
                    .filter(|s| matches!(&s.surface, servers::Surface::Plugin { plugin, .. } if plugin == &name))
                    .map(|s| {
                        // The launcher's own verdict, looked up by name. Asking
                        // `confined_server` here would answer a narrower
                        // question — whether the definition on disk names the
                        // wrapper — and report a plugin's bare server as
                        // unsandboxed when every session APEX starts confines
                        // it. The opposite mistake is the one this module
                        // exists to refuse, so neither is acceptable: the
                        // three-way answer is the only true one.
                        let wrap = launch
                            .decisions
                            .iter()
                            .find(|d| d.name == s.name)
                            .and_then(|d| d.confined);
                        (s.name.clone(), wrap)
                    })
                    .collect();

                r.plugins.push(Plugin {
                    key: key.clone(),
                    name: name.clone(),
                    marketplace_known: known_names.iter().any(|k| k == &marketplace),
                    marketplace: marketplace.clone(),
                    version: entry
                        .get("version")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    installed_at: entry
                        .get("installedAt")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    enabled: enabled.iter().any(|e| e == key),
                    confinement: Confinement {
                        mcp,
                        executables: w.executables().iter().map(|s| s.to_string()).collect(),
                        hooks: w.files.keys().any(|f| f.starts_with("hooks/")),
                        skills: w
                            .files
                            .keys()
                            .filter(|f| f.starts_with("skills/"))
                            .filter_map(|f| f.split('/').nth(1))
                            .collect::<std::collections::BTreeSet<_>>()
                            .len(),
                    },
                    excluded: w.excluded.clone(),
                    digest: d,
                    attest,
                    install_path,
                });
            }
        }
    }
    r.plugins.sort_by(|a, b| a.key.cmp(&b.key));
    r
}

/// Read one registry file, recording a refusal rather than losing it.
fn read_registry(path: &Path, unreadable: &mut Vec<String>) -> Option<Value> {
    match std::fs::read_to_string(path) {
        Ok(text) => match serde_json::from_str(&text) {
            Ok(v) => Some(v),
            Err(e) => {
                unreadable.push(format!("{} is not valid JSON: {e}", path.display()));
                None
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => {
            unreadable.push(format!("{} exists and could not be read: {e}", path.display()));
            None
        }
    }
}

// ═════════════════════════════════════════════════════════════════════════════
//  output
// ═════════════════════════════════════════════════════════════════════════════

pub fn to_json(r: &Report) -> Value {
    json!({
        "marketplaces": r.marketplaces.iter().map(Marketplace::to_json).collect::<Vec<_>>(),
        "plugins": r.plugins.iter().map(Plugin::to_json).collect::<Vec<_>>(),
        "store": {
            "path": r.baselines.path.display().to_string(),
            "recorded": r.baselines.digests.len(),
            "error": r.baselines.error,
        },
        // Said in the data, not only in prose: the baseline is one APEX took
        // the first time it looked, so a match proves the tree has not changed
        // since then and nothing about what the marketplace intended.
        "trustOnFirstUse": true,
        "signed": false,
        "unreadable": r.unreadable,
        "complete": r.complete(),
        "findings": r.findings(),
    })
}

pub fn render(r: &Report) -> String {
    let mut out = String::new();

    out.push_str("MARKETPLACES — where plugins come from\n");
    if r.marketplaces.is_empty() {
        out.push_str("  none recorded\n");
    }
    for m in &r.marketplaces {
        out.push_str(&format!("  {}\n", m.name));
        out.push_str(&format!(
            "    source     {}\n",
            match (&m.source_kind, &m.source) {
                (Some(k), Some(s)) => format!("{k}: {s}"),
                (Some(k), None) => k.clone(),
                _ => "not recorded".to_string(),
            }
        ));
        out.push_str(&format!("    revision   {}\n", m.revision.describe()));
    }

    out.push_str("\nPLUGINS — what is installed, and whether it is still what arrived\n");
    if r.plugins.is_empty() {
        out.push_str("  none installed\n");
    }
    for p in &r.plugins {
        out.push_str(&format!(
            "  {}{}\n",
            p.key,
            if p.enabled { "" } else { "  (not enabled)" }
        ));
        out.push_str(&format!(
            "    version    {}\n",
            p.version.as_deref().unwrap_or("not recorded")
        ));
        out.push_str(&format!(
            "    digest     {}\n",
            // `short()` is an em dash when there is no digest, which in a
            // column reads as "nothing to say". There is something to say —
            // why it could not be measured — so the unmeasured arm prints it.
            if p.digest.is_measured() {
                p.digest.short()
            } else {
                p.digest.describe()
            }
        ));
        out.push_str(&format!("    files      {}\n", p.attest.describe()));
        if !p.excluded.is_empty() {
            out.push_str(&format!(
                "    excluded   {} (runtime state, deliberately outside the digest)\n",
                p.excluded.join(" ")
            ));
        }
        for line in p.confinement.lines() {
            out.push_str(&format!("    isolation  {line}\n"));
        }
    }

    let findings = r.findings();
    if findings.is_empty() {
        out.push_str("\nno findings.\n");
    } else {
        out.push_str(&format!("\n{} finding(s):\n", findings.len()));
        for f in &findings {
            out.push_str(&format!("  {f}\n"));
        }
    }

    // The limit, in the output rather than only in the source. Without this
    // paragraph "unchanged" reads as "verified", which it is not.
    out.push_str(
        "\nWhat a match does and does not prove. Nobody signs a Claude Code plugin and the\n\
         registry holds no hash, so the only baseline available is the one APEX recorded\n\
         the first time it looked. \"unchanged\" therefore means the tree has not been\n\
         modified SINCE it was recorded — it says nothing about whether the tree was what\n\
         the marketplace intended when it arrived. A marketplace that is a git checkout is\n\
         the one case that can do better, and its revision line says whether the working\n\
         tree still matches its commit.\n\
         \n\
         On isolation: an MCP server is the only plugin content APEX confines on its own,\n\
         and each line above says WHEN — a definition carrying the wrapper is confined\n\
         however you start the agent, while one the launch configuration wraps is confined\n\
         only in a session `apex agent` started. Hooks and executable files a plugin ships\n\
         get no sandbox from APEX at all; they run inside whatever the session itself is\n\
         confined to, which under `--sandbox unrestricted` is nothing.\n",
    );
    out
}

// ═════════════════════════════════════════════════════════════════════════════
//  verbs
// ═════════════════════════════════════════════════════════════════════════════

/// `apex provenance show`
///
/// Exits non-zero for a finding **or** for an incomplete report, which is the
/// convention `apex skill list` and `apex agent profile doctor` already hold.
pub fn show(json: bool) -> i32 {
    let home = crate::mcp::home();
    let r = build(&home);
    if json {
        match serde_json::to_string_pretty(&to_json(&r)) {
            Ok(text) => println!("{text}"),
            Err(e) => {
                eprintln!("apex provenance: the report could not be serialised: {e}");
                return 1;
            }
        }
    } else {
        print!("{}", render(&r));
    }
    i32::from(!r.findings().is_empty() || !r.complete())
}

/// `apex provenance record`
///
/// Writes the current digest of every installed plugin as the baseline to
/// compare against later. A tree that could not be hashed is **not** recorded:
/// a store entry that came from a partial walk would be a baseline that never
/// matched again, and the run says which were skipped rather than leaving a
/// reader to compare counts.
pub fn record() -> i32 {
    match record_inner() {
        Ok(code) => code,
        Err(e) => {
            eprintln!("apex provenance record: {e:#}");
            1
        }
    }
}

fn record_inner() -> Result<i32> {
    let home = crate::mcp::home();
    let r = build(&home);
    let path = store_path(&home);

    if let Some(e) = &r.baselines.error {
        // Refusing to overwrite a store that could not be read: it may hold
        // records this run cannot reproduce, and clobbering it would destroy
        // the only evidence that a plugin had changed.
        anyhow::bail!(
            "the existing store at {} could not be read ({e}), and overwriting it would \
             destroy records this cannot reproduce. Move it aside if that is what you want.",
            path.display()
        );
    }

    let mut kept = r.baselines.digests.clone();
    let mut wrote = 0usize;
    let mut skipped: Vec<&str> = Vec::new();
    let mut changed: Vec<&str> = Vec::new();
    for p in &r.plugins {
        match &p.digest {
            Digest::Sha256(h) => {
                if p.attest.is_finding() {
                    changed.push(&p.key);
                }
                kept.insert(p.key.clone(), h.clone());
                wrote += 1;
            }
            Digest::Unmeasured(_) => skipped.push(&p.key),
        }
    }

    let doc = json!({
        "version": 1,
        "trustOnFirstUse": true,
        "plugins": kept.iter()
            .map(|(k, v)| (k.clone(), json!({"sha256": v})))
            .collect::<serde_json::Map<String, Value>>(),
    });
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&path, serde_json::to_string_pretty(&doc)?)?;

    println!("recorded {wrote} plugin digest(s) in {}", path.display());
    if !changed.is_empty() {
        // Named loudly, and on stderr: re-recording a changed tree is how a
        // real finding gets erased, and stdout is where a script's `| jq` or
        // `> baseline.txt` sends it to be ignored.
        eprintln!(
            "NOTE: {} of them had CHANGED since the last record, and this has just made the \
             new content the baseline: {}",
            changed.len(),
            changed.join(" ")
        );
    }
    if !skipped.is_empty() {
        println!(
            "{} could not be hashed and were NOT recorded: {}",
            skipped.len(),
            skipped.join(" ")
        );
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "apex-prov-{}-{tag}-{:?}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("fixture");
        dir
    }

    /// The throwaway baseline store for a fixture home.
    ///
    /// Named, never taken from `$XDG_STATE_HOME`: see [`store_path`]. Every
    /// test below goes through [`build_at`] with this, so nothing in this file
    /// can read or write the store of the machine it is running on.
    fn store_in(home: &Path) -> PathBuf {
        home.join("store/plugin-provenance.json")
    }

    /// Wait until a script this test just wrote can actually be exec'd.
    ///
    /// ETXTBSY, and it is not hypothetical. This binary is ~600 tests in ONE
    /// process, threaded, and every test that spawns a child forks. A fork
    /// landing between the `open` and the `close` inside the `std::fs::write`
    /// that created this script gives that child a WRITE handle on it, and
    /// `execve` refuses any file somebody holds open for writing. The child
    /// closes it a moment later when it execs its own program, so the window is
    /// microseconds wide and it is real: measured on 2026-09-12 over the whole
    /// binary, 2 failures in 60 runs, and 0 in 60 with `--test-threads=1`.
    ///
    /// That is the flake this file carried for three rounds. It was
    /// undiagnosable from its own panic, which said the fixture
    /// "is not a git checkout and carries no revision file" about a directory
    /// the test had just written a git into — `revision_of_with` took a git
    /// that could not START the same way as a git that ran and said no. The
    /// `GitErr` split above fixed that, and the panic then read
    /// `git could not be run: Text file busy (os error 26)`.
    ///
    /// Retrying is the whole fix and it terminates: once `fs::write` has
    /// returned, no thread holds a write fd on this path, so no FUTURE fork can
    /// inherit one. Only children forked during the write can be holding it,
    /// and each releases it the instant it execs. One successful exec therefore
    /// proves the window is shut for good.
    fn wait_until_executable(prog: &Path) {
        for _ in 0..200 {
            match Command::new(prog).arg("--probe").output() {
                // 26 is ETXTBSY. Spelled as a raw errno and not as
                // `io::ErrorKind::ExecutableFileBusy`, which needs Rust 1.83
                // and this workspace declares `rust-version = "1.75"`.
                Err(e) if e.raw_os_error() == Some(26) => {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                _ => return,
            }
        }
        panic!(
            "{} was still Text file busy after a second of retries",
            prog.display()
        );
    }

    /// A machine with one marketplace and one installed plugin.
    fn machine(home: &Path, install: &Path) {
        std::fs::create_dir_all(home.join(".claude/plugins")).expect("mkdir");
        std::fs::write(
            home.join(".claude/plugins/known_marketplaces.json"),
            json!({"mk": {
                "source": {"source": "github", "repo": "o/r"},
                "installLocation": home.join(".claude/plugins/marketplaces/mk")
                    .display().to_string(),
                "lastUpdated": "2026-01-01T00:00:00.000Z",
            }})
            .to_string(),
        )
        .expect("write");
        std::fs::write(
            home.join(".claude/plugins/installed_plugins.json"),
            json!({"version": 2, "plugins": {
                "p@mk": [{"scope": "user", "installPath": install.display().to_string(),
                          "version": "1.0.0", "installedAt": "2026-01-01T00:00:00.000Z"}]
            }})
            .to_string(),
        )
        .expect("write");
        std::fs::write(
            home.join(".claude/settings.json"),
            json!({"enabledPlugins": {"p@mk": true}}).to_string(),
        )
        .expect("write");
    }

    #[test]
    fn a_changed_file_is_a_finding_and_an_untouched_tree_is_not() {
        // P1-026's first criterion, and the difference between provenance and
        // a label: the recorded digest is re-checked against the bytes.
        let home = fixture("attest");
        let install = home.join("tree");
        std::fs::create_dir_all(&install).expect("mkdir");
        std::fs::write(install.join("go.sh"), "#!/bin/sh\necho one\n").expect("write");
        machine(&home, &install);
        let store = store_in(&home);

        // First look: nothing recorded, and that is NOT a pass.
        let r = build_at(&home, &store);
        assert_eq!(r.plugins.len(), 1, "{:?}", r.plugins);
        assert_eq!(r.plugins[0].attest, Attest::FirstSight);
        assert_eq!(to_json(&r)["plugins"][0]["attest"]["unchanged"], Value::Null);

        // Record, then an untouched tree matches.
        let digest_then = r.plugins[0].digest.clone();
        std::fs::create_dir_all(store.parent().unwrap()).expect("mkdir");
        std::fs::write(
            &store,
            json!({"version": 1, "plugins": {"p@mk": {"sha256": match &digest_then {
                Digest::Sha256(h) => h.clone(), _ => panic!("unmeasured"),
            }}}})
            .to_string(),
        )
        .expect("write");
        let r = build_at(&home, &store);
        assert_eq!(r.plugins[0].attest, Attest::Match);
        assert!(r.findings().is_empty(), "{:?}", r.findings());
        assert!(r.complete(), "a fully measured machine is a complete report");

        // Edit the script: a finding.
        std::fs::write(install.join("go.sh"), "#!/bin/sh\necho two\n").expect("write");
        let r = build_at(&home, &store);
        assert!(
            matches!(r.plugins[0].attest, Attest::Changed { .. }),
            "{:?}",
            r.plugins[0].attest
        );
        assert!(
            r.findings().iter().any(|f| f.contains("CHANGED")),
            "{:?}",
            r.findings()
        );
        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn runtime_state_inside_the_tree_does_not_count_as_a_change() {
        // The trap this module would otherwise have walked into: Claude Code
        // writes `.in_use/<pid>` INSIDE the installed plugin, so a digest that
        // covered it would report a change every time the plugin was used and
        // the check would be worthless.
        let home = fixture("volatile");
        let install = home.join("tree");
        std::fs::create_dir_all(&install).expect("mkdir");
        std::fs::write(install.join("plugin.json"), "{}\n").expect("write");
        machine(&home, &install);
        let store = store_in(&home);

        let before = build_at(&home, &store).plugins[0].digest.clone();
        std::fs::create_dir_all(install.join(".in_use")).expect("mkdir");
        std::fs::write(install.join(".in_use/4242"), "").expect("write");
        let after = build_at(&home, &store);
        assert_eq!(
            before.same_as(&after.plugins[0].digest),
            Some(true),
            "a live-process marker must not change the digest"
        );
        // And the exclusion is reported, not silent.
        assert!(
            after.plugins[0].excluded.contains(&".in_use".to_string()),
            "{:?}",
            after.plugins[0].excluded
        );
        assert!(render(&after).contains("excluded"), "{}", render(&after));
        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn a_git_marketplace_gets_a_real_commit_and_a_non_git_one_gets_a_claim() {
        let root = fixture("rev");

        // Not a repository and no revision file: unmeasured, with the reason.
        let bare = root.join("bare");
        std::fs::create_dir_all(&bare).expect("mkdir");
        let rev = revision_of(&bare);
        assert!(matches!(rev, Revision::Unmeasured(_)), "{rev:?}");
        assert!(rev.describe().starts_with("none:"), "{}", rev.describe());

        // A revision file beside it is a CLAIM, and says so.
        std::fs::write(bare.join(".gcs-sha"), "8f5c9d3f86ccaeedbaefd66b039cfb3743775e0e\n")
            .expect("write");
        let rev = revision_of(&bare);
        match &rev {
            Revision::Claimed { sha, from } => {
                assert_eq!(sha, "8f5c9d3f86ccaeedbaefd66b039cfb3743775e0e");
                assert_eq!(from, ".gcs-sha");
            }
            other => panic!("{other:?}"),
        }
        assert!(rev.describe().contains("claimed by"), "{}", rev.describe());
        assert_eq!(rev.to_json()["recheckable"], Value::Bool(false));

        // A real checkout: a commit, and whether the tree still matches it.
        let repo = root.join("repo");
        std::fs::create_dir_all(&repo).expect("mkdir");
        let ok = Command::new("git")
            .arg("-C").arg(&repo).args(["init", "-q"]).status().map(|s| s.success())
            .unwrap_or(false);
        // NOT `if ok { … }`. A draft of this skipped the whole commit arm when
        // git was missing, so the only re-checkable origin claim in the feature
        // had an assertion that vanished instead of failing. git is a hard
        // dependency of what is under test here — this repo is built with cargo
        // from a git checkout, so it is present wherever this runs.
        assert!(ok, "git could not init a repository, so the commit arm cannot be tested");
        {
            std::fs::write(repo.join("a"), "one\n").expect("write");
            for args in [
                vec!["config", "user.email", "t@t"],
                vec!["config", "user.name", "t"],
                vec!["add", "a"],
                vec!["commit", "-qm", "one"],
            ] {
                Command::new("git").arg("-C").arg(&repo).args(&args).status().ok();
            }
            let rev = revision_of(&repo);
            match &rev {
                Revision::Commit { sha, dirty, .. } => {
                    assert_eq!(sha.len(), 40, "{sha}");
                    assert_eq!(*dirty, Some(false), "a fresh commit is not dirty");
                }
                other => panic!("{other:?}"),
            }
            assert!(rev.describe().contains("still matches"), "{}", rev.describe());
            assert_eq!(rev.to_json()["recheckable"], Value::Bool(true));

            // Modified working tree: the one re-checkable origin claim in the
            // whole feature, and it must notice.
            std::fs::write(repo.join("a"), "two\n").expect("write");
            let rev = revision_of(&repo);
            assert!(
                matches!(rev, Revision::Commit { dirty: Some(true), .. }),
                "{rev:?}"
            );
            assert!(rev.describe().contains("MODIFIED"), "{}", rev.describe());
        }
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_store_that_cannot_be_read_is_not_a_machine_with_nothing_recorded() {
        // The defect shape, at the level that matters most: an unreadable
        // store makes every plugin read as FirstSight, which is exactly what a
        // deleted store looks like. It must be a finding, not silence.
        let home = fixture("store");
        let install = home.join("tree");
        std::fs::create_dir_all(&install).expect("mkdir");
        std::fs::write(install.join("x"), "x\n").expect("write");
        machine(&home, &install);
        let store = store_in(&home);
        std::fs::create_dir_all(store.parent().unwrap()).expect("mkdir");
        std::fs::write(&store, "{ this is not json").expect("write");

        let r = build_at(&home, &store);
        assert!(r.baselines.error.is_some());
        let findings = r.findings();
        assert!(
            findings.iter().any(|f| f.contains("clean bill of health")),
            "{findings:?}"
        );
        assert_eq!(to_json(&r)["complete"], Value::Bool(false));
        // And the per-plugin verdict is the one about this machine, not the one
        // about the records: `verify.rs`'s CouldNotRun, not its Absent. Called
        // "not recorded" it would read as a plugin nobody had got round to
        // recording yet, on a machine whose records had just been destroyed.
        assert!(
            matches!(r.plugins[0].attest, Attest::Unmeasurable(_)),
            "an unreadable store must not make a plugin read as first-sight: {:?}",
            r.plugins[0].attest
        );
        assert_eq!(
            to_json(&r)["plugins"][0]["attest"]["unchanged"],
            Value::Null,
            "neither a pass nor a failure"
        );
        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn a_plugin_tree_that_cannot_be_hashed_makes_the_report_incomplete() {
        // The verify.rs lesson at report level: could-not-check is neither a
        // pass nor a failure, and a report that quietly omits a plugin reads
        // exactly like one where that plugin was fine. The registry naming an
        // installPath that is not there is the realistic way in — `doctor`'s
        // `installPath.is_dir()` is what noticed it before this.
        let home = fixture("unhashable");
        machine(&home, &home.join("tree-that-was-never-created"));
        let r = build_at(&home, &store_in(&home));
        assert_eq!(r.plugins.len(), 1);
        assert!(
            matches!(r.plugins[0].attest, Attest::Unmeasurable(_)),
            "{:?}",
            r.plugins[0].attest
        );
        assert!(!r.plugins[0].attest.is_finding(), "not a failure");
        assert!(r.plugins[0].attest.reason().is_some(), "and it carries why");
        assert!(!r.complete(), "a plugin nobody could hash is not a complete report");
        assert_eq!(to_json(&r)["complete"], Value::Bool(false));
        assert!(
            r.findings().iter().any(|f| f.contains("does NOT cover")),
            "{:?}",
            r.findings()
        );
        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn a_documentation_only_plugin_is_not_reported_as_sandboxed() {
        // `everythingExecutableIsSandboxed` is vacuously true when there is
        // nothing to confine, so a consumer reading only that field would call
        // a Markdown-only plugin sandboxed. `hasExecutableContent` is what
        // makes the pair readable.
        let c = Confinement::default();
        assert!(!c.has_executable_content());
        let j = c.to_json();
        assert_eq!(j["everythingExecutableIsSandboxed"], Value::Bool(true));
        assert_eq!(j["hasExecutableContent"], Value::Bool(false));
        assert!(
            c.lines().join(" ").contains("no process of its own to confine"),
            "{:?}",
            c.lines()
        );
        // And with something to confine, the flag says so.
        let c = Confinement {
            executables: vec!["go.sh".into()],
            ..Default::default()
        };
        assert!(c.has_executable_content());
        assert_eq!(c.to_json()["hasExecutableContent"], Value::Bool(true));
    }

    #[test]
    fn an_unreadable_registry_makes_the_report_incomplete() {
        let home = fixture("registry");
        std::fs::create_dir_all(home.join(".claude/plugins")).expect("mkdir");
        std::fs::write(
            home.join(".claude/plugins/installed_plugins.json"),
            "{ not json",
        )
        .expect("write");
        let r = build_at(&home, &store_in(&home));
        assert_eq!(r.unreadable.len(), 1, "{:?}", r.unreadable);
        assert!(
            r.findings().iter().any(|f| f.contains("INCOMPLETE")),
            "{:?}",
            r.findings()
        );
        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn the_report_never_calls_unwrapped_plugin_content_sandboxed() {
        // P1-026's second criterion, kept honest. The word "sandbox" may only
        // appear for content something actually confines, and never without
        // the clause saying when.
        let c = Confinement {
            mcp: vec![("plugin:p:srv".into(), Some(Wrap::Not))],
            executables: vec!["scripts/go.sh".into()],
            hooks: true,
            skills: 0,
        };
        let lines = c.lines().join("\n");
        assert!(lines.contains("NOT sandboxed"), "{lines}");
        assert!(
            lines.contains("nothing here adds a sandbox of its own"),
            "hooks must be stated as getting no sandbox from APEX:\n{lines}"
        );
        assert!(
            lines.contains("`unrestricted`"),
            "and the case where the session has none either:\n{lines}"
        );
        let j = c.to_json();
        assert_eq!(j["everythingExecutableIsSandboxed"], Value::Bool(false));
        assert_eq!(j["unconfinedExecutableContent"], Value::Bool(true));

        // A server whose own definition carries the wrapper is confined
        // wherever it is started, and gets the word without qualification.
        let c = Confinement {
            mcp: vec![("plugin:p:srv".into(), Some(Wrap::InDefinition))],
            ..Default::default()
        };
        let lines = c.lines().join("\n");
        assert!(lines.contains("sandboxed however the agent is started"), "{lines}");
        let j = c.to_json();
        assert_eq!(j["everythingExecutableIsSandboxed"], Value::Bool(true));
        assert_eq!(
            j["everythingExecutableIsSandboxedWithoutApexAgent"],
            Value::Bool(true)
        );
        assert_eq!(j["mcpServers"][0]["sandboxSurvivesAHandRun"], Value::Bool(true));
    }

    /// The shortfall names its own remedy, and only where there is one.
    ///
    /// Round 2 of this unit deleted five sentences that told a reader a
    /// control did not exist on a tip that had built it. This is the same
    /// defect in the other direction — a shortfall reported with no remedy —
    /// and it is the one that was live until dimension 8 got a flag.
    #[test]
    fn content_no_sandbox_reaches_is_reported_with_the_thing_that_does_reach_it() {
        let c = Confinement {
            hooks: true,
            ..Default::default()
        };
        let lines = c.lines().join("\n");
        assert!(
            lines.contains("--plugins none"),
            "a reader told their hooks are unconfined must be told what removes them:\n{lines}"
        );
        assert!(
            lines.contains("--plugins curated"),
            "and that keeping some is a choice, not all-or-nothing:\n{lines}"
        );
        // REMOVES, not confines. The asymmetry is the honest part: APEX does
        // not own the agent's process. A line that said "sandboxes" here would
        // be the third time this repository spent that word wrongly.
        assert!(
            !lines.contains("--plugins none` sandboxes"),
            "dimension 8 removes; it must never be described as confining:\n{lines}"
        );
        assert_eq!(c.to_json()["removableByPluginPolicy"], Value::Bool(true));

        // Loose executables are the same case and get the same clause.
        let c = Confinement {
            executables: vec!["scripts/go.sh".into()],
            ..Default::default()
        };
        assert!(c.lines().join("\n").contains("--plugins none"));
        assert_eq!(c.to_json()["removableByPluginPolicy"], Value::Bool(true));

        // And a plugin whose only content is an MCP server does NOT get it.
        // Dimension 7 already confines that, and offering removal as the
        // remedy would overstate the shortfall — the mirror of the defect this
        // test exists for.
        let c = Confinement {
            mcp: vec![("plugin:p:srv".into(), Some(Wrap::AtLaunch))],
            ..Default::default()
        };
        let lines = c.lines().join("\n");
        assert!(
            !lines.contains("--plugins"),
            "content dimension 7 confines must not be advertised as needing removal:\n{lines}"
        );
        assert_eq!(c.to_json()["removableByPluginPolicy"], Value::Bool(false));

        // Nor does a plugin that ships nothing executable at all.
        let c = Confinement::default();
        assert!(!c.lines().join("\n").contains("--plugins"));
        assert_eq!(c.to_json()["removableByPluginPolicy"], Value::Bool(false));
    }

    #[test]
    fn a_sandbox_the_launch_configuration_supplies_is_not_reported_as_the_plugins_own() {
        // The state that did not exist before the curator did, and the one a
        // boolean gets wrong in BOTH directions. `servers::confined_server`
        // looks at the definition on disk and would call this bare server
        // unsandboxed — false of every session `apex agent` starts. Calling it
        // sandboxed is false of every session anybody else starts. Only the
        // three-way answer is true of both.
        let c = Confinement {
            mcp: vec![("plugin:p:srv".into(), Some(Wrap::AtLaunch))],
            ..Default::default()
        };
        let lines = c.lines().join("\n");
        assert!(
            lines.contains("sandboxed in a session `apex agent` starts"),
            "{lines}"
        );
        assert!(
            lines.contains("Start the agent yourself"),
            "the qualifier is the point of this arm:\n{lines}"
        );
        assert!(!lines.contains("NOT sandboxed"), "{lines}");

        let j = c.to_json();
        // Sandboxed where APEX starts it, and NOT where anybody else does.
        // The two fields differing is the whole content of this test.
        assert_eq!(j["everythingExecutableIsSandboxed"], Value::Bool(true));
        assert_eq!(
            j["everythingExecutableIsSandboxedWithoutApexAgent"],
            Value::Bool(false)
        );
        assert_eq!(j["mcpServers"][0]["sandboxed"], Value::Bool(true));
        assert_eq!(j["mcpServers"][0]["sandboxedBy"], "launch");
        assert_eq!(j["mcpServers"][0]["sandboxSurvivesAHandRun"], Value::Bool(false));
        assert_eq!(j["unconfinedExecutableContent"], Value::Bool(false));
    }

    #[test]
    fn a_plugins_bare_server_is_read_out_of_the_launcher_and_not_off_the_definition() {
        // End to end, on a fixture `$HOME`: an enabled plugin whose `.mcp.json`
        // names a bare program. Nothing on disk wraps it, so the old
        // definition-only test answers "not sandboxed" — and the session the
        // daemon starts confines it. The report must carry the launcher's
        // answer, which is the only one true of a real session.
        let home = fixture("launchwrap");
        let install = home.join("tree");
        std::fs::create_dir_all(&install).expect("mkdir");
        std::fs::write(
            install.join(".mcp.json"),
            json!({"mcpServers": {"srv": {"command": "node", "args": ["s.js"]}}}).to_string(),
        )
        .expect("write");
        machine(&home, &install);
        std::fs::write(
            home.join(".claude/settings.json"),
            json!({"enabledPlugins": {"p@mk": true}}).to_string(),
        )
        .expect("write");

        let r = build_at(&home, &store_in(&home));
        let p = r.plugins.iter().find(|p| p.key == "p@mk").expect("the plugin");
        assert_eq!(
            p.confinement.mcp,
            vec![("plugin:p:srv".to_string(), Some(Wrap::AtLaunch))],
            "a plugin's bare server is wrapped at launch, not on disk"
        );
        // And the definition on disk really is bare, so this is not the
        // wrapped case wearing a different name.
        assert!(
            servers::confined_server("node", &["s.js".to_string()]).is_none(),
            "the fixture must be a definition nothing on disk wraps"
        );
        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn a_version_of_the_literal_string_unknown_is_reported_as_no_version() {
        // Two of this machine's plugins really do record "unknown".
        let home = fixture("unknown");
        let install = home.join("tree");
        std::fs::create_dir_all(&install).expect("mkdir");
        std::fs::write(install.join("x"), "x\n").expect("write");
        machine(&home, &install);
        std::fs::write(
            home.join(".claude/plugins/installed_plugins.json"),
            json!({"version": 2, "plugins": {
                "p@mk": [{"installPath": install.display().to_string(), "version": "unknown"}]
            }})
            .to_string(),
        )
        .expect("write");
        let r = build_at(&home, &store_in(&home));
        assert!(
            r.findings().iter().any(|f| f.contains("literal string")),
            "{:?}",
            r.findings()
        );
        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn a_git_status_that_could_not_run_is_not_a_clean_working_tree() {
        // The arm no test reached before. `rev-parse` answers, `status` does
        // not — a repository under a lock, a git killed by a resource limit, a
        // filesystem that went away mid-command. `dirty` must stay `None`:
        // `Some(false)` would print "its working tree still matches it" about a
        // tree nobody looked at, which is the one sentence a reader would act
        // on.
        let root = fixture("gitfail");
        let fake = root.join("fakegit");
        std::fs::write(
            &fake,
            "#!/bin/sh\nfor a in \"$@\"; do\n  [ \"$a\" = rev-parse ] && { \
             echo 01a9cec39b961236e2d99fa2db9b22b534fa27a9; exit 0; }\n  \
             [ \"$a\" = status ] && exit 128\ndone\nexit 128\n",
        )
        .expect("write");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755))
                .expect("chmod");
        }
        wait_until_executable(&fake);
        let rev = revision_of_with(&root, fake.to_str().expect("utf8"));
        match &rev {
            Revision::Commit { dirty, .. } => assert_eq!(
                *dirty, None,
                "a status that could not run must not read as a clean tree"
            ),
            other => panic!("{other:?}"),
        }
        assert!(
            rev.describe().contains("could not be checked"),
            "and the report must say so: {}",
            rev.describe()
        );
        assert!(
            !rev.describe().contains("still matches it"),
            "it must never claim the tree matches: {}",
            rev.describe()
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_git_that_could_not_be_started_is_not_a_directory_that_is_not_a_checkout() {
        // The collapse this module exists to refuse, in the last place it was
        // still being made. `revision_of_with` took EVERY `Err` from git the
        // same way and fell through to "is not a git checkout and carries no
        // revision file" — a statement about the DIRECTORY, made when the only
        // thing that failed was starting the program. The directory might be a
        // perfectly good checkout; nobody looked.
        //
        // Not academic. It is what hid the ETXTBSY flake in
        // `a_git_status_that_could_not_run_…` for three rounds: the panic named
        // a fixture "not a git checkout" about a directory the test had just
        // written a git into, so the one word that would have pointed at exec —
        // busy — never appeared.
        let root = fixture("nogit");
        let missing = root.join("there-is-no-git-here");
        let rev = revision_of_with(&root, missing.to_str().expect("utf8"));
        match &rev {
            Revision::Unmeasured(why) => {
                assert!(
                    why.contains("could not be run"),
                    "a git that could not be started must say that: {why}"
                );
                assert!(
                    !why.contains("not a git checkout"),
                    "and must not report it as a directory that is not a repository: {why}"
                );
            }
            other => panic!("{other:?}"),
        }
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_git_that_ran_and_said_no_is_still_a_directory_that_is_not_a_checkout() {
        // The other half of the same split, and the reason it is a split rather
        // than "treat every git failure as unmeasured". A git that RUNS and
        // exits non-zero has measured something: the directory is not a
        // repository. That answer must keep falling through to the revision
        // file, or every plain directory carrying a `.gcs-sha` would stop
        // reporting the revision it does have.
        let root = fixture("refusedgit");
        let fake = root.join("fakegit");
        std::fs::write(&fake, "#!/bin/sh\nexit 128\n").expect("write");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755))
                .expect("chmod");
        }
        wait_until_executable(&fake);
        std::fs::write(
            root.join(".gcs-sha"),
            "01a9cec39b961236e2d99fa2db9b22b534fa27a9\n",
        )
        .expect("write");
        match revision_of_with(&root, fake.to_str().expect("utf8")) {
            Revision::Claimed { sha, from } => {
                assert_eq!(from, ".gcs-sha");
                assert!(sha.starts_with("01a9cec"), "{sha}");
            }
            other => panic!("a git that ran and refused must not stop the fallback: {other:?}"),
        }
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_registry_that_is_there_and_refused_is_not_a_registry_that_is_absent() {
        // `read_registry`'s permission arm. A registry file that cannot be read
        // makes the report INCOMPLETE; dropped to `None` it would instead be a
        // machine with no plugins installed, which is the reassuring answer and
        // the wrong one.
        let home = fixture("refused");
        std::fs::create_dir_all(home.join(".claude/plugins")).expect("mkdir");
        let reg = home.join(".claude/plugins/installed_plugins.json");
        std::fs::write(&reg, "{\"version\": 2, \"plugins\": {}}").expect("write");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&reg, std::fs::Permissions::from_mode(0o000))
                .expect("chmod");
        }
        if std::fs::read_to_string(&reg).is_ok() {
            // root, or CAP_DAC_OVERRIDE. Say so rather than passing quietly.
            eprintln!("skipped: this user reads a 0000 file");
        } else {
            let r = build_at(&home, &store_in(&home));
            assert_eq!(r.unreadable.len(), 1, "{:?}", r.unreadable);
            assert!(
                r.unreadable[0].contains("could not be read"),
                "{:?}",
                r.unreadable
            );
            assert!(!r.complete(), "a refused registry is not a complete report");
            assert!(
                r.findings().iter().any(|f| f.contains("INCOMPLETE")),
                "{:?}",
                r.findings()
            );
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&reg, std::fs::Permissions::from_mode(0o644)).ok();
        }
        std::fs::remove_dir_all(&home).ok();
    }

    #[test]
    fn the_json_says_it_is_trust_on_first_use_and_not_signed() {
        // The limit, machine-checkable rather than only in a paragraph.
        let home = fixture("tofu");
        let j = to_json(&build_at(&home, &store_in(&home)));
        assert_eq!(j["trustOnFirstUse"], Value::Bool(true));
        assert_eq!(j["signed"], Value::Bool(false));
        std::fs::remove_dir_all(&home).ok();
    }
}
