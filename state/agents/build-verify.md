# build-verify — does roadmap/v2.2 still build?

repo: apex-os
worktree: /var/tmp/apex-work/wt-build-verify  (exists, on `task/build-verify` @ f666f5c1)
branch: **task/build-verify** — never pushed; nothing is committed on it yet

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
operation was canceled`. That is **GitHub infrastructure, not APEX**. The
per-SHA image tag was pushed before it died: `ghcr.io/andrenijman/apex-os:
apex-bd0c41ceb90ef9e7ed67fd938dbf94fcca7f260a` exists, digest
`sha256:aae6e5b9…`, created 03:15:08Z.

**4. A fresh build of the TIP is already in flight and you own it.** Run
**35433705393**, dispatched by the orchestrator at 09:04:38Z on `roadmap/v2.2`
@ `7f647470e222cfa23e0853cac45ef3f7e74c252e`. Non-main dispatch pushes per-SHA
tags only (`PUBLISH` guard at build-image.yml:170), so it moves nothing any
machine tracks.

## NEXT

In this order.

1. **Watch 35433705393 to completion** (`gh run watch 35433705393`, or poll
   `gh run view`). It is ~1 hour. Report per job, and if the SBOM step dies the
   same way again, say whether it is the same runner-shutdown signature or
   something of ours — those are different answers and the difference is the
   whole point of this unit.
2. **Fix `build-local.sh` to resolve the matching branch**, the way
   `build-image.yml` does: try `refs/heads/$(git rev-parse --abbrev-ref HEAD)`,
   fall back to `refs/heads/main`, **print which one it used and why**, and keep
   the existing "cannot reach the remote is FATAL, never a silent fallback"
   property — that property is right and is separately load-bearing. Note the
   branch you are ON is `task/build-verify`, which apex-shell does not have, so
   the fallback path is the one your own build will take unless you build from
   `/var/tmp/apex-work/int-os` or pass `APEX_SHELL_REF=` explicitly. Say which
   you did.
3. **Make it fail in both directions before you believe it.** A gate that runs
   and inspects nothing is the dominant CI defect family in this repo. Prove the
   matching-branch path is taken on a roadmap branch AND that the main fallback
   still fires (and still announces itself) on a branch apex-shell lacks.
4. **Re-run the local build** on the integration tip with the shell ref pinned
   to apex-shell `roadmap/v2.2`, foreground under `systemd-run --user
   --unit=apex-build-verify-2 --pty` or equivalent. **A backgrounded podman is
   SIGTERMed** — it truncates the log and still exits 0. Do NOT regenerate
   `files/desktop/labwc/rc.xml` against apex-shell `main`: that reverts the
   accessibility and voice keybinds and is the wrong half of the pair to move.
5. Commit the `build-local.sh` fix on `task/build-verify`, push it, and put the
   tip sha in this card's DONE so the orchestrator can land it.

## DONE

- (round 32) the quickshell `--version` assertion, closed — see above.

## IN PROGRESS

- nothing; the previous agent's worktree is clean and nothing is committed.

## FOUND

- `build-local.sh` pins apex-shell `main` unconditionally while `build-image.yml`
  and `pr-validation.yml` both pin the matching branch. Third instance of one
  defect; the first two are already commented at length in those files.
- build-image run 35415266422 died on a GitHub runner shutdown during syft/SBOM,
  after core+base+image had all built. Not an APEX defect.

## BLOCKED ON

- nothing.

## Rules

- Headless only. Never open a window on Andre's desktop. Never run `qs -p`.
- No polkit or keyring prompts — use `sudo` or `--user`.
- Never `pkill apex-agentd`.
- Never push `main`; never open a PR.
- Use a per-agent scratch subdir; `/var/tmp/apex-work/scratch-*` is shared and an
  agent has committed another agent's message from it before.
- **Write this card as you go, never at the end.** `NEXT` is load-bearing: it is
  what a fresh agent is handed when you are killed without warning.
