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

## Staged rollout, and the part that is not built

Every machine has a rollout slot, 0 to 99, derived from `/etc/machine-id`:

```
  rollout slot : 46 of 100 — a staged release at 47% or more reaches this
                 machine. It is derived from this machine's id and is never
                 sent anywhere
```

It is stable across reboots on purpose. A machine that re-rolled on every check
would drift into a 1% cohort it was never part of, which defeats the point of
staging.

What is **not** built: nothing publishes a percentage yet. The gate reads it
from an image label and treats an image without one as reaching everybody, which
is what every APEX image published so far is. A ramp — 1%, then 5%, then 25% —
needs the number to change between builds, and a label is baked once. That
pointer does not exist, so the ramp does not either.

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
