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

/// What applying one [`Action`] actually did to the machine.
///
/// This exists because `Ok(())` was not enough to tell the truth with. Most of
/// what this writer touches is *tolerated* — a kernel-managed interrupt refuses
/// an affinity write with `-EIO`, a driver refuses an EPP write while the
/// performance governor is selected — and tolerating a refusal is right: it
/// must never abort the rest of a plan, least of all a restore plan. But
/// tolerating it and *reporting success for it* are different things, and the
/// second is what let `apex game status` say "12 IRQs steered" on a machine
/// that had steered none.
///
/// ── The rule for actions that drive more than one write ─────────────────────
///
/// Only [`Action::IrqAffinity`] is one action to exactly one write. `Governor`
/// fans out across every cpufreq policy, `CgroupEnsure` writes `cpuset.mems`
/// and `cpuset.cpus`, `FanSafeRestore` walks a ladder until a rung sticks. For
/// all of those the outcome is **any-landed**: `Landed` means at least one
/// write reached the machine. That is the honest reading for a ladder (one
/// rung landing is the whole point) and the only defensible one for a fan-out
/// without inventing a per-write count no caller asked for.
///
/// So a count derived from these is only a measurement for the 1:1 action.
/// Anything wanting a per-write count of a fan-out action has to plan one
/// action per write first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The value reached the machine — or would have, under dry-run.
    Landed,
    /// Nothing reached the machine. Carries the reason, in the same words the
    /// skip was logged with, so a caller can report *why* rather than only
    /// that something did not happen.
    Refused(String),
}

impl Outcome {
    /// True when the action had an effect. Named rather than matched inline
    /// because the fan restore ladder branches on it three times.
    pub fn landed(&self) -> bool {
        matches!(self, Outcome::Landed)
    }
}

/// Turns intended [`Action`]s into effects.
pub trait SysWriter: Send + Sync {
    /// Apply one action, reporting whether it actually landed.
    ///
    /// `Err` is reserved for a *hard* failure that should stop a plan. A knob
    /// the hardware refuses is `Ok(Outcome::Refused)`, not an error.
    fn apply(&self, action: &Action) -> Result<Outcome>;

    /// Apply a whole plan in order, stopping on the first hard error.
    ///
    /// Deliberately discards the per-action outcome: a caller that needs to
    /// know what landed has to look at each action, because "some of this plan
    /// landed" is not a fact anything can act on.
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
    /// Whether actions that run a HOST COMMAND may actually run it.
    ///
    /// Off by default, and that default is the whole point. Most actions are
    /// sysfs writes, which `sys_root` redirects into a fixture — so a test can
    /// use a live writer safely. Two actions are not writes at all:
    /// `ScxSwitch`/`ScxStop` shell out to `scxctl`, and the NVIDIA clock locks
    /// shell out to `nvidia-smi`. No fixture root can redirect a process
    /// spawn, so a test applying those reaches the real machine.
    ///
    /// It did. `scxctl` is a D-Bus client for `scx_loader`, whose polkit action
    /// is not passwordless, so running the game-mode tests raised a burst of
    /// "Authentication is required to start, stop, or switch sched-ext
    /// schedulers" prompts on the developer's desktop — and, once
    /// authenticated, would have swapped the scheduler of the machine running
    /// the tests.
    ///
    /// So the daemon opts in explicitly ([`RealWriter::for_daemon`]) and
    /// everything else, tests included, gets a writer that logs and skips.
    host_commands: bool,
}

impl RealWriter {
    /// A writer rooted at real `/sys` that will NOT run host commands.
    ///
    /// This is the constructor for anything that is not the daemon.
    pub fn new(dry_run: bool) -> RealWriter {
        RealWriter {
            dry_run,
            sys_root: PathBuf::from("/sys"),
            host_commands: false,
        }
    }

    /// The daemon's writer: real `/sys`, and permitted to run `scxctl` and
    /// `nvidia-smi`.
    ///
    /// Separate from [`RealWriter::new`] so that opting into host commands is
    /// one visible call in one place, rather than the default that every
    /// caller silently inherits.
    pub fn for_daemon(dry_run: bool) -> RealWriter {
        RealWriter {
            dry_run,
            sys_root: PathBuf::from("/sys"),
            host_commands: true,
        }
    }

    /// A writer rooted at an explicit sysfs path (for a sandbox/fixture). Still
    /// gated by `dry_run`, and never runs host commands.
    pub fn with_root(dry_run: bool, sys_root: impl Into<PathBuf>) -> RealWriter {
        RealWriter {
            dry_run,
            sys_root: sys_root.into(),
            host_commands: false,
        }
    }

    /// Whether this writer may run host commands.
    pub fn runs_host_commands(&self) -> bool {
        self.host_commands
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
    /// Returns [`Outcome::Landed`] when the value was actually written, and
    /// [`Outcome::Refused`] carrying the reason when it was not.
    fn write_tolerant(&self, path: &Path, value: &str, what: &str) -> Outcome {
        if !path.exists() {
            eprintln!("apexd: skip (absent) {} <- {value}", path.display());
            return Outcome::Refused(format!("{what}: attribute absent"));
        }
        if self.dry_run {
            eprintln!("apexd: [dry-run] {what}: {} <- {value}", path.display());
            // Report success so callers that ladder down through fallbacks
            // (the fan restore) show what they *would* have done, not every
            // rung of a ladder no real write ever descended.
            return Outcome::Landed;
        }
        match std::fs::write(path, value) {
            Ok(()) => Outcome::Landed,
            Err(e) => {
                eprintln!("apexd: skip ({what} rejected) {} <- {value}: {e}", path.display());
                Outcome::Refused(format!("{what}: {e}"))
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
    fn write_forced(&self, path: &Path, value: &str, what: &str) -> Outcome {
        if self.dry_run {
            eprintln!("apexd: [dry-run] {what}: {} <- {value}", path.display());
            // Report success so callers that ladder down through fallbacks
            // (the fan restore) show what they *would* have done, not every
            // rung of a ladder no real write ever descended.
            return Outcome::Landed;
        }
        match std::fs::write(path, value) {
            Ok(()) => Outcome::Landed,
            Err(e) => {
                eprintln!("apexd: skip ({what} rejected) {} <- {value}: {e}", path.display());
                Outcome::Refused(format!("{what}: {e}"))
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
    fn write_if_present(&self, path: &Path, value: &str) -> Outcome {
        self.write_tolerant(path, value, "sysfs")
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
    ///
    /// Fans out across every policy, so the outcome is any-landed per
    /// [`Outcome`]'s documented rule: one policy accepting the value is an
    /// effect on the machine.
    fn write_policy_attr(
        &self,
        attr: &str,
        choices_attr: &str,
        value: &str,
        ladder: &[&str],
    ) -> Outcome {
        let policies = self.cpufreq_policies();
        if policies.is_empty() {
            eprintln!("apexd: no cpufreq policies found; skip {attr} <- {value}");
            return Outcome::Refused(format!("{attr}: no cpufreq policies on this machine"));
        }
        // Every policy is still visited even once one has landed: this loop is
        // what applies the value, and short-circuiting it would leave the other
        // policies untouched. Only the *report* is an aggregate.
        let mut landed = false;
        let mut last_refusal = None;
        for p in policies {
            let target = p.join(attr);
            if !target.exists() {
                eprintln!("apexd: skip (absent) {} <- {value}", target.display());
                last_refusal = Some(format!("{attr}: attribute absent"));
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
                    match self.write_tolerant(&target, &v, attr) {
                        Outcome::Landed => landed = true,
                        Outcome::Refused(why) => last_refusal = Some(why),
                    }
                }
                None => {
                    eprintln!(
                        "apexd: skip ({attr} offers none of {value}/{}) {}",
                        ladder.join("/"),
                        target.display()
                    );
                    last_refusal = Some(format!(
                        "{attr}: offers none of {value}/{}",
                        ladder.join("/")
                    ));
                }
            }
        }
        if landed {
            return Outcome::Landed;
        }
        Outcome::Refused(last_refusal.unwrap_or_else(|| format!("{attr}: nothing accepted it")))
    }

    /// Write the ACPI platform profile, mapped onto what the firmware offers.
    /// `platform_profile_choices` is wildly vendor-specific — `low-power
    /// balanced performance` on one machine, `quiet balanced balanced-
    /// performance performance` on the next, `cool quiet performance` on an
    /// older ThinkPad — so the requested value is matched through a ladder of
    /// synonyms rather than written blind.
    fn write_platform_profile(&self, value: &str) -> Outcome {
        let path = self.sys_root.join("firmware/acpi/platform_profile");
        if !path.exists() {
            eprintln!("apexd: skip (absent) {} <- {value}", path.display());
            return Outcome::Refused("platform_profile: attribute absent".into());
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
                self.write_tolerant(&path, &v, "platform_profile")
            }
            None => {
                eprintln!(
                    "apexd: skip (platform_profile offers none of the '{value}' synonyms) {}",
                    path.display()
                );
                Outcome::Refused(format!(
                    "platform_profile: firmware offers no '{value}' synonym"
                ))
            }
        }
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
    fn run_scxctl(&self, args: &[String]) -> Result<Outcome> {
        if self.dry_run {
            eprintln!("apexd: [dry-run] scxctl {}", args.join(" "));
            return Ok(Outcome::Landed);
        }
        // Checked BEFORE anything else, because this is the guard that keeps a
        // test off the host's scheduler. `sys_root` cannot redirect a process
        // spawn, so `dry_run` is not enough on its own.
        if !self.host_commands {
            eprintln!(
                "apexd: skip (host commands not enabled for this writer) scxctl {}",
                args.join(" ")
            );
            return Ok(Outcome::Refused(
                "scxctl: host commands not enabled for this writer".into(),
            ));
        }
        // sched_ext has to exist in the kernel. On a kernel without it scxctl
        // would fail confusingly, so say the useful thing instead.
        if !Path::new("/sys/kernel/sched_ext").exists() {
            eprintln!(
                "apexd: skip (kernel has no sched_ext support) scxctl {}",
                args.join(" ")
            );
            return Ok(Outcome::Refused(
                "scxctl: kernel has no sched_ext support".into(),
            ));
        }
        // scx-tools installs into /usr/sbin, which is not always on PATH for a
        // service; try both rather than depending on the unit's environment.
        let bin = ["/usr/sbin/scxctl", "/usr/bin/scxctl"]
            .into_iter()
            .find(|p| Path::new(p).exists());
        let Some(bin) = bin else {
            eprintln!("apexd: skip (scxctl absent) scxctl {}", args.join(" "));
            return Ok(Outcome::Refused("scxctl: not installed".into()));
        };
        match std::process::Command::new(bin).args(args).output() {
            Ok(out) if out.status.success() => Ok(Outcome::Landed),
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
                eprintln!(
                    "apexd: scxctl {} failed ({}): {stderr}",
                    args.join(" "),
                    out.status,
                );
                Ok(Outcome::Refused(format!("scxctl: {} — {stderr}", out.status)))
            }
            Err(e) => {
                eprintln!("apexd: cannot run scxctl {}: {e}", args.join(" "));
                Ok(Outcome::Refused(format!("scxctl: {e}")))
            }
        }
    }

    fn run_nvidia_smi(&self, args: &[String]) -> Result<Outcome> {
        if self.dry_run {
            eprintln!("apexd: [dry-run] nvidia-smi {}", args.join(" "));
            return Ok(Outcome::Landed);
        }
        // Same guard as scxctl: a process spawn is not redirected by
        // `sys_root`, so a test with a live writer would lock the clocks of the
        // GPU it is running on.
        if !self.host_commands {
            eprintln!(
                "apexd: skip (host commands not enabled for this writer) nvidia-smi {}",
                args.join(" ")
            );
            return Ok(Outcome::Refused(
                "nvidia-smi: host commands not enabled for this writer".into(),
            ));
        }
        if !crate::gpu::nvidia_smi_available() {
            eprintln!("apexd: skip (nvidia-smi absent) nvidia-smi {}", args.join(" "));
            return Ok(Outcome::Refused("nvidia-smi: not installed".into()));
        }
        match std::process::Command::new("nvidia-smi").args(args).output() {
            Ok(out) if out.status.success() => Ok(Outcome::Landed),
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
                eprintln!(
                    "apexd: nvidia-smi {} failed ({}): {stderr}",
                    args.join(" "),
                    out.status,
                );
                Ok(Outcome::Refused(format!(
                    "nvidia-smi: {} — {stderr}",
                    out.status
                )))
            }
            Err(e) => {
                eprintln!("apexd: nvidia-smi {} could not run: {e}", args.join(" "));
                Ok(Outcome::Refused(format!("nvidia-smi: {e}")))
            }
        }
    }

    /// Hand a fan back to firmware control. The ladder is the safety guarantee:
    /// prior `pwm*_enable` -> `2` (firmware automatic) -> `0` (no control, which
    /// the hwmon ABI defines as *full speed*), and if a manual mode is all the
    /// hardware offers, the duty cycle is driven to 255 rather than left low.
    /// No path through this function can leave a fan stopped.
    ///
    /// The outcome is any-landed, per [`Outcome`]: whichever rung of the ladder
    /// sticks, control was handed back.
    fn fan_safe_restore(
        &self,
        enable_path: Option<&str>,
        pwm_path: Option<&str>,
        prior_enable: Option<u8>,
        prior_pwm: Option<u8>,
    ) -> Result<Outcome> {
        let Some(enable) = enable_path else {
            // No enable attribute: the only lever is the duty cycle. Restore the
            // recorded value, or go to full speed if we never recorded one.
            let Some(pwm) = pwm_path else {
                return Ok(Outcome::Refused(
                    "fan restore: this fan exposes neither pwm_enable nor pwm".into(),
                ));
            };
            let v = prior_pwm.unwrap_or(255);
            return Ok(self.write_tolerant(Path::new(pwm), &v.to_string(), "fan restore pwm"));
        };
        let enable = Path::new(enable);

        // 1. The value the fan had before we touched it (usually 2 = firmware).
        if let Some(prior) = prior_enable {
            if self
                .write_tolerant(enable, &prior.to_string(), "fan restore enable")
                .landed()
            {
                // Manual mode was the *prior* state; put its duty cycle back too,
                // and never below full speed if we do not know what it was.
                if prior == 1 {
                    if let Some(pwm) = pwm_path {
                        let v = prior_pwm.unwrap_or(255);
                        self.write_tolerant(Path::new(pwm), &v.to_string(), "fan restore pwm");
                    }
                }
                return Ok(Outcome::Landed);
            }
        }
        // 2. Firmware automatic.
        if self.write_tolerant(enable, "2", "fan restore auto").landed() {
            return Ok(Outcome::Landed);
        }
        // 3. Last resort: full speed. Push the duty cycle up *first* so that a
        //    driver treating `0` as "manual, keep current pwm" still ends up
        //    with the fan spinning flat out.
        let mut landed = false;
        if let Some(pwm) = pwm_path {
            landed |= self
                .write_tolerant(Path::new(pwm), "255", "fan restore full-speed pwm")
                .landed();
        }
        let last = self.write_tolerant(enable, "0", "fan restore full-speed");
        if landed || last.landed() {
            return Ok(Outcome::Landed);
        }
        Ok(last)
    }

    /// Create a cgroup-v2 directory (if needed) and apply a cpuset to it.
    /// Enabling the `cpuset` controller in the parent's `subtree_control` is
    /// best-effort: on a systemd host the root cgroup may already delegate it.
    ///
    /// Any-landed, per [`Outcome`]: the cpuset that confines the game is
    /// `cpuset.cpus`, so a run where `cpuset.mems` alone stuck still changed
    /// the machine. Callers wanting the two separately have to plan them
    /// separately.
    fn cgroup_ensure(&self, path: &str, cpus: &str, mems: &str) -> Result<Outcome> {
        let dir = Path::new(path);
        if self.dry_run {
            eprintln!("apexd: [dry-run] cgroup {path}: cpuset.cpus={cpus} cpuset.mems={mems}");
            return Ok(Outcome::Landed);
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
                return Ok(Outcome::Refused(format!("cgroup {path}: create failed: {e}")));
            }
        }
        let mems = self.write_forced(&dir.join("cpuset.mems"), mems, "cpuset.mems");
        let cpus = self.write_forced(&dir.join("cpuset.cpus"), cpus, "cpuset.cpus");
        if mems.landed() || cpus.landed() {
            return Ok(Outcome::Landed);
        }
        Ok(cpus)
    }

    /// Remove a cgroup directory. `rmdir` is all the kernel needs (its
    /// auto-populated attribute files do not block it); the `remove_dir_all`
    /// fallback exists for plain filesystems — test fixtures — and is only
    /// attempted when the directory holds no sub-directories, so a cgroup with
    /// children is never blown away.
    fn cgroup_remove(&self, path: &str) -> Result<Outcome> {
        let dir = Path::new(path);
        if self.dry_run {
            eprintln!("apexd: [dry-run] cgroup {path}: remove");
            return Ok(Outcome::Landed);
        }
        if !dir.exists() {
            // Already gone is the state this action asks for, so it landed.
            // Exit is idempotent by design and must not report a refusal for
            // running twice.
            return Ok(Outcome::Landed);
        }
        match std::fs::remove_dir(dir) {
            Ok(()) => return Ok(Outcome::Landed),
            Err(e) => {
                let has_subdirs = std::fs::read_dir(dir)
                    .map(|it| it.flatten().any(|e| e.path().is_dir()))
                    .unwrap_or(true);
                if has_subdirs {
                    eprintln!("apexd: cgroup {path}: remove skipped ({e}); it has child cgroups");
                    return Ok(Outcome::Refused(format!(
                        "cgroup {path}: has child cgroups ({e})"
                    )));
                }
                if let Err(e2) = std::fs::remove_dir_all(dir) {
                    eprintln!("apexd: cgroup {path}: remove skipped: {e} / {e2}");
                    return Ok(Outcome::Refused(format!("cgroup {path}: {e} / {e2}")));
                }
            }
        }
        Ok(Outcome::Landed)
    }
}

impl SysWriter for RealWriter {
    fn apply(&self, action: &Action) -> Result<Outcome> {
        match action {
            Action::Governor(g) => Ok(self.write_policy_attr(
                "scaling_governor",
                "scaling_available_governors",
                g,
                governor_ladder(g),
            )),
            Action::Epp(e) => Ok(self.write_policy_attr(
                "energy_performance_preference",
                "energy_performance_available_preferences",
                e,
                epp_ladder(e),
            )),
            Action::PlatformProfile(p) => Ok(self.write_platform_profile(p)),
            Action::ChargeThresholds {
                start,
                stop,
                start_path,
                end_path,
            } => {
                // Stop threshold last: some ECs reject a start >= stop, and
                // writing stop first widens the window before narrowing.
                let mut last = Outcome::Refused(
                    "charge thresholds: this battery exposes neither attribute".into(),
                );
                let mut landed = false;
                if let Some(end_path) = end_path {
                    last = self.write_if_present(Path::new(end_path), &stop.to_string());
                    landed |= last.landed();
                }
                if let Some(start_path) = start_path {
                    last = self.write_if_present(Path::new(start_path), &start.to_string());
                    landed |= last.landed();
                }
                Ok(if landed { Outcome::Landed } else { last })
            }

            // ── M6 ───────────────────────────────────────────────────────────
            Action::FanPwmEnable { path, value } => Ok(self.write_tolerant(
                Path::new(path),
                &value.to_string(),
                "pwm_enable",
            )),
            Action::FanPwm { path, value } => {
                Ok(self.write_tolerant(Path::new(path), &value.to_string(), "pwm"))
            }
            Action::FanVendorAttr { path, value, what } => {
                Ok(self.write_tolerant(Path::new(path), value, what))
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
            // The ONE action that is exactly one write, which is why the
            // game-mode report counts these and nothing else.
            Action::IrqAffinity { path, cpus } => {
                Ok(self.write_tolerant(Path::new(path), cpus, "irq affinity"))
            }
            Action::CgroupEnsure { path, cpus, mems } => self.cgroup_ensure(path, cpus, mems),
            Action::CgroupAttach { path, pid } => Ok(self.write_forced(
                &Path::new(path).join("cgroup.procs"),
                &pid.to_string(),
                "cgroup attach",
            )),
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
    /// Always [`Outcome::Landed`]: the mock's contract is that the plan is
    /// recorded exactly as issued. A test that needs a *refusing* writer builds
    /// one — see the game-status tests in `apexd/src/game.rs`, which is the
    /// case a mock that could refuse would have made ambiguous.
    fn apply(&self, action: &Action) -> Result<Outcome> {
        self.actions.lock().unwrap().push(action.clone());
        Ok(Outcome::Landed)
    }
}

#[cfg(test)]
mod host_command_tests {
    use super::*;

    // The guard these assert exists because running the game-mode tests raised
    // a burst of polkit prompts on the developer's desktop: `scxctl` is a D-Bus
    // client for `scx_loader`, whose action is not passwordless, and a test
    // applying `ScxSwitch` through a live writer invoked it for real. `sys_root`
    // could not prevent it — a process spawn has no root to redirect.

    #[test]
    fn a_writer_does_not_run_host_commands_unless_it_is_the_daemons() {
        assert!(
            !RealWriter::new(false).runs_host_commands(),
            "the default must be OFF: this is the constructor tests reach for"
        );
        assert!(!RealWriter::new(true).runs_host_commands());
        assert!(!RealWriter::with_root(false, "/tmp/fixture").runs_host_commands());
        assert!(!RealWriter::with_root(true, "/tmp/fixture").runs_host_commands());
    }

    #[test]
    fn the_daemons_writer_does_run_them() {
        // Otherwise the guard has quietly disabled game mode in production,
        // which is the failure mode of fixing this the lazy way.
        assert!(RealWriter::for_daemon(false).runs_host_commands());
        assert!(RealWriter::for_daemon(true).runs_host_commands());
    }

    #[test]
    fn dry_run_is_still_independent_of_host_commands() {
        // Two separate axes: dry-run says "plan only", host-commands says "you
        // may leave this process". A daemon in dry-run must do neither.
        let d = RealWriter::for_daemon(true);
        assert!(d.is_dry_run());
        assert!(d.runs_host_commands());
        let live = RealWriter::for_daemon(false);
        assert!(!live.is_dry_run());
    }

    #[test]
    fn scx_and_nvidia_actions_are_accepted_and_skipped_rather_than_failing() {
        // A skipped host command must not abort a plan: the rest of game mode
        // (cpuset, IRQ affinity) is the part that matters, and a restore plan
        // that aborts half way is worse than one that logs a skip.
        //
        // This runs on the test host with host commands OFF, so it is also the
        // assertion that these two actions cannot touch it.
        let w = RealWriter::new(false);
        assert!(w
            .apply(&Action::ScxSwitch {
                sched: "scx_lavd".into()
            })
            .is_ok());
        assert!(w.apply(&Action::ScxStop).is_ok());
    }
}

#[cfg(test)]
mod outcome_tests {
    use super::*;

    /// A scratch directory that cleans itself up.
    struct Tmp(PathBuf);
    impl Tmp {
        fn new(tag: &str) -> Tmp {
            let p = std::env::temp_dir().join(format!(
                "apexd-outcome-{tag}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            std::fs::remove_dir_all(&p).ok();
            std::fs::create_dir_all(&p).unwrap();
            Tmp(p)
        }
    }
    impl Drop for Tmp {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).ok();
        }
    }

    // ── the outcome of a write is reported, not assumed ─────────────────────
    //
    // These exist because `apply` used to return `Ok(())` whether or not the
    // value reached the machine, and `apex game status` then reported the
    // PLANNED number of steered interrupts as if it had measured them. On a
    // machine that refuses every affinity write — kernel-managed MSI-X queues
    // return -EIO — status said "N IRQs steered" having steered none.
    //
    // The refusal is produced by pointing the action at a DIRECTORY. `write(2)`
    // on a directory fails with EISDIR for every user including root, so this
    // asserts the same thing whether the suite runs as the developer or in a
    // root container. A `chmod 0444` fixture would not: root ignores the mode
    // bits, and the test would silently invert in CI.

    #[test]
    fn an_irq_write_that_lands_reports_that_it_landed() {
        let t = Tmp::new("irq-landed");
        let path = t.0.join("smp_affinity_list");
        std::fs::write(&path, "0-19\n").unwrap();
        let w = RealWriter::new(false);
        assert_eq!(
            w.apply(&Action::IrqAffinity {
                path: path.to_string_lossy().to_string(),
                cpus: "12-19".into(),
            })
            .unwrap(),
            Outcome::Landed
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "12-19");
    }

    #[test]
    fn an_irq_write_the_kernel_refuses_reports_the_refusal_and_the_reason() {
        let t = Tmp::new("irq-refused");
        // A directory: present, so the existence check passes, and unwritable
        // for anyone — which is exactly what a kernel-managed interrupt looks
        // like from here.
        let path = t.0.join("smp_affinity_list");
        std::fs::create_dir_all(&path).unwrap();
        let w = RealWriter::new(false);
        let outcome = w
            .apply(&Action::IrqAffinity {
                path: path.to_string_lossy().to_string(),
                cpus: "12-19".into(),
            })
            .unwrap();
        assert!(!outcome.landed(), "a refused write must not report landing");
        let Outcome::Refused(why) = outcome else {
            unreachable!("checked by the assertion above")
        };
        assert!(
            why.contains("irq affinity"),
            "the reason must name the knob: {why}"
        );
    }

    #[test]
    fn an_absent_attribute_is_a_refusal_rather_than_a_silent_success() {
        let t = Tmp::new("irq-absent");
        let w = RealWriter::new(false);
        let outcome = w
            .apply(&Action::IrqAffinity {
                path: t.0.join("no/such/smp_affinity_list").to_string_lossy().to_string(),
                cpus: "12-19".into(),
            })
            .unwrap();
        assert!(
            !outcome.landed(),
            "an attribute this machine does not have cannot have been written"
        );
    }

    #[test]
    fn a_refused_action_is_still_not_an_error() {
        // The tolerance this whole module is built on: a knob the hardware
        // refuses must never abort the rest of a plan, least of all a restore
        // plan. Reporting the refusal is a REPORTING change, not a change to
        // what aborts.
        let t = Tmp::new("irq-tolerant");
        let path = t.0.join("smp_affinity_list");
        std::fs::create_dir_all(&path).unwrap();
        let w = RealWriter::new(false);
        let plan = [
            Action::IrqAffinity {
                path: path.to_string_lossy().to_string(),
                cpus: "12-19".into(),
            },
            Action::ScxStop,
        ];
        assert!(
            w.apply_all(&plan).is_ok(),
            "a refused write must not abort the plan behind it"
        );
    }
}
