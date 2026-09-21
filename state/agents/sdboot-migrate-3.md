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

- **Run B boot 3 — the migration RETRY on a grown disk — is RUNNING** as user
  unit `sdb3-boot-b2r` (launched 03:55 AWST, ceiling 5400 s), action
  `act-unit-3b.sh`. Boot 2's attempt died with ENOSPC (see FOUND, the headline);
  the host grew `apexmig-b.img` from 45 G to 90 G and p3 from 43 GiB to 88 GiB
  with `sgdisk -e` + `growpart` + `btrfs filesystem resize max`, leaving 65 GiB
  free, and the retry action clears the partial podman storage the failed
  attempt left behind. When it powers off, read `auto rc=` and the
  `### the entry the stage wrote — IS IT COUNTED?` block.
- Then boot 4, the MIGRATED boot, which is where item 3 is decided:
  `cd /var/lab-scratch/sdboot-migrate-2 && ./setctl.sh act-check-3.sh`,
  set the qemu ceiling in `/var/lab-scratch/sdboot-migrate-3/run-boot-b.sh`
  back to 1800, then `systemd-run --user --unit=sdb3-boot-b4 --collect
  /var/lab-scratch/sdboot-migrate-3/run-boot-b.sh`.
  **Never pass `--fresh-nvram`** — `BootNext`, which the commit writes, lives in
  `/work/nvram-persist/VARS-apexmig-b.img.fd`.
- Then append run B's sections to
  `ROADMAP/evidence/sdboot-migrate-3-20260922-lab.md` (run A is already in it
  and committed as `5cdaaa39`), commit, push, and mark `## LANDABLE <sha>`.

### The item-3 decision rule, written down BEFORE the log exists

Read the entry filename on the migrated boot's live ESP:

| observed | meaning | verdict |
| --- | --- | --- |
| `name.conf` (bare) | sd-boot counted it AND bless stripped it | commit the splice, and do sdboot-migrate-2's NEXT item 2 in full (confirm unit `Wants=`→`Requires=`, the Containerfile.base assertion ~1292-1303, the two `tests/test-boot-migrate.sh` pins ~247-256) |
| `name+2-1.conf` + **EROFS** | sd-boot counted it; bless could not write a ro `/boot` | **drop the splice.** This is the migration-specific cause and it is the answer. Do not try to fix the karg in this unit |
| `name+2-1.conf` + **AVC on dosfs_t** | the SELinux cause, which the branch already fixes and this stale image lacks | inconclusive on its own — re-run boot 3's check after `setenforce 0` to separate it from the ro question before writing any verdict |
| `name+3-0.conf` | sd-boot never counted it at all | drop the splice; the counter is not even reaching the loader |

Whatever the answer, it goes in the evidence doc. A rename that is not
*observed* to be blessed is worse than no rename: the counter decrements on
every ordinary boot afterwards and sd-boot rolls a WORKING machine back on the
fourth one.

### Guest bookkeeping — the persistent-NVRAM rig makes boot ORDER matter

`boot-mig.sh` keeps one `VARS` file per guest and `lab-run.sh` counts boots in
`/var/lib/labstage`, so every boot of a disk is a step in a sequence.

| guest | boot | action on ctl.img | done? |
| --- | --- | --- | --- |
| apexmig-a (512 MiB ESP) | 1 | none (ctl on vdb, see FOUND) | wasted |
| apexmig-a (512 MiB ESP) | 2 | `act-migrate-3.sh` | **DONE — refused** |
| apexmig-b (2 GiB ESP) | 1 | `act-prep-3.sh` + `rsync-3` | **DONE** |
| apexmig-b | 2 | `act-unit-3.sh` (the migration) | **DONE — ENOSPC, see FOUND** |
| apexmig-b | 3 | `act-unit-3b.sh` (retry, grown disk) | RUNNING |
| apexmig-b | 4 | `act-check-3.sh` (the MIGRATED boot) | — |

Every boot: `./setctl.sh <action> <extra files…>` first, and the qemu launch
needs **both** `-v …:/work:z` and `--oci /work/oci-dummy-3.img`.

Evidence file: `ROADMAP/evidence/sdboot-migrate-3-20260922-lab.md` (committed
with run A in it already), NOT the
`sdboot-migrate-2-20260921-lab.md` the inherited card names — that name was a
dead unit's plan for a run that never happened, and this is unit 3 measuring on
09-22.

## DONE

- **Committed and pushed, `task/sdboot-migrate-3` now at `ebbbab54`:**
  * `5cdaaa39` — evidence sections 0 and 1: the four rig defects and the whole
    of run A (the 512 MiB refusal, the `no-rsync` discovery, the root-space
    check's first real firing).
  * `ebbbab54` — evidence sections 2 and 3: run B's clean precondition table
    and the ENOSPC defect, **plus** the one-comment correction to
    `ESP_SLACK_MIB`'s header in `files/system/libexec/apex-boot-migrate`
    (it said "two deployments'" while the check does `per * 3`).
  `tests/test-boot-migrate.sh` 83/83 on the pushed tip.
  **NOT yet LANDABLE** — the migrated boot has not happened and the item-3
  decision is not taken. The `count_staged_entry` splice is deliberately still
  uncommitted in the worktree; a backup of the spliced file is at
  `/var/lab-scratch/sdboot-migrate-3/engine-with-splice.bak`.

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

- **THE HEADLINE. `root-space` passed and the machine ran out of disk anyway.**
  Run B boot 2, the real migration on the 2 GiB-ESP guest. The precheck that
  `task/sdboot-migrate-2` landed *specifically to prevent this* said:

      OK  root-space   28 GiB free, 24 GiB needed

  and then, ~70 seconds later:

      Copying local image docker://localhost/apex-sdmig:v1 to containers-storage:localhost/bootc ...
      fatal msg="writing blob: storing blob to file
        \"/var/tmp/container_images_storage799695552/89\":
        no space left on device"
      apex-boot-migrate: FAILED [image-unavailable]
      auto rc=1

  **Why the model is wrong:** it estimates peak cost as `du -sk
  $SYSROOT/ostree/repo * 2 + 512 MiB`, on the reasoning (docs/update-cost.md)
  that the temporary podman copy and the permanent `/composefs` copy coexist.
  `bootc image copy-to-storage` is itself a **two-stage** operation — skopeo
  stages the blobs into `/var/tmp/container_images_storage*` and *then* writes
  them into containers-storage — so the temporary copy is two copies, and both
  live on the same filesystem as the repo. The host image file grew 13 GB →
  43 GB during the attempt, i.e. the guest wrote ~28 GB of a 43 GiB root before
  ENOSPC, all of it before `/composefs` was touched at all. The `du` estimate is
  also an underestimate of what lands: the ostree repo is deduplicated, and the
  containers-storage extraction is not.
  **The multiplier is wrong, not the idea.** The retry, on an 88 GiB root, is
  measuring the true peak directly as host-image growth from the 13 GB the
  install left: it passed 43 GB (where the 43 GiB guest died), then 51, 53,
  **54 GB** and was still climbing — i.e. **more than 40 GB of growth against a
  24 GiB prediction, already over 3x the repo**. Whoever fixes the check should
  take the multiplier from this measured peak and update the
  `tests/test-boot-migrate.sh` assertion that currently pins it ("the root check
  sizes for two image copies"), plus the paragraph in `docs/update-cost.md`
  that produced the 2x by argument.
- **The containment held, which is the other half of the result.** After a
  hard failure in the middle of the migration: `phase: not started`, no
  `boot entry`, `BootOrder` byte-identical, `BootCurrent: 000A` still
  `\EFI\fedora\shimx64.efi`, the ESP still at **7.6 MiB used of 2.0 GiB**
  with no `loader/entries` at all, and `/sysroot/boot` still holding
  `grub2 loader loader.1 ostree bootupd-state.json`. `auto` returned **1**, not
  10 — a failure, not a refusal — so `apex update` treats it as an error rather
  than as "carry on", which is right. The engine also correctly saved the
  removable-media fallback (949424 bytes) before touching anything.
- The 2 GiB ESP passes its check with room: `OK esp-space 2036 MiB free,
  1173 MiB needed`, and with rsync present the `tools` check turns into
  `OK tools mkfs.vfat, rsync and podman are all in this image` — confirming the
  run-A `no-rsync` refusal was the image, not the engine.

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
- **`lab-run.sh` pipes the action through `sed 's/^/LAB-ACT: /'`, and sed
  BLOCK-buffers into a pipe.** Nothing an action prints reaches the serial log
  until the whole action ends (or 4 KiB accumulates). A long step — the
  migration is ~10 minutes — therefore looks identical to a hung guest:
  `LAB-ACTION-BEGIN` and then silence. Do not interpret a quiet serial log as a
  stall; watch `systemctl --user is-active` on the launcher unit instead. A
  `stdbuf -oL`/`--unbuffered` in the baked driver would fix it, but that means
  an image rebuild and is not worth one here.
- Both of this unit's launcher wrappers reported `rc=0` for a command that
  exited 1: `echo "=== $(date -Is) … rc=$? ==="` runs the `date` command
  substitution first, which resets `$?`. Fixed by capturing `rc=$?` on its own
  line. The real exit status was in `to-filesystem-lab`'s own `install rc=1`.
- **The published image is ALSO missing both halves of the blessing fix, which
  bounds what run B can prove about item 3.** Measured in run B boot 1 on the
  guest itself: `semodule -l | grep -c apex_sdboot` → **0**, and
  `/usr/lib/systemd/system/systemd-bless-boot.service.d/` → **does not exist**.
  Both are shipped by `Containerfile.base` on `roadmap/v2.2` (the drop-in at
  1145-1146, asserted at 1244; the policy module is
  `files/system/selinux/apex_sdboot.te`), so this is the daily tag lagging the
  branch for a third time — the same lag that removed `rsync`. Consequence for
  the decision: on THIS guest, `systemd-bless-boot` runs as `init_t`, which
  Fedora 43 allows no rename on `dosfs_t`, so a failure to strip the counter
  has **two** possible causes and they must not be conflated.
  They are distinguishable and `act-check-3.sh` prints both:
    * an **AVC / EACCES** on the rename → the SELinux cause, already fixed on
      the branch, says nothing about the migrated path;
    * **EROFS**, with `/boot … ro` in the mount line → the migration-specific
      cause, and the one that decides item 3.
  Installing the drop-in as a lab accommodation would make this WORSE, not
  better: its own comment records that with the policy module absent and
  SELinux enforcing, `setexeccon` SUCCEEDS and the kernel then denies execve on
  the entrypoint, so the unit fails harder and for a third reason. Left alone
  deliberately.
- `lab-run.service` in `localhost/apex-sdmig:v1` bakes `TimeoutStartSec=900`,
  set by the predecessor against a 2 GB fedora-bootc guest. Run B's migration
  copies an 11.5 GB image into podman storage and then writes a third copy into
  `/composefs`; 15 minutes is not a safe ceiling and systemd killing the stage
  mid-flight would give a run that is safe and proves nothing. `daemon-reload`
  does not re-arm a RUNNING job's timer, so run B is deliberately THREE boots
  with the drop-in written on boot 1.

## BLOCKED ON

- nothing

---

## ORCHESTRATOR NOTE — 2026-09-22 03:55 AWST, from initramfs-slim-2's landing

Two facts that bear on your unit, found by another agent this round:

1. **katana's root filesystem is 96% full** — 907 G of 954 G. `apex-boot-migrate
   precheck` there answers `OK root-space: 43 GiB free, 43 GiB needed`, which is
   a **zero-margin pass**. If any part of your work ends up running `stage` on
   katana rather than in an L16 guest, that OK can become a failure without any
   code changing. Measure before you assume it will pass.

2. **`esp-too-small` is retired on the current image.** The initramfs work took a
   deployment to 100.9 MiB, so `precheck` needs 351 MiB where it needed 1173, and
   katana now answers "This machine can migrate" for the first time. Your refusal
   path for a 512 MiB ESP still has to hold — but the threshold it is refusing
   against has moved, so re-read the current figure rather than the one in any
   older card or evidence file.

Also closed by that landing, so do not spend time on them: `sdboot-xbootldr`
NEXT #2 (cross-build reproducibility), #3 (`rsync` now present) and #6 (the
cross-disk note has fired on real hardware).
