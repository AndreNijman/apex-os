//! Meeting through a relay, and what the relay's operator learns.
//!
//! ## The requirement, and the shape it forces
//!
//! P1-052: internet access must need **no inbound router port forwarding**,
//! and the relay must not be able to read terminal or task content. Those two
//! together rule out most designs. A machine that cannot accept an inbound
//! connection can only be reached if it has already made an outbound one, so
//! both ends dial out to a meeting point and the meeting point copies bytes
//! between them.
//!
//! That is all a relay does here: copy bytes. It terminates no encryption,
//! holds no key, and is not trusted. The Noise channel from
//! [`crate::noise`] is established end to end *through* it, so the relay
//! carries ciphertext whose keys it never sees. This is the same reason the
//! design does not use TLS to the relay and then plaintext behind it, which
//! is what a naive "encrypted relay" would mean.
//!
//! ## What the operator can see, stated plainly
//!
//! Not "nothing". Anyone who runs the relay — Cloudflare, or the owner on
//! their own VPS, or an attacker who has taken the relay over — can observe:
//!
//! * **The rendezvous id.** Both ends must name the same meeting point, so
//!   the operator necessarily learns it. It is [`rendezvous_id`], a hash of
//!   the desktop's static public key — never the key itself, so an operator
//!   cannot impersonate the desktop to a device that has scanned its QR code.
//!   It is stable, which means the operator can tell that the same desktop is
//!   being reached today and last week.
//! * **Both IP addresses**, and therefore roughly where the phone and the
//!   laptop are.
//! * **Timing and volume.** When a session is open, for how long, how many
//!   bytes went each way and in what bursts. That is enough to infer typing
//!   rhythm, roughly how much output a command produced, and when somebody is
//!   working.
//! * **That this is APEX Remote traffic**, from the path shape.
//!
//! What the operator cannot see: the content of any terminal, any prompt, any
//! file, any project name, any session id, any device name, and **which
//! device** is connecting — the device's static key travels encrypted inside
//! the `Noise_IK` handshake, so it is not a correlation handle the way the
//! rendezvous id is.
//!
//! An owner who is not willing to give up the traffic-analysis metadata above
//! should run LAN-only, which is a complete configuration: leave the relay
//! unconfigured and the QR code carries no relay at all.
//!
//! ## Why the rendezvous id is a hash and not the key
//!
//! Publishing the desktop's public key as its meeting-point name would hand
//! every relay operator, and everyone who can watch the relay, the exact value
//! a device pins during pairing. It would not let them decrypt anything — they
//! would still need the secret half — but it would let them *offer* that key
//! to a device, and a device that accepted a key it had not seen on a QR code
//! is a device with no pairing security at all. Hashing costs nothing and
//! removes the temptation.

use sha2::{Digest, Sha256};

/// The domain separator mixed into the hash.
///
/// So that this hash of a public key can never collide with some other
/// protocol's hash of the same key. Cheap, and the absence of one is a
/// recurring footgun in exactly this kind of derivation.
const DOMAIN: &[u8] = b"apex-remote/rendezvous/v1";

/// The meeting-point name for a machine with this static public key.
///
/// Deterministic and stable: both ends derive it from the same value, and the
/// device already has that value because it pinned it when it scanned the QR
/// code. Nothing has to be exchanged, and no state has to be stored on the
/// relay for a device to find its desktop again after a reinstall.
pub fn rendezvous_id(desktop_public: &[u8; 32]) -> String {
    let mut h = Sha256::new();
    h.update(DOMAIN);
    h.update(desktop_public);
    // 128 bits is far beyond collision reach for a namespace of this size,
    // and a shorter path is a shorter URL in a log the operator keeps anyway.
    crate::b64_encode(&h.finalize()[..16])
}

/// How a connection actually reached the other end.
///
/// P1-052 asks for the connection path and its quality to be visible at both
/// ends. This is the path half; round-trip time on [`crate::wire::Frame::Ping`]
/// is the quality half.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Path {
    /// A direct TCP connection on the local network.
    Lan,
    /// Through a relay. Slower, and observable by its operator.
    Relay,
}

impl Path {
    pub fn as_str(&self) -> &'static str {
        match self {
            Path::Lan => "lan",
            Path::Relay => "relay",
        }
    }

    /// What the user is told about privacy on this path.
    ///
    /// Written here, once, so the CLI and the shell page and the Android app
    /// cannot each invent their own reassuring version of it.
    pub fn disclosure(&self) -> &'static str {
        match self {
            Path::Lan => {
                "direct on this network; nothing leaves it and no third party is involved"
            }
            Path::Relay => {
                "through a relay, which carries encrypted bytes it cannot read but does see \
                 both addresses, when you connect and how much data moves"
            }
        }
    }
}

impl std::fmt::Display for Path {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.pad(self.as_str())
    }
}

/// The order a client tries paths in.
///
/// LAN first, always. It is faster, it involves nobody else, and when it
/// works the relay never learns the session happened at all. A client that
/// raced both and took whichever answered first would leak a rendezvous
/// connection every time, including on the network where it was unnecessary.
pub const PREFERENCE: [Path; 2] = [Path::Lan, Path::Relay];

/// How good a connection is, in the words a person is shown.
///
/// Three bands and not a number, because a number is what a page ends up
/// rendering and "83 ms" tells nobody whether their terminal is going to feel
/// wrong. The number is kept as well — [`Connection::rtt_ms`] — for anyone
/// who wants it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quality {
    /// Nothing has been measured yet. Distinct from `Poor`: a connection that
    /// has just opened has not failed, and a page that showed the worst band
    /// while waiting for the first ping would be lying for fifteen seconds
    /// every time.
    Unmeasured,
    Good,
    Fair,
    Poor,
}

impl Quality {
    /// The band a round trip falls in.
    ///
    /// The boundaries are about a terminal, not about a network. Local echo
    /// stops feeling immediate somewhere around 100 ms of round trip, and
    /// past about a quarter of a second typing feels like it is happening to
    /// somebody else. They are deliberately not "LAN" and "relay": a relay in
    /// the same city can be better than a congested Wi-Fi network, and the
    /// point of measuring is to say which one the person actually has.
    pub fn of(rtt_ms: Option<u64>) -> Quality {
        match rtt_ms {
            None => Quality::Unmeasured,
            Some(ms) if ms < 80 => Quality::Good,
            Some(ms) if ms < 250 => Quality::Fair,
            Some(_) => Quality::Poor,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Quality::Unmeasured => "unmeasured",
            Quality::Good => "good",
            Quality::Fair => "fair",
            Quality::Poor => "poor",
        }
    }
}

impl std::fmt::Display for Quality {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.pad(self.as_str())
    }
}

/// One connection a device is holding right now.
///
/// P1-052's last criterion asks for the connection path and its quality to be
/// visible. This is the value that carries both, and it is defined here so
/// that the daemon, the CLI and the shell page render one thing rather than
/// three that agree until one of them is edited.
///
/// Live only. Nothing here is written to the device store: a round trip
/// measured four hours ago is not a fact about now, and persisting it would
/// mean a page could show a quality for a device that is not connected.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Connection {
    pub device_id: String,
    pub device_name: String,
    /// `"lan"` or `"relay"` — [`Path::as_str`].
    pub path: String,
    /// Unix milliseconds at which this connection authenticated.
    pub since_ms: u64,
    /// The most recent measured round trip, or `None` before the first one
    /// comes back.
    pub rtt_ms: Option<u64>,
}

impl Connection {
    pub fn quality(&self) -> Quality {
        Quality::of(self.rtt_ms)
    }

    /// What the owner is told about privacy on this connection's path.
    ///
    /// Routed through [`Path`] rather than re-written, so a page cannot
    /// invent a friendlier version of it.
    pub fn disclosure(&self) -> &'static str {
        if self.path == Path::Relay.as_str() {
            Path::Relay.disclosure()
        } else {
            Path::Lan.disclosure()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rendezvous_id_is_stable_and_not_the_key() {
        let key = [7u8; 32];
        let id = rendezvous_id(&key);
        assert_eq!(id, rendezvous_id(&key), "the derivation is not deterministic");
        let key_text = crate::b64_encode(&key);
        assert_ne!(id, key_text);
        assert!(
            !key_text.contains(&id) && !id.contains(&key_text),
            "the rendezvous id embeds the key"
        );
        // And the key is not recoverable from it by length alone: 16 bytes
        // out of a 32-byte input.
        assert_eq!(crate::b64_decode(&id).map(|b| b.len()), Some(16));
    }

    #[test]
    fn different_machines_meet_in_different_places() {
        let mut seen = std::collections::HashSet::new();
        for i in 0..64u8 {
            assert!(
                seen.insert(rendezvous_id(&[i; 32])),
                "two machines share a rendezvous id"
            );
        }
    }

    #[test]
    fn one_bit_of_the_key_changes_the_whole_id() {
        // A derivation that truncated the key rather than hashing it would
        // pass the two tests above and leak a prefix of the key to the relay.
        let mut a = [0u8; 32];
        let mut b = [0u8; 32];
        b[31] = 1;
        let (ia, ib) = (rendezvous_id(&a), rendezvous_id(&b));
        assert_ne!(ia, ib);
        a[0] = 1;
        assert_ne!(rendezvous_id(&a), ia);
    }

    #[test]
    fn the_domain_separator_is_actually_mixed_in() {
        // A hash of the bare key would be a different value; this asserts the
        // constant is used rather than declared.
        let key = [9u8; 32];
        let mut bare = Sha256::new();
        bare.update(key);
        assert_ne!(
            rendezvous_id(&key),
            crate::b64_encode(&bare.finalize()[..16]),
            "the domain separator is not mixed in"
        );
    }

    #[test]
    fn lan_is_always_preferred_and_says_why() {
        assert_eq!(PREFERENCE[0], Path::Lan);
        assert_eq!(PREFERENCE[1], Path::Relay);
        // The disclosure has to name the third party on the path that has
        // one, and must not on the path that does not.
        assert!(Path::Relay.disclosure().contains("relay"));
        assert!(Path::Relay.disclosure().contains("addresses"));
        assert!(!Path::Lan.disclosure().contains("relay"));
    }

    #[test]
    fn an_unmeasured_connection_is_not_a_bad_one() {
        // A page that showed the worst band until the first ping came back
        // would tell every owner their connection was poor for the first
        // fifteen seconds of every session.
        assert_eq!(Quality::of(None), Quality::Unmeasured);
        assert_ne!(Quality::of(None), Quality::Poor);
    }

    #[test]
    fn the_quality_bands_are_about_a_terminal_and_are_ordered() {
        assert_eq!(Quality::of(Some(0)), Quality::Good);
        assert_eq!(Quality::of(Some(79)), Quality::Good);
        assert_eq!(Quality::of(Some(80)), Quality::Fair);
        assert_eq!(Quality::of(Some(249)), Quality::Fair);
        assert_eq!(Quality::of(Some(250)), Quality::Poor);
        assert_eq!(Quality::of(Some(10_000)), Quality::Poor);
        // Monotonic: a worse round trip never reports a better band.
        let rank = |q: Quality| match q {
            Quality::Unmeasured => 0,
            Quality::Good => 1,
            Quality::Fair => 2,
            Quality::Poor => 3,
        };
        let mut last = 0;
        for ms in (0..600).step_by(7) {
            let r = rank(Quality::of(Some(ms)));
            assert!(r >= last, "{ms} ms reported a better band than {} ms did", ms - 7);
            last = r;
        }
    }

    #[test]
    fn the_band_is_not_a_synonym_for_the_path() {
        // The measurement has to be able to disagree with the path, or it is
        // not a measurement. A relay in the same city beats congested Wi-Fi,
        // and an owner deciding whether to move closer to the router needs to
        // be told which one they actually have.
        let relay = Connection {
            device_id: "d".into(),
            device_name: "phone".into(),
            path: Path::Relay.as_str().into(),
            since_ms: 0,
            rtt_ms: Some(30),
        };
        let lan = Connection { path: Path::Lan.as_str().into(), rtt_ms: Some(400), ..relay.clone() };
        assert_eq!(relay.quality(), Quality::Good);
        assert_eq!(lan.quality(), Quality::Poor);
        // And the disclosure still follows the path, not the band.
        assert!(relay.disclosure().contains("relay"));
        assert!(!lan.disclosure().contains("relay"));
    }

    #[test]
    fn a_connection_round_trips_through_json_the_way_the_control_socket_sends_it() {
        let c = Connection {
            device_id: "abc".into(),
            device_name: "a phone".into(),
            path: Path::Relay.as_str().into(),
            since_ms: 17,
            rtt_ms: None,
        };
        let text = serde_json::to_string(&c).expect("serialise");
        assert!(text.contains(r#""rtt_ms":null"#), "{text}");
        assert_eq!(serde_json::from_str::<Connection>(&text).expect("parse"), c);
    }
}
