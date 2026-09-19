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
use apexd_core::game::{
    self, owner_for_pid, owner_state, GameInputs, OwnerState, PidPlacement, SessionOwner,
    CGROUP_ROOT,
};
use apexd_core::irq;
use apexd_core::syswriter::{Outcome, ScxState};
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

/// What actually happened to the sched-ext scheduler the profile asked for.
///
/// The IRQ report above closed this defect for affinity writes; sched-ext had
/// the same one and kept it for three shipped images. `apex game status`'s
/// `notes` said "sched-ext: scx_lavd for the session" — which was a sentence
/// copied out of the PLAN — while the journal line directly above it recorded
/// `scxctl` refusing the call. Nobody could have noticed, because the only
/// surface that reported it reported the intention.
///
/// So three fields, and they answer three different questions:
/// what was asked for, what the command said, and what the kernel says. The
/// last one is authoritative: a command's exit code is a fact about the
/// command.
#[derive(Debug, Clone, Default)]
pub struct ScxReport {
    /// The scheduler the profile asked for. `None` = it asked for none, which
    /// is the default for every profile that is not a gaming one.
    pub requested: Option<String>,
    /// What applying [`Action::ScxSwitch`] returned, in the writer's words.
    /// `None` only when nothing was asked for.
    pub applied: Option<Outcome>,
    /// What the kernel reported afterwards, read by the daemon rather than
    /// inferred from `applied`.
    pub observed: Option<ScxState>,
}

impl ScxReport {
    /// One word: `not requested`, `loaded`, `not loaded` or `unknown`.
    ///
    /// **`loaded` requires a kernel reading.** A command that exited 0 with no
    /// confirmable state is `unknown`, not `loaded` — that collapse is the
    /// whole defect. And an `Unsupported` kernel is `not loaded` rather than
    /// `unknown`, because "no scheduler can attach here" is a definite answer.
    pub fn verdict(&self) -> &'static str {
        if self.requested.is_none() {
            return "not requested";
        }
        match &self.observed {
            Some(st) => st.verdict(),
            // No reading at all. The command's own word is the most that can
            // be said, and it is never enough to say "loaded".
            None => match &self.applied {
                Some(Outcome::Refused(_)) => "not loaded",
                _ => "unknown",
            },
        }
    }

    /// The sentence `apex game status` prints, naming both halves so a reader
    /// can see which one disagreed.
    pub fn detail(&self) -> String {
        let Some(want) = &self.requested else {
            return "this profile asks for no sched-ext scheduler".to_string();
        };
        let said = match &self.applied {
            Some(Outcome::Landed) => "scxctl reported success".to_string(),
            Some(Outcome::Refused(why)) => format!("scxctl refused: {why}"),
            Some(Outcome::Unknown(why)) => format!("scxctl could not be confirmed: {why}"),
            None => "the switch was never applied".to_string(),
        };
        let seen = match &self.observed {
            Some(st) => st.describe(),
            None => "and the kernel was not read back".to_string(),
        };
        // The mismatch is worth its own clause: "attached, but not the one
        // asked for" is a different problem from "not attached", and the
        // kernel's struct_ops name drops the `scx_` prefix, so the comparison
        // has to be the forgiving one or it invents a failure.
        let mismatch = match &self.observed {
            Some(ScxState::Enabled { ops: Some(o) })
                if !apexd_core::syswriter::scx_ops_matches(o, want) =>
            {
                format!(" — NOTE: '{o}' is not the scheduler that was asked for")
            }
            _ => String::new(),
        };
        format!("asked for {want}; {said}; {seen}{mismatch}")
    }

    /// The single note the session carries, or `None` when nothing was asked.
    pub fn note(&self) -> Option<String> {
        self.requested.as_ref()?;
        Some(format!("sched-ext: {} — {}", self.verdict(), self.detail()))
    }
}

/// What applying the NVIDIA clock-lock actions did.
///
/// The same shape as [`IrqReport`], and it is here for the same reason found
/// the same way: `gpus_locked` in `apex game status` was
/// `plan.gpus_locked.clone()` — the list of GPUs the plan MEANT to lock — and
/// `run_nvidia_smi` has been returning a perfectly good `Outcome::Refused`
/// that nothing read. A GPU whose clock lock `nvidia-smi` rejected was still
/// listed as locked.
#[derive(Debug, Clone, Default)]
pub struct GpuLockReport {
    /// GPUs the plan asked to lock.
    pub attempted: Vec<u32>,
    /// GPUs every one of whose lock writes landed.
    pub landed: Vec<u32>,
    /// The first refusal, so status can say why.
    pub first_refusal: Option<String>,
}

impl GpuLockReport {
    /// GPUs asked for whose locks did not all land.
    pub fn refused(&self) -> Vec<u32> {
        self.attempted
            .iter()
            .copied()
            .filter(|g| !self.landed.contains(g))
            .collect()
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
    /// Measured at enter time from the writer's own outcomes, NOT copied from
    /// the plan. See [`GpuLockReport`].
    pub gpus: GpuLockReport,
    /// Measured at enter time from the writer's own outcomes.
    pub irqs: IrqReport,
    /// What the sched-ext switch actually did. See [`ScxReport`].
    pub scx: ScxReport,
    /// What sched-ext looked like BEFORE this session, so the exit path can
    /// say whether it is stopping something game mode started or something
    /// that was already running.
    pub prior_scx: ScxState,
    pub notes: Vec<String>,
    /// The process whose death ends this session, when the caller named one.
    ///
    /// `None` is a session nothing is watching — `apex game start` typed by
    /// hand, or the shell's power menu — and it behaves exactly as it always
    /// has: it stays on until something asks for it to stop.
    pub owner: Option<SessionOwner>,
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
        self.game_enter_owned(pids, None).await
    }

    /// Enter game mode and, when `owner_pid` is given, record the process whose
    /// death ends the session (see [`apexd_core::game::SessionOwner`]).
    ///
    /// **Ownership and cpuset placement are different questions and this does
    /// not conflate them.** `owner_pid` is watched, not pinned: pinning the
    /// Gaming Mode session script would put every Steam process on the p-core
    /// cpuset, which is a behaviour change the katana run did not measure.
    /// `StartForPid`/`AttachPid` remain the way to pin.
    ///
    /// **Adoption rule, stated because undefined is worse than either
    /// choice:** a session that already has an owner keeps its first one. Two
    /// callers claiming the same session is a bug in the callers; the first
    /// claim is the one that entered game mode, so it wins.
    pub async fn game_enter_owned(
        self: &Arc<Self>,
        pids: &[u32],
        owner_pid: Option<u32>,
    ) -> Result<()> {
        let cfg = self.profile().game_config();
        if !cfg.enabled {
            bail!("game mode is disabled for profile '{}'", self.selection.active);
        }

        {
            let mut existing = self.game.lock().await;
            if let Some(session) = existing.as_mut() {
                if session.owner.is_none() {
                    if let Some(pid) = owner_pid {
                        session.owner = owner_for_pid(&self.proc_root, pid);
                    }
                }
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

        // Every DRM card, with what its controls read RIGHT NOW: the exit plan
        // restores these exact values, so they have to be captured before the
        // enter plan is applied and not re-read afterwards.
        let gpus: Vec<(apexd_core::gpu::GpuDevice, apexd_core::gpu::SysfsGpuPrior)> =
            apexd_core::gpu::discover(&self.sys_root)
                .into_iter()
                .map(|d| {
                    let prior = apexd_core::gpu::read_prior(&d);
                    (d, prior)
                })
                .collect();

        let plan = game::plan(&GameInputs {
            cfg: &cfg,
            topo: &topo,
            nvidia: &nvidia,
            gpus: &gpus,
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
        // Read BEFORE the enter plan runs. If something was already attached,
        // the exit plan's `scxctl stop` removes a scheduler this session did
        // not start, and the exit log should say so rather than call it a
        // restore. (The plan cannot know: it is built without touching the
        // machine, which is what makes it testable.)
        let prior_scx = self.writer.scx_state();

        let mut irqs = IrqReport::default();
        let mut gpus = GpuLockReport {
            attempted: plan.gpus_locked.clone(),
            ..Default::default()
        };
        // GPUs whose every clock-lock write landed. Tracked as "did any write
        // for this GPU fail" rather than "did any succeed", because a card
        // whose graphics clock locked and whose memory clock did not is not a
        // locked card, and reporting it as one is the defect in miniature.
        let mut gpu_failed: Vec<u32> = Vec::new();
        let mut scx = ScxReport {
            requested: plan.enter.iter().find_map(|a| match a {
                Action::ScxSwitch { sched } => Some(sched.clone()),
                _ => None,
            }),
            ..Default::default()
        };
        for a in &plan.enter {
            let is_irq = matches!(a, Action::IrqAffinity { .. });
            if is_irq {
                irqs.attempted += 1;
            }
            let gpu_of = match a {
                Action::NvidiaLockGraphics { gpu, .. } | Action::NvidiaLockMemory { gpu, .. } => {
                    Some(*gpu)
                }
                _ => None,
            };
            let outcome = self.writer.apply(a);
            if matches!(a, Action::ScxSwitch { .. }) {
                scx.applied = match &outcome {
                    Ok(o) => Some(o.clone()),
                    // A hard error is not a refusal and not an unknown: the
                    // action blew up. Record it in the same shape so the
                    // status surface has something to print.
                    Err(e) => Some(Outcome::Refused(format!("{e:#}"))),
                };
            }
            match outcome {
                Ok(Outcome::Landed) => {
                    if is_irq {
                        irqs.landed += 1;
                    }
                }
                Ok(Outcome::Refused(why)) | Ok(Outcome::Unknown(why)) => {
                    if is_irq && irqs.first_refusal.is_none() {
                        irqs.first_refusal = Some(why.clone());
                    }
                    if let Some(g) = gpu_of {
                        if !gpu_failed.contains(&g) {
                            gpu_failed.push(g);
                        }
                        if gpus.first_refusal.is_none() {
                            gpus.first_refusal = Some(why);
                        }
                    }
                }
                Err(e) => {
                    eprintln!("apexd: game action failed ({}): {e:#}", a.describe());
                    if let Some(g) = gpu_of {
                        if !gpu_failed.contains(&g) {
                            gpu_failed.push(g);
                        }
                        if gpus.first_refusal.is_none() {
                            gpus.first_refusal = Some(format!("{e:#}"));
                        }
                    }
                }
            }
        }
        gpus.landed = gpus
            .attempted
            .iter()
            .copied()
            .filter(|g| !gpu_failed.contains(g))
            .collect();

        // Ask the KERNEL what sched-ext looks like now, independently of what
        // `scxctl` said it did. This is a second party reading the same fact,
        // which is the only thing that catches a command that succeeds and
        // changes nothing — the exact shape of the defect on katana.
        if scx.requested.is_some() {
            scx.observed = Some(self.writer.scx_state());
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
        if let Some(n) = scx.note() {
            notes.push(n);
        }
        if !gpus.refused().is_empty() {
            notes.push(format!(
                "GPU clock locks: {:?} of {:?} were refused — {}",
                gpus.refused(),
                gpus.attempted,
                gpus.first_refusal.as_deref().unwrap_or("no reason recorded")
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
            gpus,
            irqs,
            scx,
            prior_scx,
            notes,
            owner: owner_pid.and_then(|pid| owner_for_pid(&self.proc_root, pid)),
        };
        // Every number in this line is now a measurement. `GPU(s) locked` used
        // to be the plan's count and `sched-ext` was not reported at all; both
        // said more than anyone had checked.
        eprintln!(
            "apexd: game mode ON — cpus {} ({}), {}/{} IRQs steered, {}/{} GPU(s) locked, \
             sched-ext {}, tier {}",
            if session.cpu_list.is_empty() {
                "(unpinned)"
            } else {
                &session.cpu_list
            },
            session.core_source,
            session.irqs.landed,
            session.irqs.attempted,
            session.gpus.landed.len(),
            session.gpus.attempted.len(),
            session.scx.verdict(),
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

        // The exit plan's outcomes used to be discarded wholesale — only a hard
        // `Err` was logged, so a `Refused` restore was silent and the "game
        // mode OFF" line below asserted a restoration nobody had read. That is
        // the same defect as the enter path's, and it matters more here: this
        // is the plan that puts the machine back.
        let mut exit_refusals: Vec<String> = Vec::new();
        for a in &session.exit_actions {
            match self.writer.apply(a) {
                Ok(Outcome::Landed) => {}
                Ok(Outcome::Refused(why)) => {
                    eprintln!("apexd: game exit: {} refused — {why}", a.describe());
                    exit_refusals.push(format!("{}: {why}", a.describe()));
                }
                Ok(Outcome::Unknown(why)) => {
                    eprintln!("apexd: game exit: {} unconfirmed — {why}", a.describe());
                    exit_refusals.push(format!("{}: {why}", a.describe()));
                }
                Err(e) => {
                    eprintln!("apexd: game exit action failed ({}): {e:#}", a.describe());
                    exit_refusals.push(format!("{}: {e:#}", a.describe()));
                }
            }
        }
        if session.scx.requested.is_some() {
            // Now that the switch actually works, the stop actually does
            // something — and what it does depends on what was running BEFORE
            // the session. On an APEX image nothing loads a scheduler at boot,
            // so `stop` is the right verb and this is a restore. If something
            // WAS already attached, `stop` removed a scheduler game mode did
            // not start, which is a different event and is named as one rather
            // than logged as a successful restore.
            let after = self.writer.scx_state();
            match &session.prior_scx {
                ScxState::Enabled { ops } => eprintln!(
                    "apexd: game: sched-ext was already running before this session ({}) \
                     and exit STOPPED it rather than putting it back — scheduling is now: {}",
                    ops.clone().unwrap_or_else(|| "name not published".into()),
                    after.describe()
                ),
                _ => eprintln!("apexd: game: sched-ext after exit: {}", after.describe()),
            }
        }
        if !exit_refusals.is_empty() {
            eprintln!(
                "apexd: game: {} exit action(s) did not land — first: {}",
                exit_refusals.len(),
                exit_refusals[0]
            );
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

    /// The live session's owner, if it has one.
    pub async fn game_owner(&self) -> Option<SessionOwner> {
        self.game.lock().await.as_ref().and_then(|s| s.owner)
    }

    /// One tick of the owner watch: release game mode if the process that
    /// asked for it has died.
    ///
    /// This is the whole fix for the katana 2026-09-19 defect (evidence §3.4).
    /// The session script's EXIT trap cannot release game mode once logind has
    /// deactivated the session, because `apex game stop` is a polkit
    /// `allow_active=yes` action. The daemon is root and asks polkit nothing
    /// about itself, so it can — and it is the only party that still exists
    /// after a `systemctl restart greetd` has taken the session away.
    ///
    /// Returns what it did, so the caller can log it and emit the same signals
    /// a D-Bus-driven exit emits. `Ok(None)` is "nothing to do"; an
    /// unanswerable reading is `Err` and NEVER a release.
    pub async fn game_release_if_owner_gone(self: &Arc<Self>) -> Result<Option<String>> {
        let Some(owner) = self.game_owner().await else {
            return Ok(None);
        };
        match owner_state(&self.proc_root, &owner) {
            OwnerState::Alive => Ok(None),
            OwnerState::Unknown(why) => bail!("{why}"),
            OwnerState::Gone(why) => {
                eprintln!(
                    "apexd: game: the session owner is gone ({why}) — releasing game mode. \
                     Nothing else could: `apex game stop` from a deactivated session is \
                     refused by polkit."
                );
                self.game_exit().await?;
                Ok(Some(why))
            }
        }
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
                // Same correction as `irqs_steered`, one action family over:
                // this used to be the plan's list. It is now the GPUs whose
                // clock locks nvidia-smi actually accepted, with the asked-for
                // list beside it so a partial result is legible instead of
                // invisible.
                insert(&mut m, "gpus_locked", Value::from(s.gpus.landed.clone()));
                insert(
                    &mut m,
                    "gpus_lock_attempted",
                    Value::from(s.gpus.attempted.clone()),
                );
                // ── sched-ext, as three answers rather than one claim ───────
                //
                // `scx_state` is `loaded`, `not loaded`, `unknown` or `not
                // requested`, and `loaded` requires a reading of
                // /sys/kernel/sched_ext — never an exit code. Before this,
                // Gaming Mode's only sched-ext surface was a `notes` line
                // copied out of the plan, which said the scheduler was in
                // force on three images where it had never once loaded.
                insert(
                    &mut m,
                    "scx_requested",
                    Value::from(s.scx.requested.clone().unwrap_or_default()),
                );
                insert(&mut m, "scx_state", Value::from(s.scx.verdict().to_string()));
                insert(&mut m, "scx_detail", Value::from(s.scx.detail()));
                insert(&mut m, "pids", Value::from(s.pids.clone()));
                insert(&mut m, "notes", Value::from(s.notes.clone()));
                // `owner_pid` is 0 when nothing is watching this session. It is
                // reported because "can this session release itself if its
                // login is destroyed" is the question the katana run could not
                // answer from the outside, and it should be one property read
                // away from now on.
                insert(
                    &mut m,
                    "owner_pid",
                    Value::from(s.owner.map(|o| o.pid).unwrap_or(0)),
                );
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
                // What sched-ext looks like with no session running, so
                // "disabled here, disabled during the session" is visible as
                // the non-answer it is rather than read as a passing row. The
                // katana qualification quoted sched_ext/state as a release
                // discriminator when it had read `disabled` throughout.
                let live = apexd_core::syswriter::read_scx_state(&self.sys_root);
                insert(
                    &mut m,
                    "scx_requested",
                    Value::from(cfg.scx.trim().to_string()),
                );
                insert(&mut m, "scx_state", Value::from(live.verdict().to_string()));
                insert(&mut m, "scx_detail", Value::from(live.describe()));
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
            root.join("proc"),
            Arc::new(MockNvidiaSmi::default()),
        )
    }

    /// [`build_ctx`] with an explicit `nvidia-smi`, for the tests that need a
    /// GPU to exist before there is anything to lock.
    fn build_ctx_smi(
        root: &Path,
        profile: &str,
        proc_irq_root: &Path,
        writer: Arc<dyn SysWriter>,
        smi: Arc<dyn apexd_core::gpu::NvidiaSmi>,
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
            root.join("proc"),
            smi,
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

    // ── the owner watch: releasing a session nobody can stop ────────────────
    //
    // The property these hold is the one the katana 2026-09-19 run could not
    // get back (evidence §3.4): **the machine returns to its normal tier,
    // scheduler and auto-switch after its Gaming Mode session is destroyed
    // WITHOUT COOPERATION** — no trap ran, no `apex game stop`, nothing asked.
    // Not "a function was called": every one below asserts the restore
    // actually landed in the writer and in the daemon's own state.

    /// A `/proc/<pid>/stat` line in the kernel's shape (field 2 is `(comm)`).
    fn stat_line(pid: u32, starttime: u64, state: char) -> String {
        let filler: Vec<String> = (4..=21).map(|n| n.to_string()).collect();
        format!("{pid} (apex-gaming-ses) {state} {} {starttime} 0 0 0", filler.join(" "))
    }

    fn spawn_fake_owner(ctx: &Arc<Ctx>, pid: u32, starttime: u64) {
        let dir = ctx.proc_root.join(pid.to_string());
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("stat"), stat_line(pid, starttime, 'S')).unwrap();
    }

    fn kill_fake_owner(ctx: &Arc<Ctx>, pid: u32) {
        std::fs::remove_dir_all(ctx.proc_root.join(pid.to_string())).unwrap();
    }

    /// `ctx()` plus a handle on the writer, so the restore can be read back.
    fn ctx_with_writer(tag: &str) -> (Arc<Ctx>, Arc<MockWriter>) {
        let root = scratch(tag);
        let writer = Arc::new(MockWriter::new());
        let ctx = build_ctx(&root, PROFILE, &root.join("no-irqs"), writer.clone());
        (ctx, writer)
    }

    #[tokio::test]
    async fn an_owner_that_dies_releases_the_machine_with_nothing_cooperating() {
        let (ctx, writer) = ctx_with_writer("owner-release");
        spawn_fake_owner(&ctx, 4242, 900_100);

        ctx.game_enter_owned(&[], Some(4242)).await.unwrap();
        assert!(ctx.game_active().await);
        assert_eq!(
            u32_of(&ctx.game_status().await, "owner_pid"),
            4242,
            "status must say who is being watched"
        );
        {
            let st = ctx.state.lock().await;
            assert_eq!(st.tier, Tier::Performance);
            assert!(!st.auto_switch);
        }
        assert!(
            writer.recorded().contains(&Action::ScxSwitch { sched: "scx_lavd".into() }),
            "the session really did install the gaming scheduler"
        );
        writer.clear();

        // The session is destroyed the way a `systemctl restart greetd`
        // destroys it: the process is simply gone. No trap, no `game stop`.
        kill_fake_owner(&ctx, 4242);

        let why = ctx
            .game_release_if_owner_gone()
            .await
            .expect("a vanished owner is an answerable reading")
            .expect("and it must release");
        assert!(why.contains("/proc/4242"), "why was {why:?}");

        assert!(!ctx.game_active().await, "the session is over");
        let st = ctx.state.lock().await;
        assert_eq!(st.tier, Tier::Balanced, "the tier the session interrupted came back");
        assert!(st.auto_switch, "and so did auto-switch");
        drop(st);
        assert!(
            writer.recorded().contains(&Action::ScxStop),
            "the gaming scheduler was actually stopped, not merely planned to be"
        );
        let idle = ctx.game_status().await;
        assert!(
            !bool::try_from(idle.get("active").expect("status always reports active")).unwrap(),
            "and the status a user reads agrees with the hardware"
        );
        assert!(
            !idle.contains_key("owner_pid"),
            "an idle machine has no session, so nothing to watch and nothing to report"
        );
    }

    #[tokio::test]
    async fn a_live_owner_is_never_released() {
        // The other direction. Without this the test above passes just as well
        // for a watch that releases on every tick.
        let (ctx, writer) = ctx_with_writer("owner-alive");
        spawn_fake_owner(&ctx, 4243, 900_200);
        ctx.game_enter_owned(&[], Some(4243)).await.unwrap();
        writer.clear();

        for _ in 0..3 {
            assert_eq!(
                ctx.game_release_if_owner_gone().await.unwrap(),
                None,
                "a live owner must never be read as gone"
            );
        }
        assert!(ctx.game_active().await);
        assert!(writer.recorded().is_empty(), "a no-op tick must write nothing");
        let st = ctx.state.lock().await;
        assert_eq!(st.tier, Tier::Performance);
    }

    #[tokio::test]
    async fn a_session_with_no_owner_is_left_exactly_as_it_was() {
        // `apex game start` typed by hand, or the shell's power menu: nothing
        // is watching, and the watch must not invent a reason to stop it.
        let (ctx, _writer) = ctx_with_writer("owner-none");
        ctx.game_enter(&[]).await.unwrap();
        assert_eq!(ctx.game_owner().await, None);
        assert_eq!(ctx.game_release_if_owner_gone().await.unwrap(), None);
        assert!(ctx.game_active().await);
        assert_eq!(
            u32_of(&ctx.game_status().await, "owner_pid"),
            0,
            "an unwatched session reports owner_pid 0"
        );
    }

    #[tokio::test]
    async fn a_reused_owner_pid_is_a_release_and_not_a_reprieve() {
        let (ctx, _writer) = ctx_with_writer("owner-reused");
        spawn_fake_owner(&ctx, 4244, 900_300);
        ctx.game_enter_owned(&[], Some(4244)).await.unwrap();

        // Same number, different process.
        std::fs::write(
            ctx.proc_root.join("4244").join("stat"),
            stat_line(4244, 999_999, 'S'),
        )
        .unwrap();

        let why = ctx.game_release_if_owner_gone().await.unwrap().unwrap();
        assert!(why.contains("reused"), "why was {why:?}");
        assert!(!ctx.game_active().await);
    }

    #[tokio::test]
    async fn an_unreadable_proc_leaves_the_session_alone_and_says_so() {
        // Fail closed: an I/O error must not be able to change the machine's
        // power state.
        let (ctx, writer) = ctx_with_writer("owner-unreadable");
        spawn_fake_owner(&ctx, 4245, 900_400);
        ctx.game_enter_owned(&[], Some(4245)).await.unwrap();
        writer.clear();

        std::fs::remove_dir_all(&ctx.proc_root).unwrap();
        let err = ctx
            .game_release_if_owner_gone()
            .await
            .expect_err("an unreadable /proc is an error, not a release");
        assert!(format!("{err:#}").contains("nothing was measured"), "err was {err:#}");
        assert!(ctx.game_active().await, "the session survives an unanswerable reading");
        assert!(writer.recorded().is_empty());
    }

    #[tokio::test]
    async fn the_first_owner_of_a_session_keeps_it() {
        // The adoption rule, asserted rather than assumed. A second enter with
        // a different owner must not move the watch onto a process that did
        // not start the session.
        let (ctx, _writer) = ctx_with_writer("owner-adopt");
        spawn_fake_owner(&ctx, 4246, 900_500);
        spawn_fake_owner(&ctx, 4247, 900_600);
        ctx.game_enter_owned(&[], Some(4246)).await.unwrap();
        ctx.game_enter_owned(&[], Some(4247)).await.unwrap();
        assert_eq!(ctx.game_owner().await.unwrap().pid, 4246);

        // And a session that started WITHOUT an owner can be adopted by one,
        // which is what lets a caller add the watch after the fact.
        let (ctx2, _w2) = ctx_with_writer("owner-adopt-late");
        spawn_fake_owner(&ctx2, 4248, 900_700);
        ctx2.game_enter(&[]).await.unwrap();
        assert_eq!(ctx2.game_owner().await, None);
        ctx2.game_enter_owned(&[], Some(4248)).await.unwrap();
        assert_eq!(ctx2.game_owner().await.unwrap().pid, 4248);
    }

    // ── sched-ext, as `apex game status` now answers it ─────────────────────
    //
    // Three states, three different words, and `loaded` is reachable ONLY from
    // a kernel reading. The shipped behaviour was one word — a `notes` line
    // copied out of the plan — which said the scheduler was in force on three
    // images where `scxctl switch` had refused every call because nothing was
    // running for it to switch. The engine defect is pinned in
    // `apexd-core/src/syswriter.rs`'s `scx_tests`; these pin what the user is
    // told, which is the half nobody could see.

    /// Same as [`PROFILE_STEER`] but asking for a scheduler.
    const PROFILE_SCX: &str = r#"
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
        irq = "off"
        scx = "scx_lavd"
        [gamemode.nvidia]
        enabled = false
    "#;

    /// A writer whose `scxctl` returns `outcome` and whose kernel reads
    /// `state`. The two are set INDEPENDENTLY on purpose: that is what lets a
    /// test present a command that succeeded over a machine that did not move.
    struct ScxWriter {
        outcome: Outcome,
        state: ScxState,
    }

    impl SysWriter for ScxWriter {
        fn apply(&self, action: &Action) -> anyhow::Result<Outcome> {
            Ok(match action {
                Action::ScxSwitch { .. } | Action::ScxStop => self.outcome.clone(),
                _ => Outcome::Landed,
            })
        }
        fn scx_state(&self) -> ScxState {
            self.state.clone()
        }
    }

    fn str_of(m: &HashMap<String, OwnedValue>, key: &str) -> String {
        let v = m
            .get(key)
            .unwrap_or_else(|| panic!("status has no '{key}' key: {:?}", m.keys()));
        match &**v {
            Value::Str(s) => s.to_string(),
            other => panic!("'{key}' is not a string: {other:?}"),
        }
    }

    async fn scx_status(
        tag: &str,
        outcome: Outcome,
        state: ScxState,
    ) -> HashMap<String, OwnedValue> {
        let root = scratch(tag);
        let irq_root = hybrid_machine(&root);
        let ctx = build_ctx(
            &root,
            PROFILE_SCX,
            &irq_root,
            Arc::new(ScxWriter { outcome, state }),
        );
        ctx.game_enter(&[]).await.unwrap();
        ctx.game_status().await
    }

    #[tokio::test]
    async fn status_says_loaded_only_when_the_kernel_says_a_scheduler_is_attached() {
        let st = scx_status(
            "scx-loaded",
            Outcome::Landed,
            ScxState::Enabled {
                ops: Some("lavd".into()),
            },
        )
        .await;
        assert_eq!(str_of(&st, "scx_state"), "loaded");
        assert_eq!(str_of(&st, "scx_requested"), "scx_lavd");
        let detail = str_of(&st, "scx_detail");
        assert!(
            detail.contains("root/ops reads 'lavd'"),
            "the detail must quote what was read, not what was asked: {detail}"
        );
        assert!(
            !detail.contains("not the scheduler that was asked for"),
            "`lavd` IS scx_lavd — the struct_ops name drops the prefix: {detail}"
        );
        assert!(
            notes_of(&st).iter().any(|n| n.starts_with("sched-ext: loaded")),
            "{:?}",
            notes_of(&st)
        );
    }

    #[tokio::test]
    async fn status_says_not_loaded_when_scxctl_refused() {
        // The katana case, in the shape the journal recorded it.
        let st = scx_status(
            "scx-refused",
            Outcome::Refused(
                "scxctl start -s scx_lavd: error: no scx scheduler running".into(),
            ),
            ScxState::Disabled,
        )
        .await;
        assert_eq!(str_of(&st, "scx_state"), "not loaded");
        let detail = str_of(&st, "scx_detail");
        assert!(detail.contains("scxctl refused"), "{detail}");
        assert!(
            detail.contains("disabled"),
            "and it must carry the kernel's own reading too: {detail}"
        );
        let notes = notes_of(&st);
        assert!(
            !notes.iter().any(|n| n.contains("sched-ext: loaded")),
            "nothing may report a refused switch as loaded: {notes:?}"
        );
    }

    #[tokio::test]
    async fn a_command_that_said_yes_over_a_kernel_that_says_disabled_is_not_loaded() {
        // The collapse itself, at the status layer: `scxctl` exited 0 and
        // nothing attached. If this ever reads `loaded` again, the defect is
        // back regardless of which verb the engine sends.
        let st = scx_status("scx-lied", Outcome::Landed, ScxState::Disabled).await;
        assert_eq!(
            str_of(&st, "scx_state"),
            "not loaded",
            "a command's exit code is a fact about the command, not about the kernel"
        );
    }

    #[tokio::test]
    async fn status_says_unknown_when_the_machine_could_not_be_read() {
        // The third answer. Neither a pass nor a failure — and it must not be
        // rounded to either, which is what the whole program calls
        // "permission denied is not absence".
        let st = scx_status(
            "scx-unknown",
            Outcome::Unknown("could not be confirmed".into()),
            ScxState::Unreadable("/sys/kernel/sched_ext/state: EACCES".into()),
        )
        .await;
        assert_eq!(str_of(&st, "scx_state"), "unknown");
        let detail = str_of(&st, "scx_detail");
        assert!(detail.contains("EACCES"), "{detail}");
    }

    #[tokio::test]
    async fn a_kernel_without_sched_ext_is_not_loaded_rather_than_unknown() {
        // "No scheduler can attach here" is a definite answer and reads as
        // one. Only an unreadable machine is unknown.
        let st = scx_status(
            "scx-unsupported",
            Outcome::Refused("scxctl: this kernel has no sched_ext support".into()),
            ScxState::Unsupported,
        )
        .await;
        assert_eq!(str_of(&st, "scx_state"), "not loaded");
        assert!(str_of(&st, "scx_detail").contains("CONFIG_SCHED_CLASS_EXT"));
    }

    #[tokio::test]
    async fn a_scheduler_that_is_not_the_one_asked_for_is_loaded_and_says_so() {
        let st = scx_status(
            "scx-wrong",
            Outcome::Landed,
            ScxState::Enabled {
                ops: Some("rusty".into()),
            },
        )
        .await;
        assert_eq!(
            str_of(&st, "scx_state"),
            "loaded",
            "something IS attached — that is a different problem from nothing being attached"
        );
        assert!(
            str_of(&st, "scx_detail").contains("not the scheduler that was asked for"),
            "but the disagreement has to be visible"
        );
    }

    #[tokio::test]
    async fn a_profile_that_asks_for_no_scheduler_claims_nothing() {
        // `scx = ""` must produce neither a claim nor a complaint.
        let root = scratch("scx-none");
        let irq_root = hybrid_machine(&root);
        let ctx = build_ctx(&root, PROFILE_STEER, &irq_root, Arc::new(MockWriter::new()));
        ctx.game_enter(&[]).await.unwrap();
        let st = ctx.game_status().await;
        assert_eq!(str_of(&st, "scx_state"), "not requested");
        assert_eq!(str_of(&st, "scx_requested"), "");
        assert!(
            !notes_of(&st).iter().any(|n| n.contains("sched-ext")),
            "a profile that asks for nothing must not mention it: {:?}",
            notes_of(&st)
        );
    }

    #[tokio::test]
    async fn a_plan_applied_through_the_recording_mock_is_unknown_and_never_loaded() {
        // The mock records intentions and touches nothing, so it cannot say a
        // scheduler is attached. This is the guard against the whole class
        // coming back through a convenience default: if `MockWriter` ever
        // starts answering `Disabled` or `Enabled`, every test in the suite
        // would silently start asserting against a fiction.
        let root = scratch("scx-mock");
        let irq_root = hybrid_machine(&root);
        let ctx = build_ctx(&root, PROFILE_SCX, &irq_root, Arc::new(MockWriter::new()));
        ctx.game_enter(&[]).await.unwrap();
        let st = ctx.game_status().await;
        assert_eq!(
            str_of(&st, "scx_state"),
            "unknown",
            "a recorded intention is not a scheduler"
        );
    }

    #[tokio::test]
    async fn gpus_locked_reports_what_nvidia_smi_accepted_and_not_what_was_planned() {
        // The sibling defect, found by looking for the same shape: this key
        // was `plan.gpus_locked.clone()`, so a GPU whose clock lock nvidia-smi
        // rejected was still listed as locked.
        let root = scratch("gpu-refused");
        let irq_root = hybrid_machine(&root);

        struct RefusesEveryGpuLock;
        impl SysWriter for RefusesEveryGpuLock {
            fn apply(&self, action: &Action) -> anyhow::Result<Outcome> {
                Ok(match action {
                    Action::NvidiaLockGraphics { .. } | Action::NvidiaLockMemory { .. } => {
                        Outcome::Refused("nvidia-smi: exit status: 3 — Not Supported".into())
                    }
                    _ => Outcome::Landed,
                })
            }
        }

        let profile = PROFILE_SCX
            .replace("scx = \"scx_lavd\"", "scx = \"\"")
            .replace(
                "[gamemode.nvidia]\n        enabled = false",
                "[gamemode.nvidia]\n        enabled = true\n        \
                 graphics_clock = [1200, 1620]\n        memory_clock = [6000, 7000]",
            );
        // One GPU that nvidia-smi reports and whose maxima let both clock
        // locks resolve, so the plan really does contain two lock actions for
        // the writer to refuse.
        let smi = MockNvidiaSmi {
            available: true,
            gpus: vec![apexd_core::gpu::NvidiaGpu {
                index: 0,
                name: "NVIDIA GeForce RTX 3070 Laptop GPU".into(),
                max_graphics_mhz: Some(1620),
                max_memory_mhz: Some(7001),
                persistence: Some(false),
            }],
            ..Default::default()
        };
        let ctx = build_ctx_smi(
            &root,
            &profile,
            &irq_root,
            Arc::new(RefusesEveryGpuLock),
            Arc::new(smi),
        );
        ctx.game_enter(&[]).await.unwrap();
        let st = ctx.game_status().await;

        let attempted = u32_list_of(&st, "gpus_lock_attempted");
        let locked = u32_list_of(&st, "gpus_locked");
        assert!(
            !attempted.is_empty(),
            "the fixture must actually plan a lock, or this asserts nothing: {st:?}"
        );
        assert!(
            locked.is_empty(),
            "every lock was refused, so no GPU is locked — got {locked:?}"
        );
        assert!(
            notes_of(&st).iter().any(|n| n.contains("Not Supported")),
            "and status must say why: {:?}",
            notes_of(&st)
        );
    }

    fn u32_list_of(m: &HashMap<String, OwnedValue>, key: &str) -> Vec<u32> {
        let v = m
            .get(key)
            .unwrap_or_else(|| panic!("status has no '{key}' key: {:?}", m.keys()));
        let Value::Array(a) = &**v else {
            panic!("'{key}' is not an array");
        };
        a.iter()
            .map(|x| u32::try_from(x.try_clone().unwrap()).unwrap())
            .collect()
    }
}
