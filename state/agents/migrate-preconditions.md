# migrate-preconditions — decide, before anything is written, whether this machine may migrate

items: none (dispatched as the precondition half of the GRUB → systemd-boot pivot)
repo: apex-os
worktree: /var/tmp/apex-work/wt-migrate-preconditions
branch: task/migrate-preconditions, cut from roadmap/v2.2 @ 4031d43f
lab: /var/lab-scratch/migrate-preconditions

## THE UNIT

Everything that runs BEFORE `apex-boot-migrate stage` writes a byte: is this
machine safe to migrate, which ESP would be written, and — when the answer is
no — what the user can actually do about it. The refusal path is the product.

Andre, verbatim: "active machines should automitcally migrate with sudo apex
install, not this dumbass reinstall shit" / "You have to figure out a way to
make it work for all machiens" / "it has to work in 512. i dont care how it
just has to work." / "also it should work on the katana."

## MEASURED FIRST — the brief's description of the L16 is STALE

Do not plan against the brief's "512M ESP, BitLocker p3, btrfs p4 /sysroot".
Measured on the live L16 2026-09-21 (`lsblk`, `blkid`, `bootc status`,
`efibootmgr -v`, all read-only):

| part | size | type | contents |
| --- | --- | --- | --- |
| nvme0n1p1 | 600M | EFI System | vfat, PARTUUID 1c417de2-5766-455f-9318-198610885424 |
| nvme0n1p2 | 2G | Linux extended boot (XBOOTLDR) | ext4 |
| nvme0n1p3 | 396.3G | Linux filesystem | btrfs `fl_fedora` |
| nvme0n1p4 | 2G | Linux extended boot (XBOOTLDR) | ext4 `apex-newboot` |
| nvme0n1p5 | 1.5T | Linux filesystem | btrfs `apex-root` -> /sysroot |

* ESP is **600 MiB, not 512**.
* **No BitLocker and no NTFS on this machine at all** — blkid shows only
  vfat/ext4/btrfs. The L16 is NOT the BitLocker test case; katana is.
* /sysroot is **p5**, not p4. There are **two** XBOOTLDR partitions.
* `bootc status`: store `ostreeContainer`, nothing staged.
* `BootCurrent: 0000` -> `HD(1,GPT,1c417de2...)/\EFI\fedora\shimx64.efi`, so the
  booted ESP IS on the root's disk here. The katana split does not reproduce.

Kernel cost, measured on 7.2.3-cachyos2.fc43.x86_64:
vmlinuz 16 898 120 B + initramfs.img 375 558 646 B = **374.3 MiB per
deployment**; `need = per*3 + 48 MiB` = **1170.8 MiB**. The L16's own 600 MiB
ESP fails by ~2x. `initramfs-slim` is the lever (WIP commit 847b5bfb, touches
Containerfile.{apex,core,kernel} ONLY — no overlap with this unit's file; it
has no agent card yet).

## NEXT (fill in as you go)

1. Mirror bootc's OWN ESP choice, do not invent one. Check whether cmd_stage
   passes an ESP to bootc or only lets it discover one.
2. BitLocker: verdict is `proceed-with-note`, NOT a refusal. See DECIDED.
3. precheck must be read-only + leak-free before it is run on the real L16.
4. `precheck --explain`: evaluate every check, print all verdicts. That is the
   decision table.
5. Both-ways gate as PATH shims in tests/. Loopback is evidence, not the gate.

## DECIDED (with reasons, so nobody re-opens them)

- **BitLocker is a NOTE, not a refusal.** Suspending BitLocker leaves the
  `-FVE-FS-` signature in place — suspension stores the key in the clear and
  the volume stays FVE-formatted. So a refusal keyed on that signature can
  never be cleared by the instruction we would give the user; only full
  decryption clears it. That makes it a PERMANENT refusal on every BitLocker
  dual-boot machine, which loses the same argument sdboot-migrate already
  settled for katana ("a refusal would keep katana on the Windows disk
  forever") and contradicts "all machiens". The migration itself is additive
  on the ESP, touches nothing under \EFI\Microsoft and never edits Windows'
  Boot####, so it does not move Windows' PCRs. The real hazard is
  POST-migration: systemd-boot auto-detects bootmgfw.efi on a shared ESP and
  chainloading Windows through sd-boot is what trips PCR 4/7.
- **The gate is PATH shims, not loopback.** The CI runner is a second
  environment with no guaranteed /dev/loop-control or CAP_SYS_ADMIN, and a
  loop image can never reproduce the katana case at all, because
  `booted_esp_partuuid` reads the host's real `efibootmgr -v`.

## FOUND (defects in the shipped path, not mine)

- `cmd_precheck` mounts the host ESP **read-write** via `with_esp`, while its
  own documented contract says "(writes nothing)". Mounting FAT rw sets the
  dirty bit.
- The ESP mount **leaks**: `refuse "no-kernel"` (line ~378) fires after
  `with_esp` with no `release_esp`, and `refuse` is `exit 10`. There is no
  EXIT trap. Already true on the shipped path.

## IN PROGRESS

Orientation done, card written, nothing committed yet.
