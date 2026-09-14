# later
items: L-001, L-002, L-003
repo: apex-os
worktree: /var/tmp/apex-work/wt-later
branch: task/later-tpm-qualification

## NEXT

Collect the two firmware experiments, then land the diagnosis:
  podman wait later-r5-s3-fw2025 later-r5-s3-nosmm
  podman logs later-r5-s3-fw2025 > /var/tmp/apex-work/scratch-later/r5-fw2025.log 2>&1
  podman logs later-r5-s3-nosmm  > /var/tmp/apex-work/scratch-later/r5-nosmm.log  2>&1
Both run `luks-s3` from a COPY of the scripts at /var/tmp/apex-work/scratch-later/
tree-exp (so the worktree stays editable), with APEX_BOOTLAB_FW pointing at a
pre-populated firmware cache: fw-2025 = edk2-ovmf-20250812-18.fc43's 4M secboot
build (varies ONLY the edk2 version), fw-nosmm = the in-image non-secboot 4M
build (varies SMM+SB). Whichever resumes decides whether the S3 criterion can be
measured at all. Then: `## FOUND` entry 1 is the result to write up in
docs/boot-v2.md and to turn into the scenario's precise COULD-NOT-RUN.

NEVER `nohup podman run &` — `podman run -d` then `podman wait`. NEVER edit
files/scripts/boot-v2/** while a container is executing run-scenarios from it.

## DONE
- Branch landed on roadmap/v2.2 this round as merge 654fa854. Tip c84b2f83 is
  pushed; nothing of mine is unlanded.
- Round 26 results (collected by the orchestrator after the 12:06 reboot, logs
  at scratch-later/r27-*.log, State.FinishedAt verified on each):
  * luks-tpm-clear       20 passed, 0 failed — COMPLETE END TO END.
  * luks-firmware-change 18 passed, 0 failed, 1 could-not-run (PCR 0).
  * luks-s3 (r3)          4 passed, 1 failed  -> fixed in c84b2f83.
  * luks-s3 (r4)          6 passed, 0 failed, 1 could-not-run.
- Commits on the branch: d362e952 lab machinery; 378d8ddd the three scenarios;
  e2345ed7 the TPM-clear recovery no-op; 13be81a8 asserted-nothing + a control
  that could never fail; c84b2f83 `mem` is not S3 + the serial-field abort.

## IN PROGRESS
- Two firmware experiments running as detached containers (see NEXT).
- docs/boot-v2.md: the Recovery table row for a cleared TPM still documents the
  single-command `systemd-cryptenroll --wipe-slot=tpm2 --tpm2-public-key=…`,
  which is measured to be a silent no-op; and the doc has no record of the three
  completed scenarios.
- run-scenarios ~line 1405, scenario_luks_s3's `""` branch: it says "the guest
  reported no s3 field" while two other witnesses sit unread on disk.

## FOUND
- **THE QMP WAKER WORKS. THE FIRMWARE DOES NOT COME BACK.** The round-26
  could-not-run was not the waker: scratch-later/r4-luks-s3/serial-luks-s3.wake.json
  records suspended_seen=true at 26.176 s, wakeup_sent=true, resumed_seen=true,
  woke_at 26.692 — qemu saw the guest suspend, woke it, and saw it run again.
  What failed is OVMF's S3 resume, in serial-luks-s3.ovmf.log:
    8184  SecCoreStartupWithStack  (the resume boot)
    8185  SEC: S3 resume (with PEI decompression)     <- S3 was detected
    8281  PeiInstallPeiMemory MemoryBegin 0x7EF70000, MemoryLength 0x90000
    8303  PopulateMemoryTypeInformation: No Memory Type Information HOB found
    8304  Memory Type Information HOB not found during memory services
          initialization but PCD was set
    8307  ASSERT MemoryServices.c(203)
  and a DEBUG-build ASSERT deadloops, so qemu was killed at the 240 s timeout.
  On the COLD boot the same firmware orders it the other way — PeiVariable.efi
  at 106, RefreshMemTypeInfo at 110, PublishPeiMemory at 118 — so the HOB exists
  before permanent memory is installed. On the S3 path OVMF's PlatformPei
  publishes memory in its own entry point, before the variable PPI is
  dispatched, and PeiCore asserts. edk2-ovmf-20260812-4.fc43, qemu 10.1.5.
  Nothing here is an APEX defect and nothing here is a lab defect.
- THE DOCUMENTED TPM-CLEAR RECOVERY IS A NO-OP THAT REPORTS SUCCESS.
  `systemd-cryptenroll --wipe-slot=tpm2 --tpm2-device=… --tpm2-public-key=… VOL`
  prints "This PCR set is already enrolled, executing no operation." and exits 0
  with the header byte-identical; the de-dup compares the PUBLIC KEY and the PCR
  set against the header and fires BEFORE the wipe is acted on, so the TPM's
  state plays no part. Wiping in a SEPARATE invocation first produces a new
  sealed blob and works. systemd 258.10-1.fc43. Product finding: it is the
  recovery a user is told to run after a firmware TPM clear.
- PCR 11 IS ALL ZEROS ON THE L16 (measured 2026-09-13, read-only). The shipped
  image boots GRUB via bootupd and only sd-stub extends PCR 11, so the
  signed-PCR-11 policy boot-v2 chose has nothing to bind to on a default
  install. That is why L-003 cannot be flipped on for the shipped image.
- L-002 IS NOT A DEFAULT TO FLIP: the installer cannot encrypt at all
  (installer/apex-install:739-755, :1173, :1223; no encryption page in
  apex-installer-gui:682-688), there is no in-place path, the initramfs keymap
  is always `us` (Containerfile.core:2104-2107), plymouth's `message()` is a
  no-op (apex-os.script:230), and `cryptsetup` is an unpinned transitive
  dependency asserted nowhere. Two shipped files already assert LUKS2+TPM2 as
  settled fact (apex-session-select:18-22 and its sudoers file:9-12).
- PCR 0 CANNOT BE MOVED IN THIS LAB, structurally: it needs a second OVMF build
  differing ONLY in code, and the two 4M builds differ in Secure Boot
  enforcement too. The dbx update (PCR 7, 76 -> 21340 bytes) is the faithful
  stand-in for what fwupd does. Recorded as a limit, not a defect.
- Older, still true: the lab image ships no diffutils; the APEX initramfs has no
  `sync` or `dd`; tpm2_createprimary's YAML has no `name:` line (use
  tpm2_readpublic); swtpm state does persist across host sessions.

## BLOCKED ON
- Nothing.
