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
use crate::proxy::{
    connect, daemon_running, BatteryProxy, FanProxy, GameModeProxy, PowerProxy, ProfileProxy,
};

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
    /// Fans: report speeds, switch mode, restore firmware control.
    Fan {
        #[command(subcommand)]
        cmd: Option<FanCmd>,
    },
    /// Game mode: P-core pinning, IRQ steering, GPU clock locks, top tier.
    Game {
        #[command(subcommand)]
        cmd: GameCmd,
    },
    /// Print the hardware fingerprint and layered profile selection.
    Fingerprint,
    /// Pin the current deployment (ostree admin pin 0). Requires root.
    Pin,
    /// Roll back to the previous deployment (bootc rollback). Requires root.
    Rollback,
    /// Update the OS image (bootc upgrade) and firmware (fwupdmgr). Requires root.
    Update(UpdateArgs),
    /// Diagnose the power stack.
    Doctor,
    /// Show the booted image and its changelog labels.
    Changelog,
    /// Install packages from the Fedora repositories. Requires root.
    ///
    /// Packages go into a systemd system extension, NOT an rpm-ostree layer, so
    /// the OS keeps updating normally and `apex rollback` still works.
    Install {
        #[arg(required = true, value_name = "PACKAGE")]
        packages: Vec<String>,
        /// Skip weak dependencies (smaller install, fewer optional features).
        #[arg(long)]
        no_weak_deps: bool,
        /// Also consider a repository that is disabled by default.
        #[arg(long, value_name = "REPO")]
        enable_repo: Vec<String>,
    },
    /// Remove packages installed with `apex install`. Requires root.
    Remove {
        #[arg(required = true, value_name = "PACKAGE")]
        packages: Vec<String>,
    },
    /// Search the Fedora repositories.
    Search {
        #[arg(required = true, value_name = "TERM")]
        terms: Vec<String>,
    },
    /// Manage installed packages: list, status, rebuild, rollback, adopt.
    Pkg {
        #[command(subcommand)]
        cmd: PkgCmd,
    },
}

#[derive(Subcommand)]
enum PkgCmd {
    /// What is installed, and what came in as a dependency.
    List,
    /// Extension state: what it was built for, whether it is merged.
    Status,
    /// The full machine-readable record of the last build.
    Info,
    /// Re-resolve every package against the repositories. Requires root.
    Upgrade,
    /// Rebuild for the running OS version. Requires root.
    Rebuild {
        /// Do nothing unless the extension no longer matches the booted OS.
        #[arg(long)]
        if_needed: bool,
    },
    /// Restore the previous extension. Requires root.
    Rollback,
    /// Check the installed extension against its recorded checksum.
    Verify,
    /// Convert rpm-ostree layered packages into APEX packages, so that OS
    /// updates work again without losing the software. Requires root.
    Adopt,
}

#[derive(Subcommand)]
enum FanCmd {
    /// Show every discovered fan, the active mode and the supported modes.
    Status,
    /// Switch mode: auto, max, manual, manual:<0-255> or curve.
    Mode { name: String },
    /// Manual mode at an explicit duty cycle (0-255).
    Pwm { value: u8 },
    /// Hand the fans back to firmware control.
    Restore {
        /// Write sysfs directly instead of going through apexd. This is the
        /// crash-safety path (`ExecStopPost=`) and needs root, not the daemon.
        #[arg(long)]
        local: bool,
    },
}

#[derive(Subcommand)]
enum GameCmd {
    /// Enter game mode, optionally pinning a process (and its children).
    Start {
        /// PID to move into the game cpuset.
        #[arg(long)]
        pid: Option<u32>,
    },
    /// Leave game mode, restoring everything it changed.
    Stop,
    /// Show the session (or what one would look like).
    Status,
    /// Attach another PID to a running session.
    Attach { pid: u32 },
}

#[derive(Args)]
struct UpdateArgs {
    /// Report what is available without downloading or staging anything.
    #[arg(long)]
    check: bool,
    /// Skip the firmware (fwupd) pass.
    #[arg(long)]
    skip_firmware: bool,
    /// Only run the firmware pass; leave the OS image alone.
    #[arg(long, conflicts_with = "skip_firmware")]
    firmware_only: bool,
    /// Skip refreshing packages installed with `apex install`.
    #[arg(long)]
    skip_packages: bool,
    /// Keep ostree's per-object fsync on during the pull. Roughly halves update
    /// speed (measured: ~8 MiB/s with it, ~14.6 without, because 179k objects at
    /// 2.98 ms of fsync each outweighs the download itself) in exchange for
    /// durability if the machine loses power mid-update.
    #[arg(long)]
    fsync: bool,
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

    // Root-only verbs bail HERE — before any sysfs probe, D-Bus connect or
    // subprocess — so an unprivileged `apex update` costs nothing and answers
    // instantly with the command to run instead. See ops::require_root for why
    // this covers exactly these four and not the whole CLI (the desktop's power
    // tab drives `apex tier` as the session user).
    let privileged = match &cli.command {
        Cmd::Update(_) => Some("update"),
        Cmd::Rollback => Some("rollback"),
        Cmd::Pin => Some("pin"),
        // Package verbs that write: they build an extension into /var/lib and
        // ask systemd to re-merge /usr. The read-only ones (list/status/info/
        // verify) and `search` stay usable as an ordinary user on purpose.
        Cmd::Install { .. } => Some("install"),
        Cmd::Remove { .. } => Some("remove"),
        Cmd::Pkg {
            cmd: PkgCmd::Upgrade,
        } => Some("pkg upgrade"),
        Cmd::Pkg {
            cmd: PkgCmd::Rebuild { .. },
        } => Some("pkg rebuild"),
        Cmd::Pkg {
            cmd: PkgCmd::Rollback,
        } => Some("pkg rollback"),
        Cmd::Pkg {
            cmd: PkgCmd::Adopt,
        } => Some("pkg adopt"),
        // `fan restore --local` writes sysfs directly instead of asking apexd;
        // it is the crash-safety path (ExecStopPost=) and needs real privileges.
        // Every other fan verb goes through the daemon and must stay usable.
        Cmd::Fan {
            cmd: Some(FanCmd::Restore { local: true }),
        } => Some("fan restore --local"),
        _ => None,
    };
    if let Some(verb) = privileged {
        if let Err(code) = ops::require_root(verb) {
            std::process::exit(code);
        }
    }

    let code = match cli.command {
        Cmd::Status => cmd_status().await,
        Cmd::Tier { name } => cmd_tier(name).await,
        Cmd::Profile => cmd_profile().await,
        Cmd::Battery(args) => cmd_battery(args).await,
        Cmd::Fan { cmd } => cmd_fan(cmd.unwrap_or(FanCmd::Status)).await,
        Cmd::Game { cmd } => cmd_game(cmd).await,
        Cmd::Fingerprint => cmd_fingerprint(),
        Cmd::Pin => ops::pin(),
        Cmd::Rollback => ops::rollback(),
        Cmd::Update(args) => ops::update(ops::UpdateOptions {
            check: args.check,
            skip_firmware: args.skip_firmware,
            firmware_only: args.firmware_only,
            keep_fsync: args.fsync,
            skip_packages: args.skip_packages,
        }),
        Cmd::Doctor => cmd_doctor().await,
        Cmd::Changelog => ops::changelog(),
        Cmd::Install {
            packages,
            no_weak_deps,
            enable_repo,
        } => {
            let mut argv = vec!["install".to_string()];
            argv.extend(packages);
            if no_weak_deps {
                argv.push("--no-weak-deps".to_string());
            }
            for repo in enable_repo {
                argv.push(format!("--enable-repo={repo}"));
            }
            ops::pkg(&argv)
        }
        Cmd::Remove { packages } => {
            let mut argv = vec!["remove".to_string()];
            argv.extend(packages);
            ops::pkg(&argv)
        }
        Cmd::Search { terms } => {
            let mut argv = vec!["search".to_string()];
            argv.extend(terms);
            ops::pkg(&argv)
        }
        Cmd::Pkg { cmd } => {
            let argv: Vec<String> = match cmd {
                PkgCmd::List => vec!["list".into()],
                PkgCmd::Status => vec!["status".into()],
                PkgCmd::Info => vec!["info".into()],
                PkgCmd::Upgrade => vec!["upgrade".into()],
                PkgCmd::Rebuild { if_needed } => {
                    let mut a = vec!["rebuild".to_string()];
                    if if_needed {
                        a.push("--if-needed".into());
                    }
                    a
                }
                PkgCmd::Rollback => vec!["rollback".into()],
                PkgCmd::Verify => vec!["verify".into()],
                PkgCmd::Adopt => vec!["adopt".into()],
            };
            ops::pkg(&argv)
        }
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

    // Daemon-less read-only view, against whatever batteries this machine has.
    let inv = apexd_core::BatteryInventory::detect();
    let Some(bat) = inv.primary() else {
        println!("battery : (none — this machine has no battery)");
        println!("(apexd not running — read locally)");
        return 0;
    };
    println!("battery : {}", bat.read("status").unwrap_or_else(|| "Unknown".into()));
    println!("capacity: {}%", bat.read("capacity").unwrap_or_else(|| "?".into()));
    if inv.len() > 1 {
        println!("packs   : {}", inv.names().join(", "));
    }
    for b in &inv.batteries {
        let end = b.end_path.as_deref().and_then(read_abs);
        let start = b.start_path.as_deref().and_then(read_abs);
        match (start, end) {
            (Some(s), Some(e)) => println!("charge  : {} {s}-{e}", b.name),
            (None, Some(e)) => println!("charge  : {} stop at {e} (no start threshold)", b.name),
            _ => {}
        }
    }
    if !inv.supports_thresholds() {
        println!("charge  : not supported on this hardware");
    }
    println!("(apexd not running — read locally)");
    0
}

async fn cmd_fan(cmd: FanCmd) -> i32 {
    // `restore --local` deliberately skips every daemon check: it is the path
    // `apexd.service`'s ExecStopPost= takes after a crash, when there is no
    // daemon left to ask.
    if let FanCmd::Restore { local: true } = cmd {
        return fan_restore_locally();
    }

    let conn = connect().await;
    let running = match &conn {
        Some(c) => daemon_running(c).await,
        None => false,
    };
    let proxy = match (&conn, running) {
        (Some(c), true) => FanProxy::new(c).await.ok(),
        _ => None,
    };

    match cmd {
        FanCmd::Status => {
            match &proxy {
                Some(p) => {
                    let supported = p.supported().await.unwrap_or(false);
                    println!("mode      : {}", p.mode().await.unwrap_or_default());
                    println!("supported : {supported}");
                    if let Ok(modes) = p.modes().await {
                        println!("modes     : {}", modes.join(", "));
                    }
                    if let Ok(pwm) = p.pwm().await {
                        if pwm > 0 {
                            println!("pwm       : {pwm} ({}%)", (pwm as u32 * 100) / 255);
                        }
                    }
                    if let Ok(fans) = p.fans().await {
                        if fans.is_empty() {
                            println!("fans      : (none detected)");
                        }
                        for f in fans {
                            println!("  {}", render_fan(&f));
                        }
                    }
                }
                None => {
                    let v = LocalView::detect();
                    let cfg = v.active_profile().fan_config();
                    let inv = apexd_core::fan::FanInventory::discover(Path::new("/sys"), &cfg);
                    println!("apexd not running — reading fans locally.");
                    println!("supported : {}", inv.controllable());
                    println!("modes     : {}", inv.modes(&cfg).join(", "));
                    let readings = inv.read();
                    if readings.is_empty() {
                        println!("fans      : (none detected)");
                    }
                    for r in readings {
                        let mut parts = vec![r.id.clone()];
                        if let Some(rpm) = r.rpm {
                            parts.push(format!("{rpm} rpm"));
                        }
                        if let Some(p) = r.percent {
                            parts.push(format!("{p}%"));
                        }
                        if let Some(p) = r.pwm {
                            parts.push(format!("pwm {p}"));
                        }
                        if r.controllable {
                            parts.push("controllable".into());
                        }
                        println!("  {}", parts.join("  "));
                    }
                }
            }
            0
        }
        FanCmd::Mode { name } => match &proxy {
            Some(p) => match p.set_mode(&name).await {
                Ok(()) => {
                    println!("apex: fan mode -> {name}");
                    0
                }
                Err(e) => {
                    eprintln!("apex: SetMode failed: {e}");
                    1
                }
            },
            None => {
                eprintln!("apex: apexd not running — cannot change fan mode.");
                1
            }
        },
        FanCmd::Pwm { value } => match &proxy {
            Some(p) => match p.set_pwm(value).await {
                Ok(()) => {
                    println!("apex: fan pwm -> {value}");
                    0
                }
                Err(e) => {
                    eprintln!("apex: SetPwm failed: {e}");
                    1
                }
            },
            None => {
                eprintln!("apex: apexd not running — cannot set fan pwm.");
                1
            }
        },
        FanCmd::Restore { local: _ } => match &proxy {
            Some(p) => match p.restore_firmware().await {
                Ok(()) => {
                    println!("apex: fans restored to firmware control");
                    0
                }
                Err(e) => {
                    eprintln!("apex: RestoreFirmware failed: {e} — falling back to a local restore");
                    fan_restore_locally()
                }
            },
            // No daemon: still restore, directly. Never leave fans in whatever
            // state a dead daemon left them.
            None => fan_restore_locally(),
        },
    }
}

/// Write the fan-restore plan straight to sysfs. Root-only; honours
/// `APEXD_DRY_RUN=1`.
fn fan_restore_locally() -> i32 {
    let v = LocalView::detect();
    let cfg = v.active_profile().fan_config();
    let dry = apexd_core::dry_run_from_env();
    let writer = apexd_core::RealWriter::new(dry);
    let n = apexd_core::fan::restore_to_firmware(Path::new("/sys"), &cfg, &writer);
    if n == 0 {
        println!("apex: no controllable fan found — nothing to restore");
    } else {
        println!(
            "apex: fans handed back to firmware control ({n} action(s){})",
            if dry { ", dry-run" } else { "" }
        );
    }
    0
}

async fn cmd_game(cmd: GameCmd) -> i32 {
    let conn = connect().await;
    let running = match &conn {
        Some(c) => daemon_running(c).await,
        None => false,
    };
    let proxy = match (&conn, running) {
        (Some(c), true) => GameModeProxy::new(c).await.ok(),
        _ => None,
    };

    match cmd {
        GameCmd::Status => {
            match &proxy {
                Some(p) => {
                    println!("active    : {}", p.active().await.unwrap_or(false));
                    println!("supported : {}", p.supported().await.unwrap_or(false));
                    if let Ok(status) = p.status().await {
                        let mut keys: Vec<&String> = status.keys().collect();
                        keys.sort();
                        for k in keys {
                            if k == "active" || k == "supported" {
                                continue;
                            }
                            println!("{k:10}: {}", render_value(&status[k]));
                        }
                    }
                }
                None => {
                    let v = LocalView::detect();
                    let cfg = v.active_profile().game_config();
                    let topo = apexd_core::CoreTopology::detect_from(Path::new("/sys"));
                    println!("apexd not running — showing the local view.");
                    println!("supported : {}", cfg.enabled);
                    println!("tier      : {}", cfg.tier);
                    println!("cpuset    : {}", cfg.cpuset);
                    println!("irq       : {}", cfg.irq);
                    println!("cgroup    : {}", cfg.cgroup);
                    println!(
                        "cores     : P={} E={} (detected via {})",
                        if topo.pcore_list().is_empty() { "(none)".into() } else { topo.pcore_list() },
                        if topo.ecore_list().is_empty() { "(none)".into() } else { topo.ecore_list() },
                        topo.source.as_str()
                    );
                    println!(
                        "nvidia-smi: {}",
                        if apexd_core::gpu::nvidia_smi_available() { "present" } else { "absent" }
                    );
                }
            }
            0
        }
        GameCmd::Start { pid } => match &proxy {
            Some(p) => {
                let res = match pid {
                    Some(pid) => p.start_for_pid(pid).await,
                    None => p.set_active(true).await,
                };
                match res {
                    Ok(()) => {
                        println!("apex: game mode ON");
                        0
                    }
                    Err(e) => {
                        eprintln!("apex: entering game mode failed: {e}");
                        1
                    }
                }
            }
            None => {
                eprintln!("apex: apexd not running — cannot enter game mode.");
                1
            }
        },
        GameCmd::Stop => match &proxy {
            Some(p) => match p.set_active(false).await {
                Ok(()) => {
                    println!("apex: game mode OFF");
                    0
                }
                Err(e) => {
                    eprintln!("apex: leaving game mode failed: {e}");
                    1
                }
            },
            None => {
                eprintln!("apex: apexd not running — cannot leave game mode.");
                1
            }
        },
        GameCmd::Attach { pid } => match &proxy {
            Some(p) => match p.attach_pid(pid).await {
                Ok(()) => {
                    println!("apex: pid {pid} attached to the game cpuset");
                    0
                }
                Err(e) => {
                    eprintln!("apex: AttachPid failed: {e}");
                    1
                }
            },
            None => {
                eprintln!("apex: apexd not running — cannot attach a pid.");
                1
            }
        },
    }
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

    // Every check below reports what this machine has; a WARN is information,
    // not a fault. Nothing here is required for apexd to work.
    let driver = v.fingerprint.cpu.scaling_driver.as_deref().unwrap_or("");
    line(
        !driver.is_empty(),
        &format!(
            "cpufreq scaling driver present ({})",
            if driver.is_empty() { "none" } else { driver }
        ),
    );
    line(
        v.fingerprint.cpu.amd_pstate() || v.fingerprint.cpu.intel_pstate(),
        &format!(
            "EPP-capable scaling driver ({}) — without it, tiers use the governor alone",
            if driver.is_empty() { "none" } else { driver }
        ),
    );
    line(
        Path::new("/sys/firmware/acpi/platform_profile").exists(),
        &format!(
            "ACPI platform_profile present (choices: {})",
            read_sys("firmware/acpi/platform_profile_choices").unwrap_or_else(|| "none".into())
        ),
    );

    let inv = apexd_core::BatteryInventory::detect();
    line(!inv.is_empty(), &format!("battery discovery: {}", inv.summary()));
    if !inv.is_empty() {
        line(
            inv.supports_thresholds(),
            &format!(
                "charge threshold control present ({})",
                inv.threshold_support().as_str()
            ),
        );
    }

    let s2idle = read_sys("power/mem_sleep").map(|s| s.contains("[s2idle]")).unwrap_or(false);
    line(s2idle, "s2idle is the active suspend mode");

    // ── M6: fan control and game orchestration ───────────────────────────────
    let fan_cfg = v.active_profile().fan_config();
    let inv = apexd_core::fan::FanInventory::discover(Path::new("/sys"), &fan_cfg);
    line(
        inv.controllable(),
        &format!(
            "fan control channel present, write access unverified ({})",
            if inv.controls.is_empty() && inv.msi_ec.is_none() {
                "none".to_string()
            } else {
                let mut s: Vec<String> = inv.controls.iter().map(|c| c.id.clone()).collect();
                if inv.msi_ec.is_some() {
                    s.push("msi-ec".into());
                }
                s.join(", ")
            }
        ),
    );
    let topo = apexd_core::CoreTopology::detect_from(Path::new("/sys"));
    if v.fingerprint.cpu.hybrid {
        line(
            topo.is_hybrid(),
            &format!(
                "P/E split detected via {} (P={} E={})",
                topo.source.as_str(),
                topo.pcore_list(),
                topo.ecore_list()
            ),
        );
    }
    if v.fingerprint.gpus.iter().any(|g| g.vendor == apexd_core::GpuVendor::Nvidia) {
        line(
            apexd_core::gpu::nvidia_smi_available(),
            "nvidia-smi on PATH (needed for game-mode clock locks)",
        );
    }
    line(
        Path::new("/sys/fs/cgroup/cgroup.controllers").exists(),
        "cgroup v2 present (needed for game-mode cpuset pinning)",
    );

    let metrics_up = TcpStream::connect_timeout(
        &"127.0.0.1:9723".parse::<SocketAddr>().unwrap(),
        Duration::from_millis(200),
    )
    .is_ok();
    line(metrics_up, "metrics endpoint reachable on 127.0.0.1:9723");

    0
}

// ── small helpers ────────────────────────────────────────────────────────────

/// Render one `a{sv}` fan entry as a single line.
fn render_fan(f: &std::collections::HashMap<String, zvariant::OwnedValue>) -> String {
    let get = |k: &str| f.get(k).map(render_value);
    let mut parts = vec![get("id").unwrap_or_else(|| "?".into())];
    if let Some(rpm) = get("rpm") {
        parts.push(format!("{rpm} rpm"));
    }
    if let Some(pct) = get("percent") {
        parts.push(format!("{pct}%"));
    }
    if let Some(pwm) = get("pwm") {
        parts.push(format!("pwm {pwm}"));
    }
    if get("controllable").as_deref() == Some("true") {
        parts.push("controllable".into());
    }
    parts.join("  ")
}

/// Human rendering for the handful of D-Bus variant types apexd returns.
fn render_value(v: &zvariant::OwnedValue) -> String {
    fn inner(v: &zvariant::Value<'_>) -> String {
        use zvariant::Value;
        match v {
            Value::Str(s) => s.to_string(),
            Value::Bool(b) => b.to_string(),
            Value::U8(n) => n.to_string(),
            Value::U16(n) => n.to_string(),
            Value::U32(n) => n.to_string(),
            Value::U64(n) => n.to_string(),
            Value::I16(n) => n.to_string(),
            Value::I32(n) => n.to_string(),
            Value::I64(n) => n.to_string(),
            Value::F64(n) => format!("{n:.2}"),
            Value::Array(a) => a.iter().map(inner).collect::<Vec<_>>().join(", "),
            Value::Value(b) => inner(b),
            other => format!("{other:?}"),
        }
    }
    inner(v)
}

fn print_kv(key: &str, val: Option<String>) {
    if let Some(v) = val {
        println!("{key}: {v}");
    }
}

fn line(ok: bool, what: &str) {
    println!("[{}] {}", if ok { "PASS" } else { "WARN" }, what);
}

fn read_sys(rel: &str) -> Option<String> {
    read_abs(&format!("/sys/{rel}"))
}

fn read_abs(path: &str) -> Option<String> {
    std::fs::read_to_string(path).ok().map(|s| s.trim().to_string())
}
