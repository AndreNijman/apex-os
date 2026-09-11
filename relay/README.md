# apex-remote-relay

The meeting point for APEX Remote when a device is not on the same network as
the desktop. A Cloudflare Worker and one Durable Object per rendezvous id.

It copies bytes between two WebSockets that both dialled it. It terminates no
encryption, holds no key, stores nothing, and is not trusted: the `Noise_IK`
channel runs end to end through it, so every binary frame it moves is
ciphertext whose keys it never sees.

**It is not deployed.** No `wrangler deploy` has been run and no Cloudflare
credential has been used. What deploying would require — including the one
thing that would stop it working today, a client that cannot dial `wss://` —
is written out in `ROADMAP/design/P1-052-relay.md`, with the cost.

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

`src/index.js` has never been executed. That is the honest state of it.

## Testing

```
cd relay && node --test
```

Nine tests, no dependencies, no build step.

The pre-deploy step this repository could not take is `npx wrangler dev --local`
with the Rust suite pointed at it instead of its double.
