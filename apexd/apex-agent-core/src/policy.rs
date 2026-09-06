//! The six permission dimensions (roadmap §3.1).
//!
//! > These are distinct controls and must never be collapsed into one switch:
//! > agent-native permission mode, APEX filesystem/process sandbox, APEX
//! > system/root capability layer, APEX secret/cloud capability layer, APEX
//! > network policy, remote-origin policy. `bypassPermissions` changes only
//! > layer 1.
//!
//! [`AgentPolicy`] is that sentence as a type. Six fields, six enums, six
//! defaults, and no function anywhere that derives one of them from another
//! except the two documented below — both of which only ever tighten.
//!
//! ## Why a struct of six enums and not one mode enum
//!
//! A single `enum Mode { Safe, Yolo }` is how the layers get collapsed. It has
//! to be, because every new requirement becomes a new variant, and the variant
//! that means "the agent may not ask me before editing a file" is one rename
//! away from also meaning "the agent may read `~/.ssh`". The pressure is always
//! toward fewer variants, and fewer variants means coarser grants.
//!
//! Six independent fields cannot drift that way. Turning off confirmations is
//! `native = Bypass` and touches nothing else, because there is nothing else in
//! the field to touch. The three roadmap invariants this task exists to protect
//! —
//!
//! - `bypassPermissions` does not disable the APEX sandbox,
//! - unrestricted-user does not imply root,
//! - a root grant does not imply secret export —
//!
//! are then statements about which fields a change is allowed to write, which
//! is something a test can assert exhaustively over a value set. They are in
//! `tests/policy_invariants.rs`, named after the criteria they encode.
//!
//! ## Named modes are presets, not a seventh dimension
//!
//! §4 gives four user-visible modes on top of the default. Each is a
//! [`PolicyPreset`]: a point in the six-dimensional space, not a mode flag the
//! rest of the code branches on. `--agent-bypass` moves one coordinate;
//! `--unsafe-everything` moves three and leaves the secret dimension exactly
//! where it was, because §3.4 says so in as many words:
//!
//! > broker secrets are still not conveniently dumped into the agent
//! > environment.
//!
//! A user who wants a combination that has no name still sets the dimensions
//! individually. The presets exist so the four named modes are one word each,
//! not so the space is limited to five points.
//!
//! ## The two derivations, and why they only tighten
//!
//! [`AgentPolicy::effective_network`] — `strict` forces the network dimension
//! to `offline`. Strict has meant "project confinement with the network
//! removed" since before the dimensions were split, and BASE-003 preserves it.
//! It is a floor and never a ceiling: `unrestricted` does not imply an open
//! network, and asking for `unrestricted` with `offline` is refused rather than
//! quietly granted, because nothing in this build can enforce it.
//!
//! [`AgentPolicy::no_new_privs`] — every managed session runs with
//! `PR_SET_NO_NEW_PRIVS`, which is what stops `sudo` and every other setuid
//! binary inside it, unless the policy is the break-glass mode. §4.3 asks for
//! exactly this: an unrestricted-user session keeps `no_new_privs` "unless
//! system-access mode explicitly changes it". A confined session already got it
//! from `bwrap`; this is what extends it to the unconfined ones, which is the
//! kernel half of "unrestricted-user does not imply root".
//!
//! ## Fail closed on what is not built yet
//!
//! Six values in this vocabulary describe policy the runtime cannot enforce
//! today: two network modes, both system-access modes, raw secret export and
//! remote elevation. [`AgentPolicy::validate`] refuses each one and names the
//! task that will implement it.
//!
//! Refusing is the only honest option. A `--network allowlist` that parsed and
//! then ran with an open network would be worse than no flag at all: it would
//! read as a protection in `apex agent status`, in the Agent Center and in a
//! script, and there would be nothing behind it. Defining the vocabulary now
//! and rejecting the parts without an enforcement point is what lets P0-006,
//! P0-007 and P0-008 land as an implementation rather than a redesign.

use serde::{Deserialize, Serialize};

pub use crate::protocol::SandboxPolicy;

/// Dimension 1: the agent's own permission mode.
///
/// This is the only dimension that describes the upstream CLI rather than
/// APEX. Claude's `bypassPermissions`, Codex's approval policy and Gemini's
/// `--yolo` all live here, and none of them can reach any other dimension —
/// the sandbox, the root boundary and the broker are outside the process being
/// configured.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum NativeMode {
    /// The default (§4.1). APEX passes no permission flag and the agent's own
    /// profile decides. A Claude profile that sets `bypassPermissions` keeps
    /// it; APEX does not override a choice the user already made.
    #[default]
    Inherit,
    /// Force the agent to ask. Overrides a profile that would not have.
    Ask,
    /// §4.2's `--agent-bypass`: the agent stops asking for confirmations.
    /// Every APEX layer stays exactly where it was.
    Bypass,
}

impl NativeMode {
    /// Every value, for exhaustive tests and `--help` text.
    pub const ALL: &'static [NativeMode] =
        &[NativeMode::Inherit, NativeMode::Ask, NativeMode::Bypass];

    pub fn as_str(&self) -> &'static str {
        match self {
            NativeMode::Inherit => "inherit",
            NativeMode::Ask => "ask",
            NativeMode::Bypass => "bypass",
        }
    }

    pub fn parse(s: &str) -> Option<NativeMode> {
        match s {
            "inherit" | "default" => Some(NativeMode::Inherit),
            "ask" | "prompt" => Some(NativeMode::Ask),
            "bypass" | "bypasspermissions" | "bypass_permissions" => Some(NativeMode::Bypass),
            _ => None,
        }
    }
}

impl std::fmt::Display for NativeMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.pad(self.as_str())
    }
}

/// Dimension 3: the system/root capability layer.
///
/// §3.3: root is delegated, not inherited. A managed agent does not become
/// root because the user ran `sudo` an hour ago, because the agent is in native
/// bypass mode, because the sandbox is unrestricted, or because Remote Control
/// is driving it. Each of those is a different dimension, and none of them is
/// this one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SystemAccess {
    /// The default, and the only value with an implementation today. No root:
    /// a session that needs a system change files a structured request through
    /// `apex request` and a human approves it.
    #[default]
    None,
    /// §4.4's `--system-access`. A grant that is session-bound, capability-
    /// scoped, time-limited, authorised outside the agent's terminal, and not
    /// renewable by the agent. P0-007 issues it; until then this is refused.
    Session,
    /// §4.5's `--unsafe-everything`. Break-glass, deliberately not the same
    /// thing as [`SystemAccess::Session`]: local authentication, a short TTL,
    /// a red indicator, an audit record and automatic expiry. P0-006 builds
    /// that; until then this is refused.
    Unsafe,
}

impl SystemAccess {
    pub const ALL: &'static [SystemAccess] = &[
        SystemAccess::None,
        SystemAccess::Session,
        SystemAccess::Unsafe,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            SystemAccess::None => "none",
            SystemAccess::Session => "session",
            SystemAccess::Unsafe => "unsafe",
        }
    }

    pub fn parse(s: &str) -> Option<SystemAccess> {
        match s {
            "none" | "off" => Some(SystemAccess::None),
            "session" => Some(SystemAccess::Session),
            "unsafe" => Some(SystemAccess::Unsafe),
            _ => None,
        }
    }
}

impl std::fmt::Display for SystemAccess {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.pad(self.as_str())
    }
}

/// Dimension 4: the secret/cloud capability layer.
///
/// §3.2: secret values are capabilities, not environment variables. The broker
/// performs the operation and returns its result; the token stays in the
/// environment of a process the session cannot see.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SecretPolicy {
    /// The default. The session may ask the broker to perform a granted
    /// capability and never receives the credential.
    #[default]
    Brokered,
    /// Tighter than the default: the session may not call the broker at all.
    /// For a task with no business touching a credential.
    None,
    /// Raw values in the session environment. Refused: nothing implements it,
    /// §7's table denies raw secret reads from every origin including the
    /// local one, and §3.4 keeps the denial through break-glass. The variant
    /// exists so the dimension has a loose end for P0-002's owner-controlled
    /// path to attach to, and so the invariant test has something to assert a
    /// root grant does not set.
    Export,
}

impl SecretPolicy {
    pub const ALL: &'static [SecretPolicy] = &[
        SecretPolicy::Brokered,
        SecretPolicy::None,
        SecretPolicy::Export,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            SecretPolicy::Brokered => "brokered",
            SecretPolicy::None => "none",
            SecretPolicy::Export => "export",
        }
    }

    pub fn parse(s: &str) -> Option<SecretPolicy> {
        match s {
            "brokered" | "broker" => Some(SecretPolicy::Brokered),
            "none" | "off" => Some(SecretPolicy::None),
            "export" | "raw" => Some(SecretPolicy::Export),
            _ => None,
        }
    }

    /// Whether a session under this policy may call the broker.
    ///
    /// The daemon's enforcement point. Checked against the session resolved
    /// from peer credentials, never against anything the request carried.
    pub fn may_use_broker(&self) -> bool {
        matches!(self, SecretPolicy::Brokered | SecretPolicy::Export)
    }
}

impl std::fmt::Display for SecretPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.pad(self.as_str())
    }
}

/// Dimension 5: network policy.
///
/// Separate from the sandbox because "which files can this session reach" and
/// "which hosts can it reach" are different questions with different answers.
/// Today they are entangled in one place only: `strict` forces this dimension
/// to [`NetworkPolicy::Offline`], because that is what `strict` has always
/// meant. See [`AgentPolicy::effective_network`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum NetworkPolicy {
    /// The default. The session shares the host's network namespace.
    #[default]
    Open,
    /// Only the destinations a policy names. P0-008; refused until then.
    Allowlist,
    /// No direct egress; outbound work goes through an APEX broker that owns
    /// the credential and the destination. P0-008; refused until then.
    Brokered,
    /// No network at all. Enforced with `bwrap --unshare-net`, so it needs a
    /// confined sandbox — an unconfined session has no namespace to unshare,
    /// and asking for both is refused rather than silently granted.
    Offline,
}

impl NetworkPolicy {
    pub const ALL: &'static [NetworkPolicy] = &[
        NetworkPolicy::Open,
        NetworkPolicy::Allowlist,
        NetworkPolicy::Brokered,
        NetworkPolicy::Offline,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            NetworkPolicy::Open => "open",
            NetworkPolicy::Allowlist => "allowlist",
            NetworkPolicy::Brokered => "brokered",
            NetworkPolicy::Offline => "offline",
        }
    }

    pub fn parse(s: &str) -> Option<NetworkPolicy> {
        match s {
            "open" | "on" => Some(NetworkPolicy::Open),
            "allowlist" | "allow-list" => Some(NetworkPolicy::Allowlist),
            "brokered" | "broker" => Some(NetworkPolicy::Brokered),
            "offline" | "none" | "off" => Some(NetworkPolicy::Offline),
            _ => None,
        }
    }
}

impl std::fmt::Display for NetworkPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.pad(self.as_str())
    }
}

/// Dimension 6: remote-origin policy.
///
/// §7 treats Remote Control as a normal workflow, not an edge case — a remote
/// origin may edit the project, run tests and push. What it may not do by
/// default is elevate: "Root capability: local approval required. Unsafe
/// everything: local approval required."
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum OriginPolicy {
    /// The default. A request that would move the system or secret dimension
    /// has to come from a local origin, whichever origin drives the session.
    #[default]
    LocalElevationOnly,
    /// §7's opt-in: a remote origin may authorise elevation too. The roadmap
    /// allows this only behind WebAuthn/FIDO2, which P0-014 builds; refused
    /// until then.
    RemoteElevationAllowed,
}

impl OriginPolicy {
    pub const ALL: &'static [OriginPolicy] = &[
        OriginPolicy::LocalElevationOnly,
        OriginPolicy::RemoteElevationAllowed,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            OriginPolicy::LocalElevationOnly => "local_elevation_only",
            OriginPolicy::RemoteElevationAllowed => "remote_elevation_allowed",
        }
    }

    pub fn parse(s: &str) -> Option<OriginPolicy> {
        match s {
            "local_elevation_only" | "local-elevation-only" | "local" => {
                Some(OriginPolicy::LocalElevationOnly)
            }
            "remote_elevation_allowed" | "remote-elevation-allowed" | "remote" => {
                Some(OriginPolicy::RemoteElevationAllowed)
            }
            _ => None,
        }
    }

    /// Whether an elevation request from `origin` may be authorised at all.
    ///
    /// Answers only the origin question. Whether the elevation itself is
    /// available is [`AgentPolicy::validate`]'s business, and both have to
    /// pass.
    pub fn allows_elevation_from(&self, origin: RequestOrigin) -> bool {
        origin.is_local() || matches!(self, OriginPolicy::RemoteElevationAllowed)
    }
}

impl std::fmt::Display for OriginPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.pad(self.as_str())
    }
}

/// Where a privileged or capability request came from (§7's `request_origin`).
///
/// The vocabulary only. P0-013 attaches it to requests and records it in the
/// audit graph; what is here is the value set and the one question dimension 6
/// asks of it, so that task adds plumbing rather than a new type.
///
/// Not to be confused with `apex-agentd`'s internal `privilege::Origin`, which
/// answers "which session is this connection" and is resolved from peer
/// credentials.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum RequestOrigin {
    /// A shell the user is sitting in front of.
    #[default]
    LocalTerminal,
    /// The desktop shell's Agent Center.
    ApexShell,
    /// Claude Remote Control, driving a session from elsewhere.
    RemoteControl,
    /// A timer, with nobody present.
    ScheduledJob,
    /// An MCP server acting on the session's behalf.
    Mcp,
    /// A subagent of a managed session.
    Subagent,
    /// A cloud-side agent connector (§18's separate trust plane).
    CloudJob,
}

impl RequestOrigin {
    pub const ALL: &'static [RequestOrigin] = &[
        RequestOrigin::LocalTerminal,
        RequestOrigin::ApexShell,
        RequestOrigin::RemoteControl,
        RequestOrigin::ScheduledJob,
        RequestOrigin::Mcp,
        RequestOrigin::Subagent,
        RequestOrigin::CloudJob,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            RequestOrigin::LocalTerminal => "local-terminal",
            RequestOrigin::ApexShell => "apex-shell",
            RequestOrigin::RemoteControl => "claude-remote-control",
            RequestOrigin::ScheduledJob => "scheduled-job",
            RequestOrigin::Mcp => "mcp",
            RequestOrigin::Subagent => "subagent",
            RequestOrigin::CloudJob => "cloud-job",
        }
    }

    pub fn parse(s: &str) -> Option<RequestOrigin> {
        match s {
            "local-terminal" => Some(RequestOrigin::LocalTerminal),
            "apex-shell" => Some(RequestOrigin::ApexShell),
            "claude-remote-control" | "remote-control" => Some(RequestOrigin::RemoteControl),
            "scheduled-job" => Some(RequestOrigin::ScheduledJob),
            "mcp" => Some(RequestOrigin::Mcp),
            "subagent" => Some(RequestOrigin::Subagent),
            "cloud-job" => Some(RequestOrigin::CloudJob),
            _ => None,
        }
    }

    /// Whether a human is at this machine when the request arrives.
    ///
    /// A scheduled job is local by address and remote by every property that
    /// matters here: nobody is present to authenticate, so it counts as
    /// remote. So does a subagent, which is an agent asking on another agent's
    /// behalf, and an MCP server, which is a program the agent chose.
    pub fn is_local(&self) -> bool {
        matches!(
            self,
            RequestOrigin::LocalTerminal | RequestOrigin::ApexShell
        )
    }
}

impl std::fmt::Display for RequestOrigin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.pad(self.as_str())
    }
}

/// The six dimensions of §3.1, as six independent fields.
///
/// `Copy`, because it is passed through the daemon, the protocol, the sandbox
/// builder and the CLI, and a policy that has to be cloned invites a call site
/// that mutates a copy and enforces the original.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct AgentPolicy {
    /// Dimension 1. Set with `--native` / `--agent-bypass`.
    #[serde(default)]
    pub native: NativeMode,
    /// Dimension 2. Set with `--sandbox`. The field name is the pre-split wire
    /// key, kept so an older client's `{"sandbox":"strict"}` still lands here.
    #[serde(default)]
    pub sandbox: SandboxPolicy,
    /// Dimension 3. Set with `--system-access`.
    #[serde(default)]
    pub system: SystemAccess,
    /// Dimension 4. Set with `--secrets`.
    #[serde(default)]
    pub secrets: SecretPolicy,
    /// Dimension 5. Set with `--network`.
    #[serde(default)]
    pub network: NetworkPolicy,
    /// Dimension 6. Set with `--origin-policy`.
    #[serde(default)]
    pub origin: OriginPolicy,
}

impl AgentPolicy {
    /// The network this policy actually gets.
    ///
    /// The one derivation between dimensions, and it only tightens: `strict`
    /// has meant "confined with the network removed" since before the split
    /// and BASE-003 preserves that. Nothing here loosens the network — an
    /// `unrestricted` sandbox does not imply an open one.
    pub fn effective_network(&self) -> NetworkPolicy {
        match self.sandbox {
            SandboxPolicy::Strict => NetworkPolicy::Offline,
            _ => self.network,
        }
    }

    /// Whether the session runs with `PR_SET_NO_NEW_PRIVS`.
    ///
    /// True for everything except break-glass. §4.3 requires it of an
    /// unrestricted-user session "unless system-access mode explicitly changes
    /// it", and the tighter reading is taken: a capability-scoped session grant
    /// is brokered, so it has no need of a setuid binary inside the session,
    /// and handing one back would reinstate exactly the general-purpose root
    /// shell `request.rs` refuses to offer. P0-007 may loosen this, with a
    /// reason written down.
    ///
    /// A confined session gets this from `bwrap` already. It is the unconfined
    /// ones that need it set explicitly, and they are the ones §4.3 is about.
    pub fn no_new_privs(&self) -> bool {
        !matches!(self.system, SystemAccess::Unsafe)
    }

    /// The policy with every derivation applied, ready to be stored.
    ///
    /// The daemon records this rather than what was asked for, so a session
    /// listed as `strict` does not also report an open network it does not
    /// have. Enforcement still reads [`AgentPolicy::effective_network`]
    /// directly, so a record written by some future path that forgets to
    /// normalise cannot open the network back up.
    pub fn normalised(self) -> AgentPolicy {
        AgentPolicy {
            network: self.effective_network(),
            ..self
        }
    }

    /// Refuse any dimension this build cannot actually enforce.
    ///
    /// Called before a session starts, on the daemon side, so a client that
    /// skips its own checks gets the same answer. Deliberately not a check for
    /// contradictions between dimensions: an older client sends no network key
    /// at all, which deserialises to `open`, and treating that as a
    /// contradiction with `--sandbox strict` would break every existing
    /// caller. The CLI, which knows whether the user typed `--network`,
    /// rejects the explicit contradiction; this normalises it.
    pub fn validate(&self) -> Result<(), PolicyError> {
        let network = self.effective_network();
        match network {
            NetworkPolicy::Open => {}
            NetworkPolicy::Offline if self.sandbox.is_confined() => {}
            other => return Err(PolicyError::NetworkUnenforceable(other)),
        }
        if self.system != SystemAccess::None {
            return Err(PolicyError::SystemAccessUnavailable(self.system));
        }
        if self.secrets == SecretPolicy::Export {
            return Err(PolicyError::SecretExportUnavailable);
        }
        if self.origin == OriginPolicy::RemoteElevationAllowed {
            return Err(PolicyError::RemoteElevationUnavailable);
        }
        Ok(())
    }

    /// The six dimensions as label and value, in §3.1's order.
    ///
    /// One place builds this, so `apex agent status`, the session listing and
    /// whatever the Agent Center grows cannot disagree about which dimensions
    /// exist or what they are called. `network` reports the effective value,
    /// which is what the session actually has.
    pub fn dimensions(&self) -> [(&'static str, &'static str); 6] {
        [
            ("native", self.native.as_str()),
            ("sandbox", self.sandbox.as_str()),
            ("system", self.system.as_str()),
            ("secrets", self.secrets.as_str()),
            ("network", self.effective_network().as_str()),
            ("origin", self.origin.as_str()),
        ]
    }
}

/// A policy this build will not run.
///
/// Every variant is a refusal to enforce, not a preference. Each message names
/// the task that will implement the value, because a user who asked for
/// `--network allowlist` needs to know it is unbuilt rather than misspelled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyError {
    /// A network mode with no enforcement point in this build.
    NetworkUnenforceable(NetworkPolicy),
    /// A system-access mode whose grant machinery does not exist.
    SystemAccessUnavailable(SystemAccess),
    /// Raw secret values in the session environment.
    SecretExportUnavailable,
    /// Elevation authorised from a remote origin.
    RemoteElevationUnavailable,
}

impl std::fmt::Display for PolicyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PolicyError::NetworkUnenforceable(NetworkPolicy::Offline) => write!(
                f,
                "enforcing an offline session takes a network namespace, and only a confined \
                 sandbox has one; use `--sandbox strict`, or `--sandbox project --network \
                 offline`"
            ),
            PolicyError::NetworkUnenforceable(mode) => write!(
                f,
                "the {mode} network mode is not enforced by this build, and running with an \
                 open network instead would report a restriction that is not there; use \
                 `--network open` or `--network offline`"
            ),
            PolicyError::SystemAccessUnavailable(mode) => write!(
                f,
                "`--system-access {mode}` has no grant behind it in this build: the \
                 authentication, the time limit and the audit record it depends on are not \
                 written yet, and a session that reported system access without them would be \
                 claiming a boundary it does not have. Ask for the operation with \
                 `apex request` instead"
            ),
            PolicyError::SecretExportUnavailable => write!(
                f,
                "raw secret values are never placed in a session's environment: the broker \
                 performs the operation and returns its result. Use `apex secret grant` to \
                 allow a capability"
            ),
            PolicyError::RemoteElevationUnavailable => write!(
                f,
                "elevation authorised from a remote origin needs the hardware authentication \
                 this build does not have; approve it locally instead"
            ),
        }
    }
}

impl std::error::Error for PolicyError {}

/// §4's named modes, each a point in the six-dimensional space.
///
/// A preset is applied first and individual `--` flags are applied on top, so
/// every combination stays reachable and the four the roadmap names are one
/// word.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyPreset {
    /// §4.1. What `a` and an unqualified `apex agent run` get: every dimension
    /// at its default, and the agent's own permission mode left alone.
    Default,
    /// §4.2 `--agent-bypass`. The recommended high-autonomy mode: native
    /// confirmations off, project sandbox on, secret broker on, root boundary
    /// on, audit on.
    AgentBypass,
    /// §4.3 `--sandbox unrestricted`. Full user-level filesystem and process
    /// access, still no automatic root, still brokered secrets, `no_new_privs`
    /// still on.
    Unrestricted,
    /// §4.4 `--agent-bypass --sandbox unrestricted --system-access`.
    UnsafeSystemAccess,
    /// §4.5 `--unsafe-everything`. Break-glass, and deliberately not the same
    /// as [`PolicyPreset::UnsafeSystemAccess`]. Note the secret dimension: §3.4
    /// keeps the broker even here.
    UnsafeEverything,
}

impl PolicyPreset {
    pub const ALL: &'static [PolicyPreset] = &[
        PolicyPreset::Default,
        PolicyPreset::AgentBypass,
        PolicyPreset::Unrestricted,
        PolicyPreset::UnsafeSystemAccess,
        PolicyPreset::UnsafeEverything,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            PolicyPreset::Default => "default",
            PolicyPreset::AgentBypass => "agent-bypass",
            PolicyPreset::Unrestricted => "unrestricted",
            PolicyPreset::UnsafeSystemAccess => "system-access",
            PolicyPreset::UnsafeEverything => "unsafe-everything",
        }
    }

    pub fn parse(s: &str) -> Option<PolicyPreset> {
        match s {
            "default" => Some(PolicyPreset::Default),
            "agent-bypass" | "agent_bypass" => Some(PolicyPreset::AgentBypass),
            "unrestricted" => Some(PolicyPreset::Unrestricted),
            "system-access" | "system_access" => Some(PolicyPreset::UnsafeSystemAccess),
            "unsafe-everything" | "unsafe_everything" => Some(PolicyPreset::UnsafeEverything),
            _ => None,
        }
    }

    /// The six coordinates of this mode.
    ///
    /// Written out per preset rather than built by mutating the one above it,
    /// so a change to one mode cannot silently move another and so each line
    /// can be read against §4.
    pub fn policy(&self) -> AgentPolicy {
        match self {
            PolicyPreset::Default => AgentPolicy::default(),
            PolicyPreset::AgentBypass => AgentPolicy {
                native: NativeMode::Bypass,
                sandbox: SandboxPolicy::Project,
                system: SystemAccess::None,
                secrets: SecretPolicy::Brokered,
                network: NetworkPolicy::Open,
                origin: OriginPolicy::LocalElevationOnly,
            },
            PolicyPreset::Unrestricted => AgentPolicy {
                native: NativeMode::Inherit,
                sandbox: SandboxPolicy::Unrestricted,
                system: SystemAccess::None,
                secrets: SecretPolicy::Brokered,
                network: NetworkPolicy::Open,
                origin: OriginPolicy::LocalElevationOnly,
            },
            PolicyPreset::UnsafeSystemAccess => AgentPolicy {
                native: NativeMode::Bypass,
                sandbox: SandboxPolicy::Unrestricted,
                system: SystemAccess::Session,
                secrets: SecretPolicy::Brokered,
                network: NetworkPolicy::Open,
                origin: OriginPolicy::LocalElevationOnly,
            },
            PolicyPreset::UnsafeEverything => AgentPolicy {
                native: NativeMode::Bypass,
                sandbox: SandboxPolicy::Unrestricted,
                system: SystemAccess::Unsafe,
                // §3.4, verbatim: "broker secrets are still not conveniently
                // dumped into the agent environment". Break-glass removes the
                // APEX protections the owner asked it to remove; it does not
                // hand over credentials nothing in the build ever hands over.
                secrets: SecretPolicy::Brokered,
                network: NetworkPolicy::Open,
                // §7's table: unsafe-everything is "local auth" locally and
                // "local approval required" from Remote Control. Break-glass
                // does not become remotely authorisable by being break-glass.
                origin: OriginPolicy::LocalElevationOnly,
            },
        }
    }
}

impl std::fmt::Display for PolicyPreset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.pad(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_dimension_defaults_to_its_safe_value() {
        // A default that fails open would make every unqualified `apex agent
        // run` an unprotected one, and nobody would notice until it mattered.
        let p = AgentPolicy::default();
        assert_eq!(p.native, NativeMode::Inherit);
        assert_eq!(p.sandbox, SandboxPolicy::Project);
        assert_eq!(p.system, SystemAccess::None);
        assert_eq!(p.secrets, SecretPolicy::Brokered);
        assert_eq!(p.network, NetworkPolicy::Open);
        assert_eq!(p.origin, OriginPolicy::LocalElevationOnly);
        assert!(p.sandbox.is_confined());
        assert!(p.no_new_privs());
        assert_eq!(p.validate(), Ok(()));
    }

    #[test]
    fn every_dimension_name_round_trips() {
        for v in NativeMode::ALL {
            assert_eq!(NativeMode::parse(v.as_str()), Some(*v), "{v}");
        }
        for v in SystemAccess::ALL {
            assert_eq!(SystemAccess::parse(v.as_str()), Some(*v), "{v}");
        }
        for v in SecretPolicy::ALL {
            assert_eq!(SecretPolicy::parse(v.as_str()), Some(*v), "{v}");
        }
        for v in NetworkPolicy::ALL {
            assert_eq!(NetworkPolicy::parse(v.as_str()), Some(*v), "{v}");
        }
        for v in OriginPolicy::ALL {
            assert_eq!(OriginPolicy::parse(v.as_str()), Some(*v), "{v}");
        }
        for v in RequestOrigin::ALL {
            assert_eq!(RequestOrigin::parse(v.as_str()), Some(*v), "{v}");
        }
        for v in PolicyPreset::ALL {
            assert_eq!(PolicyPreset::parse(v.as_str()), Some(*v), "{v}");
        }
    }

    #[test]
    fn an_unknown_dimension_value_is_rejected_not_defaulted() {
        // A typo must not silently select the loosest thing that parses.
        assert_eq!(NativeMode::parse("yolo"), None);
        assert_eq!(SystemAccess::parse("root"), None);
        assert_eq!(SecretPolicy::parse("plaintext"), None);
        assert_eq!(NetworkPolicy::parse("allow"), None);
        assert_eq!(OriginPolicy::parse("anywhere"), None);
        assert_eq!(PolicyPreset::parse("yolo"), None);
    }

    #[test]
    fn the_dimension_names_are_stable_and_distinct() {
        // These reach the wire, `apex agent status` and the Agent Center. Two
        // dimensions reporting the same label would make a listing unreadable.
        let names: Vec<&str> = AgentPolicy::default()
            .dimensions()
            .iter()
            .map(|(n, _)| *n)
            .collect();
        assert_eq!(
            names,
            vec!["native", "sandbox", "system", "secrets", "network", "origin"]
        );
    }

    #[test]
    fn a_policy_survives_the_wire_with_every_dimension_intact() {
        let p = AgentPolicy {
            native: NativeMode::Bypass,
            sandbox: SandboxPolicy::Strict,
            system: SystemAccess::Session,
            secrets: SecretPolicy::None,
            network: NetworkPolicy::Allowlist,
            origin: OriginPolicy::RemoteElevationAllowed,
        };
        let text = serde_json::to_string(&p).expect("serialise");
        assert_eq!(serde_json::from_str::<AgentPolicy>(&text).unwrap(), p);
        // And the keys are the ones the CLI and the shell read.
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        for key in ["native", "sandbox", "system", "secrets", "network", "origin"] {
            assert!(v.get(key).is_some(), "{key} missing from {text}");
        }
    }

    #[test]
    fn a_policy_written_before_the_split_still_loads() {
        // The pre-split record carried one key. Every dimension the older
        // writer did not know about must come back at its default, not fail
        // the parse and not come back loose.
        let p: AgentPolicy = serde_json::from_str(r#"{"sandbox":"strict"}"#).expect("parse");
        assert_eq!(p.sandbox, SandboxPolicy::Strict);
        assert_eq!(p, AgentPolicy { sandbox: SandboxPolicy::Strict, ..AgentPolicy::default() });

        let empty: AgentPolicy = serde_json::from_str("{}").expect("parse");
        assert_eq!(empty, AgentPolicy::default());
    }

    #[test]
    fn strict_forces_the_network_off_and_nothing_forces_it_on() {
        // The floor: strict has always meant "no network", and a client that
        // sends no network key at all must still get that.
        for network in NetworkPolicy::ALL {
            let p = AgentPolicy {
                sandbox: SandboxPolicy::Strict,
                network: *network,
                ..AgentPolicy::default()
            };
            assert_eq!(
                p.effective_network(),
                NetworkPolicy::Offline,
                "strict must not carry {network}"
            );
        }
        // And it is a floor, not a ceiling: a looser sandbox does not open the
        // network back up.
        let p = AgentPolicy {
            sandbox: SandboxPolicy::Unrestricted,
            network: NetworkPolicy::Offline,
            ..AgentPolicy::default()
        };
        assert_eq!(p.effective_network(), NetworkPolicy::Offline);
    }

    #[test]
    fn normalising_records_the_network_the_session_really_has() {
        // Otherwise `apex agent status` on a strict session reports an open
        // network, which is a listing that lies about a security property.
        let p = AgentPolicy {
            sandbox: SandboxPolicy::Strict,
            ..AgentPolicy::default()
        }
        .normalised();
        assert_eq!(p.network, NetworkPolicy::Offline);
        assert_eq!(p.dimensions()[4], ("network", "offline"));
        // Normalising is idempotent and touches nothing else.
        assert_eq!(p.normalised(), p);
        assert_eq!(p.sandbox, SandboxPolicy::Strict);
        assert_eq!(p.native, NativeMode::Inherit);
    }

    #[test]
    fn an_offline_network_without_a_sandbox_to_enforce_it_is_refused() {
        // There is no namespace to unshare, so the session would run with the
        // network. Failing closed is the only answer that does not lie.
        let p = AgentPolicy {
            sandbox: SandboxPolicy::Unrestricted,
            network: NetworkPolicy::Offline,
            ..AgentPolicy::default()
        };
        assert_eq!(
            p.validate(),
            Err(PolicyError::NetworkUnenforceable(NetworkPolicy::Offline))
        );
        // The confined forms are accepted, both spellings of the same thing.
        for sandbox in [SandboxPolicy::Project, SandboxPolicy::Strict] {
            let p = AgentPolicy {
                sandbox,
                network: NetworkPolicy::Offline,
                ..AgentPolicy::default()
            };
            assert_eq!(p.validate(), Ok(()), "{sandbox}");
        }
    }

    #[test]
    fn unbuilt_dimension_values_are_refused_and_name_their_remedy() {
        let cases: Vec<(AgentPolicy, PolicyError)> = vec![
            (
                AgentPolicy { network: NetworkPolicy::Allowlist, ..Default::default() },
                PolicyError::NetworkUnenforceable(NetworkPolicy::Allowlist),
            ),
            (
                AgentPolicy { network: NetworkPolicy::Brokered, ..Default::default() },
                PolicyError::NetworkUnenforceable(NetworkPolicy::Brokered),
            ),
            (
                AgentPolicy { system: SystemAccess::Session, ..Default::default() },
                PolicyError::SystemAccessUnavailable(SystemAccess::Session),
            ),
            (
                AgentPolicy { system: SystemAccess::Unsafe, ..Default::default() },
                PolicyError::SystemAccessUnavailable(SystemAccess::Unsafe),
            ),
            (
                AgentPolicy { secrets: SecretPolicy::Export, ..Default::default() },
                PolicyError::SecretExportUnavailable,
            ),
            (
                AgentPolicy { origin: OriginPolicy::RemoteElevationAllowed, ..Default::default() },
                PolicyError::RemoteElevationUnavailable,
            ),
        ];
        for (policy, want) in cases {
            assert_eq!(policy.validate(), Err(want), "{policy:?}");
            let msg = want.to_string();
            assert!(msg.len() > 40, "unhelpful refusal: {msg}");
        }
    }

    #[test]
    fn the_values_with_an_enforcement_point_are_accepted() {
        // The other half of the fail-closed test: refusing everything would
        // also pass the test above.
        for p in [
            AgentPolicy::default(),
            AgentPolicy { native: NativeMode::Bypass, ..Default::default() },
            AgentPolicy { native: NativeMode::Ask, ..Default::default() },
            AgentPolicy { sandbox: SandboxPolicy::Unrestricted, ..Default::default() },
            AgentPolicy { sandbox: SandboxPolicy::Strict, ..Default::default() },
            AgentPolicy { secrets: SecretPolicy::None, ..Default::default() },
            AgentPolicy { network: NetworkPolicy::Offline, ..Default::default() },
        ] {
            assert_eq!(p.validate(), Ok(()), "{p:?}");
        }
    }

    #[test]
    fn presets_are_exactly_the_modes_section_4_describes() {
        // Read against ROADMAP.md §4.1–4.5. Each assertion is one line of that
        // section; a preset that drifts from it fails here rather than in a
        // user's session.
        assert_eq!(PolicyPreset::Default.policy(), AgentPolicy::default());

        // §4.2: "agent-native confirmations OFF, APEX project sandbox ON,
        // APEX secret broker ON, APEX root boundary ON, APEX audit ON."
        let bypass = PolicyPreset::AgentBypass.policy();
        assert_eq!(bypass.native, NativeMode::Bypass);
        assert_eq!(bypass.sandbox, SandboxPolicy::Project);
        assert!(bypass.sandbox.is_confined());
        assert_eq!(bypass.secrets, SecretPolicy::Brokered);
        assert_eq!(bypass.system, SystemAccess::None);
        assert_eq!(bypass.validate(), Ok(()));

        // §4.3: "no project filesystem sandbox; still no automatic root; still
        // brokered secrets by default; no_new_privs should remain active."
        let unres = PolicyPreset::Unrestricted.policy();
        assert_eq!(unres.sandbox, SandboxPolicy::Unrestricted);
        assert_eq!(unres.system, SystemAccess::None);
        assert_eq!(unres.secrets, SecretPolicy::Brokered);
        assert!(unres.no_new_privs());
        assert_eq!(unres.validate(), Ok(()));

        // §4.4's command line, dimension by dimension.
        let sys = PolicyPreset::UnsafeSystemAccess.policy();
        assert_eq!(sys.native, NativeMode::Bypass);
        assert_eq!(sys.sandbox, SandboxPolicy::Unrestricted);
        assert_eq!(sys.system, SystemAccess::Session);
        assert_eq!(sys.secrets, SecretPolicy::Brokered);

        // §4.5, and §3.4's last requirement.
        let breakglass = PolicyPreset::UnsafeEverything.policy();
        assert_eq!(breakglass.system, SystemAccess::Unsafe);
        assert_eq!(
            breakglass.secrets,
            SecretPolicy::Brokered,
            "§3.4: broker secrets are still not dumped into the agent environment"
        );
        assert_eq!(
            breakglass.origin,
            OriginPolicy::LocalElevationOnly,
            "§7: unsafe-everything requires local approval"
        );

        // The two elevated presets are refused until P0-006 and P0-007 land.
        assert!(sys.validate().is_err());
        assert!(breakglass.validate().is_err());
    }

    #[test]
    fn the_two_elevated_presets_are_not_the_same_policy() {
        // §4.5: "This is deliberately different from system-access mode." A
        // future edit that made one an alias of the other would erase the
        // distinction the roadmap asks for.
        assert_ne!(
            PolicyPreset::UnsafeSystemAccess.policy(),
            PolicyPreset::UnsafeEverything.policy()
        );
    }

    #[test]
    fn no_new_privs_is_on_for_everything_except_break_glass() {
        for system in SystemAccess::ALL {
            let p = AgentPolicy { system: *system, ..AgentPolicy::default() };
            assert_eq!(
                p.no_new_privs(),
                *system != SystemAccess::Unsafe,
                "{system}"
            );
        }
        // And no other dimension can switch it off — this is the kernel half
        // of "unrestricted-user does not imply root".
        for sandbox in [
            SandboxPolicy::Unrestricted,
            SandboxPolicy::Project,
            SandboxPolicy::Strict,
        ] {
            for native in NativeMode::ALL {
                let p = AgentPolicy { sandbox, native: *native, ..AgentPolicy::default() };
                assert!(p.no_new_privs(), "{sandbox} {native}");
            }
        }
    }

    #[test]
    fn only_the_broker_dimension_decides_whether_the_broker_is_reachable() {
        assert!(SecretPolicy::Brokered.may_use_broker());
        assert!(!SecretPolicy::None.may_use_broker());
        assert!(SecretPolicy::Export.may_use_broker());
    }

    #[test]
    fn a_remote_origin_cannot_elevate_unless_the_owner_allowed_it() {
        // §7's table: root capability and unsafe-everything both need local
        // approval, whichever origin is driving the session.
        let local_only = OriginPolicy::LocalElevationOnly;
        for origin in RequestOrigin::ALL {
            assert_eq!(
                local_only.allows_elevation_from(*origin),
                origin.is_local(),
                "{origin}"
            );
        }
        // The opt-in reverses it for every origin, which is why it needs the
        // hardware authentication §7 asks for and is refused until then.
        for origin in RequestOrigin::ALL {
            assert!(OriginPolicy::RemoteElevationAllowed.allows_elevation_from(*origin));
        }
    }

    #[test]
    fn an_unattended_origin_is_not_local_however_it_reached_the_socket() {
        // A scheduled job runs on this machine and a subagent is a child of a
        // local process, but in both cases nobody is present to authenticate,
        // which is the only property dimension 6 cares about.
        assert!(RequestOrigin::LocalTerminal.is_local());
        assert!(RequestOrigin::ApexShell.is_local());
        for remote in [
            RequestOrigin::RemoteControl,
            RequestOrigin::ScheduledJob,
            RequestOrigin::Mcp,
            RequestOrigin::Subagent,
            RequestOrigin::CloudJob,
        ] {
            assert!(!remote.is_local(), "{remote}");
        }
    }

    #[test]
    fn display_honours_column_width() {
        // Printed in aligned tables, like the other protocol enums.
        assert_eq!(format!("[{:<10}]", NativeMode::Bypass), "[bypass    ]");
        assert_eq!(format!("[{:>8}]", NetworkPolicy::Offline), "[ offline]");
    }
}
