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
}

impl Reading {
    pub async fn gather(ctx: &Arc<Ctx>) -> Reading {
        let (tier, on_ac) = {
            let st = ctx.state.lock().await;
            (st.tier, st.on_ac)
        };
        Reading {
            tier,
            on_ac,
            ppt_watts: read_ppt_watts(Path::new("/sys")),
            battery_uwh: read_battery_uwh(Path::new("/sys")),
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
                out.push_str(&format!("apexd_temp_celsius{{zone=\"{zone}\"}} {c:.1}\n"));
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

/// Battery energy in µWh, from BAT*/energy_now.
fn read_battery_uwh(sys_root: &Path) -> Option<u64> {
    let base = sys_root.join("class/power_supply");
    let entries = std::fs::read_dir(&base).ok()?;
    for e in entries.flatten() {
        let name = e.file_name().into_string().unwrap_or_default();
        if !name.starts_with("BAT") {
            continue;
        }
        if let Ok(s) = std::fs::read_to_string(e.path().join("energy_now")) {
            if let Ok(v) = s.trim().parse::<u64>() {
                return Some(v);
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
