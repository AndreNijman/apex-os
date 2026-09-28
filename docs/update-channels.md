# Update channels, and the stop that keeps one bad release from becoming two

Roadmap §26.

## Which channel you are on right now

```bash
rime channel status
```

If your machine was installed before channels existed, the answer looks like
this. Read it:

```
  following    : daily
  which is     : edge — `daily` moves with every build of main, so it is the
                 edge channel under the name it had before channels existed
```

`rime`, `daily`, `gaming-mesa` and `gaming-nvidia` are four names for one image,
and `build-image.yml` moves all four, with `edge`, on every successful build of
`main`. That is the definition of an edge channel. Every Rime machine has been
on edge since it was installed, and before §26 nothing said so.

Those four tags keep working and keep moving, because installed machines point
`bootc` at them. Do not clean them up as deprecated aliases. A tag that stops
moving produces no error: `bootc upgrade` reports "no update available" forever,
and that machine stops receiving security updates without a word.

## The four channels

```bash
rime channel list
```

| channel | what arrives | what it costs |
|---|---|---|
| `edge` | every successful build of `main`, as soon as it is published | new work arrives first, and so do its faults |
| `beta` | a build that has been on edge and looks sound | a wait |
| `candidate` | a build being considered for stable | a longer wait |
| `stable` | only builds that have been through the other three | the fewest updates, and the longest wait for a fix |

CI enforces that last row. A promotion refuses a digest that is not already on
the channel above the one being moved, and it refuses a digest that is not
cosign-signed by this repository's build workflow on `main`.

## Moving

```bash
sudo rime channel set beta --dry-run   # prints what it would run
sudo rime channel set beta
sudo rime update
```

`set` writes the deployment origin through `bootc switch`, so it needs root.
`status`, `list` and `report` do not: the channel comes out of the deployment's
own origin file and `/etc/machine-id`, both world-readable. A command that
answers "which channel am I on" must not ask for a password: people usually ask
that question when something has gone wrong.

### Moving toward stable is a downgrade, and it says so

Going from `edge` to `stable` usually deploys an **older** image. Two things
happen, and `set` prints both before it does anything:

**`set` pins the current deployment first.** bootc keeps the booted deployment
and one more, so switching backwards and then updating once can evict the
deployment you would want to return to. `ostree admin pin 0` runs before the
switch, and if the pin fails the switch does not happen.

**Your saved settings do not go back with the image.** The switch replaces
`/usr` and leaves `/etc`, `/var` and your home as they are.
`docs/state-migration.md` covers an older Rime reading a config a newer one
migrated, and `set` prints each store's answer:

```
  blueprint        an older Rime refuses it by name and says so; nothing is
                   misread, and nothing is lost
  task-state       an older Rime still reads it: every migration only adds keys,
                   and the reader keeps the ones it does not know
```

`rime schema status` says which of those files your machine has.

## The rollout stop

§26 asks that a rollout can stop on health regressions. On one machine that
means: if the machine came back from its last update with a problem an image
change could have caused, the next `rime update` refuses.

```
rime: this machine came back from its last update with a problem, so the next
      one is being held.
  gpu-driver: no driver is bound to the discrete GPU
  systemd unit failed: rime-shell.service

Go back with `sudo rime rollback`, then reboot.
Take it anyway with `sudo rime update --force`.
```

The verdict comes from the same probes `rime recover status` uses, plus
`systemctl --failed`. It does **not** come from `/var/lib/rime/boot/last-health.json`.
A unit conditioned on systemd-boot's `LoaderBootCountPath` writes that file,
every published Rime image boots GRUB, and so the unit has never run on any
machine. A verdict built on it would stay empty and green forever.

Four things count as a regression, and what does not count is the more important
half:

- **counts**: the GPU driver, Rime Shell, the filesystem, package extensions.
  An image change can break each, and a rollback can fix each.
- **does not count**: everything else. A machine with no default route has a
  problem the update did not cause, and refusing the update because the wifi is
  off strands you on the release that broke you.
- **does not count, and is still said**: a row that could not be measured. That
  describes the reader and not the machine, so the verdict reports it ("so this
  verdict is partial") without failing the gate.

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

A ramp (1%, then 5%, then 25%) needs a number that changes **between** builds.
CI bakes an OCI label once per image, so a label cannot hold that number. The
observation held this item open for a while, and the conclusion drawn from it
("so a staged rollout needs a server") was wrong.

The number lives in a **signed rollout document**: one small JSON object
published at `ghcr.io/andrenijman/rime-os:rollout`, in the same repository as
the image, re-published by `promote-channel.yml` every time the number moves.

```json
{
  "schema": 1,
  "serial": 12,
  "issued": 1790000000,
  "expires": 1791209600,
  "repository": "ghcr.io/andrenijman/rime-os",
  "channels": [
    { "channel": "candidate", "digest": "sha256:daf8c8eb…", "percent": 25 },
    { "channel": "beta", "digest": "sha256:1f09ab…", "percent": 100,
      "halt": true, "reason": "gpu-driver fails to bind on RTX 30-series" }
  ]
}
```

```bash
rime channel rollout          # fetch it, verify it, and say what it means here
rime channel rollout --offline  # decide on the last one this machine accepted
```

**Why a registry object rather than an endpoint.** The candidates were a fleet
endpoint Rime would run, a separately updatable object in the registry, and a
signed JSON on a static host. Rime chose the registry object, and these reasons
are what make the rollout optional:

- **No server.** There is nothing to run, nothing to keep up, and nothing whose
  outage stops machines updating. A first-party endpoint would make Rime
  operate a service for the first time, and the first version of that service
  would be a single point of failure for every machine's update path.
- **No new name to trust.** The machine already contacts this registry on every
  update and already verifies cosign signatures from it, with a Fulcio root
  pinned in the image. The document reuses that verification whole:
  `rimed/rime/src/verify.rs` is the same code that checks the image.
- **No new fact about the machine.** An endpoint learns which machines asked and
  when, which is a liveness map of everybody who installed Rime. A registry pull
  the machine was making anyway reveals nothing the update did not already.
- **No infrastructure.** Publishing is a `workflow_dispatch`, and the object is
  about 400 bytes.

**What it costs:**

- **A document in a registry is a broadcast.** A ramp is per-channel and never
  per-machine. There is no way to say "these fifty machines first" without
  something that knows which machine is asking. That is a fleet
  (`docs/fleet.md`), which is optional, and no machine is enrolled in one.
- **There is no back channel.** Nobody learns how the rollout is going except
  from the opt-in health report below, which is off by default and which Rime
  operates no endpoint for. A ramp published here is a decision made on
  evidence from somewhere else.
- **A halt takes effect at the machine's next `rime update`.** A machine that is
  off takes nothing and hears nothing.
- **Latency is the registry's.** A moved tag is visible when the registry serves
  it, which is immediately, and read when the user next updates, which is not.

### What the client does with it

In this order:

1. Resolve `…:rollout` to a digest.
2. Verify the cosign signature over that digest, under the **rollout identity**:
   `…/promote-channel.yml@refs/heads/main`, which the client derives from the
   image signer by swapping the workflow filename. The client refuses the
   image's own identity here: the document has to be publishable between builds,
   so it cannot carry the build's signature.
3. Only then read the bytes, from the object whose signature was checked and not
   from whatever the tag answers on a second round trip. The fetched manifest has
   to hash to the digest step 1 resolved, or the client reads nothing. Without
   that check the sequence would be "verify one object, read another", and a tag
   that moved in between would put one document's signature behind a different
   document's bytes. The client treats a document whose signature does not
   verify as no document at all, and nothing in it reaches the decision.

Then the client applies the entry for this machine's channel, if the entry is
usable. Each of these makes it unusable, and the client reports each one instead
of swallowing it:

| what | why it is refused |
|---|---|
| its `digest` is not what this channel resolves to now | the entry is about a build that has been superseded; without this a 5% entry for one build silently gates the next one |
| `expires` has passed | a publisher who stops publishing must not pin every machine to a last instruction forever |
| `issued` is more than 30 days ago | the same bound, enforced by the client, so that an expiry ten years out is still an expiry |
| `issued` is more than an hour in the future | a machine with a wrong clock, or a document minted for a date nobody has reached |
| `serial` is lower than one this machine already accepted | an old validly-signed "everybody takes this", re-served after a halt, is a downgrade attack that needs no key at all |
| `repository` is not this machine's | a document lifted from one repository and served from another |
| `schema` is higher than this build reads | read whole or not at all; half a policy is not a policy |

Any other entry applies. A machine that cannot reach the registry, cannot
resolve its tag, or is on a tag that is not a channel updates exactly as it does
today. That costs nothing, because a machine that cannot resolve the tag cannot
pull the image either.

The client caches the last accepted document at
`/var/lib/rime/channel/rollout.json` so that blocking the `rollout` tag cannot
defeat a halt. Without the cache the fetch fails, no document applies, and the
machine takes the release the publisher stopped: anybody with a firewall rule
could undo the halt.

### What a hold looks like

```
rime: this release is being rolled out gradually and has reached 25% of
rime: machines. This one is slot 60 of 100, so it is not its turn yet — nothing
rime: is wrong with it. Try again later, or `sudo rime update --force` to take it now.
```

`rime update` exits **0** here and goes on to update packages, flatpaks and
firmware. A machine outside a ramp has nothing to fix, and a staged rollout
covers the OS image only.

A halt reads differently, because the cause is different:

```
rime: the publisher has stopped this release, so it is not being installed.
rime:   gpu-driver fails to bind on RTX 30-series
rime: A later build will supersede it. `sudo rime update --force` takes it anyway.
```

### What is still not built

- **Nothing has published a rollout document yet.** `promote-channel.yml` has
  the steps and has never run (checked 2026-09-28: the workflow has no runs, and
  `:rollout` answers `manifest unknown`). `tests/test-rime-rollout.sh` exercises
  the client half end to end against real cryptography, and builds its fixture
  by *running the workflow's own python* rather than copying it.
- **A person halts a release; nothing halts one automatically.** Nothing counts
  health reports, because nothing receives them (see the next section). A
  person closes the loop by reading evidence and dispatching a halt.
- **Three of the four channel tags do not exist in the registry.** Measured
  2026-09-28: `:edge` resolves to the same digest as `rime`, `daily`,
  `gaming-mesa` and `gaming-nvidia` (`sha256:8601baab…`, whose
  `org.opencontainers.image.revision` is `1094c432`), because `build-image.yml`
  moves it with them on every build of `main`. `:stable`, `:candidate` and
  `:beta` still answer `manifest unknown`: only `promote-channel.yml` creates
  them, and it has never run. (On 2026-09-22 `:edge` answered `manifest unknown`
  too, and the four resolved to `57f593ad`, `main`'s tip from 2026-09-05. The
  step that creates `edge` was then only on `roadmap/v2.2`.) Until a promotion
  runs, `rime channel set beta` would point a machine at a tag the registry does
  not serve, so `set` resolves the target first and refuses:

  ```
  rime: ghcr.io/andrenijman/rime-os:beta does not resolve: manifest unknown
  rime: switching to a tag the registry does not serve would leave this machine
  rime: with nothing to update to, and `bootc upgrade` would report that as
  rime: "no update available" forever. Not switching.
  ```

  `--force` overrides it. A registry that cannot be reached at all does not
  block the switch, because that describes the network and not the tag. `set`
  tells the two apart by what the registry said, not by whether it answered, and
  when it switches unchecked it says so in those words, so you know which of the
  two happened.

## What is sent, and to whom

**Nothing, to nobody.** Rime operates no telemetry service.

```bash
rime channel report
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
installed packages, the user, the network. The report leaves out the rollout
slot on purpose: one of a hundred values derived from the machine id is a weak
identifier, and a health report has no use for it.

The opt-in is `~/.config/rime/channel.toml`:

```toml
report = true
endpoint = "https://example.invalid/rime-health"
```

The file does not exist by default, and reporting is off. A file that cannot be
read also counts as off, so consent cannot fail open.

Turning it on and pointing it at an endpoint still sends nothing in this build.
The transmitter is the piece of §26 that is not implemented, and Rime runs no
first-party endpoint to point it at. A URL written into the source would not
create a service behind it, so this document says so instead.
