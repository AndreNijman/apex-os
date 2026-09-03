//! Per-game profiles (roadmap §12): schema, validation, and the pure planner.
//!
//! §12 asks for "per-game profiles" alongside the controller-first boot-to-game
//! flow. A profile here is a *named composition of levers that already ship* —
//! the §11 mode, the power tier and the fan mode — stored per title and applied
//! when that title is about to run. Nothing in this module is a new hardware
//! lever, and nothing in it performs I/O.
//!
//! ── Where a profile is stored, and why it is not in the blueprint ────────────
//!
//! Profiles live in their own file, `~/.config/apex/games.toml`, next to the
//! blueprint but not inside it. The deciding argument is the blueprint's own
//! stated contract, in [`crate::blueprint`]'s module docs:
//!
//! > **Desired** — [`Blueprint`](crate::blueprint::Blueprint). Hand-written,
//! > user-owned, the only file a person or a future GUI edits […] Nothing in
//! > APEX ever rewrites it behind the user's back.
//!
//! `apex game profile set` is a program that writes. Putting a program-written
//! table inside the one file whose contract is that no program writes it breaks
//! that contract directly, and it would do so for every user who ever runs the
//! convenience verb rather than hand-editing TOML.
//!
//! Two supporting reasons:
//!
//! * **Different lifecycle.** A blueprint describes one *machine* and is meant
//!   to stay short and hand-readable; a games file grows one entry per title and
//!   is mostly written on the user's behalf. §10's own example blueprint is
//!   twenty lines.
//! * **Different cardinality on `apply`.** Every blueprint section is either
//!   converged or carries a `blocked` reason. A game profile is neither: it is
//!   *selected* when a game runs, not converged toward. It would have to become
//!   a third kind of section, and phase 7 already pays for two.
//!
//! ── Which of the three kinds of state this is ───────────────────────────────
//!
//! §10's rule — keep generated/system state separate from user-owned state — is
//! the one this had to be checked against, because `apex game profile set`
//! writes the file. `games.toml` is **desired, user-owned state**, on a test
//! that is about *what causes a write* rather than about who typed it:
//!
//! * It is written **only in response to an explicit user command**, and only
//!   with what that command was told. No reconcile, no timer and no probe ever
//!   writes it, so it can never come to disagree with what the user asked for.
//! * Nothing reads it back as a measurement. Applying a profile re-reads the
//!   machine over D-Bus, exactly as `apex mode set` does.
//!
//! That is why it keeps `deny_unknown_fields` and hand-editability. Contrast
//! `apex_agent_core::config::Config`, which keeps unknown keys — its reason is
//! that *two* programs at possibly different versions write it. This file has
//! exactly one program writer, so an unrecognised key here is a typo.
//!
//! ── What a profile can and cannot set ───────────────────────────────────────
//!
//! Executable, over the frozen `org.apexos.Apexd1` surface a user could drive by
//! hand, all behind the `manage-power` polkit action which ships
//! `allow_active = yes` (so an active local session authorises without a
//! prompt — verified against
//! `files/system/polkit-1/actions/org.apexos.apexd.policy` and the
//! `authorize(conn, &hdr, ACTION_POWER)` calls in the daemon's `dbus.rs`):
//!
//! * `mode` — a §11 mode, which is itself tier + auto-switch + game mode;
//! * `tier` — an override of that mode's tier, for one title;
//! * `fan`  — `Fan.SetMode`, the one lever `apex mode` models but never touches.
//!
//! **Refused, with a reason rather than silently ignored:** a per-game
//! `scheduler` or `gpu` clock policy. Both already exist, and both are chosen by
//! the *sysprofile*'s `[game]` section and applied by the daemon when game mode
//! starts — `scx` loads the sched-ext scheduler, `[game.nvidia]` locks the
//! clocks. There is no D-Bus member that sets either per title, so a profile
//! that accepted `scheduler = "scx_rusty"` would read as a reviewed setting and
//! do nothing. That is the failure mode the plugin platform refused permissions
//! over, and it is refused here the same way: the keys exist in the schema for
//! the sole purpose of producing a message that says where the setting really
//! lives.
//!
//! They are still *composed*, at mode granularity: `mode = "gaming"` turns game
//! mode on, and game mode is what loads the scheduler and locks the clocks.
//! What is refused is naming a different one per title.

use std::collections::BTreeMap;
use std::fmt;

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

use crate::fan::FanMode;
use crate::mode::{self, Mode, ModeId, ModeState, TierPolicy};
use crate::tier::Tier;

/// Schema version of the games file. Bumped only when an older `apex` could
/// misread a newer file *silently*; additive keys do not need it, because
/// unknown keys are refused loudly.
pub const SCHEMA_VERSION: u32 = 1;

/// The mode a profile that names none is understood to mean.
///
/// A file called `games.toml` whose entries are titles is asking for the gaming
/// policy; defaulting to `daily` would make the common profile a no-op that
/// still reads like a tuning decision.
pub const DEFAULT_MODE: ModeId = ModeId::Gaming;

/// The longest a game id may be. Steam AppIDs are at most seven digits; the
/// slack is for hand-written slugs.
const MAX_ID: usize = 64;

/// The longest a title may be. It is only ever printed, but it arrives from a
/// file and there is no reason for it to be unbounded.
const MAX_TITLE: usize = 200;

// ── the file ─────────────────────────────────────────────────────────────────

/// `~/.config/apex/games.toml` — every per-game profile on this machine.
///
/// A `BTreeMap` rather than an array of tables: the id is the identity, so a
/// duplicate must be impossible by construction rather than by a validation
/// pass, and a sorted map serialises deterministically, which is what makes the
/// round-trip lossless instead of merely reversible.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GameProfiles {
    /// File-format version. Absent means [`SCHEMA_VERSION`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<u32>,
    /// Profiles by id — a Steam AppID (`1091500`) or a slug for anything else.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub game: BTreeMap<String, GameProfile>,
}

/// One title's profile. Every field is optional and absent means *unmanaged*,
/// never "set to the default" — the same rule the blueprint follows, for the
/// same reason: a profile that silently asserted a value for everything it did
/// not mention would change the machine in ways nobody wrote down.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GameProfile {
    /// A human label. Purely for `list` and `show`; nothing keys off it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// A §11 mode id. Absent means [`DEFAULT_MODE`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    /// Override the mode's power tier for this title only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tier: Option<String>,
    /// `auto`, `max`, `curve` or `manual:<0-255>` — the shipped `apex fan`
    /// vocabulary, validated through [`FanMode::parse`] so the two cannot drift.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fan: Option<String>,
    /// A free-text reminder of why this profile is what it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,

    // ── recognised only to be refused ────────────────────────────────────────
    //
    // `deny_unknown_fields` would already reject these, with a message that
    // lists every legal key and explains nothing. They are declared here so the
    // refusal can say where the setting actually lives. Neither can ever survive
    // `parse`, so neither is ever serialised.
    /// **Refused.** See [`GameProfiles::validate`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheduler: Option<String>,
    /// **Refused.** See [`GameProfiles::validate`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gpu: Option<String>,
}

impl GameProfile {
    /// The mode this profile asks for, defaulted. Only meaningful after
    /// validation, so an unparseable mode falls back rather than panicking.
    pub fn mode_id(&self) -> ModeId {
        self.mode
            .as_deref()
            .and_then(|m| m.parse().ok())
            .unwrap_or(DEFAULT_MODE)
    }

    /// The tier override, if any and if parseable.
    pub fn tier_override(&self) -> Option<Tier> {
        self.tier.as_deref().and_then(|t| t.parse().ok())
    }

    /// What to show for a profile in a one-line listing.
    pub fn label(&self, id: &str) -> String {
        match &self.title {
            Some(t) => format!("{id} ({t})"),
            None => id.to_string(),
        }
    }
}

impl GameProfiles {
    /// Parse and validate a games file.
    ///
    /// Both halves are errors, never warnings — a profile naming a mode that
    /// does not exist is worse than one that fails to parse, because it would
    /// apply "successfully" and change nothing.
    pub fn parse(text: &str) -> Result<GameProfiles> {
        let profiles: GameProfiles = toml::from_str(text)
            .map_err(|e| anyhow::anyhow!("not a valid games file: {e}"))?;
        let problems = profiles.validate();
        if !problems.is_empty() {
            bail!("{}", problems.join("\n"));
        }
        Ok(profiles)
    }

    /// Render back to TOML, ready to write.
    ///
    /// The inverse of [`GameProfiles::parse`] for every value that can survive
    /// parsing, which is what `tests/test-apex-gaming.sh` and the round-trip
    /// unit test assert over a fully populated profile.
    pub fn to_toml(&self) -> Result<String> {
        Ok(toml::to_string_pretty(self)?)
    }

    /// Everything wrong with this file, as user-facing lines.
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

        for (id, p) in &self.game {
            if let Err(why) = check_game_id(id) {
                out.push(format!("[game.{id}]: {why}"));
            }
            if let Some(t) = &p.title {
                if t.len() > MAX_TITLE {
                    out.push(format!(
                        "[game.{id}] title is longer than {MAX_TITLE} characters"
                    ));
                }
            }
            if let Some(m) = &p.mode {
                if m.parse::<ModeId>().is_err() {
                    out.push(format!(
                        "[game.{id}] mode = {m:?} is not one of: {}",
                        ModeId::all_ids().join(", ")
                    ));
                }
            }
            if let Some(t) = &p.tier {
                if t.parse::<Tier>().is_err() {
                    out.push(format!(
                        "[game.{id}] tier = {t:?} is not one of: {}",
                        Tier::all_ids().join(", ")
                    ));
                }
            }
            if let Some(f) = &p.fan {
                // Parsed through the shipped fan vocabulary rather than a list
                // copied next to it. The default manual PWM is irrelevant to
                // whether the keyword is legal, so any value does; the daemon
                // supplies the real one from the sysprofile when it applies it.
                if FanMode::parse(f, 0).is_err() {
                    out.push(format!(
                        "[game.{id}] fan = {f:?} is not one of: auto, max, curve, \
                         manual, manual:<0-255>"
                    ));
                }
            }
            if p.scheduler.is_some() {
                out.push(format!(
                    "[game.{id}] scheduler: a per-game sched-ext scheduler cannot be set. \
                     The scheduler is chosen by the sysprofile's `[game] scx` and loaded by \
                     apexd when game mode starts, and there is no D-Bus member that changes \
                     it per title — so this key would read as a setting and do nothing. \
                     `mode = \"gaming\"` is what turns it on."
                ));
            }
            if p.gpu.is_some() {
                out.push(format!(
                    "[game.{id}] gpu: a per-game GPU clock policy cannot be set. GPU clock \
                     locks come from the sysprofile's `[game.nvidia]` and are applied by \
                     apexd when game mode starts, so this key would read as a setting and do \
                     nothing. `mode = \"gaming\"` is what turns it on."
                ));
            }
        }

        out
    }

    /// The profile for an id, if there is one.
    pub fn get(&self, id: &str) -> Option<&GameProfile> {
        self.game.get(id)
    }

    /// Ids in listing order.
    pub fn ids(&self) -> Vec<&str> {
        self.game.keys().map(String::as_str).collect()
    }

    /// Whether anything is stored at all.
    pub fn is_empty(&self) -> bool {
        self.game.is_empty()
    }
}

/// Why a game id is unacceptable, if it is.
///
/// A hostile-input boundary, not a nicety. The id is a **TOML table key**, so a
/// `.` in it would silently nest one profile inside another; it is printed into
/// shell snippets by `launch-command`; and it reaches argv. Steam AppIDs are
/// digits and slugs are words, so the accepted set is deliberately narrower than
/// what TOML would tolerate.
pub fn check_game_id(id: &str) -> Result<(), String> {
    if id.is_empty() {
        return Err("the id is empty".into());
    }
    if id.len() > MAX_ID {
        return Err(format!("the id is longer than {MAX_ID} characters"));
    }
    if id.starts_with('-') {
        return Err("the id starts with '-', which would be read as a command-line flag".into());
    }
    if !id
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphanumeric())
    {
        return Err("the id must start with a letter or a digit".into());
    }
    if let Some(bad) = id
        .chars()
        .find(|c| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '-')))
    {
        if bad == '.' {
            return Err(
                "the id contains '.', which TOML would read as a nested table rather than \
                 part of the name; use '-' or '_'"
                    .into(),
            );
        }
        return Err(format!(
            "the id contains {bad:?}; only letters, digits, '_' and '-' are allowed"
        ));
    }
    Ok(())
}

// ── the plan ─────────────────────────────────────────────────────────────────

/// One action applying a profile will take, in the order it must be taken.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// A §11 mode step: `Power.SetAutoSwitch`, `Power.SetTier` or
    /// `GameMode.SetActive`.
    Policy(mode::Step),
    /// `org.apexos.Apexd1.Fan.SetMode`. The one lever `apex mode` models and
    /// deliberately never touches, because a mode is a power policy and a fan
    /// mode is a per-title noise/thermal choice.
    Fan(String),
}

impl Step {
    /// A stable, log-friendly rendering, matching `mode::Step::describe`.
    pub fn describe(&self) -> String {
        match self {
            Step::Policy(s) => s.describe(),
            Step::Fan(m) => format!("fan mode -> {m}"),
        }
    }
}

impl fmt::Display for Step {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.describe())
    }
}

/// What applying a profile amounts to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolution {
    /// The id resolved.
    pub id: String,
    /// The §11 mode the profile composes.
    pub mode: ModeId,
    /// The tier policy actually in force, after any per-title override.
    pub tier: TierPolicy,
    /// Whether the profile overrode the mode's own tier.
    pub tier_overridden: bool,
    /// The ordered steps.
    pub steps: Vec<Step>,
    /// Why the plan is shaped the way it is. Printed by `show` and by a dry
    /// run, because an ordering rule nobody can see is one the next edit
    /// removes.
    pub notes: Vec<String>,
    /// What the profile relies on but does not itself set. Reported, never
    /// silently assumed.
    pub reported: Vec<String>,
}

impl Resolution {
    /// True when the machine is already in the profile's state.
    pub fn is_noop(&self) -> bool {
        self.steps.is_empty()
    }
}

/// Turn a profile and the machine's observed state into an ordered plan.
///
/// Pure: same inputs, same plan, every time. `apex game profile apply` executes
/// exactly these steps and nothing else, and `--dry-run` prints exactly the same
/// list — the two call this once each and differ only in whether the result
/// reaches a D-Bus proxy. That is what makes the dry run a report rather than a
/// rehearsal of a different program, and it is phase 7's rule applied to a
/// second subject.
///
/// ## The ordering rules, and why two of them are not cosmetic
///
/// The §11 ordering still holds and is inherited by calling [`mode::plan`]:
/// leave game mode first (its restore path moves the tier), turn auto-switch off
/// before pinning, enter game mode last.
///
/// Two more exist here, and both come from what the **daemon** does inside
/// `GameMode.SetActive` — see `apexd/src/game.rs::game_enter`, which applies the
/// *sysprofile's* `[game] tier` and `[game] fan_mode` after entering:
///
/// 1. **A pinned tier is re-asserted after entering game mode.** `game_enter`
///    calls `apply_tier(cfg.tier)` itself, so a tier set before it is overwritten
///    by whatever the machine's sysprofile says a game session should run at. The
///    CLI cannot read that value — it is daemon-side — so the only correct move
///    is to set the tier again afterwards. Without this, a profile asking for
///    `balanced` on a machine whose sysprofile pins `performance` reports success
///    and lands on performance.
/// 2. **The fan step is last, after game mode.** `game_enter` also sets the fan
///    from `cfg.fan_mode` and records the prior value for its own restore. A fan
///    step emitted earlier would be overwritten by that, silently.
///
/// Both are asserted by unit tests that mutate the order and watch it fail.
pub fn plan(id: &str, profile: &GameProfile, state: &ModeState) -> Resolution {
    let mode_id = profile.mode_id();
    let base = mode_id.spec();

    // The mode with this title's tier substituted. `Mode` is `Copy` and holds
    // only static data, so overriding one field is a value, not a mutation of
    // the shared catalogue.
    let tier_override = profile.tier_override();
    let effective: Mode = match tier_override {
        Some(t) => Mode {
            tier: TierPolicy::Pinned(t),
            ..*base
        },
        None => *base,
    };

    let mut notes = Vec::new();
    let mut reported = Vec::new();

    if profile.mode.is_none() {
        notes.push(format!(
            "no mode named, so the profile composes '{DEFAULT_MODE}' — the mode a game \
             profile means unless it says otherwise"
        ));
    }
    if let Some(t) = tier_override {
        notes.push(format!(
            "tier overridden to {t} for this title; mode '{mode_id}' would otherwise use {}",
            describe_policy(base.tier)
        ));
    }

    let mut steps: Vec<Step> = mode::plan(&effective, state)
        .into_iter()
        .map(Step::Policy)
        .collect();

    let entering_game = steps
        .iter()
        .any(|s| matches!(s, Step::Policy(mode::Step::GameMode(true))));

    // Rule 1. See the doc comment: game_enter applies the sysprofile's own
    // `[game] tier` after we set ours, so a pinned tier has to be set again.
    if entering_game {
        if let TierPolicy::Pinned(t) = effective.tier {
            steps.push(Step::Policy(mode::Step::SetTier(t)));
            notes.push(format!(
                "tier {t} is set again AFTER game mode, because entering game mode applies \
                 the sysprofile's own `[game] tier` and would otherwise overwrite it"
            ));
        }
    }

    // Rule 2. The fan step is last for the same reason, one lever along.
    if let Some(f) = &profile.fan {
        steps.push(Step::Fan(f.clone()));
        if entering_game {
            notes.push(
                "the fan step is last, because entering game mode applies the sysprofile's \
                 own `[game] fan_mode` and would otherwise overwrite it"
                    .to_string(),
            );
        }
    }

    if effective.game {
        reported.push(
            "the sched-ext scheduler comes from the sysprofile's `[game] scx` and is loaded \
             by apexd when game mode starts — it is not chosen per title"
                .to_string(),
        );
        reported.push(
            "GPU clock locks come from the sysprofile's `[game.nvidia]` and are applied by \
             apexd when game mode starts — they are not chosen per title"
                .to_string(),
        );
    }
    for s in effective.services {
        reported.push(format!(
            "service {} -> {} ({})",
            s.unit,
            s.want.as_str(),
            s.why
        ));
    }

    Resolution {
        id: id.to_string(),
        mode: mode_id,
        tier: effective.tier,
        tier_overridden: tier_override.is_some(),
        steps,
        notes,
        reported,
    }
}

/// Render a tier policy the way `apex mode` does.
fn describe_policy(p: TierPolicy) -> String {
    match p {
        TierPolicy::Auto => "auto (the profile's AC/battery defaults)".to_string(),
        TierPolicy::Pinned(t) => format!("{t} (pinned)"),
    }
}

/// The Steam launch-option line that applies a profile and then runs the game.
///
/// Built from verbs that exist today and nothing else. APEX ships no launch
/// wrapper binary — see the module docs of `apex/src/gaming.rs` — so this is a
/// plain `&&` between the shipped selection verb and Steam's own `%command%`
/// placeholder, which is exactly what a user would type.
///
/// The id has already been through [`check_game_id`], so it carries no quote, no
/// space and no shell metacharacter; it is still rendered through a checked path
/// rather than interpolated on trust.
pub fn launch_command(id: &str) -> Result<String, String> {
    check_game_id(id)?;
    Ok(format!("apex game profile apply {id} && %command%"))
}

/// Ids that are almost certainly a mistake to store, with the reason.
///
/// Not a refusal: a user may genuinely want a profile keyed on something odd.
/// It is a warning surfaced by `set`, because the common mistake — pasting a
/// Steam *store URL* fragment or the game's executable name — produces an id
/// that never matches anything and fails silently.
pub fn id_advice(id: &str) -> Option<String> {
    if id.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    if id.ends_with("-exe") || id.ends_with("_exe") {
        return Some(
            "that looks like an executable name; a Steam title is identified by its numeric \
             AppID, which is the number in its store URL"
                .to_string(),
        );
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mode::Step as ModeStep;

    fn state(tier: Tier, auto: bool, game: bool) -> ModeState {
        ModeState {
            tier,
            auto_switch: auto,
            game_active: game,
        }
    }

    /// Every field a profile can carry *after* validation, populated.
    fn full() -> GameProfiles {
        let mut game = BTreeMap::new();
        game.insert(
            "1091500".to_string(),
            GameProfile {
                title: Some("Cyberpunk 2077".to_string()),
                mode: Some("gaming".to_string()),
                tier: Some("performance".to_string()),
                fan: Some("manual:200".to_string()),
                note: Some("locks up on balanced".to_string()),
                scheduler: None,
                gpu: None,
            },
        );
        game.insert(
            "my-old-shooter".to_string(),
            GameProfile {
                title: None,
                mode: Some("couch".to_string()),
                tier: None,
                fan: Some("auto".to_string()),
                note: None,
                scheduler: None,
                gpu: None,
            },
        );
        GameProfiles {
            version: Some(SCHEMA_VERSION),
            game,
        }
    }

    // ── the round trip ───────────────────────────────────────────────────────

    #[test]
    fn a_fully_populated_file_round_trips_losslessly() {
        let original = full();
        let text = original.to_toml().expect("serialises");
        let back = GameProfiles::parse(&text).expect("parses back");
        assert_eq!(original, back, "round trip lost something:\n{text}");
        // And again, so a normalising serialiser cannot pass by converging on
        // the second pass.
        let text2 = back.to_toml().expect("serialises");
        assert_eq!(text, text2, "serialisation is not stable");
    }

    #[test]
    fn an_empty_file_round_trips_to_an_empty_file() {
        let empty = GameProfiles::default();
        let text = empty.to_toml().expect("serialises");
        assert_eq!(GameProfiles::parse(&text).expect("parses"), empty);
        assert!(empty.is_empty());
    }

    #[test]
    fn a_hand_written_file_parses_with_the_values_written() {
        let p = GameProfiles::parse(
            r#"
            version = 1
            [game.1091500]
            title = "Cyberpunk 2077"
            mode = "gaming"
            tier = "balanced"
            fan = "max"
            "#,
        )
        .expect("valid");
        let g = p.get("1091500").expect("present");
        assert_eq!(g.mode_id(), ModeId::Gaming);
        assert_eq!(g.tier_override(), Some(Tier::Balanced));
        assert_eq!(g.fan.as_deref(), Some("max"));
    }

    #[test]
    fn an_id_that_is_all_digits_survives_a_round_trip_as_a_quoted_key() {
        // A bare-digit TOML key is legal but is the case a serialiser is most
        // likely to mangle, and every Steam title has one.
        let mut game = BTreeMap::new();
        game.insert("620".to_string(), GameProfile::default());
        let p = GameProfiles { version: None, game };
        let text = p.to_toml().unwrap();
        assert_eq!(GameProfiles::parse(&text).unwrap(), p, "{text}");
    }

    // ── validation ───────────────────────────────────────────────────────────

    #[test]
    fn an_unknown_key_is_refused_rather_than_ignored() {
        let e = GameProfiles::parse("[game.620]\nturbo = true\n").unwrap_err();
        assert!(format!("{e}").contains("turbo"), "{e}");
    }

    #[test]
    fn an_unknown_mode_tier_or_fan_is_refused_and_lists_the_real_ones() {
        let e = GameProfiles::parse("[game.620]\nmode = \"turbo\"\n").unwrap_err();
        assert!(format!("{e}").contains("gaming"), "{e}");
        let e = GameProfiles::parse("[game.620]\ntier = \"ultra\"\n").unwrap_err();
        assert!(format!("{e}").contains("performance"), "{e}");
        let e = GameProfiles::parse("[game.620]\nfan = \"loud\"\n").unwrap_err();
        assert!(format!("{e}").contains("manual:<0-255>"), "{e}");
    }

    #[test]
    fn every_shipped_fan_keyword_is_accepted() {
        for kw in ["auto", "max", "full", "curve", "manual", "manual:200"] {
            let text = format!("[game.620]\nfan = {kw:?}\n");
            assert!(
                GameProfiles::parse(&text).is_ok(),
                "the shipped fan vocabulary rejected {kw:?}"
            );
        }
    }

    #[test]
    fn every_mode_id_is_accepted_as_a_profile_mode() {
        for id in ModeId::all_ids() {
            let text = format!("[game.620]\nmode = {id:?}\n");
            assert!(GameProfiles::parse(&text).is_ok(), "mode {id} was refused");
        }
    }

    #[test]
    fn a_per_game_scheduler_is_refused_and_says_where_it_lives() {
        let e = GameProfiles::parse("[game.620]\nscheduler = \"scx_rusty\"\n").unwrap_err();
        let msg = format!("{e}");
        assert!(msg.contains("scx"), "{msg}");
        assert!(msg.contains("sysprofile"), "{msg}");
    }

    #[test]
    fn a_per_game_gpu_policy_is_refused_and_says_where_it_lives() {
        let e = GameProfiles::parse("[game.620]\ngpu = \"locked\"\n").unwrap_err();
        let msg = format!("{e}");
        assert!(msg.contains("nvidia"), "{msg}");
        assert!(msg.contains("sysprofile"), "{msg}");
    }

    #[test]
    fn a_version_from_the_future_is_refused() {
        let e = GameProfiles::parse("version = 99\n").unwrap_err();
        assert!(format!("{e}").contains("99"), "{e}");
    }

    // ── ids ──────────────────────────────────────────────────────────────────

    #[test]
    fn hostile_ids_are_refused_one_reason_each() {
        assert!(check_game_id("").is_err());
        assert!(check_game_id(&"a".repeat(MAX_ID + 1)).is_err());
        assert!(check_game_id("-rf").is_err());
        assert!(check_game_id("_leading").is_err());
        assert!(check_game_id("has space").is_err());
        assert!(check_game_id("has/slash").is_err());
        assert!(check_game_id("has;semi").is_err());
        assert!(check_game_id("has$dollar").is_err());
        // The one that would corrupt the file rather than the command line.
        let e = check_game_id("a.b").unwrap_err();
        assert!(e.contains("nested table"), "{e}");
    }

    #[test]
    fn ordinary_ids_are_accepted() {
        for id in ["620", "1091500", "my-old-shooter", "Doom_1993", "a"] {
            assert!(check_game_id(id).is_ok(), "{id} was refused");
        }
    }

    #[test]
    fn a_bad_id_in_a_file_is_a_parse_failure_not_a_silent_entry() {
        // Quoted, so TOML itself accepts it and only validation can catch it.
        let e = GameProfiles::parse("[game.\"has space\"]\nmode = \"gaming\"\n").unwrap_err();
        assert!(format!("{e}").contains("has space"), "{e}");
    }

    // ── the planner ──────────────────────────────────────────────────────────

    #[test]
    fn an_empty_profile_composes_the_gaming_mode() {
        let r = plan("620", &GameProfile::default(), &state(Tier::Balanced, true, false));
        assert_eq!(r.mode, ModeId::Gaming);
        assert!(
            r.notes.iter().any(|n| n.contains("no mode named")),
            "the default has to be stated, not assumed: {:?}",
            r.notes
        );
    }

    #[test]
    fn the_plan_inherits_the_mode_ordering() {
        // Leaving game mode first, then auto-switch off, then the tier.
        let p = GameProfile {
            mode: Some("battery".to_string()),
            ..Default::default()
        };
        let r = plan("620", &p, &state(Tier::Performance, true, true));
        let s: Vec<String> = r.steps.iter().map(Step::describe).collect();
        assert!(s[0].contains("game mode off"), "{s:?}");
        assert!(s[1].contains("auto-switch off"), "{s:?}");
        assert!(s[2].contains("power-saver"), "{s:?}");
    }

    #[test]
    fn a_pinned_tier_is_re_asserted_after_entering_game_mode() {
        // THE rule. `game_enter` applies the sysprofile's own `[game] tier`
        // after GameMode.SetActive, so a tier set before it does not survive.
        let p = GameProfile {
            mode: Some("gaming".to_string()),
            tier: Some("balanced".to_string()),
            ..Default::default()
        };
        let r = plan("620", &p, &state(Tier::PowerSaver, true, false));
        let pos_game = r
            .steps
            .iter()
            .position(|s| matches!(s, Step::Policy(ModeStep::GameMode(true))))
            .expect("the gaming mode enters game mode");
        let last_tier = r
            .steps
            .iter()
            .rposition(|s| matches!(s, Step::Policy(ModeStep::SetTier(_))))
            .expect("a pinned tier is set");
        assert!(
            last_tier > pos_game,
            "the tier must be re-asserted after game mode, or the daemon overwrites it: {:?}",
            r.steps.iter().map(Step::describe).collect::<Vec<_>>()
        );
        assert_eq!(
            r.steps[last_tier],
            Step::Policy(ModeStep::SetTier(Tier::Balanced)),
            "the re-assert must carry the profile's tier, not the mode's"
        );
        assert!(
            r.notes.iter().any(|n| n.contains("AFTER game mode")),
            "the reason must travel with the plan: {:?}",
            r.notes
        );
    }

    #[test]
    fn no_tier_is_re_asserted_when_game_mode_is_not_entered() {
        let p = GameProfile {
            mode: Some("development".to_string()),
            ..Default::default()
        };
        let r = plan("620", &p, &state(Tier::Balanced, true, false));
        let tiers = r
            .steps
            .iter()
            .filter(|s| matches!(s, Step::Policy(ModeStep::SetTier(_))))
            .count();
        assert_eq!(tiers, 1, "a mode that never enters game mode needs one write");
    }

    #[test]
    fn the_fan_step_is_last_of_all() {
        let p = GameProfile {
            mode: Some("gaming".to_string()),
            fan: Some("max".to_string()),
            ..Default::default()
        };
        let r = plan("620", &p, &state(Tier::Balanced, true, false));
        assert!(
            matches!(r.steps.last(), Some(Step::Fan(f)) if f == "max"),
            "entering game mode sets the fan from the sysprofile, so ours must come after: {:?}",
            r.steps.iter().map(Step::describe).collect::<Vec<_>>()
        );
        assert!(
            r.notes.iter().any(|n| n.contains("fan step is last")),
            "{:?}",
            r.notes
        );
    }

    #[test]
    fn a_profile_the_machine_already_satisfies_plans_nothing() {
        let p = GameProfile {
            mode: Some("gaming".to_string()),
            ..Default::default()
        };
        let r = plan("620", &p, &state(Tier::Performance, false, true));
        assert!(r.is_noop(), "{:?}", r.steps);
    }

    #[test]
    fn a_fan_only_profile_still_plans_the_fan_when_nothing_else_moves() {
        let p = GameProfile {
            mode: Some("gaming".to_string()),
            fan: Some("curve".to_string()),
            ..Default::default()
        };
        let r = plan("620", &p, &state(Tier::Performance, false, true));
        assert_eq!(r.steps, vec![Step::Fan("curve".to_string())]);
    }

    #[test]
    fn what_the_profile_relies_on_but_does_not_set_is_reported() {
        let p = GameProfile {
            mode: Some("gaming".to_string()),
            ..Default::default()
        };
        let r = plan("620", &p, &state(Tier::Balanced, true, false));
        let joined = r.reported.join("\n");
        assert!(joined.contains("scx"), "{joined}");
        assert!(joined.contains("nvidia"), "{joined}");
        assert!(joined.contains("irqbalance"), "{joined}");
    }

    #[test]
    fn a_non_gaming_mode_does_not_claim_the_scheduler() {
        let p = GameProfile {
            mode: Some("couch".to_string()),
            ..Default::default()
        };
        let r = plan("620", &p, &state(Tier::Balanced, true, false));
        assert!(
            !r.reported.join("\n").contains("scx"),
            "couch does not enter game mode, so it must not claim the scheduler"
        );
    }

    #[test]
    fn the_plan_is_pure() {
        // Same inputs, same plan — the property a dry run's honesty rests on.
        let p = GameProfile {
            mode: Some("gaming".to_string()),
            tier: Some("balanced".to_string()),
            fan: Some("max".to_string()),
            ..Default::default()
        };
        let s = state(Tier::PowerSaver, true, false);
        assert_eq!(plan("620", &p, &s), plan("620", &p, &s));
    }

    #[test]
    fn every_mode_plans_without_panicking_from_every_start_state() {
        for id in ModeId::ALL {
            for tier in Tier::ALL {
                for auto in [true, false] {
                    for game in [true, false] {
                        let p = GameProfile {
                            mode: Some(id.as_str().to_string()),
                            ..Default::default()
                        };
                        let r = plan("620", &p, &state(tier, auto, game));
                        assert_eq!(r.mode, id);
                    }
                }
            }
        }
    }

    // ── the launch command ───────────────────────────────────────────────────

    #[test]
    fn the_launch_command_uses_only_verbs_that_exist() {
        let c = launch_command("1091500").unwrap();
        assert_eq!(c, "apex game profile apply 1091500 && %command%");
    }

    #[test]
    fn the_launch_command_refuses_an_id_it_would_have_to_quote() {
        assert!(launch_command("has space").is_err());
        assert!(launch_command("; rm -rf /").is_err());
    }

    #[test]
    fn an_executable_looking_id_is_advised_against_but_allowed() {
        assert!(id_advice("doom-exe").is_some());
        assert!(id_advice("1091500").is_none());
        assert!(id_advice("my-old-shooter").is_none());
    }
}
