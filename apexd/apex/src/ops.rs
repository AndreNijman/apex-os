//! Non-D-Bus operations: shelling out to bootc/ostree/fwupd for update,
//! rollback, pin and changelog, plus local read-only rendering used both as a
//! daemon-less fallback and by `apex fingerprint`/`doctor`.

use std::path::Path;
use std::process::Command;
use std::time::Instant;

use apexd_core::tier::Tier;
use apexd_core::{Fingerprint, Profile, ProfileSet, Selection};

// ── Root gating ──────────────────────────────────────────────────────────────
//
// `update`, `rollback` and `pin` drive bootc/ostree, which write to /ostree and
// /boot. Run as an ordinary user they used to reach the external tool and fail
// there, with whatever wording bootc chose — typically a bare permission error
// that says nothing about sudo. Worse, `apex update`'s firmware half ran
// afterwards regardless, so the command printed a wall of fwupd output and
// could still exit 0 having updated nothing.
//
// So these verbs now refuse up front, before any hardware probe, D-Bus connect
// or subprocess, and say exactly what to type instead.
//
// This is deliberately NOT applied to the whole CLI. `apex tier`, `status`,
// `battery`, `fan`, `game` and `doctor` are reached by APEX Shell's power tab
// as the session user — mutations go through apexd's polkit-authorised D-Bus
// API, which is precisely how an unprivileged desktop is supposed to change
// power state. A blanket root requirement would break the desktop's power
// controls to fix a message.

/// Effective UID, read from `/proc/self/status`.
///
/// `/proc` rather than a libc call: this crate has no C dependency and adding
/// one for a single integer is not worth it on a Linux-only OS CLI. Returns
/// `None` if /proc is unavailable or malformed, which the caller treats as
/// "not root" — failing closed.
pub fn effective_uid() -> Option<u32> {
    parse_effective_uid(&std::fs::read_to_string("/proc/self/status").ok()?)
}

/// Pull the effective UID out of `/proc/self/status`.
///
/// The line is `Uid:\t<real>\t<effective>\t<saved>\t<fs>`. The EFFECTIVE id is
/// the one that matters: it is what the kernel checks, and it is what differs
/// under a setuid path.
pub fn parse_effective_uid(status: &str) -> Option<u32> {
    status
        .lines()
        .find_map(|l| l.strip_prefix("Uid:"))
        .and_then(|rest| rest.split_whitespace().nth(1))
        .and_then(|euid| euid.parse().ok())
}

/// How the user invoked us, rendered as the sudo command to run instead.
///
/// The full argument list is echoed, not a bare `sudo apex <verb>`: someone who
/// typed `apex update --check --skip-firmware` should be able to copy one line,
/// not reconstruct their own flags.
fn sudo_reinvocation(argv: &[String]) -> String {
    let mut out = String::from("sudo apex");
    for a in argv.iter().skip(1) {
        out.push(' ');
        out.push_str(a);
    }
    out
}

/// The refusal text for a root-only verb.
fn root_required_message(verb: &str, argv: &[String]) -> String {
    format!(
        "apex: '{verb}' changes the booted system and must run as root.\n\
         \x20      try:  {}\n\
         \x20      (being in the wheel group is not enough — bootc writes to \
         /ostree and /boot,\n\
         \x20       so the command itself has to run with privileges.)",
        sudo_reinvocation(argv)
    )
}

/// Refuse a root-only verb when we are not root.
///
/// Returns `Err(exit_code)` after printing the refusal, so callers can bail
/// before touching anything.
pub fn require_root(verb: &str) -> Result<(), i32> {
    if effective_uid() == Some(0) {
        return Ok(());
    }
    let argv: Vec<String> = std::env::args().collect();
    eprintln!("{}", root_required_message(verb, &argv));
    Err(1)
}

/// Run an external command, streaming its output. Returns Ok(code) or a clear
/// message if the binary is missing — never panics.
pub fn run(program: &str, args: &[&str]) -> Result<i32, String> {
    eprintln!("apex: running: {program} {}", args.join(" "));
    match Command::new(program).args(args).status() {
        Ok(status) => Ok(status.code().unwrap_or(-1)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Err(format!("'{program}' not found on PATH ({e})"))
        }
        Err(e) => Err(format!("failed to run '{program}': {e}")),
    }
}

/// Capture stdout of a command (trimmed). None if it cannot run.
fn capture(program: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(program).args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// `apex pin` -> pin the current (booted) deployment so an update can't garbage
/// collect the rollback target.
pub fn pin() -> i32 {
    match run("ostree", &["admin", "pin", "0"]) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("apex: pin failed: {e}");
            1
        }
    }
}

/// `apex rollback` -> boot the previous deployment next reboot.
pub fn rollback() -> i32 {
    match run("bootc", &["rollback"]) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("apex: rollback failed: {e}");
            1
        }
    }
}

// ── fwupd exit codes ─────────────────────────────────────────────────────────
// fwupdmgr does NOT use shell conventions. It returns:
//     0  success
//     1  failure
//     2  nothing to do        (EXIT_NOTHING_TO_DO)
//     3  nothing found        (EXIT_NOT_FOUND — e.g. no LVFS-covered devices)
// Both 2 and 3 are ordinary, expected outcomes on a laptop that is already
// current, and treating them as failure is how `apex update` would come to
// report an error on the most common path of all.
const FWUPD_NOTHING_TO_DO: i32 = 2;
const FWUPD_NOT_FOUND: i32 = 3;

fn fwupd_idle(code: i32) -> bool {
    code == FWUPD_NOTHING_TO_DO || code == FWUPD_NOT_FOUND
}


// ── ostree fsync during a pull ───────────────────────────────────────────────
//
// MEASURED ON THE AUTHOR'S L16, because "updates feel slow" deserved a number
// rather than a guess:
//
//   single-stream download from GHCR ... 14.6 MiB/s   (51 ms RTT, curl)
//   6 parallel streams ................. 49.8 MiB/s
//   what `apex update` actually got ....  ~8 MiB/s
//   disk write throughput .............. 999 MB/s
//   fsync cost per small file ..........  2.98 ms   (131x slower than without)
//   objects in the ostree repo ......... 179,365
//
// The network was never the limit and neither was the disk. `core.fsync` is
// unset in the repo, which means ostree fsyncs EVERY object it writes, and
// 179k objects x 2.98 ms is ~534 s of pure fsync serialised against ~372 s of
// download. That models to 6.0 MiB/s against the ~8 observed — fsync is the
// dominant cost of an update, not bandwidth.
//
// So it is turned off for the duration of the pull and restored afterwards.
//
// THE TRADE, stated plainly: fsync is what guarantees a written object survives
// a power loss. With it off, losing power mid-pull can leave a corrupt object in
// the repo. What that costs is bounded — the BOOTED deployment is never touched
// by a pull, ostree checksums every object it reads, and the remedy is to pull
// again (`ostree fsck` reports it, `apex update` re-fetches). Weighed against
// halving the time the machine spends updating, on a laptop with a battery, that
// is the right default. `apex update --fsync` keeps it on.
const OSTREE_REPO: &str = "/ostree/repo";
/// Records that a pull turned fsync off, so a run killed mid-pull can put it
/// back rather than leaving the repo permanently unsafe and silent about it.
/// Under /var so it survives the reboot a crash might cause.
const FSYNC_MARKER: &str = "/var/lib/apexos/fsync-disabled";

/// Reads `core.fsync`; `None` when unset (ostree's default, which is on).
fn ostree_fsync_setting() -> Option<String> {
    capture("ostree", &["config", "--repo", OSTREE_REPO, "get", "core.fsync"])
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn ostree_fsync_write(value: Option<&str>) -> bool {
    let args: Vec<&str> = match value {
        Some(v) => vec!["config", "--repo", OSTREE_REPO, "set", "core.fsync", v],
        None => vec!["config", "--repo", OSTREE_REPO, "unset", "core.fsync"],
    };
    matches!(
        Command::new("ostree").args(&args).output(),
        Ok(o) if o.status.success()
    )
}

/// Restores fsync if a previous run died with it disabled. Called before every
/// pull, so the unsafe window can never outlive one update by more than one.
fn recover_stale_fsync() {
    if !Path::new(FSYNC_MARKER).exists() {
        return;
    }
    let prior = std::fs::read_to_string(FSYNC_MARKER).unwrap_or_default();
    let prior = prior.trim();
    eprintln!(
        "apex: a previous update was interrupted with ostree fsync disabled — restoring it"
    );
    let restored = if prior.is_empty() {
        ostree_fsync_write(None)
    } else {
        ostree_fsync_write(Some(prior))
    };
    if restored {
        let _ = std::fs::remove_file(FSYNC_MARKER);
    } else {
        eprintln!("apex: WARNING could not restore core.fsync — run: sudo ostree config --repo={OSTREE_REPO} unset core.fsync");
    }
}

/// Disables `core.fsync` while alive, restores it on drop.
struct FsyncGuard {
    prior: Option<String>,
    active: bool,
}

impl FsyncGuard {
    /// `None` when fsync was left alone (asked not to, or ostree unavailable).
    fn disable() -> FsyncGuard {
        recover_stale_fsync();
        let prior = ostree_fsync_setting();
        // Already false — someone set it deliberately. Leave it, and leave no
        // marker, so we never "restore" a choice that was not ours.
        if prior.as_deref() == Some("false") {
            return FsyncGuard { prior: None, active: false };
        }
        if let Some(dir) = Path::new(FSYNC_MARKER).parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        // The marker is written BEFORE the change, so a crash in between leaves
        // a spurious marker (harmless: recovery just re-asserts the prior value)
        // rather than a disabled fsync nobody knows about.
        let _ = std::fs::write(FSYNC_MARKER, prior.clone().unwrap_or_default());
        if ostree_fsync_write(Some("false")) {
            eprintln!("apex: ostree fsync disabled for this pull (restored afterwards)");
            FsyncGuard { prior, active: true }
        } else {
            let _ = std::fs::remove_file(FSYNC_MARKER);
            eprintln!("apex: could not disable ostree fsync — the pull will be slower");
            FsyncGuard { prior: None, active: false }
        }
    }
}

impl Drop for FsyncGuard {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        if ostree_fsync_write(self.prior.as_deref()) {
            let _ = std::fs::remove_file(FSYNC_MARKER);
            eprintln!("apex: ostree fsync restored");
        } else {
            eprintln!(
                "apex: WARNING could not restore core.fsync — run: sudo ostree config --repo={OSTREE_REPO} unset core.fsync"
            );
        }
    }
}

/// What `apex update` should do this run.
#[derive(Default, Clone, Copy)]
pub struct UpdateOptions {
    /// Report what is available; download and stage nothing.
    pub check: bool,
    /// Skip the firmware (fwupd) pass entirely.
    pub skip_firmware: bool,
    /// Run only the firmware pass; leave the OS image alone.
    pub firmware_only: bool,
    /// Keep ostree's per-object fsync on during the pull. Slower — see the
    /// FsyncGuard notes — but durable against a power loss mid-update.
    pub keep_fsync: bool,
}

/// `apex update` -> pull a newer OS image, then refresh firmware via fwupd.
///
/// ── Why this is not just two `fwupdmgr` calls any more ──
/// The old version ran `fwupdmgr refresh --force` and then `fwupdmgr update -y`
/// unconditionally, every single time. `--force` means "ignore the cache age",
/// so every run re-downloaded the entire LVFS metadata index — tens of MB of
/// signed XML that fwupd itself only considers stale after 24 hours — and then
/// started a full device-enumeration update pass on a machine that, nine runs
/// in ten, had no firmware updates at all. On the author's L16 that was the
/// slowest part of `apex update` whenever the OS image was already current.
///
/// Now: refresh honours fwupd's own cache window, and the update pass runs only
/// after `get-updates` says there is something to install.
pub fn update(opts: UpdateOptions) -> i32 {
    let started = Instant::now();
    let mut worst = 0;

    if opts.check {
        if !opts.firmware_only {
            match run("bootc", &["upgrade", "--check"]) {
                Ok(code) => worst = worst.max(code),
                Err(e) => {
                    eprintln!("apex: cannot check for an OS update: {e}");
                    worst = 1;
                }
            }
        }
        if !opts.skip_firmware {
            match run("fwupdmgr", &["get-updates"]) {
                Ok(code) if fwupd_idle(code) => {
                    println!("apex: no firmware updates for this machine")
                }
                Ok(code) => worst = worst.max(code),
                Err(e) => eprintln!("apex: firmware check skipped: {e}"),
            }
        }
        return worst;
    }

    if !opts.firmware_only {
        // fsync off for the pull, restored when this drops — including on the
        // error paths below. See FsyncGuard for the measurements and the trade.
        let _fsync = if opts.keep_fsync {
            recover_stale_fsync();
            None
        } else {
            Some(FsyncGuard::disable())
        };
        // Deliberately NOT preceded by `bootc upgrade --check`: bootc already
        // no-ops when the booted image is current, and checking first would add
        // a second registry round-trip to the exact path we are trying to make
        // faster.
        match run("bootc", &["upgrade"]) {
            Ok(code) => worst = worst.max(code),
            Err(e) => {
                eprintln!("apex: OS update failed: {e}");
                worst = 1;
            }
        }
    }

    if !opts.skip_firmware {
        worst = worst.max(firmware_pass());
    }

    println!(
        "apex: update finished in {:.1}s",
        started.elapsed().as_secs_f64()
    );
    worst
}

/// The firmware half of `apex update`. Best-effort throughout: a machine may
/// have no fwupd, no LVFS-covered devices, or no network.
fn firmware_pass() -> i32 {
    // No `--force`. fwupd refreshes its metadata at most once every 24h by
    // design and reports "nothing to do" (2) inside that window; forcing it
    // re-downloaded the whole index on every invocation for no benefit.
    match run("fwupdmgr", &["refresh"]) {
        Ok(code) if fwupd_idle(code) => println!("apex: firmware metadata already current"),
        Ok(0) => {}
        Ok(code) => eprintln!("apex: fwupd refresh returned {code} — continuing anyway"),
        Err(e) => {
            eprintln!("apex: fwupd refresh skipped: {e}");
            return 0;
        }
    }

    // Ask before doing. `fwupdmgr update` on a machine with nothing to install
    // still enumerates every device and re-reads every plugin; `get-updates` is
    // the cheap question.
    match run("fwupdmgr", &["get-updates"]) {
        Ok(code) if fwupd_idle(code) => {
            println!("apex: no firmware updates for this machine");
            return 0;
        }
        Ok(0) => {}
        Ok(code) => {
            eprintln!("apex: fwupd get-updates returned {code} — skipping the firmware pass");
            return 0;
        }
        Err(e) => {
            eprintln!("apex: firmware update skipped: {e}");
            return 0;
        }
    }

    match run("fwupdmgr", &["update", "-y"]) {
        Ok(code) if fwupd_idle(code) => 0,
        Ok(code) => code,
        Err(e) => {
            eprintln!("apex: fwupd update skipped: {e}");
            0
        }
    }
}

/// `apex changelog` -> show the booted image and its OCI revision/version
/// labels (best-effort across bootc/rpm-ostree/skopeo).
pub fn changelog() -> i32 {
    if let Some(status) = capture("bootc", &["status"]) {
        println!("{status}");
        // Try to surface the image's git SHA / version labels if skopeo is
        // present and we can find the image ref.
        if let Some(image) = capture("bootc", &["status", "--format", "json"])
            .and_then(|j| extract_image_ref(&j))
        {
            println!("\nimage: {image}");
            if let Some(labels) = capture(
                "skopeo",
                &["inspect", "--format", "{{.Labels}}", &format!("docker://{image}")],
            ) {
                println!("labels: {labels}");
            }
        }
        return 0;
    }
    if let Some(status) = capture("rpm-ostree", &["status"]) {
        println!("{status}");
        return 0;
    }
    eprintln!("apex: neither bootc nor rpm-ostree available to read the changelog");
    1
}

/// Extremely small extractor for the `image` field of `bootc status --format
/// json` — avoids pulling a JSON crate for one field.
fn extract_image_ref(json: &str) -> Option<String> {
    let key = "\"image\"";
    let start = json.find(key)?;
    let rest = &json[start + key.len()..];
    let colon = rest.find(':')?;
    let after = &rest[colon + 1..];
    let q1 = after.find('"')?;
    let after = &after[q1 + 1..];
    let q2 = after.find('"')?;
    let candidate = &after[..q2];
    if candidate.contains('/') || candidate.contains(':') {
        Some(candidate.to_string())
    } else {
        None
    }
}

/// Render the fingerprint as a human-readable block.
pub fn render_fingerprint(fp: &Fingerprint, sel: &Selection) -> String {
    let mut s = String::new();
    s.push_str("Machine\n");
    s.push_str(&format!("  vendor        : {}\n", fp.sys_vendor));
    s.push_str(&format!("  product       : {}\n", fp.product_name));
    s.push_str(&format!("  family        : {}\n", fp.product_family));
    s.push_str(&format!("  version       : {}\n", fp.product_version));
    s.push_str(&format!(
        "  chassis       : {} ({})\n",
        fp.chassis_type,
        if fp.is_laptop() { "laptop" } else { "desktop/other" }
    ));
    s.push_str("CPU\n");
    s.push_str(&format!("  vendor        : {}\n", fp.cpu.vendor.as_str()));
    s.push_str(&format!("  model         : {}\n", fp.cpu.model_name));
    s.push_str(&format!(
        "  topology      : {} cores / {} threads{}\n",
        fp.cpu.physical_cores,
        fp.cpu.logical_threads,
        if fp.cpu.hybrid { " (P/E hybrid)" } else { "" }
    ));
    s.push_str(&format!(
        "  scaling driver: {}\n",
        fp.cpu.scaling_driver.as_deref().unwrap_or("(unknown)")
    ));
    s.push_str("GPU\n");
    if fp.gpus.is_empty() {
        s.push_str("  (none detected)\n");
    }
    for g in &fp.gpus {
        s.push_str(&format!(
            "  {} [{}] @ {}\n",
            g.vendor.as_str(),
            g.pci_id(),
            g.pci_slot
        ));
    }
    if fp.intel_nvidia_hybrid_gpu() {
        s.push_str("  (Intel + NVIDIA hybrid / Optimus)\n");
    }
    s.push_str("Power supply\n");
    s.push_str(&format!("  AC present    : {}\n", fp.has_ac));
    s.push_str(&format!(
        "  batteries     : {}\n",
        if fp.batteries.is_empty() {
            "(none)".to_string()
        } else {
            fp.batteries.join(", ")
        }
    ));
    s.push_str("Profile (layered selection)\n");
    s.push_str(&format!("  generic       : {}\n", sel.generic));
    s.push_str(&format!(
        "  class         : {}\n",
        if sel.class_or_empty().is_empty() {
            "(none)"
        } else {
            sel.class_or_empty()
        }
    ));
    s.push_str(&format!(
        "  device        : {}\n",
        if sel.device_or_empty().is_empty() {
            "(none)"
        } else {
            sel.device_or_empty()
        }
    ));
    s.push_str(&format!("  active        : {}\n", sel.active));
    s
}

/// Render the per-tier dry-run plan for a profile (what the daemon *would*
/// apply). No hardware is touched.
pub fn render_tier_plans(profile: &Profile) -> String {
    let mut s = String::new();
    s.push_str(&format!(
        "Dry-run tier plans for profile '{}' (no hardware touched):\n",
        profile.id
    ));
    for tier in Tier::ALL {
        s.push_str(&format!("  {} [{}]\n", tier.label(), tier.as_str()));
        let plan = profile.plan_tier(tier);
        if plan.is_empty() {
            s.push_str("    (no actions)\n");
        }
        for a in plan {
            s.push_str(&format!("    - {}\n", a.describe()));
        }
    }
    // Charge thresholds are resolved against the batteries this machine
    // actually has, so the dry-run view reports discovery too — including
    // "unsupported", which is the honest answer on most hardware.
    if let Some((start, stop)) = profile.charge_window() {
        s.push_str("  charge defaults\n");
        let inv = apexd_core::BatteryInventory::detect();
        let plan = inv.plan_thresholds(start, stop);
        if plan.is_empty() {
            s.push_str(&format!(
                "    - wants {start}-{stop}, but no battery here accepts thresholds ({})\n",
                inv.summary()
            ));
        }
        for a in plan {
            s.push_str(&format!("    - {}\n", a.describe()));
        }
    }
    s.push_str(&format!(
        "  auto-switch defaults: AC -> {}, battery -> {}\n",
        profile.defaults.ac.as_str(),
        profile.defaults.battery.as_str()
    ));
    s
}

/// Local (daemon-less) read-only view: fingerprint + selection + resolved
/// profile handle.
pub struct LocalView {
    pub fingerprint: Fingerprint,
    pub selection: Selection,
    pub set: ProfileSet,
}

impl LocalView {
    pub fn detect() -> LocalView {
        let fingerprint = Fingerprint::detect();
        let set = ProfileSet::load(Some(Path::new(apexd_core::PROFILE_DIR)))
            .unwrap_or_else(|_| ProfileSet::builtin());
        let selection = apexd_core::select(&fingerprint, &set);
        LocalView {
            fingerprint,
            selection,
            set,
        }
    }

    /// The resolved profile. Falls back to the generic layer (which
    /// `ProfileSet` always retains) rather than panicking, so a broken override
    /// directory downgrades the CLI's answer instead of aborting it.
    pub fn active_profile(&self) -> &Profile {
        self.set
            .get(&self.selection.active)
            .or_else(|| self.set.get(&self.selection.generic))
            .expect("profile set always retains a generic layer")
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────
// The root gate and the fwupd exit-code reading are both places where being
// subtly wrong is invisible: one would let a privileged verb through (or block
// an unprivileged one), the other would make a completely successful update
// report failure. Both are pure functions precisely so they can be pinned here.
#[cfg(test)]
mod tests {
    use super::*;

    const STATUS: &str = "Name:\tapex\nUmask:\t0022\nState:\tR (running)\n\
                          Uid:\t1000\t1000\t1000\t1000\nGid:\t1000\t1000\t1000\t1000\n";

    #[test]
    fn reads_the_effective_uid_not_the_real_one() {
        // real 1000, effective 0 — a setuid-style split. The effective id is
        // what the kernel enforces, so it is what we must read.
        let s = "Name:\tapex\nUid:\t1000\t0\t0\t1000\n";
        assert_eq!(parse_effective_uid(s), Some(0));
        assert_eq!(parse_effective_uid(STATUS), Some(1000));
    }

    #[test]
    fn missing_or_malformed_status_is_not_root() {
        assert_eq!(parse_effective_uid(""), None);
        assert_eq!(parse_effective_uid("Name:\tapex\n"), None);
        assert_eq!(parse_effective_uid("Uid:\n"), None);
        assert_eq!(parse_effective_uid("Uid:\t1000\n"), None);
        assert_eq!(parse_effective_uid("Uid:\tx\ty\n"), None);
        // Every one of these must fail CLOSED: require_root treats anything
        // other than Some(0) as "not root".
    }

    #[test]
    fn the_hint_echoes_the_whole_invocation() {
        let argv: Vec<String> = ["apex", "update", "--check", "--skip-firmware"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            sudo_reinvocation(&argv),
            "sudo apex update --check --skip-firmware"
        );
        // …including the no-argument case, where it must not gain a trailing space.
        assert_eq!(sudo_reinvocation(&["apex".to_string()]), "sudo apex");
    }

    #[test]
    fn the_refusal_names_the_verb_and_the_fix() {
        let argv: Vec<String> = ["apex", "update"].iter().map(|s| s.to_string()).collect();
        let msg = root_required_message("update", &argv);
        assert!(msg.contains("'update'"));
        assert!(msg.contains("sudo apex update"));
    }

    #[test]
    fn fwupd_nothing_to_do_is_success_not_failure() {
        // This is the regression that matters: 2 and 3 are the ORDINARY
        // outcomes on an up-to-date laptop. Reading them as failure would make
        // `apex update` exit non-zero on its most common path.
        assert!(fwupd_idle(FWUPD_NOTHING_TO_DO));
        assert!(fwupd_idle(FWUPD_NOT_FOUND));
        assert!(!fwupd_idle(0));
        assert!(!fwupd_idle(1));
    }
}
