# PAUSED — 2026-09-21

**Andre:** *"stop all development of windows app, push to repo and everything,
but show its archived and work is paused for now cause i dont see the need."*

Development stopped here. Nothing is deleted, nothing is half-finished in the
tree, and everything that was measured is landed. This file exists so the next
person — including a future agent scanning for work — does not pick it up
without reading why it stopped.

**Do not resume this without Andre asking for it.** There is no blocker and no
bug waiting; the unit was paused on product judgement.

## State at the pause

The tree is coherent. `tests/test-windows-installer.sh` passes, the lab works,
and the `.exe` **still has no write path** — the section 0 gate that guarantees
that is unchanged and still fails both ways.

What exists: a Rust binary that cross-builds for Windows and has been run on a
real Windows Server 2022 guest. It opens `\\.\PhysicalDrive*`, reads raw GPT
bytes through IOCTLs, surveys disks and partitions, detects BitLocker by its
`-FVE-FS-` signature, decides whether a partition is eligible, and prints the
exact diskpart remedy when one is not. Plus a VM lab (`lab/winlab`, ~150 s to a
booted guest) and four jobs: `survey`, `diskpart-remedy`, `payload-write`,
`bitlocker-discover`.

What does not exist: any write from the `.exe`. Payload deployment was proven
from the PowerShell side of the lab only.

## Why the measurements outlived the feature

Three of these are facts about **Windows and about bootc**, not about this
installer, and they stay true whether or not anyone writes another line here.
Two of them contradict things that were believed. Full text in
`docs/apex-owns-its-esp.md`.

1. **"Windows will not let you" is not a safety property for the partition
   table.** Measured: Windows permits a raw write to LBA 2–33 of the disk it
   booted from, and permits `SET_DRIVE_LAYOUT_EX` on it. Anyone who assumed the
   platform was a backstop was wrong.
2. **`SET_DRIVE_LAYOUT_EX` relocates the primary GPT entry array** from LBA 2
   to LBA 2016 and leaves the stale array at LBA 2 — so two disagreeing tables
   sit in the primary area and the stale one is where hardcoded GPT readers
   look. Scoped to disks with a 1 MiB reserve, which is Linux tooling's default
   and APEX's own.
3. **Windows' ESP content is not byte-identical across a boot** — 42,481 bytes
   changed, all of it Windows writing its own BCD, two files before `bcdboot`
   even ran. "APEX never writes Windows' ESP" is a rule about APEX's behaviour,
   never a claim that the partition sits still. Any future integrity check that
   asserts otherwise will fail for an innocent reason.
4. **PCR 5 is the GPT**, and where a BitLocker profile binds it, *any* GPT
   change — including merely creating APEX's own ESP — forces a 48-digit
   recovery prompt on the next Windows boot. The `bitlocker-discover` job reads
   the profile three ways, because no single way exists on every machine.

## Known limits, stated rather than buried

- The second-ESP result was measured with the new ESP **later in partition
  order**. Position-dependence — the case that would actually bite, since
  bootc takes `find_first_colocated_esp()` — is **untested**.
- `payload-write` is a synthetic payload. It is not an APEX install.

## The three product decisions stand

They are settled and recorded in `docs/apex-owns-its-esp.md`: APEX owns its own
ESP, the tool edits GPT entries itself under stated invariants, and firmware
writes follow the `BootNext` discipline. They were decided on their merits and
are not withdrawn by this pause — they also govern the **Linux** migration path,
which is not paused and is where that reasoning is now doing its work.

## CI keeps running, and that is deliberate

The suites stay wired into CI rather than being exempted. This repo's own
`check-suites-run-in-ci.sh` exists because "code nobody compiles rots, and rots
silently" — and this directory's first round is the cited example: it shipped
655 lines that had never been built for Windows, and the unit tests all passed
because they were compiled for Linux.

So: paused means nobody adds features. It does not mean the tree stops being
checked.

**If a Windows suite goes red while paused, the correct response is to record
it here and tell Andre — not to resume development.** A red suite on paused
work is information about something else in the repo having moved, most likely
a shared gate or the Rust toolchain.
