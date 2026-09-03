//! Named operating modes (roadmap §11), composed from primitives that already
//! exist.
//!
//! §11 is explicit that a mode is **not** another image: "avoid duplicating the
//! whole OS for every mode", "use `apexd` as a narrow policy/control plane". So
//! nothing here is a new hardware lever. A mode is a *named combination* of the
//! levers `apex tier`, `apex game` and apexd's AC/battery auto-switch already
//! expose, and the whole module is pure — it reads nothing, writes nothing, and
//! spawns nothing. Turning a [`Step`] into an effect is the CLI's job, and it
//! does it by calling the same frozen `org.apexos.Apexd1` methods a user could
//! type by hand.
//!
//! That purity is deliberate and load-bearing. This is the area that has already
//! caused real harm once: a test suite reached the host, switched the
//! developer's CPU scheduler and blocked for 177 seconds on a polkit password.
//! A module that cannot perform I/O cannot repeat it, and every rule below is
//! therefore unit-testable without a machine to break.
//!
//! ## What a mode may change, and what it only reports
//!
//! Executed, because each already has a tested restore path:
//!
//! * the power tier (`org.apexos.Apexd1.Power.SetTier`),
//! * apexd's AC/battery auto-switch (`SetAutoSwitch`),
//! * game mode — cpuset, IRQ steering, GPU clock locks, sched-ext
//!   (`org.apexos.Apexd1.GameMode.SetActive`).
//!
//! Reported only, deliberately: **service sets and system extensions**. §11
//! lists them as things a mode "may change", and they are modelled here so
//! `apex mode show` can state the full intent — but merging or unmerging a
//! sysext on a mode switch is a heavyweight, machine-breaking lever with its own
//! rebuild service, and `Containerfile.gaming` already masks `irqbalance`
//! permanently, so a mode that toggled it would fight the image. A declared gap
//! beats execution that silently does not happen.

use std::fmt;
use std::str::FromStr;

use crate::tier::Tier;

/// The eight modes §11 names, in the order it lists them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ModeId {
    Daily,
    Gaming,
    Development,
    Creator,
    Ai,
    Battery,
    Couch,
    Server,
}

impl ModeId {
    /// Every mode, in the roadmap's own order. This is the order `apex mode
    /// list` prints and the tie-break order [`identify`] uses.
    pub const ALL: [ModeId; 8] = [
        ModeId::Daily,
        ModeId::Gaming,
        ModeId::Development,
        ModeId::Creator,
        ModeId::Ai,
        ModeId::Battery,
        ModeId::Couch,
        ModeId::Server,
    ];

    /// The wire/CLI string ID.
    pub const fn as_str(self) -> &'static str {
        match self {
            ModeId::Daily => "daily",
            ModeId::Gaming => "gaming",
            ModeId::Development => "development",
            ModeId::Creator => "creator",
            ModeId::Ai => "ai",
            ModeId::Battery => "battery",
            ModeId::Couch => "couch",
            ModeId::Server => "server",
        }
    }

    /// The full definition.
    pub fn spec(self) -> &'static Mode {
        MODES
            .iter()
            .find(|m| m.id == self)
            .expect("every ModeId has exactly one entry in MODES")
    }

    /// All IDs as strings, for help text and completions.
    pub fn all_ids() -> Vec<String> {
        ModeId::ALL.iter().map(|m| m.as_str().to_string()).collect()
    }
}

impl fmt::Display for ModeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ModeId {
    type Err = UnknownMode;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // Case- and separator-insensitive: "AI", "Power-Saver"-style spellings
        // and "battery-saver" all reach the right mode rather than a refusal
        // the user has to guess their way out of.
        let norm = s.trim().to_ascii_lowercase().replace(['_', ' '], "-");
        let norm = norm.trim_matches('-');
        match norm {
            "daily" | "default" => Ok(ModeId::Daily),
            "gaming" | "game" => Ok(ModeId::Gaming),
            "development" | "dev" => Ok(ModeId::Development),
            "creator" | "creative" => Ok(ModeId::Creator),
            "ai" | "ml" | "ai-ml" => Ok(ModeId::Ai),
            "battery" | "battery-saver" | "saver" => Ok(ModeId::Battery),
            "couch" | "tv" => Ok(ModeId::Couch),
            "server" | "headless" => Ok(ModeId::Server),
            other => Err(UnknownMode(other.to_string())),
        }
    }
}

/// Error for an unrecognised mode ID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownMode(pub String);

impl fmt::Display for UnknownMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "unknown mode '{}' (expected one of: {})",
            self.0,
            ModeId::ALL
                .iter()
                .map(|m| m.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )
    }
}

impl std::error::Error for UnknownMode {}

/// The six workload intents §13 names. A mode declares the one it serves so
/// that `apex mode` and `apex workload` speak the same vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyIntent {
    /// Compiling: finish the batch, all cores, wall-clock over watts.
    Throughput,
    /// Gaming: latency and frame pacing.
    Latency,
    /// Battery: efficiency.
    Efficiency,
    /// Rendering: sustained CPU/GPU over a long run.
    Sustained,
    /// Local LLM: preserve VRAM for the model.
    PreserveVram,
    /// Browsing/idle: low-power background policy.
    LowPower,
}

impl PolicyIntent {
    pub const fn as_str(self) -> &'static str {
        match self {
            PolicyIntent::Throughput => "throughput",
            PolicyIntent::Latency => "latency",
            PolicyIntent::Efficiency => "efficiency",
            PolicyIntent::Sustained => "sustained",
            PolicyIntent::PreserveVram => "preserve-vram",
            PolicyIntent::LowPower => "low-power",
        }
    }

    /// One line on what the intent actually asks the machine to do.
    pub const fn describe(self) -> &'static str {
        match self {
            PolicyIntent::Throughput => "finish the work: every core, wall-clock over watts",
            PolicyIntent::Latency => "keep frame pacing even, at the cost of efficiency",
            PolicyIntent::Efficiency => "make the charge last",
            PolicyIntent::Sustained => "hold a long run without thermal collapse",
            PolicyIntent::PreserveVram => "leave VRAM to the model; the CPU is not the bottleneck",
            PolicyIntent::LowPower => "stay quiet and cool while nothing demanding is running",
        }
    }
}

impl fmt::Display for PolicyIntent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// How a mode decides the power tier.
///
/// Only two options, and that is a hardware-contract decision rather than a
/// simplification: apexd's frozen API can pin one tier (`SetTier`) or hand the
/// choice back to its own AC/battery auto-switch (`SetAutoSwitch`). A third
/// "performance on AC, balanced on battery" policy would need the daemon to
/// re-derive on every AC transition, which is a new daemon behaviour and a new
/// D-Bus member — so modes that want power-awareness use [`TierPolicy::Auto`]
/// and get the profile's own AC/battery defaults, which already express exactly
/// that.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TierPolicy {
    /// Leave apexd's auto-switch on; the profile's AC/battery defaults decide.
    Auto,
    /// Hold one tier, with auto-switch off so nothing re-derives it.
    Pinned(Tier),
}

/// What a mode wants of a systemd unit. **Reported, never executed** — see the
/// module docs for why.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceWant {
    Running,
    Stopped,
}

impl ServiceWant {
    pub const fn as_str(self) -> &'static str {
        match self {
            ServiceWant::Running => "running",
            ServiceWant::Stopped => "stopped",
        }
    }
}

/// One unit a mode would like in a given state, and why.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServiceIntent {
    pub unit: &'static str,
    pub want: ServiceWant,
    pub why: &'static str,
}

/// A mode definition. Static data: there is no per-machine variation here,
/// because the per-machine variation already lives in the sysprofile the tier
/// and game plans are built from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mode {
    pub id: ModeId,
    /// Human label, matching the roadmap's own wording.
    pub label: &'static str,
    /// One line describing when to pick it.
    pub summary: &'static str,
    pub tier: TierPolicy,
    /// Whether apexd's game mode (cpuset, IRQ steering, GPU clock locks,
    /// sched-ext) is held for the duration.
    pub game: bool,
    /// The §13 intent this mode serves, if it serves a single one.
    pub intent: Option<PolicyIntent>,
    /// Units the mode wants moved. Reported by `apex mode show`, not applied.
    pub services: &'static [ServiceIntent],
    /// System extensions the mode expects present. Reported, not merged.
    pub sysext: &'static [&'static str],
    /// Why the tier choice is what it is. Printed by `apex mode show`, because
    /// "performance" with no reason attached is indistinguishable from a guess.
    pub rationale: &'static str,
}

/// The catalogue.
///
/// Four of these (`development`, `creator`, `server`, and `gaming` minus its
/// game flag) resolve to the same *observable* state, and that is stated rather
/// than engineered around — see [`identify`]. They differ in intent and in the
/// service sets they declare, neither of which is readable back off a running
/// machine.
pub static MODES: [Mode; 8] = [
    Mode {
        id: ModeId::Daily,
        label: "Daily",
        summary: "General use. The machine decides, per the sysprofile.",
        tier: TierPolicy::Auto,
        game: false,
        intent: None,
        services: &[],
        sysext: &[],
        rationale: "Daily is the only mode that pins nothing. Auto-switch stays \
                    on so the profile's own AC and battery defaults apply, which \
                    is what the machine does before any mode is ever selected.",
    },
    Mode {
        id: ModeId::Gaming,
        label: "Gaming",
        summary: "Latency and frame pacing: P-core pinning, IRQ steering, sched-ext.",
        tier: TierPolicy::Pinned(Tier::Performance),
        game: true,
        intent: Some(PolicyIntent::Latency),
        services: &[ServiceIntent {
            unit: "irqbalance.service",
            want: ServiceWant::Stopped,
            why: "it re-scatters the interrupts game mode steers away from the game's cores",
        }],
        sysext: &[],
        rationale: "The only mode that turns game mode on, which is where the \
                    latency work actually lives: the cpuset, the IRQ steering, \
                    the GPU clock locks and the sched-ext switch. The tier is \
                    pinned as well so a battery transition cannot drop the \
                    governor mid-session.",
    },
    Mode {
        id: ModeId::Development,
        label: "Development",
        summary: "Compiling: every core, wall-clock over watts.",
        tier: TierPolicy::Pinned(Tier::Performance),
        game: false,
        intent: Some(PolicyIntent::Throughput),
        services: &[],
        sysext: &[],
        rationale: "A build is throughput-bound and finite: the fastest run is \
                    also the one that stops drawing power soonest. Game mode \
                    stays OFF — its cpuset confines work to the P-cores, which \
                    is the opposite of what a parallel build wants.",
    },
    Mode {
        id: ModeId::Creator,
        label: "Creator",
        summary: "Rendering and encoding: sustained CPU and GPU.",
        tier: TierPolicy::Pinned(Tier::Performance),
        game: false,
        intent: Some(PolicyIntent::Sustained),
        services: &[],
        sysext: &[],
        rationale: "Same pinned tier as Development, and deliberately so: APEX \
                    exposes three tiers, and there is no separate 'sustained' \
                    knob to set. The difference is intent, not a lever — which \
                    is why `apex mode status` reports the two as \
                    indistinguishable rather than pretending to tell them apart.",
    },
    Mode {
        id: ModeId::Ai,
        label: "AI / ML",
        summary: "Local models: leave VRAM and power to the GPU.",
        tier: TierPolicy::Pinned(Tier::Balanced),
        game: false,
        intent: Some(PolicyIntent::PreserveVram),
        services: &[],
        sysext: &[],
        rationale: "Balanced, not performance, and that is the measured call: \
                    local inference is GPU- and memory-bandwidth-bound, so \
                    pinning every CPU core to the performance governor spends \
                    package power the GPU wants without moving tokens/second. \
                    APEX cannot reserve VRAM — no kernel interface does — so \
                    the mode reports VRAM headroom instead of claiming to \
                    manage it.",
    },
    Mode {
        id: ModeId::Battery,
        label: "Battery Saver",
        summary: "Make the charge last.",
        tier: TierPolicy::Pinned(Tier::PowerSaver),
        game: false,
        intent: Some(PolicyIntent::Efficiency),
        services: &[],
        sysext: &[],
        rationale: "Pinned rather than automatic on purpose: auto-switch would \
                    put the machine back on the AC default the moment it is \
                    plugged in, which is not what someone who explicitly asked \
                    for Battery Saver meant.",
    },
    Mode {
        id: ModeId::Couch,
        label: "Couch",
        summary: "Big screen, controller, media playback.",
        tier: TierPolicy::Pinned(Tier::Balanced),
        game: false,
        intent: Some(PolicyIntent::LowPower),
        services: &[],
        sysext: &[],
        rationale: "Media playback is decoded in fixed-function hardware, so the \
                    CPU tier barely moves playback quality but does move fan \
                    noise — which matters more than usual two metres from a TV.",
    },
    Mode {
        id: ModeId::Server,
        label: "Server",
        summary: "Headless and always-on: sustained throughput, no idle drop.",
        tier: TierPolicy::Pinned(Tier::Performance),
        game: false,
        intent: Some(PolicyIntent::Throughput),
        services: &[],
        sysext: &[],
        rationale: "Pinned so a headless box never quietly drops to the battery \
                    default after an AC blip. Session-level concerns (suspend \
                    inhibition, autologin) belong to the session, not to a power \
                    policy, so this mode does not claim them.",
    },
];

/// The observable state a mode is matched against.
///
/// Everything here is readable back off a running machine through the frozen
/// D-Bus surface, which is what makes [`identify`] possible without persisting
/// the selected mode anywhere. Persisting it would have meant a root-owned state
/// file and therefore root on `apex mode set` — and `apex`'s root gating already
/// documents why a blanket root requirement is wrong for the verbs the desktop's
/// power controls drive as the session user.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModeState {
    pub tier: Tier,
    pub auto_switch: bool,
    pub game_active: bool,
}

/// One difference between a mode and the machine's observed state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diff {
    pub what: &'static str,
    pub expected: String,
    pub actual: String,
}

impl fmt::Display for Diff {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} is {} (mode wants {})", self.what, self.actual, self.expected)
    }
}

/// The result of matching observed state against the catalogue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModeMatch {
    /// Every mode whose observable state the machine matches exactly.
    ///
    /// A `Vec`, not an `Option`, and that is the honest shape: `development`,
    /// `creator` and `server` all pin the performance tier with game mode off,
    /// so a machine in that state is in all three as far as anything readable
    /// can tell. Collapsing them to one would be inventing certainty.
    pub exact: Vec<ModeId>,
    /// The nearest mode when nothing matches exactly.
    pub closest: ModeId,
    /// What differs from `closest`. Empty exactly when `exact` is non-empty.
    pub diffs: Vec<Diff>,
}

impl ModeMatch {
    /// True when the machine is not in any catalogued mode.
    pub fn is_custom(&self) -> bool {
        self.exact.is_empty()
    }
}

/// Everything that differs between `mode` and `state`.
pub fn diff(mode: &Mode, state: &ModeState) -> Vec<Diff> {
    let mut out = Vec::new();
    match mode.tier {
        TierPolicy::Auto => {
            if !state.auto_switch {
                out.push(Diff {
                    what: "auto-switch",
                    expected: "on".into(),
                    actual: "off".into(),
                });
            }
        }
        TierPolicy::Pinned(want) => {
            if state.auto_switch {
                out.push(Diff {
                    what: "auto-switch",
                    expected: "off".into(),
                    actual: "on".into(),
                });
            }
            if state.tier != want {
                out.push(Diff {
                    what: "tier",
                    expected: want.as_str().into(),
                    actual: state.tier.as_str().into(),
                });
            }
        }
    }
    if mode.game != state.game_active {
        out.push(Diff {
            what: "game mode",
            expected: if mode.game { "on".into() } else { "off".into() },
            actual: if state.game_active { "on".into() } else { "off".into() },
        });
    }
    out
}

/// Which mode(s) the machine is currently in.
///
/// Ties break toward the roadmap's own listing order, so an ambiguous match
/// reports `daily` before `server` rather than whichever the hash map happened
/// to yield.
pub fn identify(state: &ModeState) -> ModeMatch {
    let mut exact = Vec::new();
    let mut best: Option<(ModeId, Vec<Diff>)> = None;

    for id in ModeId::ALL {
        let d = diff(id.spec(), state);
        if d.is_empty() {
            exact.push(id);
        }
        let better = match &best {
            None => true,
            Some((_, bd)) => d.len() < bd.len(),
        };
        if better {
            best = Some((id, d));
        }
    }

    let (closest, diffs) = best.expect("ModeId::ALL is never empty");
    // `diffs` is emptied when something matched exactly, so the two halves can
    // never disagree: a caller that prints diffs whenever they are non-empty
    // would otherwise print differences against a mode the machine is in.
    let diffs = if exact.is_empty() { diffs } else { Vec::new() };
    ModeMatch {
        exact,
        closest,
        diffs,
    }
}

/// One action `apex mode set` will take, in the order it must be taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// `org.apexos.Apexd1.Power.SetAutoSwitch`.
    AutoSwitch(bool),
    /// `org.apexos.Apexd1.Power.SetTier`.
    SetTier(Tier),
    /// `org.apexos.Apexd1.GameMode.SetActive`.
    GameMode(bool),
}

impl Step {
    /// A stable, log-friendly rendering, matching `Action::describe`'s role on
    /// the hardware side.
    pub fn describe(&self) -> String {
        match self {
            Step::AutoSwitch(true) => {
                "auto-switch on (the profile's AC/battery defaults decide the tier)".into()
            }
            Step::AutoSwitch(false) => {
                "auto-switch off (so nothing re-derives the tier underneath us)".into()
            }
            Step::SetTier(t) => format!("tier -> {t}"),
            Step::GameMode(true) => {
                "game mode on (cpuset, IRQ steering, GPU clocks, sched-ext)".into()
            }
            Step::GameMode(false) => "game mode off (restoring what it changed)".into(),
        }
    }
}

/// The ordered steps that move `state` into `mode`.
///
/// **The order is the whole point**, and two of the four rules exist because
/// getting them wrong produces a mode that silently does not stick:
///
/// 1. **Leave game mode first.** `apex game stop` restores the tier that was
///    active before the session began. Setting the new mode's tier first and
///    then stopping game mode would have the restore overwrite it, so the user
///    asks for Battery Saver and lands wherever they were an hour ago.
/// 2. **Turn auto-switch off before pinning a tier.** With it on, apexd
///    re-derives the tier from the profile's AC/battery defaults, so a `SetTier`
///    can be undone moments later by an AC transition — or immediately, since
///    enabling auto-switch reconciles at once.
/// 3. Pin the tier.
/// 4. **Turn auto-switch on last** for [`TierPolicy::Auto`], because that call
///    reconciles immediately and is itself the tier change.
/// 5. Enter game mode last, once the tier it should run under is settled.
///
/// A machine already in `mode` yields an empty plan, so `apex mode set` is
/// idempotent and a no-op prints as one.
pub fn plan(mode: &Mode, state: &ModeState) -> Vec<Step> {
    let mut steps = Vec::new();

    // 1. Game mode off first — its restore path moves the tier.
    let leaving_game = state.game_active && !mode.game;
    if leaving_game {
        steps.push(Step::GameMode(false));
    }

    match mode.tier {
        TierPolicy::Pinned(want) => {
            // 2. Auto-switch off before the pin, never after.
            if state.auto_switch {
                steps.push(Step::AutoSwitch(false));
            }
            // 3. Pin. Re-asserted unconditionally when we just left game mode:
            //    the restore moved the tier to a value this plan cannot know,
            //    so trusting the pre-plan reading would skip a needed write.
            if state.tier != want || leaving_game {
                steps.push(Step::SetTier(want));
            }
        }
        TierPolicy::Auto => {
            // 4. Enabling auto-switch reconciles immediately, so it is both the
            //    switch and the tier change. No SetTier is emitted at all —
            //    naming one would fight the very thing being handed control.
            if !state.auto_switch {
                steps.push(Step::AutoSwitch(true));
            }
        }
    }

    // 5. Enter game mode once the tier underneath it is settled.
    if mode.game && !state.game_active {
        steps.push(Step::GameMode(true));
    }

    steps
}
