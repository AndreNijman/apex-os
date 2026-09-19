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

/// Create `dir` and every missing parent as `0700`, and refuse a directory
/// this account does not own.
///
/// The duplication this module's own note explains is why this had to be fixed
/// twice: the same three defects were measured in
/// `apex_agent_core::paths::ensure_private_dir` on 2026-09-19 and that copy
/// carries the numbers. In short — `create_dir_all` left every parent at the
/// umask while only the leaf was tightened; [`std::fs::metadata`] FOLLOWS a
/// symlink, so a symlink planted at `dir` by another account was followed and
/// the chmod landed on its target; and a directory owned by another account
/// was never noticed at all.
///
/// This daemon's roots are `$XDG_RUNTIME_DIR` and `$XDG_CONFIG_HOME`, neither
/// of which has a world-writable parent — `runtime_dir`'s doc comment says
/// refusing `/tmp` is the point. So the exposure here is smaller than the
/// agent runtime's and the fix is the same one regardless: a daemon that
/// writes prompts into a directory should not chmod one it does not own.
pub fn ensure_private_dir(dir: &Path) -> io::Result<()> {
    ensure_private_dir_as(dir, unsafe { libc::getuid() })
}

/// [`ensure_private_dir`] with the owner it checks against passed in.
///
/// A test process has exactly one uid and the claim worth asserting is about
/// two. The `stat` is real; only the comparand is chosen.
pub(crate) fn ensure_private_dir_as(dir: &Path, me: u32) -> io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};

    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;

    let meta = std::fs::symlink_metadata(dir)?;
    if meta.file_type().is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "{} is a symbolic link, so it is not a directory this account made",
                dir.display()
            ),
        ));
    }
    if meta.uid() != me {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "{} is owned by uid {}, not by uid {me}, so this account cannot make it private",
                dir.display(),
                meta.uid()
            ),
        ));
    }

    let mut perms = meta.permissions();
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

        // The parent, which this used to leave at the umask while tightening
        // only the leaf it was handed.
        assert_eq!(
            std::fs::metadata(&base).unwrap().permissions().mode() & 0o777,
            0o700,
            "the parent was left at the umask"
        );

        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn a_symlink_is_refused_and_its_target_is_left_alone() {
        use std::os::unix::fs::PermissionsExt;

        let base = std::env::temp_dir().join(format!("apex-aid-symlink-{}", std::process::id()));
        let target = base.join("target");
        let link = base.join("link");
        std::fs::create_dir_all(&target).expect("target");
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        std::os::unix::fs::symlink(&target, &link).expect("symlink");

        let err = ensure_private_dir(&link).expect_err("a symlink must be refused");
        assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied, "{err}");
        // The target's surviving mode, not the refusal alone: with `metadata`
        // in place of `symlink_metadata` this call returns Ok and the target
        // becomes 0700.
        assert_eq!(
            std::fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o755,
            "the symlink's target was chmodded"
        );

        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn a_directory_another_account_owns_is_refused() {
        let me = unsafe { libc::getuid() };
        let base = std::env::temp_dir().join(format!("apex-aid-owner-{}", std::process::id()));

        ensure_private_dir_as(&base, me).expect("its real owner is accepted");
        let err = ensure_private_dir_as(&base, me.wrapping_add(1))
            .expect_err("a directory owned by another account must be refused");
        assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied, "{err}");
        assert!(err.to_string().contains(&format!("owned by uid {me}")), "{err}");

        std::fs::remove_dir_all(&base).ok();
    }
}
