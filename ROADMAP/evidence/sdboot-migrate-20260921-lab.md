# Migrating a live machine to systemd-boot — what the VM lab measured, 2026-09-21

Unit `sdboot-migrate`. The predecessor's conclusion — *"there is no in-place
ostree → composefs converter, so an existing machine cannot be converted; it is
a reinstall"* — is **wrong**, and this is the run that shows it, plus everything
APEX had to add to make the conversion survive a power cut.

Nothing here touched a real boot path. Every install and boot was a loopback
disk image under OVMF, every run went through `tests/lab/nvram-guard`, and
every one of the ~30 runs returned **`verdict: verified — boot variables
identical before and after`**.

Lab directory `/var/lab-scratch/sdboot-migrate`. Guest image
`localhost/sdmig:v2` = `quay.io/fedora/fedora-bootc:43` +
`systemd-boot-unsigned podman skopeo gdisk dosfstools efibootmgr rsync` + a
boot-time driver that runs whatever `action.sh` the control disk holds.
Versions: bootc 1.16.13, systemd 258.10-1.fc43, OVMF edk2-2970e5699ba6.

The guest starts as an **ordinary GRUB machine** — `tests/lab/bootc-install-lab
--filesystem ext4` with no `--bootloader`, so ostree + GRUB + bootupd, ESP 512
MiB, `/boot` a directory on the root filesystem and the ESP not mounted. That
is the shape the L16 has.

---

## 0. A lab defect first, because it invalidated the first three runs

`boot-once.sh` and `boot-apex.sh` — the predecessor's boot harnesses, still in
`/var/lab-scratch/sdboot-lab` — both pass:

```
-machine q35,smm=on,accel=kvm
-global driver=cfi.pflash01,property=secure,value=on
-drive if=pflash,...,file=OVMF_CODE_4M.qcow2          <- the NON-secboot build
```

With `secure=on` on the variable flash and a firmware build that has no SMM
variable driver, **every EFI variable write from the guest is silently
discarded**. Measured: `efibootmgr --create-only` returned `Boot000B`, the
entry was visible for the rest of that boot, and after the power cycle
`/sys/firmware/efi/efivars` had no `Boot000B` and no `BootNext` at all.

A migration whose commit point is an EFI variable write cannot be tested on a
machine that throws EFI variable writes away. `boot-mig.sh` drops the flag and
keeps **one persistent `VARS` file per guest** — the other harnesses recreate it
from the pristine template on every boot, which loses the same writes a second
way.

---

## 1. The bare conversion works, and it destroys the old boot path

Run inside the booted GRUB guest, from a container of its own image:

```
podman run --rm --privileged --pid=host -v /sysroot:/target \
    -v /var/lib/containers:/var/lib/containers \
    --security-opt label=type:unconfined_t localhost/sdmig:v2 \
    bootc install to-existing-root --composefs-backend --bootloader systemd \
        --generic-image /target
...
Bootloader: systemd
Installing bootloader via systemd-boot
Installation complete!            <- rc 0
```

Two things had to be right to get there, and both are worth not re-deriving:

* **`--source-imgref` does not work on this path.** Every transport —
  `oci:`, `oci:…:tag`, `oci-archive:`, `containers-storage:` — failed
  identically with `Creating source info from a given imageref: Subprocess
  failed` / `error: Multiple commit objects found` (the message is
  `/usr/bin/ostree`'s). The install must run as a *container of the image*.
* **The target is `/sysroot`, not `/`.** With `-v /:/target` it fails with
  `Creating composefs directory: Read-only file system`: on an ostree host `/`
  inside the container is the deployment, and the physical root — the one that
  holds `/ostree` and would hold `/composefs` — is `/sysroot`.

And then the damage. ESP before (ostree + GRUB) vs after the bare run:

| path | before | after |
| --- | --- | --- |
| `/EFI/fedora/shimx64.efi` — **what the machine's `Boot####` points at** | present | **gone** |
| `/EFI/fedora/{grubx64.efi,grub.cfg,mmx64.efi,BOOTX64.CSV,bootuuid.cfg}` | present | **gone** |
| `/EFI/BOOT/BOOTX64.EFI` | shim `4773d74d…` | systemd-boot `6ca517cd…` |
| `/EFI/BOOT/fbx64.efi` | present | **gone** |
| root fs `/boot`: `grub2/ loader/ loader.1/ ostree/ bootupd-state.json` | present | **wiped, only `efi` left** |

The guest still booted — and that is the trap, not the reassurance. NVRAM was
untouched (`--generic-image`), so its entry still named
`\EFI\fedora\shimx64.efi`, which no longer exists; the firmware fell through to
the removable-media fallback, which bootc had just replaced with an **unsigned**
systemd-boot. Under Secure Boot that fallback is refused. On a machine whose
firmware does not fall through, there is nothing left to boot.

**`/var` was empty**: the marker file written on the pre-migration boot was
gone, `/var/lib/labstage` was absent, `LAB-BOOT` restarted at 1. A bare
migration comes up with no `/var/home` and no user.

---

## 2. Containment: the bind over `/target/boot`

Same install, with a staging filesystem bound over `/target/boot` in the
container. Hashes of every file on the real ESP, taken in the same boot, before
and after:

```
--- esp-before.sha      +++ esp-after.sha
-4773d74d…  ./EFI/BOOT/BOOTX64.EFI
+6ca517cd…  ./EFI/BOOT/BOOTX64.EFI
+f8ec50e2…  ./EFI/Linux/bootc_composefs-<verity>/initrd
+bc028ef5…  ./EFI/Linux/bootc_composefs-<verity>/vmlinuz
+6ca517cd…  ./EFI/systemd/systemd-bootx64.efi
+067fc514…  ./loader/entries.srel
+7abb85fc…  ./loader/entries/bootc_fedora-43-1.conf
+ace4df04…  ./loader/loader.conf
 282a3b28…  ./EFI/fedora/BOOTX64.CSV        <- unchanged
 713311575…  ./EFI/fedora/grub.cfg           <- unchanged
 4773d74d…  ./EFI/fedora/shimx64.efi        <- unchanged
 e6ac4f64…  ./EFI/BOOT/fbx64.efi            <- unchanged
```

**One overwrite in the whole tree**, and it is the removable-media fallback,
which `apex-boot-migrate` saves first and restores afterwards (verified by
sha256). The root filesystem's `/boot` came through with `grub2/`, `loader.1/
entries/ostree-1.conf`, `ostree/` and `bootupd-state.json` all present.

The staging cannot be a plain directory (`error: No UUID found for /boot`) and
its volume id cannot be fresh — see §4.

---

## 3. The whole migration, end to end

`apex-boot-migrate auto` on the booted GRUB guest:

```
apex-boot-migrate: saved the removable-media fallback (949424 bytes)
apex-boot-migrate: migrating to the image this machine is already running:
                   localhost/sdmig:v2 (sha256:a5942c1d…)
apex-boot-migrate: making the booted image available to podman (no download: …)
    Copying local image docker://localhost/sdmig:v2 to containers-storage:localhost/bootc
apex-boot-migrate: carrying 2 kernel arguments across: console=ttyS0,115200n8 …
apex-boot-migrate: installing the composefs deployment (the old path is not touched)
Installing image: docker://localhost/sdmig:v2
Bootloader: systemd
Installation complete!
apex-boot-migrate: restored /EFI/BOOT/BOOTX64.EFI to what it was
apex-boot-migrate: joined /var: moved the machine's var into the composefs stateroot,
                   left /sysroot/ostree/deploy/default/var -> ../../../state/os/default/var
apex-boot-migrate: carried /etc across (44M)
apex-boot-migrate: removed the working copy of the image (bootc)
apex-boot-migrate: staged: the new boot path is written and nothing the firmware reads has changed
apex-boot-migrate: committed: the next boot is systemd-boot (Boot000B).
```

State immediately after, on the still-running old system:

```
phase:      committed
store:      ostreeContainer          <- still booted the ostree deployment
loader:     GRUB 2.12
BootOrder:  000A,0000,…              <- UNCHANGED. 000B is not in it.
/sysroot/boot: boot bootupd-state.json efi grub2 loader loader.1 ostree
```

The next boot:

```
BdsDxe: starting Boot000B "APEX-OS" from HD(2,GPT,…)/\EFI\systemd\systemd-bootx64.efi
LAB-LOADER-INFO:  systemd-boot 258.10-1.fc43
LAB-KERNEL-CMDLINE: initrd=\EFI\Linux\bootc_composefs-462d9ba6…\initrd
                    root=UUID=af17466d-… rw boot=UUID=45C8-D574
                    console=ttyS0,115200n8 systemd.journald.forward_to_console=1
                    composefs=462d9ba6… systemd.mount-extra=UUID=45C8-D574:/boot:auto:ro
LAB-VAR-MOUNT:    /dev/vda3[/state/os/default/var] /var ext4
LAB-VAR-MARKER:   written-on-boot-1        <- the file the OLD system wrote
var/lib entries:  31
/sysroot/boot:    boot bootupd-state.json efi grub2 loader loader.1 ostree
```

and the confirm unit, with **nothing calling it by hand**:

```
● apex-boot-migrate-confirm.service - Confirm or fail the systemd-boot migration
     Active: active (exited) since …; 181ms ago
    Process: 959 ExecStart=… confirm (code=exited, status=0/SUCCESS)
phase:      confirmed
BootOrder:  000B,000A,0000,…            <- GRUB kept, behind it
boot-complete.target: active
(no AVCs)
```

`boot-complete.target` **is** reached on this machine even though the migrated
entry carries no boot counter — that was the open question about the unit's
ordering, and the answer is measured, not assumed.

---

## 4. Two defects the lab found that reasoning had missed

**The staging filesystem's UUID leaks into the kernel command line.** bootc
reads a UUID off `/target/boot` and writes it as `boot=UUID=…` and
`systemd.mount-extra=UUID=…:/boot:auto:ro`. With a fresh volume id on the
staging image, the migrated machine booted and then:

```
Timed out waiting for device /dev/disk/by-uuid/BAE2-8D2F
Dependency failed for boot.mount - /boot
You are in emergency mode.
```

The staging is now created with `mkfs.vfat -i <the real ESP's volume id>`, and
the stage refuses (`entry-names-wrong-boot`) if the entry it is about to commit
names anything else.

**Symlinking the composefs stateroot at the ostree `/var` gives a read-only
`/var`.** Same guest, same image, the two arrangements measured side by side:

```
stock composefs        /dev/vda3 /var ext4 rw     (and /sysroot ro)  touch -> ok
stateroot is a symlink /dev/vda3 /var ext4 ro                        touch -> EROFS
```

So the machine's `/var` is **moved into** the composefs stateroot and the
symlink is left at the ostree path instead. That rename is `EBUSY` on a running
machine — `/sysroot/ostree/deploy/default/var` is itself a mountpoint — so it
happens inside `unshare -m --propagation private`, where the lazy unmount is
namespace-local. Measured afterwards in the parent namespace: `touch /var/tmp/…`
still succeeds, `/var/lib` still lists.

And the GRUB path, booted after the migration, resolves the symlink:

```
LAB-VAR-MOUNT: /dev/vda3[/state/os/default/var] /var ext4    <- from the GRUB boot
var/lib entries: 31   writable: yes
```

One `/var`, both paths, read-write from either.

---

## 5. The crash matrix — the power actually cut

`--kill-after N` sends `SIGKILL` to qemu; the ESP-write case is triggered from
inside the guest with `echo o > /proc/sysrq-trigger` the instant the ESP grows,
which is an immediate power-off with no sync and no filesystem flush.

| # | where the power was cut | evidence it landed there | next power-on |
| --- | --- | --- | --- |
| A | 75 s in, during `bootc install` | `qemu exit: 137` | **GRUB**, `ostree=/ostree/boot.1/…`, marker intact, `/var` writable, `phase: not started` |
| A3 | inside `bootc install`, after `Digest:` | progress file ends at `esp-kb=7684` | **GRUB**, same |
| A4 | inside `bootc install`, later | same | **GRUB**, same |
| **ESP** | **mid-write on the FAT ESP** | `Installing bootloader via systemd-boot` → `ESP HAS GROWN TO 22296kb — CUTTING POWER NOW` | **GRUB**, marker intact, 31 `/var/lib` entries, writable |
| C | after the stage, after `efibootmgr --create-only`, **before** `BootNext` | `phase: staged`, `Boot000B` present | **GRUB** — `BootOrder: 000A,…`, unchanged |
| D | after `BootNext`, before the boot | `phase: committed` | **systemd-boot** — the migration |
| E | 6 s into the first boot of the new path, before confirm | `BdsDxe: starting Boot000B "APEX-OS"` | **`BdsDxe: starting Boot000A "Fedora"`** — the firmware went back to GRUB by itself |

Nothing in row E was done by a person or a script: `BootNext` is a one-shot the
firmware consumes before it launches the loader.

**Re-running after a crash works.** The machine whose ESP write was cut mid-flight
took `apex-boot-migrate auto` again and completed — `joined /var`, `carried /etc`,
`staged`, `committed`, rc 0.

**And a failed trial is not retried on its own.** After row E the confirm unit
ran on the GRUB boot, saw `phase=committed` with a loader that is not
systemd-boot, and recorded:

```
phase: failed
$ apex-boot-migrate auto
apex-boot-migrate: REFUSED [last-attempt-failed]
    The last migration was committed and its trial boot did not come up …
    run 'apex-boot-migrate retry' to arm it once more.
auto rc=10
$ apex-boot-migrate retry
apex-boot-migrate: armed again. …
phase: staged
```

Without that, every `apex update` would spend a reboot on the same failure.

---

## 6. A migrated machine can still take an update

The question the whole migration rests on. On the confirmed guest:

```
$ bootc switch --transport oci-archive /mnt/ociimg/img3.tar
switch rc=0
entries:          bootc_fedora-43-1.conf
entries.staged:   bootc_fedora-43-0.conf  bootc_fedora-43-1.conf
```

and after the reboot:

```
loader:       systemd-boot 258.10-1.fc43
lab-version:  v3                    <- the new image
v3 marker:    lab v3
var marker:   written-on-boot-1     <- still the file the pre-migration system wrote
var writable: yes
staged:       null
rollback:     <the migrated deployment>
entries:      bootc_fedora-43-0.conf  bootc_fedora-43-1.conf
```

`bootc switch` rather than `bootc upgrade` only because the lab has no registry
the guest can reach; both stage a composefs deployment through the same
`entries.staged` → `bootc-finalize-staged` path the predecessor measured.

---

## 7. What is still NOT measured

* **The APEX image itself.** Everything above is `fedora-bootc:43` plus seven
  packages, on **ext4**. APEX's root is btrfs and its initramfs is 375 MiB.
* **Secure Boot.** Every boot was non-SB OVMF. The precheck refuses on a
  Secure Boot machine with an unsigned loader, which is the L16 today, so on
  the L16 this code path has never run past `precheck`.
* **A real `bootc upgrade`** over a registry, on a migrated machine (§6 used
  `bootc switch` from a local archive).
* **`apex update` end to end on hardware.** The Rust wiring is built and its
  behaviour on each exit code is asserted in `tests/test-boot-migrate.sh`, but
  no machine has run `sudo apex update` through it.
* **A machine with `/etc` changes between the stage and the reboot.** `/etc` is
  copied during the stage; anything written after it is not carried.
* **katana.** `find_esp` picks the ESP on the disk the root filesystem is on;
  katana's APEX `Boot0000` lives on the **Windows** disk's ESP.
