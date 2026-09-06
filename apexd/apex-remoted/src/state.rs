//! What the service holds while it runs.
//!
//! One `State` per process, shared by the control socket, the LAN listener
//! and every device connection. Everything mutable is behind its own mutex,
//! and the locks are held for as long as one map operation takes: a device
//! connection that held the store lock while it proxied a terminal would
//! stall `apex remote revoke`, which is the one command that must never wait.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use apex_remote_core::device::{DeviceStore, StoreError};
use apex_remote_core::identity::Identity;
use apex_remote_core::pairing::Offer;

/// A live device connection, as far as the rest of the service needs to know.
///
/// Kept so that revoking a device can end the connection it is already
/// holding. Without it, revocation would mean "stop letting it back in",
/// which is not what the owner of a lost phone is asking for.
pub struct Live {
    /// The device this connection authenticated as.
    pub device_id: String,
    /// A handle that closes the socket when asked. `shutdown(2)` rather than
    /// a flag the connection thread checks, because the thread is blocked in
    /// a read and would not check anything until the phone sent something.
    pub socket: std::net::TcpStream,
}

pub struct State {
    /// This machine's long-term keypair.
    pub identity: Identity,
    /// What the QR code calls this machine.
    pub machine: String,
    /// The port the LAN listener is on.
    pub port: u16,
    /// The relay base URL, when the owner configured one.
    pub relay: Option<String>,
    /// Where the device store lives.
    pub store_path: PathBuf,
    devices: Mutex<DeviceStore>,
    /// The pairing offer, when one is open. In memory only: a token that
    /// survived a restart would be one the owner did not ask to survive.
    pub offer: Mutex<Option<Offer>>,
    live: Mutex<Vec<Live>>,
}

impl State {
    pub fn new(
        identity: Identity,
        machine: String,
        port: u16,
        relay: Option<String>,
        store_path: PathBuf,
    ) -> Result<Arc<State>, StoreError> {
        let devices = DeviceStore::load(&store_path)?;
        Ok(Arc::new(State {
            identity,
            machine,
            port,
            relay,
            store_path,
            devices: Mutex::new(devices),
            offer: Mutex::new(None),
            live: Mutex::new(Vec::new()),
        }))
    }

    /// The device store, locked.
    pub fn devices(&self) -> Result<std::sync::MutexGuard<'_, DeviceStore>, StoreError> {
        self.devices
            .lock()
            .map_err(|_| StoreError::Corrupt("the device store lock was poisoned".into()))
    }

    /// Persist a store the caller has changed.
    pub fn save_devices(&self, store: &DeviceStore) -> Result<(), StoreError> {
        store.save(&self.store_path)
    }

    /// How long the open pairing offer has left, or `None`.
    pub fn offer_ms_left(&self, now_ms: u64) -> Option<u64> {
        let offer = self.offer.lock().ok()?;
        let offer = offer.as_ref()?;
        offer
            .is_live(now_ms)
            .then(|| offer.expires_ms().saturating_sub(now_ms))
    }

    /// Remember a live connection.
    pub fn register(&self, live: Live) {
        if let Ok(mut v) = self.live.lock() {
            v.push(live);
        }
    }

    /// Forget one, by the socket it holds.
    pub fn unregister(&self, device_id: &str, peer: Option<SocketAddr>) {
        if let Ok(mut v) = self.live.lock() {
            v.retain(|l| {
                l.device_id != device_id || l.socket.peer_addr().ok() != peer || peer.is_none()
            });
        }
    }

    /// End every connection a device is holding.
    ///
    /// `shutdown` on the socket, which unblocks the reading thread with an
    /// end of file. A flag the thread checked would do nothing until the
    /// phone next sent a byte, and a phone in someone else's pocket may not
    /// send one for hours.
    pub fn drop_connections_for(&self, device_id: &str) {
        if let Ok(mut v) = self.live.lock() {
            v.retain(|l| {
                if l.device_id == device_id {
                    let _ = l.socket.shutdown(std::net::Shutdown::Both);
                    false
                } else {
                    true
                }
            });
        }
    }

    /// The addresses a device on this network could dial.
    ///
    /// Every non-loopback address this machine has, with the listener's port.
    /// Loopback is excluded because it is never reachable from a phone and
    /// putting it in a QR code costs a device one failed connection attempt
    /// before it moves on.
    pub fn lan_addresses(&self) -> Vec<String> {
        crate::net::local_addresses()
            .into_iter()
            .map(|ip| format!("{ip}:{}", self.port))
            .collect()
    }
}

/// Where the control socket lives.
pub fn control_socket_in(runtime_dir: &Path) -> PathBuf {
    runtime_dir.join("apex-remoted").join("control.sock")
}

/// Where the control socket lives for this user.
pub fn control_socket() -> PathBuf {
    control_socket_in(&apex_agent_core::paths::runtime_dir())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_control_socket_is_beside_the_agent_runtimes_and_not_in_it() {
        // A separate directory, because the two daemons have separate
        // lifetimes: `apex agent enable` starts one and does not start the
        // other, and a socket inside the other's directory would make the
        // second depend on the first having run.
        let dir = Path::new("/run/user/1000");
        let mine = control_socket_in(dir);
        assert!(mine.ends_with("apex-remoted/control.sock"), "{mine:?}");
        assert_ne!(mine, apex_agent_core::paths::control_socket());
    }
}
