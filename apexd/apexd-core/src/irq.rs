//! IRQ affinity steering.
//!
//! Enumerates `/proc/irq/<n>/` read-only, records each interrupt's current
//! `smp_affinity_list`, and plans writes that park housekeeping interrupts on
//! the CPUs a game is *not* pinned to. Interrupts whose handler matches the
//! profile's `irq_pin_to_game` list (the NVIDIA GPU, typically) are steered the
//! other way — onto the game's cores — because their work belongs to the game.
//!
//! Many interrupts are kernel-managed (per-CPU timers, MSI-X queues with
//! `IRQD_AFFINITY_MANAGED`) and simply refuse an affinity write with `-EIO`.
//! That is expected and non-fatal: the writer logs a skip and carries on.

use std::path::Path;

use crate::tier::Action;
use crate::topology::format_cpu_list;

/// The procfs IRQ root. Actions carry absolute paths built from this, so
/// nothing else in the crate needs a procfs root.
pub const PROC_IRQ: &str = "/proc/irq";

/// One interrupt as read from procfs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IrqEntry {
    pub irq: u32,
    /// Absolute path to `smp_affinity_list`.
    pub path: String,
    /// The affinity list at read time (`0-19`), used verbatim on restore.
    pub affinity: String,
    /// Handler names (the sub-directories of `/proc/irq/<n>`), e.g. `nvidia`.
    pub actions: Vec<String>,
}

impl IrqEntry {
    /// True when any handler name contains one of `needles` (case-insensitive).
    pub fn matches(&self, needles: &[String]) -> bool {
        self.actions.iter().any(|a| {
            let a = a.to_ascii_lowercase();
            needles.iter().any(|n| !n.is_empty() && a.contains(&n.to_ascii_lowercase()))
        })
    }
}

/// Enumerate interrupts under `proc_irq_root` (normally [`PROC_IRQ`]).
/// Read-only; an unreadable root yields an empty list.
pub fn enumerate(proc_irq_root: &Path) -> Vec<IrqEntry> {
    let Ok(entries) = std::fs::read_dir(proc_irq_root) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for e in entries.flatten() {
        let dir = e.path();
        let Some(name) = dir.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        let Ok(irq) = name.parse::<u32>() else {
            continue;
        };
        let affinity_path = dir.join("smp_affinity_list");
        let Ok(affinity) = std::fs::read_to_string(&affinity_path) else {
            continue; // no affinity control for this interrupt
        };
        let mut actions: Vec<String> = Vec::new();
        if let Ok(sub) = std::fs::read_dir(&dir) {
            for s in sub.flatten() {
                if s.path().is_dir() {
                    actions.push(s.file_name().to_string_lossy().to_string());
                }
            }
        }
        actions.sort();
        out.push(IrqEntry {
            irq,
            path: affinity_path.to_string_lossy().to_string(),
            affinity: affinity.trim().to_string(),
            actions,
        });
    }
    out.sort_by_key(|e| e.irq);
    out
}

/// True when an `irqbalance` daemon is running.
///
/// This matters because irqbalance re-scans and re-assigns interrupt affinity
/// on its own cadence (10 s by default) and will quietly undo everything
/// [`plan_steer`] does. apexd does not try to fight it — it reports the
/// conflict, and the image is expected to mask irqbalance or ban the game CPUs
/// (see the IMAGE TODO list in `docs/m6-notes.md`).
pub fn irqbalance_running(proc_root: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(proc_root) else {
        return false;
    };
    for e in entries.flatten() {
        let name = e.file_name();
        let Some(name) = name.to_str() else { continue };
        if !name.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        if let Ok(comm) = std::fs::read_to_string(e.path().join("comm")) {
            if comm.trim() == "irqbalance" {
                return true;
            }
        }
    }
    false
}

/// Plan the steering for a game session.
///
/// * `housekeeping` — the CPUs interrupts should move to (the complement of the
///   game's cpuset). An empty list disables steering entirely.
/// * `pin_to_game` — handler-name substrings whose interrupts should instead be
///   pinned onto `game_cpus`.
///
/// Returns `(steer_actions, restore_actions)`. The restore list is built from
/// the values read *now*, so exiting puts every interrupt back exactly where it
/// was — including the ones the kernel later refuses to move.
pub fn plan_steer(
    entries: &[IrqEntry],
    game_cpus: &[u32],
    housekeeping: &[u32],
    pin_to_game: &[String],
) -> (Vec<Action>, Vec<Action>) {
    let mut steer = Vec::new();
    let mut restore = Vec::new();
    if housekeeping.is_empty() || game_cpus.is_empty() {
        return (steer, restore);
    }
    let away = format_cpu_list(housekeeping);
    let onto = format_cpu_list(game_cpus);

    for e in entries {
        // IRQ 0 (timer) and 2 (cascade) are never steerable; skip the noise.
        if e.irq == 0 || e.irq == 2 {
            continue;
        }
        let target = if e.matches(pin_to_game) { &onto } else { &away };
        if &e.affinity == target {
            continue; // already there — nothing to change, nothing to restore
        }
        steer.push(Action::IrqAffinity {
            path: e.path.clone(),
            cpus: target.clone(),
        });
        restore.push(Action::IrqAffinity {
            path: e.path.clone(),
            cpus: e.affinity.clone(),
        });
    }
    (steer, restore)
}
