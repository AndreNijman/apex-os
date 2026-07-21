//! System-tuning profiles: the data model, loading (embedded + on-disk
//! override), and the pure tier -> [`Action`] planner.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{bail, Context, Result};
use serde::Deserialize;

use crate::tier::{Action, Tier};

/// Where the profile classifies in the layered selection hierarchy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProfileKind {
    /// Chassis-level fallback (`generic-desktop`, `generic-laptop`).
    Generic,
    /// CPU-class profile (`amd-zen`, `intel-hybrid`).
    Class,
    /// Exact-machine profile (`thinkpad-l16-g2`, `msi-katana-gf76`).
    Device,
}

/// The three-knob settings for one tier. Every field is optional so a profile
/// can express only the knobs its hardware class supports; the real
/// [`SysWriter`](crate::syswriter::SysWriter) additionally path-checks each one.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct TierSettings {
    pub governor: Option<String>,
    pub epp: Option<String>,
    pub platform_profile: Option<String>,
}

/// Default tier for AC and battery, used by the daemon's auto-switch.
#[derive(Debug, Clone, Deserialize)]
pub struct Defaults {
    pub ac: Tier,
    pub battery: Tier,
}

fn default_start_path() -> String {
    "/sys/class/power_supply/BAT0/charge_control_start_threshold".to_string()
}
fn default_end_path() -> String {
    "/sys/class/power_supply/BAT0/charge_control_end_threshold".to_string()
}

/// Battery charge-threshold configuration for a device profile.
#[derive(Debug, Clone, Deserialize)]
pub struct ChargeConfig {
    pub start: u8,
    pub stop: u8,
    #[serde(default = "default_start_path")]
    pub start_path: String,
    #[serde(default = "default_end_path")]
    pub end_path: String,
}

fn default_interval_secs() -> u64 {
    1
}
fn default_ryzenadj_tiers() -> Vec<Tier> {
    vec![Tier::UltraMax]
}

/// RyzenAdj EC-defeat loop configuration (device extra, AMD only).
#[derive(Debug, Clone, Deserialize)]
pub struct RyzenAdjConfig {
    pub stapm_mw: u32,
    pub fast_mw: u32,
    pub slow_mw: u32,
    #[serde(default)]
    pub tctl_max: Option<u32>,
    /// Hard sanity ceiling in milliwatts; any limit at or above this is
    /// clamped so a bad profile can never exceed the thermal envelope.
    pub ceiling_mw: u32,
    #[serde(default = "default_interval_secs")]
    pub interval_secs: u64,
    #[serde(default = "default_ryzenadj_tiers")]
    pub tiers: Vec<Tier>,
}

impl RyzenAdjConfig {
    /// Ceiling-clamped copy of the three limits, in milliwatts.
    pub fn clamped(&self) -> (u32, u32, u32) {
        let c = self.ceiling_mw;
        (
            self.stapm_mw.min(c),
            self.fast_mw.min(c),
            self.slow_mw.min(c),
        )
    }

    /// True if this loop should run in the given tier.
    pub fn applies_to(&self, tier: Tier) -> bool {
        self.tiers.contains(&tier)
    }
}

/// A fully-parsed system profile.
#[derive(Debug, Clone, Deserialize)]
pub struct Profile {
    pub id: String,
    pub kind: ProfileKind,
    #[serde(default)]
    pub description: String,
    pub defaults: Defaults,
    tiers: HashMap<Tier, TierSettings>,
    #[serde(default)]
    pub charge: Option<ChargeConfig>,
    #[serde(default)]
    pub ryzenadj: Option<RyzenAdjConfig>,
}

impl Profile {
    /// Parse a profile from a TOML string, validating that all five tiers are
    /// present.
    pub fn from_toml(s: &str) -> Result<Profile> {
        let p: Profile = toml::from_str(s).context("parsing profile TOML")?;
        for tier in Tier::ALL {
            if !p.tiers.contains_key(&tier) {
                bail!("profile '{}' is missing tier '{}'", p.id, tier);
            }
        }
        Ok(p)
    }

    /// Settings for a tier (always present after validation).
    pub fn tier(&self, tier: Tier) -> &TierSettings {
        self.tiers
            .get(&tier)
            .expect("all tiers validated present at load")
    }

    /// The ordered per-tier hardware plan for `tier`.
    ///
    /// This is the pure heart of the tier engine: it turns a (profile, tier)
    /// pair into the exact list of [`Action`]s a `SysWriter` should apply. It
    /// performs no I/O and is fully unit-testable.
    ///
    /// Charge thresholds are intentionally NOT part of a tier plan — they are
    /// applied independently (see [`Profile::charge_action`]).
    pub fn plan_tier(&self, tier: Tier) -> Vec<Action> {
        let mut actions = Vec::new();
        let s = self.tier(tier);
        if let Some(g) = &s.governor {
            actions.push(Action::Governor(g.clone()));
        }
        if let Some(e) = &s.epp {
            actions.push(Action::Epp(e.clone()));
        }
        if let Some(p) = &s.platform_profile {
            actions.push(Action::PlatformProfile(p.clone()));
        }
        if let Some(rz) = &self.ryzenadj {
            if rz.applies_to(tier) {
                let (stapm, fast, slow) = rz.clamped();
                actions.push(Action::RyzenAdj {
                    stapm_mw: stapm,
                    fast_mw: fast,
                    slow_mw: slow,
                    tctl_max: rz.tctl_max,
                });
            }
        }
        actions
    }

    /// The transition plan from `from` to `to`: the target tier's plan, plus a
    /// `StopRyzenAdj` teardown when leaving a ryzenadj tier for a non-ryzenadj
    /// one. This is what the daemon applies on a tier change.
    pub fn plan_transition(&self, from: Option<Tier>, to: Tier) -> Vec<Action> {
        let mut actions = Vec::new();
        if let (Some(from), Some(rz)) = (from, &self.ryzenadj) {
            if rz.applies_to(from) && !rz.applies_to(to) {
                actions.push(Action::StopRyzenAdj);
            }
        }
        actions.extend(self.plan_tier(to));
        actions
    }

    /// The charge-threshold action for this profile, if it declares one.
    pub fn charge_action(&self) -> Option<Action> {
        self.charge.as_ref().map(|c| Action::ChargeThresholds {
            start: c.start,
            stop: c.stop,
            start_path: c.start_path.clone(),
            end_path: c.end_path.clone(),
        })
    }
}

/// The set of profiles available for selection, keyed by ID.
#[derive(Debug, Clone, Default)]
pub struct ProfileSet {
    profiles: HashMap<String, Profile>,
}

/// The six profiles baked into the binary. Single source of truth is the
/// checked-in `config/sysprofiles/*.toml`; these are embedded so the daemon
/// works even if `/usr/share/apexos/sysprofiles` is missing, and so tests run
/// against the exact shipped files.
pub const BUILTIN_PROFILE_TOML: [(&str, &str); 6] = [
    (
        "generic-desktop",
        include_str!("../../../config/sysprofiles/generic-desktop.toml"),
    ),
    (
        "generic-laptop",
        include_str!("../../../config/sysprofiles/generic-laptop.toml"),
    ),
    (
        "intel-hybrid",
        include_str!("../../../config/sysprofiles/intel-hybrid.toml"),
    ),
    (
        "amd-zen",
        include_str!("../../../config/sysprofiles/amd-zen.toml"),
    ),
    (
        "thinkpad-l16-g2",
        include_str!("../../../config/sysprofiles/thinkpad-l16-g2.toml"),
    ),
    (
        "msi-katana-gf76",
        include_str!("../../../config/sysprofiles/msi-katana-gf76.toml"),
    ),
];

impl ProfileSet {
    /// The six embedded profiles.
    pub fn builtin() -> ProfileSet {
        let mut set = ProfileSet::default();
        for (name, toml) in BUILTIN_PROFILE_TOML {
            let p = Profile::from_toml(toml)
                .unwrap_or_else(|e| panic!("builtin profile '{name}' failed to parse: {e:#}"));
            assert_eq!(p.id, name, "builtin profile id mismatch for {name}");
            set.profiles.insert(p.id.clone(), p);
        }
        set
    }

    /// Load profiles from a directory of `*.toml` files, falling back to the
    /// embedded set if the directory is absent or empty. A malformed file is a
    /// hard error (fail loud rather than silently mistune).
    pub fn load(dir: Option<&Path>) -> Result<ProfileSet> {
        let Some(dir) = dir else {
            return Ok(ProfileSet::builtin());
        };
        if !dir.is_dir() {
            return Ok(ProfileSet::builtin());
        }
        let mut set = ProfileSet::default();
        let mut count = 0;
        for entry in std::fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("toml") {
                continue;
            }
            let text = std::fs::read_to_string(&path)
                .with_context(|| format!("reading {}", path.display()))?;
            let p = Profile::from_toml(&text)
                .with_context(|| format!("parsing {}", path.display()))?;
            set.profiles.insert(p.id.clone(), p);
            count += 1;
        }
        if count == 0 {
            return Ok(ProfileSet::builtin());
        }
        Ok(set)
    }

    /// Look up a profile by ID.
    pub fn get(&self, id: &str) -> Option<&Profile> {
        self.profiles.get(id)
    }

    /// Number of profiles in the set.
    pub fn len(&self) -> usize {
        self.profiles.len()
    }

    /// True if the set has no profiles.
    pub fn is_empty(&self) -> bool {
        self.profiles.is_empty()
    }
}
