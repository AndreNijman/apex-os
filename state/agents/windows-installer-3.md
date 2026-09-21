# windows-installer-3 — continuation of windows-installer-2

items: none directly (queue id `windows-installer` is closed; this is a direct continuation)
repo: apex-os
worktree: /var/tmp/apex-work/wt-windows-installer-3
branch: task/windows-installer-3, cut fresh from roadmap/v2.2 @ 76aa2b95 (2026-09-21)

windows-installer-2's own branch already LANDED in round 35 (a real Windows
guest, headless install, 3 defects found) — do not redo its work. Read
ROADMAP/state/agents/windows-installer-2.md for the full history, the
Architecture doc reference, and the nine numbered traps already paid for.

## NEXT (dispatched with, fill in as you go, IN THIS ORDER)
1. **Do this first, before anything else writes to a fixture.** Fixture disks
   (fixture-a.raw, fixture-b.raw) are attached raw with no overlay, unlike the
   system disk. The first job that writes anything permanently mutates them
   and every later run silently measures a different disk. Give them qcow2
   overlays in `cmd_run` before any of the items below.
2. Test the `diskpart set id=` remedy on a fixture overlay — every
   Windows-made partition on the golden image carries GPT attribute bit 63
   (0x8000000000000000), and `plan::assess` refuses ANY partition with
   attributes set, so the remedy may lead straight into a second refusal. One
   guest boot settles it.
3. Priority 4 (payload deployment) with a SYNTHETIC payload only: write a few
   MB of known SHA256 through \\.\PhysicalDriveN at the verified offset, `cmp`
   it back from the host. Read ARCHITECTURE.md's "Exclusivity" section first —
   there is no volume to lock for a Linux-filesystem partition; safety is
   offset validation + a volume re-enumeration immediately before each write.
4. Priorities 5/6 (ESP transaction + undo) have not started; design is in
   ARCHITECTURE.md.

Two product decisions stay Andre's, do not solve them: whether Windows-side
installs stay on ostree/GRUB (composefs needs ~1.1GiB ESP, stock Windows ESP
has 68.3MiB free), and whether the tool should ever retype a basic-data
partition itself.

## FOUND
-

## BLOCKED ON
- nothing yet
