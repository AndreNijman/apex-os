# L-002 — the ESP fallback check that never ran, and what it says now

**2026-09-21, round 39b, unit `luks-boot`, commit `e8f1a97d`.**

## What was outstanding

Round 38's run of `installer/test-installer-luks-boot.sh` booted a disk
`apex-install` wrote — `switched-root=yes`, the first time that had ever been
measured — with **one** failure:

```
mount warning:
      * loop1p1: Can't mount, would change RO state
FAIL  the ESP mounts so the fallback loader can be checked mount of /dev/loop1p1 failed
```

So the `EFI/BOOT/BOOTX64.EFI` check — the only thing in the suite that says
whether OVMF can find this disk's loader at all — never executed.

## The diagnosis

The kernel prints that message from `get_tree_bdev()` when it finds a **live
superblock** for the block device whose `SB_RDONLY` flag differs from the one
being asked for. It has exactly one meaning: the ESP was still mounted
**read-write** at that moment. Mount namespaces do not enter into it; a
superblock is global to the kernel.

That is worse than a check that did not run. Twenty lines later the suite
detaches the loop device and hands the **image file** to a qemu in a different
container, on the stated grounds that a boot must see what a completely
independent reader of that file would see. A live rw vfat mount breaks exactly
that — and the `losetup -d` fails silently too, under its own `2>/dev/null`.
`apex-install` mounts the ESP at `$TROOT/boot/efi` and releases it with
`umount -R … 2>/dev/null || true`, so a failure there is silent by
construction.

## The answer, measured without a reinstall

The kept disk from round 38's run is still on the L16 at
`/var/lab-scratch/apex-luks-boot.GLHdmI/target.img` (26 GB). Attached over a
fresh read-only `losetup -r -fP` and mounted `-o ro`:

```
EFI/BOOT/BOOTX64.EFI      949424 bytes   (shim)
EFI/BOOT/fbx64.efi         87816 bytes   (the UEFI fallback loader)
EFI/fedora/{shimx64.efi, shim.efi, grubx64.efi, mmx64.efi, BOOTX64.CSV,
            grub.cfg, bootuuid.cfg}
```

Partition table: p1 1 GiB ESP, p2 1 GiB `apex-boot`, p3 22 GiB `apex-luks`,
p4 1 MiB BIOS boot.

**The fallback loader is there.** The check round 38 never ran would have
passed, and finding that out cost no 25-minute reinstall.

## The fix

The block now asks `findmnt -n -o TARGET,SOURCE,OPTIONS -S "$ESP_PART"` FIRST
and reports what holds the partition, with `fuser -vm` beside it. Only holders
under `$WORK` are unmounted — this suite does not get to unmount things it did
not create; anything else is named and left alone. Either way the fallback
check then runs and answers. `mount`'s stderr is kept rather than discarded,
because "would change RO state", "wrong fs type" and "no such device" are three
different defects and the first cost a round to diagnose from a one-line
summary.

## Tested both ways, against the disk in question

The block was **lifted out of the suite** (not restated) and run against the
ESP of the very disk round 38 booted, copied out to a scratch file so the kept
image stayed untouched:

| case | result |
|---|---|
| nothing holds the partition | `2 passed, 0 failed`, `FALLBACK_PRESENT=1` |
| a deliberate live **rw** mount on the same device | holder detected and named (target, source, options, `fuser`), suite **FAILS**, holder cleared, and the fallback check **still answers** `FALLBACK_PRESENT=1` |

A gate that runs and inspects nothing is this repo's dominant defect family.
This one now fails in both directions.

## `welcome-seen` — decided

It was never asserted by anything: `installer/luks-boot-drive.py` prints it and
the suite reads three other fields and not this one. So the outstanding "decide
whether first-boot welcome is a criterion" was about an informational field, not
a failing check.

**It is not a criterion of this suite**, and the suite now says so in place.
This suite's question is the one in its name — does the encrypted disk this
installer wrote BOOT — and that is fully answered by the prompt being drawn, the
passphrase being accepted, and dracut handing off to the real root. Everything
after `Switching root` belongs to the **installed system**: first-boot welcome
is apex-shell's, it needs a graphical session this headless serial guest never
starts, and gating a LUKS boot test on it would make a shell regression read as
an encryption defect. It is still reported, because a measurement that is taken
and then hidden is how "nobody ever checked" becomes "somebody checked and it
was fine".
