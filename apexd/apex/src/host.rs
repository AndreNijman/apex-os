//! `apex host` — the trusted devices §20 dispatches to, and the I/O around them.
//!
//! [`apexd_core::host`] owns the registry format, the validation and the argv
//! construction, and performs no I/O. This file is the other half: reading and
//! writing the two files, running `ssh`, and printing.
//!
//! ── How a host is probed, and why an APEX peer describes itself ────────────
//!
//! Two paths, tried in order:
//!
//! 1. **`apex host describe --json` on the remote.** An APEX machine reports
//!    its own capabilities from Rust, serialising the same [`HostCaps`] struct
//!    the local side deserialises. One type, one format, no parser.
//! 2. **A portable shell probe**, for a host that has no `apex` — a plain
//!    Fedora box, a server, someone else's laptop. It prints `key=value` lines.
//!
//! The fallback prints `key=value` rather than JSON on purpose. Assembling JSON
//! in `sh` means quoting model names and PCI strings by hand into a format that
//! fails as a whole if one field is wrong; `key=value` degrades per line, and a
//! field this build does not recognise is simply skipped. The probe's job is to
//! make `apex host list` say something true about a machine that is not APEX,
//! not to be complete.
//!
//! Neither path is trusted to be well-formed. A probe result is remote output —
//! from a host that is trusted to *run commands for you*, which is not the same
//! as trusted to emit valid JSON — so every field is bounded on the way in.
//!
//! ── Why the probe cache is not in the registry ──────────────────────────────
//!
//! Because a probe is a measurement and the registry is user-owned. That rule
//! is [`apexd_core::gameprofile`]'s, applied here: a file written only in
//! response to an explicit user command stays hand-editable and refuses unknown
//! keys, and anything a probe writes lives elsewhere and tolerates them. See
//! [`apexd_core::host`]'s module docs for the table.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Command;

use anyhow::{anyhow, Context, Result};
use clap::{Args, Subcommand};

use apexd_core::host::{
    remote_sh, ssh_argv, validate_destination, validate_name, Host, HostCaps, Hosts, Tty,
};

/// How long to wait for a TCP connection before giving up, in seconds.
///
/// Short on purpose. A laptop that has left the LAN must fail fast: the
/// developer's own `katana` alias falls back through a VPS and a jump host, and
/// each attempt pays this. Long enough for a slow link, short enough that
/// `apex host list` on a disconnected laptop returns rather than appearing to
/// hang.
pub const CONNECT_TIMEOUT: u32 = 8;

/// How long a probe result is presented without comment. Older than this and
/// `list` marks it stale — reported, never refused, because a stale probe is
/// still the best information available and a laptop is offline most of the
/// time.
const PROBE_FRESH_SECS: i64 = 7 * 24 * 60 * 60;

/// Longest remote output a probe will read, in bytes.
///
/// A trusted host is trusted to run your commands, not to bound its own output.
/// Without a cap, a host whose `apex` prints a gigabyte of JSON — or whose
/// shell profile writes an infinite banner — turns `apex host list` into an
/// out-of-memory kill on the *local* machine.
const MAX_PROBE_BYTES: usize = 64 * 1024;

#[derive(Args)]
pub struct HostArgs {
    #[command(subcommand)]
    pub cmd: HostCmd,
}

#[derive(Subcommand)]
pub enum HostCmd {
    /// Register a trusted device and probe what it can do.
    ///
    /// The name is how you refer to it (`apex ai run --on katana`). By default
    /// it is also the ssh destination, so a machine already in `~/.ssh/config`
    /// needs nothing else — and keeps whatever that entry says, including a
    /// ProxyCommand or a Match exec that picks a transport per network.
    Add {
        /// How you will refer to this device.
        name: String,
        /// The ssh destination, when it differs from the name: an alias from
        /// `~/.ssh/config`, or `user@host`.
        #[arg(long)]
        ssh: Option<String>,
        /// A port, when the destination is not an alias that carries one.
        #[arg(long)]
        port: Option<u16>,
        /// A reminder of what this machine is for.
        #[arg(long)]
        note: Option<String>,
        /// Register without probing. The device is recorded and its
        /// capabilities stay unknown until `apex host probe` runs.
        #[arg(long)]
        no_probe: bool,
    },
    /// Every registered device, with what the last probe found.
    List {
        /// Machine-readable, for the shell and for scripts.
        #[arg(long)]
        json: bool,
    },
    /// One device in full.
    Show {
        name: String,
        #[arg(long)]
        json: bool,
    },
    /// Forget a device. Removes the registry entry and its cached probe.
    Remove { name: String },
    /// Re-probe a device, or every device.
    Probe {
        /// The device to probe. Omit with `--all`.
        name: Option<String>,
        /// Probe every registered device.
        #[arg(long)]
        all: bool,
    },
    /// Run a command on a device. The escape hatch §24 asks APEX to keep.
    ///
    /// Arguments after `--` are passed through with their boundaries intact,
    /// which plain `ssh host cmd a b` does not do: ssh joins its remote
    /// arguments with spaces and hands the string to the remote shell, so a
    /// path with a space in it silently becomes two arguments.
    Run {
        name: String,
        /// Allocate a terminal on the remote. What an editor or a TUI needs.
        #[arg(long, short = 't')]
        tty: bool,
        #[arg(trailing_var_arg = true, required = true)]
        argv: Vec<String>,
    },
    /// What this machine offers, as a peer would see it.
    ///
    /// This is what `apex host probe` runs on the far side: the remote APEX
    /// serialises the struct the local one parses, so the probe has no parser
    /// and cannot disagree with itself. Useful directly for seeing what your
    /// own box advertises.
    Describe {
        #[arg(long)]
        json: bool,
    },
    /// Print where the registry and the probe cache live.
    Path,
}

// ── where things live ────────────────────────────────────────────────────────

/// `~/.config/apex/hosts.toml`, or `$XDG_CONFIG_HOME`'s equivalent.
///
/// Resolved through `apex_agent_core::paths`, the same tested implementation of
/// the base-directory spec that `blueprint.rs` and `gaming.rs` use, rather than
/// a third one.
pub fn hosts_path() -> PathBuf {
    apex_agent_core::paths::config_home().join("apex/hosts.toml")
}

/// `~/.local/state/apex/hosts/` — the probe cache, one JSON file per device.
///
/// Under the state directory rather than the config one because nothing here is
/// user-owned: it is a measurement, it goes stale, and deleting it costs a
/// re-probe and nothing else.
fn caps_dir() -> PathBuf {
    apex_agent_core::paths::state_home().join("apex/hosts")
}

fn caps_path(name: &str) -> Result<PathBuf> {
    // Validated again here even though every caller validated already: this is
    // the function that turns a name into a *path*, so it is where a traversal
    // would land.
    validate_name(name)?;
    Ok(caps_dir().join(format!("{name}.json")))
}

const HEADER: &str = "\
# APEX trusted devices. Hand-editable.
#
# Each entry names an ssh destination — normally an alias from ~/.ssh/config,
# which is why there is no address, key or port to repeat here. Authentication,
# host identity and transport are whatever `ssh <destination>` already does.
#
#   [host.katana]
#   ssh  = \"katana\"        # optional; defaults to the entry's own name
#   note = \"build box\"
";

/// Read the registry. A missing file is an empty set, never an error: `list` on
/// a machine nobody has configured should say so, not fail. A file that exists
/// and is *wrong* is always an error.
fn load() -> Result<Hosts> {
    let path = hosts_path();
    match std::fs::read_to_string(&path) {
        Ok(text) => Hosts::parse(&text).with_context(|| path.display().to_string()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Hosts::default()),
        Err(e) => Err(e).with_context(|| path.display().to_string()),
    }
}

/// Write the registry, atomically, after proving it reads back identically.
///
/// The round-trip is `gaming.rs`'s rule and it earns its keep the same way:
/// without it a bad write is discovered by the *next* command, or on another
/// machine, with no way to tell which end was wrong.
fn save(hosts: &Hosts) -> Result<()> {
    let path = hosts_path();
    let body = hosts.to_toml().context("cannot render the hosts file")?;
    let text = format!("{HEADER}\n{body}");

    let reparsed = Hosts::parse(&text)
        .context("refusing to write a hosts file that cannot be read back")?;
    if &reparsed != hosts {
        return Err(anyhow!(
            "refusing to write a hosts file that does not round-trip: \
             rendered {} entries, read back {}",
            hosts.host.len(),
            reparsed.host.len()
        ));
    }

    let dir = path.parent().expect("hosts path always has a parent");
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;

    // Same-directory temp then rename: a rename within one filesystem is atomic,
    // so a crash or a full disk leaves the old file intact rather than a
    // half-written one. The pid keeps two concurrent `apex host add` runs from
    // sharing a temp path.
    let tmp = path.with_extension(format!("toml.tmp.{}", std::process::id()));
    std::fs::write(&tmp, &text).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, &path).with_context(|| format!("renaming into {}", path.display()))?;
    Ok(())
}

fn load_caps(name: &str) -> Option<HostCaps> {
    let text = std::fs::read_to_string(caps_path(name).ok()?).ok()?;
    // A corrupt cache is a missing cache. It is a measurement that can be taken
    // again, so failing the whole command over it would be trading something
    // recoverable for something not.
    serde_json::from_str(&text).ok()
}

/// The cached probe for a host, if there is one.
///
/// Public for [`crate::dispatch`], which uses it to refuse a forwarded verb
/// against a host that was probed and lacks the capability — without paying an
/// ssh round trip to learn what is already on disk. A host that has never been
/// probed returns `None` and is not refused: "unknown" and "absent" are
/// different answers.
pub fn cached_caps(name: &str) -> Option<HostCaps> {
    load_caps(name)
}

fn save_caps(name: &str, caps: &HostCaps) -> Result<()> {
    let path = caps_path(name)?;
    let dir = caps_dir();
    apex_agent_core::paths::ensure_private_dir(&dir)
        .with_context(|| format!("creating {}", dir.display()))?;
    let tmp = path.with_extension(format!("json.tmp.{}", std::process::id()));
    std::fs::write(&tmp, serde_json::to_string_pretty(caps)?)
        .with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, &path).with_context(|| format!("renaming into {}", path.display()))?;
    Ok(())
}

// ── the probe ────────────────────────────────────────────────────────────────

/// The portable fallback probe, for a host with no `apex`.
///
/// Constraints that shaped it: POSIX `sh`, no `bash`, no `jq`, nothing that is
/// not on a minimal Fedora or Debian box, and every lookup individually
/// tolerant of being unavailable. It prints `key=value` lines and its exit
/// status is deliberately always 0 — a host that cannot report its GPU has
/// still told us its CPU count, and a non-zero exit would throw that away.
///
/// `2>/dev/null` on each lookup rather than once at the top, so a single
/// unavailable file cannot silence the rest.
const SHELL_PROBE: &str = r#"
os=$(. /etc/os-release 2>/dev/null && printf '%s' "$PRETTY_NAME")
[ -n "$os" ] && echo "os=$os"
variant=$(. /etc/os-release 2>/dev/null && printf '%s' "$VARIANT_ID")
[ -n "$variant" ] && echo "variant=$variant"
c=$(nproc 2>/dev/null) && echo "cpus=$c"
m=$(awk '/^MemTotal:/{printf "%d", $2/1024}' /proc/meminfo 2>/dev/null) && echo "memory_mib=$m"
f=$(df -Pm /var 2>/dev/null | awk 'NR==2{print $4}') && echo "free_mib=$f"
command -v podman >/dev/null 2>&1 && echo "podman=1"
command -v nvidia-smi >/dev/null 2>&1 && echo "accel=cuda"
[ -e /dev/kfd ] && echo "accel=rocm"
command -v vulkaninfo >/dev/null 2>&1 && echo "accel=vulkan"
for d in /sys/class/drm/card*/device; do
  [ -r "$d/uevent" ] || continue
  n=$(sed -n 's/^DRIVER=//p' "$d/uevent" 2>/dev/null | head -1)
  [ -n "$n" ] && echo "gpu=$n"
done
exit 0
"#;

/// Parse the fallback probe's `key=value` output.
///
/// Unknown keys are skipped rather than collected into `HostCaps::unknown`:
/// that field exists for *version skew between two `apex` installs*, and this
/// path runs on a host with no `apex` at all, so a key here is noise from a
/// shell profile, not a newer peer.
fn parse_shell_probe(text: &str, now: i64) -> HostCaps {
    let mut caps = HostCaps { probed_at: now, ..Default::default() };
    for line in text.lines() {
        let Some((k, v)) = line.split_once('=') else { continue };
        let v = v.trim();
        if v.is_empty() || v.len() > 200 {
            continue;
        }
        match k.trim() {
            "os" => caps.os = Some(v.to_string()),
            "variant" => caps.variant = Some(v.to_string()),
            "cpus" => caps.cpus = v.parse().ok(),
            "memory_mib" => caps.memory_mib = v.parse().ok(),
            "free_mib" => caps.free_mib = v.parse().ok(),
            "podman" => caps.podman = v == "1",
            // Repeated keys accumulate, and duplicates are dropped: a machine
            // with two AMD cards reports `gpu=amdgpu` twice and listing it once
            // is what a person wants to read. Bounded, because the length of
            // this list is the remote host's choice, not ours.
            "accel" => push_bounded(&mut caps.accel, v),
            "gpu" => push_bounded(&mut caps.gpus, v),
            _ => {}
        }
    }
    caps
}

/// Append `value` to `list` unless it is already there or the list is full.
///
/// Shared by the `accel` and `gpu` arms of the probe parser: both accumulate
/// repeated keys from remote output, and both must be bounded because the
/// number of lines is the remote's choice.
fn push_bounded(list: &mut Vec<String>, value: &str) {
    if list.len() < 8 && !list.iter().any(|e| e == value) {
        list.push(value.to_string());
    }
}

/// Parse what a peer's `apex host describe --json` printed.
///
/// Separate from [`probe`] so the join between the two ends is testable with
/// output a real machine actually produced, rather than only through a live
/// ssh: `describe_self` on one box and this function on another are the two
/// halves that have to agree, and proving each alone proves nothing about the
/// pair.
///
/// `None` means "not usable, fall back to the shell probe" — an `apex` too old
/// to know the verb prints a clap usage error, which is not JSON.
fn parse_describe(out: &str, now: i64) -> Option<HostCaps> {
    let mut caps: HostCaps = serde_json::from_str(out).ok()?;
    // A peer that answered but claims nothing identifying is not an APEX peer
    // reporting itself; it is something else that happened to emit JSON.
    caps.apex_version.as_ref()?;
    // The remote stamped its own clock. Ours is the one that decides staleness,
    // because two machines' clocks disagree and the comparison is local.
    caps.probed_at = now;
    bound_caps(&mut caps);
    Some(caps)
}

/// Run one command on a host and return its stdout, bounded.
fn ssh_capture(host: &Host, name: &str, command: &str) -> Result<(bool, String)> {
    let dest = host.destination(name);
    let argv = ssh_argv(dest, host.port, Tty::None, CONNECT_TIMEOUT, Some(command));
    let out = Command::new(&argv[0])
        .args(&argv[1..])
        .output()
        .with_context(|| format!("running ssh for host {name:?}"))?;
    let mut stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    if stdout.len() > MAX_PROBE_BYTES {
        stdout.truncate(MAX_PROBE_BYTES);
    }
    Ok((out.status.success(), stdout))
}

/// Probe a host: ask its `apex` first, fall back to the shell probe.
fn probe(name: &str, host: &Host) -> Result<HostCaps> {
    let now = unix_now();

    // Path 1: an APEX peer describes itself. `--json` output is the same struct
    // this deserialises.
    let describe = remote_sh(&["apex", "host", "describe", "--json"]);
    if let Ok((ok, out)) = ssh_capture(host, name, &describe) {
        if ok {
            if let Some(caps) = parse_describe(&out, now) {
                return Ok(caps);
            }
        }
    }

    // Path 2: not APEX, or an `apex` too old to describe itself.
    let (_, out) = ssh_capture(host, name, &remote_sh(&["sh", "-c", SHELL_PROBE]))?;
    if out.trim().is_empty() {
        return Err(anyhow!(
            "host {name:?} did not answer. Check that `ssh {}` works — APEX runs \
             ssh with BatchMode=yes, so a host that needs a password or an \
             unknown-key confirmation fails here rather than prompting",
            host.destination(name)
        ));
    }
    Ok(parse_shell_probe(&out, now))
}

/// Bound every field that arrived from a remote host.
///
/// A trusted host is trusted to run commands, not to bound its own output. A
/// peer with a 10 MB `apex_version` string would otherwise be written straight
/// into the cache and printed to the terminal.
fn bound_caps(caps: &mut HostCaps) {
    fn clamp(s: &mut Option<String>, max: usize) {
        if let Some(v) = s {
            if v.len() > max {
                v.truncate(max);
            }
        }
    }
    clamp(&mut caps.apex_version, 64);
    clamp(&mut caps.variant, 64);
    clamp(&mut caps.os, 200);
    caps.gpus.truncate(8);
    caps.accel.truncate(8);
    for g in &mut caps.gpus {
        g.truncate(200);
    }
    for a in &mut caps.accel {
        a.truncate(64);
    }
    // A newer peer's unknown fields are kept, but not unboundedly.
    if caps.unknown.len() > 32 {
        let keep: BTreeMap<_, _> =
            caps.unknown.iter().take(32).map(|(k, v)| (k.clone(), v.clone())).collect();
        caps.unknown = keep;
    }
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// ── describing this machine ──────────────────────────────────────────────────

/// What this machine offers, for a peer's probe.
///
/// Every field is read from the local system; nothing is assumed. A capability
/// this machine cannot demonstrate is reported absent rather than defaulted,
/// which is the rule `apex perf` follows for sensors it cannot find.
pub fn describe_self() -> HostCaps {
    let mut caps = HostCaps { probed_at: unix_now(), ..Default::default() };

    caps.apex_version = Some(env!("CARGO_PKG_VERSION").to_string());

    if let Ok(osr) = std::fs::read_to_string("/etc/os-release") {
        for line in osr.lines() {
            if let Some(v) = line.strip_prefix("VARIANT_ID=") {
                caps.variant = Some(v.trim_matches('"').to_string());
            } else if let Some(v) = line.strip_prefix("PRETTY_NAME=") {
                caps.os = Some(v.trim_matches('"').to_string());
            }
        }
    }

    caps.cpus = std::thread::available_parallelism().ok().map(|n| n.get() as u32);

    if let Ok(mi) = std::fs::read_to_string("/proc/meminfo") {
        for line in mi.lines() {
            if let Some(rest) = line.strip_prefix("MemTotal:") {
                caps.memory_mib =
                    rest.split_whitespace().next().and_then(|k| k.parse::<u64>().ok()).map(|k| k / 1024);
            }
        }
    }

    // Accelerator runtimes, by the presence of the thing that would be used
    // rather than by a GPU name: a machine can have an NVIDIA card and no CUDA.
    if apexd_core::gpu::nvidia_smi_available() {
        caps.accel.push("cuda".into());
    }
    if std::path::Path::new("/dev/kfd").exists() {
        caps.accel.push("rocm".into());
    }
    if which("vulkaninfo") {
        caps.accel.push("vulkan".into());
    }

    for entry in glob_drm_drivers() {
        if !caps.gpus.contains(&entry) {
            caps.gpus.push(entry);
        }
    }

    caps.podman = which("podman");
    // The agent runtime and the inference service are reported by the presence
    // of their binaries, not by asking systemd: `apex-agentd` is opt-in and
    // deliberately not enabled globally, so "not running" is its normal state
    // and would make a capable host look incapable.
    caps.agentd = which("apex-agentd");
    caps.ai = which("apex-aid");

    caps
}

/// Whether a command exists on `PATH`.
///
/// `PATH` is split by hand rather than shelling out to `command -v`: this runs
/// on the probed side of a dispatch, and spawning a shell per lookup is both
/// slower and one more thing that can inherit a hostile environment.
fn which(bin: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else { return false };
    std::env::split_paths(&path).any(|dir| dir.join(bin).is_file())
}

/// DRM driver names from `/sys`, which needs no root and no `lspci`.
fn glob_drm_drivers() -> Vec<String> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir("/sys/class/drm") else { return out };
    for e in rd.flatten() {
        let name = e.file_name();
        let name = name.to_string_lossy();
        // `card0`, not `card0-DP-1`: the connectors are outputs of the same
        // device and would each report the driver again.
        if !name.starts_with("card") || name.contains('-') {
            continue;
        }
        let uevent = e.path().join("device/uevent");
        if let Ok(text) = std::fs::read_to_string(uevent) {
            for line in text.lines() {
                if let Some(d) = line.strip_prefix("DRIVER=") {
                    let d = d.to_string();
                    if !out.contains(&d) {
                        out.push(d);
                    }
                }
            }
        }
    }
    out
}

// ── rendering ────────────────────────────────────────────────────────────────

fn describe_caps_line(caps: &Option<HostCaps>, now: i64) -> String {
    let Some(c) = caps else { return "not probed".to_string() };

    let mut bits = Vec::new();
    if let Some(v) = &c.apex_version {
        bits.push(match &c.variant {
            Some(var) => format!("APEX {v} ({var})"),
            None => format!("APEX {v}"),
        });
    } else if let Some(os) = &c.os {
        // The shell-probe path cannot report an apex version, so a real APEX
        // machine whose `apex` is too old to describe itself lands here. Its
        // VARIANT_ID was still probed and is still worth showing — dropping it
        // made a `gaming` box read identically to a `daily` one.
        bits.push(match &c.variant {
            Some(var) => format!("{os} ({var})"),
            None => os.clone(),
        });
    }
    if let Some(n) = c.cpus {
        bits.push(format!("{n} cpu"));
    }
    if let Some(m) = c.memory_mib {
        bits.push(format!("{} GiB", m / 1024));
    }
    if !c.accel.is_empty() {
        bits.push(c.accel.join("+"));
    }
    let mut can = Vec::new();
    if c.ai {
        can.push("ai");
    }
    if c.agentd {
        can.push("agent");
    }
    if c.podman {
        can.push("build");
    }
    if !can.is_empty() {
        bits.push(can.join(","));
    }

    let age = now.saturating_sub(c.probed_at);
    if age > PROBE_FRESH_SECS {
        bits.push(format!("probe {}d old", age / 86_400));
    }
    bits.join(", ")
}

// ── the commands ─────────────────────────────────────────────────────────────

/// The exit code every `apex` subcommand uses for a refusal, so `apex host`
/// fails the same way `apex blueprint` does rather than inventing a code.
use crate::blueprint::EXIT_ERROR;

/// `apex host`. Returns an exit code rather than a `Result` because that is the
/// dispatch's type in `main.rs` — every command prints its own refusal and
/// returns a code, so the top level has no error formatting to get wrong.
pub fn run(args: HostArgs) -> i32 {
    match dispatch(args) {
        Ok(()) => 0,
        Err(e) => {
            // `{e:#}` prints the anyhow context chain, so a failure that came
            // from three layers down still says which file or host it was
            // about. `{e}` alone would print only the outermost sentence.
            eprintln!("apex host: {e:#}");
            EXIT_ERROR
        }
    }
}

fn dispatch(args: HostArgs) -> Result<()> {
    match args.cmd {
        HostCmd::Add { name, ssh, port, note, no_probe } => {
            validate_name(&name)?;
            if let Some(d) = &ssh {
                validate_destination(&name, d)?;
            }
            let mut hosts = load()?;
            let host = Host { ssh, port, note, ..Default::default() };
            // Validated as part of a whole registry, not on its own: that is
            // the function the file is checked with, so a new entry is held to
            // exactly the same standard as a hand-edited one.
            let mut candidate = hosts.clone();
            candidate.host.insert(name.clone(), host.clone());
            candidate.validate()?;

            let existed = hosts.host.insert(name.clone(), host.clone()).is_some();
            save(&hosts)?;
            println!(
                "{} host {name:?} -> ssh {}",
                if existed { "updated" } else { "added" },
                host.destination(&name)
            );

            if no_probe {
                println!("not probed (--no-probe); run `apex host probe {name}` when it is up");
                return Ok(());
            }
            match probe(&name, &host) {
                Ok(caps) => {
                    save_caps(&name, &caps)?;
                    println!("  {}", describe_caps_line(&Some(caps), unix_now()));
                }
                Err(e) => {
                    // Registered but unreachable is a normal state, not a
                    // failure to add: the laptop may simply be off the LAN.
                    println!("  not reachable right now: {e}");
                    println!("  the entry is saved; `apex host probe {name}` will fill this in");
                }
            }
            Ok(())
        }

        HostCmd::List { json } => {
            let hosts = load()?;
            let now = unix_now();
            if json {
                let mut out = serde_json::Map::new();
                for (name, host) in &hosts.host {
                    let caps = load_caps(name);
                    out.insert(
                        name.clone(),
                        serde_json::json!({
                            "ssh": host.destination(name),
                            "port": host.port,
                            "note": host.note,
                            "caps": caps,
                        }),
                    );
                }
                println!("{}", serde_json::to_string_pretty(&out)?);
                return Ok(());
            }
            if hosts.host.is_empty() {
                println!("no trusted devices.");
                println!("add one with `apex host add <name>` — the name is an ssh destination,");
                println!("so an entry already in ~/.ssh/config needs nothing more.");
                return Ok(());
            }
            let width = hosts.host.keys().map(|k| k.len()).max().unwrap_or(4).max(4);
            for (name, host) in &hosts.host {
                let caps = load_caps(name);
                println!(
                    "{name:<width$}  {}",
                    describe_caps_line(&caps, now),
                    width = width
                );
                if let Some(n) = &host.note {
                    println!("{:width$}  {n}", "", width = width);
                }
            }
            Ok(())
        }

        HostCmd::Show { name, json } => {
            let hosts = load()?;
            let host = hosts.get(&name)?;
            let caps = load_caps(&name);
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "name": name,
                        "ssh": host.destination(&name),
                        "port": host.port,
                        "note": host.note,
                        "caps": caps,
                    }))?
                );
                return Ok(());
            }
            println!("{name}");
            println!("  ssh          {}", host.destination(&name));
            if let Some(p) = host.port {
                println!("  port         {p}");
            }
            if let Some(n) = &host.note {
                println!("  note         {n}");
            }
            match &caps {
                None => println!("  capabilities not probed — run `apex host probe {name}`"),
                Some(c) => {
                    println!("  {}", describe_caps_line(&caps, unix_now()));
                    if !c.gpus.is_empty() {
                        println!("  gpu          {}", c.gpus.join(", "));
                    }
                    if let Some(f) = c.free_mib {
                        println!("  free         {} GiB", f / 1024);
                    }
                    for (k, v) in &c.unknown {
                        // A newer peer reported something this build does not
                        // know. Printed rather than hidden: it is the only clue
                        // that the two ends are different versions.
                        println!("  {k:<12} {v} (not understood by this apex)");
                    }
                }
            }
            Ok(())
        }

        HostCmd::Remove { name } => {
            let mut hosts = load()?;
            hosts.get(&name)?;
            hosts.host.remove(&name);
            save(&hosts)?;
            // Best effort: a leftover cache file for a removed host is noise,
            // and failing the removal over it would leave the registry and the
            // cache disagreeing about whether the host exists.
            if let Ok(p) = caps_path(&name) {
                let _ = std::fs::remove_file(p);
            }
            println!("removed host {name:?}");
            Ok(())
        }

        HostCmd::Probe { name, all } => {
            let hosts = load()?;
            let targets: Vec<String> = match (&name, all) {
                (Some(_), true) => {
                    return Err(anyhow!("give a host name or --all, not both"));
                }
                (Some(n), false) => {
                    hosts.get(n)?;
                    vec![n.clone()]
                }
                (None, true) => hosts.host.keys().cloned().collect(),
                (None, false) => {
                    return Err(anyhow!(
                        "which host? Give a name, or --all to probe every registered device"
                    ))
                }
            };
            if targets.is_empty() {
                println!("no trusted devices to probe.");
                return Ok(());
            }
            let mut failed = 0;
            for n in &targets {
                let host = hosts.get(n)?;
                match probe(n, host) {
                    Ok(caps) => {
                        save_caps(n, &caps)?;
                        println!("{n}  {}", describe_caps_line(&Some(caps), unix_now()));
                    }
                    Err(e) => {
                        failed += 1;
                        println!("{n}  unreachable: {e}");
                    }
                }
            }
            // Non-zero when every target failed, so this is usable as a check,
            // but zero when some succeeded: on a laptop, one host being off is
            // normal and `--all` should not fail because of it.
            if failed == targets.len() {
                return Err(anyhow!(
                    "no host answered ({failed} of {} unreachable)",
                    targets.len()
                ));
            }
            Ok(())
        }

        HostCmd::Run { name, tty, argv } => {
            let hosts = load()?;
            let host = hosts.get(&name)?;
            let command = remote_sh(&argv);
            let ssh = ssh_argv(
                host.destination(&name),
                host.port,
                if tty { Tty::Interactive } else { Tty::None },
                CONNECT_TIMEOUT,
                Some(&command),
            );
            // exec rather than spawn-and-wait: the remote command's exit status
            // becomes this process's, signals reach it, and there is no local
            // process left holding a pipe. `apex host run k -- false` must exit
            // 1, or it cannot be used in a script.
            exec(&ssh)
        }

        HostCmd::Describe { json } => {
            let caps = describe_self();
            if json {
                println!("{}", serde_json::to_string(&caps)?);
            } else {
                println!("{}", describe_caps_line(&Some(caps), unix_now()));
            }
            Ok(())
        }

        HostCmd::Path => {
            println!("registry     {}", hosts_path().display());
            println!("probe cache  {}", caps_dir().display());
            Ok(())
        }
    }
}

/// Replace this process with `argv`.
///
/// Returns only on failure — on success the process is gone. Uses
/// `CommandExt::exec` so the exit status, signal handling and terminal
/// ownership are the remote command's rather than something translated through
/// a parent.
fn exec(argv: &[String]) -> Result<()> {
    use std::os::unix::process::CommandExt;
    let err = Command::new(&argv[0]).args(&argv[1..]).exec();
    Err(anyhow!("cannot run {}: {err}", argv[0]))
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── the shell fallback parser ────────────────────────────────────────────

    #[test]
    fn the_shell_probe_parses_a_realistic_answer() {
        let out = "os=Fedora Linux 43 (Server Edition)\nvariant=server\ncpus=20\n\
                   memory_mib=64000\nfree_mib=110000\npodman=1\naccel=cuda\ngpu=nvidia\n";
        let c = parse_shell_probe(out, 1000);
        assert_eq!(c.os.as_deref(), Some("Fedora Linux 43 (Server Edition)"));
        assert_eq!(c.cpus, Some(20));
        assert_eq!(c.memory_mib, Some(64000));
        assert!(c.podman);
        assert_eq!(c.accel, vec!["cuda"]);
        assert_eq!(c.gpus, vec!["nvidia"]);
        assert_eq!(c.probed_at, 1000);
    }

    #[test]
    fn a_shell_probe_with_nothing_useful_is_still_a_result() {
        // A host that answered but could report nothing is reachable, which is
        // itself worth recording.
        let c = parse_shell_probe("", 5);
        assert_eq!(c.probed_at, 5);
        assert!(c.os.is_none());
    }

    #[test]
    fn junk_lines_from_a_shell_profile_are_skipped() {
        // Login shells print things. A banner must not become a capability.
        let out = "Welcome to the server!\nLast login: never\ncpus=4\n=\nkey=\n";
        let c = parse_shell_probe(out, 0);
        assert_eq!(c.cpus, Some(4));
        assert!(c.os.is_none());
    }

    #[test]
    fn a_repeated_accel_is_listed_once() {
        let c = parse_shell_probe("accel=cuda\naccel=cuda\naccel=rocm\n", 0);
        assert_eq!(c.accel, vec!["cuda", "rocm"]);
    }

    #[test]
    fn an_absurd_value_is_dropped_rather_than_stored() {
        let long = "x".repeat(500);
        let c = parse_shell_probe(&format!("os={long}\ncpus=2\n"), 0);
        assert!(c.os.is_none(), "a 500-byte os string was accepted");
        assert_eq!(c.cpus, Some(2), "the rest of the probe was discarded with it");
    }

    #[test]
    fn a_non_numeric_cpu_count_is_absent_not_zero() {
        // Zero would render as "0 cpu", which is a claim. Absent renders as
        // nothing, which is the truth.
        let c = parse_shell_probe("cpus=lots\n", 0);
        assert_eq!(c.cpus, None);
    }

    #[test]
    fn the_gpu_and_accel_lists_are_bounded() {
        let many: String = (0..50).map(|i| format!("gpu=card{i}\n")).collect();
        let c = parse_shell_probe(&many, 0);
        assert_eq!(c.gpus.len(), 8);
    }

    // ── the join between the two ends ────────────────────────────────────────

    /// Exactly what `apex host describe --json` printed on the katana — an APEX
    /// gaming box with an RTX 3070 and an Alder Lake iGPU — captured from a real
    /// ssh run rather than written by hand.
    ///
    /// A fixture I invented would prove that `parse_describe` accepts what I
    /// imagine `describe_self` emits. This proves it accepts what the other
    /// machine actually sent.
    const KATANA_DESCRIBE: &str = r#"{"probed_at":1788439662,"apex_version":"0.1.0","variant":"gaming","os":"APEX-OS","cpus":20,"memory_mib":63997,"gpus":["i915","nvidia"],"accel":["cuda","vulkan"],"agentd":false,"ai":false,"podman":true}"#;

    #[test]
    fn a_real_peers_describe_output_parses_into_every_field() {
        let c = parse_describe(KATANA_DESCRIBE, 999).expect("real peer output was rejected");
        assert_eq!(c.apex_version.as_deref(), Some("0.1.0"));
        assert_eq!(c.variant.as_deref(), Some("gaming"));
        assert_eq!(c.cpus, Some(20));
        assert_eq!(c.memory_mib, Some(63997));
        assert_eq!(c.gpus, vec!["i915", "nvidia"]);
        assert_eq!(c.accel, vec!["cuda", "vulkan"]);
        assert!(c.podman);
        assert!(!c.ai, "the katana has no inference service yet");
    }

    #[test]
    fn the_local_clock_decides_staleness_not_the_peers() {
        // Two machines' clocks disagree, and the comparison happens here.
        let c = parse_describe(KATANA_DESCRIBE, 999).unwrap();
        assert_eq!(c.probed_at, 999, "the peer's own timestamp survived");
    }

    #[test]
    fn what_describe_self_emits_is_what_parse_describe_accepts() {
        // The round trip through the actual wire format, both halves as shipped.
        let mine = serde_json::to_string(&describe_self()).unwrap();
        let parsed = parse_describe(&mine, 1).expect("this machine's own output was rejected");
        assert_eq!(parsed.cpus, describe_self().cpus);
    }

    #[test]
    fn an_old_apex_usage_error_falls_back_rather_than_parsing() {
        // What an `apex` predating this verb actually prints. Observed: the
        // katana's installed 0.1.0 did exactly this, which is why the live
        // probe took the shell path.
        let clap_err = "error: unrecognized subcommand 'host'\n\nUsage: apex <COMMAND>\n";
        assert!(parse_describe(clap_err, 0).is_none());
    }

    #[test]
    fn json_that_is_not_a_peer_report_is_refused() {
        // Something else answering on that command must not become a host
        // record. No apex_version means it is not an APEX peer describing
        // itself, whatever else it emitted.
        assert!(parse_describe(r#"{"cpus":4}"#, 0).is_none());
        assert!(parse_describe("[]", 0).is_none());
        assert!(parse_describe("", 0).is_none());
    }

    #[test]
    fn a_hostile_peer_report_is_bounded_on_the_way_in() {
        // parse_describe is the door; bounding must happen there, not later.
        let hostile = format!(
            r#"{{"apex_version":"{}","cpus":1}}"#,
            "v".repeat(5000)
        );
        let c = parse_describe(&hostile, 0).unwrap();
        assert_eq!(c.apex_version.unwrap().len(), 64);
    }

    // ── bounding remote output ───────────────────────────────────────────────

    #[test]
    fn a_hostile_peer_cannot_write_an_unbounded_version_string() {
        let mut c = HostCaps {
            apex_version: Some("v".repeat(10_000)),
            os: Some("o".repeat(10_000)),
            ..Default::default()
        };
        bound_caps(&mut c);
        assert_eq!(c.apex_version.unwrap().len(), 64);
        assert_eq!(c.os.unwrap().len(), 200);
    }

    #[test]
    fn a_hostile_peer_cannot_flood_the_unknown_map() {
        let mut c = HostCaps::default();
        for i in 0..200 {
            c.unknown.insert(format!("k{i}"), serde_json::json!(1));
        }
        bound_caps(&mut c);
        assert_eq!(c.unknown.len(), 32);
    }

    #[test]
    fn bounding_leaves_an_ordinary_answer_untouched() {
        // The bound must not be the thing that mangles a normal probe.
        let before = HostCaps {
            apex_version: Some("0.1.0".into()),
            os: Some("APEX-OS 43".into()),
            gpus: vec!["nvidia".into(), "i915".into()],
            accel: vec!["cuda".into()],
            cpus: Some(20),
            ..Default::default()
        };
        let mut after = before.clone();
        bound_caps(&mut after);
        assert_eq!(before, after);
    }

    // ── describing this machine ──────────────────────────────────────────────

    #[test]
    fn describe_self_reports_a_version_and_a_cpu_count() {
        // Runs against the real machine, so it asserts only what must be true
        // anywhere this test can run.
        let c = describe_self();
        assert!(c.is_apex(), "describe_self must always report an apex version");
        assert!(c.cpus.unwrap_or(0) >= 1);
    }

    #[test]
    fn describe_self_round_trips_through_the_wire_format() {
        // This is the probe's actual path: serialise here, parse on the peer.
        let c = describe_self();
        let json = serde_json::to_string(&c).unwrap();
        let back: HostCaps = serde_json::from_str(&json).unwrap();
        assert_eq!(back.apex_version, c.apex_version);
        assert_eq!(back.cpus, c.cpus);
        assert_eq!(back.accel, c.accel);
    }

    #[test]
    fn describe_self_does_not_claim_a_capability_from_a_gpu_alone() {
        // A machine with a GPU and no runtime must not report an accelerator.
        // Asserted structurally: every accel name is one of the three the code
        // can produce, and each is produced only by its own probe.
        let c = describe_self();
        for a in &c.accel {
            assert!(matches!(a.as_str(), "cuda" | "rocm" | "vulkan"), "unexpected accel {a:?}");
        }
    }

    #[test]
    fn which_finds_a_binary_that_exists_and_not_one_that_does_not() {
        assert!(which("sh"), "sh was not found on PATH");
        assert!(!which("apex-a-binary-that-does-not-exist"));
    }

    // ── rendering ────────────────────────────────────────────────────────────

    #[test]
    fn an_unprobed_host_says_so_rather_than_looking_capable() {
        assert_eq!(describe_caps_line(&None, 0), "not probed");
    }

    #[test]
    fn a_stale_probe_is_marked_stale() {
        let c = HostCaps { probed_at: 0, cpus: Some(4), ..Default::default() };
        let line = describe_caps_line(&Some(c), PROBE_FRESH_SECS + 86_400);
        assert!(line.contains("probe"), "got {line}");
        assert!(line.contains("d old"), "got {line}");
    }

    #[test]
    fn a_fresh_probe_is_not_marked_stale() {
        let c = HostCaps { probed_at: 100, cpus: Some(4), ..Default::default() };
        let line = describe_caps_line(&Some(c), 200);
        assert!(!line.contains("old"), "got {line}");
    }

    #[test]
    fn the_rendered_line_names_the_capabilities_a_dispatch_needs() {
        let c = HostCaps {
            apex_version: Some("0.1.0".into()),
            variant: Some("gaming".into()),
            cpus: Some(20),
            memory_mib: Some(64 * 1024),
            accel: vec!["cuda".into()],
            ai: true,
            agentd: true,
            podman: true,
            probed_at: 0,
            ..Default::default()
        };
        let line = describe_caps_line(&Some(c), 0);
        for want in ["APEX 0.1.0", "gaming", "20 cpu", "64 GiB", "cuda", "ai", "agent", "build"] {
            assert!(line.contains(want), "{want:?} missing from {line:?}");
        }
    }

    #[test]
    fn a_non_apex_host_renders_its_os_instead_of_a_version() {
        let c = HostCaps { os: Some("Debian 13".into()), cpus: Some(2), ..Default::default() };
        let line = describe_caps_line(&Some(c), 0);
        assert!(line.contains("Debian 13"), "got {line}");
        assert!(!line.contains("APEX"), "got {line}");
    }

    // ── the paths ────────────────────────────────────────────────────────────

    #[test]
    fn a_traversing_name_cannot_become_a_cache_path() {
        assert!(caps_path("../../etc/passwd").is_err());
        assert!(caps_path("..").is_err());
    }

    #[test]
    fn a_legal_name_becomes_a_file_inside_the_cache_directory() {
        let p = caps_path("katana").unwrap();
        assert!(p.starts_with(caps_dir()));
        assert_eq!(p.file_name().unwrap(), "katana.json");
    }

    #[test]
    fn the_registry_lives_beside_the_other_apex_config() {
        let p = hosts_path();
        assert!(p.ends_with("apex/hosts.toml"), "got {}", p.display());
    }

    // ── the shell probe script itself ────────────────────────────────────────

    #[test]
    fn the_shell_probe_is_posix_and_needs_no_extras() {
        // It runs on hosts that are not APEX and may not have bash or jq. This
        // asserts the *absence* of the things that would break there.
        for forbidden in ["jq", "bash", "[[", "local ", "declare "] {
            assert!(
                !SHELL_PROBE.contains(forbidden),
                "the portable probe uses {forbidden:?}, which a minimal host may not have"
            );
        }
    }

    #[test]
    fn the_shell_probe_cannot_fail_as_a_whole() {
        // Its exit is unconditional, so one unavailable lookup does not discard
        // the fields that did work.
        assert!(SHELL_PROBE.trim_end().ends_with("exit 0"));
    }

    #[test]
    fn every_shell_probe_lookup_silences_only_itself() {
        // A single 2>/dev/null at the top would hide errors from every later
        // line too. Each lookup that can fail carries its own redirection.
        let lines: Vec<&str> = SHELL_PROBE
            .lines()
            .filter(|l| l.contains("nproc") || l.contains("meminfo") || l.contains("df -Pm"))
            .collect();
        assert_eq!(lines.len(), 3, "the probe's shape changed: {lines:?}");
        for l in lines {
            assert!(l.contains("2>/dev/null"), "unguarded lookup: {l}");
        }
    }
}
