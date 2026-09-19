//! Whether this machine can actually do §12's controller-first boot-to-game.
//!
//! ── What already exists, and what this adds ─────────────────────────────────
//!
//! §12's Desktop-Mode/Gaming-Mode split is **already built**, and this module
//! deliberately does not rebuild it:
//!
//! * `files/desktop/wayland-sessions/apex-gaming.desktop` is a session the
//!   greeter offers, exactly as SteamOS offers its two.
//! * `files/system/libexec/apex-gaming-session` is that session: gamescope
//!   presenting straight to KMS with Steam Big Picture (`-gamepadui`) inside it,
//!   `apex game start` around it, and a fail-safe that bounces back to the
//!   greeter rather than to a black screen.
//! * `files/system/libexec/apex-session-select` + its NOPASSWD sudoers rule are
//!   how the desktop switches into Gaming Mode and back.
//!
//! What is missing is that **nothing can answer the question before you reboot
//! into it.** The session's own preflight is a `FATAL` at start-up: if
//! `gamescope` or `steam` is absent it exits non-zero and greetd re-displays the
//! greeter. That is the right behaviour and a bad moment to learn it — the user
//! finds out at the worst possible time, from a log they usually cannot see.
//! (That message used to guess at the cause, "this is not the Gaming edition?";
//! with one image the cause is never the edition, so it now names the command
//! that fixes it. Same command this module's [`Readiness::install_hint`]
//! produces — but printed after the reboot rather than before it, which is the
//! whole reason this module exists.)
//!
//! ── One image changed what these signals MEAN ───────────────────────────────
//!
//! There are no editions any more: `apex-gaming.desktop` and
//! `apex-gaming-session` ship on every machine, and `gamescope`/`steam` install
//! on demand (a kernel module cannot be installed at runtime under Secure Boot,
//! userspace can — so the GPU and controller drivers are baked in and the
//! gaming userspace is not). The session entry is `TryExec`-gated so the greeter
//! hides it until gamescope exists. The consequence for this module is that the
//! file signals no longer answer "which edition is this" — they answer "is this
//! image current" — while the program signals are the only ones a user can act
//! on. See [`Readiness::blockers`].
//!
//! So this is a **read-only readiness report**: every requirement the session
//! and the mode switch depend on, measured with the path it was measured at, or
//! unavailable with a reason. It writes nothing, spawns nothing, and never
//! touches the session it is describing.
//!
//! ── The two switches ────────────────────────────────────────────────────────
//!
//! [`Probe`] takes a `root` **and** a separate `probe_programs` flag, for the
//! same reason [`crate::syswriter::RealWriter`] needs `sys_root` and
//! `host_commands` separately, and `apex/src/blueprint.rs`'s `Host` needs `root`
//! and `probe_programs`: a fixture root redirects a file read and **cannot**
//! redirect a `PATH` lookup. A probe that read the session file from a fixture
//! while asking the host's `PATH` whether `steam` exists would produce an
//! assertion that passes or fails depending on whose machine ran it.
//!
//! ── The gamepad check is sysfs, not /dev ────────────────────────────────────
//!
//! Controllers are detected through `/sys/class/input/input*/capabilities/key`,
//! not by listing `/dev/input/event*`. `/dev` is not under a sysfs root, so a
//! `/dev` probe is unfixturable and the assertion would collapse into "does the
//! developer have a controller plugged in right now". Reading the kernel's own
//! capability bitmap also answers the right question: not "is there an input
//! device" but "does an input device report gamepad buttons".

use std::path::{Path, PathBuf};

use crate::workload::Signal;

/// `BTN_SOUTH`, aka `BTN_GAMEPAD` — `0x130` in `include/uapi/linux/input-event-codes.h`.
///
/// The kernel defines `BTN_GAMEPAD` as an alias for `BTN_SOUTH` precisely so
/// that "has this key" is the test for "is this a gamepad". Every controller
/// with a face button reports it; keyboards and mice do not.
pub const BTN_GAMEPAD: u32 = 0x130;

/// The session id the Gaming editions install. Kept in step with
/// [`crate::blueprint::GAMING_SESSION`] by a unit test rather than by a comment.
pub const GAMING_SESSION: &str = "apex-gaming";

/// Paths the readiness report is built from. Relative to the probe's root, so a
/// fixture tree can mirror the shape of a real machine.
const SESSION_DESKTOP: &str = "usr/share/wayland-sessions/apex-gaming.desktop";
const SESSION_LAUNCHER: &str = "usr/libexec/apex-gaming-session";
const SWITCH_HELPER: &str = "usr/libexec/apex-session-select";
const SWITCH_SUDOERS: &str = "etc/sudoers.d/040-apex-session-select";
const RTPRIO_LIMITS: &str = "etc/security/limits.d/30-apex-gaming-rtprio.conf";
const GREETER_LAST_SESSION: &str = "var/lib/apex-greet/last-session";
const INPUT_CLASS: &str = "sys/class/input";
const PROC_SELF_STATUS: &str = "proc/self/status";
const SYS_ROOT: &str = "sys";

/// `CAP_SYS_NICE` — bit 23 of the capability bitmaps in
/// `/proc/self/status`, from `include/uapi/linux/capability.h`.
pub const CAP_SYS_NICE: u32 = 23;

/// The exact words that mark an `Unavailable` reason as "root could answer
/// this, and this account could not".
///
/// A string marker rather than a second field on [`Signal`](crate::workload::Signal)
/// because `Signal` is shared with `apex workload` and `apex perf`, and this is
/// a property of ONE probe's paths rather than of the type. It is a constant so
/// the renderer, the footer and the tests all match on the same bytes — a
/// hand-typed copy in any of the three is how this kind of marker rots.
pub const ROOT_READABLE_HINT: &str = "run `sudo apex gaming` to measure it";

/// Reason text for a path this account is not allowed to look at.
///
/// ── Why this exists at all ──────────────────────────────────────────────────
///
/// Measured on katana, 2026-09-19 (§6 of the qualification): the report said
///
/// ```text
/// sudoers rule    : not measured — could not read the path: Permission denied (os error 13)
/// ```
///
/// which is honest and useless. `/etc/sudoers.d` is `0750 root:root`, so the
/// kernel returns `EACCES` for a traversal whether or not the file is there —
/// the probe genuinely cannot distinguish missing from unreadable, and neither
/// can any amount of cleverness short of being root. What it CAN do is say
/// which of the two answers it failed to get and name the one command that
/// gets it, which `sudo apex gaming` did, returning `yes`.
///
/// "Permission denied is not absence" is a named lesson in this program, found
/// in about fourteen places. This is the half of it that is easy to miss: the
/// measurement was already correct here, and the REPORT was the thing that
/// left the reader with nothing to do.
fn eacces_reason(kind: &str) -> String {
    format!("{kind} is not readable by this account; {ROOT_READABLE_HINT}")
}

/// Reads (never writes) everything §12's boot-to-game path depends on.
pub struct Probe {
    root: PathBuf,
    probe_programs: bool,
}

impl Default for Probe {
    fn default() -> Probe {
        Probe::new()
    }
}

impl Probe {
    /// The real machine: `/`, and permitted to look programs up on `PATH`.
    pub fn new() -> Probe {
        Probe {
            root: PathBuf::from("/"),
            probe_programs: true,
        }
    }

    /// A fixture tree. Program probing is **off**, because no root can redirect
    /// a `PATH` lookup.
    pub fn with_root(root: impl Into<PathBuf>) -> Probe {
        Probe {
            root: root.into(),
            probe_programs: false,
        }
    }

    /// Whether this probe may look programs up on `PATH`.
    pub fn probes_programs(&self) -> bool {
        self.probe_programs
    }

    fn at(&self, rel: &str) -> PathBuf {
        self.root.join(rel)
    }

    /// Measure everything.
    pub fn report(&self) -> Readiness {
        Readiness {
            session_desktop: self.file(SESSION_DESKTOP),
            session_launcher: self.executable(SESSION_LAUNCHER),
            switch_helper: self.executable(SWITCH_HELPER),
            switch_sudoers: self.file(SWITCH_SUDOERS),
            rtprio_limits: self.file(RTPRIO_LIMITS),
            preselected_session: self.preselected_session(),
            gamepad: self.gamepad(),
            gamescope: self.program("gamescope"),
            steam: self.program("steam"),
            mangoapp: self.program("mangoapp"),
            cap_sys_nice: self.cap_sys_nice(),
            display: crate::gpu::choose_display(&self.at(SYS_ROOT)),
        }
    }

    /// Whether THIS process holds `CAP_SYS_NICE` in its effective set.
    ///
    /// ── Why the capability and not the rlimit ───────────────────────────────
    ///
    /// Measured on katana, 2026-09-19 (§6.2): the rtprio drop-in did its job —
    /// the soft `RLIMIT_RTPRIO` was 20 — the session script correctly saw that
    /// and passed `--rt`, and gamescope answered
    ///
    /// ```text
    /// No CAP_SYS_NICE, falling back to regular-priority compute and threads.
    /// ```
    ///
    /// on all three attempts. gamescope gates its realtime path on the
    /// CAPABILITY, not on the rlimit, so `realtime limit: yes` was a row that
    /// was true about the wrong thing. Both are reported now.
    ///
    /// ── Whose capability this is ────────────────────────────────────────────
    ///
    /// The set read here belongs to `apex`, not to the future Gaming Mode
    /// session — but both are started from the same PAM stack on the same
    /// machine, so an empty effective set here means an empty one there. It is
    /// a prediction, and the row says so rather than claiming to have measured
    /// the session.
    fn cap_sys_nice(&self) -> Signal<bool> {
        let p = self.at(PROC_SELF_STATUS);
        let text = match std::fs::read_to_string(&p) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                return Signal::unavailable(eacces_reason("the capability set"), p.display().to_string())
            }
            Err(e) => {
                return Signal::unavailable(
                    format!("cannot read the capability set: {e}"),
                    p.display().to_string(),
                )
            }
        };
        match cap_bit(&text, "CapEff", CAP_SYS_NICE) {
            Some(v) => Signal::measured(v, p.display().to_string()),
            None => Signal::unavailable(
                "no CapEff line in the process status file",
                p.display().to_string(),
            ),
        }
    }

    /// The effective uid of this process, read from the same file as the
    /// capability set so a fixture root can answer it too.
    pub fn effective_uid(&self) -> Option<u32> {
        let text = std::fs::read_to_string(self.at(PROC_SELF_STATUS)).ok()?;
        for line in text.lines() {
            if let Some(rest) = line.strip_prefix("Uid:") {
                // real, effective, saved, filesystem
                return rest.split_whitespace().nth(1)?.parse().ok();
            }
        }
        None
    }

    /// Present or absent, with the path looked at. Absence is a *measurement*,
    /// so it is `Measured(false)` rather than `Unavailable` — the latter is
    /// reserved for "could not tell", which is a different and worse answer.
    ///
    /// `Path::exists` cannot make that distinction: it answers false for every
    /// failed `stat`, including `EACCES`. `SWITCH_SUDOERS` lives in
    /// `/etc/sudoers.d`, which is `0750 root:root`, and `apex gaming` is
    /// documented as read-only and needs no root — so an unprivileged run
    /// could not stat the file and reported the correctly installed sudoers
    /// rule as missing, every time. Match on the error instead.
    fn file(&self, rel: &str) -> Signal<bool> {
        let p = self.at(rel);
        match std::fs::metadata(&p) {
            Ok(_) => Signal::measured(true, p.display().to_string()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Signal::measured(false, p.display().to_string())
            }
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                Signal::unavailable(eacces_reason("the path"), p.display().to_string())
            }
            Err(e) => Signal::unavailable(
                format!("could not read the path: {e}"),
                p.display().to_string(),
            ),
        }
    }

    /// Present, and executable. A COPY that lost `--chmod=0755` produces a
    /// session that exists and cannot start, which is the failure this
    /// distinguishes from a missing file.
    ///
    /// Carries the same `EACCES` distinction as `file`: a mode we cannot read
    /// is not a mode without the executable bit.
    fn executable(&self, rel: &str) -> Signal<bool> {
        use std::os::unix::fs::PermissionsExt;
        let p = self.at(rel);
        match std::fs::metadata(&p) {
            Ok(m) => Signal::measured(
                m.is_file() && m.permissions().mode() & 0o111 != 0,
                p.display().to_string(),
            ),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Signal::measured(false, p.display().to_string())
            }
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                Signal::unavailable(eacces_reason("the path"), p.display().to_string())
            }
            Err(e) => Signal::unavailable(
                format!("could not read the path: {e}"),
                p.display().to_string(),
            ),
        }
    }

    /// Which session the greeter will preselect at the next boot.
    ///
    /// Written by `apex-session-select` and rewritten by greetd after every
    /// login, so it answers "is this machine currently set to boot into Gaming
    /// Mode" — the actual state of the Desktop/Gaming split.
    fn preselected_session(&self) -> Signal<String> {
        let p = self.at(GREETER_LAST_SESSION);
        match std::fs::read_to_string(&p) {
            Ok(s) if !s.trim().is_empty() => {
                Signal::measured(s.trim().to_string(), p.display().to_string())
            }
            Ok(_) => Signal::unavailable(
                "the greeter's record is empty; it has not chosen a session yet",
                p.display().to_string(),
            ),
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => Signal::unavailable(
                eacces_reason("the greeter's record"),
                p.display().to_string(),
            ),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Signal::unavailable(
                "the greeter has never recorded a session on this machine",
                p.display().to_string(),
            ),
            Err(e) => Signal::unavailable(
                format!("cannot read the greeter's record: {e}"),
                p.display().to_string(),
            ),
        }
    }

    /// Every input device reporting `BTN_GAMEPAD`, by name.
    ///
    /// An empty list is a measurement, not a gap: "no controller is attached" is
    /// a true and useful answer. `Unavailable` is only for a machine with no
    /// `/sys/class/input` at all, which is a container rather than a desktop.
    fn gamepad(&self) -> Signal<Vec<String>> {
        let dir = self.at(INPUT_CLASS);
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                return Signal::unavailable(
                    eacces_reason("the input device directory"),
                    dir.display().to_string(),
                )
            }
            Err(e) => {
                return Signal::unavailable(
                    format!("cannot enumerate input devices: {e}"),
                    dir.display().to_string(),
                )
            }
        };
        let mut found: Vec<String> = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            let caps = path.join("capabilities/key");
            let bitmap = match std::fs::read_to_string(&caps) {
                Ok(b) => b,
                // Most entries under /sys/class/input are leaf nodes
                // (eventN, mouseN, jsN) with no capabilities file of their
                // own — that absence is the ordinary shape of the listing,
                // not a refusal, and skipping it is what makes the loop find
                // the `inputN` entries that do carry one.
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                // Anything else is a refused read on a device this loop
                // could not check. Silently skipping it would let a
                // permission problem masquerade as "not a gamepad", and the
                // whole answer would then certify an empty list as complete
                // when it is not.
                Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                    return Signal::unavailable(
                        eacces_reason(&format!("{}", caps.display())),
                        dir.display().to_string(),
                    );
                }
                Err(e) => {
                    return Signal::unavailable(
                        format!("{}: {e}", caps.display()),
                        dir.display().to_string(),
                    );
                }
            };
            if !has_key_bit(&bitmap, BTN_GAMEPAD) {
                continue;
            }
            let name = std::fs::read_to_string(path.join("name"))
                .ok()
                .map(|n| n.trim().to_string())
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| {
                    entry
                        .file_name()
                        .to_string_lossy()
                        .to_string()
                });
            found.push(name);
        }
        found.sort();
        found.dedup();
        Signal::measured(found, dir.display().to_string())
    }

    /// Whether a program is on `PATH` — or `Unavailable` when this probe is not
    /// permitted to look, which is the whole point of the second switch.
    fn program(&self, name: &str) -> Signal<bool> {
        if !self.probe_programs {
            return Signal::unavailable(
                "program probing is off for a fixture root, because no root can redirect a \
                 PATH lookup and the host's own programs would answer instead",
                format!("PATH:{name}"),
            );
        }
        Signal::measured(on_path(name), format!("PATH:{name}"))
    }
}

/// The measured state of §12's boot-to-game path.
pub struct Readiness {
    pub session_desktop: Signal<bool>,
    pub session_launcher: Signal<bool>,
    pub switch_helper: Signal<bool>,
    pub switch_sudoers: Signal<bool>,
    pub rtprio_limits: Signal<bool>,
    pub preselected_session: Signal<String>,
    pub gamepad: Signal<Vec<String>>,
    pub gamescope: Signal<bool>,
    pub steam: Signal<bool>,
    pub mangoapp: Signal<bool>,
    /// Whether this login holds `CAP_SYS_NICE` — what gamescope's `--rt`
    /// actually gates on. See [`Probe::cap_sys_nice`].
    pub cap_sys_nice: Signal<bool>,
    /// The screen and GPU Gaming Mode will use. See
    /// [`crate::gpu::choose_display`].
    pub display: crate::gpu::DisplayChoice,
}

impl Readiness {
    /// Whether Gaming Mode would actually start.
    ///
    /// The hard requirements are exactly the ones `apex-gaming-session` checks
    /// before it does anything, plus the two files without which the greeter
    /// cannot offer the session at all. Deliberately the same list, so this
    /// report cannot say "ready" about a session that would then FATAL.
    ///
    /// ── WHAT EACH BLOCKER MEANS NOW THAT APEX PUBLISHES ONE IMAGE ──
    ///
    /// The two groups have different remedies, and conflating them is how a user
    /// gets told to install a package that cannot help.
    ///
    /// * `session_desktop` / `session_launcher` are IMAGE content and ship on
    ///   every APEX machine. They used to discriminate editions — a missing
    ///   entry meant "you installed Daily" — and that reading is now wrong.
    ///   They are not dead code, though: absent, they mean this machine is
    ///   still booting an image from BEFORE the editions merged (the common
    ///   case: a laptop on the old `:daily`, which really did not ship them),
    ///   or the deployment is damaged. Either way the remedy is `apex update`,
    ///   never a package install.
    /// * `gamescope` / `steam` are the on-demand half of the payload split and
    ///   are the only blockers a user can fix themselves, with `apex install`.
    ///
    /// So the rule for anything offering an install hint: offer it only when
    /// every blocker present is one of the installable two. An image-content
    /// blocker means no package fixes this.
    pub fn blockers(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.session_desktop.value() == Some(&false) {
            out.push(
                "the greeter has no Gaming Mode entry, which every current APEX image ships \
                 — this machine is booting an image from before the editions merged, or a \
                 damaged one. Run `apex update`; no package install fixes this"
                    .to_string(),
            );
        }
        if self.session_launcher.value() == Some(&false) {
            out.push(
                "the gamescope session script is missing or not executable, so picking Gaming \
                 Mode at the greeter would bounce straight back. It is image content, so the \
                 remedy is `apex update`, not a package install"
                    .to_string(),
            );
        }
        // `gamescope` and `steam` are the session's own two hard requirements,
        // and since the editions merged they are also the only two blockers a
        // user can clear themselves — they install on demand rather than
        // shipping in the image, because userspace can be added at runtime and
        // a signed kernel module cannot.
        //
        // An *unmeasured* program is not a blocker: saying "not ready" because
        // a fixture root switched program probing off would be a false negative.
        if self.gamescope.value() == Some(&false) {
            out.push("gamescope is not installed; the session exits FATAL without it".to_string());
        }
        if self.steam.value() == Some(&false) {
            out.push("steam is not installed; the session exits FATAL without it".to_string());
        }
        out
    }

    /// Whether every blocker present can be cleared by installing packages.
    ///
    /// The distinction the prose above describes, as a predicate: image-content
    /// blockers need `apex update`, program blockers need `apex install`, and
    /// telling a user to install gamescope on a machine whose image has no
    /// gaming session at all is advice that cannot work. `false` when there are
    /// no blockers, because there is then nothing to remedy.
    pub fn blockers_are_installable(&self) -> bool {
        let image_content_missing = self.session_desktop.value() == Some(&false)
            || self.session_launcher.value() == Some(&false);
        let program_missing =
            self.gamescope.value() == Some(&false) || self.steam.value() == Some(&false);
        program_missing && !image_content_missing
    }

    /// The single command that clears every blocker, or `None` when no install
    /// would clear them all.
    ///
    /// Gated on [`Self::blockers_are_installable`] rather than on "is some
    /// program missing", which is the whole point: on a machine whose image
    /// predates the merge, gamescope and steam are *also* missing, so a hint
    /// derived from the programs alone would print `sudo apex install gamescope
    /// steam` to someone whose actual remedy is `apex update`. They would run
    /// it, the packages would install, and Gaming Mode would still not start.
    ///
    /// Kept out of the individual [`Self::blockers`] strings on purpose: those
    /// are word-wrapped to the terminal, which splits a command across lines
    /// and makes it unpastable. One unwrapped line that installs everything
    /// missing in a single engine run beats a remedy per blocker.
    pub fn install_hint(&self) -> Option<String> {
        if !self.blockers_are_installable() {
            return None;
        }
        let mut pkgs = Vec::new();
        if self.gamescope.value() == Some(&false) {
            pkgs.push("gamescope");
        }
        if self.steam.value() == Some(&false) {
            pkgs.push("steam");
        }
        if pkgs.is_empty() {
            return None;
        }
        Some(format!("sudo apex install {}", pkgs.join(" ")))
    }

    /// Requirements that are met but degraded, with what is lost. Not blockers:
    /// Gaming Mode starts without every one of these.
    pub fn warnings(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.switch_helper.value() == Some(&false)
            || self.switch_sudoers.value() == Some(&false)
        {
            out.push(
                "the Desktop<->Gaming switch helper or its sudoers rule is absent, so the \
                 desktop's power menu cannot change which session boots; the greeter can \
                 still be used to pick one by hand"
                    .to_string(),
            );
        }
        if self.rtprio_limits.value() == Some(&false) {
            out.push(
                "the realtime scheduling limit drop-in is absent, so no login on this \
                 machine can raise a thread to SCHED_RR at all"
                    .to_string(),
            );
        }
        // Measured on katana 2026-09-19 (§6.2): the drop-in was present, the
        // soft limit was 20, `--rt` was passed, and gamescope still ran at
        // normal priority because it gates on CAP_SYS_NICE rather than on the
        // rlimit. So this is a SEPARATE warning from the one above and not an
        // `else` of it: a machine can have the limit and not the capability,
        // which is every APEX machine today.
        if self.cap_sys_nice.value() == Some(&false) {
            out.push(
                "this login has no CAP_SYS_NICE, which is what gamescope's --rt actually \
                 needs, so Gaming Mode runs the compositor at ordinary priority and says so \
                 in its log rather than passing a flag that would be dropped. /usr is \
                 read-only on APEX, so `setcap` cannot grant it on the gamescope binary; \
                 granting it at login would also put capabilities in Steam's bwrap, which \
                 refuses to start with any"
                    .to_string(),
            );
        }
        if let Some(problem) = &self.display.problem {
            out.push(format!(
                "Gaming Mode may open the wrong GPU: {problem}. Override with \
                 APEX_GAMESCOPE_ARGS in the session environment"
            ));
        }
        if self.mangoapp.value() == Some(&false) {
            out.push(
                "mangoapp is not installed, so the in-game overlay is unavailable — which is \
                 also why `apex perf` cannot report frame time"
                    .to_string(),
            );
        }
        if let Some(pads) = self.gamepad.value() {
            if pads.is_empty() {
                out.push(
                    "no input device reports gamepad buttons, so nothing verifies the \
                     controller-first path on this machine right now; Big Picture is still \
                     usable with a keyboard"
                        .to_string(),
                );
            }
        }
        out
    }

    /// True when nothing blocks Gaming Mode from starting.
    pub fn is_ready(&self) -> bool {
        self.blockers().is_empty()
    }

    /// The rows whose answer is "this account may not look", in report order.
    ///
    /// Every one of them would be answered by the same single command, so the
    /// report names them together and prints that command once rather than
    /// repeating a remedy per row.
    pub fn rows_needing_root(&self) -> Vec<&'static str> {
        let needs = |s: &Signal<bool>| {
            s.reason()
                .map(|r| r.contains(ROOT_READABLE_HINT))
                .unwrap_or(false)
        };
        let mut out = Vec::new();
        if needs(&self.session_desktop) {
            out.push("greeter entry");
        }
        if needs(&self.session_launcher) {
            out.push("session script");
        }
        if needs(&self.rtprio_limits) {
            out.push("realtime limit");
        }
        if needs(&self.cap_sys_nice) {
            out.push("realtime capability");
        }
        if needs(&self.switch_helper) {
            out.push("switch helper");
        }
        if needs(&self.switch_sudoers) {
            out.push("sudoers rule");
        }
        if self
            .preselected_session
            .reason()
            .map(|r| r.contains(ROOT_READABLE_HINT))
            .unwrap_or(false)
        {
            out.push("boots to game");
        }
        if self
            .gamepad
            .reason()
            .map(|r| r.contains(ROOT_READABLE_HINT))
            .unwrap_or(false)
        {
            out.push("gamepads");
        }
        out
    }

    /// Whether the machine is currently set to boot into Gaming Mode.
    pub fn boots_to_game(&self) -> Option<bool> {
        self.preselected_session
            .value()
            .map(|s| s == GAMING_SESSION)
    }
}

/// One bit of one of `/proc/<pid>/status`'s capability bitmaps.
///
/// The kernel prints `CapInh`, `CapPrm`, `CapEff`, `CapBnd` and `CapAmb` as a
/// single 16-digit lowercase hex number — `cap_bset_fmt` in `fs/proc/array.c`
/// — most significant nibble first, so this is plain `u64` arithmetic and not
/// the word-reversed format `/sys/class/input/*/capabilities/key` uses. They
/// are two different kernel printers and conflating them reads the wrong bit.
///
/// `None` when the named line is absent or unparseable, which is "could not
/// tell" and must not be rendered as "the capability is missing".
pub fn cap_bit(status: &str, field: &str, bit: u32) -> Option<bool> {
    if bit >= 64 {
        return None;
    }
    for line in status.lines() {
        let Some(rest) = line.strip_prefix(field) else {
            continue;
        };
        let Some(rest) = rest.strip_prefix(':') else {
            continue;
        };
        let hex = rest.trim();
        if hex.is_empty() {
            return None;
        }
        let value = u64::from_str_radix(hex.trim_start_matches("0x"), 16).ok()?;
        return Some((value >> bit) & 1 == 1);
    }
    None
}

// ── the capability bitmap ────────────────────────────────────────────────────

/// Whether an input device's `capabilities/key` bitmap has a given key bit set.
///
/// ## The format, from the kernel that writes it
///
/// `input_print_bitmap()` walks the bitmap from its **highest** word down to
/// word 0, printing each as bare hex separated by spaces — and it **omits
/// leading zero words entirely** (`skip_empty` stays true until the first
/// non-zero word). So the word count is not fixed by `KEY_MAX`; it depends on
/// where the device's highest set bit lands.
///
/// That is why this indexes **from the right**. Word 0 is always the last word
/// printed, so `words[len - 1 - n]` is word `n` regardless of how many high
/// words the kernel elided. Indexing from the left would silently read the
/// wrong word for any device with a lower top bit than the one in the fixture.
///
/// ## The word width is the READER's, not the kernel's
///
/// `input_bits_to_string()` prints one `unsigned long` per word — *except*
/// under `in_compat_syscall()`, where it splits each long into two 32-bit
/// halves. The deciding ABI is therefore the one belonging to the process doing
/// the read, which for a natively compiled `apex` is [`usize::BITS`]. Guessing
/// it from the printed string's length is what an earlier draft did, and it is
/// wrong in a way a fixture hides: a 64-bit kernel describing a device whose
/// top word is small prints short words, and the guess then reads word 9 when
/// the bit lives in word 4.
///
/// [`has_key_bit_with_width`] takes the width explicitly so both ABIs are
/// reachable from a test on either kind of machine.
pub fn has_key_bit(bitmap: &str, bit: u32) -> bool {
    has_key_bit_with_width(bitmap, bit, usize::BITS)
}

/// [`has_key_bit`] with the reader's word width stated rather than inferred.
pub fn has_key_bit_with_width(bitmap: &str, bit: u32, width_bits: u32) -> bool {
    if width_bits == 0 || width_bits > 64 {
        return false;
    }
    let words: Vec<&str> = bitmap.split_whitespace().collect();
    if words.is_empty() {
        return false;
    }
    let word_from_right = (bit / width_bits) as usize;
    if word_from_right >= words.len() {
        return false;
    }
    let word = words[words.len() - 1 - word_from_right];
    let Ok(value) = u64::from_str_radix(word, 16) else {
        return false;
    };
    let shift = bit % width_bits;
    (value >> shift) & 1 == 1
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

/// Whether a program resolves on `PATH`. The same shape as
/// `apex/src/blueprint.rs`'s `on_path`, and for the same reason kept separate
/// from a `Command::new` — a lookup is not a spawn.
fn on_path(program: &str) -> bool {
    let Some(paths) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&paths).any(|dir| is_executable(&dir.join(program)))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture(PathBuf);

    impl Fixture {
        fn new(name: &str) -> Fixture {
            let dir = std::env::temp_dir().join(format!("apex-gaming-test-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("fixture root");
            Fixture(dir)
        }

        fn write(&self, rel: &str, text: &str) -> &Fixture {
            let p = self.0.join(rel);
            std::fs::create_dir_all(p.parent().expect("has a parent")).expect("mkdir");
            std::fs::write(&p, text).expect("write");
            self
        }

        fn write_exec(&self, rel: &str, text: &str) -> &Fixture {
            use std::os::unix::fs::PermissionsExt;
            self.write(rel, text);
            let p = self.0.join(rel);
            let mut perms = std::fs::metadata(&p).expect("stat").permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&p, perms).expect("chmod");
            self
        }

        fn probe(&self) -> Probe {
            Probe::with_root(&self.0)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A fixture shaped like a complete Gaming edition.
    fn gaming_edition(name: &str) -> Fixture {
        let f = Fixture::new(name);
        f.write(SESSION_DESKTOP, "[Desktop Entry]\nName=APEX Gaming Mode\n");
        f.write_exec(SESSION_LAUNCHER, "#!/usr/bin/env bash\n");
        f.write_exec(SWITCH_HELPER, "#!/usr/bin/env bash\n");
        f.write(SWITCH_SUDOERS, "%wheel ALL=(root) NOPASSWD: ...\n");
        f.write(RTPRIO_LIMITS, "@wheel - rtprio 20\n");
        f.write(GREETER_LAST_SESSION, GAMING_SESSION);
        f
    }

    // ── the two switches ─────────────────────────────────────────────────────

    #[test]
    fn a_fixture_root_never_probes_programs() {
        // The invariant that keeps an assertion from depending on whose machine
        // ran it. `bash` is on every runner's PATH, so a probe that leaked
        // would report it as measured.
        let f = Fixture::new("noprobe");
        let p = f.probe();
        assert!(!p.probes_programs());
        let r = p.report();
        assert!(
            !r.steam.is_measured(),
            "a fixture root must not answer from the host's PATH"
        );
        assert!(r.steam.reason().unwrap_or("").contains("PATH lookup"));
    }

    #[test]
    fn the_real_probe_does_look_at_path() {
        assert!(Probe::new().probes_programs());
    }

    // ── a stat we are not allowed to make ────────────────────────────────────

    /// Take away every mode bit on the parent so `stat` on a child fails with
    /// `EACCES` instead of `ENOENT`.
    ///
    /// Returns whether the seal actually took. Root walks through a 0000
    /// directory, and so does anyone holding `CAP_DAC_OVERRIDE`, so the check
    /// is whether the child became unreadable rather than whether the caller
    /// looks like root. That keeps the test honest under `sudo cargo test` and
    /// inside a privileged container, where a uid comparison would be a guess.
    fn seal(dir: &Path, child: &Path) -> bool {
        use std::io::ErrorKind;
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(dir).expect("stat").permissions();
        perms.set_mode(0o000);
        std::fs::set_permissions(dir, perms).expect("chmod");
        // Require the exact error we are testing for. Accepting "any error
        // other than NotFound" would let an unrelated failure count as a
        // successful seal, and the assertion would then be about nothing.
        match std::fs::metadata(child) {
            Err(e) if e.kind() == ErrorKind::PermissionDenied => true,
            Ok(_) => false, // root, or CAP_DAC_OVERRIDE
            Err(e) => panic!("expected PermissionDenied while sealing, got {e:?}"),
        }
    }

    fn unseal(dir: &Path) {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(dir).expect("stat").permissions();
        perms.set_mode(0o755);
        let _ = std::fs::set_permissions(dir, perms);
    }

    #[test]
    fn an_unreadable_file_is_unavailable_rather_than_absent() {
        // `/etc/sudoers.d` is 0750 root:root and `apex gaming` runs
        // unprivileged, so this is the real shape of the bug: the sudoers rule
        // is installed correctly and the probe could not look at it. Reporting
        // that as absent sent users chasing a file that was already there.
        let f = gaming_edition("eacces");
        let child = f.0.join(SWITCH_SUDOERS);
        let parent = child.parent().expect("has a parent").to_path_buf();
        if !seal(&parent, &child) {
            unseal(&parent);
            return; // the caller overrides the mode bit; it proves nothing here
        }
        let r = f.probe().report();
        unseal(&parent);

        assert!(
            !r.switch_sudoers.is_measured(),
            "a stat refused with EACCES is not a measurement of absence"
        );
        assert!(
            r.switch_sudoers.reason().unwrap_or("").contains("not readable by this account"),
            "the reason should say the path could not be read, got {:?}",
            r.switch_sudoers.reason()
        );
        // …and it must name the command that DOES answer it. Measured on
        // katana: `sudo apex gaming` returned `sudoers rule : yes` for exactly
        // this file, so "could not read" alone left the reader with nothing.
        assert!(
            r.switch_sudoers.reason().unwrap_or("").contains(ROOT_READABLE_HINT),
            "got {:?}",
            r.switch_sudoers.reason()
        );
        assert_eq!(
            r.rows_needing_root(),
            vec!["sudoers rule"],
            "exactly the refused row, named as the report names it"
        );
    }

    #[test]
    fn an_unreadable_executable_is_unavailable_rather_than_absent() {
        let f = gaming_edition("eacces-exec");
        let child = f.0.join(SWITCH_HELPER);
        let parent = child.parent().expect("has a parent").to_path_buf();
        if !seal(&parent, &child) {
            unseal(&parent);
            return;
        }
        let r = f.probe().report();
        unseal(&parent);

        assert!(
            !r.switch_helper.is_measured(),
            "a mode we cannot read is not a mode without the executable bit"
        );
    }

    #[test]
    fn a_genuinely_missing_file_is_still_measured_absent() {
        // The other half of the distinction: ENOENT must stay a measurement,
        // or the fix would turn every real absence into "could not tell".
        let f = Fixture::new("enoent");
        let r = f.probe().report();
        assert!(r.switch_sudoers.is_measured());
        assert_eq!(r.switch_sudoers.value(), Some(&false));
    }

    // ── the report ───────────────────────────────────────────────────────────

    #[test]
    fn a_complete_gaming_edition_has_no_blockers_it_can_measure() {
        let f = gaming_edition("complete");
        let r = f.probe().report();
        // gamescope/steam are unmeasured under a fixture root, and an unmeasured
        // program is deliberately NOT a blocker.
        assert_eq!(r.blockers(), Vec::<String>::new());
        assert!(r.is_ready());
        assert_eq!(r.boots_to_game(), Some(true));
    }

    // Renamed from `a_daily_edition_is_blocked_and_says_which_edition_it_is`.
    // There are no editions: an image with no gaming session is not "Daily", it
    // is an image from before the merge, or a damaged one. The blocker must say
    // so and must NOT read as something a package install can fix.
    #[test]
    fn an_image_without_the_gaming_session_is_blocked_and_no_package_fixes_it() {
        let f = Fixture::new("premerge");
        f.write(GREETER_LAST_SESSION, "hyprland");
        let r = f.probe().report();
        let joined = r.blockers().join("\n");
        assert!(joined.contains("apex update"), "the remedy must be an OS update: {joined}");
        assert!(
            !joined.contains("Gaming edition"),
            "there are no editions left to name: {joined}"
        );
        assert!(
            !r.blockers_are_installable(),
            "no `apex install` can put image content onto a machine"
        );
        assert!(!r.is_ready());
        assert_eq!(r.boots_to_game(), Some(false));
    }

    // The mirror image, and the case that actually matters now: a current image
    // whose only gap is the on-demand userspace. That IS installable, and it is
    // the only shape of blocker that may carry an install hint.
    #[test]
    fn a_current_image_missing_only_gamescope_is_installable() {
        let f = gaming_edition("needs-userspace");
        let mut r = f.probe().report();
        // The fixture cannot redirect a PATH lookup, so state the program
        // signals directly — that is the whole reason `probe_programs` exists.
        r.gamescope = Signal::measured(false, "PATH:gamescope".to_string());
        r.steam = Signal::measured(false, "PATH:steam".to_string());
        let joined = r.blockers().join("\n");
        assert!(joined.contains("gamescope is not installed"), "{joined}");
        assert!(
            !joined.contains("apex update"),
            "the image is current; nothing here needs an OS update: {joined}"
        );
        assert!(r.blockers_are_installable());
        assert!(!r.is_ready());
    }

    #[test]
    fn a_session_script_that_is_not_executable_is_a_blocker() {
        // The failure a COPY without --chmod=0755 produces: the file exists and
        // the session cannot start.
        let f = gaming_edition("noexec");
        f.write(SESSION_LAUNCHER, "#!/usr/bin/env bash\n");
        let mut perms = std::fs::metadata(f.0.join(SESSION_LAUNCHER))
            .unwrap()
            .permissions();
        {
            use std::os::unix::fs::PermissionsExt;
            perms.set_mode(0o644);
        }
        std::fs::set_permissions(f.0.join(SESSION_LAUNCHER), perms).unwrap();
        let r = f.probe().report();
        assert!(
            r.blockers().join("\n").contains("not executable"),
            "{:?}",
            r.blockers()
        );
    }

    #[test]
    fn a_missing_switch_helper_is_a_warning_not_a_blocker() {
        // Gaming Mode still works; only the desktop's own switch does not.
        let f = gaming_edition("noswitch");
        std::fs::remove_file(f.0.join(SWITCH_HELPER)).unwrap();
        let r = f.probe().report();
        assert!(r.is_ready(), "{:?}", r.blockers());
        assert!(
            r.warnings().join("\n").contains("power menu"),
            "{:?}",
            r.warnings()
        );
    }

    #[test]
    fn a_missing_rtprio_drop_in_is_reported_with_what_it_costs() {
        let f = gaming_edition("nort");
        std::fs::remove_file(f.0.join(RTPRIO_LIMITS)).unwrap();
        let r = f.probe().report();
        assert!(
            r.warnings().join("\n").contains("SCHED_RR"),
            "{:?}",
            r.warnings()
        );
    }

    /// The two realtime rows are independent, and this is the case katana
    /// actually produced: the drop-in present, the soft limit 20, `--rt`
    /// passed, and gamescope still at ordinary priority because it gates on
    /// CAP_SYS_NICE. A single row could not have said that.
    #[test]
    fn the_rtprio_limit_and_cap_sys_nice_are_separate_warnings() {
        let f = gaming_edition("caps");
        // The drop-in is present in this fixture; only the capability is not.
        f.write("proc/self/status", "Uid:\t1000\t1000\t1000\t1000\nCapEff:\t0000000000000000\n");
        let r = f.probe().report();
        assert_eq!(r.rtprio_limits.value(), Some(&true));
        assert_eq!(r.cap_sys_nice.value(), Some(&false));
        let joined = r.warnings().join("\n");
        assert!(joined.contains("CAP_SYS_NICE"), "{:?}", r.warnings());
        assert!(
            !joined.contains("SCHED_RR at all"),
            "the drop-in is present, so its warning must not fire: {:?}",
            r.warnings()
        );
        // And a login that DOES hold it raises neither.
        f.write(
            "proc/self/status",
            "Uid:\t0\t0\t0\t0\nCapEff:\t0000003fffffffff\n",
        );
        let r = f.probe().report();
        assert_eq!(r.cap_sys_nice.value(), Some(&true));
        assert!(
            !r.warnings().join("\n").contains("CAP_SYS_NICE"),
            "{:?}",
            r.warnings()
        );
        assert_eq!(f.probe().effective_uid(), Some(0));
    }

    #[test]
    fn an_unwritten_greeter_record_is_unavailable_not_false() {
        let f = gaming_edition("nogreet");
        std::fs::remove_file(f.0.join(GREETER_LAST_SESSION)).unwrap();
        let r = f.probe().report();
        assert_eq!(r.boots_to_game(), None);
        assert!(r.preselected_session.reason().is_some());
    }

    // ── gamepads ─────────────────────────────────────────────────────────────

    /// A `capabilities/key` bitmap with one bit set, printed the way the kernel
    /// would print it **to this process** — see [`has_key_bit`] on why the word
    /// width belongs to the reader.
    fn bitmap_with(bit: u32) -> String {
        let w = usize::BITS;
        let index = (bit / w) as usize;
        let offset = bit % w;
        let mut s = format!("{:x}", 1u64 << offset);
        for _ in 0..index {
            s.push_str(" 0");
        }
        s.push('\n');
        s
    }

    #[test]
    fn a_gamepad_is_found_through_its_capability_bitmap() {
        let f = gaming_edition("pad");
        f.write(
            "sys/class/input/input5/capabilities/key",
            &bitmap_with(BTN_GAMEPAD),
        );
        f.write("sys/class/input/input5/name", "Microsoft X-Box 360 pad\n");
        let r = f.probe().report();
        assert_eq!(
            r.gamepad.value().map(|v| v.as_slice()),
            Some(["Microsoft X-Box 360 pad".to_string()].as_slice())
        );
        assert!(!r.warnings().join("\n").contains("no input device"));
    }

    #[test]
    fn a_keyboard_is_not_a_gamepad() {
        let f = gaming_edition("kbd");
        // A keyboard sets plenty of low bits and nothing at 0x130, so the
        // kernel elides every word above word 0 — the case that makes indexing
        // from the right necessary rather than convenient.
        f.write("sys/class/input/input3/capabilities/key", "40000000\n");
        f.write("sys/class/input/input3/name", "AT Translated Set 2 keyboard\n");
        let r = f.probe().report();
        assert_eq!(r.gamepad.value().map(Vec::len), Some(0));
        assert!(
            r.warnings().join("\n").contains("no input device"),
            "{:?}",
            r.warnings()
        );
        // …and it is still not a blocker: Big Picture works with a keyboard.
        assert!(r.is_ready());
    }

    #[test]
    fn an_unreadable_capabilities_file_is_unavailable_rather_than_no_gamepad() {
        // The bug: `continue` on any read failure, including a refused one,
        // means a controller this loop could not check is silently treated
        // the same as a device confirmed not to be a gamepad. An empty
        // result then certifies "no controller attached" off a read that
        // never happened.
        let f = gaming_edition("pad-eacces");
        f.write(
            "sys/class/input/input5/capabilities/key",
            &bitmap_with(BTN_GAMEPAD),
        );
        f.write("sys/class/input/input5/name", "Microsoft X-Box 360 pad\n");
        use std::os::unix::fs::PermissionsExt;
        let caps = f.0.join("sys/class/input/input5/capabilities/key");
        let mut perms = std::fs::metadata(&caps).unwrap().permissions();
        perms.set_mode(0o000);
        std::fs::set_permissions(&caps, perms).unwrap();
        let sealed = matches!(
            std::fs::read_to_string(&caps),
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied
        );

        let r = f.probe().report();

        let mut perms = std::fs::metadata(&caps).unwrap().permissions();
        perms.set_mode(0o644);
        std::fs::set_permissions(&caps, perms).ok();

        if !sealed {
            return; // the caller overrides the mode bit; it proves nothing here
        }
        assert!(
            !r.gamepad.is_measured(),
            "a refused capabilities read must not be reported as a measured \
             (and empty) controller list, got {:?}",
            r.gamepad.value()
        );
    }

    #[test]
    fn no_input_class_at_all_is_unavailable_rather_than_empty() {
        let f = gaming_edition("noinput");
        let r = f.probe().report();
        assert!(
            !r.gamepad.is_measured(),
            "a container has no /sys/class/input; that is not 'no controller'"
        );
    }

    #[test]
    fn the_bitmap_reader_finds_the_bit_the_kernel_would_set() {
        // Bit 0 of the only word.
        assert!(has_key_bit_with_width("1", 0, 64));
        assert!(!has_key_bit_with_width("1", 1, 64));
        // With 64-bit words, bit 304 is bit 48 of word 4 from the right.
        let word4 = format!("{:x}", 1u64 << 48);
        let pad = format!("{word4} 0 0 0 0");
        assert!(has_key_bit_with_width(&pad, BTN_GAMEPAD, 64));
        // The same bitmap must NOT report a neighbouring bit.
        assert!(!has_key_bit_with_width(&pad, BTN_GAMEPAD + 1, 64));
        // Indexing is from the RIGHT, so extra leading words change nothing —
        // which is the property that survives the kernel eliding high words.
        assert!(has_key_bit_with_width(
            &format!("ffff ffff {pad}"),
            BTN_GAMEPAD,
            64
        ));
        // Too few words is false, never a panic.
        assert!(!has_key_bit_with_width("1 0", BTN_GAMEPAD, 64));
        // Garbage is false, never a panic.
        assert!(!has_key_bit_with_width("zzzz 0 0 0 0", BTN_GAMEPAD, 64));
        assert!(!has_key_bit_with_width("", 0, 64));
        assert!(!has_key_bit_with_width("1", 0, 0));
    }

    #[test]
    fn a_compat_readers_narrower_words_are_read_correctly() {
        // Under `in_compat_syscall()` the kernel splits each long, so a 32-bit
        // reader sees bit 304 as bit 16 of word 9 from the right — ten words.
        let word9 = format!("{:x}", 1u32 << 16);
        let bitmap = format!("{word9} 0 0 0 0 0 0 0 0 0");
        assert!(has_key_bit_with_width(&bitmap, BTN_GAMEPAD, 32), "{bitmap}");
        // …and the SAME text read as 64-bit words must not find it, or the
        // width would be decorative.
        assert!(!has_key_bit_with_width(&bitmap, BTN_GAMEPAD, 64));
    }

    #[test]
    fn the_default_width_is_this_processs_own() {
        // `has_key_bit` must key off the reader's ABI, not a guess from the
        // string. Proven by constructing the bitmap for `usize::BITS` and
        // showing the other width does not accept it.
        let bitmap = bitmap_with(BTN_GAMEPAD);
        assert!(has_key_bit(&bitmap, BTN_GAMEPAD), "{bitmap}");
        let other = if usize::BITS == 64 { 32 } else { 64 };
        assert!(
            !has_key_bit_with_width(&bitmap, BTN_GAMEPAD, other),
            "the two widths must disagree, or this test proves nothing"
        );
    }

    // ── the constant that must not drift ─────────────────────────────────────

    #[test]
    fn the_gaming_session_id_matches_the_blueprints() {
        assert_eq!(GAMING_SESSION, crate::blueprint::GAMING_SESSION);
    }

    // ── what a blocker tells you to do about it ──────────────────────────────

    /// A `Readiness` with everything present except the named signals, which
    /// are measured *absent*. Measured-false, not unmeasured: an unmeasured
    /// program is deliberately not a blocker, so an unmeasured fixture would
    /// exercise none of this.
    fn readiness_missing(absent: &[&str]) -> Readiness {
        let present = |name: &str| Signal::measured(!absent.contains(&name), "test fixture");
        Readiness {
            session_desktop: present("session_desktop"),
            session_launcher: present("session_launcher"),
            switch_helper: present("switch_helper"),
            switch_sudoers: present("switch_sudoers"),
            rtprio_limits: present("rtprio_limits"),
            preselected_session: Signal::measured(GAMING_SESSION.to_string(), "test fixture"),
            gamepad: Signal::measured(vec!["/dev/input/event0".to_string()], "test fixture"),
            gamescope: present("gamescope"),
            steam: present("steam"),
            mangoapp: present("mangoapp"),
            cap_sys_nice: present("cap_sys_nice"),
            display: crate::gpu::DisplayChoice {
                output: Some("HDMI-A-1".to_string()),
                card: Some("card2".to_string()),
                pci_id: Some("10de:249d".to_string()),
                vendor: Some("NVIDIA".to_string()),
                cards_with_displays: 2,
                why: "test fixture".to_string(),
                problem: None,
            },
        }
    }

    #[test]
    fn every_installable_blocker_is_covered_by_the_install_hint() {
        // The hint is derived separately from `blockers()`, so it can drift.
        // Whenever a program blocker is raised, the hint must name it.
        for absent in [
            vec!["gamescope"],
            vec!["steam"],
            vec!["gamescope", "steam"],
        ] {
            let r = readiness_missing(&absent);
            let hint = r
                .install_hint()
                .unwrap_or_else(|| panic!("{absent:?} blocks but offers no remedy"));
            for pkg in &absent {
                assert!(
                    hint.contains(pkg),
                    "{pkg} blocks Gaming Mode but {hint:?} does not install it"
                );
            }
        }
    }

    #[test]
    fn the_remedy_is_one_line_and_not_repeated_inside_each_blocker() {
        // Blocker prose is word-wrapped to the terminal, which would split a
        // command across lines. The command lives only in `install_hint`.
        let joined = readiness_missing(&["gamescope", "steam"]).blockers().join("\n");
        assert!(
            !joined.contains("apex install"),
            "an embedded command gets wrapped and stops being pastable: {joined}"
        );
    }

    #[test]
    fn both_missing_programs_collapse_into_one_install_command() {
        // Two blockers, but one command clears both — printing two separate
        // `apex install` lines would have the reader run the engine twice.
        let r = readiness_missing(&["gamescope", "steam"]);
        assert_eq!(
            r.install_hint().as_deref(),
            Some("sudo apex install gamescope steam")
        );
    }

    #[test]
    fn the_install_hint_names_only_what_is_actually_missing() {
        let r = readiness_missing(&["steam"]);
        assert_eq!(r.install_hint().as_deref(), Some("sudo apex install steam"));
    }

    #[test]
    fn a_ready_machine_has_no_install_hint() {
        let r = readiness_missing(&[]);
        assert!(r.is_ready(), "{:?}", r.blockers());
        assert_eq!(r.install_hint(), None);
    }

    #[test]
    fn a_pre_merge_image_gets_no_install_hint_because_no_package_fixes_it() {
        // The greeter entry is image content on every current image, so its
        // absence means the machine is booting something from before the
        // editions merged. Offering `apex install` would send the reader after
        // a package that cannot supply it.
        let r = readiness_missing(&["session_desktop"]);
        assert!(!r.is_ready());
        assert_eq!(r.install_hint(), None);
    }

    #[test]
    fn a_pre_merge_image_is_silent_even_though_its_programs_are_missing_too() {
        // The case the `blockers_are_installable` gate exists for, and the one
        // a hint derived from the programs alone gets WRONG. A machine on a
        // pre-merge image has no gaming session AND no gamescope/steam, so
        // "some program is missing" is true and would print `sudo apex install
        // gamescope steam`. The reader would run it, both packages would
        // install, and Gaming Mode still would not start — their actual remedy
        // is `apex update`.
        let r = readiness_missing(&["session_desktop", "session_launcher", "gamescope", "steam"]);
        assert!(!r.blockers_are_installable());
        assert_eq!(
            r.install_hint(),
            None,
            "an install hint here sends the reader to a command that cannot work"
        );
        assert!(
            r.blockers().join("\n").contains("apex update"),
            "and the blockers must still name the remedy that does work"
        );
    }
}
