// The APEX Remote relay: a Worker, and one Durable Object per rendezvous.
//
// ## What it does, and the short list of what it is trusted with
//
// It copies bytes between two WebSockets that both dialled it. That is all.
// It terminates no encryption, holds no key, and stores nothing: the Noise
// channel from `apex_remote_core::noise` is established end to end *through*
// it, so every binary frame it moves is ciphertext whose keys it never sees.
//
// What its operator can see is written out in full in
// `apexd/apex-remote-core/src/rendezvous.rs` and is repeated in the disclosure
// the desktop shows the owner before they turn a relay on: the rendezvous id,
// both IP addresses, and the timing and volume of traffic. Not nothing. An
// owner who is not willing to give that up should run LAN-only, which is a
// complete configuration — leave the relay unset and the QR code carries no
// relay at all.
//
// ## The rules are not in this file
//
// `room.js` holds every decision as a pure function, because a Worker cannot
// be run on the machine this was written on and the rules have to live
// somewhere `node --test` can reach. This file is the Cloudflare-shaped half:
// upgrades, hibernation, and forwarding.
//
// ## Why hibernation
//
// A desktop's waiting connection is open for as long as the machine is on and
// sends nothing for hours at a time. Without the hibernation API that is
// billed duration for every idle minute; with it, "Durable Objects that are
// idle and eligible for hibernation are not billed for duration". It is the
// difference between a relay that costs nothing at rest and one that bills
// per machine per hour.
//
// Hibernation is why nothing below keeps state in class fields. The object is
// reconstructed when a message arrives, so anything a field held is gone.
// Per-socket state goes in `serializeAttachment`, and room occupancy is
// derived from `ctx.getWebSockets()` every time it is asked.
//
// ## Deliberately no storage
//
// No `ctx.storage` call anywhere. The room holds two live sockets or it holds
// nothing, and there is no fact about it worth surviving them. That also
// means the only "eviction" this needs is asking the runtime which sockets
// are still there — see `waiting()`.

import { DurableObject } from "cloudflare:workers";
import { NOTICE, decide, refusal, target } from "./room.js";

export default {
  /**
   * @param {Request} request
   * @param {{ROOM: DurableObjectNamespace}} env
   */
  async fetch(request, env) {
    const url = new URL(request.url);

    // A health check that is not a room. Useful for `wrangler dev` and for
    // an uptime probe, and deliberately says nothing about who is connected.
    if (url.pathname === "/" || url.pathname === "/health") {
      return new Response("apex-remote-relay\n", {
        headers: { "content-type": "text/plain; charset=utf-8" },
      });
    }

    const asked = target(url.pathname, url.searchParams);
    if ("status" in asked) {
      const r = refusal(asked.status, asked.why);
      return new Response(r.body, { status: r.status, headers: r.headers });
    }

    if (request.headers.get("upgrade")?.toLowerCase() !== "websocket") {
      const r = refusal(426, "this endpoint speaks WebSocket only");
      return new Response(r.body, { status: r.status, headers: r.headers });
    }

    // One object per rendezvous id, addressed by name. Deterministic, so both
    // ends reach the same one without anything being stored anywhere.
    return env.ROOM.getByName(asked.rendezvous).fetch(request);
  },
};

export class RelayRoom extends DurableObject {
  /**
   * @param {Request} request
   */
  async fetch(request) {
    const url = new URL(request.url);
    const asked = target(url.pathname, url.searchParams);
    if ("status" in asked) {
      const r = refusal(asked.status, asked.why);
      return new Response(r.body, { status: r.status, headers: r.headers });
    }

    const verdict = decide(asked.role, this.waiting() !== null);
    if (!verdict.ok) {
      // Before the upgrade, so it is a status a client can read rather than a
      // close frame it has to guess at.
      const r = refusal(verdict.status, verdict.why);
      return new Response(r.body, { status: r.status, headers: r.headers });
    }

    const pair = new WebSocketPair();
    const client = pair[0];
    const server = pair[1];
    const id = crypto.randomUUID();

    // Hibernatable. The alternative, `server.accept()`, keeps this object
    // resident and billed for as long as a desktop is switched on.
    this.ctx.acceptWebSocket(server);
    server.serializeAttachment({ id, role: asked.role, peer: null });

    if (verdict.act === "hold") {
      server.send(NOTICE.waiting);
    } else {
      const host = this.waiting();
      if (host === null) {
        // Lost a race with another guest between `decide` and here. Refusing
        // is right: the alternative is a guest joined to nothing, which looks
        // to the phone like a desktop that accepts connections and then
        // ignores them.
        server.close(1013, "try again");
      } else {
        const hostState = host.deserializeAttachment();
        host.serializeAttachment({ ...hostState, peer: id });
        server.serializeAttachment({ id, role: asked.role, peer: hostState.id });
        host.send(NOTICE.paired);
        server.send(NOTICE.paired);
      }
    }

    return new Response(null, { status: 101, webSocket: client });
  }

  /**
   * The live host connection that is holding this room, or null.
   *
   * Derived from the sockets the runtime still has, never from anything
   * written down. A relay that remembered a waiting host whose socket had
   * died would answer that desktop's every later dial with 409, and the
   * machine would be unreachable until its daemon restarted. That is not
   * hypothetical: the local double in `apexd/apex-remoted/tests/relay.rs`
   * made exactly that mistake, and the reconnect suite caught it as "the
   * desktop did not re-arm".
   */
  waiting() {
    for (const ws of this.ctx.getWebSockets()) {
      const state = ws.deserializeAttachment();
      if (state && state.role === "host" && state.peer === null) {
        return ws;
      }
    }
    return null;
  }

  /**
   * @param {string} id
   */
  socketFor(id) {
    for (const ws of this.ctx.getWebSockets()) {
      const state = ws.deserializeAttachment();
      if (state && state.id === id) {
        return ws;
      }
    }
    return null;
  }

  /**
   * Copy one message to the other end, and look at none of it.
   *
   * @param {WebSocket} ws
   * @param {ArrayBuffer | string} message
   */
  async webSocketMessage(ws, message) {
    const state = ws.deserializeAttachment();
    if (!state || state.peer === null) {
      // A client that sends payload before it has been told it is paired is
      // speaking a protocol this relay does not have. Dropping the message is
      // right: forwarding it to a peer that does not exist is not possible,
      // and buffering it would make this object hold session bytes.
      return;
    }
    const peer = this.socketFor(state.peer);
    if (peer === null) {
      ws.send(NOTICE.peerGone);
      ws.close(1001, "the other end has gone");
      return;
    }
    // Verbatim. A binary message is ciphertext and a relay that inspected one
    // would find nothing; this is the line that has to stay a copy.
    peer.send(message);
  }

  /**
   * @param {WebSocket} ws
   */
  async webSocketClose(ws) {
    this.partnerLost(ws);
  }

  /**
   * @param {WebSocket} ws
   */
  async webSocketError(ws) {
    this.partnerLost(ws);
  }

  /**
   * @param {WebSocket} ws
   */
  partnerLost(ws) {
    const state = ws.deserializeAttachment();
    if (!state || state.peer === null) {
      // A waiting host that went away. Nothing to tell anybody, and nothing
      // to clean up: it is no longer in `getWebSockets()`, so the room is
      // free for the next dial by construction.
      return;
    }
    const peer = this.socketFor(state.peer);
    if (peer !== null) {
      peer.send(NOTICE.peerGone);
      peer.close(1001, "the other end has gone");
    }
  }
}
