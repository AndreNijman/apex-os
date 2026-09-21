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
