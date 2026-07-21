//! Layered profile selection: generic (chassis) -> class (CPU) -> device (DMI).
//!
//! The most specific match that exists in the [`ProfileSet`] wins as the
//! *active* profile, but every layer is reported so the D-Bus `.Profile`
//! interface can expose `Active` / `Class` / `Device` independently.

use crate::fingerprint::{CpuVendor, Fingerprint};
use crate::profile::ProfileSet;

/// The outcome of layered selection. IDs are profile IDs, or `None` when that
/// layer does not apply / is absent from the set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selection {
    /// Chassis-level fallback (always resolves to a generic profile).
    pub generic: String,
    /// CPU-class profile, if one matches and exists.
    pub class: Option<String>,
    /// Exact-device profile, if one matches and exists.
    pub device: Option<String>,
    /// The effective profile = device ?? class ?? generic.
    pub active: String,
}

impl Selection {
    /// The class ID or empty string (for the D-Bus `Class` property).
    pub fn class_or_empty(&self) -> &str {
        self.class.as_deref().unwrap_or("")
    }
    /// The device ID or empty string (for the D-Bus `Device` property).
    pub fn device_or_empty(&self) -> &str {
        self.device.as_deref().unwrap_or("")
    }
}

/// The chassis-level generic ID a fingerprint maps to.
fn generic_id(fp: &Fingerprint) -> &'static str {
    if fp.is_laptop() {
        "generic-laptop"
    } else {
        "generic-desktop"
    }
}

/// The CPU-class ID a fingerprint maps to, if any.
fn class_id(fp: &Fingerprint) -> Option<&'static str> {
    match fp.cpu.vendor {
        // AMD with the amd-pstate driver -> amd-zen. (If a future AMD box ran
        // acpi-cpufreq we still treat it as amd-zen for the EPP-less knobs;
        // the real writer skips EPP where absent.)
        CpuVendor::Amd => Some("amd-zen"),
        // Intel only gets the class profile when it is actually P/E hybrid;
        // a uniform Intel laptop falls through to the generic profile.
        CpuVendor::Intel if fp.cpu.hybrid => Some("intel-hybrid"),
        _ => None,
    }
}

/// The exact-device ID a fingerprint maps to, if any.
fn device_id(fp: &Fingerprint) -> Option<&'static str> {
    if fp.dmi_contains("Katana") {
        Some("msi-katana-gf76")
    } else if fp.dmi_contains("ThinkPad L16") {
        Some("thinkpad-l16-g2")
    } else {
        None
    }
}

/// Run layered selection against the available profiles. A layer is only
/// adopted if the corresponding profile actually exists in `set`.
pub fn select(fp: &Fingerprint, set: &ProfileSet) -> Selection {
    // Generic must always resolve; fall back the other way if the primary
    // generic profile is somehow missing from the set.
    let generic = {
        let primary = generic_id(fp);
        if set.get(primary).is_some() {
            primary.to_string()
        } else if set.get("generic-laptop").is_some() {
            "generic-laptop".to_string()
        } else {
            "generic-desktop".to_string()
        }
    };

    let class = class_id(fp)
        .filter(|id| set.get(id).is_some())
        .map(|s| s.to_string());

    let device = device_id(fp)
        .filter(|id| set.get(id).is_some())
        .map(|s| s.to_string());

    let active = device
        .clone()
        .or_else(|| class.clone())
        .unwrap_or_else(|| generic.clone());

    Selection {
        generic,
        class,
        device,
        active,
    }
}
