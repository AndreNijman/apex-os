//! The one and only path from an [`Action`] to a real hardware effect.
//!
//! `apexd-core` never writes sysfs or spawns a process directly; it emits
//! [`Action`]s and hands them to a [`SysWriter`]. Production uses
//! [`RealWriter`] (which also honours dry-run); tests use [`MockWriter`], which
//! records intended actions and touches nothing. This is what lets every logic
//! path be unit-tested without writing real sysfs.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Context, Result};

use crate::tier::Action;

/// Turns intended [`Action`]s into effects.
pub trait SysWriter: Send + Sync {
    /// Apply one action.
    fn apply(&self, action: &Action) -> Result<()>;

    /// Apply a whole plan in order, stopping on the first hard error.
    fn apply_all(&self, actions: &[Action]) -> Result<()> {
        for a in actions {
            self.apply(a)?;
        }
        Ok(())
    }

    /// Whether this writer will actually mutate hardware. `false` for dry-run
    /// and for the mock.
    fn is_live(&self) -> bool {
        false
    }
}

/// Writes real sysfs and execs `ryzenadj`. When `dry_run` is set, it logs the
/// intended effect and does nothing — the same switch `APEXD_DRY_RUN=1` flips.
pub struct RealWriter {
    dry_run: bool,
    sys_root: PathBuf,
}

impl RealWriter {
    /// A writer rooted at real `/sys`.
    pub fn new(dry_run: bool) -> RealWriter {
        RealWriter {
            dry_run,
            sys_root: PathBuf::from("/sys"),
        }
    }

    /// A writer rooted at an explicit sysfs path (for a sandbox/fixture). Still
    /// gated by `dry_run`.
    pub fn with_root(dry_run: bool, sys_root: impl Into<PathBuf>) -> RealWriter {
        RealWriter {
            dry_run,
            sys_root: sys_root.into(),
        }
    }

    pub fn is_dry_run(&self) -> bool {
        self.dry_run
    }

    /// Write a value to an absolute path, treating both a missing attribute and
    /// a rejected write as a *skip* rather than an error. M6 touches knobs the
    /// kernel routinely refuses (kernel-managed IRQ affinity, cpuset attributes
    /// on a delegated cgroup); a refusal must never abort the rest of a plan —
    /// least of all a restore plan.
    ///
    /// Returns `true` when the value was actually written.
    fn write_tolerant(&self, path: &Path, value: &str, what: &str) -> bool {
        if !path.exists() {
            eprintln!("apexd: skip (absent) {} <- {value}", path.display());
            return false;
        }
        if self.dry_run {
            eprintln!("apexd: [dry-run] {what}: {} <- {value}", path.display());
            // Report success so callers that ladder down through fallbacks
            // (the fan restore) show what they *would* have done, not every
            // rung of a ladder no real write ever descended.
            return true;
        }
        match std::fs::write(path, value) {
            Ok(()) => true,
            Err(e) => {
                eprintln!("apexd: skip ({what} rejected) {} <- {value}: {e}", path.display());
                false
            }
        }
    }

    /// Like [`RealWriter::write_tolerant`] but without the existence check.
    ///
    /// cgroup-v2 attributes (`cpuset.cpus`, `cgroup.procs`, ...) are
    /// materialised by the kernel the moment the directory is created, so
    /// "absent" is not a meaningful state to test for there — and on a plain
    /// filesystem (a test fixture) the write simply creates the file, which is
    /// the behaviour the kernel presents anyway.
    fn write_forced(&self, path: &Path, value: &str, what: &str) -> bool {
        if self.dry_run {
            eprintln!("apexd: [dry-run] {what}: {} <- {value}", path.display());
            // Report success so callers that ladder down through fallbacks
            // (the fan restore) show what they *would* have done, not every
            // rung of a ladder no real write ever descended.
            return true;
        }
        match std::fs::write(path, value) {
            Ok(()) => true,
            Err(e) => {
                eprintln!("apexd: skip ({what} rejected) {} <- {value}: {e}", path.display());
                false
            }
        }
    }

    /// Write a value to a sysfs attribute if it exists. A missing attribute is
    /// not an error (the profile expresses full intent; hardware may not have
    /// every knob).
    fn write_if_present(&self, path: &Path, value: &str) -> Result<()> {
        if !path.exists() {
            eprintln!("apexd: skip (absent) {} <- {value}", path.display());
            return Ok(());
        }
        if self.dry_run {
            eprintln!("apexd: [dry-run] {} <- {value}", path.display());
            return Ok(());
        }
        std::fs::write(path, value)
            .with_context(|| format!("writing {} <- {value}", path.display()))?;
        Ok(())
    }

    /// Every cpufreq policy directory under the sysfs root.
    fn cpufreq_policies(&self) -> Vec<PathBuf> {
        let base = self.sys_root.join("devices/system/cpu/cpufreq");
        let mut out = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&base) {
            for e in entries.flatten() {
                let p = e.path();
                if p.file_name()
                    .and_then(|s| s.to_str())
                    .map(|s| s.starts_with("policy"))
                    .unwrap_or(false)
                {
                    out.push(p);
                }
            }
        }
        out.sort();
        out
    }

    fn write_all_policies(&self, attr: &str, value: &str) -> Result<()> {
        let policies = self.cpufreq_policies();
        if policies.is_empty() {
            eprintln!("apexd: no cpufreq policies found; skip {attr} <- {value}");
        }
        for p in policies {
            self.write_if_present(&p.join(attr), value)?;
        }
        Ok(())
    }

    fn run_ryzenadj(
        &self,
        stapm_mw: u32,
        fast_mw: u32,
        slow_mw: u32,
        tctl_max: Option<u32>,
    ) -> Result<()> {
        let mut args = vec![
            format!("--stapm-limit={stapm_mw}"),
            format!("--fast-limit={fast_mw}"),
            format!("--slow-limit={slow_mw}"),
        ];
        if let Some(t) = tctl_max {
            args.push(format!("--tctl-temp={t}"));
        }
        if self.dry_run {
            eprintln!("apexd: [dry-run] ryzenadj {}", args.join(" "));
            return Ok(());
        }
        let status = std::process::Command::new("ryzenadj")
            .args(&args)
            .status()
            .context("spawning ryzenadj")?;
        if !status.success() {
            anyhow::bail!("ryzenadj exited with {status}");
        }
        Ok(())
    }

    /// Run `nvidia-smi` with `args`. A missing binary or a non-zero exit is a
    /// logged skip, never an error: a machine with no NVIDIA GPU must still be
    /// able to enter game mode.
    fn run_nvidia_smi(&self, args: &[String]) -> Result<()> {
        if self.dry_run {
            eprintln!("apexd: [dry-run] nvidia-smi {}", args.join(" "));
            return Ok(());
        }
        if !crate::gpu::nvidia_smi_available() {
            eprintln!("apexd: skip (nvidia-smi absent) nvidia-smi {}", args.join(" "));
            return Ok(());
        }
        match std::process::Command::new("nvidia-smi").args(args).output() {
            Ok(out) if out.status.success() => Ok(()),
            Ok(out) => {
                eprintln!(
                    "apexd: nvidia-smi {} failed ({}): {}",
                    args.join(" "),
                    out.status,
                    String::from_utf8_lossy(&out.stderr).trim()
                );
                Ok(())
            }
            Err(e) => {
                eprintln!("apexd: nvidia-smi {} could not run: {e}", args.join(" "));
                Ok(())
            }
        }
    }

    /// Hand a fan back to firmware control. The ladder is the safety guarantee:
    /// prior `pwm*_enable` -> `2` (firmware automatic) -> `0` (no control, which
    /// the hwmon ABI defines as *full speed*), and if a manual mode is all the
    /// hardware offers, the duty cycle is driven to 255 rather than left low.
    /// No path through this function can leave a fan stopped.
    fn fan_safe_restore(
        &self,
        enable_path: Option<&str>,
        pwm_path: Option<&str>,
        prior_enable: Option<u8>,
        prior_pwm: Option<u8>,
    ) -> Result<()> {
        let Some(enable) = enable_path else {
            // No enable attribute: the only lever is the duty cycle. Restore the
            // recorded value, or go to full speed if we never recorded one.
            if let Some(pwm) = pwm_path {
                let v = prior_pwm.unwrap_or(255);
                self.write_tolerant(Path::new(pwm), &v.to_string(), "fan restore pwm");
            }
            return Ok(());
        };
        let enable = Path::new(enable);

        // 1. The value the fan had before we touched it (usually 2 = firmware).
        if let Some(prior) = prior_enable {
            if self.write_tolerant(enable, &prior.to_string(), "fan restore enable") {
                // Manual mode was the *prior* state; put its duty cycle back too,
                // and never below full speed if we do not know what it was.
                if prior == 1 {
                    if let Some(pwm) = pwm_path {
                        let v = prior_pwm.unwrap_or(255);
                        self.write_tolerant(Path::new(pwm), &v.to_string(), "fan restore pwm");
                    }
                }
                return Ok(());
            }
        }
        // 2. Firmware automatic.
        if self.write_tolerant(enable, "2", "fan restore auto") {
            return Ok(());
        }
        // 3. Last resort: full speed. Push the duty cycle up *first* so that a
        //    driver treating `0` as "manual, keep current pwm" still ends up
        //    with the fan spinning flat out.
        if let Some(pwm) = pwm_path {
            self.write_tolerant(Path::new(pwm), "255", "fan restore full-speed pwm");
        }
        self.write_tolerant(enable, "0", "fan restore full-speed");
        Ok(())
    }

    /// Create a cgroup-v2 directory (if needed) and apply a cpuset to it.
    /// Enabling the `cpuset` controller in the parent's `subtree_control` is
    /// best-effort: on a systemd host the root cgroup may already delegate it.
    fn cgroup_ensure(&self, path: &str, cpus: &str, mems: &str) -> Result<()> {
        let dir = Path::new(path);
        if self.dry_run {
            eprintln!("apexd: [dry-run] cgroup {path}: cpuset.cpus={cpus} cpuset.mems={mems}");
            return Ok(());
        }
        if let Some(parent) = dir.parent() {
            let sc = parent.join("cgroup.subtree_control");
            if sc.exists() {
                // Only meaningful if cpuset is not already enabled; a duplicate
                // write is harmless and a rejection is tolerated.
                self.write_tolerant(&sc, "+cpuset", "cgroup subtree_control");
            }
        }
        if !dir.exists() {
            if let Err(e) = std::fs::create_dir_all(dir) {
                eprintln!("apexd: cgroup {path}: create failed: {e}");
                return Ok(());
            }
        }
        self.write_forced(&dir.join("cpuset.mems"), mems, "cpuset.mems");
        self.write_forced(&dir.join("cpuset.cpus"), cpus, "cpuset.cpus");
        Ok(())
    }

    /// Remove a cgroup directory. `rmdir` is all the kernel needs (its
    /// auto-populated attribute files do not block it); the `remove_dir_all`
    /// fallback exists for plain filesystems — test fixtures — and is only
    /// attempted when the directory holds no sub-directories, so a cgroup with
    /// children is never blown away.
    fn cgroup_remove(&self, path: &str) -> Result<()> {
        let dir = Path::new(path);
        if self.dry_run {
            eprintln!("apexd: [dry-run] cgroup {path}: remove");
            return Ok(());
        }
        if !dir.exists() {
            return Ok(());
        }
        match std::fs::remove_dir(dir) {
            Ok(()) => return Ok(()),
            Err(e) => {
                let has_subdirs = std::fs::read_dir(dir)
                    .map(|it| it.flatten().any(|e| e.path().is_dir()))
                    .unwrap_or(true);
                if has_subdirs {
                    eprintln!("apexd: cgroup {path}: remove skipped ({e}); it has child cgroups");
                    return Ok(());
                }
                if let Err(e2) = std::fs::remove_dir_all(dir) {
                    eprintln!("apexd: cgroup {path}: remove skipped: {e} / {e2}");
                }
            }
        }
        Ok(())
    }
}

impl SysWriter for RealWriter {
    fn apply(&self, action: &Action) -> Result<()> {
        match action {
            Action::Governor(g) => self.write_all_policies("scaling_governor", g),
            Action::Epp(e) => self.write_all_policies("energy_performance_preference", e),
            Action::PlatformProfile(p) => self.write_if_present(
                &self.sys_root.join("firmware/acpi/platform_profile"),
                p,
            ),
            Action::ChargeThresholds {
                start,
                stop,
                start_path,
                end_path,
            } => {
                // Stop threshold last: some ECs reject a start >= stop, and
                // writing stop first widens the window before narrowing.
                self.write_if_present(Path::new(end_path), &stop.to_string())?;
                self.write_if_present(Path::new(start_path), &start.to_string())?;
                Ok(())
            }
            Action::RyzenAdj {
                stapm_mw,
                fast_mw,
                slow_mw,
                tctl_max,
            } => self.run_ryzenadj(*stapm_mw, *fast_mw, *slow_mw, *tctl_max),
            // The reapply loop lives in the daemon; at the writer level there
            // is nothing to undo (limits decay on their own once we stop
            // re-asserting them).
            Action::StopRyzenAdj => {
                if self.dry_run {
                    eprintln!("apexd: [dry-run] stop ryzenadj loop");
                }
                Ok(())
            }

            // ── M6 ───────────────────────────────────────────────────────────
            Action::FanPwmEnable { path, value } => {
                self.write_tolerant(Path::new(path), &value.to_string(), "pwm_enable");
                Ok(())
            }
            Action::FanPwm { path, value } => {
                self.write_tolerant(Path::new(path), &value.to_string(), "pwm");
                Ok(())
            }
            Action::FanVendorAttr { path, value, what } => {
                self.write_tolerant(Path::new(path), value, what);
                Ok(())
            }
            Action::FanSafeRestore {
                enable_path,
                pwm_path,
                prior_enable,
                prior_pwm,
            } => self.fan_safe_restore(
                enable_path.as_deref(),
                pwm_path.as_deref(),
                *prior_enable,
                *prior_pwm,
            ),
            Action::NvidiaPersistence { gpu, enabled } => self.run_nvidia_smi(&[
                "-i".into(),
                gpu.to_string(),
                "-pm".into(),
                u8::from(*enabled).to_string(),
            ]),
            Action::NvidiaLockGraphics {
                gpu,
                min_mhz,
                max_mhz,
            } => self.run_nvidia_smi(&[
                "-i".into(),
                gpu.to_string(),
                "-lgc".into(),
                format!("{min_mhz},{max_mhz}"),
            ]),
            Action::NvidiaLockMemory {
                gpu,
                min_mhz,
                max_mhz,
            } => self.run_nvidia_smi(&[
                "-i".into(),
                gpu.to_string(),
                "-lmc".into(),
                format!("{min_mhz},{max_mhz}"),
            ]),
            Action::NvidiaResetGraphics { gpu } => {
                self.run_nvidia_smi(&["-i".into(), gpu.to_string(), "-rgc".into()])
            }
            Action::NvidiaResetMemory { gpu } => {
                self.run_nvidia_smi(&["-i".into(), gpu.to_string(), "-rmc".into()])
            }
            Action::IrqAffinity { path, cpus } => {
                self.write_tolerant(Path::new(path), cpus, "irq affinity");
                Ok(())
            }
            Action::CgroupEnsure { path, cpus, mems } => self.cgroup_ensure(path, cpus, mems),
            Action::CgroupAttach { path, pid } => {
                self.write_forced(
                    &Path::new(path).join("cgroup.procs"),
                    &pid.to_string(),
                    "cgroup attach",
                );
                Ok(())
            }
            Action::CgroupRemove { path } => self.cgroup_remove(path),
        }
    }

    fn is_live(&self) -> bool {
        !self.dry_run
    }
}

/// Records intended actions without touching anything. The backbone of the
/// unit tests.
#[derive(Default)]
pub struct MockWriter {
    actions: Mutex<Vec<Action>>,
}

impl MockWriter {
    pub fn new() -> MockWriter {
        MockWriter::default()
    }

    /// A snapshot of every action applied so far, in order.
    pub fn recorded(&self) -> Vec<Action> {
        self.actions.lock().unwrap().clone()
    }

    /// Clear the record.
    pub fn clear(&self) {
        self.actions.lock().unwrap().clear();
    }
}

impl SysWriter for MockWriter {
    fn apply(&self, action: &Action) -> Result<()> {
        self.actions.lock().unwrap().push(action.clone());
        Ok(())
    }
}
