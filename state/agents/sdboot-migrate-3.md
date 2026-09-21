# sdboot-migrate-3 — continuation of sdboot-migrate-2

items: none directly (design doc: docs/boot-v2.md, "Migrating a machine that already exists")
repo: apex-os
worktree: /var/tmp/apex-work/wt-sdboot-migrate-3 (create it)
branch: task/sdboot-migrate-3, cut from origin/roadmap/v2.2
lab: /var/lab-scratch/sdboot-migrate-2 (REUSE it — see below)

Dispatched round 40, 2026-09-22 ~03:30 AWST, by the autoresume orchestrator.

## WHAT LANDED AND WHAT DID NOT

`task/sdboot-migrate-2` **landed** (tip `234cc4dc`, "refuse a migration the root
filesystem cannot fit") — the root-filesystem free-space precheck is done and on
`roadmap/v2.2`. Do not redo it. Read `sdboot-migrate-2.md` and
`sdboot-migrate.md` for the history; `sdboot-migrate` corrected `sdboot-image`'s
"no in-place converter, must reinstall" conclusion, and that conclusion is WRONG
— do not revive it.

**The lab run never finished.** `/var/lab-scratch/sdboot-migrate-2/` was last
written 2026-09-21 10:46 and contains the rig but no results: `apexmig-a.img`
(48 GB), `ctl.img`, `boot-mig.sh`, `lab-run.sh`, `lab-run.service`,
`to-filesystem-lab`, and `sdboot-migrate-2-rename.patch`. There are **no serial
logs**. The agent died before the guests booted. The rig is intact — reuse it
rather than rebuilding 48 GB.

## WHAT IS LEFT, from sdboot-migrate-2's own NEXT

1. **Run the APEX/btrfs lab.** Two guests: a 512 MiB ESP that must be REFUSED,
   and a 2 GiB ESP that must migrate and boot. Both serial logs are the evidence.
2. **Write `ROADMAP/evidence/sdboot-migrate-2-20260921-lab.md`** — same shape as
   the predecessor's `sdboot-migrate-20260921-lab.md`, which is already on the
   tip and is your template.
3. **Decide the `+3-0` entry rename.** `sdboot-migrate-2-rename.patch` holds the
   uncommitted `cmd_stage` change. It lands ONLY if the 2 GiB guest's migrated
   boot shows `systemd-bless-boot` actually stripping the suffix and a clean
   `journalctl`. If it does not, say so and drop the patch — a rename that is not
   observed to be blessed is worse than no rename.

## A COLLISION THAT IS ON RECORD — read before you edit

`files/system/libexec/apex-boot-migrate` is touched by several units.
`wt-sdboot-xbootldr` ran its own `bootc install` lab work against the same file
concurrently in round 39; `task/migrate-preconditions` landed `f3280072` in that
file too (the `esp-is-windows` refusal, 83/0). **Check for line-level overlap
before editing, and expect a real three-way merge rather than independent
cherry-picks.** Your branch is cut from a tip that already has all of it.

## CONSTRAINTS I AM UNDER

- Headless only. Never a window on Andre's desktop. This is L16 work.
- `/var/lab-scratch`, never `/tmp` — it is a 15 GB tmpfs on 29 GB of RAM and
  filling it kills the machine. Stdout-only Bash failures here are memory, not disk.
- **Read `free -h` before every guest boot.** Another KVM lab unit
  (`initramfs-slim-2`) may be running concurrently; a previous round saw
  available memory fall to 1.3 GiB under two labs. If it is tight, wait rather
  than start a guest.
- qemu/podman in the FOREGROUND, or under `systemd-run --user`. Never `nohup &`
  — a backgrounded podman is SIGTERMed, truncating the run while exiting 0.
- Rootless VM traps already paid for, do not rediscover: uid 0's
  `qemu:///session` is the SYSTEM socket, and virtiofsd's namespace sandbox
  cannot nest inside a rootless podman userns.
- Never push `main`, never open a PR, never land on `roadmap/v2.2`. Push your
  branch and mark `## LANDABLE <sha>` on this card.
- Never touch the HOST's boot path or NVRAM. A `bootc install` in a loopback lab
  has rewritten host NVRAM TWICE in this program — the efivars tmpfs is INERT
  because bootc leaves the container. `--generic-image` prevents it and
  nvram-guard detects it. Use both.

## NEXT

- Read `/var/lab-scratch/sdboot-migrate-2/lab-run.sh` and `boot-mig.sh`, confirm
  the rig still matches the current `apex-boot-migrate`, then launch the 512 MiB
  refusal guest under `systemd-run --user`.

## DONE

## IN PROGRESS

## FOUND

## BLOCKED ON

- nothing
