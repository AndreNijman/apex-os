# sdboot-migrate-2 — continuation of sdboot-migrate

items: none directly (design doc: docs/boot-v2.md, "Migrating a machine that already exists")
repo: apex-os
worktree: /var/tmp/apex-work/wt-sdboot-migrate-2
branch: task/sdboot-migrate-2, cut fresh from roadmap/v2.2 @ 76aa2b95 (2026-09-21)

sdboot-migrate's own branch (task/sdboot-migrate) already LANDED in round 35 —
do not redo its work. Read ROADMAP/state/agents/sdboot-migrate.md for the full
history (it corrected sdboot-image's "no in-place migration, must reinstall"
conclusion — that conclusion is WRONG, do not revive it) and the NEXT list.

## NEXT (dispatched with, fill in as you go)
1. Run the migration lab against the real APEX image on btrfs (everything
   measured so far is fedora-bootc:43 + ext4). The lab is already set up for
   this with a control disk (no image rebuild needed between experiments) —
   the exact commands are in sdboot-migrate.md's NEXT item 1. Expect the
   512 MiB ESP refusal to fire (that's the point of the run), then repeat
   with a 2 GiB ESP and see it through.
2. A free-space precheck on the ROOT filesystem (item 7 in the predecessor
   card) — the ESP is checked pre-migration, the root is not, and a migration
   writes a second full ~15GB image copy into /composefs.
3. Tighten the confirm gate to `Requires=boot-complete.target` if the stage
   renames the new entry to carry the boot counter (item 9).

Do NOT touch the L16 or katana's actual boot config — this is lab-only work
(VM guests). Large artefacts go in /var/lab-scratch, never /tmp (15GB tmpfs).

## FOUND
-

## BLOCKED ON
- nothing yet
