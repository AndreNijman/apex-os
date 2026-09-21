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

---

## ROUND 38 CONTINUATION — L-002 remainder, THEN L-003 — orchestrator, 2026-09-21 13:55 AWST

The round-37 agent died at the 12:06 shutdown while `luks-boot-run2` was
finishing. The unit finished anyway (that is why it was a systemd unit).

### run2 result, read off disk by the orchestrator

`/var/lab-scratch/luks-boot/run2.log`, `END 2026-09-21T12:06:02+08:00`:

```
    verdict=switched-root   qemu-rc=-9   prompt-seen=yes   typed=yes
    switched-root=yes       welcome-seen=no
boot driver exit=0
PASS  the guest reached the passphrase prompt
PASS  the passphrase was typed on the emulated keyboard
PASS  dracut switched root — the disk this installer wrote BOOTS first time this has been measured
6 passed, 1 failed
artefacts kept in /var/lab-scratch/apex-luks-boot.GLHdmI
EXIT_CODE=1
```

**L-002's blocker (a) is CLOSED.** The one FAIL:

```
mount warning:
      * loop1p1: Can't mount, would change RO state
FAIL  the ESP mounts so the fallback loader can be checked mount of /dev/loop1p1 failed
```

That is `test-installer-luks-boot.sh:234`, `mount -o ro "$ESP_PART"`. The
kernel says "would change RO state" when the device is already mounted with
the other RO state — most likely the install phase's own ESP mount
(`$MNT/boot/efi`) had not been released when the check ran. Hypothesis, not
confirmed: confirm it from the kept dir (`bootmnt/`, `engine-stdout.txt`)
and fix the sequencing so the fallback-loader check actually runs. After
the reboot `losetup -a` shows only loop0 (`apex-user.raw`); nothing leaked.

### What the orchestrator did

- Landed `task/luks-boot` (fdb97b04) into `roadmap/v2.2` as **`d00c3060`**,
  pushed; tip is now `71bc2177`. Landed AHEAD of you so L-003 builds on
  `luks-boot-drive.py` instead of re-deriving it.
- Set **L-002 → partial** with the run2 record prepended to its evidence
  (roadmap.yaml ~line 8332). Read that head before your own set-status.
  `set-status.py` REPLACES: use `/var/tmp/apex-work/round24-prepend.py`
  from `/var/home/andre/Projects/apex` with a JSON `[[id, status, text]]`.
- Fixed the worktree upstream to `origin/task/luks-boot` (it tracked
  `origin/roadmap/v2.2`, so a bare `git push` was a coin toss).

### NEXT — L-002 remainder (small; do this first)

1. Merge `origin/roadmap/v2.2` (71bc2177).
2. Fix the ESP-fallback check sequencing; the check must run and say
   whether `EFI/BOOT/BOOTX64.EFI` is there.
3. Boot-half-only re-run against the kept image in
   `/var/lab-scratch/apex-luks-boot.GLHdmI` (~5 min): `systemd-run --user`,
   wrapper appends a literal `EXIT_CODE=` line. Repeating the boot, not the
   25-minute install, is the half where a fluke would live.
4. `welcome-seen=no`: decide whether first-boot welcome is a criterion of
   this suite and say so in the suite and the evidence.
5. set-status L-002 (prepend). Evidence file
   `ROADMAP/evidence/L-002-luks-boot-20260921.md` in the apex-os repo.

### THEN L-003 — luks-enroll, re-scoped

Read `agents/luks-enroll-2.md` in full. Its `luks-pcr7` scenario (a dracut
pre-mount hook running the SHIPPED `apex-luks-enroll` against a live PCR 7
extended by a real Secure-Boot chain, three boots) landed as `34132080`
but **has never been run** — it is in `run-scenarios`' `STAGED` list, and
that card's NEXT item 1 is still "run it for real". L-003's stated block,
"pending L-002 exercising this path end to end on a real boot", is lifted
by run2 above — say so in L-003's evidence.

- Where to run: katana has the bootlab and a real APEX root
  (`apex-stage-root` against `/` takes seconds). Its scratch is
  `/var/lab/scratch/luks-enroll-2/` — **`/var/lab-scratch` does not exist
  on katana**. Or here on the L16: `localhost/apex-bootlab` is in root
  podman storage (checked 11:50). If katana: `ssh katana apex game status`
  must read `active : false` first (it did at 13:50); never interrupt
  gaming.
- Rebuild the bootlab image before the run (the cached one predates the
  scenario). `systemd-run --user`, never `nohup &`.
- Assert the enrolled token CARRIES a PCR policy (non-zero policy hash),
  not that cryptenroll exited 0 — systemd 258.10 `--tpm2-device=auto` with
  no PCR argument binds to nothing.
- Keep the `--device DEV --recovery-out PATH` interface; exit 0 on every
  declined case.
- set-status L-003 (prepend) with what the run measured, and the evidence
  file. If the scenario cannot run (`cannot` → COULD-NOT-RUN), record that
  as the result; do not reword a non-run as a pass.

Battery was 58% and DISCHARGING at 13:42 on this laptop. Read
`/sys/class/power_supply/BAT*/status` before every run over ten minutes;
one heavy podman/qemu job at a time on this machine.

---

## ROUND 39 CONTINUATION — written by the orchestrator, 2026-09-21 17:20 AWST

The round-38 agent was killed by a **session usage limit at 14:59 AWST**
(`ROADMAP/state/autoresume.log`: `resume session ended (exit 1)`). It was never
messaged. You are a FRESH agent and this card is your whole inheritance —
everything above stands unless this section contradicts it, and where it
contradicts it, this section wins.

**Hard deadline: this orchestrator runs under `timeout 4h` and dies at about
21:08 AWST.** Commit and push small and often. Update this card after every
commit and whenever NEXT changes — a card that is only correct at the end is
worth nothing, which is the entire reason this directory exists.

**Write `## LANDABLE` at the top of this card, with the sha, the moment your
branch is ready to merge onto `roadmap/v2.2`.** The orchestrator lands on that
signal and will not guess. If it is NOT landable, say why in one line —
"landing this would break X" is a finding, not a failure.
__BODY_

### The contract (ROADMAP/state/README.md, short form)

Keep this card's `NEXT` / `DONE` / `IN PROGRESS` / `FOUND` / `BLOCKED ON`
sections current **as you go, never at the end**. `NEXT` is load-bearing: one
line, the exact next action, specific enough that a stranger could do it.
Everything else can be re-derived from git; the next action cannot.

### Constraints (non-negotiable)

- Never push `main`, never open a PR — final integration only.
- **Headless only.** Never open a window on Andre's desktop. Never run `qs -p`.
- No polkit and no keyring prompts (`sudo` / `--user`, never an agent helper).
- Never `pkill apex-agentd`.
- Do not interrupt gaming on katana.
- Scratch goes in `/var/lab-scratch/<your-slug>/`, NOT `/tmp` (tmpfs, 15 GB on
  29 GB RAM — a stdout-only Bash failure there is memory, not disk). The
  scratchpad is shared between agents: use your own subdirectory.
- Long builds run in the FOREGROUND or under `systemd-run --user`; a
  backgrounded `podman` gets SIGTERMed and still exits 0.
