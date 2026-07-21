# M4 — Installing APEX-OS Daily (safe runbook)

Two install targets, in order of when they happen:

- **A. External wipeable drives (near-term, executable autonomously).** Andre
  plugs in 2 disposable USB/external drives explicitly cleared for wiping. APEX-OS
  installs onto *those* — self-contained bootable drives, booted via the firmware
  boot menu (F12). This touches nothing on the internal disk: no shrink, no
  bootloader edit, no reboot of the L16. This is the immediate M4 target and is
  safe to run from within running Void (bootc `to-filesystem` via privileged
  podman, proven by M0 spike E). See §A.
- **B. Internal dual-boot on the L16 (eventual daily-driver migration).** Shrinking
  Void's partition for a real dual-boot — **user-present, from a Fedora live USB,
  never from running Void.** See §B. Not done until Andre commits to daily-driving.

---

## §A. External-drive install (near-term)

### Hard safety guard (non-negotiable)
The install script MUST refuse to touch anything that isn't a clearly-removable
external drive. Before wiping any device it verifies ALL of:
- device is NOT `nvme0n1` (the internal Void+Windows disk) — hard denylist;
- `/sys/block/<dev>/removable` == 1 OR transport is `usb` (`lsblk -dno TRAN`);
- the device and its partitions are NOT mounted and NOT the backing store of `/`;
- the device is not part of any active LUKS/LVM/RAID in use.
Any check fails → abort, no write. The target is passed explicitly (never a
glob/auto-pick that could resolve to the internal disk).

### Per-drive procedure (parameterised: `TARGET=/dev/sdX`, `EDITION=daily|gaming`)
1. Re-run the guard above against `$TARGET`. Print the model/size and its current
   partition table; proceed only after the guard passes.
2. `wipefs -a` + fresh GPT (`sgdisk --zap-all`).
3. Partitions: p1 ESP 1 GiB (FAT32), p2 = rest → LUKS2 + btrfs (`@`,`@home`,`@var`).
   (For a first bring-up drive, LUKS can be skipped for speed — note it; the
   internal daily-driver install always uses LUKS.)
4. `bootc install to-filesystem` the signed image (`ghcr.io/andrenijman/apex-os:$EDITION`)
   onto the mounted target, target ESP at `/target/boot/efi`, empty target, never
   `--replace=alongside`. Reuse `files/scripts/spike-e/guest/spike-install.sh`.
5. The drive is now self-contained bootable. Boot the L16 → F12 → pick the USB.
   Nothing on the internal disk changed; unplug to return to normal Void boot.
6. Two drives → suggested: one `daily`, one `gaming` (a real portable install of
   each edition to bench + verify without ever repartitioning the L16).

This is the M4 gate done on real hardware, safely: greeter render + login + apexd
+ suspend + AVC check, all on a bootable external drive.

---

## §B. Internal dual-boot on the L16 (eventual, user-present, live-USB only)

This is a **user-present, supervised** procedure. It is NOT run autonomously and
NOT run from within the live Void system. It coexists with Void (dual-boot);
Void stays the default boot and a working fallback until APEX-OS is proven.

## GOLDEN RULE — never repartition the running root

The L16 boots Void from `/dev/nvme0n1p5` (btrfs, mounted `/`). **Do not shrink
that partition from within running Void.** Online-resizing the btrfs *filesystem*
is reversible, but shrinking the *partition* while it is the mounted root is the
data-loss path: the kernel won't cleanly re-read the partition table of an in-use
disk, and any fs-vs-partition size mismatch corrupts the filesystem. This session
also has no `efivarfs` (can't even add a boot entry from here), which is a second
reason the install belongs to a separate boot environment.

**All repartitioning is done from a Fedora live USB with Void unmounted.** That
environment also provides working efivars, SELinux labelling, and cgroups (which
bootc-image-builder / osbuild need and the running Void session lacks).

## Pre-flight (captured read-only from running Void, 2026-07-21)

| Item | State | Consequence |
|---|---|---|
| ESP `/dev/nvme0n1p1` | 10 G, **9.8 G free** | ample; bootc adds ~7.5 MiB (`\EFI\fedora` + `\EFI\BOOT`) |
| ESP contents | Void `vmlinuz-void`/`initramfs-void*` (EFISTUB ladder) + Microsoft | bootc must NOT be allowed to reorder BootOrder; re-assert after |
| Root `nvme0n1p5` | 1.4 T btrfs, **505 G free** (min 257 G) | can carve ≤~250 G safely for APEX |
| `$HOME` | **845 G** (Projects 167 G, src 46 G, Pictures 46 G, VMs 73 G…) | **cannot** wholesale-copy into a 250 G partition — migration is SELECTIVE, or allocate more |
| Partitions | p1 ESP · p2 MSR · p3 Win NTFS 476 G · p4 recovery · p5 Void btrfs 1.4 T | new **p6** = freed space at end of disk |
| efivars (this session) | not mounted / unsupported | boot-entry step requires the live-USB/real boot env |

**Decision required from Andre before install:** APEX partition size vs what to
migrate. A 250 G APEX root can't hold the full 845 G home — either (a) selective
migration (dotfiles + chosen data, VMs/Pictures stay on Void or move later) or
(b) shrink Void much more and give APEX the larger share. This is a judgement call
about how fast you're committing to APEX as the daily driver.

## Safe procedure (from the Fedora live USB, Void unmounted)

Let `NEW_P6=300G` (example). `DISK=/dev/nvme0n1`.

1. **Back up the partition table first.**
   ```
   sgdisk --backup=/run/media/…/nvme0n1-gpt-$(date +%F).bin "$DISK"
   efibootmgr -v > /run/media/…/efibootmgr-before.txt
   ```
2. **Shrink the FILESYSTEM before the partition** (fs must always be ≤ partition).
   ```
   mount /dev/nvme0n1p5 /mnt            # from the live USB, NOT the running root
   btrfs filesystem resize -305G /mnt   # shrink fs by a bit MORE than p6 (margin)
   umount /mnt
   ```
3. **Shrink partition p5, create p6** in the freed tail. Shrink p5 so its new end
   still leaves the fs (step 2) comfortably inside it, then p6 fills the rest.
   Use `parted`/`sgdisk` carefully; verify sizes before writing. p6 type = Linux
   filesystem (8300).
4. **LUKS2 + btrfs on p6** (§5b/§5c layout):
   ```
   cryptsetup luksFormat --type luks2 /dev/nvme0n1p6
   cryptsetup open /dev/nvme0n1p6 apex
   mkfs.btrfs /dev/mapper/apex
   mount /dev/mapper/apex /mnt && btrfs subvolume create /mnt/@ \
       && btrfs subvolume create /mnt/@home && btrfs subvolume create /mnt/@var
   # recovery key → the memory vault; TPM2 enrol AFTER first successful boot
   ```
5. **bootc install** to the new root (mechanism proven by M0 spike E — reuse
   `files/scripts/spike-e/guest/spike-install.sh` as the template; empty target,
   **never `--replace=alongside`** on the shared ESP, shared ESP mounted at
   `/target/boot/efi`, `--karg root=UUID=… rd.luks…`). Pull the signed image:
   `ghcr.io/andrenijman/apex-os:daily` (make the package public or `podman login`).
6. **Boot entry — keep Void the default.** Add an APEX entry but do NOT move it
   ahead of Void's `Boot0006` yet. Re-assert BootOrder so Void is first; diff
   against `efibootmgr-before.txt` to confirm nothing else was reordered/dropped.
   Confirm Void's `\EFI\BOOT\BOOTX64.EFI` wasn't the thing bootc overwrote (it has
   a dedicated entry, so it's safe, but verify).

## First-boot verification (Void still default — this is the M4 gate)

Boot APEX **manually** (F12 one-shot), Void untouched:
- greeter (apex-greet under sway) renders + password login works;
- Brain_Shell/APEX Shell comes up themed (chartreuse), hot-reload intact;
- `apex status` reports amd-zen/thinkpad-l16-g2, tiers apply, AC/battery autoswitch;
- suspend/resume works (`apex doctor suspend`); charge thresholds honoured;
- `ausearch -m avc` shows no unexplained denials;
- migration checklist items work (sing-box via systemctl, Chrome, ghostty, fish/zoxide, docker, Bambu, NPU llama.cpp-Vulkan…).

**If anything fails: reboot → Void (still default) → nothing lost.** Only after a
parallel-run soak (days–weeks, per §12) do you make APEX the default and TPM2-enrol.

## What is done autonomously vs here

- Autonomous (done / in progress): image built + signed on ghcr, VM install+boot
  proof, this runbook, the spike-E install scripts, non-destructive pre-flight.
- Supervised (this runbook, user-present): the live-USB repartition, LUKS setup,
  bootc install to bare metal, boot-entry edit, reboot, and the daily-run soak.
