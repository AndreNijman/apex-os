# luks-installer-2 — continuation of luks-installer (L-002)

items: L-002
repo: apex-os
worktree: /var/tmp/apex-work/wt-luks-installer-2
branch: task/luks-installer-2, cut fresh from roadmap/v2.2 @ 76aa2b95 (2026-09-21)

luks-installer's own branch already LANDED in round 35 (first encrypted
install end to end, live run 8: 41/0, nvram-guard verified) — do not redo its
work. Read ROADMAP/state/agents/luks-installer.md for the full history. Note
its item 7 ("bootc-install-lab needs --generic-image") is ALREADY RESOLVED by
efivars-guard-2, also landed — do not re-do that either, and read
ROADMAP/state/agents/efivars-guard-2.md if you touch that file.

## NEXT (fill in as you go)
1. **Read the result of the first boot run before doing anything else.**
   `/var/lab-scratch/luks-installer-2-boot-run1.log` (also
   `installer/test-installer-luks-boot.sh`'s own kept `$WORK` if it failed —
   the script prints that path on exit; grep the log for "artefacts kept in").
   If `switched-root=yes` / "dracut switched root" PASSed: this is the first
   proof a disk this installer wrote boots end to end. Re-run once more to
   make sure it wasn't a fluke, then consider the item closed and move to a
   Secure-Boot-ENFORCING second boot (see item 2 below) if time remains.
   If it did NOT pass: read `serial-luksboot.log` and `luksboot.ovmf.log`
   inside the kept work dir first — the header of `installer/luks-boot-
   drive.py` names the two most likely failure shapes (no removable-media
   fallback loader on the ESP -> firmware never finds shim at all; or the
   prompt text differs from `Please enter passphrase` on this systemd
   version -> typed nothing before the timeout). The suite prints
   `fallback-present=0/1` in its own failure line for the first case.
2. **This first boot is UEFI Setup Mode, not Secure Boot enforcing** — the
   OVMF varstore this lab image ships (`OVMF_VARS_4M.qcow2`) is pristine (no
   PK/db), checked via `run-scenarios`' own `scenario_prereq`
   (`ovmf_vars_template()` dies if PK/db is already enrolled, and it does not
   die on this image). A truer test enrols Red Hat + Microsoft certs into a
   COPY of that varstore (`virt-fw-vars --enroll-redhat`, used elsewhere in
   this lab) before booting, so the real Fedora-signed shim/GRUB chain is
   actually verified rather than loaded unconditionally. Not done here —
   the first boot only proves the disk boots at all.
3. Ask luks-enroll-2 (dispatched this round too) for the hop this unit
   couldn't measure: does sd-boot actually read \loader\credentials\*.cred off
   the ESP and pass it through sd-stub? Their run-scenarios has the signed-UKI
   chain scenario for it. **Checked this round: as of 2026-09-21, their card's
   FOUND is empty and their NEXT is PCR-7 staged-root / probe_signed_pcr11,
   not the ESP `.cred` hop.** Unanswered by them so far; not something this
   unit can close solo. Do not build the scenario in `run-scenarios` yourself
   — that file's territory is luks-enroll-2's (`files/scripts/boot-v2/**`).
4. The unlock-keymap fallback timing item (predecessor NEXT #5) — see FOUND
   below for the concrete plan (`--check-passphrase` mode in `apex-install`).
   Not started; next agent should read that section before touching
   `apex-install`'s validation phase.

## DONE
- Read all three required cards (own seed, luks-installer, efivars-guard-2)
  before doing anything, per the dispatch brief.
- Confirmed via `free -h`/`df -h` that memory (6.8Gi free / 21Gi available at
  start, dropped to 1.3-1.8Gi free / 14-15Gi available mid-session — other
  agents doing heavy concurrent work) and `/var/lab-scratch` (351G free) were
  workable before starting any guest boot.
- **`installer/apex-install`**: added `APEX_LUKS_EXTRA_KARGS`, a test-only,
  environment-only hook (same shape as the existing `APEX_LUKS_ENROLL_LOCAL`
  / `APEX_RECOVERY_DIR` / `APEX_LUKS_PBKDF_MEMORY` hooks) that appends raw
  kernel arguments to the `bootc install to-filesystem` call on an encrypted
  install. Exists because the shipped karg set (`quiet splash` + the LUKS
  kargs) leaves an installed machine with nothing on a serial console — there
  was no way to make a boot-lab guest's kernel/systemd output legible without
  it. `installer/test-installer-luks.sh` re-run clean after the edit: 57
  passed, 0 failed (unchanged from the predecessor's baseline). shellcheck
  clean.
- **`installer/luks-boot-drive.py`** (NEW): a QMP driver, modelled on
  `installer/keymap-boot-drive.py`'s `Qmp` class, that boots a disk under
  OVMF (not a direct kernel boot — this one goes through firmware, shim,
  GRUB, the shipped kernel, dracut) and types a passphrase through
  `send-key`. Waits for the literal text `Please enter passphrase` before
  typing (never a fixed delay — GRUB's own menu binds letters the passphrase
  may contain), and reports `switched-root=yes/no` off dracut's own
  "Switching root" line. `python3 -m py_compile` clean.
- **`installer/test-installer-luks-boot.sh`** (NEW): the wrapper. Runs its
  OWN install phase (copied in shape from `test-installer-luks-live.sh`'s —
  stub enrolment helper, answers file, nvram-guard-wrapped engine call —
  deliberately NOT shared code, see the file's own header for why) with
  `keymap=us` and `PASSPHRASE=apexbootproof1` (no vconsole layout conversion
  in the loop — that is `test-installer-keymap-boot.sh`'s subject, not this
  one's), sets `APEX_LUKS_EXTRA_KARGS` for serial visibility, then hands the
  finished disk image to `luks-boot-drive.py` inside the `apex-bootlab`
  container (`--device /dev/kvm`, unprivileged, no `--privileged`/`--pid=host`
  — see FOUND below on why this half needs no nvram-guard). Checks the ESP
  for `EFI/BOOT/BOOTX64.EFI` (the removable-media fallback loader) before
  booting, since this install's NVRAM writes are deliberately skipped
  (`--generic-image`) and the boot varstore is fresh, so that fallback path
  is the ONLY way OVMF can find shim at all — a real machine would instead
  get a Boot#### NVRAM entry from bootupd. shellcheck clean. Added to
  `tests/suites-not-in-ci.txt` with a reasoned exception (same shape as the
  two suites next to it). `tests/check-suites-run-in-ci.sh` (94 suites, 87 by
  CI, 7 exempt, 0 unrun-and-undeclared), `tests/check-doc-verbs.sh` (272
  valid, 0 undocumented-and-undeclared, 0 stale) and
  `tests/check-shellcheck-coverage.sh` all clean except the ALREADY-KNOWN
  pre-existing `android/tools/release-version.sh` failure (efivars-guard-2's
  card, item 4 — not caused here, verified again).
- Launched the suite in the background:
  `/var/lab-scratch/luks-installer-2-boot-run1.log`. Result not yet read as
  of this card update — see NEXT #1.

## IN PROGRESS
- Waiting on the first `test-installer-luks-boot.sh` run to finish (install
  phase alone runs ~5-6 min on this machine per the predecessor's own
  measurements; the boot phase adds up to another 5 min). Whoever picks this
  up next: read the log before re-running anything.

## FOUND
- **Confirmed, not assumed: `apex-install`'s encrypted-install path boots via
  GRUB + shim today**, not sd-boot/UKI (`grep -n grub|shim|bootupd
  installer/apex-install`: "APEX boots GRUB through bootupd", literal
  `\EFI\fedora\shimx64.efi` paths). The UKI/sd-boot credential work
  (`loader/credentials/vconsole.keymap.cred`) is forward-looking, written
  today for a bootloader this installer does not yet use — matches what
  luks-installer's own card already said, now double-checked against the
  actual bootc/bootupd call sites rather than inferred.
- **The `--headless` answers-file install path has no existing hook for
  extra kernel arguments.** The UNATTENDED path (`apex.unattended` on the
  live-ISO's own kernel cmdline) has `apex.karg=`, but that is a completely
  different flow — it is for booting the LIVE INSTALLER in a VM and having
  it do an unattended self-install, not for adding kargs to a disk built via
  `apex-install --headless <answers-file>` (which is what both
  `test-installer-luks-live.sh` and this unit's new suite use). Hence the new
  `APEX_LUKS_EXTRA_KARGS` hook.
- **`files/scripts/boot-v2/run-scenarios`' `fresh_vars`/`vm_boot` machinery
  is the WRONG tool for booting a real installed disk.** `fresh_vars` enrols
  the LAB's own self-signed APEX test certificate into PK/KEK/db — built for
  that file's synthetic signed-UKI scenarios. A real `bootc install`-produced
  disk boots the actual Fedora-signed shim/GRUB, which that certificate does
  not trust; reusing it would fail Secure Boot for a reason with nothing to
  do with this installer. The right varstore is the PRISTINE one
  (`ovmf_vars_template()`'s subject — dies if PK/db is already there, and
  does NOT die on this lab image, confirmed via `scenario_prereq`), which
  means this first boot happens in UEFI Setup Mode, not Secure Boot
  enforcing. Said explicitly in both new files' headers so nobody mistakes
  this run for a signing-chain proof.
- **This install's NVRAM writes are deliberately skipped** (`--generic-image`,
  landed by efivars-guard-2), which means the produced disk gets NO Boot####
  entry anywhere and a fresh boot varstore has none either — so the ONLY path
  OVMF can find the installed shim by is the UEFI-mandated removable-media
  fallback, `\EFI\BOOT\BOOTX64.EFI`. Whether bootupd/shim's packaging drops a
  copy there was UNKNOWN going in; `test-installer-luks-boot.sh` checks it
  explicitly (`FALLBACK_PRESENT`) and fails loudly with the ESP's actual
  `EFI/` tree listed if it is absent, so a firmware-stage failure is never
  misread as a LUKS or dracut defect. (Result not yet read — see NEXT #1.)
- **The boot phase needs no nvram-guard.** Unlike the install phase (a
  privileged, `--pid=host` container that can reach the HOST's real UEFI
  variables — the whole subject of BOOT-BREAKAGE-2026-09-20.md), booting the
  finished disk image is an UNPRIVILEGED `podman run --device /dev/kvm`
  reading two pflash FILES and one disk IMAGE FILE inside a bind-mounted work
  directory. No host efivarfs is anywhere near it, same as every OVMF
  scenario in `run-scenarios` already relies on with no guard. Loop device is
  explicitly detached (`losetup -d`) before handing the file to that second,
  independent container, so nothing about the host's block layer is in the
  loop either.
- **Item 3, the unlock-keymap fallback (predecessor NEXT #5) — the concrete
  plan, not yet implemented.** The engine resolves `UNLOCK_KEYMAP` and
  whether it can type the chosen passphrase during VALIDATION, well before
  the `bootc install` call — this already happens in the code path around
  where `LUKS_KARGS` is built (`installer/apex-install`, the encrypt=yes
  branch) and the check function is `keymap_can_type` (see
  `installer/keymap-checks.sh`), already unit-tested in
  `installer/test-installer-luks.sh`'s "can the owner TYPE their passphrase"
  section (14 typeability assertions there, 547/15/11 split). The fix is a
  new engine mode, `apex-install --check-passphrase <xkb-layout> <passphrase>`
  (or reads `layout=`/`lukspass=` off a one-line answers-style stdin — needs
  a decision, not made here), that runs ONLY `keymap_can_type` +
  the XKB-to-console resolution and prints a machine-readable verdict
  (`typeable: yes` / `typeable: no chars=<...>`) with NO disk touched at all
  — the installer GUI (`installer/apex-installer-gui`) would call this
  synchronously right after the encrypt page's passphrase field loses focus,
  instead of waiting for the engine's own `note()` on the PROGRESS page. Not
  started: this needs an `apex-installer-gui` change too (which page calls
  it, when), and this unit ran out of budget after the boot proof. Whoever
  picks it up: `keymap_can_type` and the XKB resolution function are already
  factored out in a way that a `--check-passphrase`-only code path can call
  without going anywhere near partitioning — confirm that by reading
  `installer/apex-install`'s function boundaries before writing the new
  entry point, don't assume it from this note.

## BLOCKED ON
- Item 3 (sd-boot/.cred hop) needs luks-enroll-2 to build or point at a
  scenario that doesn't exist yet in `files/scripts/boot-v2/run-scenarios` as
  of this round. Not blocked in the sense of stuck — just not this unit's
  file to edit, and not yet done by the unit that owns it.
