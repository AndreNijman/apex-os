//! §P2-015 — firmware and hardware lifecycle: what fwupd knows, read safely.
//!
//! ── Why the exit status is never consulted ───────────────────────────────────
//!
//! `fwupdmgr`'s exit status is useless in **both** directions, and both were
//! measured on the development machine at the same moment:
//!
//! * `fwupdmgr get-updates --json`, nothing to update → **exit 0**, body
//!   `{"Devices": []}`.
//! * `fwupdmgr get-updates`, no `--json`, same machine, same moment →
//!   **exit 2**, the full device list on stdout, `No updates available` on
//!   stderr. So the *success* case is non-zero in one form and zero in the
//!   other, and a caller that keeps the exit check but drops `--json` reads a
//!   healthy machine as a broken firmware subsystem.
//! * `fwupdmgr get-releases --json <a device id that does not exist>` →
//!   **exit 0**, body `{"Error": {"Domain": "FwupdError", "Code": 8,
//!   "Message": "failed to find …"}}`. So exit zero can also mean the request
//!   failed outright.
//!
//! That is the same trap family as this unit's three smartctl findings and the
//! `blkid` one — exit zero with no data — except fwupd manages both endings at
//! once. Hence: the DOCUMENT is the only truth, [`parse_devices`] takes no exit
//! code at all, and an `Error` object is checked for before anything else.
//!
//! ── Why the filter is `Plugin` and not the `updatable` flag ──────────────────
//!
//! Most of what `get-devices` returns is not hardware. Measured: **28 rows, of
//! which 12 are Secure Boot key and revocation stores** — `UEFI CA`,
//! `KEK CA`, `PK CA`, `Windows UEFI CA`, `Option ROM UEFI CA`,
//! `ThinkPad Product CA`, `UEFI dbx`, `SBAT`, and the two container rows
//! `UEFI Signature Database` and `UEFI Key Exchange Key`.
//!
//! The obvious filter — keep the rows flagged `updatable` — **does not work,
//! and measuring it is the only way to find that out.** Eleven rows carry
//! `updatable`, and **nine of the eleven are certificate stores**: only the
//! Elan touchpad and the Integrated RGB Camera are real devices. Worse, that
//! list contains `KEK CA` twice and `UEFI CA` twice, so the readout it
//! produces is both mostly noise and internally ambiguous. `internal` is no
//! better: the touchpad, the CPU, the TPM and the NVMe are all `internal`.
//!
//! The stores are distinguished by their **plugin** — see
//! [`SECURE_BOOT_PLUGINS`] — and `uefi_capsule` is real system firmware, five
//! of whose six rows are all named `UEFI Device Firmware`. An **unknown
//! plugin counts as hardware**, deliberately: a new fwupd plugin should show
//! up in the readout and be wrong there, rather than be silently hidden.

use serde_json::Value;

use crate::storage::Reading;

/// The plugins that report Secure Boot key and revocation stores rather than
/// firmware on a piece of hardware.
///
/// Measured against this machine's own 28-row document, which contains twelve
/// such rows. `uefi_capsule` is deliberately absent — that one is real system
/// firmware. See the module comment for why the `updatable` flag cannot do
/// this job.
pub const SECURE_BOOT_PLUGINS: &[&str] =
    &["uefi_db", "uefi_dbx", "uefi_kek", "uefi_pk", "uefi_sbat"];

/// Whether fwupd could write this device **right now**.
///
/// ── The measurement that made this an enum ───────────────────────────────────
///
/// `updatable` looks like a property of a device and is not. Two
/// `get-devices` calls minutes apart on an idle development machine:
///
/// ```text
/// first  call: 28 devices, 11 updatable, 7 rows carrying require-ac-power
/// second call: 28 devices, 18 updatable, 0 rows carrying require-ac-power
/// ```
///
/// The **seven** devices that gained `updatable` are **exactly the seven**
/// that had carried `require-ac-power` — the laptop had been on battery and
/// was now plugged in. The overlap between "updatable" and "has a problem" is
/// **zero in both documents**: fwupd *removes* the flag while a blocker stands
/// and puts the reason in `Problems` instead.
///
/// So a readout with an `[updatable]` / `[read-only]` column flips seven of
/// sixteen hardware rows between two runs because somebody unplugged a cable,
/// and on battery it reports the System Firmware and the NVMe as things that
/// cannot be updated at all. Both are lies of the same kind the rest of this
/// unit exists to stop: a momentary condition printed as a capability.
///
/// Hence three states rather than a boolean, and [`Writable::Blocked`] carries
/// the reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Writable {
    /// fwupd says it can write this now.
    Now,
    /// fwupd will not write it at this moment, and said why. Transient: on
    /// this machine, "plug it in".
    Blocked(Vec<String>),
    /// No `updatable` flag and no stated problem — fwupd reports this device
    /// but does not offer to write it at all.
    Never,
}

/// What a fwupd row is actually describing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Firmware on a piece of hardware. The default for a plugin nobody here
    /// has heard of, so that a new one is visible rather than hidden.
    Hardware,
    /// A Secure Boot key or revocation store. Real, updatable, and not a
    /// component of this machine.
    SecureBootStore,
}

/// One row of `fwupdmgr get-devices --json`.
///
/// `name` and `version` are `Option` because on this machine's own output they
/// are **genuinely absent keys**, not nulls: one row of 28 (`linux_display`)
/// has no `Name` at all, and **four** rows have no `Version`
/// (`linux_display`, `GPIO controller`, `UEFI Key Exchange Key`,
/// `UEFI Signature Database`). A reader that indexes either unconditionally
/// panics on the output of the machine it was written on.
#[derive(Debug, Clone)]
pub struct Device {
    /// fwupd's own identity for the row: 40 hex characters.
    ///
    /// **This is the identity and the name is not.** `UEFI Device Firmware`
    /// names five different rows on this machine, with five different
    /// versions; `KEK CA` and `UEFI CA` name two each.
    pub device_id: String,
    pub name: Option<String>,
    pub version: Option<String>,
    /// `hex`, `bcd`, `number`, `triplet`, `quad`, `plain`. Kept because it is
    /// the difference between `0x0006` being a version and being a bug: the
    /// touchpad really does report `0x0006` and the CPU `0x0a70520a`.
    pub version_format: Option<String>,
    pub vendor: Option<String>,
    pub plugin: Option<String>,
    pub flags: Vec<String>,
    /// Why fwupd would refuse to update this right now. Measured: seven rows
    /// carry `require-ac-power`.
    pub problems: Vec<String>,
}

impl Device {
    /// Hardware, or a Secure Boot store.
    pub fn kind(&self) -> Kind {
        match self.plugin.as_deref() {
            Some(p) if SECURE_BOOT_PLUGINS.contains(&p) => Kind::SecureBootStore,
            _ => Kind::Hardware,
        }
    }

    /// Whether the `updatable` flag is present.
    ///
    /// Prefer [`Device::writable`]: this is a momentary condition and reading
    /// it as a capability is the trap documented on [`Writable`].
    pub fn is_updatable(&self) -> bool {
        self.flags.iter().any(|f| f == "updatable")
    }

    /// Whether fwupd could write this device right now, and if not, why not.
    pub fn writable(&self) -> Writable {
        if self.is_updatable() {
            return Writable::Now;
        }
        let why = self.blockers();
        if why.is_empty() {
            Writable::Never
        } else {
            Writable::Blocked(why)
        }
    }

    /// Whether applying an update here needs the machine restarted.
    pub fn needs_reboot(&self) -> bool {
        self.flags.iter().any(|f| f == "needs-reboot")
    }

    /// The first eight characters of the device id — enough to tell five rows
    /// called `UEFI Device Firmware` apart in a terminal, short enough to read
    /// out loud.
    pub fn short_id(&self) -> &str {
        let n = self.device_id.len().min(8);
        &self.device_id[..n]
    }

    /// Something to call this row that is never empty and never a lie.
    ///
    /// The name if it has one; otherwise the plugin, which is what the row is
    /// really identified by when fwupd declined to name it; otherwise the
    /// short id. `linux_display` on this machine has no name at all, and
    /// printing an empty column for it is how a readout loses a row.
    pub fn label(&self) -> String {
        match (&self.name, &self.plugin) {
            (Some(n), _) if !n.trim().is_empty() => n.clone(),
            (_, Some(p)) => format!("unnamed {p} device"),
            _ => format!("unnamed device {}", self.short_id()),
        }
    }

    /// The version, or the reason there is not one.
    ///
    /// A `Reading` rather than an `Option<String>` for the reason the rest of
    /// this unit uses one: "fwupd did not report a version for this device" is
    /// a fact about the row, and it must not be printed as a blank that reads
    /// like a version of nothing.
    pub fn version_text(&self) -> Reading<String> {
        match &self.version {
            Some(v) if !v.trim().is_empty() => Reading::Known(v.clone()),
            _ => Reading::Unavailable(format!(
                "fwupd reports no version for {} — the device answers, its \
                 firmware revision does not",
                self.label()
            )),
        }
    }

    /// What stands between this device and an update, in words.
    pub fn blockers(&self) -> Vec<String> {
        self.problems.iter().map(|p| problem_sentence(p)).collect()
    }
}

/// One of fwupd's `Problems` strings, as something to do about it.
pub fn problem_sentence(problem: &str) -> String {
    match problem {
        "require-ac-power" => {
            "plug the machine in — fwupd will not write firmware on battery".into()
        }
        "lid-is-closed" => "open the lid before updating this".into(),
        "update-pending" => {
            "an update is already staged here; restart to finish it first".into()
        }
        "unreachable" => "fwupd cannot reach this device at the moment".into(),
        other => format!("fwupd reports the problem {other:?}"),
    }
}

/// Every row of a `fwupdmgr … --json` document.
///
/// Takes no exit code, on purpose — see the module comment. The three checks,
/// in this order, are the whole point:
///
/// 1. Does it parse at all.
/// 2. Is there an `Error` object. **fwupd returns exit 0 with one**, so this
///    has to be looked for rather than inferred from a status.
/// 3. Is there a `Devices` array. A document with neither `Error` nor
///    `Devices` is a document nobody can read, and it is reported as such —
///    **never as an empty list of devices**, which is the reading that would
///    turn a broken fwupd into a machine with nothing to update.
pub fn parse_devices(text: &str) -> Result<Vec<Device>, String> {
    let v: Value = serde_json::from_str(text.trim())
        .map_err(|e| format!("fwupd did not return JSON: {e}"))?;

    if let Some(err) = v.get("Error") {
        let msg = err
            .get("Message")
            .and_then(Value::as_str)
            .unwrap_or("fwupd gave no message");
        return match err.get("Code").and_then(Value::as_i64) {
            Some(code) => Err(format!("fwupd refused the request: {msg} (code {code})")),
            None => Err(format!("fwupd refused the request: {msg}")),
        };
    }

    let rows = v
        .get("Devices")
        .ok_or_else(|| {
            "fwupd's answer has neither an Error nor a Devices list, so nothing \
             about this machine's firmware was read"
                .to_string()
        })?
        .as_array()
        .ok_or_else(|| "fwupd's Devices field is not a list".to_string())?;

    Ok(rows.iter().map(row_to_device).collect())
}

fn row_to_device(row: &Value) -> Device {
    let text = |k: &str| row.get(k).and_then(Value::as_str).map(str::to_string);
    let list = |k: &str| {
        row.get(k)
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
            .unwrap_or_default()
    };
    Device {
        // A row with no DeviceId is malformed rather than unreadable, and an
        // empty string is honest about it: `short_id` and `label` both cope,
        // and nothing keys a decision on the id being non-empty.
        device_id: text("DeviceId").unwrap_or_default(),
        name: text("Name"),
        version: text("Version"),
        version_format: text("VersionFormat"),
        vendor: text("Vendor"),
        plugin: text("Plugin"),
        flags: list("Flags"),
        problems: list("Problems"),
    }
}

/// What `get-devices` and `get-updates` add up to.
#[derive(Debug, Clone)]
pub struct Report {
    /// Firmware on hardware, in the order fwupd listed it.
    pub hardware: Vec<Device>,
    /// Secure Boot key and revocation stores — real and updatable, but not
    /// components. Kept apart rather than dropped: `fwupdmgr update` will
    /// offer them, so a readout that hides them cannot explain what happened.
    pub secure_boot: Vec<Device>,
    /// What `get-updates` offered, or the reason nobody knows.
    pub pending: Reading<Vec<Device>>,
}

impl Report {
    /// Split one `get-devices` document, and take the pending list as given.
    pub fn new(devices: Vec<Device>, pending: Reading<Vec<Device>>) -> Self {
        let (secure_boot, hardware) =
            devices.into_iter().partition(|d| d.kind() == Kind::SecureBootStore);
        Self { hardware, secure_boot, pending }
    }

    /// The rows a person would want to be told about, and why.
    ///
    /// **An unreadable pending list is the row that must not be silent** — a
    /// machine nobody could ask about firmware looks exactly like a machine
    /// with no firmware updates.
    ///
    /// A blocker is reported **only for a device that actually has an update
    /// waiting**, and that restriction is the whole point. Measured on this
    /// machine: seven rows carry `require-ac-power` and **nothing at all has
    /// an update waiting**, so reporting blockers unconditionally would put
    /// seven complaints on a perfectly healthy laptop, for ever, and make
    /// `apex firmware status` exit non-zero on the most common state there is.
    /// That is the same permanent-false-alarm shape as warning about the
    /// composefs root's 0 bytes free, which this unit already fixed once.
    /// `require-ac-power` is worth saying when it is standing between this
    /// machine and an update it is being offered — and is worth nothing
    /// otherwise.
    pub fn attention(&self) -> Vec<String> {
        let mut out = Vec::new();
        let pending = match &self.pending {
            Reading::Unavailable(why) => {
                out.push(format!(
                    "whether this machine has firmware updates is unknown: {why}"
                ));
                return out;
            }
            Reading::Known(list) => list,
        };
        for d in pending {
            let v = match d.version_text() {
                Reading::Known(v) => format!(" (at {v})"),
                Reading::Unavailable(_) => String::new(),
            };
            out.push(format!(
                "{}{} has a firmware update waiting{}",
                d.label(),
                v,
                if d.needs_reboot() { ", and a restart applies it" } else { "" }
            ));
            // The blockers come from the device list rather than from the
            // pending row, because `get-updates` reports the release and
            // `get-devices` reports the state of the thing being written to.
            // Matched on `DeviceId`: the name is not an identity here.
            for known in self.hardware.iter().chain(self.secure_boot.iter()) {
                if known.device_id == d.device_id {
                    for b in known.blockers() {
                        out.push(format!("{}: {}", known.label(), b));
                    }
                }
            }
            // And a blocker fwupd attached to the pending row itself.
            for b in d.blockers() {
                out.push(format!("{}: {}", d.label(), b));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The exact shapes measured on the development machine: a row with no
    /// `Name` key and no `Version` key at all, a hex version, two rows sharing
    /// a name, a certificate store that is flagged `updatable`, and a
    /// `require-ac-power` problem.
    const REAL: &str = r#"{
      "Devices": [
        {"DeviceId": "aec1a869eb0df71b7cea6b3ac71d39b830faf164",
         "Plugin": "linux_display", "Flags": ["can-emulation-tag"]},
        {"DeviceId": "f685512aa07369c9e77742acef941d779d31e766",
         "Name": "GPIO controller", "Plugin": "gpio", "Flags": ["can-emulation-tag"]},
        {"DeviceId": "a363c73cb1f37be672fa2fdd1b31bfc5c84f24d8",
         "Name": "06DA:00 04F3:320B", "Version": "0x0006", "VersionFormat": "hex",
         "Vendor": "Elan", "Plugin": "elantp",
         "Flags": ["internal", "updatable", "unsigned-payload"]},
        {"DeviceId": "1111111111111111111111111111111111111111",
         "Name": "UEFI Device Firmware", "Version": "4116", "VersionFormat": "number",
         "Plugin": "uefi_capsule", "Flags": ["internal", "needs-reboot"],
         "Problems": ["require-ac-power"]},
        {"DeviceId": "2222222222222222222222222222222222222222",
         "Name": "UEFI Device Firmware", "Version": "6", "VersionFormat": "number",
         "Plugin": "uefi_capsule", "Flags": ["internal", "needs-reboot"]},
        {"DeviceId": "3333333333333333333333333333333333333333",
         "Name": "KEK CA", "Version": "2023", "VersionFormat": "number",
         "Plugin": "uefi_kek",
         "Flags": ["internal", "updatable", "needs-reboot", "signed-payload"]},
        {"DeviceId": "4444444444444444444444444444444444444444",
         "Name": "KEK CA", "Version": "2012", "VersionFormat": "number",
         "Plugin": "uefi_kek",
         "Flags": ["internal", "updatable", "needs-reboot", "signed-payload"]}
      ]
    }"#;

    fn real() -> Vec<Device> {
        parse_devices(REAL).expect("the machine's own shapes must parse")
    }

    #[test]
    fn an_absent_name_and_an_absent_version_are_not_a_parse_failure() {
        // One row of this machine's 28 has no Name key and no Version key.
        // A reader that indexes either unconditionally panics on the output
        // of the machine it was written on.
        let d = &real()[0];
        assert!(d.name.is_none(), "{:?}", d.name);
        assert!(d.version.is_none(), "{:?}", d.version);
    }

    #[test]
    fn a_row_with_no_name_still_has_something_to_call_it() {
        let d = &real()[0];
        let label = d.label();
        assert!(!label.trim().is_empty());
        // The plugin is what fwupd identified it by, so that is what shows.
        assert!(label.contains("linux_display"), "{label}");
    }

    #[test]
    fn a_missing_version_is_a_reason_and_never_a_blank() {
        // Printing an empty column reads as "version: nothing", which is a
        // claim. The reason is a fact.
        let d = &real()[1];
        let why = d.version_text();
        assert!(!why.is_known());
        assert!(why.reason().unwrap().contains("no version"), "{why:?}");
    }

    #[test]
    fn a_hex_version_survives_as_written() {
        // The touchpad really does report 0x0006 and says so in
        // VersionFormat. Normalising it would invent a version.
        let d = &real()[2];
        assert_eq!(d.version.as_deref(), Some("0x0006"));
        assert_eq!(d.version_format.as_deref(), Some("hex"));
    }

    #[test]
    fn two_rows_with_the_same_name_are_told_apart_by_id_and_not_by_name() {
        // Five rows on this machine are all called UEFI Device Firmware and
        // KEK CA names two. A readout keyed on the name loses rows.
        let ds = real();
        let same: Vec<&Device> =
            ds.iter().filter(|d| d.name.as_deref() == Some("UEFI Device Firmware")).collect();
        assert_eq!(same.len(), 2);
        assert_ne!(same[0].short_id(), same[1].short_id());
    }

    #[test]
    fn a_device_fwupd_will_not_write_right_now_is_blocked_and_not_read_only() {
        // Measured: `updatable` and `Problems` never co-occur — fwupd removes
        // the flag while a blocker stands. Reading its absence as "this
        // device cannot be updated" reports the System Firmware and the NVMe
        // as unwritable for as long as the laptop is on battery.
        let d = &real()[3];
        assert!(!d.is_updatable(), "the fixture must reproduce the flag's absence");
        match d.writable() {
            Writable::Blocked(why) => {
                assert!(why.iter().any(|w| w.contains("plug the machine in")), "{why:?}")
            }
            other => panic!("a blocked device read as {other:?}"),
        }
    }

    #[test]
    fn the_flag_and_the_problem_are_mutually_exclusive_in_this_documents_own_rows() {
        // The fact the three states rest on, asserted so that a fixture which
        // stops reproducing it fails loudly rather than quietly making
        // `Blocked` unreachable.
        for d in real() {
            assert!(
                !(d.is_updatable() && !d.problems.is_empty()),
                "{} is both updatable and problematic: {:?}",
                d.label(),
                d.problems
            );
        }
    }

    #[test]
    fn a_device_with_no_flag_and_no_problem_is_one_fwupd_never_offers_to_write() {
        let d = &real()[1]; // GPIO controller: no updatable, no Problems.
        assert_eq!(d.writable(), Writable::Never);
    }

    #[test]
    fn the_updatable_flag_does_not_separate_hardware_from_secure_boot_stores() {
        // The measurement that decides this module's filter: of eleven
        // updatable rows on the real machine, nine are certificate stores.
        // Here, of the four updatable rows, three are.
        let ds = real();
        let updatable: Vec<&Device> = ds.iter().filter(|d| d.is_updatable()).collect();
        let stores = updatable.iter().filter(|d| d.kind() == Kind::SecureBootStore).count();
        assert!(
            stores > updatable.len() / 2,
            "the premise of this test is that most updatable rows are stores: \
             {stores} of {}",
            updatable.len()
        );
    }

    #[test]
    fn the_plugin_separates_them_and_uefi_capsule_is_hardware() {
        let r = Report::new(real(), Reading::Known(Vec::new()));
        assert!(
            r.secure_boot.iter().all(|d| d.plugin.as_deref() == Some("uefi_kek")),
            "{:?}",
            r.secure_boot.iter().map(Device::label).collect::<Vec<_>>()
        );
        // uefi_capsule is real system firmware and must not be filtered away
        // with the certificates.
        assert!(
            r.hardware.iter().any(|d| d.plugin.as_deref() == Some("uefi_capsule")),
            "uefi_capsule was treated as a Secure Boot store"
        );
    }

    #[test]
    fn a_plugin_nobody_here_has_heard_of_is_hardware() {
        // Fail visible, not silent: a new fwupd plugin should appear in the
        // readout and be wrong there rather than vanish from it.
        let d = parse_devices(
            r#"{"Devices": [{"DeviceId": "aa", "Name": "Whatever", "Plugin": "brand_new"}]}"#,
        )
        .unwrap();
        assert_eq!(d[0].kind(), Kind::Hardware);
    }

    #[test]
    fn an_error_document_is_an_error_although_fwupd_exited_zero() {
        // Measured: `get-releases --json <bogus id>` exits 0 and returns this.
        // Reading it as a device list is how a failed request becomes a
        // machine with no firmware.
        let why = parse_devices(
            r#"{"Error": {"Domain": "FwupdError", "Code": 8,
                          "Message": "failed to find deadbeef"}}"#,
        )
        .expect_err("an Error document is not a device list");
        assert!(why.contains("failed to find deadbeef"), "{why}");
        assert!(why.contains("code 8"), "{why}");
    }

    #[test]
    fn a_document_with_no_devices_key_is_unreadable_and_not_an_empty_machine() {
        // The inversion this whole unit keeps re-learning. An empty list here
        // would say "nothing to update", which is what a healthy machine says.
        let why = parse_devices("{}").expect_err("no Devices key is not no devices");
        assert!(why.contains("neither an Error nor a Devices list"), "{why}");
    }

    #[test]
    fn an_empty_devices_list_really_is_an_empty_list() {
        // And the other direction: fwupd's actual "nothing to update" answer
        // must parse cleanly, or every current machine reports a fault.
        assert!(parse_devices(r#"{"Devices": []}"#).unwrap().is_empty());
    }

    #[test]
    fn output_that_is_not_json_at_all_is_reported_as_such() {
        let why = parse_devices("Idle…: 0%\nfailed to find x\n").expect_err("not JSON");
        assert!(why.contains("did not return JSON"), "{why}");
    }

    #[test]
    fn an_unreadable_pending_list_is_the_one_row_that_must_not_be_silent() {
        // A machine nobody could ask looks exactly like a machine with no
        // updates, and that is the whole failure mode.
        let r = Report::new(real(), Reading::Unavailable("fwupd is not running".into()));
        let said = r.attention().join("\n");
        assert!(said.contains("is unknown"), "{said}");
        assert!(said.contains("fwupd is not running"), "{said}");
    }

    #[test]
    fn a_healthy_machine_with_blockers_and_no_updates_says_nothing_at_all() {
        // THE permanent-false-alarm test. Measured: this machine has seven
        // `require-ac-power` rows and zero pending updates, so reporting
        // blockers unconditionally puts seven complaints on a healthy laptop
        // for ever and makes the command exit non-zero on the commonest state
        // there is. Same shape as warning about the composefs root's 0 bytes
        // free, which this unit already fixed once.
        let r = Report::new(real(), Reading::Known(Vec::new()));
        assert!(
            r.attention().is_empty(),
            "a healthy machine complained: {:?}",
            r.attention()
        );
    }

    #[test]
    fn a_blocker_is_reported_when_it_stands_between_this_machine_and_an_update() {
        // And the other direction: with an update actually waiting for the
        // device that needs AC power, saying so is the whole value — it is the
        // difference between an update that runs tonight and one that never
        // does.
        let pending = parse_devices(
            r#"{"Devices": [{"DeviceId": "1111111111111111111111111111111111111111",
                             "Name": "UEFI Device Firmware", "Version": "4116",
                             "Plugin": "uefi_capsule",
                             "Flags": ["internal", "needs-reboot"]}]}"#,
        )
        .unwrap();
        let r = Report::new(real(), Reading::Known(pending));
        let said = r.attention().join("\n");
        assert!(said.contains("has a firmware update waiting"), "{said}");
        assert!(said.contains("plug the machine in"), "{said}");
    }

    #[test]
    fn a_blocker_is_matched_by_device_id_and_not_by_name() {
        // Two rows are called `UEFI Device Firmware` and only one of them has
        // the problem. Matching on the name would attach a complaint to the
        // wrong device, or to both.
        let pending = parse_devices(
            r#"{"Devices": [{"DeviceId": "2222222222222222222222222222222222222222",
                             "Name": "UEFI Device Firmware", "Version": "6",
                             "Plugin": "uefi_capsule", "Flags": ["internal"]}]}"#,
        )
        .unwrap();
        let r = Report::new(real(), Reading::Known(pending));
        let said = r.attention().join("\n");
        assert!(said.contains("has a firmware update waiting"), "{said}");
        assert!(
            !said.contains("plug the machine in"),
            "the other row's problem was attached to this one: {said}"
        );
    }

    #[test]
    fn a_pending_update_names_the_device_and_whether_a_restart_applies_it() {
        let pending = parse_devices(
            r#"{"Devices": [{"DeviceId": "99", "Name": "System Firmware",
                             "Version": "0.1.17", "Plugin": "uefi_capsule",
                             "Flags": ["internal", "needs-reboot"]}]}"#,
        )
        .unwrap();
        let r = Report::new(real(), Reading::Known(pending));
        let said = r.attention().join("\n");
        assert!(said.contains("System Firmware"), "{said}");
        assert!(said.contains("0.1.17"), "{said}");
        assert!(said.contains("restart"), "{said}");
    }

    #[test]
    fn an_unknown_problem_string_is_still_reported() {
        // fwupd adds these; an unrecognised one must not become silence.
        let s = problem_sentence("some-new-problem");
        assert!(s.contains("some-new-problem"), "{s}");
    }
}
