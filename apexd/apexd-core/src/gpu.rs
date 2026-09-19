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
///
/// This answers "is the tool installed", and that is ALL it answers. It is not
/// the same question as "is there a usable NVIDIA GPU here" — see
/// [`SmiOutcome`] and [`classify_smi`] for why the two came apart on real
/// hardware, and prefer them for anything the user reads.
pub fn nvidia_smi_available() -> bool {
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|dir| dir.join("nvidia-smi").is_file()))
        .unwrap_or(false)
}

/// `nvidia-smi`'s own exit code for "there is no driver to talk to".
///
/// Documented by the tool as *"NVIDIA_SMI has failed because it couldn't
/// communicate with the NVIDIA driver"*.
const SMI_NO_DRIVER: i32 = 9;

/// What running `nvidia-smi` actually told us.
///
/// ## Why this type exists
///
/// `nvidia_smi_available()` asks whether the binary is on `PATH`, and for a
/// while that stood in for "this machine has an NVIDIA GPU". Measured on the
/// ThinkPad L16 on 2026-09-08, those are different facts: `/usr/bin/nvidia-smi`
/// is installed and **exits 9** because no driver is loaded. There is no NVIDIA
/// card in the machine at all.
///
/// That is not an unusual configuration and it is not a broken one. The same
/// state occurs on any machine where the tool ships and the module does not
/// load: a laptop with its discrete card disabled in firmware, a machine whose
/// kernel was updated before the module was rebuilt, and — as here — a single
/// image that carries the tool for the machines that do have a card.
///
/// Treating it as an error had two visible costs, both fixed with this type:
///
///  * every `apex ai status`, on every such machine, printed
///    `apexd: nvidia-smi query failed (exit status: 9):` to stderr. The JSON on
///    stdout was correct and parseable — that part was never broken — but a
///    caller that merges the streams, which is the common shape for `2>&1` in a
///    script, got prose in front of its JSON.
///  * `apex game` reported `nvidia-smi: present`, which is true and useless to
///    somebody working out why their clock locks do nothing.
///
/// This is the same class of defect this codebase has found repeatedly, wearing
/// a different hat: an absent capability inferred from the wrong signal. The
/// earlier instances read a failed `stat` as "the file is not there"; this one
/// reads an installed binary as "the hardware is there".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SmiOutcome {
    /// The tool is not installed. Nothing to say about NVIDIA.
    Absent,
    /// The tool ran and said it cannot reach a driver. No usable NVIDIA GPU
    /// right now — an ordinary state, and NOT worth a diagnostic.
    NoDriver,
    /// The tool ran and answered.
    Ready,
    /// The tool ran and failed for some other reason. Worth reporting, because
    /// unlike the case above nobody has established that this is expected.
    Failed,
}

impl SmiOutcome {
    /// What to show a user who is asking why NVIDIA features are inactive.
    pub fn as_str(self) -> &'static str {
        match self {
            SmiOutcome::Absent => "absent",
            SmiOutcome::NoDriver => "present, but no driver is loaded",
            SmiOutcome::Ready => "present",
            SmiOutcome::Failed => "present, but the query failed",
        }
    }

    /// Whether this outcome deserves a line on stderr.
    ///
    /// [`SmiOutcome::NoDriver`] deliberately does not: it is the steady state
    /// of every machine in this fleet that has the tool and no card, and a
    /// diagnostic printed on every invocation is noise that trains people to
    /// ignore diagnostics.
    pub fn is_worth_reporting(self) -> bool {
        matches!(self, SmiOutcome::Failed)
    }
}

/// Classify a finished `nvidia-smi` run from its exit code.
///
/// Pure on purpose, and `pub` for the same reason [`parse_query`] is: the
/// spawning wrapper below cannot be unit-tested without a real `nvidia-smi` on
/// `PATH`, and mutating `PATH` inside a test process races every other test in
/// the binary. So the decision lives here where it can be checked exhaustively,
/// and the wrapper stays a thin shell over it.
///
/// `installed` is passed rather than probed so that a caller which has already
/// answered that question does not answer it twice — and so that this function
/// has no way to read the machine it is running on.
pub fn classify_smi(installed: bool, code: Option<i32>) -> SmiOutcome {
    if !installed {
        return SmiOutcome::Absent;
    }
    match code {
        Some(0) => SmiOutcome::Ready,
        Some(SMI_NO_DRIVER) => SmiOutcome::NoDriver,
        // A signal gives no code. Unknown rather than expected, so it is
        // reportable — a killed nvidia-smi is not the same claim as a machine
        // without a driver.
        _ => SmiOutcome::Failed,
    }
}

/// Run `nvidia-smi` cheaply and say what this machine can do with it.
///
/// `-L` lists the cards and nothing else, so this is the least work that still
/// distinguishes "the driver answers" from "the driver is not there".
pub fn nvidia_smi_state() -> SmiOutcome {
    if !nvidia_smi_available() {
        return SmiOutcome::Absent;
    }
    match std::process::Command::new("nvidia-smi").arg("-L").output() {
        Ok(o) => classify_smi(true, o.status.code()),
        // Installed but unable to execute at all. Not "no driver".
        Err(_) => SmiOutcome::Failed,
    }
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
            // A driver that is not there is an ANSWER, not a fault, and the two
            // sibling methods below already treated it that way. This arm makes
            // query() agree with them: no NVIDIA GPU is usable, so report no
            // GPUs, quietly. Printing here put a line on stderr for every
            // `apex ai status` on every machine that ships the tool without a
            // card — see SmiOutcome for the measurement.
            Ok(o) if !classify_smi(true, o.status.code()).is_worth_reporting() => Vec::new(),
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
                "{}: this GPU does not publish gt_RPn_freq_mhz and gt_RP0_freq_mhz, \
                 so there is nothing to clamp a floor against and none was set",
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

// ─── Which GPU and which screen a full-screen session should use ─────────────
//
// ## The defect this exists for
//
// Measured on katana, 2026-09-19 (`ROADMAP/evidence/katana-qualification-20260919.md`
// §6.1): `apex-gaming-session` passed gamescope no device preference, gamescope
// took its default — the first DRM node — and on a hybrid laptop that is the
// Intel iGPU. It opened `card1`, saw only `eDP-1`, and the RTX 3070 driving the
// user's only external monitor through `card2` was never touched. Gaming Mode
// came up on the laptop panel at 144 Hz instead of the monitor at 240 Hz, and
// then died on `drmModeAddFB2WithModifiers`.
//
// Also measured there, and the reason this is not solved with an environment
// variable: **gamescope ignores `WLR_DRM_DEVICES`.** Its DRM backend is its own,
// not wlroots', so the variable that steers labwc steers nothing here. The flag
// that moves both the Vulkan device AND the DRM node is `--prefer-vk-device`,
// and `10de:249d` was measured to move gamescope onto `card2` and `HDMI-A-1`.
//
// ## The rule, and why it is this one
//
// **The output decides the card, not the other way round.**
//
// The tempting rule is "use the discrete GPU" — and it is wrong. A discrete GPU
// with no connector attached is a GPU with nothing to display on; pinning
// gamescope to it produces a session with no screen, which is a worse failure
// than the one being fixed. What the user asked for is a screen ("the gaming
// modes should properly boot and on the monitor"), so a screen is what gets
// chosen first, and its card follows from it.
//
// Ranking of connected connectors:
//
//   1. **external before internal.** `eDP-*`, `LVDS-*` and `DSI-*` are the panel
//      built into a laptop; everything else is a cable the user plugged in. A
//      docked laptop with the lid shut still reports `eDP-1` connected, so
//      "external first" is also what makes the dock case work.
//   2. **by connector name**, so the answer is deterministic on a machine with
//      two monitors. Which of two external monitors is "the" gaming monitor is
//      not knowable from sysfs; `APEX_GAMESCOPE_ARGS` overrides it, and the
//      session says out loud that a choice was made.
//
// This rule needs no special case for a single-GPU machine or an all-AMD one:
// there, every connector belongs to the only card, so the same code emits that
// card's PCI id — which is what gamescope would have picked anyway — plus a
// `--prefer-output` that still moves the session onto the monitor rather than
// the panel. One rule, no branches on vendor, and nothing hardcoded about
// `10de:249d`.
//
// ## Failing loudly
//
// Every path that cannot produce an answer records [`DisplayChoice::problem`]
// rather than quietly emitting nothing: a silent fallback to gamescope's
// default IS the defect being fixed here. `Permission denied` on a connector's
// `status` is a refusal, not "disconnected", and is reported as such.

/// One DRM connector, as `/sys/class/drm/card2-HDMI-A-1` describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Connector {
    /// The card the connector hangs off: `card2`.
    pub card: String,
    /// The connector itself, in the name gamescope and `wlr-randr` use:
    /// `HDMI-A-1`. This is the sysfs node name minus the `cardN-` prefix.
    pub name: String,
    /// `status` read back as `connected`.
    pub connected: bool,
    /// `status` could not be read at all — a refusal, which is neither
    /// connected nor disconnected. Kept separate so it cannot be laundered
    /// into "no monitor here".
    pub unreadable: Option<String>,
    /// A panel built into the machine rather than a cable: `eDP`, `LVDS`, `DSI`.
    pub internal: bool,
}

/// Whether a connector name is the machine's own built-in panel.
///
/// The kernel's connector type names, from `drm_connector_enum_list` in
/// `drivers/gpu/drm/drm_connector.c`. Only three of them are soldered to the
/// chassis; everything else (HDMI-A, DP, DVI-D, VGA, Writeback…) is a port.
pub fn is_internal_connector(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    upper.starts_with("EDP-") || upper.starts_with("LVDS-") || upper.starts_with("DSI-")
}

/// Every DRM connector under `<sys>/class/drm`, sorted by card then name.
///
/// A connector node is `cardN-<NAME>`; the card nodes themselves (`cardN`, no
/// dash) are skipped, and so are the render nodes (`renderD128`).
pub fn connectors(sys: &Path) -> Vec<Connector> {
    let mut out: Vec<Connector> = Vec::new();
    let Ok(entries) = std::fs::read_dir(sys.join("class/drm")) else {
        return out;
    };
    for entry in entries.flatten() {
        let node = entry.file_name().to_string_lossy().to_string();
        if !node.starts_with("card") {
            continue;
        }
        let Some((card, name)) = node.split_once('-') else {
            continue;
        };
        if !card["card".len()..].chars().all(|c| c.is_ascii_digit()) || name.is_empty() {
            continue;
        }
        let status_path = entry.path().join("status");
        let (connected, unreadable) = match std::fs::read_to_string(&status_path) {
            Ok(s) => (s.trim().eq_ignore_ascii_case("connected"), None),
            // A connector with no `status` file is not a refusal: sysfs always
            // writes one for a real connector, so its absence means this is not
            // the kind of node being looked for.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (false, None),
            Err(e) => (false, Some(format!("{}: {e}", status_path.display()))),
        };
        out.push(Connector {
            card: card.to_string(),
            name: name.to_string(),
            connected,
            unreadable,
            internal: is_internal_connector(name),
        });
    }
    out.sort_by(|a, b| a.card.cmp(&b.card).then_with(|| a.name.cmp(&b.name)));
    out
}

/// The PCI `vendor:device` pair gamescope's `--prefer-vk-device` matches on,
/// as lowercase hex with no `0x`: `10de:249d`.
///
/// `None` when the DRM node has no PCI device behind it. That is a real case —
/// a SoC display controller, `vkms`, a virtio node — and it means the flag
/// cannot be passed rather than that the machine is broken.
pub fn card_pci_id(sys: &Path, card: &str) -> Option<String> {
    let dev = sys.join("class/drm").join(card).join("device");
    let strip = |s: String| {
        let t = s.trim().to_ascii_lowercase();
        t.strip_prefix("0x").map(str::to_string).unwrap_or(t)
    };
    let vendor = read_trim(&dev.join("vendor")).map(strip)?;
    let device = read_trim(&dev.join("device")).map(strip)?;
    if vendor.is_empty() || device.is_empty() {
        return None;
    }
    Some(format!("{vendor}:{device}"))
}

/// Which screen a full-screen session should open, and on which GPU.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DisplayChoice {
    /// The connector to ask for: `HDMI-A-1`.
    pub output: Option<String>,
    /// The card that owns it: `card2`.
    pub card: Option<String>,
    /// That card's `vendor:device`: `10de:249d`.
    pub pci_id: Option<String>,
    /// That card's vendor, for the log line: `NVIDIA`.
    pub vendor: Option<String>,
    /// How many cards carry a connected connector. `> 1` is the hybrid case
    /// this whole module exists for.
    pub cards_with_displays: usize,
    /// One line saying what was chosen and why, always populated.
    pub why: String,
    /// Set when no complete answer could be produced. Never `Some` and silent:
    /// the caller must print it.
    pub problem: Option<String>,
}

impl DisplayChoice {
    /// The gamescope arguments this choice implies, ready to splice into a
    /// command line. Empty when nothing could be chosen.
    ///
    /// `--prefer-vk-device` comes first because it is the one that was measured
    /// to move the DRM node as well as the Vulkan device; `--prefer-output`
    /// alone cannot reach a connector on a card gamescope never opened.
    pub fn gamescope_args(&self) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(id) = &self.pci_id {
            out.push("--prefer-vk-device".to_string());
            out.push(id.clone());
        }
        if let Some(name) = &self.output {
            out.push("--prefer-output".to_string());
            out.push(name.clone());
        }
        out
    }

    /// Whether this is a complete answer — a screen AND the card to drive it
    /// with. A screen with no PCI id is deliberately not complete: on a hybrid
    /// machine that is exactly the case where gamescope would open the wrong
    /// node.
    pub fn is_complete(&self) -> bool {
        self.output.is_some() && self.pci_id.is_some()
    }
}

/// Choose the screen and GPU for a full-screen session, from sysfs alone.
///
/// Reads only; spawns nothing. See this section's header for the rule and the
/// argument for it.
pub fn choose_display(sys: &Path) -> DisplayChoice {
    let all = connectors(sys);
    let mut choice = DisplayChoice::default();

    if all.is_empty() {
        choice.why = format!("{} lists no DRM connectors", sys.join("class/drm").display());
        choice.problem = Some(
            "no DRM connectors are visible, so no screen could be chosen; gamescope will \
             take its own default, which on a hybrid machine is usually the integrated GPU"
                .to_string(),
        );
        return choice;
    }

    let refused: Vec<&Connector> = all.iter().filter(|c| c.unreadable.is_some()).collect();
    let connected: Vec<&Connector> = all.iter().filter(|c| c.connected).collect();

    let mut cards: Vec<&str> = connected.iter().map(|c| c.card.as_str()).collect();
    cards.sort_unstable();
    cards.dedup();
    choice.cards_with_displays = cards.len();

    if connected.is_empty() {
        choice.why = format!(
            "none of the {} DRM connectors reports `connected`",
            all.len()
        );
        choice.problem = Some(if refused.is_empty() {
            "no display is connected, so no screen could be chosen; gamescope will take its \
             own default"
                .to_string()
        } else {
            format!(
                "no display reports `connected`, but {} connector status file(s) could not \
                 be read, so this is a refusal and not an answer: {}",
                refused.len(),
                refused
                    .iter()
                    .filter_map(|c| c.unreadable.clone())
                    .collect::<Vec<_>>()
                    .join("; ")
            )
        });
        return choice;
    }

    // Rule, in one expression: external before internal, then by name.
    let pick = connected
        .iter()
        .min_by(|a, b| {
            (a.internal, a.name.as_str())
                .cmp(&(b.internal, b.name.as_str()))
        })
        .copied()
        .expect("connected is non-empty");

    let pci = card_pci_id(sys, &pick.card);
    let vendor = read_trim(&sys.join("class/drm").join(&pick.card).join("device/vendor"))
        .map(|id| GpuVendor::from_pci_id(&id).label().to_string());

    let kind = if pick.internal {
        "the built-in panel"
    } else {
        "an external display"
    };
    choice.why = format!(
        "{} on {} is {}; {} connected output(s) across {} card(s)",
        pick.name,
        pick.card,
        kind,
        connected.len(),
        choice.cards_with_displays,
    );
    choice.output = Some(pick.name.clone());
    choice.card = Some(pick.card.clone());
    choice.vendor = vendor;
    choice.pci_id = pci;

    if choice.pci_id.is_none() {
        choice.problem = Some(format!(
            "{} has no PCI vendor/device id under sysfs, so gamescope cannot be pinned to it \
             with --prefer-vk-device; it may still open a different card",
            pick.card
        ));
    } else if !refused.is_empty() {
        choice.problem = Some(format!(
            "{} connector status file(s) could not be read, so a better screen may exist that \
             this could not see: {}",
            refused.len(),
            refused
                .iter()
                .filter_map(|c| c.unreadable.clone())
                .collect::<Vec<_>>()
                .join("; ")
        ));
    }
    choice
}
