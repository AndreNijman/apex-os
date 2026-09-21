# ci-green-input — the two reds on roadmap/v2.2's PR validation

items: none (no roadmap id — CI red on the integration branch, found by the orchestrator)
repo: apex-os
worktree: /var/tmp/apex-work/wt-ci-green-input  (EXISTS — round 39 created it, empty; reset to f3b1b3d4)
branch: task/ci-green-input, cut from origin/roadmap/v2.2
lab: /var/lab-scratch/ci-green-input

Dispatched round 39; re-dispatched round 40, 2026-09-22.

## NEXT

- Read `files/system/libexec/apex-input-apply` + the niri include block in the
  provisioner; find where `NIRI_BIN` is set and why the extracted block at
  `niri-include-block.sh:30` runs with it unbound. Reproduce locally by running
  `tests/test-apex-input.sh` ALONE in the worktree.

## DONE

## IN PROGRESS

- Worktree reset to `f3b1b3d4` (current origin/roadmap/v2.2). Nothing committed.

## FOUND

- **The brief's table is stale, confirmed against run 35625128495.**
  `mux-layouts` is **47 passed / 0 failed / 0 skipped in CI** now — the workflow
  installs zellij 0.45.1 explicitly (log line 3487-3498) and all three
  previously-red zellij assertions pass. That half of the unit is already done
  by someone else; do not touch it.
- **The two live reds are `apex-input` (93/7/8) and `secret-broker` (83/1).**
- **apex-input's single cause is named in the CI log, line 410:**
  `/tmp/tmp.XXXX/niri-include-block.sh: line 30: NIRI_BIN: unbound variable`
  The suite extracts the provisioner's niri-include block into a standalone
  script and runs it; the block reads `NIRI_BIN`, which is bound somewhere else
  in the provisioner. The block aborts before appending, hence `8 -> 8`
  (nothing added) and all six downstream assertions.
- **secret-broker's single failure is `the session's script actually ran`** in
  the "a confined session cannot read the credential" section. The test prints
  its own diagnosis: *"the sandbox did not come up... a 'uid map: Permission
  denied' here means unprivileged user namespaces are blocked — see the CI
  sysctl"*. It burned ~25 s (16:29:07 -> 16:29:32), i.e. a timeout. This is the
  runner-environment class, not a regression.

## BLOCKED ON

- nothing
