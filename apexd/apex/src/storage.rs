//! `apex storage` — the disks in this machine, what they say about themselves,
//! and what nobody was able to ask them. Roadmap §48's Storage Manager.
//!
//! The thresholds, the smartctl bitmask and the free-space rules live in
//! [`apexd_core::storage`]. This file runs the tools and reads the nodes.
//!
//! ## Reading is unprivileged; SMART is not, and the report says which
//!
//! Almost everything here is a world-readable sysfs node, so `apex storage
//! status` needs no root and can sit behind a settings page that polls. The
//! exception is the SMART log: opening a block device needs `CAP_SYS_RAWIO`,
//! so unprivileged the wear row reads *unavailable, run it with sudo* rather
//! than disappearing. Both halves of that matter — a row that vanished would
//! make a machine with a dying disk look like a machine with a healthy one.
//!
//! ## Three tools that lie about failure, all measured on this machine
//!
//! * `smartctl -j -a <dev>` unprivileged exits 2 **and prints a complete JSON
//!   document** with no health log in it. Keying on the missing log gives "this
//!   disk has no SMART support".
//! * `sudo smartctl -j <dev>` — no action option — exits **0** and returns the
//!   same empty document. Exit zero, no data, no error.
//! * `blkid -o value -s TYPE <dev>` unprivileged exits **0** and prints
//!   nothing at all. A caller reading that as "no filesystem here" would go on
//!   to report an encrypted disk as unencrypted, which is the one sentence in
//!   this module a user would act on by putting data at risk.
//!
//! So the filesystem type comes from udev's own database at
//! `/run/udev/data/b<major>:<minor>`, which udev populated as root at boot and
//! left world-readable. It is a file read, it needs no privilege, and under a
//! fixture root it is a fixture.
//!
//! ## Nothing here prints a serial number
//!
//! That same udev record carries `ID_SERIAL_SHORT`, and `nvme list -o json`
//! prints the serial to any user who asks. A storage page that pasted either
//! into a report — or into a bug report a user attaches to an issue — has
//! published the identifier of the disk. The model is what identifies the
//! hardware; the serial identifies the unit.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use apexd_core::recover::Health;
use apexd_core::storage::{
    self, Encryption, Mount, Reading, Smart, Space, Trim,
};
use clap::Subcommand;
use serde_json::{json, Value};

/// smartctl, by absolute path. Never taken from `PATH`: this command may be
/// run under sudo, and a `PATH` entry the invoking user controls would be a
/// root shell.
const SMARTCTL: &str = "/usr/sbin/smartctl";

/// Where the report reads the machine from.
///
/// The same contract as `apex trust`'s `Roots`. Under a fixture root no
/// subprocess is spawned at all: smartctl's answer, the fstrim timer's state
/// and each filesystem's size come from files the fixture wrote, so the suite
/// exercises the shipped binary without touching a real disk or asking for a
/// privilege.
pub struct Roots {
    fixture: Option<PathBuf>,
}

impl Roots {
    pub fn from_env() -> Self {
        Self { fixture: std::env::var_os("APEX_STORAGE_ROOT").map(PathBuf::from) }
    }

    fn path(&self, absolute: &str) -> PathBuf {
        match &self.fixture {
            Some(root) => root.join(absolute.trim_start_matches('/')),
            None => PathBuf::from(absolute),
        }
    }

    fn is_fixture(&self) -> bool {
        self.fixture.is_some()
    }

    fn fixture_file(&self, rel: &str) -> Option<String> {
        let root = self.fixture.as_ref()?;
        std::fs::read_to_string(root.join(".fixture").join(rel)).ok()
    }

    /// A sysfs value, or the reason there is none.
    fn read(&self, absolute: &str) -> Reading<String> {
        let p = self.path(absolute);
        storage::from_read(&p.display().to_string(), std::fs::read_to_string(&p))
    }

    fn read_u64(&self, absolute: &str) -> Reading<u64> {
        match self.read(absolute) {
            Reading::Known(s) => match s.trim().parse::<u64>() {
                Ok(v) => Reading::Known(v),
                Err(e) => Reading::Unavailable(format!("{absolute}: {s:?} is not a number ({e})")),
            },
            Reading::Unavailable(r) => Reading::Unavailable(r),
        }
    }

    fn read_bool(&self, absolute: &str) -> Reading<bool> {
        match self.read_u64(absolute) {
            Reading::Known(v) => Reading::Known(v != 0),
            Reading::Unavailable(r) => Reading::Unavailable(r),
        }
    }
}

#[derive(Subcommand)]
pub enum StorageCmd {
    /// Every disk, its health, its filesystems and their free space.
    ///
    /// Read-only. Needs no root for anything but the SMART log, and reports
    /// that row as unavailable with the remedy rather than dropping it.
    Status {
        /// Emit machine-readable JSON instead of a report.
        #[arg(long)]
        json: bool,
    },
    /// Only the rows that need somebody to do something.
    ///
    /// Exits 0 when there is nothing to say, 1 when there is. Rows that could
    /// not be measured are printed and do not change the exit status: a
    /// notifier must not wake somebody because a read was refused.
    Warnings {
        /// Emit machine-readable JSON instead of a report.
        #[arg(long)]
        json: bool,
    },
}

// ── one disk ─────────────────────────────────────────────────────────────────

pub struct Disk {
    pub name: String,
    pub model: Reading<String>,
    pub size_bytes: Reading<u64>,
    pub rotational: Reading<bool>,
    pub removable: Reading<bool>,
    pub read_only: Reading<bool>,
    pub discard_granularity: Reading<u64>,
    pub temperature_c: Reading<f64>,
    pub smart: Result<Smart, String>,
    pub partitions: Vec<Partition>,
}

pub struct Partition {
    pub name: String,
    pub size_bytes: Reading<u64>,
    pub fstype: Reading<String>,
    pub encryption: Encryption,
}

/// Every physical block device, in name order.
///
/// Virtual devices are skipped by where they live rather than by name: `/sys/
/// block/<name>` is a symlink into `/sys/devices`, and everything under
/// `devices/virtual/block` is loop, zram, ram or device-mapper. A name-prefix
/// filter would need a list that goes stale, and would hide a real disk called
/// something unexpected.
pub fn disks(roots: &Roots) -> Result<Vec<Disk>, String> {
    let block = roots.path("/sys/block");
    let entries = std::fs::read_dir(&block)
        .map_err(|e| format!("{}: {e}", block.display()))?;
    let mut names: Vec<String> = Vec::new();
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let target = std::fs::read_link(e.path()).unwrap_or_default();
        if target.to_string_lossy().contains("devices/virtual/block") {
            continue;
        }
        names.push(name);
    }
    names.sort();
    Ok(names.iter().map(|n| disk(roots, n)).collect())
}

fn disk(roots: &Roots, name: &str) -> Disk {
    let base = format!("/sys/block/{name}");
    // `size` is in 512-byte sectors regardless of the device's own block size.
    // Multiplying by the logical block size instead is a factor-of-eight error
    // on a 4K-sector disk, in the direction that makes a full disk look empty.
    let size_bytes = match roots.read_u64(&format!("{base}/size")) {
        Reading::Known(sectors) => Reading::Known(sectors.saturating_mul(512)),
        Reading::Unavailable(r) => Reading::Unavailable(r),
    };
    Disk {
        name: name.to_string(),
        model: roots.read(&format!("{base}/device/model")),
        size_bytes,
        rotational: roots.read_bool(&format!("{base}/queue/rotational")),
        removable: roots.read_bool(&format!("{base}/removable")),
        read_only: roots.read_bool(&format!("{base}/ro")),
        discard_granularity: roots.read_u64(&format!("{base}/queue/discard_granularity")),
        temperature_c: temperature(roots, name),
        smart: smart(roots, name),
        partitions: partitions(roots, name),
    }
}

/// The device's own hwmon temperature.
///
/// Worth having even though SMART carries one: hwmon is world-readable and
/// SMART is not, so on an unprivileged run this is the only temperature there
/// is. Measured on the L16: `/sys/class/hwmon/hwmon4` is named `nvme` and
/// reads 37850, which is millidegrees.
fn temperature(roots: &Roots, name: &str) -> Reading<f64> {
    // Two layouts, both live on this machine. The nvme driver registers its
    // hwmon as `<device>/hwmon4` directly; the platform drivers use
    // `<device>/hwmon/hwmon6`. Checking only one finds the temperature on some
    // machines and reports "no sensor" on the others, which is a claim about
    // the hardware built on a directory layout.
    let device = roots.path(&format!("/sys/block/{name}/device"));
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&device) {
        for e in entries.flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            if n == "hwmon" {
                if let Ok(inner) = std::fs::read_dir(e.path()) {
                    candidates.extend(inner.flatten().map(|i| i.path()));
                }
            } else if n.starts_with("hwmon") {
                candidates.push(e.path());
            }
        }
    }
    candidates.sort();
    if candidates.is_empty() {
        return Reading::Unavailable(format!("{}: no hwmon for this device", device.display()));
    }
    for c in &candidates {
        let f = c.join("temp1_input");
        match std::fs::read_to_string(&f) {
            Ok(s) => match s.trim().parse::<f64>() {
                Ok(milli) => return Reading::Known(milli / 1000.0),
                Err(_) => {
                    return Reading::Unavailable(format!("{}: {s:?} is not a temperature", f.display()))
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Reading::Unavailable(format!("{}: {e}", f.display())),
        }
    }
    Reading::Unavailable(format!("{}: no temp1_input under any hwmon", device.display()))
}

/// The SMART log, or the sentence saying why there is none.
fn smart(roots: &Roots, name: &str) -> Result<Smart, String> {
    if roots.is_fixture() {
        // No subprocess under a fixture. The fixture supplies both halves,
        // because the exit status is half the answer.
        let text = roots
            .fixture_file(&format!("smartctl/{name}.json"))
            .ok_or_else(|| format!("no smartctl fixture for {name}"))?;
        let code: i32 = roots
            .fixture_file(&format!("smartctl/{name}.exit"))
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0);
        return storage::parse_smart(&text, code);
    }
    if !Path::new(SMARTCTL).exists() {
        return Err(format!("{SMARTCTL} is not installed"));
    }
    let out = Command::new(SMARTCTL)
        .args(["-j", "-a", &format!("/dev/{name}")])
        .output()
        .map_err(|e| format!("{SMARTCTL}: {e}"))?;
    // `output()` gives the real status; nothing here goes through a pipe, so
    // the code belongs to smartctl rather than to the last stage of one.
    let code = out.status.code().unwrap_or(-1);
    storage::parse_smart(&String::from_utf8_lossy(&out.stdout), code)
}

fn partitions(roots: &Roots, disk: &str) -> Vec<Partition> {
    let base = roots.path(&format!("/sys/block/{disk}"));
    let Ok(entries) = std::fs::read_dir(&base) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.starts_with(disk) && n != disk)
        .filter(|n| base.join(n).join("partition").exists())
        .collect();
    names.sort();
    names
        .iter()
        .map(|n| {
            let udev = udev_record(roots, &format!("/sys/block/{disk}/{n}"));
            let fstype = match &udev {
                Ok(props) => match props.get("ID_FS_TYPE") {
                    Some(t) => Reading::Known(t.clone()),
                    None => Reading::Unavailable(
                        "udev recorded no filesystem type for this partition".into(),
                    ),
                },
                Err(why) => Reading::Unavailable(why.clone()),
            };
            Partition {
                name: n.clone(),
                size_bytes: match roots.read_u64(&format!("/sys/block/{disk}/{n}/size")) {
                    Reading::Known(s) => Reading::Known(s.saturating_mul(512)),
                    Reading::Unavailable(r) => Reading::Unavailable(r),
                },
                encryption: encryption(&fstype, &udev),
                fstype,
            }
        })
        .collect()
}

/// udev's own record for a block device: `/run/udev/data/b<major>:<minor>`.
///
/// Its `E:` lines are the properties `blkid` produced at boot, as root, and
/// the file is world-readable. That is why this is the source rather than
/// `blkid`, which unprivileged exits 0 and prints nothing.
fn udev_record(roots: &Roots, sys_path: &str) -> Result<BTreeMap<String, String>, String> {
    let devnode = roots.path(&format!("{sys_path}/dev"));
    let devnum = std::fs::read_to_string(&devnode)
        .map_err(|e| format!("{}: {e}", devnode.display()))?
        .trim()
        .to_string();
    let rec = roots.path(&format!("/run/udev/data/b{devnum}"));
    let text = std::fs::read_to_string(&rec).map_err(|e| format!("{}: {e}", rec.display()))?;
    let mut m = BTreeMap::new();
    for line in text.lines() {
        if let Some(kv) = line.strip_prefix("E:") {
            if let Some((k, v)) = kv.split_once('=') {
                m.insert(k.to_string(), v.to_string());
            }
        }
    }
    Ok(m)
}

/// What is known about a partition being encrypted.
fn encryption(
    fstype: &Reading<String>,
    udev: &Result<BTreeMap<String, String>, String>,
) -> Encryption {
    match udev {
        // No record means nobody looked, and an absence of evidence about
        // encryption is the one thing this module must never round down to
        // "not encrypted".
        Err(why) => Encryption::Unknown(why.clone()),
        Ok(props) => match fstype.known().map(String::as_str) {
            Some("crypto_LUKS") => {
                let v = props.get("ID_FS_VERSION").map(String::as_str).unwrap_or("LUKS");
                Encryption::Encrypted(format!("LUKS{v}"))
            }
            Some(_) => Encryption::Plain,
            None => Encryption::Unknown(
                "udev recorded no filesystem type, so a LUKS header would not have been seen"
                    .into(),
            ),
        },
    }
}

// ── filesystems ──────────────────────────────────────────────────────────────

/// Every mount, as `/proc/self/mountinfo` writes it.
///
/// mountinfo rather than `/etc/mtab` or `findmnt`: it is the kernel's own
/// view, it is per-namespace so it is the truth for this process, and its
/// separator field means a mount point containing a space parses correctly.
pub fn mounts(roots: &Roots) -> Result<Vec<Mount>, String> {
    let p = roots.path("/proc/self/mountinfo");
    let text = std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?;
    let mut out = Vec::new();
    for line in text.lines() {
        // Fields before " - " are fixed-position; after it come fstype,
        // source and superblock options. Splitting on the separator is the
        // only correct way: the optional-fields run between them is variable
        // length, so a fixed index into the whole line is wrong on any machine
        // with shared subtrees, which is every systemd machine.
        let Some((left, right)) = line.split_once(" - ") else { continue };
        let lf: Vec<&str> = left.split(' ').collect();
        let rf: Vec<&str> = right.split(' ').collect();
        if lf.len() < 6 || rf.len() < 2 {
            continue;
        }
        out.push(Mount {
            target: unescape(lf[4]),
            source: unescape(rf[1]),
            fstype: rf[0].to_string(),
            read_only: lf[5].split(',').any(|o| o == "ro"),
            super_options: rf.get(2).copied().unwrap_or_default().to_string(),
            fs_root: unescape(lf[3]),
        });
    }
    Ok(out)
}

/// mountinfo escapes space, tab, newline and backslash as octal.
fn unescape(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' && i + 3 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 4], 8) {
                out.push(v as char);
                i += 4;
                continue;
            }
        }
        out.push(b[i] as char);
        i += 1;
    }
    out
}

/// A filesystem's size, from `statvfs`.
fn space_of(roots: &Roots, target: &str) -> Reading<Space> {
    if roots.is_fixture() {
        // "<total> <available>", in bytes.
        let key = target.trim_start_matches('/').replace('/', "_");
        let key = if key.is_empty() { "root".to_string() } else { key };
        let Some(text) = roots.fixture_file(&format!("space/{key}")) else {
            return Reading::Unavailable(format!("no space fixture for {target}"));
        };
        let mut it = text.split_ascii_whitespace();
        return match (
            it.next().and_then(|s| s.parse::<u64>().ok()),
            it.next().and_then(|s| s.parse::<u64>().ok()),
        ) {
            (Some(total), Some(avail)) => {
                Reading::Known(Space { total_bytes: total, available_bytes: avail })
            }
            _ => Reading::Unavailable(format!("the space fixture for {target} is malformed")),
        };
    }
    let c = match std::ffi::CString::new(target) {
        Ok(c) => c,
        Err(_) => return Reading::Unavailable(format!("{target} is not a usable path")),
    };
    // Safe: statvfs writes only into `st`, which is owned here, and reports
    // failure through its return value rather than by writing anything.
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    let rc = unsafe { libc::statvfs(c.as_ptr(), &mut st) };
    if rc != 0 {
        let e = std::io::Error::last_os_error();
        return Reading::Unavailable(format!("statvfs({target}): {e}"));
    }
    // f_bavail, not f_bfree: the blocks reserved for root are space this user
    // cannot have, and reporting them as free is how a user runs out of disk
    // at "95% full".
    let frsize = if st.f_frsize == 0 { st.f_bsize } else { st.f_frsize } as u64;
    Reading::Known(Space {
        total_bytes: st.f_blocks as u64 * frsize,
        available_bytes: st.f_bavail as u64 * frsize,
    })
}

/// Whether `fstrim.timer` is enabled, and when it last ran.
fn trim_timer(roots: &Roots) -> (Reading<bool>, Option<String>) {
    if roots.is_fixture() {
        let enabled = roots.fixture_file("fstrim-enabled");
        return match enabled.as_deref().map(str::trim) {
            Some("enabled") => (Reading::Known(true), roots.fixture_file("fstrim-last")),
            Some("disabled") => (Reading::Known(false), None),
            Some(other) => (
                Reading::Unavailable(format!("systemctl said {other:?}")),
                None,
            ),
            None => (Reading::Unavailable("no fstrim fixture".into()), None),
        };
    }
    let out = Command::new("/usr/bin/systemctl")
        .args(["is-enabled", "fstrim.timer"])
        .output();
    let enabled = match out {
        // `is-enabled` exits non-zero for "disabled", so the word on stdout is
        // the answer and the exit status is not. A check on the status alone
        // reports a masked unit and a disabled one identically, and reports
        // "systemctl is missing" as "the timer is off".
        Ok(o) => match String::from_utf8_lossy(&o.stdout).trim() {
            "enabled" | "enabled-runtime" | "static" | "indirect" => Reading::Known(true),
            "disabled" | "masked" | "masked-runtime" => Reading::Known(false),
            other if other.is_empty() => {
                Reading::Unavailable("systemctl said nothing about fstrim.timer".into())
            }
            other => Reading::Unavailable(format!("systemctl said {other:?}")),
        },
        Err(e) => Reading::Unavailable(format!("systemctl: {e}")),
    };
    let last = Command::new("/usr/bin/systemctl")
        .args(["show", "fstrim.timer", "-p", "LastTriggerUSec", "--value"])
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty() && s != "n/a");
    (enabled, last)
}

// ── the report ───────────────────────────────────────────────────────────────

/// One line of the storage report: a stable id, a state, and a sentence.
///
/// The id is the compatibility surface, the same rule the recovery surface's
/// rows live under.
pub struct Row {
    pub id: String,
    pub label: String,
    pub state: Health,
    pub note: String,
}

pub fn rows(roots: &Roots) -> Result<Vec<Row>, String> {
    let mut out: Vec<Row> = Vec::new();
    let disks = disks(roots)?;
    let all_mounts = mounts(roots)?;
    let (timer_enabled, last_run) = trim_timer(roots);

    for d in &disks {
        let kind = match d.rotational {
            Reading::Known(true) => "spinning disk",
            Reading::Known(false) => "solid state",
            Reading::Unavailable(_) => "unknown type",
        };
        let size = match d.size_bytes {
            Reading::Known(b) => format!("{:.1} GB", b as f64 / 1e9),
            Reading::Unavailable(_) => "size unknown".into(),
        };
        let model = d.model.known().cloned().unwrap_or_else(|| "unnamed".into());
        out.push(Row {
            id: format!("disk.{}.identity", d.name),
            label: format!("/dev/{}", d.name),
            state: Health::Available,
            note: format!("{model}, {size}, {kind}"),
        });

        let (state, note) = match &d.smart {
            Ok(s) => storage::wear_health(s),
            Err(why) => (Health::Unavailable, why.clone()),
        };
        out.push(Row {
            id: format!("disk.{}.wear", d.name),
            label: format!("/dev/{} health", d.name),
            state,
            note,
        });

        // The temperature the SMART log gives when it could be read, and the
        // hwmon node otherwise. hwmon is world-readable, so unprivileged this
        // is the only one there is.
        let temp = d
            .smart
            .as_ref()
            .ok()
            .and_then(|s| s.temperature_c)
            .map(Reading::Known)
            .unwrap_or_else(|| match &d.temperature_c {
                Reading::Known(c) => Reading::Known(*c),
                Reading::Unavailable(r) => Reading::Unavailable(r.clone()),
            });
        let (state, note) = match temp {
            Reading::Known(c) => (Health::Available, format!("{c:.0} °C")),
            Reading::Unavailable(r) => (Health::Unavailable, r),
        };
        out.push(Row {
            id: format!("disk.{}.temperature", d.name),
            label: format!("/dev/{} temperature", d.name),
            state,
            note,
        });

        if let Reading::Known(true) = d.removable {
            out.push(Row {
                id: format!("disk.{}.removable", d.name),
                label: format!("/dev/{} is removable", d.name),
                state: Health::Available,
                note: "unplugging this while it is mounted loses whatever was not written".into(),
            });
        }
        // A disk the kernel has forced read-only is one that has already had a
        // write fail. Attention, and named as such: the usual cause is the
        // controller taking the device offline after an I/O error, and a user
        // who does not know that will keep trying to save.
        if let Reading::Known(true) = d.read_only {
            out.push(Row {
                id: format!("disk.{}.readonly", d.name),
                label: format!("/dev/{} is read-only", d.name),
                state: Health::Attention,
                note: "the kernel has marked this device read-only; nothing can be written to it"
                    .into(),
            });
        }

        for p in &d.partitions {
            let (state, note) = storage::encryption_health(&p.encryption);
            let size = match p.size_bytes {
                Reading::Known(b) => format!("{:.1} GB", b as f64 / 1e9),
                Reading::Unavailable(_) => "size unknown".into(),
            };
            let fs = match &p.fstype {
                Reading::Known(t) => t.clone(),
                Reading::Unavailable(_) => "filesystem unknown".into(),
            };
            out.push(Row {
                id: format!("part.{}.encryption", p.name),
                label: format!("/dev/{}", p.name),
                state,
                note: format!("{fs}, {size}, {note}"),
            });
        }

        let t = Trim {
            device_supports: match d.discard_granularity {
                Reading::Known(g) => Reading::Known(g > 0),
                Reading::Unavailable(ref r) => Reading::Unavailable(r.clone()),
            },
            // Any filesystem on any partition of this disk asking the kernel
            // to discard counts: the discard reaches the same device however
            // many partitions are in front of it.
            mount_discards: all_mounts
                .iter()
                .filter(|m| m.source.starts_with(&format!("/dev/{}", d.name)))
                .any(|m| storage::discards(&all_mounts, &m.source)),
            timer_enabled: match &timer_enabled {
                Reading::Known(v) => Reading::Known(*v),
                Reading::Unavailable(r) => Reading::Unavailable(r.clone()),
            },
            last_run: last_run.clone(),
        };
        let (state, note) = storage::trim_health(&t);
        out.push(Row {
            id: format!("disk.{}.trim", d.name),
            label: format!("/dev/{} trim", d.name),
            state,
            note,
        });
    }

    for m in storage::space(&all_mounts) {
        let (state, note) = match space_of(roots, &m.target) {
            Reading::Known(s) => storage::space_health(s),
            Reading::Unavailable(r) => (Health::Unavailable, r),
        };
        out.push(Row {
            id: format!("fs.{}.space", m.source.trim_start_matches("/dev/")),
            label: format!("{} on {}", m.fstype, m.target),
            state,
            note,
        });
    }
    Ok(out)
}

fn row_json(r: &Row) -> Value {
    json!({"id": r.id, "label": r.label, "state": r.state.as_str(), "note": r.note})
}

fn status(as_json: bool) -> i32 {
    let roots = Roots::from_env();
    let rows = match rows(&roots) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("apex: {e}");
            return 1;
        }
    };
    if as_json {
        let doc = json!({
            "rows": rows.iter().map(row_json).collect::<Vec<_>>(),
            "attention": rows.iter().filter(|r| r.state == Health::Attention).count(),
            "unavailable": rows.iter().filter(|r| r.state == Health::Unavailable).count(),
        });
        println!("{}", serde_json::to_string_pretty(&doc).unwrap_or_default());
        return 0;
    }
    for r in &rows {
        println!("[{}] {:<28} {}", r.state.as_str(), r.label, r.note);
    }
    let attention = rows.iter().filter(|r| r.state == Health::Attention).count();
    let unavailable = rows.iter().filter(|r| r.state == Health::Unavailable).count();
    println!("\n{attention} needing attention, {unavailable} that could not be measured");
    if unavailable > 0 {
        println!("Some of this needs root. Try: sudo apex storage status");
    }
    0
}

fn warnings(as_json: bool) -> i32 {
    let roots = Roots::from_env();
    let rows = match rows(&roots) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("apex: {e}");
            return 1;
        }
    };
    let attention: Vec<&Row> = rows.iter().filter(|r| r.state == Health::Attention).collect();
    let unavailable: Vec<&Row> =
        rows.iter().filter(|r| r.state == Health::Unavailable).collect();
    if as_json {
        let doc = json!({
            "attention": attention.iter().map(|r| row_json(r)).collect::<Vec<_>>(),
            "unavailable": unavailable.iter().map(|r| row_json(r)).collect::<Vec<_>>(),
        });
        println!("{}", serde_json::to_string_pretty(&doc).unwrap_or_default());
    } else {
        for r in &attention {
            println!("[attention] {:<28} {}", r.label, r.note);
        }
        for r in &unavailable {
            println!("[unavailable] {:<26} {}", r.label, r.note);
        }
    }
    // Only a row that needs doing changes the exit status. A read nobody could
    // perform is printed and is not an alert: waking somebody at 3am because
    // `apex storage` ran without root is how a notifier gets muted, and a muted
    // notifier misses the disk that is actually failing.
    i32::from(!attention.is_empty())
}

pub fn main(cmd: StorageCmd) -> i32 {
    match cmd {
        StorageCmd::Status { json } => status(json),
        StorageCmd::Warnings { json } => warnings(json),
    }
}

/// The storage lines `apex doctor` prints.
///
/// A `WARN` in the doctor is information rather than a fault, which is exactly
/// the right register for most of this — but a disk reporting itself as
/// failing is not information. Only [`Health::Attention`] rows turn a doctor
/// line false; a row nobody could measure is reported with its reason and
/// passes, because the doctor already says elsewhere that root is needed.
pub fn doctor_lines(roots: &Roots) -> Vec<(bool, String)> {
    match rows(roots) {
        Err(e) => vec![(false, format!("storage could not be read: {e}"))],
        Ok(rows) => rows
            .iter()
            .filter(|r| r.state == Health::Attention || r.state == Health::Unavailable)
            .map(|r| {
                (
                    r.state != Health::Attention,
                    format!("storage: {} — {}", r.label, r.note),
                )
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mountinfo_is_split_on_the_separator_not_on_a_field_index() {
        // The optional-fields run between field 6 and the separator is
        // variable length: `shared:1`, or nothing, or two of them. Every
        // systemd machine has some. A fixed index reads the fstype out of a
        // propagation flag.
        let text = "\
49 1 0:39 / / ro,relatime shared:1 - overlay composefs ro,seclabel
46 49 0:36 /x /etc rw,relatime shared:2 master:9 - btrfs /dev/nvme0n1p5 rw,ssd
23 20 0:22 / /proc rw,nosuid - proc proc rw
";
        let d = fixture_with("proc/self/mountinfo", text);
        let roots = Roots { fixture: Some(d.path().to_path_buf()) };
        let m = mounts(&roots).expect("parse");
        assert_eq!(m.len(), 3);
        assert_eq!(m[0].fstype, "overlay");
        assert_eq!(m[0].source, "composefs");
        assert!(m[0].read_only);
        assert_eq!(m[1].fstype, "btrfs");
        assert_eq!(m[1].source, "/dev/nvme0n1p5");
        assert!(!m[1].read_only, "two optional fields shifted the read-only flag");
        assert_eq!(m[2].source, "proc");
        assert_eq!(m[1].super_options, "rw,ssd");
    }

    #[test]
    fn a_mount_point_with_a_space_in_it_parses() {
        let text = "49 1 0:39 / /mnt/my\\040disk rw,relatime - ext4 /dev/sdb1 rw\n";
        let d = fixture_with("proc/self/mountinfo", text);
        let roots = Roots { fixture: Some(d.path().to_path_buf()) };
        let m = mounts(&roots).expect("parse");
        assert_eq!(m[0].target, "/mnt/my disk");
    }

    #[test]
    fn the_size_node_is_sectors_and_a_sector_is_512_bytes_whatever_the_disk_says() {
        // /sys/block/<d>/size is always in 512-byte units. Multiplying by the
        // logical block size is a factor-of-eight error on a 4K disk, in the
        // direction that makes a full disk look empty.
        let d = fixture_with("sys/block/sda/size", "4000797360\n");
        std::fs::write(d.path().join("sys/block/sda/queue_placeholder"), "").ok();
        let roots = Roots { fixture: Some(d.path().to_path_buf()) };
        let disk = disk(&roots, "sda");
        assert_eq!(disk.size_bytes, Reading::Known(4000797360 * 512));
    }

    #[test]
    fn a_missing_hwmon_is_a_reason_and_not_a_zero() {
        let d = fixture_with("sys/block/sda/size", "1\n");
        let roots = Roots { fixture: Some(d.path().to_path_buf()) };
        match temperature(&roots, "sda") {
            Reading::Unavailable(r) => assert!(r.contains("hwmon"), "{r}"),
            Reading::Known(v) => panic!("a missing hwmon read as {v} °C"),
        }
    }

    #[test]
    fn hwmon_millidegrees_become_degrees() {
        // Measured on the L16: hwmon4 is named nvme and reads 37850.
        // The nvme layout: hwmon4 sits directly under the device, with no
        // `hwmon` directory in between.
        let d = fixture_with("sys/block/nvme0n1/device/hwmon4/temp1_input", "37850\n");
        let roots = Roots { fixture: Some(d.path().to_path_buf()) };
        assert_eq!(temperature(&roots, "nvme0n1"), Reading::Known(37.85));
    }

    #[test]
    fn the_other_hwmon_layout_is_found_too() {
        // The platform drivers nest it: <device>/hwmon/hwmon6. Both layouts
        // are on this machine, and checking only one reports "no sensor" for
        // half the hardware in the world.
        let d = fixture_with("sys/block/sda/device/hwmon/hwmon6/temp1_input", "41000\n");
        let roots = Roots { fixture: Some(d.path().to_path_buf()) };
        assert_eq!(temperature(&roots, "sda"), Reading::Known(41.0));
    }

    #[test]
    fn a_partition_with_no_udev_record_is_never_reported_as_unencrypted() {
        let e = encryption(
            &Reading::Unavailable("no record".into()),
            &Err("/run/udev/data/b8:1: No such file or directory".into()),
        );
        assert!(matches!(e, Encryption::Unknown(_)), "{e:?}");
        let (h, note) = storage::encryption_health(&e);
        assert_eq!(h, Health::Unavailable);
        assert!(!note.contains("not encrypted"), "{note}");
    }

    #[test]
    fn a_luks_partition_is_reported_as_encrypted_with_its_version() {
        let props = BTreeMap::from([
            ("ID_FS_TYPE".to_string(), "crypto_LUKS".to_string()),
            ("ID_FS_VERSION".to_string(), "2".to_string()),
        ]);
        let e = encryption(&Reading::Known("crypto_LUKS".into()), &Ok(props));
        assert_eq!(e, Encryption::Encrypted("LUKS2".into()));
    }

    #[test]
    fn the_udev_record_is_read_for_its_e_lines_and_nothing_else() {
        // The real file starts with S: symlink lines and an I: line, and it
        // carries ID_SERIAL_SHORT — which must be readable here and must not
        // reach a report. This asserts the parse; the report's own test
        // asserts the serial does not appear.
        let text = "S:disk/by-label/apex-root\nI:11132713\n\
                    E:ID_SERIAL_SHORT=NF12312150500322\nE:ID_FS_TYPE=btrfs\n";
        let d = fixture_with("run/udev/data/b259:5", text);
        std::fs::create_dir_all(d.path().join("sys/block/nvme0n1/nvme0n1p5")).expect("mkdir");
        std::fs::write(d.path().join("sys/block/nvme0n1/nvme0n1p5/dev"), "259:5\n").expect("write");
        let roots = Roots { fixture: Some(d.path().to_path_buf()) };
        let props = udev_record(&roots, "/sys/block/nvme0n1/nvme0n1p5").expect("record");
        assert_eq!(props.get("ID_FS_TYPE").map(String::as_str), Some("btrfs"));
        assert_eq!(props.len(), 2, "an S: or I: line was parsed as a property");
    }

    #[test]
    fn a_row_that_could_not_be_measured_does_not_make_the_doctor_line_fail() {
        // A notifier keyed on the doctor's booleans must not fire because
        // somebody ran it without root.
        let unavailable = Row {
            id: "x".into(),
            label: "/dev/sda health".into(),
            state: Health::Unavailable,
            note: "permission denied — run it with sudo".into(),
        };
        let attention = Row {
            id: "y".into(),
            label: "/dev/sda health".into(),
            state: Health::Attention,
            note: "SMART reports this disk as failing".into(),
        };
        assert_ne!(unavailable.state, Health::Attention);
        assert_eq!(attention.state, Health::Attention);
    }

    fn fixture_with(rel: &str, text: &str) -> tempdir::Dir {
        let d = tempdir::Dir::new();
        let p = d.path().join(rel);
        std::fs::create_dir_all(p.parent().expect("parent")).expect("mkdir");
        std::fs::write(&p, text).expect("write");
        d
    }

    mod tempdir {
        use std::path::{Path, PathBuf};
        use std::sync::atomic::{AtomicU64, Ordering};

        static N: AtomicU64 = AtomicU64::new(0);

        pub struct Dir(PathBuf);

        impl Dir {
            pub fn new() -> Dir {
                let base = std::env::temp_dir().join(format!(
                    "apex-storage-test-{}-{}",
                    std::process::id(),
                    N.fetch_add(1, Ordering::Relaxed)
                ));
                let _ = std::fs::remove_dir_all(&base);
                std::fs::create_dir_all(&base).expect("mkdir");
                Dir(base)
            }
            pub fn path(&self) -> &Path {
                &self.0
            }
        }

        impl Drop for Dir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
    }
}
