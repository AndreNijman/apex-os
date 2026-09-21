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

- Copy `files/system/units/apex-boot-migrate-confirm.service` from the worktree
  into /var/lab-scratch/sdboot-migrate-2/, write `act-migrate-3.sh` (act-migrate.sh
  + the new `precheck --explain` verb), `./setctl.sh act-migrate-3.sh
  /var/tmp/apex-work/wt-sdboot-migrate-3/files/system/libexec/apex-boot-migrate`,
  then boot run A (512 MiB refusal) with:
  `sudo /var/tmp/apex-work/wt-sdboot-migrate-3/tests/lab/nvram-guard --label apexmig-a --
   podman run --rm --device /dev/kvm -v /var/lab-scratch/sdboot-migrate-2:/work
   localhost/apex-bootlab -c '/work/boot-mig.sh /work/apexmig-a.img
   /work/apexmig-a-3.serial 1800 --ctl /work/ctl.img'`
  under `systemd-run --user` (never nohup).

## DONE

- Orientation. Worktree `/var/tmp/apex-work/wt-sdboot-migrate-3` created on
  branch `task/sdboot-migrate-3` from `origin/roadmap/v2.2` @ `f3b1b3d4`.

## IN PROGRESS

- nothing committed yet.

## FOUND

- **The rig is genuinely intact and run A's disk is already installed.**
  `/var/lab-scratch/sdboot-migrate-2/apexmig-a.img` is a COMPLETED `bootc
  install to-disk`: 45 GiB disk, `p1 BIOS-BOOT 1 MiB / p2 EFI-SYSTEM 512 MiB
  ef00 / p3 root 44.5 GiB 8304`, 13 GB actually allocated. So APEX's own
  default ESP on a GRUB/ostree `to-disk` install **is 512 MiB** — the question
  sdboot-migrate-2 left open in act-unit-2.sh's last line is already answered by
  the partition table it wrote. `localhost/apex-sdmig:v1` (11.5 GB) and
  `localhost/apex-bootlab` (1.39 GB, the qemu container — the HOST has no
  qemu-system-x86_64 at all) both still exist.
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
