# Building locally instead of waiting for CI

Build on the Katana. Push to GitHub only when the work is finished.

## Why

| | base build |
|---|---|
| Katana (i7-12700H, 20 threads, 62 GiB) | **3m41s** |
| GitHub runner, warm cache | 3h16m |
| GitHub runner, cache miss near the top of the file | **6h+ — killed at the job ceiling** |

The gap is not mostly CPU. `podman build --cache-to` pushes every intermediate
layer to ghcr as a separate image, so a CI build pays ~90 registry round trips.
A local build has no `--cache-to` at all and keeps its layers on disk.

That difference decides what kind of feedback loop you get. Four real bugs in
this repo were each found in under four minutes locally, and every one of them
would have surfaced only as a step exit code hours into a CI run:

- a `bwrap` smoke test that cannot work under `--isolation=chroot`
- an assertion sourcing a script that self-gates on a binary copied later
- two Hyprland window rules written in a syntax newer than the pinned 0.51.1,
  which Hyprland rejects and then starts anyway, leaving them inert
- an assertion reading `/usr/share/apex/labwc/rc.xml` 240 lines above its COPY

## Setup, once

```
ssh katana
mkdir -p ~/build && cd ~/build
git clone https://github.com/AndreNijman/apex-os.git
```

`core` is public, so pull it rather than spending 45 minutes rebuilding it:

```
sudo podman pull ghcr.io/andrenijman/apex-os-core:latest
sudo podman tag  ghcr.io/andrenijman/apex-os-core:latest localhost/apex-os-core:latest
```

`build-local.sh` reuses `localhost/apex-os-core:latest` when it exists.

## The loop

```
ssh katana 'cd ~/build/apex-os && git fetch -q && git reset --hard origin/<branch> \
  && ./build-local.sh --allow-unsigned base'
```

`--allow-unsigned` because the MOK key is not on the Katana. Iteration builds do
not need a signed kernel; the final CI build signs. `build-local.sh` refuses to
produce an unsigned image without that flag, which is the behaviour to keep.

Rebuild core locally only when `Containerfile.core` changes:

```
./build-local.sh --force-core core base
```

## Verify what you built

The build's own assertions are the first check, but they only prove the layers
succeeded. Look inside:

```
sudo podman run --rm localhost/apex-os-base:latest bash -c '
  apex --version; apex agent --help | head -3
  test -x /usr/bin/apex-agentd && echo agentd ok
  sed -n "s/^org.freedesktop.impl.portal.ScreenCast=//p" \
    /etc/xdg-desktop-portal/labwc-portals.conf'
```

`/dev/kvm` and working cgroups are both present on the Katana, so
`bootc-image-builder` can produce a qcow2 or an ISO there too — unlike the old
Void box, where an empty `/sys/fs/cgroup` under runit made that impossible.

## What a local build cannot tell you

- **Signing.** `--allow-unsigned` skips the MOK chain entirely. Secure Boot is a
  product invariant and only the CI build proves it.
- **Provenance.** No cosign signature, no source-revision label worth trusting.
- **Fleet update size.** Layer digests differ from a CI build, so
  `docs/update-cost.md`'s question cannot be answered here.
- **Anything needing a GPU session.** The image builds; the desktop is not
  running. See `docs/labwc-verification.md`.

So: local for the loop, CI for the artefact. Push when it works, not to find out
whether it works.
