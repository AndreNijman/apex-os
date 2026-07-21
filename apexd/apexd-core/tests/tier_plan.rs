//! Tier-engine tests: (profile, tier) -> [Action], transitions, ryzenadj
//! gating/clamping, and proof that MockWriter records exactly the plan while
//! RealWriter's dry-run gate performs zero writes.

use apexd_core::profile::ProfileSet;
use apexd_core::syswriter::{MockWriter, RealWriter, SysWriter};
use apexd_core::tier::{Action, Tier};

fn set() -> ProfileSet {
    ProfileSet::builtin()
}

#[test]
fn amd_zen_ultra_max_has_no_ryzenadj() {
    let s = set();
    let p = s.get("amd-zen").unwrap();
    let plan = p.plan_tier(Tier::UltraMax);
    assert_eq!(
        plan,
        vec![
            Action::Governor("performance".into()),
            Action::Epp("performance".into()),
            Action::PlatformProfile("performance".into()),
        ]
    );
}

#[test]
fn thinkpad_ultra_max_adds_clamped_ryzenadj() {
    let s = set();
    let p = s.get("thinkpad-l16-g2").unwrap();
    let plan = p.plan_tier(Tier::UltraMax);
    assert_eq!(
        plan,
        vec![
            Action::Governor("performance".into()),
            Action::Epp("performance".into()),
            Action::PlatformProfile("performance".into()),
            Action::RyzenAdj {
                stapm_mw: 62000,
                fast_mw: 75000, // <= ceiling 79000, unchanged
                slow_mw: 62000,
                tctl_max: Some(95),
            },
        ]
    );
}

#[test]
fn thinkpad_lower_tiers_have_no_ryzenadj() {
    let s = set();
    let p = s.get("thinkpad-l16-g2").unwrap();
    for tier in [Tier::Ultra, Tier::Performance, Tier::Balanced, Tier::PowerSaver] {
        let plan = p.plan_tier(tier);
        assert!(
            !plan.iter().any(|a| matches!(a, Action::RyzenAdj { .. })),
            "tier {tier} unexpectedly requested ryzenadj"
        );
    }
    // power-saver is the soft end of the table.
    assert_eq!(
        p.plan_tier(Tier::PowerSaver),
        vec![
            Action::Governor("powersave".into()),
            Action::Epp("power".into()),
            Action::PlatformProfile("low-power".into()),
        ]
    );
}

#[test]
fn transition_away_from_ultra_max_tears_down_ryzenadj() {
    let s = set();
    let p = s.get("thinkpad-l16-g2").unwrap();
    let plan = p.plan_transition(Some(Tier::UltraMax), Tier::Balanced);
    assert_eq!(plan.first(), Some(&Action::StopRyzenAdj));
    // and the balanced target follows the teardown.
    assert!(plan.contains(&Action::Governor("powersave".into())));
    assert!(!plan.iter().any(|a| matches!(a, Action::RyzenAdj { .. })));
}

#[test]
fn transition_into_ultra_max_has_no_teardown() {
    let s = set();
    let p = s.get("thinkpad-l16-g2").unwrap();
    let plan = p.plan_transition(Some(Tier::Balanced), Tier::UltraMax);
    assert!(!plan.contains(&Action::StopRyzenAdj));
    assert!(plan.iter().any(|a| matches!(a, Action::RyzenAdj { .. })));
}

#[test]
fn msi_katana_omits_platform_profile_and_ryzenadj() {
    let s = set();
    let p = s.get("msi-katana-gf76").unwrap();
    let plan = p.plan_tier(Tier::UltraMax);
    assert_eq!(
        plan,
        vec![
            Action::Governor("performance".into()),
            Action::Epp("performance".into()),
        ]
    );
    // Its AC default is the aggressive tier.
    assert_eq!(p.defaults.ac, Tier::Ultra);
}

#[test]
fn thinkpad_charge_action_is_75_80() {
    let s = set();
    let p = s.get("thinkpad-l16-g2").unwrap();
    let action = p.charge_action().expect("thinkpad has charge config");
    match action {
        Action::ChargeThresholds { start, stop, .. } => {
            assert_eq!(start, 75);
            assert_eq!(stop, 80);
        }
        other => panic!("expected ChargeThresholds, got {other:?}"),
    }
    // amd-zen (class) declares none.
    assert!(s.get("amd-zen").unwrap().charge_action().is_none());
}

#[test]
fn ryzenadj_ceiling_clamps_over_limit_values() {
    // A profile that asks for more than the ceiling must be clamped so a bad
    // profile can never exceed the thermal envelope.
    let toml = r#"
        id = "test-clamp"
        kind = "device"
        [defaults]
        ac = "performance"
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
        [ryzenadj]
        stapm_mw = 90000
        fast_mw = 95000
        slow_mw = 90000
        ceiling_mw = 79000
    "#;
    let p = apexd_core::profile::Profile::from_toml(toml).unwrap();
    let plan = p.plan_tier(Tier::UltraMax);
    let rz = plan
        .iter()
        .find_map(|a| match a {
            Action::RyzenAdj {
                stapm_mw,
                fast_mw,
                slow_mw,
                ..
            } => Some((*stapm_mw, *fast_mw, *slow_mw)),
            _ => None,
        })
        .expect("ryzenadj present");
    assert_eq!(rz, (79000, 79000, 79000));
}

#[test]
fn mock_writer_records_plan_and_writes_nothing_real() {
    let s = set();
    let p = s.get("thinkpad-l16-g2").unwrap();
    let plan = p.plan_tier(Tier::UltraMax);
    let mock = MockWriter::new();
    mock.apply_all(&plan).unwrap();
    assert_eq!(mock.recorded(), plan);
    assert!(!mock.is_live());
}

#[test]
fn real_writer_dry_run_does_not_write_fixture_sysfs() {
    // Build a fake sysfs tree in a temp dir (NOT real /sys) and prove the
    // dry-run gate leaves it untouched, then that a live writer does write it.
    let root = std::env::temp_dir().join(format!("apexd-test-sys-{}", std::process::id()));
    let policy = root.join("devices/system/cpu/cpufreq/policy0");
    std::fs::create_dir_all(&policy).unwrap();
    let gov = policy.join("scaling_governor");
    std::fs::write(&gov, "powersave").unwrap();

    // dry-run: unchanged.
    let dry = RealWriter::with_root(true, &root);
    assert!(!dry.is_live());
    dry.apply(&Action::Governor("performance".into())).unwrap();
    assert_eq!(std::fs::read_to_string(&gov).unwrap(), "powersave");

    // live (against the fixture, still not real /sys): written.
    let live = RealWriter::with_root(false, &root);
    assert!(live.is_live());
    live.apply(&Action::Governor("performance".into())).unwrap();
    assert_eq!(std::fs::read_to_string(&gov).unwrap(), "performance");

    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn real_writer_dry_run_skips_absent_platform_profile() {
    // No platform_profile file in the fixture -> apply is a no-op success.
    let root = std::env::temp_dir().join(format!("apexd-test-sys-pp-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let w = RealWriter::with_root(false, &root);
    assert!(w
        .apply(&Action::PlatformProfile("performance".into()))
        .is_ok());
    std::fs::remove_dir_all(&root).ok();
}
