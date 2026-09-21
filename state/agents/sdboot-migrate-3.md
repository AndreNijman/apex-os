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

- WAIT for `systemctl --user is-active sdb3-install-b` to go inactive, confirm
  `install rc=0` in `/var/lab-scratch/sdboot-migrate-3/install-b.log`, and
  loop-mount `apexmig-b.img` p1 to confirm the 2 GiB ESP is POPULATED (a
  complete GPT proves nothing — see FOUND). Then run B, boot 1:
  `cd /var/lab-scratch/sdboot-migrate-2 && ./setctl.sh act-prep-3.sh rsync-3`
  then `systemd-run --user --unit=sdb3-boot-b1 --collect
  /var/lab-scratch/sdboot-migrate-3/run-boot-b.sh` (copy run-boot-a.sh and swap
  `apexmig-a.img`→`apexmig-b.img`, `apexmig-a-3.serial`→`apexmig-b-3.serial`;
  keep `:z` on the volume and keep `--oci /work/oci-dummy-3.img`, BOTH are
  required — see FOUND).

### Guest bookkeeping — the persistent-NVRAM rig makes boot ORDER matter

`boot-mig.sh` keeps one `VARS` file per guest and `lab-run.sh` counts boots in
`/var/lib/labstage`, so every boot of a disk is a step in a sequence.

| guest | boot | action on ctl.img | done? |
| --- | --- | --- | --- |
| apexmig-a (512 MiB ESP) | 1 | none (ctl on vdb, see FOUND) | wasted |
| apexmig-a (512 MiB ESP) | 2 | `act-migrate-3.sh` | **DONE — refused** |
| apexmig-b (2 GiB ESP) | 1 | `act-prep-3.sh` + `rsync-3` | install running |
| apexmig-b | 2 | `act-unit-3.sh` + engine + confirm.service | — |
| apexmig-b | 3 | `act-check-3.sh` (the MIGRATED boot) | — |

Every boot: `./setctl.sh <action> <extra files…>` first, and the qemu launch
needs **both** `-v …:/work:z` and `--oci /work/oci-dummy-3.img`.

Evidence file: `ROADMAP/evidence/sdboot-migrate-3-20260922-lab.md` (committed
with run A in it already), NOT the
`sdboot-migrate-2-20260921-lab.md` the inherited card names — that name was a
dead unit's plan for a run that never happened, and this is unit 3 measuring on
09-22.

## DONE

- **Committed and pushed `5cdaaa39`** on `task/sdboot-migrate-3`:
  `ROADMAP/evidence/sdboot-migrate-3-20260922-lab.md`, sections 0 and 1 —
  the four rig defects and the whole of run A. Run B's sections are still to
  be appended to the same file. NOT yet LANDABLE: the run-B half is missing
  and the item-3 decision is not taken.

- **RUN A IS MEASURED AND IT REFUSED.** Serial log
  `/var/lab-scratch/sdboot-migrate-2/apexmig-a-3.serial`, guest boot 2, real
  APEX image on btrfs, GRUB 2.12, `store=ostreeContainer` — the L16's shape.
  `precheck --explain` verdict table, verbatim:

      OK      uefi              booted through UEFI
      OK      on-ostree         booted store is ostreeContainer …
      OK      no-staged-update  no ostree deployment is staged for the next boot
      REFUSE  no-rsync          rsync is not in this image …
      OK      bootc-new-enough  install to-existing-root has --composefs-backend
      OK      secure-boot       Secure Boot is not enforcing …
      OK      root-space        30 GiB free, 24 GiB needed
      OK      esp-choice        bootc will write PARTUUID 77223857-… of 1 ESP(s)
      REFUSE  esp-too-small     The ESP has 503 MiB free, needs 1173 MiB,
                                short by 669 MiB.

  `precheck` rc=10, `auto` rc=10, phase stayed `not started`, `/var/lib/apex/
  boot-migrate` was never created, BootOrder and every Boot#### byte unchanged,
  `/sysroot/boot` intact. nvram-guard: `verified — boot variables identical`.
  The root-space check that `task/sdboot-migrate-2` landed also fired for real
  for the first time here and passed correctly (repo measured at 12,701,900 KiB
  → 24 GiB needed, 30 GiB free).

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
- **The migrated machine mounts `/boot` READ-ONLY, and an ordinary composefs
  machine mounts it rw. That difference is the whole of item 3.**
  The migrated cmdline carries `systemd.mount-extra=UUID=…:/boot:auto:ro`
  (`ROADMAP/evidence/sdboot-migrate-20260921-lab.md:176`), where the ordinary
  composefs + systemd-boot guest was measured at `/dev/vda2 /boot vfat
  rw,nosuid,nodev,noexec,relatime,…`
  (`ROADMAP/evidence/sdboot-image-20260920-decision.md:45`). On the composefs
  path the ESP **is** `/boot`, and `systemd-bless-boot` stripping `+N-M` from
  an entry filename is a rename *on that filesystem*. So on a migrated machine
  the counter can be written by the stage and then never stripped — not for the
  SELinux reason APEX already fixed, but because the filesystem is ro. Three
  boots later systemd-boot counts the entry out and rolls the machine back on
  its own, which is strictly worse than today's inert `Wants=`.
  **This is the hypothesis run B exists to decide, and it is the reason item 3
  was correctly left uncommitted rather than reasoned through.**
- APEX has ALREADY solved the other half of this on the ordinary path, which is
  why the ro question is the only one left: `files/system/units/
  10-apex-bless-boot-esp.conf` runs `systemd-bless-boot` as `bootupd_t` because
  `init_t` may not rename `dosfs_t`, with the missing permission granted by
  `files/system/selinux/apex_sdboot.te`, and `Containerfile.base` asserts
  `semodule -l` still lists the module. So if run B shows a failure to bless, a
  reader will be tempted to blame SELinux; `act-check-3.sh` prints the unit's
  SELinuxContext, the AVC log and the `/boot` mount options side by side so the
  two causes cannot be confused.
- **A second reason the predecessor's guests could never have booted, and it is
  not in any card: the rig directory has the wrong SELinux label.** The
  documented recipe is `podman run … -v /var/lab-scratch/<dir>:/work
  localhost/apex-bootlab …` with no `:z`. `/var/lab-scratch/sdboot-migrate-2`
  is `unconfined_u:object_r:var_t:s0`, the container is `container_t`, and the
  guest launch dies with `/bin/bash: line 1: /work/boot-mig.sh: Permission
  denied` — an **exec** denial that reads like a mode bit, on a file that is
  `-rwxr-xr-x`. Unit 1's directory `/var/lab-scratch/sdboot-migrate` is
  `container_file_t`, which is why its ~30 boots worked and why nobody noticed
  the recipe was incomplete. Fix: `-v …:/work:z`. Measured on this machine,
  `getenforce` = Enforcing. Anyone copying that recipe into a fresh scratch
  directory hits this immediately.
- **`boot-mig.sh --ctl` alone puts the control disk on `vdb`, and `lab-run.sh`
  looks for it on `vdc`.** A third rig defect, measured on the first real boot
  of this program's run A: `LAB-CTL: FAILED to mount /dev/vdc` / `LAB-OCI:
  mounted` / `LAB-ACTION: none on the control disk`, on a boot that otherwise
  worked perfectly. `boot-mig.sh` appends the OCI drive as `hd1` and the CTL
  drive as `hd2`, so `vdc` is only correct when BOTH are passed; the image's
  baked `lab-run.sh` hardcodes `/dev/vdc` for ctl and `/dev/vdb` for oci. Every
  predecessor run passed an OCI disk, so the coupling was never visible. It
  fails **silently and successfully** — the guest boots, powers off, exits 0,
  and the serial log says the experiment simply had nothing to do. Worked
  around with a 4 MiB `oci-dummy-3.img` in the `--oci` slot rather than by
  editing the baked driver.
- **The published `ghcr.io/andrenijman/apex-os:daily` image cannot migrate at
  all, because it has no `rsync`.** Measured in run A: `precheck --explain`
  returns `REFUSE no-rsync` and — because `no-rsync` is evaluated BEFORE the
  ESP checks — plain `precheck` and `auto` both stop there and never reach
  `esp-too-small`. `roadmap/v2.2`'s `Containerfile.base:1283` does `dnf5 -y
  install rsync` and asserts `command -v rsync` at 1296, so this is the daily
  tag lagging the branch rather than a defect in the branch. Two things follow:
    * **`precheck --explain` earned its keep on its first real use.** Without
      the verb `task/migrate-preconditions` landed, run A would have produced
      `REFUSED [no-rsync]` and NOTHING about the ESP, and the 512 MiB refusal
      this unit exists to measure would have been unmeasurable on this image.
    * Run B needs rsync present or its migration cannot start. Handled as a
      named lab accommodation in `act-prep-3.sh`, not by editing the engine:
      the host's `/usr/bin/rsync` is carried on the control disk and installed
      to `/usr/local/bin/rsync` (writable `/var/usrlocal` on ostree, and ahead
      of `/usr/bin` on systemd's default PATH). Every shared library it needs
      was checked present in the image first, and the binary was executed
      inside the image before being relied on: rsync 3.5.0, ACLs + xattrs +
      hardlinks compiled in, which is what `rsync -aAXH --delete` needs.
- **`to-filesystem-lab`, as the predecessor left it, cannot produce a working
  disk — it makes no BIOS-BOOT partition.** First run of it in this program:

      /usr/sbin/grub2-install: error: filesystem `btrfs' doesn't support blocklists.
      error: boot data installation failed: installing component BIOS to device
             /dev/loop1: installing GRUB on /dev/loop1

  bootupd installs the BIOS component as well as the EFI one; with no `ef02`
  partition `grub2-install --target i386-pc` has nowhere to embed core.img, and
  on btrfs it cannot fall back to blocklists the way it could on ext4. `bootc
  install to-disk` creates that partition itself — run A's disk has `p1 1M EF02
  BIOS-BOOT` — so a hand-partitioned lab disk has to as well or it is not the
  same machine. The install still reaches "deploy" and writes loader entries
  into the root's `/boot`, so **the failure leaves a disk that looks plausible
  and has a completely empty ESP** — the same shape as the corpse the inherited
  card called an intact rig. Patched in
  `/var/lab-scratch/sdboot-migrate-3/to-filesystem-lab-3` (ef02 1 MiB + ef00
  ESP + 8300 root, parts renumbered), with the measured error in the comment.
- Both of this unit's launcher wrappers reported `rc=0` for a command that
  exited 1: `echo "=== $(date -Is) … rc=$? ==="` runs the `date` command
  substitution first, which resets `$?`. Fixed by capturing `rc=$?` on its own
  line. The real exit status was in `to-filesystem-lab`'s own `install rc=1`.
- `lab-run.service` in `localhost/apex-sdmig:v1` bakes `TimeoutStartSec=900`,
  set by the predecessor against a 2 GB fedora-bootc guest. Run B's migration
  copies an 11.5 GB image into podman storage and then writes a third copy into
  `/composefs`; 15 minutes is not a safe ceiling and systemd killing the stage
  mid-flight would give a run that is safe and proves nothing. `daemon-reload`
  does not re-arm a RUNNING job's timer, so run B is deliberately THREE boots
  with the drop-in written on boot 1.

## BLOCKED ON

- nothing
