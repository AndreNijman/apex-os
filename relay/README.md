# apex-remote-relay

The meeting point for APEX Remote when a device is not on the same network as
the desktop. A Cloudflare Worker and one Durable Object per rendezvous id.

It copies bytes between two WebSockets that both dialled it. It terminates no
encryption, holds no key, stores nothing, and nothing trusts it: the `Noise_IK`
channel runs end to end through it, so every binary frame it moves is
ciphertext whose keys it never sees.

**It is deployed** at `wss://apex-relay.andrenijman.com`, a custom domain on a
zone this account owns instead of the shared `*.workers.dev` name.
`wrangler.jsonc` says why. `ROADMAP/design/P1-052-relay.md` records what it
cost and what it exposes. It is rate-limited (60 WebSocket upgrades per
client IP per minute, answered 429 before a Durable Object wakes), caps a room
at 16 live sockets, and accepts only binary frames up to 256 KiB. It is still
unauthenticated on a public name.

Both clients dial it. The desktop is `apex-remoted --relay <url>`, which holds
a rendezvous outbound and opens no inbound port. The phone is
`android/core/.../Relay.kt` plus `RelayDialler`, which was the missing half
until 2026-09-19; `PairingService` used to say so in its own error text.

## Layout

| file | what |
| --- | --- |
| `src/room.js` | every rule, as pure functions. No Cloudflare in it. |
| `src/index.js` | the Worker and the Durable Object: upgrades, the per-IP rate-limit binding, hibernation and forwarding. It calls the rules in `room.js` and defines none. |
| `test/room.test.mjs` | `node --test`. No wrangler, no workerd, no account. |
| `wrangler.jsonc` | the deployment's configuration, including the rate-limit binding and `env.production`, the live worker. |

The split is why `room.js` exists: a Worker could not run on the machine this
was written on, so the rules live where `node --test` can reach them.
`apexd/apex-remoted/tests/relay.rs` proves the wire separately, against a
loopback double.

`src/index.js` is serving. This directory's own tests still do not cover the
Worker under `workerd`. `room.test.mjs` proves the rules, and
`android/app/src/androidTest/.../RelayOnDeviceTest.kt` proves the deployment
from outside by pairing a real phone through it.

## Testing

```
cd relay && node --test
```

Eleven tests, no runtime dependencies, no build step.

No automated suite runs against `npx wrangler dev --local`. The rate limit, room
cap and frame rules were measured against it by hand when they landed, with
`apex-remoted`'s relay suite passing 8 of 8, but nothing repeats that on a
change. The live deployment stands in: `run-device-suite.sh` starts
`apex-remoted` with `--relay` pointed at the real relay, so three of its tests
fail if the Worker stops behaving. That gate needs the internet, which makes it
weaker than `workerd` in one way. In every other way it is stronger, because it
exercises the TLS termination, the route and the Durable Object as deployed.
