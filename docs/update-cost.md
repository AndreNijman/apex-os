# Why `apex update` used to download the whole OS, and what changed

## The measurement

On the author's L16, running the published `:daily` image, against the registry:

```
$ sudo apex update --check
apex: running: bootc upgrade --check
Update available for: docker://ghcr.io/andrenijman/apex-os:daily
  Version: daily
  Digest: sha256:2bc521664ed5b673392317d4ee01bcab54b16a6999d808bfadc683378a36776d
Total new layers: 153   Size: 5.3 GB
Removed layers:   152   Size: 5.4 GB
Added layers:     153   Size: 5.3 GB
```

**153 of 153 layers, 5.3 GB — on every single update.** Not the first update
after a big change; every update, including ones whose only content difference
was a one-line edit to a shell script.

## Why

`bootc` (via ostree-ext) fetches container layers whole, and skips a layer only
when it already holds a blob with that *exact digest*. Three things combined to
guarantee it never held one:

1. **Everything lived in one image.** `Containerfile.base` carried the CachyOS
   kernel, the firmware set, the whole desktop stack, codecs, the baked
   applications, the font stack, the dev toolchain — and also the branding
   files, the apexd binaries and the vendored shell.

2. **Its rebuild trigger was almost every commit.** The base job's path filter
   covered `files/**`, `apexd/**` and `config/**`. Those are the directories
   that actually change.

3. **A rebuild produces new digests even for identical content.** CI builds with
   no layer cache (an earlier attempt at a registry cache was removed as
   unreliable). Every `dnf` transaction rewrites the ~200 MB sqlite rpmdb into
   its layer, and rpm records install timestamps, so "install the same packages
   again" does not reproduce the same bytes.

So editing one line of QML re-issued ~90 layers, and the fleet re-downloaded the
operating system.

## The fix: a third tier

The image is now built in three tiers instead of two:

| Tier | File | Contents | Rebuilds when |
|------|------|----------|---------------|
| **core** | `Containerfile.core` | kernel + MOK signing, firmware, desktop/greeter stack, scx, Bazaar, codecs, baked apps, printing, input methods, fonts, dev toolchain, zsh/starship, awww/matugen/yazi, OS branding & locale | `Containerfile.core` or `kernel/**` changes · `force_core` · the weekly cron finds a **new** `fedora-bootc` digest |
| **base** | `Containerfile.base` | apexd + apex CLI, sysprofiles, D-Bus/polkit/units, every `files/**` COPY, the vendored APEX Shell | `Containerfile.base`, `apexd/**`, `config/**`, `files/**` change, or core rebuilt |
| **image** | `Containerfile.apex` | edition stamp, gaming-session files, Plymouth theme, final initramfs | every run |

The GPU stack, the Mesa leg and `power-profiles-daemon` used to sit in the
flavor tier. They are in `core` now, and that is a download change, not a
tidiness one: measured on the published `:daily` manifest, its flavor tier was
342 MiB over six layers, of which **67 MiB was two `dnf` transactions**
(`power-profiles-daemon`, and the Terra Mesa `distro-sync`) whose only real
content was a rewritten ~200 MB sqlite rpmdb. Every push shipped those to every
machine. In `core` they are inside a digest-pinned parent nobody re-downloads,
so collapsing three images into one — while *adding* the NVIDIA driver to every
machine — made the per-update download smaller, not larger.

The base is built `FROM ghcr.io/andrenijman/apex-os:core@sha256:…`. **A digest-pinned `FROM` reuses
the parent's layer descriptors verbatim** — the derived manifest lists the same
digests, so `bootc` recognises blobs it already has and downloads none of them.

That is the whole mechanism. It needs no build cache and no registry cache, and
it cannot silently stop working: if the core digest moves, the layers move; if
it does not, they do not.

### The rule for new content

> If it runs `dnf`, downloads, or compiles a third-party program, it belongs in
> `Containerfile.core`. The base may only COPY repo content, compile apexd, and
> assert against those.

A `RUN` that needs both must be **split across the two files**, not moved
wholesale. Four in the original file straddled the line and are now pairs:

- the zsh/starship verification (packages in core, templates in base)
- the Windows-boot helper (`efibootmgr` in core, helper + sudoers in base)
- the icon-cache and `dconf` rebuilds (they must follow the COPYs, so: base)
- the Hyprland template guards (base)

Two contracts cross the tier boundary and must survive any future edit:
`/usr/lib/apex-kver` (core → the gaming NVIDIA akmod) and
`/usr/share/apex-os/secureboot/kernel-signed` (core → base and flavor
verification). Both are plain files under `/usr`, and CI asserts both.

### The weekly rebuild

The cron used to rebuild unconditionally. That would now be the dominant cost:
six days of ~50 MiB updates and one Monday of 5 GB, most weeks for nothing.

So core stamps the digest of the `fedora-bootc` image it was built from as
`org.apexos.fedora-bootc.digest`, and the scheduled run compares that label
against the live upstream digest. Same digest → no rebuild. This does not skip
security updates: a Fedora base respin *changes* the digest, which is exactly
the trigger. COPR or RPMFusion moving without a Fedora respin is not picked up
until the next core-relevant change — run the workflow with `force_core=true` to
take those immediately.

### What must NOT be in the core path filter

`build-image.yml` itself. It was, at first, and the next CI-only commit — adding
a retry around `podman push` — rebuilt core and reissued the whole ~5 GB image to
every machine, for a change that could not alter core's content by one byte.
Rebuilding core is the most expensive thing this workflow can do, so it is driven
by core's real inputs (`Containerfile.core`, `kernel/**`) and nothing else.

A workflow change that genuinely alters *how* core is built — a new
`--build-arg`, a different base tag — therefore will not rebuild it on its own.
That is what `force_core=true` is for: an explicit action for the rare case
instead of a multi-gigabyte download for the common one.

### Measuring it

Every flavor push writes an update-cost table into the GitHub Actions run
summary: total layers, how many are inherited from core, and how many are new.
If a future change quietly pushes content back down into core, that number
climbs and the regression is visible the week it happens rather than the month
someone next runs `apex update` on a hotel connection.

## Also changed: the firmware half of `apex update`

`apex update` ran, unconditionally, on every invocation:

```sh
fwupdmgr refresh --force     # re-download the entire LVFS metadata index
fwupdmgr update -y           # full device enumeration + update pass
```

`--force` means "ignore the cache age". fwupd considers its metadata stale after
24 hours; forcing it re-downloaded tens of MB of signed XML every run. The
update pass then enumerated every device on a machine that, nine runs in ten,
had nothing to install.

Now:

- `fwupdmgr refresh` **without** `--force`, honouring fwupd's own cache window;
- `fwupdmgr get-updates` first, and the update pass only if it reports something;
- fwupd's exit codes are read correctly. `fwupdmgr` returns **2** for "nothing to
  do" and **3** for "nothing found" — both are the *normal* outcome on a current
  laptop. The old code took the maximum of every exit code, so simply dropping
  `--force` would have made `apex update` report failure on its most common path.

New flags: `apex update --check` (report only, download nothing),
`--skip-firmware`, `--firmware-only`.

## Also changed: `apex update` requires root

`update`, `rollback`, `pin` and `fan restore --local` now refuse to run
unprivileged, before any hardware probe or subprocess:

```
$ apex update --check
apex: 'update' changes the booted system and must run as root.
       try:  sudo apex update --check
       (being in the wheel group is not enough — bootc writes to /ostree and /boot,
        so the command itself has to run with privileges.)
```

Previously they reached `bootc`/`ostree` and failed there with a bare permission
error that never mentioned sudo — and `apex update` then ran its firmware half
anyway, printing a wall of output and potentially exiting 0 having updated
nothing.

This is deliberately **not** the whole CLI. `apex tier`, `status`, `battery`,
`fan`, `game` and `doctor` stay usable unprivileged: APEX Shell's power tab
shells out to `apex tier` as the session user, and mutations go through apexd's
polkit-authorised D-Bus API — which is how an unprivileged desktop is supposed
to change power state. Gating those would break the desktop's power controls in
order to improve an error message.

## CI build time — what was measured, and what actually helped

Profiled rather than guessed (run 30775672845):

| step | base | daily |
|------|------|-------|
| build | 21.9 min | 5.1 min |
| **push** | **12.9 min** | **14.3 min** |
| disk cleanup | 2.1 min | 1.6 min |

Pushes dominate, and they are bandwidth-bound (~14 MB/s to GHCR), not CPU-bound.
Counting blob operations found the waste: every image was uploaded **twice**,
once as `:<tier>-<sha>` and again as the friendly tag — 166 blobs, then 166 more,
zero reused.

**What helped**

- *Push once, then tag with a registry-to-registry copy.* A `docker://` →
  `docker://` copy knows its blob digests up front, so it skips every layer and
  writes only a manifest. Measured: base push 12.9 → **6.2 min**.
- *Stop pre-emptively cleaning the runner disk.* The step was written when
  runners had ~14 GB free. They now have 145 GB with **88 GB free before
  cleaning**, so it was reclaiming 31 GB nobody needed at 4.2 min × 7 jobs. It
  now only sweeps below a threshold.

**What did not help, and why — recorded so it is not retried**

Consolidating the tiers into one repository, in the hope the registry would skip
inherited layers. It does not, and no client-side flag changes that:

    push core (zstd:chunked) ............. 99 blobs copied,  0 skipped
    push base into the SAME repo ........ 159 blobs copied,  0 skipped
    push the IDENTICAL core ref again .... 99 blobs copied,  0 skipped
    skopeo instead of podman ............. 159 blobs copied,  0 skipped
    no --compression-format at all ....... 159 blobs copied,  0 skipped

Compressing out of `containers-storage` only produces the blob digest *after*
compression, so there is nothing to ask the registry about first. The remaining
push cost is inherent: ~5 GB compressed and uploaded per image.

Building the flavors inside the base job was also considered — it would remove
one 5 GB upload and three downloads — and rejected: each flavor would build its
own base, so the three editions would no longer be provably pinned to one
identical, verified base image. That guarantee is worth more than the minutes.
