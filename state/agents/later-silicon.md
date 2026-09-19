# later-silicon
items: L-001, L-002, L-003
repo: apex-os
worktree: /var/tmp/apex-work/wt-later-silicon
branch: task/later-silicon
machine: katana (MSI Katana GF76 12UG, Intel PTT fTPM)

## NEXT

Nothing is running, nothing is half-finished, and katana is clean and released.
Branch tip `f70d3ba8` is pushed, cut from `roadmap/v2.2` `36535383`.

**L-001 is no longer waiting on a machine. It is waiting on one sentence from
Andre**, and the whole question is:

> Katana's TPM is owned by a live Windows install — `lockoutAuthSet=1`, five
> persistent handles, and a Windows Hello NGC container created 2026-09-12.
> There is **no BitLocker** (verified: `blkid` says `TYPE="ntfs"`, no
> `-FVE-FS-` signature in either NTFS boot sector), so a TPM clear loses **no
> data**. It loses his **Windows Hello PIN**, which he re-sets with his
> Microsoft account password, and Windows re-provisions the TPM by itself on
> its next boot.
>
> **May the TPM be cleared?**

If yes, Run 2 of `docs/boot-v2.md` is two commands and needs nobody at the
machine — MSI's firmware answers PPI operation 5 with *"User not required"*:

```
echo 5 | sudo tee /sys/class/tpm/tpm0/ppi/request
sudo systemctl reboot
```

then re-make a loopback volume, confirm `tpm-unlock=REFUSED` →
`recovery-unlock=SUCCESS` in the same boot, confirm the five persistent handles
and the eight OS NV indices are gone while the manufacturer `0x1C000xx` certs
remain, and reproduce the two-invocation re-enrolment. That is an hour of work
and it moves L-001 from 3-of-5 runs to 4-of-5.

Run 5 (the TPM goes away) stays **COULD-NOT-RUN** whatever he answers: PPI
operation 2 disables the TPM and takes `/sys/class/tpm/tpm0/ppi` with it, so
re-enabling needs somebody in firmware setup. Do not attempt it remotely.

Run 3 (real firmware update) stays **COULD-NOT-RUN**: `fwupdmgr get-updates`
offers nothing for this board's System Firmware.

**Do NOT re-run what silicon already settled.** Runs 1 and 4 pass, and so does
the signed PCR 11 policy matrix. See `ROADMAP/evidence/L-001-katana-tpm-20260919.md`.

**Andre's Secure Boot session needs exactly three rows redone**, no more: the
`--tpm2-pcrs=7` volume, the PCR 7 value, and the event-log replay. Everything
else is independent of PCR 7. And warn him: **turning Secure Boot on will not
make `systemd-pcrextend` start working** — that needs the sd-stub/UKI boot path.

## DONE
- Round 31. Branched `task/later-silicon` from `roadmap/v2.2` `36535383`. Three
  commits, all pushed: `883b5bc0` (the evidence record), `17252b16` (the
  `docs/boot-v2.md` hardware section the document asked for, plus a
  Recovery-table row and a correction to the firmware-update row) and
  `f72e41f1` (the dual-boot boundary assertions — §0.1) and `f70d3ba8` (two
  record-accuracy corrections: a piped exit status was being quoted as the
  attach's, and the no-PCR-default finding is a guardrail — all four enrolment
  call sites in the tree already name their PCRs).
- Eight foreground runs on katana, each logged to
  `/var/tmp/apex-work/scratch-later-silicon/run{1..8}.log`, plus `s3.log`
  (suspend/resume, run as a transient unit so the dying SSH session could not
  truncate it) and `snap-before.txt` / `snap-after.txt`.
- L-001 evidence appended as ROUND 31; rounds 25–30 verified still present
  afterwards. L-002 and L-003 given ROUND 31 evidence, both still `blocked`.
  Counts unchanged at 92 done / 34 partial / 0 todo / 2 blocked.
- `state/queue.json`: the `later` unit's note rewritten, because the gate it
  named ("until a real TPM enrol-and-recover cycle has been done on hardware")
  is now **met and is not sufficient**, and a future orchestrator reading the
  old wording would unblock L-002/L-003 on it.
- Gates: `test-boot-v2` 105/0; shellcheck 165 discovered / 0 known-failing / 0
  newly failing; suites 75 / 71 in CI / 4 exempt; doc verbs 252 valid / 8
  deliberate / 0 not a command / 191 documented / 114 declared undocumented / 0
  undocumented and undeclared / 0 stale; no conflict markers.

## IN PROGRESS
- Nothing.

## FOUND
- **`systemd-cryptenroll --tpm2-device=auto` binds to NO PCRs on systemd
  258.10.** `tpm2-hash-pcrs` empty, `tpm2-pcr-bank` `n/a`, `tpm2-policy-hash` 32
  zero bytes. Not PCR 7 — nothing. `--help` documents no default either. Any
  APEX prose or script assuming PCR 7 is the default is wrong.
- **`systemd-pcrextend` is a silent no-op on a GRUB machine.** *"Kernel stub did
  not measure kernel image into PCR 11, skipping userspace measurement, too."*,
  **exit status 0**, PCR 11 unchanged. The four boot-phase policies
  `systemd-measure sign --current` signs by default are therefore unreachable on
  a default APEX install, and the failure lands at boot, not at enrolment.
- **A zero PCR 11 is a local denial-of-service.** `tpm2_pcrextend 11` works from
  plain root; `tpm2_pcrreset 11` answers `bad locality`. Every PCR-11-bound
  keyslot refuses until the next reboot.
- **The signed PCR 11 policy works on Intel PTT** — first time outside swtpm.
  Seven rows, including a forged signature refused **by the part**,
  `Esys_VerifySignature … 0x2db` (`TPM_RC_SIGNATURE`), and a re-signature for a
  moved PCR 11 restoring unlock.
- **The firmware's own TCG event log replays to all eleven live registers.**
  First event-log replay in this program against real firmware.
- **Real DA numbers, and a successful auth does not clear the counter.**
  `MAX_AUTH_FAIL` 32, `LOCKOUT_INTERVAL` 7200 s, `LOCKOUT_RECOVERY` 86400 s.
  Left at 2 of 32, decaying to 0 within 4 h of 10:05 AWST.
- **PPI operation 5 reading "User not required" is itself a finding.** Any root
  process can schedule a firmware TPM clear that runs unattended at the next
  boot — a data-loss primitive from a shell on a machine with a TPM-bound
  volume. `docs/boot-v2.md` now says to check it.
- **The round-26 one-command re-enrolment no-op reproduces with the TPM
  INTACT**, which is more general than the lab's finding.
- **APEX's enrolment coexists with a Windows-provisioned TPM.** Persistent
  handles identical by name before and after; systemd kept its SRK in the LUKS
  token rather than persisting one or reusing Windows' `0x81000001`.

## THE DUAL-BOOT BOUNDARIES, CHECKED AFTER THE RUN (§0.1 of the evidence)
`nvme0n1` — Windows's ESP, MSR, the 500 GB NTFS volume, WinRE and Andre's 1.3 TB
`games` partition — was **never written**. Every volume was a loopback file on
`nvme1n1p3`; `/sys/block` counters after the run read `nvme0n1 writes=22` vs
`nvme1n1 writes=177520`. `nvme0n1p3` was mounted twice **read-only** for the
Windows Hello check and unmounted. Neither ESP was mounted. `efibootmgr` was
only ever read, and all six entries plus `BootOrder` are byte-identical to the
09:46 reading. `/sys/class/tpm/tpm0/ppi/request` reads `255`,
`response` `0: No Recent Request` — **no TPM clear was ever requested**.

## MACHINE STATE — katana, left at 10:12 AWST 2026-09-19
Deployment `apex-266dcc57` / `sha256:ba263890b696` unchanged; **no reboot, no
rebase, no rollback, nothing pinned**. `apex-user.raw` intact at 540 012 544
bytes. `qual-backup-20260919/` and `~/apex-pre-rebase-20260919/` untouched.
greetd active on VT 1 with the same sway/quickshell pids as before this unit
started. 0 failed system and user units. No mappers, no leftover loop devices,
`/var/lib/apex-tpm-qual` removed, both transient units stopped and
`reset-failed`, inhibitors back to the stock three.

**Two TPM differences from the start, both deliberate and both self-healing:**
`LOCKOUT_COUNTER` 0x0 → 0x2 (decays to 0 in ≤4 h) and PCR 11 zeros →
`F226D37558876AA5…` (the next reboot restores it; PCR 11 cannot be reset from
locality 0). Nothing on the machine is sealed to PCR 11.

## DISCIPLINE THAT EARNED ITS PLACE THIS ROUND
- The suspend/resume run was launched with `systemd-run --unit=…` precisely
  because the SSH session dies during suspend. A backgrounded command would
  have been killed and its short log would have read like a result.
- `hypridle` suspends katana at 15 minutes idle. A
  `sleep:idle:handle-lid-switch` **block** inhibitor was held for the whole run
  and `systemctl suspend -i` was used to get past this unit's own inhibitor.
- Every "did it unlock?" row asserts `ls /dev/mapper/<name>`, never `$?` after a
  pipeline — `rc=$?` after `cmd | sed` reads sed's status, and it did.
