# integrate-3
items: (integration only) chore/run-clippy, task/p1-045-updates-trust,
       task/p1-002-cloudflare (apex-os); fix/locked-hint (apex-shell)
repo: both
worktree: /var/tmp/apex-work/int-os and /var/tmp/apex-work/int-shell
branch: roadmap/v2.2

## NEXT
Orienting. Nothing edited yet. Next action: verify branch tips + baseline
test count on int-os.

## PLAN
1. apex-os chore/run-clippy (1 commit) -> roadmap/v2.2 e4e221f
2. apex-os task/p1-045-updates-trust (6 commits, 5eb7f1a)
3. apex-os task/p1-002-cloudflare (6 commits, 2514e09) MINUS 8110944
4. apex-shell fix/locked-hint (1 commit, 87c84f3) -> roadmap/v2.2 0fd12ee

## FACTS FOUND SO FAR
- int-os roadmap/v2.2 = e4e221f CONFIRMED. int-shell roadmap/v2.2 = 0fd12ee CONFIRMED.
- **8110944 ALREADY LANDED** as bb5b355 by integrate-2 (out-of-order landing,
  with a one-token `Vec::new()` adaptation for P0-003's third arg on
  use_capability). So during the p1-002 rebase it must be DROPPED, not
  resolved. integrate-2's card documents this explicitly.
- Known pre-existing flake: apex-secretd
  `tests::a_stale_socket_from_a_dead_daemon_is_replaced`.
