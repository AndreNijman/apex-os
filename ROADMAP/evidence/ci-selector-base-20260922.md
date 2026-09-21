# ci-selector-base — the integration branch's CI answered a different question from the merge's

Round 40, 2026-09-22. Branch `task/ci-selector-base`, cut from
`roadmap/v2.2` @ `6fa9ddbc`. Repo `apex-os`.

## The defect

`pr-validation.yml`'s `changes` job picks its diff base per event, and the
three arms answered different questions:

| event | base | head |
|---|---|---|
| `pull_request` | `github.event.pull_request.base.sha` — i.e. `main` | PR head |
| `push` | `github.event.before` — the branch's previous tip | new sha |
| `workflow_dispatch` | `git merge-base origin/main "$head"` | new sha |

So a push to `roadmap/v2.2` classified only the paths in that one push, while
the pull request that merges the branch into `main` classifies the whole branch
diff. The branch can be green on every push for weeks and red the moment it is
proposed for merge.

**The argument for calling this an oversight is in the file, not in taste.**
Fifteen lines above the `case`, the step states its own rule:

> Whatever cannot be determined selects EVERYTHING … so the unknown case fails
> towards running too much, never too little.

Every other arm and both fallbacks obey that. The `push` arm was the single
place in the `case` that erred the other way. `workflow_dispatch`, ten lines
below, has always classified the way `push` now does.

## How much it was costing — measured, all 166 push runs

Every `push`-triggered run of `pr-validation.yml` on `roadmap/v2.2` was read
from the API (`/actions/runs/<id>/jobs`), 166 runs, 1,039 job records.
Raw data: `/var/lab-scratch/ci-selector-base/{push-runs.tsv,jobs.tsv}`.

How many of the four *selectable* jobs each push actually ran:

| jobs run | push runs |
|---|---|
| 0 of 4 | 28 |
| 1 of 4 | 30 |
| 2 of 4 | 96 |
| 3 of 4 | 10 |
| 4 of 4 | **2** |

Per job, over the same 166 runs:

| job | ran | skipped |
|---|---|---|
| Rust validation | 128 | 38 |
| Package engine | 106 | 59 (1 null) |
| Android client | 16 | 112 (and absent from 38 — the job postdates them) |
| **Installer safety and UI** | **9** | **157** |

The merge-shaped classification of the same tip selects **all four**: the
branch is 828 files different from `main`, and those files hit every selector —
545 the rust one, 252 the engine one, 183 the android one, 18 `installer/`.

So **164 of 166 landings were checked against a smaller job set than the merge
they are being built towards will use**, and 28 of them ran none of the four at
all — green having run only `Static validation`.

### A correction to the finding as it was handed over

The dispatch note said `Installer safety and UI` "ran on no push". That is not
what the runs say: it ran on nine (five green, four red — 35518416643,
35566068754, 35611127672, 35649643570). The corrected number is the stronger
one: the job was selected by 5% of pushes and by 100% of merge-shaped
classifications, and none of the four reds was a run against which a landing
was checked — each of them is a run that happened to touch `installer/` on its
way past.

## The fix

`push` now classifies an integration branch against the merge base with `main`,
exactly as `workflow_dispatch` does. Scope:

- **Applied to the branches named in `on.push.branches` — today that is
  `roadmap/v2.2` alone.** Those are the branches that accumulate work reaching
  `main` in one merge, so "would the merge be green" is the only question worth
  answering for them.
- **Not applied to task branches.** A push there keeps the narrow
  `event.before` range, because widening it would make a one-line push run the
  world, which is the cost the selector exists to avoid. Task branches are
  checked before they land with `gh workflow run pr-validation.yml --ref
  <branch>`, which already classifies the whole-branch way.
- The list is in the step as `INTEGRATION_BRANCHES` and the gate asserts it
  equals `on.push.branches`, so adding a branch to the trigger cannot silently
  leave it on the base that answers the wrong question.

Untouched, and asserted untouched: the all-zeros (`event.before` on a new
branch) and unreachable (force push) fallbacks, and the refusal that stops
`git diff "" ""` exiting 128 before a single test runs.

Also untouched, deliberately: the `result:` aggregator's skip semantics. A
skipped job counting as success is correct there and documented there.

`run:` block length, against the 21,000-character cap that surfaces as a
jobless failed run: **9,160 → 10,959**.

## The gate, red before and green after

`tests/check-ci-selector-parity.sh`. It extracts the `Classify changed paths`
step's own `run:` block out of the YAML and *executes* it against synthetic
repositories, so it cannot drift from the code it is about. It takes the
workflow path as an argument so the same commit can be pointed at an older copy.

Fixture: fork from `main`, commit 1 touches `installer/` only, commit 2 touches
`docs/` only. Those light two *different* single selectors (installer, rust),
so "selected too little" and "selected everything" are distinguishable — a
fixture touching `files/` or `tests/` would light rust and engine together and
lose that. `main` never moves after the fork, because the `pull_request` arm
takes a two-dot diff.

It lives in `static`, the job with no path filter. `.github/workflows/` selects
the **engine** job, so a gate about the selector placed there could be switched
off by the selector — which is this repository's dominant defect family.

### Against the previous revision of the workflow — RED

```
PASS: the merge PR selects exactly installer and rust — rc=0 jobs=[installer,rust]
FAIL: a push to the integration branch selects what the merge PR selects — want rc=0 jobs=[installer,rust] got rc=0 jobs=[rust]
PASS: a push to a TASK branch still classifies narrowly — rc=0 jobs=[rust]
PASS: an all-zeros before (new branch) falls back to the merge base — rc=0 jobs=[installer,rust]
PASS: an unreachable before (force push) falls back to the merge base — rc=0 jobs=[installer,rust]
PASS: no usable range selects every suite and does not exit 128 — rc=0 jobs=[android,engine,installer,rust]
FAIL: on.push.branches equals the step INTEGRATION_BRANCHES list
      | on.push.branches:     roadmap/v2.2
      | INTEGRATION_BRANCHES:

5 passed, 2 failed        (exit 1)
```

### Against this branch — GREEN

```
7 passed, 0 failed        (exit 0)
```

### And on a GitHub runner — GREEN

Run **35651168049**, `pr-validation.yml` dispatched on `task/ci-selector-base`.
`Static validation` green, and the new step reports the same seven:

```
##[group]Run ./tests/check-ci-selector-parity.sh
PASS: the merge PR selects exactly installer and rust — rc=0 jobs=[installer,rust]
PASS: a push to the integration branch selects what the merge PR selects — rc=0 jobs=[installer,rust]
PASS: a push to a TASK branch still classifies narrowly — rc=0 jobs=[rust]
PASS: an all-zeros before (new branch) falls back to the merge base — rc=0 jobs=[installer,rust]
PASS: an unreachable before (force push) falls back to the merge base — rc=0 jobs=[installer,rust]
PASS: no usable range selects every suite and does not exit 128 — rc=0 jobs=[android,engine,installer,rust]
PASS: on.push.branches equals the step's INTEGRATION_BRANCHES (roadmap/v2.2 )

7 passed, 0 failed
```

That matters for more than a tick: the gate builds git repositories and parses
YAML with nothing but python3's standard library, deliberately, because the
runner image is not contracted to carry PyYAML and a gate that cannot run is
worth nothing.

The five assertions that pass in both directions are the point of the second
list: they are the evidence that the fallbacks were not regressed while the
base was changed.

### Three mutants, each failing only its own assertion

Built in `/var/lab-scratch/ci-selector-base/mut-*.yml` from the fixed file.

| mutant | result |
|---|---|
| `integration=true` for every branch (merge-base everywhere) | 6/1 — fails *a push to a TASK branch still classifies narrowly*: got `[installer,rust]` |
| integration base set to `""` (select everything) | 6/1 — fails *parity*: got `[android,engine,installer,rust]` |
| all-zeros/unreachable fallback deleted | 5/2 — fails both fallback assertions with **`rc=128`**, reproducing exactly the failure the step's own comment describes |

## Step 4 — the full matrix on `roadmap/v2.2`, and what it surfaces

`gh workflow run pr-validation.yml --ref roadmap/v2.2` → run **35650126791**,
on `6fa9ddbc`. `Select tests` logged
`classified workflow_dispatch over 57f593ad..6fa9ddbc` and all four selectable
jobs were dispatched, which is what makes this a full matrix rather than a
claim about the event.

| job | result |
|---|---|
| Static validation | green (3m46s) |
| Select tests | green |
| Rust validation | green (3m34s) |
| Android client | green (3m40s) |
| **Installer safety and UI** | **RED** — one step, three causes |
| **Package engine** | **RED** (15m15s) — one step of 30, the known `mux-layouts` flake |

**None of these are fixed here. Each is its own unit; a round that fixes five
unrelated reds cannot say which fix did what.**

### RED 1 — `installer/test-installer-luks.sh`, 24 passed / 7 failed

Three distinct causes inside one suite, and **not** the locale/keymap red that
`6fa9ddbc` fixed — `installer-locale` reported 26/0 and `installer-keymap` 43/0
in this same run.

1. `FAIL  XKB bg resolves to console keymap bg_bds-utf8 (end to end) KEYMAP=us`
2. The suite's own fallback for a machine with no Fedora keymap data is to
   measure inside a container, and on the GitHub runner that container cannot
   start:
   `cannot open run directory '/run/user/1001/crun': Permission denied` →
   `Error: crun: … OCI permission denied` → `FAIL  the keymap checks reported a
   result — no KEYMAP-CHECKS line — they did not finish`. Same family as the
   recorded "CI runner is a second environment" defects.
3. Six `--check-passphrase` assertions fail identically with
   `APEX-INSTALL-FAILED: The APEX-OS image (localhost/apex-os:daily) is not
   present in the live environment.` — the suite drives the real installer,
   which refuses early when the image is absent. **Its mutant control fails the
   same way** (`mutant: the case arm removed … something still answers
   --check-passphrase with the arm gone`), so the suite currently cannot tell a
   broken guard from a missing image. That is the more serious half: it is a
   control that no longer controls anything.

**This suite has never once passed in CI, and that is measurable rather than
inferred.** `installer/test-installer-luks.sh` and the workflow step that runs
it landed 2026-09-20 (`92bdf557`, extended by `05b0ea55` on 09-21). Every
`Installer safety and UI` job since:

| run | date | `Run installer disk-encryption suite` |
|---|---|---|
| 35518416643 | 09-20 15:04 | skipped — `installer-locale` above it was red |
| 35566068754 | 09-21 05:50 | skipped — same |
| 35611127672 | 09-21 14:17 | skipped — same |
| 35649643570 | 09-21 20:12 | **failure** — the first run to reach it |
| 35650126791 | 09-21 20:17 | **failure** |

All three skipped runs died on `FAIL …and the console keymap — want [de] got []`,
the locale defect `6fa9ddbc` fixed. So the luks red is not something the tip
introduced: it is a red that has been hiding behind another red since the suite
landed, and `6fa9ddbc` is what uncovered it. The five *green* installer runs
(2026-09-12/13) predate the step entirely — it is not in their step list.

### RED 2 — the accessibility audit's state is UNKNOWN, not green

`Run installer accessibility audit` reported `-` in that job, because the
disk-encryption step above it failed and a failed step skips every step below
it. It last reported for itself on 2026-09-13 (run 34727364060, green), before
either the keyboard or the disk-encryption step existed above it; it has been
skipped in every installer job since. The `installer` job's steps carry no `if: ${{ !cancelled() }}`, unlike the
`engine` job's gates, which were given it after run 34714159369 for precisely
this. So the installer job's later steps are switched off by any earlier red.
Small, mechanical, and its own unit.

### RED 3 — `Package engine`: `mux-layouts`, and nothing else

15m15s, one failing step of 30: `Run terminal layout template assertions`.

```
FAIL  build creates the zellij session
FAIL  the layout landed as a tab zellij can describe
FAIL  the layout landed exactly once (0 apex tabs)
mux-layouts: 44 passed, 3 failed, 0 skipped
```

That is the flake `ci-green-input` characterised on 2026-09-21
(`ROADMAP/evidence/ci-green-input-20260921.md`): all twelve sends return rc=0
with empty stderr, so retrying is not the cure, and 6 → 12 attempts bought
nothing. Its own unit; not touched here.

Two things in the same job are worth recording because they are *not* red:

- `secret-broker: 93 passed, 0 failed`. The `killed by signal 9` flake did not
  fire this time. One green run is not a fix — at the rate that unit measured,
  green is the more likely outcome of any single run.
- `apex-plugin: 117 passed, 0 failed`. **The comment on that step
  ("EXPECT THIS RED UNTIL apex-shell PR #9 MERGES", the last step in the job
  and placed last for that reason) is stale: apex-shell PR #9 merged
  2026-09-03.** The step is green and has no reason to be treated as expected
  red any more. Out of this unit's bounds; named for whoever takes it.

## `build-image.yml` — the same shape, and the decision NOT to change it

Required by the unit's bounds, because a mis-scoped selector there costs the
fleet a ~5 GB download per machine.

**Decision: leave it alone.** The asymmetry cannot occur there.

- Its push trigger is `branches: [main]`, not an integration branch. On a push
  to `main` the push *is* the merge, so `event.before` and a merge base with
  `main` answer the same question. There is nothing to reconcile.
- It classifies with `dorny/paths-filter@v3`, not the hand-rolled `case`, and
  checks out with `fetch-depth: 2` — it could not compute a merge base without
  a deeper fetch, which is itself a cost on every run.
- What a wrong scope would cost is on the record in that file: `build-image.yml`
  was once in the `core` filter, and the next CI-only commit triggered a
  55-minute core rebuild and reissued the whole image to every machine on the
  fleet for a change that could not alter core's content by a byte. Widening
  that filter to "the whole diff against `main`" would make every push to `main`
  rebuild core.

If `build-image.yml` ever gains an integration branch in its push trigger, this
reasoning stops applying and the same parity question has to be asked again —
with `force_core` as the escape hatch rather than a widened filter.

## Noted, not fixed

- **The `pull_request` arm uses a two-dot `git diff base.sha head`, not a merge
  base.** If `main` moves after the PR opens, the PR *over*-runs: main's own new
  paths are classified as changes, inverted. Consistent with the file's "too
  much, never too little" rule, so it is left alone.
- If a branch in `on.push.branches` were ever `main` itself, the merge base
  would equal the head, the diff would be empty, every selector false, every job
  skipped and `result` green. The new list assertion forces that into the open
  rather than preventing it.
- **The `push` arm's own runner-side proof is still owed, and here is what it
  looks like.** Everything below was measured on `workflow_dispatch` runs,
  which exercise the `*)` arm — the same `git merge-base origin/main` call
  under the same `actions/checkout@v4` `fetch-depth: 0`, so this is sound
  inference, not measurement of the changed line. The first push to
  `roadmap/v2.2` after this lands must log
  `::notice::classified push over 57f593ad..<new sha>` in `Select tests` and
  dispatch all four jobs. That is the thing to look for.
- `git merge-base origin/main "$head"` does resolve on the runner —
  run 35647188306 logged `classified workflow_dispatch over 57f593ad..9c389baf`
  and `57f593ad` is the tip of `main`, so `actions/checkout@v4` with
  `fetch-depth: 0` leaves `refs/remotes/origin/main` present. Checked rather
  than assumed, because the whole fix depends on it.

## Local gates, after the change

```
suite coverage: 107 suites, 100 run by CI, 7 exempt, 0 unrun and undeclared
shellcheck coverage: 206 scripts discovered, 0 known-failing, 0 newly failing, 0 now clean
PASS  no merge conflict markers in the tree (searched with git)
```
