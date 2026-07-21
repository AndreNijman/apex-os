//! Non-D-Bus operations: shelling out to bootc/ostree/fwupd for update,
//! rollback, pin and changelog, plus local read-only rendering used both as a
//! daemon-less fallback and by `apex fingerprint`/`doctor`.

use std::path::Path;
use std::process::Command;

use apexd_core::tier::Tier;
use apexd_core::{Fingerprint, Profile, ProfileSet, Selection};

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

/// `apex update` -> pull a newer OS image, then refresh firmware via fwupd.
pub fn update() -> i32 {
    let mut worst = 0;
    match run("bootc", &["upgrade"]) {
        Ok(code) => worst = worst.max(code),
        Err(e) => {
            eprintln!("apex: OS update failed: {e}");
            worst = 1;
        }
    }
    // Firmware: refresh metadata then apply. fwupd is best-effort (a machine
    // may have no LVFS-covered devices).
    match run("fwupdmgr", &["refresh", "--force"]) {
        Ok(_) => {}
        Err(e) => eprintln!("apex: fwupd refresh skipped: {e}"),
    }
    match run("fwupdmgr", &["update", "-y"]) {
        Ok(code) => worst = worst.max(code),
        Err(e) => eprintln!("apex: fwupd update skipped: {e}"),
    }
    worst
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
    if let Some(charge) = profile.charge_action() {
        s.push_str(&format!("  charge defaults\n    - {}\n", charge.describe()));
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

    pub fn active_profile(&self) -> &Profile {
        self.set
            .get(&self.selection.active)
            .expect("active profile always present")
    }
}
