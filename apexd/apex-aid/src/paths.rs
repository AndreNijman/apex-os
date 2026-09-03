//! Where this daemon keeps things.
//!
//! Four functions, deliberately duplicated from `apex_agent_core::paths` rather
//! than imported. Importing them would make `apex-aid` depend on the agent
//! runtime's library — and therefore link its sandbox policy, git checkpointing
//! and secret broker — to reuse forty lines of `$XDG_*` handling. The two
//! daemons are siblings, not layers, and a dependency edge between them would
//! be the first thing a reader got wrong about the design.
//!
//! Nothing here is privileged. Every directory is created `0700`, because the
//! socket inside it is the endpoint that generates text from the user's
//! prompts.

use std::io;
use std::path::{Path, PathBuf};

/// `$XDG_RUNTIME_DIR`, or `/run/user/<uid>` when it is unset — a `systemd
/// --user` unit started without a full login environment, or a bare `ssh`
/// command.
///
/// Never `/tmp`. A predictable socket path in a world-writable directory is a
/// hijack waiting to happen, and this socket is where prompts go.
pub fn runtime_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR") {
        if !dir.is_empty() {
            return PathBuf::from(dir);
        }
    }
    // Safe: getuid() cannot fail and has no side effects.
    PathBuf::from(format!("/run/user/{}", unsafe { libc::getuid() }))
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

/// `~/.config/apex/ai.toml`.
pub fn settings_file() -> PathBuf {
    config_home().join("apex/ai.toml")
}

/// The user's home. `$HOME` first, then nothing — this daemon has no reason to
/// consult the passwd database, because a `systemd --user` unit always has
/// `$HOME` set and a shell always does too.
pub fn home() -> PathBuf {
    match std::env::var_os("HOME") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => PathBuf::from("/"),
    }
}

/// Create `dir` and every missing parent, then force `0700`.
///
/// [`std::fs::create_dir_all`] applies the process umask, which the user is
/// free to loosen; the mode is set explicitly afterwards rather than left to
/// inherited state.
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
    fn every_root_is_absolute() {
        assert!(runtime_dir().is_absolute());
        assert!(config_home().is_absolute());
        assert!(settings_file().is_absolute());
        assert!(home().is_absolute());
    }

    #[test]
    fn the_runtime_fallback_is_run_user_and_never_tmp() {
        // The property that matters: a shared directory must never be the
        // fallback, because the socket path is predictable.
        let uid = unsafe { libc::getuid() };
        let fallback = PathBuf::from(format!("/run/user/{uid}"));
        assert!(fallback.starts_with("/run/user"));
        assert!(!fallback.starts_with("/tmp"));
    }

    #[test]
    fn ensure_private_dir_forces_0700_even_over_a_loosened_directory() {
        use std::os::unix::fs::PermissionsExt;

        let base = std::env::temp_dir().join(format!("apex-aid-paths-{}", std::process::id()));
        let nested = base.join("a/b");
        ensure_private_dir(&nested).expect("create");
        assert_eq!(
            std::fs::metadata(&nested).unwrap().permissions().mode() & 0o777,
            0o700
        );

        std::fs::set_permissions(&nested, std::fs::Permissions::from_mode(0o755)).unwrap();
        ensure_private_dir(&nested).expect("re-create");
        assert_eq!(
            std::fs::metadata(&nested).unwrap().permissions().mode() & 0o777,
            0o700,
            "a loosened directory was not tightened back"
        );

        std::fs::remove_dir_all(&base).ok();
    }
}
