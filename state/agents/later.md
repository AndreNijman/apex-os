# later
items: L-001, L-002, L-003
repo: apex-os
worktree: /var/tmp/apex-work/wt-later
branch: task/later-tpm-qualification

## NEXT
Step 0 (baseline-before-writing): read run-scenarios 1-107, .github/workflows/boot-v2.yml
and bootlab/Containerfile; then `sudo -n files/scripts/boot-v2/apex-stage-root --output
/var/tmp/apex-work/scratch-later/apex-root` and run ONLY the existing `luks-tpm` scenario
in localhost/apex-bootlab:latest. Time it. If it cannot run on the L16, that is a FOUND
entry and the round's verdict shape changes.

## DONE
- (nothing yet)

## IN PROGRESS
- Nothing committed yet.

## FOUND
- Substrate decision, recorded so it does not read as ignoring the brief: the
  qualification goes in `files/scripts/boot-v2/run-scenarios`, NOT `tests/vmlab`.
  P2-008's vmlab guest (tests/vmlab/mk-guest) is a busybox initramfs with no
  cryptsetup and no tpm2 stack — it can prove a TPM device node exists and nothing
  about an unlock. run-scenarios boots the REAL APEX initramfs (systemd-cryptsetup,
  systemd-pcrphase, libtss2), already has apex-luks-enroll, signed-PCR-11 UKIs and a
  PERSISTENT swtpm state dir. "Snapshot, break, recover" is `cp -a` of the LUKS image
  and the swtpm state dir; vm_boot is raw qemu, so that is equivalent to a libvirt
  nvram+disk snapshot.
- `vm_boot` in files/scripts/boot-v2/lib.sh hardcodes `-global ICH9-LPC.disable_s3=1`
  and `-nodefaults` (no QMP monitor). Suspend/resume needs both changed.
- Everything in docs/boot-v2.md's "What was measured" was measured on KATANA, which is
  off-limits this round. scenario_luks_tpm has never been shown to run on the L16.

## BLOCKED ON
- Nothing.
