## LANDABLE — `76a04c49`

The kernel digest is pinned and the gate is green both ways. Landing this
unblocks every image build on the board; NOT landing it leaves `roadmap/v2.2`
with `ARG APEX_KERNEL_IMAGE=localhost/apex-kernel:local`, which is the exact
line that killed run 35552604603 and every image build after it.

- `Containerfile.core:112` = `ARG APEX_KERNEL_IMAGE=ghcr.io/andrenijman/apex-os@sha256:2ff544dd021478dbd36dab4efb1ce43ad9cc858f16cd07a271f01693822972d6`,
  a digest re-verified against the registry (not this card) before it was written in.
- `tests/check-kernel-image-pin.sh`: **13/13 ok, exit 0** on the real tree, and
  exit 1 on **ten** single-change mutants (eleven counting the re-test of the
  one that initially got through) — see GATE PROOF ROUND 39 below.
- Merged up to `origin/roadmap/v2.2` @ `b137f03f`, clean. `check-no-conflict-markers.sh`,
  `check-containerfile-assertions.sh` and `check-kernel-pin.sh` all exit 0.
- **THE ASSERTION IS MET, and then some.** CI run 35582968952's `core` job:
  step 8 "Resolve the kernel tier and prove it is reachable" success, step 9
  **"Build core" :: SUCCESS** (17:58 AWST, a full cache-bypassing rebuild).
  `core` did not merely get past `FROM ${APEX_KERNEL_IMAGE}` and the cross-tier
  contract RUN — **the whole `core` job is green**, through verify, push and
  cosign signing. For the first time in this program, `core` builds in CI.
  Proven from the 7,856-line log, not from the tick: `resolved on attempt 1`;
  `FROM ghcr.io/andrenijman/apex-os@sha256:2ff544dd… AS kernel-rpms`;
  `btf_scx=usable`; `kernel image was built from this commit's kernel/kernel.pin`
  (the `cmp` identity check — the digest is the RIGHT kernel for this tree, not
  merely a reachable one); all four kernel packages installed from the copied
  files. Secure Boot intact: kernel + 14 out-of-tree modules signed with the
  APEX MOK. `76a04c49` puts all of this in the evidence file.
- Three commits: `72bab38d` is the pin itself; `b505b747` closes a seam the
  mutants found (the gate stripped quotes, `build-image.yml`'s resolve regex
  does not — a quoted pin passed the gate and would have died in CI blaming the
  digest). Landing `72bab38d` alone is enough to unblock builds; `b505b747` is
  a gate-only change and touches no image input. `e58d39bc` adds
  `ROADMAP/evidence/kernel-publish-20260921.md` — the file `Containerfile.core`'s
  ARG comment has been citing since `72bab38d`, and which until now existed on
  no branch. That citation resolves on this branch and will resolve on v2.2.
- **Landability checked empirically, not asserted**: `git merge-tree --write-tree
  origin/roadmap/v2.2 HEAD` produces a conflict-free tree, `3dbbbc14`.
- **The workflow collision the round-39 brief warned about is resolved, by them.**
  `origin/task/kernel-akmods` moved `4031d43f` → `0cc68901`, and `0cc68901` is
  `Merge task/kernel-publish into task/kernel-akmods` — they took `72bab38d` and
  `b505b747` as instructed. The `ARG APEX_KERNEL_IMAGE=` line is byte-identical
  on their tip at the same line 112, and they took `check-kernel-image-pin.sh`
  unmodified. They do NOT have `e58d39bc` (pushed after their merge), which is
  harmless — a new file nothing else touches. **Either landing order works.**
- All three commits carry **no AI attribution**; checked across all three with
  `git log -3 --format='%h%n%B' | grep -iE 'co-authored|claude-session'`.
- CI run 35582968952 on `72bab38d`: the `changes` job, which is where
  `check-kernel-image-pin.sh` runs, is **completed/success** — the gate passes
  on GitHub's runner, not just this laptop.

# kernel-publish — publish the kernel image, unblock every image build

items: (unblocks all of roadmap/v2.2 — no image can be built until this lands)
repo: apex-os
worktree: /var/tmp/apex-work/wt-kernel-publish
branch: task/kernel-publish, cut from roadmap/v2.2 @ 4031d43f

## RUN IDS — record these first, they survive a dead agent

| what | run id | ref | result |
|---|---|---|---|
| kernel-build.yml, first run with publish | **35557283953** | task/kernel-publish | **GREEN.** Published `sha256:2ff544dd…`, tag `kernel-7.2.6-cachyos1.apex1.fc43.x86_64-544143e`. PINNED in `72bab38d`. |
| build-image.yml, the proof core builds | **35582968952** | task/kernel-publish @ `72bab38d` | **`Build core` :: SUCCESS at 17:58 AWST** (35 min, `force_core=true`, cache-bypassing). `changes` success, `rust` success. Assertion MET. https://github.com/AndreNijman/apex-os/actions/runs/35582968952 |

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

## ROUND 2 — what this agent added on top of `2cd11c6f`

- `d5441e93` merged `origin/roadmap/v2.2` (13 commits, all boot-v2/docs) into the
  branch. **Zero file overlap** with anything this unit touches, so the CI proof
  runs against the tree that will actually land. Checked, not assumed:
  `git diff --name-only 76aa2b95..origin/roadmap/v2.2` and the same against HEAD
  share no path.
- `b981e348` wires the gate into `build-local.sh`'s `build_core()`. `2cd11c6f`'s
  message claimed build-local.sh got the gate; it had only got a corrected
  comment. Placed INSIDE `build_core()` because
  `tests/test-build-local-shell-ref.sh` copies build-local.sh alone into a
  throwaway repo with no `tests/` and runs it with a bogus target — that suite
  still passes 25/25.
- Housekeeping observed while checking the branch, none of it this unit's:
  `tests/check-shellcheck-coverage.sh` fails on `android/tools/release-version.sh`
  (untouched here, present on the base); `tests/check-kernel-drift.sh` exits 2
  "COULD NOT TELL — 1 lookup could not be performed" on this laptop, a network
  lookup; and `Containerfile.core`'s ARG comment cites
  `ROADMAP/evidence/kernel-build-20260921.md`, which exists on NO branch —
  repointed to this unit's evidence file when the digest landed.

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

---

## ROUND 38 CONTINUATION — written by the orchestrator, 2026-09-21 13:55 AWST

The round-37 agent died at the 12:06 shutdown with a clean tree at
`b981e348` = `origin/task/kernel-publish`. Nothing of yours was lost.

### Run 35557283953 is GREEN and the digest exists

`gh run view 35557283953`: `status=completed conclusion=success`,
updated 2026-09-21T04:08:33Z. The orchestrator read the registry directly
(`skopeo list-tags` / `skopeo inspect --no-creds`):

| tag | digest | created |
|---|---|---|
| `kernel-7.2.6-cachyos1.apex1.fc43.x86_64-544143e` | `sha256:2ff544dd021478dbd36dab4efb1ce43ad9cc858f16cd07a271f01693822972d6` | 2026-09-21T04:05:50Z |

The floating `:kernel` tag was NOT made — correct, this ran on a `task/`
branch. So the line for `Containerfile.core:93` is exactly:

```
ARG APEX_KERNEL_IMAGE=ghcr.io/andrenijman/apex-os@sha256:2ff544dd021478dbd36dab4efb1ce43ad9cc858f16cd07a271f01693822972d6
```

Re-verify it yourself with `skopeo inspect --no-creds docker://ghcr.io/andrenijman/apex-os@sha256:2ff544dd…`
before pinning; a digest copied from a card is a claim, not a fact.

### Tree state

`origin/roadmap/v2.2` is `71bc2177` (luks-boot and windows-installer-3
merges; `installer/`, `windows-installer/`, `tests/suites-not-in-ci.txt` —
no overlap with your four files). Merge it first.

### NEXT (supersedes the NEXT above)

1. Pin the digest at `Containerfile.core:93` (one space, no quotes). Fix the
   stale sentences in the comment block at lines 79-93, including the
   citation of `ROADMAP/evidence/kernel-build-20260921.md`, which exists on
   no branch — write `ROADMAP/evidence/kernel-publish-20260921.md` in the
   apex-os repo and point at that.
2. `./tests/check-kernel-image-pin.sh` → expect "the kernel pin is sound".
   That plus the table above is the both-ways proof. Commit, push.
3. `gh workflow run build-image.yml --ref task/kernel-publish -f force_core=true`.
   Which runner does the core job use? If katana: `ssh katana apex game
   status` must say `active : false` first (it did at 13:50 AWST). Record
   the run id in the RUN IDS table THE MINUTE IT EXISTS.
4. Watch it past `FROM ${APEX_KERNEL_IMAGE}` and the cross-tier contract RUN.
   That is your assertion. If the akmods stage goes red afterwards, do not
   fix it: paste the exact failing lines into `agents/kernel-akmods.md`
   under a heading `### FROM kernel-publish — CI core run <id>` and stop.
   kernel-akmods' own reproducer PASSED this morning, so a CI failure there
   is a real second data point for them.
5. Tell the orchestrator via this card (DONE section) when the gate is green
   on your tree. Landing is the orchestrator's; do not merge to roadmap/v2.2.

Battery was 58% and discharging at 13:42; CI runs cost this machine nothing,
but read `/sys/class/power_supply/BAT*/status` before any local build.

---

## ROUND 38 AGENT — started 2026-09-21 ~14:05 AWST

Fresh agent on the card above. Working log, newest at the bottom.

- **Digest re-verified against the registry, not the card**: `skopeo inspect
  --no-creds docker://ghcr.io/andrenijman/apex-os@sha256:2ff544dd…` resolves,
  RepoTags lists `kernel-7.2.6-cachyos1.apex1.fc43.x86_64-544143e`, and run
  35557283953's own log line is `pushed ghcr.io/andrenijman/apex-os@sha256:2ff544dd021478dbd36dab4efb1ce43ad9cc858f16cd07a271f01693822972d6`.
  All three agree.
- **Merged `origin/roadmap/v2.2` (71bc2177) → `3d0925b8`**, clean, no
  conflicts. `kernel/` and `Containerfile.kernel` have zero drift since the
  kernel was built at `544143e1`, checked against both HEAD and v2.2 — so the
  `cmp kernel.pin` identity check in `Containerfile.core` will match.
- Pre-pin gates on the merged tree: `check-kernel-image-pin.sh` **exit 1**
  (`FAIL: Containerfile.core defaults APEX_KERNEL_IMAGE to 'localhost/apex-kernel:local'.`)
  — the red half on the REAL tree; `check-suites-run-in-ci.sh` 103 suites / 96
  in CI / 7 exempt / 0 unrun; `check-no-conflict-markers.sh` PASS.
- The core job runs on `ubuntu-24.04` (`build-image.yml:432`), not katana, so
  no gaming check applies. `PUBLISH` is `github.ref == refs/heads/main`, so a
  task-branch run publishes nothing.
- Battery 50% discharging at start; no local build is planned, CI costs this
  machine nothing.

### NEXT (round 38, supersedes everything above)
Pin the digest at Containerfile.core:93, fix the comment block, write
`ROADMAP/evidence/kernel-publish-20260921.md`, gate green, commit, push.

---

## ROUND 39 CONTINUATION — written by the orchestrator, 2026-09-21 17:20 AWST

The round-38 agent was killed by a **session usage limit at 14:59 AWST**
(`ROADMAP/state/autoresume.log`: `resume session ended (exit 1)`). It was never
messaged. You are a FRESH agent and this card is your whole inheritance —
everything above stands unless this section contradicts it, and where it
contradicts it, this section wins.

**Hard deadline: this orchestrator runs under `timeout 4h` and dies at about
21:08 AWST.** Commit and push small and often. Update this card after every
commit and whenever NEXT changes — a card that is only correct at the end is
worth nothing, which is the entire reason this directory exists.

**Write `## LANDABLE` at the top of this card, with the sha, the moment your
branch is ready to merge onto `roadmap/v2.2`.** The orchestrator lands on that
signal and will not guess. If it is NOT landable, say why in one line —
"landing this would break X" is a finding, not a failure.
### NEXT — rewritten for round 39, in this order

1. **Pin the digest. It is already known and it is in this card.** Round 38's
   orchestrator read it off the registry: run **35557283953** is GREEN, tag
   `kernel-7.2.6-cachyos1.apex1.fc43.x86_64-544143e`, digest
   `sha256:2ff544dd021478dbd36dab4efb1ce43ad9cc858f16cd07a271f01693822972d6`.
   `Containerfile.core:93` still reads `ARG APEX_KERNEL_IMAGE=localhost/apex-kernel:local`,
   which is why `tests/check-kernel-image-pin.sh` FAILS on this tree today —
   deliberately, and that failure is half of the gate's both-ways proof.
   Re-verify the digest still resolves before you write it in; then run the
   gate and show it PASS, and show it still FAILs on a bad pin.
2. Your branch was pushed by the orchestrator at `3d0925b8` (a merge of
   `origin/roadmap/v2.2`; the round-38 agent had it locally and unpushed).
   Nothing of yours was lost.
3. **Workflow collision.** `task/kernel-akmods` is live this round and has
   uncommitted edits to `.github/workflows/build-image.yml` and
   `pr-validation.yml` — the same two files your branch adds +88 and +14 lines
   to. That agent has been told to merge YOUR branch before pushing. If you see
   `origin/task/kernel-akmods` move, merge it back before you land.
4. `gh workflow run build-image.yml --ref task/kernel-publish -f force_core=true`,
   watch `core` get past `FROM ${APEX_KERNEL_IMAGE}` and the cross-tier
   contract RUN, and **record the run id in the table at the top of this card
   the minute it exists** — the table is what survives you.
5. Expect `core` to go red LATER, in the akmods stage. That is `kernel-akmods`'
   unit, live in parallel, and it is not yours. Getting past `FROM` and the
   cross-tier RUN is your success condition; say so plainly rather than
   inheriting somebody else's red.

You are the unblocker for every image build on the board. The moment the pin is
in and the gate is green both ways, write `## LANDABLE` at the top of this card
with the sha — the orchestrator will merge it onto `roadmap/v2.2` without
waiting for the CI core build to finish.

### The contract (ROADMAP/state/README.md, short form)

Keep this card's `NEXT` / `DONE` / `IN PROGRESS` / `FOUND` / `BLOCKED ON`
sections current **as you go, never at the end**. `NEXT` is load-bearing: one
line, the exact next action, specific enough that a stranger could do it.
Everything else can be re-derived from git; the next action cannot.

### Constraints (non-negotiable)

- Never push `main`, never open a PR — final integration only.
- **Headless only.** Never open a window on Andre's desktop. Never run `qs -p`.
- No polkit and no keyring prompts (`sudo` / `--user`, never an agent helper).
- Never `pkill apex-agentd`.
- Do not interrupt gaming on katana.
- Scratch goes in `/var/lab-scratch/<your-slug>/`, NOT `/tmp` (tmpfs, 15 GB on
  29 GB RAM — a stdout-only Bash failure there is memory, not disk). The
  scratchpad is shared between agents: use your own subdirectory.
- Long builds run in the FOREGROUND or under `systemd-run --user`; a
  backgrounded `podman` gets SIGTERMed and still exits 0.

---

## ROUND 39 AGENT — started 2026-09-21 ~17:20 AWST

Working log, newest at the bottom.

- **Digest re-verified against the registry before pinning**, not taken from
  this card: `skopeo inspect --no-creds docker://ghcr.io/andrenijman/apex-os@sha256:2ff544dd…`
  resolves, `Created 2026-09-21T04:05:50Z`, labels `org.apexos.tier = kernel`
  and `org.apexos.kernel.manifest = /manifest/kernel-build.txt`, and its only
  `kernel*` tag is `kernel-7.2.6-cachyos1.apex1.fc43.x86_64-544143e`. Right image.
- **`origin/roadmap/v2.2` had moved again**, past the `71bc2177` this card names:
  now `b137f03f` (+3 commits, `docs/apex-owns-its-esp.md`, `docs/boot-v2.md`,
  `windows-installer/ARCHITECTURE.md` — docs only, zero overlap with my six
  files). Merged clean as `8291bf5b`.
- **`72bab38d` — THE PIN IS IN, and pushed.** `Containerfile.core:112` is now
  `ARG APEX_KERNEL_IMAGE=ghcr.io/andrenijman/apex-os@sha256:2ff544dd021478dbd36dab4efb1ce43ad9cc858f16cd07a271f01693822972d6`.
  Checked against `build-image.yml:534`'s resolve regex directly (it is stricter
  than the gate's — one space, no quotes): the `sed -nE` matches and prints the ref.
- **Gates on the real tree, all exit 0**: `check-kernel-image-pin.sh` 13/13 ok
  → "the kernel pin is sound"; `check-no-conflict-markers.sh`;
  `check-containerfile-assertions.sh`; `check-kernel-pin.sh` ("all checks passed",
  confirming its `CF=Containerfile.kernel` scope still does not touch my line).
- Commit carries **no AI attribution** — the two commits before it on this
  branch (`2cd11c6f`, `544143e1`) do carry `Co-Authored-By`/`Claude-Session`
  trailers, which is against Andre's standing rule; I did not copy the habit.

### NEXT (round 39) — the pin, the gate and the dispatch are all DONE

**Do NOT re-dispatch build-image.yml.** Run 35582968952 is already in flight on
`72bab38d`; a second `force_core=true` would queue behind it in the
`apex-image-publish` concurrency group and cost another 45 minutes for nothing.

**THIS UNIT IS FINISHED.** Everything it owes is committed, pushed and proven:
the pin, the gate green both ways, the CI core build green, the evidence file.
`task/kernel-publish` @ `76a04c49` is landable and the tree is clean.

The only thing left is the orchestrator's: **merge `76a04c49` onto
`roadmap/v2.2`.** Nothing else on this roadmap reaches a machine until that
merge happens, and it is now a proven-green merge rather than a hopeful one.

If you are a fresh agent and the merge has already happened, there is nothing
here for you — go pick up another unit. Do NOT re-dispatch build-image.yml;
35582968952 already answered the question.

### GATE PROOF — ROUND 39, re-run against the PINNED tree

The round-2 proof was run against a tree with a *placeholder* digest. Pinning
the real one moved that harness's base under it, so I re-ran with
`/var/lab-scratch/kernel-publish/r39/mutate39.sh`, which `diff`s every mutant
against the source and **refuses to report an exit code for a mutation that
changed nothing** — a mutation written against the old `localhost` line would
otherwise have silently no-opped and read as green. Full output:
`/var/lab-scratch/kernel-publish/gate-proof-round39.out`.

| mutant | gate |
|---|---|
| the real pinned tree, unmutated | **exit 0**, 13/13 ok |
| reverted to `localhost/apex-kernel:local` | exit 1 |
| `:kernel` floating tag | exit 1 |
| same digest, `ghcr.io/attacker/apex-os` | exit 1 |
| `ARG APEX_KERNEL_IMAGE=` empty | exit 1 |
| digest truncated to 40 hex | exit 1 |
| **pin wrapped in double quotes** | **exit 0 — got through.** Closed by `b505b747`, re-mutated → exit 1 |
| ARG moved below the first `FROM` | exit 1, names both line numbers |
| `--digestfile` removed from kernel-build.yml | exit 1 |
| `build-local.sh` drops `--build-arg` | exit 1 |
| `btf_scx=usable` refusal removed | exit 1 |

## FOUND (round 39)

7. **The gate and the thing it protects disagreed about quoting, and mutation
   testing is the only reason anyone knows.** `check-kernel-image-pin.sh`
   stripped optional quotes before judging the pin (correct — the Dockerfile
   parser accepts them). `build-image.yml:534`'s resolve step uses an anchored
   `sed -nE 's|^ARG APEX_KERNEL_IMAGE=(ghcr\.io/…@sha256:[0-9a-f]{64})$|\1|p'`
   that does not. A quoted pin would build locally, pass the millisecond gate
   whose entire job is catching this class, and then fail in CI with a message
   **accusing the digest** — which is well formed — rather than the quotes.
   `b505b747` makes the stricter parser win. This is the "a gate that inspects
   nothing" family again, in its subtler form: the gate inspected the right
   line and applied a looser rule than its consumer.
8. **`origin/roadmap/v2.2` moved twice during this unit's life** (`71bc2177` →
   `b137f03f`), both times docs-only. Re-check it before landing rather than
   trusting any sha written in this card, including the ones I wrote.
10. **The gate PROVABLY ran in CI, and proving it nearly went wrong.** A green
   job is not evidence a gate executed — that is the dominant CI defect family
   in this repo. Checked properly: run 35582968952's `changes` job, step 4,
   "The kernel tier is pinned, and pinned to a digest" :: completed/success,
   and its log contains the gate's own output, including
   `ok: APEX_KERNEL_IMAGE is declared at line 112, before the first FROM at line 114`
   and `check-kernel-image-pin: the kernel pin is sound`. Saved:
   `/var/lab-scratch/kernel-publish/r39/ci-changes-job.clean.log`.
11. **GOTCHA that cost me ten minutes and reads exactly like a disaster:**
   `gh api repos/…/actions/jobs/<id>/logs` returns a **BOM + CRLF** stream, and
   a plain `grep` over it finds NOTHING — no match, no "binary file matches",
   no error, exit 1. My first search for `check-kernel-image-pin` in a 762-line
   log came back empty and read as "the gate did not run in CI". It had run and
   passed. Use `grep -a`, or `tr -d '\r' | sed 's/\xef\xbb\xbf//'` first.
   Same family as this repo's `grep -q`/pipefail trap: a silent zero that looks
   like a real negative finding. **Also: job logs are NOT retrievable while the
   run is in progress** — both `gh run view --log` and the job-logs API return
   empty. Wait for completion.
9. `tests/check-shellcheck-coverage.sh` still fails on
   `android/tools/release-version.sh` — pre-existing, on the base, not mine.
   Confirmed by running shellcheck on my edited gate both before and after the
   change: identical single SC2016 *info*, below the repo's `-S warning`
   threshold. My file is not among the newly-failing.
