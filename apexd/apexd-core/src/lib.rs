//! `apexd-core` — the pure, testable heart of APEX-OS power management.
//!
//! It has no D-Bus, no async runtime, and no unconditional I/O:
//!
//! * [`fingerprint`] reads (never writes) `/proc` + `/sys` to build a
//!   [`Fingerprint`].
//! * [`battery`] enumerates the machine's batteries and probes which of them
//!   (if any) accept charge thresholds. No battery is ever named in code or in
//!   a profile.
//! * [`select`] maps a [`Fingerprint`] to a layered [`Selection`]
//!   (generic -> class -> device).
//! * [`profile`] models the tuning profiles and turns a (profile, tier) pair
//!   into an ordered list of [`Action`]s.
//! * [`syswriter`] is the *only* thing that turns [`Action`]s into real
//!   effects; [`MockWriter`] records them for tests, [`RealWriter`] applies
//!   them (honouring dry-run).
//!
//! M6 adds four more pure modules on the same pattern — read sysfs, plan
//! [`Action`]s, let the writer do the touching:
//!
//! * [`topology`] resolves the P-core/E-core split (Alder Lake and friends).
//! * [`fan`] enumerates hwmon and msi-ec fans and plans mode changes, with an
//!   explicit "hand the fan back to firmware" primitive.
//! * [`gpu`] plans NVIDIA clock locks around `nvidia-smi`.
//! * [`irq`] plans interrupt-affinity steering, and [`game`] combines the three
//!   into a symmetric enter/exit pair.
//!
//! [`blueprint`] is the same pattern once more, for a different subject: it
//! parses the declarative APEX Blueprint, compares it to an observed machine,
//! and emits [`blueprint::Step`]s. It reads nothing and runs nothing — `apex`
//! does the probing and the converging, exactly as the daemon does for
//! [`Action`]s.
//!
//! The daemon and CLI are thin shells over this crate.

pub mod ai;
pub mod aiprobe;
pub mod battery;
pub mod blueprint;
pub mod dispatch;
pub mod fan;
pub mod fingerprint;
pub mod game;
pub mod gameprofile;
pub mod gaming;
pub mod gpu;
pub mod host;
pub mod irq;
pub mod mode;
pub mod perf;
pub mod profile;
pub mod recover;
pub mod select;
pub mod syswriter;
pub mod tier;
pub mod topology;
pub mod workload;

// §14's local inference service. `Store`, `Settings` and `Manifest` are
// unambiguous at the crate root; `Backend`, `Runtime`, `Listen` and the two
// `plan_*` functions deliberately stay behind `ai::` — a bare `Backend` in a
// call site would not say whether it meant a compute backend or something
// else, and `apexd-core` already learned that lesson with `Step`.
pub use ai::{AiError, Manifest as AiManifest, Settings as AiSettings, Store as AiStore};
pub use battery::{Battery, BatteryInventory, ThresholdSupport};
// `Step` is deliberately NOT re-exported here from either module. Phase 7's
// blueprint and phase 8's modes each have a type called `Step` — a convergence
// step and a mode-application step — and re-exporting both at the crate root is
// an E0252 collision. Neither is ambiguous at its own module path, and both
// consumers already import it that way (`apexd_core::blueprint::Step`,
// `apexd_core::mode::Step`), so nothing is lost by leaving the name where it is
// unambiguous. Hoisting one under an alias would just make the call sites lie
// about which kind of step they mean.
pub use blueprint::{
    AppliedState, Blueprint, Bundle, Change, Domain, Observed, Plan, ProjectRef,
};
pub use fan::{FanInventory, FanMode, FanSnapshot, UnknownFanMode};
pub use fingerprint::{CpuInfo, CpuVendor, Fingerprint, GpuInfo, GpuVendor};
pub use game::{GameInputs, GamePlan, PidPlacement};
// §12's two halves. `Step` stays behind its module path for the reason given
// above; `Resolution` and `Readiness` are unambiguous.
pub use gameprofile::{GameProfile, GameProfiles, Resolution};
pub use gaming::{Probe, Readiness};
pub use gpu::{NvidiaGpu, NvidiaSmi, RealNvidiaSmi};
pub use mode::{Mode, ModeId, ModeMatch, ModeState, PolicyIntent, TierPolicy, UnknownMode};
pub use perf::{CpuPerf, GpuPerf, PerfSnapshot, PowerReading, SchedulerState, Temp};
pub use profile::{
    ChargeConfig, CpusetPolicy, FanConfig, GameModeConfig, IrqPolicy, NvidiaConfig, Profile,
    ProfileKind, ProfileSet, TierSettings,
};
pub use select::{select, Selection};
pub use syswriter::{MockWriter, RealWriter, SysWriter};
pub use tier::{Action, Tier, UnknownTier};
pub use topology::{CoreSource, CoreTopology};
pub use workload::{Assessment, Signal, Signals, Vram, Workload};

/// The default on-disk override directory for profiles. If present it wins
/// over the embedded set.
pub const PROFILE_DIR: &str = "/usr/share/apexos/sysprofiles";

/// True when dry-run is forced via the environment (`APEXD_DRY_RUN=1`). The
/// daemon also exposes a `--dry-run` flag; either turns every real write off.
pub fn dry_run_from_env() -> bool {
    matches!(
        std::env::var("APEXD_DRY_RUN").ok().as_deref(),
        Some("1") | Some("true") | Some("yes")
    )
}
