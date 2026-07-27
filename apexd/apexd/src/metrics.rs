//! Best-effort telemetry: a Prometheus text endpoint on 127.0.0.1:9723 and the
//! D-Bus `.Metrics.Snapshot` a{sv}. Everything here reads sysfs read-only and
//! degrades gracefully when a source is absent.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use apexd_core::tier::Tier;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use zvariant::{OwnedValue, Value};

use crate::state::Ctx;

/// The metrics listen address.
pub const METRICS_ADDR: &str = "127.0.0.1:9723";

/// A point-in-time reading of everything we expose.
pub struct Reading {
    pub tier: Tier,
    pub on_ac: bool,
    pub ppt_watts: Option<f64>,
    pub battery_uwh: Option<u64>,
    pub temps: Vec<(String, f64)>,
    /// Whether the daemon is actually applying anything, or only logging.
    pub dry_run: bool,
    /// Machine identity, for the `apexd_machine_info` label set. This is how a
    /// bug report from hardware nobody here owns arrives with enough context to
    /// be actionable.
    pub machine: MachineInfo,
}

/// The label set of the `apexd_machine_info` metric.
pub struct MachineInfo {
    pub vendor: String,
    pub product: String,
    pub cpu_vendor: String,
    pub scaling_driver: String,
    pub profile: String,
    pub batteries: usize,
}

impl Reading {
    pub async fn gather(ctx: &Arc<Ctx>) -> Reading {
        let (tier, on_ac) = {
            let st = ctx.state.lock().await;
            (st.tier, st.on_ac)
        };
        let fp = &ctx.fingerprint;
        Reading {
            tier,
            on_ac,
            dry_run: ctx.dry_run,
            machine: MachineInfo {
                vendor: fp.sys_vendor.clone(),
                product: if fp.product_name.is_empty() {
                    fp.product_version.clone()
                } else {
                    fp.product_name.clone()
                },
                cpu_vendor: fp.cpu.vendor.as_str().to_string(),
                scaling_driver: fp
                    .cpu
                    .scaling_driver
                    .clone()
                    .unwrap_or_else(|| "none".to_string()),
                profile: ctx.selection.active.clone(),
                batteries: ctx.batteries.len(),
            },
            ppt_watts: read_ppt_watts(Path::new("/sys")),
            // Summed over the discovered packs, whatever they are called, and
            // derived from charge x voltage on drivers that report no energy.
            battery_uwh: ctx.batteries.energy_uwh(),
            temps: read_temps(Path::new("/sys")),
        }
    }

    /// The Prometheus text exposition.
    pub fn to_prometheus(&self) -> String {
        let mut out = String::new();
        out.push_str("# HELP apexd_tier Active power tier (1 for the active tier).\n");
        out.push_str("# TYPE apexd_tier gauge\n");
        for t in Tier::ALL {
            let v = if t == self.tier { 1 } else { 0 };
            out.push_str(&format!("apexd_tier{{tier=\"{}\"}} {}\n", t.as_str(), v));
        }

        out.push_str("# HELP apexd_ac_online Whether AC power is online.\n");
        out.push_str("# TYPE apexd_ac_online gauge\n");
        out.push_str(&format!("apexd_ac_online {}\n", if self.on_ac { 1 } else { 0 }));

        out.push_str("# HELP apexd_dry_run Whether apexd is logging intent instead of writing hardware.\n");
        out.push_str("# TYPE apexd_dry_run gauge\n");
        out.push_str(&format!("apexd_dry_run {}\n", if self.dry_run { 1 } else { 0 }));

        let m = &self.machine;
        out.push_str("# HELP apexd_machine_info Detected machine, always 1; the labels carry the detail.\n");
        out.push_str("# TYPE apexd_machine_info gauge\n");
        out.push_str(&format!(
            "apexd_machine_info{{vendor=\"{}\",product=\"{}\",cpu_vendor=\"{}\",scaling_driver=\"{}\",profile=\"{}\",batteries=\"{}\"}} 1\n",
            escape_label(&m.vendor),
            escape_label(&m.product),
            escape_label(&m.cpu_vendor),
            escape_label(&m.scaling_driver),
            escape_label(&m.profile),
            m.batteries,
        ));

        if let Some(w) = self.ppt_watts {
            out.push_str("# HELP apexd_ppt_watts Package power draw in watts.\n");
            out.push_str("# TYPE apexd_ppt_watts gauge\n");
            out.push_str(&format!("apexd_ppt_watts {w:.3}\n"));
        }

        if let Some(uwh) = self.battery_uwh {
            out.push_str("# HELP apexd_battery_uwh Battery energy in microwatt-hours.\n");
            out.push_str("# TYPE apexd_battery_uwh gauge\n");
            out.push_str(&format!("apexd_battery_uwh {uwh}\n"));
        }

        if !self.temps.is_empty() {
            out.push_str("# HELP apexd_temp_celsius Thermal zone temperature in celsius.\n");
            out.push_str("# TYPE apexd_temp_celsius gauge\n");
            for (zone, c) in &self.temps {
                out.push_str(&format!(
                    "apexd_temp_celsius{{zone=\"{}\"}} {c:.1}\n",
                    escape_label(zone)
                ));
            }
        }
        out
    }

    /// The D-Bus `a{sv}` snapshot.
    pub fn to_snapshot(&self) -> HashMap<String, OwnedValue> {
        let mut m: HashMap<String, OwnedValue> = HashMap::new();
        insert(&mut m, "tier", Value::from(self.tier.as_str()));
        insert(&mut m, "on_ac", Value::from(self.on_ac));
        if let Some(w) = self.ppt_watts {
            insert(&mut m, "ppt_watts", Value::from(w));
        }
        if let Some(uwh) = self.battery_uwh {
            insert(&mut m, "battery_uwh", Value::from(uwh));
        }
        for (zone, c) in &self.temps {
            insert(&mut m, &format!("temp_{zone}"), Value::from(*c));
        }
        m
    }
}

/// Escape a Prometheus label value. DMI strings are attacker-adjacent free text
/// (`product_name` is whatever the vendor wrote), so a stray quote, backslash
/// or newline must not be able to corrupt the exposition format.
fn escape_label(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            other => out.push(other),
        }
    }
    out
}

fn insert(m: &mut HashMap<String, OwnedValue>, key: &str, v: Value<'_>) {
    if let Ok(owned) = v.try_to_owned() {
        m.insert(key.to_string(), owned);
    }
}

/// Best-effort package power in watts, from any hwmon `power1_average` (µW).
fn read_ppt_watts(sys_root: &Path) -> Option<f64> {
    let base = sys_root.join("class/hwmon");
    let entries = std::fs::read_dir(&base).ok()?;
    for e in entries.flatten() {
        let uw = e.path().join("power1_average");
        if let Ok(s) = std::fs::read_to_string(&uw) {
            if let Ok(v) = s.trim().parse::<f64>() {
                return Some(v / 1_000_000.0);
            }
        }
    }
    None
}

/// All thermal-zone temperatures, keyed by zone `type`.
fn read_temps(sys_root: &Path) -> Vec<(String, f64)> {
    let base = sys_root.join("class/thermal");
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(&base) else {
        return out;
    };
    let mut dirs: Vec<_> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|s| s.to_str())
                .map(|s| s.starts_with("thermal_zone"))
                .unwrap_or(false)
        })
        .collect();
    dirs.sort();
    for d in dirs {
        let zone = std::fs::read_to_string(d.join("type"))
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|_| d.file_name().unwrap().to_string_lossy().to_string());
        if let Ok(s) = std::fs::read_to_string(d.join("temp")) {
            if let Ok(milli) = s.trim().parse::<f64>() {
                out.push((zone, milli / 1000.0));
            }
        }
    }
    out
}

/// Serve the Prometheus endpoint until the process exits. Best-effort: a bind
/// failure is logged and the task returns (metrics are non-critical).
pub async fn serve(ctx: Arc<Ctx>) {
    let listener = match TcpListener::bind(METRICS_ADDR).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("apexd: metrics endpoint disabled (bind {METRICS_ADDR}: {e})");
            return;
        }
    };
    eprintln!("apexd: metrics on http://{METRICS_ADDR}/metrics");
    loop {
        let (mut sock, _) = match listener.accept().await {
            Ok(pair) => pair,
            Err(e) => {
                eprintln!("apexd: metrics accept error: {e}");
                continue;
            }
        };
        let ctx = ctx.clone();
        tokio::spawn(async move {
            // Drain the request line(s); we ignore the path and always answer.
            let mut buf = [0u8; 1024];
            let _ = sock.read(&mut buf).await;
            let body = Reading::gather(&ctx).await.to_prometheus();
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/plain; version=0.0.4\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = sock.write_all(resp.as_bytes()).await;
            let _ = sock.flush().await;
        });
    }
}
