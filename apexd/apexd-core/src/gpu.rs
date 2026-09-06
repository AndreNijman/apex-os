//! GPUs: discovery across vendors, and the controls each vendor exposes.
//!
//! ## What this module was, and why it grew
//!
//! It was `nvidia-smi` and nothing else. `apex game` locked NVIDIA clocks and
//! did nothing at all for the AMD and Intel machines APEX also ships on, and
//! `docs/p3-progress.md` recorded that closing the gap "needs hardware nobody
//! here has". That is not so: the developer's ThinkPad L16 is a Radeon 780M
//! (`0x1002`, amdgpu) and the MSI Katana is an Alder Lake-P Iris Xe (`0x8086`,
//! i915) beside an RTX 3070 Mobile (`0x10de`, nvidia). All three vendors were
//! reachable, and every table in this module was read off one of them.
//!
//! ## The three vendors do not answer the same question
//!
//! | | utilisation | current clock | the safe knob |
//! |---|---|---|---|
//! | amdgpu | `gpu_busy_percent` | `pp_dpm_sclk` | `power_dpm_force_performance_level` |
//! | i915 | nothing in sysfs | `gt_cur_freq_mhz` | `gt_min_freq_mhz` / `gt_boost_freq_mhz` |
//! | nvidia | `nvidia-smi` | `nvidia-smi` | `nvidia-smi -lgc/-lmc` |
//!
//! Parity is therefore not "the same call three times". It is: every vendor
//! gets a control of the same SHAPE — clamped to limits the hardware itself
//! reports, applied through a [`SysWriter`](crate::syswriter::SysWriter), and
//! restored to the value that was there before — and every vendor that cannot
//! answer a question says so with a reason instead of a zero.
//!
//! ## NVIDIA clock locking via `nvidia-smi`
//!
//! Queries are behind the [`NvidiaSmi`] trait so the planner can be tested with
//! a mock; mutations are [`Action`]s applied by a
//! [`SysWriter`](crate::syswriter::SysWriter), which skips them entirely when
//! `nvidia-smi` is not installed. A machine with no NVIDIA GPU therefore plans
//! nothing and applies nothing — game mode still works, minus the GPU part.
//!
//! Commands used (stable across the 5xx driver series):
//!
//! | Intent | Command |
//! |---|---|
//! | persistence on/off | `nvidia-smi -i <n> -pm 1|0` |
//! | lock graphics clocks | `nvidia-smi -i <n> -lgc <min>,<max>` |
//! | lock memory clocks | `nvidia-smi -i <n> -lmc <min>,<max>` |
//! | release | `nvidia-smi -i <n> -rgc` / `-rmc` |
//! | query | `nvidia-smi --query-gpu=index,name,clocks.max.graphics,clocks.max.memory,persistence_mode --format=csv,noheader,nounits` |

use crate::profile::{ClockSpec, NvidiaConfig, SysfsGpuConfig};
use crate::tier::Action;

/// One NVIDIA GPU as `nvidia-smi` reports it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NvidiaGpu {
    pub index: u32,
    pub name: String,
    /// Maximum supported graphics clock (MHz), used to clamp a requested lock.
    pub max_graphics_mhz: Option<u32>,
    /// Maximum supported memory clock (MHz).
    pub max_memory_mhz: Option<u32>,
    /// Persistence mode at query time, so exit can restore it.
    pub persistence: Option<bool>,
}

/// Read-side access to `nvidia-smi`.
pub trait NvidiaSmi: Send + Sync {
    /// Whether `nvidia-smi` can be run at all.
    fn available(&self) -> bool;
    /// Every GPU it reports (empty when unavailable).
    fn query(&self) -> Vec<NvidiaGpu>;
    /// `(index, utilisation percent)`. NVIDIA publishes nothing in sysfs, so
    /// this is the only way a Performance Lab can answer the busy question on
    /// an NVIDIA card — and until it existed the lab said "no driver here
    /// publishes gpu_busy_percent" on a machine with an RTX 3070 in it.
    fn utilization(&self) -> Vec<(u32, f64)> {
        Vec::new()
    }

    /// Per-GPU memory as `(index, used_mib, total_mib)`.
    ///
    /// Separate from [`NvidiaSmi::query`] rather than folded into [`NvidiaGpu`]
    /// because the two have different lifetimes: the clock maxima and
    /// persistence mode `query` returns are read once when a game session is
    /// planned, while memory is a live reading the Performance Lab resamples.
    ///
    /// Defaulted to empty so an implementation that cannot report memory says
    /// so by returning nothing, rather than every caller having to guess. The
    /// NVIDIA driver exposes no sysfs VRAM total at all, which is why this
    /// exists as a querier method instead of a path in
    /// [`crate::workload::read_vram`].
    fn vram_mib(&self) -> Vec<(u32, u64, u64)> {
        Vec::new()
    }

    /// Per-GPU *current* clocks as `(index, graphics_mhz, memory_mhz)`.
    ///
    /// [`NvidiaGpu::max_graphics_mhz`] is the ceiling a lock is clamped
    /// against; this is what the GPU is doing right now, which is what the
    /// Performance Lab (§12) asks for. Defaulted to empty for the same reason
    /// as [`NvidiaSmi::vram_mib`]: amdgpu publishes its live clocks in sysfs,
    /// NVIDIA publishes none, so the honest answer from an implementation that
    /// cannot read them is nothing at all.
    fn clocks_mhz(&self) -> Vec<(u32, u64, u64)> {
        Vec::new()
    }
}

/// True when `nvidia-smi` resolves on `PATH`.
pub fn nvidia_smi_available() -> bool {
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|dir| dir.join("nvidia-smi").is_file()))
        .unwrap_or(false)
}

/// The real `nvidia-smi` querier.
#[derive(Debug, Clone, Copy, Default)]
pub struct RealNvidiaSmi;

impl NvidiaSmi for RealNvidiaSmi {
    fn available(&self) -> bool {
        nvidia_smi_available()
    }

    fn query(&self) -> Vec<NvidiaGpu> {
        if !self.available() {
            return Vec::new();
        }
        let out = std::process::Command::new("nvidia-smi")
            .args([
                "--query-gpu=index,name,clocks.max.graphics,clocks.max.memory,persistence_mode",
                "--format=csv,noheader,nounits",
            ])
            .output();
        match out {
            Ok(o) if o.status.success() => parse_query(&String::from_utf8_lossy(&o.stdout)),
            Ok(o) => {
                eprintln!(
                    "apexd: nvidia-smi query failed ({}): {}",
                    o.status,
                    String::from_utf8_lossy(&o.stderr).trim()
                );
                Vec::new()
            }
            Err(e) => {
                eprintln!("apexd: nvidia-smi query could not run: {e}");
                Vec::new()
            }
        }
    }

    fn vram_mib(&self) -> Vec<(u32, u64, u64)> {
        if !self.available() {
            return Vec::new();
        }
        let out = std::process::Command::new("nvidia-smi")
            .args([
                "--query-gpu=index,memory.used,memory.total",
                "--format=csv,noheader,nounits",
            ])
            .output();
        match out {
            Ok(o) if o.status.success() => parse_indexed_pair(&String::from_utf8_lossy(&o.stdout)),
            // Read-only and non-critical: a driver that refuses the query must
            // leave the Performance Lab reporting "unavailable", not fail.
            _ => Vec::new(),
        }
    }

    fn clocks_mhz(&self) -> Vec<(u32, u64, u64)> {
        if !self.available() {
            return Vec::new();
        }
        let out = std::process::Command::new("nvidia-smi")
            .args([
                "--query-gpu=index,clocks.current.graphics,clocks.current.memory",
                "--format=csv,noheader,nounits",
            ])
            .output();
        match out {
            Ok(o) if o.status.success() => parse_indexed_pair(&String::from_utf8_lossy(&o.stdout)),
            _ => Vec::new(),
        }
    }

    fn utilization(&self) -> Vec<(u32, f64)> {
        if !self.available() {
            return Vec::new();
        }
        let out = std::process::Command::new("nvidia-smi")
            .args([
                "--query-gpu=index,utilization.gpu",
                "--format=csv,noheader,nounits",
            ])
            .output();
        match out {
            Ok(o) if o.status.success() => parse_utilization(&String::from_utf8_lossy(&o.stdout)),
            _ => Vec::new(),
        }
    }
}

/// Parse an `index,percent` CSV. `[N/A]` — which a laptop GPU in its lowest
/// power state does report — is DROPPED rather than read as 0%: the card is not
/// idle, it is not answering, and those are different sentences.
pub fn parse_utilization(text: &str) -> Vec<(u32, f64)> {
    let mut out = Vec::new();
    for line in text.lines() {
        let f: Vec<&str> = line.trim().split(',').map(|s| s.trim()).collect();
        if f.len() < 2 {
            continue;
        }
        if let (Ok(i), Ok(pct)) = (f[0].parse::<u32>(), f[1].parse::<f64>()) {
            out.push((i, pct));
        }
    }
    out
}

/// Parse a three-column `index,a,b` CSV from `nvidia-smi --format=csv`.
///
/// Shared by the memory and clock queries because both have exactly that shape.
/// A row missing either figure is DROPPED rather than defaulted to zero: "0 MiB
/// used" and "we could not read it" mean completely different things to someone
/// sizing a model against the free VRAM.
pub fn parse_indexed_pair(text: &str) -> Vec<(u32, u64, u64)> {
    let mut out = Vec::new();
    for line in text.lines() {
        let f: Vec<&str> = line.trim().split(',').map(|s| s.trim()).collect();
        if f.len() < 3 {
            continue;
        }
        if let (Ok(i), Ok(used), Ok(total)) = (
            f[0].parse::<u32>(),
            f[1].parse::<u64>(),
            f[2].parse::<u64>(),
        ) {
            out.push((i, used, total));
        }
    }
    out
}

/// A canned querier for tests. Construct it with `..Default::default()` so a
/// future field does not break every call site.
#[derive(Debug, Clone, Default)]
pub struct MockNvidiaSmi {
    pub available: bool,
    pub gpus: Vec<NvidiaGpu>,
    /// `(index, used_mib, total_mib)`, as `vram_mib` should return it.
    pub vram: Vec<(u32, u64, u64)>,
    /// `(index, graphics_mhz, memory_mhz)`, as `clocks_mhz` should return it.
    pub clocks: Vec<(u32, u64, u64)>,
    /// `(index, percent)`, as `utilization` should return it.
    pub util: Vec<(u32, f64)>,
}

impl NvidiaSmi for MockNvidiaSmi {
    fn available(&self) -> bool {
        self.available
    }
    fn query(&self) -> Vec<NvidiaGpu> {
        if self.available {
            self.gpus.clone()
        } else {
            Vec::new()
        }
    }
    fn vram_mib(&self) -> Vec<(u32, u64, u64)> {
        if self.available {
            self.vram.clone()
        } else {
            Vec::new()
        }
    }
    fn clocks_mhz(&self) -> Vec<(u32, u64, u64)> {
        if self.available {
            self.clocks.clone()
        } else {
            Vec::new()
        }
    }
    fn utilization(&self) -> Vec<(u32, f64)> {
        if self.available {
            self.util.clone()
        } else {
            Vec::new()
        }
    }
}

/// Parse `--format=csv,noheader,nounits` output. Unparseable fields become
/// `None` rather than killing the row (`[N/A]` is common on laptop GPUs).
pub fn parse_query(text: &str) -> Vec<NvidiaGpu> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let f: Vec<&str> = line.split(',').map(|s| s.trim()).collect();
        if f.is_empty() {
            continue;
        }
        let Ok(index) = f[0].parse::<u32>() else {
            continue;
        };
        out.push(NvidiaGpu {
            index,
            name: f.get(1).copied().unwrap_or_default().to_string(),
            max_graphics_mhz: f.get(2).and_then(|v| v.parse::<u32>().ok()),
            max_memory_mhz: f.get(3).and_then(|v| v.parse::<u32>().ok()),
            persistence: f.get(4).and_then(|v| match v.to_ascii_lowercase().as_str() {
                "enabled" => Some(true),
                "disabled" => Some(false),
                _ => None,
            }),
        });
    }
    out
}

/// The clock-lock plan for one GPU. Empty when the profile disables NVIDIA
/// handling or when the GPU reports no maximum clock to clamp against — we
/// never pass an unvalidated MHz value to the driver.
pub fn plan_lock(cfg: &NvidiaConfig, gpu: &NvidiaGpu) -> Vec<Action> {
    let mut actions = Vec::new();
    if !cfg.enabled {
        return actions;
    }
    if cfg.persistence {
        actions.push(Action::NvidiaPersistence {
            gpu: gpu.index,
            enabled: true,
        });
    }
    if let (Some(spec), Some(max)) = (&cfg.graphics_clock, gpu.max_graphics_mhz) {
        if let Some((min_mhz, max_mhz)) = spec.resolve(max) {
            actions.push(Action::NvidiaLockGraphics {
                gpu: gpu.index,
                min_mhz,
                max_mhz,
            });
        }
    }
    if let (Some(spec), Some(max)) = (&cfg.memory_clock, gpu.max_memory_mhz) {
        if let Some((min_mhz, max_mhz)) = spec.resolve(max) {
            actions.push(Action::NvidiaLockMemory {
                gpu: gpu.index,
                min_mhz,
                max_mhz,
            });
        }
    }
    actions
}

/// The exact inverse of [`plan_lock`]: release whatever was locked and put
/// persistence back where it was found.
pub fn plan_unlock(cfg: &NvidiaConfig, gpu: &NvidiaGpu) -> Vec<Action> {
    let mut actions = Vec::new();
    if !cfg.enabled {
        return actions;
    }
    if let (Some(spec), Some(max)) = (&cfg.graphics_clock, gpu.max_graphics_mhz) {
        if spec.resolve(max).is_some() {
            actions.push(Action::NvidiaResetGraphics { gpu: gpu.index });
        }
    }
    if let (Some(spec), Some(max)) = (&cfg.memory_clock, gpu.max_memory_mhz) {
        if spec.resolve(max).is_some() {
            actions.push(Action::NvidiaResetMemory { gpu: gpu.index });
        }
    }
    if cfg.persistence {
        // Only touch persistence if we know what it was; leaving it enabled is
        // harmless but "restore exactly" means restoring exactly.
        if let Some(prior) = gpu.persistence {
            if !prior {
                actions.push(Action::NvidiaPersistence {
                    gpu: gpu.index,
                    enabled: false,
                });
            }
        }
    }
    actions
}

impl ClockSpec {
    /// Resolve a profile clock spec against the GPU's maximum supported clock,
    /// clamping so an over-ambitious profile can never ask for an unsupported
    /// frequency. `None` means "do not lock".
    pub fn resolve(&self, max_supported: u32) -> Option<(u32, u32)> {
        if max_supported == 0 {
            return None;
        }
        match self {
            ClockSpec::Keyword(k) => match k.to_ascii_lowercase().as_str() {
                "max" => Some((max_supported, max_supported)),
                _ => None, // "off"/"none"/anything unrecognised: do not lock.
            },
            ClockSpec::Fixed(v) => {
                let v = (*v).min(max_supported);
                Some((v, v))
            }
            ClockSpec::Range([a, b]) => {
                let lo = (*a).min(max_supported);
                let hi = (*b).min(max_supported);
                Some((lo.min(hi), lo.max(hi)))
            }
        }
    }
}

// ─── Vendor-neutral discovery and controls ───────────────────────────────────

use std::path::{Path, PathBuf};

/// Who made the card. `Other` keeps the raw PCI vendor id rather than
/// collapsing an unknown card into one of the three known ones, because a
/// wrong vendor picks the wrong sysfs attribute and reports its absence as a
/// property of the machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GpuVendor {
    Amd,
    Intel,
    Nvidia,
    Other(String),
}

impl GpuVendor {
    /// From the PCI vendor id in `/sys/class/drm/cardN/device/vendor`.
    pub fn from_pci_id(id: &str) -> GpuVendor {
        match id.trim().to_ascii_lowercase().as_str() {
            "0x1002" | "0x1022" => GpuVendor::Amd,
            "0x8086" => GpuVendor::Intel,
            "0x10de" => GpuVendor::Nvidia,
            other => GpuVendor::Other(other.to_string()),
        }
    }

    pub fn label(&self) -> &str {
        match self {
            GpuVendor::Amd => "AMD",
            GpuVendor::Intel => "Intel",
            GpuVendor::Nvidia => "NVIDIA",
            GpuVendor::Other(id) => id,
        }
    }
}

/// One GPU, with the paths that exist for it resolved once.
///
/// A path is `Some` only when the file is actually there. That is the whole
/// point: "amdgpu publishes `gpu_busy_percent`" is a statement about a driver
/// version and a card, not about a vendor, and a reader that assumes it will
/// report a missing file as 0% busy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GpuDevice {
    /// `card1`, as it appears under `/sys/class/drm`.
    pub card: String,
    pub vendor: GpuVendor,
    /// The kernel driver bound to it: `amdgpu`, `i915`, `xe`, `nvidia`.
    pub driver: Option<String>,
    /// `boot_vga` — the firmware's display adapter.
    pub boot_vga: bool,
    /// `device/gpu_busy_percent`, amdgpu only in practice.
    pub busy_path: Option<PathBuf>,
    /// Whichever of `device/pp_dpm_sclk` or `gt_cur_freq_mhz` exists.
    pub clock_path: Option<PathBuf>,
    /// amdgpu: `device/power_dpm_force_performance_level`.
    pub amd_perf_level: Option<PathBuf>,
    /// i915: `gt_min_freq_mhz`, with the limits beside it.
    pub intel_min_freq: Option<PathBuf>,
    pub intel_boost_freq: Option<PathBuf>,
    /// i915: `gt_RPn_freq_mhz` and `gt_RP0_freq_mhz` — the floor and ceiling
    /// the hardware itself reports. A write outside them is refused by the
    /// driver, so they are what a request is clamped to.
    pub intel_limits: Option<(u32, u32)>,
}

impl GpuDevice {
    /// True when this card has a knob APEX can turn without nvidia-smi.
    pub fn has_sysfs_control(&self) -> bool {
        self.amd_perf_level.is_some() || self.intel_min_freq.is_some()
    }
}

fn read_trim(p: &Path) -> Option<String> {
    std::fs::read_to_string(p).ok().map(|s| s.trim().to_string())
}

fn some_if_file(p: PathBuf) -> Option<PathBuf> {
    if p.is_file() {
        Some(p)
    } else {
        None
    }
}

/// Every real GPU under `<sys>/class/drm`, in card order.
///
/// Connector nodes (`card0-eDP-1`) are skipped, and so is anything whose
/// `device/vendor` cannot be read — a DRM node with no PCI device behind it is
/// not a GPU this code can control.
pub fn discover(sys: &Path) -> Vec<GpuDevice> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(sys.join("class/drm")) else {
        return out;
    };
    let mut cards: Vec<(String, PathBuf)> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            if !name.starts_with("card") || name.contains('-') {
                return None;
            }
            if !name["card".len()..].chars().all(|c| c.is_ascii_digit()) {
                return None;
            }
            Some((name, e.path()))
        })
        .collect();
    cards.sort();

    for (card, path) in cards {
        let dev = path.join("device");
        let Some(vendor_id) = read_trim(&dev.join("vendor")) else {
            continue;
        };
        let vendor = GpuVendor::from_pci_id(&vendor_id);
        let driver = std::fs::read_link(dev.join("driver"))
            .ok()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()));

        let intel_min_freq = some_if_file(path.join("gt_min_freq_mhz"));
        let intel_limits = if intel_min_freq.is_some() {
            match (
                read_trim(&path.join("gt_RPn_freq_mhz")).and_then(|s| s.parse::<u32>().ok()),
                read_trim(&path.join("gt_RP0_freq_mhz")).and_then(|s| s.parse::<u32>().ok()),
            ) {
                (Some(lo), Some(hi)) if hi >= lo => Some((lo, hi)),
                _ => None,
            }
        } else {
            None
        };

        out.push(GpuDevice {
            card: card.clone(),
            vendor,
            driver,
            boot_vga: read_trim(&dev.join("boot_vga")).as_deref() == Some("1"),
            busy_path: some_if_file(dev.join("gpu_busy_percent")),
            clock_path: some_if_file(dev.join("pp_dpm_sclk"))
                .or_else(|| some_if_file(path.join("gt_cur_freq_mhz"))),
            amd_perf_level: some_if_file(dev.join("power_dpm_force_performance_level")),
            intel_min_freq,
            intel_boost_freq: some_if_file(path.join("gt_boost_freq_mhz")),
            intel_limits,
        });
    }
    out
}

/// The card a Performance Lab should lead with on a hybrid machine.
///
/// NOT card order, which is what the lab used before: on the MSI Katana
/// `card1` is the Intel iGPU and `card2` is the RTX 3070, so the lab reported
/// the iGPU's clock, labelled it "GPU", and never mentioned the card the games
/// run on. A discrete GPU outranks an integrated one; among equals, the one
/// that can actually answer a question outranks one that cannot.
pub fn primary(devices: &[GpuDevice]) -> Option<&GpuDevice> {
    devices
        .iter()
        .max_by_key(|d| {
            let discrete = !d.boot_vga;
            let answers = d.busy_path.is_some() || d.vendor == GpuVendor::Nvidia;
            let clocks = d.clock_path.is_some() || d.vendor == GpuVendor::Nvidia;
            (discrete as u8, answers as u8, clocks as u8)
        })
}

// ─── The safe knob, per vendor ───────────────────────────────────────────────

/// The values amdgpu accepts. A profile naming anything else is refused rather
/// than written: the driver returns `-EINVAL` and the write is silently a
/// no-op, which would report as "applied".
pub const AMD_PERF_LEVELS: &[&str] = &[
    "auto",
    "low",
    "high",
    "manual",
    "profile_standard",
    "profile_min_sclk",
    "profile_min_mclk",
    "profile_peak",
];

/// Clamp a percentage of the reported range into an absolute MHz floor.
pub fn intel_floor_mhz(limits: (u32, u32), percent: u8) -> u32 {
    let (lo, hi) = limits;
    let pct = percent.min(100) as u64;
    let span = hi.saturating_sub(lo) as u64;
    lo + ((span * pct) / 100) as u32
}

/// What the sysfs GPU controls were set to before a game session, so leaving
/// puts them back rather than guessing at a default.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SysfsGpuPrior {
    pub amd_perf_level: Option<(String, String)>,
    pub intel_min_freq: Option<(String, u32)>,
    pub intel_boost_freq: Option<(String, u32)>,
}

/// Read what is there now, so it can be put back.
pub fn read_prior(dev: &GpuDevice) -> SysfsGpuPrior {
    SysfsGpuPrior {
        amd_perf_level: dev
            .amd_perf_level
            .as_ref()
            .and_then(|p| read_trim(p).map(|v| (p.display().to_string(), v))),
        intel_min_freq: dev.intel_min_freq.as_ref().and_then(|p| {
            read_trim(p)
                .and_then(|v| v.parse::<u32>().ok())
                .map(|v| (p.display().to_string(), v))
        }),
        intel_boost_freq: dev.intel_boost_freq.as_ref().and_then(|p| {
            read_trim(p)
                .and_then(|v| v.parse::<u32>().ok())
                .map(|v| (p.display().to_string(), v))
        }),
    }
}

/// The actions that put a GPU into game mode, and a note when it cannot.
///
/// Returns `(actions, notes)`. A note is not a failure: "this Intel GPU has no
/// published frequency limits, so the floor was skipped" is the honest report,
/// and it is exactly what the NVIDIA path already does when nvidia-smi is
/// absent.
pub fn plan_sysfs_enter(
    cfg: &SysfsGpuConfig,
    dev: &GpuDevice,
    prior: &SysfsGpuPrior,
) -> (Vec<Action>, Vec<String>) {
    let mut actions = Vec::new();
    let mut notes = Vec::new();
    if !cfg.enabled {
        return (actions, notes);
    }

    if let Some(path) = &dev.amd_perf_level {
        let want = cfg.amd_perf_level.trim();
        if want.is_empty() || want == "auto" {
            // Not a note: "auto" is the shipped value and asking for it means
            // leaving the card alone, which is a choice rather than a gap.
        } else if !AMD_PERF_LEVELS.contains(&want) {
            notes.push(format!(
                "{}: power_dpm_force_performance_level {want:?} is not one of {}                  — the driver would refuse the write, so it was not attempted",
                dev.card,
                AMD_PERF_LEVELS.join(", ")
            ));
        } else if prior
            .amd_perf_level
            .as_ref()
            .is_some_and(|(_, v)| v == want)
        {
            notes.push(format!(
                "{}: power_dpm_force_performance_level is already {want}",
                dev.card
            ));
        } else {
            actions.push(Action::GpuSysfsAttr {
                path: path.display().to_string(),
                value: want.to_string(),
                what: format!("{} DPM level", dev.card),
            });
        }
    }

    if let Some(path) = &dev.intel_min_freq {
        if cfg.intel_floor_percent == 0 {
            // Same reading as amdgpu's "auto".
        } else if let Some(limits) = dev.intel_limits {
            let want = intel_floor_mhz(limits, cfg.intel_floor_percent);
            let now = prior.intel_min_freq.as_ref().map(|(_, v)| *v);
            if now == Some(want) {
                notes.push(format!("{}: gt_min_freq_mhz is already {want}", dev.card));
            } else {
                actions.push(Action::GpuSysfsAttr {
                    path: path.display().to_string(),
                    value: want.to_string(),
                    what: format!("{} GPU frequency floor (MHz)", dev.card),
                });
            }
        } else {
            notes.push(format!(
                "{}: this GPU does not publish gt_RPn_freq_mhz and gt_RP0_freq_mhz,                  so there is nothing to clamp a floor against and none was set",
                dev.card
            ));
        }
    }

    (actions, notes)
}

/// The actions that undo [`plan_sysfs_enter`], from what was recorded on the
/// way in. Nothing is defaulted: a control whose prior value was never read is
/// left alone, because writing a guess is worse than leaving a card fast.
pub fn plan_sysfs_exit(prior: &SysfsGpuPrior) -> Vec<Action> {
    let mut actions = Vec::new();
    if let Some((path, value)) = &prior.amd_perf_level {
        actions.push(Action::GpuSysfsAttr {
            path: path.clone(),
            value: value.clone(),
            what: "restore DPM level".to_string(),
        });
    }
    if let Some((path, value)) = &prior.intel_min_freq {
        actions.push(Action::GpuSysfsAttr {
            path: path.clone(),
            value: value.to_string(),
            what: "restore GPU frequency floor (MHz)".to_string(),
        });
    }
    actions
}
