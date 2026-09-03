//! The declarative APEX Blueprint: schema, validation, and the pure planner.
//!
//! Roadmap §10 asks for NixOS-style reproducibility through an APEX-native
//! model that a person can read. One TOML file says what the machine should be;
//! `apex apply` moves the machine toward it; `apex sync` carries it to another
//! APEX machine.
//!
//! ── Three kinds of state, and why they are three files ──────────────────────
//!
//! §10's own bullet — "keep generated/system state separate from user-owned
//! blueprint state" — is the requirement that this module is shaped around, so
//! it is worth being explicit about what the three are:
//!
//!   1. **Desired** — [`Blueprint`]. Hand-written, user-owned, the only file a
//!      person or a future GUI edits, and the only file `apex sync` carries.
//!      Nothing in APEX ever rewrites it behind the user's back.
//!   2. **Observed** — [`Observed`]. What the machine actually is *right now*.
//!      Probed live on every `diff` and never cached, because a cache is how a
//!      converger ends up reporting success against a stale picture.
//!   3. **Applied** — [`AppliedState`]. Generated. Written only by `apex apply`,
//!      never read back as desire, and carrying a header that says so.
//!
//! The temptation is to collapse 2 and 3 into one "current state" file that
//! `diff` reads. That is the mistake: it makes `diff` agree with `apply` by
//! construction rather than by measurement, so a step that silently did nothing
//! reports as converged forever.
//!
//! ── What this module does NOT do ────────────────────────────────────────────
//!
//! No I/O. Nothing here reads a file, spawns a process or touches the machine —
//! it parses text, validates it, and turns (desired, observed) into an ordered
//! list of [`Step`]s. The `apex` CLI owns the probing and the executing, the
//! same split `apexd-core` already uses for tiers: this crate plans, the writer
//! touches. That is what lets every branch below be unit-tested with no machine
//! at all.

use std::collections::BTreeSet;
use std::fmt;

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

/// Schema version of the blueprint file format.
///
/// Bumped only when an older `apex` could misread a newer file *silently*. A
/// purely additive key does not need it, because unknown keys are rejected
/// loudly (see [`Blueprint`]) rather than ignored.
pub const SCHEMA_VERSION: u32 = 1;

// ── vocabularies ─────────────────────────────────────────────────────────────
//
// These are closed sets on purpose. A blueprint's whole value is that a typo is
// loud: `compositor = "hyperland"` must be a refusal at parse time, not a
// successful `apply` that changed nothing. Every list below is asserted against
// the real vocabulary it mirrors — see the parity tests in `apex/src/blueprint.rs`
// for the two that live in another crate.

/// Compositors a blueprint may ask for, and the installed session id each one
/// resolves to.
///
/// The two spellings differ and both are load-bearing. The roadmap (and the
/// user) says `labwc`; the shipped session is `apex-labwc.desktop`, because
/// `Containerfile.base` removes the stock `labwc.desktop` and installs APEX's
/// own. Mapping here rather than making the user write `apex-labwc` keeps the
/// blueprint portable across a rename of the desktop file.
pub const COMPOSITORS: [(&str, &str); 3] = [
    ("hyprland", "hyprland"),
    ("niri", "niri"),
    ("labwc", "apex-labwc"),
];

/// Colour schemes `[desktop] theme` may name.
///
/// These are matugen's scheme names, which is what APEX Shell's
/// `WallpaperService.schemes` offers and what it writes into
/// `wallpaper.json`. §10's example says `theme = "material"`; there is no such
/// scheme — the whole palette is Material You already, and the choice the shell
/// actually exposes is *which* Material scheme is derived from the wallpaper.
/// Naming a value the shell cannot honour would be a blueprint that applies
/// cleanly and changes nothing.
pub const THEMES: [&str; 6] = [
    "content",
    "tonal-spot",
    "fidelity",
    "fruit-salad",
    "neutral",
    "monochrome",
];

/// Agent ids `[agent] default` may name. Mirrors
/// `apex_agent_core::adapter::ALL`; the parity test in the `apex` crate fails if
/// the two ever drift.
pub const AGENTS: [&str; 6] = ["claude", "opencode", "codex", "gemini", "kimi", "generic"];

/// Sandbox policies `[agent] sandbox` may name. Mirrors
/// `apex_agent_core::protocol::SandboxPolicy`.
pub const SANDBOX_POLICIES: [&str; 3] = ["unrestricted", "project", "strict"];

/// Languages `[development] languages` may name.
///
/// Validated but **not converged** in phase 7. Convergence belongs to phase 6's
/// `apex env` capsules, which are being built on a parallel branch; a
/// language -> package table here would be a second, conflicting answer to the
/// same question. So the schema, the validation and the diff are real, and
/// `apply` reports the section as deferred rather than guessing at a toolchain
/// install. Validating anyway is deliberate: a user writing `typscript` today
/// should find out today, not when phase 6 lands.
pub const LANGUAGES: [&str; 8] = [
    "c",
    "cpp",
    "go",
    "javascript",
    "python",
    "rust",
    "shell",
    "typescript",
];

/// The longest an application or package name may be. Longer than any real
/// Fedora package or Flatpak id, short enough that a synced bundle cannot carry
/// a kilobyte of argv into the package engine.
const MAX_NAME: usize = 128;

// ── the blueprint itself ─────────────────────────────────────────────────────

/// The user-owned declaration of what this machine should be.
///
/// Every field is optional, and absent means **unmanaged**, not "set to the
/// default". A blueprint with no `[desktop]` table must never converge the
/// desktop — a declarative model that silently asserts defaults for everything
/// it does not mention is one that reformats a machine the first time it runs.
///
/// `deny_unknown_fields` throughout. This is the opposite choice from
/// `apex_agent_core::config::Config`, which keeps unknown keys with
/// `#[serde(flatten)] extra` — and the reason for the difference is who writes
/// the file. `agent.json` is machine-written by two programs (the CLI and APEX
/// Shell) that may be different versions, so dropping a key it does not know
/// would lose the other program's setting. The blueprint is hand-written by a
/// person, so an unrecognised key is a typo, and swallowing it would defeat the
/// point of the file.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Blueprint {
    /// File-format version. Absent means [`SCHEMA_VERSION`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<u32>,
    #[serde(default, skip_serializing_if = "Desktop::is_empty")]
    pub desktop: Desktop,
    #[serde(default, skip_serializing_if = "Apps::is_empty")]
    pub apps: Apps,
    #[serde(default, skip_serializing_if = "Development::is_empty")]
    pub development: Development,
    #[serde(default, skip_serializing_if = "AgentPrefs::is_empty")]
    pub agent: AgentPrefs,
    #[serde(default, skip_serializing_if = "Gaming::is_empty")]
    pub gaming: Gaming,
}

/// `[desktop]` — which session the greeter offers first, and the shell's colour
/// scheme.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Desktop {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compositor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub theme: Option<String>,
}

/// `[apps]` — software that should be present.
///
/// One list, not one per source, exactly as §10 writes it. The entries are
/// classified the same way `apex install` classifies its arguments: a
/// reverse-DNS id (`org.gimp.GIMP`) is a Flatpak, anything else is a package
/// name. Keeping one list means a blueprint does not have to know which source
/// APEX happens to use for a given application today.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Apps {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub install: Vec<String>,
}

/// `[development]` — declared, diffed, and deferred to phase 6 for convergence.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Development {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub languages: Vec<String>,
}

/// `[agent]` — the agent runtime's two user preferences.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AgentPrefs {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox: Option<String>,
}

/// `[gaming]` — observed and reported, never converged. See [`Step`] for why.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Gaming {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
}

impl Desktop {
    fn is_empty(&self) -> bool {
        self.compositor.is_none() && self.theme.is_none()
    }
}
impl Apps {
    fn is_empty(&self) -> bool {
        self.install.is_empty()
    }
}
impl Development {
    fn is_empty(&self) -> bool {
        self.languages.is_empty()
    }
}
impl AgentPrefs {
    fn is_empty(&self) -> bool {
        self.default.is_none() && self.sandbox.is_none()
    }
}
impl Gaming {
    fn is_empty(&self) -> bool {
        self.enabled.is_none()
    }
}

impl Blueprint {
    /// Parse and validate a blueprint.
    ///
    /// Both halves are errors, never warnings. A blueprint that parses but
    /// names a compositor that does not exist is worse than one that fails to
    /// parse, because it converges "successfully" and changes nothing.
    pub fn parse(text: &str) -> Result<Blueprint> {
        let mut bp: Blueprint = toml::from_str(text).map_err(|e| {
            // toml's own message already carries the line and column; the
            // prefix is what tells the user which file it is about.
            anyhow::anyhow!("not a valid blueprint: {e}")
        })?;
        bp.normalise();
        let problems = bp.validate();
        if !problems.is_empty() {
            bail!("{}", problems.join("\n"));
        }
        Ok(bp)
    }

    /// Render back to TOML, ready to write.
    pub fn to_toml(&self) -> Result<String> {
        Ok(toml::to_string_pretty(self)?)
    }

    /// Drop duplicate list entries, preserving first-seen order.
    ///
    /// A duplicate is a harmless mistake, not a policy question, so it is
    /// corrected rather than refused. Order is preserved because the list is a
    /// human's file and reordering it would fight the next hand edit.
    fn normalise(&mut self) {
        dedup_preserving_order(&mut self.apps.install);
        dedup_preserving_order(&mut self.development.languages);
    }

    /// Everything wrong with this blueprint, as user-facing lines.
    pub fn validate(&self) -> Vec<String> {
        let mut out = Vec::new();

        if let Some(v) = self.version {
            if v != SCHEMA_VERSION {
                out.push(format!(
                    "version = {v} is not a schema this build understands \
                     (expected {SCHEMA_VERSION})"
                ));
            }
        }

        if let Some(c) = &self.desktop.compositor {
            if session_for_compositor(c).is_none() {
                out.push(format!(
                    "[desktop] compositor = {c:?} is not one of: {}",
                    COMPOSITORS
                        .iter()
                        .map(|(name, _)| *name)
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
        }
        if let Some(t) = &self.desktop.theme {
            if !THEMES.contains(&t.as_str()) {
                out.push(format!(
                    "[desktop] theme = {t:?} is not one of: {}",
                    THEMES.join(", ")
                ));
            }
        }

        for name in &self.apps.install {
            if let Err(why) = check_app_name(name) {
                out.push(format!("[apps] install entry {name:?}: {why}"));
            }
        }

        for lang in &self.development.languages {
            if !LANGUAGES.contains(&lang.as_str()) {
                out.push(format!(
                    "[development] languages entry {lang:?} is not one of: {}",
                    LANGUAGES.join(", ")
                ));
            }
        }

        if let Some(a) = &self.agent.default {
            if !AGENTS.contains(&a.as_str()) {
                out.push(format!(
                    "[agent] default = {a:?} is not one of: {}",
                    AGENTS.join(", ")
                ));
            }
        }
        if let Some(s) = &self.agent.sandbox {
            if !SANDBOX_POLICIES.contains(&s.as_str()) {
                out.push(format!(
                    "[agent] sandbox = {s:?} is not one of: {}",
                    SANDBOX_POLICIES.join(", ")
                ));
            }
        }

        out
    }

    /// A short content digest, used to notice that the blueprint changed since
    /// the last `apply`.
    ///
    /// FNV-1a, and deliberately not a cryptographic hash: nothing here is a
    /// trust decision. It answers "is this the same text I applied last time",
    /// and adding a `sha2` dependency to the core crate to answer that would be
    /// a cost with no matching benefit. Named `digest`, never `checksum`, so it
    /// is not mistaken for the package engine's verification hashes.
    pub fn digest(&self) -> String {
        let text = self.to_toml().unwrap_or_default();
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for b in text.as_bytes() {
            h ^= u64::from(*b);
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        format!("{h:016x}")
    }

    /// The Flatpak ids in `[apps] install`.
    pub fn flatpak_ids(&self) -> Vec<String> {
        self.apps
            .install
            .iter()
            .filter(|n| is_flatpak_id(n))
            .cloned()
            .collect()
    }

    /// The RPM package names in `[apps] install`.
    pub fn package_names(&self) -> Vec<String> {
        self.apps
            .install
            .iter()
            .filter(|n| !is_flatpak_id(n))
            .cloned()
            .collect()
    }
}

/// The installed session id for a blueprint compositor name.
pub fn session_for_compositor(name: &str) -> Option<&'static str> {
    COMPOSITORS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, session)| *session)
}

/// The blueprint compositor name for an installed session id — the inverse of
/// [`session_for_compositor`], used when reporting what the machine currently
/// has.
pub fn compositor_for_session(session: &str) -> Option<&'static str> {
    COMPOSITORS
        .iter()
        .find(|(_, s)| *s == session)
        .map(|(n, _)| *n)
}

/// Whether an `[apps] install` entry is a Flatpak application id.
///
/// This is the shipped package engine's rule, transcribed: `apex-pkg`'s
/// `is_flatpak_id` is
///
/// ```text
/// [[ "$1" =~ ^[A-Za-z][A-Za-z0-9_-]*(\.[A-Za-z][A-Za-z0-9_-]*){2,}$ ]]
/// ```
///
/// — three or more dot-separated segments, each starting with a letter. The
/// planner has to classify independently (it compares against `flatpak list`
/// and against the engine's requested list, which are different sources), but
/// it must not classify *differently*, or a blueprint would report an app as
/// missing forever while the engine kept installing it somewhere else.
/// `tests/test-apex-blueprint.sh` asserts the two agree on the same inputs.
pub fn is_flatpak_id(name: &str) -> bool {
    let segments: Vec<&str> = name.split('.').collect();
    segments.len() >= 3
        && segments.iter().all(|s| {
            let mut chars = s.chars();
            matches!(chars.next(), Some(c) if c.is_ascii_alphabetic())
                && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
        })
}

/// Why an application name is unacceptable, if it is.
///
/// This is a hostile-input boundary, not a nicety. A blueprint arrives over
/// `apex sync import` from another machine, and these names become argv for the
/// package engine — which runs as root and builds paths under
/// `/var/lib/apex/pkg/local` from them. The engine has its own guard; this is
/// the one that stops a bad name being written into the local blueprint in the
/// first place.
fn check_app_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("is empty".into());
    }
    if name.len() > MAX_NAME {
        return Err(format!("is longer than {MAX_NAME} characters"));
    }
    if name.starts_with('-') {
        // `apex install -rf` must never be reachable from a synced file.
        return Err("starts with '-', which would be read as a command-line flag".into());
    }
    if name.starts_with('.') {
        return Err("starts with '.'".into());
    }
    if name.ends_with(".rpm") {
        // `apex install ./foo.rpm` is a real and supported thing; a blueprint
        // entry is not the place for it. The whole value of the file is that
        // it reproduces on another machine, and a local .rpm is exactly the
        // input that does not travel — the other machine has no such file, and
        // `sync` cannot carry a binary.
        return Err(
            "names a local .rpm; a blueprint must reproduce on another machine, so use \
             `apex install ./file.rpm` for that and keep the blueprint to repository \
             packages and Flatpak ids"
                .into(),
        );
    }
    if let Some(bad) = name
        .chars()
        .find(|c| !(c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '+' | '-')))
    {
        return Err(format!(
            "contains {bad:?}; only letters, digits and . _ + - are allowed"
        ));
    }
    Ok(())
}

fn dedup_preserving_order(items: &mut Vec<String>) {
    let mut seen = BTreeSet::new();
    items.retain(|i| seen.insert(i.clone()));
}

// ── observed state ───────────────────────────────────────────────────────────

/// What the machine actually is. Filled in by the CLI's probes; a fixture in
/// tests.
///
/// Every field is what was *measured*, so `None` means "could not be
/// determined", never "unset". The distinction matters: a diff against an
/// unreadable value must say so rather than plan a change from nothing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Observed {
    /// Session id the greeter will preselect (`/var/lib/apex-greet/last-session`).
    pub session: Option<String>,
    /// Session ids installed in `/usr/share/wayland-sessions`.
    pub sessions_available: Vec<String>,
    /// APEX Shell's matugen scheme.
    pub theme: Option<String>,
    /// RPM package names installed through the APEX extension engine.
    pub packages: Vec<String>,
    /// Flatpak application ids installed.
    pub flatpaks: Vec<String>,
    /// Toolchains detected on `PATH`.
    pub languages: Vec<String>,
    /// `default_agent` from the agent runtime's config.
    pub agent_default: Option<String>,
    /// `sandbox` from the agent runtime's config.
    pub agent_sandbox: Option<String>,
    /// `VARIANT_ID` from `/etc/os-release`.
    pub variant_id: Option<String>,
}

impl Observed {
    /// Whether this machine carries the gaming session, which is what actually
    /// distinguishes a Gaming edition from Daily at the session level.
    pub fn has_gaming_session(&self) -> bool {
        self.sessions_available
            .iter()
            .any(|s| s == GAMING_SESSION)
    }
}

/// The session id the Gaming editions install.
pub const GAMING_SESSION: &str = "apex-gaming";

// ── the plan ─────────────────────────────────────────────────────────────────

/// Which privilege domain a step belongs to.
///
/// `apply` never escalates. It converges the domain it is already running in
/// and reports the other, because the alternative — shelling out to `sudo` —
/// raises an authentication prompt, and because a root `apex apply` that also
/// wrote user config would either write it into `/root` or leave root-owned
/// files in the user's `~/.config` where the shell can no longer update them.
/// Both are silent failures. See [`Plan::steps_for`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Domain {
    /// Files in the invoking user's home. Converged by a normal `apex apply`.
    User,
    /// System state. Converged by `sudo apex apply`.
    Root,
}

impl Domain {
    pub const fn as_str(self) -> &'static str {
        match self {
            Domain::User => "user",
            Domain::Root => "root",
        }
    }
}

/// One concrete thing `apply` would do.
///
/// Every variant maps onto a primitive APEX already ships, which is the reason
/// there is no `Step` for `[gaming]` or `[development]`:
///
///   * **gaming** — `enabled = true` asks for a machine provisioned for games,
///     and that provisioning is an *image*: the Gaming editions carry the
///     session, the drivers and the low-latency tuning. No command converts
///     Daily into Gaming, and installing a bag of gaming packages onto Daily
///     would be exactly the edition leakage the repo contract forbids. So it is
///     observed, diffed and reported — never converged.
///   * **development** — belongs to phase 6's `apex env` capsules.
///
/// A [`Change`] for either carries `step: None` and a `blocked` reason, so the
/// user is told plainly instead of being shown a converged machine that is not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// Point the greeter at a session. Does NOT log anyone out: the blueprint
    /// declares which desktop this machine uses, and ending the user's session
    /// to prove it would be a destructive reading of a declarative file.
    SelectSession { session: String },
    /// Set APEX Shell's matugen colour scheme.
    SetTheme { scheme: String },
    /// Install RPM packages through the APEX extension engine.
    InstallPackages { names: Vec<String> },
    /// Install Flatpak applications.
    InstallFlatpaks { ids: Vec<String> },
    /// Set the agent runtime's default agent.
    SetAgentDefault { agent: String },
    /// Set the agent runtime's default sandbox policy.
    SetAgentSandbox { policy: String },
}

impl Step {
    /// Which privilege domain can perform this step.
    pub const fn domain(&self) -> Domain {
        match self {
            Step::SelectSession { .. } | Step::InstallPackages { .. } | Step::InstallFlatpaks { .. } => {
                Domain::Root
            }
            Step::SetTheme { .. } | Step::SetAgentDefault { .. } | Step::SetAgentSandbox { .. } => {
                Domain::User
            }
        }
    }
}

impl fmt::Display for Step {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Step::SelectSession { session } => write!(f, "select session {session}"),
            Step::SetTheme { scheme } => write!(f, "set colour scheme to {scheme}"),
            Step::InstallPackages { names } => write!(f, "install packages: {}", names.join(" ")),
            Step::InstallFlatpaks { ids } => write!(f, "install flatpaks: {}", ids.join(" ")),
            Step::SetAgentDefault { agent } => write!(f, "set default agent to {agent}"),
            Step::SetAgentSandbox { policy } => write!(f, "set agent sandbox to {policy}"),
        }
    }
}

/// One difference between desired and observed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    /// `[desktop] compositor`, `[apps] install`, …
    pub what: String,
    /// What the machine has now, rendered for a human.
    pub current: String,
    /// What the blueprint asks for.
    pub desired: String,
    /// The step that would close the gap, if one exists.
    pub step: Option<Step>,
    /// Why no step exists. Always `Some` when `step` is `None`.
    pub blocked: Option<String>,
}

impl Change {
    /// Which domain must run to close this gap.
    pub fn domain(&self) -> Option<Domain> {
        self.step.as_ref().map(Step::domain)
    }
}

/// The full result of comparing a blueprint to a machine.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Plan {
    pub changes: Vec<Change>,
}

impl Plan {
    /// True when the machine already matches the blueprint in every respect
    /// that can be converged. Blocked changes are excluded on purpose: a Daily
    /// machine asked for `[gaming] enabled = true` would otherwise never report
    /// converged no matter how many times `apply` ran, which would make the
    /// exit code useless as a signal.
    pub fn is_converged(&self) -> bool {
        self.changes.iter().all(|c| c.step.is_none())
    }

    /// Changes that cannot be converged at all, with their reasons.
    pub fn blocked(&self) -> Vec<&Change> {
        self.changes.iter().filter(|c| c.step.is_none()).collect()
    }

    /// The steps for one privilege domain, in order.
    pub fn steps_for(&self, domain: Domain) -> Vec<&Step> {
        self.changes
            .iter()
            .filter_map(|c| c.step.as_ref())
            .filter(|s| s.domain() == domain)
            .collect()
    }

    /// Every step, in order, regardless of domain.
    pub fn steps(&self) -> Vec<&Step> {
        self.changes.iter().filter_map(|c| c.step.as_ref()).collect()
    }
}

/// Compare a blueprint to a machine.
///
/// Pure: same inputs, same plan, every time. `apply` executes exactly the steps
/// this returns and nothing else — a dry run and a live run call this once each
/// and differ only in whether the resulting steps are handed to a converger
/// that touches anything. That is what makes `--dry-run` a real report rather
/// than a rehearsal of a different program.
pub fn plan(bp: &Blueprint, obs: &Observed) -> Plan {
    let mut changes = Vec::new();

    // ── [desktop] ───────────────────────────────────────────────────────────
    if let Some(want) = &bp.desktop.compositor {
        // Validation has already run, so the mapping is present; being
        // defensive here rather than unwrapping keeps a caller that skipped
        // validate() from panicking.
        if let Some(session) = session_for_compositor(want) {
            let now = obs.session.as_deref();
            let now_name = now
                .and_then(compositor_for_session)
                .or(now)
                .unwrap_or("unknown");
            if now != Some(session) {
                // A session that is not installed cannot be selected —
                // `apex-session-select` validates against the shipped
                // .desktop files and would refuse. Say so here instead of
                // planning a step that is guaranteed to fail.
                let (step, blocked) = if obs.sessions_available.is_empty() {
                    // No /usr/share/wayland-sessions at all: a container, a CI
                    // runner, a development checkout. Planning a step here
                    // would guarantee a failure, because apex-session-select
                    // validates against the very list that is empty.
                    (
                        None,
                        Some(
                            "no Wayland sessions are installed; this is not an APEX \
                             desktop, so the greeter has nothing to preselect"
                                .to_string(),
                        ),
                    )
                } else if obs.sessions_available.iter().any(|s| s == session) {
                    (Some(Step::SelectSession { session: session.into() }), None)
                } else {
                    (
                        None,
                        Some(format!(
                            "session {session:?} is not installed on this machine"
                        )),
                    )
                };
                changes.push(Change {
                    what: "[desktop] compositor".into(),
                    current: now_name.into(),
                    desired: want.clone(),
                    step,
                    blocked,
                });
            }
        }
    }

    if let Some(want) = &bp.desktop.theme {
        if obs.theme.as_deref() != Some(want.as_str()) {
            changes.push(Change {
                what: "[desktop] theme".into(),
                current: obs.theme.clone().unwrap_or_else(|| "unknown".into()),
                desired: want.clone(),
                step: Some(Step::SetTheme { scheme: want.clone() }),
                blocked: None,
            });
        }
    }

    // ── [apps] ──────────────────────────────────────────────────────────────
    //
    // Additive only. A package that is installed but no longer named in the
    // blueprint is NOT removed: the blueprint declares what must be present,
    // and reading "absent from the list" as "uninstall it" turns a forgotten
    // line into data loss on a machine the user shares with the file. Phase 10
    // of the roadmap can revisit that with an explicit `prune` verb; it must
    // never be the default.
    let missing_pkgs: Vec<String> = bp
        .package_names()
        .into_iter()
        .filter(|n| !obs.packages.iter().any(|p| p == n))
        .collect();
    if !missing_pkgs.is_empty() {
        changes.push(Change {
            what: "[apps] install (packages)".into(),
            current: format!("{} of {} present", bp.package_names().len() - missing_pkgs.len(), bp.package_names().len()),
            desired: missing_pkgs.join(" "),
            step: Some(Step::InstallPackages { names: missing_pkgs }),
            blocked: None,
        });
    }

    let missing_flatpaks: Vec<String> = bp
        .flatpak_ids()
        .into_iter()
        .filter(|n| !obs.flatpaks.iter().any(|p| p == n))
        .collect();
    if !missing_flatpaks.is_empty() {
        changes.push(Change {
            what: "[apps] install (flatpaks)".into(),
            current: format!(
                "{} of {} present",
                bp.flatpak_ids().len() - missing_flatpaks.len(),
                bp.flatpak_ids().len()
            ),
            desired: missing_flatpaks.join(" "),
            step: Some(Step::InstallFlatpaks { ids: missing_flatpaks }),
            blocked: None,
        });
    }

    // ── [development] ───────────────────────────────────────────────────────
    let missing_langs: Vec<String> = bp
        .development
        .languages
        .iter()
        .filter(|l| !obs.languages.iter().any(|d| d == *l))
        .cloned()
        .collect();
    if !missing_langs.is_empty() {
        changes.push(Change {
            what: "[development] languages".into(),
            current: if obs.languages.is_empty() {
                "none detected".into()
            } else {
                obs.languages.join(" ")
            },
            desired: missing_langs.join(" "),
            step: None,
            blocked: Some(
                "development environments are phase 6 (`apex env` capsules); \
                 the blueprint records and diffs them, but does not install \
                 toolchains"
                    .into(),
            ),
        });
    }

    // ── [agent] ─────────────────────────────────────────────────────────────
    if let Some(want) = &bp.agent.default {
        if obs.agent_default.as_deref() != Some(want.as_str()) {
            changes.push(Change {
                what: "[agent] default".into(),
                current: obs.agent_default.clone().unwrap_or_else(|| "unknown".into()),
                desired: want.clone(),
                step: Some(Step::SetAgentDefault { agent: want.clone() }),
                blocked: None,
            });
        }
    }
    if let Some(want) = &bp.agent.sandbox {
        if obs.agent_sandbox.as_deref() != Some(want.as_str()) {
            changes.push(Change {
                what: "[agent] sandbox".into(),
                current: obs.agent_sandbox.clone().unwrap_or_else(|| "unknown".into()),
                desired: want.clone(),
                step: Some(Step::SetAgentSandbox { policy: want.clone() }),
                blocked: None,
            });
        }
    }

    // ── [gaming] ────────────────────────────────────────────────────────────
    if let Some(want) = bp.gaming.enabled {
        let have = obs.has_gaming_session();
        if want != have {
            let variant = obs.variant_id.clone().unwrap_or_else(|| "unknown".into());
            changes.push(Change {
                what: "[gaming] enabled".into(),
                current: have.to_string(),
                desired: want.to_string(),
                step: None,
                blocked: Some(if want {
                    format!(
                        "gaming provisioning comes from a Gaming edition image, not a \
                         package set; this machine is VARIANT_ID={variant}. Reinstall or \
                         rebase onto gaming-mesa / gaming-nvidia."
                    )
                } else {
                    format!(
                        "this machine boots a Gaming edition image (VARIANT_ID={variant}); \
                         removing the gaming session would leave a Gaming image without its \
                         session. Rebase onto daily instead."
                    )
                }),
            });
        }
    }

    Plan { changes }
}

// ── generated state ──────────────────────────────────────────────────────────

/// The record `apex apply` leaves behind. Generated — never a source of truth.
///
/// It exists to answer two questions the blueprint file cannot: when this
/// machine last converged, and against which version of the blueprint. It is
/// deliberately NOT consulted by [`plan`]; see this module's header for why a
/// cached "current state" would make `diff` lie.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AppliedState {
    pub schema: u32,
    /// Seconds since the Unix epoch. A plain integer rather than a formatted
    /// timestamp so that no date library is needed to write it and no locale
    /// can change how it reads.
    pub applied_at: u64,
    /// `user` or `root` — which half of the plan this run was able to converge.
    pub domain: String,
    /// [`Blueprint::digest`] of the blueprint that was applied.
    pub blueprint_digest: String,
    /// Steps actually performed, rendered.
    #[serde(default)]
    pub steps: Vec<String>,
    /// Steps that were planned and then failed, rendered with their reason.
    #[serde(default)]
    pub failures: Vec<String>,
}

/// The comment block written above [`AppliedState`], so that anyone who opens
/// the file knows it is not the one to edit.
pub const APPLIED_STATE_HEADER: &str = "\
# GENERATED by `apex apply`. Do not edit.
#
# This is the machine's record of the last convergence, not a declaration of
# intent. The file you edit is the blueprint; `apex blueprint show` prints
# where it lives. Deleting this file loses history and nothing else — the next
# `apex diff` re-measures the machine from scratch either way.
";

impl AppliedState {
    /// Render with the header attached.
    pub fn to_toml(&self) -> Result<String> {
        Ok(format!("{APPLIED_STATE_HEADER}{}", toml::to_string_pretty(self)?))
    }

    /// Parse, tolerating the header (TOML comments are ignored by the parser).
    pub fn parse(text: &str) -> Result<AppliedState> {
        Ok(toml::from_str(text)?)
    }
}

// ── sync bundles ─────────────────────────────────────────────────────────────

/// What `apex sync export` writes and `apex sync import` reads.
///
/// ── What a bundle deliberately does NOT carry ───────────────────────────────
///
/// No credentials of any kind: nothing from the secret broker's store, no
/// grants, no privilege-request audit log, no SSH or cloud configuration, no
/// keyring material. `sync` reproduces *settings, applications and which
/// projects exist* — the three things §10 names — and a bundle is a file people
/// will put in a git repository or e-mail to themselves, so it has to stay
/// boring enough for that to be safe.
///
/// Project entries carry a slug, a path and a git remote. The path is the one
/// piece of genuinely machine-specific data, kept because a project's location
/// is usually the same on both of one person's machines and because `import`
/// only records it — it never creates or writes a directory.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Bundle {
    pub bundle: BundleMeta,
    pub blueprint: Blueprint,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub projects: Vec<ProjectRef>,
}

/// Provenance for a bundle. Informational — `import` trusts none of it beyond
/// the schema check.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BundleMeta {
    pub schema: u32,
    /// Seconds since the Unix epoch.
    pub created: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_variant: Option<String>,
}

/// One project, as `apex project` knows it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectRef {
    pub slug: String,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<String>,
}

impl Bundle {
    /// Parse and validate a bundle received from another machine.
    ///
    /// Hostile input: this file came from somewhere else. The blueprint inside
    /// goes through the same validation as a hand-written one, and every
    /// project entry is checked before it can be recorded.
    pub fn parse(text: &str) -> Result<Bundle> {
        let b: Bundle =
            toml::from_str(text).map_err(|e| anyhow::anyhow!("not a valid sync bundle: {e}"))?;
        if b.bundle.schema != SCHEMA_VERSION {
            bail!(
                "bundle schema {} is not one this build understands (expected {SCHEMA_VERSION})",
                b.bundle.schema
            );
        }
        let problems = b.blueprint.validate();
        if !problems.is_empty() {
            bail!("the bundle's blueprint is not valid:\n{}", problems.join("\n"));
        }
        for p in &b.projects {
            if let Err(why) = check_project(p) {
                bail!("bundle project {:?}: {why}", p.slug);
            }
        }
        Ok(b)
    }

    pub fn to_toml(&self) -> Result<String> {
        Ok(toml::to_string_pretty(self)?)
    }
}

/// Why a bundle's project entry is unacceptable, if it is.
///
/// The slug becomes a filename under the runtime's project directory, so it is
/// held to the same shape `apex project` uses. The path is never created or
/// written by `import`, but it is recorded and later shown to the user, so it
/// must at least be absolute and free of traversal — a relative or `..`-bearing
/// path recorded here would resolve against whatever directory a later command
/// happened to run in.
fn check_project(p: &ProjectRef) -> Result<(), String> {
    if p.slug.is_empty() || p.slug.len() > MAX_NAME {
        return Err("slug is empty or too long".into());
    }
    if !p
        .slug
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        || p.slug.starts_with('.')
    {
        return Err("slug must be letters, digits, - _ . and may not start with '.'".into());
    }
    if !p.path.starts_with('/') {
        return Err("path must be absolute".into());
    }
    if p.path.split('/').any(|seg| seg == "..") {
        return Err("path contains '..'".into());
    }
    // A synced project path pointing into the image or into another user's home
    // is either a mistake or an attempt to get a later command to act there.
    for forbidden in ["/usr/", "/etc/", "/boot/", "/sys/", "/proc/", "/dev/"] {
        if p.path.starts_with(forbidden) {
            return Err(format!("path is under {forbidden}, which is not a project location"));
        }
    }
    if let Some(remote) = &p.remote {
        if remote.len() > 512 || remote.contains(['\n', '\r', '\0']) {
            return Err("remote is too long or contains control characters".into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// §10's example, verbatim apart from the blank line the roadmap's own
    /// rendering lost between `[agent]` and `[gaming]`. If this ever stops
    /// parsing, the schema has drifted from the thing it was asked for.
    const ROADMAP_EXAMPLE: &str = r#"
[desktop]
compositor = "labwc"
theme = "material"

[apps]
install = ["firefox", "obsidian", "steam"]

[development]
languages = ["python", "rust", "typescript"]

[agent]
default = "claude"
sandbox = "project"

[gaming]
enabled = true
"#;

    #[test]
    fn the_roadmap_example_parses_except_for_its_illustrative_theme() {
        // Every table, key and value in §10 is accepted — except `theme =
        // "material"`, which names a scheme APEX Shell does not have. That is
        // not the schema being wrong: accepting it would produce a blueprint
        // that applies cleanly and changes no colour, which is the exact
        // failure a declarative model exists to prevent.
        let err = Blueprint::parse(ROADMAP_EXAMPLE).expect_err("theme should be refused");
        let msg = err.to_string();
        assert!(msg.contains("[desktop] theme"), "{msg}");
        assert!(msg.contains("tonal-spot"), "the refusal must list the real schemes: {msg}");
        assert!(
            !msg.contains("compositor") && !msg.contains("[apps]") && !msg.contains("[agent]"),
            "only the theme should be rejected, got: {msg}"
        );

        // With a real scheme the whole example is accepted.
        let fixed = ROADMAP_EXAMPLE.replace("\"material\"", "\"content\"");
        let bp = Blueprint::parse(&fixed).expect("the rest of §10's example must parse");
        assert_eq!(bp.desktop.compositor.as_deref(), Some("labwc"));
        assert_eq!(bp.apps.install, ["firefox", "obsidian", "steam"]);
        assert_eq!(bp.development.languages, ["python", "rust", "typescript"]);
        assert_eq!(bp.agent.default.as_deref(), Some("claude"));
        assert_eq!(bp.agent.sandbox.as_deref(), Some("project"));
        assert_eq!(bp.gaming.enabled, Some(true));
    }

    #[test]
    fn an_unknown_key_is_a_refusal_not_a_shrug() {
        // The whole point of the file. A blueprint that silently ignores
        // `compositer = "niri"` reports a converged machine that never changed.
        let err = Blueprint::parse("[desktop]\ncompositer = \"niri\"\n").unwrap_err();
        assert!(err.to_string().contains("compositer"), "{err}");

        let err = Blueprint::parse("[deskotp]\ncompositor = \"niri\"\n").unwrap_err();
        assert!(err.to_string().contains("deskotp"), "{err}");
    }

    #[test]
    fn unknown_enum_values_are_refused_with_the_real_list() {
        for (text, needle) in [
            ("[desktop]\ncompositor = \"hyperland\"\n", "hyprland"),
            ("[agent]\ndefault = \"claud\"\n", "claude"),
            ("[agent]\nsandbox = \"sandboxed\"\n", "unrestricted"),
            ("[development]\nlanguages = [\"typscript\"]\n", "typescript"),
        ] {
            let err = Blueprint::parse(text).unwrap_err();
            assert!(
                err.to_string().contains(needle),
                "{text:?} should be refused with a list containing {needle}, got: {err}"
            );
        }
    }

    #[test]
    fn an_empty_blueprint_is_valid_and_manages_nothing() {
        // Absent must mean unmanaged. If it meant "the default", the first
        // `apply` on a machine with a stub blueprint would reset the desktop.
        let bp = Blueprint::parse("").expect("an empty blueprint is legal");
        let obs = Observed {
            session: Some("niri".into()),
            theme: Some("neutral".into()),
            agent_default: Some("codex".into()),
            ..Observed::default()
        };
        assert!(plan(&bp, &obs).changes.is_empty());
    }

    #[test]
    fn a_hostile_app_name_never_reaches_the_package_engine() {
        for bad in [
            "-rf",
            "../../etc/passwd",
            "foo bar",
            "foo;rm -rf /",
            "$(id)",
            ".hidden",
            "",
            // A local file is supported by `apex install` and meaningless in a
            // blueprint, which has to reproduce on a machine that does not
            // have the file.
            "some-vendor-driver.rpm",
        ] {
            let text = format!("[apps]\ninstall = [{bad:?}]\n");
            assert!(
                Blueprint::parse(&text).is_err(),
                "{bad:?} must be refused as an app name"
            );
        }
        // …and ordinary names still work, including Flatpak ids.
        Blueprint::parse("[apps]\ninstall = [\"firefox\", \"org.gimp.GIMP\", \"gcc-c++\"]\n")
            .expect("real package names must be accepted");
    }

    #[test]
    fn flatpak_ids_are_split_out_the_same_way_apex_install_splits_them() {
        let bp = Blueprint::parse(
            "[apps]\ninstall = [\"firefox\", \"org.gimp.GIMP\", \"com.valvesoftware.Steam\", \"python3-pip\"]\n",
        )
        .unwrap();
        assert_eq!(bp.flatpak_ids(), ["org.gimp.GIMP", "com.valvesoftware.Steam"]);
        assert_eq!(bp.package_names(), ["firefox", "python3-pip"]);
    }

    #[test]
    fn the_flatpak_rule_matches_the_shipped_engines_exactly() {
        // apex-pkg's regex is ^[A-Za-z][A-Za-z0-9_-]*(\.[A-Za-z][A-Za-z0-9_-]*){2,}$
        // and its comment names the two RPM shapes that must NOT match. If this
        // drifts, a blueprint reports an application as missing forever while
        // the engine keeps installing it from the other source.
        for yes in [
            "org.gimp.GIMP",
            "io.github.foo.Bar",
            "com.valvesoftware.Steam",
            "md.obsidian.Obsidian",
        ] {
            assert!(is_flatpak_id(yes), "{yes} is a Flatpak id");
        }
        for no in [
            "firefox",
            "python3.12",         // two segments
            "java-1.8.0-openjdk", // segments starting with digits
            "gcc-c++",
            "NetworkManager-tui",
            "a.b",
        ] {
            assert!(!is_flatpak_id(no), "{no} is not a Flatpak id");
        }
    }

    #[test]
    fn a_machine_with_no_sessions_blocks_rather_than_planning_a_doomed_step() {
        // A container, a CI runner, a checkout. apex-session-select validates
        // against the list that is empty here, so a planned step could only
        // fail; say why instead.
        let bp = Blueprint::parse("[desktop]\ncompositor = \"niri\"\n").unwrap();
        let p = plan(&bp, &Observed::default());
        assert_eq!(p.changes.len(), 1);
        assert!(p.changes[0].step.is_none());
        assert!(p.changes[0]
            .blocked
            .as_deref()
            .unwrap()
            .contains("not an APEX desktop"));
    }

    #[test]
    fn duplicates_are_dropped_but_order_is_kept() {
        let bp =
            Blueprint::parse("[apps]\ninstall = [\"b\", \"a\", \"b\", \"c\", \"a\"]\n").unwrap();
        assert_eq!(bp.apps.install, ["b", "a", "c"]);
    }

    fn converged_machine() -> Observed {
        Observed {
            session: Some("apex-labwc".into()),
            sessions_available: vec!["hyprland".into(), "niri".into(), "apex-labwc".into()],
            theme: Some("content".into()),
            packages: vec!["firefox".into()],
            flatpaks: vec!["org.gimp.GIMP".into()],
            languages: vec!["rust".into()],
            agent_default: Some("claude".into()),
            agent_sandbox: Some("project".into()),
            variant_id: Some("daily".into()),
        }
    }

    const FULL: &str = r#"
[desktop]
compositor = "labwc"
theme = "content"

[apps]
install = ["firefox", "org.gimp.GIMP"]

[development]
languages = ["rust"]

[agent]
default = "claude"
sandbox = "project"
"#;

    #[test]
    fn a_converged_machine_plans_nothing() {
        // Idempotency, stated as the property it actually is: the planner is
        // pure, so "running apply twice changes nothing the second time" is
        // exactly "observed == desired plans an empty list".
        let bp = Blueprint::parse(FULL).unwrap();
        let p = plan(&bp, &converged_machine());
        assert!(p.changes.is_empty(), "unexpected changes: {:?}", p.changes);
        assert!(p.is_converged());
        assert!(p.steps().is_empty());
    }

    #[test]
    fn every_drifted_field_produces_exactly_one_step_in_the_right_domain() {
        let bp = Blueprint::parse(FULL).unwrap();
        let mut obs = converged_machine();
        obs.session = Some("niri".into());
        obs.theme = Some("neutral".into());
        obs.packages.clear();
        obs.flatpaks.clear();
        obs.agent_default = Some("codex".into());
        obs.agent_sandbox = Some("strict".into());

        let p = plan(&bp, &obs);
        assert!(!p.is_converged());
        assert_eq!(p.steps().len(), 6, "{:?}", p.changes);

        let root: Vec<String> = p.steps_for(Domain::Root).iter().map(|s| s.to_string()).collect();
        let user: Vec<String> = p.steps_for(Domain::User).iter().map(|s| s.to_string()).collect();
        assert_eq!(
            root,
            [
                "select session apex-labwc",
                "install packages: firefox",
                "install flatpaks: org.gimp.GIMP",
            ]
        );
        assert_eq!(
            user,
            [
                "set colour scheme to content",
                "set default agent to claude",
                "set agent sandbox to project",
            ]
        );
        // Every step belongs to exactly one domain, so the two lists partition
        // the plan. A step that appeared in neither would be silently skipped
        // by both `apex apply` and `sudo apex apply`.
        assert_eq!(root.len() + user.len(), p.steps().len());
    }

    #[test]
    fn apps_are_additive_never_subtractive() {
        // A package the machine has and the blueprint does not name must not
        // produce an uninstall step. Removing software because a line was
        // deleted from a text file is data loss.
        let bp = Blueprint::parse("[apps]\ninstall = [\"firefox\"]\n").unwrap();
        let obs = Observed {
            packages: vec!["firefox".into(), "htop".into(), "vim".into()],
            flatpaks: vec!["org.gimp.GIMP".into()],
            ..Observed::default()
        };
        assert!(plan(&bp, &obs).changes.is_empty());
    }

    #[test]
    fn only_the_missing_packages_are_planned() {
        let bp =
            Blueprint::parse("[apps]\ninstall = [\"firefox\", \"htop\", \"neovim\"]\n").unwrap();
        let obs = Observed {
            packages: vec!["firefox".into()],
            ..Observed::default()
        };
        let p = plan(&bp, &obs);
        assert_eq!(
            p.steps(),
            [&Step::InstallPackages {
                names: vec!["htop".into(), "neovim".into()]
            }]
        );
    }

    #[test]
    fn a_session_the_machine_does_not_have_is_blocked_not_attempted() {
        let bp = Blueprint::parse("[desktop]\ncompositor = \"labwc\"\n").unwrap();
        let obs = Observed {
            session: Some("niri".into()),
            sessions_available: vec!["hyprland".into(), "niri".into()],
            ..Observed::default()
        };
        let p = plan(&bp, &obs);
        assert_eq!(p.changes.len(), 1);
        assert!(p.changes[0].step.is_none());
        assert!(p.changes[0]
            .blocked
            .as_deref()
            .unwrap()
            .contains("not installed"));
        // Blocked is not drift you can fix, so it must not keep the machine
        // permanently "unconverged".
        assert!(p.is_converged());
    }

    #[test]
    fn gaming_is_reported_never_converged() {
        let bp = Blueprint::parse("[gaming]\nenabled = true\n").unwrap();
        let obs = Observed {
            sessions_available: vec!["hyprland".into(), "niri".into()],
            variant_id: Some("daily".into()),
            ..Observed::default()
        };
        let p = plan(&bp, &obs);
        assert_eq!(p.changes.len(), 1);
        assert!(
            p.changes[0].step.is_none(),
            "no step may install a gaming package set onto Daily"
        );
        let why = p.changes[0].blocked.as_deref().unwrap();
        assert!(why.contains("Gaming edition image"), "{why}");
        assert!(why.contains("daily"), "the reason must name the measured VARIANT_ID: {why}");

        // On a Gaming edition the same blueprint is already satisfied.
        let gaming = Observed {
            sessions_available: vec!["hyprland".into(), "apex-gaming".into()],
            variant_id: Some("gaming-mesa".into()),
            ..Observed::default()
        };
        assert!(plan(&bp, &gaming).changes.is_empty());
    }

    #[test]
    fn development_is_diffed_but_deferred_to_phase_six() {
        let bp = Blueprint::parse("[development]\nlanguages = [\"go\", \"rust\"]\n").unwrap();
        let obs = Observed {
            languages: vec!["rust".into()],
            ..Observed::default()
        };
        let p = plan(&bp, &obs);
        assert_eq!(p.changes.len(), 1);
        assert_eq!(p.changes[0].desired, "go");
        assert!(p.changes[0].step.is_none());
        assert!(p.changes[0].blocked.as_deref().unwrap().contains("phase 6"));
    }

    #[test]
    fn a_blueprint_round_trips_through_toml() {
        // `sync export` writes what `sync import` reads, and the GUI editor
        // deferred out of this phase will write this file too. A lossy
        // round-trip would quietly drop a user's setting.
        let bp = Blueprint::parse(&FULL.replace("\"content\"", "\"neutral\"")).unwrap();
        let text = bp.to_toml().unwrap();
        let again = Blueprint::parse(&text).expect("our own output must parse");
        assert_eq!(bp, again);
    }

    #[test]
    fn the_digest_tracks_content_not_formatting() {
        let a = Blueprint::parse(FULL).unwrap();
        let b = Blueprint::parse(&FULL.replace('\n', "\n\n")).unwrap();
        assert_eq!(a.digest(), b.digest(), "whitespace must not change the digest");
        let c = Blueprint::parse(&FULL.replace("\"content\"", "\"neutral\"")).unwrap();
        assert_ne!(a.digest(), c.digest(), "a changed value must change the digest");
        assert_eq!(a.digest().len(), 16);
    }

    #[test]
    fn applied_state_round_trips_and_carries_its_warning() {
        let s = AppliedState {
            schema: SCHEMA_VERSION,
            applied_at: 1_760_000_000,
            domain: "user".into(),
            blueprint_digest: "0123456789abcdef".into(),
            steps: vec!["set colour scheme to content".into()],
            failures: Vec::new(),
        };
        let text = s.to_toml().unwrap();
        assert!(text.starts_with("# GENERATED by `apex apply`. Do not edit."));
        assert_eq!(AppliedState::parse(&text).unwrap(), s);
    }

    fn bundle_text(projects: &str) -> String {
        format!(
            "[bundle]\nschema = 1\ncreated = 1760000000\n\n\
             [blueprint.desktop]\ncompositor = \"niri\"\n{projects}"
        )
    }

    #[test]
    fn a_bundle_round_trips() {
        let b = Bundle {
            bundle: BundleMeta {
                schema: SCHEMA_VERSION,
                created: 1_760_000_000,
                source_host: Some("laptop".into()),
                source_variant: Some("daily".into()),
            },
            blueprint: Blueprint::parse(FULL).unwrap(),
            projects: vec![ProjectRef {
                slug: "apex-os".into(),
                path: "/var/home/andre/Projects/apex-os".into(),
                remote: Some("git@github.com:AndreNijman/apex-os".into()),
            }],
        };
        let text = b.to_toml().unwrap();
        assert_eq!(Bundle::parse(&text).unwrap(), b);
    }

    #[test]
    fn a_bundle_from_a_newer_apex_is_refused_rather_than_half_read() {
        let text = bundle_text("").replace("schema = 1", "schema = 2");
        let err = Bundle::parse(&text).unwrap_err();
        assert!(err.to_string().contains("schema 2"), "{err}");
    }

    #[test]
    fn a_bundle_carrying_a_bad_blueprint_is_refused_as_a_whole() {
        let text = bundle_text("").replace("\"niri\"", "\"hyperland\"");
        let err = Bundle::parse(&text).unwrap_err();
        assert!(err.to_string().contains("compositor"), "{err}");
    }

    #[test]
    fn hostile_project_entries_in_a_bundle_are_refused() {
        // A bundle comes from another machine. Its project paths are recorded
        // and later shown; they must not be relative, must not traverse, and
        // must not point into the image.
        for (slug, path) in [
            ("ok", "relative/path"),
            ("ok", "/home/u/../../etc"),
            ("ok", "/usr/share/apex-shell"),
            ("ok", "/etc/systemd"),
            ("../escape", "/home/u/p"),
            (".hidden", "/home/u/p"),
            ("has space", "/home/u/p"),
        ] {
            let text = bundle_text(&format!(
                "\n[[projects]]\nslug = {slug:?}\npath = {path:?}\n"
            ));
            assert!(
                Bundle::parse(&text).is_err(),
                "slug={slug:?} path={path:?} must be refused"
            );
        }
        // A normal one is accepted.
        let text = bundle_text("\n[[projects]]\nslug = \"apex-os\"\npath = \"/home/u/apex-os\"\n");
        Bundle::parse(&text).expect("an ordinary project entry must be accepted");
    }

    #[test]
    fn compositor_names_and_session_ids_map_both_ways() {
        // The two spellings are the thing most likely to be "simplified" by a
        // later change. labwc's session really is called apex-labwc.
        assert_eq!(session_for_compositor("labwc"), Some("apex-labwc"));
        assert_eq!(compositor_for_session("apex-labwc"), Some("labwc"));
        for (name, session) in COMPOSITORS {
            assert_eq!(session_for_compositor(name), Some(session));
            assert_eq!(compositor_for_session(session), Some(name));
        }
        assert_eq!(session_for_compositor("apex-labwc"), None);
    }
}
