# Update channels, and the stop that keeps one bad release from becoming two

Roadmap §26.

## Which channel you are on right now

```bash
apex channel status
```

If your machine was installed before this existed, the answer will look like
this, and it is worth reading rather than skipping:

```
  following    : daily
  which is     : edge — `daily` moves with every build of main, so it is the
                 edge channel under the name it had before channels existed
```

`apex`, `daily`, `gaming-mesa` and `gaming-nvidia` are four names for one image,
and CI moves all four on every successful build of `main`. That is the
definition of an edge channel. Every APEX machine in existence has been on edge
since it was installed, and until now nothing said so.

Those four tags keep working and keep moving. They are not deprecated aliases to
be cleaned up: they are what installed machines point `bootc` at, and a tag that
stops moving does not produce an error — `bootc upgrade` reports "no update
available" forever, and that machine quietly stops receiving security updates.

## The four channels

```bash
apex channel list
```

| channel | what arrives | what it costs |
|---|---|---|
| `edge` | every successful build of `main`, as soon as it is published | new work arrives first, and so do its faults |
| `beta` | a build that has been on edge and looks sound | a wait |
| `candidate` | a build being considered for stable | a longer wait |
| `stable` | only builds that have been through the other three | the fewest updates, and the longest wait for a fix |

That last row is enforced in CI rather than promised in prose: a promotion
refuses a digest that is not already on the channel above the one being moved,
and it refuses a digest that is not cosign-signed by this repository's build
workflow on `main`.

## Moving

```bash
sudo apex channel set beta --dry-run   # prints what it would run
sudo apex channel set beta
sudo apex update
```

`set` writes the deployment origin through `bootc switch`, so it needs root.
`status`, `list` and `report` do not: the channel comes out of the deployment's
own origin file and `/etc/machine-id`, both world-readable. A command that
answers "which channel am I on" must not want a password, because the moment
somebody asks that is usually the moment something has gone wrong.

### Moving toward stable is a downgrade, and it says so

Going from `edge` to `stable` usually deploys an **older** image. Two things
happen, and `set` prints both before it does anything:

**The current deployment is pinned first.** bootc keeps the booted deployment
and one more, so switching backwards and then updating once can evict the
deployment you would want to return to. `ostree admin pin 0` runs before the
switch, and if the pin fails the switch does not happen.

**Your saved settings do not go back with the image.** `/usr` is replaced;
`/etc`, `/var` and your home are not. An older APEX reading a config a newer one
migrated is exactly what `docs/state-migration.md` is about, so `set` prints
each store's answer:

```
  blueprint        an older APEX refuses it by name and says so; nothing is
                   misread, and nothing is lost
  task-state       an older APEX still reads it: every migration only adds keys,
                   and the reader keeps the ones it does not know
```

`apex schema status` says which of those files your machine has.

## The rollout stop

§26 asks that a rollout can stop on health regressions. On one machine that
means: if the machine came back from its last update with a problem an image
change could have caused, the next `apex update` refuses.

```
apex: this machine came back from its last update with a problem, so the next
      one is being held.
  gpu-driver: no driver is bound to the discrete GPU
  systemd unit failed: apex-shell.service

Go back with `sudo apex rollback`, then reboot.
Take it anyway with `sudo apex update --force`.
```

The verdict comes from the same probes `apex recover status` uses, plus
`systemctl --failed`. It does **not** come from `/var/lib/apex/boot/last-health.json`:
that file is written by a unit conditioned on systemd-boot's
`LoaderBootCountPath`, every published APEX image boots GRUB, and so the unit has
never run on any machine. A verdict built on it would be permanently empty and
permanently green.

Four things count as a regression, and what does not count is the more important
half:

- **counts**: the GPU driver, APEX Shell, the filesystem, package extensions.
  An image change can break each, and a rollback can fix each.
- **does not count**: everything else. A machine with no default route has a
  problem the update did not cause, and refusing the update because the wifi is
  off strands you on the release that broke you.
- **does not count, and is still said**: a row that could not be measured. That
  is a fact about the reader, not about the machine, so it is reported —
  "so this verdict is partial" — without failing the gate.

## Staged rollout

Every machine has a rollout slot, 0 to 99, derived from `/etc/machine-id`:

```
  rollout slot : 46 of 100 — a staged release at 47% or more reaches this
                 machine. It is derived from this machine's id and is never
                 sent anywhere
```

It is stable across reboots on purpose. A machine that re-rolled on every check
would drift into a 1% cohort it was never part of, which defeats the point of
staging.

### Where the percentage lives, and why it is not a label

A ramp — 1%, then 5%, then 25% — needs a number that changes **between** builds.
An OCI label is baked once per image, so a label cannot be it. That observation
held this item open for a while, and the conclusion drawn from it ("so a staged
rollout needs a server") was the wrong one.

The number lives in a **signed rollout document**: one small JSON object
published at `ghcr.io/andrenijman/apex-os:rollout`, in the same repository as
the image, re-published by `promote-channel.yml` every time the number moves.

```json
{
  "schema": 1,
  "serial": 12,
  "issued": 1790000000,
  "expires": 1791209600,
  "repository": "ghcr.io/andrenijman/apex-os",
  "channels": [
    { "channel": "candidate", "digest": "sha256:daf8c8eb…", "percent": 25 },
    { "channel": "beta", "digest": "sha256:1f09ab…", "percent": 100,
      "halt": true, "reason": "gpu-driver fails to bind on RTX 30-series" }
  ]
}
```

```bash
apex channel rollout          # fetch it, verify it, and say what it means here
apex channel rollout --offline  # decide on the last one this machine accepted
```

**Why a registry object rather than an endpoint.** Three things were on the
table: a fleet endpoint APEX would run, a separately-updatable object in the
registry, and a signed JSON on a static host. The registry object was chosen
and the reasons are worth writing down because they are what makes this
optional:

- **No server.** There is nothing to run, nothing to keep up, and nothing whose
  outage stops machines updating. A first-party endpoint would make APEX
  operate a service for the first time, and the first version of that service
  would be a single point of failure for every machine's update path.
- **No new name to trust.** The machine already contacts this registry on every
  update and already verifies cosign signatures from it, with a Fulcio root
  pinned in the image. The document reuses that verification whole — see
  `apexd/apex/src/verify.rs`, which is the same code that checks the image.
- **No new fact about the machine.** An endpoint learns which machines asked and
  when, which is a liveness map of everybody who installed APEX. A registry pull
  the machine was making anyway reveals nothing the update did not already.
- **One number of infrastructure, and it is zero.** Publishing is a
  `workflow_dispatch`, and the object is about 400 bytes.

**What it costs, stated rather than buried:**

- **A document in a registry is a broadcast.** A ramp is per-channel and never
  per-machine. There is no way to say "these fifty machines first" without
  something that knows which machine is asking — which is a fleet, which is
  `docs/fleet.md`, which is optional and which nobody is enrolled in.
- **There is no back channel.** Nobody learns how the rollout is going except
  from the opt-in health report below, which is off by default and which APEX
  operates no endpoint for. A ramp published here is a decision made on
  evidence from somewhere else.
- **A halt takes effect at the machine's next `apex update`.** A machine that is
  off takes nothing and hears nothing.
- **Latency is the registry's.** A moved tag is visible when the registry serves
  it, which is immediately, and read when the user next updates, which is not.

### What the client does with it

In this order, and no other:

1. Resolve `…:rollout` to a digest.
2. Verify the cosign signature over that digest, under the **rollout identity** —
   `…/promote-channel.yml@refs/heads/main`, derived from the image signer by
   swapping the workflow filename. The image's own identity is deliberately not
   accepted: the document has to be publishable between builds, so it cannot
   carry the build's signature.
3. Only then read the bytes — and read the object whose signature was checked,
   not whatever the tag answers on the second round trip. The manifest fetched
   has to hash to the digest step 1 resolved, or nothing is read. Without that,
   the sequence would be "verify one object, read another", and a tag that moved
   in between would put a signature over one document behind the bytes of a
   different one. A document whose signature does not verify is not a document
   with a problem — it is not a document, and nothing in it reaches the
   decision.

Then the entry for this machine's channel is applied, if it is usable. Each of
these makes it unusable, and each one is said out loud rather than swallowed:

| what | why it is refused |
|---|---|
| its `digest` is not what this channel resolves to now | the entry is about a build that has been superseded; without this a 5% entry for one build silently gates the next one |
| `expires` has passed | a publisher who stops publishing must not pin every machine to a last instruction forever |
| `issued` is more than 30 days ago | the same bound, enforced by the client, so that an expiry ten years out is still an expiry |
| `issued` is more than an hour in the future | a machine with a wrong clock, or a document minted for a date nobody has reached |
| `serial` is lower than one this machine already accepted | an old validly-signed "everybody takes this", re-served after a halt, is a downgrade attack that needs no key at all |
| `repository` is not this machine's | a document lifted from one repository and served from another |
| `schema` is higher than this build reads | read whole or not at all; half a policy is not a policy |

Everything else admits. A machine that cannot reach the registry, cannot resolve
its tag, or is on a tag that is not a channel updates exactly as it does today —
and that costs nothing, because a machine that could not resolve the tag could
not pull the image either.

The last accepted document is cached at `/var/lib/apex/channel/rollout.json`.
That is not for speed: without it, blocking the `rollout` tag would defeat a
halt — the fetch fails, no document applies, and the machine takes the release
the publisher stopped. A halt anybody can undo with a firewall rule is not a
halt.

### What a hold looks like

```
apex: this release is being rolled out gradually and has reached 25% of
apex: machines. This one is slot 60 of 100, so it is not its turn yet — nothing
apex: is wrong with it. Try again later, or `sudo apex update --force` to take it now.
```

`apex update` exits **0** for this, and goes on to update packages, flatpaks and
firmware. A machine outside a ramp is not broken and has nothing to fix; a
staged rollout is about the OS image and nothing else.

A halt reads differently, because the cause is different:

```
apex: the publisher has stopped this release, so it is not being installed.
apex:   gpu-driver fails to bind on RTX 30-series
apex: A later build will supersede it. `sudo apex update --force` takes it anyway.
```

### What is still not built

- **Nothing has published a rollout document yet.** `promote-channel.yml` has
  the steps and has never been run; the client half is exercised end to end
  against real cryptography by `tests/test-apex-rollout.sh`, which builds its
  fixture by *running the workflow's own python* rather than copying it.
- **The halt is a publisher decision, not an automatic one.** Nothing counts
  health reports and halts a release by itself, because nothing receives health
  reports — see the next section. The loop is closed by a person reading
  evidence and dispatching a halt.
- **The four channel tags do not exist in the registry.** Measured 2026-09-22:
  `:stable`, `:candidate`, `:beta` and `:edge` all answer `manifest unknown`.
  Only `apex`, `daily`, `gaming-mesa` and `gaming-nvidia` resolve, to one digest
  whose `org.opencontainers.image.revision` is `57f593ad` — `main`'s tip from
  2026-09-05. The workflow step that creates `edge` and the promotion workflow
  are both on `roadmap/v2.2` and have never run on `main`, which is what
  publishes. Until a build of `main` carries them, `apex channel set beta` would
  point a machine at a tag the registry does not serve, so `set` resolves the
  target first and refuses:

  ```
  apex: ghcr.io/andrenijman/apex-os:beta does not resolve: manifest unknown
  apex: switching to a tag the registry does not serve would leave this machine
  apex: with nothing to update to, and `bootc upgrade` would report that as
  apex: "no update available" forever. Not switching.
  ```

  `--force` overrides it. A registry that cannot be reached at all does not
  block the switch: that is a fact about the network, not about the tag, and the
  two are told apart by what the registry said rather than by whether it
  answered. A machine that switches unchecked is told so, in those words, so the
  user knows which of the two happened.

## What is sent, and to whom

**Nothing, to nobody.** APEX operates no telemetry service.

```bash
apex channel report
```

prints the exact payload it would send, and then says nothing was sent. The
payload is five fields:

```json
{
  "channel": "edge",
  "tag": "daily",
  "digest": "sha256:308127d9...",
  "healthy": true,
  "reasons": []
}
```

Not in it: the machine id, the rollout slot, the hostname, the hardware, the
installed packages, the user, the network. The rollout slot is left out
deliberately — one of a hundred values derived from the machine id is a weak
identifier and a health report has no use for it.

The opt-in is `~/.config/apex/channel.toml`:

```toml
report = true
endpoint = "https://example.invalid/apex-health"
```

The file does not exist by default and the default is off. A file that cannot be
read is also off: a consent that fails open is not consent.

Turning it on and pointing it at an endpoint still sends nothing in this build.
The transmitter is the piece of §26 that is not implemented, and there is no
first-party endpoint to point it at. That is stated here rather than solved with
a URL, because writing one into the source would not create a service behind it.
