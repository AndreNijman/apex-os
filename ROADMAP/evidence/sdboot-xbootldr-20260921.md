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
| **`ghcr.io/andrenijman/apex-os:daily` — the one that runs the migration** | **1.16.4** (`podman run --rm … bootc --version`) |
| `sdboot-migrate`'s lab guest | 1.16.13 |
| upstream `main` | `65321ae`, fetched 2026-09-21 |

All four behave identically. Sources at
`/var/lab-scratch/sdboot-xbootldr/bootc-src` (tag `v1.16.10`) and
`/var/lab-scratch/sdboot-xbootldr/bootc-main` (`main`, and `v1.16.4` checked
out into it).

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

## 5. Lab: the L16-shaped guest

Nothing in this unit touched the L16's partitions, ESP, NVRAM or bootloader.
The only read of the machine was `stat` on two files under `/usr/lib/modules`
and `lsblk`. The APEX guest in section 2a was mounted **read-only** from an
image file `sdboot-image` left behind.
