## LANDABLE — `345c0557`

The `luks-pcr7` guest scenario now RUNS and is green: **26 passed, 0 failed**,
`EXIT_CODE=0` on katana. It is the in-boot half of L-003 and it had never been
executed before today. Three commits fix the scenario (nothing shipped changed
— the diff against `roadmap/v2.2` is exactly `files/scripts/boot-v2/run-scenarios`,
`files/scripts/boot-v2/guest-luks-enroll.sh` and one new evidence file), plus a
merge of `roadmap/v2.2` @ `69253336`.

Gates on this tip: `test-boot-v2` 150/0; shellcheck coverage 202 discovered,
**0 newly failing**; doc-verbs 0 undocumented-and-undeclared;
check-containerfile-assertions exit 0; `shellcheck -S warning -x` and `bash -n`
clean on both changed files.

Landing it breaks nothing: it only makes a STAGED scenario work that currently
aborts after its first boot.

---

# luks-boot — L-002, finish it: the installer makes a LUKS2 disk AND that disk boots

items: L-002 L-003
repo: apex-os
worktree: /var/tmp/apex-work/wt-luks-boot
branch: task/luks-boot

Predecessor cards to read before touching anything:
`ROADMAP/state/agents/luks-installer.md` (first encrypted install, LANDED round 35),
`ROADMAP/state/agents/luks-installer-2.md` (the boot suite + `--check-passphrase`;
its work is already MERGED into this branch at 3e235bc1 — do not redo it),
`ROADMAP/state/agents/efivars-guard-2.md` (`--generic-image`, already resolved).

## NEXT (round 39 — L-003)
1. **The next real piece of work is NOT this branch.** Add a `luks-pcr7-signed`
   scenario: same `build_pcr7_enroll_bundle`, same `guest-luks-enroll.sh`, the
   ONLY change is building boot A's UKI WITH `--pcr-key`/`--pcr-pubkey` (as
   `luks_uki`'s "good" spec does). Boot A measured that
   `probe_signed_pcr11`'s Secure-Boot and sd-stub gates ALREADY PASS in this
   lab and it refuses only for the missing signing key, so this should flip
   the selection to `binding=signed-pcr11`. Assert that, and the ABSENCE of
   the "not using signed-pcr11" line.
2. Then the integration gap L-003 still has: no test has ever run the REAL
   `apex-luks-enroll` through `apex-install` (every installer suite uses the
   `APEX_LUKS_ENROLL_LOCAL` recovery-only stand-in), and no installer-produced
   disk (GRUB+shim, no sd-stub) has been TPM-auto-unlocked.

## DONE (round 39)
- Read this card, `luks-enroll-2.md`, `luks-enroll.md` in full.
- Read katana's UNREAD `serial-pcr7-a.log` from 10:45 today. See FOUND #5 —
  the scenario HAS been run once; the card saying "never been run" was stale.
- `325df6c4` bundle env/dirname/wc/head + interpreter probe + passphrase proof.
- `31a3419b` the self-`cp` abort and the wrong signed-pcr11 gate assertion.
- `2225d94c` the recovery key's trailing newline, + a host-side length guard.
- `22b9295d` `ROADMAP/evidence/L-003-pcr7-inboot-20260921.md`.
- `345c0557` merge of `roadmap/v2.2` @ `69253336`. All PUSHED.
- Ran `luks-pcr7` three times on katana; run3 green, 26/0.
- Gates: test-boot-v2 150/0; shellcheck coverage 202 discovered / 0 newly
  failing (the `android/tools/release-version.sh` failure seen mid-round came
  in WITH `roadmap/v2.2` and was already fixed on its newer tip — merging
  cleared it, it was never mine); doc-verbs 0 undocumented-and-undeclared;
  check-containerfile-assertions exit 0.
- set-status L-003 `blocked` -> `partial`, evidence PREPENDED via
  `round24-prepend.py` (verified: nothing after L-003 in roadmap.yaml moved).

## NEXT — superseded (round 38 and earlier, kept for history)
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
- **Nothing is running.** `luks-pcr7-run3.service` on katana FINISHED at
  17:36:31 AWST: **26 passed, 0 failed, `EXIT_CODE=0`**.
- Primary artefacts, do not delete:
  `/var/lab/scratch/luks-boot/run3.log` and
  `/var/lab/scratch/luks-boot/out/serial-pcr7-{a,b,c}.log` on katana.
- (round 38, finished) `luks-boot-run2.service` — L-002, landed as `d00c3060`.

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
### NEXT — rewritten for round 39. L-002 is done; you are here for L-003.

**Round 38 landed your branch as merge `d00c3060` and recorded L-002 as
`partial`** — the boot proof went through: a disk `apex-install` wrote has
BOOTED, the first time that has ever been measured, and `switched-root=yes`.
`installer/luks-boot-drive.py`, `installer/test-installer-luks-boot.sh`,
`apex-install --check-passphrase` and the `suites-not-in-ci.txt` lines are all
on `roadmap/v2.2`. Do not redo any of it, and do not re-run the ~25-minute
install to re-prove it.

Your unit owns **L-002 and L-003**. `resume.sh` prints `luks-installer (L-002)`
and `luks-enroll (L-003)` in its READY list; that is an artefact of the queue
keying IN HAND by unit id rather than by item, and the orchestrator has
deliberately NOT dispatched either as a separate agent. Both are yours.

In this order:

1. **Read `ROADMAP/state/agents/luks-enroll-2.md` before touching L-003.** Its
   PCR-7 in-boot binding guest scenario is already LANDED (merge `34132080`,
   `b9844c8e feat(boot-v2): a STAGED guest scenario for the PCR 7 in-boot
   binding`). Also read `luks-enroll.md`. Between them they have already paid
   for the traps; starting from scratch is the expensive mistake here.
2. L-003 is *"TPM auto-unlock by default, where safe is a measured
   precondition"* — the load-bearing word is **measured**. `systemd-cryptenroll
   --tpm2-device=auto --tpm2-pcrs=7` is the easy half. The hard half is the
   precondition: PCR 7 is only meaningful with Secure Boot on and a firmware
   whose PCR 7 is stable across the enrolments you will actually see. Two facts
   already on the board and already paid for, do not re-derive them:
   `ROADMAP/evidence/` has the two-edk2-build PCR work — **PCR 0 is movable and
   PCR 7 turned out identical across the two Fedora edk2 revisions** — and a
   firmware *filename* is not provenance: read the edk2 revision out of the
   binary, because a non-secboot OVMF sits in scratch named `.secboot.fd`.
3. Recovery is not optional and is part of "where safe": enrolling TPM unlock
   must never be the only way in. Prove the passphrase still works after
   enrolment, in the same guest, in the same run.
4. Use the BOOT HALF of your own suite against a kept image (the
   `podman run --rm --device /dev/kvm -v $WORK:/w … luks-boot-drive.py` block,
   ~5 min) rather than a fresh install. The install is deterministic; only the
   boot phase — prompt timing and QMP typing — is worth repeating.
5. `ROADMAP/set-status.py L-003 <status> --evidence "…"` **REPLACES** the whole
   evidence string. Read `roadmap.yaml`'s current L-003 evidence first and carry
   the prior rounds' record forward, or you destroy it. (L-002's evidence is now
   13,949 chars — round 38 prepended rather than overwrote, via
   `round24-prepend.py`. Do the same.)
6. L-003 is currently the roadmap's **only `blocked` item**, and it is the last
   one. Closing it takes the board to 0 blocked.

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

## FOUND (round 39)
5. **`luks-pcr7` HAS been run once — at 10:45 today on katana — and the card
   said it never had.** Artefacts in `/var/lab/scratch/luks-enroll-2/out/`:
   `disk-pcr7-a.img`, `luks-pcr7-hook-a.cpio` (26 MB, so the bundle built),
   `serial-pcr7-a.log`. Boot A REACHED THE DRACUT PRE-MOUNT HOOK and measured:
   `have-systemd-cryptenroll=yes`, `have-python3=yes (3.14.7)`,
   `have-apex-luks-enroll=yes`, `tpm-device=present`, `secure-boot-byte=1`,
   `pcr7-pre-enroll=09D59A7648E5663744AF2050EFF9ABC7561391317DD5C8287E80141C55D7AC46`
   (a REAL firmware measurement, not zeros), `pcr11=525F9519CB34…`. Only boot A
   ran; there is no `-b`/`-c` disk. Read the artefacts before believing a card.
6. **The single defect that stopped it: `#!/usr/bin/env bash`.**
   `enroll-exit=127`, `enroll-err: timeout: failed to run command
   '/usr/libexec/apex-luks-enroll': No such file or directory` — while
   `have-apex-luks-enroll=yes`. That contradiction is the signature of a
   MISSING INTERPRETER, not a missing file: `execve` returns ENOENT for the
   shebang target. Confirmed with `lsinitrd` against katana's real
   `7.2.6-cachyos1.fc43.x86_64` initramfs: `usr/bin/bash` IS present,
   **`usr/bin/env` is NOT**. Every `files/system/libexec/apex-*` script uses
   `#!/usr/bin/env bash`, so the shebang is repo convention and is fine for
   the installer (which has coreutils); the LAB BUNDLE is what has to carry
   `env`. Note for anyone who later wants enrolment to run from a real
   initramfs in production: that path needs `env` installed by a dracut module.
7. **`have-apex-luks-enroll=yes` is a `-x` test — it inspected the file and
   said nothing about runnability.** The repo's dominant defect family. The
   hook now also reports the shebang interpreter's presence.
8. **run1 (17:27 AWST): boot A PASSED — the shipped script enrolled a TPM
   slot, live, in a real boot, for the first time.** `12 passed, 1 failed`.
   `enroll-exit=0`;
   `TPM key slot enrolled: binding=pcr7 pcrs=7 bank=sha256
   hash=91cd90cbef3cb73ad31775fbe69cb36f6112937d1b44634970b5a6b6eece3a39
   pin=no`, against a live `pcr7-pre-enroll=69E722B3756359AAE507D8D886174B62…`
   read from that boot's own firmware measurement. A recovery key was written,
   and the new `passphrase-after-enrol=SUCCESS` line proves the original
   passphrase STILL OPENS the volume afterwards. Bundling `env dirname wc head`
   was the whole fix.
9. **The scenario aborted after boot A, and the cause is in the scenario, not
   the product: `cp "$ser" "$WORK/serial-pcr7-a.log"` copies the file onto
   itself.** `vm_boot --serial` already wrote it at that exact name, so `cp`
   exits non-zero with "are the same file", and this file runs `set -euo
   pipefail`. Boots B (cold PCR 7 reproduction) and C (dbx firmware change +
   recovery unlock) never ran. All three copies removed in `31a3419b`.
10. **The signed-pcr11 assertion named the WRONG GATE, and the truth is a
   better result.** `probe_signed_pcr11` refuses in order: SB off, no StubInfo,
   no readable signing key, PCR 11 unmeasured. The scenario asserted the
   second; what fires is the THIRD — `not using signed-pcr11: the boot did not
   publish a PCR signing key at /run/systemd/tpm2-pcr-public-key.pem`. So gates
   1 and 2 PASS: this guest booted Secure-Boot-enforcing AND genuinely came up
   through sd-stub. The only thing between this lab and a signed-PCR-11
   enrolment is building the UKI with `--pcr-key` — which is exactly what
   `luks-enroll-2.md`'s NEXT #2 proposed, now with its precondition measured
   rather than assumed.
11. **The installer's PRODUCTION call path does not run the script in an
   initramfs**, so FOUND #6's `env` gap is lab-scoped and is NOT a shipped
   defect. `installer/apex-install:2189` runs `/usr/libexec/apex-luks-enroll`
   inside a `podman run --rm --privileged --pid=host -v /dev:/dev "$IMAGE"`
   container, which carries full coreutils. The installer SUITES use a
   recovery-only stand-in via `APEX_LUKS_ENROLL_LOCAL`, so no test has ever run
   the real script through the installer.
12. **Cosmetic but real: the bundled python3 prints `Could not find platform
   independent libraries <prefix>` on every invocation**, five-plus times into
   the shipped script's stderr — the stream the contract reserves for prose
   shown to a person. It does not affect correctness (every parse returned the
   right answer), but it is noise inside a user-facing stream.
13. **Boot C's recovery unlock failed on a one-byte defect in the LAB, and
   nothing shipped is wrong.** run2's boot C did the hard part right: the dbx
   update moved PCR 7 (`69E722B3…` -> `C4681679…`) and the TPM slot refused to
   unseal — `cryptsetup: TPM policy does not match current system state`, then
   `tpm-unlock=REFUSED`. The recovery arm then said
   `Failed to activate with key file '/apex-bootlab-recovery-key'.
   (Key data incorrect?)`. The key data was correct; the FILE was 72 bytes and
   a recovery key is 71. `cryptsetup --key-file <regular file>` uses EVERY BYTE
   as the passphrase, and the scenario captured the key off the serial log and
   rewrote it with `printf '%s\n'`. **Measured, not reasoned**: host-side on
   katana against that same volume, over a read-only loop device, the 72-byte
   file is REFUSED and the identical key piped without the newline OPENS.
   The shipped script writes 71 bytes (`tr -d '[:space:]'`) and
   `apex-install` re-strips (`tr -d '\r\n'`) before piping to
   `--test-passphrase`, so the product was never affected. `luks-pcr7` is the
   only scenario that recovers the key from a guest's SERIAL LOG rather than
   from the file the script wrote, which is why it alone had this. The length
   is now asserted host-side at capture time.
