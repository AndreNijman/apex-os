//! Layered-selection tests against synthetic fingerprints. No filesystem is
//! touched: fingerprints are built by hand and selection is pure.

use apexd_core::fingerprint::{CpuInfo, CpuVendor, Fingerprint, GpuInfo, GpuVendor};
use apexd_core::profile::ProfileSet;
use apexd_core::select;

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
