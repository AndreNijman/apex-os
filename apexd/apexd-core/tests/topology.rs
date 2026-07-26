//! P-core / E-core detection against synthetic sysfs trees.
//!
//! The Alder Lake fixture uses the real i7-12700H layout — 6 P-cores × 2
//! threads on CPUs 0-11 and 8 E-cores on CPUs 12-19 — so the assertions mean
//! something on the actual target rather than just exercising the parser.

use std::fs;
use std::path::{Path, PathBuf};

use apexd_core::topology::{
    format_cpu_list, online_cpus, parse_cpu_list, parse_cpu_mask, CoreSource, CoreTopology,
};

/// A throwaway sysfs root, removed when the guard drops.
struct Fixture(PathBuf);

impl Fixture {
    fn new(tag: &str) -> Fixture {
        let root = std::env::temp_dir().join(format!(
            "apexd-topo-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        fs::remove_dir_all(&root).ok();
        fs::create_dir_all(&root).unwrap();
        Fixture(root)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn write(&self, rel: &str, contents: &str) {
        let p = self.0.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, contents).unwrap();
    }

    /// Create `cpu0..cpuN-1` directories so enumeration has something to find.
    fn cpus(&self, n: u32) {
        for c in 0..n {
            fs::create_dir_all(self.0.join(format!("devices/system/cpu/cpu{c}"))).unwrap();
        }
        self.write(
            "devices/system/cpu/online",
            &format!("0-{}\n", n.saturating_sub(1)),
        );
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).ok();
    }
}

#[test]
fn cpu_list_round_trip() {
    assert_eq!(parse_cpu_list("0-11"), (0..=11).collect::<Vec<u32>>());
    assert_eq!(parse_cpu_list("12-19"), (12..=19).collect::<Vec<u32>>());
    assert_eq!(parse_cpu_list("0,2,4-6"), vec![0, 2, 4, 5, 6]);
    assert_eq!(parse_cpu_list(""), Vec::<u32>::new());
    assert_eq!(format_cpu_list(&[0, 1, 2, 3]), "0-3");
    assert_eq!(format_cpu_list(&[0, 2, 3, 9]), "0,2-3,9");
    assert_eq!(format_cpu_list(&[]), "");
    // 20-thread Alder Lake mask.
    assert_eq!(parse_cpu_mask("000fffff").len(), 20);
    assert_eq!(parse_cpu_mask("00000000,00000fff"), (0..=11).collect::<Vec<u32>>());
}

#[test]
fn alder_lake_hybrid_pmu_is_the_first_rung() {
    // i7-12700H: 6 P-cores (12 threads) + 8 E-cores.
    let f = Fixture::new("adl-pmu");
    f.cpus(20);
    f.write("devices/cpu_core/cpus", "0-11\n");
    f.write("devices/cpu_atom/cpus", "12-19\n");

    let t = CoreTopology::detect_from(f.path());
    assert_eq!(t.source, CoreSource::HybridPmu);
    assert!(t.is_hybrid());
    assert_eq!(t.pcore_list(), "0-11");
    assert_eq!(t.ecore_list(), "12-19");
    assert_eq!(t.all.len(), 20);
    // IRQs get parked on whatever the game is not using.
    assert_eq!(format_cpu_list(&t.complement(&t.pcores)), "12-19");
}

#[test]
fn cpu_types_directories_are_the_second_rung() {
    let f = Fixture::new("cpu-types");
    f.cpus(20);
    // No cpu_core/cpu_atom PMUs; the per-type directories instead.
    f.write("devices/system/cpu/types/intel_core_0/cpulist", "0-11\n");
    f.write("devices/system/cpu/types/intel_atom_0/cpulist", "12-19\n");

    let t = CoreTopology::detect_from(f.path());
    assert_eq!(t.source, CoreSource::CpuTypes);
    assert_eq!(t.pcore_list(), "0-11");
    assert_eq!(t.ecore_list(), "12-19");
}

#[test]
fn cpu_types_accepts_a_hex_cpumap() {
    let f = Fixture::new("cpu-types-mask");
    f.cpus(20);
    f.write("devices/system/cpu/types/intel_core_0/cpumap", "00000fff\n");
    f.write("devices/system/cpu/types/intel_atom_0/cpumap", "000ff000\n");

    let t = CoreTopology::detect_from(f.path());
    assert_eq!(t.source, CoreSource::CpuTypes);
    assert_eq!(t.pcore_list(), "0-11");
    assert_eq!(t.ecore_list(), "12-19");
}

#[test]
fn max_freq_is_the_last_rung_and_needs_a_real_spread() {
    let f = Fixture::new("maxfreq");
    f.cpus(4);
    // 4.7 GHz vs 3.5 GHz — a genuine P/E spread.
    for c in 0..2 {
        f.write(
            &format!("devices/system/cpu/cpu{c}/cpufreq/cpuinfo_max_freq"),
            "4700000\n",
        );
    }
    for c in 2..4 {
        f.write(
            &format!("devices/system/cpu/cpu{c}/cpufreq/cpuinfo_max_freq"),
            "3500000\n",
        );
    }
    let t = CoreTopology::detect_from(f.path());
    assert_eq!(t.source, CoreSource::MaxFreq);
    assert_eq!(t.pcore_list(), "0-1");
    assert_eq!(t.ecore_list(), "2-3");

    // A 2% bin difference is NOT a hybrid machine.
    let g = Fixture::new("maxfreq-narrow");
    g.cpus(4);
    for c in 0..2 {
        g.write(
            &format!("devices/system/cpu/cpu{c}/cpufreq/cpuinfo_max_freq"),
            "4700000\n",
        );
    }
    for c in 2..4 {
        g.write(
            &format!("devices/system/cpu/cpu{c}/cpufreq/cpuinfo_max_freq"),
            "4600000\n",
        );
    }
    let t = CoreTopology::detect_from(g.path());
    assert_eq!(t.source, CoreSource::Uniform);
    assert!(!t.is_hybrid());
}

#[test]
fn cppc_highest_perf_splits_when_freq_is_absent() {
    let f = Fixture::new("cppc");
    f.cpus(4);
    for (c, v) in [(0u32, 196u32), (1, 196), (2, 98), (3, 98)] {
        f.write(
            &format!("devices/system/cpu/cpu{c}/acpi_cppc/highest_perf"),
            &format!("{v}\n"),
        );
    }
    let t = CoreTopology::detect_from(f.path());
    assert_eq!(t.source, CoreSource::Cppc);
    assert_eq!(t.pcore_list(), "0-1");
    assert_eq!(t.ecore_list(), "2-3");
}

#[test]
fn uniform_amd_machine_has_no_split_and_never_panics() {
    // Ryzen 7 PRO 250: 8 cores / 16 threads, no hybrid interfaces at all.
    let f = Fixture::new("l16");
    f.cpus(16);
    let t = CoreTopology::detect_from(f.path());
    assert_eq!(t.source, CoreSource::Uniform);
    assert!(!t.is_hybrid());
    assert_eq!(t.pcores.len(), 16);
    assert!(t.ecores.is_empty());
    // The complement of "everything" is empty, which is what disables steering.
    assert!(t.complement(&t.pcores).is_empty());
}

#[test]
fn missing_sysfs_yields_unknown_not_a_panic() {
    let t = CoreTopology::detect_from(Path::new("/nonexistent/apexd-fixture"));
    assert_eq!(t.source, CoreSource::Unknown);
    assert!(t.all.is_empty());
    assert!(!t.is_hybrid());
    assert_eq!(t.pcore_list(), "");
    assert!(online_cpus(Path::new("/nonexistent/apexd-fixture")).is_empty());
}

#[test]
fn enumeration_falls_back_to_cpu_directories() {
    let f = Fixture::new("no-online");
    for c in 0..6u32 {
        fs::create_dir_all(f.path().join(format!("devices/system/cpu/cpu{c}"))).unwrap();
    }
    // Decoys that must not be counted as CPUs.
    fs::create_dir_all(f.path().join("devices/system/cpu/cpufreq/policy0")).unwrap();
    fs::create_dir_all(f.path().join("devices/system/cpu/cpuidle")).unwrap();
    assert_eq!(online_cpus(f.path()), vec![0, 1, 2, 3, 4, 5]);
}
