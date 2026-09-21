# sdboot-migrate-3 — continuation of sdboot-migrate-2

items: none directly (design doc: docs/boot-v2.md, "Migrating a machine that already exists")
repo: apex-os
worktree: /var/tmp/apex-work/wt-sdboot-migrate-3 (created, branch task/sdboot-migrate-3 @ f3b1b3d4)
branch: task/sdboot-migrate-3, cut from origin/roadmap/v2.2 @ f3b1b3d4
lab: /var/lab-scratch/sdboot-migrate-2 (REUSED as /work — the 48 GB image lives
     there; new artefacts of THIS unit are suffixed `-3` inside it so provenance
     is unambiguous. sdboot-migrate-2 is dead, so there is no live collision.)

Dispatched round 40, 2026-09-22 ~03:30 AWST, by the autoresume orchestrator.

## WHAT LANDED AND WHAT DID NOT

`task/sdboot-migrate-2` **landed** (tip `234cc4dc`, "refuse a migration the root
filesystem cannot fit"). Do not redo it. `sdboot-migrate` corrected
`sdboot-image`'s "no in-place converter, must reinstall" conclusion, and that
conclusion is WRONG — do not revive it.

## WHAT IS LEFT, from sdboot-migrate-2's own NEXT

1. Run the APEX/btrfs lab: 512 MiB ESP guest must be REFUSED; 2 GiB ESP guest
   must migrate and boot. Both serial logs are the evidence.
2. Write `ROADMAP/evidence/sdboot-migrate-2-20260921-lab.md`.
3. Decide the `+3-0` entry rename — lands ONLY if the 2 GiB guest's migrated
   boot shows `systemd-bless-boot` actually stripping the suffix.

## NEXT

- WAIT for `systemctl --user is-active sdb3-install-a` to go inactive, then
  `tail /var/lab-scratch/sdboot-migrate-3/install-a.log` for `install rc=0` AND
  loop-mount `apexmig-a.img` p2 to confirm the ESP is NOT empty this time (see
  FOUND — a complete GPT proved nothing). Then:
  `cd /var/lab-scratch/sdboot-migrate-2 && ./setctl.sh act-migrate-3.sh
   /var/tmp/apex-work/wt-sdboot-migrate-3/files/system/libexec/apex-boot-migrate`
  and boot run A under `systemd-run --user` (never nohup) via
  `/var/lab-scratch/sdboot-migrate-3/run-boot-a.sh`, which wraps:
  `sudo -n /var/tmp/apex-work/wt-sdboot-migrate-3/tests/lab/nvram-guard --label apexmig-a --
   podman run --rm --device /dev/kvm -v /var/lab-scratch/sdboot-migrate-2:/work
   localhost/apex-bootlab -c '/work/boot-mig.sh /work/apexmig-a.img
   /work/apexmig-a-3.serial 1800 --ctl /work/ctl.img'`
  Expect `REFUSED [esp-too-small]`.

### Guest bookkeeping — the persistent-NVRAM rig makes boot ORDER matter

`boot-mig.sh` keeps one `VARS` file per guest and `lab-run.sh` counts boots in
`/var/lib/labstage`, so every boot of a disk is a step in a sequence.

| guest | boot | action on ctl.img | done? |
| --- | --- | --- | --- |
| apexmig-a (512 MiB ESP) | 1 | `act-migrate-3.sh` | not yet |
| apexmig-b (2 GiB ESP) | 1 | `act-prep-3.sh` | disk not built |
| apexmig-b | 2 | `act-unit-3.sh` | — |
| apexmig-b | 3 | `act-check-3.sh` (the migrated boot) | — |

## DONE

- Orientation. Worktree `/var/tmp/apex-work/wt-sdboot-migrate-3` created on
  branch `task/sdboot-migrate-3` from `origin/roadmap/v2.2` @ `f3b1b3d4`.
- Lab scripts for both runs written into `/var/lab-scratch/sdboot-migrate-2/`
  (the rig dir, reused as `/work`), all suffixed `-3`: `act-migrate-3.sh`,
  `act-prep-3.sh`, `act-unit-3.sh`, `act-check-3.sh`, plus a copy of the
  worktree's `apex-boot-migrate-confirm.service`. This unit's own outputs and
  launchers are in `/var/lab-scratch/sdboot-migrate-3/`.

## IN PROGRESS

- **Run A's `bootc install` is running** as user unit `sdb3-install-a`
  (`systemctl --user is-active sdb3-install-a`), log
  `/var/lab-scratch/sdboot-migrate-3/install-a.log`, wrapper
  `/var/lab-scratch/sdboot-migrate-3/run-install-a.sh`. It goes through
  `tests/lab/bootc-install-lab` → `nvram-guard` → `--generic-image`.
- **The item-3 splice is written and UNCOMMITTED** in
  `files/system/libexec/apex-boot-migrate`: a new `count_staged_entry()` just
  above `cmd_stage()`, a `BOOT_TRIES` constant after `ROOT_SLACK_MIB`, and a
  one-line call in `cmd_stage` right after the `grub-boot-wiped` check. It is
  NOT the predecessor's patch — that one no longer applies — and it differs
  from it in one way that matters: the rename is verified by looking at the
  filesystem (`[ ! -e "$counted" ] || [ -e "$ent" ]`) instead of trusting
  `mv`'s exit status, because `mv` on FAT under a confined domain can report
  success and move nothing. `bash -n` clean, `shellcheck` adds no new finding,
  and `tests/test-boot-migrate.sh` is **83 passed, 0 failed** with it in.
  It stays uncommitted until run B's migrated boot says whether blessing works.

## FOUND

- **The card says the rig is intact. The disk image is NOT — run A's install
  never finished, and a partition table read as "installed" would have been
  believed.** `/var/lab-scratch/sdboot-migrate-2/apexmig-a.img` has a complete
  GPT (`p1 BIOS-BOOT 1 MiB / p2 EFI-SYSTEM 512 MiB ef00 / p3 root 44.5 GiB
  8304`) and 13 GB allocated, which is what made it look done. Mounted it
  read-only through a loop device and the inside is a corpse:
    * `p2` (the ESP) — **4.0 KiB used of 511 MiB. Completely empty.** No
      `/EFI/fedora`, no `/EFI/BOOT`, no bootloader of any kind.
    * `p3` root (btrfs, correct) — `/boot` exists and holds only an empty
      `efi/`. **No `loader/entries`, no kernel, no grub2.**
    * `/ostree/repo` is 13 GB (mtime 10:44) but `/ostree/deploy` is empty.
  So `bootc install to-disk` got as far as pulling the image into the repo and
  died before deploying it or writing a bootloader. The disk **cannot boot**.
  This is exactly the trap in the "permission denied is not absence" and
  "a gate that inspects nothing" memories: the partition table is evidence that
  sgdisk ran, not that the install finished. Had I booted it on the card's word
  I would have got 1800 s of nothing and called `console=ttyS0` the suspect.
  **Run A's disk has to be reinstalled from scratch.** What IS reusable, and
  is the expensive part: `localhost/apex-sdmig:v1` (11.5 GB, no 11 GB pull
  needed) and `localhost/apex-bootlab` (1.39 GB — the qemu container; the HOST
  has no `qemu-system-x86_64` at all, so that image is load-bearing).
- APEX's default ESP on a GRUB/ostree `bootc install to-disk` **is 512 MiB** —
  the one question act-unit-2.sh's last line was left open to answer. The GPT
  is trustworthy for this even though the install died, because partitioning is
  the step that demonstrably completed.
- **The rename patch no longer applies.** `sdboot-migrate-2-rename.patch` was
  cut against `234cc4dc`; `files/system/libexec/apex-boot-migrate` has moved
  **+528/-33 lines** since, from `task/migrate-preconditions` and
  `task/sdboot-xbootldr` landing (new `find_xbootldr`, `esp_candidates`,
  `bitlocker_volumes`, `windows_loader_on_esp`, `cmd_explain`, `verdict`,
  `APEX_MIGRATE_FAKEROOT`/`MOUNTDIR`). `git apply --check` fails at line 106.
  The splice has to be redone by hand at the same two anchors, which both still
  exist: the `ROOT_SLACK_MIB` constant block, and immediately after the
  `grub-boot-wiped` check / before `echo staged > "$STATE/phase"` in `cmd_stage`
  (now ~line 1082).
- Memory at start: 29 Gi total, 24 Gi available, no other qemu/podman/bootc
  process running (`ps aux` checked). `/var` has 254 GB free.

## BLOCKED ON

- nothing
