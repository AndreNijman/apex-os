# systemd-boot pivot — what the VM lab actually measured, 2026-09-20

Unit `sdboot-image`. Every row here is a command that ran on the L16 into a
loopback disk image or a qemu guest. Nothing touched a real boot path: the
host's `efibootmgr -v` was captured before the first install and compared with
`cmp` after the last one, and it is byte-identical
(`01858462d0777d3ac789e81cd6e6e59393ee8e45de03c142b15c1e761218c50e` both times).

Lab directory: `/var/lab-scratch/sdboot-lab` (real disk; the first runs were on
`/tmp`, which is a 15 GB tmpfs on this machine — 20 GiB disk images in RAM, and
it cost the machine its memory headroom before it was caught).

Versions under test: `bootc 1.16.10` (the booted APEX image), `bootc 1.16.13`
(`quay.io/fedora/fedora-bootc:43`), `systemd 258.10-1.fc43`, `ostree 2026.2`,
`bootupd 0.2.35`.

## 1. The ostree backend cannot use systemd-boot. Measured, not inferred.

```
bootc install to-disk --via-loopback --generic-image --wipe \
    --filesystem ext4 --bootloader systemd  … /work/fbootc-sdboot.img
…
Bootloader: systemd
error: Installing to disk: bootupd is required for ostree-based installs
```

`bootupd` **is** installed in that image (`bootupd-0.2.35-1.fc43.x86_64`), so
this is not a missing package. bootupd only ships a grub2+shim payload
(`/usr/lib/bootupd/updates/{EFI,BIOS}.json`), so on the ostree backend
`--bootloader systemd` has nothing to install and bootc refuses rather than
producing an unbootable disk.

This is the answer to "does GRUB have to stay": **on the ostree backend, yes.**
The pivot is a storage-backend change, not a bootloader flag.

Two structural reasons behind that refusal, both visible on this machine:

* ostree's BLS entries live at `/boot/loader/entries/ostree-N.conf` and point at
  `/boot/ostree/<deployment>/vmlinuz`. On the L16 `/boot` is a **btrfs**
  subdirectory of the root filesystem (`/dev/nvme0n1p5[/boot]`, per
  `/proc/self/mountinfo`). systemd-boot reads only what UEFI's simple-file-system
  protocol reads — FAT. It cannot see those entries or those kernels at all.
  (The disk also carries two *unmounted* ext4 XBOOTLDR partitions, p2 and p4
  labelled `apex-newboot`; ext4 is equally unreadable to sd-boot.)
* The ostree cmdline carries `ostree=/ostree/boot.0/default/<sha>/1` and
  `root=UUID=<per-machine>`. Both are per-deployment and per-machine, so a UKI
  signed in CI could never carry the right one.

## 2. The composefs backend with systemd-boot installs, boots, and updates.

```
bootc install to-disk --via-loopback --generic-image --wipe \
    --filesystem ext4 --bootloader systemd --composefs-backend … 
…
Bootloader: systemd
Installing bootloader via systemd-boot
Installation complete!
```

Partition table bootc creates itself (same for both backends except ESP size):

| partition | ostree backend | composefs backend |
| --- | --- | --- |
| 1 — BIOS boot | **1 MiB, created** | **1 MiB, created** |
| 2 — EFI System | 512 MiB | **1 GiB** |
| 3 — root | rest | rest |

The 1 MiB BIOS boot partition is created by `bootc install to-disk` on both
paths, on a UEFI host, with nothing installed into it — see §6.

### It boots

`localhost/sdtest:base` = `fedora-bootc:43` + `systemd-boot-unsigned` +
`systemd-ukify`, installed as above with `--karg console=ttyS0,115200n8`, booted
under `OVMF_CODE_4M` (non-Secure-Boot) on qemu/KVM: reached
`fedora login:` on ttyS0 with NetworkManager, resolved, chronyd and
`serial-getty@ttyS0` all active. Full serial log:
`/var/lab-scratch/sdboot-lab/sdtest-cfs.serial`.

### systemd-boot-unsigned is required, and its absence fails SILENTLY

The first composefs install used stock `fedora-bootc:43`, which does **not**
carry `systemd-boot-unsigned`. bootc printed `Installing bootloader via
systemd-boot` and exited 0, and the resulting ESP was:

```
/EFI/BOOT            <- empty directory
/EFI/systemd         <- empty directory
/EFI/Linux/bootc_composefs-<verity>/{vmlinuz,initrd}
/loader/entries/bootc_fedora-43-1.conf
```

No loader binary anywhere. The disk is unbootable and the install reported
success. **Any APEX build that selects `--bootloader systemd` must assert
`/usr/lib/systemd/boot/efi/systemd-bootx64.efi` exists in the image**, because
bootc will not.

### The ESP layout it produces

```
/EFI/BOOT/BOOTX64.EFI                          <- removable-media fallback
/EFI/systemd/systemd-bootx64.efi               <- the loader
/EFI/Linux/bootc_composefs-<128-hex verity>/vmlinuz
/EFI/Linux/bootc_composefs-<128-hex verity>/initrd
/loader/entries/bootc_<osid>-<ver>-<N>.conf    <- Type #1 BLS, N=0 is oldest
/loader/entries.srel                           <- "type1"
/loader/loader.conf                            <- "#timeout 3" + "#console-mode keep", both COMMENTED
/loader/keys/                                  <- empty; where SB keys would go
```

Entry contents:

```
title Fedora Linux 43 (Forty Three)
version 43
linux /EFI/Linux/bootc_composefs-<verity>/vmlinuz
initrd /EFI/Linux/bootc_composefs-<verity>/initrd
options rw <kargs> composefs=<verity>
sort-key bootc-fedora-<index>
```

`bootType: Bls`, `bootloader: systemd` in `bootc status`. These are
**unsealed** entries — loose kernel + initrd, not a UKI. Upstream's contract:
a UKI in the image makes the install *sealed* (the composefs digest is inside
the signed PE); a vmlinuz/initramfs layout is always unsealed and may use
either bootupd/GRUB or systemd-boot.

The ESP is mounted at `/boot` on a composefs machine (`boot.automount - EFI
System Partition Automount`), so `/boot/loader/entries` **is** the ESP's.

## 3. `bootc upgrade` produces a correct new entry. This was the load-bearing question.

Two full install → boot → `bootc upgrade` → reboot cycles, against a local
registry the guest reached at `10.0.2.2:5055`.

**Run A — v1 → v2, identical kernel and initramfs.** Booted v1, upgraded,
rebooted, booted **v2**. Afterwards both entries pointed at the *same*
`/EFI/Linux/bootc_composefs-ca2d6a35…/` directory and only one such directory
existed. `bootDigest` was identical (`87a37e3a…`) for both deployments.

**Run B — v1 → v3, initramfs deliberately changed** (a dracut
`install_items` snippet, so `initramfs.img` sha256 moves
`f8ec50e2…` → `7143a612…` while `vmlinuz` stays `bc028ef5…`). Booted v1,
upgraded, rebooted, booted **v3**, and the ESP then held **two** directories:

| directory | vmlinuz sha256 | initrd sha256 | entry |
| --- | --- | --- | --- |
| `bootc_composefs-ca2d6a35…` | `bc028ef5…` | `f8ec50e2…` | `bootc_fedora-43-0.conf` (rollback) |
| `bootc_composefs-7a4641aa…` | `bc028ef5…` | `7143a612…` | `bootc_fedora-43-1.conf` (booted) |

So run A was **deduplication by boot content**, not a stale pointer: when the
kernel and initramfs differ, a second directory is written and the new entry
points at it. A kernel update produces a correct entry.

Staging is atomic and visible: during `bootc upgrade` the ESP grows
`/boot/loader/entries.staged/` holding *both* the new and old `.conf` files,
and `bootc-finalize-staged.service` swaps the directory at shutdown. The
previous deployment survives as `rollback` in `bootc status`.

`softRebootCapable: true` on the staged deployment — the composefs backend
supports `bootc upgrade --soft-reboot`.

## 4. Boot counting: bootc writes none, `bootc upgrade` destroys one, and APEX can put it back

Four installs and two upgrades produced entry filenames of the form
`bootc_<osid>-<ver>-<N>.conf` with **no `+N-M` counter**, and a `loader.conf`
with `timeout` and `console-mode` both commented out.

So on a machine installed exactly as bootc leaves it,
`/sys/firmware/efi/efivars/LoaderBootCountPath-*` never appears,
`apex-boot-health.service` and `apex-boot-notice.service` are both skipped by
their `ConditionPathExists`, `systemd-bless-boot` never runs, and there is no
automatic rollback. The health gate `Containerfile.base` asserts so carefully is
**inert on this path** until something writes the counter into the filename.

### The counter works, and `bootc upgrade` wipes it (`count.img`, 4 boots)

Renaming the booted entry by hand:

| boot | action | entries in the ESP | `LoaderBootCountPath` |
| --- | --- | --- | --- |
| 1 | `mv bootc_fedora-43-1.conf bootc_fedora-43-1+3-0.conf`; mask `systemd-bless-boot` | `…+3-0.conf` | absent |
| 2 | sd-boot decremented before the kernel started | `…+2-1.conf` | **present**, naming `\loader\entries\bootc_fedora-43-1+2-1.conf` |
| 2 | then `bootc upgrade` | `entries/`: `…-1+2-1.conf` · `entries.staged/`: `…-0.conf` `…-1.conf` | — |
| 3 | after the reboot | `…-0.conf` `…-1.conf` — **counter gone** | absent |
| 4 | nothing restores it | same | absent |

`bootc upgrade` does not edit the live entry; it writes a **fresh, uncounted
set** into `/boot/loader/entries.staged/`, and `bootc-finalize-staged.service`
replaces the whole `entries/` directory at shutdown. Anything APEX wrote into a
filename before the upgrade is discarded with the old directory.

### Renaming inside `entries.staged/` survives the swap (`count2.img`, 5 boots)

Same machine, but the rename happens on the *staged* entry, immediately after
`bootc upgrade` returns and before the reboot:

```
LAB-STAGED-BEFORE: bootc_fedora-43-0.conf bootc_fedora-43-1.conf
mv  …/entries.staged/bootc_fedora-43-1.conf  …/entries.staged/bootc_fedora-43-1+3-0.conf
LAB-STAGED-AFTER:  bootc_fedora-43-0.conf bootc_fedora-43-1+3-0.conf
```

| boot | booted | entry in the ESP | `LoaderBootCountPath` |
| --- | --- | --- | --- |
| 2 | **d2** (the upgrade) | `bootc_fedora-43-1+2-1.conf` | present |
| 3 | d2 | `bootc_fedora-43-1+1-2.conf` | present |
| 4 | d2 | `bootc_fedora-43-1+0-3.conf` | present |
| 5 | **d1** (the previous deployment) | `bootc_fedora-43-0.conf` selected; `…-1+0-3.conf` still there, skipped | absent |

Both halves hold. The counter survives `bootc-finalize-staged`, systemd-boot
decrements it before each kernel start, and at `+0-3` **it rolled back to the
previous deployment by itself** — with `systemd-bless-boot` masked throughout,
so nothing blessed the new entry and the rollback is the counter's doing.

So the mechanism APEX needs is one unit that renames the newest file in
`/boot/loader/entries.staged/` to carry `+3-0`, ordered before
`bootc-finalize-staged.service`. It is a rename in a directory bootc is about to
swap in, not a write to a live boot path.

**Not observed here: the blessing.** `systemd-bless-boot` was unmasked at boot 3
and the `+0-3` suffix was still present at boots 4 and 5. `boot-complete.target`
is not reached on a stock `fedora-bootc` guest, and this lab image has none of
APEX's `apex-boot-health.service` / `boot-complete.target.requires` wiring. The
strip has to be measured on the APEX image, and until it is, the honest
statement is that **rollback is proven and blessing is not**.

## 5. ESP capacity is the hard constraint for existing machines

Measured on the L16's booted APEX deployment:

```
16898120   /usr/lib/modules/7.2.3-cachyos2.fc43.x86_64/vmlinuz
375558646  /usr/lib/modules/7.2.3-cachyos2.fc43.x86_64/initramfs.img
--------
392456766  bytes = 374 MiB per deployment, on the ESP
```

bootc keeps the booted and the rollback deployment, so a steady-state APEX
machine needs **~749 MiB of ESP** for two deployments, plus a third
transiently while an update is staged — call it **1.1 GiB**.

* bootc's own composefs default ESP is **1 GiB**. That is under the transient
  peak.
* **The L16's existing ESP is 600 MiB.** It cannot hold even two APEX
  deployments. An ESP cannot be grown in place without moving the partition
  that follows it.

This is not a tuning detail; it is the reason in-place migration of an existing
APEX machine is not a matter of writing a loader.

## 6. Legacy BIOS: what APEX supports today

* `bootc install to-disk` creates a **1 MiB BIOS boot partition** on every
  install, UEFI or not — observed on both backends above.
* `installer/apex-install` uses `bootc install to-disk`/`to-filesystem`, so
  every APEX disk install gets that partition.
* bootupd ships a BIOS payload: `/usr/lib/bootupd/updates/BIOS.json`
  (`grub2-tools-1:2.12-43.fc43`) alongside `EFI.json`.
* **But the L16's `/boot/bootupd-state.json` records `installed` containing only
  the `EFI` component** — no `BIOS` key. So the partition exists and nothing was
  written into it on this machine.
* The **live ISO** genuinely boots legacy BIOS: `installer/build-live-iso.sh`
  builds an El Torito core image plus an isohybrid MBR, with a VESA `vga=791`
  handoff specifically for BIOS.

So the honest statement is: **APEX's installer media boots on legacy BIOS; the
installed system on this machine does not have a BIOS bootloader written.**
Whether any APEX machine has ever booted BIOS from disk is not established by
anything in the repo or on this machine.

## 7. What has NOT been measured yet

* Secure Boot. Every boot above was on non-Secure-Boot OVMF. No sd-boot binary
  was signed and nothing was enrolled.
* A UKI on this path (sealed mode). `bootc container split-kernel-and-rootfs`
  and `bootc container ukify` exist in bootc 1.16.13 and neither was run.
* The APEX image itself. Every install above used `fedora-bootc:43` or a
  three-line derivative. APEX's 15 GB image has not been through this.
* Any migration of an existing machine.
* The blessing half of boot counting, on an image whose `boot-complete.target`
  is actually reached.
* XBOOTLDR. Ruled out by probe rather than by test: `strings` on `bootc` has
  **zero** occurrences of `xbootldr` or the XBOOTLDR type GUID `bc13c2ff…`, and
  `bootc --help` on 1.16.13 lists no migration verb. The L16's two unused 2 GiB
  XBOOTLDR partitions cannot absorb the ESP requirement, and there is no
  in-place ostree→composefs converter.
