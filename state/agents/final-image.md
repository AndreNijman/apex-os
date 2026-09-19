# final-image — P0-001, build the image from the v2.2 tip and confirm two fixes on katana

repo: apex-os
branch: task/final-image (cut from roadmap/v2.2 @ 661a9d80, no commits yet)
worktree: /var/tmp/apex-work/wt-final-image

## THE RUN ID — record first, lose nothing

**Run 35469530380** — workflow_dispatch, build-image.yml, ref task/final-image
@ 661a9d80, queued 2026-09-19T21:08:57Z.
https://github.com/AndreNijman/apex-os/actions/runs/35469530380

Expected tag: ghcr.io/andrenijman/apex-os:apex-661a9d80<...>  (per-SHA tag; the
workflow does not move a floating tag off a task/ branch).

`gh run view --log` REFUSES while a run is in progress. Use
`gh api repos/AndreNijman/apex-os/actions/jobs/<id>/logs`.
A **skipped** job reads as success here — read each job's STEPS.

Diff 97c9e8f2..661a9d80 touches no Containerfile* and no kernel/**, so **core
must not rebuild**: base + flavors only. If the run took ~45 min longer than
expected, core rebuilt and something is wrong with the path filter.

## NEXT

1. Wait for run 35469530380. Read job steps, not conclusions.
2. On katana (NOT the L16): rebase to the new per-SHA tag, reboot.
3. Verify the two fixes — plan and readings below as they are taken.

## Status

IN PROGRESS — build dispatched 2026-09-19T21:08:57Z.
