//! The frozen `org.apexos.Apexd1` D-Bus surface. Six interfaces share one
//! object path (`/org/apexos/Apexd1`); all state lives in [`Ctx`].
//!
//! Tier IDs are the frozen strings from `apexd_core::Tier` and must never
//! change — the apex-shell `PowerProfileService` and the `apex` CLI both depend
//! on them verbatim.

use std::collections::HashMap;
use std::sync::Arc;

use apexd_core::tier::Tier;
use zbus::message::Header;
use zbus::{interface, Connection, SignalContext};
use zvariant::OwnedValue;

use crate::metrics::Reading;
use crate::polkit::{authorize, ACTION_BATTERY, ACTION_POWER};
use crate::state::Ctx;

/// Well-known bus name.
pub const BUS_NAME: &str = "org.apexos.Apexd1";
/// Shared object path.
pub const OBJECT_PATH: &str = "/org/apexos/Apexd1";

fn to_fdo(e: anyhow::Error) -> zbus::fdo::Error {
    zbus::fdo::Error::Failed(format!("{e:#}"))
}

// ── Power ──────────────────────────────────────────────────────────────────

/// `org.apexos.Apexd1.Power` — the tier engine surface the shell consumes.
pub struct PowerIface {
    pub ctx: Arc<Ctx>,
}

#[interface(name = "org.apexos.Apexd1.Power")]
impl PowerIface {
    /// Current tier ID.
    #[zbus(property)]
    async fn tier(&self) -> String {
        self.ctx.state.lock().await.tier.as_str().to_string()
    }

    /// All tier IDs, highest to lowest (frozen order).
    #[zbus(property)]
    async fn tiers(&self) -> Vec<String> {
        Tier::all_ids()
    }

    /// Whether AC power is currently online.
    #[zbus(property)]
    async fn on_ac_power(&self) -> bool {
        self.ctx.state.lock().await.on_ac
    }

    /// Whether AC/battery auto-switching is enabled.
    #[zbus(property)]
    async fn auto_switch(&self) -> bool {
        self.ctx.state.lock().await.auto_switch
    }

    /// Switch to `tier`. Rejected (InvalidArgs) for an unknown ID.
    async fn set_tier(
        &self,
        tier: String,
        #[zbus(signal_context)] ctxt: SignalContext<'_>,
        #[zbus(connection)] conn: &Connection,
        #[zbus(header)] hdr: Header<'_>,
    ) -> zbus::fdo::Result<()> {
        authorize(conn, &hdr, ACTION_POWER).await?;
        let t: Tier = tier
            .parse()
            .map_err(|e: apexd_core::UnknownTier| zbus::fdo::Error::InvalidArgs(e.to_string()))?;
        self.ctx.apply_tier(t).await.map_err(to_fdo)?;
        PowerIface::tier_changed_signal(&ctxt, t.as_str()).await?;
        self.tier_changed(&ctxt).await?;
        Ok(())
    }

    /// Enable/disable auto-switching. Enabling immediately reconciles to the
    /// AC/battery default.
    async fn set_auto_switch(
        &self,
        enabled: bool,
        #[zbus(signal_context)] ctxt: SignalContext<'_>,
        #[zbus(connection)] conn: &Connection,
        #[zbus(header)] hdr: Header<'_>,
    ) -> zbus::fdo::Result<()> {
        authorize(conn, &hdr, ACTION_POWER).await?;
        {
            let mut st = self.ctx.state.lock().await;
            st.auto_switch = enabled;
        }
        self.auto_switch_changed(&ctxt).await?;
        if enabled {
            let target = self.ctx.auto_target().await;
            self.ctx.apply_tier(target).await.map_err(to_fdo)?;
            PowerIface::tier_changed_signal(&ctxt, target.as_str()).await?;
            self.tier_changed(&ctxt).await?;
        }
        Ok(())
    }

    /// Emitted whenever the active tier changes (D-Bus name `TierChanged`).
    #[zbus(signal, name = "TierChanged")]
    async fn tier_changed_signal(ctxt: &SignalContext<'_>, tier: &str) -> zbus::Result<()>;
}

// ── Battery ──────────────────────────────────────────────────────────────────

/// `org.apexos.Apexd1.Battery` — charge thresholds, travel mode, calibration.
pub struct BatteryIface {
    pub ctx: Arc<Ctx>,
}

#[interface(name = "org.apexos.Apexd1.Battery")]
impl BatteryIface {
    #[zbus(property)]
    async fn charge_start(&self) -> u8 {
        self.ctx.state.lock().await.charge_start
    }

    #[zbus(property)]
    async fn charge_end(&self) -> u8 {
        self.ctx.state.lock().await.charge_stop
    }

    #[zbus(property)]
    async fn travel_mode(&self) -> bool {
        self.ctx.state.lock().await.travel_mode
    }

    #[zbus(property)]
    async fn capacity(&self) -> u8 {
        read_battery_field(&self.ctx, "capacity")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0)
    }

    #[zbus(property)]
    async fn status(&self) -> String {
        read_battery_field(&self.ctx, "status").unwrap_or_else(|| "Unknown".to_string())
    }

    /// Set charge start/stop thresholds (percent).
    async fn set_charge_thresholds(
        &self,
        start: u8,
        end: u8,
        #[zbus(signal_context)] ctxt: SignalContext<'_>,
        #[zbus(connection)] conn: &Connection,
        #[zbus(header)] hdr: Header<'_>,
    ) -> zbus::fdo::Result<()> {
        authorize(conn, &hdr, ACTION_BATTERY).await?;
        if start > end || end > 100 {
            return Err(zbus::fdo::Error::InvalidArgs(format!(
                "invalid thresholds start={start} end={end}"
            )));
        }
        self.ctx
            .set_charge_thresholds(start, end)
            .await
            .map_err(to_fdo)?;
        self.charge_start_changed(&ctxt).await?;
        self.charge_end_changed(&ctxt).await?;
        Ok(())
    }

    /// Toggle travel mode: on = tighten to a storage window (55/60); off =
    /// restore the profile's charge defaults.
    async fn set_travel_mode(
        &self,
        enabled: bool,
        #[zbus(signal_context)] ctxt: SignalContext<'_>,
        #[zbus(connection)] conn: &Connection,
        #[zbus(header)] hdr: Header<'_>,
    ) -> zbus::fdo::Result<()> {
        authorize(conn, &hdr, ACTION_BATTERY).await?;
        if enabled {
            self.ctx.set_charge_thresholds(55, 60).await.map_err(to_fdo)?;
        } else {
            self.ctx.apply_charge_defaults().await.map_err(to_fdo)?;
        }
        {
            let mut st = self.ctx.state.lock().await;
            st.travel_mode = enabled;
        }
        self.travel_mode_changed(&ctxt).await?;
        self.charge_start_changed(&ctxt).await?;
        self.charge_end_changed(&ctxt).await?;
        Ok(())
    }

    /// Begin a battery calibration cycle: raise the charge ceiling to 100 so a
    /// full charge/discharge pass can recalibrate the gauge. (A fuller
    /// automated routine lands with the battery work in a later milestone.)
    async fn calibrate(
        &self,
        #[zbus(connection)] conn: &Connection,
        #[zbus(header)] hdr: Header<'_>,
    ) -> zbus::fdo::Result<()> {
        authorize(conn, &hdr, ACTION_BATTERY).await?;
        self.ctx.set_charge_thresholds(0, 100).await.map_err(to_fdo)?;
        eprintln!("apexd: calibration window opened (0/100); restore thresholds when the cycle completes");
        Ok(())
    }
}

fn read_battery_field(ctx: &Arc<Ctx>, field: &str) -> Option<String> {
    let bat = ctx.fingerprint.batteries.first().cloned().unwrap_or_else(|| "BAT0".to_string());
    std::fs::read_to_string(format!("/sys/class/power_supply/{bat}/{field}"))
        .ok()
        .map(|s| s.trim().to_string())
}

// ── Profile ──────────────────────────────────────────────────────────────────

/// `org.apexos.Apexd1.Profile` — the resolved layered selection (read-only).
pub struct ProfileIface {
    pub ctx: Arc<Ctx>,
}

#[interface(name = "org.apexos.Apexd1.Profile")]
impl ProfileIface {
    #[zbus(property)]
    async fn active(&self) -> String {
        self.ctx.selection.active.clone()
    }

    #[zbus(property)]
    async fn class(&self) -> String {
        self.ctx.selection.class_or_empty().to_string()
    }

    #[zbus(property)]
    async fn device(&self) -> String {
        self.ctx.selection.device_or_empty().to_string()
    }
}

// ── Metrics ──────────────────────────────────────────────────────────────────

/// `org.apexos.Apexd1.Metrics` — the a{sv} telemetry snapshot.
pub struct MetricsIface {
    pub ctx: Arc<Ctx>,
}

#[interface(name = "org.apexos.Apexd1.Metrics")]
impl MetricsIface {
    #[zbus(property)]
    async fn snapshot(&self) -> HashMap<String, OwnedValue> {
        Reading::gather(&self.ctx).await.to_snapshot()
    }
}

// ── Fan (stub, real impl M6) ─────────────────────────────────────────────────

/// `org.apexos.Apexd1.Fan` — declared now, no-op until M6.
pub struct FanIface;

#[interface(name = "org.apexos.Apexd1.Fan")]
impl FanIface {
    #[zbus(property)]
    async fn mode(&self) -> String {
        "auto".to_string()
    }

    /// No-op stub (fan control lands with M6).
    async fn set_mode(&self, _mode: String) -> zbus::fdo::Result<()> {
        Ok(())
    }
}

// ── GameMode (stub, real impl M6) ────────────────────────────────────────────

/// `org.apexos.Apexd1.GameMode` — declared now, no-op until M6.
pub struct GameModeIface;

#[interface(name = "org.apexos.Apexd1.GameMode")]
impl GameModeIface {
    #[zbus(property)]
    async fn active(&self) -> bool {
        false
    }

    /// No-op stub (game orchestration lands with M6).
    async fn set_active(&self, _active: bool) -> zbus::fdo::Result<()> {
        Ok(())
    }
}

// ── emission helpers (used by the AC/battery event loop) ─────────────────────

/// Emit `TierChanged` + the `Tier` property change from outside a D-Bus method
/// (i.e. from the auto-switch loop).
pub async fn emit_tier_changed(conn: &Connection, tier: Tier) -> zbus::Result<()> {
    let iref = conn
        .object_server()
        .interface::<_, PowerIface>(OBJECT_PATH)
        .await?;
    let ctxt = iref.signal_context().clone();
    PowerIface::tier_changed_signal(&ctxt, tier.as_str()).await?;
    iref.get().await.tier_changed(&ctxt).await?;
    Ok(())
}

/// Emit the `OnAcPower` property change from the event loop.
pub async fn emit_ac_changed(conn: &Connection) -> zbus::Result<()> {
    let iref = conn
        .object_server()
        .interface::<_, PowerIface>(OBJECT_PATH)
        .await?;
    let ctxt = iref.signal_context().clone();
    iref.get().await.on_ac_power_changed(&ctxt).await?;
    Ok(())
}
