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
use apex_remote_core::rendezvous::{Connection, Path as RemotePath};

use crate::relay::Sources;

/// A live device connection, as far as the rest of the service needs to know.
///
/// Kept so that revoking a device can end the connection it is already
/// holding. Without it, revocation would mean "stop letting it back in",
/// which is not what the owner of a lost phone is asking for.
pub struct Live {
    /// The device this connection authenticated as.
    pub device_id: String,
    /// Which of that device's connections this is.
    ///
    /// Read once when the connection is registered, not on the way out: after
    /// a `shutdown(2)` there is no address to read, so a version that asked
    /// the socket at removal time would match nothing and the list would grow
    /// one entry per connection for the daemon's life.
    pub peer: Option<SocketAddr>,
    /// A handle that closes the socket when asked. `shutdown(2)` rather than
    /// a flag the connection thread checks, because the thread is blocked in
    /// a read and would not check anything until the phone sent something.
    pub socket: std::net::TcpStream,
    /// What the device calls itself, so a listing does not have to go back to
    /// the store for a name it already had.
    pub device_name: String,
    /// Which path this connection arrived on.
    pub path: RemotePath,
    /// Unix milliseconds at which it authenticated.
    pub since_ms: u64,
    /// The most recent measured round trip, written by the frame loop's
    /// `Pong` arm and read by anybody asking for status.
    ///
    /// Shared rather than copied: the measurement happens on the connection's
    /// own thread and the reader is the control socket's, and a value that
    /// had to be pushed somewhere on every ping would be a lock held across
    /// a write.
    pub rtt_ms: Arc<Mutex<Option<u64>>>,
}

pub struct State {
    /// This machine's long-term keypair.
    pub identity: Identity,
    /// What the QR code calls this machine.
    pub machine: String,
    /// The port the LAN listener is on.
    pub port: u16,
    /// The relay base URL, when the owner configured one.
    ///
    /// Kept as the owner wrote it, because that is what `apex remote status`
    /// shows and what a QR code carries. The parsed form is
    /// [`State::relay_endpoint`].
    pub relay: Option<String>,
    /// Which loopback source ports are relay splices right now.
    ///
    /// A relayed session arrives on this daemon's own listener from
    /// `127.0.0.1`, so the source port is the only thing that distinguishes
    /// it from a LAN session, and the splice arms it before the first byte.
    pub relay_sources: Sources,
    /// Where the device store lives.
    pub store_path: PathBuf,
    /// How often an open connection is measured.
    pub ping_interval: std::time::Duration,
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
        ping_interval: std::time::Duration,
    ) -> Result<Arc<State>, StoreError> {
        let devices = DeviceStore::load(&store_path)?;
        Ok(Arc::new(State {
            identity,
            machine,
            port,
            relay,
            store_path,
            ping_interval,
            relay_sources: Sources::default(),
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

    /// Forget one connection, leaving the device's others alone.
    ///
    /// The first version of this ended its condition with `|| peer.is_none()`
    /// and was called with `None`, so the retain predicate was true for every
    /// entry and nothing was ever removed. A leak rather than a hole, and the
    /// kind that is invisible until a daemon has been up for a week.
    pub fn unregister(&self, device_id: &str, peer: Option<SocketAddr>) {
        if let Ok(mut v) = self.live.lock() {
            v.retain(|l| !(l.device_id == device_id && l.peer == peer));
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

    /// Every connection open right now, with its path and its round trip.
    ///
    /// P1-052's last criterion. Live only: nothing here is read back from the
    /// device store, because a quality measured during a session that has
    /// ended is not a fact about now and a page that showed one would be
    /// telling the owner about a connection that does not exist.
    pub fn connections(&self) -> Vec<Connection> {
        let Ok(live) = self.live.lock() else {
            return Vec::new();
        };
        live.iter()
            .map(|l| Connection {
                device_id: l.device_id.clone(),
                device_name: l.device_name.clone(),
                path: l.path.as_str().to_string(),
                since_ms: l.since_ms,
                rtt_ms: l.rtt_ms.lock().ok().and_then(|v| *v),
            })
            .collect()
    }

    /// How a connection from this address reached the machine.
    ///
    /// Relay only when BOTH halves say so: the address is loopback *and* its
    /// port is one a splice has armed. The kernel reuses ephemeral ports, and
    /// a check on the port alone would eventually label a LAN session — or a
    /// local one somebody made by hand — as relayed, which is a lie in the
    /// direction that matters, because `Path::disclosure` tells the owner a
    /// third party was involved.
    pub fn path_of(&self, peer: Option<SocketAddr>) -> RemotePath {
        match peer {
            Some(addr) if addr.ip().is_loopback() && self.relay_sources.holds(addr.port()) => {
                RemotePath::Relay
            }
            _ => RemotePath::Lan,
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
    fn a_connection_is_registered_and_then_forgotten() {
        // The leak this had: `unregister` ended its condition with
        // `|| peer.is_none()` and was called with `None`, so the predicate
        // was true for every entry and the list only ever grew.
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind");
        let addr = listener.local_addr().expect("addr");
        // The ACCEPTED sockets, which is what the daemon holds: their
        // `peer_addr` is the client's ephemeral port and differs per
        // connection. The client-side sockets all report the listener's one
        // address, which is what made the first version of this test assert
        // two equal values.
        let _c1 = std::net::TcpStream::connect(addr).expect("connect");
        let _c2 = std::net::TcpStream::connect(addr).expect("connect");
        let (s1, _) = listener.accept().expect("accept");
        let (s2, _) = listener.accept().expect("accept");
        let p1 = s1.peer_addr().ok();
        let p2 = s2.peer_addr().ok();
        assert_ne!(p1, p2, "two connections share an address");

        let sample = |peer, socket| Live {
            device_id: "d".into(),
            peer,
            socket,
            device_name: "a phone".into(),
            path: RemotePath::Lan,
            since_ms: 0,
            rtt_ms: Arc::new(Mutex::new(None)),
        };
        let live = Mutex::new(vec![sample(p1, s1), sample(p2, s2)]);
        // Exercised through the same predicate the daemon uses, over a
        // standalone list so no daemon has to be started.
        let remove = |v: &mut Vec<Live>, id: &str, peer: Option<SocketAddr>| {
            v.retain(|l| !(l.device_id == id && l.peer == peer));
        };
        let mut v = live.into_inner().expect("lock");
        remove(&mut v, "d", p1);
        assert_eq!(v.len(), 1, "the wrong number of connections survived");
        assert_eq!(v[0].peer, p2, "the wrong connection was removed");
        remove(&mut v, "d", p2);
        assert!(v.is_empty());
    }

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
