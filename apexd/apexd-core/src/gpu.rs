//! NVIDIA clock locking via `nvidia-smi`.
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

use crate::profile::{ClockSpec, NvidiaConfig};
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
