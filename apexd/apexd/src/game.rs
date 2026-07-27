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

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use anyhow::{bail, Result};
use apexd_core::fan::FanMode;
use apexd_core::game::{self, GameInputs, PidPlacement, CGROUP_ROOT};
use apexd_core::irq::{self, PROC_IRQ};
use apexd_core::tier::{Action, Tier};
use apexd_core::topology::CoreTopology;
use zvariant::{OwnedValue, Value};

use crate::state::Ctx;

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
    pub irqs_steered: usize,
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
        let irqs = irq::enumerate(Path::new(PROC_IRQ));
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

        for a in &plan.enter {
            if let Err(e) = self.writer.apply(a) {
                eprintln!("apexd: game action failed ({}): {e:#}", a.describe());
            }
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
            irqs_steered: plan.irqs_steered,
            notes: plan.notes.clone(),
        };
        eprintln!(
            "apexd: game mode ON — cpus {} ({}), {} IRQs steered, {} GPU(s) locked, tier {}",
            if session.cpu_list.is_empty() {
                "(unpinned)"
            } else {
                &session.cpu_list
            },
            session.core_source,
            session.irqs_steered,
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
                insert(&mut m, "irqs_steered", Value::from(s.irqs_steered as u32));
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

    use std::sync::Arc;

    use apexd_core::gpu::MockNvidiaSmi;
    use apexd_core::syswriter::{MockWriter, SysWriter};
    use apexd_core::{ProfileSet, Selection};

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

    fn ctx(tag: &str) -> Arc<Ctx> {
        let root = std::env::temp_dir().join(format!(
            "apexd-game-ctx-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::remove_dir_all(&root).ok();
        std::fs::create_dir_all(root.join("profiles")).unwrap();
        std::fs::write(root.join("profiles/test-game.toml"), PROFILE).unwrap();

        let set = ProfileSet::load(Some(&root.join("profiles"))).unwrap();
        let selection = Selection {
            generic: "test-game".into(),
            class: None,
            device: Some("test-game".into()),
            active: "test-game".into(),
        };
        // An empty sysfs root: no fans, no CPUs, nothing to discover.
        let fingerprint = apexd_core::Fingerprint::detect_from(&root, &root);
        let writer: Arc<dyn SysWriter> = Arc::new(MockWriter::new());
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
}
