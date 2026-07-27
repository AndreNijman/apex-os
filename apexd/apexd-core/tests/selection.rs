//! Layered-selection tests against synthetic fingerprints. No filesystem is
//! touched: fingerprints are built by hand and selection is pure.

use apexd_core::fingerprint::{CpuInfo, CpuVendor, Fingerprint, GpuInfo, GpuVendor};
use apexd_core::profile::ProfileSet;
use apexd_core::select;
use apexd_core::tier::Tier;

fn amd_cpu() -> CpuInfo {
    CpuInfo {
        vendor: CpuVendor::Amd,
        model_name: "AMD Ryzen 7 PRO 250 w/ Radeon 780M Graphics".into(),
        physical_cores: 8,
        logical_threads: 16,
        scaling_driver: Some("amd-pstate-epp".into()),
        hybrid: false,
    }
}

fn intel_hybrid_cpu() -> CpuInfo {
    CpuInfo {
        vendor: CpuVendor::Intel,
        model_name: "12th Gen Intel(R) Core(TM) i7-12700H".into(),
        physical_cores: 14,
        logical_threads: 20,
        scaling_driver: Some("intel_pstate".into()),
        hybrid: true,
    }
}

fn intel_uniform_cpu() -> CpuInfo {
    CpuInfo {
        vendor: CpuVendor::Intel,
        model_name: "Intel(R) Core(TM) i5-8250U".into(),
        physical_cores: 4,
        logical_threads: 8,
        scaling_driver: Some("intel_pstate".into()),
        hybrid: false,
    }
}

/// An ARM machine: `/proc/cpuinfo` publishes no `vendor_id`, so the vendor is
/// unknown and no CPU-class profile can apply.
fn arm_cpu() -> CpuInfo {
    CpuInfo {
        vendor: CpuVendor::Other("unknown".into()),
        model_name: String::new(),
        physical_cores: 8,
        logical_threads: 8,
        scaling_driver: Some("cpufreq-dt".into()),
        hybrid: false,
    }
}

/// An old AMD box on plain acpi-cpufreq: no EPP anywhere.
fn amd_legacy_cpu() -> CpuInfo {
    CpuInfo {
        vendor: CpuVendor::Amd,
        model_name: "AMD FX-8350 Eight-Core Processor".into(),
        physical_cores: 8,
        logical_threads: 8,
        scaling_driver: Some("acpi-cpufreq".into()),
        hybrid: false,
    }
}

fn amd_gpu() -> GpuInfo {
    GpuInfo {
        vendor: GpuVendor::Amd,
        pci_vendor: 0x1002,
        pci_device: 0x1900,
        pci_slot: "0000:c5:00.0".into(),
    }
}

fn intel_gpu() -> GpuInfo {
    GpuInfo {
        vendor: GpuVendor::Intel,
        pci_vendor: 0x8086,
        pci_device: 0x46a6,
        pci_slot: "0000:00:02.0".into(),
    }
}

fn nvidia_gpu() -> GpuInfo {
    GpuInfo {
        vendor: GpuVendor::Nvidia,
        pci_vendor: 0x10de,
        pci_device: 0x24dd,
        pci_slot: "0000:01:00.0".into(),
    }
}

fn fp(
    cpu: CpuInfo,
    gpus: Vec<GpuInfo>,
    chassis_type: u32,
    vendor: &str,
    name: &str,
    family: &str,
    version: &str,
) -> Fingerprint {
    Fingerprint {
        cpu,
        gpus,
        chassis_type,
        sys_vendor: vendor.into(),
        product_name: name.into(),
        product_family: family.into(),
        product_version: version.into(),
        has_ac: true,
        batteries: vec!["BAT0".into()],
    }
}

/// As [`fp`], but with no AC line and no battery — a desktop, a VM, or a board
/// whose power-supply drivers did not bind.
fn fp_no_power(cpu: CpuInfo, chassis_type: u32, vendor: &str, name: &str) -> Fingerprint {
    Fingerprint {
        has_ac: false,
        batteries: Vec::new(),
        ..fp(cpu, Vec::new(), chassis_type, vendor, name, "", "")
    }
}

#[test]
fn thinkpad_l16_matches_device_over_class() {
    // The real L16: product_name is the bare MTM code, the friendly string is
    // in product_version / product_family — device match must look there.
    let f = fp(
        amd_cpu(),
        vec![amd_gpu()],
        10,
        "LENOVO",
        "21SCCTO1WW",
        "ThinkPad L16 Gen 2",
        "ThinkPad L16 Gen 2",
    );
    let sel = select(&f, &ProfileSet::builtin());
    assert_eq!(sel.generic, "generic-laptop");
    assert_eq!(sel.class.as_deref(), Some("amd-zen"));
    assert_eq!(sel.device.as_deref(), Some("thinkpad-l16-g2"));
    assert_eq!(sel.active, "thinkpad-l16-g2");
}

#[test]
fn katana_matches_device_over_intel_hybrid_class() {
    let f = fp(
        intel_hybrid_cpu(),
        vec![intel_gpu(), nvidia_gpu()],
        10,
        "Micro-Star International Co., Ltd.",
        "Katana GF76 12UE",
        "Katana",
        "REV:1.0",
    );
    let sel = select(&f, &ProfileSet::builtin());
    assert_eq!(sel.generic, "generic-laptop");
    assert_eq!(sel.class.as_deref(), Some("intel-hybrid"));
    assert_eq!(sel.device.as_deref(), Some("msi-katana-gf76"));
    assert_eq!(sel.active, "msi-katana-gf76");
    // And the fingerprint should flag the Optimus hybrid graphics.
    assert!(f.intel_nvidia_hybrid_gpu());
}

#[test]
fn generic_amd_desktop_falls_to_class() {
    let f = fp(
        amd_cpu(),
        vec![amd_gpu()],
        3, // desktop
        "ASUS",
        "System Product Name",
        "To be filled by O.E.M.",
        "1.0",
    );
    let sel = select(&f, &ProfileSet::builtin());
    assert_eq!(sel.generic, "generic-desktop");
    assert_eq!(sel.class.as_deref(), Some("amd-zen"));
    assert_eq!(sel.device, None);
    assert_eq!(sel.active, "amd-zen");
    assert!(!f.is_laptop());
}

#[test]
fn unknown_intel_hybrid_laptop_uses_class_no_device() {
    let f = fp(
        intel_hybrid_cpu(),
        vec![intel_gpu()],
        10,
        "Framework",
        "Laptop 13",
        "Laptop",
        "AA",
    );
    let sel = select(&f, &ProfileSet::builtin());
    assert_eq!(sel.generic, "generic-laptop");
    assert_eq!(sel.class.as_deref(), Some("intel-hybrid"));
    assert_eq!(sel.device, None);
    assert_eq!(sel.active, "intel-hybrid");
}

#[test]
fn uniform_intel_laptop_has_no_class() {
    // Non-hybrid Intel gets no class profile and stays generic.
    let f = fp(
        intel_uniform_cpu(),
        vec![intel_gpu()],
        10,
        "Dell Inc.",
        "Latitude 7490",
        "Latitude",
        "1.0",
    );
    let sel = select(&f, &ProfileSet::builtin());
    assert_eq!(sel.generic, "generic-laptop");
    assert_eq!(sel.class, None);
    assert_eq!(sel.device, None);
    assert_eq!(sel.active, "generic-laptop");
}

#[test]
fn builtin_set_has_all_six_profiles() {
    let set = ProfileSet::builtin();
    assert_eq!(set.len(), 6);
    for id in [
        "generic-desktop",
        "generic-laptop",
        "intel-hybrid",
        "amd-zen",
        "thinkpad-l16-g2",
        "msi-katana-gf76",
    ] {
        assert!(set.get(id).is_some(), "missing builtin profile {id}");
    }
}

// ── machines nobody here owns ────────────────────────────────────────────────

#[test]
fn an_arm_laptop_matches_no_class_and_lands_on_the_generic_profile() {
    // Unknown CPU vendor, no GPU, no device match. This is the path a machine
    // takes when APEX-OS has never heard of it, and it must be a complete,
    // usable profile rather than a hole.
    let f = fp(arm_cpu(), vec![], 10, "Qualcomm", "Snapdragon Dev Kit", "", "");
    let set = ProfileSet::builtin();
    let sel = select(&f, &set);
    assert_eq!(sel.class, None, "no CPU class applies to an unknown vendor");
    assert_eq!(sel.device, None);
    assert_eq!(sel.active, "generic-laptop");
    // And the profile it landed on can plan every tier.
    let p = set.get(&sel.active).unwrap();
    for tier in Tier::ALL {
        assert!(!p.plan_tier(tier).is_empty(), "{tier} planned nothing");
    }
}

#[test]
fn an_unknown_machine_with_no_chassis_type_lands_on_generic_desktop() {
    // DMI is frequently absent or zeroed on SBCs and in VMs. chassis_type 0 is
    // "unspecified", which must not be mistaken for a laptop.
    let f = fp_no_power(arm_cpu(), 0, "", "");
    let sel = select(&f, &ProfileSet::builtin());
    assert!(!f.is_laptop());
    assert_eq!(sel.generic, "generic-desktop");
    assert_eq!(sel.active, "generic-desktop");
    assert_eq!(sel.class_or_empty(), "");
    assert_eq!(sel.device_or_empty(), "");
}

#[test]
fn a_machine_with_no_battery_selects_normally() {
    // Nothing in selection may depend on a battery being present.
    let f = fp_no_power(amd_legacy_cpu(), 3, "Gigabyte", "B450 AORUS");
    assert!(f.batteries.is_empty());
    let sel = select(&f, &ProfileSet::builtin());
    assert_eq!(sel.generic, "generic-desktop");
    assert_eq!(sel.active, "amd-zen");
}

#[test]
fn a_legacy_amd_box_without_epp_still_gets_the_amd_class() {
    // The class profile expresses EPP; the writer drops it where it does not
    // exist, so claiming the class is correct even on acpi-cpufreq.
    let f = fp(amd_legacy_cpu(), vec![amd_gpu()], 3, "ASUS", "M5A99X", "", "");
    let sel = select(&f, &ProfileSet::builtin());
    assert_eq!(sel.class.as_deref(), Some("amd-zen"));
    assert!(!f.cpu.amd_pstate(), "this part has no amd-pstate");
}

#[test]
fn selection_still_resolves_when_the_override_directory_is_partial() {
    // An on-disk override holding only a device profile used to leave `active`
    // pointing at a generic profile the set did not contain, which panicked the
    // moment anything looked it up. `ProfileSet::load` now backfills.
    let dir = std::env::temp_dir().join(format!("apexd-sel-partial-{}", std::process::id()));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("only-device.toml"),
        r#"
            id = "only-device"
            kind = "device"
            [defaults]
            ac = "performance"
            battery = "balanced"
            [tiers.performance]
            governor = "performance"
            [tiers.balanced]
            governor = "powersave"
            [tiers.power-saver]
            governor = "powersave"
        "#,
    )
    .unwrap();

    let set = ProfileSet::load(Some(&dir)).unwrap();
    assert!(set.get("only-device").is_some(), "the override is loaded");
    let f = fp_no_power(arm_cpu(), 10, "Unknown", "Unknown");
    let sel = select(&f, &set);
    assert!(
        set.get(&sel.active).is_some(),
        "selection must never name a profile the set lacks"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_stale_override_profile_is_skipped_rather_than_bricking_the_daemon() {
    // A profile left behind by an older image still naming a removed tier must
    // not stop the whole set from loading — the machine would be left with no
    // power management at all.
    let dir = std::env::temp_dir().join(format!("apexd-sel-stale-{}", std::process::id()));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("stale.toml"),
        r#"
            id = "stale"
            kind = "device"
            [defaults]
            ac = "ultra"
            battery = "balanced"
            [tiers.ultra-max]
            governor = "performance"
            [tiers.ultra]
            governor = "performance"
            [tiers.performance]
            governor = "performance"
            [tiers.balanced]
            governor = "powersave"
            [tiers.power-saver]
            governor = "powersave"
        "#,
    )
    .unwrap();

    let set = ProfileSet::load(Some(&dir)).expect("a stale override must not be fatal");
    assert!(set.get("stale").is_none(), "the stale profile is skipped");
    assert!(
        set.get("generic-laptop").is_some() && set.get("generic-desktop").is_some(),
        "and the embedded profiles carry the machine instead"
    );
    std::fs::remove_dir_all(&dir).ok();
}
