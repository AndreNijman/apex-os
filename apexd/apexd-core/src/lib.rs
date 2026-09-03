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
//! The daemon and CLI are thin shells over this crate.

pub mod battery;
pub mod fan;
pub mod fingerprint;
pub mod game;
pub mod gpu;
pub mod irq;
pub mod mode;
pub mod profile;
pub mod select;
pub mod syswriter;
pub mod tier;
pub mod topology;
pub mod workload;

pub use battery::{Battery, BatteryInventory, ThresholdSupport};
pub use fan::{FanInventory, FanMode, FanSnapshot, UnknownFanMode};
pub use fingerprint::{CpuInfo, CpuVendor, Fingerprint, GpuInfo, GpuVendor};
pub use game::{GameInputs, GamePlan, PidPlacement};
pub use gpu::{NvidiaGpu, NvidiaSmi, RealNvidiaSmi};
pub use mode::{Mode, ModeId, ModeMatch, ModeState, PolicyIntent, Step, TierPolicy, UnknownMode};
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
