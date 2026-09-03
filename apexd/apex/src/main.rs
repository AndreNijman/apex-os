//! `apex` — the APEX-OS control CLI. A thin client over the frozen
//! `org.apexos.Apexd1` D-Bus API, with read-only local fallbacks (via
//! `apexd-core`) so `fingerprint`, `status`, `profile`, `doctor` and dry-run
//! tier planning work even when `apexd` is not running. Every D-Bus verb
//! degrades gracefully — a clear message, a non-zero exit, never a panic.

mod agent;
mod ai;
mod blueprint;
mod boot;
mod dispatch;
mod disposable;
mod gaming;
mod host;
mod mode;
mod ops;
mod proxy;
mod recover;
mod request;
mod secret;
mod task;
mod touchpad;

use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
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
    /// Whether this machine can boot straight into a controller-first Gaming
    /// Mode, and whether it is set to.
    ///
    /// Read-only. `apex game` is the hardware lever (cpuset, IRQ steering, GPU
    /// clocks); this is the §12 experience around it — the greeter's Gaming
    /// Mode entry, the gamescope session, the Desktop<->Gaming switch and any
    /// attached controllers. Exits non-zero when Gaming Mode would not start,
    /// so it is usable as a check.
    Gaming(gaming::GamingArgs),
    /// What verified this boot, and what the boot counter believes (§22).
    ///
    /// Read-only. GRUB is the default bootloader for every published APEX
    /// image in this generation, and `status` reports that as the normal state
    /// rather than as a fault: boot counting, signed UKIs and TPM-bound unlock
    /// are the opt-in systemd-boot path, and this is the command that says
    /// which of them is actually in effect on this machine.
    Boot {
        #[command(subcommand)]
        cmd: boot::BootCmd,
    },
    /// Local model inference as an OS service (§14).
    ///
    /// One endpoint every application and agent client can use — a Unix socket
    /// in your `$XDG_RUNTIME_DIR` speaking the runtime's own
    /// OpenAI-compatible HTTP API — with APEX owning the model store, the
    /// backend choice (CUDA, ROCm, Vulkan or CPU), how much fits in VRAM, and
    /// when an idle model is unloaded.
    ///
    /// The service is **per-user**, like `apex agent`, and for a stronger
    /// reason: it turns your prompts into generated text, so it must not be a
    /// privileged daemon shared between accounts. The weights are shared
    /// instead — one root-owned, read-only copy under /var that no session can
    /// alter, including its own.
    ///
    /// What it deliberately does NOT do: listen on a TCP port. A TCP
    /// connection carries no peer credential, so a listener on 127.0.0.1 is
    /// reachable by every account on the machine and by every sandboxed
    /// application that holds the network permission. `--listen` exists only to
    /// say so. It also ships no inference runtime — llama.cpp with CUDA is
    /// gigabytes, and `Containerfile.core` is the tier a rebuild makes the
    /// whole fleet download — so `apex ai status` names the `apex install` or
    /// `apex env` command that provides one.
    ///
    /// Needs the per-user service: `systemctl --user enable --now apex-aid`.
    Ai {
        #[command(subcommand)]
        cmd: ai::AiCmd,
    },
    /// Trusted APEX devices, and what each one can do (§20).
    ///
    /// A device is named by an ssh destination — normally an alias already in
    /// `~/.ssh/config`, which is why there is no address, key or port to repeat
    /// here. APEX generates no key and holds no passphrase: authentication,
    /// host identity and transport are whatever `ssh <destination>` already
    /// does, including a ProxyCommand or a Match exec that picks a route per
    /// network.
    ///
    /// Capabilities are *probed*, never assumed. An APEX peer answers
    /// `apex host describe --json` with the same struct this side parses; a
    /// host that is not APEX gets a portable shell probe so `list` still says
    /// something true about it.
    Host(host::HostArgs),
    /// Build this project, here or on a trusted device (§20).
    ///
    /// Without `--on` it builds locally, running the same command it would
    /// dispatch — so a local run and a remote one cannot drift apart about
    /// what "the build" is. The command is detected from the project's marker
    /// files and *printed*, because a detector silently choosing between five
    /// possibilities is one nobody can correct; give it explicitly after `--`
    /// to override.
    ///
    /// The remote directory is assumed to be the same absolute path and then
    /// **verified** — it must exist and be the same repository, compared by
    /// `origin` URL — because the failure mode of a wrong guess is a build
    /// that succeeds against the wrong source.
    Build(dispatch::BuildArgs),
    /// Send files or the clipboard to a trusted device (§20).
    ///
    /// Files land under their own name, not the sender's directory layout, and
    /// an existing file is NOT overwritten unless `--force` says so: a send
    /// that replaced something on another machine is not recoverable from this
    /// end.
    Send(dispatch::SendArgs),
    /// Open a URL or a path on a trusted device's screen (§20).
    ///
    /// Needs a graphical session there, and checks for one: an ssh command has
    /// no session bus, and a machine sitting at its greeter would otherwise
    /// report success and open something nobody can see.
    Open(dispatch::OpenArgs),
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
    ///
    /// `--json` is §19's "expose `apex doctor` results graphically" from the OS
    /// side: the same checks, in the shape a UI renders. Not a second set of
    /// checks — the list is built once and rendered either way, because two
    /// diagnostic implementations disagree and the one the user reads would be
    /// the one wired to nothing.
    Doctor {
        /// Emit machine-readable JSON instead of PASS/WARN lines.
        #[arg(long)]
        json: bool,
    },
    /// Show the booted image and its changelog labels.
    Changelog,
    /// Install packages from the enabled repositories, Flathub, a capsule, or
    /// a local .rpm file. Requires root, except `--source capsule`.
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
        /// Pick the source yourself instead of letting APEX rank them:
        /// rpm (the system extension), flatpak, or capsule.
        ///
        /// Applies to bare names only. A path is always an RPM and an
        /// application id is always a Flatpak, so naming a source that
        /// contradicts one of those is refused rather than quietly resolved.
        /// `apex resolve <name>` shows what would happen without it.
        #[arg(long, value_name = "SOURCE")]
        source: Option<String>,
        /// Which capsule `--source capsule` installs into. Defaults to your
        /// first one.
        #[arg(long, value_name = "CAPSULE")]
        env: Option<String>,
    },
    /// Show which source APEX would install a name from, and why.
    ///
    /// Prints every candidate across the repositories, Flathub and your
    /// capsules, what vouches for each one, the choice APEX would make, and
    /// the exact command for every alternative. Read-only, so it needs no
    /// root — "what would this do" should never cost a password.
    Resolve {
        #[arg(value_name = "NAME")]
        name: String,
    },
    /// Remove packages installed with `apex install`. Requires root.
    Remove {
        #[arg(required = true, value_name = "PACKAGE")]
        packages: Vec<String>,
    },
    /// Search every package source: the enabled repositories and Flathub.
    ///
    /// `apex resolve <name>` then says which of them APEX would actually use
    /// for a given name, and why.
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
    /// APEX Capsules: development environments that leave the host alone.
    ///
    /// /usr is read-only and packages come from a system extension, which is
    /// the right shape for an operating system and the wrong one for
    /// ecosystems that expect a mutable userspace — `pip install --user`,
    /// `npm -g`, an SDK that wants /opt, a package manager that wants
    /// /etc/apt. A capsule gives each of those its own rootless container
    /// that still sees your home, your terminal and your devices.
    ///
    /// Unprivileged: capsules belong to you, not to the machine, so none of
    /// these verbs needs (or accepts) root.
    Env {
        #[command(subcommand)]
        cmd: EnvCmd,
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
    /// APEX Shell plugins: what is installed, and whether the shell will load it.
    ///
    /// The shell's plugin platform (§16) owns every rule about a manifest — the
    /// permission vocabulary, which permissions apiVersion 1 will actually
    /// grant, the import allowlist, the forbidden constructs. This command asks
    /// that validator rather than reimplementing it, so a verdict here is the
    /// verdict the shell will reach. If the shell is not installed, it refuses
    /// instead of guessing.
    ///
    /// Unprivileged: plugins live in your own `~/.config/apex-shell/plugins`.
    Plugin {
        #[command(subcommand)]
        cmd: PluginCmd,
    },
    /// Projects, agent worktrees and checkpoints.
    Project {
        #[command(subcommand)]
        cmd: agent::ProjectCmd,
    },
    /// What you are working on: the binder that can be put down and picked back
    /// up (§21).
    ///
    /// A task NAMES a project, a capsule, an agent worktree and the agents you
    /// run, and `apex task resume` checks that every one of them is still there
    /// before telling you how to continue — a task whose capsule was deleted or
    /// whose worktree was removed is refused by name rather than half-resumed.
    ///
    /// It creates none of those things and it grants nothing. There is
    /// deliberately no window list (windows come from
    /// `apex project layout save`) and no permission of any kind: §4's brokers
    /// own those, and a permission in a hand-editable file would be a grant
    /// nobody reviewed.
    ///
    /// Unprivileged: a task is yours, kept in your own `~/.config/apex` and
    /// `~/.local/state/apex`, so none of these verbs needs (or accepts) root.
    Task(task::TaskArgs),
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

    /// The declarative APEX Blueprint: what this machine should be.
    ///
    /// One TOML file — `~/.config/apex/blueprint.toml` — describes the desktop,
    /// applications, development languages, agent defaults and gaming. `apex
    /// blueprint diff` shows how the machine differs from it and `apex apply`
    /// converges it.
    ///
    /// The blueprint is yours: nothing in APEX rewrites it. `apex apply` writes
    /// its own record somewhere else (`apex blueprint show` prints where), so
    /// generated state and the file you edit never share a path.
    Blueprint {
        #[command(subcommand)]
        cmd: BlueprintCmd,
    },

    /// Converge this machine toward its blueprint.
    ///
    /// Idempotent: running it twice does nothing the second time, because the
    /// plan is recomputed from a fresh measurement of the machine every time
    /// rather than from a record of what was done last.
    ///
    /// It converges only the privilege domain it is already running in and
    /// reports the other. `apex apply` sets your desktop colour scheme and
    /// agent defaults; `sudo apex apply` selects the session and installs
    /// applications. Nothing here ever calls sudo itself, so `apply` cannot
    /// raise an authentication prompt — and a root run can never write
    /// root-owned files into your ~/.config.
    ///
    /// Applications are added, never removed. A package missing from the
    /// blueprint is left alone: reading a deleted line as "uninstall it" turns
    /// an edit into data loss.
    ///
    /// Setting APEX_BLUEPRINT_NO_APPLY to any non-empty value makes this refuse
    /// to change anything. --dry-run keeps working with it set.
    Apply(ApplyArgs),

    /// Carry settings, applications and projects to another APEX machine.
    ///
    /// `apex sync export` writes one file; `apex sync import` reads it on the
    /// other machine. The bundle carries the blueprint, which projects exist
    /// and where they came from, and nothing else — no credentials of any
    /// kind, because this is a file people put in a git repository.
    ///
    /// `import` never converges anything. It writes the blueprint and records
    /// the projects, and leaves `apex blueprint diff` and `apex apply` as
    /// separate decisions.
    Sync {
        #[command(subcommand)]
        cmd: SyncCmd,
    },

    /// Recovery, repair and rollback, in one surface (§19).
    ///
    /// `status` reports every component §19 names — the booted deployment, the
    /// rollback target, Secure Boot, the filesystem, the GPU driver, APEX
    /// Shell, the network and the package extension — with the action that
    /// addresses each one. It spawns no subprocess and contacts nothing, so it
    /// is safe for APEX Settings to poll and can never raise an authentication
    /// prompt.
    ///
    /// `repair` runs only steps that are idempotent and remove no data.
    /// `reset` is the scoped factory reset, and it is a dry run unless it is
    /// given both --commit and a token derived from the plan it printed.
    ///
    /// Rolling back is `apex rollback`, which already exists; this surface
    /// makes it visible rather than adding a second name for it.
    Recover {
        #[command(subcommand)]
        cmd: recover::RecoverCmd,
    },

    /// Disposable environments: a capsule that is deleted when you close it (§19).
    ///
    /// A mode of `apex env`, not a second mechanism. Every disposable
    /// environment is an ordinary APEX capsule created through the same engine
    /// and visible to `apex env list` — it just gets its own throwaway home,
    /// an explicit copy-in/copy-out boundary, and a teardown that removes the
    /// container and the directory together.
    ///
    /// It is a disposable ENVIRONMENT, not a security boundary: distrobox
    /// mounts the host filesystem at /run/host inside every capsule, and
    /// `apex disposable plan` prints that in full before anything starts. For
    /// confinement — a default-deny mount namespace with $HOME masked — the
    /// mechanism is `apex agent`'s sandbox.
    ///
    /// Unprivileged, like `apex env`, and for the same reason.
    Disposable {
        #[command(subcommand)]
        cmd: disposable::DisposableCmd,
    },
}

#[derive(Subcommand)]
enum SyncCmd {
    /// Write a bundle for another machine. Prints to stdout without --output.
    Export {
        /// Where to write it. Omit to print to stdout.
        #[arg(long, short, value_name = "PATH")]
        output: Option<PathBuf>,
        /// Export this blueprint rather than the one on the search path.
        #[arg(long, value_name = "PATH")]
        file: Option<PathBuf>,
        /// Leave projects out. A project entry carries a local path and a git
        /// remote, which is the only machine-specific data in a bundle.
        #[arg(long)]
        no_projects: bool,
    },
    /// Print a bundle without importing it.
    Show {
        #[arg(value_name = "PATH")]
        path: PathBuf,
    },
    /// Install a bundle's blueprint and record its projects. Converges nothing.
    Import {
        #[arg(value_name = "PATH")]
        path: PathBuf,
        /// Replace an existing blueprint that differs. The current one is kept
        /// alongside it as blueprint.toml.previous.
        #[arg(long)]
        force: bool,
    },
}

#[derive(Args)]
struct ApplyArgs {
    /// Read this blueprint instead of the usual search path.
    #[arg(long, value_name = "PATH")]
    file: Option<PathBuf>,
    /// Report exactly what would change and perform none of it.
    ///
    /// The plan is computed once, so this prints the same steps a live run
    /// executes — it is a report, not a rehearsal of a different code path.
    #[arg(long)]
    dry_run: bool,
    /// Emit the plan as JSON.
    #[arg(long)]
    json: bool,
}

#[derive(Subcommand)]
enum BlueprintCmd {
    /// Print the blueprint, where it came from, and when it was last applied.
    Show {
        /// Read this file instead of the usual search path.
        #[arg(long, value_name = "PATH")]
        file: Option<PathBuf>,
        /// Emit JSON.
        #[arg(long)]
        json: bool,
    },
    /// How the machine currently differs from the blueprint.
    ///
    /// Exits 0 when converged and 1 when there is drift `apex apply` could
    /// close, so it reads like `diff(1)` in a script. Changes that cannot be
    /// converged at all — a Gaming edition asked of a Daily machine — are
    /// reported but do not set the exit code, because no number of `apply`
    /// runs would ever clear them.
    Diff {
        #[arg(long, value_name = "PATH")]
        file: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    /// Write a commented starting blueprint to ~/.config/apex/blueprint.toml.
    ///
    /// Every section arrives commented out, so the new file manages nothing
    /// until it is edited.
    Init {
        /// Overwrite an existing blueprint.
        #[arg(long)]
        force: bool,
    },
    /// Replace the blueprint with one supplied as JSON on stdin.
    ///
    /// The write path for §10's GUI editor. Without it the shell would have to
    /// author TOML itself, which means a second implementation of the schema
    /// that drifts the first time a field is added — and the lossless
    /// round-trip is the property the whole design rests on.
    ///
    /// The JSON goes through the same normalise + validate as a hand-edited
    /// file, so what the editor writes is indistinguishable from what a human
    /// types, and an invalid one is refused with the same messages. It writes
    /// desired state only: it converges nothing, never touches the generated
    /// applied-state file, and does not escalate.
    ///
    ///     apex blueprint show --json | jq … | apex blueprint set --json -
    Set {
        /// Read the blueprint as JSON from this source. Only `-` (stdin) is
        /// supported: a path would invite passing the live blueprint's own
        /// path and truncating it mid-read.
        #[arg(long, value_name = "-")]
        json: Option<String>,
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

/// `apex env <verb>` — the capsule surface (§8).
///
/// A separate enum rather than a raw argument passthrough so that `apex env
/// --help` documents the real thing and a typo is caught before a process is
/// spawned. The engine still owns every decision; this only builds its argv.
#[derive(Subcommand)]
enum EnvCmd {
    /// Create a capsule.
    ///
    /// A name that is also an image alias (fedora, ubuntu, arch, debian,
    /// python, cuda, rocm) brings that alias's image and device profile with
    /// it, so `apex env create cuda` is a capsule that can see the GPU.
    Create {
        #[arg(value_name = "NAME")]
        name: String,
        /// Any container image reference, instead of the alias's default.
        #[arg(long, value_name = "REF")]
        image: Option<String>,
        /// Device access: nvidia (host driver passthrough), amd (/dev/kfd and
        /// the render group, for ROCm), hw (USB buses, for hardware work), or
        /// none. Defaults to none — a capsule holding a device open is a
        /// capsule that stops the machine suspending.
        #[arg(long, value_name = "PROFILE")]
        gpu: Option<String>,
        /// Give the capsule its own home directory instead of sharing yours.
        /// For ecosystems whose caches litter $HOME badly enough to contain.
        #[arg(long, value_name = "DIR")]
        home: Option<String>,
    },
    /// Capsules on this machine, with their image and device profile.
    List {
        #[arg(long)]
        json: bool,
    },
    /// The full record for one capsule: image, digest, profile, package manager.
    Info {
        #[arg(value_name = "NAME")]
        name: String,
    },
    /// Open an interactive shell in a capsule.
    Enter {
        #[arg(value_name = "NAME")]
        name: String,
        /// Run this instead of a login shell.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<String>,
    },
    /// Run one command in a capsule with no TTY. For scripts and agents.
    Exec {
        #[arg(value_name = "NAME")]
        name: String,
        #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<String>,
    },
    /// Install packages with the capsule's own package manager.
    ///
    /// This is what `apex install --source capsule` routes to: software that
    /// exists only as a package for a distribution APEX is not.
    Install {
        #[arg(value_name = "NAME")]
        name: String,
        #[arg(required = true, value_name = "PACKAGE")]
        packages: Vec<String>,
    },
    /// Remove a capsule. Only ones APEX created, unless --force.
    Rm {
        #[arg(value_name = "NAME")]
        name: String,
        /// Say where a custom home directory is rather than reporting it gone.
        #[arg(long)]
        keep_home: bool,
        /// Remove a container APEX has no record of.
        #[arg(long)]
        force: bool,
    },
    /// The image aliases and what they resolve to on this release.
    Images,
    /// Put a GUI application from a capsule into the host's launcher (§8).
    ///
    /// `distrobox-export` runs INSIDE the capsule and writes the .desktop file
    /// into your own `~/.local/share/applications`, so this needs no root and
    /// cannot raise an authentication prompt. `apex env rm` takes the launcher
    /// entry with it.
    Export {
        #[arg(value_name = "NAME")]
        name: String,
        /// The application as the capsule knows it — a bare name, not a path.
        #[arg(value_name = "APPLICATION")]
        app: String,
    },
    /// Take an exported application back out of the host's launcher.
    Unexport {
        #[arg(value_name = "NAME")]
        name: String,
        #[arg(value_name = "APPLICATION")]
        app: String,
    },
    /// What a capsule has exported, as distrobox sees it and as APEX recorded it.
    Exports {
        #[arg(value_name = "NAME")]
        name: String,
    },
    /// Make a capsule that provides a language, and record that it does.
    ///
    /// This is what `apex apply` runs for the blueprint's `[development]
    /// languages`. A toolchain goes into a capsule, never onto the read-only
    /// host — that is the whole point of §8. The language is recorded only
    /// after the toolchain answers from inside the capsule.
    Provision {
        #[arg(value_name = "LANGUAGE")]
        language: String,
    },
    /// The language table: which capsule provides what, and from which packages.
    Languages,
}

/// `apex plugin <verb>` — the OS side of §16's plugin platform.
///
/// A separate enum rather than an argument passthrough, for the same reason
/// `EnvCmd` is one: `apex plugin --help` documents the real surface and a typo
/// is caught before a process is spawned.
#[derive(Subcommand)]
enum PluginCmd {
    /// Installed plugins, whether each one is valid, and why not.
    List {
        #[arg(long)]
        json: bool,
    },
    /// One plugin in full: its grant, its permissions, or its refusal reason.
    Info {
        #[arg(value_name = "ID")]
        id: String,
    },
    /// Move a plugin into the directory the shell scans.
    ///
    /// The shell scans exactly one directory and has no allowlist file, so this
    /// is a directory move — which is what actually takes effect against the
    /// shipped shell. It takes effect at the next shell start; nothing here can
    /// load a plugin into a running shell.
    Enable {
        #[arg(value_name = "ID")]
        id: String,
    },
    /// Move a plugin out of the directory the shell scans.
    ///
    /// Nothing is deleted and no file is rewritten. The running shell keeps a
    /// plugin it has already loaded until it restarts.
    Disable {
        #[arg(value_name = "ID")]
        id: String,
    },
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
    /// Per-game profiles (roadmap §12): a stored composition of a mode, a tier
    /// and a fan mode, per title.
    ///
    /// Stored in `~/.config/apex/games.toml` — a separate user-owned file
    /// rather than a blueprint section, because the blueprint's contract is
    /// that no program rewrites it and `set` is a program that writes.
    Profile {
        #[command(subcommand)]
        cmd: gaming::ProfileCmd,
    },
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

/// Which verbs refuse to run without root, and under what name.
///
/// A function rather than a `match` inside `main` so the privilege set is an
/// assertion instead of a reading exercise. It covers exactly these and not
/// the whole CLI: the desktop's power tab drives `apex tier` as the session
/// user, and every read-only verb has to stay usable without a password.
/// See `ops::require_root` for what the refusal says.
fn privileged_verb(cmd: &Cmd) -> Option<&'static str> {
    match cmd {
        Cmd::Update(_) => Some("update"),
        Cmd::Rollback => Some("rollback"),
        Cmd::Pin => Some("pin"),
        // `--source capsule` writes nothing the system owns: it installs into
        // a rootless per-user container. Demanding root for it would be worse
        // than pointless — root has no capsules, so `sudo apex install
        // --source capsule` reports an empty list on every machine, and the
        // user who typed sudo because the CLI asked for it gets a refusal from
        // the engine instead of a package.
        Cmd::Install {
            source: Some(s), ..
        } if s == "capsule" => None,
        // Package verbs that write: they build an extension into /var/lib and
        // ask systemd to re-merge /usr. The read-only ones (list/status/info/
        // verify), `search` and `resolve` stay usable as an ordinary user on
        // purpose.
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
        // `apex env` is deliberately absent: capsules are rootless per-user
        // containers, and running one as root would put its images under
        // /var/lib/containers, share it between every account, and need an
        // authentication prompt to open a shell.
        _ => None,
    }
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    // Root-only verbs bail HERE — before any sysfs probe, D-Bus connect or
    // subprocess — so an unprivileged `apex update` costs nothing and answers
    // instantly with the command to run instead.
    if let Some(verb) = privileged_verb(&cli.command) {
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
        // Reads the task file, the capsule engine's records, the project's
        // checkpoints and the agent runtime's session list; writes only the
        // task file and the task's own state file. No D-Bus, no root, and
        // nothing that can raise a prompt — routed here, before anything
        // connects to the system bus, for the reason `apex ai` is.
        Cmd::Task(args) => task::run(args),
        Cmd::Request { cmd } => request::main(cmd),
        Cmd::Secret { cmd } => secret::main(cmd),
        // Read-only, so no root gate: seeing what the machine should be must
        // not require privilege. `apex apply` is the verb that changes things,
        // and it converges only the privilege domain it is already in.
        Cmd::Blueprint { cmd } => match cmd {
            BlueprintCmd::Show { file, json } => blueprint::cmd_show(file.as_deref(), json),
            BlueprintCmd::Diff { file, json } => blueprint::cmd_diff(file.as_deref(), json),
            BlueprintCmd::Init { force } => blueprint::cmd_init(force),
            BlueprintCmd::Set { json } => blueprint::cmd_set(json.as_deref() == Some("-")),
        },
        // Deliberately NOT in the root-only list above. `apply` is a mixed
        // verb: it converges the domain it is in and reports the other, so
        // gating the whole command on root would make the user half — colour
        // scheme, agent defaults — reachable only by running it as the wrong
        // user, which is precisely the mistake the domain split exists to
        // prevent.
        Cmd::Apply(args) => blueprint::cmd_apply(args.file.as_deref(), args.dry_run, args.json),
        // `sync` writes only the user's own blueprint and project records, so
        // it needs no privilege and must not ask for any.
        Cmd::Sync { cmd } => match cmd {
            SyncCmd::Export {
                output,
                file,
                no_projects,
            } => blueprint::cmd_sync_export(file.as_deref(), output.as_deref(), no_projects),
            SyncCmd::Show { path } => blueprint::cmd_sync_show(&path),
            SyncCmd::Import { path, force } => blueprint::cmd_sync_import(&path, force),
        },
        Cmd::Tier { name } => cmd_tier(name).await,
        Cmd::Profile => cmd_profile().await,
        Cmd::Battery(args) => cmd_battery(args).await,
        Cmd::Fan { cmd } => cmd_fan(cmd.unwrap_or(FanCmd::Status)).await,
        // `profile` is intercepted HERE rather than inside `cmd_game`, and the
        // reason is the guard: `cmd_game` connects to the system bus as its
        // first act, before it looks at the verb at all. Routing
        // `profile apply` through it would put a bus connection ahead of
        // APEX_MODE_NO_APPLY, so on a machine with no bus the command would
        // fail for the wrong reason and the guard's ordering proof would be
        // vacuous. `apex mode set` has the same rule; this is the same rule.
        Cmd::Game { cmd } => match cmd {
            GameCmd::Profile { cmd } => gaming::profile_main(cmd).await,
            other => cmd_game(other).await,
        },
        // Read-only by default and deliberately absent from the privileged set:
        // `mode set` mutates through apexd's polkit-authorised D-Bus API as the
        // session user, exactly as `apex tier` does.
        Cmd::Mode { cmd } => mode::main(cmd.unwrap_or(mode::ModeCmd::Status)).await,
        Cmd::Workload(args) => mode::workload_main(args),
        Cmd::Perf(args) => mode::perf_main(args),
        // Read-only, like `perf` and `workload`, and for the same reason it is
        // not in the privileged set: it measures and reports.
        Cmd::Gaming(args) => gaming::gaming_main(args),
        // Also read-only, and deliberately not in the privileged set even
        // though the boot chain is the most privileged thing on the machine.
        // Reading the ESP does need root, and `status` reports that as
        // "unavailable, and why" rather than demanding a password to answer
        // "what verified my boot".
        Cmd::Boot { cmd } => boot::boot_main(cmd),
        // Read-only except for `add`/`remove`/`probe`, which write only the
        // registry and the probe cache in the user's own home. Nothing here
        // touches apexd or needs root.
        // Routed here, before anything connects to the system bus: `apex ai`
        // talks to a per-user daemon and to the model store, never to `apexd`,
        // so a system-bus connection ahead of it would be a dependency the
        // feature does not have — and on a machine with no `apexd` it would be
        // a failure the user cannot act on.
        Cmd::Ai { cmd } => ai::main(cmd),
        Cmd::Host(args) => host::run(args),
        // Local by default; `--on` is the only thing that makes any of these
        // touch the network. None of them needs root: they run ssh as the
        // invoking user and write nothing outside the user's own home.
        Cmd::Build(args) => dispatch::build(args),
        Cmd::Send(args) => dispatch::send(args),
        Cmd::Open(args) => dispatch::open(args),
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
        Cmd::Doctor { json } => cmd_doctor(json).await,
        // Read-only and subprocess-free, so deliberately not in the privileged
        // set: "what state is my machine in, and how do I get back" must never
        // cost a password. `repair` converges the domain it is already in and
        // reports the other, and `reset` refuses to run as root outright —
        // root's home is not the user's, so a `sudo` run would reset the wrong
        // account while reporting success.
        Cmd::Recover { cmd } => recover::main(cmd),
        // Unprivileged for the same structural reason `apex env` is: a
        // disposable capsule is a rootless per-user container.
        Cmd::Disposable { cmd } => ops::disposable(&disposable::argv(cmd)),
        Cmd::Changelog => ops::changelog(),
        Cmd::Install {
            packages,
            no_weak_deps,
            enable_repo,
            allow_unsigned,
            source,
            env,
        } => ops::pkg(&install_argv(
            packages,
            no_weak_deps,
            enable_repo,
            allow_unsigned,
            source,
            env,
        )),
        Cmd::Resolve { name } => ops::pkg(&["resolve".to_string(), name]),
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
        Cmd::Env { cmd } => ops::env(&env_argv(cmd)),
        Cmd::Plugin { cmd } => ops::plugin(&plugin_argv(cmd)),
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
    source: Option<String>,
    env: Option<String>,
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
    // The engine validates the value and refuses an unknown one. Not
    // re-validated here: two lists of legal sources would be one list too
    // many, and the engine's is the one that decides.
    if let Some(s) = source {
        argv.push(format!("--source={s}"));
    }
    if let Some(e) = env {
        argv.push(format!("--env={e}"));
    }
    argv
}

/// Build the engine argv for `apex env`.
///
/// Split out for the same reason `install_argv` is: the engine is a separate
/// process, so a dropped flag is not a compile error but a silent policy
/// change. `--gpu` decides whether a capsule can see the GPU at all, and
/// `--force` decides whether `rm` will destroy a container APEX did not
/// create.
///
/// The `--` before a trailing command is not cosmetic. Without it the engine
/// cannot tell `apex env exec box -- ls -l` (run `ls -l`) from a flag of its
/// own, and clap has already stripped the separator the user typed.
fn env_argv(cmd: EnvCmd) -> Vec<String> {
    match cmd {
        EnvCmd::Create {
            name,
            image,
            gpu,
            home,
        } => {
            let mut a = vec!["create".to_string(), name];
            if let Some(i) = image {
                a.push(format!("--image={i}"));
            }
            if let Some(g) = gpu {
                a.push(format!("--gpu={g}"));
            }
            if let Some(h) = home {
                a.push(format!("--home={h}"));
            }
            a
        }
        EnvCmd::List { json } => {
            let mut a = vec!["list".to_string()];
            if json {
                a.push("--json".to_string());
            }
            a
        }
        EnvCmd::Info { name } => vec!["info".to_string(), name],
        EnvCmd::Enter { name, command } => {
            let mut a = vec!["enter".to_string(), name];
            if !command.is_empty() {
                a.push("--".to_string());
                a.extend(command);
            }
            a
        }
        EnvCmd::Exec { name, command } => {
            let mut a = vec!["exec".to_string(), name, "--".to_string()];
            a.extend(command);
            a
        }
        EnvCmd::Install { name, packages } => {
            let mut a = vec!["install".to_string(), name];
            a.extend(packages);
            a
        }
        EnvCmd::Rm {
            name,
            keep_home,
            force,
        } => {
            let mut a = vec!["rm".to_string(), name];
            if keep_home {
                a.push("--keep-home".to_string());
            }
            if force {
                a.push("--force".to_string());
            }
            a
        }
        EnvCmd::Images => vec!["images".to_string()],
        EnvCmd::Export { name, app } => vec!["export".to_string(), name, app],
        EnvCmd::Unexport { name, app } => vec!["unexport".to_string(), name, app],
        EnvCmd::Exports { name } => vec!["exports".to_string(), name],
        EnvCmd::Provision { language } => vec!["provision".to_string(), language],
        EnvCmd::Languages => vec!["languages".to_string()],
    }
}

fn plugin_argv(cmd: PluginCmd) -> Vec<String> {
    match cmd {
        PluginCmd::List { json } => {
            let mut a = vec!["list".to_string()];
            if json {
                a.push("--json".to_string());
            }
            a
        }
        PluginCmd::Info { id } => vec!["info".to_string(), id],
        PluginCmd::Enable { id } => vec!["enable".to_string(), id],
        PluginCmd::Disable { id } => vec!["disable".to_string(), id],
    }
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
        // Normally unreachable: the dispatch in `main` routes `profile` here
        // BEFORE this function, because everything above has already connected
        // to the system bus and `apex game profile apply`'s guard is only
        // meaningful when it is reached first.
        //
        // Routed rather than panicked on, because this crate's contract is a
        // clear message and a non-zero exit, never a panic — and routed rather
        // than wildcarded, so adding a verb to `GameCmd` is still a compile
        // error here. Reaching it costs a wasted bus connection and nothing
        // else: a connection raises no polkit prompt, and no mutating method
        // has been called at this point.
        GameCmd::Profile { cmd } => gaming::profile_main(cmd).await,
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

async fn cmd_doctor(json: bool) -> i32 {
    let v = LocalView::detect();
    let conn = connect().await;
    let running = match &conn {
        Some(c) => daemon_running(c).await,
        None => false,
    };

    // The checks themselves live in `recover`, so §19's graphical surface and
    // this command are the same list rendered twice rather than two lists that
    // can disagree. Everything except the metrics probe is a file read, and
    // the probe stays here because it is the one check that needs a socket.
    let mut checks = recover::doctor_checks(&v, running);
    let metrics_up = TcpStream::connect_timeout(
        &"127.0.0.1:9723".parse::<SocketAddr>().unwrap(),
        Duration::from_millis(200),
    )
    .is_ok();
    checks.push(recover::Check {
        ok: metrics_up,
        what: "metrics endpoint reachable on 127.0.0.1:9723".to_string(),
    });

    print!("{}", recover::render_doctor(&checks, json));
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

/// `pub(crate)` because the doctor's checks moved to `recover`, where §19's
/// JSON rendering of them lives. The reader stays here rather than being
/// duplicated: two `/sys` readers with different trimming rules would answer
/// the same question two ways.
pub(crate) fn read_sys(rel: &str) -> Option<String> {
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

    /// The parsed pieces of an `apex install`, in the order `install_argv`
    /// takes them.
    struct Install {
        packages: Vec<String>,
        no_weak_deps: bool,
        enable_repo: Vec<String>,
        allow_unsigned: bool,
        source: Option<String>,
        env: Option<String>,
    }

    fn install(argv: &[&str]) -> Install {
        match Cli::try_parse_from(argv).expect("parses").command {
            Cmd::Install {
                packages,
                no_weak_deps,
                enable_repo,
                allow_unsigned,
                source,
                env,
            } => Install {
                packages,
                no_weak_deps,
                enable_repo,
                allow_unsigned,
                source,
                env,
            },
            _ => panic!("not an install"),
        }
    }

    /// The engine argv an `apex install` command line produces.
    fn install_engine_argv(argv: &[&str]) -> Vec<String> {
        let i = install(argv);
        install_argv(
            i.packages,
            i.no_weak_deps,
            i.enable_repo,
            i.allow_unsigned,
            i.source,
            i.env,
        )
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
        let i = install(&[
            "apex",
            "install",
            "/media/usb/google-chrome-stable.rpm",
            "htop",
            "org.gimp.GIMP",
        ]);
        assert_eq!(
            i.packages,
            vec![
                "/media/usb/google-chrome-stable.rpm".to_string(),
                "htop".to_string(),
                "org.gimp.GIMP".to_string(),
            ]
        );
    }

    #[test]
    fn a_path_with_spaces_survives_as_one_argument() {
        let i = install(&["apex", "install", "/media/My Stick/an app.rpm"]);
        assert_eq!(i.packages, vec!["/media/My Stick/an app.rpm".to_string()]);
    }

    #[test]
    fn the_unverified_opt_in_is_off_unless_asked_for() {
        assert!(!install(&["apex", "install", "./x.rpm"]).allow_unsigned);
        assert!(!install_engine_argv(&["apex", "install", "./x.rpm"])
            .contains(&"--allow-unsigned".to_string()));
    }

    #[test]
    fn every_flag_reaches_the_engine_argv() {
        assert_eq!(
            install_engine_argv(&[
                "apex",
                "install",
                "--allow-unsigned",
                "--no-weak-deps",
                "--enable-repo",
                "extra",
                "./x.rpm",
            ]),
            vec![
                "install".to_string(),
                "./x.rpm".to_string(),
                "--no-weak-deps".to_string(),
                "--enable-repo=extra".to_string(),
                "--allow-unsigned".to_string(),
            ]
        );
    }

    // ── §9: the resolver's escape hatch ─────────────────────────────────────

    #[test]
    fn no_source_is_named_unless_the_user_named_one() {
        // The empty case is the one that matters: `apex install htop` must
        // reach the engine exactly as it did before the resolver existed, or
        // this is a behaviour change on a shipped command.
        assert_eq!(
            install_engine_argv(&["apex", "install", "htop"]),
            vec!["install".to_string(), "htop".to_string()]
        );
    }

    #[test]
    fn the_chosen_source_reaches_the_engine() {
        assert_eq!(
            install_engine_argv(&["apex", "install", "--source", "flatpak", "discord"]),
            vec![
                "install".to_string(),
                "discord".to_string(),
                "--source=flatpak".to_string(),
            ]
        );
        assert_eq!(
            install_engine_argv(&[
                "apex", "install", "--source", "capsule", "--env", "arch", "yay",
            ]),
            vec![
                "install".to_string(),
                "yay".to_string(),
                "--source=capsule".to_string(),
                "--env=arch".to_string(),
            ]
        );
    }

    #[test]
    fn a_capsule_install_does_not_demand_root() {
        // Root has no capsules. Demanding root here would make the CLI ask for
        // a password and the engine then refuse the privileged invocation —
        // the user gets two refusals and no package.
        assert_eq!(
            privilege(&["apex", "install", "--source", "capsule", "htop"]),
            None
        );
        // Every other source still writes something the system owns.
        assert_eq!(
            privilege(&["apex", "install", "--source", "rpm", "htop"]),
            Some("install")
        );
        assert_eq!(
            privilege(&["apex", "install", "--source", "flatpak", "discord"]),
            Some("install")
        );
    }

    #[test]
    fn asking_what_would_happen_never_needs_a_password() {
        assert_eq!(privilege(&["apex", "resolve", "discord"]), None);
        match Cli::try_parse_from(["apex", "resolve", "discord"])
            .expect("parses")
            .command
        {
            Cmd::Resolve { name } => assert_eq!(name, "discord"),
            _ => panic!("not a resolve"),
        }
    }

    fn privilege(argv: &[&str]) -> Option<&'static str> {
        privileged_verb(&Cli::try_parse_from(argv).expect("parses").command)
    }

    #[test]
    fn install_is_still_a_root_only_verb() {
        // Adding a flag must not accidentally move `install` out of the
        // privileged set: it writes an extension and re-merges /usr.
        assert_eq!(
            privilege(&["apex", "install", "--allow-unsigned", "./x.rpm"]),
            Some("install")
        );
        assert_eq!(privilege(&["apex", "remove", "htop"]), Some("remove"));
        assert_eq!(privilege(&["apex", "pkg", "upgrade"]), Some("pkg upgrade"));
    }

    #[test]
    fn reading_never_needs_a_password() {
        // Each of these is driven from the desktop as the session user. A
        // password prompt here is not a security improvement, it is a shell
        // that stops working.
        for argv in [
            vec!["apex", "search", "htop"],
            vec!["apex", "pkg", "list"],
            vec!["apex", "pkg", "verify"],
            vec!["apex", "status"],
            vec!["apex", "tier"],
            vec!["apex", "fan", "status"],
        ] {
            assert_eq!(privilege(&argv), None, "{argv:?} demanded root");
        }
    }

    // ── apex plugin (§16) ───────────────────────────────────────────────────

    fn plugin(argv: &[&str]) -> Vec<String> {
        match Cli::try_parse_from(argv).expect("parses").command {
            Cmd::Plugin { cmd } => plugin_argv(cmd),
            _ => panic!("not a plugin verb"),
        }
    }

    #[test]
    fn the_plugin_verbs_reach_the_helper_unchanged() {
        assert_eq!(plugin(&["apex", "plugin", "list"]), vec!["list"]);
        assert_eq!(
            plugin(&["apex", "plugin", "list", "--json"]),
            vec!["list", "--json"]
        );
        assert_eq!(
            plugin(&["apex", "plugin", "info", "apex-worldclock"]),
            vec!["info", "apex-worldclock"]
        );
        assert_eq!(
            plugin(&["apex", "plugin", "enable", "apex-worldclock"]),
            vec!["enable", "apex-worldclock"]
        );
        assert_eq!(
            plugin(&["apex", "plugin", "disable", "apex-worldclock"]),
            vec!["disable", "apex-worldclock"]
        );
    }

    #[test]
    fn a_plugin_id_is_passed_through_and_never_interpreted_here() {
        // The id is validated by the helper — for path safety in shell, and
        // against apex-shell's own `validId` through node. This side must not
        // pre-filter it: a CLI that silently dropped or rewrote an id would
        // make the helper's refusal unreachable, and the refusal is the thing
        // that keeps a traversal out of a filesystem path.
        assert_eq!(
            plugin(&["apex", "plugin", "info", "../../etc/passwd"]),
            vec!["info", "../../etc/passwd"]
        );
    }

    #[test]
    fn plugins_are_never_a_privileged_verb() {
        // Every path `apex plugin` touches is under the invoking user's
        // ~/.config/apex-shell, which is the directory APEX Shell itself
        // reads. A root `apex plugin disable` would move root's plugins and
        // leave the user's alone — a command that reports success and changes
        // nothing the user can see.
        for argv in [
            vec!["apex", "plugin", "list"],
            vec!["apex", "plugin", "info", "x"],
            vec!["apex", "plugin", "enable", "x"],
            vec!["apex", "plugin", "disable", "x"],
        ] {
            assert_eq!(privilege(&argv), None, "{argv:?} demanded root");
        }
    }

    #[test]
    fn the_plugin_helper_is_an_absolute_path_in_libexec() {
        // Not a PATH lookup. `apex plugin` drives a shipped program, and
        // resolving it through PATH would let anything on the user's PATH
        // answer for the shell's plugin rules.
        assert!(ops::PLUGIN_ENGINE.starts_with('/'));
        assert_ne!(ops::PLUGIN_ENGINE, ops::ENV_ENGINE);
        assert_ne!(ops::PLUGIN_ENGINE, ops::PKG_ENGINE);
    }

    // ── apex env (§8 capsules) ──────────────────────────────────────────────

    fn env(argv: &[&str]) -> Vec<String> {
        match Cli::try_parse_from(argv).expect("parses").command {
            Cmd::Env { cmd } => env_argv(cmd),
            _ => panic!("not an env verb"),
        }
    }

    #[test]
    fn env_create_passes_the_name_and_nothing_it_was_not_given() {
        assert_eq!(env(&["apex", "env", "create", "fedora"]), vec!["create", "fedora"]);
    }

    #[test]
    fn the_device_profile_reaches_the_engine() {
        // The one flag that decides whether a capsule can see the GPU. A
        // silently dropped `--gpu` produces a capsule that looks right and
        // cannot compute, which is a bug report about drivers.
        assert_eq!(
            env(&["apex", "env", "create", "ml", "--gpu", "amd"]),
            vec!["create", "ml", "--gpu=amd"]
        );
        assert_eq!(
            env(&["apex", "env", "create", "box", "--image", "docker.io/library/ubuntu:24.04"]),
            vec!["create", "box", "--image=docker.io/library/ubuntu:24.04"]
        );
    }

    #[test]
    fn a_trailing_command_is_separated_from_the_engines_own_flags() {
        // Without the `--` the engine cannot tell a command's flags from its
        // own, and clap has already consumed the separator the user typed.
        assert_eq!(
            env(&["apex", "env", "exec", "box", "ls", "-l"]),
            vec!["exec", "box", "--", "ls", "-l"]
        );
        assert_eq!(
            env(&["apex", "env", "enter", "box", "--", "bash", "-lc", "echo hi"]),
            vec!["enter", "box", "--", "bash", "-lc", "echo hi"]
        );
    }

    #[test]
    fn entering_without_a_command_asks_for_a_login_shell() {
        // `enter box --` with an empty command must not reach the engine, or it
        // would report a usage error for a request that is perfectly valid.
        assert_eq!(env(&["apex", "env", "enter", "box"]), vec!["enter", "box"]);
    }

    #[test]
    fn removing_a_capsule_does_not_inherit_force() {
        assert_eq!(env(&["apex", "env", "rm", "box"]), vec!["rm", "box"]);
        assert_eq!(
            env(&["apex", "env", "rm", "box", "--force", "--keep-home"]),
            vec!["rm", "box", "--keep-home", "--force"]
        );
    }

    #[test]
    fn the_gui_export_reaches_the_engine_with_both_halves() {
        // §8's launcher integration. The application name is a positional, not
        // a flag, and the engine refuses anything that is not a bare name — so
        // a dropped argument here would become a usage error rather than an
        // export of something else.
        assert_eq!(
            env(&["apex", "env", "export", "py", "gimp"]),
            vec!["export", "py", "gimp"]
        );
        assert_eq!(
            env(&["apex", "env", "unexport", "py", "gimp"]),
            vec!["unexport", "py", "gimp"]
        );
        assert_eq!(env(&["apex", "env", "exports", "py"]), vec!["exports", "py"]);
    }

    #[test]
    fn provisioning_a_language_names_the_language_and_not_a_capsule() {
        // The capsule a language lives in is the ENGINE's decision — c and cpp
        // share one, javascript and typescript share one — so the CLI must not
        // pass a capsule name here or it would be a second answer to the same
        // question.
        assert_eq!(
            env(&["apex", "env", "provision", "rust"]),
            vec!["provision", "rust"]
        );
        assert_eq!(env(&["apex", "env", "languages"]), vec!["languages"]);
    }

    #[test]
    fn capsules_are_never_a_privileged_verb() {
        // Capsules are rootless per-user containers. If `apex env` ever landed
        // in the privileged set it would create them under
        // /var/lib/containers, shared by every account, and need an
        // authentication prompt to enter a shell.
        //
        // `export` and `provision` are in this list for a sharper reason than
        // the others: both are reachable from `apex apply`, and the blueprint's
        // whole claim to never raising an authentication prompt is that it
        // converges the privilege domain it is already in. A privileged capsule
        // verb would break that claim from the outside.
        for argv in [
            vec!["apex", "env", "create", "fedora"],
            vec!["apex", "env", "rm", "fedora"],
            vec!["apex", "env", "install", "fedora", "htop"],
            vec!["apex", "env", "enter", "fedora"],
            vec!["apex", "env", "export", "fedora", "gimp"],
            vec!["apex", "env", "unexport", "fedora", "gimp"],
            vec!["apex", "env", "provision", "rust"],
        ] {
            assert_eq!(privilege(&argv), None, "{argv:?} demanded root");
        }
    }
}
