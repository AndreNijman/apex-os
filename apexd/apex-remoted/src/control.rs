//! The local control socket: what `apex remote` says to this service.
//!
//! Newline-delimited JSON on a Unix socket in `$XDG_RUNTIME_DIR`, exactly
//! like `apex-agentd`'s, because a second framing convention on a second
//! socket in the same runtime directory would be two things to learn for no
//! gain.
//!
//! ## The one rule this socket enforces by itself
//!
//! **Only a human at this machine may open a pairing offer.** P1-051's fourth
//! criterion is "pairing cannot be completed silently by an agent", and this
//! is the half of it that a policy can enforce; the other half is that an
//! offer expires on its own and is single-use, which lives in
//! `apex_remote_core::pairing`.
//!
//! The check is the same one `apex-agentd`'s `privilege::decide` uses for
//! approving a root operation: [`apex_agent_core::origin::observe_pid`] on
//! the peer's own pid, and a refusal unless the answer is one of §7's local
//! origins. An agent inside a managed session is under `user@N.service` and
//! classifies as `scheduled-job`; a person in a terminal is under
//! `session-N.scope` and classifies as `local-terminal`. Neither can move
//! itself between the two.
//!
//! Listing and revoking are *not* gated that way. Revoking a device is the
//! safe direction — an agent that revokes the owner's phone is a nuisance,
//! not an escalation, and the owner is the only one who can pair it back.
//! Refusing a revoke from a non-local caller would mean a script cannot
//! respond to a lost phone, which is the case where speed matters most.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::Arc;

use apex_agent_core::policy::RequestOrigin;
use apex_remote_core::pairing::{Offer, PairingOffer};
use serde::{Deserialize, Serialize};

use crate::state::State;

/// What `apex remote` can ask for.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Request {
    /// Is the service up, what is its identity, where can it be reached.
    Status,
    /// Open a pairing offer and return the text that goes in the QR code.
    Pair,
    /// Every device, paired and revoked.
    Devices,
    /// Take a device's access away.
    Revoke { device: String },
}

/// What it answers.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "reply", rename_all = "snake_case")]
pub enum Reply {
    Status {
        version: u32,
        /// This machine's static public key, base64url. Public by design.
        key: String,
        machine: String,
        /// LAN addresses a device can dial, `host:port`.
        lan: Vec<String>,
        /// The relay this machine would fall back to, when one is configured.
        relay: Option<String>,
        /// Where a device would meet this machine on that relay.
        rendezvous: String,
        /// Devices that are paired and not revoked.
        paired: usize,
        /// Whether a pairing offer is open right now, and for how much longer.
        offer_ms_left: Option<u64>,
        /// Every connection open right now, with the path it came in on and
        /// the last round trip measured on it.
        ///
        /// P1-052's last criterion, on the desktop side. Live rather than
        /// remembered: a quality from a session that has ended is not a fact
        /// about now.
        #[serde(default)]
        connections: Vec<apex_remote_core::rendezvous::Connection>,
    },
    /// A pairing offer. `qr` is the whole payload; `expires_ms` is when it
    /// stops being accepted.
    Offer { qr: String, expires_ms: u64 },
    Devices {
        devices: Vec<apex_remote_core::device::Device>,
    },
    Ok,
    Error { message: String },
}

impl Reply {
    fn error(message: impl Into<String>) -> Reply {
        Reply::Error {
            message: message.into(),
        }
    }
}

/// Serve the control socket forever.
pub fn serve(listener: UnixListener, state: Arc<State>) {
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let state = Arc::clone(&state);
        // One thread per connection, like `apex-agentd`. These connections are
        // short and rare — a person typing `apex remote devices` — so a
        // thread each costs nothing and keeps a slow client from blocking the
        // next one.
        std::thread::spawn(move || {
            let _ = handle(stream, &state);
        });
    }
}

fn handle(stream: UnixStream, state: &State) -> std::io::Result<()> {
    // Peer credentials once, from the accepted socket, before a request is
    // parsed. The kernel filled them in at `connect(2)`; anything in a request
    // line is whatever the client chose to send.
    let peer = crate::peer::credentials(&stream);
    let mut writer = stream.try_clone()?;
    let mut reader = BufReader::new(stream);
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            return Ok(());
        }
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Request>(line) {
            Ok(req) => dispatch(req, peer, state),
            Err(e) => Reply::error(format!("unparseable request: {e}")),
        };
        let mut out = serde_json::to_string(&reply).unwrap_or_else(|e| {
            format!(r#"{{"reply":"error","message":"could not serialise a reply: {e}"}}"#)
        });
        out.push('\n');
        writer.write_all(out.as_bytes())?;
        writer.flush().ok();
    }
}

fn dispatch(req: Request, peer: Option<crate::peer::Peer>, state: &State) -> Reply {
    match req {
        Request::Status => status(state),
        Request::Pair => match local_caller(peer) {
            Ok(()) => pair(state),
            Err(why) => Reply::error(why),
        },
        Request::Devices => match state.devices() {
            Ok(store) => Reply::Devices {
                devices: store.list().into_iter().cloned().collect(),
            },
            Err(e) => Reply::error(e.to_string()),
        },
        Request::Revoke { device } => revoke(state, &device),
    }
}

/// Refuse anything but a human at this machine.
///
/// The message names what was observed, because "permission denied" from a
/// service the user just started is a mystery and "you are a scheduled-job"
/// is a fact they can act on.
fn local_caller(peer: Option<crate::peer::Peer>) -> Result<(), String> {
    let Some(peer) = peer else {
        return Err(
            "the kernel would not report the peer credentials of this connection, so there is no \
             way to tell whether a human is at this machine — and pairing a device is reserved \
             for one"
                .to_string(),
        );
    };
    if !crate::peer::is_own_user(&peer) {
        return Err("that connection does not belong to this user".to_string());
    }
    may_pair(apex_agent_core::origin::observe_pid(peer.pid)?)
}

/// The rule, over a value.
///
/// Split out from [`local_caller`] because of what a mutation found: with the
/// check replaced by `if true`, every test in this file still passed. They all
/// run as whatever `cargo test` is, which on a developer's machine is a login
/// session and therefore local, so nothing ever reached the refusal. A test
/// that cannot fail is not a test, and the way to make this one able to fail
/// is to ask the rule about all seven origins rather than about this process.
pub fn may_pair(origin: RequestOrigin) -> Result<(), String> {
    if MAY_PAIR.contains(&origin) {
        return Ok(());
    }
    Err(format!(
        "a {origin} request cannot pair a device: pairing hands a phone standing access to this \
         machine's agents, so §7 reserves it for a human at the keyboard. Run `apex remote pair` \
         from a terminal or from APEX Settings"
    ))
}

fn status(state: &State) -> Reply {
    let paired = state
        .devices()
        .map(|s| s.list().iter().filter(|d| d.is_active()).count())
        .unwrap_or(0);
    let now = apex_remote_core::now_ms();
    Reply::Status {
        version: apex_remote_core::REMOTE_PROTOCOL_VERSION,
        key: state.identity.public_key(),
        machine: state.machine.clone(),
        lan: state.lan_addresses(),
        relay: state.relay.clone(),
        rendezvous: apex_remote_core::rendezvous::rendezvous_id(&state.identity.public_bytes()),
        paired,
        offer_ms_left: state.offer_ms_left(now),
        connections: state.connections(),
    }
}

fn pair(state: &State) -> Reply {
    let now = apex_remote_core::now_ms();
    let offer = Offer::new(now);
    let payload = PairingOffer {
        v: apex_remote_core::REMOTE_PROTOCOL_VERSION,
        machine: state.machine.clone(),
        key: state.identity.public_key(),
        token: offer.token_base64(),
        lan: state.lan_addresses(),
        relay: state.relay.clone(),
        expires_ms: offer.expires_ms(),
    };
    let expires_ms = offer.expires_ms();
    // Replacing any offer already open, deliberately. Two live tokens would
    // mean a QR shown five minutes ago still pairs while the owner is looking
    // at a fresh one, and the owner would have no way to tell.
    *state.offer.lock().expect("offer lock") = Some(offer);
    Reply::Offer {
        qr: payload.encode(),
        expires_ms,
    }
}

fn revoke(state: &State, device: &str) -> Reply {
    let mut store = match state.devices() {
        Ok(s) => s,
        Err(e) => return Reply::error(e.to_string()),
    };
    let now = apex_remote_core::now_ms();
    let revoked = match store.revoke(device, now) {
        Ok(d) => d,
        Err(e) => return Reply::error(e.to_string()),
    };
    if let Err(e) = state.save_devices(&store) {
        return Reply::error(format!("the device was not written: {e}"));
    }
    // A revoked device with a live connection is still holding one. The store
    // is what stops it reconnecting; this is what ends the connection it
    // already has, and without it "revoke" would mean "revoke, eventually".
    state.drop_connections_for(&revoked.id);
    Reply::Ok
}

/// The origins this socket accepts a pairing request from.
///
/// Written out and matched against, rather than left implicit in
/// `is_local()`. The two mean the same thing today and they are different
/// claims: `is_local` is §7's answer to "is a human present", and this is
/// this socket's answer to "may this caller hand a phone standing access to
/// the machine". A test pins them together, so a future §7 origin that is
/// local for one purpose and not the other has to be decided rather than
/// inherited.
const MAY_PAIR: [RequestOrigin; 2] = [RequestOrigin::LocalTerminal, RequestOrigin::ApexShell];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_origin_that_may_pair_is_local_and_no_other_is() {
        // The two lists have to agree, and the reason to assert it here is
        // that `MAY_PAIR` is what a reader checks against §7 while
        // `is_local()` is what the code runs.
        for o in RequestOrigin::ALL {
            assert_eq!(
                MAY_PAIR.contains(o),
                o.is_local(),
                "{o} disagrees between MAY_PAIR and is_local"
            );
        }
        assert_eq!(MAY_PAIR.len(), 2);
    }

    #[test]
    fn a_connection_with_no_peer_credentials_may_not_pair() {
        let why = local_caller(None).expect_err("an unidentifiable peer paired a device");
        assert!(why.contains("a human is at this machine"), "{why}");
    }

    #[test]
    fn exactly_the_two_local_origins_may_pair_and_the_other_five_are_refused_by_name() {
        // Over §7's whole vocabulary, not over whatever this process happens
        // to be. The version of this file that asked only about the running
        // process passed with the check replaced by `if true`.
        let mut allowed = 0;
        for o in RequestOrigin::ALL {
            match may_pair(*o) {
                Ok(()) => {
                    assert!(o.is_local(), "{o} was allowed to pair a device");
                    allowed += 1;
                }
                Err(why) => {
                    assert!(!o.is_local(), "{o} was refused");
                    assert!(why.contains(o.as_str()), "{why} does not name {o}");
                    // And it says what to do instead, because the person
                    // reading it is looking at a command that just failed.
                    assert!(why.contains("apex remote pair"), "{why}");
                }
            }
        }
        assert_eq!(allowed, 2, "the wrong number of origins may pair");
    }

    #[test]
    fn the_refusal_names_the_origin_it_refused() {
        // A message the user can act on. Built from the real classification of
        // this process rather than from a fixture, so it stays true.
        let me = crate::peer::Peer {
            pid: std::process::id() as libc::pid_t,
            uid: unsafe { libc::getuid() },
            gid: unsafe { libc::getgid() },
        };
        let observed = apex_agent_core::origin::observe_pid(me.pid);
        match (observed, local_caller(Some(me))) {
            (Ok(o), Ok(())) => assert!(o.is_local(), "{o} was allowed to pair"),
            (Ok(o), Err(why)) => {
                assert!(!o.is_local(), "{o} was refused");
                assert!(why.contains(o.as_str()), "{why} does not name {o}");
            }
            (Err(_), Err(_)) => {}
            (Err(why), Ok(())) => panic!("unclassifiable ({why}) and still allowed to pair"),
        }
    }

    #[test]
    fn every_request_and_reply_survives_the_line_framing() {
        // One JSON object per line. A serialised value containing a raw
        // newline would desynchronise the socket.
        let requests = vec![
            Request::Status,
            Request::Pair,
            Request::Devices,
            Request::Revoke {
                device: "pixel-8".into(),
            },
        ];
        for r in &requests {
            let text = serde_json::to_string(r).expect("serialise");
            assert!(!text.contains('\n'), "{text}");
            serde_json::from_str::<Request>(&text).expect("round trip");
        }
        let replies = vec![
            Reply::Ok,
            Reply::error("something went wrong"),
            Reply::Offer {
                qr: "apex-remote:abc".into(),
                expires_ms: 1,
            },
            Reply::Devices { devices: vec![] },
            Reply::Status {
                version: 1,
                key: "k".into(),
                machine: "l16".into(),
                lan: vec!["10.0.0.1:7717".into()],
                relay: None,
                connections: Vec::new(),
                rendezvous: "r".into(),
                paired: 0,
                offer_ms_left: Some(1000),
            },
        ];
        for r in &replies {
            let text = serde_json::to_string(r).expect("serialise");
            assert!(!text.contains('\n'), "{text}");
            serde_json::from_str::<Reply>(&text).expect("round trip");
        }
    }
}
