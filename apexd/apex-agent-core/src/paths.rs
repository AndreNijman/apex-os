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
//! * per-session scratch lives under `/tmp/apex-agent/<id>` — it is the one
//!   writable path outside the project that a sandboxed agent gets, and it is
//!   removed with the session.
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
    runtime_dir().join("apex-agentd/control.sock")
}

/// Root of the persistent session store.
pub fn state_dir() -> PathBuf {
    state_home().join("apex/agent")
}

/// One JSON record per session, named by id.
pub fn session_record(id: u32) -> PathBuf {
    state_dir().join("sessions").join(format!("{id}.json"))
}

/// The full PTY transcript for a session.
pub fn session_log(id: u32) -> PathBuf {
    state_dir().join("logs").join(format!("{id}.log"))
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

/// The scratch directory a sandboxed session may write to. Deliberately under
/// `/tmp` and not `$XDG_RUNTIME_DIR`: agents generate build output here and
/// `XDG_RUNTIME_DIR` is a small tmpfs that other software depends on.
pub fn scratch_dir(id: u32) -> PathBuf {
    PathBuf::from(format!("/tmp/apex-agent/{id}"))
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
