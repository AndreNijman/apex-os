# ci-green-input — the two reds on roadmap/v2.2's PR validation

items: none (no roadmap id — CI red on the integration branch, found by the orchestrator)
repo: apex-os
worktree: /var/tmp/apex-work/wt-ci-green-input  (EXISTS — round 39 created it, empty; reset to f3b1b3d4)
branch: task/ci-green-input, cut from origin/roadmap/v2.2
lab: /var/lab-scratch/ci-green-input

Dispatched round 39; re-dispatched round 40, 2026-09-22.

## NEXT

- secret-broker. First, is it a flake or a regression?
  `gh run list --workflow pr-validation.yml --branch roadmap/v2.2 --limit 15`,
  then for each run whose `Package engine` job actually RAN, read its
  `secret-broker: N passed, M failed` line. If it was green at an earlier SHA
  on the same test code, bisect `<green>..e02561fb` over `apexd/`,
  `tests/test-secret-broker.sh`, `tests/in-login-session.sh`. In parallel,
  reproduce locally ALONE under `systemd-run --user` with
  `APEX_REQUIRE_SANDBOX=1` (cold cargo build in this worktree — minutes).

## DONE

- **apex-input is green and pushed: `cd3c06b6` on `origin/task/ci-green-input`.**
  `112 passed, 0 failed, 0 skipped` run alone (was 101/8/0). Suite-only change;
  the shipped provisioner is untouched and was never broken.
- Mutation-tested both new invariants rather than assumed: deleting the
  `NIRI_BIN=` line -> `100 passed, 12 failed` including all three new
  assertions; moving it after its first use -> `111 passed, 1 failed`, exactly
  the ordering assertion.

## IN PROGRESS

- Branch `task/ci-green-input` is at `cd3c06b6`, pushed, one commit ahead of
  `f3b1b3d4`.
- secret-broker + mux-layouts: flake mechanism not yet named. Nothing written
  for either.

## FOUND

- **CORRECTED — an earlier version of this card said mux-layouts was fixed.
  IT IS NOT. It is a FLAKE, and so is secret-broker.** Measured over the eight
  most recent `roadmap/v2.2` runs whose `Package engine` job actually ran:

  | suite | red runs |
  | --- | --- |
  | `apex-input` | **8 / 8** (a real regression — fixed, see DONE) |
  | `mux-layouts` | **4 / 8** (44 passed/3 failed vs 47/0) |
  | `secret-broker` | **2 / 8** (83 passed/1 failed vs 93/0) |

- **Proof they are flakes and not regressions, checked rather than assumed:**
  across `1156daa4 69253336 4e7aa3fa 11c45d36 10b2f449 9eef37a2 89036ec7
  44c9a5cb e02561fb` — reds and greens interleaved — `tests/test-secret-broker.sh`
  is blob `8668547a35` at EVERY one, `apexd/` is tree `fd247b5193` at every one,
  `tests/test-mux-layouts.sh` is `d8658e648e` and `files/system/libexec/apex-mux`
  is `afe43acead` at every one. Identical bytes, different outcomes. There is
  nothing to bisect. (Use `bash -c` for `git rev-parse ref:path` — zsh eats it.)
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
- **The two red secret-broker runs are byte-identical in shape** (35616795800
  at 15:15:20->15:15:46, and 35625128495 at 16:29:07->16:29:32): exactly ONE
  transcript line, the full 25 s, zero further progress. That is a STALL, not
  slowness — a longer ceiling alone will not fix it.
- Not a transcript-buffering artefact: `Session::write_log` in
  `apexd/apex-agentd/src/registry.rs` holds `log: Option<File>` (unbuffered)
  and `write_all`s straight to the fd. The 58 bytes on disk are the 58 bytes
  `absorb()` was given.
- **Does not reproduce on the L16**: `93 passed, 0 failed` run alone, and again
  under `CPUQuota=20%`. CPU starvation is not the mechanism.
- **Finding in its own right:** that gate conflates "never started" with
  "started and stalled at line N", and prints a diagnosis that was false on the
  very run it fired on. It should say where the transcript stopped.

## BLOCKED ON

- nothing
