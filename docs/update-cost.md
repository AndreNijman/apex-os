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
| **kernel** | `Containerfile.kernel` | the kernel itself, compiled from pinned source with a pinned `dwarves` | `kernel/**` changes — i.e. `kernel/kernel.pin` moves |
| **core** | `Containerfile.core` | kernel *install* + MOK signing, firmware, desktop/greeter stack, scx, Bazaar, codecs, baked apps, printing, input methods, fonts, dev toolchain, zsh/starship, awww/matugen/yazi, OS branding & locale | `Containerfile.core` or `kernel/**` changes · `force_core` · the weekly cron finds a **new** `fedora-bootc` digest |
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

### The fourth tier: the kernel

APEX builds its own kernel (`ROADMAP/evidence/kernel-build-20260920.md` says
why — every kernel it shipped before was unable to load a sched-ext scheduler).
That compile is ~45 minutes, and the obvious place for it is `core`, because
the rule below says anything that compiles a third-party program goes there.

It is not in `core`, for a reason this document is the right place to record:
**`core` is built with no layer cache.** There is no `--cache-from` any more and
scheduled and forced runs pass `--no-cache` outright. So a kernel compile inside
`core` is paid in full on every `core` rebuild — and the "Rebuilds when" column
above says `core` rebuilds for a `Containerfile.core` edit, a `force_core`, or a
new `fedora-bootc` digest. None of those are kernel changes.

The cost that matters is not the 45 minutes, though. It is that each of those
rebuilds would produce a **different kernel binary**: new `vmlinuz`, new
modules, new BTF, akmods rebuilt against it and re-signed — all as a side effect
of an edit that had nothing to do with the kernel, with nothing tying the kernel
to its own inputs. `kernel/kernel.pin` exists so that the kernel moves when the
kernel's inputs move and at no other time.

So the kernel uses the same mechanism `base` uses to consume `core`: a
separately published image, pinned by digest. `Containerfile.core` keeps the
`dnf` transaction that *installs* the RPMs — which is what the rule below is
actually about — and gets them from the kernel image:

```dockerfile
ARG APEX_KERNEL_IMAGE=localhost/apex-kernel:local
FROM ${APEX_KERNEL_IMAGE} AS kernel-rpms
…
COPY --from=kernel-rpms /rpms     /tmp/apex-kernel-rpms
COPY --from=kernel-rpms /manifest /tmp/apex-kernel-manifest
```

**The fleet download cost is unchanged.** `core` moving is a full multi-gigabyte
download either way; the kernel image itself is never pulled by a user, only by
the `core` build. What changes is that `core` stops moving for kernel reasons
and the kernel stops moving for `core` reasons.

Two things cross this new tier boundary and must survive any future edit, in the
same way `/usr/lib/apex-kver` crosses core→gaming: the manifest's `btf_scx`
verdict, which `core` refuses to install without, and its `kver`, which `core`
checks against the kernel that actually landed in the rpmdb. Both are copied to
`/usr/share/apex-os/kernel/` so a running machine can answer what it is booting
and what built its BTF.

#### What owning the kernel obliges us to, permanently

This is the half of the decision that is not a build cost, and it is the half
that outlives whoever took it.

Before, security updates arrived by themselves: CachyOS tagged a release, the
COPR rebuilt `kernel-cachyos`, and APEX picked it up on the next `force_core`.
Nobody had to do anything. Now `KERNEL_TAG` and `KERNEL_SRC_SHA256` in
`kernel/kernel.pin` decide which kernel APEX ships, and they move when a person
moves them.

The failure mode is quiet, which is what makes it dangerous. **An unbumped pin
is a kernel that stops receiving security fixes while every gate in this
repository stays green.** The sha256 still verifies — against the old tarball.
The BTF gate still passes — the old kernel's BTF is still fine. CI is all
ticks. Green here means "this is the kernel you pinned"; it has never meant
"this kernel is current", and no other check in the repository can tell the
difference.

So the obligation is not "remember to bump the kernel". It is a mechanism:

* **`tests/check-kernel-drift.sh`** asks whether each pinned input is still
  current — is there a newer stable `cachyos-7.2.x` tag (the security-update
  question), has the pinned `dwarves` moved in or out of Fedora, do all five
  pinned URLs still resolve. It has **three** outcomes, not two: `0` no drift,
  `1` drift, `2` a lookup could not be performed — which is neither a pass nor
  drift, and fails.
* **`.github/workflows/kernel-drift.yml`** runs it weekly, on every change to
  the pin, and on demand; it keeps a single issue open until the pin is current
  again. It compiles nothing.

Two things that check is *not*, so nobody relies on it for them. It does not
tell you a CVE exists — it tells you CachyOS has tagged something you are not
on. And it cannot tell you a kernel you already pinned has become unsafe; only
a newer tag can do that.

Bumping the pin costs a ~45-minute kernel-tier rebuild and then a `core`
rebuild, which is the usual ~5 GB to the fleet. That is the real recurring
price of owning the kernel, and it is per security update, not per year.

#### The CI question this tier cannot answer for itself

**Nothing builds the kernel image in CI yet, and it needs a decision rather
than an implementation.** `.github/workflows/build-image.yml` needs a `kernel`
job that runs before `core` and passes its digest as
`--build-arg APEX_KERNEL_IMAGE=…@sha256:…`. Writing that job is an afternoon.
Running it is the problem:

> **The build tree is ~100 GB** — measured, not estimated: `/var` on the
> development machine went 552 GB free to 453 GB during the compile. **A hosted
> GitHub runner has 14 GB.**

So this is not "slower on a hosted runner", it is *cannot run on one at all*,
before the CPU argument starts. The two realistic options, with what each
actually costs:

| option | what it costs | what it changes about the product |
|---|---|---|
| **Self-hosted runner on katana** | 20 cores and podman are already there, so the compile is roughly what it is locally. Needs ≥120 GB free on katana's `/var`, which is *tight* — check before committing. Adds a machine the release path depends on being up, and a self-hosted runner executing untrusted PR code is its own security decision. | Nothing. Same kernel, same config. |
| **Restructure the spec to build far fewer modules** (`_build_minimal 1` plus a `modprobed.db`) | Brings the tree within a hosted runner's disk. | **Changes what hardware the kernel supports**, because the module set is built from one machine's `modprobed.db`. That is a product decision about which machines APEX boots on, not a CI optimisation. |

Until one is chosen, the kernel image is built by hand and `core` is pointed at
it with `--build-arg`. That works and is honest, but it means a release depends
on somebody's laptop, which is the thing this table exists to make visible.

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

### What the AI apps added

The two desktop AI apps (`docs/packages.md`) are third-party downloads, so by
the rule above they belong in `core` — and they are the largest single addition
that tier has taken. Measured in a scratch `fedora-bootc:43` container:
**1.3 GB for `/usr/lib/chatgpt` and 548 MB for `/usr/lib/claude-desktop`**,
~1.9 GB of payload before compression.

This is a real tension, stated plainly rather than left to be discovered: the
product decision says an app version bump is an image rebuild, and a core
rebuild is a multi-gigabyte download for every machine on the fleet. The
consequence is that these apps' versions move when core moves, and not on the
vendors' own release cadence. Pushing them up a tier to make bumps cheaper is
not available — a `dnf` transaction above `core` puts an rpmdb-sized layer into
every user's next update, which is the problem this whole document is about.

### What the systemd-boot pivot added

The smallest thing `core` has taken, recorded because the pivot sounds
expensive and the packages are not what makes it so. Measured with
`dnf5 install --assumeno` inside `ghcr.io/andrenijman/apex-os:daily`:

```
Installing:  systemd-boot-unsigned  248.9 KiB
             systemd-ukify           99.9 KiB
Installing dependencies: python3-cffi, python3-cryptography, python3-pefile,
             python3-ply, python3-pycparser, python3-zstandard
Total size of inbound packages is 3 MiB. … 12 MiB extra will be used.
```

`checkpolicy`, `policycoreutils` and `python3-setools` were **already
installed** — they are named in the same transaction only to make the
dependency explicit, the way `efibootmgr` is, so a future change that drops
them fails the build instead of turning the boot blessing into a silent
rollback loop.

The `apex_sdboot` SELinux module is the other half. `semodule -N -i` grows
`/etc/selinux` by **466 bytes**, but rewrites `policy.35`, which is **3.8 MB**,
and an OCI layer carries a changed file whole. That is why the module is
compiled in `core` and not in the files tier: 4 MB once, rather than 4 MB in
every thin-tier update.

`systemd-boot-unsigned` has to be in the shipped image and cannot be a
build-only tool like `sbsigntools`: `bootc install --bootloader systemd` copies
the loader **out of the image being installed**. With the package absent, bootc
printed "Installing bootloader via systemd-boot", exited 0, and produced an ESP
with an empty `/EFI/systemd/` and no loader binary anywhere — an unbootable
disk from a successful install.

### What the screen reader added

The other end of the same scale, and worth recording next to the AI apps
precisely because it is the opposite case. P2-003's acceptance line names a
screen reader, and until `Containerfile.core`'s `5a-a11y` stanza there was none
in the image. Measured the same way — `dnf5 install --assumeno orca` inside
`ghcr.io/andrenijman/apex-os:daily`:

    Installing:            orca              21.3 MiB
    Installing dependencies:
                           brlapi            594.9 KiB
                           python3-brlapi    324.7 KiB
                           python3-louis      43.4 KiB
                           python3-pyatspi   414.5 KiB
    Total download 4 MiB · 23 MiB installed

**23 MiB against the AI apps' 1.9 GB**, in the same tier, for the component
without which a blind user cannot use the machine at all. Nothing in that
transaction pulls `speech-dispatcher` or `espeak-ng`, which is the image's own
rpmdb confirming they were already present as transitive dependencies of gtk4
and Qt — so the delta really is just the reader.

The rule this illustrates: the tier argument is about DOWNLOAD SIZE PER UPDATE,
not about whether a thing is worth shipping. A 23 MiB addition to core costs the
fleet nothing measurable; a 1.3 GB one is what makes the tension above worth
writing down.

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
