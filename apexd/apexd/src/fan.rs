//! The daemon's fan controller: discovery once at start-up, a snapshot taken
//! the first time apexd commands anything, a curve loop, and a restore path
//! that is called on *every* exit route.
//!
//! Safety model (see `docs/m6-notes.md` for the full argument):
//!
//! 1. apexd never issues a duty cycle below the profile's `min_pwm`.
//! 2. The state of every control is snapshotted before the first mutation and
//!    replayed verbatim on restore.
//! 3. Restore is expressed as [`apexd_core::tier::Action::FanSafeRestore`],
//!    whose writer-side ladder ends at *full speed* rather than at a stopped
//!    fan when every friendlier option is refused.
//! 4. `apexd.service` carries `ExecStopPost=/usr/bin/apex fan restore --local`,
//!    so a crash — where no in-process restore can run — still returns the fans
//!    to firmware control.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{bail, Result};
use apexd_core::fan::{self, FanInventory, FanMode, FanReading, FanSnapshot};
use apexd_core::profile::FanConfig;
use apexd_core::syswriter::SysWriter;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

/// Read a small unsigned sysfs value.
fn read_u8(path: &str) -> Option<u8> {
    std::fs::read_to_string(path)
        .ok()?
        .trim()
        .parse::<u32>()
        .ok()
        .map(|v| v.min(255) as u8)
}

/// Read a trimmed sysfs string.
fn read_str(path: &str) -> Option<String> {
    std::fs::read_to_string(path).ok().map(|s| s.trim().to_string())
}

/// Mutable fan state.
struct Inner {
    mode: FanMode,
    /// Captured on the first mutation, never overwritten afterwards.
    snapshot: Option<FanSnapshot>,
    curve: Option<JoinHandle<()>>,
}

/// Owns fan discovery, the current mode, and the restore path.
pub struct FanController {
    inv: FanInventory,
    cfg: FanConfig,
    writer: Arc<dyn SysWriter>,
    sys_root: PathBuf,
    inner: Mutex<Inner>,
}

impl FanController {
    /// Discover fans under `sys_root` and start out reporting `auto` (which is
    /// the truth: we have not touched anything yet).
    pub fn new(
        sys_root: impl Into<PathBuf>,
        cfg: FanConfig,
        writer: Arc<dyn SysWriter>,
    ) -> Arc<FanController> {
        let sys_root = sys_root.into();
        let inv = FanInventory::discover(&sys_root, &cfg);
        Arc::new(FanController {
            inv,
            cfg,
            writer,
            sys_root,
            inner: Mutex::new(Inner {
                mode: FanMode::Auto,
                snapshot: None,
                curve: None,
            }),
        })
    }

    /// True when this machine has a fan knob apexd can actually turn.
    pub fn supported(&self) -> bool {
        self.inv.controllable()
    }

    /// The mode keywords this machine accepts.
    pub fn modes(&self) -> Vec<String> {
        self.inv.modes(&self.cfg)
    }

    /// A short description of the discovered backends, for the start-up log and
    /// `apex fan status`.
    pub fn backends(&self) -> Vec<String> {
        self.inv.summary()
    }

    /// The current mode keyword (`auto` when nothing is controllable — the
    /// frozen `Fan.Mode` property never reports "unsupported").
    pub async fn mode(&self) -> FanMode {
        self.inner.lock().await.mode
    }

    /// Current readings for every discovered fan.
    pub fn readings(&self) -> Vec<FanReading> {
        self.inv.read()
    }

    /// Apply the profile's start-up fan mode, if it declares one.
    pub async fn apply_default(self: &Arc<Self>) {
        let Some(mode) = self.cfg.default_mode.clone() else {
            return;
        };
        if !self.supported() {
            return;
        }
        match FanMode::parse(&mode, self.cfg.min_pwm) {
            Ok(m) => {
                if let Err(e) = self.set_mode(m).await {
                    eprintln!("apexd: fan: default mode '{mode}' failed: {e:#}");
                }
            }
            Err(e) => eprintln!("apexd: fan: profile default mode invalid: {e}"),
        }
    }

    /// Switch fan mode. Captures the pre-apexd snapshot on the first mutation.
    pub async fn set_mode(self: &Arc<Self>, mode: FanMode) -> Result<()> {
        if !self.supported() {
            bail!("no controllable fan on this machine");
        }
        if matches!(mode, FanMode::Curve) {
            if self.cfg.curve.is_empty() {
                bail!("this profile declares no fan curve");
            }
            if self.inv.controls.is_empty() {
                bail!("curve mode needs a PWM channel; this machine has none");
            }
        }

        {
            let mut inner = self.inner.lock().await;
            if inner.snapshot.is_none() {
                inner.snapshot = Some(FanSnapshot::capture(&self.inv));
            }
            // Any mode change stops a running curve loop first.
            if let Some(h) = inner.curve.take() {
                h.abort();
            }
        }

        match mode {
            FanMode::Curve => self.start_curve().await,
            other => {
                self.apply(&fan::plan_mode(&self.inv, &self.cfg, other));
                // A writable attribute is not the same as an effective one:
                // thinkpad_acpi, for instance, publishes pwm1/pwm1_enable at
                // 0644 and then answers -EPERM unless it was loaded with
                // fan_control=1. Read back rather than claim success.
                if !self.took_effect(other) {
                    bail!(
                        "the driver refused every fan write (attributes exist but are not \
                         effective — e.g. thinkpad_acpi needs fan_control=1)"
                    );
                }
            }
        }

        self.inner.lock().await.mode = mode;
        eprintln!("apexd: fan mode -> {mode}");
        Ok(())
    }

    /// Re-read the controls and report whether *anything* actually moved.
    /// Always true in dry-run (nothing was supposed to move) and on the msi-ec
    /// backend when its attributes read back as requested.
    fn took_effect(&self, mode: FanMode) -> bool {
        if !self.writer.is_live() {
            return true;
        }
        let want_pwm = match mode {
            FanMode::Max => Some(255u8),
            FanMode::Manual(p) => Some(p.clamp(self.cfg.min_pwm, self.cfg.max_pwm.max(self.cfg.min_pwm))),
            _ => None,
        };
        for c in &self.inv.controls {
            let enable = c.enable_path.as_deref().and_then(read_u8);
            match mode {
                // Auto: anything other than "manual" means the firmware has it.
                FanMode::Auto => {
                    if enable.map(|e| e != 1).unwrap_or(false) {
                        return true;
                    }
                }
                _ => {
                    if read_u8(&c.pwm_path) == want_pwm {
                        return true;
                    }
                }
            }
        }
        if let Some(ec) = &self.inv.msi_ec {
            let boost = ec
                .cooler_boost_path
                .as_deref()
                .and_then(read_str)
                .unwrap_or_default();
            let want_boost = matches!(mode, FanMode::Max);
            if (boost == "on") == want_boost && !boost.is_empty() {
                return true;
            }
            if ec.fan_mode_path.as_deref().and_then(read_str).is_some() {
                return true;
            }
        }
        false
    }

    /// Put the fans back exactly as they were before apexd first touched them.
    /// A no-op when apexd never touched them. Safe to call repeatedly.
    pub async fn restore(self: &Arc<Self>) {
        let (snapshot, had_curve) = {
            let mut inner = self.inner.lock().await;
            let curve = inner.curve.take();
            if let Some(h) = &curve {
                h.abort();
            }
            (inner.snapshot.clone(), curve.is_some())
        };
        let _ = had_curve;
        let Some(snapshot) = snapshot else {
            return; // never mutated -> nothing to undo
        };
        self.apply(&snapshot.plan_restore());
        let mut inner = self.inner.lock().await;
        inner.mode = FanMode::Auto;
        eprintln!("apexd: fans restored to their pre-apexd state");
    }

    /// Apply actions, logging and continuing on failure. A half-applied fan
    /// plan is worse than a fully-attempted one — especially on restore.
    fn apply(&self, actions: &[apexd_core::tier::Action]) {
        for a in actions {
            if let Err(e) = self.writer.apply(a) {
                eprintln!("apexd: fan action failed ({}): {e:#}", a.describe());
            }
        }
    }

    async fn start_curve(self: &Arc<Self>) {
        let inv = self.inv.clone();
        let cfg = self.cfg.clone();
        let writer = self.writer.clone();
        let sys_root = self.sys_root.clone();
        let interval = std::time::Duration::from_secs(cfg.curve_interval_secs.max(1));
        let handle = tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            let mut last: Option<u8> = None;
            loop {
                ticker.tick().await;
                let Some(temp) = fan::read_curve_temp(&sys_root) else {
                    continue;
                };
                let pwm = fan::curve_pwm(&cfg.curve, temp, cfg.min_pwm, cfg.max_pwm);
                // Small hysteresis so a wobbling sensor does not thrash the EC.
                if last.map(|l| l.abs_diff(pwm) < 4).unwrap_or(false) {
                    continue;
                }
                last = Some(pwm);
                for a in fan::plan_mode(&inv, &cfg, FanMode::Manual(pwm)) {
                    if let Err(e) = writer.apply(&a) {
                        eprintln!("apexd: fan curve action failed ({}): {e:#}", a.describe());
                    }
                }
            }
        });
        self.inner.lock().await.curve = Some(handle);
        eprintln!(
            "apexd: fan curve loop started ({}s cadence, {} points)",
            self.cfg.curve_interval_secs,
            self.cfg.curve.len()
        );
    }
}
