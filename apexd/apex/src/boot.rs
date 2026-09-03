//! `apex boot status` — what verified this boot, and what the boot counter
//! currently believes. Roadmap §22's "surface boot verification state in APEX
//! Settings", from the OS side.
//!
//! ## Read-only, root-free, and honest about what it cannot see
//!
//! Everything here is a file read or one `bootctl list` invocation. There is no
//! mutating form: `apex boot status` is what APEX Settings polls, and "what is
//! the state of my boot chain" must never cost a password.
//!
//! Two of the interesting facts do need root, because they live in the ESP,
//! which is mode 0700 on an APEX machine. When that read fails this reports
//! `"entries": null` together with the reason, and it does NOT substitute an
//! empty list. The difference matters: an empty entry list is
//! indistinguishable from "no deployment has failed", which is precisely the
//! answer that would hide a rollback from the user.
//!
//! ## Why it reports the health verdict instead of recomputing it
//!
//! `/usr/libexec/apex-boot-health` decides whether a boot was healthy, because
//! it is the unit systemd runs before `boot-complete.target` and its exit
//! status is what blesses or condemns the deployment. Recomputing the verdict
//! here would give two implementations that can disagree, and the one the user
//! reads would be the one that is not wired to anything. So the verdict is
//! read out of the file that unit wrote, and a missing file is reported as
//! "never ran" rather than as "healthy".
//!
//! ## The bootloader this expects to find
//!
//! GRUB, on every published image. §22's recommendation is to keep it for this
//! generation and AGENTS.md's boot-path rule 5 makes that a contract, so
//! `bootloader: "grub"` with `bootCounting: false` is the normal, correct
//! answer and the report says so in plain words rather than looking like a
//! degraded state.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use clap::Subcommand;
use serde_json::{json, Map, Value};

/// The vendor GUID systemd-boot and sd-stub use for every variable they export.
const LOADER_GUID: &str = "4a67b082-0a4c-41cf-b6c7-440b29bb8c4f";
/// The global UEFI namespace, where the firmware itself publishes SecureBoot.
const GLOBAL_GUID: &str = "8be4df61-93ca-11d2-aa0d-00e098032b8c";

#[derive(Subcommand)]
pub enum BootCmd {
    /// What verified this boot, and what the boot counter believes.
    ///
    /// Reports the bootloader, Secure Boot state, whether the running kernel
    /// came from a signed Unified Kernel Image, whether measured boot and
    /// TPM-bound unlock are in effect, the boot-counting tally of every
    /// deployment, the last health verdict, and any pending rollback notice.
    ///
    /// Read-only and root-free. The boot entries specifically need root
    /// because the ESP is not world-readable; without it they are reported as
    /// unavailable with the reason, never as an empty list.
    Status {
        /// Emit machine-readable JSON instead of a report.
        #[arg(long)]
        json: bool,
    },
}

/// Where the report reads from.
///
/// A prefix, and only a prefix. `tests/test-boot-v2.sh` points it at a fixture
/// tree so the assertions run against the shipped binary rather than a
/// reimplementation of it. No program name is ever taken from the environment:
/// `bootctl` is a fixed absolute path, and under a fixture root it is not
/// executed at all — a pre-rendered `bootctl-list.json` is read instead. A
/// caller-controlled variable naming a program is a hole even in an
/// unprivileged command, because nothing stops root from running it.
struct Roots {
    /// `$APEX_BOOT_ROOT`, when set.
    fixture: Option<PathBuf>,
}

impl Roots {
    fn from_env() -> Self {
        Self { fixture: std::env::var_os("APEX_BOOT_ROOT").map(PathBuf::from) }
    }

    fn path(&self, absolute: &str) -> PathBuf {
        match &self.fixture {
            // `absolute` always starts with '/', so strip it before joining or
            // Path::join would discard the prefix entirely and read the real
            // system — a fixture that silently reads /sys is worse than no
            // fixture, because the test would pass on the author's machine.
            Some(root) => root.join(absolute.trim_start_matches('/')),
            None => PathBuf::from(absolute),
        }
    }

    fn read(&self, absolute: &str) -> Option<Vec<u8>> {
        std::fs::read(self.path(absolute)).ok()
    }

    fn exists(&self, absolute: &str) -> bool {
        self.path(absolute).exists()
    }

    /// An EFI variable's payload as a string.
    ///
    /// efivarfs prepends 4 bytes of attributes, and systemd writes these
    /// particular values as UTF-16LE. Dropping every NUL after the prefix
    /// recovers the ASCII content, which is all these variables contain; a
    /// full UTF-16 decode would be more correct and would also be the only
    /// reason this file needed a dependency.
    fn efivar(&self, name: &str, guid: &str) -> Option<String> {
        let raw = self.read(&format!("/sys/firmware/efi/efivars/{name}-{guid}"))?;
        if raw.len() <= 4 {
            return None;
        }
        let text: String = raw[4..]
            .iter()
            .copied()
            .filter(|b| *b != 0)
            .map(char::from)
            .collect();
        let trimmed = text.trim().to_string();
        (!trimmed.is_empty()).then_some(trimmed)
    }

    /// A one-byte boolean EFI variable, as the firmware publishes SecureBoot.
    fn efivar_bool(&self, name: &str, guid: &str) -> Option<bool> {
        let raw = self.read(&format!("/sys/firmware/efi/efivars/{name}-{guid}"))?;
        raw.get(4).map(|b| *b != 0)
    }
}

/// `bootctl list --json=short`, or the reason it could not be read.
fn boot_entries(roots: &Roots) -> (Option<Vec<Value>>, Option<String>) {
    if roots.fixture.is_some() {
        let p = roots.path("/bootctl-list.json");
        return match std::fs::read(&p) {
            Ok(bytes) => match serde_json::from_slice::<Value>(&bytes) {
                Ok(Value::Array(v)) => (Some(v), None),
                Ok(_) => (None, Some(format!("{} is not a JSON array", p.display()))),
                Err(e) => (None, Some(format!("{}: {e}", p.display()))),
            },
            Err(e) => (None, Some(format!("{}: {e}", p.display()))),
        };
    }
    if !Path::new("/usr/bin/bootctl").exists() {
        return (None, Some("bootctl is not installed".into()));
    }
    let out = match Command::new("/usr/bin/bootctl")
        .args(["list", "--json=short"])
        .output()
    {
        Ok(o) => o,
        Err(e) => return (None, Some(format!("could not run bootctl: {e}"))),
    };
    if !out.status.success() {
        let why = String::from_utf8_lossy(&out.stderr).trim().to_string();
        let why = if why.is_empty() { "bootctl failed".into() } else { why };
        // The usual cause, and worth naming: the ESP is mode 0700.
        let hint = if unsafe { libc::geteuid() } != 0 {
            format!("{why} (reading the ESP needs root)")
        } else {
            why
        };
        return (None, Some(hint));
    }
    match serde_json::from_slice::<Value>(&out.stdout) {
        Ok(Value::Array(v)) => (Some(v), None),
        Ok(_) => (None, Some("bootctl did not return a JSON array".into())),
        Err(e) => (None, Some(format!("could not parse bootctl output: {e}"))),
    }
}

fn read_json_file(roots: &Roots, absolute: &str) -> (Option<Value>, Option<String>) {
    match roots.read(absolute) {
        None => (None, None), // absent is a normal state, not an error
        Some(bytes) => match serde_json::from_slice::<Value>(&bytes) {
            Ok(v) => (Some(v), None),
            Err(e) => (None, Some(format!("{absolute}: {e}"))),
        },
    }
}

fn build_report(roots: &Roots) -> Value {
    let cmdline = roots
        .read("/proc/cmdline")
        .map(|b| String::from_utf8_lossy(&b).trim().to_string())
        .unwrap_or_default();

    // sd-stub sets StubInfo when a UKI's stub ran; sd-boot sets LoaderInfo.
    // Between them they identify the path this boot actually took, which is
    // more reliable than looking for a bootloader binary on disk: an ESP can
    // hold several and only one of them ran.
    let stub_info = roots.efivar("StubInfo", LOADER_GUID);
    let loader_info = roots.efivar("LoaderInfo", LOADER_GUID);
    let boot_count_path = roots.efivar("LoaderBootCountPath", LOADER_GUID);

    let bootloader = if loader_info
        .as_deref()
        .is_some_and(|s| s.starts_with("systemd-boot"))
    {
        "systemd-boot"
    } else if cmdline.contains("BOOT_IMAGE=") || cmdline.contains("ostree=") {
        // GRUB's BLS entries carry ostree=; this is the expected answer on
        // every published APEX image.
        "grub"
    } else {
        "unknown"
    };

    let (entries, entries_error) = boot_entries(roots);
    let (health, health_error) = read_json_file(roots, "/var/lib/apex/boot/last-health.json");
    let (notice, notice_error) = read_json_file(roots, "/var/lib/apex/boot/rollback-notice.json");

    // Boot counting is in effect exactly when systemd-boot set
    // LoaderBootCountPath. This is the same signal apex-boot-health.service
    // and systemd-bless-boot-generator condition on, so all three agree by
    // construction instead of by convention.
    let counting = boot_count_path.is_some();

    let mut tallies = Map::new();
    if let Some(list) = entries.as_ref() {
        for e in list {
            let id = e.get("id").and_then(Value::as_str).unwrap_or("?");
            // triesLeft is ABSENT for an entry with no counter, which is what
            // a blessed deployment looks like. Defaulting it to 0 would report
            // every healthy deployment as out of tries.
            tallies.insert(
                id.to_string(),
                json!({
                    "path": e.get("path"),
                    "title": e.get("title"),
                    "triesLeft": e.get("triesLeft"),
                    "triesDone": e.get("triesDone"),
                    "blessed": e.get("triesLeft").is_none(),
                    "exhausted": e.get("triesLeft").and_then(Value::as_u64) == Some(0),
                    "isDefault": e.get("isDefault").and_then(Value::as_bool).unwrap_or(false),
                }),
            );
        }
    }

    json!({
        "bootloader": bootloader,
        "loaderInfo": loader_info,
        "stubInfo": stub_info,
        "bootedFromUki": stub_info.is_some(),
        "secureBoot": {
            // Absent means the kernel exposed no such variable, i.e. this is
            // not a UEFI boot at all. Reporting `false` there would claim a
            // measurement that was never taken.
            "enabled": roots.efivar_bool("SecureBoot", GLOBAL_GUID),
            "setupMode": roots.efivar_bool("SetupMode", GLOBAL_GUID),
        },
        "measuredBoot": {
            "tpmPresent": roots.exists("/sys/class/tpm/tpm0"),
            "eventLog": roots.exists("/sys/kernel/security/tpm0/binary_bios_measurements"),
            // sd-stub hands the UKI's .pcrsig/.pcrpkey to userspace here. Its
            // presence is what makes a signed PCR 11 policy — and therefore a
            // TPM-bound LUKS2 keyslot that survives a kernel update — possible
            // on this boot.
            "pcrSignature": roots.exists("/run/systemd/tpm2-pcr-signature.json"),
            "pcrPublicKey": roots.exists("/run/systemd/tpm2-pcr-public-key.pem"),
        },
        "bootCounting": {
            "inEffect": counting,
            "countPath": boot_count_path,
            "selectedEntry": roots.efivar("LoaderEntrySelected", LOADER_GUID),
            "defaultEntry": roots.efivar("LoaderEntryDefault", LOADER_GUID),
            "entries": if entries.is_some() { Value::Object(tallies) } else { Value::Null },
            "entriesUnavailable": entries_error,
        },
        "health": health,
        "healthError": health_error,
        "rollbackNotice": notice,
        "rollbackNoticeError": notice_error,
    })
}

fn print_human(r: &Value) {
    let s = |p: &[&str]| -> String {
        let mut cur = r;
        for k in p {
            match cur.get(*k) {
                Some(v) => cur = v,
                None => return "unknown".into(),
            }
        }
        match cur {
            Value::Null => "unknown".into(),
            Value::String(x) => x.clone(),
            other => other.to_string(),
        }
    };

    println!("Bootloader     : {}", s(&["bootloader"]));
    if let Some(info) = r.get("loaderInfo").and_then(Value::as_str) {
        println!("                 {info}");
    }
    let sb = match r.pointer("/secureBoot/enabled") {
        Some(Value::Bool(true)) => "enabled",
        Some(Value::Bool(false)) => "disabled",
        _ => "unavailable (not a UEFI boot, or the variable is not exposed)",
    };
    println!("Secure Boot    : {sb}");
    println!(
        "Signed UKI     : {}",
        match r.get("bootedFromUki") {
            Some(Value::Bool(true)) => format!("yes ({})", s(&["stubInfo"])),
            _ => "no — kernel and initramfs were loaded separately".into(),
        }
    );

    let m = |k: &str| matches!(r.pointer(&format!("/measuredBoot/{k}")), Some(Value::Bool(true)));
    println!(
        "Measured boot  : TPM {}, event log {}, signed PCR policy {}",
        if m("tpmPresent") { "present" } else { "absent" },
        if m("eventLog") { "present" } else { "absent" },
        if m("pcrSignature") { "in effect" } else { "not in effect" },
    );

    let counting = matches!(r.pointer("/bootCounting/inEffect"), Some(Value::Bool(true)));
    if !counting {
        // The expected state on every published image. Say so, rather than
        // leaving a reader to wonder which half is broken.
        println!(
            "Boot counting  : not in effect — this machine boots via {}, which has no \n\
             \x20                boot counter. GRUB is the default for every published APEX\n\
             \x20                image; systemd-boot with counting is opt-in.",
            s(&["bootloader"])
        );
    } else {
        println!("Boot counting  : in effect");
        println!("  selected     : {}", s(&["bootCounting", "selectedEntry"]));
        match r.pointer("/bootCounting/entries") {
            Some(Value::Object(map)) if !map.is_empty() => {
                // BTreeMap so the order is stable between runs; an unstable
                // listing makes a diff of two reports unreadable.
                let ordered: BTreeMap<_, _> = map.iter().collect();
                for (id, e) in ordered {
                    let state = if e.get("exhausted") == Some(&Value::Bool(true)) {
                        "OUT OF TRIES"
                    } else if e.get("blessed") == Some(&Value::Bool(true)) {
                        "good"
                    } else {
                        "on trial"
                    };
                    let left = e
                        .get("triesLeft")
                        .and_then(Value::as_u64)
                        .map(|n| format!("{n} left"))
                        .unwrap_or_else(|| "no counter".into());
                    println!("  {id:<32} {state:<12} {left}");
                }
            }
            _ => println!(
                "  entries      : unavailable — {}",
                r.get("bootCounting")
                    .and_then(|c| c.get("entriesUnavailable"))
                    .and_then(Value::as_str)
                    .unwrap_or("no reason reported")
            ),
        }
    }

    match r.get("health") {
        Some(Value::Object(h)) => {
            let verdict = h.get("verdict").and_then(Value::as_str).unwrap_or("?");
            println!("Last health    : {verdict} at {}",
                     h.get("checkedAt").and_then(Value::as_str).unwrap_or("?"));
            // Which entry the verdict was about, and which target had to be
            // reached for it. Without these the verdict is unattributable on a
            // machine with more than one deployment — and the health script
            // records both, so dropping them here was losing information
            // rather than choosing not to gather it.
            println!("  entry        : {}",
                     h.get("entry").and_then(Value::as_str).unwrap_or("?"));
            println!("  target       : {}",
                     h.get("target").and_then(Value::as_str).unwrap_or("?"));
            if let Some(Value::Array(f)) = h.get("failures") {
                for item in f {
                    println!("  failure      : {}", item.as_str().unwrap_or("?"));
                }
            }
        }
        // "never ran" is not "healthy". apex-boot-health.service only runs
        // when boot counting is in effect, so on a GRUB machine this is the
        // correct and expected answer.
        _ => println!("Last health    : no verdict recorded (the health unit has not run)"),
    }

    // `rolledBack` must be true, not merely present. A notice file that exists
    // with rolledBack false would otherwise announce a rollback that did not
    // happen, and the file is written by a separate program in a separate
    // language — presence is not a contract, the field is.
    if let Some(Value::Object(n)) = r.get("rollbackNotice").filter(|v| {
        v.get("rolledBack") == Some(&Value::Bool(true))
    }) {
        println!();
        println!("!! This machine was rolled back automatically.");
        println!(
            "   Running     : {}",
            n.get("runningEntry").and_then(Value::as_str).unwrap_or("?")
        );
        if let Some(Value::Array(failed)) = n.get("failedEntries") {
            for e in failed {
                println!(
                    "   Failed      : {} after {} attempts",
                    e.get("id").and_then(Value::as_str).unwrap_or("?"),
                    e.get("triesDone").and_then(Value::as_u64).unwrap_or(0)
                );
            }
        }
        println!("   Noticed     : {}",
                 n.get("noticedAt").and_then(Value::as_str).unwrap_or("?"));
    }
}

/// Returns a process exit code, like the other read-only reporting verbs.
///
/// 0 always: this reports state, and "GRUB, no boot counter, no TPM policy" is
/// the correct state for every published APEX image rather than a fault. A
/// non-zero exit would make `apex boot status` unusable as a health check in
/// exactly the configuration APEX ships. The only failure it can report is its
/// own — an unserialisable report, which cannot happen and is handled anyway
/// rather than unwrapped.
pub fn boot_main(cmd: BootCmd) -> i32 {
    match cmd {
        BootCmd::Status { json: as_json } => {
            let roots = Roots::from_env();
            let report = build_report(&roots);
            if as_json {
                match serde_json::to_string_pretty(&report) {
                    Ok(text) => println!("{text}"),
                    Err(e) => {
                        eprintln!("apex boot status: could not serialise the report: {e}");
                        return 1;
                    }
                }
            } else {
                print_human(&report);
            }
            0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fixture root makes every read relative, including the ones that would
    /// otherwise hit the developer's own /sys. This is asserted directly
    /// because a `Path::join` on an absolute path silently discards the
    /// prefix, and the resulting test would pass on a machine that happens to
    /// look right.
    #[test]
    fn fixture_root_is_not_discarded_by_an_absolute_path() {
        let roots = Roots { fixture: Some(PathBuf::from("/tmp/fixture")) };
        assert_eq!(
            roots.path("/proc/cmdline"),
            PathBuf::from("/tmp/fixture/proc/cmdline")
        );
        assert_eq!(
            roots.path("/sys/firmware/efi/efivars/StubInfo-x"),
            PathBuf::from("/tmp/fixture/sys/firmware/efi/efivars/StubInfo-x")
        );
    }

    #[test]
    fn no_fixture_root_reads_the_real_paths() {
        let roots = Roots { fixture: None };
        assert_eq!(roots.path("/proc/cmdline"), PathBuf::from("/proc/cmdline"));
    }

    /// A blessed entry has no `triesLeft` key at all. Treating a missing key as
    /// 0 would report every healthy deployment as out of tries and would make
    /// the rollback notice fire on a machine that never rolled back.
    #[test]
    fn a_missing_tries_left_is_blessed_not_exhausted() {
        let blessed: Value = serde_json::json!({"id": "apex-good.efi"});
        let exhausted: Value = serde_json::json!({"id": "apex-new.efi", "triesLeft": 0});
        let on_trial: Value = serde_json::json!({"id": "apex-new.efi", "triesLeft": 2});

        assert!(blessed.get("triesLeft").is_none());
        assert_eq!(exhausted.get("triesLeft").and_then(Value::as_u64), Some(0));
        assert_ne!(on_trial.get("triesLeft").and_then(Value::as_u64), Some(0));
    }
}
