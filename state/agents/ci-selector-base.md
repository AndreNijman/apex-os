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

- Read the `changes` job's base-selection step in full, then demonstrate the
  asymmetry: for the current tip, compute the job set for `push` and the job set
  for a `pull_request` to `main` and show they differ.

## DONE

## IN PROGRESS

## FOUND

## BLOCKED ON

- nothing
