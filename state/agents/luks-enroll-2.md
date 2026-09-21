# luks-enroll-2 — continuation of luks-enroll (L-003)

items: L-003
repo: apex-os
worktree: /var/tmp/apex-work/wt-luks-enroll-2
branch: task/luks-enroll-2, cut fresh from roadmap/v2.2 @ 76aa2b95 (2026-09-21)

luks-enroll's own branch already LANDED (checked this round with
`git merge-base --is-ancestor origin/task/luks-enroll origin/roadmap/v2.2`,
confirmed true) — do not redo its work. Read ROADMAP/state/agents/luks-enroll.md
for the full history. L-003 is still `blocked` (depends on L-002 / luks-installer
actually calling this path on a real boot) — do not expect to close it solo.

**DO NOT re-open "the disagreement" section of the predecessor card** (whether
signed PCR 11 needs Secure Boot). It is measured, not argued, and two records
already in this repository agree with the implementation as built
(ROADMAP/evidence/L-001-katana-tpm-20260919.md §3.1, and
apex-luks-enroll's own header comment). Overruling it needs a
`SB_ENFORCING` clause and a real counter-argument for how an attacker is
stopped from replaying the PCR11 extends — the predecessor agent could not
construct one and neither should you without new evidence.

## NEXT (dispatched with, fill in as you go)
1. The in-BOOT half of the PCR 7 binding is the biggest remaining hole:
   everything proven so far is host-side (LUKS2 header, TPM2 slot unsealing,
   stopping unsealing when PCR 7 moves). A guest scenario that boots a
   PCR-7-bound volume needs a staged APEX root (apex-stage-root) — build it
   as a new scenario in the `STAGED` category, not `ALL`.
2. `probe_signed_pcr11` is written and reachable but has never been taken by
   anything in the lab. If systemd-boot work has landed by the time you pick
   this up, add a fifth scenario running it inside a UKI guest.
3. The lab is on katana: `podman build -t localhost/apex-bootlab -f
   bootlab/Containerfile .` then `run-scenarios --work /work/bootlab-work/out
   enroll-sb-on enroll-sb-off enroll-no-tpm enroll-bare-policy`. Run under
   `systemd-run --user`, NEVER `nohup &` (a backgrounded podman gets SIGTERMed).
   **Use /var/lab (483G free) for anything large, NOT apex-root paths** —
   katana's apex-root is at 95% (50G free of 954G) per katana-runner's
   measurement this round. Check `apex game status` on katana before starting
   (was `active: false`, no scx scheduler, at last check 2026-09-21 10:xx —
   re-check, don't assume it's still true).

## FOUND
-

## BLOCKED ON
- L-003 stays blocked pending L-002 (luks-installer-2) actually exercising this
  path end to end on a real boot.
