//! Power tiers and the hardware actions they map to.
//!
//! The five tier IDs are frozen and must match the strings the D-Bus API and
//! the apex-shell `PowerProfileService` use verbatim:
//! `ultra-max`, `ultra`, `performance`, `balanced`, `power-saver`.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// A power tier, ordered from most aggressive (`UltraMax`) to most frugal
/// (`PowerSaver`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Tier {
    #[serde(rename = "ultra-max")]
    UltraMax,
    #[serde(rename = "ultra")]
    Ultra,
    #[serde(rename = "performance")]
    Performance,
    #[serde(rename = "balanced")]
    Balanced,
    #[serde(rename = "power-saver")]
    PowerSaver,
}

impl Tier {
    /// All tiers, highest to lowest. This is the canonical order the CLI and
    /// D-Bus `Tiers` property advertise.
    pub const ALL: [Tier; 5] = [
        Tier::UltraMax,
        Tier::Ultra,
        Tier::Performance,
        Tier::Balanced,
        Tier::PowerSaver,
    ];

    /// The frozen, wire-facing string ID.
    pub const fn as_str(self) -> &'static str {
        match self {
            Tier::UltraMax => "ultra-max",
            Tier::Ultra => "ultra",
            Tier::Performance => "performance",
            Tier::Balanced => "balanced",
            Tier::PowerSaver => "power-saver",
        }
    }

    /// A human-friendly label (matches the shell's picker labels).
    pub const fn label(self) -> &'static str {
        match self {
            Tier::UltraMax => "Ultra-Max",
            Tier::Ultra => "Ultra Performance",
            Tier::Performance => "Performance",
            Tier::Balanced => "Balanced",
            Tier::PowerSaver => "Power Saver",
        }
    }

    /// The list of frozen IDs, for the D-Bus `Tiers` property.
    pub fn all_ids() -> Vec<String> {
        Tier::ALL.iter().map(|t| t.as_str().to_string()).collect()
    }
}

impl fmt::Display for Tier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Tier {
    type Err = UnknownTier;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "ultra-max" => Ok(Tier::UltraMax),
            "ultra" => Ok(Tier::Ultra),
            "performance" => Ok(Tier::Performance),
            "balanced" => Ok(Tier::Balanced),
            "power-saver" => Ok(Tier::PowerSaver),
            other => Err(UnknownTier(other.to_string())),
        }
    }
}

/// Error for an unrecognised tier ID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownTier(pub String);

impl fmt::Display for UnknownTier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "unknown tier '{}' (expected one of: {})",
            self.0,
            Tier::ALL
                .iter()
                .map(|t| t.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )
    }
}

impl std::error::Error for UnknownTier {}

/// A single intended hardware effect. Every effect a tier can request is one of
/// these; the `SysWriter` trait is the only thing that turns them into real
/// sysfs writes or process execs, which is what makes the whole engine
/// testable with a `MockWriter`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Write `scaling_governor` on every cpufreq policy.
    Governor(String),
    /// Write `energy_performance_preference` on every cpufreq policy (skipped
    /// by the real writer where the attribute is absent).
    Epp(String),
    /// Write `/sys/firmware/acpi/platform_profile` (skipped where absent).
    PlatformProfile(String),
    /// Write battery charge start/stop thresholds.
    ChargeThresholds {
        start: u8,
        stop: u8,
        start_path: String,
        end_path: String,
    },
    /// One RyzenAdj invocation (the daemon repeats this on a cadence while the
    /// active tier requests it). Milliwatts.
    RyzenAdj {
        stapm_mw: u32,
        fast_mw: u32,
        slow_mw: u32,
        tctl_max: Option<u32>,
    },
    /// Tear the RyzenAdj reapply loop down (emitted when leaving a tier that
    /// requested it).
    StopRyzenAdj,
}

impl Action {
    /// A stable, log-friendly rendering of the action (used by the dry-run
    /// planner and the CLI).
    pub fn describe(&self) -> String {
        match self {
            Action::Governor(g) => format!("scaling_governor = {g} (all policies)"),
            Action::Epp(e) => format!("energy_performance_preference = {e} (all policies)"),
            Action::PlatformProfile(p) => format!("platform_profile = {p}"),
            Action::ChargeThresholds {
                start,
                stop,
                start_path,
                end_path,
            } => format!(
                "charge thresholds start={start} ({start_path}) stop={stop} ({end_path})"
            ),
            Action::RyzenAdj {
                stapm_mw,
                fast_mw,
                slow_mw,
                tctl_max,
            } => format!(
                "ryzenadj stapm={stapm_mw}mW fast={fast_mw}mW slow={slow_mw}mW{}",
                match tctl_max {
                    Some(t) => format!(" tctl={t}C"),
                    None => String::new(),
                }
            ),
            Action::StopRyzenAdj => "stop ryzenadj reapply loop".to_string(),
        }
    }
}
