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

// ── M6: fan control ──────────────────────────────────────────────────────────

/// Which fan backend a profile wants used.
///
/// `Auto` (the default) probes in descending order of usefulness — real hwmon
/// PWM, then `msi-wmi-platform` (RPM only), then `msi-ec` — and takes whatever
/// is actually present. Naming a backend explicitly never *asserts* that it
/// exists: every leg still probes, and a named-but-absent backend degrades to
/// "unsupported" rather than to an error.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FanBackend {
    #[default]
    Auto,
    /// Generic hwmon `pwmN`/`fanN_input` only.
    Hwmon,
    /// The `msi-wmi-platform` hwmon device only (read-only fan RPM: the driver
    /// registers four `fanN_input` channels and no PWM at all).
    MsiWmi,
    /// The MSI embedded controller only.
    MsiEc,
    /// Fan control disabled for this machine.
    None,
}

/// One point on a fan curve.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
pub struct CurvePoint {
    /// Temperature in degrees Celsius.
    pub temp_c: f64,
    /// PWM duty cycle (0-255) at that temperature.
    pub pwm: u8,
}

fn default_min_pwm() -> u8 {
    // ~30% — low enough to be quiet, high enough that no curve, clamp bug or
    // rounding error can ever command a stopped fan.
    77
}
fn default_max_pwm() -> u8 {
    255
}
fn default_boost_threshold() -> u8 {
    // Above this duty cycle, an msi-ec machine (which has no PWM) turns cooler
    // boost on instead.
    224
}
fn default_curve_interval() -> u64 {
    3
}

/// Fan policy for a profile. Every field has a default, so a profile with no
/// `[fan]` table behaves exactly as it did before M6.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct FanConfig {
    pub backend: FanBackend,
    /// Never command a duty cycle below this (safety floor).
    pub min_pwm: u8,
    pub max_pwm: u8,
    /// msi-ec cooler-boost threshold for `manual` duty cycles.
    pub boost_pwm_threshold: u8,
    /// Mode applied when the daemon starts. `None` leaves the firmware alone.
    pub default_mode: Option<String>,
    /// msi-ec `fan_mode` value meaning "let the EC decide".
    pub msi_ec_auto_mode: Option<String>,
    /// msi-ec `fan_mode` value used alongside cooler boost for `max`.
    pub msi_ec_max_mode: Option<String>,
    /// Curve points for `curve` mode (empty = no curve mode offered).
    pub curve: Vec<CurvePoint>,
    /// How often the daemon re-evaluates the curve.
    pub curve_interval_secs: u64,
    /// Only consider these hwmon chips (empty = all).
    pub include_hwmon: Vec<String>,
    /// Never touch these hwmon chips.
    pub exclude_hwmon: Vec<String>,
}

impl Default for FanConfig {
    fn default() -> FanConfig {
        FanConfig {
            backend: FanBackend::Auto,
            min_pwm: default_min_pwm(),
            max_pwm: default_max_pwm(),
            boost_pwm_threshold: default_boost_threshold(),
            default_mode: None,
            msi_ec_auto_mode: None,
            msi_ec_max_mode: None,
            curve: Vec::new(),
            curve_interval_secs: default_curve_interval(),
            include_hwmon: Vec::new(),
            exclude_hwmon: Vec::new(),
        }
    }
}

// ── M6: game orchestration ───────────────────────────────────────────────────

/// How a game's cpuset is chosen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CpusetPolicy {
    /// Pin to the performance cores (falls back to all CPUs when uniform).
    PCores,
    /// Every CPU (creates the cgroup but confines nothing).
    All,
    /// No cpuset work at all.
    Off,
    /// An explicit kernel CPU list, e.g. `0-11`.
    Explicit(String),
}

/// How interrupts are handled during a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IrqPolicy {
    /// Park interrupts on the CPUs the game is not using.
    AwayFromGame,
    /// Leave interrupt affinity alone.
    Off,
}

/// A GPU clock lock: `"max"`, a single MHz value, or a `[min, max]` pair.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum ClockSpec {
    Fixed(u32),
    Range([u32; 2]),
    Keyword(String),
}

/// NVIDIA handling inside game mode.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct NvidiaConfig {
    pub enabled: bool,
    /// Set persistence mode while the session is active.
    pub persistence: bool,
    /// Graphics clock lock; omitted = do not lock.
    pub graphics_clock: Option<ClockSpec>,
    /// Memory clock lock; omitted = do not lock.
    pub memory_clock: Option<ClockSpec>,
    /// Restrict to one GPU index (default: every GPU reported).
    pub gpu_index: Option<u32>,
}

impl Default for NvidiaConfig {
    fn default() -> NvidiaConfig {
        NvidiaConfig {
            enabled: true,
            persistence: true,
            graphics_clock: None,
            memory_clock: None,
            gpu_index: None,
        }
    }
}

fn default_cgroup() -> String {
    "/sys/fs/cgroup/apex-game".to_string()
}
fn default_cpuset() -> String {
    "p-cores".to_string()
}
fn default_irq() -> String {
    "away-from-game".to_string()
}

/// Game-mode policy for a profile. As with `[fan]`, every field defaults, so
/// pre-M6 profiles keep working untouched.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct GameModeConfig {
    pub enabled: bool,
    /// Tier held for the duration of a session (default `ultra-max`).
    pub tier: Tier,
    /// Fan mode held for the duration of a session (`None` = leave as-is).
    pub fan_mode: Option<String>,
    /// `p-cores` | `all` | `off` | an explicit CPU list.
    pub cpuset: String,
    /// cgroup-v2 directory apexd creates for the session.
    pub cgroup: String,
    /// Value for `cpuset.mems`; `None` = read the root cgroup's effective mems.
    pub cpuset_mems: Option<String>,
    /// `away-from-game` | `off`.
    pub irq: String,
    /// Interrupt handler names that belong *on* the game's cores.
    pub irq_pin_to_game: Vec<String>,
    pub nvidia: NvidiaConfig,
}

impl Default for GameModeConfig {
    fn default() -> GameModeConfig {
        GameModeConfig {
            enabled: true,
            tier: Tier::UltraMax,
            fan_mode: None,
            cpuset: default_cpuset(),
            cgroup: default_cgroup(),
            cpuset_mems: None,
            irq: default_irq(),
            irq_pin_to_game: Vec::new(),
            nvidia: NvidiaConfig::default(),
        }
    }
}

impl GameModeConfig {
    /// Parse the `cpuset` string into a policy.
    pub fn cpuset_policy(&self) -> CpusetPolicy {
        match self.cpuset.trim().to_ascii_lowercase().as_str() {
            "p-cores" | "pcores" | "performance" => CpusetPolicy::PCores,
            "all" | "" => CpusetPolicy::All,
            "off" | "none" | "disabled" => CpusetPolicy::Off,
            _ => CpusetPolicy::Explicit(self.cpuset.trim().to_string()),
        }
    }

    /// Parse the `irq` string into a policy. Anything unrecognised is treated
    /// as `off` — an unreadable policy must not start moving interrupts.
    pub fn irq_policy(&self) -> IrqPolicy {
        match self.irq.trim().to_ascii_lowercase().as_str() {
            "away-from-game" | "away" | "on" => IrqPolicy::AwayFromGame,
            _ => IrqPolicy::Off,
        }
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
    /// M6 fan policy. Absent in pre-M6 profiles; defaults apply.
    #[serde(default)]
    pub fan: Option<FanConfig>,
    /// M6 game-mode policy. Absent in pre-M6 profiles; defaults apply.
    #[serde(default)]
    pub gamemode: Option<GameModeConfig>,
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

    /// The profile's fan policy, or the shipped defaults when it declares none.
    /// Pre-M6 profiles therefore get sane, safe behaviour without edits.
    pub fn fan_config(&self) -> FanConfig {
        self.fan.clone().unwrap_or_default()
    }

    /// The profile's game-mode policy, or the shipped defaults.
    pub fn game_config(&self) -> GameModeConfig {
        self.gamemode.clone().unwrap_or_default()
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
