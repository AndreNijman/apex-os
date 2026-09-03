//! `apex blueprint` — where the blueprint lives, what the machine actually is,
//! and how the two differ.
//!
//! [`apexd_core::blueprint`] holds the schema and the pure planner and touches
//! nothing. This module is the other half: it finds the files, *measures* the
//! machine, and renders the result. The split is the same one `apexd-core`
//! already uses for power tiers — plan in the pure crate, touch from outside —
//! and it is what lets the planner be exhaustively unit-tested with no machine.
//!
//! ── The three files, and which one is authoritative ─────────────────────────
//!
//! ```text
//!   ~/.config/apex/blueprint.toml        DESIRED   — you edit this
//!   /etc/apex/blueprint.toml             DESIRED   — site default, used when
//!                                                    the user has no file
//!   ~/.local/state/apex/blueprint-state.toml
//!                                        APPLIED   — generated, never read as
//!                                                    intent
//! ```
//!
//! Observed state has no file on purpose. `diff` probes the machine every time,
//! because a cached "current state" is how a converger ends up agreeing with
//! itself: a step that silently did nothing would report converged forever.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use apexd_core::blueprint::{
    self, AppliedState, Blueprint, Change, Domain, Observed, Plan, SCHEMA_VERSION,
};

/// `/var/lib/apex-greet/last-session` — the session id the greeter preselects.
/// Written by `apex-session-select` and re-written by greetd on every
/// successful login, so it is the machine's real answer to "which desktop does
/// this box boot into".
const GREETER_SESSION: &str = "var/lib/apex-greet/last-session";

/// Where the shipped `.desktop` sessions live.
const SESSION_DIR: &str = "usr/share/wayland-sessions";

/// The package engine's requested-package list: one name per line, with local
/// `.rpm` entries recorded as `local:<NAME>`.
///
/// This, and not `state.json`, is the right source for "what did the user ask
/// to have installed" — `state.json` records the resolved transaction
/// including every dependency, so diffing a blueprint against it would report
/// convergence based on packages nobody asked for.
const REQUESTED_LIST: &str = "var/lib/apex/pkg/requested";

/// `[desktop] theme` is APEX Shell's matugen scheme, and this is where the
/// shell keeps it.
const WALLPAPER_JSON: &str = ".config/apex-shell/src/user_data/wallpaper.json";

/// The shell's own fallback when `wallpaper.json` has no `scheme`, from
/// `WallpaperService.qml`'s `property string scheme: "content"`. An absent file
/// is therefore a *measurement* of "content", not an unknown.
const SHELL_DEFAULT_SCHEME: &str = "content";

/// A blueprint to start from, written by `apex blueprint init`.
///
/// Built as a string rather than `include_str!`'d from `config/`. Two reasons:
/// `apexd-core` already `include_str!`s `config/sysprofiles/*.toml`, which is
/// the sole reason `files/scripts/clippy-local` has to mount the repository
/// root instead of the workspace, and there is no need to widen that coupling
/// for a starter file. And a committed `.toml` would be parsed by the
/// `static` job's `tomllib` sweep, which is fine here but becomes a trap the
/// moment anyone adds a deliberately-invalid example next to it.
const STARTER: &str = r#"# APEX Blueprint — what this machine should be.
#
# `apex blueprint diff` shows how the machine currently differs from this file.
# `apex apply` converges it. Nothing here is applied until you ask.
#
# Every section is optional, and leaving one out means "APEX does not manage
# this", NOT "use the default". A blueprint that asserted defaults for
# everything it did not mention would reformat a machine the first time it ran.
#
# Unknown keys are an error rather than being ignored, so a typo is loud
# instead of producing an `apex apply` that succeeds and changes nothing.

# [desktop]
# compositor = "hyprland"   # hyprland | niri | labwc
# theme = "content"         # content | tonal-spot | fidelity | fruit-salad
#                           # | neutral | monochrome

# [apps]
# Repository package names and Flatpak application ids in one list, classified
# the same way `apex install` classifies its arguments.
# install = ["firefox", "md.obsidian.Obsidian"]

# [development]
# Recorded and diffed. Toolchain installation is phase 6 (`apex env` capsules);
# until then `apex apply` reports this section rather than guessing.
# languages = ["python", "rust", "typescript"]

# [agent]
# default = "claude"        # claude | opencode | codex | gemini | kimi | generic
# sandbox = "project"       # project | strict | unrestricted

# [gaming]
# Observed and reported, never converged: gaming provisioning comes from a
# Gaming edition image, not from a package set.
# enabled = true
"#;

// ── where things live ────────────────────────────────────────────────────────
//
// `apex_agent_core::paths` already resolves `$HOME`, `$XDG_CONFIG_HOME` and
// `$XDG_STATE_HOME` correctly — including the getpwuid fallback for a process
// started without a login environment — and it is already tested. Reusing it
// beats a second implementation of the base-directory spec in this file.

/// `~/.config/apex/blueprint.toml`, or `$XDG_CONFIG_HOME`'s equivalent.
pub fn user_blueprint_path() -> PathBuf {
    apex_agent_core::paths::config_home().join("apex/blueprint.toml")
}

/// `/etc/apex/blueprint.toml` — a site default for an image or an
/// administrator, used only when the user has no blueprint of their own.
pub fn site_blueprint_path() -> PathBuf {
    PathBuf::from("/etc/apex/blueprint.toml")
}

/// `~/.local/state/apex/blueprint-state.toml`. Generated by `apex apply`.
pub fn applied_state_path() -> PathBuf {
    apex_agent_core::paths::state_home().join("apex/blueprint-state.toml")
}

/// Which file a blueprint came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// An explicit `--file`.
    Explicit(PathBuf),
    /// The user's own blueprint.
    User(PathBuf),
    /// `/etc/apex/blueprint.toml`, because the user has none.
    Site(PathBuf),
    /// Nothing on disk. An empty blueprint that manages nothing.
    None,
}

impl Source {
    pub fn path(&self) -> Option<&Path> {
        match self {
            Source::Explicit(p) | Source::User(p) | Source::Site(p) => Some(p),
            Source::None => None,
        }
    }

    fn describe(&self) -> String {
        match self {
            Source::Explicit(p) => format!("{} (--file)", p.display()),
            Source::User(p) => p.display().to_string(),
            Source::Site(p) => format!("{} (site default; you have no blueprint of your own)", p.display()),
            Source::None => format!(
                "none — no blueprint at {} and no site default at {}",
                user_blueprint_path().display(),
                site_blueprint_path().display()
            ),
        }
    }
}

/// Load the blueprint, saying where it came from.
///
/// A missing file is an empty blueprint, not an error: `apex blueprint diff` on
/// a machine nobody has configured should report "nothing is managed", not fail.
/// A file that exists and is *wrong* is always an error — that is the whole
/// point of the schema.
pub fn load(explicit: Option<&Path>) -> Result<(Blueprint, Source), String> {
    let candidates: Vec<Source> = match explicit {
        Some(p) => vec![Source::Explicit(p.to_path_buf())],
        None => vec![
            Source::User(user_blueprint_path()),
            Source::Site(site_blueprint_path()),
        ],
    };

    for source in candidates {
        let path = source.path().expect("candidate sources always carry a path");
        match std::fs::read_to_string(path) {
            Ok(text) => {
                let bp = Blueprint::parse(&text)
                    .map_err(|e| format!("{}: {e}", path.display()))?;
                return Ok((bp, source));
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // An explicit --file that does not exist IS an error: the user
                // named it, so silently falling back to an empty blueprint
                // would run `apply` against the wrong thing.
                if matches!(source, Source::Explicit(_)) {
                    return Err(format!("{}: no such file", path.display()));
                }
            }
            Err(e) => return Err(format!("{}: {e}", path.display())),
        }
    }
    Ok((Blueprint::default(), Source::None))
}

/// Read the generated record of the last `apply`, if there is one.
///
/// Never fatal and never fed to the planner. A corrupt record loses history and
/// nothing else.
pub fn load_applied() -> Option<AppliedState> {
    let text = std::fs::read_to_string(applied_state_path()).ok()?;
    AppliedState::parse(&text).ok()
}

// ── measuring the machine ────────────────────────────────────────────────────

/// Reads (never writes) the state a blueprint talks about.
///
/// `root` exists for the same reason [`apexd_core::syswriter::RealWriter`]'s
/// `sys_root` does: it lets a test point the probes at a fixture tree. And
/// `probe_programs` exists for the same reason that writer needs a separate
/// `host_commands` switch — no fixture root can redirect a process spawn, so a
/// test with a fixture root would still ask the *host's* `flatpak` what it has
/// installed, and the answer would depend on the developer's machine.
pub struct Host {
    root: PathBuf,
    probe_programs: bool,
}

impl Default for Host {
    fn default() -> Host {
        Host::new()
    }
}

impl Host {
    /// The real machine: `/` and permitted to run the read-only probes
    /// (`flatpak list`, and `PATH` lookups for toolchains).
    pub fn new() -> Host {
        Host {
            root: PathBuf::from("/"),
            probe_programs: true,
        }
    }

    /// A fixture tree, with process probes off.
    ///
    /// `#[cfg(test)]` rather than merely unused-in-production: `apex` is a
    /// binary crate, so an unreferenced constructor is dead code and CI runs
    /// clippy with `-D warnings`. Gating it also states the intent — there is
    /// no production caller and there should not be one.
    #[cfg(test)]
    pub fn with_root(root: impl Into<PathBuf>) -> Host {
        Host {
            root: root.into(),
            probe_programs: false,
        }
    }

    fn at(&self, rel: &str) -> PathBuf {
        self.root.join(rel)
    }

    /// The user's home, as the blueprint's user-owned targets see it.
    ///
    /// Under a fixture root this is `<root>/home`, so a test can lay out a
    /// `wallpaper.json` without touching the developer's own.
    fn home(&self) -> PathBuf {
        if self.root == Path::new("/") {
            apex_agent_core::paths::home()
        } else {
            self.root.join("home")
        }
    }

    /// Everything the planner needs, measured now.
    pub fn observe(&self) -> Observed {
        Observed {
            session: read_trimmed(&self.at(GREETER_SESSION)),
            sessions_available: self.sessions_available(),
            theme: self.theme(),
            packages: self.requested_packages(),
            flatpaks: self.installed_flatpaks(),
            languages: self.languages(),
            agent_default: self.agent_config().map(|(a, _)| a),
            agent_sandbox: self.agent_config().map(|(_, s)| s),
            variant_id: self.variant_id(),
        }
    }

    /// Session ids in `/usr/share/wayland-sessions`, sorted. The same set
    /// `apex-session-select --list` prints and validates against.
    fn sessions_available(&self) -> Vec<String> {
        let mut out: Vec<String> = match std::fs::read_dir(self.at(SESSION_DIR)) {
            Ok(entries) => entries
                .flatten()
                .filter_map(|e| {
                    let name = e.file_name().to_string_lossy().into_owned();
                    name.strip_suffix(".desktop").map(str::to_string)
                })
                .collect(),
            Err(_) => Vec::new(),
        };
        out.sort();
        out
    }

    /// APEX Shell's matugen scheme.
    ///
    /// An absent file is `content`, not unknown: `WallpaperService.qml`
    /// initialises `scheme` to `"content"` and only overrides it when the JSON
    /// carries a non-empty value, so "no file" has a definite effective value.
    /// A file that exists and is unreadable or malformed IS unknown, because
    /// then the shell's behaviour depends on how far its own parse got.
    fn theme(&self) -> Option<String> {
        let path = self.home().join(WALLPAPER_JSON);
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Some(SHELL_DEFAULT_SCHEME.to_string())
            }
            Err(_) => return None,
        };
        let value: serde_json::Value = serde_json::from_str(&text).ok()?;
        match value.get("scheme").and_then(|v| v.as_str()) {
            Some(s) if !s.is_empty() => Some(s.to_string()),
            // Present, parses, no scheme key: the shell's default applies.
            _ => Some(SHELL_DEFAULT_SCHEME.to_string()),
        }
    }

    /// The package engine's requested list, with the `local:` form stripped.
    fn requested_packages(&self) -> Vec<String> {
        let Ok(text) = std::fs::read_to_string(self.at(REQUESTED_LIST)) else {
            return Vec::new();
        };
        text.lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .map(|l| l.strip_prefix("local:").unwrap_or(l).to_string())
            .collect()
    }

    /// Flatpak application ids installed system-wide or for this user.
    ///
    /// `flatpak list` is read-only and cannot prompt. If flatpak is absent this
    /// is empty, and the converger — not the planner — reports the failure when
    /// a step needs it, so a machine with no flatpak gets one clear message
    /// instead of a silently different diff.
    fn installed_flatpaks(&self) -> Vec<String> {
        if !self.probe_programs {
            return Vec::new();
        }
        let Ok(out) = std::process::Command::new("flatpak")
            .args(["list", "--app", "--columns=application"])
            .output()
        else {
            return Vec::new();
        };
        if !out.status.success() {
            return Vec::new();
        }
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(str::to_string)
            .collect()
    }

    /// Which of the blueprint's languages have a usable toolchain on `PATH`.
    ///
    /// A deliberately shallow measurement: "is a compiler or interpreter
    /// reachable". It is the honest thing to report while `[development]` has
    /// no converger — phase 6's capsules replace it with a real answer about
    /// which capsule provides which language.
    fn languages(&self) -> Vec<String> {
        const PROBES: [(&str, &[&str]); 8] = [
            ("c", &["gcc", "clang", "cc"]),
            ("cpp", &["g++", "clang++"]),
            ("go", &["go"]),
            ("javascript", &["node"]),
            ("python", &["python3"]),
            ("rust", &["cargo", "rustc"]),
            ("shell", &["bash"]),
            ("typescript", &["tsc", "ts-node"]),
        ];
        if !self.probe_programs {
            return Vec::new();
        }
        PROBES
            .iter()
            .filter(|(_, bins)| bins.iter().any(|b| on_path(b)))
            .map(|(lang, _)| (*lang).to_string())
            .collect()
    }

    /// `default_agent` and `sandbox` from the agent runtime's own config.
    ///
    /// Loaded through `apex_agent_core::config`, never by hand-parsing the
    /// JSON: that type carries `#[serde(flatten)] extra` so a key written by a
    /// newer APEX Shell survives a round-trip, and re-implementing the read
    /// here would be the first half of losing it.
    fn agent_config(&self) -> Option<(String, String)> {
        if self.root != Path::new("/") {
            // The agent config resolves through $XDG_CONFIG_HOME, which a
            // fixture root cannot redirect. Reporting it as unmeasured is
            // honest; the real path is exercised by the shell suite, which
            // isolates HOME instead of using a fixture root.
            return None;
        }
        let cfg = apex_agent_core::config::Config::load();
        Some((cfg.default_agent.clone(), cfg.sandbox.as_str().to_string()))
    }

    /// `VARIANT_ID` from `/etc/os-release`. Reported in the `[gaming]` message
    /// so a user is told what the machine actually is.
    fn variant_id(&self) -> Option<String> {
        let text = std::fs::read_to_string(self.at("etc/os-release")).ok()?;
        for line in text.lines() {
            if let Some(v) = line.strip_prefix("VARIANT_ID=") {
                return Some(v.trim().trim_matches('"').to_string());
            }
        }
        None
    }
}

fn read_trimmed(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let trimmed = text.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// Whether `program` is an executable file somewhere on `PATH`.
fn on_path(program: &str) -> bool {
    let Some(paths) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&paths).any(|dir| {
        let candidate = dir.join(program);
        candidate.is_file() && is_executable(&candidate)
    })
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|m| m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

// ── rendering ────────────────────────────────────────────────────────────────

/// Exit code for `apex blueprint diff` when the machine has convergeable drift.
///
/// Chosen so the command reads like `diff(1)` and is usable in a script:
/// 0 converged, 1 drifted, 2 something went wrong. Blocked changes do not set
/// it — they are not drift anyone can close, and a permanently non-zero exit
/// would make the signal useless.
pub const EXIT_DRIFT: i32 = 1;
pub const EXIT_ERROR: i32 = 2;

/// `apex blueprint show`.
pub fn cmd_show(file: Option<&Path>, json: bool) -> i32 {
    let (bp, source) = match load(file) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("apex blueprint: {e}");
            return EXIT_ERROR;
        }
    };
    let applied = load_applied();

    if json {
        let mut obj = serde_json::Map::new();
        obj.insert("schema".into(), SCHEMA_VERSION.into());
        obj.insert(
            "source".into(),
            source
                .path()
                .map(|p| p.display().to_string())
                .map_or(serde_json::Value::Null, serde_json::Value::from),
        );
        obj.insert("digest".into(), bp.digest().into());
        obj.insert(
            "blueprint".into(),
            serde_json::to_value(&bp).unwrap_or(serde_json::Value::Null),
        );
        obj.insert(
            "applied".into(),
            applied
                .as_ref()
                .and_then(|a| serde_json::to_value(a).ok())
                .unwrap_or(serde_json::Value::Null),
        );
        obj.insert(
            "paths".into(),
            serde_json::json!({
                "user": user_blueprint_path().display().to_string(),
                "site": site_blueprint_path().display().to_string(),
                "applied_state": applied_state_path().display().to_string(),
            }),
        );
        println!("{}", serde_json::Value::Object(obj));
        return 0;
    }

    println!("blueprint  {}", source.describe());
    println!("digest     {}", bp.digest());
    match &applied {
        Some(a) if a.blueprint_digest == bp.digest() => println!(
            "applied    {} ago, as {} ({} step{})",
            ago(a.applied_at),
            a.domain,
            a.steps.len(),
            if a.steps.len() == 1 { "" } else { "s" }
        ),
        Some(a) => println!(
            "applied    {} ago, as {} — but against a DIFFERENT blueprint ({})",
            ago(a.applied_at),
            a.domain,
            a.blueprint_digest
        ),
        None => println!("applied    never on this machine"),
    }
    println!(
        "state      {}  (generated; never edit it)",
        applied_state_path().display()
    );
    println!();

    if bp == Blueprint::default() {
        println!("This blueprint manages nothing.");
        println!("`apex blueprint init` writes a commented starting point.");
        return 0;
    }

    let text = match bp.to_toml() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("apex blueprint: cannot render: {e}");
            return EXIT_ERROR;
        }
    };
    print!("{text}");
    0
}

/// `apex blueprint diff`.
pub fn cmd_diff(file: Option<&Path>, json: bool) -> i32 {
    let (bp, source) = match load(file) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("apex blueprint: {e}");
            return EXIT_ERROR;
        }
    };
    let observed = Host::new().observe();
    let plan = blueprint::plan(&bp, &observed);

    if json {
        println!("{}", plan_json(&bp, &source, &plan));
    } else {
        print_plan(&plan, &source, "would change");
    }

    if plan.is_converged() {
        0
    } else {
        EXIT_DRIFT
    }
}

/// `apex blueprint init`.
pub fn cmd_init(force: bool) -> i32 {
    let path = user_blueprint_path();
    if path.exists() && !force {
        eprintln!(
            "apex blueprint: {} already exists; --force overwrites it",
            path.display()
        );
        return EXIT_ERROR;
    }
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            eprintln!("apex blueprint: cannot create {}: {e}", parent.display());
            return EXIT_ERROR;
        }
    }
    if let Err(e) = write_atomic(&path, STARTER) {
        eprintln!("apex blueprint: {e}");
        return EXIT_ERROR;
    }
    println!("wrote {}", path.display());
    println!("Every section is commented out, so it manages nothing until you edit it.");
    0
}

/// Render a plan as a table. `verb` is how the steps are introduced, so the
/// same renderer serves `diff` ("would change") and `apply --dry-run`.
pub fn print_plan(plan: &Plan, source: &Source, verb: &str) {
    if let Some(p) = source.path() {
        println!("blueprint  {}", p.display());
    } else {
        println!("blueprint  {}", source.describe());
    }

    let actionable: Vec<&Change> = plan.changes.iter().filter(|c| c.step.is_some()).collect();
    let blocked = plan.blocked();

    if actionable.is_empty() && blocked.is_empty() {
        println!();
        println!("The machine matches the blueprint. Nothing to do.");
        return;
    }

    if !actionable.is_empty() {
        // Grouped by privilege domain, because that is the thing the user has
        // to act on: `apex apply` closes the user half, `sudo apex apply` the
        // root half, and nothing escalates on its own.
        let mut by_domain: BTreeMap<&str, Vec<&Change>> = BTreeMap::new();
        for c in &actionable {
            by_domain
                .entry(c.domain().map_or("?", Domain::as_str))
                .or_default()
                .push(c);
        }
        let width = actionable
            .iter()
            .map(|c| c.what.len())
            .max()
            .unwrap_or(0)
            .max(8);
        for (domain, changes) in by_domain {
            println!();
            println!(
                "{} ({} {verb}; run {})",
                if domain == "root" { "SYSTEM" } else { "THIS USER" },
                changes.len(),
                if domain == "root" {
                    "`sudo apex apply`"
                } else {
                    "`apex apply`"
                }
            );
            for c in changes {
                println!(
                    "  {:<width$}  {} -> {}",
                    c.what,
                    c.current,
                    c.desired,
                    width = width
                );
                if let Some(step) = &c.step {
                    println!("  {:<width$}    {step}", "", width = width);
                }
            }
        }
    }

    if !blocked.is_empty() {
        println!();
        println!("CANNOT CONVERGE ({})", blocked.len());
        for c in blocked {
            println!("  {}  {} -> {}", c.what, c.current, c.desired);
            println!(
                "      {}",
                c.blocked.as_deref().unwrap_or("no reason recorded")
            );
        }
    }
}

/// The machine-readable form of a plan, shared by `diff --json` and
/// `apply --dry-run --json` so the two cannot describe the same plan
/// differently.
pub fn plan_json(bp: &Blueprint, source: &Source, plan: &Plan) -> serde_json::Value {
    serde_json::json!({
        "schema": SCHEMA_VERSION,
        "source": source.path().map(|p| p.display().to_string()),
        "digest": bp.digest(),
        "converged": plan.is_converged(),
        "changes": plan.changes.iter().map(|c| serde_json::json!({
            "what": c.what,
            "current": c.current,
            "desired": c.desired,
            "step": c.step.as_ref().map(ToString::to_string),
            "domain": c.domain().map(Domain::as_str),
            "blocked": c.blocked,
        })).collect::<Vec<_>>(),
    })
}

/// Write `text` to `path` through a temporary file and a rename.
///
/// Atomic replacement, per the repo contract for persistent configuration: a
/// blueprint truncated by a crash mid-write would be a file the user has to
/// reconstruct by hand.
pub fn write_atomic(path: &Path, text: &str) -> Result<(), String> {
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, text).map_err(|e| format!("writing {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| {
        // Leave no debris if the rename fails.
        let _ = std::fs::remove_file(&tmp);
        format!("installing {}: {e}", path.display())
    })
}

/// Seconds since the epoch, or 0 if the clock is before 1970.
pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// "3 minutes", "2 days" — a coarse age, because the exact second of the last
/// apply has never been the interesting part and formatting a real timestamp
/// would need a date library.
fn ago(then: u64) -> String {
    let now = now_secs();
    let secs = now.saturating_sub(then);
    match secs {
        0..=59 => format!("{secs}s"),
        60..=3599 => format!("{}m", secs / 60),
        3600..=86_399 => format!("{}h", secs / 3600),
        _ => format!("{}d", secs / 86_400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fixture machine laid out under one temporary directory.
    struct Fixture {
        dir: PathBuf,
    }

    impl Fixture {
        fn new(name: &str) -> Fixture {
            let dir = std::env::temp_dir().join(format!("apex-bp-test-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("fixture root");
            Fixture { dir }
        }

        fn write(&self, rel: &str, text: &str) -> &Fixture {
            let path = self.dir.join(rel);
            std::fs::create_dir_all(path.parent().expect("relative paths have parents"))
                .expect("fixture dirs");
            std::fs::write(&path, text).expect("fixture file");
            self
        }

        fn host(&self) -> Host {
            Host::with_root(&self.dir)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    #[test]
    fn sessions_are_read_from_the_directory_the_greeter_enumerates() {
        let f = Fixture::new("sessions");
        f.write("usr/share/wayland-sessions/hyprland.desktop", "")
            .write("usr/share/wayland-sessions/niri.desktop", "")
            .write("usr/share/wayland-sessions/apex-labwc.desktop", "")
            // Not a .desktop: must be ignored, not offered as a session.
            .write("usr/share/wayland-sessions/README", "");
        let obs = f.host().observe();
        assert_eq!(obs.sessions_available, ["apex-labwc", "hyprland", "niri"]);
        assert!(!obs.has_gaming_session());
    }

    #[test]
    fn the_gaming_session_is_what_marks_a_gaming_edition() {
        let f = Fixture::new("gaming");
        f.write("usr/share/wayland-sessions/apex-gaming.desktop", "")
            .write("etc/os-release", "ID=apexos\nVARIANT_ID=\"gaming-nvidia\"\n");
        let obs = f.host().observe();
        assert!(obs.has_gaming_session());
        assert_eq!(obs.variant_id.as_deref(), Some("gaming-nvidia"));
    }

    #[test]
    fn the_preselected_session_is_read_from_the_greeters_own_state() {
        let f = Fixture::new("greeter");
        // Written by apex-session-select with no trailing newline; a stray one
        // must not become part of the id either.
        f.write("var/lib/apex-greet/last-session", "apex-labwc\n");
        assert_eq!(f.host().observe().session.as_deref(), Some("apex-labwc"));
    }

    #[test]
    fn requested_packages_come_from_the_requested_list_not_the_resolved_one() {
        // state.json records every dependency; diffing against that would
        // report convergence based on packages nobody asked for.
        let f = Fixture::new("requested");
        f.write(
            "var/lib/apex/pkg/requested",
            "firefox\nlocal:some-vendor-driver\n\nhtop\n",
        );
        assert_eq!(
            f.host().observe().packages,
            ["firefox", "some-vendor-driver", "htop"]
        );
    }

    #[test]
    fn an_absent_wallpaper_json_measures_the_shells_own_default() {
        // Not "unknown": WallpaperService initialises scheme to "content", so
        // no file has a definite effective value. Reporting unknown would make
        // a fresh machine show permanent drift on `[desktop] theme`.
        let f = Fixture::new("theme-absent");
        assert_eq!(f.host().observe().theme.as_deref(), Some("content"));
    }

    #[test]
    fn a_written_scheme_is_read_back_and_a_broken_file_is_unknown() {
        let f = Fixture::new("theme-set");
        f.write(
            "home/.config/apex-shell/src/user_data/wallpaper.json",
            r#"{"scheme": "monochrome", "wallpaperDir": "~/Pictures"}"#,
        );
        assert_eq!(f.host().observe().theme.as_deref(), Some("monochrome"));

        f.write(
            "home/.config/apex-shell/src/user_data/wallpaper.json",
            "{not json",
        );
        assert_eq!(
            f.host().observe().theme,
            None,
            "a malformed file is unknown, because the shell's own parse may have \
             stopped anywhere"
        );

        // Present, valid, and no scheme key: the shell's default applies.
        f.write(
            "home/.config/apex-shell/src/user_data/wallpaper.json",
            r#"{"wallpaperDir": "~/Pictures"}"#,
        );
        assert_eq!(f.host().observe().theme.as_deref(), Some("content"));
    }

    #[test]
    fn a_fixture_host_never_runs_a_program() {
        // The same guarantee RealWriter's `host_commands` gives, and for the
        // same reason: `root` redirects a file read, and nothing redirects a
        // process spawn. A fixture host that shelled out to `flatpak` would
        // make these tests depend on what the developer has installed.
        let f = Fixture::new("no-programs");
        assert!(!f.host().probe_programs);
        let obs = f.host().observe();
        assert!(obs.flatpaks.is_empty());
        assert!(obs.languages.is_empty());
        assert!(obs.agent_default.is_none());
    }

    #[test]
    fn the_real_host_does_probe() {
        // Otherwise the guard above has quietly disabled observation in
        // production, which is the failure mode of fixing it the lazy way.
        assert!(Host::new().probe_programs);
        assert_eq!(Host::new().root, Path::new("/"));
    }

    #[test]
    fn the_starter_blueprint_parses_and_manages_nothing() {
        // Everything in it is commented out on purpose: `apex blueprint init`
        // must never hand someone a file that changes their machine.
        let bp = Blueprint::parse(STARTER).expect("the starter must be a valid blueprint");
        assert_eq!(bp, Blueprint::default());
    }

    #[test]
    fn an_explicit_file_that_does_not_exist_is_an_error_not_an_empty_blueprint() {
        // Falling back would run `apply` against something the user did not
        // name.
        let err = load(Some(Path::new("/nonexistent/apex-blueprint.toml"))).unwrap_err();
        assert!(err.contains("no such file"), "{err}");
    }

    #[test]
    fn a_bad_blueprint_names_its_file_in_the_error() {
        let f = Fixture::new("bad-file");
        f.write("bp.toml", "[desktop]\ncompositor = \"hyperland\"\n");
        let path = f.dir.join("bp.toml");
        let err = load(Some(&path)).unwrap_err();
        assert!(err.contains("bp.toml"), "{err}");
        assert!(err.contains("hyperland"), "{err}");
    }

    #[test]
    fn the_agent_vocabularies_match_the_agent_runtimes_own() {
        // `apexd-core` cannot depend on `apex-agent-core`, so the blueprint's
        // agent and sandbox lists are a transcription. This crate depends on
        // both, so it is the one place the two can be compared — the same
        // reason `files/scripts/check-input-parity` exists for the settings
        // page and its generator. Without it, adding an adapter to the runtime
        // would leave the blueprint refusing a name the runtime accepts, and
        // the only symptom would be a confusing validation error.
        use apex_agent_core::adapter;
        use apex_agent_core::protocol::SandboxPolicy;

        assert_eq!(
            apexd_core::blueprint::AGENTS.to_vec(),
            adapter::ids(),
            "blueprint::AGENTS has drifted from apex_agent_core::adapter::ADAPTERS"
        );
        assert!(
            apexd_core::blueprint::AGENTS.contains(&adapter::DEFAULT_AGENT),
            "the runtime's default agent must be a legal blueprint value"
        );

        for policy in apexd_core::blueprint::SANDBOX_POLICIES {
            assert!(
                SandboxPolicy::parse(policy).is_some(),
                "the runtime does not accept sandbox policy {policy:?}"
            );
        }
        // And the other direction: every policy the runtime has must be
        // expressible, or a blueprint could not describe a machine the user
        // has already configured.
        for policy in [
            SandboxPolicy::Unrestricted,
            SandboxPolicy::Project,
            SandboxPolicy::Strict,
        ] {
            assert!(
                apexd_core::blueprint::SANDBOX_POLICIES.contains(&policy.as_str()),
                "no blueprint spelling for {}",
                policy.as_str()
            );
        }
    }

    #[test]
    fn the_theme_vocabulary_matches_what_the_shell_offers() {
        // WallpaperService.qml's `schemes` list, transcribed. There is no way
        // to import QML, so this asserts the constant against the same list
        // spelled out here — the value is that the two are side by side in one
        // place with a comment saying where the other one lives:
        //     apex-shell/src/services/WallpaperService.qml
        //         readonly property var schemes: [
        //             "content", "tonal-spot", "fidelity", "fruit-salad",
        //             "neutral", "monochrome"
        //         ]
        assert_eq!(
            apexd_core::blueprint::THEMES.to_vec(),
            [
                "content",
                "tonal-spot",
                "fidelity",
                "fruit-salad",
                "neutral",
                "monochrome"
            ]
        );
        assert!(apexd_core::blueprint::THEMES.contains(&SHELL_DEFAULT_SCHEME));
    }

    #[test]
    fn the_three_paths_are_distinct_and_in_the_right_trees() {
        let user = user_blueprint_path();
        let state = applied_state_path();
        assert_ne!(user, state);
        assert!(user.to_string_lossy().contains("apex/blueprint.toml"));
        // Desired lives in config, generated lives in state. Putting the
        // generated record next to the hand-edited file is how someone ends up
        // editing the wrong one.
        assert!(state.to_string_lossy().contains("apex/blueprint-state.toml"));
        assert_eq!(site_blueprint_path(), Path::new("/etc/apex/blueprint.toml"));
    }
}
