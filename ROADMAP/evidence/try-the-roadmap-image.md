# Trying the roadmap image, and getting back

**This is the gate.** Eight roadmap items are `partial` for one reason: a
container can move a reader from "not installed" to "installed and not
running", never to "working". Nobody has booted this program's work. One
reboot answers all eight.

## What is published, and why this one is different

```
ghcr.io/andrenijman/apex-os:apex-266dcc572c51bdf9ec421d79eaa8784583184cd2
  digest   sha256:61f7dea51d73d71eb7da562f1fc7f4ef7b12f31699eb4266075e63944d4f69c4
  built    2026-09-18T14:17:33Z
  apex-os  266dcc57  (roadmap/v2.2)
  shell    b7953e4f  (apex-shell roadmap/v2.2)
```

**It is the first image containing BOTH halves of the work.** Every earlier
build pinned `apex-shell` to `main`, which is 158 commits behind
`roadmap/v2.2`, so the OS half shipped and the shell half did not — the labwc
parity, per-output scaling, the privacy page, the greeter accessibility, the
lid page. Nothing you have ever booted has had them.

The machine currently runs `:daily`, which is the nightly build of `main` from
**2026-09-05**. So this is not a small step forward.

## Before

Note what you are on, so the way back is a fact rather than a memory:

```
rpm-ostree status
```

Today that is `sha256:5e206de5…` (apex 2026-09-05T03:29:10Z) booted, with
`sha256:308127d9…` retained beside it.

## Rebase

```
sudo rpm-ostree rebase \
  ostree-unverified-registry:ghcr.io/andrenijman/apex-os:apex-266dcc572c51bdf9ec421d79eaa8784583184cd2
systemctl reboot
```

`rebase` only stages; nothing changes until the reboot. Same
`ostree-unverified-registry:` transport the machine already uses — this is not
a change of trust model, only of tag.

**One honest caveat.** A crash before a clean shutdown discards a staged ostree
change: the machine comes back on the old image and nothing is lost, but
nothing is gained either. If it looks as though the rebase "did not take",
that is the first thing to check.

## Getting back

The previous deployment is retained automatically. Two commands, and you are
where you started:

```
sudo rpm-ostree rollback
systemctl reboot
```

Nothing on `/var` or `/etc` is touched by either direction, and `/home` is not
involved at all. Staying on the new image costs nothing while you decide.

## After the reboot

Run the checker, which reads rather than asserts:

```
ROADMAP/evidence/verify-roadmap-image.sh
```

It reports which digest actually booted, whether the boot was clean, and the
eight items that were waiting on exactly this. It only reads; it changes
nothing and needs no privilege.

## What it does NOT do

It does not close P0-001. That item wants a *fresh install* on real hardware
across the Daily/Gaming matrix and three GPU vendors, and a rebase on one
laptop is not that. What it closes is the eight items whose blocker is the
sentence "nobody has booted it".
