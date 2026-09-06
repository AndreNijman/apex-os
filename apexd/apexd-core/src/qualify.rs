//! The hardware qualification database. Roadmap §33.
//!
//! §33 asks for a record of "non-personal compatibility results" kept with
//! explicit consent, and for a per-machine confidence readout built from it:
//! this ThinkPad, sleep yes, audio yes, external monitor yes. P0-001 produced
//! exactly that table by hand, in a markdown file, once. This is the shape that
//! survives the next reboot and can be asked a question.
//!
//! ## Three answers, not two
//!
//! P0-001's own table is the argument. It recorded suspend as PARTIAL because
//! the L16 had resumed forty times and katana had never suspended at all — two
//! machines, one verdict each, and neither of them "fails". A `bool` per check
//! turns "nobody has tried this yet" into "this is broken", and a
//! qualification database that cannot say *unchecked* is a database that
//! invents failures for hardware nobody has exercised.
//!
//! So [`Verdict`] has three arms and the third carries a reason. The reason is
//! not decoration: "needs physical displays" and "the reader refused, run it
//! with sudo" are different futures for the same row, and only one of them is
//! anybody's job.
//!
//! ## Consent is a decision, and "never asked" is not "no"
//!
//! [`Consent`] is likewise three-valued. A machine that has never been asked
//! must be distinguishable from one whose owner said no, or the settings page
//! either nags somebody who declined or never asks anybody at all. Nothing is
//! recorded under [`Consent::Unset`] or [`Consent::Declined`] — [`record`]
//! refuses, and its refusal names the check it did not store, so a caller can
//! say what consent would unlock rather than failing silently.
//!
//! The consent decision itself is persisted, because it has to be: an absent
//! file means unasked, and a file holding nothing but `"consent": "declined"`
//! means asked and answered. That is why a declined machine still writes.
//!
//! ## What "non-personal" is enforced to mean
//!
//! [`Machine`] is a fixed set of fields and there is no free-form escape
//! hatch, so the question "could a serial number end up in here" has a
//! structural answer rather than a review-time one. DMI serials and the SMBIOS
//! UUID are `0400 root:root` on a normal machine, but `apex` runs as root often
//! enough — `apex install`, `apex firewall` — that "the kernel will stop us" is
//! not the guarantee it looks like. [`Machine::from_dmi`] takes the four DMI
//! strings it wants by name and cannot be handed a fifth.
//!
//! The machine key is a digest of those fields, which is the property §33
//! actually needs: two identical ThinkPads produce the same key and their
//! results aggregate, while nothing in the key traces back to one of them.
//!
//! ## Nothing here transmits anything
//!
//! §33 says record and expose. There is no upload, no sync and no fleet
//! endpoint in this module, and adding one is a separate decision with a
//! separate consent question — "you may keep this on my disk" is not "you may
//! send this to you".
//!
//! ## Why this store is in the migration framework and the agent's are not
//!
//! `apex schema status` lists five stores it cannot manage, each because it
//! lives in a crate that does not depend on `apexd-core`. This one is *in*
//! `apexd-core`, next to [`crate::fingerprint`] whose output it records, so
//! that reason does not apply and there is no excuse for an unversioned sixth.
//! It is registered in [`crate::migrate::STORES`] as `qualification`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// The schema version this build writes. Mirrored by the `qualification`
/// entry in [`crate::migrate::STORES`], which is where a rollback reads it.
pub const SCHEMA_VERSION: u32 = 1;

// ── consent ──────────────────────────────────────────────────────────────────

/// Whether this machine's owner has agreed to results being kept.
///
/// Three arms because a settings page needs to tell an unanswered question
/// from an answered one. Serialised in lowercase words rather than as a
/// boolean plus a null, so a person reading the file can see which of the
/// three it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Consent {
    /// Nobody has been asked. The state of a machine that has never opened the
    /// page, and never a reason to record anything.
    #[default]
    Unset,
    /// Asked and agreed. The only state under which [`record`] stores a row.
    Granted,
    /// Asked and refused. Recorded so nothing asks again.
    Declined,
}

impl Consent {
    pub fn as_str(self) -> &'static str {
        match self {
            Consent::Unset => "unset",
            Consent::Granted => "granted",
            Consent::Declined => "declined",
        }
    }

    /// One sentence a UI can print under the toggle.
    pub fn note(self) -> &'static str {
        match self {
            Consent::Unset => {
                "nobody has been asked; nothing is being recorded and nothing has been"
            }
            Consent::Granted => "compatibility results are kept on this disk and sent nowhere",
            Consent::Declined => "asked and declined; nothing is recorded and nothing is asked again",
        }
    }
}

/// Why a [`record`] call stored nothing.
///
/// A named error rather than a `bool`, because the caller has something to say
/// in each case and they are not the same sentence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refused {
    /// Consent is [`Consent::Unset`] or [`Consent::Declined`].
    NoConsent { consent: Consent, check: String },
    /// The check id is not in [`CHECKS`].
    UnknownCheck { check: String },
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refused::NoConsent { consent, check } => write!(
                f,
                "not recording {check}: consent is {}. Nothing about this machine is kept until it is granted.",
                consent.as_str()
            ),
            Refused::UnknownCheck { check } => {
                write!(f, "{check} is not a qualification check this build knows")
            }
        }
    }
}

// ── the checks ───────────────────────────────────────────────────────────────

/// One thing a machine can be qualified on.
///
/// The id is the compatibility surface, the same rule the recovery surface's
/// [`crate::recover::RowSpec`] lives under: a settings page keys on it and a
/// rename is a broken page. The label may be reworded freely.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Check {
    pub id: &'static str,
    pub label: &'static str,
    /// What a `pass` on this row actually claims. Written out because the
    /// difference between "the driver bound" and "a game ran" is the whole
    /// difference between P0-001's PASS rows and its PARTIAL ones.
    pub means: &'static str,
}

/// Every check this build records.
///
/// §33's example list — sleep, audio, Wi-Fi, Bluetooth, external monitor, VRR
/// — plus the P0-001 acceptance criteria that are facts about hardware rather
/// than about a build. Deliberately not open-ended: a free-form check id would
/// let two machines record the same question under two spellings and the
/// aggregate would be worthless.
pub const CHECKS: &[Check] = &[
    Check {
        id: "sleep",
        label: "sleep",
        means: "the machine suspended and came back with its session intact",
    },
    Check {
        id: "audio",
        label: "audio",
        means: "sound played out of the built-in output after a fresh boot",
    },
    Check {
        id: "wifi",
        label: "Wi-Fi",
        means: "the wireless interface associated and carried traffic",
    },
    Check {
        id: "bluetooth",
        label: "Bluetooth",
        means: "the adapter powered on and a device paired and connected",
    },
    Check {
        id: "external-monitor",
        label: "external monitor",
        means: "a display on the machine's own video output lit and was configurable",
    },
    Check {
        id: "vrr",
        label: "VRR",
        means: "variable refresh rate engaged on a display that advertises it",
    },
    Check {
        id: "gpu-driver",
        label: "GPU driver",
        means: "the kernel driver bound to every display device and userspace found its driver",
    },
    Check {
        id: "portals",
        label: "desktop portals",
        means: "a screenshot and a screen share went through xdg-desktop-portal",
    },
    Check {
        id: "upgrade",
        label: "upgrade",
        means: "bootc pulled a newer image and the machine rebooted into it",
    },
    Check {
        id: "rollback",
        label: "rollback",
        means: "the machine rebooted into the previous deployment and reached the desktop",
    },
    Check {
        id: "fresh-install",
        label: "fresh install",
        means: "an install onto an empty disk booted to a working session",
    },
    Check {
        id: "secure-boot",
        label: "Secure Boot",
        means: "the firmware is enforcing and the machine boots with it on",
    },
];

/// The check with this id, if this build knows it.
pub fn check(id: &str) -> Option<&'static Check> {
    CHECKS.iter().find(|c| c.id == id)
}

// ── a result ─────────────────────────────────────────────────────────────────

/// What was found when a check was looked at.
///
/// [`Verdict::NotChecked`] is the arm that makes the rest honest, and it is not
/// the absence of a row: a row that says "not checked, needs physical
/// displays" is a fact somebody established, and it is different from a check
/// nobody has ever mentioned.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "verdict", rename_all = "kebab-case")]
pub enum Verdict {
    Pass,
    Fail {
        /// What went wrong, in the words of whoever saw it.
        detail: String,
    },
    NotChecked {
        /// Why not, and what would change it. "needs physical displays" and
        /// "the sysfs read was refused" are both valid and neither is a
        /// failure.
        reason: String,
    },
}

impl Verdict {
    pub fn word(&self) -> &'static str {
        match self {
            Verdict::Pass => "pass",
            Verdict::Fail { .. } => "fail",
            Verdict::NotChecked { .. } => "not checked",
        }
    }

    /// The mark a tick list prints. `?` rather than a blank for not-checked,
    /// because a blank column reads as a pass to anybody skimming.
    pub fn mark(&self) -> &'static str {
        match self {
            Verdict::Pass => "y",
            Verdict::Fail { .. } => "n",
            Verdict::NotChecked { .. } => "?",
        }
    }

    /// The sentence after the mark, if there is one.
    pub fn detail(&self) -> Option<&str> {
        match self {
            Verdict::Pass => None,
            Verdict::Fail { detail } => Some(detail),
            Verdict::NotChecked { reason } => Some(reason),
        }
    }
}

impl Verdict {
    /// The key this arm's sentence is written under on disk, if it has one.
    ///
    /// One place, so the reader and the writer cannot disagree about which key
    /// belongs to the verdict and which is a stray field to be preserved.
    fn payload_key(tag: &str) -> Option<&'static str> {
        match tag {
            "fail" => Some("detail"),
            "not-checked" => Some("reason"),
            _ => None,
        }
    }
}

/// One recorded result: a verdict, when it was seen, and by which build.
///
/// ## Why this is written by hand
///
/// The obvious shape — `#[serde(flatten)]` on the verdict and a second
/// `#[serde(flatten)]` catch-all map for unknown keys — compiles and is wrong.
/// Both flattened fields are offered the same buffered entries, so the map
/// captures `verdict`, `detail` and `reason` as well, and the document then
/// carries each of them twice. Serialising it produced a `duplicate field
/// verdict` on the way back in. Measured, not reasoned: the round-trip test
/// below failed on exactly that before this impl existed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Observation {
    pub verdict: Verdict,
    /// Unix seconds. Supplied by the caller rather than read from the clock
    /// here, which is what lets a test assert on an exact document.
    pub at: u64,
    /// The APEX build this was seen on. §33 lists it, and it is the field that
    /// makes a stale pass visible: "sleep worked, on an image from March".
    pub build: String,
    /// Unknown keys a newer build wrote, kept so a rollback-then-forward cycle
    /// does not delete them. See the note on [`Record`].
    pub rest: Map<String, Value>,
}

impl Serialize for Observation {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        // The unknown keys go down first and the known ones overwrite them, so
        // a `rest` that somehow holds an `at` cannot shadow the real one.
        let mut m = self.rest.clone();
        let tagged = serde_json::to_value(&self.verdict).map_err(serde::ser::Error::custom)?;
        if let Value::Object(fields) = tagged {
            for (k, v) in fields {
                m.insert(k, v);
            }
        }
        m.insert("at".into(), Value::from(self.at));
        m.insert("build".into(), Value::from(self.build.clone()));
        m.serialize(s)
    }
}

impl<'de> Deserialize<'de> for Observation {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        let mut m = Map::<String, Value>::deserialize(d)?;
        let tag = m
            .get("verdict")
            .and_then(Value::as_str)
            .ok_or_else(|| D::Error::custom("an observation with no verdict"))?
            .to_string();
        let mut sub = Map::new();
        if let Some(v) = m.remove("verdict") {
            sub.insert("verdict".into(), v);
        }
        if let Some(k) = Verdict::payload_key(&tag) {
            if let Some(v) = m.remove(k) {
                sub.insert(k.to_string(), v);
            }
        }
        let verdict: Verdict =
            serde_json::from_value(Value::Object(sub)).map_err(D::Error::custom)?;
        let at = m
            .remove("at")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| D::Error::custom("an observation with no timestamp"))?;
        let build = m
            .remove("build")
            .and_then(|v| v.as_str().map(str::to_string))
            .ok_or_else(|| D::Error::custom("an observation with no build"))?;
        Ok(Observation { verdict, at, build, rest: m })
    }
}

// ── the machine ──────────────────────────────────────────────────────────────

/// The non-personal identity of a machine, and the whole of what is recorded
/// about it.
///
/// Every field is a model, a version or a device id — a class of machine
/// rather than one machine. There is no map, no free-form field and no
/// `extra`, which is what makes "no serial numbers" a property of the type
/// instead of a promise in a comment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Machine {
    /// DMI `sys_vendor`, e.g. `LENOVO`.
    pub vendor: String,
    /// DMI `product_family`, e.g. `ThinkPad L16 Gen 2`. The friendly one.
    pub family: String,
    /// DMI `product_name`, e.g. `21SCCTO1WW`. A model code, not a serial:
    /// every unit of a configuration shares it.
    pub product: String,
    /// SMBIOS chassis type. 10 is a notebook.
    pub chassis: u32,
    /// `/proc/cpuinfo`'s model name.
    pub cpu: String,
    /// PCI `vendor:device` per display device, ascending, deduplicated.
    pub gpus: Vec<String>,
    /// `uname -r`.
    pub kernel: String,
    /// DMI `bios_version`. §33's "firmware".
    pub firmware: String,
}

impl Machine {
    /// Build the machine identity from named DMI fields.
    ///
    /// The arguments are the four DMI strings this record carries and there is
    /// no variadic tail, so `product_serial` cannot arrive by accident. That is
    /// the enforcement §33's "non-personal" needs on a machine where `apex`
    /// sometimes runs as root and the `0400` mode on the serial nodes stops
    /// being a barrier.
    #[allow(clippy::too_many_arguments)]
    pub fn from_dmi(
        sys_vendor: &str,
        product_family: &str,
        product_name: &str,
        bios_version: &str,
        chassis_type: u32,
        cpu_model: &str,
        gpu_pci_ids: &[String],
        kernel: &str,
    ) -> Machine {
        let mut gpus: Vec<String> = gpu_pci_ids.to_vec();
        gpus.sort();
        gpus.dedup();
        Machine {
            vendor: sys_vendor.trim().to_string(),
            family: product_family.trim().to_string(),
            product: product_name.trim().to_string(),
            chassis: chassis_type,
            cpu: cpu_model.trim().to_string(),
            gpus,
            kernel: kernel.trim().to_string(),
            firmware: bios_version.trim().to_string(),
        }
    }

    /// A stable, non-personal key for this class of machine.
    ///
    /// Deliberately excludes the kernel and the firmware version: both change
    /// under the same laptop, and a key that changed with them would file every
    /// update as a new machine and the confidence readout would reset on every
    /// upgrade. What is left is model plus silicon, which is the granularity
    /// §33's aggregate wants.
    ///
    /// FNV-1a rather than a cryptographic hash. This is a bucket name, not a
    /// commitment: it needs to be stable across builds and cheap, and no
    /// security property rests on it — collisions merge two identical models,
    /// which is what the database is for.
    pub fn key(&self) -> String {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        let mut eat = |s: &str| {
            for b in s.as_bytes() {
                h ^= *b as u64;
                h = h.wrapping_mul(0x1000_0000_01b3);
            }
            // A separator, so ("ab","c") and ("a","bc") are different keys.
            h ^= 0xff;
            h = h.wrapping_mul(0x1000_0000_01b3);
        };
        eat(&self.vendor);
        eat(&self.family);
        eat(&self.product);
        eat(&self.chassis.to_string());
        eat(&self.cpu);
        for g in &self.gpus {
            eat(g);
        }
        format!("{h:016x}")
    }

    /// The one-line name a report prints.
    pub fn title(&self) -> String {
        let name = if self.family.is_empty() { &self.product } else { &self.family };
        if name.is_empty() {
            if self.vendor.is_empty() {
                "unidentified machine".to_string()
            } else {
                self.vendor.clone()
            }
        } else if self.vendor.is_empty() {
            name.clone()
        } else {
            format!("{} {}", self.vendor, name)
        }
    }
}

// ── the record and the database ──────────────────────────────────────────────

/// Everything kept about one class of machine.
///
/// ## Why `rest` exists
///
/// [`crate::migrate::Rollback::ReadableByOlder`] binds the *reader* as well as
/// the migration: an older build has to preserve keys it does not understand
/// when it rewrites the file, or rolling back deletes the newer build's fields
/// on the next save. A struct that simply drops unknown keys satisfies serde
/// and breaks that contract silently. `#[serde(flatten)]` into a map is how
/// this build keeps its side of it for every build after it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Record {
    pub machine: Machine,
    /// Check id to the last observation of it. A map rather than a list: one
    /// verdict per check per machine, and the newest wins.
    #[serde(default)]
    pub results: BTreeMap<String, Observation>,
    #[serde(flatten)]
    pub rest: Map<String, Value>,
}

/// The file: one consent decision, and a record per machine class.
///
/// Several records rather than one because a qualification database that only
/// ever holds the machine it is running on cannot answer §33's question — "is
/// this model known to sleep" is a question about a model, and the answer has
/// to be importable from somewhere. [`merge`] is that door, and it is a file
/// somebody hands over rather than a network call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Db {
    /// The schema key `apex schema status` reads. Named `schema` to match the
    /// other machine-written stores.
    #[serde(default)]
    pub schema: u32,
    #[serde(default)]
    pub consent: Consent,
    #[serde(default)]
    pub machines: BTreeMap<String, Record>,
    #[serde(flatten)]
    pub rest: Map<String, Value>,
}

impl Db {
    /// An empty database at this build's schema.
    pub fn new() -> Db {
        Db { schema: SCHEMA_VERSION, ..Default::default() }
    }

    pub fn parse(text: &str) -> Result<Db, String> {
        serde_json::from_str(text).map_err(|e| e.to_string())
    }

    pub fn to_text(&self) -> Result<String, String> {
        serde_json::to_string_pretty(self).map(|s| s + "\n").map_err(|e| e.to_string())
    }
}

/// Store one result, if consent allows it.
///
/// The consent check is the first thing that happens and it is the only gate:
/// there is no path into [`Db::machines`] that does not pass through here.
pub fn record(
    db: &mut Db,
    machine: &Machine,
    check_id: &str,
    verdict: Verdict,
    at: u64,
    build: &str,
) -> Result<String, Refused> {
    if db.consent != Consent::Granted {
        return Err(Refused::NoConsent {
            consent: db.consent,
            check: check_id.to_string(),
        });
    }
    if check(check_id).is_none() {
        return Err(Refused::UnknownCheck { check: check_id.to_string() });
    }
    let key = machine.key();
    let rec = db.machines.entry(key.clone()).or_insert_with(|| Record {
        machine: machine.clone(),
        ..Default::default()
    });
    // The stored machine is refreshed rather than left at whatever the first
    // observation saw: a kernel or firmware upgrade does not change the key,
    // so without this the record would keep naming the kernel the machine was
    // first qualified on and the readout would age silently.
    rec.machine = machine.clone();
    rec.results.insert(
        check_id.to_string(),
        Observation { verdict, at, build: build.to_string(), rest: Map::new() },
    );
    Ok(key)
}

/// Set the consent decision.
///
/// Revoking it deletes what was kept. "Stop recording" and "and forget what you
/// already recorded" are the same request from the user's side, and a database
/// that kept the rows after a withdrawal would be keeping data under a consent
/// that no longer exists.
pub fn set_consent(db: &mut Db, consent: Consent) {
    db.consent = consent;
    if consent != Consent::Granted {
        db.machines.clear();
    }
}

// ── querying ─────────────────────────────────────────────────────────────────

/// One line of the confidence readout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub id: &'static str,
    pub label: &'static str,
    pub means: &'static str,
    /// `None` when nothing has ever been recorded for this check on this
    /// machine. Distinct from a stored [`Verdict::NotChecked`], which is
    /// somebody having looked and said why not.
    pub observed: Option<Observation>,
}

impl Row {
    pub fn mark(&self) -> &'static str {
        match &self.observed {
            Some(o) => o.verdict.mark(),
            // Nothing recorded at all. Same mark as an explicit not-checked,
            // because both mean "this is not known" — the difference is in the
            // sentence beside it, not in the tick.
            None => "?",
        }
    }

    pub fn note(&self) -> String {
        match &self.observed {
            None => "no result recorded".to_string(),
            Some(o) => match o.verdict.detail() {
                Some(d) => d.to_string(),
                None => format!("on {}", o.build),
            },
        }
    }
}

/// §33's tick list for one machine: every check, in catalogue order, with
/// whatever is known about it.
///
/// Built from [`CHECKS`] rather than from the stored rows, so a check this
/// build added appears as unknown instead of vanishing, and a stored row for a
/// check this build has retired does not invent a line.
pub fn confidence(db: &Db, key: &str) -> Vec<Row> {
    let rec = db.machines.get(key);
    CHECKS
        .iter()
        .map(|c| Row {
            id: c.id,
            label: c.label,
            means: c.means,
            observed: rec.and_then(|r| r.results.get(c.id)).cloned(),
        })
        .collect()
}

/// Counts for a summary badge: passed, failed, and everything not known.
///
/// Two numbers would force a caller to compute the third, and the caller that
/// computes it as `total - passed` is the caller that paints an unchecked row
/// red.
pub fn tally(rows: &[Row]) -> (usize, usize, usize) {
    let mut pass = 0;
    let mut fail = 0;
    let mut unknown = 0;
    for r in rows {
        match r.observed.as_ref().map(|o| &o.verdict) {
            Some(Verdict::Pass) => pass += 1,
            Some(Verdict::Fail { .. }) => fail += 1,
            _ => unknown += 1,
        }
    }
    (pass, fail, unknown)
}

/// Fold another database's records into this one.
///
/// The import door §33's aggregate needs, with three rules that are all the
/// same rule: the receiving machine's consent decides. An import into a
/// database without consent stores nothing, an imported record never carries
/// the other file's consent decision across, and a newer observation wins over
/// an older one so replaying an old export cannot undo a fresh result.
///
/// Returns the number of observations taken.
pub fn merge(db: &mut Db, other: &Db) -> Result<usize, Refused> {
    if db.consent != Consent::Granted {
        return Err(Refused::NoConsent {
            consent: db.consent,
            check: "an imported database".to_string(),
        });
    }
    let mut taken = 0;
    for (key, rec) in &other.machines {
        let mine = db
            .machines
            .entry(key.clone())
            .or_insert_with(|| Record { machine: rec.machine.clone(), ..Default::default() });
        for (id, obs) in &rec.results {
            if check(id).is_none() {
                continue;
            }
            let newer = match mine.results.get(id) {
                Some(existing) => obs.at > existing.at,
                None => true,
            };
            if newer {
                mine.results.insert(id.clone(), obs.clone());
                taken += 1;
            }
        }
    }
    Ok(taken)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn l16() -> Machine {
        Machine::from_dmi(
            "LENOVO",
            "ThinkPad L16 Gen 2",
            "21SCCTO1WW",
            "R2UET31W (1.31 )",
            10,
            "AMD Ryzen 7 PRO 8840HS",
            &["1002:1900".to_string()],
            "7.2.3-cachyos2.fc43.x86_64",
        )
    }

    fn granted() -> Db {
        let mut db = Db::new();
        set_consent(&mut db, Consent::Granted);
        db
    }

    #[test]
    fn nothing_is_recorded_without_consent() {
        for c in [Consent::Unset, Consent::Declined] {
            let mut db = Db::new();
            db.consent = c;
            let err = record(&mut db, &l16(), "sleep", Verdict::Pass, 1, "x")
                .expect_err("must refuse");
            assert_eq!(err, Refused::NoConsent { consent: c, check: "sleep".into() });
            assert!(db.machines.is_empty(), "a refused record stored something");
            // The refusal has to name the check, or a caller cannot say what
            // consent would unlock.
            assert!(err.to_string().contains("sleep"), "{err}");
        }
    }

    #[test]
    fn a_granted_record_lands_under_the_machine_key() {
        let mut db = granted();
        let m = l16();
        let key = record(&mut db, &m, "sleep", Verdict::Pass, 1788700000, "308127d9")
            .expect("granted");
        assert_eq!(key, m.key());
        let rec = &db.machines[&key];
        assert_eq!(rec.machine, m);
        assert_eq!(rec.results["sleep"].verdict, Verdict::Pass);
        assert_eq!(rec.results["sleep"].build, "308127d9");
    }

    #[test]
    fn an_unknown_check_is_refused_rather_than_invented() {
        let mut db = granted();
        let err = record(&mut db, &l16(), "telepathy", Verdict::Pass, 1, "x")
            .expect_err("must refuse");
        assert_eq!(err, Refused::UnknownCheck { check: "telepathy".into() });
        assert!(db.machines.is_empty());
    }

    #[test]
    fn withdrawing_consent_deletes_what_was_kept() {
        let mut db = granted();
        record(&mut db, &l16(), "audio", Verdict::Pass, 1, "x").expect("granted");
        assert!(!db.machines.is_empty());
        set_consent(&mut db, Consent::Declined);
        assert!(db.machines.is_empty(), "results survived a withdrawal");
        assert_eq!(db.consent, Consent::Declined);
    }

    #[test]
    fn not_checked_is_not_a_failure_and_carries_its_reason() {
        let mut db = granted();
        let m = l16();
        record(
            &mut db,
            &m,
            "external-monitor",
            Verdict::NotChecked { reason: "needs physical displays".into() },
            1,
            "x",
        )
        .expect("granted");
        let rows = confidence(&db, &m.key());
        let row = rows.iter().find(|r| r.id == "external-monitor").expect("row");
        assert_eq!(row.mark(), "?");
        assert_eq!(row.note(), "needs physical displays");
        let (pass, fail, unknown) = tally(&rows);
        assert_eq!(fail, 0, "a not-checked row counted as a failure");
        assert_eq!(pass, 0);
        assert_eq!(unknown, CHECKS.len());
    }

    #[test]
    fn a_row_nobody_recorded_reads_as_unknown_not_as_a_pass() {
        let db = granted();
        let rows = confidence(&db, "nosuchmachine");
        assert_eq!(rows.len(), CHECKS.len());
        for r in &rows {
            assert_eq!(r.mark(), "?", "{}", r.id);
            assert_eq!(r.note(), "no result recorded");
        }
        let (pass, fail, unknown) = tally(&rows);
        assert_eq!((pass, fail, unknown), (0, 0, CHECKS.len()));
    }

    #[test]
    fn the_readout_is_built_from_the_catalogue_not_from_the_file() {
        // A stored row for a check this build does not have must not create a
        // line, and a check with no stored row must still appear.
        let mut db = granted();
        let m = l16();
        record(&mut db, &m, "audio", Verdict::Pass, 1, "x").expect("granted");
        db.machines.get_mut(&m.key()).expect("record").results.insert(
            "levitation".into(),
            Observation {
                verdict: Verdict::Pass,
                at: 1,
                build: "x".into(),
                rest: Map::new(),
            },
        );
        let rows = confidence(&db, &m.key());
        assert_eq!(rows.len(), CHECKS.len());
        assert!(!rows.iter().any(|r| r.id == "levitation"));
        assert!(rows.iter().any(|r| r.id == "vrr" && r.observed.is_none()));
    }

    #[test]
    fn the_key_ignores_the_kernel_and_the_firmware() {
        // A machine that took an update is the same machine. If the key moved,
        // every upgrade would reset the confidence readout to unknown.
        let a = l16();
        let mut b = a.clone();
        b.kernel = "7.4.0-cachyos1.fc44.x86_64".into();
        b.firmware = "R2UET40W (1.40 )".into();
        assert_eq!(a.key(), b.key());
    }

    #[test]
    fn the_key_separates_fields_so_a_shifted_boundary_is_a_different_machine() {
        let a = Machine::from_dmi("LENOVO", "ThinkPad", "L16", "", 10, "cpu", &[], "k");
        let b = Machine::from_dmi("LENOVO", "ThinkPadL", "16", "", 10, "cpu", &[], "k");
        assert_ne!(a.key(), b.key());
    }

    #[test]
    fn a_different_gpu_is_a_different_machine() {
        let a = l16();
        let mut b = a.clone();
        b.gpus = vec!["10de:24dd".into()];
        assert_ne!(a.key(), b.key());
    }

    #[test]
    fn gpu_ids_are_ordered_so_enumeration_order_does_not_change_the_key() {
        let a = Machine::from_dmi(
            "V", "F", "P", "", 3, "cpu",
            &["10de:24dd".into(), "8086:46a6".into()],
            "k",
        );
        let b = Machine::from_dmi(
            "V", "F", "P", "", 3, "cpu",
            &["8086:46a6".into(), "10de:24dd".into()],
            "k",
        );
        assert_eq!(a.key(), b.key());
        assert_eq!(a.gpus, b.gpus);
    }

    #[test]
    fn the_recorded_machine_is_refreshed_on_every_write() {
        let mut db = granted();
        let old = l16();
        record(&mut db, &old, "audio", Verdict::Pass, 1, "x").expect("granted");
        let mut new = old.clone();
        new.kernel = "7.4.0".into();
        record(&mut db, &new, "wifi", Verdict::Pass, 2, "y").expect("granted");
        assert_eq!(db.machines[&old.key()].machine.kernel, "7.4.0");
    }

    /// The whole of the "non-personal" claim, asserted rather than reviewed.
    #[test]
    fn no_serial_number_can_reach_the_record() {
        // Values a root-run probe could read from DMI and must never store.
        const SECRETS: &[&str] = &[
            "PF0ABCDE",                             // product_serial
            "L1HF12345678",                         // board_serial
            "0f2c6c1e-1f0d-11ee-be56-0242ac120002", // product_uuid
            "aa:bb:cc:dd:ee:ff",                    // a MAC address
        ];
        let mut db = granted();
        let m = l16();
        record(&mut db, &m, "sleep", Verdict::Pass, 1, "308127d9").expect("granted");
        let text = db.to_text().expect("serialise");
        for s in SECRETS {
            assert!(
                !text.contains(s),
                "the record carries {s}, which is about one machine rather than a model"
            );
        }
        // And the structural half: the identity type has no free-form map, so
        // there is nowhere for a fifth DMI field to be put without changing
        // this test's list of fields too.
        let v: Value = serde_json::from_str(&text).expect("json");
        let fields: Vec<&str> = v["machines"][m.key()]["machine"]
            .as_object()
            .expect("machine object")
            .keys()
            .map(|s| s.as_str())
            .collect();
        assert_eq!(
            fields,
            vec!["chassis", "cpu", "family", "firmware", "gpus", "kernel", "product", "vendor"],
            "a field was added to the machine identity without revisiting what it says about one unit"
        );
    }

    #[test]
    fn an_unknown_key_survives_a_read_and_a_write() {
        // The reader half of `ReadableByOlder`. A build that dropped this would
        // delete a newer build's fields the first time it saved after a
        // rollback, and nothing would say so.
        let text = r#"{
          "schema": 1,
          "consent": "granted",
          "somethingNewer": {"a": 1},
          "machines": {
            "abc": {
              "machine": {"vendor":"LENOVO","family":"L16","product":"21S","chassis":10,
                          "cpu":"ryzen","gpus":["1002:1900"],"kernel":"7.2","firmware":"R2"},
              "futureField": 7,
              "results": {"sleep": {"verdict":"pass","at":1,"build":"x","futureDetail":"keep me"}}
            }
          }
        }"#;
        let db = Db::parse(text).expect("parse");
        let out = db.to_text().expect("serialise");
        assert!(out.contains("somethingNewer"), "{out}");
        assert!(out.contains("futureField"), "{out}");
        assert!(out.contains("futureDetail"), "{out}");
        // And it still reads as itself.
        assert_eq!(Db::parse(&out).expect("reparse"), db);
    }

    #[test]
    fn a_round_trip_keeps_every_verdict_arm() {
        let mut db = granted();
        let m = l16();
        record(&mut db, &m, "sleep", Verdict::Pass, 1, "b").expect("granted");
        record(
            &mut db,
            &m,
            "vrr",
            Verdict::Fail { detail: "the panel advertises it and it never engaged".into() },
            2,
            "b",
        )
        .expect("granted");
        record(
            &mut db,
            &m,
            "rollback",
            Verdict::NotChecked { reason: "the reboot has not been exercised".into() },
            3,
            "b",
        )
        .expect("granted");
        let back = Db::parse(&db.to_text().expect("serialise")).expect("parse");
        assert_eq!(back, db);
        let rows = confidence(&back, &m.key());
        assert_eq!(tally(&rows), (1, 1, CHECKS.len() - 2));
    }

    #[test]
    fn an_import_without_consent_takes_nothing() {
        let mut mine = Db::new();
        let mut theirs = granted();
        record(&mut theirs, &l16(), "sleep", Verdict::Pass, 1, "x").expect("granted");
        let err = merge(&mut mine, &theirs).expect_err("must refuse");
        assert!(matches!(err, Refused::NoConsent { .. }));
        assert!(mine.machines.is_empty());
    }

    #[test]
    fn an_import_never_carries_the_other_files_consent() {
        let mut mine = granted();
        let mut theirs = Db::new();
        theirs.consent = Consent::Declined;
        theirs.machines.insert(
            l16().key(),
            Record {
                machine: l16(),
                results: BTreeMap::from([(
                    "audio".to_string(),
                    Observation {
                        verdict: Verdict::Pass,
                        at: 5,
                        build: "x".into(),
                        rest: Map::new(),
                    },
                )]),
                rest: Map::new(),
            },
        );
        assert_eq!(merge(&mut mine, &theirs).expect("granted"), 1);
        assert_eq!(mine.consent, Consent::Granted, "the import moved the consent decision");
    }

    #[test]
    fn an_older_export_cannot_undo_a_newer_result() {
        let mut mine = granted();
        let m = l16();
        record(
            &mut mine,
            &m,
            "sleep",
            Verdict::Fail { detail: "did not resume".into() },
            100,
            "new",
        )
        .expect("granted");
        let mut theirs = Db::new();
        theirs.machines.insert(
            m.key(),
            Record {
                machine: m.clone(),
                results: BTreeMap::from([(
                    "sleep".to_string(),
                    Observation {
                        verdict: Verdict::Pass,
                        at: 50,
                        build: "old".into(),
                        rest: Map::new(),
                    },
                )]),
                rest: Map::new(),
            },
        );
        assert_eq!(merge(&mut mine, &theirs).expect("granted"), 0);
        assert_eq!(
            mine.machines[&m.key()].results["sleep"].verdict,
            Verdict::Fail { detail: "did not resume".into() }
        );
    }

    #[test]
    fn an_import_drops_checks_this_build_does_not_know() {
        let mut mine = granted();
        let m = l16();
        let mut theirs = Db::new();
        theirs.machines.insert(
            m.key(),
            Record {
                machine: m.clone(),
                results: BTreeMap::from([(
                    "levitation".to_string(),
                    Observation {
                        verdict: Verdict::Pass,
                        at: 5,
                        build: "x".into(),
                        rest: Map::new(),
                    },
                )]),
                rest: Map::new(),
            },
        );
        assert_eq!(merge(&mut mine, &theirs).expect("granted"), 0);
        assert!(!mine.machines[&m.key()].results.contains_key("levitation"));
    }

    #[test]
    fn every_check_says_what_a_pass_claims() {
        let mut ids: Vec<&str> = CHECKS.iter().map(|c| c.id).collect();
        let n = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), n, "two checks share an id");
        for c in CHECKS {
            assert!(!c.id.contains(' '), "{} is a UI key, not a sentence", c.id);
            assert!(!c.label.is_empty(), "{}", c.id);
            assert!(
                c.means.len() > 25,
                "{}: a pass has to say what it claims, or the row means nothing",
                c.id
            );
        }
        // §33 names these six by example; losing one is losing the feature.
        for want in ["sleep", "audio", "wifi", "bluetooth", "external-monitor", "vrr"] {
            assert!(check(want).is_some(), "{want} is missing from the catalogue");
        }
    }

    #[test]
    fn a_title_never_reads_as_an_empty_string() {
        assert_eq!(l16().title(), "LENOVO ThinkPad L16 Gen 2");
        let mut m = Machine::default();
        assert_eq!(m.title(), "unidentified machine");
        m.vendor = "LENOVO".into();
        assert_eq!(m.title(), "LENOVO");
        m.product = "21SCCTO1WW".into();
        assert_eq!(m.title(), "LENOVO 21SCCTO1WW");
    }

    #[test]
    fn consent_notes_say_what_is_and_is_not_happening() {
        for c in [Consent::Unset, Consent::Granted, Consent::Declined] {
            assert!(c.note().len() > 30, "{}", c.as_str());
        }
        assert!(Consent::Granted.note().contains("sent nowhere"));
    }
}
