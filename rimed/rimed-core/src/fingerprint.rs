//! Read-only hardware fingerprinting.
//!
//! Everything here reads `/proc` and `/sys` only; nothing is ever written. The
//! sysfs/procfs roots are parameterised so detection can be pointed at
//! fixtures, but selection logic (see [`crate::select`]) is tested against
//! hand-built [`Fingerprint`] values and never touches the filesystem.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// CPU vendor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CpuVendor {
    Amd,
    Intel,
    Other(String),
}

impl CpuVendor {
    pub fn as_str(&self) -> &str {
        match self {
            CpuVendor::Amd => "AMD",
            CpuVendor::Intel => "Intel",
            CpuVendor::Other(s) => s,
        }
    }
}

/// GPU vendor, keyed off the PCI vendor ID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GpuVendor {
    Amd,
    Intel,
    Nvidia,
    Other(u16),
}

impl GpuVendor {
    pub fn from_pci(vendor: u16) -> GpuVendor {
        match vendor {
            0x1002 => GpuVendor::Amd,
            0x8086 => GpuVendor::Intel,
            0x10de => GpuVendor::Nvidia,
            other => GpuVendor::Other(other),
        }
    }

    pub fn as_str(&self) -> String {
        match self {
            GpuVendor::Amd => "AMD".to_string(),
            GpuVendor::Intel => "Intel".to_string(),
            GpuVendor::Nvidia => "NVIDIA".to_string(),
            GpuVendor::Other(v) => format!("0x{v:04x}"),
        }
    }
}

/// A display-class PCI device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GpuInfo {
    pub vendor: GpuVendor,
    pub pci_vendor: u16,
    pub pci_device: u16,
    pub pci_slot: String,
}

impl GpuInfo {
    pub fn pci_id(&self) -> String {
        format!("{:04x}:{:04x}", self.pci_vendor, self.pci_device)
    }
}

/// CPU topology and scaling facts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CpuInfo {
    pub vendor: CpuVendor,
    pub model_name: String,
    pub physical_cores: usize,
    pub logical_threads: usize,
    pub scaling_driver: Option<String>,
    /// Intel P-core/E-core hybrid topology present.
    pub hybrid: bool,
}

impl CpuInfo {
    pub fn amd_pstate(&self) -> bool {
        self.scaling_driver
            .as_deref()
            .map(|d| d.contains("amd-pstate") || d.contains("amd_pstate"))
            .unwrap_or(false)
    }
    pub fn intel_pstate(&self) -> bool {
        self.scaling_driver
            .as_deref()
            .map(|d| d.contains("intel_pstate") || d.contains("intel-pstate"))
            .unwrap_or(false)
    }
}

/// The complete machine fingerprint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fingerprint {
    pub cpu: CpuInfo,
    pub gpus: Vec<GpuInfo>,
    /// SMBIOS chassis type (3=desktop, 9/10=laptop/notebook, ...).
    pub chassis_type: u32,
    pub sys_vendor: String,
    pub product_name: String,
    pub product_family: String,
    pub product_version: String,
    pub has_ac: bool,
    pub batteries: Vec<String>,
}

impl Fingerprint {
    /// True for portable chassis types (laptop/notebook/handheld/sub-notebook).
    pub fn is_laptop(&self) -> bool {
        matches!(self.chassis_type, 8 | 9 | 10 | 11 | 14 | 30 | 31 | 32)
    }

    /// True when an Intel iGPU and an NVIDIA dGPU are both present
    /// (Optimus/PRIME hybrid graphics).
    pub fn intel_nvidia_hybrid_gpu(&self) -> bool {
        let intel = self.gpus.iter().any(|g| g.vendor == GpuVendor::Intel);
        let nvidia = self.gpus.iter().any(|g| g.vendor == GpuVendor::Nvidia);
        intel && nvidia
    }

    /// Case-insensitive search across the three DMI product strings.
    pub fn dmi_contains(&self, needle: &str) -> bool {
        let n = needle.to_ascii_lowercase();
        [&self.product_name, &self.product_family, &self.product_version]
            .iter()
            .any(|s| s.to_ascii_lowercase().contains(&n))
    }

    /// Detect from the live machine (`/proc`, `/sys`). Read-only.
    pub fn detect() -> Fingerprint {
        Self::detect_from(Path::new("/proc"), Path::new("/sys"))
    }

    /// Detect using explicit procfs/sysfs roots (for fixtures/tests).
    pub fn detect_from(proc_root: &Path, sys_root: &Path) -> Fingerprint {
        let cpu = detect_cpu(proc_root, sys_root);
        let gpus = detect_gpus(sys_root);
        let dmi = |f: &str| read_dmi(sys_root, f);
        let chassis_type = dmi("chassis_type")
            .and_then(|s| s.trim().parse::<u32>().ok())
            .unwrap_or(0);
        let (has_ac, batteries) = detect_power_supply(sys_root);
        Fingerprint {
            cpu,
            gpus,
            chassis_type,
            sys_vendor: dmi("sys_vendor").unwrap_or_default(),
            product_name: dmi("product_name").unwrap_or_default(),
            product_family: dmi("product_family").unwrap_or_default(),
            product_version: dmi("product_version").unwrap_or_default(),
            has_ac,
            batteries,
        }
    }
}

fn read_trim(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok().map(|s| s.trim().to_string())
}

fn read_dmi(sys_root: &Path, field: &str) -> Option<String> {
    read_trim(&sys_root.join("class/dmi/id").join(field))
}

fn detect_cpu(proc_root: &Path, sys_root: &Path) -> CpuInfo {
    let cpuinfo = std::fs::read_to_string(proc_root.join("cpuinfo")).unwrap_or_default();

    let mut vendor = CpuVendor::Other("unknown".to_string());
    let mut model_name = String::new();
    let mut logical_threads = 0usize;
    // (physical id, core id) pairs -> physical core count.
    let mut core_ids: BTreeSet<(String, String)> = BTreeSet::new();
    let mut cur_phys = String::new();
    let mut cur_core = String::new();

    for line in cpuinfo.lines() {
        if let Some((k, v)) = line.split_once(':') {
            let key = k.trim();
            let val = v.trim();
            match key {
                "processor" => {
                    logical_threads += 1;
                    cur_phys.clear();
                    cur_core.clear();
                }
                "vendor_id" if matches!(vendor, CpuVendor::Other(_)) => {
                    vendor = match val {
                        "AuthenticAMD" => CpuVendor::Amd,
                        "GenuineIntel" => CpuVendor::Intel,
                        other => CpuVendor::Other(other.to_string()),
                    };
                }
                "model name" if model_name.is_empty() => model_name = val.to_string(),
                "physical id" => cur_phys = val.to_string(),
                "core id" => {
                    cur_core = val.to_string();
                    core_ids.insert((cur_phys.clone(), cur_core.clone()));
                }
                _ => {}
            }
        }
    }

    let physical_cores = if core_ids.is_empty() {
        logical_threads
    } else {
        core_ids.len()
    };

    let scaling_driver = read_trim(
        &sys_root.join("devices/system/cpu/cpu0/cpufreq/scaling_driver"),
    );

    // Intel hybrid (Alder Lake+) exposes distinct core/atom PMUs.
    let hybrid = vendor == CpuVendor::Intel
        && sys_root.join("devices/cpu_core").exists()
        && sys_root.join("devices/cpu_atom").exists();

    CpuInfo {
        vendor,
        model_name,
        physical_cores,
        logical_threads,
        scaling_driver,
        hybrid,
    }
}

fn detect_gpus(sys_root: &Path) -> Vec<GpuInfo> {
    let mut gpus = Vec::new();
    let pci = sys_root.join("bus/pci/devices");
    let Ok(entries) = std::fs::read_dir(&pci) else {
        return gpus;
    };
    let mut dirs: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    dirs.sort();
    for dir in dirs {
        let class = read_trim(&dir.join("class")).unwrap_or_default();
        // Class 0x03xxxx == display controller.
        if !class.starts_with("0x03") {
            continue;
        }
        let vendor = parse_hex16(&read_trim(&dir.join("vendor")).unwrap_or_default());
        let device = parse_hex16(&read_trim(&dir.join("device")).unwrap_or_default());
        let (Some(vendor), Some(device)) = (vendor, device) else {
            continue;
        };
        gpus.push(GpuInfo {
            vendor: GpuVendor::from_pci(vendor),
            pci_vendor: vendor,
            pci_device: device,
            pci_slot: dir
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string(),
        });
    }
    gpus
}

fn parse_hex16(s: &str) -> Option<u16> {
    let s = s.trim().strip_prefix("0x").unwrap_or(s.trim());
    u16::from_str_radix(s, 16).ok()
}

fn detect_power_supply(sys_root: &Path) -> (bool, Vec<String>) {
    let dir = sys_root.join("class/power_supply");
    let mut has_ac = false;
    let mut batteries = Vec::new();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return (false, batteries);
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    names.sort();
    for name in names {
        let ty = read_trim(&dir.join(&name).join("type")).unwrap_or_default();
        match ty.as_str() {
            "Mains" | "USB" => {
                // Only count a real AC/adapter line, not the USBC PD source PSYs.
                if name.starts_with("AC") || name.starts_with("ADP") || ty == "Mains" {
                    has_ac = true;
                }
            }
            "Battery" => batteries.push(name),
            _ => {}
        }
    }
    (has_ac, batteries)
}
