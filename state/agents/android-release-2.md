# android-release-2 — dry-run the release, now that Andre's part is done

items: none (queue id `android-release` stays open; this is a direct continuation)
repo: apex-os
worktree: /var/tmp/apex-work/wt-android-release-2
branch: task/android-release-2, cut fresh from roadmap/v2.2 @ 76aa2b95 (2026-09-21)

android-signing's own branch already LANDED in round 35 (key decision
implemented, 73 assertions, 7/7 mutants killed) — do not redo its work. Read
ROADMAP/state/agents/android-signing.md AND ROADMAP/state/agents/android-release.md
for the full history.

**CHECKED THIS ROUND, do not re-derive**: the predecessor cards both say NEXT
item 1 is "Andre generates the signing key" and describe it as a blocker only
he can clear. He already has — `gh secret list -R AndreNijman/apex-os` shows
APEX_KEYSTORE_BASE64, APEX_KEYSTORE_PASSWORD, APEX_KEY_ALIAS, APEX_KEY_PASSWORD
all set 2026-09-20T14:4x-14:5x, and `android/signing-certificate.sha256` on the
roadmap/v2.2 tip already carries a real fingerprint
(9b2418f3cd37ba2ae83cdaeec5068280e02dc64135fdb1bb9fcb247326a66c67), not UNSET.
The blocker is cleared. Your job starts at NEXT item 2 of android-signing.md.

## NEXT (dispatched with, fill in as you go)
1. `gh workflow run release-android.yml -R AndreNijman/apex-os -f dry_run=true`
   and read the summary carefully — $RUNNER_TEMP, the secrets context, and
   cosign keyless are the three things no local run can exercise.
2. If the dry run fails on the keystore, suspect a trailing newline in
   APEX_KEYSTORE_PASSWORD first (`gh secret set NAME < file` stores the
   file's newline) — require-signing-secrets.sh should catch and name it.
3. If `tests/test-android-release.sh` or `test-android-signing.sh` exits 2 in
   CI, that's a missing tool on the runner (PyYAML/apksigner/aapt2/platforms
   jar), not a gate failing — read the exit code before the assertions.
4. Only once the dry run is clean, consider whether a real (non-dry-run)
   release is appropriate — that is a bigger decision (moving a real tag,
   publishing a real page) and should be flagged rather than done unprompted
   given this is autonomous overnight work; a dry run and a clean report back
   is the safe stopping point for this round.
5. Do NOT touch signing rotation (documented, not implemented — writing it
   before it's needed means writing it untested, per android-signing.md item 5).

## FOUND
-

## BLOCKED ON
- nothing yet
