# later
items: L-001, L-002, L-003
repo: apex-os
worktree: /var/tmp/apex-work/wt-later
branch: task/later-tpm-qualification

## NEXT
Baseline `luks-tpm` scenario is RUNNING in localhost/apex-bootlab (started 08:23 AWST,
log /var/tmp/apex-work/scratch-later/baseline-luks-tpm.log, work dir
/var/tmp/apex-work/scratch-later/out). Wait for it, confirm it goes green on the L16.
Then add three scenarios to files/scripts/boot-v2/run-scenarios in this order:
luks-tpm-clear, luks-firmware-change, luks-s3 (details in IN PROGRESS).
Re-run command:
  podman run --rm --device /dev/kvm -v /var/tmp/apex-work/wt-later:/work:z \
    -v /var/tmp/apex-work/scratch-later:/lab:z localhost/apex-bootlab \
    -c '/work/files/scripts/boot-v2/run-scenarios --work /lab/out <scenario>'

## DONE
- Card created at round start.
- Merged origin/roadmap/v2.2 (13d53c01) into the worktree, clean.
- Staged the REAL APEX root for UKI builds:
  `sudo -n files/scripts/boot-v2/apex-stage-root --output .../scratch-later/out/apex-root`
  -> kernel 7.2.3-cachyos2.fc43.x86_64 (16,898,120 B), initramfs 375,558,646 B, os-release
  NAME="APEX-OS". Read-only against the live OS; no boot path touched.
- Collected the read-only SILICON evidence from the L16 (see FOUND).

## IN PROGRESS
- Three new scenarios planned for files/scripts/boot-v2/run-scenarios:
  1. `luks-tpm-clear` (4 boots): boot1 unlock SUCCESS + marker written; `cp -a` snapshot
     of luks.img + tpm state; TPM2_Clear over the same swtpm TCTI apex-luks-enroll uses
     (tpm2_clear -c p; platform auth empty on swtpm); boot2 must show tpm-unlock=REFUSED
     AND recovery-unlock=SUCCESS in the SAME boot; then re-enrol host-side against the
     CLEARED TPM (--wipe-slot=tpm2 first, assert the stale token is gone not orphaned);
     boot3 unlock SUCCESS + plaintext-marker=found. That is the user's whole recovery
     procedure, not just "the recovery key works".
  2. `luks-firmware-change`: claim under test is "signed PCR 11 binds nothing in PCR 0-7".
     Control is the whole scenario — a SECOND volume on /dev/vdc bound BY VALUE to PCR 7
     (--tpm2-pcrs=7:sha256=<value the guest printed in boot 1>; a bare --tpm2-pcrs=7
     refuses before anything changes because swtpm's PCR 7 is unmeasured at host-side
     enrol time). Boot A both SUCCEED (control valid). Then change PCR 7 by applying
     Fedora's real dbx blob /usr/share/edk2/ovmf/DBXUpdate-20260630.x64.bin with
     virt-fw-vars, keeping the APEX cert in db so the UKI still boots. Boot B: PCR 11
     slot SUCCEEDS, PCR 7 control REFUSES. NOTE precisely in the writeup: a varstore
     change moves PCR 7, not PCR 0.
  3. `luks-s3`: needs vm_boot changes — it hardcodes `-global ICH9-LPC.disable_s3=1` and
     `-nodefaults` (no monitor). Add `--s3`: disable_s3=0 plus `-qmp unix:...` and a
     host-side python that polls query-status for `suspended` then issues system_wakeup.
     Guest side: read /sys/power/suspend_stats/success, `echo mem > /sys/power/state`,
     read it again; then re-read the plaintext marker through the still-open mapper AND
     detach/re-attach via the TPM to prove the TPM answered after resume. Two independent
     observers. If the kernel refuses `mem`, that is COULD-NOT-RUN with the errno.

## FOUND
- Substrate decision, recorded so it does not read as ignoring the brief: this
  qualification goes in `files/scripts/boot-v2/run-scenarios`, NOT `tests/vmlab`.
  P2-008's vmlab guest (tests/vmlab/mk-guest) is a busybox initramfs with no cryptsetup
  and no tpm2 stack — it can prove a TPM device node exists and nothing about an unlock.
  run-scenarios boots the REAL APEX initramfs (systemd-cryptsetup, systemd-pcrphase,
  libtss2), already has apex-luks-enroll, signed-PCR-11 UKIs and a PERSISTENT swtpm
  state dir. "Snapshot, break, recover" is `cp -a` of the LUKS image + swtpm state dir;
  vm_boot is raw qemu, so that is exactly equivalent to a libvirt disk+nvram snapshot.
- MEASURED ON THE L16, read-only, 2026-09-13 08:23 AWST — the decisive fact for L-003:
    /sys/class/tpm/tpm0 -> ../../devices/platform/STM0925:00   (discrete ST TPM, not fTPM)
    tpm_version_major = 2
    PCR0  = E09A852EF5AF0FFE803F0AE102B23F5E0EEBEC2C51ADEDAB39A076CB85AC414D
    PCR4  = 317B0C0C0F9C1996E21EBAF74DCB097E907CB78202935EA283D937F2470D238C
    PCR7  = 0306B4609EF33E306C907BD56CD1AD3E108022665F96C4938BDE99235665D419
    PCR11 = 0000000000000000000000000000000000000000000000000000000000000000
    mokutil --sb-state = "SecureBoot enabled"; efivar SecureBoot byte = 1
    LENOVO 21SCCTO1WW, BIOS R2UET31W (1.31), 2026-05-27
  PCR 11 IS ALL ZEROS ON THE REAL MACHINE. Nothing measured it, because the shipped
  image boots GRUB and only sd-stub extends PCR 11. So the signed-PCR-11 policy that
  boot-v2 chose — the only policy that survives a kernel update — has NOTHING TO BIND TO
  on a default APEX install. That is a measurement, not an argument, and it is why
  L-003 cannot be flipped on for the shipped image.
- `vm_boot` in files/scripts/boot-v2/lib.sh hardcodes `-global ICH9-LPC.disable_s3=1`
  and `-nodefaults`, so S3 is off and there is no QMP monitor. Both must change for the
  suspend/resume criterion.
- Everything in docs/boot-v2.md's "What was measured" was measured on KATANA, which is
  off-limits this round. scenario_luks_tpm had never been shown to run on the L16.
- The boot lab image already carries everything the new scenarios need, checked not
  assumed: tpm2_clear, tpm2_pcrread, swtpm_ioctl, socat, virt-fw-vars, and Fedora's real
  DBXUpdate-20260630.x64.bin. Two 4M OVMF builds exist (OVMF_CODE_4M.qcow2 and
  OVMF_CODE_4M.secboot.qcow2) but they differ in Secure Boot enforcement as well as in
  code, so swapping them conflates "firmware changed" with "Secure Boot off" — the dbx
  update is the faithful stand-in for what fwupd does on real hardware.

## L-002 / L-003 SURVEY OF THE SHIPPED PRODUCT (read-only, worktree @ 13d53c01)
Every line below is a file+line in the repo, not prose. This is the "what would strand
a user" material, and the headline is that L-002 is NOT a default to flip:

- THE INSTALLER CANNOT ENCRYPT AT ALL. `installer/apex-install:739-753` is the complete
  answers-file key list and has no encryption key; `:755` allows modes disk|partition
  only; `:1223` is `bootc install to-disk --wipe --filesystem "$ROOTFS_TYPE"` and `:1173`
  is a plain `mkfs."$ROOTFS_TYPE"` straight onto $TARGET. `apex-installer-gui:682-688`
  is the whole 7-step page list — no encryption page. grep for encrypt/luksFormat/
  cryptenroll/recovery key/rd.luks/crypttab over installer/ returns ZERO. LUKS appears
  only as a REFUSAL (`apex-install:920-927` refuses to mkfs over a crypto_LUKS header).
  So "enable LUKS2 by default" is a feature that does not exist, not a default.
- NO IN-PLACE ENCRYPTION. Both modes are destructive fresh installs; there is no
  `to-existing-root` path. Existing installs cannot be encrypted, so any default is
  fresh-install-only by construction.
- THE INITRAMFS KEYMAP IS ALWAYS `us`, and this is the largest lockout risk.
  `Containerfile.core:2104-2107` bakes KEYMAP=us; `Containerfile.apex:236` builds the
  initramfs inside that container; nothing passes rd.vconsole.keymap and nothing
  regenerates the initramfs (grep rd.vconsole over installer/ files/ -> 0).
  The installer writes the layout only to the TARGET deployment AFTER install
  (`apex-install:488-490`). Worse, it writes an XKB layout name into a CONSOLE keymap
  field (validated against X11/xkb/rules/base.lst at `:800-803`) — XKB `gb` is not
  console `uk` — and `apex-vconsole-guard` (Containerfile.base:2025-2039) then silently
  repairs an unloadable keymap back to `us` at every boot. A UK user's passphrase
  cannot be typed at the prompt. The installer's own comment at `:727-729` names this
  exact failure: "a wrong layout is a password that cannot be typed on first boot."
- PLYMOUTH SHOWS THE PROMPT BUT NOT THE ERROR. Both themes implement
  display_password (apex-os.script:208-235, SetDisplayPasswordFunction at :235) so the
  prompt and bullets do render — but `fun message (text) { }` at :230 is a NO-OP, so a
  wrong-password retry, an ask-password string or a dracut timeout is silently dropped.
  The user sees a prompt that appears to do nothing. Only apex-os-chartreuse ships
  (Containerfile.apex:229-234).
- `cryptsetup` IN THE IMAGE IS AN UNPINNED TRANSITIVE DEPENDENCY. grep cryptsetup over
  Containerfile.{base,core,apex} -> 0 hits; it arrives only from quay.io/fedora/
  fedora-bootc:43 and is asserted NOWHERE at build time, while the NVIDIA initramfs
  contents ARE asserted with lsinitrd right beside the dracut call
  (Containerfile.apex:247-249). The claim "the APEX initramfs can unlock LUKS" today
  rests on a comment in a CI-lab script (guest-luks-probe.sh:11-15), not on a gate.
- L-003's STRUCTURAL BLOCKER, in code not prose. The shipped image boots GRUB via
  bootupd (`apex-install:6-9`, `:1241`; ESP is \EFI\fedora\). Containerfile.base:825-837
  states no bootloader is ever installed, and `Containerfile.base:852` is the ONLY COPY
  of anything boot-v2 into the image — and it copies the DOCUMENTATION. apex-mkuki,
  apex-mkesp, apex-luks-enroll are never shipped. So there is no path by which a
  shipped APEX install boots a UKI, therefore no sd-stub, therefore no PCR 11 — which
  is exactly what the L16 measurement (PCR11 = 64 zeros) shows on real silicon.
- apexd IS REPORT-ONLY. `apex/src/storage.rs:114-139` has Status/Warnings/Wipe and no
  enroll verb; `apexd-core/src/storage.rs:513-530` maps Encryption::Plain to
  Health::Available with the comment "an unencrypted disk is a choice, and this report
  does not judge it". `apex boot status` never reads /etc/crypttab (boot.rs:427-428).
- A DISCREPANCY WORTH LANDING: two SHIPPED files assert as settled fact that the
  machines are LUKS2+TPM2 — `files/system/libexec/apex-session-select:18-22` and
  `files/system/sudoers/apex-session-select:9-12` ("both target machines end up on
  LUKS2+TPM2"), used to justify refusing autologin. The shipped installer cannot
  produce that state. The tense is aspirational; the justification is load-bearing.
- NO TEST ASSERTS ENCRYPTION IS OFF BY DEFAULT (grep luks/encrypt over tests/*.sh finds
  only reporting assertions). tests/test-boot-v2.sh:95-109 DOES assert no shipped unit
  or libexec helper runs bootctl install/efibootmgr, with both controls — but its scan
  is scoped to files/**/libexec, *.service, *.timer, so files/scripts/** is not covered.

## BLOCKED ON
- Nothing.
