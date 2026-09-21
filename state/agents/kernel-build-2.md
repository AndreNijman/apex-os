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
1. DONE — see below. Do NOT run `podman save | sudo podman load` again, it's
   already done.
2. **IN PROGRESS as of 2026-09-21 ~10:25.** `core` build running as user unit
   `kernel-build-2-core` (`systemd-run --user`, no `--collect` so it stays
   inspectable if it fails). Log: `/var/lab-scratch/kernel-build-2/core-build.log`.
   Check with `systemctl --user is-active kernel-build-2-core` and
   `tail -f /var/lab-scratch/kernel-build-2/core-build.log`.
   **DO NOT LAUNCH A SECOND ONE.** If it's still `active`, just wait/monitor —
   do not re-run `./build-local.sh`. Expect completion ~11:10-11:15.
   Command used: `./build-local.sh core` (NOT `--force-core` — that flag also
   forces a kernel recompile, see FOUND #1). If this fails, do NOT retag
   `apex-os-core:stale-20260919` back to `:latest` — investigate the failure
   log first; a base built on the pre-kernel-tier stale core is worse than
   `build_base` FATALing with "run core first".
3. Once core succeeds and something is committed+pushed on this branch, run
   `gh workflow run kernel-build.yml --ref task/kernel-build-2` (repo
   AndreNijman/apex-os) and watch it to confirm the rewritten RPMS=
   derivation from c06a48a0. Checked: this branch is NOT `main`/`roadmap/**`
   so pushing does not auto-trigger the workflow's push filter — only the
   explicit `gh workflow run` dispatch will start it. `gh auth status` is
   good (logged in as AndreNijman, workflow scope present).
4. The CI decision (self-hosted runner vs `_build_minimal`) is Andre's, not
   yours — do not engineer it.

## FOUND
1. **`--force-core` is overloaded and will silently force a ~75-min kernel
   recompile too.** `build_kernel()`'s reuse guard
   (`if [ "$FORCE_CORE" = 0 ] && sudo podman image exists "$KERNEL_IMG"`) is
   gated on the SAME `FORCE_CORE` variable as `build_core()`'s. There is no
   separate "force just core" flag. Confirmed by reading the script, not by
   triggering it.
2. **The existing `localhost/apex-os-core:latest` (built 2026-09-19) predates
   the kernel tier entirely** — created before `apex-kernel:local`
   (2026-09-20) and missing `/usr/share/apex-os/kernel/build.txt` outright
   (the kernel tier's own manifest file). Plain `./build-local.sh core` would
   have hit the "reuse existing core" branch and built nothing — a silent
   no-op exercising none of the cross-tier contracts this step exists to
   test. Retagged it out of the way rather than deleting:
   `localhost/apex-os-core:stale-20260919` (same image ID
   `8bf32f2fcf89…`), then untagged `:latest` (by ID, not by bare name, so
   only that one tag was stripped). This is why NEXT #2 runs plain
   `./build-local.sh core`, not `--force-core`.
3. `kernel/kernel.pin` in this worktree is byte-identical to
   `/manifest/kernel.pin` inside `localhost/apex-kernel:local` (checked with
   `podman create`+`cp`+`cmp`) — confirms the loaded rootless→root kernel
   image is still current and safe to reuse, no rebuild needed.
4. Round-36 roadmap orchestrator already holds a machine-wide
   `sleep:idle:handle-lid-switch` block inhibitor (`systemd-inhibit --list`),
   so the core build was launched without a redundant one of its own.
5. No concurrent `podman build` was running on the box before launch (checked
   `podman ps -a` / `pgrep -af 'podman build'`); one stray `sudo podman`
   container from `apex-sdboot-reg` was exited 25h ago, unrelated.

## BLOCKED ON
- nothing yet — waiting on the core build to finish (~45-50 min from launch)
