# Validation record — 2026-09-21

Branch `task/windows-installer-2`, worktree from `origin/roadmap/v2.2` at
`602a8376`. `git fetch origin roadmap/v2.2` succeeded this time; the base is
the current remote tip. No push to `roadmap/v2.2`, no PR.

The previous record (2026-09-20) ended with: *"No Windows disk enumeration,
volume ownership/lock validation, GUI, destructive confirmation, payload
deployment, additive bootloader transaction or undo is implemented."* Four of
those seven are now implemented and measured on a real Windows. Three are not.
This file says which is which and how each claim was checked.

---

## The Windows machine

There is one. `windows-installer/lab/winlab` builds it end to end:

| artefact | what it is |
| --- | --- |
| `ws2022-eval.iso` | Windows Server 2022 Evaluation, **5044094976 bytes**, from Microsoft's own `https://go.microsoft.com/fwlink/p/?LinkID=2195280`. Size checked against that constant on every fetch. |
| `apex-winsetup.iso` | the same media re-authored: `autounattend.xml` and `apexlab-agent.ps1` in the root, EFI El Torito image replaced with `efisys_noprompt.bin`, `install.wim` split into `install.swm` + `install2.swm`. |
| `golden.raw` + `golden-VARS.fd` | Windows Server 2022 Standard (Core), installed **headlessly in 3 qemu phases, about 150 seconds**. |
| `fixture-a.raw`, `fixture-b.raw` | the target disks. |

Its partition table, read by `sfdisk` from outside the guest:

```
1 : start=2048,   size=204800,   type=C12A7328-…  name="EFI system partition"
2 : start=206848, size=32768,    type=E3C9E316-…  name="Microsoft reserved partition"
3 : start=239616, size=83644416, type=EBD0A0A2-…  name="Basic data partition"
```

Its firmware variables after Setup, read by `virt-fw-vars` from outside the
guest — the before-baseline for every claim about the Windows entry:

```
Boot0005  title="Windows Boot Manager" devpath=Partition(nr=1)/FilePath(\EFI\Microsoft\Boot\bootmgfw.efi)
BootOrder 0005, 0003, 0000, 0001, 0004
```

Fixtures, on two different buses so the enumeration order can be changed:

```
fixture-a  AHCI, model APEX-FIXTURE-A, serial FIXA00000001
   1  17 GiB  Linux filesystem type, every byte zero      "APEX-TARGET-A"
   2   1 GiB  basic data, real NTFS, label WINDATA        "Windows data"
   3  17 GiB  basic data, every byte zero                 "Blank basic"
fixture-b  NVMe, serial FIXB00000002
   1  17 GiB  Linux filesystem type, every byte zero      "APEX-TARGET-B"
```

17 GiB and not something convenient, because the tool refuses anything under
16 decimal GB — the number `installer/apex-install` refuses too. Fixtures small
enough to be quick would have exercised every rule except the one that fires in
real life. The images are sparse.

---

## Executed, on Linux

- `cargo build --offline --locked` and `cargo test --offline --locked`:
  **14 unit tests pass**, up from 3. The new ones are in `src/plan.rs` and
  cover the eligibility rules and the confirmation text, including:
  - a partition Windows is using is refused **before** type and size are
    considered, and the refusal names the mount point;
  - every Windows-owned type is refused even when large and entirely zero;
  - the basic-data refusal carries a `diskpart set id=` remedy and states that
    the program will not retype a partition itself;
  - the confirmation text contains model, serial and partition GUID and
    **contains no device index** — the load-bearing negative;
  - everything the program prints is ASCII.
- `windows-installer/build-windows.sh`: produces
  `PE32+ executable for MS Windows 5.02 (console), x86-64`.
- `tests/test-windows-installer.sh` gained a source scan for write APIs
  (`GENERIC_WRITE`, `WriteFile`, `SetFirmwareEnvironmentVariable`,
  `FSCTL_DISMOUNT_VOLUME`, `IOCTL_DISK_SET_DRIVE_LAYOUT`, `DeleteFile`,
  `MoveFile`, `CreateDirectory`) — none appear.

## Executed, in the Windows guest

`winlab run jobs/survey`, as `NT AUTHORITY\SYSTEM` on
`Microsoft Windows Server 2022 Standard Evaluation`, against four GPT disks and
eight partitions. Verbatim from the guest's own output:

- **Both readings of every partition table agreed.** Windows'
  `IOCTL_DISK_GET_DRIVE_LAYOUT_EX` and this crate's own GPT parse of the same
  handle: `AGREE` on all four disks, `DISAGREE` on none.
- **The ESP was refused** — `in use by Windows`, `no drive letter or mount
  point -- FAT32, label "SYSTEM"`.
- **The Microsoft reserved partition was refused** — `protected partition
  type`.
- **C: was refused** — `in use by Windows … mounted at C:\ -- NTFS, label
  "Windows"`.
- **The NTFS fixture partition was refused** and the letter named — `mounted at
  E:\ -- NTFS, label "WINDATA"`.
- **The zeroed basic-data fixture was refused** — and for a stronger reason
  than its type: `mounted at F:\ -- unrecognised filesystem`. Windows had
  already given a RAW basic-data partition a drive letter. That is the design's
  claim about basic-data partitions, demonstrated rather than argued.
- **The eligible partition was read to the last byte**:
  `lock  no volume object covers this partition, so Windows has not mounted it
  and there is nothing to lock`, then
  `ALL-ZERO CONTENT: 18253611008/18253611008 bytes read.` — 17 GiB, through a
  `\\.\PhysicalDriveN` handle, not an image file.
- **The confirmation named the disk by `FIXA00000001`** and contains no
  `PhysicalDrive` and no disk number.
- **The firmware variables were byte-identical before and after**, compared by
  `virt-fw-vars` outside the guest: `IDENTICAL — no boot entry and no boot
  order changed`.

### Three defects the guest found that nothing else could have

1. **The protective-MBR check refused every Windows-formatted disk.** Windows
   writes `SizeInLBA` = `0xFFFFFFFF` unconditionally; measured on a 40 GB disk
   where the correct `0x04FFFFFF` fits easily. UEFI 2.10 permits both.
2. **The GPT geometry check refused every `sfdisk`-made disk**, because it
   required first-usable-LBA to be exactly 34. `sfdisk` aligns to 1 MiB and
   writes 2048. UEFI 2.10 §5.3.2 constrains these to a range.
3. **`PARTITION_INFORMATION_EX.Name` was read at offset 104 instead of 72.** It
   did not crash: it returned `"tion"` for `"EFI system partition"`.

And one the guest could not have reported any other way: a fresh Windows Server
console is not UTF-8, and every em dash in the program's output arrived as
`???`. All printed strings are now ASCII, with a unit test to keep them so.

---

## What is still NOT implemented, and must not be assumed

- **No installation.** No payload is written anywhere. `inspect` ends with
  `INSTALLATION IS NOT IMPLEMENTED IN THIS BUILD.`
- **No bootloader transaction, no firmware write, no undo.** The design for all
  three is in `ARCHITECTURE.md`; none of it is code.
- **No GUI.** The front end is a console program. The confirmation text exists
  and is unit-tested; the screen that shows it does not.
- **No destructive confirmation prompt.** `inspect` prints the text and stops.
  Nothing asks for consent because nothing would act on it.
- **`FSCTL_LOCK_VOLUME` has never succeeded**, because every partition that got
  as far as the lock step had no volume object for Windows to lock. The code
  path that takes a lock on a volume that *does* exist has therefore not been
  exercised — only the path that refuses when the lock fails, and the path that
  reports honestly that there was nothing to lock.
- **The build is the GNU target, not MSVC, and is unsigned.** No Authenticode,
  no static-CRT MSVC build, no supply-chain review.
- **No 4Kn disk, no BitLocker, no Storage Spaces, no dynamic disk, no hot
  unplug, no removable media** has been tested. The enumerator refuses
  `BytesPerSector` it does not expect only in the sense that the GPT reader is
  512-byte-sector-only; a 4Kn disk has not been put in front of it.
- **No physical disk, anywhere, ever.** Every disk in this record is a file
  under `/var/lab-scratch/winlab/`.

## Before this is pointed at a real machine

In addition to the review gates in `README.md`:

1. A human must read `ARCHITECTURE.md` and agree with the route, because the
   whole design follows from it.
2. The ESP tension has to be resolved as a product decision, not by this unit:
   the composefs path needs ~1.1 GiB of ESP and a stock Windows ESP has
   **68.3 MiB free, measured**.
3. The basic-data refusal has to be resolved as a product decision: it refuses
   exactly the partition a user produces by shrinking C:.
4. Everything in the "still NOT implemented" list above has to exist and be
   tested in the guest, at every write boundary, with fault injection.
