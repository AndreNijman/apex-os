//! Moving bytes between a client and the backend.
//!
//! ── Why this daemon does not speak HTTP ────────────────────────────────────
//!
//! The API endpoint carries the **backend's own HTTP API, unmodified**. A
//! proxy that parsed it would be a second HTTP implementation to keep correct,
//! and every header, trailer or streaming detail it failed to forward would
//! surface as a bug in somebody else's client — an editor plugin, an agent, a
//! `curl` one-liner. Relaying bytes is not laziness: it is what makes "one APEX
//! local-inference API" a promise about the *endpoint and its lifecycle*, which
//! APEX genuinely owns, rather than a promise about a wire format it would
//! then be responsible for reimplementing.
//!
//! What APEX adds is everything around it: which runtime, on which backend,
//! with how many layers offloaded, started on demand, stopped when idle,
//! reachable only by its owner.
//!
//! ── Why two threads and not one ────────────────────────────────────────────
//!
//! A generation is duplex: the client is still uploading a long prompt while
//! the first tokens come back, and an SSE stream stays open with nothing moving
//! for seconds at a time. Copying one direction and then the other would
//! deadlock the moment either side did that. Two `std::io::copy` calls in
//! opposite directions is the whole implementation, and each closes its
//! *write* half on EOF so the peer sees the end of the stream rather than
//! waiting for a timeout.

use std::io;
use std::net::Shutdown;
use std::os::unix::net::UnixStream;

/// Bytes moved in one relay, for the idle bookkeeping.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Moved {
    /// Client to backend.
    pub up: u64,
    /// Backend to client.
    pub down: u64,
}

impl Moved {
    /// Total, for a log line.
    pub fn total(&self) -> u64 {
        self.up.saturating_add(self.down)
    }
}

/// Relay until both directions are done.
///
/// Returns when the client and the backend have both finished. Errors are
/// reported in the return value rather than propagated per direction: a client
/// that closes mid-stream is ordinary, not a fault, and a relay that returned
/// early would leave the other direction's thread writing into a closed socket.
pub fn duplex(client: UnixStream, backend: UnixStream) -> io::Result<Moved> {
    let client_r = client;
    let backend_r = backend.try_clone()?;
    let client_w = client_r.try_clone()?;
    let backend_w = backend;

    // `scope` rather than detached threads: both halves must be finished before
    // the caller decrements the connection count, or an "idle" daemon could
    // unload a backend that is still streaming.
    let moved = std::thread::scope(|s| {
        let up = s.spawn(move || {
            let mut r = client_r;
            let mut w = backend_w;
            let n = io::copy(&mut r, &mut w).unwrap_or(0);
            // Tell the backend the request body is complete. Without this a
            // server waiting for the end of a chunked upload never replies.
            let _ = w.shutdown(Shutdown::Write);
            n
        });
        let down = s.spawn(move || {
            let mut r = backend_r;
            let mut w = client_w;
            let n = io::copy(&mut r, &mut w).unwrap_or(0);
            let _ = w.shutdown(Shutdown::Write);
            n
        });
        Moved {
            up: up.join().unwrap_or(0),
            down: down.join().unwrap_or(0),
        }
    });
    Ok(moved)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    #[test]
    fn bytes_travel_in_both_directions() {
        // A real pair of socketpairs standing in for the client and the backend,
        // so this exercises the actual copy and shutdown logic rather than a
        // model of it.
        let (client, client_peer) = UnixStream::pair().unwrap();
        let (backend, backend_peer) = UnixStream::pair().unwrap();

        // The "client" sends a request and reads a reply.
        let c = std::thread::spawn(move || {
            let mut s = client_peer;
            s.write_all(b"POST /v1/chat/completions").unwrap();
            s.shutdown(Shutdown::Write).unwrap();
            let mut got = String::new();
            s.read_to_string(&mut got).unwrap();
            got
        });
        // The "backend" reads it and answers.
        let b = std::thread::spawn(move || {
            let mut s = backend_peer;
            let mut got = String::new();
            s.read_to_string(&mut got).unwrap();
            s.write_all(b"data: {\"choices\":[]}\n\n").unwrap();
            s.shutdown(Shutdown::Write).unwrap();
            got
        });

        let moved = duplex(client, backend).expect("relay");
        assert_eq!(c.join().unwrap(), "data: {\"choices\":[]}\n\n");
        assert_eq!(b.join().unwrap(), "POST /v1/chat/completions");
        assert_eq!(moved.up, 25);
        assert_eq!(moved.down, 22);
        assert_eq!(moved.total(), 47);
    }

    #[test]
    fn a_client_that_hangs_up_first_does_not_hang_the_relay() {
        // The ordinary case, not a fault: someone pressed Ctrl-C.
        let (client, client_peer) = UnixStream::pair().unwrap();
        let (backend, backend_peer) = UnixStream::pair().unwrap();
        drop(client_peer);
        let b = std::thread::spawn(move || {
            let mut s = backend_peer;
            let mut got = Vec::new();
            let _ = s.read_to_end(&mut got);
            // Writing into a client that is gone must not panic the relay.
            let _ = s.write_all(b"late");
        });
        let moved = duplex(client, backend).expect("relay");
        b.join().unwrap();
        assert_eq!(moved.up, 0);
    }

    #[test]
    fn a_backend_that_dies_mid_stream_ends_the_relay() {
        let (client, client_peer) = UnixStream::pair().unwrap();
        let (backend, backend_peer) = UnixStream::pair().unwrap();
        let c = std::thread::spawn(move || {
            let mut s = client_peer;
            let _ = s.write_all(b"x");
            let _ = s.shutdown(Shutdown::Write);
            let mut got = Vec::new();
            let _ = s.read_to_end(&mut got);
            got.len()
        });
        // The backend reads nothing and vanishes.
        drop(backend_peer);
        let moved = duplex(client, backend).expect("relay");
        assert_eq!(moved.down, 0);
        assert_eq!(c.join().unwrap(), 0);
    }

    #[test]
    fn the_total_saturates_rather_than_overflowing() {
        assert_eq!(Moved { up: u64::MAX, down: 1 }.total(), u64::MAX);
    }
}
