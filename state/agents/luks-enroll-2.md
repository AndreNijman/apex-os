# luks-enroll-2 — continuation of luks-enroll (L-003)

items: L-003
repo: apex-os
worktree: /var/tmp/apex-work/wt-luks-enroll-2
branch: task/luks-enroll-2, cut fresh from roadmap/v2.2 @ 76aa2b95 (2026-09-21)
katana checkout: /var/lab/scratch/luks-enroll-2/apex-os (same branch)
katana staged root: /var/lab/scratch/luks-enroll-2/apex-root (from katana's own
live `/`, kernel 7.2.6-cachyos1.fc43.x86_64 — see FOUND)

luks-enroll's own branch already LANDED — do not redo its work, read
ROADMAP/state/agents/luks-enroll.md for the full history. L-003 is still
`blocked` (depends on L-002 / luks-installer actually calling this path on a
real boot) — not expected to close it solo.

**DO NOT re-open "the disagreement" section of the predecessor card** (whether
signed PCR 11 needs Secure Boot). Not touched this round either.

## NEXT
1. **Run `luks-pcr7` for real on katana — not yet done.** Commit `b9844c8e` is
   written, shellchecked (`-S warning -x` clean) and `bash -n` clean, and
   `--list` shows it, but it has never been through an actual `podman build` +
   guest boot. Steps:
   - `cd /var/lab/scratch/luks-enroll-2/apex-os && git pull` (already has
     b9844c8e once pushed — confirm).
   - Build the lab image fresh (the cached `localhost/apex-bootlab` on katana
     is 27h old, from before this branch existed):
     `podman build -t localhost/apex-bootlab -f bootlab/Containerfile .`
   - `mkdir -p /var/lab/scratch/luks-enroll-2/out && mv /var/lab/scratch/luks-enroll-2/apex-root /var/lab/scratch/luks-enroll-2/out/apex-root`
     (`luks_require_root` wants `$WORK/apex-root`, i.e. `out/apex-root`).
   - Run under `systemd-run --user`, NEVER `nohup &`:
     `podman run --rm --device /dev/kvm -v /var/lab/scratch/luks-enroll-2:/work:z localhost/apex-bootlab -c '/work/apex-os/files/scripts/boot-v2/run-scenarios --work /work/out apex-image'`
     as a smoke test FIRST (~4 min, proves KVM/OVMF/staged-root work at all —
     this exact class of scenario has never run on katana, only host-side
     enroll-* scenarios have) — the L-003 card's own invocation examples
     never pass `--device /dev/kvm`, that looks like an oversight in both
     cards, add it.
   - Then `... run-scenarios --work /work/out luks-pcr7` alone (don't bundle
     with the other STAGED scenarios yet — isolate failures).
   - Expect to debug on the first run. Untested assumptions most likely to be
     wrong, in rough order: (a) exact serial-log spacing on the "not using
     signed-pcr11" assertion (3 spaces: 1 from my hook's "enroll-err: "
     prefix + 2 baked into the shipped script's `say "  not using $skipped"`
     call — read the raw serial log before assuming the assertion text is
     wrong over the actual behaviour); (b) whether efivarfs is actually
     mounted by dracut's pre-mount hook time (I reasoned it should be, via
     systemd's own generator, not a dracut module — never empirically
     confirmed); (c) `install -D` behaviour inside the container's `install`
     build; (d) `apex-mkuki`/`apex-mkesp` exact flag behaviour when no
     `--pcr-key` is given (modelled on `scenario_apex_image`, not run).
   - If `have-systemd-cryptenroll=yes`/`have-apex-luks-enroll=yes` etc. never
     appear at all, the guest never reached the hook — check the cpio built
     correctly (`cpio -it < /var/lab/scratch/.../out/luks-pcr7-hook-a.cpio`)
     before assuming the enrolment logic itself is wrong.
2. **Scenario 5 (`probe_signed_pcr11` in a UKI guest) is now in scope.**
   `task/sdboot-image`, `task/sdboot-migrate` and `task/sdboot-migrate-2` have
   ALL landed on `roadmap/v2.2` (checked this round: `git merge-base
   --is-ancestor origin/task/sdboot-image origin/roadmap/v2.2` etc., all true,
   0 commits ahead). Once `luks-pcr7` is proven, adding a `luks-pcr7-signed`
   (or similar) scenario is nearly free: same `build_pcr7_enroll_bundle`
   helper, same `guest-luks-enroll.sh` hook (it already runs
   `apex-luks-enroll` with no `--policy`, so auto-select decides), the ONLY
   difference is building the enrolment UKI WITH `--pcr-key`/`--pcr-pubkey`
   (like `luks_uki`'s "good" spec) instead of without. That makes sd-stub
   publish `StubInfo` + `/run/systemd/tpm2-pcr-public-key.pem`, PCR 11
   non-zero, and `probe_signed_pcr11` should win over `probe_pcr7`. Assert
   `binding=signed-pcr11` and the ABSENCE of the "not using signed-pcr11"
   line. One unlock boot after it is enough — the update/foreign-signature
   arms are already `luks-tpm`'s job, don't duplicate them.
   `enroll-sb-on`'s existing "not using signed-pcr11" assertion stays
   unchanged either way — its fixture has no StubInfo and never will.
3. Once both scenarios are green, run the FULL default STAGED set together
   (`apex-image luks-tpm luks-tpm-clear luks-firmware-change
   luks-firmware-code luks-no-tpm luks-s3 luks-pcr7` [+ the new one]) to check
   for cross-scenario interference (shared `$WORK`, shared `$KEYS`) before
   calling the round done.
4. Re-check `apex game status` and `nvidia-smi --query-compute-apps` before
   EACH heavy step, not just once at session start — Hyprland desktop session
   is up on katana with `IdleHint=no` (not gaming, confirmed zero GPU compute
   procs and load 0.00 at session start, but re-verify, don't assume it held).

## DONE
- Re-verified katana idle for gaming purposes (`apex game status
  active=false`, no scx scheduler, zero `nvidia-smi` compute apps, load
  0.00 despite a live Hyprland desktop session — that's a logged-in session,
  not a game).
- Confirmed `task/sdboot-image`, `task/sdboot-migrate`, `task/sdboot-migrate-2`
  all landed on `roadmap/v2.2` (0 commits ahead each); only
  `task/sdboot-xbootldr` has not (1 commit ahead, docs-only).
- Wrote `files/scripts/boot-v2/guest-luks-enroll.sh` — a new dracut pre-mount
  hook that runs the REAL shipped `files/system/libexec/apex-luks-enroll`
  inside a guest, reporting preconditions and results over serial the same way
  `guest-luks-probe.sh` does. `shellcheck -S warning -x` clean.
- Wrote `scenario_luks_pcr7` in `run-scenarios` (three boots: live enrol, cold
  reproduction, firmware-change refusal+recovery) plus `install_real` and
  `build_pcr7_enroll_bundle` helpers, added `luks-pcr7` to `STAGED`. `bash -n`
  and `shellcheck -S warning -x` both clean on the whole file.
- Updated the stale `# L-003 — the SHIPPED enrolment path` comment header that
  said the in-boot half was unproven, to point at `luks-pcr7` instead of
  claiming a gap that code now exists to close (not yet proven green, worded
  accordingly — see FOUND).
- Committed `b9844c8e`, pushed to `origin/task/luks-enroll-2`.

## FOUND
- **`apex-stage-root` needs no image build at all: katana itself IS a live
  booted APEX-OS machine.** `cat /etc/os-release` → `NAME="APEX-OS"`;
  `/usr/lib/modules/7.2.6-cachyos1.fc43.x86_64/{vmlinuz,initramfs.img}` exist
  with a real 377MB initramfs; `/usr` is a bootc-style read-only overlay. Ran
  `sudo files/scripts/boot-v2/apex-stage-root --output
  /var/lab/scratch/luks-enroll-2/apex-root` directly against katana's own `/`
  — exit 0, checksums verified, seconds not the ~50 minutes a fresh image
  build would cost. This resolves what the advisor flagged as the one
  question that could BLOCK the whole item this round. Do not rebuild an
  image to get a staged root on katana; re-stage from `/` instead (kernel
  version may drift after `apex update` — re-run `apex-stage-root` if so).
- **`/var/lab-scratch` (the path both sdboot-image.md and sdboot-migrate.md
  cite) does not exist on katana.** The writable area under `/var/lab` is
  `/var/lab/scratch` (owned `andre:andre`, `/var/lab` itself is root-owned
  `755`). Whatever those cards meant by `/var/lab-scratch` either predates a
  rename or was never actually on katana — do not `mkdir -p` expecting it to
  work at the literal path those cards give.
- **`systemd-cryptenroll`, `od`, and python3 are ALL absent from the real APEX
  dracut initramfs**, measured with `lsinitrd`, not assumed — this is WHY
  `luks-pcr7` needs a bundle at all, and why enrolling via the shipped script
  in-guest is more than copying `guest-luks-probe.sh`'s pattern.
  `systemd-cryptsetup`, `systemd-cryptsetup-generator`, `cryptsetup`,
  `tpm2_pcrread`/`tpm2_pcrextend`/etc. (all symlinks to one `tpm2` multicall
  binary), `awk` (→ gawk), `mktemp`, `timeout` ARE present. Only 3 of
  `systemd-cryptenroll`'s 24 shared libs are missing
  (`libsystemd-shared-258.10-1.fc43.so`, `libgcc_s.so.1`, `libz.so.1`); the
  rest (libcryptsetup, libcrypto, libselinux, libmount, libblkid, libjson-c,
  libpam, …) are already there via the `crypt`/`dm`/`lvm` dracut modules.
- **A minimal python3 + `json` bundle is ~11MB and needs no `PYTHONHOME`/
  `LD_LIBRARY_PATH` if installed at the SAME absolute paths a real install
  uses** (`usr/bin/python3`, `usr/lib64/pythonX.Y/…`) — verified standalone on
  katana with an artificial `/tmp` prefix (which is why THAT test needed the
  env vars; the guest bundle doesn't). The exact file list was discovered
  empirically — `python3 -S -c "import json,sys; …sys.modules…"` — not
  guessed: 24 `.py` files + one `_json*.so` + `libpython3.14.so.1.0`. This
  generalises to whatever Python version the boot lab CONTAINER ships
  (queried dynamically in `build_pcr7_enroll_bundle`, nothing hardcoded to
  3.14 — that version number is katana's host Python, used only for the
  offline feasibility check, not baked into the scenario code).
- **PCR 7 is architecturally different from PCR 11 for lab-testing purposes,
  and this is why `luks-tpm*`'s `luks_enroll`/`enroll_pcr7` patterns don't
  transfer.** PCR 11 (signed policy) needs no live TPM read at enrolment time
  — `ukify`/`systemd-cryptenroll --tpm2-public-key=` bind to a PUBLIC KEY,
  computing the expected future PCR11 value from the UKI's own content,
  entirely offline. PCR 7 (`--tpm2-pcrs=7`, value-bound, what the shipped
  script actually runs when Secure Boot is on and there's no sd-stub PCR-11
  policy) READS THE LIVE TPM at enrolment time — so enrolling against a REAL
  PCR 7 requires a REAL firmware boot to have JUST happened, in the SAME
  swtpm session (TPM PCRs are volatile: `--flags startup-clear` resets them on
  every fresh `swtpm socket` start). Every existing PCR-7-touching fixture in
  this file (`enroll_pcr7 HEX`, `luks_firmware_change`'s by-value control) is
  host-side and fabricates or externally-computes the value rather than
  reading it live from a real boot — none of them are wrong, they're
  answering a different, narrower question than `luks-pcr7` does.
- `virt-fw-vars --add-dbx` (the `luks_firmware_change` technique, reused for
  boot C) depends on `/usr/share/edk2/ovmf/DBXUpdate-*.x64.bin` existing in
  the lab image — not confirmed present or absent in the CURRENT
  `apex-bootlab` build; if absent, `luks-pcr7`'s boot C reports
  COULD-NOT-RUN via `cannot`, same graceful fallback `luks_firmware_change`
  already has, not a failure.

## BLOCKED ON
- L-003 stays blocked pending L-002 (luks-installer) actually exercising this
  path end to end on a real boot — unchanged this round.
- `luks-pcr7` itself is NOT blocked on anything external; it just has not been
  run yet. See NEXT item 1.
