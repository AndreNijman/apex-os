//! Assertions for the workload-aware policy engine (roadmap §13).
//!
//! Every case runs against a fixture tree, never the machine: `Roots` carries an
//! explicit `/sys`, `/proc` and game-cgroup path, and the NVIDIA leg goes
//! through the injected querier, so nothing here reads the host's real state or
//! spawns `nvidia-smi`. No writer of any kind is constructed — this module is
//! read-only by construction.
//!
//! The assertions that matter most are the *negative* ones. §13 says "use
//! measured workload signals" and "do not market random tuning as AI
//! optimization", which in practice means an unmeasured or uncorroborated
//! reading must never become a confident verdict. That is what most of this
//! file is about.

use std::fs;
use std::path::{Path, PathBuf};

use apexd_core::gpu::MockNvidiaSmi;
use apexd_core::mode::{ModeId, PolicyIntent};
use apexd_core::workload::{
    assess, gather, matches_comm, read_on_ac, read_pressure, read_vram, Roots, Workload,
};

struct Fixture(PathBuf);

impl Fixture {
    fn new(tag: &str) -> Fixture {
        let root = std::env::temp_dir().join(format!(
            "apexd-workload-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        fs::remove_dir_all(&root).ok();
        fs::create_dir_all(&root).unwrap();
        Fixture(root)
    }

    fn write(&self, rel: &str, contents: &str) {
        let p = self.0.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, contents).unwrap();
    }

    fn roots(&self) -> Roots {
        Roots {
            sys: self.0.join("sys"),
            proc: self.0.join("proc"),
            game_cgroup: self.0.join("cgroup/apex-game"),
        }
    }

    /// A plausible 8-CPU laptop on AC, idle, with PSI present.
    fn baseline(&self) -> &Fixture {
        self.write("sys/devices/system/cpu/online", "0-7\n");
        self.write("proc/loadavg", "0.10 0.20 0.30 1/500 1234\n");
        self.write(
            "proc/pressure/cpu",
            "some avg10=0.00 avg60=0.00 avg300=0.00 total=13794091\n\
             full avg10=0.00 avg60=0.00 avg300=0.00 total=0\n",
        );
        self.write(
            "proc/pressure/io",
            "some avg10=0.00 avg60=0.00 avg300=0.00 total=1\n\
             full avg10=0.00 avg60=0.00 avg300=0.00 total=0\n",
        );
        self.write("sys/class/power_supply/ADP1/type", "Mains\n");
        self.write("sys/class/power_supply/ADP1/online", "1\n");
        self
    }

    /// Give the fixture a process with this `comm`.
    fn process(&self, pid: u32, comm: &str) -> &Fixture {
        self.write(&format!("proc/{pid}/comm"), &format!("{comm}\n"));
        self
    }

    fn busy(&self) -> &Fixture {
        self.write("proc/loadavg", "7.50 7.20 6.90 9/500 1234\n");
        self.write(
            "proc/pressure/cpu",
            "some avg10=42.00 avg60=38.00 avg300=30.00 total=1\n\
             full avg10=1.00 avg60=1.00 avg300=1.00 total=0\n",
        );
        self
    }

    fn on_battery(&self) -> &Fixture {
        self.write("sys/class/power_supply/ADP1/online", "0\n");
        self
    }

    /// An amdgpu-shaped VRAM report. `card1-eDP-1`-style connectors are written
    /// too, because the reader has to skip them.
    fn vram(&self, used: u64, total: u64) -> &Fixture {
        self.write(
            "sys/class/drm/card1/device/mem_info_vram_used",
            &format!("{used}\n"),
        );
        self.write(
            "sys/class/drm/card1/device/mem_info_vram_total",
            &format!("{total}\n"),
        );
        self.write("sys/class/drm/card1-eDP-1/status", "connected\n");
        self
    }

    fn game_session(&self, pids: &[u32]) -> &Fixture {
        let body: String = pids.iter().map(|p| format!("{p}\n")).collect();
        self.write("cgroup/apex-game/cgroup.procs", &body);
        self
    }
}

fn no_gpu() -> MockNvidiaSmi {
    MockNvidiaSmi::default()
}

// ── signal provenance ────────────────────────────────────────────────────────

#[test]
fn every_signal_carries_where_it_was_read_from() {
    // §13: "make automatic choices visible". A value with no provenance cannot
    // be checked by the person it is shown to.
    let f = Fixture::new("provenance");
    f.baseline().vram(1024, 4096);
    let s = gather(&f.roots(), &no_gpu());
    for (name, source) in [
        ("on_ac", s.on_ac.source()),
        ("load1", s.load1.source()),
        ("cpus", s.cpus.source()),
        ("cpu_pressure", s.cpu_pressure.source()),
        ("game_session", s.game_session.source()),
        ("processes", s.processes.source()),
        ("vram", s.vram.source()),
    ] {
        assert!(!source.is_empty(), "{name} reports no source");
        assert!(
            source.contains(&f.0.to_string_lossy().to_string()) || source.contains("nvidia-smi"),
            "{name} source {source:?} does not point into the fixture"
        );
    }
}

#[test]
fn a_missing_signal_says_so_instead_of_defaulting() {
    // The whole point of the Signal type. An empty fixture must produce
    // "unavailable, and here is why", not zeroes that read as measurements.
    let f = Fixture::new("empty");
    let s = gather(&f.roots(), &no_gpu());

    assert!(!s.on_ac.is_measured());
    assert!(!s.load1.is_measured());
    assert!(!s.cpus.is_measured());
    assert!(!s.cpu_pressure.is_measured());
    assert!(!s.vram.is_measured());
    assert_eq!(s.load_per_cpu(), None, "load per CPU needs both halves");

    // And each explains itself well enough to act on.
    for reason in [
        s.on_ac.reason().unwrap(),
        s.load1.reason().unwrap(),
        s.cpu_pressure.reason().unwrap(),
        s.vram.reason().unwrap(),
    ] {
        assert!(reason.len() > 10, "unhelpful reason: {reason:?}");
    }
    // PSI's absence names the kernel requirement rather than saying "error".
    assert!(s.cpu_pressure.reason().unwrap().contains("CONFIG_PSI"));

    let gaps = s.gaps();
    assert!(gaps.len() >= 5, "expected the gaps to be enumerated: {gaps:?}");
}

#[test]
fn no_battery_signal_is_reported_as_a_gap_not_as_on_battery() {
    // Defaulting a missing AC reading to `false` would drop a desktop into
    // Battery Saver forever. Defaulting it to `true` would ignore a real
    // laptop. Neither is acceptable, so it is a gap.
    let f = Fixture::new("nomains");
    f.baseline();
    fs::remove_dir_all(f.0.join("sys/class/power_supply")).unwrap();
    fs::create_dir_all(f.0.join("sys/class/power_supply")).unwrap();
    // A battery, but no mains adapter object at all.
    f.write("sys/class/power_supply/BAT0/type", "Battery\n");
    let sig = read_on_ac(&f.roots());
    assert!(!sig.is_measured());
    assert!(sig.reason().unwrap().contains("Mains"));
}

#[test]
fn the_mains_supply_is_found_by_type_never_by_name() {
    // A machine can call it AC, ADP1, ACAD or ucsi-source-psy-USBC000:001.
    for name in ["AC", "ADP1", "ACAD", "ucsi-source-psy-USBC000:001"] {
        let f = Fixture::new("mainsname");
        f.write(&format!("sys/class/power_supply/{name}/type"), "Mains\n");
        f.write(&format!("sys/class/power_supply/{name}/online"), "1\n");
        assert_eq!(
            read_on_ac(&f.roots()).value(),
            Some(&true),
            "{name} was not recognised as a mains supply"
        );
    }
}

#[test]
fn psi_is_parsed_from_the_some_line_not_the_full_line() {
    // `full` counts time when EVERY task stalled, which on a desktop is almost
    // always zero. Reading it instead of `some` makes the busy signal inert.
    let f = Fixture::new("psi");
    f.write(
        "proc/pressure/cpu",
        "some avg10=17.25 avg60=9.00 avg300=3.00 total=1\n\
         full avg10=0.00 avg60=0.00 avg300=0.00 total=0\n",
    );
    assert_eq!(read_pressure(&f.roots(), "cpu").value(), Some(&17.25));
}

#[test]
fn vram_skips_the_drm_connectors_and_reads_the_card() {
    let f = Fixture::new("vram");
    f.vram(2 * 1024 * 1024 * 1024, 8 * 1024 * 1024 * 1024);
    let v = read_vram(&f.roots(), &no_gpu());
    let v = v.value().expect("amdgpu-shaped sysfs is readable");
    assert_eq!(v.used_fraction(), 0.25);
    assert_eq!(v.free_bytes(), 6 * 1024 * 1024 * 1024);
}

#[test]
fn nvidia_vram_comes_from_the_injected_querier_never_a_spawn() {
    // There is no sysfs VRAM total on the proprietary driver, so this leg has
    // to ask nvidia-smi. Going through the trait is what keeps a test off the
    // host: a mock answers, and no process is ever started.
    let f = Fixture::new("nvvram");
    let smi = MockNvidiaSmi {
        available: true,
        vram: vec![(0, 2048, 8192)],
        ..Default::default()
    };
    let v = read_vram(&f.roots(), &smi);
    let got = v.value().expect("the querier reported memory");
    assert_eq!(got.total_bytes, 8192 * 1024 * 1024);
    assert_eq!(got.used_fraction(), 0.25);
    assert!(v.source().contains("nvidia-smi"));

    // An unavailable querier must produce a gap, not zeroes.
    let absent = MockNvidiaSmi {
        available: false,
        vram: vec![(0, 2048, 8192)],
        ..Default::default()
    };
    assert!(!read_vram(&f.roots(), &absent).is_measured());
}

#[test]
fn a_truncated_comm_still_matches_its_full_name() {
    // The kernel caps comm at 15 characters. A plain equality test silently
    // misses every longer name, which is the kind of bug that makes a
    // classifier look fine until the one case that matters.
    assert!(matches_comm("text-generation", "text-generation"));
    assert!(matches_comm("HandBrakeCLI", "HandBrakeCLI"));
    // 15 chars of a longer name is treated as a truncation.
    assert!(matches_comm("Isolated Web Co", "Isolated Web Content"));
    // A SHORT name that merely prefixes a longer one must NOT match, or `ld`
    // would match `ldconfig` and every login would look like a link step.
    assert!(!matches_comm("ld", "ld.lld"));
    assert!(!matches_comm("cargo", "cargo-something"));
}

// ── classification: the ladder ───────────────────────────────────────────────

#[test]
fn a_live_game_cgroup_is_first_party_fact_and_wins() {
    // apexd put those PIDs there itself. No process-name guessing can outrank
    // it, and it needs no corroboration.
    let f = Fixture::new("gamecg");
    f.baseline().game_session(&[4242, 4243]);
    let a = assess(&gather(&f.roots(), &no_gpu()));
    assert_eq!(a.workload, Workload::Gaming);
    assert_eq!(a.intent, Some(PolicyIntent::Latency));
    assert_eq!(a.recommended, Some(ModeId::Gaming));
    assert!(
        a.evidence.iter().any(|e| e.contains("game cgroup")),
        "{:?}",
        a.evidence
    );
}

#[test]
fn a_game_runtime_without_a_session_is_reported_as_the_weaker_signal() {
    let f = Fixture::new("gameproc");
    f.baseline().process(10, "gamescope");
    let a = assess(&gather(&f.roots(), &no_gpu()));
    assert_eq!(a.workload, Workload::Gaming);
    // It must say that this is a name match, not first-party fact.
    assert!(
        a.evidence
            .iter()
            .any(|e| e.contains("process-name match")),
        "{:?}",
        a.evidence
    );
}

#[test]
fn steam_running_is_not_a_game_session() {
    // Steam is open most of the time on a gaming machine. Treating it as a
    // game would put the machine in Gaming mode permanently.
    let f = Fixture::new("steam");
    f.baseline().process(10, "steam").process(11, "steamwebhelper");
    let a = assess(&gather(&f.roots(), &no_gpu()));
    assert_ne!(a.workload, Workload::Gaming);
}

#[test]
fn an_inference_server_maps_to_the_ai_mode_and_reports_vram() {
    let f = Fixture::new("llm");
    f.baseline()
        .process(20, "ollama")
        .vram(6 * 1024 * 1024 * 1024, 8 * 1024 * 1024 * 1024);
    let a = assess(&gather(&f.roots(), &no_gpu()));
    assert_eq!(a.workload, Workload::LocalLlm);
    assert_eq!(a.intent, Some(PolicyIntent::PreserveVram));
    assert_eq!(a.recommended, Some(ModeId::Ai));
    assert!(
        a.evidence.iter().any(|e| e.contains("VRAM") && e.contains("75%")),
        "{:?}",
        a.evidence
    );
}

#[test]
fn a_busy_toolchain_is_compiling_and_a_quiet_one_is_not() {
    // THE central negative case. An editor holding a stale rustc must not put
    // the machine into a performance tier.
    let quiet = Fixture::new("quietbuild");
    quiet.baseline().process(30, "rustc");
    let a = assess(&gather(&quiet.roots(), &no_gpu()));
    assert_ne!(
        a.workload,
        Workload::Compiling,
        "a present-but-idle toolchain must not read as a build: {:?}",
        a.evidence
    );
    assert!(
        a.evidence
            .iter()
            .any(|e| e.contains("not measurably working")),
        "{:?}",
        a.evidence
    );

    let busy = Fixture::new("busybuild");
    busy.baseline().busy().process(30, "cc1plus");
    let a = assess(&gather(&busy.roots(), &no_gpu()));
    assert_eq!(a.workload, Workload::Compiling);
    assert_eq!(a.intent, Some(PolicyIntent::Throughput));
    assert_eq!(a.recommended, Some(ModeId::Development));
    // The corroborating measurement must appear in the reasoning.
    assert!(
        a.evidence.iter().any(|e| e.contains("cpu pressure")),
        "{:?}",
        a.evidence
    );
}

#[test]
fn an_uncorroborated_name_match_reports_unknown_rather_than_guessing() {
    // No PSI and no loadavg: the toolchain processes are real but nothing
    // independent says the machine is working. §13's "use measured workload
    // signals" means the honest answer is "I do not know".
    let f = Fixture::new("nocorroboration");
    f.write("sys/devices/system/cpu/online", "0-7\n");
    f.write("sys/class/power_supply/ADP1/type", "Mains\n");
    f.write("sys/class/power_supply/ADP1/online", "1\n");
    f.process(30, "cc1plus");
    let s = gather(&f.roots(), &no_gpu());
    assert!(!s.cpu_pressure.is_measured() && !s.load1.is_measured());
    let a = assess(&s);
    assert_eq!(a.workload, Workload::Unknown);
    assert_eq!(a.recommended, None, "an unknown workload changes nothing");
    assert!(
        a.evidence
            .iter()
            .any(|e| e.contains("nothing corroborates")),
        "{:?}",
        a.evidence
    );
}

#[test]
fn rendering_outranks_compiling_when_both_are_present() {
    // ffmpeg invoked from a build is still a render as far as the policy goes;
    // the ordering is documented so the outcome is not accidental.
    let f = Fixture::new("both");
    f.baseline().busy().process(40, "blender").process(41, "rustc");
    let a = assess(&gather(&f.roots(), &no_gpu()));
    assert_eq!(a.workload, Workload::Rendering);
    assert_eq!(a.recommended, Some(ModeId::Creator));
}

#[test]
fn a_quiet_browser_is_browsing_and_a_busy_one_is_not() {
    let f = Fixture::new("browse");
    f.baseline().process(50, "firefox");
    let a = assess(&gather(&f.roots(), &no_gpu()));
    assert_eq!(a.workload, Workload::Browsing);
    assert_eq!(a.intent, Some(PolicyIntent::LowPower));
    assert_eq!(a.recommended, Some(ModeId::Daily));

    // A browser pegging the CPU (a WebGL page, a video call) is not "low-power
    // background policy", so it must not be classified as such.
    let hot = Fixture::new("browsebusy");
    hot.baseline().busy().process(50, "firefox");
    assert_ne!(assess(&gather(&hot.roots(), &no_gpu())).workload, Workload::Browsing);
}

#[test]
fn a_genuinely_idle_machine_is_idle() {
    let f = Fixture::new("idle");
    f.baseline();
    let a = assess(&gather(&f.roots(), &no_gpu()));
    assert_eq!(a.workload, Workload::Idle);
    assert_eq!(a.recommended, Some(ModeId::Daily));
}

// ── the battery constraint ───────────────────────────────────────────────────

#[test]
fn on_battery_efficiency_takes_precedence_and_says_so() {
    let f = Fixture::new("batcompile");
    f.baseline().busy().on_battery().process(30, "cc1plus");
    let a = assess(&gather(&f.roots(), &no_gpu()));
    // The measured workload is unchanged — only the recommendation moves.
    assert_eq!(a.workload, Workload::Compiling);
    assert_eq!(a.intent, Some(PolicyIntent::Throughput));
    assert_eq!(a.recommended, Some(ModeId::Battery));
    assert!(
        a.evidence.iter().any(|e| e.contains("on battery")),
        "{:?}",
        a.evidence
    );
}

#[test]
fn a_game_session_on_battery_is_not_silently_unwound() {
    // Dropping a running game to Battery Saver behind the user's back is
    // exactly the invisible automatic choice §13 prohibits.
    let f = Fixture::new("batgame");
    f.baseline().on_battery().game_session(&[4242]);
    let a = assess(&gather(&f.roots(), &no_gpu()));
    assert_eq!(a.recommended, Some(ModeId::Gaming));
    assert!(
        a.evidence
            .iter()
            .any(|e| e.contains("left alone")),
        "{:?}",
        a.evidence
    );
}

#[test]
fn an_unknown_workload_on_battery_still_recommends_nothing() {
    // The constraint must not manufacture a recommendation out of a
    // non-verdict; "change nothing" has to survive being on battery.
    let f = Fixture::new("batunknown");
    f.write("sys/devices/system/cpu/online", "0-7\n");
    f.write("sys/class/power_supply/ADP1/type", "Mains\n");
    f.write("sys/class/power_supply/ADP1/online", "0\n");
    f.process(30, "cc1plus");
    let a = assess(&gather(&f.roots(), &no_gpu()));
    assert_eq!(a.workload, Workload::Unknown);
    assert_eq!(a.recommended, None);
}

// ── the shape of the report ──────────────────────────────────────────────────

#[test]
fn every_verdict_arrives_with_reasoning() {
    // §13: automatic choices must be VISIBLE. A verdict with an empty evidence
    // list is not.
    for (tag, build) in [
        ("idle", 0u8),
        ("compile", 1),
        ("game", 2),
        ("llm", 3),
        ("empty", 4),
    ] {
        let f = Fixture::new(tag);
        match build {
            0 => {
                f.baseline();
            }
            1 => {
                f.baseline().busy().process(1, "cc1plus");
            }
            2 => {
                f.baseline().game_session(&[1]);
            }
            3 => {
                f.baseline().process(1, "ollama");
            }
            _ => {}
        }
        let a = assess(&gather(&f.roots(), &no_gpu()));
        assert!(!a.evidence.is_empty(), "{tag}: no evidence at all");
        for e in &a.evidence {
            assert!(e.len() > 15, "{tag}: threadbare evidence line {e:?}");
        }
    }
}

#[test]
fn the_intent_vocabulary_matches_what_the_modes_declare() {
    // `apex workload` and `apex mode` must speak the same language, or the
    // recommendation cannot be acted on. Every workload with a recommended mode
    // must agree with that mode's declared intent.
    for w in [
        Workload::Gaming,
        Workload::LocalLlm,
        Workload::Rendering,
        Workload::Compiling,
    ] {
        let mode = w.mode().expect("these four all map to a mode");
        assert_eq!(
            mode.spec().intent,
            w.intent(),
            "{w} recommends {mode}, whose declared intent disagrees"
        );
    }
    // Unknown deliberately maps to nothing at all.
    assert_eq!(Workload::Unknown.mode(), None);
    assert_eq!(Workload::Unknown.intent(), None);
}

#[test]
fn the_process_walk_is_bounded_and_survives_junk() {
    // /proc holds non-numeric entries and processes that exit mid-walk. Neither
    // may panic or abort the pass.
    let f = Fixture::new("junk");
    f.baseline();
    f.write("proc/self/comm", "bash\n");
    f.write("proc/cpuinfo", "processor : 0\n");
    fs::create_dir_all(f.0.join("proc/99999")).unwrap(); // a pid dir with no comm
    f.process(1, "systemd");
    let s = gather(&f.roots(), &no_gpu());
    assert!(s.processes.is_measured(), "the walk must complete");
}

#[test]
fn an_unreadable_proc_disables_the_name_rules_rather_than_the_whole_pass() {
    let f = Fixture::new("noproc");
    f.write("sys/devices/system/cpu/online", "0-7\n");
    let roots = Roots {
        proc: Path::new("/nonexistent/apexd-workload-proc").to_path_buf(),
        ..f.roots()
    };
    let s = gather(&roots, &no_gpu());
    assert!(!s.processes.is_measured());
    let a = assess(&s);
    assert_eq!(a.workload, Workload::Unknown);
    assert!(
        a.evidence.iter().any(|e| e.contains("process list unreadable")),
        "{:?}",
        a.evidence
    );
}
