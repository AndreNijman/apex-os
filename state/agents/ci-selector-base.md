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

## LANDABLE addd0b85

`task/ci-selector-base` @ `addd0b85` — 6 commits, pushed to origin.
Three files: `.github/workflows/pr-validation.yml`,
`tests/check-ci-selector-parity.sh`,
`ROADMAP/evidence/ci-selector-base-20260922.md`.
Merges clean, measured not assumed: at 2026-09-21 20:55Z `git fetch origin`
left `origin/roadmap/v2.2` still at `6fa9ddbc` (the commit this branch was cut
from) and `git merge-tree --write-tree origin/roadmap/v2.2 HEAD` returned 0
with tree `31e170e0`. Re-check if the branch has moved since.

**The one thing to look at on the first push after landing:** `Select tests`
must log `::notice::classified push over 57f593ad..<new sha>` and dispatch all
four jobs. Every measurement behind this fix was taken on `workflow_dispatch`
runs, which use the same `git merge-base origin/main` call under the same
checkout config — sound inference, but the changed `push` line itself has not
run on a runner. Expect the branch's landings to get slower and louder: they
will all run the full matrix from now on, which is the point.

## NEXT

- Nothing. Unit complete. If anything else is wanted here it is the follow-up
  units named under FOUND, and each is its own card.

## DONE

- **Fix + gate: `pr-validation.yml`'s `push` arm classifies an integration
  branch against `git merge-base origin/main "$head"`; task branches keep
  `event.before` and both of its fallbacks.** `INTEGRATION_BRANCHES` in the
  step, asserted equal to `on.push.branches`.
- **`tests/check-ci-selector-parity.sh`**, wired into `static` — the job with
  no selector, because `.github/workflows/` selects the ENGINE job and a gate
  about the selector must not be one the selector can switch off. It extracts
  the step's own `run:` block and executes it, so it cannot drift.
- Gate proved BOTH ways: **5 passed / 2 failed** against the pre-fix workflow
  (/var/lab-scratch/ci-selector-base/pr-validation.BEFORE.yml), **7 / 0** after,
  and **7 / 0 on a GitHub runner** (run 35651168049, `Static validation`).
  The five that pass in both directions are the evidence the fallbacks were not
  regressed.
- Three mutants, each failing only its own assertion
  (/var/lab-scratch/ci-selector-base/mut-*.yml): merge-base for every branch →
  task narrowing fails; base set to "" → parity fails; all-zeros fallback
  deleted → both fallback assertions fail with **rc=128**, the exact failure
  the step's own comment describes.
- Step 4 done: full matrix run **35650126791**, all four jobs selected,
  classified over 57f593ad..6fa9ddbc. Reds reported, none fixed.
- `build-image.yml`: decided NOT to change, with the cost stated. Its push
  trigger is `main` only, so there the push IS the merge; it classifies with
  dorny/paths-filter at `fetch-depth: 2`; and its own comment records that
  having `build-image.yml` in the `core` filter once cost a 55-minute rebuild
  and a fleet-wide ~5 GB reissue for a CI-only commit.
- `ROADMAP/evidence/ci-selector-base-20260922.md` written.
- Local gates green after the change: check-suites-run-in-ci (107 suites, 100
  in CI, 0 unrun), check-shellcheck-coverage (206 scripts, 0 newly failing),
  check-no-conflict-markers.
- `run:` block measured 9,160 -> 10,959 chars against the 21,000 cap.

## IN PROGRESS

- nothing

## FOUND

Each of the four below is its own unit. None was fixed here.

- **The card's premise was slightly wrong, and the corrected numbers are
  stronger.** `Installer safety and UI` did not run on zero pushes: over all
  **166** push runs of pr-validation on `roadmap/v2.2` it was selected by 9
  (5 green, 4 red). The real measure is the distribution — only **2 of 166**
  push runs classified all four selectable jobs, **28 classified none** (green
  having run only `Static validation`), and the merge-shaped classification of
  the tip selects all four (828 files differ from `main`). So 164 of 166
  landings were checked against a smaller job set than the merge will use.
  Data: /var/lab-scratch/ci-selector-base/{push-runs.tsv,jobs.tsv}.
- **UNIT: `installer/test-installer-luks.sh` is red 24/7, and has NEVER passed
  in CI.** Three separate causes: (1) `FAIL XKB bg resolves to console keymap
  bg_bds-utf8 (end to end) KEYMAP=us`; (2) the no-keymap-data fallback measures
  inside a container and the runner refuses it —
  `cannot open run directory '/run/user/1001/crun': Permission denied` → OCI
  permission denied → `FAIL the keymap checks reported a result`; (3) six
  `--check-passphrase` assertions fail identically with
  `APEX-INSTALL-FAILED: … localhost/apex-os:daily is not present`, **including
  the mutant control**, so the suite cannot currently tell a broken guard from
  a missing image. Dated: the suite and its step landed 2026-09-20
  (`92bdf557`/`05b0ea55`); the step was SKIPPED in runs 35518416643,
  35566068754 and 35611127672 because the locale suite above it was red, and
  FAILED in 35649643570 and 35650126791, the first two to reach it.
  `6fa9ddbc` uncovered this red, it did not cause it.
- **UNIT: the `installer` job's steps have no `if: ${{ !cancelled() }}`.**
  `Run installer accessibility audit` reported `-` in both recent runs because
  a step above it failed. The `engine` job was given that guard after run
  34714159369 for exactly this; the installer job never was. The a11y audit
  last reported for itself on 2026-09-13.
- **UNIT: `mux-layouts` — the known flake, 44/3, still the only red in
  `Package engine`** (15m15s, one failing step of 30). `secret-broker` was
  93/0 this run, which is not a fix.
- **UNIT (tiny): a stale comment.** `pr-validation.yml`'s last engine step says
  "EXPECT THIS RED UNTIL apex-shell PR #9 MERGES". PR #9 merged **2026-09-03**
  and `apex-plugin` reports 117 passed / 0 failed. The step is placed last
  *because* it was expected red; that premise is gone.
- **The `pull_request` arm uses a two-dot `git diff base.sha head`, not a merge
  base.** If `main` moves after the PR opens, the PR over-runs — main's own new
  paths are classified as changes, inverted. Consistent with the file's "too
  much, never too little" rule, so left alone. Noted, not fixed.
- If a branch in `on.push.branches` were ever `main` itself, the merge base
  would equal head, the diff would be empty, every selector false, every job
  skipped and `result` green. The new list assertion forces that decision into
  the open rather than preventing it.
- `git merge-base origin/main "$head"` DOES resolve on the runner:
  run 35647188306 logged `classified workflow_dispatch over 57f593ad..9c389baf`
  and 57f593ad is the tip of `main`, so `actions/checkout@v4` with
  `fetch-depth: 0` leaves `refs/remotes/origin/main` present. Measured, because
  the whole fix depends on it.

## BLOCKED ON

- nothing
