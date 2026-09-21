# kernel-build-2 — continuation of kernel-build (P1-043)

items: P1-043
repo: apex-os
worktree: /var/tmp/apex-work/wt-kernel-build-2
branch: task/kernel-build-2, cut fresh from roadmap/v2.2 @ 76aa2b95 (2026-09-21)

kernel-build's own branch (task/kernel-build, tip 26abfb18) already LANDED in
round 35 — do not redo its work, read ROADMAP/state/agents/kernel-build.md for
the full history and NEXT list. This card starts the next round of that NEXT
list in a clean worktree.

## NEXT (dispatched with, fill in as you go)
1. `podman save localhost/apex-kernel:local | sudo podman load` FIRST — the
   kernel image is in the ROOTLESS store, build-local.sh uses `sudo podman`,
   and its `image exists` check will miss it and silently recompile the
   kernel (~75 min wasted) if you skip this.
2. Build `core` against the kernel (`build-local.sh` or equivalent). Expect
   ~45-50 min. This is the first real exercise of the cross-tier contracts
   (`btf_scx=usable`, the `kver` check, `kernel.pin` cmp).
3. katana-runner (landed round 35) left one loose end: commit c06a48a0 changed
   the contract step's RPMS= derivation and the push-branch filter, but was
   never exercised because pushing it mid-build would have cancelled the
   in-flight run via cancel-in-progress. Run
   `gh workflow run kernel-build.yml --ref task/kernel-build-2` (or wherever
   this branch ends up) to confirm the rewritten step, once you've pushed.
4. The CI decision (self-hosted runner vs `_build_minimal`) is Andre's, not
   yours — do not engineer it.

## FOUND
-

## BLOCKED ON
- nothing yet
