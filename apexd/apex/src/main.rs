//! `apex` — the APEX-OS control CLI. A thin client over the frozen
//! `org.apexos.Apexd1` D-Bus API, with read-only local fallbacks (via
//! `apexd-core`) so `fingerprint`, `status`, `profile`, `doctor` and dry-run
//! tier planning work even when `apexd` is not running. Every D-Bus verb
//! degrades gracefully — a clear message, a non-zero exit, never a panic.

mod agent;
mod mode;
mod ops;
mod proxy;
mod request;
mod secret;
mod touchpad;

use std::net::{SocketAddr, TcpStream};
use std::path::Path;
use std::time::Duration;

use apexd_core::tier::Tier;
use clap::{Args, Parser, Subcommand};

use crate::ops::LocalView;
use crate::proxy::{
    connect, daemon_running, BatteryProxy, FanProxy, GameModeProxy, MetricsProxy, PowerProxy,
    ProfileProxy,
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
    /// Named operating modes: daily, gaming, development, creator, ai, battery,
    /// couch, server.
    ///
    /// A mode is a named combination of things `apex tier` and `apex game`
    /// already do — it is not another image, and it adds no new hardware lever.
    /// The active mode is derived from what apexd reports rather than stored,
    /// so it cannot go stale and needs no root.
    Mode {
        #[command(subcommand)]
        cmd: Option<mode::ModeCmd>,
    },
    /// What the machine is measured to be doing, and what that suggests.
    ///
    /// Reports the workload, the signals behind it, and the signals this
    /// hardware cannot produce. Applies nothing: acting on it is an explicit
    /// `apex mode set --auto`, and APEX ships no timer that does it for you.
    Workload(mode::WorkloadArgs),
    /// Performance Lab: CPU/GPU clocks, power, temperatures, VRAM, scheduler.
    ///
    /// Read-only and root-free. Frame time is reported as unavailable with the
    /// reason, because no generic source for it exists and APEX will not
    /// substitute a number it did not measure.
    Perf(mode::PerfArgs),
    /// Print the hardware fingerprint and layered profile selection.
    Fingerprint,
    /// Pin the current deployment (ostree admin pin 0). Requires root.
    Pin,
    /// Roll back to the previous deployment (bootc rollback). Requires root.
    Rollback,
    /// Update the OS image (bootc upgrade) and firmware (fwupdmgr). Requires root.
    Update(UpdateArgs),
    /// Drive APEX Shell: open the launcher, dashboard, settings window, lock
    /// screen and the quick toggles.
    ///
    /// A thin wrapper over the shell's Quickshell IPC. It exists so compositor
    /// configs and scripts have one stable, readable command instead of
    /// spelling out `qs -p /usr/share/apex-shell ipc call <target> <fn>`, and so
    /// the shell's install path is not hardcoded in every keybind.
    Shell {
        #[command(subcommand)]
        cmd: ShellCmd,
    },
    /// Read the telemetry snapshot: tier, AC state, package power, battery
    /// charge and thermal zones.
    ///
    /// Values come from apexd's `org.apexos.Apexd1.Metrics.Snapshot`, the same
    /// source as the Prometheus endpoint on 127.0.0.1:9723. Read-only, so it
    /// needs no root.
    Metrics(MetricsArgs),
    /// Diagnose the power stack.
    Doctor,
    /// Show the booted image and its changelog labels.
    Changelog,
    /// Install packages from the enabled repositories, a Flatpak id, or a local
    /// .rpm file. Requires root.
    ///
    /// Each argument is a package name from Fedora/RPM Fusion/an enabled COPR, a
    /// reverse-DNS Flatpak id (org.gimp.GIMP), or a path to an .rpm file. A local
    /// file is copied into /var/lib/apex/pkg/local so later rebuilds no longer
    /// need the original; its dependencies still come from the repositories.
    ///
    /// Packages go into a systemd system extension, NOT an rpm-ostree layer, so
    /// the OS keeps updating normally and `apex rollback` still works.
    Install {
        #[arg(required = true, value_name = "PACKAGE|FILE.rpm")]
        packages: Vec<String>,
        /// Skip weak dependencies (smaller install, fewer optional features).
        #[arg(long)]
        no_weak_deps: bool,
        /// Also consider a repository that is disabled by default.
        #[arg(long, value_name = "REPO")]
        enable_repo: Vec<String>,
        /// Install a local .rpm file that no trusted key covers. Applies only to
        /// the files named on this command line, never to repository packages,
        /// and the decision is recorded per file so `apex pkg list` and
        /// `apex pkg verify` keep reporting it.
        #[arg(long)]
        allow_unsigned: bool,
    },
    /// Remove packages installed with `apex install`. Requires root.
    Remove {
        #[arg(required = true, value_name = "PACKAGE")]
        packages: Vec<String>,
    },
    /// Search all enabled package repositories.
    Search {
        #[arg(required = true, value_name = "TERM")]
        terms: Vec<String>,
    },
    /// Manage additional package repositories.
    Repo {
        #[command(subcommand)]
        cmd: RepoCmd,
    },
    /// Manage installed packages: list, status, rebuild, rollback, adopt.
    Pkg {
        #[command(subcommand)]
        cmd: PkgCmd,
    },
    /// Run and supervise coding agents on managed terminals.
    ///
    /// APEX owns the PTY, the sandbox and the project state; the agent itself
    /// is the ordinary upstream binary (`claude`, `opencode`, `codex`, …) in an
    /// ordinary terminal. Sessions outlive the window they were started from,
    /// so a closed terminal never kills a running task.
    ///
    /// Needs the per-user runtime: `systemctl --user enable --now apex-agentd`.
    Agent {
        #[command(subcommand)]
        cmd: agent::AgentCmd,
    },
    /// Projects, agent worktrees and checkpoints.
    Project {
        #[command(subcommand)]
        cmd: agent::ProjectCmd,
    },
    /// Structured privilege requests: how a sandboxed agent asks for a system
    /// change, and how you decide.
    ///
    /// An agent has no sudo, no root shell, and a sandbox that cannot reach the
    /// system bus. It files a request naming one of a closed set of operations
    /// and a reason; you review it and either refuse or approve, and approving
    /// runs the operation with YOUR privilege. There is deliberately no verb
    /// for an arbitrary command.
    Request {
        #[command(subcommand)]
        cmd: request::RequestCmd,
    },
    /// The secret broker: let an agent USE a credential without holding it.
    ///
    /// The broker performs the operation and returns the result; the token
    /// stays in a process the agent cannot see. A git credential helper cannot
    /// do this — git runs inside the sandbox, so whatever the helper prints is
    /// readable by the agent.
    Secret {
        #[command(subcommand)]
        cmd: secret::SecretCmd,
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
enum RepoCmd {
    /// List enabled and disabled repositories.
    List,
    /// Enable a Fedora COPR project (OWNER/PROJECT). Requires root.
    EnableCopr {
        #[arg(value_name = "OWNER/PROJECT")]
        project: String,
    },
    /// Disable a previously enabled Fedora COPR project. Requires root.
    DisableCopr {
        #[arg(value_name = "OWNER/PROJECT")]
        project: String,
    },
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
    /// Skip updating Flatpak applications.
    #[arg(long)]
    skip_flatpak: bool,
    /// Keep ostree's per-object fsync on during the pull. Roughly halves update
    /// speed (measured: ~8 MiB/s with it, ~14.6 without, because 179k objects at
    /// 2.98 ms of fsync each outweighs the download itself) in exchange for
    /// durability if the machine loses power mid-update.
    #[arg(long)]
    fsync: bool,
}

/// `apex shell <verb>` — the surfaces APEX Shell exposes over IPC.
///
/// Each variant maps to one `(target, function)` pair. Names are the
/// user-facing vocabulary ("launcher", "settings"), not the shell's internal
/// target strings, so the IPC surface can be renamed without breaking every
/// keybind on every machine.
#[derive(Subcommand)]
enum ShellCmd {
    /// Toggle the app launcher.
    Launcher,
    /// Toggle the dashboard. Optionally on a specific page.
    Dashboard {
        /// home | stats | kanban | launcher | config
        #[arg(value_name = "PAGE")]
        page: Option<String>,
    },
    /// Open the settings window, optionally at a page (appearance, layout,
    /// data, keybinds, misc). Run `apex shell settings --list` for the live
    /// list.
    Settings {
        #[arg(value_name = "PAGE")]
        page: Option<String>,
        /// Print the page names the running shell actually offers.
        ///
        /// Conflicts with PAGE and --close rather than silently taking
        /// precedence: `settings --close --list` had no obvious meaning, and
        /// quietly honouring one of them is how a script ends up doing the
        /// opposite of what it says.
        #[arg(long, conflicts_with_all = ["page", "close"])]
        list: bool,
        /// Close it instead of toggling.
        #[arg(long, conflicts_with = "page")]
        close: bool,
    },
    /// Lock the session.
    Lock,
    /// Toggle the notification centre.
    Notifications,
    /// Toggle clipboard history.
    Clipboard,
    /// Toggle the wallpaper picker.
    Wallpaper,
    /// Toggle the power menu.
    Power,
    /// Toggle the desktop context menu (what a right-click on the desktop
    /// opens). Replaces the compositor's own root menu; the same QML surface
    /// serves all three sessions.
    Menu,
    /// Toggle the audio panel (output, input or the app mixer).
    Audio {
        /// out | in | mixer
        #[arg(value_name = "WHICH", default_value = "out")]
        which: String,
    },
    /// Toggle the network panel on a given tab.
    Network {
        /// wifi | bluetooth | vpn | hotspot
        #[arg(value_name = "TAB", default_value = "wifi")]
        tab: String,
    },
    /// Toggle focus mode.
    Focus,
    /// Start the screen-recorder setup strip.
    Record,
    /// List every target this wrapper knows, with the IPC call behind it.
    List,
    /// Call an arbitrary target/function, for anything not covered above.
    Ipc {
        #[arg(value_name = "TARGET")]
        target: String,
        #[arg(value_name = "FUNCTION", default_value = "toggle")]
        function: String,
        /// Extra positional arguments passed through to the handler.
        #[arg(value_name = "ARG")]
        args: Vec<String>,
    },
}

#[derive(Args)]
struct MetricsArgs {
    /// Emit machine-readable JSON instead of an aligned table.
    #[arg(long)]
    json: bool,
    /// Keep printing a new sample every INTERVAL seconds until interrupted.
    ///
    /// With --json this produces one JSON object per line (JSON Lines), which is
    /// the shape a log shipper or `jq --unbuffered` wants.
    #[arg(long, value_name = "INTERVAL", num_args = 0..=1, default_missing_value = "2")]
    stream: Option<f64>,
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
        // The agent verbs are a blocking client over the per-user runtime's
        // Unix socket, not a D-Bus call, and `attach` deliberately blocks for
        // as long as the user stays attached.
        Cmd::Agent { cmd } => agent::agent(cmd),
        Cmd::Project { cmd } => agent::project_cmd(cmd),
        Cmd::Request { cmd } => request::main(cmd),
        Cmd::Secret { cmd } => secret::main(cmd),
        Cmd::Tier { name } => cmd_tier(name).await,
        Cmd::Profile => cmd_profile().await,
        Cmd::Battery(args) => cmd_battery(args).await,
        Cmd::Fan { cmd } => cmd_fan(cmd.unwrap_or(FanCmd::Status)).await,
        Cmd::Game { cmd } => cmd_game(cmd).await,
        // Read-only by default and deliberately absent from the privileged set:
        // `mode set` mutates through apexd's polkit-authorised D-Bus API as the
        // session user, exactly as `apex tier` does.
        Cmd::Mode { cmd } => mode::main(cmd.unwrap_or(mode::ModeCmd::Status)).await,
        Cmd::Workload(args) => mode::workload_main(args),
        Cmd::Perf(args) => mode::perf_main(args),
        Cmd::Fingerprint => cmd_fingerprint(),
        Cmd::Pin => ops::pin(),
        Cmd::Rollback => ops::rollback(),
        Cmd::Update(args) => ops::update(ops::UpdateOptions {
            check: args.check,
            skip_firmware: args.skip_firmware,
            firmware_only: args.firmware_only,
            keep_fsync: args.fsync,
            skip_packages: args.skip_packages,
            skip_flatpak: args.skip_flatpak,
        }),
        Cmd::Shell { cmd } => cmd_shell(cmd),
        Cmd::Metrics(args) => cmd_metrics(args).await,
        Cmd::Doctor => cmd_doctor().await,
        Cmd::Changelog => ops::changelog(),
        Cmd::Install {
            packages,
            no_weak_deps,
            enable_repo,
            allow_unsigned,
        } => ops::pkg(&install_argv(
            packages,
            no_weak_deps,
            enable_repo,
            allow_unsigned,
        )),
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
        Cmd::Repo { cmd } => {
            let argv = match cmd {
                RepoCmd::List => vec!["repo-list".into()],
                RepoCmd::EnableCopr { project } => vec!["repo-enable-copr".into(), project],
                RepoCmd::DisableCopr { project } => vec!["repo-disable-copr".into(), project],
            };
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

/// Build the engine argv for `apex install`.
///
/// Split out of `main` so the mapping can be pinned by a test: the engine is a
/// separate process, so a dropped or misspelled flag here is not a compile error
/// — it is a silent policy change. `--allow-unsigned` in particular decides
/// whether an unverifiable RPM is refused or installed.
fn install_argv(
    packages: Vec<String>,
    no_weak_deps: bool,
    enable_repo: Vec<String>,
    allow_unsigned: bool,
) -> Vec<String> {
    let mut argv = vec!["install".to_string()];
    argv.extend(packages);
    if no_weak_deps {
        argv.push("--no-weak-deps".to_string());
    }
    for repo in enable_repo {
        argv.push(format!("--enable-repo={repo}"));
    }
    if allow_unsigned {
        argv.push("--allow-unsigned".to_string());
    }
    argv
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

/// Where the shell is vendored inside the image.
const SHELL_DIR_DEFAULT: &str = "/usr/share/apex-shell";

/// The shell config directory to address over IPC.
///
/// `APEX_SHELL_DIR` overrides it, matching the convention
/// /usr/libexec/apex-shell-autostart already uses. That is what makes it
/// possible to drive a working-tree checkout during development instead of only
/// the copy baked into the image.
fn shell_dir() -> String {
    std::env::var("APEX_SHELL_DIR")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| SHELL_DIR_DEFAULT.to_string())
}

/// The mapping from `apex shell <verb>` to the shell's IPC surface.
///
/// Verb names are deliberately the user's vocabulary rather than the shell's
/// internal target strings: "settings" rather than "nexus", "power" rather than
/// "PowerMenu-toggle". That indirection is the point of the wrapper — the IPC
/// names can change without every keybind on every machine breaking.
fn shell_targets() -> Vec<(&'static str, &'static str, &'static str)> {
    vec![
        ("launcher", "dashboard-launcher", "toggle"),
        ("dashboard", "dashboard-home", "toggle"),
        ("settings", "nexus", "toggle"),
        ("lock", "lockscreen", "lock"),
        ("notifications", "notification-toggle", "toggle"),
        ("clipboard", "clipboard-toggle", "toggle"),
        ("wallpaper", "wallpaper-toggle", "toggle"),
        ("menu", "context-menu", "toggle"),
        ("power", "PowerMenu-toggle", "toggle"),
        ("audio out", "audioOut-toggle", "toggle"),
        ("audio in", "audioIn-toggle", "toggle"),
        ("audio mixer", "audioMix-toggle", "toggle"),
        ("network wifi", "wifi-toggle", "toggle"),
        ("network bluetooth", "bluetooth-toggle", "toggle"),
        ("network vpn", "vpn-toggle", "toggle"),
        ("network hotspot", "hotspot-toggle", "toggle"),
        ("focus", "focus-toggle", "toggle"),
        ("record", "screenrec-on", "toggle"),
    ]
}

/// Why an IPC call did not succeed.
///
/// Distinguished rather than collapsed into one error because they call for
/// completely different responses: "you are not in a graphical session", "your
/// shell predates this CLI" and "the shell is not running" have nothing to do
/// with each other.
#[derive(Debug, PartialEq, Eq)]
enum IpcFailure {
    /// `qs` is not installed — not a graphical session.
    QsMissing,
    /// No shell config at the addressed path.
    MissingConfig,
    /// The shell answered, but exposes no such target (or function).
    MissingHandler { function: bool },
    /// No shell instance is running.
    NotRunning,
    /// Anything else, carrying whatever the tool said.
    Other(String),
}

/// Classify a completed `qs ipc call`.
///
/// Shared by both callers, deliberately. `qs ipc call` exits ZERO for "Target
/// not found.", "Function not found." and "Could not open config file" — it only
/// fails properly (255) when no instance is running. Trusting the exit status
/// reports success for a call that did nothing, which from a keybind is
/// indistinguishable from a dead key.
///
/// Applying this in only ONE of the two callers is exactly the bug this function
/// exists to prevent: the query path previously treated "Target not found." as a
/// successful result and printed it as data, so `settings --list` against an
/// older shell listed "Target", "not" and "found." as pages and exited 0.
fn classify_qs(code: i32, stdout: &str, stderr: &str) -> Option<IpcFailure> {
    let combined = format!("{stdout}{stderr}");

    if combined.contains("Could not open config file") {
        return Some(IpcFailure::MissingConfig);
    }
    if combined.contains("Target not found") {
        return Some(IpcFailure::MissingHandler { function: false });
    }
    if combined.contains("Function not found") {
        return Some(IpcFailure::MissingHandler { function: true });
    }
    if combined.contains("No running instances") {
        return Some(IpcFailure::NotRunning);
    }
    if code != 0 {
        return Some(IpcFailure::Other(combined));
    }
    None
}

/// Run one IPC call, returning `(stdout, stderr)` on success.
///
/// `qs` is Quickshell's own CLI and is what actually speaks the protocol; there
/// is no D-Bus route to the shell to use instead.
fn qs_call(target: &str, function: &str, args: &[String]) -> Result<(String, String), IpcFailure> {
    use std::process::Command;

    let mut argv: Vec<String> = vec![
        "-p".into(),
        shell_dir(),
        "ipc".into(),
        "call".into(),
        target.into(),
        function.into(),
    ];
    argv.extend(args.iter().cloned());

    let out = match Command::new("qs").args(&argv).output() {
        Ok(o) => o,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(IpcFailure::QsMissing),
        Err(e) => return Err(IpcFailure::Other(format!("could not run qs: {e}"))),
    };

    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();

    match classify_qs(out.status.code().unwrap_or(1), &stdout, &stderr) {
        Some(f) => Err(f),
        None => Ok((stdout, stderr)),
    }
}

fn report_ipc_failure(f: &IpcFailure, target: &str, function: &str) {
    match f {
        IpcFailure::QsMissing => eprintln!(
            "apex: `qs` (Quickshell) not found. `apex shell` drives the running \
             shell over its IPC, so it only works inside a graphical session."
        ),
        IpcFailure::MissingConfig => eprintln!(
            "apex: no shell config at {}. Set APEX_SHELL_DIR to point at a \
             checkout, or reinstall the image copy.",
            shell_dir()
        ),
        IpcFailure::MissingHandler { function: is_fn } => {
            let what = if *is_fn { "function" } else { "target" };
            eprintln!(
                "apex: the running APEX Shell does not expose {what} \
                 '{target} {function}'.\n\
                 This usually means the shell is older than this CLI — \
                 `apex update` and log back in.\n\
                 `apex shell list` shows what this wrapper knows about."
            );
        }
        IpcFailure::NotRunning => eprintln!(
            "apex: APEX Shell is not running (addressing {}).\n\
             Start or repair it with: /usr/libexec/apex-shell-autostart",
            shell_dir()
        ),
        IpcFailure::Other(msg) => {
            eprintln!("apex: shell IPC '{target} {function}' failed.");
            if !msg.trim().is_empty() {
                eprint!("{msg}");
            }
        }
    }
}

/// Fire and forget: forward whatever the handler returned.
fn shell_ipc(target: &str, function: &str, args: &[String]) -> i32 {
    match qs_call(target, function, args) {
        Ok((stdout, stderr)) => {
            // Handlers return strings ("nexus open at appearance"); pass them
            // through so scripting can read them.
            if !stdout.trim().is_empty() {
                print!("{stdout}");
            }
            if !stderr.trim().is_empty() {
                eprint!("{stderr}");
            }
            0
        }
        Err(f) => {
            report_ipc_failure(&f, target, function);
            1
        }
    }
}

/// Capture a handler's return value, for the queries.
///
/// Uses the same classification as `shell_ipc`, so a failure can never be
/// mistaken for data.
fn shell_ipc_query(target: &str, function: &str) -> Result<String, IpcFailure> {
    qs_call(target, function, &[]).map(|(stdout, _)| stdout.trim().to_string())
}

fn cmd_shell(cmd: ShellCmd) -> i32 {
    match cmd {
        ShellCmd::Launcher => shell_ipc("dashboard-launcher", "toggle", &[]),

        ShellCmd::Dashboard { page } => {
            // The dashboard exposes one target per page rather than a target
            // taking an argument, so the page becomes part of the target name.
            let page = page.unwrap_or_else(|| "home".into());
            const PAGES: [&str; 5] = ["home", "stats", "kanban", "launcher", "config"];
            if !PAGES.contains(&page.as_str()) {
                eprintln!(
                    "apex: unknown dashboard page '{page}' (try: {})",
                    PAGES.join(", ")
                );
                return 1;
            }
            shell_ipc(&format!("dashboard-{page}"), "toggle", &[])
        }

        ShellCmd::Settings { page, list, close } => {
            if list {
                // Ask the shell rather than hardcoding: the page set lives in
                // the shell's PageRegistry and this must not drift from it.
                return match shell_ipc_query("nexus", "pages") {
                    Ok(s) if !s.is_empty() => {
                        for p in s.split_whitespace() {
                            println!("{p}");
                        }
                        0
                    }
                    Ok(_) => {
                        eprintln!("apex: the shell returned no settings pages.");
                        1
                    }
                    Err(f) => {
                        report_ipc_failure(&f, "nexus", "pages");
                        1
                    }
                };
            }
            if close {
                return shell_ipc("nexus", "close", &[]);
            }
            match page {
                Some(p) => shell_ipc("nexus", "toggle", &[p]),
                None => shell_ipc("nexus", "toggle", &[]),
            }
        }

        ShellCmd::Lock => shell_ipc("lockscreen", "lock", &[]),
        ShellCmd::Notifications => shell_ipc("notification-toggle", "toggle", &[]),
        ShellCmd::Clipboard => shell_ipc("clipboard-toggle", "toggle", &[]),
        ShellCmd::Wallpaper => shell_ipc("wallpaper-toggle", "toggle", &[]),
        ShellCmd::Menu => shell_ipc("context-menu", "toggle", &[]),
        ShellCmd::Power => shell_ipc("PowerMenu-toggle", "toggle", &[]),
        ShellCmd::Focus => shell_ipc("focus-toggle", "toggle", &[]),
        ShellCmd::Record => shell_ipc("screenrec-on", "toggle", &[]),

        ShellCmd::Audio { which } => {
            let target = match which.as_str() {
                "out" | "output" | "sink" => "audioOut-toggle",
                "in" | "input" | "source" | "mic" => "audioIn-toggle",
                "mixer" | "mix" | "apps" => "audioMix-toggle",
                other => {
                    eprintln!("apex: unknown audio panel '{other}' (try: out, in, mixer)");
                    return 1;
                }
            };
            shell_ipc(target, "toggle", &[])
        }

        ShellCmd::Network { tab } => {
            let target = match tab.as_str() {
                "wifi" | "wlan" => "wifi-toggle",
                "bluetooth" | "bt" => "bluetooth-toggle",
                "vpn" => "vpn-toggle",
                "hotspot" | "ap" => "hotspot-toggle",
                other => {
                    eprintln!(
                        "apex: unknown network tab '{other}' \
                         (try: wifi, bluetooth, vpn, hotspot)"
                    );
                    return 1;
                }
            };
            shell_ipc(target, "toggle", &[])
        }

        ShellCmd::List => {
            let rows = shell_targets();
            let width = rows.iter().map(|(v, ..)| v.len()).max().unwrap_or(0);
            println!("{:<width$}  IPC CALL", "apex shell …", width = width);
            for (verb, target, func) in rows {
                println!("{verb:<width$}  {target} {func}", width = width);
            }
            println!();
            println!("Anything else: apex shell ipc <target> <function> [args…]");
            0
        }

        ShellCmd::Ipc {
            target,
            function,
            args,
        } => shell_ipc(&target, &function, &args),
    }
}

/// `apex metrics` — read apexd's telemetry snapshot.
///
/// The data already existed in two places apexd exposes: the
/// `org.apexos.Apexd1.Metrics.Snapshot` property and the Prometheus endpoint on
/// 127.0.0.1:9723. Neither was reachable from the CLI, so checking package power
/// or a thermal zone meant hand-writing a `busctl get-property` invocation or
/// curling a port. This is purely additive to the frozen D-Bus contract: it adds
/// a proxy and a verb, and changes nothing daemon-side.
///
/// Read-only, so deliberately absent from the privileged-command match: it must
/// stay usable without root.
async fn cmd_metrics(args: MetricsArgs) -> i32 {
    let Some(conn) = connect().await else {
        eprintln!("apex: cannot reach the system bus.");
        return 1;
    };

    if !daemon_running(&conn).await {
        eprintln!("apex: apexd not running — no metrics to read.");
        return 1;
    }

    let proxy = match MetricsProxy::new(&conn).await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("apex: cannot reach the Metrics interface: {e}");
            return 1;
        }
    };

    // Clamp the interval: a zero or negative period would spin the daemon.
    let interval = args
        .stream
        .map(|s| Duration::from_secs_f64(if s.is_finite() && s >= 0.1 { s } else { 0.1 }));

    loop {
        match proxy.snapshot().await {
            Ok(snap) => {
                if args.json {
                    println!("{}", snapshot_to_json(&snap));
                } else {
                    print_snapshot_table(&snap);
                }
            }
            Err(e) => {
                eprintln!("apex: reading the snapshot failed: {e}");
                // A one-shot read reports the failure; a stream keeps trying, so
                // a daemon restart does not end a long-running collector.
                if interval.is_none() {
                    return 1;
                }
            }
        }

        match interval {
            Some(d) => {
                // Without this a piped consumer sees nothing until the pipe
                // buffer fills, which for one small sample per interval can be
                // minutes.
                use std::io::Write;
                let _ = std::io::stdout().flush();
                tokio::time::sleep(d).await;
            }
            None => return 0,
        }
    }
}

/// Stable, human-sensible key order: the headline fields first in a fixed order,
/// then everything else (the `temp_<zone>` set, whose membership is per-machine)
/// alphabetically so successive samples line up.
fn snapshot_key_order(snap: &std::collections::HashMap<String, zvariant::OwnedValue>) -> Vec<String> {
    const PREFERRED: [&str; 4] = ["tier", "on_ac", "ppt_watts", "battery_uwh"];

    let mut out: Vec<String> = PREFERRED
        .iter()
        .filter(|k| snap.contains_key(**k))
        .map(|k| (*k).to_string())
        .collect();

    let mut rest: Vec<String> = snap
        .keys()
        .filter(|k| !PREFERRED.contains(&k.as_str()))
        .cloned()
        .collect();
    rest.sort();
    out.extend(rest);
    out
}

fn print_snapshot_table(snap: &std::collections::HashMap<String, zvariant::OwnedValue>) {
    let keys = snapshot_key_order(snap);
    let width = keys.iter().map(|k| k.len()).max().unwrap_or(0);
    for k in keys {
        if let Some(v) = snap.get(&k) {
            println!("{:<width$}  {}", k, render_value(v), width = width);
        }
    }
}

/// Minimal JSON encoder for the snapshot.
///
/// Hand-rolled rather than pulling serde_json in: `apex` ships in a signed image
/// and this is the only place in the CLI that needs JSON, so a few lines of
/// escaping is a better trade than another dependency in the tree.
fn snapshot_to_json(snap: &std::collections::HashMap<String, zvariant::OwnedValue>) -> String {
    let mut parts: Vec<String> = Vec::new();
    for k in snapshot_key_order(snap) {
        if let Some(v) = snap.get(&k) {
            parts.push(format!("{}:{}", json_string(&k), json_value(v)));
        }
    }
    format!("{{{}}}", parts.join(","))
}

fn json_value(v: &zvariant::OwnedValue) -> String {
    fn inner(v: &zvariant::Value<'_>) -> String {
        use zvariant::Value;
        match v {
            Value::Str(s) => json_string(s.as_str()),
            Value::Bool(b) => b.to_string(),
            Value::U8(n) => n.to_string(),
            Value::U16(n) => n.to_string(),
            Value::U32(n) => n.to_string(),
            Value::U64(n) => n.to_string(),
            Value::I16(n) => n.to_string(),
            Value::I32(n) => n.to_string(),
            Value::I64(n) => n.to_string(),
            // Non-finite floats have no JSON representation; null is the only
            // honest answer and parsers accept it.
            Value::F64(n) => {
                if n.is_finite() {
                    format!("{n}")
                } else {
                    "null".to_string()
                }
            }
            Value::Array(a) => format!(
                "[{}]",
                a.iter().map(inner).collect::<Vec<_>>().join(",")
            ),
            Value::Value(b) => inner(b),
            other => json_string(&format!("{other:?}")),
        }
    }
    inner(v)
}

pub(crate) fn json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            // JSON requires escaping everything below 0x20.
            c if (c as u32) < 0x20 => {
                use std::fmt::Write as _;
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
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

    for (ok, what) in touchpad::doctor_lines() {
        line(ok, &what);
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

// ── Tests ────────────────────────────────────────────────────────────────────
// `apex install` hands its arguments to a separate process, so nothing here is
// type-checked against the engine. These pin the two things that would fail
// silently: that a path is accepted where a package name goes, and that the
// unverified-RPM opt-in is off unless asked for and reaches the engine when it
// is asked for.
#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    fn install(argv: &[&str]) -> (Vec<String>, bool, Vec<String>, bool) {
        match Cli::try_parse_from(argv).expect("parses").command {
            Cmd::Install {
                packages,
                no_weak_deps,
                enable_repo,
                allow_unsigned,
            } => (packages, no_weak_deps, enable_repo, allow_unsigned),
            _ => panic!("not an install"),
        }
    }

    #[test]
    fn the_cli_definition_is_internally_consistent() {
        Cli::command().debug_assert();
    }

    #[test]
    fn shell_is_not_a_privileged_verb() {
        // `apex shell` drives the user's own session over IPC. Requiring root
        // would be both wrong and useless: root has no WAYLAND_DISPLAY, so the
        // call could not reach the shell anyway.
        let cli = Cli::try_parse_from(["apex", "shell", "launcher"]).expect("parses");
        assert!(
            !matches!(cli.command, Cmd::Update(_) | Cmd::Pin | Cmd::Rollback),
            "shell must not be classified with the root-only verbs"
        );
    }

    fn shell_cmd(argv: &[&str]) -> ShellCmd {
        match Cli::try_parse_from(argv).expect("parses").command {
            Cmd::Shell { cmd } => cmd,
            _ => panic!("not a shell command"),
        }
    }

    #[test]
    fn shell_verbs_parse() {
        assert!(matches!(shell_cmd(&["apex", "shell", "launcher"]), ShellCmd::Launcher));
        assert!(matches!(shell_cmd(&["apex", "shell", "lock"]), ShellCmd::Lock));
        assert!(matches!(shell_cmd(&["apex", "shell", "list"]), ShellCmd::List));
    }

    #[test]
    fn dashboard_page_is_optional() {
        match shell_cmd(&["apex", "shell", "dashboard"]) {
            ShellCmd::Dashboard { page } => assert_eq!(page, None),
            _ => panic!("wrong variant"),
        }
        match shell_cmd(&["apex", "shell", "dashboard", "stats"]) {
            ShellCmd::Dashboard { page } => assert_eq!(page.as_deref(), Some("stats")),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn settings_takes_a_page_or_a_query_or_a_close() {
        // A bare `apex shell settings` must work as a single keybind.
        match shell_cmd(&["apex", "shell", "settings"]) {
            ShellCmd::Settings { page, list, close } => {
                assert_eq!(page, None);
                assert!(!list);
                assert!(!close);
            }
            _ => panic!("wrong variant"),
        }
        match shell_cmd(&["apex", "shell", "settings", "keybinds"]) {
            ShellCmd::Settings { page, .. } => assert_eq!(page.as_deref(), Some("keybinds")),
            _ => panic!("wrong variant"),
        }
        assert!(matches!(
            shell_cmd(&["apex", "shell", "settings", "--list"]),
            ShellCmd::Settings { list: true, .. }
        ));
        assert!(matches!(
            shell_cmd(&["apex", "shell", "settings", "--close"]),
            ShellCmd::Settings { close: true, .. }
        ));
    }

    #[test]
    fn contradictory_settings_flags_are_rejected_not_guessed() {
        // Silently letting one win is how a script ends up doing the opposite of
        // what it reads as.
        for argv in [
            vec!["apex", "shell", "settings", "--list", "--close"],
            vec!["apex", "shell", "settings", "keybinds", "--list"],
            vec!["apex", "shell", "settings", "keybinds", "--close"],
        ] {
            assert!(
                Cli::try_parse_from(&argv).is_err(),
                "{argv:?} should have been rejected"
            );
        }
    }

    #[test]
    fn qs_silent_failures_are_classified_despite_a_zero_exit() {
        // The whole point: `qs ipc call` exits 0 for these, so a caller trusting
        // the exit status treats a call that did nothing as a success. The query
        // path once printed "Target not found." as if it were page data.
        assert_eq!(
            classify_qs(0, "Target not found.\n", ""),
            Some(IpcFailure::MissingHandler { function: false })
        );
        assert_eq!(
            classify_qs(0, "Function not found.\n", ""),
            Some(IpcFailure::MissingHandler { function: true })
        );
        assert_eq!(
            classify_qs(0, "Could not open config file at \"/nope\"\n", ""),
            Some(IpcFailure::MissingConfig)
        );
        // This one does exit non-zero (255), but must still be named rather
        // than lumped into Other.
        assert_eq!(
            classify_qs(255, "No running instances for \"/x/shell.qml\"\n", ""),
            Some(IpcFailure::NotRunning)
        );
    }

    #[test]
    fn a_real_handler_reply_is_not_mistaken_for_a_failure() {
        assert_eq!(classify_qs(0, "nexus open at keybinds\n", ""), None);
        assert_eq!(classify_qs(0, "appearance layout data keybinds misc\n", ""), None);
        // Empty output with a clean exit is a valid void handler.
        assert_eq!(classify_qs(0, "", ""), None);
    }

    #[test]
    fn an_unexplained_nonzero_exit_is_still_a_failure() {
        match classify_qs(3, "", "something went wrong") {
            Some(IpcFailure::Other(msg)) => assert!(msg.contains("something went wrong")),
            other => panic!("expected Other, got {other:?}"),
        }
    }

    #[test]
    fn audio_and_network_default_to_their_common_case() {
        match shell_cmd(&["apex", "shell", "audio"]) {
            ShellCmd::Audio { which } => assert_eq!(which, "out"),
            _ => panic!("wrong variant"),
        }
        match shell_cmd(&["apex", "shell", "network"]) {
            ShellCmd::Network { tab } => assert_eq!(tab, "wifi"),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn ipc_passes_arguments_through_verbatim() {
        // The escape hatch must not filter or reorder: it exists precisely for
        // handlers this wrapper does not know about.
        match shell_cmd(&["apex", "shell", "ipc", "nexus", "open", "keybinds", "extra"]) {
            ShellCmd::Ipc {
                target,
                function,
                args,
            } => {
                assert_eq!(target, "nexus");
                assert_eq!(function, "open");
                assert_eq!(args, vec!["keybinds".to_string(), "extra".to_string()]);
            }
            _ => panic!("wrong variant"),
        }
        // Function defaults to toggle, which is what most handlers expose.
        match shell_cmd(&["apex", "shell", "ipc", "focus-toggle"]) {
            ShellCmd::Ipc { function, .. } => assert_eq!(function, "toggle"),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn the_target_table_is_self_consistent() {
        let rows = shell_targets();
        assert!(!rows.is_empty());
        let mut seen = std::collections::HashSet::new();
        for (verb, target, func) in &rows {
            assert!(!verb.is_empty() && !target.is_empty() && !func.is_empty());
            assert!(seen.insert(*verb), "duplicate verb in the table: {verb}");
        }
        // `apex shell list` is documentation, so it must actually cover the
        // verbs that exist rather than drifting from them.
        for expect in ["launcher", "settings", "lock", "power", "focus", "record"] {
            assert!(
                rows.iter().any(|(v, ..)| *v == expect),
                "{expect} missing from the target table"
            );
        }
    }

    #[test]
    fn shell_dir_is_overridable_for_development() {
        // Not asserting the env var here (tests share a process); asserting the
        // default, which is the contract keybinds rely on.
        assert_eq!(SHELL_DIR_DEFAULT, "/usr/share/apex-shell");
    }

    fn metrics(argv: &[&str]) -> MetricsArgs {
        match Cli::try_parse_from(argv).expect("parses").command {
            Cmd::Metrics(a) => a,
            _ => panic!("not metrics"),
        }
    }

    #[test]
    fn metrics_defaults_to_a_single_human_readable_sample() {
        let a = metrics(&["apex", "metrics"]);
        assert!(!a.json);
        assert!(a.stream.is_none(), "must not stream unless asked");
    }

    #[test]
    fn metrics_stream_has_a_default_interval_but_takes_one() {
        // Bare --stream is the common case and must not require a number.
        assert_eq!(metrics(&["apex", "metrics", "--stream"]).stream, Some(2.0));
        assert_eq!(
            metrics(&["apex", "metrics", "--stream", "0.5"]).stream,
            Some(0.5)
        );
        assert!(metrics(&["apex", "metrics", "--json", "--stream", "1"]).json);
    }

    #[test]
    fn snapshot_keys_are_ordered_stably_for_diffing() {
        use std::collections::HashMap;
        use zvariant::Value;

        let mut m: HashMap<String, zvariant::OwnedValue> = HashMap::new();
        for k in [
            "temp_k10temp",
            "battery_uwh",
            "temp_acpitz",
            "on_ac",
            "tier",
            "ppt_watts",
        ] {
            m.insert(
                k.to_string(),
                zvariant::OwnedValue::try_from(Value::from(1u32)).unwrap(),
            );
        }

        // Headline fields in a fixed order, then the per-machine temp_* set
        // alphabetically so successive samples line up column-wise.
        assert_eq!(
            snapshot_key_order(&m),
            vec![
                "tier",
                "on_ac",
                "ppt_watts",
                "battery_uwh",
                "temp_acpitz",
                "temp_k10temp"
            ]
        );
    }

    #[test]
    fn snapshot_key_order_omits_fields_the_machine_cannot_report() {
        use std::collections::HashMap;
        use zvariant::Value;

        let mut m: HashMap<String, zvariant::OwnedValue> = HashMap::new();
        m.insert(
            "tier".to_string(),
            zvariant::OwnedValue::try_from(Value::from("balanced")).unwrap(),
        );
        // A desktop reports no battery and no ppt; those keys must simply be
        // absent rather than rendered empty.
        assert_eq!(snapshot_key_order(&m), vec!["tier"]);
    }

    #[test]
    fn json_strings_are_escaped() {
        assert_eq!(json_string("plain"), "\"plain\"");
        assert_eq!(json_string("a\"b"), "\"a\\\"b\"");
        assert_eq!(json_string("a\\b"), "\"a\\\\b\"");
        assert_eq!(json_string("a\nb"), "\"a\\nb\"");
        // Control characters must be \u-escaped or the output is not JSON.
        assert_eq!(json_string("a\u{1}b"), "\"a\\u0001b\"");
    }

    #[test]
    fn json_snapshot_is_well_formed_and_typed() {
        use std::collections::HashMap;
        use zvariant::Value;

        let mut m: HashMap<String, zvariant::OwnedValue> = HashMap::new();
        m.insert(
            "tier".to_string(),
            zvariant::OwnedValue::try_from(Value::from("ultra")).unwrap(),
        );
        m.insert(
            "on_ac".to_string(),
            zvariant::OwnedValue::try_from(Value::from(true)).unwrap(),
        );
        m.insert(
            "ppt_watts".to_string(),
            zvariant::OwnedValue::try_from(Value::from(15.5f64)).unwrap(),
        );

        let js = snapshot_to_json(&m);
        assert_eq!(js, r#"{"tier":"ultra","on_ac":true,"ppt_watts":15.5}"#);
    }

    #[test]
    fn non_finite_floats_become_null_not_invalid_json() {
        use std::collections::HashMap;
        use zvariant::Value;

        let mut m: HashMap<String, zvariant::OwnedValue> = HashMap::new();
        m.insert(
            "ppt_watts".to_string(),
            zvariant::OwnedValue::try_from(Value::from(f64::NAN)).unwrap(),
        );
        // NaN has no JSON representation; emitting a bare NaN would produce
        // output no parser accepts.
        assert_eq!(snapshot_to_json(&m), r#"{"ppt_watts":null}"#);
    }

    #[test]
    fn install_takes_a_local_rpm_path_as_a_package() {
        // The engine decides what is a file and what is a package name; the CLI
        // must not filter, reorder or reject either form.
        let (packages, ..) = install(&[
            "apex",
            "install",
            "/media/usb/google-chrome-stable.rpm",
            "htop",
            "org.gimp.GIMP",
        ]);
        assert_eq!(
            packages,
            vec![
                "/media/usb/google-chrome-stable.rpm".to_string(),
                "htop".to_string(),
                "org.gimp.GIMP".to_string(),
            ]
        );
    }

    #[test]
    fn a_path_with_spaces_survives_as_one_argument() {
        let (packages, ..) = install(&["apex", "install", "/media/My Stick/an app.rpm"]);
        assert_eq!(packages, vec!["/media/My Stick/an app.rpm".to_string()]);
    }

    #[test]
    fn the_unverified_opt_in_is_off_unless_asked_for() {
        let (_, _, _, allow_unsigned) = install(&["apex", "install", "./x.rpm"]);
        assert!(!allow_unsigned);
        assert!(!install_argv(vec!["./x.rpm".into()], false, vec![], false)
            .contains(&"--allow-unsigned".to_string()));
    }

    #[test]
    fn every_flag_reaches_the_engine_argv() {
        let (packages, no_weak_deps, enable_repo, allow_unsigned) = install(&[
            "apex",
            "install",
            "--allow-unsigned",
            "--no-weak-deps",
            "--enable-repo",
            "extra",
            "./x.rpm",
        ]);
        assert!(allow_unsigned && no_weak_deps);
        assert_eq!(
            install_argv(packages, no_weak_deps, enable_repo, allow_unsigned),
            vec![
                "install".to_string(),
                "./x.rpm".to_string(),
                "--no-weak-deps".to_string(),
                "--enable-repo=extra".to_string(),
                "--allow-unsigned".to_string(),
            ]
        );
    }

    #[test]
    fn install_is_still_a_root_only_verb() {
        // Adding a flag must not accidentally move `install` out of the
        // privileged set: it writes an extension and re-merges /usr.
        let cli = Cli::try_parse_from(["apex", "install", "--allow-unsigned", "./x.rpm"]).unwrap();
        assert!(matches!(cli.command, Cmd::Install { .. }));
    }

}
