# Payload deployment, measured: the write lands where it is aimed, and Windows
# is only half the backstop it was claimed to be

`windows-installer` priority 4 (payload deployment), 2026-09-21, one Windows
Server 2022 guest boot plus host-side byte verification. Unit
`windows-installer-3`, rounds 38–39.

Round 38's merge message (`71bc2177`) recorded this result as **UNPROVEN**: the
guest run passed at 12:04 but its host-side byte verification was cut off by a
shutdown and no evidence file existed. This is that file. Everything below is
re-derived on the host from the qcow2 overlay the guest actually wrote through;
nothing here is taken on the guest's word.

## What was run

- Job: `windows-installer/lab/jobs/payload-write/run.ps1`, `winlab run`, one
  boot, `APEXLAB-RUN-EXIT 0`, `STATUS PASS`, firmware variables **IDENTICAL**
  before and after (no boot entry and no boot order changed).
- Guest transcript: `/var/lab-scratch/winlab/payload-write.log`.
- Host verification: `/var/lab-scratch/windows-installer-3/hostverify.{sh,py}`,
  log `hostverify.log`, run inside `localhost/apex-winlab:latest`.
- Target: `payload-write-fixture-a.qcow2`, a qcow2 overlay backed by
  `fixture-a.raw` (38 654 705 664 B virtual, 10.6 MiB allocated). The pristine
  backing file's mtime was asserted unchanged (2026-09-21 02:20:58 UTC) as the
  first act of the verification, so the comparison baseline is the fixture as
  built, not a fixture the guest touched.

**No write path was added to the Rust binary.** `apex-windows-installer.exe`
still cannot write; section 0 of `tests/test-windows-installer.sh` — the
write-API denylist plus IOCTL allowlist — is unchanged and still passes. The
writes below are performed by PowerShell inside the guest, which is what the
lab is for. The binary's role in the run was to be asked, immediately before
the write, whether the partition was still eligible (`inspect`), exactly as
ARCHITECTURE.md's Exclusivity section says the real code must.

## Partition layout under test (fixture-a, from the guest's own survey)

| # | name | type | extent (bytes) | Windows claim |
|---|---|---|---|---|
| 1 | APEX-TARGET-A | Linux filesystem | 1 048 576 … 18 254 659 584 | none |
| 2 | Windows data | Windows basic data, NTFS | 18 254 659 584 … 19 328 401 408 | mounted `E:\` |
| 3 | Blank basic | Windows basic data, **RAW** | 19 328 401 408 … 37 582 012 416 | mounted `F:\`, unrecognised filesystem |

Primary GPT `0 … 17 408`; backup GPT `38 654 688 768 … 38 654 705 664`.

## Result 1 — the offset arithmetic is correct. 13 host checks, 0 failures.

A 4 MiB payload of known SHA256 was written through `\\.\PhysicalDrive2` at
partition 1's verified offset.

- `sha256(4 MiB @ 1 048 576)` read from the host =
  `551611eab74b0fd88e2c00685778fb6aad233dc0916e3c464ddbc4ac2d21b683` — the
  value the guest generated. Verified twice: read back in-guest, and here by a
  reader entirely outside Windows.
- Partition 1 contains **exactly one** guest-written extent,
  `[1 048 576, 5 242 880)`. The payload landed at the verified offset and
  nowhere else in the partition. The rest of partition 1 is unallocated in the
  overlay — it still reads the pristine bytes, and sampled sectors are zero.
- **Primary and backup GPT are byte-identical to the pristine fixture**
  (17 408 B and 16 896 B compared, zero differing bytes).
- **No cluster outside the three partitions was written at all.** The overlay's
  allocation map has 51 guest-written extents totalling 10 616 832 bytes, and
  every one falls inside p1, p2 or p3: nothing in the GPT, nothing in the free
  tail between p3 and the backup GPT, nothing anywhere else. This is a stronger
  statement than a byte comparison — a cluster that was never allocated in the
  overlay cannot differ from its backing file.

## Result 2 — "Windows itself is the backstop" is FALSE as ARCHITECTURE.md wrote it

Two further writes were attempted at partitions the tool refuses, to find out
whether the *platform* would also refuse them. ARCHITECTURE.md claimed it
would. It does for one and not the other.

| target | expected | **measured** |
|---|---|---|
| p2, mounted NTFS (`E:`) | refused | **REFUSED.** `Access to the path is denied`, HResult `-2146233087` (`0x80070005`, `ERROR_ACCESS_DENIED`). The 512 bytes it aimed at are byte-identical to the pristine fixture, confirmed host-side. |
| p3, lettered but **RAW** (`F:`) | refused | **NOT REFUSED — the write SUCCEEDED.** The bytes changed: `076a27c7…` → `f4d5587c…`, and the host confirms the new content matches what the guest wrote. |

The blast radius of the unexpected success is bounded and measured: the overlay
allocated exactly one 64 KiB cluster at p3's offset, and every differing byte
inside it lies within `[19 328 401 408, 19 328 401 920)` — the 512 bytes the
script wrote, and not one byte more.

**Reading of the result.** Windows protects a mounted volume whose filesystem
it *recognises*. A drive letter and a live volume object alone are not
protection. FAT was not measured; assume nothing about it.

**Why it matters more than a documentation fix.** `plan::assess()` refuses
partition 3 as "in use by Windows". That refusal is now known to be
**load-bearing safety rather than redundant defence** — for a RAW volume it is
the only thing standing between a user and an overwritten partition. And it is
the case a user is most likely to create: the round-35/38 phase-0 finding
showed that shrinking `C:` and leaving the new volume unformatted produces
exactly this partition, lettered by Windows as an unrecognised filesystem. The
one case the platform will not catch is the common one.

Actioned: ARCHITECTURE.md's Exclusivity section now states the measured
boundary, names the old sentence as false, and says the ownership refusal must
not be weakened on the theory that the platform will catch it.

## Honest limits of this evidence

- The 126 538 bytes that differ from pristine elsewhere inside partition 2 are
  **Windows' own NTFS metadata activity** from having `E:` mounted, not the
  tool and not the refused write. The attribution is measured, not assumed: the
  refused write's exact 512-byte target is byte-identical to pristine, so every
  other delta in that partition has another author. This is the same behaviour
  that made the qcow2 fixture overlays necessary in the first place — Windows
  dirties any NTFS volume it mounts merely by looking at it.
- The payload is synthetic (a fixed-seed 4 MiB random block). Nothing about a
  real APEX image layout is proven here; this measures the write mechanism and
  its containment, not the contents.
- One guest, one firmware, one disk topology (SATA, 512 B sectors). 4 Kn and
  NVMe-attached targets are unmeasured.
- The guest ran without a TPM, so nothing here says anything about BitLocker
  or measured boot.

---

# BitLocker, measured: three states, and PCR 5 read for the first time

`windows-installer/lab/jobs/bitlocker-discover`, two guest boots (~85 s total),
`APEXLAB-RUN-EXIT 0`, `STATUS PASS`, firmware variables **IDENTICAL** before
and after. Transcript: `/var/lab-scratch/winlab/bl-discover.log`.

This job had **never run to completion**. Round 37 left `bl-discover.log` at
351 bytes, cut off at "boot 1 of at most 2", and round 38's merge message
recorded it as committed-but-unproven. It now completes: phase 1 installs the
BitLocker feature (`Success: True`, `RestartNeeded: Yes`), the harness powers
the guest back on, and phase 2 encrypts, inspects and suspends.

## The three states, and why two of them look the same

The detector has to tell "safe to touch" from "will cost the user a 48-digit
recovery key". These are the numbers it must branch on — locale-independent,
not parsed English:

| state | WMI `ProtectionStatus` | `ConversionStatus` | `PersistentVolumeID` | raw sector-0 OEM ID (bytes 3..11) |
|---|---|---|---|---|
| not encrypted | `0` | `0` | empty | `NTFS    ` |
| encrypted, protection ON | `1` | `1` | populated | `-FVE-FS-` |
| encrypted, protection **SUSPENDED** | `0` | `1` | populated | `-FVE-FS-` |

**Two traps fall straight out of that table, and both would produce a detector
that is confidently wrong:**

1. **`ProtectionStatus` alone cannot distinguish "not encrypted" from
   "encrypted but suspended".** Both read `0`. A detector that branches on
   `ProtectionStatus == 0` treats a suspended BitLocker volume as a plain
   unencrypted one. `ConversionStatus` is what separates them, and
   `PersistentVolumeID` corroborates.
2. **The raw on-disk signature cannot distinguish "protected" from
   "suspended".** Sector 0 reads `-FVE-FS-` in both. This matters precisely
   where it hurts most: on a disk Windows does not manage there is no volume
   object and *no WMI row at all*, so the raw signature is the only thing
   available — and it can tell you the volume is encrypted but not whether it
   is safe. On such a disk the only correct answer is to refuse.

Suspension leaves the volume fully encrypted on disk (`Used Space Only
Encrypted`, `100.0%`, `BitLocker Version 2.0`) and merely clears protection.
That is the state the "suspend BitLocker first" remedy is supposed to reach,
and it is now a measured state rather than a described one.

## A defect in the job itself, found and fixed in the same round

The first run measured only two states while claiming three. `manage-bde
-protectors -disable` accepts `-RebootCount` **only on the OS volume**; against
the `WINDATA` data volume Windows rejected the entire command:

    ERROR: An error occurred (code 0x80310028):
    The drive specified is not the operating system drive.

The suspend therefore never happened — and the job went on to label the next
WMI dump *"encrypted, protection suspended"* while the volume still reported
`ProtectionStatus=1`. A transcript that names a state it never reached is worse
than one that stays silent, and this is the same failure family the program
keeps paying for: a step that runs, fails, and is recorded as having worked.

Fixed: the OS-volume form is kept deliberately (its rejection is itself a
measured fact about the API), the data-volume form runs after it, and the state
is **asserted from WMI's number** before any dump is labelled. If
`ProtectionStatus` has not moved the transcript now says `SUSPEND-EFFECTIVE: NO`
and tells the reader not to read the dump as a suspended volume. The re-run
reads `SUSPEND-EFFECTIVE: YES -- ProtectionStatus went 1 -> 0`.

## must-measure item 5: does BitLocker's profile bind PCR 5?

`docs/apex-owns-its-esp.md` item 5. PCR 5 is the GPT partition table; where a
profile binds it, **any** GPT change forces a recovery prompt on the next
Windows boot — including merely creating APEX's own ESP. The doc noted this job
did not read the profile at all. It does now, three ways, because no single way
is available on every machine.

**Answer for this guest, with its limit stated plainly:**

- `tpm-present: NO` — `Win32_Tpm` reachable with zero instances, `Get-Tpm`
  reports `TpmPresent=False`. The lab's qemu line has no `-tpmdev`, and the lab
  container has no `swtpm` to provide one.
- `HKLM\SOFTWARE\Policies\Microsoft\FVE` **absent**, as is the legacy
  `…\CurrentVersion\Policies\FVE`. No group policy configures a platform
  validation profile on this machine.
- `HKLM\SYSTEM\CurrentControlSet\Control\BitLocker` is **absent before** the
  feature install and **exists (with no values) after** it — the key tracks the
  feature, not any policy, so its presence must not be read as configuration.
- `manage-bde -protectors -get` prints no profile for either volume, correctly:
  a profile is printed only for a TPM-backed protector, and the only protector
  here is a password.

**So on this guest nothing binds PCR 5, because there is no platform validation
profile at all.** That is a real answer for the no-TPM case and it is the wrong
answer to generalise from: it says nothing about a machine that *has* a TPM,
which is every machine a user would actually be installing onto. **Item 5 is
NOT closed.** Closing it needs `swtpm` in the lab image, a `-tpmdev`/`tpm-tis`
guest, and an OS volume encrypted with a TPM protector. The reading code is now
in place and waiting for that guest.

The parser was written for the TPM case it cannot yet exercise, and one bug was
fixed before it could ever fire: `manage-bde` prints several PCR numbers on one
indented line under the `PCR Validation Profile:` header, so a per-line
`^\s+(\d+)` regex captures the first and drops the rest — it would have reported
`pcr5-bound: NO` for a profile reading `0, 2, 4, 5, 11`. It now collects every
number on every continuation line.

## Also measured, incidentally but usefully

- BitLocker **is** installable and usable in this guest: it is an optional
  feature on Server, `Install-WindowsFeature BitLocker` returns
  `ExitCode: SuccessRestartRequired`, and the harness's `--boots 2` reboot
  loop carries the job across it correctly. That is the multi-boot mechanism
  working end to end for the first time.
- A data volume can be encrypted **without a TPM** using a password protector
  (`AES 128`, `-UsedSpaceOnly`), which is what made the three-state
  measurement possible at all on a TPM-less guest.
- The OS volume `C:` stayed `Fully Decrypted / Protection Off` throughout, so
  nothing here measures OS-volume suspend semantics — the case that actually
  matters when the installer wants to touch the ESP. Unmeasured, and named as
  unmeasured.

---

# The GPT write mechanism, measured: both candidates work, and they are not equivalent

`windows-installer/lab/jobs/gpt-write-mechanism`, one guest boot (~30 s),
`APEXLAB-RUN-EXIT 0`, `STATUS PASS`, firmware variables **IDENTICAL**.
Transcript `/var/lab-scratch/winlab/gpt-write-mechanism.log`; host verification
`windows-installer/lab/jobs/gpt-write-mechanism/hostverify.py`,
**13 checks, 0 failures**.

`docs/apex-owns-its-esp.md` settled that the tool edits the GPT entry itself
and then said, deliberately: *"The mechanism is deliberately NOT decided …
Which is safer is one guest boot to find out — that measurement is yours to
make."* This is that boot.

## The question the doc called unverified: may anyone write LBA 2–33 of a LIVE system disk?

**Yes. Windows permits it.** Probed on `\\.\PhysicalDrive0` — `IsSystem=True`,
`IsBoot=True`, the disk the running Windows booted from — with a **no-op**: the
exact 512 bytes just read from LBA 2 written straight back, so a success
changes nothing and a refusal costs nothing.

    SYS-RAW-GPT-WRITE: PERMITTED -- Windows allowed a write to LBA 2 of the disk it booted from
    SYS-LBA2-UNCHANGED: YES

The same question through the layout IOCTL, also on the live system disk:

    SYS-GET_DRIVE_LAYOUT_EX: ok=True err=0 bytes=480   PartitionStyle=1 PartitionCount=3
    SYS-SET_DRIVE_LAYOUT_EX (handing back the UNMODIFIED layout): ok=True err=0

Both permitted. And confirmed from the host afterwards against pristine
`golden.raw`: the system disk's **primary and backup GPT are byte-identical**,
17 408 and 16 896 bytes comparing equal. Neither probe moved a byte.

So "Windows will not let you" is **not** a safety property either mechanism can
lean on for the partition table, exactly as it turned out not to be for a
lettered RAW volume earlier in this round. The protection has to be ours.

## The two mechanisms, head to head on fixture-a

Target: partition 3, "Blank basic" — Windows basic data, no recognised
filesystem, lettered `F:` — retyped to Linux filesystem, attributes left at 0.
Both mechanisms produced a correct, consistent GPT. They differ in three ways
that matter.

| | **M1 — raw read-modify-write of LBA 2–33** | **M2 — GET → patch → SET_DRIVE_LAYOUT_EX** |
|---|---|---|
| result | both copies consistent, all four CRCs correct | both copies consistent, all four CRCs correct |
| who maintains the backup GPT | **us**, by hand — we compute both CRCs and write both copies | **the kernel** |
| Windows' view straight after | **STALE**: still `GptType={ebd0a0a2…}`, still lettered `F:` | **already correct**: `{0fc63daf…}`, letter gone |
| after `IOCTL_DISK_UPDATE_PROPERTIES` (0x70140) | correct: `{0fc63daf…}`, letter gone | unchanged (it was already right) |
| **where the primary entry array ends up** | **LBA 2, where it was** | **LBA 2016 — the kernel MOVED it** |

The doc's two predictions both hold: the raw mechanism does leave Windows'
cached partition view stale until `IOCTL_DISK_UPDATE_PROPERTIES`, and that
IOCTL does fix it (`ok=True err=0`); and the kernel does maintain its own state
and the backup GPT for you.

## The finding that decides it: M2 relocates the table and leaves a stale copy behind

`SET_DRIVE_LAYOUT_EX` did not edit the partition entry array in place. It
**wrote a new array at LBA 2016** — immediately before the first partition, at
the end of the 1 MiB alignment gap — rewrote the header's `PartitionEntryLBA`
to point there, and **left the old array at LBA 2 completely untouched**.

Measured on the host, byte by byte, against the pristine fixture:

- primary GPT area: **161 bytes** differ — 10 in the header (`HeaderCRC32`,
  two bytes of `PartitionEntryLBA` as 2 → 2016, `PartitionEntryArrayCRC32`)
  and 151 in the newly written array at LBA 2016;
- backup GPT area: **24 bytes** differ — **16 of them are exactly the target
  entry's type GUID**, plus two 4-byte CRCs in the backup header. The backup
  array was *not* relocated;
- **LBA 2–33 is byte-identical to pristine.** The old table is still there.

So on disk there are now two partition tables in the primary area that
disagree:

    a reader that trusts LBA 2 sees this entry as type ebd0a0a2-… (basic data)
    the header says the live array is at LBA 2016, where it reads 0fc63daf-… (Linux)

**Reading LBA 2 directly is the single most common shortcut in GPT code** —
the spec permits `PartitionEntryLBA` to be anything, but in practice it is 2 on
almost every disk, so code that hardcodes it works everywhere until it does
not. After a `SET_DRIVE_LAYOUT_EX`, such a reader sees the *pre-change* table
and will happily conclude the partition is still Windows basic data. This tool
already does the right thing — its survey reported the new type correctly, and
its "the on-disk GPT and Windows' partition table AGREE" line held throughout —
but any *other* tool on the user's machine may not.

### But WHERE it relocates to is a function of `FirstUsableLBA`, and that scopes the hazard

The destination is not a constant. Windows parks the array so that it **ends
exactly at `FirstUsableLBA`** — checked, not inferred: `2016 + 32 == 2048`.
Read off the pristine images:

| disk | who partitioned it | `FirstUsableLBA` | array lands at | relocation? |
|---|---|---|---|---|
| `fixture-a.raw` / `fixture-b.raw` | `sfdisk`/`sgdisk` in the lab, 1 MiB reserve | **2048** | 2048 − 32 = **2016** | **yes**, stale table left at LBA 2 |
| `golden.raw` | **Windows Setup itself** | **34** | 34 − 32 = **2** | **no** — it lands back where it started |

So the hazard **does not arise on a disk Windows partitioned**, which is what
every real target machine has. It arises on a disk initialised by tooling that
reserves 1 MiB — which is `sgdisk`, `sfdisk`, `parted` and essentially all
Linux partitioning, **including the disks APEX itself creates**. That is not a
reason to dismiss it; it is a reason to state where it bites.

**Honest status of the golden.raw row: it is a derivation, not a measurement.**
The rule was measured once, on one disk, and `golden.raw`'s `FirstUsableLBA` is
a fact read from its header — but no `SET_DRIVE_LAYOUT_EX` with a real change
has been run against a `FirstUsableLBA=34` disk to confirm the array stays at
LBA 2. The system disk was only ever probed with no-ops in this job. The
second-ESP work, which necessarily rewrites `golden.raw`'s GPT, is where that
gets confirmed.

M1 has no such hazard: it edits the array in place, so there is exactly one
table and no stale copy.

**Both rows are host-measured, not argued.** M1 was originally undone before M2
ran — which made the comparison fair but left M1's final state unverified, true
by construction rather than by bytes. A `keep-m1.txt` switch now stops the job
after M1, and a second 30-second boot produced an image the same verifier
checked (12 checks, 0 failures; the relocation-destination check is skipped
because nothing relocated):

| | bytes changed, primary GPT area | bytes changed, backup GPT area | total | stale table left |
|---|---|---|---|---|
| **M1** raw read-modify-write | **24** — `HeaderCRC32` (4), `PartitionEntryArrayCRC32` (4), and **16 bytes of type GUID** at LBA 2 | **24** — the same three fields | **48** | none |
| **M2** `SET_DRIVE_LAYOUT_EX` | **161** — 10 in the header (incl. `PartitionEntryLBA` 2 → 2016) + 151 writing a whole new array at LBA 2016 | **24** — 16 bytes of type GUID + two CRCs; not relocated | **185** | **yes**, the entire old array at LBA 2 |

M1's delta is the theoretical minimum for this change: the 16 bytes that had to
change, and the two checksums that describe them, in each copy. Nothing else.

## Against the doc's six invariants

1. **"The delta is exactly one entry's type GUID and attributes. Nothing else
   changes, proven byte-identical against a pristine fixture."** Held for the
   partition *table contents*: exactly one entry differs, its start, end and
   attributes are unchanged, the other two entries are identical, and **not one
   byte of p1 or p3 was written** — changing an entry never touched the bytes
   it describes. **Not** held literally by M2 at the disk level, because
   relocating the array rewrites `PartitionEntryLBA` and 151 bytes of
   previously-zero space. M1 holds it literally.
2. **"Both copies end consistent, with correct CRCs."** Held by both. All four
   CRCs recomputed independently on the host with `binascii.crc32` and matched.
3. **"A backup of both copies is written to a file before the change, and undo
   restores it."** Implemented and exercised: a 33 792-byte bundle (both
   headers, both arrays) written before anything changed, then restored —
   `UNDO-BYTE-EXACT: YES`, the primary GPT region hashing back to exactly its
   pre-change SHA256, both copies consistent again, and Windows' view back to
   basic data with letter `F:`.
4. **"Windows' partition view is coherent afterwards, not stale."** Requires
   `IOCTL_DISK_UPDATE_PROPERTIES` after M1; automatic after M2. Either way the
   installer's own survey then reported "the on-disk GPT and Windows'
   partition table AGREE".
5. **"Volumes re-enumerated immediately before the write."** Not exercised by
   this job — it is the Exclusivity rule already measured by `payload-write`.
6. **"Any layout handed to the kernel is derived from a fresh read of the
   current one."** Followed by construction in both mechanisms: the target
   entry is located by its **unique partition GUID** in a fresh on-disk read
   (M1) and in the kernel's freshly-returned layout (M2), never by an index
   passed in and never from a cached table.

## What this says about the choice — stated as a reading, not a decision

The mechanism is the implementation's to pick and this is the measurement it
was waiting for. On the evidence: **M1 is the narrower change and leaves no
contradictory state on disk; M2 is less code and cannot get the CRCs wrong, but
its price is a second, stale partition table at the LBA every naive reader
looks at.** A hybrid is available and was not tested: M1 for the write, then
`IOCTL_DISK_UPDATE_PROPERTIES` for the view — which is what M1 already does
here, and which costs one extra IOCTL on the allowlist.

Note for the gate: `IOCTL_DISK_UPDATE_PROPERTIES` (`0x00070140`) and, if M2
ever wins, `IOCTL_DISK_SET_DRIVE_LAYOUT_EX` (`0x0007C054`) are **not** on
section 0's allowlist today. Adding them is a deliberate widening that must
keep the gate failing both ways, per the decision doc.

## Three defects in the job itself, found by running it

The first run reported `BOTH-COPIES-CONSISTENT: False` for every disk it
looked at, and not one of those failures was about a disk.

1. **PowerShell 5.1 parses `0xFFFFFFFF` as Int32 `-1`**, so `[uint32]0xFFFFFFFF`
   throws. Every CRC the job computed was an exception and every `ok=False` was
   its own arithmetic. CRC32 moved to C# via `Add-Type`.
2. **`FileStream`'s 64 KiB internal buffer over-reads past the end of the
   device** when reading the backup GPT header, which lives in the disk's very
   last sector — "The request could not be performed because of an I/O device
   error". Raw device streams now open unbuffered (`bufferSize 1`).
3. Worst, and only reachable because of (2): **a header that failed to read
   became a record of nulls**, so `$h.EntriesLba * 512` evaluated to `0` and
   the writes aimed at the backup GPT landed on the protective MBR. That is
   what scrambled fixture-a's overlay on the first run and made `M2` read
   `PartitionCount=0`. `Read-GptHeader` now throws unless the signature is
   `EFI PART`, the caller stops, the two headers are cross-checked against each
   other before any write, and the undo reports every write instead of
   discarding it with `$null =`.

The pristine fixtures were never at risk: guests run against qcow2 overlays and
`fixture-a.raw` / `golden.raw` mtimes are unchanged.

## Honest limits

- One guest, one firmware, SATA, 512-byte sectors. 4 Kn and NVMe-attached
  system disks are unmeasured.
- The **live system disk** was only probed with no-ops. Nothing here says a
  *real* GPT change to a live system disk is safe — only that Windows does not
  refuse the write. A real change there is what BitLocker and PCR 5 guard, and
  PCR 5 remains unmeasured on a TPM machine.
- M2's relocation behaviour was observed **once**. The rule it fits — the array
  is parked to end at `FirstUsableLBA` — is consistent with that observation and
  with both pristine images' headers, but one observation does not establish a
  rule. In particular, the claim that a `FirstUsableLBA=34` disk sees **no**
  relocation is a prediction, not a measurement.

---

# A second ESP: Windows tolerates it, and `bcdboot` does not wander

`windows-installer/lab/jobs/second-esp`, two guest boots (~60 s),
`APEXLAB-RUN-EXIT 0` on both, `STATUS PASS`, firmware variables **IDENTICAL**.
Transcript `/var/lab-scratch/winlab/second-esp.log`; host verification
`windows-installer/lab/jobs/second-esp/hostverify.py`, **9 checks, 0 failures**.

`docs/apex-owns-its-esp.md` must-measure **#2** and **#4**.
`migrate-preconditions`' card confirms it takes none of the four lab
measurements, so they are this unit's.

## What was done to the disk

On the disposable overlay of `golden.raw` — a disk **Windows Setup itself
partitioned**: `diskpart` shrank `C:` by 600 MB (the user's own act in the real
flow; the tool has no NTFS knowledge and must not grow any), then
`create partition efi size=500` + `format quick fs=fat32 label=APEXESP`. A
second ESP now exists at LBA 82 655 232 … 83 679 231, **later** in partition
order than Windows'.

## #2, the `bcdboot` half: it writes the ESP it booted from, not "the first one"

This is the question worth asking because it is the Windows analogue of what
`migrate-preconditions` found on the Linux side: `bootc` re-discovers the ESP
with `find_first_colocated_esp()` on **every** upgrade instead of staying on
the one it was handed, so an ESP earlier in partition order wins forever. If
`bcdboot` picked by position too, a second ESP would be a live hazard on both
sides of the machine.

**It does not.** `bcdboot C:\Windows` with **no `/s`**, two ESPs present,
exit 0:

| | files added | files changed |
|---|---|---|
| **Windows' own ESP** | 0 | **4** — `EFI\Microsoft\Boot\BCD`, `BOOTSTAT.DAT`, `EFI\Microsoft\Recovery\BCD`, `Recovery\BCD.LOG` |
| **the new ESP** | **0** | 0 — still completely empty |

`bcdedit /enum {bootmgr}` reports `device partition=P:` (Windows' own ESP)
before and after, and `\Device\HarddiskVolume1` after the reboot. bcdboot
updated the boot store **in place, in the ESP it came from**, and did not
scatter a single file into the new one.

**The negative half of that is a real measurement in this run and was not in
the first one.** The first attempt printed `BCDBOOT-WROTE-SECOND-ESP: NO` off
an empty reading it could not distinguish from an unreadable path — see the
defects section below. The access path is now proven usable by creating and
removing a probe file before the claim is made.

**Limit, and it is the important one:** the second ESP was **later** in
partition order than Windows'. Position-dependence is exactly what would bite,
so the case that matters — a new ESP **earlier** in partition order — is
**not** tested here and cannot be without relocating partitions. Do not read
this result as "bcdboot is position-independent"; read it as "bcdboot did not
leave the ESP it booted from."

## #2, tolerance: yes, across a reboot

Phase 2 ran, which is the proof: Windows booted normally with two ESPs on its
system disk, the layout intact, the installer's own survey clean. Across that
reboot exactly one file in Windows' ESP changed — `BOOTSTAT.DAT`, which is boot
statistics.

**The feature-update and repair-install halves of #2 are NOT doable in this
lab.** There is no update media and no repair image. That part of must-measure
#2 remains open, and nothing here should be read as covering it.

## #4, restated because as written it cannot hold

"Windows' own boot path byte-identical either side, **GPT included**, proven by
comparison against a pristine fixture" cannot be literally true once a
partition is added — adding one changes the GPT by definition. Read as
*"Windows' own partition entries and Windows' own ESP bytes are unchanged"*,
which is the property that actually protects the user, and verified on the host
against pristine `golden.raw`:

- **Windows' ESP entry: completely unchanged** — type, start 2048, end 206847,
  attributes `0x8000000000000000`, name.
- **Microsoft Reserved entry: completely unchanged.**
- **`C:`'s entry changed in `EndingLBA` and nothing else** — 83 884 031 →
  82 655 231, exactly 600 MiB returned; start, type, attributes and name
  identical. The shrink moved only the end, as it must.
- Exactly one partition added, none removed; both rewritten GPT copies
  self-consistent with correct CRCs and agreeing with each other.

**Windows' ESP *content* is NOT byte-identical: 42 483 of 104 857 600 bytes
changed (0.041%).** Every one of them is Windows writing its own BCD — the
guest's file-level manifest names the four files, and two of them changed
merely from shrinking `C:` and creating a partition, before `bcdboot` was run
at all. Worth stating plainly, because it is easy to misread the product
decision: **"APEX never writes Windows' ESP" is a rule about what APEX does. It
is not a claim that the partition sits still** — Windows rewrites it in
response to ordinary disk changes, so a design that hashed Windows' ESP and
expected stability would be building on sand.

## #1 was deliberately NOT attempted

"Does the firmware boot the intended one of two ESPs from an explicit NVRAM
entry." Telling which of two ESPs actually booted needs `BootCurrent` — a
volatile variable, absent from the varstore the harness diffs — or a bootloader
payload distinguishable from Windows' own, which this lab does not have.
Attempting it with the tools here would produce a confident wrong answer, which
is worse than a stated gap. **#1 remains open.**

## A prediction from earlier in this round, now measured

The `gpt-write-mechanism` section above found that `SET_DRIVE_LAYOUT_EX`
relocated fixture-a's primary entry array from LBA 2 to LBA 2016, and reasoned
that the destination is a function of `FirstUsableLBA` — so a disk Windows
partitioned (`FirstUsableLBA = 34`) should see **no** relocation, `34 − 32 = 2`.
That was labelled a prediction, not a measurement.

**It is now measured.** This run had Windows rewrite `golden.raw`'s GPT for real
— a shrink plus a new partition — and the primary entry array **stayed at LBA
2**. No relocation, no stale second table. The hazard is confirmed to be scoped
to disks initialised with a 1 MiB reserve, which is Linux tooling's default and
**what APEX itself creates**, and not to the Windows-made disks this tool runs
against.

## Two defects in the job, found by running it — same family, twice

1. It located the new ESP with `Get-Partition | Where DriveLetter -eq 'S'` and
   printed `SECOND-ESP-CREATED: NO` **about a partition that had been created
   correctly** and was visible in its own layout dump three lines later.
   `Get-Partition.DriveLetter` reads blank for an ESP-typed partition even when
   `diskpart` has assigned a letter — the same class of trap this unit already
   recorded for `NoDefaultDriveLetter` and `IsHidden`.
2. Worse: the manifest function returned an empty hashtable both for *"the
   directory is empty"* and for *"that path does not exist"*, so the job
   asserted `BCDBOOT-WROTE-SECOND-ESP: NO` from two readings it could not tell
   apart. It now returns null for an unreadable root, the comparison reports
   `NOT MEASURED` instead of `UNCHANGED`, and the path is proven writable with
   a probe file first.

3. And the guard added to fix (2) had the bug **a third time**:
   `Test-PathUsable` emitted its verdict into the same pipeline as its return
   value, so `$ok = Test-PathUsable …` captured a two-element array — truthy
   whether the probe passed or failed. The check that licensed the phrase *"the
   access path was proven usable"* could not itself fail, and its diagnostic
   line never reached the transcript. The verdict now travels in a
   script-scoped variable, the function only emits, and
   `PATH-USABLE[second ESP]: YES -- created and removed a probe file` is in the
   log of the run this section describes.

All three are the dominant defect family in this repo: a check that runs,
inspects nothing, and reports a result. Two of them were introduced *while
fixing* the previous one, which is worth saying out loud — in PowerShell, any
uncaptured expression inside a function joins its return value, so every helper
that both reports and decides is this bug waiting to happen.
