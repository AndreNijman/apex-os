# ci-green-input — apex-input is really broken; mux-layouts only fails on the runner

items: none (no roadmap id — CI red on the integration branch, found by the orchestrator)
repo: apex-os
worktree: /var/tmp/apex-work/wt-ci-green-input (create it)
branch: task/ci-green-input, cut from origin/roadmap/v2.2
lab: /var/lab-scratch/ci-green-input

Dispatched round 39, 2026-09-21 ~18:20 AWST, by the orchestrator.

## TWO DEFECTS, AND THE DIFFERENCE BETWEEN THEM IS THE POINT

`roadmap/v2.2`'s `Package engine` job has failed on every recent merge
(runs 35584358228, 35585290517, 35586055011). Two suites are red in CI. The
orchestrator ran both locally on the merged tip, **separately**, because this
repo's suites are known to interfere in a sequential loop:

| suite | in CI | run alone locally |
| --- | --- | --- |
| `tests/test-apex-input.sh` | 93 passed, **7 failed**, 8 skipped | 101 passed, **8 failed**, 0 skipped |
| `tests/test-mux-layouts.sh` | 44 passed, **3 failed** | **47 passed, 0 failed** |

So they are NOT the same kind of problem, and must not be fixed as one:

- **`apex-input` is a real regression.** It fails on this machine too. Eight
  assertions, and they read like one cause rather than eight:
  ```
  FAIL  the generated input config is included
  FAIL  the generated keybind config is included
  FAIL  exactly six lines were added (635 -> 635)
  FAIL  a backup was taken before the edit
  FAIL  re-running adds nothing (its own marker, not the autostart block's)
  FAIL  it refuses and says so
  FAIL  it restores and says so
  FAIL  a broken include is rejected — proving config.kdl really reads it
  ```
  `(635 -> 635)` says the edit added NOTHING — the include block is never
  written, and every later assertion is downstream of that. Find the one cause
  before touching eight assertions. Note the CI run shows `635 -> 635` as
  `8 -> 8`, i.e. the same "added nothing" against a different starting file.

- **`mux-layouts` is a runner-environment defect.** It passes 47/0 here and
  fails 3 in CI, all three about creating a zellij session
  (`build creates the zellij session`, `the layout landed as a tab zellij can
  describe`, `the layout landed exactly once (0 apex tabs)`). This repo already
  has a memory for this shape: **the CI runner is a second environment**, and
  openssl/nft/localtime/cgroup defects have been found that exist only off the
  L16. A test that silently needs a TTY, a session bus, or a writable
  `XDG_RUNTIME_DIR` is the usual cause.
  **Do not fix this by skipping it in CI unless you can say exactly what the
  runner lacks and why the thing it covers is still covered.** A suite that
  skips its own subject is the "gate that inspects nothing" family, which is
  the dominant CI defect family in this repo.

## WHAT DONE LOOKS LIKE

1. `apex-input`: the ONE cause named, fixed, and the suite green locally — and
   say whether the regression is in the shipped script or in the suite. If the
   suite is wrong, that is a finding, not a licence to weaken it.
2. `mux-layouts`: what the runner lacks, stated concretely, with the evidence
   you used to establish it. Then either make the test work there or make it a
   declared, reasoned exception — and if it becomes an exception, say what now
   covers the behaviour it was testing.
3. Both suites green in CI on your branch. Use `gh run` to see it rather than
   predicting it.
4. Evidence file `ROADMAP/evidence/ci-green-input-20260921.md`.

## BOUNDS

- The two suites and whatever they turn out to be testing. Nothing else.
- Run each suite ALONE before believing any result about it.

## NEXT

- Run `tests/test-apex-input.sh` alone and find the single cause behind
  `exactly six lines were added (635 -> 635)`.

## DONE

## IN PROGRESS

## FOUND

## BLOCKED ON

- nothing
