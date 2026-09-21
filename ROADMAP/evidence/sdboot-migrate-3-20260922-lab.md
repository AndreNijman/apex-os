# Migrating a REAL APEX machine to systemd-boot — the btrfs lab, 2026-09-22

Unit `sdboot-migrate-3`, round 40. This is the run
`ROADMAP/evidence/sdboot-migrate-20260921-lab.md` ends by asking for: everything
that document measured was `quay.io/fedora/fedora-bootc:43` plus seven packages
on **ext4**, and APEX's root is **btrfs** with a 375 MiB initramfs, which is the
whole reason the ESP is a problem at all.

Two guests, differing in one thing:

| guest | ESP | how it was installed | question |
| --- | --- | --- | --- |
| `apexmig-a` | 512 MiB (bootc's own default) | `tests/lab/bootc-install-lab --filesystem btrfs` | must REFUSE |
| `apexmig-b` | 2 GiB | hand-partitioned `bootc install to-filesystem` | must migrate and boot |

Nothing here touched a real boot path. Every install and every guest boot went
through `tests/lab/nvram-guard`, every install carried `--generic-image`, and
every one returned **`verdict: verified — boot variables identical before and
after`**. The host's own `Boot####` variables were unchanged at 29 variables /
26 entries throughout.

Lab directory `/var/lab-scratch/sdboot-migrate-2` (the predecessor's rig,
reused; this unit's artefacts are suffixed `-3`) with launchers and logs in
`/var/lab-scratch/sdboot-migrate-3`. Guest image `localhost/apex-sdmig:v1` =
`ghcr.io/andrenijman/apex-os:daily` + the control-disk driver. Kernel
`7.2.5-cachyos1.fc43.x86_64`, GRUB 2.12, OVMF `edk2-2970e5699ba6`,
`store=ostreeContainer`.

---

## 0. Four rig defects, because three of them fail silently

The inherited card said the rig was intact and the previous agent had died just
before booting it. The rig was not intact, and none of these announce themselves.

**0.1 — A complete GPT is not a complete install.** `apexmig-a.img` had a
textbook partition table (`p1 BIOS-BOOT 1M EF02 / p2 EFI-SYSTEM 512M EF00 /
p3 root 44.5G`) and 13 GB allocated. Mounted read-only through a loop device:

```
/dev/loop1p2    511M  4.0K  511M   1%      <- the ESP. Empty.
p3:/boot        boot/efi/ and nothing else <- no loader/entries, no kernel, no grub2
p3:/ostree      repo/ 13 GB, deploy/ EMPTY
```

The install had pulled the image into the ostree repo and died before deploying
it or writing a bootloader. Partitioning is simply the step that finished. Had
the guest been booted on the card's word, the run would have produced 1800
seconds of nothing and `console=ttyS0` would have been the natural suspect.

**0.2 — The documented `podman run … -v <labdir>:/work` needs `:z`.**
`/var/lab-scratch/sdboot-migrate-2` is `unconfined_u:object_r:var_t:s0`; the
container is `container_t`; the launch dies with

```
/bin/bash: line 1: /work/boot-mig.sh: Permission denied
```

on a file that is `-rwxr-xr-x`. It is an SELinux **exec** denial wearing a mode
bit's clothes. Unit 1's directory is `container_file_t`, which is why its ~30
boots worked and why the recipe it wrote down was never tested anywhere else.

**0.3 — `boot-mig.sh --ctl` without `--oci` puts the control disk on `vdb`.**
The script appends the OCI drive as `hd1` and the control drive as `hd2`, so
`vdc` — which the image's baked `lab-run.sh` hardcodes — is only right when
both are passed. The first real boot of run A said:

```
LAB-CTL: FAILED to mount /dev/vdc
LAB-OCI: mounted
LAB-ACTION: none on the control disk
```

The guest booted, powered off, and qemu exited 0. This is the "a gate that runs
and inspects nothing" family: a green run that measured nothing. Worked around
with a 4 MiB dummy in the `--oci` slot rather than by editing the baked driver.

**0.4 — `to-filesystem-lab` made no BIOS-BOOT partition, and on btrfs that is
fatal.** Its first real use:

```
/usr/sbin/grub2-install: error: filesystem `btrfs' doesn't support blocklists.
error: boot data installation failed: installing component BIOS to device
       /dev/loop1: installing GRUB on /dev/loop1
```

bootupd installs the BIOS component as well as the EFI one. With no `ef02`
partition, `grub2-install --target i386-pc` has nowhere to embed `core.img`; on
ext4 it would fall back to blocklists in the post-MBR gap, and btrfs refuses.
`bootc install to-disk` creates that partition itself, which is why run A never
hit it. The failure also **leaves a disk that looks plausible and has an empty
ESP** — defect 0.1's exact shape, arrived at a second way.

---

## 1. The real image cannot migrate at all, and not for the reason anyone expected

Run A, guest boot 2, the shape the L16 has — GRUB, ostree, btrfs, ESP not
mounted, `/boot` a subdirectory of the root filesystem:

```
LAB-LOADER-INFO: GRUB 2.12
LAB-BOOTC-STORE: ostreeContainer
/dev/vda3 /sysroot  btrfs ro,relatime,seclabel,discard=async,space_cache=v2
/dev/vda3[/boot] /boot btrfs ro,…
/dev/vda3[/ostree/deploy/default/var] /var btrfs rw,…
vda2 512M vfat c12a7328-f81f-11d2-ba4b-00a0c93ec93b   <- the ESP, unmounted
```

`apex-boot-migrate precheck --explain`, verbatim:

```
STATUS  CHECK                        REASON
------  -----                        ------
OK      uefi                         booted through UEFI
OK      on-ostree                    booted store is ostreeContainer, so there is something to migrate
OK      no-staged-update             no ostree deployment is staged for the next boot
REFUSE  no-rsync                     rsync is not in this image; the migration needs it to carry /etc across.
OK      bootc-new-enough             install to-existing-root has --composefs-backend
OK      secure-boot                  Secure Boot is not enforcing, so an unsigned loader is accepted
OK      root-space                   30 GiB free, 24 GiB needed
OK      esp-choice                   bootc will write PARTUUID 77223857-6b9d-4117-a0db-c4ebd4f01b85 of 1 ESP(s) on the root's disk
REFUSE  esp-too-small                The ESP has 503 MiB free, needs 1173 MiB, short by 669 MiB.

This machine stays on GRUB, working. Every REFUSE above has to clear
before it migrates; run this again after any of them changes.
```

**`no-rsync` is real and it is first.** `ghcr.io/andrenijman/apex-os:daily` —
the published image, which `apex-sdmig:v1` only wraps — has no `rsync`:
`command -v rsync` finds nothing and `rpm -q rsync` says it is not installed.
So plain `precheck` and plain `auto` both stop there:

```
### precheck
apex-boot-migrate: REFUSED [no-rsync]
precheck rc=10
### auto (this is what apex update runs)
apex-boot-migrate: REFUSED [no-rsync]
auto rc=10
```

`roadmap/v2.2`'s `Containerfile.base:1283` does `dnf5 -y install rsync` and
asserts `command -v rsync` at 1296, so this is the **published tag lagging the
branch**, not a defect in the branch. It still means: no machine running the
currently published APEX image migrates, whatever its ESP.

**This is the run that pays for `precheck --explain`.** The verb landed with
`task/migrate-preconditions` and this is its first use on real hardware-shaped
state. Without it, run A's whole output would have been `REFUSED [no-rsync]`
and the 512 MiB refusal — the thing the run existed to measure — would have
been invisible behind an unrelated missing package.

### 1.1 The 512 MiB refusal, which is the headline

```
REFUSE  esp-too-small   The ESP has 503 MiB free, needs 1173 MiB, short by 669 MiB.
```

Measured, not projected: `/dev/vda2 511M 7.6M 504M 2%` with only GRUB's own
`EFI/fedora` + `EFI/BOOT` on it. **1173 MiB** is `per * 3 + ESP_SLACK_MIB` —
**three** deployments' worth of APEX's kernel + initramfs (booted, rollback,
and the one an update stages alongside them) plus 48 MiB, so 1125 = 3 x 375
MiB. A 512 MiB ESP — which is what `bootc install to-disk` gives an APEX
machine by default, read off this disk's own partition table rather than
assumed — has 503 MiB of the 1173 it needs, 43%.

While checking that arithmetic rather than quoting it, one shipped comment
turned out to contradict the code it documents. `ESP_SLACK_MIB`'s header said
*"Two deployments' worth of kernel+initramfs, plus the loader and slack"*,
while the check it feeds does `per * 3` and carries its own comment saying
*"Three, not two."* Anyone deriving the budget from the header gets 798 MiB and
concludes a 1 GiB ESP is comfortable. Corrected on this branch; only the
comment changed.

### 1.2 The root-filesystem precheck fired for real, and passed

`task/sdboot-migrate-2` landed the root free-space refusal and it had never run
outside a shim test. Here it measured the live repo and let the machine through:

```
OK  root-space   30 GiB free, 24 GiB needed
du -sk /sysroot/ostree/repo -> 12701900          (12.1 GiB)
df /sysroot                 -> 45G total, 15G used, 31G avail
```

2 × 12.1 GiB + 512 MiB ≈ 24 GiB, against 30 GiB free. The arithmetic the branch
ships is the arithmetic that ran.

**Do not read this as the check being validated.** All it shows is that the
code path executes and computes what it says it computes. §3 is the same check,
on the same image, letting a migration start that then ran out of disk.

### 1.3 A refusal really does write nothing

```
### status after
phase:        not started
store:        ostreeContainer
loader:       GRUB 2.12
boot entry:   none
BootOrder:    0009,0000,0001,0002,0003,0008,0004,0005,0006,0007,000A
ls: cannot access '/var/lib/apex/boot-migrate': No such file or directory
```

`BootOrder` byte-identical to before, `BootCurrent: 0009` still
`\EFI\fedora\shimx64.efi`, `/sysroot/boot` still holding `grub2 loader loader.1
ostree bootupd-state.json`, and the state directory never created. nvram-guard
on the host: `verified — boot variables identical before and after`.

---

## 2. The 2 GiB guest: every precondition clears

`apexmig-b`, the same APEX image installed by `bootc install to-filesystem`
onto a hand-partitioned disk — `p1 1M EF02 / p2 2 GiB EF00 / p3 btrfs` — so
that the only difference from run A is the size of the ESP. Boot 1 is a prep
boot that makes three lab accommodations, each written into `/etc` so it
survives the migration:

* `systemctl set-default multi-user.target`, because `apex-boot-health`'s
  `do_check` requires the default target to be ACTIVE and APEX ships
  `graphical.target`, which a serial-only guest with no GPU never reaches;
* `TimeoutStartSec=infinity` on `lab-run.service` (the image bakes 900 s, set
  against a 2 GB fedora-bootc guest) plus `After=boot-complete.target
  systemd-bless-boot.service apex-boot-migrate-confirm.service`, so the check
  boot cannot read a race as a result;
* `rsync`, for the reason in §1 — restoring what `Containerfile.base` puts in
  the image on the branch under test.

With those in place the verdict table on boot 2 is clean:

```
OK      tools         mkfs.vfat, rsync and podman are all in this image
OK      root-space    28 GiB free, 24 GiB needed
OK      esp-choice    bootc will write PARTUUID d08a4114-…-d6e09a44e311 of 1 ESP(s) on the root's disk
OK      esp-space     2036 MiB free, 1173 MiB needed
This machine can migrate. "apex update" will do it, or run
"apex-boot-migrate auto" now.
precheck-explain rc=0
```

`2036 MiB free, 1173 MiB needed` against run A's `503 MiB free, 1173 MiB
needed`. Same image, same engine, same arithmetic; only the partition changed.

---

## 3. `root-space` passed and the machine ran out of disk anyway

This is the defect this run exists to have found, and it is in code that landed
last round specifically to prevent it.

`apex-boot-migrate auto` on the guest above, ~70 seconds after the precheck
that had just said 28 GiB was enough:

```
apex-boot-migrate: saved the removable-media fallback (949424 bytes)
apex-boot-migrate: migrating to the image this machine is already running:
    localhost/apex-sdmig:v1 (sha256:629303a3…)
apex-boot-migrate: making the booted image available to podman
    (no download: it is already on this disk)
  Copying local image docker://localhost/apex-sdmig:v1 to containers-storage:localhost/bootc ...
  level=error msg="Rolling back transaction: cannot rollback - no transaction is active"   (x4)
  level=fatal msg="writing blob: storing blob to file
      \"/var/tmp/container_images_storage799695552/89\": no space left on device"
apex-boot-migrate: FAILED [image-unavailable]
    The booted image is not in podman's storage and bootc could not copy it there.
auto rc=1
```

### Why the model is wrong

`cmd_precheck` estimates peak cost as

```
root_need = du -sk $SYSROOT/ostree/repo * 2 + ROOT_SLACK_MIB
```

on the reasoning recorded in `docs/update-cost.md`: the temporary
`bootc image copy-to-storage` copy and the permanent `/composefs` copy can
coexist, so two copies plus slack. Two things that reasoning does not account
for, both measured here:

1. **`copy-to-storage` is itself a two-stage operation.** skopeo stages the
   blobs into `/var/tmp/container_images_storage*` and *then* writes them into
   containers-storage. Both live on the same filesystem as the repo, so the
   "temporary copy" is two copies, not one — and the failure is in the staging
   directory, before `/composefs` is touched at all.
2. **`du` on the ostree repo under-reports what lands.** The repo is
   content-addressed and deduplicated; the containers-storage extraction is
   not.

Measured, read from the host's sparse image file: 13 GB → **43 GB**, i.e. the
guest wrote **~28 GB onto a 43 GiB root before ENOSPC, with `/composefs` still
empty**. The estimate said 24 GiB *total*, and the largest single consumer had
not started. That is enough to show the multiplier is wrong without knowing
what the true peak is.

A caution for whoever re-derives it: **the retry's host-image growth is not the
peak.** Re-running on the same sparse file, after growing p3, counts blocks the
failed attempt had already allocated and freed as fresh growth — that file
reached 55 GB and the number means nothing. A real peak has to be sampled
inside the guest, during a migration, on a disk that has not been used for one
before. The fix then has to move three things together: `root_need` in
`cmd_precheck`, the `tests/test-boot-migrate.sh` assertion that pins the
literal `root_need=$(( repo_kib \* 2`, and the "It roughly doubles the image's
footprint on disk" bullet in `docs/update-cost.md`, which is where the 2x came
from as an argument rather than a measurement.

### The containment held, which is the other half of the result

After a hard failure in the middle of the migration:

```
phase:        not started
boot entry:   none
BootOrder:    000A,0000,0001,0002,0003,0004,0005,0006,0007,0008,0009   (unchanged)
BootCurrent:  000A   -> \EFI\fedora\shimx64.efi                        (unchanged)
/dev/vda2     2.0G  7.6M  2.0G   1%      the ESP: untouched, no loader/entries
/sysroot/boot boot bootupd-state.json efi grub2 loader loader.1 ostree ostree-1.conf
```

`auto` returned **1**, not 10 — a failure, not a refusal — so
`apexd/apex/src/ops.rs` treats it as an error rather than as "carry on", which
is the correct direction. The removable-media fallback had already been saved
before anything was touched. Nothing needed repairing and re-running was safe.

---

## 4. The install succeeds, and then the migration dies joining `/var`

With the root grown to 88 GiB — the ENOSPC defect of §3 recorded rather than
engineered around — the same `auto` runs the whole install successfully:

```
OK      root-space    73 GiB free, 24 GiB needed
apex-boot-migrate: keeping the removable-media fallback an earlier run saved
apex-boot-migrate: migrating to the image this machine is already running:
    localhost/apex-sdmig:v1 (sha256:629303a3…)
  Copying local image docker://localhost/apex-sdmig:v1 to containers-storage:localhost/bootc ...
  Pushed: containers-storage:localhost/bootc sha256:f0ac1bec…
apex-boot-migrate: carrying 6 kernel arguments across: quiet splash
    rd.driver.blacklist=nouveau modprobe.blacklist=nouveau
    console=ttyS0,115200n8 systemd.journald.forward_to_console=1
apex-boot-migrate: installing the composefs deployment (the old path is not touched)
Bootloader: systemd
Installing bootloader via systemd-boot
Installation complete!
apex-boot-migrate: restored /EFI/BOOT/BOOTX64.EFI to what it was
```

and then:

```
apex-boot-migrate: cannot join /var:
    /sysroot/ostree/deploy/default/var is not a directory and not a symlink
apex-boot-migrate: FAILED [state-join]
auto rc=1
```

**The message blames the wrong path.** Loop-mounting the guest's root shows
`/sysroot/ostree/deploy/default/var` is a perfectly ordinary directory. The
path that failed the test is the other one. bootc's composefs layout has **two
things called `var` exactly three levels under `$SYSROOT/state`**:

```
state/os/default/var                                  drwxr-xr-x   the real directory
state/deploy/5a049b1c…/var -> ../../os/default/var    lrwxrwxrwx   a symlink to it
```

and `join_state` selected with

```
newvar="$(find "$SYSROOT/state" -mindepth 3 -maxdepth 3 -name var | head -1)"
```

— no type filter. `find` walks in **readdir order, not sorted**, so `head -1`
returns whichever the filesystem hands back first. Here it returned the
symlink, `[ ! -L "$newvar" ]` was false, the `elif` fell through, and the else
branch printed a message about a completely different path.

**The migration's success depended on readdir order.** The predecessor's ext4
`fedora-bootc:43` guest got the directory first, so ~30 guest boots of lab work
across two rounds never saw this. Same code, same bootc, different filesystem,
opposite outcome. The `etc` lookup thirty lines below has carried `-type d`
since it was written; this one did not.

Fixed on this branch: `-type d` on the lookup, and a failure message that names
which of the two paths failed. Three behavioural assertions cover it, and the
expression they test is extracted from the source rather than copied, so a copy
cannot keep passing after the source drifts. Verified to fail both ways —
reverting only `-type d` turns two of them red and names the symlink it
wrongly selected.

### What the failure left behind, and the `:ro` confirmed

The stage aborts before `phase` is written, so nothing was committed: `phase:
not started`, `boot entry: none`, `BootOrder` unchanged. But the install had
already run, so the ESP now carries the real migrated entry, and it answers a
question this card had only inferred:

```
$ cat /loader/entries/bootc_fedora-43-1.conf
title APEX-OS
version 43
linux  /EFI/Linux/bootc_composefs-5a049b1c…/vmlinuz
initrd /EFI/Linux/bootc_composefs-5a049b1c…/initrd
options root=UUID=ca9e0f69-… rootflags=subvol=/ rd.driver.blacklist=nouveau rw
        boot=UUID=8A5D-0AB8 quiet splash modprobe.blacklist=nouveau
        console=ttyS0,115200n8 systemd.journald.forward_to_console=1
        composefs=5a049b1c…
        systemd.mount-extra=UUID=8A5D-0AB8:/boot:auto:ro
```

Three things read straight off it:

* **`systemd.mount-extra=…:/boot:auto:ro` is real on the APEX/btrfs path**, not
  an artefact of the predecessor's ext4 guest. On the composefs path the ESP
  *is* `/boot`, and stripping a `+N-M` boot counter is a rename on that
  filesystem — so a read-only `/boot` is a live hazard for the entry-rename
  question, and it is what §5 has to measure. For contrast, an ordinary
  (non-migrated) composefs machine was measured at `/dev/vda2 /boot vfat
  **rw**,nosuid,nodev,noexec,relatime`
  (`ROADMAP/evidence/sdboot-image-20260920-decision.md:45`).
* **`boot=UUID=8A5D-0AB8` names the real ESP**, which is the staging-UUID
  defect the predecessor found, still fixed.
* **`rootflags=subvol=/` carried across.** That karg is btrfs-only and appears
  on this guest because `to-filesystem` put it there; `carried_kargs` passed it
  through untouched, which is the correct behaviour and had never been
  exercised.

The entry is `bootc_fedora-43-1.conf` — **uncounted**. `count_staged_entry`,
the uncommitted splice item 3 turns on, runs *after* `join_state` in
`cmd_stage`, so this run never reached it.
