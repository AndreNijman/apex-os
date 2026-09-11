// The relay's rules, under `node --test`.
//
//     cd relay && node --test
//
// No wrangler, no workerd, no account. Everything asserted here is a pure
// function over values, which is exactly why `room.js` exists separately from
// `index.js`: the Worker cannot be run on the machine this was written on,
// and rules that could only be checked by deploying would never be checked.
//
// What this does NOT cover, stated so nobody reads it as more than it is: the
// hibernation handling and the forwarding in `index.js` have not been run.
// `wrangler dev --local` against `apexd/apex-remoted/tests/relay.rs`'s client
// is the pre-deploy step, and it is written down in
// ROADMAP/state/agents/p1-052.md.

import { test } from "node:test";
import assert from "node:assert/strict";

import { NOTICE, decide, refusal, target } from "../src/room.js";

const query = (s) => new URLSearchParams(s);

test("a rendezvous and a role are both required, and named", () => {
  assert.deepEqual(target("/r/abcdefgh", query("role=host")), {
    rendezvous: "abcdefgh",
    role: "host",
  });
  assert.deepEqual(target("/r/abcdefgh", query("role=guest")), {
    rendezvous: "abcdefgh",
    role: "guest",
  });
});

test("a request that does not say which end it is gets a status, not a guess", () => {
  // A relay that paired the first two arrivals would join two phones to each
  // other, and they would discover it only when the Noise handshake failed —
  // which is a confusing way to learn that a desktop is not running.
  for (const q of ["", "role=", "role=HOST", "role=either", "roll=host"]) {
    const answer = target("/r/abcdefgh", query(q));
    assert.equal(answer.status, 400, `${q || "(no query)"} was accepted`);
  }
});

test("a path that is not a room is a 404 and never a route", () => {
  for (const p of [
    "/",
    "/r",
    "/r/",
    "/rooms/abcdefgh",
    "/r/abcdefgh/extra",
    "/r/../etc",
    "/r/short",
    "/r/has spaces",
    "/r/has.a.dot",
    "/r/" + "x".repeat(129),
  ]) {
    const answer = target(p, query("role=host"));
    assert.equal(answer.status, 404, `${p} was accepted as a room`);
  }
});

test("an id keeps the alphabet the desktop derives, and no separator", () => {
  // The id is URL-safe unpadded base64 by construction. Accepting a `/` would
  // let a room name carry a path, and accepting `+` or `=` would accept an id
  // no APEX desktop derives.
  assert.ok("rendezvous" in target("/r/AbC_-019xyzQWERTY12", query("role=host")));
  for (const bad of ["a+b/cdefgh", "abcdefg=", "abcdef/g"]) {
    assert.equal(target(`/r/${bad}`, query("role=host")).status, 404, `${bad} was accepted`);
  }
});

test("one host holds a room and a second is refused", () => {
  // Whoever has seen the QR code knows the rendezvous id. They cannot
  // impersonate the desktop — no static private key, so Noise_IK fails — but
  // a relay that let a second host displace the first would hand them a
  // denial of service against a machine they have never touched.
  assert.deepEqual(decide("host", false), { ok: true, act: "hold" });
  const second = decide("host", true);
  assert.equal(second.ok, false);
  assert.equal(second.status, 409);
});

test("a guest with no desktop waiting is told so rather than left holding a socket", () => {
  const alone = decide("guest", false);
  assert.equal(alone.ok, false);
  assert.equal(alone.status, 409);
  assert.deepEqual(decide("guest", true), { ok: true, act: "join" });
});

test("the room is decided by what is live, so a dead host does not hold it for ever", () => {
  // The defect this is about, found by the Rust suite against the local
  // double: a relay that remembers a waiting host whose socket has died
  // answers that desktop's every later dial with 409, and the machine is
  // unreachable until its daemon restarts. `decide` takes liveness as an
  // argument for exactly that reason — it is derived from the sockets the
  // runtime still has, never from something the room wrote down.
  assert.deepEqual(decide("host", false), { ok: true, act: "hold" });
  assert.equal(decide("host", true).status, 409);
});

test("the three notices are byte-for-byte what the desktop parses", () => {
  // apex_remote_core::relay::Notice::text() produces these exact strings, and
  // a Rust test reads this file to check it. Two independently maintained
  // spellings of one wire value is the drift that would leave a desktop
  // waiting through a relay that had already paired it.
  assert.equal(NOTICE.waiting, '{"relay":"waiting"}');
  assert.equal(NOTICE.paired, '{"relay":"paired"}');
  assert.equal(NOTICE.peerGone, '{"relay":"peer-gone"}');
  for (const text of Object.values(NOTICE)) {
    const parsed = JSON.parse(text);
    assert.deepEqual(Object.keys(parsed), ["relay"], `${text} carries more than a word`);
  }
});

test("a refusal carries the reason as text a person could read in a log", () => {
  const r = refusal(409, "no desktop is waiting at this rendezvous");
  assert.equal(r.status, 409);
  assert.match(r.body, /no desktop is waiting/);
  assert.match(r.headers["content-type"], /text\/plain/);
});
