# luks-boot — L-002, finish it: the installer makes a LUKS2 disk AND that disk boots

items: L-002
repo: apex-os
worktree: /var/tmp/apex-work/wt-luks-boot
branch: task/luks-boot

Predecessor cards to read before touching anything:
`ROADMAP/state/agents/luks-installer.md` (first encrypted install, LANDED round 35),
`ROADMAP/state/agents/luks-installer-2.md` (the boot suite + `--check-passphrase`;
its work is already MERGED into this branch at 3e235bc1 — do not redo it),
`ROADMAP/state/agents/efivars-guard-2.md` (`--generic-image`, already resolved).

## NEXT (fill in as you go)
1. Read `/var/lab-scratch/luks-boot/run2.log` — it ends with a literal
   `EXIT_CODE=` line. If `switched-root=yes`, L-002's blocker (a) — "NO DISK
   THIS INSTALLER PRODUCED HAS EVER BEEN BOOTED", the phrase in the item's own
   evidence — is closed.
2. Then re-run the BOOT HALF ONLY against the kept image (the
   `podman run --rm --device /dev/kvm -v $WORK:/w …  luks-boot-drive.py` block
   from the suite, ~5 min) rather than a second ~25-minute install. The install
   is deterministic; a fluke would live in the boot phase — prompt timing and
   QMP typing — so that is the half worth repeating.
3. `ROADMAP/set-status.py L-002 <status> --evidence "…"`. It REPLACES; the
   whole string has to be rewritten. Read `roadmap.yaml` L-002's current
   evidence (line ~8332) first and carry the round-32/35 record forward.

## DONE
- Read the branch's three commit messages and both predecessor cards.
- Read `/var/lab-scratch/luks-installer-2-boot-run1.log` — the run the
  predecessor never got to read. See FOUND #1.
- Merged `origin/roadmap/v2.2` (34132080, the luks-enroll-2 PCR-7 guest
  scenario + `ff921909` boot-migrate refusal) into this branch as `fdb97b04`.
  Clean merge, 4 files, none of them under `installer/`.
- Cleared run1's leftovers: no stale loop device, no dm node, no mount, no
  root podman container. Deleted the 26 GB half-written
  `/var/lab-scratch/apex-luks-boot.qclEik`; the run1 LOG is kept and is the
  only thing in it worth anything.
- Relaunched the suite — see FOUND #2 for how, and why that is not a
  violation of the foreground rule.

## FOUND
1. **run1 measured nothing. It was killed, not failed.** `engine exit=143
   after 869s`, with the literal word `Terminated` printed against the
   `podman … bootc install to-filesystem` line, while the engine was still at
   `Deploying container image…` (`layers already present: 0; layers needed:
   296 (15.5 GB)`). 143 = 128+SIGTERM. The predecessor launched the suite in
   the background and its session ended ~600 s later; this is exactly the
   known shape — "a backgrounded podman is SIGTERMed and still exits 0; a
   dead agent's last run is not a defect until re-run foreground". Note the
   OUTER script survived and printed `0 passed, 3 failed`: only the podman
   subtree was killed. **Do not quote that line as a result about the
   installer.** Checked before re-running that nothing internal imposes a
   ~15-minute limit: no `timeout`/`kill` wrapper around podman in
   `tests/lab/nvram-guard` or `installer/apex-install` (only
   `udevadm settle --timeout=30` and an INT/TERM/HUP trap that exists to make
   an interrupted install say so).
2. **How run2 is launched, and why it is not "backgrounded".** The foreground
   rule exists because a `nohup &` podman gets SIGTERMed when the harness tears
   the agent's process tree down. So run2 runs as a systemd USER unit —
   `systemd-run --user --collect --unit=luks-boot-run2
   /var/lab-scratch/luks-boot/run2.sh` — which is owned by the user manager,
   outside any agent process tree, and whose exit status systemd records.
   `systemd-run --user` was PROBED first (`--wait … /usr/bin/true`, rc=0):
   a dispatched agent's cgroup reads as a scheduled job and the user bus is
   not always reachable. The wrapper appends a literal `EXIT_CODE=<rc>` line
   to `/var/lab-scratch/luks-boot/run2.log` so a later reader can tell a
   finished run from a truncated one — the exact ambiguity that cost run1.
   `APEX_LUKS_BOOT_KEEP=1` so the work dir survives for the boot-only re-run.
3. **Idle suspend was already inhibited; no second inhibitor was taken.**
   `systemd-inhibit --list` shows pid 51488,
   `apex-roadmap-orchestrator-r37`, `sleep:idle:handle-lid-switch`, **block**
   mode, `sleep 14400` with ~3 h 40 m left at 11:55 AWST. Worth checking
   rather than assuming, because a >25-minute run with no keyboard input is
   exactly what hypridle's 15-minute idle suspend kills.
4. Preconditions checked on this machine at 11:50 AWST 2026-09-21:
   `localhost/apex-os:daily` and `localhost/apex-bootlab` both PRESENT in ROOT
   podman storage (so neither build happens inside the run), 321 GB free on
   `/var`, 21 GB available RAM, `/dev/kvm` present.

## IN PROGRESS
- `luks-boot-run2.service`. Install phase then OVMF boot phase.

## BLOCKED ON
- (nothing)

## FOR L-003 (luks-enroll, NOT dispatched this round)
1. **What this unit settles that L-003 needs.** The disk `apex-install`
   produces today boots **GRUB + shim**, not a UKI — confirmed by
   luks-installer-2 against the actual bootc/bootupd call sites, not inferred.
   So there is no sd-stub on a disk this installer writes, therefore no PCR 11
   measurement to bind to, therefore `probe_signed_pcr11` still has no
   installer-produced disk to run on. L-003's signed-PCR-11 branch stays
   forward-looking until the sd-boot/UKI pivot reaches `apex-install` itself.
2. **The lab boot is Secure Boot OFF (UEFI Setup Mode).** The varstore is the
   pristine `OVMF_VARS_4M.qcow2`; the real Fedora-signed shim/GRUB chain loads
   unconditionally and nothing is verified. That is exactly the condition
   under which a signed PCR 11 policy REPLAYS, since the signature ships
   publicly inside the UKI. Any L-003 scenario that wants to claim a
   signing-chain property must enrol Red Hat + Microsoft certs into a COPY of
   that varstore (`virt-fw-vars --enroll-redhat`) first — the lab's own
   self-signed APEX cert in `run-scenarios`' `fresh_vars` will NOT verify a
   real bootc-installed shim.
3. **The enrolment contract this installer actually calls** is
   `--device DEV --recovery-out PATH`, exit 0 on every declined case; the
   suites stand in for it with a stub that runs only
   `systemd-cryptenroll --recovery-key`. The shipped
   `files/system/libexec/apex-luks-enroll` must keep that interface.
4. **swtpm IS in the boot lab** (`bootlab/Containerfile`: `swtpm
   swtpm-tools tpm2-tools`, plus a build-time `command -v swtpm` assertion and
   the 4 MB `OVMF_CODE_4M.secboot.qcow2`/`OVMF_VARS_4M.qcow2` pair, the only
   OVMF with a TCG2 protocol). So a TPM unlock CAN be measured in this lab —
   what cannot be measured here is a TPM unlock on a disk *this installer
   produced*, for reason 1.
5. Standing facts handed down with the dispatch, not re-derived here: on
   systemd 258.10 `systemd-cryptenroll --tpm2-device=auto` with no PCR
   argument binds to **NO PCRs at all**; whatever turns encryption on must
   pass `--tpm2-pcrs=` or `--tpm2-public-key-pcrs=` explicitly, and the test
   must assert the enrolled token carries a policy rather than that the
   command exited 0.
