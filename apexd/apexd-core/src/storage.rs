//! Storage health: what the disks in this machine are telling us, and what
//! nobody was able to ask them. Roadmap §48's Storage Manager.
//!
//! Pure, like [`crate::channel`]: the CLI runs the tools and reads the sysfs
//! nodes, and hands the text in. Nothing here spawns a process or resolves a
//! path, which is what lets the whole of it be driven from fixtures.
//!
//! ## Every field is a reading or a reason
//!
//! [`Reading`] has two arms and no default. A disk temperature that could not
//! be read is not 0 °C, a wear figure nobody could obtain is not 0% used, and
//! an unreadable `rotational` node does not make a disk an SSD. This module
//! exists downstream of a defect class this repository has swept for in about
//! fourteen places, and storage is where it does the most damage: a report
//! that answers "healthy" because it could not open the device is worse than
//! one that says nothing.
//!
//! ## smartctl's exit status is a bitmask, and half of it means the read WORKED
//!
//! This is the trap in the middle of this feature, and it is documented in
//! smartctl(8) on the machine rather than assumed here. The eight bits split
//! in two:
//!
//! * bits 0–2 — the command did not parse, the device would not open, or a
//!   command to the disk failed. The read did not happen.
//! * bits 3–7 — SMART says DISK FAILING, prefail attributes are at or under
//!   their threshold, they have been in the past, the error log has entries,
//!   or the self-test log has failures. **The read happened.** The non-zero
//!   exit is the disk reporting bad news.
//!
//! So `if status != 0 { unavailable }` throws away exactly the case this
//! feature exists to catch: a dying disk reports as a disk nobody could
//! measure, and the row goes grey instead of red. [`classify_exit`] splits the
//! two, and it is the one place that knows the bitmask.
//!
//! Measured on this machine and worth keeping: unprivileged, `smartctl -j -a
//! /dev/nvme0n1` exits **2** — bit 1, device open failed — and still prints a
//! complete JSON document, with `smartctl.exit_status: 2` and a `messages`
//! array holding `Permission denied`. A parser looking for the health log and
//! not finding it reads that as a disk with no SMART support. And with `sudo`
//! but without an action option, `smartctl -j /dev/nvme0n1` exits **0** and
//! returns the same empty document: exit zero, no data, no error. `-a` is what
//! asks for the data.
//!
//! ## `/` on this operating system is always 100% full
//!
//! Also measured, and the reason [`space`] filters the way it does. On a bootc
//! machine the root filesystem is a composefs image: `findmnt` reports `/` as
//! `overlay`, source `composefs`, **37.8M, 0 available, 100% used**, on every
//! APEX machine, forever. A free-space check pointed at `/` — which is what
//! every disk-usage widget ever written does — raises a permanent critical
//! alert about a machine with 1.1 TB free.
//!
//! The honest unit is a filesystem on a block device, not a mount point. One
//! btrfs volume mounted at eight paths is one filesystem with one free-space
//! figure, and `composefs`, `tmpfs`, `devtmpfs` and the rest are not
//! filesystems anybody can run out of space on in the sense a user means.

use std::collections::BTreeMap;

use serde_json::Value;

use crate::recover::Health;

// ── a value, or the reason there is no value ─────────────────────────────────

/// One measurement, or one sentence saying why there is none.
///
/// Deliberately not `Option<T>`. An `Option` invites `unwrap_or_default`, and
/// the default for every type in this module is a lie: 0 °C, 0% worn, not
/// removable, not encrypted. A caller holding a [`Reading`] has to write the
/// unavailable arm out, and the compiler makes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reading<T> {
    Known(T),
    /// Why not. The reason reaches the user, so it says what to do where
    /// there is something to do: "run it with sudo" rather than "EACCES".
    Unavailable(String),
}

impl<T> Reading<T> {
    pub fn known(&self) -> Option<&T> {
        match self {
            Reading::Known(v) => Some(v),
            Reading::Unavailable(_) => None,
        }
    }

    pub fn reason(&self) -> Option<&str> {
        match self {
            Reading::Known(_) => None,
            Reading::Unavailable(r) => Some(r),
        }
    }

    pub fn is_known(&self) -> bool {
        matches!(self, Reading::Known(_))
    }
}

/// Turn a read that may have been refused into a reading.
///
/// The one place the EACCES rule is applied to a file, so it is the one place
/// to change if the wording of the remedy ever moves.
pub fn from_read(what: &str, r: Result<String, std::io::Error>) -> Reading<String> {
    match r {
        Ok(s) => Reading::Known(s.trim().to_string()),
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            Reading::Unavailable(format!("{what}: permission denied{REMEDY}"))
        }
        Err(e) => Reading::Unavailable(format!("{what}: {e}")),
    }
}

/// The next step a refused read gets, wherever the refusal came from.
///
/// One constant because the remedy has to be identical in all three surfaces
/// — the report, `--json` and `apex doctor` — and because the human-mode
/// footer is not a substitute. A caller reading the JSON, or a notifier
/// reading the doctor line, sees only the row.
pub const REMEDY: &str = " — run it with sudo";

/// Apply the EACCES rule to a sentence rather than to an `io::Error`.
///
/// [`from_read`] has a kind to match on. smartctl has none to give: it prints
/// the refusal into its `messages` array and exits 2, so the text is the only
/// thing there is. Both paths end at the same remedy, because "Permission
/// denied" on its own tells a user the read failed and not what to do about
/// it — which is the defect this whole module was written around.
fn remedy_for(said: &str) -> &'static str {
    let lower = said.to_ascii_lowercase();
    if lower.contains("permission denied") || lower.contains("operation not permitted") {
        REMEDY
    } else {
        ""
    }
}

// ── smartctl ─────────────────────────────────────────────────────────────────

/// What smartctl's exit status says about whether the read happened.
///
/// The split smartctl(8) documents, and the reason this type exists rather
/// than a comparison against zero.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SmartExit {
    /// Nothing was read. Bits 0–2.
    NotRead {
        /// Which of the three, in words.
        why: &'static str,
    },
    /// The read happened. Bits 3–7 are the disk's own warnings, and an empty
    /// list is a clean status rather than an unknown one.
    Read { warnings: Vec<&'static str> },
}

/// Every bit smartctl(8) defines, in order, with what it means for the reader.
///
/// The table is here rather than inline so the two halves cannot drift: bits
/// 0–2 stop a read, bits 3–7 do not.
const SMART_BITS: [(i32, bool, &str); 8] = [
    (0, false, "smartctl did not understand the command line"),
    (1, false, "the device would not open, or is in a low-power mode"),
    (2, false, "a command to the disk failed, or a SMART structure failed its checksum"),
    (3, true, "SMART says this disk is FAILING"),
    (4, true, "prefail attributes are at or under their threshold"),
    (5, true, "attributes have been at or under their threshold in the past"),
    (6, true, "the device error log has entries"),
    (7, true, "the device self-test log has failures"),
];

/// Split smartctl's exit status into "did the read happen" and "what is the
/// disk saying".
pub fn classify_exit(code: i32) -> SmartExit {
    for (bit, is_warning, text) in SMART_BITS {
        if !is_warning && code & (1 << bit) != 0 {
            return SmartExit::NotRead { why: text };
        }
    }
    let warnings = SMART_BITS
        .iter()
        .filter(|(bit, is_warning, _)| *is_warning && code & (1 << bit) != 0)
        .map(|(_, _, text)| *text)
        .collect();
    SmartExit::Read { warnings }
}

/// What a disk reports about its own health.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Smart {
    /// The overall SMART verdict, when the device gives one.
    pub passed: Option<bool>,
    /// NVMe `percentage_used`: the controller's own estimate of endurance
    /// consumed. Can exceed 100.
    pub percentage_used: Option<u32>,
    /// NVMe `available_spare`, as a percentage of the spare it started with.
    pub available_spare: Option<u32>,
    /// NVMe `available_spare_threshold`, below which the controller says the
    /// spare is running out.
    pub spare_threshold: Option<u32>,
    /// NVMe `critical_warning`. A bitmask; non-zero is the controller raising
    /// its hand.
    pub critical_warning: Option<u32>,
    pub power_on_hours: Option<u64>,
    pub temperature_c: Option<f64>,
    pub model: Option<String>,
    pub firmware: Option<String>,
    /// Warnings the exit status carried, from [`classify_exit`].
    pub warnings: Vec<&'static str>,
}

/// Read a smartctl `-j -a` document.
///
/// Takes the text and the process's exit status, because the document alone
/// cannot say whether the read happened: smartctl embeds its own
/// `smartctl.exit_status`, and the two are checked against each other rather
/// than one being trusted.
pub fn parse_smart(text: &str, exit_code: i32) -> Result<Smart, String> {
    let v: Value = serde_json::from_str(text).map_err(|e| format!("smartctl output: {e}"))?;

    // smartctl's own record of what it did. When the process status and the
    // embedded status disagree, believe the harsher one: a wrapper that lost
    // the exit code must not turn a refused read into a clean one.
    let embedded = v["smartctl"]["exit_status"].as_i64().unwrap_or(0) as i32;
    let effective = exit_code | embedded;
    if let SmartExit::NotRead { why } = classify_exit(effective) {
        // The messages array is where the actual sentence lives — "Smartctl
        // open device: /dev/nvme0n1 failed: Permission denied" — and it is far
        // more use than the bit's generic text.
        let said = v["smartctl"]["messages"]
            .as_array()
            .and_then(|a| a.first())
            .and_then(|m| m["string"].as_str())
            .map(str::to_string);
        return Err(match said {
            Some(s) => format!("{s} ({why}){}", remedy_for(&s)),
            None => why.to_string(),
        });
    }
    let warnings = match classify_exit(effective) {
        SmartExit::Read { warnings } => warnings,
        SmartExit::NotRead { .. } => unreachable!("the not-read arm returned above"),
    };

    let log = &v["nvme_smart_health_information_log"];
    let smart = Smart {
        passed: v["smart_status"]["passed"].as_bool(),
        percentage_used: log["percentage_used"].as_u64().map(|n| n as u32),
        available_spare: log["available_spare"].as_u64().map(|n| n as u32),
        spare_threshold: log["available_spare_threshold"].as_u64().map(|n| n as u32),
        critical_warning: log["critical_warning"].as_u64().map(|n| n as u32),
        power_on_hours: log["power_on_hours"]
            .as_u64()
            .or_else(|| v["power_on_time"]["hours"].as_u64()),
        temperature_c: v["temperature"]["current"].as_f64(),
        model: v["model_name"].as_str().map(str::to_string),
        firmware: v["firmware_version"].as_str().map(str::to_string),
        warnings,
    };
    // A document with an exit status of zero and nothing in it is what
    // `smartctl -j <dev>` returns when nobody asked it for anything. Reporting
    // that as a disk with no SMART support would be wrong about the disk on
    // the strength of a wrong command line.
    if smart.passed.is_none()
        && smart.percentage_used.is_none()
        && smart.power_on_hours.is_none()
        && smart.temperature_c.is_none()
    {
        return Err(
            "smartctl returned no health data; it needs an action option such as -a".to_string()
        );
    }
    Ok(smart)
}

/// What the wear figures mean for the row's colour.
///
/// [`Health::Unavailable`] when there is nothing to judge, never
/// [`Health::Verified`]: a disk nobody could question is not a disk that
/// answered.
pub fn wear_health(smart: &Smart) -> (Health, String) {
    if smart.passed == Some(false) {
        return (Health::Attention, "SMART reports this disk as failing".into());
    }
    if !smart.warnings.is_empty() {
        return (Health::Attention, smart.warnings.join("; "));
    }
    if let Some(w) = smart.critical_warning {
        if w != 0 {
            return (
                Health::Attention,
                format!("the controller has raised critical warning 0x{w:02x}"),
            );
        }
    }
    if let (Some(spare), Some(threshold)) = (smart.available_spare, smart.spare_threshold) {
        if spare <= threshold {
            return (
                Health::Attention,
                format!("spare capacity is {spare}%, at or under the controller's {threshold}% threshold"),
            );
        }
    }
    match smart.percentage_used {
        Some(used) if used >= 90 => (
            Health::Attention,
            format!("{used}% of the rated endurance is used"),
        ),
        Some(used) => {
            let hours = smart
                .power_on_hours
                .map(|h| format!(", {h} hours powered on"))
                .unwrap_or_default();
            (Health::Verified, format!("{used}% of the rated endurance is used{hours}"))
        }
        // Passed SMART with no endurance figure — a SATA disk, say. Available
        // rather than Verified: something answered, and it was not the number
        // this row is about.
        None if smart.passed == Some(true) => {
            (Health::Available, "SMART passed; this device reports no endurance figure".into())
        }
        None => (
            Health::Unavailable,
            "this device reports neither a SMART verdict nor an endurance figure".into(),
        ),
    }
}

// ── free space ───────────────────────────────────────────────────────────────

/// One filesystem, as `/proc/self/mountinfo` and `statvfs` see it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mount {
    /// The mount point this filesystem is reported under. When one filesystem
    /// is mounted at several paths, [`space`] keeps the shortest.
    pub target: String,
    /// The backing device, as mountinfo writes it.
    pub source: String,
    pub fstype: String,
    /// The mount's own read-only flag. Distinct from the superblock's: on this
    /// operating system `/sysroot` is mounted `ro` over a `rw` btrfs.
    pub read_only: bool,
    /// The superblock options, the comma-separated list mountinfo writes last.
    /// This is where `discard` and btrfs's `discard=async` live, and it is a
    /// property of the filesystem rather than of the mount — which is why
    /// [`discards`] answers per source and not per mount point.
    pub super_options: String,
    /// Which directory *within* the filesystem this mount exposes. `/` is the
    /// whole filesystem; anything else is a bind mount or a subvolume.
    pub fs_root: String,
}

/// Does anything mounting this source ask the kernel to discard as it frees?
///
/// One filesystem's superblock is one answer however many places it is
/// mounted, so this asks about the source. `discard=async` matches on the bare
/// word rather than on equality: btrfs writes the mode and ext4 does not, and
/// a comparison against `"discard"` alone reads every btrfs filesystem on this
/// operating system as untrimmed.
pub fn discards(mounts: &[Mount], source: &str) -> bool {
    mounts
        .iter()
        .filter(|m| m.source == source)
        .any(|m| {
            m.super_options
                .split(',')
                .any(|o| o == "discard" || o.starts_with("discard="))
        })
}

/// A filesystem's size, from `statvfs`. Supplied by the caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Space {
    pub total_bytes: u64,
    pub available_bytes: u64,
}

impl Space {
    /// Percentage of the filesystem that is not available to this user.
    ///
    /// Integer, rounded down, and computed against `total` rather than against
    /// `used + available`: the reserved blocks a filesystem holds back are
    /// real space that is really not available.
    pub fn used_percent(&self) -> u32 {
        if self.total_bytes == 0 {
            return 0;
        }
        let used = self.total_bytes.saturating_sub(self.available_bytes);
        ((used as u128 * 100) / self.total_bytes as u128) as u32
    }
}

/// Filesystems worth reporting free space for, one row per filesystem.
///
/// Two rules, and both come from measurements on a real APEX machine:
///
/// * **A source that is not a block device is not a filesystem anybody fills.**
///   `/` here is `composefs`, permanently 0 bytes available and 100% used, and
///   so is every `tmpfs`, `devtmpfs`, `overlay` and `proc`. A widget pointed at
///   `/` reports a machine with 1.1 TB free as out of space.
/// * **One filesystem is one row.** btrfs subvolumes put `/dev/nvme0n1p5`
///   under eight mount points on this machine. Eight identical warnings about
///   one disk is noise that trains a user to ignore the ninth.
///
/// The representative is the mount that exposes the whole filesystem, and the
/// shortest target among those. Whole-filesystem first is what stops the report
/// naming this machine's 1.5 TB btrfs volume `/etc`: `/etc` is a subvolume
/// bind, `/sysroot` is the filesystem, and both have four-character names.
pub fn space(mounts: &[Mount]) -> Vec<&Mount> {
    let mut best: BTreeMap<&str, &Mount> = BTreeMap::new();
    // Lower sorts better.
    let rank = |m: &Mount| (m.fs_root != "/", m.target.len(), m.target.clone());
    for m in mounts {
        if !m.source.starts_with("/dev/") {
            continue;
        }
        best.entry(m.source.as_str())
            .and_modify(|cur| {
                if rank(m) < rank(cur) {
                    *cur = m;
                }
            })
            .or_insert(m);
    }
    best.into_values().collect()
}

/// Thresholds for the free-space row.
///
/// Two of them, because a percentage alone is wrong at both ends: 5% of a 4 TB
/// disk is 200 GB and nothing is wrong, while 5% of a 2 GB `/boot` is 100 MB
/// and the next kernel will not fit.
pub const SPACE_ATTENTION_PERCENT: u32 = 90;
pub const SPACE_ATTENTION_BYTES: u64 = 2 * 1024 * 1024 * 1024;

pub fn space_health(s: Space) -> (Health, String) {
    let pct = s.used_percent();
    let gib = |b: u64| format!("{:.1} GiB", b as f64 / (1024.0 * 1024.0 * 1024.0));
    if s.total_bytes == 0 {
        return (Health::Unavailable, "this filesystem reports no size".into());
    }
    if pct >= SPACE_ATTENTION_PERCENT && s.available_bytes < SPACE_ATTENTION_BYTES {
        return (
            Health::Attention,
            format!("{pct}% used, {} free", gib(s.available_bytes)),
        );
    }
    (Health::Available, format!("{pct}% used, {} free", gib(s.available_bytes)))
}

// ── TRIM ─────────────────────────────────────────────────────────────────────

/// Whether a filesystem's discards reach the device, and how.
///
/// Three ways and they are not interchangeable: the kernel discarding as it
/// frees (`discard` or btrfs's `discard=async`), a periodic `fstrim`, or
/// nothing. A machine with both is fine; a machine with neither is the one to
/// say something about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trim {
    /// The block device advertises a non-zero discard granularity.
    pub device_supports: Reading<bool>,
    /// A mount option asks the kernel to discard as it frees.
    pub mount_discards: bool,
    /// `fstrim.timer` is enabled.
    pub timer_enabled: Reading<bool>,
    /// When the timer last ran, as the caller read it.
    pub last_run: Option<String>,
}

pub fn trim_health(t: &Trim) -> (Health, String) {
    match t.device_supports {
        Reading::Unavailable(ref why) => return (Health::Unavailable, why.clone()),
        Reading::Known(false) => {
            return (
                Health::Available,
                "this device does not advertise discard, so there is nothing to trim".into(),
            )
        }
        Reading::Known(true) => {}
    }
    if t.mount_discards {
        return (Health::Verified, "the filesystem discards as it frees blocks".into());
    }
    match &t.timer_enabled {
        Reading::Known(true) => {
            let when = t
                .last_run
                .as_deref()
                .map(|w| format!(", last run {w}"))
                .unwrap_or_default();
            (Health::Verified, format!("fstrim.timer is enabled{when}"))
        }
        Reading::Known(false) => (
            Health::Attention,
            "this device supports discard and nothing trims it: no discard mount option and fstrim.timer is off"
                .into(),
        ),
        Reading::Unavailable(why) => (Health::Unavailable, why.clone()),
    }
}

// ── encryption ───────────────────────────────────────────────────────────────

/// What is known about a block device being encrypted.
///
/// The absence of a LUKS header is a real answer only when the header could be
/// looked for. `blkid` on a device nobody may open answers nothing, and that
/// is not "this disk is unencrypted" — which is the single most dangerous
/// sentence this whole module could print, because a user acts on it by
/// putting something on the disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Encryption {
    /// A LUKS or dm-crypt layer was found, named.
    Encrypted(String),
    /// Looked for and not found.
    Plain,
    /// Nobody could look.
    Unknown(String),
}

pub fn encryption_health(e: &Encryption) -> (Health, String) {
    match e {
        Encryption::Encrypted(kind) => (Health::Verified, format!("encrypted ({kind})")),
        // Available, not Attention: an unencrypted disk is a choice, and this
        // report does not judge it. It is a fact the user asked to see.
        Encryption::Plain => (Health::Available, "not encrypted".into()),
        Encryption::Unknown(why) => (Health::Unavailable, why.clone()),
    }
}

// ── refusing a destructive operation ─────────────────────────────────────────
//
// Everything above this line reads. Everything below it decides whether
// something is allowed to write, and THE POLARITY OF THE UNKNOWN CASE INVERTS
// AT THIS LINE.
//
// For the report, a check that could not be performed is a grey row: say so,
// carry the remedy, do not alarm anybody. For a destructive operation, a check
// that could not be performed is a REFUSAL. An unreadable `holders` directory
// is not an absence of holders, and an unreadable `mountinfo` is not an absence
// of mounts. Getting that backwards in the read half mislabels a row; getting
// it backwards here destroys a filesystem.
//
// The two are not the same kind of "unknown", though, and the distinction is
// what [`PartitionEntry`] is shaped around. "udev has no record for this
// device" is a check that did not happen. "udev's record exists and has no
// ID_PART_ENTRY_NAME in it" is a fact — measured on the L16, four of five
// partitions have no name — and treating that as unverifiable would refuse
// every ordinary partition on the developer's own machine for a reason that is
// not true. So the [`Reading`] wraps the RECORD, and plain `Option`s inside it
// carry fields udev genuinely did not write.

/// The GPT partition type GUID of an EFI System Partition.
///
/// Measured on the L16: `nvme0n1p1` has
/// `ID_PART_ENTRY_TYPE=c12a7328-f81f-11d2-ba4b-00a0c93ec93b`, and it is the
/// only partition on the disk that carries an `ID_PART_ENTRY_NAME` at all. The
/// type is therefore the discriminator and the name is the fallback, not the
/// other way round: an installer that leaves the ESP unnamed, or names it
/// `EFI`, still gets caught by the GUID.
pub const ESP_TYPE_GUID: &str = "c12a7328-f81f-11d2-ba4b-00a0c93ec93b";

/// The MBR partition type byte for an EFI System Partition, as udev writes it.
pub const ESP_TYPE_MBR: &str = "0xef";

/// The partition fields of a udev record.
///
/// Each is an `Option` because udev writes what the partition table actually
/// says and no more. The *record* being absent is the unverifiable case and is
/// modelled one level up, by the [`Reading`] this sits inside.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PartitionEntry {
    /// `ID_PART_ENTRY_SCHEME` — `gpt` or `dos`.
    pub scheme: Option<String>,
    /// `ID_PART_ENTRY_TYPE` — a GUID under GPT, a `0x..` byte under MBR.
    pub type_id: Option<String>,
    /// `ID_PART_ENTRY_NAME`, still in udev's octal-ish escaping.
    pub name: Option<String>,
    /// `ID_FS_TYPE`.
    pub fs_type: Option<String>,
}

/// Whether a partition is the EFI System Partition, or whether nobody can say.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EspVerdict {
    /// It is, and here is what said so.
    Yes(String),
    /// It is not, and the partition table was legible enough to prove it.
    No,
    /// The question was not answerable. For a destructive operation this is a
    /// refusal, never a `No`.
    Unsure(String),
}

/// udev escapes a space in `ID_PART_ENTRY_NAME` as the four characters `\x20`.
fn unescape_udev(s: &str) -> String {
    s.replace("\\x20", " ")
}

/// Is this partition the ESP?
///
/// Ordered so the strongest evidence answers first. The `No` arm requires a
/// partition type to have been read: without one there is no basis for saying
/// what the partition is, and a `vfat` filesystem with no type entry is
/// exactly the shape an ESP has under a table this code failed to parse.
pub fn is_esp(e: &PartitionEntry) -> EspVerdict {
    if let Some(t) = &e.type_id {
        let t = t.trim().to_ascii_lowercase();
        if t == ESP_TYPE_GUID {
            return EspVerdict::Yes(format!("GPT partition type {t}"));
        }
        if t == ESP_TYPE_MBR {
            return EspVerdict::Yes(format!("MBR partition type {t}"));
        }
    }
    if let Some(n) = &e.name {
        let n = unescape_udev(n);
        if n.trim().eq_ignore_ascii_case("EFI System Partition") {
            return EspVerdict::Yes(format!("partition name {n:?}"));
        }
    }
    match &e.type_id {
        // A type was read and it is neither ESP GUID nor 0xEF. That is a
        // positive answer, and it is what keeps this guard from refusing the
        // four unnamed partitions on the developer's own disk.
        Some(t) => {
            let _ = t;
            EspVerdict::No
        }
        None => EspVerdict::Unsure(
            "udev's record has no ID_PART_ENTRY_TYPE, so the partition table \
             did not say what this partition is"
                .into(),
        ),
    }
}

/// One block device, as `/sys/block` and its udev record describe it.
///
/// The guard is pure and takes this by value so the suite can present layouts
/// the developer's machine does not have — a LUKS holder, a mounted ESP, an
/// unreadable sysfs — without arranging any of them on real hardware.
/// Measured: every `holders` directory on the L16 is empty, so the holder rule
/// could not be exercised live even once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockDevice {
    /// Kernel name, e.g. `nvme0n1p5` or `loop3`.
    pub kernel_name: String,
    /// The whole disk this is a partition of, or `None` when it *is* a whole
    /// disk. Taken from `/sys/block/<disk>/<part>` rather than by chopping
    /// digits off a name: `nvme0n1` is a whole disk whose name ends in a digit,
    /// and every name-parsing version of this relation gets that wrong.
    pub parent: Option<String>,
    /// What is stacked on top — dm-crypt, an md array, an LVM PV.
    ///
    /// A [`Reading`] and not a `Vec`, and that is the whole point of the type:
    /// `Vec::default()` is the empty list, which reads as "nothing depends on
    /// this device, go ahead".
    pub holders: Reading<Vec<String>>,
    /// The device's udev record, or the reason there is none. A freshly
    /// created loop device has no record at all.
    pub partition: Reading<PartitionEntry>,
    /// The file this loop device is attached to; `None` when it is not an
    /// attached loop device at all.
    ///
    /// **This is the check nothing else makes.** Measured on the L16:
    /// `/dev/loop0` is attached to `/lib/extensions/apex-user.raw`, a merged
    /// system extension carrying 219 packages — and it appears in
    /// `/proc/self/mountinfo` zero times, in `/proc/1/mountinfo` zero times,
    /// and its `holders` directory is empty. Every other rule in this guard
    /// passed it. `wipefs` on a loop device writes straight through to the
    /// backing file, so granting that permit destroys the extension.
    ///
    /// `Known(None)` is a definite answer and comes only from the `loop/`
    /// directory being genuinely absent — `NotFound` and nothing else, because
    /// `Path::exists()` is false on EACCES too and this module has now found
    /// that same mistake fifteen times.
    pub loop_backing: Reading<Option<String>>,
}

/// The machine a destructive request is judged against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Machine {
    pub devices: Vec<BlockDevice>,
    /// Every mount, or the reason there is no list. Unavailable refuses.
    pub mounts: Reading<Vec<Mount>>,
}

/// Mount points whose loss stops this machine booting or running.
///
/// **`/` is in this list and will never match on an APEX machine.** It is kept
/// here, with this comment, because leaving it out looks like an oversight and
/// putting it in *alone* is the defect: measured on the L16, `/`'s mount source
/// is `overlay` with fstype `composefs`, so it is not a block device and no
/// device is ever "the device `/` is on". The device that actually carries this
/// operating system, `/dev/nvme0n1p5`, is the source of `/etc`, `/sysroot`,
/// `/boot` and `/var` — and of `/` never. A guard that protects only `/`
/// protects nothing at all here and will happily wipe the running system.
const SYSTEM_TARGETS: [&str; 6] = ["/", "/etc", "/sysroot", "/boot", "/boot/efi", "/var"];

/// Why a destructive request was refused. One request can collect several.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// Not running with privileges. Nothing else was even looked at.
    NeedsRoot,
    /// The named path is not a block device this machine knows about.
    NotAKnownBlockDevice { path: String },
    /// A filesystem on it, or on one of its partitions, is mounted now.
    Mounted { device: String, targets: Vec<String> },
    /// It carries the running operating system.
    CarriesTheRunningSystem { device: String, evidence: String },
    /// Something is stacked on top of it.
    HasHolders { device: String, holders: Vec<String> },
    /// It is a loop device attached to a file, so it is in use by whoever
    /// attached it — and an attached loop device is in no mount table at all.
    LoopDeviceInUse { device: String, backing_file: String },
    /// The caller said which file a loop device backs, and was wrong. Refusing
    /// on a mismatch rather than falling back to the other rules: someone who
    /// names the wrong file has the wrong device, and the other rules are
    /// exactly the ones that do not see a loop device.
    BackingFileMismatch { device: String, expected: String, actual: Option<String> },
    /// It is the EFI System Partition — which on this machine is mounted
    /// nowhere, so no mount rule catches it.
    IsTheEsp { device: String, evidence: String },
    /// The typed confirmation does not name the device.
    Unconfirmed { typed: String, expected: String },
    /// A check could not be performed, so the answer is no.
    CouldNotVerify { what: String, why: String },
}

impl Refusal {
    /// One line, and where there is something to do about it, what.
    pub fn say(&self) -> String {
        match self {
            Refusal::NeedsRoot => "this erases a disk and must run as root — try it with sudo"
                .into(),
            Refusal::NotAKnownBlockDevice { path } => {
                format!("{path} is not a block device on this machine")
            }
            Refusal::Mounted { device, targets } => format!(
                "{device} is mounted at {} — unmount it first",
                targets.join(", ")
            ),
            Refusal::CarriesTheRunningSystem { device, evidence } => format!(
                "{device} carries the running operating system ({evidence}) — \
                 APEX will not erase the disk it booted from"
            ),
            Refusal::HasHolders { device, holders } => format!(
                "{device} has {} stacked on it — close {} first",
                holders.join(", "),
                if holders.len() == 1 { "it" } else { "them" }
            ),
            Refusal::LoopDeviceInUse { device, backing_file } => format!(
                "{device} is a loop device attached to {backing_file}, so something \
                 is using it — and an attached loop device appears in no mount \
                 table, so no other check here would have stopped you. Detach it \
                 first (losetup -d {device}), or if you do mean the file it backs, \
                 say which: --expect-backing-file {backing_file}"
            ),
            Refusal::BackingFileMismatch { device, expected, actual } => match actual {
                Some(a) => format!(
                    "{device} is attached to {a}, not to {expected} — check which \
                     loop device you meant before erasing either"
                ),
                None => format!(
                    "{device} is not an attached loop device, so it backs no file, \
                     and {expected} is not what you are about to erase"
                ),
            },
            Refusal::IsTheEsp { device, evidence } => format!(
                "{device} is the EFI System Partition ({evidence}) — erasing it \
                 leaves a machine that cannot boot, and it is mounted nowhere, \
                 so nothing else would have stopped you"
            ),
            Refusal::Unconfirmed { typed, expected } => format!(
                "to erase {expected} you have to type it exactly; got {typed:?}"
            ),
            Refusal::CouldNotVerify { what, why } => format!(
                "refusing because {what} could not be checked: {why}. \
                 A check that did not happen is not a check that passed"
            ),
        }
    }
}

/// Permission to erase one device. **The only constructor is [`guard`].**
///
/// The field is private and there is no `new`, so a caller cannot fabricate
/// one, and the destructive step takes a `&Permit` rather than a device name.
/// That makes "erase without asking the guard" not a mistake somebody has to
/// remember to avoid, but a program that does not compile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Permit {
    device: String,
}

impl Permit {
    pub fn device(&self) -> &str {
        &self.device
    }
}

/// Everything erasing `name` would destroy: the device, and — when it is a
/// whole disk — every partition on it.
///
/// The relation runs **downward only**, and both halves of that are load
/// bearing. Downward, because the argument that matters is the whole disk:
/// measured on the L16, `/dev/nvme0n1` is a mount source **zero** times — only
/// `p5` is — so `wipefs /dev/nvme0n1`, which takes the partition table and the
/// running system with it, passes an equality test against every mount source
/// on the machine. Only downward, because erasing one partition does not touch
/// its siblings, and sweeping the parent in would refuse `p1` for what is
/// mounted on `p5` — a refusal that is safe, arrives for a reason that is not
/// true, and teaches the user that the guard is noise.
fn would_destroy<'a>(devices: &'a [BlockDevice], name: &str) -> Vec<&'a BlockDevice> {
    devices
        .iter()
        .filter(|d| d.kernel_name == name || d.parent.as_deref() == Some(name))
        .collect()
}

/// The kernel name in a `/dev/...` path, or `None` if it is not one.
fn kernel_name_of(path: &str) -> Option<&str> {
    let rest = path.strip_prefix("/dev/")?;
    // No nested paths: /dev/mapper/x and /dev/disk/by-uuid/y are symlinks the
    // caller is expected to have resolved, and guessing here would mean
    // judging one device and erasing another.
    if rest.is_empty() || rest.contains('/') {
        return None;
    }
    Some(rest)
}

/// Judge one request to erase one device.
///
/// Collects **every** reason rather than returning the first, because a user
/// who unmounts the filesystem and runs it again should not then discover the
/// LUKS holder, and then the confirmation. One refusal per round trip is how a
/// person ends up hammering `--force`.
///
/// `expect_backing` is how a caller gets past [`Refusal::LoopDeviceInUse`]: an
/// attached loop device is erasable only by someone who can say which file it
/// backs. That is deliberately the same assertion the destructive test is
/// required to make before it touches anything — promoted out of the suite and
/// into the product, where it protects everyone rather than only the tests.
/// Compared as a plain string; canonicalising paths is the caller's job, at
/// the edge where a real filesystem exists.
///
/// `euid` is a [`Reading`] because it is read from `/proc/self/status` and that
/// read can fail. `apex`'s own `ops::require_root` folds an unreadable status
/// file into "must run as root", which is safe but tells the user to do
/// something that will not help; here it is reported as the unanswered check it
/// is.
pub fn guard(
    device_path: &str,
    typed_confirmation: &str,
    machine: &Machine,
    euid: &Reading<u32>,
    expect_backing: Option<&str>,
) -> Result<Permit, Vec<Refusal>> {
    // Privilege first and alone: without it nothing below can be trusted to
    // have been readable in the first place, and a list of six refusals when
    // the answer is "use sudo" buries the one line that helps.
    match euid {
        Reading::Unavailable(why) => {
            return Err(vec![Refusal::CouldNotVerify {
                what: "which user is running this".into(),
                why: why.clone(),
            }]);
        }
        Reading::Known(0) => {}
        Reading::Known(_) => return Err(vec![Refusal::NeedsRoot]),
    }

    let Some(name) = kernel_name_of(device_path) else {
        return Err(vec![Refusal::NotAKnownBlockDevice { path: device_path.into() }]);
    };
    if !machine.devices.iter().any(|d| d.kernel_name == name) {
        return Err(vec![Refusal::NotAKnownBlockDevice { path: device_path.into() }]);
    }

    let mut refusals = Vec::new();

    if typed_confirmation != device_path {
        refusals.push(Refusal::Unconfirmed {
            typed: typed_confirmation.into(),
            expected: device_path.into(),
        });
    }

    // Every device this request would destroy: the target, and — when the
    // target is a whole disk — each of its partitions.
    let affected = would_destroy(&machine.devices, name);

    match &machine.mounts {
        Reading::Unavailable(why) => refusals.push(Refusal::CouldNotVerify {
            what: "which filesystems are mounted".into(),
            why: why.clone(),
        }),
        Reading::Known(mounts) => {
            let mut system = Vec::new();
            let mut plain = Vec::new();
            for m in mounts {
                let Some(src) = kernel_name_of(&m.source) else { continue };
                if !affected.iter().any(|d| d.kernel_name == src) {
                    continue;
                }
                if SYSTEM_TARGETS.contains(&m.target.as_str()) {
                    system.push(format!("/dev/{src} is mounted at {}", m.target));
                } else {
                    plain.push(m.target.clone());
                }
            }
            if !system.is_empty() {
                refusals.push(Refusal::CarriesTheRunningSystem {
                    device: device_path.into(),
                    evidence: system.join("; "),
                });
            }
            if !plain.is_empty() {
                plain.sort();
                plain.dedup();
                refusals.push(Refusal::Mounted { device: device_path.into(), targets: plain });
            }
        }
    }

    for d in &affected {
        match &d.holders {
            Reading::Unavailable(why) => refusals.push(Refusal::CouldNotVerify {
                what: format!("what is stacked on /dev/{}", d.kernel_name),
                why: why.clone(),
            }),
            Reading::Known(h) if !h.is_empty() => refusals.push(Refusal::HasHolders {
                device: format!("/dev/{}", d.kernel_name),
                holders: h.clone(),
            }),
            Reading::Known(_) => {}
        }

        // An attached loop device is in use and no mount check sees it. This
        // sits above the ESP question rather than below it because it is the
        // rule that applies to the only device class this feature is ever
        // tested against, and getting it wrong is silent.
        let is_target = d.kernel_name == name;
        match &d.loop_backing {
            Reading::Unavailable(why) => refusals.push(Refusal::CouldNotVerify {
                what: format!("whether /dev/{} is an attached loop device", d.kernel_name),
                why: why.clone(),
            }),
            Reading::Known(Some(backing)) => match (is_target, expect_backing) {
                (true, Some(e)) if e == backing => {}
                (true, Some(e)) => refusals.push(Refusal::BackingFileMismatch {
                    device: format!("/dev/{}", d.kernel_name),
                    expected: e.to_string(),
                    actual: Some(backing.clone()),
                }),
                _ => refusals.push(Refusal::LoopDeviceInUse {
                    device: format!("/dev/{}", d.kernel_name),
                    backing_file: backing.clone(),
                }),
            },
            Reading::Known(None) => {
                if let (true, Some(e)) = (is_target, expect_backing) {
                    refusals.push(Refusal::BackingFileMismatch {
                        device: format!("/dev/{}", d.kernel_name),
                        expected: e.to_string(),
                        actual: None,
                    });
                }
            }
        }

        // The ESP question is only asked of partitions, because a whole disk
        // cannot be one. That is not a convenience: a loop device straight out
        // of `losetup --find --show` is a whole disk with no udev record at
        // all, and treating a missing record as unverifiable there would make
        // the feature refuse the only thing it is ever tested on.
        if d.parent.is_none() {
            continue;
        }
        let verdict = match &d.partition {
            Reading::Unavailable(why) => EspVerdict::Unsure(why.clone()),
            Reading::Known(e) => is_esp(e),
        };
        match verdict {
            EspVerdict::Yes(evidence) => refusals.push(Refusal::IsTheEsp {
                device: format!("/dev/{}", d.kernel_name),
                evidence,
            }),
            EspVerdict::Unsure(why) => refusals.push(Refusal::CouldNotVerify {
                what: format!("whether /dev/{} is the EFI System Partition", d.kernel_name),
                why,
            }),
            EspVerdict::No => {}
        }
    }

    if refusals.is_empty() {
        Ok(Permit { device: device_path.to_string() })
    } else {
        Err(refusals)
    }
}

/// Where a signature backup goes, and it is said once.
///
/// Measured: `wipefs --backup` with no directory writes into `$HOME`, and under
/// `sudo` `$HOME` is `/root`. So the file a user is told to keep in case they
/// need it back lands in root's home, which they will not think to look in and
/// may not be able to read. The path is passed explicitly for that reason.
pub const SIGNATURE_BACKUP_DIR: &str = "/var/lib/apex/storage/signature-backups";

#[cfg(test)]
mod tests {
    use super::*;

    /// The warnings an exit status carries, for the tests that build a
    /// [`Smart`] by hand. Inside the test module rather than after it:
    /// clippy::items_after_test_module refuses a `#[cfg(test)]` item that
    /// trails the module, and it is right — anything below `mod tests` reads
    /// as production code.
    impl SmartExit {
        fn clone_warnings(&self) -> Vec<&'static str> {
            match self {
                SmartExit::Read { warnings } => warnings.clone(),
                SmartExit::NotRead { .. } => Vec::new(),
            }
        }
    }

    #[test]
    fn the_bits_that_stop_a_read_are_exactly_zero_one_and_two() {
        for bit in 0..3 {
            match classify_exit(1 << bit) {
                SmartExit::NotRead { .. } => {}
                SmartExit::Read { .. } => panic!("bit {bit} let a read through"),
            }
        }
        for bit in 3..8 {
            match classify_exit(1 << bit) {
                SmartExit::Read { warnings } => {
                    assert_eq!(warnings.len(), 1, "bit {bit}");
                }
                SmartExit::NotRead { .. } => panic!("bit {bit} stopped a read"),
            }
        }
    }

    #[test]
    fn a_failing_disk_is_a_read_that_happened_and_not_an_unavailable_row() {
        // The whole reason this type exists. Exit 8 is bit 3, "DISK FAILING",
        // and every `if status != 0 { unavailable }` in the world turns the
        // one row this feature is for into a grey one.
        match classify_exit(8) {
            SmartExit::Read { warnings } => {
                assert_eq!(warnings, vec!["SMART says this disk is FAILING"]);
            }
            SmartExit::NotRead { .. } => panic!("a failing disk read as unreadable"),
        }
    }

    #[test]
    fn a_refused_open_and_a_failing_disk_at_once_is_still_a_refused_open() {
        // 2 | 8. Nothing was read, so the failing bit is not a claim about
        // anything.
        assert!(matches!(classify_exit(10), SmartExit::NotRead { .. }));
    }

    #[test]
    fn a_clean_disk_carries_no_warnings() {
        assert_eq!(classify_exit(0), SmartExit::Read { warnings: vec![] });
    }

    /// The document an unprivileged `smartctl -j -a /dev/nvme0n1` actually
    /// returns on this machine, trimmed. Exit 2, a full JSON body, and no
    /// health log.
    const REFUSED: &str = r#"{
      "json_format_version": [1, 0],
      "smartctl": {
        "version": [7, 5],
        "argv": ["smartctl", "-j", "-a", "/dev/nvme0n1"],
        "messages": [{"string": "Smartctl open device: /dev/nvme0n1 failed: Permission denied",
                      "severity": "error"}],
        "exit_status": 2
      },
      "local_time": {"time_t": 1788729982}
    }"#;

    #[test]
    fn a_refused_open_is_an_error_carrying_what_smartctl_said() {
        let e = parse_smart(REFUSED, 2).expect_err("a refused open parsed as health data");
        assert!(e.contains("Permission denied"), "{e}");
    }

    #[test]
    fn a_refused_open_names_the_remedy_in_the_row_and_not_only_in_a_footer() {
        // The row is what reaches `--json`, `apex storage warnings` and the
        // doctor line. A remedy printed only in the human-mode footer is a
        // remedy three of the four surfaces do not have.
        let e = parse_smart(REFUSED, 2).expect_err("a refused open parsed as health data");
        assert!(e.ends_with(REMEDY), "{e}");
        // And a read that failed for some other reason is not told to use
        // sudo, which would send a user to a password prompt that fixes
        // nothing.
        assert_eq!(remedy_for("a mandatory SMART command failed"), "");
    }

    #[test]
    fn a_wrapper_that_loses_the_exit_code_still_gets_the_refusal() {
        // The document carries its own exit_status, so a caller passing 0
        // because it read the output through a pipe does not get a clean bill
        // of health.
        let e = parse_smart(REFUSED, 0).expect_err("the embedded status was ignored");
        assert!(e.contains("Permission denied"), "{e}");
    }

    /// What `sudo smartctl -j /dev/nvme0n1` returns — no action option, so no
    /// data. Exit 0.
    const NO_ACTION: &str = r#"{
      "json_format_version": [1, 0],
      "smartctl": {"version": [7, 5], "exit_status": 0},
      "device": {"name": "/dev/nvme0n1", "type": "nvme"},
      "local_time": {"time_t": 1788729982}
    }"#;

    #[test]
    fn exit_zero_with_no_data_is_not_a_disk_without_smart() {
        // Measured: root, exit 0, and a document with nothing in it. Reading
        // that as "this disk has no SMART support" would be a claim about the
        // hardware built on a wrong command line.
        let e = parse_smart(NO_ACTION, 0).expect_err("an empty document parsed as health data");
        assert!(e.contains("action option"), "{e}");
    }

    /// The real shape, from `sudo smartctl -j -a /dev/nvme0n1` on the L16.
    fn healthy() -> &'static str {
        r#"{
          "smartctl": {"exit_status": 0},
          "model_name": "SPCC M.2 PCIe SSD",
          "firmware_version": "SN13683",
          "smart_status": {"passed": true, "nvme": {"value": 0}},
          "temperature": {"current": 38, "op_limit_max": 90},
          "nvme_smart_health_information_log": {
            "critical_warning": 0, "temperature": 311, "available_spare": 100,
            "available_spare_threshold": 10, "percentage_used": 1,
            "data_units_written": 115365267, "power_on_hours": 9222
          }
        }"#
    }

    #[test]
    fn a_healthy_nvme_reads_as_verified_with_its_numbers() {
        let s = parse_smart(healthy(), 0).expect("parse");
        assert_eq!(s.percentage_used, Some(1));
        assert_eq!(s.available_spare, Some(100));
        assert_eq!(s.power_on_hours, Some(9222));
        assert_eq!(s.temperature_c, Some(38.0));
        assert_eq!(s.model.as_deref(), Some("SPCC M.2 PCIe SSD"));
        let (h, note) = wear_health(&s);
        assert_eq!(h, Health::Verified);
        assert!(note.contains("1%"), "{note}");
        assert!(note.contains("9222"), "{note}");
    }

    #[test]
    fn spare_at_the_controllers_threshold_is_attention_even_when_smart_passed() {
        let mut s = parse_smart(healthy(), 0).expect("parse");
        s.available_spare = Some(10);
        let (h, note) = wear_health(&s);
        assert_eq!(h, Health::Attention, "{note}");
        assert!(note.contains("10%"), "{note}");
    }

    #[test]
    fn a_critical_warning_is_attention_even_when_wear_is_low() {
        let mut s = parse_smart(healthy(), 0).expect("parse");
        s.critical_warning = Some(0x04);
        let (h, note) = wear_health(&s);
        assert_eq!(h, Health::Attention);
        assert!(note.contains("0x04"), "{note}");
    }

    #[test]
    fn a_disk_with_no_numbers_at_all_is_unavailable_and_not_verified() {
        let s = Smart::default();
        let (h, _) = wear_health(&s);
        assert_eq!(h, Health::Unavailable, "an empty reading passed as healthy");
    }

    #[test]
    fn an_exit_warning_reaches_the_row_even_when_every_number_is_good() {
        let mut s = parse_smart(healthy(), 0).expect("parse");
        s.warnings = classify_exit(64) // the error log has entries
            .clone_warnings();
        let (h, note) = wear_health(&s);
        assert_eq!(h, Health::Attention);
        assert!(note.contains("error log"), "{note}");
    }

    // ── free space ──────────────────────────────────────────────────────────

    fn m(target: &str, source: &str, fstype: &str) -> Mount {
        Mount {
            target: target.into(),
            source: source.into(),
            fstype: fstype.into(),
            read_only: false,
            super_options: String::new(),
            fs_root: "/".into(),
        }
    }

    fn sub(target: &str, source: &str, fstype: &str, fs_root: &str) -> Mount {
        Mount { fs_root: fs_root.into(), ..m(target, source, fstype) }
    }

    #[test]
    fn btrfs_writes_discard_async_and_a_string_comparison_misses_it() {
        // Measured on the L16: /dev/nvme0n1p5 is mounted
        // `rw,seclabel,ssd,discard=async,space_cache=v2`. A check for the
        // exact word "discard" reads every btrfs filesystem on this operating
        // system as untrimmed and then tells the user to enable a timer that
        // would do the work twice.
        let mut mt = m("/var", "/dev/nvme0n1p5", "btrfs");
        mt.super_options = "rw,seclabel,ssd,discard=async,space_cache=v2,subvolid=5".into();
        assert!(discards(&[mt.clone()], "/dev/nvme0n1p5"));
        // And a filesystem that does not ask for it is not credited with it.
        let mut plain = m("/home", "/dev/sda1", "ext4");
        plain.super_options = "rw,seclabel,nodiscard".into();
        assert!(!discards(&[plain], "/dev/sda1"));
    }

    #[test]
    fn the_composefs_root_of_a_bootc_machine_is_not_a_filesystem_to_warn_about() {
        // Measured on the L16: / is overlay/composefs, 37.8M, 0 available,
        // 100% used, and will be on every APEX machine forever.
        let mounts = vec![
            m("/", "composefs", "overlay"),
            m("/sysroot", "/dev/nvme0n1p5", "btrfs"),
        ];
        let rows = space(&mounts);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].source, "/dev/nvme0n1p5");
    }

    #[test]
    fn one_filesystem_under_eight_mount_points_is_one_row() {
        // btrfs subvolumes, exactly as mountinfo has them on the L16. Eight
        // warnings about one disk trains a user to ignore the ninth.
        //
        // /etc is four characters and would win on length alone — and it is a
        // bind of one subvolume, not the volume. /sysroot exposes the whole
        // filesystem, which is what the free-space figure is about.
        let mounts = vec![
            sub("/sysroot/ostree/deploy/default/var", "/dev/nvme0n1p5", "btrfs",
                "/ostree/deploy/default/var"),
            sub("/etc", "/dev/nvme0n1p5", "btrfs", "/ostree/deploy/default/deploy/f3f5.0/etc"),
            sub("/var", "/dev/nvme0n1p5", "btrfs", "/ostree/deploy/default/var"),
            m("/sysroot", "/dev/nvme0n1p5", "btrfs"),
            sub("/boot", "/dev/nvme0n1p5", "btrfs", "/boot"),
        ];
        let rows = space(&mounts);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].target, "/sysroot");
    }

    #[test]
    fn a_filesystem_that_is_only_ever_bind_mounted_still_gets_a_row() {
        // No mount exposes the whole filesystem. The report must still say
        // something rather than dropping the disk.
        let mounts = vec![
            sub("/srv/data", "/dev/sdb1", "ext4", "/data"),
            sub("/srv/backup", "/dev/sdb1", "ext4", "/backup"),
        ];
        let rows = space(&mounts);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].target, "/srv/data");
    }

    #[test]
    fn pseudo_filesystems_never_appear() {
        let mounts = vec![
            m("/dev", "devtmpfs", "devtmpfs"),
            m("/dev/shm", "tmpfs", "tmpfs"),
            m("/proc", "proc", "proc"),
            m("/sys", "sysfs", "sysfs"),
            m("/run", "tmpfs", "tmpfs"),
        ];
        assert!(space(&mounts).is_empty());
    }

    #[test]
    fn a_tie_on_length_is_broken_by_name_so_the_answer_is_stable() {
        let mounts = vec![m("/bbb", "/dev/sda1", "ext4"), m("/aaa", "/dev/sda1", "ext4")];
        assert_eq!(space(&mounts)[0].target, "/aaa");
        let reversed = vec![m("/aaa", "/dev/sda1", "ext4"), m("/bbb", "/dev/sda1", "ext4")];
        assert_eq!(space(&reversed)[0].target, "/aaa");
    }

    #[test]
    fn a_big_disk_at_ninety_percent_is_not_an_alert() {
        // 400 GB free. The percentage alone would fire; the byte floor is what
        // stops it.
        let s = Space {
            total_bytes: 4_000_000_000_000,
            available_bytes: 400_000_000_000,
        };
        assert_eq!(s.used_percent(), 90);
        assert_eq!(space_health(s).0, Health::Available);
    }

    #[test]
    fn a_small_boot_partition_with_nothing_left_is_an_alert() {
        let s = Space { total_bytes: 2_000_000_000, available_bytes: 100_000_000 };
        let (h, note) = space_health(s);
        assert_eq!(h, Health::Attention);
        assert!(note.contains("95% used"), "{note}");
    }

    #[test]
    fn a_filesystem_with_no_size_is_unavailable_rather_than_empty() {
        let s = Space { total_bytes: 0, available_bytes: 0 };
        assert_eq!(space_health(s).0, Health::Unavailable);
    }

    // ── trim ────────────────────────────────────────────────────────────────

    fn trim(supports: Reading<bool>, mount: bool, timer: Reading<bool>) -> Trim {
        Trim {
            device_supports: supports,
            mount_discards: mount,
            timer_enabled: timer,
            last_run: None,
        }
    }

    #[test]
    fn a_device_that_supports_discard_with_nothing_trimming_it_is_the_only_alert() {
        let (h, note) = trim_health(&trim(Reading::Known(true), false, Reading::Known(false)));
        assert_eq!(h, Health::Attention, "{note}");
        let (h, _) = trim_health(&trim(Reading::Known(true), true, Reading::Known(false)));
        assert_eq!(h, Health::Verified, "a discard mount option is trimming");
        let (h, _) = trim_health(&trim(Reading::Known(true), false, Reading::Known(true)));
        assert_eq!(h, Health::Verified, "the timer is trimming");
        let (h, _) = trim_health(&trim(Reading::Known(false), false, Reading::Known(false)));
        assert_eq!(h, Health::Available, "a device with no discard has nothing to trim");
    }

    #[test]
    fn an_unreadable_discard_granularity_is_not_a_device_without_discard() {
        let t = trim(Reading::Unavailable("queue/discard_granularity: permission denied".into()),
                     false, Reading::Known(false));
        let (h, note) = trim_health(&t);
        assert_eq!(h, Health::Unavailable);
        assert!(note.contains("permission denied"), "{note}");
    }

    // ── encryption ──────────────────────────────────────────────────────────

    #[test]
    fn a_disk_nobody_could_look_at_is_never_reported_as_unencrypted() {
        // The most dangerous sentence in this module, because a user acts on
        // it by putting something on the disk.
        let (h, note) = encryption_health(&Encryption::Unknown(
            "/dev/sda: permission denied — run it with sudo".into(),
        ));
        assert_eq!(h, Health::Unavailable);
        assert!(!note.contains("not encrypted"), "{note}");
        assert_eq!(encryption_health(&Encryption::Plain).0, Health::Available);
        assert_eq!(
            encryption_health(&Encryption::Encrypted("LUKS2".into())).0,
            Health::Verified
        );
    }

    // ── readings ────────────────────────────────────────────────────────────

    #[test]
    fn a_refused_read_names_the_remedy_and_a_missing_one_does_not() {
        let denied = from_read(
            "queue/rotational",
            Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied)),
        );
        assert!(denied.reason().expect("reason").contains("sudo"), "{denied:?}");
        let absent =
            from_read("queue/rotational", Err(std::io::Error::from(std::io::ErrorKind::NotFound)));
        let r = absent.reason().expect("reason");
        assert!(!r.contains("sudo"), "a missing file should not tell anybody to use sudo: {r}");
    }

    #[test]
    fn a_reading_has_no_default() {
        // Structural: `Reading` implements neither Default nor Deref, so
        // `unwrap_or_default` is not reachable on one and a caller has to
        // write the unavailable arm.
        let r: Reading<u64> = Reading::Unavailable("nope".into());
        assert!(r.known().is_none());
        assert!(!r.is_known());
    }

    // ── the destructive guard ────────────────────────────────────────────────

    /// A whole disk or a partition with a legible, ordinary partition type.
    ///
    /// The type is the Linux filesystem-data GUID, which is what four of the
    /// five real partitions on the L16 carry, and none of them has a name.
    const LINUX_DATA: &str = "0fc63daf-8483-4772-8e79-3d69d8477de4";

    fn dev(name: &str, parent: Option<&str>) -> BlockDevice {
        BlockDevice {
            kernel_name: name.into(),
            parent: parent.map(str::to_string),
            holders: Reading::Known(Vec::new()),
            partition: Reading::Known(PartitionEntry {
                scheme: parent.map(|_| "gpt".into()),
                type_id: parent.map(|_| LINUX_DATA.into()),
                name: None,
                fs_type: Some("btrfs".into()),
            }),
            loop_backing: Reading::Known(None),
        }
    }

    fn mount(target: &str, source: &str) -> Mount {
        Mount {
            target: target.into(),
            source: source.into(),
            fstype: "btrfs".into(),
            read_only: false,
            super_options: "rw".into(),
            fs_root: "/".into(),
        }
    }

    const ROOT: Reading<u32> = Reading::Known(0);

    /// What `/dev/loop0` is really attached to on the development machine.
    const SYSEXT: &str = "/lib/extensions/apex-user.raw";

    /// The L16 exactly: `/` is composefs on `overlay`, the operating system
    /// lives on `nvme0n1p5`, `p1` is an unmounted ESP carrying the type GUID
    /// and the only `ID_PART_ENTRY_NAME` on the disk, and the whole disk is a
    /// mount source nowhere.
    fn l16() -> Machine {
        let mut esp = dev("nvme0n1p1", Some("nvme0n1"));
        esp.partition = Reading::Known(PartitionEntry {
            scheme: Some("gpt".into()),
            type_id: Some(ESP_TYPE_GUID.into()),
            name: Some("EFI\\x20System\\x20Partition".into()),
            fs_type: Some("vfat".into()),
        });
        Machine {
            devices: vec![
                dev("nvme0n1", None),
                esp,
                dev("nvme0n1p5", Some("nvme0n1")),
                dev("loop3", None),
                // The real loop0 on this machine: a merged system extension,
                // attached, in no mount table, no holders.
                BlockDevice {
                    kernel_name: "loop0".into(),
                    parent: None,
                    holders: Reading::Known(Vec::new()),
                    partition: Reading::Unavailable("no udev record".into()),
                    loop_backing: Reading::Known(Some(SYSEXT.into())),
                },
            ],
            mounts: Reading::Known(vec![
                mount("/", "overlay"),
                mount("/etc", "/dev/nvme0n1p5"),
                mount("/sysroot", "/dev/nvme0n1p5"),
                mount("/boot", "/dev/nvme0n1p5"),
                mount("/var", "/dev/nvme0n1p5"),
            ]),
        }
    }

    /// Index of `loop3` in [`l16`]'s device list — the only device the suite is
    /// ever allowed to pretend to erase.
    const LOOP: usize = 3;

    fn refuse(path: &str, machine: &Machine) -> Vec<Refusal> {
        guard(path, path, machine, &ROOT, None).expect_err("should have been refused")
    }

    #[test]
    fn the_disk_carrying_the_running_system_is_refused_although_slash_is_not_on_it() {
        // The measurement this test exists for: `/`'s source is `overlay`, so
        // the device is never "the device / is on". Protecting only `/` would
        // let both of these through.
        for path in ["/dev/nvme0n1p5", "/dev/nvme0n1"] {
            let why = refuse(path, &l16());
            assert!(
                why.iter().any(|r| matches!(r, Refusal::CarriesTheRunningSystem { .. })),
                "{path} was not recognised as carrying the running system: {why:?}"
            );
        }
    }

    #[test]
    fn erasing_a_whole_disk_is_judged_against_its_partitions_too() {
        // `/dev/nvme0n1` is a mount source zero times on the real machine.
        // Equality against mount sources therefore permits it, and it is the
        // single most destructive argument the command can be given.
        let m = l16();
        let sources: Vec<&str> =
            m.mounts.known().unwrap().iter().map(|x| x.source.as_str()).collect();
        assert!(
            !sources.contains(&"/dev/nvme0n1"),
            "fixture no longer reproduces the measurement: the whole disk must not be a source"
        );
        let why = refuse("/dev/nvme0n1", &m);
        assert!(why.iter().any(|r| matches!(r, Refusal::CarriesTheRunningSystem { .. })), "{why:?}");
    }

    #[test]
    fn erasing_one_partition_is_not_refused_for_what_a_sibling_carries() {
        // The other direction of `would_destroy`, and the reason it is not
        // symmetric. `p9` is a spare partition on the same disk as the running
        // system; sweeping the parent in would refuse it, safely, for a reason
        // that is not true.
        let mut m = l16();
        m.devices.push(dev("nvme0n1p9", Some("nvme0n1")));
        guard("/dev/nvme0n1p9", "/dev/nvme0n1p9", &m, &ROOT, None)
            .expect("a sibling's mounts are not this partition's problem");
    }

    #[test]
    fn the_efi_system_partition_is_refused_even_though_it_is_mounted_nowhere() {
        let m = l16();
        assert!(
            !m.mounts.known().unwrap().iter().any(|x| x.source == "/dev/nvme0n1p1"),
            "fixture no longer reproduces the measurement: the ESP must be unmounted"
        );
        let why = refuse("/dev/nvme0n1p1", &m);
        assert!(
            why.iter().any(|r| matches!(r, Refusal::IsTheEsp { .. })),
            "an unmounted ESP was not refused: {why:?}"
        );
    }

    #[test]
    fn an_esp_is_caught_by_its_type_guid_when_it_has_no_name_at_all() {
        // Measured on the L16: four of five partitions carry no
        // ID_PART_ENTRY_NAME. An installer that leaves the ESP unnamed is
        // therefore ordinary, and a name-only rule wipes the boot partition.
        let mut m = l16();
        m.devices[1].partition = Reading::Known(PartitionEntry {
            scheme: Some("gpt".into()),
            type_id: Some(ESP_TYPE_GUID.to_ascii_uppercase()),
            name: None,
            fs_type: Some("vfat".into()),
        });
        let why = refuse("/dev/nvme0n1p1", &m);
        let said = why.iter().map(Refusal::say).collect::<Vec<_>>().join("\n");
        assert!(
            why.iter().any(|r| matches!(r, Refusal::IsTheEsp { .. })),
            "an unnamed ESP was not caught by its type GUID: {said}"
        );
    }

    #[test]
    fn an_ordinary_unnamed_partition_is_not_refused_as_unverifiable() {
        // The complement, and the reason the udev RECORD is the Reading while
        // the fields inside it are Options. udev writing no name is a fact,
        // not a check that failed; treating it as unverifiable would refuse
        // four of the five partitions on the developer's own disk.
        let mut m = l16();
        let spare = dev("nvme0n1p9", Some("nvme0n1"));
        assert!(
            matches!(&spare.partition, Reading::Known(e) if e.name.is_none()),
            "the fixture must reproduce an unnamed partition"
        );
        m.devices.push(spare);
        guard("/dev/nvme0n1p9", "/dev/nvme0n1p9", &m, &ROOT, None)
            .expect("a legible non-ESP partition type is a positive answer");
    }

    #[test]
    fn an_unreadable_check_refuses_and_is_not_treated_as_a_check_that_passed() {
        // Every arm of the inversion this section is built around.
        let mut m = l16();
        m.mounts = Reading::Unavailable("/proc/self/mountinfo: permission denied".into());
        let why = guard("/dev/loop3", "/dev/loop3", &m, &ROOT, None).expect_err("unreadable mounts");
        assert!(why.iter().any(|r| matches!(r, Refusal::CouldNotVerify { .. })), "{why:?}");

        let mut m = l16();
        m.devices[LOOP].holders = Reading::Unavailable("holders: permission denied".into());
        let why = guard("/dev/loop3", "/dev/loop3", &m, &ROOT, None).expect_err("unreadable holders");
        assert!(why.iter().any(|r| matches!(r, Refusal::CouldNotVerify { .. })), "{why:?}");

        let mut m = l16();
        m.devices[LOOP].loop_backing =
            Reading::Unavailable("/sys/block/loop3/loop: permission denied".into());
        let why = guard("/dev/loop3", "/dev/loop3", &m, &ROOT, None)
            .expect_err("an unreadable loop directory refuses");
        assert!(why.iter().any(|r| matches!(r, Refusal::CouldNotVerify { .. })), "{why:?}");

        let unknown = Reading::Unavailable("/proc/self/status: no Uid line".into());
        let why =
            guard("/dev/loop3", "/dev/loop3", &l16(), &unknown, None).expect_err("unreadable euid");
        assert!(
            why.iter().any(|r| matches!(r, Refusal::CouldNotVerify { .. })),
            "an unreadable euid became a plain sudo hint: {why:?}"
        );
    }

    #[test]
    fn a_stacked_device_is_refused_naming_what_is_on_top() {
        let mut m = l16();
        m.devices[LOOP].holders = Reading::Known(vec!["dm-0".into()]);
        let why = guard("/dev/loop3", "/dev/loop3", &m, &ROOT, None).expect_err("a holder refuses");
        let said = why.iter().map(Refusal::say).collect::<Vec<_>>().join("\n");
        assert!(said.contains("dm-0"), "the refusal did not name the holder: {said}");
    }

    #[test]
    fn a_confirmation_that_does_not_name_the_device_is_refused() {
        let m = l16();
        let why =
            guard("/dev/loop3", "yes", &m, &ROOT, None).expect_err("a bare yes is not a confirmation");
        assert!(why.iter().any(|r| matches!(r, Refusal::Unconfirmed { .. })), "{why:?}");
        // A near miss is still a miss, and an empty string most of all.
        for typed in ["", "loop3", "/dev/loop", "/dev/loop30", " /dev/loop3"] {
            let why = guard("/dev/loop3", typed, &m, &ROOT, None)
                .expect_err("only the exact device name confirms");
            assert!(
                why.iter().any(|r| matches!(r, Refusal::Unconfirmed { .. })),
                "{typed:?} was accepted as a confirmation: {why:?}"
            );
        }
        // And typing it exactly is what gets through.
        guard("/dev/loop3", "/dev/loop3", &m, &ROOT, None).expect("the device name confirms");
    }

    #[test]
    fn without_root_nothing_else_is_even_reported() {
        let why = guard("/dev/loop3", "/dev/loop3", &l16(), &Reading::Known(1000), None)
            .expect_err("refused");
        assert_eq!(why, vec![Refusal::NeedsRoot], "the sudo line must not be buried");
        assert!(Refusal::NeedsRoot.say().contains("sudo"), "and it says how");
    }

    #[test]
    fn every_reason_is_collected_so_a_user_does_not_learn_them_one_at_a_time() {
        let mut m = l16();
        m.devices[LOOP].holders = Reading::Known(vec!["dm-0".into()]);
        let why = guard("/dev/loop3", "nope", &m, &ROOT, None).expect_err("refused");
        assert!(why.len() >= 2, "only one reason came back: {why:?}");
    }

    #[test]
    fn a_device_this_machine_does_not_have_is_not_erased_by_default() {
        for path in ["/dev/sdz", "/dev/mapper/secret", "nvme0n1p5", "/dev/", ""] {
            let why = guard(path, path, &l16(), &ROOT, None).expect_err("unknown device refused");
            assert!(
                why.iter().any(|r| matches!(r, Refusal::NotAKnownBlockDevice { .. })),
                "{path:?} was not refused as unknown: {why:?}"
            );
        }
    }

    #[test]
    fn a_permit_can_only_come_from_the_guard() {
        // Structural, and the reason `Permit`'s field is private: the only way
        // to hold one is to have been granted it. If this stops compiling
        // because somebody added a constructor or made the field public, the
        // type-level guarantee is gone and this test is the notice.
        let m = l16();
        let p = guard("/dev/loop3", "/dev/loop3", &m, &ROOT, None).expect("permitted");
        assert_eq!(p.device(), "/dev/loop3");
        let src = include_str!("storage.rs");
        let decl = src.split("pub struct Permit").nth(1).expect("Permit is declared");
        let body = decl.split('}').next().expect("a body");
        assert!(
            !body.contains("pub device"),
            "Permit's field became public, so anyone can forge permission"
        );
    }

    #[test]
    fn a_loop_device_with_no_udev_record_is_still_erasable() {
        // The device the suite is allowed to touch. A fresh loop device has no
        // udev record, and if a missing record refused, the destructive path
        // would be untestable and would grow a --force nobody could review.
        let mut m = l16();
        m.devices[LOOP].partition = Reading::Unavailable("no udev record".into());
        guard("/dev/loop3", "/dev/loop3", &m, &ROOT, None)
            .expect("a whole loop device cannot be an ESP");
    }

    #[test]
    fn a_partition_whose_type_cannot_be_read_is_refused() {
        // The other side of the same rule: a real partition with no readable
        // record could be the ESP, and nobody can say it is not. Both the
        // missing-record and the record-without-a-type shapes.
        for entry in [
            Reading::Unavailable("no udev record".into()),
            Reading::Known(PartitionEntry {
                scheme: None,
                type_id: None,
                name: None,
                fs_type: Some("vfat".into()),
            }),
        ] {
            let mut m = l16();
            let mut p = dev("nvme0n1p9", Some("nvme0n1"));
            p.partition = entry;
            m.devices.push(p);
            let why = guard("/dev/nvme0n1p9", "/dev/nvme0n1p9", &m, &ROOT, None).expect_err("refused");
            assert!(why.iter().any(|r| matches!(r, Refusal::CouldNotVerify { .. })), "{why:?}");
        }
    }

    #[test]
    fn an_mbr_esp_is_caught_by_its_type_byte() {
        let e = PartitionEntry {
            scheme: Some("dos".into()),
            type_id: Some(ESP_TYPE_MBR.into()),
            name: None,
            fs_type: Some("vfat".into()),
        };
        assert!(matches!(is_esp(&e), EspVerdict::Yes(_)), "{:?}", is_esp(&e));
    }

    #[test]
    fn an_attached_loop_device_is_refused_although_nothing_at_all_sees_it_in_use() {
        // THE REGRESSION TEST FOR THIS FEATURE'S WORST NEAR MISS. Measured on
        // the development machine: /dev/loop0 is attached to a merged system
        // extension carrying 219 packages, and
        //   grep loop /proc/self/mountinfo -> 0
        //   grep loop /proc/1/mountinfo    -> 0
        //   /sys/block/loop0/holders       -> empty
        // Every other rule in this guard passed it, and wipefs on a loop
        // device writes straight through to the backing file. The first
        // version of this guard granted the permit.
        let m = l16();
        let loop0 = m.devices.iter().find(|d| d.kernel_name == "loop0").expect("loop0");
        assert_eq!(
            loop0.holders,
            Reading::Known(Vec::new()),
            "the fixture must reproduce the measurement: no holders"
        );
        assert!(
            !m.mounts.known().unwrap().iter().any(|x| x.source.contains("loop")),
            "the fixture must reproduce the measurement: in no mount table"
        );
        let why = refuse("/dev/loop0", &m);
        assert!(
            why.iter().any(|r| matches!(r, Refusal::LoopDeviceInUse { .. })),
            "an attached loop device was permitted: {why:?}"
        );
    }

    #[test]
    fn the_refusal_says_both_ways_out_and_names_the_file() {
        // A user who legitimately wants to wipe their own loop-backed image
        // needs a next step; without one the only path forward is reading the
        // source, and the path after that is a --force flag.
        let said = refuse("/dev/loop0", &l16())
            .iter()
            .map(Refusal::say)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(said.contains(SYSEXT), "the refusal did not name the file: {said}");
        assert!(said.contains("losetup -d"), "no detach instruction: {said}");
        assert!(said.contains("--expect-backing-file"), "no way through: {said}");
    }

    #[test]
    fn an_attached_loop_device_is_erasable_by_someone_who_says_which_file_it_backs() {
        // The way through, and the only way: this is the assertion the
        // destructive test is required to make, enforced by the product.
        let m = l16();
        let p = guard("/dev/loop0", "/dev/loop0", &m, &ROOT, Some(SYSEXT))
            .expect("naming the backing file correctly is permission");
        assert_eq!(p.device(), "/dev/loop0");
    }

    #[test]
    fn naming_the_wrong_backing_file_is_its_own_refusal_and_not_a_fallback() {
        // Someone who names the wrong file has the wrong device, and the other
        // rules are precisely the ones that cannot see a loop device.
        let m = l16();
        let why = guard("/dev/loop0", "/dev/loop0", &m, &ROOT, Some("/var/tmp/mine.img"))
            .expect_err("a mismatch refuses");
        assert!(
            why.iter().any(|r| matches!(r, Refusal::BackingFileMismatch { actual: Some(_), .. })),
            "{why:?}"
        );
        let said = why.iter().map(Refusal::say).collect::<Vec<_>>().join("\n");
        assert!(said.contains(SYSEXT) && said.contains("/var/tmp/mine.img"), "{said}");
    }

    #[test]
    fn asserting_a_backing_file_for_a_device_that_backs_nothing_is_refused() {
        // The typo that would otherwise be silent: --expect-backing-file
        // pointed at a real image, and the device argument pointed at a disk.
        let m = l16();
        let why = guard("/dev/loop3", "/dev/loop3", &m, &ROOT, Some(SYSEXT))
            .expect_err("loop3 backs nothing in this fixture");
        assert!(
            why.iter().any(|r| matches!(r, Refusal::BackingFileMismatch { actual: None, .. })),
            "{why:?}"
        );
    }

    #[test]
    fn a_backing_file_assertion_does_not_excuse_a_second_loop_device() {
        // `expect_backing` speaks only for the device that was named. If the
        // request would take another attached loop device with it, that one is
        // still in use and nobody vouched for it.
        let mut m = l16();
        m.devices[LOOP].parent = Some("loop0".into());
        m.devices[LOOP].loop_backing = Reading::Known(Some("/var/tmp/other.img".into()));
        let why = guard("/dev/loop0", "/dev/loop0", &m, &ROOT, Some(SYSEXT))
            .expect_err("the other attached device is still in use");
        assert!(why.iter().any(|r| matches!(r, Refusal::LoopDeviceInUse { .. })), "{why:?}");
    }

    #[test]
    fn a_signature_backup_does_not_land_in_roots_home() {
        // Measured: `wipefs --backup` with no directory writes into $HOME, and
        // $HOME under sudo is /root.
        assert!(SIGNATURE_BACKUP_DIR.starts_with("/var/lib/apex/"), "{SIGNATURE_BACKUP_DIR}");
        assert!(!SIGNATURE_BACKUP_DIR.starts_with("/root"), "{SIGNATURE_BACKUP_DIR}");
    }
}
