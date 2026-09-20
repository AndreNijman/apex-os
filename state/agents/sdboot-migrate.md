# sdboot-migrate — move a live machine to systemd-boot, in place, safely

- **Repo / branch**: apex-os, `task/sdboot-migrate`, branched from
  `roadmap/v2.2` @ `602a8376`. Three commits, all pushed:
  `6b40edb5` the engine, `70181f18` the `apex update` wiring and the confirm
  unit, `ae239be5` docs, tests and evidence. **Not landed.**
- **Worktree**: `/var/tmp/apex-work/wt-sdboot-migrate`.
- **Evidence**: `ROADMAP/evidence/sdboot-migrate-20260921-lab.md` — ~30 guest
  boots, the crash matrix with the power actually cut, and the two defects the
  lab found that reasoning had missed.
- **The design lives in the repo**: `docs/boot-v2.md`, "Migrating a machine
  that already exists". `AGENTS.md` rule 5 is the contract version.

## What this round settled

**The predecessor's conclusion was wrong, and Andre was right to reject it.**
`bootc install to-existing-root --composefs-backend --bootloader systemd`
converts a running ostree + GRUB machine in place. `bootc --help` listing no
migration verb is true and means something else.

What bootc will not do on its own is survive a power cut: run bare it deletes
`/EFI/fedora` — the file the machine's own NVRAM entry points at — overwrites
`/EFI/BOOT/BOOTX64.EFI`, and wipes the root filesystem's `/boot`. The guest
that did that booted **only** because the firmware fell through to the
removable-media fallback bootc had just replaced with an unsigned loader.

| file | what |
| --- | --- |
| `files/system/libexec/apex-boot-migrate` | precheck / stage / commit / confirm / retry / abort. The whole engine. |
| `files/system/units/apex-boot-migrate-confirm.service` | writes `BootOrder` after a boot that worked, or records the migration as failed |
| `apexd/apex/src/ops.rs` | `apex update` runs the migration **instead of** `bootc upgrade`; exit 10 (refused) is "carry on" |
| `Containerfile.base` | ships both, enables the unit, asserts podman/mkfs.vfat/rsync/efibootmgr/unshare and the Wants=-not-Requires= ordering |
| `tests/test-boot-migrate.sh` | 50 assertions, 13 mutations, all caught; wired into `pr-validation.yml`'s `static` job |
| `docs/boot-v2.md`, `docs/update-cost.md`, `AGENTS.md` | the design, the disk cost, and the corrected rule 5 |
| `apexd/apex/src/boot.rs` + `main.rs` + `tests/test-boot-v2.sh` | the "GRUB is the default" string, its help and its test, changed together |
| `tests/lab/bootc-install-lab` | its usage text cited rule 5 for the opposite of what rule 5 says |

### The shape, in one paragraph

**Stage** runs the install with a throwaway FAT filesystem bound over
`/target/boot`, so bootc's wipe lands on scratch: `/EFI/fedora` comes through
byte-identical, the root's `/boot` is untouched, and the one overwrite on the
ESP is the removable-media fallback, which is saved and put back. **Commit** is
one `SetVariable` of `BootNext` — the entry is created with `--create-only`, so
it is not in `BootOrder` and changes nothing. **Confirm** writes `BootOrder` on
a boot that worked; on a boot that came back to GRUB it records `phase=failed`
and `apex update` stops re-arming. GRUB is demoted, never removed.

### Two defects the lab found that argument would not have

* **The staging filesystem's UUID leaks into the kernel command line.** bootc
  bakes `boot=UUID=…` from `/target/boot`; with a fresh volume id the migrated
  guest dropped to emergency mode on `boot.mount`. It now gets the real ESP's
  volume id, and the stage refuses if the entry names anything else.
* **Symlinking the composefs stateroot at the ostree `/var` gives a read-only
  `/var`** — measured both ways in one guest. So the machine's var is *moved
  into* the stateroot (inside `unshare -m`, because it is a mountpoint and a
  plain rename is EBUSY) and the symlink is left at the ostree path.

### And one lab defect that invalidated three runs

`boot-once.sh` and `boot-apex.sh` pass `-global
driver=cfi.pflash01,property=secure,value=on` with the **non-secboot** OVMF
build. Every EFI variable write from the guest is then silently discarded —
`efibootmgr --create-only` returns a Boot####, and it is gone after the power
cycle. They also rebuild `OVMF_VARS` from the template on every boot. Any NVRAM
test on those harnesses proves nothing. `/var/lab-scratch/sdboot-migrate/boot-mig.sh`
is the fixed one (persistent vars, `--kill-after` for power cuts, `--ctl` for a
control disk that drives the experiment without an image rebuild).

## NEXT — for a stranger

1. **Run it against the APEX image, on btrfs.** Everything measured so far is
   `fedora-bootc:43` + seven packages on ext4. APEX's root is btrfs and its
   initramfs is 375 MiB, which is the whole ESP problem. The lab is set up for
   it — the control disk means no image rebuild between experiments:

   ```
   # a GRUB/ostree guest from the APEX image, the shape the L16 has
   cd /var/tmp/apex-work/wt-sdboot-migrate
   sudo tests/lab/bootc-install-lab --size 43G --filesystem btrfs \
     localhost/apex-os:daily /var/lab-scratch/sdboot-migrate/apexmig.img \
     -- --karg console=ttyS0,115200n8 --karg systemd.journald.forward_to_console=1

   cd /var/lab-scratch/sdboot-migrate
   ./setctl.sh act-migrate.sh \
     /var/tmp/apex-work/wt-sdboot-migrate/files/system/libexec/apex-boot-migrate
   sudo /var/tmp/apex-work/int-os/tests/lab/nvram-guard --label apexmig -- \
     podman run --rm --device /dev/kvm -v /var/lab-scratch/sdboot-migrate:/work \
     localhost/apex-bootlab -c '/work/boot-mig.sh /work/apexmig.img \
     /work/apexmig.serial 1800 --ctl /work/ctl.img'
   ```

   The APEX image has podman, rsync, dosfstools and efibootmgr already, so it
   needs no oci disk: `bootc image copy-to-storage` gets the image locally.
   Expect the ESP refusal to fire on a 512 MiB ESP — that is the point of the
   run. Then repeat with a 2 GiB ESP and see it through.
2. **The L16 does not migrate today, and both reasons are real.**
   `secure-boot-unsigned-loader` (Secure Boot is **enabled** — `mokutil
   --sb-state` on the machine) and `esp-too-small` (600 MiB against ~1.1 GiB).
   Neither is this unit's to fix:
   - the first needs the Secure Boot decision in `docs/boot-v2.md`, "Secure
     Boot: a decision, not a measurement", plus a signer for
     `systemd-bootx64.efi` and a `/usr/share/apex-os/boot/sdboot-signed`
     marker for the precheck to read;
   - the second needs a bigger ESP. **p2 on the L16 is a dead July `/boot`** —
     2 GiB, ext4, XBOOTLDR-typed, unmounted, 292 MB of stale ostree/grub2 —
     sitting immediately after the 600 MiB ESP. Deleting it and growing p1 is
     the obvious move and it was not attempted here. Nothing on this machine
     may be touched without Andre.
3. **`/etc` written between the stage and the reboot is not carried.** A user
   added, a Wi-Fi profile saved, in that window, is lost on the first boot of
   the new path. A shutdown-time re-sync ordered like `apex-boot-count`'s
   ExecStop would close it.
4. **Reclaiming the old deployment.** A migrated machine carries two copies of
   its image (~15 GB on APEX) and nothing deletes the ostree one — deliberately,
   it is the recovery path. There is no verb for taking that decision later.
   Whatever it is, it must refuse while `phase != confirmed`.
5. **katana.** `find_esp` picks the ESP on the disk the root filesystem is on;
   katana's APEX `Boot0000` lives on the **Windows** disk's ESP. The precheck
   should notice when the entry it is about to demote points at a different
   ESP than the one it is writing, and refuse.
6. **A Type #1 entry that chainloads `\EFI\fedora\shimx64.efi`** would put GRUB
   in systemd-boot's own menu, and make it the boot counter's fallback once
   `BootOrder` is confirmed — today a migrated machine has exactly one Type #1
   entry, so a counted-out deployment has nothing to fall back to within
   sd-boot.
7. **Tighten the confirm gate.** It is `Wants=boot-complete.target`, not
   `Requires=`, because the migrated entry carries no boot counter so
   `systemd-bless-boot` is condition-skipped. Measured: the target **is**
   reached anyway on the lab guest. If the stage renames the new entry to carry
   `+3-0`, the counter, the blessing and the health gate all become real on the
   first migrated boot and the gate can be `Requires=`.

## For `luks-installer` — flagged, not edited

**`installer/apex-install` still passes neither
`--bootloader` nor `--composefs-backend`**, so both its modes install ostree +
GRUB. That is `luks-installer`'s file and it is live. With this unit landed,
that is no longer a blocker for anybody: a machine installed the old way
migrates itself on its first `apex update` that can do it safely — so the
installer can stay exactly as it is, and a fresh install is no longer a
different machine from an upgraded one. If you do change it, the contract is
`docs/boot-v2.md`, "Migrating a machine that already exists", and the two
things that bite are in there: `--bootloader systemd` is refused on the ostree
backend, and a composefs install needs an ESP that can hold two deployments —
which for an APEX initramfs is ~1.1 GiB, not bootc's 1 GiB default. The
installer is the one place that can simply *make* the ESP big enough.

Bounds that still hold: **do not migrate the L16 or katana**, large artefacts in
`/var/lab-scratch`, address katana's disks by serial, never `pkill -f`.
