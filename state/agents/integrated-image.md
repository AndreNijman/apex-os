# integrated-image — build the integrated image and qualify it on katana

Repo: apex-os. Branch `task/integrated-image` @ 97c9e8f2 (= roadmap/v2.2 tip,
pushed 2026-09-19T18:32Z). Worktree `/var/tmp/apex-work/wt-integrated-image`.

## IMAGE BUILD — RECORD THIS FIRST
- **Run 35461554871**, workflow_dispatch on `task/integrated-image` @ 97c9e8f2,
  queued 2026-09-19T18:33:05Z.
  https://github.com/AndreNijman/apex-os/actions/runs/35461554871
- A non-main dispatch pushes only per-SHA tags, so the artifact to expect is
  `ghcr.io/andrenijman/apex-os:apex-97c9e8f25ee55593a97502505f51c6115ebbee7c`.
  It moves no floating tag; the fleet is untouched.
- FALLBACK IF IT FAILS: run **35457162588** (success, 1h17m, task/sbom-attest
  @ fd456c5a) already built this exact source tree for everything that lands in
  the image — `git diff fd456c5a 97c9e8f2` touches only
  `.github/workflows/build-image.yml` and deletes `sbom-probe.yml`. Its tag is
  `apex-fd456c5acf1531beeb1e90cf050a1d9c3a47ef93`. Check the shell sha it
  resolved before treating it as equivalent.

## Boundaries (from the brief, not negotiable)
- **Do not touch the L16.** Booting the L16 is Andre's half of `final`.
- **Do not merge to main, do not open a PR.** Order when Andre does it:
  apex-shell roadmap/v2.2 -> main FIRST (Containerfile.base carries
  ARG APEX_SHELL_REF=main), then apex-os roadmap/v2.2 -> main.
- katana: never write to the Windows disk (serial 240023925111005) — it holds
  APEX's own Boot0000 ESP and Andre's 1.3 TB games library. APEX disk is
  Micron serial 220534D1CB81. `/dev/nvme0n1` is not stable across reboots.
- Keep the rollback deployment and `~/apex-pre-rebase-20260919/` intact.

## Katana starting state (measured 2026-09-19T18:3xZ, before anything)
- booted `ghcr.io/andrenijman/apex-os:apex-7f647470e222cfa23e0853cac45ef3f7e74c252e`
  digest sha256:be3bdd0c…3aafb, deployed 2026-09-19T10:09Z
- rollback `apex-266dcc572c51bdf9ec421d79eaa8784583184cd2`
  digest sha256:ba263890…503d

## NEXT
Wait on run 35461554871, then `bootc switch` katana to the per-SHA tag and
work the four verifications (pkg-update, gaming-release, p2-b i18n, coredump
drop-in). Discriminators are in the `closed` notes of those units in
ROADMAP/state/queue.json — do not invent new ones.
