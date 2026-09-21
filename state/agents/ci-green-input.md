# ci-green-input — the two reds on roadmap/v2.2's PR validation

items: none (no roadmap id — CI red on the integration branch, found by the orchestrator)
repo: apex-os
worktree: /var/tmp/apex-work/wt-ci-green-input  (EXISTS — round 39 created it, empty; reset to f3b1b3d4)
branch: task/ci-green-input, cut from origin/roadmap/v2.2
lab: /var/lab-scratch/ci-green-input

Dispatched round 39; re-dispatched round 40, 2026-09-22.

## NEXT

- Edit `tests/test-apex-input.sh`: extract the shipped `NIRI_BIN=` line into
  `${WORK}/niri-bin-line.sh`, source it from `run_inc` before `$INC_BLOCK`,
  assert it is exactly one line and that it sits BEFORE the 6a marker, and add
  an "nothing unbound" assertion on `$out`. Then re-run the suite alone.

## DONE

## IN PROGRESS

- Worktree reset to `f3b1b3d4` (current origin/roadmap/v2.2). Nothing committed.
- apex-input fix designed (see NEXT). It touches the SUITE only — the shipped
  provisioner is correct.

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
- **REPRODUCED locally, alone: `apex-input: 101 passed, 8 failed, 0 skipped`.**
  Identical 7 as CI plus `a broken include is rejected` (that one only runs
  where niri exists; the L16 has niri 26.04, the runner does not).
- **The regression is `37497975` (2026-09-19, "Safe Graphics… niri ran two
  bars").** `2605db27` had deliberately put `NIRI_BIN=` *inside* block 6a — its
  message says "defined inside 6a so the block stays self-contained for the
  suite that extracts and runs it". `37497975` added block 6a-pre (the waybar
  disable) above it and MOVED the `NIRI_BIN=` line up to share it. The suite
  extracts `/^    # ── 6a\./,/^    fi$/`, so the line left the extracted range
  silently. **The shipped script is correct** — at runtime NIRI_BIN is assigned
  at the top of step 6's if-body, before every use. Only the extraction
  contract broke, and nothing asserted it.
- Proof the cause is exactly that: `it refuses and says which way it refused`
  (the missing-include-target branch) still PASSES — it is the one branch that
  returns before touching `${NIRI_BIN}`.
- **A latent blind assertion in the same suite:** `a setting changed in
  Settings reaches niri through the include` passed in the broken run, because
  it only checks `niri validate` + `grep repeat-rate 42` in the *generated*
  file — neither of which needs the include to exist.
- **secret-broker's single failure is `the session's script actually ran`** in
  the "a confined session cannot read the credential" section. **CLASSIFICATION
  UNKNOWN — do not repeat the test's own diagnosis, it is wrong here.** The
  test prints *"the sandbox did not come up... uid map: Permission denied…"*,
  but the CI log refutes that: the `Install bubblewrap` step printed
  `kernel.apparmor_restrict_unprivileged_userns = 0`,
  `bubblewrap works: a confined session can be built`,
  `dev.tty.legacy_tiocsti = 0`, bwrap 0.9.0 installed; the session started
  (`PASS a confined session started (id 1)`); and the FIRST line of the
  in-sandbox script *did* reach the transcript
  (`--- can the session read the credential file directly? ---`). So the
  sandbox came up and the script stalled after line 1, or the transcript
  stopped updating. The 25 s is exactly the poll budget (100 x 0.25 s).
- **Finding in its own right:** that gate conflates "never started" with
  "started and stalled at line N", and prints a diagnosis that was false on the
  very run it fired on. It should say where the transcript stopped.

## BLOCKED ON

- nothing
