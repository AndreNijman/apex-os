# apex-remote-relay

The meeting point for APEX Remote when a device is not on the same network as
the desktop. A Cloudflare Worker and one Durable Object per rendezvous id.

It copies bytes between two WebSockets that both dialled it. It terminates no
encryption, holds no key, stores nothing, and is not trusted: the `Noise_IK`
channel runs end to end through it, so every binary frame it moves is
ciphertext whose keys it never sees.

**It is deployed**, at `wss://apex-relay.andrenijman.com` — a custom domain on
a zone this account owns rather than the shared `*.workers.dev` name, and
`wrangler.jsonc` says why. What it cost and what it exposes is in
`ROADMAP/design/P1-052-relay.md`; the thing still open there is that it is
unauthenticated and unrate-limited on a public name.

Both clients dial it. The desktop is `apex-remoted --relay <url>`, which holds
a rendezvous outbound and opens no inbound port; the phone is
`android/core/.../Relay.kt` plus `RelayDialler`, which was the missing half
until 2026-09-19 — `PairingService` used to say so in its own error text.

## Layout

| file | what |
| --- | --- |
| `src/room.js` | every rule, as pure functions. No Cloudflare in it. |
| `src/index.js` | the Worker and the Durable Object. Upgrades, hibernation, forwarding, and no rules. |
| `test/room.test.mjs` | `node --test`. No wrangler, no workerd, no account. |
| `wrangler.jsonc` | what a deployment would use. |

The split is why `room.js` exists: a Worker cannot be run on the machine this
was written on, so the rules live where `node --test` can reach them. The wire
itself is proven separately, against a loopback double, by
`apexd/apex-remoted/tests/relay.rs`.

`src/index.js` has been executed — it is serving. What this directory's own
tests still do not cover is the Worker under `workerd`: `room.test.mjs` proves
the rules, and the deployment is proved from outside by
`android/app/src/androidTest/.../RelayOnDeviceTest.kt`, which pairs a real
phone through it. That is the honest state of it.

## Testing

```
cd relay && node --test
```

Nine tests, no dependencies, no build step.

The step this repository still cannot take is `npx wrangler dev --local` with a
suite pointed at it instead of at a double — there is no JavaScript toolchain
here. What stands in for it is the live deployment: `run-device-suite.sh`
starts `apex-remoted` with `--relay` pointed at the real one, so three of its
tests fail if the Worker stops behaving. That is a weaker gate than `workerd`
in one way — it needs the internet — and a stronger one in every other, since
it exercises the TLS termination, the route and the Durable Object as
deployed.
