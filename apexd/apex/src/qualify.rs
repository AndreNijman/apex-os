//! `apex qualify` — what is this class of machine known to do, and who said so.
//! Roadmap §33.
//!
//! The store, the consent rule and the three-valued verdict live in
//! [`apexd_core::qualify`]. This file is the half that touches the machine: it
//! resolves the path, reads sysfs, and refuses to turn a read it could not
//! perform into a result.
//!
//! ## `probe` records two checks and refuses to guess the rest
//!
//! It would be easy to write a probe that ticked every row. Almost none of
//! §33's checks can be established without somebody using the machine: "audio"
//! means sound came out, "sleep" means the lid closed and the session came
//! back, "external monitor" needs a monitor. A probe that answered those from
//! `/sys` would be recording the presence of a device driver as a working
//! feature, which is the difference P0-001 was careful about when it marked
//! its own GPU row "PASS at driver level".
//!
//! So `probe` fills in the two rows that genuinely are file reads —
//! `gpu-driver` and `secure-boot` — and writes every other row as
//! [`Verdict::NotChecked`] with the sentence that says who can change it. A
//! machine after a probe reads as mostly unknown, which is the truth.
//!
//! ## A refused read is never a failure
//!
//! `secure-boot` is the live example. The EFI variable is world-readable on a
//! UEFI machine, absent on a BIOS one, and unreadable inside a container with
//! no `efivarfs`. Those are three different answers and only one of them is
//! about the firmware. A `NotChecked` carrying the errno is what the other two
//! get; recording `fail` for "I could not look" would tell a user their
//! machine boots unverified when nobody has any idea.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use apexd_core::migrate;
use apexd_core::qualify::{
    self, Consent, Db, Machine, Refused, Verdict, CHECKS,
};
use clap::Subcommand;
use serde_json::{json, Value};

/// Where the report reads the machine from.
///
/// A prefix and only a prefix, the same contract as `apex trust`'s `Roots`:
/// `tests/test-apex-qualify.sh` points `$APEX_QUALIFY_ROOT` at a fixture tree
/// so the assertions run against the shipped binary rather than against
/// whatever hardware the runner happens to have.
pub struct Roots {
    fixture: Option<PathBuf>,
}

impl Roots {
    pub fn from_env() -> Self {
        Self { fixture: std::env::var_os("APEX_QUALIFY_ROOT").map(PathBuf::from) }
    }

    fn path(&self, absolute: &str) -> PathBuf {
        match &self.fixture {
            // `absolute` always starts with '/'; without the strip, `join`
            // discards the prefix and reads the real machine, which is a
            // fixture that passes on the author's box and nowhere else.
            Some(root) => root.join(absolute.trim_start_matches('/')),
            None => PathBuf::from(absolute),
        }
    }

    /// A file's text, or the reason it could not be read.
    ///
    /// No `.ok()` twin. Every read here feeds a claim about whether a piece of
    /// hardware works, and "the read did not happen" is not one of the
    /// available claims.
    fn read(&self, absolute: &str) -> Result<String, String> {
        let p = self.path(absolute);
        std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))
    }
}

/// The database file. Machine-written, so `apex schema migrate` owns its
/// forward migration and this module only ever reads a document at the version
/// this build understands.
pub fn db_path() -> PathBuf {
    apex_agent_core::paths::state_home().join("apex/qualification.json")
}

#[derive(Subcommand)]
pub enum QualifyCmd {
    /// What this machine is known to do, check by check.
    ///
    /// Read-only, needs no root and no network. A row nobody has recorded
    /// reads as unknown rather than as a pass.
    Status {
        /// Emit machine-readable JSON instead of a report.
        #[arg(long)]
        json: bool,
    },
    /// Whether results may be kept on this disk at all.
    ///
    /// Nothing is recorded until this is granted, and withdrawing it deletes
    /// what was kept. Nothing is ever transmitted either way.
    Consent {
        #[command(subcommand)]
        cmd: ConsentCmd,
    },
    /// Record one result by hand.
    ///
    /// This is how the checks a person has to try — sleep, audio, an external
    /// monitor — get into the database. Exactly one of --pass, --fail or
    /// --not-checked.
    Record {
        /// Which check. `apex qualify status` lists them.
        check: String,
        #[arg(long, conflicts_with_all = ["fail", "not_checked"])]
        pass: bool,
        /// It did not work, and what happened.
        #[arg(long, value_name = "WHAT HAPPENED", conflicts_with = "not_checked")]
        fail: Option<String>,
        /// Nobody has established it, and why not.
        #[arg(long = "not-checked", value_name = "WHY NOT")]
        not_checked: Option<String>,
    },
    /// Record the two checks that are file reads, and say why the rest are not.
    ///
    /// Fills in the GPU driver and Secure Boot rows from `/sys`, and writes
    /// every remaining row as not-checked with the reason. It never marks a
    /// row passed on the strength of a device existing.
    Probe {
        /// Emit machine-readable JSON instead of a report.
        #[arg(long)]
        json: bool,
    },
    /// Write this machine's results to a file, to hand to somebody else.
    ///
    /// The file goes where you say. Nothing here uploads anything.
    Export {
        /// Where to write. `-` for standard output.
        #[arg(default_value = "-")]
        out: String,
    },
    /// Fold an exported file into this machine's database.
    ///
    /// Needs consent, takes only checks this build knows, and never overwrites
    /// a newer result with an older one.
    Import {
        /// The file to read.
        file: PathBuf,
    },
}

#[derive(Subcommand)]
pub enum ConsentCmd {
    /// Say whether results are being kept, and what that means.
    Show,
    /// Allow results to be kept on this disk.
    Grant,
    /// Refuse, and delete anything already kept.
    Decline,
}

// ── the store ────────────────────────────────────────────────────────────────

/// Read the database, or say why it could not be read.
///
/// Three outcomes and they stay apart: no file is an empty database, a file
/// from a newer build is refused with the message §25 owes the user, and a
/// file that could not be read is an error rather than an empty database —
/// defaulting there would write a fresh file over one that was merely
/// unreadable, which is what `apex env create` used to do.
fn load(path: &Path) -> Result<Db, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Db::new()),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    let store = migrate::store("qualification")
        .ok_or_else(|| "the qualification store is not registered".to_string())?;
    let found = migrate::peek(store, &text)?;
    match migrate::plan(store, found) {
        migrate::Plan::TooNew { found, current } => {
            return Err(migrate::too_new_message(store, path, found, current))
        }
        migrate::Plan::NoRoute { found, current } => {
            return Err(format!("no migration reaches schema {current} from schema {found}"))
        }
        // Forward steps are applied in memory here so a read is never blocked
        // on somebody having run `apex schema migrate` first. The file itself
        // is left alone; that command owns the on-disk rewrite and the
        // checkpoint that goes with it.
        migrate::Plan::Forward(_) => {
            let out = migrate::migrate_text(store, &text)?;
            let migrated = out.map(|o| o.text).unwrap_or(text);
            return Db::parse(&migrated);
        }
        migrate::Plan::UpToDate => {}
    }
    Db::parse(&text)
}

fn save(path: &Path, db: &Db) -> Result<(), String> {
    let mut db = db.clone();
    db.schema = qualify::SCHEMA_VERSION;
    let text = db.to_text()?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    // Atomic replacement, the repo contract for persistent state: a file
    // truncated by a crash mid-write is a file somebody has to reconstruct.
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &text).map_err(|e| format!("writing {}: {e}", tmp.display()))?;
    // 0600 before the rename, not after. Between a 0644 create and a later
    // chmod there is a window in which another user on the machine can read
    // it, and this file describes the owner's hardware.
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
    }
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("installing {}: {e}", path.display())
    })
}

// ── this machine ─────────────────────────────────────────────────────────────

/// Build the machine identity from the live machine, or a fixture root.
///
/// The DMI strings and the GPU ids come from [`apexd_core::fingerprint`],
/// which already reads them and already has a `detect_from` that takes roots.
/// The kernel and the BIOS version are the two it does not carry.
pub fn this_machine(roots: &Roots) -> Machine {
    let proc_root = roots.path("/proc");
    let sys_root = roots.path("/sys");
    let fp = apexd_core::fingerprint::Fingerprint::detect_from(&proc_root, &sys_root);
    let gpus: Vec<String> = fp.gpus.iter().map(|g| g.pci_id()).collect();
    // Both of these are informational fields on the record rather than part of
    // the key, so an unreadable one is a blank rather than an error.
    let kernel = roots
        .read("/proc/sys/kernel/osrelease")
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    let firmware = roots
        .read("/sys/class/dmi/id/bios_version")
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    Machine::from_dmi(
        &fp.sys_vendor,
        &fp.product_family,
        &fp.product_name,
        &firmware,
        fp.chassis_type,
        &fp.cpu.model_name,
        &gpus,
        &kernel,
    )
}

/// The APEX build a result was seen on.
///
/// The booted deployment's ostree checksum, found the way the kernel found the
/// deployment: the `ostree=` argument on the command line, whose path carries
/// it. `/etc/os-release` was the obvious place to look and it is the wrong
/// one — measured on the L16, it holds `ID=fedora` and `VERSION_ID=43` and no
/// image field at all, so every result would have carried the same stamp on
/// every APEX build ever made.
///
/// A blank rather than a guess when the command line cannot be read. A wrong
/// build stamp is worse than none, because it makes a stale pass look fresh.
fn build_id(roots: &Roots) -> String {
    let Ok(cmdline) = roots.read("/proc/cmdline") else {
        return String::new();
    };
    cmdline
        .split_ascii_whitespace()
        .find_map(|a| a.strip_prefix("ostree="))
        .and_then(|p| {
            p.split('/')
                .find(|seg| seg.len() == 64 && seg.chars().all(|c| c.is_ascii_hexdigit()))
        })
        .map(|c| c[..12].to_string())
        .unwrap_or_default()
}

// ── the two checks that are file reads ───────────────────────────────────────

/// Is every display device bound to a kernel driver?
///
/// `/sys/bus/pci/devices/<slot>/driver` is a symlink to the bound driver. Its
/// absence on a device that exists means nothing claimed it, which is a real
/// failure and the thing this check is for. A directory that cannot be listed
/// at all is not that.
fn probe_gpu_driver(roots: &Roots) -> Verdict {
    let sys_root = roots.path("/sys");
    let fp = apexd_core::fingerprint::Fingerprint::detect_from(&roots.path("/proc"), &sys_root);
    if fp.gpus.is_empty() {
        return Verdict::NotChecked {
            reason: "no display device was found on the PCI bus, so there is nothing to bind"
                .into(),
        };
    }
    let mut unbound: Vec<String> = Vec::new();
    for g in &fp.gpus {
        let link = sys_root.join("bus/pci/devices").join(&g.pci_slot).join("driver");
        match std::fs::read_link(&link) {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                unbound.push(format!("{} ({})", g.pci_slot, g.pci_id()));
            }
            // A refused readlink says nothing about whether a driver is bound.
            Err(e) => {
                return Verdict::NotChecked {
                    reason: format!("{}: {e}", link.display()),
                }
            }
        }
    }
    if unbound.is_empty() {
        Verdict::Pass
    } else {
        Verdict::Fail {
            detail: format!("no kernel driver is bound to {}", unbound.join(", ")),
        }
    }
}

/// The EFI `SecureBoot` variable: is the firmware enforcing?
///
/// The efivars encoding is four bytes of attributes followed by the value, so
/// the fifth byte is the one that matters. A four-byte file is a variable with
/// no data, which is not the same as a zero.
const SECUREBOOT_VAR: &str =
    "/sys/firmware/efi/efivars/SecureBoot-8be4df61-93ca-11d2-aa0d-00e098032b8c";

fn probe_secure_boot(roots: &Roots) -> Verdict {
    let p = roots.path(SECUREBOOT_VAR);
    let bytes = match std::fs::read(&p) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // No variable. Either the machine booted through BIOS, or nothing
            // mounted efivarfs. Neither is a firmware that fails to enforce.
            return Verdict::NotChecked {
                reason: format!(
                    "{} is not there; this machine did not boot through UEFI, or efivarfs is not mounted",
                    p.display()
                ),
            };
        }
        Err(e) => return Verdict::NotChecked { reason: format!("{}: {e}", p.display()) },
    };
    match bytes.get(4) {
        Some(1) => Verdict::Pass,
        Some(0) => Verdict::Fail {
            detail: "the firmware has Secure Boot switched off".into(),
        },
        Some(other) => Verdict::NotChecked {
            reason: format!("the SecureBoot variable holds {other}, which is neither on nor off"),
        },
        None => Verdict::NotChecked {
            reason: format!(
                "{} is {} bytes, too short to hold a value",
                p.display(),
                bytes.len()
            ),
        },
    }
}

/// Why a check nobody can measure from a file is not measured here.
///
/// One sentence per check, and it is the actionable half of the readout: a
/// user who reads "close the lid" knows what to do, where "unknown" tells them
/// nothing.
fn why_not_probed(id: &str) -> &'static str {
    match id {
        "sleep" => "suspend the machine and wake it, then run: apex qualify record sleep --pass",
        "audio" => "play something through the built-in output, then: apex qualify record audio --pass",
        "wifi" => "connect to a network and load a page, then: apex qualify record wifi --pass",
        "bluetooth" => "pair and connect a device, then: apex qualify record bluetooth --pass",
        "external-monitor" => {
            "plug a display into the machine's own video output, then: apex qualify record external-monitor --pass"
        }
        "vrr" => "run something that changes frame rate on a display that advertises VRR",
        "portals" => "take a screenshot and share a screen, then: apex qualify record portals --pass",
        "upgrade" => "run apex update and reboot into the newer image",
        "rollback" => "run apex rollback and reboot; qualify it from the deployment you land in",
        "fresh-install" => "install onto an empty disk and boot the result",
        _ => "nobody has established this yet",
    }
}

// ── rendering ────────────────────────────────────────────────────────────────

fn row_json(r: &qualify::Row) -> Value {
    let mut m = serde_json::Map::new();
    m.insert("id".into(), json!(r.id));
    m.insert("label".into(), json!(r.label));
    m.insert("means".into(), json!(r.means));
    match &r.observed {
        None => {
            m.insert("verdict".into(), Value::Null);
            m.insert("note".into(), json!("no result recorded"));
        }
        Some(o) => {
            m.insert("verdict".into(), json!(o.verdict.word()));
            m.insert("at".into(), json!(o.at));
            m.insert("build".into(), json!(o.build));
            if let Some(d) = o.verdict.detail() {
                m.insert("detail".into(), json!(d));
            }
        }
    }
    Value::Object(m)
}

fn status(as_json: bool) -> i32 {
    let roots = Roots::from_env();
    let path = db_path();
    let db = match load(&path) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("apex: {e}");
            return 1;
        }
    };
    let machine = this_machine(&roots);
    let key = machine.key();
    let rows = qualify::confidence(&db, &key);
    let (pass, fail, unknown) = qualify::tally(&rows);
    let known = db.machines.contains_key(&key);

    if as_json {
        let doc = json!({
            "consent": db.consent.as_str(),
            "consentMeans": db.consent.note(),
            "path": path.display().to_string(),
            "machine": {
                "key": key,
                "title": machine.title(),
                "vendor": machine.vendor,
                "family": machine.family,
                "product": machine.product,
                "chassis": machine.chassis,
                "cpu": machine.cpu,
                "gpus": machine.gpus,
                "kernel": machine.kernel,
                "firmware": machine.firmware,
            },
            "recorded": known,
            "checks": rows.iter().map(row_json).collect::<Vec<_>>(),
            "passed": pass,
            "failed": fail,
            "unknown": unknown,
            "otherMachines": db
                .machines
                .iter()
                .filter(|(k, _)| **k != key)
                .map(|(k, r)| json!({"key": k, "title": r.machine.title()}))
                .collect::<Vec<_>>(),
        });
        println!("{}", serde_json::to_string_pretty(&doc).unwrap_or_default());
        return 0;
    }

    println!("{}", machine.title());
    println!("  {}  {}", key, machine.cpu);
    println!("  kernel {}   firmware {}", machine.kernel, machine.firmware);
    println!("\nconsent: {} — {}\n", db.consent.as_str(), db.consent.note());
    for r in &rows {
        println!("  [{}] {:<18} {}", r.mark(), r.label, r.note());
    }
    println!("\n{pass} passed, {fail} failed, {unknown} not known");
    if db.consent != Consent::Granted {
        println!("Nothing is recorded until you run: apex qualify consent grant");
    } else if !known {
        println!("Nothing recorded for this machine yet. Start with: apex qualify probe");
    }
    let others: Vec<&String> = db.machines.keys().filter(|k| **k != key).collect();
    if !others.is_empty() {
        println!("\n{} other machine(s) in this database", others.len());
        for k in others {
            println!("  {k}  {}", db.machines[k].machine.title());
        }
    }
    0
}

fn consent(cmd: ConsentCmd) -> i32 {
    let path = db_path();
    let mut db = match load(&path) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("apex: {e}");
            return 1;
        }
    };
    let want = match cmd {
        ConsentCmd::Show => {
            println!("{} — {}", db.consent.as_str(), db.consent.note());
            println!("{}", path.display());
            return 0;
        }
        ConsentCmd::Grant => Consent::Granted,
        ConsentCmd::Decline => Consent::Declined,
    };
    let had = db.machines.len();
    qualify::set_consent(&mut db, want);
    if let Err(e) = save(&path, &db) {
        eprintln!("apex: {e}");
        return 1;
    }
    println!("consent: {} — {}", want.as_str(), want.note());
    if want != Consent::Granted && had > 0 {
        println!("{had} machine record(s) deleted.");
    }
    0
}

fn record_one(check: String, pass: bool, fail: Option<String>, not_checked: Option<String>) -> i32 {
    let verdict = match (pass, fail, not_checked) {
        (true, None, None) => Verdict::Pass,
        (false, Some(d), None) => Verdict::Fail { detail: d },
        (false, None, Some(r)) => Verdict::NotChecked { reason: r },
        _ => {
            eprintln!("apex: give exactly one of --pass, --fail <what happened> or --not-checked <why not>");
            return 2;
        }
    };
    let roots = Roots::from_env();
    let path = db_path();
    let mut db = match load(&path) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("apex: {e}");
            return 1;
        }
    };
    let machine = this_machine(&roots);
    let at = now();
    let build = build_id(&roots);
    match qualify::record(&mut db, &machine, &check, verdict, at, &build) {
        Ok(_) => {}
        Err(e @ Refused::NoConsent { .. }) => {
            eprintln!("apex: {e}");
            eprintln!("apex: allow it with: apex qualify consent grant");
            return 1;
        }
        Err(e @ Refused::UnknownCheck { .. }) => {
            eprintln!("apex: {e}");
            eprintln!("apex: the checks are: {}", check_ids().join(", "));
            return 2;
        }
    }
    if let Err(e) = save(&path, &db) {
        eprintln!("apex: {e}");
        return 1;
    }
    println!("recorded {check} for {}", machine.title());
    0
}

fn check_ids() -> Vec<&'static str> {
    CHECKS.iter().map(|c| c.id).collect()
}

fn probe(as_json: bool) -> i32 {
    let roots = Roots::from_env();
    let path = db_path();
    let mut db = match load(&path) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("apex: {e}");
            return 1;
        }
    };
    if db.consent != Consent::Granted {
        eprintln!(
            "apex: not probing: consent is {}. Nothing about this machine is kept until it is granted.",
            db.consent.as_str()
        );
        eprintln!("apex: allow it with: apex qualify consent grant");
        return 1;
    }
    let machine = this_machine(&roots);
    let at = now();
    let build = build_id(&roots);

    let mut measured: BTreeMap<&str, Verdict> = BTreeMap::new();
    measured.insert("gpu-driver", probe_gpu_driver(&roots));
    measured.insert("secure-boot", probe_secure_boot(&roots));

    let mut wrote: Vec<Value> = Vec::new();
    for c in CHECKS {
        // A row already recorded by a person is never overwritten by a probe's
        // not-checked. Somebody suspended the machine and said so; a later
        // probe saying "nobody has established this" would erase their work.
        let existing = db
            .machines
            .get(&machine.key())
            .and_then(|r| r.results.get(c.id))
            .cloned();
        let verdict = match measured.remove(c.id) {
            Some(v) => v,
            None => match existing {
                Some(_) => continue,
                None => Verdict::NotChecked { reason: why_not_probed(c.id).to_string() },
            },
        };
        let word = verdict.word();
        let detail = verdict.detail().map(str::to_string);
        if let Err(e) = qualify::record(&mut db, &machine, c.id, verdict, at, &build) {
            eprintln!("apex: {e}");
            return 1;
        }
        wrote.push(json!({"id": c.id, "verdict": word, "detail": detail}));
    }
    if let Err(e) = save(&path, &db) {
        eprintln!("apex: {e}");
        return 1;
    }
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "machine": machine.key(),
                "title": machine.title(),
                "recorded": wrote,
            }))
            .unwrap_or_default()
        );
        return 0;
    }
    println!("{}", machine.title());
    for w in &wrote {
        let id = w["id"].as_str().unwrap_or("");
        let v = w["verdict"].as_str().unwrap_or("");
        match w["detail"].as_str() {
            Some(d) => println!("  {id:<18} {v} — {d}"),
            None => println!("  {id:<18} {v}"),
        }
    }
    0
}

fn export(out: String) -> i32 {
    let path = db_path();
    let db = match load(&path) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("apex: {e}");
            return 1;
        }
    };
    let text = match db.to_text() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("apex: {e}");
            return 1;
        }
    };
    if out == "-" {
        print!("{text}");
        return 0;
    }
    match std::fs::write(&out, &text) {
        Ok(()) => {
            println!("wrote {out}");
            0
        }
        Err(e) => {
            eprintln!("apex: writing {out}: {e}");
            1
        }
    }
}

fn import(file: PathBuf) -> i32 {
    let path = db_path();
    let mut db = match load(&path) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("apex: {e}");
            return 1;
        }
    };
    let text = match std::fs::read_to_string(&file) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("apex: {}: {e}", file.display());
            return 1;
        }
    };
    let other = match Db::parse(&text) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("apex: {}: {e}", file.display());
            return 1;
        }
    };
    match qualify::merge(&mut db, &other) {
        Ok(n) => {
            if let Err(e) = save(&path, &db) {
                eprintln!("apex: {e}");
                return 1;
            }
            println!("took {n} result(s) from {}", file.display());
            0
        }
        Err(e) => {
            eprintln!("apex: {e}");
            eprintln!("apex: allow it with: apex qualify consent grant");
            1
        }
    }
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub fn main(cmd: QualifyCmd) -> i32 {
    match cmd {
        QualifyCmd::Status { json } => status(json),
        QualifyCmd::Consent { cmd } => consent(cmd),
        QualifyCmd::Record { check, pass, fail, not_checked } => {
            record_one(check, pass, fail, not_checked)
        }
        QualifyCmd::Probe { json } => probe(json),
        QualifyCmd::Export { out } => export(out),
        QualifyCmd::Import { file } => import(file),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> tempdir::Dir {
        tempdir::Dir::new()
    }

    #[test]
    fn a_missing_secureboot_variable_is_not_a_firmware_that_fails() {
        let d = tree();
        let roots = Roots { fixture: Some(d.path().to_path_buf()) };
        let v = probe_secure_boot(&roots);
        match v {
            Verdict::NotChecked { reason } => {
                assert!(reason.contains("UEFI"), "{reason}");
            }
            other => panic!("a missing variable reported as {}", other.word()),
        }
    }

    #[test]
    fn secure_boot_reads_the_fifth_byte_not_the_first() {
        // The efivars encoding is four attribute bytes then the value. Reading
        // byte 0 would answer from the attributes, which on a real machine are
        // 0x06 — neither 0 nor 1, so the bug would surface as "neither on nor
        // off" rather than as a wrong answer, and only on hardware.
        let d = tree();
        let p = d.path().join("sys/firmware/efi/efivars");
        std::fs::create_dir_all(&p).expect("mkdir");
        let f = p.join("SecureBoot-8be4df61-93ca-11d2-aa0d-00e098032b8c");
        let roots = Roots { fixture: Some(d.path().to_path_buf()) };

        std::fs::write(&f, [0x06, 0x00, 0x00, 0x00, 0x01]).expect("write");
        assert_eq!(probe_secure_boot(&roots), Verdict::Pass);

        std::fs::write(&f, [0x06, 0x00, 0x00, 0x00, 0x00]).expect("write");
        assert!(matches!(probe_secure_boot(&roots), Verdict::Fail { .. }));

        // Four bytes: a variable with attributes and no value.
        std::fs::write(&f, [0x06, 0x00, 0x00, 0x00]).expect("write");
        match probe_secure_boot(&roots) {
            Verdict::NotChecked { reason } => assert!(reason.contains("too short"), "{reason}"),
            other => panic!("an empty variable reported as {}", other.word()),
        }
    }

    #[test]
    fn a_refused_secureboot_read_is_not_checked_rather_than_failed() {
        use std::os::unix::fs::PermissionsExt;
        let d = tree();
        let p = d.path().join("sys/firmware/efi/efivars");
        std::fs::create_dir_all(&p).expect("mkdir");
        let f = p.join("SecureBoot-8be4df61-93ca-11d2-aa0d-00e098032b8c");
        std::fs::write(&f, [0x06, 0x00, 0x00, 0x00, 0x01]).expect("write");
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o000)).expect("chmod");
        // Root walks through 0000. Verify the seal actually took before
        // asserting on it, or this test silently becomes a no-op for whoever
        // runs the suite as root.
        if std::fs::read(&f).is_ok() {
            eprintln!("skip: this user reads a 0000 file (root or CAP_DAC_OVERRIDE)");
            return;
        }
        let roots = Roots { fixture: Some(d.path().to_path_buf()) };
        match probe_secure_boot(&roots) {
            Verdict::NotChecked { reason } => {
                assert!(reason.contains("Permission denied"), "{reason}")
            }
            other => panic!("a refused read reported as {}", other.word()),
        }
    }

    #[test]
    fn a_machine_with_no_display_device_is_not_a_failed_gpu() {
        let d = tree();
        std::fs::create_dir_all(d.path().join("sys/bus/pci/devices")).expect("mkdir");
        std::fs::create_dir_all(d.path().join("proc")).expect("mkdir");
        let roots = Roots { fixture: Some(d.path().to_path_buf()) };
        match probe_gpu_driver(&roots) {
            Verdict::NotChecked { reason } => assert!(reason.contains("nothing to bind"), "{reason}"),
            other => panic!("no GPU reported as {}", other.word()),
        }
    }

    #[test]
    fn every_check_has_a_sentence_saying_who_can_settle_it() {
        for c in CHECKS {
            // The two the probe measures do not need one; every other row's
            // note is the only actionable thing on the page.
            if c.id == "gpu-driver" || c.id == "secure-boot" {
                continue;
            }
            let w = why_not_probed(c.id);
            assert_ne!(
                w, "nobody has established this yet",
                "{} has no instruction, so its row tells the user nothing",
                c.id
            );
        }
    }

    #[test]
    fn a_file_from_a_newer_build_is_refused_with_the_message_it_owes() {
        let d = tree();
        let p = d.path().join("qualification.json");
        std::fs::write(&p, r#"{"schema": 99, "consent": "granted", "machines": {}}"#)
            .expect("write");
        let e = load(&p).expect_err("must refuse");
        assert!(e.contains("99"), "{e}");
        assert!(e.contains("newer"), "{e}");
    }

    #[test]
    fn an_unreadable_database_is_an_error_rather_than_an_empty_one() {
        use std::os::unix::fs::PermissionsExt;
        let d = tree();
        let p = d.path().join("qualification.json");
        std::fs::write(&p, r#"{"schema": 1, "consent": "granted", "machines": {}}"#)
            .expect("write");
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o000)).expect("chmod");
        if std::fs::read(&p).is_ok() {
            eprintln!("skip: this user reads a 0000 file (root or CAP_DAC_OVERRIDE)");
            return;
        }
        let e = load(&p).expect_err("an unreadable file became an empty database");
        assert!(e.contains("Permission denied"), "{e}");
    }

    #[test]
    fn a_missing_database_is_an_empty_one_and_not_an_error() {
        let d = tree();
        let db = load(&d.path().join("nothing-here.json")).expect("absent is fine");
        assert_eq!(db.consent, Consent::Unset);
        assert!(db.machines.is_empty());
    }

    #[test]
    fn a_saved_database_is_only_readable_by_its_owner() {
        use std::os::unix::fs::PermissionsExt;
        let d = tree();
        let p = d.path().join("sub/qualification.json");
        let db = Db::new();
        save(&p, &db).expect("save");
        let mode = std::fs::metadata(&p).expect("stat").permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "mode {mode:o}");
        assert_eq!(load(&p).expect("reload"), db);
    }

    #[test]
    fn saving_stamps_the_schema_this_build_writes() {
        let d = tree();
        let p = d.path().join("qualification.json");
        let mut db = Db::new();
        db.schema = 0;
        save(&p, &db).expect("save");
        let text = std::fs::read_to_string(&p).expect("read");
        let v: Value = serde_json::from_str(&text).expect("json");
        assert_eq!(v["schema"], json!(qualify::SCHEMA_VERSION));
    }

    /// A tiny temp directory that removes itself. The workspace has no
    /// dev-dependency on a temp-dir crate and adding one for four tests is
    /// more than this needs.
    mod tempdir {
        use std::path::{Path, PathBuf};

        pub struct Dir(PathBuf);

        impl Dir {
            pub fn new() -> Dir {
                let base = std::env::temp_dir().join(format!(
                    "apex-qualify-test-{}-{:?}",
                    std::process::id(),
                    std::thread::current().id()
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
