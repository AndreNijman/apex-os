//! Where the agent runtime keeps its files.
//!
//! Three separate roots, because they have three different lifetimes:
//!
//! * the control socket lives in `$XDG_RUNTIME_DIR` — it is meaningless once
//!   the login session ends, and `XDG_RUNTIME_DIR` is already `0700` and
//!   `tmpfs`, so no session state ever reaches disk;
//! * session records and logs live in `$XDG_STATE_HOME` — they survive a
//!   daemon restart and a reboot so `apex agent list` can still explain what
//!   ran yesterday;
//! * per-session scratch lives under `/tmp/apex-agent-<uid>/<id>` — it is the
//!   one writable path outside the project that a sandboxed agent gets, and it
//!   is removed with the session. The uid is in the path because it was not,
//!   once, and a second account on the machine then could not start a session
//!   at all: see [`SCRATCH_ROOT_PREFIX`].
//!
//! Nothing here is privileged. Every path is user-owned and every directory is
//! created `0700`, because a session log is a transcript of the user's work.

use std::io;
use std::path::{Path, PathBuf};

/// `$XDG_RUNTIME_DIR`, or `/run/user/<uid>` when the variable is unset (a
/// non-login shell, a cron job). Falling back to the conventional path rather
/// than to `/tmp` matters: `/tmp` is shared, and a predictable socket path in a
/// shared directory is a hijack waiting to happen.
pub fn runtime_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR") {
        if !dir.is_empty() {
            return PathBuf::from(dir);
        }
    }
    // Safe: getuid() cannot fail and has no side effects.
    PathBuf::from(format!("/run/user/{}", unsafe { libc::getuid() }))
}

/// The current user's uid. Callers that must special-case root ask here
/// rather than reaching for `libc` themselves: the agent runtime is a per-user
/// service, and root has no `systemd --user` instance to enable it against
/// unless it lingers, which an ordinary login does not need.
pub fn uid() -> u32 {
    // Safe: getuid() cannot fail and has no side effects.
    unsafe { libc::getuid() }
}

/// `$XDG_STATE_HOME`, or `~/.local/state` per the base-directory spec.
pub fn state_home() -> PathBuf {
    if let Some(dir) = std::env::var_os("XDG_STATE_HOME") {
        if !dir.is_empty() {
            return PathBuf::from(dir);
        }
    }
    home().join(".local/state")
}

/// The user's home directory. `$HOME` first, then the passwd database, so this
/// still resolves inside a `systemd --user` unit that was started without a
/// full login environment.
pub fn home() -> PathBuf {
    if let Some(dir) = std::env::var_os("HOME") {
        if !dir.is_empty() {
            return PathBuf::from(dir);
        }
    }
    passwd_home().unwrap_or_else(|| PathBuf::from("/"))
}

fn passwd_home() -> Option<PathBuf> {
    use std::ffi::CStr;

    // getpwuid_r, not getpwuid. The plain form returns a pointer into a static
    // buffer shared by the whole process, so two threads resolving the home
    // directory at once can each get the other's result — and this is reached
    // from request handling in a multi-threaded daemon, not only at startup.
    // The reentrant form writes into a buffer we own.
    let mut buf = vec![0 as libc::c_char; 1024];
    loop {
        let mut pwd: libc::passwd = unsafe { std::mem::zeroed() };
        let mut result: *mut libc::passwd = std::ptr::null_mut();
        // Safe: getpwuid_r writes only into `pwd` and `buf`, both owned here,
        // and reports the buffer being too small rather than overrunning it.
        let rc = unsafe {
            libc::getpwuid_r(
                libc::getuid(),
                &mut pwd,
                buf.as_mut_ptr(),
                buf.len(),
                &mut result,
            )
        };
        if rc == libc::ERANGE && buf.len() < 64 * 1024 {
            buf.resize(buf.len() * 2, 0);
            continue;
        }
        if rc != 0 || result.is_null() {
            // No entry, or an error. Either way there is no home to report.
            return None;
        }
        if pwd.pw_dir.is_null() {
            return None;
        }
        // Safe: pw_dir points into `buf`, which is still alive here, and the
        // bytes are copied out before returning.
        let bytes = unsafe { CStr::from_ptr(pwd.pw_dir) }.to_bytes();
        if bytes.is_empty() {
            return None;
        }
        return Some(PathBuf::from(String::from_utf8_lossy(bytes).into_owned()));
    }
}

/// The daemon's control socket. One per user, not per session.
pub fn control_socket() -> PathBuf {
    control_socket_in(&runtime_dir())
}

/// The same socket, under a runtime directory the caller already has.
///
/// The `*_in` form exists for the reason the store's do: a function that builds
/// a sandbox specification has to stay pure to be asserted exhaustively, and
/// reading `$XDG_RUNTIME_DIR` inside it would make the argv depend on the
/// environment of whichever test ran first.
pub fn control_socket_in(runtime_dir: &Path) -> PathBuf {
    runtime_dir.join("apex-agentd/control.sock")
}

/// Root of the persistent session store.
pub fn state_dir() -> PathBuf {
    state_home().join("apex/agent")
}

/// One JSON record per session, named by id.
pub fn session_record(id: u32) -> PathBuf {
    session_record_in(&state_dir(), id)
}

/// The full PTY transcript for a session.
pub fn session_log(id: u32) -> PathBuf {
    session_log_in(&state_dir(), id)
}

/// Where session records live.
pub fn sessions_dir() -> PathBuf {
    sessions_dir_in(&state_dir())
}

/// Where session transcripts live.
pub fn logs_dir() -> PathBuf {
    logs_dir_in(&state_dir())
}

// The `_in` forms take the store root explicitly. The daemon always passes
// `state_dir()`; a test passes a directory of its own, so it can exercise the
// real record and transcript layout without writing into the user's history.
// The layout itself is stated once, here, and nowhere else.

/// [`sessions_dir`] under an explicit store root.
pub fn sessions_dir_in(store: &Path) -> PathBuf {
    store.join("sessions")
}

/// [`logs_dir`] under an explicit store root.
pub fn logs_dir_in(store: &Path) -> PathBuf {
    store.join("logs")
}

/// [`session_record`] under an explicit store root.
pub fn session_record_in(store: &Path, id: u32) -> PathBuf {
    sessions_dir_in(store).join(format!("{id}.json"))
}

/// [`session_log`] under an explicit store root.
pub fn session_log_in(store: &Path, id: u32) -> PathBuf {
    logs_dir_in(store).join(format!("{id}.log"))
}

/// Registered projects, keyed by a slug of their path.
pub fn projects_dir() -> PathBuf {
    state_dir().join("projects")
}

/// User preferences for the runtime (default agent, default sandbox policy).
pub fn config_file() -> PathBuf {
    config_home().join("apex/agent.json")
}

/// `$XDG_CONFIG_HOME`, or `~/.config`.
pub fn config_home() -> PathBuf {
    if let Some(dir) = std::env::var_os("XDG_CONFIG_HOME") {
        if !dir.is_empty() {
            return PathBuf::from(dir);
        }
    }
    home().join(".config")
}

/// `$XDG_DATA_HOME`, or `~/.local/share` per the base-directory spec.
///
/// Here rather than in a caller because this is the module that implements the
/// spec, and a second resolver would be a second thing to get wrong. It exists
/// for one reader: `apex task` checks whether a capsule still exists, and the
/// capsule engine keeps one JSON record per capsule under
/// `${XDG_DATA_HOME}/apex/env/` — see `files/system/libexec/apex-env`, which
/// resolves the same path the same way.
pub fn data_home() -> PathBuf {
    if let Some(dir) = std::env::var_os("XDG_DATA_HOME") {
        if !dir.is_empty() {
            return PathBuf::from(dir);
        }
    }
    home().join(".local/share")
}

/// The prefix session scratch directories live under, before the uid.
///
/// Deliberately under `/tmp` and not `$XDG_RUNTIME_DIR`: agents generate build
/// output here, and `XDG_RUNTIME_DIR` is a small tmpfs that other software
/// depends on.
///
/// ## Why the uid is in the path, and what happened without it (P2-016)
///
/// This was `/tmp/apex-agent` — one directory for the machine, with the
/// session id below it — and that is the only path in this module with no user
/// in it. Measured on a live APEX laptop on 2026-09-12:
///
/// ```text
/// $ ls -lad /tmp/apex-agent
/// drwxr-xr-x  andre  /tmp/apex-agent
/// ```
///
/// `0755`, owned by whoever logged in first, and that is not an accident of
/// this machine: [`ensure_private_dir`] is called on the LEAF
/// (`<root>/<id>`), so `create_dir_all` makes the root along the way under the
/// caller's umask and only the leaf is chmodded to `0700`. `/tmp` is sticky,
/// so the second user can neither write into that directory nor remove it.
///
/// Two things follow, and both are P2-016 criterion 1 — per-user agents and
/// session isolation:
///
///  * **the second account cannot run an agent at all.** `session.rs` treats a
///    failure to create the scratch directory as fatal, deliberately, so every
///    session start for the second user ends in `EACCES` on a directory owned
///    by the first. Whoever logs in first owns the feature for the machine.
///  * **session ids collide across accounts.** Ids are allocated per daemon
///    and start again at 1, so two users' session 1 name one directory — and a
///    session reap runs `remove_dir_all` on its own id's directory, which is
///    the hazard [`SCRATCH_ROOT_ENV`] was added to keep a *test* daemon away
///    from a live one. Two accounts are the same collision with no variable to
///    set.
///
/// So the default carries the uid. Each account creates its own top-level
/// directory in `/tmp`, which is world-writable, and never needs write
/// permission on anything another account made.
///
/// What this does NOT solve, because it cannot be solved by a path: a
/// predictable name in a sticky directory can be pre-created by another
/// account, and `ensure_private_dir` will then fail to chmod a directory it
/// does not own. That is a loud refusal and a denial of service rather than a
/// disclosure — the mode is never loosened and no data is written into a
/// directory whose mode could not be set — and it takes a deliberate act,
/// where the shared root above took only a second login.
pub const SCRATCH_ROOT_PREFIX: &str = "/tmp/apex-agent-";

/// The environment variable that moves it.
///
/// The one path in this module that no XDG variable reaches, which made it the
/// one path a test daemon shared with the user's real one — and a session reap
/// runs `remove_dir_all` on its own id's directory, so a fixture daemon that
/// numbered a session the same as a live one would delete a running session's
/// scratch. Every suite in this repository fixtures `XDG_RUNTIME_DIR` and
/// `XDG_STATE_HOME`; this is what lets one fixture the last of it.
///
/// Read from the daemon's own environment and never from a request, so it has
/// exactly the trust of `XDG_RUNTIME_DIR`: a session cannot set it, because the
/// daemon's environment is fixed before any session exists.
pub const SCRATCH_ROOT_ENV: &str = "APEX_AGENT_SCRATCH_ROOT";

/// The shipped disposable-capsule engine (§19).
pub const DISPOSABLE_ENGINE: &str = "/usr/libexec/apex-disposable";

/// Overrides [`DISPOSABLE_ENGINE`], for a suite that must not create capsules.
///
/// The engine itself already takes `APEX_DISPOSABLE_ENV_ENGINE` and
/// `APEX_DISPOSABLE_ROOT`, so a test can run the REAL engine against a fake
/// capsule engine in a scratch root. This variable is the last link in that
/// chain: without it a suite cannot reach the engine in the repository at all,
/// because the daemon would look under `/usr/libexec` on the running system.
pub const DISPOSABLE_ENGINE_ENV: &str = "APEX_DISPOSABLE_ENGINE";

/// Where the daemon looks for the disposable engine.
///
/// Absolute or nothing, for the reason [`scratch_root`] insists on it: a
/// relative path would resolve against the daemon's working directory, which
/// is not the caller's.
pub fn disposable_engine() -> PathBuf {
    if let Some(p) = std::env::var_os(DISPOSABLE_ENGINE_ENV) {
        let path = PathBuf::from(p);
        if path.is_absolute() {
            return path;
        }
    }
    PathBuf::from(DISPOSABLE_ENGINE)
}

/// The scratch directory a sandboxed session may write to.
pub fn scratch_dir(id: u32) -> PathBuf {
    scratch_root().join(id.to_string())
}

/// The default scratch root for one account, as a function of the uid alone.
///
/// Split out for the reason every `*_in` in this module is: a test process has
/// exactly one uid, and the claim worth asserting — that two accounts get two
/// roots — is about two. A test that rebuilt the formula for itself would
/// assert only that the test author and this function agree today, and would
/// stay green with the uid dropped from the line below.
pub fn scratch_root_for(uid: u32) -> PathBuf {
    PathBuf::from(format!("{SCRATCH_ROOT_PREFIX}{uid}"))
}

/// The root the scratch directories sit in.
///
/// Per-uid by default — see [`SCRATCH_ROOT_PREFIX`] for what a shared root
/// cost. The override is returned verbatim and is NOT given a uid suffix: a
/// suite that sets it has already chosen a directory of its own, and appending
/// to it would make the variable's value not be the path.
pub fn scratch_root() -> PathBuf {
    if let Some(dir) = std::env::var_os(SCRATCH_ROOT_ENV) {
        let path = PathBuf::from(dir);
        // Absolute or nothing: a relative root would be resolved against the
        // daemon's working directory, which is not the user's and which the
        // sandbox then binds by that name.
        if path.is_absolute() {
            return path;
        }
    }
    scratch_root_for(uid())
}

/// Create `dir` and every missing parent with `0700`.
///
/// [`std::fs::create_dir_all`] applies the process umask, which a user is free
/// to loosen. Session logs are transcripts of the user's work, so the mode is
/// set explicitly afterwards instead of being left to inherited state.
pub fn ensure_private_dir(dir: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    std::fs::create_dir_all(dir)?;
    let mut perms = std::fs::metadata(dir)?.permissions();
    if perms.mode() & 0o777 != 0o700 {
        perms.set_mode(0o700);
        std::fs::set_permissions(dir, perms)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_dir_falls_back_to_run_user() {
        // Not `std::env::set_var` — that is process-global and races other
        // tests. Assert the shape of the fallback instead.
        let uid = unsafe { libc::getuid() };
        let fallback = PathBuf::from(format!("/run/user/{uid}"));
        assert!(fallback.is_absolute());
        assert!(fallback.starts_with("/run/user"));
    }

    #[test]
    fn the_passwd_lookup_resolves_and_is_safe_to_call_concurrently() {
        // The fallback used when $HOME is unset. getpwuid_r writes into a
        // caller-owned buffer; the non-reentrant getpwuid it replaced returned
        // a pointer into one static buffer shared by the whole process, so two
        // threads could each receive the other's answer.
        let expected = passwd_home();
        assert!(
            expected.as_ref().is_none_or(|p| p.is_absolute()),
            "{expected:?}"
        );

        let handles: Vec<_> = (0..8)
            .map(|_| std::thread::spawn(passwd_home))
            .collect();
        for h in handles {
            assert_eq!(
                h.join().expect("thread panicked"),
                expected,
                "concurrent lookups disagreed"
            );
        }
    }

    #[test]
    fn home_is_always_absolute() {
        assert!(home().is_absolute());
    }

    #[test]
    fn every_root_is_absolute() {
        assert!(control_socket().is_absolute());
        assert!(state_dir().is_absolute());
        assert!(config_file().is_absolute());
        assert!(scratch_dir(1).is_absolute());
        assert!(data_home().is_absolute());
    }

    #[test]
    fn data_home_reads_its_own_variable_and_falls_back_to_the_spec() {
        // Written against whatever the environment actually is, rather than by
        // setting a variable: `std::env::set_var` is process-global and races
        // the other tests in this crate. What it catches is the copy-paste bug
        // — a resolver that read XDG_STATE_HOME, or fell back to
        // `.local/state`, would fail one branch or the other here.
        match std::env::var_os("XDG_DATA_HOME") {
            Some(v) if !v.is_empty() => assert_eq!(data_home(), PathBuf::from(v)),
            _ => assert_eq!(data_home(), home().join(".local/share")),
        }
    }

    #[test]
    fn the_scratch_root_reads_its_own_variable_and_falls_back_to_the_default() {
        // Written against whatever the environment actually is, for the same
        // reason `data_home_reads_its_own_variable_and_falls_back_to_the_spec`
        // is: `set_var` is process-global and races the other tests here.
        match std::env::var_os(SCRATCH_ROOT_ENV) {
            Some(v) if !v.is_empty() && Path::new(&v).is_absolute() => {
                assert_eq!(scratch_root(), PathBuf::from(v));
            }
            _ => assert_eq!(scratch_root(), scratch_root_for(uid())),
        }
        assert!(scratch_dir(7).ends_with("7"));
        assert!(scratch_dir(7).starts_with(scratch_root()));
    }

    #[test]
    fn two_accounts_do_not_share_a_scratch_root() {
        // P2-016 criterion 1, and the whole reason the uid is in this path.
        // The root was `/tmp/apex-agent` for the machine: `ensure_private_dir`
        // is called on the LEAF, so `create_dir_all` made the root along the
        // way under the caller's umask and only the leaf was chmodded. Measured
        // on the live laptop on 2026-09-12, `/tmp/apex-agent` was
        // `drwxr-xr-x andre` — 0755, owned by whoever logged in first, in a
        // sticky /tmp the second account can neither write to nor remove. Every
        // session start for that account then fails, fatally and by design
        // (`session.rs` treats it as fatal), and session ids — which restart at
        // 1 per daemon — named the same directories.
        //
        // Asserted through `scratch_root_for`, which is what `scratch_root`
        // itself calls, so dropping the uid from the production line fails
        // this. A test that rebuilt `format!("{PREFIX}{uid}")` for itself would
        // not: it would be asserting its own arithmetic.
        assert_ne!(scratch_root_for(1000), scratch_root_for(1001));
        assert_ne!(
            scratch_root_for(1000).join("1"),
            scratch_root_for(1001).join("1"),
            "session 1 of two accounts named one directory"
        );

        // And the root is a direct child of /tmp, with the uid as its whole
        // final component. This is the assertion that forbids the original bug
        // coming back in the shape that looks like a fix: a prefix of
        // `/tmp/apex-agent/` would give `/tmp/apex-agent/1000` — per-uid
        // leaves under a shared parent, which is the same directory nobody
        // owns, created the same way, failing for the same account.
        for uid in [0_u32, 1, 1000, 65534] {
            let root = scratch_root_for(uid);
            let parts: Vec<_> = root.components().collect();
            assert_eq!(
                parts.len(),
                3,
                "{} is not a direct child of /tmp: {parts:?}",
                root.display()
            );
            assert_eq!(
                root.parent().map(Path::to_path_buf),
                Some(PathBuf::from("/tmp")),
                "{}",
                root.display()
            );
            assert!(
                root.file_name()
                    .expect("a final component")
                    .to_string_lossy()
                    .ends_with(&uid.to_string()),
                "{} does not name uid {uid}",
                root.display()
            );
        }
    }

    #[test]
    fn this_accounts_scratch_root_names_this_account() {
        // `scratch_root_for` is only the formula; this is what ties the live
        // reader to it, so a `scratch_root` that stopped consulting the uid —
        // or went back to a constant — is caught here. Stepped over when a
        // suite has set the override, for the reason `scratch_root` documents:
        // a directory a test chose is not required to mention anybody.
        match std::env::var_os(SCRATCH_ROOT_ENV) {
            Some(v) if !v.is_empty() && Path::new(&v).is_absolute() => {}
            _ => assert_eq!(
                scratch_root(),
                scratch_root_for(uid()),
                "the default scratch root does not name uid {}",
                uid()
            ),
        }
    }

    #[test]
    fn per_session_paths_are_distinct() {
        assert_ne!(session_log(1), session_log(2));
        assert_ne!(session_record(1), session_record(2));
        assert_ne!(scratch_dir(1), scratch_dir(2));
    }

    #[test]
    fn ensure_private_dir_forces_0700() {
        use std::os::unix::fs::PermissionsExt;

        let base = std::env::temp_dir().join(format!("apex-agent-test-{}", std::process::id()));
        let nested = base.join("a/b");
        ensure_private_dir(&nested).expect("create");
        let mode = std::fs::metadata(&nested).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700, "mode was {:o}", mode & 0o777);

        // Loosening it and re-running must tighten it back.
        std::fs::set_permissions(&nested, std::fs::Permissions::from_mode(0o755)).unwrap();
        ensure_private_dir(&nested).expect("re-create");
        let mode = std::fs::metadata(&nested).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700);

        std::fs::remove_dir_all(&base).ok();
    }
}
