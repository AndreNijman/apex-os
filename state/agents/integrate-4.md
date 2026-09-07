# integrate-4
items: (integration only)
  apex-os:    task/p1-044-firewall-live, task/p2-005-device-maturity,
              task/p1-020-agent-graph-daemon
  apex-shell: task/p1-020-agent-graph, task/p1-044-firewall-settings,
              task/p1-038-labwc-parity, task/p1-048-guided-settings
repo: both
worktree: /var/tmp/apex-work/int-os and /var/tmp/apex-work/int-shell
branch: roadmap/v2.2

## NEXT
Measure the apex-os baseline on the clean int-os worktree at b2d7905
(`cargo test --workspace --manifest-path apexd/Cargo.toml` + tests/run-clippy.sh),
then rebase task/p1-044-firewall-live onto roadmap/v2.2.

## DONE
(nothing yet)

## IN PROGRESS
- Orientation. Both worktrees confirmed CLEAN and at origin:
    int-os    roadmap/v2.2 = b2d7905
    int-shell roadmap/v2.2 = 8d081ff

## FOUND
(nothing yet)

## BLOCKED ON
- nothing
