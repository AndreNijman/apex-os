# android-release-2 — dry-run the release, now that Andre's part is done

items: none (queue id `android-release` stays open; this is a direct continuation)
repo: apex-os
worktree: /var/tmp/apex-work/wt-android-release-2
branch: task/android-release-2, cut fresh from roadmap/v2.2 @ 76aa2b95 (2026-09-21)
No code changes made this round. Branch tip is still 76aa2b95, exactly matching
`origin/task/android-release-2` — this branch carries zero commits of its own
and can be dropped without losing anything if that's ever useful.

STATUS: the dry run cannot be dispatched — structural, program-level blocker,
not a bug in any script. See BLOCKED ON. Everything reachable without a real
CI dispatch has been checked and is clean.

## NEXT
Once roadmap/v2.2 lands on main: `gh workflow run release-android.yml -R AndreNijman/apex-os -f dry_run=true`, then read the run summary and logs.

## DONE

1. Re-verified the seed card's two pre-checked facts myself: `gh secret list
   -R AndreNijman/apex-os` still shows all four `APEX_KEYSTORE*`/`APEX_KEY*`
   secrets set 2026-09-20, and `android/signing-certificate.sha256` on this
   branch's tip still carries the real fingerprint
   `9b2418f3cd37ba2ae83cdaeec5068280e02dc64135fdb1bb9fcb247326a66c67`, not
   `UNSET`. Both hold.
2. Attempted the dispatch (`gh workflow run release-android.yml -R
   AndreNijman/apex-os -f dry_run=true`, then again with `--ref roadmap/v2.2`)
   — both 404: `workflow release-android.yml not found on the default
   branch`. Confirmed via `gh api repos/AndreNijman/apex-os/actions/workflows`
   that only 9 workflows are registered and `release-android` isn't one of
   them (no numeric ID, never run once). Root-caused, not just observed: see
   BLOCKED ON.
3. Verified `require-signing-secrets.sh`'s trailing-newline guard directly
   against the real script (not just via the test suite's harness): fed it
   `APEX_KEYSTORE_PASSWORD=$'secretpw\n'` with the other three vars set →
   exits 1, `FATAL: APEX_KEYSTORE_PASSWORD begins or ends with whitespace...
   Set it again from a file with no trailing newline.` Also checked the clean
   case (exit 0, "signing secrets: all four are set") and a missing-secret
   case (exit 1, names the missing var). The guard is real, not aspirational.
4. Read `generate-signing-key.sh` in full: `password.txt` is written via
   `printf '%s' "$(openssl rand -hex 24)" > "$pw_file"` — no trailing
   newline, deliberately — and the `gh secret set NAME < file` commands it
   prints read from that same file. See FOUND for what the actual secret
   timestamps say about whether Andre followed this exactly.
5. Ran `android/tools/verify-signing-identity.sh --published` directly
   (workflow's second gate, needs no secrets) against the real pin file in
   this worktree: exit 0, `published signing certificate:
   9b2418f3cd37ba2ae83cdaeec5068280e02dc64135fdb1bb9fcb247326a66c67`.
6. Ran both suites with `JAVA_HOME=/var/tmp/apex-android-release-jdk/jdk-21.0.12.1+1
   ANDROID_HOME=/var/tmp/android-sdk`, exit code read before output in both
   cases (neither hit the exit-2-means-missing-tool case):
   - `tests/test-android-signing.sh`: **73 passed, 0 failed**, exit 0.
   - `tests/test-android-release.sh`: **42 passed, 0 failed**, exit 0.

No repository secret was read back, set, or changed. No workflow file was
created, modified, or pushed anywhere — including no throwaway probe
workflow (considered, rejected, see BLOCKED ON). No release, tag, or publish
action of any kind was taken.

## IN PROGRESS
-

## FOUND

- **Both predecessor cards and this unit's own seed card assumed the dispatch
  would work and none checked `main`.** `android-signing.md` and
  `android-release.md` both wrote "once secrets exist, dispatch
  release-android.yml" as the next step; so did my seed card. None of the
  three noticed that `main` — confirmed via `gh repo view --json
  defaultBranchRef` to be the repo's real default branch — has never had the
  workflow file at all (`git show origin/main:.github/workflows/release-android.yml`
  → no such path), because the whole android pipeline lives only on
  `roadmap/v2.2`, which is 1058 commits ahead of `main` / 0 behind. Recording
  this so no future unit re-attempts the same dispatch and reads a fresh 404
  as a new regression.
- **The `gh secret list` timestamps show the alias was set differently from
  the other three.** `APEX_KEYSTORE_BASE64`, `APEX_KEYSTORE_PASSWORD`, and
  `APEX_KEY_PASSWORD` were all set within the same 2 seconds (14:59:30-32),
  consistent with copy-pasting the three `gh secret set ... < file` lines
  `generate-signing-key.sh` prints as one block. `APEX_KEY_ALIAS` was set 17
  minutes earlier (14:42:56), in what must have been a separate action — the
  script prints it last and differently (`printf '%s' '$alias_name' | gh
  secret set APEX_KEY_ALIAS`, a pipe, not a file redirect). This doesn't
  prove a defect: `require-signing-secrets.sh`'s whitespace loop (verified
  above) covers `APEX_KEY_ALIAS` too, so if it was set some other way (e.g.
  `echo apex-release | gh secret set ...`, which appends a newline) that will
  be caught and named at dispatch time, not silently mis-signed. Flagging it
  so if the first real dry run refuses with `APEX_KEY_ALIAS begins or ends
  with whitespace`, whoever reads the log knows why and the fix is
  `printf '%s' apex-release | gh secret set APEX_KEY_ALIAS -R AndreNijman/apex-os`
  — no need to re-investigate from scratch.
- `generate-signing-key.sh`'s own printed instructions do not introduce the
  newline defect for the three password/keystore secrets — confirmed by
  reading the script, not just inferred.

## BLOCKED ON

The android release dry run cannot be dispatched or completed until
`roadmap/v2.2` lands on `main` via the program's "final integration PR"
(`ROADMAP/PROGRESS.md`, "Ground rules for this program": *"Integration
branch, not main. All work lands on roadmap/v2.2 in both repos... Both stay
untouched until the final integration PR"* — landing order specified there
too: apex-shell to main first, then apex-os). Two independent reasons stack:

1. GitHub's `workflow_dispatch` name/ID resolution reads the **default
   branch's** file list regardless of `--ref` — confirmed by trying `--ref
   roadmap/v2.2` explicitly and getting the identical 404. Because
   `release-android.yml` has never existed on `main` and has never run once,
   GitHub has never indexed it: no numeric workflow ID exists to fall back
   to.
2. Even if dispatch somehow succeeded, the workflow's own first step ("This
   must be main") exits 1 the instant `github.ref != refs/heads/main` —
   before `$RUNNER_TEMP`, the `secrets` context, or cosign are touched. The
   workflow is designed to be untestable in CI on any ref but `main`, not
   merely hard to reach today.

Two workarounds occurred to me; neither was taken, per the instruction to be
conservative on this unit specifically:
- A PR carrying just the android files into `main` — breaks the explicit
  "main stays untouched until the final integration PR" rule.
- A throwaway `push:`-triggered probe workflow on `task/android-release-2`
  with the main-check stripped, to exercise `$RUNNER_TEMP`/secrets/cosign
  directly. Technically works, but decodes the real production keystore from
  a **second** workflow — android-signing's whole point was making the key
  appear in exactly one — and writes a real cosign Rekor entry under the repo
  identity from an unsanctioned path. Same secret exposure as the real dry
  run, without the review. Left here as an option the orchestrator can
  explicitly authorize if the integration PR is far off; not built.

This is not this unit's blocker to clear — it is not a task-branch change, a
secret, or a script defect, and unblocks only when the orchestrator or Andre
runs the integration merge. Everything reachable without that merge is
checked clean: signing secrets present (see FOUND re: the alias), fingerprint
published and verified via both `--published` gate scripts, newline guard
verified working end-to-end, both test suites green (73/73, 42/42).

**A real release is NOT ready to cut, and cannot even be dry-run tested,
until `roadmap/v2.2` reaches `main`.** Once it does, the `## NEXT` command
above is the very next action; nothing else in this pipeline needs
revisiting first based on what's been checked so far.
