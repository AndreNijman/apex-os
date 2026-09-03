//! Game-mode orchestration on top of [`Ctx`]: enter/exit, prior-state capture,
//! and the D-Bus status payload.
//!
//! Symmetry rules, in order of importance:
//!
//! * Prior state is captured **only on the 0 -> 1 transition**. A second
//!   `SetActive(true)` attaches PIDs and nothing else, so it can never
//!   overwrite the values exit has to restore.
//! * Auto-switch is *disabled* for the duration of a session. Otherwise an
//!   AC/battery transition mid-game would re-apply the profile default, clobber
//!   the game tier, and leave the recorded "prior tier" pointing at a tier that
//!   is no longer meaningful.
//! * The exit plan is computed at enter time from values read before anything
//!   was written (see [`apexd_core::game::plan`]).
//!
//! ── Where "what actually landed" lives ──────────────────────────────────────
//!
//! In [`GameSession`], in memory, and nowhere else. This is generated state —
//! a measurement of one session, not anything a user typed — so §10's rule
//! about keeping generated state out of user-owned files is satisfied without
//! a file at all: the fact is scoped to a session, the session dies with the
//! daemon, and a file would add ownership, labelling and staleness questions
//! to answer a question nobody can ask once the session is over.
//!
//! `apex game status` reads it over the `org.apexos.Apexd1.GameMode` `Status`
//! property, which is a plain property read: no polkit action, no root, no
//! recomputation of a plan. That is what keeps status read-only while still
//! reporting a fact only the privileged applier could have observed.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use anyhow::{bail, Result};
use apexd_core::fan::FanMode;
use apexd_core::game::{self, GameInputs, PidPlacement, CGROUP_ROOT};
use apexd_core::irq;
use apexd_core::syswriter::Outcome;
use apexd_core::tier::{Action, Tier};
use apexd_core::topology::CoreTopology;
use zvariant::{OwnedValue, Value};

use crate::state::Ctx;

/// What applying the enter plan's [`Action::IrqAffinity`] writes actually did.
///
/// Kept separate from the plan's own count because they are different facts and
/// conflating them is the bug this type exists to close: `apex game status`
/// used to report `plan.irqs_attempted` under the name `irqs_steered`, so a
/// machine whose kernel refused every affinity write was told "N IRQs steered".
///
/// `IrqAffinity` is the one action that is exactly one write (see
/// [`apexd_core::syswriter::Outcome`]), which is what makes counting these a
/// measurement rather than an estimate.
#[derive(Debug, Clone, Default)]
pub struct IrqReport {
    /// Affinity writes the plan contained.
    pub attempted: usize,
    /// Affinity writes the kernel accepted.
    pub landed: usize,
    /// The first refusal's reason, so status can say *why* rather than only
    /// that a number is lower than expected. Refusals are overwhelmingly the
    /// same reason repeated (a kernel-managed interrupt returns -EIO), so one
    /// example beats a list as long as the count is next to it.
    pub first_refusal: Option<String>,
}

impl IrqReport {
    pub fn refused(&self) -> usize {
        self.attempted.saturating_sub(self.landed)
    }
}

/// Everything needed to undo a session.
pub struct GameSession {
    /// The exit plan, built at enter time.
    pub exit_actions: Vec<Action>,
    pub prior_tier: Tier,
    pub prior_auto_switch: bool,
    /// The fan mode in force before the session (only restored when the session
    /// actually changed it).
    pub prior_fan_mode: Option<FanMode>,
    pub tier: Tier,
    pub cpus: Vec<u32>,
    pub cpu_list: String,
    pub core_source: String,
    pub pids: Vec<u32>,
    pub gpus_locked: Vec<u32>,
    /// Measured at enter time from the writer's own outcomes.
    pub irqs: IrqReport,
    pub notes: Vec<String>,
}

impl Ctx {
    /// True while a session is active.
    pub async fn game_active(&self) -> bool {
        self.game.lock().await.is_some()
    }

    /// Whether the active profile permits game mode at all.
    pub fn game_supported(&self) -> bool {
        self.profile().game_config().enabled
    }

    /// Enter game mode (idempotent). When a session is already running, extra
    /// PIDs are attached to the existing cpuset and nothing else changes.
    pub async fn game_enter(self: &Arc<Self>, pids: &[u32]) -> Result<()> {
        let cfg = self.profile().game_config();
        if !cfg.enabled {
            bail!("game mode is disabled for profile '{}'", self.selection.active);
        }

        {
            let existing = self.game.lock().await;
            if existing.is_some() {
                drop(existing);
                for pid in pids {
                    self.game_attach(*pid).await?;
                }
                return Ok(());
            }
        }

        // ── read the machine as it is right now ──────────────────────────────
        let topo = CoreTopology::detect_from(&self.sys_root);
        let irqs = irq::enumerate(&self.proc_irq_root);
        let nvidia = self.nvidia.query();
        let mems = cfg
            .cpuset_mems
            .clone()
            .unwrap_or_else(|| game::read_cgroup_mems(Path::new(CGROUP_ROOT)));
        let placements: Vec<PidPlacement> = pids
            .iter()
            .map(|pid| PidPlacement {
                pid: *pid,
                prior_cgroup: game::read_pid_cgroup(Path::new("/proc"), CGROUP_ROOT, *pid),
            })
            .collect();

        let plan = game::plan(&GameInputs {
            cfg: &cfg,
            topo: &topo,
            nvidia: &nvidia,
            irqs: &irqs,
            pids: &placements,
            mems,
            irqbalance: irq::irqbalance_running(Path::new("/proc")),
        });

        // ── prior state (0 -> 1 only) ────────────────────────────────────────
        let (prior_tier, prior_auto_switch) = {
            let mut st = self.state.lock().await;
            let prior = (st.tier, st.auto_switch);
            // A tier change from the AC loop mid-session would desynchronise the
            // recorded prior tier; hold auto-switch off until exit.
            st.auto_switch = false;
            prior
        };

        // ── apply ────────────────────────────────────────────────────────────
        if let Err(e) = self.apply_tier(cfg.tier).await {
            eprintln!("apexd: game: tier {} failed: {e:#}", cfg.tier);
        }

        let prior_fan_mode = match &cfg.fan_mode {
            Some(want) if self.fan.supported() => {
                let prior = self.fan.mode().await;
                match FanMode::parse(want, self.fan.default_manual_pwm()) {
                    Ok(m) => match self.fan.set_mode(m).await {
                        Ok(()) => Some(prior),
                        Err(e) => {
                            eprintln!("apexd: game: fan mode '{want}' failed: {e:#}");
                            None
                        }
                    },
                    Err(e) => {
                        eprintln!("apexd: game: profile fan_mode invalid: {e}");
                        None
                    }
                }
            }
            _ => None,
        };

        // The applier is also the only place that can observe what the machine
        // did with the plan, so it counts as it goes. An `Err` here is a hard
        // failure; an `Outcome::Refused` is the ordinary case this counting
        // exists for — a kernel-managed interrupt refusing an affinity write.
        let mut irqs = IrqReport::default();
        for a in &plan.enter {
            let is_irq = matches!(a, Action::IrqAffinity { .. });
            if is_irq {
                irqs.attempted += 1;
            }
            match self.writer.apply(a) {
                Ok(Outcome::Landed) => {
                    if is_irq {
                        irqs.landed += 1;
                    }
                }
                Ok(Outcome::Refused(why)) => {
                    if is_irq && irqs.first_refusal.is_none() {
                        irqs.first_refusal = Some(why);
                    }
                }
                Err(e) => {
                    eprintln!("apexd: game action failed ({}): {e:#}", a.describe());
                }
            }
        }

        let mut notes = plan.notes.clone();
        if irqs.refused() > 0 {
            notes.push(format!(
                "IRQ steering: {} of {} affinity writes were refused — {}",
                irqs.refused(),
                irqs.attempted,
                irqs
                    .first_refusal
                    .as_deref()
                    .unwrap_or("no reason recorded")
            ));
        }

        let session = GameSession {
            exit_actions: plan.exit.clone(),
            prior_tier,
            prior_auto_switch,
            prior_fan_mode,
            tier: cfg.tier,
            cpu_list: plan.cpu_list(),
            cpus: plan.cpus.clone(),
            core_source: topo.source.as_str().to_string(),
            pids: pids.to_vec(),
            gpus_locked: plan.gpus_locked.clone(),
            irqs,
            notes,
        };
        eprintln!(
            "apexd: game mode ON — cpus {} ({}), {}/{} IRQs steered, {} GPU(s) locked, tier {}",
            if session.cpu_list.is_empty() {
                "(unpinned)"
            } else {
                &session.cpu_list
            },
            session.core_source,
            session.irqs.landed,
            session.irqs.attempted,
            session.gpus_locked.len(),
            session.tier
        );
        for n in &session.notes {
            eprintln!("apexd: game: {n}");
        }
        *self.game.lock().await = Some(session);
        Ok(())
    }

    /// Leave game mode (idempotent). Every recorded value is put back.
    pub async fn game_exit(self: &Arc<Self>) -> Result<()> {
        let Some(session) = self.game.lock().await.take() else {
            return Ok(()); // not active: nothing to undo
        };

        for a in &session.exit_actions {
            if let Err(e) = self.writer.apply(a) {
                eprintln!("apexd: game exit action failed ({}): {e:#}", a.describe());
            }
        }

        if let Some(prior) = session.prior_fan_mode {
            if let Err(e) = self.fan.set_mode(prior).await {
                eprintln!("apexd: game: restoring fan mode failed: {e:#} — forcing firmware control");
                self.fan.restore().await;
            }
        }

        {
            let mut st = self.state.lock().await;
            st.auto_switch = session.prior_auto_switch;
        }
        if let Err(e) = self.apply_tier(session.prior_tier).await {
            eprintln!("apexd: game: restoring tier {} failed: {e:#}", session.prior_tier);
        }
        eprintln!(
            "apexd: game mode OFF — tier restored to {}, auto-switch {}",
            session.prior_tier,
            if session.prior_auto_switch { "on" } else { "off" }
        );
        Ok(())
    }

    /// Attach one more PID to a running session's cpuset.
    pub async fn game_attach(self: &Arc<Self>, pid: u32) -> Result<()> {
        let cfg = self.profile().game_config();
        let mut guard = self.game.lock().await;
        let Some(session) = guard.as_mut() else {
            bail!("game mode is not active");
        };
        if session.cpus.is_empty() {
            bail!("this session has no cpuset to attach to");
        }
        if session.pids.contains(&pid) {
            return Ok(());
        }
        let prior = game::read_pid_cgroup(Path::new("/proc"), CGROUP_ROOT, pid);
        if let Err(e) = self.writer.apply(&Action::CgroupAttach {
            path: cfg.cgroup.clone(),
            pid,
        }) {
            bail!("attaching pid {pid}: {e:#}");
        }
        // Restore this PID before the cgroup is torn down: the attach must land
        // ahead of the CgroupRemove that already sits at the end of the plan.
        if let Some(prior) = prior {
            let at = session
                .exit_actions
                .iter()
                .position(|a| matches!(a, Action::CgroupRemove { .. }))
                .unwrap_or(session.exit_actions.len());
            session
                .exit_actions
                .insert(at, Action::CgroupAttach { path: prior, pid });
        }
        session.pids.push(pid);
        Ok(())
    }

    /// The `GameMode.Status` payload.
    pub async fn game_status(self: &Arc<Self>) -> HashMap<String, OwnedValue> {
        let mut m: HashMap<String, OwnedValue> = HashMap::new();
        let cfg = self.profile().game_config();
        let guard = self.game.lock().await;
        insert(&mut m, "active", Value::from(guard.is_some()));
        insert(&mut m, "supported", Value::from(cfg.enabled));
        insert(&mut m, "cgroup", Value::from(cfg.cgroup.clone()));
        insert(&mut m, "cpuset_policy", Value::from(cfg.cpuset.clone()));
        insert(&mut m, "irq_policy", Value::from(cfg.irq.clone()));
        insert(&mut m, "tier", Value::from(cfg.tier.as_str()));
        match guard.as_ref() {
            Some(s) => {
                insert(&mut m, "cpus", Value::from(s.cpu_list.clone()));
                insert(&mut m, "core_source", Value::from(s.core_source.clone()));
                insert(&mut m, "prior_tier", Value::from(s.prior_tier.as_str()));
                // `irqs_steered` keeps its name and changes its meaning, on
                // purpose. It is the key the CLI and the shell already render,
                // and it used to hold the number of writes the plan INTENDED.
                // It now holds the number the kernel accepted. Renaming it
                // would leave the old name reading as a measurement somewhere;
                // reporting both under one name would be the same lie with
                // more words. The two flanking keys are what makes a partial
                // result legible — some interrupts are unmovable on some
                // hardware, and that is normal rather than a failure.
                insert(&mut m, "irqs_steered", Value::from(s.irqs.landed as u32));
                insert(&mut m, "irqs_attempted", Value::from(s.irqs.attempted as u32));
                insert(&mut m, "irqs_refused", Value::from(s.irqs.refused() as u32));
                insert(
                    &mut m,
                    "gpus_locked",
                    Value::from(s.gpus_locked.clone()),
                );
                insert(&mut m, "pids", Value::from(s.pids.clone()));
                insert(&mut m, "notes", Value::from(s.notes.clone()));
            }
            None => {
                // Not active: report what a session *would* look like.
                let topo = CoreTopology::detect_from(&self.sys_root);
                insert(&mut m, "cpus", Value::from(String::new()));
                insert(&mut m, "core_source", Value::from(topo.source.as_str()));
                insert(&mut m, "pcores", Value::from(topo.pcore_list()));
                insert(&mut m, "ecores", Value::from(topo.ecore_list()));
                insert(
                    &mut m,
                    "nvidia_smi",
                    Value::from(apexd_core::gpu::nvidia_smi_available()),
                );
            }
        }
        m
    }
}

fn insert(m: &mut HashMap<String, OwnedValue>, key: &str, v: Value<'_>) {
    if let Ok(owned) = v.try_to_owned() {
        m.insert(key.to_string(), owned);
    }
}

#[cfg(test)]
mod tests {
    //! The half of "enter/exit is symmetric" that lives in the daemon rather
    //! than in sysfs: the tier and the auto-switch flag.
    //!
    //! `apexd-core`'s fixture tests prove the filesystem is restored;
    //! these prove the daemon state is, including the rule that prior state is
    //! captured only on the 0 -> 1 transition. Everything runs against a
    //! `MockWriter` and a temp-dir sysfs root, so nothing real is touched.
    //!
    //! The second group proves what `apex game status` reports about IRQ
    //! steering, because status used to report the PLAN: on a machine that
    //! refuses every affinity write it said "N IRQs steered" having steered
    //! none. Those run against a fixture procfs and a writer that refuses, so
    //! the refusal is produced rather than hoped for, and no interrupt on the
    //! machine running the suite is enumerated, let alone moved.

    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};

    use apexd_core::gpu::MockNvidiaSmi;
    use apexd_core::syswriter::{MockWriter, Outcome, SysWriter};
    use apexd_core::{ProfileSet, Selection};

    use super::*;
    use crate::state::{Ctx, State};
    use apexd_core::tier::Tier;

    /// A profile whose game mode changes nothing outside the daemon: no
    /// cpuset, no IRQ steering, no NVIDIA. What is left is exactly the tier
    /// and auto-switch behaviour under test.
    const PROFILE: &str = r#"
        id = "test-game"
        kind = "device"
        [defaults]
        ac = "balanced"
        battery = "power-saver"
        [tiers.performance]
        governor = "performance"
        [tiers.balanced]
        governor = "powersave"
        [tiers.power-saver]
        governor = "powersave"
        [gamemode]
        tier = "performance"
        cpuset = "off"
        irq = "off"
        [gamemode.nvidia]
        enabled = false
    "#;

    fn scratch(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "apexd-game-ctx-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::remove_dir_all(&root).ok();
        root
    }

    fn ctx(tag: &str) -> Arc<Ctx> {
        let root = scratch(tag);
        // An empty sysfs root and an empty procfs IRQ root: no fans, no CPUs,
        // no interrupts, nothing to discover.
        build_ctx(&root, PROFILE, &root.join("no-irqs"), Arc::new(MockWriter::new()))
    }

    fn build_ctx(
        root: &Path,
        profile: &str,
        proc_irq_root: &Path,
        writer: Arc<dyn SysWriter>,
    ) -> Arc<Ctx> {
        std::fs::create_dir_all(root.join("profiles")).unwrap();
        std::fs::write(root.join("profiles/test-game.toml"), profile).unwrap();

        let set = ProfileSet::load(Some(&root.join("profiles"))).unwrap();
        let selection = Selection {
            generic: "test-game".into(),
            class: None,
            device: Some("test-game".into()),
            active: "test-game".into(),
        };
        let fingerprint = apexd_core::Fingerprint::detect_from(root, root);
        Ctx::new(
            set,
            selection,
            fingerprint,
            writer,
            false,
            State {
                tier: Tier::Balanced,
                auto_switch: true,
                on_ac: true,
                travel_mode: false,
                charge_start: 0,
                charge_stop: 100,
            },
            root,
            proc_irq_root,
            Arc::new(MockNvidiaSmi::default()),
        )
    }

    #[tokio::test]
    async fn enter_holds_the_game_tier_and_exit_puts_everything_back() {
        let ctx = ctx("roundtrip");
        assert!(!ctx.game_active().await);

        ctx.game_enter(&[]).await.unwrap();
        {
            let st = ctx.state.lock().await;
            assert_eq!(
                st.tier,
                Tier::Performance,
                "the session holds the profile's game tier"
            );
            assert!(
                !st.auto_switch,
                "auto-switch must be off, or an AC transition would clobber the game tier"
            );
        }
        assert!(ctx.game_active().await);

        ctx.game_exit().await.unwrap();
        let st = ctx.state.lock().await;
        assert_eq!(st.tier, Tier::Balanced, "the tier the session interrupted comes back");
        assert!(st.auto_switch, "and so does auto-switch");
        drop(st);
        assert!(!ctx.game_active().await);
    }

    #[tokio::test]
    async fn a_second_enter_cannot_overwrite_the_recorded_prior_state() {
        let ctx = ctx("double-enter");
        ctx.game_enter(&[]).await.unwrap();
        // At this point tier == performance and auto_switch == false. A second
        // enter must NOT record those as the values to restore.
        ctx.game_enter(&[]).await.unwrap();
        ctx.game_enter(&[]).await.unwrap();

        ctx.game_exit().await.unwrap();
        let st = ctx.state.lock().await;
        assert_eq!(st.tier, Tier::Balanced);
        assert!(st.auto_switch);
    }

    #[tokio::test]
    async fn exit_without_a_session_is_a_no_op() {
        let ctx = ctx("exit-idle");
        {
            let mut st = ctx.state.lock().await;
            st.tier = Tier::Performance;
            st.auto_switch = false;
        }
        ctx.game_exit().await.unwrap();
        ctx.game_exit().await.unwrap();
        let st = ctx.state.lock().await;
        assert_eq!(st.tier, Tier::Performance, "an idle exit changes nothing");
        assert!(!st.auto_switch);
    }

    #[tokio::test]
    async fn exit_is_idempotent_after_a_real_session() {
        let ctx = ctx("exit-twice");
        ctx.game_enter(&[]).await.unwrap();
        ctx.game_exit().await.unwrap();
        {
            let mut st = ctx.state.lock().await;
            st.tier = Tier::PowerSaver; // something else moves the tier afterwards
        }
        ctx.game_exit().await.unwrap();
        let st = ctx.state.lock().await;
        assert_eq!(
            st.tier,
            Tier::PowerSaver,
            "a second exit must not re-apply the restored tier"
        );
    }

    #[tokio::test]
    async fn attach_requires_an_active_session() {
        let ctx = ctx("attach");
        assert!(ctx.game_attach(4242).await.is_err());
        ctx.game_enter(&[]).await.unwrap();
        // cpuset = "off": there is no cgroup to attach to, and saying so beats
        // pretending the PID was pinned.
        assert!(ctx.game_attach(4242).await.is_err());
        ctx.game_exit().await.unwrap();
    }

    // ── what status says about IRQ steering ─────────────────────────────────
    //
    // `apex game status` reported `plan.irqs_attempted` under the name
    // `irqs_steered`: the number of affinity writes the plan CONTAINED, printed
    // as though it had been measured. Every write is tolerated — a
    // kernel-managed MSI-X queue returns -EIO — and the applier threw the
    // outcome away, so on a machine that refused every one of them status still
    // said "N IRQs steered".

    /// A profile that really does steer interrupts: P-core cpuset, IRQ steering
    /// on, nothing else. `scx = ""` keeps a host command out of the plan and
    /// `cpuset_mems` keeps the daemon from reading the real cgroup root.
    const PROFILE_STEER: &str = r#"
        id = "test-game"
        kind = "device"
        [defaults]
        ac = "balanced"
        battery = "power-saver"
        [tiers.performance]
        governor = "performance"
        [tiers.balanced]
        governor = "powersave"
        [tiers.power-saver]
        governor = "powersave"
        [gamemode]
        tier = "performance"
        cpuset = "p-cores"
        cpuset_mems = "0"
        irq = "away-from-game"
        scx = ""
        [gamemode.nvidia]
        enabled = false
    "#;

    /// A hybrid machine with three interrupts, as sysfs and procfs present it.
    ///
    /// Two of the three are steerable: IRQ 0 is the timer, which the planner
    /// never touches. So a correct run attempts exactly 2 affinity writes, and
    /// that number being fixed by the fixture rather than by the host is what
    /// makes "landed" and "refused" comparable to it.
    fn hybrid_machine(root: &Path) -> PathBuf {
        std::fs::create_dir_all(root.join("devices/system/cpu")).unwrap();
        std::fs::write(root.join("devices/system/cpu/online"), "0-19\n").unwrap();
        std::fs::create_dir_all(root.join("devices/cpu_core")).unwrap();
        std::fs::write(root.join("devices/cpu_core/cpus"), "0-11\n").unwrap();
        std::fs::create_dir_all(root.join("devices/cpu_atom")).unwrap();
        std::fs::write(root.join("devices/cpu_atom/cpus"), "12-19\n").unwrap();

        let irq_root = root.join("proc-irq");
        for (n, handler) in [(0u32, "timer"), (16, "nvidia"), (24, "xhci_hcd")] {
            std::fs::create_dir_all(irq_root.join(n.to_string()).join(handler)).unwrap();
            std::fs::write(
                irq_root.join(n.to_string()).join("smp_affinity_list"),
                "0-19\n",
            )
            .unwrap();
        }
        irq_root
    }

    /// The machine the bug was invisible on: every affinity write comes back
    /// `-EIO`, everything else succeeds.
    ///
    /// Deliberately not a `MockWriter` option. The mock's contract is that it
    /// records the plan exactly as issued and reports success, which is what
    /// every other test in this file leans on; a mock that could also refuse
    /// would make those assertions ambiguous about which behaviour they were
    /// pinning.
    #[derive(Default)]
    struct RefusesEveryIrqWrite {
        applied: Mutex<Vec<Action>>,
    }

    impl SysWriter for RefusesEveryIrqWrite {
        fn apply(&self, action: &Action) -> anyhow::Result<Outcome> {
            self.applied.lock().unwrap().push(action.clone());
            Ok(match action {
                Action::IrqAffinity { .. } => Outcome::Refused(
                    "irq affinity: Input/output error (os error 5)".to_string(),
                ),
                _ => Outcome::Landed,
            })
        }
    }

    fn u32_of(m: &HashMap<String, OwnedValue>, key: &str) -> u32 {
        let v = m
            .get(key)
            .unwrap_or_else(|| panic!("status has no '{key}' key: {:?}", m.keys()));
        u32::try_from(v).unwrap_or_else(|e| panic!("'{key}' is not a u32: {e}"))
    }

    /// The `notes` array, unwrapped the way `apex game status`'s own renderer
    /// unwraps it — so the assertions below are over the lines a user reads.
    fn notes_of(m: &HashMap<String, OwnedValue>) -> Vec<String> {
        let Value::Array(a) = &**m.get("notes").expect("status has a notes key") else {
            panic!("notes is not an array");
        };
        a.iter()
            .map(|v| match v {
                Value::Str(s) => s.to_string(),
                other => format!("{other:?}"),
            })
            .collect()
    }

    #[tokio::test]
    async fn status_reports_zero_steered_when_the_machine_refuses_every_write() {
        let root = scratch("irq-refused");
        let irq_root = hybrid_machine(&root);
        let writer = Arc::new(RefusesEveryIrqWrite::default());
        let ctx = build_ctx(&root, PROFILE_STEER, &irq_root, writer.clone());

        ctx.game_enter(&[]).await.unwrap();
        let status = ctx.game_status().await;

        // The negative control: the plan really did contain the writes, so a
        // zero below is a refusal and not an empty plan.
        let attempted = u32_of(&status, "irqs_attempted");
        assert_eq!(
            attempted, 2,
            "the fixture's two steerable interrupts must both be attempted"
        );
        assert_eq!(
            writer
                .applied
                .lock()
                .unwrap()
                .iter()
                .filter(|a| matches!(a, Action::IrqAffinity { .. }))
                .count(),
            2,
            "and the writer must actually have been asked to perform them"
        );

        assert_eq!(
            u32_of(&status, "irqs_steered"),
            0,
            "not one write landed, so status must not claim any interrupt was steered"
        );
        assert_eq!(u32_of(&status, "irqs_refused"), 2);

        let notes = notes_of(&status);
        assert!(
            notes.iter().any(|n| n.contains("2 of 2") && n.contains("refused")),
            "status must say how many were refused: {notes:?}"
        );
        assert!(
            notes.iter().any(|n| n.contains("Input/output error")),
            "and why, when the writer knows: {notes:?}"
        );

        ctx.game_exit().await.unwrap();
        std::fs::remove_dir_all(&root).ok();
    }

    #[tokio::test]
    async fn status_reports_what_landed_when_the_machine_accepts_the_writes() {
        // The other half: the report is not hardwired to zero. Same fixture,
        // same plan, a writer that accepts — and now the count is the count.
        let root = scratch("irq-landed");
        let irq_root = hybrid_machine(&root);
        let ctx = build_ctx(&root, PROFILE_STEER, &irq_root, Arc::new(MockWriter::new()));

        ctx.game_enter(&[]).await.unwrap();
        let status = ctx.game_status().await;
        assert_eq!(u32_of(&status, "irqs_attempted"), 2);
        assert_eq!(u32_of(&status, "irqs_steered"), 2);
        assert_eq!(u32_of(&status, "irqs_refused"), 0);
        assert!(
            !notes_of(&status).iter().any(|n| n.contains("refused")),
            "nothing was refused, so nothing must say so"
        );

        ctx.game_exit().await.unwrap();
        std::fs::remove_dir_all(&root).ok();
    }

    #[tokio::test]
    async fn a_partial_refusal_is_reported_as_a_partial_result() {
        // The normal case on real hardware: some interrupts move, some are
        // kernel-managed and do not. Reporting either extreme would be a lie.
        #[derive(Default)]
        struct RefusesOneIrqWrite {
            seen: Mutex<usize>,
        }
        impl SysWriter for RefusesOneIrqWrite {
            fn apply(&self, action: &Action) -> anyhow::Result<Outcome> {
                if !matches!(action, Action::IrqAffinity { .. }) {
                    return Ok(Outcome::Landed);
                }
                let mut seen = self.seen.lock().unwrap();
                *seen += 1;
                Ok(if *seen == 1 {
                    Outcome::Refused("irq affinity: Input/output error (os error 5)".into())
                } else {
                    Outcome::Landed
                })
            }
        }

        let root = scratch("irq-partial");
        let irq_root = hybrid_machine(&root);
        let ctx = build_ctx(
            &root,
            PROFILE_STEER,
            &irq_root,
            Arc::new(RefusesOneIrqWrite::default()),
        );

        ctx.game_enter(&[]).await.unwrap();
        let status = ctx.game_status().await;
        assert_eq!(u32_of(&status, "irqs_attempted"), 2);
        assert_eq!(u32_of(&status, "irqs_steered"), 1);
        assert_eq!(u32_of(&status, "irqs_refused"), 1);
        assert!(
            notes_of(&status)
                .iter()
                .any(|n| n.contains("1 of 2") && n.contains("refused")),
            "a partial result must read as a partial result"
        );

        ctx.game_exit().await.unwrap();
        std::fs::remove_dir_all(&root).ok();
    }

    #[tokio::test]
    async fn status_needs_no_session_and_writes_nothing() {
        // `apex game status` must stay read-only and root-free. It reads a
        // recorded fact rather than recomputing a plan, so with no session
        // there is no IRQ number to report at all — reporting a plan's count
        // here is exactly the thing that was wrong.
        let root = scratch("irq-idle");
        let irq_root = hybrid_machine(&root);
        let ctx = build_ctx(&root, PROFILE_STEER, &irq_root, Arc::new(MockWriter::new()));

        let status = ctx.game_status().await;
        for key in ["irqs_steered", "irqs_attempted", "irqs_refused"] {
            assert!(
                !status.contains_key(key),
                "an idle machine has steered nothing; '{key}' must not be reported"
            );
        }
        assert_eq!(
            std::fs::read_to_string(irq_root.join("24/smp_affinity_list")).unwrap(),
            "0-19\n",
            "status must not have touched an interrupt"
        );
        std::fs::remove_dir_all(&root).ok();
    }
}
