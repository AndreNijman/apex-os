# The Windows installer got a Windows

Unit `windows-installer-2`, branch `task/windows-installer-2` from
`roadmap/v2.2` @ `602a8376`. Not landed.

Andre's ask: *"I want the full completed working windows app that does the
whole stack, not the dumb barebones you made it."* This is not that app. It is
the two things that had to exist before that app could be written honestly: a
decision about what "install APEX from Windows" can even mean, and a Windows
machine to prove anything against.

---

## 1. The architectural question, answered

`bootc install` is Linux software. It opens block devices, runs `mkfs`, writes
a store through Linux filesystem drivers and relabels files with the target's
SELinux policy. So "install APEX from a Windows app" cannot mean "run the
normal installer". Full reasoning in `windows-installer/ARCHITECTURE.md`.

**Chosen: stage a payload, complete on the first boot.** Windows moves bytes;
Linux installs the operating system. The Windows program verifies and takes one
empty partition, writes an APEX bootstrap environment into it, adds one
self-contained directory to the shared Windows ESP, and appends one firmware
boot entry at the **end** of `BootOrder`. The first boot runs the real
`installer/apex-install` in `mode=partition` against the same partition.

Three measured facts killed "write a prepared filesystem image":

1. On the composefs backend `/boot` **is** the FAT ESP, so a root image
   installs nothing that can start (`docs/boot-v2.md:292-295`).
2. `apex-install` does not finish when `bootc install` returns: `useradd
   --root` (`:1142`), `chroot … setfiles` with the **target's** policy
   (`:1161`), `mokutil --import` (`:2535`). A missing relabel is fatal —
   *"Without the relabel, login would be denied"* (`:1154`). None of it has a
   Windows implementation, so a Linux first boot is mandatory whichever route
   is taken, which removes the prepared image's only advantage.
3. The composefs ESP directory is named by the image's verity digest, computed
   at install time. A program that cannot compute it cannot name it.

### The finding the boot units need

**A Windows-side APEX install cannot use systemd-boot + composefs into a shared
Windows ESP.** Measured inside the guest:

```
esp-total-bytes: 100663296     96.0 MiB
esp-used-bytes :  29087744     27.7 MiB   EFI/Microsoft/Boot + EFI/Boot
esp-free-bytes :  71575552     68.3 MiB
```

| path | needs | fits? |
| --- | --- | --- |
| ostree + GRUB | 7.47 MiB (`docs/m0-results.md:766-776`) | yes, 60 MiB spare |
| systemd-boot + composefs | ~1.1 GiB (`docs/boot-v2.md:239-242`) | no, off by 16x |

One composefs deployment alone is 374 MiB — five times the entire free space.
`docs/boot-v2.md:58` already records that even the L16's 600 MiB ESP is too
small. So either Windows-started dual-boot installs stay on ostree/GRUB, or the
Windows installer creates a second ESP, which is a GPT modification and
avoiding one is the point of the whole design. **That is a product decision,
not this unit's.**

---

## 2. The Windows machine

`windows-installer/lab/winlab` builds it from Microsoft's evaluation media.
Windows Server 2022 Standard (Core) installs **unattended and headless in three
qemu phases, about 150 seconds**, onto a GPT disk with a 100 MB ESP — 100 MB
deliberately, because that is the constraint the design has to survive.

Four things had to be true first, each a trap the next person does not have to
pay for:

* the stock El Torito UEFI image prints *"Press any key to boot from CD"* and
  waits for a keyboard a headless run does not have — the media is re-authored
  against `efisys_noprompt.bin`;
* `xorriso -indev` on the Microsoft ISO sees **one** file, `README.TXT`.
  Everything real is in the UDF volume, which libisofs cannot read, so it
  reports `efisys_noprompt.bin` as absent when it is not;
* `install.wim` is 4.34 GB, over the ISO-9660 file limit, and Windows' CDFS
  reads no multi-extent files — `wimsplit` into `install.swm` + `install2.swm`;
* `/IMAGE/INDEX` is read from the wim with `wiminfo`, not guessed.

Fixtures: an AHCI disk (`APEX-FIXTURE-A` / `FIXA00000001`) carrying a 17 GiB
zeroed Linux-type partition, a 1 GiB NTFS partition and a 17 GiB zeroed
basic-data partition; and an NVMe disk (`FIXB00000002`) with one 17 GiB zeroed
Linux-type partition. 17 GiB because the tool refuses anything under the 16
decimal GB `apex-install` refuses; fixtures small enough to be convenient would
have exercised every rule except the one that fires in real life.

---

## 3. What the guest proved

`APEX_WINLAB_GUEST=1 tests/test-windows-installer.sh` — **29 passed, 0 failed,
0 could-not-run**, in one uninterrupted run with two Windows guest boots:

* both readings of every partition table **AGREE** on all four disks — Windows'
  `IOCTL_DISK_GET_DRIVE_LAYOUT_EX` against this crate's own parse of the same
  handle, and a disagreement is a refusal;
* the ESP and C: refused as *in use by Windows*, naming `C:\`; the Microsoft
  reserved partition refused as a *protected partition type*; the NTFS fixture
  refused naming `E:\`;
* the **zeroed** basic-data fixture refused — and for a stronger reason than
  its type: `mounted at F:\ -- unrecognised filesystem`. Windows gives a RAW
  basic-data partition a drive letter. That is the design's claim about
  basic-data partitions, demonstrated rather than argued;
* an eligible partition read to the last byte through a `\\.\PhysicalDriveN`
  handle: `ALL-ZERO CONTENT: 18253611008/18253611008 bytes read`;
* the firmware variables byte-identical before and after, compared by
  `virt-fw-vars` outside the guest;
* **the enumeration-order claim, made properly**: the second run moves fixture
  A to a different AHCI port, Windows numbers it `disk 2` and then `disk 1`,
  and the confirmation text is byte-identical across both. The suite fails if
  the number did *not* move, because comparing two identical situations proves
  nothing while looking like proof.

### Three defects only a real Windows could have found

1. **The protective-MBR check refused every Windows-formatted disk.** Windows
   writes `SizeInLBA` = `0xFFFFFFFF` unconditionally — measured on a 40 GB disk
   where the correct `0x04FFFFFF` fits easily. UEFI 2.10 permits both.
2. **The GPT geometry check refused every `sfdisk`-made disk**, requiring
   first-usable-LBA to be exactly 34. `sfdisk` aligns to 1 MiB and writes 2048.
3. **`PARTITION_INFORMATION_EX.Name` was read at offset 104 instead of 72.** It
   did not crash. It returned `"tion"` for `"EFI system partition"`.

And one nothing else could have reported: a fresh Windows Server console is not
UTF-8, so every em dash arrived as `???`. All printed strings are ASCII now,
with a unit test to keep them so.

---

## 4. How far down the priority list

| # | | |
| --- | --- | --- |
| 1 | disk and partition enumeration | **done and proven in the guest** |
| 2 | selection and destructive confirmation | **text done and proven; no selection flow, no prompt** — `inspect` takes a GUID argument |
| 3 | ownership and locking | **refusal proven on five partitions; locking does not apply** |
| 4 | payload deployment | not started |
| 5 | bootloader transaction | not started |
| 6 | undo | not started |

Priority 3's second half is a finding: **Windows creates no volume object for a
Linux-filesystem-type partition**, so there is nothing to lock, and a partition
that *does* have a volume has already been refused. An earlier draft carried an
`FSCTL_LOCK_VOLUME` call that could not be reached on any input; it was removed
rather than left in place, because safety code that never executes reads as
coverage. What runs instead is a fresh volume enumeration immediately before
the content is read.

---

## 5. Before this is pointed at a real machine

Nothing in this branch has touched a physical disk, and it cannot: there is no
write path, and `tests/test-windows-installer.sh` greps the source for
`GENERIC_WRITE`, `WriteFile`, `SetFirmwareEnvironmentVariable`,
`FSCTL_DISMOUNT_VOLUME`, `IOCTL_DISK_SET_DRIVE_LAYOUT`, `DeleteFile`,
`MoveFile` and `CreateDirectory` to keep that mechanical rather than asserted.

A person must still settle:

1. **The ESP tension**, above. It is a product decision.
2. **Whether the tool may ever retype a basic-data partition.** Today it
   refuses and hands the user a `diskpart set id=` command. That is exactly the
   partition a user produces by shrinking C:, so the refusal is the common
   case — and the remedy has never been exercised. Every Windows-made partition
   carries GPT attribute bit 63, and the tool also refuses partitions with
   attributes set, so if `set id=` leaves the bit the remedy is a dead end.
3. **Everything in priorities 4 to 6**, with fault injection at every write
   boundary, in VM firmware, before any of it runs anywhere else.
4. The build is the **GNU** target, not MSVC, and is **unsigned**. No
   Authenticode, no supply-chain review.
5. No 4Kn disk, BitLocker, Storage Spaces, dynamic disk, hot unplug or
   removable media has been in front of it.
