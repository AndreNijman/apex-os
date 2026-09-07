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
apex-os baseline `cargo test` is RUNNING in the background (log
/var/tmp/apex-int4-logs/test-baseline.log). When it finishes, record the count,
run tests/run-clippy.sh, then land branch 1:
  cd /var/tmp/apex-work/int-os
  git checkout -b land/p1-044 origin/task/p1-044-firewall-live
  git rebase --onto roadmap/v2.2 9a24d2b761213d5965af41592c68e7701b55c2ab
  git checkout roadmap/v2.2 && git merge --ff-only land/p1-044 && git push --force-with-lease

## DONE
- Nothing landed yet.

## IN PROGRESS
- Both worktrees CLEAN and at origin: int-os b2d7905, int-shell 8d081ff.

## MEASURED BASELINES
apex-shell @ 8d081ff (run from repo ROOT, XDG_* -> /var/tmp/apex-shell-xdg):
  check-no-conflict-markers  PASS
  settings-semantics         33/0
  settings-pages             15/0
  check-color-tokens         22/0    EXPECT_WHITE_FG = 211
  check-scale-tokens          5/0
  agent-state                27/0
apex-os @ b2d7905: build rc=0 (43s). test count PENDING.
Scripts: /var/tmp/apex-int4-logs/{runtests.sh,count.sh,shelltests.sh}
  (both refuse to run on a dirty tree, exit 3 — integrate-3's guard, kept)
Build cache CARGO_TARGET_DIR=/var/tmp/apex-build-cache/int4

## FOUND
- The `[0/10 added lines on tip]` entries are SETTLED for all three apex-os
  branches: `git diff origin/roadmap/v2.2 origin/<b> -- <branch files>` still
  shows the FULL branch insertion count (1338 / 2259 / 2677). Nothing landed
  early; nothing to skip. (The extra DELETIONS vs the merge-base stat are the
  tip's own later work on the same files, not landed branch content.)
- The brief said task/p1-044-firewall-settings forked at 0fd12ee. It did not:
  `git merge-base` says ALL FOUR apex-shell branches fork at d2d1f33, which is
  an ancestor of roadmap/v2.2. Using the measured fork point.
- Containerfile.base and Containerfile.core BOTH still exist on roadmap/v2.2
  (alongside Containerfile.apex), so p2-005's edits to them are not a
  modify/delete conflict. Checked before rebasing, not during.
- **HEADLESS HAZARD, and it reorders the shell testing.** p1-048's 0b36897
  found TWELVE tests/run-*.sh that start quickshell/a compositor on the
  INHERITED WAYLAND_DISPLAY — i.e. on Andre's desk. They are exactly the
  suites a naive "run the nested labwc/sway suites" instruction would run,
  including run-nested-labwc.sh itself. I will NOT run any of the twelve until
  p1-048 is on the tip. The twelve: run-agent-center-smoke, run-nested-labwc,
  run-compositor-facade-test, run-hypr-configerrors-test, run-nexus-smoke,
  run-niri-keybinds-test, run-plugin-host-test, run-popup-smoke,
  run-remote-agent-smoke, run-scaling-test, run-service-tier-test,
  measure-idle-cost/measure-idle-inhibit.
  VERIFIED SAFE (they already unset WAYLAND_DISPLAY + private XDG_RUNTIME_DIR):
  run-settings-pages-test.sh, run-agent-state-render-test.sh, run-nav-geometry.
- I will NOT run tests/test-apex-firewall-live.sh or test-apex-firewall-ssh.sh:
  they ssh to katana and load a default-drop policy there. Static review only.

## ANTICIPATED CONFLICTS (from file overlap, before rebasing)
- apex-os: the three branches are file-DISJOINT. Zero expected.
- apex-shell: src/services/qmldir (p1-020 +7, p1-044 +2, p1-048 +1);
  src/nexus/PageRegistry.qml (p1-044 +13, p1-048 +16);
  .github/workflows/ci.yml (p1-020 +80, p1-038 +47, p1-048 +56).
  All additive; expect keep-both.

## BLOCKED ON
- nothing
