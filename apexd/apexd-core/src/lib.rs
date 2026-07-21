//! `apexd-core` — the pure, testable heart of APEX-OS power management.
//!
//! It has no D-Bus, no async runtime, and no unconditional I/O:
//!
//! * [`fingerprint`] reads (never writes) `/proc` + `/sys` to build a
//!   [`Fingerprint`].
//! * [`select`] maps a [`Fingerprint`] to a layered [`Selection`]
//!   (generic -> class -> device).
//! * [`profile`] models the tuning profiles and turns a (profile, tier) pair
//!   into an ordered list of [`Action`]s.
//! * [`syswriter`] is the *only* thing that turns [`Action`]s into real
//!   effects; [`MockWriter`] records them for tests, [`RealWriter`] applies
//!   them (honouring dry-run).
//!
//! The daemon and CLI are thin shells over this crate.

pub mod fingerprint;
pub mod profile;
pub mod select;
pub mod syswriter;
pub mod tier;

pub use fingerprint::{CpuInfo, CpuVendor, Fingerprint, GpuInfo, GpuVendor};
pub use profile::{ChargeConfig, Profile, ProfileKind, ProfileSet, RyzenAdjConfig, TierSettings};
pub use select::{select, Selection};
pub use syswriter::{MockWriter, RealWriter, SysWriter};
pub use tier::{Action, Tier, UnknownTier};

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
