# android-signing — decide the APK signing key, and make every consequence a gate

ask: Andre delegated it — *"you decide signing key or whatever, thats your job"*.
  The orchestrator decided; this unit implemented the decision.
repo: apex-os
worktree: `/var/tmp/apex-work/wt-android-signing`, branch `task/android-signing`
  (**pushed, not merged**; tip `d433cbd6`, four commits)
owns: `android/tools/{generate-signing-key,require-signing-secrets,verify-signing-identity}.sh`,
  `android/signing-certificate.sha256`, `docs/android-signing.md`,
  `tests/test-android-signing.sh`, the signing parts of
  `.github/workflows/release-android.yml` and of pr-validation's android job,
  the README's fingerprint block. Does NOT own `installer/**`, the boot path,
  `Containerfile.*`, the kernel.

## STATUS — COMPLETE. Waiting on one act only Andre can perform.

Four commits:

* `7a7136f9` merge `origin/task/android-release` — **this branch carries that
  unit's work too.** `roadmap/v2.2` does not have the Android release pipeline,
  and this unit is entirely about it, so the branch was based on
  `roadmap/v2.2` @ `f9e1c81b` and android-release was merged in (cleanly).
  Landing `task/android-signing` therefore lands `task/android-release` as
  well; landing android-release first is also fine and this still merges.
* `a7aaeed5` the decision, the scripts, the gates, the docs, the suite
* `49a2e9ed` `ROADMAP/evidence/android-signing-20260920.md`
* `d433cbd6` the android job installs PyYAML rather than assuming the runner has it

## THE DECISION, as implemented

Written in full in `docs/android-signing.md`. In one paragraph: the key is
generated **offline on Andre's machine, once, never in CI**; the authoritative
copy is **his and offline**, in two places, one not network-attached; GitHub
holds a **working copy, not the backup** — because **a GitHub secret cannot be
read back**, so the copy there goes on signing releases while being permanently
unretrievable. It has the shape of redundancy without being redundancy, and
that is the failure that would actually happen. Mechanics copy the Secure Boot
key in `build-image.yml` (base64 secret → `$RUNNER_TEMP` 0600 → `shred` in
`if: always()`); the backup posture deliberately does not, because a lost
Secure Boot key means future images ship unsigned kernels while a lost APK key
means **every existing install can never be upgraded**.

## WHAT ANDRE RUNS — one command

```
cd <an apex-os checkout>
android/tools/generate-signing-key.sh
```

It writes `~/apex-android-signing/{release.jks,password.txt,keystore.base64,
BACKUP-README.txt}` (all 0600), publishes the certificate fingerprint into
`android/signing-certificate.sha256`, `README.md` and `docs/android-signing.md`,
and prints the four `gh secret set` commands — reading from files, so no
password reaches shell history. It refuses to run in CI, refuses `/tmp` (tmpfs
is RAM here), refuses a destination inside a git checkout, and never prints the
password.

**BACK UP `~/apex-android-signing` FIRST** — all four files together, two places
that are not GitHub, one not network-attached. Then set the secrets, commit the
fingerprint files, then `gh workflow run release-android.yml -f dry_run=true`.

## WHAT HAPPENS TODAY, WITH NO KEY

`release-android.yml` refuses in its first minute, twice over and with no `if:`
on either step: `require-signing-secrets.sh` names each missing secret
individually, and `verify-signing-identity.sh --published` refuses while
`android/signing-certificate.sha256` says `UNSET`. There is no fallback to a
debug or throwaway key, deliberately: an APK signed by a key nobody keeps,
reaching one phone, freezes that install forever.

## THE OTHER HALF: the WRONG key

`verify-signing-identity.sh` compares the keystore against the published
fingerprint **before** Gradle and the built APK **after** it. "The build signed
something" was otherwise the only evidence anybody had about what signed it,
and a keystore secret quietly replaced passes every other gate in the pipeline.
After a rotation an APK carries two certificates, so the pin file takes one or
more and the comparison is set equality.

## ONE THING CHANGED THAT WAS NOT ASKED FOR

`pr-validation.yml`'s android job **no longer signs at all**. It used to decode
`APEX_KEYSTORE_BASE64` into the WORKSPACE whenever the secret was visible —
nothing consumed it, but the day the production key is set it would appear on a
runner for every push and every same-repo PR, in the checkout directory. The
key now appears in exactly one workflow, and the job asserts no keystore exists
anywhere in the workspace.

## PROVED WITHOUT PUBLISHING

`tests/test-android-signing.sh`: **73 assertions, 0 failures**, wired into
pr-validation's android job and into the android path selector. Real keystores
from keytool, a real 707-byte APK from aapt2 signed by apksigner, the workflow
parsed structurally as YAML rather than grepped. **Seven mutants, seven
killed** — the table is in `ROADMAP/evidence/android-signing-20260920.md`
along with every measurement (PKCS12 ignoring a separate key password, the
apksigner/keytool digest equivalence, the rotation behaviour).

No release was created, no tag moved, no repository secret was set, no
production key exists. Every keystore made while testing was a throwaway and
was shredded; `find` over the scratch and the worktree returns no `*.jks`,
`*.p12` or `password*`.

## NEXT

1. **Andre runs `android/tools/generate-signing-key.sh`** and backs the
   directory up before doing anything else. Nothing else in this unit can move
   until then, and that is the correct blocker.
2. **After the secrets exist: `gh workflow run release-android.yml -R
   AndreNijman/apex-os -f dry_run=true`** and read the summary. `$RUNNER_TEMP`,
   the `secrets` context and cosign keyless are the three things no local run
   can exercise. Only then turn `dry_run` off.
3. **If the dry run fails on the keystore**, suspect a trailing newline in
   `APEX_KEYSTORE_PASSWORD` first — `gh secret set NAME < file` stores the
   file's newline. `require-signing-secrets.sh` catches it and says so, but
   only if the value really has one; a password that is merely wrong looks the
   same from inside Gradle.
4. **First CI run: if `tests/test-android-signing.sh` exits 2, that is a
   missing tool on the runner, not a gate failing.** The suite refuses to skip
   its own prerequisites, so it FATALs without PyYAML, apksigner, aapt2 or a
   `platforms/*/android.jar`. The android job installs `python3-yaml` and
   `platforms;android-36` in earlier steps, and build-tools comes with the
   image — but read the exit code before reading the assertions.
5. **Rotation is documented and NOT implemented.** AGP has no
   `SigningCertificateLineage` support, so a rotated release is an `apksigner`
   step outside the Gradle build and `lineage.bin` becomes a permanent fifth
   secret. Measured facts are in the doc; the pipeline change is not written,
   and writing it before it is needed would be writing it untested.
6. `docs/android-app.md`'s open-decision section is gone, replaced by a pointer.
   Anything that still says the decision is open is stale.

## TRAPS PAID FOR HERE

* **`tr 'A-F' 'a-f'` turns `UNSET` into `UNSeT`.** The sentinel then read as a
  malformed fingerprint and the wrong refusal fired. Lowercase the whole line.
* **Mutating an uncommitted worktree destroys the work.** `git checkout --` on
  a tracked file reverts to the INDEX — which, before the first commit, is the
  pre-edit state — and on an untracked file it silently does nothing, leaving
  the mutant in place. Both happened. Commit, then mutate.
* `mktemp -d` lands in `/tmp`, which is a 15 GB tmpfs here, and
  `generate-signing-key.sh` refuses to put a key on one. The suite builds its
  work directory under `/var/tmp`.
* A grep for `verify-signing-identity.sh --keystore` matches nothing, because
  the call is written `"$here/verify-signing-identity.sh" --keystore`. An
  ordering assertion reported a check as missing from a file that had it.
