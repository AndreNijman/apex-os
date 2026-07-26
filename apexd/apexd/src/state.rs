//! Daemon runtime state and the logic that turns tier changes into writer
//! actions, including the gated RyzenAdj reapply loop.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use apexd_core::gpu::NvidiaSmi;
use apexd_core::profile::Profile;
use apexd_core::syswriter::SysWriter;
use apexd_core::tier::{Action, Tier};
use apexd_core::{ProfileSet, Selection};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

use crate::fan::FanController;
use crate::game::GameSession;

/// Mutable daemon state (behind an async mutex).
#[derive(Debug, Clone)]
pub struct State {
    pub tier: Tier,
    pub auto_switch: bool,
    pub on_ac: bool,
    pub travel_mode: bool,
    pub charge_start: u8,
    pub charge_stop: u8,
}

/// Everything the interfaces and loops share. Immutable config lives inline;
/// mutable bits live behind `state`.
pub struct Ctx {
    pub set: ProfileSet,
    pub selection: Selection,
    pub fingerprint: apexd_core::Fingerprint,
    pub writer: Arc<dyn SysWriter>,
    pub dry_run: bool,
    /// True when the resolved device profile is the L16 (the only machine the
    /// RyzenAdj loop is allowed to touch).
    pub device_is_l16: bool,
    /// True when `ryzenadj` is on PATH.
    pub ryzenadj_present: bool,
    /// The sysfs root everything reads from (parameterised for fixtures).
    pub sys_root: PathBuf,
    /// M6: fan discovery, mode state and the restore path.
    pub fan: Arc<FanController>,
    /// M6: read-side access to `nvidia-smi`.
    pub nvidia: Arc<dyn NvidiaSmi>,
    /// M6: the active game session, if any.
    pub game: Mutex<Option<GameSession>>,
    pub state: Mutex<State>,
    ryzenadj_loop: Mutex<Option<JoinHandle<()>>>,
}

impl Ctx {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        set: ProfileSet,
        selection: Selection,
        fingerprint: apexd_core::Fingerprint,
        writer: Arc<dyn SysWriter>,
        dry_run: bool,
        initial: State,
        sys_root: impl Into<PathBuf>,
        nvidia: Arc<dyn NvidiaSmi>,
    ) -> Arc<Ctx> {
        let device_is_l16 = selection.device.as_deref() == Some("thinkpad-l16-g2");
        let sys_root = sys_root.into();
        let fan_cfg = set
            .get(&selection.active)
            .map(|p| p.fan_config())
            .unwrap_or_default();
        let fan = FanController::new(sys_root.clone(), fan_cfg, writer.clone());
        Arc::new(Ctx {
            set,
            selection,
            fingerprint,
            writer,
            dry_run,
            device_is_l16,
            ryzenadj_present: ryzenadj_available(),
            sys_root,
            fan,
            nvidia,
            game: Mutex::new(None),
            state: Mutex::new(initial),
            ryzenadj_loop: Mutex::new(None),
        })
    }

    /// The active profile.
    pub fn profile(&self) -> &Profile {
        self.set
            .get(&self.selection.active)
            .expect("active profile always present")
    }

    /// Whether the gated RyzenAdj reapply loop may run at all on this machine.
    /// All three conditions must hold: it's the L16, ryzenadj is installed, and
    /// we're not in dry-run.
    pub fn ryzenadj_allowed(&self) -> bool {
        self.device_is_l16 && self.ryzenadj_present && !self.dry_run
    }

    /// Apply a tier: update state, apply the transition plan through the
    /// writer, and start/stop the RyzenAdj loop as the tier requires. Returns
    /// the previous tier (or None if unchanged is fine to re-apply).
    pub async fn apply_tier(self: &Arc<Self>, tier: Tier) -> Result<Option<Tier>> {
        let prev = {
            let mut st = self.state.lock().await;
            let prev = st.tier;
            st.tier = tier;
            Some(prev)
        };

        let profile = self.profile();
        let plan = profile.plan_transition(prev, tier);

        // Apply non-ryzenadj actions synchronously through the writer. The
        // RyzenAdj action itself is handled by the loop below (so we don't fire
        // a single stray invocation here).
        for action in &plan {
            match action {
                Action::RyzenAdj { .. } => {} // handled by the loop
                other => self.writer.apply(other)?,
            }
        }

        // Manage the reapply loop.
        let wants_ryzenadj = profile
            .ryzenadj
            .as_ref()
            .map(|rz| rz.applies_to(tier))
            .unwrap_or(false);

        if wants_ryzenadj && self.ryzenadj_allowed() {
            self.start_ryzenadj_loop().await;
        } else {
            self.stop_ryzenadj_loop().await;
        }

        Ok(prev)
    }

    /// Compute the tier the auto-switch policy wants for the current AC state.
    pub async fn auto_target(&self) -> Tier {
        let on_ac = self.state.lock().await.on_ac;
        let d = &self.profile().defaults;
        if on_ac {
            d.ac
        } else {
            d.battery
        }
    }

    /// Apply the battery charge thresholds the active profile declares (if
    /// any), recording them in state.
    pub async fn apply_charge_defaults(self: &Arc<Self>) -> Result<()> {
        if let Some(action) = self.profile().charge_action() {
            if let Action::ChargeThresholds { start, stop, .. } = &action {
                let mut st = self.state.lock().await;
                st.charge_start = *start;
                st.charge_stop = *stop;
            }
            self.writer.apply(&action)?;
        }
        Ok(())
    }

    /// Set explicit charge thresholds (from the D-Bus method).
    pub async fn set_charge_thresholds(self: &Arc<Self>, start: u8, stop: u8) -> Result<()> {
        // Reuse the profile's sysfs paths where known, else the BAT0 defaults.
        let (start_path, end_path) = match &self.profile().charge {
            Some(c) => (c.start_path.clone(), c.end_path.clone()),
            None => (
                "/sys/class/power_supply/BAT0/charge_control_start_threshold".to_string(),
                "/sys/class/power_supply/BAT0/charge_control_end_threshold".to_string(),
            ),
        };
        self.writer.apply(&Action::ChargeThresholds {
            start,
            stop,
            start_path,
            end_path,
        })?;
        let mut st = self.state.lock().await;
        st.charge_start = start;
        st.charge_stop = stop;
        Ok(())
    }

    async fn start_ryzenadj_loop(self: &Arc<Self>) {
        let mut guard = self.ryzenadj_loop.lock().await;
        if guard.is_some() {
            return; // already running
        }
        let Some(rz) = self.profile().ryzenadj.clone() else {
            return;
        };
        let (stapm, fast, slow) = rz.clamped();
        let action = Action::RyzenAdj {
            stapm_mw: stapm,
            fast_mw: fast,
            slow_mw: slow,
            tctl_max: rz.tctl_max,
        };
        let writer = self.writer.clone();
        let interval = std::time::Duration::from_secs(rz.interval_secs.max(1));
        let handle = tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            loop {
                ticker.tick().await;
                if let Err(e) = writer.apply(&action) {
                    eprintln!("apexd: ryzenadj reapply failed: {e:#}");
                }
            }
        });
        *guard = Some(handle);
        eprintln!("apexd: ryzenadj reapply loop started ({}s cadence)", rz.interval_secs);
    }

    async fn stop_ryzenadj_loop(self: &Arc<Self>) {
        let mut guard = self.ryzenadj_loop.lock().await;
        if let Some(handle) = guard.take() {
            handle.abort();
            let _ = self.writer.apply(&Action::StopRyzenAdj);
            eprintln!("apexd: ryzenadj reapply loop torn down");
        }
    }
}

/// True when `ryzenadj` is resolvable on PATH.
fn ryzenadj_available() -> bool {
    std::env::var_os("PATH")
        .map(|p| {
            std::env::split_paths(&p).any(|dir| dir.join("ryzenadj").is_file())
        })
        .unwrap_or(false)
}

/// Read `<power_supply>/AC-ish/online`. Returns true if any Mains supply is
/// online. Read-only.
pub fn read_ac_online(sys_root: &Path) -> bool {
    let dir = sys_root.join("class/power_supply");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return true; // assume AC if we cannot tell (desktop-safe default)
    };
    let mut saw_mains = false;
    for e in entries.flatten() {
        let p = e.path();
        let ty = std::fs::read_to_string(p.join("type"))
            .unwrap_or_default()
            .trim()
            .to_string();
        if ty == "Mains" {
            saw_mains = true;
            if std::fs::read_to_string(p.join("online"))
                .map(|s| s.trim() == "1")
                .unwrap_or(false)
            {
                return true;
            }
        }
    }
    // If there is a Mains supply and none reported online, we're on battery.
    !saw_mains
}
