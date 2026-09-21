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
   At 03:49Z (minute 27 of ~40) it was in the `compile` step, steps 1-3 green.
3. **DONE except the digest, pushed as `2cd11c6f`.** `tests/check-kernel-image-pin.sh`
   (new), the reachability resolve step in `build-image.yml`'s core job (which
   also now passes `--build-arg APEX_KERNEL_IMAGE` explicitly and refuses an
   empty one), the same gate in `pr-validation.yml`, and the corrected comment
   at `build-local.sh:105`. **The one thing still outstanding is the digest
   itself**: `Containerfile.core:93` still reads
   `ARG APEX_KERNEL_IMAGE=localhost/apex-kernel:local`, so the new gate FAILS on
   this tree today — deliberately, and that is half of its both-ways proof:
       FAIL: Containerfile.core defaults APEX_KERNEL_IMAGE to 'localhost/apex-kernel:local'.
4. **TODO** — `gh workflow run build-image.yml --ref task/kernel-publish -f force_core=true`
   and watch `core` get past `FROM ${APEX_KERNEL_IMAGE}` and the cross-tier
   contract RUN. **Record the run id in the table above the minute it exists.**

## NEXT (for a stranger picking this up)

Read the table above first. If run 35557283953 finished green, `gh run view
35557283953 --log | grep -A3 'pushed ghcr'` gives the digest; pin it into
`Containerfile.core:93` as exactly `ARG APEX_KERNEL_IMAGE=ghcr.io/andrenijman/apex-os@sha256:<64 hex>`
(one space, no quotes — `build-image.yml`'s resolve regex is stricter than the
gate's), fix the stale sentences in the comment block at lines 79-93, re-run
`./tests/check-kernel-image-pin.sh` to see it go green, commit, push, then do
step 4. If the kernel run failed on a `curl … 22`, just dispatch it again — see
the 429 note.

The akmods/nvidia failure named in FOUND item 2 is now owned by the
`kernel-akmods` unit. Do not work on it; this unit's assertion stops at "core
gets past the kernel-rpms stage and the cross-tier contract RUN".

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

## GATE PROOF — both directions, run 2026-09-21 (round 2 agent)

Harness: `/var/lab-scratch/kernel-publish/mutate.sh` copies the four files the
gate reads into a throwaway tree, applies ONE mutation, runs the gate.
`m0` is the tree with a placeholder digest pinned — the state the branch will be
in once the real digest lands.

| mutant | what it does | gate |
|---|---|---|
| m0-pinned | digest default | **exit 0** "the kernel pin is sound" |
| (tree as committed) | `localhost/apex-kernel:local` | exit 1 — the localhost branch |
| m1-tag | `ghcr.io/andrenijman/apex-os:kernel` | exit 1 — "not a digest" |
| m2-wrong-repo | a digest on `ghcr.io/attacker/apex-os` | exit 1 — "not a digest reference on …" |
| m3-empty | `ARG APEX_KERNEL_IMAGE=` | exit 1 — "no default value" |
| m4-arg-after-from | ARG moved below the first FROM | exit 1 — names both line numbers |
| m5-no-local-override | `build-local.sh` drops `--build-arg` | exit 1 |
| m6-copr-fallback | a `dnf5 install kernel-cachyos` added | exit 1 — "installs kernel-cachyos from a repository" |
| m7-no-btf | the `btf_scx=usable` refusal removed | exit 1 |
| m8-no-digestfile | `--digestfile` removed from kernel-build.yml | exit 1 |

`build-image.yml`'s resolve step was proven the same way WITHOUT spending a CI
run, by copying its body verbatim into
`/var/lab-scratch/kernel-publish/resolve-probe.sh`:

- today's `localhost` default → the regex finds no ref, prints the `::error::`
  pair and the offending line, exit 1.
- a well-formed but unpublished digest → all **five** attempts fail with
  `manifest unknown`, the loop does not die early under `set -e` (probed
  separately: `[ x -lt n ] && sleep` as the last command of a loop body does NOT
  trip errexit), the final `::error::` fires, exit 1.
- a digest that IS published → `resolved on attempt 1`, exit 0.

Incidental finding while doing that: **`ghcr.io/andrenijman/apex-os` is PUBLIC.**
`skopeo inspect --no-creds` resolves `:daily` fine. Several comments in
`build-image.yml` say "the packages are private" and hand credentials around on
that basis. Harmless, but the comments are stale.

## BOUNDS I AM HOLDING TO

- The runner's isolation is not loosened. The push uses the job's own
  `GITHUB_TOKEN` widened to `packages: write` for one job; the runner user stays
  unprivileged, podman stays rootless, and the authfile lives under
  `RUNNER_TEMP` and is deleted in an `always()` step. The rejected alternative
  (OCI archive → artifact → hosted pusher) is written into the workflow comment.
- Not landing this branch myself. Not touching `roadmap/v2.2`.
- Not editing `Containerfile.kernel` — that is kernel-build-2's ground. The curl
  retry it wants for the 429 belongs to them; noted, not done.
