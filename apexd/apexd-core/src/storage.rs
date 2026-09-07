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
}
