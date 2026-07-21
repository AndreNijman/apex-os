//! `apex` — the APEX-OS control CLI. A thin client over the frozen
//! `org.apexos.Apexd1` D-Bus API, with read-only local fallbacks (via
//! `apexd-core`) so `fingerprint`, `status`, `profile`, `doctor` and dry-run
//! tier planning work even when `apexd` is not running. Every D-Bus verb
//! degrades gracefully — a clear message, a non-zero exit, never a panic.

mod ops;
mod proxy;

use std::net::{SocketAddr, TcpStream};
use std::path::Path;
use std::time::Duration;

use apexd_core::tier::Tier;
use clap::{Args, Parser, Subcommand};

use crate::ops::LocalView;
use crate::proxy::{connect, daemon_running, BatteryProxy, PowerProxy, ProfileProxy};

#[derive(Parser)]
#[command(name = "apex", version, about = "APEX-OS control CLI")]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Full status: machine, profile, tier, battery.
    Status,
    /// Show the current tier, or switch to `name`.
    Tier { name: Option<String> },
    /// Show the resolved (layered) profile.
    Profile,
    /// Battery: status, charge thresholds, travel mode, calibration.
    Battery(BatteryArgs),
    /// Print the hardware fingerprint and layered profile selection.
    Fingerprint,
    /// Pin the current deployment (ostree admin pin 0).
    Pin,
    /// Roll back to the previous deployment (bootc rollback).
    Rollback,
    /// Update the OS image (bootc upgrade) and firmware (fwupdmgr).
    Update,
    /// Diagnose the power stack.
    Doctor,
    /// Show the booted image and its changelog labels.
    Changelog,
}

#[derive(Args)]
struct BatteryArgs {
    /// Enable travel mode (tighten charge to a storage window).
    #[arg(long)]
    travel: bool,
    /// Set charge start/stop thresholds (percent).
    #[arg(long, num_args = 2, value_names = ["START", "END"])]
    thresholds: Option<Vec<u8>>,
    /// Begin a battery calibration cycle.
    #[arg(long)]
    calibrate: bool,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let code = match cli.command {
        Cmd::Status => cmd_status().await,
        Cmd::Tier { name } => cmd_tier(name).await,
        Cmd::Profile => cmd_profile().await,
        Cmd::Battery(args) => cmd_battery(args).await,
        Cmd::Fingerprint => cmd_fingerprint(),
        Cmd::Pin => ops::pin(),
        Cmd::Rollback => ops::rollback(),
        Cmd::Update => ops::update(),
        Cmd::Doctor => cmd_doctor().await,
        Cmd::Changelog => ops::changelog(),
    };
    std::process::exit(code);
}

fn cmd_fingerprint() -> i32 {
    let v = LocalView::detect();
    print!("{}", ops::render_fingerprint(&v.fingerprint, &v.selection));
    0
}

async fn cmd_status() -> i32 {
    let v = LocalView::detect();
    print!("{}", ops::render_fingerprint(&v.fingerprint, &v.selection));

    let conn = connect().await;
    let running = match &conn {
        Some(c) => daemon_running(c).await,
        None => false,
    };

    if !running {
        println!("\napexd: not running — showing local dry-run view.\n");
        print!("{}", ops::render_tier_plans(v.active_profile()));
        return 0;
    }

    let conn = conn.unwrap();
    println!("\nDaemon (live):");
    if let Ok(p) = PowerProxy::new(&conn).await {
        print_kv("  tier", p.tier().await.ok());
        print_kv(
            "  on AC",
            p.on_ac_power().await.ok().map(|b| b.to_string()),
        );
        print_kv(
            "  auto-switch",
            p.auto_switch().await.ok().map(|b| b.to_string()),
        );
        if let Ok(tiers) = p.tiers().await {
            println!("  tiers        : {}", tiers.join(", "));
        }
    }
    if let Ok(b) = BatteryProxy::new(&conn).await {
        print_kv("  battery", b.status().await.ok());
        print_kv(
            "  capacity",
            b.capacity().await.ok().map(|c| format!("{c}%")),
        );
        if let (Ok(s), Ok(e)) = (b.charge_start().await, b.charge_end().await) {
            println!("  charge       : {s}-{e}");
        }
        print_kv(
            "  travel mode",
            b.travel_mode().await.ok().map(|b| b.to_string()),
        );
    }
    0
}

async fn cmd_tier(name: Option<String>) -> i32 {
    let v = LocalView::detect();
    let conn = connect().await;
    let running = match &conn {
        Some(c) => daemon_running(c).await,
        None => false,
    };

    match name {
        // Query mode.
        None => {
            if running {
                if let Ok(p) = PowerProxy::new(conn.as_ref().unwrap()).await {
                    let cur = p.tier().await.unwrap_or_default();
                    let tiers = p.tiers().await.unwrap_or_else(|_| Tier::all_ids());
                    for t in tiers {
                        println!("{} {}", if t == cur { "*" } else { " " }, t);
                    }
                    return 0;
                }
            }
            println!("apexd not running — tiers (local):");
            for t in Tier::ALL {
                println!("  {} [{}]", t.label(), t.as_str());
            }
            let d = &v.active_profile().defaults;
            println!("  default: AC -> {}, battery -> {}", d.ac, d.battery);
            0
        }
        // Set mode.
        Some(name) => {
            let tier: Tier = match name.parse() {
                Ok(t) => t,
                Err(e) => {
                    eprintln!("apex: {e}");
                    return 2;
                }
            };
            if running {
                match PowerProxy::new(conn.as_ref().unwrap()).await {
                    Ok(p) => match p.set_tier(tier.as_str()).await {
                        Ok(()) => {
                            println!("apex: tier -> {tier}");
                            0
                        }
                        Err(e) => {
                            eprintln!("apex: SetTier failed: {e}");
                            1
                        }
                    },
                    Err(e) => {
                        eprintln!("apex: cannot reach apexd: {e}");
                        1
                    }
                }
            } else {
                eprintln!("apex: apexd not running — cannot apply '{tier}'. Dry-run plan:");
                for a in v.active_profile().plan_tier(tier) {
                    eprintln!("  - {}", a.describe());
                }
                1
            }
        }
    }
}

async fn cmd_profile() -> i32 {
    let v = LocalView::detect();
    let conn = connect().await;
    let running = match &conn {
        Some(c) => daemon_running(c).await,
        None => false,
    };

    if running {
        if let Ok(p) = ProfileProxy::new(conn.as_ref().unwrap()).await {
            println!("active : {}", p.active().await.unwrap_or_default());
            let class = p.class().await.unwrap_or_default();
            let device = p.device().await.unwrap_or_default();
            println!("class  : {}", if class.is_empty() { "(none)" } else { &class });
            println!(
                "device : {}",
                if device.is_empty() { "(none)" } else { &device }
            );
        }
    } else {
        let s = &v.selection;
        println!("active : {}", s.active);
        println!(
            "class  : {}",
            if s.class_or_empty().is_empty() { "(none)" } else { s.class_or_empty() }
        );
        println!(
            "device : {}",
            if s.device_or_empty().is_empty() { "(none)" } else { s.device_or_empty() }
        );
        println!("(apexd not running — resolved locally)");
    }

    let d = &v.active_profile().defaults;
    println!("\ndefaults: AC -> {}, battery -> {}", d.ac, d.battery);
    if let Some(c) = &v.active_profile().charge {
        println!("charge  : {}-{}", c.start, c.stop);
    }
    0
}

async fn cmd_battery(args: BatteryArgs) -> i32 {
    let conn = connect().await;
    let running = match &conn {
        Some(c) => daemon_running(c).await,
        None => false,
    };

    // Mutating verbs require the daemon.
    let mutating = args.travel || args.calibrate || args.thresholds.is_some();
    if mutating && !running {
        eprintln!("apex: apexd not running — cannot change battery settings.");
        return 1;
    }

    if running {
        let conn = conn.as_ref().unwrap();
        if let Ok(b) = BatteryProxy::new(conn).await {
            if let Some(t) = &args.thresholds {
                let (start, end) = (t[0], t[1]);
                return match b.set_charge_thresholds(start, end).await {
                    Ok(()) => {
                        println!("apex: charge thresholds -> {start}-{end}");
                        0
                    }
                    Err(e) => {
                        eprintln!("apex: SetChargeThresholds failed: {e}");
                        1
                    }
                };
            }
            if args.travel {
                return match b.set_travel_mode(true).await {
                    Ok(()) => {
                        println!("apex: travel mode enabled");
                        0
                    }
                    Err(e) => {
                        eprintln!("apex: SetTravelMode failed: {e}");
                        1
                    }
                };
            }
            if args.calibrate {
                return match b.calibrate().await {
                    Ok(()) => {
                        println!("apex: calibration cycle started");
                        0
                    }
                    Err(e) => {
                        eprintln!("apex: Calibrate failed: {e}");
                        1
                    }
                };
            }
            // No flags: show live battery.
            print_kv("battery ", b.status().await.ok());
            print_kv("capacity", b.capacity().await.ok().map(|c| format!("{c}%")));
            if let (Ok(s), Ok(e)) = (b.charge_start().await, b.charge_end().await) {
                println!("charge  : {s}-{e}");
            }
            print_kv(
                "travel  ",
                b.travel_mode().await.ok().map(|b| b.to_string()),
            );
            return 0;
        }
    }

    // Daemon-less read-only view.
    let v = LocalView::detect();
    let bat = v
        .fingerprint
        .batteries
        .first()
        .cloned()
        .unwrap_or_else(|| "BAT0".to_string());
    println!("battery : {}", read_sys(&format!("class/power_supply/{bat}/status")).unwrap_or_else(|| "Unknown".into()));
    println!("capacity: {}%", read_sys(&format!("class/power_supply/{bat}/capacity")).unwrap_or_else(|| "?".into()));
    if let Some(end) = read_sys(&format!("class/power_supply/{bat}/charge_control_end_threshold")) {
        let start = read_sys(&format!("class/power_supply/{bat}/charge_control_start_threshold"))
            .unwrap_or_else(|| "?".into());
        println!("charge  : {start}-{end}");
    }
    println!("(apexd not running — read locally)");
    0
}

async fn cmd_doctor() -> i32 {
    let v = LocalView::detect();
    let conn = connect().await;
    let running = match &conn {
        Some(c) => daemon_running(c).await,
        None => false,
    };

    line(running, "apexd running (owns org.apexos.Apexd1)");
    line(true, &format!("profile resolved: active={} class={} device={}",
        v.selection.active, v.selection.class_or_empty(), v.selection.device_or_empty()));

    let driver = v.fingerprint.cpu.scaling_driver.as_deref().unwrap_or("");
    line(
        v.fingerprint.cpu.amd_pstate() || v.fingerprint.cpu.intel_pstate(),
        &format!("EPP-capable scaling driver ({})", if driver.is_empty() { "none" } else { driver }),
    );
    line(
        Path::new("/sys/firmware/acpi/platform_profile").exists(),
        "ACPI platform_profile present",
    );

    let bat = v.fingerprint.batteries.first().cloned().unwrap_or_else(|| "BAT0".into());
    line(
        Path::new(&format!("/sys/class/power_supply/{bat}/charge_control_end_threshold")).exists(),
        "charge threshold control present",
    );

    if v.selection.device.as_deref() == Some("thinkpad-l16-g2") {
        let rz = which("ryzenadj");
        line(rz, "ryzenadj on PATH (needed for ultra-max EC-defeat loop)");
    }

    let s2idle = read_sys("power/mem_sleep").map(|s| s.contains("[s2idle]")).unwrap_or(false);
    line(s2idle, "s2idle is the active suspend mode");

    let metrics_up = TcpStream::connect_timeout(
        &"127.0.0.1:9723".parse::<SocketAddr>().unwrap(),
        Duration::from_millis(200),
    )
    .is_ok();
    line(metrics_up, "metrics endpoint reachable on 127.0.0.1:9723");

    0
}

// ── small helpers ────────────────────────────────────────────────────────────

fn print_kv(key: &str, val: Option<String>) {
    if let Some(v) = val {
        println!("{key}: {v}");
    }
}

fn line(ok: bool, what: &str) {
    println!("[{}] {}", if ok { "PASS" } else { "WARN" }, what);
}

fn read_sys(rel: &str) -> Option<String> {
    std::fs::read_to_string(format!("/sys/{rel}"))
        .ok()
        .map(|s| s.trim().to_string())
}

fn which(program: &str) -> bool {
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d.join(program).is_file()))
        .unwrap_or(false)
}
