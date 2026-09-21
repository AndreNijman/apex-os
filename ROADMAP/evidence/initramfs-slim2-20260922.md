# initramfs-slim-2 — the first real image build, and what "reproducible" means

Unit: `initramfs-slim-2`, round 40, 2026-09-22 on the L16 (plus read-only
`ssh katana` and read-only registry inspection). Continues
`ROADMAP/evidence/initramfs-slim-20260921.md`, which measured everything in a
lab chroot and said so. This file is the part that could only be measured once
an image had actually been built.

Every number here is a measurement. Where something is not measured it says so.

## 1. The open risk the predecessor named is closed

`initramfs-slim` landed with one caveat stated rather than buried: *"no full
image build has run the new stanza end to end"*. One has.

**Build run `35624291221`**, `roadmap/v2.2` @ `44c9a5cb`, 2026-09-21 16:14Z,
1h43m40s, job `image` (id 106444907037) **succeeded**. From its log:

```
STEP 11/16: COPY --chmod=0755 files/scripts/check-initramfs-budget.sh  /tmp/check-initramfs-budget.sh
STEP 12/16: RUN set -eux; … dracut … ; /tmp/check-initramfs-budget.sh … ; rm -f /tmp/…
+ /tmp/check-initramfs-budget.sh --initrd /usr/lib/modules/7.2.6-cachyos1.apex1.fc43.x86_64/initramfs.img …
initramfs gates: 54 dracut modules, 3684 entries
initramfs-budget: vmlinuz 16.1 MiB + initramfs 84.8 MiB = 100.9 MiB per deployment (ceiling 130 MiB)
initramfs-budget: apex-boot-migrate needs 3 x 100.9 + 48 = 350 MiB of ESP
  ok    esp-budget … no-gpu-firmware … no-kms-driver … no-network-modules … no-networkmanager
  ok    net-driver-tree: 11 files / 1.2 MiB (ceiling 40 / 6.0 MiB)
  ok    simpledrm-built-in: all five =y
  ok    plymouth-engine … plymouth-theme
  ok    unlock-cryptsetup … unlock-tpm2-token … unlock-keymaps … unlock-hint
  ok    unlock-hint-wants … unlock-vconsole … unlock-vconsole-wants … unlock-crypt-module
initramfs gates: 17 passed, 0 failed
```

**17 of 17 gates passed in a real build**, including the eight `unlock-*` gates
the predecessor explicitly could not claim had ever run against a real
initramfs — its only sample predated `files/dracut/apex-unlock-hint/`, so the
suite tested them against a synthetic listing and said so. They are now tested
against the artifact.

Log kept at `/var/lab-scratch/initramfs-slim-2/imagejob-35624291221.log`.

## 2. The shipped numbers are better than the landed ones, because the kernel moved

The predecessor measured in a chroot on `7.2.5-cachyos1.fc43`. The build used
**`7.2.6-cachyos1.apex1.fc43.x86_64`** — the APEX kernel tier. Exact bytes read
off katana, which is booted on this exact image:

```
initramfs.img   88,935,408 B   84.81 MiB
vmlinuz         16,906,312 B   16.12 MiB
                ───────────
per deployment 105,841,720 B  100.94 MiB      need = 3 × 100.94 + 48 = 351 MiB
```

| | landed evidence (lab, 7.2.5) | shipped image (7.2.6-apex1) |
|---|---|---|
| initramfs | 103,256,987 B / 98.5 MiB | **88,935,408 B / 84.8 MiB** |
| per deployment | 114.6 MiB | **100.9 MiB** |
| `need` | 391 MiB | **351 MiB** |
| `=drivers/net` residue | 11 files / 1.5 MiB | 11 files / **1.2 MiB** |

Against `migrate-preconditions`' measured ceilings — katana 154 MiB, L16
180 MiB per deployment — the shipped image clears katana by **53 MiB** and the
L16 by **79 MiB**. Today's fat image is 374 MiB and clears neither.

## 3. It is no longer arithmetic: three deployments exist on a real machine

The landed evidence listed as a gap: *"No real 512 MiB ESP was filled. The
391 MiB figure is arithmetic over measured file sizes, not an observed
three-deployment ESP."*

katana, read-only over ssh. `bootc status` first, so the image is identified
rather than assumed:

```
booted.image.image:  ghcr.io/andrenijman/apex-os:apex-44c9a5cb6ba07b5892ee3b1e34ec77403e7b446a
booted.imageDigest:  sha256:06ba23c3a9d666780b01bb8774a56693cf66e68e49d35232a52223e756596112
booted.timestamp:    2026-09-21T17:42:03.782257946Z
uname -r:            7.2.6-cachyos1.apex1.fc43.x86_64
```

and then `du -sh` on the deployments that boot it:

```
101M  /boot/ostree/default-2e8ac7c8…    <- this build, slim
376M  /boot/ostree/default-ed5f1247…    <- previous, fat
376M  /boot/ostree/default-93133f9a…    <- previous, fat
```

**101 MiB observed against 376 MiB observed, on the same machine, in the same
directory.** Three slim deployments are 303 MiB; with the 48 MiB the migration
engine reserves that is 351 MiB against katana's own 512 MiB ESP
(`nvme1n1p2`). The engine measures FAT free space rather than partition size and
reports **503 MiB free against 350 MiB needed — 153 MiB spare** (§4). The same
three fat deployments are 1128 MiB, which is why katana was refused
`esp-too-small` until this image.

This is a `du` of the ostree deployment directories, not of an ESP: katana boots
GRUB/ostree from `/boot` on the root filesystem. The per-deployment directory
holds exactly what a Type #1 ESP layout would hold, so the figure transfers, but
no ESP was written and nothing on katana was modified.

## 4. katana can migrate — the first APEX machine that can

This is the end-to-end consequence, and it is the only measurement here that
was not predictable from the file sizes.

`apex-boot-migrate precheck` documents itself as writing nothing and mounting
the ESP `ro` (the mode is deliberate — a `rw` mount sets the dirty bit on the
partition the machine boots from). `status` was read first; nothing was staged
and no `stage`, `commit` or `auto` was run.

```
$ sudo apex-boot-migrate precheck --explain          # on katana, over ssh
STATUS  CHECK                REASON
OK      uefi                 booted through UEFI
OK      on-ostree            booted store is ostreeContainer, so there is something to migrate
OK      no-staged-update     no ostree deployment is staged for the next boot
OK      tools                mkfs.vfat, rsync and podman are all in this image
OK      bootc-new-enough     install to-existing-root has --composefs-backend
OK      secure-boot          Secure Boot is not enforcing, so an unsigned loader is accepted
OK      root-space           43 GiB free, 43 GiB needed
OK      esp-choice           bootc will write PARTUUID 99af3362-… of 1 ESP(s) on the root's disk
OK      esp-space            503 MiB free, 350 MiB needed
NOTE    esp-changes-disk     boots from PARTUUID 2ba9a2ea-…, will write 99af3362-…

This machine can migrate. "apex update" will do it, or run
"apex-boot-migrate auto" now.
rc=0
```

**Before this image the same machine, same partition, refused
`esp-too-small`: 503 MiB free against 1173 MiB needed.** The initramfs work is
the only thing that changed between those two runs. The engine's own
arithmetic — **350 MiB** — matches the build log's `3 × 100.9 + 48` to the
rounding, which is worth saying explicitly because the engine derives it from
the live deployment and the build log derives it from the files dracut wrote.

### Three other units' open items are closed by that one command

* **`sdboot-xbootldr` NEXT #6** — *"the cross-disk note is tested against both
  real `efibootmgr -v` spellings but has never fired in a guest."* It has now
  fired on real hardware: `NOTE esp-changes-disk`, with BootCurrent's PARTUUID
  `2ba9a2ea…` (the Windows disk katana boots from today) against
  `99af3362…` (APEX's own 512 MiB `EFI-SYSTEM` on the root's disk). It is a
  note and not a refusal, which is what that unit argued for.
* **`sdboot-xbootldr` NEXT #3** — *"`apex-os:daily` ships no `rsync` — the first
  refusal a real APEX machine gets today."* `OK tools: mkfs.vfat, rsync and
  podman are all in this image`. Fixed by this build, exactly as predicted.
* **`migrate-preconditions`** — katana's measured per-deployment ceiling of
  154 MiB is met with **53 MiB to spare**, and its §5 conclusion ("512 MiB
  still strands it, both wait on `initramfs-slim`") is discharged.

### One of those OKs has no margin, and a stranger should know which

`OK root-space: 43 GiB free, 43 GiB needed` is a **zero-margin pass**. katana's
root filesystem is 96% full — `df` says 907 G used of 954 G, 44 G available —
so the check clears by less than a gigabyte and will start refusing the moment
anything lands on that disk. The ESP check has 153 MiB of headroom; this one has
effectively none. Not a defect of this unit or of the engine, but
`sdboot-migrate-2`, which runs `stage` next on this machine, needs to know that
"every check OK" was true at 03:50 on 2026-09-22 and is not a durable property.

What this does **not** show: that migrating succeeds. Only that nothing refuses
it. `stage`, `commit`, the trial boot and `confirm` on real hardware remain
`sdboot-migrate-2`'s work, and deliberately nothing beyond `precheck` was run.

## 5. Cross-build reproducibility — the answer splits in two

This answers `sdboot-xbootldr`'s NEXT #2 ("does an APEX update ever leave the
initramfs byte-identical, so `find_vmlinuz_initrd_duplicate` fires and a second
deployment costs zero ESP?"). The predecessor measured run-to-run inside one
chroot; this is across independent builds.

### 5a. The content IS bit-reproducible

Two `podman build --no-cache --layers=false --isolation=chroot` runs, half a
minute apart, from the **same parent image**
(`ghcr.io/andrenijman/apex-os:core-68ca7094…`), running the exact dracut
invocation `Containerfile.apex` uses
(`/var/lab-scratch/initramfs-slim-2/repro/Containerfile`):

| | build a | build b |
|---|---|---|
| `initramfs.img` size | 88,934,458 B | 88,934,458 B |
| sha256 | `4569c3b0…22a7d881` | `4569c3b0…22a7d881` |
| `cmp` | — | **byte-identical** |

So `dracut --reproducible` holds across separate container builds, not only
across two runs in one chroot. **bootc's `find_vmlinuz_initrd_duplicate`
compares content, so it *can* fire.** The blocker is upstream of dracut.

### 5b. The layer digest is NOT reproducible — and a landed comment says it is

The same two builds:

| | build a | build b |
|---|---|---|
| final layer | `sha256:42760fdc…b285` | `sha256:a096af77…fb60` |
| `initramfs.img` mtime | 03:32:33 | 03:33:03 |

`Containerfile.apex` states, as landed:

> *"all this layer holds is a theme plus a `--reproducible` dracut run:
> unchanged theme + unchanged kernel yield a byte-identical initramfs and the
> layer digest stops moving."*

The first half is right (5a). **The second half is false.** The layer tar
records `initramfs.img`'s mtime, `dracut --reproducible` normalises timestamps
*inside* the cpio and not the file it writes, and there is no `--timestamp` and
no `SOURCE_DATE_EPOCH` anywhere in `.github/workflows/build-image.yml`. The
sentence is corrected in this branch.

**One flag closes it, demonstrated rather than suggested.** Two further
`--no-cache` builds with `podman build --timestamp 0`:

```
ts1  last layer sha256:0c81bf74…53ec   image id d920817c…57af
ts2  last layer sha256:0c81bf74…53ec   image id d920817c…57af
```

Identical layer *and* identical image id. This is recorded as a lever, **not**
adopted: `--timestamp` rewrites every mtime in the tier, and its interaction
with ostree, bootc and the `core`/`base` tiers is unmeasured. Anyone taking it
owes a real image build first.

### 5c. In practice it has never fired, and the registry proves it without a pull

14 published `apex-<sha>` tags, read with `skopeo inspect --raw` — manifests
only, no image pulled (cached in `/var/lab-scratch/initramfs-slim-2/manifests/`):

* the tier is built `--layers=false` (`build-image.yml:1339`), so the whole of
  `Containerfile.apex` is **one** layer;
* **14 of 14 final layer digests are distinct** — no APEX update has ever
  reused the previous apex-tier layer;
* **no two of them share a parent**, so content dedup never had the
  opportunity: 12 of the 14 match a published `base-<their own sha>` layer for
  layer, and the other two (`bd0c41ce`, `d12d3450`) match no published base tag
  at all — not one matches somebody else's. (`9e9ab146` is also the older
  12-layer design, before `--layers=false`, and is excluded from the
  one-layer statements above.)
* the 13 fat-era layers are not even the same compressed size as each other —
  358.1, 358.2, 359.0, 359.4, 359.5 MiB — so their *content* differed, not just
  their timestamps.

**The one-line answer for `sdboot-xbootldr` NEXT #2:** no APEX update has ever
left the initramfs byte-identical, but that is not dracut's fault — dracut is
already deterministic from an unchanged parent. The lever is *"stop rebuilding
`base` when nothing in it changed"*, not *"make the initramfs reproducible"*.
Until then `per_deployment × 3` stays the binding number, which is exactly why
the 100.9 MiB in §2 matters.

## 6. A 275 MiB-per-update download win that nobody had claimed

Same registry data, same zero pulls. The apex tier is one squashed layer whose
digest moves every build (5c), so **every APEX machine re-downloads all of it on
every update**:

| | compressed apex-tier layer |
|---|---|
| `apex-44c9a5cb` (slim, this build) | **84.5 MiB** |
| the 13 builds before it (fat) | **358.1 – 359.5 MiB** |

That is **~275 MiB off every single `bootc upgrade`**, independent of the ESP
argument, and it is the cheapest download win in `docs/update-cost.md`'s table —
the `image` tier is the one row that rebuilds "every run". Recorded there.

## 7. Found: `/root` is a dangling symlink, and dracut says FAILED then exits 0

The real build log carries, between `dracut` starting and the budget check
passing:

```
dracut-install: ERROR: installing '/root'
dracut[E]: FAILED: /usr/lib/dracut/dracut-install -D /var/tmp/dracut.dp78rrG/initramfs -f /root
```

and dracut still returned **0**, so `set -eux` did not stop the build.

Cause, read out of the image rather than guessed:

```
$ podman run --rm --entrypoint /bin/sh ghcr.io/…:core-68ca7094… -c 'ls -ld /root /var/roothome'
ls: cannot access '/var/roothome': No such file or directory
lrwxrwxrwx. 2 root root 12 Jan  1 1970 /root -> var/roothome
```

`/root` points into `/var`, which a bootc image ships empty. The predecessor's
lab chroot bind-mounted `/var`, which is exactly why it recorded `var/roothome`
as one of its "14 extra entries" and never saw this failure.

It is harmless — nothing in an initramfs needs `/root` — but the transferable
point is that **dracut's exit status does not cover this class of failure**. A
build that trusted `set -e` alone would ship a defective initramfs silently.
The budget predicate is what actually inspects the artifact, which is the whole
argument for having it.

## 8. The dGPU gap in the boot proof is closed, on real hardware

The predecessor's evidence lists, under what its boot proof does not cover:
*"No dGPU. A guest has no NVIDIA card, so the cosmetic cost this branch accepts
— firmware-resolution splash on a dGPU-attached panel, then a visible mode
change at handover — is unmeasured."* katana has an RTX 3070 and an Intel iGPU
and is booted on the shipped slim initramfs. Its own kernel log, read-only:

```
02:11:50  Console: colour dummy device 80x25
02:11:51  simple-framebuffer simple-framebuffer.0: [drm] Registered 1 planes with drm panic
02:11:51  [drm] Initialized simpledrm 1.0.0 for simple-framebuffer.0 on minor 0
02:11:51  simple-framebuffer simple-framebuffer.0: [drm] fb0: simpledrmdrmfb frame buffer device
02:11:51  fbcon: Deferring console take-over
02:11:55  [drm] [nvidia-drm] [GPU ID 0x00000100] Loading driver
02:11:55  fbcon: i915drmfb (fb0) is primary device
02:11:57  [drm] Initialized nvidia-drm 0.0.0 for 0000:01:00.0 on minor 2
02:11:57  nvidia 0000:01:00.0: [drm] fb1: nvidia-drmdrmfb frame buffer device
02:11:59  fbcon: Taking over console
02:11:59  Console: switching to colour frame buffer device 240x67
```

**simpledrm bound the framebuffer UEFI's GOP set up, one second into the boot,
with zero KMS drivers and zero firmware in the initramfs, on a machine with a
discrete NVIDIA GPU.** That is the same result the guest gave under a blacklist,
now on the hardware class the design is actually for.

The cost the branch accepted is measured rather than assumed too: **eight
seconds** at firmware resolution — simpledrm at +1 s, real KMS at +5 s, console
take-over at +9 s. It is the **iGPU** (`i915drmfb`) that claims `fb0` as primary,
not nvidia, which lands on `fb1`.

Incidentally confirms `Containerfile.core`'s FINDING 1: the live kernel command
line carries `rd.driver.blacklist=nouveau modprobe.blacklist=nouveau`, and no
nouveau line appears anywhere in the boot.

## What this file does not prove

* **Cross-host reproducibility is still untested.** 5a is two builds on one
  machine from one parent. A GitHub runner was not one of the two.
* **`--timestamp` was not put through an image build.** 5b's lever is measured
  on a local two-build pair only.
* **No ESP was written and no migration was run.** §3 is `du` on ostree deployment directories on a
  GRUB/ostree machine, not a filled 512 MiB FAT partition.
* **katana was read, never modified.** `bootc status`, `ls -l`, `du`, `df`,
  `lsblk`, `uname` only.
* **No new VM boot** — the two-boot proof in the predecessor's evidence stands
  and was not re-run against the image-built initramfs. §8 is a reading of
  katana's own boot, not a controlled experiment: nothing was blacklisted and
  nothing was varied.
* **`Containerfile.core`'s budget comment still quotes the lab figures** (98.5 /
  114.6 MiB, `392 MiB`). Correcting it was written and then deliberately
  reverted out of this branch: the `changes` job filters `core` on the *path*
  `Containerfile.core`, so a comment-only edit costs a 1h8m core rebuild and
  ~5 GB to every machine — exactly what `docs/update-cost.md` argues against.
  Fold it into the next commit that already forces a core rebuild.
