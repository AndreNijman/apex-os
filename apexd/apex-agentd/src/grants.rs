//! Who actually holds a system-access grant, daemon side.
//!
//! [`apex_agent_core::grant`] is the model: what a grant is, when it has
//! ended, and how it is written down. This is the part that decides whether
//! one exists, and it is deliberately small, because the whole security
//! argument rests on one sentence:
//!
//! > **A grant is in force only while this daemon process is holding it in
//! > memory.**
//!
//! Everything else follows. `apex-agentd` runs as the user; a `--sandbox
//! unrestricted` session runs as the user; a break-glass session is
//! unrestricted by definition. So every file this daemon can write, a granted
//! session can rewrite — including the grant record and including the JSONL
//! audit. If the grant store were consulted for permission, an agent could
//! mint itself root with a text editor.
//!
//! It therefore never is. [`GrantAuthority::active_for`] reads
//! [`GrantAuthority::live`] and nothing else. The store is written so a human
//! can read what happened, and so the next start can say what ended; it is
//! never read back as authority. The journal copy is what survives the subject
//! of the grant having write access to everything else.
//!
//! ## The startup sweep
//!
//! Because authority is process memory, a daemon that has just started holds
//! nothing. Every grant left on disk has therefore ended, and the sweep's job
//! is to say *how*, once, in the trail:
//!
//! * a grant from a previous boot that was still live when the machine went
//!   down — [`ClosureReason::Reboot`], which is §3.4's "no silent persistence
//!   after reboot" answered rather than dodged;
//! * one whose TTL ran out before that — [`ClosureReason::Expired`];
//! * one from *this* boot, which means this daemon replaced another —
//!   [`ClosureReason::RuntimeRestart`].
//!
//! And a session record still claiming an elevated policy is a session from a
//! daemon that is gone, whose PTY died with it; `registry::reconcile_stale_records`
//! already marks those exited. The sweep does not have to chase them.

use std::collections::HashMap;
use std::sync::Mutex;

use apex_agent_core::grant::{
    self, BootStamp, ClosureReason, GrantKind, GrantState, SystemGrant,
};
use apex_agent_core::policy::RequestOrigin;
use apex_agent_core::webauthn::{AssertionError, RemoteElevationRefused, SecondFactor};

/// Everything the daemon knows about system-access grants.
pub struct GrantAuthority {
    /// The running kernel's boot identity, read once at startup.
    boot: BootStamp,
    /// The grants this process minted and has not yet seen end. The only
    /// thing anywhere that confers permission.
    live: Mutex<HashMap<u32, SystemGrant>>,
}

/// Why a grant could not be issued, renewed or revoked.
///
/// Every variant is a refusal a human should be able to act on, so each
/// renders as a sentence naming what to do instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GrantError {
    /// P0-007's fourth criterion. The connection resolves to a managed
    /// session, so whatever it is asking for, it is the granted party asking
    /// about its own grant.
    FromInsideASession { session: u32, what: &'static str },
    /// §7: elevation is reserved for a local origin.
    NotLocal { origin: RequestOrigin, what: &'static str },
    /// The origin could not be established at all, which is not the same as
    /// being local and must not be treated as it.
    OriginUnknown(String),
    /// No grant with that id.
    NoSuchGrant(u32),
    /// The grant exists but is not in force, so there is nothing to change.
    NotActive { id: u32, state: String },
    /// The TTL was refused.
    Ttl(grant::TtlError),
    /// polkit said no, or could not be asked.
    NotAuthenticated(String),
    /// The peer could not be pinned for polkit.
    SubjectUnreadable(String),
    /// §7's second column, decided (P0-014).
    ///
    /// The origin is not local, and whether that can be authorised at all is
    /// the owner's `OriginPolicy` — plus, when it is allowed, a touch on an
    /// enrolled security key that names this exact elevation.
    ///
    /// This is what a non-local caller is now told instead of
    /// [`GrantError::NotLocal`]. `NotLocal` is still what
    /// [`crate::privilege::may_be_granted`] answers, because that function
    /// asks "is this the local column"; what P0-014 changed is that the answer
    /// to that question is no longer the end of the decision.
    RemoteElevation(RemoteElevationRefused),
    /// A key answered, and the answer did not check out.
    ///
    /// Kept apart from [`GrantError::RemoteElevation`] because the two ask
    /// different things of the reader: that one is a setting to change or a
    /// touch to collect, this one is a signature, a counter or a challenge
    /// that ran out.
    SecondFactorRefused(AssertionError),
}

impl std::fmt::Display for GrantError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GrantError::FromInsideASession { session, what } => write!(
                f,
                "this connection belongs to session {session}, and a session cannot {what}. \
                 §3.3 makes a root grant something a human delegates, so it is asked for from \
                 a terminal or the Agent Center — not from inside the session that would hold \
                 it. The daemon knows which this is from the kernel's view of the connection, \
                 not from anything the request said"
            ),
            GrantError::NotLocal { origin, what } => write!(
                f,
                "a {origin} request cannot {what}: §7 reserves elevation for a human at this \
                 machine, whichever origin is driving the session. Ask from a local terminal \
                 or from the Agent Center"
            ),
            GrantError::OriginUnknown(why) => write!(
                f,
                "{why}, so this connection cannot be shown to be a human at this machine — and \
                 a grant that cannot name who authorised it is not a grant"
            ),
            GrantError::NoSuchGrant(id) => write!(f, "no system-access grant {id}"),
            GrantError::NotActive { id, state } => write!(
                f,
                "grant {id} is {state}, so there is nothing to change. Start a new session \
                 with the mode you need; a grant is issued to a session, not renewed into one"
            ),
            GrantError::Ttl(e) => write!(f, "{e}"),
            GrantError::NotAuthenticated(why) => write!(f, "{why}"),
            GrantError::SubjectUnreadable(why) => write!(f, "{why}"),
            // Already a sentence naming what to do; see
            // `RemoteElevationRefused`'s own `Display`.
            GrantError::RemoteElevation(why) => write!(f, "{why}"),
            GrantError::SecondFactorRefused(why) => write!(
                f,
                "the security key's answer was refused: {why}. The challenge it answered has \
                 been spent either way — one issue, one attempt, so that a challenge cannot be \
                 ground against — which means a new one has to be issued before the key is \
                 touched again"
            ),
        }
    }
}

impl std::error::Error for GrantError {}

impl GrantAuthority {
    /// Read the boot stamp and close out everything left on disk.
    ///
    /// Returns one line per grant it closed, which the daemon prints at
    /// startup. Printed rather than swallowed because "the break-glass window
    /// you opened before the reboot is over" is the sentence §3.4's fifth
    /// requirement is really asking for, and a state nobody renders is a
    /// silent one.
    pub fn new() -> GrantAuthority {
        GrantAuthority {
            boot: BootStamp::current(),
            live: Mutex::new(HashMap::new()),
        }
    }

    /// Close out every grant left on disk by a previous daemon or a previous
    /// boot, recording how each ended.
    ///
    /// Separate from [`GrantAuthority::new`] so a test can build an authority
    /// without touching the user's real state directory.
    pub fn sweep_previous_lives(&self) -> Vec<String> {
        let dir = grant::grants_dir();
        let now = apex_agent_core::request::now_ms();
        let mut said = Vec::new();
        for mut g in grant::list(&dir).unwrap_or_default() {
            if g.closed.is_some() {
                continue;
            }
            // Nothing on disk is live, because this process has just started
            // and holds nothing. What varies is why.
            let why = match g.state_at(now, &self.boot).reason() {
                Some(reason) => reason,
                // Still inside its TTL and stamped with this boot: a daemon
                // restart, which this function is the only thing that can see.
                None => ClosureReason::RuntimeRestart,
            };
            if !g.close(why, now) {
                continue;
            }
            let state = g.state_at(now, &self.boot);
            let _ = grant::save(&dir, &g);
            grant::audit(&apex_agent_core::request::audit_log(), why.as_str(), &g, &state);
            said.push(g.describe(now, &self.boot));
        }
        said
    }

    /// The running boot's stamp.
    pub fn boot(&self) -> &BootStamp {
        &self.boot
    }

    /// Mint a grant, having already established that the asker may have one.
    ///
    /// Takes the authentication as a value it cannot produce itself
    /// ([`Authenticated`]), so a call site that skipped the prompt does not
    /// compile. That is a weaker guarantee than a kernel check and a stronger
    /// one than a comment.
    #[allow(clippy::too_many_arguments)]
    pub fn issue(
        &self,
        proof: Authenticated,
        kind: GrantKind,
        session: u32,
        agent: &str,
        project: Option<&str>,
        ttl_ms: u64,
        origin: RequestOrigin,
        now_ms: u64,
    ) -> SystemGrant {
        let dir = grant::grants_dir();
        let g = SystemGrant {
            id: grant::next_id(&dir),
            kind,
            session,
            agent: agent.to_string(),
            project: project.map(|p| p.to_string()),
            // A session grant covers the whole privilege vocabulary; a
            // break-glass grant covers none of it, because break-glass does
            // not go through `apex request` at all — it goes through sudo.
            // Naming the verbs rather than saying "all" is what keeps the
            // scope fixed at issue time: a verb added tomorrow is not covered
            // by a grant issued today.
            capabilities: match kind {
                GrantKind::SystemAccess => grant::normalise_capabilities(
                    apex_agent_core::request::Verb::names().iter().copied(),
                ),
                GrantKind::BreakGlass => Vec::new(),
            },
            issued_ms: now_ms,
            expires_ms: now_ms.saturating_add(ttl_ms),
            boot_id: self.boot.id.clone(),
            request_origin: origin,
            authenticated_by: proof.recorded_as(kind),
            closed: None,
        };
        let _ = grant::save(&dir, &g);
        grant::audit(
            &apex_agent_core::request::audit_log(),
            "issued",
            &g,
            &GrantState::Active { remaining_ms: ttl_ms },
        );
        self.live.lock().expect("grants lock").insert(g.id, g.clone());
        g
    }

    /// The grant in force for `session`, if any.
    ///
    /// Reads only the in-memory map. This is the enforcement point, and it is
    /// three lines on purpose.
    pub fn active_for(&self, session: u32, now_ms: u64) -> Option<SystemGrant> {
        let live = self.live.lock().expect("grants lock");
        live.values()
            .find(|g| g.session == session && g.state_at(now_ms, &self.boot).is_active())
            .cloned()
    }

    /// Whether an in-force grant for `session` covers `verb`.
    pub fn covers(&self, session: Option<u32>, verb: &str, now_ms: u64) -> bool {
        self.covering_grant(session, verb, now_ms).is_some()
    }

    /// The id of the in-force grant that covers `verb` for `session`.
    ///
    /// The same decision as [`GrantAuthority::covers`], but saying *which*
    /// grant, because a request decided by a session grant has to record the
    /// authority that decided it or the audit cannot tell it apart from a
    /// standing project grant.
    pub fn covering_grant(&self, session: Option<u32>, verb: &str, now_ms: u64) -> Option<u32> {
        let session = session?;
        self.active_for(session, now_ms)
            .filter(|g| g.covers(verb))
            .map(|g| g.id)
    }

    /// Every grant this process is holding that is still in force.
    ///
    /// The live map, never the store: the store is written so a human can
    /// read what happened and is not authority, because everything this
    /// daemon can write, a granted session can rewrite.
    pub fn active(&self, now_ms: u64) -> Vec<SystemGrant> {
        let live = self.live.lock().expect("grants lock");
        let mut out: Vec<SystemGrant> = live
            .values()
            .filter(|g| g.state_at(now_ms, &self.boot).is_active())
            .cloned()
            .collect();
        out.sort_by_key(|g| g.id);
        out
    }

    /// Take a grant back.
    pub fn revoke(&self, id: u32, now_ms: u64) -> Result<SystemGrant, GrantError> {
        self.close_live(id, ClosureReason::Revoked, now_ms)
    }

    /// Replace a grant's window with a fresh one, having authenticated again.
    ///
    /// Not an extension of the old consent: the grant's `expires_ms` is
    /// recomputed from now, and the caller had to satisfy the same polkit
    /// action it satisfied to get the grant in the first place. See
    /// [`apex_agent_core::auth::required_for`], which says the same thing over
    /// values.
    pub fn renew(
        &self,
        _proof: Authenticated,
        id: u32,
        ttl_ms: u64,
        now_ms: u64,
    ) -> Result<SystemGrant, GrantError> {
        let mut live = self.live.lock().expect("grants lock");
        let Some(g) = live.get_mut(&id) else {
            return Err(GrantError::NoSuchGrant(id));
        };
        let state = g.state_at(now_ms, &self.boot);
        if !state.is_active() {
            return Err(GrantError::NotActive {
                id,
                state: state.as_str().to_string(),
            });
        }
        grant::ttl_for(g.kind, Some(ttl_ms)).map_err(GrantError::Ttl)?;
        g.expires_ms = now_ms.saturating_add(ttl_ms);
        let out = g.clone();
        drop(live);
        let _ = grant::save(&grant::grants_dir(), &out);
        grant::audit(
            &apex_agent_core::request::audit_log(),
            "renewed",
            &out,
            &GrantState::Active { remaining_ms: ttl_ms },
        );
        Ok(out)
    }

    /// Close every grant whose window has run out, and say which sessions must
    /// now end.
    ///
    /// §3.4's "automatic expiry". For a session grant, closing it *is* the
    /// expiry: it stops satisfying requests the moment it leaves the map. For
    /// break-glass it is not enough — `PR_SET_NO_NEW_PRIVS` was cleared at
    /// exec and cannot be put back, so the session would keep its root
    /// whatever this map said. Those sessions are returned so the caller ends
    /// them.
    pub fn expire(&self, now_ms: u64) -> Vec<(SystemGrant, bool)> {
        let expired: Vec<SystemGrant> = {
            let live = self.live.lock().expect("grants lock");
            live.values()
                .filter(|g| !g.state_at(now_ms, &self.boot).is_active())
                .cloned()
                .collect()
        };
        expired
            .into_iter()
            .filter_map(|g| {
                let closed = self
                    .close_live(g.id, ClosureReason::Expired, now_ms)
                    .ok()?;
                let ends_session = closed.kind.expiry_ends_the_session();
                Some((closed, ends_session))
            })
            .collect()
    }

    /// Every grant on record, with the state each is in now.
    ///
    /// Merges the live map over the store, so a grant this process holds
    /// reports its current window rather than the one written at issue.
    pub fn list(&self, now_ms: u64) -> Vec<(SystemGrant, GrantState, String)> {
        let live = self.live.lock().expect("grants lock").clone();
        let mut out: Vec<SystemGrant> = grant::list(&grant::grants_dir())
            .unwrap_or_default()
            .into_iter()
            .map(|g| live.get(&g.id).cloned().unwrap_or(g))
            .collect();
        for g in live.values() {
            if !out.iter().any(|o| o.id == g.id) {
                out.push(g.clone());
            }
        }
        out.sort_by_key(|g| (g.issued_ms, g.id));
        out.into_iter()
            .map(|g| {
                let state = g.state_at(now_ms, &self.boot);
                let said = g.describe(now_ms, &self.boot);
                (g, state, said)
            })
            .collect()
    }

    /// Drop a live grant with a recorded reason, writing the trail once.
    fn close_live(
        &self,
        id: u32,
        why: ClosureReason,
        now_ms: u64,
    ) -> Result<SystemGrant, GrantError> {
        let mut g = {
            let mut live = self.live.lock().expect("grants lock");
            match live.remove(&id) {
                Some(g) => g,
                None => return Err(GrantError::NoSuchGrant(id)),
            }
        };
        g.close(why, now_ms);
        let state = g.state_at(now_ms, &self.boot);
        let _ = grant::save(&grant::grants_dir(), &g);
        grant::audit(
            &apex_agent_core::request::audit_log(),
            why.as_str(),
            &g,
            &state,
        );
        Ok(g)
    }
}

impl Default for GrantAuthority {
    fn default() -> Self {
        GrantAuthority::new()
    }
}

/// Evidence that a human authenticated, for the call that mints a grant.
///
/// A token with a private field, constructible only by [`authenticate`] and
/// [`authenticated_by_key`]. `issue` and `renew` take one by value, so a call
/// site that forgot to ask does not compile. It proves nothing about *which*
/// human or *when* — the daemon's ordering does that — but it does make the
/// asking impossible to leave out by accident, which is the failure mode a
/// comment cannot prevent.
///
/// ## There are two ways to mint one, and that is P0-014
///
/// This used to say "a token only [`authenticate`] can produce", and that
/// sentence is now wrong. §7 has two columns and they are authenticated
/// differently:
///
/// - **local** — polkit, exactly as before. [`authenticate`].
/// - **non-local** — a verified assertion from an enrolled security key.
///   [`authenticated_by_key`], reachable only from
///   `privilege::decide_origin` having returned `Approved::ByKey`, which in
///   turn requires the owner to have opted in *and* a real signature over the
///   exact elevation being asked for.
///
/// The second is not a weakening, and the reason is in
/// `files/system/polkit-1/actions/org.apexos.agent.policy`: both actions are
/// `allow_any: no` and `allow_active: auth_admin`, and that file says in as
/// many words that "there is no password that makes a remote caller local".
/// A remote caller therefore *cannot pass* polkit — it is refused under
/// `allow_any`, and even a locally-logged-in owner would get the dialog on
/// the machine's own desktop, which the remote human by definition cannot
/// reach. Asking polkit after a verified touch would not be a second lock; it
/// would be a wall, and the security key would be dead code. So for the
/// non-local column the key *replaces* polkit rather than adding to it. The
/// local column is untouched.
///
/// The token carries which of the two happened, so
/// [`crate::grant::SystemGrant::authenticated_by`] records it and an auditor
/// can tell a touch from a password.
///
/// It carries a value now and no longer a `()`, and the derive list is the
/// thing to be careful with: it is deliberately **not** `Clone`. The whole
/// property is that `issue` and `renew` consume one, so one authentication
/// mints one grant; a `Clone` would turn a single touch or password into as
/// many grants as the holder cared to ask for, silently and with every test
/// still green. Nothing needs it — checked by removing it and building the
/// workspace, not by reading.
#[derive(Debug)]
pub struct Authenticated(Method);

/// How a human proved they were present.
#[derive(Debug)]
enum Method {
    /// polkit authorised the action. The recorded value is the action id.
    Polkit,
    /// An enrolled security key signed for this exact elevation, named by the
    /// label the owner enrolled it under.
    SecurityKey(String),
}

impl Authenticated {
    /// What [`crate::grant::SystemGrant::authenticated_by`] should say.
    ///
    /// The `security-key:` prefix is not decoration: it is what stops a key
    /// enrolled under the label `org.apexos.agent.break-glass` from producing
    /// an audit line that reads as though polkit had authorised it. Labels are
    /// chosen by the owner, so the namespace has to be separated by something
    /// the owner cannot write into the label's own value.
    pub fn recorded_as(&self, kind: GrantKind) -> String {
        match &self.0 {
            Method::Polkit => apex_agent_core::auth::action_for(kind).to_string(),
            Method::SecurityKey(label) => format!("security-key:{label}"),
        }
    }
}

/// The non-local column's proof: a receipt that has already been verified.
///
/// Takes the [`SecondFactor`] by reference rather than taking nothing, so this
/// cannot be called by a path that has not got one. `SecondFactor` itself has
/// no public constructor, so the only way to reach this function with a value
/// is to have gone through `webauthn::redeem_and_verify` — a real signature,
/// over a challenge this daemon issued, by a credential the owner enrolled.
pub fn authenticated_by_key(factor: &SecondFactor) -> Authenticated {
    Authenticated(Method::SecurityKey(factor.credential().to_string()))
}

/// Ask polkit, for a grant of `kind`, about `peer`.
///
/// The subject is the peer of the control connection. By the time this runs,
/// the caller has established that the peer is not inside a managed session
/// and that its origin is local — see `session::start` and `main::dispatch`,
/// where the order is written out. That ordering is the whole of §4.4's "the
/// user authenticates outside the agent PTY": polkit sends the challenge to
/// the authentication agent of the *subject's* login session.
pub fn authenticate(
    auth: &dyn apex_agent_core::auth::Authenticator,
    kind: GrantKind,
    peer: &crate::peer::Peer,
) -> Result<Authenticated, GrantError> {
    use apex_agent_core::auth::{ProcessSubject, Verdict};

    let action = apex_agent_core::auth::action_for(kind);
    let Some(subject) = ProcessSubject::for_pid(peer.pid, peer.uid) else {
        return Err(GrantError::SubjectUnreadable(format!(
            "/proc/{}/stat could not be read, so the process asking for this cannot be \
             identified to polkit, and an authentication that cannot name its subject \
             authorises nothing",
            peer.pid
        )));
    };
    match auth.check(action, &subject) {
        Ok(Verdict::Authorized) => Ok(Authenticated(Method::Polkit)),
        Ok(Verdict::Refused) => Err(GrantError::NotAuthenticated(format!(
            "the local authentication for {action} was refused or cancelled, so no grant was \
             issued"
        ))),
        Err(e) => Err(GrantError::NotAuthenticated(e.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use apex_agent_core::auth::{AuthError, Authenticator, ProcessSubject, Verdict};

    /// An authority with a store of its own, so these tests never touch the
    /// developer's real grants.
    fn isolated() -> (GrantAuthority, tempdir::Dir) {
        let dir = tempdir::Dir::new();
        (
            GrantAuthority {
                boot: BootStamp {
                    id: "test-boot".into(),
                    booted_ms: 1_000,
                },
                live: Mutex::new(HashMap::new()),
            },
            dir,
        )
    }

    /// A private `$XDG_STATE_HOME` for one test.
    ///
    /// The grant store's path comes from `paths::state_dir()`, which reads the
    /// environment, and `cargo test` runs in the developer's own environment.
    /// Without this, these tests would write grants into the state directory
    /// `apex agent grants` reads.
    mod tempdir {
        use std::path::PathBuf;
        use std::sync::{Mutex, MutexGuard, OnceLock};

        /// `set_var` is process-global, so the tests that need it run one at a
        /// time. A lock rather than `--test-threads=1`, which would slow the
        /// whole suite for four tests.
        fn lock() -> MutexGuard<'static, ()> {
            static L: OnceLock<Mutex<()>> = OnceLock::new();
            L.get_or_init(|| Mutex::new(()))
                .lock()
                .unwrap_or_else(|e| e.into_inner())
        }

        pub struct Dir {
            path: PathBuf,
            previous: Option<std::ffi::OsString>,
            _guard: MutexGuard<'static, ()>,
        }

        impl Dir {
            pub fn new() -> Dir {
                use std::sync::atomic::{AtomicU32, Ordering};
                static N: AtomicU32 = AtomicU32::new(0);
                let guard = lock();
                let path = std::env::temp_dir().join(format!(
                    "apex-grants-test-{}-{}",
                    std::process::id(),
                    N.fetch_add(1, Ordering::Relaxed)
                ));
                std::fs::remove_dir_all(&path).ok();
                std::fs::create_dir_all(&path).expect("mkdir");
                let previous = std::env::var_os("XDG_STATE_HOME");
                // Safe: the lock above serialises every test that touches it.
                unsafe { std::env::set_var("XDG_STATE_HOME", &path) };
                Dir {
                    path,
                    previous,
                    _guard: guard,
                }
            }
        }

        impl Drop for Dir {
            fn drop(&mut self) {
                // Safe: same lock, still held.
                unsafe {
                    match &self.previous {
                        Some(v) => std::env::set_var("XDG_STATE_HOME", v),
                        None => std::env::remove_var("XDG_STATE_HOME"),
                    }
                }
                std::fs::remove_dir_all(&self.path).ok();
            }
        }
    }

    struct Stub(Result<Verdict, AuthError>);

    impl Authenticator for Stub {
        fn check(&self, _a: &str, _s: &ProcessSubject) -> Result<Verdict, AuthError> {
            self.0.clone()
        }
    }

    fn me() -> crate::peer::Peer {
        crate::peer::Peer {
            pid: std::process::id() as libc::pid_t,
            uid: unsafe { libc::getuid() },
            gid: unsafe { libc::getgid() },
        }
    }

    fn proof() -> Authenticated {
        authenticate(
            &Stub(Ok(Verdict::Authorized)),
            GrantKind::BreakGlass,
            &me(),
        )
        .expect("the stub authorises")
    }

    /// A receipt for `kind`, minted the way the daemon mints one.
    fn touched(label: &str, kind: GrantKind) -> SecondFactor {
        use apex_agent_core::webauthn::{test_support::Signer, test_support::UP_UV, Challenge};
        let c = Challenge::with_nonce(None, kind, 900_000, 1_000, b"a-nonce-for-this-test");
        Signer::new(label).receipt(&c, UP_UV, 1)
    }

    #[test]
    fn a_grant_records_whether_a_password_or_a_key_authorised_it() {
        // §7's two columns are authenticated by different things, and an
        // audit line that could not tell them apart would answer "on whose
        // authority" with the same sentence for a password typed at this
        // keyboard and a touch collected from the other side of a network.
        for kind in [GrantKind::SystemAccess, GrantKind::BreakGlass] {
            let by_password = authenticate(&Stub(Ok(Verdict::Authorized)), kind, &me())
                .expect("the stub authorises");
            assert_eq!(
                by_password.recorded_as(kind),
                apex_agent_core::auth::action_for(kind),
                "the local column still records the polkit action id it satisfied"
            );

            let by_key = authenticated_by_key(&touched("yubikey 5c nfc", kind));
            assert_eq!(by_key.recorded_as(kind), "security-key:yubikey 5c nfc");
            assert_ne!(by_key.recorded_as(kind), by_password.recorded_as(kind));
        }
    }

    #[test]
    fn a_key_labelled_like_a_polkit_action_cannot_forge_a_password_audit_line() {
        // Labels are chosen by the owner at `apex agent key add`, and this
        // field is what `journalctl APEX_GRANT_AUTH=...` is read by. Without
        // a namespace the owner cannot write into, a key enrolled under the
        // action's own id would produce a line indistinguishable from one a
        // human typed a password for. That is not a privilege escalation —
        // enrolling the key is the owner's own act — but it is an audit trail
        // that can be made to say something that did not happen, which §3.4's
        // "APEX audit ON" is about.
        for kind in [GrantKind::SystemAccess, GrantKind::BreakGlass] {
            let action = apex_agent_core::auth::action_for(kind);
            let impersonating = authenticated_by_key(&touched(action, kind));
            assert_ne!(impersonating.recorded_as(kind), action);
            assert_eq!(impersonating.recorded_as(kind), format!("security-key:{action}"));
        }
    }

    #[test]
    fn the_grant_a_key_authorised_carries_that_into_the_record_and_the_journal_fields() {
        // Not just the token: the whole way through `issue` to the field an
        // auditor reads. A test of `recorded_as` alone would pass even if
        // `issue` went on ignoring the proof, which is what it did before.
        let _dir = tempdir::Dir::new();
        let a = GrantAuthority::new();
        let g = a.issue(
            authenticated_by_key(&touched("the owner's key", GrantKind::SystemAccess)),
            GrantKind::SystemAccess,
            7,
            "claude",
            None,
            900_000,
            RequestOrigin::RemoteControl,
            1_000,
        );
        assert_eq!(g.authenticated_by, "security-key:the owner's key");
        assert_eq!(g.request_origin, RequestOrigin::RemoteControl);
    }

    #[test]
    fn a_grant_is_in_force_only_while_this_process_holds_it() {
        // The whole security argument in one test. The record is on disk and
        // says "active"; a second authority — which is what a restarted
        // daemon is — holds nothing and answers no.
        let (a, _dir) = isolated();
        let now = 10_000;
        let g = a.issue(
            proof(),
            GrantKind::BreakGlass,
            7,
            "claude",
            None,
            900_000,
            RequestOrigin::LocalTerminal,
            now,
        );
        assert!(a.active_for(7, now).is_some());
        assert_eq!(a.active_for(7, now).map(|g| g.id), Some(g.id));
        // On disk and unclosed …
        let stored = grant::load(&grant::grants_dir(), g.id).unwrap().expect("saved");
        assert_eq!(stored.closed, None);
        assert!(stored.state_at(now, a.boot()).is_active());
        // … and worth nothing to a process that did not mint it.
        let fresh = GrantAuthority {
            boot: BootStamp {
                id: "test-boot".into(),
                booted_ms: 1_000,
            },
            live: Mutex::new(HashMap::new()),
        };
        assert!(fresh.active_for(7, now).is_none());
        assert!(!fresh.covers(Some(7), "install", now));
    }

    #[test]
    fn a_grant_belongs_to_one_session_and_covers_only_its_own_verbs() {
        // P0-007 criteria 1 and 3, at the enforcement point.
        let (a, _dir) = isolated();
        let now = 10_000;
        a.issue(
            proof(),
            GrantKind::SystemAccess,
            7,
            "claude",
            Some("/p"),
            900_000,
            RequestOrigin::LocalTerminal,
            now,
        );
        assert!(a.covers(Some(7), "install", now));
        // Not another session's, however close the id.
        assert!(!a.covers(Some(8), "install", now));
        assert!(!a.covers(Some(6), "install", now));
        // And not an unsessioned caller's, which is the human — who does not
        // need one, because they approve requests themselves.
        assert!(!a.covers(None, "install", now));
        // A verb outside the vocabulary is not covered, so a grant cannot be
        // widened by inventing a name.
        assert!(!a.covers(Some(7), "rm-rf", now));
    }

    #[test]
    fn the_authority_says_which_grant_covered_a_verb_not_only_that_one_did() {
        // The request record has to name the grant, so `covers` cannot be the
        // only answer available: a bool cannot be joined to the journal line
        // for the same window.
        let (a, _dir) = isolated();
        let now = 10_000;
        let g = a.issue(
            proof(),
            GrantKind::SystemAccess,
            7,
            "claude",
            None,
            60_000,
            RequestOrigin::LocalTerminal,
            now,
        );
        assert_eq!(a.covering_grant(Some(7), "install", now), Some(g.id));
        assert_eq!(
            a.covering_grant(Some(8), "install", now),
            None,
            "another session"
        );
        assert_eq!(a.covering_grant(None, "install", now), None, "no session");
        assert_eq!(
            a.covering_grant(Some(7), "rm-rf", now),
            None,
            "a verb outside the vocabulary"
        );
        assert_eq!(
            a.covering_grant(Some(7), "install", now + 60_001),
            None,
            "after the window"
        );
    }

    #[test]
    fn break_glass_covers_no_privilege_verb_at_all() {
        // The two modes are different things, and this is where that shows.
        // Break-glass is sudo inside the session; it does not pre-approve the
        // request vocabulary, so an expired break-glass grant cannot leave
        // behind a session whose `apex request install` goes through unasked.
        let (a, _dir) = isolated();
        let now = 10_000;
        a.issue(
            proof(),
            GrantKind::BreakGlass,
            7,
            "claude",
            None,
            900_000,
            RequestOrigin::LocalTerminal,
            now,
        );
        assert!(a.active_for(7, now).is_some());
        for verb in apex_agent_core::request::Verb::names() {
            assert!(!a.covers(Some(7), verb, now), "break-glass covered {verb}");
        }
    }

    #[test]
    fn expiry_drops_the_grant_and_names_the_sessions_that_have_to_end() {
        // §3.4's automatic expiry, and the difference between the two modes.
        // A session grant stops applying by leaving the map. Break-glass
        // cannot: no_new_privs was cleared at exec and nothing can put it
        // back, so the session itself has to go.
        let (a, _dir) = isolated();
        let now = 10_000;
        a.issue(proof(), GrantKind::SystemAccess, 7, "claude", None, 1_000,
                RequestOrigin::LocalTerminal, now);
        a.issue(proof(), GrantKind::BreakGlass, 8, "claude", None, 1_000,
                RequestOrigin::LocalTerminal, now);
        assert!(a.expire(now).is_empty(), "nothing has expired yet");
        assert!(a.active_for(7, now).is_some());

        let after = now + 1_000;
        let mut ended = a.expire(after);
        ended.sort_by_key(|(g, _)| g.session);
        assert_eq!(ended.len(), 2);
        assert_eq!(ended[0].0.session, 7);
        assert!(!ended[0].1, "a session grant does not end its session");
        assert_eq!(ended[1].0.session, 8);
        assert!(ended[1].1, "break-glass must end its session");

        // Gone from the map, and the ending is recorded once.
        assert!(a.active_for(7, after).is_none());
        assert!(a.expire(after + 1).is_empty(), "expiry was reported twice");
        let stored = grant::load(&grant::grants_dir(), ended[1].0.id).unwrap().unwrap();
        assert_eq!(stored.closed.map(|c| c.why), Some(ClosureReason::Expired));
    }

    #[test]
    fn a_renewal_replaces_the_window_and_only_while_the_grant_is_alive() {
        let (a, _dir) = isolated();
        let now = 10_000;
        let g = a.issue(proof(), GrantKind::BreakGlass, 7, "claude", None, 60_000,
                        RequestOrigin::LocalTerminal, now);
        assert_eq!(g.expires_ms, 70_000);

        // From now, not from the old expiry: a renewal is a new window on a
        // fresh authentication, not an extension of the old consent.
        let renewed = a.renew(proof(), g.id, 60_000, 40_000).expect("renew");
        assert_eq!(renewed.expires_ms, 100_000);
        // Still bounded — a renewal cannot buy more than the cap.
        assert!(matches!(
            a.renew(proof(), g.id, grant::MAX_BREAK_GLASS_MS + 1, 40_000),
            Err(GrantError::Ttl(_))
        ));
        // And a grant that has ended cannot be brought back.
        a.revoke(g.id, 50_000).expect("revoke");
        assert_eq!(
            a.renew(proof(), g.id, 60_000, 51_000),
            Err(GrantError::NoSuchGrant(g.id))
        );
    }

    #[test]
    fn the_startup_sweep_says_how_each_leftover_grant_ended() {
        // What the machine says on the boot after a grant was open. Three
        // records, three different endings, and every one of them written to
        // the trail with the reason rather than lost.
        let (a, _dir) = isolated();
        let dir = grant::grants_dir();
        let now = apex_agent_core::request::now_ms();
        let base = SystemGrant {
            id: 0,
            kind: GrantKind::BreakGlass,
            session: 1,
            agent: "claude".into(),
            project: None,
            capabilities: Vec::new(),
            issued_ms: 0,
            expires_ms: 0,
            boot_id: String::new(),
            request_origin: RequestOrigin::LocalTerminal,
            authenticated_by: "org.apexos.agent.break-glass".into(),
            closed: None,
        };
        // 1. still live when the machine went down.
        grant::save(&dir, &SystemGrant {
            id: 1,
            issued_ms: 500,
            expires_ms: 5_000,
            boot_id: "an-older-boot".into(),
            ..base.clone()
        }).expect("save");
        // 2. its window had already run out before that.
        grant::save(&dir, &SystemGrant {
            id: 2,
            issued_ms: 100,
            expires_ms: 900,
            boot_id: "an-older-boot".into(),
            ..base.clone()
        }).expect("save");
        // 3. this boot, so a daemon restart rather than a reboot.
        grant::save(&dir, &SystemGrant {
            id: 3,
            issued_ms: now,
            expires_ms: now + 3_600_000,
            boot_id: "test-boot".into(),
            ..base.clone()
        }).expect("save");

        let said = a.sweep_previous_lives();
        assert_eq!(said.len(), 3, "{said:#?}");

        let why = |id: u32| grant::load(&dir, id).unwrap().unwrap().closed.unwrap().why;
        assert_eq!(why(1), ClosureReason::Reboot);
        assert_eq!(why(2), ClosureReason::Expired);
        assert_eq!(why(3), ClosureReason::RuntimeRestart);

        // The sentences are the deliverable: a state nothing renders is a
        // silent one, which is exactly what §3.4's fifth requirement forbids.
        let all = said.join("\n");
        assert!(all.contains("machine rebooted"), "{all}");
        assert!(all.contains("agent runtime restarted"), "{all}");
        assert!(all.contains("nothing has been re-authorised"), "{all}");

        // And it is said once. A second sweep has nothing left to report,
        // because the ending is recorded on the grant.
        assert!(a.sweep_previous_lives().is_empty());

        // The audit trail carries all three, and none of them was minted by
        // this process — so nothing became live by being swept.
        let trail = std::fs::read_to_string(apex_agent_core::request::audit_log())
            .expect("the trail exists");
        assert_eq!(trail.matches("\"event\":\"ended-at-reboot\"").count(), 1, "{trail}");
        assert_eq!(trail.matches("\"event\":\"expired\"").count(), 1, "{trail}");
        assert_eq!(trail.matches("\"event\":\"ended-with-the-runtime\"").count(), 1, "{trail}");
        assert!(a.active_for(1, now).is_none());
    }

    #[test]
    fn a_refused_password_is_not_a_grant_and_says_which_it_was() {
        // The two failures a human can act on differently: they said no, and
        // the machine could not ask. Neither produces an `Authenticated`, so
        // neither can reach `issue` — that is what the token is for.
        let refused = authenticate(&Stub(Ok(Verdict::Refused)), GrantKind::BreakGlass, &me())
            .expect_err("a refusal is not a grant");
        assert!(refused.to_string().contains("refused or cancelled"), "{refused}");

        let unregistered = authenticate(
            &Stub(Err(AuthError::ActionNotRegistered(
                "org.apexos.agent.break-glass".into(),
            ))),
            GrantKind::BreakGlass,
            &me(),
        )
        .expect_err("an unaskable question is not a grant");
        assert!(unregistered.to_string().contains("xmllint"), "{unregistered}");

        // A peer that has gone cannot be pinned, and that is a refusal too.
        let gone = crate::peer::Peer {
            pid: 0x7fff_fffe,
            uid: 0,
            gid: 0,
        };
        let err = authenticate(&Stub(Ok(Verdict::Authorized)), GrantKind::BreakGlass, &gone)
            .expect_err("an unpinnable subject is not a grant");
        assert!(matches!(err, GrantError::SubjectUnreadable(_)), "{err}");
    }

    #[test]
    fn every_refusal_names_something_the_reader_can_do() {
        // These reach the user as the reason their session did not start.
        for e in [
            GrantError::FromInsideASession { session: 3, what: "ask for a grant" },
            GrantError::NotLocal {
                origin: RequestOrigin::RemoteControl,
                what: "ask for a grant",
            },
            GrantError::OriginUnknown("/proc/9/cgroup: no such file".into()),
            GrantError::NotActive { id: 1, state: "expired".into() },
            GrantError::RemoteElevation(RemoteElevationRefused::PolicyForbids {
                origin: RequestOrigin::RemoteControl,
            }),
            GrantError::RemoteElevation(RemoteElevationRefused::NoSecondFactor {
                origin: RequestOrigin::RemoteControl,
            }),
            GrantError::SecondFactorRefused(AssertionError::BadSignature),
            GrantError::SecondFactorRefused(AssertionError::NoUserPresence),
        ] {
            let msg = e.to_string();
            assert!(msg.len() > 60, "unhelpful refusal: {msg}");
        }
        assert!(GrantError::FromInsideASession { session: 3, what: "x" }
            .to_string()
            .contains("§3.3"));
        assert!(GrantError::NotLocal {
            origin: RequestOrigin::RemoteControl,
            what: "x"
        }
        .to_string()
        .contains("§7"));
        // The two P0-014 refusals have to name the thing the owner can change
        // — the setting, or the fact that the touch has to be collected again
        // — rather than merely saying no.
        assert!(
            GrantError::RemoteElevation(RemoteElevationRefused::PolicyForbids {
                origin: RequestOrigin::RemoteControl
            })
            .to_string()
            .contains("--origin-policy remote")
        );
        assert!(GrantError::SecondFactorRefused(AssertionError::BadSignature)
            .to_string()
            .contains("spent"));
    }
}
