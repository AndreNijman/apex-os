# later
items: L-001, L-002, L-003
repo: apex-os
worktree: /var/tmp/apex-work/wt-later
branch: task/later-tpm-qualification

## NEXT

Nothing is running and nothing is half-finished. Branch tip 985ae69a is pushed;
roadmap/v2.2 266dcc57 is merged in. All containers from this unit are removed.

L-001 cannot be closed from this machine. Its acceptance word is "Real TPM" and
state/queue.json is explicit: L-002/L-003 "must stay blocked until a real TPM
enrol-and-recover cycle has been done on hardware". Every measurement this unit
has ever made used swtpm in a VM. The L16's TPM is off-limits to this program,
so the next step is a decision by Andre, not a task an agent can pick up:
nominate a machine somebody is willing to clear the TPM on, or accept the VM
qualification and say so in the roadmap.

If more VM work IS wanted, the two honest candidates, in order:
  1. Re-run luks-tpm-clear and luks-firmware-change on the fw-2025 firmware.
     They passed on the shipped one, but S3 did not, and nobody has checked
     whether the older edk2 changes their results.
  2. A no-TPM machine arm. `L-002 does not strand users` covers hardware with
     no TPM at all, and no scenario models it.

NEVER `nohup podman run &`. `podman run -d` then a FOREGROUND `podman wait`, and
check `State.FinishedAt` and `State.ExitCode` before believing any log.
NEVER edit files/scripts/boot-v2/** while a container executes run-scenarios
from it. The containers run from a COPY at scratch-later/tree-exp, so the
worktree stays editable while they run.

## DONE
- Round 28. Merged roadmap/v2.2 266dcc57 into the branch, then four commits,
  each pushed as it was made: a9c9ebe6 (the two harness defects), 8ad79870
  (docs/boot-v2.md), fc0ac3f5 (stop-slop on the prose added this round),
  985ae69a (the killed-run regression test, and an overclaim withdrawn).
- tests/test-boot-v2.sh is 78 passed / 0 failed. All four gates green.
- tree-exp WAS refreshed from the worktree at the end of this round, so it no
  longer lags. Check `diff -rq` anyway before the next run.
- L-001 evidence updated with round 28 appended, rounds 25/26/27 verified still
  present afterwards. L-002 and L-003 deliberately left `blocked`.
- The r5 firmware experiments the card told me to collect were NOT results.
  Both containers had exited 143 at the same nanosecond and both logs ended
  "6 passed, 0 failed" anyway. Re-ran foreground as r6; logs saved at
  scratch-later/r6-s3-fw2025.log and r6-s3-nosmm.log.
- All later-* containers removed, and the r5/r6 work dirs' stale witnesses are
  the only thing left in scratch-later.

## IN PROGRESS
- Nothing.

## FOUND
- **THE S3 CRITERION IS MEASURABLE AND IT PASSES — ON edk2-20250812.**
  luks-s3 gives 17 passed / 0 failed, exit 0, against
  edk2-ovmf-20250812-18.fc43's 4M secboot build. Volume open across S3; a
  mapper created AFTER the resume reads the marker back (so the plaintext came
  off the disk, not the page cache); QMP and the guest's suspend_stats agree;
  s3-mode=deep, so S3 and not suspend-to-idle.
- **THE SHIPPED FIRMWARE'S S3 FAILURE IS AN EDK2 REGRESSION, AND ONE VARIABLE
  AT A TIME PROVES IT.** in-image edk2-20260812 secboot: ASSERT
  MemoryServices.c(203) on the S3 resume path, DEBUG build deadloops, timeout
  kill. in-image NON-secboot, SAME edk2: asserts identically -> not Secure Boot
  or SMM. edk2-20250812 secboot: resumes -> it is the edk2 version. Provenance
  checked by reading the version string out of each binary
  (edk2-d46aa46c8361 vs edk2-2970e5699ba6), not from filenames, and by
  checksumming the cache against fresh qemu-img conversions.
- **A KILLED RUN REPORTED A PASS.** The EXIT trap's own comment claimed it
  survived "a signal". It does run on an untrapped SIGTERM, but `$?` reads 0
  inside it, so the INCOMPLETE branch never fired. Fixed in a9c9ebe6 by
  trapping TERM/INT/HUP by name; the same kill now yields "1 failed" plus a
  named FAIL line. Reproduced in isolation BEFORE fixing, then verified IN SITU
  (real container, `podman kill -s TERM` mid-boot -> `8 passed, 1 failed`,
  ExitCode 143; proof at scratch-later/r6-trapcheck-sigterm-proof.log), and a
  regression test now lifts the trap block out of run-scenarios so deleting the
  signal traps fails the suite. Mutation-tested both ways; the assertion that
  did NOT move is exit 143, because bash re-raises regardless — the exit status
  was never the problem, the log was.
- **A COULD-NOT-RUN THAT MISNAMED ITS OWN CAUSE.** luks-s3 said "the guest
  reported no s3 field at all" while the QMP record (suspended and resumed,
  both true) and the OVMF log (the ASSERT) sat unread in the same directory.
  luks_s3_other_witnesses() now reads both; checked against five inputs so it
  discriminates instead of emitting one fixed string.
- THE DOCUMENTED TPM-CLEAR RECOVERY IS A NO-OP THAT REPORTS SUCCESS.
  `systemd-cryptenroll --wipe-slot=tpm2 --tpm2-device=… --tpm2-public-key=… VOL`
  prints "This PCR set is already enrolled, executing no operation", exits 0,
  header byte-identical: systemd de-dups on the public key and PCR set before
  acting on the wipe, so the TPM plays no part. Two invocations work. Now in
  docs/boot-v2.md's Recovery table. systemd 258.10-1.fc43.
- PCR 11 IS ALL ZEROS ON THE L16 (measured 2026-09-13, read-only). The shipped
  image boots GRUB via bootupd and only sd-stub extends PCR 11, so the
  signed-PCR-11 policy boot-v2 chose has nothing to bind to on a default
  install. That is why L-003 cannot be flipped on for the shipped image.
- L-002 IS NOT A DEFAULT TO FLIP: the installer cannot encrypt at all —
  re-verified this round, every `crypto_LUKS` branch in installer/apex-install
  is a refusal to overwrite an existing header, never a path that creates one
  (:920-925, and CONTAINER_FS at apex-installer-gui:170). No in-place path, the
  initramfs keymap is always `us` (Containerfile.core:2104-2107), plymouth's
  `message()` is a no-op (apex-os.script:230), and `cryptsetup` is an unpinned
  transitive dependency asserted nowhere. Two shipped files already assert
  LUKS2+TPM2 as settled fact (apex-session-select:18-22 and its sudoers :9-12).
- PCR 0 CANNOT BE MOVED IN THIS LAB, structurally: it needs a second OVMF build
  differing ONLY in code, and the two 4M builds in the image differ in Secure
  Boot enforcement too. NOTE, now that fw-2025 exists: a 20250812-vs-20260812
  secboot pair DOES differ only in code, so this limit is worth re-testing.
  The dbx update (PCR 7, 76 -> 21340 bytes) remains the faithful stand-in.
- Older, still true: the lab image ships no diffutils; the APEX initramfs has no
  `sync` or `dd`; tpm2_createprimary's YAML has no `name:` line (use
  tpm2_readpublic); swtpm state does persist across host sessions.

- **S3 IS NOT A RECOVERY TEST, AND I BRIEFLY SAID IT WAS.** scenario_luks_s3
  has no reference to a recovery key; its negative arm is the same run with
  qemu's S3 support off, and what it proves is that the volume stays open
  across the suspend. Only luks-tpm-clear and luks-firmware-change demonstrate
  a refusal followed by a recovery-key unlock in the same boot. Withdrawn from
  docs/boot-v2.md in 985ae69a. If L-002's "does not strand users" is to cover
  suspend/resume, a refusal arm for S3 does not exist yet.

## BLOCKED ON
- L-001's "Real TPM" acceptance needs silicon. Not an agent decision.
- L-002 and L-003 stay blocked behind it, per state/queue.json.
