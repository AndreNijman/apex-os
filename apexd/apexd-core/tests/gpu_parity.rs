//! GPU parity across AMD, Intel and NVIDIA (roadmap P1-043).
//!
//! ## The fixtures are two real machines
//!
//! Every path, every value and every absence below was read off hardware on
//! 2026-09-07 rather than invented:
//!
//! * **ThinkPad L16** — one card, `0x1002` HawkPoint1 (Radeon 780M) on amdgpu,
//!   `boot_vga`. It publishes `gpu_busy_percent`, a three-level `pp_dpm_sclk`
//!   with the 800 MHz level active, `power_dpm_force_performance_level = auto`,
//!   and no `pp_power_profile_mode` at all.
//! * **MSI Katana GF76** — `card1` is `0x8086` Alder Lake-P Iris Xe on i915,
//!   `boot_vga=1`, with `gt_min_freq_mhz=100`, `gt_max_freq_mhz=1400`,
//!   `gt_RPn=100`, `gt_RP1=350`, `gt_RP0=1400` and `gt_cur_freq_mhz=0` (RC6);
//!   `card2` is `0x10de` GA104M (RTX 3070 Mobile) on the proprietary driver,
//!   with NO `gpu_busy_percent`, no `pp_dpm_*` and no `power_dpm_*` — nothing
//!   in sysfs at all.
//!
//! `docs/p3-progress.md` said closing this row "needs hardware nobody here
//! has". All three vendors were on the two machines the whole time.
//!
//! ## What is demonstrated and what is reasoned
//!
//! Discovery, the vendor split, the planner and the restore are demonstrated
//! against these fixtures and against a writer that really writes them. What is
//! NOT demonstrated is the effect of a write on a live card: nothing here runs
//! against `/sys`, because the AMD machine is the developer's workstation and
//! the Intel/NVIDIA machine was running a game.

use std::fs;
use std::path::PathBuf;

use apexd_core::gpu::{
    self, discover, intel_floor_mhz, plan_sysfs_enter, plan_sysfs_exit, primary, read_prior,
    GpuVendor, MockNvidiaSmi,
};
use apexd_core::perf::read_gpu_devices;
use apexd_core::profile::SysfsGpuConfig;
use apexd_core::syswriter::{RealWriter, SysWriter};
use apexd_core::tier::Action;

struct Fixture(PathBuf);

impl Fixture {
    fn new(tag: &str) -> Fixture {
        let root = std::env::temp_dir().join(format!(
            "apexd-gpu-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        fs::remove_dir_all(&root).ok();
        fs::create_dir_all(&root).unwrap();
        Fixture(root)
    }

    fn write(&self, rel: &str, contents: &str) -> &Fixture {
        let p = self.0.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, contents).unwrap();
        self
    }

    fn link(&self, rel: &str, target: &str) -> &Fixture {
        let p = self.0.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(target, p).unwrap();
        self
    }

    fn sys(&self) -> PathBuf {
        self.0.join("sys")
    }

    fn read(&self, rel: &str) -> String {
        fs::read_to_string(self.0.join(rel)).unwrap().trim().into()
    }
}

/// The L16, as measured.
fn l16(tag: &str) -> Fixture {
    let f = Fixture::new(tag);
    f.write("sys/class/drm/card1/device/vendor", "0x1002\n")
        .write("sys/class/drm/card1/device/boot_vga", "1\n")
        .write("sys/class/drm/card1/device/gpu_busy_percent", "37\n")
        .write(
            "sys/class/drm/card1/device/pp_dpm_sclk",
            "0: 800Mhz *\n1: 1100Mhz \n2: 2700Mhz \n",
        )
        .write(
            "sys/class/drm/card1/device/power_dpm_force_performance_level",
            "auto\n",
        )
        .write("sys/class/drm/card1/device/power_dpm_state", "performance\n")
        // A connector node, which must never be mistaken for a card.
        .write("sys/class/drm/card1-eDP-1/status", "connected\n");
    f.link("sys/class/drm/card1/device/driver", "../../../bus/pci/drivers/amdgpu");
    f
}

/// The Katana, as measured: Intel iGPU on card1, NVIDIA dGPU on card2.
fn katana(tag: &str) -> Fixture {
    let f = Fixture::new(tag);
    f.write("sys/class/drm/card1/device/vendor", "0x8086\n")
        .write("sys/class/drm/card1/device/boot_vga", "1\n")
        .write("sys/class/drm/card1/gt_cur_freq_mhz", "0\n")
        .write("sys/class/drm/card1/gt_act_freq_mhz", "0\n")
        .write("sys/class/drm/card1/gt_min_freq_mhz", "100\n")
        .write("sys/class/drm/card1/gt_max_freq_mhz", "1400\n")
        .write("sys/class/drm/card1/gt_boost_freq_mhz", "1400\n")
        .write("sys/class/drm/card1/gt_RPn_freq_mhz", "100\n")
        .write("sys/class/drm/card1/gt_RP1_freq_mhz", "350\n")
        .write("sys/class/drm/card1/gt_RP0_freq_mhz", "1400\n")
        // card2 has a vendor and a driver and nothing else. That is the point.
        .write("sys/class/drm/card2/device/vendor", "0x10de\n")
        .write("sys/class/drm/card2/device/boot_vga", "0\n");
    f.link("sys/class/drm/card1/device/driver", "../../../bus/pci/drivers/i915");
    f.link("sys/class/drm/card2/device/driver", "../../../bus/pci/drivers/nvidia");
    f
}

fn rtx3070() -> MockNvidiaSmi {
    MockNvidiaSmi {
        available: true,
        gpus: vec![gpu::NvidiaGpu {
            index: 0,
            name: "NVIDIA GeForce RTX 3070 Laptop GPU".into(),
            max_graphics_mhz: Some(2100),
            max_memory_mhz: Some(6001),
            persistence: Some(false),
        }],
        vram: vec![(0, 1024, 8192)],
        clocks: vec![(0, 1710, 6001)],
        util: vec![(0, 64.0)],
    }
}

// ── discovery ───────────────────────────────────────────────────────────────

#[test]
fn amd_workstation_is_one_card_with_a_dpm_knob() {
    let f = l16("amd-discover");
    let d = discover(&f.sys());
    assert_eq!(d.len(), 1, "a connector node was counted as a card");
    assert_eq!(d[0].card, "card1");
    assert_eq!(d[0].vendor, GpuVendor::Amd);
    assert_eq!(d[0].driver.as_deref(), Some("amdgpu"));
    assert!(d[0].boot_vga);
    assert!(d[0].busy_path.is_some(), "amdgpu publishes gpu_busy_percent");
    assert!(d[0].amd_perf_level.is_some());
    assert!(
        d[0].intel_min_freq.is_none() && d[0].intel_limits.is_none(),
        "an AMD card must not be offered an i915 control"
    );
    assert!(d[0].has_sysfs_control());
}

#[test]
fn hybrid_laptop_is_two_cards_of_two_vendors() {
    let f = katana("hybrid-discover");
    let d = discover(&f.sys());
    assert_eq!(d.len(), 2);
    assert_eq!(d[0].vendor, GpuVendor::Intel);
    assert_eq!(d[0].driver.as_deref(), Some("i915"));
    assert!(d[0].boot_vga);
    assert_eq!(d[0].intel_limits, Some((100, 1400)));
    assert!(d[0].busy_path.is_none(), "i915 publishes no busy percentage");

    assert_eq!(d[1].vendor, GpuVendor::Nvidia);
    assert_eq!(d[1].driver.as_deref(), Some("nvidia"));
    assert!(!d[1].boot_vga);
    assert!(
        d[1].busy_path.is_none() && d[1].clock_path.is_none() && !d[1].has_sysfs_control(),
        "the proprietary driver publishes nothing in sysfs, and pretending \
         otherwise is how a reader reports a missing file as a zero"
    );
}

#[test]
fn the_headline_card_is_the_discrete_one_not_the_lowest_numbered() {
    // THE DEFECT. The lab took whichever card answered first, which on this
    // machine is the Intel iGPU — so it reported the iGPU's clock, called it
    // "GPU", and never mentioned the RTX 3070 the games run on.
    let f = katana("hybrid-primary");
    let d = discover(&f.sys());
    assert_eq!(primary(&d).map(|p| p.card.as_str()), Some("card2"));

    // On a single-GPU machine the same rule has to pick the only card there is.
    let f2 = l16("amd-primary");
    let d2 = discover(&f2.sys());
    assert_eq!(primary(&d2).map(|p| p.card.as_str()), Some("card1"));

    assert!(primary(&[]).is_none());
}

#[test]
fn an_unknown_vendor_keeps_its_id_rather_than_guessing() {
    assert_eq!(GpuVendor::from_pci_id("0x1002"), GpuVendor::Amd);
    assert_eq!(GpuVendor::from_pci_id("0x8086"), GpuVendor::Intel);
    assert_eq!(GpuVendor::from_pci_id("0x10DE"), GpuVendor::Nvidia);
    assert_eq!(
        GpuVendor::from_pci_id("0x1234"),
        GpuVendor::Other("0x1234".into())
    );
    assert_eq!(GpuVendor::Other("0x1234".into()).label(), "0x1234");
}

// ── the Performance Lab, per card ───────────────────────────────────────────

#[test]
fn amd_reports_a_clock_and_a_busy_percentage_from_sysfs() {
    let f = l16("amd-perf");
    let d = read_gpu_devices(&f.sys(), &MockNvidiaSmi::default());
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].vendor, "AMD");
    // The STARRED level, not the highest: taking the ceiling would report the
    // card's maximum as though it were the live clock.
    assert_eq!(d[0].clock_mhz.value(), Some(&800));
    assert_eq!(d[0].busy_percent.value(), Some(&37.0));
}

#[test]
fn intel_reports_a_clock_and_says_why_it_cannot_report_busy() {
    let f = katana("intel-perf");
    let d = read_gpu_devices(&f.sys(), &MockNvidiaSmi::default());
    let intel = &d[0];
    assert_eq!(intel.vendor, "Intel");
    assert_eq!(intel.clock_mhz.value(), Some(&0), "RC6 is a real 0 MHz");
    assert!(intel.busy_percent.value().is_none());
    let why = intel.busy_percent.reason().unwrap_or_default();
    assert!(
        why.contains("PMU"),
        "the reason has to name the mechanism that WOULD work, not just say no: {why}"
    );
}

#[test]
fn nvidia_reports_through_nvidia_smi_or_not_at_all() {
    let f = katana("nvidia-perf");

    let with = read_gpu_devices(&f.sys(), &rtx3070());
    let nv = with.iter().find(|d| d.vendor == "NVIDIA").unwrap();
    assert_eq!(nv.clock_mhz.value(), Some(&1710));
    assert_eq!(nv.busy_percent.value(), Some(&64.0));
    assert_eq!(
        nv.busy_percent.source(),
        "nvidia-smi --query-gpu=utilization.gpu"
    );

    // Driver installed, nvidia-smi absent: unavailable with a reason, never a
    // zero. A 0% busy GPU and an unaskable one are different machines.
    let without = read_gpu_devices(&f.sys(), &MockNvidiaSmi::default());
    let nv2 = without.iter().find(|d| d.vendor == "NVIDIA").unwrap();
    assert!(nv2.busy_percent.value().is_none());
    assert!(nv2.clock_mhz.value().is_none());
    assert!(nv2.clock_mhz.reason().is_some());
}

// ── the safe knob ───────────────────────────────────────────────────────────

#[test]
fn amd_gets_a_dpm_level_and_only_a_valid_one() {
    let f = l16("amd-plan");
    let dev = &discover(&f.sys())[0];
    let prior = read_prior(dev);
    assert_eq!(
        prior.amd_perf_level.as_ref().map(|(_, v)| v.as_str()),
        Some("auto")
    );

    let cfg = SysfsGpuConfig {
        amd_perf_level: "high".into(),
        ..Default::default()
    };
    let (actions, notes) = plan_sysfs_enter(&cfg, dev, &prior);
    assert_eq!(actions.len(), 1, "notes: {notes:?}");
    match &actions[0] {
        Action::GpuSysfsAttr { path, value, .. } => {
            assert!(path.ends_with("power_dpm_force_performance_level"));
            assert_eq!(value, "high");
        }
        other => panic!("wrong action: {other:?}"),
    }

    // A level the driver would answer with -EINVAL is refused HERE, because a
    // refused sysfs write and an applied one look identical to the writer.
    let bad = SysfsGpuConfig {
        amd_perf_level: "turbo".into(),
        ..Default::default()
    };
    let (actions, notes) = plan_sysfs_enter(&bad, dev, &prior);
    assert!(actions.is_empty());
    assert!(notes.iter().any(|n| n.contains("turbo")), "{notes:?}");

    // "auto" is the shipped value and means leave it alone — not a gap, so not
    // a note either.
    let leave = SysfsGpuConfig {
        amd_perf_level: "auto".into(),
        ..Default::default()
    };
    let (actions, notes) = plan_sysfs_enter(&leave, dev, &prior);
    assert!(actions.is_empty() && notes.is_empty());
}

#[test]
fn intel_gets_a_frequency_floor_clamped_to_its_own_published_limits() {
    let f = katana("intel-plan");
    let dev = &discover(&f.sys())[0];
    let prior = read_prior(dev);
    assert_eq!(prior.intel_min_freq.as_ref().map(|(_, v)| *v), Some(100));

    // 100..1400 is the range the card reports. 50% of it is 750.
    assert_eq!(intel_floor_mhz((100, 1400), 50), 750);
    assert_eq!(intel_floor_mhz((100, 1400), 0), 100);
    assert_eq!(intel_floor_mhz((100, 1400), 100), 1400);
    // A profile asking for more than the whole range gets the ceiling, not an
    // out-of-range write the driver would refuse.
    assert_eq!(intel_floor_mhz((100, 1400), 250), 1400);

    let cfg = SysfsGpuConfig {
        intel_floor_percent: 50,
        ..Default::default()
    };
    let (actions, _) = plan_sysfs_enter(&cfg, dev, &prior);
    let floor = actions
        .iter()
        .find_map(|a| match a {
            Action::GpuSysfsAttr { path, value, .. } if path.ends_with("gt_min_freq_mhz") => {
                Some(value.clone())
            }
            _ => None,
        })
        .expect("no floor was planned");
    assert_eq!(floor, "750");
}

#[test]
fn a_card_with_no_published_limits_is_skipped_with_a_reason() {
    // Same i915 attributes, but no gt_RPn/gt_RP0 — which is what an older or a
    // partially-populated driver looks like. Planning a floor against limits
    // that are not there would be guessing at the hardware's range.
    let f = Fixture::new("intel-nolimits");
    f.write("sys/class/drm/card1/device/vendor", "0x8086\n")
        .write("sys/class/drm/card1/device/boot_vga", "1\n")
        .write("sys/class/drm/card1/gt_min_freq_mhz", "100\n");
    let dev = &discover(&f.sys())[0];
    assert_eq!(dev.intel_limits, None);
    let cfg = SysfsGpuConfig {
        intel_floor_percent: 50,
        ..Default::default()
    };
    let (actions, notes) = plan_sysfs_enter(&cfg, dev, &read_prior(dev));
    assert!(actions.is_empty());
    assert!(
        notes.iter().any(|n| n.contains("gt_RPn_freq_mhz")),
        "{notes:?}"
    );
}

#[test]
fn nvidia_only_machine_plans_nothing_through_sysfs() {
    let f = katana("nv-sysfs");
    let nv = discover(&f.sys())
        .into_iter()
        .find(|d| d.vendor == GpuVendor::Nvidia)
        .unwrap();
    let cfg = SysfsGpuConfig {
        amd_perf_level: "high".into(),
        intel_floor_percent: 80,
        ..Default::default()
    };
    let (actions, notes) = plan_sysfs_enter(&cfg, &nv, &read_prior(&nv));
    assert!(
        actions.is_empty() && notes.is_empty(),
        "an NVIDIA card must not be handed an amdgpu or i915 attribute: {actions:?}"
    );
}

#[test]
fn disabled_plans_nothing_on_any_vendor() {
    for (tag, fixture) in [("off-amd", l16("off-amd")), ("off-intel", katana("off-intel"))] {
        for dev in discover(&fixture.sys()) {
            let cfg = SysfsGpuConfig {
                enabled: false,
                amd_perf_level: "high".into(),
                intel_floor_percent: 90,
            };
            let (actions, notes) = plan_sysfs_enter(&cfg, &dev, &read_prior(&dev));
            assert!(actions.is_empty() && notes.is_empty(), "{tag}: {actions:?}");
        }
    }
}

// ── enter, then exit, against a writer that really writes ───────────────────

#[test]
fn the_exit_plan_puts_back_exactly_what_was_there() {
    let f = l16("amd-roundtrip");
    let dev = discover(&f.sys()).into_iter().next().unwrap();
    let prior = read_prior(&dev);
    let cfg = SysfsGpuConfig {
        amd_perf_level: "high".into(),
        ..Default::default()
    };

    // A REAL writer, rooted at the fixture. The actions carry absolute paths,
    // so this genuinely writes the files and genuinely reads them back — a mock
    // that records intent could not tell a restore from a no-op.
    let w = RealWriter::with_root(false, f.sys());

    let (enter, _) = plan_sysfs_enter(&cfg, &dev, &prior);
    for a in &enter {
        w.apply(a).unwrap();
    }
    assert_eq!(
        f.read("sys/class/drm/card1/device/power_dpm_force_performance_level"),
        "high"
    );

    for a in &plan_sysfs_exit(&prior) {
        w.apply(a).unwrap();
    }
    assert_eq!(
        f.read("sys/class/drm/card1/device/power_dpm_force_performance_level"),
        "auto",
        "leaving game mode must put the card back where it was found"
    );
}

#[test]
fn an_intel_floor_is_restored_to_the_mhz_that_was_there() {
    let f = katana("intel-roundtrip");
    let dev = discover(&f.sys()).into_iter().next().unwrap();
    let prior = read_prior(&dev);
    let cfg = SysfsGpuConfig {
        intel_floor_percent: 50,
        ..Default::default()
    };
    let w = RealWriter::with_root(false, f.sys());

    let (enter, _) = plan_sysfs_enter(&cfg, &dev, &prior);
    for a in &enter {
        w.apply(a).unwrap();
    }
    assert_eq!(f.read("sys/class/drm/card1/gt_min_freq_mhz"), "750");

    for a in &plan_sysfs_exit(&prior) {
        w.apply(a).unwrap();
    }
    assert_eq!(f.read("sys/class/drm/card1/gt_min_freq_mhz"), "100");
}

#[test]
fn nothing_is_restored_that_was_never_read() {
    // A control whose prior value could not be read is LEFT ALONE. Writing a
    // plausible default would be inventing a state the machine was never in.
    let actions = plan_sysfs_exit(&Default::default());
    assert!(actions.is_empty());
}

#[test]
fn a_dry_run_writer_changes_nothing() {
    let f = l16("amd-dryrun");
    let dev = discover(&f.sys()).into_iter().next().unwrap();
    let cfg = SysfsGpuConfig {
        amd_perf_level: "high".into(),
        ..Default::default()
    };
    let w = RealWriter::with_root(true, f.sys());
    let (enter, _) = plan_sysfs_enter(&cfg, &dev, &read_prior(&dev));
    assert!(!enter.is_empty(), "the plan itself must not be empty");
    for a in &enter {
        w.apply(a).unwrap();
    }
    assert_eq!(
        f.read("sys/class/drm/card1/device/power_dpm_force_performance_level"),
        "auto"
    );
}

// ── the action's own rendering ──────────────────────────────────────────────

#[test]
fn the_action_says_which_file_it_writes() {
    let a = Action::GpuSysfsAttr {
        path: "/sys/class/drm/card1/device/power_dpm_force_performance_level".into(),
        value: "high".into(),
        what: "card1 DPM level".into(),
    };
    let d = a.describe();
    assert!(d.contains("card1 DPM level"), "{d}");
    assert!(d.contains("power_dpm_force_performance_level"), "{d}");
    assert!(d.contains("high"), "{d}");
}
