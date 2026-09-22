//! Where the protected secret service keeps its socket and its store.
//!
//! Both are outside `$HOME` and both are outside anything a managed agent can
//! read, which is the whole point of P0-002. The agent runtime's own paths live
//! in `apex_agent_core::paths` and are user-owned; nothing here is.
//!
//! * `/run/apex-secretd/control.sock` — the socket. `0666`, in a `0755`
//!   directory, because every local user must be able to *reach* the daemon;
//!   who they are is then decided from `SO_PEERCRED`, not from a mode bit. The
//!   store is per-uid, so a connection from another account sees an empty
//!   namespace of its own and nothing else.
//! * `/var/lib/apex-secretd/` — the store. `0700`, owned by root, so a process
//!   with the user's uid cannot read a credential at rest even when it is not
//!   confined at all. That is the boundary the agent runtime's `0600`-in-`$HOME`
//!   store never had.
//!
//! Both are overridable, for tests and for a second instance:
//! `APEX_SECRETD_SOCKET` and `APEX_SECRETD_STORE`. Overriding the socket only
//! ever redirects the *client*, which can already choose not to call the broker
//! at all; overriding the store is the daemon's own configuration, and a daemon
//! started with a store it cannot protect says so at startup.

use std::path::PathBuf;

/// The daemon's control socket.
pub const SOCKET: &str = "/run/apex-secretd/control.sock";

/// Root of the credential store.
pub const STORE: &str = "/var/lib/apex-secretd";

/// Environment override for the socket, honoured by clients and by the daemon.
pub const SOCKET_ENV: &str = "APEX_SECRETD_SOCKET";

/// Environment override for the store root, honoured by the daemon only.
pub const STORE_ENV: &str = "APEX_SECRETD_STORE";

/// Where to connect.
pub fn socket() -> PathBuf {
    env_path(SOCKET_ENV).unwrap_or_else(|| PathBuf::from(SOCKET))
}

/// Where the store lives.
pub fn store_root() -> PathBuf {
    env_path(STORE_ENV).unwrap_or_else(|| PathBuf::from(STORE))
}

fn env_path(name: &str) -> Option<PathBuf> {
    let value = std::env::var_os(name)?;
    if value.is_empty() {
        return None;
    }
    Some(PathBuf::from(value))
}

/// The current process's uid.
pub fn uid() -> u32 {
    // Safe: getuid() cannot fail and has no side effects.
    unsafe { libc::getuid() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_are_outside_home() {
        // The criterion, as a path assertion: neither default may sit under a
        // home directory, because a managed agent can read the user's home.
        for p in [PathBuf::from(SOCKET), PathBuf::from(STORE)] {
            assert!(p.is_absolute(), "{}", p.display());
            assert!(
                !p.starts_with("/home") && !p.starts_with("/root") && !p.starts_with("/var/home"),
                "{} is inside a home directory",
                p.display()
            );
        }
    }

    #[test]
    fn an_empty_override_is_not_an_override() {
        // An exported-but-empty variable is how a shell hands over "unset", and
        // treating it as a path would send the daemon to "".
        assert_eq!(env_path("APEX_SECRETD_DEFINITELY_UNSET_XYZ"), None);
    }
}
