//! Workload-aware performance policy (roadmap §13): measure what the machine is
//! doing, map it to an intent, and say out loud how the conclusion was reached.
//!
//! §13 is unusually prescriptive about the *manner* of this feature, and those
//! sentences are the design:
//!
//! > Make automatic choices visible and overrideable.
//! > Do not market random tuning as AI optimization.
//! > Use measured workload signals and hardware capabilities.
//! > Provide conservative defaults and per-device testing.
//!
//! So three rules run through everything here.
//!
//! **Every signal reports its own provenance.** A [`Signal`] is either
//! `Measured` — carrying the value *and the path it was read from* — or
//! `Unavailable`, carrying the reason. There is no third state where a missing
//! reading quietly becomes a default. A machine with no PSI, no battery or no
//! readable VRAM says so, per signal, and `apex workload` prints it.
//!
//! **A process name alone never decides anything.** An editor holding a stale
//! `rustc` is not a build. Every classification that rests on process names
//! requires corroboration from an independently measured busy signal, and when
//! that corroboration cannot be read the answer falls back to
//! [`Workload::Unknown`] with the gap named. Guessing harder is exactly what
//! "do not market random tuning as AI optimization" prohibits.
//!
//! **Nothing here applies anything.** This module reads `/proc` and `/sys` and
//! returns a description. It performs no writes, spawns no process except
//! through the injected [`NvidiaSmi`] querier, and constructs no writer. The
//! decision to act on an assessment is the caller's, and in the CLI it is an
//! explicit user command.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::gpu::NvidiaSmi;
use crate::mode::{ModeId, PolicyIntent};

/// Where to read from. Explicit roots so the whole gatherer is fixture-testable,
/// exactly as [`CoreTopology::detect_from`](crate::topology::CoreTopology::detect_from)
/// already is.
#[derive(Debug, Clone)]
pub struct Roots {
    pub sys: PathBuf,
    pub proc: PathBuf,
    /// The cgroup apexd confines a game session to. Its `cgroup.procs` is the
    /// authoritative gaming signal: apexd put those PIDs there itself, so it
    /// beats any amount of guessing at process names.
    pub game_cgroup: PathBuf,
}

impl Default for Roots {
    fn default() -> Roots {
        Roots {
            sys: PathBuf::from("/sys"),
            proc: PathBuf::from("/proc"),
            game_cgroup: PathBuf::from("/sys/fs/cgroup/apex-game"),
        }
    }
}

impl Roots {
    /// The live machine.
    pub fn live() -> Roots {
        Roots::default()
    }
}

/// One reading, with its provenance.
///
/// The `source` is carried on BOTH arms on purpose: "unavailable" is far more
/// useful when it also says which path was looked at, because that is the
/// difference between "this kernel lacks PSI" and "the fixture is wrong".
#[derive(Debug, Clone, PartialEq)]
pub enum Signal<T> {
    Measured { value: T, source: String },
    Unavailable { reason: String, source: String },
}

impl<T> Signal<T> {
    pub fn measured(value: T, source: impl Into<String>) -> Signal<T> {
        Signal::Measured {
            value,
            source: source.into(),
        }
    }

    pub fn unavailable(reason: impl Into<String>, source: impl Into<String>) -> Signal<T> {
        Signal::Unavailable {
            reason: reason.into(),
            source: source.into(),
        }
    }

    /// The value, or `None` when the signal could not be read.
    pub fn value(&self) -> Option<&T> {
        match self {
            Signal::Measured { value, .. } => Some(value),
            Signal::Unavailable { .. } => None,
        }
    }

    pub fn is_measured(&self) -> bool {
        matches!(self, Signal::Measured { .. })
    }

    /// Where it came from, measured or not.
    pub fn source(&self) -> &str {
        match self {
            Signal::Measured { source, .. } | Signal::Unavailable { source, .. } => source,
        }
    }

    /// Why it is missing, or `None` when it is not.
    pub fn reason(&self) -> Option<&str> {
        match self {
            Signal::Unavailable { reason, .. } => Some(reason),
            Signal::Measured { .. } => None,
        }
    }
}

/// Processes matched against the classifier tables, by category.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProcessHits {
    pub compiler: BTreeSet<String>,
    pub llm: BTreeSet<String>,
    pub render: BTreeSet<String>,
    pub game: BTreeSet<String>,
    pub browser: BTreeSet<String>,
}

impl ProcessHits {
    pub fn is_empty(&self) -> bool {
        self.compiler.is_empty()
            && self.llm.is_empty()
            && self.render.is_empty()
            && self.game.is_empty()
            && self.browser.is_empty()
    }
}

/// GPU memory, as the driver reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Vram {
    pub used_bytes: u64,
    pub total_bytes: u64,
}

impl Vram {
    /// Fraction in use, 0.0-1.0. Zero when the driver reports no total, which
    /// is a driver quirk rather than an empty GPU.
    pub fn used_fraction(&self) -> f64 {
        if self.total_bytes == 0 {
            return 0.0;
        }
        self.used_bytes as f64 / self.total_bytes as f64
    }

    pub fn free_bytes(&self) -> u64 {
        self.total_bytes.saturating_sub(self.used_bytes)
    }
}

/// Everything gathered in one pass.
#[derive(Debug, Clone)]
pub struct Signals {
    pub on_ac: Signal<bool>,
    /// 1-minute load average.
    pub load1: Signal<f64>,
    pub cpus: Signal<usize>,
    /// PSI `some avg10` for CPU, as a percentage. This is a genuinely better
    /// "is the machine working" signal than load average, because it measures
    /// time tasks spent *waiting* rather than counting runnable tasks.
    pub cpu_pressure: Signal<f64>,
    pub io_pressure: Signal<f64>,
    /// How many PIDs apexd has in the game cgroup.
    pub game_session: Signal<usize>,
    pub processes: Signal<ProcessHits>,
    pub vram: Signal<Vram>,
}

impl Signals {
    /// Load per CPU, the scale-free form of the load average. `None` when
    /// either half is unavailable — a load of 8 means nothing without knowing
    /// whether the machine has 4 cores or 64.
    pub fn load_per_cpu(&self) -> Option<f64> {
        let load = *self.load1.value()?;
        let cpus = *self.cpus.value()?;
        if cpus == 0 {
            return None;
        }
        Some(load / cpus as f64)
    }

    /// Every signal that could not be read, rendered for display.
    pub fn gaps(&self) -> Vec<String> {
        let mut out = Vec::new();
        let mut note = |name: &str, s: Option<&str>, source: &str| {
            if let Some(reason) = s {
                out.push(format!("{name}: {reason} ({source})"));
            }
        };
        note("on AC", self.on_ac.reason(), self.on_ac.source());
        note("load average", self.load1.reason(), self.load1.source());
        note("cpu count", self.cpus.reason(), self.cpus.source());
        note(
            "cpu pressure",
            self.cpu_pressure.reason(),
            self.cpu_pressure.source(),
        );
        note(
            "io pressure",
            self.io_pressure.reason(),
            self.io_pressure.source(),
        );
        note(
            "game session",
            self.game_session.reason(),
            self.game_session.source(),
        );
        note("processes", self.processes.reason(), self.processes.source());
        note("vram", self.vram.reason(), self.vram.source());
        out
    }
}

// ── thresholds ───────────────────────────────────────────────────────────────
//
// Named, with the reasoning attached, because an unexplained magic number in a
// policy engine is indistinguishable from the "random tuning" §13 rejects.

/// Load per CPU at or above which the machine counts as genuinely working.
/// Half the cores busy is a real build or render, not a background indexer.
pub const BUSY_LOAD_PER_CPU: f64 = 0.5;

/// PSI `some avg10` (percent) at or above which tasks are measurably waiting
/// for CPU. PSI is the more direct signal: nonzero means somebody stalled.
pub const BUSY_PRESSURE_PCT: f64 = 5.0;

/// Load per CPU below which the machine counts as idle. The gap between this
/// and [`BUSY_LOAD_PER_CPU`] is deliberate — in between, the machine is neither
/// clearly busy nor clearly idle, and the honest answer is to say nothing.
pub const IDLE_LOAD_PER_CPU: f64 = 0.15;

/// VRAM fraction above which a resident model is corroborated.
pub const VRAM_COMMITTED: f64 = 0.5;

// ── the process classifier tables ────────────────────────────────────────────
//
// `/proc/<pid>/comm` is truncated to 15 characters by the kernel, so matching is
// prefix-aware (see `matches_comm`) rather than a plain equality test that would
// silently miss `HandBrakeCLI`-length names.

/// Toolchain processes. Deliberately the *workers*, not the wrappers: `cargo`
/// and `make` sit idle supervising, while `cc1plus` and `rustc` are the ones
/// actually burning cores.
const COMPILER: &[&str] = &[
    "cc1", "cc1plus", "cc1obj", "rustc", "clang", "clang++", "gcc", "g++", "ld", "ld.lld", "lld",
    "mold", "collect2", "javac", "kotlinc", "swiftc", "zig", "tsc", "esbuild", "ninja", "make",
    "cargo", "rustc-driver",
];

/// Local inference servers and runners.
const LLM: &[&str] = &[
    "ollama",
    "llama-server",
    "llama-cli",
    "llamafile",
    "vllm",
    "koboldcpp",
    "text-generation",
    "localai",
    "mlc_llm",
    "sglang",
    "lmstudio",
];

/// Render, encode and heavy media processing.
const RENDER: &[&str] = &[
    "blender",
    "ffmpeg",
    "kdenlive",
    "HandBrakeCLI",
    "darktable-cli",
    "resolve",
    "natron",
    "cycles",
    "obs",
    "Blender",
];

/// Game runtimes. `steam` is deliberately absent: it runs whenever the client is
/// open, which is most of the time on a gaming machine, so it would report a
/// game session that is not happening.
const GAME: &[&str] = &[
    "gamescope",
    "wineserver",
    "wine-preloader",
    "wine64-preload",
    "proton",
    "gamemoded",
];

/// Browsers, including the content-process names Firefox and Chromium use.
const BROWSER: &[&str] = &[
    "firefox",
    "chromium",
    "chrome",
    "brave",
    "librewolf",
    "epiphany",
    "Isolated Web Co",
    "Web Content",
    "WebKitWebProces",
];

/// Whether a (possibly truncated) `comm` names `full`.
///
/// The kernel truncates `comm` to 15 characters, so `HandBrakeCLI` survives but
/// a 20-character name does not. An equality test alone therefore misses every
/// long name, silently — which is the kind of bug that makes a classifier look
/// like it works right up until it matters.
pub fn matches_comm(comm: &str, full: &str) -> bool {
    if comm == full {
        return true;
    }
    // Only treat it as a truncation when the name really is at the cap.
    comm.len() >= 15 && full.starts_with(comm)
}

fn hit(comm: &str, table: &[&str]) -> bool {
    table.iter().any(|t| matches_comm(comm, t))
}

// ── gathering ────────────────────────────────────────────────────────────────

fn read_trim(path: &Path) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_string())
}

/// AC line state, from whichever power supply declares itself `Mains`.
///
/// No supply is named in code: a machine can call it `AC`, `ADP1`, `ACAD` or
/// `ucsi-source-psy-USBC000:001`, and the `type` attribute is the portable way
/// to tell a mains adapter from a battery.
pub fn read_on_ac(roots: &Roots) -> Signal<bool> {
    let base = roots.sys.join("class/power_supply");
    let src = base.display().to_string();
    let Ok(entries) = std::fs::read_dir(&base) else {
        return Signal::unavailable("no power_supply class on this machine", src);
    };
    let mut saw_mains = false;
    let mut online = false;
    for e in entries.flatten() {
        let p = e.path();
        if read_trim(&p.join("type")).as_deref() != Some("Mains") {
            continue;
        }
        saw_mains = true;
        if read_trim(&p.join("online")).as_deref() == Some("1") {
            online = true;
        }
    }
    if !saw_mains {
        // A desktop with no mains supply object is always on AC, but saying so
        // would be an inference. Report the gap and let the caller decide.
        return Signal::unavailable("no supply reports type=Mains", src);
    }
    Signal::measured(online, src)
}

/// 1-minute load average.
pub fn read_load1(roots: &Roots) -> Signal<f64> {
    let p = roots.proc.join("loadavg");
    let src = p.display().to_string();
    match read_trim(&p).and_then(|s| s.split_whitespace().next()?.parse::<f64>().ok()) {
        Some(v) => Signal::measured(v, src),
        None => Signal::unavailable("unreadable or malformed", src),
    }
}

/// PSI `some avg10` as a percentage, from `/proc/pressure/<what>`.
///
/// PSI needs `CONFIG_PSI=y` and, on some distributions, `psi=1` on the kernel
/// command line — so its absence is an ordinary state to report, not a fault.
pub fn read_pressure(roots: &Roots, what: &str) -> Signal<f64> {
    let p = roots.proc.join("pressure").join(what);
    let src = p.display().to_string();
    let Ok(text) = std::fs::read_to_string(&p) else {
        return Signal::unavailable(
            "no PSI on this kernel (needs CONFIG_PSI and psi=1)",
            src,
        );
    };
    for line in text.lines() {
        let Some(rest) = line.strip_prefix("some ") else {
            continue;
        };
        for field in rest.split_whitespace() {
            if let Some(v) = field.strip_prefix("avg10=") {
                if let Ok(v) = v.parse::<f64>() {
                    return Signal::measured(v, src);
                }
            }
        }
    }
    Signal::unavailable("no 'some avg10=' field", src)
}

/// How many PIDs apexd currently has confined to the game cgroup.
pub fn read_game_session(roots: &Roots) -> Signal<usize> {
    let p = roots.game_cgroup.join("cgroup.procs");
    let src = p.display().to_string();
    match std::fs::read_to_string(&p) {
        Ok(text) => Signal::measured(
            text.lines().filter(|l| !l.trim().is_empty()).count(),
            src,
        ),
        // The cgroup only exists while a session is running, so "absent" is the
        // ordinary no-game state rather than a missing capability.
        Err(_) => Signal::measured(0, src),
    }
}

/// The most processes to inspect in one pass. A bound, because an unbounded
/// walk of `/proc` on a machine under fork pressure is a way to make a
/// diagnostic command itself part of the problem.
const MAX_PROCESSES: usize = 8192;

/// Classify every running process by name.
pub fn read_processes(roots: &Roots) -> Signal<ProcessHits> {
    let src = roots.proc.join("<pid>/comm").display().to_string();
    let Ok(entries) = std::fs::read_dir(&roots.proc) else {
        return Signal::unavailable("cannot read /proc", src);
    };
    let mut hits = ProcessHits::default();
    let mut seen = 0usize;
    for e in entries.flatten() {
        if seen >= MAX_PROCESSES {
            break;
        }
        let name = e.file_name();
        let Some(name) = name.to_str() else { continue };
        if name.is_empty() || !name.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        seen += 1;
        // A process that exits mid-walk is normal, not an error.
        let Some(comm) = read_trim(&e.path().join("comm")) else {
            continue;
        };
        if comm.is_empty() {
            continue;
        }
        if hit(&comm, COMPILER) {
            hits.compiler.insert(comm.clone());
        }
        if hit(&comm, LLM) {
            hits.llm.insert(comm.clone());
        }
        if hit(&comm, RENDER) {
            hits.render.insert(comm.clone());
        }
        if hit(&comm, GAME) {
            hits.game.insert(comm.clone());
        }
        if hit(&comm, BROWSER) {
            hits.browser.insert(comm);
        }
    }
    Signal::measured(hits, src)
}

/// GPU memory, from the DRM device attributes the kernel exposes.
///
/// `mem_info_vram_{used,total}` is amdgpu's; it is read here rather than
/// special-cased because it is the only *portable-shaped* VRAM interface in
/// sysfs. Intel's i915/xe expose no equivalent total, and NVIDIA's proprietary
/// driver exposes none at all — which is why the NVIDIA leg goes through the
/// injected [`NvidiaSmi`] querier instead of a sysfs path that does not exist.
pub fn read_vram(roots: &Roots, smi: &dyn NvidiaSmi) -> Signal<Vram> {
    let base = roots.sys.join("class/drm");
    let src = base.join("card*/device/mem_info_vram_*").display().to_string();
    if let Ok(entries) = std::fs::read_dir(&base) {
        let mut cards: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .and_then(|s| s.to_str())
                    // `card1`, not `card1-eDP-1`: the connectors carry no memory.
                    .map(|s| s.starts_with("card") && !s.contains('-'))
                    .unwrap_or(false)
            })
            .collect();
        cards.sort();
        for card in cards {
            let dev = card.join("device");
            let used = read_trim(&dev.join("mem_info_vram_used")).and_then(|s| s.parse::<u64>().ok());
            let total =
                read_trim(&dev.join("mem_info_vram_total")).and_then(|s| s.parse::<u64>().ok());
            if let (Some(used_bytes), Some(total_bytes)) = (used, total) {
                return Signal::measured(
                    Vram {
                        used_bytes,
                        total_bytes,
                    },
                    dev.join("mem_info_vram_used").display().to_string(),
                );
            }
        }
    }

    // NVIDIA: no sysfs equivalent exists, so ask the querier. Behind the trait,
    // so a test injects a mock and never spawns anything.
    let vram = smi.vram_mib();
    if let Some((_, used_mib, total_mib)) = vram.first() {
        return Signal::measured(
            Vram {
                used_bytes: used_mib * 1024 * 1024,
                total_bytes: total_mib * 1024 * 1024,
            },
            "nvidia-smi --query-gpu=memory.used,memory.total",
        );
    }

    Signal::unavailable(
        "no driver on this machine reports VRAM (amdgpu exposes it in sysfs; \
         i915/xe expose no total; NVIDIA needs nvidia-smi)",
        src,
    )
}

/// Read every signal in one pass.
pub fn gather(roots: &Roots, smi: &dyn NvidiaSmi) -> Signals {
    let cpus = {
        let list = crate::topology::online_cpus(&roots.sys);
        let src = roots
            .sys
            .join("devices/system/cpu/online")
            .display()
            .to_string();
        if list.is_empty() {
            Signal::unavailable("no CPUs enumerable", src)
        } else {
            Signal::measured(list.len(), src)
        }
    };
    Signals {
        on_ac: read_on_ac(roots),
        load1: read_load1(roots),
        cpus,
        cpu_pressure: read_pressure(roots, "cpu"),
        io_pressure: read_pressure(roots, "io"),
        game_session: read_game_session(roots),
        processes: read_processes(roots),
        vram: read_vram(roots, smi),
    }
}

// ── classification ───────────────────────────────────────────────────────────

/// What the machine is measured to be doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Workload {
    Gaming,
    LocalLlm,
    Rendering,
    Compiling,
    Browsing,
    Idle,
    /// Not enough corroborated evidence. The conservative default, and a real
    /// answer rather than a failure: §13 asks for measured signals, and
    /// "nothing measurable is distinctive right now" is a measurement.
    Unknown,
}

impl Workload {
    pub const fn as_str(self) -> &'static str {
        match self {
            Workload::Gaming => "gaming",
            Workload::LocalLlm => "local-llm",
            Workload::Rendering => "rendering",
            Workload::Compiling => "compiling",
            Workload::Browsing => "browsing",
            Workload::Idle => "idle",
            Workload::Unknown => "unknown",
        }
    }

    /// The §13 intent this workload asks for, if any.
    pub const fn intent(self) -> Option<PolicyIntent> {
        match self {
            Workload::Gaming => Some(PolicyIntent::Latency),
            Workload::LocalLlm => Some(PolicyIntent::PreserveVram),
            Workload::Rendering => Some(PolicyIntent::Sustained),
            Workload::Compiling => Some(PolicyIntent::Throughput),
            Workload::Browsing | Workload::Idle => Some(PolicyIntent::LowPower),
            Workload::Unknown => None,
        }
    }

    /// The mode that serves this workload, before any battery constraint.
    pub const fn mode(self) -> Option<ModeId> {
        match self {
            Workload::Gaming => Some(ModeId::Gaming),
            Workload::LocalLlm => Some(ModeId::Ai),
            Workload::Rendering => Some(ModeId::Creator),
            Workload::Compiling => Some(ModeId::Development),
            Workload::Browsing | Workload::Idle => Some(ModeId::Daily),
            Workload::Unknown => None,
        }
    }
}

impl std::fmt::Display for Workload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The conclusion, with its reasoning attached.
///
/// `evidence` and `gaps` are not decoration. §13's "make automatic choices
/// visible" is only satisfiable if the choice arrives together with what drove
/// it and what could not be read, so both travel with the verdict rather than
/// being reconstructed by whoever prints it.
#[derive(Debug, Clone, PartialEq)]
pub struct Assessment {
    pub workload: Workload,
    pub intent: Option<PolicyIntent>,
    /// The mode this recommends, after any constraint. `None` means "not enough
    /// evidence — change nothing", which is a valid and common outcome.
    pub recommended: Option<ModeId>,
    /// Why, in the order the rules fired.
    pub evidence: Vec<String>,
    /// Signals that could not be read, and what that cost.
    pub gaps: Vec<String>,
}

/// Whether the machine is measurably working.
///
/// Two independent signals, either of which suffices: PSI (tasks actually
/// stalled on CPU) and load-per-CPU. `None` when neither could be read, which
/// is what makes the corroboration requirement enforceable instead of decorative.
fn busy(s: &Signals) -> Option<bool> {
    let pressure = s.cpu_pressure.value().map(|p| *p >= BUSY_PRESSURE_PCT);
    let load = s.load_per_cpu().map(|l| l >= BUSY_LOAD_PER_CPU);
    match (pressure, load) {
        (None, None) => None,
        (a, b) => Some(a.unwrap_or(false) || b.unwrap_or(false)),
    }
}

/// Classify, and explain.
///
/// The rules are a **ladder**, most authoritative first, in the same spirit as
/// the P/E-core detection in [`crate::topology`]:
///
/// 1. **A live game session.** apexd put those PIDs in the cgroup itself, so
///    this is first-party fact and needs no corroboration.
/// 2. **A game runtime process** (gamescope, wine). Weaker, so it is reported
///    as such.
/// 3. **A local inference server.** Named processes plus, where the driver
///    reports it, VRAM actually committed.
/// 4. **Rendering**, then 5. **compiling** — both require a busy signal, because
///    an editor holding a stale `rustc` is not a build.
/// 6. **Browsing** — browser processes and *not* busy.
/// 7. **Idle** — measurably below the idle threshold with nothing else running.
///
/// Anything else is [`Workload::Unknown`], and the gap is named.
pub fn assess(s: &Signals) -> Assessment {
    let mut evidence = Vec::new();
    let gaps = s.gaps();
    let hits = s.processes.value();
    let is_busy = busy(s);

    let workload = 'verdict: {
        // 1. First-party fact.
        if let Some(n) = s.game_session.value() {
            if *n > 0 {
                evidence.push(format!(
                    "{n} process(es) in apexd's game cgroup ({}) — a session is live",
                    s.game_session.source()
                ));
                break 'verdict Workload::Gaming;
            }
        }
        // 2. A game runtime, without a confined session.
        if let Some(h) = hits {
            if !h.game.is_empty() {
                evidence.push(format!(
                    "game runtime running: {} (no apexd game session is active, so this \
                     is a process-name match rather than first-party fact)",
                    h.game.iter().cloned().collect::<Vec<_>>().join(", ")
                ));
                break 'verdict Workload::Gaming;
            }
            // 3. Local inference.
            if !h.llm.is_empty() {
                evidence.push(format!(
                    "inference server running: {}",
                    h.llm.iter().cloned().collect::<Vec<_>>().join(", ")
                ));
                if let Some(v) = s.vram.value() {
                    evidence.push(format!(
                        "VRAM {:.0}% committed ({:.1} GiB of {:.1} GiB) — consistent with a resident model",
                        v.used_fraction() * 100.0,
                        v.used_bytes as f64 / (1024.0 * 1024.0 * 1024.0),
                        v.total_bytes as f64 / (1024.0 * 1024.0 * 1024.0),
                    ));
                }
                break 'verdict Workload::LocalLlm;
            }
            // 4/5. Corroborated by a busy signal, or not at all.
            for (set, name, verdict) in [
                (&h.render, "render/encode", Workload::Rendering),
                (&h.compiler, "toolchain", Workload::Compiling),
            ] {
                if set.is_empty() {
                    continue;
                }
                let listed = set.iter().cloned().collect::<Vec<_>>().join(", ");
                match is_busy {
                    Some(true) => {
                        evidence.push(format!("{name} processes running: {listed}"));
                        evidence.push(busy_line(s));
                        break 'verdict verdict;
                    }
                    Some(false) => {
                        evidence.push(format!(
                            "{name} processes are present ({listed}) but the machine is not \
                             measurably working — {}. Treated as not {}.",
                            busy_line(s),
                            verdict.as_str()
                        ));
                    }
                    None => {
                        evidence.push(format!(
                            "{name} processes are present ({listed}) but neither PSI nor the \
                             load average could be read, so nothing corroborates them. \
                             Reporting unknown rather than guessing."
                        ));
                        break 'verdict Workload::Unknown;
                    }
                }
            }
            // 6. Browsing: browsers, and demonstrably not busy.
            if !h.browser.is_empty() && is_busy == Some(false) {
                evidence.push(format!(
                    "browser running ({}) and the machine is not measurably working — {}",
                    h.browser.iter().cloned().collect::<Vec<_>>().join(", "),
                    busy_line(s)
                ));
                break 'verdict Workload::Browsing;
            }
        } else {
            evidence.push(
                "process list unreadable, so every name-based rule was skipped".to_string(),
            );
        }
        // 7. Idle.
        if let Some(l) = s.load_per_cpu() {
            if l < IDLE_LOAD_PER_CPU && hits.map(|h| h.is_empty()).unwrap_or(false) {
                evidence.push(format!(
                    "load {l:.2} per CPU is below the idle threshold of {IDLE_LOAD_PER_CPU:.2} \
                     and nothing distinctive is running"
                ));
                break 'verdict Workload::Idle;
            }
        }
        evidence
            .push("nothing measurable is distinctive right now; no policy change".to_string());
        Workload::Unknown
    };

    // The battery constraint. §13 lists "Battery -> efficiency" alongside the
    // workload rows, but it is a CONSTRAINT rather than a workload: it applies
    // on top of whatever the machine is doing.
    let mut recommended = workload.mode();
    if s.on_ac.value() == Some(&false) {
        if workload == Workload::Gaming {
            // Not overridden, and deliberately. A game session is an explicit
            // foreground activity the user started; silently unwinding it to
            // save power is exactly the kind of invisible automatic choice §13
            // says must not happen.
            evidence.push(
                "on battery, but a game session is running — left alone, because unwinding \
                 something the user started explicitly is not a decision to make silently"
                    .to_string(),
            );
        } else if workload != Workload::Unknown {
            evidence.push(
                "on battery: efficiency takes precedence over the workload's own intent"
                    .to_string(),
            );
            recommended = Some(ModeId::Battery);
        }
    }

    Assessment {
        workload,
        intent: workload.intent(),
        recommended,
        evidence,
        gaps,
    }
}

/// The measured busy signals, rendered for the evidence list.
fn busy_line(s: &Signals) -> String {
    let mut parts = Vec::new();
    if let Some(p) = s.cpu_pressure.value() {
        parts.push(format!("cpu pressure {p:.2}% (avg10)"));
    }
    if let Some(l) = s.load_per_cpu() {
        parts.push(format!("load {l:.2} per CPU"));
    }
    if parts.is_empty() {
        return "no busy signal available".to_string();
    }
    parts.join(", ")
}
