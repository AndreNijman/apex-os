//! `apex recover` and `apex doctor --json` — roadmap §19's recovery surface,
//! automatic repair, and the scoped reset.
//!
//! ## `status` spawns nothing, and that is a property rather than an accident
//!
//! Every fact on the recovery surface is a file read. No `bootc`, no `ostree`,
//! no `nvidia-smi`, no `systemctl`, no D-Bus, no packet. Three things follow,
//! and each of them is asserted in `tests/test-apex-recover.sh`:
//!
//! * it cannot raise an authentication prompt, because nothing it does needs
//!   authorising;
//! * it cannot be slow or hang, so APEX Settings can poll it;
//! * it is exercisable against a fixture tree, so the states the developer's
//!   own machine does not have — a missing rollback target, an extension built
//!   for the previous release, `/usr` mounted read-write — are covered by
//!   assertions instead of by reasoning.
//!
//! The deliberate cost is that the deployment row reports the ostree checksum
//! rather than the image reference: the reference lives in `bootc status`, and
//! parsing another tool's JSON schema to duplicate what `apex changelog`
//! already prints would buy a second thing to keep in sync. The row names
//! `apex changelog` instead.
//!
//! ## `repair` converges the domain it is in, and reports the other
//!
//! The same split `apex apply` uses. A repair verb that demanded root would
//! make the half that fixes a broken desktop reachable only by running it as
//! the user who has no desktop, and nothing here ever calls `sudo` — so
//! `apex recover repair` is structurally incapable of producing the
//! authentication prompt this project has asked twice never to see.
//!
//! The steps themselves are in [`apexd_core::recover::REPAIRS`], where a test
//! asserts the invariant that makes a single button defensible: every step is
//! idempotent and removes no data. §19 lists repair, "boot previous
//! deployment" and "factory reset" as three separate actions precisely because
//! they carry three different consequences, and collapsing them would be the
//! whole point missed.
//!
//! ## `reset` is the destructive verb, so it is built to be refused
//!
//! Dry run is the default and cannot be turned off by a flag alone. Performing
//! it needs `--commit` **and** `--confirm <token>`, where the token is derived
//! from the scope *and the exact set of paths the plan found*. A caller cannot
//! construct it from the scope; it has to run the plan, which is the step that
//! prints the loss list. A machine that changed between plan and commit
//! produces a different token and the commit is refused with nothing touched.
//!
//! On top of that: it refuses to run as root, it validates `$HOME` before
//! resolving anything under it, every target is re-resolved and asserted to be
//! inside the home before removal, everything it removes is copied to a
//! backup directory outside every target first, and after the commit it
//! re-checks every preserved landmark that existed beforehand. Grepping for
//! what you deleted cannot detect what you deleted as well; the landmark check
//! is what can.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use apexd_core::recover::{
    confirm_token, preserved, targets, Disposition, Domain, Health, Kind, RepairStep, ResetScope,
    Target, PRESERVED_LANDMARKS, REPAIRS,
};
use clap::{Args, Subcommand};
use serde_json::{json, Value};

use crate::ops::LocalView;

#[derive(Subcommand)]
pub enum RecoverCmd {
    /// The recovery surface: every component §19 lists, with its state and the
    /// action that addresses it.
    ///
    /// Read-only, root-free, and it spawns no subprocess at all — so it is
    /// safe to poll from APEX Settings and can never raise an authentication
    /// prompt. Exits non-zero when any component needs attention, so it is
    /// usable as a check.
    Status {
        /// Emit machine-readable JSON instead of a report.
        #[arg(long)]
        json: bool,
    },
    /// Run the repairs that are idempotent and remove nothing.
    ///
    /// A DRY RUN unless `--commit` is given. Converges only the privilege
    /// domain it is already running in and reports the other, exactly as
    /// `apex apply` does: `apex recover repair` re-seeds your desktop,
    /// `sudo apex recover repair` rebuilds the package extension. Nothing here
    /// calls sudo itself.
    ///
    /// Rollback and factory reset are deliberately NOT repairs. §19 lists them
    /// as separate actions because they carry consequences the user has to see
    /// first, and `apex recover repair` will never perform either.
    Repair(RepairArgs),
    /// Reset APEX-owned state for this account, back to what the image
    /// provisions.
    ///
    /// A DRY RUN unless BOTH `--commit` and a matching `--confirm` are given.
    /// The dry run prints, per path, exactly what is removed and exactly what
    /// is preserved, and then the one command line that performs it — carrying
    /// a token derived from that plan, so a confirmation cannot be constructed
    /// without having seen the list.
    ///
    /// `--scope desktop` is settings, keybinds and caches. `--scope user` adds
    /// your blueprint, per-game profiles, trusted devices, local-model
    /// settings and recorded agent sessions. Neither touches a document, a
    /// checkout, a credential, a capsule, an installed package or the booted
    /// deployment — `apex recover reset --scope user` prints the full list.
    ///
    /// A full factory reset — accounts removed, `/etc` restored, disks
    /// repartitioned — is the installer's job and not a verb on a running
    /// system. `docs/recovery.md` says why.
    Reset(ResetArgs),
}

#[derive(Args)]
pub struct RepairArgs {
    /// Actually run the applicable steps. Without it, nothing is changed.
    #[arg(long)]
    pub commit: bool,
    /// Emit the plan as JSON.
    #[arg(long)]
    pub json: bool,
}

#[derive(Args)]
pub struct ResetArgs {
    /// How far the reset reaches: desktop or user.
    ///
    /// Required, and there is no default. A destructive verb whose blast
    /// radius came from a default is one nobody can review.
    #[arg(long, value_name = "SCOPE")]
    pub scope: String,
    /// Actually perform it. Requires a matching --confirm.
    #[arg(long)]
    pub commit: bool,
    /// The token the dry run printed. Bound to the plan, not to the scope.
    #[arg(long, value_name = "TOKEN")]
    pub confirm: Option<String>,
    /// Skip re-seeding the desktop afterwards.
    ///
    /// Without this, a reset refuses to start when the provisioner is missing:
    /// removing the files APEX Shell needs and having no way to put them back
    /// is a worse state than the one being left. Use it only if you want the
    /// deletion alone.
    #[arg(long)]
    pub no_reprovision: bool,
    /// Emit the plan as JSON.
    #[arg(long)]
    pub json: bool,
}

// ── the fixture root ─────────────────────────────────────────────────────────

/// Where the system half of the surface reads from.
///
/// A prefix, and only a prefix — the same shape `apex boot status` uses, for
/// the same reason: the interesting states are ones a healthy machine does not
/// have, so they have to be presentable as a tree. It also maps the
/// provisioner's path, so no environment variable ever names a *program*: a
/// caller-controlled program name is a hole even in an unprivileged command,
/// because nothing stops root from running it.
struct Sys {
    fixture: Option<PathBuf>,
}

impl Sys {
    fn from_env() -> Sys {
        Sys {
            fixture: std::env::var_os("APEX_RECOVER_ROOT")
                .filter(|v| !v.is_empty())
                .map(PathBuf::from),
        }
    }

    fn path(&self, absolute: &str) -> PathBuf {
        match &self.fixture {
            // `absolute` always starts with '/', so strip it before joining, or
            // Path::join discards the prefix and silently reads the real
            // system — a fixture that reads /sys is worse than no fixture at
            // all, because the test then passes on the author's machine only.
            Some(root) => root.join(absolute.trim_start_matches('/')),
            None => PathBuf::from(absolute),
        }
    }

    fn read(&self, absolute: &str) -> Option<String> {
        std::fs::read_to_string(self.path(absolute)).ok()
    }

    fn exists(&self, absolute: &str) -> bool {
        self.path(absolute).exists()
    }
}

/// The per-user provisioner. Re-seeds everything a reset removes, is idempotent
/// by design, runs at every login already, and needs no network.
const PROVISIONER: &str = "/usr/libexec/apex-shell-firstrun";

// ── doctor, made machine-readable ────────────────────────────────────────────

/// One line of `apex doctor`.
///
/// §19 asks for "`apex doctor` results graphically", which from the OS side
/// means the same checks in a shape a UI can render — not a second set of
/// checks that can disagree with the text one. So `doctor` builds this list
/// once and renders it either way.
pub struct Check {
    pub ok: bool,
    pub what: String,
}

/// Render the doctor's checks as text or JSON.
///
/// ## Why there is no severity field
///
/// `apex doctor` reports what this machine has, and its own comment says a
/// WARN is information rather than a fault: a laptop with no ACPI
/// `platform_profile` is not broken. Adding a severity would mean inventing a
/// judgement the checks do not make, and a UI painting an invented judgement
/// red is worse than one showing two states. So the JSON carries exactly what
/// the text carries — a boolean and a sentence — plus the counts, so a summary
/// badge needs no client-side arithmetic.
pub fn render_doctor(checks: &[Check], json: bool) -> String {
    if !json {
        let mut out = String::new();
        for c in checks {
            let _ = writeln!(out, "[{}] {}", if c.ok { "PASS" } else { "WARN" }, c.what);
        }
        return out;
    }
    let passed = checks.iter().filter(|c| c.ok).count();
    let doc = json!({
        "checks": checks
            .iter()
            .map(|c| json!({"ok": c.ok, "check": c.what}))
            .collect::<Vec<_>>(),
        "passed": passed,
        "warned": checks.len() - passed,
        "total": checks.len(),
    });
    format!("{}\n", serde_json::to_string_pretty(&doc).unwrap_or_default())
}

/// Every check `apex doctor` performs.
///
/// `daemon_running` is a parameter rather than something read here, so this is
/// a synchronous function that needs no bus — which is also what makes it
/// callable from a test.
pub fn doctor_checks(v: &LocalView, daemon_running: bool) -> Vec<Check> {
    let mut out: Vec<Check> = Vec::new();
    let mut line = |ok: bool, what: String| out.push(Check { ok, what });

    line(
        daemon_running,
        "apexd running (owns org.apexos.Apexd1)".to_string(),
    );
    line(
        true,
        format!(
            "profile resolved: active={} class={} device={}",
            v.selection.active,
            v.selection.class_or_empty(),
            v.selection.device_or_empty()
        ),
    );

    // Every check below reports what this machine has; a WARN is information,
    // not a fault. Nothing here is required for apexd to work.
    let driver = v.fingerprint.cpu.scaling_driver.as_deref().unwrap_or("");
    let driver_name = if driver.is_empty() { "none" } else { driver };
    line(
        !driver.is_empty(),
        format!("cpufreq scaling driver present ({driver_name})"),
    );
    line(
        v.fingerprint.cpu.amd_pstate() || v.fingerprint.cpu.intel_pstate(),
        format!(
            "EPP-capable scaling driver ({driver_name}) — without it, tiers use the governor alone"
        ),
    );
    line(
        Path::new("/sys/firmware/acpi/platform_profile").exists(),
        format!(
            "ACPI platform_profile present (choices: {})",
            crate::read_sys("firmware/acpi/platform_profile_choices")
                .unwrap_or_else(|| "none".into())
        ),
    );

    let inv = apexd_core::BatteryInventory::detect();
    line(!inv.is_empty(), format!("battery discovery: {}", inv.summary()));
    if !inv.is_empty() {
        line(
            inv.supports_thresholds(),
            format!(
                "charge threshold control present ({})",
                inv.threshold_support().as_str()
            ),
        );
    }

    for (ok, what) in crate::touchpad::doctor_lines() {
        line(ok, what);
    }

    let s2idle = crate::read_sys("power/mem_sleep")
        .map(|s| s.contains("[s2idle]"))
        .unwrap_or(false);
    line(s2idle, "s2idle is the active suspend mode".to_string());

    // ── M6: fan control and game orchestration ──────────────────────────────
    let fan_cfg = v.active_profile().fan_config();
    let fans = apexd_core::fan::FanInventory::discover(Path::new("/sys"), &fan_cfg);
    let fan_names = if fans.controls.is_empty() && fans.msi_ec.is_none() {
        "none".to_string()
    } else {
        let mut s: Vec<String> = fans.controls.iter().map(|c| c.id.clone()).collect();
        if fans.msi_ec.is_some() {
            s.push("msi-ec".into());
        }
        s.join(", ")
    };
    line(
        fans.controllable(),
        format!("fan control channel present, write access unverified ({fan_names})"),
    );
    let topo = apexd_core::CoreTopology::detect_from(Path::new("/sys"));
    if v.fingerprint.cpu.hybrid {
        line(
            topo.is_hybrid(),
            format!(
                "P/E split detected via {} (P={} E={})",
                topo.source.as_str(),
                topo.pcore_list(),
                topo.ecore_list()
            ),
        );
    }
    if v.fingerprint
        .gpus
        .iter()
        .any(|g| g.vendor == apexd_core::GpuVendor::Nvidia)
    {
        line(
            apexd_core::gpu::nvidia_smi_available(),
            "nvidia-smi on PATH (needed for game-mode clock locks)".to_string(),
        );
    }
    line(
        Path::new("/sys/fs/cgroup/cgroup.controllers").exists(),
        "cgroup v2 present (needed for game-mode cpuset pinning)".to_string(),
    );
    out
}

// ── the recovery surface ─────────────────────────────────────────────────────

/// One rendered row.
struct Row {
    id: &'static str,
    label: &'static str,
    state: Health,
    detail: String,
    /// The command that addresses this row, when one exists. Not every row has
    /// an action — "no default route" is not something a CLI verb fixes.
    action: Option<String>,
}

/// A recovery route: a way back into a working system.
struct Route {
    id: &'static str,
    /// `None` means "cannot be determined from a running system" — which is
    /// the honest answer for installer media and is not the same as "no".
    available: Option<bool>,
    how: String,
}

/// The whole surface, probed.
struct Surface {
    bootloader: &'static str,
    rows: Vec<Row>,
    routes: Vec<Route>,
}

/// Deployments present under `/ostree/deploy/*/deploy`.
///
/// Counted from the filesystem rather than asked of `bootc status`, because a
/// directory count is a fact with no schema: it cannot break when another
/// tool changes its JSON, it needs no subprocess, and it is presentable as a
/// fixture. A deployment is `<checksum>.<serial>`; the sibling
/// `<checksum>.<serial>.origin` file is not one.
fn deployment_count(sys: &Sys) -> Option<usize> {
    let root = sys.path("/ostree/deploy");
    let stateroots = std::fs::read_dir(&root).ok()?;
    let mut n = 0usize;
    for sr in stateroots.flatten() {
        let deploy = sr.path().join("deploy");
        if let Ok(entries) = std::fs::read_dir(&deploy) {
            for e in entries.flatten() {
                let name = e.file_name();
                let name = name.to_string_lossy();
                if name.ends_with(".origin") {
                    continue;
                }
                if e.path().is_dir() {
                    n += 1;
                }
            }
        }
    }
    Some(n)
}

/// The booted deployment's ostree checksum, from the kernel command line.
///
/// `ostree=/ostree/boot.1/apex/<hash>/0` on a GRUB/BLS machine. The checksum
/// in that path is the deployment identity, and it is the value `ostree admin
/// pin` and `bootc status` both key on.
fn booted_deployment(cmdline: &str) -> Option<String> {
    let arg = cmdline
        .split_whitespace()
        .find_map(|w| w.strip_prefix("ostree="))?;
    // The last two components are `<hash>/<serial>` for boot.N paths and
    // `<csum>.<serial>` for deploy paths. Take the longest hex-looking
    // component, which is the identity in both layouts.
    arg.split('/')
        .map(|c| c.split('.').next().unwrap_or(c))
        .filter(|c| c.len() >= 32 && c.chars().all(|ch| ch.is_ascii_hexdigit()))
        .max_by_key(|c| c.len())
        .map(str::to_string)
}

/// Read `KEY=value` pairs out of an os-release file, unquoted.
fn os_release(sys: &Sys) -> BTreeMap<String, String> {
    let text = sys
        .read("/etc/os-release")
        .or_else(|| sys.read("/usr/lib/os-release"))
        .unwrap_or_default();
    let mut map = BTreeMap::new();
    for l in text.lines() {
        let l = l.trim();
        if l.is_empty() || l.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = l.split_once('=') {
            let v = v.trim().trim_matches('"').trim_matches('\'');
            map.insert(k.trim().to_string(), v.to_string());
        }
    }
    map
}

/// Whether `/usr` is mounted read-only, from `/proc/mounts`.
///
/// `None` when no mount covers `/usr`, which on an ostree machine means it is
/// covered by the root mount instead — so the root mount's flags are checked
/// as a fallback. Returning "read-write" for a machine whose `/proc/mounts`
/// simply looks different would report the drift AGENTS.md prohibits on a
/// machine that has none.
fn usr_readonly(mounts: &str) -> (Option<bool>, Option<String>) {
    let mut root: Option<(bool, String)> = None;
    for l in mounts.lines() {
        let f: Vec<&str> = l.split_whitespace().collect();
        if f.len() < 4 {
            continue;
        }
        let (target, fstype, opts) = (f[1], f[2], f[3]);
        let ro = opts.split(',').any(|o| o == "ro");
        if target == "/usr" || target == "/sysroot/usr" {
            return (Some(ro), Some(fstype.to_string()));
        }
        if target == "/" {
            root = Some((ro, fstype.to_string()));
        }
    }
    match root {
        Some((ro, fs)) => (Some(ro), Some(fs)),
        None => (None, None),
    }
}

/// Does the machine have a default route? Read, never probed.
///
/// `/proc/net/route` lists a destination of `00000000` for the default. This
/// deliberately contacts nothing: a recovery surface that resolved a name or
/// opened a socket would be slow on exactly the machine whose network is the
/// problem, and "the internet is reachable" is not a fact APEX needs to assert
/// to tell the user whether their machine has a route.
fn has_default_route(route: &str) -> bool {
    route
        .lines()
        .skip(1)
        .filter_map(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            (f.len() >= 2).then_some(f[1])
        })
        .any(|dest| dest == "00000000")
}

/// The kernel module a GPU vendor needs, and whether one of them is loaded.
fn gpu_modules(vendor: &apexd_core::GpuVendor) -> &'static [&'static str] {
    use apexd_core::GpuVendor as V;
    match vendor {
        V::Nvidia => &["nvidia", "nouveau"],
        V::Amd => &["amdgpu", "radeon"],
        V::Intel => &["i915", "xe"],
        _ => &[],
    }
}

fn probe(sys: &Sys) -> Surface {
    let cmdline = sys.read("/proc/cmdline").unwrap_or_default();
    let osr = os_release(sys);
    let chain = crate::boot::chain_facts(sys.fixture.clone());
    let ostree_booted = sys.exists("/run/ostree-booted");
    let mut rows: Vec<Row> = Vec::new();

    // ── current deployment ──────────────────────────────────────────────────
    let deployment = booted_deployment(&cmdline);
    let variant = osr.get("VARIANT_ID").cloned().unwrap_or_default();
    let version = osr.get("VERSION_ID").cloned().unwrap_or_default();
    rows.push(match (ostree_booted, deployment.as_deref()) {
        (true, Some(csum)) => Row {
            id: "current-deployment",
            label: "Current deployment",
            state: Health::Verified,
            detail: format!(
                "ostree {} — APEX-OS {} {} (the image reference and its source \
                 revision are `apex changelog`)",
                &csum[..csum.len().min(12)],
                if version.is_empty() { "?" } else { &version },
                if variant.is_empty() { "?" } else { &variant },
            ),
            action: None,
        },
        (true, None) => Row {
            id: "current-deployment",
            label: "Current deployment",
            state: Health::Attention,
            detail: "the machine booted an ostree deployment but the kernel \
                     command line carries no ostree= argument, so the booted \
                     deployment cannot be identified"
                .to_string(),
            action: Some("apex changelog".to_string()),
        },
        (false, _) => Row {
            id: "current-deployment",
            label: "Current deployment",
            state: Health::Unavailable,
            detail: "not an ostree/bootc boot (/run/ostree-booted is absent) — \
                     this is what a container or a CI runner looks like, not a \
                     fault on an installed APEX machine"
                .to_string(),
            action: None,
        },
    });

    // ── previous deployment: §19's [Boot previous deployment] ───────────────
    //
    // There is deliberately no `apex recover previous` verb. `apex rollback`
    // already swaps the default and the previous deployment, and a second name
    // for it would be a second thing to keep correct. What §19 actually asks
    // for is that the action be *visible from Settings rather than only the
    // CLI*, so the row reports whether there is anything to roll back to and
    // names the command a button runs.
    let deployments = deployment_count(sys);
    rows.push(match deployments {
        Some(n) if n >= 2 => Row {
            id: "previous-deployment",
            label: "Previous deployment",
            state: Health::Available,
            detail: format!(
                "{n} deployments present, so there is one to go back to. Nothing \
                 has verified that it boots — that is what the boot counter does \
                 on the opt-in systemd-boot path. `sudo apex pin` before a risky \
                 change, or two bad updates in a row can evict it."
            ),
            action: Some("sudo apex rollback".to_string()),
        },
        Some(1) => Row {
            id: "previous-deployment",
            label: "Previous deployment",
            state: Health::Attention,
            detail: "only the booted deployment exists, so there is nothing to \
                     roll back to yet. The next `apex update` creates one."
                .to_string(),
            action: None,
        },
        Some(n) => Row {
            id: "previous-deployment",
            label: "Previous deployment",
            state: Health::Unavailable,
            detail: format!("/ostree/deploy holds {n} deployments, which should be impossible"),
            action: None,
        },
        None => Row {
            id: "previous-deployment",
            label: "Previous deployment",
            state: Health::Unavailable,
            detail: "/ostree/deploy could not be read, so the deployment count \
                     is unknown. An empty answer here would be indistinguishable \
                     from 'nothing to roll back to', which is the answer that \
                     would hide a rollback."
                .to_string(),
            action: None,
        },
    });

    // ── Secure Boot ─────────────────────────────────────────────────────────
    rows.push(match chain.secure_boot {
        Some(true) => Row {
            id: "secure-boot",
            label: "Secure Boot",
            state: Health::Verified,
            detail: format!(
                "firmware reports Secure Boot enabled{}",
                match chain.setup_mode {
                    Some(true) => ", and the firmware is in Setup Mode",
                    _ => "",
                }
            ),
            action: None,
        },
        Some(false) => Row {
            id: "secure-boot",
            label: "Secure Boot",
            state: Health::Attention,
            detail: "firmware reports Secure Boot disabled. It is a product \
                     invariant for published images, and enabling it writes \
                     your firmware — an explicitly user-initiated procedure, \
                     never a script in this repository. See docs/boot-v2.md."
                .to_string(),
            action: None,
        },
        None => Row {
            id: "secure-boot",
            label: "Secure Boot",
            state: Health::Unavailable,
            detail: "no SecureBoot EFI variable: this is not a UEFI boot. \
                     Reporting 'disabled' would claim a measurement nobody took."
                .to_string(),
            action: None,
        },
    });

    // ── filesystem ──────────────────────────────────────────────────────────
    let mounts = sys.read("/proc/mounts").unwrap_or_default();
    let (usr_ro, usr_fs) = usr_readonly(&mounts);
    let fs = usr_fs.unwrap_or_else(|| "unknown".into());
    rows.push(match usr_ro {
        Some(true) => Row {
            id: "filesystem",
            label: "Filesystem",
            state: Health::Verified,
            detail: format!(
                "/usr is read-only on a {fs} root{}",
                if ostree_booted { ", ostree-booted" } else { "" }
            ),
            action: None,
        },
        Some(false) => Row {
            id: "filesystem",
            label: "Filesystem",
            state: Health::Attention,
            detail: format!(
                "/usr is mounted READ-WRITE on a {fs} root. /usr is image-owned \
                 and read-only at runtime; a writable one is machine drift, and \
                 anything written there is lost at the next update. Reboot to \
                 restore it."
            ),
            action: None,
        },
        None => Row {
            id: "filesystem",
            label: "Filesystem",
            state: Health::Unavailable,
            detail: "no mount covering /usr or / was found in /proc/mounts".to_string(),
            action: None,
        },
    });

    // ── GPU driver ──────────────────────────────────────────────────────────
    let modules = sys.read("/proc/modules").unwrap_or_default();
    let loaded: Vec<&str> = modules
        .lines()
        .filter_map(|l| l.split_whitespace().next())
        .collect();
    let fp = apexd_core::Fingerprint::detect_from(&sys.path("/proc"), &sys.path("/sys"));
    let mut missing: Vec<String> = Vec::new();
    let mut present: Vec<String> = Vec::new();
    for g in &fp.gpus {
        let want = gpu_modules(&g.vendor);
        if want.is_empty() {
            continue;
        }
        match want.iter().find(|m| loaded.contains(m)) {
            Some(m) => present.push(format!("{} via {}", g.vendor.as_str(), m)),
            None => missing.push(format!("{} (wanted one of {})", g.vendor.as_str(), want.join("/"))),
        }
    }
    rows.push(if fp.gpus.is_empty() {
        Row {
            id: "gpu-driver",
            label: "GPU driver",
            state: Health::Unavailable,
            detail: "no PCI display device was found, so there is no driver to \
                     check. This is what a headless machine or a VM without a \
                     virtual GPU looks like."
                .to_string(),
            action: None,
        }
    } else if missing.is_empty() {
        Row {
            id: "gpu-driver",
            label: "GPU driver",
            state: Health::Verified,
            detail: format!("{} — {}", present.len(), present.join(", ")),
            action: None,
        }
    } else {
        Row {
            id: "gpu-driver",
            label: "GPU driver",
            state: Health::Attention,
            detail: format!(
                "no kernel module loaded for {}{}",
                missing.join(", "),
                if present.is_empty() {
                    String::new()
                } else {
                    format!("; working: {}", present.join(", "))
                }
            ),
            action: Some("sudo apex rollback".to_string()),
        }
    });

    // ── APEX Shell ──────────────────────────────────────────────────────────
    //
    // The same two facts `apex-shell-firstrun` checks, in the same order: the
    // image has to carry the shell (otherwise it is an image-build defect and
    // no user action helps), and this account has to be provisioned.
    let shell_qml = sys.path("/usr/share/apex-shell/shell.qml");
    let shipped = std::fs::metadata(&shell_qml).map(|m| m.len() > 0).unwrap_or(false);
    let provisioned = user_home()
        .map(|h| h.join(".config/apex-shell").is_dir())
        .unwrap_or(false);
    rows.push(match (shipped, provisioned) {
        (true, true) => Row {
            id: "apex-shell",
            label: "APEX Shell",
            state: Health::Verified,
            detail: "vendored in the image at /usr/share/apex-shell, and this \
                     account is provisioned"
                .to_string(),
            action: None,
        },
        (true, false) => Row {
            id: "apex-shell",
            label: "APEX Shell",
            state: Health::Attention,
            detail: "the image carries the shell but this account has no \
                     ~/.config/apex-shell, so the desktop has never been \
                     provisioned"
                .to_string(),
            action: Some("apex recover repair --commit".to_string()),
        },
        (false, _) => Row {
            id: "apex-shell",
            label: "APEX Shell",
            state: Health::Unavailable,
            detail: "/usr/share/apex-shell/shell.qml is missing or empty — the \
                     image did not ship APEX Shell. That is an image-build \
                     defect, not something a reset or a repair can fix; roll \
                     back to the previous deployment."
                .to_string(),
            action: Some("sudo apex rollback".to_string()),
        },
    });

    // ── network ─────────────────────────────────────────────────────────────
    let route = sys.read("/proc/net/route").unwrap_or_default();
    // `Available` is the ceiling here on purpose. Nothing was contacted, so
    // nothing was verified — claiming `verified` would be claiming a
    // reachability test this row deliberately does not perform.
    rows.push(if has_default_route(&route) {
        Row {
            id: "network",
            label: "Network",
            state: Health::Available,
            detail: "a default route exists. Nothing was contacted, so nothing \
                     about reachability is claimed."
                .to_string(),
            action: None,
        }
    } else {
        Row {
            id: "network",
            label: "Network",
            state: Health::Attention,
            detail: "no default route. `apex update` and `apex install` need \
                     one; every verb on this surface does not."
                .to_string(),
            action: None,
        }
    });

    // ── package extensions ──────────────────────────────────────────────────
    rows.push(package_row(sys, &version));

    // ── recovery routes ─────────────────────────────────────────────────────
    let mut routes: Vec<Route> = Vec::new();
    routes.push(Route {
        id: "previous-deployment",
        available: deployments.map(|n| n >= 2),
        how: "`sudo apex rollback` then reboot, or select the previous entry in \
              the boot menu. /etc and /var — including /var/home — are preserved."
            .to_string(),
    });
    // The rescue route is NOT uniform, and saying so is the point.
    //
    // Reaching `rescue.target` means editing the kernel command line at the
    // boot menu. GRUB lets you. A Unified Kernel Image does not: its command
    // line is inside the signed image, which is what makes the signature worth
    // having, so on the opt-in systemd-boot+UKI path this route does not exist
    // — on exactly the machines that are hardest to get into. Reporting it
    // uniformly would be a false claim there, so the condition is the UKI, not
    // the loader's name: systemd-boot booting a type #1 entry still has an
    // editable command line.
    let rescue_present = sys.exists("/usr/lib/systemd/system/rescue.target");
    let cmdline_editable = chain.bootloader == "grub"
        || (chain.bootloader == "systemd-boot" && !chain.booted_from_uki);
    routes.push(Route {
        id: "rescue-target",
        available: Some(rescue_present && cmdline_editable),
        how: if cmdline_editable {
            format!(
                "at the {} menu, edit the entry ({}) and append \
                 `systemd.unit=rescue.target` to the kernel command line. It \
                 asks for the root password.",
                chain.bootloader,
                if chain.bootloader == "grub" { "`e`, then Ctrl-X" } else { "`e`" }
            )
        } else if chain.booted_from_uki {
            "not available: this machine booted a Unified Kernel Image, whose \
             command line is inside the signed image and cannot be edited at \
             the menu. Use the boot counter or the previous deployment."
                .to_string()
        } else {
            format!(
                "not available: the bootloader is {} and rescue.target is {}.",
                chain.bootloader,
                if rescue_present { "present" } else { "absent from this image" }
            )
        },
    });
    routes.push(Route {
        id: "boot-counting",
        available: Some(chain.boot_counting),
        how: if chain.boot_counting {
            "three boots that do not reach boot-complete.target and \
             systemd-boot selects the previous blessed entry by itself. \
             `apex boot status` shows the tally."
                .to_string()
        } else {
            "not in effect: this machine boots through GRUB, which is the \
             default for every published APEX image. Automatic boot counting is \
             the opt-in systemd-boot path — see docs/boot-v2.md."
                .to_string()
        },
    });
    routes.push(Route {
        id: "disposable-environment",
        available: Some(sys.exists("/usr/libexec/apex-disposable")),
        how: "`apex disposable run` gives you a throwaway userspace on a machine \
              that still boots — a whole environment that is deleted when you \
              close it. It is not a repair environment for a machine that will \
              not boot."
            .to_string(),
    });
    routes.push(Route {
        id: "recovery-boot-entry",
        // Deliberately `false`, not `null`. APEX ships no recovery boot entry,
        // and that is a decision rather than a gap: creating one means writing
        // an ESP or an EFI variable, and there is no rollback for either —
        // the thing that would perform the rollback is what you broke.
        // docs/recovery.md carries the operator procedure.
        available: Some(false),
        how: "APEX ships no recovery boot entry, and nothing in this repository \
              writes an ESP or an EFI variable. Adding one is an operator \
              procedure — see docs/recovery.md — because there is no rollback \
              for a boot path you overwrote."
            .to_string(),
    });
    routes.push(Route {
        id: "installer-media",
        // `null`: a running system cannot tell whether the user has a USB
        // stick in a drawer. `false` would be a claim; `true` would be a lie.
        available: None,
        how: "the route for a machine that will not boot at all. Cannot be \
              determined from a running system."
            .to_string(),
    });

    Surface {
        bootloader: chain.bootloader,
        rows,
        routes,
    }
}

/// The package-extension row.
///
/// Split out because it is the one row that reads another program's state file,
/// and every branch of it is a state the developer's machine does not have at
/// the same time.
fn package_row(sys: &Sys, running_version: &str) -> Row {
    const STATE: &str = "/var/lib/apex/pkg/state.json";
    let id = "package-extensions";
    let label = "Package extensions";
    let Some(text) = sys.read(STATE) else {
        return Row {
            id,
            label,
            state: Health::Verified,
            // Absence is a checked fact, and it is the common case: most
            // machines install nothing with `apex install`.
            detail: "no user packages on this machine".to_string(),
            action: None,
        };
    };
    let doc: Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(e) => {
            return Row {
                id,
                label,
                state: Health::Attention,
                detail: format!("{STATE} could not be parsed: {e}"),
                action: Some("sudo apex pkg rebuild".to_string()),
            }
        }
    };
    let built_for = doc
        .get("os_version_id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let resolved = doc
        .get("resolved")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);
    let unsigned = doc
        .get("unsigned_accepted")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);

    if !built_for.is_empty() && !running_version.is_empty() && built_for != running_version {
        return Row {
            id,
            label,
            state: Health::Attention,
            detail: format!(
                "{resolved} packages, built for OS {built_for} but this machine \
                 runs OS {running_version} — the extension needs rebuilding \
                 against the booted image"
            ),
            action: Some("sudo apex pkg rebuild --if-needed".to_string()),
        };
    }
    if unsigned > 0 {
        return Row {
            id,
            label,
            state: Health::Attention,
            detail: format!(
                "{resolved} packages built for OS {built_for}, of which \
                 {unsigned} were installed with --allow-unsigned and are \
                 covered by no trusted key"
            ),
            action: Some("apex pkg verify".to_string()),
        };
    }
    Row {
        id,
        label,
        state: Health::Verified,
        detail: format!("{resolved} packages, built for OS {built_for} (running {running_version})"),
        action: None,
    }
}

/// §19's four action buttons, and the command each one runs.
///
/// The command strings are bounded at 66 characters, because the text renderer
/// prints them after a 28-column prefix and the report as a whole is held to
/// 96. A test asserts the rendered width rather than this constant: the first
/// version of this table produced a 102-column line while every `wrap` unit
/// test passed, because the fixed-width action rows do not go through `wrap`
/// at all.
fn actions() -> Vec<(&'static str, &'static str, &'static str)> {
    vec![
        (
            "repair",
            "Repair automatically",
            "apex recover repair                  (dry run; --commit runs it)",
        ),
        (
            "bootPrevious",
            "Boot previous deployment",
            "sudo apex rollback                   then reboot",
        ),
        (
            "factoryReset",
            "Factory reset",
            "apex recover reset --scope desktop|user   (a dry run)",
        ),
        (
            "diagnostics",
            "Hardware diagnostics",
            "apex doctor                          (--json for a UI)",
        ),
    ]
}

fn cmd_status(json: bool) -> i32 {
    let sys = Sys::from_env();
    let s = probe(&sys);
    let attention = s.rows.iter().filter(|r| r.state == Health::Attention).count();

    if json {
        let doc = json!({
            "bootloader": s.bootloader,
            "rows": s.rows.iter().map(|r| json!({
                "id": r.id,
                "label": r.label,
                "state": r.state.as_str(),
                "detail": r.detail,
                "action": r.action,
            })).collect::<Vec<_>>(),
            "actions": actions().iter().map(|(id, label, cmd)| json!({
                "id": id, "label": label, "command": cmd.trim_end(),
            })).collect::<Vec<_>>(),
            "routes": s.routes.iter().map(|r| json!({
                "id": r.id, "available": r.available, "how": r.how,
            })).collect::<Vec<_>>(),
            "needsAttention": attention,
            "resetScopes": ResetScope::ALL.iter().map(|sc| json!({
                "id": sc.as_str(), "summary": sc.summary(),
            })).collect::<Vec<_>>(),
        });
        println!("{}", serde_json::to_string_pretty(&doc).unwrap_or_default());
        return if attention > 0 { 1 } else { 0 };
    }

    println!("APEX recovery");
    println!("  bootloader : {}", s.bootloader);
    println!();
    println!("{:<22}  {:<12}  DETAIL", "COMPONENT", "STATE");
    // 38, not 24: the prefix printed before the first line of the detail is
    // 22 + 2 + 12 + 2 columns, and `wrap` bounds a line INCLUDING the indent it
    // is given. Told 24 it left the first line free to reach 110 columns on a
    // 96-column budget — invisible to a test that checks `wrap`'s contract
    // rather than the rendered row.
    const DETAIL_COL: usize = 38;
    for r in &s.rows {
        println!("{:<22}  {:<12}  {}", r.label, r.state.as_str(), wrap(&r.detail, DETAIL_COL));
        if let Some(a) = &r.action {
            println!("{:<22}  {:<12}  -> {a}", "", "");
        }
    }
    println!("\nActions");
    for (_, label, cmd) in actions() {
        println!("  {label:<26} {cmd}");
    }
    println!("\nRecovery routes on this machine");
    for r in &s.routes {
        let mark = match r.available {
            Some(true) => "yes",
            Some(false) => "no",
            None => "unknown",
        };
        println!("  {:<24} {:<8} {}", r.id, mark, wrap(&r.how, 36));
    }
    if attention > 0 {
        println!("\n{attention} component(s) need attention.");
    }
    if attention > 0 {
        1
    } else {
        0
    }
}

/// Re-flow a detail string so a long sentence does not run off the terminal.
///
/// Whitespace-collapsing, because the details are written as multi-line Rust
/// string literals and would otherwise carry their source indentation into the
/// report.
fn wrap(text: &str, indent: usize) -> String {
    let words: Vec<&str> = text.split_whitespace().collect();
    let mut out = String::new();
    let mut col = indent;
    for w in words {
        if col + w.len() + 1 > 96 && col > indent {
            out.push('\n');
            for _ in 0..indent {
                out.push(' ');
            }
            col = indent;
        } else if !out.is_empty() {
            out.push(' ');
            col += 1;
        }
        out.push_str(w);
        col += w.len();
    }
    out
}

// ── repair ───────────────────────────────────────────────────────────────────

/// Which repair steps this machine currently needs.
///
/// Diagnosed from the same surface `status` renders, so the button and the
/// report cannot disagree. A step with no diagnosis is not offered: a
/// `[Repair automatically]` that proposes something on every healthy machine
/// trains people to ignore it.
fn applicable_repairs(s: &Surface) -> Vec<&'static RepairStep> {
    let state = |id: &str| s.rows.iter().find(|r| r.id == id).map(|r| r.state);
    REPAIRS
        .iter()
        .filter(|step| match step.id {
            "reprovision-desktop" => state("apex-shell") == Some(Health::Attention),
            "rebuild-package-extension" => {
                state("package-extensions") == Some(Health::Attention)
            }
            // An unrecognised step is never offered. A new entry in the table
            // with no diagnosis here would otherwise be silently proposed
            // always, which is the failure mode this filter exists to avoid.
            _ => false,
        })
        .collect()
}

fn cmd_repair(args: RepairArgs) -> i32 {
    let sys = Sys::from_env();
    let s = probe(&sys);
    let steps = applicable_repairs(&s);
    let root = crate::ops::effective_uid() == Some(0);
    let here = if root { Domain::System } else { Domain::User };

    let mine: Vec<&&RepairStep> = steps.iter().filter(|s| s.domain == here).collect();
    let theirs: Vec<&&RepairStep> = steps.iter().filter(|s| s.domain != here).collect();

    if args.json {
        let doc = json!({
            "domain": here.as_str(),
            "committed": args.commit,
            "steps": steps.iter().map(|s| json!({
                "id": s.id,
                "domain": s.domain.as_str(),
                "what": s.what,
                "whySafe": s.why_safe,
                "command": s.argv,
                "runnableHere": s.domain == here,
            })).collect::<Vec<_>>(),
        });
        println!("{}", serde_json::to_string_pretty(&doc).unwrap_or_default());
        if !args.commit {
            return 0;
        }
    } else {
        println!(
            "Automatic repair — {}",
            if args.commit {
                "COMMITTING"
            } else {
                "DRY RUN, nothing has been changed"
            }
        );
        println!("  privilege domain: {} (this run converges only this one)", here.as_str());
        if steps.is_empty() {
            println!("\nNothing to repair: every component this verb can fix reports fine.");
            println!("`apex recover status` shows the full surface.");
            return 0;
        }
        println!();
        for st in &steps {
            println!(
                "  [{}] {:<28} {}",
                st.domain.as_str(),
                st.id,
                wrap(st.what, 40)
            );
            println!("       command : {}", st.argv.join(" "));
            println!("       safe    : {}", wrap(st.why_safe, 17));
        }
        if !theirs.is_empty() {
            println!(
                "\n{} step(s) belong to the other privilege domain and were NOT run.",
                theirs.len()
            );
            println!(
                "  run: {}apex recover repair{}",
                if root { "" } else { "sudo " },
                if args.commit { " --commit" } else { "" }
            );
        }
        if !args.commit {
            println!("\nTo perform the {} step(s) above: apex recover repair --commit", mine.len());
            return 0;
        }
    }

    if steps.is_empty() {
        return 0;
    }

    let mut worst = 0;
    for st in &mine {
        // The program is spelled by the table, absolute, and under a fixture
        // root it is remapped like every other path — so no environment
        // variable ever names the program that runs here.
        let program = sys.path(st.argv[0]);
        eprintln!("apex: running: {} {}", program.display(), st.argv[1..].join(" "));
        match Command::new(&program).args(&st.argv[1..]).status() {
            Ok(status) => {
                let code = status.code().unwrap_or(-1);
                if code != 0 {
                    eprintln!("apex: {} exited {code}", st.id);
                }
                worst = worst.max(code);
            }
            Err(e) => {
                eprintln!("apex: {} could not run ({}): {e}", st.id, program.display());
                worst = worst.max(1);
            }
        }
    }
    worst
}

// ── reset ────────────────────────────────────────────────────────────────────

/// The invoking user's home, validated.
///
/// Validation, not convenience. Everything a reset removes is resolved under
/// this path, so a `$HOME` of `/` would turn `.config/apex-shell/input.json`
/// into `/.config/apex-shell/input.json` and, worse, would make the
/// "is the resolved path inside the home" check pass for anything at all. So:
/// absolute, an existing directory, at least two components deep, and not one
/// of the shared parents that are never anybody's home.
fn user_home() -> Option<PathBuf> {
    let raw = std::env::var_os("HOME").filter(|v| !v.is_empty())?;
    let p = PathBuf::from(raw);
    if !p.is_absolute() || !p.is_dir() {
        return None;
    }
    let real = std::fs::canonicalize(&p).ok()?;
    let s = real.to_string_lossy();
    if matches!(s.as_ref(), "/" | "/home" | "/var/home" | "/usr" | "/etc" | "/var") {
        return None;
    }
    if real.components().count() < 3 {
        // `/x` is one root plus one component. A real home is at least
        // `/home/<user>` or `/var/home/<user>`.
        return None;
    }
    Some(real)
}

/// One resolved reset target.
struct Planned {
    target: &'static Target,
    path: PathBuf,
    exists: bool,
}

/// Why a path was refused. Every one of these means nothing is deleted.
fn safe_to_touch(home: &Path, t: &Target, path: &Path) -> Result<(), String> {
    // The final component must not be a symlink. A symlink at the target path
    // pointing somewhere else is the one input a naive prefix check passes and
    // a recursive delete then follows out of the tree. `symlink_metadata` does
    // not follow, which is the whole reason it is used here.
    match std::fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_symlink() => {
            return Err(format!("{} is a symlink; refusing to remove it", path.display()))
        }
        Ok(m) => {
            // The declared kind is enforced, so a directory sitting where the
            // table declares a file can never be removed recursively.
            if t.kind == Kind::Dir && !m.is_dir() {
                return Err(format!("{} is not a directory but the table says it is", path.display()));
            }
            if t.kind == Kind::File && !m.is_file() {
                return Err(format!("{} is not a regular file but the table says it is", path.display()));
            }
        }
        Err(_) => return Ok(()), // absent: nothing to do, nothing to refuse
    }
    // Resolve the PARENT and rebuild the path, then compare. Canonicalising
    // the target itself would follow a symlink and hide the very thing the
    // check above refuses; canonicalising the parent catches a symlinked
    // directory higher up.
    let parent = path.parent().ok_or_else(|| format!("{} has no parent", path.display()))?;
    let real_parent = std::fs::canonicalize(parent)
        .map_err(|e| format!("cannot resolve {}: {e}", parent.display()))?;
    let name = path
        .file_name()
        .ok_or_else(|| format!("{} has no final component", path.display()))?;
    let resolved = real_parent.join(name);
    if !real_parent.starts_with(home) {
        return Err(format!(
            "{} resolves to {}, which is outside {}",
            path.display(),
            resolved.display(),
            home.display()
        ));
    }
    if resolved == home {
        return Err(format!("{} resolves to the home directory itself", path.display()));
    }
    Ok(())
}

fn plan(home: &Path, scope: ResetScope) -> Vec<Planned> {
    targets(scope)
        .into_iter()
        .map(|t| {
            let path = home.join(t.rel);
            let exists = std::fs::symlink_metadata(&path).is_ok();
            Planned { target: t, path, exists }
        })
        .collect()
}

/// Paths a plan would actually change — the set the confirmation token binds
/// to.
fn token_paths(planned: &[Planned]) -> Vec<String> {
    planned
        .iter()
        .filter(|p| p.exists)
        .map(|p| p.path.to_string_lossy().to_string())
        .collect()
}

fn cmd_reset(args: ResetArgs) -> i32 {
    // Root is refused outright, and not as a formality. Root's home is not the
    // user's, so a `sudo apex recover reset` would reset root's desktop and
    // leave the user's untouched while reporting success — a destructive verb
    // that acts on the wrong account is worse than one that refuses.
    if crate::ops::effective_uid() == Some(0) {
        eprintln!(
            "apex: `recover reset` is per-account and must not run as root.\n\
             \x20      Root's home is not yours, so this would reset root's desktop\n\
             \x20      and leave yours exactly as it is — while reporting success.\n\
             \x20      Run it as yourself, without sudo."
        );
        return 1;
    }

    let scope: ResetScope = match args.scope.parse() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("apex: {e}");
            return 2;
        }
    };
    let Some(home) = user_home() else {
        eprintln!(
            "apex: $HOME is unset, is not an existing directory, or names a\n\
             \x20      shared parent (/, /home, /var/home). Everything this verb\n\
             \x20      removes is resolved under $HOME, so it refuses rather than\n\
             \x20      guessing."
        );
        return 1;
    };

    let planned = plan(&home, scope);
    let paths = token_paths(&planned);
    let token = confirm_token(scope, &paths);
    let provisioner = Sys::from_env().path(PROVISIONER);

    // ── the plan, printed the same way whether or not this run commits ──────
    if args.json {
        let doc = json!({
            "scope": scope.as_str(),
            "summary": scope.summary(),
            "confirmToken": token,
            "committed": args.commit,
            "targets": planned.iter().map(|p| json!({
                "path": p.path.to_string_lossy(),
                "relative": p.target.rel,
                "disposition": p.target.how.as_str(),
                "kind": if p.target.kind == Kind::Dir { "dir" } else { "file" },
                "exists": p.exists,
                "backedUp": p.target.worth_backing_up(),
                "what": p.target.what,
            })).collect::<Vec<_>>(),
            "preserved": preserved(scope),
            "preservedLandmarks": PRESERVED_LANDMARKS,
            "reprovision": !args.no_reprovision,
            "provisioner": provisioner.to_string_lossy(),
        });
        println!("{}", serde_json::to_string_pretty(&doc).unwrap_or_default());
    } else {
        print!("{}", render_reset_plan(scope, &planned, &token, args.commit, args.no_reprovision));
    }

    if !args.commit {
        return 0;
    }

    // ── the confirmation ───────────────────────────────────────────────────
    let Some(given) = args.confirm.as_deref() else {
        eprintln!(
            "\napex: --commit needs --confirm. Nothing has been changed.\n\
             \x20      The token is derived from this scope AND the exact set of\n\
             \x20      paths above, so it cannot be constructed without running\n\
             \x20      the plan — which is the step that prints what is lost.\n\
             \x20      run: apex recover reset --scope {} --commit --confirm {}",
            scope.as_str(),
            token
        );
        return 2;
    };
    if given != token {
        eprintln!(
            "\napex: the confirmation does not match this plan. Nothing has been changed.\n\
             \x20      given    : {given}\n\
             \x20      expected : {token}\n\
             \x20      A mismatch means the machine changed since the plan was\n\
             \x20      printed, or the token was constructed rather than read.\n\
             \x20      Re-run without --commit and use the token it prints."
        );
        return 2;
    }

    if !args.no_reprovision && !provisioner.is_file() {
        eprintln!(
            "\napex: {} is missing, so this reset could not put back the files it\n\
             \x20      removes. Refusing rather than leaving the desktop with no\n\
             \x20      configuration and no way to regenerate it.\n\
             \x20      On an APEX machine this is an image defect — `sudo apex update`.\n\
             \x20      To take the deletion alone anyway: --no-reprovision",
            provisioner.display()
        );
        return 1;
    }

    // ── landmarks, before ──────────────────────────────────────────────────
    // Snapshotted rather than assumed present: a landmark that never existed
    // cannot be asserted afterwards, and asserting it would fail every reset
    // on a machine with no ~/.gnupg.
    let landmarks_before: Vec<&&str> = PRESERVED_LANDMARKS
        .iter()
        .filter(|l| std::fs::symlink_metadata(home.join(l)).is_ok())
        .collect();

    // ── safety pass: every target, before anything is touched ──────────────
    // All-or-nothing on purpose. A reset that removed four paths and then
    // refused the fifth would leave a state nobody planned.
    for p in planned.iter().filter(|p| p.exists) {
        if let Err(why) = safe_to_touch(&home, p.target, &p.path) {
            eprintln!("\napex: refusing this reset — {why}");
            eprintln!("apex: nothing has been changed.");
            return 1;
        }
    }

    // ── backup ─────────────────────────────────────────────────────────────
    // Outside every target, and named so it is obvious. `~/.local/state/apex`
    // is itself a target at user scope, so the backup cannot live under it.
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let backup = home.join(format!("apex-reset-backup-{stamp}"));
    let mut backed_up = 0usize;
    let wants_backup = planned
        .iter()
        .any(|p| p.exists && p.target.worth_backing_up());
    if wants_backup {
        if let Err(e) = std::fs::create_dir_all(&backup) {
            eprintln!(
                "\napex: cannot create the backup directory {}: {e}\n\
                 apex: nothing has been changed.",
                backup.display()
            );
            return 1;
        }
    }
    for p in planned.iter().filter(|p| p.exists && p.target.worth_backing_up()) {
        let dest = backup.join(p.target.rel);
        if let Some(parent) = dest.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                eprintln!("\napex: cannot prepare {}: {e}", parent.display());
                eprintln!("apex: nothing has been changed.");
                return 1;
            }
        }
        if let Err(e) = copy_tree(&p.path, &dest) {
            eprintln!("\napex: cannot back up {}: {e}", p.path.display());
            eprintln!("apex: nothing has been changed.");
            return 1;
        }
        backed_up += 1;
    }
    if backed_up > 0 {
        println!("\nbacked up {backed_up} path(s) to {}", backup.display());
    }

    // ── perform ────────────────────────────────────────────────────────────
    let mut removed = 0usize;
    let mut emptied = 0usize;
    for p in planned.iter().filter(|p| p.exists) {
        let r = match (p.target.how, p.target.kind) {
            (Disposition::Delete, Kind::Dir) => std::fs::remove_dir_all(&p.path),
            (Disposition::Delete, Kind::File) => std::fs::remove_file(&p.path),
            // Emptied in place: hyprland.conf `source=`s these and Hyprland
            // treats a source with no match as a fatal config error, so a
            // delete here takes the whole session's configuration with it.
            (Disposition::Truncate, _) => std::fs::write(&p.path, b""),
        };
        match r {
            Ok(()) => {
                if p.target.how == Disposition::Truncate {
                    emptied += 1;
                    println!("emptied {}", p.path.display());
                } else {
                    removed += 1;
                    println!("removed {}", p.path.display());
                }
            }
            Err(e) => eprintln!("apex: could not change {}: {e}", p.path.display()),
        }
    }
    println!("\nremoved {removed} path(s), emptied {emptied}.");

    // ── landmarks, after ───────────────────────────────────────────────────
    // The postcondition that catches a table entry which somehow widened.
    // Grepping for what you deleted cannot detect what you deleted as well.
    let mut lost: Vec<String> = Vec::new();
    for l in &landmarks_before {
        if std::fs::symlink_metadata(home.join(**l)).is_err() {
            lost.push((**l).to_string());
        }
    }
    if !lost.is_empty() {
        eprintln!(
            "\napex: FAILURE — this reset removed something it promised to preserve:\n\
             \x20       {}\n\
             \x20     This is a defect in the reset table, not a normal outcome.\n\
             \x20     {}",
            lost.join(", "),
            if backed_up > 0 {
                format!("A backup of what was removed is in {}.", backup.display())
            } else {
                "Nothing was backed up, because no target asked for it.".to_string()
            }
        );
        return 1;
    }
    println!("preserved {} landmark(s), re-checked after the fact.", landmarks_before.len());

    // ── re-seed ────────────────────────────────────────────────────────────
    if args.no_reprovision {
        println!(
            "\n--no-reprovision: the files APEX Shell needs were NOT put back.\n\
             Log in again, or run {}, before starting a session.",
            provisioner.display()
        );
        return 0;
    }
    eprintln!("apex: running: {}", provisioner.display());
    let rc = match Command::new(&provisioner).status() {
        Ok(s) => s.code().unwrap_or(-1),
        Err(e) => {
            eprintln!("apex: the provisioner could not run: {e}");
            -1
        }
    };
    // Postcondition on the reseed, not just its exit status: the provisioner
    // is `set -e` but writes several files best-effort, so "exited 0" and
    // "the files are back" are two different claims.
    let mut absent: Vec<String> = Vec::new();
    for rel in [
        ".config/apex-shell",
        ".config/hypr/apex-input.conf",
        ".config/hypr/apex-display.conf",
    ] {
        if !home.join(rel).exists() {
            absent.push(rel.to_string());
        }
    }
    if rc != 0 || !absent.is_empty() {
        eprintln!(
            "\napex: the reset completed but the desktop was not fully re-seeded\n\
             \x20      (provisioner exit {rc}{}).\n\
             \x20      Log out and back in — the provisioner runs at every login and\n\
             \x20      self-heals. If it still does not, `sudo apex update`.",
            if absent.is_empty() {
                String::new()
            } else {
                format!(", still absent: {}", absent.join(", "))
            }
        );
        return 1;
    }
    println!("re-seeded the desktop. Log out and back in for the compositor to re-read its configuration.");
    0
}

/// Copy a file or a whole directory. Used only for the pre-reset backup.
fn copy_tree(src: &Path, dest: &Path) -> std::io::Result<()> {
    let meta = std::fs::symlink_metadata(src)?;
    if meta.is_dir() {
        std::fs::create_dir_all(dest)?;
        for e in std::fs::read_dir(src)? {
            let e = e?;
            copy_tree(&e.path(), &dest.join(e.file_name()))?;
        }
        return Ok(());
    }
    if meta.file_type().is_symlink() {
        // Not followed, and not recreated. A symlink in the backup would point
        // at the same place the original did, which is not a copy of anything.
        return Ok(());
    }
    std::fs::copy(src, dest).map(|_| ())
}

fn render_reset_plan(
    scope: ResetScope,
    planned: &[Planned],
    token: &str,
    committing: bool,
    no_reprovision: bool,
) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "Factory reset — {}",
        if committing {
            "COMMITTING"
        } else {
            "DRY RUN. Nothing has been changed."
        }
    );
    let _ = writeln!(out, "\nscope: {} — {}", scope.as_str(), wrap(scope.summary(), 7));

    let del: Vec<&Planned> = planned
        .iter()
        .filter(|p| p.exists && p.target.how == Disposition::Delete)
        .collect();
    let trunc: Vec<&Planned> = planned
        .iter()
        .filter(|p| p.exists && p.target.how == Disposition::Truncate)
        .collect();
    let absent: Vec<&Planned> = planned.iter().filter(|p| !p.exists).collect();

    let _ = writeln!(out, "\nWILL BE REMOVED ({}):", del.len());
    if del.is_empty() {
        let _ = writeln!(out, "  (nothing — none of these paths exists)");
    }
    for p in &del {
        let _ = writeln!(out, "  {}", p.path.display());
        let _ = writeln!(out, "      {}", wrap(p.target.what, 6));
    }
    let _ = writeln!(out, "\nWILL BE EMPTIED, NOT REMOVED ({}):", trunc.len());
    if trunc.is_empty() {
        let _ = writeln!(out, "  (nothing)");
    }
    for p in &trunc {
        let _ = writeln!(out, "  {}", p.path.display());
        let _ = writeln!(out, "      {}", wrap(p.target.what, 6));
    }
    if !absent.is_empty() {
        let _ = writeln!(out, "\nNOT PRESENT, NOTHING TO DO ({}):", absent.len());
        for p in &absent {
            let _ = writeln!(out, "  {}", p.path.display());
        }
    }

    let _ = writeln!(out, "\nPRESERVED:");
    for k in preserved(scope) {
        let _ = writeln!(out, "  - {}", wrap(k, 4));
    }
    let _ = writeln!(
        out,
        "\nEverything removed is copied to ~/apex-reset-backup-<timestamp> first,\n\
         except caches. Delete that directory yourself once you are sure."
    );
    if no_reprovision {
        let _ = writeln!(
            out,
            "\n--no-reprovision: the desktop will NOT be re-seeded afterwards."
        );
    } else {
        let _ = writeln!(
            out,
            "\nAfterwards {PROVISIONER} re-seeds the files APEX Shell needs, and this\n\
             verb re-checks that they came back. Log out and back in for the\n\
             compositor to re-read its configuration."
        );
    }
    if !committing {
        let _ = writeln!(
            out,
            "\nTo perform it, run exactly:\n  apex recover reset --scope {} --commit --confirm {}",
            scope.as_str(),
            token
        );
    }
    out
}

pub fn main(cmd: RecoverCmd) -> i32 {
    match cmd {
        RecoverCmd::Status { json } => cmd_status(json),
        RecoverCmd::Repair(args) => cmd_repair(args),
        RecoverCmd::Reset(args) => cmd_reset(args),
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────
// The parsers here decide what the surface says about a machine, and each one
// reads a file format this suite can present exactly. `tests/test-apex-recover
// .sh` drives the shipped binary against whole fixture trees; these pin the
// individual readers, where a wrong answer would be a plausible-looking
// sentence rather than a crash.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_booted_deployment_is_read_out_of_the_ostree_argument() {
        // A real APEX kernel command line, GRUB/BLS shape.
        let cmdline = "BOOT_IMAGE=(hd0,gpt2)/ostree/apex-1f0d/vmlinuz-7.1.5 \
                       root=UUID=abc ostree=/ostree/boot.1/apex/\
                       8f14e45fceea167a5a36dedd4bea2543f14e45fceea167a5a36dedd4bea25431/0 \
                       rw quiet";
        assert_eq!(
            booted_deployment(cmdline).as_deref(),
            Some("8f14e45fceea167a5a36dedd4bea2543f14e45fceea167a5a36dedd4bea25431")
        );
        // The `.0` serial suffix of a deploy-path layout must be stripped, not
        // included: the checksum is the identity `ostree admin pin` keys on.
        let deploy = "ostree=/ostree/deploy/apex/deploy/\
                      8f14e45fceea167a5a36dedd4bea2543f14e45fceea167a5a36dedd4bea25431.0";
        assert_eq!(
            booted_deployment(deploy).as_deref(),
            Some("8f14e45fceea167a5a36dedd4bea2543f14e45fceea167a5a36dedd4bea25431")
        );
        // No ostree argument at all: an honest None, never a guess.
        assert_eq!(booted_deployment("root=UUID=abc rw quiet"), None);
        // Short non-hex components must not be mistaken for a checksum.
        assert_eq!(booted_deployment("ostree=/ostree/boot.1/apex/short/0"), None);
    }

    #[test]
    fn usr_readonly_prefers_the_usr_mount_and_falls_back_to_root() {
        // An ostree machine: / is the composefs overlay, /usr has its own ro
        // mount.
        let m = "overlay / overlay ro,relatime,lowerdir=x 0 0\n\
                 none /usr overlay ro,relatime 0 0\n\
                 tmpfs /run tmpfs rw,nosuid 0 0\n";
        assert_eq!(usr_readonly(m), (Some(true), Some("overlay".to_string())));
        // No /usr line: the root mount's flags answer instead of the row
        // reporting drift on a machine that has none.
        let m2 = "overlay / overlay ro,relatime 0 0\ntmpfs /run tmpfs rw 0 0\n";
        assert_eq!(usr_readonly(m2), (Some(true), Some("overlay".to_string())));
        // A genuinely writable /usr is the drift the row exists to report.
        let m3 = "overlay / overlay ro 0 0\nnone /usr overlay rw,relatime 0 0\n";
        assert_eq!(usr_readonly(m3), (Some(false), Some("overlay".to_string())));
        // `ro` must be matched as a whole option. `rootcontext=` and
        // `errors=remount-ro` both contain the two letters.
        let m4 = "/dev/sda2 /usr ext4 rw,errors=remount-ro,rootcontext=x 0 0\n";
        assert_eq!(usr_readonly(m4).0, Some(false));
        // Nothing at all: unavailable, not "read-write".
        assert_eq!(usr_readonly(""), (None, None));
    }

    #[test]
    fn a_default_route_is_read_never_probed() {
        let table = "Iface\tDestination\tGateway \tFlags\n\
                     wlan0\t00000000\t0101A8C0\t0003\n\
                     wlan0\t0001A8C0\t00000000\t0001\n";
        assert!(has_default_route(table));
        // Only non-default routes: a machine on a LAN with no gateway.
        let no_default = "Iface\tDestination\tGateway\n\
                          wlan0\t0001A8C0\t00000000\t0001\n";
        assert!(!has_default_route(no_default));
        // The header alone, and an empty file.
        assert!(!has_default_route("Iface\tDestination\tGateway\n"));
        assert!(!has_default_route(""));
    }

    #[test]
    fn os_release_values_are_unquoted() {
        let dir = std::env::temp_dir().join(format!("apex-recover-osr-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("etc")).unwrap();
        std::fs::write(
            dir.join("etc/os-release"),
            "NAME=\"APEX-OS\"\n# a comment\nVERSION_ID=43\nVARIANT_ID='gaming'\n\n",
        )
        .unwrap();
        let sys = Sys { fixture: Some(dir.clone()) };
        let m = os_release(&sys);
        assert_eq!(m.get("NAME").map(String::as_str), Some("APEX-OS"));
        assert_eq!(m.get("VERSION_ID").map(String::as_str), Some("43"));
        assert_eq!(m.get("VARIANT_ID").map(String::as_str), Some("gaming"));
        assert!(!m.contains_key("# a comment"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_fixture_root_is_a_prefix_and_never_falls_through_to_the_real_system() {
        // The mistake this guards: `Path::join` on an absolute argument
        // discards the prefix and reads the real machine, so a test would pass
        // on the author's laptop and assert nothing.
        let sys = Sys { fixture: Some(PathBuf::from("/tmp/fixture")) };
        assert_eq!(sys.path("/proc/cmdline"), PathBuf::from("/tmp/fixture/proc/cmdline"));
        assert_eq!(
            sys.path("/usr/libexec/apex-shell-firstrun"),
            PathBuf::from("/tmp/fixture/usr/libexec/apex-shell-firstrun")
        );
        let real = Sys { fixture: None };
        assert_eq!(real.path("/proc/cmdline"), PathBuf::from("/proc/cmdline"));
    }

    #[test]
    fn every_repair_step_has_a_diagnosis() {
        // A step in the table with no branch in `applicable_repairs` would be
        // silently never offered — or, with a permissive default, offered
        // always. Both are wrong, and this is the check that keeps the two
        // lists in step.
        let known = ["reprovision-desktop", "rebuild-package-extension"];
        for step in REPAIRS {
            assert!(
                known.contains(&step.id),
                "{} has no diagnosis in applicable_repairs()",
                step.id
            );
        }
        assert_eq!(known.len(), REPAIRS.len());
    }

    #[test]
    fn the_doctor_json_carries_every_check_and_the_counts() {
        let checks = vec![
            Check { ok: true, what: "one".into() },
            Check { ok: false, what: "two \"quoted\"".into() },
        ];
        let out = render_doctor(&checks, true);
        let v: Value = serde_json::from_str(&out).expect("valid JSON");
        assert_eq!(v["total"], 2);
        assert_eq!(v["passed"], 1);
        assert_eq!(v["warned"], 1);
        assert_eq!(v["checks"][1]["check"], "two \"quoted\"");
        assert_eq!(v["checks"][1]["ok"], false);
        // And the text form is unchanged, because scripts already read it.
        let text = render_doctor(&checks, false);
        assert!(text.contains("[PASS] one"));
        assert!(text.contains("[WARN] two"));
    }

    #[test]
    fn the_surface_reports_every_row_even_on_an_empty_fixture() {
        // A machine that is nothing like an APEX install: no /proc, no
        // /ostree, no os-release. Every row must still be present and say
        // something, because a UI keyed on the row ids must not lose a row
        // when a read fails.
        let dir = std::env::temp_dir().join(format!("apex-recover-empty-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let s = probe(&Sys { fixture: Some(dir.clone()) });
        let ids: Vec<&str> = s.rows.iter().map(|r| r.id).collect();
        let want: Vec<&str> = apexd_core::recover::ROWS.iter().map(|r| r.id).collect();
        assert_eq!(ids, want);
        for r in &s.rows {
            assert!(!r.detail.is_empty(), "{} has no detail", r.id);
        }
        assert_eq!(s.bootloader, "unknown");
        // And a route list that always names the boot-entry decision.
        assert!(s.routes.iter().any(|r| r.id == "recovery-boot-entry"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_symlinked_target_is_refused_rather_than_followed() {
        let base = std::env::temp_dir().join(format!("apex-recover-sym-{}", std::process::id()));
        let home = base.join("home");
        let outside = base.join("outside");
        std::fs::create_dir_all(home.join(".cache")).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("precious"), b"do not delete").unwrap();
        let link = home.join(".cache/apex-shell");
        std::os::unix::fs::symlink(&outside, &link).unwrap();

        let t = targets(ResetScope::Desktop)
            .into_iter()
            .find(|t| t.rel == ".cache/apex-shell")
            .unwrap();
        let home_real = std::fs::canonicalize(&home).unwrap();
        let err = safe_to_touch(&home_real, t, &link).expect_err("must refuse");
        assert!(err.contains("symlink"), "wrong refusal: {err}");
        // And the thing it pointed at is untouched — the assertion that makes
        // the refusal mean something.
        assert!(outside.join("precious").exists());
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn a_target_whose_parent_escapes_the_home_is_refused() {
        let base = std::env::temp_dir().join(format!("apex-recover-esc-{}", std::process::id()));
        let home = base.join("home");
        let elsewhere = base.join("elsewhere/apex-shell");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::fs::write(elsewhere.join("keep"), b"x").unwrap();
        // A symlinked PARENT: ~/.cache -> ../elsewhere. The final component is
        // a real directory, so the symlink check above does not fire and the
        // parent resolution is what has to catch it.
        std::os::unix::fs::symlink(base.join("elsewhere"), home.join(".cache")).unwrap();

        let t = targets(ResetScope::Desktop)
            .into_iter()
            .find(|t| t.rel == ".cache/apex-shell")
            .unwrap();
        let home_real = std::fs::canonicalize(&home).unwrap();
        let err = safe_to_touch(&home_real, t, &home_real.join(t.rel)).expect_err("must refuse");
        assert!(err.contains("outside"), "wrong refusal: {err}");
        assert!(elsewhere.join("keep").exists());
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn a_file_target_that_is_really_a_directory_is_refused() {
        // Without this, a directory sitting where the table declares a file
        // would be passed to `remove_file`, which fails — but the check is
        // what makes the refusal explicit rather than an errno.
        let base = std::env::temp_dir().join(format!("apex-recover-kind-{}", std::process::id()));
        let home = base.join("home");
        std::fs::create_dir_all(home.join(".config/apex-shell/input.json")).unwrap();
        let t = targets(ResetScope::Desktop)
            .into_iter()
            .find(|t| t.rel == ".config/apex-shell/input.json")
            .unwrap();
        let home_real = std::fs::canonicalize(&home).unwrap();
        let err = safe_to_touch(&home_real, t, &home_real.join(t.rel)).expect_err("must refuse");
        assert!(err.contains("not a regular file"), "wrong refusal: {err}");
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn a_plan_finds_only_what_exists_and_the_token_follows_it() {
        let base = std::env::temp_dir().join(format!("apex-recover-plan-{}", std::process::id()));
        let home = base.join("home");
        std::fs::create_dir_all(home.join(".config/apex-shell")).unwrap();
        std::fs::write(home.join(".config/apex-shell/input.json"), b"{}").unwrap();
        let home_real = std::fs::canonicalize(&home).unwrap();

        let p1 = plan(&home_real, ResetScope::Desktop);
        assert_eq!(token_paths(&p1).len(), 1);
        let t1 = confirm_token(ResetScope::Desktop, &token_paths(&p1));

        // One more file appears: the token must change, so a confirmation
        // printed before it is refused afterwards.
        std::fs::write(home_real.join(".config/apex-shell/display.json"), b"{}").unwrap();
        let p2 = plan(&home_real, ResetScope::Desktop);
        assert_eq!(token_paths(&p2).len(), 2);
        assert_ne!(t1, confirm_token(ResetScope::Desktop, &token_paths(&p2)));
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn the_rendered_plan_names_the_loss_and_the_exact_command() {
        let base = std::env::temp_dir().join(format!("apex-recover-render-{}", std::process::id()));
        let home = base.join("home");
        std::fs::create_dir_all(home.join(".config/apex")).unwrap();
        std::fs::write(home.join(".config/apex/blueprint.toml"), b"x").unwrap();
        let home_real = std::fs::canonicalize(&home).unwrap();
        let planned = plan(&home_real, ResetScope::User);
        let token = confirm_token(ResetScope::User, &token_paths(&planned));
        let text = render_reset_plan(ResetScope::User, &planned, &token, false, false);

        // The blueprint must be named, with its own loss line, not merely
        // counted. It is the one file in APEX whose contract is that no
        // program writes it.
        assert!(text.contains("blueprint.toml"));
        assert!(text.contains("apex sync export"));
        // The preserved list must be there, or "explicit about what is
        // preserved" is not satisfied.
        assert!(text.contains("PRESERVED:"));
        assert!(text.contains(".ssh"));
        // And the one command line that performs it, carrying this plan's
        // token.
        assert!(text.contains(&format!("--confirm {token}")));
        assert!(text.contains("DRY RUN"));
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn wrap_collapses_source_indentation_and_bounds_the_width() {
        // The details are multi-line Rust string literals, so they arrive
        // carrying their own source indentation. A report that printed that
        // verbatim would have ragged gaps mid-sentence.
        let ragged = "one two    three\n                     four five";
        assert_eq!(wrap(ragged, 0), "one two three four five");

        let long = "one two three four five six seven eight nine ten eleven twelve \
                    thirteen fourteen fifteen sixteen seventeen eighteen nineteen twenty \
                    twentyone twentytwo twentythree twentyfour twentyfive twentysix";
        let w = wrap(long, 10);
        let mut lines = w.lines();
        // The first line carries no indent of its own: the caller has already
        // printed a column before it.
        let first = lines.next().expect("at least one line");
        assert!(!first.starts_with(' '));
        // Continuations are indented by exactly the requested amount, and the
        // word after the indent is a word rather than more whitespace.
        let mut continuations = 0;
        for l in lines {
            assert_eq!(&l[..10], "          ", "continuation not indented by 10");
            assert!(!l[10..].starts_with(' '), "double indent on a continuation");
            continuations += 1;
        }
        assert!(continuations >= 1, "the long line was never wrapped");
        for l in w.lines() {
            assert!(l.len() <= 96, "line too long: {} chars", l.len());
        }
        assert_eq!(wrap("", 4), "");
    }
}
