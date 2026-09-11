// The relay's decisions, with no Cloudflare in them.
//
// Everything in this file is a pure function over values: which rendezvous a
// request names, which role it asks for, and whether that is allowed given
// what the room is already holding. `index.js` is the part that knows about
// WebSockets and Durable Objects, and it contains no rules.
//
// The split is not tidiness. A Worker cannot be run on the machine this was
// written on — there is no wrangler here and deploying one is a decision
// about somebody's Cloudflare account — so the rules live where `node --test`
// can reach them, and the Rust suite in
// `apexd/apex-remoted/tests/relay.rs` is the conformance reference for the
// half that has to travel over a socket.
//
// Plain JavaScript rather than TypeScript for the same reason: `node --test`
// runs this file unmodified, with no build step and no toolchain to install.

/// The three things a relay is allowed to say about itself.
///
/// Byte-for-byte what `apex_remote_core::relay::Notice::text()` produces. A
/// Rust test reads this file and asserts that, because two independently
/// maintained spellings of one wire value is the drift that would make a
/// desktop sit waiting through a relay that had already paired it.
export const NOTICE = {
  waiting: '{"relay":"waiting"}',
  paired: '{"relay":"paired"}',
  peerGone: '{"relay":"peer-gone"}',
};

/// What a rendezvous id may contain.
///
/// The id is URL-safe unpadded base64 of 16 bytes by construction — 22
/// characters — but this is deliberately a shape check and not a length
/// check: a later revision may derive a longer one, and a relay that refused
/// it would be a relay that has to be redeployed in lockstep with the
/// desktop. What it will not accept is a path separator, which is the
/// character that turns a room name into a route.
const ID = /^[A-Za-z0-9_-]{8,128}$/;

/**
 * Read a request target.
 *
 * @param {string} pathname e.g. "/r/abc123"
 * @param {URLSearchParams} query
 * @returns {{rendezvous: string, role: "host"|"guest"} | {status: number, why: string}}
 */
export function target(pathname, query) {
  const parts = pathname.split("/").filter((p) => p !== "");
  if (parts.length !== 2 || parts[0] !== "r") {
    return { status: 404, why: "not a rendezvous" };
  }
  const rendezvous = parts[1];
  if (!ID.test(rendezvous)) {
    return { status: 404, why: "not a rendezvous" };
  }
  const role = query.get("role");
  if (role !== "host" && role !== "guest") {
    // Named rather than guessed. A relay that paired the first two arrivals
    // would join two phones to each other, and they would find out only when
    // the Noise handshake failed.
    return { status: 400, why: "a connection must say whether it is the host or a guest" };
  }
  return { rendezvous, role };
}

/**
 * Whether a connection may have the room, given what the room already holds.
 *
 * `waiting` is whether a LIVE host connection is parked. It must be derived
 * from the sockets the runtime still has, never from something the room
 * wrote down: a relay that remembers a waiting host whose socket has died
 * answers that desktop's every later dial with 409, and the machine is
 * unreachable until its daemon restarts. That defect was found by the Rust
 * suite against the local double, which made exactly this mistake first.
 *
 * @param {"host"|"guest"} role
 * @param {boolean} waiting
 * @returns {{ok: true, act: "hold"|"join"} | {ok: false, status: number, why: string}}
 */
export function decide(role, waiting) {
  if (role === "host") {
    if (waiting) {
      // Whoever has seen the QR code knows the rendezvous id and can dial as
      // a host. They cannot impersonate the desktop — they have no static
      // private key, so Noise_IK fails for them — but a relay that let a
      // second host displace the first would hand them a denial of service
      // against a machine they have never touched.
      return { ok: false, status: 409, why: "this rendezvous already has a host waiting" };
    }
    return { ok: true, act: "hold" };
  }
  if (!waiting) {
    return { ok: false, status: 409, why: "no desktop is waiting at this rendezvous" };
  }
  return { ok: true, act: "join" };
}

/**
 * The HTTP refusal for a decision that said no.
 *
 * A status the client can read, sent BEFORE the upgrade rather than as a
 * close frame afterwards, so a device whose desktop is off is told so
 * instead of being left holding a socket. 409 in particular is how a device
 * learns to retry: the desktop consumes its waiting connection the moment a
 * device attaches and opens the next one immediately, so a device that
 * arrives in that window sees 409 about a machine that is running.
 *
 * @param {number} status
 * @param {string} why
 */
export function refusal(status, why) {
  return { status, body: why + "\n", headers: { "content-type": "text/plain; charset=utf-8" } };
}
