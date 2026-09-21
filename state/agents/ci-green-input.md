# ci-green-input — one regression and two flakes in roadmap/v2.2's PR validation

items: none (no roadmap id — CI red on the integration branch, found by the orchestrator)
repo: apex-os
worktree: /var/tmp/apex-work/wt-ci-green-input  (EXISTS — round 39 created it, empty; reset to f3b1b3d4)
branch: task/ci-green-input, cut from origin/roadmap/v2.2
lab: /var/lab-scratch/ci-green-input

Dispatched round 39; re-dispatched round 40, 2026-09-22.

## LANDABLE 65be1e49

Branch `task/ci-green-input`, 8 commits on `origin`. The three that change code
are `cd3c06b6` (apex-input suite), `762c5204` (secret-broker suite),
`9c389baf` (`apex-mux` + mux-layouts suite); the rest is
`ROADMAP/evidence/ci-green-input-20260921.md`. Land by merge.

## NEXT

- Nothing in this unit. Two follow-ups belong to OTHER units and are written up
  in the evidence file — see FOUND: the installer-locale console-keymap red
  (deterministic, 4/4, and it WILL fail the merge-to-main PR), and the
  `killed by signal 9` on a confined session in CI (an `apexd/` question).

## DONE

- **CI MEASURED, four `workflow_dispatch` runs on the branch
  (`35647154151 35647166056 35647177148 35647188306`, all at `9c389baf`):**

  | step | before | after |
  | --- | --- | --- |
  | `Run input-settings assertions` | **8/8 red** | **4/4 GREEN** |
  | `Run terminal layout template assertions` | 4/8 red | 1/4 red (`35647154151`) |
  | `Run secret-broker assertions` | 2/8 red | 1/4 red (`35647177148`) |

  So the regression is fixed and **both flakes fired once each — which is the
  payoff, not a setback**: each fired with the new diagnostics in place, so for
  the first time the CI log says what actually happened. Four runs cannot show
  a flake is cured and no such claim is made.
- **All four runs report overall `failure`, and that is NOT this unit.** The
  overall tick could never have been green: `Installer safety and UI` fails in
  **4 of 4** (deterministic, pre-existing, out of bounds — see FOUND). Read the
  step conclusions, not the tick.
- **`apex-input: 103 passed, 0 failed, 8 skipped` in CI — exactly the predicted
  number** (93 + the 7 repaired + 3 new; the strengthened assertion is in the
  niri-only section, which skips on the runner). Diffing **all 41 suite summary
  lines** against baseline run `35625128495` changes exactly that one line, so
  nothing else moved.
- Evidence written and pushed: `ROADMAP/evidence/ci-green-input-20260921.md`
  (`0549f7e5`, extended through `65be1e49`). **Checked, not assumed:**
  `gh run view <id> --json headSha` says all four in-flight runs checked out
  `9c389baf`; every later commit is docs-only, so the code under test is the
  code that will land.
- **The new secret-broker gate is mutation-tested.** `exit 7` spliced into
  `inside.sh` before its `echo DONE`: the gate now reports `the session left
  before printing DONE (exited 7)` after `waited 0s` (against 25 s), prints
  where the transcript stopped, `state failed`, `pid`, `outcome exited 7` and
  the runtime log — and the false uid-map hint correctly did NOT print.
- **apex-input is green and pushed: `cd3c06b6` on `origin/task/ci-green-input`.**
  `112 passed, 0 failed, 0 skipped` run alone (was 101/8/0). Suite-only change;
  the shipped provisioner is untouched and was never broken.
- Mutation-tested both new invariants rather than assumed: deleting the
  `NIRI_BIN=` line -> `100 passed, 12 failed` including all three new
  assertions; moving it after its first use -> `111 passed, 1 failed`, exactly
  the ordering assertion.

## IN PROGRESS

- Working tree clean, everything pushed. The only outstanding work is reading
  the four in-flight CI runs (see NEXT) and filling the `<!-- CI-RESULTS -->`
  placeholder in the evidence file.

## COMMITS

- `cd3c06b6` test(niri) — apex-input, the real regression.
- `762c5204` test(secret) — `tests/test-secret-broker.sh` — the confined-session wait is now
    state-aware (DONE / session-left / deadline), fails at once on a session
    that has gone, raises the ceiling 25s -> 90s, and on failure prints the
    transcript byte count, the wait reason, `apex agent status`, `run.err` and
  the tail of `agentd.log`. The false uid-map hint is now conditional on
  `run.err` actually saying it. Verified green locally: `93 passed, 0 failed`.
- `9c389baf` fix(mux) — `apex-mux` `zellij_build` keeps each send's form, rc
  and stderr instead of `>/dev/null 2>&1`, prints them plus
  `zellij list-sessions` and the session's TAB NAMES before dying, and retries
  12 instead of 6. Plus the `backends` pipefail/SIGPIPE fix in the suite.
  Mutation-verified; suite alone afterwards `47 passed, 0 failed, 0 skipped`.

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
- **A THIRD flake, found locally, in `tests/test-mux-layouts.sh` line 128.**
  `"$MUX" backends | grep -qE '^(tmux|zellij)$'` under the file's own
  `set -o pipefail`: `backends` is a loop of printfs in another process,
  `grep -q` exits on the first match, the second printf takes SIGPIPE, and
  pipefail turns the match into 141. **Measured at 2 in 60** runs of that exact
  pipeline on the L16. Fixed with `grep -c` (reads to EOF, nothing to race).
- **Finding in its own right:** that gate conflates "never started" with
  "started and stalled at line N", and prints a diagnosis that was false on the
  very run it fired on. It should say where the transcript stopped.

- **THE `secret-broker` MECHANISM IS NOW NAMED, and every previous report of
  it was wrong.** Run `35647177148`, with the new gate:
  `the session left before printing DONE (killed by signal 9)`, `waited 0s`,
  `transcript is 59 bytes`, `outcome  killed by signal 9`. **The confined
  session is SIGKILLed immediately** — not a sandbox that failed to come up,
  not a sysctl, not slowness.
  The obvious follow-up hypothesis was TESTED AND REFUTED rather than written
  down as fact: `apex-agent-core/src/sandbox.rs` passes `--die-with-parent`,
  the `fork()` is on a per-connection `apex-agentd-conn` thread, and Linux
  `PR_SET_PDEATHSIG` is parent-THREAD-scoped — so a detached session ought to
  die with its client. It does not here: a probe
  (`/var/lab-scratch/ci-green-input/pdeath-probe.sh`) ran
  `apex agent run -d -- sh -c 'echo LINE1; sleep 6; echo DONE-LATE'`, watched
  `apex agent status`, and got `starting -> working -> complete`,
  `outcome exited 0`, both lines. **Needs an `apexd/` unit.**
- **THE `mux-layouts` DROP IS NOT CURED BY RETRYING, which the shipped comment
  assumed it was.** Run `35647154151`: all **twelve** sends returned `rc=0`
  with **empty stderr**, both IPC forms alternating, and
  `zellij list-sessions` showed the session alive with only
  `tab name="Tab #1"` — zellij's own scratch tab — 42 s in. So the top-level
  form is not failing on the runner, and 6 -> 12 attempts bought nothing.
  Whoever picks this up should not spend a round adding more attempts.
- **OUT OF SCOPE BUT IT WILL BLOCK THE MERGE: `Installer safety and UI` is
  red too, and a push to `roadmap/v2.2` never sees it.** Run 35647154151 on
  this branch:

  ```
  installer-locale: 25 passed, 1 failed, 0 skipped
  FAIL  …and the console keymap  — want [de] got []
  ```

  (`installer/test-installer-locale.sh`, the `set_locale_keymap_in` section:
  an explicit layout reaches the X11 keyboard config but the console keymap
  comes back empty.) It does not show on `roadmap/v2.2` pushes because the
  `changes` selector diffs against the previous push and leaves `installer`
  false — but a `pull_request` to `main` diffs against `main`, so **every job
  runs and this one fails**. It needs its own unit before A.4 can be answered.
  I did not touch it: the brief's bounds are the two suites.

## BLOCKED ON

- nothing
