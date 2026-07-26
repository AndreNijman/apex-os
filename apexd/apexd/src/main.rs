//! `apexd` — the APEX-OS power daemon.
//!
//! Detects the machine, selects a layered profile, exposes the frozen
//! `org.apexos.Apexd1` D-Bus surface, auto-switches tiers on AC/battery
//! transitions, runs the gated RyzenAdj reapply loop, and serves Prometheus
//! metrics. Never writes hardware when `APEXD_DRY_RUN=1` (or `--dry-run`).

mod dbus;
mod fan;
mod game;
mod metrics;
mod polkit;
mod state;

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use apexd_core::gpu::{NvidiaSmi, RealNvidiaSmi};
use apexd_core::syswriter::{RealWriter, SysWriter};
use apexd_core::{select, Fingerprint, ProfileSet};

use crate::dbus::{
    BatteryIface, FanIface, GameModeIface, MetricsIface, PowerIface, ProfileIface, BUS_NAME,
    OBJECT_PATH,
};
use crate::state::{read_ac_online, Ctx, State};

#[tokio::main]
async fn main() -> Result<()> {
    let dry_run = apexd_core::dry_run_from_env() || std::env::args().any(|a| a == "--dry-run");

    // Detect + select (read-only).
    let fingerprint = Fingerprint::detect();
    let profiles = ProfileSet::load(Some(Path::new(apexd_core::PROFILE_DIR)))
        .context("loading system profiles")?;
    let selection = select(&fingerprint, &profiles);

    eprintln!(
        "apexd: {} / {} — profile active={} class={} device={} (dry_run={})",
        fingerprint.sys_vendor,
        fingerprint.product_version,
        selection.active,
        selection.class_or_empty(),
        selection.device_or_empty(),
        dry_run
    );

    // Writer: real sysfs, gated by dry-run.
    let writer: Arc<dyn SysWriter> = Arc::new(RealWriter::new(dry_run));

    // Initial state.
    let on_ac = read_ac_online(Path::new("/sys"));
    let profile = profiles
        .get(&selection.active)
        .context("active profile missing from set")?;
    let (charge_start, charge_stop) = profile
        .charge
        .as_ref()
        .map(|c| (c.start, c.stop))
        .unwrap_or((0, 100));
    let initial_tier = if on_ac {
        profile.defaults.ac
    } else {
        profile.defaults.battery
    };
    let initial = State {
        tier: initial_tier,
        auto_switch: true,
        on_ac,
        travel_mode: false,
        charge_start,
        charge_stop,
    };

    let nvidia: Arc<dyn NvidiaSmi> = Arc::new(RealNvidiaSmi);
    let ctx = Ctx::new(
        profiles,
        selection,
        fingerprint,
        writer,
        dry_run,
        initial,
        Path::new("/sys"),
        nvidia,
    );

    if ctx.device_is_l16 && !ctx.ryzenadj_present {
        eprintln!("apexd: note: L16 detected but ryzenadj not on PATH — ultra-max EC-defeat loop disabled");
    }
    if ctx.fan.supported() {
        eprintln!("apexd: fan control: {}", ctx.fan.backends().join("; "));
    } else {
        eprintln!("apexd: fan control: no controllable fan found (reporting unsupported)");
    }

    // Bring hardware to the initial state (charge thresholds + tier + fan).
    ctx.apply_charge_defaults().await.ok();
    ctx.apply_tier(initial_tier).await.ok();
    ctx.fan.apply_default().await;

    // Build the D-Bus service: six interfaces on one path.
    let conn = zbus::connection::Builder::system()
        .context("connecting to the system bus")?
        .name(BUS_NAME)
        .context("claiming bus name")?
        .serve_at(OBJECT_PATH, PowerIface { ctx: ctx.clone() })?
        .serve_at(OBJECT_PATH, BatteryIface { ctx: ctx.clone() })?
        .serve_at(OBJECT_PATH, ProfileIface { ctx: ctx.clone() })?
        .serve_at(OBJECT_PATH, MetricsIface { ctx: ctx.clone() })?
        .serve_at(OBJECT_PATH, FanIface { ctx: ctx.clone() })?
        .serve_at(OBJECT_PATH, GameModeIface { ctx: ctx.clone() })?
        .build()
        .await
        .context("building the D-Bus service")?;

    eprintln!("apexd: serving {BUS_NAME} at {OBJECT_PATH}");

    // Metrics endpoint.
    tokio::spawn(metrics::serve(ctx.clone()));

    // AC/battery poll loop.
    tokio::spawn(ac_event_loop(ctx.clone(), conn.clone()));

    // Run until told to stop, then unwind in the reverse order of set-up.
    wait_for_shutdown().await;
    eprintln!("apexd: shutting down");
    // 1. Leave game mode: releases the GPU clock locks, the IRQ affinities and
    //    the cpuset, and restores the tier the session interrupted.
    ctx.game_exit().await.ok();
    // 2. Hand the fans back to the firmware. This must happen before the
    //    process can exit for any *graceful* reason; a crash is covered by
    //    `ExecStopPost=/usr/bin/apex fan restore --local` in apexd.service.
    ctx.fan.restore().await;
    // 3. Dropping ctx aborts the ryzenadj task; make it explicit by switching to
    //    a non-ryzenadj tier so the writer records the teardown too.
    ctx.apply_tier(apexd_core::Tier::Balanced).await.ok();
    Ok(())
}

/// Poll AC state; on a transition, update state, emit the property change, and
/// (when auto-switch is on) reconcile the tier.
async fn ac_event_loop(ctx: Arc<Ctx>, conn: zbus::Connection) {
    let mut ticker = tokio::time::interval(Duration::from_secs(2));
    loop {
        ticker.tick().await;
        let now = read_ac_online(Path::new("/sys"));
        let (changed, auto) = {
            let mut st = ctx.state.lock().await;
            let changed = st.on_ac != now;
            st.on_ac = now;
            (changed, st.auto_switch)
        };
        if !changed {
            continue;
        }
        if let Err(e) = dbus::emit_ac_changed(&conn).await {
            eprintln!("apexd: emit OnAcPower failed: {e}");
        }
        if auto {
            let target = ctx.auto_target().await;
            if let Err(e) = ctx.apply_tier(target).await {
                eprintln!("apexd: auto-switch apply failed: {e:#}");
                continue;
            }
            if let Err(e) = dbus::emit_tier_changed(&conn, target).await {
                eprintln!("apexd: emit TierChanged failed: {e}");
            }
            eprintln!("apexd: AC {} -> tier {}", if now { "on" } else { "off" }, target);
        }
    }
}

/// Resolve on SIGINT or SIGTERM.
async fn wait_for_shutdown() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut term = match signal(SignalKind::terminate()) {
            Ok(s) => s,
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
                return;
            }
        };
        tokio::select! {
            _ = term.recv() => {}
            _ = tokio::signal::ctrl_c() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
