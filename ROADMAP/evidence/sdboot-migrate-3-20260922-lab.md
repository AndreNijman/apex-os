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
`EFI/fedora` + `EFI/BOOT` on it. **1173 MiB** is two deployments of APEX's
kernel + 375 MiB initramfs plus the loader and 48 MiB of slack. A 512 MiB ESP —
which is what `bootc install to-disk` gives an APEX machine by default, checked
on this disk's own partition table rather than assumed — is short by two thirds
of what it needs.

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
