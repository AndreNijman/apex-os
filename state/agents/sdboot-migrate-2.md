# sdboot-migrate-2 — continuation of sdboot-migrate

items: none directly (design doc: docs/boot-v2.md, "Migrating a machine that already exists")
repo: apex-os
worktree: /var/tmp/apex-work/wt-sdboot-migrate-2
branch: task/sdboot-migrate-2, cut fresh from roadmap/v2.2 @ 76aa2b95 (2026-09-21)

sdboot-migrate's own branch (task/sdboot-migrate) already LANDED in round 35 —
do not redo its work. Read ROADMAP/state/agents/sdboot-migrate.md for the full
history (it corrected sdboot-image's "no in-place migration, must reinstall"
conclusion — that conclusion is WRONG, do not revive it) and the NEXT list.

## NEXT (fill in as you go)
1. Item 1 (the APEX/btrfs lab run) is set up and running in the background —
   see IN PROGRESS. When it lands, read the two serial logs it produces
   (512 MiB refusal, 2 GiB confirmed boot) and write
   ROADMAP/evidence/sdboot-migrate-2-20260921-lab.md from them, same shape as
   the predecessor's evidence doc.
2. Item 3's code (the +3-0 entry rename in cmd_stage, uncommitted, see FOUND)
   is sitting in the worktree waiting on that same lab run. If the 2 GiB
   guest's migrated boot shows `systemd-bless-boot` actually stripping the
   suffix and `journalctl` clean, commit the rename AND flip
   files/system/units/apex-boot-migrate-confirm.service from
   `Wants=boot-complete.target` to `Requires=`, updating the matching
   Containerfile.base assertion (~line 1292-1303) and the two
   tests/test-boot-migrate.sh assertions at lines ~247-256 that currently
   pin Wants=. If it does NOT succeed (EROFS on the counted rename, an AVC,
   or boot-complete.target simply not reached), do NOT commit the rename —
   revert `git diff` on apex-boot-migrate's stage() and leave the gate at
   Wants=, and write down exactly what failed in FOUND/BLOCKED ON so nobody
   re-attempts it blind.
3. Nothing else queued. If both of the above land, this unit's three NEXT
   items are done; check ROADMAP/resume.sh -f for what else is ready.

## DONE
- Item 2: root-filesystem free-space precheck. `files/system/libexec/
  apex-boot-migrate` — `cmd_precheck` now estimates peak disk cost from
  `du -sk $SYSROOT/ostree/repo` (offline, seconds, no network) and refuses
  `root-too-small` (or `no-repo-size` if it can't measure) before the ESP
  check, sized at 2x the repo estimate + 512 MiB slack (the temporary
  `bootc image copy-to-storage` copy and the permanent `/composefs` copy can
  coexist right before the temporary one is deleted — docs/update-cost.md,
  "Also changed: the one update that migrates the boot path"). Placed and
  worded to stay clear of the `esp-too-small` block the concurrently-running
  `sdboot-xbootldr` agent is editing in the same file (confirmed via `ps aux`
  before writing — their diff only touches lines around `find_esp()` and the
  interior of the esp-too-small refusal, both untouched here).
  tests/test-boot-migrate.sh: +6 assertions (67/67 passing), including one
  that checks the new block runs before `with_esp` so it never depends on
  ESP state. shellcheck clean, `bash -n` clean.
  Committed `234cc4dc`, pushed to `origin/task/sdboot-migrate-2`.

## IN PROGRESS
- Item 1, the APEX/btrfs lab run. Plan (adapted from sdboot-migrate.md's
  NEXT item 1 — its literal `sdmig:v2` recipe is fedora-bootc:43-specific and
  doesn't carry over to the real image without adjustment):
  - Built `localhost/apex-sdmig:v1` = `ghcr.io/andrenijman/apex-os:daily`
    (the same tag `sdboot-xbootldr` is using concurrently, chosen to avoid a
    second 11+ GB pull) + the predecessor's `lab-run.sh`/`lab-run.service`
    control-disk driver copied on top (no dnf install needed — the real
    image already ships podman/rsync/dosfstools/efibootmgr, unlike bare
    fedora-bootc:43). Containerfile at
    /var/lab-scratch/sdboot-migrate-2/Containerfile.
  - Run A (512 MiB / default ESP, expect refusal): `tests/lab/bootc-install-
    lab --size 45G --filesystem btrfs localhost/apex-sdmig:v1
    /var/lab-scratch/sdboot-migrate-2/apexmig-a.img -- --karg
    console=ttyS0,115200n8 --karg systemd.journald.forward_to_console=1`,
    no `--bootloader`/`--composefs-backend` (ordinary GRUB/ostree, the L16's
    shape), then boot-mig.sh + control disk running act-migrate.sh (copied
    from /var/lab-scratch/sdboot-migrate/, this unit's own copies live in
    /var/lab-scratch/sdboot-migrate-2/ per the per-unit scratchpad rule)
    pointed at THIS worktree's apex-boot-migrate (carrying the item 2 + item
    3 changes).
  - Run B (2 GiB ESP, expect a confirmed boot): `bootc install to-disk` has
    no ESP-size flag (checked: `--help` output has no such option) and
    bootc's own default for a GRUB/ostree install is what produced 512 MiB
    on the predecessor's fedora-bootc:43 run — untested whether the real
    APEX image's default differs, but there is no flag to force 2 GiB
    through `to-disk` either way. Built a hand-partitioned `to-filesystem`
    lab script instead (sgdisk p1=2GiB ef00, p2=btrfs root; the exact
    `--skip-fetch-check --acknowledge-destructive` invocation is the same
    one `installer/apex-install`'s partition-mode path already uses and
    ships, at ~line 1992 — not lab-invented), wrapped the same way
    bootc-install-lab wraps to-disk: `--generic-image` in the argv,
    `tests/lab/nvram-guard` around the whole podman call, no host disk or
    NVRAM touched. Script: /var/lab-scratch/sdboot-migrate-2/to-filesystem-
    lab (mirrors bootc-install-lab's safety comments; not added to
    tests/lab/ in the repo — it's lab-only scaffolding for one run, not
    reused by anything else yet, so it stays in the scratchpad rather than
    becoming a second supported entry point without review).
  - Both launched via `systemd-run --user` per the no-nohup rule; check with
    `systemctl --user list-units 'run-*'` / `journalctl --user -u <unit>`.
  - Both runs also exercise item 3's rename (see NEXT item 2) since the
    control disk's apex-boot-migrate is this worktree's copy.

## FOUND
- Concurrency: `sdboot-xbootldr` is running the exact same kind of
  btrfs/APEX-image lab work at the same time (its own `bootc install to-disk
  --filesystem btrfs --root-size 40G` on `ghcr.io/andrenijman/apex-os:daily`
  was mid-run when this unit started, PID visible in `ps aux`). Its diff to
  `apex-boot-migrate` (a `find_xbootldr()` helper + wording inside the
  `esp-too-small` refusal, seen via its live `python3 -c` patch heredoc in
  the process list) does not overlap this unit's edits by line, but both
  branches touch the same file and will need a real merge at landing, not
  just two clean cherry-picks.
- `bootc install to-disk` has no per-run ESP-size option (`--help` checked
  on `ghcr.io/andrenijman/apex-os:daily`, bootc as shipped in that image).
  Getting a specific ESP size onto a GRUB/ostree lab disk means
  `to-filesystem` on a hand-partitioned image, not a `to-disk` flag — see
  IN PROGRESS.
- Memory when this unit started: 29G total, 6.8G free, 21G available (one
  other agent's `bootc install` running, no qemu guest yet). `/var` had
  380G free — large disk images are not the constraint here, RAM during
  simultaneous guest boots might be; checked `free -h` again before each
  `qemu-system-x86_64` launch per the standing instruction.
- The entry-rename risk that made item 3 NOT safe to commit blind: if
  `systemd-bless-boot` cannot actually strip the `+3-0` suffix on a migrated
  boot (the migrated cmdline carries `systemd.mount-extra=UUID=…:/boot:auto:
  ro` per the predecessor's evidence doc, vs. GRUB machines where `bootupd_t`
  writes the ESP — untested whether the ordinary composefs+systemd-boot path
  gets the same `:ro` or something else), the counter keeps decrementing on
  every ordinary boot after the migration with nothing ever blessing it —
  three boots after a SUCCESSFUL migration, systemd-boot would consider the
  entry failed and roll the machine back on its own. That is a worse outcome
  than today's inert-but-harmless `Wants=`, so the rename is only going in if
  the lab shows blessing actually completing, not just boot-complete.target
  being reached (which the predecessor already showed happens either way).

## BLOCKED ON
- The item-1/item-3 lab run finishing (running in background). Nothing else
  in this unit's scope is blocked.
