# android-release-2 — dry-run the release, now that Andre's part is done

items: none (queue id `android-release` stays open; this is a direct continuation)
repo: apex-os
worktree: /var/tmp/apex-work/wt-android-release-2
branch: task/android-release-2, cut fresh from roadmap/v2.2 @ 76aa2b95 (2026-09-21)
No code changes were needed this round; branch tip is still 76aa2b95, pushed
and matching origin exactly.

## STATUS — the dry run cannot be dispatched. This is structural, not a bug.

`gh workflow run release-android.yml -R AndreNijman/apex-os -f dry_run=true`
**404s**: `workflow release-android.yml not found on the default branch`. Not a
typo, not an auth issue — checked directly against the Actions API
(`gh api repos/AndreNijman/apex-os/actions/workflows`): only 9 workflows are
registered (boot-v2, build-image, katana-probe, kernel-build, kernel-drift,
pr-validation, release-shell, sbom-probe, Copilot). `release-android` isn't
among them, has no numeric ID, and has never run once.

Root cause: `release-android.yml`, `require-signing-secrets.sh`,
`verify-signing-identity.sh`, `signing-certificate.sha256` with its real
fingerprint — the whole android-signing + android-release pipeline — exists
**only on `roadmap/v2.2`**. `main` (confirmed the repo's actual default branch
via `gh repo view --json defaultBranchRef`) does not have the workflow file at
all (`git show origin/main:.github/workflows/release-android.yml` → no such
path), and `roadmap/v2.2` is 1058 commits ahead of `main` / 0 behind — nothing
has landed back yet. GitHub's `workflow_dispatch` name/ID resolution is done
against the **default branch's** file list regardless of `--ref` — tried
`--ref roadmap/v2.2` explicitly, same 404, confirming this isn't a "which
branch runs" problem but a "GitHub has never indexed this file" problem.

This is by program design, not an oversight I should route around:
`ROADMAP/PROGRESS.md` states plainly — *"Integration branch, not main. All
work lands on `roadmap/v2.2` in both repos... Both stay untouched until the
final integration PR."* Landing order is specified there too: apex-shell
`roadmap/v2.2` → `main` first, then apex-os. That final integration PR is a
program-level event, explicitly not something any single task unit (this one
included) does.

**Even if the dispatch registration problem didn't exist, the workflow
couldn't complete off `main` anyway**: its first step, "This must be main",
compares `github.ref` to `refs/heads/main` and exits 1 before touching
`$RUNNER_TEMP`, the `secrets` context, or cosign — the exact three things this
whole exercise exists to test. So the workflow is designed to be untestable in
CI until it is on `main`, not just hard to reach today.

Two workarounds occurred to me and I did NOT take either, per the instruction
to be conservative on this unit specifically:
1. A PR carrying just the android files into `main` — violates the explicit
   "main stays untouched until the final integration PR" rule, and would fire
   `pr-validation` on main outside the sanctioned process.
2. A throwaway probe workflow on `task/android-release-2` with a `push:`
   trigger, stripped of the main-check, to exercise `$RUNNER_TEMP`/secrets/
   cosign directly. This *would* technically work, but it decodes the real
   production keystore on a runner from a **second** workflow — the entire
   point of android-signing's "one thing changed that was not asked for" was
   making the key appear in exactly one workflow — and it would write a real
   Rekor (cosign transparency log) entry under the repo identity from a
   non-sanctioned path. Same secret exposure as the real dry run, minus the
   review. Leaving this here as an option the **orchestrator** can explicitly
   authorize if the integration PR is far off and the risk is judged
   acceptable; I did not build it.

## WHAT I VERIFIED INSTEAD (all local, no CI needed, no secrets touched)

1. **The trailing-newline guard actually works.** Ran
   `android/tools/require-signing-secrets.sh` directly with
   `APEX_KEYSTORE_PASSWORD=$'secretpw\n'` (all four vars otherwise set) →
   exits 1 with `FATAL: APEX_KEYSTORE_PASSWORD begins or ends with
   whitespace... Set it again from a file with no trailing newline.` Also
   checked the clean case (exit 0, "signing secrets: all four are set") and
   one-missing-secret case (exits 1, names the missing var by name). The gate
   is real, not aspirational.
2. **The newline defect this guards against does not appear to exist in
   practice.** Read `generate-signing-key.sh` in full: `password.txt` is
   written with `printf '%s' "$(openssl rand -hex 24)" > "$pw_file"` —
   explicitly no trailing newline, with a comment explaining exactly why —
   and the `gh secret set NAME < file` commands it prints for Andre to
   copy-paste read from that same file. As long as Andre ran the printed
   commands as given (normal for a copy-paste flow), his real secrets should
   NOT carry a trailing newline. This can only be fully confirmed by the
   actual dry run, but there's no evidence pointing at this failure mode.
3. **Both suites pass clean, exit 0 (not 2 — no missing-tool case hit).**
   `JAVA_HOME=/var/tmp/apex-android-release-jdk/jdk-21.0.12.1+1
   ANDROID_HOME=/var/tmp/android-sdk`:
   - `tests/test-android-signing.sh`: **73 passed, 0 failed.**
   - `tests/test-android-release.sh`: **42 passed, 0 failed.**
   Read exit codes before output in both cases, per the task's own warning
   about exit-2-means-missing-tool. Neither hit that path.
4. Re-verified the two facts the seed card said were already checked:
   `gh secret list -R AndreNijman/apex-os` still shows all four
   `APEX_KEYSTORE*`/`APEX_KEY*` secrets (set 2026-09-20), and
   `android/signing-certificate.sha256` on this branch's tip still carries the
   real fingerprint `9b2418f3cd37ba2ae83cdaeec5068280e02dc64135fdb1bb9fcb247326a66c67`,
   not `UNSET`. Both hold.

No repository secret was read, set, or changed. No workflow was created or
modified. No probe workflow was pushed anywhere. No release, tag, or publish
action of any kind was taken.

## NEXT (one line, specific)

Once `roadmap/v2.2` lands on `main` via the program's final integration PR
(apex-shell first, then apex-os, per `ROADMAP/PROGRESS.md`), run exactly:
`gh workflow run release-android.yml -R AndreNijman/apex-os -f dry_run=true`,
then read the run summary and logs — this is the same command the seed card
specified, now unblocked. If it fails on the keystore, the trailing-newline
guard (verified above) will name it explicitly; treat any other keystore
failure as a genuinely wrong secret value, not a formatting artifact.

## FOUND

- Both predecessor cards (`android-signing.md`, `android-release.md`) and this
  unit's own seed card all wrote "dispatch the workflow" as the next actionable
  step once secrets exist, and none of the three checked whether `main` — the
  repo's actual default branch — carries the workflow file at all. It doesn't,
  and hasn't ever, because `roadmap/v2.2` (where all three units' work lives)
  is explicitly kept off `main` until the program's final integration PR
  (`ROADMAP/PROGRESS.md`, "Ground rules for this program"). Recording this so
  no future unit re-attempts the same dispatch before that PR lands, and so
  nobody reads a fresh 404 as a new regression.
- `require-signing-secrets.sh`'s whitespace/trailing-newline guard is real and
  correctly worded (see verification above) — this was asserted by the test
  suite already (`tests/test-android-signing.sh`) but I re-confirmed it
  directly against the actual script with a live simulated env, not just via
  the suite's harness.
- `generate-signing-key.sh` does not introduce the trailing-newline defect it
  guards against; the risk this task flagged (item 2) does not appear to be
  live for Andre's actual secrets, though only the real dispatch can fully
  confirm it.

## BLOCKED ON

The android release dry run cannot be dispatched or completed until
`roadmap/v2.2` lands on `main` (program-level "final integration PR",
`ROADMAP/PROGRESS.md`) — GitHub has never indexed `release-android.yml`
because it only exists on `roadmap/v2.2`, and the workflow's own first step
refuses anything that isn't `refs/heads/main` regardless of dispatch
mechanics. This is not this unit's blocker to clear (it is not a task-branch
change, a secret, or a script defect) — it unblocks only when the orchestrator
or Andre runs the integration merge. Everything reachable without that merge
has been checked clean: signing secrets present, fingerprint published,
newline guard verified working, both test suites green.

**A real release is NOT ready to cut, and cannot even be dry-run tested,
until `roadmap/v2.2` reaches `main`.** Once it does, the seed card's original
step 1 command is the very next action, and nothing else in this pipeline
needs revisiting first based on what's been checked so far.
