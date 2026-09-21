# kernel-publish — publish the kernel image, unblock every image build

items: (unblocks all of roadmap/v2.2 — no image can be built until this lands)
repo: apex-os
worktree: /var/tmp/apex-work/wt-kernel-publish
branch: task/kernel-publish, cut from roadmap/v2.2 @ 4031d43f

## RUN IDS — record these first, they survive a dead agent

| what | run id | ref | result |
|---|---|---|---|
| kernel-build.yml, first run with publish | **35557283953** | task/kernel-publish | IN PROGRESS from 2026-09-21 11:22 AWST, ~40 min |
| build-image.yml, the proof core builds | (not dispatched yet — needs the digest from the run above) | task/kernel-publish | — |

Earlier, for context (not mine):
- `35552604603` roadmap/v2.2 — the failure this unit exists to fix.
- `35518017589` task/katana-runner — the kernel compiled green on katana in 38m39s, publishing nothing.
- `35520599712` roadmap/v2.2 — kernel build failed, **429 from GitHub** on the
  CachyOS tarball, 50 min after the previous run downloaded it. Transient rate
  limit, NOT a pin defect: both dwarves koji URLs and the tarball URL return
  200 today. If a kernel run dies at `curl … exit 22`, re-dispatch once before
  diagnosing.

## THE PLAN, and where it had got to

1. **DONE, pushed as `544143e1`.** `.github/workflows/kernel-build.yml` now
   publishes. One `podman push` to an immutable `kernel-<kver>-<sha7>` tag with
   `--digestfile`; the floating `:kernel` name is made afterwards by an
   in-registry `skopeo copy`, and only from `main` or `roadmap/**`.
2. **IN PROGRESS** — run 35557283953. What I need out of it is the line the
   step summary prints: `ARG APEX_KERNEL_IMAGE=ghcr.io/andrenijman/apex-os@sha256:…`
3. **TODO** — put that digest in `Containerfile.core`'s `ARG APEX_KERNEL_IMAGE`
   default, add `tests/check-kernel-image-pin.sh`, add the reachability guard to
   `build-image.yml`'s core job, fix the now-false comment at `build-local.sh:107`.
4. **TODO** — `gh workflow run build-image.yml --ref task/kernel-publish -f force_core=true`
   and watch `core` get past `FROM ${APEX_KERNEL_IMAGE}` and the cross-tier
   contract RUN. **Record the run id in the table above the minute it exists.**

## NEXT (for a stranger picking this up)

Read the table above first. If run 35557283953 finished green, `gh run view
35557283953 --log | grep -A3 'pushed ghcr'` gives the digest; carry on at step 3.
If it failed on a `curl … 22`, just dispatch it again — see the 429 note.

## FOUND

1. **The diagnosis in the brief was half the story.** The brief says `core` fails
   because `localhost/apex-kernel:local` does not exist in CI. True, but it would
   have failed *on the machine that has one too*: `ARG APEX_KERNEL_IMAGE` was
   declared inside the `toolbuilder` stage, and an ARG is visible to a stage's
   own `FROM` line only when declared before the FIRST `FROM` in the file. It
   expanded empty everywhere. kernel-build-2 diagnosed and fixed that; the fix
   was already sitting **uncommitted-then-unpushed** on this branch as `3dc80629`
   when I arrived. Pushing it was the first thing I did.
2. **`core` will probably still go red after my stage, in the akmods stage, and
   that is not this unit.** kernel-build-2's local `core` build (log:
   `/var/lab-scratch/kernel-build-2/core-build.log`) got all the way through the
   kernel install and the whole desktop dnf transaction, then died at
   `akmods --force --kernels 7.2.6-cachyos1.apex1.fc43.x86_64 --kmod nvidia`
   → `Building and installing nvidia-kmod` EXIT=1. That is nvidia 580.178.04
   against CachyOS 7.2.6. Expect the same ~35-40 min into the CI core build.
   **My assertion is "core gets past the kernel-rpms stage and the cross-tier
   contract RUN", which is the stage that failed.** Somebody owns the akmods
   one; it is not kernel-publish and it is not kernel-build-2's card either.
3. **`ghcr.io/andrenijman/apex-os:kernel` needs no new GHCR package and no new
   permission.** Every tier already shares the one `apex-os` repository,
   distinguished by tag, and that package is already linked to this repo — so
   `build-image.yml`'s existing `sudo podman login` in the core job can already
   pull the kernel by digest. Checked in `build-image.yml`'s `env:` block, not
   assumed.
4. **Nothing in `.github/` deletes package versions.** Grepped for
   `delete-package|package-versions|untagged|retention`: the only hits are
   `retention-days` on build artefacts. So a published kernel digest is not at
   risk of being pruned, and the immutable per-build tag keeps it tagged anyway.
5. `tests/check-kernel-pin.sh` has `CF=Containerfile.kernel`, so its
   `NOT_FROM_PIN="APEX_KERNEL_IMAGE|VERSION_ID"` line does not touch
   `Containerfile.core` and my changes cannot trip it.
6. Katana's runner is **online and idle**, `/var/lab` has 479 GiB free, and
   `/usr/bin/skopeo` exists there — all three checked before dispatching.

## BOUNDS I AM HOLDING TO

- The runner's isolation is not loosened. The push uses the job's own
  `GITHUB_TOKEN` widened to `packages: write` for one job; the runner user stays
  unprivileged, podman stays rootless, and the authfile lives under
  `RUNNER_TEMP` and is deleted in an `always()` step. The rejected alternative
  (OCI archive → artifact → hosted pusher) is written into the workflow comment.
- Not landing this branch myself. Not touching `roadmap/v2.2`.
- Not editing `Containerfile.kernel` — that is kernel-build-2's ground. The curl
  retry it wants for the 429 belongs to them; noted, not done.
