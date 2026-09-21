# initramfs-slim — "it has to work in 512"

Unit: `initramfs-slim`. Branch `task/initramfs-slim`. Measured 2026-09-21 on
the L16 and in a QEMU guest. Everything below is a measurement or is labelled
as not one.

## The answer, in one quotable line

> **A deployment is 114.6 MiB — vmlinuz 16,910,408 B + initramfs 103,256,987 B
> = 120,167,395 B — so `apex-boot-migrate` needs `3 x 114.6 + 48 = 391 MiB`
> and a 512 MiB ESP fits, with about 120 MiB to spare.**

For comparison, the image as it ships today is 16,910,408 + 376,456,492 =
**375.2 MiB per deployment**, needing **1173 MiB**, which is why every 512 MiB
ESP is refused `esp-too-small` right now. `migrate-preconditions` should cite
the line above rather than re-deriving it.

The initramfs was **96%** of the problem: 359.0 MiB of the 375.2.

## Where the megabytes went — the attribution matrix

Five variants, all built in the **same** chroot — a real bootc-installed APEX
deployment on a lab disk, kernel `7.2.5-cachyos1.fc43.x86_64` — with the exact
dracut flags `Containerfile.apex:244` uses. Same kernel, same package set, one
variable at a time, so the deltas are attributable.

| | what it adds | bytes | MiB | this step bought |
|---|---|---|---|---|
| v0 | as shipped (`add_drivers+=" nvidia … "`) | 376,595,424 | 359.1 | — |
| v1 | minus `add_drivers` — RPMFusion's own default | 291,208,822 | 277.7 | **−81.4 MiB** |
| v2 | + omit nouveau, amdgpu, i915, xe, radeon, amdkfd, nvidia\* | 138,978,697 | 132.5 | **−145.2 MiB** |
| v3 | + omit network/network-manager/kernel-network-modules/nfs/nvmf | 103,256,987 | 98.5 | **−34.0 MiB** |
| v4 | v3 with `compress="zstd -19"` in dracut.conf.d | 103,256,987 | 98.5 | **0.0 MiB** |

**v3 is what the branch ships.**

Two things this table settles that arithmetic alone would not have:

* **Both omissions are load-bearing.** v2 — dropping every big KMS driver but
  keeping the network stack — is **132.5 MiB**, which does *not* meet the
  130 MiB per-deployment ceiling. Removing the GPU drivers is not sufficient
  on its own.
* **The biggest single win is deleting a line, not adding one.** v0→v1 is
  −81.4 MiB and consists entirely of *stopping* `add_drivers+=" nvidia … "`.
  dracut installs the firmware of every driver it installs, and the nvidia GSP
  blobs are enormous: `gsp_ga10x.bin` alone is 75,028,464 B.

## Finding: `compress=` in dracut.conf.d is dead code in this build

v3 and v4 are the **same file** — sha256
`b6ca6196d1d1b2a886f682f644a52510c35b9dc0bf878a4160430e7c21c63946` for both,
`cmp` clean — and both record `--zstd` in their own dracut `Arguments:` line.
`Containerfile.apex:244` passes `--zstd` on the **command line**, which
overrides any `compress=` in `dracut.conf.d`. So compression cannot be tuned
from that file; an entry there is a setting that looks effective and is not.
Adopting `-19` would mean changing the Containerfile's dracut flags, and it is
worth roughly 3% — not worth spending against 98.5 MiB.

## The defect this round existed to fix

`Containerfile.apex` shipped, as its "the initramfs has no network" gate:

```
! grep -qE 'kernel/drivers/net/ethernet/'   ->   FATAL
```

The slim initramfs this branch produces **contains eleven files** under
`kernel/drivers/net`. The gate would have FATALed **every image build**, on the
exact artifact it was written to pass — the "Containerfile assertions that
cannot pass" family, which this repo has already paid five days of image builds
for.

The eleven arrive by **three unrelated routes**, and not one of them is the
network stack (the card's earlier "SCSI offload deps" covered only 2 of 11):

| module | pulled in by | why it is there |
|---|---|---|
| `cnic` | `scsi/bnx2fc` | FCoE offload |
| `qed` | `scsi/qedf` | FCoE offload |
| `cxgb4` | `crypto/chelsio/chcr` | a **crypto accelerator**, not storage |
| `libertas`, `libertas_sdio`, `mt76`, `mt76-sdio`, `mt76-connac-lib`, `mt792x-lib`, `mt7921-common`, `mt7921s` | the MMC/**SDIO** path | the root could be on an SD card |

They are dependencies of modules that are in the initramfs for storage and
crypto reasons, so **no `omit_dracutmodules` setting removes them**. "No path
contains the word ethernet" was never the property anyone meant.

### What replaced it

`files/scripts/check-initramfs-budget.sh` — the same gates as a predicate that
runs **without a 40-minute image build**. `--initrd IMG` runs `lsinitrd` (what
the Containerfile uses); `--list FILE` reads a saved listing (what CI uses,
because a GitHub runner has no `lsinitrd`, no initramfs and no kernel to make
one from). The network property is now the three things actually meant:

* the `network`, `network-manager`, `kernel-network-modules`, `nfs`, `nvmf`
  dracut modules are absent;
* `usr/bin/NetworkManager` is absent;
* the `=drivers/net` tree has not come back — a **ceiling**, not an absence:
  **≤ 40 files / 6 MiB**, against a measured baseline of **322 files /
  21.9 MiB** and a measured residue of **11 files / 1.5 MiB**.

`tests/test-apex-initramfs-budget.sh` runs it against two checked-in listings —
a fat one and a slim one — and asserts **every gate's verdict by name in both
directions**. 35 assertions. A gate that cannot be shown red is not a gate.

### And the same defect family, inside the fix

The first draft of `check-initramfs-budget.sh` held the listing in a shell
variable and asked `printf '%s\n' "$FILES" | grep -qE …`. `grep -q` exits the
instant it matches, the writer takes SIGPIPE, and `set -o pipefail` makes 141
the pipeline's status — **so a match read as a miss.** Position-dependent,
which is what makes it vicious: measured here, a match at listing line 280
returned **141** while one near the end returned **0**, so seven present files
were reported absent and two equally present ones passed. The predicates now
grep a real file, which cannot take SIGPIPE.

## Finding: the branch's own policy statement is inaccurate

`Containerfile.core` stage 1f states the policy as **"NO KMS DRIVER IS IN THE
INITRAMFS"**. Measured against v3, that is false. The `omit_drivers` list
covers `nouveau amdgpu i915 xe radeon amdkfd nvidia*`, and these remain:

```
drivers/gpu/drm/tiny/bochs.ko.zst
drivers/gpu/drm/tiny/cirrus-qemu.ko.zst
drivers/gpu/drm/virtio/virtio-gpu.ko.zst
drivers/gpu/drm/vmwgfx/vmwgfx.ko.zst
drivers/gpu/drm/ast/ast.ko.zst
```

This is **not a size problem** — they are tiny and carry no firmware — and it
is **not a correctness problem**, for the same reason: the hazard the policy
guards against is a KMS driver that evicts `simpledrm` during probe and then
has no firmware to bring the display back. These five need none. But the
accurate statement is *"no firmware-bearing KMS driver is in the initramfs"*,
and it matters because it is exactly what makes a QEMU guest a poor proxy for
the L16 unless you know to blacklist `bochs` — see the boot proof below.

## Reproducibility

**Run-to-run: byte-identical.** v3 built twice in the same chroot produced
`b6ca6196…3946` both times, `cmp` clean. `--reproducible` holds.

**Lab chroot vs the real image build: faithful in content, not bit-identical.**
Comparing lab-built v0 against the initramfs the image build actually shipped
on this disk:

* the dracut **module list is identical**;
* **zero** paths present in the shipped image are missing from the lab one;
* the lab one has **14 extra entries** and is **138,932 B larger (0.037%)**:
  `dev/{console,kmsg,null,random,urandom}`, `var/roothome`,
  `var/lib/nfs/statd*`, `usr/etc/*`, and one authselect symlink — every one of
  them an artefact of the bind mounts that make the chroot runnable at all.

So the lab predicts image **content** exactly and image **bytes** to 0.04%.

**What this does not prove:** cross-**host** bit-reproducibility. Everything
here is one machine. `--reproducible` normalises timestamps; it does not pin
the package set, and a different base image would produce a different
initramfs that is still perfectly "reproducible" on its own host.

## The boot proof

The size win is only worth having if the machine still gets a console. Two
headless QEMU boots of the **same disk** — a reflink copy of a real
bootc-installed APEX disk with the **slim (98.5 MiB) initramfs** installed over
the shipped one, `-vga std` so the guest has a real UEFI GOP framebuffer, a
dracut `pre-pivot` hook reporting from **inside the initramfs** to `/dev/kmsg`.

| | boot 1 — guest defaults | boot 2 — every KMS driver blacklisted |
|---|---|---|
| `card0` driver | `bochs-drm` | **`simple-framebuffer`** |
| framebuffer console | `fb0 fbcon` | `fb0 fbcon` |
| reached switch-root | yes | yes |
| network interfaces in initrd | `lo` only | `lo` only |
| NetworkManager in initrd | absent | absent |
| `systemd-cryptsetup` in initrd | present | present |
| full boot | `graphical.target`, 37.0 s | `graphical.target`, **7.714 s** |

**Boot 1 is the trap, and it is why boot 2 exists.** `bochs` is still in the
initramfs (see the finding above), so on `-vga std` it binds `card0` and
evicts `simpledrm`. A guest run without the blacklist therefore measures the
*old* design and would have "passed" while proving nothing about the claim.

**Boot 2 is the claim.** With `rd.driver.blacklist=nouveau,bochs,cirrus,
virtio_gpu,vmwgfx,ast`, no KMS driver can load at all — the L16-equivalent
condition — and the probe from inside the initramfs reports
`card0 driver=simple-framebuffer` with `fb0 fbcon` present. **simpledrm bound
the framebuffer UEFI's GOP set up, with zero KMS modules and zero firmware in
the initrd, and the machine booted through to `graphical.target` in 7.7 s.**

That is the whole safety argument for removing the drivers, measured rather
than reasoned. It is consistent with this L16's own boot log
(`[drm] Initialized simpledrm 1.0.0 for simple-framebuffer.0 on minor 0`) and
with the kernel contract asserted in `Containerfile.kernel`: `CONFIG_DRM`,
`CONFIG_DRM_SIMPLEDRM`, `CONFIG_SYSFB_SIMPLEFB`, `CONFIG_DRM_FBDEV_EMULATION`,
`CONFIG_FRAMEBUFFER_CONSOLE` all `=y`.

Serial logs: `/var/lab-scratch/initramfs-slim/serial-1.txt`, `serial-2.txt`.

### What the boot proof does not cover

* **No LUKS.** The lab disk has a plain btrfs root, so "the passphrase prompt
  is drawn and accepted on simpledrm" is **not** measured here. The console
  that would draw it is measured; the prompt itself is not.
* **No dGPU.** A guest has no NVIDIA card, so the cosmetic cost this branch
  accepts — firmware-resolution splash on a dGPU-attached panel, then a
  visible mode change at handover — is unmeasured.
* **No real 512 MiB ESP was filled.** The 391 MiB figure is arithmetic over
  measured file sizes, not an observed three-deployment ESP.
* The guest boots via **GRUB**, not systemd-boot, so this says nothing about
  the systemd-boot migration itself.

## What it costs, stated plainly

* **Network-root installs of any kind** — NFS, NVMe-oF, iSCSI, netboot — cannot
  work with this initramfs. Nothing in this repo sets `rd.neednet`, `netroot`,
  `nfsroot` or `iscsi`, and `installer/build-live-iso.sh` already made the same
  call for the live ISO.
* **Clevis/Tang network unlock** drops out with the network module.
  `clevis-pin-tpm2`, which `50-bootc-clevis.conf` and APEX's own enrolment
  actually use, stays. Note that `dracut` now logs
  `Module 'clevis-pin-tang' depends on module 'network', which can't be
  installed` on every build — expected, `rc=0`, and better than silent.
* On a dGPU-attached panel the splash and any passphrase prompt run at firmware
  resolution.

## Landability

**Not landable on its own yet.** The gate defect is fixed and proven both ways,
but no image build has run the new stanza end to end — the boot proof used a
lab-built initramfs installed onto an existing disk, not an image the build
produced. See the agent card for the current state.
