//! Lid-closed continuous operation (P1-063).
//!
//! The request, in the owner's words:
//!
//! > "if i close my laptop lid without shutting down, most things pause to save
//! > battery, but all agents and whatever theyre using/doing or whatever can
//! > stay running, also staying with the vpn — like if i close my laptop at
//! > school and codex is running (which needs vpn to work) it keeps working."
//!
//! ── The primitive, and what it deliberately is not ───────────────────────────
//!
//! This module never writes `HandleLidSwitch=`. A static `HandleLidSwitch=ignore`
//! turns every laptop bag into an oven: it applies to a machine with nothing
//! running as readily as to one mid-build, and it survives a crash of whatever
//! set it. The primitive here is a **logind `handle-lid-switch` block
//! inhibitor**, which is dynamic, is scoped to the process holding it, and
//! evaporates when that process dies — so a machine with no live work suspends
//! on a lid close exactly as it does today, and a machine whose keeper crashed
//! falls back to suspending rather than to cooking.
//!
//! Measured on the L16 on 2026-09-12 rather than assumed: the polkit action
//! `org.freedesktop.login1.inhibit-handle-lid-switch` defaults to
//! `allow_active=yes`, `pkcheck` on an ordinary session process exits 0 without
//! being allowed to interact, and the inhibitor was taken for real as uid 1000
//! with nothing appearing on screen. No polkit rule was added and none is
//! needed.
//!
//! ── Why the decision is a pure function ──────────────────────────────────────
//!
//! Everything in [`LidPolicy::decide`] is total over its inputs and touches
//! nothing. A lid policy is exactly the kind of code that cannot be tested by
//! running it: the act it authorises (or refuses) is the machine going to sleep
//! in someone's bag, and the machine that would have to perform it belongs to a
//! person using it. So the matrix — pin × work × lid × thermal × charge — is
//! asserted here, against injected readings, and the driver that gathers those
//! readings is a separate, thin thing.
//!
//! ── Which way each unknown fails ─────────────────────────────────────────────
//!
//! The same rule as `apex-agent-core`'s `LockState`: a reading that could
//! not be taken is not a reading of "fine". This codebase has already swept a
//! defect class where a failed `stat` was recorded as a checked absence, so
//! every measurement here carries an `Unreadable(String)` variant and every one
//! of them fails towards **suspending**:
//!
//! * work unreadable   → not live → the lid suspends as it does today;
//! * thermal unreadable → a guard fires and the machine suspends anyway;
//! * charge unreadable  → a guard fires and the machine suspends anyway;
//! * lid unreadable     → treated as closed, so the guards apply.
//!
//! "It kept working until it died" is worse than suspending, and a machine that
//! cannot tell you it is overheating must not be trusted to stay awake inside a
//! schoolbag.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// Where the lid is.
///
/// `NoLid` and `Unreadable` are different answers and neither is `Open`. A
/// desktop has no lid at all — the policy is inert there rather than wrong —
/// and a lid whose state could not be read is treated as closed, because that
/// is the arm where the guards are needed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum LidState {
    Open,
    Closed,
    /// This machine has no lid: a desktop, a VM, a server.
    NoLid,
    /// A lid exists and its position could not be determined. Carries the
    /// reason. Never a synonym for open.
    Unreadable { why: String },
}

impl LidState {
    /// Whether the guards should be evaluated.
    ///
    /// True for `Closed` and for `Unreadable`. An unreadable lid is treated as
    /// closed for the same reason an unreadable screen counts as locked in
    /// `apex-agent-core`'s `lock.rs`: the variant exists so that a failed
    /// read cannot be laundered into the permissive answer.
    pub fn treat_as_closed(&self) -> bool {
        matches!(self, LidState::Closed | LidState::Unreadable { .. })
    }

    /// A machine with no lid can never be in a bag with the fan blocked.
    pub fn is_absent(&self) -> bool {
        matches!(self, LidState::NoLid)
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            LidState::Open => "open",
            LidState::Closed => "closed",
            LidState::NoLid => "no lid",
            LidState::Unreadable { .. } => "unknown",
        }
    }
}

impl std::fmt::Display for LidState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LidState::Unreadable { why } => write!(f, "unknown — {why}"),
            other => f.pad(other.as_str()),
        }
    }
}

/// The owner's override.
///
/// `Auto` is the default and the point of the feature: the machine works out
/// whether there is live work rather than asking the owner to remember a mode.
/// The two explicit pins exist because a measurement can only ever see what it
/// knows how to look for — a long `rsync`, a kernel build in a terminal, a
/// download — and because the opposite case matters too: an owner who wants a
/// guaranteed suspend at the moment the lid shuts should be able to say so
/// without uninstalling anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Pin {
    /// Decide from measured work. The default.
    #[default]
    Auto,
    /// Always keep working through a lid close (guards still apply).
    On,
    /// Never keep working; the lid suspends as it does on a stock install.
    Off,
}

impl Pin {
    pub fn parse(s: &str) -> Option<Pin> {
        match s {
            "auto" => Some(Pin::Auto),
            "on" | "yes" | "true" => Some(Pin::On),
            "off" | "no" | "false" => Some(Pin::Off),
            _ => None,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Pin::Auto => "auto",
            Pin::On => "on",
            Pin::Off => "off",
        }
    }
}

impl std::fmt::Display for Pin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.pad(self.as_str())
    }
}

/// Whether there is work worth staying awake for.
///
/// Counted, not guessed, and the count is carried so the after-the-fact report
/// can say *what* kept the machine up rather than only that something did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "work", rename_all = "kebab-case")]
pub enum Work {
    /// `n` live agent sessions were counted. `n == 0` is a real measurement of
    /// "nothing running", not an error.
    Sessions { live: u32 },
    /// The agent runtime could not be asked — it is not enabled, its socket is
    /// absent, or the query failed. Carries the reason and counts as **no**
    /// work: a machine that cannot tell whether anything is running is not a
    /// machine to leave awake in a bag. The pin is the override for that.
    Unreadable { why: String },
}

impl Work {
    pub fn is_live(&self) -> bool {
        matches!(self, Work::Sessions { live } if *live > 0)
    }

    pub fn live_count(&self) -> u32 {
        match self {
            Work::Sessions { live } => *live,
            Work::Unreadable { .. } => 0,
        }
    }

    pub fn reason(&self) -> Option<&str> {
        match self {
            Work::Unreadable { why } => Some(why),
            Work::Sessions { .. } => None,
        }
    }
}

/// The hottest temperature the machine will admit to, in degrees Celsius.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "thermal", rename_all = "kebab-case")]
pub enum Thermal {
    Celsius { c: f64 },
    /// There is no temperature sensor at all. Absence, not failure — and still
    /// not safe: see [`LidPolicy::require_thermal`].
    NoSensor,
    /// Sensors exist and could not be read. Carries the reason.
    Unreadable { why: String },
}

impl Thermal {
    pub fn celsius(&self) -> Option<f64> {
        match self {
            Thermal::Celsius { c } => Some(*c),
            _ => None,
        }
    }

    /// Read the live machine's hottest sensor under `sys_root`.
    ///
    /// Reuses [`crate::fan::read_curve_temp`], which already knows the sensor
    /// ladder (preferred package chips, then any hwmon, then the thermal
    /// zones). The one thing it cannot express is the difference between "this
    /// machine has no sensors" and "the sensors are there and would not read",
    /// which is exactly the distinction this policy turns on — so the two are
    /// separated here by asking whether the class directories exist at all.
    pub fn read(sys_root: &Path) -> Thermal {
        if let Some(c) = crate::fan::read_curve_temp(sys_root) {
            return Thermal::Celsius { c };
        }
        let hwmon = sys_root.join("class/hwmon");
        let thermal = sys_root.join("class/thermal");
        let hwmon_there = hwmon.is_dir();
        let thermal_there = thermal.is_dir();
        if !hwmon_there && !thermal_there {
            return Thermal::NoSensor;
        }
        // Directories are present but nothing in them yielded a plausible
        // reading. That is a failed read, and it is reported as one: an empty
        // `class/hwmon` on a laptop means a driver did not bind, not that the
        // machine runs cold.
        let mut seen = Vec::new();
        for (there, dir) in [(hwmon_there, &hwmon), (thermal_there, &thermal)] {
            if !there {
                continue;
            }
            match std::fs::read_dir(dir) {
                Ok(entries) => {
                    let n = entries.count();
                    seen.push(format!("{} has {n} entries and none reported a usable temperature", dir.display()));
                }
                Err(e) => seen.push(format!("{} could not be listed: {e}", dir.display())),
            }
        }
        Thermal::Unreadable { why: seen.join("; ") }
    }
}

/// Where the machine's energy is coming from, and how much is left.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "charge", rename_all = "kebab-case")]
pub enum Charge {
    /// On mains. The battery floor is not a concern.
    Ac,
    /// On battery, with a known percentage remaining.
    Battery { percent: u8 },
    /// No battery exists — a desktop. Not the same as a full one, and not a
    /// reason to fire the floor guard.
    NoBattery,
    /// A battery exists and its state could not be read. Carries the reason.
    Unreadable { why: String },
}

impl Charge {
    pub fn percent(&self) -> Option<u8> {
        match self {
            Charge::Battery { percent } => Some(*percent),
            _ => None,
        }
    }

    pub fn on_ac(&self) -> bool {
        matches!(self, Charge::Ac)
    }

    /// Read the live machine's power state under `sys_root`.
    ///
    /// Mains is decided by any `type=Mains` power supply reporting `online=1`,
    /// which is how the kernel spells it on every laptop APEX runs on. The
    /// battery percentage is the primary pack's `capacity`, falling back to
    /// `energy_now/energy_full` where a driver omits `capacity`.
    pub fn read(sys_root: &Path) -> Charge {
        let dir = sys_root.join(crate::battery::POWER_SUPPLY_REL);
        let mut mains_online: Option<bool> = None;
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for e in entries.flatten() {
                let p = e.path();
                let kind = read_trim(&p.join("type")).unwrap_or_default();
                if kind != "Mains" && kind != "USB" {
                    continue;
                }
                match read_trim(&p.join("online")).as_deref() {
                    Some("1") => mains_online = Some(true),
                    Some("0") => mains_online = Some(mains_online.unwrap_or(false)),
                    _ => {}
                }
            }
        }
        if mains_online == Some(true) {
            return Charge::Ac;
        }

        let inv = crate::battery::BatteryInventory::discover(sys_root);
        if inv.is_empty() {
            // No battery and no mains supply either means the whole
            // power_supply class is missing (a container, some VMs). Report
            // that as absence: a machine with no battery cannot run one flat.
            return Charge::NoBattery;
        }
        let Some(bat) = inv.primary() else {
            return Charge::NoBattery;
        };
        if let Some(pct) = bat.read("capacity").and_then(|s| s.trim().parse::<u8>().ok()) {
            return Charge::Battery { percent: pct.min(100) };
        }
        let now = bat.read("energy_now").or_else(|| bat.read("charge_now"));
        let full = bat.read("energy_full").or_else(|| bat.read("charge_full"));
        if let (Some(n), Some(f)) = (
            now.as_deref().and_then(|s| s.parse::<u64>().ok()),
            full.as_deref().and_then(|s| s.parse::<u64>().ok()),
        ) {
            if f > 0 {
                let pct = ((n.min(f) as f64 / f as f64) * 100.0).round() as u8;
                return Charge::Battery { percent: pct.min(100) };
            }
        }
        Charge::Unreadable {
            why: format!(
                "battery {} is present at {} and reported neither `capacity` nor a usable \
                 energy_now/energy_full pair",
                bat.name, bat.path
            ),
        }
    }
}

/// Which guard ended a closed period — or that none did.
///
/// Every variant names itself, because "the machine suspended in my bag" is
/// useless as a report and "the thermal guard fired at 87 °C" is actionable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Guard {
    /// The hottest sensor reached the ceiling.
    Thermal,
    /// A machine that cannot report its temperature does not get to stay awake
    /// with its vents against a textbook.
    ThermalUnreadable,
    /// The machine has no temperature sensor at all, and the owner has not said
    /// that is acceptable. See [`LidPolicy::require_thermal`].
    ThermalNoSensor,
    /// Charge fell to the floor. The driver checkpoints before this suspend.
    Battery,
    /// The battery is there and would not say how full it is.
    BatteryUnreadable,
}

impl Guard {
    pub const fn as_str(self) -> &'static str {
        match self {
            Guard::Thermal => "thermal",
            Guard::ThermalUnreadable => "thermal-unreadable",
            Guard::ThermalNoSensor => "thermal-no-sensor",
            Guard::Battery => "battery",
            Guard::BatteryUnreadable => "battery-unreadable",
        }
    }

    /// Whether the driver must checkpoint live sessions before acting.
    ///
    /// Both battery guards: the machine is about to run out of energy, and the
    /// whole difference between this feature and a crash is that the work is
    /// written down first. The thermal guards checkpoint too — there is no
    /// reason not to, and a machine hot enough to fire that guard is a machine
    /// whose next event might be a firmware shutdown rather than a suspend.
    pub const fn checkpoint_first(self) -> bool {
        true
    }
}

impl std::fmt::Display for Guard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.pad(self.as_str())
    }
}

/// What the driver should do right now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "kebab-case")]
pub enum LidDecision {
    /// Hold the `handle-lid-switch` block inhibitor.
    ///
    /// Taken *before* the lid closes, not in response to it: logind decides at
    /// the `SW_LID` edge, and a holder that reacts to the edge has already
    /// lost. So this is the answer whenever work is live, lid open or shut.
    KeepWorking { why: String },
    /// Do not hold the inhibitor. A lid close suspends exactly as it does on a
    /// stock install, which is the criterion this arm exists to satisfy.
    Release { why: String },
    /// Work is live, the lid is shut, and a guard fired: checkpoint, release
    /// the inhibitor and suspend deliberately rather than wait to be cooked or
    /// to die mid-task.
    GuardSuspend { guard: Guard, why: String },
}

impl LidDecision {
    /// Whether the inhibitor should be held after this decision.
    pub fn holds_inhibitor(&self) -> bool {
        matches!(self, LidDecision::KeepWorking { .. })
    }

    /// The guard that fired, when one did.
    pub fn guard(&self) -> Option<Guard> {
        match self {
            LidDecision::GuardSuspend { guard, .. } => Some(*guard),
            _ => None,
        }
    }

    /// Whether the machine should be put to sleep now.
    pub fn suspends_now(&self) -> bool {
        matches!(self, LidDecision::GuardSuspend { .. })
    }

    pub fn why(&self) -> &str {
        match self {
            LidDecision::KeepWorking { why }
            | LidDecision::Release { why }
            | LidDecision::GuardSuspend { why, .. } => why,
        }
    }

    pub const fn as_str(&self) -> &'static str {
        match self {
            LidDecision::KeepWorking { .. } => "keep-working",
            LidDecision::Release { .. } => "release",
            LidDecision::GuardSuspend { .. } => "guard-suspend",
        }
    }
}

/// Everything the decision is made from, in one value.
///
/// A struct rather than five arguments so that a new input cannot be added
/// without every construction site being made to name it — the matrix tests
/// included.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LidInputs {
    pub lid: LidState,
    pub work: Work,
    pub thermal: Thermal,
    pub charge: Charge,
}

/// The owner's settings.
///
/// Serialised into `~/.config/apex/lid.toml` (and `/etc/apex/lid.toml` for the
/// system driver) with `#[serde(default)]` on the container, so a file naming
/// one key keeps the shipped answer for the rest instead of deserialising the
/// others as `false`/`0` — the same trap `LockPolicy` documents.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LidPolicy {
    /// Master switch. `false` makes the whole feature inert and the machine
    /// behaves as a stock install.
    pub enabled: bool,
    /// The owner's override.
    pub pin: Pin,
    /// Charge percentage at or below which the battery guard fires.
    ///
    /// 20 rather than 5: the point of the floor is to checkpoint and suspend
    /// with enough left to resume and finish, not to squeeze the last watt out
    /// and hand the owner a dead laptop and a half-written file.
    pub battery_floor_pct: u8,
    /// Temperature at or above which the thermal guard fires, in Celsius.
    ///
    /// 85 is below every `Critical` trip point on the hardware APEX has been
    /// measured on and above any sustained load temperature a closed,
    /// mostly-idle machine should reach. A lid-closed machine in a bag has no
    /// airflow, so this is a *lower* ceiling than an open machine would use.
    pub thermal_ceiling_c: f64,
    /// Whether a machine with no temperature sensor at all may stay awake.
    ///
    /// `true` (the default) means it may not: [`Guard::ThermalNoSensor`] fires.
    /// That is the safe direction and it is deliberately overridable, because
    /// a machine that genuinely has no sensors would otherwise have the
    /// feature dead on arrival with nothing saying why.
    pub require_thermal: bool,
    /// Seconds between re-evaluations while the lid is shut.
    ///
    /// The decision is not made once at the edge: a bag heats up and a battery
    /// drains, so the guards must be re-asked for the whole closed period.
    pub poll_secs: u64,
    /// What gets powered down for a keep-working close.
    pub powerdown: PowerDown,
}

impl Default for LidPolicy {
    fn default() -> LidPolicy {
        LidPolicy {
            enabled: true,
            pin: Pin::Auto,
            battery_floor_pct: 20,
            thermal_ceiling_c: 85.0,
            require_thermal: true,
            poll_secs: 30,
            powerdown: PowerDown::default(),
        }
    }
}

/// What "everything not needed for that work" means, written down.
///
/// A named, configurable list rather than a hard-coded sweep, because
/// "discretionary" is a judgement about *this* owner's machine and a policy
/// that guesses it silently is a policy that one day stops something load
/// bearing at the bottom of a schoolbag.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PowerDown {
    /// Blank the panel. The single largest saving on a laptop and the one
    /// thing that is unambiguously useless with the lid shut.
    pub display: bool,
    /// Zero the keyboard backlight, restoring the prior value on reopen.
    pub keyboard_backlight: bool,
    /// Soft-block Bluetooth. Restored on reopen only if this policy blocked it
    /// — an owner who had it off already gets it left off.
    pub bluetooth: bool,
    /// Turn Wi-Fi power saving **off** for the closed period.
    ///
    /// This costs power, and it is on by default anyway, because the request
    /// that motivated the whole item is a VPN that keeps working. Aggressive
    /// 802.11 power save is a known way to lose a long-lived tunnel with no
    /// suspend involved at all. The cost is recorded in the report rather than
    /// hidden: see [`PowerAction::WifiPowerSave`].
    pub wifi_powersave_off: bool,
    /// User-level systemd units stopped for the closed period and started
    /// again on reopen — and *only* those that were actually running when the
    /// lid shut, so the restore is exact.
    ///
    /// The default is periodic maintenance: work that has no deadline, wakes
    /// the machine on a timer, and can wait for the lid to open. Nothing
    /// interactive, nothing an agent session depends on, and explicitly not
    /// the shell or the agent runtime.
    pub stop_user_units: Vec<String>,
    /// System-level units, same rule.
    pub stop_system_units: Vec<String>,
}

impl Default for PowerDown {
    fn default() -> PowerDown {
        PowerDown {
            display: true,
            keyboard_backlight: true,
            bluetooth: true,
            wifi_powersave_off: true,
            stop_user_units: vec![
                // Anthropic's desktop-app updater stopgap, and any other
                // per-user update timer: an update that starts while the
                // machine is in a bag is an update nobody can answer a prompt
                // for.
                "claude-desktop-update.timer".to_string(),
            ],
            stop_system_units: vec![
                "apex-storage-notice.timer".to_string(),
                "fwupd-refresh.timer".to_string(),
                "dnf-makecache.timer".to_string(),
                "flatpak-system-update.timer".to_string(),
                "podman-auto-update.timer".to_string(),
                "raid-check.timer".to_string(),
                "mlocate-updatedb.timer".to_string(),
            ],
        }
    }
}

impl LidPolicy {
    /// The whole policy, as one total function over measured inputs.
    ///
    /// Order matters and is asserted: the master switch, then a machine with no
    /// lid, then the pin, then whether there is work, and only then the guards.
    /// Guards are evaluated **last and only when the lid is shut** — a guard
    /// that suspended an open machine the owner is typing on would be a bug
    /// wearing a safety feature's clothes.
    pub fn decide(&self, inputs: &LidInputs) -> LidDecision {
        if !self.enabled {
            return LidDecision::Release {
                why: "lid keep-working is disabled (`enabled = false`); the lid suspends as it \
                      does on a stock install"
                    .to_string(),
            };
        }

        if inputs.lid.is_absent() {
            return LidDecision::Release {
                why: "this machine has no lid, so there is nothing to keep open".to_string(),
            };
        }

        // ── The pin ─────────────────────────────────────────────────────────
        // `Off` short-circuits past the guards on purpose: with no inhibitor
        // held there is no closed-and-awake period for a guard to protect.
        match self.pin {
            Pin::Off => {
                return LidDecision::Release {
                    why: "the owner pinned lid keep-working off; the lid suspends immediately"
                        .to_string(),
                }
            }
            Pin::On => {}
            Pin::Auto => {
                if !inputs.work.is_live() {
                    let why = match inputs.work.reason() {
                        Some(r) => format!(
                            "no live work could be established ({r}), which is treated as no \
                             work — pin keep-working on to override"
                        ),
                        None => "no agent session is running, so the lid suspends as it does \
                                 today"
                            .to_string(),
                    };
                    return LidDecision::Release { why };
                }
            }
        }

        // From here the machine *wants* to stay up. Say why once, so both the
        // keep-working arm and any guard message can quote it.
        let want = match self.pin {
            Pin::On => "the owner pinned lid keep-working on".to_string(),
            _ => {
                let n = inputs.work.live_count();
                let s = if n == 1 { "session" } else { "sessions" };
                format!("{n} live agent {s}")
            }
        };

        // ── Guards, only with the lid shut ──────────────────────────────────
        if !inputs.lid.treat_as_closed() {
            return LidDecision::KeepWorking {
                why: format!("{want}; the lid is open"),
            };
        }

        let closed_as = match &inputs.lid {
            LidState::Unreadable { why } => {
                format!("the lid position could not be read ({why}), which is treated as closed")
            }
            _ => "the lid is closed".to_string(),
        };

        // Thermal first: it is the guard whose failure damages hardware, where
        // the battery guard's failure loses a session's work.
        match &inputs.thermal {
            Thermal::Celsius { c } if *c >= self.thermal_ceiling_c => {
                return LidDecision::GuardSuspend {
                    guard: Guard::Thermal,
                    why: format!(
                        "{closed_as} and the hottest sensor reads {c:.1} °C, at or above the \
                         {:.1} °C lid-closed ceiling — a shut lid has no airflow, so the \
                         machine suspends rather than cook ({want} checkpointed first)",
                        self.thermal_ceiling_c
                    ),
                }
            }
            Thermal::Unreadable { why } => {
                return LidDecision::GuardSuspend {
                    guard: Guard::ThermalUnreadable,
                    why: format!(
                        "{closed_as} and the temperature could not be read ({why}); a machine \
                         that cannot report its own heat is not left awake in a bag"
                    ),
                }
            }
            Thermal::NoSensor if self.require_thermal => {
                return LidDecision::GuardSuspend {
                    guard: Guard::ThermalNoSensor,
                    why: "this machine exposes no temperature sensor at all, so the thermal \
                          guard cannot protect it; set `require_thermal = false` to accept that \
                          deliberately"
                        .to_string(),
                }
            }
            _ => {}
        }

        match &inputs.charge {
            Charge::Battery { percent } if *percent <= self.battery_floor_pct => {
                return LidDecision::GuardSuspend {
                    guard: Guard::Battery,
                    why: format!(
                        "{closed_as} and the battery is at {percent}%, at or below the {}% \
                         floor — live work is checkpointed and the machine suspends with \
                         enough charge left to resume it",
                        self.battery_floor_pct
                    ),
                }
            }
            Charge::Unreadable { why } => {
                return LidDecision::GuardSuspend {
                    guard: Guard::BatteryUnreadable,
                    why: format!(
                        "{closed_as} and the battery would not report its charge ({why}); an \
                         unknown charge is not a full one"
                    ),
                }
            }
            _ => {}
        }

        let power = match &inputs.charge {
            Charge::Ac => "on mains".to_string(),
            Charge::Battery { percent } => format!("battery {percent}%"),
            Charge::NoBattery => "no battery".to_string(),
            Charge::Unreadable { .. } => unreachable!("handled by the battery guard above"),
        };
        LidDecision::KeepWorking {
            why: format!("{want}; {closed_as} and every guard is clear ({power})"),
        }
    }
}

/// One reversible thing done to the machine for a keep-working close.
///
/// Each variant carries what is needed to undo it, captured at the moment it
/// was applied. Nothing is restored from a default: a keyboard backlight put
/// back to "the usual" rather than to the value it had is a small lie that the
/// owner notices every single time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "kebab-case")]
pub enum PowerAction {
    /// Blank every output.
    Display { on: bool },
    /// Write a `/sys/class/leds/*/brightness`, remembering the prior value.
    KeyboardBacklight { path: String, value: u32, prior: u32 },
    /// `rfkill block bluetooth` / `unblock`.
    Bluetooth { blocked: bool },
    /// `iw dev <dev> set power_save on|off`, remembering the prior setting.
    ///
    /// Named separately from the rest because it is the one action here that
    /// *costs* power rather than saving it, and the report says so.
    WifiPowerSave { dev: String, on: bool, prior: Option<bool> },
    /// `systemctl [--user] stop|start <unit>`.
    Unit { unit: String, user: bool, start: bool },
}

impl PowerAction {
    /// A one-line description for the report.
    pub fn describe(&self) -> String {
        match self {
            PowerAction::Display { on } => {
                format!("display {}", if *on { "on" } else { "off" })
            }
            PowerAction::KeyboardBacklight { path, value, prior } => {
                format!("keyboard backlight {path} {prior} -> {value}")
            }
            PowerAction::Bluetooth { blocked } => {
                format!("bluetooth {}", if *blocked { "blocked" } else { "unblocked" })
            }
            PowerAction::WifiPowerSave { dev, on, .. } => format!(
                "wifi power save {} on {dev} (costs power; keeps the tunnel alive)",
                if *on { "on" } else { "off" }
            ),
            PowerAction::Unit { unit, user, start } => format!(
                "{} {}{unit}",
                if *start { "start" } else { "stop" },
                if *user { "--user " } else { "" }
            ),
        }
    }

    /// The action that undoes this one.
    pub fn inverse(&self) -> PowerAction {
        match self {
            PowerAction::Display { on } => PowerAction::Display { on: !on },
            PowerAction::KeyboardBacklight { path, value, prior } => {
                PowerAction::KeyboardBacklight {
                    path: path.clone(),
                    value: *prior,
                    prior: *value,
                }
            }
            PowerAction::Bluetooth { blocked } => PowerAction::Bluetooth { blocked: !blocked },
            PowerAction::WifiPowerSave { dev, on, prior } => PowerAction::WifiPowerSave {
                dev: dev.clone(),
                // A prior that could not be read is restored to `on`, the
                // kernel and NetworkManager default, and the report says the
                // prior was unknown rather than pretending it was measured.
                on: prior.unwrap_or(true),
                prior: Some(*on),
            },
            PowerAction::Unit { unit, user, start } => PowerAction::Unit {
                unit: unit.clone(),
                user: *user,
                start: !start,
            },
        }
    }
}

/// What could not be done, and why — never silently dropped.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Skipped {
    pub what: String,
    pub why: String,
}

/// The power-down half of a keep-working close.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PowerPlan {
    pub actions: Vec<PowerAction>,
    /// Everything the plan wanted and could not have. A permission denied, an
    /// absent LED class and a disabled policy line are three different
    /// sentences, and all three appear here rather than as an empty plan that
    /// looks like success.
    pub skipped: Vec<Skipped>,
}

impl PowerPlan {
    pub fn is_empty(&self) -> bool {
        self.actions.is_empty()
    }

    /// The plan that puts the machine back as it was, in reverse order.
    ///
    /// Reverse because the units were stopped last and should come back first:
    /// a timer restarted before the radio it needs is a timer that fails once
    /// on every reopen.
    pub fn restore(&self) -> PowerPlan {
        PowerPlan {
            actions: self.actions.iter().rev().map(PowerAction::inverse).collect(),
            skipped: Vec::new(),
        }
    }
}

/// A closed period, start to finish, as the machine will report it.
///
/// This is the sixth acceptance criterion: after reopening, the machine says
/// what happened. It is written by the driver at every poll so that a period
/// ended by a guard — or by the battery running out despite one — still leaves
/// a record behind.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClosedPeriod {
    /// Unix seconds when the lid shut.
    pub closed_at: u64,
    /// Unix seconds of the last poll, or of the reopen.
    pub last_seen: u64,
    /// Set once the lid opens again.
    pub opened_at: Option<u64>,
    /// Live agent sessions at the moment the lid shut.
    pub sessions_at_close: u32,
    /// Why the machine stayed awake.
    pub why: String,
    /// The guard that ended it, if one did.
    pub ended_by: Option<Guard>,
    /// Battery percentage at close and at the last poll — the honest form of
    /// "what it cost", because a machine on mains costs nothing and should say
    /// so rather than report a fabricated delta.
    pub charge_at_close: Option<u8>,
    pub charge_last: Option<u8>,
    /// Hottest reading seen during the period.
    pub peak_c: Option<f64>,
    /// What was powered down, in the words of [`PowerAction::describe`].
    pub powered_down: Vec<String>,
    /// Everything the power-down could not do.
    pub skipped: Vec<Skipped>,
    /// One entry per poll: the VPN's state at that moment.
    ///
    /// A timeline, not a verdict, and that is the point. "Did the VPN hold" is
    /// then answered from what was observed every 30 seconds rather than
    /// inferred from the machine not having suspended — which is precisely the
    /// assumption this item was told not to make.
    pub vpn: Vec<VpnSample>,
}

/// The VPN, once, at a moment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VpnSample {
    /// Unix seconds.
    pub at: u64,
    /// The tunnel's name, when there is one.
    pub name: Option<String>,
    pub state: VpnState,
}

/// Whether a tunnel is up.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "vpn", rename_all = "kebab-case")]
pub enum VpnState {
    /// A tunnel is active.
    Up,
    /// A tunnel was active when the lid shut and is not now. This is the answer
    /// the whole criterion exists to catch.
    Down,
    /// There was no VPN to keep. Not a failure and not a success.
    None,
    /// NetworkManager could not be asked. Carries the reason.
    Unreadable { why: String },
}

impl VpnState {
    pub fn as_str(&self) -> &'static str {
        match self {
            VpnState::Up => "up",
            VpnState::Down => "down",
            VpnState::None => "none",
            VpnState::Unreadable { .. } => "unknown",
        }
    }
}

impl ClosedPeriod {
    /// Whether the VPN held for the whole period.
    ///
    /// `None` when there was nothing to hold, or when the samples are not
    /// conclusive — an unreadable sample is not a held tunnel and is not a
    /// dropped one either, and collapsing it into `true` would turn this
    /// report into the assumption it replaced.
    pub fn vpn_held(&self) -> Option<bool> {
        if self.vpn.is_empty() {
            return None;
        }
        if self.vpn.iter().all(|s| matches!(s.state, VpnState::None)) {
            return None;
        }
        if self.vpn.iter().any(|s| matches!(s.state, VpnState::Down)) {
            return Some(false);
        }
        if self.vpn.iter().any(|s| matches!(s.state, VpnState::Unreadable { .. })) {
            return None;
        }
        Some(true)
    }

    /// Battery percentage consumed, when both ends are known.
    pub fn charge_cost(&self) -> Option<i32> {
        match (self.charge_at_close, self.charge_last) {
            (Some(a), Some(b)) => Some(a as i32 - b as i32),
            _ => None,
        }
    }

    /// Seconds the machine stayed awake with the lid shut.
    pub fn duration_secs(&self) -> u64 {
        self.opened_at
            .unwrap_or(self.last_seen)
            .saturating_sub(self.closed_at)
    }

    /// The sentence the owner reads when they open the lid.
    pub fn summary(&self) -> String {
        let mins = self.duration_secs() / 60;
        let secs = self.duration_secs() % 60;
        let dur = if mins > 0 {
            format!("{mins}m {secs}s")
        } else {
            format!("{secs}s")
        };
        let s = if self.sessions_at_close == 1 { "session" } else { "sessions" };
        let mut out = format!(
            "lid closed for {dur} with {} agent {s} still running",
            self.sessions_at_close
        );
        match self.vpn_held() {
            Some(true) => out.push_str("; the VPN held"),
            Some(false) => out.push_str("; the VPN DROPPED"),
            None => out.push_str("; no VPN state to report"),
        }
        match self.charge_cost() {
            Some(d) if d > 0 => out.push_str(&format!("; {d}% of battery")),
            Some(_) => out.push_str("; no battery used"),
            None => {}
        }
        if let Some(g) = self.ended_by {
            out.push_str(&format!("; ended by the {g} guard"));
        }
        out
    }
}

fn read_trim(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok().map(|s| s.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inputs(lid: LidState, work: Work, thermal: Thermal, charge: Charge) -> LidInputs {
        LidInputs { lid, work, thermal, charge }
    }

    fn cool() -> Thermal {
        Thermal::Celsius { c: 45.0 }
    }

    fn busy() -> Work {
        Work::Sessions { live: 2 }
    }

    fn quiet() -> Work {
        Work::Sessions { live: 0 }
    }

    // ── criterion 4: with no live work, a lid close suspends as it does today ─

    #[test]
    fn no_live_work_releases_the_inhibitor() {
        let p = LidPolicy::default();
        let d = p.decide(&inputs(LidState::Closed, quiet(), cool(), Charge::Ac));
        assert!(!d.holds_inhibitor(), "an idle machine must suspend on a lid close: {d:?}");
        assert!(
            d.why().contains("no agent session is running"),
            "the release must say why: {}",
            d.why()
        );
    }

    #[test]
    fn unreadable_work_counts_as_no_work_and_names_the_reason() {
        let p = LidPolicy::default();
        let d = p.decide(&inputs(
            LidState::Closed,
            Work::Unreadable { why: "apex-agentd socket is absent".to_string() },
            cool(),
            Charge::Ac,
        ));
        assert!(!d.holds_inhibitor(), "an unknown work state must not keep the machine up");
        assert!(
            d.why().contains("apex-agentd socket is absent"),
            "the reason must survive into the decision: {}",
            d.why()
        );
    }

    // ── criterion 1: live work keeps the machine up ──────────────────────────

    #[test]
    fn live_work_keeps_working_through_a_close() {
        let p = LidPolicy::default();
        let d = p.decide(&inputs(LidState::Closed, busy(), cool(), Charge::Ac));
        assert!(d.holds_inhibitor(), "two live sessions must hold the lid open: {d:?}");
        assert!(d.why().contains("2 live agent sessions"), "{}", d.why());
    }

    #[test]
    fn the_inhibitor_is_held_before_the_lid_ever_shuts() {
        // logind decides at the SW_LID edge. A policy that only answers
        // "keep working" once the lid is already closed has already lost the
        // race, so the open-lid arm must hold it too.
        let p = LidPolicy::default();
        let d = p.decide(&inputs(LidState::Open, busy(), cool(), Charge::Ac));
        assert!(d.holds_inhibitor(), "the inhibitor must be held with the lid open: {d:?}");
        assert!(d.why().contains("the lid is open"), "{}", d.why());
    }

    #[test]
    fn an_open_lid_never_fires_a_guard() {
        // A guard that suspended a machine the owner is typing on would be a
        // bug wearing a safety feature's clothes.
        let p = LidPolicy::default();
        let d = p.decide(&inputs(
            LidState::Open,
            busy(),
            Thermal::Celsius { c: 99.0 },
            Charge::Battery { percent: 3 },
        ));
        assert_eq!(d.guard(), None, "an open lid must not suspend the machine: {d:?}");
        assert!(d.holds_inhibitor());
    }

    // ── criterion 5: guards fire and name themselves ─────────────────────────

    #[test]
    fn the_thermal_guard_fires_at_the_ceiling_and_names_itself() {
        let p = LidPolicy::default();
        let d = p.decide(&inputs(
            LidState::Closed,
            busy(),
            Thermal::Celsius { c: 85.0 },
            Charge::Ac,
        ));
        assert_eq!(d.guard(), Some(Guard::Thermal), "{d:?}");
        assert!(d.suspends_now());
        assert!(d.why().contains("85.0 °C"), "{}", d.why());
    }

    #[test]
    fn one_degree_below_the_ceiling_keeps_working() {
        // The boundary in both directions, so `>=` cannot silently become `>`.
        let p = LidPolicy::default();
        let d = p.decide(&inputs(
            LidState::Closed,
            busy(),
            Thermal::Celsius { c: 84.9 },
            Charge::Ac,
        ));
        assert_eq!(d.guard(), None, "{d:?}");
        assert!(d.holds_inhibitor());
    }

    #[test]
    fn an_unreadable_temperature_fires_a_guard_of_its_own() {
        let p = LidPolicy::default();
        let d = p.decide(&inputs(
            LidState::Closed,
            busy(),
            Thermal::Unreadable { why: "class/hwmon has 0 entries".to_string() },
            Charge::Ac,
        ));
        assert_eq!(d.guard(), Some(Guard::ThermalUnreadable), "{d:?}");
        assert!(d.why().contains("class/hwmon has 0 entries"), "{}", d.why());
    }

    #[test]
    fn no_sensor_at_all_fires_unless_the_owner_accepted_it() {
        let p = LidPolicy::default();
        let d = p.decide(&inputs(LidState::Closed, busy(), Thermal::NoSensor, Charge::Ac));
        assert_eq!(d.guard(), Some(Guard::ThermalNoSensor), "{d:?}");

        let p = LidPolicy { require_thermal: false, ..LidPolicy::default() };
        let d = p.decide(&inputs(LidState::Closed, busy(), Thermal::NoSensor, Charge::Ac));
        assert_eq!(d.guard(), None, "an explicit opt-out must be honoured: {d:?}");
        assert!(d.holds_inhibitor());
    }

    #[test]
    fn the_battery_floor_fires_and_asks_for_a_checkpoint() {
        let p = LidPolicy::default();
        let d = p.decide(&inputs(
            LidState::Closed,
            busy(),
            cool(),
            Charge::Battery { percent: 20 },
        ));
        assert_eq!(d.guard(), Some(Guard::Battery), "{d:?}");
        assert!(d.guard().unwrap().checkpoint_first());
        assert!(d.why().contains("checkpointed"), "{}", d.why());
    }

    #[test]
    fn one_percent_above_the_floor_keeps_working() {
        let p = LidPolicy::default();
        let d = p.decide(&inputs(
            LidState::Closed,
            busy(),
            cool(),
            Charge::Battery { percent: 21 },
        ));
        assert_eq!(d.guard(), None, "{d:?}");
        assert!(d.why().contains("battery 21%"), "{}", d.why());
    }

    #[test]
    fn an_unreadable_charge_fires_a_guard_of_its_own() {
        let p = LidPolicy::default();
        let d = p.decide(&inputs(
            LidState::Closed,
            busy(),
            cool(),
            Charge::Unreadable { why: "BAT0 reported no capacity".to_string() },
        ));
        assert_eq!(d.guard(), Some(Guard::BatteryUnreadable), "{d:?}");
        assert!(d.why().contains("BAT0 reported no capacity"), "{}", d.why());
    }

    #[test]
    fn mains_power_never_fires_the_battery_guard() {
        let p = LidPolicy::default();
        let d = p.decide(&inputs(LidState::Closed, busy(), cool(), Charge::Ac));
        assert_eq!(d.guard(), None, "{d:?}");
    }

    #[test]
    fn thermal_is_checked_before_battery() {
        // Both would fire. The one that damages hardware must win, because the
        // report names exactly one guard and it should be the urgent one.
        let p = LidPolicy::default();
        let d = p.decide(&inputs(
            LidState::Closed,
            busy(),
            Thermal::Celsius { c: 95.0 },
            Charge::Battery { percent: 2 },
        ));
        assert_eq!(d.guard(), Some(Guard::Thermal), "{d:?}");
    }

    // ── criterion 6: the pin ─────────────────────────────────────────────────

    #[test]
    fn pin_on_keeps_working_with_nothing_running() {
        let p = LidPolicy { pin: Pin::On, ..LidPolicy::default() };
        let d = p.decide(&inputs(LidState::Closed, quiet(), cool(), Charge::Ac));
        assert!(d.holds_inhibitor(), "{d:?}");
        assert!(d.why().contains("pinned lid keep-working on"), "{}", d.why());
    }

    #[test]
    fn pin_on_does_not_disable_the_guards() {
        // The pin is an override of the *work* question, never of the safety
        // one. An owner cannot opt into cooking the machine.
        let p = LidPolicy { pin: Pin::On, ..LidPolicy::default() };
        let d = p.decide(&inputs(
            LidState::Closed,
            quiet(),
            Thermal::Celsius { c: 90.0 },
            Charge::Ac,
        ));
        assert_eq!(d.guard(), Some(Guard::Thermal), "a pin must not disable a guard: {d:?}");
    }

    #[test]
    fn pin_off_suspends_even_with_work_running() {
        let p = LidPolicy { pin: Pin::Off, ..LidPolicy::default() };
        let d = p.decide(&inputs(LidState::Closed, busy(), cool(), Charge::Ac));
        assert!(!d.holds_inhibitor(), "{d:?}");
        assert_eq!(d.guard(), None, "pin off releases; it does not fire a guard");
        assert!(d.why().contains("pinned lid keep-working off"), "{}", d.why());
    }

    #[test]
    fn disabled_is_inert() {
        let p = LidPolicy { enabled: false, pin: Pin::On, ..LidPolicy::default() };
        let d = p.decide(&inputs(LidState::Closed, busy(), cool(), Charge::Ac));
        assert!(!d.holds_inhibitor(), "{d:?}");
        assert!(d.why().contains("disabled"), "{}", d.why());
    }

    #[test]
    fn a_machine_with_no_lid_is_inert() {
        let p = LidPolicy { pin: Pin::On, ..LidPolicy::default() };
        let d = p.decide(&inputs(LidState::NoLid, busy(), cool(), Charge::Ac));
        assert!(!d.holds_inhibitor(), "{d:?}");
        assert!(d.why().contains("no lid"), "{}", d.why());
    }

    #[test]
    fn an_unreadable_lid_is_treated_as_closed_so_the_guards_apply() {
        let p = LidPolicy::default();
        let d = p.decide(&inputs(
            LidState::Unreadable { why: "UPower did not answer".to_string() },
            busy(),
            Thermal::Celsius { c: 90.0 },
            Charge::Ac,
        ));
        assert_eq!(d.guard(), Some(Guard::Thermal), "{d:?}");
        assert!(d.why().contains("UPower did not answer"), "{}", d.why());
    }

    // ── the whole matrix, so a reordering cannot pass unnoticed ──────────────

    #[test]
    fn every_input_combination_has_an_answer_and_the_safe_ones_are_safe() {
        let lids = [
            LidState::Open,
            LidState::Closed,
            LidState::NoLid,
            LidState::Unreadable { why: "x".into() },
        ];
        let works = [
            Work::Sessions { live: 0 },
            Work::Sessions { live: 1 },
            Work::Unreadable { why: "y".into() },
        ];
        let thermals = [
            Thermal::Celsius { c: 40.0 },
            Thermal::Celsius { c: 90.0 },
            Thermal::NoSensor,
            Thermal::Unreadable { why: "z".into() },
        ];
        let charges = [
            Charge::Ac,
            Charge::Battery { percent: 80 },
            Charge::Battery { percent: 5 },
            Charge::NoBattery,
            Charge::Unreadable { why: "w".into() },
        ];
        let pins = [Pin::Auto, Pin::On, Pin::Off];

        let mut n = 0;
        for lid in &lids {
            for work in &works {
                for thermal in &thermals {
                    for charge in &charges {
                        for pin in pins {
                            let p = LidPolicy { pin, ..LidPolicy::default() };
                            let i = inputs(
                                lid.clone(),
                                work.clone(),
                                thermal.clone(),
                                charge.clone(),
                            );
                            let d = p.decide(&i);
                            n += 1;
                            assert!(!d.why().is_empty(), "every decision explains itself: {i:?}");

                            // The invariant that makes this feature safe to
                            // ship: nothing holds the lid open on a hot,
                            // closed machine.
                            if d.holds_inhibitor() && lid.treat_as_closed() {
                                match thermal {
                                    Thermal::Celsius { c } => assert!(
                                        *c < p.thermal_ceiling_c,
                                        "held the lid open at {c} °C: {i:?}"
                                    ),
                                    Thermal::NoSensor => assert!(
                                        !p.require_thermal,
                                        "held the lid open with no sensor: {i:?}"
                                    ),
                                    Thermal::Unreadable { .. } => {
                                        panic!("held the lid open with an unreadable sensor: {i:?}")
                                    }
                                }
                                match charge {
                                    Charge::Battery { percent } => assert!(
                                        *percent > p.battery_floor_pct,
                                        "held the lid open at {percent}%: {i:?}"
                                    ),
                                    Charge::Unreadable { .. } => panic!(
                                        "held the lid open with an unreadable battery: {i:?}"
                                    ),
                                    _ => {}
                                }
                            }

                            // Pin off and disabled are absolute.
                            if pin == Pin::Off {
                                assert!(!d.holds_inhibitor(), "pin off held the lid: {i:?}");
                                assert_eq!(d.guard(), None, "pin off fired a guard: {i:?}");
                            }
                            if lid.is_absent() {
                                assert!(!d.holds_inhibitor(), "no lid held the lid: {i:?}");
                            }
                        }
                    }
                }
            }
        }
        assert_eq!(n, 4 * 3 * 4 * 5 * 3, "the matrix shrank");
    }

    // ── the power-down plan ──────────────────────────────────────────────────

    #[test]
    fn every_power_action_has_an_exact_inverse() {
        let plan = PowerPlan {
            actions: vec![
                PowerAction::Display { on: false },
                PowerAction::KeyboardBacklight {
                    path: "/sys/class/leds/kbd/brightness".into(),
                    value: 0,
                    prior: 2,
                },
                PowerAction::Bluetooth { blocked: true },
                PowerAction::WifiPowerSave { dev: "wlp1s0".into(), on: false, prior: Some(true) },
                PowerAction::Unit { unit: "x.timer".into(), user: false, start: false },
            ],
            skipped: Vec::new(),
        };
        let back = plan.restore();
        assert_eq!(back.actions.len(), plan.actions.len());
        // Reverse order: the units stopped last come back first.
        assert_eq!(
            back.actions[0],
            PowerAction::Unit { unit: "x.timer".into(), user: false, start: true }
        );
        assert_eq!(
            back.actions[3],
            PowerAction::KeyboardBacklight {
                path: "/sys/class/leds/kbd/brightness".into(),
                value: 2,
                prior: 0,
            },
            "the backlight must go back to the value it had, not to a default"
        );
        assert_eq!(back.actions[4], PowerAction::Display { on: true });
        // Applying the inverse twice is the identity.
        for a in &plan.actions {
            assert_eq!(&a.inverse().inverse(), a, "{a:?} is not its own double inverse");
        }
    }

    #[test]
    fn an_unknown_wifi_powersave_prior_restores_to_the_kernel_default() {
        let a = PowerAction::WifiPowerSave { dev: "wlp1s0".into(), on: false, prior: None };
        assert_eq!(
            a.inverse(),
            PowerAction::WifiPowerSave { dev: "wlp1s0".into(), on: true, prior: Some(false) }
        );
    }

    #[test]
    fn the_default_powerdown_list_names_nothing_interactive() {
        let d = PowerDown::default();
        let all: Vec<&String> =
            d.stop_user_units.iter().chain(d.stop_system_units.iter()).collect();
        assert!(!all.is_empty(), "a list that stops nothing satisfies no criterion");
        for u in &all {
            assert!(
                u.ends_with(".timer"),
                "{u} is not a timer; only periodic maintenance is discretionary by default"
            );
            for forbidden in ["apex-agentd", "apex-shell", "quickshell", "NetworkManager", "sing-box"] {
                assert!(
                    !u.contains(forbidden),
                    "{u} would stop something the work or the tunnel depends on"
                );
            }
        }
    }

    // ── the report ───────────────────────────────────────────────────────────

    fn period() -> ClosedPeriod {
        ClosedPeriod {
            closed_at: 1_000,
            last_seen: 2_800,
            opened_at: Some(2_800),
            sessions_at_close: 1,
            why: "1 live agent session".into(),
            ended_by: None,
            charge_at_close: Some(90),
            charge_last: Some(74),
            peak_c: Some(61.0),
            powered_down: vec!["display off".into()],
            skipped: Vec::new(),
            vpn: vec![
                VpnSample { at: 1_000, name: Some("school".into()), state: VpnState::Up },
                VpnSample { at: 2_800, name: Some("school".into()), state: VpnState::Up },
            ],
        }
    }

    #[test]
    fn a_held_vpn_is_reported_as_held() {
        let p = period();
        assert_eq!(p.vpn_held(), Some(true));
        assert_eq!(p.charge_cost(), Some(16));
        assert_eq!(p.duration_secs(), 1_800);
        let s = p.summary();
        assert!(s.contains("30m 0s"), "{s}");
        assert!(s.contains("the VPN held"), "{s}");
        assert!(s.contains("16% of battery"), "{s}");
    }

    #[test]
    fn one_dropped_sample_makes_the_whole_period_a_drop() {
        let mut p = period();
        p.vpn[1].state = VpnState::Down;
        assert_eq!(p.vpn_held(), Some(false));
        assert!(p.summary().contains("the VPN DROPPED"), "{}", p.summary());
    }

    #[test]
    fn an_unreadable_sample_is_not_a_held_tunnel() {
        // The entire reason this report exists: "we did not suspend, therefore
        // the VPN held" is the assumption it replaces.
        let mut p = period();
        p.vpn[1].state = VpnState::Unreadable { why: "nmcli exited 1".into() };
        assert_eq!(p.vpn_held(), None, "an unknown sample must not read as a held tunnel");
        assert!(p.summary().contains("no VPN state to report"), "{}", p.summary());
    }

    #[test]
    fn no_vpn_at_all_is_neither_pass_nor_fail() {
        let mut p = period();
        p.vpn = vec![VpnSample { at: 1_000, name: None, state: VpnState::None }];
        assert_eq!(p.vpn_held(), None);
    }

    #[test]
    fn a_guard_that_fired_appears_in_the_summary() {
        let mut p = period();
        p.ended_by = Some(Guard::Battery);
        assert!(p.summary().contains("ended by the battery guard"), "{}", p.summary());
    }

    #[test]
    fn a_period_still_running_measures_to_the_last_poll() {
        let mut p = period();
        p.opened_at = None;
        p.last_seen = 1_600;
        assert_eq!(p.duration_secs(), 600);
    }

    // ── readers against fixture trees ────────────────────────────────────────

    struct Tmp(std::path::PathBuf);
    impl Tmp {
        fn new(tag: &str) -> Tmp {
            let p = std::env::temp_dir().join(format!(
                "apex-lid-test-{tag}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0)
            ));
            std::fs::create_dir_all(&p).expect("temp dir");
            Tmp(p)
        }
        fn write(&self, rel: &str, body: &str) {
            let p = self.0.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).expect("mkdir");
            std::fs::write(p, body).expect("write");
        }
    }
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn thermal_read_separates_no_sensor_from_unreadable() {
        let t = Tmp::new("nosensor");
        // Nothing at all: no class directories.
        assert_eq!(Thermal::read(&t.0), Thermal::NoSensor);

        // A hwmon class with a chip that reports nothing usable is a FAILED
        // read, not an absent sensor.
        let t2 = Tmp::new("unreadable");
        t2.write("class/hwmon/hwmon0/name", "coretemp\n");
        match Thermal::read(&t2.0) {
            Thermal::Unreadable { why } => assert!(why.contains("class/hwmon"), "{why}"),
            other => panic!("expected Unreadable, got {other:?}"),
        }

        let t3 = Tmp::new("hot");
        t3.write("class/hwmon/hwmon0/name", "coretemp\n");
        t3.write("class/hwmon/hwmon0/temp1_input", "73500\n");
        assert_eq!(Thermal::read(&t3.0), Thermal::Celsius { c: 73.5 });
    }

    #[test]
    fn charge_read_distinguishes_mains_battery_and_a_silent_pack() {
        let t = Tmp::new("ac");
        t.write("class/power_supply/AC/type", "Mains\n");
        t.write("class/power_supply/AC/online", "1\n");
        t.write("class/power_supply/BAT0/type", "Battery\n");
        t.write("class/power_supply/BAT0/capacity", "44\n");
        assert_eq!(Charge::read(&t.0), Charge::Ac);

        let t2 = Tmp::new("bat");
        t2.write("class/power_supply/AC/type", "Mains\n");
        t2.write("class/power_supply/AC/online", "0\n");
        t2.write("class/power_supply/BAT0/type", "Battery\n");
        t2.write("class/power_supply/BAT0/capacity", "44\n");
        assert_eq!(Charge::read(&t2.0), Charge::Battery { percent: 44 });

        // A pack with no `capacity` but a usable energy pair.
        let t3 = Tmp::new("energy");
        t3.write("class/power_supply/BAT0/type", "Battery\n");
        t3.write("class/power_supply/BAT0/energy_now", "25000000\n");
        t3.write("class/power_supply/BAT0/energy_full", "50000000\n");
        assert_eq!(Charge::read(&t3.0), Charge::Battery { percent: 50 });

        // A pack that reports neither is UNREADABLE, never 100%.
        let t4 = Tmp::new("silent");
        t4.write("class/power_supply/BAT0/type", "Battery\n");
        t4.write("class/power_supply/BAT0/status", "Discharging\n");
        match Charge::read(&t4.0) {
            Charge::Unreadable { why } => assert!(why.contains("BAT0"), "{why}"),
            other => panic!("a silent battery must be Unreadable, got {other:?}"),
        }

        // No power_supply class at all — a container or a VM.
        let t5 = Tmp::new("none");
        assert_eq!(Charge::read(&t5.0), Charge::NoBattery);
    }

    #[test]
    fn pin_round_trips_through_its_spelling() {
        for p in [Pin::Auto, Pin::On, Pin::Off] {
            assert_eq!(Pin::parse(p.as_str()), Some(p));
        }
        assert_eq!(Pin::parse("maybe"), None);
    }

    #[test]
    fn a_policy_file_naming_one_key_keeps_the_rest() {
        // The LockPolicy trap: `#[serde(default)]` on the container so that a
        // config setting one field does not zero the others.
        let p: LidPolicy = toml::from_str("battery_floor_pct = 35\n").expect("parse");
        assert_eq!(p.battery_floor_pct, 35);
        assert!(p.enabled, "enabled was silently turned off");
        assert!(p.require_thermal, "require_thermal was silently turned off");
        assert_eq!(p.thermal_ceiling_c, 85.0);
        assert_eq!(p.pin, Pin::Auto);
        assert!(p.powerdown.display, "the powerdown block was zeroed");
        assert!(
            !p.powerdown.stop_system_units.is_empty(),
            "the default unit list was zeroed by an unrelated key"
        );
    }
}
