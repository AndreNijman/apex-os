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

---

## ROUND 40 CONTINUATION — written by the autoresume orchestrator, 2026-09-22 03:30 AWST

**The round-39 agent produced nothing.** There is no `task/ci-green-input`
branch on origin and no worktree. Everything above is still the dispatch brief,
unconsumed — treat it as if it were written for you, because it was.

**The reds are still live, checked rather than assumed.** PR validation run
`35625128495` on `roadmap/v2.2` ("Merge fix/pkg-flatpak-only-route", 13m54s)
failed with exactly:

```
Package engine >> Run input-settings assertions
Package engine >> Run secret-broker assertions
PR validation  >> Require every applicable test
```

Note that is **not** the same pair the brief above names. The brief names
`test-apex-input.sh` and `test-mux-layouts.sh`; CI now names the
**input-settings** and **secret-broker** assertion steps. Find out whether
`Run secret-broker assertions` is a third defect or the same `apex-input` cause
wearing a different step name — `gh run view 35625128495 --log-failed` answers
it and costs one command. **Do not assume the brief's table is current.**

The four `PR validation` runs that succeeded after this one (`35636984370` and
friends) are **docs-only commits** — 3-4 minutes against 13m54s, i.e. the
selector skipped the jobs. They are not evidence that the reds went away.

### Why this is the highest-priority non-hardware unit on the board

The full image now builds green (run 35624291221) and katana is booted on it.
Per `ROADMAP/ANDRE-TODO.md` item A.4, the only thing left before Andre approves
the merge to `main` is qualification — and merging an integration branch whose
PR validation is red is exactly what this program has spent 40 rounds not doing.

### Two facts from this repo's memory, handed over so you do not pay for them

1. **The suites interfere in a sequential loop.** `apex-os` suites fail
   spuriously when run together — re-run any suite ALONE before believing a
   regression. The table above was built that way and it is why `mux-layouts`
   was correctly classified as environmental.
2. **The CI runner is a second environment.** openssl/nft/localtime/cgroup
   defects have been found that exist ONLY off the L16. "It passes here" proves
   nothing about the runner. Reproduce in a runner-matching container.

And the standing rule this repo cares most about: **a gate that runs and
inspects nothing is the dominant CI defect family here.** If any of this ends in
a skip, the skip must name what the runner lacks and what still covers the
behaviour.

## NEXT (round 40)

- `gh run view 35625128495 --log-failed` and find out whether `Run secret-broker
  assertions` is a third defect or the `apex-input` cause under another name.
