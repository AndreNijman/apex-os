//! System-access grants: §4.4's session grant and §4.5's break-glass window.
//!
//! Dimension 3 has three values and P0-004 shipped one of them. `none` is the
//! default and needs nothing behind it; `session` and `unsafe` are the two
//! §3.3 describes as "explicit, scoped, time-limited, auditable, and bound to
//! a concrete agent session", and until now `AgentPolicy::validate` refused
//! both because there was nothing that could be any of those things. This
//! module is that thing.
//!
//! ## What a grant is, and where its authority lives
//!
//! A [`SystemGrant`] on disk is **history, not authority**. `apex-agentd` runs
//! as the user, and a session running `--sandbox unrestricted` — which every
//! break-glass session is, by definition — is also the user. Anything the
//! daemon can write under `$XDG_STATE_HOME`, that session can write too. A
//! grant file is therefore something the subject of the grant can forge, and
//! treating one as permission would mean an agent could mint itself root by
//! writing a JSON file.
//!
//! So the authority is the daemon's own memory: a grant counts only while the
//! process that minted it, after a successful authentication, is still holding
//! it. The store exists so that `apex agent grants` can explain what happened,
//! so the next boot can say what ended, and so the record survives for a human
//! to read. Nothing reads it back as permission.
//!
//! The same reasoning applies to the JSONL audit trail, which is why every
//! event here is also mirrored to the journal ([`crate::journal`]). A user
//! process may append to the journal; it cannot alter or remove what is
//! already there. That is what makes §3.4's "permanent audit record" true
//! rather than aspirational.
//!
//! ## Restart and reboot are the same fact, seen twice
//!
//! Because authority is process memory, a daemon restart drops every grant.
//! That is fail-closed and deliberate: re-adopting a grant from disk after a
//! restart would be exactly the "read the forgeable file back as permission"
//! move above.
//!
//! §3.4's "no silent persistence after reboot" asks for more than that. A
//! grant that merely vanishes is also wrong, because the owner who authorised
//! fifteen minutes of break-glass and then rebooted has no way to tell whether
//! the window is still open. So a grant is stamped with the boot it was issued
//! under, [`SystemGrant::state_at`] reports [`ClosureReason::Reboot`] when
//! that stamp does not match the running kernel's, and the daemon says so —
//! once, in the audit trail and in `apex agent grants` — the next time it
//! starts. The machine answers the question rather than losing it.
//!
//! The distinction between "expired on its own" and "ended at the reboot" is
//! kept rather than collapsed, because they are different things to have
//! happened and the owner may be trying to work out which. A grant whose TTL
//! ran out before the machine went down expired; one that was still live when
//! it went down ended at the reboot. `/proc/stat`'s `btime` is what separates
//! them, and it is read once into a [`BootStamp`].

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::policy::{RequestOrigin, SystemAccess};

/// Which of §4's two elevated modes a grant backs.
///
/// One enum rather than reusing [`SystemAccess`] directly, because
/// `SystemAccess::None` is not a grant and a type whose value set includes
/// "no grant" invites a call site that forgets to check for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GrantKind {
    /// §4.4. Capability-scoped, brokered, `no_new_privs` still on. The session
    /// does not become root; named operations it files stop needing a separate
    /// human decision for the life of the grant.
    SystemAccess,
    /// §4.5. Break-glass: `no_new_privs` off, so setuid binaries work and the
    /// session can genuinely become root. Deliberately a different thing.
    BreakGlass,
}

impl GrantKind {
    pub const ALL: &'static [GrantKind] = &[GrantKind::SystemAccess, GrantKind::BreakGlass];

    /// The dimension-3 value this grant backs.
    pub fn system_access(&self) -> SystemAccess {
        match self {
            GrantKind::SystemAccess => SystemAccess::Session,
            GrantKind::BreakGlass => SystemAccess::Unsafe,
        }
    }

    /// The grant a dimension-3 value needs, or `None` for the default.
    pub fn for_system_access(system: SystemAccess) -> Option<GrantKind> {
        match system {
            SystemAccess::None => None,
            SystemAccess::Session => Some(GrantKind::SystemAccess),
            SystemAccess::Unsafe => Some(GrantKind::BreakGlass),
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            GrantKind::SystemAccess => "system-access",
            GrantKind::BreakGlass => "break-glass",
        }
    }

    /// Whether expiry can only be enforced by ending the session.
    ///
    /// True for break-glass and false for the session grant, and the
    /// difference is a kernel fact rather than a preference.
    /// `PR_SET_NO_NEW_PRIVS` is set once, between `fork` and `exec`, and a
    /// process cannot clear it — so neither can APEX. A break-glass session
    /// that outlived its TTL would still be able to use `sudo`, whatever a
    /// record said. §3.4 asks for automatic expiry, and for this mode the
    /// only honest implementation of that is to end the session.
    ///
    /// A session grant needs none of that: it is capability-scoped and lives
    /// entirely in what the daemon will do when the session next asks, so
    /// expiry is simply the daemon stopping.
    pub fn expiry_ends_the_session(&self) -> bool {
        matches!(self, GrantKind::BreakGlass)
    }
}

impl std::fmt::Display for GrantKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.pad(self.as_str())
    }
}

// ── the TTL ─────────────────────────────────────────────────────────────────

/// The longest break-glass window this build will issue.
///
/// §3.4 asks for an "explicit short TTL" and for no "remember forever". An
/// hour is the outer edge of short for a mode that turns off the root
/// boundary; the roadmap's own example is fifteen minutes.
pub const MAX_BREAK_GLASS_MS: u64 = 60 * 60 * 1000;

/// The longest session grant this build will issue.
///
/// Looser than break-glass because it is a smaller thing: the session stays
/// under `no_new_privs`, the grant is capability-scoped, and every operation
/// it covers still runs through `apex request`. A working day is the bound.
pub const MAX_SESSION_ACCESS_MS: u64 = 8 * 60 * 60 * 1000;

/// What `--system-access session` gets when no `--ttl` was given.
///
/// Break-glass has no equivalent on purpose: §3.4 says *explicit*, and a
/// default TTL is the opposite of explicit. [`ttl_for`] refuses it.
pub const DEFAULT_SESSION_ACCESS_MS: u64 = 30 * 60 * 1000;

/// Break-glass is the shorter window, and its default is the one that does not
/// exist. Both are facts about constants, so they are checked when the crate
/// compiles rather than when a test runs — the same form `protocol.rs` uses for
/// its version guards. A build in which break-glass had become the *looser*
/// mode would be one where §4.5 had quietly turned into §4.4.
const _: () = assert!(MAX_BREAK_GLASS_MS < MAX_SESSION_ACCESS_MS);
const _: () = assert!(DEFAULT_SESSION_ACCESS_MS <= MAX_SESSION_ACCESS_MS);

/// A TTL this build will not issue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TtlError {
    /// No unit, or a unit that is not one of `s`, `m`, `h`.
    Unparseable(String),
    /// Zero, which is a grant that has already expired.
    Zero,
    /// Longer than the mode's cap.
    TooLong { asked_ms: u64, cap_ms: u64, kind: GrantKind },
    /// Break-glass with no `--ttl` at all.
    BreakGlassNeedsTtl,
}

impl std::fmt::Display for TtlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TtlError::Unparseable(s) => write!(
                f,
                "'{s}' is not a duration; write it as a number and a unit, like 15m, 90s or 1h"
            ),
            TtlError::Zero => write!(
                f,
                "a zero TTL is a grant that has already expired; ask for the time you actually \
                 need, or do not ask for the mode"
            ),
            TtlError::TooLong { asked_ms, cap_ms, kind } => write!(
                f,
                "{} caps at {}, and {} was asked for. §3.4 asks for an explicit short window and \
                 no way to remember forever; ask again when it runs out",
                kind,
                format_ms(*cap_ms),
                format_ms(*asked_ms)
            ),
            TtlError::BreakGlassNeedsTtl => write!(
                f,
                "`--unsafe-everything` takes the APEX root boundary off, so §3.4 requires the \
                 window to be stated rather than defaulted; add `--ttl 15m`"
            ),
        }
    }
}

impl std::error::Error for TtlError {}

/// Parse `15m`, `90s`, `1h`.
///
/// A bare number is refused rather than assumed to be seconds or minutes. The
/// value is a security window and the two readings differ by sixty times.
pub fn parse_ttl(s: &str) -> Result<u64, TtlError> {
    let s = s.trim();
    // Split on the last *character*, not the last byte: `s.split_at(len - 1)`
    // panics on a multi-byte suffix, and "15µ" is a typo a user can make.
    let (digits, unit) = match s.char_indices().next_back() {
        Some((at, _)) => s.split_at(at),
        None => return Err(TtlError::Unparseable(s.to_string())),
    };
    let multiplier = match unit {
        "s" => 1_000,
        "m" => 60 * 1_000,
        "h" => 60 * 60 * 1_000,
        _ => return Err(TtlError::Unparseable(s.to_string())),
    };
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(TtlError::Unparseable(s.to_string()));
    }
    let n: u64 = digits.parse().map_err(|_| TtlError::Unparseable(s.to_string()))?;
    n.checked_mul(multiplier)
        .ok_or_else(|| TtlError::Unparseable(s.to_string()))
}

/// The TTL a grant of `kind` gets from an optional `--ttl`.
///
/// The one place the two modes' rules are written down, so the CLI and the
/// daemon cannot disagree about whether a missing `--ttl` is an error.
pub fn ttl_for(kind: GrantKind, asked_ms: Option<u64>) -> Result<u64, TtlError> {
    let cap = match kind {
        GrantKind::BreakGlass => MAX_BREAK_GLASS_MS,
        GrantKind::SystemAccess => MAX_SESSION_ACCESS_MS,
    };
    let ms = match (kind, asked_ms) {
        (GrantKind::BreakGlass, None) => return Err(TtlError::BreakGlassNeedsTtl),
        (GrantKind::SystemAccess, None) => DEFAULT_SESSION_ACCESS_MS,
        (_, Some(ms)) => ms,
    };
    if ms == 0 {
        return Err(TtlError::Zero);
    }
    if ms > cap {
        return Err(TtlError::TooLong { asked_ms: ms, cap_ms: cap, kind });
    }
    Ok(ms)
}

/// A capability list this build will not issue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CapabilityError {
    /// A name that is not one of [`crate::request::Verb::names`].
    NotAVerb(String),
    /// `--capabilities` with nothing in it.
    Empty,
    /// `--capabilities` on a break-glass grant, which does not go through
    /// `apex request` at all.
    NotScopable,
}

impl std::fmt::Display for CapabilityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CapabilityError::NotAVerb(name) => write!(
                f,
                "'{name}' is not a privilege verb, so a grant naming it would cover nothing; \
                 the verbs are {}",
                crate::request::Verb::names().join(", ")
            ),
            CapabilityError::Empty => write!(
                f,
                "a grant covering no verb authorises nothing; name the verbs the session needs, \
                 or drop `--capabilities` to cover all of them"
            ),
            CapabilityError::NotScopable => write!(
                f,
                "`--unsafe-everything` does not go through `apex request`, so there are no verbs \
                 for `--capabilities` to narrow; it clears no_new_privs and the session uses \
                 sudo directly"
            ),
        }
    }
}

impl std::error::Error for CapabilityError {}

/// Which privilege verbs a grant about to be issued covers.
///
/// The companion of [`ttl_for`], and here for the same reason: the CLI is not
/// the only thing that can send a [`crate::protocol::RunRequest`], so the rule
/// that decides what a grant covers has to live where the daemon applies it to
/// a client that never saw a flag.
///
/// `None` — no `--capabilities` — is the whole privilege vocabulary, which is
/// what every session grant covered before the flag existed. The names are
/// resolved *here*, at issue time, rather than stored as "all": a verb added
/// tomorrow is not covered by a grant issued today, and that property is the
/// reason this returns a list and never a wildcard.
///
/// A break-glass grant is refused a list rather than given an empty one. Its
/// `capabilities` is empty because break-glass does not go through
/// `apex request` — accepting `--capabilities` there would be a flag that
/// narrows nothing while reading as though it did.
pub fn capabilities_for(
    kind: GrantKind,
    asked: Option<&[String]>,
) -> Result<Vec<String>, CapabilityError> {
    match (kind, asked) {
        (GrantKind::BreakGlass, Some(_)) => Err(CapabilityError::NotScopable),
        (GrantKind::BreakGlass, None) => Ok(Vec::new()),
        (GrantKind::SystemAccess, None) => Ok(normalise_capabilities(
            crate::request::Verb::names().iter().copied(),
        )),
        (GrantKind::SystemAccess, Some(names)) => {
            let caps = normalise_capabilities(names.iter().map(|s| s.as_str()));
            if caps.is_empty() {
                return Err(CapabilityError::Empty);
            }
            // Checked against the vocabulary rather than normalised away. A
            // misspelled verb silently dropped would hand back a grant that
            // covers less than the caller asked for, and the first thing they
            // would learn about it is a refusal in the middle of a session.
            for c in &caps {
                if !crate::request::Verb::names().contains(&c.as_str()) {
                    return Err(CapabilityError::NotAVerb(c.clone()));
                }
            }
            Ok(caps)
        }
    }
}

/// A duration as a human writes it back: `15m`, `1h 30m`, `45s`.
pub fn format_ms(ms: u64) -> String {
    let secs = ms / 1000;
    let (h, m, s) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    let mut parts = Vec::new();
    if h > 0 {
        parts.push(format!("{h}h"));
    }
    if m > 0 {
        parts.push(format!("{m}m"));
    }
    if s > 0 || parts.is_empty() {
        parts.push(format!("{s}s"));
    }
    parts.join(" ")
}

// ── the boot stamp ──────────────────────────────────────────────────────────

/// The kernel's identity for the running boot, and when it started.
///
/// Read once and passed in, so [`SystemGrant::state_at`] stays a pure function
/// of its arguments and the reboot rule can be tested without rebooting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootStamp {
    /// `/proc/sys/kernel/random/boot_id`. New on every boot, and not writable
    /// by anything in userspace.
    pub id: String,
    /// Unix milliseconds at which this boot started, from `/proc/stat`'s
    /// `btime`. Zero when it could not be read, which makes every previous
    /// boot's grant read as [`GrantState::EndedAtReboot`] — the conservative
    /// answer, since it never claims a grant expired on its own when it might
    /// have been cut short.
    pub booted_ms: u64,
}

impl BootStamp {
    /// Read the running kernel's stamp.
    pub fn current() -> BootStamp {
        BootStamp {
            id: read_boot_id().unwrap_or_default(),
            booted_ms: read_boot_time_ms().unwrap_or(0),
        }
    }
}

fn read_boot_id() -> Option<String> {
    let text = std::fs::read_to_string("/proc/sys/kernel/random/boot_id").ok()?;
    let id = text.trim().to_string();
    (!id.is_empty()).then_some(id)
}

/// Unix milliseconds of the current boot, from `/proc/stat`'s `btime` line.
///
/// `btime` and not `uptime`: uptime is a duration that has to be subtracted
/// from a clock reading, and the clock moves — an NTP step between issuing a
/// grant and reading it back would move the computed boot time with it.
/// `btime` is the boot's wall-clock timestamp as the kernel recorded it.
fn read_boot_time_ms() -> Option<u64> {
    let text = std::fs::read_to_string("/proc/stat").ok()?;
    parse_btime(&text)
}

fn parse_btime(stat: &str) -> Option<u64> {
    stat.lines()
        .find_map(|l| l.strip_prefix("btime "))
        .and_then(|s| s.trim().parse::<u64>().ok())
        .and_then(|secs| secs.checked_mul(1000))
}

// ── the grant ───────────────────────────────────────────────────────────────

/// One issued system-access grant.
///
/// Every field is a criterion of P0-006 or P0-007 rather than a convenience:
/// `session` is "session-bound", `expires_ms` is "time-limited", `capabilities`
/// is "capability-scoped", `boot_id` is "does not silently persist across
/// reboot", and the whole record written to the audit trail is "permanent
/// audit record".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemGrant {
    pub id: u32,
    pub kind: GrantKind,
    /// The session this grant belongs to. A grant is never issued without
    /// one: §3.3 requires it to be "bound to a concrete agent session", and a
    /// grant that outlives its session is a standing root capability.
    pub session: u32,
    pub agent: String,
    pub project: Option<String>,
    /// The privilege verbs this grant covers, by name.
    ///
    /// Sorted and de-duplicated, so two grants covering the same operations
    /// compare equal and an audit line reads the same way twice. Empty is a
    /// legal value and means "no verb is pre-approved" — which is what
    /// break-glass carries, because break-glass does not work through
    /// [`crate::request`] at all.
    pub capabilities: Vec<String>,
    pub issued_ms: u64,
    pub expires_ms: u64,
    /// The boot this was issued under. See the module docs.
    pub boot_id: String,
    /// Where the session that holds this grant is driven from, at the moment
    /// the grant was issued. Recorded because §7 gives root a different answer
    /// per column and an audit line that omits it answers half the question.
    pub request_origin: RequestOrigin,
    /// How the human proved they were present — the polkit action id that was
    /// satisfied.
    pub authenticated_by: String,
    /// How this grant ended, once APEX has observed and recorded that it did.
    ///
    /// Distinct from [`SystemGrant::expires_ms`], which is only when it was
    /// *due* to end. A grant can stop applying for four different reasons and
    /// they are not interchangeable to somebody reading the trail afterwards,
    /// so the reason is stored rather than re-derived: by the time anybody
    /// looks, the clock has moved past all of them and every ending would read
    /// as "expired".
    ///
    /// It is also what makes the audit line get written exactly once. The
    /// first sweep that sees an ended grant with no closure records it and
    /// stamps this.
    #[serde(default)]
    pub closed: Option<GrantClosure>,
}

/// How a grant ended, and when APEX recorded that.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct GrantClosure {
    pub ms: u64,
    pub why: ClosureReason,
}

/// The four ways a grant stops applying.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClosureReason {
    /// A human took it back.
    Revoked,
    /// Its TTL ran out. §3.4's "automatic expiry".
    Expired,
    /// It was still live when the machine went down. §3.4's "no silent
    /// persistence after reboot".
    Reboot,
    /// `apex-agentd` restarted while it was live.
    ///
    /// A grant's authority is the daemon's memory, so a restart ends every
    /// grant. That is fail-closed and deliberate — re-adopting a grant from a
    /// file the granted session can write would be reading permission out of
    /// something its own subject controls. It is recorded as its own reason
    /// because "the runtime restarted" and "your fifteen minutes were up" are
    /// different things to have happened.
    RuntimeRestart,
}

impl ClosureReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            ClosureReason::Revoked => "revoked",
            ClosureReason::Expired => "expired",
            ClosureReason::Reboot => "ended-at-reboot",
            ClosureReason::RuntimeRestart => "ended-with-the-runtime",
        }
    }
}

impl std::fmt::Display for ClosureReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.pad(self.as_str())
    }
}

/// Where a grant stands, at a given moment on a given boot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GrantState {
    /// In force, with this long left.
    Active { remaining_ms: u64 },
    /// Over, for this reason, at this moment.
    ///
    /// One variant rather than four, because every caller either wants the
    /// reason — and gets it as a value it can print or match on — or only
    /// wants to know that the grant is not active. Four variants meant four
    /// arms in every renderer and an easy fifth to forget.
    Ended { why: ClosureReason, at_ms: u64 },
}

impl GrantState {
    pub fn is_active(&self) -> bool {
        matches!(self, GrantState::Active { .. })
    }

    /// The reason, when it has ended.
    pub fn reason(&self) -> Option<ClosureReason> {
        match self {
            GrantState::Active { .. } => None,
            GrantState::Ended { why, .. } => Some(*why),
        }
    }

    /// The word `apex agent grants` prints in its STATE column.
    pub fn as_str(&self) -> &'static str {
        match self {
            GrantState::Active { .. } => "active",
            GrantState::Ended { why, .. } => why.as_str(),
        }
    }
}

impl std::fmt::Display for GrantState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.pad(self.as_str())
    }
}

impl SystemGrant {
    /// Where this grant stands.
    ///
    /// Pure, over two arguments the caller reads once. The order of the checks
    /// is the order the things could have happened in, and it matters:
    ///
    /// 1. **an ending already recorded** first. Once APEX has written down how
    ///    a grant finished, that is the answer forever — otherwise every
    ///    ending would read as "expired" a day later, when the clock has moved
    ///    past all of them, and the trail would lose the only part of it
    ///    somebody reads it for.
    /// 2. **a boot that is not this one** next, and split by `btime` — a grant
    ///    whose TTL ran out *before* the machine went down expired on its own,
    ///    and saying it "ended at the reboot" would misdescribe it;
    /// 3. **expiry** against the clock;
    /// 4. otherwise active.
    ///
    /// Note what is not here: a daemon restart. This function cannot see one,
    /// so the sweep that runs at startup is what records
    /// [`ClosureReason::RuntimeRestart`], and rule 1 is what makes it stick.
    pub fn state_at(&self, now_ms: u64, boot: &BootStamp) -> GrantState {
        if let Some(closed) = self.closed {
            return GrantState::Ended {
                why: closed.why,
                at_ms: closed.ms,
            };
        }
        if self.boot_id != boot.id {
            let why = if self.expires_ms <= boot.booted_ms {
                ClosureReason::Expired
            } else {
                ClosureReason::Reboot
            };
            let at_ms = match why {
                ClosureReason::Expired => self.expires_ms,
                _ => boot.booted_ms.max(self.issued_ms),
            };
            return GrantState::Ended { why, at_ms };
        }
        if self.expires_ms <= now_ms {
            return GrantState::Ended {
                why: ClosureReason::Expired,
                at_ms: self.expires_ms,
            };
        }
        GrantState::Active {
            remaining_ms: self.expires_ms - now_ms,
        }
    }

    /// Record how this grant ended, if it has not been recorded already.
    ///
    /// Returns whether anything changed, so the caller writes one audit line
    /// per ending rather than one per sweep.
    pub fn close(&mut self, why: ClosureReason, at_ms: u64) -> bool {
        if self.closed.is_some() {
            return false;
        }
        self.closed = Some(GrantClosure { ms: at_ms, why });
        true
    }

    /// What the machine says about this grant, in one sentence.
    ///
    /// The sentence is the deliverable, not a nicety: §3.4's requirement is
    /// that a grant does not *silently* stop applying, and a state nothing can
    /// render is silent whatever the enum says.
    pub fn describe(&self, now_ms: u64, boot: &BootStamp) -> String {
        let who = format!("grant {} ({} for session {})", self.id, self.kind, self.session);
        let window = format_ms(self.expires_ms.saturating_sub(self.issued_ms));
        match self.state_at(now_ms, boot) {
            GrantState::Active { remaining_ms } => {
                format!("{who} is active with {} of its {window} left", format_ms(remaining_ms))
            }
            GrantState::Ended {
                why: ClosureReason::Revoked,
                ..
            } => format!("{who} was revoked"),
            GrantState::Ended {
                why: ClosureReason::Expired,
                ..
            } => format!("{who} expired after {window}"),
            GrantState::Ended {
                why: ClosureReason::Reboot,
                at_ms,
            } => format!(
                "{who} ended when the machine rebooted, with {} of its {window} window unused. \
                 It was not carried across and nothing has been re-authorised",
                format_ms(self.expires_ms.saturating_sub(at_ms))
            ),
            GrantState::Ended {
                why: ClosureReason::RuntimeRestart,
                at_ms,
            } => format!(
                "{who} ended when the agent runtime restarted, with {} of its {window} window \
                 unused. A grant lives in the running daemon and is not adopted from disk, so \
                 nothing has been re-authorised",
                format_ms(self.expires_ms.saturating_sub(at_ms))
            ),
        }
    }

    /// Whether this grant covers `verb_name`.
    ///
    /// Capability scoping, and it is a whitelist: a verb that is not named is
    /// not covered, so adding a verb to the vocabulary does not widen every
    /// grant already issued.
    pub fn covers(&self, verb_name: &str) -> bool {
        self.capabilities.iter().any(|c| c == verb_name)
    }
}

/// Normalise a capability list: sorted, de-duplicated, no empties.
pub fn normalise_capabilities<I, S>(caps: I) -> Vec<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    caps.into_iter()
        .map(|c| c.as_ref().trim().to_string())
        .filter(|c| !c.is_empty())
        .collect::<BTreeSet<String>>()
        .into_iter()
        .collect()
}

// ── the store ───────────────────────────────────────────────────────────────
//
// One JSON file per grant, beside the privilege requests, for the same reason
// those are files: a human being able to read and diff what privilege this
// machine has handed out is a feature. See the module docs for why this is
// history rather than authority.

/// Where issued grants are recorded.
pub fn grants_dir() -> PathBuf {
    crate::paths::state_dir().join("system-grants")
}

fn record_path(dir: &Path, id: u32) -> PathBuf {
    dir.join(format!("{id}.json"))
}

/// Every grant on disk, oldest first.
///
/// A file that will not parse is skipped rather than failing the listing, for
/// the same reason [`crate::request::list`] does it: one corrupt record must
/// not hide the rest of what has been granted on this machine.
pub fn list(dir: &Path) -> std::io::Result<Vec<SystemGrant>> {
    let mut out = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
        Err(e) => return Err(e),
    };
    for entry in entries.flatten() {
        if entry.path().extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        if let Ok(text) = std::fs::read_to_string(entry.path()) {
            if let Ok(g) = serde_json::from_str::<SystemGrant>(&text) {
                out.push(g);
            }
        }
    }
    out.sort_by_key(|g| (g.issued_ms, g.id));
    Ok(out)
}

/// Read one grant.
pub fn load(dir: &Path, id: u32) -> std::io::Result<Option<SystemGrant>> {
    match std::fs::read_to_string(record_path(dir, id)) {
        Ok(text) => Ok(serde_json::from_str(&text).ok()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// Write a grant, replacing any previous version atomically.
pub fn save(dir: &Path, grant: &SystemGrant) -> std::io::Result<()> {
    crate::paths::ensure_private_dir(dir)?;
    let path = record_path(dir, grant.id);
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(grant)?.as_bytes())?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

/// The next free id, from the highest ever used rather than the count.
///
/// Same rule as [`crate::request::next_id`]: an audit trail with two meanings
/// for one id is not an audit trail.
pub fn next_id(dir: &Path) -> u32 {
    list(dir)
        .unwrap_or_default()
        .iter()
        .map(|g| g.id)
        .max()
        .unwrap_or(0)
        + 1
}

/// Append one grant event to the privilege audit trail, and mirror it to the
/// journal.
///
/// The same `privilege-audit.jsonl` the request path writes, because "what
/// privilege was exercised on this machine" is one question and two files
/// would be two partial answers. The journal copy is the one §3.4's
/// "permanent audit record" rests on — see the module docs.
pub fn audit(path: &Path, event: &str, grant: &SystemGrant, state: &GrantState) {
    let line = serde_json::json!({
        "ms": crate::request::now_ms(),
        "event": event,
        "grant": grant.id,
        "kind": grant.kind.as_str(),
        "state": state.as_str(),
        "session": grant.session,
        "agent": grant.agent,
        "project": grant.project,
        "capabilities": grant.capabilities,
        "issued_ms": grant.issued_ms,
        "expires_ms": grant.expires_ms,
        "boot_id": grant.boot_id,
        "request_origin": grant.request_origin.as_str(),
        "authenticated_by": grant.authenticated_by,
    });
    if let Some(parent) = path.parent() {
        let _ = crate::paths::ensure_private_dir(parent);
    }
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        use std::io::Write;
        let _ = writeln!(file, "{line}");
    }
    crate::journal::grant_event(event, grant, state);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_narrowing_is_the_whole_vocabulary_and_it_is_named_not_wildcarded() {
        // P0-007 criterion 3, the default half. The flag's absence has to mean
        // exactly what it meant before the flag existed, or every session
        // already running loses privilege on upgrade.
        let all = capabilities_for(GrantKind::SystemAccess, None).expect("the default is issuable");
        assert_eq!(all.len(), crate::request::Verb::names().len());
        for v in crate::request::Verb::names() {
            assert!(all.contains(&v.to_string()), "the default dropped {v}");
        }
        // Named, never "all": the list is resolved at issue time, so a verb
        // added tomorrow is not covered by a grant issued today. A wildcard
        // would be, silently, and nobody would ever see the widening.
        assert!(!all.iter().any(|c| c == "*" || c == "all"));
    }

    #[test]
    fn a_narrowed_grant_carries_only_the_verbs_that_were_named() {
        let caps = capabilities_for(
            GrantKind::SystemAccess,
            Some(&["update".to_string(), "install".to_string()]),
        )
        .expect("two real verbs are issuable");
        assert_eq!(caps, vec!["install".to_string(), "update".to_string()]);
        // And the grant built from it answers `covers` the same way, which is
        // the enforcement point rather than the record.
        let mut g = grant(GrantKind::SystemAccess, 1_000, 900_000, "b");
        g.capabilities = caps;
        assert!(g.covers("install"));
        assert!(g.covers("update"));
        assert!(!g.covers("rollback"), "a verb nobody named is not covered");
        assert!(!g.covers("pkg-rollback"));
    }

    #[test]
    fn a_name_that_is_not_a_verb_is_refused_rather_than_dropped() {
        // The failure this prevents: `--capabilities instal` normalised away
        // leaves a grant covering nothing, or — worse, if the caller also
        // named a real verb — a grant that silently covers less than was
        // asked for, discovered mid-session.
        assert_eq!(
            capabilities_for(GrantKind::SystemAccess, Some(&["instal".to_string()])),
            Err(CapabilityError::NotAVerb("instal".to_string()))
        );
        assert_eq!(
            capabilities_for(
                GrantKind::SystemAccess,
                Some(&["install".to_string(), "sudo".to_string()])
            ),
            Err(CapabilityError::NotAVerb("sudo".to_string())),
            "one bad name refuses the whole list rather than issuing the good half"
        );
        // The sentence has to name the vocabulary, or the remedy is a guess.
        let said = CapabilityError::NotAVerb("instal".to_string()).to_string();
        for v in crate::request::Verb::names() {
            assert!(said.contains(v), "the refusal does not say {v} is available");
        }
    }

    #[test]
    fn an_empty_narrowing_is_refused_and_break_glass_cannot_be_narrowed_at_all() {
        assert_eq!(
            capabilities_for(GrantKind::SystemAccess, Some(&[])),
            Err(CapabilityError::Empty)
        );
        // Whitespace is the same thing arriving by a different route:
        // `--capabilities " "` must not be an all-covering grant.
        assert_eq!(
            capabilities_for(GrantKind::SystemAccess, Some(&["  ".to_string()])),
            Err(CapabilityError::Empty)
        );
        // Break-glass has no verbs to narrow — it clears no_new_privs and the
        // session uses sudo — so the flag is refused rather than accepted and
        // ignored, which would read as a narrowing that is not one.
        assert_eq!(
            capabilities_for(GrantKind::BreakGlass, Some(&["install".to_string()])),
            Err(CapabilityError::NotScopable)
        );
        assert_eq!(capabilities_for(GrantKind::BreakGlass, None), Ok(Vec::new()));
    }

    fn boot(id: &str, booted_ms: u64) -> BootStamp {
        BootStamp {
            id: id.to_string(),
            booted_ms,
        }
    }

    fn grant(kind: GrantKind, issued_ms: u64, ttl_ms: u64, boot_id: &str) -> SystemGrant {
        SystemGrant {
            id: 1,
            kind,
            session: 7,
            agent: "claude".into(),
            project: Some("/home/tester/Projects/demo".into()),
            capabilities: normalise_capabilities(["install", "update"]),
            issued_ms,
            expires_ms: issued_ms + ttl_ms,
            boot_id: boot_id.to_string(),
            request_origin: RequestOrigin::LocalTerminal,
            authenticated_by: "org.apexos.agent.break-glass".into(),
            closed: None,
        }
    }

    #[test]
    fn a_grant_is_active_until_its_ttl_runs_out_and_not_a_millisecond_after() {
        let b = boot("boot-a", 1_000);
        let g = grant(GrantKind::BreakGlass, 10_000, 900_000, "boot-a");
        assert_eq!(
            g.state_at(10_000, &b),
            GrantState::Active { remaining_ms: 900_000 }
        );
        assert_eq!(
            g.state_at(909_999, &b),
            GrantState::Active { remaining_ms: 1 }
        );
        // The boundary is closed at the top: at the expiry instant it is gone.
        let gone = GrantState::Ended { why: ClosureReason::Expired, at_ms: 910_000 };
        assert_eq!(g.state_at(910_000, &b), gone);
        assert_eq!(g.state_at(910_001, &b), gone);
    }

    #[test]
    fn a_grant_from_a_previous_boot_is_reported_as_ended_not_forgotten() {
        // P0-006 criterion 5, and the whole reason `boot_id` is on the record.
        // A grant that was still live when the machine went down must come
        // back as something the machine can SAY, not as an absence.
        let issued = 1_000_000;
        let g = grant(GrantKind::BreakGlass, issued, 900_000, "boot-a");
        // The machine is now on a different boot, which started after the
        // grant was issued and before it was due to expire.
        let now = boot("boot-b", issued + 300_000);
        let state = g.state_at(issued + 400_000, &now);
        assert_eq!(state.reason(), Some(ClosureReason::Reboot));
        assert_eq!(state.as_str(), "ended-at-reboot");
        assert!(!state.is_active());
        // And it says so in words, naming the reboot and the unused window.
        let said = g.describe(issued + 400_000, &now);
        assert!(said.contains("rebooted"), "{said}");
        assert!(said.contains("nothing has been re-authorised"), "{said}");
    }

    #[test]
    fn a_grant_that_ran_out_before_the_reboot_is_expired_rather_than_cut_short() {
        // The distinction that stops the reboot rule from over-claiming. This
        // grant's fifteen minutes were over long before the machine went
        // down, so the reboot did not end it and saying so would be wrong.
        let issued = 1_000_000;
        let g = grant(GrantKind::BreakGlass, issued, 900_000, "boot-a");
        let now = boot("boot-b", issued + 900_000);
        assert_eq!(
            g.state_at(issued + 5_000_000, &now),
            GrantState::Ended { why: ClosureReason::Expired, at_ms: issued + 900_000 }
        );
        // One millisecond of the window left when the machine went down is
        // still the other answer.
        let now = boot("boot-b", issued + 899_999);
        assert_eq!(
            g.state_at(issued + 5_000_000, &now).reason(),
            Some(ClosureReason::Reboot)
        );
    }

    #[test]
    fn an_unreadable_boot_time_never_claims_a_grant_expired_on_its_own() {
        // `booted_ms` is zero when /proc/stat could not be read. The
        // conservative answer is the one that does not assert something about
        // a window it cannot place in time.
        let g = grant(GrantKind::BreakGlass, 1_000_000, 900_000, "boot-a");
        assert_eq!(
            g.state_at(9_000_000, &boot("boot-b", 0)).reason(),
            Some(ClosureReason::Reboot)
        );
    }

    #[test]
    fn a_recorded_ending_is_the_answer_forever() {
        // The reason a grant carries how it ended rather than having it
        // re-derived. Once the clock has moved past every deadline, every
        // ending would read as "expired" — and which of the four it really
        // was is the only part of the trail anybody reads it for.
        for why in [
            ClosureReason::Revoked,
            ClosureReason::Reboot,
            ClosureReason::RuntimeRestart,
        ] {
            let mut g = grant(GrantKind::SystemAccess, 1_000, 60_000, "boot-a");
            assert!(g.close(why, 30_000), "the first close must take");
            for b in [boot("boot-a", 0), boot("boot-b", 40_000)] {
                assert_eq!(
                    g.state_at(9_000_000, &b),
                    GrantState::Ended { why, at_ms: 30_000 },
                    "{why} was re-derived as something else"
                );
            }
            // And closing again changes nothing, so the audit line for one
            // ending is written once however many sweeps see it.
            assert!(!g.close(ClosureReason::Expired, 999_999));
            assert_eq!(g.state_at(9_000_000, &boot("boot-a", 0)).reason(), Some(why));
        }
    }

    #[test]
    fn a_restart_ending_says_so_rather_than_claiming_the_window_ran_out() {
        // A daemon restart ends every grant, because authority is the
        // daemon's memory. Reporting that as "expired" would tell the owner
        // their fifteen minutes were up when two of them had been used.
        let mut g = grant(GrantKind::BreakGlass, 1_000_000, 900_000, "boot-a");
        g.close(ClosureReason::RuntimeRestart, 1_120_000);
        let said = g.describe(9_000_000, &boot("boot-a", 0));
        assert!(said.contains("agent runtime restarted"), "{said}");
        assert!(said.contains("13m"), "unused window not reported: {said}");
        assert!(said.contains("nothing has been re-authorised"), "{said}");
    }

    #[test]
    fn the_two_kinds_are_not_interchangeable() {
        // §4.5: break-glass "is deliberately different from system-access
        // mode". The difference that costs something is which one expiry can
        // be enforced against without ending the session.
        assert_eq!(GrantKind::BreakGlass.system_access(), SystemAccess::Unsafe);
        assert_eq!(GrantKind::SystemAccess.system_access(), SystemAccess::Session);
        assert!(GrantKind::BreakGlass.expiry_ends_the_session());
        assert!(!GrantKind::SystemAccess.expiry_ends_the_session());
        // And the mapping is total and round-trips both ways.
        assert_eq!(GrantKind::for_system_access(SystemAccess::None), None);
        for k in GrantKind::ALL {
            assert_eq!(GrantKind::for_system_access(k.system_access()), Some(*k));
        }
    }

    #[test]
    fn a_ttl_without_a_unit_is_refused_rather_than_guessed() {
        // "15" is fifteen seconds or fifteen minutes depending on who is
        // reading, and the value is a security window.
        for bad in ["15", "", "m", "15x", "-5m", "1.5h", "15 m", "fifteen"] {
            assert!(parse_ttl(bad).is_err(), "{bad:?} parsed");
        }
        assert_eq!(parse_ttl("15m"), Ok(900_000));
        assert_eq!(parse_ttl("90s"), Ok(90_000));
        assert_eq!(parse_ttl("1h"), Ok(3_600_000));
        assert_eq!(parse_ttl(" 15m "), Ok(900_000));
    }

    #[test]
    fn break_glass_refuses_to_default_its_own_window() {
        // §3.4: "explicit short TTL". A default is not explicit.
        assert_eq!(
            ttl_for(GrantKind::BreakGlass, None),
            Err(TtlError::BreakGlassNeedsTtl)
        );
        assert!(ttl_for(GrantKind::BreakGlass, None)
            .unwrap_err()
            .to_string()
            .contains("--ttl 15m"));
        // The session grant is a smaller thing and may default.
        assert_eq!(
            ttl_for(GrantKind::SystemAccess, None),
            Ok(DEFAULT_SESSION_ACCESS_MS)
        );
    }

    #[test]
    fn no_window_is_unbounded_and_none_is_zero() {
        // "no remember forever" as a bound rather than a sentence, and over
        // both kinds so a new mode cannot arrive uncapped.
        for kind in GrantKind::ALL {
            assert_eq!(ttl_for(*kind, Some(0)), Err(TtlError::Zero));
            let cap = match kind {
                GrantKind::BreakGlass => MAX_BREAK_GLASS_MS,
                GrantKind::SystemAccess => MAX_SESSION_ACCESS_MS,
            };
            assert_eq!(ttl_for(*kind, Some(cap)), Ok(cap));
            let over = ttl_for(*kind, Some(cap + 1)).expect_err("must be capped");
            assert!(over.to_string().contains("§3.4"), "{over}");
        }
        // That break-glass is the shorter of the two is a fact about two
        // constants and is asserted at compile time, beside them.
    }

    #[test]
    fn a_grant_covers_only_the_verbs_it_names() {
        let g = grant(GrantKind::SystemAccess, 0, 1000, "b");
        assert!(g.covers("install"));
        assert!(g.covers("update"));
        assert!(!g.covers("remove"));
        // A verb added to the vocabulary tomorrow is not covered by a grant
        // issued today.
        assert!(!g.covers("something-new"));
        // Break-glass carries none, because it does not work through the
        // request vocabulary at all.
        let mut bg = grant(GrantKind::BreakGlass, 0, 1000, "b");
        bg.capabilities = Vec::new();
        assert!(!bg.covers("install"));
    }

    #[test]
    fn capabilities_are_stored_in_one_canonical_shape() {
        assert_eq!(
            normalise_capabilities(["update", "install", "install", " ", " update "]),
            vec!["install".to_string(), "update".to_string()]
        );
        assert!(normalise_capabilities(Vec::<String>::new()).is_empty());
    }

    #[test]
    fn btime_is_read_from_the_line_that_says_btime() {
        // /proc/stat's first lines start with `cpu`, and one of them is
        // `intr` with hundreds of numbers. Picking a field by position would
        // find the wrong one.
        let stat = "cpu  1 2 3 4\ncpu0 1 2 3 4\nintr 9 9 9 9\nctxt 12345\nbtime 1757000000\n\
                    processes 7\n";
        assert_eq!(parse_btime(stat), Some(1_757_000_000_000));
        assert_eq!(parse_btime("cpu 1 2 3\nctxt 4\n"), None);
        assert_eq!(parse_btime("btime notanumber\n"), None);
    }

    #[test]
    fn the_running_kernel_has_a_boot_stamp_this_can_read() {
        // Against real /proc, because the parsing is where this breaks. Both
        // halves must be present on any machine this runs on.
        let b = BootStamp::current();
        assert!(!b.id.is_empty(), "no boot id");
        assert!(b.booted_ms > 1_000_000_000_000, "implausible btime {}", b.booted_ms);
    }

    #[test]
    fn a_grant_survives_the_wire_and_a_record_without_the_optional_fields_loads() {
        let g = grant(GrantKind::BreakGlass, 5, 10, "boot-a");
        let text = serde_json::to_string(&g).expect("serialise");
        assert_eq!(serde_json::from_str::<SystemGrant>(&text).unwrap(), g);
        // `closed` defaults, so a record written before it existed still
        // loads rather than making the listing blind.
        let older = serde_json::json!({
            "id": 2, "kind": "system_access", "session": 3, "agent": "claude",
            "project": null, "capabilities": [], "issued_ms": 1, "expires_ms": 2,
            "boot_id": "b", "request_origin": "local-terminal",
            "authenticated_by": "org.apexos.agent.system-access",
        });
        let parsed: SystemGrant = serde_json::from_value(older).expect("parse");
        assert_eq!(parsed.closed, None);
    }

    #[test]
    fn the_store_round_trips_and_never_reuses_an_id() {
        let dir = std::env::temp_dir().join(format!("apex-grant-store-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        assert_eq!(next_id(&dir), 1);
        assert!(list(&dir).expect("empty listing").is_empty());

        for (id, issued) in [(1u32, 10u64), (2, 20), (3, 30)] {
            let mut g = grant(GrantKind::SystemAccess, issued, 100, "b");
            g.id = id;
            save(&dir, &g).expect("save");
        }
        assert_eq!(next_id(&dir), 4);
        assert_eq!(load(&dir, 3).unwrap().map(|g| g.id), Some(3));
        assert_eq!(load(&dir, 9).unwrap(), None);
        assert_eq!(list(&dir).unwrap().len(), 3);
        // Oldest first, by when they were issued.
        assert_eq!(
            list(&dir).unwrap().iter().map(|g| g.id).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );

        // Deleting one from the middle must not let the next grant reuse its
        // id: two records meaning "grant 2" is not an audit trail.
        std::fs::remove_file(dir.join("2.json")).expect("remove");
        assert_eq!(next_id(&dir), 4);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_duration_reads_back_the_way_a_human_wrote_it() {
        assert_eq!(format_ms(900_000), "15m");
        assert_eq!(format_ms(5_400_000), "1h 30m");
        assert_eq!(format_ms(90_000), "1m 30s");
        assert_eq!(format_ms(0), "0s");
    }
}
