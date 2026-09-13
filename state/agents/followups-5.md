# followups-5
items: (no roadmap ids — CI plumbing: three pre-existing red steps gating ~45 others)
repo: apex-os
worktree: /var/tmp/apex-work/wt-followups-5
branch: task/followups-5

## NEXT
Read the failing output of the three red steps on the last dispatch run
(`gh run view <id> --log-failed`) — Static "Validate Containerfile layer order",
engine "Run virtualization assertions", rust "§26 channels" — and classify each
as product defect / gate defect / runner-vs-L16 environment difference BEFORE
changing anything.

## DONE
- (nothing yet)

## IN PROGRESS
- (nothing yet)

## FOUND
- P3 labwc thread CLOSED by the orchestrator: dispatch run 34717723637 reports
  `P3 labwc — the shipped session, probed headless by real clients: success`.
  followups-4's one outstanding question is answered; the labwc matrix is green.

## BLOCKED ON
- nothing
