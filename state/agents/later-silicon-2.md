# later-silicon-2
items: L-001, L-002, L-003
repo: apex-os
worktree: /var/tmp/apex-work/wt-later-silicon-2
branch: task/later-silicon-2
machine: katana (MSI Katana GF76 12UG, Intel PTT fTPM)

## NEXT

Nothing is running, nothing is half-finished, katana is clean and released.
Branch tip `16fe7ad3` is pushed, cut from `roadmap/v2.2` `f602f1cb`.

**Andre said yes. The TPM was cleared. Run 2 is PASS.** L-001 went from 3 of 5
runs to **4 of 5** and is still `partial`.

**Do not dispatch a unit to "finish L-001". There is nothing left that an agent
can do on this machine.** Run 5 disables the TPM, and on this firmware that
removes the `/sys/class/tpm/tpm0/ppi` directory that would re-enable it — so
re-enabling needs somebody in firmware setup. Two units have now declined to
create a state they cannot leave, and that is the right answer, not caution to
be overridden. Run 3 has no firmware capsule to apply. Both are recorded in
`state/queue.json` under `_hardware_blocked`.

**What is left is Andre's, and it is two things:**

1. **Run 5**, at the keyboard: disable the TPM in firmware setup, confirm the
   boot prompts for the recovery key rather than hanging, confirm the data is
   intact, then re-enable it and confirm unlock returns **with no
   re-enrolment** — the sealed object is still bound to the same SRK, which a
   clear destroys and a disable does not. That last clause is now measured
   rather than assumed: §17.1 showed the SRK is deterministic from the storage
   primary seed.
2. **The Secure Boot session**, still exactly three rows: the `--tpm2-pcrs=7`
   volume, the PCR 7 value itself, and the event-log replay. **Round 32 adds
   none** — its volume was bound to no PCRs precisely so a Secure Boot change
   could not be confused with a TPM clear. And warn him again: **turning Secure
   Boot on will not make `systemd-pcrextend` start working.** That needs the
   sd-stub/UKI boot path.

**Andre's Windows Hello PIN is gone. That is the approved cost, not damage.**
He re-sets it at a Windows login with his Microsoft account password, and
Windows re-provisions the TPM by itself. There was no BitLocker, so no data was
ever at risk.

**A rule for anyone who touches katana again, which cost this unit a rewrite of
the predecessor's safety argument:** `/dev/nvme0n1` is **not a stable name**
there. The two NVMe controllers are probed asynchronously (`nvme1` =
`0000:02:00.0`, `nvme0` = `0000:03:00.0`) and the indices swapped across the
reboot, so the disk this programme is told never to write became the APEX disk.
Identify by serial — `Micron_2450_MTFDKBA1T0TFK` `220534D1CB81` is **APEX**,
`SPCC M.2 PCIe SSD` `240023925111005` is **WINDOWS** — or by PARTUUID, or by
label (`apex-root`, `EFI-SYSTEM`, `games`). And **APEX's default boot entry
lives on the Windows disk's ESP**: `Boot0000* APEX-OS Primary` is
`HD(1,GPT,2ba9a2ea-…)/\EFI\APEX\SHIMX64.EFI`, the same 200 MB partition
`Boot0002* Windows Boot Manager` boots from, and `BootCurrent` was `0000` on all
three boots.

L-002 and L-003 stay **blocked**, for reasons silicon has now strengthened
rather than removed — `state/queue.json`'s note for the `later` unit carries
them.

## DONE
- Round 32. Branched `task/later-silicon-2` from `roadmap/v2.2` `f602f1cb`. One
  commit, `16fe7ad3`, pushed: the ROUND 32 record (§14–§21 of
  `ROADMAP/evidence/L-001-katana-tpm-20260919.md`), forward pointers on the
  three places round 31 left stale (§7's heading, §0.1's "the TPM was not
  cleared" row, §12's TPM-clear row), and four `docs/boot-v2.md` changes: the
  katana table's Run 2 row now reads **PASS**, the Run 2 procedure names the PPI
  path and says to *verify* the clear rather than infer it, the Recovery table
  gains an evicted-SRK row, and the one-command no-op paragraph gains its
  silicon confirmation.
- Eight scripted runs on katana, each uploaded and run foreground over SSH, logs
  in the session scratchpad: `r1-prepare`, `r1b-time`, `r2-verify-clear`,
  `r3-refuse-recover`, `r4-reenrol`, `r5-srk`, `r6*-evt`, `r7-control`,
  `r8-cleanup`. Two reboots: the clear (`96c7fbc3` → `c93168d6`) and a
  **control reboot with no PPI request** (`c93168d6` → `9dad77b0`), taken so
  that "the clear moved PCR 1" could be separated from "PCR 1 moves every boot".
- L-001 recorded `partial` with ROUND 32 evidence **appended**: rounds 25–31
  verified afterwards to be an exact 28 215-character prefix of the 38 793
  stored. Counts unchanged at 92 done / 34 partial / 0 todo / 2 blocked.
- `state/queue.json`: the `later` unit's `note` and `closed` rewritten for the
  answered question, and `_hardware_blocked` gains an L-001-run-5 entry so
  nobody re-derives why the last run is not dispatchable.
- Gates: `test-boot-v2` 105/0; shellcheck 167 discovered / 0 known-failing / 0
  newly failing; suites 76 / 72 in CI / 4 exempt; doc verbs 257 valid / 8
  deliberate / 0 not a command / 191 documented / 114 declared undocumented / 0
  undocumented and undeclared / 0 stale; no conflict markers.

## IN PROGRESS
- Nothing.

## FOUND
- **`ppi/response` is not a discriminator.** It reads `5 0: Success` the instant
  the request is written, **before any reboot**, and still reads it afterwards
  while `request` reads `96`. Anything that treats it as proof reports success
  for a request the firmware has not looked at. The clear was verified instead
  by five pieces of TPM state that all moved: `lockoutAuthSet` 1→0, five
  persistent handles→none, thirteen NV indices→five, four NV counters→zero, DA
  counter 2→0 (unambiguous: the first decay step was ~90 minutes away).
  `tpmGeneratedEPS` correctly did **not** move — `TPM2_Clear` leaves the
  endorsement seed alone, so an unchanged value there is not evidence of
  failure.
- **THE HEADLINE: `tpm-unlock=REFUSED` → `recovery-unlock=SUCCESS` →
  `recovery-marker=MATCH`, in one boot, on an Intel PTT fTPM.** The lab has
  shown that since round 27; no physical part ever had. 65 ms to refuse, 76 ms
  to recover, marker byte-identical. The LUKS header was byte-identical across
  the clear: a clear destroys TPM state, not disk state.
- **The part returns two codes and systemd prints one sentence for both.**
  `Esys_Load 0x18b` = `TPM_RC_HANDLE`, the SRK at `0x81000001` is **absent** —
  recoverable with no re-enrolment. `Esys_Load 0x1df` = `TPM_RC_INTEGRITY`, the
  storage seed itself was **rolled** — only re-enrolment works. Both surface as
  `Failed to unseal secret using TPM2: State not recoverable`, and both hide in
  an `ERROR:esys:` line a script reading systemd's message never sees.
- **The SRK is deterministic from the storage primary seed, and
  `systemd-cryptsetup` never creates it.** Measured by evicting `0x81000001` and
  putting it back, not reasoned. So a machine whose persistent SRK has been
  evicted refuses every TPM unlock at boot with `TPM_RC_HANDLE`, has no way to
  repair itself from the initrd, and the recovery key is the only way in — while
  the key material was never lost.
- **`executing no operation` is a no-op on the LUKS header only.** It creates
  and persists a key at `0x81000001`, a handle shared with every other OS on the
  machine. And the one-command re-enrolment trap is now measured **after a real
  firmware clear**, which is the case `docs/boot-v2.md` is actually about:
  `rc=0`, a cheerful message, and a machine that still cannot unlock. Two
  separate invocations changed the blob and unlock worked in 298 ms.
- **A TPM clear moves PCR 1 permanently on this board.** `68CBF16D…` →
  `AA21AE08…`, and the firmware's own log grew 42 328 → 42 805 bytes, 80 → 83
  events, 8 → 11 on PCR 1 — **identical again on the control boot**, so it is a
  new steady state. Never bind a keyslot to PCR 1. Meanwhile **PCR 0, 4 and 7
  are byte-identical across three boots**, which §13.1 called a cheap extra it
  had skipped.
- **`/dev/nvme0n1` is not a stable name on katana** — see NEXT. This corrects
  §0.1's *method*; its conclusion still holds, because every volume in both
  rounds lived in a loopback file on `apex-root` whatever the kernel called it,
  and the Windows disk recorded 7 writes / 24 sectors for the whole control
  boot with neither ESP and no NTFS volume mounted.
- Incidental, and worth folding into the repo's `CLAUDE.md` if anyone edits it:
  the 2026-09-17 note describes an `ostree admin unlock --hotfix` overlay on
  `/usr`. That overlay belongs to the **rollback** deployment
  (`gaming-nvidia`, 2026-09-05); the booted one is `apex-266dcc57` from
  2026-09-18, so the hotfix has not applied since that update and
  `/usr/share/vulkan/icd.d/nvidia_icd.i686.json` was already absent before this
  round's reboots. `/usr` is now an overlay from the **sysext**, and the
  "maximum fs stacking depth exceeded" problem that note records is gone with
  the hotfix: `apex-user.raw` merged cleanly on **both** post-clear boots —
  `/usr/bin/steam` present, 538 fonts, `fc-match sans` → Noto Sans. That is the
  reboot-persistence test the note says was never performed.

## BLOCKED ON
- **L-001 Run 5** — needs a hand in firmware setup. Not an agent's. Recorded in
  `state/queue.json` `_hardware_blocked`.
- **L-001 Run 3** — `fwupdmgr get-updates` offers nothing for this board's
  System Firmware. No capsule exists to apply.
- L-002: `installer/apex-install` cannot create a LUKS header at all, and
  `systemd-cryptenroll --tpm2-device=auto` binds to no PCRs on systemd 258.10.
- L-003: `systemd-pcrextend` is a silent no-op on a GRUB machine, so a signed
  PCR 11 policy cannot be satisfied at boot on a default APEX install. Needs the
  sd-stub/UKI path, not the Secure Boot switch.
