//! Game-mode orchestration planning: cpuset pinning, IRQ steering and NVIDIA
//! clock locks, resolved into a symmetric pair of action lists.
//!
//! The planner is pure. It takes the machine's topology, the interrupts as they
//! are *right now*, and what `nvidia-smi` reported, and returns both the enter
//! plan and the exit plan that undoes it. Building the exit plan up-front, from
//! values read before anything was written, is what makes "exit restores
//! exactly what enter changed" a property of the data rather than of a code
//! path that has to be remembered.
//!
//! Tier and fan changes are *not* in these lists — those go through the
//! daemon's existing tier engine and fan controller, which have their own
//! restore paths.

use crate::gpu::{self, NvidiaGpu};
use crate::irq::{self, IrqEntry};
use crate::profile::{CpusetPolicy, GameModeConfig, IrqPolicy};
use crate::tier::Action;
use crate::topology::{format_cpu_list, parse_cpu_list, CoreTopology};

/// The default cgroup-v2 mount point.
pub const CGROUP_ROOT: &str = "/sys/fs/cgroup";

/// Read the cgroup-v2 root's effective NUMA nodes (for `cpuset.mems`).
/// Falls back to `0`, which is correct for every single-socket machine.
pub fn read_cgroup_mems(cgroup_root: &std::path::Path) -> String {
    for attr in ["cpuset.mems.effective", "cpuset.mems"] {
        if let Ok(s) = std::fs::read_to_string(cgroup_root.join(attr)) {
            let s = s.trim().to_string();
            if !s.is_empty() {
                return s;
            }
        }
    }
    "0".to_string()
}

/// The absolute cgroup-v2 directory a PID currently lives in, by reading
/// `/proc/<pid>/cgroup`. `None` when the process is gone or on cgroup v1.
pub fn read_pid_cgroup(proc_root: &std::path::Path, cgroup_root: &str, pid: u32) -> Option<String> {
    let text = std::fs::read_to_string(proc_root.join(pid.to_string()).join("cgroup")).ok()?;
    for line in text.lines() {
        // cgroup v2 lines look like `0::/user.slice/...`.
        if let Some(rest) = line.strip_prefix("0::") {
            let rel = rest.trim();
            let rel = rel.strip_prefix('/').unwrap_or(rel);
            return Some(if rel.is_empty() {
                cgroup_root.to_string()
            } else {
                format!("{cgroup_root}/{rel}")
            });
        }
    }
    None
}

/// A process to pin, plus the cgroup it came from (so exit can put it back).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PidPlacement {
    pub pid: u32,
    /// Absolute path of the cgroup the PID was in before we moved it.
    pub prior_cgroup: Option<String>,
}

/// Everything the planner needs.
pub struct GameInputs<'a> {
    pub cfg: &'a GameModeConfig,
    pub topo: &'a CoreTopology,
    pub nvidia: &'a [NvidiaGpu],
    pub irqs: &'a [IrqEntry],
    pub pids: &'a [PidPlacement],
    /// Value for `cpuset.mems` (normally the root cgroup's effective mems).
    pub mems: String,
    /// Whether an `irqbalance` daemon is running; it will undo IRQ steering, so
    /// the plan says so out loud rather than pretending the pinning holds.
    pub irqbalance: bool,
}

/// The symmetric plan.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GamePlan {
    pub enter: Vec<Action>,
    pub exit: Vec<Action>,
    /// CPUs the game is confined to.
    pub cpus: Vec<u32>,
    /// CPUs everything else is pushed onto.
    pub housekeeping: Vec<u32>,
    /// GPU indices whose clocks were locked.
    pub gpus_locked: Vec<u32>,
    /// How many interrupts the plan attempts to move.
    pub irqs_steered: usize,
    /// Human-readable explanations (shown by `apex game status`).
    pub notes: Vec<String>,
}

impl GamePlan {
    /// `0-11` rendering of the game's cpuset.
    pub fn cpu_list(&self) -> String {
        format_cpu_list(&self.cpus)
    }
}

/// Resolve the cpuset policy into an explicit CPU list.
pub fn resolve_cpus(cfg: &GameModeConfig, topo: &CoreTopology, notes: &mut Vec<String>) -> Vec<u32> {
    match cfg.cpuset_policy() {
        CpusetPolicy::Off => {
            notes.push("cpuset pinning disabled by profile".into());
            Vec::new()
        }
        CpusetPolicy::All => topo.all.clone(),
        CpusetPolicy::PCores => {
            if topo.is_hybrid() {
                notes.push(format!(
                    "P-cores {} (detected via {}), E-cores {}",
                    topo.pcore_list(),
                    topo.source.as_str(),
                    topo.ecore_list()
                ));
                topo.pcores.clone()
            } else {
                notes.push(format!(
                    "no P/E split detected ({}) — pinning to all CPUs",
                    topo.source.as_str()
                ));
                topo.all.clone()
            }
        }
        CpusetPolicy::Explicit(list) => {
            let want = parse_cpu_list(&list);
            let have: Vec<u32> = want.iter().copied().filter(|c| topo.all.contains(c)).collect();
            if have.is_empty() {
                notes.push(format!(
                    "profile cpuset '{list}' matches no online CPU — pinning to all CPUs"
                ));
                topo.all.clone()
            } else {
                have
            }
        }
    }
}

/// Build the enter/exit plans.
pub fn plan(inputs: &GameInputs<'_>) -> GamePlan {
    let cfg = inputs.cfg;
    let mut notes = Vec::new();
    let cpus = resolve_cpus(cfg, inputs.topo, &mut notes);
    let housekeeping = inputs.topo.complement(&cpus);

    let mut enter = Vec::new();
    let mut exit = Vec::new();

    // ── 0. sched-ext scheduler ───────────────────────────────────────────────
    // FIRST on enter and LAST on exit, on purpose: swapping the scheduler
    // migrates every runnable task, so do it before the game is pinned into its
    // cpuset (and undo it after the pinning is unwound), rather than shuffling
    // tasks that are mid-move.
    //
    // Empty `scx` = leave the kernel scheduler alone, which is the default for
    // every profile that does not ask. The Gaming profiles opt in to scx_lavd:
    // it is the latency-first sched-ext scheduler, which is the one that helps a
    // game rather than a build farm.
    if !cfg.scx.trim().is_empty() {
        enter.push(Action::ScxSwitch {
            sched: cfg.scx.trim().to_string(),
        });
        // NOTE the exit half is appended at the very END of this function, not
        // here: pushing it now would make ScxStop the FIRST exit action, i.e.
        // restore the scheduler while the game is still pinned. A test asserts
        // the ordering, and it caught exactly that mistake.
        notes.push(format!(
            "sched-ext: {} for the session, kernel scheduler restored on exit",
            cfg.scx.trim()
        ));
    }

    // ── 1. cpuset ────────────────────────────────────────────────────────────
    let pinning = !cpus.is_empty() && cpus.len() < inputs.topo.all.len().max(1);
    if !cpus.is_empty() && cfg.cpuset_policy() != CpusetPolicy::Off {
        enter.push(Action::CgroupEnsure {
            path: cfg.cgroup.clone(),
            cpus: format_cpu_list(&cpus),
            mems: inputs.mems.clone(),
        });
        for p in inputs.pids {
            enter.push(Action::CgroupAttach {
                path: cfg.cgroup.clone(),
                pid: p.pid,
            });
        }
        if !pinning {
            notes.push("cpuset covers every CPU — the cgroup is created but confines nothing".into());
        }
    }

    // ── 2. IRQ steering ──────────────────────────────────────────────────────
    let (mut steer, mut irq_restore) = match cfg.irq_policy() {
        IrqPolicy::Off => (Vec::new(), Vec::new()),
        IrqPolicy::AwayFromGame => {
            if pinning {
                irq::plan_steer(inputs.irqs, &cpus, &housekeeping, &cfg.irq_pin_to_game)
            } else {
                notes.push("IRQ steering skipped — the game is not confined to a subset of CPUs".into());
                (Vec::new(), Vec::new())
            }
        }
    };
    let irqs_steered = steer.len();
    if irqs_steered > 0 && inputs.irqbalance {
        notes.push(
            "irqbalance is running and will re-scatter these interrupts — mask it, or ban the game CPUs in its config".into(),
        );
    }
    enter.append(&mut steer);

    // ── 3. NVIDIA clock locks ────────────────────────────────────────────────
    let mut gpus_locked = Vec::new();
    for gpu_info in inputs.nvidia {
        if let Some(only) = cfg.nvidia.gpu_index {
            if only != gpu_info.index {
                continue;
            }
        }
        let lock = gpu::plan_lock(&cfg.nvidia, gpu_info);
        if lock.is_empty() {
            continue;
        }
        let has_clock_lock = lock.iter().any(|a| {
            matches!(
                a,
                Action::NvidiaLockGraphics { .. } | Action::NvidiaLockMemory { .. }
            )
        });
        if has_clock_lock {
            gpus_locked.push(gpu_info.index);
        }
        enter.extend(lock);
        // Unlock first on the way out.
        let mut unlock = gpu::plan_unlock(&cfg.nvidia, gpu_info);
        unlock.append(&mut exit);
        exit = unlock;
    }
    if inputs.nvidia.is_empty() {
        notes.push("no NVIDIA GPU reported by nvidia-smi — GPU clock locking skipped".into());
    }

    // ── exit: IRQs, then release the cgroup ──────────────────────────────────
    exit.append(&mut irq_restore);
    if !cpus.is_empty() && cfg.cpuset_policy() != CpusetPolicy::Off {
        for p in inputs.pids {
            if let Some(prior) = &p.prior_cgroup {
                exit.push(Action::CgroupAttach {
                    path: prior.clone(),
                    pid: p.pid,
                });
            }
        }
        exit.push(Action::CgroupRemove {
            path: cfg.cgroup.clone(),
        });
    }

    // Hand scheduling back only after every cpuset/IRQ/clock action has been
    // unwound — the mirror of loading it first on enter.
    if !cfg.scx.trim().is_empty() {
        exit.push(Action::ScxStop);
    }

    GamePlan {
        enter,
        exit,
        cpus,
        housekeeping,
        gpus_locked,
        irqs_steered,
        notes,
    }
}
