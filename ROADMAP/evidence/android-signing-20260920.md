# android-signing — evidence, 2026-09-20

The unit's claim is that a release with no signing key, or with a key this
repository has not published, **refuses**. Nothing below was published: no
release was created, no tag moved, no repository secret was set, and no
production key exists. Every key made here was a throwaway and was shredded.

## What was measured, not assumed

Local, on the L16. `keytool` from `/usr/bin/keytool` (OpenJDK 21), Android
build-tools **36.0.0** at `/var/tmp/android-sdk`.

1. **A PKCS12 keystore cannot hold a separate key password.**
   `keytool -genkeypair -storetype PKCS12 … -storepass X -keypass Y` prints
   *"Warning: Different store and key passwords not supported for PKCS12
   KeyStores. Ignoring user-specified -keypass value."* and generates anyway.
   Consequence, documented rather than left to be discovered in CI:
   `APEX_KEY_PASSWORD` must hold the same value as `APEX_KEYSTORE_PASSWORD`.
2. **`keytool -exportcert | sha256sum` is byte-for-byte the digest apksigner
   prints.** Both gave
   `01cf168cde23587040fc3e7a19cb2c407f14b58cf89086eb261bf01e2a0db83c` for the
   same throwaway certificate; `keytool -list -v` prints the same bytes
   uppercase and colon-separated, and `openssl x509 -fingerprint -sha256` a
   third way. The pin file therefore normalises all three spellings.
3. **`keytool` accepts `-storepass:file` and `-storepass:env`.** Confirmed by
   generating and reading back a keystore with neither password ever appearing
   in an argument — `ps` shows the command line of every process on the machine
   to every other user.
4. **A 707-byte APK is enough to test a signature.** `aapt2 link` with a
   four-line manifest produces one in under a second, apksigner signs it, and
   `apksigner verify --print-certs` reads the certificate back. A bare zip is
   NOT enough: apksigner signs it but refuses to verify it with *"Missing
   AndroidManifest.xml"*, even with `--min-sdk-version` supplied.
5. **Rotation, in detail, because the doc had to be right.**
   `apksigner rotate --old-signer … --new-signer …` produced a 3.1 kB
   `lineage.bin` listing both certificates with their capabilities. Signing
   with `--lineage` and **only the new key fails**: *"v2 signing enabled but
   the oldest signer in the SigningCertificateLineage is missing. Please
   provide the oldest signer to enable v2 signing."* Signing with both
   (`--ks old … --next-signer --ks new … --lineage`) succeeds, and the result
   verifies with v3 **and v3.1**, reporting two signers with SDK ranges:
   the new key for `minSdkVersion=33` upwards, the old key for `24..32`. That
   is why `android/signing-certificate.sha256` accepts more than one
   fingerprint and why the identity check requires set equality rather than
   "at least one matches".

## How the refusal was proved without publishing anything

`tests/test-android-signing.sh` — **73 assertions, 0 failures**, run both as
`./tests/test-android-signing.sh` and as `bash -e ./tests/test-android-signing.sh`
(GitHub invokes a script the second way). It generates two real keystores with
keytool, links and signs two real APKs, and drives the gates directly:

* `require-signing-secrets.sh` refuses with each of the four secrets missing in
  turn, and names the one that is missing; refuses with none set; refuses a
  password carrying a trailing newline (the `gh secret set NAME < file` paste
  mistake); warns but does not refuse when the two passwords differ; and never
  prints a secret's value.
* `release-android.yml` is parsed as YAML and asserted structurally: the gate
  step exists, carries **no `if:`**, receives all four secrets through `env:`,
  and runs **before** both the keystore materialisation and the build. No step
  condition reads the `secrets` context (which is always false there), and no
  `run:` block approaches the 21 000-character cap.
* `verify-signing-identity.sh` refuses an `UNSET` fingerprint, a missing pin
  file, a malformed one, a keystore holding a different key, an APK signed by a
  different key, an APK missing one of the published signers (a half-finished
  rotation), an unsigned APK, and — the case that matters most here — **a run
  with no apksigner available**, rather than reporting that it found no problem.
* `release-artifacts.sh` was run end to end against a real throwaway keystore
  in a fixture repository whose published fingerprint was a *different* key: it
  refused, and the refusal arrived before a single line of Gradle. With the
  matching fingerprint the same run passed the identity check and continued.
* `generate-signing-key.sh` was **run for real** (into a temporary directory,
  with a fixture repo): it refuses under `GITHUB_ACTIONS`, refuses a `/tmp`
  destination (tmpfs is RAM), refuses a destination inside a git checkout, and
  otherwise writes `release.jks` + `password.txt` + `keystore.base64` +
  `BACKUP-README.txt`, all 0600, with a 48-hex-character password and no
  trailing newline in the file that becomes a GitHub secret. Its output prints
  the fingerprint, prints the `gh secret set` commands reading from files, says
  a GitHub secret cannot be read back, and never prints the password. The key
  it generated then satisfied the same gate the release runs, and the base64 it
  wrote decoded back to exactly the keystore.

## Mutation-checked — seven mutants, seven killed

Each gate was broken on its own, the suite re-run, and the failing assertions
recorded. The worktree was committed first and restored by `git checkout` after
each; a first attempt against an **uncommitted** worktree reverted the code
under test for tracked files and silently left the mutant in place for untracked
ones, which is worth knowing before anybody repeats it.

| mutation | suite | killed by |
| --- | --- | --- |
| workflow no longer runs the secrets gate | 68/5 | "nothing in release-android.yml runs the secrets gate" + 4 ordering assertions |
| the gate carries an `if:` | 72/1 | "the secrets gate has an if: — a skipped step counts as success here" |
| the refusal script never refuses | 69/4 | all four "X missing refuses the release" |
| the identity check compares but does not exit | 69/4 | wrong keystore, wrong APK, half-rotation, and the before-Gradle ordering |
| `release-artifacts.sh` stops checking the keystore | 69/4 | the run reached `./gradlew` — which is what the check exists to prevent |
| `pr-validation.yml` materialises the key again | 72/1 | "pr-validation.yml still materialises the signing key on every PR runner" |
| the pin disagrees with the published docs | 71/2 | README.md and docs/android-signing.md both named |

## What is still unproved

* **No release has been dispatched.** `$RUNNER_TEMP`, the `secrets` context and
  cosign keyless are the three things no local run can exercise; the dry run is
  the next step and it needs the secrets to exist.
* **No production key exists**, by design — generation is Andre's one act.
* **Rotation is documented, not implemented.** AGP has no lineage support, so a
  rotated release would be an `apksigner` step outside the Gradle build. The
  measurements above are real; the pipeline change is not written.
