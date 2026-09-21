# sdboot-xbootldr — can Type #1 entries live on an XBOOTLDR partition?

items: none (no task id; dispatched as a design question)
repo: apex-os
worktree: /var/tmp/apex-work/wt-sdboot-xbootldr
branch: task/sdboot-xbootldr, cut from roadmap/v2.2 @ 76aa2b95
lab: /var/lab-scratch/sdboot-xbootldr
evidence: ROADMAP/evidence/sdboot-xbootldr-20260921.md

## THE ANSWER, IN ONE LINE

**No. bootc's composefs backend writes the kernel and initramfs to the ESP
unconditionally and has never implemented XBOOTLDR.** The idea is sound —
systemd-boot reads that layout perfectly well — but sd-boot is not the actor.

## What was established, and how

Read the source, not the binary. `strings` was how the previous round reached
the same conclusion and it is not evidence: the XBOOTLDR GUID const *is* in
bootc's source (`discoverable_partition_specification.rs:482`) and is absent
from the binary only because nothing reachable references it.

Four revisions checked, because the one that matters is not the host's:
**1.16.4** (inside `apex-os:daily`, the bootc that actually runs the
migration), 1.16.10 (L16 host), 1.16.13 (sdboot-migrate's guest), and upstream
`main` @ `65321ae`. **Identical in all four.**

* `spec.rs:288` — `--bootloader systemd` maps to `BootloaderKind::BLSCompatible`.
* `bootc_composefs/boot.rs:751` vs `:775` — the two arms of
  `setup_composefs_bls_boot` are **not symmetric**. `GRUBClassic` asks
  `root.is_mountpoint("boot")`; `BLSCompatible` never asks, calls
  `mount_esp_writable()` and sets `abs_entries_path = /EFI/Linux`.
* `boot.rs:1422` — literal `// TODO: support XBOOTLDR … bootc does not yet
  detect or use XBOOTLDR in the composefs install path`.
* `bootloader.rs:287` — `// If we supported XBOOTLDR in the future, that'd go
  here with --boot-path.`
* `store/mod.rs:396` — `// NOTE: Handle XBOOTLDR partitions here if and when we
  use it`, in the **runtime** store. This kills the obvious workaround: let
  bootc write to the ESP, then move the files. `boot_dir` is the ESP for every
  later `bootc upgrade`, `status`, `rollback` and `image delete`, so a
  relocation breaks the first steady-state update and would have to be redone
  forever. That is a fork of bootc, not a migration option.
* bootc PR **#2440** (merged 2026-09-16): *"GrubCC and SystemdBoot both do not
  store anything inside of /sysroot/boot and will (should) always have the ESP
  mounted at /boot."* The separate-`/boot` support that does exist upstream was
  added for GRUB and deliberately not for systemd-boot.

Sources kept at `/var/lab-scratch/sdboot-xbootldr/bootc-src` (v1.16.10) and
`bootc-main` (main; v1.16.4 also fetched into it).

## The premise was wrong twice, and that matters more than the answer

1. **APEX is already on Type #1.** `BootType::Bls` is `#[default]` and an image
   with a plain kernel under `/usr/lib/modules` gets it. `docs/boot-v2.md`
   already calls it *phase 1*. "Switch to Type #1" is not an available change.
2. **The 1.1 GiB is three deployments, not two UKIs.** `apex-boot-migrate`
   computes `per * 3 + 48 MiB` where `per` is `vmlinuz + initramfs.img`.
   Measured on the L16: 392 456 766 B = 374 MiB, so `need` = 1170 MiB.

Measured on the APEX composefs guest `sdboot-image` left at
`/var/lab-scratch/sdboot-lab/apex.img` — the real image, **btrfs** root —
mounted read-only:

```
ESP 1022 MiB, 376 MiB used (37%) for ONE deployment
  EFI/systemd/systemd-bootx64.efi      136 KiB
  EFI/BOOT/BOOTX64.EFI                 136 KiB
  loader/                               28 KiB   (entries.srel = "type1")
  EFI/Linux/bootc_composefs-085d…/vmlinuz   16 922 688 B
  EFI/Linux/bootc_composefs-085d…/initrd   376 918 577 B
loader/entries/bootc_fedora-43-1+2-1.conf:
  linux  /EFI/Linux/bootc_composefs-085d…/vmlinuz
  initrd /EFI/Linux/bootc_composefs-085d…/initrd
```

**The bootloader is 300 KiB of the 376 MiB.** The orchestrator's arithmetic was
right and irrelevant: bootc puts the other 375.6 MiB beside it.

`CFS_EFIPN_SIZE_MB = 1024` in `install/baseline.rs`, comment *"we have UKIs and
UKI addons"*, not settable by any flag — upstream's own default is **below**
APEX's 1170 MiB peak. And bootc's one self-created "XBOOTLDR" partition
(`requires_bootpart()`, tpm2-luks only, 510 MiB) is written with **no `type=`**,
so it is not even XBOOTLDR-typed.

## Secure Boot: the delta is zero

ESP vs XBOOTLDR is the same bytes on a different partition. The real delta is
**Type #1 vs UKI** (initrd outside the signature, no `.pcrsig`, no PCR 11) and
APEX already pays it in phase 1. `secure-boot-unsigned-loader` is about
`systemd-bootx64.efi` itself and XBOOTLDR never touched it. PCR 11 is unusable
on a shipped APEX machine anyway (`luks-enroll`).

## What landed

* `9b91fa6e` — `ROADMAP/evidence/sdboot-xbootldr-20260921.md`, the full
  transcript.
* `bc55e032` — `esp-too-small` now names the XBOOTLDR partition and says why it
  cannot help (`find_xbootldr()`, by GPT type GUID, same shape as `find_esp`);
  `docs/boot-v2.md` gains "XBOOTLDR: the Boot Loader Specification allows it,
  bootc does not implement it" and loses the `strings`-absence argument; the
  stale "migrating means backing up and reinstalling" section is marked
  superseded. `test-boot-migrate` 61 → **65**, four mutations checked
  (wrong GUID, note dropped, finder never matches, finder matches everything).

Both pushed. Shellcheck clean. **Not landed** — do not merge this yourself.

## VERDICTS

**Would the L16 still refuse?** Yes, both reasons, and XBOOTLDR removes
neither. `secure-boot-unsigned-loader` fires first (Secure Boot is on, sd-boot
is unsigned); `esp-too-small` second (600 MiB against 1170). `p2` and `p4` stay
exactly as useless to this path as they are now. The two things that would move
it, neither this unit's: **shrink the 359 MiB initramfs** (`kernel-build` —
every MiB is worth three on the ESP), or **delete the dead `p2` and grow `p1`**
(Andre's call on his own machine; `p2` sits immediately after the ESP so the
growth is contiguous).

**A Windows dual-boot machine?** Nothing changes. `windows-installer` measured
a stock shared Windows ESP at 96.0 MiB total / 27.7 MiB used / **68.3 MiB
free**. sd-boot's 136 KiB fits; 374 MiB per deployment does not, and it cannot
be diverted to an XBOOTLDR the installer creates. The "second ESP or stay on
GRUB" fork stands.

## NEXT — for a stranger

1. **Do not reopen the XBOOTLDR question without new upstream code.** If it ever
   moves, the six sites are `boot.rs:775`, `boot.rs:1422`, `bootloader.rs:287`,
   `store/mod.rs:396`, `status.rs:408`, `finalize.rs:149`/`delete.rs:159` — all
   keyed off the same `boot_dir` decision. Watch bootc for a PR touching
   `esp_subdir`; do **not** carry it as an APEX fork.
2. **`apex-os:daily` has no `rsync`.** `command -v rsync` is empty in the
   shipped image, so `apex-boot-migrate precheck` refuses `no-rsync` before it
   reaches the ESP arithmetic. `Containerfile.base` on `roadmap/v2.2` asserts
   rsync is present, so the next image build fixes it — but until one exists,
   every precheck on a real APEX machine stops at `no-rsync`, and that is not
   the refusal anyone is expecting to see.
3. **The `bootc-too-old` gate is a `grep -q` under `pipefail`.**
   `bootc install to-existing-root --help | grep -q -- --composefs-backend` at
   `apex-boot-migrate:298`. Measured five times on the L16: rc=0, because the
   help is 5874 bytes and fits the pipe buffer, so bootc exits before grep
   closes. It is latent, not live — if that help text ever grows past 64 KiB,
   every machine refuses `bootc-too-old` with a working bootc. Same family as
   the recorded `printf | grep -q` → 141 trap. One-line fix
   (`out="$(bootc … --help 2>/dev/null)"; case "$out" in …`), deliberately not
   taken here because it is another unit's file and the defect is not firing.
4. **`sdboot-migrate-2` is dispatched for the full APEX-on-btrfs migration**
   (its NEXT item 1) and my lab overlaps its setup. What is reusable:
   `/var/lab-scratch/sdboot-xbootldr/apexgrub.img` is `apex-os:daily`
   (ostree + GRUB, **btrfs**, 512 MiB ESP, `--root-size 40G`) with a 2 GiB
   `EA00` XBOOTLDR added in the tail — an L16-shaped guest that costs ~40
   minutes to build. `ctl.img` carries `action.sh` + the engine; the boot
   harness is `/var/lab-scratch/sdboot-migrate/boot-mig.sh` (persistent NVRAM,
   `--ctl`). Hand them the image rather than building a second one.
5. **The two-ESP question is unowned.** bootc picks with
   `find_first_colocated_esp()` (first ESP walking up to the root disk);
   `apex-boot-migrate`'s `find_esp` picks by GPT type GUID on the root's disk.
   Neither is "the one you meant" on a machine with two — which is katana today
   (its APEX `Boot0000` is on the *Windows* disk's ESP) and every Windows
   dual-boot machine tomorrow if the second-ESP route is taken. Same family as
   `sdboot-migrate`'s NEXT #5.

## BOUNDS HONOURED

Nothing touched the L16's partitions, ESP, NVRAM or bootloader. The only reads
of the machine were `stat` on two files under `/usr/lib/modules`, `lsblk`, and
`find_xbootldr` (lsblk only). The APEX guest in the measurement above was
mounted **read-only** from an image file `sdboot-image` left behind. Every
install went through `tests/lab/bootc-install-lab` under `nvram-guard`. katana
untouched.
