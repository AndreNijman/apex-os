# later
items: L-001, L-002, L-003
repo: apex-os
worktree: /var/tmp/apex-work/wt-later
branch: task/later-2

## NEXT

**SUPERSEDED TWICE. Read `later-silicon-2.md` first, then `later-silicon.md`.**
Round 32 (2026-09-19, unit `later-silicon-2`, branch `task/later-silicon-2`,
commits `16fe7ad3` and `b5435034`, pushed) answered the one question round 31 ended on.

**Andre said yes, katana's TPM was cleared, and Run 2 is PASS.** L-001 is now
**4 of 5 runs** and still `partial`. In one boot on an Intel PTT fTPM, a volume
enrolled before the clear gave `tpm-unlock=REFUSED` → `recovery-unlock=SUCCESS`
→ marker identical. The clear was verified five ways rather than inferred from
the reboot, because `ppi/response` reads `5 0: Success` before any reboot
happens and proves nothing.

**Do not dispatch a unit to "finish L-001".** Run 5 needs somebody in firmware
setup — disabling the TPM takes the `ppi` directory that would re-enable it —
and Run 3 has no firmware capsule to apply. Both are in `state/queue.json`
under `_hardware_blocked`.

**If you are going to touch katana: `/dev/nvme0n1` is not a stable name there.**
The two NVMe controllers are probed asynchronously. Round 32 logged three boots;
the third — an ordinary reboot, not the clear — enumerated them the other way
round, and the disk this programme is told never to write became the APEX disk.
Use the serial, the PCI function, the PARTUUID or the label.

The round-31 card follows, and the round-29 card after that. Both are still
accurate about what was true when they were written.

---

**SUPERSEDED 2026-09-19 (round 31) — read `later-silicon.md` instead.** The
card below said the remaining work was Andre's five-run hardware procedure and
that no agent could satisfy L-001's word "Real" from here. A machine appeared:
katana. Unit `later-silicon` ran the procedure on branch `task/later-silicon`
(`17252b16`, pushed).

What changed, in one line each:

- **Runs 1 and 4 PASS on real silicon** (Intel PTT fTPM). Enrol, TPM unlock at
  0.29 s, recovery-key unlock, the same plaintext through both, suspend/resume
  with a fresh unseal afterwards and all 24 PCRs intact.
- **The signed PCR 11 policy works on Intel PTT**, forged signature refused by
  the part itself — the mechanism this program chose, first run outside swtpm.
- **Run 2 (TPM clear) is now one decision, not an absent machine.** Katana's
  TPM is owned by a live Windows install with Windows Hello provisioned. No
  BitLocker, so no data is at risk; the cost is his PIN. The PPI procedure is
  two commands and needs nobody at the machine.
- **Runs 3 and 5 are COULD-NOT-RUN** with hard reasons (no firmware capsule
  exists; disabling the TPM is not symmetric with enabling it).
- **Four defects found that no VM could produce** — see `later-silicon.md`
  FOUND. The one that matters most: `systemd-pcrextend` is a silent no-op on a
  GRUB machine, exit 0, so a boot-phase PCR 11 policy can never be satisfied on
  a default APEX install.

L-002 and L-003 are **still blocked**, and `state/queue.json`'s note for this
unit was rewritten because the gate it named is now met and is not sufficient.

The historical NEXT follows, kept because it is the record of what was true
before the hardware existed.

### Historical NEXT (round 29)

Nothing is running and nothing is half-finished. Branch tip 0391b4ec is pushed
and cut from roadmap/v2.2 1668ed9c. All containers from this unit are removed
and no qemu process is left.

**Everything a VM can qualify is now qualified.** The PCR 0 limit the previous
card named as the next experiment is gone, and the no-TPM arm it named as the
second candidate exists. What is left of L-001 is the acceptance word "Real",
and no agent can satisfy it from here: state/queue.json holds L-002 and L-003
behind "a real TPM enrol-and-recover cycle on hardware", the L16's TPM is
off-limits, and katana is off-limits.

So the next step is Andre's, not an agent's, and it is now written down as a
procedure rather than as a wish: **docs/boot-v2.md, "The run somebody with
hardware would have to do"** - five numbered runs, four prerequisites, what to
record at each step. Hand that to whoever has a machine they are willing to
clear the TPM on. Runs 1, 2 and 5 are the minimum that moves L-001 off
`partial`.

If more VM work is wanted anyway, what is honestly left is small:

  1. Re-run luks-tpm-clear and luks-firmware-change on the fw-2025 firmware.
     They passed on the shipped build and S3 did not, and nobody has checked
     whether the older edk2 changes their results. Still open from round 28.
  2. `luks-no-tpm` runs the guest with `headless=1`, which turns a passphrase
     prompt into a refusal. On a real machine the same situation shows a
     prompt, and nobody has checked whether plymouth renders it legibly. That
     needs a guest with a console, not a serial log.

NEVER `nohup podman run &`. `podman run -d` then a FOREGROUND `podman wait`, and
check `State.FinishedAt` and `State.ExitCode` before believing any log.
NEVER edit files/scripts/boot-v2/** while a container executes run-scenarios
from it. The containers run from a COPY at scratch-later/tree-exp, so the
worktree stays editable while they run. `diff -rq` the two before every launch.

## DONE
- Round 29. Branched task/later-2 from roadmap/v2.2 1668ed9c (round 28's work
  is already merged, nothing to fold in), six commits, each pushed as it was
  made: acc0a65c (the PCR 0 scenario), 79d26db1 (the no-TPM scenario), bc2538ab
  (static tests, 78 -> 105), f80d524f (the L-001 write-up), 18a015ec (stop-slop
  plus two Recovery table rows), 0391b4ec (the TPM-clear marker assertion and a
  provenance header that had gone stale).
- Four real runs, every one foreground-waited with ExitCode and FinishedAt read
  before the log was believed: `scratch-later/r7-fwcode.log` (22/0/1),
  `r7-notpm.log` (16/0/1), `r7-tpmclear.log` (20/0) and `r7-tpmclear2.log`
  (22/0).
- THE SHARED FIXTURE CHANGED, SO THE SCENARIO THAT RUNS THROUGH IT WAS
  RE-MEASURED. guest-luks-probe.sh gained pcr0, the recovery-marker read-back
  and a top-level MARKER, so luks-tpm-clear was re-run rather than assumed:
  20/0, identical to round 27. Its refused boot also carried the new
  recovery-marker=found line unasserted, which is this repository's dominant
  defect shape; asserting it both ways took the scenario to 22/0.
- All four gates green: test-boot-v2 105/0; shellcheck 165 discovered, 0
  known-failing, 0 newly failing; suites 75 / 71 in CI / 4 exempt; doc verbs 250
  valid / 8 deliberate / 0 not a command, 191 documented / 114 declared
  undocumented / 0 undocumented and undeclared / 0 stale.
- L-001 evidence appended with ROUND 29, rounds 25/26/27/28 verified still
  present afterwards. L-002 and L-003 kept `blocked` and given evidence for the
  first time, so the reason no longer lives only in this card.

## IN PROGRESS
- Nothing.

## FOUND
- **PCR 0 MOVES, AND THE POLICY DOES NOT CARE.** `luks-firmware-code`, 22
  passed / 0 failed / 1 could-not-run. Two Fedora OVMF builds differing in edk2
  revision and nothing else - `edk2-d46aa46c8361` (20250812, supplied through
  the new `$APEX_BOOTLAB_FW_ALT`) and `edk2-2970e5699ba6` (20260812, the lab
  image's) - both Secure Boot, both 4 MB, all three boots from a fresh copy of
  ONE varstore template. PCR 0 `0FA5AE84...` -> `0E338031...`; PCR 11 stayed
  `22C006FB...`; the signed PCR 11 policy still unlocked; a control volume bound
  BY VALUE to PCR 0 refused in that same boot; the marker written under the old
  firmware was read back under the new one. THE TPM IS EMULATED.
- **PCR 7 IS IDENTICAL ACROSS THE TWO EDK2 REVISIONS** (`933DE452...`). Not
  planned, and worth more than the thing that was: a firmware version change
  moves the code register and leaves the Secure Boot policy register alone. The
  scenario reports it and asserts nothing about it, because that is a fact
  about edk2 rather than about APEX.
- **THE SUPPLIED FIRMWARE IS CHECKED, NOT TRUSTED.** Its revision comes out of
  the binary (`grep -a` for the rpm build path; the lab image has no binutils),
  and it must refuse an unsigned UKI before anything rests on it. Without that,
  a build with Secure Boot compiled out would move PCR 0 for the obvious reason
  and the run would report that a firmware update had not broken the policy,
  about a firmware that had stopped checking signatures. The trap is real and
  sits in this unit's own scratch dir: `fw-nosmm/OVMF_CODE_4M.secboot.fd` is a
  NON-secboot build under a secboot filename.
- **A MISSING TPM DOES NOT STRAND THE USER.** `luks-no-tpm`, 16/0/1. One volume
  booted twice with the TPM device as the only difference. Without it:
  `tpm-device=absent`, systemd's own *"No TPM2 hardware discovered and EFI
  firmware does not see it either, falling back to traditional unlocking"*,
  `tpm-unlock=REFUSED`, `recovery-unlock=SUCCESS`, `recovery-marker=found`. The
  qemu exit status is asserted, because a guest hanging on a TPM that will
  never answer strands the user as surely as one that refuses. Again emulated,
  and it does not model a machine that NEVER had a TPM - such a volume could
  not carry a systemd-tpm2 token at all.
- **A SECOND OVERCLAIM FROM ROUND 28 IS WITHDRAWN.** Round 28 said the PCR
  7-bound control volume refuses the TPM unlock and then opens with the recovery
  key in the same boot. It does not. The control carries no recovery key by
  construction, `guest-luks-probe.sh`'s control branch has no recovery path, and
  no `recovery-unlock` line appears in any serial log of any firmware-change
  boot (checked in r3-luks-firmware-change and r7-fwcode). TWO scenarios show
  refusal followed by recovery in one boot: `luks-tpm-clear` and the new
  `luks-no-tpm`. Found by reading the logs, because the source is what made the
  claim sound right.
- **set-status.py COULD NOT WRITE EVIDENCE TO THE LAST TASK IN roadmap.yaml.**
  Its body regex ran to the next `- id: ` line, so for the final task - L-003 -
  it swallowed the whole `global_agent_rules` block and produced a file that
  does not parse. The guard refused to write, so nothing was ever corrupted and
  nothing could be recorded either. Fixed to capture the run of indented lines,
  after checking that both expressions capture identical bodies for all 127
  other tasks.
- **A SCENARIO CAN BE WRITTEN AND NEVER REGISTERED**, and a default run then
  reports green without it. `tests/test-boot-v2.sh` now compares the
  `scenario_*` functions against `--list` as sets in both directions.
- **docs/boot-v2.md's provenance header had gone stale by two machines and four
  rounds.** It said every figure below it came from the katana on 2026-09-03 at
  kernel 7.1.5-cachyos1. Every LUKS and TPM result since 2026-09-14 ran on the
  L16 against a root staged from 7.2.3-cachyos2, which each run's own `.apexinf`
  line records. It now names both.
- Older, still true: the S3 failure is an edk2 regression, not Secure Boot and
  not SMM (20250812 resumes, 17/0; both 20260812 builds assert at
  `MemoryServices.c(203)`). TPM clear 20/0 with `tpm-unlock=REFUSED` ->
  `recovery-unlock=SUCCESS` in one boot. The documented one-command TPM-clear
  recovery is a no-op that reports success; two invocations work. PCR 11 is all
  zeros on the L16, so L-003 cannot be flipped on for the shipped image. L-002
  is not a default to flip: the installer cannot encrypt at all. The lab image
  ships no diffutils; the APEX initramfs has no `sync` or `dd`; swtpm state
  persists across host sessions.

## BLOCKED ON
- L-001's "Real TPM" acceptance needs silicon. Not an agent decision. The
  procedure is now written: docs/boot-v2.md, "The run somebody with hardware
  would have to do".
- L-002 and L-003 stay blocked behind it, per state/queue.json. L-002 has a
  second blocker that silicon would not fix: the installer cannot encrypt a
  disk at all.
