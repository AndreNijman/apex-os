# ci-selector-base — the integration branch's CI answers a different question from the merge's

items: none (CI correctness; gates the merge to main)
repo: apex-os
worktree: /var/tmp/apex-work/wt-ci-selector-base (create it)
branch: task/ci-selector-base, cut from origin/roadmap/v2.2
lab: /var/lab-scratch/ci-selector-base

Dispatched round 40, 2026-09-22 ~05:45 AWST, by the autoresume orchestrator.
Found by `ci-green-input`; the keymap half was fixed and landed by the peer
session as `6fa9ddbc` and is NOT your problem.

## READ THIS FIRST, BECAUSE THE OBVIOUS FIX IS WRONG

**"A skipped job counts as success" is DELIBERATE, DOCUMENTED AND CORRECT.**
`pr-validation.yml`'s `result:` job says so in a comment written for exactly the
reader you are about to be:

> `skipped` counts as success HERE and only here … a documentation-only PR
> legitimately does not run them and must not be blocked by them. `changes`
> itself is in `needs`, so a FAILURE there is caught rather than cascading into
> skips that pass, and `cancelled` fails the `all` too.

Do **not** "fix" this by making skips fail. That comment also names the real
adjacent hazard and puts the remedy in the right place — a SUITE that prints
"0 passed, 0 failed (skipped)" and exits 0 is invisible to any aggregate gate,
so a missing prerequisite must be a failure in the suite, never a skip.

The orchestrator's earlier one-line framing of this finding ("a skipped job
counts as success") was imprecise and is corrected here.

## THE ACTUAL DEFECT — a base-ref asymmetry, measured in the file

`changes` picks its diff base differently per event (`pr-validation.yml`, the
`changes` job):

| event | base | head |
|---|---|---|
| `pull_request` | the PR base sha — i.e. **`main`** | PR head |
| `push` | **`github.event.before`** — the branch's previous tip | new sha |
| `workflow_dispatch` | **`git merge-base origin/main "$head"`** | new sha |

So a push to `roadmap/v2.2` classifies **only the paths in that one push**,
while the pull request that merges the branch into `main` classifies **the whole
branch diff**. The integration branch can therefore be green on every push for
weeks and red the moment it is proposed for merge — which is exactly what
happened: `Installer safety and UI` was red 4 of 4 runs and no push ever ran it,
because no single push happened to touch `installer/`.

**`workflow_dispatch` already does the right thing in this very file.** The fix
has a precedent ten lines away, which is the strongest argument that the `push`
branch is an oversight rather than a decision.

## WHAT DONE LOOKS LIKE

1. **On a push to an INTEGRATION branch, classify against the merge base with
   `main`**, the way `workflow_dispatch` already does. Scope it deliberately:
   doing this for every push on every branch would make a one-line task-branch
   push run the world, which is the cost the selector exists to avoid. Say which
   branches you applied it to and why.
2. **Keep the existing fallbacks working.** That block already handles a missing
   or all-zero `$PUSH_BEFORE` (a new branch, a force push) by falling back to the
   merge base, and it refuses rather than running `git diff "" ""` and exiting
   128 before a single test ran. Do not regress either. Read the comment at the
   top of that step before editing it.
3. **A gate, proved BOTH ways.** The property: for the integration branch, the
   set of jobs a push runs must equal the set the merge PR would run. Show it
   red against today's workflow and green after. A gate that cannot be shown red
   is this repo's dominant defect family.
4. **Re-run the full matrix on `roadmap/v2.2` once** and report what it finds.
   The whole point is that nobody currently knows what else has been hiding —
   `installer` is one job of five, and the same reasoning applies to `rust`,
   `engine` and `android`. **Expect to find more, and report them rather than
   fixing them** — each is its own unit.
5. `ROADMAP/evidence/ci-selector-base-20260922.md`.

## BOUNDS

- `pr-validation.yml`'s `changes` job, the new gate, its wiring, the evidence.
- Do NOT touch the `result:` aggregator's skip semantics. See above.
- Do NOT fix whatever reds the full-matrix run surfaces. Name them; they are
  separate units. A round that fixes five unrelated reds at once cannot say
  which fix did what.
- `build-image.yml` has the same `changes` shape and a `core` job whose rebuild
  costs the fleet ~5 GB. If you decide it needs the same change, say what a
  mis-scoped selector would cost there before making it — do not assume
  symmetry with `pr-validation.yml`.

## CONSTRAINTS I AM UNDER

- Never push `main`, never open a PR, never land on `roadmap/v2.2`. Push your
  branch and mark `## LANDABLE <sha>` on this card.
- **A workflow `run:` block is capped at 21,000 characters** and exceeding it
  surfaces as a JOBLESS FAILED RUN, not a failing step. This repo has paid for
  that once.
- `gh run` is the route; the `plugin:github` MCP does not connect (malformed PAT).
- Headless only. `/var/lab-scratch`, never `/tmp` (15 GB tmpfs on 29 GB RAM).
- Long jobs under `systemd-run --user`, never `nohup &`.

## NEXT

- ONLY thing outstanding: `Package engine` on run **35650126791** (step 4's
  last job, ~15 min). `gh run view 35650126791` from
  /var/tmp/apex-work/wt-ci-selector-base. Write its outcome into the
  "Package engine" section of
  `ROADMAP/evidence/ci-selector-base-20260922.md` (replace the
  "still running when this file was written" paragraph — it must not point at
  this card, which is not in git), commit, push, and mark `## LANDABLE <sha>`.
  Everything else is done and pushed.

## DONE

- **Fix + gate committed and pushed: `task/ci-selector-base` @ `79ce1dc9`.**
  `pr-validation.yml`'s `push` arm now classifies an integration branch against
  `git merge-base origin/main "$head"`; task branches keep `event.before`.
  New gate `tests/check-ci-selector-parity.sh`, wired into `static`.
- Gate proved BOTH ways: 5 passed / 2 failed against `HEAD:` (pre-fix copy at
  /var/lab-scratch/ci-selector-base/pr-validation.BEFORE.yml), 7/0 after.
  Three mutants each fail exactly their own assertion —
  mut-all-branches (task narrowing), mut-select-all (parity),
  mut-no-zero-fallback (both fallbacks, rc=128).
- Local gates green: check-suites-run-in-ci (107 suites, 100 in CI, 0 unrun),
  check-shellcheck-coverage (206 scripts, 0 newly failing), no-conflict-markers.
- Step 4 full-matrix run dispatched and read: 35650126791 (all four jobs
  selected; classified over 57f593ad..6fa9ddbc).
- `ROADMAP/evidence/ci-selector-base-20260922.md` written, committed, pushed.
- `build-image.yml` decision written into the evidence: do NOT change it — its
  push trigger is `main` only, so the push IS the merge; it uses
  dorny/paths-filter with fetch-depth 2; and the file records that having
  build-image.yml in the `core` filter once cost a 55-minute rebuild and a
  fleet-wide ~5 GB reissue for a CI-only commit.
- **Gate green on a GitHub runner**: run 35651168049, `Static validation`
  success, step "A push to the integration branch selects what the merge would"
  reports 7 passed / 0 failed. The gate uses python3 stdlib only (no PyYAML)
  precisely so it can run there.
- **Pushed: `task/ci-selector-base` @ `b427e2f0`** (3 commits: fix+gate,
  evidence, evidence update).
- Run block measured 9,160 -> 10,959 chars against the 21,000 cap.

## IN PROGRESS

- One CI job outstanding: `Package engine` on 35650126791.

## FOUND

- **The card's premise is slightly wrong and the corrected numbers are
  stronger.** `Installer safety and UI` did NOT run on zero pushes: over all
  166 push runs of pr-validation on `roadmap/v2.2` it was selected by 9
  (5 green, 4 red — 35518416643, 35566068754, 35611127672, 35649643570).
  The real measure: only **2 of 166** push runs classified all four selectable
  jobs, **28 classified none of them** (green having run only `Static
  validation`), and the merge-shaped classification of the tip selects all
  four (828 files differ from `main`). Data:
  /var/lab-scratch/ci-selector-base/{push-runs.tsv,jobs.tsv}.
- **STEP 4, run 35650126791, `workflow_dispatch` on roadmap/v2.2 @ 6fa9ddbc,
  classified over 57f593ad..6fa9ddbc, all four jobs selected:**
  - Static validation ✓, Select tests ✓, Rust validation ✓, Android client ✓
  - **Installer safety and UI ✗ — `Run installer disk-encryption suite`,
    exit 1. NEW: this is not the locale/keymap red that 6fa9ddbc fixed
    (locale ✓ and keyboard ✓ in this very run). Its own unit.**
  - `Run installer accessibility audit` reported `-` because the failing step
    above it skipped it — that step has no `if: !cancelled()`, unlike the
    engine job's gates. So the a11y audit's state on this tree is UNKNOWN,
    which is its own (small) unit.
  - Package engine: see NEXT — still running.
- **RED 1 is dated: `installer/test-installer-luks.sh` has NEVER passed in
  CI.** It and its workflow step landed 2026-09-20 (`92bdf557`, extended by
  `05b0ea55`). `Run installer disk-encryption suite` was SKIPPED in runs
  35518416643 / 35566068754 / 35611127672 (the locale suite above it was red)
  and FAILED in 35649643570 and 35650126791, the first two to reach it. So
  `6fa9ddbc` did not break it — it uncovered it. The five green installer runs
  of 2026-09-12/13 predate the step entirely.
- **A stale comment in `pr-validation.yml`, out of my bounds:** the `engine`
  job's last step says "EXPECT THIS RED UNTIL apex-shell PR #9 MERGES".
  apex-shell PR #9 merged **2026-09-03**. Somebody should re-read that step's
  premise; I did not touch it.
- `git merge-base origin/main "$head"` DOES resolve on the runner:
  run 35647188306 logged `classified workflow_dispatch over 57f593ad..9c389baf`
  and 57f593ad is the tip of `main`. actions/checkout@v4 with fetch-depth: 0
  leaves refs/remotes/origin/main present. Measured, not assumed.
- **The `pull_request` arm uses a two-dot `git diff base.sha head`, not a
  merge base.** When `main` moves after the PR opens, the PR over-runs — it
  classifies main's own new paths as changes, inverted. Consistent with the
  file's "too much, never too little" rule, so left alone. Noted, not fixed.
- If a branch in `on.push.branches` were ever `main` itself, merge-base would
  equal head, the diff would be empty, every selector false, every job skipped
  and `result` green. The new list assertion forces that decision into the
  open rather than preventing it. Noted, not guarded.

## BLOCKED ON

- nothing
