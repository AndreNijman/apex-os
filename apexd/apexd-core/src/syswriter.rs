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
