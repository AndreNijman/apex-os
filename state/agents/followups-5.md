# followups-5
items: (no roadmap ids — CI plumbing: red steps gating 77 others)
worktree: /var/tmp/apex-work/wt-followups-5
branch: task/followups-5
repo: apex-os

## NEXT
Fix `files/scripts/check-containerfile-order`: a path inside a grep PATTERN is
not a read. Then read run 34728030705 (dispatched on the structural commit) for
the downstream picture.

## DONE
- `13e7ec84` ci: 93 `run:` steps in static/rust/engine now carry
  `if: ${{ !cancelled() }}`. PUSHED. Run 34728030705 dispatched on it.

## IN PROGRESS
- the three red steps (see FOUND for the diagnosis of each).

## FOUND
- P3 labwc thread CLOSED: run 34717723637 reports
  `P3 labwc — the shipped session, probed headless by real clients: success`.
- **77 steps, not 45.** Static loses 20 as well as engine's 35 and rust's 22.
- **rust's red step is no longer `§26 channels` — it is `Tests`, step 8**, which
  is 16 steps HIGHER, so §26 did not even run on 34727364060.

## BLOCKED ON
- nothing
