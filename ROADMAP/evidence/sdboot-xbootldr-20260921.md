# Type #1 entries on an XBOOTLDR partition — what bootc actually does, 2026-09-21

Unit `sdboot-xbootldr`. The question it was dispatched to answer:

> The migration refuses on the L16 with `esp-too-small`: 600 MiB against
> ~1.1 GiB. **Type #1 boot entries** put the kernel and initramfs on an
> **XBOOTLDR partition**, leaving the ESP holding only `systemd-bootx64.efi`
> (~100 KB). The L16 already has two unused 2 GiB XBOOTLDR partitions. Does
> `bootc install --composefs-backend --bootloader systemd` support Type #1
> entries on an XBOOTLDR partition at all?

**No.** And the premise underneath the question is wrong in two ways that
matter more than the answer. Both corrections are below, from primary source
and from a measured ESP.

---

## 1. The answer, from bootc's own source, at four independent sites

Checked against **four** bootc revisions, because the one that matters on a real
APEX machine is not the one on the developer's host:

| where | bootc version |
| --- | --- |
| the L16 host | 1.16.10-1.fc43 (`rpm -q bootc`) |
| **`ghcr.io/andrenijman/apex-os:daily` — the one that runs the migration** | **1.16.11**, read out of a booted guest installed from a fresh pull |
| `sdboot-migrate`'s lab guest | 1.16.13 |
| upstream `main` | `65321ae`, fetched 2026-09-21 |

All four behave identically.

> **A trap, because it nearly went into this document as a fact.**
> `podman run --rm ghcr.io/andrenijman/apex-os:daily bootc --version` answered
> **1.16.4**, and that number is wrong. Rootless and root podman keep separate
> storages, and `podman run` on a registry reference uses whatever local copy
> exists without asking the registry. The rootless copy on this machine is
> **7 weeks old**; root's, pulled for the install, is 6 days old:
>
> ```
> $ podman images | grep apex-os:daily
> ghcr.io/andrenijman/apex-os:daily   7 weeks ago   a20b0b04f958
> $ sudo podman images | grep apex-os:daily
> ghcr.io/andrenijman/apex-os:daily   6 days ago    1d21fb12c5de
> ```
>
> The authority for "what ships in the image" is the **booted guest**, which
> says 1.16.11. 1.16.4 is kept in the list below anyway — it was read from
> source and is identical, so the conclusion held despite the stale read. Sources at `/var/lab-scratch/sdboot-xbootldr/bootc-src` (tag `v1.16.10`) and
`/var/lab-scratch/sdboot-xbootldr/bootc-main` (`main`, with `v1.16.4` and
`v1.16.11` also checked out into it). In **v1.16.11** the same three lines are
at `boot.rs:793` (`mount_esp_writable`), `boot.rs:802`
(`abs_entries_path = /EFI/Linux`), `boot.rs:1455` (the TODO) and
`store/mod.rs:396` (the NOTE).

### 1a. `--bootloader systemd` maps to `BLSCompatible`

`crates/lib/src/spec.rs:288-291`

```rust
pub(crate) fn kind(&self) -> Result<BootloaderKind> {
    match ... {
        Bootloader::Grub => Ok(BootloaderKind::GRUBClassic),
        Bootloader::Systemd | Bootloader::GrubCC => Ok(BootloaderKind::BLSCompatible),
```

### 1b. The BLS install path mounts the ESP and roots the entry paths there

`crates/lib/src/bootc_composefs/boot.rs:751-789`, inside
`setup_composefs_bls_boot`. The two arms are not symmetric, and that asymmetry
is the whole finding:

```rust
BootloaderKind::GRUBClassic => {
    // If "boot" is a partition, we want the paths to be absolute to "/"
    let entries_path = match root.is_mountpoint("boot")? {
        Some(true) => "/",
        Some(false) | None => "/boot",
    };
    ...
}

BootloaderKind::BLSCompatible => {
    let efi_mount = mount_esp_writable(&esp_device).context("Mounting ESP")?;
    let mounted_efi = Utf8PathBuf::from(efi_mount.dir.path().as_str()?);
    let efi_linux_dir = mounted_efi.join(EFI_LINUX);
    BLSEntryPath {
        entries_path: efi_linux_dir,                              // <ESP>/EFI/Linux
        config_path:  mounted_efi.clone(),                        // <ESP>
        abs_entries_path: Utf8PathBuf::from("/").join(EFI_LINUX), // /EFI/Linux
    }
}
```

**Only the GRUB arm asks whether `/boot` is a separate partition.** The
systemd-boot arm never asks: it mounts the ESP unconditionally and writes
`vmlinuz` and `initrd` into `<ESP>/EFI/Linux/bootc_composefs-<verity>/`, the
`.conf` into `<ESP>/loader/entries/`, and the `linux=`/`initrd=` lines as paths
absolute **to the ESP**. `EFI_LINUX` is `"EFI/Linux"` (`boot.rs:124`).

### 1c. The XBOOTLDR gap is a literal TODO, in three places, still open on `main`

```
crates/lib/src/bootc_composefs/boot.rs:1422   (main: 1455)
    // TODO: support XBOOTLDR.  Per BLS, the ESP should be mounted at /efi
    // when a separate XBOOTLDR partition is present at /boot.  bootc does
    // not yet detect or use XBOOTLDR in the composefs install path, so
    // unconditionally mount the ESP at /boot for now.
    let esp_subdir = "boot";

crates/lib/src/bootloader.rs:287
    let mut bootctl_args = vec![
        "install", "--root", root_path, "--esp-path", esp_path_in_root.as_str(),
        // If we supported XBOOTLDR in the future, that'd go here with --boot-path.
    ];

crates/lib/src/store/mod.rs:392-398     (v1.16.4: 336-341)
    let boot_dir = match get_bootloader()?.kind()? {
        BootloaderKind::GRUBClassic => physical_root.open_dir("boot")?,
        // NOTE: Handle XBOOTLDR partitions here if and when we use it
        BootloaderKind::BLSCompatible => esp_mount.fd.try_clone()?,
    };
```

The third one is the important one for anybody tempted by a workaround: it is
the **runtime** store, used by every `bootc upgrade`, `bootc status`,
`bootc rollback` and `bootc image delete` afterwards — not just install.

A fourth, for anybody hoping to ask for one in `/usr/lib/bootc/install/*.toml`
instead — `crates/lib/src/install/config.rs:74-79`:

```rust
#[serde(deny_unknown_fields)]
pub(crate) struct BasicFilesystems {
    pub(crate) root: Option<RootFS>,
    // TODO allow configuration of these other filesystems too
    // pub(crate) xbootldr: Option<FilesystemCustomization>,
    // pub(crate) esp: Option<FilesystemCustomization>,
}
```

Both are commented out, and `deny_unknown_fields` means a config that names
either is **rejected**, not ignored. There is no way to ask for an XBOOTLDR and
no way to ask for a bigger ESP.

### 1c-bis. systemd-boot *does* support XBOOTLDR — measured on the binary bootc installed

The idea is not wrong about the bootloader. Pulled out of the very
`systemd-bootx64.efi` that `bootc install --bootloader systemd` wrote onto the
APEX guest's ESP (`/var/lab-scratch/sdboot-lab/apex.img`, copied to
`/var/lab-scratch/sdboot-xbootldr/sdboot.efi`, 135 KiB):

```
$ strings -a -n 5 sdboot.efi | grep -i xbootldr
config_load_xbootldr
$ strings -a -n 4 /usr/bin/bootc | grep -c -i 'xbootldr\|bc13c2ff'
0
```

and `bootctl` carries `--boot-path=PATH` and `-x --print-boot-path` for exactly
this. So the loader on the ESP would happily read Type #1 entries off an
XBOOTLDR partition. **Nothing ever puts any there.** The actor is bootc's
composefs backend, not systemd-boot — which is why "systemd-boot does not
require kernels in the ESP" is a true sentence that does not help.

### 1d. Upstream says it in words, in a merged PR, five days ago

bootc PR **#2440**, *"cfs/boot: Handle separate /boot mount"*, merged
2026-09-16, closing issue #2399. From the PR body:

> If /boot is mounted as XBOOTLDR partition then grub configs are stored there
> as we were unconditionally searching for grub configs in `/sysroot/boot` …
> **This is only an issue with Grub as GrubCC and SystemdBoot both do not store
> anything inside of `/sysroot/boot` and will (should) always have the ESP
> mounted at /boot.**

So the separate-`/boot` handling that *does* exist upstream was added for GRUB
and deliberately not for systemd-boot. A GitHub search of `bootc-dev/bootc` for
`xbootldr` returns 9 results and none of them is an open issue or PR to support
it on the composefs/systemd-boot path.

### 1e. What "support XBOOTLDR" would cost upstream

Six sites key off the same `boot_dir`/`esp_subdir` decision, so this is not a
one-line patch:

| file:line | what it decides |
| --- | --- |
| `bootc_composefs/boot.rs:775` | where install writes vmlinuz/initrd and the `.conf` |
| `bootc_composefs/boot.rs:1422` | `esp_subdir`, what `bootctl --root` sees |
| `bootloader.rs:287` | `bootctl install --boot-path` |
| `store/mod.rs:396` | the runtime `boot_dir` every later command uses |
| `bootc_composefs/status.rs:408` | `list_type1_entries(boot_dir)` |
| `bootc_composefs/finalize.rs:149`, `delete.rs:159` | staged-entry rename, deployment deletion |

**Not this unit's to attempt, and not APEX's to carry as a fork.**

### 1f. A note on how the previous round got this right for the wrong reason

`docs/boot-v2.md` said: *"`strings` on the `bootc` binary contains **zero**
occurrences of `xbootldr` or the XBOOTLDR type GUID."* That is true — verified
again here, both counts are 0 — and it is **not** evidence. The source *does*
define the constant:

```
crates/lib/src/discoverable_partition_specification.rs:482
    pub const XBOOTLDR: &str = "bc13c2ff-59e6-4262-a352-b275fd6f7172";
```

It is absent from the binary only because nothing on any reachable path
references it, which is a conclusion you can only draw after reading the source.
An unused `&str` const being optimised out is indistinguishable from a feature
being present under another name. Same family as *"permission denied is not
absence"*. The doc has been corrected to cite the call sites.

---

## 2. The premise was wrong twice, and the second one is the expensive one

### 2a. APEX is *already* on Type #1. There is nothing to switch to.

`BootType::Bls` is `#[default]`, and an image that ships a plain kernel under
`/usr/lib/modules` gets it (`boot.rs:253-289`):

```rust
pub enum BootType { #[default] Bls, Uki }
...
ComposefsBootEntry::Type1(..)              => Self::Bls,
ComposefsBootEntry::Type2(..)              => Self::Uki,
ComposefsBootEntry::UsrLibModulesVmLinuz(..) => Self::Bls,
```

`docs/boot-v2.md` already calls this **phase 1 — unsealed Type #1 entries**, and
`apex-boot-migrate` already measures `vmlinuz` + `initramfs.img` under
`/usr/lib/modules`, not a UKI (`deployment_kernel_bytes`, line 225).

Measured on an APEX composefs guest that `sdboot-image` installed on 2026-09-20
and left at `/var/lab-scratch/sdboot-lab/apex.img` — **the real APEX image, on
btrfs**, mounted read-only here:

```
/dev/loop1p3: LABEL="root" TYPE="btrfs"
/dev/loop1p2: LABEL="EFI-SYSTEM" TYPE="vfat"     1022 MiB, 376 MiB used (37%)

<ESP>/EFI/systemd/systemd-bootx64.efi                    136 KiB
<ESP>/EFI/BOOT/BOOTX64.EFI                               136 KiB
<ESP>/loader/                                             28 KiB
<ESP>/loader/entries.srel                              -> "type1"
<ESP>/loader/entries/bootc_fedora-43-1+2-1.conf
<ESP>/EFI/Linux/bootc_composefs-085d092a…1491/vmlinuz     16 922 688 B
<ESP>/EFI/Linux/bootc_composefs-085d092a…1491/initrd     376 918 577 B
```

and the entry itself:

```
title APEX-OS
version 43
linux  /EFI/Linux/bootc_composefs-085d092a…1491/vmlinuz
initrd /EFI/Linux/bootc_composefs-085d092a…1491/initrd
options rw console=ttyS0,115200n8 … composefs=085d092a…1491 quiet splash …
sort-key bootc-fedora-0
```

That is a Type #1 entry, on the ESP, with its `linux=` and `initrd=` paths
absolute to the ESP. Per the Boot Loader Specification those paths would be
absolute to the **XBOOTLDR** partition if one were in use, and the `.conf` would
live on XBOOTLDR too. bootc writes neither.

**The bootloader is 300 KiB of the 376 MiB.** The orchestrator's arithmetic that
sd-boot alone would fit in a small ESP is correct — and irrelevant, because
bootc puts the other 375.6 MiB next to it.

### 2b. It is three deployments, not two, and they are not UKIs

`apex-boot-migrate` line 218-232 and 318-322:

```
per  = vmlinuz + initramfs.img under /usr/lib/modules
need = per * 3 + 48 MiB slack
```

Measured on the L16 itself, read-only, 2026-09-21:

```
/usr/lib/modules/7.2.3-cachyos2.fc43.x86_64/vmlinuz + initramfs.img
  = 392 456 766 B = 374 MiB per deployment
need = 374 * 3 + 48 = 1170 MiB
```

`sdboot-migrate`'s comment says why three: booted + rollback + the third an
update stages alongside them. The 1.1 GiB is **3 × (vmlinuz + initramfs)**, not
"two 374 MiB UKIs". Switching to Type #1 moves none of it, because it is already
Type #1. The only lever that would move it is *which partition those files live
on*, and bootc holds that lever.

### 2c. Upstream's own ESP default is under APEX's peak

`crates/lib/src/install/baseline.rs:62-68`

```rust
pub(crate) const EFIPN_SIZE_MB: u32 = 512;
/// EFI Partition size for composefs installations
/// We need more space than ostree as we have UKIs and UKI addons
/// We might also need to store UKIs for pinned deployments
pub(crate) const CFS_EFIPN_SIZE_MB: u32 = 1024;
```

Not configurable by any flag — `bootc install to-disk --help` offers
`--root-size` and nothing for the ESP. Upstream's sizing model explicitly
assumes the boot artefacts are ESP-resident, and its 1024 MiB is **below**
APEX's 1170 MiB steady-state-plus-update peak. The `apex.img` guest above is the
proof: 1022 MiB of ESP, 37% consumed by **one** deployment, room for a second
and not a third.

One more thing from the same file, worth knowing before someone assumes bootc
can be coaxed into an XBOOTLDR by asking for one:

```rust
/// Returns true if the block setup requires a separate /boot aka XBOOTLDR partition.
pub(crate) fn requires_bootpart(&self) -> bool {
    match self { BlockSetup::Direct => false, BlockSetup::Tpm2Luks => true }
}
...
writeln!(&mut partitioning_buf, r#"size={BOOTPN_SIZE_MB}MiB, name="boot""#)?;
```

bootc *will* create a separate `/boot` partition — only for `tpm2-luks`, only
510 MiB, and the sfdisk line carries **no `type=`**, so it is created as a plain
Linux filesystem partition, not `bc13c2ff-…`. Even bootc's own "XBOOTLDR" is not
XBOOTLDR-typed, and the systemd-boot path ignores it regardless.

---

## 3. What Secure Boot costs in Type #1 mode: nothing that XBOOTLDR would change

The delta the orchestrator asked about — *"Type #1 means no sd-stub, so no
`.pcrsig` and no measurement into PCR 11"* — is real, but it is the delta
between **Type #1 and UKI**, not between **ESP and XBOOTLDR**. Same files, same
signatures, different partition. Moving them to XBOOTLDR would cost and save
exactly zero Secure Boot.

And APEX already pays that delta: phase 1 *is* Type #1, today, in the image.
`docs/boot-v2.md`, "Two phases, and which one ships first". So the answer to
"what does Secure Boot cost here" is **nothing new**.

What is separately true and unchanged:

* **`secure-boot-unsigned-loader` is about sd-boot's own binary**, not about
  where the kernel lives. `bootc` lays down `systemd-bootx64.efi` and
  `BOOTX64.EFI` from `systemd-boot-unsigned`, with no shim and no `/EFI/fedora`;
  on a machine whose `db` holds only the Microsoft CA neither validates. That
  refusal fires on the L16 today and an XBOOTLDR partition would not have
  touched it. The decision it waits on is `docs/boot-v2.md`, "Secure Boot: a
  decision, not a measurement".
* **The kernel itself can be signed and verified** — `kernel-build` (merge
  `74b777ac`) established `vmlinuz` is a real `pei-x86-64` PE with
  `CONFIG_EFI_STUB=y`, that `sbsign` runs on it and `sbverify` confirms. sd-boot
  loading a signed kernel through `LoadImage` is validated by the firmware
  wherever the file sits: ESP or XBOOTLDR makes no difference to that check.
* **The initrd is outside the signed PE either way in phase 1.** sd-boot hands
  it over through the initrd media protocol; nothing verifies it. That is a
  Type #1 property, not an XBOOTLDR property.
* **PCR 11 is not usable on a shipped APEX machine anyway** — the `luks-enroll`
  unit established that. So the measurement the orchestrator was worried about
  losing is one APEX cannot spend today.

---

## 3b. The design, since XBOOTLDR is not on the menu

The dispatch asked for "Type #1 on XBOOTLDR as the universal baseline, with UKIs
as an optional upgrade where the ESP is genuinely large". With XBOOTLDR gone,
the two modes that exist are Type #1 on the ESP and UKI on the ESP, and the
honest statement is:

| | Type #1 (phase 1, today) | UKI (phase 2) |
| --- | --- | --- |
| what is on the ESP | `vmlinuz` + `initrd` + a `.conf` | one signed PE |
| **ESP cost per deployment** | **375.6 MiB** (measured) | **~376.5 MiB** — stub, PE headers, `.osrel`/`.cmdline` sections on top of the same kernel and initramfs |
| kernel signature | firmware validates the kernel PE through `LoadImage` | firmware validates the UKI |
| initramfs | outside the signature; sd-boot hands it over unverified | inside the signature |
| kargs | the `options` line works | frozen in the PE; per-machine values need a credential |
| PCR 11 / `.pcrsig` | none | yes — and `luks-enroll` established PCR 11 is unusable on a shipped APEX machine today |

**A UKI is not the answer to the ESP problem either.** It is the same kernel and
the same initramfs in one file; the stub and the section headers make it
marginally *larger*, not smaller. UKI vs Type #1 is a signing and measurement
decision, and `docs/boot-v2.md` already frames it that way in "Two phases, and
which one ships first". Nothing in this unit's finding changes which phase ships
first.

### The one lever inside bootc that does reduce the ESP cost

`find_vmlinuz_initrd_duplicate` (`boot.rs:533`). Before writing a new
deployment's kernel and initrd, bootc computes their combined sha256 and scans
the existing `bootc_composefs-*` directories on the ESP; on a match it points
the new `.conf` at the **existing** directory and writes nothing:

```rust
// Multiple deployments could be using the same kernel + initrd, but there
// would be only one available
//
// Symlinking directories themselves would be better, but vfat does not support
// symlinks
```

So an image update that leaves `vmlinuz` and `initramfs.img` **byte-identical**
costs **zero** additional ESP, and `apex-boot-migrate`'s `per * 3` is a worst
case rather than a steady state.

`sdboot-migrate` already measured the mechanism itself working both ways —
`docs/boot-v2.md`, "What boots through systemd-boot today": *"dedupes the ESP
directory when kernel+initramfs are unchanged, writes a second one when the
initramfs moves."* So the question is not whether the code works; it is
**whether an APEX image update ever leaves the initramfs byte-identical**, and
that is unmeasured. The kernel comes from `kernel-build` and is stable across
image builds, but the initramfs is regenerated per build and dracut output is
not byte-reproducible by default. If it were made reproducible, most updates
would stop costing ESP at all — which would matter far more than any partition
layout. Nobody has checked. It is written down here so somebody does.

## 4. Verdicts

### Does the L16 still refuse, and why

Yes, for both of the reasons `sdboot-migrate` recorded, and the XBOOTLDR idea
removes neither:

| refusal | still fires | why XBOOTLDR does not help |
| --- | --- | --- |
| `esp-too-small` | yes — 600 MiB ESP, 1170 MiB needed | bootc writes all 3 × 374 MiB to the ESP; it never enumerates `bc13c2ff-…` |
| `secure-boot-unsigned-loader` | yes — Secure Boot is on, sd-boot is unsigned | orthogonal: about the loader binary, not the kernel's partition |

The L16's `p2` (2 GiB, ext4, `EA00`, unmounted) and `p4` (`apex-newboot`, same)
stay exactly as useless to this path as they are today. What *would* move the
refusal, neither of which is this unit's:

1. **Shrink the 359 MiB initramfs** (`kernel-build` owns it). Every MiB is
   worth three on the ESP.
2. **Delete the dead `p2` and grow `p1`** — 600 MiB + 2 GiB is far past 1170
   MiB, and `p2` sits immediately after the ESP so the growth is contiguous.
   Andre's decision on his own machine; already `sdboot-migrate`'s NEXT #2.

### What a Windows dual-boot machine gets

Nothing changes, and the fork `windows-installer` reported stands. That unit
measured a stock shared Windows ESP at **96 MiB usable, 27.7 MiB spent, 68.3 MiB
free**. systemd-boot's own 136 KiB fits; the 374 MiB per deployment does not,
and cannot be diverted to an XBOOTLDR partition the installer creates, because
bootc will not look at it. So a Windows dual-boot install still needs either a
second, APEX-owned ESP of ~1.25 GiB, or it stays on GRUB.

A caveat for whoever takes the second-ESP route: bootc picks the ESP with
`find_first_colocated_esp()` — first ESP found walking up to the root disk — and
`apex-boot-migrate`'s `find_esp` picks by GPT type GUID on the disk the root is
on. Neither is "the one you meant" on a two-ESP machine. Same family as
`sdboot-migrate`'s NEXT #5 about katana, whose APEX `Boot0000` lives on the
*Windows* disk's ESP.

---

## 5. katana, and the ESP the machine actually boots from

Andre added a requirement mid-unit: **the migration must work on katana too.**
katana's measured layout (by serial, because its NVMe names have now swapped
four times):

```
nvme1n1  1.8T  serial 240023925111005 (SPCC)    — the WINDOWS disk
  p1  200 MiB  EFI System  PARTUUID 2ba9a2ea…   ← Boot0000* APEX-OS Primary
                                                  \EFI\APEX\SHIMX64.EFI, BootCurrent=0000
nvme0n1  954G  serial 220534D1CB81 (Micron)     — the APEX disk (root lives here)
  p2  512 MiB  EFI System  label EFI-SYSTEM  PARTUUID 99af3362…   ← unused
```

### The suggestion was "make the migration prefer the machine's own ESP". It already does.

The brief assumed `find_esp()` resolves the ESP the machine currently boots
from. It does not, and the file says so in a comment written for exactly this
machine:

```sh
# The ESP, by GPT type GUID on the disk the root filesystem is on. Not by
# /etc/fstab and not by label: on this machine's lineage the ESP is not mounted
# at all, and katana's APEX entry lives on the ESP Windows created.
ESP_TYPE_GUID=c12a7328-f81f-11d2-ba4b-00a0c93ec93b

find_esp() {
    disk="$(root_disk)" || return 1        # findmnt --target /sysroot, then up to a disk
    for part in $(lsblk -lno NAME "/dev/$disk"); do ...
```

bootc agrees, from its own source — `crates/blockdev/src/blockdev.rs:214`:

```rust
pub fn find_first_colocated_esp(&self) -> Result<Device> {
    self.find_colocated_esps()? ...
```

`find_colocated_esps` searches `find_all_roots()`, the disks backing the **root**
device. So the engine and bootc pick the same partition, and on katana that is
`nvme0n1p2` — the APEX disk's own unused 512 MiB `EFI-SYSTEM`, **not** the
200 MiB ESP on the Windows disk.

**Measured in the lab, in a guest built for this**: the APEX guest was booted
with a second disk carrying a 200 MiB ESP holding a stand-in Windows Boot
Manager, an `EFI/Boot/bootx64.efi` and an `EFI/APEX/SHIMX64.EFI` — katana's
shape. From inside the guest:

```
ESPs on the ROOT disk    : /dev/vda2
ESPs on OTHER disk /dev/vdb  : /dev/vdb1
  root_disk()      = vda
  find_esp()       = /dev/vda2      <- the root's own disk, with two ESPs present
  find_xbootldr()  = /dev/vda4
```

So katana's migration moves APEX's boot onto its own disk as a **side effect of
migrating at all**, which is the outcome asked for. Nothing had to change to
get it.

### The Windows ESP is untouched, asserted rather than intended

Hashed before and after the whole precheck run, in the same boot:

```
  before                                   after
  d2bf54c5…  EFI/Microsoft/Boot/bootmgfw.efi   d2bf54c5…   identical
  6b2164ba…  EFI/Boot/bootx64.efi              6b2164ba…   identical
  710d912d…  EFI/APEX/SHIMX64.EFI              710d912d…   identical
```

A precheck writes nothing anywhere, so this is a weak assertion on its own —
its value is that it is now *in* the harness, so the same three hashes can be
taken around a stage and a commit when somebody runs those. The structural
argument is the stronger one: the engine only ever mounts `find_esp()`'s
partition, and `find_esp()` cannot return a partition on a disk the root is not
on.

### What was missing, and now is not: the user is never told their boot moved disks

A machine that boots from one disk's ESP and roots on another's silently stops
depending on the first. That is the improvement, and it is also exactly the sort
of change that should not be silent. `apex-boot-migrate` now reads the PARTUUID
out of `BootCurrent`'s device path and, when it differs from the ESP it is about
to write, prints:

```
note: this machine BOOTS from the ESP at PARTUUID 2ba9a2ea-…,
      which is not on the disk its root filesystem is on. The
      migration writes /dev/nvme0n1p2 (PARTUUID 99af3362-…), the ESP on
      the root's own disk, and adds a firmware entry pointing there.
      Nothing is written to 2ba9a2ea-…, so another operating system
      installed on it keeps its own boot entry untouched. After the
      trial boot this machine no longer depends on that disk.
```

**A note, deliberately not a refusal**, against `sdboot-migrate`'s NEXT #5 which
proposed refusing. Refusing would keep katana booting off the Windows disk
forever, which is the thing worth fixing; and the failure mode a refusal would
be guarding against — firmware that cannot see the new ESP — is already handled
by the machinery that exists: the trial boot comes back to GRUB, `confirm`
records `phase=failed`, and nothing re-arms.

The parser was checked against **the L16's real firmware output**, read-only:

```
$ efibootmgr -v | grep '^Boot0000'
Boot0000* APEX-OS	HD(1,GPT,1c417de2-5766-455f-9318-198610885424,0x800,0x12c000)/\EFI\fedora\shimx64.efi

booted_esp_partuuid  = 1c417de2-5766-455f-9318-198610885424
find_esp             = /dev/nvme0n1p1
partuuid_of find_esp = 1c417de2-5766-455f-9318-198610885424   -> equal, so the note does NOT fire
```

The L16 boots from its own disk, so the negative case is measured on real
hardware. Both device-path spellings are fixtures in `test-boot-migrate` —
katana's `…/File(\EFI\APEX\SHIMX64.EFI)` and the L16's bare
`…/\EFI\fedora\shimx64.efi` — because a parser of another tool's output is
exactly where an assumption breaks quietly.

### katana still refuses today, and the number that decides it

512 MiB against 1173 MiB. The binding rule is `apex-boot-migrate`'s, and it is
**three** deployments, not two:

```
need = (vmlinuz + initramfs) x 3 + 48 MiB slack
```

Solving it for the initramfs, with `vmlinuz` at 16.1 MiB:

| machine | ESP | largest initramfs that migrates |
| --- | --- | --- |
| **katana** | 512 MiB | **≤ 138.6 MiB** |
| L16 today | 600 MiB | ≤ 168.6 MiB |
| L16 if `p2` were absorbed into `p1` | 2648 MiB | ≤ 866 MiB (not a constraint) |

**For `initramfs-slim`:** its stated target — *"slimmed toward ~120 MiB so two
deployments fit 512"* — lands correctly but is sized against the wrong rule.
Two deployments in 512 MiB would allow 215.9 MiB; the engine asks for three, so
the real budget is **138.6 MiB**. At 120 MiB, `need` = 456 MiB and katana
migrates with 56 MiB to spare. At 160 MiB — comfortably inside a "two
deployments" target — it does not. The three-deployment rule is
`apex-boot-migrate` lines 218-232 and 348-352, and `sdboot-migrate`'s own
comment explains it: the steady state is booted + rollback, and an update stages
a third alongside them. Nothing in this unit assumes the slimmer number.

---

## 6. Lab: the L16-shaped guest, and what it measured

Guest: `ghcr.io/andrenijman/apex-os:daily` — the real image — installed through
`tests/lab/bootc-install-lab` (so `--generic-image`, under `nvram-guard`) onto a
45 GiB loopback file, `--filesystem btrfs`, no `--bootloader`, so **ostree +
GRUB + bootupd**: the shape the L16 has. Then a 2 GiB `EA00` XBOOTLDR added in
the free tail left by `--root-size 40G`, and a second disk attached carrying
katana's 200 MiB Windows ESP.

```
vda  45G      p1 1M BIOS-BOOT · p2 512M vfat EFI-SYSTEM · p3 40G btrfs root · p4 2G ext4 apex-newboot (EA00)
vdb 400M      p1 200M vfat SYSTEM (ESP) · p2 199M basic data
vdc  64M      the control disk
```

Both installs returned **`verdict: verified — boot variables identical before
and after`**. Firmware read out of the binary, not the filename:
`edk2-2970e5699ba6`.

### What the guest says

```
bootc 1.16.11
root: /dev/vda3  btrfs
/usr/lib/modules/7.2.5-cachyos1.fc43.x86_64/vmlinuz         16 910 408 B
/usr/lib/modules/7.2.5-cachyos1.fc43.x86_64/initramfs.img  376 456 492 B
per-deployment = 393 366 900 bytes = 375 MiB
need (3x + 48) = 1173 MiB
ESP: 511 MiB total, 503 MiB free — EFI/BOOT/{BOOTX64.EFI,fbx64.efi},
     EFI/fedora/{shim.efi,shimx64.efi,mmx64.efi,grubx64.efi,grub.cfg,…}
XBOOTLDR /dev/vda4: 0 files
```

### The two refusals, in order, verbatim

```
--- 6. precheck, exactly as the image would run it ---
apex-boot-migrate: REFUSED [no-rsync]
    rsync is not in this image; the migration needs it to carry /etc across.
PRECHECK_RC=10
```

**`apex-os:daily` ships no `rsync`.** That is the *first* refusal on a real
APEX machine today, not `esp-too-small`, and nobody has seen it because the
migration has never run on a real image. `Containerfile.base` on `roadmap/v2.2`
asserts rsync is present, so the next image build fixes it — but until one
exists, this is what a user gets.

With a stub `rsync` on `PATH` purely to reach the next refusal — the run stages
nothing, so `rsync` is never called:

```
--- 7. precheck again with a stub rsync on PATH ---
apex-boot-migrate: REFUSED [esp-too-small]
    The EFI System Partition has 503 MiB free and this
    machine needs 1173 MiB — one deployment's kernel and
    initramfs is 375 MiB, systemd-boot keeps the
    booted one and the rollback, and an update stages a third
    alongside them.
    An ESP cannot be grown in place without moving the partition after
    it, so this machine stays on GRUB until its ESP is made bigger.
    This machine has an XBOOTLDR partition (/dev/vda4) and it cannot
    absorb this. The Boot Loader Specification allows the kernel
    and initramfs to live there; bootc's composefs backend does
    not write them there — it mounts the ESP unconditionally and
    writes the entry paths absolute to it. docs/boot-v2.md,
    'XBOOTLDR: the Boot Loader Specification allows it, bootc
    does not implement it'.
PRECHECK_RC=10
```

That is the refusal the L16 gets, on the real image, on btrfs, with an XBOOTLDR
partition present — which is the question this unit was asked. The XBOOTLDR is
still **0 files** afterwards, and the other disk's ESP still hashes identically.

### What this run did NOT do

* **It did not migrate.** Both prechecks refused, by design — the point was the
  refusal. The stage, the commit, the trial boot and the confirm are unmeasured
  on the APEX image and on btrfs; that is `sdboot-migrate-2`'s NEXT item 1, and
  `apexgrub.img` is handed to it rather than built twice.
* **It did not exercise the cross-disk note end to end.** `ctl.img` carried the
  engine as it stood before that note was written, and the guest's NVRAM was
  fresh (no `Boot0000` pointing at the second disk), so the note had nothing to
  fire on. It is covered by three assertions in `test-boot-migrate` against
  katana's and the L16's real `efibootmgr -v` shapes, and by the L16 run above.
  A guest boot that arms `Boot0000` on the second disk first would close it.
* **It did not test Secure Boot.** The OVMF build is non-secboot, so
  `secure-boot-unsigned-loader` could not fire and did not; the guest's
  `SecureBoot` efivar read is in the log for whoever needs it.

### A lab trap worth one line

The first boot died with `/bin/bash: line 1: /work/boot-mig.sh: Permission
denied` and exit 126 — not a mode bit. A freshly created directory under
`/var/lab-scratch` is `var_t`; a bind mount into a container needs
`container_file_t` (what `-v …:z` sets). `sdboot-migrate`'s lab directory
already had it, so the harness looked like it "just works". `chcon -R -t
container_file_t` on the lab directory, and the second run was clean.

---

## 7. Bounds

Nothing in this unit touched the L16's partitions, ESP, NVRAM or bootloader.
The reads of the machine were: `stat` on two files under `/usr/lib/modules`,
`lsblk`, `efibootmgr -v` (which prints variables and writes none), and
`find_esp`/`find_xbootldr`/`booted_esp_partuuid` run out of the engine. The APEX
composefs guest in section 2a was mounted **read-only** from an image file
`sdboot-image` left behind. Every install went through
`tests/lab/bootc-install-lab` under `nvram-guard`, and every run returned
`verdict: verified`. katana was not touched at all — its layout above is the
orchestrator's measurement, and the lab guest stands in for it.
