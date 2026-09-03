//! `apex gaming` and `apex game profile` — the other half of roadmap §12.
//!
//! `apexd_core::gameprofile` holds the schema and the pure planner;
//! `apexd_core::gaming` holds the readiness probe. This file is the part that
//! touches the world: path resolution, reading and writing
//! `~/.config/apex/games.toml`, and turning a [`Resolution`] into calls on the
//! frozen `org.apexos.Apexd1` surface. The split is phase 7's, for phase 7's
//! reason — a planner that cannot perform I/O cannot be the thing that reaches
//! a developer's machine.
//!
//! ## Two verbs, and why they are not one
//!
//! `apex game` is the **hardware lever**: cpuset pinning, IRQ steering, GPU
//! clock locks. It already ships and is untouched here.
//!
//! `apex gaming` is the **§12 experience**: whether this machine can boot
//! straight into a controller-first Gaming Mode at all. It reads and reports;
//! it has no mutating form.
//!
//! `apex game profile` sits under the lever because that is what a profile
//! composes.
//!
//! ## The guard is the one that already exists
//!
//! `apex game profile apply` moves the tier, auto-switch, game mode and fan —
//! the same levers `apex mode set` moves — so it obeys the same guard,
//! [`crate::mode::NO_APPLY_ENV`], checked in the same place: **first**, ahead of
//! the bus connection. A second variable would split one invariant across two
//! names, oblige a third static check in `pr-validation.yml`, and give a future
//! reader two things to remember instead of one.
//!
//! ## What is NOT here, and why
//!
//! **There is no launch wrapper binary.** SteamOS applies a per-game profile by
//! interposing on the game's own launch, and the honest equivalent here would be
//! `apex game launch -- %command%`: resolve the AppID from Steam's environment,
//! apply the profile, spawn the game, restore on exit. It is not built, and the
//! reason is that it cannot be verified from here — it needs a real Steam
//! install, a real title and real launch options, none of which this machine
//! has. An unverified exec path in the position where *every* game starts is
//! worse than no exec path at all: its failure mode is "the game does not
//! launch", and the user cannot tell whether APEX or Proton broke it.
//!
//! What ships instead is `apex game profile launch-command`, which prints a
//! launch-option line built only from verbs that exist today:
//!
//! ```text
//! apex game profile apply 1091500 && %command%
//! ```
//!
//! That applies the profile and then runs the game, using Steam's own
//! `%command%` placeholder. It does not restore anything when the game exits —
//! `apex mode set daily` or `apex game stop` does that — and `show` says so
//! rather than leaving it to be discovered.

use std::path::PathBuf;

use apexd_core::fan::FanMode;
use apexd_core::gameprofile::{
    self, GameProfile, GameProfiles, Resolution, Step, DEFAULT_MODE, SCHEMA_VERSION,
};
use apexd_core::gaming::{self, Readiness};
use apexd_core::mode as coremode;
use clap::{Args, Subcommand};

use crate::json_string as js;
use crate::proxy::{connect, FanProxy, GameModeProxy, PowerProxy};

/// The header `set` and `remove` write above the profiles.
///
/// TOML comments are not part of the parsed model, so a rewrite cannot preserve
/// them — and this file *is* rewritten, by `set` and `remove`. Rather than let
/// somebody discover that by losing their annotations, the file says it about
/// itself. `note = "..."` is the field that does survive, and it is named here
/// so the alternative is visible at the moment it is needed.
const HEADER: &str = "\
# APEX-OS per-game profiles (roadmap §12) — user-owned, hand-editable.
#
# `apex game profile set` and `remove` REWRITE this file, so comments you add
# below do not survive them. Use each profile's `note = \"...\"` field for
# anything you want to keep.
#
#   apex game profile list                  what is stored
#   apex game profile show <id>             one profile and the plan it implies
#   apex game profile apply <id>            put the machine into it
#   apex game profile launch-command <id>   the Steam launch-option line
#
# An id is a Steam AppID (the number in the store URL) or any slug of letters,
# digits, '-' and '_'.
";

// ── the CLI surface ──────────────────────────────────────────────────────────

#[derive(Args)]
pub struct GamingArgs {
    /// Emit machine-readable JSON instead of a report.
    #[arg(long)]
    pub json: bool,
}

#[derive(Subcommand)]
pub enum ProfileCmd {
    /// Every stored profile.
    List {
        #[arg(long)]
        json: bool,
    },
    /// One profile, and the ordered plan applying it would run.
    Show {
        #[arg(value_name = "ID")]
        id: String,
        #[arg(long)]
        json: bool,
    },
    /// Create or update a profile. Only the options given are changed; an empty
    /// value clears a field.
    Set {
        #[arg(value_name = "ID")]
        id: String,
        /// Human label, for listings.
        #[arg(long)]
        title: Option<String>,
        /// A mode: daily, gaming, development, creator, ai, battery, couch,
        /// server.
        #[arg(long)]
        mode: Option<String>,
        /// Override the mode's tier: performance, balanced, power-saver.
        #[arg(long)]
        tier: Option<String>,
        /// Fan mode: auto, max, curve, manual, manual:<0-255>.
        #[arg(long)]
        fan: Option<String>,
        /// A reminder of why this profile is what it is. Survives a rewrite.
        #[arg(long)]
        note: Option<String>,
    },
    /// Delete a profile.
    Remove {
        #[arg(value_name = "ID")]
        id: String,
    },
    /// Put the machine into a profile.
    Apply {
        #[arg(value_name = "ID")]
        id: String,
        /// Print the steps and change nothing.
        #[arg(long)]
        dry_run: bool,
    },
    /// Print the Steam launch-option line for a profile.
    LaunchCommand {
        #[arg(value_name = "ID")]
        id: String,
    },
    /// Print where profiles are stored.
    Path,
}

// ── where profiles live ──────────────────────────────────────────────────────

/// `~/.config/apex/games.toml`, or `$XDG_CONFIG_HOME`'s equivalent.
///
/// Beside the blueprint and not inside it — see
/// [`apexd_core::gameprofile`]'s module docs for the argument. Resolved through
/// `apex_agent_core::paths`, the same already-tested implementation of the
/// base-directory spec `apex/src/blueprint.rs` uses, rather than a second one.
pub fn games_path() -> PathBuf {
    apex_agent_core::paths::config_home().join("apex/games.toml")
}

/// Read the games file. A missing file is an empty set, never an error: `list`
/// on a machine nobody has configured should say "nothing is stored", not fail.
/// A file that exists and is *wrong* is always an error.
fn load() -> Result<GameProfiles, String> {
    let path = games_path();
    match std::fs::read_to_string(&path) {
        Ok(text) => {
            GameProfiles::parse(&text).map_err(|e| format!("{}: {e}", path.display()))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(GameProfiles::default()),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

/// Write the games file, creating its directory.
///
/// Round-tripped through `parse` before it lands. Without that, a bad write is
/// only discovered by the *next* command — or on another machine — with no way
/// to tell which end was wrong. `apex sync export` validates its own output for
/// the same reason.
fn save(profiles: &GameProfiles) -> Result<(), String> {
    let path = games_path();
    let body = profiles
        .to_toml()
        .map_err(|e| format!("cannot render the games file: {e}"))?;
    let text = format!("{HEADER}\n{body}");
    let reparsed = GameProfiles::parse(&text)
        .map_err(|e| format!("refusing to write a games file that cannot be read back: {e}"))?;
    if &reparsed != profiles {
        return Err(
            "refusing to write a games file that does not read back as what was asked for"
                .to_string(),
        );
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("creating {}: {e}", dir.display()))?;
    }
    crate::blueprint::write_atomic(&path, &text)
}

// ── entry points ─────────────────────────────────────────────────────────────

pub async fn profile_main(cmd: ProfileCmd) -> i32 {
    match cmd {
        ProfileCmd::List { json } => cmd_list(json),
        ProfileCmd::Show { id, json } => cmd_show(&id, json).await,
        ProfileCmd::Set {
            id,
            title,
            mode,
            tier,
            fan,
            note,
        } => cmd_set(&id, title, mode, tier, fan, note),
        ProfileCmd::Remove { id } => cmd_remove(&id),
        ProfileCmd::Apply { id, dry_run } => cmd_apply(&id, dry_run).await,
        ProfileCmd::LaunchCommand { id } => cmd_launch_command(&id),
        ProfileCmd::Path => {
            println!("{}", games_path().display());
            0
        }
    }
}

// ── list ─────────────────────────────────────────────────────────────────────

fn cmd_list(json: bool) -> i32 {
    let profiles = match load() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("apex: {e}");
            return 1;
        }
    };
    if json {
        let rows: Vec<String> = profiles
            .game
            .iter()
            .map(|(id, p)| {
                format!(
                    "{{\"id\":{},\"title\":{},\"mode\":{},\"tier\":{},\"fan\":{}}}",
                    js(id),
                    p.title.as_deref().map(js).unwrap_or("null".into()),
                    js(p.mode_id().as_str()),
                    p.tier.as_deref().map(js).unwrap_or("null".into()),
                    p.fan.as_deref().map(js).unwrap_or("null".into()),
                )
            })
            .collect();
        println!(
            "{{\"path\":{},\"profiles\":[{}]}}",
            js(&games_path().display().to_string()),
            rows.join(",")
        );
        return 0;
    }

    if profiles.is_empty() {
        println!("No per-game profiles are stored.");
        println!();
        println!("  {}", games_path().display());
        println!();
        println!("Create one with, for example:");
        println!("  apex game profile set 1091500 --title 'Cyberpunk 2077' --fan max");
        return 0;
    }

    let width = profiles
        .ids()
        .iter()
        .map(|i| i.len())
        .max()
        .unwrap_or(2)
        .max(2);
    println!("{:<width$}  MODE         TIER         FAN     TITLE", "ID", width = width);
    for (id, p) in &profiles.game {
        println!(
            "{:<width$}  {:<12} {:<12} {:<7} {}",
            id,
            p.mode_id().as_str(),
            p.tier.as_deref().unwrap_or("-"),
            p.fan.as_deref().unwrap_or("-"),
            p.title.as_deref().unwrap_or(""),
            width = width
        );
    }
    println!();
    println!("{}", games_path().display());
    0
}

// ── show ─────────────────────────────────────────────────────────────────────

/// The state a plan is built against, or a reason there is none.
///
/// A profile can be *shown* without a daemon, which matters: `show` is how a
/// user checks what they stored, and refusing to print it because apexd is down
/// would make the file unreadable for the wrong reason. So the plan is the part
/// that needs the machine, and its absence is reported rather than fatal.
async fn observed_or_reason() -> Result<coremode::ModeState, String> {
    crate::mode::observe().await
}

async fn cmd_show(id: &str, json: bool) -> i32 {
    let profiles = match load() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("apex: {e}");
            return 1;
        }
    };
    let Some(p) = profiles.get(id) else {
        eprintln!("apex: no profile for '{id}'.");
        if !profiles.is_empty() {
            eprintln!("      stored: {}", profiles.ids().join(", "));
        }
        eprintln!("      create one with: apex game profile set {id} …");
        return 1;
    };

    // Two answers, and the distinction is load-bearing: `full` is what the
    // profile asks for and needs nothing but the file, `resolution` is what
    // would change right now and needs the daemon.
    let full = gameprofile::intent(id, p);
    let state = observed_or_reason().await;
    let resolution = state.as_ref().ok().map(|s| gameprofile::plan(id, p, s));

    if json {
        println!(
            "{}",
            show_json(id, p, &full, resolution.as_ref(), state.as_ref().err())
        );
        return 0;
    }

    kv("profile", &p.label(id));
    kv("mode", p.mode_id().as_str());
    kv(
        "tier",
        &match p.tier_override() {
            Some(t) => format!("{t} (overrides the mode)"),
            None => format!("from mode '{}'", p.mode_id()),
        },
    );
    kv("fan", p.fan.as_deref().unwrap_or("unmanaged"));
    if let Some(n) = &p.note {
        kv("note", n);
    }

    // The full intent first, because it needs no daemon and is what the user
    // actually stored. The delta against the running machine is printed after
    // it, when there is a machine to read.
    println!();
    println!("asks for (in order):");
    for s in &full.steps {
        println!("  - {}", s.describe());
    }

    if !full.notes.is_empty() {
        println!();
        println!("why the order is what it is:");
        for n in &full.notes {
            print_wrapped("  - ", n);
        }
    }
    if !full.reported.is_empty() {
        println!();
        println!("reported, NOT set by this profile:");
        for n in &full.reported {
            print_wrapped("  - ", n);
        }
    }

    println!();
    match &resolution {
        Some(r) if r.is_noop() => {
            println!("changes now: none — the machine is already in this profile.")
        }
        Some(r) => {
            println!("changes now:");
            for s in &r.steps {
                println!("  - {}", s.describe());
            }
        }
        // A readable state always yields a resolution, so the absent case is
        // always an unreadable one. Written as a catch-all rather than an
        // `unreachable!` because this crate's contract is a message and an exit
        // code, never a panic.
        None => {
            let why = state
                .as_ref()
                .err()
                .cloned()
                .unwrap_or_else(|| "no reason given".to_string());
            println!("changes now: unknown — {why}.");
            println!("             What would change is the difference between the profile and");
            println!("             what apexd reports, so it needs the daemon. Everything above");
            println!("             is derived from the file and is accurate without it.");
        }
    }

    println!();
    println!("Steam launch option:");
    match gameprofile::launch_command(id) {
        Ok(c) => println!("  {c}"),
        Err(e) => println!("  unavailable — {e}"),
    }
    println!();
    println!("Applying a profile does not undo itself when the game exits. APEX ships no");
    println!("launch wrapper, so leaving is a separate, explicit step:");
    println!("  apex mode set daily        (or `apex game stop` for game mode alone)");
    0
}

fn show_json(
    id: &str,
    p: &GameProfile,
    full: &Resolution,
    r: Option<&Resolution>,
    state_error: Option<&String>,
) -> String {
    let list = |v: &[String]| v.iter().map(|x| js(x)).collect::<Vec<_>>().join(",");
    let steps = |r: &Resolution| {
        r.steps
            .iter()
            .map(|s| js(&s.describe()))
            .collect::<Vec<_>>()
            .join(",")
    };
    format!(
        "{{\"id\":{},\"title\":{},\"mode\":{},\"tier\":{},\"fan\":{},\"note\":{},\
         \"asks_for\":[{}],\"notes\":[{}],\"reported\":[{}],\
         \"changes_now\":{{\"available\":{},\"unavailable\":{},\"steps\":[{}]}},\
         \"launch_command\":{}}}",
        js(id),
        p.title.as_deref().map(js).unwrap_or("null".into()),
        js(p.mode_id().as_str()),
        p.tier.as_deref().map(js).unwrap_or("null".into()),
        p.fan.as_deref().map(js).unwrap_or("null".into()),
        p.note.as_deref().map(js).unwrap_or("null".into()),
        steps(full),
        list(&full.notes),
        list(&full.reported),
        r.is_some(),
        state_error.map(|e| js(e)).unwrap_or("null".into()),
        r.map(steps).unwrap_or_default(),
        gameprofile::launch_command(id)
            .ok()
            .as_deref()
            .map(js)
            .unwrap_or("null".into()),
    )
}

// ── set / remove ─────────────────────────────────────────────────────────────

/// Merge one option onto a field. `None` leaves it alone; `Some("")` clears it.
///
/// The three-way distinction is what makes `set` incremental. A two-way one
/// would force every `set` to restate every field, and the first time somebody
/// ran `apex game profile set 620 --fan max` they would silently lose the tier
/// they set last week.
fn merge(field: &mut Option<String>, given: Option<String>) {
    match given {
        None => {}
        Some(v) if v.trim().is_empty() => *field = None,
        Some(v) => *field = Some(v.trim().to_string()),
    }
}

fn cmd_set(
    id: &str,
    title: Option<String>,
    mode: Option<String>,
    tier: Option<String>,
    fan: Option<String>,
    note: Option<String>,
) -> i32 {
    // Checked before the file is read, let alone written: the id becomes a TOML
    // table key and reaches argv, so it is validated at the boundary rather
    // than trusted because it came from a person's own shell.
    if let Err(why) = gameprofile::check_game_id(id) {
        eprintln!("apex: refusing the id '{id}': {why}");
        return 2;
    }

    let mut profiles = match load() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("apex: {e}");
            eprintln!("      fix the file, or move it aside; refusing to overwrite it.");
            return 1;
        }
    };

    let existed = profiles.game.contains_key(id);
    let entry = profiles.game.entry(id.to_string()).or_default();
    merge(&mut entry.title, title);
    merge(&mut entry.mode, mode);
    merge(&mut entry.tier, tier);
    merge(&mut entry.fan, fan);
    merge(&mut entry.note, note);

    // Validated as a whole before anything is written, so a bad value is a
    // refusal rather than a file that fails to load next time.
    profiles.version = Some(SCHEMA_VERSION);
    let problems = profiles.validate();
    if !problems.is_empty() {
        eprintln!("apex: refusing to store that profile:");
        for p in problems {
            eprintln!("  {p}");
        }
        return 2;
    }

    if let Err(e) = save(&profiles) {
        eprintln!("apex: {e}");
        return 1;
    }

    let p = profiles.get(id).expect("just inserted");
    println!(
        "apex: {} profile '{}' — mode {}{}{}",
        if existed { "updated" } else { "stored" },
        p.label(id),
        p.mode_id(),
        p.tier
            .as_deref()
            .map(|t| format!(", tier {t}"))
            .unwrap_or_default(),
        p.fan
            .as_deref()
            .map(|f| format!(", fan {f}"))
            .unwrap_or_default(),
    );
    if p.mode.is_none() {
        println!("apex: no mode named, so it composes '{DEFAULT_MODE}'.");
    }
    if let Some(advice) = gameprofile::id_advice(id) {
        println!("apex: note — {advice}");
    }
    println!("apex: {}", games_path().display());
    0
}

fn cmd_remove(id: &str) -> i32 {
    let mut profiles = match load() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("apex: {e}");
            return 1;
        }
    };
    if profiles.game.remove(id).is_none() {
        eprintln!("apex: no profile for '{id}' — nothing removed.");
        return 1;
    }
    if let Err(e) = save(&profiles) {
        eprintln!("apex: {e}");
        return 1;
    }
    println!("apex: removed profile '{id}'.");
    0
}

fn cmd_launch_command(id: &str) -> i32 {
    match gameprofile::launch_command(id) {
        Ok(c) => {
            println!("{c}");
            0
        }
        Err(e) => {
            eprintln!("apex: {e}");
            2
        }
    }
}

// ── apply ────────────────────────────────────────────────────────────────────

async fn cmd_apply(id: &str, dry_run: bool) -> i32 {
    // FIRST, ahead of the bus connection, the file read and every other check:
    // the guard that keeps a test suite off the machine running it. Identical
    // placement to `apex mode set`, and the suite proves the ordering rather
    // than assuming it.
    if !dry_run && crate::mode::guard_set() {
        eprintln!(
            "apex: refusing to apply — {} is set.\n\
             \x20     This guard exists so a test suite can run the real binary without\n\
             \x20     moving the tier of the machine it runs on. Use --dry-run to see the\n\
             \x20     plan, or unset {} to apply for real.",
            crate::mode::NO_APPLY_ENV,
            crate::mode::NO_APPLY_ENV
        );
        return 2;
    }

    let profiles = match load() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("apex: {e}");
            return 1;
        }
    };
    let Some(profile) = profiles.get(id) else {
        eprintln!("apex: no profile for '{id}'.");
        if !profiles.is_empty() {
            eprintln!("      stored: {}", profiles.ids().join(", "));
        }
        return 1;
    };

    let state = match crate::mode::observe().await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("apex: {e} — cannot read the current state, so no plan can be built.");
            return 1;
        }
    };

    // Computed ONCE. A dry run prints exactly this list; a live run executes
    // exactly this list. There is no second planning path for either to drift
    // from.
    let plan = gameprofile::plan(id, profile, &state);

    if plan.is_noop() {
        println!("apex: already in profile '{}' — nothing to do.", profile.label(id));
        return 0;
    }

    if dry_run {
        println!(
            "apex: plan for profile '{}' (dry run — nothing was changed):",
            profile.label(id)
        );
        for s in &plan.steps {
            println!("  - {}", s.describe());
        }
        for n in &plan.notes {
            print_wrapped("  # ", n);
        }
        return 0;
    }

    let Some(conn) = connect().await else {
        eprintln!("apex: cannot reach the system bus.");
        return 1;
    };
    for step in &plan.steps {
        if let Err(e) = perform(&conn, step).await {
            eprintln!("apex: {} failed: {e}", step.describe());
            eprintln!("apex: stopping here rather than applying a partial profile.");
            return 1;
        }
        println!("apex: {}", step.describe());
    }
    println!("apex: profile -> {}", profile.label(id));

    // Re-measure and report residual drift, per `apexd/AGENTS.md`: a command
    // that reports success must verify the requested state, and a step can exit
    // 0 having changed nothing. This is where the tier re-assert earns its
    // place — without it, this check would fire on any machine whose sysprofile
    // pins a different game tier.
    match residual(&conn, id, profile).await {
        Ok(drift) if drift.is_empty() => {}
        Ok(drift) => {
            println!();
            println!("apex: applied, but the machine did not settle where the profile asked:");
            for d in drift {
                println!("  - {d}");
            }
            println!("apex: that is reported rather than retried — a second pass would hide");
            println!("      whichever lever is refusing, and the profile is not the place to");
            println!("      diagnose it. `apex perf` and `apex fan` report the levers directly.");
            return 1;
        }
        Err(e) => {
            println!("apex: applied, but could not re-measure to confirm it: {e}");
        }
    }
    0
}

/// Perform one step over the frozen D-Bus surface.
///
/// Every arm is a method a user could type by hand, behind the `manage-power`
/// polkit action which ships `allow_active = yes` — so an active local session
/// authorises without a prompt.
async fn perform(conn: &zbus::Connection, step: &Step) -> Result<(), String> {
    match step {
        Step::Policy(coremode::Step::AutoSwitch(v)) => {
            let p = PowerProxy::new(conn).await.map_err(|e| e.to_string())?;
            p.set_auto_switch(*v).await.map_err(|e| e.to_string())
        }
        Step::Policy(coremode::Step::SetTier(t)) => {
            let p = PowerProxy::new(conn).await.map_err(|e| e.to_string())?;
            p.set_tier(t.as_str()).await.map_err(|e| e.to_string())
        }
        Step::Policy(coremode::Step::GameMode(v)) => {
            let g = GameModeProxy::new(conn).await.map_err(|e| e.to_string())?;
            g.set_active(*v).await.map_err(|e| e.to_string())
        }
        Step::Fan(mode) => {
            let f = FanProxy::new(conn).await.map_err(|e| e.to_string())?;
            f.set_mode(mode).await.map_err(|e| e.to_string())
        }
    }
}

/// What still differs from the profile after applying it.
///
/// Re-measured, not inferred from the steps having returned Ok. The fan is
/// checked too, and by keyword rather than by string equality: the profile may
/// say `manual:200` while the daemon reports `manual`, because the duty cycle is
/// a separate property.
async fn residual(
    conn: &zbus::Connection,
    id: &str,
    profile: &GameProfile,
) -> Result<Vec<String>, String> {
    let state = crate::mode::observe().await?;
    let replanned = gameprofile::plan(id, profile, &state);
    let mut out: Vec<String> = replanned
        .steps
        .iter()
        .filter(|s| !matches!(s, Step::Fan(_)))
        .map(|s| format!("still wants: {}", s.describe()))
        .collect();

    if let Some(want) = &profile.fan {
        let f = FanProxy::new(conn).await.map_err(|e| e.to_string())?;
        let supported = f.supported().await.unwrap_or(false);
        if !supported {
            out.push(format!(
                "fan mode '{want}': this machine has no controllable fan, so the profile's \
                 fan setting cannot take effect"
            ));
        } else {
            let now = f.mode().await.map_err(|e| e.to_string())?;
            // The keyword, not the whole string: `manual:200` is reported as
            // `manual` with the duty cycle on a separate property.
            let want_kw = FanMode::parse(want, 0)
                .map(|m| m.as_str().to_string())
                .unwrap_or_else(|_| want.clone());
            if now != want_kw {
                out.push(format!("fan mode is '{now}', profile wants '{want_kw}'"));
            }
        }
    }
    Ok(out)
}

// ── apex gaming ──────────────────────────────────────────────────────────────

/// The fixture root, honoured for the same reason `apex workload` and
/// `apex perf` honour theirs: it is what lets the suite assert on real output
/// for a machine this developer does not have — a Daily image, a Gaming image,
/// a machine with a controller — instead of only on whichever laptop ran it.
///
/// `APEX_ROOT` rather than `APEX_SYS_ROOT`: the readiness probe reads
/// `/usr/share`, `/usr/libexec`, `/etc`, `/var/lib` and `/sys`, so it is a
/// filesystem root and not a sysfs root, and reusing the sysfs name would make
/// `apex perf` and this verb disagree about what the same variable means.
pub const ROOT_ENV: &str = "APEX_ROOT";

fn probe() -> gaming::Probe {
    match std::env::var(ROOT_ENV) {
        Ok(v) if !v.trim().is_empty() => gaming::Probe::with_root(v),
        _ => gaming::Probe::new(),
    }
}

pub fn gaming_main(args: GamingArgs) -> i32 {
    let p = probe();
    let r = p.report();

    if args.json {
        println!("{}", gaming_json(&r, p.probes_programs()));
        return if r.is_ready() { 0 } else { 1 };
    }

    println!("── §12 boot-to-game readiness ──");
    kv("ready", if r.is_ready() { "yes" } else { "NO" });
    kv(
        "boots to game",
        &match r.boots_to_game() {
            Some(true) => "yes — the greeter will preselect Gaming Mode".to_string(),
            Some(false) => format!(
                "no — the greeter will preselect '{}'",
                r.preselected_session.value().map(String::as_str).unwrap_or("?")
            ),
            None => format!(
                "unknown — {}",
                r.preselected_session.reason().unwrap_or("no reason given")
            ),
        },
    );

    println!();
    println!("── the Gaming Mode session ──");
    flag("greeter entry", &r.session_desktop);
    flag("session script", &r.session_launcher);
    flag("gamescope", &r.gamescope);
    flag("steam", &r.steam);
    flag("mangoapp", &r.mangoapp);
    flag("realtime limit", &r.rtprio_limits);

    println!();
    println!("── the Desktop <-> Gaming switch ──");
    flag("switch helper", &r.switch_helper);
    flag("sudoers rule", &r.switch_sudoers);

    println!();
    println!("── controllers ──");
    match r.gamepad.value() {
        Some(pads) if pads.is_empty() => {
            kv("gamepads", "none attached");
        }
        Some(pads) => {
            for pad in pads {
                kv("  gamepad", pad);
            }
        }
        None => kv(
            "gamepads",
            &format!("unavailable — {}", r.gamepad.reason().unwrap_or("")),
        ),
    }

    let blockers = r.blockers();
    if !blockers.is_empty() {
        println!();
        println!("Gaming Mode would NOT start:");
        for b in &blockers {
            print_wrapped("  - ", b);
        }
    }
    let warnings = r.warnings();
    if !warnings.is_empty() {
        println!();
        println!("It would start, degraded:");
        for w in &warnings {
            print_wrapped("  - ", w);
        }
    }

    println!();
    if !p.probes_programs() {
        println!("Program presence was not measured: {ROOT_ENV} is set, and no filesystem");
        println!("root can redirect a PATH lookup, so the host's own programs would have");
        println!("answered for the fixture's.");
    }
    println!("Nothing above was changed. This verb only reads.");
    if r.is_ready() {
        0
    } else {
        1
    }
}

fn gaming_json(r: &Readiness, probes_programs: bool) -> String {
    let b = |name: &str, s: &apexd_core::workload::Signal<bool>| match s.value() {
        Some(v) => format!(
            "{}:{{\"value\":{v},\"source\":{}}}",
            js(name),
            js(s.source())
        ),
        None => format!(
            "{}:{{\"value\":null,\"unavailable\":{},\"source\":{}}}",
            js(name),
            js(s.reason().unwrap_or("")),
            js(s.source())
        ),
    };
    let list = |v: &[String]| v.iter().map(|x| js(x)).collect::<Vec<_>>().join(",");
    let pads = match r.gamepad.value() {
        Some(v) => format!("[{}]", list(v)),
        None => "null".to_string(),
    };
    format!(
        "{{\"ready\":{},\"probes_programs\":{},\"boots_to_game\":{},\
         \"preselected_session\":{},\"checks\":{{{}}},\"gamepads\":{},\
         \"blockers\":[{}],\"warnings\":[{}]}}",
        r.is_ready(),
        probes_programs,
        r.boots_to_game()
            .map(|v| v.to_string())
            .unwrap_or("null".into()),
        r.preselected_session
            .value()
            .map(|s| js(s))
            .unwrap_or("null".into()),
        [
            b("session_desktop", &r.session_desktop),
            b("session_launcher", &r.session_launcher),
            b("switch_helper", &r.switch_helper),
            b("switch_sudoers", &r.switch_sudoers),
            b("rtprio_limits", &r.rtprio_limits),
            b("gamescope", &r.gamescope),
            b("steam", &r.steam),
            b("mangoapp", &r.mangoapp),
        ]
        .join(","),
        pads,
        list(&r.blockers()),
        list(&r.warnings()),
    )
}

// ── rendering ────────────────────────────────────────────────────────────────

fn kv(key: &str, value: &str) {
    println!("{key:<16}: {value}");
}

/// A present/absent row that says which path it looked at when the answer is
/// "no" — the difference between "the image did not ship it" and "the fixture
/// is wrong".
fn flag(name: &str, s: &apexd_core::workload::Signal<bool>) {
    match s.value() {
        Some(true) => kv(name, "yes"),
        Some(false) => kv(name, &format!("no ({})", s.source())),
        None => kv(name, &format!("not measured — {}", s.reason().unwrap_or(""))),
    }
}

fn print_wrapped(prefix: &str, text: &str) {
    let indent = " ".repeat(prefix.len());
    for (i, line) in wrap(text, 72).into_iter().enumerate() {
        println!("{}{line}", if i == 0 { prefix } else { &indent });
    }
}

/// Wrap prose to a width. The same helper `apex mode` uses, kept local rather
/// than shared because it is four lines and a cross-module dependency for it
/// would be the larger cost.
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

/// Whether a stored mode/tier pair is one this build can still act on. Used by
/// the parity test below.
#[cfg(test)]
fn vocabulary_is_closed(p: &GameProfile) -> bool {
    p.mode
        .as_deref()
        .map(|m| m.parse::<apexd_core::mode::ModeId>().is_ok())
        != Some(false)
        && p.tier
            .as_deref()
            .map(|t| t.parse::<apexd_core::tier::Tier>().is_ok())
            != Some(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use apexd_core::mode::ModeId;
    use apexd_core::tier::Tier;

    /// An isolated `$XDG_CONFIG_HOME`, so nothing here can read or write the
    /// developer's own profiles. Serialised by a mutex because the environment
    /// is process-global and `cargo test` runs cases in parallel — without it,
    /// two cases setting the variable would read each other's files.
    struct Sandbox {
        dir: PathBuf,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    impl Sandbox {
        fn new(name: &str) -> Sandbox {
            let lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let dir = std::env::temp_dir().join(format!(
                "apex-gaming-cli-{name}-{}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("sandbox");
            std::env::set_var("XDG_CONFIG_HOME", &dir);
            Sandbox { dir, _lock: lock }
        }

        fn path(&self) -> PathBuf {
            self.dir.join("apex/games.toml")
        }
    }

    impl Drop for Sandbox {
        fn drop(&mut self) {
            std::env::remove_var("XDG_CONFIG_HOME");
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    #[test]
    fn the_games_file_lives_beside_the_blueprint_not_inside_it() {
        let s = Sandbox::new("path");
        assert_eq!(games_path(), s.path());
        // The point of the storage decision: two files, one directory.
        assert_eq!(
            games_path().parent(),
            crate::blueprint::user_blueprint_path().parent()
        );
        assert_ne!(games_path(), crate::blueprint::user_blueprint_path());
    }

    #[test]
    fn a_missing_file_loads_as_empty_rather_than_failing() {
        let _s = Sandbox::new("missing");
        let p = load().expect("a missing games file is not an error");
        assert!(p.is_empty());
    }

    #[test]
    fn what_is_saved_is_what_is_loaded() {
        let _s = Sandbox::new("roundtrip");
        let mut p = GameProfiles {
            version: Some(SCHEMA_VERSION),
            ..Default::default()
        };
        p.game.insert(
            "1091500".to_string(),
            GameProfile {
                title: Some("Cyberpunk 2077".to_string()),
                mode: Some("gaming".to_string()),
                tier: Some("balanced".to_string()),
                fan: Some("manual:200".to_string()),
                note: Some("kept".to_string()),
                ..Default::default()
            },
        );
        save(&p).expect("saves");
        assert_eq!(load().expect("loads"), p);
    }

    #[test]
    fn the_saved_file_carries_its_own_warning_about_comments() {
        let s = Sandbox::new("header");
        save(&GameProfiles::default()).expect("saves");
        let text = std::fs::read_to_string(s.path()).expect("written");
        assert!(text.contains("REWRITE"), "{text}");
        assert!(text.contains("note ="), "{text}");
    }

    #[test]
    fn set_is_incremental_and_an_empty_value_clears() {
        let mut f = Some("max".to_string());
        merge(&mut f, None);
        assert_eq!(f.as_deref(), Some("max"), "None must leave a field alone");
        merge(&mut f, Some("curve".to_string()));
        assert_eq!(f.as_deref(), Some("curve"));
        merge(&mut f, Some(String::new()));
        assert_eq!(f, None, "an empty value must clear the field");
        merge(&mut f, Some("  ".to_string()));
        assert_eq!(f, None, "whitespace is empty too");
    }

    #[test]
    fn a_second_set_does_not_lose_the_first_ones_fields() {
        // The bug a two-way merge would have: `set --fan max` silently dropping
        // a tier set last week. The same class of mistake phase 6's project
        // `remember` had with capsule bindings.
        let _s = Sandbox::new("incremental");
        assert_eq!(
            cmd_set("620", None, Some("gaming".into()), Some("balanced".into()), None, None),
            0
        );
        assert_eq!(cmd_set("620", None, None, None, Some("max".into()), None), 0);
        let p = load().expect("loads");
        let g = p.get("620").expect("stored");
        assert_eq!(g.tier_override(), Some(Tier::Balanced), "the tier survived");
        assert_eq!(g.fan.as_deref(), Some("max"));
        assert_eq!(g.mode_id(), ModeId::Gaming);
    }

    #[test]
    fn a_bad_value_is_refused_and_nothing_is_written() {
        let s = Sandbox::new("badvalue");
        assert_eq!(cmd_set("620", None, Some("turbo".into()), None, None, None), 2);
        assert!(
            !s.path().exists(),
            "a refusal must not leave a file behind"
        );
    }

    #[test]
    fn a_bad_value_does_not_corrupt_an_existing_file() {
        let _s = Sandbox::new("badvalue2");
        assert_eq!(cmd_set("620", None, Some("gaming".into()), None, None, None), 0);
        let before = load().expect("loads");
        assert_eq!(cmd_set("620", None, None, Some("ultra".into()), None, None), 2);
        assert_eq!(load().expect("loads"), before, "the file must be unchanged");
    }

    #[test]
    fn a_hostile_id_is_refused_before_the_file_is_touched() {
        let s = Sandbox::new("hostileid");
        for bad in ["-rf", "a.b", "has space", "", "x/y"] {
            assert_eq!(
                cmd_set(bad, None, None, None, None, None),
                2,
                "id {bad:?} was accepted"
            );
        }
        assert!(!s.path().exists());
    }

    #[test]
    fn remove_deletes_only_the_named_profile() {
        let _s = Sandbox::new("remove");
        cmd_set("620", None, None, None, None, None);
        cmd_set("730", None, None, None, None, None);
        assert_eq!(cmd_remove("620"), 0);
        let p = load().expect("loads");
        assert_eq!(p.ids(), vec!["730"]);
        // …and removing what is not there is a refusal, not a silent success.
        assert_ne!(cmd_remove("620"), 0);
    }

    #[test]
    fn a_broken_file_is_never_overwritten_by_set() {
        let s = Sandbox::new("broken");
        std::fs::create_dir_all(s.path().parent().unwrap()).unwrap();
        std::fs::write(s.path(), "this is not toml [[[\n").unwrap();
        assert_eq!(cmd_set("620", None, None, None, None, None), 1);
        assert_eq!(
            std::fs::read_to_string(s.path()).unwrap(),
            "this is not toml [[[\n",
            "a parse failure must not cost the user their file"
        );
    }

    #[test]
    fn the_stored_vocabulary_stays_actionable() {
        // Every value `set` accepts must still parse into something this build
        // can act on, or a profile would store cleanly and refuse to apply.
        let _s = Sandbox::new("vocab");
        for m in ModeId::all_ids() {
            for t in Tier::all_ids() {
                assert_eq!(
                    cmd_set("620", None, Some(m.clone()), Some(t.clone()), None, None),
                    0,
                    "mode {m} / tier {t} was refused by set"
                );
                let p = load().unwrap();
                assert!(vocabulary_is_closed(p.get("620").unwrap()));
            }
        }
    }

    #[test]
    fn the_launch_command_names_a_verb_this_binary_has() {
        // The string is only useful if `apex game profile apply` exists, and a
        // rename would otherwise leave a paste-ready command that does nothing.
        let c = gameprofile::launch_command("620").unwrap();
        assert!(c.starts_with("apex game profile apply "), "{c}");
        assert!(c.ends_with("%command%"), "{c}");
    }

    // ── the readiness probe, through the CLI's own root switch ───────────────

    #[test]
    fn the_root_env_switches_the_probe_and_disables_program_lookups() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("apex-gaming-root-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var(ROOT_ENV, &dir);
        let p = probe();
        assert!(
            !p.probes_programs(),
            "a fixture root must never answer from the host's PATH"
        );
        std::env::remove_var(ROOT_ENV);
        assert!(probe().probes_programs());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
