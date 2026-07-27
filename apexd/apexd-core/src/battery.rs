//! Runtime battery discovery.
//!
//! Nothing in APEX-OS may name a battery. `BAT0` is the common case, `BAT1` and
//! `BAT2` happen (ThinkPad dual-battery, MSI, some Dells), Chromebooks and a few
//! ARM laptops use `CMB0` / `battery` / `sbs-…`, and a desktop has none at all.
//! So the rule is: enumerate `/sys/class/power_supply`, keep everything whose
//! `type` reads `Battery`, and let the *absence* of a battery be an ordinary,
//! non-error state.
//!
//! Charge-threshold control is equally ragged and is probed the same way:
//!
//! * `charge_control_start_threshold` + `charge_control_end_threshold` — the
//!   modern kernel power-supply ABI (thinkpad_acpi, system76, huawei-wmi, …).
//! * `charge_start_threshold` + `charge_stop_threshold` — the older spelling
//!   still used by a few drivers.
//! * **end only** — very common (ASUS, several Dell and LG models expose a stop
//!   threshold and no start threshold).
//! * **neither** — the majority of hardware, including every desktop and any
//!   machine whose vendor driver did not bind. That is *unsupported*, reported
//!   as such, and never an error.
//!
//! Everything here is read-only; the writes are [`Action`]s applied by a
//! [`SysWriter`](crate::syswriter::SysWriter).

use std::path::{Path, PathBuf};

use crate::tier::Action;

/// The power-supply class directory, relative to a sysfs root.
pub const POWER_SUPPLY_REL: &str = "class/power_supply";

/// Attribute-name pairs for charge thresholds, most current spelling first.
const THRESHOLD_SPELLINGS: [(&str, &str); 2] = [
    ("charge_control_start_threshold", "charge_control_end_threshold"),
    ("charge_start_threshold", "charge_stop_threshold"),
];

/// How much charge-threshold control a battery offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThresholdSupport {
    /// No threshold attribute at all — the common case.
    None,
    /// A stop/end threshold only (ASUS and friends).
    EndOnly,
    /// Both a start and a stop threshold.
    StartAndEnd,
}

impl ThresholdSupport {
    pub const fn as_str(self) -> &'static str {
        match self {
            ThresholdSupport::None => "none",
            ThresholdSupport::EndOnly => "end-only",
            ThresholdSupport::StartAndEnd => "start+end",
        }
    }

    /// True when at least one threshold can be written.
    pub const fn is_supported(self) -> bool {
        !matches!(self, ThresholdSupport::None)
    }
}

/// One discovered battery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Battery {
    /// Kernel name, e.g. `BAT0`, `BAT1`, `CMB0`, `macsmc-battery`.
    pub name: String,
    /// Absolute path to the power-supply directory.
    pub path: String,
    /// Absolute path to a writable charge *start* threshold, when one exists.
    pub start_path: Option<String>,
    /// Absolute path to a writable charge *stop* threshold, when one exists.
    pub end_path: Option<String>,
}

impl Battery {
    /// What this battery supports.
    pub fn threshold_support(&self) -> ThresholdSupport {
        match (&self.start_path, &self.end_path) {
            (Some(_), Some(_)) => ThresholdSupport::StartAndEnd,
            (None, Some(_)) => ThresholdSupport::EndOnly,
            // A start threshold with no stop threshold is not a thing any driver
            // ships, and writing one alone would be meaningless — treat it as
            // unsupported rather than half-applying.
            _ => ThresholdSupport::None,
        }
    }

    /// Read one attribute of this battery (trimmed). `None` when absent.
    pub fn read(&self, field: &str) -> Option<String> {
        read_trim(&Path::new(&self.path).join(field))
    }

    /// The write plan for this battery, or `None` when it has no thresholds.
    /// On end-only hardware the start value is simply dropped — a stop
    /// threshold on its own is still worth having.
    pub fn plan_thresholds(&self, start: u8, stop: u8) -> Option<Action> {
        if !self.threshold_support().is_supported() {
            return None;
        }
        Some(Action::ChargeThresholds {
            start,
            stop,
            start_path: self.start_path.clone(),
            end_path: self.end_path.clone(),
        })
    }
}

/// Every battery the machine has, in kernel-name order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BatteryInventory {
    pub batteries: Vec<Battery>,
}

impl BatteryInventory {
    /// Discover under a sysfs root. Read-only, never fails: a missing
    /// `power_supply` class (containers, some VMs) yields an empty inventory.
    pub fn discover(sys_root: &Path) -> BatteryInventory {
        let dir = sys_root.join(POWER_SUPPLY_REL);
        let mut batteries = Vec::new();
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return BatteryInventory { batteries };
        };
        let mut names: Vec<String> = entries
            .flatten()
            .filter_map(|e| e.file_name().into_string().ok())
            .collect();
        names.sort();
        for name in names {
            let path = dir.join(&name);
            if read_trim(&path.join("type")).as_deref() != Some("Battery") {
                continue;
            }
            let (start_path, end_path) = probe_thresholds(&path);
            batteries.push(Battery {
                name,
                path: abs(&path),
                start_path,
                end_path,
            });
        }
        BatteryInventory { batteries }
    }

    /// Discover from the live machine.
    pub fn detect() -> BatteryInventory {
        Self::discover(Path::new("/sys"))
    }

    /// True on a desktop, a VM, or anything else with no battery.
    pub fn is_empty(&self) -> bool {
        self.batteries.is_empty()
    }

    pub fn len(&self) -> usize {
        self.batteries.len()
    }

    /// The battery whose readings represent "the battery" for single-value
    /// consumers (`Battery.Capacity`, `Battery.Status`). Prefers one that
    /// actually reports a capacity, so a drained-and-removed secondary bay does
    /// not shadow the pack that is really there.
    pub fn primary(&self) -> Option<&Battery> {
        self.batteries
            .iter()
            .find(|b| b.read("capacity").is_some())
            .or_else(|| self.batteries.first())
    }

    /// Kernel names, for logs and the fingerprint.
    pub fn names(&self) -> Vec<String> {
        self.batteries.iter().map(|b| b.name.clone()).collect()
    }

    /// The best threshold support any battery offers. `None` when no battery
    /// has any — which is what `Battery.Supported` reports over D-Bus.
    pub fn threshold_support(&self) -> ThresholdSupport {
        self.batteries
            .iter()
            .map(Battery::threshold_support)
            .max_by_key(|s| match s {
                ThresholdSupport::None => 0,
                ThresholdSupport::EndOnly => 1,
                ThresholdSupport::StartAndEnd => 2,
            })
            .unwrap_or(ThresholdSupport::None)
    }

    /// True when at least one battery accepts a charge threshold.
    pub fn supports_thresholds(&self) -> bool {
        self.threshold_support().is_supported()
    }

    /// The write plan for every battery that supports thresholds. Empty on a
    /// machine that supports none — the caller decides whether that is a silent
    /// skip (start-up defaults) or a reportable "unsupported" (an explicit
    /// request over D-Bus).
    pub fn plan_thresholds(&self, start: u8, stop: u8) -> Vec<Action> {
        self.batteries
            .iter()
            .filter_map(|b| b.plan_thresholds(start, stop))
            .collect()
    }

    /// A one-line human summary for the start-up log and `apex doctor`.
    pub fn summary(&self) -> String {
        if self.batteries.is_empty() {
            return "no battery (desktop or VM)".to_string();
        }
        self.batteries
            .iter()
            .map(|b| format!("{} (thresholds: {})", b.name, b.threshold_support().as_str()))
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// Total remaining battery energy in microwatt-hours, summed over every
    /// pack. Uses `energy_now` where the driver reports energy and derives it
    /// from `charge_now` x `voltage_now` where it reports charge instead (most
    /// smart-battery and many ARM drivers). `None` when nothing is readable.
    pub fn energy_uwh(&self) -> Option<u64> {
        let mut total: u64 = 0;
        let mut any = false;
        for b in &self.batteries {
            if let Some(uwh) = b.read("energy_now").and_then(|s| s.parse::<u64>().ok()) {
                total = total.saturating_add(uwh);
                any = true;
                continue;
            }
            // charge_now is in µAh and voltage_now in µV: µAh * µV / 1e6 = µWh.
            let charge = b.read("charge_now").and_then(|s| s.parse::<u64>().ok());
            let volts = b
                .read("voltage_now")
                .or_else(|| b.read("voltage_min_design"))
                .and_then(|s| s.parse::<u64>().ok());
            if let (Some(c), Some(v)) = (charge, volts) {
                total = total.saturating_add(c.saturating_mul(v) / 1_000_000);
                any = true;
            }
        }
        any.then_some(total)
    }
}

/// Probe a power-supply directory for the two threshold spellings.
fn probe_thresholds(path: &Path) -> (Option<String>, Option<String>) {
    for (start, end) in THRESHOLD_SPELLINGS {
        let s = path.join(start);
        let e = path.join(end);
        let (s, e) = (s.exists().then(|| abs(&s)), e.exists().then(|| abs(&e)));
        if s.is_some() || e.is_some() {
            return (s, e);
        }
    }
    (None, None)
}

fn read_trim(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok().map(|s| s.trim().to_string())
}

fn abs(path: &Path) -> String {
    PathBuf::from(path).to_string_lossy().to_string()
}
