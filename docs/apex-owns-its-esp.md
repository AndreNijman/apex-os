# APEX owns its own ESP

**Decision (Andre, 2026-09-21):** *"windows side install should be like
everything else with the systemd-boot. maybe it should build a new esp for
apex or something."*

This settles the first of the two product questions the Windows installer unit
was told to leave alone. It is not a Windows-only decision — it removes a
special case rather than adding one.

## What was decided

1. **There is no ostree/GRUB variant for Windows machines.** Every APEX machine
   boots systemd-boot, from a UKI, through the same `bootc install
   --composefs-backend --bootloader systemd` path. One boot story, one set of
   tests, one failure mode to understand.
2. **APEX gets its own EFI System Partition. It does not write into Windows'.**
   Not "prefers not to" — the Windows ESP is read for facts and never written.

The second question — whether the tool may ever retype a basic-data partition
itself — is still Andre's and is **not** answered here.

## Why this is the right shape, and not a workaround

The 512 MiB problem was never really about 512 MiB. It was about *borrowing*.

A stock Windows ESP was measured at **68.3 MiB free** while composefs needs
about **1.1 GiB**. Every route out of that — XBOOTLDR, shrinking the initramfs
far enough to fit, sharing the loader directory — was an attempt to squeeze
APEX into a partition Microsoft sized for Microsoft, which also means competing
with it forever: a Windows feature update that grows `\EFI\Microsoft` reclaims
the slack, and the next `apex update` fails on a machine that worked yesterday.

Owning the partition ends that class of failure outright, and it is *also* the
safer option. The strongest guarantee this project can offer a dual-boot user
is that Windows' own boot path survives byte-identical, and the cheapest way to
guarantee that is to never open the file for writing.

## The mechanism already mostly exists

Three things are already true on `roadmap/v2.2`, checked rather than assumed:

- **`bootc install to-filesystem` uses the ESP the CALLER mounted.** Its own
  help says partitions "are prepared and mounted by an external tool or
  script". bootc does not go hunting; it writes where it is pointed. So
  choosing the ESP is entirely APEX's decision to make.
- **`apex-boot-migrate` already accepts `APEX_MIGRATE_ESP`** — "an already-
  mounted ESP, instead of finding one". The override needed to target a chosen
  partition is in the shipped tool today.
- **The tool already knows a machine can have two ESPs, and which is which.**
  `find_esp()` answers "which ESP belongs to this machine's root disk";
  `booted_esp_partuuid()` answers "which ESP did the firmware load the loader
  from". Its comment records that on katana those are different partitions on
  different disks.

So the work is not "invent ESP selection". It is "add ESP *creation*, and make
the chooser prefer APEX's own".

## What this fixes that is already broken

**katana is booting APEX off the Windows disk right now.** `Boot0000* APEX-OS
Primary` points at PARTUUID `2ba9a2ea…`, the 200 MiB ESP Windows created, while
the APEX disk carries its own **unused 512 MiB `EFI-SYSTEM`** at PARTUUID
`99af3362…`. That machine therefore depends on another operating system's disk
to boot, and its NVMe device names reorder across ordinary reboots. Under this
decision katana needs no new partition at all — it needs to start using the one
it already has.

That is the first migration to run, because it is the cheapest proof: no
partitioning, no shrink, and a machine that measurably stops depending on
Windows.

## Where the space comes from, in order of preference

1. **An existing unused ESP on the APEX disk** — katana. Nothing to create.
2. **Free/unallocated space** on the target disk. No data moves.
3. **Shrinking the Windows NTFS volume.** The installer already surveys
   partitions and produces a plan; this becomes a planned, consented step, and
   per the standing constraint the tool tells the user the commands rather than
   silently repartitioning.

Size it for the job — room for two deployments plus slack, not 68 MiB of
someone else's leftovers.

## This does NOT retire the initramfs work

`initramfs-slim` stays necessary and its priority does not drop. Two different
machines, two different problems:

| machine | situation | answer |
|---|---|---|
| L16, and every existing APEX install | 512 MiB ESP that is **already APEX's own** | shrink the initramfs — there is nothing to create |
| Windows dual-boot | Windows' ESP has 68.3 MiB free | build APEX its own ESP |
| katana | has an unused 512 MiB ESP on its own disk | use it; stop booting off the Windows disk |

Andre's *"it has to work in 512. i dont care how it just has to work"* was
about the L16, which has no Windows on it. A new ESP does not help there. A
smaller initramfs helps everywhere, including making case 2 fit on machines
with little free space to give.

## What must be MEASURED before any of this is claimed to work

None of the following is safe to assume, and the first three are the whole risk
of the design:

1. **Two ESPs on one GPT disk.** Does the firmware boot the intended one from
   an explicit NVRAM entry? APEX already writes its own `Boot0000`, so the
   mechanism is there — but "the spec permits it" is not evidence, and firmware
   is where this project has been bitten before.
2. **Does Windows tolerate a second ESP?** Specifically across a feature
   update, a repair install, and `bcdboot`. A design that survives installation
   and dies at the next Patch Tuesday is worse than no design.
3. **Does `bootupd` stay on the mounted ESP** for every later `bootc upgrade`,
   or does it re-discover one? XBOOTLDR was ruled out partly because bootupd
   "mounts the ESP unconditionally" and records paths in a runtime store every
   later upgrade reads. The same store must not point at Windows' ESP.
4. **Windows' own boot path byte-identical** either side, GPT included, proven
   by comparison against a pristine fixture — not by Windows still booting once.
5. **Whether BitLocker's PCR profile on this machine binds PCR 5.** TCG assigns
   PCR 5 to the GPT partition table. BitLocker's default UEFI + Secure Boot
   profile binds PCR 7 and 11 and leaves it alone, but the legacy profile
   includes it and the profile is group-policy configurable on any machine.
   Where PCR 5 is bound, **any** GPT change forces a recovery prompt on the
   next Windows boot — creating APEX's ESP included, not just retyping. This
   is read, never assumed: `manage-bde -protectors -get C:` prints the
   profile. The lab's `bitlocker-discover` job runs `manage-bde -status` and
   `-protectors -disable` and **does not read the profile at all** — adding
   that is the first thing it needs. This project has measured PCR 0, 7 and 11
   carefully and has never looked at 5.

## Bounds that do not change

- The Windows ESP is **read-only, always**. There is no flag that makes it
  writable.
- Never `bootc install` without `--generic-image` outside a real target, and
  always through `tests/lab/bootc-install-lab`. `--generic-image` is the
  prevention; `tests/lab/nvram-guard` is only detection. bootc runs `--pid=host`
  and `nsenter`s into the host mount namespace, so a tmpfs over efivars inside
  the container is inert. That mistake broke the L16 twice in one evening.
- An `efibootmgr -v` diff either side of anything that could touch firmware
  variables.
- The single commit point stays one `SetVariable` of `BootNext`. Creating a
  partition must be complete and verified before the boot entry moves, and a
  failed trial boot must land back where it started.

## The second decision: the tool edits GPT entries itself

**Decided 2026-09-21. Andre delegated it — "you decide" — so the reasoning is
written out rather than asserted, and it is a judgement, not a proof.**

**The tool changes the partition's type GUID and attributes itself. It does not
print `set id=` and `gpt attributes=0x0` for the user to run in diskpart.**

### Why

Reading the ESP decision for what it asks for: *"build a new esp for apex"* and
*"like everything else"*. The tool could print diskpart commands for creating
that ESP too — but a wizard whose answer is "now go and type seven things into
another program" is the *"suckky system where half the time peoples things wont
work"* that was rejected by name. Once the tool creates a partition, refusing to
change 24 bytes of an existing entry is not a safety position, it is an
inconsistency: the edit is strictly smaller and strictly more reversible than
the creation.

And handing a user raw diskpart is the **more** dangerous option, not the safer
one. diskpart has no undo, and it makes the *user* do the targeting: `select
disk N`, `select partition M`. The tool has already read the raw GPT and knows
exactly which disk and which entry. Transcription is where the accident lives —
one wrong `select disk` and they have retyped the volume Windows boots from.
Programmatic-with-verification beats manual transcription. That is a judgement
about where the risk actually sits, not a theorem.

### The boundary, as a principle — because the next write will be asked about

| operation | who |
|---|---|
| **GPT entry edits** — retype; create from unallocated space | **the tool**, under the invariants below |
| **Filesystem operations** — the NTFS shrink | **the user**, in Windows' own tooling. This is correct, not a compromise: the tool has no NTFS knowledge and must not grow any |
| **Whole-layout construction** — a table built from anything but a fresh read of the current one | **never** |
| **Firmware writes** — `SetFirmwareEnvironmentVariable` | **still open.** ARCHITECTURE.md's "Into the firmware" section plans it; the denylist forbids it today. It is its own decision and is not folded into this one |

### Invariants, not mechanism

The mechanism is deliberately **not** decided here, because neither candidate is
measured yet and the choice is empirical:

- a raw sector read-modify-write through `\\.\PhysicalDriveN` is narrow in
  blast radius but it is unverified that Windows permits a write to LBA 2–33 on
  a **live system disk** at all — the payload-write proof was to a partition
  extent, a different protection regime — and after one, Windows' cached
  partition view is stale until `IOCTL_DISK_UPDATE_PROPERTIES` (0x70140), which
  is not on the allowlist either;
- `GET_DRIVE_LAYOUT_EX` → change one entry → `SET_DRIVE_LAYOUT_EX` is wide in
  API but narrow in intent, and Windows maintains its own state and the backup
  GPT for you.

Which is safer is one guest boot to find out.

> **MEASURED 2026-09-21 — that boot has happened.** Evidence:
> `ROADMAP/evidence/windows-installer-3-20260921.md`; job
> `windows-installer/lab/jobs/gpt-write-mechanism`; 13 host checks, 0 failures.
> The mechanism is still the implementation's to choose, but it is no longer
> choosing blind.
>
> - **The stated unknown is answered: Windows PERMITS a write to LBA 2–33 of
>   the disk it booted from**, and permits `SET_DRIVE_LAYOUT_EX` on it too.
>   Both probed with no-ops, and the host confirms the system disk's primary
>   and backup GPT are byte-identical to pristine afterwards. "Windows will not
>   let you" is not a safety property available here — the second time in one
>   round that a platform backstop turned out not to be there.
> - **Both mechanisms work** and produce a GPT whose four CRCs are all correct,
>   and **neither writes a byte of partition content**.
> - The raw mechanism's stale-view prediction **holds**, and
>   `IOCTL_DISK_UPDATE_PROPERTIES` (`0x00070140`) fixes it (`ok=True err=0`).
> - **`SET_DRIVE_LAYOUT_EX` relocated the primary entry array from LBA 2 to
>   LBA 2016 and left the old array at LBA 2 untouched** — two partition tables
>   that disagree in the primary GPT area, the stale one at the LBA hardcoded
>   GPT readers use. The raw mechanism edits in place and leaves one table.
>   **Where it relocates to is a function of `FirstUsableLBA`**: the array is
>   parked to END at it (`2016 + 32 == 2048`, checked). The lab fixtures are
>   `sgdisk`-made with a 1 MiB reserve, `FirstUsableLBA = 2048`; `golden.raw`,
>   partitioned by **Windows Setup itself**, has `FirstUsableLBA = 34`, where
>   the same rule yields LBA 2 and **no relocation at all**. So this hazard
>   does not bite a disk Windows made — it bites disks made by Linux tooling,
>   **which is what APEX itself creates**. The `FirstUsableLBA=34` row is a
>   prediction from one measurement, not a second measurement.
> - **Invariant 3 is exercised, not just specified**: both copies saved to a
>   33 792-byte file before the change, restored afterwards, byte-exact.
> - If `SET_DRIVE_LAYOUT_EX` ever wins, `0x0007C054` and `0x00070140` have to
>   join section 0's allowlist — a deliberate widening that must keep the gate
>   failing both ways.

What the implementation must satisfy either way:

1. The delta is **exactly one entry's type GUID and attributes**. Nothing else
   on the disk changes, proven byte-identical against a pristine fixture.
2. Both GPT copies — primary and backup — end consistent, with correct CRCs.
3. A **backup of both copies is written to a file before the change**, and
   `--undo-gpt <file>` restores it. Undo restores the **table**, not partition
   contents; say so to the user in those words.
4. Windows' partition view is coherent afterwards, not stale.
5. Volumes are **re-enumerated immediately before the write** and it is refused
   if anything now overlaps — ARCHITECTURE.md's Exclusivity rule, which already
   applies to the payload write.
6. Any layout handed to the kernel is derived from a **fresh read of the current
   one**. Never from a cached or reconstructed table. This holds forever,
   whichever mechanism wins.

### Refusals that are never overridable

- Anything with a **recognised filesystem signature**. NTFS present → refuse,
  and say "delete the volume in Disk Management first". This keeps the tool
  entirely out of data destruction, and the survey already reads the raw bytes
  to tell.
- The ESP, Microsoft Reserved, any recovery partition, and the partition
  Windows booted from.
- A BitLocker-protected volume (`-FVE-FS-`).
- **A disk whose BitLocker profile binds PCR 5** — see must-measure item 5 —
  unless the user confirms they hold the recovery key, with the reason stated.

Consent names the disk, partition number, size, current type, filesystem and
label, and confirms *that partition*. Not a yes/no prompt.

### What happens to the write-API gate

It **sharpens; it does not weaken.** `tests/test-windows-installer.sh` section 0
is a real gate — a denylist of names *plus* an allowlist of IOCTL codes, which
exists precisely because `IOCTL_DISK_SET_DRIVE_LAYOUT_EX` is `0x0007C054`, a
number no name-based grep will ever see, *plus* a refusal of bare numeric
literals. It fails both ways today and must still fail both ways afterwards.

Whether that is one binary or a default build plus a declared write build is the
implementation's call, and it proves whichever shape it picks. Deleting an
assertion to get a job green is not on the table.

### Standing bound

Every test of a GPT write is against a **fixture inside the guest**. The L16's
disks are never a target. A bug in this code pointed at the wrong
`PhysicalDriveN` is the same class that took this machine's boot path out twice
in one evening.
