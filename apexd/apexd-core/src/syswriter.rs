//! The one and only path from an [`Action`] to a real hardware effect.
//!
//! `apexd-core` never writes sysfs or spawns a process directly; it emits
//! [`Action`]s and hands them to a [`SysWriter`]. Production uses
//! [`RealWriter`] (which also honours dry-run); tests use [`MockWriter`], which
//! records intended actions and touches nothing. This is what lets every logic
//! path be unit-tested without writing real sysfs.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::Result;

use crate::tier::Action;

/// Turns intended [`Action`]s into effects.
pub trait SysWriter: Send + Sync {
    /// Apply one action.
    fn apply(&self, action: &Action) -> Result<()>;

    /// Apply a whole plan in order, stopping on the first hard error.
    fn apply_all(&self, actions: &[Action]) -> Result<()> {
        for a in actions {
            self.apply(a)?;
        }
        Ok(())
    }

    /// Whether this writer will actually mutate hardware. `false` for dry-run
    /// and for the mock.
    fn is_live(&self) -> bool {
        false
    }
}

/// Writes real sysfs and runs `nvidia-smi`. When `dry_run` is set, it logs the
/// intended effect and does nothing — the same switch `APEXD_DRY_RUN=1` flips.
///
/// Every write is capability-checked first: absent attributes are skipped, and
/// values the running kernel does not advertise are substituted from a ladder
/// of near-equivalents (see [`governor_ladder`], [`epp_ladder`],
/// [`platform_profile_ladder`]) rather than pushed at a driver that will refuse
/// them.
pub struct RealWriter {
    dry_run: bool,
    sys_root: PathBuf,
}

impl RealWriter {
    /// A writer rooted at real `/sys`.
    pub fn new(dry_run: bool) -> RealWriter {
        RealWriter {
            dry_run,
            sys_root: PathBuf::from("/sys"),
        }
    }

    /// A writer rooted at an explicit sysfs path (for a sandbox/fixture). Still
    /// gated by `dry_run`.
    pub fn with_root(dry_run: bool, sys_root: impl Into<PathBuf>) -> RealWriter {
        RealWriter {
            dry_run,
            sys_root: sys_root.into(),
        }
    }

    pub fn is_dry_run(&self) -> bool {
        self.dry_run
    }

    /// Write a value to an absolute path, treating both a missing attribute and
    /// a rejected write as a *skip* rather than an error. M6 touches knobs the
    /// kernel routinely refuses (kernel-managed IRQ affinity, cpuset attributes
    /// on a delegated cgroup); a refusal must never abort the rest of a plan —
    /// least of all a restore plan.
    ///
    /// Returns `true` when the value was actually written.
    fn write_tolerant(&self, path: &Path, value: &str, what: &str) -> bool {
        if !path.exists() {
            eprintln!("apexd: skip (absent) {} <- {value}", path.display());
            return false;
        }
        if self.dry_run {
            eprintln!("apexd: [dry-run] {what}: {} <- {value}", path.display());
            // Report success so callers that ladder down through fallbacks
            // (the fan restore) show what they *would* have done, not every
            // rung of a ladder no real write ever descended.
            return true;
        }
        match std::fs::write(path, value) {
            Ok(()) => true,
            Err(e) => {
                eprintln!("apexd: skip ({what} rejected) {} <- {value}: {e}", path.display());
                false
            }
        }
    }

    /// Like [`RealWriter::write_tolerant`] but without the existence check.
    ///
    /// cgroup-v2 attributes (`cpuset.cpus`, `cgroup.procs`, ...) are
    /// materialised by the kernel the moment the directory is created, so
    /// "absent" is not a meaningful state to test for there — and on a plain
    /// filesystem (a test fixture) the write simply creates the file, which is
    /// the behaviour the kernel presents anyway.
    fn write_forced(&self, path: &Path, value: &str, what: &str) -> bool {
        if self.dry_run {
            eprintln!("apexd: [dry-run] {what}: {} <- {value}", path.display());
            // Report success so callers that ladder down through fallbacks
            // (the fan restore) show what they *would* have done, not every
            // rung of a ladder no real write ever descended.
            return true;
        }
        match std::fs::write(path, value) {
            Ok(()) => true,
            Err(e) => {
                eprintln!("apexd: skip ({what} rejected) {} <- {value}: {e}", path.display());
                false
            }
        }
    }

    /// Write a value to a sysfs attribute if it exists. A missing attribute is
    /// not an error (the profile expresses full intent; hardware may not have
    /// every knob), and neither is a driver that rejects the write.
    ///
    /// A rejection used to be fatal, which made the whole tier plan abort
    /// part-applied on perfectly ordinary hardware — `intel_pstate` in active
    /// mode refuses an `energy_performance_preference` write while the
    /// `performance` governor is selected, for instance. Tolerating it is what
    /// lets one plan run everywhere.
    fn write_if_present(&self, path: &Path, value: &str) -> Result<()> {
        self.write_tolerant(path, value, "sysfs");
        Ok(())
    }

    /// Every cpufreq policy directory under the sysfs root.
    ///
    /// Prefers the per-policy directories (`cpufreq/policy*`), which every
    /// modern driver registers, and falls back to the per-CPU `cpuN/cpufreq`
    /// links that older kernels and some ARM `cpufreq-dt` setups present
    /// instead. A machine with no cpufreq at all (a VM with no scaling driver)
    /// simply gets an empty list and a logged skip.
    fn cpufreq_policies(&self) -> Vec<PathBuf> {
        let base = self.sys_root.join("devices/system/cpu/cpufreq");
        let mut out = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&base) {
            for e in entries.flatten() {
                let p = e.path();
                if p.file_name()
                    .and_then(|s| s.to_str())
                    .map(|s| s.starts_with("policy"))
                    .unwrap_or(false)
                {
                    out.push(p);
                }
            }
        }
        if out.is_empty() {
            for cpu in crate::topology::online_cpus(&self.sys_root) {
                let p = self
                    .sys_root
                    .join(format!("devices/system/cpu/cpu{cpu}/cpufreq"));
                if p.is_dir() {
                    out.push(p);
                }
            }
        }
        out.sort();
        out.dedup();
        out
    }

    /// Write a per-policy attribute, choosing the closest value the policy says
    /// it accepts.
    ///
    /// `choices_attr` names the sibling attribute that lists the legal values
    /// (`scaling_available_governors`,
    /// `energy_performance_available_preferences`). When it is absent the value
    /// is attempted as-is; when it is present the ladder is walked and the
    /// first advertised candidate wins. This is the whole reason a
    /// `performance`/`powersave` table works on `acpi-cpufreq`, `intel_pstate`,
    /// `amd-pstate` and ARM `cpufreq-dt` without per-driver special cases.
    fn write_policy_attr(&self, attr: &str, choices_attr: &str, value: &str, ladder: &[&str]) {
        let policies = self.cpufreq_policies();
        if policies.is_empty() {
            eprintln!("apexd: no cpufreq policies found; skip {attr} <- {value}");
            return;
        }
        for p in policies {
            let target = p.join(attr);
            if !target.exists() {
                eprintln!("apexd: skip (absent) {} <- {value}", target.display());
                continue;
            }
            let choices = read_tokens(&p.join(choices_attr));
            let chosen = match &choices {
                // No list published: the driver takes whatever it takes.
                None => Some(value.to_string()),
                Some(list) => pick_supported(value, ladder, list),
            };
            match chosen {
                Some(v) => {
                    if v != value {
                        eprintln!(
                            "apexd: {} does not offer '{value}'; using '{v}' instead",
                            target.display()
                        );
                    }
                    self.write_tolerant(&target, &v, attr);
                }
                None => eprintln!(
                    "apexd: skip ({attr} offers none of {value}/{}) {}",
                    ladder.join("/"),
                    target.display()
                ),
            }
        }
    }

    /// Write the ACPI platform profile, mapped onto what the firmware offers.
    /// `platform_profile_choices` is wildly vendor-specific — `low-power
    /// balanced performance` on one machine, `quiet balanced balanced-
    /// performance performance` on the next, `cool quiet performance` on an
    /// older ThinkPad — so the requested value is matched through a ladder of
    /// synonyms rather than written blind.
    fn write_platform_profile(&self, value: &str) -> Result<()> {
        let path = self.sys_root.join("firmware/acpi/platform_profile");
        if !path.exists() {
            eprintln!("apexd: skip (absent) {} <- {value}", path.display());
            return Ok(());
        }
        let choices = read_tokens(&self.sys_root.join("firmware/acpi/platform_profile_choices"));
        let chosen = match &choices {
            None => Some(value.to_string()),
            Some(list) => pick_supported(value, platform_profile_ladder(value), list),
        };
        match chosen {
            Some(v) => {
                if v != value {
                    eprintln!("apexd: platform_profile has no '{value}'; using '{v}' instead");
                }
                self.write_tolerant(&path, &v, "platform_profile");
            }
            None => eprintln!(
                "apexd: skip (platform_profile offers none of the '{value}' synonyms) {}",
                path.display()
            ),
        }
        Ok(())
    }

    /// Run `nvidia-smi` with `args`. A missing binary or a non-zero exit is a
    /// logged skip, never an error: a machine with no NVIDIA GPU must still be
    /// able to enter game mode.
    /// `scxctl <args>`, best-effort.
    ///
    /// Deliberately never fatal, and for the same reason as nvidia-smi: a
    /// scheduler swap is a performance nicety, and a machine without sched-ext
    /// support, without scxctl, or whose scheduler refuses to load must still
    /// enter game mode with its cpuset, IRQ and clock work applied. Failing the
    /// whole plan because a scheduler would not attach would be strictly worse
    /// than running on the kernel's own scheduler.
    fn run_scxctl(&self, args: &[String]) -> Result<()> {
        if self.dry_run {
            eprintln!("apexd: [dry-run] scxctl {}", args.join(" "));
            return Ok(());
        }
        // sched_ext has to exist in the kernel. On a kernel without it scxctl
        // would fail confusingly, so say the useful thing instead.
        if !Path::new("/sys/kernel/sched_ext").exists() {
            eprintln!(
                "apexd: skip (kernel has no sched_ext support) scxctl {}",
                args.join(" ")
            );
            return Ok(());
        }
        // scx-tools installs into /usr/sbin, which is not always on PATH for a
        // service; try both rather than depending on the unit's environment.
        let bin = ["/usr/sbin/scxctl", "/usr/bin/scxctl"]
            .into_iter()
            .find(|p| Path::new(p).exists());
        let Some(bin) = bin else {
            eprintln!("apexd: skip (scxctl absent) scxctl {}", args.join(" "));
            return Ok(());
        };
        match std::process::Command::new(bin).args(args).output() {
            Ok(out) if out.status.success() => Ok(()),
            Ok(out) => {
                eprintln!(
                    "apexd: scxctl {} failed ({}): {}",
                    args.join(" "),
                    out.status,
                    String::from_utf8_lossy(&out.stderr).trim()
                );
                Ok(())
            }
            Err(e) => {
                eprintln!("apexd: cannot run scxctl {}: {e}", args.join(" "));
                Ok(())
            }
        }
    }

    fn run_nvidia_smi(&self, args: &[String]) -> Result<()> {
        if self.dry_run {
            eprintln!("apexd: [dry-run] nvidia-smi {}", args.join(" "));
            return Ok(());
        }
        if !crate::gpu::nvidia_smi_available() {
            eprintln!("apexd: skip (nvidia-smi absent) nvidia-smi {}", args.join(" "));
            return Ok(());
        }
        match std::process::Command::new("nvidia-smi").args(args).output() {
            Ok(out) if out.status.success() => Ok(()),
            Ok(out) => {
                eprintln!(
                    "apexd: nvidia-smi {} failed ({}): {}",
                    args.join(" "),
                    out.status,
                    String::from_utf8_lossy(&out.stderr).trim()
                );
                Ok(())
            }
            Err(e) => {
                eprintln!("apexd: nvidia-smi {} could not run: {e}", args.join(" "));
                Ok(())
            }
        }
    }

    /// Hand a fan back to firmware control. The ladder is the safety guarantee:
    /// prior `pwm*_enable` -> `2` (firmware automatic) -> `0` (no control, which
    /// the hwmon ABI defines as *full speed*), and if a manual mode is all the
    /// hardware offers, the duty cycle is driven to 255 rather than left low.
    /// No path through this function can leave a fan stopped.
    fn fan_safe_restore(
        &self,
        enable_path: Option<&str>,
        pwm_path: Option<&str>,
        prior_enable: Option<u8>,
        prior_pwm: Option<u8>,
    ) -> Result<()> {
        let Some(enable) = enable_path else {
            // No enable attribute: the only lever is the duty cycle. Restore the
            // recorded value, or go to full speed if we never recorded one.
            if let Some(pwm) = pwm_path {
                let v = prior_pwm.unwrap_or(255);
                self.write_tolerant(Path::new(pwm), &v.to_string(), "fan restore pwm");
            }
            return Ok(());
        };
        let enable = Path::new(enable);

        // 1. The value the fan had before we touched it (usually 2 = firmware).
        if let Some(prior) = prior_enable {
            if self.write_tolerant(enable, &prior.to_string(), "fan restore enable") {
                // Manual mode was the *prior* state; put its duty cycle back too,
                // and never below full speed if we do not know what it was.
                if prior == 1 {
                    if let Some(pwm) = pwm_path {
                        let v = prior_pwm.unwrap_or(255);
                        self.write_tolerant(Path::new(pwm), &v.to_string(), "fan restore pwm");
                    }
                }
                return Ok(());
            }
        }
        // 2. Firmware automatic.
        if self.write_tolerant(enable, "2", "fan restore auto") {
            return Ok(());
        }
        // 3. Last resort: full speed. Push the duty cycle up *first* so that a
        //    driver treating `0` as "manual, keep current pwm" still ends up
        //    with the fan spinning flat out.
        if let Some(pwm) = pwm_path {
            self.write_tolerant(Path::new(pwm), "255", "fan restore full-speed pwm");
        }
        self.write_tolerant(enable, "0", "fan restore full-speed");
        Ok(())
    }

    /// Create a cgroup-v2 directory (if needed) and apply a cpuset to it.
    /// Enabling the `cpuset` controller in the parent's `subtree_control` is
    /// best-effort: on a systemd host the root cgroup may already delegate it.
    fn cgroup_ensure(&self, path: &str, cpus: &str, mems: &str) -> Result<()> {
        let dir = Path::new(path);
        if self.dry_run {
            eprintln!("apexd: [dry-run] cgroup {path}: cpuset.cpus={cpus} cpuset.mems={mems}");
            return Ok(());
        }
        if let Some(parent) = dir.parent() {
            let sc = parent.join("cgroup.subtree_control");
            if sc.exists() {
                // Only meaningful if cpuset is not already enabled; a duplicate
                // write is harmless and a rejection is tolerated.
                self.write_tolerant(&sc, "+cpuset", "cgroup subtree_control");
            }
        }
        if !dir.exists() {
            if let Err(e) = std::fs::create_dir_all(dir) {
                eprintln!("apexd: cgroup {path}: create failed: {e}");
                return Ok(());
            }
        }
        self.write_forced(&dir.join("cpuset.mems"), mems, "cpuset.mems");
        self.write_forced(&dir.join("cpuset.cpus"), cpus, "cpuset.cpus");
        Ok(())
    }

    /// Remove a cgroup directory. `rmdir` is all the kernel needs (its
    /// auto-populated attribute files do not block it); the `remove_dir_all`
    /// fallback exists for plain filesystems — test fixtures — and is only
    /// attempted when the directory holds no sub-directories, so a cgroup with
    /// children is never blown away.
    fn cgroup_remove(&self, path: &str) -> Result<()> {
        let dir = Path::new(path);
        if self.dry_run {
            eprintln!("apexd: [dry-run] cgroup {path}: remove");
            return Ok(());
        }
        if !dir.exists() {
            return Ok(());
        }
        match std::fs::remove_dir(dir) {
            Ok(()) => return Ok(()),
            Err(e) => {
                let has_subdirs = std::fs::read_dir(dir)
                    .map(|it| it.flatten().any(|e| e.path().is_dir()))
                    .unwrap_or(true);
                if has_subdirs {
                    eprintln!("apexd: cgroup {path}: remove skipped ({e}); it has child cgroups");
                    return Ok(());
                }
                if let Err(e2) = std::fs::remove_dir_all(dir) {
                    eprintln!("apexd: cgroup {path}: remove skipped: {e} / {e2}");
                }
            }
        }
        Ok(())
    }
}

impl SysWriter for RealWriter {
    fn apply(&self, action: &Action) -> Result<()> {
        match action {
            Action::Governor(g) => {
                self.write_policy_attr(
                    "scaling_governor",
                    "scaling_available_governors",
                    g,
                    governor_ladder(g),
                );
                Ok(())
            }
            Action::Epp(e) => {
                self.write_policy_attr(
                    "energy_performance_preference",
                    "energy_performance_available_preferences",
                    e,
                    epp_ladder(e),
                );
                Ok(())
            }
            Action::PlatformProfile(p) => self.write_platform_profile(p),
            Action::ChargeThresholds {
                start,
                stop,
                start_path,
                end_path,
            } => {
                // Stop threshold last: some ECs reject a start >= stop, and
                // writing stop first widens the window before narrowing.
                if let Some(end_path) = end_path {
                    self.write_if_present(Path::new(end_path), &stop.to_string())?;
                }
                if let Some(start_path) = start_path {
                    self.write_if_present(Path::new(start_path), &start.to_string())?;
                }
                Ok(())
            }

            // ── M6 ───────────────────────────────────────────────────────────
            Action::FanPwmEnable { path, value } => {
                self.write_tolerant(Path::new(path), &value.to_string(), "pwm_enable");
                Ok(())
            }
            Action::FanPwm { path, value } => {
                self.write_tolerant(Path::new(path), &value.to_string(), "pwm");
                Ok(())
            }
            Action::FanVendorAttr { path, value, what } => {
                self.write_tolerant(Path::new(path), value, what);
                Ok(())
            }
            Action::FanSafeRestore {
                enable_path,
                pwm_path,
                prior_enable,
                prior_pwm,
            } => self.fan_safe_restore(
                enable_path.as_deref(),
                pwm_path.as_deref(),
                *prior_enable,
                *prior_pwm,
            ),
            Action::NvidiaPersistence { gpu, enabled } => self.run_nvidia_smi(&[
                "-i".into(),
                gpu.to_string(),
                "-pm".into(),
                u8::from(*enabled).to_string(),
            ]),
            Action::NvidiaLockGraphics {
                gpu,
                min_mhz,
                max_mhz,
            } => self.run_nvidia_smi(&[
                "-i".into(),
                gpu.to_string(),
                "-lgc".into(),
                format!("{min_mhz},{max_mhz}"),
            ]),
            Action::NvidiaLockMemory {
                gpu,
                min_mhz,
                max_mhz,
            } => self.run_nvidia_smi(&[
                "-i".into(),
                gpu.to_string(),
                "-lmc".into(),
                format!("{min_mhz},{max_mhz}"),
            ]),
            Action::NvidiaResetGraphics { gpu } => {
                self.run_nvidia_smi(&["-i".into(), gpu.to_string(), "-rgc".into()])
            }
            Action::NvidiaResetMemory { gpu } => {
                self.run_nvidia_smi(&["-i".into(), gpu.to_string(), "-rmc".into()])
            }
            Action::IrqAffinity { path, cpus } => {
                self.write_tolerant(Path::new(path), cpus, "irq affinity");
                Ok(())
            }
            Action::CgroupEnsure { path, cpus, mems } => self.cgroup_ensure(path, cpus, mems),
            Action::CgroupAttach { path, pid } => {
                self.write_forced(
                    &Path::new(path).join("cgroup.procs"),
                    &pid.to_string(),
                    "cgroup attach",
                );
                Ok(())
            }
            Action::CgroupRemove { path } => self.cgroup_remove(path),
            Action::ScxSwitch { sched } => self.run_scxctl(&["switch".into(), "-s".into(), sched.clone()]),
            Action::ScxStop => self.run_scxctl(&["stop".into()]),
        }
    }

    fn is_live(&self) -> bool {
        !self.dry_run
    }
}

// ── capability probing: what does this kernel actually accept? ───────────────

/// Read a whitespace-separated sysfs list (`scaling_available_governors` and
/// friends). `None` when the attribute does not exist — which means "the driver
/// publishes no list", not "the list is empty".
fn read_tokens(path: &Path) -> Option<Vec<String>> {
    let text = std::fs::read_to_string(path).ok()?;
    Some(
        text.split_whitespace()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect(),
    )
}

/// The first of `value` then `ladder` that appears in `available`.
fn pick_supported(value: &str, ladder: &[&str], available: &[String]) -> Option<String> {
    let has = |c: &str| available.iter().any(|a| a.eq_ignore_ascii_case(c));
    if has(value) {
        return Some(value.to_string());
    }
    ladder
        .iter()
        .find(|c| has(c))
        .map(|c| (*c).to_string())
}

/// Fallbacks for a `scaling_governor` value.
///
/// `performance` and `powersave` are near-universal, but they are not
/// guaranteed: a kernel can be built without `CPU_FREQ_GOV_POWERSAVE`, and some
/// ARM defconfigs ship only `schedutil` plus `performance`. Substituting the
/// nearest governor in the same direction beats writing `EINVAL` at the driver.
fn governor_ladder(value: &str) -> &'static [&'static str] {
    match value.to_ascii_lowercase().as_str() {
        "performance" => &["performance", "schedutil", "ondemand"],
        "powersave" => &["powersave", "schedutil", "conservative", "ondemand"],
        "schedutil" => &["schedutil", "ondemand", "powersave"],
        "ondemand" => &["ondemand", "schedutil", "conservative"],
        "conservative" => &["conservative", "ondemand", "schedutil", "powersave"],
        _ => &["schedutil", "ondemand", "powersave"],
    }
}

/// Fallbacks for an `energy_performance_preference` value.
///
/// The four canonical strings (`performance`, `balance_performance`,
/// `balance_power`, `power`) are what `intel_pstate` and `amd-pstate` publish,
/// but a driver in a different operating mode may offer only a subset, and
/// `default` is always a safe landing spot.
fn epp_ladder(value: &str) -> &'static [&'static str] {
    match value.to_ascii_lowercase().as_str() {
        "performance" => &["performance", "balance_performance", "default"],
        "balance_performance" => &["balance_performance", "performance", "default"],
        "balance_power" => &["balance_power", "balance_performance", "default"],
        "power" => &["power", "balance_power", "default"],
        _ => &["default", "balance_performance"],
    }
}

/// Synonyms for an ACPI `platform_profile` value, ordered by how close they are
/// to the intent. The vocabulary differs per vendor: `low-power` on one
/// machine, `quiet` or `cool` on another, and `balanced-performance` sits
/// between `balanced` and `performance` on newer firmware.
fn platform_profile_ladder(value: &str) -> &'static [&'static str] {
    match value.to_ascii_lowercase().as_str() {
        "performance" => &["performance", "balanced-performance", "balanced"],
        "balanced-performance" => &["balanced-performance", "performance", "balanced"],
        "balanced" => &["balanced", "balanced-performance", "quiet", "performance"],
        "low-power" => &["low-power", "quiet", "cool", "balanced"],
        "quiet" => &["quiet", "low-power", "cool", "balanced"],
        "cool" => &["cool", "quiet", "low-power", "balanced"],
        _ => &["balanced"],
    }
}

/// Records intended actions without touching anything. The backbone of the
/// unit tests.
#[derive(Default)]
pub struct MockWriter {
    actions: Mutex<Vec<Action>>,
}

impl MockWriter {
    pub fn new() -> MockWriter {
        MockWriter::default()
    }

    /// A snapshot of every action applied so far, in order.
    pub fn recorded(&self) -> Vec<Action> {
        self.actions.lock().unwrap().clone()
    }

    /// Clear the record.
    pub fn clear(&self) {
        self.actions.lock().unwrap().clear();
    }
}

impl SysWriter for MockWriter {
    fn apply(&self, action: &Action) -> Result<()> {
        self.actions.lock().unwrap().push(action.clone());
        Ok(())
    }
}
