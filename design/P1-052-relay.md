# P1-052 — the relay: what it is, what it costs, and what deploying it needs

Companion to `P1-050-remote-protocol.md`, which settled the protocol. This one
is about the meeting point: the Cloudflare Worker plus Durable Object under
`relay/` in apex-os, the client that dials it, and the honest list of what is
not done.

**Nothing here is deployed.** No `wrangler deploy` has been run, no Worker
exists on any account, and no Cloudflare credential has been used. Everything
below was proven against a local double — a `TcpListener` on loopback in
`apexd/apex-remoted/tests/relay.rs` that implements the same room protocol —
which is the pattern the Cloudflare capability sets in `apex-secretd` already
use for the API, and the pattern the trust gate used when it proved itself
against a minted CA rather than production sigstore.

---

## 1. Why a relay at all

P1-052 asks for internet access with **no inbound router port forwarding**,
and for the relay to be unable to read terminal or task content. Those two
together decide the design. A machine that cannot accept an inbound connection
can only be reached if it has already made an outbound one, so both ends dial
out to a meeting point and the meeting point copies bytes between them.

That is all a relay does. It terminates no encryption, holds no key, and is
not trusted. The `Noise_IK` channel is established end to end *through* it.

## 2. The wire

One HTTP endpoint, two roles.

```
GET /r/<rendezvous-id>?role=host     the desktop, waiting
GET /r/<rendezvous-id>?role=guest    a paired device, arriving
```

`<rendezvous-id>` is `apex_remote_core::rendezvous::rendezvous_id` — URL-safe
unpadded base64 of the first 16 bytes of `SHA-256("apex-remote/rendezvous/v1"
|| desktop-public-key)`. A hash and not the key: publishing the key would hand
every relay operator the exact value a device pins when it scans a QR code.

Both are ordinary RFC 6455 upgrades. Refusals happen **before** the upgrade,
so a client reads an HTTP status rather than guessing at a close frame:

| status | means |
| --- | --- |
| 101 | joined, or holding the room |
| 400 | no role, or a role that is not `host` or `guest` |
| 404 | not a rendezvous path, or an id outside the derived alphabet |
| 409 | a host when one is already waiting, or a guest when none is |
| 426 | the path is right but the request is not an upgrade |

Once upgraded there are exactly two kinds of frame:

- **Binary** is the carried byte stream — `[u32 big-endian length][Noise
  ciphertext]`, the same framing the LAN leg carries over TCP. The relay
  copies it verbatim and never inspects it.
- **Text** is the relay talking about itself, and is the whole vocabulary:
  `{"relay":"waiting"}`, `{"relay":"paired"}`, `{"relay":"peer-gone"}`.
  A client never sends one.

The split is the point. "The relay cannot read the session" does not depend on
the relay being polite; it depends on the payload being sealed before it gets
there, and on there being no channel through which a relay could say anything
about what it is copying.

### The three rules that are decisions, not accidents

**Roles are explicit.** A relay that paired the first two arrivals would join
two phones to each other, and they would discover it only when the Noise
handshake failed — a confusing way to learn that a desktop is not running.

**The host re-arms.** The desktop opens a fresh waiting connection the moment a
device attaches, so one desktop serves several devices. The consequence is a
window in which a device is told 409 about a machine that is running, so a
client retries a 409 rather than treating it as an outage.

**A waiting host must be evicted when its socket dies.** This one was found by
testing, not by design. A relay that parks a waiting host and never notices the
socket has gone answers that desktop's every later dial with 409 *for ever*,
and the machine is unreachable until its daemon restarts. The first version of
the local double did exactly that, and the reconnect suite caught it as "the
desktop did not re-arm". `RelayRoom.waiting()` therefore derives occupancy from
`ctx.getWebSockets()` every time it is asked, and writes nothing down.

## 3. What the operator can see

Not nothing, and the desktop says so before an owner turns a relay on —
`rendezvous::Path::disclosure` is the single source of that sentence.

- **The rendezvous id.** Both ends must name the same meeting point. It is
  stable, so an operator can tell that the same desktop is being reached today
  and last week.
- **Both IP addresses**, and therefore roughly where the phone and the laptop
  are.
- **Timing and volume** — when a session is open, how long, how many bytes each
  way and in what bursts. Enough to infer typing rhythm and when somebody is
  working.
- **That this is APEX Remote traffic**, from the path shape.

Not visible: any terminal, prompt, file, project name, session id or device
name — and not *which* device, because the device's static key travels
encrypted inside the `Noise_IK` handshake.

An impostor host is a denial of service, not an impersonation. Anyone who has
seen the QR code knows the rendezvous id and can dial as a host for it; they do
not have the static private key, so `Noise_IK` fails for them. That is why a
second host is refused rather than allowed to displace the first.

An owner unwilling to give up the metadata above should run LAN-only, which is
a complete configuration: leave the relay unset and the QR code carries no
relay at all. **That is the shipped default** —
`files/system/units/apex-remoted.service` passes no `--relay`, and a test pins
it.

## 4. What is written, and what has run

| piece | where | run? |
| --- | --- | --- |
| RFC 6455 client, endpoint parsing, roles | `apexd/apex-remote-core/src/relay.rs` | yes, 26 tests |
| dial + loopback splice + re-arm + backoff | `apexd/apex-remoted/src/relay.rs` | yes |
| round-trip measurement and quality bands | `serve.rs`, `rendezvous.rs` | yes |
| the room protocol, end to end | `apexd/apex-remoted/tests/relay.rs` | yes, 8 suites against a double |
| the relay's rules | `relay/src/room.js` | yes, 9 `node --test` |
| **the Worker and the Durable Object** | `relay/src/index.js` | **yes — 6 of 8 suites, over `ws://`, under `wrangler dev --local`** |

> **CLOSED 2026-09-12 by the `relay-tls` unit** (apex-os `0ad66ab2`). wrangler
> is now a dev-dependency of `relay/` and `wrangler dev --local` needs **no
> Cloudflare account**, so this row was a missing dependency and never a
> missing permission. `APEX_RELAY_URL=ws://127.0.0.1:8787 cargo test -p
> apex-remoted --test relay` runs **6 of the 8 suites against the real Worker**
> — including pairing and a live session end to end, and the Worker's own 404,
> 409 and 426. The two that do not run print `SKIP-EXTERNAL` and say why: one
> asserts what the *operator* can see by reading everything the double copied,
> and one simulates a network change by cutting the relay's own sockets from
> inside it. Neither is a thing a real relay hands over; both stay covered
> against the double.
>
> **Running it found a defect, which is the point of having run it.**
> `Uncaught TypeError: Can't call WebSocket send() after close()` at
> `webSocketMessage` — `ctx.getWebSockets()` keeps returning sockets the object
> has already closed. The same staleness would have let `waiting()` hold a room
> **for ever** (the 409-forever failure §2 above claims to have designed out)
> and let `socketFor` route a live peer's frames into a dead socket. Fixed with
> an `isOpen()` guard on both, and a `farewell()` that is a no-op on an
> already-closed socket. Measured: against the pre-fix Worker the Rust suite
> reported `8 passed; 0 failed` three times in a row while wrangler logged two
> uncaught exceptions — a Worker that throws inside a WebSocket event still
> leaves the socket closed, which is what the client was waiting for, so the
> client is *satisfied by the failure*. Only the Worker's own log sees it.

A Rust test reads `relay/src/room.js` and asserts the three notice strings and
the two role words match the enum, so the two languages cannot drift apart on
the wire values even though they are tested separately.

## 5. Deploying it — exactly what Andre would run

### 5.1 The blocker first

> **RESOLVED 2026-09-12 by the `relay-tls` unit** (apex-os `f4e854f0`).
> **Option 1 was chosen** — Andre delegated the decision and the orchestrator
> took it. `apex-remote-core` now has `src/tls.rs`: `rustls` on the `ring`
> provider, with the **machine's** root store via `rustls-native-certs` rather
> than a vendored `webpki-roots` bundle. `apex-remoted --relay wss://…` works.
>
> The root-store half was the decision left open. The machine wins because the
> Sigstore root is pinned for a reason that does not transfer: there the thing
> being verified **is the OS image**, so the store inside that image cannot be
> the authority on it. A relay dial happens afterwards on a booted, trusted
> machine — where `update-ca-trust` removals must apply, and where the
> realistic private deployment sits behind an internal CA that a vendored
> bundle could never reach. An empty store is therefore a **named refusal** and
> never a fall back to a vendored bundle, since falling back would re-trust
> exactly what an administrator had just removed.
>
> Cost, measured rather than estimated: `Cargo.lock` 178 → **193, so 15
> crates** — which is what this section predicted. Only **9** compile on Linux;
> the other 6 are the macOS/Windows entries `rustls-native-certs` carries.
>
> Verification is the **refusals**, because a client that accepts any
> certificate also connects and passes every happy-path assertion:
> `apex-remote-core/tests/tls.rs` mints a CA with the `openssl` CLI, serves it
> from a loopback `rustls` listener, and asserts by variant *and* by message
> that an untrusted issuer, a wrong hostname and an expired certificate are
> each refused. All three are mutation-proved: made to accept one at a time,
> each named test goes red. Nothing reaches the network.
>
> The account below is kept as the record of how the choice was made.

**The client cannot dial `wss://` today.** Nothing in the apexd workspace
speaks TLS; `apex-secretd`'s Cloudflare provider settled that question for the
tree by shelling out to `curl`, and a WebSocket cannot be carried over a
one-shot `curl`. `apex-remoted` refuses a `wss://` relay by name rather than
opening a plain socket to port 443 and carrying the session in the clear while
the configuration says otherwise.

`*.workers.dev` is HTTPS-only, so the Worker as it stands is reachable by a
browser and not by this client. Three ways out, in the order I would take
them:

1. **Add `rustls` + `webpki-roots`** to `apex-remote-core`. About 40 lines to
   wrap the stream, and the WebSocket client is already generic over
   `Read + Write` so nothing else changes. The cost is a dependency decision:
   roughly 15 new crates in `Cargo.lock`, in a workspace that has deliberately
   had none for TLS. **This is a decision for Andre, not for an agent.**
2. **Tunnel through a TLS child process** — `openssl s_client -quiet -connect
   host:443 -servername host -verify_return_error`, whose stdin/stdout is the
   stream. Zero new crates, and it is the same shape as the `curl` child the
   tree already blessed. Ugly, and one more process per relay connection.
3. **A custom domain with "Always Use HTTPS" off**, so `ws://relay.example.com`
   on port 80 reaches the Worker. Zero code. The trade is real and must not be
   waved away: TLS to the relay hides the rendezvous id and the traffic
   pattern from an *on-path observer* — an ISP, a café network — which is a
   different party from the relay operator the disclosure already names. The
   session content stays sealed either way.

Until one of those is chosen, deploying the Worker produces a relay that works
and a desktop that will not dial it.

### 5.2 Then the deployment

```
cd relay
# npm install --save-dev wrangler    # DONE 2026-09-12; wrangler 4.131.1 is
                                     # committed as a dev-dependency of relay/
npx wrangler login                   # opens a browser; grants the CLI the account
npx wrangler deploy
```

A fresh clone still needs `npm install` inside `relay/` first —
`node_modules/` and `.wrangler/` are gitignored.

`wrangler deploy` prints the URL, `https://apex-remote-relay.<subdomain>.workers.dev`.
Then, on each machine:

```
systemctl --user edit apex-remoted
#   [Service]
#   ExecStart=
#   ExecStart=/usr/bin/apex-remoted --relay wss://apex-remote-relay.<subdomain>.workers.dev
systemctl --user restart apex-remoted
apex remote status          # "relay" and "rendezvous" should both be filled in
```

Before any of that, the step this unit could not take — **taken 2026-09-12**:

```
cd relay && npx wrangler dev --local
# then, in apex-os/apexd:
APEX_RELAY_URL=ws://127.0.0.1:8787 cargo test -p apex-remoted --test relay
```

That turned the last row of §4 from "no" to "yes", and it found the
send-after-close defect recorded there. Worth re-running after any change to
`index.js`, and worth reading `wrangler`'s own log afterwards rather than only
the test summary — the whole lesson of that defect is that the summary was
green while the Worker was throwing.

### 5.3 What it costs

From Cloudflare's own pages, read on 2026-09-12 — the per-dimension rates
from the Durable Objects pricing page, the plan fee quoted verbatim from the
Workers pricing page: *"The Workers Paid plan includes Workers, Pages
Functions, Workers KV, Hyperdrive, and Durable Objects usage for a minimum
charge of $5 USD per month for an account."*

| | Workers Free | Workers Paid |
| --- | --- | --- |
| requests | 100,000 / day | 1 M / month, then $0.15/M |
| duration | 13,000 GB-s / day | 400,000 GB-s / month, then $12.50/M GB-s |
| plan fee | — | $5 / month minimum |

Two things make this cheap for personal use and both are properties of the
design rather than luck:

- **Hibernation.** A desktop's waiting connection is open for as long as the
  machine is on and silent for hours. "Durable Objects that are idle and
  eligible for hibernation are not billed for duration", and `index.js` uses
  `ctx.acceptWebSocket` rather than `server.accept()` precisely for that. It is
  the difference between a relay that costs nothing at rest and one that bills
  per machine per hour.
- **The 20:1 message ratio.** Incoming WebSocket messages bill at 100 messages
  per 5 requests, so a busy terminal session of ten thousand frames is five
  hundred billable requests.

Durable Objects are available on the **free** plan provided they are
SQLite-backed, which `migrations` declares. This Worker stores nothing at all,
so no row is ever read or written and the storage dimension does not apply.

For one person with a laptop and a phone this sits inside the free plan. The
thing that would change that is other people using it: the relay is not
authenticated, and anybody who learns a rendezvous id can occupy it. See §6.

## 5.4 — unrelated to the relay: LAN discovery

`apex-remoted` announces `_apex-remote._tcp` on the local network for as long
as it runs, by holding an `avahi-publish-service` child. Deliberately not a
file in `/etc/avahi/services/`, which would advertise the machine whether or
not the service was running, whether or not the user had ever enabled APEX
Remote, and whether or not the port was open.

avahi is installed **and enabled** in the image — `Containerfile.core` enables
it and fails the build if `systemctl is-enabled` disagrees — and
`avahi-tools`, which carries `avahi-publish-service`, is now named in the same
`dnf5 install` with a `command -v` assertion beside it. It was already present
as somebody else's dependency, which is exactly the kind of thing that
disappears in a rebase nobody connects to remote access breaking.

The TXT record carries the protocol version and nothing else. In particular
not the rendezvous id, which is stable across networks and would let anybody
on a café network recognise the same laptop again next week. A device learns
an address and a port and nothing about which machine it is; `Noise_IK` is
what decides that, and a wrong desktop cannot read the attempt.

The record names the port the daemon is actually on, not the catalogue's 7717
default. Note that the port stays closed to the network until
`sudo apex firewall allow apex-remote`, so on a machine that has not opened it
a device will find the record, fail to connect, and fall back to the relay —
which is the order `rendezvous::PREFERENCE` already declares.
`--no-announce` turns it off for a machine that is only ever reached through a
relay.

The *browsing* half is the device's, and it is the Android app's (P1-053).

## 6. Known gaps, named

- ~~**The Worker has never run.**~~ **CLOSED 2026-09-12** — it runs under
  `wrangler dev --local` and 6 of 8 suites drive it. §4.
- ~~**No `wss://` from the client.**~~ **CLOSED 2026-09-12** — `rustls` plus the
  machine's root store, in `apex-remote-core`. §5.1. This was the one that
  stopped "deployable" from meaning "deployed and working"; what remains
  between here and a working relay is `wrangler login` and `wrangler deploy`,
  both of which are Andre's account and Andre's decision.
- **The relay is unauthenticated.** Any client that knows a rendezvous id can
  dial it, as host or guest. That is a denial of service, not a disclosure —
  Noise refuses an impostor at either end — but a public deployment would want
  a token, or Cloudflare Access in front of it, before it is given a URL that
  strangers can reach. Not done, and not pretended.
- **No rate limiting and no connection cap.** A room is two sockets, but
  nothing stops somebody opening rooms.
- **Reconnect is the desktop's half only.** The daemon re-arms unasked and the
  agent runtime keeps its sessions, both asserted. A phone deciding to re-dial
  after its network changes is the Android app's job (P1-053) and is not in
  this repository.
