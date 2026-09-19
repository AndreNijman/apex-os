# build-verify — does roadmap/v2.2 still build?

repo: apex-os
worktree: /var/tmp/apex-work/wt-build-verify  (on `task/build-verify`, reset onto roadmap/v2.2 @ 7f647470 in round 33)
branch: **task/build-verify** @ `6e4174ee`, PUSHED 2026-09-19 — one commit on top of `roadmap/v2.2` @ `7f647470`. This is the sha to land.

> Re-dispatched fresh 2026-09-19 (round 33). The previous agent died with a
> clean worktree and nothing pushed. Its measurements are kept below because
> they are correct; what changed is the diagnosis of the build failure, which
> the orchestrator settled before dispatching. Read "What round 33 already
> settled" before running anything.

## What is already measured (round 32 agent, all still true)

- `quickshell --version` from `quickshell-git-0.3.1^860.gitc6a5160-1.fc43.x86_64`
  (errornointernet COPR, on quay.io/fedora/fedora-bootc:43) prints, on **stdout**,
  exit 0, one line, 126 bytes:

      Quickshell 0.3.1 (revision c6a516096dd84d5255b409482eb4bf740b952f88, distributed by Fedora COPR (errornointernet/quickshell))

  The assertion `36535383` added matches it. Replayed verbatim under `set -eux`:
  ASSERTION_PASSED, exit 0.
- No `SHELL` directive in any Containerfile and the RUN is `set -eux`, so
  **pipefail is not in scope** and the `printf | grep -qE` 141-on-match trap does
  not apply here. Checked, not reasoned about.
- **CONFIRMED IN A REAL BUILD**: `./build-local.sh --force-core` reached
  `Containerfile.core` STEP 19/59 and printed that line byte-identical to the
  probe, and did NOT print FATAL. **The quickshell question is CLOSED. No change
  to `Containerfile.core` is needed and none should be made.**
- CI: the new multilib extraction step on the tip (run 35414179241) genuinely ran
  on the runner — `6 passed, 0 failed`, negative control 14 shadows.

## What round 33 already settled — do not re-derive any of this

**1. The local build failure is NOT a defect in the integration branch.** That
build died at `Containerfile.base` STEP 157/180 in `check-labwc-keybinds`, with
the seeded `rc.xml` holding `W-A-s` (screen reader) and `W-A-v` (voice) that the
generated block lacked. Measured four ways in the orchestrator's own session:

| apex-os rc.xml | apex-shell ref | result |
|---|---|---|
| `roadmap/v2.2` | `roadmap/v2.2` | **PASS** — 70 defaults, 4 skipped |
| `roadmap/v2.2` | `main`         | **FAIL** — exactly the two keybinds above |

**2. The cause is `build-local.sh`, and it is the third instance of a defect the
other two callers already fixed.** `build-local.sh` lines 118-127 resolve
`refs/heads/main` UNCONDITIONALLY:

    SHELL_REF="$(git ls-remote …/apex-shell refs/heads/main | awk '{print $1}')"

`build-image.yml`'s `Pin apex-shell` step (line ~210) resolves the **matching
branch name** and only falls back to `main` when apex-shell has no such branch,
*and prints which one it used*. Its own comment says why, and names these exact
two keybinds as the failure. `pr-validation.yml` learned the same lesson
earlier. `build-local.sh` never did — so **a local build of `roadmap/v2.2` can
never pass `check-labwc-keybinds`**, and that is what ate the last agent's build.

**3. CI on the tip does NOT have this problem, and is nearly green.** Run
`35415266422` (build-image, `roadmap/v2.2` @ `bd0c41ce`): `rust`, `changes`,
`core` and `base` all **succeeded** — so `check-labwc-keybinds` passes on the
runner. The run failed only in the `image` job at *Generate and attest the
SBOM*, and the log says `The runner has received a shutdown signal … The
operation was canceled`. ~~That is **GitHub infrastructure, not APEX**.~~
**SUPERSEDED — see FOUND. Round 33 reproduced it on 35433705393 and syft was
OOM-killed (exit 137) after 209 s. It is APEX's image size against a 16 GB
runner, not a GitHub reclaim. Do not restate the struck-through sentence.** The
per-SHA image tag was pushed before it died: `ghcr.io/andrenijman/apex-os:
apex-bd0c41ceb90ef9e7ed67fd938dbf94fcca7f260a` exists, digest
`sha256:aae6e5b9…`, created 03:15:08Z.

**4. A fresh build of the TIP is already in flight and you own it.** Run
**35433705393**, dispatched by the orchestrator at 09:04:38Z on `roadmap/v2.2`
@ `7f647470e222cfa23e0853cac45ef3f7e74c252e`. Non-main dispatch pushes per-SHA
tags only (`PUBLISH` guard at build-image.yml:170), so it moves nothing any
machine tracks.

## NEXT

**Nothing is outstanding in this unit.** Both verifications finished; the branch
is pushed. Two things for the ORCHESTRATOR, in order:

1. **Land `task/build-verify` @ `6e4174ee`** (one commit on `roadmap/v2.2` @
   `7f647470`; `build-local.sh`, `tests/test-build-local-shell-ref.sh`,
   `.github/workflows/pr-validation.yml`).
2. **Dispatch a new unit for the SBOM OOM below — it is APEX's, not GitHub's,
   and the roadmap's record of it is wrong.** Do NOT let it be re-diagnosed as
   "runner infrastructure" a third time. It is the ONLY thing standing between
   `roadmap/v2.2` and a fully green `build-image`.

## DONE

- **Wired into CI and both gates re-run green.** The suite is a step in
  `pr-validation.yml`'s `engine` job beside `check-suites-run-in-ci.sh`;
  `./tests/check-suites-run-in-ci.sh` -> 78 suites, 74 run by CI, 4 exempt, 0
  unrun; `./tests/check-shellcheck-coverage.sh` -> 169 discovered, 0 failing.
  Linted with the RUNNER's shellcheck (0.9.0 in
  `docker.io/koalaman/shellcheck-alpine:v0.9.0`, which disagrees with the local
  0.11.0): both `build-local.sh` and the new suite are clean.
- **The engine path selector now names `build-local\.sh`**, and that was proved
  both ways too: a change to `build-local.sh` alone gave `engine=false` under the
  old pattern and `engine=true` under the new one, while a docs-only change still
  gives `false` under both — so the selector was widened, not defeated.
- (round 32) the quickshell `--version` assertion, closed — see above.
- **Reset `wt-build-verify` onto `roadmap/v2.2` @ `7f647470`** before editing.
  Nothing was committed on the branch, so this cost nothing and avoids landing a
  workflow edit a merge behind.
- **`6e4174ee` CHANGES NOTHING THAT ENTERS THE IMAGE.** No Containerfile COPYs
  `tests/`, `.github/` or `build-local.sh`, none copies the whole context, and
  there is no `.containerignore`/`.dockerignore`. So the local build's image is
  content-identical to one built from `7f647470`; only the
  `org.opencontainers.image.revision` label differs, because `REV` is
  `git rev-parse HEAD` in this worktree. Do not read that label as "a different
  tree was built".
- **COMMITTED AND PUSHED: `task/build-verify` @ `6e4174ee`** — one commit on top
  of `roadmap/v2.2` @ `7f647470`, three files. **This is the sha for the
  orchestrator to land.** Nothing else is outstanding on the branch.
- **`build-local.sh` shell-ref block rewritten.** Three-rung chain `want -> roadmap/v2.2 -> main`,
  which is **pr-validation.yml's chain, not build-image.yml's two-rung one** —
  deliberate: build-image.yml only runs on branches apex-shell also has, while
  this script is run from whatever worktree a human is standing in, and every
  `task/*` worktree in this program has no apex-shell twin. Two rungs would send
  all of them to `main` and reproduce the failure verbatim.
- **`tests/test-build-local-shell-ref.sh` written, 25 assertions, 8 cases.**
  Fixture bare repos through the new `APEX_SHELL_REMOTE` seam; no network, no
  podman, no sudo. It runs the REAL `build-local.sh` with
  `--allow-unsigned bogus-target`, which resolves the ref and then exits 2 at
  dispatch before any podman call; every case asserts the `unknown target` line
  on stderr too, so "reached dispatch" is proved rather than assumed.
- **Proved in both directions.** New code: **25 passed, 0 failed**. Old block
  restored via `git stash push -- build-local.sh`: **8 passed, 17 failed** —
  every matching-branch, fallback-message, detached-HEAD and fatal-path
  assertion went red, while the three "reached dispatch" assertions correctly
  stayed green.
- **The suite discriminates the MIDDLE RUNG specifically, not just old-vs-new.**
  In a throwaway copy (never in the build's context), `for cand in "$want"
  roadmap/v2.2 main` was cut down to build-image.yml's two rungs. Result:
  **23 passed, 2 failed** — and the two are exactly the ones that should be,
  `task/* -> roadmap/v2.2` and `detached HEAD`, both now pinning the `main` sha.
  So the deviation from the card (three rungs, pr-validation's chain) is held by
  assertions that go red the moment someone reverts it to two.
- **Core reuse is justified on INPUTS, not just on the recipe.** `Containerfile.core`
  is unchanged f666f5c1..7f647470, and so are both of the only two repo files it
  COPYs — `files/system/src/quickshell-argc-shim.c` and
  `files/system/libexec/apex-screen-reader`. (Its other two directives are a
  pinned nerd-fonts URL and a `--from=` stage copy.) None of the 41 changed files
  is a core input; every one is base/image-tier. So the cached
  `localhost/apex-os-core:latest` is not a stale core, which is the failure mode
  build-local.sh's own header warns about.
- **Real-remote smoke test** from `wt-build-verify` (on `task/build-verify`):
  `pinned apex-shell branch 'roadmap/v2.2' — apex-shell has no branch named
  'task/build-verify'` / `vendoring 9df72cf2939345fea90db0af1fd35d2e6ebe4708`,
  byte-equal to `git ls-remote … refs/heads/roadmap/v2.2`.

## IN PROGRESS

- nothing. Both the local build and CI run 35433705393 are finished and their
  outcomes are recorded above.

## FOUND

- `build-local.sh` pins apex-shell `main` unconditionally while `build-image.yml`
  and `pr-validation.yml` both pin the matching branch. Third instance of one
  defect; the first two are already commented at length in those files.
- **SECOND defect, underneath the first, and it was silent.** The property the
  brief called load-bearing — "cannot reach the remote is FATAL, never a silent
  fallback" — was UNREACHABLE CODE. `SHELL_REF="$(git ls-remote … 2>/dev/null |
  awk …)"` under `set -euo pipefail`: pipefail gives the assignment ls-remote's
  status, errexit kills the script ON that line, and the `[ -n … ] || FATAL`
  below it never runs — with git's own message already thrown away by
  `2>/dev/null`. Measured directly, not reasoned about:
  `bash -c 'set -euo pipefail; S="$(git ls-remote https://github.invalid/nope
  refs/heads/main 2>/dev/null | awk "{print \$1}")"; echo reached'` → **exit
  128, "reached" never printed, not one word of output**. The FATAL could only
  fire if the remote answered successfully and had no `main`, which does not
  happen. It is now an ls-remote RETURN CODE decision (0 found / 2 absent /
  anything else unreachable) and git's stderr is no longer suppressed.
- `tests/check-shellcheck-coverage.sh` discovers scripts under `tests/`,
  `files/` and `android/tools/` only. **`build-local.sh` at the repo root is
  linted by nobody** — same hole the file's own header describes, one directory
  up. Not this unit's to fix; recorded for whoever owns that gate.
- **THE SBOM FAILURE IS OURS. "GitHub infrastructure, not APEX" IS WRONG, and
  round 33's own card said it. Corrected here with the smoking gun the earlier
  run could not produce.** Run 35433705393 died at the same step, and this time
  the wrapper lived long enough to say why:

      10:18:45  after prune: 86G avail; free 14213 MB of 15989
      10:22:13  line 64: 25574 Killed   SYFT_PARALLELISM=4 timeout 2700 \
                  syft "registry:$REF" -o spdx-json > /tmp/sbom.spdx.json
      10:22:14  ##[error]syft exited 137 after 209s

  **137 = 128+9 = SIGKILL.** Not 124 (the `timeout 2700` arm, which had 2491 s
  left) and not 143 (SIGTERM). syft was cataloguing a ~13 GB image with ~14.9 GB
  of RAM available and something shot it at 209 s.

  **Calibration, so the next unit does not inherit an overclaim.** That it was
  the OOM KILLER specifically is inferred from shape — SIGKILL, the phase, the
  duration, the size ratio — not read off a kernel message. Nothing printed
  `Out of memory`, and `free` is only sampled before syft starts. The
  struck-through "GitHub infrastructure, not APEX" is now firmly wrong either
  way, but confirm the mechanism before fixing it: `dmesg`/`journalctl -k` after
  the step, or a `free -m` loop in the background during syft. The candidate
  directions below hold under either mechanism.

  Checked, not assumed, that this is the SAME defect as 35415266422 rather than
  two unrelated ones: `git show bd0c41ce:.github/workflows/build-image.yml |
  grep -c SYFT_PARALLELISM` = 2, so the prior run carried the identical wrapper.
  It went silent at the same point for 153 s and then the whole RUNNER
  disappeared — which is what the same memory exhaustion looks like when it
  takes the VM instead of just the biggest process. Two consecutive runs, same
  step, same phase, same duration band, one of them an explicit SIGKILL. That is
  a reproducible resource ceiling, not a coincidence of GitHub reclaims.

  Why it is ours specifically: `SYFT_PARALLELISM=4` is a DELIBERATE raise from
  syft's default of 1, and the workflow comment says why — "this image has
  thousands of packages across two app bundles". Those bundles are the Claude
  and ChatGPT desktop apps that the 2026-09-11 product decision put into `core`
  (1.3 GB + 548 MB, and `docs/update-cost.md` already records that tension). So
  the image got much bigger, the cataloguer was told to use four workers on it,
  and a 16 GB runner cannot hold it. Every input to that is APEX's.

  Candidate directions for whoever gets the unit — NOT decided here, this was
  out of scope: drop `SYFT_PARALLELISM` back to 1 or 2; catalogue the LOCAL
  image (`oci-archive:`/`docker-archive:`) instead of `registry:`, which avoids
  holding pulled layers; add swap on the runner; or move the SBOM to a larger
  runner. Measure before choosing — the comment right above the invocation says
  the last unmeasured number in this step "cost three builds to disprove".

  Small separate defect in the same step, free to fix alongside: the error text
  is `syft exited $rc after ${_el}s (143 = killed; …)` — it hardcodes 143 as the
  "killed" hint while the code actually seen is 137. 143 is SIGTERM, 137 is
  SIGKILL, and it is the difference between "something asked it to stop" and
  "the kernel OOM-killed it". `.github/workflows/build-image.yml:1436`.
- build-image run 35415266422 died on a GitHub runner shutdown during syft/SBOM,
  after core+base+image had all built. Not an APEX defect — **but read the log
  before repeating that flatly.** Pulled with `gh run view 35415266422
  --log-failed`, the step's real shape is:

      03:23:46  installed /usr/local/bin/syft   (syft 1.52.0)
      03:23:57  after prune: /dev/root 145G, 86G avail; free 14161 MB of 15989
      03:26:30  ##[error]The runner has received a shutdown signal…
      03:26:31  ##[error]The operation was canceled.

  So it went quiet for **2m33s inside a syft scan of a ~13 GB image on a 16 GB
  runner**, and then the runner went away. "Runner shutdown signal" is what
  GitHub reports for its own reclaim AND for a VM that stopped answering, so the
  position of the silence is not nothing. One occurrence is infrastructure; the
  same stall at the same step on run 35433705393 would be a repeatable pattern
  and the "not ours" reading would be the weaker one. That distinction is the
  deliverable of this unit's NEXT item 1 — do not collapse it.

## BLOCKED ON

- nothing.

## Rules

- Headless only. Never open a window on Andre's desktop. Never run `qs -p`.
- No polkit or keyring prompts — use `sudo` or `--user`.
- Never `pkill apex-agentd`.
- Never push `main`; never open a PR.
- Use a per-agent scratch subdir; `/var/tmp/apex-work/scratch-*` is shared and an
  agent has committed another agent's message from it before.
- **The Bash tool caps at 600 s and silently backgrounds anything longer**, which
  killed the first `gh run` waiter mid-wait. What worked: `Monitor` with
  `timeout_ms: 3600000` and a bounded `until … completed` poll loop, plus a
  detached `systemd-run --user` unit (NOT `--pty`/`--wait`) for the build itself
  so it lived outside this agent's process tree.
- **Write this card as you go, never at the end.** `NEXT` is load-bearing: it is
  what a fresh agent is handed when you are killed without warning.
